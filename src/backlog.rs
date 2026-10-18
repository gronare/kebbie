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
    let base = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .map_err(|_| "Neither XDG_STATE_HOME nor HOME is set".to_string())?;
    let dir = base.join("kelpie");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join("backlog.json"))
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
    if let Ok(index) = crate::vault::Index::open() {
        entries = still_backlog(entries, |key| index.status_of(key));
    }
    entries.sort_by_key(|entry| PRIORITIES.iter().position(|priority| *priority == entry.priority));
    Backlog { updated: value["updated"].as_u64(), entries }
}

// A vault note picked up or closed since the snapshot leaves the Backlog column without waiting for a refresh.
fn still_backlog(entries: Vec<Entry>, status: impl Fn(&str) -> Option<String>) -> Vec<Entry> {
    entries.into_iter().filter(|entry| entry.source != "vault" || status(&entry.reference).as_deref() == Some("backlog")).collect()
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
    fn a_vault_note_picked_up_since_the_snapshot_leaves_the_backlog() {
        let note = |reference: &str| Entry { reference: reference.to_string(), ..entry("vault") };
        let entries = vec![note("Projects/open.md"), note("Projects/started.md"), note("Projects/closed.md"), entry("linear")];
        let status = |key: &str| match key { "Projects/open.md" => Some("backlog".to_string()), "Projects/started.md" => Some("active".to_string()), _ => None };
        let kept: Vec<String> = still_backlog(entries, status).into_iter().map(|entry| entry.reference).collect();
        assert_eq!(kept, ["Projects/open.md", "ABC-1"]);
    }

    #[test]
    fn a_task_prompt_reads_the_source_only_when_it_is_configured() {
        assert!(task_prompt(&entry("linear"), &[Source::Linear]).starts_with("Work on Linear issue ABC-1: Fix it."));
        assert!(task_prompt(&entry("linear"), &[Source::Vault]).starts_with("Work on: Fix it. It broke."));
        assert!(task_prompt(&entry("vault"), &[Source::Vault]).starts_with("Work on the vault note ABC-1"));
        assert!(task_prompt(&entry("vault"), &[Source::Vault]).contains("set its status to active"));
    }
}
