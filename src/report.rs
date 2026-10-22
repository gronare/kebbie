use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::herdr;
use crate::lead::state_dir;
use crate::messages::{self, Report};
use crate::peers;
use crate::repo::{default_branch, git, main_checkout, name_of};
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use crate::tasks;

pub const STATES: [&str; 4] = ["needs-you", "ready", "working", "blocked"];

const USAGE: &str = "kelpie report --state needs-you|ready|working|blocked --next \"<what the user should do next, or nothing>\" [--card <file>, with ready] \"<where the task stands>\"";

pub fn parse(state: Option<&str>, next: Option<&str>, card: bool, summary: &str) -> Result<(Report, String)> {
    let state = state.ok_or(format!("kelpie report needs --state: {USAGE}"))?;
    if !STATES.contains(&state) {
        return Err(format!("--state is one of {}, not \"{state}\"", STATES.join(", ")));
    }
    if card && !["ready", "needs-you"].contains(&state) {
        return Err(format!("--card goes with --state ready: the user reads the card when they push. {USAGE}"));
    }
    let next = next.map(str::trim).filter(|next| !next.is_empty()).ok_or(format!("kelpie report needs --next (\"nothing\" when there is nothing for the user): {USAGE}"))?;
    let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        return Err(format!("kelpie report needs a one-line summary: {USAGE}"));
    }
    Ok((Report { state: state.to_string(), next: next.split_whitespace().collect::<Vec<_>>().join(" ") }, summary))
}

pub fn report(cwd: &Path, state: Option<&str>, next: Option<&str>, card: Option<&str>, summary: &str) -> Result<String> {
    let (report, summary) = parse(state, next, card.is_some(), summary)?;
    let me = peers::me(cwd).filter(|me| me != LEAD_LABEL).ok_or("kelpie report runs in a task's worktree")?;
    card_goes_with(&report.state, card.is_some(), crate::routines::routine_of(&me).is_some())?;
    let card = card.map(|file| cwd.join(file));
    let text = card.as_deref().map(read_card).transpose()?;
    messages::log_report(&me, &report, &summary)?;
    let kept = match (&card, &text) {
        (Some(file), Some(text)) => take_card(cwd, file, &state_dir()?, &me, text)?,
        _ => String::new(),
    };
    show_in_sidebar(&report);
    if crate::notify::Event::of_state(&report.state).is_some() && crate::routines::found(&me, &summary) {
        crate::notify::send(cwd, &crate::notify::Notice::from(crate::notify::Event::RoutineFindings, &me, format!("{me} found: {summary}")));
    } else if let Some(event) = crate::notify::Event::of_state(&report.state) {
        let text = if event == crate::notify::Event::Ready { format!("{me} is ready: {summary}") } else { format!("{me} needs you: {}", report.next) };
        crate::notify::send(cwd, &crate::notify::Notice::from(event, &me, text));
    }
    let check = if report.state == "ready" { lead_check(cwd) + &review_again(cwd) } else { String::new() };
    Ok(format!("Reported to the lead and the board as {me}.{kept}{check}"))
}

// A routine's task lists its findings on a card while it waits on the user; any other task writes one only when it is ready.
pub fn card_goes_with(state: &str, card: bool, routine: bool) -> Result<()> {
    if card && state != "ready" && !(routine && state == "needs-you") {
        return Err(format!("--card goes with --state ready (or needs-you in a routine's task): the user reads the card when they push. {USAGE}"));
    }
    Ok(())
}

fn review_again(cwd: &Path) -> String {
    match crate::review::again_if_stale(cwd) {
        Ok(Some(_)) => " Your review read older code, so the reviewer reads the branch again; the lead hears its verdict.".to_string(),
        Ok(None) => String::new(),
        Err(error) => format!(" The reviewer could not be started again: {error}."),
    }
}

pub struct Card {
    pub sha: String,
    pub text: String,
    pub file: PathBuf,
}

