use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::peers;
use crate::permit::Caller;
use crate::repo::{default_branch, git, main_checkout, name_of};
use crate::sh::Result;
use super::pool::{Slot, find_task, task_of};

// The task's subtasks in plan order, the default branch's head when they were first listed, each commit kebbie last saw on the branch with the subtask it belongs to, and the subtask new commits go to when the task named one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub base: String,
    pub parts: Vec<Part>,
    pub commits: Vec<Seen>,
    pub on: Option<String>,
}

// A subtask as the plan file holds it: done is the task's word that its work is finished.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    pub id: String,
    pub title: String,
    pub done: bool,
    pub extra: bool,
    pub added: Option<Added>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Added {
    pub at: u64,
    pub reason: String,
    pub by: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    pub sha: String,
    pub code: String,
    pub subject: String,
    pub subtask: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Planned,
    InProgress,
    Ready,
    Accepted,
    Pushed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub accepted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Subtask {
    pub id: String,
    pub number: usize,
    pub title: String,
    pub state: State,
    pub commits: Vec<Commit>,
    pub pushed: Vec<String>,
    pub extra: bool,
    pub added: Option<Added>,
    pub contiguous: bool,
    pub waits_for: Option<String>,
}

impl Subtask {
    pub fn head(&self) -> Option<&str> {
        self.commits.last().map(|commit| commit.sha.as_str())
    }

    pub fn holds(&self, sha: &str) -> bool {
        self.commits.iter().any(|commit| commit.sha == sha)
    }
}

// One open commit in the push order, top first, with the subtask it belongs to.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub sha: String,
    pub subject: String,
    pub subtask: String,
}

// The longest run from the top of the push order whose every commit's subtask is accepted.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub count: usize,
    pub through: String,
    pub subtasks: Vec<String>,
}

// The subtasks as they stand: pushed ones apart, the rest in plan order, and the push order; a task without a plan has none, so callers show what they show without subtasks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subtasks {
    pub pushed: Vec<Subtask>,
    pub items: Vec<Subtask>,
    pub order: Vec<Placed>,
    pub pushable: Option<Run>,
}

impl Subtasks {
    pub fn listed(&self) -> bool {
        !self.pushed.is_empty() || !self.items.is_empty()
    }

    pub fn holding(&self, sha: &str) -> Option<&Subtask> {
        self.items.iter().find(|subtask| subtask.holds(sha))
    }

    pub fn named(&self, id: &str) -> Option<&Subtask> {
        self.items.iter().chain(&self.pushed).find(|subtask| subtask.id == id)
    }

