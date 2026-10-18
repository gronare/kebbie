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

fn fetched_at(repo: &Path) -> Option<SystemTime> {
    std::fs::metadata(repo.join(".git/FETCH_HEAD")).and_then(|meta| meta.modified()).ok()
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
                let result = fetch(&repo);
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
