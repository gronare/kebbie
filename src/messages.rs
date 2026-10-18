use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use serde_json::{Value, json};

use crate::board::{FAINT, MAUVE, MUTED, PEACH, SUBTEXT, SURFACE, TEAL, TEXT, YELLOW, bold, style};
use crate::herdr;
use crate::lead::state_dir;
use crate::peers::{self, Session};
use crate::questions;
use crate::sh::Result;
use crate::status::LEAD_LABEL;

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
}

// A question a task put to the user through kelpie: its options as (label, meaning), the recommended one (1-based), and the plan to read.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub options: Vec<(String, String)>,
    pub recommend: usize,
    pub plan: Option<String>,
    pub given: bool,
    pub answered: bool,
    pub waits: bool,
}

// What a task's Claude says at the end of a turn: where it stands, and what the user should do next.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub state: String,
    pub next: String,
}

impl Message {
    // A report the user has nothing to do for says "nothing".
    pub fn next_for_you(&self) -> Option<&str> {
        let next = self.report.as_ref()?.next.trim();
        (!next.is_empty() && !next.eq_ignore_ascii_case("nothing")).then_some(next)
    }
}

const LOG: &str = "messages.jsonl";
const LEAD_MARK: &str = "messages-lead-read";
const ROTATE_BYTES: u64 = 1 << 20;

fn log_in(dir: &Path) -> PathBuf {
    dir.join(LOG)
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

fn question_from(record: &Value) -> Option<Question> {
    let question = record.get("question")?;
    let options = question["options"].as_array()?.iter()
        .map(|option| (option["label"].as_str().unwrap_or_default().to_string(), option["meaning"].as_str().unwrap_or_default().to_string()))
        .collect();
    Some(Question { options, recommend: question["recommend"].as_u64().unwrap_or(1) as usize, plan: question["plan"].as_str().map(str::to_string), given: false, answered: false, waits: false })
}

// The log is append-only: a message line, and later lines with only its id and its new state (delivered, given to the lead, answered).
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
                }),
                question: question_from(&record),
                about: record["about"].as_str().map(str::to_string),
                kind: record["kind"].as_str().map(str::to_string),
            }),
            None => {
                let Some(message) = messages.iter_mut().find(|message| message.id == id) else { continue };
                match (record["state"].as_str(), message.question.as_mut()) {
                    (Some("given"), Some(question)) => question.given = true,
                    (Some("answered"), Some(question)) => question.answered = true,
                    (Some("waits"), Some(question)) => {
                        question.waits = true;
                        question.given = false;
                    }
                    _ => message.held = held,
                }
            }
        }
    }
    messages
}

type LogKey = (u64, u64, SystemTime);
type Folded = HashMap<PathBuf, (LogKey, Vec<Message>)>;

// The board and the messages pane read the log every tick; an unchanged file (same inode, length and mtime) is folded once.
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
    let mut file = OpenOptions::new().create(true).append(true).open(log_in(dir)).map_err(|error| error.to_string())?;
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
        live.extend(copies_for_lead(messages, seen_count(messages, lead_mark)).into_iter().filter(|message| is_told(message)).map(|message| message.id.clone()));
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
        fs::write(&mark, mark_after_rotation(&messages, &live, &seen)).map_err(|error| error.to_string())?;
    }
    fs::copy(&path, dir.join(format!("{LOG}.1"))).map_err(|error| error.to_string())?;
    let fresh = dir.join(format!("{LOG}.new"));
    fs::write(&fresh, kept).map_err(|error| error.to_string())?;
    fs::rename(&fresh, &path).map_err(|error| error.to_string())
}

// Nothing is ever typed into a question: a session waiting for an answer, or with no Claude, gets the message later.
pub fn hold_reason(session: &Session) -> Option<&'static str> {
    match (&session.pane, session.status.as_deref()) {
        (None, _) => Some("has no Claude running"),
        (Some(_), Some("blocked")) => Some("is waiting for an answer"),
        _ => None,
    }
}

