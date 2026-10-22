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
pub const CHECKING: &str = "lead checking rules";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    NeedsYou,
    Working,
    ToReview,
    ReadyToPush,
    CiRunning,
    Idle,
    Queued,
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
            Group::Queued => "QUEUED",
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

pub const ANSWER_IN_SPACE: &str = "answer Claude's prompt in its Space or through the lead";

#[derive(Clone, Debug, PartialEq)]
pub struct Queued {
    pub after: Vec<(String, String)>,
    pub blocked: Option<String>,
    pub waiting: String,
    pub names: Vec<String>,
    pub base: String,
    pub at: Option<String>,
    pub backlog: bool,
}

#[derive(Clone)]
pub struct Item {
    pub review: Option<(usize, usize)>,
    pub verdict: Option<crate::review::Shown>,
    pub permit: Option<crate::permit::Permit>,
    pub follows: bool,
    pub bypass: bool,
    pub profile: Option<String>,
    pub server: bool,
    pub worker: bool,
    pub url: Option<String>,
    pub name: String,
    pub title: Option<String>,
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
    pub push_waits: Option<tasks::PendingPush>,
    pub queued: Option<Queued>,
    pub held_by_draft: Option<String>,
    pub starting: bool,
}

impl Item {
    pub fn is_task(&self) -> bool {
        self.git.is_some() || self.queued.is_some()
    }

    pub fn titled(&self) -> String {
        let shown = self.title.as_deref().unwrap_or(&self.name);
        match self.profile.as_deref() {
            Some(profile) if profile != crate::profiles::TASK => format!("{shown} · {profile}"),
            _ => shown.to_string(),
        }
    }

    fn queued_from(shown: tasks::WaitShown) -> Item {
        let tasks::WaitShown { wait, after, names, waiting, title, base, at } = shown;
        Item {
            review: None, verdict: None, permit: None, follows: false, bypass: false, profile: Some(wait.profile.clone().unwrap_or_else(|| crate::profiles::TASK.to_string())), server: false, worker: false, url: None,
            name: wait.task.clone(), title, place: name_of(&wait.repo), repo: Some(wait.repo.clone()), path: None, workspace: None, agent: None, git: None,
            report: None, question: None, ci: None, recover: None, routine: None, push_waits: None,
            queued: Some(Queued { after, blocked: wait.blocked.clone(), waiting, names, base, at, backlog: wait.backlog.is_some() }),
            held_by_draft: None, starting: false,
        }
    }

    // Commits and no tracked file left uncommitted; how far behind the default branch it is plays no part, push rebases first.
    fn committed(&self) -> bool {
        self.git.as_ref().is_some_and(|git| git.ahead > 0 && git.uncommitted == 0)
    }

    fn pushable(&self) -> bool {
        self.committed() && self.git.as_ref().is_some_and(|git| git.approved)
    }

