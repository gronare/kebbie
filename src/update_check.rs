use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::config::Channel;
use crate::sh::Result;

pub const FILE: &str = "update-check.json";
pub const EVERY: u64 = 6 * 60 * 60;
pub const PENDING_EVERY: u64 = 15 * 60;
pub const KELPIE: &str = "kelpie";
pub const VAULT: &str = "vault";
pub const PIN: &str = "notes-vault-mcp";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Component {
    pub name: String,
    pub installed: Option<String>,
    pub built: Option<String>,
    pub checkout: Option<String>,
    pub latest: Option<String>,
    pub unpublished: Option<String>,
    pub error: Option<String>,
}

impl Component {
    fn new(name: &str, installed: Option<String>) -> Self {
        Self { name: name.to_string(), installed, ..Self::default() }
    }

    fn ahead_of_installed(&self, version: Option<&String>) -> bool {
        matches!((&self.installed, version), (Some(installed), Some(version)) if newer(version, installed))
    }

    // A release checkout on a tag whose binary is not published yet has nothing to rebuild from.
    pub fn rebuild(&self) -> bool {
        let beyond_published = self.unpublished.is_some() && self.checkout.as_ref().is_some_and(|checkout| self.latest.as_ref().is_none_or(|latest| newer(checkout, latest)));
        self.ahead_of_installed(self.checkout.as_ref()) && !beyond_published
    }

    fn latest_ahead(&self) -> bool {
        self.ahead_of_installed(self.latest.as_ref()) && !(self.rebuild() && self.checkout.as_ref().zip(self.latest.as_ref()).is_some_and(|(checkout, latest)| !newer(latest, checkout)))
    }

    pub fn newer(&self) -> bool {
        self.rebuild() || self.ahead_of_installed(self.latest.as_ref())
    }

    pub fn target(&self) -> Option<&String> {
        if self.latest_ahead() { self.latest.as_ref() } else if self.rebuild() { self.checkout.as_ref() } else { None }
    }

    pub fn label(&self) -> &str {
        if self.name == VAULT { "vault plugin" } else { &self.name }
    }

    fn to_json(&self) -> Value {
        json!({ "name": self.name, "installed": self.installed, "latest": self.latest, "unpublished": self.unpublished, "error": self.error })
    }

    fn from_json(value: &Value) -> Option<Self> {
        let text = |key: &str| value[key].as_str().map(str::to_string);
        Some(Self { name: text("name")?, installed: text("installed"), latest: text("latest"), unpublished: text("unpublished"), error: text("error"), ..Self::default() })
    }
}

pub fn newer(latest: &str, installed: &str) -> bool {
    match (crate::doctor::version(latest), crate::doctor::version(installed)) {
        (Some(latest), Some(installed)) => {
            let pad = |parts: &[u64]| (0..3.max(parts.len())).map(|index| parts.get(index).copied().unwrap_or(0)).collect::<Vec<_>>();
            pad(&latest) > pad(&installed)
        }
        _ => false,
    }
}

pub struct Places {
    pub root: Option<PathBuf>,
    pub claude_dir: PathBuf,
    pub channel: Channel,
    pub binaries: Option<crate::release::Binaries>,
}

impl Places {
    pub fn new(root: Option<PathBuf>, claude_dir: PathBuf, channel: Channel) -> Self {
        Self { root, claude_dir, channel, binaries: None }
    }

    pub fn current() -> Self {
        Self::new(install_root(), crate::agent::claude::home().unwrap_or_default(), crate::config::channel().unwrap_or_default())
    }
}

pub fn install_root() -> Option<PathBuf> {
    let list = crate::sh::run(&crate::herdr::bin(), &["plugin", "list", "--json", "--plugin", "kelpie"], None).ok();
    let from_herdr = list.and_then(|list| serde_json::from_str::<Value>(&list).ok()).and_then(|list| {
        let plugins = list["result"]["plugins"].as_array()?.clone();
        plugins.iter().find(|plugin| plugin["plugin_id"] == "kelpie").and_then(|plugin| plugin["plugin_root"].as_str()).filter(|root| !root.is_empty()).map(PathBuf::from)
    });
    from_herdr.or_else(|| {
        let current = PathBuf::from(std::env::var("HOME").ok()?).join(".local/share/kelpie/current");
        current.exists().then_some(current)
    })
}

fn plugin_version(text: &str) -> Option<String> {
    serde_json::from_str::<Value>(text).ok()?["version"].as_str().map(str::to_string)
}

fn cargo_version(text: &str) -> Option<String> {
    text.parse::<toml::Table>().ok()?.get("package")?.get("version")?.as_str().map(str::to_string)
}

