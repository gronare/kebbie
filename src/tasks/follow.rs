use std::collections::BTreeSet;
use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::board::widgets::plural;
use crate::messages;
use crate::peers;
use crate::permit::Caller;
use crate::repo::{ahead_behind, default_branch, git, main_checkout, name_of, rebase_in_progress, uncommitted};
use crate::sh::Result;
use super::pool::{Slot, find_task, slots, task_of};
use super::push::{HandOver, Touched, Updated, conflict_prompt, hand_over, update};

const WHY: &str = "This task follows main, so kebbie tried to rebase it when main moved";

fn dir(state: &Path) -> PathBuf {
    state.join("follow-main")
}

fn flag_file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(address)
}

// Addresses carry an @, so this folder never shares a name with a task's flag.
fn handed_file(state: &Path, address: &str) -> PathBuf {
    dir(state).join("handed").join(address)
}

pub fn follows_in(state: &Path, address: &str) -> bool {
    flag_file(state, address).is_file()
}

pub fn follows(address: &str) -> bool {
    crate::lead::state_location().is_ok_and(|state| follows_in(&state, address))
}

// The flag holds the repo's path, so the keeper finds the task without herdr.
pub fn set_in(state: &Path, address: &str, repo: &Path, on: bool) -> Result<()> {
    if on {
        return crate::state::write(&flag_file(state, address), repo.to_string_lossy().as_bytes());
    }
    for file in [flag_file(state, address), handed_file(state, address)] {
        if let Err(error) = fs::remove_file(&file) && error.kind() != std::io::ErrorKind::NotFound {
            return Err(error.to_string());
        }
    }
    Ok(())
}

pub fn forget(address: &str) {
    if let Ok(state) = crate::lead::state_dir() {
        let _ = set_in(&state, address, Path::new(""), false);
    }
}

fn flagged(state: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir(state)) else { return Vec::new() };
    let mut found: Vec<(String, PathBuf)> = entries.filter_map(|entry| entry.ok()).filter(|entry| entry.path().is_file())
        .filter_map(|entry| {
            let address = entry.file_name().to_string_lossy().to_string();
            let repo = fs::read_to_string(entry.path()).ok()?;
            (address.contains('@') && !address.starts_with('.') && !repo.trim().is_empty()).then(|| (address, PathBuf::from(repo.trim())))
        }).collect();
    found.sort();
    found
}

fn switched(task: &str, upstream: &str, on: bool) -> String {
    let main = upstream.trim_start_matches("origin/");
    if on {
        crate::notice::Notice::new(crate::notice::Tone::Done, format!("It follows {main} now")).next(format!("When {main} moves, the keeper rebases it while its Claude is idle and its files are committed; a conflict goes to its Claude. Nothing is pushed.")).detail("task", task).to_string()
    } else {
        crate::notice::Notice::new(crate::notice::Tone::Done, format!("It no longer follows {main}")).next(format!("Rebase it when you choose (e on the board, or kebbie land {task}).")).detail("task", task).to_string()
    }
}

pub fn toggle(repo: &Path, task: &str) -> Result<String> {
    let state = crate::lead::state_dir()?;
    let address = peers::address(task, &name_of(repo));
    let on = !follows_in(&state, &address);
    set_in(&state, &address, repo, on)?;
    Ok(switched(task, &default_branch(repo).unwrap_or_else(|_| "main".to_string()), on))
}

pub fn follow(cwd: &Path, words: &[String]) -> Result<String> {
    let switch = |word: &str| matches!(word, "on" | "off");
    let (name, word) = match words {
        [] => (None, None),
        [word] if switch(word) => (None, Some(word.as_str())),
        [name] => (Some(name.as_str()), None),
        [name, word] if switch(word) => (Some(name.as_str()), Some(word.as_str())),
        _ => return Err("kebbie follow takes a task and on or off: kebbie follow [name] [on|off]".to_string()),
    };
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    let address = peers::address(task, &name_of(&repo));
    let upstream = default_branch(&repo)?;
    let state = crate::lead::state_dir()?;
    let Some(word) = word else {
        return Ok(if follows_in(&state, &address) { switched(task, &upstream, true) } else { format!("{task} does not follow {upstream}: kebbie follow {task} on switches it on.") });
    };
    let caller = crate::permit::caller();
    if !matches!(caller, Caller::User | Caller::Lead) {
        return Err(format!("switching whether {task} follows main is the user's or the lead's; {} cannot", crate::permit::describe(&caller)));
    }
    set_in(&state, &address, &repo, word == "on")?;
    Ok(switched(task, &upstream, word == "on"))
}

