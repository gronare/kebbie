use std::path::Path;

use crate::config::{BeforePush, Settings};
use crate::evidence::{Outcome, Run};
use crate::repo::git;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Full,
    Lint,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Full => "its full suite",
            Kind::Lint => "its lint",
        }
    }

    fn setting(self) -> &'static str {
        match self {
            Kind::Full => "tests.before_push = \"full\"",
            Kind::Lint => "tests.lint_command",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Required {
    pub kind: Kind,
    pub command: String,
}

// The full suite a repo's tasks must pass on HEAD before ready and push; a repo on related, or one that names no full command, asks for none.
pub fn full(settings: &Settings) -> Option<String> {
    settings.tests_full_command.value.clone().filter(|_| settings.tests_before_push.value == BeforePush::Full)
}

// Every run a repo's tasks must have green on HEAD before ready and push: the full suite under tests.before_push = "full", and the lint whenever the repo names one.
pub fn required(settings: &Settings) -> Vec<Required> {
    let full = full(settings).map(|command| Required { kind: Kind::Full, command });
    let lint = settings.tests_lint_command.value.clone().map(|command| Required { kind: Kind::Lint, command });
    full.into_iter().chain(lint).collect()
}

fn counts(run: &Run, required: &Required) -> bool {
    let same_runner = run.runner.trim_start_matches("./") == required.command.trim_start_matches("./");
    same_runner && match required.kind {
        Kind::Full => crate::evidence::whole_run(&run.command, &required.command),
        Kind::Lint => crate::evidence::exact_run(&run.command, &required.command),
    }
}

// What stands between the task and a required run on HEAD: none, or why its runs do not count.
pub fn missing(runs: &[Run], required: &Required, on_head: impl Fn(&Run) -> bool) -> Option<String> {
    if runs.iter().any(|run| run.result == Outcome::Passed && counts(run, required) && on_head(run)) {
        return None;
    }
    Some(match runs.iter().rev().find(|run| counts(run, required)) {
        None => "kebbie recorded no run of it in this task".to_string(),
        Some(run) if run.result == Outcome::Failed => format!("its latest run failed at {}", run.short()),
        Some(run) if run.result == Outcome::Unknown => format!("kebbie could not read how its latest run ended ({})", if run.why.is_empty() { "no result" } else { &run.why }),
        Some(run) => format!("its latest green run was at {}, and HEAD holds other code since", run.short()),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Owed {
    pub required: Required,
    pub why: String,
}

#[derive(Debug)]
pub struct Gap {
    pub owed: Vec<Owed>,
    pub end: Option<String>,
}

impl Gap {
    // "its full suite", "its lint", or both, as the subject of a sentence.
    pub fn names(&self) -> String {
        self.joined(|owed| owed.required.kind.name().to_string())
    }

    pub fn commands(&self) -> String {
        self.joined(|owed| format!("`{}`", owed.required.command))
    }

    pub fn has(&self) -> &'static str {
        if self.owed.len() > 1 { "have" } else { "has" }
    }

    pub fn runs(&self) -> &'static str {
        if self.owed.len() > 1 { "run" } else { "runs" }
    }

    fn settings(&self) -> String {
        self.owed.iter().map(|owed| owed.required.kind.setting()).collect::<Vec<_>>().join(", ")
    }

    fn whys(&self) -> String {
        if let [owed] = self.owed.as_slice() {
            return owed.why.clone();
        }
        self.owed.iter().map(|owed| format!("for `{}`, {}", owed.required.command, owed.why)).collect::<Vec<_>>().join("; ")
    }

    fn joined(&self, each: impl Fn(&Owed) -> String) -> String {
        self.owed.iter().map(each).collect::<Vec<_>>().join(" and ")
    }
}

// The required runs the task still owes on HEAD; a run counts on the code it ran on, so one run before committing counts for the commit that holds the same tree.
pub fn owed(worktree: &Path, address: &str) -> Option<Gap> {
    owed_at(worktree, address, None)
}

// The same for a push that ends at an earlier commit than HEAD, which goes out only on green runs of that commit's own code.
pub fn owed_at(worktree: &Path, address: &str, end: Option<&str>) -> Option<Gap> {
    let settings = crate::config::resolve(Some(worktree)).ok()?.settings;
    let required = required(&settings);
    if required.is_empty() {
        return None;
    }
    let state = crate::lead::state_dir().ok()?;
    crate::evidence::settle(&state, address, &crate::evidence::listed_for(&settings), crate::messages::now());
    let head = git(worktree, &["rev-parse", end.unwrap_or("HEAD")]).ok()?;
    let head_tree = crate::evidence::tree_of(worktree, &head);
    let on_head = |run: &Run| run.code == head || (head_tree.is_some() && crate::evidence::tree_of(worktree, &run.code) == head_tree);
    let (tests, lints) = (crate::evidence::runs_in(&state, address), crate::evidence::lint_runs_in(&state, address));
    let owed: Vec<Owed> = required.into_iter().filter_map(|required| {
        let runs = match required.kind { Kind::Full => &tests, Kind::Lint => &lints };
        missing(runs, &required, on_head).map(|why| Owed { required, why })
    }).collect();
    (!owed.is_empty()).then(|| Gap { owed, end: end.map(|end| end[..7.min(end.len())].to_string()) })
}

pub fn report_refusal(gap: &Gap) -> String {
    format!(
        "Not reported ready: this repo takes a ready report only on a green run of {} on HEAD ({}), and {}. Run {} in your worktree, each as a command of its own, with nothing after it, then report ready again; when one stays red on something your change did not cause, raise it with kebbie obstacle.",
        gap.names(), gap.settings(), gap.whys(), gap.commands()
    )
}

pub fn push_refusal(gap: &Gap, refused: &str, task: &str) -> String {
    let notice = match &gap.end {
        None => crate::notice::Notice::new(crate::notice::Tone::NotNow, format!("{refused}: {} {} no green run on HEAD", gap.names(), gap.has()))
            .next(format!("Its Claude runs {} in its worktree and reports ready; push it again then ({}).", gap.commands(), gap.settings())),
        Some(end) => crate::notice::Notice::new(crate::notice::Tone::NotNow, format!("{refused}: {} {} no green run on {end}, where this push ends", gap.names(), gap.has()))
            .next(format!("Push the whole task once its HEAD has a green run of {}, or have its Claude run it with {end} checked out ({}).", gap.commands(), gap.settings())),
    };
    notice.detail("task", task).detail("tests", gap.whys()).to_string()
}

// What the task's Claude is asked when a push rebased it onto a moved default branch and its required runs are owed again.
pub fn rebased_prompt(gap: &Gap) -> String {
    format!(
        "this repo pushes only on a green run of {} on HEAD ({}). Run {} in this worktree, each as a command of its own, and fix what broke with a new commit. Do not push.",
        gap.names(), gap.settings(), gap.commands()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(command: &str, runner: &str, result: Outcome, code: &str) -> Run {
        Run { command: command.to_string(), runner: runner.to_string(), result, code: code.to_string(), sha: code.to_string(), ..Run::default() }
    }

    fn full(command: &str) -> Required {
        Required { kind: Kind::Full, command: command.to_string() }
    }

    fn lint(command: &str) -> Required {
        Required { kind: Kind::Lint, command: command.to_string() }
    }

    #[test]
    fn only_a_green_whole_run_of_the_full_command_on_head_counts() {
        let on = |code: &'static str| move |run: &Run| run.code == code;
        let cmd = "cargo test --locked";
        let req = full(cmd);
        assert_eq!(missing(&[run("cargo test --locked", cmd, Outcome::Passed, "head")], &req, on("head")), None);
        assert_eq!(missing(&[run("cd w && KEBBIE_STATE_DIR=x cargo test --locked -q 2>&1", cmd, Outcome::Passed, "head")], &req, on("head")), None, "flags and a redirection keep it whole");
        assert_eq!(missing(&[run("cargo test --locked evidence", cmd, Outcome::Passed, "head")], &req, on("head")).as_deref(), Some("kebbie recorded no run of it in this task"), "a filtered run is not the full suite");
        assert_eq!(missing(&[run("cargo test", "cargo test", Outcome::Passed, "head")], &req, on("head")).as_deref(), Some("kebbie recorded no run of it in this task"), "another command is not it");
        assert_eq!(missing(&[run("cargo test --locked", cmd, Outcome::Passed, "old1234")], &req, on("head")).as_deref(), Some("its latest green run was at old1234, and HEAD holds other code since"));
        assert_eq!(missing(&[run("cargo test --locked", cmd, Outcome::Passed, "head"), run("cargo test --locked", cmd, Outcome::Failed, "head")], &req, on("head")), None, "a green run on HEAD counts whatever came after; the red gate judges the latest run");
        assert_eq!(missing(&[run("cargo test --locked", cmd, Outcome::Failed, "abcdef0123")], &req, on("head")).as_deref(), Some("its latest run failed at abcdef0"));
        assert_eq!(missing(&[run("./scripts/check.sh", "scripts/check.sh", Outcome::Passed, "head")], &full("scripts/check.sh"), on("head")), None, "a leading ./ runs the same script");
    }

    #[test]
    fn only_a_green_run_of_exactly_the_lint_command_on_head_counts() {
        let on = |code: &'static str| move |run: &Run| run.code == code;
        let cmd = "cargo clippy --locked --all-targets -- -D warnings";
        let req = lint(cmd);
        assert_eq!(missing(&[run(cmd, cmd, Outcome::Passed, "head")], &req, on("head")), None);
        assert_eq!(missing(&[run(&format!("cd w && {cmd} 2>&1"), cmd, Outcome::Passed, "head")], &req, on("head")), None, "cd and a redirection keep it the same run");
        assert_eq!(missing(&[run(&format!("{cmd} -A clippy::all"), cmd, Outcome::Passed, "head")], &req, on("head")).as_deref(), Some("kebbie recorded no run of it in this task"), "a lint with warnings allowed is not the repo's lint");
        assert_eq!(missing(&[run(cmd, cmd, Outcome::Passed, "old1234")], &req, on("head")).as_deref(), Some("its latest green run was at old1234, and HEAD holds other code since"), "a rebase gives HEAD other code, so the lint runs again");
        assert_eq!(missing(&[run(cmd, cmd, Outcome::Failed, "head")], &req, on("head")).as_deref(), Some("its latest run failed at head"));
        assert_eq!(missing(&[run("cargo test --locked", "cargo test --locked", Outcome::Passed, "head")], &req, on("head")).as_deref(), Some("kebbie recorded no run of it in this task"), "a test run is no lint run");
    }

    #[test]
    fn a_repo_asks_for_the_full_suite_under_full_and_for_its_lint_whenever_it_names_one() {
        let mut settings = crate::config::settings(&crate::config::Worktree::default(), &crate::config::Machine::default());
        assert_eq!(required(&settings), [], "related with no lint is the default");
        settings.tests_full_command.value = Some("uv run pytest -q".to_string());
        assert_eq!(required(&settings), [], "a full command alone changes nothing");
        settings.tests_lint_command.value = Some("ruff check".to_string());
        assert_eq!(required(&settings), [lint("ruff check")], "the lint is required on related too");
        settings.tests_before_push.value = BeforePush::Full;
        assert_eq!(required(&settings), [full("uv run pytest -q"), lint("ruff check")]);
        settings.tests_full_command.value = None;
        assert_eq!(required(&settings), [lint("ruff check")], "full without a command stays on related, and doctor says so");
    }

    #[test]
    fn the_refusals_name_each_command_to_run() {
        let no_run = |required: Required| Owed { required, why: "kebbie recorded no run of it in this task".to_string() };
        let gap = Gap { owed: vec![no_run(full("uv run pytest -q"))], end: None };
        let report = report_refusal(&gap);
        assert!(report.starts_with("Not reported ready:") && report.contains("Run `uv run pytest -q` in your worktree") && report.contains("kebbie obstacle"), "{report}");
        let push = push_refusal(&gap, "Not pushed", "billing");
        assert!(push.contains("Not pushed: its full suite has no green run on HEAD") && push.contains("`uv run pytest -q`"), "{push}");
        let part = push_refusal(&Gap { end: Some("abc1234".to_string()), ..gap }, "Not pushed", "billing");
        assert!(part.contains("Not pushed: its full suite has no green run on abc1234, where this push ends"), "{part}");
        let both = Gap { owed: vec![no_run(full("uv run pytest -q")), no_run(lint("ruff check"))], end: None };
        let report = report_refusal(&both);
        assert!(report.contains("green run of its full suite and its lint on HEAD (tests.before_push = \"full\", tests.lint_command)") && report.contains("Run `uv run pytest -q` and `ruff check` in your worktree") && report.contains("for `ruff check`, kebbie recorded no run"), "{report}");
        let push = push_refusal(&both, "Not pushed", "billing");
        assert!(push.contains("Not pushed: its full suite and its lint have no green run on HEAD"), "{push}");
        assert!(rebased_prompt(&both).contains("Run `uv run pytest -q` and `ruff check` in this worktree"));
    }
}
