use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::{self, Channel};
use crate::herdr;
use crate::lead;
use crate::release::{self, Plan, Request};
use crate::setup;
use crate::sh::{self, Result};
use crate::update_check::{self, Component, Places};
use crate::vault_cli;

pub fn run(claude_dir: &Path, visible: bool, request: &Request) -> Result<String> {
    let plugin = kelpie_plugin()?;
    let root = plugin["plugin_root"].as_str().filter(|root| !root.is_empty()).map(PathBuf::from);
    let channel = config::channel()?;
    let state = lead::state_dir()?;
    let places = Places::new(root.clone(), claude_dir.to_path_buf(), channel);
    let before = update_check::installed(&places);
    let mut lines = vec![herdr_plugin(&plugin, &state, channel, request, visible)?];
    lines.extend(finish_on_new_binary(claude_dir, root.as_deref()));
    let running = release::read_override(&state);
    let mut summary = versions(&before, &update_check::installed(&places));
    summary.insert(0, match &running {
        Some(running) => format!("kelpie is {}", running.describe(channel)),
        None => format!("kelpie follows the {} channel", channel.key()),
    });
    summary.push(String::new());
    summary.extend(lines);
    Ok(summary.join("\n"))
}

// The steps after the build run on the binary just built, so a fix to them reaches the update that installs it.
fn finish_on_new_binary(claude_dir: &Path, root: Option<&Path>) -> Vec<String> {
    let built = root.map(|root| root.join("target/release/kelpie")).filter(|binary| binary.is_file());
    match built.map(|binary| sh::run(&binary.to_string_lossy(), &["update", "--finish"], None)) {
        Some(Ok(output)) => output.lines().map(str::to_string).collect(),
        Some(Err(error)) => vec![format!("the new kelpie could not finish the update ({error}); this one does")].into_iter().chain(finish(claude_dir).unwrap_or_else(|error| vec![error])).collect(),
        None => finish(claude_dir).unwrap_or_else(|error| vec![error]),
    }
}

// The old keeper stops before the update restarts sessions, so the two never restart one together, and a keeper on this binary starts after.
pub fn finish(claude_dir: &Path) -> Result<Vec<String>> {
    let state = lead::state_dir()?;
    let keeper_ran = crate::keeper::stop_for_update(&state);
    let steps = finish_in(claude_dir, &state);
    let keeper = crate::keeper::start_after_update(&state, keeper_ran);
    match steps {
        Ok(mut lines) => {
            lines.push(keeper);
            Ok(lines)
        }
        Err(error) => Err(format!("{error}\n{keeper}")),
    }
}

fn finish_in(claude_dir: &Path, state: &Path) -> Result<Vec<String>> {
    let plugin = kelpie_plugin()?;
    let root = plugin["plugin_root"].as_str().filter(|root| !root.is_empty()).map(PathBuf::from);
    let channel = config::channel()?;
    let running = release::read_override(state);
    let mut lines = vec![claude_plugin(claude_dir, &setup::claude_bin(), root.as_deref(), channel == Channel::Main && running.is_none())];
    lines.extend(vault_plugin(claude_dir, &setup::claude_bin()));
    update_check::forget(state);
    crate::restart::mark_updated(state, crate::restart::now())?;
    lines.extend(crate::restart::report(&crate::restart::run(crate::restart::Mode::Update, &mut crate::restart::Memory::default(), None)?));
    lines.extend(crate::known::announce().unwrap_or_else(|error| vec![format!("telling the sessions what changed failed: {error}")]));
    lines.push(lead::renew_board().unwrap_or_else(|error| error));
    lines.extend(crate::issues::after_update(state, root.as_deref()));
    Ok(lines)
}

pub fn check(claude_dir: &Path) -> String {
    let places = Places::new(update_check::install_root(), claude_dir.to_path_buf(), config::channel().unwrap_or_default());
    let running = lead::state_location().ok().and_then(|state| release::read_override(&state)).map(|running| running.describe(places.channel));
    format!("channel {}\n\n{}", places.channel.key(), update_check::render(&update_check::check(&places), running.as_deref()))
}

fn versions(before: &[Component], after: &[Component]) -> Vec<String> {
    after.iter().map(|now| {
        let earlier = before.iter().find(|was| was.name == now.name);
        let was = earlier.and_then(|was| was.installed.clone());
        let built = (earlier.and_then(|was| was.built.as_ref()), now.built.as_ref());
        match (was, &now.installed) {
            (Some(was), Some(now_version)) if &was == now_version => match built {
                (Some(old), Some(new)) if old != new => format!("{} {was} rebuilt, {old} → {new}", now.label()),
                _ => format!("{} {was}, already latest", now.label()),
            },
            (Some(was), Some(now_version)) => format!("{} {was} → {now_version}", now.label()),
            (None, Some(now_version)) => format!("{} → {now_version}", now.label()),
            (_, None) => format!("{}: version unknown", now.label()),
        }
    }).collect()
}