fn deliver(session: &Session, from: &str, text: &str) -> std::result::Result<(), &'static str> {
    if let Some(reason) = hold_reason(session) {
        return Err(reason);
    }
    let pane = session.pane.as_deref().unwrap_or_default();
    match herdr::prompt_agent(pane, &format!("[from {from}] {text}")) {
        Ok(()) => Ok(()),
        Err(error) if error.contains("agent_blocked") => Err("is waiting for an answer"),
        Err(_) => Err("could not be reached"),
    }
}

// One sender at a time, so a held message is never delivered twice; a lock older than a minute is left over from a crash.
pub struct Lock(PathBuf);

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
        for _ in 0..100 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => {
                    HELD.with(|held| held.borrow_mut().push(path.clone()));
                    return Ok(Some(Lock(path)));
                }
                Err(_) if fs::metadata(&path).and_then(|metadata| metadata.modified()).is_ok_and(|time| time.elapsed().unwrap_or_default() > Duration::from_secs(60)) => {
                    let _ = fs::remove_file(&path);
                }
                Err(_) if wait => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => return Ok(None),
            }
        }
        Err("Another kelpie is holding the message log; try again".to_string())
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        HELD.with(|held| held.borrow_mut().retain(|path| path != &self.0));
        let _ = fs::remove_file(&self.0);
    }
}

// A task with a question open waits for the user's answer, which only the lead brings; the rest is held until then.
pub fn question_hold(from: &str, question_open: bool) -> Option<&'static str> {
    (question_open && from != LEAD_LABEL).then_some("waits for the user's answer to its question")
}

// A question the lead was to relay that has since been answered or dropped is never typed into its chat.
fn stale(message: &Message, open: &[&Message]) -> bool {
    message.about.as_ref().is_some_and(|id| !open.iter().any(|question| &question.id == id))
}

fn flush_held(sessions: &[Session]) -> Result<usize> {
    let messages = load();
    let open = questions::open_now(&messages);
    let mut delivered = 0;
    for message in messages.iter().filter(|message| message.held) {
        if stale(message, &open) {
            append(&json!({ "id": message.id, "at": now(), "state": "dropped" }))?;
            continue;
        }
        let Some(session) = sessions.iter().find(|session| session.address == message.to) else { continue };
        let question_open = open.iter().any(|question| question.from == session.address);
        if question_hold(&message.from, question_open).is_none() && deliver(session, &message.from, &message.text).is_ok() {
            append(&json!({ "id": message.id, "at": now(), "state": "delivered" }))?;
            delivered += 1;
        }
    }
    Ok(delivered)
}

// Held messages to sessions that are free again, and the next question once the lead has passed one on; the messages pane runs this on every tick.
pub fn flush() -> Result<usize> {
    let messages = load();
    if !messages.iter().any(|message| message.held) && questions::next_to_give(&questions::open_now(&messages), crate::permit::away_since().is_some()).is_none() {
        return Ok(0);
    }
    let Some(_lock) = Lock::take(false)? else { return Ok(0) };
    let sessions = peers::sessions()?;
    let delivered = flush_held(&sessions)?;
    questions::release(&sessions)?;
    Ok(delivered)
}

fn send(from: &str, recipients: &[&Session], text: &str, sessions: &[Session]) -> Result<Vec<String>> {
    let _lock = Lock::take(true)?;
    let report = send_locked(from, recipients, text, sessions, None, None)?;
    questions::release(sessions)?;
    Ok(report)
}

