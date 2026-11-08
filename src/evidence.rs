use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::peers;
use crate::repo::{default_branch, git, is_clean, main_checkout, name_of};
use crate::sh::Result;
use crate::tasks;

mod shell;

pub const DEFAULT_COMMANDS: [&str; 16] = [
    "cargo test", "cargo nextest", "bin/rails test", "bundle exec rails test", "bundle exec rspec", "bin/rspec", "rspec",
    "pytest", "python -m pytest", "uv run pytest", "npm test", "npm run test", "pnpm test", "yarn test", "go test", "mix test",
];

const KEPT: usize = 20;
const COMMAND_LIMIT: usize = 300;
const UNRECORDED_KEPT: usize = 3;
const OLDER_SHOWN: usize = 5;
const RUNNER_WORDS: [&str; 4] = ["test", "tests", "spec", "specs"];
const TASK_RUNNERS: [&str; 22] = ["make", "just", "rake", "npm", "pnpm", "yarn", "bun", "deno", "dotnet", "mvn", "gradle", "gradlew", "swift", "zig", "flutter", "dart", "mix", "go", "nx", "turbo", "task", "bundle"];
const TEST_TOOLS: [&str; 8] = ["jest", "vitest", "mocha", "phpunit", "ctest", "tox", "nox", "bats"];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Outcome {
    Passed,
    Failed,
    #[default]
    Unknown,
}

impl Outcome {
    fn name(self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Failed => "failed",
            Outcome::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub command: String,
    pub runner: String,
    pub result: Outcome,
    pub why: String,
    pub exit: Option<i64>,
    pub duration_ms: Option<u64>,
    pub summary: String,
    pub sha: String,
    pub dirty: bool,
    pub code: String,
    pub at: u64,
    pub background: String,
    pub transcript: String,
    pub says: Option<Exit>,
    pub worktree: String,
    pub read_to: u64,
}

impl Run {
    pub fn to_json(&self) -> Value {
        json!({
            "command": self.command, "runner": self.runner, "result": self.result.name(), "ok": self.result == Outcome::Passed, "why": self.why, "exit": self.exit, "duration_ms": self.duration_ms,
            "summary": self.summary, "sha": self.sha, "dirty": self.dirty, "code": self.code, "at": self.at,
            "background": self.background, "transcript": self.transcript, "says": self.says.map(Exit::key), "worktree": self.worktree, "read_to": self.read_to,
        })
    }

    fn from_json(value: &Value) -> Option<Run> {
        let result = match value["result"].as_str() {
            Some("passed") => Outcome::Passed,
            Some("failed") => Outcome::Failed,
            Some(_) => Outcome::Unknown,
            None => if value["ok"].as_bool()? { Outcome::Passed } else { Outcome::Failed },
        };
        Some(Run {
            command: value["command"].as_str()?.to_string(),
            runner: value["runner"].as_str()?.to_string(),
            result,
            why: value["why"].as_str().unwrap_or_default().to_string(),
            exit: value["exit"].as_i64(),
            duration_ms: value["duration_ms"].as_u64(),
            summary: value["summary"].as_str().unwrap_or_default().to_string(),
            sha: value["sha"].as_str()?.to_string(),
            dirty: value["dirty"].as_bool().unwrap_or(false),
            code: value["code"].as_str()?.to_string(),
            at: value["at"].as_u64().unwrap_or(0),
            background: value["background"].as_str().unwrap_or_default().to_string(),
            transcript: value["transcript"].as_str().unwrap_or_default().to_string(),
            says: value["says"].as_str().map(Exit::from_key),
            worktree: value["worktree"].as_str().unwrap_or_default().to_string(),
            read_to: value["read_to"].as_u64().unwrap_or(0),
        })
    }

    fn short(&self) -> String {
        self.sha.chars().take(7).collect()
    }
}

pub fn commands(configured: &[String]) -> Vec<String> {
    DEFAULT_COMMANDS.iter().map(|command| command.to_string()).chain(configured.iter().cloned()).collect()
}

// The longest listed command a simple command starts with, whole words only.
fn listed_start<'a>(simple: &str, listed: &'a [String]) -> Option<&'a str> {
    let core = simple.trim_start_matches("./");
    listed.iter().map(|prefix| prefix.trim()).filter(|prefix| !prefix.is_empty())
        .filter(|prefix| { let bare = prefix.trim_start_matches("./"); core == bare || core.strip_prefix(bare).is_some_and(|rest| rest.starts_with(char::is_whitespace)) })
        .max_by_key(|prefix| prefix.len())
}

// What the line's exit says about the test command in it, from least to most.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub enum Exit {
    Silent(&'static str),
    ZeroOnly,
    Speaks,
}

impl Exit {
    fn least(self, other: Exit) -> Exit {
        if other < self { other } else { self }
    }

    fn key(self) -> &'static str {
        match self {
            Exit::Silent(why) => why,
            Exit::ZeroOnly => "zero only",
            Exit::Speaks => "speaks",
        }
    }

    fn from_key(key: &str) -> Exit {
        match key {
            "speaks" => Exit::Speaks,
            "zero only" => Exit::ZeroOnly,
            PIPED => Exit::Silent(PIPED),
            _ => Exit::Silent(OTHER),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Found<'a> {
    pub runner: &'a str,
    pub exit: Exit,
}

const PIPED: &str = "piped, so the exit is the pipe's";
const OTHER: &str = "the exit is another command's";

// What the exit of the line says about the command at `at`: whole when nothing runs after it, only a zero after &&, nothing after a pipe without pipefail or after ; || &.
fn exit_of(pieces: &[(&str, Option<shell::Op>)], at: usize, errexit: bool, pipefail: bool) -> Exit {
    use shell::Op;
    let mut exit = Exit::Speaks;
    for (text, op) in pieces[..at].iter().rev() {
        match op {
            Some(Op::Seq) if !errexit => break,
            Some(Op::Background) => break,
            Some(Op::Or) => return Exit::Silent(OTHER),
            Some(Op::Pipe) if pipefail => exit = exit.least(Exit::ZeroOnly),
            Some(Op::Seq | Op::And) if shell::piece(text) != shell::Piece::Setup => exit = exit.least(Exit::ZeroOnly),
            _ => {}
        }
    }
    let mut after = pieces[at..].iter().map(|(_, op)| *op).skip_while(|op| *op == Some(Op::Pipe));
    if pieces[at].1 == Some(Op::Pipe) {
        exit = if pipefail { exit.least(Exit::ZeroOnly) } else { return Exit::Silent(PIPED) };
    }
    for op in after.by_ref() {
        match op {
            None => break,
            Some(Op::And | Op::Pipe) => exit = exit.least(Exit::ZeroOnly),
            Some(Op::Seq) if errexit => exit = exit.least(Exit::ZeroOnly),
            Some(Op::Seq | Op::Or | Op::Background) => return Exit::Silent(OTHER),
        }
    }
    exit
}

fn sets(text: &str, flag: char, option: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    words.first() == Some(&"set") && (words.contains(&option) || words.iter().any(|word| word.starts_with('-') && !word.starts_with("--") && word.contains(flag)))
}

// The first listed command the line runs as a command of its own, after cd, assignments, wrappers and any earlier command of a ; && || chain, with what the line's exit says about it.
pub fn find<'a>(command: &str, listed: &'a [String]) -> Option<Found<'a>> {
    let pieces = shell::pieces(command);
    let (mut errexit, mut pipefail) = (false, false);
    for (at, (text, _)) in pieces.iter().enumerate() {
        let found = match shell::piece(text) {
            shell::Piece::Simple(simple) => listed_start(simple, listed).map(|runner| Found { runner, exit: Exit::Speaks }),
            shell::Piece::Nested(inner) => find(&inner, listed),
            shell::Piece::Setup => {
                let text = shell::unwrap(text);
                errexit |= sets(text, 'e', "errexit");
                pipefail |= sets(text, 'o', "pipefail") && text.contains("pipefail");
                None
            }
            shell::Piece::Hidden(..) => None,
        };
        if let Some(found) = found {
            return Some(Found { exit: found.exit.least(exit_of(&pieces, at, errexit, pipefail)), ..found });
        }
    }
    None
}

pub fn recognise<'a>(command: &str, listed: &'a [String]) -> Option<&'a str> {
    find(command, listed).map(|found| found.runner)
}

