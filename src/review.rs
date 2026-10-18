use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::config;
use crate::lead::{install_root, roles_dir, state_dir, state_path, write_settings};
use crate::messages;
use crate::peers;
use crate::repo::{ahead_behind, default_branch, git, main_checkout, name_of};
use crate::sh::Result;
use crate::tasks::{self, Slot};

const TOOLS: &str = "Read,Grep,Glob,Bash";
const ALLOWED: [&str; 6] = ["Read", "Grep", "Glob", "Bash(git log:*)", "Bash(git diff:*)", "Bash(git show:*)"];

// Not reviews/, where the review split keeps its state.
fn dir(state: &Path) -> PathBuf {
    state.join("verdicts")
}

pub fn verdict_file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.md"))
}

fn running_file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.running"))
}

fn log_file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.log"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub ok: bool,
    pub sha: String,
    pub findings: usize,
}

impl Verdict {
    pub fn summary(&self) -> String {
        match (self.ok, self.findings) {
            (true, _) => "ok".to_string(),
            (false, 0) => "changes asked".to_string(),
            (false, 1) => "1 finding".to_string(),
            (false, count) => format!("{count} findings"),
        }
    }
}

// The file kelpie writes: the verdict line, the sha it reviewed, then the findings, each a line starting "- ".
pub fn parse_verdict(text: &str) -> Option<Verdict> {
    let mut lines = text.lines();
    let ok = match lines.next()?.trim() {
        "verdict: ok" => true,
        "verdict: changes" => false,
        _ => return None,
    };
    let sha = lines.next()?.trim().strip_prefix("reviewed: ")?.trim().to_string();
    Some(Verdict { ok, sha, findings: lines.filter(|line| line.starts_with("- ")).count() })
}

