use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::herdr;
use crate::messages::{self, Message};
use crate::peers;
use crate::phase::{Phase, Why};
use crate::repo::{ahead_behind, changed_count, default_branch, git, name_of, pooled_checkouts, uncommitted};
use crate::sh::Result;
use crate::tasks::{self, slots};

pub const LEAD_LABEL: &str = "lead";
pub const UNCHECKED: &str = "rules not checked";
pub const CHECKING: &str = "lead checking rules";
pub const REVIEWER_CHECKING: &str = "reviewer checking rules";
pub const STAGED: &str = "on local main · waiting for your push";

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
    pub const ALL: [Group; 7] = [Group::NeedsYou, Group::Working, Group::ToReview, Group::ReadyToPush, Group::CiRunning, Group::Idle, Group::Queued];

    pub fn key(self) -> &'static str {
        match self {
            Group::NeedsYou => "needs-you",
            Group::Working => "working",
            Group::ToReview => "to-review",
            Group::ReadyToPush => "ready-to-push",
            Group::CiRunning => "ci-running",
            Group::Idle => "waiting-for-work",
            Group::Queued => "queued",
        }
    }

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
    pub head: Option<String>,
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
    pub rules: Option<crate::rules_check::Shown>,
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
    pub phase: Option<crate::phase::Record>,
    pub landed: Option<tasks::LandedUnsure>,
    pub ci_outside: Option<String>,
    pub ci_dismissed: Vec<Value>,
    pub push_left: Option<String>,
    pub staged: bool,
}

impl Item {
    // A push kelpie stopped between taking it from the line and sending it leaves Pushing behind with nothing in the line; the task is ready again.
    fn settle_orphaned_push(&mut self) {
        let head = self.git.as_ref().and_then(|git| git.head.clone());
        if let Some(record) = self.phase.as_mut().filter(|record| matches!(record.phase, Phase::Pushing { .. })).filter(|_| self.push_waits.is_none()) {
            record.phase = Phase::Ready { head };
        }
    }

