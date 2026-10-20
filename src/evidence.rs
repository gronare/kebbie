use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::peers;
use crate::repo::{default_branch, git, is_clean, main_checkout, name_of};
use crate::sh::Result;
use crate::tasks;

pub const DEFAULT_COMMANDS: [&str; 16] = [
    "cargo test", "cargo nextest", "bin/rails test", "bundle exec rails test", "bundle exec rspec", "bin/rspec", "rspec",
    "pytest", "python -m pytest", "uv run pytest", "npm test", "npm run test", "pnpm test", "yarn test", "go test", "mix test",
];

const KEPT: usize = 20;
const COMMAND_LIMIT: usize = 300;

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub command: String,
    pub runner: String,
    pub ok: bool,
    pub exit: Option<i64>,
    pub duration_ms: Option<u64>,
    pub summary: String,
    pub sha: String,
    pub dirty: bool,
    pub code: String,
    pub at: u64,
}

impl Run {
    pub fn to_json(&self) -> Value {
        json!({
            "command": self.command, "runner": self.runner, "ok": self.ok, "exit": self.exit, "duration_ms": self.duration_ms,
            "summary": self.summary, "sha": self.sha, "dirty": self.dirty, "code": self.code, "at": self.at,
        })
    }

    fn from_json(value: &Value) -> Option<Run> {
        Some(Run {
            command: value["command"].as_str()?.to_string(),
            runner: value["runner"].as_str()?.to_string(),
            ok: value["ok"].as_bool()?,
            exit: value["exit"].as_i64(),
            duration_ms: value["duration_ms"].as_u64(),
            summary: value["summary"].as_str().unwrap_or_default().to_string(),
            sha: value["sha"].as_str()?.to_string(),
            dirty: value["dirty"].as_bool().unwrap_or(false),
            code: value["code"].as_str()?.to_string(),
            at: value["at"].as_u64().unwrap_or(0),
        })
    }

    fn short(&self) -> String {
        self.sha.chars().take(7).collect()
    }
}

pub fn commands(configured: &[String]) -> Vec<String> {
    DEFAULT_COMMANDS.iter().map(|command| command.to_string()).chain(configured.iter().cloned()).collect()
}

fn assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else { return false };
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && chars.all(|char| char.is_ascii_alphanumeric() || char == '_')
}

// The end of the leading word, a quoted value counting as part of it.
fn word_end(text: &str) -> usize {
    let mut quote = None;
    for (index, char) in text.char_indices() {
        match (quote, char) {
            (Some(open), _) if char == open => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(char),
            (None, char) if char.is_whitespace() => return index,
            _ => {}
        }
    }
    text.len()
}

// The command as it runs after any leading `cd … &&` and environment assignments.
pub fn core(command: &str) -> &str {
    let mut rest = command.trim();
    loop {
        if (rest.starts_with("cd ") || rest.starts_with("cd\t")) && let Some((_, after)) = rest.split_once("&&") {
            rest = after.trim_start();
            continue;
        }
        if let Some(after) = rest.strip_prefix("env ") {
            rest = after.trim_start();
            continue;
        }
        let end = word_end(rest);
        if end < rest.len() && assignment(&rest[..end]) {
            rest = rest[end..].trim_start();
            continue;
        }
        return rest;
    }
}

// The longest listed command the given one starts with, whole words only.
pub fn recognise<'a>(command: &str, listed: &'a [String]) -> Option<&'a str> {
    let core = core(command);
    listed.iter().map(|prefix| prefix.trim()).filter(|prefix| !prefix.is_empty())
        .filter(|prefix| core == *prefix || core.strip_prefix(prefix).is_some_and(|rest| rest.starts_with(char::is_whitespace)))
        .max_by_key(|prefix| prefix.len())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub text: String,
    pub failed: bool,
}

fn plain(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(char) = chars.next() {
        if char == '\u{1b}' && chars.peek() == Some(&'[') {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(char);
        }
    }
    out
}

// The number right before a word starting with `noun`, as in "3 failed" or "12 examples,".
fn count(text: &str, noun: &str) -> Option<u64> {
    let words: Vec<&str> = text.split(|char: char| char.is_whitespace() || char == ',' || char == ';' || char == '|' || char == '(' || char == ')').filter(|word| !word.is_empty()).collect();
    words.windows(2).find(|pair| pair[1].starts_with(noun)).and_then(|pair| pair[0].parse().ok())
}

