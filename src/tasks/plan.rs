use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::peers;
use crate::permit::Caller;
use crate::repo::{default_branch, git, main_checkout, name_of};
use crate::sh::Result;
use super::pool::{Slot, find_task, task_of};

// The task's steps in order, the default branch's head when they were first listed, and the commits each step held when last seen, by their change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub titles: Vec<String>,
    pub base: String,
    pub held: Vec<Held>,
}

// A step's commits by their change, and whether its Claude said with kebbie steps done that the step's work is finished.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Held {
    pub seen: Vec<Seen>,
    pub done: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    pub code: String,
    pub subject: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Planned,
    InProgress,
    Ready,
    Accepted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub accepted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub index: usize,
    pub title: String,
    pub state: State,
    pub commits: Vec<Commit>,
    pub extra: bool,
}

impl Step {
    pub fn head(&self) -> Option<&str> {
        self.commits.last().map(|commit| commit.sha.as_str())
    }

    pub fn holds(&self, sha: &str) -> bool {
        self.commits.iter().any(|commit| commit.sha == sha)
    }
}

// The unpushed steps; a task without a plan has none and no pushed count, so callers show what they show today.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Steps {
    pub pushed: usize,
    pub items: Vec<Step>,
}

impl Steps {
    pub fn listed(&self) -> bool {
        self.pushed > 0 || !self.items.is_empty()
    }

    pub fn holding(&self, sha: &str) -> Option<&Step> {
        self.items.iter().find(|step| step.holds(sha))
    }
}

// One of the task's commits not on the default branch, with its change as accept hashes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Unpushed {
    pub sha: String,
    pub subject: String,
    pub code: String,
    pub accepted: bool,
}

fn file(state: &Path, address: &str) -> PathBuf {
    state.join("steps").join(format!("{address}.json"))
}

fn head_file(state: &Path, address: &str) -> PathBuf {
    state.join("steps").join(format!("{address}.head"))
}

fn seen_of(value: &Value) -> Seen {
    Seen { code: value["code"].as_str().unwrap_or_default().to_string(), subject: value["subject"].as_str().unwrap_or_default().to_string() }
}

// Steps an older kebbie noted as one commit each read as that commit, done.
pub fn load(state: &Path, address: &str) -> Option<Plan> {
    let value: Value = serde_json::from_str(&fs::read_to_string(file(state, address)).ok()?).ok()?;
    let titles: Vec<String> = value["titles"].as_array()?.iter().filter_map(|title| title.as_str().map(str::to_string)).collect();
    let held = match (value["held"].as_array(), value["seen"].as_array()) {
        (Some(held), _) => held.iter().map(|step| Held { seen: step["seen"].as_array().map(|seen| seen.iter().map(seen_of).collect()).unwrap_or_default(), done: step["done"] == true }).collect(),
        (None, Some(seen)) => seen.iter().map(|one| Held { seen: vec![seen_of(one)], done: true }).collect(),
        (None, None) => Vec::new(),
    };
    Some(Plan { titles, base: value["base"].as_str().unwrap_or_default().to_string(), held })
}

fn save(state: &Path, address: &str, plan: &Plan) -> Result<()> {
    let held: Vec<Value> = plan.held.iter().map(|step| json!({ "seen": step.seen.iter().map(|one| json!({ "code": one.code, "subject": one.subject })).collect::<Vec<_>>(), "done": step.done })).collect();
    crate::state::write(&file(state, address), json!({ "titles": plan.titles, "base": plan.base, "held": held }).to_string())
}

pub fn forget(task: &str, repo: &Path) {
    if let Ok(state) = state_dir() {
        let address = peers::address(task, &name_of(repo));
        let _ = fs::remove_file(file(&state, &address));
        let _ = fs::remove_file(head_file(&state, &address));
    }
}

