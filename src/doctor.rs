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
        tool("setsid", false, "the sidebar sync's background git fetch (scripts/sync.sh)", None, "none yet: macOS has no setsid, so sync.sh fetches nothing there", "sudo apt install util-linux"),
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

pub fn state_dirs(state_home: &Path, data_home: &Path) -> Vec<(PathBuf, &'static str)> {
    vec![
        (state_home.join("kelpie"), "backlog, messages, lead, review marks"),
        (state_home.join("kelpie/roles"), "task and lead roles"),
        (state_home.join("kelpie/turns"), "task reports"),
        (state_home.join("kelpie/reviews"), "review splits"),
        (state_home.join("herdr-sync"), "sidebar sync"),
        (state_home.join("herdr-sync-fetch"), "sidebar sync's fetch stamps"),
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

pub fn backlog(sources: &[Source], vault_index: impl Fn() -> Result<PathBuf, String>) -> Vec<Check> {
    if sources.is_empty() {
        return vec![ok("backlog", "no source configured; set [board.backlog] sources to fill the board's Backlog tab")];
    }
    sources.iter().map(|source| match source {
        Source::Vault => match vault_index() {
            Ok(index) => ok("backlog vault", format!("vault index {}", index.display())),
            Err(error) => warn("backlog vault", error, one("open a Claude session with the vault plugin once; the Areas page reads its index")),
        },
        Source::Linear => ok("backlog linear", "the lead reads Linear through a Linear MCP server in Claude (not checked here)"),
    }).collect()
}

pub struct Env {
    pub path: Vec<PathBuf>,
    pub home: PathBuf,
    pub state_home: PathBuf,
    pub data_home: Option<PathBuf>,
    pub claude_dir: PathBuf,
    pub config: PathBuf,
    pub repo_config: Option<PathBuf>,
    pub uid: u32,
}

impl Env {
    pub fn current() -> Self {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty()).map(PathBuf::from);
        Self {
            path: std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default(),
            state_home: var("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")),
            data_home: var("XDG_DATA_HOME"),
            claude_dir: var("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            config: config::path(),
            repo_config: std::env::current_dir().ok().and_then(|cwd| config::repo_file(&cwd)),
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
    checks.push(current_link(&env.home, env.data_home.as_deref(), plugin.as_ref().map(|plugin| plugin.root.as_path())));
    let data_home = env.data_home.clone().unwrap_or_else(|| env.home.join(".local/share"));
    for (dir, about) in state_dirs(&env.state_home, &data_home) {
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
    let (Some(machine), Some(own)) = (machine, own) else {
        return checks;
    };
    let settings = config::settings(&own, &machine);
    checks.push(setup_skill(&settings.setup_skill, installed.as_ref(), &claude_settings));
    checks.extend(backlog(&settings.sources.value, crate::vault::index_path));
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
        let setsid = tool(named(&tools, "setsid"), None, None);
        assert_eq!(setsid.level, Level::Warn);
        assert!(setsid.fact.contains("sync.sh"));
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
        let dirs = state_dirs(Path::new("/s"), Path::new("/d"));
        assert!(dirs.iter().any(|(dir, _)| dir == Path::new("/s/herdr-sync-fetch")) && dirs.iter().any(|(dir, _)| dir == Path::new("/d/kelpie")));
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
    fn backlog_sources_are_checked_per_source() {
        let none = backlog(&[], || unreachable!());
        assert_eq!((none.len(), none[0].level), (1, Level::Ok));
        let missing = backlog(&[Source::Vault, Source::Linear], || Err("No vault index at /x".to_string()));
        assert_eq!((missing[0].level, missing[0].fact.as_str()), (Level::Warn, "No vault index at /x"));
        assert!(missing[1].level == Level::Ok && missing[1].fact.contains("Linear MCP"));
        assert_eq!(backlog(&[Source::Vault], || Ok(PathBuf::from("/i.sqlite")))[0].level, Level::Ok);
    }

    #[test]
    fn render_prints_one_line_per_check_with_fixes_under_it() {
        let checks = [ok("git", "/usr/bin/git"), fail("jq", "not on PATH", both("brew install jq", "sudo apt install jq"))];
        let text = render(&checks);
        assert!(text.starts_with("ok    git  /usr/bin/git\nfail  jq   not on PATH\n             fix: macOS: brew install jq\n"), "{text}");
        assert!(text.ends_with("1 failed, 0 warnings, 1 ok"));
        assert!(failed(&checks) && !failed(&checks[..1]));
    }
}
