use std::time::Duration;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::herdr;
use crate::profiles::Profile;
use crate::sandbox::{Policy, Protected, TaskDirs};
use crate::sh::Result;
use crate::tasks::Session;

enum Refusal {
    NotYet,
    Starting,
    Final,
}

// A new pane's shell takes a moment to reach its prompt; until then herdr calls the pane busy, or on macOS measures the line against a shorter limit and asks for a retry; a Claude that just exited holds its agent name a moment longer.
fn refusal(error: &str) -> Refusal {
    if error.contains("agent_pane_busy") || error.contains("wait for the shell prompt and retry") || error.contains("agent_name_taken") {
        Refusal::NotYet
    } else if error.contains("agent_not_ready") || error.contains("timeout") {
        Refusal::Starting
    } else {
        Refusal::Final
    }
}

#[derive(Debug, PartialEq)]
enum Launched {
    Up(Started),
    ShellBack(String),
}

#[derive(Debug, PartialEq)]
pub enum Started {
    Ready,
    AwaitsTrust,
    AwaitsBypass,
    AwaitsHooks,
    BypassDisabled,
}

impl Started {
    // What Claude asks in its Space before it runs; the answer is the user's, so kelpie sends it no key.
    pub fn asks(&self) -> Option<&'static str> {
        match self {
            Started::AwaitsTrust => Some("whether you trust its folder"),
            Started::AwaitsBypass => Some("you to accept bypass permissions mode"),
            Started::AwaitsHooks => Some("whether to trust the hooks it found new or changed"),
            Started::Ready | Started::BypassDisabled => None,
        }
    }
}

// What kelpie reads on an agent's screen as it starts, and how herdr knows it.
pub(crate) struct Screens {
    pub kind: &'static str,
    pub name: &'static str,
    pub asks_trust: fn(&str) -> bool,
    pub trust_selected: fn(&str) -> bool,
    pub toward_trust: &'static str,
    pub held: fn(&str) -> Option<Started>,
    pub running: fn(&str) -> Started,
}

const CLAUDE: Screens = Screens { kind: "claude", name: "Claude", asks_trust: |screen| screen.contains("trust this folder"), trust_selected: yes_selected, toward_trust: "down", held: |screen| asks_for_bypass(screen).then_some(Started::AwaitsBypass), running };

