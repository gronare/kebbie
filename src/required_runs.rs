use std::path::Path;

use crate::config::{BeforePush, Settings};
use crate::evidence::{Outcome, Run};
use crate::repo::git;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Full,
    Lint,
    Tests(After),
}

// Why any green test run on HEAD is owed: the review had findings, or the default branch moved under the task's tests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum After {
    Findings,
    Rebase,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Full => "its full suite",
            Kind::Lint => "its lint",
            Kind::Tests(_) => "its tests",
        }
    }

    fn setting(self) -> &'static str {
        match self {
            Kind::Full => "tests.before_push = \"full\"",
            Kind::Lint => "tests.lint_command",
            Kind::Tests(After::Findings) => "its review had findings",
            Kind::Tests(After::Rebase) => "the default branch moved under its tests",
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
    match required.kind {
        Kind::Full => same_runner && crate::evidence::whole_run(&run.command, &required.command),
        Kind::Lint => same_runner && crate::evidence::exact_run(&run.command, &required.command),
        Kind::Tests(_) => true,
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
    pub from_main: Option<FromMain>,
}

// What a ready report owes, and the lint red on HEAD it was let off because the error came from the default branch and is fixed there.
#[derive(Debug, Default)]
pub struct Owing {
    pub gap: Option<Gap>,
    pub from_main: Option<FromMain>,
}

// The lint red on HEAD may have come with the branch's base: the default branch has moved on from where the branch left it, and no run disproves it. `shown` once the lint's latest runs are red on the base's tree and green on a commit of the default branch since, `fixed`.
#[derive(Clone, Debug, PartialEq)]
pub struct FromMain {
    pub lint: String,
    pub upstream: String,
    pub base: String,
    pub fixed: Option<String>,
    pub shown: bool,
}

impl Gap {
    // "its full suite", "its lint", or both, as the subject of a sentence.
    pub fn names(&self) -> String {
        self.joined(|owed| owed.required.kind.name().to_string())
    }

    pub fn commands(&self) -> String {
        self.joined(|owed| if owed.required.command.is_empty() { "the tests that cover your change".to_string() } else { format!("`{}`", owed.required.command) })
    }

    pub fn has(&self) -> &'static str {
        if self.plural() { "have" } else { "has" }
    }

    pub fn runs(&self) -> &'static str {
        if self.plural() { "run" } else { "runs" }
    }

    fn plural(&self) -> bool {
        self.owed.len() > 1 || self.owed.iter().any(|owed| matches!(owed.required.kind, Kind::Tests(_)))
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

// The required runs a ready report still owes on HEAD; a run counts on the code it ran on, so one run before committing counts for the commit that holds the same tree.
pub fn owed(worktree: &Path, address: &str, excused: Option<&str>) -> Owing {
    let Ok(state) = crate::lead::state_dir() else { return Owing::default() };
    owed_in(&state, worktree, address, None, had_findings(&state, address), excused.map(str::to_string))
}

// The same for a push, which may end at an earlier commit than HEAD and goes out only on green runs of that commit's own code; findings never hold the user's push.
pub fn owed_at(worktree: &Path, address: &str, end: Option<&str>) -> Option<Gap> {
    let state = crate::lead::state_dir().ok()?;
    let excused = std::fs::read_to_string(crate::evidence::excuse_file(&state, address)).ok().map(|code| code.trim().to_string());
    owed_in(&state, worktree, address, end, false, excused).gap
}

// Kept once a ready report with --red-cause is taken, so the push answers to the same red run the report did.
pub fn excuse(state: &Path, address: &str, code: &str) -> crate::sh::Result<()> {
    crate::state::write(&crate::evidence::excuse_file(state, address), code)
}

// The review's latest verdict asked for changes the task makes in code, not only for the user's confirmation.
fn had_findings(state: &Path, address: &str) -> bool {
    std::fs::read_to_string(crate::review::verdict_file(state, address)).ok().and_then(|text| crate::review::parse_verdict(&text)).is_some_and(|verdict| !verdict.ok && !verdict.asks_only_confirmation())
}

// The default branch moved under the task's tests: no green test run holds the base HEAD now sits on, as after a rebase onto a newer default branch; a task that never ran a test needs one once HEAD left the base its branch started on, unless it changes only files that run no test.
fn rebased(worktree: &Path, head: &str, tests: &[Run]) -> bool {
    let Some(upstream) = crate::repo::main_checkout(worktree).and_then(|repo| crate::repo::default_branch(&repo)).map(|upstream| crate::repo::tracking(&upstream)).ok() else { return false };
    let Ok(base) = git(worktree, &["merge-base", head, &upstream]) else { return false };
    if tests.is_empty() {
        let code = git(worktree, &["diff", "--name-only", "--no-renames", &base, head]).map(|files| files.lines().any(|file| !file.is_empty() && !crate::tasks::text_only(file)));
        return code.unwrap_or(true) && started_on(worktree, &upstream).is_none_or(|start| start != base);
    }
    !tests.iter().any(|run| run.result == Outcome::Passed && crate::sh::succeeds("git", &["merge-base", "--is-ancestor", &base, &run.code], Some(worktree)))
}

// Where the task's branch left the default branch when it was made, from the oldest entry of its reflog.
fn started_on(worktree: &Path, upstream: &str) -> Option<String> {
    let branch = git(worktree, &["symbolic-ref", "--short", "HEAD"]).ok()?;
    let created = git(worktree, &["reflog", "show", "--format=%H", &format!("refs/heads/{branch}")]).ok()?.lines().last()?.to_string();
    git(worktree, &["merge-base", &created, upstream]).ok()
}

// The test command a task is pointed to when any green test run will do: the one it ran last, else the repo's first named one.
fn suggested(settings: &Settings, tests: &[Run]) -> String {
    tests.last().map(|run| run.runner.clone()).or_else(|| settings.test_commands.value.first().cloned()).unwrap_or_default()
}

fn owed_in(state: &Path, worktree: &Path, address: &str, end: Option<&str>, findings: bool, excused: Option<String>) -> Owing {
    let Ok(settings) = crate::config::resolve(Some(worktree)).map(|loaded| loaded.settings) else { return Owing::default() };
    crate::evidence::settle(state, address, &crate::evidence::listed_for(&settings), crate::messages::now());
    let Ok(head) = git(worktree, &["rev-parse", end.unwrap_or("HEAD")]) else { return Owing::default() };
    let head_tree = crate::evidence::tree_of(worktree, &head);
    let on_head = |run: &Run| run.code == head || (head_tree.is_some() && crate::evidence::tree_of(worktree, &run.code) == head_tree);
    let (tests, lints) = (crate::evidence::runs_in(state, address), crate::evidence::lint_runs_in(state, address));
    let mut required = required(&settings);
    let after = if findings { Some(After::Findings) } else if rebased(worktree, &head, &tests) { Some(After::Rebase) } else { None };
    if let Some(after) = after.filter(|_| !required.iter().any(|required| required.kind == Kind::Full)) {
        required.push(Required { kind: Kind::Tests(after), command: suggested(&settings, &tests) });
    }
    if required.is_empty() {
        return Owing::default();
    }
    // A red test run whose cause the task found and leaves red answers the test run owed on HEAD while it is the latest and ran on HEAD's code; the lint and the full suite still need theirs.
    let excused_on_head = excused.is_some_and(|code| crate::evidence::latest(&tests).is_some_and(|run| run.result == Outcome::Failed && run.code == code && on_head(run)));
    let mut owed: Vec<Owed> = required.into_iter().filter(|required| !(excused_on_head && matches!(required.kind, Kind::Tests(_)))).filter_map(|required| {
        let runs = match required.kind { Kind::Full | Kind::Tests(_) => &tests, Kind::Lint => &lints };
        missing(runs, &required, on_head).map(|why| Owed { required, why })
    }).collect();
    // Only the whole task, on its own HEAD: once the push rebases it, its base is the tip and the lint is owed on the rebased code again.
    let from_main = end.is_none().then(|| owed.iter().find(|owed| owed.required.kind == Kind::Lint).and_then(|owed| from_main(worktree, &head, &lints, &owed.required))).flatten();
    let shown = from_main.clone().filter(|from_main| from_main.shown);
    if shown.is_some() {
        owed.retain(|owed| owed.required.kind != Kind::Lint);
    }
    let gap = (!owed.is_empty()).then(|| Gap { owed, end: end.map(|end| end[..7.min(end.len())].to_string()), from_main: from_main.filter(|from_main| !from_main.shown) });
    Owing { gap, from_main: shown }
}

// Whether the lint red on HEAD came from the default branch, by the lint's latest run on each tree: red on HEAD's, red on the base's and green on a commit of the default branch since shows it, also once that branch moved on again; a green run on the base or a red one there rules it out.
fn from_main(worktree: &Path, head: &str, lints: &[Run], lint: &Required) -> Option<FromMain> {
    let upstream = crate::repo::main_checkout(worktree).and_then(|repo| crate::repo::default_branch(&repo)).ok()?;
    let tip = git(worktree, &["rev-parse", &crate::repo::tracking(&upstream)]).ok()?;
    let base = git(worktree, &["merge-base", head, &tip]).ok()?;
    if base == tip {
        return None;
    }
    let ran: Vec<(Option<String>, Outcome)> = lints.iter().filter(|run| counts(run, lint)).map(|run| (crate::evidence::tree_of(worktree, &run.code), run.result)).collect();
    let latest_on = |rev: &str| {
        let tree = crate::evidence::tree_of(worktree, rev)?;
        ran.iter().rev().find(|(ran_on, _)| ran_on.as_deref() == Some(tree.as_str())).map(|(_, result)| *result)
    };
    if latest_on(head) != Some(Outcome::Failed) {
        return None;
    }
    let since = git(worktree, &["log", "--format=%H %T", &format!("{base}..{tip}")]).ok()?;
    let since: Vec<(&str, &str)> = since.lines().filter_map(|line| line.split_once(' ')).collect();
    let on_main = ran.iter().rev().find_map(|(ran_on, result)| since.iter().find(|(_, tree)| ran_on.as_deref() == Some(*tree)).map(|(commit, _)| (commit.to_string(), *result)));
    let on_base = latest_on(&base);
    if on_base == Some(Outcome::Passed) || on_main.as_ref().is_some_and(|(_, result)| *result == Outcome::Failed) {
        return None;
    }
    let fixed = on_main.map(|(commit, _)| commit);
    Some(FromMain { lint: lint.command.clone(), upstream, base, shown: on_base == Some(Outcome::Failed) && fixed.is_some(), fixed })
}

impl FromMain {
    // How a task shows it, put on its own branch: a detached worktree would be free to the pool.
    fn how_to_show(&self) -> String {
        let base = short(&self.base);
        format!(
            " When the lint fails on code that came from {upstream}, which has fixed it since your branch left it at {base}, show that instead, with your work committed and each step as a command of its own, since kebbie reads the code a run ran on once its line ends: put the base's code in your worktree with `git restore --source={base} --staged --worktree :/`, run `{lint}`, do the same with `--source={upstream}`, put your own code back with `git restore --source=HEAD --staged --worktree :/`, and report ready again. Stay on your branch: never check out another commit.",
            upstream = self.upstream, lint = self.lint
        )
    }

    // What the ready report says when kebbie took it with the lint red.
    pub fn taken(&self) -> String {
        format!(
            " kebbie took it with your lint red: `{lint}` fails too where your branch left {upstream} ({base}) and passes on {upstream} at {fixed}, so the error comes from {upstream} and is fixed there. The push rebases your branch onto {upstream} and lints the rebased code before it goes out; when that run is red, kebbie hands it to you.",
            lint = self.lint, upstream = self.upstream, base = short(&self.base), fixed = short(self.fixed.as_deref().unwrap_or_default())
        )
    }
}

fn short(sha: &str) -> &str {
    &sha[..7.min(sha.len())]
}

pub fn report_refusal(gap: &Gap) -> String {
    format!(
        "Not reported ready: this repo takes a ready report only on a green run of {} on HEAD ({}), and {}. Run {} in your worktree, each as a command of its own with no pipe after it, then report ready again; when one stays red on something your change did not cause, raise it with kebbie obstacle.",
        gap.names(), gap.settings(), gap.whys(), gap.commands()
    ) + &gap.from_main.as_ref().map(FromMain::how_to_show).unwrap_or_default()
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
    fn after_findings_a_ready_report_owes_a_green_test_run_on_heads_tree_and_a_reworded_commit_keeps_it() {
        let (dir, origin, slot, _) = crate::tasks::test_support::task_with_a_commit("billing");
        let state = crate::lead::state_dir().unwrap();
        let address = crate::peers::address("billing", &crate::repo::name_of(&dir));
        let head = || git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let verdict = crate::review::verdict_file(&state, &address);
        std::fs::create_dir_all(verdict.parent().unwrap()).unwrap();
        let owed_kinds = || owed(&slot.path, &address, None).gap.map(|gap| gap.owed.iter().map(|owed| owed.required.kind).collect::<Vec<_>>());
        assert_eq!(owed_kinds(), None, "no review, nothing owed in a repo on related");
        std::fs::write(&verdict, format!("verdict: changes\nreviewed: {}\n\n- confirm: is the new wording what you want\n", head())).unwrap();
        assert_eq!(owed_kinds(), None, "a finding only the user can settle asks for no test run");
        std::fs::write(&verdict, format!("verdict: changes\nreviewed: {}\n\n- src/billing.rs:3 the total drops the tax\n", head())).unwrap();
        assert_eq!(owed_kinds(), Some(vec![Kind::Tests(After::Findings)]));
        let run = |code: &str, result: Outcome| crate::evidence::save(&state, &address, &Run { command: "cargo test --lib billing".to_string(), runner: "cargo test".to_string(), result, sha: code.to_string(), code: code.to_string(), ..Run::default() }).unwrap();
        run(&head(), Outcome::Failed);
        assert!(owed(&slot.path, &address, None).gap.is_some_and(|gap| report_refusal(&gap).contains("green run of its tests on HEAD (its review had findings)")));
        assert!(owed(&slot.path, &address, Some(&head())).gap.is_none(), "a red run on HEAD whose cause the task found and leaves red, with --red-cause, answers the findings");
        let tested = head();
        run(&tested, Outcome::Passed);
        assert_eq!(owed_kinds(), None, "any green test run on HEAD answers the findings");
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--amend", "-m", "reworded"]).unwrap();
        assert_ne!(head(), tested);
        assert_eq!(owed_kinds(), None, "a reworded commit holds the same tree, so the run still counts");
        crate::tasks::test_support::commit(&slot.path, "fix", "the fix after the run");
        assert_eq!(owed_kinds(), Some(vec![Kind::Tests(After::Findings)]), "code committed after the run is untested");
        let older = head();
        crate::tasks::test_support::commit(&slot.path, "more", "more after the red run");
        run(&older, Outcome::Failed);
        assert!(owed(&slot.path, &address, Some(&older)).gap.is_some(), "a red run on older code excuses nothing on HEAD");
        let _ = std::fs::remove_file(verdict);
        let _ = std::fs::remove_file(crate::evidence::file(&state, &address));
        crate::tasks::test_support::cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_lint_red_on_head_from_main_that_main_has_fixed_holds_no_ready_report_but_a_push_of_part_of_the_task() {
        use crate::tasks::test_support::{cleanup, task_with_a_commit, upstream_commit};
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = crate::lead::state_dir().unwrap();
        let address = crate::peers::address("billing", &crate::repo::name_of(&dir));
        let cmd = "cargo clippy --locked --all-targets -- -D warnings";
        let file = crate::config::repo_file(&dir);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, format!("[tests]\nlint_command = \"{cmd}\"\n")).unwrap();
        let rev = |rev: &str| git(&slot.path, &["rev-parse", rev]).unwrap();
        let (head, base) = (rev("HEAD"), rev(&upstream));
        let ran = |code: &str, result: Outcome| crate::evidence::save(&state, &address, &Run { command: cmd.to_string(), runner: cmd.to_string(), result, sha: code.to_string(), code: code.to_string(), lint: true, ..Run::default() }).unwrap();
        ran(&head, Outcome::Failed);
        assert!(owed(&slot.path, &address, None).gap.is_some_and(|gap| gap.from_main.is_none()), "on the tip of main, a red lint is the task's own");
        upstream_commit(&dir, "fix", "x", "main fixes its lint");
        let tip = rev(&upstream);
        let owing = owed(&slot.path, &address, None);
        let refusal = report_refusal(owing.gap.as_ref().unwrap());
        assert!(owing.from_main.is_none() && refusal.contains(&format!("`git restore --source={} --staged --worktree :/`", &base[..7])) && refusal.contains(&format!("--source={upstream}")) && refusal.contains("each step as a command of its own") && refusal.contains("never check out another commit"), "the refusal says how to show it: {refusal}");
        ran(&base, Outcome::Failed);
        assert!(owed(&slot.path, &address, None).gap.is_some(), "red on the base alone does not say main fixed it");
        ran(&tip, Outcome::Passed);
        let owing = owed(&slot.path, &address, None);
        assert!(owing.gap.is_none(), "red on HEAD and on the base, green on the tip: the error came from main");
        let taken = owing.from_main.unwrap().taken();
        assert!(taken.contains(&format!("passes on {upstream} at {}", &tip[..7])) && taken.contains(&format!("the error comes from {upstream} and is fixed there")) && taken.contains("lints the rebased code"), "{taken}");
        assert!(owed_at(&slot.path, &address, None).is_none(), "the push of the whole task joins the line on the same proof");
        upstream_commit(&dir, "later", "y", "another task's push");
        assert!(owed(&slot.path, &address, None).gap.is_none(), "the green run on main's commit still shows it once main moved on");
        assert!(owed_at(&slot.path, &address, Some(&head)).is_some_and(|gap| gap.owed.iter().any(|owed| owed.required.kind == Kind::Lint)), "a push that ends at a named commit gets no let-off");
        ran(&tip, Outcome::Failed);
        assert!(owed(&slot.path, &address, None).gap.is_some_and(|gap| gap.from_main.is_none()), "red on the tip too: main has not fixed it");
        ran(&tip, Outcome::Passed);
        ran(&base, Outcome::Passed);
        assert!(owed(&slot.path, &address, None).gap.is_some_and(|gap| gap.from_main.is_none()), "green on the base: the error is the task's");
        let _ = std::fs::remove_file(file);
        let _ = std::fs::remove_file(crate::evidence::file(&state, &address));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_restore_the_refusal_names_gives_a_run_the_bases_tree_and_puts_the_branch_back() {
        let (dir, origin, slot, upstream) = crate::tasks::test_support::task_with_a_commit("billing");
        let base = git(&slot.path, &["rev-parse", &upstream]).unwrap();
        git(&slot.path, &["restore", &format!("--source={}", &base[..7]), "--staged", "--worktree", ":/"]).unwrap();
        let (_, dirty, code) = crate::evidence::snapshot(&slot.path).unwrap();
        assert!(dirty && !slot.path.join("d").exists(), "the task's own file goes with its code");
        assert_eq!(crate::evidence::tree_of(&slot.path, &code), crate::evidence::tree_of(&slot.path, &base), "a lint run then counts on the base's tree");
        git(&slot.path, &["restore", "--source=HEAD", "--staged", "--worktree", ":/"]).unwrap();
        assert_eq!(git(&slot.path, &["status", "--porcelain"]).unwrap(), "", "the branch's code is back as committed");
        assert_eq!(git(&slot.path, &["branch", "--show-current"]).unwrap(), "billing", "it never left the branch, so the pool still sees the task");
        crate::tasks::test_support::cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_refusals_name_each_command_to_run() {
        let no_run = |required: Required| Owed { required, why: "kebbie recorded no run of it in this task".to_string() };
        let gap = Gap { owed: vec![no_run(full("uv run pytest -q"))], end: None, from_main: None };
        let report = report_refusal(&gap);
        assert!(report.starts_with("Not reported ready:") && report.contains("Run `uv run pytest -q` in your worktree") && report.contains("kebbie obstacle"), "{report}");
        assert!(report.contains("each as a command of its own with no pipe after it") && !report.contains("nothing after it"), "an echo of its exit after the run is what the role recommends: {report}");
        let push = push_refusal(&gap, "Not pushed", "billing");
        assert!(push.contains("Not pushed: its full suite has no green run on HEAD") && push.contains("`uv run pytest -q`"), "{push}");
        let part = push_refusal(&Gap { end: Some("abc1234".to_string()), ..gap }, "Not pushed", "billing");
        assert!(part.contains("Not pushed: its full suite has no green run on abc1234, where this push ends"), "{part}");
        let both = Gap { owed: vec![no_run(full("uv run pytest -q")), no_run(lint("ruff check"))], end: None, from_main: None };
        let report = report_refusal(&both);
        assert!(report.contains("green run of its full suite and its lint on HEAD (tests.before_push = \"full\", tests.lint_command)") && report.contains("Run `uv run pytest -q` and `ruff check` in your worktree") && report.contains("for `ruff check`, kebbie recorded no run"), "{report}");
        let push = push_refusal(&both, "Not pushed", "billing");
        assert!(push.contains("Not pushed: its full suite and its lint have no green run on HEAD"), "{push}");
        assert!(rebased_prompt(&both).contains("Run `uv run pytest -q` and `ruff check` in this worktree"));
    }
}
