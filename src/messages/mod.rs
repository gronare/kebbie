use std::cell::RefCell;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use serde_json::{Value, json};

use crate::board::widgets::{FAINT, MAUVE, MUTED, PEACH, SUBTEXT, SURFACE, TEAL, TEXT, YELLOW, bold, style};
use crate::lead::state_dir;
use crate::inbox::Via;
use crate::peers::{self, Session};
use crate::questions;
use crate::sh::Result;
use crate::status::LEAD_LABEL;

mod log;

pub use log::*;

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

// One sender at a time, so a held message is never delivered twice; the kernel drops the flock when its holder dies, so a crash leaves nothing to steal.
pub struct Lock(File, PathBuf);

thread_local! {
    static HELD: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

fn lock_in(dir: &Path) -> PathBuf {
    dir.join("messages.lock")
}

// An append under a lock this thread already holds must not wait for itself.
fn holds(dir: &Path) -> bool {
    HELD.with(|held| held.borrow().contains(&lock_in(dir)))
}

impl Lock {
    pub fn take(wait: bool) -> Result<Option<Lock>> {
        Lock::take_in(&state_dir()?, wait)
    }

    pub fn take_in(dir: &Path, wait: bool) -> Result<Option<Lock>> {
        let path = lock_in(dir);
        let file = OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        for _ in 0..100 {
            // SAFETY: flock on a descriptor this File owns.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                HELD.with(|held| held.borrow_mut().push(path.clone()));
                return Ok(Some(Lock(file, path)));
            }
            if !wait {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err("Another kebbie is holding the message log; try again".to_string())
    }
}

impl Drop for Lock {
    // A child forked by another thread holds a copy of the descriptor until it execs; unlocking releases the lock for every copy at once.
    fn drop(&mut self) {
        HELD.with(|held| held.borrow_mut().retain(|path| path != &self.1));
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
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

// What sessions told each other, the questions and the user's answers, and the task reports that wait on the user.
fn shown_in_log(messages: Vec<Message>) -> Vec<Message> {
    messages.into_iter().filter(|message| !is_copy(message) && message.report.as_ref().is_none_or(|report| report.state != "working")).collect()
}

pub fn recent(count: usize) -> Vec<Message> {
    let messages = shown_in_log(load());
    messages[messages.len().saturating_sub(count)..].to_vec()
}

pub fn addressee(message: &Message) -> String {
    if message.question.is_some() {
        return "you · asks".to_string();
    }
    match &message.report {
        Some(report) => format!("you · {}", report.state.replace('-', " ")),
        None => message.to.clone(),
    }
}

pub fn age(at: u64) -> String {
    let seconds = now().saturating_sub(at);
    match seconds {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

pub fn held_note(message: &Message) -> Option<String> {
    if message.forced {
        return Some("forced".to_string());
    }
    message.held.then(|| message.why.as_ref().map_or("held".to_string(), |why| format!("held: {why}")))
}

pub fn print(count: usize) -> String {
    print_lines(&recent(count))
}

fn print_lines(messages: &[Message]) -> String {
    if messages.is_empty() {
        return "No messages yet.".to_string();
    }
    messages.iter().map(|message| {
        let held = held_note(message).map(|note| format!("  ({note})")).unwrap_or_default();
        let next = message.next_for_you().map(|next| format!("\n      you: {next}")).unwrap_or_default();
        format!("{:>4}  {} → {}{held}\n      {}{next}", age(message.at), message.from, addressee(message), message.text)
    }).collect::<Vec<_>>().join("\n")
}

// The lead's copy: what sessions told each other since it last looked, and their latest reports, given to it with each of the user's prompts.
pub fn unread_for_lead() -> Result<String> {
    let unread = unread_in(&state_dir()?)?;
    Ok(crate::known::stale_for(LEAD_LABEL).into_iter().chain((!unread.is_empty()).then_some(unread)).collect::<Vec<_>>().join("\n\n"))
}

// How many of the messages the lead has seen, by the id of the last one; a mark that names no message in the log counts all as seen, so the lead is never flooded with history.
fn seen_count(messages: &[Message], mark: Option<&str>) -> usize {
    match mark {
        None | Some("") => 0,
        Some(id) => messages.iter().rposition(|message| message.id == id).map_or(messages.len(), |seen| seen + 1),
    }
}

pub(crate) fn unread_in(dir: &Path) -> Result<String> {
    let _lock = if holds(dir) { None } else { Lock::take_in(dir, true)? };
    let mark = dir.join(LEAD_MARK);
    let messages = load_in(dir);
    let seen = seen_count(&messages, fs::read_to_string(&mark).ok().as_deref().map(str::trim));
    crate::state::write(&mark, messages.last().map(|message| message.id.as_str()).unwrap_or_default())?;
    let parked = crate::tasks::parked_all(dir, now());
    let copies: Vec<&Message> = copies_for_lead(&messages, seen).into_iter().filter(|message| !task_about(message).is_some_and(|task| parked.iter().any(|(address, _)| address == task))).collect();
    let told: Vec<&Message> = copies.iter().copied().filter(|message| is_told(message)).collect();
    let board: Vec<String> = copies.iter().filter(|message| is_copy(message)).map(|message| format!("- {}", indented(&message.text))).collect();
    let notices: Vec<&Message> = copies.iter().copied().filter(|message| message.fyi).collect();
    let mut sections = Vec::new();
    if !notices.is_empty() {
        sections.push(format!("kebbie's notices since you last looked, by task (pushes, closes, starts, reviews, updates; nothing to answer):\n{}", by_task(dir, &notices, |message| format!("- {}", indented(&message.text))).join("\n")));
    }
    if !board.is_empty() {
        sections.push(format!("Questions on the board since you last looked (the user answers them there; these are your copies):\n{}", board.join("\n")));
    }
    if !told.is_empty() {
        let lines: Vec<String> = told.iter().map(|message| {
            let held = held_note(message).map(|note| format!(" ({note})")).unwrap_or_default();
            format!("- {} → {}{held}: {}", crate::status::with_titles_in(dir, &message.from), crate::status::with_titles_in(dir, &message.to), indented(&message.text))
        }).collect();
        sections.push(format!("Messages between sessions since you last looked (kebbie messages --print for more):\n{}", lines.join("\n")));
    }
    let latest = latest_reports(&copies.into_iter().cloned().collect::<Vec<_>>());
    if !latest.is_empty() {
        let mut lines: Vec<String> = latest.values().map(report_line).collect();
        lines.sort();
        sections.push(format!("Task reports since you last looked (each task's latest; kebbie status --json has them all):\n{}", lines.join("\n")));
    }
    if !parked.is_empty() {
        let lines: Vec<String> = parked.iter().map(|(address, parked)| format!("- {} ({})", crate::status::with_titles_in(dir, address), parked.back_now())).collect();
        sections.push(format!("Parked by the user (leave them out of Done, Going on and Needs you until they are back; what they said meanwhile waits on the board):\n{}", lines.join("\n")));
    }
    Ok(sections.join("\n\n"))
}

// The first task address a text names, as kebbie writes them: name@repo.
pub fn address_in(text: &str) -> Option<&str> {
    let name = |character: char| character.is_ascii_alphanumeric() || matches!(character, '-' | '_');
    text.match_indices('@').find_map(|(at, _)| {
        let start = text[..at].char_indices().rev().find(|(_, character)| !name(*character)).map_or(0, |(index, character)| index + character.len_utf8());
        let end = text[at + 1..].find(|character: char| !(name(character) || character == '.')).map_or(text.len(), |index| at + 1 + index);
        let address = text[start..end].trim_end_matches('.');
        (start < at && address.len() > at + 1 - start).then_some(address)
    })
}

// The task a message to the lead is about: the task that sent it, or the first one its text names.
pub fn task_about(message: &Message) -> Option<&str> {
    if message.from.contains('@') { Some(message.from.as_str()) } else { address_in(&message.text) }
}

// Lines grouped under each task's "title (name)", in the order the tasks first come up, with what names no task last.
fn by_task(dir: &Path, messages: &[&Message], line: impl Fn(&Message) -> String) -> Vec<String> {
    let mut groups: Vec<(Option<&str>, Vec<String>)> = Vec::new();
    for message in messages {
        let task = task_about(message);
        match groups.iter_mut().find(|(named, _)| *named == task) {
            Some((_, lines)) => lines.push(line(message)),
            None => groups.push((task, vec![line(message)])),
        }
    }
    groups.sort_by_key(|(task, _)| task.is_none());
    let several = groups.len() > 1 || groups.iter().any(|(task, _)| task.is_some());
    groups.into_iter().flat_map(|(task, lines)| {
        let head = several.then(|| task.map_or("Other".to_string(), |task| crate::status::with_titles_in(dir, task)));
        head.into_iter().chain(lines)
    }).collect()
}

// What sessions said to each other: not a report, a task's end marker, a question, an answer from the board, the lead's copy of one, or a task's own prompt waiting to be typed.
fn is_told(message: &Message) -> bool {
    message.report.is_none() && !message.ended && message.question.is_none() && !is_copy(message) && !message.fyi && !matches!(message.kind.as_deref(), Some("answer" | PROMPT_KIND))
}

fn is_copy(message: &Message) -> bool {
    message.kind.as_deref() == Some("copy")
}

pub fn report_line(message: &Message) -> String {
    let state = message.report.as_ref().map(|report| report.state.as_str()).unwrap_or_default();
    format!("- {} ({state}): {} — you: {}", crate::status::with_titles(&message.from), message.text, message.next_for_you().unwrap_or("nothing"))
}

// Messages to or from the lead are already in its chat; reports, the board's copies and kebbie's notices to it are not; what kebbie sent its maintainers, the bug reports it filed from them and the comments it read are the user's alone.
fn copies_for_lead(messages: &[Message], seen: usize) -> Vec<&Message> {
    messages.iter().skip(seen.min(messages.len()))
        .filter(|message| message.report.is_some() || is_copy(message) || message.fyi || (message.from != LEAD_LABEL && message.to != LEAD_LABEL && !users_alone(message))).collect()
}

fn users_alone(message: &Message) -> bool {
    message.to == USER_LABEL && message.kind.as_deref().is_some_and(|kind| [crate::hub::report::SENT_KIND, crate::signals::KIND, crate::hub::inbox::FILED_KIND, crate::comments::KIND].contains(&kind))
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

fn message_lines(message: &Message, width: usize) -> Vec<Line<'static>> {
    let mut head = vec![
        Span::styled(format!("{:>4} ", age(message.at)), style(FAINT)),
        Span::styled(message.from.clone(), style(TEAL)),
        Span::styled(" → ", style(MUTED)),
        Span::styled(addressee(message), style(if message.report.is_some() { PEACH } else { TEAL })),
    ];
    if let Some(note) = held_note(message) {
        head.push(Span::styled(format!("  {note}"), style(PEACH)));
    }
    let mut lines = vec![Line::from(head)];
    lines.extend(wrap(&message.text, width.saturating_sub(5).max(10)).into_iter().map(|text| Line::from(vec![Span::raw("     "), Span::styled(text, style(SUBTEXT))])));
    if let Some(next) = message.next_for_you() {
        lines.extend(wrap(&format!("you: {next}"), width.saturating_sub(5).max(10)).into_iter().map(|text| Line::from(vec![Span::raw("     "), Span::styled(text, style(YELLOW))])));
    }
    lines.push(Line::default());
    lines
}

fn draw(frame: &mut Frame, messages: &[Message], scroll: usize) {
    let [title, rule, body] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)]).areas(frame.area());
    let width = body.width as usize;
    let held = messages.iter().filter(|message| message.held).count();
    let right = if held > 0 { format!("{held} held · {}", messages.len()) } else { messages.len().to_string() };
    let gap = width.saturating_sub(8 + right.chars().count()).max(1);
    frame.render_widget(Paragraph::new(Line::from(vec![
        Span::styled("MESSAGES", bold(MAUVE)), Span::raw(" ".repeat(gap)), Span::styled(right, style(if held > 0 { PEACH } else { MUTED })),
    ])), title);
    frame.render_widget(Paragraph::new(Line::from(Span::styled("─".repeat(width), style(SURFACE)))), rule);
    let mut lines: Vec<Line> = messages.iter().flat_map(|message| message_lines(message, width)).collect();
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("Sessions' messages to each other show here,", style(MUTED))));
        lines.push(Line::from(Span::styled("and task reports that wait on you.", style(MUTED))));
    }
    let height = body.height as usize;
    let bottom = lines.len().saturating_sub(height);
    let top = bottom.saturating_sub(scroll);
    let shown: Vec<Line> = lines.into_iter().skip(top).take(height).collect();
    frame.render_widget(Paragraph::new(shown).style(style(TEXT)), body);
}

// The log, newest at the bottom; ↑ ↓ scroll back and forth, and the pane delivers held messages as sessions free up.
pub fn run() -> Result<String> {
    let mut terminal = ratatui::init();
    let mut scroll = 0usize;
    let mut flushed = SystemTime::UNIX_EPOCH;
    let result = (|| -> Result<()> {
        loop {
            if flushed.elapsed().unwrap_or_default() > Duration::from_secs(2) {
                let _ = flush();
                crate::typing::check();
                flushed = SystemTime::now();
            }
            let messages = shown_in_log(load());
            terminal.draw(|frame| draw(frame, &messages, scroll)).map_err(|error| error.to_string())?;
            if event::poll(Duration::from_millis(500)).map_err(|error| error.to_string())?
                && let Event::Key(key) = event::read().map_err(|error| error.to_string())?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Up | KeyCode::Char('k') => scroll += 1,
                    KeyCode::Down | KeyCode::Char('j') => scroll = scroll.saturating_sub(1),
                    KeyCode::End | KeyCode::Char('G') => scroll = 0,
                    _ => {}
                }
            }
        }
    })();
    ratatui::restore();
    result.map(|_| String::new())
}

#[cfg(test)]
mod tests;
