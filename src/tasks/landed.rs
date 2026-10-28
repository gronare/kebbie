use std::collections::BTreeMap;
use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::messages::{self, Message};
use crate::notice::Notice;
use crate::peers;
use crate::repo::{default_branch, git, is_clean, name_of, rebase_in_progress};
use crate::sh::Result;
use super::finish::{close_task, missing_from, note_waits};
use super::intent::{self, Work};
use super::pool::{Slot, find_task, slots, task_of};

pub(crate) const EVERY: u64 = 60;
const PASSED: [&str; 3] = ["success", "skipped", "neutral"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unsure {
    Turn,
    Prompt,
    Question,
    Uncommitted,
    Note,
}

impl Unsure {
    fn key(self) -> &'static str {
        match self {
            Unsure::Turn => "turn",
            Unsure::Prompt => "prompt",
            Unsure::Question => "question",
            Unsure::Uncommitted => "uncommitted",
            Unsure::Note => "note",
        }
    }

    fn of_key(key: &str) -> Option<Unsure> {
        [Unsure::Turn, Unsure::Prompt, Unsure::Question, Unsure::Uncommitted, Unsure::Note].into_iter().find(|unsure| unsure.key() == key)
    }

    pub(crate) fn said(self, main: &str, done: char) -> String {
        match self {
            Unsure::Turn => format!("landed on {main} · kebbie closes it after this turn"),
            Unsure::Uncommitted => format!("landed on {main}, but its worktree has uncommitted changes or a stash: commit them, or drop it to discard them"),
            Unsure::Note => format!("landed on {main} · its note waits for its solution and decisions"),
            Unsure::Prompt | Unsure::Question => format!("landed on {main} · {done} closes it"),
        }
    }

    pub(crate) fn waits_for_you(self) -> bool {
        self != Unsure::Turn
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Verdict {
    Open,
    Later,
    Unsure(Unsure),
    Close,
}

pub(super) struct Seen {
    pub status: Option<String>,
    pub question: bool,
}

fn reflog(path: &Path, reference: &str) -> Vec<(String, u64, String)> {
    git(path, &["reflog", "show", "--date=unix", "--format=%H%x09%gd%x09%gs", reference]).unwrap_or_default().lines().filter_map(|line| {
        let mut fields = line.splitn(3, '\t');
        let sha = fields.next()?.to_string();
        let at = fields.next()?.rsplit_once('{')?.1.trim_end_matches('}').parse().ok()?;
        Some((sha, at, fields.next()?.to_string()))
    }).collect()
}

// A cherry-pick may bring in someone else's commit, so it is no work of the task's own.
fn made_here(subject: &str) -> bool {
    subject.starts_with("commit") || (subject.starts_with("rebase (") && !["(start)", "(finish)", "(abort)"].iter().any(|step| subject.contains(step)))
}

fn ancestor(path: &Path, sha: &str, of: &str) -> bool {
    crate::sh::succeeds("git", &["merge-base", "--is-ancestor", sha, of], Some(path))
}

fn in_upstream(path: &Path, sha: &str, upstream: &str) -> bool {
    ancestor(path, sha, upstream) || git(path, &["cherry", upstream, sha, &format!("{sha}^")]).is_ok_and(|line| line.starts_with('-'))
}

struct Made {
    start: String,
    commits: Vec<String>,
}

// HEAD's reflog belongs to the worktree, which earlier tasks used too, so only its entries since the branch was made count.
fn made(path: &Path, upstream: &str) -> Option<Made> {
    let branch = git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()?;
    let own = reflog(path, &format!("refs/heads/{branch}"));
    let Some((start, made, _)) = own.last().cloned() else {
        let start = crate::status::fork_point(path, upstream).ok()?;
        let commits = git(path, &["rev-list", &format!("{start}..HEAD")]).ok()?.lines().map(str::to_string).collect();
        return Some(Made { start, commits });
    };
    let mut commits: Vec<String> = own.into_iter().chain(reflog(path, "HEAD").into_iter().filter(|(_, at, _)| *at >= made))
        .filter(|(sha, _, subject)| made_here(subject) && !ancestor(path, sha, &start)).map(|(sha, _, _)| sha).collect();
    commits.dedup();
    Some(Made { start, commits })
}

// A branch rebased or reset onto main after its work landed keeps none of its commits, so those it made are looked for on main instead.
pub(super) fn landed(path: &Path, upstream: &str) -> Result<bool> {
    let Some(made) = made(path, upstream) else { return Ok(false) };
    if !made.commits.iter().any(|sha| ancestor(path, sha, "HEAD")) {
        return Ok(ancestor(path, "HEAD", upstream) && made.commits.iter().any(|sha| in_upstream(path, sha, upstream)));
    }
    let unmatched = git(path, &["cherry", upstream, "HEAD"])?.lines().any(|line| line.starts_with('+'));
    if !unmatched {
        return Ok(true);
    }
    if !missing_from(path, upstream)?.is_empty() {
        return Ok(false);
    }
    let base = crate::status::fork_point(path, upstream)?;
    Ok(!git(path, &["diff", "--name-only", &base, "HEAD"])?.is_empty())
}

// The first commit on main since the task began that touches a file its commits touch: never later than the landing, so a report it cannot place counts as made after it.
fn landed_at(path: &Path, upstream: &str) -> Option<u64> {
    let made = made(path, upstream)?;
    let mut files: Vec<String> = made.commits.iter().filter_map(|sha| git(path, &["show", "--name-only", "--format=", sha]).ok()).flat_map(|names| names.lines().map(str::to_string).collect::<Vec<_>>()).collect();
    files.sort();
    files.dedup();
    if files.is_empty() {
        return None;
    }
    let mut args = vec!["log".to_string(), "--reverse".to_string(), "--format=%ct".to_string(), format!("{}..{upstream}", made.start), "--".to_string()];
    args.extend(files);
    git(path, &args.iter().map(String::as_str).collect::<Vec<_>>()).ok()?.lines().next()?.parse().ok()
}

fn asks_user(report_at: u64, turn_started: Option<u64>, landed_at: Option<u64>) -> bool {
    report_at >= turn_started.unwrap_or(0) && landed_at.is_none_or(|landed| report_at >= landed)
}

pub(super) fn verdict(state: &Path, slot: &Slot, address: &str, upstream: &str, seen: &Seen, superseded: &mut dyn FnMut(&crate::ci::Watch) -> bool) -> Result<Verdict> {
    if kept_open(state, address) || rebase_in_progress(&slot.path) || !landed(&slot.path, upstream)? {
        return Ok(Verdict::Open);
    }
    if crate::ci::pushing(state, address, messages::now()) {
        return Ok(Verdict::Later);
    }
    let red_settled = match crate::ci::load(state, address) {
        Some(watch) if watch.running() || !superseded(&watch) => return Ok(Verdict::Later),
        Some(_) => true,
        None => false,
    };
    if intent::pending(state, address).is_some_and(|pending| !(red_settled && pending.work == Work::Ci)) || crate::review::running_in(state, address) {
        return Ok(Verdict::Later);
    }
    let unsure = match seen.status.as_deref() {
        Some("idle" | "done") => None,
        Some("blocked") => Some(Unsure::Prompt),
        Some("working") => Some(Unsure::Turn),
        _ => return Ok(Verdict::Later),
    };
    if !is_clean(&slot.path)? || super::finish::stashed(&slot.path, task_of(slot)) {
        return Ok(Verdict::Unsure(Unsure::Uncommitted));
    }
    Ok(match unsure {
        Some(unsure) => Verdict::Unsure(unsure),
        None if seen.question => Verdict::Unsure(Unsure::Question),
        None => Verdict::Close,
    })
}

fn dir(state: &Path) -> PathBuf {
    state.join("landed")
}

fn mark_file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.json"))
}

fn head(path: &Path) -> Option<String> {
    git(path, &["rev-parse", "HEAD"]).ok()
}

fn read_mark(state: &Path, address: &str) -> Option<(Unsure, String, u64)> {
    let value: Value = serde_json::from_str(&fs::read_to_string(mark_file(state, address)).ok()?).ok()?;
    Some((Unsure::of_key(value["why"].as_str()?)?, value["head"].as_str()?.to_string(), value["at"].as_u64().unwrap_or_default()))
}

fn save_mark(state: &Path, address: &str, why: Unsure, head: &str) {
    let at = read_mark(state, address).filter(|(was, seen, _)| *was == why && seen == head).map_or(messages::now(), |(_, _, at)| at);
    let _ = crate::state::write(&mark_file(state, address), json!({ "why": why.key(), "head": head, "at": at }).to_string());
}

fn kept_file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.kept"))
}

