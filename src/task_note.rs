use std::path::{Path, PathBuf};

use serde_json::json;

use crate::sh::Result;
use crate::vault::Claim;
use crate::vault_cli::{self, Cli, Released};

pub fn open(cli: &Cli, task: &str, worktree: &Path, item: Option<&str>) -> Result<String> {
    cli.open(task, worktree, Some(task), item.filter(|item| vault_cli::is_note(item))).map(|opened| opened.path)
}

// A started/ entry holds one item per line: the task's vault note, and the backlog item it came from when that is not the note.
pub fn items(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|item| !item.is_empty()).map(str::to_string).collect()
}

pub fn note_in(text: &str) -> Option<String> {
    items(text).into_iter().find(|item| vault_cli::is_note(item))
}

pub fn linked(state: &Path, address: &str) -> Option<String> {
    std::fs::read_to_string(state.join("started").join(address)).ok().and_then(|text| note_in(&text))
}

fn halves(state: &Path, task: &str, address: &str) -> Vec<String> {
    let prefix = format!("{task}@");
    std::fs::read_dir(state.join("started")).into_iter().flatten().flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.starts_with(&prefix) && name != address)
        .collect()
}

// One task across repos has one note: a half started after another takes the note the first one opened.
pub fn shared(state: &Path, task: &str, address: &str) -> Option<String> {
    halves(state, task, address).iter().find_map(|half| linked(state, half))
}

pub fn held_elsewhere(state: &Path, task: &str, address: &str, note: &str) -> bool {
    halves(state, task, address).iter().any(|half| linked(state, half).as_deref() == Some(note))
}

fn passed_file(state: &Path, address: &str) -> PathBuf {
    state.join("vault-gate").join(address)
}

pub fn forget(state: &Path, address: &str) {
    let _ = std::fs::remove_file(passed_file(state, address));
}

#[derive(Debug, PartialEq)]
pub enum Gate {
    Allow,
    Passed,
    Deny(String),
    Warn(String),
}

// Problem written once is written for good, so a pass is kept per note and the vault is asked again only for another note.
pub fn gate(note: &str, passed: Option<&str>, check: impl FnOnce() -> Result<Vec<String>>) -> Gate {
    if passed == Some(note) {
        return Gate::Allow;
    }
    match check() {
        Ok(missing) if missing.iter().any(|key| key == "problem") => Gate::Deny(format!(
            "This task's note {note} has no Problem yet, and code waits for it. With the vault MCP tools, write its Problem section in your own clear words: what is wrong or missing, for whom, and what done looks like, from the user's prompt and what you read. Rewrite the prompt rather than pasting it. Then make this change again."
        )),
        Ok(_) => Gate::Passed,
        Err(error) => Gate::Warn(format!("kebbie could not check {note} in the vault ({error}), so this edit goes through; write its Problem section when the vault answers again.")),
    }
}

pub fn decide(state: &Path, address: &str, cli: impl FnOnce() -> Option<Cli>) -> Gate {
    let Some(note) = linked(state, address) else { return Gate::Allow };
    let passed = std::fs::read_to_string(passed_file(state, address)).ok().map(|text| text.trim().to_string());
    let verdict = gate(&note, passed.as_deref(), || cli().ok_or("the vault command is not available".to_string())?.missing(&note, &["problem"]));
    match &verdict {
        Gate::Passed => {
            let file = passed_file(state, address);
            let _ = crate::state::write(&file, &note);
        }
        Gate::Warn(text) => vault_cli::warn_log(state, &format!("{address}: {text}")),
        _ => {}
    }
    verdict
}

pub fn answer(verdict: &Gate) -> String {
    match verdict {
        Gate::Allow | Gate::Passed => String::new(),
        Gate::Deny(reason) => json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason } }).to_string(),
        Gate::Warn(text) => json!({ "systemMessage": text }).to_string(),
    }
}

