use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::herdr;
use crate::messages::{self, Message};
use crate::peers;
use crate::repo::{ahead_behind, changed_count, default_branch, git, name_of, pooled_checkouts, uncommitted};
use crate::sh::Result;
use crate::tasks::{self, slots};

pub const LEAD_LABEL: &str = "lead";
pub const UNCHECKED: &str = "rules not checked by the lead";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    NeedsYou,
    Working,
    ToReview,
    ReadyToPush,
    CiRunning,
    Idle,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::NeedsYou => "NEEDS YOU",
            Group::Working => "WORKING",
            Group::ToReview => "TO REVIEW",
            Group::ReadyToPush => "READY TO PUSH",
            Group::CiRunning => "CI RUNNING",
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
    pub uncommitted: usize,
    pub approved: bool,
}

#[derive(Clone)]
pub struct Item {
    pub review: Option<(usize, usize)>,
    pub verdict: Option<crate::review::Shown>,
    pub permit: Option<crate::permit::Permit>,
    pub follows: bool,
    pub server: bool,
    pub worker: bool,
    pub url: Option<String>,
    pub name: String,
    pub place: String,
    pub repo: Option<PathBuf>,
    pub path: Option<PathBuf>,
    pub workspace: Option<String>,
    pub agent: Option<Agent>,
    pub git: Option<Git>,
    pub report: Option<Message>,
    pub question: Option<Message>,
    pub ci: Option<crate::ci::Watch>,
    pub recover: Option<crate::recover::Record>,
    pub routine: Option<String>,
    pub push_waits: Option<tasks::PushBy>,
}

impl Item {
    pub fn is_task(&self) -> bool {
        self.git.is_some()
    }

    // Commits and no tracked file left uncommitted; how far behind the default branch it is plays no part, push rebases first.
    fn committed(&self) -> bool {
        self.git.as_ref().is_some_and(|git| git.ahead > 0 && git.uncommitted == 0)
    }

    fn pushable(&self) -> bool {
        self.committed() && self.git.as_ref().is_some_and(|git| git.approved)
    }

    pub fn unchecked(&self) -> bool {
        self.git.as_ref().is_some_and(|git| git.ahead > 0 && !git.approved) && self.group() != Group::Working
    }

    // Sent to review and not passed yet: it waits under To review, and push waits with it.
    pub fn awaiting_review(&self) -> bool {
        self.git.as_ref().is_some_and(|git| git.ahead > 0) && self.verdict.as_ref().is_some_and(crate::review::Shown::pending)
    }

    pub fn review_note(&self) -> Option<String> {
        let shown = self.verdict.as_ref()?;
        let verdict = shown.verdict.as_ref();
        Some(match verdict {
            _ if shown.running => "review running".to_string(),
            Some(_) if shown.stale => "review stale".to_string(),
            Some(verdict) if verdict.ok => "reviewed".to_string(),
            Some(verdict) => format!("review: {}", verdict.summary()),
            None => return None,
        })
    }

    pub fn report_state(&self) -> Option<&str> {
        self.report.as_ref()?.report.as_ref().map(|report| report.state.as_str())
    }

    // A report the task wrote after CI last changed outranks the watch.
    pub fn ci_group(&self) -> Option<Group> {
        let ci = self.ci.as_ref()?;
        if self.report.as_ref().is_some_and(|report| report.at > ci.since()) {
            return None;
        }
        match ci.state {
            crate::ci::State::Failed { .. } => Some(Group::NeedsYou),
            crate::ci::State::Running if ci.close => Some(Group::CiRunning),
            crate::ci::State::Running => None,
        }
    }

    pub fn ci_note(&self) -> Option<String> {
        self.ci.as_ref().map(crate::ci::Watch::note)
    }

    fn recovering(&self) -> Option<&crate::recover::Record> {
        let record = self.recover.as_ref()?;
        let newer_report = self.report.as_ref().is_some_and(|report| report.at > record.error_at);
        let working = self.agent.as_ref().is_some_and(|agent| agent.status == "working");
        (!newer_report && !working).then_some(record)
    }