fn one_line(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

// The first `pushed` steps stay first as they were, a pushed step beyond the list under its first commit's subject; a list that starts with them again takes them as said; every step keeps the commits it holds, by position.
pub fn rewritten(old: Option<&Plan>, pushed: usize, base: &str, titles: &[String]) -> Result<Plan> {
    let titles: Vec<String> = titles.iter().map(|title| one_line(title)).collect();
    if titles.is_empty() || titles.iter().any(String::is_empty) {
        return Err(format!("kebbie steps needs each step as a one-line title: {USAGE}"));
    }
    let Some(old) = old else { return Ok(Plan { titles, base: base.to_string(), held: Vec::new() }) };
    let pushed = pushed.min(old.held.len());
    let kept: Vec<String> = (0..pushed).map(|at| old.titles.get(at).cloned().unwrap_or_else(|| first_subject(&old.held[at]))).collect();
    let rest = if titles.starts_with(&kept) { titles[kept.len()..].to_vec() } else { titles };
    Ok(Plan { titles: kept.into_iter().chain(rest).collect(), base: old.base.clone(), held: old.held.clone() })
}

fn first_subject(held: &Held) -> String {
    held.seen.first().map(|one| one.subject.clone()).unwrap_or_default()
}

// The step new commits go to: the first after the pushed ones not done, else the one after the last.
fn in_progress(held: &[Held], pushed: usize) -> usize {
    (pushed..held.len()).find(|at| !held[*at].done).unwrap_or(held.len().max(pushed))
}

// Each open commit's step: the step that held its change, else, for a change rewritten by an amend or a squash, the step around it that lost one, else the step in progress; steps never interleave along the branch.
fn placed(held: &[Held], pushed: usize, open: &[&Unpushed]) -> Vec<usize> {
    let mut wanted: Vec<Vec<&str>> = held.iter().enumerate().map(|(at, step)| if at < pushed { Vec::new() } else { step.seen.iter().map(|one| one.code.as_str()).collect() }).collect();
    let matched: Vec<Option<usize>> = open.iter().map(|commit| (pushed..wanted.len()).find(|at| taken(&mut wanted[*at], &commit.code))).collect();
    let lost: Vec<bool> = wanted.iter().map(|left| !left.is_empty()).collect();
    let current = in_progress(held, pushed);
    let mut steps: Vec<usize> = Vec::with_capacity(open.len());
    for (at, found) in matched.iter().enumerate() {
        let before = steps.last().copied();
        let step = found.unwrap_or_else(|| match (before, matched[at + 1..].iter().flatten().next().copied()) {
            (Some(earlier), Some(later)) if earlier != later && lost[later] && !lost[earlier] => later,
            (Some(earlier), Some(_)) => earlier,
            (Some(earlier), None) if earlier == current || lost[earlier] => earlier,
            (None, Some(later)) => later,
            _ => current,
        });
        steps.push(before.map_or(step, |before| step.max(before)));
    }
    steps
}

// Pushed are the leading steps whose every commit's change the default branch gained since base, however it got there; landed holds those changes.
pub fn derive(plan: Option<&Plan>, landed: &[String], commits: &[Unpushed], working: bool) -> (Steps, Vec<Held>) {
    let Some(plan) = plan.filter(|plan| !plan.titles.is_empty()) else { return (Steps::default(), Vec::new()) };
    let mut landed: Vec<&str> = landed.iter().map(String::as_str).collect();
    let pushed = plan.held.iter().take_while(|step| !step.seen.is_empty() && step.seen.iter().all(|one| !one.code.is_empty()) && taken_all(&mut landed, &step.seen)).count();
    let mut gone: Vec<&str> = plan.held[..pushed].iter().flat_map(|step| step.seen.iter().map(|one| one.code.as_str())).collect();
    let open: Vec<&Unpushed> = commits.iter().filter(|commit| !taken(&mut gone, &commit.code)).collect();
    let steps = placed(&plan.held, pushed, &open);
    let count = steps.iter().map(|step| step + 1).max().unwrap_or(0).max(plan.held.len());
    let mut held: Vec<Held> = (0..count).map(|at| {
        if at < pushed {
            return plan.held[at].clone();
        }
        let seen: Vec<Seen> = open.iter().zip(&steps).filter(|(_, step)| **step == at).map(|(commit, _)| Seen { code: commit.code.clone(), subject: commit.subject.clone() }).collect();
        Held { done: !seen.is_empty() && plan.held.get(at).is_some_and(|step| step.done), seen }
    }).collect();
    while held.len() > pushed && held.last().is_some_and(|step| step.seen.is_empty()) {
        held.pop();
    }
    let current = in_progress(&held, pushed);
    let items = (pushed..plan.titles.len().max(held.len())).map(|index| {
        let commits: Vec<Commit> = open.iter().zip(&steps).filter(|(_, step)| **step == index).map(|(commit, _)| Commit { sha: commit.sha.clone(), subject: commit.subject.clone(), accepted: commit.accepted }).collect();
        let done = held.get(index).is_some_and(|step| step.done);
        let state = match (commits.is_empty(), done) {
            (true, _) if working && index == current => State::InProgress,
            (true, _) => State::Planned,
            (false, false) => State::InProgress,
            (false, true) if commits.iter().all(|commit| commit.accepted) => State::Accepted,
            (false, true) => State::Ready,
        };
        let title = plan.titles.get(index).cloned().or_else(|| commits.first().map(|commit| commit.subject.clone())).unwrap_or_default();
        Step { index, title, state, commits, extra: index >= plan.titles.len() }
    }).collect();
    (Steps { pushed, items }, held)
}

// Takes one of code out of codes, so a change made twice is counted twice.
fn taken(codes: &mut Vec<&str>, code: &str) -> bool {
    codes.iter().position(|one| *one == code).map(|at| codes.remove(at)).is_some()
}

// Takes every one of a step's changes, or none of them.
fn taken_all(codes: &mut Vec<&str>, seen: &[Seen]) -> bool {
    let mut left = codes.clone();
    let all = seen.iter().all(|one| taken(&mut left, &one.code));
    if all {
        *codes = left;
    }
    all
}

struct Task<'a> {
    path: &'a Path,
    upstream: &'a str,
    address: &'a str,
}

static OPEN: super::accept::Cached<Vec<(String, String, String)>> = std::sync::Mutex::new(None);

// The tips a reading stands on: the default branch's and HEAD's shas.
#[derive(Clone, Debug)]
pub struct Tips {
    pub upstream: String,
    pub head: String,
}

fn tips_of(task: &Task) -> Result<Tips> {
    let shas = git(task.path, &["rev-parse", &crate::repo::tracking(task.upstream), "HEAD"])?;
    let (upstream, head) = shas.split_once('\n').ok_or_else(|| format!("git rev-parse {} HEAD printed one sha", task.upstream))?;
    Ok(Tips { upstream: upstream.to_string(), head: head.to_string() })
}

fn unpushed(state: &Path, path: &Path, address: &str, tips: &Tips) -> Result<Vec<Unpushed>> {
    let range = format!("{}..{}", tips.upstream, tips.head);
    let commits = super::accept::cached(&OPEN, (&tips.upstream, &tips.head), || {
        let codes = super::accept::commit_codes(path, &["--reverse", &range])?;
        let subjects = git(path, &["log", "--reverse", "--format=%s", &range])?;
        Ok(codes.into_iter().zip(subjects.lines()).map(|((sha, code), subject)| (sha, subject.to_string(), code)).collect())
    })?;
    let accepted = super::accept::codes_in(state, address);
    let codes: Vec<(String, String)> = commits.iter().map(|(sha, _, code)| (sha.clone(), code.clone())).collect();
    let keys = super::accept::keys(&codes);
    Ok(commits.into_iter().zip(keys).map(|((sha, subject, code), key)| Unpushed { accepted: accepted.contains(&key), sha, subject, code }).collect())
}