// The caller holds the lock. The lead's message to a task whose question it was given is the user's answer to it.
pub fn send_locked(from: &str, recipients: &[&Session], text: &str, sessions: &[Session], about: Option<&str>, kind: Option<&str>) -> Result<Vec<String>> {
    flush_held(sessions)?;
    let mut report = Vec::new();
    for (index, session) in recipients.iter().enumerate() {
        let messages = load();
        let open = questions::open_now(&messages);
        let question = open.iter().find(|question| question.from == session.address);
        let earlier_held = messages.iter().any(|message| message.held && message.to == session.address);
        let answering = question.is_some_and(|question| questions::answers(from, question));
        let outcome = match question_hold(from, question.is_some()) {
            Some(reason) => Err(reason),
            None if earlier_held && !answering => Err("still has earlier messages held"),
            None => deliver(session, from, text),
        };
        let id = format!("{}-{}-{index}", now(), std::process::id());
        let mut record = json!({ "id": id, "at": now(), "from": from, "to": session.address, "text": text, "state": if outcome.is_ok() { "delivered" } else { "held" } });
        if let Some(about) = about {
            record["about"] = json!(about);
        }
        if let Some(kind) = kind {
            record["kind"] = json!(kind);
        }
        append(&record)?;
        if let Some(question) = question.filter(|_| answering) {
            append(&json!({ "id": question.id, "at": now(), "state": "answered" }))?;
        }
        report.push(match outcome {
            Ok(()) => format!("Told {}.", session.address),
            Err(reason) => format!("{} {reason}; held, and delivered when it is free.", session.address),
        });
    }
    Ok(report)
}

pub fn tell(cwd: &Path, to: &str, text: &str) -> Result<String> {
    if text.trim().is_empty() {
        return Err("kelpie tell needs a message: kelpie tell <task|task@repo|lead> \"<text>\"".to_string());
    }
    let from = peers::me(cwd).ok_or("kelpie tell runs in a task's worktree or in the lead Space")?;
    let sessions = peers::sessions()?;
    let recipients = peers::resolve(to, &from, &sessions)?;
    Ok(send(&from, &recipients, text.trim(), &sessions)?.join("\n"))
}

// kelpie's own word to the lead, for a change it would otherwise learn of from stale reports.
pub fn notify_lead(text: &str) -> Result<()> {
    let sessions = peers::sessions()?;
    let Ok(lead) = peers::resolve(LEAD_LABEL, "kelpie", &sessions) else { return Ok(()) };
    send("kelpie", &lead, text, &sessions).map(|_| ())
}

// kelpie's word to the lead that must not be lost: with no lead session yet, it waits in the log and reaches the lead once one runs.
pub fn tell_lead(text: &str, kind: Option<&str>) -> Result<()> {
    let sessions = peers::sessions().unwrap_or_default();
    let _lock = Lock::take(true)?;
    let Some(lead) = sessions.iter().find(|session| session.address == LEAD_LABEL) else {
        let id = format!("{}-{}-lead", now(), std::process::id());
        return append(&json!({ "id": id, "at": now(), "from": "kelpie", "to": LEAD_LABEL, "text": text, "state": "held", "kind": kind }));
    };
    send_locked("kelpie", &[lead], text, &sessions, None, kind)?;
    questions::release(&sessions)
}

// kelpie's own word to a task, held like any message while it waits for an answer.
pub fn notify(to: &str, text: &str) -> Result<()> {
    let sessions = peers::sessions()?;
    let Ok(recipients) = peers::resolve(to, "kelpie", &sessions) else { return Ok(()) };
    send("kelpie", &recipients, text, &sessions).map(|_| ())
}

// When a task starts in another repo, the halves already running learn of it.
pub fn announce(task: &str, repo: &str) -> Result<()> {
    let sessions = peers::sessions()?;
    let me = peers::address(task, repo);
    let Ok(halves) = peers::resolve(task, &me, &sessions) else { return Ok(()) };
    send("kelpie", &halves, &format!("{me} has started: the same task, in {repo}. Keep it informed of changes it depends on."), &sessions).map(|_| ())
}

// From kelpie itself, held like any message while the lead waits for an answer.
pub fn ask_lead(text: &str) -> Result<String> {
    let sessions = peers::sessions()?;
    let lead = sessions.iter().find(|session| session.address == LEAD_LABEL).ok_or("no lead session to check the rules")?;
    Ok(send("kelpie", &[lead], text, &sessions)?.join("\n"))
}