// The words of a simple command up to the first redirection.
fn words(simple: &str) -> Vec<&str> {
    simple.split_whitespace().take_while(|word| !word.starts_with(['>', '<']) && !word.contains(">&") && !word.starts_with(|char: char| char.is_ascii_digit() && word.contains('>'))).collect()
}

fn runner_word(word: &str) -> bool {
    RUNNER_WORDS.contains(&word) || word.starts_with("test:") || word.starts_with("spec:")
}

// The start of a simple command that looks like a test run, kept conservative: a test script by path (script/test, bin/spec), a test tool, or a task runner given a test word (make test, npm run test).
fn test_like(simple: &str) -> Option<String> {
    let words = words(simple);
    let first = words.first()?.trim_start_matches("./");
    let name = first.rsplit('/').next().unwrap_or(first);
    if first.contains('/') && (RUNNER_WORDS.contains(&name) || name == "run-tests" || name == "run_tests") {
        return Some(words[0].to_string());
    }
    if TEST_TOOLS.contains(&name) || TEST_TOOLS.iter().any(|tool| first == format!("node_modules/.bin/{tool}")) {
        return Some(words[0].to_string());
    }
    if !TASK_RUNNERS.contains(&name) {
        return None;
    }
    let mut taken = 1;
    if words.get(1).is_some_and(|word| ["run", "exec", "run-script"].contains(word)) {
        taken = 2;
    }
    if name == "bundle" && words.get(2).is_some_and(|word| *word == "rake") {
        taken = 3;
    }
    words.get(taken).filter(|word| runner_word(word)).map(|_| words[..=taken].join(" "))
}

// A command in the line that looks like a test run kebbie will not record: one the list leaves out, or a listed one inside a loop, a condition or xargs.
pub fn unrecorded(command: &str, listed: &[String]) -> Option<String> {
    shell::pieces(command).into_iter().find_map(|(text, _)| match shell::piece(text) {
        shell::Piece::Simple(simple) => test_like(simple).filter(|_| listed_start(simple, listed).is_none()),
        shell::Piece::Nested(inner) => unrecorded(&inner, listed),
        shell::Piece::Hidden(keyword, simple) => {
            let simple = shell::unwrap(simple);
            listed_start(simple, listed).map(str::to_string).or_else(|| test_like(simple)).map(|start| format!("{keyword} {start}"))
        }
        shell::Piece::Setup => None,
    })
}

fn hidden(entry: &str) -> bool {
    matches!(shell::piece(entry), shell::Piece::Hidden(..))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub text: String,
    pub failed: bool,
    pub ran_none: bool,
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
    let zero = |noun: &str| count(trimmed, noun) == Some(0);
    if let Some(rest) = trimmed.strip_prefix("Summary [").and_then(|rest| rest.split_once(']')).map(|(_, rest)| rest.trim()) {
        let text = rest.split_once(": ").map_or(rest, |(_, counts)| counts).to_string();
        return Some(Summary { failed: nonzero("failed") || nonzero("timed"), ran_none: zero("test"), text });
    }
    if count(trimmed, "example").is_some() && count(trimmed, "failure").is_some() {
        return Some(Summary { failed: nonzero("failure") || nonzero("error"), ran_none: zero("example"), text: trimmed.to_string() });
    }
    if count(trimmed, "run").is_some() && count(trimmed, "assertion").is_some() {
        return Some(Summary { failed: nonzero("failure") || nonzero("error"), ran_none: zero("run"), text: trimmed.to_string() });
    }
    if count(trimmed, "test").is_some() && count(trimmed, "failure").is_some() {
        return Some(Summary { failed: nonzero("failure"), ran_none: zero("test"), text: trimmed.to_string() });
    }
    if trimmed.starts_with('=') && trimmed.ends_with('=') && trimmed.contains(" in ") {
        let text = trimmed.trim_matches(|char| char == '=' || char == ' ').to_string();
        if ["passed", "failed", "error", "no tests ran"].iter().any(|word| text.contains(word)) {
            return Some(Summary { failed: text.contains("failed") || text.contains("error"), ran_none: text.contains("no tests ran"), text });
        }
    }
    if let Some(rest) = trimmed.strip_prefix("Tests:").or_else(|| trimmed.strip_prefix("Tests ")) {
        let text = rest.trim().to_string();
        if text.contains("passed") || text.contains("failed") {
            return Some(Summary { failed: nonzero("failed"), ran_none: !nonzero("passed") && !nonzero("failed"), text });
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
        return Some(Summary { text, failed: broke, ran_none: passed == 0 && failed == 0 });
    }
    if let Some(failed) = lines.iter().rev().find_map(|line| match line.trim() { "** TEST FAILED **" => Some(true), "** TEST SUCCEEDED **" => Some(false), _ => None }) {
        let counts = lines.iter().rev().map(|line| line.trim()).find(|line| line.starts_with("Executed ") && count(line, "test").is_some());
        let text = counts.map_or(if failed { "TEST FAILED" } else { "TEST SUCCEEDED" }, |line| line.split(" in ").next().unwrap_or(line));
        return Some(Summary { text: text.to_string(), failed, ran_none: counts.is_some_and(|line| count(line, "test") == Some(0)) });
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
        return Some(Summary { text, failed: failing.is_some_and(|count| count > 0), ran_none: passing.unwrap_or(0) == 0 && failing.unwrap_or(0) == 0 });
    }
    let ok = lines.iter().filter(|line| line.starts_with("ok ") || line.starts_with("ok\t")).count();
    let failed = lines.iter().filter(|line| line.starts_with("FAIL\t") || line.starts_with("FAIL ")).count();
    if ok > 0 || failed > 0 {
        let packages = |count: usize| if count == 1 { "1 package".to_string() } else { format!("{count} packages") };
        let text = if failed > 0 { format!("{} failed, {} ok", packages(failed), ok) } else { format!("{} ok", packages(ok)) };
        return Some(Summary { text, failed: failed > 0, ran_none: false });
    }
    None
}

pub fn file(state: &Path, address: &str) -> PathBuf {
    state.join("evidence").join(format!("{address}.jsonl"))
}

fn hook_file(state: &Path, address: &str) -> PathBuf {
    state.join("evidence").join(format!("{address}.hook.json"))
}

// What the hook saw besides test runs: when it last ran, and commands that looked like test runs but are not in the list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    pub at: Option<u64>,
    pub unrecorded: Vec<String>,
}

pub fn seen_in(state: &Path, address: &str) -> Seen {
    let value: Value = fs::read_to_string(hook_file(state, address)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    Seen { at: value["at"].as_u64(), unrecorded: value["unrecorded"].as_array().into_iter().flatten().filter_map(|command| command.as_str().map(str::to_string)).collect() }
}

fn note_seen(state: &Path, address: &str, unrecorded: Option<String>, at: u64) {
    let mut seen = seen_in(state, address);
    seen.at = Some(at);
    if let Some(command) = unrecorded.map(|command| command.trim_start_matches("./").to_string()) {
        seen.unrecorded.retain(|known| *known != command);
        seen.unrecorded.push(command);
        let start = seen.unrecorded.len().saturating_sub(UNRECORDED_KEPT);
        seen.unrecorded.drain(..start);
    }
    let _ = crate::state::write(&hook_file(state, address), json!({ "at": seen.at, "unrecorded": seen.unrecorded }).to_string());
}

// The newest test-like command a task ran that kebbie still leaves out, as the board says it.
pub fn unrecorded_note(seen: &Seen, listed: &[String]) -> Option<String> {
    let command = seen.unrecorded.iter().rev().find(|command| recognise(command, listed).is_none())?;
    Some(if hidden(command) {
        format!("{command} is not recorded as a test run: kebbie cannot tell its result inside a loop, a condition or xargs")
    } else {
        format!("{command} is not recorded as a test run: add it to task.test_commands")
    })
}

// What the hook tells the task's Claude about a test-like command it did not record.
fn unrecorded_context(command: &str) -> String {
    if hidden(command) {
        format!("kebbie did not record `{command}` as a test run: it cannot tell the result of a test command inside a loop, a condition or xargs. Run the test command as a command of its own to have its result recorded.")
    } else {
        format!("kebbie did not record `{command}` as a test run: it is not in task.test_commands. Name it in your report, so the user can add it.")
    }
}

pub fn unrecorded_for(state: &Path, address: &str, repo: &Path) -> Option<String> {
    let configured = crate::config::resolve(Some(repo)).map(|loaded| loaded.settings.test_commands.value).unwrap_or_default();
    unrecorded_note(&seen_in(state, address), &commands(&configured))
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
        let address = peers::address(task, &name_of(repo));
        let _ = fs::remove_file(file(&state, &address));
        let _ = fs::remove_file(hook_file(&state, &address));
    }
}

