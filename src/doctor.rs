use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::{self, Machine, Setting, Source, Worktree};
use crate::sh::run;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, PartialEq)]
pub struct Check {
    pub level: Level,
    pub name: String,
    pub fact: String,
    pub fix: Vec<String>,
}

fn ok(name: &str, fact: impl Into<String>) -> Check {
    Check { level: Level::Ok, name: name.to_string(), fact: fact.into(), fix: Vec::new() }
}

fn warn(name: &str, fact: impl Into<String>, fix: Vec<String>) -> Check {
    Check { level: Level::Warn, name: name.to_string(), fact: fact.into(), fix }
}

fn fail(name: &str, fact: impl Into<String>, fix: Vec<String>) -> Check {
    Check { level: Level::Fail, name: name.to_string(), fact: fact.into(), fix }
}

fn both(mac: &str, linux: &str) -> Vec<String> {
    vec![format!("macOS: {mac}"), format!("Linux: {linux}")]
}

fn one(fix: impl Into<String>) -> Vec<String> {
    vec![fix.into()]
}

pub struct Tool {
    pub name: &'static str,
    pub required: bool,
    pub feature: &'static str,
    pub min: Option<String>,
    pub mac: &'static str,
    pub linux: &'static str,
}

const MANIFEST: &str = include_str!("../herdr-plugin.toml");

pub fn min_herdr() -> String {
    let manifest: toml::Table = MANIFEST.parse().unwrap_or_default();
    manifest.get("min_herdr_version").and_then(toml::Value::as_str).unwrap_or("0.0.0").to_string()
}

pub fn tools(min_herdr: &str) -> Vec<Tool> {
    let tool = |name, required, feature, min: Option<&str>, mac, linux| Tool { name, required, feature, min: min.map(str::to_string), mac, linux };
    vec![
        tool("git", true, "everything", None, "xcode-select --install (or brew install git)", "sudo apt install git"),
        tool("herdr", true, "everything", Some(min_herdr), "brew install herdr", "curl -fsSL https://herdr.dev/install.sh | sh"),
        tool("claude", true, "everything", None, "curl -fsSL https://claude.ai/install.sh | bash", "curl -fsSL https://claude.ai/install.sh | bash"),
        tool("jq", true, "the sidebar sync and the review hook", None, "brew install jq", "sudo apt install jq"),
        tool("fzf", false, "the review split's file list (needs --id-nth)", Some("0.71.0"), "brew install fzf (brew upgrade fzf when older)", "git clone --depth 1 https://github.com/junegunn/fzf.git ~/.fzf && ~/.fzf/install (distro packages are older than 0.71)"),
        tool("nvim", false, "the review split's diffs", Some("0.10.0"), "brew install neovim", "brew install neovim, or a release from https://github.com/neovim/neovim/releases (distro packages are often older than 0.10)"),
        tool("tig", false, "the history split", None, "brew install tig", "sudo apt install tig"),
        tool("curl", false, "the review list reloading when files change", None, "brew install curl", "sudo apt install curl"),
        tool("cargo", false, "herdr building the kelpie plugin", None, "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh", "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"),
    ]
}

pub fn which(name: &str, path: &[PathBuf]) -> Option<PathBuf> {
    path.iter().map(|dir| dir.join(name)).find(|file| std::fs::metadata(file).is_ok_and(|meta| meta.is_file() && meta.mode() & 0o111 != 0))
}

pub fn version(text: &str) -> Option<Vec<u64>> {
    let word = text.split_whitespace().map(|word| word.trim_start_matches('v')).find(|word| word.starts_with(|character: char| character.is_ascii_digit()) && word.contains('.'))?;
    let parts: Vec<u64> = word.split('.').map_while(|part| part.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()).collect();
    (!parts.is_empty()).then_some(parts)
}

fn at_least(have: &[u64], want: &[u64]) -> bool {
    let pad = |parts: &[u64]| (0..3).map(|index| parts.get(index).copied().unwrap_or(0)).collect::<Vec<_>>();
    pad(have) >= pad(want)
}

