use std::fs;
use std::path::Path;

use crate::lead::state_dir;
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use super::{LEAD_MARK, Lock, Message, PROMPT_KIND, USER_LABEL, held_note, holds, indented, latest_reports, load_in, now};

// The lead's copy: what sessions told each other since it last looked, and their latest reports, given to it with each of the user's prompts.
pub fn unread_for_lead() -> Result<String> {
    let unread = unread_in(&state_dir()?)?;
    Ok(crate::known::stale_for(LEAD_LABEL).into_iter().chain((!unread.is_empty()).then_some(unread)).collect::<Vec<_>>().join("\n\n"))
}

// How many of the messages the lead has seen, by the id of the last one; a mark that names no message in the log counts all as seen, so the lead is never flooded with history.
pub(super) fn seen_count(messages: &[Message], mark: Option<&str>) -> usize {
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
pub(super) fn by_task(dir: &Path, messages: &[&Message], line: impl Fn(&Message) -> String) -> Vec<String> {
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
pub(crate) fn is_told(message: &Message) -> bool {
    message.report.is_none() && !message.ended && message.question.is_none() && !is_copy(message) && !message.fyi && !matches!(message.kind.as_deref(), Some("answer" | PROMPT_KIND))
}

pub(super) fn is_copy(message: &Message) -> bool {
    message.kind.as_deref() == Some("copy")
}

pub fn report_line(message: &Message) -> String {
    let state = message.report.as_ref().map(|report| report.state.as_str()).unwrap_or_default();
    format!("- {} ({state}): {} — you: {}", crate::status::with_titles(&message.from), message.text, message.next_for_you().unwrap_or("nothing"))
}

// Messages to or from the lead are already in its chat; reports, the board's copies and kebbie's notices to it are not; what kebbie sent its maintainers, the bug reports it filed from them and the comments it read are the user's alone.
pub(super) fn copies_for_lead(messages: &[Message], seen: usize) -> Vec<&Message> {
    messages.iter().skip(seen.min(messages.len()))
        .filter(|message| message.report.is_some() || is_copy(message) || message.fyi || (message.from != LEAD_LABEL && message.to != LEAD_LABEL && !users_alone(message))).collect()
}

fn users_alone(message: &Message) -> bool {
    message.to == USER_LABEL && message.kind.as_deref().is_some_and(|kind| [crate::hub::report::SENT_KIND, crate::signals::KIND, crate::hub::inbox::FILED_KIND, crate::comments::KIND].contains(&kind))
}
