use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::herdr;
use crate::sh::{self, Result};

pub fn run(claude_dir: &Path) -> Result<String> {
    let mut lines = vec![herdr_plugin()?, claude_plugin(claude_dir)];
    lines.extend(reload()?);
    Ok(lines.join("\n"))
}

fn herdr_plugin() -> Result<String> {
    let list: Value = serde_json::from_str(&sh::run(&herdr::bin(), &["plugin", "list", "--json"], None)?).map_err(|error| format!("herdr plugin list: {error}"))?;
    let plugin = list["result"]["plugins"].as_array().into_iter().flatten().find(|plugin| plugin["plugin_id"] == "kelpie")
        .ok_or("kelpie is not registered with herdr; kelpie doctor shows how")?;
    if plugin["source"]["kind"] != "local" {
        sh::run_visibly(&herdr::bin(), &["plugin", "update", "--yes", "kelpie"], None)?;
        return Ok("herdr plugin: updated from GitHub and rebuilt".to_string());
    }
    let root = PathBuf::from(plugin["plugin_root"].as_str().unwrap_or_default());
    let pulled = pull(&root);
    sh::run_visibly("cargo", &["build", "--release", "--quiet"], Some(&root))?;
    Ok(format!("herdr plugin: {pulled}; built {}", root.display()))
}

// A linked checkout may be where kelpie is being developed, so it only fast-forwards a clean tree.
fn pull(root: &Path) -> String {
    let git = |args: &[&str]| sh::run("git", args, Some(root));
    if !git(&["status", "--porcelain"]).unwrap_or_default().is_empty() {
        return "uncommitted changes in the linked checkout, not pulled".to_string();
    }
    if git(&["rev-parse", "--abbrev-ref", "@{upstream}"]).is_err() {
        return "the linked checkout has no upstream, not pulled".to_string();
    }
    match git(&["pull", "--ff-only", "--quiet"]) {
        Ok(_) => format!("pulled to {}", git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default()),
        Err(_) => "the linked checkout cannot fast-forward, not pulled".to_string(),
    }
}

fn claude_plugin(claude_dir: &Path) -> String {
    let installed: Value = std::fs::read_to_string(claude_dir.join("plugins/installed_plugins.json")).ok()
        .and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let Some(id) = installed["plugins"].as_object().and_then(|plugins| plugins.keys().find(|id| id.starts_with("kelpie@")).cloned()) else {
        return "Claude Code plugin: not installed; kelpie doctor shows how".to_string();
    };
    match sh::run("claude", &["plugin", "update", &id], None) {
        Ok(output) => format!("Claude Code plugin: {}", output.lines().last().unwrap_or("up to date").trim_start_matches(['✔', ' '])),
        Err(error) => format!("Claude Code plugin: {error}"),
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

fn reload() -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for agent in herdr::agents()? {
        let Some(reload) = reload_for(&agent) else { continue };
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
}
