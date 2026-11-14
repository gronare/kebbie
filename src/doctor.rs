use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::login::Login;
use crate::config::{self, Machine, Setting, Source, Worktree};
use crate::sh::run;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Level {
    Ok,
    Skip,
    Warn,
    Fail,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub level: Level,
    pub name: String,
    pub fact: String,
    pub fix: Vec<String>,
    pub brief: String,
}

impl Check {
    pub fn new(level: Level, name: impl Into<String>, fact: impl Into<String>, fix: Vec<String>) -> Self {
        Check { level, name: name.into(), fact: fact.into(), fix, brief: String::new() }
    }

    pub fn brief(mut self, brief: impl Into<String>) -> Self {
        self.brief = brief.into();
        self
    }
}

fn ok(name: &str, fact: impl Into<String>) -> Check {
    Check::new(Level::Ok, name, fact, Vec::new())
}

fn warn(name: &str, fact: impl Into<String>, fix: Vec<String>) -> Check {
    Check::new(Level::Warn, name, fact, fix)
}

fn fail(name: &str, fact: impl Into<String>, fix: Vec<String>) -> Check {
    Check::new(Level::Fail, name, fact, fix)
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
        tool("nvim", false, "the review split's diffs", Some("0.10.0"), "brew install neovim", "brew install neovim, or a release from https://github.com/neovim/neovim/releases (distro packages are often older than 0.10)"),
        tool("tig", false, "the history split", None, "brew install tig", "sudo apt install tig"),
        tool("curl", false, "downloading a release's prebuilt binary", None, "brew install curl", "sudo apt install curl"),
        tool("cargo", false, "building kebbie on the main channel and with kebbie update --from", None, "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh", "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"),
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
    let Some(min) = &tool.min else { return ok(tool.name, found.display().to_string()).brief(tool.name) };
    let want = version(min).unwrap_or_default();
    match version_output.and_then(version) {
        Some(have) if at_least(&have, &want) => ok(tool.name, format!("{} at {}", shown(&have), found.display())).brief(if tool.name == "herdr" { shown(&have) } else { tool.name.to_string() }),
        Some(have) => missing(format!("{} is older than {min}, needed for {}", shown(&have), tool.feature), both(tool.mac, tool.linux)),
        None => missing(format!("cannot read the version from {} --version; need {min}", found.display()), both(tool.mac, tool.linux)),
    }
}

pub struct Plugin {
    pub enabled: bool,
    pub root: PathBuf,
}

pub fn herdr_plugin(list: Option<&str>, channel: config::Channel) -> (Check, Option<Plugin>) {
    let register = both("herdr plugin install gronare/kebbie (or herdr plugin link <a kebbie checkout>)", "herdr plugin install gronare/kebbie (or herdr plugin link <a kebbie checkout>)");
    let Some(list) = list else { return (fail("herdr plugin", "herdr plugin list --json failed", one("install herdr first")), None) };
    let value: Value = serde_json::from_str(list).unwrap_or_default();
    let found = value["result"]["plugins"].as_array().into_iter().flatten().find(|plugin| plugin["plugin_id"] == "kebbie");
    let Some(found) = found else { return (fail("herdr plugin", "kebbie is not registered with herdr", register), None) };
    let plugin = Plugin { enabled: found["enabled"] == true, root: PathBuf::from(found["plugin_root"].as_str().unwrap_or_default()) };
    let kebbie = crate::shown_version(channel, found["version"].as_str().unwrap_or("?"));
    let check = if plugin.enabled {
        ok("herdr plugin", format!("{kebbie} enabled, at {}", plugin.root.display())).brief("plugin enabled")
    } else {
        fail("herdr plugin", format!("{kebbie} is registered but disabled"), one("herdr plugin enable kebbie"))
    };
    (check, Some(plugin))
}

pub fn herdr_link(link: &crate::herdr_link::Link) -> Option<Check> {
    use crate::herdr_link::Link;
    match link {
        Link::Unregistered => None,
        Link::Installed => Some(ok("herdr link", "installed with herdr plugin install, not linked from a checkout").brief("installed in herdr")),
        Link::Linked(root) => Some(ok("herdr link", format!("linked from {}", root.display())).brief("linked in herdr")),
        Link::Gone { linked, target: Some(target) } => Some(fail("herdr link", format!("herdr links kebbie from {}, which is gone, so its startup starts neither the board nor the keeper", linked.display()), vec![
            format!("kebbie doctor --fix links it again from {} and starts both; a running keeper does it within the hour", target.display()),
            format!("or herdr plugin unlink kebbie && herdr plugin link {}", target.display()),
        ])),
        Link::Gone { linked, target: None } => Some(fail("herdr link", format!("herdr links kebbie from {}, which is gone, and no kebbie checkout is found to link instead", linked.display()), one("herdr plugin unlink kebbie && herdr plugin link <a kebbie checkout> (or herdr plugin install gronare/kebbie)"))),
    }
}

pub fn updates(state: &Path, channel: crate::config::Channel, installed: &[crate::update_check::Component]) -> Check {
    let (latest, fact) = crate::update_check::doctor_line(state, channel, installed);
    let running = crate::release::read_override(state);
    let check = ok("updates", fact).brief(running.as_ref().map_or_else(|| running_brief(channel), crate::release::Override::brief));
    let fix = if running.is_some() { "kebbie update --stable" } else { "kebbie update" };
    if latest { check } else { check.with(one(fix)) }
}

fn running_brief(channel: crate::config::Channel) -> String {
    let number = crate::shown_number(channel, env!("CARGO_PKG_VERSION"));
    match env!("KEBBIE_BUILT_FROM").split(' ').next().filter(|sha| !sha.is_empty()) {
        Some(sha) => format!("{number} @ {sha}"),
        None => number,
    }
}

pub fn keeper(enabled: bool, running: Option<&crate::keeper::Holder>, now: u64) -> Check {
    match (enabled, running) {
        (false, None) => ok("keeper", "off ([keeper] enabled = false): an open board runs the routines").brief("keeper off"),
        (false, Some(_)) => warn("keeper", "off in config.toml, but one still runs", one("kebbie keeper --stop")),
        (true, Some(running)) => {
            let pid = running.pid.map_or("?".to_string(), |pid| pid.to_string());
            let up = running.started.map_or(String::new(), |started| format!(", up {}", crate::keeper::span(now.saturating_sub(started))));
            ok("keeper", format!("running, pid {pid}{up}; kebbie keeper --status shows its routines")).brief("keeper running")
        }
        (true, None) => warn("keeper", "not running, so stale sessions, held messages and typed prompts wait for an open board", one("kebbie keeper --start (herdr's startup and the board start it too)")),
    }
}

// Each running board with when it started; one that started before the binary was written still runs the old code.
pub fn boards(running: &[(crate::lead::Board, Option<u64>)], built: Option<u64>) -> Option<Check> {
    let built = built.filter(|_| !running.is_empty())?;
    let old: Vec<String> = running.iter().filter(|(_, started)| started.is_some_and(|started| started <= built)).map(|(board, _)| format!("the board in {} (pane {}, pid {})", board.space, board.pane, board.pid)).collect();
    if old.is_empty() {
        return Some(ok("board", format!("{} on the installed kebbie", if running.len() == 1 { "running".to_string() } else { format!("{} running", running.len()) })));
    }
    Some(warn("board", format!("{} {} an older kebbie than the one installed", old.join(", "), if old.len() == 1 { "runs" } else { "run" }), one("kebbie lead --renew-board restarts it on the new one")))
}

pub fn sandbox(on: &Setting<bool>, missing: &[String]) -> Check {
    match (on.value, missing.is_empty()) {
        (false, _) => warn("sandbox", crate::sandbox::OFF, one(format!("set task.sandbox = true (now from the {})", on.layer.label()))),
        (true, true) => ok("sandbox", "on: a task's shell writes only in its worktree and the folders it is given, reaches only the allowed hosts and no Unix socket, and kebbie refuses its user-only commands to a sandboxed process").brief("sandbox on"),
        (true, false) => fail("sandbox", format!("on, but this machine cannot sandbox a task's shell fully, so kebbie starts no task: {} missing", missing.join(", ")), crate::sandbox::fixes(missing).into_iter().chain([format!("or set task.sandbox = false in {}", config::path().display())]).collect()),
    }
}

pub fn plugin_binary(root: &Path) -> Check {
    let binary = root.join("target/release/kebbie");
    match crate::update::binary_origin(root).filter(|_| binary.is_file()) {
        Some(origin @ crate::update::Origin::Downloaded { provenance: crate::update::Provenance::Unverified, .. }) => warn("plugin binary", format!("{}, {}", binary.display(), origin.describe()), one(crate::update::UNVERIFIED_FIX)),
        Some(origin) => ok("plugin binary", format!("{}, {}", binary.display(), origin.describe())).brief(origin.describe()),
        None => fail("plugin binary", format!("{} is missing", binary.display()), one(format!("sh {} in {} (herdr runs it on install and update)", crate::update::BUILD_SCRIPT, root.display()))),
    }
}

pub fn cargo(tool: &Tool, found: Option<&Path>, channel: crate::config::Channel, origin: Option<&crate::update::Origin>) -> Check {
    let downloaded = matches!(origin, Some(crate::update::Origin::Downloaded { .. }));
    match found {
        None if downloaded && channel == crate::config::Channel::Release => ok("cargo", "not on PATH, not needed: each release installs its prebuilt binary").brief("no cargo needed"),
        None => warn("cargo", "not on PATH; needed for the main channel and kebbie update --from", both(tool.mac, tool.linux)),
        Some(found) => self::tool(tool, Some(found), None),
    }
}

// No answer from the agent is never read as logged out.
pub fn login(name: &str, signed: Login, fix: &str, why: &str) -> Check {
    match signed {
        Login::In => ok(name, "logged in").brief("signed in"),
        Login::Out => warn(name, format!("not logged in, so {why}"), one(fix)),
        Login::Unknown => Check::new(Level::Skip, name, "login not checked: no answer to its status command", Vec::new()),
    }
}

pub fn checkout_version(plugin: Option<&Plugin>) -> Option<String> {
    plugin.and_then(|plugin| read_json(&plugin.root.join(".claude-plugin/plugin.json"))).and_then(|manifest| manifest["version"].as_str().map(str::to_string))
}

pub fn claude_plugin(installed: Option<&Value>, settings: &Value, checkout: Option<&str>, channel: config::Channel) -> Check {
    let install = one("claude plugin marketplace add ~/.local/share/kebbie/current && claude plugin install kebbie@kebbie");
    let Some(installed) = installed else { return warn("claude plugin", "no installed_plugins.json; the review skills and hook are missing", install) };
    let Some((id, installs)) = installed["plugins"].as_object().into_iter().flatten().find(|(id, _)| id.starts_with("kebbie@")) else {
        return warn("claude plugin", "kebbie is not installed in Claude Code; the review skills and hook are missing", install);
    };
    let version = installs[0]["version"].as_str().unwrap_or("?");
    let shown = crate::shown_number(channel, version);
    if settings["enabledPlugins"][id] == false {
        return warn("claude plugin", format!("{id} {shown} is installed but disabled"), one(format!("claude plugin enable {id}")));
    }
    if let Some(checkout) = checkout.filter(|checkout| *checkout != version) {
        let behind = match channel {
            config::Channel::Main => format!("{id} runs in Claude Code from an older copy than kebbie's checkout; Claude Code runs a cached copy"),
            config::Channel::Release => format!("{id} {version} runs in Claude Code, but kebbie's checkout is {checkout}; Claude Code runs a cached copy"),
        };
        return warn("claude plugin", behind, one("kebbie update"));
    }
    ok("claude plugin", format!("{id} {shown}")).brief("plugin current")
}

pub fn plugin_source(source: &crate::update::Marketplace, channel: crate::config::Channel) -> Check {
    use crate::update::Marketplace;
    match (source, channel) {
        (Marketplace::InPlace(path), _) => ok("claude plugin source", format!("loaded in place from {}", path.display())).brief("plugin in place"),
        (Marketplace::Git, crate::config::Channel::Main) => ok("claude plugin source", "GitHub, following the default branch like this machine's main channel").brief("plugin from GitHub"),
        (Marketplace::Git, crate::config::Channel::Release) => warn("claude plugin source", "GitHub, following the default branch, while this machine follows release tags; kebbie update leaves it as it is", one(crate::update::IN_PLACE_FIX)),
    }
}

// Kebbie and the lead's role read the link under $HOME; scripts/herdr-startup.sh writes it under $XDG_DATA_HOME.
pub fn current_link(home: &Path, data_home: Option<&Path>, plugin_root: Option<&Path>) -> Check {
    let link = home.join(".local/share/kebbie/current");
    let start = one("start herdr once: the kebbie plugin's startup (scripts/herdr-startup.sh) links it");
    if let Some(data) = data_home.filter(|data| *data != home.join(".local/share")) {
        return warn("current link", format!("XDG_DATA_HOME is {}, so herdr links {}/kebbie/current while kebbie reads {}", data.display(), data.display(), link.display()), one("unset XDG_DATA_HOME for herdr, or link it yourself: ln -sfn <plugin root> ~/.local/share/kebbie/current"));
    }
    if std::fs::symlink_metadata(&link).is_err() {
        return fail("current link", format!("{} is missing", link.display()), start);
    }
    let Ok(target) = link.canonicalize() else { return fail("current link", format!("{} points nowhere", link.display()), start) };
    match plugin_root.map(|root| root.canonicalize().unwrap_or_else(|_| root.to_path_buf())) {
        Some(root) if root != target => warn("current link", format!("{} points at {}, herdr's kebbie is at {}", link.display(), target.display(), root.display()), one("restart herdr so its startup relinks it")),
        _ => ok("current link", format!("{} → {}", link.display(), target.display())).brief("current link"),
    }
}

const LINK_KEBBIE: &str = "ln -s ~/.local/share/kebbie/current/target/release/kebbie ~/.local/bin/kebbie";
pub const ON_PATH: &str = "kebbie on PATH";
pub const NOT_ON_PATH: &str = "not on PATH; the lead, tasks and hooks run kebbie by name";

pub fn kebbie_on_path(home: &Path, path: &[PathBuf]) -> Check {
    let current = home.join(".local/share/kebbie/current/target/release/kebbie");
    let Some(found) = which("kebbie", path) else {
        return warn(ON_PATH, NOT_ON_PATH, vec![LINK_KEBBIE.to_string(), "make sure ~/.local/bin is on PATH".to_string()]);
    };
    let resolved = found.canonicalize().unwrap_or_else(|_| found.clone());
    match current.canonicalize() {
        Ok(binary) if binary == resolved => ok(ON_PATH, format!("{} → {}", found.display(), binary.display())).brief("on PATH"),
        _ => warn(ON_PATH, format!("{} resolves to {}, not {}", found.display(), resolved.display(), current.display()), one(format!("remove {} or put ~/.local/bin ahead of it on PATH, then {LINK_KEBBIE}", found.display()))),
    }
}

pub fn state_dirs(kebbie: &Path, own: bool, state_home: &Path, data_home: &Path) -> Vec<(PathBuf, &'static str)> {
    let about = if own { "backlog, messages, lead, review marks; set by KEBBIE_STATE_DIR" } else { "backlog, messages, lead, review marks" };
    vec![
        (kebbie.to_path_buf(), about),
        (kebbie.join("roles"), "task and lead roles"),
        (kebbie.join("turns"), "task reports"),
        (kebbie.join("reviews"), "review splits"),
        (state_home.join("herdr-sync"), "sidebar sync"),
        (data_home.join("kebbie"), "the current link"),
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
        return ok(name, fact).brief("state dir");
    }
    if mine {
        return fail(name, format!("{} ({about}): {} is not writable", path.display(), existing.display()), one(format!("chmod u+w {}", existing.display())));
    }
    let owner = if meta.uid() == 0 { "root".to_string() } else { format!("uid {}", meta.uid()) };
    fail(name, format!("{} ({about}): {} is owned by {owner}", path.display(), existing.display()), one(format!("sudo chown -R \"$(id -un)\" {}", existing.display())))
}

