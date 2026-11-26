use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{NewCase, Source};
use crate::lead_thread::{Event, Input};
use crate::sh::Result;

pub const EVERY: u64 = 30;
const READ_AT_MOST: u64 = 8 * 1024 * 1024;
const LONGEST: usize = 4_000;
const REPLY_CHARS: usize = 700;
const KEPT_UNLISTED: u64 = 30 * super::DAY;
pub const CORRECTING: [&str; 3] = ["Ask for changes", "Reject with a comment", "Something is wrong"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Held {
    words: String,
    before: String,
    after: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Mark {
    inode: u64,
    offset: u64,
    #[serde(default)]
    last_reply: String,
    #[serde(default)]
    held: Option<Held>,
}

fn file(state: &Path) -> PathBuf {
    super::dir(state).join("read.json")
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Marks {
    started: u64,
    #[serde(default)]
    marks: BTreeMap<String, Mark>,
}

fn load(state: &Path) -> Option<Marks> {
    std::fs::read_to_string(file(state)).ok().and_then(|text| serde_json::from_str(&text).ok())
}

pub fn worth(text: &str) -> bool {
    let text = text.trim();
    let words = text.split_whitespace().count();
    !text.is_empty() && text.chars().count() <= LONGEST && !text.starts_with('/') && (words >= 4 || text.contains('?') || text.contains('!'))
}

fn context(held: &Held) -> String {
    let before = if held.before.is_empty() { "nothing".to_string() } else { super::clipped(&held.before, REPLY_CHARS) };
    let after = if held.after.is_empty() { "nothing yet".to_string() } else { super::clipped(&held.after, REPLY_CHARS) };
    format!("Before it, the agent had said: {before}\nThe agent answered: {after}")
}

fn changed(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|since| since.as_secs())
}

fn born(meta: &std::fs::Metadata) -> Option<u64> {
    meta.created().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|since| since.as_secs())
}

// A file seen for the first time is read whole only when it began after the watcher's first look, so the history before the watcher ran is never weighed and a new chat's first words are.
fn read_on(mark: Option<Mark>, path: &Path, started: u64) -> std::io::Result<(Mark, Vec<Held>)> {
    let mut opened = File::open(path)?;
    let meta = opened.metadata()?;
    let mut mark = match mark {
        Some(mark) if mark.inode == meta.ino() && mark.offset <= meta.len() => mark,
        Some(_) => Mark { inode: meta.ino(), ..Mark::default() },
        None if born(&meta).is_some_and(|born| born >= started) => Mark { inode: meta.ino(), ..Mark::default() },
        None => return Ok((Mark { inode: meta.ino(), offset: meta.len(), ..Mark::default() }, Vec::new())),
    };
    let start = mark.offset.max(meta.len().saturating_sub(READ_AT_MOST));
    opened.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(opened.take(meta.len() - start));
    let mut line = Vec::new();
    let mut offset = start;
    let mut found = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 || line.last() != Some(&b'\n') {
            break;
        }
        offset += read as u64;
        let Some(event) = std::str::from_utf8(&line[..read - 1]).ok().and_then(crate::lead_thread::event) else { continue };
        step(&mut mark, event, &mut found);
    }
    mark.offset = offset;
    Ok((mark, found))
}

fn step(mark: &mut Mark, event: Event, found: &mut Vec<Held>) {
    match event {
        Event::Input { input: Input::You, text, .. } => {
            found.extend(mark.held.take());
            mark.held = Some(Held { words: text, before: mark.last_reply.clone(), after: String::new() });
        }
        Event::Reply { text, .. } => {
            match mark.held.as_mut() {
                Some(held) if held.after.is_empty() => held.after = text.clone(),
                _ => {}
            }
            mark.last_reply = super::clipped(&text, REPLY_CHARS);
        }
        Event::End { .. } => found.extend(mark.held.take()),
        Event::Input { .. } | Event::Hook => {}
    }
}

pub struct Chat {
    pub from: String,
    pub path: PathBuf,
    pub kebbie_typed: Vec<String>,
}

fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// kebbie types a task's start prompt, each prompt it gives a restarted task and the lead's first prompt with no [from …] prefix, so the transcript shows them as the user's turns.
fn typed_by_kebbie(chat: &Chat, text: &str) -> bool {
    let text = flat(text);
    crate::lead::first_prompt_by_kebbie(&text) || chat.kebbie_typed.iter().map(|typed| flat(typed)).any(|typed| !typed.is_empty() && (text.starts_with(&typed) || typed.starts_with(&text)))
}

fn chats(state: &Path) -> Vec<Chat> {
    let mut chats: Vec<Chat> = crate::lead::transcript_file(state).into_iter().map(|path| Chat { from: crate::status::LEAD_LABEL.to_string(), path, kebbie_typed: Vec::new() }).collect();
    for repo in crate::repo::pooled_checkouts() {
        for slot in crate::tasks::slots(&repo).unwrap_or_default() {
            let Some(task) = slot.task.clone() else { continue };
            if let Some((crate::agent::Agent::Claude, transcript)) = crate::tasks::transcript(&repo, &slot) {
                chats.push(Chat { from: crate::peers::address(&task, &crate::repo::name_of(&repo)), path: transcript, kebbie_typed: crate::tasks::kebbie_prompts(&task, &repo) });
            }
        }
    }
    chats
}

pub fn tick(state: &Path) -> Vec<String> {
    if !super::watch::on() {
        return Vec::new();
    }
    tick_in(state, &chats(state), crate::messages::now())
}

pub fn tick_in(state: &Path, chats: &[Chat], now: u64) -> Vec<String> {
    let Marks { started, marks } = load(state).unwrap_or(Marks { started: now, marks: BTreeMap::new() });
    let mut added = 0;
    let listed = |key: &str| chats.iter().any(|chat| chat.path.display().to_string() == key);
    let mut kept: BTreeMap<String, Mark> = marks.into_iter().filter(|(key, _)| listed(key) || changed(Path::new(key)).is_some_and(|at| now.saturating_sub(at) < KEPT_UNLISTED)).collect();
    for chat in chats {
        let key = chat.path.display().to_string();
        let Ok((mark, found)) = read_on(kept.remove(&key), &chat.path, started) else { continue };
        for held in found.into_iter().filter(|held| worth(&held.words) && !typed_by_kebbie(chat, &held.words)) {
            if let Ok(Some(_)) = super::add_case(state, &NewCase { source: Source::Chat, from: &chat.from, words: &held.words, context: &context(&held), fingerprint: None }, now) {
                added += 1;
            }
        }
        kept.insert(key, mark);
    }
    if let Ok(text) = serde_json::to_string_pretty(&Marks { started, marks: kept }) {
        let _ = crate::state::write(&file(state), text);
    }
    if added == 0 { Vec::new() } else { vec![format!("faults: {added} of the user's words to weigh")] }
}

pub fn board_answer_in(state: &Path, task: &str, question: &str, label: Option<&str>, comment: &str, now: u64) -> Result<Option<String>> {
    let comment = comment.trim();
    let correcting = match label {
        Some(label) => CORRECTING.contains(&label),
        None => !comment.is_empty(),
    };
    if !correcting {
        return Ok(None);
    }
    let words = if comment.is_empty() { label.unwrap_or_default().to_string() } else { comment.to_string() };
    let chose = label.map_or_else(|| "their own words in place of the options".to_string(), str::to_string);
    let context = format!("The question on the board: {}\nThe user chose: {chose}", super::clipped(question, 800));
    super::add_case(state, &NewCase { source: Source::Board, from: task, words: &words, context: &context, fingerprint: None }, now)
}

pub fn board_answer(task: &str, question: &str, label: Option<&str>, comment: &str) {
    if cfg!(test) || !super::watch::on() {
        return;
    }
    let Ok(state) = crate::lead::state_dir() else { return };
    if let Err(error) = board_answer_in(&state, task, question, label, comment, crate::messages::now()) {
        eprintln!("kebbie: the fault watcher could not keep the user's answer: {error}");
    }
}