fn generic(line: &str) -> Option<Summary> {
    let trimmed = line.trim();
    let nonzero = |noun: &str| count(trimmed, noun).is_some_and(|count| count > 0);
    if let Some(rest) = trimmed.strip_prefix("Summary [").and_then(|rest| rest.split_once(']')).map(|(_, rest)| rest.trim()) {
        let text = rest.split_once(": ").map_or(rest, |(_, counts)| counts).to_string();
        return Some(Summary { failed: nonzero("failed") || nonzero("timed"), text });
    }
    if count(trimmed, "example").is_some() && count(trimmed, "failure").is_some() {
        return Some(Summary { failed: nonzero("failure") || nonzero("error"), text: trimmed.to_string() });
    }
    if count(trimmed, "run").is_some() && count(trimmed, "assertion").is_some() {
        return Some(Summary { failed: nonzero("failure") || nonzero("error"), text: trimmed.to_string() });
    }
    if count(trimmed, "test").is_some() && count(trimmed, "failure").is_some() {
        return Some(Summary { failed: nonzero("failure"), text: trimmed.to_string() });
    }
    if trimmed.starts_with('=') && trimmed.ends_with('=') && trimmed.contains(" in ") {
        let text = trimmed.trim_matches(|char| char == '=' || char == ' ').to_string();
        if ["passed", "failed", "error", "no tests ran"].iter().any(|word| text.contains(word)) {
            return Some(Summary { failed: text.contains("failed") || text.contains("error"), text });
        }
    }
    if let Some(rest) = trimmed.strip_prefix("Tests:").or_else(|| trimmed.strip_prefix("Tests ")) {
        let text = rest.trim().to_string();
        if text.contains("passed") || text.contains("failed") {
            return Some(Summary { failed: nonzero("failed"), text });
        }
    }
    None
}

// The result lines of the common runners, read from the end of the output; cargo's and go's lines are summed over every test binary or package.
pub fn summary(output: &str) -> Option<Summary> {
    let lines: Vec<String> = output.lines().map(plain).collect();
    let cargo: Vec<&String> = lines.iter().filter(|line| line.trim_start().starts_with("test result: ")).collect();
    if !cargo.is_empty() {
        let sum = |noun: &str| cargo.iter().filter_map(|line| count(line, noun)).sum::<u64>();
        let (passed, failed) = (sum("passed"), sum("failed"));
        let broke = failed > 0 || cargo.iter().any(|line| line.contains("FAILED"));
        let text = if failed > 0 { format!("{passed} passed, {failed} failed") } else { format!("{passed} passed") };
        return Some(Summary { text, failed: broke });
    }
    if let Some(found) = lines.iter().rev().find_map(|line| generic(line)) {
        return Some(found);
    }
    let passing = lines.iter().rev().find_map(|line| count(line.trim(), "passing"));
    let failing = lines.iter().rev().find_map(|line| count(line.trim(), "failing"));
    if passing.is_some() || failing.is_some() {
        let mut text = format!("{} passing", passing.unwrap_or(0));
        if let Some(failing) = failing {
            text.push_str(&format!(", {failing} failing"));
        }
        return Some(Summary { text, failed: failing.is_some_and(|count| count > 0) });
    }
    let ok = lines.iter().filter(|line| line.starts_with("ok ") || line.starts_with("ok\t")).count();
    let failed = lines.iter().filter(|line| line.starts_with("FAIL\t") || line.starts_with("FAIL ")).count();
    if ok > 0 || failed > 0 {
        let packages = |count: usize| if count == 1 { "1 package".to_string() } else { format!("{count} packages") };
        let text = if failed > 0 { format!("{} failed, {} ok", packages(failed), ok) } else { format!("{} ok", packages(ok)) };
        return Some(Summary { text, failed: failed > 0 });
    }
    None
}

pub fn file(state: &Path, address: &str) -> PathBuf {
    state.join("evidence").join(format!("{address}.jsonl"))
}

