use std::fs;
use std::path::{Path, PathBuf};

use crate::lead::state_dir;
use crate::notice::{Notice, Tone};
use crate::peers;
use crate::permit::Caller;
use crate::repo::{default_branch, git, main_checkout, name_of};
use crate::sh::Result;
use super::approval::{changed_lines, hashed};
use super::pool::{find_task, task_of};

fn file(state: &Path, address: &str) -> PathBuf {
    state.join("accepted").join(address)
}

pub fn codes_in(state: &Path, address: &str) -> Vec<String> {
    fs::read_to_string(file(state, address)).unwrap_or_default().lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect()
}

fn save(state: &Path, address: &str, codes: &[String]) -> Result<()> {
    if codes.is_empty() {
        return match fs::remove_file(file(state, address)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        };
    }
    crate::state::write(&file(state, address), codes.iter().map(|code| format!("{code}\n")).collect::<String>())
}

pub(crate) fn set_in(state: &Path, address: &str, code: &str, on: bool) -> Result<bool> {
    let mut codes = codes_in(state, address);
    let had = codes.iter().any(|stored| stored == code);
    if on == had {
        return Ok(false);
    }
    if on { codes.push(code.to_string()) } else { codes.retain(|stored| stored != code) }
    save(state, address, &codes).map(|_| true)
}

// Each commit's own change against its parent, without its message, line numbers or context, so a clean rebase or a reword keeps it and a changed line does not.
pub(crate) fn commit_codes(path: &Path, args: &[&str]) -> Result<Vec<(String, String)>> {
    let log = git(path, &[&["log", "-p", "-U0", "--no-color", "--no-ext-diff", "--format=%x00%H"], args].concat())?;
    Ok(log.split('\0').skip(1).map(|commit| {
        let (sha, changes) = commit.split_once('\n').unwrap_or((commit, ""));
        (sha.trim().to_string(), hashed(changed_lines(changes).trim().as_bytes()))
    }).collect())
}

// Commit codes by the shas of their range, which never change, so a refresh runs git log -p only once the default branch or HEAD moves.
pub(super) type Cached<T> = std::sync::Mutex<Option<std::collections::HashMap<(String, String), T>>>;

pub(super) fn cached<T: Clone>(cache: &Cached<T>, key: (&str, &str), read: impl FnOnce() -> Result<T>) -> Result<T> {
    let key = (key.0.to_string(), key.1.to_string());
    if let Some(found) = cache.lock().ok().and_then(|held| held.as_ref().and_then(|map| map.get(&key).cloned())) {
        return Ok(found);
    }
    let read = read()?;
    if let Ok(mut held) = cache.lock() {
        let map = held.get_or_insert_with(Default::default);
        if map.len() >= 256 {
            map.clear();
        }
        map.insert(key, read.clone());
    }
    Ok(read)
}

static LANDED: Cached<Vec<String>> = std::sync::Mutex::new(None);
static RAW: Cached<(Vec<(String, String)>, String)> = std::sync::Mutex::new(None);

// The changes the default branch at upstream_sha gained since base; a base it no longer holds gains nothing.
pub(super) fn landed_between(path: &Path, base: &str, upstream_sha: &str) -> Vec<String> {
    cached(&LANDED, (base, upstream_sha), || Ok(commit_codes(path, &[&format!("{base}..{upstream_sha}")]).map(|codes| codes.into_iter().map(|(_, code)| code).collect()).unwrap_or_default())).unwrap_or_default()
}

// One of the task's commits whose change the default branch has not taken in, with its change and its acceptance key.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Open {
    pub sha: String,
    pub code: String,
    pub key: String,
}

// The task's commits not yet on the default branch, oldest first: a commit whose change reached it since the fork point, by a push of the first commits rebased elsewhere or any other road, is left out once per landed copy, as git rebase drops it; changeless commits always count.
pub(crate) fn open_commits(path: &Path, upstream: &str) -> Result<Vec<Open>> {
    let tips = git(path, &["rev-parse", upstream, "HEAD"])?;
    let (shas, head) = tips.split_once('\n').ok_or_else(|| format!("git rev-parse {upstream} HEAD printed one sha"))?;
    let (raw, base) = cached(&RAW, (shas, head), || {
        let raw = commit_codes(path, &["--reverse", &format!("{shas}..{head}")])?;
        let base = if raw.is_empty() { String::new() } else { git(path, &["merge-base", head, shas])? };
        Ok((raw, base))
    })?;
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let keys = keys(&raw);
    let nothing = hashed(b"");
    let landed = landed_between(path, &base, shas);
    let mut landed: Vec<&str> = landed.iter().map(String::as_str).filter(|code| *code != nothing).collect();
    Ok(raw.into_iter().zip(keys).filter(|((_, code), _)| match landed.iter().position(|one| one == code) {
        Some(at) => {
            landed.remove(at);
            false
        }
        None => true,
    }).map(|((sha, code), key)| Open { sha, code, key }).collect())
}

