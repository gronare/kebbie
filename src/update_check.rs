use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::sh::Result;

pub const FILE: &str = "update-check.json";
pub const EVERY: u64 = 6 * 60 * 60;
pub const KELPIE: &str = "kelpie";
pub const VAULT: &str = "vault";
pub const PIN: &str = "notes-vault-mcp";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Component {
    pub name: String,
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub error: Option<String>,
}

impl Component {
    fn new(name: &str, installed: Option<String>) -> Self {
        Self { name: name.to_string(), installed, ..Self::default() }
    }

    pub fn newer(&self) -> bool {
        matches!((&self.installed, &self.latest), (Some(installed), Some(latest)) if newer(latest, installed))
    }

    pub fn label(&self) -> &str {
        if self.name == VAULT { "vault plugin" } else { &self.name }
    }

    fn to_json(&self) -> Value {
        json!({ "name": self.name, "installed": self.installed, "latest": self.latest, "error": self.error })
    }

    fn from_json(value: &Value) -> Option<Self> {
        let text = |key: &str| value[key].as_str().map(str::to_string);
        Some(Self { name: text("name")?, installed: text("installed"), latest: text("latest"), error: text("error") })
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
}

impl Places {
    pub fn current() -> Self {
        Self { root: install_root(), claude_dir: crate::tasks::claude_home().unwrap_or_default() }
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

pub fn installed(places: &Places) -> Vec<Component> {
    let mut components = vec![Component::new(KELPIE, places.root.as_deref().and_then(|root| kelpie_version(|file| std::fs::read_to_string(root.join(file)).ok())))];
    if let Some((_, version, root)) = crate::vault_cli::installed(&places.claude_dir) {
        components.push(Component::new(VAULT, Some(version)));
        components.push(Component::new(PIN, pin(&crate::vault_cli::hooks_of(&root))));
    }
    components
}

// A fetch must never wait on a password or a passphrase, as the board runs it unseen.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
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

fn latest_kelpie(root: &Path) -> Result<String> {
    let head = fetched_default(root)?;
    kelpie_version(|file| show(root, &head, file)).ok_or_else(|| format!("no version on {head}"))
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
    match places.root.as_deref().ok_or_else(|| "cannot tell where kelpie is installed".to_string()).and_then(latest_kelpie) {
        Ok(latest) => kelpie.latest = Some(latest),
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

pub fn render(components: &[Component]) -> String {
    let width = components.iter().map(|component| component.label().len()).max().unwrap_or(0);
    let mut lines: Vec<String> = components.iter().map(|component| {
        let installed = component.installed.as_deref().unwrap_or("?");
        let state = match (&component.latest, &component.error) {
            (Some(latest), _) if component.newer() => format!("{installed} installed, {latest} available"),
            (Some(_), _) => format!("{installed} installed, the latest"),
            (None, Some(error)) => format!("{installed} installed, latest unknown: {error}"),
            (None, None) => format!("{installed} installed, latest unknown"),
        };
        format!("{:<width$}  {state}", component.label())
    }).collect();
    if components.iter().any(Component::newer) {
        lines.push(String::new());
        lines.push("kelpie update brings everything to the latest.".to_string());
    }
    lines.join("\n")
}

pub struct Cache {
    pub checked_at: u64,
    pub components: Vec<Component>,
}

fn cache_file(state: &Path) -> PathBuf {
    state.join(FILE)
}

pub fn read_cache(state: &Path) -> Option<Cache> {
    let value = read_json(&cache_file(state))?;
    let components = value["components"].as_array().into_iter().flatten().filter_map(Component::from_json).collect();
    Some(Cache { checked_at: value["checked_at"].as_u64()?, components })
}

fn write_cache(state: &Path, checked_at: u64, components: &[Component]) {
    let value = json!({ "checked_at": checked_at, "components": components.iter().map(Component::to_json).collect::<Vec<_>>() });
    let _ = std::fs::write(cache_file(state), value.to_string());
}

pub fn forget(state: &Path) {
    let _ = std::fs::remove_file(cache_file(state));
}

// The installed side is read again each time, so an update made elsewhere clears the notice without a fetch.
fn with_installed(mut components: Vec<Component>, now: &[Component]) -> Vec<Component> {
    for component in &mut components {
        if let Some(found) = now.iter().find(|found| found.name == component.name) {
            component.installed = found.installed.clone();
        }
    }
    components.retain(|component| now.iter().any(|found| found.name == component.name));
    components
}

// The claim is written before the fetch, so other boards starting meanwhile do not fetch too.
pub fn current(state: &Path, now: u64, installed: Vec<Component>, fetch: impl FnOnce() -> Vec<Component>) -> Vec<Component> {
    let cache = read_cache(state);
    if let Some(cache) = cache.as_ref().filter(|cache| now.saturating_sub(cache.checked_at) < EVERY) {
        return with_installed(cache.components.clone(), &installed);
    }
    write_cache(state, now, &cache.map(|cache| cache.components).unwrap_or_default());
    let fresh = fetch();
    write_cache(state, now, &fresh);
    fresh
}

pub fn notice(components: &[Component]) -> Vec<(String, String)> {
    components.iter().filter(|component| component.name != PIN && component.newer())
        .filter_map(|component| Some((component.name.clone(), component.latest.clone()?))).collect()
}

pub fn doctor_line(state: &Path, installed: &[Component]) -> (bool, String) {
    let Some(cache) = read_cache(state) else { return (true, "not checked yet; kelpie update --check, or the board checks every 6 hours".to_string()) };
    let components = with_installed(cache.components, installed);
    let age = match crate::messages::age(cache.checked_at).as_str() {
        "now" => "just now".to_string(),
        age => format!("{age} ago"),
    };
    let newer: Vec<String> = components.iter().filter(|component| component.newer())
        .map(|component| format!("{} {} → {}", component.label(), component.installed.as_deref().unwrap_or("?"), component.latest.as_deref().unwrap_or("?"))).collect();
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

    fn fake_install(dir: &Path, with_vault: bool) -> Places {
        let origin = dir.join("kelpie-origin");
        std::fs::create_dir_all(&origin).unwrap();
        run_git(&origin, &["init", "--quiet", "-b", "main"]);
        write(&origin.join(".claude-plugin/plugin.json"), r#"{"name":"kelpie","version":"0.18.1"}"#);
        commit_all(&origin, "kelpie 0.18.1");
        run_git(dir, &["clone", "--quiet", origin.to_str().unwrap(), "root"]);
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
        Places { root: Some(dir.join("root")), claude_dir }
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
        let text = render(&components);
        assert!(text.contains("kelpie           0.18.1 installed, 0.18.10 available"), "{text}");
        assert!(text.contains("vault plugin     0.2.16 installed, 0.2.17 available"), "{text}");
        assert!(text.contains("notes-vault-mcp  0.6.0 installed, 0.6.1 available"), "{text}");
        assert!(text.ends_with("kelpie update brings everything to the latest."), "{text}");
        assert_eq!((run_git(&root, &["rev-parse", "HEAD"]), run_git(&market, &["rev-parse", "HEAD"])), heads, "no merge, no checkout");
        assert!(run_git(&root, &["status", "--porcelain"]).is_empty() && run_git(&market, &["status", "--porcelain"]).is_empty());
        assert_eq!(notice(&components), [("kelpie".to_string(), "0.18.10".to_string()), ("vault".to_string(), "0.2.17".to_string())]);
    }

    #[test]
    fn check_without_the_vault_plugin_reports_kelpie_alone_and_a_failed_fetch_is_unknown() {
        let dir = scratch("alone");
        let places = fake_install(&dir, false);
        run_git(places.root.as_deref().unwrap(), &["pull", "--quiet", "--ff-only"]);
        let components = check(&places);
        assert_eq!(components.len(), 1);
        assert_eq!(render(&components), "kelpie  0.18.10 installed, the latest");
        run_git(places.root.as_deref().unwrap(), &["remote", "set-url", "origin", dir.join("missing").to_str().unwrap()]);
        let components = check(&places);
        assert!(components[0].latest.is_none() && components[0].error.is_some(), "{components:?}");
        assert!(notice(&components).is_empty(), "a failed check shows nothing");
        assert!(render(&components).starts_with("kelpie  0.18.10 installed, latest unknown: git fetch"), "{}", render(&components));
    }

    #[test]
    fn the_cache_fetches_at_most_every_six_hours_and_rereads_what_is_installed() {
        let state = scratch("cache");
        let fetched = std::cell::Cell::new(0);
        let component = |installed: &str, latest: &str| Component { name: KELPIE.to_string(), installed: Some(installed.to_string()), latest: Some(latest.to_string()), error: None };
        let fetch = || { fetched.set(fetched.get() + 1); vec![component("0.18.1", "0.18.2")] };
        let start = 1_000_000;
        let installed = || vec![Component::new(KELPIE, Some("0.18.1".to_string()))];
        assert!(current(&state, start, installed(), fetch)[0].newer());
        assert_eq!(fetched.get(), 1);
        assert_eq!(read_cache(&state).unwrap().checked_at, start);
        assert!(current(&state, start + EVERY - 1, installed(), fetch)[0].newer());
        assert_eq!(fetched.get(), 1, "a second board within 6 h reads the cache");
        let updated = current(&state, start + 60, vec![Component::new(KELPIE, Some("0.18.2".to_string()))], fetch);
        assert!(!updated[0].newer() && fetched.get() == 1, "an update made elsewhere clears the notice without a fetch");
        current(&state, start + EVERY, installed(), fetch);
        assert_eq!(fetched.get(), 2, "after 6 h it fetches again");
        assert_eq!(read_cache(&state).unwrap().checked_at, start + EVERY);
        forget(&state);
        current(&state, start + EVERY + 1, installed(), fetch);
        assert_eq!(fetched.get(), 3, "kelpie update forgets the cache");
    }

    #[test]
    fn the_cache_claims_the_check_before_fetching() {
        let state = scratch("claim");
        current(&state, 50, Vec::new(), || {
            assert_eq!(read_cache(&state).map(|cache| cache.checked_at), Some(50));
            Vec::new()
        });
    }

    #[test]
    fn doctor_reads_the_cache_and_names_what_is_newer() {
        let state = scratch("doctor");
        let installed = vec![Component::new(KELPIE, Some("0.18.1".to_string())), Component::new(VAULT, Some("0.2.17".to_string()))];
        assert!(doctor_line(&state, &installed).1.starts_with("not checked yet"));
        let latest = |name: &str, version: &str| Component { name: name.to_string(), installed: None, latest: Some(version.to_string()), error: None };
        write_cache(&state, crate::messages::now(), &[latest(KELPIE, "0.18.2"), latest(VAULT, "0.2.17")]);
        assert_eq!(doctor_line(&state, &installed), (false, "kelpie 0.18.1 → 0.18.2 (checked just now); kelpie update".to_string()));
        write_cache(&state, crate::messages::now(), &[latest(KELPIE, "0.18.1"), latest(VAULT, "0.2.17")]);
        assert_eq!(doctor_line(&state, &installed), (true, "everything is the latest (checked just now)".to_string()));
    }

    #[test]
    fn the_pin_is_read_from_the_hook_commands() {
        assert_eq!(pin(r#"{"hooks":{"SessionStart":[{"hooks":[{"command":"uvx notes-vault-mcp==0.6.0 hook session-start"}]}]}}"#), Some("0.6.0".to_string()));
        assert_eq!(pin("uvx notes-vault-mcp hook"), None);
    }
}