fn shown(parts: &[u64]) -> String {
    parts.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

pub fn tool(tool: &Tool, found: Option<&Path>, version_output: Option<&str>) -> Check {
    let missing = |fact: String, fix: Vec<String>| if tool.required { fail(tool.name, fact, fix) } else { warn(tool.name, fact, fix) };
    let Some(found) = found else {
        let fact = if tool.required { "not on PATH".to_string() } else { format!("not on PATH; needed for {}", tool.feature) };
        return missing(fact, both(tool.mac, tool.linux));
    };
    let Some(min) = &tool.min else { return ok(tool.name, found.display().to_string()) };
    let want = version(min).unwrap_or_default();
    match version_output.and_then(version) {
        Some(have) if at_least(&have, &want) => ok(tool.name, format!("{} at {}", shown(&have), found.display())),
        Some(have) => missing(format!("{} is older than {min}, needed for {}", shown(&have), tool.feature), both(tool.mac, tool.linux)),
        None => missing(format!("cannot read the version from {} --version; need {min}", found.display()), both(tool.mac, tool.linux)),
    }
}

pub struct Plugin {
    pub enabled: bool,
    pub root: PathBuf,
}

pub fn herdr_plugin(list: Option<&str>) -> (Check, Option<Plugin>) {
    let register = both("herdr plugin install gronare/kelpie (or herdr plugin link <a kelpie checkout>)", "herdr plugin install gronare/kelpie (or herdr plugin link <a kelpie checkout>)");
    let Some(list) = list else { return (fail("herdr plugin", "herdr plugin list --json failed", one("install herdr first")), None) };
    let value: Value = serde_json::from_str(list).unwrap_or_default();
    let found = value["result"]["plugins"].as_array().into_iter().flatten().find(|plugin| plugin["plugin_id"] == "kelpie");
    let Some(found) = found else { return (fail("herdr plugin", "kelpie is not registered with herdr", register), None) };
    let plugin = Plugin { enabled: found["enabled"] == true, root: PathBuf::from(found["plugin_root"].as_str().unwrap_or_default()) };
    let version = found["version"].as_str().unwrap_or("?");
    let check = if plugin.enabled {
        ok("herdr plugin", format!("kelpie {version} enabled, at {}", plugin.root.display()))
    } else {
        fail("herdr plugin", format!("kelpie {version} is registered but disabled"), one("herdr plugin enable kelpie"))
    };
    (check, Some(plugin))
}

pub fn updates(state: &Path, installed: &[crate::update_check::Component]) -> Check {
    let (latest, fact) = crate::update_check::doctor_line(state, installed);
    let check = ok("updates", fact);
    if latest { check } else { check.with(one("kelpie update")) }
}

pub fn plugin_binary(root: &Path) -> Check {
    let binary = root.join("target/release/kelpie");
    if binary.is_file() {
        ok("plugin binary", binary.display().to_string())
    } else {
        fail("plugin binary", format!("{} is not built", binary.display()), one(format!("cargo build --release in {} (herdr runs it on install and update)", root.display())))
    }
}

pub fn claude_plugin(installed: Option<&Value>, settings: &Value) -> Check {
    let install = one("claude plugin marketplace add https://github.com/gronare/kelpie && claude plugin install kelpie@kelpie");
    let Some(installed) = installed else { return warn("claude plugin", "no installed_plugins.json; the review skills and hook are missing", install) };
    let Some((id, installs)) = installed["plugins"].as_object().into_iter().flatten().find(|(id, _)| id.starts_with("kelpie@")) else {
        return warn("claude plugin", "kelpie is not installed in Claude Code; the review skills and hook are missing", install);
    };
    let version = installs[0]["version"].as_str().unwrap_or("?");
    if settings["enabledPlugins"][id] == false {
        return warn("claude plugin", format!("{id} {version} is installed but disabled"), one(format!("claude plugin enable {id}")));
    }
    ok("claude plugin", format!("{id} {version}"))
}

// Kelpie and the lead's role read the link under $HOME; scripts/herdr-startup.sh writes it under $XDG_DATA_HOME.
pub fn current_link(home: &Path, data_home: Option<&Path>, plugin_root: Option<&Path>) -> Check {
    let link = home.join(".local/share/kelpie/current");
    let start = one("start herdr once: the kelpie plugin's startup (scripts/herdr-startup.sh) links it");
    if let Some(data) = data_home.filter(|data| *data != home.join(".local/share")) {
        return warn("current link", format!("XDG_DATA_HOME is {}, so herdr links {}/kelpie/current while kelpie reads {}", data.display(), data.display(), link.display()), one("unset XDG_DATA_HOME for herdr, or link it yourself: ln -sfn <plugin root> ~/.local/share/kelpie/current"));
    }
    if std::fs::symlink_metadata(&link).is_err() {
        return fail("current link", format!("{} is missing", link.display()), start);
    }
    let Ok(target) = link.canonicalize() else { return fail("current link", format!("{} points nowhere", link.display()), start) };
    match plugin_root.map(|root| root.canonicalize().unwrap_or_else(|_| root.to_path_buf())) {
        Some(root) if root != target => warn("current link", format!("{} points at {}, herdr's kelpie is at {}", link.display(), target.display(), root.display()), one("restart herdr so its startup relinks it")),
        _ => ok("current link", format!("{} → {}", link.display(), target.display())),
    }
}

const LINK_KELPIE: &str = "ln -s ~/.local/share/kelpie/current/target/release/kelpie ~/.local/bin/kelpie";

pub fn kelpie_on_path(home: &Path, path: &[PathBuf]) -> Check {
    let current = home.join(".local/share/kelpie/current/target/release/kelpie");
    let Some(found) = which("kelpie", path) else {
        return warn("kelpie on PATH", "not on PATH; the lead, tasks and hooks run kelpie by name", vec![LINK_KELPIE.to_string(), "make sure ~/.local/bin is on PATH".to_string()]);
    };
    let resolved = found.canonicalize().unwrap_or_else(|_| found.clone());
    match current.canonicalize() {
        Ok(binary) if binary == resolved => ok("kelpie on PATH", format!("{} → {}", found.display(), binary.display())),
        _ => warn("kelpie on PATH", format!("{} resolves to {}, not {}", found.display(), resolved.display(), current.display()), one(format!("remove {} or put ~/.local/bin ahead of it on PATH, then {LINK_KELPIE}", found.display()))),
    }
}

pub fn state_dirs(kelpie: &Path, own: bool, state_home: &Path, data_home: &Path) -> Vec<(PathBuf, &'static str)> {
    let about = if own { "backlog, messages, lead, review marks; set by KELPIE_STATE_DIR" } else { "backlog, messages, lead, review marks" };
    vec![
        (kelpie.to_path_buf(), about),
        (kelpie.join("roles"), "task and lead roles"),
        (kelpie.join("turns"), "task reports"),
        (kelpie.join("reviews"), "review splits"),
        (state_home.join("herdr-sync"), "sidebar sync"),
        (data_home.join("kelpie"), "the current link"),
    ]
}

fn nearest_existing(path: &Path) -> Option<&Path> {
    path.ancestors().find(|dir| std::fs::symlink_metadata(dir).is_ok())
}

pub fn writable(path: &Path, about: &str, uid: u32) -> Check {
    let name = "state dir";
    let Some(existing) = nearest_existing(path) else { return fail(name, format!("{}: no parent exists", path.display()), Vec::new()) };
    let Ok(meta) = std::fs::metadata(existing) else { return fail(name, format!("{} is a broken link", existing.display()), one(format!("rm {}", existing.display()))) };
    if !meta.is_dir() {
        return fail(name, format!("{} ({about}): {} is not a directory", path.display(), existing.display()), one(format!("mv {} {}.bak", existing.display(), existing.display())));
    }
    let mine = meta.uid() == uid;
    if (mine && meta.mode() & 0o200 != 0) || meta.mode() & 0o002 != 0 {
        let fact = if existing == path { format!("{} ({about})", path.display()) } else { format!("{} ({about}) is created on first use in {}", path.display(), existing.display()) };
        return ok(name, fact);
    }
    if mine {
        return fail(name, format!("{} ({about}): {} is not writable", path.display(), existing.display()), one(format!("chmod u+w {}", existing.display())));
    }
    let owner = if meta.uid() == 0 { "root".to_string() } else { format!("uid {}", meta.uid()) };
    fail(name, format!("{} ({about}): {} is owned by {owner}", path.display(), existing.display()), one(format!("sudo chown -R \"$(id -un)\" {}", existing.display())))
}

pub fn config_file(path: &Path) -> (Check, Option<Machine>) {
    match config::load_machine(path) {
        Ok(config) if path.exists() => (ok("config", format!("{} parses", path.display())), Some(config)),
        Ok(config) => (ok("config", format!("{} not found; every machine setting is its default", path.display())), Some(config)),
        Err(error) => (fail("config", error, one("fix that key; kelpie config shows the result")), None),
    }
}

pub fn repo_file(path: &Path) -> (Check, Option<Worktree>) {
    match config::load_repo(path) {
        Ok(worktree) if path.exists() => (ok("repo config", format!("{} parses", path.display())), Some(worktree)),
        Ok(worktree) => (ok("repo config", format!("{} not found; the machine config and defaults apply", path.display())), Some(worktree)),
        Err(error) => (fail("repo config", error, one("fix that key; kelpie config shows the result")), None),
    }
}

pub fn setup_skill(skill: &Setting<String>, installed: Option<&Value>, settings: &Value) -> Check {
    let name = "setup skill";
    let shown = format!("{} ({})", skill.value, skill.layer.label());
    let Some((plugin, _)) = skill.value.split_once(':') else {
        return ok(name, format!("{shown}: a user or project skill, not checked"));
    };
    let other = "or set worktree.setup_skill to a skill this machine has".to_string();
    let found = installed.and_then(|installed| installed["plugins"].as_object()).into_iter().flatten().find(|(id, _)| id.split_once('@').map(|(id, _)| id) == Some(plugin));
    let Some((id, entries)) = found else {
        return warn(name, format!("{shown}: the plugin {plugin} is not installed in Claude Code, so a task's Claude cannot run it"), vec![format!("claude plugin install {plugin}@<marketplace>"), other]);
    };
    if settings["enabledPlugins"][id] == false {
        return warn(name, format!("{shown}: {id} is installed but disabled"), vec![format!("claude plugin enable {id}"), other]);
    }
    let entry = entries.as_array().and_then(|entries| entries.last());
    if let (Some(root), Some((_, skill_name))) = (entry.and_then(|entry| entry["installPath"].as_str()), skill.value.split_once(':'))
        && !Path::new(root).join("skills").join(skill_name).join("SKILL.md").exists()
    {
        let version = entry.and_then(|entry| entry["version"].as_str()).unwrap_or("?");
        return warn(name, format!("{shown}: {id} {version} has no skill {skill_name}"), vec![format!("claude plugin update {id}"), other]);
    }
    ok(name, format!("{shown}: {id}"))
}

// Looks for the command's program in the main checkout and never runs it.
pub fn setup_command(command: &Setting<Option<String>>, repo: &Path) -> Option<Check> {
    let value = command.value.as_deref()?;
    let name = "setup command";
    let shown = format!("{value:?} ({})", command.layer.label());
    let program = value.split_whitespace().next().unwrap_or(value);
    if !program.contains('/') {
        return Some(ok(name, format!("{shown}: {program} is found on PATH when it runs, not checked")));
    }
    let path = repo.join(program);
    let fix = format!("add {program} to the main checkout, or set worktree.setup_command to the repo's prepare command");
    Some(match std::fs::metadata(&path) {
        Err(_) if command.layer == config::Layer::Machine => ok(name, format!("{shown}: this repo has no {program}, so kelpie's default setup runs here")),
        Err(_) => warn(name, format!("{shown}: {} not found, so the setup skill cannot run it", path.display()), one(fix)),
        Ok(meta) if !meta.is_file() || meta.mode() & 0o111 == 0 => warn(name, format!("{shown}: {} is not an executable file", path.display()), vec![format!("chmod +x {program}"), fix]),
        Ok(_) => ok(name, format!("{shown}: {}", path.display())),
    })
}

// Reports whether the key is set and never runs the command, so doctor sends nothing.
pub fn notify(command: &Setting<Option<String>>) -> Check {
    match &command.value {
        Some(value) => ok("notify", format!("notify.command {value:?} ({}): run when a task needs you or is ready, a question waits, or the lead notifies; failures go to notify.log in the state dir", command.layer.label())),
        None => ok("notify", "notify.command not set, so kelpie sends no notifications; set [notify] command to a shell command to get them"),
    }
}

const STATUS_LINE: &str = r#""statusLine": { "type": "command", "command": "~/.local/share/kelpie/current/target/release/kelpie statusline" }"#;

// Claude Code passes the plan's usage only to the statusLine command, so the board's usage bar needs kelpie there.
pub fn status_line(settings: &Value) -> Check {
    let command = settings["statusLine"]["command"].as_str().unwrap_or_default();
    if command.contains("kelpie") && command.contains("statusline") {
        return ok("status line", format!("{command:?} keeps the plan's usage for the board"));
    }
    let fact = if command.is_empty() { "no statusLine command in Claude Code's settings, so the board shows no plan usage".to_string() } else { format!("statusLine runs {command:?}, so the board shows no plan usage") };
    let fix = if command.is_empty() { format!("add to ~/.claude/settings.json: {STATUS_LINE}") } else { "have your statusLine command pipe its stdin to `kelpie statusline` as well (it prints nothing, so your line stays as it is), or use kelpie's, which shows no line: ".to_string() + STATUS_LINE };
    warn("status line", fact, one(fix))
}

pub fn push_block(repo: &Path) -> Check {
    match crate::push_block::unsupported(repo) {
        Ok(None) => ok("push block", format!("task worktrees of {} block git push", repo.display())),
        Ok(Some(why)) => warn("push block", why, one("move core.worktree, or core.bare = true, from the shared config into the main worktree's config.worktree, then restart the repo's tasks")),
        Err(error) => warn("push block", error, Vec::new()),
    }
}

pub fn backlog(sources: &[Source], vault_index: impl Fn() -> Result<PathBuf, String>, local_file: &Path) -> Vec<Check> {
    if sources.is_empty() {
        return vec![ok("backlog", "no source configured; set [board.backlog] sources to fill the board's Backlog tab")];
    }
    sources.iter().map(|source| match source {
        Source::Local if local_file.exists() => ok("backlog local", format!("{}: {} items", local_file.display(), crate::backlog_file::read(local_file).len())),
        Source::Local => ok("backlog local", format!("{} is created by the first item (the board's a, or kelpie backlog add)", local_file.display())),
        Source::Vault => match vault_index() {
            Ok(index) => ok("backlog vault", format!("vault index {}", index.display())),
            Err(error) => warn("backlog vault", error, one("open a Claude session with the vault plugin once; the Areas page reads its index")),
        },
        Source::Linear => ok("backlog linear", "the lead reads Linear through a Linear MCP server in Claude (not checked here)"),
    }).collect()
}

fn plugin_ids<'a>(installed: Option<&'a Value>, prefix: &'a str) -> impl Iterator<Item = (&'a String, &'a Value)> {
    installed.and_then(|installed| installed["plugins"].as_object()).into_iter().flatten().filter(move |(id, _)| id.starts_with(prefix))
}