fn git_with(cwd: &Path, index: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(crate::repo::unhooked(Some(cwd), args)).current_dir(cwd).env("GIT_INDEX_FILE", index)
        .env("GIT_AUTHOR_NAME", "kebbie").env("GIT_AUTHOR_EMAIL", "kebbie@localhost").env("GIT_COMMITTER_NAME", "kebbie").env("GIT_COMMITTER_EMAIL", "kebbie@localhost")
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
    let index = real.with_file_name(format!("kebbie-evidence-{}.index", std::process::id()));
    let made = fs::copy(&real, &index).map_err(|error| format!("{}: {error}", index.display()))
        .and_then(|_| git_with(worktree, &index, &["add", "-A"]))
        .and_then(|_| git_with(worktree, &index, &["write-tree"]))
        .and_then(|tree| git_with(worktree, &index, &["commit-tree", &tree, "-p", &head, "-m", "kebbie: the tree a test run ran on"]));
    let _ = fs::remove_file(&index);
    Ok((head, true, made?))
}

pub struct Ended {
    pub command: String,
    pub runner: String,
    pub exit: Option<i64>,
    pub finished: bool,
    pub output: String,
    pub says: Exit,
    pub background: Option<String>,
}

// From a PostToolUse or PostToolUseFailure payload: the command and how it ended, or None when it is no test run or did not run to its end.
pub fn outcome(input: &Value, listed: &[String]) -> Option<Ended> {
    if input["tool_name"] != "Bash" {
        return None;
    }
    let command = input["tool_input"]["command"].as_str()?;
    let found = find(command, listed)?;
    let ended = |exit, finished, output: String, background| Some(Ended { command: command.to_string(), runner: found.runner.to_string(), exit, finished, output, says: found.exit, background });
    match input["hook_event_name"].as_str()? {
        "PostToolUse" => {
            let response = &input["tool_response"];
            if let Some(id) = response["backgroundTaskId"].as_str() {
                return ended(None, false, String::new(), Some(id.to_string()));
            }
            if response["interrupted"] == true {
                return None;
            }
            let output = match response {
                Value::String(text) => text.clone(),
                _ => format!("{}\n{}", response["stdout"].as_str().unwrap_or_default(), response["stderr"].as_str().unwrap_or_default()),
            };
            ended(Some(0), true, output, (input["tool_input"]["run_in_background"] == true).then(String::new))
        }
        "PostToolUseFailure" if input["is_interrupt"] != true => {
            let error = input["error"].as_str().unwrap_or_default();
            let exit = error.lines().next().and_then(|line| line.strip_prefix("Exit code ")).and_then(|code| code.trim().parse().ok());
            ended(exit, false, error.to_string(), None)
        }
        _ => None,
    }
}

const IN_BACKGROUND: &str = "still running in the background";
const RAN_NONE: &str = "it ran no tests";
const NEVER_ENDED: &str = "kebbie never saw it end";
const BACKGROUND_GIVE_UP: u64 = 24 * 60 * 60;

// How a background command ended, as its <task-notification> in the session's transcript says: whether it ran to its end, its exit code when given, and its output file.
#[derive(Clone, Debug, PartialEq)]
pub struct End {
    pub ended: bool,
    pub exit: Option<i64>,
    pub output: Option<PathBuf>,
}

pub fn background_end(transcript: &str, id: &str) -> Option<End> {
    let wanted = |line: &&str| line.contains("<task-notification>") && line.contains(id) && !line.contains("\"type\":\"api-request");
    for line in transcript.lines().filter(wanted) {
        let Ok(record) = serde_json::from_str::<Value>(line) else { continue };
        for notice in crate::restart::notices(&record) {
            for block in crate::restart::blocks(&notice, "task-notification") {
                let tag = |name: &str| crate::restart::blocks(block, name).first().map(|text| text.trim().to_string());
                if tag("task-id").as_deref() != Some(id) {
                    continue;
                }
                let Some(status) = tag("status") else { continue };
                let exit = tag("summary").and_then(|summary| summary.rsplit_once("(exit code ").and_then(|(_, rest)| rest.split([')', ':']).next()?.trim().parse().ok()));
                return Some(End { ended: status == "completed" || status == "failed", exit, output: tag("output-file").map(PathBuf::from) });
            }
        }
    }
    None
}

