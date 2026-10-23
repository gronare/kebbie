use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

use crate::repo::name_of;

type Outcome = (PathBuf, Result<(), String>);

pub const EVERY: Duration = Duration::from_secs(5 * 60);

// Keeps the remotes of the repos the board shows fresh, so behind-counts move without anyone fetching.
pub struct Fetcher {
    attempted: HashMap<PathBuf, SystemTime>,
    running: HashSet<PathBuf>,
    failed: BTreeMap<PathBuf, String>,
    sender: Sender<Outcome>,
    outcomes: Receiver<Outcome>,
}

// FETCH_HEAD counts fetches by anyone (another board, a task landing); the attempt counts this board's failures, which leave it untouched.
pub fn due(fetched: Option<SystemTime>, attempted: Option<SystemTime>, running: bool, now: SystemTime) -> bool {
    let older = |at: Option<SystemTime>| at.is_none_or(|at| now.duration_since(at).unwrap_or_default() >= EVERY);
    !running && older(fetched) && older(attempted)
}

pub fn failed_note(names: &[String]) -> String {
    if names.is_empty() { String::new() } else { format!("fetch failed: {}   ", names.join(", ")) }
}

pub fn fetched_at(repo: &Path) -> Option<SystemTime> {
    std::fs::metadata(repo.join(".git/FETCH_HEAD")).and_then(|meta| meta.modified()).ok()
}

// Every worktree of a repo writes the same refs/remotes/origin/<branch>, so two fetches at once fail on its lock.
pub fn ref_lock_error(error: &str) -> bool {
    error.contains("cannot lock ref") || error.contains("reference already exists") || (error.contains("Unable to create") && error.contains(".lock"))
}

pub const RETRIES: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(4)];

pub fn retrying(mut attempt: impl FnMut() -> Result<(), String>, mut pause: impl FnMut(Duration)) -> Result<(), String> {
    for wait in RETRIES {
        match attempt() {
            Err(error) if ref_lock_error(&error) => pause(wait),
            other => return other,
        }
    }
    attempt()
}

const LOCK_STALE: Duration = Duration::from_secs(300);

fn lock_file(state: &Path, repo: &Path) -> PathBuf {
    let name: String = repo.to_string_lossy().chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '_' }).collect();
    state.join("fetch").join(format!("{name}.lock"))
}

struct FetchLock(PathBuf);

impl FetchLock {
    fn take(path: PathBuf, wait: Duration) -> Option<FetchLock> {
        std::fs::create_dir_all(path.parent()?).ok()?;
        let started = SystemTime::now();
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Some(FetchLock(path)),
                Err(_) if std::fs::metadata(&path).and_then(|meta| meta.modified()).is_ok_and(|at| at.elapsed().unwrap_or_default() > LOCK_STALE) => {
                    let _ = std::fs::remove_file(&path);
                }
                Err(_) if started.elapsed().unwrap_or_default() > wait => return None,
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        }
    }
}

impl Drop for FetchLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// One fetch per repo at a time across kelpie's processes; one that landed while this one waited for the lock (FETCH_HEAD changed meanwhile, never FETCH_HEAD against a clock a time sync can step back) is shared, and a ref lock taken by a git outside kelpie is waited out.
pub fn shared_in(state: &Path, repo: &Path, attempt: impl FnMut() -> Result<(), String>, pause: impl FnMut(Duration)) -> Result<(), String> {
    let before = fetched_at(repo);
    let _lock = FetchLock::take(lock_file(state, repo), Duration::from_secs(120));
    if fetched_at(repo).is_some_and(|at| before.is_none_or(|before| at > before)) {
        return Ok(());
    }
    retrying(attempt, pause)
}

pub fn shared(repo: &Path, attempt: impl FnMut() -> Result<(), String>) -> Result<(), String> {
    match crate::lead::state_dir() {
        Ok(state) => shared_in(&state, repo, attempt, thread::sleep),
        Err(_) => retrying(attempt, thread::sleep),
    }
}

fn fetch(repo: &Path) -> Result<(), String> {
    let mut command = Command::new("git");
    command.args(["fetch", "--quiet", "--no-tags"]).current_dir(repo).stdin(Stdio::null()).stdout(Stdio::null()).env("GIT_TERMINAL_PROMPT", "0");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let output = command.output().map_err(|error| format!("git fetch: {error}"))?;
    if output.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&output.stderr).trim().to_string()) }
}

impl Fetcher {
    pub fn new() -> Self {
        let (sender, outcomes) = mpsc::channel();
        Self { attempted: HashMap::new(), running: HashSet::new(), failed: BTreeMap::new(), sender, outcomes }
    }

    // Collects finished fetches and starts the due ones; true when one landed, so the board reads git again.
    pub fn tick(&mut self, repos: impl IntoIterator<Item = PathBuf>) -> bool {
        let mut landed = false;
        while let Ok((repo, result)) = self.outcomes.try_recv() {
            self.running.remove(&repo);
            match result {
                Ok(()) => {
                    self.failed.remove(&repo);
                    landed = true;
                }
                Err(error) => {
                    self.failed.insert(repo, error);
                }
            }
        }
        let now = SystemTime::now();
        for repo in repos {
            if !due(fetched_at(&repo), self.attempted.get(&repo).copied(), self.running.contains(&repo), now) {
                continue;
            }
            self.attempted.insert(repo.clone(), now);
            self.running.insert(repo.clone());
            let sender = self.sender.clone();
            thread::spawn(move || {
                let result = shared(&repo, || fetch(&repo));
                let _ = sender.send((repo, result));
            });
        }
        landed
    }

