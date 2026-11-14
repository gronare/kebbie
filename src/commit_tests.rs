use std::collections::BTreeMap;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use kebbie_hub_protocol::{CommitTests, TestsState};
use serde_json::{Value, json};

use crate::peers;
use crate::repo::{default_branch, git, main_checkout, name_of};
use crate::sh::Result;
use crate::tasks::{self, Placed};

pub const EVERY: u64 = 15;
const KEPT: usize = 200;
const COMMANDS: usize = 2;
const CEILING: Duration = Duration::from_secs(30 * 60);
const ARGS: [&str; 2] = ["order", "--run-tests"];

fn dir(state: &Path) -> PathBuf {
    state.join("commit-tests")
}

fn file(state: &Path, address: &str, kind: &str) -> PathBuf {
    dir(state).join(format!("{address}.{kind}"))
}

fn tests_state_of(word: &str) -> Option<TestsState> {
    match word {
        "passed" => Some(TestsState::Passed),
        "failed" => Some(TestsState::Failed),
        "running" => Some(TestsState::Running),
        "not-run" => Some(TestsState::NotRun),
        _ => None,
    }
}

// Each commit's own result by its sha, which never changes, so a commit is tested once however often the task reports ready.
pub fn results(state: &Path, address: &str) -> BTreeMap<String, CommitTests> {
    let Ok(text) = fs::read_to_string(file(state, address, "json")) else { return BTreeMap::new() };
    let Ok(Value::Object(all)) = serde_json::from_str::<Value>(&text) else { return BTreeMap::new() };
    all.into_iter().filter_map(|(sha, value)| {
        let state = tests_state_of(value["state"].as_str()?)?;
        Some((sha, CommitTests { state, at: value["at"].as_u64().filter(|_| state != TestsState::NotRun) }))
    }).collect()
}

fn save(state: &Path, address: &str, all: &BTreeMap<String, (CommitTests, u64)>) -> Result<()> {
    let mut newest: Vec<(&String, &(CommitTests, u64))> = all.iter().collect();
    newest.sort_by_key(|(_, (_, noted))| std::cmp::Reverse(*noted));
    let kept: serde_json::Map<String, Value> = newest.into_iter().take(KEPT).map(|(sha, (tests, noted))| (sha.clone(), json!({ "state": tasks::tests_word(tests.state), "at": tests.at, "noted": noted }))).collect();
    crate::state::write(&file(state, address, "json"), Value::Object(kept).to_string())
}

fn noted(state: &Path, address: &str) -> BTreeMap<String, (CommitTests, u64)> {
    let raw: BTreeMap<String, u64> = fs::read_to_string(file(state, address, "json")).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().map(|all| all.iter().map(|(sha, value)| (sha.clone(), value["noted"].as_u64().unwrap_or_default())).collect())).unwrap_or_default();
    results(state, address).into_iter().map(|(sha, tests)| { let at = raw.get(&sha).copied().unwrap_or_default(); (sha, (tests, at)) }).collect()
}

// The push order's rows carry what the last runs said of each commit; a read of one small file, nothing runs.
pub fn mark(state: &Path, address: &str, order: &mut [Placed]) {
    if order.is_empty() {
        return;
    }
    let known = results(state, address);
    if known.is_empty() {
        return;
    }
    for row in order.iter_mut() {
        row.tests = known.get(&row.sha).cloned();
    }
}

// A ready report asks for the task's unpushed commits to be tested one by one; the keeper starts the run, so it runs from the keeper's own environment.
pub fn want(state: &Path, address: &str, worktree: &Path) -> Result<()> {
    crate::state::write(&file(state, address, "wanted"), worktree.to_string_lossy().as_ref())
}

pub fn on_ready(cwd: &Path) {
    let _ = (|| -> Result<()> {
        let repo = main_checkout(cwd)?;
        if !crate::config::resolve(Some(&repo))?.settings.tests_per_commit.value {
            return Ok(());
        }
        let slot = tasks::find_task(&repo, None, cwd)?;
        want(&crate::lead::state_dir()?, &peers::address(tasks::task_of(&slot), &name_of(&repo)), &slot.path)
    })();
}

