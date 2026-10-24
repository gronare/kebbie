use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::board::widgets::plural;
use crate::config;
use crate::lead::state_dir;
use crate::messages;
use crate::notice::{Notice, Tone};
use crate::peers;
use crate::permit::Caller;
use crate::phase::{self, Event};
use crate::repo::{ahead_behind, default_branch, git, git_writing, main_checkout, name_of, rebase_in_progress, uncommitted};
use crate::sh::Result;
use super::intent::{self, By, Through, Work};
use super::line::Turnstile;
use super::pool::{Slot, find_task, task_of};
use super::push::{HandOver, authorize_as, committed, conflict_prompt, hand_over, leave_line};

// A task on local main: its address and the patch-ids of the commits kebbie picked for it, which survive main being rebased.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub address: String,
    pub task: String,
    pub patches: Vec<String>,
}

pub(super) type Rebuild<'a> = &'a dyn Fn(&Path) -> Option<String>;

fn dir(state: &Path) -> PathBuf {
    state.join("staged")
}

fn file(state: &Path, repo: &Path) -> PathBuf {
    dir(state).join(format!("{}.json", name_of(repo)))
}

fn read(path: &Path) -> Vec<Entry> {
    let value: Value = fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    value.as_array().into_iter().flatten().filter_map(|entry| Some(Entry {
        address: entry["address"].as_str()?.to_string(),
        task: entry["task"].as_str()?.to_string(),
        patches: entry["patches"].as_array()?.iter().filter_map(|patch| patch.as_str().map(str::to_string)).collect(),
    })).collect()
}

fn write(path: &Path, entries: &[Entry]) -> Result<()> {
    if entries.is_empty() {
        let _ = fs::remove_file(path);
        return Ok(());
    }
    let listed: Vec<Value> = entries.iter().map(|entry| json!({ "address": entry.address, "task": entry.task, "patches": entry.patches })).collect();
    crate::state::write(path, Value::Array(listed).to_string())
}

// The repo's staged tasks, in the order they went on main.
pub(crate) fn load(state: &Path, repo: &Path) -> Vec<Entry> {
    read(&file(state, repo))
}

pub(crate) fn staged(state: &Path, repo: &Path, address: &str) -> bool {
    load(state, repo).iter().any(|entry| entry.address == address)
}

pub(crate) fn forget(state: &Path, address: &str) {
    let _ = fs::remove_file(pending_file(state, address));
    let Ok(files) = fs::read_dir(dir(state)) else { return };
    for path in files.flatten().map(|entry| entry.path()) {
        let entries = read(&path);
        if entries.iter().any(|entry| entry.address == address) {
            let _ = write(&path, &entries.into_iter().filter(|entry| entry.address != address).collect::<Vec<_>>());
        }
    }
}

// A stage that conflicted waits here for its Claude's ready report, then goes on main again for whoever asked.
#[derive(Clone, Debug, PartialEq)]
struct Pending {
    repo: PathBuf,
    task: String,
    by: By,
    at: u64,
    waits: Option<u64>,
}

// How long a re-stage waits on what blocks it, a dirty or busy main checkout, before it leaves the task for the user.
pub(crate) const WAIT_CEILING: u64 = 30 * 60;

fn pending_file(state: &Path, address: &str) -> PathBuf {
    state.join("staging").join(format!("{address}.json"))
}

fn save_pending(state: &Path, address: &str, pending: &Pending) -> Result<()> {
    crate::state::write(&pending_file(state, address), json!({ "repo": pending.repo, "task": pending.task, "by": pending.by.key(), "at": pending.at, "waits": pending.waits }).to_string())
}

fn all_pending(state: &Path) -> Vec<(String, Pending)> {
    let Ok(files) = fs::read_dir(state.join("staging")) else { return Vec::new() };
    files.flatten().filter_map(|entry| {
        let address = entry.file_name().to_string_lossy().strip_suffix(".json")?.to_string();
        let value: Value = serde_json::from_str(&fs::read_to_string(entry.path()).ok()?).ok()?;
        let by = if value["by"] == "lead" { By::Lead } else { By::User };
        Some((address, Pending { repo: PathBuf::from(value["repo"].as_str()?), task: value["task"].as_str()?.to_string(), by, at: value["at"].as_u64()?, waits: value["waits"].as_u64() }))
    }).collect()
}

// A task stays staged only while one of its copies is still on local main ahead of origin; pushed or reset away, it is staged no more.
fn prune(state: &Path, repo: &Path) -> Vec<String> {
    let entries = load(state, repo);
    if entries.is_empty() {
        return Vec::new();
    }
    let Ok(upstream) = default_branch(repo) else { return Vec::new() };
    let Ok(ahead) = commits(repo, &format!("{upstream}..HEAD")) else { return Vec::new() };
    let mut on_main = Vec::new();
    for sha in ahead {
        match patch_id(repo, &sha) {
            Ok(patch) => on_main.extend(patch),
            Err(_) => return Vec::new(),
        }
    }
    let (kept, gone): (Vec<Entry>, Vec<Entry>) = entries.into_iter().partition(|entry| entry.patches.iter().any(|patch| on_main.contains(patch)));
    if gone.is_empty() || write(&file(state, repo), &kept).is_err() {
        return Vec::new();
    }
    gone.iter().map(|entry| {
        let head = find_task(repo, Some(&entry.task), repo).ok().and_then(|slot| git(&slot.path, &["rev-parse", "HEAD"]).ok());
        phase::record(state, &entry.address, "stage", &Event::Unstaged { head: head.as_deref() });
        format!("stage: {} is no longer on local main ahead of origin", entry.address)
    }).collect()
}

// Taken off main before a drop or reset forgets it, so its copies never stay behind as commits nobody staged.
pub(super) fn take_off_before_discard(state: &Path, caller: &Caller, repo: &Path, slot: &Slot) -> Result<Option<String>> {
    let address = peers::address(task_of(slot), &name_of(repo));
    prune(state, repo);
    if !staged(state, repo, &address) {
        return Ok(None);
    }
    unstage_in(state, caller, repo, slot, &|repo| crate::update::rebuild_staged(repo, false)).map(Some).map_err(|error| {
        let why = Notice::parse(&error).map_or(error.clone(), |notice| notice.headline);
        Notice::new(Tone::You, format!("Not dropped: it is on local main, and kebbie could not take it off ({why})")).you(format!("Run kebbie unstage {} first.", task_of(slot))).detail("task", task_of(slot)).to_string()
    })
}