// The PreToolUse hook on a task's edits; the task's settings name its address, so a warm pass reads two files and runs nothing.
pub fn hook(address: &str, input: &str) -> String {
    let input: serde_json::Value = serde_json::from_str(input).unwrap_or_default();
    if input["tool_name"] == "Bash" {
        let cwd = input["cwd"].as_str().map(PathBuf::from).or_else(|| std::env::current_dir().ok()).unwrap_or_default();
        let command = input["tool_input"]["command"].as_str().unwrap_or_default();
        if !crate::shell_write::writes_into(command, &cwd) {
            return String::new();
        }
    }
    let Ok(state) = crate::lead::state_dir() else { return String::new() };
    answer(&decide(&state, address, vault_cli::cli))
}

pub fn log_line(task: &str, commits: &[(String, String)]) -> String {
    format!("{task}: {}", commits.iter().map(|(_, subject)| subject.as_str()).collect::<Vec<_>>().join("; "))
}

// `git log --format=%h %s` lines as (sha, subject).
pub fn commits(log: &str) -> Vec<(String, String)> {
    log.lines().filter_map(|line| line.split_once(' ').map(|(sha, subject)| (sha.to_string(), subject.to_string()))).collect()
}

// Ok(None) when there is nothing to log, Ok(Some(path)) for the log note written.
pub fn log_push(cli: &Cli, worktree: &Path, task: &str, log: &str) -> std::result::Result<Option<String>, String> {
    let commits = commits(log);
    if commits.is_empty() {
        return Ok(None);
    }
    let shas: Vec<&str> = commits.iter().map(|(sha, _)| sha.as_str()).collect();
    cli.log(worktree, Some(task), &log_line(task, &commits), &shas).map(Some)
}

pub fn logged_text(logged: &std::result::Result<Option<String>, String>) -> String {
    match logged {
        Ok(Some(path)) => path.clone(),
        Ok(None) => String::new(),
        Err(error) => format!("not written ({error}); write it with the vault's log tool"),
    }
}

// Ok(None) lets done go on; Ok(Some(..)) is the refusal, said to the user and to the task.
pub fn waits(cli: &Cli, note: &str, task: &str) -> std::result::Result<Option<(String, String)>, String> {
    let missing = cli.missing(note, &["solution", "decisions"])?;
    if missing.is_empty() {
        return Ok(None);
    }
    let named = missing.join(" and ");
    let user = crate::notice::Notice::new(crate::notice::Tone::You, format!("Not closed yet: its note lacks its {named}"))
        .next("Its Claude was asked to write them.").you("Close it again once it reports ready.").detail("task", task).detail("note", note).to_string();
    let told = format!("kebbie done waits for your note {note}: write its {named} sections there with the vault MCP tools (what changes from the user's side, and each decision with its reason), then report ready.");
    Ok(Some((user, told)))
}

// Ok(Some(claim)) is another machine's claim, which a start refuses without --anyway; a vault without claims claims nothing.
pub fn claim(cli: &Cli, note: &str, task: &str, repo: &str, anyway: bool) -> std::result::Result<Option<Claim>, String> {
    if !cli.claims {
        return Ok(None);
    }
    cli.claim(note, task, repo, anyway).map(|claimed| match claimed {
        vault_cli::Claimed::Mine => None,
        vault_cli::Claimed::Held(claim) => Some(claim),
    })
}

// Held when another machine has taken the note over, so this machine leaves it as it is; a failed release is logged and lets the end go on.
pub fn release(cli: &Cli, state: &Path, note: &str, task: &str) -> Released {
    if !cli.claims {
        return Released::Done;
    }
    cli.release(note, task).unwrap_or_else(|error| {
        vault_cli::warn_log(state, &format!("{note}: its claim was not cleared ({error})"));
        Released::Done
    })
}

pub fn gone_text(note: &str) -> String {
    format!("{note} is gone from the vault, and its claim with it")
}

// A push or a ready report renews the task's claim, so a note still worked on never reads as stale on the other machine.
pub fn heartbeat(cli: &Cli, state: &Path, address: &str) {
    let Some(note) = linked(state, address).filter(|_| cli.claims) else { return };
    let (task, repo) = address.rsplit_once('@').unwrap_or((address, ""));
    match claim(cli, &note, task, repo, false) {
        Ok(None) => {}
        Ok(Some(held)) => vault_cli::warn_log(state, &format!("{address}: {note} is held by {} · {}, so its claim was not renewed", held.machine, held.task)),
        Err(error) => vault_cli::warn_log(state, &format!("{address}: the claim on {note} was not renewed ({error})")),
    }
}

