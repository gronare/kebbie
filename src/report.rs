use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::herdr;
use crate::lead::state_dir;
use crate::messages::{self, Report};
use crate::peers;
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use crate::tasks;

pub const STATES: [&str; 4] = ["needs-you", "ready", "working", "blocked"];

const USAGE: &str = "kelpie report --state needs-you|ready|working|blocked --next \"<what the user should do next, or nothing>\" \"<where the task stands>\"";

pub fn parse(state: Option<&str>, next: Option<&str>, summary: &str) -> Result<(Report, String)> {
    let state = state.ok_or(format!("kelpie report needs --state: {USAGE}"))?;
    if !STATES.contains(&state) {
        return Err(format!("--state is one of {}, not \"{state}\"", STATES.join(", ")));
    }
    let next = next.map(str::trim).filter(|next| !next.is_empty()).ok_or(format!("kelpie report needs --next (\"nothing\" when there is nothing for the user): {USAGE}"))?;
    let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        return Err(format!("kelpie report needs a one-line summary: {USAGE}"));
    }
    Ok((Report { state: state.to_string(), next: next.split_whitespace().collect::<Vec<_>>().join(" ") }, summary))
}

pub fn report(cwd: &Path, state: Option<&str>, next: Option<&str>, summary: &str) -> Result<String> {
    let (report, summary) = parse(state, next, summary)?;
    let me = peers::me(cwd).filter(|me| me != LEAD_LABEL).ok_or("kelpie report runs in a task's worktree")?;
    messages::log_report(&me, &report, &summary)?;
    // herdr's sidebar shows the same: the next step as $next, and the state in place of idle or done.
    if let Ok(pane) = std::env::var("HERDR_PANE_ID") {
        let args = metadata_args(&report);
        let _ = herdr::report_metadata(&pane, &args.iter().map(String::as_str).collect::<Vec<_>>());
    }
    let check = if report.state == "ready" { lead_check(cwd) } else { String::new() };
    Ok(format!("Reported to the lead and the board as {me}.{check}"))
}

// A ready task waits under Needs you until the lead has checked it against the rules skill.
fn lead_check(cwd: &Path) -> String {
    match tasks::rules_check(cwd).and_then(|request| request.map(|text| messages::ask_lead(&text)).transpose()) {
        Ok(Some(_)) => " The lead checks your commits against the rules next; it tells you if one needs fixing.".to_string(),
        Ok(None) => String::new(),
        Err(error) => format!(" The lead could not be asked to check the rules: {error}."),
    }
}

fn label(state: &str) -> Option<&'static str> {
    match state {
        "needs-you" => Some("needs you"),
        "ready" => Some("ready"),
        "blocked" => Some("blocked"),
        _ => None,
    }
}

// herdr replaces a source's labels with each new set, and refuses to set and clear the same field at once.
pub fn metadata_args(report: &Report) -> Vec<String> {
    let mut args: Vec<String> = ["--source", "kelpie", "--agent", "claude"].map(str::to_string).to_vec();
    let next = report.next.trim();
    if next.is_empty() || next.eq_ignore_ascii_case("nothing") {
        args.extend(["--clear-token".to_string(), "next".to_string()]);
    } else {
        args.extend(["--token".to_string(), format!("next={next}")]);
    }
    match label(&report.state) {
        Some(label) => args.extend(["idle", "done"].iter().flat_map(|status| ["--state-label".to_string(), format!("{status}={label}")])),
        None => args.push("--clear-state-labels".to_string()),
    }
    args
}

fn turn_file(me: &str) -> Result<PathBuf> {
    let dir = state_dir()?.join("turns");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join(me))
}

// Claude Code sets stop_hook_active on the stop that follows a block, so a session is asked once per turn.
pub fn should_block(stop_hook_active: bool, turn_started: Option<u64>, last_report: Option<u64>) -> bool {
    !stop_hook_active && last_report.is_none_or(|at| at < turn_started.unwrap_or(0))
}

fn block_reason(kelpie: &str) -> String {
    format!("Before you stop, report where this task stands, so the board shows the user what to act on: {kelpie} report --state needs-you|ready|working|blocked --next \"<what the user should do next, or nothing>\" \"<one-line summary>\". Then stop; there is no need to repeat your answer.")
}

// Both hooks kelpie gives a task's Claude: a prompt marks the start of a turn, and a stop without a report since then is sent back once.
pub fn hook(input: &str) -> String {
    let input: Value = serde_json::from_str(input).unwrap_or_default();
    let cwd = input["cwd"].as_str().map(PathBuf::from).or_else(|| std::env::current_dir().ok());
    let Some(me) = cwd.and_then(|cwd| peers::me(&cwd)).filter(|me| me != LEAD_LABEL) else { return String::new() };
    let Ok(turn) = turn_file(&me) else { return String::new() };
    match input["hook_event_name"].as_str() {
        Some("UserPromptSubmit") => {
            let _ = fs::write(turn, messages::now().to_string());
            String::new()
        }
        Some("Stop") => {
            let turn_started = fs::read_to_string(turn).ok().and_then(|at| at.trim().parse().ok());
            let last_report = messages::latest_reports(&messages::load()).get(&me).map(|message| message.at);
            if !should_block(input["stop_hook_active"] == true, turn_started, last_report) {
                return String::new();
            }
            let kelpie = peers::kelpie_command().unwrap_or_else(|_| "kelpie".to_string());
            json!({ "decision": "block", "reason": block_reason(&kelpie) }).to_string()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_needs_a_known_state_a_next_step_and_a_summary() {
        let (report, summary) = parse(Some("needs-you"), Some(" answer:  which queue? "), "plan  written,\nwaiting").unwrap();
        assert_eq!(report, Report { state: "needs-you".to_string(), next: "answer: which queue?".to_string() });
        assert_eq!(summary, "plan written, waiting");
        assert!(parse(None, Some("nothing"), "x").is_err());
        assert!(parse(Some("done"), Some("nothing"), "x").is_err());
        assert!(parse(Some("ready"), None, "x").is_err());
        assert!(parse(Some("ready"), Some("  "), "x").is_err());
        assert!(parse(Some("ready"), Some("push and close"), " ").is_err());
    }

    #[test]
    fn a_stop_is_sent_back_once_when_the_turn_has_no_report() {
        assert!(should_block(false, Some(100), None));
        assert!(should_block(false, Some(100), Some(90)));
        assert!(should_block(false, None, None));
        assert!(!should_block(false, Some(100), Some(100)));
        assert!(!should_block(false, Some(100), Some(120)));
        assert!(!should_block(false, None, Some(5)));
        assert!(!should_block(true, Some(100), None));
    }

    #[test]
    fn herdr_shows_the_next_step_and_the_state() {
        let report = |state: &str, next: &str| Report { state: state.to_string(), next: next.to_string() };
        assert_eq!(metadata_args(&report("needs-you", "answer: which queue?"))[4..], [
            "--token", "next=answer: which queue?", "--state-label", "idle=needs you", "--state-label", "done=needs you",
        ]);
        assert_eq!(metadata_args(&report("working", "nothing"))[4..], ["--clear-token", "next", "--clear-state-labels"]);
    }

    #[test]
    fn the_hook_is_quiet_outside_a_task() {
        assert_eq!(hook(r#"{"hook_event_name":"Stop","cwd":"/","stop_hook_active":false}"#), "");
        assert_eq!(hook("not json"), "");
    }
}
