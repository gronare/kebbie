use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::inbox::Via;
use crate::peers::{self, Session};
use crate::questions;
use crate::sh::Result;
use crate::status::LEAD_LABEL;

mod digest;
mod lock;
mod log;
mod pane;

pub use digest::*;
pub use lock::*;
pub use log::*;
pub use pane::*;

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
fn part(message: &Message) -> String {
    if message.kind.as_deref() == Some(PROMPT_KIND) { message.text.clone() } else { delivered(&message.from, &message.text) }
}

pub const LEAD_CUE: &str = "(kebbie: work silently; end in Done / Going on / Needs you, or one line if nothing needs the user)";

// A message that starts a lead turn carries the cue for how that turn ends; the user's own word does not.
fn typed_text(from: &str, to: &str, text: &str) -> String {
    let typed = delivered(from, text);
    if !cued(from, to) {
        return typed;
    }
    format!("{typed}\n  {LEAD_CUE}")
}

fn cued(from: &str, to: &str) -> bool {
    to == LEAD_LABEL && from != USER_LABEL && from != LEAD_LABEL
}

pub const LEAD_BUSY: &str = "is mid-turn; it goes in one batch once the lead is idle and the user has been quiet for 3 minutes";
pub const LEAD_TALKING: &str = "is in a conversation with the user; it goes in one batch once the user has been quiet for 3 minutes";
pub const QUIET_FOR: u64 = 180;
const LEAD_PROMPTED: &str = "lead-prompted";
const LEAD_TALKED: &str = "lead-talked";

// kebbie's own text opens with its sender line, or comes in the inbox's envelope.
fn kebbies_prompt(prompt: &str) -> bool {
    let prompt = prompt.trim_start();
    prompt.starts_with("[from ") || prompt.starts_with("<cross-session-message")
}

// Who wrote the lead's latest prompt and when, from its prompt hook and from kebbie's own deliveries.
pub fn note_lead_prompt_in(dir: &Path, prompt: &str, at: u64) -> Result<()> {
    crate::state::write(&dir.join(LEAD_PROMPTED), format!("{at} {}", if kebbies_prompt(prompt) { "kebbie" } else { USER_LABEL }))
}

// The lead's prompt hook gets the prompt on stdin; a run from a terminal, or a hook that sends nothing, notes nothing.
pub fn note_lead_prompt_from_stdin() {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return;
    }
    let (sent, got) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        let _ = sent.send(input);
    });
    let Ok(input) = got.recv_timeout(Duration::from_millis(300)) else { return };
    let Some(prompt) = serde_json::from_str::<Value>(&input).ok().and_then(|input| input["prompt"].as_str().map(str::to_string)) else { return };
    if let Ok(dir) = state_dir() {
        let _ = note_lead_prompt_in(&dir, &prompt, now());
    }
}

fn stamp(dir: &Path, name: &str) -> Option<String> {
    fs::read_to_string(dir.join(name)).ok()
}

// After the user's prompt, the lead's transcript is written by its turn on it, so its last write is when that turn ended.
fn users_last_word(dir: &Path, session: &Session) -> Option<u64> {
    let prompted = stamp(dir, LEAD_PROMPTED).and_then(|stamp| stamp.strip_suffix(&format!(" {USER_LABEL}")).and_then(|at| at.trim().parse::<u64>().ok()));
    let talked = stamp(dir, LEAD_TALKED).and_then(|at| at.trim().parse::<u64>().ok());
    prompted.map(|prompted| prompted.max(lead_written(session).unwrap_or(0))).max(talked)
}

#[cfg(not(test))]
fn lead_written(session: &Session) -> Option<u64> {
    let file = crate::typing::transcript_of(session.pane.as_deref()?)?;
    fs::metadata(file).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|written| written.as_secs())
}

#[cfg(test)]
fn lead_written(_: &Session) -> Option<u64> {
    None
}

