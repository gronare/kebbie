use std::path::Path;

use crate::board::widgets::plural;
use crate::config;
use crate::herdr;
use crate::lead::state_dir;
use crate::messages;
use crate::peers;
use crate::repo::{ahead_behind, default_branch, git, git_writing, is_clean, main_checkout, name_of};
use crate::sh::Result;
use super::approval::{approved, fingerprint, record_approval, same_code};
use super::finish::{close_task, note_ready};
use super::{fast_forward_main, fetch};
use super::pool::{Slot, find_task, task_of};
use super::started::started_file;

pub fn land(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    if !is_clean(&slot.path)? {
        return Err(format!("{task} has uncommitted changes in {}; commit or stash them first", slot.path.display()));
    }
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    if ahead_behind(&slot.path, &upstream)?.0 == 0 {
        return Err(format!("{task} has no commits to land"));
    }
    if !rebase_or_hand_over(&slot, &upstream, "kelpie tried to land this task")? {
        return Err(format!("{task} conflicts with {upstream}; its Claude is resolving the rebase. Push again when it reports back."));
    }
    let commits = git(&slot.path, &["log", "--oneline", &format!("{upstream}..HEAD")])?;
    Ok(format!("{task} is rebased onto {upstream} and ready:\n{commits}\n\nPush with:  kelpie push {task}"))
}

// Ok(false) when the rebase conflicted and the task's Claude took it over.
fn rebase_or_hand_over(slot: &Slot, upstream: &str, why: &str) -> Result<bool> {
    if git_writing(&slot.path, &["rebase", "--quiet", upstream]).is_ok() {
        return Ok(true);
    }
    git_writing(&slot.path, &["rebase", "--abort"])?;
    let task = task_of(slot);
    let Some(pane) = claude_of(&slot.path) else {
        return Err(format!("{task} conflicts with {upstream} and has no Claude to resolve it; rebase it in {}", slot.path.display()));
    };
    let text = format!(
        "{why} and rebasing onto {upstream} conflicts (already fetched). Rebase this branch onto {upstream} \
         with git rebase {upstream} (no -i), resolve each conflict keeping the intent of both sides (read the upstream \
         commits that touched the file), run the tests for the files involved, and continue until the rebase is finished. Do not \
         push. Then tell me in a few lines what conflicted and how you resolved it."
    );
    crate::typing::prompt(&pane, &text, task).map_err(|refused| format!("{task} conflicts with {upstream}, and its Claude {}, so kelpie typed nothing; rebase it in {} or clear its box and land again", refused.reason(), slot.path.display()))?;
    Ok(false)
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
    let pane = claude_of(&slot.path).ok_or_else(|| format!("{task} has no Claude running to tidy its commits"))?;
    let upstream = default_branch(&repo)?;
    let rules = config::resolve(Some(&repo))?.settings.rules_skill.value;
    let mut prompt = format!(
        "Reshape this branch's commits on top of {upstream} for review, following the skill `{rules}`. Interactive git is not \
         available: use git rebase with GIT_SEQUENCE_EDITOR editing the todo, git commit --amend, or git reset --soft and recommit. \
         The final tree must be identical: git diff against the old head must be empty. Do not push. Show the commit list before and after."
    );
    if !note.trim().is_empty() {
        prompt.push_str(&format!(" From the user: {}", note.trim()));
    }
    crate::typing::prompt(&pane, &prompt, task).map_err(|refused| format!("{task}'s Claude {}, so kelpie typed nothing; tidy again once its box is clear", refused.reason()))?;
    Ok(format!("Asked {task}'s Claude to tidy its commits."))
}