fn kelpie_version(read: impl Fn(&str) -> Option<String>) -> Option<String> {
    read(".claude-plugin/plugin.json").as_deref().and_then(plugin_version).or_else(|| read("Cargo.toml").as_deref().and_then(cargo_version))
}

pub fn pin(hooks: &str) -> Option<String> {
    let at = hooks.find("notes-vault-mcp==")? + "notes-vault-mcp==".len();
    let version: String = hooks[at..].chars().take_while(|character| character.is_ascii_alphanumeric() || *character == '.').collect();
    (!version.is_empty()).then_some(version)
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok())
}

pub fn parse_binary_version(text: &str) -> Option<(String, Option<String>)> {
    let rest = text.lines().next()?.trim().strip_prefix("kelpie ")?;
    let (version, built) = rest.split_once(' ').map_or((rest, None), |(version, built)| (version, Some(built)));
    crate::doctor::version(version)?;
    let sha = built.and_then(|built| built.trim_start_matches('(').split([' ', ')']).next()).filter(|sha| !sha.is_empty() && sha.chars().all(|character| character.is_ascii_hexdigit()));
    Some((version.to_string(), sha.map(str::to_string)))
}

pub fn binary_version(binary: &Path) -> Option<(String, Option<String>)> {
    let output = Command::new(binary).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().ok().filter(|output| output.status.success())?;
    parse_binary_version(&String::from_utf8_lossy(&output.stdout))
}

// What runs is the binary, so a checkout moved ahead without a rebuild shows as the binary's version with the checkout's beside it.
fn installed_kelpie(root: Option<&Path>) -> Component {
    let Some(root) = root else { return Component::new(KELPIE, None) };
    let (installed, built) = binary_version(&root.join("target/release/kelpie")).map_or((None, None), |(version, built)| (Some(version), built));
    Component { built, checkout: kelpie_version(|file| std::fs::read_to_string(root.join(file)).ok()), ..Component::new(KELPIE, installed) }
}

pub fn installed(places: &Places) -> Vec<Component> {
    let mut components = vec![installed_kelpie(places.root.as_deref())];
    if let Some((_, version, root)) = crate::vault_cli::installed(&places.claude_dir) {
        components.push(Component::new(VAULT, Some(version)));
        components.push(Component::new(PIN, pin(&crate::vault_cli::hooks_of(&root))));
    }
    components
}

// A fetch must never wait on a password or a passphrase, as the board runs it unseen.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir).stdin(Stdio::null()).env("GIT_TERMINAL_PROMPT", "0");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let output = command.output().map_err(|error| format!("git: {error}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("git {} failed: {}", args.join(" "), stderr.trim().lines().last().unwrap_or_default()))
}

fn fetched_default(dir: &Path) -> Result<String> {
    git(dir, &["fetch", "--quiet", "origin"])?;
    if let Ok(head) = git(dir, &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"]) {
        return Ok(head);
    }
    ["refs/remotes/origin/main", "refs/remotes/origin/master"].into_iter().find(|branch| git(dir, &["rev-parse", "--verify", "--quiet", branch]).is_ok())
        .map(str::to_string).ok_or_else(|| format!("{} has no origin default branch", dir.display()))
}

fn show(dir: &Path, reference: &str, file: &str) -> Option<String> {
    git(dir, &["show", &format!("{reference}:{file}")]).ok()
}

// The release channel's latest is a tag whose binary is published, so the notice names it and nothing pushed after it.
fn latest_kelpie(root: &Path, channel: Channel, binaries: Option<&crate::release::Binaries>) -> Result<(Option<String>, Option<String>)> {
    if channel == Channel::Release {
        let latest = crate::release::latest_published(root, &binaries.cloned().unwrap_or_else(|| crate::release::Binaries::of(root)))?;
        return Ok((latest.tag, latest.unpublished));
    }
    let head = fetched_default(root)?;
    kelpie_version(|file| show(root, &head, file)).map(|version| (Some(version), None)).ok_or_else(|| format!("no version on {head}"))
}

fn marketplace_dir(claude_dir: &Path, name: &str) -> PathBuf {
    read_json(&claude_dir.join("plugins/known_marketplaces.json"))
        .and_then(|known| known[name]["installLocation"].as_str().map(PathBuf::from))
        .unwrap_or_else(|| claude_dir.join("plugins/marketplaces").join(name))
}