const CARD_LIMIT: usize = 4_000;

fn card_file(state: &Path, address: &str) -> PathBuf {
    state.join("cards").join(format!("{address}.md"))
}

pub fn read_card(file: &Path) -> Result<String> {
    let text = fs::read_to_string(file).map_err(|error| format!("--card {}: {error}", file.display()))?;
    let text = text.trim();
    if text.is_empty() {
        return Err(format!("--card {} is empty: write the change in the user's words, where to look, and what to confirm", file.display()));
    }
    if text.chars().count() > CARD_LIMIT {
        return Err(format!("--card {} is over {CARD_LIMIT} characters; the user reads it in the push dialog, so keep it to a dozen lines", file.display()));
    }
    Ok(text.to_string())
}

pub fn save_card(state: &Path, address: &str, sha: &str, text: &str) -> Result<()> {
    let file = card_file(state, address);
    crate::state::write(&file, format!("card: {sha}\n\n{text}\n"))
}

// An untracked card left in the worktree would count as an uncommitted change and stop the push, so kelpie keeps the card and removes the file.
fn take_card(cwd: &Path, file: &Path, state: &Path, address: &str, text: &str) -> Result<String> {
    save_card(state, address, &git(cwd, &["rev-parse", "HEAD"])?, text)?;
    let untracked = !crate::sh::succeeds("git", &["ls-files", "--error-unmatch", &file.to_string_lossy()], Some(cwd));
    let top = git(cwd, &["rev-parse", "--show-toplevel"])?;
    let inside = file.canonicalize().is_ok_and(|file| Path::new(&top).canonicalize().is_ok_and(|top| file.starts_with(top)));
    if untracked && inside {
        fs::remove_file(file).map_err(|error| format!("{}: {error}", file.display()))?;
    }
    Ok(" The card is kept for the push dialog.".to_string())
}

pub fn card_in(state: &Path, address: &str) -> Option<Card> {
    let file = card_file(state, address);
    let text = fs::read_to_string(&file).ok()?;
    let (first, body) = text.split_once('\n')?;
    let sha = first.strip_prefix("card: ")?.trim().to_string();
    Some(Card { sha, text: body.trim().to_string(), file })
}

// The card of the task whose worktree this is, and whether HEAD still makes the change it was written for.
pub fn card_for(path: &Path) -> Option<(Card, bool)> {
    let repo = main_checkout(path).ok()?;
    let slot = tasks::find_task(&repo, None, path).ok()?;
    let upstream = default_branch(&repo).ok()?;
    let card = card_in(&state_dir().ok()?, &peers::address(tasks::task_of(&slot), &name_of(&repo)))?;
    let head = git(&slot.path, &["rev-parse", "HEAD"]).ok()?;
    let current = tasks::same_code(&slot.path, &upstream, &card.sha, &head);
    Some((card, current))
}

pub fn forget_card(task: &str, repo: &Path) {
    if let Ok(state) = state_dir() {
        forget_card_in(&state, &peers::address(task, &name_of(repo)));
    }
}

pub fn forget_card_in(state: &Path, address: &str) {
    let _ = fs::remove_file(card_file(state, address));
}

// herdr's sidebar shows the same as the board: the next step as $next, and the state in place of idle or done.
pub fn show_in_sidebar(report: &Report) {
    if let Ok(pane) = std::env::var("HERDR_PANE_ID") {
        let args = metadata_args(report);
        let _ = herdr::report_metadata(&pane, &args.iter().map(String::as_str).collect::<Vec<_>>());
    }
}

// A ready task waits under Ready to push, marked, until the lead has checked it against the rules skill.
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

