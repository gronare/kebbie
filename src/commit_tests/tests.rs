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
    let said = test_all(&state, &dir, &slot.path, "billing@app", &commands, None, &runner).unwrap();
    assert_eq!(said, "billing@app: 2 passed, 1 failed, 0 not run");
    assert_eq!(*seen.borrow(), [format!("cargo test {fifth}"), format!("cargo test {sixth}")], "one at a time, oldest first, and fourth is the task's own passed run");
    let known = results(&state, "billing@app");
    assert_eq!((known[&fourth].state, known[&fifth].state, known[&sixth].state), (TestsState::Passed, TestsState::Passed, TestsState::Failed));
    assert!(!tree_of(&state, "billing@app").exists() && !git(&dir, &["worktree", "list"]).unwrap().contains("commit-tests"), "the throwaway worktree is gone");
    test_all(&state, &dir, &slot.path, "billing@app", &commands, None, &runner).unwrap();
    assert_eq!(seen.borrow().len(), 2, "a commit tested before is not run again");

    let mut order: Vec<Placed> = [&fourth, &sixth, &"f".repeat(40)].iter().map(|sha| Placed { sha: sha.to_string(), ..Placed::default() }).collect();
    mark(&state, "billing@app", &mut order);
    assert_eq!(order.iter().map(|row| row.tests.as_ref().map(|tests| tests.state)).collect::<Vec<_>>(), [Some(TestsState::Passed), Some(TestsState::Failed), None]);
    let seventh = commit(&slot.path, "g", "seventh");
    test_all(&state, &dir, &slot.path, "billing@app", &[], None, &runner).unwrap();
    assert_eq!(results(&state, "billing@app")[&seventh], CommitTests { state: TestsState::NotRun, at: None }, "without a command a commit is not run");
    crate::sandbox::mark(&state, &slot.path, true).unwrap();
    assert!(commands_for(&state, "billing@app", &slot.path, &["bin/test".to_string()]).is_empty(), "kebbie runs nothing of a sandboxed task's outside its sandbox");
    cleanup(dir, origin, &slot);
}

#[test]
fn a_filtered_run_of_the_tasks_own_never_marks_its_commit_passed() {
    let (dir, origin, slot, _) = task_with_a_commit("billing");
    let state = state_of(&dir);
    let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
    crate::evidence::save(&state, "billing@app", &crate::evidence::Run { command: "cargo test one_test".to_string(), ..run_of("cargo test", &fourth, true) }).unwrap();
    let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let runner = |command: &str, _: &Path| { seen.borrow_mut().push(command.to_string()); false };
    test_all(&state, &dir, &slot.path, "billing@app", &["cargo test".to_string()], None, &runner).unwrap();
    assert_eq!(*seen.borrow(), ["cargo test"], "one test passing says nothing of the commit, so the suite runs on it");
    assert_eq!(results(&state, "billing@app")[&fourth].state, TestsState::Failed);
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

#[test]
fn the_repos_setup_prepares_the_tree_first_and_a_setup_that_fails_leaves_the_commits_not_run() {
    let (dir, origin, slot, _) = task_with_a_commit("billing");
    let state = state_of(&dir);
    let fifth = commit(&slot.path, "e", "fifth");
    let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let runner = |command: &str, _: &Path| {
        seen.borrow_mut().push(command.to_string());
        command != "bin/setup-worktree"
    };
    let said = test_all(&state, &dir, &slot.path, "billing@app", &["cargo test".to_string()], Some("bin/setup-worktree"), &runner).unwrap();
    assert_eq!(said, "billing@app: 0 passed, 0 failed, 2 not run");
    assert_eq!(*seen.borrow(), ["bin/setup-worktree"], "no test runs in a tree its setup did not prepare");
    assert_eq!(results(&state, "billing@app")[&fifth].state, TestsState::NotRun);
    seen.borrow_mut().clear();
    let prepared = |command: &str, _: &Path| {
        seen.borrow_mut().push(command.to_string());
        true
    };
    test_all(&state, &dir, &slot.path, "billing@app", &["cargo test".to_string()], Some("bin/setup-worktree"), &prepared).unwrap();
    assert_eq!(*seen.borrow(), ["bin/setup-worktree", "cargo test", "cargo test"], "the setup runs once, before the first commit, and a commit not run before is run now");
    cleanup(dir, origin, &slot);
}

#[test]
fn left_at_its_default_a_repo_tests_each_commit_only_when_it_names_a_setup() {
    use crate::config::{Layer, Setting};
    let default = Setting { value: true, layer: Layer::Default };
    assert!(!per_commit(&default, false), "a worktree nobody set up would fail every commit");
    assert!(per_commit(&default, true));
    assert!(per_commit(&Setting { value: true, layer: Layer::Repo }, false), "turned on, it runs without a setup");
    assert!(!per_commit(&Setting { value: false, layer: Layer::Machine }, true));
    let dir = crate::sh::test_dir("commit-tests-setup");
    assert_eq!(setup_in(None, &dir), None);
    assert_eq!(setup_in(Some("script/prepare".to_string()), &dir), None, "a path the repo does not have is no setup");
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::write(dir.join("bin/setup-worktree"), "#!/bin/sh\n").unwrap();
    assert_eq!(setup_in(None, &dir).as_deref(), Some("bin/setup-worktree"));
    assert_eq!(setup_in(Some("make prepare".to_string()), &dir).as_deref(), Some("make prepare"));
    std::fs::remove_dir_all(dir).unwrap();
}
