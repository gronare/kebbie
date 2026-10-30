use std::path::Path;

use crate::herdr;
use crate::lead::state_dir;
use crate::messages;
use crate::notice::{Notice, Tone};
use crate::peers;
use crate::repo::{ahead_behind, changed_count, default_branch, git, git_writing, is_clean, main_checkout, name_of};
use crate::board::widgets::plural;
use crate::sh::Result;
use super::approval::forget_approval;
use super::launch::forget_session;
use super::{fast_forward_main, fetch};
use super::pool::{Slot, find_task, release, task_of};
use super::started::{NoteBack, forget_start, forget_start_in, note_to_backlog};

// Stops a task for good: its work is discarded, its worktree and Space go, and the backlog item it came from returns.
pub fn reset(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot).to_string();
    let upstream = default_branch(&repo)?;
    let discarded = git(&slot.path, &["log", "--format=%h %s", &format!("{upstream}..HEAD")]).unwrap_or_default();
    let items = forget_start(&task, &repo).map(|text| crate::task_note::items(&text)).unwrap_or_default();
    let state = state_dir()?;
    let address = peers::address(&task, &name_of(&repo));
    let vault = items.iter().any(|item| crate::vault_cli::is_note(item)).then(crate::vault_cli::detect).flatten();
    let dropped = discard(&repo, Some(&task), true, "reset", None)?;
    let mut shown = Notice::new(Tone::Done, "Reset: its work is discarded").next(if items.is_empty() { "Its worktree is free for the next task.".to_string() } else { "Its backlog item is back in the Backlog column.".to_string() });
    shown.details = dropped.details;
    let mut notice = format!("The user reset {task}: it is gone, and its work was discarded and will never be pushed.");
    if !discarded.is_empty() {
        notice.push_str(&format!(" Discarded: {}.", discarded.lines().collect::<Vec<_>>().join("; ")));
    }
    for item in &items {
        match vault.as_ref().filter(|_| crate::vault_cli::is_note(item)).map(|cli| note_to_backlog(cli, &state, &task, &address, item)) {
            Some(NoteBack::Shared) => shown = shown.detail("note", format!("{item} stays open for the task's other halves")),
            Some(NoteBack::Elsewhere(claim)) => shown = shown.detail("note", crate::task_note::held_text(item, &claim, crate::backlog::today())),
            Some(NoteBack::Gone) => shown = shown.detail("note", crate::task_note::gone_text(item)),
            Some(NoteBack::Back(path)) => notice.push_str(&format!(" kebbie put its note {path} back in the backlog.")),
            Some(NoteBack::Failed(error)) => {
                shown = shown.detail("note", format!("{item} could not go back to the backlog ({error}); set its status to backlog in the vault"));
                notice.push_str(&crate::backlog::reset_notice(item));
            }
            None => notice.push_str(&crate::backlog::reset_notice(item)),
        }
    }
    notice.push_str(" Drop any review or recommendation about it.");
    if let Err(error) = messages::to_lead(messages::ToLead::TaskEnded, &notice) {
        eprintln!("Could not tell the lead: {error}");
    }
    shown.ok()
}

// Last step of done and drop: closing the Space ends the task's Claude, which may be the one running this.
fn close_space(workspace: Option<String>) -> Option<String> {
    match workspace.map(|workspace| herdr::close_workspace(&workspace)) {
        Some(Err(error)) => Some(format!("could not be closed: {error}")),
        _ => None,
    }
}

pub fn done(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let task = task_of(&find_task(&repo, name, cwd)?).to_string();
    let state = state_dir()?;
    let address = peers::address(&task, &name_of(&repo));
    let vault = crate::task_note::linked(&state, &address).and_then(|_| crate::vault_cli::detect());
    let said = note_ready(vault.as_ref(), &state, &address, &task)?;
    close_task(cwd, name, vault.as_ref(), said, None).map(|closed| closed.to_string())
}

// Done waits for the task's note to have its solution and decisions: Err is the refusal, the task told what to write; a vault that does not answer lets done go on, and Ok says so as a detail.
pub(super) fn note_ready(vault: Option<&crate::vault_cli::Cli>, state: &Path, address: &str, task: &str) -> Result<Option<String>> {
    note_waits(vault, state, address, task, true)
}