    // A task that reported ready has asked the lead for its rules check; one that has not, has not.
    pub fn rules_note(&self) -> Option<&'static str> {
        let unchecked = self.git.as_ref().is_some_and(|git| git.ahead > 0 && !git.approved) && self.group() != Group::Working && self.push_waits.is_none();
        unchecked.then(|| if self.report_state() == Some("ready") { CHECKING } else { UNCHECKED })
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
        let pending = self.push_waits?;
        let who = match pending.by {
            tasks::PushBy::User => "you",
            tasks::PushBy::Lead => "the lead",
        };
        let what = match pending.work {
            _ if self.reported_since(pending.at, "ready") && self.git.as_ref().is_some_and(|git| git.approved) => "kelpie pushes it now",
            _ if self.reported_since(pending.at, "ready") => "the lead checks the new commits",
            tasks::PushWork::Tests => "its Claude runs the tests",
            tasks::PushWork::Conflict => "its Claude resolves the conflict",
            tasks::PushWork::Ci => "its Claude fixes the red CI",
        };
        Some(format!("pushing for {who} · {what}"))
    }

    fn reported_since(&self, at: u64, state: &str) -> bool {
        self.report.as_ref().is_some_and(|report| report.at > at && report.report.as_ref().is_some_and(|report| report.state == state))
    }

    // A push under way stays where it was pushed from; only the task's word that something broke, or a draft in the way, puts it under Needs you.
    fn push_group(&self) -> Option<Group> {
        let pending = self.push_waits?;
        Some(if self.held_by_draft.is_some() || self.reported_since(pending.at, "needs-you") { Group::NeedsYou } else { Group::ReadyToPush })
    }

    // A turn that has not touched the approved commits, such as reading a notice, leaves a finished task where it is; an edit, a new commit or a new report moves it.
    fn turn_leaves_it_ready(&self) -> bool {
        self.pushable() && !self.awaiting_review() && self.held_by_draft.is_none()
    }

    pub fn turn_under_way(&self) -> bool {
        self.agent.as_ref().is_some_and(|agent| agent.status == "working")
    }

    // Between turns the task's own report says what it needs; herdr's "done" only says nobody has looked yet.
    pub fn group(&self) -> Group {
        if let Some(queued) = &self.queued {
            return if queued.blocked.is_some() { Group::NeedsYou } else { Group::Queued };
        }
        let status = self.agent.as_ref().map(|agent| agent.status.as_str());
        if let Some(group) = self.push_group().filter(|_| status != Some("blocked")) {
            return group;
        }
        match (status, self.report_state(), self.ci_group().or_else(|| self.recover_group())) {
            (Some("blocked"), _, _) => Group::NeedsYou,
            (Some("working"), Some("ready"), None) if self.turn_leaves_it_ready() => Group::ReadyToPush,
            (Some("working"), _, _) | (_, Some("working"), _) => Group::Working,
            (_, _, Some(group)) => group,
            _ if self.held_by_draft.is_some() => Group::NeedsYou,
            _ if self.starting => Group::Working,
            (_, Some("needs-you"), _) => Group::NeedsYou,
            (_, Some("blocked"), _) => Group::Idle,
            _ if self.awaiting_review() => Group::ToReview,
            (_, Some("ready"), _) => if self.committed() { Group::ReadyToPush } else { Group::NeedsYou },
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
            _ if self.queued.is_some() => 1,
            (Some("blocked"), _) => 0,
            _ if self.ci_group().is_some() || self.recover_group().is_some() || self.held_by_draft.is_some() => 1,
            (_, Some("needs-you")) => 1,
            (_, Some("ready")) => 2,
            _ => 3,
        }
    }

    // A turn under way has most likely taken up what the last report asked.
    pub fn next_for_you(&self) -> Option<&str> {
        if let Some(queued) = &self.queued {
            return queued.blocked.as_ref().map(|_| "start it anyway or drop the wait");
        }
        if self.agent.as_ref().is_some_and(|agent| agent.status == "blocked") {
            return Some(ANSWER_IN_SPACE);
        }
        if self.agent.as_ref().is_some_and(|agent| agent.status == "working") || self.ci_group().is_some() || self.recover_group().is_some() || self.push_group() == Some(Group::ReadyToPush) {
            return None;
        }
        if let Some(held) = &self.held_by_draft {
            return Some(held);
        }
        self.report.as_ref()?.next_for_you()
    }

    // What the user is wanted for, in a few words; outside Needs you the group says it all.
    pub fn wanted(&self, go: &str) -> Option<String> {
        if self.group() != Group::NeedsYou {
            return None;
        }
        if let Some(queued) = &self.queued {
            return queued.blocked.as_ref().map(|blocked| format!("{blocked}: start it anyway or drop the wait"));
        }
        if self.agent.as_ref().is_some_and(|agent| agent.status == "blocked") {
            return Some(format!("answer Claude's prompt · {go} goes there"));
        }
        if self.ci_group().is_some() || self.recover_group().is_some() {
            return None;
        }
        if let Some(held) = &self.held_by_draft {
            return Some(held.clone());
        }
        if let Some(question) = self.question.as_ref().and_then(|message| message.question.as_ref()) {
            return Some(if question.plan.is_some() { "approve the plan".to_string() } else { format!("answer a question · {}", plural(question.options.len(), "option")) });
        }
        self.report.as_ref()?.next_for_you().map(str::to_string)
    }

    pub fn queued_note(&self) -> Option<String> {
        let queued = self.queued.as_ref()?;
        Some(match &queued.blocked {
            Some(blocked) => blocked.clone(),
            None => queued.waiting.clone(),
        })
    }

    // What waits on a queued row's second line; a blocked one says it under Needs you.
    pub fn waiting(&self) -> Option<&str> {
        self.queued.as_ref().filter(|queued| queued.blocked.is_none()).map(|queued| queued.waiting.as_str())
    }

    pub fn start_now_means(&self) -> Option<String> {
        let queued = self.queued.as_ref()?;
        Some(match (queued.names.is_empty(), &queued.at) {
            (true, Some(at)) => format!("from {} now instead of {at}", queued.base),
            (true, None) => format!("from {} now", queued.base),
            (false, _) => format!("from {}, without the changes from {}; its Claude is told what is missing", queued.base, tasks::and_list(&queued.names)),
        })
    }

    pub fn start_at_means(&self) -> Option<String> {
        let queued = self.queued.as_ref()?;
        let then = if queued.names.is_empty() { "it starts then" } else { "it starts then, once what it waits for is done too" };
        Some(match &queued.at {
            Some(at) => format!("now {at}; type another time and {then}"),
            None => format!("type a time and {then}"),
        })
    }

    pub fn drop_means(&self) -> Option<&'static str> {
        let queued = self.queued.as_ref()?;
        Some(if queued.backlog { "it never starts, and its backlog item goes back to the backlog" } else { "it never starts and leaves the board" })
    }

    pub fn summary(&self) -> String {
        if let Some(note) = self.queued_note() {
            return note;
        }
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
        if let (Group::ReadyToPush, Some(note)) = (self.group(), self.push_note()) {
            return if git.ahead == 0 { note } else { format!("{} · {note}", plural(git.ahead, "commit")) };
        }
        if self.group() == Group::ReadyToPush {
            let review: String = self.rules_note().map(str::to_string).into_iter().chain(self.review_note()).map(|note| format!(" · {note}")).collect();
            let behind = if git.behind > 0 { format!(" · {} behind {}", git.behind, git.upstream.trim_start_matches("origin/")) } else { String::new() };
            let turn = if self.turn_under_way() { " · its Claude is taking a turn" } else { "" };
            return format!("{} · ready to push{behind}{review}{turn}", plural(git.ahead, "commit"));
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
        parts.extend(self.rules_note().map(str::to_string));
        parts.extend(self.review_note());
        if parts.is_empty() { "nothing yet".to_string() } else { parts.join(" · ") }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "title": self.title.as_deref().unwrap_or(&self.name),
            "place": self.place,
            "kind": if self.queued.is_some() { "queued" } else if self.is_task() { "task" } else { "session" },
            "after": self.queued.as_ref().map(|queued| queued.after.iter().map(|(address, state)| json!({ "address": address, "state": state })).collect::<Vec<_>>()),
            "blocked": self.queued.as_ref().and_then(|queued| queued.blocked.clone()),
            "waiting": self.waiting(),
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
            "bypass_permissions": self.bypass,
            "profile": self.profile,
            "push_permission": self.permit.map(|permit| match permit { crate::permit::Permit::Task => "task", crate::permit::Permit::Repo => "repo, once reviewed" }),
            "report": self.report.as_ref().and_then(|message| Some(json!({
                "state": message.report.as_ref()?.state, "next": message.report.as_ref()?.next, "summary": message.text, "age": messages::age(message.at),
            }))),
            "question": self.question.as_ref().and_then(|message| {
                let question = message.question.as_ref()?;
                let options: Vec<Value> = question.options.iter().enumerate().map(|(index, (label, meaning))| json!({ "label": label, "meaning": meaning, "comment": question.comment.contains(&index) })).collect();
                Some(json!({ "text": message.text, "options": options, "recommend": question.recommend, "plan": question.plan, "kept_for_user": question.waits, "age": messages::age(message.at) }))
            }),
            "ci": self.ci.as_ref().map(crate::ci::Watch::to_json),
            "recover": self.recover.as_ref().map(|record| { let mut shown = record.to_json(); shown["note"] = json!(record.note()); shown }),
            "routine": self.routine,
            "push_waits": self.push_waits.map(|pending| pending.by.key()),
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
    let base = fork_point(path, upstream).ok()?;
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

const TITLES: &str = "titles";

pub(crate) fn kept_title(state: &Path, address: &str) -> Option<String> {
    fs::read_to_string(state.join(TITLES).join(address)).ok().map(|kept| kept.trim().to_string()).filter(|kept| !kept.is_empty())
}

pub(crate) fn keep_title(state: &Path, address: &str, title: &str) {
    if !title.trim().is_empty() {
        let _ = fs::create_dir_all(state.join(TITLES)).and_then(|_| fs::write(state.join(TITLES).join(address), title.trim()));
    }
}

pub(crate) fn forget_title(state: &Path, address: &str) {
    let _ = fs::remove_file(state.join(TITLES).join(address));
}

// Claude names its session after the first prompt; kept per task so the title outlives the session. A default title, or none yet, shows the task's name.
pub(crate) fn title_of(state: &Path, address: &str, seen: Option<&str>, repo: &str, task: &str, folder: &str) -> Option<String> {
    let file = state.join(TITLES).join(address);
    let seen = seen.map(|title| readable(title, repo)).filter(|title| !title.is_empty() && !["Claude", "Claude Code", task, folder].iter().any(|default| title.eq_ignore_ascii_case(default)));
    let kept = kept_title(state, address);
    if let Some(seen) = seen {
        if kept.as_deref() != Some(seen.as_str()) {
            let _ = fs::create_dir_all(state.join(TITLES)).and_then(|_| fs::write(&file, &seen));
        }
        return Some(seen);
    }
    kept
}

// Every task in a repo is about that repo, so a title that opens with its name drops it.
pub fn readable(title: &str, repo: &str) -> String {
    let title = title.trim_start_matches(|character: char| !character.is_alphanumeric()).trim();
    let rest = title.split_once(char::is_whitespace)
        .filter(|(first, _)| first.trim_end_matches([':', ',']).eq_ignore_ascii_case(repo))
        .map_or(title, |(_, rest)| rest.trim_start_matches(['-', '—', '–', ':', '·', ' ']));
    let mut characters = rest.chars();
    characters.next().map(|first| first.to_uppercase().chain(characters).collect()).unwrap_or_default()
}

// The task's own commits start where it left the default branch, local or remote, whichever is newer; local commits not pushed yet are not the task's.
pub fn fork_point(path: &Path, upstream: &str) -> Result<String> {
    let local = upstream.split_once('/').map(|(_, branch)| branch).filter(|branch| git(path, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok());
    let mut args = vec!["merge-base", "HEAD", upstream];
    args.extend(local);
    git(path, &args)
}

fn own_commits(path: &Path, upstream: &str) -> Result<usize> {
    let Ok(base) = fork_point(path, upstream) else { return Ok(ahead_behind(path, upstream)?.0) };
    Ok(git(path, &["rev-list", "--count", &format!("{base}..HEAD")])?.parse().unwrap_or(0))
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
pub(crate) fn known_repos(workspaces: &[Value], pooled: Vec<PathBuf>) -> BTreeSet<PathBuf> {
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

// Messages that wait because the task's input box holds the user's own text: the row says what clears the way.
fn held_by_draft(log: &[Message], task: &str, address: &str) -> Option<String> {
    let waiting = log.iter().filter(|message| message.held && message.to == address).count();
    let draft = log.iter().any(|message| message.held && message.to == address && message.why.as_deref() == Some(messages::DRAFT_HOLD));
    draft.then(|| format!("clear the draft in its input box, or kelpie tell --force {task} \"<text>\" ({} held)", plural(waiting, "message")))
}

pub const STARTING_SECONDS: u64 = 120;

// Started with a prompt its Claude has not taken up yet: it is about to work, not waiting for work; a prompt that never lands stops counting after STARTING_SECONDS.
fn starting(prompted: Option<u64>, turn: Option<u64>, now: u64) -> bool {
    prompted.is_some_and(|at| turn.is_none_or(|turn| turn < at) && now.saturating_sub(at) < STARTING_SECONDS)
}

pub fn gather() -> Result<Vec<Item>> {
    let workspaces = herdr::workspaces()?;
    let agents: Vec<Value> = herdr::agents()?.into_iter().filter(|agent| crate::agent::Agent::of(agent).is_some()).collect();
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
            let agent_title = agent.as_ref().map(|agent| agent.title.clone());
            let (_, behind) = ahead_behind(&slot.path, &upstream)?;
            let ahead = own_commits(&slot.path, &upstream)?;
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
                bypass: tasks::bypasses(&address),
                profile: Some(tasks::profile_of(&address).unwrap_or_else(|| crate::profiles::TASK.to_string())),
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
                queued: None,
                held_by_draft: held_by_draft(&log, &task, &address),
                starting: state.as_deref().is_some_and(|state| starting(tasks::prompted_at(state, &address), crate::report::turn_started(&address), messages::now())),
                title: state.as_deref().and_then(|state| title_of(state, &address, agent_title.as_deref(), &name_of(repo), &task, &name_of(&slot.path))),
                name: task,
            });
        }
    }
    if let Some(state) = state.as_deref() {
        let queued: Vec<Item> = tasks::waits_shown(state, &items).into_iter().map(Item::queued_from).collect();
        items.extend(queued);
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
            bypass: false,
            profile: None,
            server: false,
            worker: false,
            url: None,
            name: agent.title.clone(),
            title: None,
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
            queued: None,
            held_by_draft: None,
            starting: false,
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
            review: None, verdict: None, permit: None, follows: false, bypass: false, profile: Some(crate::profiles::TASK.to_string()), server: false, worker: false, url: None, name: "billing".to_string(), title: None, place: "web".to_string(), repo: None, path: None, workspace: None,
            agent: status.map(|status| Agent { pane: "w1:p1".to_string(), status: status.to_string(), title: String::new(), context: None }),
            git: Some(Git { upstream: "origin/main".to_string(), ahead, behind: 0, changed: 0, uncommitted: 0, approved: true }),
            report: report.map(|state| Message {
                id: "1".to_string(), at: 0, from: "billing@web".to_string(), to: LEAD_LABEL.to_string(), text: "plan written".to_string(), held: false,
                report: Some(Report { state: state.to_string(), next: "nothing".to_string() }), ended: false, question: None, about: None, kind: None, landed: None, why: None, forced: false,
            }),
            question: None,
            ci: None,
            recover: None,
            routine: None,
            push_waits: None,
            queued: None,
            held_by_draft: None,
            starting: false,
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
    fn a_ready_task_waiting_only_on_the_lead_shows_under_ready_to_push_marked() {
        let mut ready = task(Some("idle"), Some("ready"), 2);
        ready.git.as_mut().unwrap().approved = false;
        assert!(ready.group() == Group::ReadyToPush, "a finished task waiting only on the lead is no question for the user");
        assert_eq!(ready.summary(), "2 commits · ready to push · lead checking rules");
        let mut quiet = task(Some("idle"), None, 2);
        quiet.git.as_mut().unwrap().approved = false;
        assert!(quiet.group() == Group::Idle);
        assert!(quiet.summary().contains(UNCHECKED), "a task that never reported ready never asked the lead: {}", quiet.summary());
        let mut empty = task(Some("idle"), Some("ready"), 0);
        empty.git.as_mut().unwrap().approved = false;
        assert!(empty.group() == Group::NeedsYou, "ready with nothing committed is no push");
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
        assert!(behind.rules_note().is_none());
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
        assert!(item.group() == Group::ReadyToPush, "passed review, waiting only on the lead");
        assert_eq!(item.summary(), "2 commits · ready to push · lead checking rules · reviewed", "the reviewer's pass does not stand in for the lead's rules check");
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

    fn at(item: &mut Item, status: Option<&str>) -> Group {
        item.agent = status.map(|status| Agent { pane: "w1:p1".to_string(), status: status.to_string(), title: String::new(), context: None });
        item.group()
    }

    fn reported(item: &mut Item, state: &str, when: u64) {
        let message = item.report.get_or_insert_with(|| task(None, Some(state), 0).report.unwrap());
        message.at = when;
        message.report.as_mut().unwrap().state = state.to_string();
    }

    // herdr shows a restarting Claude as gone, then unknown, then idle; none of those is news for the user.
    const RESTARTING: [Option<&str>; 3] = [None, Some("unknown"), Some("idle")];

    #[test]
    fn a_task_started_with_a_prompt_goes_straight_to_working() {
        assert!(starting(Some(100), None, 110) && starting(Some(100), Some(90), 110), "a turn from an earlier task by that name is not this one's");
        assert!(!starting(Some(100), Some(100), 110) && !starting(None, None, 110), "a turn has taken it up, or there was no prompt");
        assert!(!starting(Some(100), None, 100 + STARTING_SECONDS), "a prompt that never landed stops counting");
        let mut item = task(None, None, 0);
        item.starting = true;
        for status in [None, Some("unknown"), Some("idle"), Some("done"), Some("working")] {
            assert_eq!(at(&mut item, status), Group::Working, "{status:?}: never Waiting for work on the way in");
        }
        assert_eq!(at(&mut item, Some("blocked")), Group::NeedsYou, "the trust prompt is the user's");
        item.starting = false;
        assert_eq!(at(&mut item, Some("idle")), Group::Idle, "started without a prompt, it waits for work");
    }

    #[test]
    fn each_transition_lands_in_its_group_without_a_stop_on_the_way() {
        let mut item = task(Some("working"), None, 0);
        item.git.as_mut().unwrap().approved = false;
        assert_eq!(item.group(), Group::Working, "start");
        reported(&mut item, "working", 10);
        assert_eq!(at(&mut item, Some("idle")), Group::Working, "a working report between turns");
        reported(&mut item, "needs-you", 20);
        assert_eq!(at(&mut item, Some("idle")), Group::NeedsYou, "a question");
        assert_eq!(at(&mut item, Some("working")), Group::Working, "the answer starts a turn");
        item.git.as_mut().unwrap().ahead = 2;
        reported(&mut item, "ready", 30);
        assert_eq!(at(&mut item, Some("idle")), Group::ReadyToPush, "ready, the lead checking its rules");
        item.git.as_mut().unwrap().approved = true;
        assert_eq!(item.group(), Group::ReadyToPush, "approved");
        assert_eq!(at(&mut item, Some("working")), Group::ReadyToPush, "a notice it reads, such as an update's, leaves it ready");
        assert!(item.turn_under_way(), "the board holds push until the turn ends");
        assert!(item.summary().ends_with("ready to push · its Claude is taking a turn"), "{}", item.summary());
        item.git.as_mut().unwrap().uncommitted = 1;
        assert_eq!(item.group(), Group::Working, "an edit in that turn is new work");
        item.git.as_mut().unwrap().uncommitted = 0;
        for status in RESTARTING {
            assert_eq!(at(&mut item, status), Group::ReadyToPush, "restart {status:?}");
        }
        item.push_waits = Some(tasks::PendingPush { by: tasks::PushBy::User, work: tasks::PushWork::Tests, at: 40 });
        assert_eq!(at(&mut item, Some("working")), Group::ReadyToPush, "a waiting push");
        item.push_waits = None;
        item.git.as_mut().unwrap().ahead = 0;
        item.ci = Some(watched(crate::ci::State::Running, true));
        item.ci.as_mut().unwrap().started_at = 50;
        for status in [Some("done"), Some("idle"), None, Some("unknown")] {
            assert_eq!(at(&mut item, status), Group::CiRunning, "pushed, {status:?}");
        }
        item.ci = None;
        assert_eq!(at(&mut item, Some("done")), Group::NeedsYou, "without its watch a pushed task has nowhere to be, which is why the watch outlives the push and the close");
    }

    #[test]
    fn a_restart_never_moves_a_task_to_another_group() {
        for state in ["working", "needs-you", "ready", "blocked"] {
            let mut item = task(Some("idle"), Some(state), 2);
            let before = item.group();
            for status in RESTARTING {
                assert_eq!(at(&mut item, status), before, "{state}, {status:?}");
            }
        }
        let mut pushed = task(Some("idle"), Some("ready"), 0);
        pushed.ci = Some(watched(crate::ci::State::Running, true));
        for status in RESTARTING {
            assert_eq!(at(&mut pushed, status), Group::CiRunning, "{status:?}");
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
        let mut red = task(Some("working"), Some("ready"), 0);
        red.ci = Some(watched(crate::ci::State::Failed { at: 20, what: "CI · test".to_string(), url: String::new() }, true));
        red.push_waits = Some(tasks::PendingPush { by: tasks::PushBy::User, work: tasks::PushWork::Ci, at: 20 });
        assert_eq!((red.group(), red.summary(), red.next_for_you(), red.wanted("enter")), (Group::ReadyToPush, "pushing for you · its Claude fixes the red CI".to_string(), None, None), "the fix is the push still under way, not the user's to do");
        red.report.as_mut().unwrap().at = 30;
        red.report.as_mut().unwrap().report.as_mut().unwrap().state = "needs-you".to_string();
        assert_eq!(red.group(), Group::NeedsYou, "its Claude cannot fix it, or the fix needs a decision");
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
        item.agent.as_mut().unwrap().status = "blocked".to_string();
        assert_eq!(item.next_for_you(), Some(ANSWER_IN_SPACE), "a prompt in its pane outranks the last report");
        assert_eq!(ANSWER_IN_SPACE, "answer Claude's prompt in its Space or through the lead");
    }

    #[test]
    fn a_task_whose_messages_wait_behind_the_users_draft_needs_you_and_its_row_says_what_clears_the_way() {
        let log = messages::fold(&[
            r#"{"id":"m1","at":1,"from":"lead","to":"billing@web","text":"a","state":"held","why":"has a draft in its input box"}"#,
            r#"{"id":"m2","at":2,"from":"you","to":"billing@web","text":"b","state":"held","why":"still has earlier messages held"}"#,
            r#"{"id":"m3","at":3,"from":"lead","to":"shop@web","text":"c","state":"held","why":"is waiting for an answer"}"#,
        ].join("\n"));
        let held = held_by_draft(&log, "billing", "billing@web");
        assert_eq!(held.as_deref(), Some("clear the draft in its input box, or kelpie tell --force billing \"<text>\" (2 messages held)"));
        assert_eq!(held_by_draft(&log, "shop", "shop@web"), None, "held for another reason, the row stays as the task reported");
        let mut item = task(Some("idle"), Some("blocked"), 0);
        assert!(item.group() == Group::Idle);
        item.held_by_draft = held;
        assert!(item.group() == Group::NeedsYou && item.urgency() == 1);
        assert_eq!(item.next_for_you(), item.held_by_draft.as_deref());
        item.agent.as_mut().unwrap().status = "working".to_string();
        assert!(item.group() == Group::Working && item.next_for_you().is_none(), "a turn under way takes it up");
        let delivered = messages::fold(r#"{"id":"m1","at":1,"from":"lead","to":"billing@web","text":"a","state":"held","why":"has a draft in its input box"}
{"id":"m1","at":2,"state":"delivered"}"#);
        assert_eq!(held_by_draft(&delivered, "billing", "billing@web"), None);
    }

    #[test]
    fn without_a_report_herdr_decides_as_before() {
        assert!(task(Some("done"), None, 0).group() == Group::NeedsYou);
        assert!(task(Some("idle"), None, 1).group() == Group::ReadyToPush);
        assert!(task(Some("idle"), None, 0).group() == Group::Idle);
    }

    #[test]
    fn a_push_under_way_stays_under_ready_to_push_and_says_what_it_waits_for() {
        use tasks::{PendingPush, PushBy, PushWork};
        let mut item = task(Some("working"), Some("ready"), 1);
        item.git.as_mut().unwrap().approved = false;
        assert_eq!((item.to_json()["push_waits"].clone(), item.push_note(), item.group()), (Value::Null, None, Group::Working));
        item.push_waits = Some(PendingPush { by: PushBy::Lead, work: PushWork::Tests, at: 10 });
        assert_eq!((item.to_json()["push_waits"].clone(), item.group()), (json!("lead"), Group::ReadyToPush), "the Claude working on the push is the push, not new work");
        assert_eq!(item.summary(), "1 commit · pushing for the lead · its Claude runs the tests");
        assert_eq!(item.next_for_you(), None, "nothing for the user to do while it runs");
        item.push_waits = Some(PendingPush { by: PushBy::User, work: PushWork::Conflict, at: 10 });
        assert_eq!((item.to_json()["push_waits"].clone(), item.push_note().unwrap()), (json!("user"), "pushing for you · its Claude resolves the conflict".to_string()));
        let report = |item: &mut Item, state: &str| { let message = item.report.as_mut().unwrap(); message.at = 20; message.report.as_mut().unwrap().state = state.to_string(); };
        item.agent.as_mut().unwrap().status = "idle".to_string();
        report(&mut item, "ready");
        item.git.as_mut().unwrap().approved = false;
        assert_eq!((item.group(), item.push_note().unwrap()), (Group::ReadyToPush, "pushing for you · the lead checks the new commits".to_string()));
        assert_eq!(item.rules_note(), None, "the push note says it");
        item.git.as_mut().unwrap().approved = true;
        assert_eq!(item.push_note().unwrap(), "pushing for you · kelpie pushes it now");
        report(&mut item, "needs-you");
        assert_eq!(item.group(), Group::NeedsYou, "a red test or a choice between behaviours stops the push");
        report(&mut item, "working");
        item.held_by_draft = Some("its box holds your draft".to_string());
        assert_eq!(item.group(), Group::NeedsYou, "a draft in the way needs the user");
        item.held_by_draft = None;
        item.agent.as_mut().unwrap().status = "blocked".to_string();
        assert_eq!(item.group(), Group::NeedsYou, "a prompt of Claude's own needs the user");
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
            question: Some(crate::messages::Question { options: vec![("A".to_string(), "a".to_string())], recommend: 1, plan: None, plan_at: None, comment: Vec::new(), answered: false, waits: true }), about: None, kind: None, landed: None, why: None, forced: false,
        });
        let shown = item.to_json();
        assert_eq!(shown["question"]["kept_for_user"], true);
        assert_eq!(shown["question"]["options"][0]["label"], "A");
        assert_eq!(shown["report"]["summary"], "plan written");
    }

    fn queued(state: &Path, blocked: Option<&str>) -> Vec<Item> {
        let wait = json!({ "task": "web-billing", "repo": "/repos/web", "after": [{ "address": "billing@api", "repo": "/repos/api" }], "prompt": "Use the rounding field", "by": "lead", "at": 4_000_000_000u64, "blocked": blocked });
        crate::state::write(&state.join("after").join("web-billing@web.json"), wait.to_string()).unwrap();
        let mut billing = task(Some("idle"), Some("ready"), 0);
        billing.place = "api".to_string();
        billing.ci = Some(watched(crate::ci::State::Running, true));
        let mut items = vec![billing];
        let shown: Vec<Item> = tasks::waits_shown(state, &items).into_iter().map(Item::queued_from).collect();
        items.extend(shown);
        items
    }

    #[test]
    fn a_queued_task_shows_under_queued_with_what_it_waits_for() {
        let state = crate::sh::test_dir("status-queued");
        let items = queued(&state, None);
        let item = &items[1];
        assert!(item.is_task() && item.group() == Group::Queued && Group::Queued.title() == "QUEUED");
        assert!(Group::Idle < Group::Queued, "queued tasks come last among the tasks");
        assert_eq!(item.summary(), "Waits for billing in api (ci running). Starts by itself once it is pushed and CI is green.");
        assert_eq!(item.to_json()["waiting"], json!(item.summary()), "kelpie status --json says it in the same words");
        assert_eq!(item.next_for_you(), None);
        let shown = item.to_json();
        assert_eq!((shown["kind"].clone(), shown["group"].clone(), shown["after"][0]["address"].clone(), shown["after"][0]["state"].clone()), (json!("queued"), json!("QUEUED"), json!("billing@api"), json!("ci running")));
        assert!(shown["git"].is_null() && shown["path"].is_null() && shown["workspace"].is_null(), "no worktree, no Space");
        let source = include_str!("status.rs");
        let gather = &source[source.find("\npub fn gather(").unwrap()..];
        assert!(gather[..gather.find("\n}\n").unwrap()].contains("tasks::waits_shown(state, &items)"), "kelpie status and the board list them");
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_blocked_wait_shows_under_needs_you_with_its_reason() {
        let state = crate::sh::test_dir("status-blocked");
        let items = queued(&state, Some("billing@api was dropped, so web-billing did not start"));
        let item = &items[1];
        assert!(item.group() == Group::NeedsYou && item.urgency() == 1);
        assert_eq!(item.summary(), "billing@api was dropped, so web-billing did not start");
        assert_eq!(item.next_for_you(), Some("start it anyway or drop the wait"));
        assert_eq!(item.to_json()["blocked"], json!("billing@api was dropped, so web-billing did not start"));
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_row_says_in_a_few_words_what_the_user_is_wanted_for_and_nothing_outside_needs_you() {
        let go = "g";
        let mut asked = task(Some("idle"), Some("needs-you"), 0);
        asked.report.as_mut().unwrap().report.as_mut().unwrap().next = "answer: which queue for refund mails, Sidekiq or Solid Queue or inline?".to_string();
        asked.question = Some(messages::fold(r#"{"id":"q1","at":10,"from":"billing@web","to":"lead","text":"Which queue?","state":"asked","question":{"options":[{"label":"A","meaning":"one"},{"label":"B","meaning":"two"}],"recommend":1}}"#).remove(0));
        assert_eq!(asked.wanted(go).as_deref(), Some("answer a question · 2 options"));
        asked.question.as_mut().unwrap().question.as_mut().unwrap().plan = Some("vault:Projects/billing.md".to_string());
        assert_eq!(asked.wanted(go).as_deref(), Some("approve the plan"));
        asked.agent.as_mut().unwrap().status = "blocked".to_string();
        assert_eq!(asked.wanted(go).as_deref(), Some("answer Claude's prompt · g goes there"), "a prompt on screen outranks the question");
        let mut mock = task(Some("idle"), Some("needs-you"), 1);
        mock.report.as_mut().unwrap().report.as_mut().unwrap().next = "approve the mock".to_string();
        assert_eq!(mock.wanted(go).as_deref(), Some("approve the mock"), "the task's own words, whole");
        let mut ready = task(Some("idle"), Some("ready"), 5);
        ready.report.as_mut().unwrap().report.as_mut().unwrap().next = "review the 5 commits and the card, then push and close".to_string();
        for approved in [false, true] {
            ready.git.as_mut().unwrap().approved = approved;
            assert!(ready.group() == Group::ReadyToPush);
            assert_eq!(ready.wanted(go), None, "the group says push, the row asks nothing");
        }
        let mut held = task(Some("idle"), Some("ready"), 0);
        held.held_by_draft = Some("clear the draft in its input box".to_string());
        assert_eq!(held.wanted(go).as_deref(), Some("clear the draft in its input box"));
        let mut working = task(Some("working"), Some("needs-you"), 1);
        working.report.as_mut().unwrap().report.as_mut().unwrap().next = "approve the mock".to_string();
        assert_eq!(working.wanted(go), None);
        let mut waiting = task(Some("idle"), Some("blocked"), 1);
        waiting.report.as_mut().unwrap().report.as_mut().unwrap().next = "nudge the other task".to_string();
        assert_eq!(waiting.wanted(go), None, "blocked on something other than the user");
    }

    #[test]
    fn a_task_shows_claudes_session_title_without_the_repo_word_and_keeps_it_when_claude_is_gone() {
        assert_eq!(readable("Kelpie board task rows", "kelpie"), "Board task rows");
        assert_eq!(readable("✳ kelpie: wrapped CI links", "kelpie"), "Wrapped CI links");
        assert_eq!(readable("Kelpies everywhere", "kelpie"), "Kelpies everywhere");
        let state = crate::sh::test_dir("titles");
        assert_eq!(title_of(&state, "rows@kelpie", Some("Claude Code"), "kelpie", "rows", "wt4.kelpie"), None, "Claude has not named the session yet");
        assert_eq!(title_of(&state, "rows@kelpie", Some("wt4.kelpie"), "kelpie", "rows", "wt4.kelpie"), None, "the terminal's folder is no title");
        assert_eq!(title_of(&state, "rows@kelpie", Some("Kelpie board task rows"), "kelpie", "rows", "wt4.kelpie").as_deref(), Some("Board task rows"));
        assert_eq!(title_of(&state, "rows@kelpie", None, "kelpie", "rows", "wt4.kelpie").as_deref(), Some("Board task rows"), "kept for when its Claude is not running");
        let mut item = task(Some("idle"), None, 0);
        assert_eq!(item.titled(), "billing");
        item.title = Some("Refund mails".to_string());
        assert_eq!((item.titled(), item.to_json()["title"].as_str(), item.to_json()["name"].as_str()), ("Refund mails".to_string(), Some("Refund mails"), Some("billing")));
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn a_task_counts_only_its_own_commits_when_local_main_is_ahead_of_origin() {
        let dir = crate::sh::test_dir("own-commits");
        let commit = |message: &str| {
            std::fs::write(dir.join("file"), message).unwrap();
            git(&dir, &["add", "file"]).unwrap();
            git(&dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", message]).unwrap();
        };
        git(&dir, &["init", "--quiet", "-b", "main"]).unwrap();
        commit("pushed");
        git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
        for unpushed in ["local one", "local two", "local three"] {
            commit(unpushed);
        }
        git(&dir, &["checkout", "--quiet", "-b", "task"]).unwrap();
        commit("the task's own");
        assert_eq!(ahead_behind(&dir, "origin/main").unwrap().0, 4, "what a push would land");
        assert_eq!(own_commits(&dir, "origin/main").unwrap(), 1);
        assert_eq!(review_progress(&dir, "origin/main").map(|(_, files)| files), Some(1));
        git(&dir, &["branch", "--quiet", "-D", "main"]).unwrap();
        assert_eq!(own_commits(&dir, "origin/main").unwrap(), 4, "without a local main the remote one is the fork point");
        let _ = std::fs::remove_dir_all(dir);
    }
}