// The vault plugin that runs notes-vault-mcp, by its marketplace name or its plugin files.
pub fn vault_plugin(installed: Option<&Value>, read: impl Fn(&Path) -> Option<String>) -> Option<String> {
    plugin_ids(installed, "vault@").find(|(id, entries)| {
        id.contains("notes-vault-mcp") || entries.as_array().into_iter().flatten().filter_map(|entry| entry["installPath"].as_str()).any(|root| {
            [".mcp.json", ".claude-plugin/plugin.json"].iter().any(|file| read(&Path::new(root).join(file)).is_some_and(|text| text.contains("notes-vault-mcp")))
        })
    }).map(|(id, _)| id.clone())
}

// A Linear plugin, or an MCP server named for Linear in Claude Code's user or project config; claude.ai connectors are not visible here.
pub fn linear_mcp(installed: Option<&Value>, claude: Option<&Value>) -> Option<String> {
    if let Some((id, _)) = plugin_ids(installed, "linear@").next() {
        return Some(format!("the plugin {id}"));
    }
    let claude = claude?;
    let servers = std::iter::once(&claude["mcpServers"]).chain(claude["projects"].as_object().into_iter().flatten().map(|(_, project)| &project["mcpServers"]));
    servers.filter_map(Value::as_object).flatten().map(|(name, _)| name).find(|name| name.to_lowercase().contains("linear")).map(|name| format!("the MCP server {name}"))
}

