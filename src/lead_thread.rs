use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kebbie_hub_protocol::{CommandResult, Delivery, Id, MAX_LEAD_TURN_CHARS, Outcome, Reason, Turn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sh::Result;

const FILE: &str = "lead-thread.json";
const LOCK: &str = "lead-thread.lock";
const KEPT_SENT: usize = 200;
const KEPT_SWITCHES: usize = 100;
const KEPT_EVENTS: usize = 4_000;
const FIRST_READ: u64 = 16 * 1024 * 1024;
// A turn the transcript shows as running long after its last write was cut off without its end, as by a killed Claude.
const STALE_TURN: u64 = 30 * 60;
pub const DELIVERED: &str = "Delivered to the lead.";

// A message the user wrote to the lead on a device: what was typed for it, the log line that carries it, and the hub directory its result goes back through.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sent {
    pub command: String,
    pub device: String,
    pub typed: String,
    pub text: String,
    pub at: u64,
    pub exp: u64,
    pub message: String,
    pub dir: PathBuf,
    #[serde(default)]
    pub delivered: Option<u64>,
}

// Away mode switched, at the computer or from a device; `by` is when the switch was asked for, which decides between two devices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Switch {
    pub id: String,
    pub at: u64,
    pub by: u64,
    pub on: bool,
    #[serde(default)]
    pub device: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub sent: Vec<Sent>,
    #[serde(default)]
    pub switches: Vec<Switch>,
    #[serde(default)]
    pub answered: Option<String>,
}

pub fn load(state: &Path) -> Store {
    std::fs::read_to_string(state.join(FILE)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub fn change<T>(state: &Path, change: impl FnOnce(&mut Store) -> T) -> Result<T> {
    let _locked = crate::state::Locked::wait(&state.join(LOCK))?;
    let before = load(state);
    let mut store = before.clone();
    let done = change(&mut store);
    let keep_from = store.sent.len().saturating_sub(KEPT_SENT);
    store.sent.drain(..keep_from);
    let keep_from = store.switches.len().saturating_sub(KEPT_SWITCHES);
    store.switches.drain(..keep_from);
    if store != before {
        crate::state::write(&state.join(FILE), serde_json::to_string_pretty(&store).map_err(|error| error.to_string())?)?;
    }
    Ok(done)
}

pub fn typed(name: &str, text: &str) -> String {
    format!("(from {}) {}", name.trim(), text.trim())
}

// Away mode as it stood at a time: the latest switch before it, or the current stretch when kebbie recorded no switch.
fn away_at(store: &Store, away_since: Option<u64>, at: u64) -> bool {
    match store.switches.iter().filter(|switch| switch.at <= at).max_by_key(|switch| switch.at) {
        Some(switch) => switch.on,
        None => away_since.is_some_and(|since| since <= at),
    }
}

pub fn switched(state: &Path, on: bool, at: u64, by: u64, id: Option<&str>, device: Option<&str>) -> Result<()> {
    change(state, |store| {
        let id = id.map_or_else(|| format!("away-{at}-{}", store.switches.len()), str::to_string);
        if !store.switches.iter().any(|switch| switch.id == id) {
            store.switches.push(Switch { id, at, by, on, device: device.map(str::to_string) });
        }
    })
}

pub fn newest_switch(state: &Path) -> Option<u64> {
    load(state).switches.iter().map(|switch| switch.by).max()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    You,
    Away,
    Kebbie,
}

// What the transcript says, reduced to what the thread is built from: who began a turn, the lead's replies, a hook's word and the turn's end.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Input { id: String, at: u64, input: Input, text: String },
    Reply { id: String, at: u64, text: String },
    Hook,
    End { at: u64 },
}

const AWAY_MARKS: [&str; 2] = [crate::permit::AWAY_ON, crate::permit::AWAY_OFF];

fn input_of(text: &str) -> Input {
    if AWAY_MARKS.iter().any(|mark| text.contains(&format!("[from kebbie] {mark}"))) { Input::Away } else { Input::Kebbie }
}

fn content_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) if !blocks.is_empty() && blocks.iter().all(|block| block["type"] == "text") => Some(blocks.iter().filter_map(|block| block["text"].as_str()).collect::<Vec<_>>().join("\n")),
        _ => None,
    }
}

fn seconds(record: &Value) -> u64 {
    record["timestamp"].as_str().and_then(crate::typing::iso_millis).map_or(0, |millis| millis / 1000)
}