// A repo whose turnstile another holds is pruned next round, so a phase never flips during someone's replay.
fn prune_all(state: &Path, repos: &[PathBuf]) -> Vec<String> {
    repos.iter().filter_map(|repo| Turnstile::try_take(state, repo).map(|_turnstile| prune(state, repo))).flatten().collect()
}

pub(crate) fn tick(state: &Path) -> Vec<String> {
    let mut notes = prune_all(state, &crate::repo::pooled_checkouts());
    let opted = |repo: &Path| config::resolve(Some(repo)).is_ok_and(|loaded| loaded.settings.stage_on_main.value);
    notes.extend(restage_in(state, &messages::load(), messages::now(), &opted, &hand_over, &|repo| crate::update::rebuild_staged(repo, false)));
    notes
}

// A ready report after the hand-over puts the task on main again; needs-you, or the task gone, drops the wait.
fn restage_in(state: &Path, log: &[messages::Message], now: u64, opted: &dyn Fn(&Path) -> bool, hand_over: HandOver, rebuild: Rebuild) -> Vec<String> {
    let reports = messages::latest_reports(log);
    let mut notes = Vec::new();
    for (address, pending) in all_pending(state) {
        let said = reports.get(&address).filter(|report| report.at > pending.at).and_then(|report| report.report.as_ref()).map(|report| report.state.clone());
        let slot = find_task(&pending.repo, Some(&pending.task), &pending.repo);
        match (said.as_deref(), slot) {
            (_, Err(_)) | (Some("needs-you"), _) => {
                let _ = fs::remove_file(pending_file(state, &address));
                notes.push(format!("stage: {address} no longer waits to go on local main"));
            }
            (Some("ready"), Ok(slot)) if !rebase_in_progress(&slot.path) => {
                let outcome = stage_in(state, &pending.by.caller(), &pending.repo, &slot, opted(&pending.repo), hand_over, rebuild);
                let blocked = outcome.as_ref().err().and_then(|error| Notice::parse(error)).filter(|notice| matches!(notice.tone, Tone::NotNow | Tone::You));
                let waits = blocked.is_some();
                if let Some(blocked) = &blocked {
                    let why = blocked.you.clone().map_or(blocked.headline.clone(), |you| format!("{} {you}", blocked.headline));
                    match pending.waits {
                        None => {
                            let _ = save_pending(state, &address, &Pending { waits: Some(now), ..pending.clone() });
                            tell_user(&format!("kebbie waits to put {address} on local main again, its conflict resolved: {why}. It goes on by itself once that is settled, for up to {} minutes.", WAIT_CEILING / 60));
                            notes.push(format!("stage: {address} waits: {why}"));
                        }
                        Some(since) if now.saturating_sub(since) >= WAIT_CEILING => {
                            let _ = fs::remove_file(pending_file(state, &address));
                            phase::record(state, &address, "stage", &Event::StageGaveUp);
                            tell_user(&format!("kebbie gave up putting {address} on local main again after {} minutes: {why}. Stage it yourself once that is settled.", WAIT_CEILING / 60));
                            notes.push(format!("stage: {address} gave up: {why}"));
                        }
                        Some(_) => {}
                    }
                }
                let handed_again = all_pending(state).iter().any(|(again, newer)| *again == address && newer.at != pending.at);
                if !waits && !handed_again {
                    let _ = fs::remove_file(pending_file(state, &address));
                }
                if let (Err(error), false) = (&outcome, waits) {
                    tell_lead(&format!("kebbie could not put {address} on local main again after its Claude resolved the conflict: {error}"));
                }
                if !waits {
                    notes.push(format!("stage: {address}: {}", outcome.unwrap_or_else(|error| error)));
                }
            }
            _ => {}
        }
    }
    notes
}

// The commits on local main that are neither on origin nor a staged task's: the user's own, which kebbie never rewrites.
fn foreign(repo: &Path, upstream: &str, entries: &[Entry]) -> Result<Vec<String>> {
    let mut own = Vec::new();
    for sha in commits(repo, &format!("{upstream}..HEAD"))? {
        let patch = patch_id(repo, &sha)?;
        if !patch.is_some_and(|patch| entries.iter().any(|entry| entry.patches.contains(&patch))) {
            own.push(sha[..7.min(sha.len())].to_string());
        }
    }
    Ok(own)
}

fn foreign_note(local: &str, own: &[String]) -> String {
    format!("local {local} has your own commits {}; push {local} with them, or take the staged tasks off with kebbie unstage first, then stage again", own.join(", "))
}

// With tasks staged, main is rebased onto what went out, unless it holds commits of the user's own; then it is left alone and the user told.
pub(super) fn follow_staged(repo: &Path, upstream: &str, entries: &[Entry]) -> Result<String> {
    let local = local_of(upstream);
    let name = name_of(repo);
    let own = foreign(repo, upstream, entries)?;
    if !own.is_empty() {
        let note = foreign_note(local, &own);
        tell_lead(&format!("{name}: kebbie left local {local} as it is after a push: {note}."));
        return Ok(format!("{name} left as is: {note}"));
    }
    if !uncommitted(repo)?.is_empty() {
        return Ok(format!("{name} keeps what is staged on {local}, not rebased onto {upstream}: it has uncommitted changes"));
    }
    if git_writing(repo, &["rebase", "--quiet", upstream]).is_ok() {
        return Ok(format!("{name} rebased what is staged on {local} onto {upstream}"));
    }
    let _ = git_writing(repo, &["rebase", "--abort"]);
    Ok(format!("{name} keeps what is staged on {local}, not rebased onto {upstream}: it conflicts"))
}