#[cfg(not(test))]
fn lead_draft(session: &Session) -> bool {
    session.pane.as_deref().is_some_and(|pane| matches!(crate::typing::draft_in(pane, crate::agent::Agent::Claude), crate::typing::Draft::Typed(_)))
}

#[cfg(test)]
thread_local! {
    static LEAD_DRAFT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn lead_draft(_: &Session) -> bool {
    LEAD_DRAFT.with(std::cell::Cell::get)
}

// While the user is in a conversation with the lead, what asks the lead waits: the lead mid-turn, the user's text in its box, or the user's prompt or the lead's turn on it within 3 minutes, which a prompt blocking a task does not wait out. Away mode delivers at once.
fn lead_hold_in(dir: &Path, session: &Session, from: &str, kind: Option<&str>, at: u64) -> Option<&'static str> {
    if session.address != LEAD_LABEL || from == USER_LABEL || from == LEAD_LABEL || crate::permit::away_since_in(dir).is_some() {
        return None;
    }
    if session.status.as_deref() == Some("working") {
        if stamp(dir, LEAD_PROMPTED).is_some_and(|stamp| stamp.ends_with(&format!(" {USER_LABEL}"))) {
            let _ = crate::state::write(&dir.join(LEAD_TALKED), at.to_string());
        }
        return Some(LEAD_BUSY);
    }
    if lead_draft(session) {
        return Some(DRAFT_HOLD);
    }
    if kind == Some(crate::menu::KIND) {
        return None;
    }
    users_last_word(dir, session).filter(|last| at.saturating_sub(*last) < QUIET_FOR).map(|_| LEAD_TALKING)
}

// What waited for the lead goes as one message, grouped by task, a prompt that blocks a task first, with one cue at the end.
fn batch_text(dir: &Path, messages: &[&Message]) -> String {
    let urgent = messages.first().is_some_and(|message| message.kind.as_deref() == Some(crate::menu::KIND));
    let lines = by_task(dir, messages, |message| format!("- {}", part(message)));
    let cue = if messages.iter().any(|message| cued(&message.from, &message.to)) { format!("\n{LEAD_CUE}") } else { String::new() };
    let first = if urgent { "; the first blocks a task" } else { "" };
    delivered("kebbie", &format!("{} messages for you, by task{first}:\n{}{cue}", messages.len(), lines.join("\n")))
}

pub const DRAFT_HOLD: &str = "has a draft in its input box";
pub const EARLIER_TYPING: &str = "is still taking the messages kebbie typed for it just now";
const TYPED_FOR: u64 = 10;

fn typed_file(dir: &Path, address: &str) -> PathBuf {
    dir.join("typed").join(address)
}

fn note_typed(dir: &Path, address: &str) {
    let _ = crate::state::write(&typed_file(dir, address), now().to_string());
}

// The box seen as a draft right after kebbie typed into it, in this pass or a recent one, holds kebbie's own text.
fn kebbies_own<'a>(dir: &Path, address: &str, why: &'a str) -> &'a str {
    let recent = fs::read_to_string(typed_file(dir, address)).ok().and_then(|at| at.trim().parse::<u64>().ok()).is_some_and(|at| now().saturating_sub(at) <= TYPED_FOR);
    if why == DRAFT_HOLD && recent { EARLIER_TYPING } else { why }
}

