use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::config::{self, Source};
use crate::sh::Result;

#[derive(Clone)]
pub struct Entry {
    pub source: String,
    pub title: String,
    pub priority: String,
    pub area: String,
    pub reference: String,
    pub summary: String,
    pub age: String,
}

pub struct Backlog {
    pub updated: Option<u64>,
    pub entries: Vec<Entry>,
}

pub const PRIORITIES: [&str; 5] = ["urgent", "high", "medium", "low", "none"];

fn file() -> Result<PathBuf> {
    Ok(crate::lead::state_dir()?.join("backlog.json"))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().trim().to_string()
}

fn entry(value: &Value) -> Option<Entry> {
    let title = text(value, "title");
    if title.is_empty() {
        return None;
    }
    let priority = text(value, "priority").to_lowercase();
    Some(Entry {
        source: text(value, "source"),
        title,
        priority: if PRIORITIES.contains(&priority.as_str()) { priority } else { "none".to_string() },
        area: text(value, "area"),
        reference: text(value, "ref"),
        summary: text(value, "summary"),
        age: text(value, "age"),
    })
}

pub fn load() -> Backlog {
    let parsed: Option<Value> = file().ok().and_then(|path| fs::read_to_string(path).ok()).and_then(|raw| serde_json::from_str(&raw).ok());
    let Some(value) = parsed else { return Backlog { updated: None, entries: Vec::new() } };
    let mut entries: Vec<Entry> = value["entries"].as_array().into_iter().flatten().filter_map(entry).collect();
    let index = crate::vault::Index::open().ok();
    if let Some(index) = &index {
        if config::sources().unwrap_or_default().contains(&Source::Vault) {
            let notes = index.backlog().unwrap_or_default();
            entries.extend(missing_notes(&entries, &notes, today()));
        }
        entries = still_backlog(entries, |key| index.status_of(key), &crate::tasks::started_items());
    }
    sort(&mut entries, |entry| index.as_ref().filter(|_| entry.source == "vault").and_then(|index| index.changed_at(&entry.reference)));
    Backlog { updated: value["updated"].as_u64(), entries }
}

// Backlog notes the lead's last snapshot does not have yet: filed since, or put back by a reset.
fn missing_notes(entries: &[Entry], notes: &[[String; 6]], today: i64) -> Vec<Entry> {
    notes.iter().filter(|[key, ..]| !entries.iter().any(|entry| entry.source == "vault" && &entry.reference == key)).map(|[key, title, summary, priority, area, date]| Entry {
        source: "vault".to_string(),
        title: title.clone(),
        priority: if PRIORITIES.contains(&priority.as_str()) { priority.clone() } else { "none".to_string() },
        area: area.trim_start_matches("[[").trim_end_matches("]]").to_string(),
        reference: key.clone(),
        summary: summary.clone(),
        age: days_since(date, today).map_or(String::new(), |days| format!("{days} d")),
    }).collect()
}

fn today() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs() as i64 / 86_400)
}

fn days_since(date: &str, today: i64) -> Option<i64> {
    epoch_day(date).map(|day| (today - day).max(0))
}

// Days from 1970-01-01 to a YYYY-MM-DD date, by the civil-calendar formula.
fn epoch_day(date: &str) -> Option<i64> {
    let mut parts = date.get(..10)?.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

// By priority, then newest first: by age in days, and within a day by when the note last changed.
fn sort(entries: &mut [Entry], changed_at: impl Fn(&Entry) -> Option<f64>) {
    let days = |entry: &Entry| entry.age.split_whitespace().next().and_then(|days| days.parse::<u64>().ok()).unwrap_or(u64::MAX);
    let mut keyed: Vec<(usize, u64, f64, Entry)> = entries.iter().map(|entry| {
        (PRIORITIES.iter().position(|priority| *priority == entry.priority).unwrap_or(PRIORITIES.len()), days(entry), -changed_at(entry).unwrap_or(0.0), entry.clone())
    }).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)));
    for (slot, (_, _, _, entry)) in entries.iter_mut().zip(keyed) {
        *slot = entry;
    }
}

// An item a running task came from, or a vault note closed since the snapshot, leaves the Backlog column without waiting for a refresh; a reset task's note shows again even while it still reads active.
fn still_backlog(entries: Vec<Entry>, status: impl Fn(&str) -> Option<String>, running: &HashSet<String>) -> Vec<Entry> {
    entries.into_iter()
        .filter(|entry| !running.contains(&entry.reference))
        .filter(|entry| entry.source != "vault" || matches!(status(&entry.reference).as_deref(), Some("backlog" | "active")))
        .collect()
}

pub fn age(updated: Option<u64>) -> String {
    let Some(updated) = updated else { return "never".to_string() };
    let minutes = now().saturating_sub(updated) / 60;
    match minutes {
        0 => "just now".to_string(),
        1..=59 => format!("{minutes} min ago"),
        _ => format!("{} h ago", minutes / 60),
    }
}

