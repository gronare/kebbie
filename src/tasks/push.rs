use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::board::widgets::plural;
use crate::config;
use crate::herdr;
use crate::lead::state_dir;
use crate::messages;
use crate::notice::{Notice, Tone};
use crate::peers;
use crate::permit::Caller;
use crate::repo::{UNHOOKED, ahead_behind, default_branch, git, git_writing, hidden_from_status, main_checkout, name_of, rebase_in_progress, uncommitted};
use crate::sh::Result;
use super::approval::{approved, fingerprint, fingerprint_at, same_code};
use super::intent::{self, By, Intent, Through, Work};
use super::finish::{close_task, note_ready};
use super::{fetch, follow_upstream};
use super::pool::{Slot, find_task, task_of};
use super::started::started_file;

pub fn land(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    committed(&slot.path, task)?;
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    let main = main_of(&upstream);
    if ahead_behind(&slot.path, &upstream)?.0 == 0 {
        return Notice::new(Tone::NotNow, "Nothing to land: it has no commits").detail("task", task).err();
    }
    if !rebase_or_hand_over(&slot, &upstream, "kebbie tried to land this task")? {
        return Notice::new(Tone::Moving, format!("Not landed yet: it conflicts with {main}")).next("Its Claude resolves the rebase and reports back; push it then.").detail("task", task).err();
    }
    let commits = git(&slot.path, &["log", "--oneline", &format!("{upstream}..HEAD")])?;
    Notice::new(Tone::Done, format!("Rebased onto {main} and ready to push")).next(format!("Push it with: kebbie push {task}")).detail("commits", commits).ok()
}

fn main_of(upstream: &str) -> &str {
    upstream.trim_start_matches("origin/")
}

pub fn rebase(cwd: &Path) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, None, cwd)?;
    let task = task_of(&slot);
    committed(&slot.path, task)?;
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    let main = main_of(&upstream);
    if ahead_behind(&slot.path, &upstream)?.1 == 0 {
        return Notice::new(Tone::Done, format!("Already on top of {main}")).detail("task", task).ok();
    }
    if !rebase_or_hand_over(&slot, &upstream, "The user asked kebbie to rebase this task")? {
        return Notice::new(Tone::Moving, format!("It conflicts with {main}")).next("Its Claude resolves the rebase and reports back.").detail("task", task).ok();
    }
    Notice::new(Tone::Done, format!("Rebased onto {main}")).next("Nothing was pushed.").detail("task", task).ok()
}

// Uncommitted changes to tracked files may be unfinished work, so they keep a task from landing or pushing; untracked files do not.
pub(super) fn committed(path: &Path, task: &str) -> Result<()> {
    let dirty = uncommitted(path)?;
    if dirty.is_empty() {
        return Ok(());
    }
    Notice::new(Tone::You, format!("It has {}", plural(dirty.len(), "uncommitted file"))).you("Commit or stash them in its worktree first.")
        .detail("task", task).detail("files", dirty.join(" · ")).detail("folder", path.display().to_string()).err()
}

// Files the task changes that the upstream commits it was rebased over changed too, each with those commits as "sha subject".
#[derive(Debug, PartialEq)]
pub(super) struct Touched {
    pub file: String,
    pub commits: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub(super) enum Updated {
    Clean,
    Overlap(Vec<Touched>),
    Conflict,
}

// Rebases onto upstream, already fetched; a conflict is aborted, and a rebase git refuses to start is an error with git's own reason.
pub(super) fn update(path: &Path, upstream: &str) -> Result<Updated> {
    update_in(path, upstream, &[], None)
}

// With from, only the commits after it go onto upstream, as git rebase --onto takes them.
fn update_in(path: &Path, upstream: &str, options: &[&str], from: Option<&str>) -> Result<Updated> {
    let touched = overlap(path, upstream, from)?;
    let rebase: Vec<&str> = match from {
        Some(from) => vec!["rebase", "--quiet", "--onto", upstream, from],
        None => vec!["rebase", "--quiet", upstream],
    };
    match git_writing(path, &[options, &rebase].concat()) {
        Ok(_) if touched.is_empty() => Ok(Updated::Clean),
        Ok(_) => Ok(Updated::Overlap(touched)),
        Err(_) if rebase_in_progress(path) => {
            git_writing(path, &[options, &["rebase", "--abort"]].concat())?;
            Ok(Updated::Conflict)
        }
        Err(error) => Err(refused(path, upstream, &error)),
    }
}

// Rebases the commits after from through sha onto upstream in a detached worktree of kebbie's own, so the task's worktree, its HEAD and its uncommitted work stay as its Claude left them.
fn rebased_elsewhere(path: &Path, from: &str, sha: &str, upstream: &str) -> Result<(Updated, String)> {
    let place = std::env::temp_dir().join(format!("kebbie-push-{}-{}", std::process::id(), &sha[..12.min(sha.len())]));
    let _ = std::fs::remove_dir_all(&place);
    let spot = place.to_string_lossy().to_string();
    git(path, &[&UNHOOKED[..], &["worktree", "add", "--quiet", "--detach", &spot, sha]].concat())?;
    let built = update_in(&place, upstream, &UNHOOKED, Some(from)).and_then(|updated| Ok((updated, git(&place, &["rev-parse", "HEAD"])?)));
    if git(path, &["worktree", "remove", "--force", &spot]).is_err() {
        let _ = std::fs::remove_dir_all(&place);
        let _ = git(path, &["worktree", "prune"]);
    }
    built
}

fn refused(path: &Path, upstream: &str, error: &str) -> String {
    let said = Notice::new(Tone::Failed, format!("git would not start the rebase onto {}", main_of(upstream))).detail("folder", path.display().to_string()).detail("error", error);
    match hidden_from_status(path) {
        Ok(hidden) if !hidden.is_empty() => said
            .you("Unhide the files below with git update-index --no-skip-worktree or --no-assume-unchanged, commit or stash them, then try again.")
            .detail("hidden", format!("{} (skip-worktree or assume-unchanged)", hidden.join(", "))).to_string(),
        _ => said.to_string(),
    }
}

// The task's own commits that reached upstream, a push of its first commits among them, are no one else's changes, so they never count as overlap.
fn overlap(path: &Path, upstream: &str, since: Option<&str>) -> Result<Vec<Touched>> {
    let base = git(path, &["merge-base", "HEAD", upstream])?;
    let mine: BTreeSet<String> = git(path, &["log", "--format=", "--name-only", "--no-renames", &format!("{}..HEAD", since.unwrap_or(&base))])?.lines().filter(|line| !line.is_empty()).map(str::to_string).collect();
    let own: BTreeSet<String> = super::accept::commit_codes(path, &[&format!("{base}..HEAD")])?.into_iter().map(|(_, code)| code).collect();
    let landed_own: BTreeSet<String> = super::accept::commit_codes(path, &[&format!("{base}..{upstream}")])?.into_iter().filter(|(_, code)| own.contains(code)).map(|(sha, _)| sha).collect();
    let theirs = git(path, &["log", "--reverse", "--format=%x00%H %s", "--name-only", "--no-renames", &format!("{base}..{upstream}")])?;
    let mut touched: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in theirs.split('\0').filter(|entry| !entry.trim().is_empty()) {
        let mut lines = entry.trim().lines();
        let line = lines.next().unwrap_or_default();
        let (sha, subject) = line.split_once(' ').unwrap_or((line, ""));
        if landed_own.contains(sha) {
            continue;
        }
        let commit = format!("{} {subject}", &sha[..7.min(sha.len())]);
        for file in lines.filter(|file| mine.contains(*file)) {
            touched.entry(file.to_string()).or_default().push(commit.clone());
        }
    }
    Ok(touched.into_iter().map(|(file, commits)| Touched { file, commits }).collect())
}

// Ok(false) when the rebase conflicted and the task's Claude took it over.
fn rebase_or_hand_over(slot: &Slot, upstream: &str, why: &str) -> Result<bool> {
    if update(&slot.path, upstream)? != Updated::Conflict {
        return Ok(true);
    }
    let task = task_of(slot);
    if let Err(cannot) = hand_over(slot, &conflict_prompt(why, upstream)) {
        return Notice::new(Tone::Failed, format!("It conflicts with {}, and kebbie could not pass the rebase to its Claude", main_of(upstream)))
            .you("Rebase it yourself in its worktree.").detail("task", task).detail("folder", slot.path.display().to_string()).detail("error", cannot).err();
    }
    Ok(false)
}

pub(super) fn conflict_prompt(why: &str, upstream: &str) -> String {
    format!(
        "{why} and rebasing onto {upstream} conflicts (already fetched). Rebase this branch onto {upstream} with git rebase {upstream} (no -i), \
         resolve each conflict keeping the intent of both sides (read the upstream commits that touched the file), run the tests for the files \
         involved, and continue until the rebase is finished. When a conflict is a choice of intent rather than mechanics, both sides wanting \
         different behaviour from the same code, do not pick one: ask the user with kebbie ask, with the options and your recommendation, and \
         resolve it by their answer. Do not push. Then report ready, saying in a few lines what conflicted and how you resolved it. When a \
         push started this, kebbie pushes the resolved commits as soon as their rules are checked; report needs-you only when a test stays red."
    )
}

fn overlap_prompt(by: By, upstream: &str, touched: &[Touched], through: Through) -> String {
    let files: Vec<String> = touched.iter().map(|touched| format!("- {}: {}", touched.file, touched.commits.join("; "))).collect();
    if let Through::First { count, .. } = through {
        return format!(
            "{} asked kebbie to push the first {} of this task. kebbie rebased them onto {upstream} in a worktree of its own without conflicts, but the commits \
             they went over changed files they change too:\n{}\nkebbie changed nothing in your worktree. Once your work in progress is committed or stashed, \
             rebase your branch onto {upstream} yourself (git rebase {upstream}, already fetched, no -i), run the tests that cover those files, and fix what \
             broke in those first commits themselves, not in a commit after them (git commit --amend, or git rebase with GIT_SEQUENCE_EDITOR editing the \
             todo), since kebbie pushes only them. Do not push. When the tests pass, report ready: kebbie pushes them for {} then, once the lead has checked a fix against the \
             rules. Report needs-you only when a test stays red. {WAITING_BEHIND}",
            capitalized(by.word()), plural(count, "commit"), files.join("\n"), by.word()
        );
    }
    format!(
        "{} asked kebbie to push this task. kebbie fetched and rebased it onto {upstream} without conflicts, but the commits it was rebased over \
         changed files this task changes too:\n{}\nRun the tests that cover those files and fix what broke with a new commit. Do not push. When \
         the tests pass, report ready: kebbie pushes the task for {} then, once the lead has checked a fix against the rules. Report needs-you \
         only when a test stays red. {WAITING_BEHIND}",
        capitalized(by.word()), files.join("\n"), by.word()
    )
}

const LEFT_AS_IT_WAS: &str = "kebbie changed nothing in your worktree: rebase once your work in progress is committed or stashed. kebbie pushes only the commits it was asked to, so resolve the conflict in them, not in a commit after them.";

const WAITING_BEHIND: &str = "The other pushes of this repo wait behind this one, so do this before anything else.";

// Held like any message, so a draft in the box or an open question delays the work and never drops it.
pub(super) fn hand_over(slot: &Slot, text: &str) -> std::result::Result<(), String> {
    let repo = main_checkout(&slot.path)?;
    messages::hand_to(&peers::address(task_of(slot), &name_of(&repo)), text)
}

fn claude_of(path: &Path) -> Option<String> {
    let workspace = herdr::workspace_for(path).ok()??;
    herdr::agents().ok()?.into_iter()
        .find(|agent| agent["agent"] == "claude" && agent["workspace_id"].as_str() == Some(workspace.as_str()))
        .and_then(|agent| agent["pane_id"].as_str().map(str::to_string))
}

// Agents have no interactive git, so the prompt names the non-interactive ways to reshape history.
pub fn tidy(cwd: &Path, name: Option<&str>, note: &str) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    let pane = claude_of(&slot.path).ok_or_else(|| Notice::new(Tone::NotNow, "Not tidied: its Claude is not running").detail("task", task).to_string())?;
    let upstream = default_branch(&repo)?;
    let rules = config::resolve(Some(&repo))?.settings.rules_skill.value;
    let mut prompt = format!(
        "Reshape this branch's commits on top of {upstream} for review, following {}. Interactive git is not \
         available: use git rebase with GIT_SEQUENCE_EDITOR editing the todo, git commit --amend, or git reset --soft and recommit. \
         The final tree must be identical: git diff against the old head must be empty. Do not push. Show the commit list before and after.",
        config::rules_named(&rules)
    );
    if !note.trim().is_empty() {
        prompt.push_str(&format!(" From the user: {}", note.trim()));
    }
    crate::typing::prompt(&pane, &prompt, task).map_err(|refused| Notice::new(Tone::NotNow, "Not tidied: kebbie typed nothing").next("Tidy again once its input box is clear.").detail("task", task).detail("why", format!("its Claude {}", refused.reason())).to_string())?;
    Notice::new(Tone::Moving, "Its Claude tidies the commits").next("It reports back when they are reshaped.").detail("task", task).ok()
}

