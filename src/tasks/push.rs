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
use crate::repo::{UNHOOKED, ahead_behind, default_branch, git, git_writing, hidden_from_status, main_checkout, name_of, rebase_in_progress, rebase_writing, uncommitted};
use crate::sh::Result;
use super::approval::{approved, fingerprint, fingerprint_at, same_code};
use super::intent::{self, By, Intent, Through, Work};
use super::finish::close_task;
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
    let commits = git(&slot.path, &["log", "--oneline", &format!("{}..HEAD", crate::repo::tracking(&upstream))])?;
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
    Conflict(Vec<String>),
}

// Rebases onto upstream, already fetched; a conflict is aborted, and a rebase git refuses to start is an error that names where git's own words are kept.
pub(super) fn update(path: &Path, upstream: &str) -> Result<Updated> {
    update_in(path, upstream, &[], None)
}

// A rebase that stops on a git error rather than a conflict, as when a hook stages files while it replays commits, is aborted and run again with hooks off.
const REBASE_TRIES: usize = 3;

enum Attempt {
    Done,
    Conflict(Vec<String>),
    Failed(String),
}

// With from, only the commits after it go onto upstream, as git rebase --onto takes them.
fn update_in(path: &Path, upstream: &str, options: &[&str], from: Option<&str>) -> Result<Updated> {
    let _alone = rebase_alone(path)?;
    let touched = overlap(path, upstream, from)?;
    let unhooked: Vec<&str> = options.iter().copied().chain(UNHOOKED).collect();
    for tried in 1..=REBASE_TRIES {
        match rebase_once(path, upstream, if tried == 1 { options } else { &unhooked }, from)? {
            Attempt::Done if touched.is_empty() => return Ok(Updated::Clean),
            Attempt::Done => return Ok(Updated::Overlap(touched)),
            Attempt::Conflict(files) => return Ok(Updated::Conflict(files)),
            Attempt::Failed(error) if tried == REBASE_TRIES => return Err(refused(path, upstream, &error, tried)),
            Attempt::Failed(_) => std::thread::sleep(std::time::Duration::from_millis(300)),
        }
    }
    unreachable!("the last try returns")
}

fn rebase_once(path: &Path, upstream: &str, options: &[&str], from: Option<&str>) -> Result<Attempt> {
    let tracked = crate::repo::tracking(upstream);
    let rebase: Vec<&str> = match from {
        Some(from) => vec!["rebase", "--quiet", "--onto", &tracked, from],
        None => vec!["rebase", "--quiet", &tracked],
    };
    let mut rebased = rebase_writing(path, options, &rebase);
    while rebased.is_err() && rebase_in_progress(path) && union_release_notes(path, options) {
        rebased = rebase_writing(path, options, &["-c", "core.editor=true", "rebase", "--continue"]);
    }
    let Err(error) = rebased else { return Ok(Attempt::Done) };
    if !rebase_in_progress(path) {
        return Ok(Attempt::Failed(error));
    }
    let unmerged = git(path, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    if let Err(abort) = git_writing(path, &[options, &["rebase", "--abort"]].concat()) {
        return Err(refused(path, upstream, &format!("{error}; git rebase --abort then failed: {abort}"), 1));
    }
    Ok(if unmerged.is_empty() { Attempt::Failed(error) } else { Attempt::Conflict(unmerged.lines().map(str::to_string).collect()) })
}

const RELEASE_NOTES: &str = crate::changes::FILE;

// Every task adds its line at the top of the same list in CHANGES.md, so a rebase stopped on that file alone keeps both sides' lines and goes on; a side that changed or dropped a line, or a union that would repeat a heading, is left a conflict.
fn union_release_notes(path: &Path, options: &[&str]) -> bool {
    if git(path, &["diff", "--name-only", "--diff-filter=U"]).ok().as_deref() != Some(RELEASE_NOTES) {
        return false;
    }
    let Ok(dir) = git(path, &["rev-parse", "--path-format=absolute", "--git-path", "kebbie-union"]) else { return false };
    let dir = Path::new(&dir);
    let merged = union_stages(path, dir);
    let _ = std::fs::remove_dir_all(dir);
    let Some(merged) = merged.filter(|text| !heading_twice(text)) else { return false };
    std::fs::write(path.join(RELEASE_NOTES), merged).is_ok() && git_writing(path, &[options, &["add", "--", RELEASE_NOTES]].concat()).is_ok()
}

// The union of the conflicted file's three stages, a missing base read as empty; ours is the upstream side, so its lines come first.
fn union_stages(path: &Path, dir: &Path) -> Option<String> {
    std::fs::create_dir_all(dir).ok()?;
    let stage = |number: u8| -> Option<std::path::PathBuf> {
        let shown = crate::sh::run_untrimmed("git", &["show", &format!(":{number}:{RELEASE_NOTES}")], Some(path));
        let text = if number == 1 { shown.unwrap_or_default() } else { shown.ok()? };
        let file = dir.join(number.to_string());
        std::fs::write(&file, text).ok().map(|_| file)
    };
    let (base, ours, theirs) = (stage(1)?, stage(2)?, stage(3)?);
    let read = |file: &std::path::PathBuf| std::fs::read_to_string(file).ok();
    let base_text = read(&base)?;
    if !only_adds(&base_text, &read(&ours)?) || !only_adds(&base_text, &read(&theirs)?) {
        return None;
    }
    let [ours_path, base_path, theirs_path] = [&ours, &base, &theirs].map(|file| file.to_string_lossy().to_string());
    git(path, &["merge-file", "--union", &ours_path, &base_path, &theirs_path]).ok()?;
    std::fs::read_to_string(&ours).ok()
}

// Whether side keeps every line of base in order, so it only added lines: a union with a side that reworded or dropped a line would keep the old wording beside the new.
fn only_adds(base: &str, side: &str) -> bool {
    let mut kept = side.lines();
    base.lines().all(|line| kept.any(|candidate| candidate == line))
}

// A heading met twice in the same part of the notes: a version twice, or a group twice under one version.
fn heading_twice(text: &str) -> bool {
    let mut versions = BTreeSet::new();
    let mut groups = BTreeSet::new();
    for line in text.lines().map(str::trim_end) {
        if line.starts_with("## ") {
            groups.clear();
            if !versions.insert(line) {
                return true;
            }
        } else if line.starts_with("### ") && !groups.insert(line) {
            return true;
        }
    }
    false
}

// Every kebbie rebase of a worktree runs under this, through rebase_writing, and none starts over a rebase under way there.
pub(super) fn rebase_alone(path: &Path) -> Result<RebaseLock> {
    let alone = RebaseLock::take(path)?;
    if rebase_in_progress(path) {
        return Err(Notice::new(Tone::NotNow, "Not rebased: a rebase is under way in its worktree").next("kebbie left it running; try again once it is finished.").detail("folder", path.display().to_string()).to_string());
    }
    Ok(alone)
}

// Held for the length of one kebbie rebase of a worktree, so a push, land, rebase, follow or reword never starts a second one there.
pub(super) struct RebaseLock(std::fs::File);

impl RebaseLock {
    pub(super) fn take(path: &Path) -> Result<RebaseLock> {
        use std::os::fd::AsRawFd;
        let lock = Path::new(&git(path, &["rev-parse", "--absolute-git-dir"])?).join("kebbie-rebase.lock");
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&lock).map_err(|error| format!("{}: {error}", lock.display()))?;
        // SAFETY: flock on a descriptor this File owns.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(Notice::new(Tone::NotNow, "Not rebased: kebbie is rebasing its worktree already").next("Try again once that rebase is done.").detail("folder", path.display().to_string()).to_string());
        }
        Ok(RebaseLock(file))
    }
}

impl Drop for RebaseLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

// Rebases the commits after from through sha onto upstream in a detached worktree of kebbie's own, so the task's worktree, its HEAD and its uncommitted work stay as its Claude left them.
fn rebased_elsewhere(path: &Path, from: &str, sha: &str, upstream: &str) -> Result<(Updated, String)> {
    let place = push_place(sha)?;
    let spot = place.to_string_lossy().to_string();
    git(path, &[&UNHOOKED[..], &["worktree", "add", "--quiet", "--detach", &spot, sha]].concat())?;
    let built = update_in(&place, upstream, &UNHOOKED, Some(from)).and_then(|updated| Ok((updated, git(&place, &["rev-parse", "HEAD"])?)));
    if git(path, &["worktree", "remove", "--force", &spot]).is_err() {
        let _ = std::fs::remove_dir_all(&place);
        let _ = git(path, &["worktree", "prune"]);
    }
    built
}

fn push_place(sha: &str) -> Result<std::path::PathBuf> {
    super::launch::scratch_dir(&format!("push-{}-{}", std::process::id(), &sha[..12.min(sha.len())]))
}

// git's own words go to a file the notice names, so the push dialog says in one line what happened and what to do.
fn refused(path: &Path, upstream: &str, error: &str, tried: usize) -> String {
    let kept = git(path, &["rev-parse", "--path-format=absolute", "--git-path", "kebbie-rebase-error.txt"]).ok().filter(|file| std::fs::write(file, format!("{error}\n")).is_ok());
    let tries = if tried > 1 { format!(" kebbie tried {tried} times, the last {} with hooks off.", plural(tried - 1, "time").trim_start_matches("1 ")) } else { String::new() };
    let said = Notice::new(Tone::Failed, format!("Not rebased: git stopped the rebase onto {} on an error, not a conflict", main_of(upstream)));
    let said = match hidden_from_status(path) {
        Ok(hidden) if !hidden.is_empty() => said
            .you("Unhide the files below with git update-index --no-skip-worktree or --no-assume-unchanged, commit or stash them, then push again.")
            .detail("hidden", format!("{} (skip-worktree or assume-unchanged)", hidden.join(", "))),
        _ => said.next(format!("Nothing was pushed; try again, with p on the board for a push.{tries}")),
    };
    let said = said.detail("folder", path.display().to_string());
    match kept {
        Some(file) => said.detail("git said", file).to_string(),
        None => said.detail("error", error.lines().next().unwrap_or_default()).to_string(),
    }
}