pub fn config_file(path: &Path) -> (Check, Option<Machine>) {
    match config::check_machine(path) {
        Ok((config, unknown)) if !unknown.is_empty() => (warn("config", unknown.join("; "), one(config::UNKNOWN_FIX)), Some(config)),
        Ok((config, _)) if path.exists() => (ok("config", format!("{} parses", path.display())).brief("config"), Some(config)),
        Ok((config, _)) => (ok("config", format!("{} not found; every machine setting is its default", path.display())).brief("default config"), Some(config)),
        Err(error) => (fail("config", format!("{error}; until it parses, the hub takes no commands and approves devices only at this machine"), one("fix that key; kebbie config shows the result")), None),
    }
}

pub fn repo_file(path: &Path) -> (Check, Option<Worktree>) {
    let (name, shown) = ("repo config", config::shown(path));
    let fix = || one("fix that key; kebbie config --repo <the repo> shows the result");
    if !path.exists() && std::fs::symlink_metadata(path).is_ok() {
        return (fail(name, format!("{shown}: the link points at nothing"), one("clone the repo it links into, or remove the link")), None);
    }
    match config::check_repo(path) {
        Ok((worktree, unknown)) if path.exists() => match crate::actions::load_from(path).1 {
            errors if !errors.is_empty() => (fail(name, errors.join("; "), fix()), Some(worktree)),
            _ if !unknown.is_empty() => (warn(name, unknown.join("; "), one(config::UNKNOWN_FIX)), Some(worktree)),
            _ => (ok(name, format!("{shown} parses")).brief("repo config"), Some(worktree)),
        },
        Ok((worktree, _)) => (ok(name, format!("{shown}; config.toml and the defaults apply")).brief("repo config"), Some(worktree)),
        Err(error) => (fail(name, format!("{shown}: {error}"), fix()), None),
    }
}

pub fn repo_files(dir: &Path, current: Option<&Path>) -> (Vec<Check>, Option<Worktree>) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "toml")).collect();
    files.sort();
    if let Some(current) = current.filter(|current| !files.iter().any(|file| file == current)) {
        files.push(current.to_path_buf());
    }
    let mut own = Some(Worktree::default());
    let checks = files.iter().map(|file| {
        let (check, worktree) = repo_file(file);
        if Some(file.as_path()) == current {
            own = worktree;
        }
        check
    }).collect();
    (checks, own)
}

pub fn task_profiles(profiles_dir: &Path, named: &[(String, String)]) -> Option<Check> {
    if named.is_empty() {
        return None;
    }
    let names: Vec<String> = crate::profiles::list_in(profiles_dir).0.into_iter().filter(|profile| !profile.review).map(|profile| profile.name).collect();
    let missing: Vec<String> = named.iter().filter(|(_, profile)| !names.contains(profile)).map(|(file, profile)| format!("task.profile = \"{profile}\" in {file} names no profile")).collect();
    if missing.is_empty() {
        let set: Vec<String> = named.iter().map(|(file, profile)| format!("\"{profile}\" in {file}")).collect();
        return Some(ok("task profile", format!("task.profile names a profile there is: {}", set.join(", "))).brief("task profile"));
    }
    Some(warn("task profile", format!("{}; the profiles are {}", missing.join("; "), names.join(", ")), one("pick it on the Settings page (,) under Tasks")))
}

pub fn unchosen_profiles(profiles: &[crate::profiles::Profile], chosen: &[crate::agent::Agent]) -> Vec<Check> {
    profiles.iter().filter(|profile| !chosen.contains(&profile.agent)).map(|profile| {
        warn("profile", format!("profile {}: {}", profile.name, profile.agent.not_set_up()), one(format!("kebbie setup {}", profile.agent.key())))
    }).collect()
}

const CLAUDE_ALIASES: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

// A Claude profile that pins a full model id stays on it after Claude Code moves on; an alias follows each new model.
pub fn claude_models(profiles: &[crate::profiles::Profile]) -> Vec<Check> {
    profiles.iter().filter(|profile| profile.agent == crate::agent::Agent::Claude && profile.file.is_some()).filter_map(|profile| {
        let model = profile.model.as_deref()?;
        let base = model.split('[').next().unwrap_or(model);
        (!CLAUDE_ALIASES.contains(&base)).then(|| warn("claude model", format!("profile {} pins {model}, a full model id, so its tasks stay on that model; an alias ({}) follows Claude Code to each new one", profile.name, CLAUDE_ALIASES.join(", ")), one("pick one with m on the Settings page")))
    }).collect()
}

// A profile's kubeconfig is the whole of what its tasks may do in a cluster, so it must be there, the user's alone, and not the user's own login.
pub fn kubeconfigs(profiles: &[crate::profiles::Profile], home: &Path) -> Vec<Check> {
    use std::os::unix::fs::PermissionsExt;
    let default = home.join(".kube/config");
    let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    profiles.iter().filter_map(|profile| Some((profile, profile.sandbox.kubeconfig_file()?))).map(|(profile, file)| {
        let shown = crate::area_page::home_relative(&file);
        let fact = |what: String| format!("profile {}: sandbox.kubeconfig {shown} {what}", profile.name);
        if real(&file) == real(&default) {
            return warn("kubeconfig", fact("is your default kubeconfig, usually an admin login, so its tasks may do anything you may in the cluster".to_string()), one("export a kubeconfig for a ServiceAccount with only the RBAC the tasks need to its own file, and point sandbox.kubeconfig at it"));
        }
        match std::fs::metadata(&file) {
            Err(_) => warn("kubeconfig", fact("is not there, so its tasks' kubectl and helm fail".to_string()), one(format!("export the ServiceAccount's kubeconfig to {shown} with mode 0600"))),
            Ok(metadata) if metadata.permissions().mode() & 0o077 != 0 => warn("kubeconfig", fact(format!("is readable by others (mode {:04o})", metadata.permissions().mode() & 0o777)), one(format!("chmod 600 {shown}"))),
            Ok(_) => ok("kubeconfig", fact("is there and yours alone (mode 0600)".to_string())).brief("kubeconfig"),
        }
    }).collect()
}

pub(crate) fn named_profiles(machine: Option<&Machine>, config: &Path, repos_dir: &Path) -> Vec<(String, String)> {
    let mut named: Vec<(String, String)> = machine.and_then(|machine| machine.worktree.profile.clone()).map(|profile| (crate::area_page::home_relative(config), profile)).into_iter().collect();
    let mut files: Vec<PathBuf> = std::fs::read_dir(repos_dir).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|extension| extension == "toml")).collect();
    files.sort();
    named.extend(files.iter().filter_map(|file| config::check_repo(file).ok()?.0.profile.map(|profile| (crate::area_page::home_relative(file), profile))));
    named.retain(|(_, profile)| profile != config::NONE);
    named
}

pub fn setup_skill(skill: &Setting<String>, installed: Option<&Value>, settings: &Value) -> Check {
    let name = "setup skill";
    let shown = format!("{} ({})", skill.value, skill.layer.label());
    if let Some(path) = config::skill_path(&skill.value) {
        return if path.is_file() { ok(name, format!("{shown}: a file a task reads")).brief("setup skill") } else { warn(name, format!("{shown}: no file at {}, so a task sets its worktree up without it", path.display()), one("create the file, or set worktree.setup_skill to a skill or a file this machine has")) };
    }
    let Some((plugin, _)) = skill.value.split_once(':') else {
        return ok(name, format!("{shown}: a user or project skill, not checked")).brief("setup skill");
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
    ok(name, format!("{shown}: {id}")).brief("setup skill")
}

// Looks for the command's program in the main checkout and never runs it.
pub fn setup_command(command: &Setting<Option<String>>, repo: &Path) -> Option<Check> {
    let value = command.value.as_deref()?;
    let name = "setup command";
    let shown = format!("{value:?} ({})", command.layer.label());
    let program = value.split_whitespace().next().unwrap_or(value);
    if !program.contains('/') {
        return Some(ok(name, format!("{shown}: {program} is found on PATH when it runs, not checked")).brief("setup command"));
    }
    let path = repo.join(program);
    let fix = format!("add {program} to the main checkout, or set worktree.setup_command to the repo's prepare command");
    Some(match std::fs::metadata(&path) {
        Err(_) if command.layer == config::Layer::Machine => ok(name, format!("{shown}: this repo has no {program}, so kebbie's default setup runs here")).brief("default setup"),
        Err(_) => warn(name, format!("{shown}: {} not found, so the setup skill cannot run it", path.display()), one(fix)),
        Ok(meta) if !meta.is_file() || meta.mode() & 0o111 == 0 => warn(name, format!("{shown}: {} is not an executable file", path.display()), vec![format!("chmod +x {program}"), fix]),
        Ok(_) => ok(name, format!("{shown}: {}", path.display())).brief("setup command"),
    })
}

// Reports whether the key is set and never runs the command, so doctor sends nothing.
pub fn notify(command: &Setting<Option<String>>) -> Check {
    match &command.value {
        Some(value) => ok("notify", format!("notify.command {value:?} ({}): run when a task needs you or is ready, a question waits, or the lead notifies; failures go to notify.log in the state dir", command.layer.label())).brief("notifications on"),
        None => ok("notify", "notify.command not set, so kebbie sends no notifications; set [notify] command to a shell command to get them").brief("no notifications"),
    }
}

const STATUS_LINE: &str = r#""statusLine": { "type": "command", "command": "~/.local/share/kebbie/current/target/release/kebbie statusline" }"#;
pub const STATUS_LINE_COMMAND: &str = "~/.local/share/kebbie/current/target/release/kebbie statusline";

// Claude Code passes the plan's usage only to the statusLine command, so the board's usage bar needs kebbie there.
pub fn status_line(settings: &Value) -> Check {
    let command = settings["statusLine"]["command"].as_str().unwrap_or_default();
    if command.contains("kebbie") && command.contains("statusline") {
        return ok("status line", format!("{command:?} keeps the plan's usage for the board")).brief("status line");
    }
    let fact = if command.is_empty() { "no statusLine command in Claude Code's settings, so the board shows no plan usage".to_string() } else { format!("statusLine runs {command:?}, so the board shows no plan usage") };
    let fix = if command.is_empty() { format!("add to ~/.claude/settings.json: {STATUS_LINE}") } else { "have your statusLine command pipe its stdin to `kebbie statusline` as well (it prints nothing, so your line stays as it is), or use kebbie's, which shows no line: ".to_string() + STATUS_LINE };
    warn("status line", fact, one(fix))
}

pub fn push_block(repo: &Path) -> Check {
    match crate::push_block::unsupported(repo) {
        Ok(None) => ok("push block", format!("task worktrees of {} block git push", repo.display())).brief("push block"),
        Ok(Some(why)) => warn("push block", why, one("move core.worktree, or core.bare = true, from the shared config into the main worktree's config.worktree, then restart the repo's tasks")),
        Err(error) => warn("push block", error, Vec::new()),
    }
}

// Task sessions whose hooks run a kebbie binary that is gone, so their test runs and edits go unrecorded.
pub fn task_hooks(sessions: &[String]) -> Check {
    if sessions.is_empty() {
        return ok("task hooks", "every open task's hooks run a kebbie that is there").brief("task hooks");
    }
    warn("task hooks", format!("the hooks of {} run a kebbie that is gone, so their test runs and edits go unrecorded", sessions.join(", ")), one("kebbie restart (a busy session restarts once it is idle; the restart writes its settings again)"))
}

// The open tasks and the lead whose settings file names hooks that no longer run.
pub fn sessions_with_gone_hooks(roles: &Path, addresses: &[String]) -> Vec<String> {
    addresses.iter().filter(|address| read_json(&roles.join(format!("{address}.settings.json"))).is_some_and(|settings| !crate::restart::gone_hooks(&settings).is_empty())).cloned().collect()
}

fn open_addresses(repos: &[PathBuf]) -> Vec<String> {
    let mut addresses = vec![crate::status::LEAD_LABEL.to_string()];
    for repo in repos {
        for slot in crate::tasks::slots(repo).unwrap_or_default() {
            addresses.extend(slot.task.map(|task| crate::peers::address(&task, &crate::repo::name_of(repo))));
        }
    }
    addresses
}

fn known_repos() -> Vec<PathBuf> {
    crate::status::known_repos(&crate::herdr::workspaces().unwrap_or_default(), crate::repo::pooled_checkouts()).into_iter().collect()
}

// kebbie doctor --fix: what the doctor can mend by itself before it checks.
pub fn fix(state: &Path) -> Vec<String> {
    let mut lines = crate::herdr_link::heal().unwrap_or_else(|error| vec![error]);
    lines.extend(crate::push_block::renew_old_blocks(&known_repos(), state));
    let mut asked = |question: &str| crate::setup::yes(question, &mut std::io::stdin().lock(), &mut std::io::stdout()).unwrap_or(false);
    let ask: Option<&mut dyn FnMut(&str) -> bool> = if crate::setup::in_terminal() { Some(&mut asked) } else { None };
    lines.extend(vault_fix(vault_suggestion(), &config::path(), ask));
    if lines.is_empty() { vec!["Nothing to fix: herdr's kebbie folder is there and no worktree has an earlier push block.".to_string()] } else { lines }
}

pub const GH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

// No gh, no GitHub remote, no login or a slow network is no answer, never a failure.
pub fn visibility(gh: Option<&Path>, repo: &Path, timeout: std::time::Duration) -> Option<String> {
    let answer = crate::notify::output_with_timeout(&gh?.to_string_lossy(), &["repo", "view", "--json", "visibility", "--jq", ".visibility"], repo, timeout).ok()?;
    Some(answer.trim().to_ascii_lowercase()).filter(|answer| !answer.is_empty())
}

// KEBBIE_GH_BIN as CI watching reads it, else gh on the doctor's PATH.
fn gh_on(path: &[PathBuf]) -> Option<PathBuf> {
    let gh = crate::ci::gh();
    if gh.contains('/') { Some(PathBuf::from(gh)) } else { which(&gh, path) }
}

pub fn docs(required: &Setting<bool>, visibility: impl FnOnce() -> Option<String>, repo_config: &Path) -> Option<Check> {
    if required.value {
        return Some(ok("docs", format!("docs.required is on ({}): the reviewer fails a branch that changes what a user sees or does and documents nothing", required.layer.label())).brief("docs required"));
    }
    (visibility().as_deref() == Some("public")).then(|| {
        ok("docs", "this repo is public on GitHub and docs.required is off, so nothing holds a change that leaves its README and docs/ behind").brief("docs not required")
            .with(one(format!("to turn it on, add [docs] required = true to {}", repo_config.display())))
    })
}

// A whole-suite run this short is cheap enough to ask of every push.
const SHORT_FULL_MS: u64 = 5 * 60 * 1000;

// The whole-suite command the repo's tasks ran most, with its median time, when kebbie recorded its duration green at least once.
pub fn short_full_run(runs: &[crate::evidence::Run], listed: &[String]) -> Option<(String, u64)> {
    let mut timed: Vec<(String, Vec<u64>)> = Vec::new();
    for run in runs.iter().filter(|run| run.result == crate::evidence::Outcome::Passed) {
        let (Some(command), Some(ms)) = (crate::evidence::whole_command(&run.command, listed), run.duration_ms) else { continue };
        match timed.iter_mut().find(|(known, _)| *known == command) {
            Some((_, times)) => times.push(ms),
            None => timed.push((command, vec![ms])),
        }
    }
    let (command, mut times) = timed.into_iter().max_by_key(|(_, times)| times.len())?;
    times.sort_unstable();
    let median = times[times.len() / 2];
    (median <= SHORT_FULL_MS).then_some((command, median))
}

pub fn tests(policy: &Setting<config::BeforePush>, full: Option<&str>, short: impl FnOnce() -> Option<(String, u64)>, repo_config: &Path) -> Option<Check> {
    if policy.value == config::BeforePush::Full {
        return Some(match full {
            Some(full) => ok("tests", format!("tests.before_push is full ({}): a ready report and every push wait for a green `{full}` on the task's HEAD", policy.layer.label())).brief("full suite before push"),
            None => warn("tests", "tests.before_push is full, but this repo names no tests.full_command, so its tasks run only related tests before a push", one(format!("add [tests] full_command = \"<the command CI runs>\" to {}", repo_config.display()))),
        });
    }
    let (command, ms) = short()?;
    Some(ok("tests", format!("this repo's whole suite `{command}` passed in {}s on its tasks' recorded runs, and a push asks only for the tests related to the change", ms.div_ceil(1000))).brief("related tests before push")
        .with(one(format!("to require it before every push, add [tests] before_push = \"full\" and full_command = \"{command}\" to {}", repo_config.display()))))
}

// Every task's recorded runs in this repo, wherever its tasks stand now.
fn repo_runs(state: &Path, repo: &Path) -> Vec<crate::evidence::Run> {
    let tail = format!("@{}.jsonl", crate::repo::name_of(repo));
    let names = std::fs::read_dir(state.join("evidence")).into_iter().flatten().flatten().filter_map(|entry| entry.file_name().to_str().map(str::to_string));
    names.filter_map(|name| name.strip_suffix(".jsonl").filter(|_| name.ends_with(&tail)).map(str::to_string)).flat_map(|address| crate::evidence::runs_in(state, &address)).collect()
}

// The same requirement and the same reasons as CI watching.
pub fn comments(watch: &Setting<bool>, days: u64, target: impl FnOnce() -> std::result::Result<String, String>) -> Check {
    if !watch.value {
        return ok("comments", format!("comments.watch is off ({}): comments on pushed commits are not read", watch.layer.label())).brief("comments not read");
    }
    match target() {
        Ok(slug) => ok("comments", format!("on: kebbie reads colleagues' comments on the commits it pushes to {slug}, for {days} days after each push")).brief("comments read"),
        Err(reason) => warn("comments", format!("on, but comments here are not read: {reason}"), one("gh auth login, with origin on GitHub")),
    }
}

pub fn backlog(sources: &[Source], vault_source: impl Fn() -> Result<String, String>, local_dir: &Path) -> Vec<Check> {
    if sources.is_empty() {
        return vec![ok("backlog", "no source configured; set [board.backlog] sources to fill the board's Backlog tab").brief("no backlog source")];
    }
    sources.iter().map(|source| match source {
        Source::Local if local_dir.exists() => {
            let items = crate::backlog_file::items(local_dir);
            let to_write = items.iter().filter(|item| item.state() == crate::backlog_file::State::ToWrite).count();
            let waiting = if to_write == 0 { String::new() } else { format!(", {to_write} for the lead to write out") };
            ok("backlog local", format!("{}: {} items{waiting}", local_dir.display(), items.len())).brief(format!("{} backlog items", items.len()))
        }
        Source::Local => ok("backlog local", format!("{} is created by the first item (the board's n, or kebbie backlog add)", local_dir.display())).brief("local backlog"),
        Source::Vault => match vault_source() {
            Ok(source) => ok("backlog vault", source).brief("backlog"),
            Err(error) => warn("backlog vault", error, one("open a Claude session with the vault plugin once; the Areas page reads its index")),
        },
        Source::Linear => ok("backlog linear", "the lead reads Linear through a Linear MCP server in Claude (not checked here)").brief("Linear backlog"),
    }).collect()
}

// A template file [board.backlog] template names that does not read leaves local items on kebbie's own.
pub fn item_template(named: Option<&Path>) -> Option<Check> {
    let file = named?;
    Some(match crate::item_template::read_file(file) {
        Ok(_) => ok("backlog template", format!("local items follow {}", file.display())).brief("backlog template"),
        Err(error) => warn("backlog template", format!("{error}; local items follow kebbie's own template until it reads"), one("fix the file, or remove template under [board.backlog]; kebbie backlog template --local shows the sections in use")),
    })
}

// An older config that lists both reads as the vault alone; the local folder stays on disk, unread.
pub fn mixed_sources(listed: &[Source], local_dir: &Path, config: &Path) -> Option<Check> {
    crate::config::mixes_local_and_vault(listed).then(|| {
        let kept: Vec<String> = crate::config::read_sources(listed).iter().map(|source| format!("{:?}", source.key())).collect();
        warn("backlog sources", format!("board.backlog.sources lists \"local\" and \"vault\": new items go to the vault, and {} is no longer read (kebbie leaves it as it is)", local_dir.display()),
            one(format!("set sources = [{}] under [board.backlog] in {}, after moving what you still want from that folder into the vault", kept.join(", "), config.display())))
    })
}

fn plugin_ids<'a>(installed: Option<&'a Value>, prefix: &'a str) -> impl Iterator<Item = (&'a String, &'a Value)> {
    installed.and_then(|installed| installed["plugins"].as_object()).into_iter().flatten().filter(move |(id, _)| id.starts_with(prefix))
}

