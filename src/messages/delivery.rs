use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::lead::state_dir;
use crate::inbox::Via;
use crate::peers::{self, Session};
use crate::questions;
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use super::{HAND_KIND, Lock, Message, NOTICE_KIND, PROMPT_KIND, ROTATE_BYTES, RULES_KIND, USER_LABEL, append_in, batch_text, holds, lead_hold_in, load, load_in, new_id, note_lead_prompt_in, now};

// Nothing is ever typed into a question: a session waiting for an answer, or with no Claude, gets the message later.
pub fn hold_reason(session: &Session) -> Option<&'static str> {
    match (&session.pane, session.status.as_deref()) {
        (None, _) => Some("has no Claude running"),
        (Some(_), Some("blocked")) => Some("is waiting for an answer"),
        _ => None,
    }
}

// Every line after the header is indented, so no text a session sends can start a line with [from you].
pub fn delivered(from: &str, text: &str) -> String {
    format!("[from {from}] {}", indented(text))
}

// Text with every line break, whatever character spells it, turned into a newline and two spaces.
pub fn indented(text: &str) -> String {
    let text: String = text.chars().map(|character| if matches!(character, '\r' | '\u{0b}' | '\u{0c}' | '\u{85}' | '\u{2028}' | '\u{2029}') { '\n' } else { character }).collect();
    text.split('\n').collect::<Vec<_>>().join("\n  ")
}

pub(crate) fn typed_form(message: &Message) -> String {
    if message.kind.as_deref() == Some(PROMPT_KIND) { message.text.clone() } else { typed_text(&message.from, &message.to, &message.text) }
}

// What of a message is typed whether it goes alone or in a batch, so a check for it finds it in either.
pub(super) fn part(message: &Message) -> String {
    if message.kind.as_deref() == Some(PROMPT_KIND) { message.text.clone() } else { delivered(&message.from, &message.text) }
}

pub const LEAD_CUE: &str = "(kebbie: work silently; end in Done / Going on / Needs you, or one line if nothing needs the user)";

// A message that starts a lead turn carries the cue for how that turn ends; the user's own word does not.
pub(super) fn typed_text(from: &str, to: &str, text: &str) -> String {
    let typed = delivered(from, text);
    if !cued(from, to) {
        return typed;
    }
    format!("{typed}\n  {LEAD_CUE}")
}

pub(super) fn cued(from: &str, to: &str) -> bool {
    to == LEAD_LABEL && from != USER_LABEL && from != LEAD_LABEL
}

pub const DRAFT_HOLD: &str = "has a draft in its input box";
pub const EARLIER_TYPING: &str = "is still taking the messages kebbie typed for it just now";
pub(super) const TYPED_FOR: u64 = 10;

pub(super) fn typed_file(dir: &Path, address: &str) -> PathBuf {
    dir.join("typed").join(address)
}

pub(super) fn note_typed(dir: &Path, address: &str) {
    let _ = crate::state::write(&typed_file(dir, address), now().to_string());
}

// The box seen as a draft right after kebbie typed into it, in this pass or a recent one, holds kebbie's own text.
fn kebbies_own<'a>(dir: &Path, address: &str, why: &'a str) -> &'a str {
    let recent = fs::read_to_string(typed_file(dir, address)).ok().and_then(|at| at.trim().parse::<u64>().ok()).is_some_and(|at| now().saturating_sub(at) <= TYPED_FOR);
    if why == DRAFT_HOLD && recent { EARLIER_TYPING } else { why }
}

pub(super) type TypeInto<'a> = &'a mut dyn FnMut(&str, &str, &str) -> std::result::Result<Via, crate::typing::Refused>;

fn deliver(session: &Session, text: &str, type_into: TypeInto) -> std::result::Result<Via, &'static str> {
    if let Some(reason) = hold_reason(session) {
        return Err(reason);
    }
    let pane = session.pane.as_deref().unwrap_or_default();
    match type_into(pane, text, &session.address) {
        Ok(via) => Ok(via),
        Err(crate::typing::Refused::Draft) => Err(DRAFT_HOLD),
        Err(crate::typing::Refused::Unreadable) => Err("shows no input box to type into"),
        Err(crate::typing::Refused::Failed(error)) if error.contains("agent_blocked") => Err("is waiting for an answer"),
        Err(crate::typing::Refused::Failed(_)) => Err("could not be reached"),
    }
}