// Reads a JSON array of {source, title, priority, area, ref, summary, age} on stdin; the lead keeps it fresh.
pub fn save() -> Result<String> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&input).map_err(|error| format!("Not JSON: {error}"))?;
    let entries: Vec<Value> = value.as_array().ok_or("Expected a JSON array of entries")?.iter()
        .filter(|item| entry(item).is_some()).cloned().collect();
    let count = entries.len();
    let path = file()?;
    fs::write(&path, json!({ "updated": now(), "entries": entries }).to_string()).map_err(|error| error.to_string())?;
    Ok(format!("Saved {count} backlog entries to {}.", path.display()))
}

fn joined(sources: &[Source], part: fn(Source) -> &'static str, word: &str) -> String {
    sources.iter().map(|source| part(*source)).collect::<Vec<_>>().join(word)
}

fn wanted(source: Source) -> &'static str {
    match source {
        Source::Vault => "the vault backlog",
        Source::Linear => "my open Linear issues",
    }
}

fn reference(source: Source) -> &'static str {
    match source {
        Source::Vault => "the vault path",
        Source::Linear => "the Linear id",
    }
}

pub fn from(sources: &[Source]) -> String {
    joined(sources, |source| if source == Source::Vault { "the vault" } else { "Linear" }, " and ")
}

pub fn label(sources: &[Source]) -> String {
    if sources.is_empty() { "no source".to_string() } else { joined(sources, Source::title, " + ") }
}

pub fn not_configured() -> String {
    format!("No backlog source is configured; set [board.backlog] sources = [\"vault\", \"linear\"] (either or both) in {}.", config::path().display())
}

pub fn refresh_prompt(sources: &[Source]) -> Result<String> {
    if sources.is_empty() {
        return Err(not_configured());
    }
    Ok(format!(
        "Refresh the kelpie backlog: take {} and pipe them as one JSON array to `$HOME/.local/share/kelpie/current/target/release/kelpie backlog save`. \
         Fields per entry: source ({}), title, priority (urgent, high, medium, low or none), area (the folder name of the repo the work happens in), \
         ref ({}), summary (one line), age (e.g. 3 d). Reply in one line when saved.",
        joined(sources, wanted, " and "), joined(sources, Source::key, " or "), joined(sources, reference, " or "),
    ))
}

pub fn task_prompt(entry: &Entry, sources: &[Source]) -> String {
    match entry.source.as_str() {
        "linear" if sources.contains(&Source::Linear) => format!("Work on Linear issue {}: {}. Read the issue first, then ask me what you need decided, one question at a time, before you plan with me; no code until I approve the plan.", entry.reference, entry.title),
        "vault" if sources.contains(&Source::Vault) => format!("Work on the vault note {} (\"{}\"). Read it with the vault tools first and set its status to active, then ask me what you need decided, one question at a time, before you plan with me; no code until I approve the plan.", entry.reference, entry.title),
        _ => format!("Work on: {}. {} Ask me what you need decided, one question at a time, before you plan with me; no code until I approve the plan.", entry.title, entry.summary),
    }
}

fn role_wanted(source: Source) -> &'static str {
    match source {
        Source::Vault => "the vault backlog (the vault's backlog tool)",
        Source::Linear => "the user's open Linear issues (assigned to them, not done; Linear's MCP)",
    }
}

fn role_finding(source: Source) -> &'static str {
    match source {
        Source::Vault => "the vault backlog by priority",
        Source::Linear => "Linear (assigned to the user, current cycle)",
    }
}

// Filled into roles/lead.md: {finding} and {backlog}.
pub fn role_texts(sources: &[Source]) -> (String, String) {
    if sources.is_empty() {
        let backlog = format!("No backlog source is configured (`[board.backlog] sources` in {}), so the Backlog tab stays empty and there is nothing to refresh.", config::path().display());
        return ("ask the user what to work on".to_string(), backlog);
    }
    let backlog = format!(
        "The board's Backlog tab shows what you save with `kelpie backlog save`. When the user asks you to refresh it, or when it is more than a day old at a briefing: \
         take {} and pipe them as one JSON array to `kelpie backlog save`. Each entry: `source` ({}), `title`, `priority` (`urgent`, `high`, `medium`, `low` or `none`), \
         `area` (the folder name of the repo the work happens in; not the vault area when the two differ, since starting the task looks the repo up by this name), \
         `ref` ({}), `summary` (one line) and `age` (e.g. `3 d`, `1 mo`).",
        joined(sources, role_wanted, " and "), joined(sources, |source| if source == Source::Vault { "`vault`" } else { "`linear`" }, " or "), joined(sources, reference, " or "),
    );
    (joined(sources, role_finding, " and "), backlog)
}