fn latest_vault(claude_dir: &Path, id: &str) -> Result<(String, Option<String>)> {
    let (plugin, marketplace) = id.split_once('@').ok_or_else(|| format!("{id} names no marketplace"))?;
    let dir = marketplace_dir(claude_dir, marketplace);
    let head = fetched_default(&dir)?;
    let listing: Value = show(&dir, &head, ".claude-plugin/marketplace.json").and_then(|text| serde_json::from_str(&text).ok()).ok_or("the marketplace lists no plugins")?;
    let source = listing["plugins"].as_array().into_iter().flatten().find(|entry| entry["name"] == plugin)
        .and_then(|entry| entry["source"].as_str()).ok_or_else(|| format!("the marketplace {marketplace} has no {plugin} in its own repo"))?;
    let path = source.trim_start_matches("./").trim_end_matches('/');
    let version = show(&dir, &head, &format!("{path}/.claude-plugin/plugin.json")).as_deref().and_then(plugin_version).ok_or_else(|| format!("no {plugin} version on {head}"))?;
    Ok((version, show(&dir, &head, &format!("{path}/hooks/hooks.json")).as_deref().and_then(pin)))
}

pub fn check(places: &Places) -> Vec<Component> {
    let mut components = installed(places);
    let kelpie = &mut components[0];
    match places.root.as_deref().ok_or_else(|| "cannot tell where kelpie is installed".to_string()).and_then(|root| latest_kelpie(root, places.channel, places.binaries.as_ref())) {
        Ok((Some(latest), unpublished)) => (kelpie.latest, kelpie.unpublished) = (Some(latest), unpublished),
        Ok((None, unpublished)) => (kelpie.error, kelpie.unpublished) = (Some(crate::release::not_published(unpublished.as_deref().unwrap_or_default())), unpublished),
        Err(error) => kelpie.error = Some(error),
    }
    if let Some((id, _, _)) = crate::vault_cli::installed(&places.claude_dir) {
        let latest = latest_vault(&places.claude_dir, &id);
        for component in components.iter_mut().filter(|component| component.name != KELPIE) {
            match &latest {
                Ok((version, pin)) => component.latest = if component.name == VAULT { Some(version.clone()) } else { pin.clone() },
                Err(error) => component.error = Some(error.clone()),
            }
        }
    }
    components
}

pub fn render(components: &[Component], running: Option<&str>) -> String {
    let width = components.iter().map(|component| component.label().len()).max().unwrap_or(0);
    let mut lines: Vec<String> = components.iter().map(|component| format!("{:<width$}  {}", component.label(), state(component))).collect();
    if let Some(running) = running {
        lines.push(String::new());
        lines.push(format!("kelpie is {running}; kelpie update --stable returns to it."));
    }
    if components.iter().any(Component::newer) {
        lines.push(String::new());
        lines.push("kelpie update brings everything to the latest.".to_string());
    }
    lines.join("\n")
}

fn state(component: &Component) -> String {
    let installed = component.installed.as_deref().unwrap_or("?");
    let mut parts = vec![match &component.built {
        Some(built) => format!("{installed} installed ({built})"),
        None => format!("{installed} installed"),
    }];
    if component.rebuild() {
        parts.push(format!("{} in the checkout: rebuild", component.checkout.as_deref().unwrap_or_default()));
    }
    match (&component.latest, &component.error) {
        (Some(latest), _) if component.latest_ahead() => parts.push(format!("{latest} available")),
        (Some(_), _) if component.rebuild() => {}
        (Some(_), _) => parts.push("the latest".to_string()),
        (None, Some(error)) => parts.push(format!("latest unknown: {error}")),
        (None, None) => parts.push("latest unknown".to_string()),
    }
    let mut text = parts.join(", ");
    if let Some(unpublished) = component.unpublished.as_ref().filter(|unpublished| component.latest.is_some() && component.installed.as_ref().is_none_or(|installed| newer(unpublished, installed))) {
        text.push_str(&format!("; {}", crate::release::not_published(unpublished)));
    }
    text
}

pub struct Cache {
    pub checked_at: u64,
    pub channel: Option<String>,
    pub components: Vec<Component>,
}

fn cache_file(state: &Path) -> PathBuf {
    state.join(FILE)
}

pub fn read_cache(state: &Path) -> Option<Cache> {
    let value = read_json(&cache_file(state))?;
    let components = value["components"].as_array().into_iter().flatten().filter_map(Component::from_json).collect();
    Some(Cache { checked_at: value["checked_at"].as_u64()?, channel: value["channel"].as_str().map(str::to_string), components })
}

fn write_cache(state: &Path, checked_at: u64, channel: Channel, components: &[Component]) {
    let value = json!({ "checked_at": checked_at, "channel": channel.key(), "components": components.iter().map(Component::to_json).collect::<Vec<_>>() });
    let _ = crate::state::write(&cache_file(state), value.to_string());
}

pub fn forget(state: &Path) {
    let _ = std::fs::remove_file(cache_file(state));
}