fn said(id: &str, at: u64, text: &str) -> Option<Event> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.starts_with("[Request interrupted") {
        return Some(Event::End { at });
    }
    if text.starts_with("[from ") || text.contains("<cross-session-message") {
        return Some(Event::Input { id: id.to_string(), at, input: input_of(text), text: String::new() });
    }
    if text.starts_with('<') {
        return None;
    }
    Some(Event::Input { id: id.to_string(), at, input: Input::You, text: text.to_string() })
}

// One transcript line; most lines are request bodies and bookkeeping, so they are skipped before they are parsed.
pub fn event(line: &str) -> Option<Event> {
    let wanted = ["\"type\":\"user\"", "\"type\":\"assistant\"", "\"turn_duration\"", "\"stop_hook_summary\"", "\"queued_command\""];
    if !wanted.iter().any(|mark| line.contains(mark)) || line.contains("\"type\":\"tool_result\"") {
        return None;
    }
    let record: Value = serde_json::from_str(line).ok()?;
    if record["isSidechain"] == true {
        return None;
    }
    let id = record["uuid"].as_str().unwrap_or_default();
    let at = seconds(&record);
    match record["type"].as_str()? {
        "user" => {
            let text = content_text(&record["message"]["content"])?;
            if record["isMeta"] == true && !text.contains("<cross-session-message") {
                return None;
            }
            said(id, at, &text)
        }
        "attachment" if record["attachment"]["type"] == "queued_command" && record["attachment"]["commandMode"] == "prompt" => said(id, at, &content_text(&record["attachment"]["prompt"])?),
        "assistant" if record["message"]["stop_reason"] != "tool_use" => {
            let text: Vec<&str> = record["message"]["content"].as_array()?.iter().filter(|block| block["type"] == "text").filter_map(|block| block["text"].as_str()).collect();
            let text = text.join("\n\n");
            (!text.trim().is_empty()).then(|| Event::Reply { id: id.to_string(), at, text })
        }
        "system" if record["subtype"] == "stop_hook_summary" => Some(Event::Hook),
        "system" if record["subtype"] == "turn_duration" => Some(Event::End { at }),
        _ => None,
    }
}

// The lead's transcript read so far: only what was appended since the last read is parsed, a line at a time, a first read takes only the file's last FIRST_READ bytes, and a file replaced or cut short is read again.
#[derive(Clone, Debug, Default)]
pub struct Reader {
    pub path: PathBuf,
    pub inode: u64,
    pub offset: u64,
    pub written: u64,
    pub events: Vec<Event>,
}

impl Reader {
    pub fn read(&mut self, path: &Path) -> std::io::Result<()> {
        self.read_from(path, FIRST_READ)
    }

    fn read_from(&mut self, path: &Path, first: u64) -> std::io::Result<()> {
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        if self.path != path || self.inode != metadata.ino() || metadata.len() < self.offset {
            *self = Reader { path: path.to_path_buf(), inode: metadata.ino(), ..Reader::default() };
        }
        self.written = metadata.mtime().max(0) as u64;
        if metadata.len() == self.offset {
            return Ok(());
        }
        let tail = self.offset == 0 && metadata.len() > first;
        let start = if tail { metadata.len() - first } else { self.offset };
        file.seek(SeekFrom::Start(start))?;
        let mut lines = BufReader::new(file.take(metadata.len() - start));
        let mut line = Vec::new();
        let mut offset = start;
        if tail {
            offset += lines.read_until(b'\n', &mut line)? as u64;
            if line.last() != Some(&b'\n') {
                return Ok(());
            }
        }
        loop {
            line.clear();
            let read = lines.read_until(b'\n', &mut line)?;
            if read == 0 || line.last() != Some(&b'\n') {
                break;
            }
            offset += read as u64;
            if let Some(event) = std::str::from_utf8(&line[..read - 1]).ok().and_then(event) {
                self.events.push(event);
                if self.events.len() >= 2 * KEPT_EVENTS {
                    self.events.drain(..KEPT_EVENTS);
                }
            }
        }
        self.offset = offset;
        let keep_from = self.events.len().saturating_sub(KEPT_EVENTS);
        self.events.drain(..keep_from);
        Ok(())
    }
}

static READ: Mutex<Option<Reader>> = Mutex::new(None);

// What a held message waits on, from its line in the messages log.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Waits {
    Turn,
    Draft,
    Typed,
}

// The thread as the phone shows it, and what changed for the device that wrote to the lead.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thread {
    pub turns: Vec<Turn>,
    pub working_since: Option<u64>,
    pub replied_at: Option<u64>,
    pub delivered: Vec<(String, u64)>,
    pub answers: Vec<(String, String)>,
}