fn kelpie_plugin() -> Result<Value> {
    let list: Value = serde_json::from_str(&sh::run(&herdr::bin(), &["plugin", "list", "--json"], None)?).map_err(|error| format!("herdr plugin list: {error}"))?;
    list["result"]["plugins"].as_array().into_iter().flatten().find(|plugin| plugin["plugin_id"] == "kelpie").cloned()
        .ok_or_else(|| "kelpie is not registered with herdr; kelpie doctor shows how".to_string())
}

// Inside the board the output is captured, as a child writing to the terminal would tear its screen.
fn step(program: &str, args: &[&str], cwd: Option<&Path>, visible: bool) -> Result<()> {
    if visible { sh::run_visibly(program, args, cwd) } else { sh::run(program, args, cwd).map(|_| ()) }
}

fn herdr_plugin(plugin: &Value, state: &Path, channel: Channel, request: &Request, visible: bool) -> Result<String> {
    let root = PathBuf::from(plugin["plugin_root"].as_str().filter(|root| !root.is_empty()).ok_or("herdr names no folder kelpie is installed in")?);
    match release::install(&root, state, channel, request, plugin["source"]["kind"] != "local", &release::Binaries::of(&root))? {
        Plan::Herdr => {
            step("env", &["KELPIE_CHANNEL=main", &herdr::bin(), "plugin", "update", "--yes", "kelpie"], None, visible)?;
            Ok("herdr plugin: updated from GitHub and rebuilt".to_string())
        }
        Plan::Done(line) => Ok(format!("herdr plugin: {line}; {}", build(&root, build_mode(channel, release::read_override(state).is_some()), visible)?)),
    }
}

pub const BUILD_SCRIPT: &str = "scripts/build.sh";
const ORIGIN_FILE: &str = "target/release/kelpie.origin";

#[derive(Debug, PartialEq)]
pub enum Origin {
    Downloaded { tag: String, target: String },
    Built,
}

impl Origin {
    pub fn describe(&self) -> String {
        match self {
            Origin::Downloaded { tag, target } => format!("downloaded {tag} for {target}"),
            Origin::Built => "built from source".to_string(),
        }
    }
}

// The build script writes the origin file after the binary it downloaded, so a binary newer than it was built by cargo since.
pub fn binary_origin(root: &Path) -> Option<Origin> {
    let modified = |path: PathBuf| std::fs::metadata(path).and_then(|meta| meta.modified()).ok();
    let binary = modified(root.join("target/release/kelpie"))?;
    let downloaded = modified(root.join(ORIGIN_FILE)).filter(|written| *written >= binary).and_then(|_| std::fs::read_to_string(root.join(ORIGIN_FILE)).ok()).and_then(|text| {
        let mut words = text.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("downloaded"), Some(tag), Some(target)) => Some(Origin::Downloaded { tag: tag.to_string(), target: target.to_string() }),
            _ => None,
        }
    });
    Some(downloaded.unwrap_or(Origin::Built))
}

// A branch or checkout installed with --from has no release binary, so it builds from source as the main channel does.
fn build_mode(channel: Channel, overridden: bool) -> Channel {
    if overridden { Channel::Main } else { channel }
}

// An install root checked out at a commit from before the build script has only cargo.
fn build(root: &Path, channel: Channel, visible: bool) -> Result<String> {
    if root.join(BUILD_SCRIPT).is_file() {
        step("env", &[&format!("KELPIE_CHANNEL={}", channel.key()), "sh", BUILD_SCRIPT, "--quiet"], Some(root), visible)?;
    } else {
        let _ = std::fs::remove_file(root.join(ORIGIN_FILE));
        step("cargo", &["build", "--release", "--quiet"], Some(root), visible)?;
    }
    let origin = binary_origin(root).map_or_else(|| "built".to_string(), |origin| origin.describe());
    Ok(format!("{origin} in {}", root.display()))
}

fn vault_plugin(claude_dir: &Path, claude: &str) -> Vec<String> {
    let Some((id, _, _)) = vault_cli::installed(claude_dir) else { return Vec::new() };
    let marketplace = id.split_once('@').map_or("gronare", |(_, marketplace)| marketplace).to_string();
    for args in [vec!["plugin", "marketplace", "update", marketplace.as_str()], vec!["plugin", "update", id.as_str()]] {
        if let Err(error) = sh::run(claude, &args, None) {
            return vec![format!("vault plugin: {error}")];
        }
    }
    Vec::new()
}

