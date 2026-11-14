use std::path::Path;

use crate::config::{BeforePush, Settings};
use crate::evidence::{Outcome, Run};
use crate::repo::git;

// The command a repo's tasks must pass on HEAD before ready and push; a repo on related, or one that names no full command, asks for none.
pub fn required(settings: &Settings) -> Option<String> {
    settings.tests_full_command.value.clone().filter(|_| settings.tests_before_push.value == BeforePush::Full)
}

fn is_full(run: &Run, full: &str) -> bool {
    run.runner.trim_start_matches("./") == full.trim_start_matches("./") && crate::evidence::whole_run(&run.command, full)
}

// What stands between the task and its full run on HEAD: none, or why its runs do not count.
pub fn missing(runs: &[Run], full: &str, on_head: impl Fn(&Run) -> bool) -> Option<String> {
    if runs.iter().any(|run| run.result == Outcome::Passed && is_full(run, full) && on_head(run)) {
        return None;
    }
    Some(match runs.iter().rev().find(|run| is_full(run, full)) {
        None => "kebbie recorded no run of it in this task".to_string(),
        Some(run) if run.result == Outcome::Failed => format!("its latest run failed at {}", run.short()),
        Some(run) if run.result == Outcome::Unknown => format!("kebbie could not read how its latest run ended ({})", if run.why.is_empty() { "no result" } else { &run.why }),
        Some(run) => format!("its latest green run was at {}, and HEAD holds other code since", run.short()),
    })
}

fn tree(worktree: &Path, rev: &str) -> Option<String> {
    git(worktree, &["rev-parse", &format!("{rev}^{{tree}}")]).ok()
}

pub struct Gap {
    pub full: String,
    pub why: String,
}

// The repo's full run the task still owes on HEAD, if its repo asks for one; a run counts on the code it ran on, so one run before committing counts for the commit that holds the same tree.
pub fn owed(worktree: &Path, address: &str) -> Option<Gap> {
    let settings = crate::config::resolve(Some(worktree)).ok()?.settings;
    let full = required(&settings)?;
    let state = crate::lead::state_dir().ok()?;
    crate::evidence::settle(&state, address, &crate::evidence::listed_for(&settings), crate::messages::now());
    let head = git(worktree, &["rev-parse", "HEAD"]).ok()?;
    let head_tree = tree(worktree, &head);
    let on_head = |run: &Run| run.code == head || (head_tree.is_some() && tree(worktree, &run.code) == head_tree);
    missing(&crate::evidence::runs_in(&state, address), &full, on_head).map(|why| Gap { full, why })
}

pub fn report_refusal(gap: &Gap) -> String {
    format!(
        "Not reported ready: this repo takes a ready report only on a green run of its full suite on HEAD (tests.before_push = \"full\"), and {}. Run `{}` in your worktree as a command of its own, with nothing after it, then report ready again; when it stays red on something your change did not cause, raise it with kebbie obstacle.",
        gap.why, gap.full
    )
}

pub fn push_refusal(gap: &Gap, refused: &str, task: &str) -> String {
    crate::notice::Notice::new(crate::notice::Tone::NotNow, format!("{refused}: its full suite has no green run on HEAD"))
        .next(format!("Its Claude runs `{}` in its worktree and reports ready; push it again then (tests.before_push = \"full\").", gap.full))
        .detail("task", task).detail("tests", &gap.why).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(command: &str, runner: &str, result: Outcome, code: &str) -> Run {
        Run { command: command.to_string(), runner: runner.to_string(), result, code: code.to_string(), sha: code.to_string(), ..Run::default() }
    }

    #[test]
    fn only_a_green_whole_run_of_the_full_command_on_head_counts() {
        let on = |code: &'static str| move |run: &Run| run.code == code;
        let full = "cargo test --locked";
        assert_eq!(missing(&[run("cargo test --locked", full, Outcome::Passed, "head")], full, on("head")), None);
        assert_eq!(missing(&[run("cd w && KEBBIE_STATE_DIR=x cargo test --locked -q 2>&1", full, Outcome::Passed, "head")], full, on("head")), None, "flags and a redirection keep it whole");
        assert_eq!(missing(&[run("cargo test --locked evidence", full, Outcome::Passed, "head")], full, on("head")).as_deref(), Some("kebbie recorded no run of it in this task"), "a filtered run is not the full suite");
        assert_eq!(missing(&[run("cargo test", "cargo test", Outcome::Passed, "head")], full, on("head")).as_deref(), Some("kebbie recorded no run of it in this task"), "another command is not it");
        assert_eq!(missing(&[run("cargo test --locked", full, Outcome::Passed, "old1234")], full, on("head")).as_deref(), Some("its latest green run was at old1234, and HEAD holds other code since"));
        assert_eq!(missing(&[run("cargo test --locked", full, Outcome::Passed, "head"), run("cargo test --locked", full, Outcome::Failed, "head")], full, on("head")), None, "a green run on HEAD counts whatever came after; the red gate judges the latest run");
        assert_eq!(missing(&[run("cargo test --locked", full, Outcome::Failed, "abcdef0123")], full, on("head")).as_deref(), Some("its latest run failed at abcdef0"));
        assert_eq!(missing(&[run("./scripts/check.sh", "scripts/check.sh", Outcome::Passed, "head")], "scripts/check.sh", on("head")), None, "a leading ./ runs the same script");
    }

    #[test]
    fn a_repo_asks_for_a_full_run_only_under_full_with_a_command() {
        let mut settings = crate::config::settings(&crate::config::Worktree::default(), &crate::config::Machine::default());
        assert_eq!(required(&settings), None, "related is the default");
        settings.tests_full_command.value = Some("uv run pytest -q".to_string());
        assert_eq!(required(&settings), None, "a command alone changes nothing");
        settings.tests_before_push.value = BeforePush::Full;
        assert_eq!(required(&settings).as_deref(), Some("uv run pytest -q"));
        settings.tests_full_command.value = None;
        assert_eq!(required(&settings), None, "full without a command stays on related, and doctor says so");
    }

    #[test]
    fn the_refusals_name_the_command_to_run() {
        let gap = Gap { full: "uv run pytest -q".to_string(), why: "kebbie recorded no run of it in this task".to_string() };
        let report = report_refusal(&gap);
        assert!(report.starts_with("Not reported ready:") && report.contains("Run `uv run pytest -q` in your worktree") && report.contains("kebbie obstacle"), "{report}");
        let push = push_refusal(&gap, "Not pushed", "billing");
        assert!(push.contains("Not pushed: its full suite has no green run on HEAD") && push.contains("`uv run pytest -q`"), "{push}");
    }
}