// Pushes the task's commits through `through` (all of them without it) to the default branch, then closes the task, or with keep_open leaves the rest of its branch on top of what was pushed.
pub fn push(cwd: &Path, name: Option<&str>, through: Option<&str>, keep_open: bool) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let upstream = default_branch(&repo)?;
    let through = chosen(&slot, &upstream, through, keep_open)?;
    push_task(&crate::permit::caller(), &repo, &slot, through)
}

// The commit the user picked, as a count of the task's commits, so it survives the rebase push starts with.
fn chosen(slot: &Slot, upstream: &str, through: Option<&str>, keep_open: bool) -> Result<Through> {
    let open = open_shas(&slot.path, upstream)?;
    if open.is_empty() {
        return nothing_to_push(task_of(slot));
    }
    let Some(rev) = through else { return Ok(Through::All) };
    let sha = pushable_through(&slot.path, upstream, rev, keep_open)?;
    if !keep_open {
        return Ok(Through::All);
    }
    let count = open.iter().position(|commit| *commit == sha).map_or(0, |at| at + 1);
    Ok(Through::First { count, of: open.len() })
}

// The task's commits not yet on the default branch, oldest first; a push of the first commits counts from the first of them, so one pushed already and still on the branch is never counted again.
fn open_shas(path: &Path, upstream: &str) -> Result<Vec<String>> {
    Ok(super::accept::open_commits(path, upstream)?.into_iter().map(|open| open.sha).collect())
}

// The task's Claude keeps committing while a push of its first commits waits, so only fewer commits than were chosen stop it; the line checks that the chosen ones are unchanged.
fn through_sha(path: &Path, upstream: &str, through: Through) -> Result<String> {
    match through {
        Through::All => git(path, &["rev-parse", "HEAD"]),
        Through::First { count, of } => {
            let commits = open_shas(path, upstream)?;
            if count == 0 || count > commits.len() {
                return Notice::new(Tone::You, "Not pushed: its commits changed after you chose push").you("Open push again to see the new ones.")
                    .detail("commits", format!("{} now, {of} when you chose", commits.len())).err();
            }
            Ok(commits[count - 1].clone())
        }
    }
}

// What a push stands on: the whole branch, or for a push of the first commits only those, so the task's Claude may commit after them meanwhile.
pub(super) fn pushed_fingerprint(path: &Path, upstream: &str, through: Through) -> Result<String> {
    match through {
        Through::All => fingerprint(path, upstream),
        Through::First { .. } => fingerprint_at(path, upstream, &through_sha(path, upstream, through)?),
    }
}

fn authorize(caller: &Caller, repo: &Path, slot: &Slot, upstream: &str, through: Through) -> Result<&'static str> {
    authorize_as(caller, repo, slot, upstream, through, "Not pushed")
}

pub(super) fn authorize_as(caller: &Caller, repo: &Path, slot: &Slot, upstream: &str, through: Through, refused: &str) -> Result<&'static str> {
    let task = task_of(slot);
    let address = peers::address(task, &name_of(repo));
    let head = git(&slot.path, &["rev-parse", "HEAD"])?;
    let sha = through_sha(&slot.path, upstream, through)?;
    let shown = crate::review::shown(&address, &slot.path, upstream);
    let covered = |reviewed: &str| same_code(&slot.path, upstream, reviewed, &head) || same_code(&slot.path, upstream, reviewed, &sha);
    crate::review::gate(shown.as_ref(), covered).map_err(|why| Notice::new(Tone::NotNow, format!("{refused}: {why}")).detail("task", task).to_string())?;
    let check = Pusher { caller: caller.clone(), permit: crate::permit::of(&address, &name_of(repo)), reviewed: shown.as_ref().is_some_and(|shown| !shown.pending()), approved: approved(&slot.path, upstream, &address) };
    check.allowed().map_err(|why| Notice::new(Tone::Failed, format!("{refused}: {} may not push it", crate::permit::describe(&check.caller))).detail("task", task).detail("why", why).to_string())
}

fn nothing_to_push<T>(task: &str) -> Result<T> {
    Notice::new(Tone::NotNow, "Nothing to push: it has no commits").detail("task", task).err()
}

pub(super) fn push_task(caller: &Caller, repo: &Path, slot: &Slot, through: Through) -> Result<String> {
    let state = state_dir()?;
    let address = peers::address(task_of(slot), &name_of(repo));
    join(&state, caller, repo, slot, through)?;
    let read = super::line::live_read();
    let mut push = live_push(&state);
    let driven = super::line::drive(&state, repo, Some(&address), &messages::load(), messages::now(), &read, &mut push);
    driven.mine.unwrap_or_else(|| queued_notice(&state, &address, task_of(slot)))
}

pub(super) fn live_push(state: &Path) -> impl FnMut(&Intent, &Slot) -> Result<Sent> + '_ {
    move |intent: &Intent, slot: &Slot| {
        push_task_with(state, &intent.by.caller(), &intent.repo, slot, intent.through, intent.queued, &hand_over, &mut |pusher, out| deliver(&intent.repo, slot, &default_branch(&intent.repo)?, intent.through, out, pusher, state, &crate::ci::begin))
    }
}

pub(super) fn queued_notice(state: &Path, address: &str, task: &str) -> Result<String> {
    let place = intent::pending(state, address).map(|pending| place_in_line(pending.ahead, pending.after.as_deref())).unwrap_or_else(|| "in line".to_string());
    let place = match intent::load(state, address).and_then(|mine| intent::line(state, &mine.repo).into_iter().next()).filter(|head| head.address != address).and_then(|head| head_waits(head.work).map(|what| (head, what))) {
        Some((_, what)) if intent::pending(state, address).is_some_and(|pending| pending.ahead == 1) => format!("{place}, which waits for its Claude to {what}"),
        Some((head, what)) => format!("{place}; {} at the head waits for its Claude to {what}", head.task),
        None => place,
    };
    Notice::new(Tone::Moving, format!("In the push line: {place}")).next("kebbie rebases and pushes it when its turn comes; the pushes before it go out first.").detail("task", task).ok()
}

fn head_waits(work: Work) -> Option<&'static str> {
    match work {
        Work::Turn | Work::Ci => None,
        Work::Tests => Some("run the tests for files main changed under it"),
        Work::Conflict => Some("resolve a conflict"),
    }
}

