use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::{self, Channel};
use crate::herdr;
use crate::lead;
use crate::peers;
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
    let places = Places { root: root.clone(), claude_dir: claude_dir.to_path_buf(), channel };
    let before = update_check::installed(&places);
    let mut lines = vec![herdr_plugin(&plugin, &state, channel, request, visible)?];
    let running = release::read_override(&state);
    lines.push(claude_plugin(claude_dir, &setup::claude_bin(), root.as_deref(), channel == Channel::Main && running.is_none()));
    lines.extend(vault_plugin(claude_dir, &setup::claude_bin()));
    let mut summary = versions(&before, &update_check::installed(&places));
    summary.insert(0, match &running {
        Some(running) => format!("kelpie is {}", running.describe(channel)),
        None => format!("kelpie follows the {} channel", channel.key()),
    });
    update_check::forget(&state);
    lines.extend(reload()?);
    summary.push(String::new());
    summary.extend(lines);
    Ok(summary.join("\n"))
}

pub fn check(claude_dir: &Path) -> String {
    let places = Places { root: update_check::install_root(), claude_dir: claude_dir.to_path_buf(), channel: config::channel().unwrap_or_default() };
    let running = lead::state_location().ok().and_then(|state| release::read_override(&state)).map(|running| running.describe(places.channel));
    format!("channel {}\n\n{}", places.channel.key(), update_check::render(&update_check::check(&places), running.as_deref()))
}

