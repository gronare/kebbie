use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::peers;
use crate::questions;
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use super::{Lock, copies_for_lead, holds, is_copy, is_told, seen_count};

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub id: String,
    pub at: u64,
    pub from: String,
    pub to: String,
    pub text: String,
    pub held: bool,
    pub report: Option<Report>,
    pub ended: bool,
    pub question: Option<Question>,
    pub about: Option<String>,
    pub kind: Option<String>,
    pub landed: Option<String>,
    pub why: Option<String>,
    pub forced: bool,
    pub fyi: bool,
}

// A question a task put to the user through kebbie: its options as (label, meaning), the recommended one (1-based), the plan to read (a file, or vault:<note>), the options (0-based) that take the user's comment, whether the lead kept it for the user while they were away, and when the lead marked it answered through itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub options: Vec<(String, String)>,
    pub recommend: usize,
    pub plan: Option<String>,
    pub plan_at: Option<i64>,
    pub comment: Vec<usize>,
    pub answered: bool,
    pub waits: bool,
    pub relayed: Option<u64>,
}

pub const USER_LABEL: &str = "you";

// What a task's Claude says at the end of a turn: where it stands, and what the user should do next.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub state: String,
    pub next: String,
    pub head: Option<String>,
    pub after: Vec<String>,
}

impl Message {
    // A report the user has nothing to do for says "nothing".
    pub fn next_for_you(&self) -> Option<&str> {
        let next = self.report.as_ref()?.next.trim();
        (!next.is_empty() && !next.eq_ignore_ascii_case("nothing")).then_some(next)
    }
}

const LOG: &str = "messages.jsonl";
pub(super) const LEAD_MARK: &str = "messages-lead-read";
pub(super) const ROTATE_BYTES: u64 = 1 << 20;

