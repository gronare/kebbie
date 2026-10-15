use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::herdr;
use crate::repo::{ahead_behind, changed_count, default_branch, name_of};
use crate::sh::Result;
use crate::tasks::slots;

pub const LEAD_LABEL: &str = "lead";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    NeedsYou,
    Working,
    ReadyToPush,
    Idle,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::NeedsYou => "NEEDS YOU",
            Group::Working => "WORKING",
            Group::ReadyToPush => "READY TO PUSH",
            Group::Idle => "WAITING FOR WORK",
        }
    }
}

#[derive(Clone)]
pub struct Agent {
    pub pane: String,
    pub status: String,
    pub title: String,
    pub context: Option<String>,
}

#[derive(Clone)]
pub struct Git {
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
    pub changed: usize,
}

#[derive(Clone)]
pub struct Item {
    pub name: String,
    pub place: String,
    pub repo: Option<PathBuf>,
    pub path: Option<PathBuf>,
    pub workspace: Option<String>,
    pub agent: Option<Agent>,
    pub git: Option<Git>,
}

impl Item {
    pub fn is_task(&self) -> bool {
        self.git.is_some()
    }

    pub fn group(&self) -> Group {
        match self.agent.as_ref().map(|agent| agent.status.as_str()) {
            Some("blocked" | "done") => Group::NeedsYou,
            Some("working") => Group::Working,
            _ => match &self.git {
                Some(git) if git.ahead > 0 && git.behind == 0 && git.changed == 0 => Group::ReadyToPush,
                _ => Group::Idle,
            },
        }
    }

    pub fn summary(&self) -> String {
        let Some(git) = &self.git else {
            return "Claude session outside a task worktree".to_string();
        };
        if self.group() == Group::ReadyToPush {
            return format!("{} · rebased, ready to push", plural(git.ahead, "commit"));
        }
        let mut parts = Vec::new();
        if git.ahead > 0 {
            parts.push(plural(git.ahead, "commit"));
        }
        if git.changed > 0 {
            parts.push(format!("{} changed", plural(git.changed, "file")));
        }
        if git.behind > 0 {
            parts.push(format!("{} behind {}", git.behind, git.upstream.trim_start_matches("origin/")));
        }
        if parts.is_empty() { "nothing yet".to_string() } else { parts.join(" · ") }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "place": self.place,
            "kind": if self.is_task() { "task" } else { "session" },
            "group": self.group().title(),
            "summary": self.summary(),
            "repo": self.repo,
            "path": self.path,
            "workspace": self.workspace,
            "agent": self.agent.as_ref().map(|agent| json!({
                "pane": agent.pane, "status": agent.status, "title": agent.title, "context": agent.context,
            })),
            "git": self.git.as_ref().map(|git| json!({
                "upstream": git.upstream, "ahead": git.ahead, "behind": git.behind, "changed": git.changed,
            })),
        })
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

fn agent_from(value: &Value) -> Agent {
    let title = value["display_agent"].as_str().or(value["terminal_title_stripped"].as_str()).unwrap_or("Claude");
    Agent {
        pane: value["pane_id"].as_str().unwrap_or_default().to_string(),
        status: value["agent_status"].as_str().unwrap_or("unknown").to_string(),
        title: title.to_string(),
        context: value["tokens"]["ctx"].as_str().map(str::to_string),
    }
}

// Every task in a pooled worktree of a repo open in herdr, then every other Claude session, sorted by what they need.
pub fn gather() -> Result<Vec<Item>> {
    let workspaces = herdr::workspaces()?;
    let agents: Vec<Value> = herdr::agents()?.into_iter().filter(|agent| agent["agent"] == "claude").collect();
    let label_of = |id: &str| {
        workspaces.iter().find(|workspace| workspace["workspace_id"] == id)
            .and_then(|workspace| workspace["label"].as_str()).unwrap_or(id).to_string()
    };
    let workspace_at = |path: &PathBuf| {
        workspaces.iter().find(|workspace| workspace["worktree"]["checkout_path"].as_str() == path.to_str())
            .and_then(|workspace| workspace["workspace_id"].as_str()).map(str::to_string)
    };
    let repos: BTreeSet<PathBuf> = workspaces.iter()
        .filter_map(|workspace| workspace["worktree"]["repo_root"].as_str()).map(PathBuf::from).collect();

    let mut items = Vec::new();
    let mut task_workspaces = BTreeSet::new();
    for repo in &repos {
        let Ok(upstream) = default_branch(repo) else { continue };
        for slot in slots(repo)? {
            let Some(task) = slot.task else { continue };
            let workspace = workspace_at(&slot.path);
            if let Some(id) = &workspace {
                task_workspaces.insert(id.clone());
            }
            let agent = workspace.as_ref()
                .and_then(|id| agents.iter().find(|agent| agent["workspace_id"].as_str() == Some(id.as_str())))
                .map(agent_from);
            let (ahead, behind) = ahead_behind(&slot.path, &upstream)?;
            items.push(Item {
                name: task,
                place: name_of(repo),
                repo: Some(repo.clone()),
                path: Some(slot.path.clone()),
                workspace,
                agent,
                git: Some(Git { upstream: upstream.clone(), ahead, behind, changed: changed_count(&slot.path)? }),
            });
        }
    }
    for agent in &agents {
        let workspace = agent["workspace_id"].as_str().unwrap_or_default().to_string();
        let label = label_of(&workspace);
        if task_workspaces.contains(&workspace) || label == LEAD_LABEL {
            continue;
        }
        let agent = agent_from(agent);
        items.push(Item {
            name: agent.title.clone(),
            place: label,
            repo: None,
            path: None,
            workspace: Some(workspace),
            agent: Some(agent),
            git: None,
        });
    }
    items.sort_by_key(|item| (item.group(), !item.is_task()));
    Ok(items)
}