type TypeInto<'a> = &'a mut dyn FnMut(&str, &str, &str) -> std::result::Result<Via, crate::typing::Refused>;

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
fn stale(message: &Message, open: &[&Message]) -> bool {
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

fn typed_by_user_since(agent: crate::agent::Agent, transcript: &str, since: u64) -> bool {
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

fn flush_held_in(dir: &Path, sessions: &[Session], type_into: TypeInto, heard: &dyn Fn(&str, u64) -> bool, wanted: Wanted) -> Result<usize> {
    flush_typing_in(dir, sessions, type_into, heard, wanted, &inbox_live(dir)).map(|(delivered, _)| delivered)
}

// What it delivered, and the sessions it typed into: their boxes hold kebbie's own text for a moment.
fn flush_typing_in(dir: &Path, sessions: &[Session], type_into: TypeInto, heard: &dyn Fn(&str, u64) -> bool, wanted: Wanted, live: Live) -> Result<(usize, HashSet<String>)> {
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

// A rules check Claude did not take is held again, so the next flush retypes it once the lead is free; other messages are only reported.
pub fn hold_again(to: &str, typed: &str) -> Result<bool> {
    let dir = state_dir()?;
    let _lock = Lock::take_in(&dir, true)?;
    hold_again_in(&dir, to, typed)
}

// A batch the lead did not take holds again each rules check it carried; one message sent alone holds only itself.
fn hold_again_in(dir: &Path, to: &str, typed: &str) -> Result<bool> {
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

fn hold_prompt_in(dir: &Path, to: &str, text: &str) -> Result<()> {
    let _lock = if holds(dir) { None } else { Lock::take_in(dir, true)? };
    append_in(dir, &json!({ "id": new_id("prompt"), "at": now(), "from": "kebbie", "to": to, "text": text, "state": "held", "kind": PROMPT_KIND }), ROTATE_BYTES)
}

pub fn drop_held_prompts(to: &str) -> Result<()> {
    drop_held_prompts_in(&state_dir()?, to)
}

fn drop_held_prompts_in(dir: &Path, to: &str) -> Result<()> {
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

fn drop_held_for_ended_in(dir: &Path, address: &str) -> Result<()> {
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

fn send(from: &str, recipients: &[&Session], text: &str, sessions: &[Session]) -> Result<Vec<String>> {
    let _lock = Lock::take(true)?;
    send_locked(from, recipients, text, sessions, None, None)
}

// The caller holds the lock. A message about the recipient's open question is the answer to it: it goes ahead of what is held for the recipient, and closes the question.
pub fn send_locked(from: &str, recipients: &[&Session], text: &str, sessions: &[Session], about: Option<&str>, kind: Option<&str>) -> Result<Vec<String>> {
    send_locked_in(&state_dir()?, from, recipients, text, sessions, about, kind, &mut crate::inbox::prompt)
}

#[allow(clippy::too_many_arguments)]
fn send_locked_in(dir: &Path, from: &str, recipients: &[&Session], text: &str, sessions: &[Session], about: Option<&str>, kind: Option<&str>, type_into: TypeInto) -> Result<Vec<String>> {
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

pub const FORCED_KIND: &str = "forced";

// The user's or the lead's override for a delivery that is stuck: it clears the user's draft (logged word for word), then types what is held for the session and the new message, in order, as one prompt.
pub fn tell_forced(to: &str, text: Option<&str>) -> Result<String> {
    if text.is_some_and(|text| text.trim().is_empty()) {
        return Err("kebbie tell --force needs a message: kebbie tell --force <task|task@repo|lead> \"<text>\"; kebbie release <task> delivers only what is held".to_string());
    }
    let from = forcer(&crate::permit::caller())?;
    let sessions = peers::sessions()?;
    let recipients = peers::resolve(to, from, &sessions)?;
    let _lock = Lock::take(true)?;
    let dir = state_dir()?;
    let mut report = Vec::new();
    for session in recipients {
        report.push(force_in(&dir, from, session, text.map(str::trim), &mut crate::typing::prompt_forced)?);
    }
    Ok(report.join("\n"))
}

fn forcer(caller: &crate::permit::Caller) -> Result<&'static str> {
    match caller {
        crate::permit::Caller::User => Ok(USER_LABEL),
        crate::permit::Caller::Lead => Ok(LEAD_LABEL),
        other => Err(format!("forcing a delivery is the user's or the lead's; {} cannot, since it clears what the user typed", crate::permit::describe(other))),
    }
}

type TypeForced<'a> = &'a mut dyn FnMut(&str, &str, &str) -> std::result::Result<Option<String>, crate::typing::Refused>;

fn force_in(dir: &Path, from: &str, session: &Session, text: Option<&str>, type_forced: TypeForced) -> Result<String> {
    let address = &session.address;
    let Some(pane) = session.pane.as_deref() else { return Err(format!("{address} has no Claude running; nothing was forced")) };
    if session.status.as_deref() == Some("blocked") {
        return Err(format!("{address} shows a prompt of Claude's own (a question or a permission), where a key would pick an option; nothing was forced. Answer it in its Space first."));
    }
    let messages = load_in(dir);
    let mut held: Vec<&Message> = messages.iter().filter(|message| message.held && &message.to == address).collect();
    held.sort_by_key(|message| message.kind.as_deref() != Some(PROMPT_KIND));
    let mut parts: Vec<String> = held.iter().map(|message| typed_form(message)).collect();
    parts.extend(text.map(|text| delivered(from, text)));
    if parts.is_empty() {
        return Ok(format!("{address} has nothing held; nothing to release."));
    }
    let cleared = match type_forced(pane, &parts.join("\n\n"), address) {
        Ok(cleared) => cleared,
        Err(refused) => return Err(format!("{address} {}; nothing was forced", refused.reason())),
    };
    if let Some(draft) = &cleared {
        append_in(dir, &json!({ "id": new_id("forced"), "at": now(), "from": "kebbie", "to": address, "text": format!("Cleared {address}'s input box so {from}'s forced delivery could go in; the draft was:\n{draft}"), "state": "logged", "kind": FORCED_KIND, "forced": true }), ROTATE_BYTES)?;
    }
    for message in &held {
        append_in(dir, &json!({ "id": message.id, "at": now(), "state": "delivered", "forced": true }), ROTATE_BYTES)?;
    }
    if let Some(text) = text {
        append_in(dir, &json!({ "id": new_id("forced"), "at": now(), "from": from, "to": address, "text": text, "state": "delivered", "forced": true }), ROTATE_BYTES)?;
    }
    let count = if held.len() == 1 { "1 message".to_string() } else { format!("{} messages", held.len()) };
    let what = match (held.len(), text.is_some()) {
        (0, _) => "your message".to_string(),
        (_, false) => format!("{count} held"),
        (_, true) => format!("{count} held, then yours"),
    };
    let clearing = cleared.map(|draft| format!(" It cleared this draft from the input box (Ctrl+Y in its pane brings it back):\n{}", indented(&draft))).unwrap_or_default();
    Ok(format!("Forced to {address}: {what}.{clearing}"))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Informs,
    Asks,
}

// Every kind of word that reaches the lead. Informs never start a lead turn: they wait in its digest for the user's next prompt (live only while the user is away). Asks are typed into the lead's chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToLead {
    TaskStarted,
    TaskRestarted,
    TaskEnded,
    Pushed,
    PushCancelled,
    PushRebasing,
    Staged,
    Landed,
    CiGreen,
    CiHanded,
    CiDismissed,
    RebaseHanded,
    RebaseStuck,
    ReviewPassed,
    ReviewAgain,
    ReviewWithdrawn,
    RulesBreak,
    Notice,
    Findings,
    RulesCheck,
    Prompt,
    PushFailed,
    PushToLog,
    RedCi,
    CiOutside,
    ChainBlocked,
    NoteUnsettled,
    TaskStopped,
    Permission,
    Away,
    // A task's own `kebbie tell lead`, typed as any tell is, and what kebbie tells the lead to write for a Codex task.
    TaskMessage,
    QuestionCopy,
}

impl ToLead {
    #[cfg_attr(not(test), allow(dead_code))]
    pub const ALL: [ToLead; 32] = [
        ToLead::TaskStarted, ToLead::TaskRestarted, ToLead::TaskEnded, ToLead::Pushed, ToLead::PushCancelled, ToLead::PushRebasing, ToLead::Staged, ToLead::Landed,
        ToLead::CiGreen, ToLead::CiHanded, ToLead::CiDismissed, ToLead::RebaseHanded, ToLead::RebaseStuck, ToLead::ReviewPassed, ToLead::ReviewAgain, ToLead::ReviewWithdrawn,
        ToLead::RulesBreak, ToLead::Notice, ToLead::Findings, ToLead::RulesCheck, ToLead::Prompt, ToLead::PushFailed, ToLead::PushToLog, ToLead::RedCi,
        ToLead::CiOutside, ToLead::ChainBlocked, ToLead::NoteUnsettled, ToLead::TaskStopped, ToLead::Permission, ToLead::Away, ToLead::TaskMessage, ToLead::QuestionCopy,
    ];

    // The one table: whether the lead has something to do, and the kind its log record carries (the away summary reads "push" and "after").
    pub fn route(self) -> (Class, Option<&'static str>) {
        use Class::{Asks, Informs};
        match self {
            ToLead::TaskStarted => (Informs, Some("after")),
            ToLead::TaskRestarted => (Informs, None),
            ToLead::TaskEnded => (Informs, None),
            ToLead::Pushed => (Informs, None),
            ToLead::PushCancelled => (Informs, Some("push")),
            ToLead::PushRebasing => (Informs, Some("push")),
            ToLead::Staged => (Informs, Some("push")),
            ToLead::Landed => (Informs, Some("landed")),
            ToLead::CiGreen => (Informs, Some("ci")),
            ToLead::CiHanded => (Informs, Some("ci")),
            ToLead::CiDismissed => (Informs, Some("ci")),
            ToLead::RebaseHanded => (Informs, Some("follow")),
            ToLead::RebaseStuck => (Informs, Some("follow")),
            ToLead::ReviewPassed => (Informs, None),
            ToLead::ReviewAgain => (Informs, None),
            ToLead::ReviewWithdrawn => (Informs, None),
            ToLead::RulesBreak => (Informs, None),
            ToLead::Notice => (Informs, Some(NOTICE_KIND)),
            ToLead::Findings => (Asks, None),
            ToLead::RulesCheck => (Asks, Some(RULES_KIND)),
            ToLead::Prompt => (Asks, Some(crate::menu::KIND)),
            ToLead::PushFailed => (Asks, Some("push")),
            ToLead::PushToLog => (Asks, Some("push")),
            ToLead::RedCi => (Asks, Some("ci")),
            ToLead::CiOutside => (Asks, Some("ci")),
            ToLead::ChainBlocked => (Asks, Some("after")),
            ToLead::NoteUnsettled => (Asks, None),
            ToLead::TaskStopped => (Asks, Some("recover")),
            ToLead::Permission => (Asks, None),
            ToLead::Away => (Asks, None),
            ToLead::TaskMessage => (Asks, None),
            ToLead::QuestionCopy => (Asks, Some("copy")),
        }
    }

    pub fn class(self) -> Class {
        self.route().0
    }

    pub fn kind(self) -> Option<&'static str> {
        self.route().1
    }
}

// kebbie's word to the lead, routed by its class: an inform to the digest, an ask into the lead's chat (held while it cannot take it, and kept until a lead runs).
pub fn to_lead(what: ToLead, text: &str) -> Result<()> {
    let dir = state_dir()?;
    if what == ToLead::RulesCheck {
        return ask_lead(text).map(|_| ());
    }
    to_lead_in(&dir, what, &crate::status::with_titles_in(&dir, text), &mut tell_lead)
}

fn to_lead_in(dir: &Path, what: ToLead, text: &str, ask: &mut dyn FnMut(&str, Option<&str>) -> Result<()>) -> Result<()> {
    match what.class() {
        Class::Informs => fyi_or_live_in(dir, text, what.kind(), ask),
        Class::Asks => ask(text, what.kind()),
    }
}

// kebbie's own word to a task, typed in as any message is.
pub fn tell_from_kebbie(recipients: &[&Session], text: &str, sessions: &[Session]) -> Result<()> {
    send("kebbie", recipients, text, sessions).map(|_| ())
}

// kebbie's word to the lead that must not be lost: with no lead session yet, it waits in the log and reaches the lead once one runs.
fn tell_lead(text: &str, kind: Option<&str>) -> Result<()> {
    let text = &crate::status::with_titles(text);
    let sessions = peers::sessions().unwrap_or_default();
    let _lock = Lock::take(true)?;
    let Some(lead) = sessions.iter().find(|session| session.address == LEAD_LABEL) else {
        let id = new_id("lead");
        return append(&json!({ "id": id, "at": now(), "from": "kebbie", "to": LEAD_LABEL, "text": text, "state": "held", "kind": kind }));
    };
    send_locked("kebbie", &[lead], text, &sessions, None, kind).map(|_| ())
}

// kebbie's word about itself (an update, a resume): held behind a draft like any message, but the draft stays the user's own business; the lead reads it with the user's next prompt.
pub fn notice_locked(session: &Session, text: &str, sessions: &[Session]) -> Result<Vec<String>> {
    notice_locked_in(&state_dir()?, session, text, sessions, &mut crate::inbox::prompt)
}

fn notice_locked_in(dir: &Path, session: &Session, text: &str, sessions: &[Session], type_into: TypeInto) -> Result<Vec<String>> {
    if session.address == LEAD_LABEL && crate::permit::away_since_in(dir).is_none() {
        log_for_lead_in(dir, text, Some(NOTICE_KIND))?;
        return Ok(vec![format!("Logged for {LEAD_LABEL}.")]);
    }
    send_locked_in(dir, "kebbie", &[session], text, sessions, None, Some(NOTICE_KIND), type_into)
}

// While the user is away the lead answers for them and writes their summary, so it hears kebbie's notices live.
fn fyi_or_live_in(dir: &Path, text: &str, kind: Option<&str>, live: &mut dyn FnMut(&str, Option<&str>) -> Result<()>) -> Result<()> {
    if crate::permit::away_since_in(dir).is_some() {
        return live(text, kind);
    }
    log_for_lead_in(dir, text, kind)
}

pub fn log_for_user_in(dir: &Path, text: &str, kind: &str) -> Result<()> {
    append_in(dir, &json!({ "id": new_id(kind), "at": now(), "from": "kebbie", "to": USER_LABEL, "text": text, "state": "logged", "kind": kind }), ROTATE_BYTES)
}

fn log_for_lead_in(dir: &Path, text: &str, kind: Option<&str>) -> Result<()> {
    append_in(dir, &json!({ "id": new_id("fyi"), "at": now(), "from": "kebbie", "to": LEAD_LABEL, "text": text, "state": "logged", "kind": kind, "fyi": true }), ROTATE_BYTES)
}

// kebbie's word to a task that must reach it: held while its box holds the user's draft or it waits for an answer, kept for a session that is not running yet, and typed again when its Claude did not take it.
pub fn hand_to(to: &str, text: &str) -> Result<()> {
    let sessions = peers::sessions().unwrap_or_default();
    let _lock = Lock::take(true)?;
    hand_to_in(&state_dir()?, to, text, &sessions, &mut crate::inbox::prompt)
}

fn hand_to_in(dir: &Path, to: &str, text: &str, sessions: &[Session], type_into: TypeInto) -> Result<()> {
    match sessions.iter().find(|session| session.address == to) {
        Some(session) => send_locked_in(dir, "kebbie", &[session], text, sessions, None, Some(HAND_KIND), type_into).map(|_| ()),
        None => append_in(dir, &json!({ "id": new_id("held"), "at": now(), "from": "kebbie", "to": to, "text": text, "state": "held", "kind": HAND_KIND }), ROTATE_BYTES),
    }
}

// The caller holds the lock. A message for a session that has none now, delivered once it runs.
pub fn hold_for(from: &str, to: &str, text: &str, about: Option<&str>, kind: Option<&str>) -> Result<()> {
    let id = new_id("held");
    append(&json!({ "id": id, "at": now(), "from": from, "to": to, "text": text, "state": "held", "about": about, "kind": kind }))
}

// The caller holds the lock. The lead's copy of what the user is asked (about that question, so a copy still held when it is answered is dropped) and answers on the board: typed into its chat while the user is away, so it can settle a question of method; otherwise given with the user's next prompt to it.
pub fn copy_to_lead_locked(text: &str, about: Option<&str>, sessions: &[Session]) -> Result<()> {
    let text = &crate::status::with_titles(text);
    if crate::permit::away_since().is_none() {
        return append(&json!({ "id": new_id("copy"), "at": now(), "from": "kebbie", "to": LEAD_LABEL, "text": text, "state": "logged", "kind": ToLead::QuestionCopy.kind(), "about": about }));
    }
    match sessions.iter().find(|session| session.address == LEAD_LABEL) {
        Some(lead) => send_locked("kebbie", &[lead], text, sessions, about, ToLead::QuestionCopy.kind()).map(|_| ()),
        None => hold_for("kebbie", LEAD_LABEL, text, about, ToLead::QuestionCopy.kind()),
    }
}

// kebbie's own word to a task, held like any message while it waits for an answer.
pub fn notify(to: &str, text: &str) -> Result<()> {
    let sessions = peers::sessions()?;
    let Ok(recipients) = peers::resolve(to, "kebbie", &sessions) else { return Ok(()) };
    send("kebbie", &recipients, text, &sessions).map(|_| ())
}

// When a task starts in another repo, the halves already running learn of it.
pub fn announce(task: &str, repo: &str) -> Result<()> {
    let sessions = peers::sessions()?;
    let me = peers::address(task, repo);
    let Ok(halves) = peers::resolve(task, &me, &sessions) else { return Ok(()) };
    send("kebbie", &halves, &format!("{me} has started: the same task, in {repo}. Keep it informed of changes it depends on."), &sessions).map(|_| ())
}

// From kebbie itself, held like any message while the lead waits for an answer; the lead is asked once about a task at a sha.
pub fn ask_lead(text: &str) -> Result<String> {
    let sessions = peers::sessions()?;
    let _lock = Lock::take(true)?;
    ask_lead_in(&state_dir()?, text, &sessions, &mut crate::inbox::prompt, &|message, lead| lead.pane.as_deref().is_some_and(|pane| crate::typing::taken_by(pane, &part(message), message.at * 1000)))
}

// A check the lead lost (dropped at its inbox, or typed and never taken) is no longer pending, so it is asked again.
fn ask_lead_in(dir: &Path, text: &str, sessions: &[Session], type_into: TypeInto, taken: &dyn Fn(&Message, &Session) -> bool) -> Result<String> {
    let lead = sessions.iter().find(|session| session.address == LEAD_LABEL).ok_or("no lead session to check the rules")?;
    let subject = crate::tasks::rules_check_subject(text);
    let pending = |message: &Message| message.held || crate::typing::watching_part_in(dir, LEAD_LABEL, &part(message)) || taken(message, lead);
    let asked = load_in(dir).into_iter().any(|message| message.to == LEAD_LABEL && message.kind.as_deref() == Some(RULES_KIND) && subject.is_some() && crate::tasks::rules_check_subject(&message.text) == subject && pending(&message));
    if asked {
        return Ok(format!("The lead already has this rules check ({}).", subject.unwrap_or_default()));
    }
    Ok(send_locked_in(dir, "kebbie", &[lead], text, sessions, None, Some(RULES_KIND), type_into)?.join("\n"))
}

#[cfg(test)]
mod tests;