pub fn unused_sources(sources: &[Source], vault: Option<String>, linear: Option<String>, config: &Path) -> Vec<Check> {
    let suggest = |source: Source| {
        let names: Vec<String> = sources.iter().chain([&source]).map(|source| format!("{:?}", source.key())).collect();
        one(format!("to use it, set sources = [{}] under [board.backlog] in {}", names.join(", "), config.display()))
    };
    let mut checks = Vec::new();
    if let Some(id) = vault.filter(|_| !sources.contains(&Source::Vault)) {
        checks.push(ok("backlog vault", format!("the vault plugin {id} is installed in Claude Code, but the vault is not a backlog source")).with(suggest(Source::Vault)));
    }
    if let Some(found) = linear.filter(|_| !sources.contains(&Source::Linear)) {
        checks.push(ok("backlog linear", format!("Claude Code has {found}, but Linear is not a backlog source")).with(suggest(Source::Linear)));
    }
    checks
}

// Optional: without the plugin kelpie keeps its own backlog and state, so a missing plugin is no gap.
// override_bin is KELPIE_VAULT_BIN, which stands in for the plugin as it does for detect().
pub fn vault(plugin: &crate::vault_cli::Plugin, override_bin: Option<&str>, probe: Option<Result<crate::vault_cli::Probe, String>>) -> Check {
    use crate::vault_cli::{NEEDS_UPDATE, Plugin};
    let name = "vault";
    let shown = match (override_bin, plugin) {
        (Some(bin), _) => Plugin::Enabled { id: format!("KELPIE_VAULT_BIN={bin}"), version: "(in place of the plugin)".to_string(), root: PathBuf::new() },
        (None, plugin) => plugin.clone(),
    };
    match (&shown, probe) {
        (Plugin::Missing, _) => ok(name, "the vault plugin is not installed (optional): with it, kelpie keeps each task's note and the repos' logs in your Obsidian vault").with(one("kelpie setup vault")),
        (Plugin::Disabled { id, version }, _) => ok(name, format!("{id} {version} is installed but disabled (optional), so kelpie keeps tasks without the vault")).with(one(format!("claude plugin enable {id}"))),
        (Plugin::Enabled { id, version, .. }, None) => warn(name, format!("{id} {version} has no notes-vault-mcp command in its hooks, so kelpie cannot reach the vault"), vec![NEEDS_UPDATE.to_string(), "or set [vault] command in the machine config".to_string()]),
        (Plugin::Enabled { id, version, .. }, Some(Err(error))) => warn(name, format!("{id} {version}: the vault command failed ({error}), so kelpie keeps tasks without the vault"), one("check that uvx runs, or set [vault] command in the machine config")),
        (Plugin::Enabled { id, version, .. }, Some(Ok(probe))) if !probe.configured => warn(name, format!("{id} {version} is installed but no vault is configured"), vec![format!("claude plugin configure {id} shows its options: vault_path for a local folder, or the s3_ ones"), "kelpie setup vault walks through it".to_string()]),
        (Plugin::Enabled { id, version, .. }, Some(Ok(probe))) if !probe.tasks => warn(name, format!("{id} {version} (notes-vault-mcp {}) is older than the task and log commands: the vault is read-only to kelpie, so tasks get no notes and pushes no log lines", probe.version), one(NEEDS_UPDATE)),
        (Plugin::Enabled { id, version, .. }, Some(Ok(probe))) if probe.writable == Some(false) => warn(name, format!("{id} {version}: the {} vault is not writable, so kelpie keeps tasks without it", probe.backend), one("check the vault folder's permissions or the S3 key's write access")),
        (Plugin::Enabled { id, version, .. }, Some(Ok(probe))) => ok(name, format!("{id} {version} (notes-vault-mcp {}, {}): each task gets a note, edits wait for its Problem, pushes are logged, done closes it", probe.version, probe.backend)),
    }
}

impl Check {
    fn with(mut self, fix: Vec<String>) -> Self {
        self.fix = fix;
        self
    }
}

pub struct Env {
    pub path: Vec<PathBuf>,
    pub home: PathBuf,
    pub state_home: PathBuf,
    pub kelpie_state: PathBuf,
    pub own_state: bool,
    pub data_home: Option<PathBuf>,
    pub claude_dir: PathBuf,
    pub config: PathBuf,
    pub repo_config: Option<PathBuf>,
    pub repo: Option<PathBuf>,
    pub uid: u32,
}

impl Env {
    pub fn current() -> Self {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty()).map(PathBuf::from);
        Self {
            path: std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default(),
            state_home: var("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")),
            kelpie_state: crate::lead::state_location().unwrap_or_else(|_| home.join(".local/state/kelpie")),
            own_state: var(crate::lead::STATE_DIR_VAR).is_some(),
            data_home: var("XDG_DATA_HOME"),
            claude_dir: var("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            config: config::path(),
            repo_config: std::env::current_dir().ok().and_then(|cwd| config::repo_file(&cwd)),
            repo: std::env::current_dir().ok().and_then(|cwd| crate::repo::main_checkout(&cwd).ok()),
            uid: run("id", &["-u"], None).ok().and_then(|uid| uid.parse().ok()).unwrap_or(u32::MAX),
            home,
        }
    }
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok())
}