// After land: pushes the task's commits through `through` (all of them without it) to the default branch, then closes the task, or with keep_open leaves the rest of its branch on top of what was pushed.
pub fn push(cwd: &Path, name: Option<&str>, through: Option<&str>, keep_open: bool) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot).to_string();
    let upstream = default_branch(&repo)?;
    let (ahead, behind) = ahead_behind(&slot.path, &upstream)?;
    if ahead == 0 || behind > 0 || !is_clean(&slot.path)? {
        return Err(format!("{task} is not landed; run land first"));
    }
    let head = git(&slot.path, &["rev-parse", "HEAD"])?;
    let sha = pushable_through(&slot.path, &upstream, through.unwrap_or(&head), keep_open)?;
    let keep_open = stays_open(keep_open, &sha, &head);
    let address = peers::address(&task, &name_of(&repo));
    let shown = crate::review::shown(&address, &slot.path, &upstream);
    let covered = |reviewed: &str| same_code(&slot.path, &upstream, reviewed, &head) || same_code(&slot.path, &upstream, reviewed, &sha);
    crate::review::gate(shown.as_ref(), covered).map_err(|why| format!("{task} cannot be pushed yet: {why}."))?;
    let was_approved = approved(&slot.path, &upstream, &address);
    let check = Pusher { caller: crate::permit::caller(), permit: crate::permit::of(&address, &name_of(&repo)), reviewed: shown.as_ref().is_some_and(|shown| !shown.pending()), approved: was_approved };
    let pusher = check.allowed().map_err(|why| format!("{task} cannot be pushed by {}: {why}.", crate::permit::describe(&check.caller)))?;
    let state = state_dir()?;
    let pushed = git(&slot.path, &["log", "--reverse", "--format=%h %s", &format!("{upstream}..{sha}")])?;
    let note = started_file(&task, &repo).ok().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| crate::task_note::items(&text).into_iter().next());
    let count = push_prefix(&slot.path, &upstream, &sha, &state).map_err(|error| format!("Pushing {task} failed: {error}"))?;
    let vault = crate::vault_cli::detect();
    let log = vault.as_ref().map(|cli| crate::task_note::log_push(cli, &slot.path, &task, &pushed));
    let logged = log.as_ref().map(crate::task_note::logged_text).unwrap_or_default();
    let target = upstream.trim_start_matches("origin/").to_string();
    let landed = |closed: bool, waits: bool| Landed { pusher, address: &address, target: &target, commits: &pushed, closed, note: note.as_deref(), logged: log.as_ref().map(|log| matches!(log, Ok(Some(_)))), waits };
    let short = &sha[..7.min(sha.len())];
    let told_ci = crate::ci::Pushed { pusher: pusher.to_string(), target: target.clone(), commits: pushed.clone(), note: note.clone(), logged: log.as_ref().map(|log| matches!(log, Ok(Some(_)))) };
    let watch = crate::ci::begin(&state, &repo, &address, &sha, !keep_open, told_ci);
    let ci = match &watch {
        crate::ci::Begin::Watching => format!(" kelpie watches CI for {short}."),
        crate::ci::Begin::NotWatched(reason) => format!(" CI not watched: {reason}."),
        crate::ci::Begin::Off => String::new(),
    };
    if !keep_open {
        if matches!(watch, crate::ci::Begin::Watching) {
            if let Err(error) = messages::notify_lead(&format!("{} pushed {address}: {} to {target}. kelpie watches CI for {short} and closes the task once every run is green; you hear what landed then.", capitalized(pusher), plural(count, "commit"))) {
                eprintln!("Could not tell the lead about the push: {error}");
            }
            return Ok(format!("Pushed {} to {upstream}.{logged} {task} stays open under CI running until CI for {short} is green; kelpie closes it then. {}", plural(count, "commit"), fast_forward_main(&repo, &upstream)?));
        }
        let ready = note_ready(vault.as_ref(), &state, &address, &task);
        if let Err(waits) = &ready {
            tell_lead_pushed(&landed(false, true));
            return Ok(format!("Pushed {} to {upstream}.{logged}{ci} {waits}", plural(count, "commit")));
        }
        let closed = close_task(cwd, name, vault.as_ref(), ready.unwrap_or_default());
        tell_lead_pushed(&landed(closed.is_ok(), false));
        return closed.map(|closed| format!("Pushed {} to {upstream}.{logged}{ci} {closed}", plural(count, "commit")));
    }
    tell_lead_pushed(&landed(false, false));
    let (clean, rest) = match keep_going(&state, &repo, &slot, &upstream, &address, was_approved)? {
        Rest::Rebased { clean, rest } => (clean, rest),
        Rest::NotFetched(error) => return Ok(format!("Pushed {} through {short} to {upstream}.{logged} Fetching afterwards failed ({error}), so the rest of {task} is not rebased onto it yet: run kelpie land {task}.", plural(count, "commit"))),
    };
    let told = format!(
        "{} pushed {} of this task, through {short}, to {target}; they are on {target} for good. Your branch continues from there with {} on top: reshape only those.",
        capitalized(pusher), plural(count, "commit"), plural(rest, "commit")
    );
    if let Err(error) = messages::notify(&address, &told) {
        eprintln!("Could not tell {address} about the push: {error}");
    }
    let after = if clean { format!("{task} stays open with {} on top.", plural(rest, "commit")) } else { format!("{task} stays open; its Claude is resolving the rebase of the rest.") };
    Ok(format!("Pushed {} through {short} to {upstream}.{logged}{ci} {after} {}", plural(count, "commit"), fast_forward_main(&repo, &upstream)?))
}

