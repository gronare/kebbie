use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::messages;
use crate::notify::{self, Event as NotifyEvent, Notice};
use crate::sh::Result;

pub const POLL: Duration = Duration::from_secs(30);
pub const APPEAR_SECONDS: u64 = 180;
pub const SETTLE_SECONDS: u64 = 60;
pub const LOG_LINES: usize = 60;
pub const GH_VAR: &str = "KELPIE_GH_BIN";
const PASSED: [&str; 3] = ["success", "skipped", "neutral"];

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub id: u64,
    pub name: String,
    pub workflow: String,
    pub status: String,
    pub conclusion: String,
    pub url: String,
}

impl Run {
    pub fn title(&self) -> &str {
        if self.workflow.is_empty() { &self.name } else { &self.workflow }
    }

    fn done(&self) -> bool {
        self.status == "completed"
    }

    fn failed(&self) -> bool {
        self.done() && !PASSED.contains(&self.conclusion.as_str())
    }

    fn to_json(&self) -> Value {
        json!({ "id": self.id, "name": self.name, "workflow": self.workflow, "status": self.status, "conclusion": self.conclusion, "url": self.url })
    }

    fn from_json(value: &Value) -> Option<Run> {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
        Some(Run { id: value["id"].as_u64()?, name: text("name"), workflow: text("workflow"), status: text("status"), conclusion: text("conclusion"), url: text("url") })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Running,
    Failed { at: u64, what: String, url: String },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pushed {
    pub pusher: String,
    pub target: String,
    pub commits: String,
    pub note: Option<String>,
    pub logged: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Watch {
    pub address: String,
    pub task: String,
    pub repo: PathBuf,
    pub slug: String,
    pub sha: String,
    pub started_at: u64,
    pub checked_at: Option<u64>,
    pub close: bool,
    pub runs: Vec<Run>,
    pub state: State,
    pub error: Option<String>,
    pub pushed: Pushed,
}

impl Watch {
    pub fn short(&self) -> &str {
        &self.sha[..7.min(self.sha.len())]
    }

    // When the watch last changed what the task needs, so a report the task wrote later outranks it.
    pub fn since(&self) -> u64 {
        match &self.state {
            State::Running => self.started_at,
            State::Failed { at, .. } => *at,
        }
    }

    pub fn running(&self) -> bool {
        self.state == State::Running
    }

    pub fn note(&self) -> String {
        match &self.state {
            State::Failed { what, .. } => format!("CI failed: {what}"),
            State::Running if self.runs.is_empty() => format!("CI running: waiting for runs of {}", self.short()),
            State::Running => format!("CI running: {}", self.runs.iter().map(Run::title).collect::<Vec<_>>().join(", ")),
        }
    }

    pub fn to_json(&self) -> Value {
        let (state, failed_at, failure, failure_url) = match &self.state {
            State::Running => ("running", None, None, None),
            State::Failed { at, what, url } => ("failed", Some(*at), Some(what.clone()), Some(url.clone())),
        };
        json!({
            "address": self.address, "task": self.task, "repo": self.repo, "slug": self.slug, "sha": self.sha,
            "started_at": self.started_at, "checked_at": self.checked_at, "close": self.close,
            "runs": self.runs.iter().map(Run::to_json).collect::<Vec<_>>(),
            "state": state, "failed_at": failed_at, "failure": failure, "failure_url": failure_url, "error": self.error,
            "pushed": { "pusher": self.pushed.pusher, "target": self.pushed.target, "commits": self.pushed.commits, "note": self.pushed.note, "logged": self.pushed.logged },
        })
    }

    pub fn from_json(value: &Value) -> Option<Watch> {
        let text = |key: &str| value[key].as_str().map(str::to_string);
        let state = match value["state"].as_str()? {
            "running" => State::Running,
            "failed" => State::Failed { at: value["failed_at"].as_u64()?, what: text("failure").unwrap_or_default(), url: text("failure_url").unwrap_or_default() },
            _ => return None,
        };
        let pushed = &value["pushed"];
        let pushed_text = |key: &str| pushed[key].as_str().unwrap_or_default().to_string();
        Some(Watch {
            address: text("address")?, task: text("task")?, repo: PathBuf::from(text("repo")?), slug: text("slug")?, sha: text("sha")?,
            started_at: value["started_at"].as_u64()?, checked_at: value["checked_at"].as_u64(), close: value["close"].as_bool().unwrap_or(true),
            runs: value["runs"].as_array().map(|runs| runs.iter().filter_map(Run::from_json).collect()).unwrap_or_default(),
            state, error: text("error"),
            pushed: Pushed { pusher: pushed_text("pusher"), target: pushed_text("target"), commits: pushed_text("commits"), note: pushed["note"].as_str().map(str::to_string), logged: pushed["logged"].as_bool() },
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Pending,
    Green,
    NoneRan,
    Failed(Vec<Run>),
}

// A run that shows up late must not miss a green, so green waits SETTLE_SECONDS after the push, and no runs at all waits APPEAR_SECONDS.
pub fn verdict(runs: &[Run], waited: u64) -> Verdict {
    let failed: Vec<Run> = runs.iter().filter(|run| run.failed()).cloned().collect();
    if !failed.is_empty() {
        return Verdict::Failed(failed);
    }
    if runs.is_empty() {
        return if waited >= APPEAR_SECONDS { Verdict::NoneRan } else { Verdict::Pending };
    }
    if runs.iter().all(Run::done) && waited >= SETTLE_SECONDS { Verdict::Green } else { Verdict::Pending }
}

pub fn parse_runs(text: &str) -> Result<Vec<Run>> {
    let value: Value = serde_json::from_str(text).map_err(|error| format!("gh run list gave no JSON: {error}"))?;
    let runs = value.as_array().ok_or("gh run list gave no list")?;
    Ok(runs.iter().filter_map(|run| {
        let text = |key: &str| run[key].as_str().unwrap_or_default().to_string();
        Some(Run { id: run["databaseId"].as_u64()?, name: text("name"), workflow: text("workflowName"), status: text("status"), conclusion: text("conclusion"), url: text("url") })
    }).collect())
}

pub fn failed_job(text: &str) -> Option<(String, Option<u64>)> {
    let value: Value = serde_json::from_str(text).ok()?;
    let job = value["jobs"].as_array()?.iter().find(|job| job["status"] == "completed" && !PASSED.contains(&job["conclusion"].as_str().unwrap_or_default()))?;
    Some((job["name"].as_str().unwrap_or("a job").to_string(), job["databaseId"].as_u64()))
}

// gh prints each line as job, step and text separated by tabs; the text alone is kept, without escapes a terminal would act on.
pub fn log_tail(log: &str, count: usize) -> String {
    let lines: Vec<String> = log.lines().map(|line| {
        let text = match line.splitn(3, '\t').collect::<Vec<_>>().as_slice() {
            [_, _, text] => *text,
            _ => line,
        };
        strip_escapes(text).trim_end().to_string()
    }).collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

fn strip_escapes(text: &str) -> String {
    let mut out = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            if characters.peek() == Some(&'[') {
                characters.next();
                for next in characters.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if character == '\t' || !character.is_control() {
            out.push(character);
        }
    }
    out
}

pub fn github_repo(url: &str, hosts: &[String]) -> Option<String> {
    let (rest, scp) = match url.split_once("://") {
        Some((_, rest)) => (rest, false),
        None => (url, true),
    };
    let cut = rest.find(if scp { ':' } else { '/' })?;
    let (authority, path) = (&rest[..cut], &rest[cut + 1..]);
    let host = authority.rsplit('@').next()?;
    let host = if scp { host } else { host.split(':').next()? };
    if !hosts.iter().any(|known| known.eq_ignore_ascii_case(host)) {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 2 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    Some(if host.eq_ignore_ascii_case("github.com") { path.to_string() } else { format!("{host}/{path}") })
}

pub fn gh() -> String {
    std::env::var(GH_VAR).ok().filter(|path| !path.is_empty()).unwrap_or_else(|| "gh".to_string())
}

fn hosts() -> Vec<String> {
    let mut hosts = vec!["github.com".to_string()];
    hosts.extend(std::env::var("GH_HOST").ok().filter(|host| !host.is_empty()));
    hosts
}

// The repo gh watches, or why CI is not watched; the raw remote URL, so an insteadOf rewrite never hides GitHub.
pub fn target(repo: &Path, gh: &str) -> std::result::Result<String, String> {
    let url = crate::repo::git(repo, &["config", "--get", "remote.origin.url"]).map_err(|_| "origin has no URL".to_string())?;
    let slug = github_repo(&url, &hosts()).ok_or("origin is not on GitHub")?;
    let host = if slug.matches('/').count() == 2 { slug.split('/').next().unwrap_or("github.com").to_string() } else { "github.com".to_string() };
    match crate::sh::attempt(gh, &["auth", "status", "--hostname", &host], Some(repo)) {
        Err(_) => Err("gh is not installed".to_string()),
        Ok(Err(_)) => Err(format!("gh is not logged in to {host} (gh auth login)")),
        Ok(Ok(_)) => Ok(slug),
    }
}

fn dir(state: &Path) -> PathBuf {
    state.join("ci")
}

fn file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.json"))
}

pub fn save(state: &Path, watch: &Watch) -> Result<()> {
    crate::state::write(&file(state, &watch.address), serde_json::to_string_pretty(&watch.to_json()).unwrap_or_default())
}

pub fn load(state: &Path, address: &str) -> Option<Watch> {
    Watch::from_json(&serde_json::from_str(&fs::read_to_string(file(state, address)).ok()?).ok()?)
}

pub fn forget(state: &Path, address: &str) {
    let _ = fs::remove_file(file(state, address));
}

pub fn keep_watching(state: &Path, address: &str) {
    if let Some(watch) = load(state, address) {
        let _ = save(state, &Watch { close: false, ..watch });
    }
}

pub fn all(state: &Path) -> Vec<Watch> {
    let Ok(entries) = fs::read_dir(dir(state)) else { return Vec::new() };
    let mut watches: Vec<Watch> = entries.filter_map(|entry| {
        let path = entry.ok()?.path();
        let name = path.file_name()?.to_string_lossy().to_string();
        let address = name.strip_suffix(".json").filter(|_| !name.starts_with('.'))?;
        load(state, address)
    }).collect();
    watches.sort_by(|one, other| one.address.cmp(&other.address));
    watches
}

// Cheap enough for the board to ask every round: no gh call unless a watch is running.
pub fn active(state: &Path) -> bool {
    all(state).iter().any(Watch::running)
}

pub enum Begin {
    Watching,
    NotWatched(String),
    Off,
}

pub fn begin(state: &Path, repo: &Path, address: &str, sha: &str, close: bool, pushed: Pushed) -> Begin {
    if !crate::config::resolve(Some(repo)).map(|loaded| loaded.settings.ci_watch.value).unwrap_or(true) {
        return Begin::Off;
    }
    let slug = match target(repo, &gh()) {
        Ok(slug) => slug,
        Err(reason) => return Begin::NotWatched(reason),
    };
    let task = address.split('@').next().unwrap_or(address).to_string();
    let watch = Watch { address: address.to_string(), task, repo: repo.to_path_buf(), slug, sha: sha.to_string(), started_at: messages::now(), checked_at: None, close, runs: Vec::new(), state: State::Running, error: None, pushed };
    match save(state, &watch) {
        Ok(()) => Begin::Watching,
        Err(error) => Begin::NotWatched(format!("its record could not be saved ({error})")),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Green,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub address: String,
    pub kind: Kind,
    pub text: String,
}

// Held while a round runs, so a board and a keeper never finish the same task twice.
struct Lock {
    _file: fs::File,
}

impl Drop for Lock {
    // A child forked by another thread holds a copy of the descriptor until it execs; unlocking releases the lock for every copy at once.
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
    }
}

impl Lock {
    fn take(state: &Path) -> Option<Lock> {
        let file = crate::state::append_to(&dir(state).join(".lock")).ok()?;
        // SAFETY: flock on a descriptor this File owns.
        (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(Lock { _file: file })
    }
}

pub fn tick(state: &Path) -> Vec<Event> {
    tick_with(state, &gh(), messages::now())
}

pub fn tick_with(state: &Path, gh: &str, now: u64) -> Vec<Event> {
    let Some(_lock) = Lock::take(state) else { return Vec::new() };
    let mut events = Vec::new();
    for mut watch in all(state).into_iter().filter(Watch::running) {
        if watch.close && crate::tasks::find_task(&watch.repo, Some(&watch.task), &watch.repo).is_err() {
            forget(state, &watch.address);
            continue;
        }
        let listed = crate::sh::run(gh, &["run", "list", "-R", &watch.slug, "--commit", &watch.sha, "--limit", "50", "--json", "databaseId,name,status,conclusion,workflowName,url"], Some(&watch.repo)).and_then(|text| parse_runs(&text));
        watch.checked_at = Some(now);
        match listed {
            Ok(runs) => {
                watch.runs = runs;
                watch.error = None;
            }
            Err(error) => {
                watch.error = Some(error);
                let _ = save(state, &watch);
                continue;
            }
        }
        match verdict(&watch.runs, now.saturating_sub(watch.started_at)) {
            Verdict::Pending => {
                let _ = save(state, &watch);
            }
            Verdict::Green => events.push(green(state, &watch, false)),
            Verdict::NoneRan => events.push(green(state, &watch, true)),
            Verdict::Failed(failed) => events.push(failed_run(state, gh, &mut watch, &failed[0], now)),
        }
    }
    events
}

fn green(state: &Path, watch: &Watch, none_ran: bool) -> Event {
    forget(state, &watch.address);
    let what = if none_ran { format!("No CI ran for {}", watch.short()) } else { format!("CI is green for {}", watch.short()) };
    let text = if watch.close {
        match crate::tasks::after_ci(&watch.repo, &watch.task, &watch.address, &watch.pushed) {
            Ok(said) => format!("{what}. {said}"),
            Err(error) => format!("{what}, but {} could not be closed: {error}", watch.task),
        }
    } else {
        let text = format!("{what}, the commits pushed from {}.", watch.address);
        if let Err(error) = messages::notify_lead(&text) {
            eprintln!("Could not tell the lead: {error}");
        }
        text
    };
    notify::send(&watch.repo, &Notice::from(NotifyEvent::CiGreen, &watch.address, format!("{}: {text}", watch.address)));
    Event { address: watch.address.clone(), kind: Kind::Green, text }
}

pub fn task_message(watch: &Watch, run: &Run, job: &str, tail: &str) -> String {
    let log = if tail.trim().is_empty() { "gh showed no log for it.".to_string() } else { format!("The last lines of its log:\n{tail}") };
    format!(
        "CI failed for {} on {}: {} · {job} ({}). {log}\nFind the cause, fix it with a new commit and report ready; the next push is watched the same way.",
        watch.short(), if watch.pushed.target.is_empty() { "the default branch" } else { &watch.pushed.target }, run.title(), run.url
    )
}

fn failed_run(state: &Path, gh: &str, watch: &mut Watch, run: &Run, now: u64) -> Event {
    let id = run.id.to_string();
    let jobs = crate::sh::run(gh, &["run", "view", &id, "-R", &watch.slug, "--json", "jobs"], Some(&watch.repo)).ok();
    let (job, job_id) = jobs.as_deref().and_then(failed_job).unwrap_or_else(|| (run.name.clone(), None));
    let log = match job_id {
        Some(job_id) => crate::sh::run(gh, &["run", "view", "-R", &watch.slug, "--job", &job_id.to_string(), "--log-failed"], Some(&watch.repo)),
        None => crate::sh::run(gh, &["run", "view", &id, "-R", &watch.slug, "--log-failed"], Some(&watch.repo)),
    }.unwrap_or_default();
    let what = format!("{} · {job}", run.title());
    watch.state = State::Failed { at: now, what: what.clone(), url: run.url.clone() };
    let _ = save(state, watch);
    if let Err(error) = messages::notify(&watch.address, &task_message(watch, run, &job, &log_tail(&log, LOG_LINES))) {
        eprintln!("Could not tell {} that CI failed: {error}", watch.address);
    }
    let text = format!("CI failed for {} ({}): {what}. kelpie sent the task the failing log; it stays open under Needs you. {}", watch.address, watch.short(), run.url);
    if let Err(error) = messages::tell_lead(&text, Some("ci")) {
        eprintln!("Could not tell the lead that CI failed: {error}");
    }
    notify::send(&watch.repo, &Notice::from(NotifyEvent::CiFailed, &watch.address, format!("{}: CI failed: {what}", watch.address)));
    Event { address: watch.address.clone(), kind: Kind::Failed, text }
}

pub fn list(state: &Path, as_json: bool) -> String {
    let watches = all(state);
    if as_json {
        return serde_json::to_string_pretty(&watches.iter().map(Watch::to_json).collect::<Vec<_>>()).unwrap_or_default();
    }
    if watches.is_empty() {
        return "No CI watches.".to_string();
    }
    watches.iter().map(|watch| {
        let mut lines = vec![format!("{}  {} on {}  {}", watch.address, watch.short(), watch.slug, watch.note())];
        lines.extend(watch.runs.iter().map(|run| format!("  {}  {}{}  {}", run.title(), run.status, if run.conclusion.is_empty() { String::new() } else { format!(" · {}", run.conclusion) }, run.url)));
        if let Some(error) = &watch.error {
            lines.push(format!("  last check failed: {error}"));
        }
        lines.join("\n")
    }).collect::<Vec<_>>().join("\n")
}

pub fn tick_cli() -> Result<String> {
    let state = crate::lead::state_dir()?;
    if !active(&state) {
        return Ok("No CI watch is running.".to_string());
    }
    let events = tick(&state);
    if events.is_empty() {
        return Ok(list(&state, false));
    }
    Ok(events.iter().map(|event| event.text.clone()).collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(status: &str, conclusion: &str) -> Run {
        Run { id: 1, name: "CI".to_string(), workflow: "CI".to_string(), status: status.to_string(), conclusion: conclusion.to_string(), url: "https://github.com/acme/app/actions/runs/1".to_string() }
    }

    #[test]
    fn runs_add_up_to_pending_green_failed_or_none_after_the_wait() {
        assert_eq!(verdict(&[], 10), Verdict::Pending, "runs show up a few seconds after the push");
        assert_eq!(verdict(&[], APPEAR_SECONDS), Verdict::NoneRan);
        assert_eq!(verdict(&[run("in_progress", "")], 600), Verdict::Pending);
        assert_eq!(verdict(&[run("completed", "success"), run("queued", "")], 600), Verdict::Pending);
        assert_eq!(verdict(&[run("completed", "success"), run("completed", "skipped"), run("completed", "neutral")], SETTLE_SECONDS), Verdict::Green);
        assert_eq!(verdict(&[run("completed", "success")], SETTLE_SECONDS - 1), Verdict::Pending, "a late workflow still has time to appear");
        for conclusion in ["failure", "cancelled", "timed_out", "startup_failure", "action_required"] {
            assert!(matches!(verdict(&[run("completed", "success"), run("completed", conclusion), run("in_progress", "")], 5), Verdict::Failed(failed) if failed.len() == 1), "{conclusion}");
        }
    }

    #[test]
    fn a_watch_round_trips_through_its_state_file() {
        let state = crate::sh::test_dir("ci-file");
        let mut watch = Watch {
            address: "billing@app".to_string(), task: "billing".to_string(), repo: PathBuf::from("/src/app"), slug: "acme/app".to_string(), sha: "abcdef0123".to_string(),
            started_at: 100, checked_at: Some(130), close: true, runs: vec![run("in_progress", "")], state: State::Running, error: None,
            pushed: Pushed { pusher: "the user".to_string(), target: "main".to_string(), commits: "abc Add refunds".to_string(), note: Some("Projects/x.md".to_string()), logged: Some(true) },
        };
        save(&state, &watch).unwrap();
        assert_eq!(load(&state, "billing@app").as_ref(), Some(&watch));
        assert!(active(&state));
        watch.state = State::Failed { at: 200, what: "CI · test".to_string(), url: "u".to_string() };
        watch.error = Some("rate limited".to_string());
        save(&state, &watch).unwrap();
        assert_eq!(all(&state), [watch.clone()]);
        assert!(!active(&state), "a failed watch is not polled");
        assert_eq!(watch.since(), 200);
        assert_eq!(watch.note(), "CI failed: CI · test");
        fs::write(file(&state, "broken@app"), "not json").unwrap();
        assert_eq!(all(&state).len(), 1, "a broken record is skipped");
        forget(&state, "billing@app");
        assert!(load(&state, "billing@app").is_none());
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn origin_is_on_github_in_every_url_spelling_and_nowhere_else() {
        let hosts = vec!["github.com".to_string(), "git.acme.dev".to_string()];
        for url in ["https://github.com/acme/app.git", "https://github.com/acme/app", "https://ada@github.com/acme/app.git", "git@github.com:acme/app.git", "ssh://git@github.com/acme/app.git", "ssh://git@github.com:22/acme/app", "git://github.com/acme/app/"] {
            assert_eq!(github_repo(url, &hosts).as_deref(), Some("acme/app"), "{url}");
        }
        assert_eq!(github_repo("git@git.acme.dev:acme/app.git", &hosts).as_deref(), Some("git.acme.dev/acme/app"), "GH_HOST's repos name their host");
        for url in ["https://gitlab.com/acme/app.git", "/srv/origin.git", "../origin.git", "https://github.com/acme", "https://github.com/acme/app/extra", "git@gitlab.com:acme/app.git", "https://github.com.evil.example/acme/app"] {
            assert_eq!(github_repo(url, &hosts), None, "{url}");
        }
    }

    #[test]
    fn the_failure_names_the_job_and_gives_the_last_lines_of_its_log_without_escapes() {
        let jobs = r#"{"jobs":[{"name":"lint","status":"completed","conclusion":"success","databaseId":7},{"name":"test","status":"completed","conclusion":"failure","databaseId":8},{"name":"deploy","status":"completed","conclusion":"skipped","databaseId":9}]}"#;
        assert_eq!(failed_job(jobs), Some(("test".to_string(), Some(8))));
        assert_eq!(failed_job(r#"{"jobs":[]}"#), None);
        let log: String = (1..=100).map(|line| format!("test\tRun tests\t2026-10-20T10:00:00Z \u{1b}[31mline {line}\u{1b}[0m\u{7}\n")).collect();
        let tail = log_tail(&log, LOG_LINES);
        assert_eq!(tail.lines().count(), LOG_LINES);
        assert!(tail.starts_with("2026-10-20T10:00:00Z line 41") && tail.ends_with("line 100"), "{tail}");
        assert!(!tail.contains('\u{1b}') && !tail.contains('\u{7}') && !tail.contains("Run tests"));
        let watch = Watch { address: "billing@app".to_string(), task: "billing".to_string(), repo: PathBuf::new(), slug: "acme/app".to_string(), sha: "abcdef0123".to_string(), started_at: 0, checked_at: None, close: true, runs: Vec::new(), state: State::Running, error: None, pushed: Pushed { target: "main".to_string(), ..Pushed::default() } };
        let told = task_message(&watch, &run("completed", "failure"), "test", &tail);
        assert!(told.starts_with("CI failed for abcdef0 on main: CI · test (https://github.com/acme/app/actions/runs/1)."), "{told}");
        assert!(told.contains("line 100") && !told.contains("line 40\n"), "{told}");
        assert!(task_message(&watch, &run("completed", "failure"), "test", "").contains("gh showed no log"));
    }

    #[test]
    fn gh_run_list_is_read_by_its_fields() {
        let runs = parse_runs(r#"[{"databaseId":42,"name":"CI","status":"in_progress","conclusion":"","workflowName":"Build","url":"https://x/42"},{"name":"no id"}]"#).unwrap();
        assert_eq!(runs, [Run { id: 42, name: "CI".to_string(), workflow: "Build".to_string(), status: "in_progress".to_string(), conclusion: String::new(), url: "https://x/42".to_string() }]);
        assert_eq!(runs[0].title(), "Build");
        assert!(parse_runs("gh: not found").is_err());
    }

    #[test]
    fn one_round_at_a_time_holds_the_lock() {
        let state = crate::sh::test_dir("ci-lock");
        let held = Lock::take(&state).unwrap();
        assert!(Lock::take(&state).is_none(), "a second board or a keeper skips the round");
        drop(held);
        assert!(Lock::take(&state).is_some());
        fs::remove_dir_all(state).unwrap();
    }
}