// The reviewer's answer from its verdict line on, with the reviewed sha after that line; None when it gave no verdict.
pub fn record(answer: &str, sha: &str) -> Option<String> {
    let lines: Vec<&str> = answer.lines().collect();
    let start = lines.iter().position(|line| matches!(line.trim().trim_matches(['*', '`']), "verdict: ok" | "verdict: changes"))?;
    let verdict = lines[start].trim().trim_matches(['*', '`']);
    let findings = lines[start + 1..].join("\n");
    Some(format!("{verdict}\nreviewed: {sha}\n\n{}\n", findings.trim()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub verdict: Option<Verdict>,
    pub stale: bool,
    pub running: bool,
    pub file: PathBuf,
}

impl Shown {
    pub fn to_json(&self) -> Value {
        json!({
            "verdict": self.verdict.as_ref().map(|verdict| if verdict.ok { "ok" } else { "changes" }),
            "findings": self.verdict.as_ref().map(|verdict| verdict.findings),
            "sha": self.verdict.as_ref().map(|verdict| verdict.sha.chars().take(7).collect::<String>()),
            "stale": self.stale,
            "running": self.running,
            "file": self.file,
        })
    }
}

pub fn shown(address: &str, head: &str) -> Option<Shown> {
    shown_in(&state_path(|name| std::env::var(name).ok()).ok()?, address, head)
}

pub fn shown_in(state: &Path, address: &str, head: &str) -> Option<Shown> {
    let file = verdict_file(state, address);
    let verdict = fs::read_to_string(&file).ok().and_then(|text| parse_verdict(&text));
    let running = running_in(state, address);
    if verdict.is_none() && !running {
        return None;
    }
    let stale = verdict.as_ref().is_some_and(|verdict| verdict.sha != head);
    Some(Shown { verdict, stale, running, file })
}

// A marker whose process is gone is a review that died, not one still running.
fn running_in(state: &Path, address: &str) -> bool {
    let Ok(pid) = fs::read_to_string(running_file(state, address)) else { return false };
    let pid = pid.trim();
    !pid.is_empty() && Command::new("kill").args(["-0", pid]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|status| status.success())
}

pub fn forget(task: &str, repo: &Path) {
    if let Ok(state) = state_dir() {
        forget_in(&state, &peers::address(task, &name_of(repo)));
    }
}

pub fn forget_in(state: &Path, address: &str) {
    for file in [verdict_file(state, address), running_file(state, address), log_file(state, address)] {
        let _ = fs::remove_file(file);
    }
}

// Starts the reviewer as a process of its own, so neither the board nor the caller waits for it.
pub fn request(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = tasks::find_task(&repo, name, cwd)?;
    let task = tasks::task_of(&slot);
    let upstream = default_branch(&repo)?;
    if ahead_behind(&slot.path, &upstream)?.0 == 0 {
        return Err(format!("{task} has no commits to review"));
    }
    let address = peers::address(task, &name_of(&repo));
    let state = state_dir()?;
    fs::create_dir_all(dir(&state)).map_err(|error| error.to_string())?;
    if running_in(&state, &address) {
        return Err(format!("A review of {address} is already running; the lead hears when its verdict is in."));
    }
    let sha = git(&slot.path, &["rev-parse", "--short", "HEAD"])?;
    let log = log_file(&state, &address);
    let output = fs::File::create(&log).map_err(|error| format!("{}: {error}", log.display()))?;
    let errors = output.try_clone().map_err(|error| error.to_string())?;
    Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
        .arg("-C").arg(&slot.path).args(["review", "--run"])
        .stdin(Stdio::null()).stdout(output).stderr(errors).process_group(0)
        .spawn().map_err(|error| format!("kelpie review: {error}"))?;
    Ok(format!(
        "Reviewing {address} at {sha} in the background; the verdict goes to {} and the lead is told (output in {}).",
        verdict_file(&state, &address).display(), log.display()
    ))
}

// The background half: runs the reviewer, saves its verdict and tells the lead, also when it fails.
pub fn run(cwd: &Path) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = tasks::find_task(&repo, None, cwd)?;
    let address = peers::address(tasks::task_of(&slot), &name_of(&repo));
    let state = state_dir()?;
    fs::create_dir_all(dir(&state)).map_err(|error| error.to_string())?;
    let marker = running_file(&state, &address);
    fs::write(&marker, std::process::id().to_string()).map_err(|error| error.to_string())?;
    let result = review(&repo, &slot, &address, &state);
    let _ = fs::remove_file(&marker);
    let notice = match &result {
        Ok(text) => text.clone(),
        Err(error) => format!("The review of {address} failed: {error} (output in {})", log_file(&state, &address).display()),
    };
    if let Err(error) = messages::notify_lead(&notice) {
        eprintln!("Could not tell the lead: {error}");
    }
    result
}

fn review(repo: &Path, slot: &Slot, address: &str, state: &Path) -> Result<String> {
    let upstream = default_branch(repo)?;
    let sha = git(&slot.path, &["rev-parse", "HEAD"])?;
    let settings = config::resolve(Some(repo))?.settings;
    let commits = git(&slot.path, &["log", "--reverse", "--format=%h %s", &format!("{upstream}..{sha}")])?;
    let said = tasks::transcript(repo, slot).and_then(|file| fs::read_to_string(file).ok()).map(|text| user_messages(&text)).unwrap_or_default();
    let file = verdict_file(state, address);
    let kelpie = install_root()?;
    let rules = &settings.rules_skill.value;
    let rules_file = tasks::claude_home().and_then(|home| skill_file(rules, &slot.path, &home, &kelpie));
    let input = brief(&Brief { address, worktree: &slot.path, upstream: &upstream, sha: &sha, commits: &commits, rules, rules_file: rules_file.as_deref(), said: &said });
    let guard = write_settings(&roles_dir()?, &format!("{address}.review"), &guard_settings(&peers::kelpie_command()?))?;
    let skill = kelpie.join("skills/review/SKILL.md");
    let args = claude_args(&Reviewer { model: settings.review_model.value.as_deref(), settings: &guard, skill: &skill, rules: rules_file.as_deref().and_then(Path::parent) });
    let mut child = Command::new("claude").args(&args).current_dir(&slot.path)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|error| format!("claude: {error}"))?;
    use std::io::Write;
    child.stdin.take().ok_or("claude has no stdin")?.write_all(input.as_bytes()).map_err(|error| error.to_string())?;
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    let answer = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        return Err(format!("claude stopped with {}: {}", output.status, String::from_utf8_lossy(&output.stderr).trim()));
    }
    let recorded = record(&answer, &sha).ok_or_else(|| format!("the reviewer's answer has no verdict line; it ended: {}", answer.trim().chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect::<String>()))?;
    let verdict = parse_verdict(&recorded).ok_or("the recorded verdict does not parse")?;
    let partial = file.with_extension("md.partial");
    fs::write(&partial, &recorded).and_then(|_| fs::rename(&partial, &file)).map_err(|error| format!("{}: {error}", file.display()))?;
    Ok(format!("{address} reviewed at {}: {} — {}", &sha[..7.min(sha.len())], verdict.summary(), file.display()))
}

