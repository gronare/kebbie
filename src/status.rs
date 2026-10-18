use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::herdr;
use crate::messages::{self, Message};
use crate::peers;
use crate::repo::{ahead_behind, changed_count, default_branch, git, name_of, pooled_checkouts};
use crate::sh::Result;
use crate::tasks::{self, slots};

pub const LEAD_LABEL: &str = "lead";
pub const UNCHECKED: &str = "rules not checked by the lead";

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
    pub approved: bool,
}

#[derive(Clone)]
pub struct Item {
    pub review: Option<(usize, usize)>,
    pub server: bool,
    pub name: String,
    pub place: String,
    pub repo: Option<PathBuf>,
    pub path: Option<PathBuf>,
    pub workspace: Option<String>,
    pub agent: Option<Agent>,
    pub git: Option<Git>,
    pub report: Option<Message>,
}

impl Item {
    pub fn is_task(&self) -> bool {
        self.git.is_some()
    }

    fn rebased(&self) -> bool {
        self.git.as_ref().is_some_and(|git| git.ahead > 0 && git.behind == 0 && git.changed == 0)
    }

    // Ready to push takes the lead's check against the rules skill as well as a clean rebase.
    fn pushable(&self) -> bool {
        self.rebased() && self.git.as_ref().is_some_and(|git| git.approved)
    }

    pub fn unchecked(&self) -> bool {
        self.rebased() && !self.pushable() && self.group() != Group::Working
    }

    pub fn report_state(&self) -> Option<&str> {
        self.report.as_ref()?.report.as_ref().map(|report| report.state.as_str())
    }

    // Between turns the task's own report says what it needs; herdr's "done" only says nobody has looked yet.
    pub fn group(&self) -> Group {
        let status = self.agent.as_ref().map(|agent| agent.status.as_str());
        match (status, self.report_state()) {
            (Some("blocked"), _) => Group::NeedsYou,
            (Some("working"), _) | (_, Some("working")) => Group::Working,
            (_, Some("needs-you")) => Group::NeedsYou,
            (_, Some("ready")) => if self.pushable() { Group::ReadyToPush } else { Group::NeedsYou },
            (_, Some("blocked")) => Group::Idle,
            (Some("done"), _) => Group::NeedsYou,
            _ => if self.pushable() { Group::ReadyToPush } else { Group::Idle },
        }
    }

    // Within Needs you: a question on screen first, then what the task asks of you, then what is ready, then the rest.
    pub fn urgency(&self) -> u8 {
        if self.group() != Group::NeedsYou {
            return 0;
        }
        match (self.agent.as_ref().map(|agent| agent.status.as_str()), self.report_state()) {
            (Some("blocked"), _) => 0,
            (_, Some("needs-you")) => 1,
            (_, Some("ready")) => 2,
            _ => 3,
        }
    }

    // A turn under way has most likely taken up what the last report asked.
    pub fn next_for_you(&self) -> Option<&str> {
        if self.agent.as_ref().is_some_and(|agent| agent.status == "working") {
            return None;
        }
        self.report.as_ref()?.next_for_you()
    }

    pub fn summary(&self) -> String {
        let Some(git) = &self.git else {
            return "Claude session outside a task worktree".to_string();
        };
        let work = self.work(git);
        match (&self.workspace, &self.agent) {
            (None, None) => format!("no Space · {work}"),
            (Some(_), None) => format!("no Claude · {work}"),
            _ => work,
        }
    }

    fn work(&self, git: &Git) -> String {
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
        if self.unchecked() {
            parts.push(UNCHECKED.to_string());
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
            "review": self.review.map(|(read, total)| json!({ "read": read, "files": total })),
            "report": self.report.as_ref().and_then(|message| Some(json!({
                "state": message.report.as_ref()?.state, "next": message.report.as_ref()?.next, "summary": message.text, "age": messages::age(message.at),
            }))),
            "server": self.server,
            "git": self.git.as_ref().map(|git| json!({
                "upstream": git.upstream, "ahead": git.ahead, "behind": git.behind, "changed": git.changed, "approved": git.approved,
            })),
        })
    }
}