// A task with a question open waits for its answer: a message typed into it would start a turn and drop the question, so only the answer itself (about that question) reaches it.
pub fn question_hold(about: Option<&str>, open_question: Option<&str>) -> Option<&'static str> {
    open_question.filter(|id| about != Some(*id)).map(|_| "waits for the user's answer to its question")
}

// A held message to the lead about a question is dropped once the question is settled.
pub(super) fn stale(message: &Message, open: &[&Message]) -> bool {
    message.to == LEAD_LABEL && message.about.as_ref().is_some_and(|id| !open.iter().any(|question| &question.id == id))
}

fn flush_held(sessions: &[Session]) -> Result<usize> {
    flush_held_in(&state_dir()?, sessions, &mut crate::inbox::prompt, &heard, &rules_check_wanted)
}

fn note_kebbie_typed_lead(dir: &Path, address: &str) {
    if address == LEAD_LABEL {
        let _ = note_lead_prompt_in(dir, "[from kebbie]", now());
    }
}

// A rules check is wanted while its task still waits for one, a hand-over always; either only while its Claude has not taken it since it was first typed.
fn rules_check_wanted(message: &Message, session: &Session) -> bool {
    let rules = message.kind.as_deref() == Some(RULES_KIND);
    (!rules || crate::tasks::rules_check_task(&message.text).is_some_and(crate::tasks::awaits_check))
        && !session.pane.as_deref().is_some_and(|pane| crate::typing::taken_by(pane, &part(message), message.at * 1000))
}

fn heard(address: &str, since: u64) -> bool {
    let Some((task, repo)) = address.split_once('@') else { return false };
    let Some(repo) = crate::repo::checkout_named(repo) else { return false };
    crate::tasks::slots(&repo).ok().and_then(|slots| slots.into_iter().find(|slot| slot.task.as_deref() == Some(task)))
        .and_then(|slot| crate::tasks::transcript(&repo, &slot)).and_then(|(agent, file)| Some((agent, fs::read_to_string(file).ok()?)))
        .is_some_and(|(agent, text)| typed_by_user_since(agent, &text, since))
}

pub(super) fn typed_by_user_since(agent: crate::agent::Agent, transcript: &str, since: u64) -> bool {
    agent.user_messages_at(transcript).iter().any(|(at, text)| at.is_some_and(|at| at >= since * 1000) && !text.starts_with("[from "))
}

type Wanted<'a> = &'a dyn Fn(&Message, &Session) -> bool;
type Live<'a> = &'a dyn Fn(&Session) -> bool;

fn inbox_live(dir: &Path) -> impl Fn(&Session) -> bool + '_ {
    move |session: &Session| session.pane.as_deref().is_some_and(|pane| crate::inbox::live_in(dir, pane, &session.address, &crate::inbox::herdr_session))
}

// A rules check or a hand-over waits out its Claude's turn, and goes when its Claude took it after all, or for a rules check when a newer one for the same task or an approval made it moot.
fn sent_again_waits(dir: &Path, message: &Message, messages: &[Message], session: &Session, wanted: Wanted, live: Live) -> Result<bool> {
    let task = crate::tasks::rules_check_task(&message.text);
    let after = messages.iter().position(|other| other.id == message.id).map_or(messages.len(), |at| at + 1);
    let rules = message.kind.as_deref() == Some(RULES_KIND);
    let newer = rules && messages[after..].iter().any(|other| other.to == message.to && other.kind.as_deref() == Some(RULES_KIND) && crate::tasks::rules_check_task(&other.text) == task);
    if newer || !wanted(message, session) {
        append_in(dir, &json!({ "id": message.id, "at": now(), "state": "dropped" }), ROTATE_BYTES)?;
        return Ok(true);
    }
    if session.status.as_deref() == Some("working") && !live(session) {
        note_why(dir, message, BUSY_HOLD)?;
        return Ok(true);
    }
    Ok(false)
}