pub fn refusal(note: &str, claim: &Claim, today: i64) -> String {
    crate::notice::Notice::new(crate::notice::Tone::You, format!("Not started: {} works on it", claim.machine))
        .next("Its vault note is claimed by kebbie on another machine.")
        .you("Start it with --anyway to work on it here too, or pick another item.")
        .detail("note", note).detail("claim", format!("{} since {}", claim.shown(today), claim.since)).to_string()
}

pub fn held_text(note: &str, claim: &Claim, today: i64) -> String {
    format!("{note} is left as it is: {}", claim.shown(today))
}

pub fn close(cli: &Cli, note: &str) -> std::result::Result<String, String> {
    cli.close(note).map(|path| if path.is_empty() { note.to_string() } else { path })
}

pub fn to_backlog(cli: &Cli, note: &str) -> std::result::Result<String, String> {
    cli.backlog(note).map(|path| if path.is_empty() { note.to_string() } else { path })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_cli::fake::Fake;

    fn link(state: &Path, address: &str, item: &str) {
        std::fs::create_dir_all(state.join("started")).unwrap();
        std::fs::write(state.join("started").join(address), item).unwrap();
    }

    #[test]
    fn a_start_with_the_vault_on_opens_a_note_for_the_task() {
        let fake = Fake::new("start");
        fake.answer("task-open", r#"{"path":"Projects/kebbie-billing.md","created":true,"repo":"kebbie","area":"kebbie"}"#);
        assert_eq!(open(&fake.cli, "billing", Path::new("/src/wt1.kebbie"), None).unwrap(), "Projects/kebbie-billing.md");
        assert_eq!(open(&fake.cli, "billing", Path::new("/src/wt1.kebbie"), Some("local:Fix login")).unwrap(), "Projects/kebbie-billing.md");
        assert_eq!(fake.calls(), ["task open --name billing --cwd /src/wt1.kebbie --branch billing"; 2], "a local item or a Linear id is no vault note to claim");
    }

    #[test]
    fn a_start_from_a_vault_backlog_item_claims_that_note() {
        let fake = Fake::new("claim");
        fake.answer("task-open", r#"{"path":"Projects/kebbie-refunds.md","created":false,"repo":"kebbie","area":"kebbie"}"#);
        assert_eq!(open(&fake.cli, "refunds", Path::new("/src/wt2.kebbie"), Some("Projects/kebbie-refunds.md")).unwrap(), "Projects/kebbie-refunds.md");
        assert_eq!(fake.calls(), ["task open --name refunds --cwd /src/wt2.kebbie --branch refunds --note Projects/kebbie-refunds.md"]);
    }

    #[test]
    fn edits_wait_for_the_problem_and_pass_once_it_is_written_asking_no_more() {
        let fake = Fake::new("gate");
        let state = fake.state();
        link(&state, "billing@kebbie", "Projects/kebbie-billing.md");
        fake.answer("task-check", r#"{"missing":["problem","solution","decisions"]}"#);
        let Gate::Deny(reason) = decide(&state, "billing@kebbie", || Some(fake.cli.clone())) else { panic!("an edit before Problem is blocked") };
        assert!(reason.contains("Projects/kebbie-billing.md") && reason.contains("vault MCP tools") && reason.contains("Rewrite the prompt rather than pasting it"), "{reason}");
        assert!(answer(&Gate::Deny(reason)).contains("\"permissionDecision\":\"deny\""));
        fake.answer("task-check", r#"{"missing":["solution","decisions"]}"#);
        assert_eq!(decide(&state, "billing@kebbie", || Some(fake.cli.clone())), Gate::Passed);
        assert_eq!(decide(&state, "billing@kebbie", || Some(fake.cli.clone())), Gate::Allow);
        assert_eq!(fake.calls().len(), 2, "a pass is kept: {:?}", fake.calls());
        assert_eq!(fake.calls()[0], "task check Projects/kebbie-billing.md --need problem");
        link(&state, "billing@kebbie", "Projects/kebbie-other.md");
        fake.answer("task-check", r#"{"missing":["problem"]}"#);
        assert!(matches!(decide(&state, "billing@kebbie", || Some(fake.cli.clone())), Gate::Deny(_)), "a pass holds for its own note only");
    }

    #[test]
    fn an_edit_goes_through_with_a_warning_when_the_vault_does_not_answer() {
        let fake = Fake::new("gate-down");
        let state = fake.state();
        link(&state, "billing@kebbie", "Projects/kebbie-billing.md");
        fake.fail("task-check", "uvx: no network");
        let Gate::Warn(text) = decide(&state, "billing@kebbie", || Some(fake.cli.clone())) else { panic!("a vault that is down never locks the task out") };
        assert!(text.contains("uvx: no network") && text.contains("goes through"), "{text}");
        assert!(answer(&Gate::Warn(text)).contains("systemMessage"));
        assert!(std::fs::read_to_string(state.join("vault.log")).unwrap().contains("billing@kebbie"));
        assert!(matches!(decide(&state, "billing@kebbie", || None), Gate::Warn(_)), "no vault command at all lets it through too");
        assert!(!passed_file(&state, "billing@kebbie").exists(), "a warning is no pass");
    }

    #[test]
    fn a_warm_pass_answers_without_building_the_vault_command() {
        let fake = Fake::new("gate-warm");
        let state = fake.state();
        link(&state, "billing@kebbie", "Projects/kebbie-billing.md");
        std::fs::create_dir_all(state.join("vault-gate")).unwrap();
        std::fs::write(passed_file(&state, "billing@kebbie"), "Projects/kebbie-billing.md").unwrap();
        assert_eq!(decide(&state, "billing@kebbie", || unreachable!("a kept pass reads no config and runs no vault")), Gate::Allow);
    }

    #[test]
    fn the_gate_denies_only_on_the_vaults_own_answer_never_on_its_failure() {
        let fake = Fake::new("gate-never");
        let state = fake.state();
        link(&state, "billing@kebbie", "Projects/kebbie-billing.md");
        fake.answer("task-check", "not json");
        assert!(matches!(decide(&state, "billing@kebbie", || Some(fake.cli.clone())), Gate::Warn(_)), "an answer kebbie cannot read lets the edit through");
        fake.answer("task-check", r#"{"other":1}"#);
        assert!(matches!(decide(&state, "billing@kebbie", || Some(fake.cli.clone())), Gate::Warn(_)));
        let slow = Cli::new(vec!["/nonexistent/vault".to_string()]);
        assert!(matches!(decide(&state, "billing@kebbie", || Some(slow)), Gate::Warn(_)));
    }

    #[test]
    fn a_started_entry_keeps_the_note_next_to_the_backlog_item_it_came_from() {
        assert_eq!(items("Projects/kebbie-billing.md\nlocal:Fix login\n"), ["Projects/kebbie-billing.md", "local:Fix login"]);
        assert_eq!(note_in("local:Fix login\nProjects/kebbie-billing.md").as_deref(), Some("Projects/kebbie-billing.md"));
        assert_eq!(note_in("ABC-12"), None);
        assert_eq!(items("Projects/old.md"), ["Projects/old.md"], "an entry from before reads as it did");
    }

    #[test]
    fn the_halves_of_one_task_share_one_note() {
        let fake = Fake::new("halves");
        let state = fake.state();
        assert_eq!(shared(&state, "billing", "billing@api"), None);
        link(&state, "billing@web", "Projects/web-billing.md\nlocal:Fix login");
        link(&state, "billingx@other", "Projects/other.md");
        assert_eq!(shared(&state, "billing", "billing@api").as_deref(), Some("Projects/web-billing.md"));
        assert_eq!(shared(&state, "billing", "billing@web"), None, "a half is no other half of itself");
        assert!(held_elsewhere(&state, "billing", "billing@api", "Projects/web-billing.md"));
        assert!(!held_elsewhere(&state, "billing", "billing@web", "Projects/web-billing.md"));
        assert!(!held_elsewhere(&state, "billing", "billing@api", "Projects/other.md"));
    }

    #[test]
    fn a_task_without_a_vault_note_is_never_gated_and_the_vault_never_asked() {
        let fake = Fake::new("gate-off");
        let state = fake.state();
        assert_eq!(decide(&state, "billing@kebbie", || Some(fake.cli.clone())), Gate::Allow);
        link(&state, "billing@kebbie", "local:Fix login");
        assert_eq!(decide(&state, "billing@kebbie", || Some(fake.cli.clone())), Gate::Allow);
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn a_push_logs_one_line_of_its_commit_subjects_with_their_shas() {
        let fake = Fake::new("push");
        fake.answer("log-append", r#"{"path":"Log/kebbie-logg.md"}"#);
        let said = logged_text(&log_push(&fake.cli, Path::new("/src/wt1.kebbie"), "billing", "a1b2c3d Add refunds\ne4f5a6b Show refunds on the invoice"));
        assert_eq!(said, "Log/kebbie-logg.md");
        assert_eq!(fake.calls(), ["log append --cwd /src/wt1.kebbie --branch billing --line billing: Add refunds; Show refunds on the invoice --commit a1b2c3d --commit e4f5a6b"]);
        fake.fail("log-append", "S3 unreachable");
        assert!(logged_text(&log_push(&fake.cli, Path::new("/src/wt1.kebbie"), "billing", "a1b2c3d Add refunds")).contains("not written (vault log append: S3 unreachable)"), "a failing vault never fails the push");
        assert_eq!(log_push(&fake.cli, Path::new("/src/wt1.kebbie"), "billing", ""), Ok(None));
    }

    #[test]
    fn done_waits_for_the_solution_and_decisions_and_says_so_to_both() {
        let fake = Fake::new("done");
        fake.answer("task-check", r#"{"missing":["decisions"]}"#);
        let (user, told) = waits(&fake.cli, "Projects/kebbie-billing.md", "billing").unwrap().unwrap();
        assert!(user.contains("its note lacks its decisions") && user.contains("Close it again once it reports ready"), "{user}");
        assert!(told.contains("Projects/kebbie-billing.md") && told.contains("decisions sections") && told.contains("report ready"), "{told}");
        fake.answer("task-check", r#"{"missing":[]}"#);
        assert_eq!(waits(&fake.cli, "Projects/kebbie-billing.md", "billing").unwrap(), None);
        fake.answer("task-close", r#"{"path":"Archive/kebbie-billing.md"}"#);
        assert_eq!(close(&fake.cli, "Projects/kebbie-billing.md").unwrap(), "Archive/kebbie-billing.md");
        assert_eq!(fake.calls(), ["task check Projects/kebbie-billing.md --need solution,decisions", "task check Projects/kebbie-billing.md --need solution,decisions", "task close Projects/kebbie-billing.md"]);
    }

    #[test]
    fn a_vault_without_claims_is_never_asked_to_claim_or_release() {
        let fake = Fake::new("no-claims");
        let state = fake.state();
        assert_eq!(claim(&fake.cli, "Projects/x.md", "billing", "api", false), Ok(None));
        assert_eq!(release(&fake.cli, &state, "Projects/x.md", "billing"), Released::Done);
        link(&state, "billing@api", "Projects/x.md");
        heartbeat(&fake.cli, &state, "billing@api");
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
    }

    #[test]
    fn a_release_that_fails_is_logged_and_never_holds_the_end_up() {
        let fake = Fake::new("release-down");
        let state = fake.state();
        fake.fail("task-release", "S3 unreachable");
        assert_eq!(release(&fake.claiming(), &state, "Projects/x.md", "billing"), Released::Done);
        assert!(std::fs::read_to_string(state.join("vault.log")).unwrap().contains("Projects/x.md: its claim was not cleared (vault task release: S3 unreachable)"));
    }

    #[test]
    fn a_push_or_a_ready_report_renews_the_tasks_claim() {
        let fake = Fake::new("heartbeat");
        let state = fake.state();
        heartbeat(&fake.claiming(), &state, "billing@api");
        assert!(fake.calls().is_empty(), "a task without a vault note renews nothing");
        link(&state, "billing@api", "Projects/api-billing.md\nlocal:Fix login");
        fake.answer("task-claim", r#"{"path":"Projects/api-billing.md","claimed":true,"claim":{"machine":"mac","task":"billing","repo":"api","since":"2026-10-20T08:00:00Z"}}"#);
        heartbeat(&fake.claiming(), &state, "billing@api");
        assert_eq!(fake.calls(), ["task claim Projects/api-billing.md --machine mac --task billing --repo api"]);
        fake.answer("task-claim", r#"{"path":"Projects/api-billing.md","claimed":false,"claim":{"machine":"devbox","task":"x","since":"2026-10-21T08:00:00Z"}}"#);
        heartbeat(&fake.claiming(), &state, "billing@api");
        assert!(std::fs::read_to_string(state.join("vault.log")).unwrap().contains("held by devbox · x, so its claim was not renewed"), "a heartbeat never takes a note over");
        let push = include_str!("tasks/push.rs");
        let report = include_str!("report.rs");
        assert!(push.contains("crate::task_note::heartbeat(cli, state, &address)") && report.contains("crate::task_note::heartbeat(&cli, &state, &me)"));
    }

    #[test]
    fn the_refusal_names_the_machine_the_task_and_the_override() {
        let held = Claim { machine: "devbox".to_string(), task: "search".to_string(), repo: "web".to_string(), since: "2026-10-20T08:00:00Z".to_string(), updated: String::new() };
        let today = crate::backlog::epoch_day("2026-10-22").unwrap();
        let said = refusal("Projects/x.md", &held, today);
        assert!(said.contains("Not started: devbox works on it") && said.contains("--anyway") && said.contains("working on devbox · search since 2026-10-20T08:00:00Z") && said.contains("Projects/x.md"), "{said}");
        assert_eq!(held_text("Projects/x.md", &held, today), "Projects/x.md is left as it is: working on devbox · search");
        assert_eq!(gone_text("Projects/x.md"), "Projects/x.md is gone from the vault, and its claim with it");
    }

    #[test]
    fn a_reset_puts_the_note_back_in_the_backlog() {
        let fake = Fake::new("reset");
        fake.answer("task-backlog", r#"{"path":"Projects/kebbie-billing.md"}"#);
        assert_eq!(to_backlog(&fake.cli, "Projects/kebbie-billing.md").unwrap(), "Projects/kebbie-billing.md");
        assert_eq!(fake.calls(), ["task backlog Projects/kebbie-billing.md"]);
    }

    // KEBBIE_VAULT_E2E=<wrapper> VAULT_PATH=<scratch vault> cargo test -- --ignored real_vault; the wrapper's folder holds wt1.orchard, a worktree of a repo the vault knows.
    #[test]
    #[ignore]
    fn real_vault_open_gate_log_done_close() {
        let Some(wrapper) = std::env::var_os("KEBBIE_VAULT_E2E").map(PathBuf::from) else { return };
        let vault = PathBuf::from(std::env::var("VAULT_PATH").expect("VAULT_PATH names the scratch vault"));
        let worktree = wrapper.parent().unwrap().join("wt1.orchard");
        let cli = Cli::new(vec![wrapper.to_string_lossy().to_string()]);
        let state = std::env::temp_dir().join(format!("kebbie-e2e-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        std::fs::create_dir_all(&state).unwrap();
        let address = "prune@orchard";
        let probe = cli.probe(true).unwrap();
        assert!(probe.on(), "{probe:?}");
        let note = open(&cli, "prune", &worktree, None).unwrap();
        eprintln!("opened {note}");
        link(&state, address, &note);
        let file = vault.join(&note);
        let skeleton = std::fs::read_to_string(&file).unwrap();
        eprintln!("{skeleton}");
        let Gate::Deny(reason) = decide(&state, address, || Some(cli.clone())) else { panic!("an empty Problem denies") };
        eprintln!("denied: {reason}");
        let headings: Vec<String> = skeleton.lines().filter(|line| line.starts_with("## ")).map(str::to_string).collect();
        let problem = headings.first().unwrap().clone();
        std::fs::write(&file, skeleton.replacen(&format!("{problem}\n"), &format!("{problem}\n\nThe orchard's pruning is untracked.\n"), 1)).unwrap();
        assert_eq!(decide(&state, address, || Some(cli.clone())), Gate::Passed);
        assert_eq!(decide(&state, address, || unreachable!()), Gate::Allow);
        let sha = crate::sh::run("git", &["rev-parse", "--short", "HEAD"], Some(&worktree)).unwrap();
        let logged = log_push(&cli, &worktree, "prune", &format!("{sha} Track the pruning")).unwrap().unwrap();
        let log = std::fs::read_to_string(vault.join(&logged)).unwrap();
        eprintln!("logged in {logged}: {}", log.lines().last().unwrap_or_default());
        assert!(log.contains(&sha) && log.contains("prune: Track the pruning"), "{log}");
        let (user, told) = waits(&cli, &note, "prune").unwrap().expect("done waits for the solution and decisions");
        eprintln!("done refuses: {user}\ntask told: {told}");
        let mut text = std::fs::read_to_string(&file).unwrap();
        for heading in &headings[1..3] {
            text = text.replacen(&format!("{heading}\n"), &format!("{heading}\n\nFilled.\n"), 1);
        }
        std::fs::write(&file, text).unwrap();
        assert_eq!(waits(&cli, &note, "prune").unwrap(), None);
        let closed = close(&cli, &note).unwrap();
        eprintln!("closed to {closed}");
        assert!(closed.starts_with("Archive/") && vault.join(&closed).exists() && !file.exists());
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn the_log_line_is_the_subjects_joined_and_the_commits_are_read_from_gits_short_log() {
        let parsed = commits("a1b2c3d Add refunds\ne4f5a6b Show refunds; twice\n\nnosubject\n");
        assert_eq!(parsed, [("a1b2c3d".to_string(), "Add refunds".to_string()), ("e4f5a6b".to_string(), "Show refunds; twice".to_string())]);
        assert_eq!(log_line("billing", &parsed), "billing: Add refunds; Show refunds; twice");
        assert_eq!(log_line("billing", &[]), "billing: ");
        assert_eq!(items("  \n Projects/x.md \n\n local:y \n"), ["Projects/x.md", "local:y"]);
    }

    #[test]
    fn the_gate_answers_claude_code_in_its_hook_shapes() {
        assert_eq!(answer(&Gate::Allow), "");
        assert_eq!(answer(&Gate::Passed), "");
        let deny: serde_json::Value = serde_json::from_str(&answer(&Gate::Deny("why".to_string()))).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(deny["hookSpecificOutput"]["permissionDecisionReason"], "why");
        let warn: serde_json::Value = serde_json::from_str(&answer(&Gate::Warn("down".to_string()))).unwrap();
        assert_eq!(warn["systemMessage"], "down");
        assert!(warn.get("hookSpecificOutput").is_none(), "a warning never denies");
    }

    #[test]
    fn a_pass_recorded_for_another_half_does_not_pass_this_one() {
        let fake = Fake::new("gate-halves");
        let state = fake.state();
        link(&state, "billing@web", "Projects/billing.md");
        link(&state, "billing@api", "Projects/billing.md");
        std::fs::create_dir_all(state.join("vault-gate")).unwrap();
        std::fs::write(passed_file(&state, "billing@web"), "Projects/billing.md").unwrap();
        fake.answer("task-check", r#"{"missing":["problem"]}"#);
        assert_eq!(decide(&state, "billing@web", || unreachable!()), Gate::Allow);
        assert!(matches!(decide(&state, "billing@api", || Some(fake.cli.clone())), Gate::Deny(_)), "each half asks for itself");
        forget(&state, "billing@web");
        assert!(matches!(decide(&state, "billing@web", || Some(fake.cli.clone())), Gate::Deny(_)), "a forgotten pass is asked again");
    }
}
