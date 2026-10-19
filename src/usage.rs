use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::sh::Result;

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub used: f64,
    pub resets_at: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    pub session: Option<Window>,
    pub week: Option<Window>,
    pub seen_at: u64,
}

fn file_in(state: &Path) -> PathBuf {
    state.join("usage.json")
}

fn window(value: &Value) -> Option<Window> {
    Some(Window { used: value["used_percentage"].as_f64()?, resets_at: value["resets_at"].as_u64().or_else(|| value["resets_at"].as_f64().map(|at| at as u64))? })
}

fn stored(window: &Option<Window>) -> Value {
    window.as_ref().map_or(Value::Null, |window| json!({ "used_percentage": window.used, "resets_at": window.resets_at }))
}

pub fn from_status_input(input: &Value, seen_at: u64) -> Option<Usage> {
    let limits = &input["rate_limits"];
    let usage = Usage { session: window(&limits["five_hour"]), week: window(&limits["seven_day"]), seen_at };
    (usage.session.is_some() || usage.week.is_some()).then_some(usage)
}

// Written whole and renamed into place, so the board never reads half a file.
pub fn save_in(state: &Path, usage: &Usage) -> Result<()> {
    let record = json!({ "five_hour": stored(&usage.session), "seven_day": stored(&usage.week), "seen_at": usage.seen_at });
    let fresh = state.join(format!("usage.json.{}", std::process::id()));
    fs::write(&fresh, record.to_string()).map_err(|error| error.to_string())?;
    fs::rename(&fresh, file_in(state)).map_err(|error| error.to_string())
}

// A window whose reset has passed says nothing about now, so it is left out.
pub fn load_in(state: &Path, now: u64) -> Option<Usage> {
    let record: Value = serde_json::from_str(&fs::read_to_string(file_in(state)).ok()?).ok()?;
    let current = |value: &Value| window(value).filter(|window| window.resets_at > now);
    let usage = Usage { session: current(&record["five_hour"]), week: current(&record["seven_day"]), seen_at: record["seen_at"].as_u64().unwrap_or(0) };
    (usage.session.is_some() || usage.week.is_some()).then_some(usage)
}

pub fn load() -> Option<Usage> {
    load_in(&crate::lead::state_path(|name| std::env::var(name).ok()).ok()?, crate::messages::now())
}

fn percent(used: f64) -> String {
    format!("{}%", used.round() as i64)
}

pub fn line(input: &Value, usage: Option<&Usage>) -> String {
    let model = input["model"]["display_name"].as_str().or_else(|| input["model"]["id"].as_str()).unwrap_or("Claude");
    let mut parts = vec![model.to_string()];
    if let Some(usage) = usage {
        parts.extend(usage.session.as_ref().map(|window| format!("5h {}", percent(window.used))));
        parts.extend(usage.week.as_ref().map(|window| format!("7d {}", percent(window.used))));
    }
    parts.join(" · ")
}

// Claude Code shows whatever this prints, so a state dir that cannot be written costs the board its numbers, never the user their line.
pub fn statusline_in(input: &str, state: Result<PathBuf>) -> String {
    let input: Value = serde_json::from_str(input).unwrap_or_default();
    let usage = from_status_input(&input, crate::messages::now());
    if let Some(usage) = &usage {
        let _ = state.and_then(|state| save_in(&state, usage));
    }
    line(&input, usage.as_ref())
}

pub fn statusline(input: &str) -> String {
    statusline_in(input, crate::lead::state_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-usage-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn input() -> Value {
        json!({
            "model": { "id": "claude-opus-5-5", "display_name": "Opus 5.5" },
            "rate_limits": { "five_hour": { "used_percentage": 23.4, "resets_at": 2000 }, "seven_day": { "used_percentage": 71.0, "resets_at": 9000 } },
        })
    }

    #[test]
    fn the_status_line_names_the_model_and_the_windows_it_was_given() {
        let usage = from_status_input(&input(), 1000);
        assert_eq!(line(&input(), usage.as_ref()), "Opus 5.5 · 5h 23% · 7d 71%");
        let bare = json!({ "model": { "id": "claude-opus-5-5" } });
        assert_eq!(from_status_input(&bare, 1000), None);
        assert_eq!(line(&bare, None), "claude-opus-5-5");
        let week_only = json!({ "model": { "display_name": "Opus" }, "rate_limits": { "seven_day": { "used_percentage": 41, "resets_at": 9000 } } });
        assert_eq!(line(&week_only, from_status_input(&week_only, 1000).as_ref()), "Opus · 7d 41%");
        assert_eq!(line(&Value::Null, None), "Claude");
    }

    #[test]
    fn the_status_line_prints_even_when_the_state_dir_cannot_be_written() {
        let raw = input().to_string();
        assert_eq!(statusline_in(&raw, Ok(PathBuf::from("/dev/null/x"))), "Opus 5.5 · 5h 23% · 7d 71%");
        assert_eq!(statusline_in(&raw, Err("no HOME".to_string())), "Opus 5.5 · 5h 23% · 7d 71%");
    }

    #[test]
    fn the_windows_are_kept_with_when_they_were_seen_and_a_passed_reset_drops_one() {
        let dir = state("load");
        assert_eq!(load_in(&dir, 1000), None);
        save_in(&dir, &from_status_input(&input(), 1000).unwrap()).unwrap();
        assert_eq!(load_in(&dir, 1500), Some(Usage { session: Some(Window { used: 23.4, resets_at: 2000 }), week: Some(Window { used: 71.0, resets_at: 9000 }), seen_at: 1000 }));
        assert_eq!(load_in(&dir, 2000).map(|usage| (usage.session, usage.week.is_some())), Some((None, true)));
        assert_eq!(load_in(&dir, 9000), None);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "nothing but usage.json is left behind");
        fs::remove_dir_all(&dir).unwrap();
    }
}