pub fn checks(env: &Env) -> Vec<Check> {
    let mut checks = Vec::new();
    let mut herdr = None;
    for tool in tools(&min_herdr()) {
        let found = which(tool.name, &env.path);
        let output = found.as_ref().filter(|_| tool.min.is_some()).and_then(|found| run(&found.to_string_lossy(), &["--version"], None).ok());
        if tool.name == "herdr" {
            herdr = found.clone();
        }
        checks.push(self::tool(&tool, found.as_deref(), output.as_deref()));
    }
    let list = herdr.and_then(|herdr| run(&herdr.to_string_lossy(), &["plugin", "list", "--json", "--plugin", "kelpie"], None).ok());
    let (check, plugin) = herdr_plugin(list.as_deref());
    checks.push(check);
    if let Some(plugin) = &plugin {
        checks.push(plugin_binary(&plugin.root));
    }
    let installed = read_json(&env.claude_dir.join("plugins/installed_plugins.json"));
    let claude_settings = read_json(&env.claude_dir.join("settings.json")).unwrap_or_default();
    checks.push(claude_plugin(installed.as_ref(), &claude_settings));
    checks.push(status_line(&claude_settings));
    checks.push(current_link(&env.home, env.data_home.as_deref(), plugin.as_ref().map(|plugin| plugin.root.as_path())));
    checks.push(kelpie_on_path(&env.home, &env.path));
    let places = crate::update_check::Places { root: plugin.as_ref().map(|plugin| plugin.root.clone()), claude_dir: env.claude_dir.clone() };
    checks.push(updates(&env.kelpie_state, &crate::update_check::installed(&places)));
    let data_home = env.data_home.clone().unwrap_or_else(|| env.home.join(".local/share"));
    for (dir, about) in state_dirs(&env.kelpie_state, env.own_state, &env.state_home, &data_home) {
        checks.push(writable(&dir, about, env.uid));
    }
    let (check, machine) = config_file(&env.config);
    checks.push(check);
    let own = match env.repo_config.as_deref().map(repo_file) {
        Some((check, own)) => {
            checks.push(check);
            own
        }
        None => Some(Worktree::default()),
    };
    checks.extend(env.repo.as_deref().map(push_block));
    let (Some(machine), Some(own)) = (machine, own) else {
        return checks;
    };
    let settings = config::settings(&own, &machine);
    checks.push(setup_skill(&settings.setup_skill, installed.as_ref(), &claude_settings));
    checks.extend(env.repo.as_deref().and_then(|repo| setup_command(&settings.setup_command, repo)));
    checks.push(notify(&settings.notify_command));
    let plugin = crate::vault_cli::plugin(&env.claude_dir);
    let override_bin = std::env::var(crate::vault_cli::BIN_VAR).ok().filter(|bin| !bin.is_empty());
    let reachable = override_bin.is_some() || matches!(plugin, crate::vault_cli::Plugin::Enabled { .. });
    let command = crate::vault_cli::command(override_bin.clone(), settings.vault_command.value.clone(), &plugin).filter(|_| reachable);
    checks.push(vault(&plugin, override_bin.as_deref(), command.map(|command| crate::vault_cli::Cli::new(command).probe(true))));
    checks.extend(backlog(&settings.sources.value, crate::vault::index_path, &env.config.with_file_name("backlog.md")));
    let claude = read_json(&env.claude_dir.join(".claude.json")).or_else(|| read_json(&env.home.join(".claude.json")));
    let vault = vault_plugin(installed.as_ref(), |file| std::fs::read_to_string(file).ok());
    checks.extend(unused_sources(&settings.sources.value, vault, linear_mcp(installed.as_ref(), claude.as_ref()), &env.config));
    checks
}

pub fn render(checks: &[Check]) -> String {
    let width = checks.iter().map(|check| check.name.len()).max().unwrap_or(0);
    let mut lines = Vec::new();
    for check in checks {
        let level = match check.level {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Fail => "fail",
        };
        lines.push(format!("{level:<4}  {:<width$}  {}", check.name, check.fact));
        for fix in &check.fix {
            lines.push(format!("{:<4}  {:<width$}    fix: {fix}", "", ""));
        }
    }
    let count = |level: Level| checks.iter().filter(|check| check.level == level).count();
    lines.push(String::new());
    lines.push(format!("{} failed, {} warnings, {} ok", count(Level::Fail), count(Level::Warn), count(Level::Ok)));
    lines.join("\n")
}