    // kelpie resuming the task keeps it under Working; an error it will not retry puts it under Needs you.
    pub fn recover_group(&self) -> Option<Group> {
        self.recovering().map(|record| if record.kind == crate::recover::Kind::Stuck { Group::NeedsYou } else { Group::Working })
    }

    pub fn recover_note(&self) -> Option<String> {
        self.recovering().map(crate::recover::Record::note)
    }

    pub fn push_note(&self) -> Option<String> {
        let who = match self.push_waits? {
            tasks::PushBy::User => "you",
            tasks::PushBy::Lead => "lead",
        };
        Some(format!("push waits for tests · {who}"))
    }

    // Between turns the task's own report says what it needs; herdr's "done" only says nobody has looked yet.
    pub fn group(&self) -> Group {
        let status = self.agent.as_ref().map(|agent| agent.status.as_str());
        match (status, self.report_state(), self.ci_group().or_else(|| self.recover_group())) {
            (Some("blocked"), _, _) => Group::NeedsYou,
            (Some("working"), _, _) | (_, Some("working"), _) => Group::Working,
            (_, _, Some(group)) => group,
            (_, Some("needs-you"), _) => Group::NeedsYou,
            (_, Some("blocked"), _) => Group::Idle,
            _ if self.awaiting_review() => Group::ToReview,
            (_, Some("ready"), _) => if self.pushable() { Group::ReadyToPush } else { Group::NeedsYou },
            (Some("done"), _, _) => Group::NeedsYou,
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
            _ if self.ci_group().is_some() || self.recover_group().is_some() => 1,
            (_, Some("needs-you")) => 1,
            (_, Some("ready")) => 2,
            _ => 3,
        }
    }