// What the review's whole-branch diff compares against: the commit under the first one not yet on the default branch, HEAD when the default branch took in every one, None without commits.
pub(crate) fn rest_base(path: &Path, upstream: &str) -> Option<String> {
    let open = open_commits(path, upstream).ok()?;
    match open.first() {
        Some(first) => git(path, &["rev-parse", &format!("{}^", first.sha)]).ok(),
        None if crate::repo::ahead_behind(path, upstream).is_ok_and(|(ahead, _)| ahead > 0) => git(path, &["rev-parse", "HEAD"]).ok(),
        None => None,
    }
}

// An acceptance's key: the change, and for a change an earlier unpushed commit made too, which time it is, so two identical commits are accepted apart.
pub(crate) fn keys(codes: &[(String, String)]) -> Vec<String> {
    codes.iter().enumerate().map(|(at, (_, code))| match codes[..at].iter().filter(|(_, earlier)| earlier == code).count() {
        0 => code.clone(),
        again => format!("{code}#{again}"),
    }).collect()
}

// The task's unpushed commits, oldest first, each with whether the user accepted its code.
pub fn accepted_at(state: &Path, address: &str, path: &Path, upstream: &str) -> Result<Vec<(String, bool)>> {
    let accepted = codes_in(state, address);
    Ok(open_commits(path, upstream)?.into_iter().map(|open| (open.sha, accepted.contains(&open.key))).collect())
}

// The last commit of the unbroken run of accepted ones from the first unpushed commit.
pub fn last_of_run(commits: &[(String, bool)]) -> Option<&str> {
    commits.iter().take_while(|(_, accepted)| *accepted).last().map(|(sha, _)| sha.as_str())
}

struct Task {
    path: PathBuf,
    upstream: String,
    address: String,
}

fn task_at(cwd: &Path, name: Option<&str>) -> Result<Task> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    Ok(Task { upstream: default_branch(&repo)?, address: peers::address(task_of(&slot), &name_of(&repo)), path: slot.path })
}

// What the push window preselects: None when the first unpushed commit is not accepted, or the task has none.
pub fn last_accepted(path: &Path) -> Option<String> {
    let task = task_at(path, None).ok()?;
    let commits = accepted_at(&state_dir().ok()?, &task.address, &task.path, &task.upstream).ok()?;
    last_of_run(&commits).map(str::to_string)
}

// The task's unpushed commits, oldest first, each with whether the user accepted it, in one pass; empty when path is no task's worktree.
pub fn accepted_in(path: &Path) -> Vec<(String, bool)> {
    let (Ok(task), Ok(state)) = (task_at(path, None), state_dir()) else { return Vec::new() };
    accepted_at(&state, &task.address, &task.path, &task.upstream).unwrap_or_default()
}

// `kebbie accepted [name] [--json]`: a read, so it is anyone's, a task's too, whatever task.steps says; its own name keeps it clear of the deny rule on kebbie accept.
pub fn shown(cwd: &Path, name: Option<&str>, as_json: bool) -> Result<String> {
    let task = task_at(cwd, name)?;
    let listed = listing(&state_dir()?, &task)?;
    if as_json {
        return serde_json::to_string_pretty(&listed).map_err(|error| error.to_string());
    }
    let commits = listed["commits"].as_array().cloned().unwrap_or_default();
    if commits.is_empty() {
        return Ok(format!("{} has no commits to push.", task.address));
    }
    let lines = commits.iter().map(|commit| {
        let sha = commit["sha"].as_str().unwrap_or_default();
        format!("  {} {} {}", if commit["accepted"] == true { "✓" } else { " " }, &sha[..7.min(sha.len())], commit["subject"].as_str().unwrap_or_default())
    });
    Ok(std::iter::once(format!("{}'s commits to push:", task.address)).chain(lines).collect::<Vec<_>>().join("\n"))
}

// What kebbie accepted --json prints for the task whose worktree is path, read in-process; None outside a task.
pub fn listing_at(path: &Path) -> Option<serde_json::Value> {
    listing(&state_dir().ok()?, &task_at(path, None).ok()?).ok()
}

