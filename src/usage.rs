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
    crate::state::write(&file_in(state), record.to_string())
}

// A window whose reset has passed says nothing about now, so it is left out.
pub fn load_in(state: &Path, now: u64) -> Option<Usage> {
    let record: Value = serde_json::from_str(&fs::read_to_string(file_in(state)).ok()?).ok()?;
    let current = |value: &Value| window(value).filter(|window| window.resets_at > now);
    let usage = Usage { session: current(&record["five_hour"]), week: current(&record["seven_day"]), seen_at: record["seen_at"].as_u64().unwrap_or(0) };
    (usage.session.is_some() || usage.week.is_some()).then_some(usage)
}

pub fn load() -> Option<Usage> {
    load_in(&crate::lead::state_location().ok()?, crate::messages::now())
}

// Claude Code shows whatever this prints under every prompt, so it prints nothing and a state dir that cannot be written costs only the board its numbers.
pub fn record_in(input: &str, state: Result<PathBuf>, lead_dir: Option<&Path>) {
    let input: Value = serde_json::from_str(input).unwrap_or_default();
    let Ok(state) = state else { return };
    if let Some(usage) = from_status_input(&input, crate::messages::now()) {
        let _ = save_in(&state, &usage);
    }
    if let Some(used) = lead_dir.filter(|dir| runs_in(&input, dir)).and_then(|_| context_used(&input)) {
        let _ = crate::state::write(&state.join(LEAD_CONTEXT), json!({ "used_percentage": used }).to_string());
    }
}

pub fn record(input: &str) {
    record_in(input, crate::lead::state_dir(), crate::lead::lead_dir_path().ok().as_deref())
}

const LEAD_CONTEXT: &str = "lead-context.json";

// Claude Code's own share of the model's window, else the last reply's input over the window size it names, none before a first reply; without a window size the share is unknown.
pub fn context_used(input: &Value) -> Option<f64> {
    let window = &input["context_window"];
    window["used_percentage"].as_f64().or_else(|| {
        let size = window["context_window_size"].as_f64().filter(|size| *size > 0.0)?;
        let usage = &window["current_usage"];
        if usage.is_null() {
            return Some(0.0);
        }
        let used: f64 = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].iter().map(|field| usage[field].as_f64()).sum::<Option<f64>>()?;
        Some(used * 100.0 / size)
    })
}

// The lead runs in a folder of its own, so a status line from there is the lead's.
fn runs_in(input: &Value, dir: &Path) -> bool {
    let same = |path: &str| Path::new(path) == dir || fs::canonicalize(path).ok().zip(fs::canonicalize(dir).ok()).is_some_and(|(left, right)| left == right);
    [&input["workspace"]["project_dir"], &input["workspace"]["current_dir"], &input["cwd"]].iter().filter_map(|path| path.as_str()).any(same)
}

pub fn lead_context_in(state: &Path) -> Option<f64> {
    let record: Value = serde_json::from_str(&fs::read_to_string(state.join(LEAD_CONTEXT)).ok()?).ok()?;
    record["used_percentage"].as_f64()
}

pub fn lead_context() -> Option<f64> {
    lead_context_in(&crate::lead::state_location().ok()?)
}

// A new conversation starts empty, so the share of the one before it says nothing.
pub fn forget_lead_context(state: &Path) {
    let _ = fs::remove_file(state.join(LEAD_CONTEXT));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-usage-{name}-{}", std::process::id()));
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
    fn only_status_input_with_a_window_counts_as_usage() {
        assert_eq!(from_status_input(&input(), 1000).map(|usage| usage.session.is_some() && usage.week.is_some()), Some(true));
        assert_eq!(from_status_input(&json!({ "model": { "id": "claude-opus-5-5" } }), 1000), None);
        let week_only = json!({ "rate_limits": { "seven_day": { "used_percentage": 41, "resets_at": 9000 } } });
        assert_eq!(from_status_input(&week_only, 1000).map(|usage| (usage.session, usage.week)), Some((None, Some(Window { used: 41.0, resets_at: 9000 }))));
    }

    #[test]
    fn recording_keeps_the_usage_and_survives_a_state_dir_that_cannot_be_written() {
        let dir = state("record");
        record_in(&input().to_string(), Ok(dir.clone()), None);
        assert_eq!(load_in(&dir, 1500).and_then(|usage| usage.week), Some(Window { used: 71.0, resets_at: 9000 }));
        record_in(&input().to_string(), Ok(PathBuf::from("/dev/null/x")), None);
        record_in("not json", Err("no HOME".to_string()), None);
        fs::remove_dir_all(&dir).unwrap();
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

    #[test]
    fn only_the_leads_status_line_records_its_context_and_a_new_conversation_forgets_it() {
        let dir = state("lead-context");
        let lead = dir.join("lead");
        fs::create_dir_all(&lead).unwrap();
        let from = |cwd: &Path, window: Value| json!({ "cwd": cwd, "workspace": { "current_dir": cwd }, "context_window": window }).to_string();
        record_in(&from(&dir, json!({ "used_percentage": 91.0 })), Ok(dir.clone()), Some(&lead));
        assert_eq!(lead_context_in(&dir), None, "a task's status line is not the lead's");
        record_in(&from(&lead, json!({ "used_percentage": 88.5 })), Ok(dir.clone()), Some(&lead));
        assert_eq!(lead_context_in(&dir), Some(88.5));
        record_in(&from(&lead, json!({ "context_window_size": 200000, "current_usage": { "input_tokens": 10000, "cache_creation_input_tokens": 20000, "cache_read_input_tokens": 150000 } })), Ok(dir.clone()), Some(&lead));
        assert_eq!(lead_context_in(&dir), Some(90.0), "without Claude Code's share, the last reply's input over the window");
        record_in(&from(&lead, json!({ "context_window_size": 200000 })), Ok(dir.clone()), Some(&lead));
        assert_eq!(lead_context_in(&dir), Some(0.0), "a conversation with no reply yet, as after /clear, is empty");
        assert_eq!(context_used(&json!({ "model": { "id": "claude-opus-5-5" } })), None, "no window, no share");
        forget_lead_context(&dir);
        assert_eq!(lead_context_in(&dir), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_window_needs_both_numbers_and_takes_a_fractional_reset_time() {
        assert_eq!(window(&json!({ "used_percentage": 10, "resets_at": 20.9 })), Some(Window { used: 10.0, resets_at: 20 }));
        assert_eq!(window(&json!({ "used_percentage": 10 })), None);
        assert_eq!(window(&json!({ "resets_at": 20 })), None);
        assert_eq!(window(&json!({ "used_percentage": "10", "resets_at": 20 })), None);
        assert_eq!(window(&json!(null)), None);
        let dir = state("broken");
        fs::write(file_in(&dir), "not json").unwrap();
        assert_eq!(load_in(&dir, 1), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