    // A turn under way has most likely taken up what the last report asked.
    pub fn next_for_you(&self) -> Option<&str> {
        if self.agent.as_ref().is_some_and(|agent| agent.status == "working") || self.ci_group().is_some() || self.recover_group().is_some() {
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
            let review = self.review_note().map(|note| format!(" · {note}")).unwrap_or_default();
            let behind = if git.behind > 0 { format!(" · {} behind {}", git.behind, git.upstream.trim_start_matches("origin/")) } else { String::new() };
            return format!("{} · ready to push{behind}{review}", plural(git.ahead, "commit"));
        }
        let mut parts: Vec<String> = self.ci_note().into_iter().chain(self.recover_note()).chain(self.push_note()).collect();
        if git.ahead > 0 {
            parts.push(plural(git.ahead, "commit"));
        }
        if git.uncommitted > 0 {
            parts.push(plural(git.uncommitted, "uncommitted file"));
        }
        if git.changed > git.uncommitted {
            parts.push(plural(git.changed - git.uncommitted, "untracked file"));
        }
        if git.behind > 0 {
            parts.push(format!("{} behind {}", git.behind, git.upstream.trim_start_matches("origin/")));
        }
        if self.unchecked() {
            parts.push(UNCHECKED.to_string());
        }
        parts.extend(self.review_note());
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
            "verdict": self.verdict.as_ref().map(crate::review::Shown::to_json),
            "follows_main": self.follows,
            "push_permission": self.permit.map(|permit| match permit { crate::permit::Permit::Task => "task", crate::permit::Permit::Repo => "repo, once reviewed" }),
            "report": self.report.as_ref().and_then(|message| Some(json!({
                "state": message.report.as_ref()?.state, "next": message.report.as_ref()?.next, "summary": message.text, "age": messages::age(message.at),
            }))),
            "question": self.question.as_ref().and_then(|message| {
                let question = message.question.as_ref()?;
                let options: Vec<Value> = question.options.iter().map(|(label, meaning)| json!({ "label": label, "meaning": meaning })).collect();
                Some(json!({ "text": message.text, "options": options, "recommend": question.recommend, "plan": question.plan, "kept_for_user": question.waits, "age": messages::age(message.at) }))
            }),
            "ci": self.ci.as_ref().map(crate::ci::Watch::to_json),
            "recover": self.recover.as_ref().map(|record| { let mut shown = record.to_json(); shown["note"] = json!(record.note()); shown }),
            "routine": self.routine,
            "push_waits": self.push_waits.map(tasks::PushBy::key),
            "server": self.server,
            "url": self.url,
            "worker": self.worker,
            "git": self.git.as_ref().map(|git| json!({
                "upstream": git.upstream, "ahead": git.ahead, "behind": git.behind, "changed": git.changed, "uncommitted": git.uncommitted, "approved": git.approved,
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
    let marks_dir = crate::lead::state_location().ok()?;
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
pub fn running_url(repo: &Path, worktree: &Path) -> Option<String> {
    let settings = crate::config::resolve(Some(repo)).ok()?.settings;
    crate::config::dev_url(settings.dev_url.value.as_deref(), repo, worktree)
}

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

    let log = messages::load();
    let state = crate::lead::state_dir().ok();
    let reports = messages::latest_reports(&log);
    let questions = crate::questions::open_now(&log);
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
            let uncommitted = uncommitted(&slot.path)?.len();
            let address = peers::address(&task, &name_of(repo));
            let approved = ahead > 0 && tasks::approved(&slot.path, &upstream, &address);
            let tokens = workspace.as_ref().and_then(|id| herdr::workspace_tokens(id).ok()).unwrap_or_default();
            let server = tokens["server"].is_string();
            items.push(Item {
                review: review_progress(&slot.path, &upstream),
                verdict: crate::review::shown(&address, &slot.path, &upstream),
                permit: crate::permit::of(&address, &name_of(repo)),
                follows: tasks::follows(&address),
                server,
                worker: tokens["worker"].is_string(),
                url: if server { running_url(repo, &slot.path) } else { None },
                place: name_of(repo),
                repo: Some(repo.clone()),
                path: Some(slot.path.clone()),
                workspace,
                agent,
                git: Some(Git { upstream: upstream.clone(), ahead, behind, changed, uncommitted, approved }),
                report: reports.get(&address).cloned(),
                question: questions.iter().find(|question| question.from == address).map(|question| (*question).clone()),
                ci: state.as_deref().and_then(|state| crate::ci::load(state, &address)),
                recover: state.as_deref().and_then(|state| crate::recover::load(state, &address)),
                routine: state.as_deref().and_then(|state| crate::routines::run_of(state, &address)).map(|(id, _)| id),
                push_waits: state.as_deref().and_then(|state| tasks::pending_push(state, &address)),
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
            verdict: None,
            permit: None,
            follows: false,
            server: false,
            worker: false,
            url: None,
            name: agent.title.clone(),
            place: label,
            repo: None,
            path: None,
            workspace: Some(workspace),
            agent: Some(agent),
            git: None,
            report: None,
            question: None,
            ci: None,
            recover: None,
            routine: None,
            push_waits: None,
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
            review: None, verdict: None, permit: None, follows: false, server: false, worker: false, url: None, name: "billing".to_string(), place: "web".to_string(), repo: None, path: None, workspace: None,
            agent: status.map(|status| Agent { pane: "w1:p1".to_string(), status: status.to_string(), title: String::new(), context: None }),
            git: Some(Git { upstream: "origin/main".to_string(), ahead, behind: 0, changed: 0, uncommitted: 0, approved: true }),
            report: report.map(|state| Message {
                id: "1".to_string(), at: 0, from: "billing@web".to_string(), to: LEAD_LABEL.to_string(), text: "plan written".to_string(), held: false,
                report: Some(Report { state: state.to_string(), next: "nothing".to_string() }), ended: false, question: None, about: None, kind: None,
            }),
            question: None,
            ci: None,
            recover: None,
            routine: None,
            push_waits: None,
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
        item.git.as_mut().unwrap().changed = 3;
        item.git.as_mut().unwrap().uncommitted = 2;
        assert_eq!(item.summary(), "no Space · 1 commit · 2 uncommitted files · 1 untracked file");
        item.workspace = Some("w1".to_string());
        assert_eq!(item.summary(), "no Claude · 1 commit · 2 uncommitted files · 1 untracked file");
        item.agent = task(Some("idle"), None, 0).agent;
        assert_eq!(item.summary(), "1 commit · 2 uncommitted files · 1 untracked file");
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
    }

    #[test]
    fn being_behind_the_default_branch_never_keeps_a_task_out_of_ready_to_push() {
        let mut behind = task(Some("idle"), Some("ready"), 2);
        behind.git.as_mut().unwrap().behind = 3;
        assert!(behind.group() == Group::ReadyToPush, "push rebases first, so behind is information only");
        assert_eq!(behind.summary(), "2 commits · ready to push · 3 behind main");
        assert!(!behind.unchecked());
        let mut quiet = task(Some("idle"), None, 2);
        quiet.git.as_mut().unwrap().behind = 3;
        assert!(quiet.group() == Group::ReadyToPush);
    }

    #[test]
    fn untracked_files_leave_a_task_ready_and_uncommitted_ones_keep_it_out_and_say_so() {
        let mut untracked = task(Some("idle"), Some("ready"), 2);
        untracked.git.as_mut().unwrap().changed = 1;
        assert!(untracked.group() == Group::ReadyToPush, "an untracked file is no unfinished commit");
        let mut dirty = task(Some("idle"), Some("ready"), 2);
        dirty.git.as_mut().unwrap().changed = 2;
        dirty.git.as_mut().unwrap().uncommitted = 2;
        assert!(dirty.group() == Group::NeedsYou);
        assert!(dirty.summary().contains("2 uncommitted files") && !dirty.summary().contains(UNCHECKED), "the approval holds; the row names what keeps it from push: {}", dirty.summary());
    }

    pub(crate) fn in_review(ok: bool, findings: usize, stale: bool, running: bool) -> crate::review::Shown {
        crate::review::Shown { verdict: Some(crate::review::Verdict { ok, sha: "abc".to_string(), findings }), stale, running, file: PathBuf::from("/state/verdicts/billing@web.md") }
    }

    #[test]
    fn a_task_sent_to_review_waits_under_to_review_until_its_reviewer_passes_its_code() {
        let reviewed = |shown: crate::review::Shown| { let mut item = task(Some("idle"), Some("ready"), 2); item.verdict = Some(shown); item };
        let running = reviewed(crate::review::Shown { verdict: None, stale: false, running: true, file: PathBuf::new() });
        assert!(running.group() == Group::ToReview);
        assert_eq!(running.summary(), "2 commits · review running");
        let findings = reviewed(in_review(false, 2, false, false));
        assert!(findings.group() == Group::ToReview);
        assert!(findings.summary().ends_with("review: 2 findings"), "{}", findings.summary());
        let stale = reviewed(in_review(true, 0, true, false));
        assert!(stale.group() == Group::ToReview, "a pass on older code is not a pass");
        assert!(stale.summary().ends_with("review stale"));
        let passed = reviewed(in_review(true, 0, false, false));
        assert!(passed.group() == Group::ReadyToPush);
        assert_eq!(passed.summary(), "2 commits · ready to push · reviewed");
        assert!(!task(Some("idle"), Some("ready"), 2).awaiting_review(), "a task never sent to review goes straight to push");
    }

    #[test]
    fn reviewed_and_approved_are_separate_facts() {
        let mut item = task(Some("idle"), Some("ready"), 2);
        item.verdict = Some(in_review(true, 0, false, false));
        item.git.as_mut().unwrap().approved = false;
        assert!(item.group() == Group::NeedsYou, "the reviewer's pass does not stand in for the lead's rules check");
        assert!(item.summary().contains(UNCHECKED) && item.summary().contains("reviewed"), "{}", item.summary());
        let mut working = task(Some("working"), Some("ready"), 2);
        working.verdict = Some(in_review(false, 1, false, false));
        assert!(working.group() == Group::Working);
        let mut asks = task(Some("idle"), Some("needs-you"), 2);
        asks.verdict = Some(in_review(false, 1, false, false));
        assert!(asks.group() == Group::NeedsYou);
        let mut pushed = task(Some("idle"), Some("ready"), 0);
        pushed.verdict = Some(in_review(false, 1, true, false));
        assert!(!pushed.awaiting_review(), "nothing left to push, nothing left to review");
    }

    fn watched(state: crate::ci::State, close: bool) -> crate::ci::Watch {
        crate::ci::Watch {
            address: "billing@web".to_string(), task: "billing".to_string(), repo: PathBuf::new(), slug: "acme/web".to_string(), sha: "abcdef0".to_string(),
            started_at: 10, checked_at: None, close, runs: Vec::new(), state, error: None, pushed: crate::ci::Pushed::default(),
        }
    }

    #[test]
    fn a_pushed_task_waits_under_ci_running_and_a_failed_run_puts_it_under_needs_you_until_it_reports_again() {
        let mut item = task(Some("idle"), Some("ready"), 0);
        item.ci = Some(watched(crate::ci::State::Running, true));
        assert!(item.group() == Group::CiRunning);
        assert_eq!(item.summary(), "CI running: waiting for runs of abcdef0");
        assert_eq!(item.next_for_you(), None, "the push the old report asked for is done");
        item.ci = Some(watched(crate::ci::State::Failed { at: 20, what: "CI · test".to_string(), url: String::new() }, true));
        assert!(item.group() == Group::NeedsYou && item.urgency() == 1);
        assert_eq!(item.summary(), "CI failed: CI · test");
        item.agent.as_mut().unwrap().status = "working".to_string();
        assert!(item.group() == Group::Working, "its Claude is fixing it");
        item.agent.as_mut().unwrap().status = "idle".to_string();
        item.report.as_mut().unwrap().at = 30;
        assert!(item.group() == Group::NeedsYou && item.ci_group().is_none(), "a report after the failure decides again");
        let mut partial = task(Some("idle"), Some("working"), 1);
        partial.ci = Some(watched(crate::ci::State::Running, false));
        assert!(partial.group() == Group::Working, "a partial push leaves the task where it is");
        assert!(partial.summary().starts_with("CI running"));
    }

    fn stopped(kind: crate::recover::Kind, sent: bool) -> crate::recover::Record {
        crate::recover::Record {
            address: "billing@web".to_string(), kind, reason: "overload".to_string(), text: "API Error: Repeated 529 Overloaded errors.".to_string(), error_id: "e1".to_string(), error_at: 10,
            attempts: 1, next_at: Some(4_000_000_000), reset_at: Some(4_000_000_000), sent_for: sent.then(|| "e1".to_string()),
        }
    }

    #[test]
    fn a_task_kelpie_resumes_stays_under_working_and_one_it_will_not_resume_needs_you() {
        let mut item = task(Some("idle"), Some("needs-you"), 1);
        item.recover = Some(stopped(crate::recover::Kind::Retrying, false));
        assert!(item.group() == Group::Working);
        assert!(item.summary().starts_with("retrying after overload (2/4, next "), "{}", item.summary());
        assert_eq!(item.next_for_you(), None);
        item.recover = Some(stopped(crate::recover::Kind::Allowance, false));
        assert!(item.summary().starts_with("out of allowance until "), "{}", item.summary());
        item.recover = Some(stopped(crate::recover::Kind::Stuck, false));
        assert!(item.group() == Group::NeedsYou && item.urgency() == 1);
        assert!(item.summary().starts_with("stopped by overload: API Error"), "{}", item.summary());
        item.report.as_mut().unwrap().at = 20;
        assert!(item.recover_group().is_none() && item.recover_note().is_none(), "a report after the error decides again");
        let mut busy = task(Some("working"), None, 0);
        busy.recover = Some(stopped(crate::recover::Kind::Stuck, false));
        assert!(busy.group() == Group::Working && busy.recover_note().is_none(), "a turn under way is past the error");
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
    fn a_push_waiting_for_the_tasks_tests_says_who_asked_in_the_summary_and_the_json() {
        let mut item = task(Some("working"), None, 1);
        assert_eq!((item.to_json()["push_waits"].clone(), item.push_note()), (Value::Null, None));
        item.push_waits = Some(tasks::PushBy::Lead);
        assert_eq!((item.to_json()["push_waits"].clone(), item.summary()), (json!("lead"), "push waits for tests · lead · 1 commit".to_string()));
        item.push_waits = Some(tasks::PushBy::User);
        assert_eq!(item.to_json()["push_waits"], json!("user"));
    }

    #[test]
    fn the_json_carries_the_url_only_while_the_dev_server_runs() {
        let mut item = task(Some("idle"), None, 0);
        assert_eq!((item.to_json()["server"].clone(), item.to_json()["url"].clone(), item.to_json()["worker"].clone()), (json!(false), Value::Null, json!(false)));
        item.server = true;
        item.url = Some("https://wt1.app.dev".to_string());
        item.worker = true;
        let shown = item.to_json();
        assert_eq!((shown["server"].clone(), shown["url"].clone(), shown["worker"].clone()), (json!(true), json!("https://wt1.app.dev"), json!(true)));
    }

    #[test]
    fn needs_you_puts_questions_then_asks_then_ready_first() {
        let urgency = |status, report| task(Some(status), report, 0).urgency();
        assert_eq!([urgency("blocked", None), urgency("idle", Some("needs-you")), urgency("idle", Some("ready")), urgency("done", None)], [0, 1, 2, 3]);
    }

    #[test]
    fn an_agent_is_read_from_herdrs_record_with_defaults_for_what_is_missing() {
        let full = agent_from(&json!({ "pane_id": "p_1", "agent_status": "working", "display_agent": "✻ refunds", "terminal_title_stripped": "claude", "tokens": { "ctx": "41k" } }));
        assert_eq!((full.pane.as_str(), full.status.as_str(), full.title.as_str(), full.context.as_deref()), ("p_1", "working", "✻ refunds", Some("41k")));
        let bare = agent_from(&json!({}));
        assert_eq!((bare.pane.as_str(), bare.status.as_str(), bare.title.as_str(), bare.context), ("", "unknown", "Claude", None));
        let titled = agent_from(&json!({ "terminal_title_stripped": "wt1.app" }));
        assert_eq!(titled.title, "wt1.app");
    }

    #[test]
    fn a_session_outside_a_task_is_listed_after_the_tasks_with_no_git_facts() {
        let mut session = task(Some("working"), None, 0);
        session.git = None;
        assert!(!session.is_task());
        assert_eq!(session.summary(), "Claude session outside a task worktree");
        assert!(session.group() == Group::Working);
        assert_eq!(session.to_json()["kind"], "session");
        assert!(session.to_json()["git"].is_null() && session.to_json()["push_permission"].is_null());
        let checkout = Checkout { name: "app".to_string(), changed: 2, free_slots: 1, busy_slots: 3 };
        assert_eq!(checkout.to_json(), json!({ "repo": "app", "changed": 2, "free_worktrees": 1, "busy_worktrees": 3 }));
    }

    #[test]
    fn the_json_names_the_permission_and_the_question_the_user_has_to_settle() {
        let mut item = task(Some("idle"), Some("ready"), 2);
        item.permit = Some(crate::permit::Permit::Repo);
        assert_eq!(item.to_json()["push_permission"], "repo, once reviewed");
        item.permit = Some(crate::permit::Permit::Task);
        assert_eq!(item.to_json()["push_permission"], "task");
        item.question = Some(Message {
            id: "q".to_string(), at: 0, from: "billing@web".to_string(), to: "you".to_string(), text: "Which?".to_string(), held: false, report: None, ended: false,
            question: Some(crate::messages::Question { options: vec![("A".to_string(), "a".to_string())], recommend: 1, plan: None, plan_at: None, answered: false, waits: true }), about: None, kind: None,
        });
        let shown = item.to_json();
        assert_eq!(shown["question"]["kept_for_user"], true);
        assert_eq!(shown["question"]["options"][0]["label"], "A");
        assert_eq!(shown["report"]["summary"], "plan written");
    }
}