pub fn show() -> String {
    let backlog = load();
    if backlog.entries.is_empty() {
        return "The backlog is empty; ask the lead to refresh it.".to_string();
    }
    let mut lines = vec![format!("updated {}", age(backlog.updated))];
    for entry in &backlog.entries {
        lines.push(format!("  [{}] {}  ({} · {})", entry.priority, entry.title, entry.source, entry.area));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str) -> Entry {
        Entry { source: source.to_string(), title: "Fix it".to_string(), priority: "high".to_string(), area: "app".to_string(), reference: "ABC-1".to_string(), summary: "It broke.".to_string(), age: "3 d".to_string() }
    }

    #[test]
    fn the_refresh_prompt_names_only_the_configured_sources() {
        let both = refresh_prompt(&[Source::Vault, Source::Linear]).unwrap();
        assert!(both.starts_with("Refresh the kelpie backlog: take the vault backlog and my open Linear issues and pipe"));
        assert!(both.contains("source (vault or linear)") && both.contains("ref (the vault path or the Linear id)"));
        let vault = refresh_prompt(&[Source::Vault]).unwrap();
        assert!(vault.contains("take the vault backlog and pipe") && vault.contains("source (vault)") && !vault.contains("Linear"));
        let linear = refresh_prompt(&[Source::Linear]).unwrap();
        assert!(linear.contains("take my open Linear issues and pipe") && !linear.contains("vault"));
        let none = refresh_prompt(&[]).unwrap_err();
        assert!(none.starts_with("No backlog source is configured") && none.contains("[board.backlog] sources"));
    }

    #[test]
    fn labels_and_the_role_follow_the_sources() {
        assert_eq!(label(&[Source::Vault, Source::Linear]), "vault + Linear");
        assert_eq!(label(&[]), "no source");
        assert_eq!(from(&[Source::Vault]), "the vault");
        let (finding, backlog) = role_texts(&[Source::Vault]);
        assert_eq!(finding, "the vault backlog by priority");
        assert!(!backlog.contains("Linear") && backlog.contains("`source` (`vault`)"));
        let (finding, backlog) = role_texts(&[]);
        assert!(!finding.contains("Linear") && !backlog.contains("Linear") && backlog.contains("No backlog source"));
        assert!(role_texts(&[Source::Linear, Source::Vault]).1.contains("Linear issues (assigned to them, not done; Linear's MCP) and the vault backlog"));
    }

    #[test]
    fn the_newest_comes_first_within_a_priority() {
        let note = |reference: &str, priority: &str, age: &str| Entry { reference: reference.to_string(), priority: priority.to_string(), age: age.to_string(), ..entry("vault") };
        let mut entries = vec![note("old", "high", "9 d"), note("today-early", "high", "0 d"), note("low", "low", "0 d"), note("today-late", "high", "0 d")];
        sort(&mut entries, |entry| Some(match entry.reference.as_str() { "today-early" => 100.0, "today-late" => 200.0, _ => 50.0 }));
        let order: Vec<&str> = entries.iter().map(|entry| entry.reference.as_str()).collect();
        assert_eq!(order, ["today-late", "today-early", "old", "low"]);
    }

    #[test]
    fn backlog_notes_missing_from_the_snapshot_are_added_with_their_age() {
        let snapshot = vec![Entry { reference: "Projects/known.md".to_string(), ..entry("vault") }];
        let row = |key: &str| [key.to_string(), "Title".to_string(), "Sum".to_string(), "high".to_string(), "[[kelpie]]".to_string(), "2026-10-15".to_string()];
        let added = missing_notes(&snapshot, &[row("Projects/known.md"), row("Projects/new.md")], epoch_day("2026-10-18").unwrap());
        assert_eq!(added.len(), 1);
        assert_eq!((added[0].reference.as_str(), added[0].area.as_str(), added[0].age.as_str()), ("Projects/new.md", "kelpie", "3 d"));
        assert_eq!((epoch_day("1970-01-01"), epoch_day("2000-03-01")), (Some(0), Some(11_017)));
    }

    #[test]
    fn a_running_or_closed_item_leaves_the_backlog_and_a_reset_one_returns() {
        let note = |reference: &str| Entry { reference: reference.to_string(), ..entry("vault") };
        let entries = vec![note("Projects/open.md"), note("Projects/running.md"), note("Projects/reset.md"), note("Projects/closed.md"), entry("linear")];
        let status = |key: &str| match key { "Projects/open.md" => Some("backlog".to_string()), "Projects/running.md" | "Projects/reset.md" => Some("active".to_string()), _ => None };
        let running = HashSet::from(["Projects/running.md".to_string()]);
        let kept: Vec<String> = still_backlog(entries, status, &running).into_iter().map(|entry| entry.reference).collect();
        assert_eq!(kept, ["Projects/open.md", "Projects/reset.md", "ABC-1"]);
    }

    #[test]
    fn a_task_prompt_reads_the_source_only_when_it_is_configured() {
        assert!(task_prompt(&entry("linear"), &[Source::Linear]).starts_with("Work on Linear issue ABC-1: Fix it."));
        assert!(task_prompt(&entry("linear"), &[Source::Vault]).starts_with("Work on: Fix it. It broke."));
        assert!(task_prompt(&entry("vault"), &[Source::Vault]).starts_with("Work on the vault note ABC-1"));
        assert!(task_prompt(&entry("vault"), &[Source::Vault]).contains("set its status to active"));
    }
}