pub(super) fn keep_open(state: &Path, address: &str, kept: bool) {
    if kept {
        let _ = crate::state::write(&kept_file(state, address), "");
    } else {
        let _ = fs::remove_file(kept_file(state, address));
    }
}

pub(super) fn kept_open(state: &Path, address: &str) -> bool {
    kept_file(state, address).exists()
}

fn drop_mark(state: &Path, address: &str) {
    let _ = fs::remove_file(mark_file(state, address));
}

pub(crate) fn forget(state: &Path, address: &str) {
    drop_mark(state, address);
    super::red_ci::forget(state, address);
    keep_open(state, address, false);
}

pub(crate) fn mark(state: &Path, address: &str, path: &Path) -> Option<Unsure> {
    let (why, seen, _) = read_mark(state, address)?;
    (head(path).as_deref() == Some(seen.as_str())).then_some(why)
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

fn failed_workflows(watch: &crate::ci::Watch) -> Vec<String> {
    let mut failed: Vec<String> = watch.runs.iter().filter(|run| run.status == "completed" && !PASSED.contains(&run.conclusion.as_str())).map(|run| run.title().to_string()).collect();
    if failed.is_empty() && let crate::ci::State::Failed { what, .. } = &watch.state {
        failed.extend(what.split(" · ").next().filter(|workflow| !workflow.is_empty()).map(str::to_string));
    }
    failed.sort();
    failed.dedup();
    failed
}

pub(super) fn superseded(watch: &crate::ci::Watch, gh: &str) -> bool {
    passed_later(watch, &failed_workflows(watch), gh)
}

// A later push on the branch whose every named workflow passed, on a commit that holds the watched one.
pub(crate) fn passed_later(watch: &crate::ci::Watch, failed: &[String], gh: &str) -> bool {
    passing_later(watch, failed, gh).iter().any(|sha| ancestor(&watch.repo, &watch.sha, sha))
}

// The later commit must be known here for the ancestry check, so a clone that lacks one fetches first.
pub(crate) fn passed_after_cancel(watch: &crate::ci::Watch, cancelled: &[String], gh: &str) -> bool {
    let shas = passing_later(watch, cancelled, gh);
    if shas.iter().any(|sha| !crate::sh::succeeds("git", &["cat-file", "-e", &format!("{sha}^{{commit}}")], Some(&watch.repo))) {
        let _ = super::fetch(&watch.repo);
    }
    shas.iter().any(|sha| ancestor(&watch.repo, &watch.sha, sha))
}

// The shas of later pushes on the branch on which every named workflow passed.
fn passing_later(watch: &crate::ci::Watch, failed: &[String], gh: &str) -> Vec<String> {
    if failed.is_empty() {
        return Vec::new();
    }
    let target = if watch.pushed.target.is_empty() { default_branch(&watch.repo).map(|upstream| upstream.trim_start_matches("origin/").to_string()).unwrap_or_default() } else { watch.pushed.target.clone() };
    let Ok(listed) = crate::sh::run(gh, &["run", "list", "-R", &watch.slug, "--branch", &target, "--event", "push", "--limit", "100", "--json", "databaseId,name,status,conclusion,workflowName,url,headSha,event"], Some(&watch.repo)) else { return Vec::new() };
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(&listed) else { return Vec::new() };
    let mut passed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in entries.iter().filter(|entry| entry["event"] == "push" && entry["status"] == "completed" && entry["conclusion"] == "success") {
        let (Some(sha), Some(run)) = (entry["headSha"].as_str(), crate::ci::parse_runs(&Value::Array(vec![entry.clone()]).to_string()).unwrap_or_default().pop()) else { continue };
        passed.entry(sha.to_string()).or_default().push(run.title().to_string());
    }
    passed.into_iter().filter(|(sha, workflows)| *sha != watch.sha && failed.iter().all(|workflow| workflows.contains(workflow))).map(|(sha, _)| sha).collect()
}

fn stale(repo: &Path) -> bool {
    crate::fetch::fetched_at(repo).is_none_or(|at| SystemTime::now().duration_since(at).unwrap_or_default() >= crate::fetch::EVERY)
}

fn status_of(path: &Path, workspaces: &[Value], agents: &[Value]) -> Option<String> {
    let workspace = workspaces.iter().find(|workspace| workspace["worktree"]["checkout_path"].as_str() == path.to_str())?["workspace_id"].as_str()?;
    agents.iter().find(|agent| agent["workspace_id"].as_str() == Some(workspace))?["agent_status"].as_str().map(str::to_string)
}

pub(super) enum Refused {
    Note(String),
    Other(String),
}

pub(super) type Close<'a> = &'a mut dyn FnMut(&Path, &str, bool) -> std::result::Result<Notice, Refused>;

pub(super) type Fresh = dyn Fn(&Path) -> Option<String>;

pub(super) struct World<'a> {
    pub repos: &'a [PathBuf],
    pub workspaces: &'a [Value],
    pub agents: &'a [Value],
    pub log: &'a [Message],
    pub gh: &'a str,
    pub began: u64,
    pub fresh: Option<&'a Fresh>,
}