pub fn failed(checks: &[Check]) -> bool {
    checks.iter().any(|check| check.level == Level::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-doctor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn executable(path: &Path) {
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn uid(dir: &Path) -> u32 {
        std::fs::metadata(dir).unwrap().uid()
    }

    fn named<'a>(tools: &'a [Tool], name: &str) -> &'a Tool {
        tools.iter().find(|tool| tool.name == name).unwrap()
    }

    #[test]
    fn versions_are_read_from_each_tools_own_output() {
        assert_eq!(version("herdr 0.9.5"), Some(vec![0, 9, 5]));
        assert_eq!(version("NVIM v0.11.5\nBuild type: Release"), Some(vec![0, 11, 5]));
        assert_eq!(version("0.57.0 (brew)"), Some(vec![0, 57, 0]));
        assert_eq!(version("no version"), None);
        assert!(at_least(&[0, 9, 5], &[0, 9, 0]) && at_least(&[0, 10], &[0, 10, 0]) && !at_least(&[0, 57, 0], &[0, 71, 0]));
        assert_eq!(min_herdr(), "0.9.0");
    }

    #[test]
    fn tools_are_found_on_an_injected_path_and_checked_for_versions() {
        let dir = scratch("path");
        executable(&dir.join("herdr"));
        std::fs::write(dir.join("fzf"), "not executable").unwrap();
        let path = vec![dir.clone()];
        assert_eq!(which("herdr", &path), Some(dir.join("herdr")));
        assert_eq!(which("fzf", &path), None);
        let tools = tools("0.9.0");
        assert_eq!(tool(named(&tools, "herdr"), Some(&dir.join("herdr")), Some("herdr 0.9.5")).level, Level::Ok);
        let old = tool(named(&tools, "herdr"), Some(&dir.join("herdr")), Some("herdr 0.8.2"));
        assert_eq!((old.level, old.fact.as_str()), (Level::Fail, "0.8.2 is older than 0.9.0, needed for everything"));
        let fzf = tool(named(&tools, "fzf"), Some(&dir.join("fzf")), Some("0.57.0 (brew)"));
        assert_eq!(fzf.level, Level::Warn);
        assert!(fzf.fact.contains("review split"));
        let missing = tool(named(&tools, "jq"), None, None);
        assert_eq!((missing.level, missing.fix.len()), (Level::Fail, 2));
        assert!(missing.fix[0].starts_with("macOS: brew") && missing.fix[1].starts_with("Linux: "));
        assert!(tools.iter().all(|tool| tool.name != "setsid"), "the board fetches; nothing needs setsid");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_herdr_plugin_is_read_from_plugin_list() {
        let list = r#"{"result":{"plugins":[{"plugin_id":"kelpie","enabled":true,"plugin_root":"/src/kelpie","version":"0.11.0"}],"type":"plugin_list"}}"#;
        let (check, plugin) = herdr_plugin(Some(list));
        assert_eq!((check.level, check.fact.as_str()), (Level::Ok, "kelpie 0.11.0 enabled, at /src/kelpie"));
        assert_eq!(plugin.unwrap().root, PathBuf::from("/src/kelpie"));
        let (check, _) = herdr_plugin(Some(&list.replace("true", "false")));
        assert_eq!((check.level, check.fix[0].as_str()), (Level::Fail, "herdr plugin enable kelpie"));
        let (check, plugin) = herdr_plugin(Some(r#"{"result":{"plugins":[]}}"#));
        assert!(check.level == Level::Fail && plugin.is_none() && check.fix[0].contains("herdr plugin install"));
        let dir = scratch("plugin");
        assert_eq!(plugin_binary(&dir).level, Level::Fail);
        std::fs::create_dir_all(dir.join("target/release")).unwrap();
        executable(&dir.join("target/release/kelpie"));
        assert_eq!(plugin_binary(&dir).level, Level::Ok);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_claude_plugin_is_read_from_installed_plugins() {
        let installed: Value = serde_json::from_str(r#"{"version":2,"plugins":{"kelpie@kelpie":[{"version":"0.11.0"}]}}"#).unwrap();
        assert_eq!(claude_plugin(Some(&installed), &Value::Null).fact, "kelpie@kelpie 0.11.0");
        let disabled: Value = serde_json::from_str(r#"{"enabledPlugins":{"kelpie@kelpie":false}}"#).unwrap();
        assert_eq!(claude_plugin(Some(&installed), &disabled).fix, ["claude plugin enable kelpie@kelpie"]);
        let other: Value = serde_json::from_str(r#"{"plugins":{"vault@x":[{}]}}"#).unwrap();
        assert_eq!(claude_plugin(Some(&other), &Value::Null).level, Level::Warn);
        assert_eq!(claude_plugin(None, &Value::Null).level, Level::Warn);
    }

    #[test]
    fn the_current_link_must_resolve() {
        let home = scratch("home");
        let missing = current_link(&home, None, None);
        assert!(missing.level == Level::Fail && missing.fix[0].starts_with("start herdr once"));
        let root = home.join("plugin");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(home.join(".local/share/kelpie")).unwrap();
        std::os::unix::fs::symlink(home.join("gone"), home.join(".local/share/kelpie/current")).unwrap();
        assert!(current_link(&home, None, None).fact.ends_with("points nowhere"));
        std::fs::remove_file(home.join(".local/share/kelpie/current")).unwrap();
        std::os::unix::fs::symlink(&root, home.join(".local/share/kelpie/current")).unwrap();
        assert_eq!(current_link(&home, None, Some(&root)).level, Level::Ok);
        assert_eq!(current_link(&home, None, Some(&home)).level, Level::Warn);
        assert_eq!(current_link(&home, Some(&home.join("data")), Some(&root)).level, Level::Warn);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn kelpie_on_path_must_be_the_current_links_binary() {
        let home = scratch("on-path");
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let missing = kelpie_on_path(&home, std::slice::from_ref(&bin));
        assert!(missing.level == Level::Warn && missing.fix[0] == LINK_KELPIE && missing.fix[1].contains("~/.local/bin is on PATH"));
        let release = home.join("plugin/target/release");
        std::fs::create_dir_all(&release).unwrap();
        executable(&release.join("kelpie"));
        std::fs::create_dir_all(home.join(".local/share/kelpie")).unwrap();
        std::os::unix::fs::symlink(home.join("plugin"), home.join(".local/share/kelpie/current")).unwrap();
        std::os::unix::fs::symlink(home.join(".local/share/kelpie/current/target/release/kelpie"), bin.join("kelpie")).unwrap();
        assert_eq!(kelpie_on_path(&home, std::slice::from_ref(&bin)).level, Level::Ok);
        let stale = home.join("cargo/bin");
        std::fs::create_dir_all(&stale).unwrap();
        executable(&stale.join("kelpie"));
        let shadowed = kelpie_on_path(&home, &[stale.clone(), bin.clone()]);
        assert_eq!(shadowed.level, Level::Warn);
        assert!(shadowed.fact.contains(&stale.join("kelpie").canonicalize().unwrap().display().to_string()));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn state_dirs_must_be_writable_or_creatable() {
        let base = scratch("state");
        let me = uid(&base);
        let created = writable(&base.join("kelpie/roles"), "roles", me);
        assert!(created.level == Level::Ok && created.fact.contains("created on first use"), "{created:?}");
        let locked = base.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let check = writable(&locked.join("kelpie"), "x", me);
        assert_eq!((check.level, check.fix[0].clone()), (Level::Fail, format!("chmod u+w {}", locked.display())));
        let foreign = writable(&base.join("kelpie"), "x", me.wrapping_add(1));
        assert!(foreign.level == Level::Fail && foreign.fix[0].starts_with("sudo chown -R \"$(id -un)\" "), "{foreign:?}");
        std::fs::write(base.join("file"), "").unwrap();
        assert!(writable(&base.join("file/kelpie"), "x", me).fact.contains("is not a directory"));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(base);
        let dirs = state_dirs(Path::new("/s/kelpie"), false, Path::new("/s"), Path::new("/d"));
        assert!(dirs.iter().any(|(dir, _)| dir == Path::new("/s/herdr-sync")) && dirs.iter().any(|(dir, _)| dir == Path::new("/d/kelpie")));
        assert!(dirs.iter().all(|(dir, _)| dir != Path::new("/s/herdr-sync-fetch")));
    }

    #[test]
    fn an_own_state_dir_holds_every_kelpie_dir_and_says_where_it_came_from() {
        let dirs = state_dirs(Path::new("/probe/state"), true, Path::new("/s"), Path::new("/d"));
        let kelpie: Vec<&PathBuf> = dirs.iter().map(|(dir, _)| dir).filter(|dir| !dir.starts_with("/s") && !dir.starts_with("/d")).collect();
        assert_eq!(kelpie, [Path::new("/probe/state"), Path::new("/probe/state/roles"), Path::new("/probe/state/turns"), Path::new("/probe/state/reviews")]);
        assert!(!dirs.iter().any(|(dir, _)| dir.starts_with("/s/kelpie")));
        assert!(dirs[0].1.ends_with("set by KELPIE_STATE_DIR"));
    }

    #[test]
    fn the_machine_and_repo_configs_are_checked() {
        let dir = scratch("config");
        let file = dir.join("config.toml");
        assert!(config_file(&file).0.fact.ends_with("not found; every machine setting is its default"));
        std::fs::write(&file, "[board.backlog]\nsources = [\"jira\"]\n").unwrap();
        let (check, config) = config_file(&file);
        assert!(check.level == Level::Fail && config.is_none() && check.fact.contains("board.backlog.sources"));
        let own = dir.join(".kelpie.toml");
        assert_eq!(repo_file(&own).0.level, Level::Ok);
        std::fs::write(&own, "[worktree]\ndev_server = \"bin/dev\"\nport = 3000\n").unwrap();
        let (check, worktree) = repo_file(&own);
        assert!(check.level == Level::Fail && worktree.is_none() && check.fact.contains("worktree.port: unknown key"), "{check:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_status_line_check_advises_kelpies_command_and_never_fails() {
        let set = status_line(&serde_json::json!({ "statusLine": { "type": "command", "command": "~/.local/share/kelpie/current/target/release/kelpie statusline" } }));
        assert_eq!(set.level, Level::Ok);
        let unset = status_line(&serde_json::json!({}));
        assert_eq!(unset.level, Level::Warn);
        assert!(unset.fix[0].contains("kelpie statusline") && unset.fix[0].contains("~/.claude/settings.json"), "{:?}", unset.fix);
        let other = status_line(&serde_json::json!({ "statusLine": { "command": "~/bin/line" } }));
        assert!(other.level == Level::Warn && other.fact.contains("~/bin/line"), "{}", other.fact);
    }

    #[test]
    fn a_repo_whose_shared_config_keeps_the_push_block_off_is_a_warning() {
        let dir = scratch("push-block");
        crate::repo::git(&dir, &["init", "--quiet"]).unwrap();
        assert_eq!(push_block(&dir).level, Level::Ok);
        crate::repo::git(&dir, &["config", "core.worktree", &dir.to_string_lossy()]).unwrap();
        let off = push_block(&dir);
        assert!(off.level == Level::Warn && off.fact.contains("push block is off") && off.fix[0].contains("config.worktree"), "{off:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_setup_skills_plugin_must_be_installed_and_enabled() {
        let skill = |value: &str, layer: config::Layer| Setting { value: value.to_string(), layer };
        let installed: Value = serde_json::from_str(r#"{"version":2,"plugins":{"kelpie@kelpie":[{"version":"0.11.0"}],"my-tools@private":[{"version":"1.0.0"}]}}"#).unwrap();
        let default = setup_skill(&skill("kelpie:set-up-worktree", config::Layer::Default), Some(&installed), &Value::Null);
        assert_eq!((default.level, default.fact.as_str()), (Level::Ok, "kelpie:set-up-worktree (default): kelpie@kelpie"));
        assert_eq!(setup_skill(&skill("my-tools:set-up-worktree", config::Layer::Repo), Some(&installed), &Value::Null).level, Level::Ok);
        let missing = setup_skill(&skill("work-tools:set-up-worktree", config::Layer::Machine), Some(&installed), &Value::Null);
        assert_eq!(missing.level, Level::Warn);
        assert!(missing.fact.starts_with("work-tools:set-up-worktree (machine config): the plugin work-tools is not installed"), "{}", missing.fact);
        assert_eq!(missing.fix[0], "claude plugin install work-tools@<marketplace>");
        assert_eq!(setup_skill(&skill("kelpie:set-up-worktree", config::Layer::Default), None, &Value::Null).level, Level::Warn);
        let disabled: Value = serde_json::from_str(r#"{"enabledPlugins":{"my-tools@private":false}}"#).unwrap();
        let off = setup_skill(&skill("my-tools:set-up-worktree", config::Layer::Repo), Some(&installed), &disabled);
        assert_eq!((off.level, off.fix[0].as_str()), (Level::Warn, "claude plugin enable my-tools@private"));
        assert_eq!(setup_skill(&skill("set-up-worktree", config::Layer::Machine), None, &Value::Null).level, Level::Ok);
    }

    #[test]
    fn the_setup_skill_must_be_in_the_installed_plugin_version() {
        let dir = std::env::temp_dir().join(format!("kelpie-doctor-skill-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills/set-up-worktree")).unwrap();
        std::fs::write(dir.join("skills/set-up-worktree/SKILL.md"), "").unwrap();
        let installed = serde_json::json!({"plugins": {"kelpie@kelpie": [{"version": "0.12.0", "installPath": dir.to_str().unwrap()}]}});
        let skill = |value: &str| Setting { value: value.to_string(), layer: config::Layer::Default };
        assert_eq!(setup_skill(&skill("kelpie:set-up-worktree"), Some(&installed), &Value::Null).level, Level::Ok);
        let stale = setup_skill(&skill("kelpie:make-worktree-safe"), Some(&installed), &Value::Null);
        assert_eq!((stale.level, stale.fact.as_str(), stale.fix[0].as_str()), (Level::Warn, "kelpie:make-worktree-safe (default): kelpie@kelpie 0.12.0 has no skill make-worktree-safe", "claude plugin update kelpie@kelpie"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_set_setup_command_must_be_an_executable_in_the_main_checkout() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("setup-command");
        let command = |value: Option<&str>| Setting { value: value.map(str::to_string), layer: config::Layer::Repo };
        assert_eq!(setup_command(&command(None), &dir), None, "unset means bin/setup-worktree when it exists");
        let missing = setup_command(&command(Some("script/prepare_worktree --quiet")), &dir).unwrap();
        assert!(missing.level == Level::Warn && missing.fact.contains("script/prepare_worktree not found"), "{missing:?}");
        std::fs::create_dir_all(dir.join("script")).unwrap();
        let script = dir.join("script/prepare_worktree");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        let plain = setup_command(&command(Some("script/prepare_worktree")), &dir).unwrap();
        assert!(plain.level == Level::Warn && plain.fact.contains("not an executable file") && plain.fix[0] == "chmod +x script/prepare_worktree", "{plain:?}");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(setup_command(&command(Some("script/prepare_worktree")), &dir).unwrap().level, Level::Ok);
        assert_eq!(setup_command(&command(Some("make prepare")), &dir).unwrap().level, Level::Ok);
        let machine_wide = Setting { value: Some("script/elsewhere".to_string()), layer: config::Layer::Machine };
        let elsewhere = setup_command(&machine_wide, &dir).unwrap();
        assert!(elsewhere.level == Level::Ok && elsewhere.fact.contains("default setup runs here"), "{elsewhere:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn backlog_sources_are_checked_per_source() {
        let nowhere = Path::new("/nonexistent/kelpie/backlog.md");
        let none = backlog(&[], || unreachable!(), nowhere);
        assert_eq!((none.len(), none[0].level), (1, Level::Ok));
        let missing = backlog(&[Source::Vault, Source::Linear], || Err("No vault index at /x".to_string()), nowhere);
        assert_eq!((missing[0].level, missing[0].fact.as_str()), (Level::Warn, "No vault index at /x"));
        assert!(missing[1].level == Level::Ok && missing[1].fact.contains("Linear MCP"));
        assert_eq!(backlog(&[Source::Vault], || Ok(PathBuf::from("/i.sqlite")), nowhere)[0].level, Level::Ok);
        let local = backlog(&[Source::Local], || unreachable!(), nowhere);
        assert!(local[0].level == Level::Ok && local[0].fact.contains("is created by the first item"), "{:?}", local[0]);
        let dir = std::env::temp_dir().join(format!("kelpie-doctor-local-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("backlog.md"), "# Mine\n- [high] One (app)\n- Two\n").unwrap();
        assert!(backlog(&[Source::Local], || unreachable!(), &dir.join("backlog.md"))[0].fact.ends_with("backlog.md: 2 items"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_installed_vault_plugin_or_linear_mcp_that_is_not_a_source_is_suggested_with_its_config_line() {
        let installed = json!({ "plugins": { "vault@gronare": [{ "installPath": "/plugins/vault/1.0" }], "kelpie@kelpie": [{ "installPath": "/plugins/kelpie" }] } });
        let files = |file: &Path| (file == Path::new("/plugins/vault/1.0/.mcp.json")).then(|| r#"{"mcpServers":{"vault":{"command":"uvx","args":["notes-vault-mcp"]}}}"#.to_string());
        assert_eq!(vault_plugin(Some(&installed), files).as_deref(), Some("vault@gronare"));
        assert_eq!(vault_plugin(Some(&installed), |_| None), None, "a vault plugin of some other server is not this one");
        assert_eq!(vault_plugin(Some(&json!({ "plugins": { "vault@notes-vault-mcp": [] } })), |_| None).as_deref(), Some("vault@notes-vault-mcp"));
        assert_eq!(vault_plugin(None, files), None);
        assert_eq!(linear_mcp(Some(&json!({ "plugins": { "linear@claude-plugins": [] } })), None).as_deref(), Some("the plugin linear@claude-plugins"));
        assert_eq!(linear_mcp(None, Some(&json!({ "mcpServers": { "Linear": {} } }))).as_deref(), Some("the MCP server Linear"));
        assert_eq!(linear_mcp(None, Some(&json!({ "projects": { "/src/app": { "mcpServers": { "linear-server": {} } } } }))).as_deref(), Some("the MCP server linear-server"));
        assert_eq!(linear_mcp(Some(&installed), Some(&json!({ "mcpServers": { "github": {} } }))), None);
        let config = Path::new("/home/u/.config/kelpie/config.toml");
        let found = unused_sources(&[Source::Local], Some("vault@gronare".to_string()), Some("the MCP server Linear".to_string()), config);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].name.as_str(), found[0].level), ("backlog vault", Level::Ok));
        assert_eq!(found[0].fix, ["to use it, set sources = [\"local\", \"vault\"] under [board.backlog] in /home/u/.config/kelpie/config.toml"]);
        assert_eq!(found[1].fix, ["to use it, set sources = [\"local\", \"linear\"] under [board.backlog] in /home/u/.config/kelpie/config.toml"]);
        assert!(unused_sources(&[Source::Vault, Source::Linear], Some("vault@gronare".to_string()), Some("x".to_string()), config).is_empty(), "an enabled source is not suggested");
        assert!(unused_sources(&[Source::Local], None, None, config).is_empty());
    }

    #[test]
    fn the_vault_plugin_is_optional_and_an_old_one_reads_as_read_only() {
        use crate::vault_cli::{Plugin, Probe};
        let missing = vault(&Plugin::Missing, None, None);
        assert_eq!(missing.level, Level::Ok, "a user without the vault plugin sees no error");
        assert!(missing.fact.contains("(optional)") && missing.fix == ["kelpie setup vault"], "{missing:?}");
        let enabled = Plugin::Enabled { id: "vault@gronare".to_string(), version: "0.2.15".to_string(), root: "/x".into() };
        let probe = |configured: bool, tasks: bool, writable: Option<bool>| Some(Ok(Probe { configured, writable, tasks, version: "0.5.1".to_string(), backend: "s3".to_string() }));
        let old = vault(&enabled, None, probe(true, false, Some(true)));
        assert_eq!(old.level, Level::Warn);
        assert!(old.fact.contains("read-only") && old.fix == ["claude plugin update vault@gronare"], "{old:?}");
        assert!(vault(&enabled, None, probe(false, true, None)).fact.contains("no vault is configured"));
        assert!(vault(&enabled, None, probe(true, true, Some(false))).fact.contains("not writable"));
        assert_eq!(vault(&enabled, None, probe(true, true, Some(true))).level, Level::Ok);
        assert_eq!(vault(&Plugin::Disabled { id: "vault@gronare".to_string(), version: "0.2.15".to_string() }, None, None).level, Level::Ok);
        assert_eq!(vault(&enabled, None, Some(Err("uvx: not found".to_string()))).level, Level::Warn);
        let stood_in = vault(&Plugin::Missing, Some("/tmp/fake-vault"), probe(true, true, Some(true)));
        assert_eq!(stood_in.level, Level::Ok, "KELPIE_VAULT_BIN stands in for a missing plugin, as detect() lets it: {stood_in:?}");
        assert!(stood_in.fact.contains("KELPIE_VAULT_BIN=/tmp/fake-vault") && stood_in.fact.contains("each task gets a note"), "{stood_in:?}");
        assert!(vault(&Plugin::Missing, Some("/tmp/fake-vault"), Some(Err("no such file".to_string()))).fact.contains("the vault command failed"));
    }

    #[test]
    fn render_prints_one_line_per_check_with_fixes_under_it() {
        let checks = [ok("git", "/usr/bin/git"), fail("jq", "not on PATH", both("brew install jq", "sudo apt install jq"))];
        let text = render(&checks);
        assert!(text.starts_with("ok    git  /usr/bin/git\nfail  jq   not on PATH\n             fix: macOS: brew install jq\n"), "{text}");
        assert!(text.ends_with("1 failed, 0 warnings, 1 ok"));
        assert!(failed(&checks) && !failed(&checks[..1]));
    }

    #[test]
    fn doctor_says_whether_notify_is_set_and_runs_nothing() {
        let marker = std::env::temp_dir().join(format!("kelpie-doctor-notify-{}", std::process::id()));
        let command = format!("touch {}", marker.display());
        let set = notify(&Setting { value: Some(command.clone()), layer: config::Layer::Repo });
        assert_eq!(set.level, Level::Ok);
        assert!(set.fact.contains(&format!("{command:?} (.kelpie.toml)")), "{}", set.fact);
        assert!(!marker.exists(), "doctor never sends a notification");
        let unset = notify(&Setting { value: None, layer: config::Layer::Default });
        assert_eq!(unset.level, Level::Ok);
        assert!(unset.fact.contains("not set"), "{}", unset.fact);
    }
}
