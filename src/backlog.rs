use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

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
    entries.sort_by_key(|entry| PRIORITIES.iter().position(|priority| *priority == entry.priority));
    Backlog { updated: value["updated"].as_u64(), entries }
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