// The vault plugin: one whose .mcp.json starts a server named vault.
pub fn vault_plugin(installed: Option<&Value>, read: impl Fn(&Path) -> Option<String>) -> Option<String> {
    plugin_ids(installed, "vault@").find(|(_, entries)| {
        entries.as_array().into_iter().flatten().filter_map(|entry| entry["installPath"].as_str()).any(|root| {
            read(&Path::new(root).join(".mcp.json")).and_then(|text| serde_json::from_str::<Value>(&text).ok()).is_some_and(|mcp| mcp["mcpServers"]["vault"].is_object())
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
        let names: Vec<String> = sources.iter().filter(|listed| source != Source::Vault || **listed != Source::Local).chain([&source]).map(|source| format!("{:?}", source.key())).collect();
        one(format!("to use it, set sources = [{}] under [board.backlog] in {}", names.join(", "), config.display()))
    };
    let mut checks = Vec::new();
    if let Some(id) = vault.filter(|_| !sources.contains(&Source::Vault)) {
        checks.push(ok("backlog vault", format!("the vault plugin {id} is installed in Claude Code, but the vault is not a backlog source")).brief("not the backlog").with(suggest(Source::Vault)));
    }
    if let Some(found) = linear.filter(|_| !sources.contains(&Source::Linear)) {
        checks.push(ok("backlog linear", format!("Claude Code has {found}, but Linear is not a backlog source")).brief("Linear not the backlog").with(suggest(Source::Linear)));
    }
    checks
}

// The [vault] command line doctor offers: the package's new name, or the command the plugin bridge runs, so kebbie reaches the vault without the Claude plugin.
pub fn suggested_command(command: &[String], origin: crate::vault_cli::Origin, probe: Option<&crate::vault_cli::Probe>) -> Option<String> {
    use crate::vault_cli::Origin;
    if origin == Origin::Env {
        return None;
    }
    match probe.and_then(|probe| crate::vault_cli::renamed(command, probe)) {
        Some(renamed) => Some(renamed.join(" ")),
        None => (origin == Origin::PluginHooks).then(|| command.join(" ")),
    }
}

fn identity(probe: &crate::vault_cli::Probe) -> String {
    let name = if probe.name.is_empty() { "the vault" } else { probe.name.as_str() };
    let read = if probe.contract.is_none() { ", read from its status and help as it answers no about --json yet" } else { "" };
    format!("{name} {}{read}", probe.version)
}

fn newer_vault(origin: crate::vault_cli::Origin, probe: &crate::vault_cli::Probe) -> Vec<String> {
    match origin {
        crate::vault_cli::Origin::PluginHooks => vec![crate::vault_cli::NEEDS_UPDATE.to_string(), "or set [vault] command in the machine config to a newer vault".to_string()],
        _ => one(format!("point [vault] command at a newer {}", if probe.name.is_empty() { "vault" } else { probe.name.as_str() })),
    }
}

// Optional: without a vault command kebbie keeps its own backlog and state, so none is no gap; KEBBIE_VAULT_BIN stands in for the command as it does for detect().
pub fn vault(plugin: &crate::vault_cli::Plugin, command: Option<&(Vec<String>, crate::vault_cli::Origin)>, probe: Option<Result<crate::vault_cli::Probe, String>>) -> Check {
    use crate::vault_cli::{Origin, Plugin};
    let name = "vault";
    let Some((command, origin)) = command else {
        return match plugin {
            Plugin::Enabled { id, version, .. } => warn(name, format!("{id} {version} names no vault command kebbie can run"), vec!["kebbie setup finds the vault commands on this machine and asks which to use".to_string(), "or set [vault] command in the machine config".to_string()]),
            _ => ok(name, "no [vault] command (optional): with one, kebbie keeps each task's note and the repos' logs in your notes vault, whatever agent runs the task").brief("no vault (optional)").with(one("kebbie setup finds the vault commands on this machine and asks which to use")),
        };
    };
    let shown = command.join(" ");
    let who = match (origin, plugin) {
        (Origin::Env, _) => format!("KEBBIE_VAULT_BIN={shown}"),
        (Origin::Config, _) => format!("[vault] command \"{shown}\""),
        (Origin::PluginHooks, Plugin::Enabled { id, version, .. }) => format!("{id} {version}'s \"{shown}\""),
        (Origin::PluginHooks, _) => format!("\"{shown}\""),
    };
    let probe = match probe {
        None => return warn(name, format!("{who} was not run"), Vec::new()),
        Some(Err(error)) => return warn(name, format!("{who} failed ({error}), so kebbie keeps tasks without the vault"), vec![format!("check that it runs: {shown} about --json"), "or kebbie setup to pick another vault command".to_string()]),
        Some(Ok(probe)) => probe,
    };
    let suggested = suggested_command(command, *origin, Some(&probe));
    let label = identity(&probe);
    match () {
        _ if !probe.speaks() => warn(name, format!("{who} answers contract {}, and this kebbie reads contract {}, so it keeps tasks without the vault", probe.contract.unwrap_or_default(), crate::vault_cli::CONTRACT), one("kebbie update")),
        _ if !probe.configured => warn(name, format!("{who} ({label}) answers, but no vault is configured"), vec![format!("{shown} status says what it is missing"), format!("{shown} setup chooses where the vault lives: a folder, an S3 bucket or a server")]),
        _ if !probe.can(crate::vault_cli::TASKS) => warn(name, format!("{who} ({label}) states no tasks capability: the vault is read-only to kebbie, so tasks get no notes and pushes no log lines"), newer_vault(*origin, &probe)),
        _ if probe.writable == Some(false) => warn(name, format!("{who}: the {} vault is not writable, so kebbie keeps tasks without it", probe.backend), one("check the vault folder's permissions or the S3 key's write access")),
        _ if suggested.is_some() && *origin == Origin::Config => warn(name, format!("{who} runs {label}, the package's new name"), vec![format!("set [vault] command = \"{}\" in the machine config", suggested.unwrap_or_default()), "kebbie doctor --fix writes it after your yes".to_string()]),
        _ if !probe.claims() => warn(name, format!("{who} ({label}) is too old for cross-machine claims: tasks get notes and logs, but kebbie on another machine sharing this vault cannot see which machine works on a note"), newer_vault(*origin, &probe)),
        _ => {
            let found = ok(name, format!("{who}: {label}, {}, states {}: each task gets a note claimed for this machine, edits wait for its Problem, pushes are logged, done closes it", probe.backend, probe.capabilities.join(", "))).brief(format!("{} {} · {} · task notes", if probe.name.is_empty() { "vault" } else { probe.name.as_str() }, probe.version, probe.backend));
            match suggested {
                Some(line) => found.with(vec![format!("set [vault] command = \"{line}\" in the machine config, so kebbie reaches the vault without the Claude plugin"), "kebbie doctor --fix writes it after your yes".to_string()]),
                None => found,
            }
        }
    }
}

fn vault_suggestion() -> Option<String> {
    let plugin = crate::vault_cli::plugin(&crate::agent::claude::home().unwrap_or_default());
    let env_bin = std::env::var(crate::vault_cli::BIN_VAR).ok().filter(|bin| !bin.is_empty());
    let (command, origin) = crate::vault_cli::command(env_bin, crate::vault_cli::configured(), &plugin)?;
    let probe = crate::vault_cli::Cli::new(command.clone()).probe(false).ok();
    suggested_command(&command, origin, probe.as_ref())
}

// kebbie doctor --fix: the suggested [vault] command, written to kebbie's own config only on a yes; without a terminal it is only shown.
pub fn vault_fix(suggested: Option<String>, config: &Path, ask: Option<&mut dyn FnMut(&str) -> bool>) -> Option<String> {
    let line = suggested?;
    let question = format!("Set [vault] command = \"{line}\" in {}?", config.display());
    let Some(ask) = ask else { return Some(format!("{question} Run kebbie doctor --fix in a terminal, or set it yourself.")) };
    if !ask(&question) {
        return Some("[vault] command stays as it is.".to_string());
    }
    Some(match crate::config_edit::write(config, true, false, |text| crate::config_edit::set(text, "vault.command", &crate::config_edit::Set::Text(line.clone()))) {
        Ok(()) => format!("[vault] command = \"{line}\" in {}", config.display()),
        Err(error) => format!("[vault] command not written: {error}"),
    })
}

// The phone and the other machines on the vault show this name, so a name nobody chose is worth one warning.
pub fn machine_name(name: &config::Setting<String>, config_file: &Path) -> Check {
    match name.layer {
        config::Layer::Default => warn("machine", format!("machine.name is unset, so your phone and your other machines on the vault see this one as \"{}\", the host's short name", name.value), vec![format!("set [machine] name = \"<a name of your own, such as mac or devbox>\" in {}", config_file.display()), "or run /kebbie:configure, which asks for it first".to_string()]),
        _ => ok("machine", format!("goes by \"{}\" on your phone and in vault claims", name.value)).brief(format!("claims as {}", name.value)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Repair {
    StatusLine,
    KeeperStart,
    KeeperStop,
    HerdrPlugin,
}

pub struct Found<'a> {
    pub claude_settings: &'a Value,
    pub keeper_on: bool,
    pub keeper_running: bool,
    pub herdr_list: Option<&'a str>,
}

pub fn repairs(found: &Found) -> Vec<Repair> {
    let mut repairs = Vec::new();
    if found.claude_settings.get("statusLine").is_none() {
        repairs.push(Repair::StatusLine);
    }
    match (found.keeper_on, found.keeper_running) {
        (true, false) => repairs.push(Repair::KeeperStart),
        (false, true) => repairs.push(Repair::KeeperStop),
        _ => {}
    }
    if herdr_plugin(found.herdr_list, config::Channel::default()).1.is_some_and(|plugin| !plugin.enabled) {
        repairs.push(Repair::HerdrPlugin);
    }
    repairs
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
    pub kebbie_state: PathBuf,
    pub own_state: bool,
    pub data_home: Option<PathBuf>,
    pub claude_dir: PathBuf,
    pub codex_dir: PathBuf,
    pub profiles_dir: PathBuf,
    pub config: PathBuf,
    pub repos_dir: PathBuf,
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
            kebbie_state: crate::lead::state_location().unwrap_or_else(|_| home.join(".local/state/kebbie")),
            own_state: var(crate::lead::STATE_DIR_VAR).is_some(),
            data_home: var("XDG_DATA_HOME"),
            claude_dir: var("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            codex_dir: var("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            profiles_dir: crate::profiles::dir(),
            config: config::path(),
            repos_dir: config::repos_dir(),
            repo_config: std::env::current_dir().ok().filter(|cwd| crate::repo::main_checkout(cwd).is_ok()).map(|cwd| config::repo_file(&cwd)),
            repo: std::env::current_dir().ok().and_then(|cwd| crate::repo::main_checkout(&cwd).ok()),
            uid: run("id", &["-u"], None).ok().and_then(|uid| uid.parse().ok()).unwrap_or(u32::MAX),
            home,
        }
    }
}

pub const MIN_CODEX: &str = "0.163.0";

// Codex is optional until a profile runs it; then its binary, kebbie's plugin and nothing in the user's config that turns hooks off are what a Codex task needs to report.
pub fn codex(found: &crate::agent::codex::Availability, config: Option<&toml::Table>, needed: bool, home: &Path, signed: Option<Login>) -> Vec<Check> {
    use crate::agent::codex::Installed;
    let missing = |name: &str, fact: String, fix: Vec<String>| if needed { fail(name, fact, fix) } else { warn(name, fact, fix) };
    let install = both("curl -fsSL https://chatgpt.com/codex/install.sh | sh (or brew install --cask codex)", "curl -fsSL https://chatgpt.com/codex/install.sh | sh (or npm install -g @openai/codex)");
    let Some(binary) = &found.binary else {
        return vec![if needed { fail("codex", "not on PATH; a profile here runs Codex", install) } else { ok("codex", "not on PATH; only a profile with agent = \"codex\" needs it").brief("not installed") }];
    };
    let want = version(MIN_CODEX).unwrap_or_default();
    let mut checks = vec![match found.version.as_deref().and_then(version) {
        Some(have) if at_least(&have, &want) => ok("codex", format!("{} at {}", shown(&have), binary.display())).brief(shown(&have)),
        Some(have) => missing("codex", format!("{} is older than {MIN_CODEX}, whose plugin hooks kebbie's Codex tasks need", shown(&have)), one("codex update")),
        None => missing("codex", format!("cannot read the version from {} --version; need {MIN_CODEX}", binary.display()), one("codex update")),
    }];
    let root = crate::agent::codex::plugin_root(home);
    checks.push(match &found.plugin {
        Installed::Current => ok("codex plugin", format!("kebbie's plugin at {}", root.display())).brief("plugin"),
        Installed::Missing => missing("codex plugin", "kebbie's plugin is not installed, so a Codex task neither reports nor gets its role".to_string(), one("kebbie setup codex")),
        Installed::Stale(files) => missing("codex plugin", format!("kebbie's plugin is out of date ({})", files.join(", ")), one("kebbie setup codex")),
        Installed::Replaced(copies) => missing("codex plugin", format!("Codex replaced kebbie's plugin with {}, a copy from a repo's marketplace of the same name", crate::agent::codex::shown_paths(copies)), one("kebbie setup codex")),
        Installed::UnderOldId(old) => missing("codex plugin", format!("kebbie's plugin is at {}, under kebbie@kebbie, which a repo's marketplace named kebbie replaces; kebbie setup codex moves it to {}", old.display(), root.display()), one("kebbie setup codex")),
    });
    let old = crate::agent::codex::old_copies(home);
    if !old.is_empty() {
        checks.push(warn("codex old plugin", format!("{} is an unused copy under kebbie@kebbie, which Codex installed from a repo's marketplace named kebbie; kebbie runs its own plugin as kebbie@kebbie-tasks", crate::agent::codex::shown_paths(&old)), old.iter().map(|path| format!("rm -rf {}", crate::sh::quote(&path.to_string_lossy()))).collect()));
    }
    let blocked = crate::agent::codex::hooks_blocked(config);
    checks.push(if blocked.is_empty() {
        ok("codex hooks", format!("kebbie enables its plugin and trusts its {} hooks and the plugin's files by hash in each Codex session it starts", crate::agent::codex::hook_count())).brief("hooks")
    } else {
        missing("codex hooks", format!("{}: {}", home.join("config.toml").display(), blocked.join("; ")), one("turn them back on in Codex's /hooks, or drop [features] hooks = false"))
    });
    if let Some(model) = config.and_then(crate::agent::codex::model_in) {
        checks.push(ok("codex model", format!("{model}, from {}, for a Codex profile without model", home.join("config.toml").display())).brief(format!("model {model}")));
    }
    checks.extend(signed.map(|signed| match login("codex login", signed, crate::agent::login::CODEX_FIX, "codex tasks cannot start") {
        check if check.level == Level::Warn && needed => Check { level: Level::Fail, ..check },
        check => check,
    }));
    checks
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok())
}

pub fn checks(env: &Env) -> Vec<Check> {
    let mut checks = Vec::new();
    let mut herdr = None;
    let mut cargo = None;
    for tool in tools(&min_herdr()) {
        let found = which(tool.name, &env.path);
        if tool.name == "cargo" {
            cargo = Some((tool, found));
            continue;
        }
        let output = found.as_ref().filter(|_| tool.min.is_some()).and_then(|found| run(&found.to_string_lossy(), &["--version"], None).ok());
        if tool.name == "herdr" {
            herdr = found.clone();
        }
        checks.push(self::tool(&tool, found.as_deref(), output.as_deref()));
    }
    if herdr.is_some() {
        let running: Vec<_> = crate::lead::running_boards().unwrap_or_default().into_iter().map(|board| { let started = crate::restart::started(board.pid); (board, started) }).collect();
        checks.extend(boards(&running, crate::lead::written_at()));
    }
    let list = herdr.and_then(|herdr| run(&herdr.to_string_lossy(), &["plugin", "list", "--json", "--plugin", "kebbie"], None).ok());
    let channel = config::channel_in(&env.config).unwrap_or_default();
    let (check, plugin) = herdr_plugin(list.as_deref(), channel);
    checks.push(check);
    let link = list.as_deref().and_then(|list| serde_json::from_str(list).ok()).map_or(crate::herdr_link::Link::Unregistered, |list| crate::herdr_link::read(&list, crate::herdr_link::target(&env.home, crate::peers::own_exe().ok().as_deref()).as_deref()));
    let gone = matches!(link, crate::herdr_link::Link::Gone { .. });
    checks.extend(herdr_link(&link));
    if let Some(plugin) = plugin.as_ref().filter(|_| !gone) {
        checks.push(plugin_binary(&plugin.root));
    }
    if let Some((tool, found)) = &cargo {
        let origin = plugin.as_ref().and_then(|plugin| crate::update::binary_origin(&plugin.root));
        checks.push(self::cargo(tool, found.as_deref(), channel, origin.as_ref()));
    }
    let installed = read_json(&env.claude_dir.join("plugins/installed_plugins.json"));
    let claude_settings = read_json(&env.claude_dir.join("settings.json")).unwrap_or_default();
    let checkout = checkout_version(plugin.as_ref());
    checks.push(claude_plugin(installed.as_ref(), &claude_settings, checkout.as_deref(), channel));
    if let Some(claude) = which("claude", &env.path) {
        checks.push(login("claude login", crate::agent::login::claude(Some(&claude), crate::agent::login::TIMEOUT), crate::agent::login::CLAUDE_FIX, "the lead, the reviewer and tasks cannot start"));
    }
    let kebbie_id = installed.as_ref().and_then(|installed| installed["plugins"].as_object()?.keys().find(|id| id.starts_with("kebbie@")).cloned());
    if let Some(id) = kebbie_id {
        checks.push(plugin_source(&crate::update::marketplace(&env.claude_dir, &id), channel));
    }
    checks.push(status_line(&claude_settings));
    checks.push(current_link(&env.home, env.data_home.as_deref(), plugin.as_ref().filter(|_| !gone).map(|plugin| plugin.root.as_path())));
    checks.push(kebbie_on_path(&env.home, &env.path));
    let kebbie = crate::peers::kebbie_command().unwrap_or_default();
    let codex_config = std::fs::read_to_string(env.codex_dir.join("config.toml")).ok().and_then(|text| text.parse::<toml::Table>().ok());
    let found = crate::agent::codex::availability(&env.path, Some(&env.codex_dir), &kebbie);
    let chosen = crate::agent::chosen_with(&found.plugin);
    let needed = chosen.contains(&crate::agent::Agent::Codex) && crate::profiles::list_in(&env.profiles_dir).0.iter().any(|profile| profile.agent == crate::agent::Agent::Codex);
    let signed = (needed || found.plugin != crate::agent::codex::Installed::Missing).then(|| crate::agent::login::codex(found.binary.as_deref(), crate::agent::login::TIMEOUT)).filter(|_| found.binary.is_some());
    checks.extend(codex(&found, codex_config.as_ref(), needed, &env.codex_dir, signed));
    if found.binary.is_some() && (needed || found.plugin != crate::agent::codex::Installed::Missing) {
        let rules = crate::agent::codex_setup::rules_text(&crate::agent::codex_setup::rule_forms(&kebbie, &env.home.join(".local/bin/kebbie")));
        let mode = config::check_machine(&env.config).ok().and_then(|(machine, _)| machine.agent_updates).unwrap_or_default();
        let vault_command: Option<Vec<String>> = config::check_machine(&env.config).ok().and_then(|(machine, _)| machine.worktree.vault_command).filter(|command| !command.trim().is_empty()).map(|command| command.split_whitespace().map(str::to_string).collect());
        let vault_on = vault_command.is_some() || matches!(crate::vault_cli::plugin(&env.claude_dir), crate::vault_cli::Plugin::Enabled { .. });
        let checked = crate::agent_check::read(&env.kebbie_state);
        checks.extend(crate::agent_check::doctor(&crate::agent_check::Doctor { found: checked.as_ref(), mode, codex_home: &env.codex_dir, claude_dir: &env.claude_dir, rules: &rules, vault_on, vault_command: vault_command.as_deref() }));
    }
    let places = crate::update_check::Places::new(plugin.as_ref().map(|plugin| plugin.root.clone()), env.claude_dir.clone(), channel);
    checks.push(updates(&env.kebbie_state, channel, &crate::update_check::installed(&places)));
    let keeper_on = config::check_machine(&env.config).ok().and_then(|(machine, _)| machine.keeper).unwrap_or(true);
    checks.push(keeper(keeper_on, crate::keeper::holder(&env.kebbie_state).as_ref(), crate::messages::now()));
    checks.push(crate::hub::doctor(&env.kebbie_state, crate::hub::configured_name(&env.config).as_deref(), crate::messages::now(), crate::hub::reach));
    if let Ok((machine, _)) = config::check_machine(&env.config) {
        checks.push(crate::signals::doctor(&env.kebbie_state, &machine.report, crate::messages::now()));
        checks.extend(crate::hub::inbox::doctor(&env.kebbie_state, &machine.report, crate::messages::now()));
    }
    let data_home = env.data_home.clone().unwrap_or_else(|| env.home.join(".local/share"));
    for (dir, about) in state_dirs(&env.kebbie_state, env.own_state, &env.state_home, &data_home) {
        checks.push(writable(&dir, about, env.uid));
    }
    let (check, machine) = config_file(&env.config);
    checks.push(check);
    let (repo_checks, own) = repo_files(&env.repos_dir, env.repo_config.as_deref());
    checks.extend(repo_checks);
    checks.extend(task_profiles(&env.profiles_dir, &named_profiles(machine.as_ref(), &env.config, &env.repos_dir)));
    checks.extend(claude_models(&crate::profiles::list_in(&env.profiles_dir).0));
    checks.extend(kubeconfigs(&crate::profiles::list_in(&env.profiles_dir).0, &env.home));
    checks.extend(unchosen_profiles(&crate::profiles::list_in(&env.profiles_dir).0, &chosen));
    checks.extend(config::resolve(env.repo.as_deref()).ok().and_then(|loaded| crate::review::doctor(&env.profiles_dir, &loaded.settings)).map(|wrong| warn("reviewer", format!("kebbie review refuses: {wrong}"), one("pick a review profile for review.profile and review.second on the Settings page (,)"))));
    checks.extend(env.repo.as_deref().map(push_block));
    let repos = known_repos();
    checks.push(task_hooks(&sessions_with_gone_hooks(&env.kebbie_state.join("roles"), &open_addresses(&repos))));
    let (Some(machine), Some(own)) = (machine, own) else {
        return checks;
    };
    let settings = config::settings(&own, &machine);
    checks.push(setup_skill(&settings.setup_skill, installed.as_ref(), &claude_settings));
    checks.extend(env.repo.as_deref().and_then(|repo| setup_command(&settings.setup_command, repo)));
    if let (Some(repo), Some(file)) = (env.repo.as_deref(), env.repo_config.as_deref()) {
        checks.extend(docs(&settings.docs_required, || visibility(gh_on(&env.path).as_deref(), repo, GH_TIMEOUT), file));
        let short = || short_full_run(&repo_runs(&env.kebbie_state, repo), &crate::evidence::listed_for(&settings));
        checks.extend(tests(&settings.tests_before_push, settings.tests_full_command.value.as_deref(), short, file));
    }
    if let Some(repo) = env.repo.as_deref() {
        checks.push(comments(&settings.comments_watch, settings.comments_days.value, || crate::ci::target(repo, &crate::ci::gh())));
    }
    checks.push(notify(&settings.notify_command));
    checks.push(sandbox(&settings.sandbox, &crate::sandbox::missing_here()));
    let plugin = crate::vault_cli::plugin(&env.claude_dir);
    let override_bin = std::env::var(crate::vault_cli::BIN_VAR).ok().filter(|bin| !bin.is_empty());
    let command = crate::vault_cli::command(override_bin, settings.vault_command.value.clone(), &plugin);
    let reached = command.is_some();
    let probe = command.as_ref().map(|(command, _)| crate::vault_cli::Cli::new(command.clone()).probe(true));
    checks.push(vault(&plugin, command.as_ref(), probe));
    if reached || crate::hub::identity::load(&env.kebbie_state).ok().flatten().is_some() {
        checks.push(machine_name(&settings.machine_name, &env.config));
    }
    let local_dir = env.config.with_file_name("backlog");
    checks.extend(config::load_machine(&env.config).ok().and_then(|machine| machine.sources).and_then(|listed| mixed_sources(&listed, &local_dir, &env.config)));
    checks.extend(backlog(&settings.sources.value, crate::vault::source, &local_dir));
    checks.extend(item_template(crate::item_template::configured_file(&env.config).as_deref()));
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
            Level::Skip => "skip",
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
    let skipped = count(Level::Skip);
    let skipped = if skipped > 0 { format!(", {skipped} not checked") } else { String::new() };
    lines.push(format!("{} failed, {} warnings, {} ok{skipped}", count(Level::Fail), count(Level::Warn), count(Level::Ok)));
    lines.join("\n")
}

pub fn failed(checks: &[Check]) -> bool {
    checks.iter().any(|check| check.level == Level::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_started_before_the_installed_kebbie_was_built_is_named_as_running_old_code() {
        let board = |space: &str, pid: u32| crate::lead::Board { pane: format!("{space}:p1"), space: space.to_string(), pid };
        assert_eq!(boards(&[], Some(100)), None, "no board, nothing to say");
        let fresh = boards(&[(board("lead", 7), Some(150))], Some(100)).unwrap();
        assert_eq!((fresh.level, fresh.fact.as_str()), (Level::Ok, "running on the installed kebbie"));
        let old = boards(&[(board("lead", 7), Some(150)), (board("work", 9), Some(50))], Some(100)).unwrap();
        assert_eq!(old.level, Level::Warn);
        assert_eq!(old.fact, "the board in work (pane work:p1, pid 9) runs an older kebbie than the one installed");
        assert_eq!(old.fix, ["kebbie lead --renew-board restarts it on the new one"]);
    }

    #[test]
    fn task_hooks_name_the_sessions_whose_kebbie_is_gone_and_how_to_mend_them() {
        assert_eq!(task_hooks(&[]).level, Level::Ok);
        let found = task_hooks(&["billing@app".to_string()]);
        assert_eq!(found.level, Level::Warn);
        assert!(found.fact.contains("the hooks of billing@app run a kebbie that is gone"), "{}", found.fact);
        assert!(found.fix[0].starts_with("kebbie restart"));
        let roles = crate::sh::test_dir("doctor-gone-hooks");
        std::fs::write(roles.join("billing@app.settings.json"), crate::agent::claude::settings("/home/u/.local/share/kebbie/gone/target/release/kebbie", "billing@app", None, None, None)).unwrap();
        let kebbie = roles.join("kebbie");
        crate::executable::write(&kebbie, "#!/bin/sh\n").unwrap();
        std::fs::write(roles.join("search@app.settings.json"), crate::agent::claude::settings(&kebbie.to_string_lossy(), "search@app", None, None, None)).unwrap();
        let addresses = ["billing@app".to_string(), "search@app".to_string(), "lead".to_string()];
        let gone = sessions_with_gone_hooks(&roles, &addresses);
        assert_eq!(gone, ["billing@app"], "a kebbie that runs, and a session with no settings file, are fine");
        std::fs::remove_dir_all(roles).unwrap();
    }
    use crate::config::Channel;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-doctor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn executable(path: &Path) {
        crate::executable::write(path, "#!/bin/sh\n").unwrap();
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
        std::fs::write(dir.join("nvim"), "not executable").unwrap();
        let path = vec![dir.clone()];
        assert_eq!(which("herdr", &path), Some(dir.join("herdr")));
        assert_eq!(which("nvim", &path), None);
        let tools = tools("0.9.0");
        assert_eq!(tool(named(&tools, "herdr"), Some(&dir.join("herdr")), Some("herdr 0.9.5")).level, Level::Ok);
        let old = tool(named(&tools, "herdr"), Some(&dir.join("herdr")), Some("herdr 0.8.2"));
        assert_eq!((old.level, old.fact.as_str()), (Level::Fail, "0.8.2 is older than 0.9.0, needed for everything"));
        let nvim = tool(named(&tools, "nvim"), Some(&dir.join("nvim")), Some("NVIM v0.9.5"));
        assert_eq!(nvim.level, Level::Warn);
        assert!(nvim.fact.contains("review split"));
        let missing = tool(named(&tools, "jq"), None, None);
        assert_eq!((missing.level, missing.fix.len()), (Level::Fail, 2));
        assert!(missing.fix[0].starts_with("macOS: brew") && missing.fix[1].starts_with("Linux: "));
        assert!(tools.iter().all(|tool| tool.name != "setsid"), "the board fetches; nothing needs setsid");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_herdr_plugin_is_read_from_plugin_list() {
        let list = r#"{"result":{"plugins":[{"plugin_id":"kebbie","enabled":true,"plugin_root":"/src/kebbie","version":"0.11.0"}],"type":"plugin_list"}}"#;
        let (check, plugin) = herdr_plugin(Some(list), Channel::Release);
        assert_eq!((check.level, check.fact.as_str()), (Level::Ok, "kebbie 0.11.0 enabled, at /src/kebbie"));
        assert_eq!(herdr_plugin(Some(list), Channel::Main).0.fact, "kebbie main enabled, at /src/kebbie", "the main channel names no number");
        assert_eq!(plugin.unwrap().root, PathBuf::from("/src/kebbie"));
        let (check, _) = herdr_plugin(Some(&list.replace("true", "false")), Channel::Release);
        assert_eq!((check.level, check.fix[0].as_str()), (Level::Fail, "herdr plugin enable kebbie"));
        let (check, plugin) = herdr_plugin(Some(r#"{"result":{"plugins":[]}}"#), Channel::Release);
        assert!(check.level == Level::Fail && plugin.is_none() && check.fix[0].contains("herdr plugin install"));
        let dir = scratch("plugin");
        let missing = plugin_binary(&dir);
        assert!(missing.level == Level::Fail && missing.fix[0].starts_with("sh scripts/build.sh in "), "{missing:?}");
        std::fs::create_dir_all(dir.join("target/release")).unwrap();
        executable(&dir.join("target/release/kebbie"));
        assert_eq!(plugin_binary(&dir).fact, format!("{}, built from source", dir.join("target/release/kebbie").display()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_plugin_binary_says_where_it_came_from_and_whether_cargo_is_needed() {
        use crate::update::{Origin, Provenance, binary_origin};
        let dir = scratch("origin");
        std::fs::create_dir_all(dir.join("target/release")).unwrap();
        executable(&dir.join("target/release/kebbie"));
        std::fs::write(dir.join("target/release/kebbie.origin"), "downloaded v0.24.0 aarch64-apple-darwin\n").unwrap();
        let downloaded = Origin::Downloaded { tag: "v0.24.0".to_string(), target: "aarch64-apple-darwin".to_string(), provenance: Provenance::Unattested };
        assert_eq!(binary_origin(&dir).as_ref(), Some(&downloaded));
        assert!(plugin_binary(&dir).fact.ends_with(", downloaded v0.24.0 for aarch64-apple-darwin"));
        std::fs::write(dir.join("target/release/kebbie.origin"), "downloaded v0.37.0 aarch64-apple-darwin verified\n").unwrap();
        let verified = plugin_binary(&dir);
        assert!(verified.level == Level::Ok && verified.fact.ends_with(", downloaded v0.37.0 for aarch64-apple-darwin, its attestation verified"), "{verified:?}");
        std::fs::write(dir.join("target/release/kebbie.origin"), "downloaded v0.37.0 aarch64-apple-darwin unverified\n").unwrap();
        let unverified = plugin_binary(&dir);
        assert!(unverified.level == Level::Warn && unverified.fact.ends_with(", its origin not verified") && unverified.fix[0].starts_with("install the GitHub CLI (gh)"), "an installed binary nothing checked is a warning, not a failure: {unverified:?}");
        std::fs::write(dir.join("target/release/kebbie.origin"), "downloaded v0.24.0 aarch64-apple-darwin\n").unwrap();
        let tools = tools("0.9.0");
        let cargo_tool = named(&tools, "cargo");
        assert_eq!(cargo(cargo_tool, None, Channel::Release, Some(&downloaded)).level, Level::Ok, "a release machine with a downloaded binary needs no Rust");
        assert_eq!(cargo(cargo_tool, None, Channel::Main, Some(&downloaded)).level, Level::Warn);
        assert_eq!(cargo(cargo_tool, None, Channel::Release, Some(&Origin::Built)).level, Level::Warn);
        assert_eq!(cargo(cargo_tool, Some(&dir.join("target/release/kebbie")), Channel::Main, None).level, Level::Ok);
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(dir.join("target/release/kebbie")).unwrap().set_modified(later).unwrap();
        assert_eq!(binary_origin(&dir), Some(Origin::Built), "a binary cargo replaced after the download is built");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_claude_plugin_is_read_from_installed_plugins() {
        let installed: Value = serde_json::from_str(r#"{"version":2,"plugins":{"kebbie@kebbie":[{"version":"0.11.0"}]}}"#).unwrap();
        assert_eq!(claude_plugin(Some(&installed), &Value::Null, None, Channel::Release).fact, "kebbie@kebbie 0.11.0");
        let disabled: Value = serde_json::from_str(r#"{"enabledPlugins":{"kebbie@kebbie":false}}"#).unwrap();
        assert_eq!(claude_plugin(Some(&installed), &disabled, None, Channel::Release).fix, ["claude plugin enable kebbie@kebbie"]);
        let other: Value = serde_json::from_str(r#"{"plugins":{"vault@x":[{}]}}"#).unwrap();
        assert_eq!(claude_plugin(Some(&other), &Value::Null, None, Channel::Release).level, Level::Warn);
        assert_eq!(claude_plugin(None, &Value::Null, None, Channel::Release).level, Level::Warn);
        let behind = claude_plugin(Some(&installed), &Value::Null, Some("0.12.0"), Channel::Release);
        assert_eq!((behind.level, behind.fix), (Level::Warn, vec!["kebbie update".to_string()]), "a cached copy older than the checkout is a warning");
        assert_eq!(claude_plugin(Some(&installed), &Value::Null, Some("0.11.0"), Channel::Release).level, Level::Ok);
        assert_eq!(claude_plugin(Some(&installed), &Value::Null, None, Channel::Main).fact, "kebbie@kebbie main", "the main channel names no number");
        let behind = claude_plugin(Some(&installed), &Value::Null, Some("0.12.0"), Channel::Main);
        assert!(behind.level == Level::Warn && !behind.fact.contains("0.1"), "{}", behind.fact);
    }

    #[test]
    fn a_release_machine_warns_while_the_claude_plugin_still_follows_github() {
        use crate::update::Marketplace;
        let warned = plugin_source(&Marketplace::Git, Channel::Release);
        assert_eq!((warned.level, warned.fix.clone()), (Level::Warn, vec![crate::update::IN_PLACE_FIX.to_string()]));
        assert_eq!(plugin_source(&Marketplace::Git, Channel::Main).level, Level::Ok);
        assert_eq!(plugin_source(&Marketplace::InPlace(PathBuf::from("/k")), Channel::Release).fact, "loaded in place from /k");
    }

    #[test]
    fn the_current_link_must_resolve() {
        let home = scratch("home");
        let missing = current_link(&home, None, None);
        assert!(missing.level == Level::Fail && missing.fix[0].starts_with("start herdr once"));
        let root = home.join("plugin");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(home.join(".local/share/kebbie")).unwrap();
        std::os::unix::fs::symlink(home.join("gone"), home.join(".local/share/kebbie/current")).unwrap();
        assert!(current_link(&home, None, None).fact.ends_with("points nowhere"));
        std::fs::remove_file(home.join(".local/share/kebbie/current")).unwrap();
        std::os::unix::fs::symlink(&root, home.join(".local/share/kebbie/current")).unwrap();
        assert_eq!(current_link(&home, None, Some(&root)).level, Level::Ok);
        assert_eq!(current_link(&home, None, Some(&home)).level, Level::Warn);
        assert_eq!(current_link(&home, Some(&home.join("data")), Some(&root)).level, Level::Warn);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn a_herdr_link_to_a_missing_folder_fails_with_the_relink_and_a_healthy_one_is_ok() {
        use crate::herdr_link::Link;
        let gone = herdr_link(&Link::Gone { linked: PathBuf::from("/home/me/projects/kebbie-old"), target: Some(PathBuf::from("/home/me/projects/kebbie")) }).unwrap();
        assert_eq!((gone.level, gone.name.as_str(), gone.fact.as_str()), (Level::Fail, "herdr link", "herdr links kebbie from /home/me/projects/kebbie-old, which is gone, so its startup starts neither the board nor the keeper"));
        assert_eq!(gone.fix, ["kebbie doctor --fix links it again from /home/me/projects/kebbie and starts both; a running keeper does it within the hour", "or herdr plugin unlink kebbie && herdr plugin link /home/me/projects/kebbie"]);
        let nowhere = herdr_link(&Link::Gone { linked: PathBuf::from("/k"), target: None }).unwrap();
        assert!(nowhere.level == Level::Fail && nowhere.fact.contains("no kebbie checkout is found"), "{}", nowhere.fact);
        assert_eq!(herdr_link(&Link::Linked(PathBuf::from("/src/kebbie"))).map(|check| (check.level, check.fact)), Some((Level::Ok, "linked from /src/kebbie".to_string())));
        assert_eq!(herdr_link(&Link::Installed).map(|check| check.level), Some(Level::Ok), "a GitHub install is never a link to fix");
        assert_eq!(herdr_link(&Link::Unregistered), None, "the herdr plugin line names an unregistered kebbie");
    }

    #[test]
    fn kebbie_on_path_must_be_the_current_links_binary() {
        let home = scratch("on-path");
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let missing = kebbie_on_path(&home, std::slice::from_ref(&bin));
        assert!(missing.level == Level::Warn && missing.fix[0] == LINK_KEBBIE && missing.fix[1].contains("~/.local/bin is on PATH"));
        let release = home.join("plugin/target/release");
        std::fs::create_dir_all(&release).unwrap();
        executable(&release.join("kebbie"));
        std::fs::create_dir_all(home.join(".local/share/kebbie")).unwrap();
        std::os::unix::fs::symlink(home.join("plugin"), home.join(".local/share/kebbie/current")).unwrap();
        std::os::unix::fs::symlink(home.join(".local/share/kebbie/current/target/release/kebbie"), bin.join("kebbie")).unwrap();
        assert_eq!(kebbie_on_path(&home, std::slice::from_ref(&bin)).level, Level::Ok);
        let stale = home.join("cargo/bin");
        std::fs::create_dir_all(&stale).unwrap();
        executable(&stale.join("kebbie"));
        let shadowed = kebbie_on_path(&home, &[stale.clone(), bin.clone()]);
        assert_eq!(shadowed.level, Level::Warn);
        assert!(shadowed.fact.contains(&stale.join("kebbie").canonicalize().unwrap().display().to_string()));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn state_dirs_must_be_writable_or_creatable() {
        let base = scratch("state");
        let me = uid(&base);
        let created = writable(&base.join("kebbie/roles"), "roles", me);
        assert!(created.level == Level::Ok && created.fact.contains("created on first use"), "{created:?}");
        let locked = base.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let check = writable(&locked.join("kebbie"), "x", me);
        assert_eq!((check.level, check.fix[0].clone()), (Level::Fail, format!("chmod u+w {}", locked.display())));
        let foreign = writable(&base.join("kebbie"), "x", me.wrapping_add(1));
        assert!(foreign.level == Level::Fail && foreign.fix[0].starts_with("sudo chown -R \"$(id -un)\" "), "{foreign:?}");
        std::fs::write(base.join("file"), "").unwrap();
        assert!(writable(&base.join("file/kebbie"), "x", me).fact.contains("is not a directory"));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(base);
        let dirs = state_dirs(Path::new("/s/kebbie"), false, Path::new("/s"), Path::new("/d"));
        assert!(dirs.iter().any(|(dir, _)| dir == Path::new("/s/herdr-sync")) && dirs.iter().any(|(dir, _)| dir == Path::new("/d/kebbie")));
        assert!(dirs.iter().all(|(dir, _)| dir != Path::new("/s/herdr-sync-fetch")));
    }

    #[test]
    fn an_own_state_dir_holds_every_kebbie_dir_and_says_where_it_came_from() {
        let dirs = state_dirs(Path::new("/probe/state"), true, Path::new("/s"), Path::new("/d"));
        let kebbie: Vec<&PathBuf> = dirs.iter().map(|(dir, _)| dir).filter(|dir| !dir.starts_with("/s") && !dir.starts_with("/d")).collect();
        assert_eq!(kebbie, [Path::new("/probe/state"), Path::new("/probe/state/roles"), Path::new("/probe/state/turns"), Path::new("/probe/state/reviews")]);
        assert!(!dirs.iter().any(|(dir, _)| dir.starts_with("/s/kebbie")));
        assert!(dirs[0].1.ends_with("set by KEBBIE_STATE_DIR"));
    }

    #[test]
    fn the_machine_and_repo_configs_are_checked() {
        let dir = scratch("config");
        let file = dir.join("config.toml");
        assert!(config_file(&file).0.fact.ends_with("not found; every machine setting is its default"));
        std::fs::write(&file, "[board.backlog]\nsources = [\"jira\"]\n").unwrap();
        let (check, config) = config_file(&file);
        assert!(check.level == Level::Fail && config.is_none() && check.fact.contains("board.backlog.sources"));
        std::fs::write(&file, "[update]\nchannel = \"main\"\n\n[board]\nlanes = 2\n\n[signals]\nsample = 0.5\n").unwrap();
        let (check, config) = config_file(&file);
        assert!(check.level == Level::Warn && check.fact.contains("signals: unknown key") && check.fact.contains("board.lanes: unknown key"), "{check:?}");
        assert_eq!(check.fix, ["a newer kebbie may know it; kebbie update"]);
        assert_eq!(config.and_then(|config| config.channel), Some(crate::config::Channel::Main), "the rest of the file still applies");
        std::fs::write(&file, "[restart]\nidle_minutes = \"soon\"\n").unwrap();
        let (check, _) = config_file(&file);
        assert_eq!(check.level, Level::Fail, "a known key with the wrong type still fails");
        assert!(check.fact.contains("the hub takes no commands and approves devices only at this machine"), "{}", check.fact);
        let repos = dir.join("repos");
        std::fs::create_dir_all(&repos).unwrap();
        let own = repos.join("app.toml");
        assert_eq!(repo_file(&own).0.level, Level::Ok);
        std::fs::write(&own, "[worktree]\ndev_server = \"bin/dev\"\nport = 3000\n").unwrap();
        let (check, worktree) = repo_file(&own);
        assert!(check.level == Level::Warn && check.fact.contains("worktree.port: unknown key") && check.fix == ["a newer kebbie may know it; kebbie update"], "{check:?}");
        assert_eq!(worktree.and_then(|worktree| worktree.dev_server).as_deref(), Some("bin/dev"));
        std::fs::write(&own, "[ci]\nwatch = \"yes\"\n").unwrap();
        assert_eq!(repo_file(&own).0.level, Level::Fail, "a known key with the wrong type still fails");
        std::fs::write(&own, "[[actions]]\nkey = \"B\"\ntitle = \"Build\"\nrun = \"cargo build\"\ncolour = \"red\"\n").unwrap();
        let (check, _) = repo_file(&own);
        assert!(check.level == Level::Fail && check.fact.contains("actions[0].colour: unknown key"), "an action's error fails the check too: {check:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn doctor_checks_every_repo_config_and_shows_where_a_link_points() {
        let dir = scratch("repos");
        let (team, repos) = (dir.join("team"), dir.join("repos"));
        std::fs::create_dir_all(&team).unwrap();
        std::fs::create_dir_all(&repos).unwrap();
        std::fs::write(team.join("app.toml"), "[worktree]\ndev_server = \"bin/dev\"\n").unwrap();
        std::os::unix::fs::symlink(team.join("app.toml"), repos.join("app.toml")).unwrap();
        std::os::unix::fs::symlink(team.join("gone.toml"), repos.join("gone.toml")).unwrap();
        std::fs::write(repos.join("notes.txt"), "not a config").unwrap();
        let (checks, own) = repo_files(&repos, Some(&repos.join("app.toml")));
        assert_eq!(checks.len(), 2, "{checks:?}");
        assert_eq!(checks[0].level, Level::Ok);
        assert!(checks[0].fact.contains(&format!("{} -> {} (found) parses", repos.join("app.toml").display(), team.join("app.toml").display())), "{:?}", checks[0]);
        assert_eq!(own.unwrap().dev_server.as_deref(), Some("bin/dev"));
        assert_eq!(checks[1].level, Level::Fail);
        assert!(checks[1].fact.contains("the link points at nothing"), "{:?}", checks[1]);
        let (checks, own) = repo_files(&repos, Some(&repos.join("web.toml")));
        assert!(checks[2].level == Level::Ok && checks[2].fact.contains("web.toml (not found); config.toml and the defaults apply"), "{:?}", checks[2]);
        assert_eq!(own, Some(Worktree::default()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_status_line_check_advises_kebbies_command_and_never_fails() {
        let set = status_line(&serde_json::json!({ "statusLine": { "type": "command", "command": "~/.local/share/kebbie/current/target/release/kebbie statusline" } }));
        assert_eq!(set.level, Level::Ok);
        let unset = status_line(&serde_json::json!({}));
        assert_eq!(unset.level, Level::Warn);
        assert!(unset.fix[0].contains("kebbie statusline") && unset.fix[0].contains("~/.claude/settings.json"), "{:?}", unset.fix);
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
        let installed: Value = serde_json::from_str(r#"{"version":2,"plugins":{"kebbie@kebbie":[{"version":"0.11.0"}],"my-tools@private":[{"version":"1.0.0"}]}}"#).unwrap();
        let default = setup_skill(&skill("kebbie:set-up-worktree", config::Layer::Default), Some(&installed), &Value::Null);
        assert_eq!((default.level, default.fact.as_str()), (Level::Ok, "kebbie:set-up-worktree (default): kebbie@kebbie"));
        assert_eq!(setup_skill(&skill("my-tools:set-up-worktree", config::Layer::Repo), Some(&installed), &Value::Null).level, Level::Ok);
        let missing = setup_skill(&skill("work-tools:set-up-worktree", config::Layer::Machine), Some(&installed), &Value::Null);
        assert_eq!(missing.level, Level::Warn);
        assert!(missing.fact.starts_with("work-tools:set-up-worktree (machine config): the plugin work-tools is not installed"), "{}", missing.fact);
        assert_eq!(missing.fix[0], "claude plugin install work-tools@<marketplace>");
        assert_eq!(setup_skill(&skill("kebbie:set-up-worktree", config::Layer::Default), None, &Value::Null).level, Level::Warn);
        let disabled: Value = serde_json::from_str(r#"{"enabledPlugins":{"my-tools@private":false}}"#).unwrap();
        let off = setup_skill(&skill("my-tools:set-up-worktree", config::Layer::Repo), Some(&installed), &disabled);
        assert_eq!((off.level, off.fix[0].as_str()), (Level::Warn, "claude plugin enable my-tools@private"));
        assert_eq!(setup_skill(&skill("set-up-worktree", config::Layer::Machine), None, &Value::Null).level, Level::Ok);
        let dir = crate::sh::test_dir("doctor-setup-file");
        std::fs::write(dir.join("SETUP.md"), "Run make deps.\n").unwrap();
        let file = setup_skill(&skill(&dir.join("SETUP.md").display().to_string(), config::Layer::Repo), None, &Value::Null);
        assert_eq!((file.level, file.fact.ends_with("a file a task reads")), (Level::Ok, true), "{file:?}");
        let gone = setup_skill(&skill(&dir.join("gone.md").display().to_string(), config::Layer::Repo), None, &Value::Null);
        assert!(gone.level == Level::Warn && gone.fact.contains("no file at"), "{gone:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_setup_skill_must_be_in_the_installed_plugin_version() {
        let dir = std::env::temp_dir().join(format!("kebbie-doctor-skill-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills/set-up-worktree")).unwrap();
        std::fs::write(dir.join("skills/set-up-worktree/SKILL.md"), "").unwrap();
        let installed = serde_json::json!({"plugins": {"kebbie@kebbie": [{"version": "0.12.0", "installPath": dir.to_str().unwrap()}]}});
        let skill = |value: &str| Setting { value: value.to_string(), layer: config::Layer::Default };
        assert_eq!(setup_skill(&skill("kebbie:set-up-worktree"), Some(&installed), &Value::Null).level, Level::Ok);
        let stale = setup_skill(&skill("kebbie:make-worktree-safe"), Some(&installed), &Value::Null);
        assert_eq!((stale.level, stale.fact.as_str(), stale.fix[0].as_str()), (Level::Warn, "kebbie:make-worktree-safe (default): kebbie@kebbie 0.12.0 has no skill make-worktree-safe", "claude plugin update kebbie@kebbie"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_set_setup_command_must_be_an_executable_in_the_main_checkout() {
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
        crate::executable::write(&script, "#!/bin/sh\n").unwrap();
        assert_eq!(setup_command(&command(Some("script/prepare_worktree")), &dir).unwrap().level, Level::Ok);
        assert_eq!(setup_command(&command(Some("make prepare")), &dir).unwrap().level, Level::Ok);
        let machine_wide = Setting { value: Some("script/elsewhere".to_string()), layer: config::Layer::Machine };
        let elsewhere = setup_command(&machine_wide, &dir).unwrap();
        assert!(elsewhere.level == Level::Ok && elsewhere.fact.contains("default setup runs here"), "{elsewhere:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn backlog_sources_are_checked_per_source() {
        let nowhere = Path::new("/nonexistent/kebbie/backlog.md");
        let none = backlog(&[], || unreachable!(), nowhere);
        assert_eq!((none.len(), none[0].level), (1, Level::Ok));
        let missing = backlog(&[Source::Vault, Source::Linear], || Err("No vault index at /x".to_string()), nowhere);
        assert_eq!((missing[0].level, missing[0].fact.as_str()), (Level::Warn, "No vault index at /x"));
        assert!(missing[1].level == Level::Ok && missing[1].fact.contains("Linear MCP"));
        assert_eq!(backlog(&[Source::Vault], || Ok("vault index /i.sqlite".to_string()), nowhere)[0].level, Level::Ok);
        let local = backlog(&[Source::Local], || unreachable!(), nowhere);
        assert!(local[0].level == Level::Ok && local[0].fact.contains("is created by the first item"), "{:?}", local[0]);
        let dir = std::env::temp_dir().join(format!("kebbie-doctor-local-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("backlog")).unwrap();
        std::fs::write(dir.join("backlog/one.md"), "+++\ntitle = \"One\"\n+++\n\nshort\n").unwrap();
        std::fs::write(dir.join("backlog/two.md"), "Two, by hand\n").unwrap();
        assert!(backlog(&[Source::Local], || unreachable!(), &dir.join("backlog"))[0].fact.ends_with("backlog: 2 items, 2 for the lead to write out"));
        std::fs::remove_dir_all(dir).unwrap();
        let config = Path::new("/home/u/.config/kebbie/config.toml");
        let mixed = mixed_sources(&[Source::Local, Source::Vault, Source::Linear], Path::new("/home/u/.config/kebbie/backlog"), config).unwrap();
        assert!(mixed.level == Level::Warn && mixed.fact.contains("new items go to the vault, and /home/u/.config/kebbie/backlog is no longer read"), "{mixed:?}");
        assert_eq!(mixed.fix, ["set sources = [\"vault\", \"linear\"] under [board.backlog] in /home/u/.config/kebbie/config.toml, after moving what you still want from that folder into the vault"]);
        assert!(mixed_sources(&[Source::Vault, Source::Linear], nowhere, config).is_none());
    }

    #[test]
    fn a_backlog_template_file_that_does_not_read_is_a_warning() {
        let dir = crate::sh::test_dir("doctor-template");
        assert_eq!(item_template(None), None, "kebbie's own when none is named");
        std::fs::write(dir.join("item.toml"), "[[sections]]\nkey = \"goal\"\nheading = \"Goal\"\n").unwrap();
        assert_eq!(item_template(Some(&dir.join("item.toml"))).unwrap().level, Level::Ok);
        std::fs::write(dir.join("broken.toml"), "sections = 3").unwrap();
        let broken = item_template(Some(&dir.join("broken.toml"))).unwrap();
        assert!(broken.level == Level::Warn && broken.fact.contains("broken.toml") && broken.fact.ends_with("local items follow kebbie's own template until it reads"), "{broken:?}");
        assert_eq!(item_template(Some(&dir.join("missing.toml"))).unwrap().level, Level::Warn);
    }

    #[test]
    fn an_installed_vault_plugin_or_linear_mcp_that_is_not_a_source_is_suggested_with_its_config_line() {
        let installed = json!({ "plugins": { "vault@gronare": [{ "installPath": "/plugins/vault/1.0" }], "kebbie@kebbie": [{ "installPath": "/plugins/kebbie" }] } });
        let files = |file: &Path| (file == Path::new("/plugins/vault/1.0/.mcp.json")).then(|| r#"{"mcpServers":{"vault":{"command":"uvx","args":["notes-vault-mcp"]}}}"#.to_string());
        assert_eq!(vault_plugin(Some(&installed), files).as_deref(), Some("vault@gronare"));
        assert_eq!(vault_plugin(Some(&installed), |_| None), None, "a vault plugin of some other server is not this one");
        let other = |file: &Path| (file == Path::new("/plugins/vault/1.0/.mcp.json")).then(|| r#"{"mcpServers":{"notes":{"command":"uvx","args":["mindin"]}}}"#.to_string());
        assert_eq!(vault_plugin(Some(&installed), other), None, "the server is found by its name in .mcp.json, not by the package it runs");
        assert_eq!(vault_plugin(None, files), None);
        assert_eq!(linear_mcp(Some(&json!({ "plugins": { "linear@claude-plugins": [] } })), None).as_deref(), Some("the plugin linear@claude-plugins"));
        assert_eq!(linear_mcp(None, Some(&json!({ "mcpServers": { "Linear": {} } }))).as_deref(), Some("the MCP server Linear"));
        assert_eq!(linear_mcp(None, Some(&json!({ "projects": { "/src/app": { "mcpServers": { "linear-server": {} } } } }))).as_deref(), Some("the MCP server linear-server"));
        assert_eq!(linear_mcp(Some(&installed), Some(&json!({ "mcpServers": { "github": {} } }))), None);
        let config = Path::new("/home/u/.config/kebbie/config.toml");
        let found = unused_sources(&[Source::Local], Some("vault@gronare".to_string()), Some("the MCP server Linear".to_string()), config);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].name.as_str(), found[0].level), ("backlog vault", Level::Ok));
        assert_eq!(found[0].fix, ["to use it, set sources = [\"vault\"] under [board.backlog] in /home/u/.config/kebbie/config.toml"], "the vault takes the local folder's place");
        assert_eq!(found[1].fix, ["to use it, set sources = [\"local\", \"linear\"] under [board.backlog] in /home/u/.config/kebbie/config.toml"]);
        assert!(unused_sources(&[Source::Vault, Source::Linear], Some("vault@gronare".to_string()), Some("x".to_string()), config).is_empty(), "an enabled source is not suggested");
        assert!(unused_sources(&[Source::Local], None, None, config).is_empty());
    }

    #[test]
    fn an_unset_machine_name_is_a_warning_with_where_to_set_it() {
        let unset = machine_name(&config::Setting { value: "mbp-7".to_string(), layer: config::Layer::Default }, Path::new("/home/u/.config/kebbie/config.toml"));
        assert_eq!(unset.level, Level::Warn);
        assert!(unset.fact.contains("machine.name is unset") && unset.fact.contains("\"mbp-7\"") && unset.fix[0].contains("[machine] name = ") && unset.fix[0].contains("/home/u/.config/kebbie/config.toml"), "{unset:?}");
        let set = machine_name(&config::Setting { value: "devbox".to_string(), layer: config::Layer::Machine }, Path::new("/c.toml"));
        assert_eq!((set.level, set.fact.as_str()), (Level::Ok, "goes by \"devbox\" on your phone and in vault claims"));
        assert!(unset.fix[1].contains("/kebbie:configure"), "{unset:?}");
    }

    fn vault_probe(configured: bool, tasks: bool, writable: Option<bool>) -> crate::vault_cli::Probe {
        let capabilities = if tasks { vec!["tasks".to_string(), "log".to_string(), "claims".to_string()] } else { Vec::new() };
        crate::vault_cli::Probe { configured, writable, version: "0.5.1".to_string(), backend: "s3".to_string(), name: "notes-vault-mcp".to_string(), capabilities, ..crate::vault_cli::Probe::default() }
    }

    fn words(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn the_vault_command_is_optional_and_an_old_one_reads_as_read_only() {
        use crate::vault_cli::{Origin, Plugin};
        let missing = vault(&Plugin::Missing, None, None);
        assert_eq!(missing.level, Level::Ok, "a user without a vault command sees no error");
        assert!(missing.fact.contains("(optional)") && missing.fix[0].starts_with("kebbie setup"), "{missing:?}");
        let enabled = Plugin::Enabled { id: "vault@gronare".to_string(), version: "0.2.15".to_string(), root: "/x".into() };
        let bridged = (words("uvx notes-vault-mcp==0.5.1"), Origin::PluginHooks);
        let probe = |configured: bool, tasks: bool, writable: Option<bool>| Some(Ok(vault_probe(configured, tasks, writable)));
        let old = vault(&enabled, Some(&bridged), probe(true, false, Some(true)));
        assert_eq!(old.level, Level::Warn);
        assert!(old.fact.contains("read-only") && old.fix[0] == "claude plugin update vault@gronare", "{old:?}");
        assert!(vault(&enabled, Some(&bridged), probe(false, true, None)).fact.contains("no vault is configured"));
        assert!(vault(&enabled, Some(&bridged), probe(true, true, Some(false))).fact.contains("not writable"));
        let fine = vault(&enabled, Some(&bridged), probe(true, true, Some(true)));
        assert_eq!(fine.level, Level::Ok);
        assert_eq!(fine.fix, ["set [vault] command = \"uvx notes-vault-mcp==0.5.1\" in the machine config, so kebbie reaches the vault without the Claude plugin", "kebbie doctor --fix writes it after your yes"], "the plugin bridge suggests kebbie's own line");
        assert_eq!(vault(&Plugin::Disabled { id: "vault@gronare".to_string(), version: "0.2.15".to_string() }, None, None).level, Level::Ok);
        assert_eq!(vault(&enabled, Some(&bridged), Some(Err("uvx: not found".to_string()))).level, Level::Warn);
        let stood_in = vault(&Plugin::Missing, Some(&(vec!["/tmp/fake-vault".to_string()], Origin::Env)), probe(true, true, Some(true)));
        assert_eq!(stood_in.level, Level::Ok, "KEBBIE_VAULT_BIN stands in for the command, as detect() lets it: {stood_in:?}");
        assert!(stood_in.fact.contains("KEBBIE_VAULT_BIN=/tmp/fake-vault") && stood_in.fact.contains("each task gets a note") && stood_in.fix.is_empty(), "{stood_in:?}");
        assert!(vault(&Plugin::Missing, Some(&(vec!["/tmp/fake-vault".to_string()], Origin::Env)), Some(Err("no such file".to_string()))).fact.contains("failed (no such file)"));
        let unclaimed = vault(&enabled, Some(&bridged), Some(Ok(crate::vault_cli::Probe { capabilities: vec!["tasks".to_string(), "log".to_string()], version: "0.7.0".to_string(), ..vault_probe(true, true, Some(true)) })));
        assert_eq!(unclaimed.level, Level::Warn, "a server without claims still keeps notes, and says once what it lacks");
        assert!(unclaimed.fact.contains("too old for cross-machine claims") && unclaimed.fact.contains("notes-vault-mcp 0.7.0") && unclaimed.fix[0] == "claude plugin update vault@gronare", "{unclaimed:?}");
    }

    #[test]
    fn a_configured_vault_command_is_checked_by_its_contract_without_the_claude_plugin() {
        use crate::vault_cli::{Origin, Plugin, Probe};
        let configured = (words("uvx mindin"), Origin::Config);
        let about = Probe { name: "mindin".to_string(), version: "0.14.0".to_string(), contract: Some(1), capabilities: words("tasks claims log snapshot"), ..vault_probe(true, true, None) };
        let fine = vault(&Plugin::Missing, Some(&configured), Some(Ok(about.clone())));
        assert_eq!(fine.level, Level::Ok, "{fine:?}");
        assert_eq!(fine.fact, "[vault] command \"uvx mindin\": mindin 0.14.0, s3, states tasks, claims, log, snapshot: each task gets a note claimed for this machine, edits wait for its Problem, pushes are logged, done closes it");
        assert!(fine.fix.is_empty());
        let bridge = vault(&Plugin::Missing, Some(&configured), Some(Ok(vault_probe(true, true, None))));
        assert!(bridge.fact.contains("read from its status and help as it answers no about --json yet"), "{bridge:?}");
        let lacking = vault(&Plugin::Missing, Some(&configured), Some(Ok(Probe { capabilities: words("log"), ..about.clone() })));
        assert_eq!(lacking.level, Level::Warn);
        assert!(lacking.fact.contains("states no tasks capability") && lacking.fix == ["point [vault] command at a newer mindin"], "{lacking:?}");
        let broken = vault(&Plugin::Missing, Some(&configured), Some(Err("uvx: not found".to_string())));
        assert!(broken.fact.starts_with("[vault] command \"uvx mindin\" failed (uvx: not found)") && broken.fix[0] == "check that it runs: uvx mindin about --json", "{broken:?}");
        let newer = vault(&Plugin::Missing, Some(&configured), Some(Ok(Probe { contract: Some(2), ..about.clone() })));
        assert!(newer.level == Level::Warn && newer.fact.contains("answers contract 2") && newer.fix == ["kebbie update"], "{newer:?}");
        let old_name = (words("uvx notes-vault-mcp==0.14.0"), Origin::Config);
        let renamed = vault(&Plugin::Missing, Some(&old_name), Some(Ok(Probe { former_names: vec!["notes-vault-mcp".to_string()], ..about })));
        assert_eq!(renamed.level, Level::Warn);
        assert!(renamed.fact.contains("the package's new name") && renamed.fix == ["set [vault] command = \"uvx mindin==0.14.0\" in the machine config", "kebbie doctor --fix writes it after your yes"], "{renamed:?}");
    }

    #[test]
    fn doctor_fix_writes_the_suggested_vault_command_only_on_a_yes() {
        let dir = scratch("vault-fix");
        let config = dir.join("config.toml");
        std::fs::write(&config, "# mine\n[vault]\ncommand = \"uvx notes-vault-mcp==0.14.0\"\n").unwrap();
        assert_eq!(vault_fix(None, &config, None), None);
        let unasked = vault_fix(Some("uvx mindin==0.14.0".to_string()), &config, None).unwrap();
        assert!(unasked.contains("Run kebbie doctor --fix in a terminal"), "{unasked}");
        let mut no = |_: &str| false;
        assert_eq!(vault_fix(Some("uvx mindin==0.14.0".to_string()), &config, Some(&mut no)).as_deref(), Some("[vault] command stays as it is."));
        assert!(std::fs::read_to_string(&config).unwrap().contains("notes-vault-mcp"), "nothing is written without a yes");
        let mut asked = Vec::new();
        let mut yes = |question: &str| { asked.push(question.to_string()); true };
        vault_fix(Some("uvx mindin==0.14.0".to_string()), &config, Some(&mut yes)).unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "# mine\n[vault]\ncommand = \"uvx mindin==0.14.0\"\n");
        assert_eq!(asked, [format!("Set [vault] command = \"uvx mindin==0.14.0\" in {}?", config.display())]);
        assert_eq!(suggested_command(&words("uvx notes-vault-mcp"), crate::vault_cli::Origin::Env, None), None, "KEBBIE_VAULT_BIN is never written over");
    }

    #[test]
    fn doctor_suggests_docs_required_for_a_public_github_repo_and_never_fails_without_gh() {
        let dir = scratch("docs");
        let (repo, file) = (dir.join("app"), dir.join("repos/app.toml"));
        std::fs::create_dir_all(&repo).unwrap();
        let gh = dir.join("gh");
        crate::executable::write(&gh, "#!/bin/sh\n[ \"$*\" = \"repo view --json visibility --jq .visibility\" ] && echo PUBLIC\n").unwrap();
        assert_eq!(visibility(Some(&gh), &repo, GH_TIMEOUT).as_deref(), Some("public"));
        assert_eq!(visibility(None, &repo, GH_TIMEOUT), None, "without gh there is no answer");
        let slow = dir.join("gh-slow");
        crate::executable::write(&slow, "#!/bin/sh\nsleep 30\necho PUBLIC\n").unwrap();
        let started = std::time::Instant::now();
        assert_eq!(visibility(Some(&slow), &repo, std::time::Duration::from_millis(200)), None, "a gh that hangs is no answer");
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "and doctor does not wait for it");
        if std::env::var(crate::ci::GH_VAR).is_err() {
            assert_eq!(gh_on(std::slice::from_ref(&dir)), Some(gh.clone()), "gh is found on the doctor's PATH");
        }
        let failing = dir.join("gh-failing");
        crate::executable::write(&failing, "#!/bin/sh\necho 'no git remotes found' >&2\nexit 1\n").unwrap();
        assert_eq!(visibility(Some(&failing), &repo, GH_TIMEOUT), None, "a repo gh cannot read is no answer");
        let off = Setting { value: false, layer: config::Layer::Default };
        let suggested = docs(&off, || visibility(Some(&gh), &repo, GH_TIMEOUT), &file).unwrap();
        assert_eq!(suggested.level, Level::Ok, "a suggestion, not a warning");
        assert!(suggested.fact.contains("public on GitHub") && suggested.fix == [format!("to turn it on, add [docs] required = true to {}", file.display())], "{suggested:?}");
        assert_eq!(docs(&off, || Some("private".to_string()), &file), None);
        assert_eq!(docs(&off, || None, &file), None);
        let on = Setting { value: true, layer: config::Layer::Repo };
        let asked = std::cell::Cell::new(false);
        let check = docs(&on, || { asked.set(true); None }, &file).unwrap();
        assert!(check.fact.contains("docs.required is on (repo config)") && check.fix.is_empty(), "{check:?}");
        assert!(!asked.get(), "gh is not asked when there is nothing to suggest");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn doctor_suggests_the_full_suite_before_push_for_a_repo_whose_whole_run_is_short() {
        let file = PathBuf::from("/c/repos/app.toml");
        let run = |command: &str, seconds: u64| crate::evidence::Run { command: command.to_string(), result: crate::evidence::Outcome::Passed, duration_ms: Some(seconds * 1000), ..crate::evidence::Run::default() };
        let listed = crate::evidence::commands(&[]);
        let short = short_full_run(&[run("uv run pytest -q", 40), run("uv run pytest -q", 60), run("uv run pytest tests/test_a.py", 2), run("uv run pytest", 50)], &listed);
        assert_eq!(short, Some(("uv run pytest".to_string(), 50_000)), "the whole suite, its median, never a filtered run");
        assert_eq!(short_full_run(&[run("cargo test", 900)], &listed), None, "a slow suite is not suggested");
        let related = Setting { value: config::BeforePush::Related, layer: config::Layer::Default };
        let suggested = tests(&related, None, || short.clone(), &file).unwrap();
        assert_eq!(suggested.level, Level::Ok, "a suggestion, not a warning");
        assert_eq!(suggested.fix, [format!("to require it before every push, add [tests] before_push = \"full\" and full_command = \"uv run pytest\" to {}", file.display())]);
        assert_eq!(tests(&related, None, || None, &file), None, "nothing measured, nothing said");
        let full = Setting { value: config::BeforePush::Full, layer: config::Layer::Repo };
        let on = tests(&full, Some("cargo test --locked"), || panic!("nothing to suggest"), &file).unwrap();
        assert!(on.fact.contains("tests.before_push is full (repo config)") && on.fact.contains("`cargo test --locked`") && on.fix.is_empty(), "{on:?}");
        let unnamed = tests(&full, None, || None, &file).unwrap();
        assert_eq!(unnamed.level, Level::Warn, "{unnamed:?}");
        assert!(unnamed.fact.contains("names no tests.full_command"), "{unnamed:?}");
    }

    #[test]
    fn doctor_says_whether_comments_on_pushed_commits_are_read_and_why_not() {
        let on = Setting { value: true, layer: config::Layer::Default };
        let watched = comments(&on, 7, || Ok("acme/app".to_string()));
        assert_eq!((watched.level, watched.fact.as_str()), (Level::Ok, "on: kebbie reads colleagues' comments on the commits it pushes to acme/app, for 7 days after each push"));
        let unread = comments(&on, 7, || Err("gh is not logged in to github.com (gh auth login)".to_string()));
        assert!(unread.level == Level::Warn && unread.fact == "on, but comments here are not read: gh is not logged in to github.com (gh auth login)", "the same reason CI watching gives: {unread:?}");
        let off = comments(&Setting { value: false, layer: config::Layer::Repo }, 7, || panic!("gh is not asked when it is off"));
        assert_eq!(off.fact, "comments.watch is off (repo config): comments on pushed commits are not read");
    }

    #[test]
    fn doctor_says_whether_the_keeper_runs() {
        let running = crate::keeper::Holder { pid: Some(4242), exe: None, started: Some(100) };
        let check = keeper(true, Some(&running), 100 + 3 * 3600);
        assert_eq!((check.level, check.fact.as_str()), (Level::Ok, "running, pid 4242, up 3h; kebbie keeper --status shows its routines"));
        let check = keeper(true, None, 0);
        assert_eq!((check.level, check.fix.as_slice()), (Level::Warn, ["kebbie keeper --start (herdr's startup and the board start it too)".to_string()].as_slice()));
        assert_eq!(keeper(false, None, 0).level, Level::Ok);
        assert_eq!(keeper(false, Some(&running), 0).fix, ["kebbie keeper --stop"]);
    }

    #[test]
    fn setup_fixes_only_what_needs_no_choice_of_the_users() {
        let list = |enabled: bool| format!(r#"{{"result":{{"plugins":[{{"plugin_id":"kebbie","enabled":{enabled},"version":"0.28.0","plugin_root":"/x"}}]}}}}"#);
        let (on, off) = (list(true), list(false));
        let found = |settings: &Value, keeper: (bool, bool), herdr: Option<&str>| repairs(&Found { claude_settings: settings, keeper_on: keeper.0, keeper_running: keeper.1, herdr_list: herdr });
        let none = serde_json::json!({});
        assert_eq!(found(&none, (true, false), Some(&off)), [Repair::StatusLine, Repair::KeeperStart, Repair::HerdrPlugin]);
        let own = serde_json::json!({ "statusLine": { "type": "command", "command": "my-line" } });
        assert!(found(&own, (true, true), Some(&on)).is_empty(), "the user's own statusLine is theirs to change, as kebbie doctor says");
        assert_eq!(found(&own, (false, true), None), [Repair::KeeperStop]);
        assert!(STATUS_LINE.contains(STATUS_LINE_COMMAND));
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
    fn an_ok_check_carries_a_brief_for_its_group_line() {
        let herdr = tools("0.9.0").into_iter().find(|tool| tool.name == "herdr").unwrap();
        assert_eq!(tool(&herdr, Some(Path::new("/bin/herdr")), Some("herdr 0.9.5")).brief, "0.9.5");
        let git = tools("0.9.0").into_iter().find(|tool| tool.name == "git").unwrap();
        assert_eq!(tool(&git, Some(Path::new("/usr/bin/git")), None).brief, "git");
        assert_eq!(login("claude login", Login::In, "", "").brief, "signed in");
        assert_eq!(keeper(false, None, 0).brief, "keeper off");
        assert_eq!(task_hooks(&[]).brief, "task hooks");
        assert!(task_hooks(&["a@b".to_string()]).brief.is_empty(), "a check that needs you is shown by its text");
    }

    #[test]
    fn doctor_says_whether_notify_is_set_and_runs_nothing() {
        let marker = std::env::temp_dir().join(format!("kebbie-doctor-notify-{}", std::process::id()));
        let command = format!("touch {}", marker.display());
        let set = notify(&Setting { value: Some(command.clone()), layer: config::Layer::Repo });
        assert_eq!(set.level, Level::Ok);
        assert!(set.fact.contains(&format!("{command:?} (repo config)")), "{}", set.fact);
        assert!(!marker.exists(), "doctor never sends a notification");
        let unset = notify(&Setting { value: None, layer: config::Layer::Default });
        assert_eq!(unset.level, Level::Ok);
        assert!(unset.fact.contains("not set"), "{}", unset.fact);
    }

    #[test]
    fn the_doctor_runs_against_an_empty_environment_without_touching_the_users_home() {
        let dir = scratch("env");
        let env = Env {
            path: vec![dir.join("bin")], home: dir.join("home"), state_home: dir.join("state"), kebbie_state: dir.join("state/kebbie"), own_state: true, data_home: None,
            claude_dir: dir.join("claude"), codex_dir: dir.join("codex"), profiles_dir: dir.join("profiles"), config: dir.join("config.toml"), repos_dir: dir.join("repos"), repo_config: None, repo: None, uid: 0,
        };
        let checks = checks(&env);
        assert!(failed(&checks), "no tools on PATH is a failure");
        let names: Vec<&str> = checks.iter().map(|check| check.name.as_str()).collect();
        for needed in ["git", "herdr", "claude", "jq", "herdr plugin", "claude plugin", "status line", "current link", "kebbie on PATH", "updates", "state dir", "config", "setup skill", "notify", "vault", "backlog local"] {
            assert!(names.contains(&needed), "{needed} is not checked: {names:?}");
        }
        assert!(!dir.join("home").exists() && !dir.join("claude").exists() && !dir.join("codex").exists(), "doctor reads, it never creates");
        assert!(names.contains(&"codex"), "{names:?}");
        let rendered = render(&checks);
        assert!(rendered.lines().last().unwrap().ends_with(" ok"), "{rendered}");
        assert!(rendered.contains("fail  git"), "{rendered}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn doctor_shows_codex_as_optional_until_a_profile_runs_it_and_then_what_it_still_needs() {
        use crate::agent::codex::{Availability, Installed};
        let home = Path::new("/u/.codex");
        let absent = Availability { binary: None, version: None, plugin: Installed::Missing };
        assert_eq!(codex(&absent, None, false, home, None).iter().map(|check| (check.level, check.name.as_str())).collect::<Vec<_>>(), [(Level::Ok, "codex")], "no line nags a Claude-only machine");
        let needed = codex(&absent, None, true, home, None);
        assert_eq!((needed.len(), needed[0].level), (1, Level::Fail));
        assert!(needed[0].fix.iter().any(|fix| fix.contains("https://chatgpt.com/codex/install.sh")));
        let ready = Availability { binary: Some(PathBuf::from("/u/.local/bin/codex")), version: Some("codex-cli 0.163.0".to_string()), plugin: Installed::Current };
        let lines = codex(&ready, Some(&"model = \"gpt-5.5\"\n".parse().unwrap()), true, home, None);
        assert_eq!(lines.iter().map(|check| (check.level, check.name.as_str())).collect::<Vec<_>>(), [(Level::Ok, "codex"), (Level::Ok, "codex plugin"), (Level::Ok, "codex hooks"), (Level::Ok, "codex model")]);
        assert_eq!(lines[3].fact, "gpt-5.5, from /u/.codex/config.toml, for a Codex profile without model");
        assert_eq!(lines[0].fact, "0.163.0 at /u/.local/bin/codex");
        assert!(lines[1].fact.contains("/u/.codex/plugins/cache/kebbie-tasks/kebbie/local") && lines[2].fact.contains(&format!("trusts its {} hooks and the plugin's files by hash", crate::agent::codex::hook_count())));
        let old = Availability { version: Some("codex-cli 0.120.4".to_string()), plugin: Installed::Missing, ..ready };
        let off: toml::Table = "[features]\nhooks = false\n".parse().unwrap();
        let lines = codex(&old, Some(&off), true, home, None);
        assert!(lines.iter().all(|check| check.level == Level::Fail), "{lines:?}");
        assert!(lines[0].fact.contains("older than 0.163.0") && lines[1].fix == ["kebbie setup codex"] && lines[2].fact.contains("[features] hooks = false"), "{lines:?}");
        assert_eq!(codex(&old, Some(&off), false, home, None)[2].level, Level::Warn, "a gap is a warning until a profile runs Codex");
        let current = Availability { binary: Some(PathBuf::from("/u/.local/bin/codex")), version: Some("codex-cli 0.163.0".to_string()), plugin: Installed::Current };
        let signed = |login: Login, needed: bool| codex(&current, None, needed, home, Some(login)).pop().unwrap();
        assert_eq!((signed(Login::In, true).level, signed(Login::In, true).fact.as_str()), (Level::Ok, "logged in"));
        let out = signed(Login::Out, false);
        assert_eq!((out.name.as_str(), out.level, out.fix), ("codex login", Level::Warn, vec!["codex login".to_string()]));
        assert_eq!(signed(Login::Out, true).level, Level::Fail, "a profile that runs Codex cannot start logged out");
        let unknown = signed(Login::Unknown, true);
        assert!(unknown.level == Level::Skip && unknown.fact.starts_with("login not checked"), "no answer is never logged out, nor ok: {unknown:?}");
        assert!(render(std::slice::from_ref(&unknown)).ends_with("0 failed, 0 warnings, 0 ok, 1 not checked"), "{}", render(&[unknown]));
        assert_eq!(login("claude login", Login::Out, "claude auth login", "the lead cannot start").fix, ["claude auth login"]);
    }

    #[test]
    fn doctor_names_a_copy_codex_installed_in_place_of_kebbies_plugin_and_one_left_under_kebbie_at_kebbie() {
        use crate::agent::codex::{Availability, Installed};
        let home = crate::sh::test_dir("doctor-codex-copies");
        let copy = home.join("plugins/cache/kebbie-tasks/kebbie/0.30.0");
        let replaced = Availability { binary: Some(PathBuf::from("/u/.local/bin/codex")), version: Some("codex-cli 0.163.0".to_string()), plugin: Installed::Replaced(vec![copy.clone()]) };
        let plugin = codex(&replaced, None, true, &home, None).remove(1);
        assert_eq!((plugin.level, plugin.fix), (Level::Fail, vec!["kebbie setup codex".to_string()]));
        assert!(plugin.fact.contains("Codex replaced kebbie's plugin with") && plugin.fact.contains("0.30.0"), "{}", plugin.fact);
        let old = home.join("plugins/cache/kebbie/kebbie/0.30.0");
        std::fs::create_dir_all(&old).unwrap();
        let current = Availability { plugin: Installed::Current, ..replaced };
        let named = codex(&current, None, true, &home, None).into_iter().find(|check| check.name == "codex old plugin").unwrap();
        assert_eq!(named.level, Level::Warn);
        assert!(named.fact.contains("plugins/cache/kebbie/kebbie/0.30.0") && named.fix[0].starts_with("rm -rf "), "{named:?}");
        let moved = Availability { plugin: Installed::UnderOldId(home.join("plugins/cache/kebbie/kebbie/local")), ..current };
        let plugin = codex(&moved, None, true, &home, None).remove(1);
        assert!(plugin.level == Level::Fail && plugin.fact.contains("under kebbie@kebbie") && plugin.fix == ["kebbie setup codex"], "an opted-in plugin under kebbie@kebbie is named, not called not installed: {plugin:?}");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn version_parsing_takes_the_first_dotted_number_and_stops_at_a_non_digit() {
        assert_eq!(version("git version 2.44.0 (Apple Git-154)"), Some(vec![2, 44, 0]));
        assert_eq!(version("fzf 0.71.0 (brew)"), Some(vec![0, 71, 0]));
        assert_eq!(version("tig version 2.5.8"), Some(vec![2, 5, 8]));
        assert_eq!(version("1.2.3-rc1"), Some(vec![1, 2, 3]));
        assert_eq!(version("v1.2"), Some(vec![1, 2]));
        assert_eq!(version("build 2024 of 1.0"), Some(vec![1, 0]), "a bare number without a dot is skipped");
        assert_eq!(version(""), None);
        assert!(!at_least(&[], &[0, 9, 0]) && at_least(&[1], &[0, 9, 9]));
    }

    #[test]
    fn doctor_says_plainly_when_the_sandbox_is_off_and_fails_a_machine_that_cannot_sandbox() {
        let set = |value: bool, layer: config::Layer| Setting { value, layer };
        let off = sandbox(&set(false, config::Layer::Machine), &[]);
        assert_eq!(off.level, Level::Warn);
        assert!(off.fact.starts_with("off (task.sandbox = false): a task's shell commands run unsandboxed"), "{}", off.fact);
        assert_eq!(sandbox(&set(true, config::Layer::Default), &[]).level, Level::Ok);
        let lacking = sandbox(&set(true, config::Layer::Default), &["socat".to_string(), "the seccomp filter".to_string()]);
        assert_eq!(lacking.level, Level::Fail);
        assert!(lacking.fact.contains("kebbie starts no task: socat, the seccomp filter missing"), "{}", lacking.fact);
        assert!(lacking.fix.iter().any(|fix| fix.contains("npm install -g @anthropic-ai/sandbox-runtime")) && lacking.fix.last().unwrap().contains("task.sandbox = false"), "{:?}", lacking.fix);
        assert!(include_str!("doctor.rs").contains("checks.push(sandbox(&settings.sandbox, &crate::sandbox::missing_here()));"), "doctor runs the check");
    }

    #[test]
    fn a_task_profile_that_names_no_profile_is_a_warning_with_the_list_and_the_settings_page_as_fix() {
        let dir = crate::sh::test_dir("doctor-task-profile");
        let (profiles, repos) = (dir.join("profiles"), dir.join("repos"));
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::create_dir_all(&repos).unwrap();
        std::fs::write(profiles.join("coder.toml"), "agent = \"codex\"\n").unwrap();
        std::fs::write(dir.join("config.toml"), "[task]\nprofile = \"coder\"\n").unwrap();
        std::fs::write(repos.join("app.toml"), "[task]\nprofile = \"designer\"\n").unwrap();
        std::fs::write(repos.join("api.toml"), "[ci]\nwatch = true\n").unwrap();
        std::fs::write(repos.join("web.toml"), "[task]\nprofile = \"none\"\n").unwrap();
        let machine = config::check_machine(&dir.join("config.toml")).unwrap().0;
        let named = named_profiles(Some(&machine), &dir.join("config.toml"), &repos);
        assert_eq!(named, [(dir.join("config.toml").display().to_string(), "coder".to_string()), (repos.join("app.toml").display().to_string(), "designer".to_string())]);
        let check = task_profiles(&profiles, &named).unwrap();
        assert_eq!(check.level, Level::Warn);
        assert_eq!(check.fact, format!("task.profile = \"designer\" in {} names no profile; the profiles are task, ops, coder", repos.join("app.toml").display()));
        assert_eq!(check.fix, ["pick it on the Settings page (,) under Tasks"]);
        assert!(task_profiles(&profiles, &[("config.toml".to_string(), "reviewer".to_string())]).unwrap().level == Level::Warn, "the reviewer is never a task's profile");
        assert_eq!(task_profiles(&profiles, &named[..1]).unwrap().level, Level::Ok);
        assert_eq!(task_profiles(&profiles, &[]), None, "nothing to say when no file sets it");
        let coder = crate::profiles::list_in(&profiles).0.into_iter().find(|profile| profile.name == "coder").unwrap();
        let unchosen = unchosen_profiles(&[crate::profiles::built_in(crate::profiles::TASK).unwrap(), coder.clone()], &[crate::agent::Agent::Claude]);
        assert_eq!(unchosen.len(), 1);
        assert_eq!((unchosen[0].level, unchosen[0].fact.as_str(), unchosen[0].fix[0].as_str()), (Level::Warn, "profile coder: Codex is not set up: kebbie setup codex", "kebbie setup codex"));
        assert!(unchosen_profiles(&[coder], &crate::agent::Agent::ALL).is_empty(), "a machine that chose Codex has nothing to say");
        let claude = |name: &str, model: &str| crate::profiles::Profile { name: name.to_string(), model: Some(model.to_string()), file: Some(PathBuf::from(format!("/p/{name}.toml"))), ..crate::profiles::built_in(crate::profiles::TASK).unwrap() };
        let pinned = claude_models(&[claude("designer", "claude-opus-4-5-20251101"), claude("writer", "sonnet"), claude("long", "opus[1m]"), crate::profiles::Profile { agent: crate::agent::Agent::Codex, ..claude("coder", "gpt-6.1-sol") }]);
        assert_eq!(pinned.len(), 1, "an alias, an alias with its context suffix and a Codex model are fine: {pinned:?}");
        assert_eq!((pinned[0].level, pinned[0].fact.as_str(), pinned[0].fix[0].as_str()), (Level::Warn, "profile designer pins claude-opus-4-5-20251101, a full model id, so its tasks stay on that model; an alias (opus, sonnet, haiku, fable) follows Claude Code to each new one", "pick one with m on the Settings page"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn doctor_warns_of_a_profile_kubeconfig_that_is_missing_readable_by_others_or_the_users_own_default_login() {
        use std::os::unix::fs::PermissionsExt;
        let home = crate::sh::test_dir("doctor-kubeconfig");
        std::fs::create_dir_all(home.join(".kube")).unwrap();
        let file = |name: &str, mode: u32| {
            let path = home.join(".kube").join(name);
            std::fs::write(&path, "").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        };
        let (default, open, full) = (file("config", 0o600), file("shared.yaml", 0o644), file("kebbie-cluster-full.yaml", 0o600));
        let kube = |name: &str, path: &Path| crate::profiles::Profile { name: name.to_string(), sandbox: crate::sandbox::Policy { kubeconfig: Some(path.display().to_string()), ..Default::default() }, ..crate::profiles::built_in(crate::profiles::TASK).unwrap() };
        let checks = kubeconfigs(&[kube("admin", &default), kube("shared", &open), kube("gone", &home.join(".kube/gone.yaml")), kube("cluster-full", &full), crate::profiles::built_in(crate::profiles::TASK).unwrap()], &home);
        let found: Vec<(Level, &str, Option<&str>)> = checks.iter().map(|check| (check.level, check.fact.as_str(), check.fix.first().map(String::as_str))).collect();
        assert_eq!(found.len(), 4, "a profile without a kubeconfig says nothing: {found:?}");
        assert!(found[0].0 == Level::Warn && found[0].1.contains("profile admin: sandbox.kubeconfig") && found[0].1.contains("is your default kubeconfig, usually an admin login") && found[0].2.unwrap().contains("ServiceAccount"), "{found:?}");
        assert!(found[1].0 == Level::Warn && found[1].1.ends_with("is readable by others (mode 0644)") && found[1].2.unwrap().starts_with("chmod 600 "), "{found:?}");
        assert!(found[2].0 == Level::Warn && found[2].1.ends_with("is not there, so its tasks' kubectl and helm fail"), "{found:?}");
        assert!(found[3].0 == Level::Ok && found[3].1.ends_with("is there and yours alone (mode 0600)"), "a dedicated file the user chose reads ok, cluster-admin or not: {found:?}");
        std::fs::remove_dir_all(home).unwrap();
    }
}