    // A subtask by its id, else by the number it is shown with.
    pub fn find(&self, said: &str) -> Option<&Subtask> {
        self.named(said).or_else(|| said.parse::<usize>().ok().and_then(|number| self.items.iter().chain(&self.pushed).find(|subtask| subtask.number == number)))
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

fn lock_file(state: &Path, address: &str) -> PathBuf {
    state.join("steps").join(format!("{address}.lock"))
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

fn added_of(value: &Value) -> Option<Added> {
    let reason = value["reason"].as_str().filter(|reason| !reason.trim().is_empty())?;
    Some(Added { at: value["at"].as_u64().unwrap_or_default(), reason: reason.to_string(), by: text(value, "by") })
}

// The plan in its own shape, or an older kebbie's steps: each step becomes the subtask its number names, and a step's commits become that subtask's.
pub fn load(state: &Path, address: &str) -> Option<Plan> {
    let value: Value = serde_json::from_str(&fs::read_to_string(file(state, address)).ok()?).ok()?;
    let base = text(&value, "base");
    if let Some(parts) = value["subtasks"].as_array() {
        let parts = parts.iter().map(|part| Part { id: text(part, "id"), title: text(part, "title"), done: part["done"] == true, extra: part["extra"] == true, added: added_of(&part["added"]) }).filter(|part| !part.id.is_empty()).collect();
        let commits = value["commits"].as_array().map(|commits| commits.iter().map(|seen| Seen { sha: text(seen, "sha"), code: text(seen, "code"), subject: text(seen, "subject"), subtask: text(seen, "subtask") }).collect()).unwrap_or_default();
        return Some(Plan { base, parts, commits, on: value["on"].as_str().map(str::to_string) });
    }
    Some(migrated(&value, base))
}

fn migrated(value: &Value, base: String) -> Plan {
    let titles: Vec<String> = value["titles"].as_array().map(|titles| titles.iter().filter_map(|title| title.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let seen_of = |one: &Value| (text(one, "code"), text(one, "subject"));
    let held: Vec<(Vec<(String, String)>, bool)> = match (value["held"].as_array(), value["seen"].as_array()) {
        (Some(held), _) => held.iter().map(|step| (step["seen"].as_array().map(|seen| seen.iter().map(seen_of).collect()).unwrap_or_default(), step["done"] == true)).collect(),
        (None, Some(seen)) => seen.iter().map(|one| (vec![seen_of(one)], true)).collect(),
        (None, None) => Vec::new(),
    };
    let count = titles.len().max(held.len());
    let parts = (0..count).map(|at| {
        let (seen, done) = held.get(at).cloned().unwrap_or_default();
        let title = titles.get(at).cloned().unwrap_or_else(|| seen.first().map(|(_, subject)| subject.clone()).unwrap_or_default());
        Part { id: (at + 1).to_string(), title, done, extra: at >= titles.len(), added: None }
    }).collect();
    let commits = held.iter().enumerate().flat_map(|(at, (seen, _))| seen.iter().map(move |(code, subject)| Seen { sha: String::new(), code: code.clone(), subject: subject.clone(), subtask: (at + 1).to_string() })).collect();
    Plan { base, parts, commits, on: None }
}

fn save(state: &Path, address: &str, plan: &Plan) -> Result<()> {
    let parts: Vec<Value> = plan.parts.iter().map(|part| {
        let mut shown = json!({ "id": part.id, "title": part.title, "done": part.done });
        if part.extra {
            shown["extra"] = json!(true);
        }
        if let Some(added) = &part.added {
            shown["added"] = json!({ "at": added.at, "reason": added.reason, "by": added.by });
        }
        shown
    }).collect();
    let commits: Vec<Value> = plan.commits.iter().map(|seen| json!({ "sha": seen.sha, "code": seen.code, "subject": seen.subject, "subtask": seen.subtask })).collect();
    crate::state::write(&file(state, address), json!({ "base": plan.base, "subtasks": parts, "commits": commits, "on": plan.on }).to_string())
}

pub fn forget(task: &str, repo: &Path) {
    if let Ok(state) = state_dir() {
        let address = peers::address(task, &name_of(repo));
        for path in [file(&state, &address), head_file(&state, &address), lock_file(&state, &address)] {
            let _ = fs::remove_file(path);
        }
    }
}

fn one_line(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

const SMALL_WORDS: [&str; 16] = ["a", "an", "and", "as", "at", "by", "for", "from", "in", "into", "it", "its", "of", "on", "the", "to"];

// A short id from a title's first words, kept apart from the ids already taken.
pub fn new_id(title: &str, taken: &[&str]) -> String {
    let folded: String = super::ascii_fold(&title.to_lowercase()).chars().map(|character| if character.is_ascii_alphanumeric() { character } else { ' ' }).collect();
    let words: Vec<&str> = folded.split_whitespace().filter(|word| !SMALL_WORDS.contains(word)).take(2).collect();
    let mut stem = words.join("-");
    stem.truncate(24);
    let stem = stem.trim_matches('-').to_string();
    let stem = if stem.is_empty() || stem.bytes().all(|byte| byte.is_ascii_digit()) { format!("s{stem}") } else { stem };
    (1..).map(|again| if again == 1 { stem.clone() } else { format!("{stem}-{again}") }).find(|id| !taken.contains(&id.as_str())).unwrap_or(stem)
}

fn ids(parts: &[Part]) -> Vec<&str> {
    parts.iter().map(|part| part.id.as_str()).collect()
}

// A list again: a subtask keeps its id and commits under the same title, a changed title takes the place of the unpushed subtask holding commits at its position, a pushed subtask stays first, and one left out that still holds commits stays after the list.
pub fn relisted(old: Option<&Plan>, now: Option<&Subtasks>, base: &str, titles: &[String], because: Option<&Added>) -> Result<Plan> {
    let titles: Vec<String> = titles.iter().map(|title| one_line(title)).collect();
    if titles.is_empty() || titles.iter().any(String::is_empty) {
        return Err(format!("kebbie subtasks needs each subtask as a one-line title: {USAGE}"));
    }
    let Some(old) = old else {
        let mut parts: Vec<Part> = Vec::new();
        for title in titles {
            let id = new_id(&title, &ids(&parts));
            parts.push(Part { id, title, ..Part::default() });
        }
        return Ok(Plan { base: base.to_string(), parts, commits: Vec::new(), on: None });
    };
    let pushed: Vec<&str> = now.map(|now| now.pushed.iter().map(|subtask| subtask.id.as_str()).collect()).unwrap_or_default();
    let holding: Vec<&str> = now.map(|now| now.items.iter().filter(|subtask| !subtask.commits.is_empty()).map(|subtask| subtask.id.as_str()).collect()).unwrap_or_default();
    let mut kept: Vec<Part> = old.parts.iter().filter(|part| pushed.contains(&part.id.as_str())).cloned().collect();
    let titles: Vec<String> = titles.into_iter().filter(|title| !kept.iter().any(|part| part.title == *title)).collect();
    let open: Vec<&Part> = old.parts.iter().filter(|part| !pushed.contains(&part.id.as_str())).collect();
    let mut used: Vec<&str> = Vec::new();
    let mut chosen: Vec<Option<&Part>> = titles.iter().map(|title| open.iter().find(|part| part.title == *title).copied()).collect();
    used.extend(chosen.iter().flatten().map(|part| part.id.as_str()));
    for (at, choice) in chosen.iter_mut().enumerate() {
        if choice.is_none() && let Some(part) = open.get(at).filter(|part| !used.contains(&part.id.as_str()) && holding.contains(&part.id.as_str())) {
            *choice = Some(part);
            used.push(part.id.as_str());
        }
    }
    for (title, choice) in titles.iter().zip(chosen) {
        let part = match choice {
            Some(part) => Part { title: title.clone(), extra: false, ..part.clone() },
            None => Part { id: new_id(title, &[ids(&kept), ids(&old.parts)].concat()), title: title.clone(), added: because.cloned(), ..Part::default() },
        };
        kept.push(part);
    }
    kept.extend(open.iter().filter(|part| !used.contains(&part.id.as_str()) && holding.contains(&part.id.as_str())).map(|part| Part { extra: true, ..(*part).clone() }));
    let on = old.on.clone().filter(|on| kept.iter().any(|part| part.id == *on));
    Ok(Plan { base: old.base.clone(), parts: kept, commits: old.commits.clone(), on })
}

// The subtask new commits go to: the one the task named, else the first not done; None when every one is done.
fn current(plan: &Plan) -> Option<String> {
    plan.on.clone().filter(|on| plan.parts.iter().any(|part| part.id == *on)).or_else(|| plan.parts.iter().find(|part| !part.done).map(|part| part.id.clone()))
}

// Takes one of code out of codes, so a change made twice is counted twice.
fn taken(codes: &mut Vec<&str>, code: &str) -> bool {
    codes.iter().position(|one| *one == code).map(|at| codes.remove(at)).is_some()
}

// Each open commit's subtask: the one that held its change; a change rewritten by an amend or a squash goes to the subtask beside it that lost one; any other to the subtask commits go to now, or to a new one titled by it once every subtask is done.
fn placed(plan: &mut Plan, kept: &[bool], open: &[&Unpushed]) -> Vec<String> {
    let mut wanted: Vec<Option<(String, String)>> = plan.commits.iter().zip(kept).map(|(seen, pushed)| (!pushed).then(|| (seen.code.clone(), seen.subtask.clone()))).collect();
    let matched: Vec<Option<String>> = open.iter().map(|commit| {
        let at = wanted.iter().position(|want| want.as_ref().is_some_and(|(code, _)| *code == commit.code))?;
        wanted[at].take().map(|(_, subtask)| subtask)
    }).collect();
    let mut lost: Vec<(String, usize)> = Vec::new();
    for (_, subtask) in wanted.into_iter().flatten() {
        match lost.iter_mut().find(|(id, _)| *id == subtask) {
            Some((_, count)) => *count += 1,
            None => lost.push((subtask, 1)),
        }
    }
    let mut take_lost = |id: &str| match lost.iter_mut().find(|(lost, count)| lost == id && *count > 0) {
        Some((_, count)) => {
            *count -= 1;
            true
        }
        None => false,
    };
    let mut placed: Vec<String> = Vec::with_capacity(open.len());
    for (at, found) in matched.iter().enumerate() {
        let id = match found {
            Some(id) => id.clone(),
            None => {
                let before = placed.last().cloned();
                let later = matched[at + 1..].iter().flatten().next().cloned();
                match (before, later) {
                    (Some(before), _) if take_lost(&before) => before,
                    (_, Some(later)) if take_lost(&later) => later,
                    _ => current(plan).unwrap_or_else(|| {
                        let title = open[at].subject.clone();
                        let id = new_id(&title, &ids(&plan.parts));
                        plan.parts.push(Part { id: id.clone(), title, extra: true, ..Part::default() });
                        plan.on = None;
                        id
                    }),
                }
            }
        };
        placed.push(id);
    }
    placed
}

// The subtasks as the branch has them: a commit whose change the default branch gained since base is pushed, however it got there, and a subtask with commits and none left open is pushed; the plan comes back with what each subtask holds now, to be noted.
pub fn derive(plan: Option<&Plan>, landed: &[String], commits: &[Unpushed], working: bool) -> (Subtasks, Plan) {
    let Some(plan) = plan.filter(|plan| !plan.parts.is_empty()) else { return (Subtasks::default(), Plan::default()) };
    let mut plan = plan.clone();
    let mut landed: Vec<&str> = landed.iter().map(String::as_str).collect();
    let kept: Vec<bool> = plan.commits.iter().map(|seen| !seen.code.is_empty() && taken(&mut landed, &seen.code)).collect();
    let mut gone: Vec<&str> = plan.commits.iter().zip(&kept).filter(|(_, pushed)| **pushed).map(|(seen, _)| seen.code.as_str()).collect();
    let open: Vec<&Unpushed> = commits.iter().filter(|commit| !taken(&mut gone, &commit.code)).collect();
    let owners = placed(&mut plan, &kept, &open);
    let pushed_seen: Vec<Seen> = plan.commits.iter().zip(&kept).filter(|(_, pushed)| **pushed).map(|(seen, _)| seen.clone()).collect();
    plan.commits = pushed_seen.iter().cloned().chain(open.iter().zip(&owners).map(|(commit, subtask)| Seen { sha: commit.sha.clone(), code: commit.code.clone(), subject: commit.subject.clone(), subtask: subtask.clone() })).collect();
    let holds = |id: &str| plan.commits.iter().any(|seen| seen.subtask == id);
    let empty: Vec<String> = plan.parts.iter().filter(|part| !holds(&part.id)).map(|part| part.id.clone()).collect();
    plan.parts.retain(|part| !(part.extra && empty.contains(&part.id)));
    for part in plan.parts.iter_mut().filter(|part| empty.contains(&part.id)) {
        part.done = false;
    }
    if plan.on.as_ref().is_some_and(|on| !plan.parts.iter().any(|part| part.id == *on)) {
        plan.on = None;
    }
    let now = current(&plan);
    let order: Vec<Placed> = open.iter().zip(&owners).map(|(commit, subtask)| Placed { sha: commit.sha.clone(), subject: commit.subject.clone(), subtask: subtask.clone() }).collect();
    let mut all: Vec<Subtask> = plan.parts.iter().enumerate().map(|(at, part)| {
        let positions: Vec<usize> = owners.iter().enumerate().filter(|(_, owner)| **owner == part.id).map(|(at, _)| at).collect();
        let commits: Vec<Commit> = positions.iter().map(|at| Commit { sha: open[*at].sha.clone(), subject: open[*at].subject.clone(), accepted: open[*at].accepted }).collect();
        let pushed: Vec<String> = pushed_seen.iter().filter(|seen| seen.subtask == part.id && !seen.sha.is_empty()).map(|seen| seen.sha.clone()).collect();
        let any_pushed = pushed_seen.iter().any(|seen| seen.subtask == part.id);
        let state = match (commits.is_empty(), part.done) {
            (true, _) if any_pushed => State::Pushed,
            (true, _) if working && now.as_deref() == Some(part.id.as_str()) => State::InProgress,
            (true, _) => State::Planned,
            (false, false) => State::InProgress,
            (false, true) if commits.iter().all(|commit| commit.accepted) => State::Accepted,
            (false, true) => State::Ready,
        };
        let contiguous = positions.windows(2).all(|pair| pair[1] == pair[0] + 1);
        Subtask { id: part.id.clone(), number: at + 1, title: part.title.clone(), state, commits, pushed, extra: part.extra, added: part.added.clone(), contiguous, waits_for: None }
    }).collect();
    let accepted = |id: &str, all: &[Subtask]| all.iter().any(|subtask| subtask.id == id && subtask.state == State::Accepted);
    let count = order.iter().take_while(|row| accepted(&row.subtask, &all)).count();
    let pushable = order[..count].last().map(|last| {
        let mut subtasks: Vec<String> = Vec::new();
        for row in &order[..count] {
            if !subtasks.contains(&row.subtask) {
                subtasks.push(row.subtask.clone());
            }
        }
        Run { count, through: last.sha.clone(), subtasks }
    });
    let behind = order.get(count).map(|row| row.subtask.clone());
    let title_of = |id: &str, all: &[Subtask]| all.iter().find(|subtask| subtask.id == id).map(|subtask| subtask.title.clone());
    let waits = behind.as_deref().and_then(|id| title_of(id, &all));
    for subtask in all.iter_mut().filter(|subtask| subtask.state == State::Accepted && !pushable.as_ref().is_some_and(|run| run.subtasks.contains(&subtask.id))) {
        subtask.waits_for = waits.clone();
    }
    let (pushed, items): (Vec<Subtask>, Vec<Subtask>) = all.into_iter().partition(|subtask| subtask.state == State::Pushed);
    (Subtasks { pushed, items, order, pushable }, plan)
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

fn current_at(state: &Path, path: &Path, address: &str, plan: &Plan, tips: &Tips, working: bool) -> Result<(Subtasks, Plan)> {
    Ok(derive(Some(plan), &landed(path, &plan.base, tips), &unpushed(state, path, address, tips)?, working))
}

fn read_in(state: &Path, task: &Task, plan: &Plan, working: bool) -> Result<(Subtasks, Plan)> {
    current_at(state, task.path, task.address, plan, &tips_of(task)?, working)
}

// Whether the task's subtasks hold work beyond `pushed`, the tip just pushed to the default branch: a commit not on it, or a subtask not done; false for a task without subtasks.
pub fn left_at(state: &Path, path: &Path, address: &str, pushed: &str) -> bool {
    let Some(plan) = load(state, address) else { return false };
    tips_of(&Task { path, upstream: pushed, address }).and_then(|tips| current_at(state, path, address, &plan, &tips, false)).is_ok_and(|(subtasks, _)| !subtasks.items.is_empty())
}

// What the board reads for a task on what it already holds, task.steps on; it notes nothing, which the hook, a report, a push and kebbie subtasks do.
pub fn read_at(state: &Path, path: &Path, address: &str, tips: &Tips, working: bool) -> Subtasks {
    let Some(plan) = load(state, address) else { return Subtasks::default() };
    current_at(state, path, address, &plan, tips, working).map(|(subtasks, _)| subtasks).unwrap_or_default()
}

// The subtasks of the task in path as they stand, noting nothing; empty without a plan.
pub(super) fn read_now(state: &Path, path: &Path, upstream: &str, address: &str) -> Subtasks {
    tips_of(&Task { path, upstream, address }).map(|tips| read_at(state, path, address, &tips, false)).unwrap_or_default()
}

// Read, change and write the plan under the task's own lock, so the hook, the task, the hub and the investigator never write over each other.
fn changed<T>(state: &Path, address: &str, change: impl FnOnce(Option<Plan>) -> Result<(Option<Plan>, T)>) -> Result<T> {
    let _held = crate::state::Locked::wait(&lock_file(state, address))?;
    let old = load(state, address);
    let (new, out) = change(old.clone())?;
    if let Some(new) = new.filter(|new| Some(new) != old.as_ref()) {
        save(state, address, &new)?;
    }
    Ok(out)
}

// Records which commits each subtask holds now, so a subtask keeps them through a rebase and they still count once they reach the default branch by any road.
fn listed_in(state: &Path, task: &Task, working: bool) -> Result<Subtasks> {
    changed(state, task.address, |plan| {
        let Some(plan) = plan else { return Ok((None, Subtasks::default())) };
        let (subtasks, noted) = read_in(state, task, &plan, working)?;
        Ok((subtasks.listed().then_some(noted), subtasks))
    })
}

// What the board and the review split read for the task whose worktree is path, empty with task.steps off; working is whether its agent is at work now.
pub fn subtasks_in(state: &Path, path: &Path, working: bool) -> Result<Subtasks> {
    let (repo, slot, address) = located(path)?;
    if !crate::config::resolve(Some(&repo))?.settings.steps.value {
        return Ok(Subtasks::default());
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

// From the task's PostToolUse and PostToolUseFailure hooks on Bash, with task.steps on: notes the subtasks once HEAD moved since they were last noted, so a commit pushed at once with plain git is still its subtask's.
pub fn noted_after(state: &Path, address: &str, input: &Value) -> bool {
    if input["tool_name"] != "Bash" || load(state, address).is_none_or(|plan| plan.parts.is_empty()) {
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

// Notes what each subtask's commit is before it can leave the branch, as a push or a turn's end does.
pub fn seen_in(state: &Path, path: &Path) {
    let _ = subtasks_in(state, path, false);
}

// The same, with no plan and no error to handle: no subtasks.
pub fn for_path(path: &Path, working: bool) -> Subtasks {
    state_dir().and_then(|state| subtasks_in(&state, path, working)).unwrap_or_default()
}

fn working_now(state: &Path, address: &str) -> bool {
    crate::phase::load(state, address).is_some_and(|record| matches!(record.phase, crate::phase::Phase::Working | crate::phase::Phase::Starting))
}

// What kebbie subtasks --json prints for the task whose worktree is path, read in-process and noting nothing; None with task.steps off or outside a task.
pub fn shown_at(path: &Path) -> Option<Value> {
    let (repo, slot, address) = located(path).ok()?;
    if !crate::config::resolve(Some(&repo)).ok()?.settings.steps.value {
        return None;
    }
    let state = state_dir().ok()?;
    let upstream = default_branch(&repo).ok()?;
    let task = Task { path: &slot.path, upstream: &upstream, address: &address };
    let subtasks = read_at(&state, &slot.path, &address, &tips_of(&task).ok()?, working_now(&state, &address));
    Some(to_json(&address, &subtasks, super::accept::last_accepted(&slot.path).as_deref()))
}

const USAGE: &str = "kebbie subtasks \"<subtask>\" \"<subtask>\" … lists them; kebbie subtasks add \"<subtask>\" --because \"<why>\" [--prepares <id>] adds one; kebbie subtasks on <id> sends the next commits to it; kebbie subtasks done [<id>] marks one ready; kebbie subtasks [--json] shows them. In the task's worktree, or with the task's name first elsewhere";

fn off_line(repo: &Path) -> String {
    format!("kebbie subtasks is off for {}: set task.steps = true in {} or {}, or on the board's Settings page.", name_of(repo), crate::config::path().display(), crate::config::repo_file(repo).display())
}

// In a task's worktree every word is the command's; elsewhere the first names the task.
fn target<'a>(repo: &Path, cwd: &Path, words: &'a [String]) -> Result<(Slot, &'a [String])> {
    if let Ok(slot) = find_task(repo, None, cwd) {
        return Ok((slot, words));
    }
    let name = words.first().ok_or_else(|| format!("Not inside a task's worktree; name the task: {USAGE}"))?;
    Ok((find_task(repo, Some(name), cwd)?, &words[1..]))
}

// What kebbie subtasks was asked: a list, or add, on or done.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asked<'a> {
    pub because: Option<&'a str>,
    pub prepares: Option<&'a str>,
    pub as_json: bool,
}

pub fn subtasks(cwd: &Path, words: &[String], asked: &Asked) -> Result<String> {
    let repo = main_checkout(cwd)?;
    if !crate::config::resolve(Some(&repo))?.settings.steps.value {
        return Err(off_line(&repo));
    }
    let (slot, words) = target(&repo, cwd, words)?;
    let address = peers::address(task_of(&slot), &name_of(&repo));
    let upstream = default_branch(&repo)?;
    let task = Task { path: &slot.path, upstream: &upstream, address: &address };
    let state = state_dir()?;
    let mut working = working_now(&state, &address);
    let mut said = None;
    if !words.is_empty() {
        let caller = crate::permit::caller();
        let word = |at: usize| words.get(at).map(String::as_str);
        said = match (word(0), words.len()) {
            (Some("done"), 1 | 2) => Some(done_as(&caller, &state, &task, word(1), true)?),
            (Some("on"), 2) => Some(on_as(&caller, &state, &task, word(1).unwrap_or_default())?),
            (Some("add"), 2) => Some(add_as(&caller, &state, &task, word(1).unwrap_or_default(), asked.because.ok_or_else(|| format!("kebbie subtasks add needs --because \"<why the plan changes>\": {USAGE}"))?, asked.prepares, crate::messages::now())?),
            _ => {
                write_as(&caller, &state, &task, words, asked.because)?;
                None
            }
        };
        working |= caller == Caller::Task(address.clone());
    }
    let subtasks = listed_in(&state, &task, working)?;
    let last = super::accept::last_accepted(&slot.path);
    if asked.as_json {
        return serde_json::to_string_pretty(&to_json(&address, &subtasks, last.as_deref())).map_err(|error| error.to_string());
    }
    let shown = shown(&address, &subtasks);
    Ok(said.map_or(shown.clone(), |said| format!("{said}\n\n{shown}")))
}

// A task writes its own subtasks only; the lead and the user write any task's.
fn writes(caller: &Caller, task: &Task) -> Result<()> {
    match caller {
        Caller::Task(own) if own != task.address => Err(format!("the task {own} writes its own subtasks, not {}'s", task.address)),
        _ => Ok(()),
    }
}

fn by_word(caller: &Caller) -> &'static str {
    match caller {
        Caller::Task(_) => "task",
        Caller::Lead | Caller::LeadSandboxed => "lead",
        Caller::User | Caller::Hub { .. } => "you",
        _ => "kebbie",
    }
}

fn write_as(caller: &Caller, state: &Path, task: &Task, titles: &[String], because: Option<&str>) -> Result<()> {
    writes(caller, task)?;
    let base = git(task.path, &["rev-parse", &crate::repo::tracking(task.upstream)])?;
    let because = because.map(|reason| Added { at: crate::messages::now(), reason: one_line(reason), by: by_word(caller).to_string() });
    changed(state, task.address, |old| {
        let (now, old) = match old {
            Some(plan) => read_in(state, task, &plan, false).map(|(now, noted)| (Some(now), Some(noted)))?,
            None => (None, None),
        };
        Ok((Some(relisted(old.as_ref(), now.as_ref(), &base, titles, because.as_ref())?), ()))
    })
}

fn noted_plan(state: &Path, task: &Task) -> Result<(Subtasks, Plan)> {
    let plan = load(state, task.address).filter(|plan| !plan.parts.is_empty()).ok_or_else(|| format!("{} has no subtasks: {USAGE}", task.address))?;
    read_in(state, task, &plan, false)
}

// kebbie subtasks done [<id>]: the subtask named, else the one commits go to, is ready once it holds a commit.
fn done_as(caller: &Caller, state: &Path, task: &Task, said: Option<&str>, formed: bool) -> Result<String> {
    writes(caller, task)?;
    changed(state, task.address, |_| {
        let (subtasks, mut plan) = noted_plan(state, task)?;
        let id = match said {
            Some(said) => subtasks.find(said).map(|subtask| subtask.id.clone()).ok_or_else(|| format!("{} has no subtask {said}: kebbie subtasks lists them", task.address))?,
            None => current(&plan).ok_or_else(|| "Every subtask is done already".to_string())?,
        };
        let subtask = subtasks.named(&id).cloned().ok_or_else(|| format!("{} has no subtask {id}", task.address))?;
        if subtask.commits.is_empty() {
            return Err(format!("{} holds no commit yet: commit its work, then kebbie subtasks done", subtask.title));
        }
        if formed {
            unformed(task, &subtask)?;
        }
        if let Some(part) = plan.parts.iter_mut().find(|part| part.id == id) {
            part.done = true;
        }
        if plan.on.as_deref() == Some(id.as_str()) {
            plan.on = None;
        }
        Ok((Some(plan), format!("{} is ready for the user [{id}].", subtask.title)))
    })
}

// Rule 1 of kebbie:work-rules on the subtask's own commits, while it is the rules skill in effect, so a subtask goes to the user well formed.
fn unformed(task: &Task, subtask: &Subtask) -> Result<()> {
    let repo = main_checkout(task.path)?;
    if crate::config::resolve(Some(&repo))?.settings.rules_skill.value != crate::config::DEFAULT_RULES_SKILL {
        return Ok(());
    }
    let breaks: Vec<String> = subtask.commits.iter().filter_map(|commit| {
        let message = git(task.path, &["log", "-1", "--format=%B", &commit.sha]).ok()?;
        crate::rules_check::form_break(&message).map(|what| format!("{} {}: {what}", short(&commit.sha), commit.subject))
    }).collect();
    if breaks.is_empty() {
        return Ok(());
    }
    let mut notice = crate::notice::Notice::new(crate::notice::Tone::NotNow, format!("Not marked done: a commit of {} breaks rule 1 of kebbie:work-rules, subject, blank line, body", subtask.title))
        .next("Reword each commit below so its subject is followed by a blank line and a body that says why the change was needed and what the code now does, then mark it done again.");
    for line in breaks {
        notice = notice.detail("commit", line);
    }
    notice.err()
}

// kebbie subtasks on <id>: the next commits go to that subtask, which is under way again.
fn on_as(caller: &Caller, state: &Path, task: &Task, said: &str) -> Result<String> {
    writes(caller, task)?;
    changed(state, task.address, |_| {
        let (subtasks, mut plan) = noted_plan(state, task)?;
        let subtask = subtasks.find(said).cloned().ok_or_else(|| format!("{} has no subtask {said}: kebbie subtasks lists them", task.address))?;
        if subtask.state == State::Pushed {
            return Err(format!("{} is pushed already: add a subtask for what is left", subtask.title));
        }
        if let Some(part) = plan.parts.iter_mut().find(|part| part.id == subtask.id) {
            part.done = false;
        }
        plan.on = Some(subtask.id.clone());
        Ok((Some(plan), format!("The next commits go to {} [{}].", subtask.title, subtask.id)))
    })
}

fn add_as(caller: &Caller, state: &Path, task: &Task, title: &str, because: &str, prepares: Option<&str>, now: u64) -> Result<String> {
    writes(caller, task)?;
    let added = Added { at: now, reason: one_line(because), by: by_word(caller).to_string() };
    if added.reason.is_empty() {
        return Err(format!("kebbie subtasks add needs a reason with --because: {USAGE}"));
    }
    changed(state, task.address, |_| {
        let (subtasks, plan) = noted_plan(state, task)?;
        let before = match prepares {
            Some(said) => Some(subtasks.find(said).map(|subtask| subtask.id.clone()).ok_or_else(|| format!("{} has no subtask {said} to prepare: kebbie subtasks lists them", task.address))?),
            None => None,
        };
        let (plan, id) = inserted(plan, title, before.as_deref(), added)?;
        Ok((Some(plan), format!("Added {} [{id}].", one_line(title))))
    })
}

fn inserted(mut plan: Plan, title: &str, before: Option<&str>, added: Added) -> Result<(Plan, String)> {
    let title = one_line(title);
    if title.is_empty() {
        return Err("a subtask needs a title".to_string());
    }
    let id = new_id(&title, &ids(&plan.parts));
    let at = before.and_then(|before| plan.parts.iter().position(|part| part.id == before)).unwrap_or(plan.parts.len());
    plan.parts.insert(at, Part { id: id.clone(), title, added: Some(added), ..Part::default() });
    Ok((plan, id))
}

// A preparatory subtask goes in ahead of the one commits go to now, and the next commits go to it until it is done; its id, or none for a task without subtasks.
pub(crate) fn add_preparatory(state: &Path, path: &Path, address: &str, title: &str, reason: &str, now: u64) -> Result<Option<String>> {
    let repo = main_checkout(path)?;
    let upstream = default_branch(&repo)?;
    let task = Task { path, upstream: &upstream, address };
    let reason: String = one_line(reason).chars().take(400).collect();
    let added = Added { at: now, reason: if reason.is_empty() { "an obstacle needs it first".to_string() } else { reason }, by: "investigator".to_string() };
    changed(state, address, |plan| {
        if plan.is_none_or(|plan| plan.parts.is_empty()) {
            return Ok((None, None));
        }
        let (_, plan) = noted_plan(state, &task)?;
        let before = current(&plan);
        let (plan, id) = inserted(plan, title, before.as_deref(), added)?;
        Ok((Some(Plan { on: Some(id.clone()), ..plan }), Some(id)))
    })
}

pub fn mark(state: &State) -> &'static str {
    match state {
        State::Planned => "○",
        State::InProgress => "◐",
        State::Ready => "●",
        State::Accepted | State::Pushed => "✓",
    }
}

pub fn word(state: &State) -> &'static str {
    match state {
        State::Planned => "planned",
        State::InProgress => "in progress",
        State::Ready => "ready",
        State::Accepted => "accepted",
        State::Pushed => "pushed",
    }
}

// What a subtask's row says after its title: its state, what an accepted one waits for, and once it holds any, how many commits.
pub fn label(subtask: &Subtask) -> String {
    let state = match (&subtask.state, &subtask.waits_for) {
        (State::Accepted, Some(waits)) => format!("accepted · waits for {waits}"),
        (state, _) => word(state).to_string(),
    };
    match (&subtask.state, subtask.commits.len()) {
        (State::Planned, _) => String::new(),
        (_, 0) => state,
        (_, 1) => format!("{state} · 1 commit"),
        (_, many) => format!("{state} · {many} commits"),
    }
}

fn shown(address: &str, subtasks: &Subtasks) -> String {
    if !subtasks.listed() {
        return format!("{address} has no subtasks: kebbie subtasks \"<subtask>\" … lists them, and kebbie subtasks done marks each one ready.");
    }
    let mut lines = vec![format!("{address}'s subtasks:")];
    if !subtasks.pushed.is_empty() {
        lines.push(format!("  ↑ {} pushed", subtasks.pushed.len()));
    }
    for item in &subtasks.items {
        let mut line = format!("  {} {}. {} [{}]", mark(&item.state), item.number, item.title, item.id);
        let label = label(item);
        if !label.is_empty() {
            line.push_str(&format!("  {label}"));
        }
        if item.extra {
            line.push_str("  (not in the list)");
        }
        lines.push(line);
        lines.extend(item.commits.iter().map(|commit| format!("       {} {}", short(&commit.sha), commit.subject)));
    }
    if !subtasks.order.is_empty() {
        lines.push(match &subtasks.pushable {
            Some(run) => format!("Pushable: the first {} through {}", if run.count == 1 { "commit".to_string() } else { format!("{} commits", run.count) }, short(&run.through)),
            None => "Pushable: nothing yet; the top commit's subtask is not accepted".to_string(),
        });
    }
    lines.join("\n")
}

fn short(sha: &str) -> &str {
    &sha[..7.min(sha.len())]
}

fn state_name(state: &State) -> &'static str {
    match state {
        State::Planned => "planned",
        State::InProgress => "in-progress",
        State::Ready => "ready",
        State::Accepted => "accepted",
        State::Pushed => "pushed",
    }
}

fn subtask_json(subtask: &Subtask) -> Value {
    let commits: Vec<Value> = subtask.commits.iter().map(|commit| json!({ "sha": commit.sha, "subject": commit.subject, "accepted": commit.accepted })).collect();
    json!({
        "id": subtask.id, "number": subtask.number, "title": subtask.title, "extra": subtask.extra, "state": state_name(&subtask.state), "contiguous": subtask.contiguous,
        "added": subtask.added.as_ref().map(|added| json!({ "at": added.at, "reason": added.reason, "by": added.by })), "waits_for": subtask.waits_for, "commits": commits, "pushed": subtask.pushed,
    })
}

fn to_json(address: &str, subtasks: &Subtasks, last_accepted: Option<&str>) -> Value {
    let order: Vec<Value> = subtasks.order.iter().map(|row| json!({ "sha": row.sha, "subject": row.subject, "subtask": row.subtask })).collect();
    json!({
        "task": address, "listed": subtasks.listed(), "pushed": subtasks.pushed.len(), "pushed_subtasks": subtasks.pushed.iter().map(subtask_json).collect::<Vec<_>>(),
        "subtasks": subtasks.items.iter().map(subtask_json).collect::<Vec<_>>(), "order": order,
        "pushable": subtasks.pushable.as_ref().map(|run| json!({ "count": run.count, "through": run.through, "subtasks": run.subtasks })), "last_accepted": last_accepted,
    })
}

// The push order and pushable run of subtasks given whole, their commits in their own order.
#[cfg(test)]
pub(crate) fn ordered(mut subtasks: Subtasks) -> Subtasks {
    subtasks.order = subtasks.items.iter().flat_map(|subtask| subtask.commits.iter().map(|commit| Placed { sha: commit.sha.clone(), subject: commit.subject.clone(), subtask: subtask.id.clone() })).collect();
    let accepted = |id: &str| subtasks.items.iter().any(|subtask| subtask.id == id && subtask.state == State::Accepted);
    let count = subtasks.order.iter().take_while(|row| accepted(&row.subtask)).count();
    subtasks.pushable = subtasks.order[..count].last().map(|last| {
        let mut ids: Vec<String> = Vec::new();
        for row in &subtasks.order[..count] {
            if !ids.contains(&row.subtask) {
                ids.push(row.subtask.clone());
            }
        }
        Run { count, through: last.sha.clone(), subtasks: ids }
    });
    subtasks
}

#[cfg(test)]
pub(crate) fn list_at(state: &Path, path: &Path, upstream: &str, address: &str, titles: &[&str]) -> Result<()> {
    let titles: Vec<String> = titles.iter().map(|title| title.to_string()).collect();
    write_as(&Caller::User, state, &Task { path, upstream, address }, &titles, None)
}

#[cfg(test)]
pub(crate) fn done_at(state: &Path, path: &Path, upstream: &str, address: &str) -> Result<()> {
    done_as(&Caller::User, state, &Task { path, upstream, address }, None, false).map(|_| ())
}

#[cfg(test)]
pub(crate) fn listed_at(state: &Path, path: &Path, upstream: &str, address: &str) -> Subtasks {
    listed_in(state, &Task { path, upstream, address }, false).unwrap()
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
