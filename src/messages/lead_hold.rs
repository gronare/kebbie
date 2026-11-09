use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::lead::state_dir;
use crate::peers::Session;
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use super::{DRAFT_HOLD, LEAD_CUE, Message, USER_LABEL, by_task, cued, delivered, now, part};

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
    pub(super) static LEAD_DRAFT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn lead_draft(_: &Session) -> bool {
    LEAD_DRAFT.with(std::cell::Cell::get)
}

// While the user is in a conversation with the lead, what asks the lead waits: the lead mid-turn, the user's text in its box, or the user's prompt or the lead's turn on it within 3 minutes, which a prompt blocking a task does not wait out. Away mode delivers at once.
pub(super) fn lead_hold_in(dir: &Path, session: &Session, from: &str, kind: Option<&str>, at: u64) -> Option<&'static str> {
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
pub(super) fn batch_text(dir: &Path, messages: &[&Message]) -> String {
    let urgent = messages.first().is_some_and(|message| message.kind.as_deref() == Some(crate::menu::KIND));
    let lines = by_task(dir, messages, |message| format!("- {}", part(message)));
    let cue = if messages.iter().any(|message| cued(&message.from, &message.to)) { format!("\n{LEAD_CUE}") } else { String::new() };
    let first = if urgent { "; the first blocks a task" } else { "" };
    delivered("kebbie", &format!("{} messages for you, by task{first}:\n{}{cue}", messages.len(), lines.join("\n")))
}