pub(super) fn flush_held_in(dir: &Path, sessions: &[Session], type_into: TypeInto, heard: &dyn Fn(&str, u64) -> bool, wanted: Wanted) -> Result<usize> {
    flush_typing_in(dir, sessions, type_into, heard, wanted, &inbox_live(dir)).map(|(delivered, _)| delivered)
}

// What it delivered, and the sessions it typed into: their boxes hold kebbie's own text for a moment.
pub(super) fn flush_typing_in(dir: &Path, sessions: &[Session], type_into: TypeInto, heard: &dyn Fn(&str, u64) -> bool, wanted: Wanted, live: Live) -> Result<(usize, HashSet<String>)> {
    let messages = load_in(dir);
    let open = questions::open_now(&messages);
    let mut delivered = 0;
    let mut held: Vec<&Message> = messages.iter().filter(|message| message.held).collect();
    held.sort_by_key(|message| message.kind.as_deref() != Some(PROMPT_KIND));
    let mut typed_into = HashSet::new();
    let mut for_lead: Vec<&Message> = Vec::new();
    for message in held {
        if stale(message, &open) {
            append_in(dir, &json!({ "id": message.id, "at": now(), "state": "dropped" }), ROTATE_BYTES)?;
            continue;
        }
        let Some(session) = sessions.iter().find(|session| session.address == message.to) else { continue };
        let open_question = questions::holding(&open, &session.address);
        if let Some(why) = question_hold(message.about.as_deref(), open_question).or_else(|| hold_reason(session)).or_else(|| lead_hold_in(dir, session, &message.from, message.kind.as_deref(), now())) {
            note_why(dir, message, why)?;
            continue;
        }
        if SENT_AGAIN.contains(&message.kind.as_deref().unwrap_or_default()) && sent_again_waits(dir, message, &messages, session, wanted, live)? {
            continue;
        }
        if session.address == LEAD_LABEL {
            for_lead.push(message);
            continue;
        }
        let prompt = message.kind.as_deref() == Some(PROMPT_KIND);
        if prompt && heard(&message.to, message.at) {
            append_in(dir, &json!({ "id": message.id, "at": now(), "state": "dropped" }), ROTATE_BYTES)?;
            crate::routines::trust_answered_in(dir, &message.to);
            continue;
        }
        match deliver(session, &typed_form(message), type_into) {
            Ok(via) => {
                if via != Via::Inbox {
                    note_typed(dir, &session.address);
                    typed_into.insert(session.address.clone());
                }
                append_in(dir, &json!({ "id": message.id, "at": now(), "state": "delivered", "via": via.word() }), ROTATE_BYTES)?;
                if prompt {
                    crate::routines::trust_answered_in(dir, &message.to);
                }
                delivered += 1;
            }
            Err(why) if !typed_into.contains(&session.address) => note_why(dir, message, kebbies_own(dir, &session.address, why))?,
            Err(_) => {}
        }
    }
    for_lead.sort_by_key(|message| message.kind.as_deref() != Some(crate::menu::KIND));
    if let Some(lead) = sessions.iter().find(|session| session.address == LEAD_LABEL).filter(|_| !for_lead.is_empty()) {
        let text = if let [one] = for_lead.as_slice() { typed_form(one) } else { batch_text(dir, &for_lead) };
        match deliver(lead, &text, type_into) {
            Ok(via) => {
                if via != Via::Inbox {
                    note_typed(dir, LEAD_LABEL);
                    typed_into.insert(LEAD_LABEL.to_string());
                }
                note_kebbie_typed_lead(dir, LEAD_LABEL);
                for message in &for_lead {
                    append_in(dir, &json!({ "id": message.id, "at": now(), "state": "delivered", "via": via.word() }), ROTATE_BYTES)?;
                }
                delivered += for_lead.len();
            }
            Err(why) => {
                for message in &for_lead {
                    note_why(dir, message, kebbies_own(dir, LEAD_LABEL, why))?;
                }
            }
        }
    }
    Ok((delivered, typed_into))
}