    pub fn failures(&self) -> Vec<String> {
        self.failed.keys().map(|repo| name_of(repo)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repo_is_fetched_once_per_interval_and_never_twice_at_once() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let ago = |seconds| Some(now - Duration::from_secs(seconds));
        assert!(due(None, None, false, now));
        assert!(!due(None, None, true, now));
        assert!(!due(ago(60), None, false, now));
        assert!(due(ago(300), ago(300), false, now));
        assert!(!due(ago(600), ago(60), false, now), "a failed fetch leaves FETCH_HEAD old; the attempt still waits out the interval");
        assert!(!due(ago(600), ago(600), true, now));
    }

    #[test]
    fn a_fetch_that_meets_a_ref_lock_tries_again_after_two_and_four_seconds_and_nothing_else_is_retried() {
        let locked = "error: cannot lock ref 'refs/remotes/origin/main': is at 1a2b but expected 3c4d";
        assert!(ref_lock_error(locked) && ref_lock_error("error: cannot lock ref 'refs/remotes/origin/main': reference already exists"));
        assert!(ref_lock_error("fatal: Unable to create '/r/.git/refs/remotes/origin/main.lock': File exists."));
        assert!(!ref_lock_error("fatal: Could not read from remote repository."));
        let mut tries = 0;
        let mut waits = Vec::new();
        let result = retrying(|| { tries += 1; if tries < 3 { Err(locked.to_string()) } else { Ok(()) } }, |wait| waits.push(wait));
        assert_eq!((result, tries, waits.clone()), (Ok(()), 3, RETRIES.to_vec()));
        let mut tries = 0;
        assert!(retrying(|| { tries += 1; Err(locked.to_string()) }, |_| {}).is_err());
        assert_eq!(tries, 3, "the third failure is the answer");
        let mut tries = 0;
        assert!(retrying(|| { tries += 1; Err("fatal: Could not read from remote repository.".to_string()) }, |_| {}).is_err());
        assert_eq!(tries, 1);
    }

    #[test]
    fn fetches_of_one_repo_from_two_threads_never_overlap_and_a_fresh_one_is_shared() {
        let state = crate::sh::test_dir("fetch-lock");
        let repo = state.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let running = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let most = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let threads: Vec<_> = (0..4).map(|_| {
            let (state, repo, running, most) = (state.clone(), repo.clone(), running.clone(), most.clone());
            thread::spawn(move || shared_in(&state, &repo, || {
                let now = running.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                most.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                thread::sleep(Duration::from_millis(50));
                running.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }, |_| {}))
        }).collect();
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        assert_eq!(most.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!lock_file(&state, &repo).exists(), "the lock goes with its fetch");
        std::fs::write(repo.join(".git/FETCH_HEAD"), "").unwrap();
        let mut called = false;
        shared_in(&state, &repo, || { called = true; Ok(()) }, |_| {}).unwrap();
        assert!(called, "a fetch from before this one was asked for is not new enough to share");
        let held = FetchLock::take(lock_file(&state, &repo), Duration::from_secs(1)).unwrap();
        let waiting = { let (state, repo) = (state.clone(), repo.clone()); thread::spawn(move || { let mut called = false; shared_in(&state, &repo, || { called = true; Ok(()) }, |_| {}).unwrap(); called }) };
        thread::sleep(Duration::from_millis(300));
        std::fs::write(repo.join(".git/FETCH_HEAD"), "").unwrap();
        drop(held);
        assert!(!waiting.join().unwrap(), "a fetch that landed while it waited for the lock is shared, not repeated");
        std::fs::File::options().write(true).open(repo.join(".git/FETCH_HEAD")).unwrap().set_modified(SystemTime::now() + Duration::from_secs(1)).unwrap();
        let mut called = false;
        shared_in(&state, &repo, || { called = true; Ok(()) }, |_| {}).unwrap();
        assert!(called, "a fetch from before the clock stepped back is not shared either");
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn failures_are_one_quiet_line_of_repo_names() {
        assert_eq!(failed_note(&[]), "");
        assert_eq!(failed_note(&["kelpie".to_string(), "web".to_string()]), "fetch failed: kelpie, web   ");
    }

    #[test]
    fn a_fetch_failure_is_reported_and_cleared_by_the_next_success() {
        let dir = std::env::temp_dir().join(format!("kelpie-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (origin, clone) = (dir.join("origin"), dir.join("clone"));
        let git = |cwd: &Path, args: &[&str]| assert!(Command::new("git").args(args).current_dir(cwd).output().unwrap().status.success(), "{args:?}");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "--quiet", "--bare"]);
        git(&dir, &["clone", "--quiet", origin.to_str().unwrap(), "clone"]);
        git(&clone, &["remote", "set-url", "origin", dir.join("missing").to_str().unwrap()]);
        let mut fetcher = Fetcher::new();
        let wait = |fetcher: &mut Fetcher| {
            for _ in 0..200 {
                if fetcher.running.is_empty() {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
                fetcher.tick(Vec::new());
            }
        };
        fetcher.tick(vec![clone.clone(), clone.clone()]);
        assert_eq!(fetcher.running.len(), 1);
        wait(&mut fetcher);
        assert_eq!(fetcher.failures(), vec!["clone".to_string()]);
        git(&clone, &["remote", "set-url", "origin", origin.to_str().unwrap()]);
        fetcher.attempted.clear();
        let _ = std::fs::remove_file(clone.join(".git/FETCH_HEAD"));
        fetcher.tick(vec![clone.clone()]);
        wait(&mut fetcher);
        assert!(fetcher.failures().is_empty(), "{:?}", fetcher.failed);
        assert!(!due(fetched_at(&clone), None, false, SystemTime::now()), "a landed fetch stamps FETCH_HEAD");
        let _ = std::fs::remove_dir_all(dir);
    }
}
