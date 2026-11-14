use std::cell::RefCell;

use super::*;
use crate::tasks::test_support::*;

fn run_of(command: &str, code: &str, passed: bool) -> crate::evidence::Run {
    crate::evidence::Run { command: command.to_string(), runner: command.to_string(), result: if passed { crate::evidence::Outcome::Passed } else { crate::evidence::Outcome::Failed }, sha: code.to_string(), code: code.to_string(), at: 1, ..Default::default() }
}

#[test]
fn each_unpushed_commit_is_tested_once_by_its_sha_and_a_commit_the_tasks_own_run_tested_is_not_run_again() {
    let (dir, origin, slot, _) = task_with_a_commit("billing");
    let state = state_of(&dir);
    let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
    let fifth = commit(&slot.path, "e", "fifth");
    let sixth = commit(&slot.path, "f", "sixth");
    crate::evidence::save(&state, "billing@app", &run_of("cargo test", &fourth, true)).unwrap();
    let commands = commands_for(&state, "billing@app", &slot.path, &["bin/test".to_string()]);
    assert_eq!(commands, ["cargo test"], "the command the task's own runs used comes before the configured one");
    let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let runner = |command: &str, tree: &Path| {
        let head = git(tree, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(tree, slot.path.as_path(), "a commit runs in kebbie's own worktree, never the task's");
        seen.borrow_mut().push(format!("{command} {head}"));
        head != sixth
    };
    let said = test_all(&state, &dir, &slot.path, "billing@app", &commands, &runner).unwrap();
    assert_eq!(said, "billing@app: 2 passed, 1 failed, 0 not run");
    assert_eq!(*seen.borrow(), [format!("cargo test {fifth}"), format!("cargo test {sixth}")], "one at a time, oldest first, and fourth is the task's own passed run");
    let known = results(&state, "billing@app");
    assert_eq!((known[&fourth].state, known[&fifth].state, known[&sixth].state), (TestsState::Passed, TestsState::Passed, TestsState::Failed));
    assert!(!tree_of(&state, "billing@app").exists() && !git(&dir, &["worktree", "list"]).unwrap().contains("commit-tests"), "the throwaway worktree is gone");
    test_all(&state, &dir, &slot.path, "billing@app", &commands, &runner).unwrap();
    assert_eq!(seen.borrow().len(), 2, "a commit tested before is not run again");

    let mut order: Vec<Placed> = [&fourth, &sixth, &"f".repeat(40)].iter().map(|sha| Placed { sha: sha.to_string(), ..Placed::default() }).collect();
    mark(&state, "billing@app", &mut order);
    assert_eq!(order.iter().map(|row| row.tests.as_ref().map(|tests| tests.state)).collect::<Vec<_>>(), [Some(TestsState::Passed), Some(TestsState::Failed), None]);
    let seventh = commit(&slot.path, "g", "seventh");
    test_all(&state, &dir, &slot.path, "billing@app", &[], &runner).unwrap();
    assert_eq!(results(&state, "billing@app")[&seventh], CommitTests { state: TestsState::NotRun, at: None }, "without a command a commit is not run");
    crate::sandbox::mark(&state, &slot.path, true).unwrap();
    assert!(commands_for(&state, "billing@app", &slot.path, &["bin/test".to_string()]).is_empty(), "kebbie runs nothing of a sandboxed task's outside its sandbox");
    cleanup(dir, origin, &slot);
}

#[test]
fn at_rest_the_tick_reads_one_folder_and_starts_nothing() {
    let state = crate::sh::test_dir("commit-tests-rest");
    assert!(tick(&state).is_empty(), "no folder, nothing to do");
    want(&state, "billing@app", Path::new("/nowhere/wt1.app")).unwrap();
    assert!(tick(&state).is_empty(), "a wanted run whose worktree is not the task's starts nothing");
    assert!(!file(&state, "billing@app", "wanted").exists(), "and is dropped");
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_command_past_its_ceiling_fails() {
    let dir = crate::sh::test_dir("commit-tests-ceiling");
    assert!(run_command("true", &dir, Duration::from_secs(5)));
    assert!(!run_command("exit 3", &dir, Duration::from_secs(5)));
    assert!(!run_command("sleep 5", &dir, Duration::from_millis(300)));
    std::fs::remove_dir_all(dir).unwrap();
}