fn tail_of(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = fs::File::open(path) else { return String::new() };
    let length = file.metadata().map_or(0, |meta| meta.len());
    let _ = file.seek(SeekFrom::Start(length.saturating_sub(64 * 1024)));
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

// The run's result once its background command ended, from the exit and the output the notification names.
pub fn settled(run: &Run, end: &End, listed: &[String]) -> Run {
    let summary = end.output.as_deref().map(tail_of).and_then(|output| summary(&output));
    let says = run.says.or_else(|| find(&run.command, listed).map(|found| found.exit)).unwrap_or(Exit::Silent(OTHER));
    let (result, why) = match (end.ended, end.exit) {
        (false, _) => match summary.as_ref() { Some(summary) if summary.failed => (Outcome::Failed, ""), _ => (Outcome::Unknown, "stopped before it ended") },
        (true, None) => judged(false, Exit::Silent("ended without an exit code"), summary.as_ref()),
        (true, Some(exit)) => judged(exit == 0, says, summary.as_ref()),
    };
    Run { result, why: why.to_string(), exit: end.exit, summary: summary.map(|summary| summary.text).unwrap_or_default(), background: String::new(), transcript: String::new(), read_to: 0, ..run.clone() }
}

// The complete lines a transcript gained since byte `from`, and the byte after the last of them; a transcript shorter than `from` was replaced and is read from its start.
fn transcript_since(path: &Path, from: u64) -> Option<(String, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let from = if from > file.metadata().ok()?.len() { 0 } else { from };
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let complete = bytes.iter().rposition(|byte| *byte == b'\n').map_or(0, |at| at + 1);
    bytes.truncate(complete);
    Some((String::from_utf8_lossy(&bytes).into_owned(), from + complete as u64))
}

// Settles every run still in the background whose end the session's transcript now holds, reading only what it gained since the last look, and gives up on one that has not ended in a day; gives the runs it settled.
pub fn settle(state: &Path, address: &str, listed: &[String], now: u64) -> Vec<Run> {
    let mut runs = runs_in(state, address);
    let (mut changed, mut moved) = (Vec::new(), false);
    for run in runs.iter_mut().filter(|run| !run.background.is_empty() && !run.transcript.is_empty()) {
        if let Some((transcript, read_to)) = transcript_since(Path::new(&run.transcript), run.read_to) {
            moved |= read_to != run.read_to;
            run.read_to = read_to;
            if let Some(end) = background_end(&transcript, &run.background) {
                *run = settled(run, &end, listed);
                changed.push(run.clone());
                continue;
            }
        }
        if now.saturating_sub(run.at) >= BACKGROUND_GIVE_UP {
            *run = Run { result: Outcome::Unknown, why: NEVER_ENDED.to_string(), background: String::new(), transcript: String::new(), read_to: 0, ..run.clone() };
            changed.push(run.clone());
        }
    }
    if !changed.is_empty() || moved {
        let text: String = runs.iter().map(|run| format!("{}\n", run.to_json())).collect();
        let _ = crate::state::write(&file(state, address), text);
    }
    changed
}

// A result line that counts failures decides, one that counts no test at all proves nothing; else the exit, as far as it speaks for the test command.
fn judged(finished: bool, says: Exit, summary: Option<&Summary>) -> (Outcome, &'static str) {
    match (summary.map(|summary| summary.failed), says) {
        (Some(true), _) => (Outcome::Failed, ""),
        _ if summary.is_some_and(|summary| summary.ran_none) => (Outcome::Unknown, RAN_NONE),
        (_, Exit::Speaks) => (if finished { Outcome::Passed } else { Outcome::Failed }, ""),
        (Some(false), _) => (Outcome::Passed, ""),
        (None, Exit::ZeroOnly) if finished => (Outcome::Passed, ""),
        (None, Exit::ZeroOnly) => (Outcome::Unknown, "the exit may be another command's"),
        (None, Exit::Silent(why)) => (Outcome::Unknown, why),
    }
}

// The run a hook payload describes, with the worktree's state at that moment.
pub fn record_in(state: &Path, address: &str, worktree: &Path, listed: &[String], input: &Value, at: u64) -> Option<Run> {
    let Ended { command, runner, exit, finished, output, says, background } = outcome(input, listed)?;
    let summary = if background.is_some() { None } else { summary(&output) };
    let (result, why) = if background.is_some() { (Outcome::Unknown, IN_BACKGROUND) } else { judged(finished, says, summary.as_ref()) };
    let (sha, dirty, code) = snapshot(worktree).ok()?;
    let command = if command.chars().count() > COMMAND_LIMIT { format!("{}…", command.chars().take(COMMAND_LIMIT).collect::<String>()) } else { command };
    let transcript = if background.as_deref().is_some_and(|id| !id.is_empty()) { input["transcript_path"].as_str().unwrap_or_default().to_string() } else { String::new() };
    let run = Run { command, runner, result, why: why.to_string(), exit, duration_ms: input["duration_ms"].as_u64(), summary: summary.map(|summary| summary.text).unwrap_or_default(), sha, dirty, code, at, background: background.unwrap_or_default(), transcript, says: Some(says), worktree: worktree.to_string_lossy().into_owned(), read_to: 0 };
    save(state, address, &run).ok()?;
    Some(run)
}

fn context(event: &Value, text: &str) -> String {
    json!({ "hookSpecificOutput": { "hookEventName": event, "additionalContext": text } }).to_string()
}

// The PostToolUse and PostToolUseFailure hooks on a task's Bash; it prints only what the task must hear about a test run kebbie could not record.
pub fn hook(address: &str, input: &str) -> String {
    let input: Value = serde_json::from_str(input).unwrap_or_default();
    let Some(cwd) = input["cwd"].as_str().map(PathBuf::from) else { return String::new() };
    let settings = crate::config::resolve(Some(&cwd)).map(|loaded| loaded.settings);
    if settings.as_ref().is_ok_and(|settings| settings.steps.value) && let Ok(state) = state_dir() {
        tasks::steps_noted_after(&state, address, &input);
    }
    let listed = commands(&settings.map(|settings| settings.test_commands.value).unwrap_or_default());
    let mut said = Vec::new();
    if input["tool_name"] == "Bash" && let Ok(state) = state_dir() {
        let missed = unrecorded(input["tool_input"]["command"].as_str().unwrap_or_default(), &listed);
        said.extend(missed.as_deref().map(unrecorded_context));
        note_seen(&state, address, missed, crate::messages::now());
    }
    let mut passed = passed_in(address, &state_dir().map(|state| settle(&state, address, &listed, crate::messages::now())).unwrap_or_default());
    if let Some(run) = recorded(address, &cwd, &listed, &input) {
        match run.result {
            Outcome::Passed => passed = passed.or_else(|| passed_in(address, std::slice::from_ref(&run))),
            Outcome::Unknown => said.push(unknown_context(&run)),
            Outcome::Failed => {}
        }
    }
    if let Some(worktree) = passed && let Ok(state) = state_dir() {
        crate::review::again_for_tests(&worktree, address, &crate::status::named(&state, address));
    }
    if said.is_empty() { String::new() } else { context(&input["hook_event_name"], &said.join("\n")) }
}

// The worktree of the task under `address` that one of these runs passed in, wherever the shell that ran the hook stands now.
fn passed_in(address: &str, runs: &[Run]) -> Option<PathBuf> {
    runs.iter().filter(|run| run.result == Outcome::Passed && !run.worktree.is_empty()).map(|run| PathBuf::from(&run.worktree))
        .find(|worktree| peers::me(worktree).as_deref() == Some(address))
}

fn recorded(address: &str, cwd: &Path, listed: &[String], input: &Value) -> Option<Run> {
    outcome(input, listed)?;
    let top = git(cwd, &["rev-parse", "--show-toplevel"]).ok()?;
    if peers::me(Path::new(&top)).as_deref() != Some(address) {
        return None;
    }
    record_in(&state_dir().ok()?, address, Path::new(&top), listed, input, crate::messages::now())
}

fn unknown_context(run: &Run) -> String {
    if run.why == IN_BACKGROUND {
        let settles = if run.background.is_empty() { "kebbie cannot see how it ends" } else { "kebbie reads its result from the notice of its end at your next shell command" };
        return format!("kebbie recorded `{}` without a result: it is {IN_BACKGROUND}, and {settles}. Until then it counts as no test run: before you report, run it in the foreground, or say in your report how it ended.", run.runner);
    }
    format!("kebbie recorded `{}` without a result: {}. Run it again so its own exit ends the command (no pipe after it, or set -o pipefail first, and nothing after it), or say in your report how it ended.", run.runner, run.why)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Covered,
    Stale,
    Failed,
    Unknown,
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
            Mark::Unknown => "?",
        }
    }

    pub fn line(&self) -> String {
        format!("{} {}", self.symbol(), self.text)
    }
}

// The latest run against the code now: covered when it passed on the same code with a clean tree, failed or unknown whatever the code since.
pub fn shown(run: &Run, current: impl Fn(&str) -> bool, clean: bool) -> Shown {
    let counts = if run.summary.is_empty() { String::new() } else { format!(" · {}", run.summary) };
    match run.result {
        Outcome::Failed => return Shown { mark: Mark::Failed, text: format!("{} failed{counts} · at {}", run.runner, run.short()) },
        Outcome::Unknown => {
            let why = if run.why.is_empty() { String::new() } else { format!(", {}", run.why) };
            return Shown { mark: Mark::Unknown, text: format!("{} result unknown{why}{counts} · at {}", run.runner, run.short()) };
        }
        Outcome::Passed => {}
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

// Whether the hook that records test runs works in the task, so a missing run reads as no run only when it does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hook {
    Works,
    NeverRan,
    Broken,
}

fn with_changes(run: &Run) -> &'static str {
    if run.dirty { " · with uncommitted changes" } else { "" }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

// Every run on the reviewed code, one line per command so a task that ran its tests fifty times still reads short; older runs only when none ran on that code.
pub fn brief(runs: &[Run], on_reviewed: impl Fn(&Run) -> bool, judge: impl Fn(&Run) -> Shown, hook: Hook, unrecorded: &[String]) -> String {
    let mut lines = Vec::new();
    let reviewed: Vec<&Run> = runs.iter().rev().filter(|run| on_reviewed(run)).collect();
    if !reviewed.is_empty() {
        lines.push("The test runs kebbie recorded on the code under review, one line per command, newest first (✓ passed, ✗ failed, ? result unknown); weigh them in the Tests step:".to_string());
        let mut commands: Vec<&str> = Vec::new();
        for run in &reviewed {
            if !commands.contains(&run.command.as_str()) {
                commands.push(&run.command);
            }
        }
        for command in commands {
            let same: Vec<&&Run> = reviewed.iter().filter(|run| run.command == command).collect();
            let newest = same[0];
            let tally = |result: Outcome| same.iter().filter(|run| run.result == result).count();
            let unknown = match tally(Outcome::Unknown) { 0 => String::new(), count => format!(", {count} unknown") };
            let tally = if same.len() == 1 { String::new() } else { format!(" · {} ({} passed, {} failed{unknown}), the newest:", plural(same.len(), "run"), tally(Outcome::Passed), tally(Outcome::Failed)) };
            lines.push(format!("- `{command}`{tally} {}{}", judge(newest).line(), with_changes(newest)));
        }
        if reviewed.iter().all(|run| run.result == Outcome::Unknown) {
            lines.push("kebbie saw none of these runs end with a result it could read, so they prove neither a pass nor a fail: treat them as no recorded run and ask in the Tests step which tests passed.".to_string());
        }
    } else if !runs.is_empty() {
        lines.push("No test run kebbie recorded ran on the code under review. The newest older ones (○ passed on older code, ✗ failed, ? result unknown), for the Tests step:".to_string());
        for run in runs.iter().rev().take(OLDER_SHOWN) {
            lines.push(format!("- {} · `{}`{}", judge(run).line(), run.command, with_changes(run)));
        }
    } else {
        lines.push(match hook {
            Hook::Works => "kebbie recorded no test run in this task: say whether the change needed one.".to_string(),
            Hook::NeverRan => "kebbie's hook that records test runs never ran in this task: either its Claude ran no shell command, or the hook is broken. So no recorded run does not prove that no test ran; ask in the Tests step which tests ran.".to_string(),
            Hook::Broken => "The task's hooks run a kebbie that is no longer installed, so the hook that records test runs is broken and nothing it ran was recorded. Say so in the Tests step: a missing record here is not a missing test run.".to_string(),
        });
    }
    let named = |hidden_ones: bool| unrecorded.iter().filter(|command| hidden(command) == hidden_ones).map(|command| format!("`{command}`")).collect::<Vec<_>>().join(", ");
    if unrecorded.iter().any(|command| !hidden(command)) {
        lines.push(format!("It also ran {}, which looks like a test run but is not in task.test_commands, so kebbie did not record its result.", named(false)));
    }
    if unrecorded.iter().any(|command| hidden(command)) {
        lines.push(format!("It also ran {} inside a loop, a condition or xargs, where kebbie cannot tell a test run's result, so it did not record it.", named(true)));
    }
    lines.join("\n")
}

fn hook_in(state: &Path, address: &str, seen: &Seen) -> Hook {
    let settings = state.join("roles").join(format!("{address}.settings.json"));
    let broken = fs::read_to_string(settings).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).is_some_and(|settings| !crate::restart::gone_hooks(&settings).is_empty());
    match (broken, seen.at) {
        (true, _) => Hook::Broken,
        (false, None) => Hook::NeverRan,
        (false, Some(_)) => Hook::Works,
    }
}