    // A red CI's push waits for its fix outside the line, so it is never taken out of one.
    pub fn in_push_line(&self) -> bool {
        self.push_waits.as_ref().is_some_and(|pending| pending.work != tasks::PushWork::Ci)
    }

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
            review: None, verdict: None, rules: None, permit: None, follows: false, bypass: false, profile: Some(wait.profile.clone().unwrap_or_else(|| crate::profiles::TASK.to_string())), server: false, worker: false, url: None,
            name: wait.task.clone(), title, place: name_of(&wait.repo), repo: Some(wait.repo.clone()), path: None, workspace: None, agent: None, git: None,
            report: None, question: None, ci: None, recover: None, routine: None, push_waits: None,
            queued: Some(Queued { after, blocked: wait.blocked.clone(), waiting, names, base, at, backlog: wait.backlog.is_some() }),
            held_by_draft: None, phase: None, landed: None, ci_outside: None, ci_dismissed: Vec::new(), push_left: None, staged: false,
        }
    }

    // Commits and no tracked file left uncommitted; how far behind the default branch it is plays no part, push rebases first.
    fn committed(&self) -> bool {
        self.git.as_ref().is_some_and(|git| git.ahead > 0 && git.uncommitted == 0)
    }

    fn pushable(&self) -> bool {
        self.committed() && self.git.as_ref().is_some_and(|git| git.approved)
    }

    // A task that reported ready has its rules checked by the reviewer, or by the lead once kelpie asked it; one that has not, has not.
    pub fn rules_note(&self) -> Option<String> {
        let unchecked = self.git.as_ref().is_some_and(|git| git.ahead > 0 && !git.approved) && self.group() != Group::Working && self.push_waits.is_none();
        if !unchecked {
            return None;
        }
        Some(match &self.rules {
            Some(crate::rules_check::Shown::Checking) => REVIEWER_CHECKING.to_string(),
            Some(crate::rules_check::Shown::Broken(1)) => "rules: 1 break".to_string(),
            Some(crate::rules_check::Shown::Broken(count)) => format!("rules: {count} breaks"),
            Some(crate::rules_check::Shown::Lead) => CHECKING.to_string(),
            None => UNCHECKED.to_string(),
        })
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

    fn phase_note(&self) -> Option<String> {
        match self.phase()? {
            Phase::Closing => Some("pushed · kelpie closes it".to_string()),
            Phase::NeedsYou(Why::Failed) => Some("kelpie could not finish its push or close it; the lead has why".to_string()),
            Phase::NeedsYou(Why::Unreadable) => Some(format!("gh has not read its CI for {} minutes", crate::ci::UNREADABLE_AFTER / 60)),
            Phase::NeedsYou(Why::LeftLine) => Some(self.left_note()),
            Phase::NeedsYou(Why::NotStaged) => Some(format!("kelpie could not put it on local main again within {} minutes; stage it once the main checkout is clear", tasks::STAGE_WAIT / 60)),
            _ => None,
        }
    }

    pub fn ci_note(&self) -> Option<String> {
        let note = self.ci.as_ref().map(crate::ci::Watch::note)?;
        Some(match &self.ci_outside {
            Some(reason) => format!("{note} · its Claude says outside the code: {reason}"),
            None => note,
        })
    }

    fn recovering(&self) -> Option<&crate::recover::Record> {
        let record = self.recover.as_ref()?;
        let newer_report = self.report.as_ref().is_some_and(|report| report.at > record.error_at);
        let working = self.agent.as_ref().is_some_and(|agent| agent.status == "working");
        (!newer_report && !working).then_some(record)
    }

    pub fn recover_note(&self) -> Option<String> {
        self.recovering().map(crate::recover::Record::note)
    }

    pub fn push_note(&self) -> Option<String> {
        self.push_note_at(messages::now())
    }

    fn push_note_at(&self, now: u64) -> Option<String> {
        let pending = self.push_waits.as_ref()?;
        let stalled = self.report.as_ref().is_some_and(|report| now.saturating_sub(report.at) > PUSH_STALLS_AFTER);
        let who = match pending.by {
            tasks::PushBy::User => "you",
            tasks::PushBy::Lead => "the lead",
        };
        if pending.work == tasks::PushWork::Turn {
            let place = if pending.ahead == 0 { "kelpie pushes it now".to_string() } else { tasks::place_in_line(pending.ahead, pending.after.as_deref()) };
            return Some(format!("pushing for {who} · {place}"));
        }
        let what = match pending.work {
            _ if self.reported_since(pending.at, "ready") && self.git.as_ref().is_some_and(|git| git.approved) && stalled => "ready for a while, but kelpie has not seen its Claude take the request; push again",
            _ if self.reported_since(pending.at, "ready") && self.git.as_ref().is_some_and(|git| git.approved) => "kelpie pushes it now",
            _ if self.reported_since(pending.at, "ready") => "the new commits' rules are checked",
            tasks::PushWork::Turn | tasks::PushWork::Tests => "its Claude runs the tests",
            tasks::PushWork::Conflict => "its Claude resolves the conflict",
            tasks::PushWork::Ci => "its Claude fixes the red CI",
        };
        Some(format!("pushing for {who} · {what}"))
    }

    fn reported_since(&self, at: u64, state: &str) -> bool {
        self.report.as_ref().is_some_and(|report| report.at > at && report.report.as_ref().is_some_and(|report| report.state == state))
    }

    pub fn turn_under_way(&self) -> bool {
        self.agent.as_ref().is_some_and(|agent| agent.status == "working")
    }

    fn phase(&self) -> Option<&Phase> {
        self.phase.as_ref().map(|record| &record.phase)
    }

    fn blocked_on_a_prompt(&self) -> bool {
        self.agent.as_ref().is_some_and(|agent| agent.status == "blocked")
    }

    // What wants the user now, whatever the phase: Claude's own prompt, a question, a draft holding messages back, or work that landed and waits for them.
    fn wants_you_now(&self) -> bool {
        self.blocked_on_a_prompt() || self.question.is_some() || self.held_by_draft.is_some() || self.landed.is_some_and(tasks::LandedUnsure::waits_for_you)
    }

    // A task's group is its kept phase, which only the actors that move it write; herdr's working or idle is the row's icon, never its group.
    pub fn group(&self) -> Group {
        if let Some(queued) = &self.queued {
            return if queued.blocked.is_some() { Group::NeedsYou } else { Group::Queued };
        }
        let Some(phase) = self.phase() else {
            return match self.agent.as_ref().map(|agent| agent.status.as_str()) {
                Some("blocked" | "done") => Group::NeedsYou,
                Some("working") => Group::Working,
                _ => Group::Idle,
            };
        };
        if self.wants_you_now() {
            return Group::NeedsYou;
        }
        match phase {
            Phase::Starting | Phase::Working | Phase::Closing => Group::Working,
            Phase::Idle => Group::Idle,
            Phase::NeedsYou(_) | Phase::CiRed { .. } => Group::NeedsYou,
            Phase::Ready { .. } if self.awaiting_review() => Group::ToReview,
            Phase::Ready { .. } | Phase::Staged { .. } | Phase::Pushing { .. } => Group::ReadyToPush,
            Phase::CiRunning { .. } => Group::CiRunning,
        }
    }

    // The phase a task had before phases were kept, read once from what the board showed for it then.
    pub(crate) fn derive_legacy(&self) -> Phase {
        let reported_after = |at: u64| self.report.as_ref().is_some_and(|report| report.at > at);
        if self.push_waits.is_some() {
            return Phase::Pushing { ahead: 0 };
        }
        if let Some(ci) = self.ci.as_ref().filter(|ci| !reported_after(ci.since())) {
            match ci.state {
                crate::ci::State::Failed { .. } => return Phase::CiRed { sha: ci.sha.clone() },
                crate::ci::State::Running if ci.close => return Phase::CiRunning { sha: ci.sha.clone() },
                crate::ci::State::Running => {}
            }
        }
        if self.recover.as_ref().is_some_and(|record| record.kind == crate::recover::Kind::Stuck && !reported_after(record.error_at)) {
            return Phase::NeedsYou(Why::Stopped);
        }
        let head = self.git.as_ref().and_then(|git| git.head.clone());
        match (self.report_state(), self.agent.as_ref().map(|agent| agent.status.as_str())) {
            (Some("working"), _) | (None, Some("working")) => Phase::Working,
            (Some("needs-you"), _) => Phase::NeedsYou(if self.question.is_some() { Why::Question } else { Why::Report }),
            (Some("blocked"), _) => Phase::Idle,
            (Some("ready"), _) if self.committed() => Phase::Ready { head },
            (Some("ready"), _) => Phase::NeedsYou(Why::NothingToPush),
            (_, Some("done")) => Phase::NeedsYou(Why::Unreported),
            _ if self.pushable() => Phase::Ready { head },
            _ => Phase::Idle,
        }
    }

    // Within Needs you: a question on screen first, then what the task asks of you, then what is ready, then the rest.
    pub fn urgency(&self) -> u8 {
        if self.group() != Group::NeedsYou {
            return 0;
        }
        if self.queued.is_some() {
            return 1;
        }
        if self.blocked_on_a_prompt() {
            return 0;
        }
        if self.wants_you_now() {
            return 1;
        }
        match self.phase() {
            Some(Phase::NeedsYou(Why::NothingToPush)) => 2,
            Some(Phase::NeedsYou(Why::Unreported)) | None => 3,
            Some(_) => 1,
        }
    }

    // A turn under way has most likely taken up what the last report asked.
    pub fn next_for_you(&self) -> Option<&str> {
        if let Some(queued) = &self.queued {
            return queued.blocked.as_ref().map(|_| "start it anyway or drop the wait");
        }
        if self.blocked_on_a_prompt() {
            return Some(ANSWER_IN_SPACE);
        }
        if self.turn_under_way() || self.recovering().is_some() || matches!(self.phase(), Some(Phase::Pushing { .. } | Phase::CiRunning { .. } | Phase::CiRed { .. } | Phase::Closing)) {
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
        if self.blocked_on_a_prompt() {
            return Some(format!("answer Claude's prompt · {go} goes there"));
        }
        if let Some(landed) = self.landed_note() {
            return Some(landed);
        }
        if self.recovering().is_some() || matches!(self.phase(), Some(Phase::CiRed { .. })) {
            return None;
        }
        if let Some(held) = &self.held_by_draft {
            return Some(held.clone());
        }
        if matches!(self.phase(), Some(Phase::NeedsYou(Why::LeftLine))) && self.question.is_none() {
            return Some(self.left_note());
        }
        if let Some(question) = self.question.as_ref().and_then(|message| message.question.as_ref()) {
            return Some(if question.plan.is_some() { "approve the plan".to_string() } else { format!("answer a question · {}", plural(question.options.len(), "option")) });
        }
        self.report.as_ref()?.next_for_you().map(str::to_string)
    }

    fn left_note(&self) -> String {
        match &self.push_left {
            Some(why) => format!("out of the push line: {why}"),
            None => "out of the push line before it went out; push it again when it is ready".to_string(),
        }
    }

    pub fn landed_note(&self) -> Option<String> {
        let main = self.git.as_ref()?.upstream.trim_start_matches("origin/");
        self.landed.map(|why| why.said(main, crate::keys::key(crate::keys::Act::Done)))
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
        if self.group() == Group::ReadyToPush && self.staged {
            return format!("{} · {STAGED}", plural(git.ahead, "commit"));
        }
        if self.group() == Group::ReadyToPush {
            let review: String = self.rules_note().into_iter().chain(self.review_note()).map(|note| format!(" · {note}")).collect();
            let behind = if git.behind > 0 { format!(" · {} behind {}", git.behind, git.upstream.trim_start_matches("origin/")) } else { String::new() };
            let turn = if self.turn_under_way() { " · its Claude is taking a turn" } else { "" };
            let dirty = if git.uncommitted > 0 { format!(" · {}", plural(git.uncommitted, "uncommitted file")) } else { String::new() };
            return format!("{} · ready to push{dirty}{behind}{review}{turn}", plural(git.ahead, "commit"));
        }
        let mut parts: Vec<String> = self.staged.then(|| STAGED.to_string()).into_iter().chain(self.phase_note()).chain(self.ci_note()).chain(self.recover_note()).chain(self.push_note()).chain(self.landed.filter(|why| !why.waits_for_you()).and(self.landed_note())).collect();
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
        parts.extend(self.rules_note());
        parts.extend(self.review_note());
        if parts.is_empty() { "nothing yet".to_string() } else { parts.join(" · ") }
    }

    pub fn to_json(&self) -> Value {
        let mut shown = self.shown_json();
        shown["staged"] = json!(self.staged);
        shown
    }

    fn shown_json(&self) -> Value {
        json!({
            "name": self.name,
            "title": self.title.as_deref().unwrap_or(&self.name),
            "place": self.place,
            "kind": if self.queued.is_some() { "queued" } else if self.is_task() { "task" } else { "session" },
            "after": self.queued.as_ref().map(|queued| queued.after.iter().map(|(address, state)| json!({ "address": address, "state": state })).collect::<Vec<_>>()),
            "blocked": self.queued.as_ref().and_then(|queued| queued.blocked.clone()),
            "waiting": self.waiting(),
            "group": self.group().title(),
            "phase": self.phase.as_ref().map(crate::phase::Record::to_json),
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
            "push_waits": self.push_waits.as_ref().map(|pending| pending.by.key()),
            "push_ahead": self.push_waits.as_ref().filter(|pending| pending.work == tasks::PushWork::Turn).map(|pending| pending.ahead),
            "landed": self.landed_note(),
            "ci_outside": self.ci_outside,
            "ci_dismissed": self.ci_dismissed,
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

// What a ready report stands on: commits of the task's own and no tracked file left uncommitted.
pub(crate) fn committed_at(path: &Path) -> bool {
    let Ok(upstream) = crate::repo::main_checkout(path).and_then(|repo| default_branch(&repo)) else { return false };
    own_commits(path, &upstream).is_ok_and(|ahead| ahead > 0) && uncommitted(path).is_ok_and(|files| files.is_empty())
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
pub(crate) fn held_by_draft(log: &[Message], task: &str, address: &str) -> Option<String> {
    let held: Vec<&Message> = log.iter().filter(|message| message.held && message.to == address && message.kind.as_deref() != Some(messages::NOTICE_KIND)).collect();
    let (waiting, draft) = (held.len(), held.iter().any(|message| message.why.as_deref() == Some(messages::DRAFT_HOLD)));
    draft.then(|| format!("clear the draft in its input box, or kelpie tell --force {task} \"<text>\" ({} held)", plural(waiting, "message")))
}

const PUSH_STALLS_AFTER: u64 = 600;

// The line names the push in front by its task; the row shows that task's title.
fn name_push_fronts(items: &mut [Item]) {
    let titles: Vec<(Option<PathBuf>, String, String)> = items.iter().map(|item| (item.repo.clone(), item.name.clone(), item.titled())).collect();
    for item in items.iter_mut() {
        let repo = item.repo.clone();
        if let Some(after) = item.push_waits.as_mut().and_then(|pending| pending.after.as_mut())
            && let Some((_, _, title)) = titles.iter().find(|(other, name, _)| *other == repo && name == after) {
            *after = title.clone();
        }
    }
}

// Only a read of every pool knows which tasks are gone.
fn sweep_phases(state: &Path, items: &[Item], every_pool_read: bool, began: u64) {
    if every_pool_read {
        let live: Vec<String> = items.iter().filter_map(|item| Some(peers::address(&item.name, &name_of(item.repo.as_deref()?)))).collect();
        crate::phase::sweep(state, &live, began);
    }
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

    let began = messages::now();
    let log = messages::load();
    let state = crate::lead::state_dir().ok();
    let reports = messages::latest_reports(&log);
    let questions = crate::questions::open_now(&log);
    let mut items = Vec::new();
    let mut task_workspaces = BTreeSet::new();
    let mut every_pool_read = true;
    for repo in &repos {
        let (Ok(upstream), Ok(pool)) = (default_branch(repo), slots(repo)) else {
            every_pool_read = false;
            continue;
        };
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
            let head = crate::repo::git(&slot.path, &["rev-parse", "HEAD"]).ok();
            let tokens = workspace.as_ref().and_then(|id| herdr::workspace_tokens(id).ok()).unwrap_or_default();
            let server = tokens["server"].is_string();
            let mut item = Item {
                review: review_progress(&slot.path, &upstream),
                verdict: crate::review::shown(&address, &slot.path, &upstream),
                rules: state.as_deref().and_then(|state| crate::rules_check::shown(state, &address, &slot.path, &upstream)),
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
                git: Some(Git { upstream: upstream.clone(), ahead, behind, changed, uncommitted, approved, head }),
                report: reports.get(&address).cloned(),
                question: questions.iter().find(|question| question.from == address).map(|question| (*question).clone()),
                ci: state.as_deref().and_then(|state| crate::ci::load(state, &address)),
                recover: state.as_deref().and_then(|state| crate::recover::load(state, &address)),
                routine: state.as_deref().and_then(|state| crate::routines::run_of(state, &address)).map(|(id, _)| id),
                push_waits: state.as_deref().and_then(|state| tasks::pending_push(state, &address)),
                queued: None,
                held_by_draft: held_by_draft(&log, &task, &address),
                landed: state.as_deref().and_then(|state| tasks::landed_mark(state, &address, &slot.path)),
                ci_outside: state.as_deref().and_then(|state| tasks::ci_outside_reason(state, &address)),
                ci_dismissed: state.as_deref().map(|state| tasks::ci_dismissed(state, &address)).unwrap_or_default(),
                push_left: state.as_deref().and_then(|state| tasks::push_left(state, &address)),
                staged: state.as_deref().is_some_and(|state| tasks::staged(state, repo, &address)),
                phase: None,
                title: state.as_deref().and_then(|state| title_of(state, &address, agent_title.as_deref(), &name_of(repo), &task, &name_of(&slot.path))),
                name: task,
            };
            item.phase = Some(match state.as_deref() {
                Some(state) => crate::phase::current(state, &address, || item.derive_legacy()),
                None => crate::phase::Record { phase: item.derive_legacy(), since: messages::now(), by: "migration".to_string() },
            });
            item.settle_orphaned_push();
            items.push(item);
        }
    }
    if let Some(state) = state.as_deref() {
        sweep_phases(state, &items, every_pool_read, began);
    }
    name_push_fronts(&mut items);
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
            rules: None,
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
            phase: None,
            landed: None,
            ci_outside: None,
            ci_dismissed: Vec::new(),
            push_left: None,
            staged: false,
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
    use crate::phase::Event;

    pub(crate) fn task(status: Option<&str>, report: Option<&str>, ahead: usize) -> Item {
        let mut item = Item {
            review: None, verdict: None, rules: None, permit: None, follows: false, bypass: false, profile: Some(crate::profiles::TASK.to_string()), server: false, worker: false, url: None, name: "billing".to_string(), title: None, place: "web".to_string(), repo: None, path: None, workspace: None,
            agent: status.map(|status| Agent { pane: "w1:p1".to_string(), status: status.to_string(), title: String::new(), context: None }),
            git: Some(Git { upstream: "origin/main".to_string(), ahead, behind: 0, changed: 0, uncommitted: 0, approved: true, head: None }),
            report: report.map(|state| Message {
                id: "1".to_string(), at: 0, from: "billing@web".to_string(), to: LEAD_LABEL.to_string(), text: "plan written".to_string(), held: false,
                report: Some(Report { state: state.to_string(), next: "nothing".to_string(), head: None }), ended: false, question: None, about: None, kind: None, landed: None, why: None, forced: false, fyi: false,
            }),
            question: None,
            ci: None,
            recover: None,
            routine: None,
            push_waits: None,
            queued: None,
            held_by_draft: None,
            phase: None,
            landed: None,
            ci_outside: None,
            ci_dismissed: Vec::new(),
            push_left: None,
            staged: false,
        };
        rederive(&mut item);
        item
    }

    pub(crate) fn in_phase(item: &mut Item, phase: Phase) {
        item.phase = Some(crate::phase::Record { phase, since: 0, by: "test".to_string() });
    }

    // As migration reads a task the first time; a test that changes a signal and wants it read again says so.
    pub(crate) fn rederive(item: &mut Item) {
        let phase = item.derive_legacy();
        in_phase(item, phase);
    }

    pub(crate) fn moved(item: &mut Item, event: &crate::phase::Event) {
        if let Some(phase) = crate::phase::next(item.phase(), event) {
            in_phase(item, phase);
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
        in_phase(&mut item, Phase::Working);
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
    fn a_task_from_before_phases_takes_once_the_phase_its_signals_showed() {
        let derived = |status, report, ahead| task(status, report, ahead).phase().cloned().unwrap();
        assert_eq!(derived(Some("idle"), Some("needs-you"), 0), Phase::NeedsYou(Why::Report));
        assert_eq!(derived(Some("idle"), Some("ready"), 0), Phase::NeedsYou(Why::NothingToPush));
        assert_eq!(derived(Some("done"), Some("ready"), 2), Phase::Ready { head: None });
        assert_eq!(derived(Some("done"), Some("working"), 0), Phase::Working);
        assert_eq!(derived(Some("idle"), Some("blocked"), 0), Phase::Idle);
        assert_eq!(derived(None, Some("needs-you"), 0), Phase::NeedsYou(Why::Report));
        assert_eq!(derived(Some("done"), None, 0), Phase::NeedsYou(Why::Unreported));
        assert_eq!(derived(Some("working"), None, 0), Phase::Working);
        assert_eq!(derived(Some("idle"), None, 1), Phase::Ready { head: None });
        assert_eq!(derived(Some("idle"), None, 0), Phase::Idle);
        let mut pushing = task(Some("idle"), Some("ready"), 1);
        pushing.push_waits = Some(tasks::PendingPush { by: tasks::PushBy::User, work: tasks::PushWork::Tests, at: 5, ahead: 0, after: None });
        rederive(&mut pushing);
        assert_eq!(pushing.phase(), Some(&Phase::Pushing { ahead: 0 }));
        let mut pushed = task(Some("idle"), Some("ready"), 0);
        pushed.ci = Some(watched(crate::ci::State::Running, true));
        rederive(&mut pushed);
        assert_eq!(pushed.phase(), Some(&Phase::CiRunning { sha: "abcdef0".to_string() }));
        pushed.ci = Some(watched(crate::ci::State::Failed { at: 20, what: "CI · test".to_string(), url: String::new() }, true));
        rederive(&mut pushed);
        assert_eq!(pushed.phase(), Some(&Phase::CiRed { sha: "abcdef0".to_string() }));
        pushed.report.as_mut().unwrap().at = 30;
        rederive(&mut pushed);
        assert_eq!(pushed.phase(), Some(&Phase::NeedsYou(Why::NothingToPush)), "a report after the failure decided");
        let mut stopped = task(Some("idle"), Some("working"), 0);
        stopped.recover = Some(stopped_by(crate::recover::Kind::Stuck));
        rederive(&mut stopped);
        assert_eq!(stopped.phase(), Some(&Phase::NeedsYou(Why::Stopped)));
        let mut ready = task(Some("idle"), Some("ready"), 2);
        for status in ["working", "done", "unknown"] {
            assert_eq!(at(&mut ready, Some(status)), Group::ReadyToPush, "once derived, herdr never moves it: {status}");
        }
    }

    #[test]
    fn a_ready_task_waiting_only_on_the_lead_shows_under_ready_to_push_marked() {
        let mut ready = task(Some("idle"), Some("ready"), 2);
        ready.git.as_mut().unwrap().approved = false;
        ready.rules = Some(crate::rules_check::Shown::Lead);
        assert!(ready.group() == Group::ReadyToPush, "a finished task waiting only on the lead is no question for the user");
        assert_eq!(ready.summary(), "2 commits · ready to push · lead checking rules");
        let mut quiet = task(Some("idle"), None, 2);
        quiet.git.as_mut().unwrap().approved = false;
        rederive(&mut quiet);
        assert!(quiet.group() == Group::Idle);
        assert!(quiet.summary().contains(UNCHECKED), "a task that never reported ready never asked the lead: {}", quiet.summary());
        let empty = task(Some("idle"), Some("ready"), 0);
        assert!(empty.group() == Group::NeedsYou, "ready with nothing committed is no push");
        let mut working = task(Some("working"), Some("ready"), 2);
        working.git.as_mut().unwrap().approved = false;
        assert!(!working.summary().contains(CHECKING), "the lead checks only once it was asked: {}", working.summary());
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
        assert_eq!(dirty.summary(), "2 commits · ready to push · 2 uncommitted files", "an edit after the report leaves it where it is and says what keeps it from push");
        rederive(&mut dirty);
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
    fn a_reviewers_pass_is_the_rules_check_and_its_findings_wait_under_to_review() {
        let mut item = task(Some("idle"), Some("ready"), 2);
        item.verdict = Some(in_review(true, 0, false, false));
        assert!(item.group() == Group::ReadyToPush);
        assert_eq!(item.summary(), "2 commits · ready to push · reviewed", "the pass recorded the approval, so no rules check waits");
        let mut working = task(Some("working"), Some("ready"), 2);
        working.verdict = Some(in_review(false, 1, false, false));
        assert!(working.group() == Group::ToReview, "a turn is the row's icon, not its group");
        let mut asks = task(Some("idle"), Some("needs-you"), 2);
        asks.verdict = Some(in_review(false, 1, false, false));
        assert!(asks.group() == Group::NeedsYou);
        let mut pushed = task(Some("idle"), Some("ready"), 0);
        pushed.verdict = Some(in_review(false, 1, true, false));
        assert!(!pushed.awaiting_review(), "nothing left to push, nothing left to review");
    }

    #[test]
    fn a_ready_task_shows_the_reviewer_checking_its_rules_then_the_breaks_it_found() {
        let mut item = task(Some("idle"), Some("ready"), 2);
        item.git.as_mut().unwrap().approved = false;
        item.rules = Some(crate::rules_check::Shown::Checking);
        assert!(item.group() == Group::ReadyToPush, "the check is no question for the user");
        assert_eq!(item.summary(), "2 commits · ready to push · reviewer checking rules");
        item.rules = Some(crate::rules_check::Shown::Broken(2));
        assert_eq!(item.summary(), "2 commits · ready to push · rules: 2 breaks");
        item.rules = Some(crate::rules_check::Shown::Broken(1));
        assert!(item.summary().ends_with("rules: 1 break"));
        item.rules = Some(crate::rules_check::Shown::Lead);
        assert!(item.summary().ends_with(CHECKING), "the lead asked after the reviewer gave no verdict");
        item.rules = None;
        assert!(item.summary().ends_with(UNCHECKED), "the lead checks only once it was asked: {}", item.summary());
        item.git.as_mut().unwrap().approved = true;
        assert_eq!(item.summary(), "2 commits · ready to push", "an approved row stays quiet");
    }

    fn watched(state: crate::ci::State, close: bool) -> crate::ci::Watch {
        crate::ci::Watch {
            address: "billing@web".to_string(), task: "billing".to_string(), repo: PathBuf::new(), slug: "acme/web".to_string(), sha: "abcdef0".to_string(),
            started_at: 10, checked_at: None, close, runs: Vec::new(), state, error: None, erring_since: None, pushed: crate::ci::Pushed::default(),
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
    fn a_task_started_with_a_prompt_goes_straight_to_working_and_one_without_waits_for_work() {
        let mut item = task(None, None, 0);
        moved(&mut item, &Event::Started { prompted: true });
        for status in [None, Some("unknown"), Some("idle"), Some("done"), Some("working")] {
            assert_eq!(at(&mut item, status), Group::Working, "{status:?}: never Waiting for work on the way in");
        }
        assert_eq!(at(&mut item, Some("blocked")), Group::NeedsYou, "the trust prompt is the user's");
        moved(&mut item, &Event::TurnBegan { by_user: true });
        assert_eq!(at(&mut item, Some("idle")), Group::Working, "the prompt taken up");
        assert_eq!(item.phase(), Some(&Phase::Working));
        moved(&mut item, &Event::Started { prompted: false });
        assert_eq!(at(&mut item, Some("idle")), Group::Idle, "started without a prompt, it waits for work");
        moved(&mut item, &Event::TurnBegan { by_user: false });
        assert_eq!(item.group(), Group::Idle, "a notice of kelpie's is no work");
        moved(&mut item, &Event::TurnBegan { by_user: true });
        assert_eq!(item.group(), Group::Working, "the user's prompt is");
    }

    #[test]
    fn a_task_goes_from_start_to_done_along_its_path_and_never_leaves_it() {
        let mut item = task(None, None, 0);
        item.git.as_mut().unwrap().approved = false;
        let mut seen = Vec::new();
        let mut step = |item: &mut Item, what: &str, event: Option<Event>, statuses: &[Option<&str>], expected: Group| {
            if let Some(event) = event {
                moved(item, &event);
            }
            for status in statuses {
                assert_eq!(at(item, *status), expected, "{what}, herdr {status:?}");
            }
            seen.push(expected);
        };
        step(&mut item, "start", Some(Event::Started { prompted: true }), &[None, Some("unknown"), Some("idle")], Group::Working);
        step(&mut item, "first turn", Some(Event::TurnBegan { by_user: true }), &[Some("working")], Group::Working);
        item.git.as_mut().unwrap().ahead = 2;
        reported(&mut item, "ready", 30);
        step(&mut item, "ready, the lead checking its rules", Some(Event::Reported { state: "ready", head: Some("abc"), committed: true, pushing: false }), &[Some("idle"), Some("done")], Group::ReadyToPush);
        item.git.as_mut().unwrap().approved = true;
        step(&mut item, "approved", None, &[Some("idle")], Group::ReadyToPush);
        step(&mut item, "an update notice it reads", Some(Event::TurnBegan { by_user: false }), &[Some("working"), Some("idle"), Some("done")], Group::ReadyToPush);
        item.push_waits = Some(tasks::PendingPush { by: tasks::PushBy::User, work: tasks::PushWork::Tests, at: 40, ahead: 0, after: None });
        step(&mut item, "push: main changed its files, so its Claude runs the tests", Some(Event::PushHanded), &[Some("idle")], Group::ReadyToPush);
        step(&mut item, "the hand-over it takes up", Some(Event::TurnBegan { by_user: false }), &[Some("working")], Group::ReadyToPush);
        reported(&mut item, "ready", 50);
        step(&mut item, "the tests pass", Some(Event::Reported { state: "ready", head: Some("abd"), committed: true, pushing: true }), &[Some("idle"), Some("done")], Group::ReadyToPush);
        item.push_waits = None;
        item.git.as_mut().unwrap().ahead = 0;
        step(&mut item, "kelpie pushes it", Some(Event::PushOut { sha: "abcdef0" }), &[Some("done"), Some("idle"), None], Group::CiRunning);
        item.ci = Some(watched(crate::ci::State::Running, true));
        step(&mut item, "the push notice it reads", Some(Event::TurnBegan { by_user: false }), &[Some("working")], Group::CiRunning);
        reported(&mut item, "ready", 60);
        step(&mut item, "the report that turn had to write", Some(Event::Reported { state: "ready", head: Some("abd"), committed: false, pushing: false }), &[Some("idle"), Some("done"), Some("unknown"), None], Group::CiRunning);
        item.ci = None;
        step(&mut item, "CI green, between the watch going and the task closing", None, &[Some("done")], Group::CiRunning);
        assert_eq!(seen.iter().filter(|group| **group == Group::NeedsYou).count(), 0, "nothing on the way asked anything of the user: {seen:?}");
        let state = crate::sh::test_dir("phase-done");
        crate::phase::record(&state, "billing@web", "ci", &Event::PushOut { sha: "abcdef0" });
        crate::phase::forget(&state, "billing@web");
        assert_eq!(crate::phase::load(&state, "billing@web"), None, "done: the phase goes with the task");
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_push_never_shows_under_needs_you_on_its_way_out() {
        let mut item = task(Some("idle"), Some("ready"), 2);
        moved(&mut item, &Event::PushOut { sha: "abcdef0" });
        item.git.as_mut().unwrap().ahead = 0;
        for status in [Some("done"), Some("idle"), Some("unknown"), None] {
            assert_eq!(at(&mut item, status), Group::CiRunning, "pushed, its ready report now has nothing committed under it and no watch is read yet, herdr {status:?}");
        }
        let mut handed = task(Some("idle"), Some("ready"), 0);
        moved(&mut handed, &Event::PushHanded);
        assert_eq!(at(&mut handed, Some("done")), Group::ReadyToPush, "a push its Claude works for");
    }

    #[test]
    fn green_ci_never_shows_under_needs_you() {
        let mut item = task(Some("idle"), Some("ready"), 0);
        moved(&mut item, &Event::PushOut { sha: "abcdef0" });
        item.ci = None;
        moved(&mut item, &Event::Closing);
        for status in [Some("done"), Some("idle"), Some("working")] {
            assert_eq!(at(&mut item, status), Group::Working, "green, its note being written before kelpie closes it, herdr {status:?}");
        }
        reported(&mut item, "ready", 90);
        moved(&mut item, &Event::Reported { state: "ready", head: None, committed: false, pushing: false });
        assert_eq!(at(&mut item, Some("idle")), Group::Working, "the note written, it waits for kelpie to close it");
        assert!(item.summary().starts_with("pushed · kelpie closes it"), "{}", item.summary());
    }

    #[test]
    fn an_update_notice_never_moves_a_task() {
        for (phase, group) in [(Phase::Ready { head: Some("abc".to_string()) }, Group::ReadyToPush), (Phase::NeedsYou(Why::Report), Group::NeedsYou), (Phase::Idle, Group::Idle), (Phase::CiRunning { sha: "abc".to_string() }, Group::CiRunning), (Phase::Pushing { ahead: 0 }, Group::ReadyToPush)] {
            let mut item = task(Some("idle"), Some("ready"), 2);
            in_phase(&mut item, phase.clone());
            moved(&mut item, &Event::TurnBegan { by_user: false });
            assert_eq!(at(&mut item, Some("working")), group, "{phase:?}");
            assert!(item.turn_under_way(), "the turn shows on the row's icon");
        }
    }

    #[test]
    fn a_ready_task_the_lead_has_not_approved_stays_ready_through_any_turn() {
        let mut item = task(Some("idle"), Some("ready"), 2);
        item.git.as_mut().unwrap().approved = false;
        item.rules = Some(crate::rules_check::Shown::Lead);
        item.git.as_mut().unwrap().head = Some("abc".to_string());
        item.report.as_mut().unwrap().report.as_mut().unwrap().head = Some("abc".to_string());
        assert_eq!(item.group(), Group::ReadyToPush);
        moved(&mut item, &Event::TurnBegan { by_user: true });
        assert_eq!(at(&mut item, Some("working")), Group::ReadyToPush, "the lead's word or the user's");
        item.git.as_mut().unwrap().head = Some("def".to_string());
        assert_eq!(item.group(), Group::ReadyToPush, "a new commit in that turn moves it only with the report that says so");
        assert!(item.summary().ends_with("ready to push · lead checking rules · its Claude is taking a turn"), "{}", item.summary());
        reported(&mut item, "working", 70);
        moved(&mut item, &Event::Reported { state: "working", head: Some("def"), committed: true, pushing: false });
        assert_eq!(at(&mut item, Some("idle")), Group::Working);
    }

    #[test]
    fn a_pushed_task_stays_under_ci_running_across_a_turn_and_its_ready_report_and_leaves_only_on_needs_you() {
        let mut item = task(Some("idle"), Some("ready"), 0);
        item.ci = Some(watched(crate::ci::State::Running, true));
        rederive(&mut item);
        assert_eq!(at(&mut item, Some("working")), Group::CiRunning, "a notice it reads is no new work");
        reported(&mut item, "ready", 30);
        item.report.as_mut().unwrap().report.as_mut().unwrap().next = "push".to_string();
        for state in ["ready", "working"] {
            moved(&mut item, &Event::Reported { state, head: None, committed: false, pushing: false });
            for status in [Some("idle"), Some("done"), Some("working")] {
                assert_eq!(at(&mut item, status), Group::CiRunning, "the {state} report that turn had to write, {status:?}");
            }
        }
        assert_eq!(item.next_for_you(), None, "the push its report asks for is done");
        item.ci.as_mut().unwrap().error = Some("gh: not logged in".to_string());
        assert_eq!(at(&mut item, Some("idle")), Group::CiRunning, "a run gh cannot read stays where it is");
        let ready = item.clone();
        reported(&mut item, "blocked", 40);
        moved(&mut item, &Event::Reported { state: "blocked", head: None, committed: false, pushing: false });
        assert_eq!(at(&mut item, Some("idle")), Group::Idle, "the task's word that it is blocked");
        item = ready;
        reported(&mut item, "needs-you", 40);
        moved(&mut item, &Event::Reported { state: "needs-you", head: None, committed: false, pushing: false });
        assert_eq!(at(&mut item, Some("idle")), Group::NeedsYou, "the task's word that it needs the user");
        assert_eq!(at(&mut item, Some("blocked")), Group::NeedsYou);
    }

    #[test]
    fn a_draft_holding_back_only_kelpies_own_notices_is_no_reason_to_need_the_user() {
        let log = |kind: &str| crate::messages::fold(&format!(r#"{{"id":"m1","at":1,"from":"kelpie","to":"billing@web","text":"kelpie was updated","state":"held","why":"{}","kind":"{kind}"}}"#, messages::DRAFT_HOLD));
        assert_eq!(held_by_draft(&log(messages::NOTICE_KIND), "billing", "billing@web"), None);
        assert!(held_by_draft(&log(messages::HAND_KIND), "billing", "billing@web").is_some_and(|said| said.starts_with("clear the draft")));
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
        rederive(&mut pushed);
        for status in RESTARTING {
            assert_eq!(at(&mut pushed, status), Group::CiRunning, "{status:?}");
        }
    }

    #[test]
    fn a_pushed_task_waits_under_ci_running_and_a_failed_run_puts_it_under_needs_you_until_it_reports_again() {
        let mut item = task(Some("idle"), Some("ready"), 0);
        item.ci = Some(watched(crate::ci::State::Running, true));
        moved(&mut item, &Event::PushOut { sha: "abcdef0" });
        assert!(item.group() == Group::CiRunning);
        assert_eq!(item.summary(), "CI running: waiting for runs of abcdef0");
        assert_eq!(item.next_for_you(), None, "the push the old report asked for is done");
        item.ci = Some(watched(crate::ci::State::Failed { at: 20, what: "CI · test".to_string(), url: String::new() }, true));
        moved(&mut item, &Event::CiRed { sha: "abcdef0", pushing: false });
        assert!(item.group() == Group::NeedsYou && item.urgency() == 1);
        assert_eq!(item.summary(), "CI failed: CI · test");
        moved(&mut item, &Event::TurnBegan { by_user: false });
        item.agent.as_mut().unwrap().status = "working".to_string();
        assert!(item.group() == Group::NeedsYou, "the log kelpie hands it starts a turn, shown on its icon");
        item.agent.as_mut().unwrap().status = "idle".to_string();
        item.report.as_mut().unwrap().at = 30;
        moved(&mut item, &Event::Reported { state: "ready", head: None, committed: false, pushing: false });
        assert_eq!(item.phase(), Some(&Phase::NeedsYou(Why::NothingToPush)), "a report after the failure decides again");
        let mut red = task(Some("working"), Some("ready"), 0);
        red.ci = Some(watched(crate::ci::State::Failed { at: 20, what: "CI · test".to_string(), url: String::new() }, true));
        red.push_waits = Some(tasks::PendingPush { by: tasks::PushBy::User, work: tasks::PushWork::Ci, at: 20, ahead: 0, after: None });
        moved(&mut red, &Event::CiRed { sha: "abcdef0", pushing: true });
        assert_eq!((red.group(), red.summary(), red.next_for_you(), red.wanted("enter")), (Group::ReadyToPush, "pushing for you · its Claude fixes the red CI".to_string(), None, None), "the fix is the push still under way, not the user's to do");
        red.report.as_mut().unwrap().at = 30;
        red.report.as_mut().unwrap().report.as_mut().unwrap().state = "needs-you".to_string();
        moved(&mut red, &Event::Reported { state: "needs-you", head: None, committed: false, pushing: true });
        assert_eq!(red.group(), Group::NeedsYou, "its Claude cannot fix it, or the fix needs a decision");
        let mut partial = task(Some("idle"), Some("working"), 1);
        partial.ci = Some(watched(crate::ci::State::Running, false));
        rederive(&mut partial);
        assert!(partial.group() == Group::Working, "a partial push leaves the task where it is");
        assert!(partial.summary().starts_with("CI running"));
    }

    fn stopped_by(kind: crate::recover::Kind) -> crate::recover::Record {
        crate::recover::Record {
            address: "billing@web".to_string(), kind, reason: "overload".to_string(), text: "API Error: Repeated 529 Overloaded errors.".to_string(), error_id: "e1".to_string(), error_at: 10,
            attempts: 1, next_at: Some(4_000_000_000), reset_at: Some(4_000_000_000), sent_for: None,
        }
    }

    #[test]
    fn a_task_kelpie_resumes_stays_where_it_was_and_one_it_will_not_resume_needs_you() {
        let mut item = task(Some("idle"), Some("needs-you"), 1);
        in_phase(&mut item, Phase::Working);
        item.recover = Some(stopped_by(crate::recover::Kind::Retrying));
        assert!(item.group() == Group::Working);
        assert!(item.summary().starts_with("retrying after overload (2/4, next "), "{}", item.summary());
        assert_eq!(item.next_for_you(), None);
        item.recover = Some(stopped_by(crate::recover::Kind::Allowance));
        assert!(item.summary().starts_with("out of allowance until "), "{}", item.summary());
        item.recover = Some(stopped_by(crate::recover::Kind::Stuck));
        moved(&mut item, &Event::Stopped);
        assert!(item.group() == Group::NeedsYou && item.urgency() == 1);
        assert!(item.summary().starts_with("stopped by overload: API Error"), "{}", item.summary());
        item.report.as_mut().unwrap().at = 20;
        assert!(item.recover_note().is_none(), "a report after the error decides again");
        let mut busy = task(Some("working"), None, 0);
        busy.recover = Some(stopped_by(crate::recover::Kind::Stuck));
        moved(&mut busy, &Event::Stopped);
        moved(&mut busy, &Event::TurnBegan { by_user: true });
        assert!(busy.group() == Group::Working && busy.recover_note().is_none(), "the user's prompt is past the error");
    }

    #[test]
    fn a_prompt_on_screen_outranks_the_phase_and_a_turn_does_not() {
        assert!(task(Some("working"), Some("needs-you"), 0).group() == Group::NeedsYou);
        assert!(task(Some("blocked"), Some("working"), 0).group() == Group::NeedsYou);
        let mut asked = task(Some("idle"), Some("working"), 0);
        asked.question = Some(messages::fold(r#"{"id":"q1","at":10,"from":"billing@web","to":"you","text":"Which queue?","state":"asked","question":{"options":[{"label":"A","meaning":"one"},{"label":"B","meaning":"two"}],"recommend":1}}"#).remove(0));
        assert!(asked.group() == Group::NeedsYou, "an open question needs the user whatever the phase says");
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
        assert!(item.group() == Group::NeedsYou && item.next_for_you().is_none(), "the draft still holds them; the turn shows on the icon");
        let delivered = messages::fold(r#"{"id":"m1","at":1,"from":"lead","to":"billing@web","text":"a","state":"held","why":"has a draft in its input box"}
{"id":"m1","at":2,"state":"delivered"}"#);
        assert_eq!(held_by_draft(&delivered, "billing", "billing@web"), None);
    }

    #[test]
    fn a_pushing_phase_with_no_push_left_in_the_line_reads_as_ready_again() {
        let mut item = task(Some("idle"), Some("ready"), 1);
        item.git.as_mut().unwrap().head = Some("abc".to_string());
        moved(&mut item, &Event::PushQueued { ahead: 0 });
        item.push_waits = Some(tasks::PendingPush { by: tasks::PushBy::User, work: tasks::PushWork::Turn, at: 1, ahead: 0, after: None });
        item.settle_orphaned_push();
        assert_eq!(item.phase(), Some(&Phase::Pushing { ahead: 0 }), "a push in the line stays pushing");
        item.push_waits = None;
        item.settle_orphaned_push();
        assert_eq!((item.phase(), item.group()), (Some(&Phase::Ready { head: Some("abc".to_string()) }), Group::ReadyToPush), "a crash between the line and the push leaves it pushable, not stuck");
    }

    #[test]
    fn a_push_in_line_says_its_place_by_the_title_of_the_one_in_front_and_one_that_left_says_why() {
        use tasks::{PendingPush, PushBy, PushWork};
        let mut front = task(Some("idle"), Some("ready"), 1);
        front.title = Some("Refunds on invoices".to_string());
        let mut item = task(Some("idle"), Some("ready"), 2);
        item.name = "search".to_string();
        item.push_waits = Some(PendingPush { by: PushBy::User, work: PushWork::Turn, at: 10, ahead: 1, after: Some("billing".to_string()) });
        moved(&mut item, &Event::PushQueued { ahead: 1 });
        let mut items = [front, item];
        name_push_fronts(&mut items);
        let item = &mut items[1];
        assert_eq!((item.group(), item.summary()), (Group::ReadyToPush, "2 commits · pushing for you · 2nd in line after Refunds on invoices".to_string()));
        assert_eq!((item.next_for_you(), item.to_json()["push_ahead"].clone()), (None, json!(1)), "nothing for the user while it waits its turn");
        item.push_waits = Some(PendingPush { by: PushBy::Lead, work: PushWork::Turn, at: 10, ahead: 0, after: None });
        assert_eq!(item.push_note().unwrap(), "pushing for the lead · kelpie pushes it now");
        item.push_waits = None;
        item.push_left = Some("its Claude did not report ready within 30 minutes, so the pushes behind it went on".to_string());
        moved(item, &Event::PushFailed);
        item.phase.as_mut().unwrap().phase = Phase::NeedsYou(Why::LeftLine);
        assert_eq!(item.group(), Group::NeedsYou);
        assert_eq!(item.wanted("g").unwrap(), "out of the push line: its Claude did not report ready within 30 minutes, so the pushes behind it went on");
    }

    #[test]
    fn a_push_under_way_stays_under_ready_to_push_and_says_what_it_waits_for() {
        use tasks::{PendingPush, PushBy, PushWork};
        let mut item = task(Some("working"), Some("ready"), 1);
        item.git.as_mut().unwrap().approved = false;
        assert_eq!((item.to_json()["push_waits"].clone(), item.push_note(), item.group()), (Value::Null, None, Group::ReadyToPush));
        item.push_waits = Some(PendingPush { by: PushBy::Lead, work: PushWork::Tests, at: 10, ahead: 0, after: None });
        moved(&mut item, &Event::PushHanded);
        assert_eq!((item.to_json()["push_waits"].clone(), item.group()), (json!("lead"), Group::ReadyToPush), "the Claude working on the push is the push, not new work");
        assert_eq!(item.summary(), "1 commit · pushing for the lead · its Claude runs the tests");
        assert_eq!(item.next_for_you(), None, "nothing for the user to do while it runs");
        item.push_waits = Some(PendingPush { by: PushBy::User, work: PushWork::Conflict, at: 10, ahead: 0, after: None });
        assert_eq!((item.to_json()["push_waits"].clone(), item.push_note().unwrap()), (json!("user"), "pushing for you · its Claude resolves the conflict".to_string()));
        let report = |item: &mut Item, state: &str| {
            let message = item.report.as_mut().unwrap();
            message.at = 20;
            message.report.as_mut().unwrap().state = state.to_string();
            moved(item, &Event::Reported { state, head: None, committed: true, pushing: true });
        };
        item.agent.as_mut().unwrap().status = "idle".to_string();
        report(&mut item, "ready");
        item.git.as_mut().unwrap().approved = false;
        assert_eq!((item.group(), item.push_note().unwrap()), (Group::ReadyToPush, "pushing for you · the new commits' rules are checked".to_string()));
        assert_eq!(item.rules_note(), None, "the push note says it");
        item.git.as_mut().unwrap().approved = true;
        assert_eq!(item.push_note_at(20 + PUSH_STALLS_AFTER).unwrap(), "pushing for you · kelpie pushes it now");
        assert_eq!(item.push_note_at(21 + PUSH_STALLS_AFTER).unwrap(), "pushing for you · ready for a while, but kelpie has not seen its Claude take the request; push again", "a push that never completes says so");
        report(&mut item, "needs-you");
        assert_eq!(item.group(), Group::NeedsYou, "a red test or a choice between behaviours stops the push");
        report(&mut item, "working");
        assert_eq!(item.group(), Group::ReadyToPush, "back at it, the push goes on");
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
        session.phase = None;
        assert!(!session.is_task());
        assert_eq!(session.summary(), "Claude session outside a task worktree");
        assert!(session.group() == Group::Working);
        assert_eq!(session.to_json()["kind"], "session");
        assert!(session.to_json()["git"].is_null() && session.to_json()["push_permission"].is_null() && session.to_json()["phase"].is_null());
        let checkout = Checkout { name: "app".to_string(), changed: 2, free_slots: 1, busy_slots: 3 };
        assert_eq!(checkout.to_json(), json!({ "repo": "app", "changed": 2, "free_worktrees": 1, "busy_worktrees": 3 }));
    }

    #[test]
    fn the_json_keeps_its_fields_and_adds_the_phase() {
        let item = task(Some("idle"), Some("ready"), 2);
        let shown = item.to_json();
        assert_eq!((shown["group"].clone(), shown["phase"]["name"].clone(), shown["phase"]["by"].clone()), (json!("READY TO PUSH"), json!("ready"), json!("test")));
        for field in ["name", "title", "place", "kind", "summary", "agent", "report", "ci", "push_waits", "landed", "git"] {
            assert!(shown.get(field).is_some(), "{field}");
        }
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
            question: Some(crate::messages::Question { options: vec![("A".to_string(), "a".to_string())], recommend: 1, plan: None, plan_at: None, comment: Vec::new(), answered: false, waits: true }), about: None, kind: None, landed: None, why: None, forced: false, fyi: false,
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
        let mut answered = task(Some("idle"), Some("needs-you"), 1);
        answered.report.as_mut().unwrap().report.as_mut().unwrap().next = "approve the mock".to_string();
        moved(&mut answered, &Event::TurnBegan { by_user: true });
        answered.agent.as_mut().unwrap().status = "working".to_string();
        assert_eq!(answered.wanted(go), None, "the user answered in its Space");
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
    fn a_staged_task_waits_under_ready_to_push_saying_it_is_on_local_main() {
        let mut item = task(Some("idle"), Some("ready"), 2);
        in_phase(&mut item, Phase::Staged { head: Some("abc".to_string()) });
        item.staged = true;
        assert_eq!(item.group(), Group::ReadyToPush);
        assert_eq!(item.summary(), "2 commits · on local main · waiting for your push");
        let shown = item.to_json();
        assert_eq!((shown["staged"].as_bool(), shown["phase"]["name"].as_str()), (Some(true), Some("staged")));
        in_phase(&mut item, Phase::Working);
        assert_eq!(item.summary(), "on local main · waiting for your push · 2 commits", "still on main while its Claude works again");
        item.staged = false;
        assert_eq!(item.to_json()["staged"].as_bool(), Some(false));
    }

    #[test]
    fn phases_are_swept_only_after_a_read_of_every_pool() {
        let state = crate::sh::test_dir("status-sweep");
        for address in ["billing@web", "gone@web"] {
            crate::phase::record(&state, address, "start", &Event::Started { prompted: false });
        }
        let mut billing = task(Some("idle"), None, 0);
        billing.repo = Some(PathBuf::from("/repos/web"));
        let began = messages::now() + 1;
        sweep_phases(&state, std::slice::from_ref(&billing), false, began);
        assert!(crate::phase::load(&state, "gone@web").is_some(), "a pool that could not be read may hold it");
        sweep_phases(&state, std::slice::from_ref(&billing), true, began);
        assert_eq!((crate::phase::load(&state, "billing@web").is_some(), crate::phase::load(&state, "gone@web")), (true, None));
        std::fs::remove_dir_all(state).unwrap();
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