// herdr types the agent's command into the pane's shell, so it is typed only once that shell is idle at its prompt.
pub fn start_claude(name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<Started> {
    start_with(&CLAUDE, name, pane, args, trusted)
}

pub(crate) fn start_with(screens: &Screens, name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<Started> {
    start_twice(screens, pane, || {
        wait_for_prompt(pane)?;
        type_agent(screens, name, pane, args, trusted)
    })
}

// A line read away by the shell's own startup, as oh-my-zsh's update question takes its first key, leaves the shell at its prompt: one more try then lands.
fn start_twice(screens: &Screens, pane: &str, mut attempt: impl FnMut() -> Result<Launched>) -> Result<Started> {
    let mut shown = String::new();
    for _ in 0..2 {
        match attempt()? {
            Launched::Up(started) => return Ok(started),
            Launched::ShellBack(line) => shown = line,
        }
    }
    Err(format!("{} did not start in pane {pane}: its shell is back at the prompt, showing \"{shown}\"; see what the shell printed there", screens.name))
}

fn type_agent(screens: &Screens, name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<Launched> {
    let mut last = String::new();
    for _ in 0..30 {
        match herdr::start_agent(name, screens.kind, pane, args) {
            Ok(()) => return Ok(Launched::Up((screens.running)(&herdr::read_pane(pane).unwrap_or_default()))),
            Err(error) => match refusal(&error) {
                Refusal::NotYet => last = error,
                Refusal::Starting => return wait_until_ready(screens, name, pane, trusted),
                Refusal::Final => return Err(format!("herdr would not start {} in pane {pane}: {error}", screens.name)),
            },
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err(format!("The shell in pane {pane} never became ready for {}; herdr said: {last}", screens.name))
}

fn last_line(screen: &str) -> &str {
    screen.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or_default()
}

fn asks(line: &str) -> bool {
    let line = line.to_lowercase();
    ["[y/n]", "(y/n)", "[yes/no]", "(yes/no)"].iter().any(|question| line.contains(question))
}

#[derive(Debug, PartialEq)]
enum Prompt {
    Waiting,
    Asking(String),
    Ready,
}

#[derive(Default)]
struct Settle {
    previous: Option<String>,
    quiet: usize,
}

const QUIET_LOOKS: usize = 4;

impl Settle {
    fn look(&mut self, at_shell: bool, screen: &str) -> Prompt {
        let line = last_line(screen);
        let unchanged = self.previous.as_deref() == Some(screen);
        self.previous = Some(screen.to_string());
        if !at_shell || line.is_empty() {
            self.quiet = 0;
            return Prompt::Waiting;
        }
        if asks(line) {
            self.quiet = 0;
            return Prompt::Asking(line.to_string());
        }
        self.quiet = if unchanged { self.quiet + 1 } else { 0 };
        if self.quiet >= QUIET_LOOKS { Prompt::Ready } else { Prompt::Waiting }
    }
}

pub(crate) fn wait_for_prompt(pane: &str) -> Result<()> {
    let look = || Ok((herdr::foreground(pane)?.is_none(), herdr::read_pane(pane)?));
    prompt_with(look, || std::thread::sleep(Duration::from_millis(250)), 120)?
        .map_err(|question| format!("The shell in pane {pane} asks \"{question}\" as it starts; answer it there, then start again"))
}

// A prompt that keeps redrawing never settles, so the wait ends without an error and the check after the start decides.
fn prompt_with(mut look: impl FnMut() -> Result<(bool, String)>, mut pause: impl FnMut(), tries: usize) -> Result<std::result::Result<(), String>> {
    let mut settle = Settle::default();
    let mut asking = None;
    for _ in 0..tries {
        let (at_shell, screen) = look()?;
        match settle.look(at_shell, &screen) {
            Prompt::Ready => return Ok(Ok(())),
            Prompt::Asking(question) => asking = Some(question),
            Prompt::Waiting => asking = None,
        }
        pause();
    }
    Ok(asking.map_or(Ok(()), Err))
}

// Escape clears what is typed or interrupts; Claude Code and Codex exit on their second ctrl-c, and a ctrl-c more would reach the shell.
pub fn end_claude(pane: &str) -> Result<()> {
    end_agent("Claude", pane)
}

pub(crate) fn end_agent(name: &str, pane: &str) -> Result<()> {
    end_with(|key| herdr::send_keys(pane, &[key]), || Ok(herdr::foreground(pane)?.is_none()), || std::thread::sleep(Duration::from_millis(500)), 10)
        .map_err(|_| format!("{name} in {pane} did not exit after escape and ctrl-c; see what holds it there"))
}

fn end_with(mut send: impl FnMut(&str) -> Result<()>, mut at_shell: impl FnMut() -> Result<bool>, mut pause: impl FnMut(), tries: usize) -> Result<()> {
    send("esc")?;
    pause();
    for _ in 0..tries {
        if at_shell()? {
            return Ok(());
        }
        send("ctrl+c")?;
        pause();
    }
    if at_shell()? { Ok(()) } else { Err("still running".to_string()) }
}

fn wait_until_ready(screens: &Screens, name: &str, pane: &str, trusted: bool) -> Result<Launched> {
    let look = || Ok((herdr::foreground(pane)?.is_none(), herdr::read_pane(pane)?));
    let press = |key: &str| herdr::send_keys(pane, &[key]);
    ready_with(screens, look, || herdr::agent_ready(name).unwrap_or(false), press, || std::thread::sleep(Duration::from_millis(500)), trusted, 120)?
        .ok_or_else(|| format!("{} in pane {pane} never became ready; see what it waits for there", screens.name))
}

// Claude wraps a dialog's lines to the pane's width and draws them inside a box.
fn flat(screen: &str) -> String {
    screen.split_whitespace().filter(|word| *word != "│").collect::<Vec<_>>().join(" ")
}

fn asks_for_bypass(screen: &str) -> bool {
    let flat = flat(screen);
    flat.contains("Bypass Permissions mode") && flat.contains("Yes, I accept")
}

// Managed settings with permissions.disableBypassPermissionsMode start Claude in another mode and say so on its screen.
fn running(screen: &str) -> Started {
    if flat(screen).contains("Bypass permissions mode was disabled") { Started::BypassDisabled } else { Started::Ready }
}

fn yes_selected(screen: &str) -> bool {
    screen.lines().any(|line| line.trim_start().trim_start_matches('│').trim_start().starts_with('❯') && line.contains("Yes, I trust this folder"))
}

// Claude asks about a folder it has not seen, and about bypass permissions mode until the user accepts it once, with "No, exit" selected; a prompt sent before it is ready is lost.
fn ready_with(screens: &Screens, mut look: impl FnMut() -> Result<(bool, String)>, mut ready: impl FnMut() -> bool, mut press: impl FnMut(&str) -> Result<()>, mut pause: impl FnMut(), trusted: bool, tries: usize) -> Result<Option<Launched>> {
    let mut at_shell = 0;
    for _ in 0..tries {
        let (shell, screen) = look()?;
        at_shell = if shell { at_shell + 1 } else { 0 };
        if at_shell >= QUIET_LOOKS {
            return Ok(Some(Launched::ShellBack(last_line(&screen).to_string())));
        }
        if let Some(held) = (screens.held)(&screen) {
            return Ok(Some(Launched::Up(held)));
        }
        if (screens.asks_trust)(&screen) {
            if !trusted {
                return Ok(Some(Launched::Up(Started::AwaitsTrust)));
            }
            press(if (screens.trust_selected)(&screen) { "enter" } else { screens.toward_trust })?;
        } else if ready() {
            return Ok(Some(Launched::Up((screens.running)(&screen))));
        }
        pause();
    }
    Ok(None)
}

// Claude Code's own record of the folders the user trusted, counting a folder under a trusted one.
pub fn trusted_by_user(dir: &Path) -> bool {
    let config = std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|_| std::env::var("HOME").map(PathBuf::from)).map(|dir| dir.join(".claude.json"));
    let claude = config.ok().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| serde_json::from_str::<Value>(&text).ok());
    claude.is_some_and(|claude| trusted_in(&claude, dir))
}

fn trusted_in(claude: &Value, dir: &Path) -> bool {
    dir.ancestors().any(|folder| folder.to_str().is_some_and(|folder| claude["projects"][folder]["hasTrustDialogAccepted"] == true))
}

// The deny rules and env rewrite name the rule, the push block holds when they are slipped, and the sandbox keeps the shell from passing as the user; gate is the task's address when kelpie keeps its note, whose Problem its edits then wait for.
pub(crate) fn settings(kelpie: &str, address: &str, gate: Option<&str>, dirs: Option<&TaskDirs>, sandbox: Option<&Policy>) -> String {
    let hook = json!([{ "hooks": [{ "type": "command", "command": format!("{} hook", crate::sh::quote(kelpie)) }] }]);
    let allow = ["peers", "tell", "report", "ask", "routine ok"].map(|command| format!("Bash({kelpie} {command}:*)"));
    let evidence = json!([{ "matcher": "Bash", "hooks": [{ "type": "command", "command": format!("{} hook evidence {}", crate::sh::quote(kelpie), crate::sh::quote(address)) }] }]);
    let mut hooks = json!({ "UserPromptSubmit": hook, "Stop": hook, "PostToolUse": evidence, "PostToolUseFailure": evidence });
    if let Some(address) = gate {
        hooks["PreToolUse"] = json!([{ "matcher": "Edit|Write|MultiEdit|NotebookEdit|Bash", "hooks": [{ "type": "command", "command": format!("{} hook gate {}", crate::sh::quote(kelpie), crate::sh::quote(address)), "timeout": 40 }] }]);
    }
    if let Some(dirs) = dirs.filter(|_| sandbox.is_some()) {
        let task = address.rsplit_once('@').map_or(address, |(task, _)| task);
        let files = json!({ "matcher": "Edit|Write|MultiEdit|NotebookEdit", "hooks": [{ "type": "command", "command": format!("{} hook files {} {} {}", crate::sh::quote(kelpie), crate::sh::quote(task), crate::sh::quote(&dirs.worktree.to_string_lossy()), crate::sh::quote(&dirs.tmp.to_string_lossy())) }] });
        match hooks["PreToolUse"].as_array_mut() {
            Some(entries) => entries.push(files),
            None => hooks["PreToolUse"] = json!([files]),
        }
    }
    let mut env = json!({ "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "url.kelpie-blocks-push:.pushInsteadOf", "GIT_CONFIG_VALUE_0": "" });
    if let Some(dirs) = dirs {
        for name in TMP_VARS {
            env[name] = json!(dirs.tmp.to_string_lossy());
        }
    }
    let mut deny = vec!["Bash(git push:*)".to_string()];
    deny.extend(crate::permit::USER_ONLY.iter().map(|command| format!("Bash({kelpie} {command}:*)")));
    deny.extend(["herdr pane send-keys", "herdr pane send-text", "herdr pane run", "herdr agent send-keys", "herdr agent prompt"].map(|command| format!("Bash({command}:*)")));
    if sandbox.is_some() {
        deny.extend(Protected::here().deny_rules(kelpie, dirs));
    }
    let mut written = json!({
        "env": env,
        "permissions": { "deny": deny, "allow": allow },
        "hooks": hooks,
    });
    if let Some(policy) = sandbox {
        written["sandbox"] = crate::sandbox::block(kelpie, policy, dirs, cfg!(target_os = "macos"));
    }
    written.to_string()
}

pub(crate) const TMP_VARS: [&str; 4] = ["TMPDIR", "TMP", "TEMP", "CLAUDE_CODE_TMPDIR"];

// A profile's rules go after kelpie's own, so kelpie's deny rules, hooks and env stay whatever the profile says.
pub(crate) fn with_profile(settings: &str, profile: &Profile) -> String {
    let mut written: Value = serde_json::from_str(settings).expect("kelpie writes its own task settings as JSON");
    for (key, rules) in [("deny", &profile.deny), ("allow", &profile.allow)] {
        if let Some(list) = written["permissions"][key].as_array_mut() {
            list.extend(rules.iter().map(|rule| json!(rule)));
        }
    }
    if !profile.plugins.is_empty() {
        written["enabledPlugins"] = profile.plugins.iter().map(|plugin| (plugin.clone(), json!(true))).collect::<serde_json::Map<_, _>>().into();
    }
    written.to_string()
}

pub(crate) fn session_args(session: &Session) -> [String; 2] {
    match session {
        Session::New(id) => ["--session-id".to_string(), id.clone()],
        Session::Resume(id) => ["--resume".to_string(), id.clone()],
    }
}

pub(crate) fn start_args(mode: Option<&str>, profile: &Profile, settings: &Path, role: &Path, session: &Session) -> Vec<String> {
    let mut args: Vec<String> = mode.map(|mode| vec!["--permission-mode".to_string(), mode.to_string()]).unwrap_or_default();
    for (flag, value) in [("--model", &profile.model), ("--effort", &profile.effort)] {
        if let Some(value) = value {
            args.extend([flag.to_string(), value.clone()]);
        }
    }
    args.extend(["--settings".to_string(), settings.to_string_lossy().to_string(), "--append-system-prompt-file".to_string(), role.to_string_lossy().to_string()]);
    args.extend(session_args(session));
    args
}

// Claude Code keeps a folder's conversations as <session id>.jsonl under its projects dir, the folder's path with every other character than a letter or digit as '-'.
pub(crate) fn transcripts_dir(claude_home: &Path, folder: &Path) -> PathBuf {
    let name: String = folder.to_string_lossy().chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '-' }).collect();
    claude_home.join("projects").join(name)
}

pub fn home() -> Option<PathBuf> {
    std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).ok().or_else(|| std::env::var("HOME").ok().map(|home| Path::new(&home).join(".claude")))
}

// What the user typed into the task's conversation, oldest first: its prompt and every later word, without tool results or kelpie's own wrappers.
pub fn user_messages(transcript: &str) -> Vec<String> {
    user_messages_at(transcript).into_iter().map(|(_, text)| text).collect()
}

pub fn user_messages_at(transcript: &str) -> Vec<(Option<u64>, String)> {
    transcript.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["type"] == "user" && entry["isMeta"] != true && entry["isSidechain"] != true)
        .filter_map(|entry| {
            let text = match &entry["message"]["content"] {
                Value::String(text) => text.clone(),
                Value::Array(blocks) if blocks.iter().all(|block| block["type"] == "text") => blocks.iter().filter_map(|block| block["text"].as_str()).collect::<Vec<_>>().join("\n"),
                _ => return None,
            };
            Some((entry["timestamp"].as_str().and_then(crate::typing::iso_millis), text.trim().to_string()))
        })
        .filter(|(_, text)| !text.is_empty() && !text.starts_with('<') && !text.starts_with("[Request interrupted"))
        .collect()
}

const SWITCHES: [&str; 20] = ["--allow-dangerously-skip-permissions", "--ax-screen-reader", "--bare", "--brief", "--chrome", "--no-chrome", "--dangerously-skip-permissions", "--disable-slash-commands", "--exclude-dynamic-system-prompt-sections", "--forward-subagent-text", "--ide", "--include-hook-events", "--include-partial-messages", "--no-session-persistence", "--replay-user-messages", "--restricted", "--safe-mode", "--strict-mcp-config", "--verbose", "--mcp-debug"];
const VALUES: [&str; 27] = ["--agent", "--agents", "--append-system-prompt", "--append-system-prompt-file", "--autocompact", "--debug-file", "--effort", "--environment", "--fallback-model", "--input-format", "--json-schema", "--max-budget-usd", "--max-turns", "--model", "-n", "--name", "--output-format", "--permission-mode", "--permission-prompts", "--permission-prompt-tool", "--plugin-dir", "--plugin-url", "--remote-control-session-name-prefix", "--setting-sources", "--settings", "--system-prompt", "--system-prompt-file"];
const OPTIONALS: [&str; 4] = ["-d", "--debug", "--prompt-suggestions", "--remote-control"];
const MANY: [&str; 9] = ["--add-dir", "--allowedTools", "--allowed-tools", "--betas", "--disallowedTools", "--disallowed-tools", "--file", "--mcp-config", "--tools"];
const DROPPED_SWITCHES: [&str; 4] = ["-c", "--continue", "--fork-session", "--tmux"];
const DROPPED_OPTIONALS: [&str; 7] = ["-r", "--resume", "--from-pr", "--teleport", "--cloud", "-w", "--worktree"];
const REFUSED: [&str; 9] = ["-p", "--print", "--bg", "--background", "--desktop", "-h", "--help", "-v", "--version"];

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// The process's own flags, without its conversation and prompt, then --resume <session>; an option kelpie cannot place is refused, since its next word could be a prompt that would be sent again.
pub fn resume_args(argv: &[String], session: &str) -> std::result::Result<Vec<String>, String> {
    let start = match argv {
        [program, ..] if base(program) == "claude" => 1,
        [program, script, ..] if ["node", "bun"].iter().any(|name| base(program).starts_with(name)) && (script.contains("claude") || script.ends_with("cli.js")) => 2,
        [] => return Err("its command line is empty".to_string()),
        [program, ..] => return Err(format!("its command is {}, not claude", base(program))),
    };
    let words = &argv[start..];
    let next_is_value = |index: usize| words.get(index).is_some_and(|word| !word.starts_with('-'));
    let mut kept = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let word = words[index].as_str();
        index += 1;
        if word == "--" {
            break;
        }
        if !word.starts_with('-') || word == "-" {
            continue;
        }
        let flag = word.split_once('=').map_or(word, |(flag, _)| flag);
        let inline = flag.len() < word.len();
        if REFUSED.contains(&flag) {
            return Err(format!("it runs with {flag}, which a resumed session cannot keep"));
        }
        if DROPPED_SWITCHES.contains(&flag) {
            continue;
        }
        if flag == "--session-id" {
            index += usize::from(!inline);
            continue;
        }
        if DROPPED_OPTIONALS.contains(&flag) {
            index += usize::from(!inline && next_is_value(index));
            continue;
        }
        kept.push(word.to_string());
        if SWITCHES.contains(&flag) || inline {
            continue;
        }
        if VALUES.contains(&flag) {
            let value = words.get(index).ok_or(format!("its {flag} has no value"))?;
            kept.push(value.clone());
            index += 1;
        } else if OPTIONALS.contains(&flag) {
            if next_is_value(index) {
                kept.push(words[index].clone());
                index += 1;
            }
        } else if MANY.contains(&flag) {
            while next_is_value(index) {
                kept.push(words[index].clone());
                index += 1;
            }
        } else {
            return Err(format!("it runs with {flag}, an option kelpie does not know"));
        }
    }
    kept.extend(["--resume".to_string(), session.to_string()]);
    Ok(kept)
}