pub(super) fn pass(state: &Path, world: &World, close: Close, tell: &mut dyn FnMut(&str)) -> Vec<String> {
    let questions = crate::questions::open_now(world.log);
    let reports = messages::latest_reports(world.log);
    let asks = |address: &str, path: &Path, upstream: &str| reports.get(address).is_some_and(|report| {
        report.report.as_ref().is_some_and(|said| said.state == "needs-you") && asks_user(report.at, crate::report::turn_started(address), landed_at(path, upstream))
    });
    let mut notes = Vec::new();
    for repo in world.repos {
        let Ok(pool) = slots(repo) else { continue };
        if !pool.iter().any(|slot| slot.task.is_some()) || (stale(repo) && super::fetch(repo).is_err()) {
            continue;
        }
        let Ok(upstream) = default_branch(repo) else { continue };
        let main = upstream.trim_start_matches("origin/").to_string();
        for slot in pool.iter().filter(|slot| slot.task.is_some()) {
            let task = task_of(slot);
            let address = peers::address(task, &name_of(repo));
            let seen = Seen { status: status_of(&slot.path, world.workspaces, world.agents), question: questions.iter().any(|question| question.from == address) || asks(&address, &slot.path, &upstream) };
            let verdict = verdict(state, slot, &address, &upstream, &seen, &mut |watch| superseded(watch, world.gh));
            match verdict {
                Ok(Verdict::Unsure(why)) => {
                    if let Some(head) = head(&slot.path) {
                        save_mark(state, &address, why, &head);
                    }
                }
                Ok(Verdict::Close) => {
                    let now = world.fresh.map_or_else(|| status_of(&slot.path, world.workspaces, world.agents), |fresh| fresh(&slot.path));
                    if !matches!(now.as_deref(), Some("idle" | "done")) || crate::report::turn_started(&address).is_some_and(|at| at >= world.began) {
                        continue;
                    }
                    let head = head(&slot.path);
                    let told = read_mark(state, &address).is_some_and(|(why, seen, _)| why == Unsure::Note && head.as_deref() == Some(seen.as_str()));
                    match close(repo, task, told) {
                        Ok(_) => {
                            drop_mark(state, &address);
                            if crate::ci::load(state, &address).is_some_and(|watch| !watch.running()) {
                                crate::ci::forget(state, &address);
                                super::after::red_ci_superseded(state, &address);
                            }
                            if intent::pending(state, &address).is_some_and(|pending| pending.work == Work::Ci) {
                                intent::forget(state, &address);
                            }
                            let text = format!("{address} landed on {main} outside kebbie's push, so kebbie closed it as done.");
                            tell(&text);
                            notes.push(format!("landed: {text}"));
                        }
                        Err(Refused::Note(error)) => {
                            if let Some(head) = head {
                                save_mark(state, &address, Unsure::Note, &head);
                            }
                            notes.push(format!("landed: {address} waits for its note: {error}"));
                        }
                        Err(Refused::Other(error)) => {
                            drop_mark(state, &address);
                            if find_task(repo, Some(task), repo).is_ok() {
                                notes.push(format!("landed: {address} could not be closed: {error}"));
                            }
                        }
                    }
                }
                Ok(Verdict::Open | Verdict::Later) | Err(_) => drop_mark(state, &address),
            }
        }
    }
    let mut fetched: Vec<PathBuf> = Vec::new();
    for watch in crate::ci::all(state).into_iter().filter(|watch| !watch.running() && find_task(&watch.repo, Some(&watch.task), &watch.repo).is_err()) {
        if !fetched.contains(&watch.repo) {
            fetched.push(watch.repo.clone());
            if stale(&watch.repo) {
                let _ = super::fetch(&watch.repo);
            }
        }
        if superseded(&watch, world.gh) {
            crate::ci::forget(state, &watch.address);
            let waiting = super::after::red_ci_superseded(state, &watch.address);
            notes.push(format!("landed: CI is green after {} on a later commit; {} waiting again", watch.address, if waiting.is_empty() { "nothing".to_string() } else { waiting.join(", ") }));
        }
    }
    notes
}

fn close_for_real(repo: &Path, task: &str, told: bool) -> std::result::Result<Notice, Refused> {
    let state = crate::lead::state_dir().map_err(Refused::Other)?;
    let address = peers::address(task, &name_of(repo));
    let vault = crate::task_note::linked(&state, &address).and_then(|_| crate::vault_cli::detect());
    let said = note_waits(vault.as_ref(), &state, &address, task, !told).map_err(Refused::Note)?;
    close_task(repo, Some(task), vault.as_ref(), said, None).map_err(Refused::Other)
}

pub fn tick(state: &Path) -> Vec<String> {
    let Some(_lock) = Lock::take(state) else { return Vec::new() };
    let (Ok(workspaces), Ok(agents)) = (crate::herdr::workspaces(), crate::herdr::agents()) else { return Vec::new() };
    let repos: Vec<PathBuf> = crate::status::known_repos(&workspaces, crate::repo::pooled_checkouts()).into_iter().collect();
    let log = messages::load();
    let fresh = |path: &Path| match (crate::herdr::workspaces(), crate::herdr::agents()) {
        (Ok(workspaces), Ok(agents)) => status_of(path, &workspaces, &agents),
        _ => None,
    };
    let world = World { repos: &repos, workspaces: &workspaces, agents: &agents, log: &log, gh: &crate::ci::gh(), began: messages::now(), fresh: Some(&fresh) };
    pass(state, &world, &mut close_for_real, &mut |text| {
        if let Err(error) = messages::to_lead(messages::ToLead::Landed, text) {
            eprintln!("Could not tell the lead: {error}");
        }
    })
}