fn versions(before: &[Component], after: &[Component]) -> Vec<String> {
    after.iter().map(|now| {
        let was = before.iter().find(|was| was.name == now.name).and_then(|was| was.installed.clone());
        match (was, &now.installed) {
            (Some(was), Some(now_version)) if &was == now_version => format!("{} {was}, already latest", now.label()),
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
    match release::install(&root, state, channel, request, plugin["source"]["kind"] != "local")? {
        Plan::Herdr => {
            step(&herdr::bin(), &["plugin", "update", "--yes", "kelpie"], None, visible)?;
            Ok("herdr plugin: updated from GitHub and rebuilt".to_string())
        }
        Plan::Done(line) => {
            step("cargo", &["build", "--release", "--quiet"], Some(&root), visible)?;
            Ok(format!("herdr plugin: {line}; built {}", root.display()))
        }
    }
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
        Marketplace::InPlace(path) if root.is_some_and(|root| same_folder(&path, root)) => format!("Claude Code plugin: loaded in place from {}, the same checkout", path.display()),
        Marketplace::InPlace(path) => format!("Claude Code plugin: loaded in place from {}, which is not the install root; {IN_PLACE_FIX}", path.display()),
        Marketplace::Git if follows_github => match sh::run(claude, &["plugin", "update", &id], None) {
            Ok(output) => format!("Claude Code plugin: {}", output.lines().last().unwrap_or("up to date").trim_start_matches(['✔', ' '])),
            Err(error) => format!("Claude Code plugin: {error}"),
        },
        Marketplace::Git => format!("Claude Code plugin: left as it is, since its GitHub marketplace follows the default branch and not this channel. Load it from the install root once (the remove forgets its options, such as auto_open): {IN_PLACE_FIX}"),
    }
}

#[derive(Debug, PartialEq)]
enum Reload {
    Now,
    Focused,
    Busy,
}

fn reload_for(agent: &Value) -> Option<Reload> {
    if agent["agent"] != "claude" {
        return None;
    }
    // Typing into the pane you are in would land in the middle of what you are writing.
    if agent["focused"] == true {
        return Some(Reload::Focused);
    }
    let waiting = ["idle", "done"].contains(&agent["agent_status"].as_str().unwrap_or_default());
    Some(if waiting && agent["interactive_ready"] == true { Reload::Now } else { Reload::Busy })
}

// The lead reads its role and settings only at start, so it is restarted instead, by the binary just built.
fn lead_line(reload: &Reload, restart: impl FnOnce() -> Result<String>) -> String {
    match reload {
        Reload::Now => match restart() {
            Ok(said) => format!("  lead: {said}"),
            Err(error) => format!("  lead: {error}"),
        },
        Reload::Focused | Reload::Busy => "  lead: restart the lead when it waits: kelpie lead --restart".to_string(),
    }
}

fn restart_lead() -> Result<String> {
    sh::run(&peers::kelpie_command()?, &["lead", "--restart"], None)
}

fn reload() -> Result<Vec<String>> {
    let mut lines = Vec::new();
    let lead = lead::lead_workspace(&herdr::workspaces()?);
    for agent in herdr::agents()? {
        let Some(reload) = reload_for(&agent) else { continue };
        if lead.is_some() && agent["workspace_id"].as_str() == lead.as_deref() {
            lines.push(lead_line(&reload, restart_lead));
            continue;
        }
        let pane = agent["pane_id"].as_str().unwrap_or_default();
        let name = agent["name"].as_str().or(agent["display_agent"].as_str()).unwrap_or(pane);
        lines.push(match reload {
            Reload::Now => match herdr::prompt_agent(pane, "/reload-plugins") {
                Ok(()) => format!("  {name}: reloaded"),
                Err(error) => format!("  {name}: {error}"),
            },
            Reload::Focused => format!("  {name}: you are in it, so type /reload-plugins there"),
            Reload::Busy => format!("  {name}: busy, type /reload-plugins there when it waits"),
        });
    }
    if lines.is_empty() {
        lines.push("  no Claude sessions running".to_string());
    }
    lines.insert(0, "Claude sessions:".to_string());
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn a_folder_marketplace_at_the_install_root_is_the_plugin_and_needs_no_command() {
        let dir = scratch("in-place");
        let (claude, log) = fake_claude(&dir);
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let home = claude_home(&dir, json!({ "source": "directory", "path": root }));
        assert_eq!(marketplace(&home, "kelpie@kelpie"), Marketplace::InPlace(root.clone()));
        for follows in [true, false] {
            assert_eq!(claude_plugin(&home, &claude, Some(&root), follows), format!("Claude Code plugin: loaded in place from {}, the same checkout", root.display()));
        }
        assert!(claude_plugin(&home, &claude, Some(&dir), false).contains("which is not the install root"));
        assert!(!log.exists(), "claude was not run");
    }

    #[test]
    fn update_lists_each_component_before_and_after() {
        let component = |name: &str, installed: Option<&str>| Component { name: name.to_string(), installed: installed.map(str::to_string), latest: None, error: None };
        let before = [component("kelpie", Some("0.18.1")), component("vault", Some("0.2.16")), component("notes-vault-mcp", Some("0.6.0"))];
        let after = [component("kelpie", Some("0.18.2")), component("vault", Some("0.2.16")), component("notes-vault-mcp", Some("0.6.1"))];
        assert_eq!(versions(&before, &after), ["kelpie 0.18.1 → 0.18.2", "vault plugin 0.2.16, already latest", "notes-vault-mcp 0.6.0 → 0.6.1"]);
        assert_eq!(versions(&before[..1], &after[..2]), ["kelpie 0.18.1 → 0.18.2", "vault plugin → 0.2.16"]);
        assert_eq!(versions(&[], &[component("kelpie", None)]), ["kelpie: version unknown"]);
    }

    #[test]
    fn reloads_only_a_waiting_claude_you_are_not_typing_in() {
        let agent = |status: &str, ready: bool, focused: bool| json!({ "agent": "claude", "agent_status": status, "interactive_ready": ready, "focused": focused });
        assert_eq!(reload_for(&agent("idle", true, false)), Some(Reload::Now));
        assert_eq!(reload_for(&agent("done", true, false)), Some(Reload::Now));
        assert_eq!(reload_for(&agent("idle", true, true)), Some(Reload::Focused));
        assert_eq!(reload_for(&agent("working", true, false)), Some(Reload::Busy));
        assert_eq!(reload_for(&agent("blocked", true, false)), Some(Reload::Busy));
        assert_eq!(reload_for(&agent("idle", false, false)), Some(Reload::Busy));
        assert_eq!(reload_for(&json!({ "agent": "codex", "agent_status": "idle" })), None);
    }

    #[test]
    fn update_restarts_a_waiting_lead_and_otherwise_says_how_to_restart_it() {
        let restarted = std::cell::Cell::new(0);
        let restart = || { restarted.set(restarted.get() + 1); Ok("Restarted the lead with its current role and settings, resuming its conversation.".to_string()) };
        assert_eq!(lead_line(&Reload::Now, restart), "  lead: Restarted the lead with its current role and settings, resuming its conversation.");
        assert_eq!(lead_line(&Reload::Now, || Err("The lead is working; run kelpie lead --restart when it is idle.".to_string())), "  lead: The lead is working; run kelpie lead --restart when it is idle.");
        for reload in [Reload::Focused, Reload::Busy] {
            assert_eq!(lead_line(&reload, restart), "  lead: restart the lead when it waits: kelpie lead --restart");
        }
        assert_eq!(restarted.get(), 1);
    }
}