pub fn runs_in(state: &Path, address: &str) -> Vec<Run> {
    fs::read_to_string(file(state, address)).unwrap_or_default().lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok()).filter_map(|value| Run::from_json(&value)).collect()
}

pub fn save(state: &Path, address: &str, run: &Run) -> Result<()> {
    let mut runs = runs_in(state, address);
    runs.push(run.clone());
    let start = runs.len().saturating_sub(KEPT);
    let text: String = runs[start..].iter().map(|run| format!("{}\n", run.to_json())).collect();
    crate::state::write(&file(state, address), text)
}

pub fn forget(task: &str, repo: &Path) {
    if let Ok(state) = state_dir() {
        let _ = fs::remove_file(file(&state, &peers::address(task, &name_of(repo))));
    }
}

fn git_with(cwd: &Path, index: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(cwd).env("GIT_INDEX_FILE", index)
        .env("GIT_AUTHOR_NAME", "kelpie").env("GIT_AUTHOR_EMAIL", "kelpie@localhost").env("GIT_COMMITTER_NAME", "kelpie").env("GIT_COMMITTER_EMAIL", "kelpie@localhost")
        .stdin(Stdio::null()).output().map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// HEAD, whether the tree had changes, and a commit of the tree as it was, so tests run before committing count for the commit that holds the same code.
pub fn snapshot(worktree: &Path) -> Result<(String, bool, String)> {
    let head = git(worktree, &["rev-parse", "HEAD"])?;
    let dirty = !is_clean(worktree)?;
    if !dirty {
        return Ok((head.clone(), false, head));
    }
    let real = PathBuf::from(git(worktree, &["rev-parse", "--path-format=absolute", "--git-path", "index"])?);
    let index = real.with_file_name(format!("kelpie-evidence-{}.index", std::process::id()));
    let made = fs::copy(&real, &index).map_err(|error| format!("{}: {error}", index.display()))
        .and_then(|_| git_with(worktree, &index, &["add", "-A"]))
        .and_then(|_| git_with(worktree, &index, &["write-tree"]))
        .and_then(|tree| git_with(worktree, &index, &["commit-tree", &tree, "-p", &head, "-m", "kelpie: the tree a test run ran on"]));
    let _ = fs::remove_file(&index);
    Ok((head, true, made?))
}

// From a PostToolUse or PostToolUseFailure payload: the command and how it ended, or None when it is no test run or did not run to its end.
pub fn outcome(input: &Value, listed: &[String]) -> Option<(String, String, Option<i64>, bool, String)> {
    if input["tool_name"] != "Bash" || input["tool_input"]["run_in_background"] == true {
        return None;
    }
    let command = input["tool_input"]["command"].as_str()?;
    let runner = recognise(command, listed)?.to_string();
    match input["hook_event_name"].as_str()? {
        "PostToolUse" => {
            let response = &input["tool_response"];
            if response["interrupted"] == true {
                return None;
            }
            let output = match response {
                Value::String(text) => text.clone(),
                _ => format!("{}\n{}", response["stdout"].as_str().unwrap_or_default(), response["stderr"].as_str().unwrap_or_default()),
            };
            Some((command.to_string(), runner, Some(0), true, output))
        }
        "PostToolUseFailure" if input["is_interrupt"] != true => {
            let error = input["error"].as_str().unwrap_or_default();
            let exit = error.lines().next().and_then(|line| line.strip_prefix("Exit code ")).and_then(|code| code.trim().parse().ok());
            Some((command.to_string(), runner, exit, false, error.to_string()))
        }
        _ => None,
    }
}

// The run a hook payload describes, with the worktree's state at that moment.
pub fn record_in(state: &Path, address: &str, worktree: &Path, listed: &[String], input: &Value, at: u64) -> Option<Run> {
    let (command, runner, exit, finished, output) = outcome(input, listed)?;
    let summary = summary(&output);
    let ok = finished && !summary.as_ref().is_some_and(|summary| summary.failed);
    let (sha, dirty, code) = snapshot(worktree).ok()?;
    let command = if command.chars().count() > COMMAND_LIMIT { format!("{}…", command.chars().take(COMMAND_LIMIT).collect::<String>()) } else { command };
    let run = Run { command, runner, ok, exit, duration_ms: input["duration_ms"].as_u64(), summary: summary.map(|summary| summary.text).unwrap_or_default(), sha, dirty, code, at };
    save(state, address, &run).ok()?;
    Some(run)
}

// The PostToolUse and PostToolUseFailure hooks on a task's Bash; it prints nothing, so Claude sees no change.
pub fn hook(address: &str, input: &str) -> String {
    let input: Value = serde_json::from_str(input).unwrap_or_default();
    let Some(cwd) = input["cwd"].as_str().map(PathBuf::from) else { return String::new() };
    let listed = commands(&crate::config::resolve(Some(&cwd)).map(|loaded| loaded.settings.test_commands.value).unwrap_or_default());
    if outcome(&input, &listed).is_none() {
        return String::new();
    }
    let Ok(top) = git(&cwd, &["rev-parse", "--show-toplevel"]) else { return String::new() };
    if peers::me(Path::new(&top)).as_deref() != Some(address) {
        return String::new();
    }
    if let Ok(state) = state_dir() {
        record_in(&state, address, Path::new(&top), &listed, &input, crate::messages::now());
    }
    String::new()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Covered,
    Stale,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub mark: Mark,
    pub text: String,
}

impl Shown {
    pub fn symbol(&self) -> &'static str {
        match self.mark {
            Mark::Covered => "✓",
            Mark::Stale => "○",
            Mark::Failed => "✗",
        }
    }

    pub fn line(&self) -> String {
        format!("{} {}", self.symbol(), self.text)
    }
}