// The task's own commits that reached upstream, a push of its first commits among them, are no one else's changes, so they never count as overlap.
fn overlap(path: &Path, upstream: &str, since: Option<&str>) -> Result<Vec<Touched>> {
    let upstream = &crate::repo::tracking(upstream);
    let base = git(path, &["merge-base", "HEAD", upstream])?;
    let mine: BTreeSet<String> = git(path, &["log", "--format=", "--name-only", "--no-renames", &format!("{}..HEAD", since.unwrap_or(&base))])?.lines().filter(|line| !line.is_empty() && !text_only(line)).map(str::to_string).collect();
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

// Release notes and docs pages run no test, so a change both sides made there asks for no test run; a repo's full suite or lint still runs on the rebased HEAD.
fn text_only(file: &str) -> bool {
    file == RELEASE_NOTES || file.starts_with("docs/")
}

// Ok(false) when the rebase conflicted and the task's Claude took it over.
fn rebase_or_hand_over(slot: &Slot, upstream: &str, why: &str) -> Result<bool> {
    if !matches!(update(&slot.path, upstream)?, Updated::Conflict(_)) {
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
         resolve each conflict keeping the intent of both sides (read the upstream commits that touched the file), \
         and continue until the rebase is finished. When a conflict is a choice of intent rather than mechanics, both sides wanting \
         different behaviour from the same code, do not pick one: ask the user with kebbie ask, with the options and your recommendation, and \
         resolve it by their answer. Do not push. Then report ready, saying in a few lines what conflicted and how you resolved it. When a \
         push started this, kebbie pushes the resolved commits as soon as their rules are checked, with no test run first: CI tests them once they land."
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
pub fn push(cwd: &Path, name: Option<&str>, through: Option<&str>, keep_open: bool, anyway: bool, to: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    if !anyway {
        super::waiting::refuse_push(&state_dir()?, &peers::address(task_of(&slot), &name_of(&repo)), task_of(&slot))?;
    }
    let upstream = default_branch(&repo)?;
    let through = chosen(&slot, &upstream, through, keep_open)?;
    push_task(&crate::permit::caller(), &repo, &slot, through, to)
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

fn authorize(caller: &Caller, repo: &Path, slot: &Slot, upstream: &str, through: Through) -> Result<String> {
    let pusher = authorize_as(caller, repo, slot, upstream, through, "Not pushed")?;
    let take = match through { Through::All => None, Through::First { count, .. } => Some(count) };
    if super::accept::accept_required(repo) {
        super::accept::refuse_unaccepted(&state_dir()?, &peers::address(task_of(slot), &name_of(repo)), task_of(slot), &slot.path, upstream, take, "Not pushed")?;
    }
    super::rollout::refuse_past_stage(caller, &state_dir()?, &peers::address(task_of(slot), &name_of(repo)), task_of(slot), &slot.path, upstream, take, "Not pushed")?;
    Ok(pusher)
}

pub(super) fn authorize_as(caller: &Caller, repo: &Path, slot: &Slot, upstream: &str, through: Through, refused: &str) -> Result<String> {
    let task = task_of(slot);
    let address = peers::address(task, &name_of(repo));
    let head = git(&slot.path, &["rev-parse", "HEAD"])?;
    let sha = through_sha(&slot.path, upstream, through)?;
    let shown = crate::review::shown(&address, &slot.path, upstream);
    let covered = |reviewed: &str| same_code(&slot.path, upstream, reviewed, &head) || same_code(&slot.path, upstream, reviewed, &sha);
    if let Some(why) = state_dir().ok().and_then(|state| crate::assessment::holds_push(&state, &address)) {
        return Notice::new(Tone::NotNow, format!("{refused}: {why}")).next("It stays under Ready to push while the assessor reads it; push it again once the board shows its word.").detail("task", task).err();
    }
    crate::review::gate(shown.as_ref(), covered).map_err(|why| Notice::new(Tone::NotNow, format!("{refused}: {why}")).detail("task", task).to_string())?;
    if *caller == Caller::Lead && state_dir().is_ok_and(|state| !intent::lead_may_repush(&state, &address)) {
        return Notice::new(Tone::NotNow, format!("{refused}: it left the push line more than {} times, so it waits for the user", intent::REPUSHES)).detail("task", task).err();
    }
    let check = Pusher { caller: caller.clone(), permit: crate::permit::of(&address, &name_of(repo)), approved: approved(&slot.path, upstream, &address) };
    check.allowed().map_err(|why| Notice::new(Tone::Failed, format!("{refused}: {} may not push it", crate::permit::describe(&check.caller))).detail("task", task).detail("why", why).to_string())
}

fn nothing_to_push<T>(task: &str) -> Result<T> {
    Notice::new(Tone::NotNow, "Nothing to push: it has no commits").detail("task", task).err()
}

#[derive(Debug, PartialEq)]
pub(crate) enum DevicePush {
    Pushed { short: String, main: String },
    Queued(String),
    Conflict { files: Vec<String>, main: String },
    Tests { files: Vec<String>, main: String },
    Settled(String),
    HeadMoved(String),
    Refused(String),
    UnknownTask(String),
    Failed(String),
}

// The same push kebbie push runs for the hub on the user's behalf, through the head the phone sent: the task's HEAD pushes the whole branch, the end of its pushable run pushes the run; the whole branch is refused only by the gates every push of the user's passes, never because HEAD or main moved since the user saw the task.
pub(crate) fn push_as(caller: &Caller, address: &str, head: Option<&str>) -> DevicePush {
    let (task, place) = address.split_once('@').unwrap_or((address, ""));
    let Some(repo) = crate::repo::pooled_checkouts().into_iter().find(|repo| name_of(repo) == place) else {
        return DevicePush::UnknownTask(format!("No repo {place} on this machine"));
    };
    let slot = match find_task(&repo, Some(task), &repo) {
        Ok(slot) => slot,
        Err(error) => return DevicePush::UnknownTask(error),
    };
    let state = match state_dir() {
        Ok(state) => state,
        Err(error) => return DevicePush::Failed(error),
    };
    let (read, working) = super::line::live_claude();
    device_push(&state, caller, &repo, &slot, head, &read, &working, &mut live_push(&state))
}

// An open commit short of the last is pushed through only while it still ends the pushable run the phone saw; a head on the default branch already is settled; a head no longer on the branch pushes the whole branch only while all of it can go out, since it may have ended a run a rebase rewrote; any other head pushes the whole branch.
fn through_head(state: &Path, slot: &Slot, address: &str, upstream: &str, head: &str) -> std::result::Result<Through, DevicePush> {
    let open = open_shas(&slot.path, upstream).map_err(DevicePush::Failed)?;
    let short = &head[..7.min(head.len())];
    let moved = || DevicePush::HeadMoved(Notice::new(Tone::You, "Not pushed: what can go out changed since you saw it").you("Look at its push order again and push from there.").detail("task", task_of(slot)).detail("commit", short).to_string());
    let Some(at) = open.iter().position(|sha| sha == head) else {
        if crate::sh::succeeds("git", &["merge-base", "--is-ancestor", head, &crate::repo::tracking(upstream)], Some(&slot.path)) {
            return Err(DevicePush::Settled(Notice::new(Tone::NotNow, format!("Nothing to push: what you saw is on {} already", main_of(upstream))).detail("task", task_of(slot)).detail("commit", short).to_string()));
        }
        let subtasks = super::plan::read_now(state, &slot.path, upstream, address);
        if subtasks.listed() && subtasks.pushable.as_ref().is_none_or(|run| run.count < open.len()) {
            return Err(moved());
        }
        return Ok(Through::All);
    };
    if at + 1 == open.len() {
        return Ok(Through::All);
    }
    let subtasks = super::plan::read_now(state, &slot.path, upstream, address);
    if subtasks.listed() && subtasks.pushable.as_ref().is_none_or(|run| run.through != head) {
        return Err(moved());
    }
    Ok(Through::First { count: at + 1, of: open.len() })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn device_push(state: &Path, caller: &Caller, repo: &Path, slot: &Slot, head: Option<&str>, read: super::line::Read, working: super::line::Working, push: super::line::Push) -> DevicePush {
    let task = task_of(slot);
    let address = peers::address(task, &name_of(repo));
    if let Err(refused) = super::waiting::refuse_push(state, &address, task) {
        return DevicePush::Refused(refused);
    }
    let upstream = match default_branch(repo) {
        Ok(upstream) => upstream,
        Err(error) => return DevicePush::Failed(error),
    };
    let main = main_of(&upstream).to_string();
    match open_shas(&slot.path, &upstream) {
        Ok(open) if open.is_empty() => return DevicePush::Settled(Notice::new(Tone::NotNow, "Nothing to push: it has no commits").detail("task", task).to_string()),
        Ok(_) => {}
        Err(error) => return DevicePush::Failed(error),
    }
    let through = match head.map(|head| through_head(state, slot, &address, &upstream, head)) {
        None => Through::All,
        Some(Ok(through)) => through,
        Some(Err(refused)) => return refused,
    };
    if let Err(refused) = join(state, caller, repo, slot, through) {
        return DevicePush::Refused(refused);
    }
    let driven = super::line::drive(state, repo, Some(&address), &messages::load(), messages::now(), read, working, push);
    let said = match driven.mine {
        Some(Err(error)) => return DevicePush::Failed(error),
        Some(Ok(said)) => said,
        None => return DevicePush::Queued(queued_notice(state, &address, task).unwrap_or_default()),
    };
    let files = || Notice::read(&said, false).details.into_iter().find(|(label, _)| label == "files").map(|(_, files)| files.split(" · ").map(str::to_string).collect()).unwrap_or_default();
    match intent::load(state, &address).map(|waiting| waiting.work) {
        Some(Work::Conflict) => DevicePush::Conflict { files: files(), main },
        Some(Work::Tests) => DevicePush::Tests { files: files(), main },
        Some(Work::Turn | Work::Ci) => DevicePush::Queued(said),
        None => match crate::phase::load(state, &address).map(|record| record.phase) {
            Some(crate::phase::Phase::OnBranch { branch, .. }) => match git(repo, &["rev-parse", "--short=7", &remote_branch(&branch)]) {
                Ok(short) => DevicePush::Pushed { short, main: branch },
                Err(_) => DevicePush::Failed(said),
            },
            _ => match push_target(repo, &upstream, task, None) {
                Ok(Some(target)) => match git(repo, &["rev-parse", "--short=7", &remote_branch(&target.branch)]) {
                    Ok(short) => DevicePush::Pushed { short, main: target.branch },
                    Err(_) => DevicePush::Failed(said),
                },
                Ok(None) => match git(repo, &["rev-parse", "--short=7", &crate::repo::tracking(&upstream)]) {
                    Ok(short) => DevicePush::Pushed { short, main },
                    Err(_) => DevicePush::Failed(said),
                },
                Err(_) => DevicePush::Failed(said),
            },
        },
    }
}

fn push_task(caller: &Caller, repo: &Path, slot: &Slot, through: Through, to: Option<&str>) -> Result<String> {
    let state = state_dir()?;
    let address = peers::address(task_of(slot), &name_of(repo));
    join_to(&state, caller, repo, slot, through, to)?;
    let (read, working) = super::line::live_claude();
    let mut push = live_push(&state);
    let driven = super::line::drive(&state, repo, Some(&address), &messages::load(), messages::now(), &read, &working, &mut push);
    driven.mine.unwrap_or_else(|| queued_notice(&state, &address, task_of(slot)))
}

pub(super) fn live_push(state: &Path) -> impl FnMut(&Intent, &Slot) -> Result<Sent> + '_ {
    move |intent: &Intent, slot: &Slot| {
        push_task_with(state, &intent.by.caller(), &intent.repo, slot, (intent.through, intent.queued, intent.to.as_deref()), &hand_over, &mut |pusher, out| deliver(&intent.repo, slot, &default_branch(&intent.repo)?, intent.through, out, pusher, state, &crate::ci::begin))
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

pub(crate) fn ordinal(number: usize) -> String {
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
    join_to(state, caller, repo, slot, through, None)
}

// `to` is the branch the user picked for this push, by its template, or the default branch; none follows the repo's push.branch.
pub(super) fn join_to(state: &Path, caller: &Caller, repo: &Path, slot: &Slot, through: Through, to: Option<&str>) -> Result<()> {
    let seen = messages::latest_reports(&messages::load()).get(&peers::address(task_of(slot), &name_of(repo))).map(|report| report.id.clone());
    join_on(state, caller, repo, slot, through, seen, to)
}

// `seen` is the task's latest report, which this push uses up whoever asked for it, so kebbie never queues it again for that report.
pub(super) fn join_on(state: &Path, caller: &Caller, repo: &Path, slot: &Slot, through: Through, seen: Option<String>, to: Option<&str>) -> Result<()> {
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
    if to == Some(crate::push_target::DEFAULT_BRANCH) && !By::of(caller).is_some_and(|by| by.is_user()) {
        return Notice::new(Tone::Failed, format!("Not pushed: only you push this task to {}", main_of(&upstream))).next("Its repo sets push.branch, so a push goes to the default branch only when you pick it.").detail("task", &task).err();
    }
    if let Some(target) = push_target(repo, &upstream, &task, to)?.filter(|target| target.per_task) {
        fast_forwards(&slot.path, &target.branch, &through_sha(&slot.path, &upstream, through)?, &task)?;
    }
    authorize(caller, repo, slot, &upstream, through)?;
    let by = By::of(caller).ok_or_else(|| Notice::new(Tone::Failed, format!("Not pushed: {} may not push it", crate::permit::describe(caller))).detail("task", &task).to_string())?;
    let waiting = intent::load(state, &address);
    if waiting.is_some() && !By::of(caller).is_some_and(|by| by.is_user()) {
        return Notice::new(Tone::NotNow, "Already being pushed").next("It is in the push line; kebbie pushes it when its turn comes.").detail("task", &task).err();
    }
    let now = messages::now();
    let queued = waiting.and_then(|waiting| waiting.queued).unwrap_or(now);
    intent::save(state, &Intent { by: by.clone(), work: Work::Turn, address: address.clone(), repo: repo.to_path_buf(), task, fingerprint: pushed_fingerprint(&slot.path, &upstream, through)?, through, at: now, seen: seen.clone(), hand: None, queued: Some(queued), waited: None, to: to.map(str::to_string) })?;
    intent::forget_left(state, &address);
    if by.is_user() {
        intent::forget_left_count(state, &address);
    }
    if let Some(seen) = &seen {
        super::permitted::used_up(state, &address, seen);
    }
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
    if by == By::Lead && waiting.by.is_user() {
        return Notice::new(Tone::Failed, "Not taken out: the user asked for this push").you("Ask the user; only they take their push back.").detail("task", task).err();
    }
    let Some(_turnstile) = super::line::Turnstile::try_take(state, repo) else {
        return Notice::new(Tone::NotNow, "Not taken out: kebbie is pushing it now").next("Try again in a moment; if it went out, it is under CI running.").detail("task", task).err();
    };
    if intent::load(state, &address).is_none() {
        return Notice::new(Tone::NotNow, "Not in the push line any more").detail("task", task).err();
    }
    leave_line(state, repo, slot, &address);
    if let Err(error) = messages::to_lead(messages::ToLead::PushCancelled, &format!("{} took {address} out of the push line before it went out; nothing of it was pushed.", capitalized(by.word()))) {
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
pub(super) fn push_task_with(state: &Path, caller: &Caller, repo: &Path, slot: &Slot, (through, queued, to): (Through, Option<u64>, Option<&str>), hand_over: HandOver, send: &mut dyn FnMut(&str, &Out) -> Result<String>) -> Result<Sent> {
    let mut tried = 0;
    loop {
        tried += 1;
        match prepare(state, caller, repo, slot, through, queued, hand_over, to)? {
            Prepared::Handed(said) => return Ok(Sent::Handed(said)),
            Prepared::Push(pusher, out) => match send(&pusher, &out) {
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
    pub to: Option<crate::push_target::Target>,
}

#[derive(Debug, PartialEq)]
pub(super) enum Prepared {
    Push(String, Out),
    Handed(String),
}

pub(super) type HandOver<'a> = &'a dyn Fn(&Slot, &str) -> std::result::Result<(), String>;

// Only the head of a repo's line gets here: it fetches and rebases onto origin's default branch, never the local one; a push of the first commits rebases them elsewhere and leaves the task's worktree alone.
#[allow(clippy::too_many_arguments)]
fn prepare(state: &Path, caller: &Caller, repo: &Path, slot: &Slot, through: Through, queued: Option<u64>, hand_over: HandOver, chosen: Option<&str>) -> Result<Prepared> {
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
    let rule = crate::push_target::rule(repo)?;
    super::push_steps::step(&task, &rule.as_ref().map_or_else(|| format!("rebasing onto {}", main_of(&upstream)), |rule| format!("checking {}", rule.branch.replace(config::TASK_PLACE, &task))));
    fetch(repo)?;
    let target = push_target(repo, &upstream, &task, chosen)?;
    let recorded = pushed_fingerprint(&slot.path, &upstream, through)?;
    let seen = messages::latest_reports(&messages::load()).get(&address).map(|report| report.id.clone());
    let record = |work: Work, fingerprint: String| intent::save(state, &Intent { by: by.clone(), work, address: address.clone(), repo: repo.to_path_buf(), task: task.clone(), fingerprint, through, at: messages::now(), seen: seen.clone(), hand: None, queued, waited: None, to: chosen.map(str::to_string) });
    let mut shared = None;
    let (updated, out) = match (target, through) {
        (Some(target), _) if target.per_task => {
            let sha = through_sha(&slot.path, &upstream, through)?;
            fast_forwards(&slot.path, &target.branch, &sha, &task)?;
            (Updated::Clean, Some(Out { sha, rebased: false, to: Some(target) }))
        }
        (Some(target), _) => {
            shared = Some(target);
            (Updated::Clean, None)
        }
        (None, Through::All) => (update(&slot.path, &upstream)?, None),
        (None, Through::First { .. }) => {
            let sha = through_sha(&slot.path, &upstream, through)?;
            if crate::sh::succeeds("git", &["merge-base", "--is-ancestor", &crate::repo::tracking(&upstream), &sha], Some(&slot.path)) {
                (Updated::Clean, Some(Out { sha, rebased: false, to: None }))
            } else {
                let first = open_shas(&slot.path, &upstream)?.into_iter().next().unwrap_or_else(|| sha.clone());
                let (updated, built) = rebased_elsewhere(&slot.path, &format!("{first}^"), &sha, &upstream)?;
                (updated, Some(Out { sha: built, rebased: true, to: None }))
            }
        }
    };
    let kept = if through == Through::All { String::new() } else { format!(" {LEFT_AS_IT_WAS}") };
    if let Updated::Conflict(files) = updated {
            record(Work::Conflict, recorded)?;
            let why = if by == By::Keeper { asked(&by, "this task") } else { format!("{} asked kebbie to push this task", capitalized(&pusher)) };
            let prompt = format!("{}{kept} {WAITING_BEHIND}", conflict_prompt(&why, &upstream));
            if let Err(cannot) = hand_over(slot, &prompt).and_then(|_| intent::note_hand_over(state, &address)) {
                intent::forget(state, &address);
                return Notice::new(Tone::Failed, format!("Not pushed: it conflicts with {}, and kebbie could not pass the rebase to its Claude", main_of(&upstream)))
                    .you("Push again; if this repeats, rebase it yourself in its worktree.").detail("task", &task).detail("files", files.join(" · ")).detail("folder", slot.path.display().to_string()).detail("error", cannot).err();
            }
            crate::phase::record(state, &address, by.key(), &crate::phase::Event::PushHanded);
            tell_lead(&format!("{}; rebasing onto {upstream} conflicted, so its Claude resolves it as part of the push. When it reports ready, the reviewer checks the resolved commits against the rules and kebbie pushes them{} then; you hear only of a break, or when the reviewer gives no verdict. The task keeps the head of its repo's push line while its Claude works, and for up to {} minutes once another push waits behind it.", who_pushed(&pusher, &address), for_by(&by), super::line::CEILING / 60));
            return Ok(Prepared::Handed(Notice::new(Tone::Moving, format!("Not pushed yet: it conflicts with {}", main_of(&upstream)))
                .next(format!("Its Claude resolves the conflict, the reviewer checks the result against the rules, then kebbie pushes it{}.", for_whom(&pusher))).detail("task", &task).detail("files", files.join(" · ")).to_string()));
    }
    intent::forget(state, &address);
    let pusher = authorize(caller, repo, slot, &upstream, through)?;
    let out = match (out, shared) {
        (_, Some(target)) => onto_shared(state, repo, &slot.path, &upstream, through, target, rule.as_ref().and_then(|rule| rule.prepare.as_deref()), &task)?,
        (Some(out), None) => out,
        (None, None) => Out { sha: git(&slot.path, &["rev-parse", "HEAD"])?, rebased: false, to: None },
    };
    Ok(Prepared::Push(pusher, out))
}

pub(super) type Watch<'a> = &'a dyn Fn(&Path, &Path, &str, &str, bool, crate::ci::Pushed) -> crate::ci::Begin;

struct Went {
    sha: String,
    keep_open: bool,
    rest: usize,
    pushed: String,
    shas: Vec<String>,
    note: Option<String>,
    target: String,
}

fn went_out(repo: &Path, slot: &Slot, upstream: &str, through: Through, out: &Out) -> Result<Went> {
    let sha = out.sha.clone();
    let keep_open = matches!(through, Through::First { .. });
    let rest = if keep_open { git(&slot.path, &["rev-list", "--count", &format!("{}..HEAD", through_sha(&slot.path, upstream, through)?)])?.parse::<usize>().unwrap_or(0) } else { 0 };
    let pushed = git(&slot.path, &["log", "--reverse", "--format=%h %s", &format!("{}..{sha}", crate::repo::tracking(upstream))])?;
    let shas: Vec<String> = git(&slot.path, &["rev-list", "--reverse", &format!("{}..{sha}", crate::repo::tracking(upstream))])?.lines().map(str::to_string).collect();
    let note = started_file(task_of(slot), repo).ok().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| crate::task_note::items(&text).into_iter().next());
    let target = upstream.trim_start_matches("origin/").to_string();
    Ok(Went { sha, keep_open, rest, pushed, shas, note, target })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn deliver(repo: &Path, slot: &Slot, upstream: &str, through: Through, out: &Out, pusher: &str, state: &Path, watch: Watch) -> Result<String> {
    let task = task_of(slot).to_string();
    let address = peers::address(&task, &name_of(repo));
    if let Some(target) = &out.to {
        return deliver_to_branch(repo, slot, upstream, through, out, pusher, state, target);
    }
    let mut went = went_out(repo, slot, upstream, through, out)?;
    went.keep_open &= !(went.rest == 0 && crate::verify_after::load(state, &address).is_some());
    super::push_steps::step(&task, &format!("pushing to {}", main_of(upstream)));
    let (count, watch, staged) = send(state, repo, slot, upstream, &address, &task, pusher, &went, watch)?;
    let (vault, log) = after_push(state, repo, slot, &address, &task, &went, &crate::vault_cli::detect);
    let followed = crate::report::check_in(state, &address).is_some() || staged.check.is_some() || crate::verify_after::load(state, &address).is_some();
    let full = !went.keep_open && !followed;
    let tidy = || full.then(|| super::pool::tidy(repo, &slot.path, upstream, &super::pool::build_folders(repo))).flatten().unwrap_or_default();
    let Went { sha, keep_open, rest, pushed, note, target, .. } = went;
    let main = main_of(upstream);
    let logged = log.as_ref().map(crate::task_note::logged_text).unwrap_or_default();
    let told = Landed { pusher, address: &address, target: &target, commits: &pushed, closed: false, note: note.as_deref(), logged: log.as_ref().map(|log| matches!(log, Ok(Some(_)))) };
    let short = &sha[..7.min(sha.len())];
    if matches!(watch, crate::ci::Begin::Watching) {
        crate::ci::arm(state, &address, log.as_ref().map(|log| matches!(log, Ok(Some(_)))));
    } else {
        super::unblocked::tell(repo, &address, &super::unblocked::landed(&address, &sha, &target, super::unblocked::Ci::Unwatched));
    }
    let ci = match &watch {
        crate::ci::Begin::Watching => format!("kebbie watches {short}"),
        crate::ci::Begin::NotWatched(reason) => format!("not watched: {reason}"),
        crate::ci::Begin::Off => String::new(),
    };
    let headline = |through: bool| if through { format!("Pushed {} through {short} to {main}", plural(count, "commit")) } else { format!("Pushed {} to {main}", plural(count, "commit")) };
    let said = |notice: Notice| notice.detail("task", &task).detail("commit", short).detail("ci", &ci).detail("log", &logged);
    if !staged.stages.is_empty() {
        let rolled = (!matches!(watch, crate::ci::Begin::Watching)).then(|| unwatched_stages(state, repo, &address, &sha, &target, &staged, &watch, keep_open));
        let stages = rolled.as_ref().map(|rolled| rolled.text()).filter(|text| !text.is_empty()).unwrap_or_else(|| "kebbie follows CI for them, then asks for each stage's requirement".to_string());
        if !keep_open && !rolled.as_ref().is_some_and(|rolled| rolled.all_met && staged.check.is_none()) {
            if let Err(error) = messages::to_lead(messages::ToLead::Pushed, &format!("{}: {} to {target}, the last of its stages: {stages}; the task closes once every stage is met.", who_pushed(pusher, &address), plural(count, "commit"))) {
                eprintln!("Could not tell the lead about the push: {error}");
            }
            let notice = said(Notice::new(Tone::Done, headline(false)).next(format!("Stages: {stages}; kebbie closes the task once every stage is met."))).detail("freed", tidy());
            return Ok(notice.detail("checkout", follow_upstream(repo, upstream, &super::stage::load(state, repo))?).to_string());
        }
    }
    if !keep_open {
        let check = crate::report::check_in(state, &address);
        if matches!(watch, crate::ci::Begin::Watching) {
            let then = check.as_ref().map(|check| format!(" and the check it named for after the push holds ({check})")).unwrap_or_default();
            if let Err(error) = messages::to_lead(messages::ToLead::Pushed, &format!("{}: {} to {target}. kebbie watches CI for {short} and closes the task once every run is green{then}; you hear what landed then.", who_pushed(pusher, &address), plural(count, "commit"))) {
                eprintln!("Could not tell the lead about the push: {error}");
            }
            let next = match &check {
                Some(check) => format!("CI runs now; once it is green the task checks {check}, and kebbie closes it when that holds."),
                None => "CI runs now; kebbie closes the task when it is green.".to_string(),
            };
            let notice = Notice::new(Tone::Done, headline(false)).next(next).detail("task", &task).detail("commit", short).detail("log", &logged).detail("freed", tidy());
            return Ok(notice.detail("checkout", follow_upstream(repo, upstream, &super::stage::load(state, repo))?).to_string());
        }
        if let Some(check) = check {
            let pushed = crate::ci::Pushed { pusher: pusher.to_string(), target: target.clone(), commits: pushed.clone(), note: note.clone(), logged: log.as_ref().map(|log| matches!(log, Ok(Some(_)))), check: Some(crate::ci::Check::new(&check)), stages: Vec::new(), deploy: None, window: None, held: None, refused: None, inherited: None, told: false };
            let watch = crate::ci::Watch { address: address.clone(), repo: repo.to_path_buf(), slug: String::new(), sha: sha.clone(), started_at: messages::now(), checked_at: None, close: true, runs: Vec::new(), state: crate::ci::State::Running, error: None, erring_since: None, following: None, pushed };
            match crate::ci::ask_check(state, &watch, &format!("{short} is on {target}"), messages::now()) {
                Ok(_) => {
                    crate::phase::record(state, &address, "push", &crate::phase::Event::PushOut { sha: &sha });
                    let notice = said(Notice::new(Tone::Done, headline(false)).next(format!("The task checks {check} now; kebbie closes it when that holds."))).detail("freed", tidy());
                    return Ok(notice.detail("checkout", follow_upstream(repo, upstream, &super::stage::load(state, repo))?).to_string());
                }
                Err(error) => eprintln!("kebbie could not ask {address} to run its check after the push, so it closes the task without it: {error}"),
            }
        }
        if let Some(waits) = crate::verify_after::hold(state, &address, Some(&pushed)) {
            tell_lead_pushed(&told);
            return Ok(said(Notice::new(Tone::Done, headline(false)).next(waits)).detail("checkout", follow_upstream(repo, upstream, &super::stage::load(state, repo))?).to_string());
        }
        return Ok(match close_pushed(state, "push", told, |commits| close_task(repo, Some(&task), vault.as_ref(), Some(commits))) {
            Ok(closed) => {
                let mut notice = said(Notice::new(Tone::Done, format!("{} and closed the task", headline(false))).next(closed.next.unwrap_or_default()));
                notice.details.extend(closed.details);
                notice.to_string()
            }
            Err(error) => said(Notice::new(Tone::You, format!("{}, but the task did not close", headline(false)))).you("Close it once the reason below is settled.").detail("error", error).detail("freed", tidy()).to_string(),
        });
    }
    let named = crate::status::with_titles_in(state, &address);
    let after = kept_open(state, slot, upstream, &address, &sha, rest, count, !out.rebased, &Landed { address: &named, ..told });
    Ok(said(Notice::new(Tone::Done, headline(true)).next(after)).detail("checkout", follow_upstream(repo, upstream, &super::stage::load(state, repo))?).to_string())
}

// A push of stages kebbie cannot follow CI for: each stage is out once pushed, so a check goes to the task now and a deploy or a verification to the user.
#[allow(clippy::too_many_arguments)]
fn unwatched_stages(state: &Path, repo: &Path, address: &str, sha: &str, target: &str, staged: &Staged, watch: &crate::ci::Begin, keep_open: bool) -> super::rollout::Rolled {
    let unseen = match watch {
        crate::ci::Begin::NotWatched(reason) => format!("CI is not watched: {reason}"),
        _ => "ci.watch is off".to_string(),
    };
    let rolled = super::rollout::rolled_out(state, address, &staged.stages, Some(&unseen), messages::now());
    if let Some(check) = &staged.check {
        let pushed = crate::ci::Pushed { check: Some(crate::ci::Check::new(check)), stages: staged.stages.clone(), target: target.to_string(), ..crate::ci::Pushed::default() };
        let watch = crate::ci::Watch { address: address.to_string(), repo: repo.to_path_buf(), slug: String::new(), sha: sha.to_string(), started_at: messages::now(), checked_at: None, close: !keep_open, runs: Vec::new(), state: crate::ci::State::Running, error: None, erring_since: None, following: None, pushed };
        if let Err(error) = crate::ci::ask_check(state, &watch, &format!("{} is on {target}", &sha[..7.min(sha.len())]), messages::now()) {
            eprintln!("kebbie could not ask {address} to run its check of the stage: {error}");
        }
    }
    rolled
}

#[allow(clippy::too_many_arguments)]
fn kept_open(state: &Path, slot: &Slot, upstream: &str, address: &str, sha: &str, rest: usize, count: usize, clean: bool, landed: &Landed) -> String {
    let (pusher, target, short, main) = (landed.pusher, landed.target, &sha[..7.min(sha.len())], main_of(upstream));
    tell_lead_pushed(landed);
    crate::report::forget_card_in(state, address);
    crate::phase::record(state, address, "push", &crate::phase::Event::KeptOpen { rest, steps_left: super::plan::left_at(state, &slot.path, address, sha), clean, head: git(&slot.path, &["rev-parse", "HEAD"]).ok().as_deref() });
    let told = if clean {
        format!(
            "{} pushed {} of this task, through {short}, to {target}; they are on {target} for good. Your branch continues from there with {} on top: reshape only those.",
            pushed_by(pusher), plural(count, "commit"), plural(rest, "commit")
        )
    } else {
        format!(
            "{} pushed {} of this task to {target}, rebased onto it as {short}; they are on {target} for good. kebbie changed nothing in your worktree, so your branch still holds them under their old shas: \
             once your work in progress is committed, at a point of your choosing, run git rebase {upstream} (already fetched, no -i), which drops the pushed ones as already on {target}. \
             Do not push. Reshape only the {} after them.",
            pushed_by(pusher), plural(count, "commit"), plural(rest, "commit")
        )
    };
    if let Err(error) = messages::notify(address, &told) {
        eprintln!("Could not tell {address} about the push: {error}");
    }
    if clean && rest == 0 && super::rollout::all_met(state, address) {
        let task = address.split_once('@').map_or(address, |(task, _)| task);
        let _ = crate::kebbie_questions::ask_close(state, address, &format!("{task} is on {target} with nothing left to push, and stays open as you chose. Close it?"));
    }
    if clean && rest == 0 { "It stays open, as you chose, with everything on main; kebbie asks you whether to close it.".to_string() } else if clean { format!("It stays open with {} on top.", plural(rest, "commit")) } else { format!("It stays open; its Claude rebases the rest onto {main} when it is ready, and kebbie changed nothing in its worktree.") }
}

#[allow(clippy::too_many_arguments)]
fn send(state: &Path, repo: &Path, slot: &Slot, upstream: &str, address: &str, task: &str, pusher: &str, went: &Went, watch: Watch) -> Result<(usize, crate::ci::Begin, Staged)> {
    let sha = &went.sha;
    let touched = super::rollout::touched(state, &slot.path, upstream, address, went.shas.len());
    let (stages, stage_check, deploy) = super::rollout::for_watch(repo, &touched);
    let check = stage_check.clone().or_else(|| crate::report::check_in(state, address).filter(|_| !went.keep_open)).map(|check| crate::ci::Check::new(&check));
    let told_ci = crate::ci::Pushed { pusher: pusher.to_string(), target: went.target.clone(), commits: went.pushed.clone(), note: went.note.clone(), logged: None, check, stages: stages.clone(), deploy, window: None, held: None, refused: None, inherited: None, told: false };
    let before = crate::ci::load(state, address);
    let phase_before = crate::phase::load(state, address);
    let watch = watch(state, repo, address, sha, !went.keep_open, told_ci);
    if !went.keep_open && matches!(watch, crate::ci::Begin::Watching) {
        crate::phase::record(state, address, "push", &crate::phase::Event::PushOut { sha });
    }
    super::plan::seen_in(state, &slot.path);
    match push_prefix(&slot.path, upstream, sha, state) {
        Ok(count) => {
            super::rollout::mark_pushed(state, address, &stages, sha, messages::now());
            Ok((count, watch, Staged { stages, check: stage_check }))
        }
        Err(error) => {
            crate::ci::put_back(state, address, before);
            crate::phase::put_back(state, address, &crate::phase::Phase::CiRunning { sha: sha.clone() }, phase_before);
            Notice::new(Tone::Failed, "Not pushed: git refused the push").detail("task", task).detail("error", error).err()
        }
    }
}

type Log = Option<std::result::Result<Option<String>, String>>;

// The stages a push put out, and the task's check when one of them is a check.
struct Staged {
    stages: Vec<String>,
    check: Option<String>,
}

fn after_push(state: &Path, repo: &Path, slot: &Slot, address: &str, task: &str, went: &Went, vault: &dyn Fn() -> Option<crate::vault_cli::Cli>) -> (Option<crate::vault_cli::Cli>, Log) {
    super::landed::keep_open(state, address, went.keep_open);
    crate::hub::notices::settle(crate::hub::notices::Subject::Ready, address);
    if went.keep_open && let Err(error) = crate::assessment::note_pushed(state, address, &went.shas) {
        eprintln!("kebbie: the review of {address} may miss its pushed steps: {error}");
    }
    crate::comments::begin(state, repo, address, &went.shas, messages::now());
    crate::comments::after_push(state);
    let vault = vault();
    let log = vault.as_ref().map(|cli| crate::task_note::log_push(cli, &slot.path, task, &went.pushed));
    if let Some(cli) = vault.as_ref() {
        crate::task_note::heartbeat(cli, state, address);
    }
    (vault, log)
}

fn tell_lead(text: &str) {
    if let Err(error) = messages::to_lead(messages::ToLead::PushRebasing, text) {
        eprintln!("Could not tell the lead: {error}");
    }
}

pub fn after_ci(repo: &Path, task: &str, address: &str, told: &crate::ci::Pushed) -> Result<String> {
    let state = state_dir()?;
    let vault = crate::vault_cli::detect();
    let landed = Landed { pusher: &told.pusher, address, target: &told.target, commits: &told.commits, closed: false, note: told.note.as_deref(), logged: told.logged };
    if let Some(waits) = crate::verify_after::hold(&state, address, Some(&told.commits)) {
        tell_lead_pushed(&landed);
        return Ok(waits);
    }
    close_pushed(&state, "ci", landed, |commits| close_task(repo, Some(task), vault.as_ref(), Some(commits))).map(|closed| closed.detail("task", task).to_string())
}

// The lead's notice is named before close runs, since the close forgets the task's title.
fn close_pushed(state: &Path, by: &str, told: Landed, close: impl FnOnce(&str) -> Result<Notice>) -> Result<Notice> {
    let named = crate::status::with_titles_in(state, told.address);
    let landed = |closed: bool| Landed { address: &named, closed, ..told };
    let closed = close(told.commits);
    if closed.is_err() {
        crate::phase::record(state, told.address, by, &crate::phase::Event::PushFailed);
    }
    tell_lead_pushed(&landed(closed.is_ok()));
    closed
}

fn for_whom(pusher: &str) -> String {
    match pusher {
        KEEPER_PUSHER => " under the user's push permission".to_string(),
        _ if pusher.starts_with("the user") => String::new(),
        _ => format!(" for {pusher}"),
    }
}

// Who started the push, as the task's Claude reads it: kebbie under the user's permission asked no one.
pub(crate) fn asked(by: &By, what: &str) -> String {
    if *by == By::Keeper { format!("kebbie is pushing {what} under the user's push permission") } else { format!("{} asked kebbie to push {what}", capitalized(by.word())) }
}

pub(crate) fn given(by: &By) -> String {
    if *by == By::Keeper { "the push under the user's push permission".to_string() } else { format!("the push {} gave", by.word()) }
}

pub(crate) fn for_by(by: &By) -> String {
    if *by == By::Keeper { " under the user's push permission".to_string() } else { format!(" for {}", by.word()) }
}

fn pushed_by(pusher: &str) -> String {
    if pusher == KEEPER_PUSHER { "kebbie, under the user's push permission,".to_string() } else { capitalized(pusher) }
}

const HUB_PUSHER: &str = "the user from ";
const KEEPER_PUSHER: &str = "kebbie";

// The lead's word on who pushed: the user's push through the hub says so after the task, as the lead's copy of a hub answer does.
fn who_pushed(pusher: &str, address: &str) -> String {
    if pusher == KEEPER_PUSHER {
        return format!("kebbie pushed {address} under the user's push permission");
    }
    match pusher.strip_prefix(HUB_PUSHER) {
        Some(device) => format!("The user pushed {address} from {device}"),
        None => format!("{} pushed {address}", capitalized(pusher)),
    }
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map(|first| first.to_uppercase().chain(characters).collect()).unwrap_or_default()
}

// Who may push this task now: the user always, the lead or kebbie itself only with the user's permission and a rules approval, a task or any other Claude never.
struct Pusher {
    caller: crate::permit::Caller,
    permit: Option<crate::permit::Permit>,
    approved: bool,
}

impl Pusher {
    fn allowed(&self) -> std::result::Result<String, String> {
        use crate::permit::Caller;
        match &self.caller {
            Caller::User => Ok("the user".to_string()),
            Caller::Hub { name, .. } => Ok(format!("{HUB_PUSHER}{name}")),
            Caller::Lead => {
                crate::permit::lead_may_push(self.permit)?;
                if !self.approved {
                    return Err("its commits have no rules approval that matches them: wait for the reviewer's rules check, or check them and run kebbie approve".to_string());
                }
                Ok("the lead".to_string())
            }
            Caller::Keeper => {
                crate::permit::keeper_may_push(self.permit)?;
                if !self.approved {
                    return Err("its commits have no rules approval that matches them".to_string());
                }
                Ok(KEEPER_PUSHER.to_string())
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
}

// logged is None without the vault, else whether kebbie wrote the repo's log line itself; with the vault kebbie settles the note. address is named with its title before the close forgets it.
fn pushed_notice(landed: &Landed) -> String {
    let Landed { pusher, address, target, commits, closed, note, logged } = *landed;
    let count = commits.lines().count();
    let mut text = format!("{}: {} to {target} ({}).", who_pushed(pusher, address), plural(count, "commit"), commits.lines().collect::<Vec<_>>().join("; "));
    text.push_str(if closed { " The task is done and closed." } else { " The task stays open with the rest of its branch." });
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
    let told = messages::to_lead(if landed.logged != Some(true) { messages::ToLead::PushToLog } else { messages::ToLead::Landed }, &pushed_notice(landed));
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

// The branches the board's push window offers for the task in this worktree; none for a repo without push.branch. Reads origin's refs and may run push.version.command, so never on the board's own thread.
pub fn push_offers(path: &Path) -> Vec<crate::push_target::Offer> {
    let found = main_checkout(path).and_then(|repo| Ok((find_task(&repo, None, path)?, default_branch(&repo)?, crate::push_target::rule(&repo)?, repo)));
    match found {
        Ok((slot, upstream, Some(rule), repo)) => crate::push_target::offers(&repo, &upstream, &rule, task_of(&slot)),
        _ => Vec::new(),
    }
}

// Where the repo's push.branch sends this task's push; none pushes to the default branch, and a rule kebbie cannot follow pushes nowhere.
pub(crate) fn push_target(repo: &Path, upstream: &str, task: &str, chosen: Option<&str>) -> Result<Option<crate::push_target::Target>> {
    let Some(rule) = crate::push_target::rule(repo)? else { return Ok(None) };
    let template = match chosen {
        Some(crate::push_target::DEFAULT_BRANCH) => return Ok(None),
        Some(chosen) if chosen != rule.branch && !rule.choices.iter().any(|choice| choice == chosen) => {
            return Notice::new(Tone::You, format!("Not pushed: the repo offers no branch {chosen:?}")).you(format!("Pick one of push.branch and push.choices: {}.", rule.offered().join(", "))).detail("task", task).err();
        }
        Some(chosen) => chosen,
        None => rule.branch.as_str(),
    };
    let refused = |error: String| Notice::new(Tone::You, format!("Not pushed: {template:?} names no branch kebbie can push to")).you("Fix push.branch or push.choices in the repo's config, or push the branch it names to origin first.").detail("task", task).detail("error", error).to_string();
    let target = crate::push_target::resolve(repo, upstream, &rule, template, task).map_err(refused)?;
    if target.branch == main_of(upstream) {
        return Err(refused(format!("it names {}, the default branch", target.branch)));
    }
    Ok(Some(target))
}

// Without git cherry the commits an earlier push put on the branch, rebased under other shas, are replayed onto it again and conflict with themselves.
fn past_what_went_out(path: &Path, remote: &str, base: &str, sha: &str) -> Result<String> {
    let marked = git(path, &["cherry", remote, sha, base])?;
    match marked.lines().find_map(|line| line.strip_prefix("+ ")) {
        Some(first) => Ok(format!("{first}^")),
        None => Ok(sha.to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
fn onto_shared(state: &Path, repo: &Path, path: &Path, upstream: &str, through: Through, target: crate::push_target::Target, prepare: Option<&str>, task: &str) -> Result<Out> {
    let branch = target.branch.clone();
    if !target.exists {
        super::push_steps::step(task, &format!("creating {branch}"));
        create_shared(state, repo, upstream, &target, prepare, task)?;
    }
    let sha = through_sha(path, upstream, through)?;
    let first = open_shas(path, upstream)?.into_iter().next().unwrap_or_else(|| sha.clone());
    let onto = format!("origin/{branch}");
    let from = past_what_went_out(path, &remote_branch(&branch), &format!("{first}^"), &sha)?;
    if from == sha {
        return Notice::new(Tone::NotNow, format!("Nothing new to push: {branch} on origin already holds it")).next(format!("It waits under On branch until its commits reach {}.", main_of(upstream))).detail("task", task).err();
    }
    let (updated, built) = rebased_elsewhere(path, &from, &sha, &onto)?;
    if let Updated::Conflict(files) = updated {
        return Notice::new(Tone::You, format!("Not pushed: it conflicts with what is on {branch}"))
            .you(format!("Its commits do not apply on top of {branch}; settle the conflict on {branch} yourself, or push it once {branch} has reached {}.", main_of(upstream)))
            .detail("task", task).detail("files", files.join(" · ")).err();
    }
    if git(path, &["rev-list", "--count", &format!("{}..{built}", remote_branch(&branch))])? == "0" {
        return Notice::new(Tone::NotNow, format!("Nothing new to push: {branch} on origin already holds it")).next(format!("It waits under On branch until its commits reach {}.", main_of(upstream))).detail("task", task).err();
    }
    Ok(Out { sha: built, rebased: true, to: Some(target) })
}

const PREPARE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

// The branch is pushed before any task's commits go onto it, so a conflict or a refused push later still leaves every task a base on origin to go onto.
fn create_shared(state: &Path, repo: &Path, upstream: &str, target: &crate::push_target::Target, prepare: Option<&str>, task: &str) -> Result<()> {
    let branch = &target.branch;
    let tip = git(repo, &["rev-parse", &crate::repo::tracking(upstream)])?;
    let place = super::launch::scratch_dir(&format!("branch-{}-{}", std::process::id(), &tip[..12.min(tip.len())]))?;
    let spot = place.to_string_lossy().to_string();
    git(repo, &[&UNHOOKED[..], &["worktree", "add", "--quiet", "--detach", &spot, &tip]].concat())?;
    let made = prepared(&place, branch, target.version.as_deref(), prepare, upstream).and_then(|_| crate::repo::push_from(&place, &[&format!("HEAD:refs/heads/{branch}")], state)
        .map_err(|error| Notice::new(Tone::Failed, format!("Not pushed: git refused to create {branch} on origin")).detail("task", task).detail("error", error).to_string()));
    if git(repo, &["worktree", "remove", "--force", &spot]).is_err() {
        let _ = std::fs::remove_dir_all(&place);
        let _ = git(repo, &["worktree", "prune"]);
    }
    made.map_err(|error| if error.contains("Not pushed") { error } else { Notice::new(Tone::Failed, format!("Not pushed: push.prepare failed, so kebbie did not create {branch}")).you("Fix push.prepare in the repo's config, or create the branch yourself, then push again.").detail("task", task).detail("error", error).to_string() })?;
    fetch(repo)
}

fn prepared(place: &Path, branch: &str, version: Option<&str>, prepare: Option<&str>, upstream: &str) -> Result<()> {
    let Some(prepare) = prepare else { return Ok(()) };
    let mut command = std::process::Command::new("sh");
    command.args(["-c", prepare]).current_dir(place).env("KEBBIE_BRANCH", branch).env("KEBBIE_VERSION", version.unwrap_or_default());
    let ran = crate::sh::output_with_timeout(command, PREPARE_TIMEOUT).map_err(|error| format!("{prepare:?} did not start: {error}"))?
        .ok_or_else(|| format!("{prepare:?} ran past {} minutes", PREPARE_TIMEOUT.as_secs() / 60))?;
    if ran.code != Some(0) {
        let said = if ran.stderr.trim().is_empty() { ran.stdout.trim() } else { ran.stderr.trim() };
        return Err(format!("{prepare:?} exited {}: {said}", ran.code.map_or("on a signal".to_string(), |code| code.to_string())));
    }
    if crate::repo::is_clean(place)? {
        return Ok(());
    }
    git(place, &["add", "--all"])?;
    let message = format!("Prepare {branch}\n\nkebbie ran push.prepare ({prepare}) when it created {branch} from {}.", main_of(upstream));
    git(place, &[&UNHOOKED[..], &["commit", "--quiet", "-m", &message]].concat()).map(|_| ())
}

fn remote_branch(branch: &str) -> String {
    format!("refs/remotes/origin/{branch}")
}

// The task's branch on origin, once pushed, else the default branch: what a push to its own branch adds on top of.
fn branch_base(path: &Path, upstream: &str, branch: &str) -> String {
    let remote = remote_branch(branch);
    if crate::sh::succeeds("git", &["rev-parse", "--verify", "--quiet", &remote], Some(path)) { remote } else { crate::repo::tracking(upstream) }
}

// kebbie never force-pushes a task's branch, so a push must add commits on top of what origin holds there.
fn fast_forwards(path: &Path, branch: &str, sha: &str, task: &str) -> Result<()> {
    let remote = remote_branch(branch);
    if !crate::sh::succeeds("git", &["rev-parse", "--verify", "--quiet", &remote], Some(path)) {
        return Ok(());
    }
    let ancestor = |of: &str, sha: &str| crate::sh::succeeds("git", &["merge-base", "--is-ancestor", of, sha], Some(path));
    if ancestor(sha, &remote) {
        return Notice::new(Tone::NotNow, format!("Nothing new to push: {branch} on origin already holds it")).next("It waits under On branch until its commits reach the default branch.").detail("task", task).err();
    }
    if !ancestor(&remote, sha) {
        return Notice::new(Tone::You, format!("Not pushed: {branch} on origin has commits this task's branch does not hold"))
            .you(format!("Its pushed commits were rewritten, or someone pushed to {branch}: have its Claude put the new commits on top of origin/{branch}; kebbie never force-pushes it."))
            .detail("task", task).err();
    }
    Ok(())
}

// A push to the task's own branch: nothing is rebased or watched, and the task stays open until its commits reach the default branch, where the keeper's landed round closes it.
#[allow(clippy::too_many_arguments)]
fn deliver_to_branch(repo: &Path, slot: &Slot, upstream: &str, through: Through, out: &Out, pusher: &str, state: &Path, target: &crate::push_target::Target) -> Result<String> {
    let branch = target.branch.as_str();
    let task = task_of(slot).to_string();
    let address = peers::address(&task, &name_of(repo));
    let sha = out.sha.clone();
    let base = branch_base(&slot.path, upstream, branch);
    let pushed = git(&slot.path, &["log", "--reverse", "--format=%h %s", &format!("{base}..{sha}")])?;
    let shas: Vec<String> = git(&slot.path, &["rev-list", "--reverse", &format!("{base}..{sha}")])?.lines().map(str::to_string).collect();
    let own = through_sha(&slot.path, upstream, through)?;
    let rest = git(&slot.path, &["rev-list", "--count", &format!("{own}..HEAD")])?.parse::<usize>().unwrap_or(0);
    super::push_steps::step(&task, &format!("pushing to {branch}"));
    crate::repo::push_from(&slot.path, &[&format!("{sha}:refs/heads/{branch}")], state)
        .map_err(|error| Notice::new(Tone::Failed, "Not pushed: git refused the push").detail("task", &task).detail("error", error).to_string())?;
    let note = started_file(&task, repo).ok().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| crate::task_note::items(&text).into_iter().next());
    let went = Went { sha: sha.clone(), keep_open: false, rest, pushed: pushed.clone(), shas, note, target: branch.to_string() };
    let (_, log) = after_push(state, repo, slot, &address, &task, &went, &crate::vault_cli::detect);
    let head = git(&slot.path, &["rev-parse", "HEAD"]).ok();
    let steps_left = super::plan::left_at(state, &slot.path, &address, &own);
    let event = if rest == 0 && !steps_left { crate::phase::Event::BranchOut { sha: &own, branch } } else { crate::phase::Event::KeptOpen { rest, steps_left, clean: true, head: head.as_deref() } };
    crate::phase::record(state, &address, "push", &event);
    crate::report::forget_card_in(state, &address);
    let (count, short, main) = (pushed.lines().count(), &sha[..7.min(sha.len())], main_of(upstream));
    let logged = log.as_ref().map(|log| matches!(log, Ok(Some(_))));
    let named = crate::status::with_titles_in(state, &address);
    let created = if target.exists { String::new() } else { format!(" kebbie created {branch} from {main} for it, its own commits first.") };
    let mut lead = format!("{}: {} to {branch} ({}).{created} The task stays open under On branch, waiting for {main}; kebbie closes it once they reach {main}.", who_pushed(pusher, &named), plural(count, "commit"), pushed.lines().collect::<Vec<_>>().join("; "));
    lead.push_str(match logged {
        Some(true) => " kebbie wrote the repo's log line in the vault from these commits.",
        Some(false) => " kebbie could not write the repo's log line: write it in the vault with what landed on the branch and these shas.",
        None => " Write the repo's log line in the vault with what went to the branch and these shas.",
    });
    if let Err(error) = messages::to_lead(if logged == Some(true) { messages::ToLead::Landed } else { messages::ToLead::PushToLog }, &lead) {
        eprintln!("Could not tell the lead about the push: {error}");
    }
    let told = if target.per_task {
        format!(
            "{} pushed {} of this task, through {short}, to {branch} on origin; nothing was rebased. They reach {main} only when someone merges {branch}, and kebbie closes this task once they are on {main}. \
             Never rebase, amend or reorder the pushed commits: the next push of this task must add to {branch}, never rewrite it. Put new work in commits on top.",
            pushed_by(pusher), plural(count, "commit")
        )
    } else {
        format!(
            "{} pushed {} of this task to {branch} on origin, rebased onto {branch} in a worktree of kebbie's, through {short} there; your branch is as it was, on {main}. They reach {main} when {branch} is merged, and kebbie closes this task once they are on {main}. \
             Never rebase, amend or reorder the pushed commits: put new work in commits on top, and kebbie rebases only those onto {branch} at the next push.",
            pushed_by(pusher), plural(count, "commit")
        )
    };
    if let Err(error) = messages::notify(&address, &told) {
        eprintln!("Could not tell {address} about the push: {error}");
    }
    let next = if rest == 0 { format!("It waits under On branch, waiting for {main}; kebbie closes it once its commits reach {main}.") } else { format!("It stays open with {} on top.", plural(rest, "commit")) };
    let logged_text = log.as_ref().map(crate::task_note::logged_text).unwrap_or_default();
    Notice::new(Tone::Done, format!("Pushed {} to {branch}", plural(count, "commit"))).next(next).detail("task", &task).detail("commit", short).detail("log", &logged_text).ok()
}

// Fast-forwards the default branch on origin to sha; the remote-tracking branch follows.
pub(super) fn push_prefix(path: &Path, upstream: &str, sha: &str, state: &Path) -> Result<usize> {
    let count = git(path, &["rev-list", "--count", &format!("{}..{sha}", crate::repo::tracking(upstream))])?.parse::<usize>().map_err(|error| error.to_string())?;
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
        let check = |caller: Caller, permit: Option<Permit>, approved: bool| Pusher { caller, permit, approved }.allowed();
        assert_eq!(check(Caller::User, None, false), Ok("the user".to_string()));
        assert_eq!(check(Caller::hub("acc_01"), None, false), Ok("the user from your kebbie hub account".to_string()), "the hub pushes as the user, named");
        assert_eq!(check(Caller::Lead, Some(Permit::Task), true), Ok("the lead".to_string()));
        assert!(check(Caller::Lead, None, true).unwrap_err().contains("no push permission"));
        assert!(check(Caller::Lead, Some(Permit::Task), false).unwrap_err().contains("kebbie approve"));
        assert!(check(Caller::Lead, Some(Permit::Repo), false).unwrap_err().contains("kebbie approve"));
        assert_eq!(check(Caller::Lead, Some(Permit::Repo), true), Ok("the lead".to_string()), "the repo's permission needs no review");
        for agent in [Caller::Task("billing@app".to_string()), Caller::Agent] {
            assert!(check(agent, Some(Permit::Task), true).unwrap_err().contains("no agent pushes"), "a permission is the lead's, never a task's");
        }
    }

    #[test]
    fn after_a_push_the_lead_hears_what_landed_and_is_cued_to_log_it_and_settle_the_note() {
        let commits = "a1b2c3d Add refunds\ne4f5a6b Show refunds on the invoice";
        let done = pushed_notice(&Landed { pusher: "the lead", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing-refunds.md"), logged: None });
        assert_eq!(done, "The lead pushed billing@app: 2 commits to main (a1b2c3d Add refunds; e4f5a6b Show refunds on the invoice). The task is done and closed. Write the repo's log line in the vault with what landed and these shas, and close or update its note Projects/billing-refunds.md.");
        let open = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits: "a1b2c3d Add refunds", closed: false, note: None, logged: None });
        assert!(open.starts_with("The user pushed billing@app: 1 commit to main (a1b2c3d Add refunds). The task stays open"), "{open}");
        assert!(open.ends_with("these shas."), "{open}");
    }

    #[test]
    fn with_the_vault_on_the_lead_hears_kebbie_logged_the_push_and_is_not_asked_to_log_it() {
        let commits = "a1b2c3d Add refunds";
        let kept = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing.md"), logged: Some(true) });
        assert!(kept.contains("kebbie wrote the repo's log line") && !kept.contains("Write the repo's log line"), "{kept}");
        assert!(!kept.contains("area note") && !failed_text(commits).contains("area note"), "kebbie logs the push itself; the notice carries no chore for the lead's area note: {kept}");
        let failed = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing.md"), logged: Some(false) });
        assert!(failed.contains("kebbie could not write the repo's log line: write it in the vault with what landed and these shas") && !failed.contains("kebbie wrote"), "a log append that failed leaves the line to the lead: {failed}");
    }

    #[test]
    fn the_lead_hears_of_a_closed_task_by_the_title_the_close_forgets() {
        let state = crate::sh::test_dir("push-named");
        crate::status::keep_title(&state, "billing@app", "Refunds on invoices");
        let named = crate::status::with_titles_in(&state, "billing@app");
        crate::status::forget_title(&state, "billing@app");
        let done = pushed_notice(&Landed { pusher: "the user", address: &named, target: "main", commits: "a1b2c3d Add refunds", closed: true, note: None, logged: Some(true) });
        assert!(done.starts_with("The user pushed Refunds on invoices (billing@app): 1 commit"), "{done}");
        crate::status::keep_title(&state, "billing@app", "Refunds on invoices");
        assert_eq!(crate::status::with_titles_in(&state, &done), done, "a title still kept is not added twice");
        let address = format!("named-{}@app", std::process::id());
        crate::status::keep_title(&state, &address, "Refunds on invoices");
        let handed = std::cell::RefCell::new(None);
        let closed = close_pushed(&state, "push", told(&address), |commits| {
            crate::status::forget_title(&state, &address);
            *handed.borrow_mut() = Some(commits.to_string());
            Ok(Notice::new(Tone::Done, "Closed"))
        });
        assert!(closed.is_ok());
        assert_eq!(handed.into_inner().as_deref(), Some("a1b2c3d Add refunds"), "the close records the commits the lead hears of");
        let heard = lead_about(&address);
        assert!(heard.len() == 1 && heard[0].text.starts_with(&format!("The user pushed Refunds on invoices ({address}): 1 commit")) && heard[0].text.contains("The task is done and closed"), "the close forgets the title, so the notice is named before it: {heard:?}");
        std::fs::remove_dir_all(state).unwrap();
    }

    fn told(address: &str) -> Landed<'_> {
        Landed { pusher: "the user", address, target: "main", commits: "a1b2c3d Add refunds", closed: false, note: None, logged: Some(true) }
    }

    fn lead_about(address: &str) -> Vec<messages::Message> {
        messages::load().into_iter().filter(|message| message.to == crate::status::LEAD_LABEL && message.text.contains(address)).collect()
    }

    #[test]
    fn a_pushed_task_closes_without_asking_its_note_and_one_that_fails_to_close_needs_the_user() {
        use crate::phase::{Phase, Why};
        let state = crate::sh::test_dir("push-closing");
        let id = std::process::id();
        let (noted, failing) = (format!("noted-{id}@app"), format!("failing-{id}@app"));
        std::fs::create_dir_all(state.join("started")).unwrap();
        std::fs::write(state.join("started").join(&noted), "Projects/app-noted.md").unwrap();
        let closed = close_pushed(&state, "ci", told(&noted), |_| Ok(Notice::new(Tone::Done, "Closed")));
        assert!(closed.is_ok(), "a task with a note closes as one without");
        assert!(lead_about(&noted).iter().any(|message| message.text.contains("The task is done and closed")));
        let failed = close_pushed(&state, "push", told(&failing), |_| Err("herdr is gone".to_string()));
        assert!(matches!(failed, Err(error) if error == "herdr is gone"));
        let record = crate::phase::load(&state, &failing).unwrap();
        assert_eq!((record.phase, record.by.as_str()), (Phase::NeedsYou(Why::Failed), "push"));
        assert!(lead_about(&failing).iter().any(|message| message.text.contains("The task stays open with the rest of its branch")));
        std::fs::remove_dir_all(state).unwrap();
    }

    fn failed_text(commits: &str) -> String {
        pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: None, logged: Some(false) })
    }

    fn told_lead(address: &str) -> Vec<messages::Message> {
        messages::load().into_iter().filter(|message| message.to == crate::status::LEAD_LABEL && message.text.contains(&format!("pushed {address}:"))).collect()
    }

    fn watching(state: &Path, repo: &Path, address: &str, sha: &str, close: bool, pushed: crate::ci::Pushed) -> crate::ci::Begin {
        let watch = crate::ci::Watch { address: address.to_string(), repo: repo.to_path_buf(), slug: "acme/app".to_string(), sha: sha.to_string(), started_at: 0, checked_at: None, close, runs: Vec::new(), state: crate::ci::State::Running, error: None, erring_since: None, following: None, pushed };
        crate::ci::save(state, &watch).unwrap();
        crate::ci::Begin::Watching
    }

    #[test]
    fn a_push_notice_reaches_the_lead_live_only_when_kebbie_could_not_write_the_log_line() {
        let landed = |address: &str, logged: Option<bool>| tell_lead_pushed(&Landed { pusher: "the user", address, target: "main", commits: "a1b2c3d Add refunds", closed: true, note: None, logged });
        let id = std::process::id();
        let (wrote, failed, off) = (format!("wrote-{id}@app"), format!("failed-{id}@app"), format!("off-{id}@app"));
        landed(&wrote, Some(true));
        landed(&failed, Some(false));
        landed(&off, None);
        let heard = |address: &str| told_lead(address).iter().map(|message| (message.fyi, message.kind.clone())).collect::<Vec<_>>();
        assert_eq!(heard(&wrote), [(true, Some("landed".to_string()))], "logged by kebbie, the lead reads it in its digest");
        assert_eq!(heard(&failed), [(false, Some("push".to_string()))], "a log line kebbie could not write is the lead's to write now");
        assert_eq!(heard(&off), [(false, Some("push".to_string()))], "without the vault the lead writes the log line");
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head, rebased: false, to: None }, "the user", &state, &watching).unwrap();
        let told = told_lead(&address);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(told[0].fyi && told[0].kind.is_none() && told[0].text.starts_with(&format!("The user pushed {address}: 1 commit to {}. kebbie watches CI", upstream.trim_start_matches("origin/"))), "the push under CI watch is a notice for the digest, not a turn of the lead's: {:?}", told[0]);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_full_push_empties_the_tasks_build_output_and_says_how_much_while_a_push_the_task_or_a_check_follows_leaves_it() {
        let built = |slot: &Slot| {
            std::fs::create_dir_all(slot.path.join("target/debug")).unwrap();
            std::fs::write(slot.path.join("target/debug/app"), vec![0u8; 1_500_000]).unwrap();
        };
        let rust_task = |name: &str| {
            let (dir, origin, slot, upstream) = task_with_a_commit(name);
            upstream_commit(&dir, ".gitignore", "target/\n", "ignore target");
            upstream_commit(&dir, "Cargo.toml", "[package]\n", "a Rust repo");
            git(&slot.path, &["rebase", "--quiet", &upstream]).unwrap();
            built(&slot);
            (dir, origin, slot, upstream)
        };
        let (dir, origin, slot, upstream) = rust_task("billing");
        let state = state_of(&dir);
        let done = agent_worktree(&dir, &upstream, "agent-done");
        untouched_for_two_days(&done);
        let dirty = agent_worktree(&dir, &upstream, "agent-dirty");
        std::fs::write(dirty.join("draft"), "draft").unwrap();
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let said = deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head, rebased: false, to: None }, "the user", &state, &watching).unwrap();
        assert!(said.contains("removed target/ (1.5 MB), .claude/worktrees/agent-done (") && said.contains("; kept .claude/worktrees/agent-dirty (uncommitted changes)"), "CI still runs, but the build is not needed past the push: {said}");
        assert!(!slot.path.join("target").exists() && !done.exists());
        crate::ci::forget(&state, &crate::peers::address("billing", &name_of(&dir)));
        cleanup(dir, origin, &slot);

        let (dir, origin, slot, upstream) = rust_task("rates");
        let state = state_of(&dir);
        let first = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(slot.path.join("more"), "more").unwrap();
        git(&slot.path, &["add", "more"]).unwrap();
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", "more"]).unwrap();
        let said = deliver(&dir, &slot, &upstream, Through::First { count: 1, of: 2 }, &Out { sha: first, rebased: false, to: None }, "the user", &state, &watching).unwrap();
        assert!(!said.contains("removed"), "{said}");
        assert!(slot.path.join("target/debug/app").exists(), "the task builds on, so its build stays");
        crate::ci::forget(&state, &crate::peers::address("rates", &name_of(&dir)));
        cleanup(dir, origin, &slot);
        let unwatched = |_: &Path, _: &Path, _: &str, _: &str, _: bool, _: crate::ci::Pushed| crate::ci::Begin::Off;
        let checked = |state: &Path, address: &str| crate::report::save_check(state, address, "the old key gets 401").unwrap();
        let verified = |state: &Path, address: &str| crate::verify_after::name(state, address, "the refunds page loads").unwrap();
        for (followed, by) in [(&checked as &dyn Fn(&Path, &str), "the task's check"), (&verified, "the user's verification")] {
            for ci in [&watching as Watch, &unwatched] {
                let (dir, origin, slot, upstream) = rust_task("refunds");
                let state = state_of(&dir);
                let address = crate::peers::address("refunds", &name_of(&dir));
                followed(&state, &address);
                let said = deliver(&dir, &slot, &upstream, Through::All, &Out { sha: git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebased: false, to: None }, "the user", &state, ci).unwrap();
                assert!(!said.contains("removed target/") && slot.path.join("target/debug/app").exists(), "with {by} to follow, the build stays until the close releases the worktree: {said}");
                crate::ci::forget(&state, &address);
                crate::verify_after::forget(&state, &address);
                crate::report::forget_check_in(&state, &address);
                crate::kebbie_questions::forget(&address);
                cleanup(dir, origin, &slot);
            }
        }
    }

    #[test]
    fn a_push_without_ci_tells_the_blocked_tasks_of_its_repo_it_landed() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let at = |task: &str| peers::address(task, &name_of(&dir));
        let blocked = messages::Report { state: "blocked".to_string(), next: "nothing".to_string(), head: None, after: Vec::new() };
        messages::log_report(&at("copy"), &blocked, "waiting for billing's push").unwrap();
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let unwatched = |_: &Path, _: &Path, _: &str, _: &str, _: bool, _: crate::ci::Pushed| crate::ci::Begin::Off;
        deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head.clone(), rebased: false, to: None }, "the user", &state, &unwatched).unwrap();
        let told: Vec<String> = messages::load().into_iter().filter(|message| message.to == at("copy")).map(|message| message.text).collect();
        assert!(told.len() == 1 && told[0].starts_with(&format!("{} landed on", at("billing"))) && told[0].contains(&head[..7]) && told[0].contains("watches no CI"), "{told:?}");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_whose_close_fails_still_tidies_once_and_names_each_kept_agent_worktree_once() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let dirty = agent_worktree(&dir, &upstream, "agent-dirty");
        std::fs::write(dirty.join("draft"), "draft").unwrap();
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let unwatched = |_: &Path, _: &Path, _: &str, _: &str, _: bool, _: crate::ci::Pushed| crate::ci::Begin::Off;
        let said = deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head, rebased: false, to: None }, "the user", &state, &unwatched).unwrap();
        assert!(said.contains("but the task did not close"), "closing needs herdr, which tests have not: {said}");
        assert_eq!(said.matches("agent-dirty").count(), 1, "{said}");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_carries_the_tasks_check_to_its_watch_and_without_ci_asks_for_it_rather_than_closing() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        crate::report::save_check(&state, &address, "the old key gets 401").unwrap();
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let said = deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head, rebased: false, to: None }, "the user", &state, &watching).unwrap();
        assert!(said.contains("once it is green the task checks the old key gets 401"), "{said}");
        assert_eq!(crate::ci::load(&state, &address).and_then(|watch| watch.pushed.check), Some(crate::ci::Check::new("the old key gets 401")), "asked only once CI is green");
        cleanup(dir, origin, &slot);
        let (dir, origin, slot, upstream) = task_in_another_repo("rates");
        let state = state_of(&dir);
        let address = crate::peers::address("rates", &name_of(&dir));
        crate::report::save_check(&state, &address, "the rates page shows 2027").unwrap();
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let unwatched = |_: &Path, _: &Path, _: &str, _: &str, _: bool, _: crate::ci::Pushed| crate::ci::Begin::Off;
        let said = deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head.clone(), rebased: false, to: None }, "the user", &state, &unwatched).unwrap();
        assert!(said.contains("The task checks the rates page shows 2027 now"), "{said}");
        assert!(find_task(&dir, Some("rates"), &dir).is_ok(), "with no CI to wait for, the check runs at once and the task stays open for it");
        let watch = crate::ci::load(&state, &address).unwrap();
        assert!(watch.checking().is_some_and(|check| check.what == "the rates page shows 2027" && !check.passed) && watch.close, "{watch:?}");
        assert_eq!(crate::phase::load(&state, &address).map(|record| record.phase), Some(crate::phase::Phase::CiRunning { sha: head }));
        assert!(messages::load().iter().any(|message| message.to == address && message.text.contains("Run the check you named for after the push now: the rates page shows 2027")));
        crate::ci::forget(&state, &address);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_with_a_check_for_the_user_asks_it_rather_than_closing_without_ci_and_through_its_last_commit_kept_open() {
        let solved = |state: &Path, address: &str| crate::kebbie_questions::load(state).into_iter().filter(|asked| asked.address == address && asked.kind == crate::kebbie_questions::Kind::Solved).count();
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        crate::verify_after::name(&state, &address, "the refunds page loads").unwrap();
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let unwatched = |_: &Path, _: &Path, _: &str, _: &str, _: bool, _: crate::ci::Pushed| crate::ci::Begin::Off;
        let said = deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head, rebased: false, to: None }, "the user", &state, &unwatched).unwrap();
        assert!(said.contains("waits on your verification"), "{said}");
        assert!(find_task(&dir, Some("billing"), &dir).is_ok(), "with no CI, the push asks the user and the task stays open");
        assert_eq!(solved(&state, &address), 1);
        assert!(crate::verify_after::load(&state, &address).and_then(|held| held.landed).is_some_and(|landed| landed.ends_with("fourth")), "the done record keeps the pushed commits");
        cleanup(dir, origin, &slot);
        let (dir, origin, slot, upstream) = task_in_another_repo("rates");
        commit(&slot.path, "e", "fifth");
        let state = state_of(&dir);
        let address = crate::peers::address("rates", &name_of(&dir));
        crate::verify_after::name(&state, &address, "the rates page shows 2027").unwrap();
        deliver(&dir, &slot, &upstream, Through::First { count: 2, of: 2 }, &Out { sha: git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebased: false, to: None }, "the user", &state, &unwatched).unwrap();
        assert_eq!(solved(&state, &address), 1, "kept open with everything on main and no CI, kebbie asks the user's check");
        assert!(crate::kebbie_questions::load(&state).iter().all(|asked| asked.address != address || asked.kind != crate::kebbie_questions::Kind::Close), "and not whether to close it");
        cleanup(dir, origin, &slot);
        let (dir, origin, slot, upstream) = task_with_a_commit("refunds");
        commit(&slot.path, "e", "fifth");
        let state = state_of(&dir);
        let address = crate::peers::address("refunds", &name_of(&dir));
        crate::verify_after::name(&state, &address, "the refunds page loads").unwrap();
        crate::report::save_check(&state, &address, "the old key gets 401").unwrap();
        let said = deliver(&dir, &slot, &upstream, Through::First { count: 2, of: 2 }, &Out { sha: git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebased: false, to: None }, "the user", &state, &watching).unwrap();
        assert!(!said.contains("asks you whether to close it"), "{said}");
        assert_eq!(solved(&state, &address), 0, "with CI to watch, nothing is asked before it is green");
        let watch = crate::ci::load(&state, &address).unwrap();
        assert!(watch.close && watch.pushed.check == Some(crate::ci::Check::new("the old key gets 401")), "followed as a whole push, the task's own check first: {watch:?}");
        crate::ci::forget(&state, &address);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_logs_its_commits_in_the_vault_and_renews_the_task_notes_claim() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        let fake = crate::vault_cli::fake::Fake::new(&format!("push-{}", name_of(&dir)));
        fake.answer("task-claim", r#"{"path":"Projects/api-billing.md","claimed":true}"#);
        std::fs::create_dir_all(state.join("started")).unwrap();
        std::fs::write(state.join("started").join(&address), "Projects/api-billing.md").unwrap();
        let went = went_out(&dir, &slot, &upstream, Through::All, &Out { sha: git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebased: false, to: None }).unwrap();
        let claiming = fake.claiming();
        let (vault, log) = after_push(&state, &dir, &slot, &address, "billing", &went, &|| Some(claiming.clone()));
        assert!(vault.is_some() && log.is_some(), "the vault that answered is the one the report and the close use");
        let calls = fake.calls();
        assert!(calls.iter().any(|call| call.starts_with("log ")), "the push writes the repo's log line: {calls:?}");
        assert_eq!(calls.last().map(String::as_str), Some(format!("task claim Projects/api-billing.md --machine mac --task billing --repo {}", name_of(&dir)).as_str()), "then renews the claim, so a note still worked on never reads as stale elsewhere");
        let _ = std::fs::remove_dir_all(&fake.dir);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn git_pushes_with_the_ci_watch_and_ci_running_already_written_and_a_refused_push_puts_both_back() {
        use crate::phase::{Event, Phase};
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        let phase = |state: &Path| crate::phase::load(state, &address).map(|record| record.phase);
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        crate::phase::record(&state, &address, "report", &Event::Reported { state: "ready", head: Some(&head), committed: true, pushing: false });
        git(&dir, &["remote", "set-url", "origin", &dir.with_extension("nowhere").to_string_lossy()]).unwrap();
        assert!(deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head.clone(), rebased: false, to: None }, "the user", &state, &watching).unwrap_err().contains("git refused the push"));
        assert!(crate::ci::load(&state, &address).is_none(), "a refused push leaves no watch");
        assert_eq!(phase(&state), Some(Phase::Ready { head: Some(head.clone()) }), "and leaves the phase where it was");
        git(&dir, &["remote", "set-url", "origin", &origin.to_string_lossy()]).unwrap();
        let seen = dir.with_extension("seen");
        let _ = std::fs::remove_dir_all(&seen);
        crate::executable::write(dir.join(".git/hooks/pre-push"), format!("#!/bin/sh\ncp -R '{}' '{}'\n", state.display(), seen.display())).unwrap();
        deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head.clone(), rebased: false, to: None }, "the user", &state, &watching).unwrap();
        assert_eq!(crate::ci::load(&seen, &address).map(|watch| watch.sha), Some(head.clone()), "between the push and the watch a task is neither pushable nor CI running");
        assert_eq!(phase(&seen), Some(Phase::CiRunning { sha: head.clone() }), "CI running is written before the push, so the push never shows anywhere else");
        assert_eq!(git(&origin, &["rev-parse", upstream.trim_start_matches("origin/")]).unwrap(), head, "pushed");
        std::fs::remove_dir_all(seen).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_rebases_elsewhere_in_kebbies_private_temp_folder_emptied_of_anything_left_there() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let sha = "0123456789abcdef0123";
        let place = push_place(sha).unwrap();
        let root = place.parent().unwrap();
        // SAFETY: getuid cannot fail.
        let uid = unsafe { libc::getuid() };
        assert_eq!(root, Path::new("/tmp").canonicalize().unwrap().join(format!("kebbie-{uid}")), "never the shared temp dir, where anyone can plant the folder first");
        let meta = std::fs::symlink_metadata(root).unwrap();
        assert!(meta.is_dir() && meta.uid() == uid && meta.permissions().mode() & 0o077 == 0, "the folder is private to the user");
        assert!(!place.exists(), "git worktree add makes the folder itself");
        std::fs::create_dir_all(place.join("left")).unwrap();
        assert_eq!(push_place(sha).unwrap(), place);
        assert!(!place.exists(), "what an earlier push left is gone");
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
        let Prepared::Push(pusher, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, through, None, &hand, None).unwrap() else { panic!("a clean rebase pushes") };
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
        let Prepared::Push(_, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, through, None, &hand, None).unwrap() else { panic!("a clean rebase pushes") };
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
        let Prepared::Push(_, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, rest, None, &hand, None).unwrap() else { panic!("the rest pushes") };
        deliver(&dir, &slot, &upstream, rest, &out, "the user", &state_of(&dir), &crate::ci::begin).unwrap();
        let branch = upstream.trim_start_matches("origin/");
        assert_eq!(git(&origin, &["log", "--format=%s", "-3", branch]).unwrap(), "fifth\nfourth\nsomeone else's work", "fifth went out once, fourth not twice");
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), sixth, "the worktree is still the Claude's");
        let all = chosen(&slot, &upstream, None, false).unwrap();
        assert!(matches!(prepare(&pushing.state, &Caller::User, &dir, &slot, all, None, &hand, None), Ok(Prepared::Push(..))), "the rebased rest goes out with no test run first");
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
        assert_eq!(prepare(&pushing.state, &Caller::User, &dir, &slot, through, None, &hand, None).unwrap(), Prepared::Push("the user".to_string(), Out { sha: fourth.clone(), rebased: false, to: None }));
        let state = state_of(&dir);
        let said = deliver(&dir, &slot, &upstream, through, &Out { sha: fourth.clone(), rebased: false, to: None }, "the user", &state, &crate::ci::begin).unwrap();
        assert!(said.contains("It stays open with 1 commit on top"), "{said}");
        assert_eq!(crate::assessment::pushed_in(&state, &crate::peers::address("billing", &name_of(&dir))), std::slice::from_ref(&fourth), "the pushed step stays in the task's review");
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
    fn a_push_of_the_first_commits_that_overlaps_goes_out_rebased_elsewhere_and_one_that_conflicts_asks_its_claude_to_rebase_and_changes_nothing_itself() {
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
        assert!(matches!(prepare(&pushing.state, &Caller::User, &dir, &slot, through, None, &hand, None).unwrap(), Prepared::Push(_, Out { rebased: true, .. })), "main changed m too, and the clean rebase goes out with no test run first");
        assert!(texts.borrow().is_empty(), "nothing goes to its Claude: {:?}", texts.borrow());
        assert_eq!((git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), std::fs::read_to_string(slot.path.join("e")).unwrap()), (head.clone(), "half done".to_string()));
        assert!(intent::load(&pushing.state, &pushing.address).is_none(), "no push waits on its Claude");
        upstream_commit(&dir, "d", "theirs", "d on main");
        assert!(matches!(prepare(&pushing.state, &Caller::User, &dir, &slot, through, None, &hand, None).unwrap(), Prepared::Handed(said) if said.contains("conflicts with")));
        let typed = texts.borrow()[0].clone();
        assert!(typed.contains("kebbie changed nothing in your worktree") && typed.contains("resolve the conflict in them") && typed.contains("with no test run first"), "{typed}");
        assert_eq!((git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebase_in_progress(&slot.path), worktrees_of(&dir)), (head, false, 2), "the conflict was kebbie's own, aborted elsewhere");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_after_main_moved_goes_out_at_once_in_a_full_suite_and_linted_repo_whatever_its_runs_and_its_overlap() {
        use crate::evidence::{Outcome, Run};
        use crate::permit::Caller;
        let lint = "cargo clippy --locked --all-targets -- -D warnings";
        let settings = format!("[tests]\nbefore_push = \"full\"\nfull_command = \"scripts/check.sh\"\nlint_command = \"{lint}\"\n");
        for (runs, overlapping) in [(true, false), (false, false), (true, true), (false, true)] {
            let (dir, origin, slot, upstream) = task_with_a_commit("billing");
            let pushing = Pushing::new(&dir);
            let file = crate::config::repo_file(&dir);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, &settings).unwrap();
            if overlapping {
                shared_file(&dir, &slot, &upstream);
            }
            let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
            if runs {
                for (command, lint) in [("scripts/check.sh", false), (lint, true)] {
                    crate::evidence::save(&pushing.state, &pushing.address, &Run { command: command.to_string(), runner: command.to_string(), result: Outcome::Passed, sha: head.clone(), code: head.clone(), lint, ..Run::default() }).unwrap();
                }
            }
            if overlapping {
                upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
            } else {
                upstream_commit(&dir, "elsewhere", "x", "someone else's work");
            }
            let (texts, hand) = handed();
            let prepared = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None);
            assert!(matches!(&prepared, Ok(Prepared::Push(..))), "runs {runs}, overlapping {overlapping}: the rebased task goes out with no test or lint run first, as CI tests it: {prepared:?}");
            assert!(texts.borrow().is_empty() && intent::load(&pushing.state, &pushing.address).is_none(), "nothing goes back to its Claude: {:?}", texts.borrow());
            join(&pushing.state, &Caller::User, &dir, &slot, Through::All).expect("the push joins the line on the rebased code, with no run on it");
            intent::forget(&pushing.state, &pushing.address);
            if !overlapping {
                commit(&slot.path, "e", "fifth");
                assert!(matches!(prepare(&pushing.state, &Caller::User, &dir, &slot, Through::First { count: 1, of: 2 }, None, &hand, None), Ok(Prepared::Push(..))), "a push of the first commits needs no run where it ends either");
            }
            let _ = std::fs::remove_file(file);
            cleanup(dir, origin, &slot);
        }
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
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Conflict(vec!["d".to_string()]), "the conflict names the file both sides changed");
        assert!(!rebase_in_progress(&slot.path) && git(&slot.path, &["rev-parse", "HEAD"]).unwrap() == head, "a conflict is aborted and leaves the branch as it was");
        cleanup(dir, origin, &slot);
    }

    const NOTES: &str = "# Changes\n\n## Unreleased\n\n### What's new\n- old feature\n\n### Fixed\n- old fix\n\n## 0.1.0\n\n### Fixed\n- first fix\n";

    fn notes_on_both_sides(task_lines: &str, main_lines: &str) -> (std::path::PathBuf, std::path::PathBuf, crate::tasks::Slot, String) {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        upstream_commit(&dir, "CHANGES.md", NOTES, "add notes");
        git(&slot.path, &["rebase", "--quiet", &upstream]).unwrap();
        std::fs::write(slot.path.join("CHANGES.md"), NOTES.replacen("### Fixed\n", &format!("### Fixed\n{task_lines}"), 1)).unwrap();
        git(&slot.path, &["commit", "--quiet", "-am", "task line"]).unwrap();
        upstream_commit(&dir, "CHANGES.md", &NOTES.replacen("### Fixed\n", &format!("### Fixed\n{main_lines}"), 1), "main line");
        (dir, origin, slot, upstream)
    }

    fn unreleased(slot: &crate::tasks::Slot) -> crate::changes::Section {
        crate::changes::parse(&std::fs::read_to_string(slot.path.join("CHANGES.md")).unwrap()).into_iter().find(|section| section.version == "Unreleased").unwrap()
    }

    #[test]
    fn a_rebase_stopped_only_on_changes_md_keeps_both_sides_lines_under_their_heading_and_goes_on() {
        let (dir, origin, slot, upstream) = notes_on_both_sides("- the task's fix\n", "- main's fix\n");
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Clean, "CHANGES.md alone never conflicts, and asks for no test run");
        assert!(!rebase_in_progress(&slot.path) && ahead_behind(&slot.path, &upstream).unwrap().1 == 0, "the rebase finished on top of main");
        let notes = unreleased(&slot);
        assert_eq!(notes.fixed, ["main's fix", "the task's fix", "old fix"], "each line once, under Fixed of Unreleased");
        assert_eq!(notes.new, ["old feature"]);
        assert!(git(&slot.path, &["status", "--porcelain"]).unwrap().is_empty(), "the worktree holds the merged file, committed");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_changes_md_union_beside_another_conflict_stays_a_conflict() {
        let (dir, origin, slot, upstream) = notes_on_both_sides("- the task's fix\n", "- main's fix\n");
        std::fs::write(slot.path.join("d"), "mine").unwrap();
        git(&slot.path, &["add", "d"]).unwrap();
        git(&slot.path, &["commit", "--quiet", "-m", "d in the task"]).unwrap();
        upstream_commit(&dir, "d", "theirs", "d on main");
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let Updated::Conflict(files) = update(&slot.path, &upstream).unwrap() else { panic!("d conflicts") };
        assert!(files.contains(&"d".to_string()), "{files:?}");
        assert!(!rebase_in_progress(&slot.path) && git(&slot.path, &["rev-parse", "HEAD"]).unwrap() == head, "the whole rebase is aborted, its union with it");
        cleanup(dir, origin, &slot);
    }

    // The task and main both change CHANGES.md apart, a docs page and, with code, the file m.
    fn text_on_both_sides(dir: &std::path::Path, slot: &crate::tasks::Slot, upstream: &str, code: bool) {
        upstream_commit(dir, "CHANGES.md", NOTES, "add notes");
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        upstream_commit(dir, "docs/push.md", "1\n2\n3\n4\n5\n6\n7\n8\n", "add a page");
        if code {
            shared_file(dir, slot, upstream);
        }
        git(&slot.path, &["rebase", "--quiet", upstream]).unwrap();
        std::fs::write(slot.path.join("CHANGES.md"), NOTES.replacen("- old fix\n", "- old fix\n- the task's fix\n", 1)).unwrap();
        std::fs::write(slot.path.join("docs/push.md"), "one\n2\n3\n4\n5\n6\n7\n8\n").unwrap();
        git(&slot.path, &["commit", "--quiet", "-am", "task text"]).unwrap();
        upstream_commit(dir, "CHANGES.md", &NOTES.replacen("- old feature\n", "- old feature\n- main's feature\n", 1), "main's note");
        upstream_commit(dir, "docs/push.md", "1\n2\n3\n4\n5\n6\n7\neight\n", "main's page");
        if code {
            upstream_commit(dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        }
    }

    #[test]
    fn changes_md_and_docs_changed_on_both_sides_are_no_overlap_and_code_beside_them_is() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        text_on_both_sides(&dir, &slot, &upstream, false);
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Clean);
        cleanup(dir, origin, &slot);
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        text_on_both_sides(&dir, &slot, &upstream, true);
        let Updated::Overlap(touched) = update(&slot.path, &upstream).unwrap() else { panic!("main changed m too") };
        assert_eq!(touched.iter().map(|touched| touched.file.as_str()).collect::<Vec<_>>(), ["m"], "only the code file asks for its tests");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_over_text_on_both_sides_goes_out_at_once() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        text_on_both_sides(&dir, &slot, &upstream, false);
        let (texts, hand) = handed();
        let outcome = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap();
        assert!(matches!(&outcome, Prepared::Push(..)) && texts.borrow().is_empty(), "{outcome:?}");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_changes_md_union_that_would_repeat_a_heading_stays_a_conflict() {
        let (dir, origin, slot, upstream) = notes_on_both_sides("- the task's fix\n\n### Changed\n- the task's change\n", "- main's fix\n\n### Changed\n- main's change\n");
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Conflict(vec!["CHANGES.md".to_string()]), "a new Changed heading on each side goes to its Claude");
        assert!(!rebase_in_progress(&slot.path) && git(&slot.path, &["rev-parse", "HEAD"]).unwrap() == head, "the branch is left as it was");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_changes_md_side_that_rewords_or_drops_a_line_stays_a_conflict() {
        let (dir, origin, slot, upstream) = notes_on_both_sides("- the task's fix\n", "- main's fix\n");
        git(&slot.path, &["reset", "--quiet", "--hard", "HEAD~1"]).unwrap();
        std::fs::write(slot.path.join("CHANGES.md"), NOTES.replacen("### Fixed\n- old fix\n", "### Fixed\n- the old fix, said better\n", 1)).unwrap();
        git(&slot.path, &["commit", "--quiet", "-am", "reword a line"]).unwrap();
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Conflict(vec!["CHANGES.md".to_string()]), "a reworded line would sit beside its old wording");
        assert!(!rebase_in_progress(&slot.path) && git(&slot.path, &["rev-parse", "HEAD"]).unwrap() == head, "the branch is left as it was");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_side_only_adds_when_it_keeps_every_line_of_the_base_in_order() {
        assert!(only_adds("a\nb\n", "new\na\nnew\nb\n"));
        assert!(only_adds("", "a\n"));
        assert!(!only_adds("a\nb\n", "a\nb said better\n"), "a reworded line");
        assert!(!only_adds("a\nb\n", "b\n"), "a dropped line");
        assert!(!only_adds("a\nb\n", "b\na\n"), "lines moved out of order");
    }

    #[test]
    fn a_heading_counts_twice_only_within_one_part_of_the_notes() {
        assert!(!heading_twice(NOTES), "Fixed under two versions is two parts");
        assert!(heading_twice("## Unreleased\n### Fixed\n- a\n### Fixed\n- b\n"));
        assert!(heading_twice("## Unreleased\n## Unreleased\n"));
    }

    #[test]
    fn a_rebase_under_way_in_the_worktree_is_left_running_and_never_met_by_a_second_one() {
        let (dir, origin, slot, upstream) = task_with_a_commit("twice");
        upstream_commit(&dir, "d", "theirs", "d on main");
        assert!(git(&slot.path, &["rebase", "--quiet", &upstream]).is_err(), "the other rebase stops on its conflict");
        let refused = update(&slot.path, &upstream).unwrap_err();
        assert!(refused.contains("Not rebased: a rebase is under way in its worktree"), "{refused}");
        assert!(rebase_in_progress(&slot.path), "kebbie never aborts a rebase it did not start");
        git(&slot.path, &["rebase", "--abort"]).unwrap();
        let held = RebaseLock::take(&slot.path).unwrap();
        let refused = update(&slot.path, &upstream).unwrap_err();
        assert!(refused.contains("Not rebased: kebbie is rebasing its worktree already"), "{refused}");
        drop(held);
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Conflict(vec!["d".to_string()]), "once the first is done, the next one runs");
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
        let base = || crate::board::review_data::resolve_base(&slot.path, &crate::board::review_data::base_spec(&d));
        let files = || crate::board::review_data::files(&d, &slot.path, &base(), None).into_iter().map(|file| file.path).collect::<Vec<_>>();
        assert_eq!(files(), ["d"]);
        land_on_main(&dir, &upstream, |_| {});
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Clean);
        assert_eq!(files(), ["d"], "main's commits stay out of a review opened before the rebase");
        assert_eq!(base(), git(&slot.path, &["merge-base", "HEAD", &upstream]).unwrap(), "the diff compares against the fork point after the rebase too");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_rebase_git_refuses_to_start_names_the_hidden_files_keeps_gits_reason_in_a_file_and_is_no_conflict() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&slot.path, &["update-index", "--skip-worktree", "a"]).unwrap();
        std::fs::write(slot.path.join("a"), "local only").unwrap();
        assert!(uncommitted(&slot.path).unwrap().is_empty(), "git status does not show it");
        upstream_commit(&dir, "a", "theirs", "a on main");
        let refused = update(&slot.path, &upstream).unwrap_err();
        assert!(refused.contains("skip-worktree") && refused.contains("hidden: a ("), "{refused}");
        assert!(!refused.contains("overwritten"), "git's own words stay out of the notice: {refused}");
        assert!(kept_error(&slot).contains("overwritten"), "git's own reason is kept in the file the notice names");
        assert!(!rebase_in_progress(&slot.path), "{refused}");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_rebase_whose_abort_fails_too_still_reports_gits_first_error() {
        let source = include_str!("push.rs");
        let once = &source[source.find("fn rebase_once(").unwrap()..];
        let once = &once[..once.find("\n}\n").unwrap()];
        assert!(once.contains("if let Err(abort) = git_writing(path, &[options, &[\"rebase\", \"--abort\"]].concat())") && once.contains("refused(path, upstream, &format!(\"{error}; git rebase --abort then failed: {abort}\"), 1)"), "\"no rebase in progress\" from the abort never stands in for why the rebase stopped");
    }

    #[test]
    fn a_rebase_git_stops_without_a_conflicted_file_is_tried_again_with_hooks_off_then_said_in_one_line() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        git(&dir, &["config", "--unset", "user.email"]).unwrap();
        let refused = update(&slot.path, &upstream).unwrap_err();
        let notice = crate::notice::Notice::parse(&refused).expect("a notice");
        assert_eq!(notice.headline, format!("Not rebased: git stopped the rebase onto {} on an error, not a conflict", main_of(&upstream)), "{refused}");
        assert!(notice.next.as_deref().is_some_and(|next| next.contains("try again, with p on the board for a push") && next.contains("tried 3 times, the last 2 times with hooks off")), "{refused}");
        assert!(!refused.contains("no email was given") && kept_error(&slot).contains("no email was given"), "git's own words go to the file, not the dialog: {refused}");
        assert!(!rebase_in_progress(&slot.path) && git(&slot.path, &["rev-parse", "HEAD"]).unwrap() == head, "the stopped rebase is aborted and leaves the branch as it was");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_rebase_a_hook_stops_goes_through_on_the_try_with_hooks_off() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        commit(&slot.path, "e", "fifth");
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        let hooks = slot.path.join("../hooks-of-billing");
        std::fs::create_dir_all(&hooks).unwrap();
        crate::executable::write(hooks.join("pre-rebase"), "#!/bin/sh\necho 'a hook holds the index' >&2\nexit 1\n").unwrap();
        git(&slot.path, &["config", "core.hooksPath", &hooks.to_string_lossy()]).unwrap();
        assert!(git(&slot.path, &["rebase", "--quiet", &upstream]).is_err(), "the hook stops a rebase with hooks on");
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Clean, "the hook stopped the first try; the second ran without hooks");
        assert_eq!(git(&slot.path, &["log", "--format=%s", "-3"]).unwrap(), "fifth\nfourth\nsomeone else's work");
        assert!(!rebase_in_progress(&slot.path), "the rebase finished");
        git(&slot.path, &["config", "--unset", "core.hooksPath"]).unwrap();
        let _ = std::fs::remove_dir_all(hooks);
        cleanup(dir, origin, &slot);
    }

    fn kept_error(slot: &Slot) -> String {
        std::fs::read_to_string(git(&slot.path, &["rev-parse", "--path-format=absolute", "--git-path", "kebbie-rebase-error.txt"]).unwrap()).unwrap_or_default()
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
            let _ = std::fs::remove_file(crate::evidence::file(&self.state, &self.address));
            let _ = std::fs::remove_file(crate::evidence::excuse_file(&self.state, &self.address));
        }
    }

    // A green test run on the task's HEAD as it stands, as its Claude records once kebbie hands it the rebased code.
    fn tested(pushing: &Pushing, slot: &crate::tasks::Slot) {
        let code = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        crate::evidence::save(&pushing.state, &pushing.address, &crate::evidence::Run { command: "cargo test".to_string(), runner: "cargo test".to_string(), result: crate::evidence::Outcome::Passed, sha: code.clone(), code, ..Default::default() }).unwrap();
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
        let prepared = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap();
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0), "rebased, ready for the push");
        assert_eq!(prepared, Prepared::Push("the user".to_string(), Out { sha: git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebased: false, to: None }), "it goes at once, with no test run on the rebased code");
        assert!(texts.borrow().is_empty());
        std::fs::write(slot.path.join("d"), "half done").unwrap();
        let dirty = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap_err();
        assert!(dirty.contains("1 uncommitted file") && dirty.contains("files: d"), "{dirty}");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn main_moving_between_the_rebase_and_the_push_rebases_again_and_pushes_with_nothing_for_its_claude() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        shared_file(&dir, &slot, &upstream);
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        let (texts, hand) = handed();
        let sends = std::cell::Cell::new(0);
        let said = push_task_with(&pushing.state, &Caller::User, &dir, &slot, (Through::All, None, None), &hand, &mut |_, out| {
            sends.set(sends.get() + 1);
            if sends.get() == 1 {
                upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\nnine\nten\n", "another line of m");
            }
            push_prefix(&slot.path, &upstream, &out.sha, &pushing.state).map(|count| format!("pushed {count}"))
        });
        assert_eq!((said, sends.get()), (Ok(Sent::Pushed("pushed 2".to_string())), 2), "rejected, rebased over m again, pushed on the second try");
        assert!(texts.borrow().is_empty() && intent::load(&pushing.state, &pushing.address).is_none(), "nothing went to its Claude: {:?}", texts.borrow());
        let landed = git(&origin, &["log", "--format=%s", upstream.trim_start_matches("origin/")]).unwrap();
        assert!(landed.starts_with("first line of m\nfourth\nanother line of m\n"), "the task's commits on top of everything that landed: {landed}");
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
        record_approval(&pushing.state, &pushing.address, &fingerprint(&slot.path, &upstream).unwrap(), crate::tasks::ApprovedBy::Reviewer).unwrap();
        let (texts, hand) = handed();
        for caller in [Caller::Agent, Caller::Task(pushing.address.clone()), Caller::Lead] {
            let refused = prepare(&pushing.state, &caller, &dir, &slot, Through::All, None, &hand, None).unwrap_err();
            assert!(refused.contains("no agent pushes") || refused.contains("no push permission"), "{refused}");
            assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), head, "refused before the rebase");
            assert!(intent::load(&pushing.state, &pushing.address).is_none() && texts.borrow().is_empty());
        }
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_rebase_over_the_tasks_own_files_goes_out_for_who_asked_and_keeps_the_approval() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        shared_file(&dir, &slot, &upstream);
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        crate::permit::set_task_in(&pushing.state, &pushing.address, true).unwrap();
        record_approval(&pushing.state, &pushing.address, &fingerprint(&slot.path, &upstream).unwrap(), crate::tasks::ApprovedBy::Reviewer).unwrap();
        let (texts, hand) = handed();
        let prepared = prepare(&pushing.state, &Caller::Lead, &dir, &slot, Through::All, Some(7), &hand, None).unwrap();
        assert_eq!(prepared, Prepared::Push("the lead".to_string(), Out { sha: git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebased: false, to: None }), "main changed m too, and the clean rebase goes out at once");
        assert!(texts.borrow().is_empty() && intent::load(&pushing.state, &pushing.address).is_none(), "nothing went to its Claude: {:?}", texts.borrow());
        assert!(approved(&slot.path, &upstream, &pushing.address), "the clean rebase kept the approval");
        cleanup(dir, origin, &slot);
    }

    fn device_pushes<'a>(state: &'a std::path::Path, hand: &'a dyn Fn(&Slot, &str) -> std::result::Result<(), String>) -> impl FnMut(&Intent, &Slot) -> Result<Sent> + 'a {
        move |intent: &Intent, slot: &Slot| {
            let upstream = default_branch(&intent.repo)?;
            push_task_with(state, &intent.by.caller(), &intent.repo, slot, (intent.through, intent.queued, intent.to.as_deref()), hand, &mut |_, out| push_prefix(&slot.path, &upstream, &out.sha, state).map(|count| format!("pushed {count}")))
        }
    }

    #[test]
    fn a_device_push_of_a_waiting_task_is_refused_as_kebbie_push_refuses_it() {
        use crate::permit::Caller;
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        let known = super::super::after::Known { open: vec![super::super::after::Dep { address: "rounding@api".to_string(), repo: std::path::PathBuf::from("/repos/api") }], waits: Vec::new(), ends: Default::default(), running: Vec::new(), tracked: Default::default() };
        let ask = super::super::waiting::Asked { address: &pushing.address, repo: &dir, on: &["rounding@api"], then: None };
        super::super::waiting::wait_in(&pushing.state, &Caller::User, &ask, &known, 100, &mut |_, _| {}).unwrap();
        let (typed, hand) = handed();
        let pushed = device_push(&pushing.state, &Caller::hub("acc_01"), &dir, &slot, None, &|_, _| None, &|_| false, &mut device_pushes(&pushing.state, &hand));
        let said = super::super::waiting::refuse_push(&pushing.state, &pushing.address, "billing").unwrap_err();
        assert_eq!(pushed, DevicePush::Refused(said), "the phone's push gets the words kebbie push gives");
        assert!(typed.borrow().is_empty() && intent::load(&pushing.state, &pushing.address).is_none(), "nothing joined the line");
        super::super::waiting::off_in(&pushing.state, &Caller::User, &pushing.address, &mut |_, _| {}).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_device_push_goes_out_rebased_over_a_main_that_moved_since_the_phone_saw_it() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        let seen = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        upstream_commit(&dir, "elsewhere", "x", "landed after the phone showed the task");
        let (_, hand) = handed();
        let phone = Caller::hub("acc_01");
        let pushed = device_push(&pushing.state, &phone, &dir, &slot, None, &|_, _| None, &|_| false, &mut device_pushes(&pushing.state, &hand));
        let main = main_of(&upstream).to_string();
        let short = git(&origin, &["rev-parse", "--short=7", &main]).unwrap();
        assert_eq!(pushed, DevicePush::Pushed { short, main: main.clone() }, "HEAD and main both moved since the phone saw {seen}, and it is pushed all the same");
        let landed = git(&origin, &["log", "--format=%s", &main]).unwrap();
        assert!(landed.starts_with("fourth\nlanded after the phone showed the task\n"), "the task's commit on top of what landed: {landed}");
        assert!(intent::load(&pushing.state, &pushing.address).is_none());
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_device_push_of_a_run_whose_end_a_rebase_rewrote_is_refused_and_never_pushes_the_whole_branch() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        super::super::plan::list_at(&pushing.state, &slot.path, &upstream, &pushing.address, &["Model", "Command"]).unwrap();
        super::super::plan::done_at(&pushing.state, &slot.path, &upstream, &pushing.address).unwrap();
        let key = super::super::accept::open_commits(&slot.path, &upstream).unwrap()[0].key.clone();
        super::super::accept::set_in(&pushing.state, &pushing.address, &key, true).unwrap();
        commit(&slot.path, "e", "fifth, the command in progress");
        let seen = super::super::plan::listed_at(&pushing.state, &slot.path, &upstream, &pushing.address);
        let run = seen.pushable.clone().unwrap();
        assert_eq!((seen.order.len(), run.count), (2, 1));
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "rebase", "--quiet", &upstream]).unwrap();
        let main = main_of(&upstream).to_string();
        let before = git(&origin, &["rev-parse", &main]).unwrap();
        let (_, hand) = handed();
        let pushed = device_push(&pushing.state, &Caller::hub("acc_01"), &dir, &slot, Some(&run.through), &|_, _| None, &|_| false, &mut device_pushes(&pushing.state, &hand));
        assert!(matches!(&pushed, DevicePush::HeadMoved(said) if said.contains("what can go out changed")), "{pushed:?}");
        assert_eq!(git(&origin, &["rev-parse", &main]).unwrap(), before, "nothing went out");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_device_push_through_the_end_of_the_pushable_run_pushes_the_run_and_leaves_the_rest_in_the_task() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        let first = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        super::super::plan::list_at(&pushing.state, &slot.path, &upstream, &pushing.address, &["Model", "Command"]).unwrap();
        super::super::plan::done_at(&pushing.state, &slot.path, &upstream, &pushing.address).unwrap();
        commit(&slot.path, "e", "fifth");
        let second = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let (_, hand) = handed();
        let phone = Caller::hub("acc_01");
        let through = |head: &str| device_push(&pushing.state, &phone, &dir, &slot, Some(head), &|_, _| None, &|_| false, &mut device_pushes(&pushing.state, &hand));
        let moved = through(&first);
        assert!(matches!(&moved, DevicePush::HeadMoved(said) if said.contains("what can go out changed")), "the model is not accepted, so no run ends at its commit: {moved:?}");
        let key = super::super::accept::open_commits(&slot.path, &upstream).unwrap()[0].key.clone();
        super::super::accept::set_in(&pushing.state, &pushing.address, &key, true).unwrap();
        let main = main_of(&upstream).to_string();
        super::super::plan::listed_at(&pushing.state, &slot.path, &upstream, &pushing.address);
        assert_eq!(through(&first), DevicePush::Pushed { short: first[..7].to_string(), main: main.clone() });
        assert_eq!(git(&origin, &["rev-parse", &main]).unwrap(), first, "only the run went out");
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), second, "the later subtask stays in the task");
        let settled = through(&first);
        assert!(matches!(&settled, DevicePush::Settled(said) if said.contains(&format!("on {main} already"))), "{settled:?}");
        let subtasks = super::super::plan::read_now(&pushing.state, &slot.path, &upstream, &pushing.address);
        assert_eq!((subtasks.pushed.len(), subtasks.items.len(), subtasks.items[0].id.as_str()), (1, 1, "command"), "the board shows the next subtask as it stands");
        assert_eq!(subtasks.items[0].head(), Some(second.as_str()));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_device_push_that_conflicts_names_the_files_and_waits_for_its_claude_as_the_users_push() {
        use crate::permit::Caller;
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        upstream_commit(&dir, "d", "theirs", "d on main");
        let (texts, hand) = handed();
        let phone = Caller::hub("acc_01");
        let pushed = device_push(&pushing.state, &phone, &dir, &slot, None, &|_, _| None, &|_| false, &mut device_pushes(&pushing.state, &hand));
        let DevicePush::Conflict { files, .. } = &pushed else { panic!("the rebase conflicts: {pushed:?}") };
        assert_eq!(files, &["d".to_string()]);
        assert_eq!(texts.borrow().len(), 1, "its Claude got the rebase");
        let waiting = intent::load(&pushing.state, &pushing.address).unwrap();
        assert_eq!((waiting.by, waiting.work), (By::Hub { account: "acc_01".to_string(), name: crate::permit::HUB_ACCOUNT.to_string() }, Work::Conflict), "the push waits as the hub's, so its turn later names the hub account");
        let stored = std::fs::read_to_string(intent::file(&pushing.state, &pushing.address)).unwrap();
        let stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!((stored["by"].as_str(), stored["hub"]["name"].as_str()), (Some("user"), Some("your kebbie hub account")), "an older kebbie reads it as the user's push");
        let told = format!("The user pushed {} from your kebbie hub account; rebasing onto", pushing.address);
        assert!(messages::load().iter().any(|message| message.to == crate::status::LEAD_LABEL && message.text.contains(&told)), "the lead hears which device pushed");
        assert_eq!(who_pushed("the user", "billing@app"), "The user pushed billing@app");
        assert_eq!(who_pushed("the lead", "billing@app"), "The lead pushed billing@app");
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
        let Prepared::Handed(said) = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap() else { panic!("the rebase conflicts") };
        assert!(said.contains("conflicts with") && said.contains("then kebbie pushes it"), "{said}");
        assert!(Notice::read(&said, false).details.contains(&("files".to_string(), "d".to_string())), "the notice names the conflicting file: {said}");
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), head);
        let typed = texts.borrow()[0].clone();
        assert!(typed.contains("kebbie ask") && typed.contains("recommendation") && typed.contains("Do not push") && typed.contains("kebbie pushes the resolved commits"), "{typed}");
        let recorded = intent::load(&pushing.state, &pushing.address).unwrap();
        assert_eq!((recorded.by.clone(), recorded.work, recorded.fingerprint), (By::User, Work::Conflict, fingerprint(&slot.path, &upstream).unwrap()));
        let failing = |_: &Slot, _: &str| Err("the log is locked".to_string());
        let refused = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &failing, None).unwrap_err();
        assert!(refused.contains("could not pass the rebase to its Claude") && refused.contains("error: the log is locked") && Notice::read(&refused, true).details.contains(&("files".to_string(), "d".to_string())), "{refused}");
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
        deliver(&dir, &slot, &upstream, Through::First { count: 1, of: 2 }, &Out { sha: git(&slot.path, &["rev-parse", "HEAD~1"]).unwrap(), rebased: false, to: None }, "the user", &state, &crate::ci::begin).unwrap();
        let fifth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        git(&dir, &["merge", "--quiet", "--ff-only", &upstream]).unwrap();
        pick(&dir, &fifth);
        git(&dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        let repos = [dir.clone()];
        let world = World { repos: &repos, workspaces: &[], agents: &[], log: &[], gh: "false", began: 0, fresh: None };
        let told = std::cell::Cell::new(0);
        pass(&state, &world, &mut |_, _| panic!("a task kept open is never closed by the round"), &mut |_| told.set(told.get() + 1));
        assert!(branch_exists(&dir, "billing") && told.get() == 0);
        assert_eq!(crate::tasks::landed_mark(&state, &address, &slot.path), None, "no landed line asks the user to close it");
        let idle = Seen { status: Some("idle".to_string()), question: false };
        crate::tasks::landed::keep_open(&state, &address, false);
        crate::ci::forget(&state, &address);
        assert_eq!(verdict(&state, &slot, &address, &upstream, &idle, &mut |_| false).unwrap(), Verdict::Close, "it is the keep-open choice alone that holds it");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_through_a_commit_counts_its_subtask_as_pushed_and_the_rest_keep_theirs() {
        use crate::tasks::plan::{done_at, list_at, listed_at, load};
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        list_at(&state, &slot.path, &upstream, &address, &["Model", "Command", "Docs"]).unwrap();
        done_at(&state, &slot.path, &upstream, &address).unwrap();
        commit(&slot.path, "e", "fifth");
        assert!(listed_at(&state, &slot.path, &upstream, &address).pushed.is_empty());
        deliver(&dir, &slot, &upstream, Through::First { count: 1, of: 2 }, &Out { sha: git(&slot.path, &["rev-parse", "HEAD~1"]).unwrap(), rebased: false, to: None }, "the user", &state, &crate::ci::begin).unwrap();
        let shown = listed_at(&state, &slot.path, &upstream, &address);
        assert_eq!((shown.pushed.len(), shown.items[0].id.as_str(), shown.items[0].title.as_str()), (1, "command", "Command"), "one commit went out: the model");
        assert_eq!(shown.items[0].commits.iter().map(|commit| commit.subject.as_str()).collect::<Vec<_>>(), ["fifth"]);
        list_at(&state, &slot.path, &upstream, &address, &["Command, again"]).unwrap();
        assert_eq!(load(&state, &address).unwrap().parts[0].title, "Model", "the pushed subtask stays first");
        assert_eq!(listed_at(&state, &slot.path, &upstream, &address).pushed.len(), 1);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_whose_every_commit_was_pushed_kept_open_stays_open_with_no_line_and_done_still_closes_it() {
        use crate::tasks::landed::{World, pass};
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        commit(&slot.path, "e", "fifth");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        let said = deliver(&dir, &slot, &upstream, Through::First { count: 2, of: 2 }, &Out { sha: git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebased: false, to: None }, "the user", &state, &crate::ci::begin).unwrap();
        assert!(said.contains("It stays open, as you chose, with everything on main"), "{said}");
        assert!(branch_exists(&dir, "billing"));
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap().0, 0, "everything is on main");
        crate::ci::forget(&state, &address);
        let repos = [dir.clone()];
        let workspaces = [serde_json::json!({ "workspace_id": "w1", "worktree": { "checkout_path": slot.path.to_string_lossy() } })];
        let agents = [serde_json::json!({ "workspace_id": "w1", "agent_status": "idle" })];
        let world = World { repos: &repos, workspaces: &workspaces, agents: &agents, log: &[], gh: "false", began: 0, fresh: None };
        pass(&state, &world, &mut |_, _| panic!("a task kept open is never closed by the round"), &mut |_| panic!("nothing to tell"));
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
        assert!(deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head.clone(), rebased: false, to: None }, "the user", &state, watching).unwrap_err().contains("git refused the push"));
        assert_eq!(phase(), Some(Phase::Ready { head: Some(head.clone()) }), "refused, it is where it was");
        git(&dir, &["remote", "set-url", "origin", &origin.to_string_lossy()]).unwrap();
        deliver(&dir, &slot, &upstream, Through::All, &Out { sha: head.clone(), rebased: false, to: None }, "the user", &state, watching).unwrap();
        assert_eq!(phase(), Some(Phase::CiRunning { sha: head.clone() }));
        assert_eq!(git(&origin, &["rev-parse", upstream.trim_start_matches("origin/")]).unwrap(), head, "pushed");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_held_for_the_assessment_says_where_the_task_waits() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_dir().unwrap();
        let address = crate::peers::address("billing", &name_of(&dir));
        assert!(crate::assessment::ask(&state, &address, &slot.path, crate::config::ReviewAuto::Require).unwrap());
        let refused = authorize(&Caller::User, &dir, &slot, &upstream, Through::All).unwrap_err();
        assert!(refused.contains("is being assessed") && refused.contains("stays under Ready to push while the assessor reads it") && !refused.contains("In review"), "{refused}");
        crate::assessment::forget_in(&state, &address);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_repos_permission_lets_the_lead_push_a_ready_task_reviewed_or_not() {
        use crate::assessment::{Assessment, settle};
        use crate::config::ReviewAuto;
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_dir().unwrap();
        let address = crate::peers::address("billing", &name_of(&dir));
        crate::permit::set_repo_in(&state, &name_of(&dir), true).unwrap();
        record_approval(&state, &address, &fingerprint(&slot.path, &upstream).unwrap(), crate::tasks::ApprovedBy::Reviewer).unwrap();
        assert_eq!(authorize(&Caller::Lead, &dir, &slot, &upstream, Through::All), Ok("the lead".to_string()), "a review is optional under the repo's permission");
        let assess = |needed: bool| {
            crate::assessment::ask(&state, &address, &slot.path, ReviewAuto::Suggest).unwrap();
            settle(&state, &address, &slot.path, ReviewAuto::Suggest, Some(&Assessment { needed, reason: "wording".to_string() }));
        };
        assess(false);
        assert_eq!(authorize(&Caller::Lead, &dir, &slot, &upstream, Through::All), Ok("the lead".to_string()), "no review and a rules approval: the repo's permission covers it");
        commit(&slot.path, "e", "fifth");
        record_approval(&state, &address, &fingerprint(&slot.path, &upstream).unwrap(), crate::tasks::ApprovedBy::Reviewer).unwrap();
        assess(true);
        assert_eq!(authorize(&Caller::Lead, &dir, &slot, &upstream, Through::All), Ok("the lead".to_string()), "a suggested review holds nothing");
        for _ in 0..=intent::REPUSHES {
            intent::record_left(&state, &address, "it reported needs-you");
        }
        assert!(authorize(&Caller::Lead, &dir, &slot, &upstream, Through::All).unwrap_err().contains("left the push line more than 3 times"), "past the cap it is the user's");
        assert_eq!(authorize(&Caller::User, &dir, &slot, &upstream, Through::All), Ok("the user".to_string()));
        intent::forget_left(&state, &address);
        intent::forget_left_count(&state, &address);
        commit(&slot.path, "f", "sixth");
        assert!(authorize(&Caller::Lead, &dir, &slot, &upstream, Through::All).unwrap_err().contains("rules approval"), "a commit without a rules approval does");
        crate::permit::set_repo_in(&state, &name_of(&dir), false).unwrap();
        crate::assessment::forget_in(&state, &address);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn kebbie_pushes_under_either_permission_what_its_rules_approval_covers_unless_its_review_asks_for_changes() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_dir().unwrap();
        let address = crate::peers::address("billing", &name_of(&dir));
        assert!(authorize(&Caller::Keeper, &dir, &slot, &upstream, Through::All).unwrap_err().contains("no push permission"));
        crate::permit::set_repo_in(&state, &name_of(&dir), true).unwrap();
        assert!(authorize(&Caller::Keeper, &dir, &slot, &upstream, Through::All).unwrap_err().contains("no rules approval"), "a permission without a rules approval waits");
        record_approval(&state, &address, &fingerprint(&slot.path, &upstream).unwrap(), crate::tasks::ApprovedBy::Reviewer).unwrap();
        assert_eq!(authorize(&Caller::Keeper, &dir, &slot, &upstream, Through::All), Ok(KEEPER_PUSHER.to_string()), "the repo's permission and a rules approval: kebbie pushes it, review or not");
        crate::permit::set_repo_in(&state, &name_of(&dir), false).unwrap();
        crate::permit::set_task_in(&state, &address, true).unwrap();
        assert_eq!(authorize(&Caller::Keeper, &dir, &slot, &upstream, Through::All), Ok(KEEPER_PUSHER.to_string()), "the task's permission covers it as the repo's does");
        commit(&slot.path, "e", "fifth");
        assert!(authorize(&Caller::Keeper, &dir, &slot, &upstream, Through::All).is_err(), "a commit after the approval waits for the next one");
        assert_eq!(who_pushed(KEEPER_PUSHER, "billing@app"), "kebbie pushed billing@app under the user's push permission");
        crate::permit::set_task_in(&state, &address, false).unwrap();
        cleanup(dir, origin, &slot);
    }

    fn require_accept(dir: &std::path::Path, text: &str) {
        let file = crate::config::repo_file(dir);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    fn accept_commit(state: &std::path::Path, address: &str, slot: &Slot, upstream: &str, sha: &str) {
        let key = crate::tasks::open_commits(&slot.path, upstream).unwrap().into_iter().find(|open| open.sha == sha).unwrap().key;
        crate::tasks::accept::set_in(state, address, &key, true).unwrap();
    }

    #[test]
    fn a_repo_with_a_branch_per_task_pushes_it_there_unrebased_leaves_main_alone_and_adds_to_it_only_by_fast_forward() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let main = upstream.trim_start_matches("origin/").to_string();
        require_accept(&dir, "[push]\nbranch = \"feature/{task}\"\n");
        let pushing = Pushing::new(&dir);
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        let main_before = git(&origin, &["rev-parse", &main]).unwrap();
        let first = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        tested(&pushing, &slot);
        let (texts, hand) = handed();
        let unwatched = |_: &Path, _: &Path, _: &str, _: &str, _: bool, _: crate::ci::Pushed| crate::ci::Begin::Off;
        let own = |exists: bool| Some(crate::push_target::Target { branch: "feature/billing".to_string(), version: None, exists, per_task: true });
        assert_eq!(prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap(), Prepared::Push("the user".to_string(), Out { sha: first.clone(), rebased: false, to: own(false) }), "main moved, yet nothing is rebased");
        let said = deliver(&dir, &slot, &upstream, Through::All, &Out { sha: first.clone(), rebased: false, to: own(false) }, "the user", &pushing.state, &unwatched).unwrap();
        assert!(said.contains("to feature/billing") && said.contains("On branch, waiting for"), "{said}");
        assert_eq!(git(&origin, &["rev-parse", "feature/billing"]).unwrap(), first, "the task's commits are on its branch");
        assert_eq!(git(&origin, &["rev-parse", &main]).unwrap(), main_before, "main is untouched");
        assert_eq!(crate::phase::load(&pushing.state, &pushing.address).map(|record| record.phase), Some(crate::phase::Phase::OnBranch { sha: first.clone(), branch: "feature/billing".to_string() }));
        assert!(find_task(&dir, Some("billing"), &dir).is_ok(), "the task stays open");
        assert!(texts.borrow().is_empty());
        let again = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap_err();
        assert!(again.contains("Nothing new to push: feature/billing on origin already holds it"), "{again}");
        let second = commit(&slot.path, "e", "more work");
        tested(&pushing, &slot);
        assert_eq!(prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap(), Prepared::Push("the user".to_string(), Out { sha: second.clone(), rebased: false, to: own(true) }));
        let said = deliver(&dir, &slot, &upstream, Through::All, &Out { sha: second.clone(), rebased: false, to: own(true) }, "the user", &pushing.state, &unwatched).unwrap();
        assert!(said.starts_with("Pushed 1 commit to feature/billing") || said.contains("Pushed 1 commit to feature/billing"), "only the new commit goes: {said}");
        assert_eq!(git(&origin, &["rev-parse", "feature/billing"]).unwrap(), second, "a fast-forward of the branch");
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--amend", "-m", "more work, reworded"]).unwrap();
        tested(&pushing, &slot);
        let rewritten = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap_err();
        assert!(rewritten.contains("feature/billing on origin has commits this task's branch does not hold"), "never a force push: {rewritten}");
        assert_eq!(git(&origin, &["rev-parse", "feature/billing"]).unwrap(), second);
        crate::phase::forget(&pushing.state, &pushing.address);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_goes_to_a_branch_the_repo_offers_and_to_main_only_when_the_user_picks_it() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let main = upstream.trim_start_matches("origin/").to_string();
        require_accept(&dir, "[push]\nbranch = \"release/{next}\"\nchoices = [\"feature/{task}\"]\n[push.version]\ncommand = \"echo 3.5\"\n");
        let target = |chosen: Option<&str>| push_target(&dir, &upstream, "billing", chosen);
        assert_eq!(target(None).unwrap().map(|to| to.branch), Some("release/3.5".to_string()), "the repo's rule by default");
        assert_eq!(target(Some("feature/{task}")).unwrap().map(|to| (to.branch, to.per_task)), Some(("feature/billing".to_string(), true)));
        assert_eq!(target(Some(crate::push_target::DEFAULT_BRANCH)).unwrap(), None, "the default branch, picked");
        let unoffered = target(Some("hotfix/{task}")).unwrap_err();
        assert!(unoffered.contains("the repo offers no branch \"hotfix/{task}\"") && unoffered.contains("release/{next}, feature/{task}"), "{unoffered}");
        let shown: Vec<String> = crate::push_target::offers(&dir, &upstream, &crate::push_target::rule(&dir).unwrap().unwrap(), "billing").into_iter().map(|offer| offer.shown).collect();
        assert_eq!(shown, ["release/3.5 · new: kebbie creates it from ".to_string() + &main, "feature/billing".to_string(), format!("{main} · the default branch, only because you pick it")]);
        let pushing = Pushing::new(&dir);
        tested(&pushing, &slot);
        let lead = join_on(&pushing.state, &Caller::Lead, &dir, &slot, Through::All, None, Some(crate::push_target::DEFAULT_BRANCH)).unwrap_err();
        assert!(lead.contains(&format!("only you push this task to {main}")), "{lead}");
        join_on(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, Some("feature/{task}")).unwrap();
        assert_eq!(intent::load(&pushing.state, &pushing.address).and_then(|waiting| waiting.to).as_deref(), Some("feature/{task}"), "the push line keeps what the user picked");
        let (_, hand) = handed();
        let picked = intent::load(&pushing.state, &pushing.address).and_then(|waiting| waiting.to);
        for _ in 0..2 {
            let Prepared::Push(_, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, picked.as_deref()).unwrap() else { panic!("pushed") };
            assert_eq!(out.to.map(|to| to.branch), Some("feature/billing".to_string()), "prepare follows the pick, not the rule, also when a rejected push tries again");
        }
        intent::forget(&pushing.state, &pushing.address);
        join_on(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, Some(crate::push_target::DEFAULT_BRANCH)).unwrap();
        let Prepared::Push(_, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, Some(crate::push_target::DEFAULT_BRANCH)).unwrap() else { panic!("pushed") };
        assert_eq!(out.to, None, "picked, it goes to the default branch");
        intent::forget(&pushing.state, &pushing.address);
        crate::phase::forget(&pushing.state, &pushing.address);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_tasks_next_push_to_a_shared_branch_adds_only_its_new_commits() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let main = upstream.trim_start_matches("origin/").to_string();
        require_accept(&dir, "[push]\nbranch = \"release/{next}\"\n[push.version]\ncommand = \"echo 3.5\"\n");
        let unwatched = |_: &Path, _: &Path, _: &str, _: &str, _: bool, _: crate::ci::Pushed| crate::ci::Begin::Off;
        let (_, hand) = handed();
        let pushing = Pushing::new(&dir);
        commit(&slot.path, "d", "d, again");
        tested(&pushing, &slot);
        let Prepared::Push(pusher, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap() else { panic!("pushed") };
        deliver(&dir, &slot, &upstream, Through::All, &out, &pusher, &pushing.state, &unwatched).unwrap();
        let again = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap_err();
        assert!(again.contains("Nothing new to push: release/3.5 on origin already holds it"), "two commits on one file, both out: {again}");
        commit(&slot.path, "d", "d, a third time");
        tested(&pushing, &slot);
        let Prepared::Push(pusher, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap() else { panic!("pushed") };
        let said = deliver(&dir, &slot, &upstream, Through::All, &out, &pusher, &pushing.state, &unwatched).unwrap();
        assert!(said.contains("Pushed 1 commit to release/3.5"), "only the new commit goes: {said}");
        let log: Vec<String> = git(&origin, &["log", "--reverse", "--format=%s", &format!("{main}..release/3.5")]).unwrap().lines().map(str::to_string).collect();
        assert_eq!(log, ["fourth", "d, again", "d, a third time"]);
        assert_eq!(git(&origin, &["show", "release/3.5:d"]).unwrap(), "d, a third time");
        crate::phase::forget(&pushing.state, &pushing.address);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_missing_release_branch_is_created_from_main_with_its_version_bump_first_and_two_tasks_land_on_it_in_order() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let main = upstream.trim_start_matches("origin/").to_string();
        upstream_commit(&dir, "package.json", "{\"version\": \"3.4.3\"}\n", "Ship 3.4.3");
        let unwatched = |_: &Path, _: &Path, _: &str, _: &str, _: bool, _: crate::ci::Pushed| crate::ci::Begin::Off;
        let (texts, hand) = handed();
        let pushing = Pushing::new(&dir);
        tested(&pushing, &slot);
        require_accept(&dir, "[push]\nbranch = \"release/{next}\"\nprepare = \"echo cannot >&2; exit 4\"\n[push.version]\nfile = \"package.json\"\n");
        let failed = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap_err();
        assert!(failed.contains("push.prepare failed, so kebbie did not create release/3.5") && failed.contains("exited 4: cannot"), "{failed}");
        assert!(git(&origin, &["rev-parse", "--verify", "--quiet", "release/3.5"]).is_err(), "a failed preparation creates no branch");
        require_accept(&dir, "[push]\nbranch = \"release/{next}\"\nprepare = \"printf '{\\\"version\\\": \\\"%s\\\"}\\\\n' \\\"$KEBBIE_VERSION\\\" > package.json\"\n[push.version]\nfile = \"package.json\"\n");
        let main_before = git(&origin, &["rev-parse", &main]).unwrap();
        let Prepared::Push(pusher, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap() else { panic!("pushed") };
        assert_eq!(out.to.as_ref().map(|to| (to.branch.as_str(), to.exists)), Some(("release/3.5", false)), "3.4.3 on main counts up to 3.5");
        let said = deliver(&dir, &slot, &upstream, Through::All, &out, &pusher, &pushing.state, &unwatched).unwrap();
        assert!(said.contains("Pushed 1 commit to release/3.5"), "{said}");
        crate::phase::forget(&pushing.state, &pushing.address);
        drop(pushing);
        let other = another_task(&dir, "invoices", "e");
        let address = crate::peers::address("invoices", &name_of(&dir));
        let state = crate::lead::state_dir().unwrap();
        let head = git(&other.path, &["rev-parse", "HEAD"]).unwrap();
        crate::evidence::save(&state, &address, &crate::evidence::Run { command: "cargo test".to_string(), runner: "cargo test".to_string(), result: crate::evidence::Outcome::Passed, sha: head.clone(), code: head, ..Default::default() }).unwrap();
        let Prepared::Push(pusher, out) = prepare(&state, &Caller::User, &dir, &other, Through::All, None, &hand, None).unwrap() else { panic!("pushed") };
        assert_eq!(out.to.as_ref().map(|to| (to.branch.as_str(), to.exists)), Some(("release/3.5", true)), "main still says 3.4.3, so the second task goes to the same branch");
        deliver(&dir, &other, &upstream, Through::All, &out, &pusher, &state, &unwatched).unwrap();
        let log: Vec<String> = git(&origin, &["log", "--reverse", "--format=%s", &format!("{main}..release/3.5")]).unwrap().lines().map(str::to_string).collect();
        assert_eq!(log, ["Prepare release/3.5", "fourth", "invoices"], "the bump first, then the tasks in the order they went out");
        assert_eq!(git(&origin, &["show", "release/3.5:package.json"]).unwrap(), "{\"version\": \"3.5\"}");
        assert_eq!(git(&origin, &["rev-parse", &main]).unwrap(), main_before, "main is untouched");
        assert!(texts.borrow().is_empty());
        crate::phase::forget(&state, &address);
        intent::forget(&state, &address);
        let _ = std::fs::remove_file(crate::evidence::file(&state, &address));
        git(&dir, &["worktree", "remove", "--force", &other.path.to_string_lossy()]).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn tasks_sharing_a_release_branch_go_out_rebased_onto_it_in_line_order_and_leave_main_and_their_own_branches_alone() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let main = upstream.trim_start_matches("origin/").to_string();
        require_accept(&dir, "[push]\nbranch = \"release/{next}\"\n");
        git(&dir, &["push", "--quiet", "origin", "HEAD:refs/heads/release/3.4"]).unwrap();
        let unwatched = |_: &Path, _: &Path, _: &str, _: &str, _: bool, _: crate::ci::Pushed| crate::ci::Begin::Off;
        let (texts, hand) = handed();
        let pushing = Pushing::new(&dir);
        tested(&pushing, &slot);
        commit(&dir, "version", "3.5");
        git(&dir, &["push", "--quiet", "origin", "HEAD:refs/heads/release/3.5"]).unwrap();
        git(&dir, &["reset", "--quiet", "--hard", "HEAD~1"]).unwrap();
        let main_before = git(&origin, &["rev-parse", &main]).unwrap();
        let own = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let Prepared::Push(pusher, out) = prepare(&pushing.state, &Caller::User, &dir, &slot, Through::All, None, &hand, None).unwrap() else { panic!("pushed") };
        assert_eq!((out.rebased, out.to.as_ref().map(|to| (to.branch.as_str(), to.exists, to.per_task))), (true, Some(("release/3.5", true, false))), "the open release branch is next");
        let said = deliver(&dir, &slot, &upstream, Through::All, &out, &pusher, &pushing.state, &unwatched).unwrap();
        assert!(said.contains("Pushed 1 commit to release/3.5"), "{said}");
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), own, "the task's own branch is as it was");
        assert_eq!(crate::phase::load(&pushing.state, &pushing.address).map(|record| record.phase), Some(crate::phase::Phase::OnBranch { sha: own.clone(), branch: "release/3.5".to_string() }));
        crate::phase::forget(&pushing.state, &pushing.address);
        drop(pushing);
        let other = another_task(&dir, "invoices", "e");
        let address = crate::peers::address("invoices", &name_of(&dir));
        let state = crate::lead::state_dir().unwrap();
        let head = git(&other.path, &["rev-parse", "HEAD"]).unwrap();
        crate::evidence::save(&state, &address, &crate::evidence::Run { command: "cargo test".to_string(), runner: "cargo test".to_string(), result: crate::evidence::Outcome::Passed, sha: head.clone(), code: head.clone(), ..Default::default() }).unwrap();
        let Prepared::Push(pusher, out) = prepare(&state, &Caller::User, &dir, &other, Through::All, None, &hand, None).unwrap() else { panic!("pushed") };
        deliver(&dir, &other, &upstream, Through::All, &out, &pusher, &state, &unwatched).unwrap();
        let log: Vec<String> = git(&origin, &["log", "--reverse", "--format=%s", &format!("{main}..release/3.5")]).unwrap().lines().map(str::to_string).collect();
        assert_eq!(log, ["3.5", "fourth", "invoices"], "the branch's own commit first, then each task in the order it went out");
        assert_eq!(git(&origin, &["rev-parse", &main]).unwrap(), main_before, "main is untouched");
        assert_eq!(git(&other.path, &["rev-parse", "HEAD"]).unwrap(), head);
        assert!(texts.borrow().is_empty(), "nothing was handed to a Claude");
        let again = prepare(&state, &Caller::User, &dir, &other, Through::All, None, &hand, None).unwrap_err();
        assert!(again.contains("Nothing new to push: release/3.5 on origin already holds it"), "{again}");
        assert!(!crate::tasks::landed::landed(&slot.path, &upstream).unwrap());
        git(&dir, &["push", "--quiet", "origin", &format!("refs/remotes/origin/release/3.5:refs/heads/{main}")]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        assert!(crate::tasks::landed::landed(&slot.path, &upstream).unwrap() && crate::tasks::landed::landed(&other.path, &upstream).unwrap(), "once the release branch reaches main, both tasks have landed though their own shas never went out");
        crate::phase::forget(&state, &address);
        intent::forget(&state, &address);
        let _ = std::fs::remove_file(crate::evidence::file(&state, &address));
        git(&dir, &["worktree", "remove", "--force", &other.path.to_string_lossy()]).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn with_accepting_required_a_push_waits_for_every_commit_it_takes_and_goes_once_they_are_accepted() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let fifth = commit(&slot.path, "e", "fifth");
        let state = state_dir().unwrap();
        let address = crate::peers::address("billing", &name_of(&dir));
        assert!(authorize(&Caller::User, &dir, &slot, &upstream, Through::All).is_ok(), "accepting is optional by default");
        require_accept(&dir, "[task]\nsteps = false\nrequire_accept = true\n");
        assert!(!crate::tasks::accept_required(&dir) && authorize(&Caller::User, &dir, &slot, &upstream, Through::All).is_ok(), "with task.steps off it has no effect");
        require_accept(&dir, "[task]\nrequire_accept = true\n");
        assert!(crate::tasks::accept_required(&dir));
        let refused = authorize(&Caller::User, &dir, &slot, &upstream, Through::All).unwrap_err();
        let notice = crate::notice::Notice::parse(&refused).unwrap();
        assert_eq!(notice.headline, format!("Not pushed: commit {} is not accepted", &fourth[..7]), "{refused}");
        assert!(refused.contains(&format!("kebbie accept billing {}", &fourth[..7])), "it says how to accept it: {refused}");
        accept_commit(&state, &address, &slot, &upstream, &fourth);
        assert!(authorize(&Caller::User, &dir, &slot, &upstream, Through::First { count: 1, of: 2 }).is_ok(), "push --through an accepted commit goes");
        let refused = authorize(&Caller::User, &dir, &slot, &upstream, Through::All).unwrap_err();
        assert!(refused.contains(&format!("commit {} is not accepted", &fifth[..7])), "{refused}");
        accept_commit(&state, &address, &slot, &upstream, &fifth);
        assert!(authorize(&Caller::User, &dir, &slot, &upstream, Through::All).is_ok(), "every commit accepted, it goes");
        crate::tasks::accept::forget("billing", &dir);
        std::fs::remove_file(crate::config::repo_file(&dir)).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn push_in_order_stops_at_a_task_with_a_commit_not_accepted() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        require_accept(&dir, "[task]\nrequire_accept = true\n");
        let mut tried = Vec::new();
        let order = [("billing".to_string(), slot.path.clone()), ("pricing".to_string(), dir.join("elsewhere"))];
        let outcome = crate::next::push_in_order(&order, |path| {
            tried.push(path.to_path_buf());
            authorize(&Caller::User, &dir, &crate::tasks::find_task(&dir, None, path)?, &upstream, Through::All)
        }).unwrap_err();
        let notice = crate::notice::Notice::parse(&outcome).unwrap();
        assert_eq!(notice.headline, "billing did not push, so the rest wait");
        assert!(outcome.contains("is not accepted") && outcome.contains("pricing"), "{outcome}");
        assert_eq!(tried, std::slice::from_ref(&slot.path), "the rest are not tried");
        std::fs::remove_file(crate::config::repo_file(&dir)).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_working_task_whose_accepted_subtasks_are_pushed_keeps_working_and_a_ready_one_stays_ready() {
        use crate::phase::{Event, Phase};
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        commit(&slot.path, "e", "fifth");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        let phase = || crate::phase::load(&state, &address).map(|record| record.phase);
        let first = |slot: &Slot| Out { sha: git(&slot.path, &["rev-parse", "HEAD~1"]).unwrap(), rebased: false, to: None };
        crate::phase::record(&state, &address, "report", &Event::Reported { state: "working", head: None, committed: true, pushing: false });
        crate::phase::record(&state, &address, "user", &Event::PushQueued { ahead: 1 });
        crate::phase::record(&state, &address, "push", &Event::PushQueued { ahead: 0 });
        deliver(&dir, &slot, &upstream, Through::First { count: 1, of: 2 }, &first(&slot), "the user", &state, &crate::ci::begin).unwrap();
        assert_eq!(phase(), Some(Phase::Working), "it goes on with the subtasks after the pushed one");
        commit(&slot.path, "f", "sixth");
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        crate::phase::record(&state, &address, "report", &Event::Reported { state: "ready", head: Some(&head), committed: true, pushing: false });
        crate::phase::record(&state, &address, "user", &Event::PushQueued { ahead: 0 });
        deliver(&dir, &slot, &upstream, Through::First { count: 1, of: 2 }, &first(&slot), "the user", &state, &crate::ci::begin).unwrap();
        assert_eq!(phase(), Some(Phase::Ready { head: Some(head) }), "a ready task keeps the rest ready to push");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_working_task_whose_every_commit_is_pushed_keeps_working_while_a_listed_subtask_is_left() {
        use crate::phase::{Event, Phase};
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        let phase = || crate::phase::load(&state, &address).map(|record| record.phase);
        let all = |slot: &Slot| Out { sha: git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), rebased: false, to: None };
        crate::tasks::plan::list_at(&state, &slot.path, &upstream, &address, &["Model", "Command"]).unwrap();
        crate::phase::record(&state, &address, "report", &Event::Reported { state: "working", head: None, committed: true, pushing: false });
        crate::phase::record(&state, &address, "user", &Event::PushQueued { ahead: 0 });
        crate::tasks::plan::listed_at(&state, &slot.path, &upstream, &address);
        deliver(&dir, &slot, &upstream, Through::First { count: 1, of: 1 }, &all(&slot), "the user", &state, &crate::ci::begin).unwrap();
        assert_eq!(phase(), Some(Phase::Working), "the second subtask is still to come");
        crate::tasks::plan::list_at(&state, &slot.path, &upstream, &address, &["Model"]).unwrap();
        commit(&slot.path, "e", "fifth");
        let ahead = git(&slot.path, &["rev-list", "--count", &format!("{upstream}..HEAD")]).unwrap().parse().unwrap();
        crate::phase::record(&state, &address, "user", &Event::PushQueued { ahead: 0 });
        crate::tasks::plan::listed_at(&state, &slot.path, &upstream, &address);
        deliver(&dir, &slot, &upstream, Through::First { count: ahead, of: ahead }, &all(&slot), "the user", &state, &crate::ci::begin).unwrap();
        assert_eq!(phase(), Some(Phase::Idle), "with no subtask left it waits for work");
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