#[derive(Debug, PartialEq)]
pub(super) enum Step {
    Waits(&'static str),
    Current,
    Rebased(Vec<Touched>),
    Handed,
    Stuck(String),
}

pub(super) fn step(state: &Path, slot: &Slot, address: &str, waiting: Option<bool>, hand: HandOver, tell: &dyn Fn(&str)) -> Result<Step> {
    if waiting == Some(false) {
        return Ok(Step::Waits("its Claude is mid-turn"));
    }
    if rebase_in_progress(&slot.path) {
        return Ok(Step::Waits("a rebase is in progress"));
    }
    if !uncommitted(&slot.path)?.is_empty() {
        return Ok(Step::Waits("it has uncommitted files"));
    }
    if super::intent::load(state, address).is_some() {
        return Ok(Step::Waits("a push waits for its tests"));
    }
    if crate::review::running_in(state, address) {
        return Ok(Step::Waits("its reviewer is reading it"));
    }
    let upstream = default_branch(&main_checkout(&slot.path)?)?;
    if ahead_behind(&slot.path, &upstream)?.1 == 0 {
        return Ok(Step::Current);
    }
    let pair = format!("{} {}", git(&slot.path, &["rev-parse", &upstream])?, git(&slot.path, &["rev-parse", "HEAD"])?);
    if fs::read_to_string(handed_file(state, address)).is_ok_and(|seen| seen.trim() == pair) {
        return Ok(Step::Waits("its conflict with this upstream is handed over"));
    }
    let outcome = update(&slot.path, &upstream);
    if !matches!(outcome, Ok(Updated::Clean | Updated::Overlap(_))) {
        crate::state::write(&handed_file(state, address), &pair)?;
    }
    match outcome {
        Ok(Updated::Clean) => Ok(Step::Rebased(Vec::new())),
        Ok(Updated::Overlap(touched)) => {
            tell(&overlap_text(&upstream, &touched));
            Ok(Step::Rebased(touched))
        }
        Ok(Updated::Conflict) => match hand(slot, &conflict_prompt(WHY, &upstream)) {
            Ok(()) => Ok(Step::Handed),
            Err(cannot) => Ok(Step::Stuck(format!("rebasing onto {upstream} conflicts and {cannot}"))),
        },
        Err(refused) => Ok(Step::Stuck(refused)),
    }
}

fn overlap_text(upstream: &str, touched: &[Touched]) -> String {
    let files: Vec<String> = touched.iter().map(|touched| format!("- {}: {}", touched.file, touched.commits.join("; "))).collect();
    format!(
        "This task follows main, so kebbie rebased it onto {upstream} without conflicts, but the commits it was rebased over changed files this task \
         changes too:\n{}\nRun the tests that cover those files and fix what broke with a new commit. Do not push.",
        files.join("\n")
    )
}

fn stale(repo: &Path) -> bool {
    crate::fetch::fetched_at(repo).is_none_or(|at| SystemTime::now().duration_since(at).unwrap_or_default() >= crate::fetch::EVERY)
}

// Some(true) idle at its prompt, Some(false) mid-turn or asking, None no Claude in the task's Space.
fn waiting_of(path: &Path, workspaces: &[Value], agents: &[Value]) -> Option<bool> {
    let workspace = workspaces.iter().find(|workspace| workspace["worktree"]["checkout_path"].as_str() == path.to_str())?["workspace_id"].as_str()?;
    let agent = agents.iter().find(|agent| agent["workspace_id"].as_str() == Some(workspace))?;
    crate::restart::Pane::from_agent(agent).map(|pane| pane.waiting)
}

fn log_line(address: &str, text: &str) {
    if let Ok(_lock) = messages::Lock::take(true) {
        let _ = messages::append(&json!({ "id": messages::new_id("follow"), "at": messages::now(), "from": "kebbie", "to": address, "text": text, "state": "logged", "kind": "follow" }));
    }
}

pub fn tick(state: &Path) -> Vec<String> {
    if flagged(state).is_empty() {
        return Vec::new();
    }
    let Some(_lock) = Lock::take(state) else { return Vec::new() };
    let (Ok(workspaces), Ok(agents)) = (crate::herdr::workspaces(), crate::herdr::agents()) else { return Vec::new() };
    pass(state, &workspaces, &agents, &hand_over)
}

struct Lock(fs::File);

impl Lock {
    fn take(state: &Path) -> Option<Lock> {
        let file = crate::state::append_to(&dir(state).join(".lock")).ok()?;
        // SAFETY: flock on a descriptor this File owns.
        (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(Lock(file))
    }
}

impl Drop for Lock {
    // A child forked by another thread holds a copy of the descriptor until it execs; unlocking releases the lock for every copy at once.
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn pass(state: &Path, workspaces: &[Value], agents: &[Value], hand: HandOver) -> Vec<String> {
    let flagged = flagged(state);
    let mut fetched = BTreeSet::new();
    let mut notes = Vec::new();
    for (address, repo) in flagged {
        let Ok(pool) = slots(&repo) else { continue };
        let task = address.split('@').next().unwrap_or_default();
        let Some(slot) = pool.into_iter().find(|slot| slot.task.as_deref() == Some(task)) else {
            let _ = set_in(state, &address, &repo, false);
            continue;
        };
        if fetched.insert(repo.clone()) && stale(&repo) && super::fetch(&repo).is_err() {
            continue;
        }
        let tell = |text: &str| {
            if let Err(error) = messages::notify(&address, text) {
                eprintln!("Could not tell {address} about the rebase: {error}");
            }
        };
        let said = match step(state, &slot, &address, waiting_of(&slot.path, workspaces, agents), hand, &tell) {
            Ok(Step::Rebased(touched)) => {
                let upstream = default_branch(&repo).unwrap_or_default();
                let ahead = ahead_behind(&slot.path, &upstream).map(|counts| counts.0).unwrap_or_default();
                let text = format!("kebbie rebased {address} onto {upstream}, {} on top{}", plural(ahead, "commit"), if touched.is_empty() { "" } else { "; its Claude tests the files both changed" });
                log_line(&address, &text);
                text
            }
            Ok(Step::Handed) => {
                let text = format!("{address} follows main and rebasing it conflicted, so its Claude is resolving the rebase. Its commits change: check them against the rules again when it reports ready.");
                if let Err(error) = messages::tell_lead(&text, Some("follow")) {
                    eprintln!("Could not tell the lead: {error}");
                }
                text
            }
            Ok(Step::Stuck(why)) => {
                let text = format!("{address} follows main but kebbie could not rebase it: {why}. kebbie tries again once main or the task moves.");
                if let Err(error) = messages::tell_lead(&text, Some("follow")) {
                    eprintln!("Could not tell the lead: {error}");
                }
                text
            }
            Ok(Step::Waits(_) | Step::Current) => continue,
            Err(error) => format!("follow {address}: {error}"),
        };
        notes.push(format!("follow: {said}"));
    }
    notes
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::tasks::test_support::*;

    #[test]
    fn the_follow_flag_round_trips_in_state_and_off_also_forgets_a_handed_conflict() {
        let state = crate::sh::test_dir("follow-flag");
        assert!(!follows_in(&state, "billing@app"));
        assert!(flagged(&state).is_empty());
        set_in(&state, "billing@app", Path::new("/work/app"), true).unwrap();
        assert!(follows_in(&state, "billing@app"));
        assert!(!follows_in(&state, "billing@web"), "the flag is the task's in its repo");
        assert_eq!(flagged(&state), vec![("billing@app".to_string(), PathBuf::from("/work/app"))]);
        crate::state::write(&handed_file(&state, "billing@app"), "a b").unwrap();
        assert_eq!(flagged(&state).len(), 1, "the handed folder is no task");
        set_in(&state, "billing@app", Path::new(""), false).unwrap();
        set_in(&state, "billing@app", Path::new(""), false).unwrap();
        assert!(!follows_in(&state, "billing@app") && !handed_file(&state, "billing@app").exists());
        let _ = fs::remove_dir_all(state);
    }

    type Typed = std::rc::Rc<RefCell<Vec<String>>>;

    fn handed() -> (Typed, impl Fn(&Slot, &str) -> std::result::Result<(), String>) {
        let texts = std::rc::Rc::new(RefCell::new(Vec::new()));
        let kept = texts.clone();
        (texts, move |_: &Slot, text: &str| { kept.borrow_mut().push(text.to_string()); Ok(()) })
    }

    #[test]
    fn an_idle_committed_task_behind_main_is_rebased_and_one_mid_turn_or_with_uncommitted_files_is_not() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        let (typed, hand) = handed();
        let told = RefCell::new(Vec::new());
        let tell = |text: &str| told.borrow_mut().push(text.to_string());
        assert_eq!(step(&state, &slot, "billing@app", Some(false), &hand, &tell).unwrap(), Step::Waits("its Claude is mid-turn"));
        std::fs::write(slot.path.join("d"), "half done").unwrap();
        assert_eq!(step(&state, &slot, "billing@app", Some(true), &hand, &tell).unwrap(), Step::Waits("it has uncommitted files"));
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 1), "nothing moved under the work in progress");
        git(&slot.path, &["checkout", "--", "d"]).unwrap();
        std::fs::write(slot.path.join("scratch.txt"), "untracked").unwrap();
        assert_eq!(step(&state, &slot, "billing@app", Some(true), &hand, &tell).unwrap(), Step::Rebased(Vec::new()));
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0));
        assert_eq!(step(&state, &slot, "billing@app", None, &hand, &tell).unwrap(), Step::Current);
        assert!(typed.borrow().is_empty() && told.borrow().is_empty(), "a clean rebase types nothing");
        assert_eq!(git(&origin, &["rev-list", "--count", "HEAD"]).unwrap(), "4", "never pushed");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_keepers_pass_rebases_a_flagged_idle_task_and_leaves_one_that_does_not_follow_main_alone() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let other = crate::tasks::pool::create_slot(&dir, "HEAD").unwrap();
        git(&other.path, &["switch", "--quiet", "--create", "search"]).unwrap();
        commit(&other.path, "s", "search");
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        let address = peers::address("billing", &name_of(&dir));
        assert!(tick(&state).is_empty(), "nothing follows main, so herdr is not even asked");
        set_in(&state, &address, &dir, true).unwrap();
        set_in(&state, &peers::address("gone", &name_of(&dir)), &dir, true).unwrap();
        let _ = tick(&state);
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 1), "without herdr kebbie cannot tell whether its Claude is mid-turn, so it waits");
        let workspaces: Vec<Value> = [(&slot, "w1"), (&other, "w2")].iter().map(|(slot, id)| json!({ "workspace_id": id, "worktree": { "checkout_path": slot.path } })).collect();
        let agents: Vec<Value> = ["w1", "w2"].iter().enumerate().map(|(at, id)| json!({ "agent": "claude", "pane_id": format!("{id}:p{at}"), "workspace_id": id, "agent_status": "idle", "interactive_ready": true })).collect();
        let (typed, hand) = handed();
        let notes = pass(&state, &workspaces, &agents, &hand);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with(&format!("follow: kebbie rebased {address} onto {upstream}, 1 commit on top")), "{notes:?}");
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0));
        assert_eq!(ahead_behind(&other.path, &upstream).unwrap(), (1, 1), "a task that does not follow main stays where it is");
        assert!(!follows_in(&state, &peers::address("gone", &name_of(&dir))), "a task that is gone loses its flag");
        assert!(typed.borrow().is_empty());
        assert!(messages::load().iter().any(|message| message.to == address && message.text.contains("rebased")), "the message log has the line");
        assert!(pass(&state, &workspaces, &agents, &hand).is_empty(), "up to date, so silent");
        git(&dir, &["worktree", "remove", "--force", &other.path.to_string_lossy()]).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_conflict_is_handed_to_its_claude_once_per_upstream_and_head() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let state = state_of(&dir);
        upstream_commit(&dir, "d", "theirs", "d on main");
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let (typed, hand) = handed();
        let tell = |_: &str| {};
        assert_eq!(step(&state, &slot, "billing@app", Some(true), &hand, &tell).unwrap(), Step::Handed);
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), head, "the conflict is aborted for its Claude to redo");
        assert!(!rebase_in_progress(&slot.path));
        assert_eq!(typed.borrow().len(), 1);
        assert!(typed.borrow()[0].contains("follows main") && typed.borrow()[0].contains("Do not push"), "{}", typed.borrow()[0]);
        assert_eq!(step(&state, &slot, "billing@app", Some(true), &hand, &tell).unwrap(), Step::Waits("its conflict with this upstream is handed over"));
        assert_eq!(typed.borrow().len(), 1, "the next pass with the same upstream types nothing");
        upstream_commit(&dir, "e", "more", "e on main");
        assert_eq!(step(&state, &slot, "billing@app", Some(true), &hand, &tell).unwrap(), Step::Handed, "main moved again, so it is tried again");
        assert_eq!(typed.borrow().len(), 2);
        let refused = |_: &Slot, _: &str| Err("it has no Claude running to take it over".to_string());
        upstream_commit(&dir, "f", "more", "f on main");
        let Step::Stuck(why) = step(&state, &slot, "billing@app", None, &refused, &tell).unwrap() else { panic!("no Claude to hand it to") };
        assert!(why.contains("conflicts") && why.contains("no Claude"), "{why}");
        assert_eq!(step(&state, &slot, "billing@app", None, &refused, &tell).unwrap(), Step::Waits("its conflict with this upstream is handed over"), "nor is a conflict without a Claude retried every pass");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_rebase_over_the_tasks_own_files_is_done_and_its_claude_told_to_test_them() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n", "add m");
        git(&slot.path, &["rebase", "--quiet", &upstream]).unwrap();
        std::fs::write(slot.path.join("m"), "one\n2\n3\n4\n5\n6\n7\n8\n9\n10\n").unwrap();
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-am", "first line of m"]).unwrap();
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        let (_, hand) = handed();
        let told = RefCell::new(Vec::new());
        let tell = |text: &str| told.borrow_mut().push(text.to_string());
        let Step::Rebased(touched) = step(&state, &slot, "billing@app", Some(true), &hand, &tell).unwrap() else { panic!("main changed m too") };
        assert_eq!(touched.len(), 1);
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (2, 0));
        assert!(told.borrow()[0].contains("- m: ") && told.borrow()[0].contains("Do not push"), "{:?}", told.borrow());
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_rebase_git_refuses_to_start_is_reported_once_per_upstream_and_head() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        let (_, hand) = handed();
        let tell = |_: &str| {};
        git(&slot.path, &["update-index", "--skip-worktree", "a"]).unwrap();
        std::fs::write(slot.path.join("a"), "local only").unwrap();
        upstream_commit(&dir, "a", "theirs", "a on main");
        let Step::Stuck(refused) = step(&state, &slot, "billing@app", Some(true), &hand, &tell).unwrap() else { panic!("git refuses to start") };
        assert!(refused.contains("skip-worktree"), "{refused}");
        assert_eq!(step(&state, &slot, "billing@app", Some(true), &hand, &tell).unwrap(), Step::Waits("its conflict with this upstream is handed over"));
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap().1, 2);
        cleanup(dir, origin, &slot);
    }
}