// The latest run against the code now: covered when it passed on the same code with a clean tree, failed whatever the code since.
pub fn shown(run: &Run, current: impl Fn(&str) -> bool, clean: bool) -> Shown {
    let counts = if run.summary.is_empty() { String::new() } else { format!(" · {}", run.summary) };
    if !run.ok {
        return Shown { mark: Mark::Failed, text: format!("{} failed{counts} · at {}", run.runner, run.short()) };
    }
    if clean && current(&run.code) {
        Shown { mark: Mark::Covered, text: format!("{}{counts} · at {}", run.runner, run.short()) }
    } else {
        Shown { mark: Mark::Stale, text: format!("{}{counts} · at {}, code changed since", run.runner, run.short()) }
    }
}

struct Task {
    path: PathBuf,
    upstream: String,
    address: String,
}

fn task_at(cwd: &Path, name: Option<&str>) -> Result<Task> {
    let repo = main_checkout(cwd)?;
    let slot = tasks::find_task(&repo, name, cwd)?;
    Ok(Task { upstream: default_branch(&repo)?, address: peers::address(tasks::task_of(&slot), &name_of(&repo)), path: slot.path })
}

impl Task {
    fn judge(&self, run: &Run) -> Shown {
        let head = git(&self.path, &["rev-parse", "HEAD"]).unwrap_or_default();
        shown(run, |code| tasks::same_code(&self.path, &self.upstream, code, &head), is_clean(&self.path).unwrap_or(false))
    }
}

// The line the board and the push dialog show for the task whose worktree this is.
pub fn for_path(path: &Path) -> Option<Shown> {
    let task = task_at(path, None).ok()?;
    let run = runs_in(&state_dir().ok()?, &task.address).pop()?;
    Some(task.judge(&run))
}

// The reviewer's brief: the newest runs, each judged against the code under review.
pub fn brief(runs: &[Run], judge: impl Fn(&Run) -> Shown) -> String {
    if runs.is_empty() {
        return "kelpie recorded no test run in this task: say whether the change needed one.".to_string();
    }
    let mut lines = vec!["The test runs kelpie recorded in this task, newest first (✓ passed on the code under review, ○ passed on older code, ✗ failed); weigh them in the Tests step:".to_string()];
    for run in runs.iter().rev().take(5) {
        lines.push(format!("- {} · `{}`{}", judge(run).line(), run.command, if run.dirty { " · with uncommitted changes" } else { "" }));
    }
    lines.join("\n")
}

// The reviewer reads the commits, not the tree, so uncommitted work does not make a run stale here.
pub fn brief_for(state: &Path, address: &str, worktree: &Path, upstream: &str, sha: &str) -> String {
    brief(&runs_in(state, address), |run| shown(run, |code| tasks::same_code(worktree, upstream, code, sha), true))
}