// A report goes to the lead through the log only: typed into its chat, every turn of every task would start a turn of the lead.
pub fn log_report(from: &str, report: &Report, text: &str) -> Result<()> {
    let id = format!("{}-{}-report", now(), std::process::id());
    append(&json!({ "id": id, "at": now(), "from": from, "to": LEAD_LABEL, "text": text, "state": "logged", "report": { "state": report.state, "next": report.next } }))
}

// The end of a task's life, so a later task by the same name does not inherit its reports.
pub fn log_end(task: &str, repo: &str, how: &str) -> Result<()> {
    let id = format!("{}-{}-end", now(), std::process::id());
    append(&json!({ "id": id, "at": now(), "from": "kelpie", "to": peers::address(task, repo), "text": format!("{task} ended ({how})"), "state": "ended" }))
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

// What sessions told each other, and the task reports that wait on the user.
fn shown_in_log(messages: Vec<Message>) -> Vec<Message> {
    messages.into_iter().filter(|message| message.report.as_ref().is_none_or(|report| report.state != "working")).collect()
}

fn addressee(message: &Message) -> String {
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

pub fn print(count: usize) -> String {
    let messages = shown_in_log(load());
    if messages.is_empty() {
        return "No messages yet.".to_string();
    }
    messages[messages.len().saturating_sub(count)..].iter().map(|message| {
        let held = if message.held { "  (held)" } else { "" };
        let next = message.next_for_you().map(|next| format!("\n      you: {next}")).unwrap_or_default();
        format!("{:>4}  {} → {}{held}\n      {}{next}", age(message.at), message.from, addressee(message), message.text)
    }).collect::<Vec<_>>().join("\n")
}

// The lead's copy: what sessions told each other since it last looked, and their latest reports, given to it with each of the user's prompts.
pub fn unread_for_lead() -> Result<String> {
    unread_in(&state_dir()?)
}

// How many of the messages the lead has seen, by the id of the last one; a mark that names no message in the log counts all as seen, so the lead is never flooded with history.
fn seen_count(messages: &[Message], mark: Option<&str>) -> usize {
    match mark {
        None | Some("") => 0,
        Some(id) => messages.iter().rposition(|message| message.id == id).map_or(messages.len(), |seen| seen + 1),
    }
}

fn unread_in(dir: &Path) -> Result<String> {
    let _lock = if holds(dir) { None } else { Lock::take_in(dir, true)? };
    let mark = dir.join(LEAD_MARK);
    let messages = load_in(dir);
    let seen = seen_count(&messages, fs::read_to_string(&mark).ok().as_deref().map(str::trim));
    fs::write(&mark, messages.last().map(|message| message.id.as_str()).unwrap_or_default()).map_err(|error| error.to_string())?;
    let copies = copies_for_lead(&messages, seen);
    let told: Vec<&Message> = copies.iter().copied().filter(|message| is_told(message)).collect();
    let mut sections = Vec::new();
    if !told.is_empty() {
        let lines: Vec<String> = told.iter().map(|message| {
            let held = if message.held { " (held)" } else { "" };
            format!("- {} → {}{held}: {}", message.from, message.to, message.text)
        }).collect();
        sections.push(format!("Messages between sessions since you last looked (kelpie messages --print for more):\n{}", lines.join("\n")));
    }
    let latest = latest_reports(&copies.into_iter().cloned().collect::<Vec<_>>());
    if !latest.is_empty() {
        let mut lines: Vec<String> = latest.values().map(report_line).collect();
        lines.sort();
        sections.push(format!("Task reports since you last looked (each task's latest; kelpie status --json has them all):\n{}", lines.join("\n")));
    }
    Ok(sections.join("\n\n"))
}

// What sessions said to each other, not a report or a task's end marker.
fn is_told(message: &Message) -> bool {
    message.report.is_none() && !message.ended
}

pub fn report_line(message: &Message) -> String {
    let state = message.report.as_ref().map(|report| report.state.as_str()).unwrap_or_default();
    format!("- {} ({state}): {} — you: {}", message.from, message.text, message.next_for_you().unwrap_or("nothing"))
}

// Messages to or from the lead are already in its chat; reports to it are not.
fn copies_for_lead(messages: &[Message], seen: usize) -> Vec<&Message> {
    messages.iter().skip(seen.min(messages.len()))
        .filter(|message| message.report.is_some() || (message.from != LEAD_LABEL && message.to != LEAD_LABEL)).collect()
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
    if message.held {
        head.push(Span::styled("  held", style(PEACH)));
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
mod tests {
    use super::*;

    fn session(pane: Option<&str>, status: Option<&str>) -> Session {
        Session { address: "billing@web".to_string(), task: Some("billing".to_string()), pane: pane.map(str::to_string), status: status.map(str::to_string) }
    }

    #[test]
    fn a_session_waiting_for_an_answer_or_without_claude_is_held() {
        assert_eq!(hold_reason(&session(Some("w1:p1"), Some("blocked"))), Some("is waiting for an answer"));
        assert_eq!(hold_reason(&session(None, None)), Some("has no Claude running"));
        for status in ["idle", "working", "done"] {
            assert_eq!(hold_reason(&session(Some("w1:p1"), Some(status))), None);
        }
    }

    #[test]
    fn the_log_folds_into_messages_with_their_latest_state() {
        let log = [
            r#"{"id":"1","at":10,"from":"billing@api","to":"billing@web","text":"person_id replaces user_id","state":"held"}"#,
            r#"{"id":"2","at":11,"from":"lead","to":"search@api","text":"rebase on main","state":"delivered"}"#,
            "not json",
            r#"{"id":"1","at":20,"state":"delivered"}"#,
            r#"{"id":"3","at":30,"from":"billing@web","to":"billing@api","text":"done","state":"held"}"#,
        ].join("\n");
        let messages = fold(&log);
        assert_eq!(messages.len(), 3);
        assert_eq!((messages[0].text.as_str(), messages[0].held, messages[0].at), ("person_id replaces user_id", false, 10));
        assert!(!messages[1].held);
        assert!(messages[2].held);
    }

    #[test]
    fn a_held_message_is_delivered_once_the_log_records_it() {
        let held = r#"{"id":"1","at":10,"from":"a@x","to":"b@x","text":"hi","state":"held"}"#;
        assert!(fold(held)[0].held);
        assert!(!fold(&format!("{held}\n{{\"id\":\"1\",\"at\":11,\"state\":\"delivered\"}}"))[0].held);
    }

    #[test]
    fn the_lead_is_copied_on_new_messages_between_others() {
        let message = |id: &str, from: &str, to: &str| Message { id: id.to_string(), at: 0, from: from.to_string(), to: to.to_string(), text: String::new(), held: false, report: None, ended: false, question: None, about: None, kind: None };
        let messages = [message("1", "a@x", "b@x"), message("2", "a@x", "lead"), message("3", "lead", "b@x"), message("4", "b@x", "a@x")];
        let ids = |seen| copies_for_lead(&messages, seen).iter().map(|message| message.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(0), ["1", "4"]);
        assert_eq!(ids(1), ["4"]);
        assert!(ids(9).is_empty());
    }

    #[test]
    fn a_report_folds_with_its_state_and_next_step() {
        let log = [
            r#"{"id":"1","at":10,"from":"billing@web","to":"lead","text":"payload renamed","state":"logged","report":{"state":"working","next":"nothing"}}"#,
            r#"{"id":"2","at":20,"from":"billing@web","to":"lead","text":"plan written","state":"logged","report":{"state":"needs-you","next":"approve the plan: keep the branch field?"}}"#,
            r#"{"id":"3","at":30,"from":"search@api","to":"lead","text":"rebase on main","state":"delivered"}"#,
        ].join("\n");
        let messages = fold(&log);
        assert_eq!(messages[1].report, Some(Report { state: "needs-you".to_string(), next: "approve the plan: keep the branch field?".to_string() }));
        assert!(!messages[1].held);
        assert_eq!(messages[0].next_for_you(), None);
        assert_eq!(messages[1].next_for_you(), Some("approve the plan: keep the branch field?"));
        assert_eq!(messages[2].report, None);
        let latest = latest_reports(&messages);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest["billing@web"].text, "plan written");
        let shown: Vec<String> = shown_in_log(messages).into_iter().map(|message| message.id).collect();
        assert_eq!(shown, ["2", "3"]);
    }

    #[test]
    fn the_lead_is_copied_on_reports_to_it() {
        let report = Some(Report { state: "ready".to_string(), next: "push and close".to_string() });
        let messages = [Message { id: "1".to_string(), at: 0, from: "a@x".to_string(), to: "lead".to_string(), text: "done".to_string(), held: false, report, ended: false, question: None, about: None, kind: None }];
        assert_eq!(copies_for_lead(&messages, 0).len(), 1);
        assert_eq!(report_line(&messages[0]), "- a@x (ready): done — you: push and close");
    }

    #[test]
    fn a_report_counts_only_until_its_task_ends() {
        let report = |id: &str, text: &str| format!(r#"{{"id":"{id}","at":10,"from":"billing@web","to":"lead","text":"{text}","state":"logged","report":{{"state":"needs-you","next":"approve the plan"}}}}"#);
        let end = r#"{"id":"e","at":20,"from":"kelpie","to":"billing@web","text":"billing ended (done)","state":"ended"}"#;
        let other = r#"{"id":"o","at":15,"from":"search@api","to":"lead","text":"rebased","state":"logged","report":{"state":"ready","next":"push"}}"#;
        let ended = fold(&[report("1", "old plan"), other.to_string(), end.to_string()].join("\n"));
        let latest = latest_reports(&ended);
        assert!(!latest.contains_key("billing@web"));
        assert_eq!(latest["search@api"].text, "rebased");
        let reused = fold(&[report("1", "old plan"), end.to_string(), report("2", "new plan")].join("\n"));
        assert_eq!(latest_reports(&reused)["billing@web"].text, "new plan");
    }

    #[test]
    fn an_end_marker_is_not_told_to_the_lead_but_shows_in_messages() {
        let end = fold(r#"{"id":"e","at":20,"from":"kelpie","to":"billing@web","text":"billing ended (dropped)","state":"ended"}"#);
        assert!(end[0].ended && end[0].report.is_none());
        assert!(!copies_for_lead(&end, 0).into_iter().any(is_told));
        assert_eq!(shown_in_log(end).len(), 1);
    }

    #[test]
    fn a_question_folds_with_its_options_and_its_place_in_the_queue() {
        let question = r#"{"id":"q1","at":10,"from":"billing@web","to":"lead","text":"Which queue?","state":"asked","question":{"options":[{"label":"A","meaning":"one"},{"label":"B","meaning":"two"}],"recommend":1,"plan":null}}"#;
        let asked = fold(question);
        assert_eq!(asked[0].question, Some(Question { options: vec![("A".to_string(), "one".to_string()), ("B".to_string(), "two".to_string())], recommend: 1, plan: None, given: false, answered: false, waits: false }));
        assert_eq!(addressee(&asked[0]), "you · asks");
        let given = fold(&format!("{question}\n{{\"id\":\"q1\",\"at\":11,\"state\":\"given\"}}"));
        assert!(given[0].question.as_ref().unwrap().given && !given[0].held);
        let answered = fold(&format!("{question}\n{{\"id\":\"q1\",\"at\":11,\"state\":\"given\"}}\n{{\"id\":\"q1\",\"at\":12,\"state\":\"answered\"}}"));
        assert!(answered[0].question.as_ref().unwrap().answered);
    }

    #[test]
    fn a_held_question_for_the_lead_is_dropped_once_it_is_no_longer_open() {
        let question = fold(r#"{"id":"q1","at":10,"from":"billing@web","to":"lead","text":"Which queue?","state":"asked","question":{"options":[],"recommend":1}}"#);
        let relay = fold(r#"{"id":"m1","at":11,"from":"kelpie","to":"lead","text":"billing@web asks the user: Which queue?","state":"held","about":"q1"}"#);
        assert!(!stale(&relay[0], &[&question[0]]));
        assert!(stale(&relay[0], &[]));
        let plain = fold(r#"{"id":"m2","at":11,"from":"kelpie","to":"lead","text":"rebased","state":"held"}"#);
        assert!(!stale(&plain[0], &[]));
    }

    fn state(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-messages-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn told(id: &str, at: u64, from: &str, to: &str, state: &str) -> Value {
        json!({ "id": id, "at": at, "from": from, "to": to, "text": format!("text of {id}"), "state": state })
    }

    fn report(id: &str, at: u64, from: &str, text: &str) -> Value {
        json!({ "id": id, "at": at, "from": from, "to": LEAD_LABEL, "text": text, "state": "logged", "report": { "state": "needs-you", "next": "look" } })
    }

    fn question(id: &str, at: u64, from: &str) -> Value {
        json!({ "id": id, "at": at, "from": from, "to": LEAD_LABEL, "text": format!("question {id}"), "state": "asked", "question": { "options": [], "recommend": 1 } })
    }

    fn mark(id: &str, at: u64, state: &str) -> Value {
        json!({ "id": id, "at": at, "state": state })
    }

    fn write_log(dir: &Path, records: &[Value]) {
        fs::write(log_in(dir), records.iter().map(|record| format!("{record}\n")).collect::<String>()).unwrap();
    }

    fn ids(messages: &[Message]) -> Vec<String> {
        messages.iter().map(|message| message.id.clone()).collect()
    }

    #[test]
    fn an_unchanged_log_is_not_read_again_and_a_changed_one_is() {
        let dir = state("cache");
        write_log(&dir, &[told("a", 1, "x@r", "y@r", "delivered")]);
        assert_eq!(ids(&load_in(&dir)), ["a"]);
        let modified = fs::metadata(log_in(&dir)).unwrap().modified().unwrap();
        write_log(&dir, &[told("b", 1, "x@r", "y@r", "delivered")]);
        File::options().write(true).open(log_in(&dir)).unwrap().set_modified(modified).unwrap();
        assert_eq!(ids(&load_in(&dir)), ["a"], "same inode, length and mtime is the folded copy");
        append_in(&dir, &told("c", 2, "x@r", "y@r", "delivered"), ROTATE_BYTES).unwrap();
        assert_eq!(ids(&load_in(&dir)), ["b", "c"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rotation_keeps_what_is_live_and_drops_the_rest() {
        let dir = state("rotate");
        let records = [
            told("delivered", 1, "a@r", "b@r", "delivered"),
            told("held", 2, "a@r", "b@r", "held"),
            report("old-report", 3, "billing@web", "old plan"),
            question("answered", 4, "search@api"),
            mark("answered", 5, "answered"),
            question("open", 6, "billing@web"),
            mark("open", 7, "given"),
            report("new-report", 8, "billing@web", "new plan"),
            report("ended-report", 9, "gone@api", "finished"),
            json!({ "id": "end", "at": 10, "from": "kelpie", "to": "gone@api", "text": "gone ended (done)", "state": "ended" }),
            json!({ "id": "push-before", "at": 11, "from": "kelpie", "to": LEAD_LABEL, "text": "pushed earlier", "state": "delivered", "kind": "push" }),
            json!({ "id": "push-away", "at": 21, "from": "kelpie", "to": LEAD_LABEL, "text": "pushed while away", "state": "delivered", "kind": "push" }),
            mark("held", 22, "held"),
        ];
        write_log(&dir, &records);
        let before = load_in(&dir);
        rotate_in(&dir, Some(20)).unwrap();
        let after = load_in(&dir);
        assert_eq!(ids(&after), ["held", "open", "new-report", "end", "push-away"]);
        assert!(after[0].held);
        assert!(after[1].question.as_ref().unwrap().given);
        assert_eq!(fold(&fs::read_to_string(dir.join("messages.jsonl.1")).unwrap()), before);
        let open = |messages: &[Message]| ids(&questions::open(messages, |_| None).into_iter().cloned().collect::<Vec<_>>());
        assert_eq!(open(&after), open(&before));
        assert_eq!(latest_reports(&after), latest_reports(&before));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_lead_neither_misses_nor_sees_again_a_message_across_a_rotation() {
        let dir = state("lead-mark");
        write_log(&dir, &[told("seen-1", 1, "a@r", "b@r", "delivered"), told("seen-held", 2, "a@r", "b@r", "held"), told("seen-2", 3, "b@r", "a@r", "delivered")]);
        let first = unread_in(&dir).unwrap();
        assert!(first.contains("text of seen-1") && first.contains("text of seen-held") && first.contains("text of seen-2"));
        append_in(&dir, &told("unseen-1", 4, "a@r", "b@r", "delivered"), ROTATE_BYTES).unwrap();
        append_in(&dir, &told("unseen-2", 5, "b@r", "a@r", "delivered"), 0).unwrap();
        assert!(fs::exists(dir.join("messages.jsonl.1")).unwrap(), "an append past the size rotates first");
        assert!(!ids(&load_in(&dir)).contains(&"seen-1".to_string()));
        let next = unread_in(&dir).unwrap();
        assert!(next.contains("text of unseen-1") && next.contains("text of unseen-2"), "{next}");
        assert!(!next.contains("seen-held") && !next.contains("text of seen-"), "{next}");
        assert_eq!(unread_in(&dir).unwrap(), "");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_mark_naming_no_message_counts_everything_as_seen() {
        let messages = fold(&[told("1", 1, "a@r", "b@r", "delivered"), told("2", 2, "a@r", "b@r", "delivered")].map(|record| record.to_string()).join("\n"));
        assert_eq!(seen_count(&messages, None), 0);
        assert_eq!(seen_count(&messages, Some("")), 0);
        assert_eq!(seen_count(&messages, Some("1")), 1);
        assert_eq!(seen_count(&messages, Some("17")), 2);
    }

    #[test]
    fn an_append_under_the_threads_own_lock_does_not_wait_for_it() {
        let dir = state("reentrant");
        let _lock = Lock::take_in(&dir, false).unwrap().unwrap();
        append_in(&dir, &told("a", 1, "a@r", "b@r", "held"), ROTATE_BYTES).unwrap();
        assert_eq!(ids(&load_in(&dir)), ["a"]);
        drop(_lock);
        assert!(!holds(&dir));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn concurrent_appenders_lose_nothing_live_while_the_log_rotates() {
        let dir = state("concurrent");
        let writers: Vec<_> = (0..4).map(|writer| {
            let dir = dir.clone();
            std::thread::spawn(move || {
                for n in 0..20 {
                    append_in(&dir, &told(&format!("{writer}-{n}"), 1, "a@r", "b@r", "held"), 600).unwrap();
                }
            })
        }).collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert!(fs::exists(dir.join("messages.jsonl.1")).unwrap());
        let mut kept = ids(&load_in(&dir));
        kept.sort();
        let mut all: Vec<String> = (0..4).flat_map(|writer| (0..20).map(move |n| format!("{writer}-{n}"))).collect();
        all.sort();
        assert_eq!(kept, all);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wrap_breaks_on_words_and_keeps_paragraphs() {
        assert_eq!(wrap("one two three four\nfive", 9), ["one two", "three", "four", "five"]);
    }
}