// The changes the default branch gained since base; a base it no longer holds gains nothing.
fn landed(path: &Path, base: &str, tips: &Tips) -> Vec<String> {
    if base.is_empty() {
        return Vec::new();
    }
    super::accept::landed_between(path, base, &tips.upstream)
}

fn current_at(state: &Path, path: &Path, address: &str, plan: &Plan, tips: &Tips, working: bool) -> Result<(Steps, Vec<Held>)> {
    Ok(derive(Some(plan), &landed(path, &plan.base, tips), &unpushed(state, path, address, tips)?, working))
}

fn current(state: &Path, task: &Task, plan: &Plan, working: bool) -> Result<(Steps, Vec<Held>)> {
    current_at(state, task.path, task.address, plan, &tips_of(task)?, working)
}

// Whether the task's steps name one beyond `pushed`, the tip just pushed to the default branch; false for a task without steps.
pub fn left_at(state: &Path, path: &Path, address: &str, pushed: &str) -> bool {
    let Some(plan) = load(state, address) else { return false };
    tips_of(&Task { path, upstream: pushed, address }).and_then(|tips| current_at(state, path, address, &plan, &tips, false)).is_ok_and(|(steps, _)| !steps.items.is_empty())
}

// What the board reads for a task on what it already holds, task.steps on; it notes nothing, which the hook, a report, a push and kebbie steps do.
pub fn read_at(state: &Path, path: &Path, address: &str, tips: &Tips, working: bool) -> Steps {
    let Some(plan) = load(state, address) else { return Steps::default() };
    current_at(state, path, address, &plan, tips, working).map(|(steps, _)| steps).unwrap_or_default()
}

// The steps of the task in path as they stand, noting nothing; empty without a plan.
pub(super) fn read_now(state: &Path, path: &Path, upstream: &str, address: &str) -> Steps {
    tips_of(&Task { path, upstream, address }).map(|tips| read_at(state, path, address, &tips, false)).unwrap_or_default()
}

// Records which commits each step holds now, so a step keeps them through a rebase and still counts once they reach the default branch by any road.
fn listed_in(state: &Path, task: &Task, working: bool) -> Result<Steps> {
    let Some(mut plan) = load(state, task.address) else { return Ok(Steps::default()) };
    let (steps, held) = current(state, task, &plan, working)?;
    if held != plan.held && steps.listed() {
        plan.held = held;
        save(state, task.address, &plan)?;
    }
    Ok(steps)
}

// What the board and the review split read for the task whose worktree is path, empty with task.steps off; working is whether its Claude is at work now.
pub fn steps_in(state: &Path, path: &Path, working: bool) -> Result<Steps> {
    let (repo, slot, address) = located(path)?;
    if !crate::config::resolve(Some(&repo))?.settings.steps.value {
        return Ok(Steps::default());
    }
    let upstream = default_branch(&repo)?;
    listed_in(state, &Task { path: &slot.path, upstream: &upstream, address: &address }, working)
}

fn located(path: &Path) -> Result<(PathBuf, Slot, String)> {
    let repo = main_checkout(path)?;
    let slot = find_task(&repo, None, path)?;
    let address = peers::address(task_of(&slot), &name_of(&repo));
    Ok((repo, slot, address))
}

// From the task's PostToolUse and PostToolUseFailure hooks on Bash, with task.steps on: notes the steps once HEAD moved since they were last noted, so a commit pushed at once with plain git is still its step's.
pub fn noted_after(state: &Path, address: &str, input: &Value) -> bool {
    if input["tool_name"] != "Bash" || load(state, address).is_none_or(|plan| plan.titles.is_empty()) {
        return false;
    }
    let Some(cwd) = input["cwd"].as_str() else { return false };
    let Ok(found) = git(Path::new(cwd), &["rev-parse", "--show-toplevel", "HEAD"]) else { return false };
    let Some((top, head)) = found.split_once('\n') else { return false };
    if fs::read_to_string(head_file(state, address)).is_ok_and(|noted| noted == head) {
        return false;
    }
    let noted = located(Path::new(top)).and_then(|(repo, slot, own)| {
        if own != address {
            return Err(format!("{top} is {own}'s worktree, not {address}'s"));
        }
        let upstream = default_branch(&repo)?;
        listed_in(state, &Task { path: &slot.path, upstream: &upstream, address }, false)
    });
    noted.is_ok() && crate::state::write(&head_file(state, address), head).is_ok()
}

// Notes what each step's commit is before it can leave the branch, as a push or a turn's end does.
pub fn seen_in(state: &Path, path: &Path) {
    let _ = steps_in(state, path, false);
}

// The same, with no plan and no error to handle: an empty Steps.
pub fn for_path(path: &Path, working: bool) -> Steps {
    state_dir().and_then(|state| steps_in(&state, path, working)).unwrap_or_default()
}

// What kebbie steps --json prints for the task whose worktree is path, read in-process and noting nothing; None with task.steps off or outside a task.
pub fn shown_at(path: &Path) -> Option<Value> {
    let (repo, slot, address) = located(path).ok()?;
    if !crate::config::resolve(Some(&repo)).ok()?.settings.steps.value {
        return None;
    }
    let state = state_dir().ok()?;
    let upstream = default_branch(&repo).ok()?;
    let task = Task { path: &slot.path, upstream: &upstream, address: &address };
    let working = crate::phase::load(&state, &address).is_some_and(|record| matches!(record.phase, crate::phase::Phase::Working | crate::phase::Phase::Starting));
    let steps = read_at(&state, &slot.path, &address, &tips_of(&task).ok()?, working);
    Some(to_json(&address, &steps, super::accept::last_accepted(&slot.path).as_deref()))
}