fn clean(text: &str) -> String {
    let text: String = text.chars().filter(|char| !char.is_control() || matches!(char, '\n' | '\t')).collect();
    let text = text.trim();
    if text.chars().count() <= MAX_LEAD_TURN_CHARS { text.to_string() } else { format!("{}…", text.chars().take(MAX_LEAD_TURN_CHARS - 1).collect::<String>()) }
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn carries(said: &str, typed: &str) -> bool {
    let wanted: String = normalized(typed).chars().take(200).collect();
    !wanted.is_empty() && normalized(said).starts_with(&wanted)
}

fn turn_id(id: &str, at: u64, seen: &mut std::collections::BTreeSet<String>) -> String {
    let valid = !id.is_empty() && id.len() <= 64 && id.chars().all(|char| char.is_ascii_alphanumeric() || matches!(char, '-' | '_'));
    let mut id = if valid { id.to_string() } else { format!("t-{at}") };
    while !seen.insert(id.clone()) {
        id = format!("{}-{}", &id[..id.len().min(56)], seen.len());
    }
    id
}

#[derive(Clone, Copy, PartialEq)]
enum Began {
    You,
    Device,
    Away,
    Kebbie,
}

// The whole thread from the transcript's events and kebbie's own records: your turns at the computer, each message from a device with how far it got, each away switch, and the lead's replies to you, or every reply while you are away; nothing it said to kebbie or a hook otherwise.
pub fn assemble(events: &[Event], store: &Store, waits: &dyn Fn(&str) -> Option<Waits>, away_since: Option<u64>, written: u64, now: u64) -> Thread {
    let mut thread = Thread::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut matched: Vec<Option<u64>> = vec![None; store.sent.len()];
    let mut turns: Vec<(u64, Turn)> = Vec::new();
    let (mut began, mut hooked, mut open, mut working): (Option<Began>, bool, Option<usize>, Option<u64>) = (None, false, None, None);
    let first = events.iter().find_map(|event| match event {
        Event::Input { at, .. } | Event::Reply { at, .. } | Event::End { at } => Some(*at),
        Event::Hook => None,
    });
    for event in events {
        match event {
            Event::Input { id, at, input, text } => {
                (hooked, open) = (false, None);
                working = working.or(Some(*at));
                began = Some(match input {
                    Input::Kebbie => Began::Kebbie,
                    Input::Away => Began::Away,
                    Input::You => match store.sent.iter().enumerate().find(|(index, sent)| matched[*index].is_none() && sent.at <= at + 5 && carries(text, &sent.typed)) {
                        Some((index, _)) => {
                            matched[index] = Some(*at);
                            Began::Device
                        }
                        None => {
                            let text = clean(text);
                            if !text.is_empty() {
                                turns.push((*at, Turn::You { id: turn_id(id, *at, &mut seen), at: *at, device: None, text: Some(text), away: None, delivery: None }));
                            }
                            Began::You
                        }
                    },
                });
            }
            Event::Reply { id, at, text } => {
                let shown = matches!(began, Some(Began::You | Began::Device | Began::Away)) || away_at(store, away_since, *at);
                let text = clean(text);
                if hooked || !shown || text.is_empty() {
                    continue;
                }
                match open.and_then(|at| turns.get_mut(at)) {
                    Some((_, Turn::Lead { text: before, .. })) => *before = clean(&format!("{before}\n\n{text}")),
                    _ => {
                        let id = turn_id(id, *at, &mut seen);
                        if began == Some(Began::Device) || away_at(store, away_since, *at) {
                            thread.answers.push((id.clone(), text.clone()));
                        }
                        turns.push((*at, Turn::Lead { id, at: *at, text }));
                        open = Some(turns.len() - 1);
                    }
                }
            }
            Event::Hook => hooked = true,
            Event::End { .. } => {
                (working, open) = (None, None);
            }
        }
    }
    for (index, sent) in store.sent.iter().enumerate() {
        let delivery = match (matched[index], sent.delivered) {
            (Some(at), _) => {
                if sent.delivered.is_none() {
                    thread.delivered.push((sent.command.clone(), at.max(sent.at)));
                }
                Delivery::Delivered { at: at.max(sent.at) }
            }
            (None, Some(at)) if first.is_some_and(|first| at >= first) => Delivery::Delivered { at: at.max(sent.at) },
            (None, Some(_)) => continue,
            (None, None) if now > sent.exp => continue,
            (None, None) => match waits(&sent.message) {
                Some(Waits::Draft) => Delivery::WaitingDraft,
                _ => Delivery::WaitingTurn,
            },
        };
        let Ok(device) = Id::parse(&sent.device) else { continue };
        let text = clean(&sent.text);
        if text.is_empty() {
            continue;
        }
        turns.push((sent.at, Turn::You { id: turn_id(&sent.command, sent.at, &mut seen), at: sent.at, device: Some(device), text: Some(text), away: None, delivery: Some(delivery) }));
    }
    for switch in store.switches.iter().filter(|switch| first.is_none_or(|first| switch.at >= first)) {
        let device = switch.device.as_deref().and_then(|device| Id::parse(device).ok());
        turns.push((switch.at, Turn::You { id: turn_id(&switch.id, switch.at, &mut seen), at: switch.at, device, text: None, away: Some(switch.on), delivery: None }));
    }
    turns.sort_by_key(|(at, _)| *at);
    thread.turns = turns.into_iter().map(|(_, turn)| turn).collect();
    thread.replied_at = thread.turns.iter().rev().find_map(|turn| matches!(turn, Turn::Lead { .. }).then(|| turn.at()));
    thread.working_since = working.filter(|_| now.saturating_sub(written) < STALE_TURN);
    thread
}

fn waits_in(state: &Path) -> impl Fn(&str) -> Option<Waits> {
    let messages = crate::messages::load_in(state);
    move |id: &str| {
        let message = messages.iter().find(|message| message.id == id)?;
        if !message.held {
            return Some(Waits::Typed);
        }
        Some(match message.why.as_deref() {
            Some(crate::messages::DRAFT_HOLD | crate::messages::EARLIER_TYPING) => Waits::Draft,
            _ => Waits::Turn,
        })
    }
}

pub fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().trim_start_matches(['#', '*', '-', ' ']).trim_end_matches('*').trim().to_string()
}