const ROLE_FLAGS: [&str; 2] = ["--settings", "--append-system-prompt-file"];

fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let inline = format!("{flag}=");
    args.iter().enumerate().find_map(|(index, word)| if word == flag { args.get(index + 1).map(String::as_str) } else { word.strip_prefix(&inline) })
}

// A task's Claude without the settings and role kelpie writes, or whose role file is gone, has none of kelpie's hooks and never reports.
pub fn lacks_role(argv: &[String]) -> bool {
    ROLE_FLAGS.iter().any(|flag| value_of(argv, flag).is_none_or(|file| !Path::new(file).is_file()))
}

pub fn with_role(args: &[String], settings: &Path, role: &Path) -> Vec<String> {
    let mut kept = Vec::new();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if ROLE_FLAGS.contains(&word.as_str()) {
            words.next();
        } else if !ROLE_FLAGS.iter().any(|flag| word.starts_with(&format!("{flag}="))) {
            kept.push(word.clone());
        }
    }
    let at = kept.iter().rposition(|word| word == "--resume").unwrap_or(kept.len());
    kept.splice(at..at, ["--settings".to_string(), settings.to_string_lossy().to_string(), "--append-system-prompt-file".to_string(), role.to_string_lossy().to_string()]);
    kept
}

const REVIEW_TOOLS: &str = "Read,Grep,Glob,Bash";
const REVIEW_ALLOWED: [&str; 6] = ["Read", "Grep", "Glob", "Bash(git log:*)", "Bash(git diff:*)", "Bash(git show:*)"];