// Files the task changed since it left the default branch, and how many of them the review has marked read
// (marks are "path<TAB>content hash" lines, kept per worktree by scripts/review.sh).
fn review_progress(path: &Path, upstream: &str) -> Option<(usize, usize)> {
    let base = git(path, &["merge-base", "HEAD", upstream]).ok()?;
    let mut files: Vec<String> = git(path, &["diff", "--name-only", &base]).ok()?.lines().map(str::to_string).collect();
    files.extend(git(path, &["ls-files", "--others", "--exclude-standard"]).ok()?.lines().map(str::to_string));
    if files.is_empty() {
        return None;
    }
    let marks_dir = crate::lead::state_path(|name| std::env::var(name).ok()).ok()?;
    let marks = fs::read_to_string(marks_dir.join(path.to_string_lossy().replace('/', "_"))).unwrap_or_default();
    let existing: Vec<&String> = files.iter().filter(|file| path.join(file).exists()).collect();
    let mut args = vec!["hash-object", "--"];
    args.extend(existing.iter().map(|file| file.as_str()));
    let hashes = if existing.is_empty() { String::new() } else { git(path, &args).unwrap_or_default() };
    let read = existing.iter().zip(hashes.lines())
        .filter(|(file, hash)| marks.lines().any(|line| line == format!("{file}\t{hash}")))
        .count();
    Some((read, files.len()))
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

pub struct Checkout {
    pub name: String,
    pub changed: usize,
    pub free_slots: usize,
    pub busy_slots: usize,
}

impl Checkout {
    pub fn to_json(&self) -> Value {
        json!({ "repo": self.name, "changed": self.changed, "free_worktrees": self.free_slots, "busy_worktrees": self.busy_slots })
    }
}

// Repos open in herdr and repos with a worktree pool on disk, once each however their paths are spelled.
fn known_repos(workspaces: &[Value], pooled: Vec<PathBuf>) -> BTreeSet<PathBuf> {
    workspaces.iter().filter_map(|workspace| workspace["worktree"]["repo_root"].as_str()).map(PathBuf::from).chain(pooled)
        .map(|repo| repo.canonicalize().unwrap_or(repo)).collect()
}

// The main checkout of every repo kelpie knows: leftover uncommitted files there, and its worktree pool.
pub fn checkouts() -> Result<Vec<Checkout>> {
    Ok(known_repos(&herdr::workspaces()?, pooled_checkouts()).iter().filter_map(|repo| {
        let pool = slots(repo).ok()?;
        Some(Checkout {
            name: name_of(repo),
            changed: changed_count(repo).ok()?,
            free_slots: pool.iter().filter(|slot| slot.free).count(),
            busy_slots: pool.iter().filter(|slot| !slot.free).count(),
        })
    }).collect())
}

// Every task in a pooled worktree of a repo kelpie knows, then every other Claude session, sorted by what they need.
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
    let repos = known_repos(&workspaces, pooled_checkouts());

    let reports = messages::latest_reports(&messages::load());
    let mut items = Vec::new();
    let mut task_workspaces = BTreeSet::new();
    for repo in &repos {
        let Ok(upstream) = default_branch(repo) else { continue };
        let Ok(pool) = slots(repo) else { continue };
        for slot in pool {
            let Some(task) = slot.task else { continue };
            let workspace = workspace_at(&slot.path);
            if let Some(id) = &workspace {
                task_workspaces.insert(id.clone());
            }
            let agent = workspace.as_ref()
                .and_then(|id| agents.iter().find(|agent| agent["workspace_id"].as_str() == Some(id.as_str())))
                .map(agent_from);
            let (ahead, behind) = ahead_behind(&slot.path, &upstream)?;
            let changed = changed_count(&slot.path)?;
            let address = peers::address(&task, &name_of(repo));
            let approved = ahead > 0 && behind == 0 && changed == 0 && tasks::approved(&slot.path, &upstream, &address);
            let tokens = workspace.as_ref().and_then(|id| herdr::workspace_tokens(id).ok()).unwrap_or_default();
            items.push(Item {
                review: review_progress(&slot.path, &upstream),
                server: tokens["server"].is_string(),
                place: name_of(repo),
                repo: Some(repo.clone()),
                path: Some(slot.path.clone()),
                workspace,
                agent,
                git: Some(Git { upstream: upstream.clone(), ahead, behind, changed, approved }),
                report: reports.get(&address).cloned(),
                name: task,
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
            review: None,
            server: false,
            name: agent.title.clone(),
            place: label,
            repo: None,
            path: None,
            workspace: Some(workspace),
            agent: Some(agent),
            git: None,
            report: None,
        });
    }
    // A task's halves in different repos sit together within a group, after what needs you most.
    items.sort_by(|one, other| (!one.is_task(), one.group(), one.urgency(), &one.name).cmp(&(!other.is_task(), other.group(), other.urgency(), &other.name)));
    Ok(items)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::messages::Report;

    pub(crate) fn task(status: Option<&str>, report: Option<&str>, ahead: usize) -> Item {
        Item {
            review: None, server: false, name: "billing".to_string(), place: "web".to_string(), repo: None, path: None, workspace: None,
            agent: status.map(|status| Agent { pane: "w1:p1".to_string(), status: status.to_string(), title: String::new(), context: None }),
            git: Some(Git { upstream: "origin/main".to_string(), ahead, behind: 0, changed: 0, approved: true }),
            report: report.map(|state| Message {
                id: "1".to_string(), at: 0, from: "billing@web".to_string(), to: LEAD_LABEL.to_string(), text: "plan written".to_string(), held: false,
                report: Some(Report { state: state.to_string(), next: "nothing".to_string() }), ended: false,
            }),
        }
    }

    #[test]
    fn a_repo_counts_once_whether_herdr_has_it_open_or_only_its_pool_is_on_disk() {
        let dir = std::env::temp_dir().join(format!("kelpie-known-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("shop")).unwrap();
        std::fs::create_dir_all(dir.join("blog")).unwrap();
        let shop = dir.join("shop").canonicalize().unwrap();
        let workspaces = [json!({ "worktree": { "repo_root": dir.join("shop") } }), json!({ "label": "lead" })];
        let known = known_repos(&workspaces, vec![dir.join("./shop"), dir.join("blog")]);
        assert_eq!(known.into_iter().collect::<Vec<_>>(), [dir.join("blog").canonicalize().unwrap(), shop]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_task_without_its_space_or_its_claude_says_so() {
        let mut item = task(None, None, 1);
        item.git.as_mut().unwrap().changed = 2;
        assert_eq!(item.summary(), "no Space · 1 commit · 2 files changed");
        item.workspace = Some("w1".to_string());
        assert_eq!(item.summary(), "no Claude · 1 commit · 2 files changed");
        item.agent = task(Some("idle"), None, 0).agent;
        assert_eq!(item.summary(), "1 commit · 2 files changed");
        assert_eq!(task(None, None, 0).summary(), "no Space · nothing yet");
    }

    #[test]
    fn between_turns_the_report_decides_the_group() {
        assert!(task(Some("idle"), Some("needs-you"), 0).group() == Group::NeedsYou);
        assert!(task(Some("idle"), Some("ready"), 0).group() == Group::NeedsYou);
        assert!(task(Some("done"), Some("ready"), 2).group() == Group::ReadyToPush);
        assert!(task(Some("done"), Some("working"), 0).group() == Group::Working);
        assert!(task(Some("done"), Some("blocked"), 0).group() == Group::Idle);
        assert!(task(None, Some("needs-you"), 0).group() == Group::NeedsYou);
    }

    #[test]
    fn a_task_the_lead_has_not_approved_is_not_ready_to_push() {
        let mut ready = task(Some("idle"), Some("ready"), 2);
        ready.git.as_mut().unwrap().approved = false;
        assert!(ready.group() == Group::NeedsYou);
        assert!(ready.summary().contains("rules not checked by the lead"), "{}", ready.summary());
        let mut quiet = task(Some("idle"), None, 2);
        quiet.git.as_mut().unwrap().approved = false;
        assert!(quiet.group() == Group::Idle);
        let mut working = task(Some("working"), Some("ready"), 2);
        working.git.as_mut().unwrap().approved = false;
        assert!(!working.summary().contains("rules not checked"), "{}", working.summary());
        let approved = task(Some("idle"), Some("ready"), 2);
        assert!(approved.group() == Group::ReadyToPush && !approved.summary().contains("rules not checked"));
        let mut behind = task(Some("idle"), Some("ready"), 2);
        behind.git.as_mut().unwrap().behind = 1;
        assert!(behind.group() == Group::NeedsYou);
    }

    #[test]
    fn a_turn_under_way_or_a_question_on_screen_outranks_the_report() {
        assert!(task(Some("working"), Some("needs-you"), 0).group() == Group::Working);
        assert!(task(Some("blocked"), Some("working"), 0).group() == Group::NeedsYou);
    }

    #[test]
    fn the_next_step_is_hidden_while_a_turn_is_under_way() {
        let mut item = task(Some("idle"), Some("needs-you"), 0);
        item.report.as_mut().unwrap().report.as_mut().unwrap().next = "answer: which queue?".to_string();
        assert_eq!(item.next_for_you(), Some("answer: which queue?"));
        item.agent.as_mut().unwrap().status = "working".to_string();
        assert_eq!(item.next_for_you(), None);
    }

    #[test]
    fn without_a_report_herdr_decides_as_before() {
        assert!(task(Some("done"), None, 0).group() == Group::NeedsYou);
        assert!(task(Some("idle"), None, 1).group() == Group::ReadyToPush);
        assert!(task(Some("idle"), None, 0).group() == Group::Idle);
    }

    #[test]
    fn needs_you_puts_questions_then_asks_then_ready_first() {
        let urgency = |status, report| task(Some(status), report, 0).urgency();
        assert_eq!([urgency("blocked", None), urgency("idle", Some("needs-you")), urgency("idle", Some("ready")), urgency("done", None)], [0, 1, 2, 3]);
    }
}