fn listing(state: &Path, task: &Task) -> Result<serde_json::Value> {
    let commits = accepted_at(state, &task.address, &task.path, &task.upstream)?;
    let subjects = git(&task.path, &["log", "--format=%H%x09%s", &format!("{}..HEAD", task.upstream)])?;
    let subject = |sha: &str| subjects.lines().find_map(|line| line.strip_prefix(sha).and_then(|rest| rest.strip_prefix('\t'))).unwrap_or_default().to_string();
    let listed: Vec<serde_json::Value> = commits.iter().map(|(sha, accepted)| serde_json::json!({ "sha": sha, "subject": subject(sha), "accepted": accepted })).collect();
    Ok(serde_json::json!({ "task": task.address, "last_accepted": last_of_run(&commits), "commits": listed }))
}

// The user's word that one commit's code is right; it holds through a clean rebase and a reword, and a changed line takes it back.
pub fn accept(cwd: &Path, name: Option<&str>, sha: Option<&str>, on: bool) -> Result<String> {
    accept_as(&crate::permit::caller(), cwd, name, sha, on)
}

// Read from the Claude above, not the folder, which kebbie -C or a cd sets to anything.
fn accept_as(caller: &Caller, cwd: &Path, name: Option<&str>, sha: Option<&str>, on: bool) -> Result<String> {
    crate::permit::user_only_as(caller, "accepting a commit")?;
    let name = name.ok_or("kebbie accept needs a task name and a commit: kebbie accept <name> <sha> [--withdraw]")?;
    let sha = sha.ok_or("kebbie accept needs the commit: kebbie accept <name> <sha> [--withdraw]")?;
    let task = task_at(cwd, Some(name))?;
    accept_in(&state_dir()?, &task, sha, on)
}

fn accept_in(state: &Path, task: &Task, sha: &str, on: bool) -> Result<String> {
    let full = git(&task.path, &["rev-parse", "--verify", "--quiet", &format!("{sha}^{{commit}}")]).map_err(|_| Notice::new(Tone::Failed, "Not accepted: that is not a commit").detail("commit", sha).to_string())?;
    let Some(key) = open_commits(&task.path, &task.upstream)?.into_iter().find(|open| open.sha == full).map(|open| open.key) else {
        return Notice::new(Tone::Failed, "That commit is not one of the task's commits to push").detail("task", &task.address).detail("commit", &full[..7.min(full.len())]).err();
    };
    let changed = set_in(state, &task.address, &key, on)?;
    let short = &full[..7.min(full.len())];
    let headline = match (on, changed) {
        (true, true) => "Accepted",
        (true, false) => "Already accepted",
        (false, true) => "Acceptance withdrawn",
        (false, false) => "It was not accepted",
    };
    let notice = Notice::new(Tone::Done, headline).detail("task", &task.address).detail("commit", short);
    if on { notice.next("A changed line in it takes the acceptance back.").ok() } else { notice.ok() }
}