// A held message keeps the latest reason it waits, written only when it changes, so the board can say why.
fn note_why(dir: &Path, message: &Message, why: &str) -> Result<()> {
    if message.why.as_deref() == Some(why) {
        return Ok(());
    }
    append_in(dir, &json!({ "id": message.id, "at": now(), "state": "held", "why": why }), ROTATE_BYTES)
}

// kebbie's own requests that a Claude did not take are typed again; a tell between sessions is only reported.
const SENT_AGAIN: [&str; 2] = [RULES_KIND, HAND_KIND];
pub const BUSY_HOLD: &str = "did not take it mid-turn; it goes again when the turn ends";

// A rules check or hand-over Claude did not take is held again, so the next flush retypes it once the lead is free; other messages are only reported.
pub fn hold_again(to: &str, typed: &str) -> Result<bool> {
    let dir = state_dir()?;
    let _lock = Lock::take_in(&dir, true)?;
    hold_again_in(&dir, to, typed)
}

// A batch the lead did not take holds again each rules check it carried; one message sent alone holds only itself.
pub(super) fn hold_again_in(dir: &Path, to: &str, typed: &str) -> Result<bool> {
    let messages = load_in(dir);
    let mut parts = HashSet::new();
    let again: Vec<&Message> = messages.iter().rev()
        .filter(|message| message.to == to && !message.held && SENT_AGAIN.contains(&message.kind.as_deref().unwrap_or_default()) && crate::typing::holds_part(typed, &part(message)))
        .filter(|message| parts.insert(part(message))).collect();
    for message in &again {
        append_in(dir, &json!({ "id": message.id, "at": now(), "state": "held", "why": BUSY_HOLD }), ROTATE_BYTES)?;
    }
    Ok(!again.is_empty())
}

pub fn hold_prompt(to: &str, text: &str) -> Result<()> {
    hold_prompt_in(&state_dir()?, to, text)
}

pub(super) fn hold_prompt_in(dir: &Path, to: &str, text: &str) -> Result<()> {
    let _lock = if holds(dir) { None } else { Lock::take_in(dir, true)? };
    append_in(dir, &json!({ "id": new_id("prompt"), "at": now(), "from": "kebbie", "to": to, "text": text, "state": "held", "kind": PROMPT_KIND }), ROTATE_BYTES)
}

pub fn drop_held_prompts(to: &str) -> Result<()> {
    drop_held_prompts_in(&state_dir()?, to)
}

pub(super) fn drop_held_prompts_in(dir: &Path, to: &str) -> Result<()> {
    let _lock = if holds(dir) { None } else { Lock::take_in(dir, true)? };
    for message in load_in(dir).iter().filter(|message| message.held && message.to == to && message.kind.as_deref() == Some(PROMPT_KIND)) {
        append_in(dir, &json!({ "id": message.id, "at": now(), "state": "dropped" }), ROTATE_BYTES)?;
    }
    Ok(())
}

// A task's life ended: what kebbie still held for it, its prompt, hand-overs and the lead's rules check of it, would reach a later task by the same name.
pub fn drop_held_for_ended(address: &str) -> Result<()> {
    drop_held_for_ended_in(&state_dir()?, address)
}

pub(super) fn drop_held_for_ended_in(dir: &Path, address: &str) -> Result<()> {
    let _lock = if holds(dir) { None } else { Lock::take_in(dir, true)? };
    let kebbies = |message: &Message| match message.kind.as_deref() {
        Some(PROMPT_KIND | HAND_KIND | NOTICE_KIND) => message.to == address,
        Some(RULES_KIND) => crate::tasks::rules_check_task(&message.text) == Some(address),
        _ => false,
    };
    for message in load_in(dir).iter().filter(|message| message.held && kebbies(message)) {
        append_in(dir, &json!({ "id": message.id, "at": now(), "state": "dropped" }), ROTATE_BYTES)?;
    }
    Ok(())
}

// Held messages to sessions that are free again; the board runs this every few seconds, and so does `kebbie messages`.
pub fn flush() -> Result<usize> {
    if !load().iter().any(|message| message.held) {
        return Ok(0);
    }
    let Some(_lock) = Lock::take(false)? else { return Ok(0) };
    flush_held(&peers::sessions()?)
}

