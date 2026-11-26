use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::issues::{self, Issue};
use crate::sh::Result;

pub mod capture;
pub mod incidents;
pub mod watch;

pub const BY: &str = "fault watcher";
pub const DAILY_FIXES: usize = 3;
pub const DAY: u64 = 86_400;
const KEPT_CASES: usize = 400;
const QUOTE_CHARS: usize = 600;
const CONTEXT_CHARS: usize = 1_500;

pub fn dir(state: &Path) -> PathBuf {
    state.join("faults")
}

// Held across each read, change and write of the fault watcher's files, so the keeper, the board and a watcher run never drop each other's cases.
fn locked(state: &Path) -> Result<crate::state::Locked> {
    crate::state::Locked::wait(&dir(state).join("faults.lock"))
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Chat,
    Board,
    Incident,
}

impl Source {
    pub fn word(self) -> &'static str {
        match self {
            Source::Chat => "the user's words in a chat",
            Source::Board => "the user's answer on the board",
            Source::Incident => "the same incident a second time",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    pub at: u64,
    pub source: Source,
    pub from: String,
    pub words: String,
    #[serde(default)]
    pub context: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    #[serde(default)]
    pub tries: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    next: u64,
    #[serde(default)]
    cases: Vec<Case>,
    #[serde(default)]
    fixes: Vec<u64>,
}

fn store_file(state: &Path) -> PathBuf {
    dir(state).join("cases.json")
}

fn load(state: &Path) -> Store {
    fs::read_to_string(store_file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save(state: &Path, store: &Store) -> Result<()> {
    crate::state::write(&store_file(state), serde_json::to_string_pretty(store).map_err(|error| error.to_string())?)
}

fn change<T>(state: &Path, change: impl FnOnce(&mut Store) -> T) -> Result<T> {
    let _lock = locked(state)?;
    let mut store = load(state);
    let out = change(&mut store);
    save(state, &store)?;
    Ok(out)
}

pub fn clipped(text: &str, most: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= most {
        return text.to_string();
    }
    format!("{}…", text.chars().take(most).collect::<String>().trim_end())
}

pub struct NewCase<'a> {
    pub source: Source,
    pub from: &'a str,
    pub words: &'a str,
    pub context: &'a str,
    pub fingerprint: Option<&'a str>,
}

pub fn add_case(state: &Path, new: &NewCase, now: u64) -> Result<Option<String>> {
    let words = clipped(new.words, QUOTE_CHARS);
    if words.is_empty() {
        return Ok(None);
    }
    change(state, |store| {
        if store.cases.iter().any(|case| case.settled.is_none() && case.from == new.from && case.words == words) {
            return None;
        }
        store.next += 1;
        let id = format!("c{}", store.next);
        store.cases.push(Case { id: id.clone(), at: now, source: new.source, from: new.from.to_string(), words, context: clipped(new.context, CONTEXT_CHARS), fingerprint: new.fingerprint.map(str::to_string), settled: None, issue: None, tries: 0 });
        let settled = store.cases.iter().filter(|case| case.settled.is_some()).count();
        if settled > KEPT_CASES {
            let mut drop = settled - KEPT_CASES;
            store.cases.retain(|case| case.settled.is_none() || if drop > 0 { drop -= 1; false } else { true });
        }
        Some(id)
    })
}

pub fn pending(state: &Path) -> Vec<Case> {
    load(state).cases.into_iter().filter(|case| case.settled.is_none()).collect()
}

#[cfg(test)]
pub fn cases(state: &Path) -> Vec<Case> {
    load(state).cases
}

pub fn tried(state: &Path, ids: &[String]) -> Result<()> {
    change(state, |store| store.cases.iter_mut().filter(|case| ids.contains(&case.id)).for_each(|case| case.tries += 1))
}

pub fn settle(state: &Path, id: &str, outcome: &str, issue: Option<&str>) -> Result<()> {
    change(state, |store| {
        if let Some(case) = store.cases.iter_mut().find(|case| case.id == id) {
            case.settled = Some(outcome.to_string());
            case.issue = issue.map(str::to_string);
        }
    })
}

pub fn next_start(state: &Path, now: u64) -> u64 {
    let mut recent: Vec<u64> = load(state).fixes.into_iter().filter(|at| now.saturating_sub(*at) < DAY).collect();
    recent.sort_unstable();
    if recent.len() < DAILY_FIXES { now } else { recent[recent.len() - DAILY_FIXES] + DAY }
}

pub fn count_fix(state: &Path, starts: u64, now: u64) -> Result<()> {
    change(state, |store| {
        store.fixes.retain(|at| now.saturating_sub(*at) < 2 * DAY);
        store.fixes.push(starts);
    })
}

// What happened each time, kept on this machine only: it holds the user's words, which never go into an issue's text or a report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Occurrence {
    pub at: u64,
    pub case: String,
    pub source: Source,
    pub from: String,
    pub quote: String,
    #[serde(default)]
    pub came_back: bool,
}

fn occurrences_file(state: &Path, id: &str) -> PathBuf {
    dir(state).join("occurrences").join(format!("{id}.jsonl"))
}

pub fn record_occurrence(state: &Path, id: &str, occurrence: &Occurrence) -> Result<()> {
    use std::io::Write;
    let _lock = locked(state)?;
    let file = occurrences_file(state, id);
    let mut out = crate::state::append_to(&file).map_err(|error| format!("{}: {error}", file.display()))?;
    writeln!(out, "{}", serde_json::to_string(occurrence).map_err(|error| error.to_string())?).map_err(|error| error.to_string())
}

pub fn occurrences(state: &Path, id: &str) -> Vec<Occurrence> {
    fs::read_to_string(occurrences_file(state, id)).unwrap_or_default().lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

pub fn occurrences_line(state: &Path, id: &str) -> Option<String> {
    let all = occurrences(state, id);
    if all.is_empty() {
        return None;
    }
    let back = all.iter().filter(|occurrence| occurrence.came_back).count();
    let after = if back > 0 { format!(", {back} after its fix was in use") } else { String::new() };
    Some(format!("seen {} {}{after} · kebbie faults show {id}", all.len(), if all.len() == 1 { "time" } else { "times" }))
}

// The keeper's recheck closes an issue only once the installed kebbie holds its fix, so a fixed one with that line is in use on this machine.
pub fn in_use(issue: &Issue) -> bool {
    issue.status == "fixed" && issue.history.last().is_some_and(|line| line.ends_with("checked automatically"))
}

pub fn tracked(state: &Path) -> Vec<Issue> {
    issues::load(&issues::dir(state)).into_iter().filter(|issue| issue.source.is_none() && (issue.by == BY || occurrences_file(state, &issue.id).exists())).collect()
}

fn state_word(state: &Path, issue: &Issue) -> String {
    if in_use(issue) {
        return "in use".to_string();
    }
    if !issue.is_open() {
        return issue.status.clone();
    }
    let word = issue.fix.as_ref().map_or_else(|| "open".to_string(), |task| format!("fixing in {task}"));
    let back = occurrences(state, &issue.id).iter().any(|occurrence| occurrence.came_back);
    format!("{word}{}", if back { ", came back" } else { "" })
}

pub fn list_in(state: &Path, as_json: bool) -> String {
    let tracked = tracked(state);
    if as_json {
        let rows: Vec<_> = tracked.iter().map(|issue| {
            let seen = occurrences(state, &issue.id);
            json!({ "id": issue.id, "title": issue.title, "status": issue.status, "in_use": in_use(issue), "fix": issue.fix, "cause": issue.cause, "barrier": issue.barrier, "seen": seen.len(), "came_back": seen.iter().filter(|occurrence| occurrence.came_back).count() })
        }).collect();
        return serde_json::to_string_pretty(&json!({ "faults": rows, "pending": pending(state).len() })).unwrap_or_default();
    }
    let waiting = pending(state).len();
    let waiting = if waiting > 0 { format!("\n{waiting} {} for the fault watcher to weigh.", if waiting == 1 { "case waits" } else { "cases wait" }) } else { String::new() };
    if tracked.is_empty() {
        return format!("The fault watcher follows no faults yet.{waiting}");
    }
    let lines: Vec<String> = tracked.iter().map(|issue| {
        let seen = occurrences(state, &issue.id);
        let back = seen.iter().filter(|occurrence| occurrence.came_back).count();
        format!("{}  {:<24}  seen {}{}  {}", issue.id, state_word(state, issue), seen.len(), if back > 0 { format!(" ({back} after)") } else { String::new() }, issue.title)
    }).collect();
    format!("{}{waiting}", lines.join("\n"))
}

pub fn show_in(state: &Path, id: &str) -> Result<String> {
    let mut lines = vec![issues::show_in(state, id)?];
    let seen = occurrences(state, id);
    if !seen.is_empty() {
        lines.push(String::new());
        lines.push("Each time, kept on this machine only:".to_string());
        lines.extend(seen.iter().map(|occurrence| format!("- {} {}{}: {}", crate::keeper::stamp(occurrence.at), occurrence.from, if occurrence.came_back { " (after its fix was in use)" } else { "" }, occurrence.quote)));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests;