pub fn tick_cli() -> Result<String> {
    let notes = tick(&crate::lead::state_dir()?);
    Ok(if notes.is_empty() { "No open task has landed.".to_string() } else { notes.join("\n") })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::tasks::finish::finish;
    use crate::tasks::pool::{create_slot, find_task};
    use crate::tasks::test_support::*;
    use crate::repo::branch_exists;

    fn idle() -> Seen {
        Seen { status: Some("idle".to_string()), question: false }
    }

    fn never(_: &crate::ci::Watch) -> bool {
        panic!("no CI watch is asked about here")
    }

    fn on_main(dir: &Path, slot: &Slot, upstream: &str) {
        git(&slot.path, &["push", "--quiet", "origin", &format!("HEAD:{}", upstream.trim_start_matches("origin/"))]).unwrap();
        git(dir, &["fetch", "--quiet", "origin"]).unwrap();
    }

    // Main has its own d, so the task's commit comes onto it only through a conflict, resolved to the task's d: its patch differs, its tree is the task's.
    fn picked_with_a_resolution(dir: &Path, slot: &Slot) {
        upstream_commit(dir, "d", "main's own d", "main adds d too");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        assert!(git(dir, &["-c", "user.name=Bo", "-c", "user.email=bo@example.com", "cherry-pick", &fourth]).is_err(), "the pick conflicts");
        std::fs::write(dir.join("d"), "d").unwrap();
        git(dir, &["add", "d"]).unwrap();
        git(dir, &["-c", "user.name=Bo", "-c", "user.email=bo@example.com", "-c", "core.editor=true", "cherry-pick", "--continue"]).unwrap();
        git(dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
        git(dir, &["fetch", "--quiet", "origin"]).unwrap();
    }

    struct Seen2 {
        workspaces: Vec<Value>,
        agents: Vec<Value>,
    }

    fn herdr(slot: &Slot, status: &str) -> Seen2 {
        Seen2 { workspaces: vec![json!({ "workspace_id": "w1", "worktree": { "checkout_path": slot.path.to_string_lossy() } })], agents: vec![json!({ "workspace_id": "w1", "agent_status": status })] }
    }

    fn world<'a>(repos: &'a [PathBuf], seen: &'a Seen2, log: &'a [Message], gh: &'a str) -> World<'a> {
        World { repos, workspaces: &seen.workspaces, agents: &seen.agents, log, gh, began: messages::now(), fresh: None }
    }

    fn closing_by_finish(state: &Path, upstream: &str) -> impl FnMut(&Path, &str, bool) -> std::result::Result<Notice, Refused> {
        let (state, upstream) = (state.to_path_buf(), upstream.to_string());
        move |repo: &Path, task: &str, _| {
            let slot = find_task(repo, Some(task), repo).map_err(Refused::Other)?;
            finish(repo, &slot, &upstream, &state, None).map_err(Refused::Other)
        }
    }

    #[test]
    fn a_task_whose_commit_was_cherry_picked_onto_main_with_a_conflict_resolution_closes_in_a_round() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        picked_with_a_resolution(&dir, &slot);
        assert!(git(&slot.path, &["cherry", &upstream, "HEAD"]).unwrap().starts_with('+'), "git cherry does not match the resolved pick");
        let told = RefCell::new(Vec::new());
        let repos = [dir.clone()];
        let seen = herdr(&slot, "idle");
        let notes = pass(&state, &world(&repos, &seen, &[], "false"), &mut closing_by_finish(&state, &upstream), &mut |text| told.borrow_mut().push(text.to_string()));
        assert!(!branch_exists(&dir, "billing"), "closed as done: {notes:?}");
        let address = peers::address("billing", &name_of(&dir));
        assert_eq!(told.borrow().as_slice(), [format!("{address} landed on {} outside kebbie's push, so kebbie closed it as done.", upstream.trim_start_matches("origin/"))], "the lead hears one line");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_rebased_onto_main_and_then_pushed_with_git_counts_as_landed() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "rebase", "--quiet", &upstream]).unwrap();
        on_main(&dir, &slot, &upstream);
        assert_eq!(verdict(&state, &slot, "billing@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Close);
        cleanup(dir, origin, &slot);
    }

    fn git_at(path: &Path, at: u64, args: &[&str]) {
        let status = std::process::Command::new("git").args(["-c", "user.name=Ada", "-c", "user.email=ada@example.com"]).args(args)
            .env("GIT_COMMITTER_DATE", format!("@{at} +0000")).env("GIT_AUTHOR_DATE", format!("@{at} +0000")).current_dir(path).output().unwrap();
        assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
    }

    fn fresh_slot(dir: &Path, upstream: &str) -> Slot {
        create_slot(dir, upstream).unwrap()
    }

    #[test]
    fn a_task_that_only_cherry_picked_a_commit_from_main_has_landed_nothing_of_its_own() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let fresh = fresh_slot(&dir, &upstream);
        git(&fresh.path, &["switch", "--quiet", "--create", "fresh"]).unwrap();
        upstream_commit(&dir, "toolchain", "pinned", "pin the toolchain");
        let fix = git(&dir, &["rev-parse", "HEAD"]).unwrap();
        git(&fresh.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "cherry-pick", &fix]).unwrap();
        let fresh = find_task(&dir, Some("fresh"), &dir).unwrap();
        assert_eq!(verdict(&state, &fresh, "fresh@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Open);
        git(&dir, &["worktree", "remove", "--force", &fresh.path.to_string_lossy()]).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_commit_the_worktree_made_before_the_task_began_is_not_the_tasks() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let branch = upstream.trim_start_matches("origin/");
        let same = fresh_slot(&dir, &upstream);
        std::fs::write(same.path.join("x"), "x").unwrap();
        git(&same.path, &["add", "x"]).unwrap();
        git_at(&same.path, 1_900_000_000, &["commit", "--quiet", "-m", "made just before the branch"]);
        git(&same.path, &["push", "--quiet", "origin", &format!("HEAD:{branch}")]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        git_at(&same.path, 1_900_000_000, &["switch", "--quiet", "--create", "same"]);
        let same = find_task(&dir, Some("same"), &dir).unwrap();
        assert_eq!(verdict(&state, &same, "same@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Open, "its start point is no work of its own, even made in the same second");
        let earlier = fresh_slot(&dir, &upstream);
        let before = git(&earlier.path, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(earlier.path.join("y"), "y").unwrap();
        git(&earlier.path, &["add", "y"]).unwrap();
        git_at(&earlier.path, 1_000_000_000, &["commit", "--quiet", "-m", "an earlier task's work"]);
        git(&earlier.path, &["push", "--quiet", "origin", &format!("HEAD:{branch}")]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        git_at(&earlier.path, 2_000_000_000, &["switch", "--quiet", "--create", "later", &before]);
        let later = find_task(&dir, Some("later"), &dir).unwrap();
        assert_eq!(verdict(&state, &later, "later@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Open, "what the worktree made for an earlier task is not this one's");
        for path in [&same.path, &later.path] {
            git(&dir, &["worktree", "remove", "--force", &path.to_string_lossy()]).unwrap();
        }
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_landed_task_rebased_onto_main_afterwards_still_closes() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        land_and_push(&dir, &slot);
        git(&slot.path, &["rebase", "--quiet", &upstream]).unwrap();
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), git(&dir, &["rev-parse", &upstream]).unwrap(), "the rebase dropped its landed commit");
        assert_eq!(verdict(&state, &slot, "billing@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Close);
        cleanup(dir, origin, &slot);
    }

    fn land_and_push(dir: &Path, slot: &Slot) {
        upstream_commit(dir, "elsewhere", "x", "someone else's work");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        git(dir, &["-c", "user.name=Bo", "-c", "user.email=bo@example.com", "cherry-pick", &fourth]).unwrap();
        git(dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
        git(dir, &["fetch", "--quiet", "origin"]).unwrap();
    }

    #[test]
    fn a_needs_you_report_counts_only_after_both_the_last_turn_and_the_landing() {
        assert!(asks_user(200, Some(150), Some(50)));
        assert!(!asks_user(100, Some(150), Some(50)), "a turn began since: the report is old");
        assert!(!asks_user(40, None, Some(50)), "written before the work landed");
        assert!(asks_user(40, None, None), "the landing unknown, the report is kept");
    }

    #[test]
    fn a_turn_that_starts_after_the_round_looked_keeps_the_task() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        on_main(&dir, &slot, &upstream);
        let repos = [dir.clone()];
        let seen = herdr(&slot, "idle");
        let working = |_: &Path| Some("working".to_string());
        let rechecked = World { fresh: Some(&working), ..world(&repos, &seen, &[], "false") };
        pass(&state, &rechecked, &mut |_, _, _| panic!("its Claude started a turn before the close"), &mut |_| {});
        let address = peers::address("billing", &name_of(&dir));
        let turns = crate::lead::state_dir().unwrap().join("turns");
        std::fs::create_dir_all(&turns).unwrap();
        std::fs::write(turns.join(&address), messages::now().to_string()).unwrap();
        let turned = World { began: messages::now() - 5, ..world(&repos, &seen, &[], "false") };
        pass(&state, &turned, &mut |_, _, _| panic!("a turn began while the round ran"), &mut |_| {});
        let _ = std::fs::remove_file(turns.join(&address));
        assert!(branch_exists(&dir, "billing"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_with_no_commit_of_its_own_never_closes_even_when_it_only_followed_main_or_started_over() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let fresh = create_slot(&dir, &upstream).unwrap();
        git(&fresh.path, &["switch", "--quiet", "--create", "fresh"]).unwrap();
        let fresh = find_task(&dir, Some("fresh"), &dir).unwrap();
        assert_eq!(verdict(&state, &fresh, "fresh@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Open, "a fresh task is not landed");
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        git(&fresh.path, &["rebase", "--quiet", &upstream]).unwrap();
        assert_eq!(verdict(&state, &fresh, "fresh@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Open, "following main makes no commit of its own");
        git(&slot.path, &["reset", "--quiet", "--hard", &upstream]).unwrap();
        assert_eq!(verdict(&state, &slot, "billing@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Open, "a task that threw its commit away and started over from main has landed nothing");
        let repos = [dir.clone()];
        let seen = herdr(&slot, "idle");
        let told = RefCell::new(0);
        pass(&state, &world(&repos, &seen, &[], "false"), &mut closing_by_finish(&state, &upstream), &mut |_| *told.borrow_mut() += 1);
        assert!(branch_exists(&dir, "fresh") && branch_exists(&dir, "billing") && *told.borrow() == 0);
        git(&dir, &["worktree", "remove", "--force", &fresh.path.to_string_lossy()]).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_whose_claude_herdr_cannot_place_is_left_alone_without_a_line() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        on_main(&dir, &slot, &upstream);
        for status in [None, Some("unknown".to_string()), Some("starting".to_string())] {
            assert_eq!(verdict(&state, &slot, "billing@app", &upstream, &Seen { status, question: false }, &mut never).unwrap(), Verdict::Later);
        }
        let repos = [dir.clone()];
        let away = Seen2 { workspaces: Vec::new(), agents: Vec::new() };
        pass(&state, &world(&repos, &away, &[], "false"), &mut |_, _, _| panic!("never closed while herdr cannot say what its Claude does"), &mut |_| {});
        assert_eq!(mark(&state, &peers::address("billing", &name_of(&dir)), &slot.path), None, "no row text either");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_mid_turn_asking_or_with_uncommitted_changes_is_marked_not_closed_and_closes_once_that_passes() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        on_main(&dir, &slot, &upstream);
        let address = peers::address("billing", &name_of(&dir));
        let working = Seen { status: Some("working".to_string()), question: false };
        assert_eq!(verdict(&state, &slot, &address, &upstream, &working, &mut never).unwrap(), Verdict::Unsure(Unsure::Turn));
        assert_eq!(verdict(&state, &slot, &address, &upstream, &Seen { status: Some("blocked".to_string()), question: false }, &mut never).unwrap(), Verdict::Unsure(Unsure::Prompt));
        assert_eq!(verdict(&state, &slot, &address, &upstream, &Seen { status: Some("done".to_string()), question: true }, &mut never).unwrap(), Verdict::Unsure(Unsure::Question));
        std::fs::write(slot.path.join("d"), "edited").unwrap();
        assert_eq!(verdict(&state, &slot, &address, &upstream, &idle(), &mut never).unwrap(), Verdict::Unsure(Unsure::Uncommitted));
        let repos = [dir.clone()];
        let told = RefCell::new(0);
        pass(&state, &world(&repos, &herdr(&slot, "idle"), &[], "false"), &mut closing_by_finish(&state, &upstream), &mut |_| *told.borrow_mut() += 1);
        assert_eq!(mark(&state, &address, &slot.path), Some(Unsure::Uncommitted), "the row says the work landed and the worktree has changes");
        assert_eq!(std::fs::read_to_string(slot.path.join("d")).unwrap(), "edited", "nothing is discarded");
        git(&slot.path, &["checkout", "--", "d"]).unwrap();
        pass(&state, &world(&repos, &herdr(&slot, "working"), &[], "false"), &mut closing_by_finish(&state, &upstream), &mut |_| *told.borrow_mut() += 1);
        assert_eq!(mark(&state, &address, &slot.path), Some(Unsure::Turn));
        assert!(branch_exists(&dir, "billing") && *told.borrow() == 0, "never closed mid-turn");
        pass(&state, &world(&repos, &herdr(&slot, "idle"), &[], "false"), &mut closing_by_finish(&state, &upstream), &mut |_| *told.borrow_mut() += 1);
        assert!(!branch_exists(&dir, "billing") && *told.borrow() == 1, "the turn over, it closes");
        assert_eq!(mark(&state, &address, &slot.path), None);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_stash_made_on_the_task_keeps_it_open_and_done_refuses_it_too() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        commit(&slot.path, "e", "fifth");
        git(&slot.path, &["reset", "--quiet", "--soft", "HEAD~1"]).unwrap();
        git(&slot.path, &["stash", "--quiet"]).unwrap();
        on_main(&dir, &slot, &upstream);
        assert!(is_clean(&slot.path).unwrap(), "git status shows nothing");
        assert_eq!(verdict(&state, &slot, "billing@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Unsure(Unsure::Uncommitted));
        assert!(finish(&dir, &slot, &upstream, &state, None).unwrap_err().contains("it has a stash"));
        assert!(branch_exists(&dir, "billing"));
        git(&slot.path, &["stash", "drop", "--quiet"]).unwrap();
        assert_eq!(verdict(&state, &slot, "billing@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Close);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_needs_you_report_since_its_last_turn_keeps_a_landed_task_for_the_user() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        on_main(&dir, &slot, &upstream);
        let address = peers::address("billing", &name_of(&dir));
        let report = |state: &str| Message {
            id: "r1".to_string(), at: messages::now(), from: address.clone(), to: crate::status::LEAD_LABEL.to_string(), text: "pick one".to_string(), held: false,
            report: Some(crate::messages::Report { state: state.to_string(), next: "choose the wording".to_string(), head: None }), ended: false, question: None, about: None, kind: None, landed: None, why: None, forced: false, fyi: false,
        };
        let repos = [dir.clone()];
        let seen = herdr(&slot, "idle");
        let asks = [report("needs-you")];
        pass(&state, &world(&repos, &seen, &asks, "false"), &mut |_, _, _| panic!("a task that asks the user is not closed"), &mut |_| {});
        assert_eq!(mark(&state, &address, &slot.path), Some(Unsure::Question), "the row keeps the ask and offers done");
        let mut answered = report("needs-you");
        answered.at = git(&slot.path, &["log", "-1", "--format=%ct", "HEAD"]).unwrap().parse::<u64>().unwrap() - 60;
        pass(&state, &world(&repos, &seen, &[answered], "false"), &mut closing_by_finish(&state, &upstream), &mut |_| {});
        assert!(!branch_exists(&dir, "billing"), "a needs-you from before the work landed asked for the push the landing was: it closes");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_note_that_waits_is_asked_for_once_per_commit() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        on_main(&dir, &slot, &upstream);
        let repos = [dir.clone()];
        let seen = herdr(&slot, "idle");
        let asked = RefCell::new(Vec::new());
        let mut close = |_: &Path, _: &str, told: bool| { asked.borrow_mut().push(told); Err(Refused::Note("its note lacks its decisions".to_string())) };
        for _ in 0..3 {
            pass(&state, &world(&repos, &seen, &[], "false"), &mut close, &mut |_| {});
        }
        assert_eq!(asked.borrow().as_slice(), [false, true, true], "the task hears what its note lacks once, not every round");
        assert_eq!(mark(&state, &peers::address("billing", &name_of(&dir)), &slot.path), Some(Unsure::Note));
        commit(&slot.path, "e", "fifth");
        on_main(&dir, &slot, &upstream);
        pass(&state, &world(&repos, &seen, &[], "false"), &mut close, &mut |_| {});
        assert_eq!(asked.borrow().last(), Some(&false), "a new commit is asked about again");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_needs_you_report_counts_from_the_first_main_commit_that_carries_the_tasks_change() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        git(&dir, &["-c", "user.name=Bo", "-c", "user.email=bo@example.com", "cherry-pick", &fourth]).unwrap();
        std::process::Command::new("git").args(["-c", "user.name=Bo", "-c", "user.email=bo@example.com", "commit", "--quiet", "--amend", "--no-edit"]).env("GIT_COMMITTER_DATE", "@2000000000 +0000").current_dir(&dir).status().unwrap();
        git(&dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        assert_eq!(landed_at(&slot.path, &upstream), Some(2000000000), "the picked commit, not someone else's before it");
        git(&slot.path, &["rebase", "--quiet", &upstream]).unwrap();
        assert_eq!(landed_at(&slot.path, &upstream), Some(2000000000), "rebased away, the branch's head on main is its landing");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_rows_say_landed_in_place_under_needs_you_or_beside_the_turn() {
        let mut item = crate::status::tests::task(Some("idle"), Some("needs-you"), 1);
        item.landed = Some(Unsure::Uncommitted);
        assert_eq!(item.group(), crate::status::Group::NeedsYou);
        assert_eq!(item.wanted("g").as_deref(), Some("landed on main, but its worktree has uncommitted changes or a stash: commit them, or drop it to discard them"));
        item.landed = Some(Unsure::Question);
        assert_eq!(item.wanted("g"), Some(format!("landed on main · {} closes it", crate::keys::key(crate::keys::Act::Done))));
        let mut turning = crate::status::tests::task(Some("working"), None, 1);
        turning.landed = Some(Unsure::Turn);
        assert_eq!(turning.group(), crate::status::Group::Working);
        assert!(turning.summary().contains("landed on main · kebbie closes it after this turn"), "{}", turning.summary());
        let mut ready = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        assert_eq!(ready.group(), crate::status::Group::ReadyToPush);
        ready.landed = Some(Unsure::Prompt);
        assert_eq!(ready.group(), crate::status::Group::NeedsYou);
        let mut red = crate::status::tests::task(Some("idle"), None, 1);
        red.ci = Some(crate::ci::Watch {
            address: "billing@web".to_string(), task: "billing".to_string(), repo: PathBuf::new(), slug: "acme/web".to_string(), sha: "abc".to_string(), started_at: 1, checked_at: None, close: true, runs: Vec::new(),
            state: crate::ci::State::Failed { at: 2, what: "deploy · release · Deploy".to_string(), url: String::new() }, error: None, erring_since: None, pushed: crate::ci::Pushed::default(),
        });
        red.ci_outside = Some("pushed outside the deploy window".to_string());
        crate::status::tests::rederive(&mut red);
        assert_eq!(red.group(), crate::status::Group::NeedsYou);
        assert!(red.summary().starts_with("CI failed: deploy · release · Deploy · its Claude says outside the code: pushed outside the deploy window"), "{}", red.summary());
    }

    #[test]
    fn a_close_under_way_wins_and_the_round_neither_closes_again_nor_tells_the_lead() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        on_main(&dir, &slot, &upstream);
        let address = peers::address("billing", &name_of(&dir));
        let held = super::super::finish::Closing::take(&crate::lead::state_dir().unwrap(), &address).unwrap();
        let repos = [dir.clone()];
        let seen = herdr(&slot, "idle");
        let told = RefCell::new(0);
        let notes = pass(&state, &world(&repos, &seen, &[], "false"), &mut close_for_real, &mut |_| *told.borrow_mut() += 1);
        assert!(notes.iter().any(|note| note.contains("being closed already")), "{notes:?}");
        assert!(branch_exists(&dir, "billing") && *told.borrow() == 0, "the user's done owns the close");
        finish(&dir, &slot, &upstream, &state, None).unwrap();
        drop(held);
        let notes = pass(&state, &world(&repos, &seen, &[], "false"), &mut close_for_real, &mut |_| *told.borrow_mut() += 1);
        assert!(notes.is_empty() && *told.borrow() == 0, "closed once: {notes:?}");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_of_kebbies_own_under_way_holds_the_round_off_even_with_ci_watch_off() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        on_main(&dir, &slot, &upstream);
        let begun = crate::ci::begin(&state, &dir, "billing@app", "abc", true, crate::ci::Pushed::default());
        assert!(!matches!(begun, crate::ci::Begin::Watching), "origin is not on GitHub, so nothing is watched");
        assert_eq!(verdict(&state, &slot, "billing@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Later, "between the push and its own close the round keeps off");
        crate::ci::forget(&state, "billing@app");
        assert_eq!(verdict(&state, &slot, "billing@app", &upstream, &idle(), &mut never).unwrap(), Verdict::Close);
        cleanup(dir, origin, &slot);
    }

    fn fake_gh(dir: &Path, runs: &str) -> String {
        let gh = dir.with_extension("gh");
        crate::executable::write(&gh, format!("#!/bin/sh\ncat <<'JSON'\n{runs}\nJSON\n")).unwrap();
        gh.to_string_lossy().to_string()
    }

    fn red_watch(dir: &Path, address: &str, sha: &str, close: bool) -> crate::ci::Watch {
        let failed = crate::ci::Run { id: 1, name: "deploy".to_string(), workflow: "deploy".to_string(), status: "completed".to_string(), conclusion: "failure".to_string(), url: "u".to_string() };
        let passed = crate::ci::Run { id: 2, name: "lint".to_string(), workflow: "lint".to_string(), status: "completed".to_string(), conclusion: "success".to_string(), url: "u".to_string() };
        crate::ci::Watch {
            address: address.to_string(), task: "billing".to_string(), repo: dir.to_path_buf(), slug: "acme/app".to_string(), sha: sha.to_string(), started_at: 1, checked_at: None, close, runs: vec![failed, passed],
            state: crate::ci::State::Failed { at: 2, what: "deploy · release · Deploy".to_string(), url: String::new() }, error: None, erring_since: None, pushed: crate::ci::Pushed::default(),
        }
    }

    fn run(sha: &str, workflow: &str, conclusion: &str, event: &str) -> String {
        format!(r#"{{"databaseId":1,"name":"{workflow}","workflowName":"{workflow}","status":"completed","conclusion":"{conclusion}","url":"u","headSha":"{sha}","event":"{event}"}}"#)
    }

    #[test]
    fn a_pass_after_a_cancel_on_a_commit_the_clone_lacks_is_fetched_even_right_after_a_fetch() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let watched = git(&dir, &["rev-parse", &upstream]).unwrap();
        let other = dir.with_extension("other");
        let _ = std::fs::remove_dir_all(&other);
        git(&dir, &["clone", "--quiet", &origin.to_string_lossy(), &other.to_string_lossy()]).unwrap();
        commit(&other, "later", "later");
        git(&other, &["push", "--quiet", "origin", &format!("HEAD:{}", upstream.trim_start_matches("origin/"))]).unwrap();
        let later = git(&other, &["rev-parse", "HEAD"]).unwrap();
        assert!(!crate::sh::succeeds("git", &["cat-file", "-e", &format!("{later}^{{commit}}")], Some(&dir)), "the clone does not hold the later commit");
        std::fs::write(dir.join(".git/FETCH_HEAD"), "").unwrap();
        assert!(!stale(&dir), "and was fetched just now");
        let watch = crate::ci::Watch { sha: watched.clone(), state: crate::ci::State::Running, ..red_watch(&dir, "billing@app", &watched, true) };
        let gh = fake_gh(&dir, &format!("[{}]", run(&later, "deploy", "success", "push")));
        assert!(passed_after_cancel(&watch, &["deploy".to_string()], &gh), "the passing commit is fetched before its ancestry is read");
        std::fs::remove_dir_all(other).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn red_ci_on_the_landing_commit_gives_way_only_when_the_same_workflow_passes_on_a_later_push_that_carries_it() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        on_main(&dir, &slot, &upstream);
        git(&dir, &["merge", "--quiet", "--ff-only", &upstream]).unwrap();
        let landing = git(&dir, &["rev-parse", &upstream]).unwrap();
        let before = git(&dir, &["rev-parse", &format!("{upstream}~1")]).unwrap();
        upstream_commit(&dir, "fix", "toolchain", "outside the deploy window no more");
        let later = git(&dir, &["rev-parse", &upstream]).unwrap();
        let address = peers::address("billing", &name_of(&dir));
        let watch = red_watch(&dir, &address, &landing, true);
        let asks = |runs: &[String]| superseded(&watch, &fake_gh(&dir, &format!("[{}]", runs.join(","))));
        assert!(asks(&[run(&later, "deploy", "success", "push"), run(&landing, "deploy", "failure", "push")]));
        assert!(!asks(&[run(&later, "lint", "success", "push")]), "a fast lint passing says nothing about the deploy");
        let mut two = watch.clone();
        two.runs.push(crate::ci::Run { id: 3, name: "test".to_string(), workflow: "test".to_string(), status: "completed".to_string(), conclusion: "failure".to_string(), url: "u".to_string() });
        assert!(!superseded(&two, &fake_gh(&dir, &format!("[{}]", run(&later, "deploy", "success", "push")))), "every workflow that failed must pass, not one of them");
        assert!(superseded(&two, &fake_gh(&dir, &format!("[{},{}]", run(&later, "deploy", "success", "push"), run(&later, "test", "success", "push")))));
        assert!(!asks(&[run(&later, "deploy", "success", "workflow_dispatch")]), "a run started by hand is not the push's");
        assert!(!asks(&[run(&later, "deploy", "failure", "push")]), "still red later");
        assert!(!asks(&[run(&before, "deploy", "success", "push")]), "green before the landing commit says nothing about it");
        assert!(!asks(&[run(&landing, "deploy", "success", "push")]), "the watched commit is the watch's own business");

        crate::ci::save(&state, &watch).unwrap();
        intent::after_red_ci(&state, &dir, &address, intent::By::User).unwrap();
        assert_eq!(verdict(&state, &slot, &address, &upstream, &idle(), &mut |_| false).unwrap(), Verdict::Later, "red and not superseded: it stays under CI failed");
        let green = fake_gh(&dir, &format!("[{}]", run(&later, "deploy", "success", "push")));
        let repos = [dir.clone()];
        let seen = herdr(&slot, "idle");
        pass(&state, &world(&repos, &seen, &[], &green), &mut closing_by_finish(&state, &upstream), &mut |_| {});
        assert!(!branch_exists(&dir, "billing"), "landed and its deploy green later: it closes");
        assert!(crate::ci::load(&state, &address).is_none() && intent::pending(&state, &address).is_none(), "the red watch and the push waiting for its fix go");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_red_watch_stays_when_the_close_fails() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        on_main(&dir, &slot, &upstream);
        git(&dir, &["merge", "--quiet", "--ff-only", &upstream]).unwrap();
        let landing = git(&dir, &["rev-parse", &upstream]).unwrap();
        upstream_commit(&dir, "fix", "toolchain", "later");
        let later = git(&dir, &["rev-parse", &upstream]).unwrap();
        let address = peers::address("billing", &name_of(&dir));
        crate::ci::save(&state, &red_watch(&dir, &address, &landing, true)).unwrap();
        let green = fake_gh(&dir, &format!("[{}]", run(&later, "deploy", "success", "push")));
        let repos = [dir.clone()];
        let seen = herdr(&slot, "idle");
        pass(&state, &world(&repos, &seen, &[], &green), &mut |_, _, _| Err(Refused::Other("no".to_string())), &mut |_| {});
        assert!(crate::ci::load(&state, &address).is_some(), "the red stays on the row until the task really closes");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_wait_blocked_by_red_ci_waits_again_once_a_later_commit_is_green() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        on_main(&dir, &slot, &upstream);
        let landing = git(&dir, &["rev-parse", &upstream]).unwrap();
        finish(&dir, &slot, &upstream, &state, None).unwrap();
        let elsewhere = dir.with_extension("elsewhere");
        let _ = std::fs::remove_dir_all(&elsewhere);
        git(&dir, &["clone", "--quiet", &origin.to_string_lossy(), &elsewhere.to_string_lossy()]).unwrap();
        let later = commit(&elsewhere, "fix", "fix the toolchain");
        git(&elsewhere, &["push", "--quiet", "origin", "HEAD"]).unwrap();
        std::fs::remove_dir_all(&elsewhere).unwrap();
        assert!(git(&dir, &["cat-file", "-e", &later]).is_err(), "the later commit is on origin only");
        let old = std::fs::File::options().write(true).open(dir.join(".git/FETCH_HEAD")).unwrap();
        old.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600)).unwrap();
        let address = peers::address("billing", &name_of(&dir));
        crate::ci::save(&state, &red_watch(&dir, &address, &landing, false)).unwrap();
        let wait = super::super::after::Wait {
            task: "invoices".to_string(), repo: dir.clone(), after: vec![super::super::after::Dep { address: address.clone(), repo: dir.clone() }], prompt: "go".to_string(), mode: None, profile: None, backlog: None,
            by: intent::By::User, at: 1, blocked: Some(format!("{address} landed with red CI, so invoices did not start")), gone_since: None, why: None, starts_at: None,
        };
        super::super::after::save(&state, &wait).unwrap();
        let repos = [dir.clone()];
        let green = fake_gh(&dir, &format!("[{}]", run(&later, "deploy", "success", "push")));
        let seen = Seen2 { workspaces: Vec::new(), agents: Vec::new() };
        pass(&state, &world(&repos, &seen, &[], &green), &mut closing_by_finish(&state, &upstream), &mut |_| {});
        assert!(crate::ci::load(&state, &address).is_none(), "the red watch goes, so the wait reads the task as done");
        assert_eq!(super::super::after::load(&state, &wait.address()).unwrap().blocked, None, "and the wait blocked only by that red waits again");
        cleanup(dir, origin, &slot);
    }
}