pub(super) fn send(from: &str, recipients: &[&Session], text: &str, sessions: &[Session]) -> Result<Vec<String>> {
    let _lock = Lock::take(true)?;
    send_locked(from, recipients, text, sessions, None, None)
}

// The caller holds the lock. A message about the recipient's open question is the answer to it: it goes ahead of what is held for the recipient, and closes the question.
pub fn send_locked(from: &str, recipients: &[&Session], text: &str, sessions: &[Session], about: Option<&str>, kind: Option<&str>) -> Result<Vec<String>> {
    send_locked_in(&state_dir()?, from, recipients, text, sessions, about, kind, &mut crate::inbox::prompt)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn send_locked_in(dir: &Path, from: &str, recipients: &[&Session], text: &str, sessions: &[Session], about: Option<&str>, kind: Option<&str>, type_into: TypeInto) -> Result<Vec<String>> {
    let to_lead = recipients.iter().any(|session| session.address == LEAD_LABEL) && from != USER_LABEL && from != LEAD_LABEL;
    let others: Vec<Session> = sessions.iter().filter(|session| !to_lead || session.address != LEAD_LABEL).cloned().collect();
    let (_, typed_into) = flush_typing_in(dir, &others, type_into, &heard, &rules_check_wanted, &inbox_live(dir))?;
    let mut report = Vec::new();
    for (index, session) in recipients.iter().enumerate() {
        let messages = load_in(dir);
        let open = questions::open_now(&messages);
        let question = open.iter().find(|question| question.from == session.address);
        let earlier_held = messages.iter().any(|message| message.held && message.to == session.address);
        let answering = question.is_some_and(|question| about == Some(question.id.as_str()));
        let outcome = match question_hold(about, questions::holding(&open, &session.address)) {
            Some(reason) => Err(reason),
            None if earlier_held && !answering => Err("still has earlier messages held"),
            None => match lead_hold_in(dir, session, from, kind, now()).map_or_else(|| deliver(session, &typed_text(from, &session.address, text), type_into), Err) {
                Err(DRAFT_HOLD) if typed_into.contains(&session.address) => Err(EARLIER_TYPING),
                Ok(via) => {
                    if via != Via::Inbox {
                        note_typed(dir, &session.address);
                    }
                    note_kebbie_typed_lead(dir, &session.address);
                    Ok(via)
                }
                Err(why) => Err(kebbies_own(dir, &session.address, why)),
            },
        };
        let id = new_id(&index.to_string());
        let mut record = json!({ "id": id, "at": now(), "from": from, "to": session.address, "text": text, "state": if outcome.is_ok() { "delivered" } else { "held" } });
        match outcome {
            Ok(via) => record["via"] = json!(via.word()),
            Err(why) => record["why"] = json!(why),
        }
        if let Some(about) = about {
            record["about"] = json!(about);
        }
        if let Some(kind) = kind {
            record["kind"] = json!(kind);
        }
        append_in(dir, &record, ROTATE_BYTES)?;
        if let Some(question) = question.filter(|_| answering) {
            append_in(dir, &json!({ "id": question.id, "at": now(), "state": "answered" }), ROTATE_BYTES)?;
        }
        report.push(match outcome {
            Ok(_) => format!("Told {}.", session.address),
            Err(reason) => format!("{} {reason}; held, and delivered when it is free.", session.address),
        });
    }
    Ok(report)
}

pub fn tell(cwd: &Path, to: &str, text: &str) -> Result<String> {
    if text.trim().is_empty() {
        return Err("kebbie tell needs a message: kebbie tell <task|task@repo|lead> \"<text>\"".to_string());
    }
    let from = peers::me(cwd).ok_or("kebbie tell runs in a task's worktree or in the lead Space")?;
    let sessions = peers::sessions()?;
    let recipients = peers::resolve(to, &from, &sessions)?;
    Ok(send(&from, &recipients, text.trim(), &sessions)?.join("\n"))
}