pub type Answer<'a> = dyn Fn(&Path, CommandResult) -> Result<()> + 'a;

// One round: the transcript read on from where it was, each message the lead took answered done, each that waited past its week answered expired and dropped, and a notice for a reply that came while you were away or to a device.
pub fn tick(state: &Path, now: u64, answer: &Answer<'_>, notify: &dyn Fn(&str)) -> Option<Thread> {
    let mut read = READ.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    tick_from(state, crate::lead::transcript_file(state).as_deref(), read.get_or_insert_with(Reader::default), now, answer, notify)
}

pub fn tick_from(state: &Path, file: Option<&Path>, reader: &mut Reader, now: u64, answer: &Answer<'_>, notify: &dyn Fn(&str)) -> Option<Thread> {
    let store = load(state);
    if let Some(file) = file {
        let _ = reader.read(file);
    }
    if reader.events.is_empty() && store == Store::default() && file.is_none() {
        return None;
    }
    let thread = assemble(&reader.events, &store, &waits_in(state), crate::permit::away_since_in(state), reader.written, now);
    settle(state, &store, &thread, now, answer, notify);
    Some(thread)
}

fn settle(state: &Path, store: &Store, thread: &Thread, now: u64, answer: &Answer<'_>, notify: &dyn Fn(&str)) {
    let result = |sent: &Sent, outcome: Outcome, text: &str| {
        let (Ok(command), Ok(device)) = (Id::parse(&sent.command), Id::parse(&sent.device)) else { return };
        let _ = answer(&sent.dir, CommandResult { command, device, outcome, text: text.to_string(), blob: None });
    };
    for (command, _) in &thread.delivered {
        if let Some(sent) = store.sent.iter().find(|sent| &sent.command == command) {
            result(sent, Outcome::Done, DELIVERED);
        }
    }
    let expired: Vec<&Sent> = store.sent.iter().filter(|sent| sent.delivered.is_none() && now > sent.exp && !thread.delivered.iter().any(|(command, _)| command == &sent.command)).collect();
    for sent in &expired {
        result(sent, Outcome::Refused { reason: Reason::Expired }, "Not delivered: it waited a week and the lead never took it; send it again");
        let _ = crate::messages::drop_held_in(state, &sent.message);
    }
    let newest = thread.answers.last().cloned();
    let told = store.answered.is_some();
    if let Some((_, text)) = newest.as_ref().filter(|(id, _)| told && store.answered.as_ref() != Some(id)) {
        notify(&first_line(text));
    }
    let gone: Vec<String> = expired.iter().map(|sent| sent.command.clone()).collect();
    if thread.delivered.is_empty() && gone.is_empty() && newest.as_ref().is_none_or(|(id, _)| store.answered.as_ref() == Some(id)) && told {
        return;
    }
    let _ = change(state, |store| {
        for (command, at) in &thread.delivered {
            if let Some(sent) = store.sent.iter_mut().find(|sent| &sent.command == command) {
                sent.delivered = Some(*at);
            }
        }
        store.sent.retain(|sent| !gone.contains(&sent.command));
        store.answered = newest.map(|(id, _)| id).or_else(|| store.answered.clone()).or_else(|| Some(String::new()));
    });
}

#[cfg(test)]
mod tests;