pub fn forget(task: &str, repo: &Path) {
    let Ok(state) = crate::lead::state_dir() else { return };
    let address = peers::address(task, &name_of(repo));
    if let Some(pid) = running_pid(&state, &address) {
        crate::review::stop_group(&pid.to_string());
    }
    for kind in ["json", "wanted", "running", "run", "log"] {
        let _ = fs::remove_file(file(&state, &address, kind));
    }
    let _ = remove_tree(repo, &tree_of(&state, &address));
}

fn is_ours(args: &str) -> bool {
    args.contains(&ARGS.join(" "))
}

fn running_pid(state: &Path, address: &str) -> Option<u32> {
    let pid: u32 = fs::read_to_string(file(state, address, "running")).ok()?.trim().parse().ok()?;
    crate::sh::run("ps", &["-p", &pid.to_string(), "-o", "args="], None).ok().filter(|args| is_ours(args)).map(|_| pid)
}

// One directory read at rest: only a ready report leaves a wanted file, and only then does a run start.
pub fn tick(state: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir(state)) else { return Vec::new() };
    let wanted: Vec<String> = entries.flatten().filter_map(|entry| entry.file_name().to_string_lossy().strip_suffix(".wanted").map(str::to_string)).collect();
    let mut notes = Vec::new();
    for address in wanted {
        if running_pid(state, &address).is_some() {
            continue;
        }
        let worktree = fs::read_to_string(file(state, &address, "wanted")).map(|text| PathBuf::from(text.trim())).unwrap_or_default();
        let _ = fs::remove_file(file(state, &address, "wanted"));
        if !crate::rules_check::worktree_is(&worktree, &address) {
            continue;
        }
        notes.push(match start(state, &address, &worktree) {
            Ok(()) => format!("tests: testing {address}'s commits one by one"),
            Err(error) => format!("tests: {address}: the run could not start ({error})"),
        });
    }
    notes
}

fn start(state: &Path, address: &str, worktree: &Path) -> Result<()> {
    let mut command = Command::new(peers::own_exe()?);
    command.arg("-C").arg(worktree).args(ARGS).current_dir(worktree).stdin(Stdio::piped()).process_group(0);
    let pid = crate::trusted_spawn::launch(command, &file(state, address, "log"), &file(state, address, "run"), "")?;
    crate::state::write(&file(state, address, "running"), pid.to_string())
}

// `kebbie order --run-tests`: only the keeper's own start, with the nonce it handed over, tests anything.
pub fn run(cwd: &Path) -> Result<String> {
    let presented = crate::trusted_spawn::presented();
    let repo = main_checkout(cwd)?;
    let slot = tasks::find_task(&repo, None, cwd)?;
    let address = peers::address(tasks::task_of(&slot), &name_of(&repo));
    let state = crate::lead::state_dir()?;
    let record = fs::read_to_string(file(&state, &address, "run")).ok();
    let _ = fs::remove_file(file(&state, &address, "run"));
    if !crate::trusted_spawn::trusted(&crate::permit::caller(), &presented, record.as_deref()) {
        return Err("kebbie order --run-tests is the keeper's own; a task's commits are tested once it reports ready".to_string());
    }
    crate::state::write(&file(&state, &address, "running"), std::process::id().to_string())?;
    let configured = crate::config::resolve(Some(&repo))?.settings.test_commands.value;
    let commands = commands_for(&state, &address, &slot.path, &configured);
    let tested = test_all(&state, &repo, &slot.path, &address, &commands, &|command, tree| run_command(command, tree, CEILING));
    let _ = fs::remove_file(file(&state, &address, "running"));
    tested
}

// The commands a commit is tested with: those the task's own recorded runs used, newest first, else the repo's task.test_commands; none in a sandboxed task, which kebbie never runs outside its sandbox.
pub fn commands_for(state: &Path, address: &str, worktree: &Path, configured: &[String]) -> Vec<String> {
    if crate::sandbox::marked(state, worktree) {
        return Vec::new();
    }
    let mut commands: Vec<String> = Vec::new();
    for run in crate::evidence::runs_in(state, address).iter().rev().filter(|run| run.result != crate::evidence::Outcome::Unknown) {
        if !run.runner.is_empty() && !commands.contains(&run.runner) {
            commands.push(run.runner.clone());
        }
    }
    if commands.is_empty() {
        commands = configured.to_vec();
    }
    commands.truncate(COMMANDS);
    commands
}