// None for a merge or a commit that changes nothing: neither has a patch to match.
fn patch_id(path: &Path, sha: &str) -> Result<Option<String>> {
    let shown = crate::sh::run_untrimmed("git", &["show", "--no-color", "--no-ext-diff", "--no-merges", sha], Some(path))?;
    if shown.trim().is_empty() {
        return Ok(None);
    }
    let mut child = Command::new("git").args(["patch-id", "--stable"]).current_dir(path).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|error| format!("git patch-id: {error}"))?;
    child.stdin.take().ok_or("git patch-id: no input")?.write_all(shown.as_bytes()).map_err(|error| format!("git patch-id: {error}"))?;
    let output = child.wait_with_output().map_err(|error| format!("git patch-id: {error}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).split_whitespace().next().map(str::to_string))
}

fn commits(path: &Path, range: &str) -> Result<Vec<String>> {
    Ok(git(path, &["rev-list", "--reverse", range])?.lines().map(str::to_string).collect())
}

fn local_of(upstream: &str) -> &str {
    upstream.trim_start_matches("origin/")
}

#[derive(Debug, PartialEq)]
enum Stuck {
    Kept,
    Added,
}

// Main loses the commits whose patch is in `drop` and gains `add` on top; anything that does not apply leaves main exactly as it was.
fn replay(repo: &Path, upstream: &str, drop: &[String], add: &[String]) -> Result<std::result::Result<(), Stuck>> {
    let before = git(repo, &["rev-parse", "HEAD"])?;
    let own = commits(repo, &format!("{upstream}..HEAD"))?;
    let mut dropped = Vec::new();
    for sha in &own {
        if patch_id(repo, sha)?.is_some_and(|patch| drop.contains(&patch)) {
            dropped.push(sha.clone());
        }
    }
    let (base, kept) = match own.iter().position(|sha| dropped.contains(sha)) {
        Some(first) => (git(repo, &["rev-parse", &format!("{}^", own[first])])?, own[first..].iter().filter(|sha| !dropped.contains(sha)).cloned().collect()),
        None => (before.clone(), Vec::new()),
    };
    if base != before {
        git_writing(repo, &["reset", "--quiet", "--keep", &base])?;
    }
    for (shas, stuck) in [(kept, Stuck::Kept), (add.to_vec(), Stuck::Added)] {
        if shas.is_empty() {
            continue;
        }
        let mut args = vec!["cherry-pick"];
        args.extend(shas.iter().map(String::as_str));
        if git_writing(repo, &args).is_err() {
            let _ = git_writing(repo, &["cherry-pick", "--abort"]);
            if let Err(error) = git_writing(repo, &["reset", "--quiet", "--keep", &before]) {
                let at = git(repo, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
                let short = &before[..7.min(before.len())];
                return Notice::new(Tone::Failed, format!("{} is left at {at}, part-way: kebbie could not put it back to {short}", name_of(repo)))
                    .you(format!("A file in the main checkout changed meanwhile. Save that change, then run git reset --keep {short} in {} to put main back as it was.", repo.display())).detail("error", error).err();
            }
            return Ok(Err(stuck));
        }
    }
    Ok(Ok(()))
}

// The main checkout must be on the local default branch with nothing of the user's uncommitted, so a rewrite never touches their work.
fn checkout_ready(repo: &Path, local: &str, refused: &str) -> Result<()> {
    let name = name_of(repo);
    if git(repo, &["branch", "--show-current"])? != local {
        return Notice::new(Tone::NotNow, format!("{refused}: {name} is not on {local}")).you(format!("Switch the main checkout back to {local} first.")).detail("checkout", repo.display().to_string()).err();
    }
    let dirty = uncommitted(repo)?;
    if !dirty.is_empty() {
        return Notice::new(Tone::You, format!("{refused}: {name} has {}", plural(dirty.len(), "uncommitted file"))).you("Commit or stash them in the main checkout first.").detail("files", dirty.join(" · ")).err();
    }
    let busy = ["CHERRY_PICK_HEAD", "MERGE_HEAD", "REVERT_HEAD"].iter().any(|head| git(repo, &["rev-parse", "--verify", "--quiet", head]).is_ok());
    if busy || rebase_in_progress(repo) {
        return Notice::new(Tone::NotNow, format!("{refused}: a rebase, merge or cherry-pick is under way in {name}")).you("Finish or abort it first.").err();
    }
    if !git(repo, &["rev-list", "--merges", &format!("{}..HEAD", default_branch(repo)?)])?.is_empty() {
        return Notice::new(Tone::NotNow, format!("{refused}: {local} has a merge on it, which kebbie does not rewrite")).you(format!("Push or reset {local} yourself first.")).err();
    }
    Ok(())
}

// announce prints a line before a build starts, for a terminal; the board passes false, as a line there would tear its screen.
pub fn stage(cwd: &Path, name: Option<&str>, announce: bool) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let opted = config::resolve(Some(&repo))?.settings.stage_on_main.value;
    stage_in(&state_dir()?, &crate::permit::caller(), &repo, &slot, opted, &hand_over, &|repo| crate::update::rebuild_staged(repo, announce))
}

pub fn unstage(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    unstage_in(&state_dir()?, &crate::permit::caller(), &repo, &slot, &|repo| crate::update::rebuild_staged(repo, false))
}

fn opt_in(repo: &Path) -> String {
    format!("Set worktree.stage_on_main = true in {} to turn it on for this repo.", config::repo_file_in(&config::repos_dir(), repo).display())
}

pub(super) fn stage_in(state: &Path, caller: &Caller, repo: &Path, slot: &Slot, opted: bool, hand_over: HandOver, rebuild: Rebuild) -> Result<String> {
    let task = task_of(slot).to_string();
    let address = peers::address(&task, &name_of(repo));
    let refused = "Not put on local main";
    if !opted {
        return Notice::new(Tone::NotNow, format!("{refused}: {} does not stage tasks", name_of(repo))).you(opt_in(repo)).detail("task", &task).err();
    }
    let upstream = default_branch(repo)?;
    let local = local_of(&upstream).to_string();
    if ahead_behind(&slot.path, &upstream)?.0 == 0 {
        return Notice::new(Tone::NotNow, format!("{refused}: it has no commits")).detail("task", &task).err();
    }
    committed(&slot.path, &task)?;
    if rebase_in_progress(&slot.path) {
        return Notice::new(Tone::NotNow, format!("{refused}: a rebase is under way in its worktree")).next("Try again once the rebase is finished.").detail("task", &task).err();
    }
    let pusher = authorize_as(caller, repo, slot, &upstream, Through::All, refused)?;
    let Some(turnstile) = Turnstile::try_take(state, repo) else {
        return Notice::new(Tone::NotNow, format!("{refused}: kebbie is pushing in {} now", name_of(repo))).next("Try again in a moment.").detail("task", &task).err();
    };
    if let Some(waiting) = intent::load(state, &address) {
        if waiting.work == Work::Ci {
            return Notice::new(Tone::NotNow, format!("{refused}: its commits are on {local} already")).you("Fix the red CI, or dismiss it.").detail("task", &task).err();
        }
        if waiting.by == By::User && *caller != Caller::User {
            return Notice::new(Tone::Failed, format!("{refused}: the user asked for its push")).you("Ask the user; only they take their push back.").detail("task", &task).err();
        }
    }
    checkout_ready(repo, &local, refused)?;
    super::fetch(repo)?;
    let mut entries = load(state, repo);
    let own = foreign(repo, &upstream, &entries)?;
    if !own.is_empty() {
        return Notice::new(Tone::You, format!("{refused}: {}", foreign_note(&local, &own))).detail("task", &task).err();
    }
    let follow = super::follow_upstream(repo, &upstream, &entries)?;
    let earlier = entries.iter().find(|entry| entry.address == address).map(|entry| entry.patches.clone()).unwrap_or_default();
    let mut on_main = Vec::new();
    for sha in commits(repo, &format!("{upstream}..HEAD"))? {
        on_main.extend(patch_id(repo, &sha)?);
    }
    let mut add = Vec::new();
    let mut patches = Vec::new();
    for sha in commits(&slot.path, &format!("{upstream}..HEAD"))? {
        let Some(patch) = patch_id(&slot.path, &sha)? else { continue };
        if !on_main.contains(&patch) || earlier.contains(&patch) {
            add.push(sha);
            patches.push(patch);
        }
    }
    if add.is_empty() {
        return Notice::new(Tone::NotNow, format!("{refused}: its commits are there already")).detail("task", &task).err();
    }
    let others: Vec<String> = entries.iter().filter(|entry| entry.address != address).map(|entry| entry.task.clone()).collect();
    match replay(repo, &upstream, &earlier, &add)? {
        Ok(()) => {}
        Err(Stuck::Kept) => {
            return Notice::new(Tone::Failed, format!("{refused} again: the tasks staged after it build on its earlier commits")).you("Take those off local main first.").detail("task", &task).detail("staged", others.join(" · ")).err();
        }
        Err(Stuck::Added) => {
            let why = format!("The user asked kebbie to put this task's commits on the local {local} branch of the main checkout, to try them before pushing, on top of {}", if others.is_empty() { "what is there".to_string() } else { super::and_list(&others) });
            let prompt = format!("{} When you report ready, kebbie puts it on local {local} again.", conflict_prompt(&why, &local));
            let by = By::of(caller).unwrap_or(By::User);
            if let Err(cannot) = hand_over(slot, &prompt).and_then(|_| save_pending(state, &address, &Pending { repo: repo.to_path_buf(), task: task.clone(), by, at: messages::now(), waits: None })) {
                return Notice::new(Tone::Failed, format!("{refused}: it conflicts with local {local}, and kebbie could not pass the rebase to its Claude")).you(format!("Rebase it onto {local} yourself in its worktree.")).detail("task", &task).detail("error", cannot).err();
            }
            tell_lead(&format!("{} asked to put {address} on local {local}; it conflicts with what is staged there, so its Claude rebases it onto {local}. {local} is as it was.", capitalized(pusher)));
            return Notice::new(Tone::Moving, format!("{refused}: it conflicts with local {local}")).next(format!("Its Claude rebases it onto {local}; when it reports ready, kebbie puts it on {local} again. {local} is as it was.")).detail("task", &task).ok();
        }
    }
    for sha in commits(repo, &format!("{upstream}..HEAD"))?.iter().rev().take(add.len()) {
        if let Some(patch) = patch_id(repo, sha)?.filter(|patch| !patches.contains(patch)) {
            patches.push(patch);
        }
    }
    if intent::load(state, &address).is_some() {
        leave_line(state, repo, slot, &address);
    }
    entries.retain(|entry| entry.address != address);
    entries.push(Entry { address: address.clone(), task: task.clone(), patches });
    write(&file(state, repo), &entries)?;
    let head = git(&slot.path, &["rev-parse", "HEAD"]).ok();
    phase::record(state, &address, "stage", &Event::Staged { head: head.as_deref() });
    tell_lead(&format!("{} put {address} on local {local} to try it, unpushed; it stays open until main is pushed.", capitalized(pusher)));
    let order: Vec<&str> = entries.iter().map(|entry| entry.task.as_str()).collect();
    let mut notice = Notice::new(Tone::Done, format!("On local {local}: {}", plural(add.len(), "commit"))).next(format!("Try it from the main checkout. Push {local} when it is right, and kebbie closes it once its commits are on origin; or take it off with kebbie unstage {task}."))
        .detail("task", &task).detail("staged", order.join(" · ")).detail("checkout", follow);
    drop(turnstile);
    if let Some(built) = rebuild(repo) {
        notice = notice.detail("build", built);
    }
    notice.ok()
}

pub(super) fn unstage_in(state: &Path, caller: &Caller, repo: &Path, slot: &Slot, rebuild: Rebuild) -> Result<String> {
    let task = task_of(slot).to_string();
    let address = peers::address(&task, &name_of(repo));
    let refused = "Not taken off local main";
    may_unstage(caller, &address, repo).map_err(|why| Notice::new(Tone::Failed, format!("{refused}: {} may not change it", crate::permit::describe(caller))).detail("task", &task).detail("why", why).to_string())?;
    let mut entries = load(state, repo);
    let Some(entry) = entries.iter().find(|entry| entry.address == address).cloned() else {
        return Notice::new(Tone::NotNow, format!("{refused}: it is not on local main")).detail("task", &task).err();
    };
    let upstream = default_branch(repo)?;
    let local = local_of(&upstream).to_string();
    let held = Turnstile::held_here(state, repo);
    let turnstile = if held { None } else { Turnstile::try_take(state, repo) };
    if !held && turnstile.is_none() {
        return Notice::new(Tone::NotNow, format!("{refused}: kebbie is pushing in {} now", name_of(repo))).next("Try again in a moment.").detail("task", &task).err();
    }
    checkout_ready(repo, &local, refused)?;
    let own = foreign(repo, &upstream, &entries)?;
    if !own.is_empty() {
        return Notice::new(Tone::You, format!("{refused}: {}", foreign_note(&local, &own))).detail("task", &task).err();
    }
    let mut present = false;
    for sha in commits(repo, &format!("{upstream}..HEAD"))? {
        present |= patch_id(repo, &sha)?.is_some_and(|patch| entry.patches.contains(&patch));
    }
    let after: Vec<String> = entries.iter().skip_while(|staged| staged.address != address).skip(1).map(|staged| staged.task.clone()).collect();
    if present && replay(repo, &upstream, &entry.patches, &[])? == Err(Stuck::Kept) {
        return Notice::new(Tone::Failed, format!("{refused}: {} build on it", super::and_list(&after))).you("Take those off first, then this one.").detail("task", &task).err();
    }
    entries.retain(|staged| staged.address != address);
    write(&file(state, repo), &entries)?;
    let head = git(&slot.path, &["rev-parse", "HEAD"]).ok();
    phase::record(state, &address, "stage", &Event::Unstaged { head: head.as_deref() });
    tell_lead(&format!("{address} was taken off local {local}; its commits stay on its branch, unpushed."));
    let mut notice = Notice::new(Tone::Done, format!("Taken off local {local}")).next("Its commits stay on its branch; it waits under Ready to push.").detail("task", &task);
    if !present {
        notice = notice.detail("main", format!("its commits were no longer on local {local}"));
    }
    if !entries.is_empty() {
        notice = notice.detail("staged", entries.iter().map(|staged| staged.task.as_str()).collect::<Vec<_>>().join(" · "));
    }
    drop(turnstile);
    if present && let Some(built) = rebuild(repo) {
        notice = notice.detail("build", built);
    }
    notice.ok()
}

// Taking a task off main pushes nothing, so the lead needs only the permission it would push the task with.
fn may_unstage(caller: &Caller, address: &str, repo: &Path) -> std::result::Result<(), String> {
    match caller {
        Caller::User => Ok(()),
        Caller::Lead => crate::permit::lead_may_push(crate::permit::of(address, &name_of(repo)), true),
        _ => Err("only the user, or the lead for a task the user gave push permission".to_string()),
    }
}

// A staged task goes out with main, never through the push line.
pub(super) fn refuse_push(state: &Path, repo: &Path, address: &str, task: &str) -> Result<()> {
    prune(state, repo);
    if !staged(state, repo, address) {
        return Ok(());
    }
    let local = default_branch(repo).map(|upstream| local_of(&upstream).to_string()).unwrap_or_else(|_| "main".to_string());
    Notice::new(Tone::NotNow, format!("Not pushed: it is on local {local}; push {local}")).next(format!("Push {local} from the main checkout or the Areas page, and kebbie closes it once its commits are on origin; or take it off with kebbie unstage {task}.")).detail("task", task).err()
}

// The lead's line shows on the board's messages too, so what it says reaches the user there.
fn tell_user(text: &str) {
    if let Err(error) = messages::tell_lead(text, Some("push")) {
        eprintln!("Could not tell the lead: {error}");
    }
}

fn tell_lead(text: &str) {
    if let Err(error) = messages::log_for_lead(text, Some("push")) {
        eprintln!("Could not tell the lead: {error}");
    }
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map(|first| first.to_uppercase().chain(characters).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::phase::Phase;
    use crate::tasks::landed::{World, pass};
    use crate::tasks::test_support::*;

    struct Staging {
        dir: PathBuf,
        origin: PathBuf,
        state: PathBuf,
        upstream: String,
        alpha: Slot,
        beta: Slot,
    }

    impl Drop for Staging {
        fn drop(&mut self) {
            let _ = git(&self.dir, &["worktree", "remove", "--force", &self.beta.path.to_string_lossy()]);
            let _ = std::fs::remove_dir_all(self.dir.with_extension("state"));
            let _ = git(&self.dir, &["worktree", "remove", "--force", &self.alpha.path.to_string_lossy()]);
            let _ = std::fs::remove_dir_all(&self.dir);
            let _ = std::fs::remove_dir_all(&self.origin);
        }
    }

    fn staging() -> Staging {
        let (dir, origin, alpha, upstream) = task_with_a_commit("alpha");
        let beta = another_task(&dir, "beta", "e");
        let state = state_of(&dir);
        Staging { dir, origin, state, upstream, alpha, beta }
    }

    fn no_hand_over(_: &Slot, text: &str) -> std::result::Result<(), String> {
        panic!("nothing conflicts here: {text}")
    }

    fn built(_: &Path) -> Option<String> {
        Some("rebuilt".to_string())
    }

    fn stage_as_user(staging: &Staging, slot: &Slot) -> Result<String> {
        stage_in(&staging.state, &Caller::User, &staging.dir, slot, true, &no_hand_over, &built)
    }

    fn on_main(staging: &Staging) -> Vec<String> {
        git(&staging.dir, &["log", "--reverse", "--format=%s", &format!("{}..HEAD", staging.upstream)]).unwrap().lines().map(str::to_string).collect()
    }

    fn address(staging: &Staging, task: &str) -> String {
        peers::address(task, &name_of(&staging.dir))
    }

    fn phase_of(staging: &Staging, task: &str) -> Option<Phase> {
        phase::load(&staging.state, &address(staging, task)).map(|record| record.phase)
    }

    #[test]
    fn two_tasks_go_on_local_main_in_the_order_staged_and_nothing_is_pushed() {
        let staging = staging();
        let origin_before = git(&staging.dir, &["rev-parse", &staging.upstream]).unwrap();
        let first = stage_as_user(&staging, &staging.alpha).unwrap();
        assert!(first.contains("On local") && first.contains("rebuilt"), "{first}");
        stage_as_user(&staging, &staging.beta).unwrap();
        assert_eq!(on_main(&staging), ["fourth", "beta"], "alpha's commit, then beta's, on top of origin's");
        assert_eq!(git(&staging.origin, &["rev-parse", staging.upstream.trim_start_matches("origin/")]).unwrap(), origin_before, "origin never moves");
        assert_eq!(load(&staging.state, &staging.dir).iter().map(|entry| entry.task.as_str()).collect::<Vec<_>>(), ["alpha", "beta"]);
        assert!(staged(&staging.state, &staging.dir, &address(&staging, "alpha")));
        assert!(matches!(phase_of(&staging, "alpha"), Some(Phase::Staged { .. })), "{:?}", phase_of(&staging, "alpha"));
        assert!(crate::repo::branch_exists(&staging.dir, "alpha") && find_task(&staging.dir, Some("alpha"), &staging.dir).is_ok(), "the task stays open");
        assert_eq!(ahead_behind(&staging.alpha.path, &staging.upstream).unwrap().0, 1, "its branch still measures against origin");
    }

    #[test]
    fn a_conflict_on_the_second_goes_to_its_claude_and_leaves_main_as_it_was() {
        let staging = staging();
        let gamma = another_task(&staging.dir, "gamma", "d");
        stage_as_user(&staging, &staging.alpha).unwrap();
        let main_before = git(&staging.dir, &["rev-parse", "HEAD"]).unwrap();
        let typed = RefCell::new(Vec::new());
        let hand = |_: &Slot, text: &str| { typed.borrow_mut().push(text.to_string()); Ok(()) };
        let said = stage_in(&staging.state, &Caller::User, &staging.dir, &gamma, true, &hand, &|_| panic!("nothing to rebuild")).unwrap();
        assert!(said.contains("it conflicts with local"), "{said}");
        assert_eq!(git(&staging.dir, &["rev-parse", "HEAD"]).unwrap(), main_before, "main is exactly as it was");
        assert!(crate::repo::uncommitted(&staging.dir).unwrap().is_empty() && !Path::new(&staging.dir).join(".git/CHERRY_PICK_HEAD").exists());
        assert_eq!(typed.borrow().len(), 1);
        assert!(typed.borrow()[0].contains("git rebase") && typed.borrow()[0].contains("on top of alpha") && typed.borrow()[0].contains("Do not push"), "{}", typed.borrow()[0]);
        assert!(!staged(&staging.state, &staging.dir, &address(&staging, "gamma")));
        git(&staging.dir, &["worktree", "remove", "--force", &gamma.path.to_string_lossy()]).unwrap();
    }

    #[test]
    fn unstaging_the_first_drops_exactly_its_commits_and_keeps_the_second() {
        let staging = staging();
        stage_as_user(&staging, &staging.alpha).unwrap();
        stage_as_user(&staging, &staging.beta).unwrap();
        let said = unstage_in(&staging.state, &Caller::User, &staging.dir, &staging.alpha, &built).unwrap();
        assert!(said.contains("Taken off local") && said.contains("rebuilt"), "{said}");
        assert_eq!(on_main(&staging), ["beta"]);
        assert!(!staging.dir.join("d").exists() && staging.dir.join("e").exists(), "alpha's file left the main checkout, beta's stayed");
        assert_eq!(git(&staging.alpha.path, &["log", "-1", "--format=%s"]).unwrap(), "fourth", "its commits stay on its branch");
        assert_eq!(load(&staging.state, &staging.dir).iter().map(|entry| entry.task.as_str()).collect::<Vec<_>>(), ["beta"]);
        assert!(matches!(phase_of(&staging, "alpha"), Some(Phase::Ready { .. })));
        assert!(unstage_in(&staging.state, &Caller::User, &staging.dir, &staging.alpha, &built).unwrap_err().contains("not on local main"));
        assert!(unstage_in(&staging.state, &Caller::Task(address(&staging, "beta")), &staging.dir, &staging.beta, &built).unwrap_err().contains("may not change it"), "a task's Claude never takes itself off");
    }

    #[test]
    fn pushing_main_takes_everything_staged_out_and_landed_closes_each_task() {
        let staging = staging();
        stage_as_user(&staging, &staging.alpha).unwrap();
        stage_as_user(&staging, &staging.beta).unwrap();
        git(&staging.dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
        git(&staging.dir, &["fetch", "--quiet", "origin"]).unwrap();
        let repos = [staging.dir.clone()];
        let slots = [&staging.alpha, &staging.beta];
        let workspaces: Vec<Value> = slots.iter().enumerate().map(|(at, slot)| json!({ "workspace_id": format!("w{at}"), "worktree": { "checkout_path": slot.path.to_string_lossy() } })).collect();
        let agents: Vec<Value> = (0..slots.len()).map(|at| json!({ "workspace_id": format!("w{at}"), "agent_status": "idle" })).collect();
        let world = World { repos: &repos, workspaces: &workspaces, agents: &agents, log: &[], gh: "false", began: messages::now(), fresh: None };
        let (state, upstream) = (staging.state.clone(), staging.upstream.clone());
        let mut close = |repo: &Path, task: &str, _| {
            let slot = find_task(repo, Some(task), repo).map_err(crate::tasks::landed::Refused::Other)?;
            super::super::finish::finish(repo, &slot, &upstream, &state, None).map_err(crate::tasks::landed::Refused::Other)
        };
        let notes = pass(&staging.state, &world, &mut close, &mut |_| {});
        assert!(!crate::repo::branch_exists(&staging.dir, "alpha") && !crate::repo::branch_exists(&staging.dir, "beta"), "both closed as landed: {notes:?}");
        assert!(load(&staging.state, &staging.dir).is_empty(), "closing forgets what was staged");
    }

    #[test]
    fn a_staged_task_is_refused_the_push_line_and_main_follows_what_others_push() {
        let staging = staging();
        stage_as_user(&staging, &staging.alpha).unwrap();
        let refused = super::super::push::join(&staging.state, &Caller::User, &staging.dir, &staging.alpha, Through::All).unwrap_err();
        assert!(refused.contains("it is on local") && refused.contains("; push "), "{refused}");
        assert!(intent::load(&staging.state, &address(&staging, "alpha")).is_none(), "it never joins the line");
        let branch = staging.upstream.trim_start_matches("origin/");
        git(&staging.beta.path, &["push", "--quiet", "origin", &format!("HEAD:{branch}")]).unwrap();
        git(&staging.dir, &["fetch", "--quiet", "origin"]).unwrap();
        let followed = super::super::follow_upstream(&staging.dir, &staging.upstream, &load(&staging.state, &staging.dir)).unwrap();
        assert!(followed.contains("rebased what is staged"), "{followed}");
        assert_eq!(on_main(&staging), ["fourth"], "what beta pushed is under it, alpha's commit on top");
        assert!(staging.dir.join("e").exists());
        assert!(!super::super::landed::landed(&staging.alpha.path, &staging.upstream).unwrap(), "local main is not origin: alpha has not landed");
        let address = address(&staging, "alpha");
        assert!(staged(&staging.state, &staging.dir, &address), "{:?}", load(&staging.state, &staging.dir));
        assert!(unstage_in(&staging.state, &Caller::User, &staging.dir, &staging.alpha, &built).is_ok(), "found by patch after main was rebased");
        assert!(on_main(&staging).is_empty());
    }

    #[test]
    fn a_conflicted_stage_goes_on_main_by_itself_once_its_claude_reports_ready() {
        let staging = staging();
        let gamma = another_task(&staging.dir, "gamma", "d");
        stage_as_user(&staging, &staging.alpha).unwrap();
        stage_in(&staging.state, &Caller::User, &staging.dir, &gamma, true, &|_, _| Ok(()), &built).unwrap();
        assert_eq!(on_main(&staging), ["fourth"]);
        let opted = |_: &Path| true;
        assert!(restage_in(&staging.state, &[], messages::now(), &opted, &no_hand_over, &built).is_empty(), "no report yet: it waits");
        assert!(git(&gamma.path, &["rebase", "--quiet", &git(&staging.dir, &["branch", "--show-current"]).unwrap()]).is_err(), "its Claude meets the conflict");
        std::fs::write(gamma.path.join("d"), "d and gamma").unwrap();
        git(&gamma.path, &["add", "d"]).unwrap();
        git(&gamma.path, &["-c", "core.editor=true", "rebase", "--continue"]).unwrap();
        let mut ready = crate::tasks::intent::tests::report("r1", messages::now() + 1, "ready");
        ready.from = address(&staging, "gamma");
        std::fs::write(staging.dir.join("a"), "edited in the main checkout").unwrap();
        let now = messages::now();
        let told = restage_in(&staging.state, &[ready.clone()], now, &opted, &no_hand_over, &built);
        assert!(told.len() == 1 && told[0].contains("waits") && told[0].contains("uncommitted"), "the main checkout is dirty: the user hears why, once: {told:?}");
        assert!(restage_in(&staging.state, &[ready.clone()], now + 60, &opted, &no_hand_over, &built).is_empty(), "and not again each round");
        assert_eq!(all_pending(&staging.state).len(), 1, "and keeps waiting");
        git(&staging.dir, &["checkout", "--", "a"]).unwrap();
        let notes = restage_in(&staging.state, &[ready.clone()], now + 120, &opted, &no_hand_over, &built);
        assert!(notes.len() == 1 && notes[0].contains("On local"), "{notes:?}");
        assert_eq!(on_main(&staging), ["fourth", "gamma"], "alpha's copy is not picked twice");
        assert!(staged(&staging.state, &staging.dir, &address(&staging, "gamma")));
        assert!(restage_in(&staging.state, &[ready], now + 180, &opted, &no_hand_over, &built).is_empty(), "it goes on once");
        git(&staging.dir, &["worktree", "remove", "--force", &gamma.path.to_string_lossy()]).unwrap();
    }

    #[test]
    fn commits_of_the_users_own_on_local_main_are_never_rewritten() {
        let staging = staging();
        stage_as_user(&staging, &staging.alpha).unwrap();
        commit(&staging.dir, "mine", "my own fix");
        let mine = git(&staging.dir, &["rev-parse", "--short=7", "HEAD"]).unwrap();
        let branch = staging.upstream.trim_start_matches("origin/");
        git(&staging.beta.path, &["push", "--quiet", "origin", &format!("HEAD:{branch}")]).unwrap();
        git(&staging.dir, &["fetch", "--quiet", "origin"]).unwrap();
        let before = git(&staging.dir, &["rev-parse", "HEAD"]).unwrap();
        let followed = super::super::follow_upstream(&staging.dir, &staging.upstream, &load(&staging.state, &staging.dir)).unwrap();
        assert!(followed.contains(&format!("has your own commits {mine}; push ")), "{followed}");
        assert_eq!(git(&staging.dir, &["rev-parse", "HEAD"]).unwrap(), before, "main is left alone");
        let delta = another_task(&staging.dir, "delta", "f");
        let refused = stage_as_user(&staging, &delta).unwrap_err();
        assert!(refused.contains(&format!("your own commits {mine}")), "{refused}");
        git(&staging.dir, &["worktree", "remove", "--force", &delta.path.to_string_lossy()]).unwrap();
        assert!(unstage_in(&staging.state, &Caller::User, &staging.dir, &staging.alpha, &built).unwrap_err().contains("your own commits"));
        assert_eq!(git(&staging.dir, &["rev-parse", "HEAD"]).unwrap(), before);
    }

    #[test]
    fn a_merge_or_an_empty_commit_on_main_is_the_users_own_and_main_is_left_alone() {
        let staging = staging();
        stage_as_user(&staging, &staging.alpha).unwrap();
        git(&staging.dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--allow-empty", "-m", "a note to self"]).unwrap();
        let entries = load(&staging.state, &staging.dir);
        assert_eq!(foreign(&staging.dir, &staging.upstream, &entries).unwrap().len(), 1, "an empty commit has no patch, so it is the user's");
        git(&staging.dir, &["switch", "--quiet", "--create", "side", "HEAD~1"]).unwrap();
        commit(&staging.dir, "side", "on a side branch");
        let local = staging.upstream.trim_start_matches("origin/");
        git(&staging.dir, &["switch", "--quiet", local]).unwrap();
        git(&staging.dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "merge", "--quiet", "--no-ff", "--no-edit", "side"]).unwrap();
        git(&staging.beta.path, &["push", "--quiet", "origin", &format!("HEAD:{local}")]).unwrap();
        git(&staging.dir, &["fetch", "--quiet", "origin"]).unwrap();
        let before = git(&staging.dir, &["rev-parse", "HEAD"]).unwrap();
        let followed = super::super::follow_upstream(&staging.dir, &staging.upstream, &load(&staging.state, &staging.dir)).unwrap();
        assert!(followed.contains("has your own commits"), "{followed}");
        assert_eq!(git(&staging.dir, &["rev-parse", "HEAD"]).unwrap(), before);
        assert!(super::super::follow_upstream(&std::env::temp_dir().join("kebbie-no-such-checkout"), &staging.upstream, &entries).unwrap().contains("left as is"), "housekeeping after a push never fails it");
    }

    #[test]
    fn a_staged_task_whose_branch_moved_on_after_main_was_pushed_is_staged_no_more() {
        let staging = staging();
        stage_as_user(&staging, &staging.alpha).unwrap();
        commit(&staging.alpha.path, "later", "more work after staging");
        git(&staging.dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
        git(&staging.dir, &["fetch", "--quiet", "origin"]).unwrap();
        assert!(staged(&staging.state, &staging.dir, &address(&staging, "alpha")));
        assert!(refuse_push(&staging.state, &staging.dir, &address(&staging, "alpha"), "alpha").is_ok(), "its staged copies are on origin, so the push line takes it again");
        assert!(!staged(&staging.state, &staging.dir, &address(&staging, "alpha")));
        assert!(matches!(phase_of(&staging, "alpha"), Some(Phase::Ready { .. })));
    }

    #[test]
    fn a_drop_or_reset_takes_a_staged_task_off_main_before_it_forgets_it() {
        let staging = staging();
        stage_as_user(&staging, &staging.alpha).unwrap();
        stage_as_user(&staging, &staging.beta).unwrap();
        assert!(take_off_before_discard(&staging.state, &Caller::User, &staging.dir, &staging.alpha).unwrap().is_some());
        assert_eq!(on_main(&staging), ["beta"], "no copy of it is left behind as commits nobody staged");
        std::fs::write(staging.dir.join("a"), "edited").unwrap();
        let refused = take_off_before_discard(&staging.state, &Caller::User, &staging.dir, &staging.beta).unwrap_err();
        assert!(refused.contains("kebbie unstage beta first"), "{refused}");
        assert_eq!(take_off_before_discard(&staging.state, &Caller::User, &staging.dir, &staging.alpha).unwrap(), None, "an unstaged task has nothing to take off");
        let source = include_str!("finish.rs");
        let discard = &source[source.find("\nfn discard(").unwrap()..];
        assert!(discard.find("take_off_before_discard(").unwrap() < discard.find("release(").unwrap(), "taken off before its worktree goes");
    }

    #[test]
    fn move_refuses_while_tasks_are_staged_and_the_turnstile_knows_its_own_thread() {
        let staging = staging();
        let state = crate::lead::state_dir().unwrap();
        stage_in(&state, &Caller::User, &staging.dir, &staging.alpha, true, &no_hand_over, &built).unwrap();
        let refused = super::super::move_work(&staging.dir, "newer", &[], None, false).unwrap_err();
        assert!(refused.contains("alpha is staged") && refused.contains("kebbie unstage"), "{refused}");
        forget(&state, &address(&staging, "alpha"));
        let held = Turnstile::try_take(&state, &staging.dir).unwrap();
        assert!(Turnstile::held_here(&state, &staging.dir) && Turnstile::try_take(&state, &staging.dir).is_none());
        drop(held);
        assert!(!Turnstile::held_here(&state, &staging.dir));
    }

    #[test]
    fn a_re_stage_blocked_for_thirty_minutes_gives_up_under_needs_you() {
        let staging = staging();
        let gamma = another_task(&staging.dir, "gamma", "d");
        stage_as_user(&staging, &staging.alpha).unwrap();
        stage_in(&staging.state, &Caller::User, &staging.dir, &gamma, true, &|_, _| Ok(()), &built).unwrap();
        let mut ready = crate::tasks::intent::tests::report("r1", messages::now() + 1, "ready");
        ready.from = address(&staging, "gamma");
        std::fs::write(staging.dir.join("a"), "edited in the main checkout").unwrap();
        let opted = |_: &Path| true;
        let now = messages::now();
        restage_in(&staging.state, &[ready.clone()], now, &opted, &no_hand_over, &built);
        assert!(restage_in(&staging.state, &[ready.clone()], now + WAIT_CEILING - 1, &opted, &no_hand_over, &built).is_empty());
        let gave_up = restage_in(&staging.state, &[ready], now + WAIT_CEILING, &opted, &no_hand_over, &built);
        assert!(gave_up.len() == 1 && gave_up[0].contains("gave up"), "{gave_up:?}");
        assert!(all_pending(&staging.state).is_empty());
        assert_eq!(phase_of(&staging, "gamma"), Some(Phase::NeedsYou(crate::phase::Why::NotStaged)));
        git(&staging.dir, &["checkout", "--", "a"]).unwrap();
        git(&staging.dir, &["worktree", "remove", "--force", &gamma.path.to_string_lossy()]).unwrap();
    }

    #[test]
    fn the_keepers_prune_waits_while_another_holds_the_repos_turnstile() {
        let staging = staging();
        stage_as_user(&staging, &staging.alpha).unwrap();
        commit(&staging.alpha.path, "later", "more work after staging");
        git(&staging.dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
        git(&staging.dir, &["fetch", "--quiet", "origin"]).unwrap();
        let repos = [staging.dir.clone()];
        let held = Turnstile::try_take(&staging.state, &staging.dir).unwrap();
        assert!(prune_all(&staging.state, &repos).is_empty());
        assert!(staged(&staging.state, &staging.dir, &address(&staging, "alpha")), "untouched while someone else replays main");
        drop(held);
        assert_eq!(prune_all(&staging.state, &repos).len(), 1);
        assert!(!staged(&staging.state, &staging.dir, &address(&staging, "alpha")));
    }

    #[test]
    fn a_repo_without_stage_on_main_refuses_and_says_how_to_turn_it_on() {
        let staging = staging();
        let refused = stage_in(&staging.state, &Caller::User, &staging.dir, &staging.alpha, false, &no_hand_over, &built).unwrap_err();
        assert!(refused.contains("does not stage tasks") && refused.contains("worktree.stage_on_main = true"), "{refused}");
        assert!(on_main(&staging).is_empty());
        let lead = stage_in(&staging.state, &Caller::Lead, &staging.dir, &staging.alpha, true, &no_hand_over, &built).unwrap_err();
        assert!(lead.contains("Not put on local main") && lead.contains("may not push it"), "the lead stages only with the permission it pushes with: {lead}");
        assert!(stage_in(&staging.state, &Caller::Task(address(&staging, "alpha")), &staging.dir, &staging.alpha, true, &no_hand_over, &built).is_err());
        assert!(on_main(&staging).is_empty());
    }
}
