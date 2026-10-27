use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::peers;
use crate::permit::Caller;
use crate::repo::{default_branch, git, main_checkout, name_of};
use crate::sh::Result;
use super::pool::{Slot, find_task, task_of};

// The task's steps in order, one commit each, the default branch's head when they were first listed, and the change each step's commit made when last seen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub titles: Vec<String>,
    pub base: String,
    pub seen: Vec<Seen>,
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
    Committed { sha: String, subject: String, accepted: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub index: usize,
    pub title: String,
    pub state: State,
    pub extra: bool,
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

pub fn load(state: &Path, address: &str) -> Option<Plan> {
    let value: Value = serde_json::from_str(&fs::read_to_string(file(state, address)).ok()?).ok()?;
    let titles: Vec<String> = value["titles"].as_array()?.iter().filter_map(|title| title.as_str().map(str::to_string)).collect();
    let seen = value["seen"].as_array().map(|seen| seen.iter().map(|one| Seen { code: one["code"].as_str().unwrap_or_default().to_string(), subject: one["subject"].as_str().unwrap_or_default().to_string() }).collect()).unwrap_or_default();
    Some(Plan { titles, base: value["base"].as_str().unwrap_or_default().to_string(), seen })
}

fn save(state: &Path, address: &str, plan: &Plan) -> Result<()> {
    let seen: Vec<Value> = plan.seen.iter().map(|one| json!({ "code": one.code, "subject": one.subject })).collect();
    crate::state::write(&file(state, address), json!({ "titles": plan.titles, "base": plan.base, "seen": seen }).to_string())
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

// The first `pushed` steps stay first as they were, a pushed commit beyond the list under its subject; a list that starts with them again takes them as said.
pub fn rewritten(old: Option<&Plan>, pushed: usize, base: &str, titles: &[String]) -> Result<Plan> {
    let titles: Vec<String> = titles.iter().map(|title| one_line(title)).collect();
    if titles.is_empty() || titles.iter().any(String::is_empty) {
        return Err(format!("kebbie steps needs each step as a one-line title: {USAGE}"));
    }
    let Some(old) = old else { return Ok(Plan { titles, base: base.to_string(), seen: Vec::new() }) };
    let pushed = pushed.min(old.seen.len());
    let kept: Vec<String> = (0..pushed).map(|at| old.titles.get(at).cloned().unwrap_or_else(|| old.seen[at].subject.clone())).collect();
    let rest = if titles.starts_with(&kept) { titles[kept.len()..].to_vec() } else { titles };
    Ok(Plan { titles: kept.into_iter().chain(rest).collect(), base: old.base.clone(), seen: old.seen[..pushed].to_vec() })
}

// Pushed are the leading steps whose last seen change the default branch gained since base, however it got there; landed holds those changes.
pub fn derive(plan: Option<&Plan>, landed: &[String], commits: &[Unpushed], working: bool) -> (Steps, Vec<Seen>) {
    let Some(plan) = plan.filter(|plan| !plan.titles.is_empty()) else { return (Steps::default(), Vec::new()) };
    let mut landed: Vec<&str> = landed.iter().map(String::as_str).collect();
    let pushed = plan.seen.iter().take_while(|seen| !seen.code.is_empty() && taken(&mut landed, &seen.code)).count();
    let mut gone: Vec<&str> = plan.seen[..pushed].iter().map(|seen| seen.code.as_str()).collect();
    let open: Vec<&Unpushed> = commits.iter().filter(|commit| !taken(&mut gone, &commit.code)).collect();
    let mut items: Vec<Step> = open.iter().enumerate().map(|(at, commit)| Step {
        index: pushed + at,
        title: plan.titles.get(pushed + at).cloned().unwrap_or_else(|| commit.subject.clone()),
        state: State::Committed { sha: commit.sha.clone(), subject: commit.subject.clone(), accepted: commit.accepted },
        extra: pushed + at >= plan.titles.len(),
    }).collect();
    for index in (pushed + open.len())..plan.titles.len() {
        let state = if working && index == pushed + open.len() { State::InProgress } else { State::Planned };
        items.push(Step { index, title: plan.titles[index].clone(), state, extra: false });
    }
    let seen = plan.seen[..pushed].iter().cloned().chain(open.iter().map(|commit| Seen { code: commit.code.clone(), subject: commit.subject.clone() })).collect();
    (Steps { pushed, items }, seen)
}

// Takes one of code out of codes, so a change made twice is counted twice.
fn taken(codes: &mut Vec<&str>, code: &str) -> bool {
    codes.iter().position(|one| *one == code).map(|at| codes.remove(at)).is_some()
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
    let shas = git(task.path, &["rev-parse", task.upstream, "HEAD"])?;
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

fn current_at(state: &Path, path: &Path, address: &str, plan: &Plan, tips: &Tips, working: bool) -> Result<(Steps, Vec<Seen>)> {
    Ok(derive(Some(plan), &landed(path, &plan.base, tips), &unpushed(state, path, address, tips)?, working))
}

fn current(state: &Path, task: &Task, plan: &Plan, working: bool) -> Result<(Steps, Vec<Seen>)> {
    current_at(state, task.path, task.address, plan, &tips_of(task)?, working)
}

// What the board reads for a task on what it already holds, task.steps on; it notes nothing, which the hook, a report, a push and kebbie steps do.
pub fn read_at(state: &Path, path: &Path, address: &str, tips: &Tips, working: bool) -> Steps {
    let Some(plan) = load(state, address) else { return Steps::default() };
    current_at(state, path, address, &plan, tips, working).map(|(steps, _)| steps).unwrap_or_default()
}

// Records what each step's commit is now, so a step still counts once its commit reaches the default branch by any road.
fn listed_in(state: &Path, task: &Task, working: bool) -> Result<Steps> {
    let Some(mut plan) = load(state, task.address) else { return Ok(Steps::default()) };
    let (steps, seen) = current(state, task, &plan, working)?;
    if seen != plan.seen && steps.listed() {
        plan.seen = seen;
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

const USAGE: &str = "kebbie steps \"<step>\" \"<step>\" … in the task's worktree, or kebbie steps <name> \"<step>\" … elsewhere; kebbie steps [name] [--json] shows them";

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
        write_as(&caller, &state, &task, titles)?;
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
fn write_as(caller: &Caller, state: &Path, task: &Task, titles: &[String]) -> Result<()> {
    if let Caller::Task(own) = caller && own != task.address {
        return Err(format!("the task {own} writes its own steps, not {}'s", task.address));
    }
    let old = load(state, task.address);
    let (pushed, seen) = match &old {
        Some(plan) => current(state, task, plan, false).map(|(steps, seen)| (steps.pushed, seen))?,
        None => (0, Vec::new()),
    };
    let base = git(task.path, &["rev-parse", task.upstream])?;
    let old = old.map(|plan| Plan { seen, ..plan });
    save(state, task.address, &rewritten(old.as_ref(), pushed, &base, titles)?)
}

pub fn mark(state: &State) -> &'static str {
    match state {
        State::Planned => "○",
        State::InProgress => "◐",
        State::Committed { accepted: false, .. } => "●",
        State::Committed { accepted: true, .. } => "✓",
    }
}

fn shown(address: &str, steps: &Steps) -> String {
    if !steps.listed() {
        return format!("{address} has no steps: kebbie steps \"<step>\" … lists them, a commit per step.");
    }
    let mut lines = vec![format!("{address}'s steps:")];
    if steps.pushed > 0 {
        lines.push(format!("  ↑ {} pushed", steps.pushed));
    }
    for item in &steps.items {
        let mut line = format!("  {} {}. {}", mark(&item.state), item.index + 1, item.title);
        if let State::Committed { sha, .. } = &item.state {
            line.push_str(&format!("  {}", &sha[..7.min(sha.len())]));
        }
        if item.extra {
            line.push_str("  (not in the list)");
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn to_json(address: &str, steps: &Steps, last_accepted: Option<&str>) -> Value {
    let items: Vec<Value> = steps.items.iter().map(|item| {
        let mut value = json!({ "index": item.index, "title": item.title, "extra": item.extra });
        match &item.state {
            State::Planned => value["state"] = json!("planned"),
            State::InProgress => value["state"] = json!("in-progress"),
            State::Committed { sha, subject, accepted } => {
                value["state"] = json!("committed");
                value["sha"] = json!(sha);
                value["subject"] = json!(subject);
                value["accepted"] = json!(accepted);
            }
        }
        value
    }).collect();
    json!({ "task": address, "listed": steps.listed(), "pushed": steps.pushed, "steps": items, "last_accepted": last_accepted })
}

#[cfg(test)]
pub(crate) fn list_at(state: &Path, path: &Path, upstream: &str, address: &str, titles: &[&str]) -> Result<()> {
    let titles: Vec<String> = titles.iter().map(|title| title.to_string()).collect();
    write_as(&Caller::User, state, &Task { path, upstream, address }, &titles)
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

    fn plan(words: &[&str], seen: Vec<Seen>) -> Plan {
        Plan { titles: titles(words), base: "base".to_string(), seen }
    }

    fn subjects(steps: &Steps) -> Vec<(usize, String)> {
        steps.items.iter().map(|item| (item.index, match &item.state { State::Committed { subject, .. } => subject.clone(), _ => String::new() })).collect()
    }

    #[test]
    fn a_rewrite_changes_what_is_not_pushed_and_keeps_the_pushed_steps_first() {
        let first = rewritten(None, 0, "base", &titles(&["Add the model", "  Wire\nthe command ", "Document it"])).unwrap();
        assert_eq!(first, plan(&["Add the model", "Wire the command", "Document it"], Vec::new()), "each title is one line");
        let pushed = Plan { seen: seen(&["a", "b"]), ..first };
        assert_eq!(rewritten(Some(&pushed), 1, "later", &titles(&["Wire it differently", "Test it"])).unwrap(), plan(&["Add the model", "Wire it differently", "Test it"], seen(&["a"])), "the base stays the one the steps were first listed on");
        assert_eq!(rewritten(Some(&pushed), 1, "later", &titles(&["Add the model", "Test it"])).unwrap().titles, titles(&["Add the model", "Test it"]), "a list that repeats the pushed ones first is taken as the whole list");
        assert_eq!(rewritten(Some(&pushed), 1, "later", &titles(&["Add the model"])).unwrap().titles, titles(&["Add the model"]), "a list of just the pushed ones is the whole list, not them twice");
        let beyond = Plan { titles: titles(&["Model"]), base: "base".to_string(), seen: seen(&["a", "b"]) };
        assert_eq!(rewritten(Some(&beyond), 2, "base", &titles(&["Docs"])).unwrap().titles, titles(&["Model", "subject of b", "Docs"]), "a pushed commit beyond the list keeps its subject");
        assert!(rewritten(Some(&pushed), 1, "base", &[]).is_err() && rewritten(None, 0, "base", &titles(&["ok", " "])).is_err(), "no empty list and no empty title");
    }

    #[test]
    fn steps_map_the_commits_in_order_after_the_pushed_ones_and_the_next_is_in_progress_only_while_at_work() {
        let listed = plan(&["Model", "Command", "Docs", "Tests"], seen(&["landed"]));
        let commits = [commit_of("aaa", "Add the command", true)];
        let (at_work, seen_now) = derive(Some(&listed), &["landed".to_string()], &commits, true);
        assert_eq!(at_work.pushed, 1);
        assert_eq!(at_work.items, vec![
            Step { index: 1, title: "Command".to_string(), state: State::Committed { sha: "aaa".to_string(), subject: "Add the command".to_string(), accepted: true }, extra: false },
            Step { index: 2, title: "Docs".to_string(), state: State::InProgress, extra: false },
            Step { index: 3, title: "Tests".to_string(), state: State::Planned, extra: false },
        ]);
        assert_eq!(seen_now.iter().map(|one| one.code.as_str()).collect::<Vec<_>>(), ["landed", "code of aaa"], "each step's commit is noted as it is now");
        assert_eq!(derive(Some(&listed), &["landed".to_string()], &commits, false).0.items[1].state, State::Planned, "nobody is at work on it");
        assert_eq!(derive(Some(&listed), &[], &commits, true).0.pushed, 0, "a change the default branch did not gain is not pushed");
    }

    #[test]
    fn a_landed_step_counts_once_even_while_its_commit_is_still_on_the_branch_and_only_a_leading_run_counts() {
        let listed = plan(&["Model", "Command", "Docs"], seen(&["code of aaa", "code of bbb"]));
        let commits = [commit_of("aaa", "Add the model", false), commit_of("bbb", "Add the command", false)];
        let (steps, _) = derive(Some(&listed), &["code of aaa".to_string()], &commits, false);
        assert_eq!((steps.pushed, subjects(&steps)), (1, vec![(1, "Add the command".to_string()), (2, String::new())]), "a cherry-picked copy on main takes its step out of sight");
        let (steps, _) = derive(Some(&listed), &["code of bbb".to_string()], &commits, false);
        assert_eq!(steps.pushed, 0, "the second step alone on main is no pushed run");
        assert_eq!(steps.items.len(), 3, "and nothing is hidden");
    }

    #[test]
    fn an_open_commit_with_the_same_change_as_a_pushed_step_stays_in_sight() {
        let same = |sha: &str| Unpushed { sha: sha.to_string(), subject: format!("subject {sha}"), code: "same".to_string(), accepted: false };
        let listed = plan(&["Model", "Again"], seen(&["same", "same"]));
        let (steps, _) = derive(Some(&listed), &["same".to_string()], &[same("aaa"), same("bbb")], false);
        assert_eq!((steps.pushed, subjects(&steps)), (1, vec![(1, "subject bbb".to_string())]), "one landed copy pushes one step and hides one commit");
        let (steps, _) = derive(Some(&listed), &["same".to_string(), "same".to_string()], &[same("aaa"), same("bbb")], false);
        assert_eq!((steps.pushed, steps.items.len()), (2, 0), "two landed copies push both");
    }

    #[test]
    fn commits_beyond_the_list_show_as_extra_steps_titled_by_their_subject() {
        let listed = plan(&["Model"], Vec::new());
        let (found, _) = derive(Some(&listed), &[], &[commit_of("aaa", "Add the model", false), commit_of("bbb", "Fix a typo", false)], true);
        assert_eq!(found.items.len(), 2, "nothing is hidden");
        assert_eq!((found.items[1].index, found.items[1].title.as_str(), found.items[1].extra), (1, "Fix a typo", true));
        assert!(!found.items[0].extra && found.items[0].title == "Model");
        assert!(!found.items.iter().any(|item| item.state == State::InProgress), "no planned step is left to be under way");
    }

    #[test]
    fn a_task_without_steps_has_none_and_no_pushed_count() {
        let (none, _) = derive(None, &[], &[commit_of("aaa", "Add the model", false)], true);
        assert_eq!(none, Steps::default());
        assert!(!none.listed(), "callers fall back to the rows they show today");
        assert!(!derive(Some(&Plan::default()), &[], &[commit_of("aaa", "x", false)], true).0.listed());
        assert!(derive(Some(&plan(&["Model"], seen(&["a"]))), &["a".to_string()], &[], false).0.listed(), "a list pushed in full is still a list");
    }

    fn landed_task() -> (PathBuf, PathBuf, crate::tasks::Slot, String, PathBuf, String) {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        let state = state_of(&dir);
        list_at(&state, &slot.path, &upstream, "billing@app", &["Model", "Command"]).unwrap();
        assert_eq!(listed_at(&state, &slot.path, &upstream, "billing@app").pushed, 0);
        (dir, origin, slot, upstream, state, fourth)
    }

    fn after_landing(state: &Path, slot: &crate::tasks::Slot, upstream: &str) -> (usize, Vec<(usize, String)>) {
        let steps = listed_at(state, &slot.path, upstream, "billing@app");
        (steps.pushed, subjects(&steps))
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
        commit(&slot.path, "e", "fifth");
        assert!(noted_after(&state, &address, &bash_in(&slot.path, "git commit -m fifth")));
        git(&dir, &["push", "--quiet", "origin", &format!("{fourth}:refs/heads/{}", upstream.trim_start_matches("origin/"))]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        let steps = listed_at(&state, &slot.path, &upstream, &address);
        assert_eq!((steps.pushed, subjects(&steps)), (1, vec![(1, "fifth".to_string())]), "the step after the pushed one keeps its own commit");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_bash_hook_notes_nothing_while_head_stays_where_it_was_noted() {
        let (dir, origin, slot, _, state, address) = hooked_task();
        let noted = load(&state, &address).unwrap();
        assert_eq!(noted.seen.len(), 1);
        save(&state, &address, &Plan { seen: Vec::new(), ..noted }).unwrap();
        assert!(!noted_after(&state, &address, &bash_in(&slot.path, "ls -la")), "an unchanged HEAD notes nothing");
        assert!(load(&state, &address).unwrap().seen.is_empty(), "and leaves the steps as they were");
        commit(&slot.path, "e", "fifth");
        let mut edit = bash_in(&slot.path, "git commit -m fifth");
        edit["tool_name"] = json!("Edit");
        assert!(!noted_after(&state, &address, &edit), "only Bash is noted after");
        assert!(!noted_after(&state, "search@app", &bash_in(&slot.path, "git commit -m fifth")), "a task without steps notes nothing");
        assert!(noted_after(&state, &address, &bash_in(&slot.path, "git commit -m fifth")));
        assert_eq!(load(&state, &address).unwrap().seen.len(), 2);
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
        assert_eq!((read.items.len(), &read.items[0].state), (2, &State::Committed { sha: tips.head.clone(), subject: "fourth".to_string(), accepted: false }));
        assert_eq!(load(&state, "billing@app").unwrap(), before, "a read writes no plan");
        assert_eq!(listed_at(&state, &slot.path, &upstream, "billing@app"), read, "kebbie steps reads the same");
        assert_ne!(load(&state, "billing@app").unwrap().seen, before.seen, "and it notes the commit");
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
    fn the_shown_steps_mark_each_state_and_say_when_there_are_none() {
        let listed = plan(&["Model", "Command", "Docs"], seen(&["a"]));
        let landed = ["a".to_string()];
        let text = shown("billing@app", &derive(Some(&listed), &landed, &[commit_of("abcdef0123", "Add the command", false)], true).0);
        assert_eq!(text, "billing@app's steps:\n  ↑ 1 pushed\n  ● 2. Command  abcdef0\n  ◐ 3. Docs");
        assert!(shown("billing@app", &Steps::default()).contains("has no steps"));
        let value = to_json("billing@app", &derive(Some(&listed), &landed, &[commit_of("abc", "Add the command", true)], false).0, Some("abc"));
        assert_eq!((value["pushed"].as_u64(), value["last_accepted"].as_str(), value["steps"][0]["state"].as_str(), value["steps"][0]["accepted"].as_bool(), value["steps"][1]["state"].as_str()), (Some(1), Some("abc"), Some("committed"), Some(true), Some("planned")));
    }
}