// When the task's latest turn began: a prompt typed into it, by the user or as a message.
pub fn turn_started(me: &str) -> Option<u64> {
    fs::read_to_string(turn_file(me).ok()?).ok()?.trim().parse().ok()
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
            let _ = crate::state::write(&turn, messages::now().to_string());
            crate::known::stale_for(&me).unwrap_or_default()
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
        let (report, summary) = parse(Some("needs-you"), Some(" answer:  which queue? "), false, "plan  written,\nwaiting").unwrap();
        assert_eq!(report, Report { state: "needs-you".to_string(), next: "answer: which queue?".to_string() });
        assert_eq!(summary, "plan written, waiting");
        assert!(parse(None, Some("nothing"), false, "x").is_err());
        assert!(parse(Some("done"), Some("nothing"), false, "x").is_err());
        assert!(parse(Some("ready"), None, false, "x").is_err());
        assert!(parse(Some("ready"), Some("  "), false, "x").is_err());
        assert!(parse(Some("ready"), Some("push and close"), false, " ").is_err());
    }

    #[test]
    fn a_card_goes_with_a_ready_report_and_a_routines_findings_only() {
        assert!(parse(Some("ready"), Some("push and close"), true, "refunds done").is_ok());
        let error = parse(Some("working"), Some("nothing"), true, "x").err().unwrap();
        assert!(error.contains("--card goes with --state ready"), "{error}");
        assert_eq!(card_goes_with("ready", true, false), Ok(()));
        assert!(card_goes_with("needs-you", true, false).unwrap_err().contains("--card goes with --state ready (or needs-you in a routine's task)"));
        assert_eq!(card_goes_with("needs-you", true, true), Ok(()), "a routine's findings wait on the user with their card");
        assert!(card_goes_with("blocked", true, true).is_err());
        assert_eq!(card_goes_with("needs-you", false, false), Ok(()));
    }

    #[test]
    fn a_card_is_kept_with_the_sha_it_was_written_at_and_gone_once_forgotten() {
        let dir = std::env::temp_dir().join(format!("kelpie-cards-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let worktree = dir.join("wt1.app");
        fs::create_dir_all(&worktree).unwrap();
        git(&worktree, &["init", "--quiet"]).unwrap();
        git(&worktree, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--allow-empty", "-m", "first"]).unwrap();
        let head = git(&worktree, &["rev-parse", "HEAD"]).unwrap();
        let state = dir.join("state");
        let source = worktree.join("card.md");
        fs::write(&source, "  ").unwrap();
        assert!(read_card(&source).unwrap_err().contains("is empty"));
        fs::write(&source, "x".repeat(CARD_LIMIT + 1)).unwrap();
        assert!(read_card(&source).unwrap_err().contains("over 4000"));
        fs::write(&source, "\nRefunds show on the billing page.\n\nLook: /billing\n![the refund row](/tmp/refund.png)\n").unwrap();
        let text = read_card(&source).unwrap();
        assert!(card_in(&state, "billing@app").is_none());
        take_card(&worktree, &source, &state, "billing@app", &text).unwrap();
        assert!(!source.exists(), "a card left in the worktree is an uncommitted file that stops the push");
        fs::create_dir_all(worktree.join("src")).unwrap();
        fs::write(&source, &text).unwrap();
        take_card(&worktree.join("src"), &source, &state, "billing@app", &text).unwrap();
        assert!(!source.exists(), "taken from anywhere in the worktree, whatever folder the report ran in");
        let outside = dir.join("card.md");
        fs::write(&outside, &text).unwrap();
        take_card(&worktree, &outside, &state, "billing@app", &text).unwrap();
        assert!(outside.exists(), "a file outside the worktree is the task's own business");
        assert_eq!(git(&worktree, &["status", "--porcelain"]).unwrap(), "");
        let card = card_in(&state, "billing@app").unwrap();
        assert_eq!(card.sha, head);
        assert_eq!(card.text, "Refunds show on the billing page.\n\nLook: /billing\n![the refund row](/tmp/refund.png)");
        assert!(card_in(&state, "billing@web").is_none());
        forget_card_in(&state, "billing@app");
        assert!(card_in(&state, "billing@app").is_none());
        fs::remove_dir_all(dir).unwrap();
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