pub fn forget(task: &str, repo: &Path) {
    if let Ok(state) = state_dir() {
        let _ = save(&state, &peers::address(task, &name_of(repo)), &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::test_support::*;
    use crate::tasks::history::reword;

    fn task(slot: &crate::tasks::Slot, upstream: &str) -> Task {
        Task { path: slot.path.clone(), upstream: upstream.to_string(), address: "billing@app".to_string() }
    }

    #[test]
    fn an_acceptance_survives_a_clean_rebase_and_a_reworded_message_and_a_changed_line_drops_it() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fifth = commit(&slot.path, "e", "fifth");
        let state = state_of(&dir);
        accept_in(&state, &task(&slot, &upstream), &fifth[..8], true).unwrap();
        let accepted = |commits: Vec<(String, bool)>| commits.into_iter().map(|(_, accepted)| accepted).collect::<Vec<_>>();
        assert_eq!(accepted(accepted_at(&state, "billing@app", &slot.path, &upstream).unwrap()), [false, true], "only the commit the user accepted");
        land_on_main(&dir, &upstream, |_| ());
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "rebase", "--quiet", &upstream]).unwrap();
        assert_ne!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), fifth);
        assert_eq!(accepted(accepted_at(&state, "billing@app", &slot.path, &upstream).unwrap()), [false, true], "a clean rebase keeps it");
        reword(&slot.path, &git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), "fifth, reworded").unwrap();
        assert_eq!(accepted(accepted_at(&state, "billing@app", &slot.path, &upstream).unwrap()), [false, true], "a new message keeps it");
        std::fs::write(slot.path.join("e"), "fifth, changed").unwrap();
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--amend", "-a", "--no-edit"]).unwrap();
        assert_eq!(accepted(accepted_at(&state, "billing@app", &slot.path, &upstream).unwrap()), [false, false], "a changed line is code the user never accepted");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn two_commits_with_the_same_change_are_accepted_apart_and_keep_their_own_through_a_rebase() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let empty = |message: &str| { git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--allow-empty", "-m", message]).unwrap(); git(&slot.path, &["rev-parse", "HEAD"]).unwrap() };
        empty("first empty");
        let second = empty("second empty");
        let state = state_of(&dir);
        accept_in(&state, &task(&slot, &upstream), &second, true).unwrap();
        let accepted = || accepted_at(&state, "billing@app", &slot.path, &upstream).unwrap().into_iter().map(|(_, accepted)| accepted).collect::<Vec<_>>();
        assert_eq!(accepted(), [false, false, true], "the same change twice is two commits to accept");
        land_on_main(&dir, &upstream, |_| ());
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "rebase", "--quiet", "--keep-empty", &upstream]).unwrap();
        assert_eq!(accepted(), [false, false, true], "a clean rebase keeps which of them was accepted");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn withdrawing_takes_the_acceptance_back_and_only_the_tasks_own_commits_are_accepted() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let state = state_of(&dir);
        assert!(accept_in(&state, &task(&slot, &upstream), &fourth, true).unwrap().contains("Accepted"));
        assert!(accept_in(&state, &task(&slot, &upstream), &fourth, true).unwrap().contains("Already accepted"));
        assert!(accept_in(&state, &task(&slot, &upstream), &fourth, false).unwrap().contains("Acceptance withdrawn"));
        assert!(codes_in(&state, "billing@app").is_empty() && !file(&state, "billing@app").exists(), "nothing is left behind");
        assert!(accept_in(&state, &task(&slot, &upstream), &upstream, true).unwrap_err().contains("not one of the task's commits"), "a commit on the default branch is not the task's");
        assert!(accept_in(&state, &task(&slot, &upstream), "no-such-ref", true).unwrap_err().contains("not a commit"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_last_accepted_commit_ends_the_unbroken_run_from_the_first_unpushed_one() {
        let commits = |marks: &[bool]| marks.iter().enumerate().map(|(at, accepted)| (format!("c{at}"), *accepted)).collect::<Vec<_>>();
        assert_eq!(last_of_run(&commits(&[true, true, false, true])), Some("c1"), "a gap ends the run, whatever is accepted after it");
        assert_eq!(last_of_run(&commits(&[false, true])), None, "the first unpushed commit is not accepted");
        assert_eq!(last_of_run(&commits(&[true, true])), Some("c1"));
        assert_eq!(last_of_run(&commits(&[])), None);
    }

    #[test]
    fn the_last_accepted_commit_is_read_from_the_tasks_own_commits() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let fifth = commit(&slot.path, "e", "fifth");
        let sixth = commit(&slot.path, "f", "sixth");
        let state = state_of(&dir);
        for sha in [&fourth, &sixth] {
            accept_in(&state, &task(&slot, &upstream), sha, true).unwrap();
        }
        let commits = accepted_at(&state, "billing@app", &slot.path, &upstream).unwrap();
        assert_eq!(last_of_run(&commits), Some(fourth.as_str()), "fifth is a gap");
        accept_in(&state, &task(&slot, &upstream), &fifth, true).unwrap();
        assert_eq!(last_of_run(&accepted_at(&state, "billing@app", &slot.path, &upstream).unwrap()), Some(sixth.as_str()));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_listing_names_each_unpushed_commit_with_its_subject_and_whether_it_is_accepted() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let fifth = commit(&slot.path, "e", "fifth");
        let state = state_of(&dir);
        accept_in(&state, &task(&slot, &upstream), &fourth, true).unwrap();
        let listed = listing(&state, &task(&slot, &upstream)).unwrap();
        assert_eq!(listed["last_accepted"].as_str(), Some(fourth.as_str()));
        assert_eq!(listed["commits"], serde_json::json!([{ "sha": fourth, "subject": "fourth", "accepted": true }, { "sha": fifth, "subject": "fifth", "accepted": false }]));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn only_the_user_accepts_a_commit_whatever_folder_it_runs_from() {
        for caller in [Caller::Lead, Caller::Task("billing@app".to_string()), Caller::Agent, Caller::Sandboxed, Caller::LeadSandboxed] {
            for on in [true, false] {
                let refused = accept_as(&caller, Path::new("/nowhere"), Some("billing"), Some("abc"), on).unwrap_err();
                assert!(refused.contains("accepting a commit is the user's call") && refused.contains(&crate::permit::describe(&caller)), "{refused}");
            }
        }
        assert!(!accept_as(&Caller::User, Path::new("/nowhere"), Some("billing"), Some("abc"), true).unwrap_err().contains("the user's call"), "the user gets past the guard");
    }
}