struct Reviewer<'a> {
    model: Option<&'a str>,
    settings: &'a Path,
    skill: &'a Path,
    rules: Option<&'a Path>,
}

// Only these tools exist, only the allowed ones run, the guard hook stops any other shell command, and no user setting, hook or plugin joins in.
fn claude_args(reviewer: &Reviewer) -> Vec<String> {
    let Reviewer { model, settings, skill, rules } = *reviewer;
    let mut args: Vec<String> = ["-p", "--setting-sources", "", "--output-format", "text", "--no-session-persistence", "--strict-mcp-config", "--permission-mode", "dontAsk", "--tools", TOOLS, "--allowedTools"]
        .map(str::to_string).to_vec();
    args.extend(ALLOWED.map(str::to_string));
    args.extend(["--settings".to_string(), settings.to_string_lossy().to_string(), "--append-system-prompt-file".to_string(), skill.to_string_lossy().to_string()]);
    if let Some(rules) = rules {
        args.extend(["--add-dir".to_string(), rules.to_string_lossy().to_string()]);
    }
    if let Some(model) = model {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    args
}

fn guard_settings(kelpie: &str) -> String {
    json!({ "hooks": { "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": format!("'{kelpie}' hook --review") }] }] } }).to_string()
}

// The rules skill's own file, where Claude Code finds it: kelpie's skills, another plugin's, or the repo's or the user's .claude/skills.
pub fn skill_file(name: &str, worktree: &Path, claude_home: &Path, kelpie: &Path) -> Option<PathBuf> {
    let found = |root: &Path, skill: &str| Some(root.join("skills").join(skill).join("SKILL.md")).filter(|file| file.is_file());
    match name.split_once(':') {
        Some(("kelpie", skill)) => found(kelpie, skill),
        Some((plugin, skill)) => {
            let installed: Value = serde_json::from_str(&fs::read_to_string(claude_home.join("plugins/installed_plugins.json")).ok()?).ok()?;
            installed["plugins"].as_object()?.iter()
                .filter(|(id, _)| id.split_once('@').is_some_and(|(own, _)| own == plugin))
                .filter_map(|(_, installs)| installs[0]["installPath"].as_str())
                .find_map(|root| found(Path::new(root), skill))
        }
        None => found(&worktree.join(".claude"), name).or_else(|| found(claude_home, name)),
    }
}

// git log, diff and show alone: no other program, no redirection, and no --output, which writes a file.
pub fn read_only(command: &str) -> bool {
    if command.contains(['\n', '\r', ';', '&', '|', '<', '>', '`', '$', '\\', '(', ')']) {
        return false;
    }
    let mut words = command.split_whitespace().map(|word| word.replace(['"', '\''], ""));
    if words.next().as_deref() != Some("git") || !matches!(words.next().as_deref(), Some("log" | "diff" | "show")) {
        return false;
    }
    words.all(|word| !(word.starts_with("--ou") || word.starts_with("--ext")))
}

// The PreToolUse hook the reviewer runs before each shell command.
pub fn guard(input: &str) -> String {
    let input: Value = serde_json::from_str(input).unwrap_or_default();
    if input["tool_input"]["command"].as_str().is_some_and(read_only) {
        return String::new();
    }
    let reason = "The reviewer only reads: run git log, git diff or git show on their own (no pipes, redirection or --output), and use Read, Grep and Glob for files.";
    json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason } }).to_string()
}

// What the user typed into the task's conversation, oldest first: its prompt and every later word, without tool results or kelpie's own wrappers.
pub fn user_messages(transcript: &str) -> Vec<String> {
    transcript.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["type"] == "user" && entry["isMeta"] != true && entry["isSidechain"] != true)
        .filter_map(|entry| match &entry["message"]["content"] {
            Value::String(text) => Some(text.clone()),
            Value::Array(blocks) if blocks.iter().all(|block| block["type"] == "text") => {
                Some(blocks.iter().filter_map(|block| block["text"].as_str()).collect::<Vec<_>>().join("\n"))
            }
            _ => None,
        })
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty() && !text.starts_with('<') && !text.starts_with("[Request interrupted"))
        .collect()
}