pub(crate) fn place_in_line(ahead: usize, after: Option<&str>) -> String {
    match (ahead, after) {
        (0, _) => "next in line".to_string(),
        (ahead, Some(after)) => format!("{} in line after {after}", ordinal(ahead + 1)),
        (ahead, None) => format!("{} in line", ordinal(ahead + 1)),
    }
}

fn ordinal(number: usize) -> String {
    let suffix = match (number % 10, number % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{number}{suffix}")
}

// What a caller may push is checked here, once, and again when its turn comes; a push already in line keeps its place.
pub(super) fn join(state: &Path, caller: &Caller, repo: &Path, slot: &Slot, through: Through) -> Result<()> {
    let task = task_of(slot).to_string();
    let upstream = default_branch(repo)?;
    let address = peers::address(&task, &name_of(repo));
    if ahead_behind(&slot.path, &upstream)?.0 == 0 {
        return nothing_to_push(&task);
    }
    if through == Through::All {
        committed(&slot.path, &task)?;
    }
    super::stage::refuse_push(state, repo, &address, &task)?;
    if rebase_in_progress(&slot.path) {
        return Notice::new(Tone::NotNow, "Not pushed: a rebase is under way in its worktree").next("Push it once the rebase is finished.").detail("task", &task).err();
    }
    authorize(caller, repo, slot, &upstream, through)?;
    let by = By::of(caller).ok_or_else(|| Notice::new(Tone::Failed, format!("Not pushed: {} may not push it", crate::permit::describe(caller))).detail("task", &task).to_string())?;
    let waiting = intent::load(state, &address);
    if waiting.is_some() && *caller != Caller::User {
        return Notice::new(Tone::NotNow, "Already being pushed").next("It is in the push line; kebbie pushes it when its turn comes.").detail("task", &task).err();
    }
    let now = messages::now();
    let seen = messages::latest_reports(&messages::load()).get(&address).map(|report| report.id.clone());
    let queued = waiting.and_then(|waiting| waiting.queued).unwrap_or(now);
    intent::save(state, &Intent { by, work: Work::Turn, address: address.clone(), repo: repo.to_path_buf(), task, fingerprint: pushed_fingerprint(&slot.path, &upstream, through)?, through, at: now, seen, hand: None, queued: Some(queued) })?;
    intent::forget_left(state, &address);
    let ahead = intent::line(state, repo).iter().position(|queued| queued.address == address).unwrap_or(0);
    crate::phase::record(state, &address, by.key(), &crate::phase::Event::PushQueued { ahead });
    Ok(())
}

pub fn cancel(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    cancel_as(&state_dir()?, &crate::permit::caller(), &repo, &slot)
}

pub(super) fn cancel_as(state: &Path, caller: &Caller, repo: &Path, slot: &Slot) -> Result<String> {
    let task = task_of(slot);
    let address = peers::address(task, &name_of(repo));
    let by = By::of(caller).ok_or_else(|| Notice::new(Tone::Failed, format!("Not taken out: {} may not change a push", crate::permit::describe(caller))).detail("task", task).to_string())?;
    let Some(waiting) = intent::load(state, &address) else {
        return Notice::new(Tone::NotNow, "Not in the push line").detail("task", task).err();
    };
    if waiting.work == Work::Ci {
        return Notice::new(Tone::NotNow, "Not taken out: its commits are on main").you("Fix the red CI, or dismiss it.").detail("task", task).err();
    }
    if by == By::Lead && waiting.by == By::User {
        return Notice::new(Tone::Failed, "Not taken out: the user asked for this push").you("Ask the user; only they take their push back.").detail("task", task).err();
    }
    let Some(_turnstile) = super::line::Turnstile::try_take(state, repo) else {
        return Notice::new(Tone::NotNow, "Not taken out: kebbie is pushing it now").next("Try again in a moment; if it went out, it is under CI running.").detail("task", task).err();
    };
    if intent::load(state, &address).is_none() {
        return Notice::new(Tone::NotNow, "Not in the push line any more").detail("task", task).err();
    }
    leave_line(state, repo, slot, &address);
    if let Err(error) = messages::log_for_lead(&format!("{} took {address} out of the push line before it went out; nothing of it was pushed.", capitalized(by.word())), Some("push")) {
        eprintln!("Could not tell the lead: {error}");
    }
    Notice::new(Tone::Done, "Taken out of the push line").next("Nothing was pushed; it waits under Ready to push.").detail("task", task).ok()
}

pub(super) fn leave_line(state: &Path, repo: &Path, slot: &Slot, address: &str) {
    intent::forget(state, address);
    let head = git(&slot.path, &["rev-parse", "HEAD"]).ok();
    crate::phase::update(state, address, "push", |current| matches!(current, Some(crate::phase::Phase::Pushing { .. })).then_some(crate::phase::Phase::Ready { head }));
    super::line::restamp(state, repo);
}

const PUSH_TRIES: usize = 3;

// What a push turn ended in: out on the default branch, or with the task's Claude, the push waiting at the head of its line.
#[derive(Debug, PartialEq)]
pub(super) enum Sent {
    Pushed(String),
    Handed(String),
}

// Someone else's push can land between the fetch and this push; then it fetches, rebases and decides again, so shared files still go to the task's Claude.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_task_with(state: &Path, caller: &Caller, repo: &Path, slot: &Slot, through: Through, queued: Option<u64>, hand_over: HandOver, send: &mut dyn FnMut(&'static str, &Out) -> Result<String>) -> Result<Sent> {
    let mut tried = 0;
    loop {
        tried += 1;
        match prepare(state, caller, repo, slot, through, queued, hand_over)? {
            Prepared::Handed(said) => return Ok(Sent::Handed(said)),
            Prepared::Push(pusher, out) => match send(pusher, &out) {
                Err(error) if rejected(&error) && tried < PUSH_TRIES => {}
                sent => return sent.map(Sent::Pushed),
            },
        }
    }
}

// The default branch moved on the remote after kebbie fetched it.
pub(super) fn rejected(error: &str) -> bool {
    ["non-fast-forward", "(fetch first)", "stale info"].iter().any(|sign| error.contains(sign))
}

// What goes out: sha, and whether kebbie rebased it elsewhere, so the task's branch still holds the commits under their old shas.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Out {
    pub sha: String,
    pub rebased: bool,
}