// The reviewer reads the commits, not the tree, so uncommitted work does not make a run stale here.
pub fn brief_for(state: &Path, address: &str, worktree: &Path, upstream: &str, sha: &str) -> String {
    let seen = seen_in(state, address);
    let configured = crate::config::resolve(Some(worktree)).map(|loaded| loaded.settings.test_commands.value).unwrap_or_default();
    let listed = commands(&configured);
    settle(state, address, &listed, crate::messages::now());
    let unrecorded: Vec<String> = seen.unrecorded.iter().filter(|command| recognise(command, &listed).is_none()).cloned().collect();
    let current = |code: &str| tasks::same_code(worktree, upstream, code, sha);
    brief(&runs_in(state, address), |run| current(&run.code), |run| shown(run, current, true), hook_in(state, address, &seen), &unrecorded)
}

fn duration(ms: u64) -> String {
    if ms < 60_000 { format!("{:.1}s", ms as f64 / 1000.0) } else { format!("{}m {}s", ms / 60_000, ms / 1000 % 60) }
}

pub fn print(cwd: &Path, name: Option<&str>, as_json: bool) -> Result<String> {
    let task = task_at(cwd, name)?;
    let configured = crate::config::resolve(Some(&task.path)).map(|loaded| loaded.settings.test_commands.value).unwrap_or_default();
    settle(&state_dir()?, &task.address, &commands(&configured), crate::messages::now());
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
        return Ok(format!("No test run recorded for {}: kebbie records the test commands its Claude runs (task.test_commands adds to the built-in list).", task.address));
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
        assert_eq!(recognise("cargo build && cargo test", &list), Some("cargo test"), "any command of the chain counts");
        assert_eq!(recognise("echo cargo test", &list), None);
        assert_eq!(recognise("bin/test unit", &list), None);
        assert_eq!(recognise("bin/test unit", &listed(&["bin/test"])), Some("bin/test"), "a repo's own runner is added to the list");
        assert_eq!(recognise("make check", &listed(&["make check", ""])), Some("make check"));
        assert_eq!(recognise("./script/test --fast", &listed(&["script/test"])), Some("script/test"), "a leading ./ runs the same script");
        assert_eq!(recognise("cd app && ./script/test", &listed(&["script/test"])), Some("script/test"));
        assert_eq!(recognise("script/test", &listed(&["./script/test"])), Some("./script/test"));
    }

    #[test]
    fn a_test_command_after_a_separate_statement_or_a_wrapper_is_recorded() {
        let list = listed(&["script/test"]);
        for (command, runner) in [
            ("cd /src/wt1.app; script/test spec/a_spec.rb spec/b_spec.rb 2>&1 | grep examples", "script/test"),
            ("T=$(mktemp -d /tmp/kbj.XXXX); TMPDIR=$T script/test spec/javascripts/x_spec.js 2>&1 | tail -6; rm -rf $T", "script/test"),
            ("TMPDIR=$(mktemp -d); script/test", "script/test"),
            ("export CI=1; script/test", "script/test"),
            ("cd x; script/test", "script/test"),
            ("timeout 600 bin/rails test", "bin/rails test"),
            ("nice -n 5 cargo test", "cargo test"),
            ("time cargo test", "cargo test"),
            ("nohup npm test", "npm test"),
            ("set -euo pipefail\ncd app\nbin/rails test test/models", "bin/rails test"),
            ("bash -c 'cd app && script/test'", "script/test"),
            ("(cd app; cargo test)", "cargo test"),
        ] {
            assert_eq!(recognise(command, &list), Some(runner), "{command}");
        }
        for command in ["echo 'cd x; script/test'", "git commit -m 'run cargo test; fix'", "for f in a b; do script/test $f; done", "ls | xargs cargo test", "x=$(cargo test)", "set -e\ncat <<'EOF' > plan.md\ncargo test\nEOF"] {
            assert_eq!(recognise(command, &list), None, "{command}");
        }
    }

    #[test]
    fn a_listed_test_command_kebbie_cannot_attribute_is_told_to_the_task() {
        let list = listed(&["script/test"]);
        assert_eq!(unrecorded("for f in a b; do script/test $f; done", &list).as_deref(), Some("do script/test"));
        assert_eq!(unrecorded("ls spec | xargs bin/rails test", &list).as_deref(), Some("xargs bin/rails test"));
        assert_eq!(unrecorded("cd x; script/test", &list), None, "a recorded run needs no note");
        assert_eq!(unrecorded("cd x; make test", &list).as_deref(), Some("make test"), "after a separate statement too");
        let seen = Seen { at: Some(1), unrecorded: vec!["do script/test".to_string()] };
        assert_eq!(unrecorded_note(&seen, &list).as_deref(), Some("do script/test is not recorded as a test run: kebbie cannot tell its result inside a loop, a condition or xargs"));
        let brief = brief(&[], |_| true, |run: &Run| shown(run, |_| true, true), Hook::Works, &["do script/test".to_string(), "make test".to_string()]);
        assert!(brief.contains("It also ran `make test`, which looks like a test run but is not in task.test_commands") && brief.contains("It also ran `do script/test` inside a loop"), "{brief}");
        let input = json!({ "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": { "command": "cd x; make test" }, "cwd": "/nowhere" });
        let said: Value = serde_json::from_str(&hook("unrecorded-note@app", &input.to_string())).unwrap();
        assert_eq!(said["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert!(said["hookSpecificOutput"]["additionalContext"].as_str().unwrap().starts_with("kebbie did not record `make test` as a test run"), "{said}");
        assert_eq!(hook("unrecorded-note@app", &json!({ "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": { "command": "ls" }, "cwd": "/nowhere" }).to_string()), "", "nothing to say, nothing printed");
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
            ("Test Suite 'All tests' failed at 2026-11-06.\n\t Executed 12 tests, with 1 failure (0 unexpected) in 3.012 (3.020) seconds\n\n** TEST FAILED **", "Executed 12 tests, with 1 failure (0 unexpected)", true),
            ("\t Executed 12 tests, with 0 failures (0 unexpected) in 3.012 (3.020) seconds\n** TEST SUCCEEDED **", "Executed 12 tests, with 0 failures (0 unexpected)", false),
            ("Testing failed:\n\tUITests-Runner encountered an error\n** TEST FAILED **", "TEST FAILED", true),
        ];
        for (output, text, failed) in cases {
            assert_eq!(summary(output), Some(Summary { text: text.to_string(), failed, ran_none: false }), "{output}");
        }
        assert_eq!(summary("Compiling kebbie\nerror[E0425]: cannot find value"), None);
    }

    #[test]
    fn a_test_run_whose_result_line_counts_no_test_is_unknown_and_never_passed() {
        for output in [
            "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out", "     Summary [   0.0s] 0 tests run: 0 passed, 0 skipped",
            "0 examples, 0 failures", "0 runs, 0 assertions, 0 failures, 0 errors, 0 skips", "============ no tests ran in 0.01s ============", "  0 passing (1ms)",
            "\t Executed 0 tests, with 0 failures (0 unexpected) in 0.0 (0.0) seconds\n** TEST SUCCEEDED **",
        ] {
            let found = summary(output).unwrap();
            assert!(found.ran_none && !found.failed, "{output}");
            assert_eq!(judged(true, Exit::Speaks, Some(&found)), (Outcome::Unknown, RAN_NONE), "{output}");
        }
        assert!(!summary("test result: ok. 3 passed; 0 failed").unwrap().ran_none);
        assert_eq!(judged(false, Exit::Speaks, summary("test result: FAILED. 0 passed; 1 failed").as_ref()), (Outcome::Failed, ""));
        let dir = repo();
        let mut filtered = passed("cargo test no_such_test");
        filtered["tool_response"]["stdout"] = json!("running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 490 filtered out");
        let run = record_in(&dir.join("state"), "billing@app", &dir, &listed(&[]), &filtered, 1).unwrap();
        assert_eq!((run.result, run.why.as_str()), (Outcome::Unknown, RAN_NONE), "a filter that matched nothing is no pass");
        fs::remove_dir_all(dir).unwrap();
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
        assert_eq!(run, Run { command: "cargo test".to_string(), runner: "cargo test".to_string(), result: Outcome::Passed, why: String::new(), exit: Some(0), duration_ms: Some(4187), summary: "2 passed".to_string(), sha: head.clone(), dirty: false, code: head, at: 100, background: String::new(), transcript: String::new(), says: Some(Exit::Speaks), worktree: dir.to_string_lossy().into_owned(), read_to: 0 });
        assert_eq!(runs_in(&state, "billing@app"), [run]);
        let broke = record_in(&state, "billing@app", &dir, &list, &failed("npm test"), 200).unwrap();
        assert!(broke.result == Outcome::Failed && broke.exit == Some(1) && broke.runner == "npm test");
        assert!(record_in(&state, "billing@app", &dir, &list, &passed("ls -la"), 300).is_none());
        let mut interrupted = failed("cargo test");
        interrupted["is_interrupt"] = json!(true);
        assert!(record_in(&state, "billing@app", &dir, &list, &interrupted, 300).is_none());
        let mut piped = passed("cargo test 2>&1 | tail -3");
        piped["tool_response"]["stdout"] = json!("test result: FAILED. 1 passed; 1 failed; 0 ignored");
        assert_eq!(record_in(&state, "billing@app", &dir, &list, &piped, 400).unwrap().result, Outcome::Failed, "a pipe hides the exit code, so the result line decides");
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
    fn the_exit_counts_only_as_far_as_it_is_the_test_commands_own() {
        let list = listed(&["xcodebuild test"]);
        for (command, exit) in [
            ("cargo test", Exit::Speaks), ("cd x && cargo test", Exit::Speaks), ("cd x; T=$(mktemp -d); TMPDIR=$T cargo test 2>&1", Exit::Speaks),
            ("cargo test 2>&1 | tail -6", Exit::Silent(PIPED)), ("xcodebuild test -scheme App 2>&1 | grep -E 'passed|failed' | tail -6", Exit::Silent(PIPED)),
            ("set -o pipefail; cargo test | tail -6", Exit::ZeroOnly), ("set -euo pipefail\ncargo test 2>&1 | tail -6", Exit::ZeroOnly),
            ("cargo build && cargo test", Exit::ZeroOnly), ("cargo test && cargo clippy", Exit::ZeroOnly), ("set -e; cargo test; rm -rf t", Exit::ZeroOnly),
            ("cargo test; rm -rf $T", Exit::Silent(OTHER)), ("cargo test || true", Exit::Silent(OTHER)), ("false || cargo test", Exit::Silent(OTHER)), ("cargo test &", Exit::Silent(OTHER)),
            ("bash -c 'cargo test | tail -3'", Exit::Silent(PIPED)), ("(cd app; cargo test) && echo ok", Exit::ZeroOnly), ("cat list | cargo test", Exit::Speaks),
        ] {
            assert_eq!(find(command, &list).map(|found| found.exit), Some(exit), "{command}");
        }
    }

    #[test]
    fn a_test_command_piped_into_another_is_judged_by_its_result_line_and_else_unknown() {
        let dir = repo();
        let state = dir.join("state");
        let list = listed(&["xcodebuild test"]);
        let command = "xcodebuild test -scheme App -destination 'platform=iOS Simulator,name=iPhone 16' 2>&1 | grep -E 'Executed|TEST|error' | tail -6";
        let mut piped = passed(command);
        piped["tool_response"]["stdout"] = json!("Test Case '-[AppUITests testCheckout]' failed (4.2 seconds).\n\t Executed 12 tests, with 1 failure (0 unexpected) in 31.2 (31.3) seconds\n** TEST FAILED **");
        let run = record_in(&state, "billing@app", &dir, &list, &piped, 1).unwrap();
        assert_eq!((run.result, run.summary.as_str()), (Outcome::Failed, "Executed 12 tests, with 1 failure (0 unexpected)"), "tail's exit 0 is no pass");
        piped["tool_response"]["stdout"] = json!("Build succeeded");
        let run = record_in(&state, "billing@app", &dir, &list, &piped, 2).unwrap();
        assert_eq!((run.result, run.why.as_str()), (Outcome::Unknown, PIPED), "no result line, so the pipe's exit says nothing");
        let mut rest = passed("cargo test; rm -rf $T");
        rest["tool_response"]["stdout"] = json!("");
        assert_eq!(record_in(&state, "billing@app", &dir, &list, &rest, 3).unwrap().result, Outcome::Unknown, "the exit is rm's");
        let mut pipefail = passed("set -o pipefail; cargo test 2>&1 | grep -c ok");
        pipefail["tool_response"]["stdout"] = json!("2");
        assert_eq!(record_in(&state, "billing@app", &dir, &list, &pipefail, 4).unwrap().result, Outcome::Passed, "with pipefail a zero is the test's too");
        let mut broke = failed("cd app && cargo test 2>&1");
        broke["error"] = json!("Exit code 101");
        assert_eq!(record_in(&state, "billing@app", &dir, &list, &broke, 5).unwrap().result, Outcome::Failed, "nothing after it, so its exit is its own");
        assert_eq!(record_in(&state, "billing@app", &dir, &list, &failed("cargo build && cargo test"), 6).unwrap().result, Outcome::Unknown, "the build may have failed before the tests ran");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_background_run_reads_only_what_the_transcript_gained_and_is_given_up_after_a_day() {
        let dir = repo();
        let state = dir.join("state");
        let list = listed(&[]);
        let transcript = dir.join("session.jsonl");
        let old = format!("{}\n", json!({ "type": "user", "content": "x".repeat(4096) }));
        fs::write(&transcript, &old).unwrap();
        let mut background = passed("cargo test");
        background["tool_response"] = json!({ "stdout": "", "stderr": "", "interrupted": false, "backgroundTaskId": "bx9" });
        background["transcript_path"] = json!(transcript.to_str().unwrap());
        record_in(&state, "billing@app", &dir, &list, &background, 100).unwrap();
        assert!(settle(&state, "billing@app", &list, 200).is_empty());
        assert_eq!(runs_in(&state, "billing@app")[0].read_to, old.len() as u64, "the next look starts where this one ended");
        let output = dir.join("bx9.output");
        fs::write(&output, "test result: ok. 3 passed; 0 failed\n").unwrap();
        let end = notification("bx9", &output, "completed", "Background command \"cargo test\" completed (exit code 0)");
        let mut grown = fs::OpenOptions::new().append(true).open(&transcript).unwrap();
        std::io::Write::write_all(&mut grown, &end.as_bytes()[..end.len() / 2]).unwrap();
        assert!(settle(&state, "billing@app", &list, 300).is_empty(), "half a line is left for the next look");
        assert_eq!(runs_in(&state, "billing@app")[0].read_to, old.len() as u64);
        std::io::Write::write_all(&mut grown, &end.as_bytes()[end.len() / 2..]).unwrap();
        assert_eq!(settle(&state, "billing@app", &list, 400)[0].result, Outcome::Passed);
        background["tool_response"]["backgroundTaskId"] = json!("bx10");
        record_in(&state, "billing@app", &dir, &list, &background, 1000).unwrap();
        assert!(settle(&state, "billing@app", &list, 1000 + BACKGROUND_GIVE_UP - 1).is_empty());
        let given_up = settle(&state, "billing@app", &list, 1000 + BACKGROUND_GIVE_UP);
        assert_eq!((given_up[0].result, given_up[0].why.as_str(), given_up[0].background.as_str()), (Outcome::Unknown, NEVER_ENDED, ""), "a notice that never lands stops costing a read");
        assert!(settle(&state, "billing@app", &list, 1000 + BACKGROUND_GIVE_UP + 1).is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_settled_pass_goes_back_to_the_reviewer_for_its_own_task_and_never_for_the_worktree_the_shell_is_in() {
        let (dir, origin, slot, _) = crate::tasks::test_support::task_with_a_commit("billing");
        let (other_dir, other_origin, other_slot, _) = crate::tasks::test_support::task_in_another_repo("api");
        let address = peers::address("billing", &name_of(&dir));
        let run = |worktree: &Path, result: Outcome| Run { result, worktree: worktree.to_string_lossy().into_owned(), ..Run::default() };
        assert_eq!(passed_in(&address, &[run(&other_slot.path, Outcome::Passed)]), None, "a pass of this task never sends the task the shell has cd'd into");
        assert_eq!(passed_in(&address, &[run(&slot.path, Outcome::Failed)]), None);
        assert_eq!(passed_in(&address, &[run(&other_slot.path, Outcome::Passed), run(&slot.path, Outcome::Passed)]), Some(slot.path.clone()));
        assert_eq!(passed_in(&address, &[Run { result: Outcome::Passed, ..Run::default() }]), None, "a run recorded before runs named their worktree");
        let source = include_str!("evidence.rs");
        let hook = &source[source.find("\npub fn hook(").unwrap()..];
        assert!(!hook[..hook.find("\n}\n").unwrap()].contains("show-toplevel"), "the hook's cwd never picks the task to send back");
        crate::tasks::test_support::cleanup(dir, origin, &slot);
        crate::tasks::test_support::cleanup(other_dir, other_origin, &other_slot);
    }

    fn notification(id: &str, output: &Path, status: &str, summary: &str) -> String {
        let content = format!("<task-notification>\n<task-id>{id}</task-id>\n<tool-use-id>toolu_01</tool-use-id>\n<output-file>{}</output-file>\n<status>{status}</status>\n<summary>{summary}</summary>\n</task-notification>", output.display());
        format!("{}\n", json!({ "type": "queue-operation", "operation": "enqueue", "timestamp": "2026-11-06T10:00:00.000Z", "content": content }))
    }

    #[test]
    fn a_test_run_moved_to_the_background_is_unknown_until_its_end_settles_it() {
        let dir = repo();
        let state = dir.join("state");
        let list = listed(&["xcodebuild test"]);
        let transcript = dir.join("session.jsonl");
        fs::write(&transcript, "").unwrap();
        let mut timed_out = passed("xcodebuild test -scheme App");
        timed_out["tool_response"] = json!({ "stdout": "", "stderr": "", "interrupted": false, "isImage": false, "backgroundTaskId": "bx1", "timedOutAfterMs": 600000 });
        timed_out["transcript_path"] = json!(transcript.to_str().unwrap());
        let run = record_in(&state, "billing@app", &dir, &list, &timed_out, 1).unwrap();
        assert_eq!((run.result, run.why.as_str(), run.exit, run.background.as_str()), (Outcome::Unknown, IN_BACKGROUND, None, "bx1"), "moved to the background at its timeout, it has not ended");
        assert!(unknown_context(&run).contains("run it in the foreground, or say in your report how it ended"));
        assert!(settle(&state, "billing@app", &list, 10).is_empty(), "no notice of its end yet");
        let output = dir.join("bx1.output");
        fs::write(&output, "Test Case '-[AppUITests testCheckout]' failed\n\t Executed 40 tests, with 1 failure (0 unexpected) in 700.1 (700.2) seconds\n** TEST FAILED **\n").unwrap();
        fs::write(&transcript, notification("bx1", &output, "completed", "Background command \"xcodebuild test -scheme App\" completed (exit code 65)")).unwrap();
        let settled = settle(&state, "billing@app", &list, 10);
        assert_eq!(settled.len(), 1);
        assert_eq!((settled[0].result, settled[0].exit, settled[0].summary.as_str()), (Outcome::Failed, Some(65), "Executed 40 tests, with 1 failure (0 unexpected)"));
        assert_eq!(runs_in(&state, "billing@app").last().unwrap().result, Outcome::Failed, "the record holds the real result");
        assert!(settle(&state, "billing@app", &list, 10).is_empty(), "settled once");

        let mut explicit = passed("cargo test");
        explicit["tool_input"]["run_in_background"] = json!(true);
        explicit["tool_response"] = json!({ "stdout": "", "stderr": "", "interrupted": false, "backgroundTaskId": "bx2" });
        explicit["transcript_path"] = json!(transcript.to_str().unwrap());
        assert_eq!(record_in(&state, "billing@app", &dir, &list, &explicit, 2).unwrap().result, Outcome::Unknown, "started in the background on purpose, it is recorded too");
        let output = dir.join("bx2.output");
        fs::write(&output, "test result: ok. 3 passed; 0 failed; 0 ignored\n").unwrap();
        fs::write(&transcript, notification("bx2", &output, "completed", "Background command \"cargo test\" completed (exit code 0)")).unwrap();
        assert_eq!(settle(&state, "billing@app", &list, 10)[0].result, Outcome::Passed);

        let long = format!("cargo test {} 2>&1 | tail -3", "--features x ".repeat(30));
        let mut piped = explicit.clone();
        piped["tool_input"]["command"] = json!(long);
        piped["tool_response"]["backgroundTaskId"] = json!("bx4");
        let recorded = record_in(&state, "billing@app", &dir, &list, &piped, 3).unwrap();
        assert!(recorded.command.ends_with('…') && !recorded.command.contains("tail"), "the stored command is cut short");
        let output = dir.join("bx4.output");
        fs::write(&output, "Compiling kebbie\n").unwrap();
        fs::write(&transcript, notification("bx4", &output, "completed", "Background command completed (exit code 0)")).unwrap();
        assert_eq!((settle(&state, "billing@app", &list, 10)[0].result), Outcome::Unknown, "judged on the whole line as it ran, so tail's exit 0 is no pass");

        let mut stopped = explicit.clone();
        stopped["tool_response"]["backgroundTaskId"] = json!("bx3");
        record_in(&state, "billing@app", &dir, &list, &stopped, 3).unwrap();
        fs::write(&transcript, notification("bx3", &dir.join("none"), "killed", "Background command \"cargo test\" was stopped")).unwrap();
        assert_eq!(settle(&state, "billing@app", &list, 10)[0].why, "stopped before it ended", "a stopped run proves nothing");
        let mut unseen = passed("cargo test");
        unseen["tool_input"]["run_in_background"] = json!(true);
        let run = record_in(&state, "billing@app", &dir, &list, &unseen, 4).unwrap();
        assert!(run.result == Outcome::Unknown && run.background.is_empty() && unknown_context(&run).contains("cannot see how it ends"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_run_is_current_until_the_code_changes_and_a_run_before_the_commit_counts_for_it() {
        let dir = repo();
        let state = std::env::temp_dir().join(format!("kebbie-evidence-state-{}", std::process::id()));
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
    fn the_reviewer_reads_every_run_on_the_reviewed_code_one_line_per_command_and_older_ones_only_without_them() {
        let run = |at: u64, command: &str, ok: bool, code: &str| Run { command: command.to_string(), runner: "cargo test".to_string(), result: if ok { Outcome::Passed } else { Outcome::Failed }, why: String::new(), exit: Some(0), duration_ms: None, summary: "3 passed".to_string(), sha: format!("{at}abcdef0123"), dirty: at == 9, code: code.to_string(), at, background: String::new(), transcript: String::new(), says: None, worktree: String::new(), read_to: 0 };
        let mut runs = vec![run(1, "cargo test", false, "old")];
        for at in 2..9 {
            runs.push(run(at, "cargo test --lib evidence", at != 4, "reviewed"));
        }
        runs.push(run(9, "cargo test", true, "reviewed"));
        let judge = |run: &Run| shown(run, |code| code == "reviewed", true);
        let text = brief(&runs, |run| run.code == "reviewed", judge, Hook::Works, &[]);
        assert!(text.contains("weigh them in the Tests step"), "{text}");
        assert!(text.contains("- `cargo test --lib evidence` · 7 runs (6 passed, 1 failed), the newest: ✓ cargo test · 3 passed · at 8abcdef"), "every run on the reviewed code counts, not the newest five: {text}");
        assert!(text.contains("- `cargo test` ✓ cargo test · 3 passed · at 9abcdef · with uncommitted changes"), "{text}");
        assert!(text.find("`cargo test` ✓").unwrap() < text.find("`cargo test --lib evidence`").unwrap(), "newest command first: {text}");
        assert!(!text.contains("1abcdef"), "an older run is left out while runs on the reviewed code exist: {text}");
        assert_eq!(text.lines().count(), 3, "one line per command keeps it short: {text}");
        let older = brief(&runs, |_| false, |run| shown(run, |_| false, true), Hook::Works, &[]);
        assert!(older.starts_with("No test run kebbie recorded ran on the code under review") && older.lines().count() == 1 + OLDER_SHOWN, "{older}");
        assert!(brief(&[], |_| true, judge, Hook::Works, &[]).contains("recorded no test run"));
    }

    #[test]
    fn a_run_without_a_result_reads_as_unknown_and_counts_as_no_run_for_the_reviewer() {
        let run = Run { command: "cargo test | tail -3".to_string(), runner: "cargo test".to_string(), result: Outcome::Unknown, why: "piped".to_string(), exit: Some(0), duration_ms: None, summary: String::new(), sha: "abcdef0123".to_string(), dirty: false, code: "reviewed".to_string(), at: 1, background: String::new(), transcript: String::new(), says: Some(Exit::Silent(PIPED)), worktree: String::new(), read_to: 0 };
        let judge = |run: &Run| shown(run, |code| code == "reviewed", true);
        assert_eq!(judge(&run).line(), "? cargo test result unknown, piped · at abcdef0", "not a pass even on the code as it is");
        assert_eq!(Run::from_json(&run.to_json()), Some(run.clone()));
        assert_eq!(run.to_json()["ok"], false, "an older kebbie reading the file sees no pass");
        let older = json!({ "command": "cargo test", "runner": "cargo test", "ok": true, "sha": "a", "code": "a" });
        assert_eq!(Run::from_json(&older).map(|run| run.result), Some(Outcome::Passed), "a run recorded before results could be unknown");
        let text = brief(std::slice::from_ref(&run), |_| true, judge, Hook::Works, &[]);
        assert!(text.contains("- `cargo test | tail -3` ? cargo test result unknown, piped") && text.contains("treat them as no recorded run"), "{text}");
        let passed = Run { result: Outcome::Passed, why: String::new(), at: 2, command: "cargo test".to_string(), ..run.clone() };
        let both = brief(&[run.clone(), passed, run.clone()], |_| true, judge, Hook::Works, &[]);
        assert!(both.contains("2 runs (0 passed, 0 failed, 2 unknown)") && !both.contains("treat them as no recorded run"), "{both}");
    }

    #[test]
    fn the_reviewer_tells_a_broken_test_run_hook_from_no_test_run() {
        let judge = |run: &Run| shown(run, |_| true, true);
        let none = brief(&[], |_| true, judge, Hook::Works, &[]);
        let never = brief(&[], |_| true, judge, Hook::NeverRan, &[]);
        let broken = brief(&[], |_| true, judge, Hook::Broken, &[]);
        assert!(none.contains("recorded no test run") && !none.contains("broken"));
        assert!(never.contains("never ran in this task") && never.contains("does not prove that no test ran"), "{never}");
        assert!(broken.contains("hook that records test runs is broken") && broken.contains("not a missing test run"), "{broken}");
        assert!(brief(&[], |_| true, judge, Hook::Works, &["script/test".to_string()]).contains("It also ran `script/test`, which looks like a test run but is not in task.test_commands"));
        let state = crate::sh::test_dir("evidence-hook-health");
        assert_eq!(hook_in(&state, "billing@app", &Seen::default()), Hook::NeverRan);
        assert_eq!(hook_in(&state, "billing@app", &Seen { at: Some(1), unrecorded: Vec::new() }), Hook::Works);
        fs::create_dir_all(state.join("roles")).unwrap();
        fs::write(state.join("roles/billing@app.settings.json"), crate::agent::claude::settings("/home/u/.local/share/kebbie/gone/target/release/kebbie", "billing@app", None, None, None)).unwrap();
        assert_eq!(hook_in(&state, "billing@app", &Seen { at: Some(1), unrecorded: Vec::new() }), Hook::Broken, "settings whose hooks run a kebbie binary that is gone");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_command_that_looks_like_a_test_run_is_told_apart_conservatively() {
        for (command, prefix) in [
            ("script/test", "script/test"), ("./script/test --fast spec/models", "./script/test"), ("bin/test unit", "bin/test"), ("cd /src/wt1.app && CI=1 bin/spec", "bin/spec"),
            ("make test", "make test"), ("just test-all", ""), ("npm run test:unit", "npm run test:unit"), ("yarn spec", "yarn spec"), ("bundle exec rake spec", "bundle exec rake spec"),
            ("deno test -A", "deno test"), ("npx vitest run", ""), ("vitest run", "vitest"), ("tox -e py312", "tox"), ("make test 2>&1 | tail", "make test"),
        ] {
            assert_eq!(unrecorded(command, &[]).unwrap_or_default(), prefix, "{command}");
        }
        for command in ["test -f Cargo.toml", "rg test src", "grep -r spec .", "git commit -m 'add test'", "ls tests", "cat spec/a_spec.rb", "echo make test", "make build", "vim test/x.rb", "npm install", "cargo build", "echo 'a; make test'"] {
            assert_eq!(unrecorded(command, &[]), None, "{command}");
        }
    }

    #[test]
    fn the_hook_notes_that_it_ran_and_which_test_like_commands_the_list_leaves_out() {
        let state = crate::sh::test_dir("evidence-seen");
        assert_eq!(seen_in(&state, "billing@app"), Seen::default());
        note_seen(&state, "billing@app", None, 10);
        assert_eq!(seen_in(&state, "billing@app").at, Some(10));
        for (at, command) in [(11, "script/test"), (12, "bin/test"), (13, "./script/test"), (14, "make test"), (15, "tox")] {
            note_seen(&state, "billing@app", Some(command.to_string()), at);
        }
        let seen = seen_in(&state, "billing@app");
        assert_eq!((seen.at, seen.unrecorded.clone()), (Some(15), vec!["script/test".to_string(), "make test".to_string(), "tox".to_string()]), "./script/test and script/test are one entry");
        assert_eq!(unrecorded_note(&seen, &listed(&[])).as_deref(), Some("tox is not recorded as a test run: add it to task.test_commands"));
        assert_eq!(unrecorded_note(&seen, &listed(&["tox", "make test"])).as_deref(), Some("script/test is not recorded as a test run: add it to task.test_commands"), "once listed, the note moves on");
        assert_eq!(unrecorded_note(&seen, &listed(&["tox", "make test", "script/test"])), None);
        let dotted = Seen { at: Some(16), unrecorded: vec!["./script/test".to_string()] };
        assert_eq!(unrecorded_note(&dotted, &listed(&["script/test"])), None, "./script/test runs the listed script/test, so the row never asks to add it");
        let source = include_str!("evidence.rs");
        let hook = &source[source.find("\npub fn hook(").unwrap()..];
        assert!(hook.find("note_seen(").unwrap() < hook.find("recorded(address").unwrap(), "every Bash call counts as the hook running, test run or not");
        assert!(!DEFAULT_COMMANDS.contains(&"script/test") && !DEFAULT_COMMANDS.contains(&"bin/test"), "a repo's own runner stays its own config");
        fs::remove_dir_all(state).unwrap();
    }
}