const PROMPT_BUDGET: usize = 12_000;
const LATER_BUDGET: usize = 12_000;

fn clip(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget { text.to_string() } else { format!("{}…", text.chars().take(budget).collect::<String>()) }
}

// The first prompt whole up to its budget, then the newest later messages that fit theirs, in the order they were said.
pub fn conversation(said: &[String]) -> String {
    let Some((prompt, later)) = said.split_first() else {
        return "kelpie has no record of what the task was asked; take its intent from the commit messages.".to_string();
    };
    let mut text = format!("The prompt the task was started with:\n\n{}\n", clip(prompt, PROMPT_BUDGET));
    let mut kept = Vec::new();
    let mut left = LATER_BUDGET;
    for message in later.iter().rev() {
        let message = clip(message, LATER_BUDGET / 4);
        let size = message.chars().count();
        if size > left {
            break;
        }
        left -= size;
        kept.push(message);
    }
    if !kept.is_empty() {
        let skipped = later.len() - kept.len();
        let note = if skipped > 0 { format!(" ({skipped} older ones left out)") } else { String::new() };
        text.push_str(&format!("\nWhat the task was told after that, oldest first{note}:\n"));
        for message in kept.iter().rev() {
            text.push_str(&format!("\n---\n{message}\n"));
        }
    }
    text
}

struct Brief<'a> {
    address: &'a str,
    worktree: &'a Path,
    upstream: &'a str,
    sha: &'a str,
    commits: &'a str,
    rules: &'a str,
    rules_file: Option<&'a Path>,
    said: &'a [String],
}