// The installed side is read again each time, so an update made elsewhere clears the notice without a fetch.
fn with_installed(mut components: Vec<Component>, now: &[Component]) -> Vec<Component> {
    for component in &mut components {
        if let Some(found) = now.iter().find(|found| found.name == component.name) {
            component.installed = found.installed.clone();
            component.built = found.built.clone();
            component.checkout = found.checkout.clone();
        }
    }
    components.retain(|component| now.iter().any(|found| found.name == component.name));
    components
}

// What the last check found, without checking: a board reads this while the keeper does the checking.
pub fn cached(state: &Path, channel: Channel, installed: &[Component]) -> Vec<Component> {
    read_cache(state).filter(|cache| cache.channel.as_deref() == Some(channel.key())).map(|cache| with_installed(cache.components, installed)).unwrap_or_default()
}

// A release that is tagged but not yet published is usually published within minutes, so it is looked for again sooner.
fn every(components: &[Component]) -> u64 {
    if components.iter().any(|component| component.unpublished.is_some()) { PENDING_EVERY } else { EVERY }
}

// The claim is written before the fetch, so other boards starting meanwhile do not fetch too.
// A check made for the other channel compared against the wrong latest, so it counts as none.
pub fn current(state: &Path, now: u64, channel: Channel, installed: Vec<Component>, fetch: impl FnOnce() -> Vec<Component>) -> Vec<Component> {
    let cache = read_cache(state).filter(|cache| cache.channel.as_deref() == Some(channel.key()));
    if let Some(cache) = cache.as_ref().filter(|cache| now.saturating_sub(cache.checked_at) < every(&cache.components)) {
        return with_installed(cache.components.clone(), &installed);
    }
    write_cache(state, now, channel, &cache.map(|cache| cache.components).unwrap_or_default());
    let fresh = fetch();
    write_cache(state, now, channel, &fresh);
    fresh
}

pub fn notice(components: &[Component], running: Option<&crate::release::Override>) -> Vec<(String, String)> {
    let running = running.map(|found| ("running".to_string(), format!("{}, not the channel", found.name())));
    running.into_iter().chain(components.iter().filter(|component| component.name != PIN)
        .filter_map(|component| Some((component.name.clone(), component.target()?.clone())))).collect()
}