#[derive(Debug, PartialEq)]
pub(super) enum Prepared {
    Push(&'static str, Out),
    Handed(String),
}

pub(super) type HandOver<'a> = &'a dyn Fn(&Slot, &str) -> std::result::Result<(), String>;

// Only the head of a repo's line gets here: it fetches and rebases onto origin's default branch, never the local one; a push of the first commits rebases them elsewhere and leaves the task's worktree alone.
fn prepare(state: &Path, caller: &Caller, repo: &Path, slot: &Slot, through: Through, queued: Option<u64>, hand_over: HandOver) -> Result<Prepared> {
    let task = task_of(slot).to_string();
    let upstream = default_branch(repo)?;
    let address = peers::address(&task, &name_of(repo));
    if ahead_behind(&slot.path, &upstream)?.0 == 0 {
        return nothing_to_push(&task);
    }
    if through == Through::All {
        committed(&slot.path, &task)?;
    }
    super::stage::refuse_push(state, repo, &address, &task)?;
    let pusher = authorize(caller, repo, slot, &upstream, through)?;
    let by = By::of(caller).ok_or_else(|| Notice::new(Tone::Failed, format!("Not pushed: {} may not push it", crate::permit::describe(caller))).detail("task", &task).to_string())?;
    fetch(repo)?;
    let recorded = pushed_fingerprint(&slot.path, &upstream, through)?;
    let seen = messages::latest_reports(&messages::load()).get(&address).map(|report| report.id.clone());
    let record = |work: Work, fingerprint: String| intent::save(state, &Intent { by, work, address: address.clone(), repo: repo.to_path_buf(), task: task.clone(), fingerprint, through, at: messages::now(), seen: seen.clone(), hand: None, queued });
    let (updated, out) = match through {
        Through::All => (update(&slot.path, &upstream)?, None),
        Through::First { .. } => {
            let sha = through_sha(&slot.path, &upstream, through)?;
            if crate::sh::succeeds("git", &["merge-base", "--is-ancestor", &upstream, &sha], Some(&slot.path)) {
                (Updated::Clean, Some(Out { sha, rebased: false }))
            } else {
                let first = open_shas(&slot.path, &upstream)?.into_iter().next().unwrap_or_else(|| sha.clone());
                let (updated, built) = rebased_elsewhere(&slot.path, &format!("{first}^"), &sha, &upstream)?;
                (updated, Some(Out { sha: built, rebased: true }))
            }
        }
    };
    let kept = if through == Through::All { String::new() } else { format!(" {LEFT_AS_IT_WAS}") };
    match updated {
        Updated::Clean => {}
        Updated::Conflict => {
            record(Work::Conflict, recorded)?;
            let why = format!("{} asked kebbie to push this task", capitalized(pusher));
            let prompt = format!("{}{kept} {WAITING_BEHIND}", conflict_prompt(&why, &upstream));
            if let Err(cannot) = hand_over(slot, &prompt).and_then(|_| intent::note_hand_over(state, &address)) {
                intent::forget(state, &address);
                return Notice::new(Tone::Failed, format!("Not pushed: it conflicts with {}, and kebbie could not pass the rebase to its Claude", main_of(&upstream)))
                    .you("Push again; if this repeats, rebase it yourself in its worktree.").detail("task", &task).detail("folder", slot.path.display().to_string()).detail("error", cannot).err();
            }
            crate::phase::record(state, &address, by.key(), &crate::phase::Event::PushHanded);
            tell_lead(&format!("{} pushed {address}; rebasing onto {upstream} conflicted, so its Claude resolves it as part of the push. When it reports ready, the reviewer checks the resolved commits against the rules and kebbie pushes them for {} then; you hear only of a break, or when the reviewer gives no verdict. The task keeps the head of its repo's push line meanwhile, for up to {} minutes.", capitalized(pusher), by.word(), super::line::CEILING / 60), Some("push"));
            return Ok(Prepared::Handed(Notice::new(Tone::Moving, format!("Not pushed yet: it conflicts with {}", main_of(&upstream)))
                .next(format!("Its Claude resolves the conflict, the reviewer checks the result against the rules, then kebbie pushes it{}.", for_whom(pusher))).detail("task", &task).to_string()));
        }
        Updated::Overlap(touched) => {
            let files: Vec<&str> = touched.iter().map(|touched| touched.file.as_str()).collect();
            record(Work::Tests, pushed_fingerprint(&slot.path, &upstream, through)?)?;
            if let Err(cannot) = hand_over(slot, &overlap_prompt(by, &upstream, &touched, through)).and_then(|_| intent::note_hand_over(state, &address)) {
                intent::forget(state, &address);
                return Notice::new(Tone::Failed, format!("Not pushed: {} changed the same files, and kebbie could not pass their tests to its Claude", main_of(&upstream)))
                    .you("Run the tests for those files in its worktree, then push again.").detail("task", &task).detail("files", files.join(" · "))
                    .detail("folder", slot.path.display().to_string()).detail("error", cannot).err();
            }
            crate::phase::record(state, &address, by.key(), &crate::phase::Event::PushHanded);
            return Ok(Prepared::Handed(Notice::new(Tone::Moving, format!("Not pushed yet: {} changed the same files, so their tests run first", main_of(&upstream)))
                .next(format!("Its Claude runs those tests now; kebbie pushes it{} as soon as it reports ready.", for_whom(pusher))).detail("task", &task).detail("files", files.join(" · ")).to_string()));
        }
    }
    intent::forget(state, &address);
    let pusher = authorize(caller, repo, slot, &upstream, through)?;
    let out = match out {
        Some(out) => out,
        None => Out { sha: git(&slot.path, &["rev-parse", "HEAD"])?, rebased: false },
    };
    Ok(Prepared::Push(pusher, out))
}

pub(super) type Watch<'a> = &'a dyn Fn(&Path, &Path, &str, &str, bool, crate::ci::Pushed) -> crate::ci::Begin;

#[allow(clippy::too_many_arguments)]
fn deliver(repo: &Path, slot: &Slot, upstream: &str, through: Through, out: &Out, pusher: &'static str, state: &Path, watch: Watch) -> Result<String> {
    let task = task_of(slot).to_string();
    let address = peers::address(&task, &name_of(repo));
    let sha = out.sha.clone();
    let keep_open = matches!(through, Through::First { .. });
    let rest = if keep_open { git(&slot.path, &["rev-list", "--count", &format!("{}..HEAD", through_sha(&slot.path, upstream, through)?)])?.parse::<usize>().unwrap_or(0) } else { 0 };
    let pushed = git(&slot.path, &["log", "--reverse", "--format=%h %s", &format!("{upstream}..{sha}")])?;
    let note = started_file(&task, repo).ok().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| crate::task_note::items(&text).into_iter().next());
    let target = upstream.trim_start_matches("origin/").to_string();
    let main = main_of(upstream);
    let told_ci = crate::ci::Pushed { pusher: pusher.to_string(), target: target.clone(), commits: pushed.clone(), note: note.clone(), logged: None };
    let before = crate::ci::load(state, &address);
    let phase_before = crate::phase::load(state, &address);
    let watch = watch(state, repo, &address, &sha, !keep_open, told_ci);
    if !keep_open && matches!(watch, crate::ci::Begin::Watching) {
        crate::phase::record(state, &address, "push", &crate::phase::Event::PushOut { sha: &sha });
    }
    super::plan::seen_in(state, &slot.path);
    let count = match push_prefix(&slot.path, upstream, &sha, state) {
        Ok(count) => count,
        Err(error) => {
            crate::ci::put_back(state, &address, before);
            crate::phase::put_back(state, &address, &crate::phase::Phase::CiRunning { sha: sha.clone() }, phase_before);
            return Notice::new(Tone::Failed, "Not pushed: git refused the push").detail("task", &task).detail("error", error).err();
        }
    };
    super::landed::keep_open(state, &address, keep_open);
    let vault = crate::vault_cli::detect();
    let log = vault.as_ref().map(|cli| crate::task_note::log_push(cli, &slot.path, &task, &pushed));
    let logged = log.as_ref().map(crate::task_note::logged_text).unwrap_or_default();
    if let Some(cli) = vault.as_ref() {
        crate::task_note::heartbeat(cli, state, &address);
    }
    let landed = |closed: bool, waits: bool| Landed { pusher, address: &address, target: &target, commits: &pushed, closed, note: note.as_deref(), logged: log.as_ref().map(|log| matches!(log, Ok(Some(_)))), waits };
    let short = &sha[..7.min(sha.len())];
    if matches!(watch, crate::ci::Begin::Watching) {
        crate::ci::arm(state, &address, log.as_ref().map(|log| matches!(log, Ok(Some(_)))));
    }
    let ci = match &watch {
        crate::ci::Begin::Watching => format!("kebbie watches {short}"),
        crate::ci::Begin::NotWatched(reason) => format!("not watched: {reason}"),
        crate::ci::Begin::Off => String::new(),
    };
    let headline = |through: bool| if through { format!("Pushed {} through {short} to {main}", plural(count, "commit")) } else { format!("Pushed {} to {main}", plural(count, "commit")) };
    let said = |notice: Notice| notice.detail("task", &task).detail("commit", short).detail("ci", &ci).detail("log", &logged);
    if !keep_open {
        if matches!(watch, crate::ci::Begin::Watching) {
            if let Err(error) = messages::log_for_lead(&format!("{} pushed {address}: {} to {target}. kebbie watches CI for {short} and closes the task once every run is green; you hear what landed then.", capitalized(pusher), plural(count, "commit")), None) {
                eprintln!("Could not tell the lead about the push: {error}");
            }
            let notice = Notice::new(Tone::Done, headline(false)).next("CI runs now; kebbie closes the task when it is green.").detail("task", &task).detail("commit", short).detail("log", &logged);
            return Ok(notice.detail("checkout", follow_upstream(repo, upstream, &super::stage::load(state, repo))?).to_string());
        }
        let ready = note_ready(vault.as_ref(), state, &address, &task);
        if let Err(waits) = &ready {
            crate::phase::record(state, &address, "push", &crate::phase::Event::Closing);
            tell_lead_pushed(&landed(false, true));
            let waits = Notice::read(waits, false);
            let mut notice = said(Notice::new(Tone::You, headline(false)).next(waits.next.unwrap_or_default()).you(waits.you.unwrap_or_default()));
            notice.details.extend(waits.details.into_iter().filter(|(label, _)| label != "task"));
            return Ok(notice.to_string());
        }
        let closed = close_task(repo, Some(&task), vault.as_ref(), ready.unwrap_or_default(), Some(&pushed));
        if closed.is_err() {
            crate::phase::record(state, &address, "push", &crate::phase::Event::PushFailed);
        }
        tell_lead_pushed(&landed(closed.is_ok(), false));
        return Ok(match closed {
            Ok(closed) => {
                let mut notice = said(Notice::new(Tone::Done, format!("{} and closed the task", headline(false))).next(closed.next.unwrap_or_default()));
                notice.details.extend(closed.details);
                notice.to_string()
            }
            Err(error) => said(Notice::new(Tone::You, format!("{}, but the task did not close", headline(false)))).you("Close it once the reason below is settled.").detail("error", error).to_string(),
        });
    }
    tell_lead_pushed(&landed(false, false));
    crate::report::forget_card_in(state, &address);
    let clean = !out.rebased;
    crate::phase::record(state, &address, "push", &crate::phase::Event::KeptOpen { rest, clean, head: git(&slot.path, &["rev-parse", "HEAD"]).ok().as_deref() });
    let told = if clean {
        format!(
            "{} pushed {} of this task, through {short}, to {target}; they are on {target} for good. Your branch continues from there with {} on top: reshape only those.",
            capitalized(pusher), plural(count, "commit"), plural(rest, "commit")
        )
    } else {
        format!(
            "{} pushed {} of this task to {target}, rebased onto it as {short}; they are on {target} for good. kebbie changed nothing in your worktree, so your branch still holds them under their old shas: \
             once your work in progress is committed, at a point of your choosing, run git rebase {upstream} (already fetched, no -i), which drops the pushed ones as already on {target}. \
             Do not push. Reshape only the {} after them.",
            capitalized(pusher), plural(count, "commit"), plural(rest, "commit")
        )
    };
    if let Err(error) = messages::notify(&address, &told) {
        eprintln!("Could not tell {address} about the push: {error}");
    }
    let after = if clean && rest == 0 { "It stays open, as you chose, with everything on main; close it from its options when you are done.".to_string() } else if clean { format!("It stays open with {} on top.", plural(rest, "commit")) } else { format!("It stays open; its Claude rebases the rest onto {main} when it is ready, and kebbie changed nothing in its worktree.") };
    Ok(said(Notice::new(Tone::Done, headline(true)).next(after)).detail("checkout", follow_upstream(repo, upstream, &super::stage::load(state, repo))?).to_string())
}

fn tell_lead(text: &str, kind: Option<&str>) {
    if let Err(error) = messages::tell_lead(text, kind) {
        eprintln!("Could not tell the lead: {error}");
    }
}