pub(super) fn note_waits(vault: Option<&crate::vault_cli::Cli>, state: &Path, address: &str, task: &str, tell: bool) -> Result<Option<String>> {
    let (Some(cli), Some(note)) = (vault, crate::task_note::linked(state, address)) else { return Ok(None) };
    match crate::task_note::waits(cli, &note, task) {
        Ok(None) => Ok(None),
        Ok(Some((user, told))) => {
            if tell && let Err(error) = messages::notify(address, &told) {
                eprintln!("Could not tell {address} what its note lacks: {error}");
            }
            Err(user)
        }
        Err(error) => Ok(Some(format!("{note} could not be checked ({error}), so it closed without that check"))),
    }
}

pub(super) fn close_task(cwd: &Path, name: Option<&str>, vault: Option<&crate::vault_cli::Cli>, said: Option<String>, landed: Option<&str>) -> Result<Notice> {
    let repo = main_checkout(cwd)?;
    let _closing = Closing::take(&state_dir()?, &peers::address(task_of(&find_task(&repo, name, cwd)?), &name_of(&repo)))?;
    let slot = find_task(&repo, name, cwd)?;
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    let workspace = herdr::workspace_for(&slot.path)?;
    let address = peers::address(task_of(&slot), &name_of(&repo));
    let state = crate::lead::state_dir()?;
    let landed = landed.map(str::to_string).or_else(|| crate::ci::load(&state, &address).map(|watch| watch.pushed.commits).filter(|commits| !commits.is_empty()))
        .or_else(|| git(&slot.path, &["log", "--reverse", "--format=%h %s", &format!("{upstream}..HEAD")]).ok().filter(|commits| !commits.is_empty()));
    let mut finished = finish(&repo, &slot, &upstream, &crate::lead::state_dir()?, vault)?;
    if let Some(said) = said {
        finished = finished.detail("note", said);
    }
    forget_session(task_of(&slot), &repo);
    forget_approval(task_of(&slot), &repo);
    super::plan::forget(task_of(&slot), &repo);
    super::accept::forget(task_of(&slot), &repo);
    crate::permit::forget(&peers::address(task_of(&slot), &name_of(&repo)));
    super::follow::forget(&peers::address(task_of(&slot), &name_of(&repo)));
    super::park::forget(&peers::address(task_of(&slot), &name_of(&repo)));
    crate::review::forget(task_of(&slot), &repo);
    crate::rules_check::forget(task_of(&slot), &repo);
    crate::report::forget_card(task_of(&slot), &repo);
    crate::evidence::forget(task_of(&slot), &repo);
    super::after::settle_ci(&state, &address);
    crate::recover::forget(&state, &address);
    forget_held_prompt(&address);
    end_life(task_of(&slot), &repo, "done", landed.as_deref(), None);
    Ok(match close_space(workspace) {
        Some(error) => finished.detail("space", error),
        None => finished,
    })
}

