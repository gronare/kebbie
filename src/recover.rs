use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::messages;
use crate::notify::{self, Event as NotifyEvent, Notice};
use crate::sh::Result;
use crate::usage::Usage;

pub const DELAYS: [u64; 4] = [60, 120, 240, 480];
pub const RESUMES: usize = DELAYS.len();
pub const MARGIN: u64 = 60;
const TAIL_BYTES: u64 = 512 * 1024;

const FATAL: [(&str, &str); 8] = [
    ("authentication_failed", "a login problem"),
    ("oauth_org_not_allowed", "a login problem"),
    ("account_on_hold", "an account on hold"),
    ("billing_error", "a billing problem"),
    ("invalid_request", "an invalid request"),
    ("model_not_found", "a model that is not available"),
    ("verification_required", "a verification the account needs"),
    ("cloud_credential_error", "missing cloud credentials"),
];
const NOT_QUOTA: [&str; 4] = ["temporarily limiting requests", "not your usage limit", "high load", "request rejected (429)"];
const QUOTA: [&str; 4] = ["you've hit your", "you've reached your", "you're out of usage", "usage limit"];
const PASSING: [&str; 9] = ["overloaded", "at capacity", "timed out", "timeout", "no response", "connection", "econn", "socket", "server error"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Window {
    Session,
    Week,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clock {
    pub year: Option<i64>,
    pub month: Option<u32>,
    pub day: Option<u32>,
    pub weekday: Option<i64>,
    pub hour: u32,
    pub minute: u32,
    pub zone: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ApiError {
    Transient { status: Option<u64>, reason: &'static str, text: String },
    Quota { clock: Option<Clock>, window: Option<Window>, text: String },
    Fatal { kind: &'static str, text: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub id: String,
    pub at: u64,
    pub error: ApiError,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tail {
    pub failure: Option<Failure>,
    pub prompted: bool,
    pub replied: bool,
}

fn text_of(record: &Value) -> String {
    match &record["message"]["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn reason(status: Option<u64>, lower: &str) -> &'static str {
    if status == Some(529) || ["overloaded", "at capacity", "high load"].iter().any(|word| lower.contains(word)) {
        "overload"
    } else if status == Some(429) || ["limiting requests", "rate limit", "(429)"].iter().any(|word| lower.contains(word)) {
        "a rate limit"
    } else if status == Some(408) || ["timed out", "timeout", "no response"].iter().any(|word| lower.contains(word)) {
        "a timeout"
    } else if ["connection", "econn", "socket", "network"].iter().any(|word| lower.contains(word)) {
        "a dropped connection"
    } else {
        "a server error"
    }
}

fn quota(text: &str) -> ApiError {
    let lower = text.to_lowercase();
    let window = if lower.contains("session limit") { Some(Window::Session) } else if lower.contains("weekly limit") { Some(Window::Week) } else { None };
    ApiError::Quota { clock: reset_clock(text), window, text: text.to_string() }
}

// What Claude Code wrote for a turn the API ended: `isApiErrorMessage`, its own `error` code and the HTTP `apiErrorStatus`; the words decide only within rate_limit and for codes it does not name.
pub fn classify(record: &Value) -> Option<ApiError> {
    if record["isApiErrorMessage"] != true {
        return None;
    }
    let text = text_of(record);
    let lower = text.to_lowercase();
    let status = record["apiErrorStatus"].as_u64();
    let code = record["error"].as_str().unwrap_or("unknown");
    let transient = |text: String| ApiError::Transient { status, reason: reason(status, &lower), text };
    if record["message"]["stop_reason"] == "refusal" {
        return Some(ApiError::Fatal { kind: "a refusal", text });
    }
    if let Some((_, kind)) = FATAL.iter().find(|(known, _)| *known == code) {
        return Some(ApiError::Fatal { kind, text });
    }
    let is_quota = !NOT_QUOTA.iter().any(|words| lower.contains(words)) && QUOTA.iter().any(|words| lower.contains(words));
    Some(match code {
        "max_output_tokens" => return None,
        "rate_limit" if is_quota => quota(&text),
        "rate_limit" | "server_error" | "overloaded" => transient(text),
        _ if is_quota && status == Some(429) => quota(&text),
        _ if status.is_some_and(|status| status >= 500 || status == 408 || status == 429) || PASSING.iter().any(|word| lower.contains(word)) => transient(text),
        _ => ApiError::Fatal { kind: "an API error kebbie does not know", text },
    })
}

fn prompt(record: &Value) -> bool {
    match record["type"].as_str() {
        Some("user") if record["isMeta"] != true => match &record["message"]["content"] {
            Value::String(text) => !text.trim().is_empty(),
            Value::Array(parts) => parts.iter().any(|part| part["type"] == "text"),
            _ => false,
        },
        Some("attachment") => record["attachment"]["type"] == "queued_command",
        _ => false,
    }
}

pub fn observe(transcript: &str) -> Tail {
    let mut error: Option<Value> = None;
    let mut tail = Tail::default();
    for record in transcript.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).filter(|record| record["isSidechain"] != true) {
        if record["type"] == "assistant" {
            if record["isApiErrorMessage"] == true {
                error = Some(record);
                tail.prompted = false;
                tail.replied = false;
            } else {
                error = None;
                tail.replied = true;
            }
        } else if prompt(&record) {
            tail.prompted = true;
        }
    }
    tail.failure = error.filter(|_| !tail.prompted).and_then(|record| {
        let at = record["timestamp"].as_str().and_then(crate::typing::iso_millis)? / 1000;
        Some(Failure { id: record["uuid"].as_str().unwrap_or_default().to_string(), at, error: classify(&record)? })
    });
    tail
}

fn month_of(word: &str) -> Option<u32> {
    ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|month| word.starts_with(month)).map(|index| index as u32 + 1)
}

fn weekday_of(word: &str) -> Option<i64> {
    ["sun", "mon", "tue", "wed", "thu", "fri", "sat"].iter().position(|day| word.starts_with(day)).map(|index| index as i64)
}

fn time_of(word: &str, half: Option<&str>) -> Option<(u32, u32)> {
    let (clock, half) = match half {
        Some(half) => (word, Some(half)),
        None if word.ends_with("am") || word.ends_with("pm") => (&word[..word.len() - 2], Some(&word[word.len() - 2..])),
        None => (word, None),
    };
    let (hour, minute) = clock.split_once(':').map_or((clock, "0"), |(hour, minute)| (hour, minute));
    let (hour, minute): (u32, u32) = (hour.parse().ok()?, minute.parse().ok()?);
    if minute > 59 {
        return None;
    }
    match half {
        Some(half) if (1..=12).contains(&hour) => Some((hour % 12 + if half == "pm" { 12 } else { 0 }, minute)),
        None if hour <= 23 && clock.contains(':') => Some((hour, minute)),
        _ => None,
    }
}

// The reset in Claude Code's words: "resets 5pm (Europe/Stockholm)", "resets 3:45pm", "resets Mon 12:00am", "resets Oct 23, 5pm", "resets Jan 2, 2027, 9:30am (UTC)".
pub fn reset_clock(text: &str) -> Option<Clock> {
    let start = text.find("resets ")? + "resets ".len();
    let rest = &text[start..];
    let rest = rest.split(['·', '\n']).next().unwrap_or(rest).trim();
    let (rest, zone) = match (rest.find('('), rest.find(')')) {
        (Some(open), Some(close)) if open < close => (&rest[..open], Some(rest[open + 1..close].trim().to_string()).filter(|zone| !zone.is_empty())),
        _ => (rest, None),
    };
    let words: Vec<String> = rest.to_lowercase().replace([',', '\u{202f}', '\u{a0}'], " ").split_whitespace().map(str::to_string).collect();
    let mut clock = Clock { zone, ..Clock::default() };
    let mut time = None;
    for (index, word) in words.iter().enumerate() {
        let next = words.get(index + 1).map(String::as_str).filter(|next| *next == "am" || *next == "pm");
        if word == "am" || word == "pm" {
            continue;
        }
        if let Some(month) = month_of(word).filter(|_| word.chars().all(char::is_alphabetic)) {
            clock.month = Some(month);
        } else if let Some(weekday) = weekday_of(word).filter(|_| word.chars().all(char::is_alphabetic)) {
            clock.weekday = Some(weekday);
        } else if word.len() == 4 && word.chars().all(|character| character.is_ascii_digit()) {
            clock.year = word.parse().ok();
        } else if let Some(found) = time_of(word, next).filter(|_| next.is_some() || word.contains(':') || word.ends_with('m')) {
            time = Some(found);
        } else if word.chars().all(|character| character.is_ascii_digit()) {
            clock.day = word.parse().ok().filter(|day| (1..=31).contains(day));
        } else {
            return None;
        }
    }
    (clock.hour, clock.minute) = time?;
    if clock.month.is_some() != clock.day.is_some() {
        return None;
    }
    Some(clock)
}

pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let (y, m) = if month <= 2 { (year - 1, i64::from(month) + 9) } else { (year, i64::from(month) - 3) };
    let era = y.div_euclid(400);
    let of_era = y - era * 400;
    let of_year = (153 * m + 2) / 5 + i64::from(day) - 1;
    era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468
}

pub(crate) fn year_of(days: i64) -> i64 {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    year_of_era + era * 400 + i64::from((5 * of_year + 2) / 153 >= 10)
}

pub(crate) fn weekday(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

pub fn resolve(clock: &Clock, base: u64, offset: &dyn Fn(i64) -> i64) -> Option<u64> {
    let base = i64::try_from(base).ok()?;
    let today = (base + offset(base)).div_euclid(86_400);
    let at_day = |day: i64| {
        let local = day * 86_400 + i64::from(clock.hour) * 3600 + i64::from(clock.minute) * 60;
        local - offset(local - offset(local))
    };
    let at = match (clock.month, clock.day) {
        (Some(month), Some(day)) => {
            let year = clock.year.unwrap_or_else(|| year_of(today));
            let at = at_day(days_from_civil(year, month, day));
            if clock.year.is_none() && at + 86_400 < base { at_day(days_from_civil(year + 1, month, day)) } else { at }
        }
        _ => {
            let (first, step) = match clock.weekday {
                Some(wanted) => (today + (wanted - weekday(today)).rem_euclid(7), 7),
                None => (today, 1),
            };
            let at = at_day(first);
            if at <= base { at_day(first + step) } else { at }
        }
    };
    u64::try_from(at).ok()
}

pub fn local_offset(at: i64) -> i64 {
    let time = at as libc::time_t;
    // SAFETY: an all-zero tm is a valid value, and localtime_r writes only into the one it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to locals that outlive the call.
    let found = unsafe { !libc::localtime_r(&time, &mut tm).is_null() };
    if found { tm.tm_gmtoff } else { 0 }
}

pub fn local_zone() -> Option<String> {
    if let Ok(zone) = std::env::var("TZ") {
        let zone = zone.trim_start_matches(':');
        if !zone.is_empty() {
            return Some(zone.to_string());
        }
    }
    let link = fs::read_link("/etc/localtime").ok()?;
    link.to_string_lossy().split_once("zoneinfo/").map(|(_, zone)| zone.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zone {
    Local,
    Utc,
}

// Claude Code names the zone it wrote the time in; kebbie reads it only as this machine's own zone or UTC.
pub fn zone_of(named: Option<&str>, local: Option<&str>) -> Option<Zone> {
    match named {
        None => Some(Zone::Local),
        Some("UTC" | "Etc/UTC" | "GMT" | "Etc/GMT" | "Z") => Some(Zone::Utc),
        Some(named) if Some(named) == local => Some(Zone::Local),
        Some(_) => None,
    }
}

pub fn reset_at(clock: Option<&Clock>, window: Option<Window>, base: u64, usage: Option<&Usage>, local: Option<&str>, offset: &dyn Fn(i64) -> i64) -> Option<u64> {
    let parsed = clock.and_then(|clock| match zone_of(clock.zone.as_deref(), local)? {
        Zone::Local => resolve(clock, base, offset),
        Zone::Utc => resolve(clock, base, &|_| 0),
    });
    parsed.or_else(|| {
        let usage = usage?;
        match window {
            Some(Window::Session) => usage.session.as_ref().map(|window| window.resets_at),
            Some(Window::Week) => usage.week.as_ref().map(|window| window.resets_at),
            None => [&usage.session, &usage.week].into_iter().flatten().filter(|window| window.used >= 100.0).map(|window| window.resets_at).max(),
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Retrying,
    Allowance,
    Stuck,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Retrying => "retrying",
            Kind::Allowance => "allowance",
            Kind::Stuck => "stuck",
        }
    }

    fn from_key(key: &str) -> Option<Kind> {
        [Kind::Retrying, Kind::Allowance, Kind::Stuck].into_iter().find(|kind| kind.key() == key)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub address: String,
    pub kind: Kind,
    pub reason: String,
    pub text: String,
    pub error_id: String,
    pub error_at: u64,
    pub attempts: usize,
    pub next_at: Option<u64>,
    pub reset_at: Option<u64>,
    pub sent_for: Option<String>,
}

fn clock_text(at: u64, now: u64, offset: &dyn Fn(i64) -> i64) -> String {
    let local = at as i64 + offset(at as i64);
    let seconds = local.rem_euclid(86_400);
    let time = format!("{:02}:{:02}", seconds / 3600, seconds % 3600 / 60);
    if at > now + 20 * 3600 {
        let day = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][weekday(local.div_euclid(86_400)) as usize];
        format!("{day} {time}")
    } else {
        time
    }
}

impl Record {
    pub fn sent(&self) -> bool {
        self.sent_for.as_deref() == Some(self.error_id.as_str())
    }

    pub fn note_at(&self, now: u64, offset: &dyn Fn(i64) -> i64) -> String {
        let when = |at: Option<u64>| at.map_or("soon".to_string(), |at| clock_text(at, now, offset));
        match self.kind {
            Kind::Retrying if self.sent() => format!("resumed after {} ({}/{RESUMES})", self.reason, self.attempts),
            Kind::Retrying => format!("retrying after {} ({}/{RESUMES}, next {})", self.reason, self.attempts + 1, when(self.next_at)),
            Kind::Allowance if self.sent() => "resumed after the allowance reset".to_string(),
            Kind::Allowance => format!("out of allowance until {}", when(self.reset_at)),
            Kind::Stuck => format!("stopped by {}: {}", self.reason, self.text.split_whitespace().collect::<Vec<_>>().join(" ")),
        }
    }

    pub fn note(&self) -> String {
        self.note_at(messages::now(), &local_offset)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "address": self.address, "kind": self.kind.key(), "reason": self.reason, "text": self.text, "error_id": self.error_id, "error_at": self.error_at,
            "attempts": self.attempts, "next_at": self.next_at, "reset_at": self.reset_at, "sent_for": self.sent_for,
        })
    }

    pub fn from_json(value: &Value) -> Option<Record> {
        let text = |key: &str| value[key].as_str().map(str::to_string);
        Some(Record {
            address: text("address")?, kind: Kind::from_key(value["kind"].as_str()?)?, reason: text("reason").unwrap_or_default(), text: text("text").unwrap_or_default(),
            error_id: text("error_id")?, error_at: value["error_at"].as_u64()?, attempts: value["attempts"].as_u64().unwrap_or(0) as usize,
            next_at: value["next_at"].as_u64(), reset_at: value["reset_at"].as_u64(), sent_for: text("sent_for"),
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum Act {
    Nothing,
    Forget,
    Keep(Record),
    Send(Record),
    Stop(Record),
}

// One round for an idle task: each API error is acted on once, a reply after it ends the episode, and four resumes without one stop it.
pub fn decide(address: &str, old: Option<&Record>, tail: &Tail, now: u64, reset: Option<u64>) -> Act {
    let Some(failure) = &tail.failure else {
        return match old {
            Some(record) if tail.replied || (record.kind == Kind::Stuck && tail.prompted) => Act::Forget,
            _ => Act::Nothing,
        };
    };
    if old.is_some_and(|record| record.error_id == failure.id && (record.sent() || record.kind == Kind::Stuck)) {
        return Act::Nothing;
    }
    let attempts = old.filter(|record| record.kind != Kind::Stuck).map_or(0, |record| record.attempts);
    let record = |kind: Kind, reason: String, text: &str, next_at: Option<u64>, reset_at: Option<u64>| Record {
        address: address.to_string(), kind, reason, text: text.to_string(), error_id: failure.id.clone(), error_at: failure.at, attempts, next_at, reset_at, sent_for: None,
    };
    let scheduled = |made: Record| if made.next_at.is_some_and(|at| now >= at) { Act::Send(made) } else { Act::Keep(made) };
    match &failure.error {
        ApiError::Fatal { kind, text } => Act::Stop(record(Kind::Stuck, kind.to_string(), text, None, None)),
        ApiError::Transient { reason, text, .. } if attempts >= RESUMES => Act::Stop(record(Kind::Stuck, format!("{reason}, still after {RESUMES} retries"), text, None, None)),
        ApiError::Transient { reason, text, .. } => scheduled(record(Kind::Retrying, reason.to_string(), text, Some(failure.at + DELAYS[attempts]), None)),
        ApiError::Quota { text, .. } if attempts >= RESUMES => Act::Stop(record(Kind::Stuck, format!("the usage limit, still after {RESUMES} resumes"), text, None, None)),
        ApiError::Quota { text, .. } => match reset {
            None => Act::Stop(record(Kind::Stuck, "the usage limit, with no reset time kebbie can read".to_string(), text, None, None)),
            Some(reset) => scheduled(record(Kind::Allowance, "the usage limit".to_string(), text, Some(reset.max(failure.at) + MARGIN), Some(reset))),
        },
    }
}

pub fn resume_text(record: &Record) -> String {
    match record.kind {
        Kind::Allowance => "Your plan's usage allowance has reset. Carry on where you left off.".to_string(),
        _ => format!("Your last turn was cut off by an API error ({}); it has passed. Carry on where you left off.", record.reason),
    }
}

fn dir(state: &Path) -> PathBuf {
    state.join("recover")
}

fn file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.json"))
}

pub fn save(state: &Path, record: &Record) -> Result<()> {
    crate::state::write(&file(state, &record.address), serde_json::to_string_pretty(&record.to_json()).unwrap_or_default())
}

pub fn load(state: &Path, address: &str) -> Option<Record> {
    Record::from_json(&serde_json::from_str(&fs::read_to_string(file(state, address)).ok()?).ok()?)
}

pub fn forget(state: &Path, address: &str) {
    let _ = fs::remove_file(file(state, address));
}

fn addresses(state: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir(state)) else { return Vec::new() };
    entries.filter_map(|entry| {
        let name = entry.ok()?.file_name().to_string_lossy().to_string();
        name.strip_suffix(".json").filter(|_| !name.starts_with('.')).map(str::to_string)
    }).collect()
}

pub fn enabled() -> bool {
    crate::config::recover_enabled().unwrap_or(true)
}

pub fn tail_of(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    Some(if start == 0 { text } else { text.split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string()) })
}

pub struct Task {
    pub address: String,
    pub repo: PathBuf,
    pub pane: String,
    pub idle: bool,
    pub transcript: Option<PathBuf>,
}

// Only the Claudes kebbie started in its pooled worktrees: never the lead, never the user's own sessions.
fn tasks() -> Result<(Vec<String>, Vec<Task>)> {
    let workspaces = crate::herdr::workspaces()?;
    let agents: Vec<Value> = crate::herdr::agents()?.into_iter().filter(|agent| agent["agent"] == crate::agent::Agent::Claude.kind()).collect();
    let repos: BTreeSet<PathBuf> = workspaces.iter().filter_map(|workspace| workspace["worktree"]["repo_root"].as_str()).map(PathBuf::from).collect();
    let home = crate::agent::claude::home();
    let mut found = Vec::new();
    let mut all = Vec::new();
    for repo in &repos {
        for slot in crate::tasks::slots(repo)? {
            let Some(task) = slot.task else { continue };
            let address = crate::peers::address(&task, &crate::repo::name_of(repo));
            all.push(address.clone());
            let Some(workspace) = workspaces.iter().find(|workspace| workspace["worktree"]["checkout_path"].as_str() == slot.path.to_str()).and_then(|workspace| workspace["workspace_id"].as_str()) else { continue };
            let Some(pane) = agents.iter().find(|agent| agent["workspace_id"].as_str() == Some(workspace)).and_then(crate::restart::Pane::from_agent) else { continue };
            let transcript = pane.session.as_deref().zip(home.as_deref()).and_then(|(session, home)| crate::restart::transcript(home, pane.cwd.as_deref(), session));
            found.push(Task { address, repo: repo.clone(), pane: pane.pane, idle: pane.waiting, transcript });
        }
    }
    Ok((all, found))
}

// Held while a round runs, so a board and a keeper never resume the same task twice.
struct Lock {
    _file: fs::File,
}

impl Drop for Lock {
    // A child forked by another thread holds a copy of the descriptor until it execs; unlocking releases the lock for every copy at once.
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
    }
}

impl Lock {
    fn take(state: &Path) -> Option<Lock> {
        let file = crate::state::append_to(&dir(state).join(".lock")).ok()?;
        // SAFETY: flock on a descriptor this File owns.
        (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(Lock { _file: file })
    }
}

pub fn tick(state: &Path) {
    if !enabled() {
        return;
    }
    let Some(_lock) = Lock::take(state) else { return };
    let Ok((all, tasks)) = tasks() else { return };
    let now = messages::now();
    let usage = crate::usage::load_in(state, now);
    let local = local_zone();
    for address in addresses(state).iter().filter(|address| !all.contains(address)) {
        forget(state, address);
    }
    for task in tasks.iter().filter(|task| task.idle) {
        let Some(tail) = task.transcript.as_deref().and_then(tail_of).map(|text| observe(&text)) else { continue };
        let old = load(state, &task.address);
        let reset = tail.failure.as_ref().and_then(|failure| match &failure.error {
            ApiError::Quota { clock, window, .. } => reset_at(clock.as_ref(), *window, failure.at, usage.as_ref(), local.as_deref(), &local_offset),
            _ => None,
        });
        apply(state, task, old.as_ref(), decide(&task.address, old.as_ref(), &tail, now, reset));
    }
}

fn apply(state: &Path, task: &Task, old: Option<&Record>, act: Act) {
    match act {
        Act::Nothing => {}
        Act::Forget => forget(state, &task.address),
        Act::Keep(record) => {
            if old != Some(&record) {
                let _ = save(state, &record);
            }
        }
        Act::Send(mut record) => {
            let text = resume_text(&record);
            if let Ok(via) = crate::inbox::prompt(&task.pane, &messages::delivered("kebbie", &text), &task.address) {
                record.attempts += 1;
                record.sent_for = Some(record.error_id.clone());
                if let Ok(_lock) = messages::Lock::take(true) {
                    let _ = messages::append(&json!({ "id": messages::new_id("recover"), "at": messages::now(), "from": "kebbie", "to": task.address, "text": text, "state": "delivered", "kind": "recover", "via": via.word() }));
                }
            }
            if old != Some(&record) {
                let _ = save(state, &record);
            }
        }
        Act::Stop(record) => {
            let _ = save(state, &record);
            crate::phase::record(state, &record.address, "recover", &crate::phase::Event::Stopped);
            let line = stuck_line(&record);
            if let Err(error) = messages::tell_lead(&line, Some("recover")) {
                eprintln!("Could not tell the lead that {} stopped: {error}", record.address);
            }
            notify::send(&task.repo, &Notice::from(NotifyEvent::ApiError, &record.address, format!("{}: {}", record.address, record.note())));
        }
    }
}

pub fn stuck_line(record: &Record) -> String {
    format!("{} stopped by {} and waits under Needs you; kebbie does not resume it. Claude Code said: {}", record.address, record.reason, record.text.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::Window as Used;

    const AT: &str = "2026-10-20T19:00:00.000Z";
    const AT_S: u64 = 1_792_522_800;

    fn error(code: &str, status: u64, text: &str) -> Value {
        json!({
            "type": "assistant", "uuid": format!("e-{code}-{status}"), "timestamp": AT, "isSidechain": false, "isApiErrorMessage": true, "error": code, "apiErrorStatus": status,
            "message": { "model": "<synthetic>", "role": "assistant", "type": "message", "stop_reason": "stop_sequence", "content": [{ "type": "text", "text": text }] },
        })
    }

    fn user(text: &str, at: &str) -> Value {
        json!({ "type": "user", "uuid": "u", "timestamp": at, "isSidechain": false, "message": { "role": "user", "content": text } })
    }

    fn reply(at: &str) -> Value {
        json!({ "type": "assistant", "uuid": "a", "timestamp": at, "isSidechain": false, "message": { "model": "claude-opus-5-5", "stop_reason": "end_turn", "content": [{ "type": "text", "text": "Done." }] } })
    }

    fn lines(records: &[Value]) -> String {
        records.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn each_kind_of_api_error_is_told_by_claude_codes_own_code_status_and_words() {
        let kind = |record: Value| classify(&record);
        assert!(matches!(kind(error("server_error", 529, "API Error: Repeated 529 Overloaded errors. The API is at capacity — this is usually temporary. Try again in a moment.")), Some(ApiError::Transient { status: Some(529), reason: "overload", .. })));
        assert!(matches!(kind(error("server_error", 500, "API Error: 500 Internal server error. This is a server-side issue, usually temporary — try again in a moment.")), Some(ApiError::Transient { reason: "a server error", .. })));
        assert!(matches!(kind(error("rate_limit", 429, "API Error: Server is temporarily limiting requests (not your usage limit) · Rate limited")), Some(ApiError::Transient { reason: "a rate limit", .. })), "a throttle is no usage limit, though it names one");
        assert!(matches!(kind(error("rate_limit", 429, "Opus is experiencing high load, please use /model to switch to Sonnet")), Some(ApiError::Transient { reason: "overload", .. })));
        assert!(matches!(kind(error("server_error", 0, "API Error: Connection to the API was lost (ECONNRESET). This is usually temporary — try again.")), Some(ApiError::Transient { reason: "a dropped connection", .. })));
        assert!(matches!(kind(error("server_error", 0, "Request timed out")), Some(ApiError::Transient { reason: "a timeout", .. })));
        assert!(matches!(kind(error("unknown", 529, "API Error: 529 {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}")), Some(ApiError::Transient { reason: "overload", .. })));
        assert!(matches!(kind(error("rate_limit", 429, "You've hit your session limit · resets 5pm (UTC)")), Some(ApiError::Quota { window: Some(Window::Session), clock: Some(_), .. })));
        assert!(matches!(kind(error("rate_limit", 429, "You've hit your weekly limit · resets Oct 23, 9am · progress saved")), Some(ApiError::Quota { window: Some(Window::Week), .. })));
        assert!(matches!(kind(error("rate_limit", 429, "You've reached your Fable limit. Run /usage-credits to continue or switch models with /model.")), Some(ApiError::Quota { window: None, clock: None, .. })));
        for (code, status, expected) in [("authentication_failed", 401, "a login problem"), ("oauth_org_not_allowed", 403, "a login problem"), ("billing_error", 400, "a billing problem"), ("invalid_request", 400, "an invalid request"), ("model_not_found", 404, "a model that is not available"), ("unknown", 418, "an API error kebbie does not know")] {
            assert!(matches!(kind(error(code, status, "words")), Some(ApiError::Fatal { kind, .. }) if kind == expected), "{code}");
        }
        let mut refused = error("invalid_request", 400, "Claude declined this request.");
        refused["message"]["stop_reason"] = json!("refusal");
        assert!(matches!(classify(&refused), Some(ApiError::Fatal { kind: "a refusal", .. })));
        assert_eq!(kind(error("max_output_tokens", 0, "too long")), None, "Claude Code carries on after these itself");
        assert_eq!(classify(&reply(AT)), None);
    }

    #[test]
    fn the_reset_is_read_in_every_shape_claude_code_writes_it() {
        let clock = |text: &str| reset_clock(text);
        assert_eq!(clock("You've hit your session limit · resets 5pm (Europe/Stockholm) · progress saved"), Some(Clock { hour: 17, zone: Some("Europe/Stockholm".to_string()), ..Clock::default() }));
        assert_eq!(clock("You've hit your session limit · resets 3:45pm"), Some(Clock { hour: 15, minute: 45, ..Clock::default() }));
        assert_eq!(clock("You've hit your weekly limit · resets Mon 12:00am"), Some(Clock { weekday: Some(1), ..Clock::default() }));
        assert_eq!(clock("You've hit your weekly limit · resets Oct 23, 9am (UTC)"), Some(Clock { month: Some(10), day: Some(23), hour: 9, zone: Some("UTC".to_string()), ..Clock::default() }));
        assert_eq!(clock("resets Jan 2, 2027, 9:30 PM"), Some(Clock { year: Some(2027), month: Some(1), day: Some(2), hour: 21, minute: 30, ..Clock::default() }));
        assert_eq!(clock("resets 12pm").map(|clock| clock.hour), Some(12));
        assert_eq!(clock("resets 17:30").map(|clock| (clock.hour, clock.minute)), Some((17, 30)));
        for broken in ["You've reached your Fable limit.", "resets soon", "resets 13pm", "resets Oct 5pm", "resets 5"] {
            assert_eq!(clock(broken), None, "{broken}");
        }
    }

    #[test]
    fn a_reset_time_is_the_next_moment_it_names_in_its_own_zone_across_days_and_daylight_saving() {
        let utc = |_: i64| 0;
        let stockholm_summer = |_: i64| 7200;
        let five = Clock { hour: 17, ..Clock::default() };
        assert_eq!(resolve(&five, AT_S, &utc), Some(AT_S + 22 * 3600), "19:00 UTC has passed five, so it is tomorrow's");
        assert_eq!(resolve(&five, AT_S, &stockholm_summer), Some(AT_S + 20 * 3600), "21:00 local, tomorrow 17:00 local is 15:00 UTC");
        assert_eq!(resolve(&Clock { hour: 20, ..Clock::default() }, AT_S, &utc), Some(AT_S + 3600));
        let monday = Clock { weekday: Some(1), ..Clock::default() };
        assert_eq!(resolve(&monday, AT_S, &utc), Some(1_792_972_800), "Tuesday the 20th, so the Monday after, the 26th");
        let dated = Clock { month: Some(10), day: Some(23), hour: 9, ..Clock::default() };
        assert_eq!(resolve(&dated, AT_S, &utc), Some(1_792_746_000));
        let new_year = Clock { month: Some(1), day: Some(2), hour: 9, ..Clock::default() };
        assert_eq!(resolve(&new_year, AT_S, &utc), Some(1_798_880_400), "January after October is next year's");
        let switch = 1_792_976_400;
        let dst = move |at: i64| if at < switch { 7200 } else { 3600 };
        assert_eq!(resolve(&Clock { month: Some(10), day: Some(26), hour: 9, ..Clock::default() }, 1_792_900_000, &dst), Some(1_793_001_600), "after the switch the offset is an hour");
        assert_eq!(zone_of(Some("Europe/Stockholm"), Some("Europe/Stockholm")), Some(Zone::Local));
        assert_eq!(zone_of(None, Some("Europe/Stockholm")), Some(Zone::Local));
        assert_eq!(zone_of(Some("UTC"), Some("Europe/Stockholm")), Some(Zone::Utc));
        assert_eq!(zone_of(Some("America/New_York"), Some("Europe/Stockholm")), None, "a zone kebbie cannot read gives no time");
    }

    #[test]
    fn without_a_readable_reset_the_status_lines_window_stands_in() {
        let usage = Usage { session: Some(Used { used: 100.0, resets_at: AT_S + 3000 }), week: Some(Used { used: 40.0, resets_at: AT_S + 90_000 }), seen_at: AT_S };
        let here = |_: i64| 0;
        let clock = Clock { hour: 21, zone: Some("UTC".to_string()), ..Clock::default() };
        assert_eq!(reset_at(Some(&clock), Some(Window::Session), AT_S, Some(&usage), None, &here), Some(AT_S + 7200), "the message's own time first");
        let elsewhere = Clock { zone: Some("Asia/Tokyo".to_string()), ..clock.clone() };
        assert_eq!(reset_at(Some(&elsewhere), Some(Window::Session), AT_S, Some(&usage), Some("Europe/Stockholm"), &here), Some(AT_S + 3000));
        assert_eq!(reset_at(None, Some(Window::Week), AT_S, Some(&usage), None, &here), Some(AT_S + 90_000));
        assert_eq!(reset_at(None, None, AT_S, Some(&usage), None, &here), Some(AT_S + 3000), "the window that is used up");
        let fresh = Usage { session: Some(Used { used: 10.0, resets_at: AT_S + 3000 }), week: None, seen_at: AT_S };
        assert_eq!(reset_at(None, None, AT_S, Some(&fresh), None, &here), None);
        assert_eq!(reset_at(None, Some(Window::Session), AT_S, None, None, &here), None);
    }

    #[test]
    fn only_an_error_nobody_typed_after_counts_and_a_reply_ends_it() {
        let failed = error("server_error", 529, "API Error: Repeated 529 Overloaded errors.");
        let tail = observe(&lines(&[user("fix it", "2026-10-20T18:59:00.000Z"), failed.clone()]));
        assert_eq!(tail.failure.as_ref().map(|failure| (failure.id.as_str(), failure.at)), Some(("e-server_error-529", AT_S)));
        let typed = observe(&lines(&[failed.clone(), user("[from kebbie] carry on", "2026-10-20T19:01:00.000Z")]));
        assert!(typed.failure.is_none() && typed.prompted && !typed.replied, "a newer prompt is being worked on");
        let queued = json!({ "type": "attachment", "attachment": { "type": "queued_command", "prompt": "go on" } });
        assert!(observe(&lines(&[failed.clone(), queued])).failure.is_none());
        let answered = observe(&lines(&[failed.clone(), user("go on", AT), reply(AT)]));
        assert!(answered.failure.is_none() && answered.replied);
        let mut helper = failed.clone();
        helper["isSidechain"] = json!(true);
        assert!(observe(&lines(&[reply(AT), helper])).failure.is_none(), "a helper's error is not the task's");
        let tool = json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "content": "ok" }] } });
        assert!(observe(&lines(&[failed, tool])).failure.is_some(), "a tool result is no prompt");
    }

    fn tail(record: Value) -> Tail {
        observe(&record.to_string())
    }

    #[test]
    fn transient_errors_resume_after_one_two_four_and_eight_minutes_then_stop() {
        let mut old: Option<Record> = None;
        let overloaded = |id: &str, at: &str| { let mut record = error("server_error", 529, "API Error: Repeated 529 Overloaded errors."); record["uuid"] = json!(id); record["timestamp"] = json!(at); record };
        let errors = [("e1", "2026-10-20T19:00:00.000Z", AT_S), ("e2", "2026-10-20T19:10:00.000Z", AT_S + 600), ("e3", "2026-10-20T19:20:00.000Z", AT_S + 1200), ("e4", "2026-10-20T19:30:00.000Z", AT_S + 1800)];
        for (index, &(id, at, at_s)) in errors.iter().enumerate() {
            let seen = tail(overloaded(id, at));
            let wait = DELAYS[index];
            match decide("api@web", old.as_ref(), &seen, at_s + wait - 1, None) {
                Act::Keep(record) => {
                    assert_eq!((record.attempts, record.next_at), (index, Some(at_s + wait)));
                    assert!(record.note_at(at_s, &|_| 0).starts_with(&format!("retrying after overload ({}/4, next ", index + 1)), "{}", record.note_at(at_s, &|_| 0));
                }
                other => panic!("waits {wait}s first: {other:?}"),
            }
            let Act::Send(mut record) = decide("api@web", old.as_ref(), &seen, at_s + wait, None) else { panic!("due after {wait}s") };
            record.attempts += 1;
            record.sent_for = Some(record.error_id.clone());
            assert_eq!(decide("api@web", Some(&record), &seen, at_s + wait + 30, None), Act::Nothing, "never twice for one error");
            old = Some(record);
        }
        assert_eq!(old.as_ref().unwrap().note_at(AT_S, &|_| 0), "resumed after overload (4/4)");
        let Act::Stop(stuck) = decide("api@web", old.as_ref(), &tail(overloaded("e5", "2026-10-20T19:40:00.000Z")), AT_S + 4000, None) else { panic!("the fifth error stops") };
        assert_eq!((stuck.kind, stuck.reason.as_str()), (Kind::Stuck, "overload, still after 4 retries"));
        assert_eq!(decide("api@web", Some(&stuck), &tail(overloaded("e5", "2026-10-20T19:40:00.000Z")), AT_S + 5000, None), Act::Nothing, "the owner is told once");
        let replied = observe(&lines(&[overloaded("e5", AT), user("go", AT), reply(AT)]));
        assert_eq!(decide("api@web", old.as_ref(), &replied, AT_S + 9000, None), Act::Forget, "a reply starts the count again");
        assert_eq!(decide("api@web", None, &replied, AT_S + 9000, None), Act::Nothing);
    }

    #[test]
    fn a_usage_limit_waits_for_its_reset_and_shows_it() {
        let limited = tail(error("rate_limit", 429, "You've hit your session limit · resets 9pm (UTC)"));
        let reset = AT_S + 7200;
        let Act::Keep(record) = decide("api@web", None, &limited, AT_S + 60, Some(reset)) else { panic!("waits for the reset") };
        assert_eq!((record.kind, record.next_at), (Kind::Allowance, Some(reset + MARGIN)));
        assert_eq!(record.note_at(AT_S, &|_| 0), "out of allowance until 21:00");
        assert_eq!(record.note_at(AT_S, &|_| 7200), "out of allowance until 23:00", "shown in this machine's time");
        assert!(matches!(decide("api@web", Some(&record), &limited, reset + MARGIN, Some(reset)), Act::Send(_)));
        assert_eq!(resume_text(&record), "Your plan's usage allowance has reset. Carry on where you left off.");
        let far = Record { reset_at: Some(AT_S + 3 * 86_400), ..record.clone() };
        assert_eq!(far.note_at(AT_S, &|_| 0), "out of allowance until Fri 19:00");
        assert!(matches!(decide("api@web", None, &limited, AT_S, None), Act::Stop(stuck) if stuck.reason == "the usage limit, with no reset time kebbie can read"));
        let past = AT_S - 600;
        assert!(matches!(decide("api@web", None, &limited, AT_S + 60, Some(past)), Act::Send(made) if made.next_at == Some(AT_S + MARGIN)), "never sooner than a minute after the error");
    }

    #[test]
    fn a_fatal_error_stops_at_once_with_its_words_and_a_new_prompt_clears_it() {
        let login = error("oauth_org_not_allowed", 403, "Your organization has disabled Claude subscription access for Claude Code");
        let Act::Stop(stuck) = decide("api@web", None, &tail(login.clone()), AT_S, None) else { panic!("never retried") };
        assert_eq!(stuck.note_at(AT_S, &|_| 0), "stopped by a login problem: Your organization has disabled Claude subscription access for Claude Code");
        assert!(stuck_line(&stuck).starts_with("api@web stopped by a login problem and waits under Needs you"), "{}", stuck_line(&stuck));
        assert_eq!(decide("api@web", Some(&stuck), &tail(login.clone()), AT_S + 30, None), Act::Nothing);
        let typed = observe(&lines(&[login, user("I logged in again", AT)]));
        assert_eq!(decide("api@web", Some(&stuck), &typed, AT_S + 60, None), Act::Forget);
    }

    #[test]
    fn a_record_survives_a_restart_in_its_state_file() {
        let state = crate::sh::test_dir("recover-file");
        let record = Record { address: "api@web".to_string(), kind: Kind::Retrying, reason: "overload".to_string(), text: "API Error".to_string(), error_id: "e1".to_string(), error_at: AT_S, attempts: 2, next_at: Some(AT_S + 240), reset_at: None, sent_for: Some("e0".to_string()) };
        save(&state, &record).unwrap();
        assert_eq!(load(&state, "api@web"), Some(record.clone()));
        assert_eq!(addresses(&state), ["api@web"]);
        fs::write(file(&state, "broken@web"), "not json").unwrap();
        assert_eq!(load(&state, "broken@web"), None);
        forget(&state, "api@web");
        assert_eq!(load(&state, "api@web"), None);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_tail_starts_at_a_line() {
        let state = crate::sh::test_dir("recover-tail");
        let path = state.join("t.jsonl");
        let long = "x".repeat(TAIL_BYTES as usize);
        fs::write(&path, format!("{long}\n{}\n", reply(AT))).unwrap();
        let text = tail_of(&path).unwrap();
        assert!(text.starts_with('{'), "the cut-off first line is dropped");
        assert!(observe(&text).replied);
        fs::remove_dir_all(state).unwrap();
    }
}