pub fn after_ci(repo: &Path, task: &str, address: &str, told: &crate::ci::Pushed) -> Result<String> {
    let state = state_dir()?;
    let vault = crate::vault_cli::detect();
    let landed = |closed: bool, waits: bool| Landed { pusher: &told.pusher, address, target: &told.target, commits: &told.commits, closed, note: told.note.as_deref(), logged: told.logged, waits };
    let ready = note_ready(vault.as_ref(), &state, address, task);
    if let Err(waits) = &ready {
        crate::phase::record(&state, address, "ci", &crate::phase::Event::Closing);
        tell_lead_pushed(&landed(false, true));
        return Ok(waits.clone());
    }
    let closed = close_task(repo, Some(task), vault.as_ref(), ready.unwrap_or_default(), Some(&told.commits));
    if closed.is_err() {
        crate::phase::record(&state, address, "ci", &crate::phase::Event::PushFailed);
    }
    tell_lead_pushed(&landed(closed.is_ok(), false));
    closed.map(|closed| closed.detail("task", task).to_string())
}

fn for_whom(pusher: &str) -> String {
    if pusher == "the user" { String::new() } else { format!(" for {pusher}") }
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map(|first| first.to_uppercase().chain(characters).collect()).unwrap_or_default()
}

// Who may push this task now: the user always, the lead only with the user's permission and a rules approval, a task or any other Claude never.
struct Pusher {
    caller: crate::permit::Caller,
    permit: Option<crate::permit::Permit>,
    reviewed: bool,
    approved: bool,
}

impl Pusher {
    fn allowed(&self) -> std::result::Result<&'static str, String> {
        use crate::permit::Caller;
        match &self.caller {
            Caller::User => Ok("the user"),
            Caller::Lead => {
                crate::permit::lead_may_push(self.permit, self.reviewed)?;
                if !self.approved {
                    return Err("its commits have no rules approval that matches them: wait for the reviewer's rules check, or check them and run kebbie approve".to_string());
                }
                Ok("the lead")
            }
            Caller::LeadSandboxed => Err(format!("{} cannot push", crate::permit::describe(&Caller::LeadSandboxed))),
            Caller::Task(_) | Caller::Agent | Caller::Sandboxed => Err("no agent pushes; the user pushes from the board, or the lead for a task the user gave push permission".to_string()),
        }
    }
}

struct Landed<'a> {
    pusher: &'a str,
    address: &'a str,
    target: &'a str,
    commits: &'a str,
    closed: bool,
    note: Option<&'a str>,
    logged: Option<bool>,
    waits: bool,
}

// logged is None without the vault, else whether kebbie wrote the repo's log line itself; with the vault kebbie settles the note.
fn pushed_notice(landed: &Landed) -> String {
    let Landed { pusher, address, target, commits, closed, note, logged, waits } = *landed;
    let count = commits.lines().count();
    let mut text = format!("{} pushed {address}: {} to {target} ({}).", capitalized(pusher), plural(count, "commit"), commits.lines().collect::<Vec<_>>().join("; "));
    text.push_str(match (closed, waits) {
        (true, _) => " The task is done and closed.",
        (false, true) => " The task stays open until its note has its solution and decisions; kebbie asked it to write them.",
        (false, false) => " The task stays open with the rest of its branch.",
    });
    match logged {
        Some(true) => return text + " kebbie wrote the repo's log line in the vault from these commits.",
        Some(false) => return text + " kebbie could not write the repo's log line: write it in the vault with what landed and these shas.",
        None => {}
    }
    text.push_str(" Write the repo's log line in the vault with what landed and these shas");
    match (note, closed) {
        (Some(note), true) => text.push_str(&format!(", and close or update its note {note}.")),
        (Some(note), false) => text.push_str(&format!(", and note in {note} what landed.")),
        (None, true) => text.push_str(", and close or update the task's note if it has one."),
        (None, false) => text.push('.'),
    }
    text
}

// Only a push kebbie logged itself leaves the lead nothing to do now; without that, the notice asks it to write the log line.
fn tell_lead_pushed(landed: &Landed) {
    let told = if landed.logged != Some(true) { messages::tell_lead(&pushed_notice(landed), Some("push")) } else { messages::log_for_lead(&pushed_notice(landed), Some("push")) };
    if let Err(error) = told {
        eprintln!("Could not tell the lead about the push: {error}");
    }
}

// One of the task's commits not yet on upstream; a full push names HEAD, so a commit that arrived after the dialog opened is never pushed unseen.
fn pushable_through(path: &Path, upstream: &str, rev: &str, keep_open: bool) -> Result<String> {
    let sha = git(path, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).map_err(|_| Notice::new(Tone::Failed, "Not pushed: that is not a commit").detail("commit", rev).to_string())?;
    if !open_shas(path, upstream)?.contains(&sha) {
        return Notice::new(Tone::Failed, "Not pushed: that commit is not one of the task's commits to push").detail("commit", &sha[..7.min(sha.len())]).err();
    }
    if !keep_open && sha != git(path, &["rev-parse", "HEAD"])? {
        return Notice::new(Tone::You, "Not pushed: the branch has commits you were not shown").you("Open push again to see them.").detail("after", &sha[..7.min(sha.len())]).err();
    }
    Ok(sha)
}