pub(super) fn finish(repo: &Path, slot: &Slot, upstream: &str, state: &Path, vault: Option<&crate::vault_cli::Cli>) -> Result<Notice> {
    let task = task_of(slot);
    if !is_clean(&slot.path)? {
        return Notice::new(Tone::You, "Not closed: it has uncommitted changes").you("Commit them in its Space, or drop the task.").detail("task", task).err();
    }
    if stashed(&slot.path, task) {
        return Notice::new(Tone::You, "Not closed: it has a stash").you("Apply and commit it in its Space, or drop it there with git stash drop.").detail("task", task).err();
    }
    let missing = missing_from(&slot.path, upstream)?;
    if !missing.is_empty() {
        let main = upstream.trim_start_matches("origin/");
        return Notice::new(Tone::You, format!("Not closed: some of its commits are not on {main}")).you("Push them first, or drop the task.").detail("task", task).detail("commits", missing.join("\n")).err();
    }
    let freed = release(repo, slot, upstream)?;
    let address = peers::address(task, &name_of(repo));
    forget_address(state, &address);
    let mut said = Vec::new();
    for item in forget_start_in(state, task, repo).map(|text| crate::task_note::items(&text)).unwrap_or_default() {
        let told = match vault.filter(|_| crate::vault_cli::is_note(&item)) {
            Some(_) if crate::task_note::held_elsewhere(state, task, &address, &item) => {
                said.push(format!("{item} stays open for the task's other halves"));
                continue;
            }
            Some(cli) => match crate::task_note::release(cli, state, &item, task) {
                crate::vault_cli::Released::Held(claim) => {
                    said.push(crate::task_note::held_text(&item, &claim, crate::backlog::today()));
                    messages::to_lead(messages::ToLead::NoteUnsettled, &format!("{task} is done and its commits are on the default branch, but {} on {} has taken over its note {item}, so kebbie left the note open: settle it with the user.", claim.task, claim.machine))
                }
                crate::vault_cli::Released::Gone => {
                    said.push(crate::task_note::gone_text(&item));
                    messages::to_lead(messages::ToLead::TaskEnded, &format!("{task} is done and its commits are on the default branch; its note {item} is gone from the vault, so there was nothing to close."))
                }
                crate::vault_cli::Released::Done => close_note(cli, task, &item, &mut said),
            },
            None => messages::to_lead(messages::ToLead::NoteUnsettled, &crate::backlog::done_notice(task, &item)),
        };
        if let Err(error) = told {
            eprintln!("Could not tell the lead: {error}");
        }
    }
    let main = upstream.trim_start_matches("origin/");
    let mut closed = Notice::new(Tone::Done, format!("Closed: its work is on {main}")).next(format!("{} is free for the next task.", name_of(&slot.path)));
    for note in said {
        closed = closed.detail("note", note);
    }
    if let Some(freed) = freed {
        closed = closed.detail("freed", freed);
    }
    Ok(closed.detail("checkout", fast_forward_main(repo, upstream)?))
}

fn close_note(cli: &crate::vault_cli::Cli, task: &str, item: &str, said: &mut Vec<String>) -> Result<()> {
    match crate::task_note::close(cli, item) {
        Ok(path) => {
            said.push(format!("closed ({path})"));
            messages::to_lead(messages::ToLead::TaskEnded, &format!("{task} is done and its commits are on the default branch; kebbie closed its note {item}."))
        }
        Err(error) => {
            said.push(format!("{item} could not be closed ({error})"));
            messages::to_lead(messages::ToLead::NoteUnsettled, &format!("{task} is done and its commits are on the default branch, but kebbie could not close its note {item} ({error}): close it with the vault tools."))
        }
    }
}

// Stashes are shared by every worktree of the repo; git names the branch each was made on.
pub(super) fn stashed(path: &Path, branch: &str) -> bool {
    git(path, &["stash", "list", "--format=%gs"]).unwrap_or_default().lines().any(|line| line.starts_with(&format!("WIP on {branch}:")) || line.starts_with(&format!("On {branch}:")))
}

// The task is looked up again once this is held: a close that won the race has freed it already.
pub(super) struct Closing(std::fs::File);

impl Closing {
    pub(super) fn take(state: &Path, address: &str) -> Result<Closing> {
        let file = crate::state::append_to(&state.join("closing").join(format!("{address}.lock"))).map_err(|error| error.to_string())?;
        // SAFETY: flock on a descriptor this File owns.
        if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&file), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Notice::new(Tone::NotNow, "It is being closed already").detail("task", address).err();
        }
        Ok(Closing(file))
    }
}

impl Drop for Closing {
    // A child forked by another thread holds a copy of the descriptor until it execs; unlocking releases the lock for every copy at once.
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&self.0), libc::LOCK_UN) };
    }
}

// The branch's commits whose change upstream lacks, as "sha subject"; empty when every change landed, however it was cherry-picked or squashed.
pub(super) fn missing_from(path: &Path, upstream: &str) -> Result<Vec<String>> {
    let unmatched: Vec<String> = git(path, &["cherry", "-v", "--abbrev", upstream, "HEAD"])?.lines()
        .filter_map(|line| line.strip_prefix("+ ")).map(str::to_string).collect();
    if unmatched.is_empty() {
        return Ok(unmatched);
    }
    let merged = git(path, &["merge-tree", "--write-tree", upstream, "HEAD"]);
    if merged.is_ok_and(|tree| tree.lines().next() == git(path, &["rev-parse", &format!("{upstream}^{{tree}}")]).ok().as_deref()) {
        return Ok(Vec::new());
    }
    Ok(unmatched)
}