pub fn doctor_line(state: &Path, channel: Channel, installed: &[Component]) -> (bool, String) {
    if let Some(running) = crate::release::read_override(state) {
        return (false, format!("kelpie is {}; kelpie update --stable returns to it", running.describe(channel)));
    }
    let Some(cache) = read_cache(state).filter(|cache| cache.channel.as_deref() == Some(channel.key())) else { return (true, format!("not checked yet on the {} channel; kelpie update --check, or kelpie checks every 6 hours", channel.key())) };
    let components = with_installed(cache.components, installed);
    let age = match crate::messages::age(cache.checked_at).as_str() {
        "now" => "just now".to_string(),
        age => format!("{age} ago"),
    };
    let newer: Vec<String> = components.iter().filter_map(|component| Some(format!("{} {} → {}", component.label(), component.installed.as_deref()?, component.target()?))).collect();
    if !newer.is_empty() {
        return (false, format!("{} (checked {age}); kelpie update", newer.join(", ")));
    }
    if components.iter().any(|component| component.latest.is_none()) {
        return (true, format!("the last check could not read every latest version (checked {age}); kelpie update --check says why"));
    }
    (true, format!("everything is the latest (checked {age})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert!(newer("0.18.10", "0.18.9"));
        assert!(!newer("0.18.9", "0.18.10"));
        assert!(newer("0.19.0", "0.18.12"));
        assert!(newer("1.0", "0.99.99"));
        assert!(!newer("0.18.1", "0.18.1"));
        assert!(!newer("0.18", "0.18.0"));
        assert!(newer("0.18.1.1", "0.18.1"));
        assert!(!newer("garbage", "0.1.0") && !newer("0.1.0", ""));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-update-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run_git(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null"]).args(args).current_dir(cwd).output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn commit_all(repo: &Path, message: &str) {
        run_git(repo, &["add", "-A"]);
        run_git(repo, &["commit", "--quiet", "-m", message]);
    }

    fn hooks(pin: &str) -> String {
        format!(r#"{{"hooks":{{"SessionStart":[{{"hooks":[{{"type":"command","command":"uvx notes-vault-mcp=={pin} hook session-start"}}]}}]}}}}"#)
    }

    fn vault_release(repo: &Path, version: &str, pin: &str) {
        write(&repo.join("plugins/vault/.claude-plugin/plugin.json"), &format!(r#"{{"name":"vault","version":"{version}"}}"#));
        write(&repo.join("plugins/vault/hooks/hooks.json"), &hooks(pin));
        commit_all(repo, &format!("vault {version}"));
    }

    fn fake_binary(root: &Path, said: &str) {
        let binary = root.join("target/release/kelpie");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        crate::executable::write(&binary, format!("#!/bin/sh\n[ \"$1\" = --version ] && echo '{said}'\n")).unwrap();
    }

    fn fake_install(dir: &Path, with_vault: bool) -> Places {
        let origin = dir.join("kelpie-origin");
        std::fs::create_dir_all(&origin).unwrap();
        run_git(&origin, &["init", "--quiet", "-b", "main"]);
        write(&origin.join(".gitignore"), "target/\n");
        write(&origin.join(".claude-plugin/plugin.json"), r#"{"name":"kelpie","version":"0.18.1"}"#);
        commit_all(&origin, "kelpie 0.18.1");
        run_git(dir, &["clone", "--quiet", origin.to_str().unwrap(), "root"]);
        fake_binary(&dir.join("root"), "kelpie 0.18.1 (abc1234 2026-10-01)");
        write(&origin.join(".claude-plugin/plugin.json"), r#"{"name":"kelpie","version":"0.18.10"}"#);
        commit_all(&origin, "kelpie 0.18.10");
        let claude_dir = dir.join("claude");
        if with_vault {
            let market = dir.join("market-origin");
            std::fs::create_dir_all(&market).unwrap();
            run_git(&market, &["init", "--quiet", "-b", "main"]);
            write(&market.join(".claude-plugin/marketplace.json"), r#"{"name":"gronare","plugins":[{"name":"vault","source":"./plugins/vault"}]}"#);
            vault_release(&market, "0.2.16", "0.6.0");
            let clone = claude_dir.join("plugins/marketplaces/gronare");
            std::fs::create_dir_all(clone.parent().unwrap()).unwrap();
            run_git(clone.parent().unwrap(), &["clone", "--quiet", market.to_str().unwrap(), "gronare"]);
            vault_release(&market, "0.2.17", "0.6.1");
            let install = claude_dir.join("plugins/cache/gronare/vault/0.2.16");
            write(&install.join("hooks/hooks.json"), &hooks("0.6.0"));
            write(&claude_dir.join("plugins/installed_plugins.json"), &json!({ "plugins": { "vault@gronare": [{ "installPath": install, "version": "0.2.16" }] } }).to_string());
            write(&claude_dir.join("plugins/known_marketplaces.json"), &json!({ "gronare": { "installLocation": clone } }).to_string());
        }
        let binaries = crate::release::Binaries { base: Some(format!("file://{}", dir.join("releases").display())), target: crate::release::target_triple(std::env::consts::OS, std::env::consts::ARCH), curl: crate::release::fake_curl(dir) };
        Places { binaries: Some(binaries), ..Places::new(Some(dir.join("root")), claude_dir, Channel::Main) }
    }

    #[test]
    fn check_reports_installed_against_the_fetched_default_branches_and_changes_nothing() {
        let dir = scratch("check");
        let places = fake_install(&dir, true);
        let root = places.root.clone().unwrap();
        let market = places.claude_dir.join("plugins/marketplaces/gronare");
        let heads = (run_git(&root, &["rev-parse", "HEAD"]), run_git(&market, &["rev-parse", "HEAD"]));
        let components = check(&places);
        let shown: Vec<(&str, Option<&str>, Option<&str>)> = components.iter().map(|component| (component.name.as_str(), component.installed.as_deref(), component.latest.as_deref())).collect();
        assert_eq!(shown, [("kelpie", Some("0.18.1"), Some("0.18.10")), ("vault", Some("0.2.16"), Some("0.2.17")), ("notes-vault-mcp", Some("0.6.0"), Some("0.6.1"))]);
        let text = render(&components, None);
        assert!(text.contains("kelpie           0.18.1 installed (abc1234), 0.18.10 available"), "{text}");
        assert!(text.contains("vault plugin     0.2.16 installed, 0.2.17 available"), "{text}");
        assert!(text.contains("notes-vault-mcp  0.6.0 installed, 0.6.1 available"), "{text}");
        assert!(text.ends_with("kelpie update brings everything to the latest."), "{text}");
        assert_eq!((run_git(&root, &["rev-parse", "HEAD"]), run_git(&market, &["rev-parse", "HEAD"])), heads, "no merge, no checkout");
        assert!(run_git(&root, &["status", "--porcelain"]).is_empty() && run_git(&market, &["status", "--porcelain"]).is_empty());
        assert_eq!(notice(&components, None), [("kelpie".to_string(), "0.18.10".to_string()), ("vault".to_string(), "0.2.17".to_string())]);
    }

    #[test]
    fn check_without_the_vault_plugin_reports_kelpie_alone_and_a_failed_fetch_is_unknown() {
        let dir = scratch("alone");
        let places = fake_install(&dir, false);
        run_git(places.root.as_deref().unwrap(), &["pull", "--quiet", "--ff-only"]);
        let moved = check(&places);
        assert_eq!(render(&moved, None), "kelpie  0.18.1 installed (abc1234), 0.18.10 in the checkout: rebuild\n\nkelpie update brings everything to the latest.", "a checkout moved ahead without a rebuild is the binary's version, not the files'");
        assert_eq!(notice(&moved, None), [("kelpie".to_string(), "0.18.10".to_string())], "so the board offers the update that rebuilds it");
        fake_binary(places.root.as_deref().unwrap(), "kelpie 0.18.10 (def5678 2026-10-02)");
        let components = check(&places);
        assert_eq!(components.len(), 1);
        assert_eq!(render(&components, None), "kelpie  0.18.10 installed (def5678), the latest");
        run_git(places.root.as_deref().unwrap(), &["remote", "set-url", "origin", dir.join("missing").to_str().unwrap()]);
        let components = check(&places);
        assert!(components[0].latest.is_none() && components[0].error.is_some(), "{components:?}");
        assert!(notice(&components, None).is_empty(), "a failed check shows nothing");
        assert!(render(&components, None).starts_with("kelpie  0.18.10 installed (def5678), latest unknown: git fetch"), "{}", render(&components, None));
    }

    #[test]
    fn the_cache_fetches_at_most_every_six_hours_and_rereads_what_is_installed() {
        let state = scratch("cache");
        let fetched = std::cell::Cell::new(0);
        let component = |installed: &str, latest: &str| Component { name: KELPIE.to_string(), installed: Some(installed.to_string()), latest: Some(latest.to_string()), ..Component::default() };
        let fetch = || { fetched.set(fetched.get() + 1); vec![component("0.18.1", "0.18.2")] };
        let start = 1_000_000;
        let installed = || vec![Component::new(KELPIE, Some("0.18.1".to_string()))];
        assert!(current(&state, start, Channel::Main, installed(), fetch)[0].newer());
        assert_eq!(fetched.get(), 1);
        assert_eq!(read_cache(&state).unwrap().checked_at, start);
        assert!(current(&state, start + EVERY - 1, Channel::Main, installed(), fetch)[0].newer());
        assert_eq!(fetched.get(), 1, "a second board within 6 h reads the cache");
        let updated = current(&state, start + 60, Channel::Main, vec![Component::new(KELPIE, Some("0.18.2".to_string()))], fetch);
        assert!(!updated[0].newer() && fetched.get() == 1, "an update made elsewhere clears the notice without a fetch");
        current(&state, start + EVERY, Channel::Main, installed(), fetch);
        assert_eq!(fetched.get(), 2, "after 6 h it fetches again");
        assert_eq!(read_cache(&state).unwrap().checked_at, start + EVERY);
        forget(&state);
        current(&state, start + EVERY + 1, Channel::Main, installed(), fetch);
        assert_eq!(fetched.get(), 3, "kelpie update forgets the cache");
        current(&state, start + EVERY + 2, Channel::Release, installed(), fetch);
        assert_eq!(fetched.get(), 4, "a check made for the other channel is no check");
        assert!(cached(&state, Channel::Release, &installed())[0].newer() && fetched.get() == 4, "the board reads what the keeper found and fetches nothing");
        assert!(cached(&state, Channel::Main, &installed()).is_empty(), "nor a check for the other channel");
    }

    #[test]
    fn the_release_channel_compares_against_the_highest_tag_and_the_notice_names_it() {
        let dir = scratch("release");
        let mut places = fake_install(&dir, false);
        places.channel = Channel::Release;
        let origin = dir.join("kelpie-origin");
        let tagged = check(&places);
        assert_eq!(tagged[0].error.as_deref(), Some("origin has no vX.Y.Z release tag yet"), "main's 0.18.10 is not a release");
        assert!(notice(&tagged, None).is_empty());
        run_git(&origin, &["tag", "v0.18.2", "HEAD~1"]);
        run_git(&origin, &["tag", "v0.18.10-rc1"]);
        run_git(&origin, &["tag", "nightly"]);
        write(&origin.join(".claude-plugin/plugin.json"), r#"{"name":"kelpie","version":"0.19.0"}"#);
        commit_all(&origin, "kelpie 0.19.0 on main, not released");
        let draft = check(&places);
        assert_eq!((draft[0].latest.as_deref(), draft[0].error.as_deref()), (None, Some("kelpie 0.18.2 is not published yet; try again in a while")), "a tag whose release binary is not downloadable is never offered");
        assert!(notice(&draft, None).is_empty());
        publish(&dir, "v0.18.2");
        let components = check(&places);
        assert_eq!((components[0].installed.as_deref(), components[0].latest.as_deref()), (Some("0.18.1"), Some("v0.18.2")));
        assert_eq!(notice(&components, None), [("kelpie".to_string(), "v0.18.2".to_string())], "the tag, never what main carries");
        assert!(render(&components, None).starts_with("kelpie  0.18.1 installed (abc1234), v0.18.2 available"), "{}", render(&components, None));
        run_git(&origin, &["tag", "v0.18.3"]);
        let pending = check(&places);
        assert_eq!(notice(&pending, None), [("kelpie".to_string(), "v0.18.2".to_string())], "a newer draft leaves the published release on offer");
        assert!(render(&pending, None).starts_with("kelpie  0.18.1 installed (abc1234), v0.18.2 available; kelpie 0.18.3 is not published yet; try again in a while"), "{}", render(&pending, None));
        places.channel = Channel::Main;
        assert_eq!(check(&places)[0].latest.as_deref(), Some("0.19.0"), "main compares against origin's default branch");
    }

    fn publish(dir: &Path, tag: &str) {
        let target = crate::release::target_triple(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        write(&dir.join("releases").join(tag).join(format!("kelpie-{}-{target}.tar.gz", tag.trim_start_matches('v'))), "binary");
    }

    #[test]
    fn a_release_checkout_on_an_unpublished_tag_offers_no_rebuild() {
        let on_draft = Component { name: KELPIE.to_string(), installed: Some("0.24.0".to_string()), built: Some("e70d5d5".to_string()), checkout: Some("0.25.0".to_string()), latest: Some("v0.24.0".to_string()), unpublished: Some("v0.25.0".to_string()), error: None };
        assert!(!on_draft.newer() && notice(std::slice::from_ref(&on_draft), None).is_empty(), "never ↑ kelpie 0.25.0 while 0.25.0 is a draft");
        assert_eq!(render(std::slice::from_ref(&on_draft), None), "kelpie  0.24.0 installed (e70d5d5), the latest; kelpie 0.25.0 is not published yet; try again in a while");
        let nothing_published = Component { latest: None, error: Some(crate::release::not_published("v0.25.0")), ..on_draft.clone() };
        assert!(notice(&[nothing_published], None).is_empty());
        let main = Component { latest: Some("0.25.0".to_string()), unpublished: None, ..on_draft };
        assert_eq!(notice(&[main], None), [("kelpie".to_string(), "0.25.0".to_string())], "a main checkout ahead of its binary is rebuilt");
    }

    #[test]
    fn an_unpublished_release_is_looked_for_again_after_fifteen_minutes() {
        let state = scratch("pending");
        let fetched = std::cell::Cell::new(0);
        let pending = Component { name: KELPIE.to_string(), latest: Some("v0.24.0".to_string()), unpublished: Some("v0.25.0".to_string()), ..Component::default() };
        let fetch = || { fetched.set(fetched.get() + 1); vec![pending.clone()] };
        current(&state, 1_000, Channel::Release, Vec::new(), fetch);
        current(&state, 1_000 + PENDING_EVERY - 1, Channel::Release, Vec::new(), fetch);
        assert_eq!(fetched.get(), 1);
        current(&state, 1_000 + PENDING_EVERY, Channel::Release, Vec::new(), fetch);
        assert_eq!(fetched.get(), 2, "not six hours while a tagged release waits to be published");
    }

    #[test]
    fn the_installed_version_is_what_the_binary_says() {
        assert_eq!(parse_binary_version("kelpie 0.24.0 (e70d5d5 2026-10-20)\n"), Some(("0.24.0".to_string(), Some("e70d5d5".to_string()))));
        assert_eq!(parse_binary_version("kelpie 0.24.0\n"), Some(("0.24.0".to_string(), None)), "a build outside git names no commit");
        assert_eq!(parse_binary_version("usage: kelpie <command>"), None);
        assert_eq!(parse_binary_version(""), None);
        let own = std::env::current_exe().unwrap();
        assert!(binary_version(&own.with_file_name("no-such-binary")).is_none());
    }

    #[test]
    fn an_override_shows_in_the_check_the_notice_and_doctor() {
        let state = scratch("override");
        std::fs::write(state.join(crate::release::OVERRIDE_FILE), r#"{"branch":"try-this","sha":"abcdef0123"}"#).unwrap();
        let running = crate::release::read_override(&state).unwrap();
        let kelpie = Component { name: KELPIE.to_string(), installed: Some("0.20.0".to_string()), latest: Some("v0.20.0".to_string()), ..Component::default() };
        let text = render(std::slice::from_ref(&kelpie), Some(&running.describe(Channel::Release)));
        assert!(text.contains("kelpie is running try-this @ abcdef0, not the release channel; kelpie update --stable returns to it."), "{text}");
        assert_eq!(notice(&[kelpie], Some(&running)), [("running".to_string(), "try-this, not the channel".to_string())]);
        assert_eq!(doctor_line(&state, Channel::Release, &[]), (false, "kelpie is running try-this @ abcdef0, not the release channel; kelpie update --stable returns to it".to_string()));
    }

    #[test]
    fn the_cache_claims_the_check_before_fetching() {
        let state = scratch("claim");
        current(&state, 50, Channel::Main, Vec::new(), || {
            assert_eq!(read_cache(&state).map(|cache| cache.checked_at), Some(50));
            Vec::new()
        });
    }

    #[test]
    fn doctor_reads_the_cache_and_names_what_is_newer() {
        let state = scratch("doctor");
        let installed = vec![Component::new(KELPIE, Some("0.18.1".to_string())), Component::new(VAULT, Some("0.2.17".to_string()))];
        assert!(doctor_line(&state, Channel::Main, &installed).1.starts_with("not checked yet"));
        let latest = |name: &str, version: &str| Component { name: name.to_string(), installed: None, latest: Some(version.to_string()), ..Component::default() };
        write_cache(&state, crate::messages::now(), Channel::Main, &[latest(KELPIE, "0.18.2"), latest(VAULT, "0.2.17")]);
        assert_eq!(doctor_line(&state, Channel::Main, &installed), (false, "kelpie 0.18.1 → 0.18.2 (checked just now); kelpie update".to_string()));
        write_cache(&state, crate::messages::now(), Channel::Main, &[latest(KELPIE, "0.18.1"), latest(VAULT, "0.2.17")]);
        assert_eq!(doctor_line(&state, Channel::Main, &installed), (true, "everything is the latest (checked just now)".to_string()));
    }

    #[test]
    fn the_pin_is_read_from_the_hook_commands() {
        assert_eq!(pin(r#"{"hooks":{"SessionStart":[{"hooks":[{"command":"uvx notes-vault-mcp==0.6.0 hook session-start"}]}]}}"#), Some("0.6.0".to_string()));
        assert_eq!(pin("uvx notes-vault-mcp hook"), None);
        assert_eq!(pin("notes-vault-mcp==0.6.0rc1 hook"), Some("0.6.0rc1".to_string()));
        assert_eq!(pin("notes-vault-mcp== hook"), None);
    }

    #[test]
    fn kelpies_version_is_the_plugin_manifests_and_the_cargo_manifest_stands_in() {
        let read = |files: &'static [(&'static str, &'static str)]| move |name: &str| files.iter().find(|(file, _)| *file == name).map(|(_, text)| text.to_string());
        assert_eq!(kelpie_version(read(&[(".claude-plugin/plugin.json", r#"{"version":"0.19.0"}"#), ("Cargo.toml", "[package]\nversion = \"0.18.0\"\n")])), Some("0.19.0".to_string()));
        assert_eq!(kelpie_version(read(&[("Cargo.toml", "[package]\nname = \"kelpie\"\nversion = \"0.18.0\"\n")])), Some("0.18.0".to_string()));
        assert_eq!(kelpie_version(read(&[(".claude-plugin/plugin.json", "not json")])), None);
        assert_eq!(kelpie_version(read(&[])), None);
        let manifest: toml::Table = include_str!("../Cargo.toml").parse().unwrap();
        let plugin: Value = serde_json::from_str(include_str!("../.claude-plugin/plugin.json")).unwrap();
        let herdr: toml::Table = include_str!("../herdr-plugin.toml").parse().unwrap();
        let cargo = manifest["package"]["version"].as_str().unwrap();
        assert_eq!(plugin["version"].as_str().unwrap(), cargo, "the three manifests release together");
        assert_eq!(herdr["version"].as_str().unwrap(), cargo);
    }

    #[test]
    fn a_cache_that_cannot_be_read_is_no_cache() {
        let state = scratch("broken");
        std::fs::write(cache_file(&state), "not json").unwrap();
        assert!(read_cache(&state).is_none());
        std::fs::write(cache_file(&state), r#"{"components":[{"name":"kelpie"},{"installed":"x"}]}"#).unwrap();
        assert!(read_cache(&state).is_none(), "no checked_at, no cache");
        std::fs::write(cache_file(&state), r#"{"checked_at":5,"components":[{"name":"kelpie"},{"installed":"x"}]}"#).unwrap();
        let cache = read_cache(&state).unwrap();
        assert_eq!((cache.checked_at, cache.components.len()), (5, 1), "a component without a name is dropped");
        let _ = std::fs::remove_dir_all(state);
    }
}