// Only these tools exist, only the allowed ones run, the guard hook stops any other shell command, and no user setting, hook or plugin joins in.
pub(crate) fn reviewer_args(reviewer: &super::Reviewer) -> Vec<String> {
    let super::Reviewer { model, effort, settings, prompt, rules, docs } = *reviewer;
    let mut args: Vec<String> = ["-p", "--setting-sources", "", "--output-format", "text", "--no-session-persistence", "--strict-mcp-config", "--permission-mode", "dontAsk", "--tools", REVIEW_TOOLS, "--allowedTools"]
        .map(str::to_string).to_vec();
    args.extend(REVIEW_ALLOWED.map(str::to_string));
    args.extend(["--settings".to_string(), settings.to_string_lossy().to_string(), "--append-system-prompt-file".to_string(), prompt.to_string_lossy().to_string()]);
    for dir in [rules, docs].into_iter().flatten() {
        args.extend(["--add-dir".to_string(), dir.to_string_lossy().to_string()]);
    }
    if let Some(model) = model {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    if let Some(effort) = effort {
        args.extend(["--effort".to_string(), effort.to_string()]);
    }
    args
}

pub(crate) fn reviewer_settings(kelpie: &str, sandbox: bool) -> String {
    let mut written = json!({ "hooks": { "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": format!("{} hook --review", crate::sh::quote(kelpie)) }] }] } });
    if sandbox {
        written["sandbox"] = crate::sandbox::reader_block();
    }
    written.to_string()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;

    #[test]
    fn ending_a_claude_sends_escape_then_ctrl_c_only_until_the_shell_is_back() {
        let run = |exits_after: usize, tries: usize| {
            let keys = std::cell::RefCell::new(Vec::new());
            let result = end_with(|key| { keys.borrow_mut().push(key.to_string()); Ok(()) }, || Ok(keys.borrow().iter().filter(|key| *key == "ctrl+c").count() >= exits_after), || {}, tries);
            (result, keys.into_inner())
        };
        assert_eq!(run(2, 10), (Ok(()), vec!["esc".to_string(), "ctrl+c".to_string(), "ctrl+c".to_string()]));
        assert_eq!(run(0, 10), (Ok(()), vec!["esc".to_string()]));
        let (result, keys) = run(usize::MAX, 3);
        assert!(result.is_err());
        assert_eq!(keys.len(), 4, "{keys:?}");
    }

    #[test]
    fn start_retries_while_herdr_asks_for_the_prompt_and_stops_on_any_other_refusal() {
        assert!(matches!(refusal("herdr agent start refused: agent pane p_1 is busy (agent_pane_busy)"), Refusal::NotYet));
        assert!(matches!(refusal("herdr agent start refused: agent name lead is already used; candidates: … (agent_name_taken)"), Refusal::NotYet), "a Claude that just exited still holds its name");
        assert!(matches!(refusal("herdr agent start refused: input contains a 1100-byte line but the terminal is in canonical mode with a 1024-byte limit; wait for the shell prompt and retry (agent_start_input_failed)"), Refusal::NotYet));
        assert!(matches!(refusal("herdr agent start refused: not ready (agent_not_ready)"), Refusal::Starting));
        assert!(matches!(refusal("herdr agent start refused: agent target pane p_1 not found (agent_pane_not_found)"), Refusal::Final));
    }

    fn waited(looks: &[(bool, &str)], tries: usize) -> (std::result::Result<(), String>, usize) {
        let mut seen = looks.iter().map(|(at_shell, screen)| (*at_shell, screen.to_string()));
        let mut taken = 0;
        let result = prompt_with(|| { taken += 1; Ok(seen.next().unwrap_or((true, String::new()))) }, || {}, tries).unwrap();
        (result, taken)
    }

    #[test]
    fn claude_is_typed_only_once_the_shell_has_shown_the_same_prompt_for_a_while() {
        let prompt = "~/app/.worktrees/1 main\n❯";
        let settled = [(true, prompt); QUIET_LOOKS + 1];
        assert_eq!(waited(&settled, 20), (Ok(()), QUIET_LOOKS + 1));
        let busy_then_prompt = [(false, ""), (true, ""), (false, "Updating"), (true, prompt), (true, prompt), (true, prompt), (true, prompt), (true, prompt)];
        assert_eq!(waited(&busy_then_prompt, 20), (Ok(()), busy_then_prompt.len()), "startup work and an empty screen keep the wait going");
        let redrawn = [(true, "❯"), (true, "❯ 1"), (true, "❯"), (true, "❯ 1")];
        assert_eq!(waited(&redrawn, 4), (Ok(()), 4), "a prompt that never settles ends the wait without an error");
    }

    #[test]
    fn a_shell_asking_a_question_as_it_starts_gets_no_keys_and_the_question_is_named() {
        let omz = "[oh-my-zsh] Would you like to update? [Y/n]";
        let (result, taken) = waited(&[(true, omz); 12], 12);
        assert_eq!((result, taken), (Err(omz.to_string()), 12), "a question that holds still is not a settled prompt");
        let answered = [(true, omz), (true, omz), (false, "git pull"), (true, "❯"), (true, "❯"), (true, "❯"), (true, "❯"), (true, "❯")];
        assert_eq!(waited(&answered, 20), (Ok(()), answered.len()), "once answered in the pane the wait goes on to the prompt");
        assert!(asks("Proceed? (y/N)") && asks("Continue [yes/no]") && !asks("~/app ❯") && !asks("% "));
    }

    #[test]
    fn a_start_whose_line_the_shell_read_away_is_tried_once_more_and_then_named_by_its_pane() {
        let run = |outcomes: Vec<Launched>| {
            let mut outcomes = outcomes.into_iter();
            let mut tries = 0;
            let result = start_twice(&CLAUDE, "w1-2", || { tries += 1; Ok(outcomes.next().unwrap()) });
            (result, tries)
        };
        assert_eq!(run(vec![Launched::Up(Started::Ready)]), (Ok(Started::Ready), 1));
        assert_eq!(run(vec![Launched::ShellBack("zsh: command not found: laude".to_string()), Launched::Up(Started::Ready)]), (Ok(Started::Ready), 2));
        assert_eq!(run(vec![Launched::Up(Started::AwaitsTrust)]), (Ok(Started::AwaitsTrust), 1), "a trust prompt waiting for the user is a start, not a failure");
        let (result, tries) = run(vec![Launched::ShellBack("❯".to_string()), Launched::ShellBack("❯".to_string())]);
        assert_eq!(tries, 2);
        assert!(result.unwrap_err().starts_with("Claude did not start in pane w1-2: its shell is back at the prompt, showing \"❯\""));
    }

    const TRUST: &str = "│ Do you trust this folder?\n│ ❯ No, exit\n│   Yes, I trust this folder";
    const TRUST_YES: &str = "│ Do you trust this folder?\n│   No, exit\n│ ❯ Yes, I trust this folder";

    fn readied(screens: &[&str], ready_after: usize, trusted: bool) -> (Option<Launched>, Vec<String>) {
        let mut screens = screens.iter().map(|screen| screen.to_string());
        let keys = std::cell::RefCell::new(Vec::new());
        let mut asked = 0;
        let result = ready_with(&CLAUDE, || Ok((false, screens.next().unwrap_or_default())), || { asked += 1; asked > ready_after }, |key| { keys.borrow_mut().push(key.to_string()); Ok(()) }, || {}, trusted, 20).unwrap();
        (result, keys.into_inner())
    }

    #[test]
    fn an_untrusted_folder_leaves_the_trust_prompt_to_the_user_and_a_trusted_one_is_answered() {
        assert_eq!(readied(&[TRUST, TRUST, TRUST], 0, false), (Some(Launched::Up(Started::AwaitsTrust)), Vec::new()), "no key reaches a trust prompt the user has not answered for this repo");
        assert_eq!(readied(&[TRUST, TRUST_YES, "❯ "], 0, true), (Some(Launched::Up(Started::Ready)), vec!["down".to_string(), "enter".to_string()]), "Claude Code 2.1.306 focuses No, exit first, inside a box");
        assert_eq!(readied(&["Do you trust this folder?\n❯ 1. Yes, I trust this folder\n  2. No, exit", "❯ "], 0, true), (Some(Launched::Up(Started::Ready)), vec!["enter".to_string()]), "Claude numbers its choices");
        assert_eq!(readied(&["", "", "❯ "], 2, false), (Some(Launched::Up(Started::Ready)), Vec::new()), "a folder Claude does not ask about is ready as before");
    }

    const BYPASS: &str = "╭──────────────────────────────────────────────────────────────╮\n│ WARNING: Claude Code running in Bypass Permissions mode      │\n│                                                              │\n│ In Bypass Permissions mode, Claude Code will not ask for     │\n│ your approval before running potentially dangerous commands. │\n│ This mode should only be used in a sandboxed container/VM    │\n│ that has restricted internet access and can easily be        │\n│ restored if damaged.                                         │\n│                                                              │\n│ By proceeding, you accept all responsibility for actions     │\n│ taken while running in Bypass Permissions mode.              │\n│                                                              │\n│ https://code.claude.com/docs/en/security                     │\n│                                                              │\n│ ❯ No, exit                                                   │\n│   Yes, I accept                                              │\n╰──────────────────────────────────────────────────────────────╯";

    #[test]
    fn the_bypass_permissions_warning_is_left_to_the_user_even_in_a_trusted_folder_and_a_policy_refusal_is_named() {
        for trusted in [false, true] {
            assert_eq!(readied(&[BYPASS, BYPASS], 0, trusted), (Some(Launched::Up(Started::AwaitsBypass)), Vec::new()), "accepting bypass permissions mode is the user's, so no key reaches it");
        }
        assert_eq!(readied(&[TRUST_YES, BYPASS], 0, true), (Some(Launched::Up(Started::AwaitsBypass)), vec!["enter".to_string()]), "Claude Code 2.1.306 shows the warning after the trust prompt");
        assert_eq!(Started::AwaitsBypass.asks(), Some("you to accept bypass permissions mode"));
        assert_eq!((Started::Ready.asks(), Started::BypassDisabled.asks()), (None, None));
        let disabled = "╭───╮\n│ > │\n╰───╯\n  ⏵⏵ accept edits on · Bypass permissions mode was disabled by settings";
        assert_eq!(readied(&[disabled], 0, false), (Some(Launched::Up(Started::BypassDisabled)), Vec::new()), "Claude Code 2.1.306 runs in another mode when managed settings disable bypass");
        assert_eq!(running("❯ "), Started::Ready);
        assert_eq!(crate::typing::draft(BYPASS), crate::typing::Draft::Unreadable, "a held prompt waits until the warning is answered");
    }

    #[test]
    fn a_folder_counts_as_trusted_when_it_or_a_folder_above_it_is() {
        let claude = json!({ "projects": { "/home/ada/code": { "hasTrustDialogAccepted": true }, "/home/ada/other": { "hasTrustDialogAccepted": false } } });
        assert!(trusted_in(&claude, Path::new("/home/ada/code")));
        assert!(trusted_in(&claude, Path::new("/home/ada/code/shop")));
        assert!(!trusted_in(&claude, Path::new("/home/ada/other/shop")));
        assert!(!trusted_in(&claude, Path::new("/home/ada")));
        assert!(!trusted_in(&json!({}), Path::new("/home/ada/code")));
    }

    #[test]
    fn the_settle_counts_quiet_looks_only_at_a_shell_prompt_that_asks_nothing() {
        let mut settle = Settle::default();
        assert_eq!(settle.look(false, "❯"), Prompt::Waiting);
        assert_eq!(settle.look(true, ""), Prompt::Waiting, "an empty screen is not a prompt");
        assert_eq!(settle.look(true, "Proceed? [y/N]"), Prompt::Asking("Proceed? [y/N]".to_string()));
        for _ in 0..QUIET_LOOKS {
            assert_eq!(settle.look(true, "❯"), Prompt::Waiting, "the first look at a new screen starts the count, the quiet ones add to it");
        }
        assert_eq!(settle.look(true, "❯"), Prompt::Ready);
        assert_eq!(settle.look(true, "❯ x"), Prompt::Waiting, "a change starts the count again");
        assert_eq!(last_line("a\n\n  b  \n\n"), "b");
        assert_eq!(last_line(""), "");
    }

    fn task_settings(sandbox: Option<&Policy>) -> Value {
        serde_json::from_str(&settings("/opt/kelpie/kelpie", "billing@app", None, Some(&dirs()), sandbox)).unwrap()
    }

    fn dirs() -> TaskDirs {
        TaskDirs::new(Path::new("/src/wt1.app"), Path::new("/src/app/.git/worktrees/1/kelpie-tmp"))
    }

    fn strings(value: &Value) -> Vec<String> {
        value.as_array().unwrap().iter().map(|item| item.as_str().unwrap().to_string()).collect()
    }

    // Claude Code's excludedCommands syntax: "x *" matches x with or without arguments, anything else matches exactly.
    fn excluded_matches(pattern: &str, command: &str) -> bool {
        match pattern.strip_suffix(" *") {
            Some(prefix) => command == prefix || command.starts_with(&format!("{prefix} ")),
            None => command == pattern,
        }
    }

    #[test]
    fn a_sandboxed_task_runs_its_shell_in_the_strict_sandbox_with_kelpies_own_commands_outside_it() {
        let written = task_settings(Some(&crate::profiles::built_in(crate::profiles::TASK).unwrap().sandbox));
        let sandbox = &written["sandbox"];
        assert_eq!((&sandbox["enabled"], &sandbox["allowUnsandboxedCommands"], &sandbox["failIfUnavailable"]), (&json!(true), &json!(false), &json!(true)), "no retry outside the sandbox, and no start without one");
        assert_eq!(strings(&sandbox["excludedCommands"]), crate::sandbox::TASK_COMMANDS.map(|command| format!("/opt/kelpie/kelpie {command} *")).to_vec(), "matched on the absolute path the role names");
        assert_eq!(strings(&sandbox["network"]["allowedDomains"]), crate::sandbox::DOMAINS.map(str::to_string).to_vec());
        assert_eq!(strings(&sandbox["filesystem"]["allowWrite"]), ["~/.cargo/registry", "~/.cargo/git", "~/.npm", "~/.cache", "/src/app/.git/worktrees/1/kelpie-tmp"]);
        let denied = strings(&sandbox["filesystem"]["denyWrite"]);
        for path in ["/src/app/.git/worktrees/1/config.worktree", "/src/app/.git/worktrees/1/kelpie-hooks"] {
            assert!(denied.contains(&path.to_string()), "the push block stays out of the shell's reach: {path}");
        }
        for path in ["/src/wt1.app/.git", "/src/app/.git/worktrees/1/commondir", "/src/app/.git/worktrees/1/gitdir"] {
            assert!(denied.contains(&path.to_string()), "the shell cannot point git at a config of its own: {path}");
        }
        let uid = unsafe { libc::getuid() };
        assert!(denied.contains(&format!("/private/tmp/claude-{uid}")) && denied.contains(&format!("/tmp/claude-{uid}")), "other Claude sessions' temp folders stay out of reach");
        for secret in ["~/.ssh", "~/.config/gh", "~/.git-credentials", "~/.netrc"] {
            assert!(strings(&sandbox["filesystem"]["denyRead"]).contains(&secret.to_string()), "{secret}");
        }
        assert_eq!(sandbox["network"]["allowLocalBinding"].as_bool(), cfg!(target_os = "macos").then_some(true));
        assert_eq!(crate::sandbox::block("/k", &Policy::default(), None, true)["network"]["allowLocalBinding"], json!(true), "dev servers listen on macOS");
        assert_eq!(crate::sandbox::block("/k", &Policy::default(), None, false)["network"].get("allowLocalBinding"), None);
    }

    #[test]
    fn the_ops_profile_takes_ssh_scp_and_rsync_out_of_the_sandbox_on_top_of_kelpies_own() {
        let ops = crate::profiles::built_in(crate::profiles::OPS).unwrap();
        let excluded = strings(&task_settings(Some(&ops.sandbox))["sandbox"]["excludedCommands"]);
        assert_eq!(excluded[..crate::sandbox::TASK_COMMANDS.len()], crate::sandbox::TASK_COMMANDS.map(|command| format!("/opt/kelpie/kelpie {command} *")));
        assert_eq!(excluded[crate::sandbox::TASK_COMMANDS.len()..], ["ssh *", "scp *", "rsync *"]);
    }

    #[test]
    fn no_profile_takes_a_user_only_command_out_of_the_sandbox() {
        let ops = crate::profiles::built_in(crate::profiles::OPS).unwrap();
        for policy in [Policy::default(), ops.sandbox] {
            for pattern in strings(&task_settings(Some(&policy))["sandbox"]["excludedCommands"]) {
                for command in crate::permit::USER_ONLY {
                    for spelled in [format!("/opt/kelpie/kelpie {command}"), format!("/opt/kelpie/kelpie {command} billing"), format!("kelpie {command}")] {
                        assert!(!excluded_matches(&pattern, &spelled), "{pattern} would run {spelled} outside the sandbox");
                    }
                }
            }
        }
        assert!(excluded_matches("/k report *", "/k report --state ready x") && excluded_matches("/k report *", "/k report") && !excluded_matches("/k report *", "/k reports"));
    }

    #[test]
    fn every_kelpie_command_a_task_is_told_to_run_leaves_the_sandbox_or_is_the_users() {
        let told = [include_str!("../../roles/task.md"), include_str!("../peers.rs"), include_str!("../report.rs"), include_str!("../routines.rs")].concat();
        let excluded = crate::sandbox::excluded("{kelpie}", &Policy::default());
        let mut seen = Vec::new();
        for (at, _) in told.match_indices("{kelpie} ") {
            let words: Vec<&str> = told[at + 9..].split(|character: char| !character.is_ascii_lowercase() && character != '-').take(2).collect();
            let command = if ["routine", "backlog", "ci", "issue"].contains(&words[0]) { words.join(" ") } else { words[0].to_string() };
            let line = format!("{{kelpie}} {command}");
            let user_only = crate::permit::USER_ONLY.iter().any(|guarded| command == *guarded || command.starts_with(&format!("{guarded} ")));
            assert!(excluded.iter().any(|pattern| excluded_matches(pattern, &line)) || user_only || command == "done", "a task is told to run {line}, which the sandbox would cut off from kelpie's state");
            seen.push(command);
        }
        for command in ["report", "ask", "tell", "peers", "routine ok", "link", "backlog add"] {
            assert!(seen.iter().any(|seen| seen == command), "{command} is no longer in the task's instructions; drop it from TASK_COMMANDS");
        }
    }

    #[test]
    fn a_sandboxed_tasks_file_tools_never_write_what_runs_as_the_user() {
        let written = task_settings(Some(&Policy::default()));
        let deny = strings(&written["permissions"]["deny"]);
        let state = crate::lead::state_location().unwrap();
        let paths = [
            format!("/{}/**", state.display()), "~/.config/kelpie/**".to_string(), "~/.config/herdr/**".to_string(), "~/.claude/settings*.json".to_string(), "~/.zshrc".to_string(), "~/.bashrc".to_string(),
            "//opt/kelpie/kelpie".to_string(), "~/.claude.json".to_string(), "~/.claude/hooks/**".to_string(), "~/.claude/skills/**".to_string(), "~/.claude/agents/**".to_string(), "~/.claude/commands/**".to_string(),
            "~/.gitconfig".to_string(), "~/.config/git/**".to_string(), "~/.codex/**".to_string(), "~/.ssh/**".to_string(), "~/Library/LaunchAgents/**".to_string(), "~/.local/bin/**".to_string(),
            "//src/wt1.app/.claude/settings.json".to_string(), "//src/wt1.app/.claude/settings.local.json".to_string(), "//src/wt1.app/.claude/hooks/**".to_string(), "//src/wt1.app/.mcp.json".to_string(), "//src/wt1.app/.git".to_string(),
            "//src/app/.claude/**".to_string(), "//src/app/.mcp.json".to_string(), "//src/app/.git/config".to_string(), "//src/app/.git/hooks/**".to_string(), "//src/app/.git/worktrees/*/commondir".to_string(), "//src/app/.git/worktrees/1/config.worktree".to_string(),
        ];
        for path in paths {
            assert!(deny.contains(&format!("Edit({path})")), "Edit({path}) in {deny:?}");
        }
        assert!(!deny.iter().any(|rule| (rule.starts_with("Write(") || rule.starts_with("MultiEdit(")) && rule.contains('/')), "Claude Code never consults a Write or MultiEdit path rule: {deny:?}");
        assert!(!deny.iter().any(|rule| rule.starts_with("Edit(/") && !rule.starts_with("Edit(//")), "a single leading slash anchors at the settings file, not at /: {deny:?}");
        assert!(!deny.contains(&"Edit(//src/wt1.app/**)".to_string()) && !deny.iter().any(|rule| rule.starts_with("Edit(//src/app/.git/worktrees/1/kelpie-tmp")), "the worktree and the temp folder stay the task's to write");
    }

    #[test]
    fn with_the_sandbox_off_a_task_keeps_its_unsandboxed_settings_with_no_file_guard() {
        let off = task_settings(None);
        assert_eq!(off.get("sandbox"), None, "kelpie writes no sandbox key, so the user's own settings decide");
        assert!(!strings(&off["permissions"]["deny"]).iter().any(|rule| rule.starts_with("Edit(") || rule.starts_with("Write(") || rule.starts_with("MultiEdit(")), "{:?}", off["permissions"]["deny"]);
        assert!(off["hooks"].get("PreToolUse").is_none(), "no file hook");
    }

    #[test]
    fn a_sandboxed_task_runs_the_file_hook_on_its_own_worktree_and_temp_folder() {
        let written = task_settings(Some(&Policy::default()));
        let entries = written["hooks"]["PreToolUse"].as_array().unwrap();
        let files = entries.iter().find(|entry| entry["matcher"] == "Edit|Write|MultiEdit|NotebookEdit").unwrap();
        assert_eq!(files["hooks"][0]["command"], "'/opt/kelpie/kelpie' hook files 'billing' '/src/wt1.app' '/src/app/.git/worktrees/1/kelpie-tmp'", "the task's name finds its halves in other repos");
        let gated: Value = serde_json::from_str(&settings("/opt/kelpie/kelpie", "billing@app", Some("billing@app"), Some(&dirs()), Some(&Policy::default()))).unwrap();
        let matchers: Vec<&str> = gated["hooks"]["PreToolUse"].as_array().unwrap().iter().filter_map(|entry| entry["matcher"].as_str()).collect();
        assert_eq!(matchers, ["Edit|Write|MultiEdit|NotebookEdit|Bash", "Edit|Write|MultiEdit|NotebookEdit"], "the vault's gate and the file hook both run");
        let off: Value = serde_json::from_str(&settings("/opt/kelpie/kelpie", "billing@app", Some("billing@app"), Some(&dirs()), None)).unwrap();
        assert_eq!(off["hooks"]["PreToolUse"].as_array().unwrap().len(), 1, "only the vault's gate without the sandbox");
    }

    #[test]
    fn a_task_across_repos_takes_every_halfs_worktree_as_its_own_in_the_deny_rules() {
        let mut dirs = dirs();
        dirs.halves.push(TaskDirs::new(Path::new("/src/wt2.api"), Path::new("/src/api/.git/worktrees/2/kelpie-tmp")));
        let deny = Protected { path: vec![PathBuf::from("/src")], install: Some(PathBuf::from("/src/wt2.api")), ..Protected::here() }.deny_rules("/opt/kelpie/kelpie", Some(&dirs));
        for rule in ["Edit(//src/wt2.api/.claude/settings.json)", "Edit(//src/wt2.api/.mcp.json)", "Edit(//src/api/.git/config)", "Edit(//src/api/.claude/**)"] {
            assert!(deny.contains(&rule.to_string()), "{rule} in {deny:?}");
        }
        assert!(!deny.iter().any(|rule| rule == "Edit(//src/**)" || rule == "Edit(//src/wt2.api/**)"), "no folder that holds a half's worktree is denied: {deny:?}");
    }

    #[test]
    fn the_reviewer_reads_in_a_sandbox_with_nothing_outside_it() {
        let on: Value = serde_json::from_str(&reviewer_settings("/opt/kelpie/kelpie", true)).unwrap();
        assert_eq!((&on["sandbox"]["enabled"], &on["sandbox"]["allowUnsandboxedCommands"], &on["sandbox"]["failIfUnavailable"]), (&json!(true), &json!(false), &json!(true)));
        assert_eq!((&on["sandbox"]["excludedCommands"], &on["sandbox"]["network"]["allowedDomains"], &on["sandbox"]["filesystem"]["allowWrite"]), (&json!([]), &json!([]), &json!([])));
        assert_eq!(on["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "'/opt/kelpie/kelpie' hook --review");
        let off: Value = serde_json::from_str(&reviewer_settings("/opt/kelpie/kelpie", false)).unwrap();
        assert_eq!(off.get("sandbox"), None);
    }
}