pub fn drop(cwd: &Path, name: Option<&str>, force: bool) -> Result<String> {
    discard(cwd, name, force, "dropped", None).map(|dropped| dropped.to_string())
}

// The refusal of a drop that would discard work names the flag that discards it, so the board can ask again with it.
pub const FORCE: &str = "--force";

pub const ROUTINE_OK: &str = "routine-ok";

// A routine's ok closes its task as a drop does, and its end line says ok so a task waiting on it starts.
pub fn drop_routine_ok(cwd: &Path) -> Result<String> {
    discard(cwd, None, false, "dropped", Some(ROUTINE_OK)).map(|dropped| dropped.to_string())
}

fn discard(cwd: &Path, name: Option<&str>, force: bool, how: &str, kind: Option<&str>) -> Result<Notice> {
    let repo = main_checkout(cwd)?;
    let _closing = Closing::take(&state_dir()?, &peers::address(task_of(&find_task(&repo, name, cwd)?), &name_of(&repo)))?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    let upstream = default_branch(&repo)?;
    let changed = changed_count(&slot.path)?;
    let (ahead, _) = ahead_behind(&slot.path, &upstream)?;
    if !force && (changed > 0 || ahead > 0) {
        let main = upstream.trim_start_matches("origin/");
        return Notice::new(Tone::You, format!("Not dropped: it has {} and {} not on {main}", plural(changed, "uncommitted change"), plural(ahead, "commit")))
            .you(format!("Drop it with {FORCE} to discard them.")).detail("task", task).err();
    }
    let taken_off = super::stage::take_off_before_discard(&state_dir()?, &crate::permit::caller(), &repo, &slot)?;
    let workspace = herdr::workspace_for(&slot.path)?;
    if force {
        git_writing(&slot.path, &["reset", "--quiet", "--hard"])?;
        git_writing(&slot.path, &["clean", "-fdq"])?;
    }
    let freed = release(&repo, &slot, &upstream)?;
    let notes: Vec<String> = forget_start(task, &repo).map(|text| crate::task_note::items(&text)).unwrap_or_default().into_iter().filter(|item| crate::vault_cli::is_note(item)).collect();
    let mut said = Vec::new();
    if let Ok(state) = state_dir() {
        let address = peers::address(task, &name_of(&repo));
        forget_address(&state, &address);
        said = dropped_notes(if notes.is_empty() { None } else { crate::vault_cli::detect() }.as_ref(), &state, task, &address, &notes);
    }
    forget_session(task, &repo);
    forget_approval(task, &repo);
    super::plan::forget(task, &repo);
    super::accept::forget(task, &repo);
    crate::permit::forget(&peers::address(task, &name_of(&repo)));
    super::follow::forget(&peers::address(task, &name_of(&repo)));
    super::park::forget(&peers::address(task, &name_of(&repo)));
    crate::review::forget(task, &repo);
    crate::rules_check::forget(task, &repo);
    crate::report::forget_card(task, &repo);
    crate::evidence::forget(task, &repo);
    if let Ok(state) = state_dir() {
        crate::ci::forget(&state, &peers::address(task, &name_of(&repo)));
        crate::recover::forget(&state, &peers::address(task, &name_of(&repo)));
    }
    forget_held_prompt(&peers::address(task, &name_of(&repo)));
    end_life(task, &repo, how, None, kind);
    let mut dropped = Notice::new(Tone::Done, "Dropped").next(format!("{} is free for the next task.", name_of(&slot.path)));
    if taken_off.is_some() {
        dropped = dropped.detail("main", "its commits were taken off local main first");
    }
    for note in said {
        dropped = dropped.detail("note", note);
    }
    if let Some(freed) = freed {
        dropped = dropped.detail("freed", freed);
    }
    if let Some(error) = close_space(workspace) {
        dropped = dropped.detail("space", error);
    }
    Ok(dropped)
}