pub fn after_ci(repo: &Path, task: &str, address: &str, told: &crate::ci::Pushed) -> Result<String> {
    let state = state_dir()?;
    let vault = crate::vault_cli::detect();
    let landed = |closed: bool, waits: bool| Landed { pusher: &told.pusher, address, target: &told.target, commits: &told.commits, closed, note: told.note.as_deref(), logged: told.logged, waits };
    let ready = note_ready(vault.as_ref(), &state, address, task);
    if let Err(waits) = &ready {
        tell_lead_pushed(&landed(false, true));
        return Ok(waits.clone());
    }
    let closed = close_task(repo, Some(task), vault.as_ref(), ready.unwrap_or_default());
    tell_lead_pushed(&landed(closed.is_ok(), false));
    closed
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map(|first| first.to_uppercase().chain(characters).collect()).unwrap_or_default()
}

// Who may push this task now: the user always, the lead only with the user's permission and its own rules check, a task or any other Claude never.
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
                    return Err("its commits have no rules approval of yours that matches them: check them and run kelpie approve first".to_string());
                }
                Ok("the lead")
            }
            Caller::Task(_) | Caller::Agent => Err("no agent pushes; the user pushes from the board, or the lead for a task the user gave push permission".to_string()),
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

// logged is None without the vault, else whether kelpie wrote the repo's log line itself; with the vault kelpie settles the note.
fn pushed_notice(landed: &Landed) -> String {
    let Landed { pusher, address, target, commits, closed, note, logged, waits } = *landed;
    let count = commits.lines().count();
    let mut text = format!("{} pushed {address}: {} to {target} ({}).", capitalized(pusher), plural(count, "commit"), commits.lines().collect::<Vec<_>>().join("; "));
    text.push_str(match (closed, waits) {
        (true, _) => " The task is done and closed.",
        (false, true) => " The task stays open until its note has its solution and decisions; kelpie asked it to write them.",
        (false, false) => " The task stays open with the rest of its branch.",
    });
    match logged {
        Some(true) => return text + " kelpie wrote the repo's log line in the vault from these commits; update the area note with what changed.",
        Some(false) => return text + " kelpie could not write the repo's log line: write it in the vault with what landed and these shas, and update the area note with what changed.",
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

fn tell_lead_pushed(landed: &Landed) {
    if let Err(error) = messages::tell_lead(&pushed_notice(landed), Some("push")) {
        eprintln!("Could not tell the lead about the push: {error}");
    }
}

#[derive(Debug, PartialEq)]
enum Rest {
    Rebased { clean: bool, rest: usize },
    NotFetched(String),
}

// After a push that keeps the task open: the rest of its branch goes on top of the new default branch, an approval of the whole branch holds for the rest, and the card, written for what was pushed, goes.
fn keep_going(state: &Path, repo: &Path, slot: &Slot, upstream: &str, address: &str, was_approved: bool) -> Result<Rest> {
    crate::report::forget_card_in(state, address);
    if let Err(error) = fetch(repo) {
        return Ok(Rest::NotFetched(error));
    }
    let clean = rebase_or_hand_over(slot, upstream, "kelpie pushed this task's first commits")?;
    let rest = ahead_behind(&slot.path, upstream)?.0;
    if clean && was_approved && rest > 0 {
        record_approval(state, address, &fingerprint(&slot.path, upstream)?)?;
    }
    Ok(Rest::Rebased { clean, rest })
}

// A push through the last commit leaves nothing to keep the task open for, so it closes like a full push.
fn stays_open(keep_open: bool, sha: &str, head: &str) -> bool {
    keep_open && sha != head
}

// One of the task's commits not yet on upstream; a full push names HEAD, so a commit that arrived after the dialog opened is never pushed unseen.
fn pushable_through(path: &Path, upstream: &str, rev: &str, keep_open: bool) -> Result<String> {
    let sha = git(path, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).map_err(|_| format!("{rev} is not a commit"))?;
    let on_branch = crate::sh::succeeds("git", &["merge-base", "--is-ancestor", &sha, "HEAD"], Some(path));
    let new = git(path, &["rev-list", "--count", &format!("{upstream}..{sha}")])?.parse::<usize>().unwrap_or(0) > 0;
    if !on_branch || !new {
        return Err(format!("{} is not one of the task's commits to push", &sha[..7.min(sha.len())]));
    }
    if !keep_open && sha != git(path, &["rev-parse", "HEAD"])? {
        return Err(format!("The branch has commits after {} that were not shown; open push again to see them", &sha[..7.min(sha.len())]));
    }
    Ok(sha)
}

// Fast-forwards the default branch on origin to sha; the remote-tracking branch follows.
fn push_prefix(path: &Path, upstream: &str, sha: &str, state: &Path) -> Result<usize> {
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
    use crate::tasks::approval::{approval_file, approval_holds, fingerprint, record_approval};
    use crate::tasks::pool::slots;

    #[test]
    fn only_the_user_pushes_freely_the_lead_with_permission_and_its_approval_and_no_agent_at_all() {
        use crate::permit::{Caller, Permit};
        let check = |caller: Caller, permit: Option<Permit>, reviewed: bool, approved: bool| Pusher { caller, permit, reviewed, approved }.allowed();
        assert_eq!(check(Caller::User, None, false, false), Ok("the user"));
        assert_eq!(check(Caller::Lead, Some(Permit::Task), false, true), Ok("the lead"));
        assert!(check(Caller::Lead, None, true, true).unwrap_err().contains("no push permission"));
        assert!(check(Caller::Lead, Some(Permit::Task), true, false).unwrap_err().contains("kelpie approve"));
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
    fn with_the_vault_on_the_lead_hears_kelpie_logged_the_push_and_is_not_asked_to_log_it() {
        let commits = "a1b2c3d Add refunds";
        let kept = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing.md"), logged: Some(true), waits: false });
        assert!(kept.contains("kelpie wrote the repo's log line") && kept.contains("update the area note") && !kept.contains("Write the repo's log line"), "{kept}");
        let waits = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: false, note: Some("Projects/billing.md"), logged: Some(true), waits: true });
        assert!(waits.contains("stays open until its note has its solution and decisions"), "{waits}");
        let failed = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing.md"), logged: Some(false), waits: false });
        assert!(failed.contains("kelpie could not write the repo's log line: write it in the vault with what landed and these shas") && !failed.contains("kelpie wrote"), "a log append that failed leaves the line to the lead: {failed}");
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
        assert!(unseen.contains("not shown"), "a commit that arrived after the dialog opened is never pushed unseen: {unseen}");
        assert!(pushable_through(&slot.path, &upstream, &upstream, true).unwrap_err().contains("not one of the task's commits"));
        let elsewhere = commit(&dir, "x", "on main only");
        assert!(pushable_through(&slot.path, &upstream, &elsewhere, true).unwrap_err().contains("not one of the task's commits"));
        assert!(pushable_through(&slot.path, &upstream, "no-such-ref", true).unwrap_err().contains("is not a commit"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_of_the_first_commits_leaves_the_task_open_with_the_rest_on_top_and_its_approval() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        let state = dir.with_extension("state");
        let _ = std::fs::remove_dir_all(&state);
        record_approval(&state, "billing@app", &fingerprint(&slot.path, &upstream).unwrap()).unwrap();
        crate::report::save_card(&state, "billing@app", &fourth, "Refunds on the billing page.").unwrap();
        assert_eq!(push_prefix(&slot.path, &upstream, &fourth, &state).unwrap(), 1);
        let branch = upstream.trim_start_matches("origin/");
        assert_eq!(git(&origin, &["rev-parse", branch]).unwrap(), fourth, "origin has the first commit");
        assert_eq!(keep_going(&state, &dir, &slot, &upstream, "billing@app", true).unwrap(), Rest::Rebased { clean: true, rest: 1 });
        assert!(branch_exists(&dir, "billing"), "the task stays open");
        assert_eq!(git(&slot.path, &["branch", "--show-current"]).unwrap(), "billing");
        assert!(slots(&dir).unwrap().iter().any(|busy| busy.path == slot.path && !busy.free), "its worktree stays the task's");
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0), "the rest sits on top of what was pushed");
        assert_eq!(git(&slot.path, &["log", "--format=%s", &format!("{upstream}..HEAD")]).unwrap(), "fifth", "no pushed commit twice");
        assert!(approval_holds(&state, "billing@app", &fingerprint(&slot.path, &upstream).unwrap()), "an approval of the whole branch holds for the rest");
        assert!(crate::report::card_in(&state, "billing@app").is_none(), "the card was for what was pushed");
        assert!(matches!(keep_going(&state.join("other"), &dir, &slot, &upstream, "billing@app", false).unwrap(), Rest::Rebased { rest: 1, .. }));
        assert!(!approval_file(&state.join("other"), "billing@app").exists(), "no approval is made up for a branch the lead never approved");
        std::fs::remove_dir_all(state).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_partial_push_that_landed_is_reported_even_when_the_fetch_after_it_fails() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        let state = dir.with_extension("state");
        let _ = std::fs::remove_dir_all(&state);
        crate::report::save_card(&state, "billing@app", &fourth, "Refunds on the billing page.").unwrap();
        push_prefix(&slot.path, &upstream, &fourth, &state).unwrap();
        git(&dir, &["remote", "set-url", "origin", &dir.with_extension("gone").to_string_lossy()]).unwrap();
        assert!(matches!(keep_going(&state, &dir, &slot, &upstream, "billing@app", true).unwrap(), Rest::NotFetched(_)), "the commits are on origin, so the push is no failure");
        assert!(crate::report::card_in(&state, "billing@app").is_none(), "the card was for what was pushed");
        git(&dir, &["remote", "set-url", "origin", &origin.to_string_lossy()]).unwrap();
        std::fs::remove_dir_all(state).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_through_the_last_commit_closes_the_task_like_a_full_push() {
        assert!(!stays_open(true, "abc", "abc"), "p on the last commit, or --through HEAD, leaves no commit to keep open for");
        assert!(stays_open(true, "abc", "def"));
        assert!(!stays_open(false, "abc", "def"));
    }

    #[test]
    fn capitalized_takes_the_first_character_up_and_leaves_the_rest() {
        assert_eq!(capitalized("the lead"), "The lead");
        assert_eq!(capitalized(""), "");
        assert_eq!(capitalized("ärlig"), "Ärlig");
        assert_eq!(capitalized("ßx"), "SSx");
    }
}