const USAGE: &str = "kebbie steps \"<step>\" \"<step>\" … in the task's worktree, or kebbie steps <name> \"<step>\" … elsewhere; kebbie steps [name] done marks the step in progress ready; kebbie steps [name] [--json] shows them";

fn off_line(repo: &Path) -> String {
    format!("kebbie steps is off for {}: set task.steps = true in {} or {}, or on the board's Settings page.", name_of(repo), crate::config::path().display(), crate::config::repo_file(repo).display())
}

// In a task's worktree every word is a step; elsewhere the first names the task.
fn target<'a>(repo: &Path, cwd: &Path, words: &'a [String]) -> Result<(Slot, &'a [String])> {
    if let Ok(slot) = find_task(repo, None, cwd) {
        return Ok((slot, words));
    }
    let name = words.first().ok_or_else(|| format!("Not inside a task's worktree; name the task: {USAGE}"))?;
    Ok((find_task(repo, Some(name), cwd)?, &words[1..]))
}

pub fn steps(cwd: &Path, words: &[String], as_json: bool) -> Result<String> {
    let repo = main_checkout(cwd)?;
    if !crate::config::resolve(Some(&repo))?.settings.steps.value {
        return Err(off_line(&repo));
    }
    let (slot, titles) = target(&repo, cwd, words)?;
    let address = peers::address(task_of(&slot), &name_of(&repo));
    let upstream = default_branch(&repo)?;
    let task = Task { path: &slot.path, upstream: &upstream, address: &address };
    let state = state_dir()?;
    let mut working = crate::phase::load(&state, &address).is_some_and(|record| matches!(record.phase, crate::phase::Phase::Working | crate::phase::Phase::Starting));
    if !titles.is_empty() {
        let caller = crate::permit::caller();
        if titles == ["done"] {
            done_as(&caller, &state, &task)?;
        } else {
            write_as(&caller, &state, &task, titles)?;
        }
        working |= caller == Caller::Task(address.clone());
    }
    let steps = listed_in(&state, &task, working)?;
    let last = super::accept::last_accepted(&slot.path);
    if as_json {
        return serde_json::to_string_pretty(&to_json(&address, &steps, last.as_deref())).map_err(|error| error.to_string());
    }
    Ok(shown(&address, &steps))
}

// A task writes its own steps only; the lead and the user write any task's.
fn writes(caller: &Caller, task: &Task) -> Result<()> {
    match caller {
        Caller::Task(own) if own != task.address => Err(format!("the task {own} writes its own steps, not {}'s", task.address)),
        _ => Ok(()),
    }
}

fn write_as(caller: &Caller, state: &Path, task: &Task, titles: &[String]) -> Result<()> {
    writes(caller, task)?;
    let old = load(state, task.address);
    let (pushed, held) = match &old {
        Some(plan) => current(state, task, plan, false).map(|(steps, held)| (steps.pushed, held))?,
        None => (0, Vec::new()),
    };
    let base = git(task.path, &["rev-parse", &crate::repo::tracking(task.upstream)])?;
    let old = old.map(|plan| Plan { held, ..plan });
    save(state, task.address, &rewritten(old.as_ref(), pushed, &base, titles)?)
}

// kebbie steps done: the step in progress is ready, and the commits after it go to the next step.
fn done_as(caller: &Caller, state: &Path, task: &Task) -> Result<()> {
    writes(caller, task)?;
    let mut plan = load(state, task.address).filter(|plan| !plan.titles.is_empty()).ok_or_else(|| format!("{} has no steps to finish: {USAGE}", task.address))?;
    let (steps, held) = current(state, task, &plan, false)?;
    plan.held = held;
    let at = in_progress(&plan.held, steps.pushed);
    match plan.held.get_mut(at) {
        Some(step) if !step.seen.is_empty() => step.done = true,
        _ => return Err(format!("Step {} holds no commit yet: commit its work, then kebbie steps done", at + 1)),
    }
    save(state, task.address, &plan)
}

pub fn mark(state: &State) -> &'static str {
    match state {
        State::Planned => "○",
        State::InProgress => "◐",
        State::Ready => "●",
        State::Accepted => "✓",
    }
}

pub fn word(state: &State) -> &'static str {
    match state {
        State::Planned => "planned",
        State::InProgress => "in progress",
        State::Ready => "ready",
        State::Accepted => "accepted",
    }
}

// What a step's row says after its title: its state and, once it holds any, how many commits.
pub fn label(step: &Step) -> String {
    match (&step.state, step.commits.len()) {
        (State::Planned, _) => String::new(),
        (state, 0) => word(state).to_string(),
        (state, 1) => format!("{} · 1 commit", word(state)),
        (state, many) => format!("{} · {many} commits", word(state)),
    }
}

fn shown(address: &str, steps: &Steps) -> String {
    if !steps.listed() {
        return format!("{address} has no steps: kebbie steps \"<step>\" … lists them, and kebbie steps done marks each one ready.");
    }
    let mut lines = vec![format!("{address}'s steps:")];
    if steps.pushed > 0 {
        lines.push(format!("  ↑ {} pushed", steps.pushed));
    }
    for item in &steps.items {
        let mut line = format!("  {} {}. {}", mark(&item.state), item.index + 1, item.title);
        let label = label(item);
        if !label.is_empty() {
            line.push_str(&format!("  {label}"));
        }
        if item.extra {
            line.push_str("  (not in the list)");
        }
        lines.push(line);
        lines.extend(item.commits.iter().map(|commit| format!("       {} {}", &commit.sha[..7.min(commit.sha.len())], commit.subject)));
    }
    lines.join("\n")
}

fn state_name(state: &State) -> &'static str {
    match state {
        State::Planned => "planned",
        State::InProgress => "in-progress",
        State::Ready => "ready",
        State::Accepted => "accepted",
    }
}