// A dropped task's note goes back to the backlog as a reset's does, so no active note is left without a task; without the vault nothing is asked.
fn dropped_notes(vault: Option<&crate::vault_cli::Cli>, state: &Path, task: &str, address: &str, notes: &[String]) -> Vec<String> {
    let Some(cli) = vault else { return Vec::new() };
    notes.iter().map(|note| match note_to_backlog(cli, state, task, address, note) {
        NoteBack::Shared => format!("{note} stays open for the task's other halves"),
        NoteBack::Elsewhere(claim) => crate::task_note::held_text(note, &claim, crate::backlog::today()),
        NoteBack::Gone => crate::task_note::gone_text(note),
        NoteBack::Back(path) => format!("back in the backlog ({path})"),
        NoteBack::Failed(error) => format!("{note} could not go back to the backlog ({error}); set its status to backlog in the vault"),
    }).collect()
}

pub(super) fn forget_address(state: &Path, address: &str) {
    let queued = super::intent::load(state, address);
    super::intent::forget(state, address);
    if let Some(queued) = queued {
        super::line::restamp(state, &queued.repo);
    }
    super::intent::forget_left(state, address);
    crate::phase::forget(state, address);
    crate::task_note::forget(state, address);
    super::landed::forget(state, address);
    super::stage::forget(state, address);
    crate::status::forget_title(state, address);
}

fn forget_held_prompt(address: &str) {
    if let Err(error) = messages::drop_held_for_ended(address) {
        eprintln!("Could not drop what kebbie still held for {address}: {error}");
    }
}