#[derive(Debug, PartialEq)]
pub enum Marketplace {
    InPlace(PathBuf),
    Git,
}

// A marketplace added from a folder loads its plugin from that folder at each start, so the install root's checkout is the plugin, whatever its version says.
pub fn marketplace(claude_dir: &Path, id: &str) -> Marketplace {
    let name = id.split_once('@').map_or(id, |(_, name)| name);
    let known: Value = std::fs::read_to_string(claude_dir.join("plugins/known_marketplaces.json")).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let entry = &known[name];
    if entry["source"]["source"] != "directory" {
        return Marketplace::Git;
    }
    Marketplace::InPlace(PathBuf::from(entry["source"]["path"].as_str().or(entry["installLocation"].as_str()).unwrap_or_default()))
}

pub const IN_PLACE_FIX: &str = "claude plugin marketplace remove kelpie && claude plugin marketplace add ~/.local/share/kelpie/current && claude plugin install kelpie@kelpie";

fn same_folder(one: &Path, other: &Path) -> bool {
    one.canonicalize().ok().is_some_and(|one| other.canonicalize().ok().as_ref() == Some(&one))
}

fn claude_plugin(claude_dir: &Path, claude: &str, root: Option<&Path>, follows_github: bool) -> String {
    let installed: Value = std::fs::read_to_string(claude_dir.join("plugins/installed_plugins.json")).ok()
        .and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let Some(id) = installed["plugins"].as_object().and_then(|plugins| plugins.keys().find(|id| id.starts_with("kelpie@")).cloned()) else {
        return "Claude Code plugin: not installed; kelpie doctor shows how".to_string();
    };
    match marketplace(claude_dir, &id) {
        Marketplace::InPlace(path) if root.is_some_and(|root| same_folder(&path, root)) => {
            let marketplace = id.split_once('@').map_or("kelpie", |(_, market)| market);
            match sh::run(claude, &["plugin", "marketplace", "update", marketplace], None).and_then(|_| sh::run(claude, &["plugin", "update", &id], None)) {
                Ok(output) => format!("Claude Code plugin: {}", output.lines().last().unwrap_or("up to date").trim_start_matches(['✔', ' '])),
                Err(error) => format!("Claude Code plugin: {error}"),
            }
        }
        Marketplace::InPlace(path) => format!("Claude Code plugin: loaded in place from {}, which is not the install root; {IN_PLACE_FIX}", path.display()),
        Marketplace::Git if follows_github => match sh::run(claude, &["plugin", "update", &id], None) {
            Ok(output) => format!("Claude Code plugin: {}", output.lines().last().unwrap_or("up to date").trim_start_matches(['✔', ' '])),
            Err(error) => format!("Claude Code plugin: {error}"),
        },
        Marketplace::Git => format!("Claude Code plugin: left as it is, since its GitHub marketplace follows the default branch and not this channel. Load it from the install root once (the remove forgets its options, such as auto_open): {IN_PLACE_FIX}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_steps_after_the_build_run_on_the_binary_just_built() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("finish");
        let binary = dir.join("target/release/kelpie");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, "#!/bin/sh\n[ \"$*\" = \"update --finish\" ] && printf 'Claude Code plugin: from the new binary\\nboard: up to date\\n'\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(finish_on_new_binary(&dir, Some(&dir)), ["Claude Code plugin: from the new binary", "board: up to date"]);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_claude(dir: &Path) -> (String, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let log = dir.join("claude.log");
        let bin = dir.join("claude");
        std::fs::write(&bin, format!("#!/bin/sh\necho \"$*\" >> '{}'\n", log.display())).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin.to_string_lossy().to_string(), log)
    }

    #[test]
    fn a_checkout_installed_with_from_builds_from_source_even_on_the_release_channel() {
        assert_eq!(build_mode(Channel::Release, false), Channel::Release);
        assert_eq!(build_mode(Channel::Release, true), Channel::Main);
        assert_eq!(build_mode(Channel::Main, false), Channel::Main);
    }

    #[test]
    fn update_skips_the_vault_plugin_when_it_is_not_installed() {
        let dir = scratch("no-vault");
        let (claude, log) = fake_claude(&dir);
        assert!(vault_plugin(&dir.join("claude-home"), &claude).is_empty());
        assert!(!log.exists(), "claude was not run");
    }

    #[test]
    fn update_refreshes_the_marketplace_then_updates_the_vault_plugin() {
        let dir = scratch("vault");
        let (claude, log) = fake_claude(&dir);
        let home = dir.join("claude-home");
        let install = home.join("plugins/cache/gronare/vault/0.2.16");
        std::fs::create_dir_all(install.join("hooks")).unwrap();
        std::fs::write(install.join("hooks/hooks.json"), r#"{"command":"uvx notes-vault-mcp==0.6.0 hook session-start"}"#).unwrap();
        std::fs::write(home.join("plugins/installed_plugins.json"), json!({ "plugins": { "vault@gronare": [{ "installPath": install, "version": "0.2.16" }] } }).to_string()).unwrap();
        assert!(vault_plugin(&home, &claude).is_empty());
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "plugin marketplace update gronare\nplugin update vault@gronare\n");
        assert_eq!(vault_plugin(&home, "false"), ["vault plugin: false plugin marketplace update gronare failed"]);
    }

    fn claude_home(dir: &Path, source: Value) -> PathBuf {
        let home = dir.join("claude-home");
        std::fs::create_dir_all(home.join("plugins")).unwrap();
        std::fs::write(home.join("plugins/installed_plugins.json"), json!({ "plugins": { "kelpie@kelpie": [{ "version": "0.20.0" }] } }).to_string()).unwrap();
        std::fs::write(home.join("plugins/known_marketplaces.json"), json!({ "kelpie": { "source": source, "installLocation": "/elsewhere" } }).to_string()).unwrap();
        home
    }

    #[test]
    fn the_claude_code_plugin_follows_github_only_on_the_main_channel() {
        let dir = scratch("claude-plugin");
        let (claude, log) = fake_claude(&dir);
        let home = claude_home(&dir, json!({ "source": "git", "url": "https://github.com/gronare/kelpie.git" }));
        assert_eq!(marketplace(&home, "kelpie@kelpie"), Marketplace::Git);
        claude_plugin(&home, &claude, None, true);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "plugin update kelpie@kelpie\n");
        let left = claude_plugin(&home, &claude, None, false);
        assert!(left.contains("left as it is") && left.contains(IN_PLACE_FIX), "{left}");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "plugin update kelpie@kelpie\n", "a release machine never pulls the plugin from main");
    }

    #[test]
    fn a_folder_marketplace_at_the_install_root_is_refreshed_and_its_cached_copy_updated() {
        let dir = scratch("in-place");
        let (claude, log) = fake_claude(&dir);
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let home = claude_home(&dir, json!({ "source": "directory", "path": root }));
        assert_eq!(marketplace(&home, "kelpie@kelpie"), Marketplace::InPlace(root.clone()));
        assert!(claude_plugin(&home, &claude, Some(&dir), false).contains("which is not the install root"));
        assert!(!log.exists(), "claude was not run for a folder that is not the install root");
        claude_plugin(&home, &claude, Some(&root), false);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "plugin marketplace update kelpie\nplugin update kelpie@kelpie\n", "Claude Code runs a cached copy, so the folder marketplace is refreshed and the plugin updated");
    }

    #[test]
    fn update_lists_each_component_before_and_after() {
        let component = |name: &str, installed: Option<&str>| Component { name: name.to_string(), installed: installed.map(str::to_string), ..Component::default() };
        let before = [component("kelpie", Some("0.18.1")), component("vault", Some("0.2.16")), component("notes-vault-mcp", Some("0.6.0"))];
        let after = [component("kelpie", Some("0.18.2")), component("vault", Some("0.2.16")), component("notes-vault-mcp", Some("0.6.1"))];
        assert_eq!(versions(&before, &after), ["kelpie 0.18.1 → 0.18.2", "vault plugin 0.2.16, already latest", "notes-vault-mcp 0.6.0 → 0.6.1"]);
        assert_eq!(versions(&before[..1], &after[..2]), ["kelpie 0.18.1 → 0.18.2", "vault plugin → 0.2.16"]);
        assert_eq!(versions(&[], &[component("kelpie", None)]), ["kelpie: version unknown"]);
        let built = |sha: &str| Component { built: Some(sha.to_string()), ..component("kelpie", Some("0.25.0")) };
        assert_eq!(versions(&[built("e70d5d5")], &[built("1234567")]), ["kelpie 0.25.0 rebuilt, e70d5d5 → 1234567"], "a rebuild within one version names both commits");
        assert_eq!(versions(&[built("e70d5d5")], &[built("e70d5d5")]), ["kelpie 0.25.0, already latest"]);
    }


}