fn duration(ms: u64) -> String {
    if ms < 60_000 { format!("{:.1}s", ms as f64 / 1000.0) } else { format!("{}m {}s", ms / 60_000, ms / 1000 % 60) }
}

pub fn print(cwd: &Path, name: Option<&str>, as_json: bool) -> Result<String> {
    let task = task_at(cwd, name)?;
    let runs = runs_in(&state_dir()?, &task.address);
    if as_json {
        let items: Vec<Value> = runs.iter().rev().map(|run| {
            let mut value = run.to_json();
            let judged = task.judge(run);
            value["shown"] = json!(judged.line());
            value["current"] = json!(judged.mark == Mark::Covered);
            value
        }).collect();
        return serde_json::to_string_pretty(&json!({ "task": task.address, "runs": items })).map_err(|error| error.to_string());
    }
    if runs.is_empty() {
        return Ok(format!("No test run recorded for {}: kelpie records the test commands its Claude runs (task.test_commands adds to the built-in list).", task.address));
    }
    let mut lines = vec![format!("Test runs of {}, newest first:", task.address)];
    for run in runs.iter().rev() {
        let mut facts = vec![crate::board::widgets::ago(run.at)];
        facts.extend(run.duration_ms.map(duration));
        facts.extend(run.exit.map(|exit| format!("exit {exit}")));
        if run.dirty {
            facts.push("uncommitted changes".to_string());
        }
        lines.push(format!("{}\n    {} ({})", task.judge(run).line(), run.command, facts.join(" · ")));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(extra: &[&str]) -> Vec<String> {
        commands(&extra.iter().map(|command| command.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn a_test_command_is_known_by_its_start_after_cd_and_environment_assignments() {
        let list = listed(&[]);
        assert_eq!(recognise("cargo test", &list), Some("cargo test"));
        assert_eq!(recognise("cargo test --lib evidence 2>&1 | tail -20", &list), Some("cargo test"));
        assert_eq!(recognise("cd /src/wt1.app && RAILS_ENV=test bin/rails test test/models", &list), Some("bin/rails test"));
        assert_eq!(recognise("cd a && cd b && FOO='a b' BAR=\"c d\" pytest -q", &list), Some("pytest"));
        assert_eq!(recognise("env CI=1 npm test", &list), Some("npm test"));
        assert_eq!(recognise("uv run pytest tests/", &list), Some("uv run pytest"), "the longest listed command wins");
        assert_eq!(recognise("bundle exec rspec spec/models", &list), Some("bundle exec rspec"));
        assert_eq!(recognise("cargo testify", &list), None, "whole words only");
        assert_eq!(recognise("cargo build && cargo test", &list), None, "only the command's start counts");
        assert_eq!(recognise("echo cargo test", &list), None);
        assert_eq!(recognise("bin/test unit", &list), None);
        assert_eq!(recognise("bin/test unit", &listed(&["bin/test"])), Some("bin/test"), "a repo's own runner is added to the list");
        assert_eq!(recognise("make check", &listed(&["make check", ""])), Some("make check"));
    }

    #[test]
    fn each_runner_gives_its_result_line() {
        let cases = [
            ("running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured\n\nrunning 490 tests\ntest result: ok. 490 passed; 0 failed; 1 ignored", "493 passed", false),
            ("test result: FAILED. 488 passed; 2 failed; 0 ignored", "488 passed, 2 failed", true),
            ("\u{1b}[32mtest result: ok\u{1b}[0m. 5 passed; 0 failed", "5 passed", false),
            ("     Summary [  12.302s] 493 tests run: 493 passed, 2 skipped", "493 passed, 2 skipped", false),
            ("     Summary [   1.0s] 10 tests run: 9 passed, 1 failed, 0 skipped", "9 passed, 1 failed, 0 skipped", true),
            ("Finished in 0.5 seconds\n12 examples, 0 failures, 2 pending", "12 examples, 0 failures, 2 pending", false),
            ("12 examples, 1 failure", "12 examples, 1 failure", true),
            ("Finished in 1.2s\n40 runs, 98 assertions, 0 failures, 0 errors, 1 skips", "40 runs, 98 assertions, 0 failures, 0 errors, 1 skips", false),
            ("40 runs, 98 assertions, 0 failures, 2 errors, 0 skips", "40 runs, 98 assertions, 0 failures, 2 errors, 0 skips", true),
            ("============ 12 passed, 1 skipped in 0.34s ============", "12 passed, 1 skipped in 0.34s", false),
            ("=== 1 failed, 11 passed in 2.01s ===", "1 failed, 11 passed in 2.01s", true),
            ("Test Suites: 3 passed, 3 total\nTests:       1 failed, 11 passed, 12 total", "1 failed, 11 passed, 12 total", true),
            (" Test Files  4 passed (4)\n      Tests  12 passed (12)", "12 passed (12)", false),
            ("  12 passing (40ms)\n  1 failing", "12 passing, 1 failing", true),
            ("ok  \texample.com/a\t0.012s\n?   \texample.com/b\t[no test files]\nok  \texample.com/c\t0.3s", "2 packages ok", false),
            ("--- FAIL: TestX (0.00s)\nFAIL\nFAIL\texample.com/a\t0.01s\nok  \texample.com/c\t0.3s", "1 package failed, 1 ok", true),
            ("Finished in 0.1 seconds\n1 doctest, 12 tests, 0 failures", "1 doctest, 12 tests, 0 failures", false),
        ];
        for (output, text, failed) in cases {
            assert_eq!(summary(output), Some(Summary { text: text.to_string(), failed }), "{output}");
        }
        assert_eq!(summary("Compiling kelpie\nerror[E0425]: cannot find value"), None);
    }

    fn repo() -> PathBuf {
        let dir = crate::sh::test_dir("evidence");
        let run = |args: &[&str]| git(&dir, &[&["-c", "user.name=Ada", "-c", "user.email=ada@example.com"], args].concat()).unwrap();
        run(&["init", "--quiet", "--initial-branch=main"]);
        fs::write(dir.join("a"), "one\n").unwrap();
        run(&["add", "a"]);
        run(&["commit", "--quiet", "-m", "first"]);
        run(&["checkout", "--quiet", "-b", "billing"]);
        dir
    }

    fn commit_all(dir: &Path, message: &str) {
        git(dir, &["add", "-A"]).unwrap();
        git(dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", message]).unwrap();
    }

    fn judge(dir: &Path, run: &Run) -> Shown {
        let head = git(dir, &["rev-parse", "HEAD"]).unwrap();
        shown(run, |code| tasks::same_code(dir, "main", code, &head), is_clean(dir).unwrap())
    }

    // The payload shapes of https://code.claude.com/docs/en/hooks, PostToolUse input and PostToolUseFailure input.
    fn passed(command: &str) -> Value {
        json!({
            "session_id": "abc123", "transcript_path": "/t.jsonl", "cwd": "/src", "permission_mode": "default", "hook_event_name": "PostToolUse",
            "tool_name": "Bash", "tool_input": { "command": command, "description": "Run test suite" },
            "tool_response": { "stdout": "running 2 tests\ntest result: ok. 2 passed; 0 failed; 0 ignored", "stderr": "", "interrupted": false, "isImage": false },
            "tool_use_id": "toolu_01ABC123", "duration_ms": 4187,
        })
    }

    fn failed(command: &str) -> Value {
        json!({
            "session_id": "abc123", "transcript_path": "/t.jsonl", "cwd": "/src", "permission_mode": "default", "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash", "tool_input": { "command": command, "description": "Run test suite" },
            "tool_use_id": "toolu_01ABC123", "error": "Exit code 1\nError: Cannot find module 'express'", "is_interrupt": false, "duration_ms": 4187,
        })
    }

    #[test]
    fn the_hook_records_a_test_run_from_the_payload_and_skips_other_commands() {
        let dir = repo();
        let state = dir.join("state");
        let list = listed(&[]);
        let run = record_in(&state, "billing@app", &dir, &list, &passed("cargo test"), 100).unwrap();
        let head = git(&dir, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(run, Run { command: "cargo test".to_string(), runner: "cargo test".to_string(), ok: true, exit: Some(0), duration_ms: Some(4187), summary: "2 passed".to_string(), sha: head.clone(), dirty: false, code: head, at: 100 });
        assert_eq!(runs_in(&state, "billing@app"), [run]);
        let broke = record_in(&state, "billing@app", &dir, &list, &failed("npm test"), 200).unwrap();
        assert!(!broke.ok && broke.exit == Some(1) && broke.runner == "npm test");
        assert!(record_in(&state, "billing@app", &dir, &list, &passed("ls -la"), 300).is_none());
        let mut background = passed("cargo test");
        background["tool_input"]["run_in_background"] = json!(true);
        assert!(record_in(&state, "billing@app", &dir, &list, &background, 300).is_none(), "a background run has no result yet");
        let mut interrupted = failed("cargo test");
        interrupted["is_interrupt"] = json!(true);
        assert!(record_in(&state, "billing@app", &dir, &list, &interrupted, 300).is_none());
        let mut piped = passed("cargo test 2>&1 | tail -3");
        piped["tool_response"]["stdout"] = json!("test result: FAILED. 1 passed; 1 failed; 0 ignored");
        assert!(!record_in(&state, "billing@app", &dir, &list, &piped, 400).unwrap().ok, "a pipe hides the exit code, so the result line decides");
        let latest = runs_in(&state, "billing@app").pop().unwrap();
        for at in 0..30 {
            save(&state, "billing@app", &Run { at: 1000 + at, ..latest.clone() }).unwrap();
        }
        let kept = runs_in(&state, "billing@app");
        assert_eq!(kept.len(), KEPT);
        assert_eq!(kept.last().unwrap().at, 1029);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_run_is_current_until_the_code_changes_and_a_run_before_the_commit_counts_for_it() {
        let dir = repo();
        let state = std::env::temp_dir().join(format!("kelpie-evidence-state-{}", std::process::id()));
        let list = listed(&[]);
        fs::write(dir.join("a"), "two\n").unwrap();
        let before = record_in(&state, "billing@app", &dir, &list, &passed("cargo test"), 1).unwrap();
        assert!(before.dirty);
        assert_eq!(judge(&dir, &before).mark, Mark::Stale, "the change is not committed yet");
        commit_all(&dir, "second");
        assert_eq!(judge(&dir, &before).line(), format!("✓ cargo test · 2 passed · at {}", &before.sha[..7]), "the commit holds the code the tests ran on");
        fs::write(dir.join("a"), "three\n").unwrap();
        assert_eq!(judge(&dir, &before).mark, Mark::Stale, "a dirty tree is not the code that ran");
        commit_all(&dir, "third");
        let stale = judge(&dir, &before);
        assert_eq!(stale.line(), format!("○ cargo test · 2 passed · at {}, code changed since", &before.sha[..7]));
        let clean = record_in(&state, "billing@app", &dir, &list, &passed("cargo test"), 2).unwrap();
        assert!(!clean.dirty && clean.code == clean.sha);
        assert_eq!(judge(&dir, &clean).mark, Mark::Covered);
        let broke = record_in(&state, "billing@app", &dir, &list, &failed("cargo test"), 3).unwrap();
        assert_eq!(judge(&dir, &broke).line(), format!("✗ cargo test failed · at {}", &broke.sha[..7]));
        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_reviewer_reads_the_newest_runs_judged_against_the_code_under_review() {
        let run = |at: u64, ok| Run { command: "cargo test --lib".to_string(), runner: "cargo test".to_string(), ok, exit: Some(0), duration_ms: None, summary: "3 passed".to_string(), sha: format!("{at}abcdef0123"), dirty: at == 2, code: at.to_string(), at };
        let text = brief(&[run(1, false), run(2, true)], |run| shown(run, |code| code == "2", true));
        assert!(text.contains("weigh them in the Tests step"), "{text}");
        let newest = text.find("✓ cargo test · 3 passed · at 2abcdef · `cargo test --lib` · with uncommitted changes").unwrap();
        let oldest = text.find("✗ cargo test failed · 3 passed · at 1abcdef · `cargo test --lib`").unwrap();
        assert!(newest < oldest, "{text}");
        assert!(brief(&[], |run| shown(run, |_| true, true)).contains("recorded no test run"));
    }
}