fn to_json(address: &str, steps: &Steps, last_accepted: Option<&str>) -> Value {
    let items: Vec<Value> = steps.items.iter().map(|item| {
        let commits: Vec<Value> = item.commits.iter().map(|commit| json!({ "sha": commit.sha, "subject": commit.subject, "accepted": commit.accepted })).collect();
        json!({ "index": item.index, "title": item.title, "extra": item.extra, "state": state_name(&item.state), "commits": commits })
    }).collect();
    json!({ "task": address, "listed": steps.listed(), "pushed": steps.pushed, "steps": items, "last_accepted": last_accepted })
}

#[cfg(test)]
pub(crate) fn list_at(state: &Path, path: &Path, upstream: &str, address: &str, titles: &[&str]) -> Result<()> {
    let titles: Vec<String> = titles.iter().map(|title| title.to_string()).collect();
    write_as(&Caller::User, state, &Task { path, upstream, address }, &titles)
}

#[cfg(test)]
pub(crate) fn done_at(state: &Path, path: &Path, upstream: &str, address: &str) -> Result<()> {
    done_as(&Caller::User, state, &Task { path, upstream, address })
}

#[cfg(test)]
pub(crate) fn listed_at(state: &Path, path: &Path, upstream: &str, address: &str) -> Steps {
    listed_in(state, &Task { path, upstream, address }, false).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::test_support::*;

    fn titles(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    fn commit_of(sha: &str, subject: &str, accepted: bool) -> Unpushed {
        Unpushed { sha: sha.to_string(), subject: subject.to_string(), code: format!("code of {sha}"), accepted }
    }

    fn seen(codes: &[&str]) -> Vec<Seen> {
        codes.iter().map(|code| Seen { code: code.to_string(), subject: format!("subject of {code}") }).collect()
    }

    fn done(groups: &[&[&str]]) -> Vec<Held> {
        groups.iter().map(|codes| Held { seen: seen(codes), done: true }).collect()
    }

    fn plan(words: &[&str], held: Vec<Held>) -> Plan {
        Plan { titles: titles(words), base: "base".to_string(), held }
    }

    fn shas(steps: &Steps) -> Vec<(usize, Vec<&str>)> {
        steps.items.iter().map(|item| (item.index, item.commits.iter().map(|commit| commit.sha.as_str()).collect())).collect()
    }

    fn states(steps: &Steps) -> Vec<State> {
        steps.items.iter().map(|item| item.state.clone()).collect()
    }

    fn rewritten_code(sha: &str, code: &str) -> Unpushed {
        Unpushed { code: code.to_string(), ..commit_of(sha, &format!("subject {sha}"), false) }
    }

    #[test]
    fn a_rewrite_changes_the_titles_not_pushed_and_every_step_keeps_its_commits() {
        let first = rewritten(None, 0, "base", &titles(&["Add the model", "  Wire\nthe command ", "Document it"])).unwrap();
        assert_eq!(first, plan(&["Add the model", "Wire the command", "Document it"], Vec::new()), "each title is one line");
        let held = Plan { held: done(&[&["a"], &["b", "c"]]), ..first };
        assert_eq!(rewritten(Some(&held), 1, "later", &titles(&["Wire it differently", "Test it"])).unwrap(), plan(&["Add the model", "Wire it differently", "Test it"], done(&[&["a"], &["b", "c"]])), "the base stays the one the steps were first listed on, and the commits stay where they are");
        assert_eq!(rewritten(Some(&held), 1, "later", &titles(&["Add the model", "Test it"])).unwrap().titles, titles(&["Add the model", "Test it"]), "a list that repeats the pushed ones first is taken as the whole list");
        assert_eq!(rewritten(Some(&held), 1, "later", &titles(&["Add the model"])).unwrap().titles, titles(&["Add the model"]), "a list of just the pushed ones is the whole list, not them twice");
        let beyond = Plan { titles: titles(&["Model"]), base: "base".to_string(), held: done(&[&["a"], &["b", "c"]]) };
        assert_eq!(rewritten(Some(&beyond), 2, "base", &titles(&["Docs"])).unwrap().titles, titles(&["Model", "subject of b", "Docs"]), "a pushed step beyond the list keeps its first commit's subject");
        assert!(rewritten(Some(&held), 1, "base", &[]).is_err() && rewritten(None, 0, "base", &titles(&["ok", " "])).is_err(), "no empty list and no empty title");
    }

    #[test]
    fn commits_go_to_the_step_in_progress_until_it_is_done_and_then_to_the_next() {
        let listed = plan(&["Model", "Command", "Docs"], Vec::new());
        let (fresh, _) = derive(Some(&listed), &[], &[], true);
        assert_eq!(states(&fresh), [State::InProgress, State::Planned, State::Planned], "the first step is under way while its Claude works");
        assert_eq!(states(&derive(Some(&listed), &[], &[], false).0), [State::Planned, State::Planned, State::Planned], "and planned while nobody works on it");
        let two = [commit_of("aaa", "Add the model", true), commit_of("bbb", "Test the model", false)];
        let (working, held) = derive(Some(&listed), &[], &two, false);
        assert_eq!((shas(&working), states(&working)), (vec![(0, vec!["aaa", "bbb"]), (1, vec![]), (2, vec![])], vec![State::InProgress, State::Planned, State::Planned]), "commits made with no step marked done go to the step in progress");
        assert_eq!(label(&working.items[0]), "in progress · 2 commits");
        let mut marked = Plan { held, ..listed.clone() };
        marked.held[0].done = true;
        let three = [two[0].clone(), two[1].clone(), commit_of("ccc", "Add the command", false)];
        let (next, _) = derive(Some(&marked), &[], &three, true);
        assert_eq!((shas(&next), states(&next)), (vec![(0, vec!["aaa", "bbb"]), (1, vec!["ccc"]), (2, vec![])], vec![State::Ready, State::InProgress, State::Planned]), "once done, the next commit opens the next step");
        let accepted: Vec<Unpushed> = three.iter().map(|commit| Unpushed { accepted: commit.sha != "ccc", ..commit.clone() }).collect();
        assert_eq!(derive(Some(&marked), &[], &accepted, true).0.items[0].state, State::Accepted, "a done step whose every commit is accepted is accepted");
    }

    #[test]
    fn a_rebase_keeps_each_commit_in_its_step_and_a_change_folded_into_a_commit_stays_in_that_commits_step() {
        let listed = plan(&["Model", "Command", "Docs"], vec![Held { seen: seen(&["a1", "a2"]), done: true }, Held { seen: seen(&["b1", "b2"]), done: false }]);
        let rebased = [rewritten_code("x1", "a1"), rewritten_code("x2", "a2"), rewritten_code("x3", "b1"), rewritten_code("x4", "b2")];
        assert_eq!(shas(&derive(Some(&listed), &[], &rebased, false).0)[..2], [(0, vec!["x1", "x2"]), (1, vec!["x3", "x4"])], "new shas, the same changes");
        let into_last_of_first = [rewritten_code("x1", "a1"), rewritten_code("x2", "a2+fix"), rewritten_code("x3", "b1"), rewritten_code("x4", "b2")];
        assert_eq!(shas(&derive(Some(&listed), &[], &into_last_of_first, false).0)[..2], [(0, vec!["x1", "x2"]), (1, vec!["x3", "x4"])]);
        let into_first_of_second = [rewritten_code("x1", "a1"), rewritten_code("x2", "a2"), rewritten_code("x3", "b1+fix"), rewritten_code("x4", "b2")];
        assert_eq!(shas(&derive(Some(&listed), &[], &into_first_of_second, false).0)[..2], [(0, vec!["x1", "x2"]), (1, vec!["x3", "x4"])]);
        let finished = plan(&["Model", "Command"], done(&[&["a1", "a2"]]));
        let amended = [rewritten_code("x1", "a1"), rewritten_code("x2", "a2+fix")];
        let (steps, held) = derive(Some(&finished), &[], &amended, false);
        assert_eq!((shas(&steps), held[0].done), (vec![(0, vec!["x1", "x2"]), (1, vec![])], true), "an amend of a done step's last commit stays in it");
        let after = [rewritten_code("x1", "a1"), rewritten_code("x2", "a2+fix"), rewritten_code("x3", "new")];
        assert_eq!(shas(&derive(Some(&Plan { held, ..finished }), &[], &after, false).0), [(0, vec!["x1", "x2"]), (1, vec!["x3"])], "and the next new commit goes to the step in progress");
    }

    #[test]
    fn a_step_counts_as_pushed_once_the_default_branch_gained_every_one_of_its_changes() {
        let listed = plan(&["Model", "Command", "Docs"], done(&[&["code of aaa", "code of bbb"], &["code of ccc"]]));
        let commits = [commit_of("aaa", "Add the model", false), commit_of("bbb", "Test it", false), commit_of("ccc", "Add the command", false)];
        let (steps, _) = derive(Some(&listed), &["code of aaa".to_string(), "code of bbb".to_string()], &commits, false);
        assert_eq!((steps.pushed, shas(&steps)), (1, vec![(1, vec!["ccc"]), (2, vec![])]), "cherry-picked copies on main take the step out of sight");
        let (steps, _) = derive(Some(&listed), &["code of aaa".to_string()], &commits, false);
        assert_eq!((steps.pushed, steps.items[0].commits.len()), (0, 2), "half a step on main is no pushed step");
        let (steps, _) = derive(Some(&listed), &["code of ccc".to_string()], &commits, false);
        assert_eq!((steps.pushed, steps.items.len()), (0, 3), "the second step alone on main is no pushed run, and nothing is hidden");
    }

    #[test]
    fn an_open_commit_with_the_same_change_as_a_pushed_step_stays_in_sight() {
        let same = |sha: &str| Unpushed { sha: sha.to_string(), subject: format!("subject {sha}"), code: "same".to_string(), accepted: false };
        let listed = plan(&["Model", "Again"], done(&[&["same"], &["same"]]));
        let (steps, _) = derive(Some(&listed), &["same".to_string()], &[same("aaa"), same("bbb")], false);
        assert_eq!((steps.pushed, shas(&steps)), (1, vec![(1, vec!["bbb"])]), "one landed copy pushes one step and hides one commit");
        let (steps, _) = derive(Some(&listed), &["same".to_string(), "same".to_string()], &[same("aaa"), same("bbb")], false);
        assert_eq!((steps.pushed, steps.items.len()), (2, 0), "two landed copies push both");
    }

    #[test]
    fn commits_after_every_listed_step_is_done_make_one_extra_step_titled_by_its_first_subject() {
        let listed = plan(&["Model"], done(&[&["code of aaa"]]));
        let (found, held) = derive(Some(&listed), &[], &[commit_of("aaa", "Add the model", false), commit_of("bbb", "Fix a typo", false), commit_of("ccc", "Fix another", false)], true);
        assert_eq!(shas(&found), [(0, vec!["aaa"]), (1, vec!["bbb", "ccc"])], "nothing is hidden");
        assert_eq!((found.items[1].title.as_str(), found.items[1].extra, &found.items[1].state), ("Fix a typo", true, &State::InProgress));
        assert!(!found.items[0].extra && found.items[0].title == "Model");
        assert_eq!(held.len(), 2, "the extra step is noted with its commits");
    }

    #[test]
    fn steps_an_older_kebbie_noted_one_commit_each_read_as_done_steps() {
        let dir = crate::sh::test_dir("plan-older");
        crate::state::write(&file(&dir, "billing@app"), json!({ "titles": ["Model", "Command", "Docs"], "base": "base", "seen": [{ "code": "code of aaa", "subject": "Add the model" }, { "code": "code of bbb", "subject": "Add the command" }] }).to_string()).unwrap();
        let old = load(&dir, "billing@app").unwrap();
        assert_eq!(old.held, vec![Held { seen: vec![Seen { code: "code of aaa".to_string(), subject: "Add the model".to_string() }], done: true }, Held { seen: vec![Seen { code: "code of bbb".to_string(), subject: "Add the command".to_string() }], done: true }]);
        let (steps, _) = derive(Some(&old), &[], &[commit_of("aaa", "Add the model", false), commit_of("bbb", "Add the command", true), commit_of("ccc", "Write the docs", false)], false);
        assert_eq!((shas(&steps), states(&steps)), (vec![(0, vec!["aaa"]), (1, vec!["bbb"]), (2, vec!["ccc"])], vec![State::Ready, State::Accepted, State::InProgress]));
        save(&dir, "billing@app", &old).unwrap();
        assert_eq!(load(&dir, "billing@app").unwrap(), old, "written back in the new shape and read the same");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_task_without_steps_has_none_and_no_pushed_count() {
        let (none, _) = derive(None, &[], &[commit_of("aaa", "Add the model", false)], true);
        assert_eq!(none, Steps::default());
        assert!(!none.listed(), "callers fall back to the rows they show today");
        assert!(!derive(Some(&Plan::default()), &[], &[commit_of("aaa", "x", false)], true).0.listed());
        assert!(derive(Some(&plan(&["Model"], done(&[&["a"]]))), &["a".to_string()], &[], false).0.listed(), "a list pushed in full is still a list");
    }

    fn landed_task() -> (PathBuf, PathBuf, crate::tasks::Slot, String, PathBuf, String) {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let state = state_of(&dir);
        list_at(&state, &slot.path, &upstream, "billing@app", &["Model", "Command"]).unwrap();
        done_at(&state, &slot.path, &upstream, "billing@app").unwrap();
        commit(&slot.path, "e", "fifth");
        assert_eq!(listed_at(&state, &slot.path, &upstream, "billing@app").pushed, 0);
        (dir, origin, slot, upstream, state, fourth)
    }

    fn after_landing(state: &Path, slot: &crate::tasks::Slot, upstream: &str) -> (usize, Vec<(usize, String)>) {
        let steps = listed_at(state, &slot.path, upstream, "billing@app");
        (steps.pushed, steps.items.iter().map(|item| (item.index, item.commits.first().map(|commit| commit.subject.clone()).unwrap_or_default())).collect())
    }

    #[test]
    fn a_step_staged_on_main_and_pushed_by_hand_counts_as_pushed() {
        let (dir, origin, slot, upstream, state, fourth) = landed_task();
        pick(&dir, &fourth);
        git(&dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        assert_eq!(git(&slot.path, &["rev-list", "--count", &format!("{upstream}..HEAD")]).unwrap(), "2", "the branch still holds both, unrebased");
        assert_eq!(after_landing(&state, &slot, &upstream), (1, vec![(1, "fifth".to_string())]));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_first_commits_pushed_with_plain_git_from_the_main_checkout_count_as_pushed() {
        let (dir, origin, slot, upstream, state, fourth) = landed_task();
        git(&dir, &["push", "--quiet", "origin", &format!("{fourth}:refs/heads/{}", upstream.trim_start_matches("origin/"))]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        assert_eq!(after_landing(&state, &slot, &upstream), (1, vec![(1, "fifth".to_string())]));
        cleanup(dir, origin, &slot);
    }

    // The PostToolUse input of https://code.claude.com/docs/en/hooks for a Bash command run in the task's worktree.
    fn bash_in(path: &Path, command: &str) -> Value {
        json!({
            "session_id": "abc123", "transcript_path": "/t.jsonl", "cwd": path, "permission_mode": "default", "hook_event_name": "PostToolUse",
            "tool_name": "Bash", "tool_input": { "command": command, "description": "Commit the step" },
            "tool_response": { "stdout": "", "stderr": "", "interrupted": false, "isImage": false },
            "tool_use_id": "toolu_01ABC123", "duration_ms": 41,
        })
    }

    fn hooked_task() -> (PathBuf, PathBuf, crate::tasks::Slot, String, PathBuf, String) {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = peers::address("billing", &name_of(&dir));
        list_at(&state, &slot.path, &upstream, &address, &["Model", "Command"]).unwrap();
        assert!(noted_after(&state, &address, &bash_in(&slot.path, "git commit -m fourth")), "the first commit is noted by the hook after it");
        (dir, origin, slot, upstream, state, address)
    }

    #[test]
    fn a_commit_noted_by_the_bash_hook_counts_as_pushed_after_a_plain_push_with_nothing_else_listing_the_steps() {
        let (dir, origin, slot, upstream, state, address) = hooked_task();
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        done_at(&state, &slot.path, &upstream, &address).unwrap();
        commit(&slot.path, "e", "fifth");
        assert!(noted_after(&state, &address, &bash_in(&slot.path, "git commit -m fifth")));
        git(&dir, &["push", "--quiet", "origin", &format!("{fourth}:refs/heads/{}", upstream.trim_start_matches("origin/"))]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        let steps = listed_at(&state, &slot.path, &upstream, &address);
        assert_eq!((steps.pushed, steps.items.iter().map(|item| (item.index, item.commits.iter().map(|commit| commit.subject.as_str()).collect::<Vec<_>>())).collect::<Vec<_>>()), (1, vec![(1, vec!["fifth"])]), "the step after the pushed one keeps its own commit");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_bash_hook_notes_nothing_while_head_stays_where_it_was_noted() {
        let (dir, origin, slot, _, state, address) = hooked_task();
        let noted = load(&state, &address).unwrap();
        assert_eq!(noted.held.len(), 1);
        save(&state, &address, &Plan { held: Vec::new(), ..noted }).unwrap();
        assert!(!noted_after(&state, &address, &bash_in(&slot.path, "ls -la")), "an unchanged HEAD notes nothing");
        assert!(load(&state, &address).unwrap().held.is_empty(), "and leaves the steps as they were");
        commit(&slot.path, "e", "fifth");
        let mut edit = bash_in(&slot.path, "git commit -m fifth");
        edit["tool_name"] = json!("Edit");
        assert!(!noted_after(&state, &address, &edit), "only Bash is noted after");
        assert!(!noted_after(&state, "search@app", &bash_in(&slot.path, "git commit -m fifth")), "a task without steps notes nothing");
        assert!(noted_after(&state, &address, &bash_in(&slot.path, "git commit -m fifth")));
        assert_eq!(load(&state, &address).unwrap().held[0].seen.len(), 2, "both commits are the step in progress's");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn steps_done_marks_the_step_in_progress_ready_once_it_holds_a_commit_and_only_by_its_own_task() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let task = Task { path: &slot.path, upstream: &upstream, address: "billing@app" };
        assert!(done_as(&Caller::User, &state, &task).unwrap_err().contains("has no steps"));
        list_at(&state, &slot.path, &upstream, "billing@app", &["Model", "Command"]).unwrap();
        assert!(done_as(&Caller::Task("search@app".to_string()), &state, &task).unwrap_err().contains("writes its own steps"));
        done_as(&Caller::Task("billing@app".to_string()), &state, &task).unwrap();
        let steps = listed_at(&state, &slot.path, &upstream, "billing@app");
        assert_eq!((&steps.items[0].state, steps.items[0].commits.len(), &steps.items[1].state), (&State::Ready, 1, &State::Planned));
        let refused = done_as(&Caller::Lead, &state, &task).unwrap_err();
        assert!(refused.contains("Step 2 holds no commit yet"), "{refused}");
        commit(&slot.path, "e", "fifth");
        done_as(&Caller::Lead, &state, &task).unwrap();
        assert_eq!(states(&listed_at(&state, &slot.path, &upstream, "billing@app")), [State::Ready, State::Ready]);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_boards_read_shows_the_steps_and_notes_nothing() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        list_at(&state, &slot.path, &upstream, "billing@app", &["Model", "Command"]).unwrap();
        let before = load(&state, "billing@app").unwrap();
        let tips = tips_of(&Task { path: &slot.path, upstream: &upstream, address: "billing@app" }).unwrap();
        let read = read_at(&state, &slot.path, "billing@app", &tips, false);
        assert_eq!((read.items.len(), &read.items[0].state, &read.items[0].commits), (2, &State::InProgress, &vec![Commit { sha: tips.head.clone(), subject: "fourth".to_string(), accepted: false }]));
        assert_eq!(load(&state, "billing@app").unwrap(), before, "a read writes no plan");
        assert_eq!(listed_at(&state, &slot.path, &upstream, "billing@app"), read, "kebbie steps reads the same");
        assert_ne!(load(&state, "billing@app").unwrap().held, before.held, "and it notes the commit");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_writes_only_its_own_steps_and_the_lead_and_the_user_write_any() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let task = Task { path: &slot.path, upstream: &upstream, address: "billing@app" };
        let refused = write_as(&Caller::Task("search@app".to_string()), &state, &task, &titles(&["Model"])).unwrap_err();
        assert!(refused.contains("writes its own steps"), "{refused}");
        assert_eq!(load(&state, "billing@app"), None);
        for caller in [Caller::Task("billing@app".to_string()), Caller::Lead, Caller::User] {
            write_as(&caller, &state, &task, &titles(&["Model"])).unwrap();
        }
        let written = load(&state, "billing@app").unwrap();
        assert_eq!((written.titles, written.base), (titles(&["Model"]), git(&slot.path, &["rev-parse", &upstream]).unwrap()));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_shown_steps_mark_each_state_with_its_commits_and_say_when_there_are_none() {
        let listed = plan(&["Model", "Command", "Docs", "Tests"], vec![Held { seen: seen(&["a"]), done: true }, Held { seen: vec![Seen { code: "code of abcdef0123".to_string(), subject: String::new() }, Seen { code: "code of 1234567890".to_string(), subject: String::new() }], done: true }]);
        let landed = ["a".to_string()];
        let commits = [commit_of("abcdef0123", "Add the command", false), commit_of("1234567890", "Wire the command", true), commit_of("fedcba9876", "Write the docs", false)];
        let text = shown("billing@app", &derive(Some(&listed), &landed, &commits, true).0);
        assert_eq!(text, "billing@app's steps:\n  ↑ 1 pushed\n  ● 2. Command  ready · 2 commits\n       abcdef0 Add the command\n       1234567 Wire the command\n  ◐ 3. Docs  in progress · 1 commit\n       fedcba9 Write the docs\n  ○ 4. Tests");
        assert!(shown("billing@app", &Steps::default()).contains("has no steps"));
        let value = to_json("billing@app", &derive(Some(&listed), &landed, &commits, false).0, Some("abc"));
        assert_eq!((value["pushed"].as_u64(), value["last_accepted"].as_str(), value["steps"][0]["state"].as_str(), value["steps"][1]["state"].as_str(), value["steps"][2]["state"].as_str()), (Some(1), Some("abc"), Some("ready"), Some("in-progress"), Some("planned")));
        assert_eq!(value["steps"][0]["commits"], json!([{ "sha": "abcdef0123", "subject": "Add the command", "accepted": false }, { "sha": "1234567890", "subject": "Wire the command", "accepted": true }]));
    }
}