fn brief(brief: &Brief) -> String {
    let Brief { address, worktree, upstream, sha, commits, rules, rules_file, said } = *brief;
    let rules = match rules_file {
        Some(file) => format!("The rules skill in effect is `{rules}`, at {}: read it, and the files beside it that it points to, and check every commit against each of its rules.", file.display()),
        None => format!("The rules skill in effect is `{rules}`, but kelpie found no file for it: say so on the line after the verdict line, and review without it."),
    };
    format!(
        "Review the branch of the task {address} in {}, at {sha}, by the kelpie:review instructions in your system prompt.\n\n\
         Its commits, oldest first, are {upstream}..{sha}:\n{commits}\n\n\
         {rules}\n\n\
         {}\n\
         Your last message is the verdict, starting with its verdict line; kelpie saves it and tells the lead.",
        worktree.display(), conversation(said)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reviewer_has_only_reading_tools_and_git_log_diff_show_on_the_shell() {
        let reviewer = Reviewer { model: None, settings: Path::new("/state/roles/billing@app.review.settings.json"), skill: Path::new("/kelpie/skills/review/SKILL.md"), rules: None };
        let args = claude_args(&reviewer);
        let after = |flag: &str| args[args.iter().position(|arg| arg == flag).unwrap() + 1].clone();
        assert_eq!(args[0], "-p");
        assert_eq!(after("--permission-mode"), "dontAsk", "anything not allowed is refused, never asked");
        assert_eq!(after("--tools"), "Read,Grep,Glob,Bash");
        assert_eq!(after("--setting-sources"), "", "the user's allow rules and hooks stay out: a Stop hook of theirs took over the answer");
        for tool in ["Edit", "Write", "MultiEdit", "NotebookEdit", "WebFetch"] {
            assert!(!args.iter().any(|arg| arg.contains(tool)), "{tool} in {args:?}");
        }
        let allowed: Vec<&String> = args.iter().filter(|arg| arg.starts_with("Bash(")).collect();
        assert_eq!(allowed, ["Bash(git log:*)", "Bash(git diff:*)", "Bash(git show:*)"]);
        assert!(args.contains(&"--strict-mcp-config".to_string()), "no MCP server's tools");
        assert_eq!(after("--append-system-prompt-file"), "/kelpie/skills/review/SKILL.md");
        assert_eq!(after("--settings"), "/state/roles/billing@app.review.settings.json");
        assert!(!args.contains(&"--model".to_string()), "no model set leaves Claude's default");
        assert!(!args.contains(&"--add-dir".to_string()));
        let with_model = claude_args(&Reviewer { model: Some("opus"), rules: Some(Path::new("/plugins/rules/skills/house")), ..reviewer });
        assert_eq!(with_model[with_model.len() - 4..], ["--add-dir", "/plugins/rules/skills/house", "--model", "opus"]);
        let settings: Value = serde_json::from_str(&guard_settings("/bin/kelpie")).unwrap();
        assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        assert_eq!(settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "'/bin/kelpie' hook --review");
    }

    #[test]
    fn the_guard_lets_git_log_diff_and_show_through_and_stops_every_other_command() {
        for allowed in ["git log -p origin/main..HEAD", "git diff origin/main...HEAD -- src/a.rs", "git show --stat 1a2b3c", "git log --oneline --format=\"%h %s\"", "git show HEAD:src/main.rs"] {
            assert!(read_only(allowed), "{allowed}");
            assert_eq!(guard(&json!({ "tool_input": { "command": allowed } }).to_string()), "");
        }
        for refused in [
            "git diff --output=/tmp/x", "git log --outp=x", "git diff --o'utput'=x", "git diff --ext-diff", "git diff > patch", "git log | sh",
            "git log; rm -rf .", "git log && git push", "git log $(touch x)", "git log `touch x`", "git -C /elsewhere log", "git -c core.pager=sh log",
            "git commit -m x", "git checkout .", "git push", "rm -rf src", "cargo test", "GIT_EXTERNAL_DIFF=x git diff", "git log\ntouch x", "(git log)",
        ] {
            assert!(!read_only(refused), "{refused}");
            let decision: Value = serde_json::from_str(&guard(&json!({ "tool_input": { "command": refused } }).to_string())).unwrap();
            assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny", "{refused}");
        }
        let decision: Value = serde_json::from_str(&guard("not json")).unwrap();
        assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny", "an input it cannot read is refused");
    }

    #[test]
    fn a_verdict_is_saved_from_its_verdict_line_with_the_sha_and_its_findings_counted() {
        let answer = "I read every commit.\n\nverdict: changes\n\n- src/pay.rs:41 — the refund is booked twice on retry; the customer is charged back double.\n  It happens when the hook times out.\n- src/pay.rs:88 — a typo in a log line; style only.\n";
        let recorded = record(answer, "0123456789abcdef").unwrap();
        assert!(recorded.starts_with("verdict: changes\nreviewed: 0123456789abcdef\n\n- src/pay.rs:41"), "{recorded}");
        assert!(!recorded.contains("I read every commit"));
        assert_eq!(parse_verdict(&recorded), Some(Verdict { ok: false, sha: "0123456789abcdef".to_string(), findings: 2 }));
        assert_eq!(parse_verdict(&recorded).unwrap().summary(), "2 findings");
        let ok = record("**verdict: ok**\n", "abc").unwrap();
        assert_eq!(parse_verdict(&ok), Some(Verdict { ok: true, sha: "abc".to_string(), findings: 0 }));
        assert_eq!(parse_verdict(&ok).unwrap().summary(), "ok");
        assert_eq!(record("Looks fine to me.", "abc"), None, "an answer without a verdict line is not a verdict");
        assert_eq!(parse_verdict("verdict: maybe\nreviewed: abc\n"), None);
        assert_eq!(Verdict { ok: false, sha: String::new(), findings: 1 }.summary(), "1 finding");
    }

    #[test]
    fn a_verdict_goes_stale_when_head_moves_and_is_gone_once_forgotten() {
        let state = std::env::temp_dir().join(format!("kelpie-verdicts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        assert_eq!(shown_in(&state, "billing@app", "aaa"), None);
        fs::create_dir_all(dir(&state)).unwrap();
        assert!(!verdict_file(&state, "billing@app").starts_with(state.join("reviews")), "the review split owns reviews/");
        fs::write(verdict_file(&state, "billing@app"), record("verdict: changes\n- a.rs:1 — wrong; breaks x\n", "aaa").unwrap()).unwrap();
        let current = shown_in(&state, "billing@app", "aaa").unwrap();
        assert_eq!((current.stale, current.running, current.verdict.as_ref().unwrap().findings), (false, false, 1));
        assert!(shown_in(&state, "billing@app", "bbb").unwrap().stale, "a new commit makes the verdict stale");
        assert_eq!(shown_in(&state, "billing@web", "aaa"), None);
        fs::write(running_file(&state, "billing@app"), std::process::id().to_string()).unwrap();
        assert!(shown_in(&state, "billing@app", "aaa").unwrap().running);
        fs::write(running_file(&state, "billing@app"), "999999999").unwrap();
        assert!(!shown_in(&state, "billing@app", "aaa").unwrap().running, "a review whose process is gone is not running");
        forget_in(&state, "billing@app");
        assert_eq!(shown_in(&state, "billing@app", "aaa"), None);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_brief_carries_the_prompt_and_what_the_user_said_later_without_tool_traffic() {
        let lines = [
            json!({ "type": "user", "message": { "content": "Add refunds to the billing page." } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "On it." }] } }),
            json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "content": "ok" }] } }),
            json!({ "type": "user", "isMeta": true, "message": { "content": "Caveat: generated" } }),
            json!({ "type": "user", "message": { "content": "<command-name>/clear</command-name>" } }),
            json!({ "type": "user", "message": { "content": [{ "type": "text", "text": "Refunds only within 30 days." }] } }),
        ];
        let transcript = lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\nnot json\n";
        let said = user_messages(&transcript);
        assert_eq!(said, ["Add refunds to the billing page.", "Refunds only within 30 days."]);
        let mut shape = Brief { address: "billing@app", worktree: Path::new("/src/wt1.app"), upstream: "origin/main", sha: "abc123", commits: "abc123 Add refunds", rules: "kelpie:work-rules", rules_file: Some(Path::new("/kelpie/skills/work-rules/SKILL.md")), said: &said };
        let text = brief(&shape);
        assert!(text.contains("origin/main..abc123:\nabc123 Add refunds"), "{text}");
        assert!(text.contains("`kelpie:work-rules`, at /kelpie/skills/work-rules/SKILL.md: read it"), "{text}");
        shape.rules_file = None;
        assert!(brief(&shape).contains("kelpie found no file for it"));
        assert!(text.contains("started with:\n\nAdd refunds to the billing page.") && text.contains("---\nRefunds only within 30 days."), "{text}");
        assert!(conversation(&[]).contains("no record"));
        let many: Vec<String> = std::iter::once("prompt".to_string()).chain((0..20).map(|index| format!("{index}{}", "x".repeat(2_000)))).collect();
        let kept = conversation(&many);
        assert!(kept.contains("older ones left out") && kept.contains("19xx") && !kept.contains("\n0xx"), "the newest later messages are kept");
        assert!(kept.find("15xx").unwrap() < kept.find("19xx").unwrap(), "in the order they were said");
    }

    #[test]
    fn the_rules_skill_file_is_found_in_kelpie_another_plugin_the_repo_or_the_user_skills() {
        let root = std::env::temp_dir().join(format!("kelpie-skill-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (home, worktree, kelpie, plugin) = (root.join("claude"), root.join("wt1.app"), root.join("kelpie"), root.join("cache/house/1.0"));
        let skill = |dir: &Path, name: &str| { let file = dir.join("skills").join(name).join("SKILL.md"); fs::create_dir_all(file.parent().unwrap()).unwrap(); fs::write(&file, "---\n").unwrap(); file };
        let ours = skill(&kelpie, "work-rules");
        let theirs = skill(&plugin, "commit-style");
        let repo_own = skill(&worktree.join(".claude"), "repo-rules");
        let user_own = skill(&home, "my-rules");
        fs::create_dir_all(home.join("plugins")).unwrap();
        fs::write(home.join("plugins/installed_plugins.json"), json!({ "version": 2, "plugins": { "house@market": [{ "installPath": plugin }] } }).to_string()).unwrap();
        let find = |name: &str| skill_file(name, &worktree, &home, &kelpie);
        assert_eq!(find("kelpie:work-rules"), Some(ours));
        assert_eq!(find("house:commit-style"), Some(theirs));
        assert_eq!(find("repo-rules"), Some(repo_own));
        assert_eq!(find("my-rules"), Some(user_own));
        assert_eq!(find("house:missing"), None);
        assert_eq!(find("other:commit-style"), None);
        assert_eq!(find("missing"), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_review_skill_ships_where_the_reviewer_is_pointed() {
        let skill = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/review/SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: review\n"), "{skill}");
        assert!(skill.contains("verdict: ok") && skill.contains("verdict: changes"));
    }
}
