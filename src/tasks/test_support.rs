use std::path::{Path, PathBuf};

use crate::repo::git;
use super::pool::{Slot, create_slot, find_task};

pub(crate) fn repo() -> PathBuf {
    repo_named("")
}

fn repo_named(suffix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kelpie-test-{}-{:?}{suffix}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "--quiet"]).unwrap();
    git(&dir, &["config", "user.name", "Ada"]).unwrap();
    git(&dir, &["config", "user.email", "ada@example.com"]).unwrap();
    for (file, message) in [("a", "first"), ("b", "second"), ("c", "third")] {
        std::fs::write(dir.join(file), file).unwrap();
        git(&dir, &["add", file]).unwrap();
        git(&dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", message]).unwrap();
    }
    dir
}

pub(crate) fn task_with_a_commit(name: &str) -> (PathBuf, PathBuf, Slot, String) {
    task_in(repo(), name)
}

// The same, in a second repo of its own, for what two repos do side by side.
pub(crate) fn task_in_another_repo(name: &str) -> (PathBuf, PathBuf, Slot, String) {
    task_in(repo_named("-other"), name)
}

fn task_in(dir: PathBuf, name: &str) -> (PathBuf, PathBuf, Slot, String) {
    let origin = dir.with_extension("origin");
    let _ = std::fs::remove_dir_all(&origin);
    git(&dir, &["clone", "--quiet", "--bare", ".", &origin.to_string_lossy()]).unwrap();
    git(&dir, &["remote", "add", "origin", &origin.to_string_lossy()]).unwrap();
    git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
    let branch = git(&dir, &["branch", "--show-current"]).unwrap();
    git(&dir, &["symbolic-ref", "refs/remotes/origin/HEAD", &format!("refs/remotes/origin/{branch}")]).unwrap();
    let slot = create_slot(&dir, "HEAD").unwrap();
    git(&slot.path, &["switch", "--quiet", "--create", name]).unwrap();
    std::fs::write(slot.path.join("d"), "d").unwrap();
    git(&slot.path, &["add", "d"]).unwrap();
    git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", "fourth"]).unwrap();
    let slot = find_task(&dir, Some(name), &dir).unwrap();
    (dir, origin, slot, format!("origin/{branch}"))
}

// Another task in the same repo, its own worktree with one commit of its own on `file`.
pub(crate) fn another_task(dir: &Path, name: &str, file: &str) -> Slot {
    let slot = create_slot(dir, "HEAD").unwrap();
    git(&slot.path, &["switch", "--quiet", "--create", name]).unwrap();
    commit(&slot.path, file, name);
    find_task(dir, Some(name), dir).unwrap()
}

pub(crate) fn state_of(dir: &Path) -> PathBuf {
    let state = dir.with_extension("state");
    std::fs::create_dir_all(&state).unwrap();
    state
}

pub(crate) fn cleanup(dir: PathBuf, origin: PathBuf, slot: &Slot) {
    let _ = std::fs::remove_dir_all(dir.with_extension("state"));
    git(&dir, &["worktree", "remove", "--force", &slot.path.to_string_lossy()]).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(origin).unwrap();
}

pub(crate) fn commit(path: &Path, file: &str, message: &str) -> String {
    std::fs::write(path.join(file), message).unwrap();
    git(path, &["add", file]).unwrap();
    git(path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", message]).unwrap();
    git(path, &["rev-parse", "HEAD"]).unwrap()
}

// Main moves on first, so the landed commits cannot come out with the task's own shas.
pub(crate) fn land_on_main(dir: &Path, upstream: &str, land: impl Fn(&Path)) {
    commit(dir, "elsewhere", "someone else's work");
    land(dir);
    git(dir, &["update-ref", &format!("refs/remotes/{upstream}"), "HEAD"]).unwrap();
}

pub(crate) fn pick(dir: &Path, sha: &str) {
    git(dir, &["-c", "user.name=Bo", "-c", "user.email=bo@example.com", "cherry-pick", sha]).unwrap();
}

pub(crate) fn upstream_commit(dir: &Path, file: &str, text: &str, message: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]).unwrap();
    git(dir, &["-c", "user.name=Bo", "-c", "user.email=bo@example.com", "commit", "--quiet", "-m", message]).unwrap();
    git(dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
    git(dir, &["fetch", "--quiet", "origin"]).unwrap();
}

pub(crate) fn cleanup_intents(state: &Path, address: &str) {
    super::intent::forget(state, address);
}