pub(super) fn log_in(dir: &Path) -> PathBuf {
    dir.join(LOG)
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// A log line's id; a state line marks the first message with its id, so two messages must never share one, even from one process in one second.
pub fn new_id(what: &str) -> String {
    format!("{}-{}-{}-{what}", now(), std::process::id(), NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

fn question_from(record: &Value) -> Option<Question> {
    let question = record.get("question")?;
    let listed = question["options"].as_array()?;
    let options = listed.iter()
        .map(|option| (option["label"].as_str().unwrap_or_default().to_string(), option["meaning"].as_str().unwrap_or_default().to_string()))
        .collect();
    let plan = question["plan"].as_str().map(str::to_string);
    let marked = listed.iter().enumerate().filter(|(_, option)| option["comment"].as_bool() == Some(true)).map(|(index, _)| index).collect();
    // A plan asked before options carried their comment mark still takes one on its three commented answers.
    let old_plan = plan.is_some() && listed.iter().all(|option| option.get("comment").is_none());
    let comment = if old_plan { questions::PLAN_COMMENT.to_vec() } else { marked };
    Some(Question { options, recommend: question["recommend"].as_u64().unwrap_or(1) as usize, plan, plan_at: question["plan_at_ms"].as_i64().or_else(|| question["plan_at"].as_f64().map(questions::millis)), comment, answered: false, waits: false, relayed: None })
}

// The log is append-only: a message line, and later lines with only its id and its new state (delivered, answered, kept for the user).
pub fn fold(log: &str) -> Vec<Message> {
    let mut messages: Vec<Message> = Vec::new();
    for record in log.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
        let id = record["id"].as_str().unwrap_or_default();
        let held = record["state"] == "held";
        match record["text"].as_str() {
            Some(text) => messages.push(Message {
                id: id.to_string(),
                at: record["at"].as_u64().unwrap_or(0),
                from: record["from"].as_str().unwrap_or_default().to_string(),
                to: record["to"].as_str().unwrap_or_default().to_string(),
                text: text.to_string(),
                held,
                ended: record["state"] == "ended",
                report: record["report"]["state"].as_str().map(|state| Report {
                    state: state.to_string(),
                    next: record["report"]["next"].as_str().unwrap_or_default().to_string(),
                    head: record["report"]["head"].as_str().map(str::to_string),
                    after: record["report"]["after"].as_array().map(|after| after.iter().filter_map(|address| address.as_str().map(str::to_string)).collect()).unwrap_or_default(),
                }),
                question: question_from(&record),
                about: record["about"].as_str().map(str::to_string),
                kind: record["kind"].as_str().map(str::to_string),
                landed: record["landed"].as_str().map(str::to_string),
                why: record["why"].as_str().filter(|_| held).map(str::to_string),
                forced: record["forced"] == true,
                fyi: record["fyi"] == true,
            }),
            None => {
                let Some(message) = messages.iter_mut().find(|message| message.id == id) else { continue };
                match (record["state"].as_str(), message.question.as_mut()) {
                    (Some("answered"), Some(question)) => question.answered = true,
                    (Some("waits"), Some(question)) => question.waits = true,
                    (Some("relayed"), Some(question)) => question.relayed = Some(record["at"].as_u64().unwrap_or(0)),
                    (_, Some(_)) => {}
                    _ => {
                        message.held = held;
                        message.why = record["why"].as_str().filter(|_| held).map(str::to_string);
                        message.forced |= record["forced"] == true;
                    }
                }
            }
        }
    }
    messages
}

type LogKey = (u64, u64, SystemTime);
type Folded = HashMap<PathBuf, (LogKey, Vec<Message>)>;

// The board and `kebbie messages` read the log every tick; an unchanged file (same inode, length and mtime) is folded once.
static FOLDED: Mutex<Option<Folded>> = Mutex::new(None);

fn load_from(path: &Path) -> Vec<Message> {
    let Ok(mut file) = File::open(path) else { return Vec::new() };
    let Ok(metadata) = file.metadata() else { return Vec::new() };
    let key = (metadata.ino(), metadata.len(), metadata.modified().unwrap_or(UNIX_EPOCH));
    let mut folded = FOLDED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let folded = folded.get_or_insert_with(HashMap::new);
    if let Some((cached, messages)) = folded.get(path) && *cached == key {
        return messages.clone();
    }
    let mut log = String::new();
    if file.read_to_string(&mut log).is_err() {
        return Vec::new();
    }
    let messages = fold(&log);
    folded.insert(path.to_path_buf(), (key, messages.clone()));
    messages
}

pub fn load_in(dir: &Path) -> Vec<Message> {
    load_from(&log_in(dir))
}

// The log with the one rotation keeps beside it, for what must reach back past a rotation, such as a task's answers from the user.
pub fn load_with_rotated_in(dir: &Path) -> Vec<Message> {
    let rotated = fs::read_to_string(dir.join(format!("{LOG}.1"))).unwrap_or_default();
    let current = fs::read_to_string(log_in(dir)).unwrap_or_default();
    fold(&format!("{rotated}\n{current}"))
}

pub fn load() -> Vec<Message> {
    state_dir().map(|dir| load_in(&dir)).unwrap_or_default()
}

pub fn append(record: &Value) -> Result<()> {
    append_in(&state_dir()?, record, ROTATE_BYTES)
}

pub fn append_in(dir: &Path, record: &Value, rotate_past: u64) -> Result<()> {
    let _lock = if holds(dir) { None } else { Lock::take_in(dir, true)? };
    if fs::metadata(log_in(dir)).is_ok_and(|metadata| metadata.len() > rotate_past) {
        rotate_in(dir, crate::permit::away_since_in(dir))?;
    }
    let mut file = crate::state::append_to(&log_in(dir)).map_err(|error| error.to_string())?;
    writeln!(file, "{record}").map_err(|error| error.to_string())
}

// What a rotation must carry into the new log: held messages, open questions, each address's latest report and end, what the lead has not seen yet, and while the user is away the pushes and calls their summary is written from.
fn live_ids(messages: &[Message], away: Option<u64>, lead_mark: Option<&str>) -> HashSet<String> {
    let mut live: HashSet<String> = messages.iter().filter(|message| message.held).map(|message| message.id.clone()).collect();
    live.extend(questions::open(messages, |_| None).into_iter().map(|question| question.id.clone()));
    live.extend(latest_reports(messages).into_values().map(|report| report.id));
    let ends: HashMap<&str, &str> = messages.iter().filter(|message| message.ended).map(|message| (message.to.as_str(), message.id.as_str())).collect();
    live.extend(ends.into_values().map(str::to_string));
    if lead_mark.is_some() {
        live.extend(copies_for_lead(messages, seen_count(messages, lead_mark)).into_iter().filter(|message| is_told(message) || is_copy(message) || message.fyi).map(|message| message.id.clone()));
    }
    live.extend(messages.iter().filter(|message| message.kind.is_some() && away.is_some_and(|since| message.at >= since)).map(|message| message.id.clone()));
    live
}

// The lead's mark names the last message it saw; when rotation drops that one, the last kept message before it takes its place.
fn mark_after_rotation(messages: &[Message], live: &HashSet<String>, mark: &str) -> String {
    let Some(seen) = messages.iter().rposition(|message| message.id == mark) else { return mark.to_string() };
    messages[..=seen].iter().rev().find(|message| live.contains(&message.id)).map(|message| message.id.clone()).unwrap_or_default()
}

// The caller holds the lock. The old log is kept as messages.jsonl.1, and the new one is swapped in whole, so a reader never sees it missing.
pub fn rotate_in(dir: &Path, away: Option<u64>) -> Result<()> {
    let path = log_in(dir);
    let log = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let messages = fold(&log);
    let mark = dir.join(LEAD_MARK);
    let seen = fs::read_to_string(&mark).ok().map(|seen| seen.trim().to_string());
    let live = live_ids(&messages, away, seen.as_deref());
    let kept: String = log.lines()
        .filter(|line| serde_json::from_str::<Value>(line).ok().is_some_and(|record| record["id"].as_str().is_some_and(|id| live.contains(id))))
        .map(|line| format!("{line}\n")).collect();
    if let Some(seen) = seen {
        crate::state::write(&mark, mark_after_rotation(&messages, &live, &seen))?;
    }
    crate::state::write(&dir.join(format!("{LOG}.1")), &log)?;
    crate::state::write(&path, kept)
}

pub const PROMPT_KIND: &str = "prompt";
pub const NOTICE_KIND: &str = "notice";
pub const RULES_KIND: &str = "rules-check";
pub const HAND_KIND: &str = "hand-over";

// A report goes to the lead through the log only: typed into its chat, every turn of every task would start a turn of the lead.
pub fn log_report(from: &str, report: &Report, text: &str) -> Result<()> {
    let id = new_id("report");
    append(&json!({ "id": id, "at": now(), "from": from, "to": LEAD_LABEL, "text": text, "state": "logged", "report": { "state": report.state, "next": report.next, "head": report.head, "after": report.after } }))
}

// What the board shows as a task's latest report: an ask the user or the lead answered since shows as that answer, the task working on it, until the task reports again.
pub fn shown_report(messages: &[Message], report: &Message) -> Message {
    if report.report.as_ref().is_none_or(|said| said.state != "needs-you") {
        return report.clone();
    }
    let answer = messages.iter().skip_while(|message| message.id != report.id).skip(1)
        .filter(|message| message.to == report.from && matches!(message.kind.as_deref(), Some("answer" | "decision"))).last();
    match answer {
        Some(answer) => Message { id: answer.id.clone(), at: answer.at, text: answer.text.clone(), report: Some(Report { state: "working".to_string(), next: String::new(), head: None, after: Vec::new() }), ..report.clone() },
        None => report.clone(),
    }
}

// The end of a task's life, so a later task by the same name does not inherit its reports; landed is the "sha subject" lines a done task put on the default branch.
pub fn log_end(task: &str, repo: &str, how: &str, landed: Option<&str>, kind: Option<&str>) -> Result<()> {
    let id = new_id("end");
    append(&json!({ "id": id, "at": now(), "from": "kebbie", "to": peers::address(task, repo), "text": format!("{task} ended ({how})"), "state": "ended", "landed": landed, "kind": kind }))
}

pub fn ended_how(message: &Message) -> Option<&str> {
    message.text.rsplit_once(" ended (")?.1.strip_suffix(')').filter(|_| message.ended)
}

// Each session's latest report since its address last ended, by its address.
pub fn latest_reports(messages: &[Message]) -> HashMap<String, Message> {
    let mut latest = HashMap::new();
    for message in messages {
        if message.ended {
            latest.remove(&message.to);
        } else if message.report.is_some() {
            latest.insert(message.from.clone(), message.clone());
        }
    }
    latest
}