fn tree_of(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.tree"))
}

fn remove_tree(repo: &Path, tree: &Path) -> Result<()> {
    if tree.exists() {
        let _ = git(repo, &["worktree", "remove", "--force", &tree.to_string_lossy()]);
        let _ = fs::remove_dir_all(tree);
    }
    git(repo, &["worktree", "prune"]).map(|_| ())
}

// The tree a recorded run ran on, so a commit the task's own run already tested on its own is not run again.
fn tree(path: &Path, rev: &str) -> Option<String> {
    git(path, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{tree}}")]).ok()
}

type Runner<'a> = &'a dyn Fn(&str, &Path) -> bool;

// Each unpushed commit, oldest first, one at a time, in one throwaway worktree of kebbie's that is gone again at the end.
pub fn test_all(state: &Path, repo: &Path, worktree: &Path, address: &str, commands: &[String], runner: Runner) -> Result<String> {
    let upstream = default_branch(repo)?;
    let open: Vec<String> = tasks::open_commits_of(worktree, &upstream)?;
    let runs: Vec<(String, bool)> = crate::evidence::runs_in(state, address).into_iter()
        .filter(|run| run.result != crate::evidence::Outcome::Unknown)
        .filter_map(|run| Some((tree(worktree, &run.code)?, run.result == crate::evidence::Outcome::Passed))).collect();
    let mut all = noted(state, address);
    let tree_dir = tree_of(state, address);
    let mut made = false;
    let mut counts = (0, 0, 0);
    for sha in &open {
        if all.get(sha).is_some_and(|(tests, _)| matches!(tests.state, TestsState::Passed | TestsState::Failed)) {
            continue;
        }
        let now = crate::messages::now();
        let recorded = tree(worktree, sha).and_then(|own| runs.iter().rev().find(|(tree, _)| *tree == own).map(|(_, passed)| *passed));
        let passed = match recorded {
            Some(passed) => Some(passed),
            None if commands.is_empty() => None,
            None => {
                all.insert(sha.clone(), (CommitTests { state: TestsState::Running, at: Some(now) }, now));
                save(state, address, &all)?;
                if !made {
                    let _ = remove_tree(repo, &tree_dir);
                    fs::create_dir_all(dir(state)).map_err(|error| error.to_string())?;
                    git(worktree, &["worktree", "add", "--quiet", "--detach", "--force", &tree_dir.to_string_lossy(), sha])?;
                    made = true;
                }
                git(&tree_dir, &["checkout", "--quiet", "--detach", "--force", sha])?;
                let _ = git(&tree_dir, &["clean", "--quiet", "-fd"]);
                Some(commands.iter().all(|command| runner(command, &tree_dir)))
            }
        };
        let tests = match passed {
            Some(true) => { counts.0 += 1; CommitTests { state: TestsState::Passed, at: Some(crate::messages::now()) } }
            Some(false) => { counts.1 += 1; CommitTests { state: TestsState::Failed, at: Some(crate::messages::now()) } }
            None => { counts.2 += 1; CommitTests { state: TestsState::NotRun, at: None } }
        };
        all.insert(sha.clone(), (tests, crate::messages::now()));
        save(state, address, &all)?;
    }
    if made {
        let _ = remove_tree(repo, &tree_dir);
    }
    Ok(format!("{address}: {} passed, {} failed, {} not run", counts.0, counts.1, counts.2))
}

// A command that runs past the ceiling counts as failed, and its whole process group is stopped.
fn run_command(command: &str, tree: &Path, ceiling: Duration) -> bool {
    let Ok(mut child) = Command::new("sh").args(["-c", command]).current_dir(tree).stdin(Stdio::null()).process_group(0).spawn() else { return false };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() > ceiling => {
                crate::review::stop_group(&child.id().to_string());
                let _ = child.wait();
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
mod tests;