fn end_life(task: &str, repo: &Path, how: &str, landed: Option<&str>, kind: Option<&str>) {
    if let Err(error) = messages::log_end(task, &name_of(repo), how, landed, kind) {
        eprintln!("Could not log the end of {task}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::tasks::test_support::*;
    use crate::repo::{ahead_behind, branch_exists, git, is_clean};
    use crate::agent::claude::{TMP_VARS, settings};
    use crate::tasks::launch::task_tmp;
    use crate::tasks::pool::{find_task, release, slots};
    use crate::tasks::started::{record_start, started_file_in};

    #[test]
    fn done_drop_and_reset_forget_the_approval() {
        let source = include_str!("finish.rs");
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).or_else(|| source.find(&format!("\npub(super) fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("done").contains("close_task(") && body("close_task").contains("forget_approval("));
        assert!(body("discard").contains("forget_approval("));
        assert!(body("reset").contains("discard("));
        assert!(body("close_task").contains("review::forget(") && body("discard").contains("review::forget("), "a later task by the same name must not show this one's review");
        assert!(body("close_task").contains("report::forget_card(") && body("discard").contains("report::forget_card("), "a later task by the same name must not show this one's card");
        assert!(body("close_task").contains("evidence::forget(") && body("discard").contains("evidence::forget("), "a later task by the same name must not show this one's test runs");
        assert!(body("close_task").contains("permit::forget(") && body("discard").contains("permit::forget("), "done, drop and reset take the push permission back");
        assert!(body("close_task").contains("follow::forget(") && body("discard").contains("follow::forget("), "a later task by the same name must not follow main unasked");
        assert!(body("close_task").contains("park::forget(") && body("discard").contains("park::forget("), "a later task by the same name must not start out parked");
        assert!(body("finish").contains("forget_address(") && body("discard").contains("forget_address("), "a later task by the same name must not inherit this one's Problem pass or title");
        assert!(body("close_task").contains("forget_held_prompt(") && body("discard").contains("forget_held_prompt("), "a prompt still held for the task is never typed into a later task by the same name");
        assert!(source.contains("\npub fn drop(cwd: &Path, name: Option<&str>, force: bool) -> Result<String> {\n    discard(cwd, name, force, \"dropped\", None)"));
        assert!(source.contains("discard(cwd, None, false, \"dropped\", Some(ROUTINE_OK))"), "a routine's ok closes like a drop, marked ok");
    }

    #[test]
    fn drop_puts_the_note_back_in_the_backlog_unless_another_half_holds_it() {
        let fake = crate::vault_cli::fake::Fake::new("drop");
        let state = fake.state();
        fake.answer("task-backlog", r#"{"path":"Projects/billing.md"}"#);
        let notes = ["Projects/billing.md".to_string()];
        assert!(dropped_notes(None, &state, "billing", "billing@app", &notes).is_empty(), "without the vault a drop asks nothing");
        assert!(fake.calls().is_empty());
        assert_eq!(dropped_notes(Some(&fake.cli), &state, "billing", "billing@app", &notes), ["back in the backlog (Projects/billing.md)"]);
        std::fs::create_dir_all(state.join("started")).unwrap();
        std::fs::write(state.join("started").join("billing@api"), "Projects/billing.md").unwrap();
        assert!(dropped_notes(Some(&fake.cli), &state, "billing", "billing@app", &notes)[0].contains("stays open for the task's other halves"));
        assert_eq!(fake.calls(), ["task backlog Projects/billing.md"]);
        let source = include_str!("finish.rs");
        let discard = &source[source.find("\nfn discard(").unwrap()..];
        assert!(discard[..discard.find("\n}\n").unwrap()].contains("dropped_notes("), "drop settles the note");
    }

    #[test]
    fn done_closes_the_tasks_vault_note_and_without_the_vault_asks_nothing() {
        let fake = crate::vault_cli::fake::Fake::new("finish");
        fake.answer("task-close", r#"{"path":"Archive/billing.md"}"#);
        for vault in [None, Some(&fake.cli)] {
            let (dir, origin, slot, upstream) = task_with_a_commit("billing");
            git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
            let state = state_of(&dir);
            record_start(&started_file_in(&state, "billing", &dir).unwrap(), "Projects/billing.md").unwrap();
            let said = finish(&dir, &slot, &upstream, &state, vault).unwrap().to_string();
            assert_eq!(said.contains("note: closed (Archive/billing.md)"), vault.is_some(), "{said}");
            cleanup(dir, origin, &slot);
        }
        assert_eq!(fake.calls(), ["task close Projects/billing.md"], "the vault is asked only when it is on");
    }

    #[test]
    fn done_clears_this_machines_claim_before_it_closes_and_leaves_a_note_another_machine_took_over() {
        let fake = crate::vault_cli::fake::Fake::new("finish-claim");
        fake.answer("task-release", r#"{"path":"Projects/billing.md","released":true,"claim":null}"#);
        fake.answer("task-close", r#"{"path":"Archive/billing.md"}"#);
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
        let state = state_of(&dir);
        record_start(&started_file_in(&state, "billing", &dir).unwrap(), "Projects/billing.md").unwrap();
        let said = finish(&dir, &slot, &upstream, &state, Some(&fake.claiming())).unwrap().to_string();
        assert!(said.contains("note: closed (Archive/billing.md)"), "{said}");
        assert_eq!(fake.calls(), ["task release Projects/billing.md --machine mac", "task close Projects/billing.md"]);
        cleanup(dir, origin, &slot);
        let fake = crate::vault_cli::fake::Fake::new("finish-taken");
        fake.answer("task-release", r#"{"path":"Projects/billing.md","released":false,"claim":{"machine":"devbox","task":"billing-v2","since":"2026-10-22T08:00:00Z"}}"#);
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
        let state = state_of(&dir);
        record_start(&started_file_in(&state, "billing", &dir).unwrap(), "Projects/billing.md").unwrap();
        let said = finish(&dir, &slot, &upstream, &state, Some(&fake.claiming())).unwrap().to_string();
        assert!(said.contains("note: Projects/billing.md is left as it is: working on devbox · billing-v2"), "{said}");
        assert_eq!(fake.calls(), ["task release Projects/billing.md --machine mac"], "a note another machine took over is not closed under it");
        cleanup(dir, origin, &slot);
        let landed = include_str!("landed.rs");
        assert!(landed.contains("close_task(repo, Some(task), vault.as_ref(), said, None)"), "a landed task closes through close_task and so clears its claim as done does");
    }

    #[test]
    fn drop_and_reset_clear_this_machines_claim_before_the_note_goes_back_to_the_backlog() {
        let fake = crate::vault_cli::fake::Fake::new("drop-claim");
        let state = fake.state();
        let notes = ["Projects/billing.md".to_string()];
        fake.answer("task-release", r#"{"path":"Projects/billing.md","released":true,"claim":null}"#);
        fake.answer("task-backlog", r#"{"path":"Projects/billing.md"}"#);
        assert_eq!(dropped_notes(Some(&fake.claiming()), &state, "billing", "billing@app", &notes), ["back in the backlog (Projects/billing.md)"]);
        assert!(matches!(note_to_backlog(&fake.claiming(), &state, "billing", "billing@app", &notes[0]), NoteBack::Back(_)), "a reset settles the note the same way");
        assert_eq!(fake.calls(), ["task release Projects/billing.md --machine mac", "task backlog Projects/billing.md", "task release Projects/billing.md --machine mac", "task backlog Projects/billing.md"]);
        fake.answer("task-release", r#"{"path":"Projects/billing.md","released":false,"claim":{"machine":"devbox","task":"billing-v2","since":"2026-10-22T08:00:00Z"}}"#);
        let asked = fake.calls().len();
        assert_eq!(dropped_notes(Some(&fake.claiming()), &state, "billing", "billing@app", &notes), ["Projects/billing.md is left as it is: working on devbox · billing-v2"]);
        assert_eq!(fake.calls().len(), asked + 1, "the note another machine works on stays active: {:?}", fake.calls());
        let source = include_str!("finish.rs");
        let reset = &source[source.find("\npub fn reset(").unwrap()..];
        assert!(reset[..reset.find("\n}\n").unwrap()].contains("note_to_backlog("), "a reset settles the note through note_to_backlog");
    }

    #[test]
    fn done_on_one_half_leaves_the_shared_note_to_the_other_and_settles_the_backlog_item() {
        let fake = crate::vault_cli::fake::Fake::new("finish-halves");
        fake.answer("task-close", r#"{"path":"Archive/billing.md"}"#);
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
        let state = state_of(&dir);
        record_start(&started_file_in(&state, "billing", &dir).unwrap(), "Projects/billing.md\nlocal:Fix login").unwrap();
        std::fs::write(state.join("started").join("billing@elsewhere"), "Projects/billing.md").unwrap();
        let said = finish(&dir, &slot, &upstream, &state, Some(&fake.cli)).unwrap().to_string();
        assert!(said.contains("note: Projects/billing.md stays open for the task's other halves"), "{said}");
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
        assert!(!started_file_in(&state, "billing", &dir).unwrap().exists());
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_later_task_by_the_same_name_shows_its_own_name_not_the_title_of_the_one_that_ended() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
        let state = state_of(&dir);
        let address = peers::address("billing", &name_of(&dir));
        let title = |seen: Option<&str>| crate::status::title_of(&state, &address, seen, &name_of(&dir), "billing", "wt1");
        assert_eq!(title(Some("Refund mails")).as_deref(), Some("Refund mails"));
        assert_eq!(title(None).as_deref(), Some("Refund mails"), "kept while the task runs");
        finish(&dir, &slot, &upstream, &state, None).unwrap();
        assert_eq!(title(None), None, "the next billing task shows its name until its Claude names it");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn done_frees_a_pushed_task_whose_branch_has_no_upstream_while_the_local_branch_is_behind() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
        assert_eq!(ahead_behind(&dir, &upstream).unwrap(), (0, 1));
        finish(&dir, &slot, &upstream, &state_of(&dir), None).unwrap();
        assert!(!branch_exists(&dir, "billing"));
        assert!(slots(&dir).unwrap().iter().any(|free| free.path == slot.path && free.free));
        assert_eq!(ahead_behind(&dir, &upstream).unwrap(), (0, 0));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_tasks_temp_dir_is_its_own_outside_the_worktree_and_goes_when_the_task_is_done() {
        let (dir, origin, slot, upstream) = task_with_a_commit("scratch");
        let tmp = task_tmp(&slot.path).unwrap();
        assert!(tmp.starts_with(dir.join(".git/worktrees").canonicalize().unwrap()), "{}", tmp.display());
        std::fs::create_dir_all(tmp.join("deep")).unwrap();
        std::fs::write(tmp.join("deep/file"), "x").unwrap();
        assert!(is_clean(&slot.path).unwrap(), "temp files never show as changes in the worktree");
        let dirs = crate::sandbox::TaskDirs::new(&slot.path, &tmp);
        let written: Value = serde_json::from_str(&settings("/bin/kebbie", "billing@app", None, Some(&dirs), None)).unwrap();
        for name in TMP_VARS {
            assert_eq!(written["env"][name], json!(tmp.to_string_lossy()), "{name}");
        }
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "scratch"]).unwrap();
        finish(&dir, &slot, &upstream, &state_of(&dir), None).unwrap();
        assert!(!tmp.exists(), "done takes the temp dir and what is in it");
        std::fs::create_dir_all(&tmp).unwrap();
        git(&slot.path, &["switch", "--quiet", "--create", "again"]).unwrap();
        let again = find_task(&dir, Some("again"), &dir).unwrap();
        release(&dir, &again, &upstream).unwrap();
        assert!(!tmp.exists(), "drop and reset free the worktree through the same release");
        let source = include_str!("finish.rs");
        let discard = &source[source.find("\nfn discard(").unwrap()..];
        assert!(discard[..discard.find("\n}\n").unwrap()].contains("release(&repo, &slot, &upstream)"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn done_refuses_a_task_with_commits_not_on_upstream_and_leaves_it_as_it_was() {
        let (dir, origin, slot, upstream) = task_with_a_commit("search");
        assert!(finish(&dir, &slot, &upstream, &state_of(&dir), None).is_err());
        assert!(branch_exists(&dir, "search"));
        assert_eq!(git(&slot.path, &["branch", "--show-current"]).unwrap(), "search");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn done_frees_a_task_whose_commits_were_cherry_picked_onto_upstream() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fifth = commit(&slot.path, "e", "fifth");
        let fourth = git(&slot.path, &["rev-parse", "HEAD~1"]).unwrap();
        land_on_main(&dir, &upstream, |dir| { pick(dir, &fourth); pick(dir, &fifth); });
        assert!(ahead_behind(&slot.path, &upstream).unwrap().0 > 0);
        finish(&dir, &slot, &upstream, &state_of(&dir), None).unwrap();
        assert!(!branch_exists(&dir, "billing"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn done_frees_a_task_whose_commits_were_squashed_into_one_on_upstream() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        commit(&slot.path, "d", "fifth, rewriting the fourth");
        land_on_main(&dir, &upstream, |dir| {
            git(dir, &["merge", "--squash", "--quiet", "billing"]).unwrap();
            git(dir, &["-c", "user.name=Bo", "-c", "user.email=bo@example.com", "commit", "--quiet", "-m", "billing, squashed"]).unwrap();
        });
        assert!(git(&slot.path, &["cherry", &upstream, "HEAD"]).unwrap().contains('+'));
        finish(&dir, &slot, &upstream, &state_of(&dir), None).unwrap();
        assert!(!branch_exists(&dir, "billing"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn done_refuses_a_task_with_one_change_missing_from_upstream_and_names_that_commit() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fifth = commit(&slot.path, "e", "fifth");
        let fourth = git(&slot.path, &["rev-parse", "HEAD~1"]).unwrap();
        land_on_main(&dir, &upstream, |dir| pick(dir, &fourth));
        let error = finish(&dir, &slot, &upstream, &state_of(&dir), None).err().unwrap();
        assert!(error.contains(&fifth[..7]) && error.contains("fifth"), "{error}");
        assert!(!error.contains("fourth") && !error.contains(&fourth[..7]), "{error}");
        assert!(branch_exists(&dir, "billing"));
        assert_eq!(git(&slot.path, &["branch", "--show-current"]).unwrap(), "billing");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn missing_from_names_only_the_commits_whose_change_upstream_lacks() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "--short", "HEAD"]).unwrap();
        let missing = missing_from(&slot.path, &upstream).unwrap();
        assert_eq!(missing.len(), 1);
        assert!(missing[0].starts_with(&fourth) && missing[0].ends_with("fourth"), "{missing:?}");
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
        assert!(missing_from(&slot.path, &upstream).unwrap().is_empty());
        cleanup(dir, origin, &slot);
    }
}