// Fast-forwards the default branch on origin to sha; the remote-tracking branch follows.
pub(super) fn push_prefix(path: &Path, upstream: &str, sha: &str, state: &Path) -> Result<usize> {
    let count = git(path, &["rev-list", "--count", &format!("{upstream}..{sha}")])?.parse::<usize>().map_err(|error| error.to_string())?;
    let target = format!("{sha}:refs/heads/{}", upstream.trim_start_matches("origin/"));
    crate::repo::push_from(path, &[&target], state)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::test_support::*;
    use crate::repo::{ahead_behind, branch_exists, git};
    use crate::tasks::approval::{approval_file, covers, fingerprint, record_approval};

    #[test]
    fn only_the_user_pushes_freely_the_lead_with_permission_and_its_approval_and_no_agent_at_all() {
        use crate::permit::{Caller, Permit};
        let check = |caller: Caller, permit: Option<Permit>, reviewed: bool, approved: bool| Pusher { caller, permit, reviewed, approved }.allowed();
        assert_eq!(check(Caller::User, None, false, false), Ok("the user"));
        assert_eq!(check(Caller::Lead, Some(Permit::Task), false, true), Ok("the lead"));
        assert!(check(Caller::Lead, None, true, true).unwrap_err().contains("no push permission"));
        assert!(check(Caller::Lead, Some(Permit::Task), true, false).unwrap_err().contains("kebbie approve"));
        assert!(check(Caller::Lead, Some(Permit::Repo), false, true).unwrap_err().contains("reviewer passed"));
        assert_eq!(check(Caller::Lead, Some(Permit::Repo), true, true), Ok("the lead"));
        for agent in [Caller::Task("billing@app".to_string()), Caller::Agent] {
            assert!(check(agent, Some(Permit::Task), true, true).unwrap_err().contains("no agent pushes"), "a permission is the lead's, never a task's");
        }
    }

    #[test]
    fn after_a_push_the_lead_hears_what_landed_and_is_cued_to_log_it_and_settle_the_note() {
        let commits = "a1b2c3d Add refunds\ne4f5a6b Show refunds on the invoice";
        let done = pushed_notice(&Landed { pusher: "the lead", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing-refunds.md"), logged: None, waits: false });
        assert_eq!(done, "The lead pushed billing@app: 2 commits to main (a1b2c3d Add refunds; e4f5a6b Show refunds on the invoice). The task is done and closed. Write the repo's log line in the vault with what landed and these shas, and close or update its note Projects/billing-refunds.md.");
        let open = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits: "a1b2c3d Add refunds", closed: false, note: None, logged: None, waits: false });
        assert!(open.starts_with("The user pushed billing@app: 1 commit to main (a1b2c3d Add refunds). The task stays open"), "{open}");
        assert!(open.ends_with("these shas."), "{open}");
    }

    #[test]
    fn with_the_vault_on_the_lead_hears_kebbie_logged_the_push_and_is_not_asked_to_log_it() {
        let commits = "a1b2c3d Add refunds";
        let kept = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing.md"), logged: Some(true), waits: false });
        assert!(kept.contains("kebbie wrote the repo's log line") && !kept.contains("Write the repo's log line"), "{kept}");
        assert!(!kept.contains("area note") && !failed_text(commits).contains("area note"), "kebbie logs the push itself; the notice carries no chore for the lead's area note: {kept}");
        let waits = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: false, note: Some("Projects/billing.md"), logged: Some(true), waits: true });
        assert!(waits.contains("stays open until its note has its solution and decisions"), "{waits}");
        let failed = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing.md"), logged: Some(false), waits: false });
        assert!(failed.contains("kebbie could not write the repo's log line: write it in the vault with what landed and these shas") && !failed.contains("kebbie wrote"), "a log append that failed leaves the line to the lead: {failed}");
    }

    fn failed_text(commits: &str) -> String {
        pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: None, logged: Some(false), waits: false })
    }

    #[test]
    fn a_push_notice_reaches_the_lead_live_only_when_kebbie_could_not_write_the_log_line() {
        let source = include_str!("push.rs");
        let tell = &source[source.find("fn tell_lead_pushed").unwrap()..];
        let tell = &tell[..tell.find("\n}\n").unwrap()];
        assert!(tell.contains("landed.logged != Some(true) { messages::tell_lead(") && tell.contains("messages::log_for_lead(&pushed_notice(landed), Some(\"push\"))"), "{tell}");
        let deliver = &source[source.find("\nfn deliver(").unwrap()..];
        assert!(deliver[..deliver.find("\n}\n").unwrap()].contains("messages::log_for_lead(&format!(\"{} pushed {address}: {} to {target}. kebbie watches CI"), "the push under CI watch is a notice for the digest, not a turn of the lead's");
    }

    #[test]
    fn push_takes_one_of_the_tasks_own_commits_and_a_full_push_only_up_to_head() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let fifth = commit(&slot.path, "e", "fifth");
        assert_eq!(pushable_through(&slot.path, &upstream, &fourth, true).unwrap(), fourth);
        assert_eq!(pushable_through(&slot.path, &upstream, &fourth[..10], true).unwrap(), fourth);
        assert_eq!(pushable_through(&slot.path, &upstream, &fifth, false).unwrap(), fifth);
        let unseen = pushable_through(&slot.path, &upstream, &fourth, false).unwrap_err();
        assert!(unseen.contains("you were not shown"), "a commit that arrived after the dialog opened is never pushed unseen: {unseen}");
        assert!(pushable_through(&slot.path, &upstream, &upstream, true).unwrap_err().contains("not one of the task's commits"));
        let elsewhere = commit(&dir, "x", "on main only");
        assert!(pushable_through(&slot.path, &upstream, &elsewhere, true).unwrap_err().contains("not one of the task's commits"));
        assert!(pushable_through(&slot.path, &upstream, "no-such-ref", true).unwrap_err().contains("is not a commit"));
        cleanup(dir, origin, &slot);
    }

    fn worktrees_of(dir: &std::path::Path) -> usize {
        git(dir, &["worktree", "list", "--porcelain"]).unwrap().lines().filter(|line| line.starts_with("worktree ")).count()
    }

    #[test]
    fn a_push_of_the_first_commits_rebases_them_elsewhere_and_never_touches_the_tasks_worktree_or_its_edits() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let fifth = commit(&slot.path, "e", "fifth");
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        std::fs::write(slot.path.join("e"), "the next step, half done").unwrap();
        let through = chosen(&slot, &upstream, Some(&fourth), true).unwrap();
        join(&pushing.state, &Caller::User, &dir, &slot, through).unwrap();
        let (texts, hand) = handed();
        let Prepared::Push(pusher, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, through, None, &hand).unwrap() else { panic!("a clean rebase pushes") };
        assert!(out.rebased && out.sha != fourth && pusher == "the user" && texts.borrow().is_empty(), "{out:?}");
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), fifth, "HEAD stays where its Claude left it");
        assert_eq!(std::fs::read_to_string(slot.path.join("e")).unwrap(), "the next step, half done", "and so does its uncommitted work");
        assert_eq!(worktrees_of(&dir), 2, "kebbie's own worktree is gone again");
        let state = state_of(&dir);
        let said = deliver(&dir, &slot, &upstream, through, &out, "the user", &state, &crate::ci::begin).unwrap();
        assert!(said.contains("kebbie changed nothing in its worktree"), "{said}");
        let branch = upstream.trim_start_matches("origin/");
        assert_eq!(git(&origin, &["log", "--format=%s", "-2", branch]).unwrap(), "fourth\nsomeone else's work", "the first commit went out on top of main");
        assert_eq!((git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), std::fs::read_to_string(slot.path.join("e")).unwrap()), (fifth.clone(), "the next step, half done".to_string()), "the push left the worktree alone too");
        assert!(branch_exists(&dir, "billing"), "the task stays open");
        git(&slot.path, &["checkout", "--quiet", "--", "e"]).unwrap();
        git(&slot.path, &["rebase", "--quiet", &upstream]).unwrap();
        assert_eq!(git(&slot.path, &["log", "--format=%s", &format!("{upstream}..HEAD")]).unwrap(), "fifth", "its Claude's own rebase drops the pushed commit");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn after_a_push_of_the_first_commits_rebased_elsewhere_only_the_rest_is_listed_and_pushed_before_its_claude_rebases() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let fifth = commit(&slot.path, "e", "fifth");
        let sixth = commit(&slot.path, "f", "sixth");
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        let through = chosen(&slot, &upstream, Some(&fourth), true).unwrap();
        let (_, hand) = handed();
        let Prepared::Push(_, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, through, None, &hand).unwrap() else { panic!("a clean rebase pushes") };
        deliver(&dir, &slot, &upstream, through, &out, "the user", &state_of(&dir), &crate::ci::begin).unwrap();
        assert_eq!(git(&slot.path, &["rev-list", "--count", &format!("{upstream}..HEAD")]).unwrap(), "3", "its Claude has not rebased: the branch still holds fourth");
        let listed: Vec<String> = crate::tasks::commits(&slot.path).unwrap().into_iter().map(|commit| commit.sha).collect();
        assert_eq!(listed, [fifth.clone(), sixth.clone()], "the push window lists only the rest");
        super::super::accept::set_in(&pushing.state, &pushing.address, &super::super::accept::open_commits(&slot.path, &upstream).unwrap()[0].key, true).unwrap();
        let accepted = crate::tasks::accepted_listing_at(&slot.path).unwrap();
        assert_eq!((accepted["commits"].as_array().unwrap().iter().map(|commit| commit["sha"].as_str().unwrap()).collect::<Vec<_>>(), accepted["last_accepted"].as_str()), (vec![fifth.as_str(), sixth.as_str()], Some(fifth.as_str())), "kebbie accepted too");
        assert!(pushable_through(&slot.path, &upstream, &fourth, true).unwrap_err().contains("not one of the task's commits"), "the pushed one is not pushed again");
        let rest = chosen(&slot, &upstream, Some(&fifth), true).unwrap();
        assert_eq!(rest, Through::First { count: 1, of: 2 }, "the count starts at the first commit really unpushed");
        assert_eq!(through_sha(&slot.path, &upstream, rest).unwrap(), fifth);
        let Prepared::Push(_, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, rest, None, &hand).unwrap() else { panic!("the rest pushes") };
        deliver(&dir, &slot, &upstream, rest, &out, "the user", &state_of(&dir), &crate::ci::begin).unwrap();
        let branch = upstream.trim_start_matches("origin/");
        assert_eq!(git(&origin, &["log", "--format=%s", "-3", branch]).unwrap(), "fifth\nfourth\nsomeone else's work", "fifth went out once, fourth not twice");
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), sixth, "the worktree is still the Claude's");
        let all = chosen(&slot, &upstream, None, false).unwrap();
        assert_eq!(prepare(&pushing.state, &Caller::User, &dir, &slot, all, None, &hand).map(|prepared| matches!(prepared, Prepared::Push(..))), Ok(true));
        assert_eq!(git(&slot.path, &["log", "--format=%s", &format!("{upstream}..HEAD")]).unwrap(), "sixth", "a full push rebases the worktree as ever, and the pushed ones drop out");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_of_the_first_commits_on_top_of_main_goes_out_as_they_are_and_the_branch_continues_from_them() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let fifth = commit(&slot.path, "e", "fifth");
        std::fs::write(slot.path.join("d"), "uncommitted").unwrap();
        let through = chosen(&slot, &upstream, Some(&fourth), true).unwrap();
        let (_, hand) = handed();
        assert_eq!(prepare(&pushing.state, &Caller::User, &dir, &slot, through, None, &hand).unwrap(), Prepared::Push("the user", Out { sha: fourth.clone(), rebased: false }));
        let state = state_of(&dir);
        let said = deliver(&dir, &slot, &upstream, through, &Out { sha: fourth.clone(), rebased: false }, "the user", &state, &crate::ci::begin).unwrap();
        assert!(said.contains("It stays open with 1 commit on top"), "{said}");
        assert_eq!(git(&origin, &["rev-parse", upstream.trim_start_matches("origin/")]).unwrap(), fourth);
        assert_eq!((git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), ahead_behind(&slot.path, &upstream).unwrap()), (fifth, (1, 0)), "nothing to rebase: the rest sits on what was pushed");
        assert_eq!(std::fs::read_to_string(slot.path.join("d")).unwrap(), "uncommitted");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_of_the_first_commits_stands_on_them_alone_so_a_commit_after_them_keeps_it_and_a_changed_one_does_not() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        let through = chosen(&slot, &upstream, Some(&fourth), true).unwrap();
        let chose = pushed_fingerprint(&slot.path, &upstream, through).unwrap();
        commit(&slot.path, "f", "sixth, while it waits");
        assert_eq!(through_sha(&slot.path, &upstream, through).unwrap(), fourth, "a commit after the chosen ones is no reason to stop");
        assert!(covers(&chose, &pushed_fingerprint(&slot.path, &upstream, through).unwrap()));
        assert!(!covers(&chose, &fingerprint(&slot.path, &upstream).unwrap()), "the whole branch moved on");
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--amend", "-m", "sixth, reworded"]).unwrap();
        assert!(covers(&chose, &pushed_fingerprint(&slot.path, &upstream, through).unwrap()), "only the chosen commits count");
        git(&slot.path, &["reset", "--quiet", "--soft", &upstream]).unwrap();
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", "everything at once"]).unwrap();
        assert!(!covers(&chose, &pushed_fingerprint(&slot.path, &upstream, through).unwrap()), "a chosen commit that changed stops the push");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_of_the_first_commits_that_overlaps_or_conflicts_asks_its_claude_to_rebase_and_changes_nothing_itself() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        shared_file(&dir, &slot, &upstream);
        commit(&slot.path, "e", "fifth");
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        std::fs::write(slot.path.join("e"), "half done").unwrap();
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let first = git(&slot.path, &["rev-list", "--reverse", &format!("{upstream}..HEAD")]).unwrap().lines().nth(1).unwrap().to_string();
        let through = chosen(&slot, &upstream, Some(&first), true).unwrap();
        let (texts, hand) = handed();
        assert!(matches!(prepare(&pushing.state, &Caller::User, &dir, &slot, through, None, &hand).unwrap(), Prepared::Handed(said) if said.contains("files: m")));
        let typed = texts.borrow()[0].clone();
        assert!(typed.contains("first 2 commits") && typed.contains("kebbie changed nothing in your worktree") && typed.contains("git rebase origin/") && typed.contains("Do not push"), "{typed}");
        assert_eq!((git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), std::fs::read_to_string(slot.path.join("e")).unwrap()), (head.clone(), "half done".to_string()));
        assert_eq!(intent::load(&pushing.state, &pushing.address).map(|waiting| (waiting.work, waiting.through)), Some((Work::Tests, through)));
        upstream_commit(&dir, "d", "theirs", "d on main");
        assert!(matches!(prepare(&pushing.state, &Caller::User, &dir, &slot, through, None, &hand).unwrap(), Prepared::Handed(said) if said.contains("conflicts with")));
        assert!(texts.borrow()[1].contains("kebbie changed nothing in your worktree") && texts.borrow()[1].contains("resolve the conflict in them"), "{}", texts.borrow()[1]);
        assert_eq!((git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebase_in_progress(&slot.path), worktrees_of(&dir)), (head, false, 2), "the conflict was kebbie's own, aborted elsewhere");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_kept_open_stays_open_even_through_the_last_commit_and_a_plain_push_closes() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let head = commit(&slot.path, "e", "fifth");
        assert_eq!(chosen(&slot, &upstream, Some(&head), true).unwrap(), Through::First { count: 2, of: 2 }, "p on the last commit keeps it open, as the user chose");
        assert_eq!(chosen(&slot, &upstream, Some(&head), false).unwrap(), Through::All, "y pushes all and closes");
        assert_eq!(chosen(&slot, &upstream, None, false).unwrap(), Through::All);
        cleanup(dir, origin, &slot);
    }

    // A file both main and the task change, apart, so the rebase is clean but the files overlap.
    fn shared_file(dir: &std::path::Path, slot: &crate::tasks::Slot, upstream: &str) {
        upstream_commit(dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n", "add m");
        git(&slot.path, &["rebase", "--quiet", upstream]).unwrap();
        std::fs::write(slot.path.join("m"), "one\n2\n3\n4\n5\n6\n7\n8\n9\n10\n").unwrap();
        git(&slot.path, &["commit", "--quiet", "-am", "first line of m"]).unwrap();
    }

    #[test]
    fn a_rebase_over_other_files_is_clean_over_the_same_files_is_an_overlap_and_a_conflict_is_aborted() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Clean);
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0));
        shared_file(&dir, &slot, &upstream);
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        let Updated::Overlap(touched) = update(&slot.path, &upstream).unwrap() else { panic!("main changed m too") };
        assert_eq!(touched.len(), 1);
        assert_eq!(touched[0].file, "m");
        assert!(touched[0].commits.len() == 1 && touched[0].commits[0].ends_with(" last line of m"), "{touched:?}");
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (2, 0), "an overlap is rebased all the same");
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        upstream_commit(&dir, "d", "theirs", "d on main");
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Conflict);
        assert!(!rebase_in_progress(&slot.path) && git(&slot.path, &["rev-parse", "HEAD"]).unwrap() == head, "a conflict is aborted and leaves the branch as it was");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_review_open_across_a_rebase_still_shows_only_the_tasks_own_changes() {
        let (dir, origin, slot, upstream) = task_with_a_commit("review");
        let state = state_of(&dir);
        let herdr = state.join("herdr");
        crate::executable::write(&herdr, "#!/bin/sh\necho '{\"result\":{\"pane\":{\"pane_id\":\"p_review\"}}}'\n").unwrap();
        let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/review.sh");
        let review = |args: &[&str]| {
            let output = std::process::Command::new("sh").arg(&script).args(args).current_dir(&slot.path)
                .env("KEBBIE_STATE_DIR", &state).env("HERDR_BIN_PATH", &herdr).env("HERDR_PANE_ID", "p_claude").output().unwrap();
            assert!(output.status.success(), "review.sh {args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        review(&["split"]);
        let d = state.join("reviews/p_claude");
        let files = |rows: &str| rows.lines().skip(1).map(|line| line.split('\t').next().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(files(&review(&["rows", &d.to_string_lossy()])), ["d"]);
        land_on_main(&dir, &upstream, |_| {});
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Clean);
        assert_eq!(files(&review(&["rows", &d.to_string_lossy()])), ["d"], "main's commits stay out of a review opened before the rebase");
        let base = std::fs::read_to_string(d.join("base")).unwrap();
        assert_eq!(review(&["diff", base.trim(), "elsewhere"]), "", "the open diff compares against the fork point after the rebase too");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_rebase_git_refuses_to_start_reports_gits_reason_and_the_hidden_files_and_is_no_conflict() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&slot.path, &["update-index", "--skip-worktree", "a"]).unwrap();
        std::fs::write(slot.path.join("a"), "local only").unwrap();
        assert!(uncommitted(&slot.path).unwrap().is_empty(), "git status does not show it");
        upstream_commit(&dir, "a", "theirs", "a on main");
        let refused = update(&slot.path, &upstream).unwrap_err();
        assert!(refused.contains("would be overwritten") || refused.contains("overwritten"), "git's own reason: {refused}");
        assert!(refused.contains("skip-worktree") && refused.contains("hidden: a ("), "{refused}");
        assert!(!refused.contains("no rebase in progress") && !rebase_in_progress(&slot.path), "{refused}");
        cleanup(dir, origin, &slot);
    }

    struct Pushing {
        state: std::path::PathBuf,
        address: String,
    }

    impl Pushing {
        fn new(dir: &std::path::Path) -> Pushing {
            Pushing { state: crate::lead::state_dir().unwrap(), address: crate::peers::address("billing", &name_of(dir)) }
        }
    }

    impl Drop for Pushing {
        fn drop(&mut self) {
            intent::forget(&self.state, &self.address);
            let _ = crate::permit::set_task_in(&self.state, &self.address, false);
            let _ = std::fs::remove_file(approval_file(&self.state, &self.address));
        }
    }

    type Typed = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    fn handed() -> (Typed, impl Fn(&Slot, &str) -> std::result::Result<(), String>) {
        let texts = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let kept = texts.clone();
        (texts, move |_: &Slot, text: &str| { kept.borrow_mut().push(text.to_string()); Ok(()) })
    }

    #[test]
    fn being_behind_main_never_blocks_a_push_it_rebases_first_and_untracked_files_do_not_block_it() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        std::fs::write(slot.path.join("notes.txt"), "scratch").unwrap();
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 1));
        let (texts, hand) = handed();
        assert_eq!(prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand).unwrap(), Prepared::Push("the user", Out { sha: git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebased: false }));
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0), "rebased, ready for the push");
        assert!(texts.borrow().is_empty(), "nothing for its Claude to do");
        std::fs::write(slot.path.join("d"), "half done").unwrap();
        let dirty = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand).unwrap_err();
        assert!(dirty.contains("1 uncommitted file") && dirty.contains("files: d"), "{dirty}");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn main_moving_while_its_claude_runs_the_tests_and_again_before_the_finishing_push_never_loses_the_push() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        shared_file(&dir, &slot, &upstream);
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        let (texts, hand) = handed();
        assert!(matches!(prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand).unwrap(), Prepared::Handed(_)), "m changed under it: its Claude runs the tests");
        upstream_commit(&dir, "elsewhere", "x", "landed while the tests ran");
        let finish = |race: &dyn Fn()| {
            intent::forget(&pushing.state, &pushing.address);
            let sends = std::cell::Cell::new(0);
            let said = push_task_with(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, &mut |_, out| {
                sends.set(sends.get() + 1);
                if sends.get() == 1 {
                    race();
                }
                push_prefix(&slot.path, &upstream, &out.sha, &pushing.state).map(|count| format!("pushed {count}"))
            });
            (said, sends.get())
        };
        let (said, sends) = finish(&|| upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\nnine\nten\n", "another line of m"));
        assert!(matches!(&said, Ok(Sent::Handed(said)) if said.contains("runs those tests")) && sends == 1, "rejected, rebased over m again: back to its Claude, {said:?}");
        let waiting = intent::load(&pushing.state, &pushing.address).expect("the same push waits again");
        assert_eq!((waiting.by, waiting.work), (By::User, Work::Tests));
        assert_eq!(texts.borrow().len(), 2);
        let (said, sends) = finish(&|| upstream_commit(&dir, "other", "y", "pushed in between"));
        assert_eq!((said, sends), (Ok(Sent::Pushed("pushed 2".to_string())), 2), "rejected, rebased cleanly, pushed on the second try");
        let landed = git(&origin, &["log", "--format=%s", upstream.trim_start_matches("origin/")]).unwrap();
        assert!(landed.starts_with("first line of m\nfourth\npushed in between\n"), "the task's commits on top of everything that landed: {landed}");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn no_agent_and_no_lead_without_permission_can_start_a_push_and_nothing_is_touched_or_recorded() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        shared_file(&dir, &slot, &upstream);
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        record_approval(&pushing.state, &pushing.address, &fingerprint(&slot.path, &upstream).unwrap()).unwrap();
        let (texts, hand) = handed();
        for caller in [Caller::Agent, Caller::Task(pushing.address.clone()), Caller::Lead] {
            let refused = prepare(&pushing.state, &caller, &dir, &slot, Through::All, None, &hand).unwrap_err();
            assert!(refused.contains("no agent pushes") || refused.contains("no push permission"), "{refused}");
            assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), head, "refused before the rebase");
            assert!(intent::load(&pushing.state, &pushing.address).is_none() && texts.borrow().is_empty());
        }
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_rebase_over_the_tasks_own_files_hands_it_to_its_claude_and_records_the_push_for_who_asked() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        shared_file(&dir, &slot, &upstream);
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        crate::permit::set_task_in(&pushing.state, &pushing.address, true).unwrap();
        record_approval(&pushing.state, &pushing.address, &fingerprint(&slot.path, &upstream).unwrap()).unwrap();
        let (texts, hand) = handed();
        let Prepared::Handed(said) = prepare(&pushing.state, &Caller::Lead, &dir, &slot, Through::All, Some(7), &hand).unwrap() else { panic!("main changed m too") };
        assert!(said.contains("files: m") && said.contains("for the lead"), "{said}");
        let typed = texts.borrow()[0].clone();
        assert!(typed.contains("- m: ") && typed.contains("last line of m") && typed.contains("Do not push") && typed.contains("report ready"), "{typed}");
        let recorded = intent::load(&pushing.state, &pushing.address).unwrap();
        assert_eq!((recorded.by, recorded.fingerprint.as_str(), recorded.through, recorded.queued), (By::Lead, fingerprint(&slot.path, &upstream).unwrap().as_str(), Through::All, Some(7)), "handed over, it keeps its place at the head");
        assert!(approved(&slot.path, &upstream, &pushing.address), "the clean rebase kept the approval");
        let waits = join(&pushing.state, &Caller::Lead, &dir, &slot, Through::All).unwrap_err();
        assert!(waits.contains("Already being pushed"), "{waits}");
        join(&pushing.state, &Caller::User, &dir, &slot, Through::All).unwrap();
        let again = intent::load(&pushing.state, &pushing.address).unwrap();
        assert_eq!((again.by, again.work, again.queued), (By::User, Work::Turn, recorded.queued), "the user may push past it, in the same place in line");
        assert_eq!(prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, again.queued, &hand).unwrap(), Prepared::Push("the user", Out { sha: git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebased: false }));
        assert!(intent::load(&pushing.state, &pushing.address).is_none());
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_conflict_on_push_goes_to_its_claude_with_the_intent_rule_and_records_the_push_for_who_asked() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        upstream_commit(&dir, "d", "theirs", "d on main");
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let (texts, hand) = handed();
        let Prepared::Handed(said) = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand).unwrap() else { panic!("the rebase conflicts") };
        assert!(said.contains("conflicts with") && said.contains("then kebbie pushes it"), "{said}");
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), head);
        let typed = texts.borrow()[0].clone();
        assert!(typed.contains("kebbie ask") && typed.contains("recommendation") && typed.contains("Do not push") && typed.contains("kebbie pushes the resolved commits"), "{typed}");
        let recorded = intent::load(&pushing.state, &pushing.address).unwrap();
        assert_eq!((recorded.by, recorded.work, recorded.fingerprint), (By::User, Work::Conflict, fingerprint(&slot.path, &upstream).unwrap()));
        let failing = |_: &Slot, _: &str| Err("the log is locked".to_string());
        let refused = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &failing).unwrap_err();
        assert!(refused.contains("could not pass the rebase to its Claude") && refused.contains("error: the log is locked"), "{refused}");
        assert!(intent::load(&pushing.state, &pushing.address).is_none(), "a push nobody works on is not shown as under way");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_partial_push_keeps_its_place_across_the_rebase() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        let through = chosen(&slot, &upstream, Some(&fourth), true).unwrap();
        assert_eq!(through, Through::First { count: 1, of: 2 });
        assert_eq!(chosen(&slot, &upstream, None, false).unwrap(), Through::All);
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Clean);
        let rebased = through_sha(&slot.path, &upstream, through).unwrap();
        assert_ne!(rebased, fourth);
        assert_eq!(git(&slot.path, &["log", "-1", "--format=%s", &rebased]).unwrap(), "fourth");
        assert!(through_sha(&slot.path, &upstream, Through::First { count: 3, of: 3 }).unwrap_err().contains("Open push again"), "fewer commits than were chosen");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_pushed_with_keep_open_stays_open_when_the_rest_lands_elsewhere_and_asks_nothing() {
        use crate::tasks::landed::{Seen, Verdict, World, pass, verdict};
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        commit(&slot.path, "e", "fifth");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        deliver(&dir, &slot, &upstream, Through::First { count: 1, of: 2 }, &Out { sha: git(&slot.path, &["rev-parse", "HEAD~1"]).unwrap(), rebased: false }, "the user", &state, &crate::ci::begin).unwrap();
        let fifth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        git(&dir, &["merge", "--quiet", "--ff-only", &upstream]).unwrap();
        pick(&dir, &fifth);
        git(&dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        let repos = [dir.clone()];
        let world = World { repos: &repos, workspaces: &[], agents: &[], log: &[], gh: "false", began: 0, fresh: None };
        let told = std::cell::Cell::new(0);
        pass(&state, &world, &mut |_, _, _| panic!("a task kept open is never closed by the round"), &mut |_| told.set(told.get() + 1));
        assert!(branch_exists(&dir, "billing") && told.get() == 0);
        assert_eq!(crate::tasks::landed_mark(&state, &address, &slot.path), None, "no landed line asks the user to close it");
        let idle = Seen { status: Some("idle".to_string()), question: false };
        crate::tasks::landed::keep_open(&state, &address, false);
        crate::ci::forget(&state, &address);
        assert_eq!(verdict(&state, &slot, &address, &upstream, &idle, &mut |_| false).unwrap(), Verdict::Close, "it is the keep-open choice alone that holds it");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_through_a_commit_counts_its_steps_as_pushed_and_the_rest_map_onto_the_list_after_them() {
        use crate::tasks::plan::{State, list_at, listed_at, load};
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        commit(&slot.path, "e", "fifth");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        list_at(&state, &slot.path, &upstream, &address, &["Model", "Command", "Docs"]).unwrap();
        assert_eq!(listed_at(&state, &slot.path, &upstream, &address).pushed, 0);
        deliver(&dir, &slot, &upstream, Through::First { count: 1, of: 2 }, &Out { sha: git(&slot.path, &["rev-parse", "HEAD~1"]).unwrap(), rebased: false }, "the user", &state, &crate::ci::begin).unwrap();
        let shown = listed_at(&state, &slot.path, &upstream, &address);
        assert_eq!((shown.pushed, shown.items[0].index, shown.items[0].title.as_str()), (1, 1, "Command"), "one commit went out: the first step");
        assert!(matches!(&shown.items[0].state, State::Committed { subject, .. } if subject == "fifth"));
        list_at(&state, &slot.path, &upstream, &address, &["Command, again"]).unwrap();
        assert_eq!(load(&state, &address).unwrap().titles[0], "Model", "the pushed step stays first");
        assert_eq!(listed_at(&state, &slot.path, &upstream, &address).pushed, 1);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_whose_every_commit_was_pushed_kept_open_stays_open_with_no_line_and_done_still_closes_it() {
        use crate::tasks::landed::{World, pass};
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        commit(&slot.path, "e", "fifth");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        let said = deliver(&dir, &slot, &upstream, Through::First { count: 2, of: 2 }, &Out { sha: git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebased: false }, "the user", &state, &crate::ci::begin).unwrap();
        assert!(said.contains("It stays open, as you chose, with everything on main"), "{said}");
        assert!(branch_exists(&dir, "billing"));
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap().0, 0, "everything is on main");
        crate::ci::forget(&state, &address);
        let repos = [dir.clone()];
        let workspaces = [serde_json::json!({ "workspace_id": "w1", "worktree": { "checkout_path": slot.path.to_string_lossy() } })];
        let agents = [serde_json::json!({ "workspace_id": "w1", "agent_status": "idle" })];
        let world = World { repos: &repos, workspaces: &workspaces, agents: &agents, log: &[], gh: "false", began: 0, fresh: None };
        pass(&state, &world, &mut |_, _, _| panic!("a task kept open is never closed by the round"), &mut |_| panic!("nothing to tell"));
        assert_eq!(crate::tasks::landed_mark(&state, &address, &slot.path), None, "no landed line");
        crate::tasks::finish::finish(&dir, &slot, &upstream, &state, None).unwrap();
        assert!(!branch_exists(&dir, "billing"), "done closes it when the user chooses");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_watched_push_is_under_ci_running_and_one_git_refuses_leaves_the_phase_as_it_was() {
        use crate::phase::{Event, Phase};
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        let phase = || crate::phase::load(&state, &address).map(|record| record.phase);
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        crate::phase::record(&state, &address, "report", &Event::Reported { state: "ready", head: Some(&head), committed: true, pushing: false });
        let watching: Watch = &|_, _, _, _, _, _| crate::ci::Begin::Watching;
        git(&dir, &["remote", "set-url", "origin", &dir.with_extension("nowhere").to_string_lossy()]).unwrap();
        assert!(deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head.clone(), rebased: false }, "the user", &state, watching).unwrap_err().contains("git refused the push"));
        assert_eq!(phase(), Some(Phase::Ready { head: Some(head.clone()) }), "refused, it is where it was");
        git(&dir, &["remote", "set-url", "origin", &origin.to_string_lossy()]).unwrap();
        deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head.clone(), rebased: false }, "the user", &state, watching).unwrap();
        assert_eq!(phase(), Some(Phase::CiRunning { sha: head.clone() }));
        assert_eq!(git(&origin, &["rev-parse", upstream.trim_start_matches("origin/")]).unwrap(), head, "pushed");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn capitalized_takes_the_first_character_up_and_leaves_the_rest() {
        assert_eq!(capitalized("the lead"), "The lead");
        assert_eq!(capitalized(""), "");
        assert_eq!(capitalized("ärlig"), "Ärlig");
        assert_eq!(capitalized("ßx"), "SSx");
    }
}
