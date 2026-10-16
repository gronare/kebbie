use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::herdr;
use crate::lead::{install_root, state_dir};
use crate::repo::{main_checkout, name_of};
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use crate::tasks::slots;

// A Claude that can be told things: a task in a repo (task@repo) or the lead.
pub struct Session {
    pub address: String,
    pub task: Option<String>,
    pub pane: Option<String>,
    pub status: Option<String>,
}

impl Session {
    pub fn state(&self) -> &str {
        match (&self.pane, self.status.as_deref()) {
            (None, _) => "no Claude",
            (Some(_), Some("blocked")) => "waiting for an answer",
            (Some(_), Some(status)) => status,
            (Some(_), None) => "unknown",
        }
    }
}

pub fn address(task: &str, repo: &str) -> String {
    format!("{task}@{repo}")
}

pub fn sessions() -> Result<Vec<Session>> {
    let workspaces = herdr::workspaces()?;
    let agents: Vec<Value> = herdr::agents()?.into_iter().filter(|agent| agent["agent"] == "claude").collect();
    let agent_in = |workspace: Option<&str>| workspace.and_then(|id| agents.iter().find(|agent| agent["workspace_id"].as_str() == Some(id)));
    let text = |value: &Value| value.as_str().map(str::to_string);
    let mut list = Vec::new();
    for workspace in workspaces.iter().filter(|workspace| workspace["label"] == LEAD_LABEL) {
        let agent = agent_in(workspace["workspace_id"].as_str());
        list.push(Session { address: LEAD_LABEL.to_string(), task: None, pane: agent.and_then(|agent| text(&agent["pane_id"])), status: agent.and_then(|agent| text(&agent["agent_status"])) });
    }
    let repos: BTreeSet<PathBuf> = workspaces.iter().filter_map(|workspace| workspace["worktree"]["repo_root"].as_str()).map(PathBuf::from).collect();
    for repo in &repos {
        for slot in slots(repo)? {
            let Some(task) = slot.task else { continue };
            let workspace = workspaces.iter().find(|workspace| workspace["worktree"]["checkout_path"].as_str() == slot.path.to_str())
                .and_then(|workspace| workspace["workspace_id"].as_str());
            let agent = agent_in(workspace);
            list.push(Session {
                address: address(&task, &name_of(repo)),
                task: Some(task),
                pane: agent.and_then(|agent| text(&agent["pane_id"])),
                status: agent.and_then(|agent| text(&agent["agent_status"])),
            });
        }
    }
    Ok(list)
}

// Who runs this: the task whose worktree holds cwd, or the lead by its herdr Space.
pub fn me(cwd: &Path) -> Option<String> {
    if let Ok(repo) = main_checkout(cwd)
        && let Some(task) = slots(&repo).ok()?.into_iter().find(|slot| slot.task.is_some() && cwd.starts_with(&slot.path)).and_then(|slot| slot.task)
    {
        return Some(address(&task, &name_of(&repo)));
    }
    let workspace = std::env::var("HERDR_WORKSPACE_ID").ok()?;
    herdr::workspaces().ok()?.iter()
        .any(|space| space["workspace_id"] == workspace.as_str() && space["label"] == LEAD_LABEL)
        .then(|| LEAD_LABEL.to_string())
}

pub fn list(me: Option<&str>, sessions: &[Session]) -> String {
    let task = me.and_then(|me| me.split_once('@')).map(|(task, _)| task);
    let line = |session: &Session| format!("  {:<36} {}", session.address, session.state());
    let others = |same: bool| -> Vec<String> {
        sessions.iter()
            .filter(|session| session.task.is_some() && Some(session.address.as_str()) != me && (session.task.as_deref() == task) == same)
            .map(line).collect()
    };
    let mut lines = vec![match me {
        Some(me) => format!("You are {me}."),
        None => "You are not in a task's worktree or the lead.".to_string(),
    }];
    let (same, other) = (others(true), others(false));
    if !same.is_empty() {
        lines.push("Same task, other repos:".to_string());
        lines.extend(same);
    }
    lines.push("Other tasks:".to_string());
    lines.extend(if other.is_empty() { vec!["  none".to_string()] } else { other });
    if me != Some(LEAD_LABEL) {
        lines.extend(sessions.iter().filter(|session| session.address == LEAD_LABEL).map(line));
    }
    lines.join("\n")
}

pub fn peers(cwd: &Path) -> Result<String> {
    Ok(list(me(cwd).as_deref(), &sessions()?))
}

// The stable link when it leads to this binary, so a task's settings survive reinstalls.
pub fn kelpie_command() -> Result<String> {
    let exe = std::env::current_exe().and_then(|path| path.canonicalize()).map_err(|error| error.to_string())?;
    let home = std::env::var("HOME").unwrap_or_default();
    let stable = PathBuf::from(home).join(".local/share/kelpie/current/target/release/kelpie");
    Ok(if stable.canonicalize().ok() == Some(exe.clone()) { stable } else { exe }.to_string_lossy().to_string())
}

// roles/task.md filled in for this task, written to the state dir for --append-system-prompt-file.
pub fn role_file(task: &str, repo: &str) -> Result<PathBuf> {
    let template = fs::read_to_string(install_root()?.join("roles/task.md")).map_err(|error| format!("roles/task.md: {error}"))?;
    let me = address(task, repo);
    let sessions = sessions()?;
    let listed = list(Some(&me), &sessions);
    let peers = listed.split_once('\n').map(|(_, rest)| rest).unwrap_or_default();
    let role = template.replace("{address}", &me).replace("{task}", task).replace("{repo}", repo)
        .replace("{kelpie}", &kelpie_command()?).replace("{peers}", peers);
    let dir = state_dir()?.join("roles");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let file = dir.join(format!("{me}.md"));
    fs::write(&file, role).map_err(|error| error.to_string())?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(address: &str, status: Option<&str>) -> Session {
        let task = address.split_once('@').map(|(task, _)| task.to_string());
        Session { address: address.to_string(), task, pane: status.map(|_| "w1:p1".to_string()), status: status.map(str::to_string) }
    }

    fn sessions() -> Vec<Session> {
        vec![session("lead", Some("idle")), session("posthog@auctionet", Some("working")), session("posthog@relay", Some("blocked")), session("turnstile@auctionet", None)]
    }

    #[test]
    fn the_list_puts_the_other_halves_first() {
        let listed = list(Some("posthog@auctionet"), &sessions());
        assert!(listed.starts_with("You are posthog@auctionet.\nSame task, other repos:\n  posthog@relay"), "{listed}");
        assert!(listed.contains("Other tasks:\n  turnstile@auctionet"));
        assert!(listed.lines().last().unwrap().starts_with("  lead"));
    }
}
