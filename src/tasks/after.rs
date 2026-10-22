use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::notice::{Notice, Tone};
use crate::messages::{self, Message};
use crate::peers;
use crate::permit::Caller;
use crate::repo::name_of;
use crate::sh::Result;
use super::intent::By;
use super::launch::{Begun, Start};
use super::pool::slots;
use super::slug;

pub const EVERY: u64 = 15;
pub const GRACE: u64 = 60;
const CLAIM_HOLDS: u64 = 300;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Dep {
    pub address: String,
    pub repo: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Wait {
    pub task: String,
    pub repo: PathBuf,
    pub after: Vec<Dep>,
    pub prompt: String,
    pub mode: Option<String>,
    pub profile: Option<String>,
    pub backlog: Option<String>,
    pub by: By,
    pub at: u64,
    pub blocked: Option<String>,
    pub gone_since: Option<u64>,
    pub why: Option<String>,
    pub starts_at: Option<u64>,
}

impl Wait {
    pub(crate) fn address(&self) -> String {
        peers::address(&self.task, &name_of(&self.repo))
    }

    fn to_json(&self) -> Value {
        json!({
            "task": self.task, "repo": self.repo,
            "after": self.after.iter().map(|dep| json!({ "address": dep.address, "repo": dep.repo })).collect::<Vec<_>>(),
            "prompt": self.prompt, "mode": self.mode, "profile": self.profile, "backlog": self.backlog, "by": self.by.key(), "at": self.at,
            "blocked": self.blocked, "gone_since": self.gone_since, "why": self.why, "starts_at": self.starts_at,
        })
    }

    fn from_json(value: &Value) -> Option<Wait> {
        let by = match value["by"].as_str()? {
            "user" => By::User,
            "lead" => By::Lead,
            _ => return None,
        };
        let after = value["after"].as_array()?.iter().map(|dep| Some(Dep { address: dep["address"].as_str()?.to_string(), repo: PathBuf::from(dep["repo"].as_str()?) })).collect::<Option<Vec<_>>>()?;
        let text = |key: &str| value[key].as_str().map(str::to_string);
        let starts_at = value["starts_at"].as_u64();
        Some(Wait {
            task: text("task").filter(|task| slug(task).as_deref() == Ok(task.as_str()))?,
            repo: PathBuf::from(text("repo")?),
            after: Some(after).filter(|after| !after.is_empty() || starts_at.is_some())?,
            prompt: text("prompt").filter(|prompt| !prompt.trim().is_empty())?,
            mode: text("mode"), profile: text("profile"), backlog: text("backlog"), by, at: value["at"].as_u64()?, blocked: text("blocked"), gone_since: value["gone_since"].as_u64(), why: text("why"), starts_at,
        })
    }
}

fn dir(state: &Path) -> PathBuf {
    state.join("after")
}

fn file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.json"))
}

fn claimed_file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!(".{address}.json.claimed"))
}

pub(crate) fn save(state: &Path, wait: &Wait) -> Result<()> {
    crate::state::write(&file(state, &wait.address()), serde_json::to_string_pretty(&wait.to_json()).unwrap_or_default())
}

pub(crate) fn load(state: &Path, address: &str) -> Option<Wait> {
    Wait::from_json(&serde_json::from_str(&fs::read_to_string(file(state, address)).ok()?).ok()?).filter(|wait| wait.address() == address)
}

fn forget(state: &Path, address: &str) {
    let _ = fs::remove_file(file(state, address));
}

// A file that does not read as a wait the user or the lead set is removed unread, so it never starts a task.
pub(crate) fn all(state: &Path) -> Vec<Wait> {
    let Ok(entries) = fs::read_dir(dir(state)) else { return Vec::new() };
    let mut waits = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(address) = name.strip_suffix(".json").filter(|_| !name.starts_with('.')) else { continue };
        match load(state, address) {
            Some(wait) => waits.push(wait),
            None => forget(state, address),
        }
    }
    waits.sort_by_key(Wait::address);
    waits
}

// Moving the file away is atomic, so a board and a keeper never start the same wait twice; the claimed file says it is starting until the start returns.
fn claim(state: &Path, address: &str) -> bool {
    fs::rename(file(state, address), claimed_file(state, address)).is_ok()
}

fn unclaim(state: &Path, address: &str) {
    let _ = fs::remove_file(claimed_file(state, address));
}

fn starting(state: &Path, address: &str) -> bool {
    fs::metadata(claimed_file(state, address)).and_then(|metadata| metadata.modified()).is_ok_and(|at| at.elapsed().unwrap_or_default().as_secs() < CLAIM_HOLDS)
}

struct Lock(fs::File);

impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

impl Lock {
    fn take(state: &Path, wait: bool) -> Option<Lock> {
        crate::state::private_dir(&dir(state)).ok()?;
        let file = crate::state::append_to(&dir(state).join(".lock")).ok()?;
        let operation = if wait { libc::LOCK_EX } else { libc::LOCK_EX | libc::LOCK_NB };
        // SAFETY: flock on a descriptor this File owns.
        (unsafe { libc::flock(file.as_raw_fd(), operation) } == 0).then_some(Lock(file))
    }
}

pub(crate) fn held_items(state: &Path) -> HashSet<String> {
    all(state).into_iter().filter_map(|wait| wait.backlog).collect()
}

fn waited_on(state: &Path, address: &str) -> bool {
    all(state).iter().any(|wait| wait.after.iter().any(|dep| dep.address == address))
}

// A done task's CI watch is kept when a wait names it, so the wait starts only on green.
pub(super) fn settle_ci(state: &Path, address: &str) {
    if waited_on(state, address) {
        crate::ci::keep_watching(state, address);
    } else {
        crate::ci::forget(state, address);
    }
}

// A new task by a done task's name must not end the CI watch a wait still needs.
pub(super) fn forget_unwaited_watch(state: &Path, address: &str) {
    if !waited_on(state, address) {
        crate::ci::forget(state, address);
    }
}

fn refusal(task: &str) -> String {
    Notice::new(Tone::NotNow, "It is queued").next(format!("kelpie after {task} --now starts it; --drop drops the wait.")).detail("task", task).to_string()
}

// By address: a wait's own start has claimed its file away, and its other halves are other addresses.
pub(super) fn queued_refusal(state: &Path, address: &str) -> Result<()> {
    match all(state).iter().find(|wait| wait.address() == address) {
        Some(wait) => Err(refusal(&wait.task)),
        None => Ok(()),
    }
}

pub(crate) fn queued_name_refusal(state: &Path, name: &str) -> Result<()> {
    let task = slug(name)?;
    match all(state).iter().any(|wait| wait.task == task) {
        true => Err(refusal(&task)),
        false => Ok(()),
    }
}

pub fn refuse_queued_name(name: &str) -> Result<()> {
    queued_name_refusal(&crate::lead::state_dir()?, name)
}

// A failed CI watch kept for a wait that is gone now has no one to tell.
fn release_watches(state: &Path, waits: &[Wait]) {
    for dep in waits.iter().flat_map(|wait| wait.after.iter()) {
        let leftover = crate::ci::load(state, &dep.address).filter(|watch| !watch.close && !watch.running() && !open_in(&watch.repo).contains(&dep.address));
        if leftover.is_some() && !waited_on(state, &dep.address) {
            crate::ci::forget(state, &dep.address);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DepState {
    Open,
    Queued,
    Done,
    CiPending,
    CiFailed,
    Ended(String),
    Gone,
}

#[derive(Default)]
pub(crate) struct Facts {
    pub ends: HashMap<String, Message>,
    pub open: HashSet<String>,
    pub queued: HashSet<String>,
    pub ci: HashMap<String, bool>,
}

pub(crate) fn latest_ends(log: &[Message]) -> HashMap<String, Message> {
    log.iter().filter(|message| message.ended).map(|message| (message.to.clone(), message.clone())).collect()
}

impl Facts {
    pub(crate) fn read(state: &Path, log: &[Message], waits: &[Wait]) -> Facts {
        let mut repos: Vec<PathBuf> = waits.iter().flat_map(|wait| wait.after.iter().map(|dep| dep.repo.clone()).chain([wait.repo.clone()])).collect();
        repos.sort();
        repos.dedup();
        let open = repos.iter().flat_map(|repo| open_in(repo)).collect();
        let mut queued: HashSet<String> = waits.iter().map(Wait::address).collect();
        queued.extend(waits.iter().flat_map(|wait| wait.after.iter()).filter(|dep| starting(state, &dep.address)).map(|dep| dep.address.clone()));
        let ci = crate::ci::all(state).into_iter().map(|watch| (watch.address.clone(), watch.running())).collect();
        Facts { ends: latest_ends(log), open, queued, ci }
    }

    pub(crate) fn state(&self, address: &str, since: u64) -> DepState {
        if let Some(end) = self.ends.get(address).filter(|end| end.at >= since) {
            return match messages::ended_how(end) {
                Some("done") => match self.ci.get(address) {
                    Some(true) => DepState::CiPending,
                    Some(false) => DepState::CiFailed,
                    None => DepState::Done,
                },
                Some("dropped") if end.kind.as_deref() == Some(super::finish::ROUTINE_OK) => DepState::Done,
                how => DepState::Ended(how.unwrap_or("ended").to_string()),
            };
        }
        if self.open.contains(address) {
            DepState::Open
        } else if self.queued.contains(address) {
            DepState::Queued
        } else {
            DepState::Gone
        }
    }
}

// git lists a worktree mid-rebase as detached, so a task resolving a conflict is found by the branch its rebase names.
fn open_in(repo: &Path) -> Vec<String> {
    slots(repo).unwrap_or_default().into_iter().filter_map(|slot| slot.task.clone().or_else(|| rebasing(&slot.path))).map(|task| peers::address(&task, &name_of(repo))).collect()
}

fn rebasing(worktree: &Path) -> Option<String> {
    ["rebase-merge/head-name", "rebase-apply/head-name"].iter().find_map(|name| {
        let path = crate::repo::git(worktree, &["rev-parse", "--git-path", name]).ok()?;
        let text = fs::read_to_string(worktree.join(path)).ok()?;
        text.trim().strip_prefix("refs/heads/").map(str::to_string)
    })
}

#[derive(Debug, PartialEq)]
pub(crate) enum Next {
    Wait,
    Missing,
    Start,
    Blocked(String),
}

// A blocked wait is the user's: it never starts by itself, whatever happens to the tasks it names afterwards.
pub(crate) fn next(wait: &Wait, facts: &Facts, now: u64) -> Next {
    if wait.blocked.is_some() {
        return Next::Wait;
    }
    let states: Vec<(&str, DepState)> = wait.after.iter().map(|dep| (dep.address.as_str(), facts.state(&dep.address, wait.at))).collect();
    if let Some((address, DepState::Ended(how))) = states.iter().find(|(_, state)| matches!(state, DepState::Ended(_))) {
        return Next::Blocked(format!("{address} was {how}, so {} did not start", wait.task));
    }
    if let Some((address, _)) = states.iter().find(|(_, state)| *state == DepState::CiFailed) {
        return Next::Blocked(red_ci(address, &wait.task));
    }
    if let Some((address, _)) = states.iter().find(|(_, state)| *state == DepState::Gone) {
        return match wait.gone_since {
            Some(since) if now.saturating_sub(since) >= GRACE => Next::Blocked(format!("{address} is gone without being done, so {} did not start", wait.task)),
            _ => Next::Missing,
        };
    }
    let due = wait.starts_at.is_none_or(|at| now >= at);
    if due && states.iter().all(|(_, state)| *state == DepState::Done) { Next::Start } else { Next::Wait }
}

fn red_ci(address: &str, task: &str) -> String {
    format!("{address} landed with red CI, so {task} did not start")
}

pub(super) fn red_ci_superseded(state: &Path, address: &str) -> Vec<String> {
    all(state).into_iter().filter(|wait| wait.blocked.as_deref() == Some(red_ci(address, &wait.task).as_str()))
        .filter_map(|wait| save(state, &Wait { blocked: None, ..wait.clone() }).ok().map(|_| wait.address())).collect()
}

const SUBJECTS_SHOWN: usize = 20;

// The subjects are the other task's words, so they are quoted apart from kelpie's own sentence.
fn landed_lines(address: &str, end: Option<&Message>) -> String {
    let lines: Vec<&str> = end.and_then(|end| end.landed.as_deref()).map(|landed| landed.lines().map(str::trim).filter(|line| !line.is_empty()).collect()).unwrap_or_default();
    if lines.is_empty() {
        return format!("This task waited for {address}, which is done; kelpie did not record its commits.");
    }
    let mut text = format!("This task waited for {address}, which is done. Its commits (subjects as that task wrote them):");
    for line in lines.iter().take(SUBJECTS_SHOWN) {
        text.push_str(&format!("\n> {line}"));
    }
    if lines.len() > SUBJECTS_SHOWN {
        text.push_str(&format!("\nand {} more", lines.len() - SUBJECTS_SHOWN));
    }
    text
}

fn not_done(state: &DepState) -> &'static str {
    match state {
        DepState::Open => "still open",
        DepState::Queued => "still queued",
        DepState::CiPending => "done, with CI not green yet on what it landed",
        DepState::CiFailed => "done, with red CI on what it landed",
        DepState::Gone => "gone",
        DepState::Done | DepState::Ended(_) => "not done",
    }
}

pub(crate) fn intro(wait: &Wait, facts: &Facts, anyway: Option<By>) -> String {
    wait.after.iter().map(|dep| match facts.state(&dep.address, wait.at) {
        DepState::Done => landed_lines(&dep.address, facts.ends.get(&dep.address)),
        DepState::Ended(how) => format!("This task waited for {}, which was {how}; {} started it anyway.", dep.address, anyway.map_or("kelpie", By::word)),
        state => format!("This task waited for {}, which was not done ({}) when {} started it anyway: build on main as it is, and expect {} to land later.", dep.address, not_done(&state), anyway.map_or("kelpie", By::word), dep.address),
    }).collect::<Vec<_>>().join("\n")
}

pub(crate) enum Told {
    Started { address: String, repo: PathBuf, text: String },
    Blocked { address: String, repo: PathBuf, text: String },
    Dropped { text: String },
}

pub(crate) type Begin<'a> = &'a mut dyn FnMut(&Path, &str, &Start) -> Result<Begun>;

fn launch(state: &Path, wait: &Wait, intro: &str, begin: Begin, tell: &mut dyn FnMut(Told)) -> Result<String> {
    let prompt = if intro.is_empty() { wait.prompt.clone() } else { format!("{intro}\n\n{}", wait.prompt) };
    let options = Start { prompt: Some(&prompt), focus: false, from: None, mode: wait.mode.as_deref(), profile: wait.profile.as_deref(), backlog: wait.backlog.as_deref(), note: true };
    let started = begin(&wait.repo, &wait.task, &options);
    unclaim(state, &wait.address());
    match started {
        Ok(begun) => Ok(begun.said),
        Err(error) => {
            let reason = format!("kelpie could not start it: {}", Notice::read(&error, true).details.iter().find(|(label, _)| label == "error").map_or(error.clone(), |(_, error)| error.clone()));
            let _ = save(state, &Wait { blocked: Some(reason.clone()), ..wait.clone() });
            tell(Told::Blocked { address: wait.address(), repo: wait.repo.clone(), text: blocked_text(wait, &reason) });
            Notice::new(Tone::Failed, "kelpie could not start it").detail("task", wait.address()).detail("error", error).err()
        }
    }
}

fn blocked_text(wait: &Wait, reason: &str) -> String {
    format!("{} did not start: {reason}. It waits for you under Needs you: start it anyway (kelpie after {} --now) or drop the wait (kelpie after {} --drop).", wait.address(), wait.task, wait.task)
}

// A start that never returned, kelpie having stopped mid-way, leaves its claim behind: the task runs, or the wait is the user's again.
fn sweep_claims(state: &Path, tell: &mut dyn FnMut(Told)) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir(state)) else { return Vec::new() };
    let mut notes = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(address) = name.strip_prefix('.').and_then(|name| name.strip_suffix(".json.claimed")) else { continue };
        if starting(state, address) {
            continue;
        }
        let wait = fs::read_to_string(entry.path()).ok().and_then(|text| serde_json::from_str(&text).ok()).and_then(|value| Wait::from_json(&value)).filter(|wait| wait.address() == address);
        if let Some(wait) = wait.filter(|wait| !open_in(&wait.repo).contains(&wait.address())) {
            let reason = "kelpie was interrupted while starting it".to_string();
            if save(state, &Wait { blocked: Some(reason.clone()), ..wait.clone() }).is_err() {
                continue;
            }
            tell(Told::Blocked { address: address.to_string(), repo: wait.repo.clone(), text: blocked_text(&wait, &reason) });
            notes.push(format!("after: {address}: {reason}"));
        }
        let _ = fs::remove_file(entry.path());
    }
    notes
}

pub(crate) fn tick_in(state: &Path, log: &[Message], now: u64, begin: Begin, tell: &mut dyn FnMut(Told)) -> Vec<String> {
    if !dir(state).is_dir() {
        return Vec::new();
    }
    let Some(_lock) = Lock::take(state, false) else { return Vec::new() };
    let mut notes = sweep_claims(state, tell);
    let waits = all(state);
    let facts = Facts::read(state, log, &waits);
    for wait in waits {
        let address = wait.address();
        if facts.open.contains(&address) {
            forget(state, &address);
            let text = format!("{address} was started while it was queued, so kelpie dropped its wait; it runs as it was started.");
            tell(Told::Dropped { text: text.clone() });
            notes.push(format!("after: {text}"));
            continue;
        }
        match next(&wait, &facts, now) {
            Next::Wait if wait.gone_since.is_some() => {
                let _ = save(state, &Wait { gone_since: None, ..wait });
            }
            Next::Wait => {}
            Next::Missing if wait.gone_since.is_none() => {
                let _ = save(state, &Wait { gone_since: Some(now), ..wait });
            }
            Next::Missing => {}
            Next::Blocked(reason) => {
                let _ = save(state, &Wait { blocked: Some(reason.clone()), ..wait.clone() });
                tell(Told::Blocked { address: address.clone(), repo: wait.repo.clone(), text: blocked_text(&wait, &reason) });
                notes.push(format!("after: {address}: {reason}"));
            }
            Next::Start => {
                if !claim(state, &address) {
                    continue;
                }
                let names: Vec<&str> = wait.after.iter().map(|dep| dep.address.as_str()).collect();
                let said = match launch(state, &wait, &intro(&wait, &facts, None), begin, tell) {
                    Ok(said) => {
                        let why = if names.is_empty() { "its start time came".to_string() } else { format!("{} {} done", names.join(" and "), if names.len() == 1 { "is" } else { "are" }) };
                        tell(Told::Started { address: address.clone(), repo: wait.repo.clone(), text: format!("{address} started: {why}. {said}") });
                        said
                    }
                    Err(error) => error,
                };
                notes.push(format!("after: {address}: {said}"));
            }
        }
    }
    notes
}

fn tell_for_real(told: Told) {
    let (notice, text, live) = match told {
        Told::Started { address, repo, text } => (crate::permit::away_since().map(|_| (crate::notify::Event::Summary, address, repo)), text, false),
        Told::Blocked { address, repo, text } => (Some((crate::notify::Event::NeedsYou, address, repo)), text, true),
        Told::Dropped { text } => (None, text, true),
    };
    let told = if live { messages::tell_lead(&text, Some("after")) } else { messages::log_for_lead(&text, Some("after")) };
    if let Err(error) = told {
        eprintln!("Could not tell the lead: {error}");
    }
    if let Some((event, address, repo)) = notice {
        crate::notify::send(&repo, &crate::notify::Notice::from(event, &address, text));
    }
}

pub fn tick(state: &Path) -> Vec<String> {
    tick_in(state, &messages::load(), messages::now(), &mut super::launch::begin, &mut tell_for_real)
}

pub(crate) struct Known {
    pub open: Vec<Dep>,
    pub waits: Vec<Wait>,
    pub ends: HashMap<String, Message>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Plan {
    Now,
    Queue(Vec<Dep>),
}

fn queuer(caller: &Caller) -> Result<By> {
    By::of(caller).ok_or_else(|| format!("only the user or the lead queues a task after another or changes a wait; {} cannot", crate::permit::describe(caller)))
}

fn task_part(address: &str) -> &str {
    address.split('@').next().unwrap_or(address)
}

pub(crate) fn plan(me: &str, prompt: &str, asked: &[&str], known: &Known) -> Result<Plan> {
    if prompt.trim().is_empty() {
        return Err(format!("{me} needs its prompt to wait: kelpie start {} \"<prompt>\" --after <task>", task_part(me)));
    }
    let mut deps: Vec<Dep> = Vec::new();
    for asked in asked {
        let (task, repo) = match asked.split_once('@') {
            Some((task, repo)) => (slug(task)?, Some(repo.to_string())),
            None => (slug(asked)?, None),
        };
        if task == task_part(me) {
            return Err(format!("{me} cannot wait for itself"));
        }
        let matches = |address: &str| task_part(address) == task && repo.as_deref().is_none_or(|repo| address.split_once('@').map(|(_, name)| name) == Some(repo));
        let found: Vec<Dep> = known.open.iter().filter(|dep| matches(&dep.address)).cloned()
            .chain(known.waits.iter().filter(|wait| matches(&wait.address())).map(|wait| Dep { address: wait.address(), repo: wait.repo.clone() }))
            .collect();
        if found.is_empty() {
            let done = known.ends.iter().any(|(address, end)| matches(address) && messages::ended_how(end) == Some("done"));
            if done {
                continue;
            }
            return Err(format!("{me} cannot wait for {asked}: no task, queued task or task done by that name"));
        }
        for dep in found {
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
    }
    if let Some(path) = deps.iter().find_map(|dep| circle(me, &dep.address, &known.waits, vec![me.to_string()])) {
        return Err(format!("{me} cannot wait in a circle: {}", path.join(" → ")));
    }
    let mine = task_part(me);
    if let Some(running) = known.open.iter().find(|dep| task_part(&dep.address) == mine) {
        return Err(format!("{mine} is already running ({}), so it cannot wait", running.address));
    }
    if let Some(queued) = known.waits.iter().find(|wait| wait.task == mine) {
        return Err(format!("{mine} is already queued ({}): kelpie after {mine} shows what it waits for", queued.address()));
    }
    Ok(if deps.is_empty() { Plan::Now } else { Plan::Queue(deps) })
}

fn circle(me: &str, address: &str, waits: &[Wait], mut path: Vec<String>) -> Option<Vec<String>> {
    path.push(address.to_string());
    if address == me {
        return Some(path);
    }
    if path[..path.len() - 1].iter().any(|seen| seen == address) {
        return None;
    }
    let wait = waits.iter().find(|wait| wait.address() == address)?;
    wait.after.iter().find_map(|dep| circle(me, &dep.address, waits, path.clone()))
}

fn known(state: &Path, repos: &[PathBuf]) -> Known {
    let waits = all(state);
    let mut every: Vec<PathBuf> = crate::status::known_repos(&crate::herdr::workspaces().unwrap_or_default(), crate::repo::pooled_checkouts()).into_iter().collect();
    every.extend(repos.iter().cloned());
    every.extend(waits.iter().map(|wait| wait.repo.clone()));
    every.sort();
    every.dedup();
    let open = every.iter().flat_map(|repo| open_in(repo).into_iter().map(|address| Dep { address, repo: repo.clone() })).collect();
    Known { open, waits, ends: latest_ends(&messages::load()) }
}

#[derive(Clone, Copy)]
pub(crate) struct Asked<'a> {
    pub name: &'a str,
    pub prompt: &'a str,
    pub after: &'a [&'a str],
    pub mode: Option<&'a str>,
    pub profile: Option<&'a str>,
    pub backlog: Option<&'a str>,
    pub title: Option<&'a str>,
    pub why: Option<&'a str>,
    pub starts_at: Option<u64>,
}

// Every half is checked before any is written, so a refusal changes nothing.
pub(crate) fn queue_in(state: &Path, caller: &Caller, repos: &[PathBuf], asked: &Asked, known: &Known, now: u64) -> Result<Option<String>> {
    let by = queuer(caller)?;
    let task = slug(asked.name)?;
    let mut planned = Vec::new();
    for repo in repos {
        let me = peers::address(&task, &name_of(repo));
        planned.push((repo, plan(&me, asked.prompt, asked.after, known)?));
    }
    if asked.starts_at.is_none() && planned.iter().all(|(_, plan)| *plan == Plan::Now) {
        return Ok(None);
    }
    let _lock = Lock::take(state, true);
    let mut said = Vec::new();
    for (repo, plan) in planned {
        let after = match plan {
            Plan::Queue(after) => after,
            Plan::Now if asked.starts_at.is_some() => Vec::new(),
            Plan::Now => continue,
        };
        let wait = Wait { task: task.clone(), repo: repo.clone(), after, prompt: asked.prompt.to_string(), mode: asked.mode.map(str::to_string), profile: asked.profile.map(str::to_string), backlog: asked.backlog.map(str::to_string), by, at: now, blocked: None, gone_since: None, why: asked.why.map(str::to_string), starts_at: asked.starts_at };
        save(state, &wait)?;
        if let Some(title) = asked.title.map(str::to_string).or_else(|| prompt_title(asked.prompt)) {
            crate::status::keep_title(state, &wait.address(), &crate::status::readable(&title, &name_of(repo)));
        }
        said.push(format!("Queued {}: it starts from the default branch {}, with no worktree, Space or Claude until then.", wait.address(), starts_when(&wait, now)));
    }
    Ok(Some(said.join("\n")))
}

fn names(wait: &Wait) -> String {
    wait.after.iter().map(|dep| dep.address.as_str()).collect::<Vec<_>>().join(" and ")
}

fn starts_when(wait: &Wait, now: u64) -> String {
    let at = wait.starts_at.map(|at| crate::when::shown(at, now, &crate::recover::local_offset));
    match (wait.after.is_empty(), at) {
        (true, Some(at)) => format!("at {at}"),
        (false, Some(at)) => format!("once {} {} done, not before {at}", names(wait), if wait.after.len() == 1 { "is" } else { "are" }),
        (_, None) => format!("once {} {} done", names(wait), if wait.after.len() == 1 { "is" } else { "are" }),
    }
}

// None means every task it names is done already, so the caller starts it as usual.
pub fn queue(repos: &[PathBuf], asked: &Asked) -> Result<Option<String>> {
    asked.mode.map(crate::config::permission_mode).transpose().map_err(|error| format!("--mode: {error}"))?;
    asked.profile.map(|profile| crate::profiles::for_task(&crate::profiles::dir(), profile)).transpose().map_err(|error| format!("--profile: {error}"))?;
    let state = crate::lead::state_dir()?;
    let repos = repos.iter().map(|repo| crate::repo::main_checkout(repo)).collect::<Result<Vec<_>>>()?;
    let known = known(&state, &repos);
    let title = asked.title.map(str::to_string).or_else(|| asked.backlog.and_then(crate::backlog::title_of));
    let why = asked.why.map(str::trim).filter(|why| !why.is_empty());
    queue_in(&state, &crate::permit::caller(), &repos, &Asked { title: title.as_deref(), why, ..*asked }, &known, messages::now())
}

fn halves(state: &Path, name: &str) -> Result<Vec<Wait>> {
    let task = slug(name)?;
    let found: Vec<Wait> = all(state).into_iter().filter(|wait| wait.task == task).collect();
    if found.is_empty() {
        return Notice::new(Tone::NotNow, "It is not queued").next("kelpie after lists what is.").detail("task", &task).err();
    }
    Ok(found)
}

pub(crate) fn now_in(state: &Path, caller: &Caller, name: &str, log: &[Message], begin: Begin, tell: &mut dyn FnMut(Told)) -> Result<String> {
    let by = queuer(caller)?;
    let waits = halves(state, name)?;
    let _lock = Lock::take(state, true);
    let facts = Facts::read(state, log, &waits);
    let (mut said, mut failed) = (Vec::new(), false);
    for wait in &waits {
        if !claim(state, &wait.address()) {
            said.push((wait.address(), Notice::new(Tone::NotNow, "It is starting already").detail("task", wait.address()).to_string()));
            continue;
        }
        match launch(state, wait, &intro(wait, &facts, Some(by)), begin, tell) {
            Ok(started) => said.push((wait.address(), started)),
            Err(error) => {
                failed = true;
                said.push((wait.address(), error));
            }
        }
    }
    release_watches(state, &waits);
    let said = match said.len() {
        1 => said.remove(0).1,
        _ => said.into_iter().fold(Notice::new(if failed { Tone::Failed } else { Tone::Done }, if failed { "Not every half started" } else { "Started every half" }), |notice, (address, text)| {
            let said = Notice::read(&text, false);
            let error = said.details.iter().find(|(label, _)| label == "error").map(|(_, error)| format!(" ({error})")).unwrap_or_default();
            notice.detail(&address, format!("{}{error}", said.headline))
        }).to_string(),
    };
    if failed { Err(said) } else { Ok(said) }
}

pub(crate) fn drop_in(state: &Path, caller: &Caller, name: &str) -> Result<String> {
    queuer(caller)?;
    let waits = halves(state, name)?;
    let _lock = Lock::take(state, true);
    for wait in &waits {
        forget(state, &wait.address());
        crate::status::forget_title(state, &wait.address());
    }
    release_watches(state, &waits);
    let back = waits.iter().find_map(|wait| wait.backlog.as_deref());
    Notice::new(Tone::Done, "Dropped the wait; nothing was started").next(if back.is_some() { "Its backlog item is back in the Backlog column." } else { "" })
        .detail("task", waits.iter().map(Wait::address).collect::<Vec<_>>().join(" and ")).detail("item", back.unwrap_or_default()).ok()
}

pub(crate) fn label(state: &DepState, item: Option<&crate::status::Item>) -> String {
    match state {
        DepState::Open => item.map_or("open".to_string(), |item| match &item.ci {
            Some(watch) if watch.running() => "ci running".to_string(),
            Some(_) => "ci failed".to_string(),
            None => item.group().title().to_lowercase(),
        }),
        DepState::Queued => "queued".to_string(),
        DepState::Done => "done".to_string(),
        DepState::CiPending => "ci running on what it landed".to_string(),
        DepState::CiFailed => "ci failed on what it landed".to_string(),
        DepState::Ended(how) => how.clone(),
        DepState::Gone => "gone".to_string(),
    }
}

pub(crate) struct Shown {
    pub wait: Wait,
    pub after: Vec<(String, String)>,
    pub names: Vec<String>,
    pub waiting: String,
    pub title: Option<String>,
    pub base: String,
    pub at: Option<String>,
}

pub(crate) fn shown(state: &Path, items: &[crate::status::Item]) -> Vec<Shown> {
    let waits = all(state);
    let facts = Facts::read(state, &messages::load(), &waits);
    waits.into_iter().map(|wait| {
        let deps: Vec<(String, String, DepState)> = wait.after.iter().map(|dep| {
            let item = items.iter().find(|item| item.is_task() && item.queued.is_none() && peers::address(&item.name, &item.place) == dep.address);
            let dep_state = match facts.state(&dep.address, wait.at) {
                DepState::Gone if item.is_some() => DepState::Open,
                dep_state => dep_state,
            };
            let label = label(&dep_state, item);
            (dep.address.clone(), label, dep_state)
        }).collect();
        let names: Vec<String> = wait.after.iter().map(|dep| {
            let item = items.iter().find(|item| item.is_task() && item.queued.is_none() && peers::address(&item.name, &item.place) == dep.address);
            dep_name(state, dep, &wait, item)
        }).collect();
        let now = messages::now();
        let at = wait.starts_at.filter(|at| *at > now).map(|at| crate::when::shown(at, now, &crate::recover::local_offset));
        let waiting = waiting_line(&names.iter().cloned().zip(deps.iter().map(|(_, label, dep_state)| (label.clone(), dep_state.clone()))).map(|(name, (label, dep_state))| (name, label, dep_state)).collect::<Vec<_>>(), wait.why.as_deref(), at.as_deref());
        let base = crate::repo::default_branch(&wait.repo).map_or("the default branch".to_string(), |branch| branch.trim_start_matches("origin/").to_string());
        let title = crate::status::kept_title(state, &wait.address());
        Shown { after: deps.into_iter().map(|(address, label, _)| (address, label)).collect(), names, waiting, title, base, at, wait }
    }).collect()
}

// The other task by the title its row shows, with its repo when it is not this one's.
fn dep_name(state: &Path, dep: &Dep, wait: &Wait, item: Option<&crate::status::Item>) -> String {
    let task = dep.address.split_once('@').map_or(dep.address.as_str(), |(task, _)| task);
    let title = item.and_then(|item| item.title.clone()).or_else(|| crate::status::kept_title(state, &dep.address)).unwrap_or_else(|| task.to_string());
    let repo = name_of(&dep.repo);
    if repo == name_of(&wait.repo) { title } else { format!("{title} in {repo}") }
}

// Why it waits and what starts it, in the user's words: each task it waits for with that task's state, the time it waits for, then what has to happen.
pub(crate) fn waiting_line(deps: &[(String, String, DepState)], why: Option<&str>, at: Option<&str>) -> String {
    let waits_for = deps.iter().map(|(name, label, _)| format!("{name} ({label})")).collect::<Vec<_>>();
    let why = why.map(|why| why.trim_end_matches('.'));
    let head = match (waits_for.is_empty(), why) {
        (true, Some(why)) => format!("Waits: {why}. "),
        (true, None) => String::new(),
        (false, Some(why)) => format!("Waits for {}: {why}. ", and_list(&waits_for)),
        (false, None) => format!("Waits for {}. ", and_list(&waits_for)),
    };
    let one = deps.len() == 1;
    if let Some((name, _, DepState::Ended(how))) = deps.iter().find(|(_, _, dep_state)| matches!(dep_state, DepState::Ended(_))) {
        return format!("{head}It will not start by itself: {} was {how}.", if one { "it" } else { name });
    }
    let pending: Vec<String> = deps.iter().filter_map(|(name, _, dep_state)| {
        let name = if one { "it" } else { name.as_str() };
        match dep_state {
            DepState::Open | DepState::Queued => Some(format!("{name} is pushed and CI is green")),
            DepState::CiPending => Some(format!("CI is green on what {name} pushed")),
            DepState::CiFailed => Some(format!("CI, red now, is green on what {name} pushed")),
            DepState::Gone => Some(format!("{name}, gone for now, is back and done")),
            DepState::Done | DepState::Ended(_) => None,
        }
    }).collect();
    match (pending.is_empty(), at) {
        (true, Some(at)) => format!("{head}Starts {at}."),
        (true, None) => format!("{head}Starts in a moment."),
        (false, Some(at)) => format!("{head}Starts by itself once {}, not before {at}.", pending.join(", and ")),
        (false, None) => format!("{head}Starts by itself once {}.", pending.join(", and ")),
    }
}

pub(crate) fn and_list(names: &[String]) -> String {
    match names.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

// A queued task has no Claude to name it yet: the backlog item's title, else the prompt's first sentence.
pub(crate) fn prompt_title(prompt: &str) -> Option<String> {
    let line = prompt.lines().map(str::trim).find(|line| !line.is_empty())?;
    let sentence = [". ", "? ", "! "].iter().filter_map(|end| line.find(end)).min().map_or(line, |at| &line[..at]);
    let mut words: Vec<&str> = sentence.trim_end_matches(['.', '?', '!', ':']).split_whitespace().take(TITLE_WORDS).collect();
    while words.last().is_some_and(|word| DANGLING.contains(&word.to_lowercase().as_str())) {
        words.pop();
    }
    (!words.is_empty()).then(|| words.join(" "))
}

const TITLE_WORDS: usize = 8;
const DANGLING: [&str; 12] = ["a", "an", "the", "and", "or", "of", "to", "in", "on", "for", "with", "from"];

pub fn list(name: Option<&str>) -> Result<String> {
    let state = crate::lead::state_dir()?;
    let items = crate::status::gather().unwrap_or_default();
    let task = name.map(slug).transpose()?;
    let lines: Vec<String> = shown(&state, &items).into_iter().filter(|shown| task.as_ref().is_none_or(|task| &shown.wait.task == task)).map(|Shown { wait, after, at, .. }| {
        let mut parts: Vec<String> = Vec::new();
        if !after.is_empty() {
            parts.push(format!("after {}", after.iter().map(|(address, state)| format!("{address} ({state})")).collect::<Vec<_>>().join(", ")));
        }
        parts.extend(at.map(|at| format!("at {at}")));
        let mut line = format!("{}  {}", wait.address(), parts.join(", not before "));
        if let Some(blocked) = &wait.blocked {
            line.push_str(&format!("\n  blocked: {blocked}; kelpie after {} --now starts it, --drop drops the wait", wait.task));
        }
        line
    }).collect();
    Ok(match (lines.is_empty(), &task) {
        (true, Some(task)) => format!("{task} is not queued."),
        (true, None) => "Nothing is queued.".to_string(),
        _ => lines.join("\n"),
    })
}

pub fn now(name: &str) -> Result<String> {
    let state = crate::lead::state_dir()?;
    now_in(&state, &crate::permit::caller(), name, &messages::load(), &mut super::launch::begin, &mut tell_for_real)
}

pub fn drop(name: &str) -> Result<String> {
    drop_in(&crate::lead::state_dir()?, &crate::permit::caller(), name)
}

pub(crate) fn at_in(state: &Path, caller: &Caller, name: &str, at: u64, now: u64) -> Result<String> {
    queuer(caller)?;
    let waits = halves(state, name)?;
    let _lock = Lock::take(state, true);
    let mut said = Vec::new();
    let waived: Vec<Wait> = waits.iter().filter(|wait| wait.blocked.is_some()).cloned().collect();
    for wait in waits {
        // A time set on a blocked wait waives the tasks it waited for, as --now does; kept, they would block it again.
        let after = if wait.blocked.is_some() { Vec::new() } else { wait.after.clone() };
        let wait = Wait { starts_at: Some(at), blocked: None, after, ..wait };
        save(state, &wait)?;
        said.push(format!("{} starts from the default branch {}.", wait.address(), starts_when(&wait, now)));
    }
    release_watches(state, &waived);
    Ok(said.join("\n"))
}

pub fn at(name: &str, text: &str) -> Result<String> {
    let now = messages::now();
    at_in(&crate::lead::state_dir()?, &crate::permit::caller(), name, crate::when::parse(text, now, &crate::recover::local_offset)?, now)
}

pub fn after(words: &[String], now_flag: bool, drop_flag: bool, at_flag: Option<&str>) -> Result<String> {
    if let Some(text) = at_flag {
        return match (words.first(), now_flag || drop_flag) {
            (_, true) => Err("kelpie after takes one of --now, --drop and --at".to_string()),
            (Some(name), false) => at(name, text),
            (None, false) => Err("kelpie after --at needs the queued task's name".to_string()),
        };
    }
    match (words.first(), now_flag, drop_flag) {
        (_, true, true) => Err("kelpie after takes --now or --drop, not both".to_string()),
        (Some(name), true, false) => now(name),
        (Some(name), false, true) => drop(name),
        (None, true, false) | (None, false, true) => Err("kelpie after --now and --drop need the queued task's name".to_string()),
        (name, false, false) => list(name.map(String::as_str)),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::repo::{ahead_behind, git};
    use crate::agent::Started;
    use crate::tasks::test_support::*;

    fn dep(address: &str) -> Dep {
        Dep { address: address.to_string(), repo: PathBuf::from(format!("/repos/{}", address.split_once('@').unwrap().1)) }
    }

    fn wait_for(task: &str, repo: &str, after: &[&str]) -> Wait {
        Wait { task: task.to_string(), repo: PathBuf::from(format!("/repos/{repo}")), after: after.iter().map(|address| dep(address)).collect(), prompt: "Use the rounding field".to_string(), mode: None, profile: None, backlog: None, by: By::Lead, at: 100, blocked: None, gone_since: None, why: None, starts_at: None }
    }

    fn end(address: &str, at: u64, how: &str, landed: Option<&str>) -> Message {
        let task = address.split('@').next().unwrap();
        Message { id: format!("end-{address}-{at}"), at, from: "kelpie".to_string(), to: address.to_string(), text: format!("{task} ended ({how})"), held: false, report: None, ended: true, question: None, about: None, kind: None, landed: landed.map(str::to_string), why: None, forced: false, fyi: false }
    }

    fn facts(ends: &[Message], open: &[&str]) -> Facts {
        Facts { ends: latest_ends(ends), open: open.iter().map(|address| address.to_string()).collect(), ..Facts::default() }
    }

    fn begun(said: &str) -> Result<Begun> {
        Ok(Begun { said: said.to_string(), started: Started::Ready })
    }

    struct Launched {
        task: String,
        prompt: String,
        focus: bool,
        from: Option<String>,
        note: bool,
    }

    fn ticked(state: &Path, log: &[Message], now: u64) -> (Vec<Launched>, Vec<Told>) {
        let (started, told) = (RefCell::new(Vec::new()), RefCell::new(Vec::new()));
        tick_in(state, log, now, &mut |_, task, options| { started.borrow_mut().push(Launched { task: task.to_string(), prompt: options.prompt.unwrap_or_default().to_string(), focus: options.focus, from: options.from.map(str::to_string), note: options.note }); begun("Started.") }, &mut |said| told.borrow_mut().push(said));
        (started.into_inner(), told.into_inner())
    }

    fn known(open: &[&str], waits: Vec<Wait>, ends: &[Message]) -> Known {
        Known { open: open.iter().map(|address| dep(address)).collect(), waits, ends: latest_ends(ends) }
    }

    #[test]
    fn a_task_can_wait_for_a_time_alone_or_as_well_as_another_task() {
        let state = crate::sh::test_dir("after-at");
        let known = known(&["billing@api"], Vec::new(), &[]);
        let asked = Asked { name: "nightly", prompt: "Run the import", after: &[], mode: None, profile: None, backlog: None, title: None, why: None, starts_at: Some(1_000) };
        let said = queue_in(&state, &Caller::User, &[PathBuf::from("/repos/web")], &asked, &known, 100).unwrap().unwrap();
        assert!(said.starts_with("Queued nightly@web: it starts from the default branch at "), "a wait for a time alone queues, with nothing to wait for: {said}");
        let wait = load(&state, "nightly@web").unwrap();
        assert_eq!((wait.after.len(), wait.starts_at), (0, Some(1_000)), "read back from its file with no task to wait for");
        assert_eq!(next(&wait, &facts(&[], &[]), 999), Next::Wait);
        assert_eq!(next(&wait, &facts(&[], &[]), 1_000), Next::Start);
        assert!(ticked(&state, &[], 999).0.is_empty(), "the keeper leaves it before its time");
        let (started, told) = ticked(&state, &[], 1_000);
        assert_eq!(started.iter().map(|launched| (launched.task.as_str(), launched.prompt.as_str())).collect::<Vec<_>>(), [("nightly", "Run the import")], "it starts at its time with its own prompt");
        assert!(matches!(told.as_slice(), [Told::Started { text, .. }] if text.starts_with("nightly@web started: its start time came.")));
        let both = Wait { starts_at: Some(1_000), ..wait_for("web-billing", "web", &["billing@api"]) };
        assert_eq!(next(&both, &facts(&[end("billing@api", 200, "done", None)], &[]), 500), Next::Wait, "the other task done, the time not yet come");
        assert_eq!(next(&both, &facts(&[], &["billing@api"]), 2_000), Next::Wait, "the time come, the other task still open");
        assert_eq!(next(&both, &facts(&[end("billing@api", 200, "done", None)], &[]), 2_000), Next::Start);
        let one = |dep_state: DepState, at: Option<&str>| waiting_line(&[("Billing".to_string(), "working".to_string(), dep_state)], None, at);
        assert_eq!(one(DepState::Open, Some("15:00 today")), "Waits for Billing (working). Starts by itself once it is pushed and CI is green, not before 15:00 today.");
        assert_eq!(one(DepState::Done, Some("15:00 today")), "Waits for Billing (working). Starts 15:00 today.");
        assert_eq!(waiting_line(&[], None, Some("Fri 24 Oct 09:00")), "Starts Fri 24 Oct 09:00.");
        assert_eq!(waiting_line(&[], Some("the import runs at night"), Some("02:00 tomorrow")), "Waits: the import runs at night. Starts 02:00 tomorrow.");
        save(&state, &both).unwrap();
        assert!(at_in(&state, &Caller::Agent, "web-billing", 5_000, 100).is_err(), "only the user or the lead changes a wait");
        let moved = at_in(&state, &Caller::User, "web-billing", 5_000, 100).unwrap();
        assert!(moved.starts_with("web-billing@web starts from the default branch once billing@api is done, not before "), "{moved}");
        assert_eq!(load(&state, "web-billing@web").unwrap().starts_at, Some(5_000));
        save(&state, &Wait { blocked: Some("billing@api was dropped, so web-billing did not start".to_string()), ..wait_for("web-billing", "web", &["billing@api"]) }).unwrap();
        assert!(at_in(&state, &Caller::User, "web-billing", 5_000, 100).unwrap().contains("starts from the default branch"));
        let dropped = facts(&[end("billing@api", 200, "dropped", None)], &[]);
        assert_eq!(next(&load(&state, "web-billing@web").unwrap(), &dropped, 4_999), Next::Wait, "not before its time, and not blocked again by the dropped task");
        assert_eq!(next(&load(&state, "web-billing@web").unwrap(), &dropped, 5_000), Next::Start, "a time the user set on a blocked wait starts it then");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_queued_row_says_why_it_waits_and_what_starts_it_in_plain_words() {
        let one = |state: DepState, why: Option<&str>| waiting_line(&[("Push finishes itself".to_string(), "ready to push".to_string(), state)], why, None);
        assert_eq!(one(DepState::Open, Some("it builds on that change.")), "Waits for Push finishes itself (ready to push): it builds on that change. Starts by itself once it is pushed and CI is green.");
        assert_eq!(one(DepState::Open, None), "Waits for Push finishes itself (ready to push). Starts by itself once it is pushed and CI is green.");
        assert_eq!(one(DepState::CiPending, None), "Waits for Push finishes itself (ready to push). Starts by itself once CI is green on what it pushed.");
        assert_eq!(one(DepState::Ended("dropped".to_string()), None), "Waits for Push finishes itself (ready to push). It will not start by itself: it was dropped.");
        assert_eq!(one(DepState::Done, None), "Waits for Push finishes itself (ready to push). Starts in a moment.");
        let two = waiting_line(&[("Billing".to_string(), "working".to_string(), DepState::Open), ("Search in web".to_string(), "queued".to_string(), DepState::Queued)], None, None);
        assert_eq!(two, "Waits for Billing (working) and Search in web (queued). Starts by itself once Billing is pushed and CI is green, and Search in web is pushed and CI is green.");
        assert!(!two.contains('@'), "tasks by their titles, not their addresses");
    }

    #[test]
    fn a_queued_task_is_titled_from_its_backlog_item_or_its_prompt_and_keeps_the_title_once_started() {
        assert_eq!(prompt_title("Fix the login redirect. It loops on Safari.").as_deref(), Some("Fix the login redirect"));
        assert_eq!(prompt_title("\n  Bring the Queued row in line with the other rows of the board, title and all\nmore").as_deref(), Some("Bring the Queued row in line"), "a title cut to its words does not end on a dangling one");
        assert_eq!(prompt_title("   "), None);
        assert_eq!(crate::status::readable("kelpie — Queued row says what it waits for", "kelpie"), "Queued row says what it waits for", "a vault note's title drops its repo");
        let state = crate::sh::test_dir("after-title");
        let known = known(&["billing@api"], Vec::new(), &[]);
        let asked = |title| Asked { name: "web-billing", prompt: "use the rounding field. Then bill.", after: &["billing@api"], mode: None, profile: None, backlog: None, title, why: Some("it needs the rounding field"), starts_at: None };
        queue_in(&state, &Caller::Lead, &[PathBuf::from("/repos/web")], &asked(None), &known, 100).unwrap().unwrap();
        assert_eq!(crate::status::kept_title(&state, "web-billing@web").as_deref(), Some("Use the rounding field"));
        assert_eq!(load(&state, "web-billing@web").unwrap().why.as_deref(), Some("it needs the rounding field"), "the reason is kept with the wait");
        drop_in(&state, &Caller::Lead, "web-billing").unwrap();
        assert_eq!(crate::status::kept_title(&state, "web-billing@web"), None, "a dropped wait takes its title along");
        queue_in(&state, &Caller::Lead, &[PathBuf::from("/repos/web")], &asked(Some("web — Bill in cents")), &known, 100).unwrap().unwrap();
        assert_eq!(crate::status::kept_title(&state, "web-billing@web").as_deref(), Some("Bill in cents"));
        assert_eq!(crate::status::title_of(&state, "web-billing@web", Some("Claude Code"), "web", "web-billing", "wt1.web").as_deref(), Some("Bill in cents"), "started, it keeps the title until Claude names the session");
        assert_eq!(crate::status::title_of(&state, "web-billing@web", Some("Rounded invoices"), "web", "web-billing", "wt1.web").as_deref(), Some("Rounded invoices"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_task_after_another_starts_once_it_has_landed_on_main() {
        let wait = wait_for("web-billing", "web", &["billing@api"]);
        assert_eq!(next(&wait, &facts(&[], &["billing@api"]), 200), Next::Wait);
        assert_eq!(next(&wait, &facts(&[end("billing@api", 200, "done", None)], &[]), 200), Next::Start);
        let state = crate::sh::test_dir("after-starts");
        save(&state, &wait).unwrap();
        let (started, _) = ticked(&state, &[end("billing@api", 200, "done", None)], 210);
        assert_eq!(started.iter().map(|started| started.task.as_str()).collect::<Vec<_>>(), ["web-billing"]);
        assert!(load(&state, "web-billing@web").is_none() && !claimed_file(&state, "web-billing@web").exists(), "a started wait leaves nothing behind");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_started_task_after_another_branches_from_main_with_its_commits() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let landed = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let fetched = git(&dir, &["rev-parse", &upstream]).unwrap();
        git(&slot.path, &["push", "--quiet", "origin", &format!("HEAD:{}", upstream.trim_start_matches("origin/"))]).unwrap();
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), &fetched]).unwrap();
        assert_ne!(git(&dir, &["rev-parse", &upstream]).unwrap(), landed, "the main checkout has not fetched what billing landed");
        let state = state_of(&dir);
        let address = peers::address("billing", &name_of(&dir));
        let wait = Wait { task: "search".to_string(), repo: dir.clone(), after: vec![Dep { address: address.clone(), repo: dir.clone() }], prompt: "Search the invoices".to_string(), mode: None, profile: None, backlog: None, by: By::User, at: 100, blocked: None, gone_since: None, why: None, starts_at: None };
        save(&state, &wait).unwrap();
        let mut told = Vec::new();
        tick_in(&state, &[end(&address, 200, "done", Some("abc fourth"))], 210, &mut super::super::launch::begin, &mut |said| told.push(said));
        let search = slots(&dir).unwrap().into_iter().find(|slot| slot.task.as_deref() == Some("search")).expect("search got a worktree on its branch");
        assert_eq!(git(&search.path, &["rev-parse", "HEAD"]).unwrap(), landed, "it starts on main as fetched, with billing's commit");
        assert_eq!(ahead_behind(&search.path, &upstream).unwrap(), (0, 0));
        git(&dir, &["worktree", "remove", "--force", &search.path.to_string_lossy()]).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_after_several_waits_for_every_one_of_them() {
        let wait = wait_for("web-billing", "web", &["billing@api", "search@api"]);
        assert_eq!(next(&wait, &facts(&[end("billing@api", 200, "done", None)], &["search@api"]), 300), Next::Wait);
        assert_eq!(next(&wait, &facts(&[end("billing@api", 200, "done", None), end("search@api", 250, "done", None)], &[]), 300), Next::Start);
    }

    #[test]
    fn a_task_after_one_with_halves_in_two_repos_waits_for_both_halves() {
        let planned = plan("report@app", "Show the totals", &["billing"], &known(&["billing@api", "billing@web", "other@web"], Vec::new(), &[])).unwrap();
        assert_eq!(planned, Plan::Queue(vec![dep("billing@api"), dep("billing@web")]), "a name is every half of that task");
        let Plan::Queue(after) = planned else { unreachable!() };
        let wait = Wait { after, ..wait_for("report", "app", &["billing@api"]) };
        assert_eq!(next(&wait, &facts(&[end("billing@api", 200, "done", None)], &["billing@web"]), 300), Next::Wait);
        assert_eq!(next(&wait, &facts(&[end("billing@api", 200, "done", None), end("billing@web", 210, "done", None)], &[]), 300), Next::Start);
    }

    #[test]
    fn a_task_after_one_in_another_repo_starts_when_that_one_is_done() {
        let planned = plan("web-billing@web", "Use it", &["billing@api"], &known(&["billing@api", "billing@web"], Vec::new(), &[])).unwrap();
        assert_eq!(planned, Plan::Queue(vec![dep("billing@api")]), "task@repo names that half only");
        let wait = wait_for("web-billing", "web", &["billing@api"]);
        assert_eq!(next(&wait, &facts(&[end("billing@api", 200, "done", None)], &["billing@web"]), 300), Next::Start);
    }

    #[test]
    fn the_prompt_of_a_task_after_another_names_what_landed_before_it() {
        let state = crate::sh::test_dir("after-landed");
        save(&state, &wait_for("web-billing", "web", &["billing@api"])).unwrap();
        let (started, _) = ticked(&state, &[end("billing@api", 200, "done", Some("abc1234 Add invoice rounding\ndef5678 Round in the API\n"))], 210);
        assert_eq!(started[0].prompt, "This task waited for billing@api, which is done. Its commits (subjects as that task wrote them):\n> abc1234 Add invoice rounding\n> def5678 Round in the API\n\nUse the rounding field");
        assert_eq!(intro(&wait_for("x", "web", &["billing@api"]), &facts(&[end("billing@api", 200, "done", None)], &[]), None), "This task waited for billing@api, which is done; kelpie did not record its commits.");
        let many: String = (1..=25).map(|n| format!("abc{n:04} Step {n}\n")).collect();
        let shown = intro(&wait_for("x", "web", &["billing@api"]), &facts(&[end("billing@api", 200, "done", Some(&many))], &[]), None);
        assert_eq!(shown.lines().filter(|line| line.starts_with("> ")).count(), 20);
        assert!(shown.ends_with("> abc0020 Step 20\nand 5 more"), "{shown}");
        let address = format!("landed-{}@api", std::process::id());
        messages::log_end(task_part(&address), "api", "done", Some("abc1234 Add invoice rounding"), None).unwrap();
        let logged = latest_ends(&messages::load()).remove(&address).unwrap();
        assert_eq!((messages::ended_how(&logged), logged.landed.as_deref()), (Some("done"), Some("abc1234 Add invoice rounding")), "the end line carries what landed");
        let source = include_str!("finish.rs");
        let close = &source[source.find("\npub(super) fn close_task(").unwrap()..];
        let close = &close[..close.find("\n}\n").unwrap()];
        assert!(close.contains("end_life(task_of(&slot), &repo, \"done\", landed.as_deref(), None)") && close.contains("watch.pushed.commits"), "done records the commits the push or its CI watch named");
        let push = include_str!("push.rs");
        assert!(push.contains("ready.unwrap_or_default(), Some(&pushed))") && push.contains("ready.unwrap_or_default(), Some(&told.commits))"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_queued_task_holds_no_worktree_space_or_claude() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let billing = peers::address("billing", &name_of(&dir));
        let before = slots(&dir).unwrap().len();
        let known = Known { open: vec![Dep { address: billing.clone(), repo: dir.clone() }], waits: Vec::new(), ends: HashMap::new() };
        let said = queue_in(&state, &Caller::User, std::slice::from_ref(&dir), &Asked { name: "search", prompt: "Search the invoices", after: &["billing"], mode: None, profile: None, backlog: None, title: None, why: None, starts_at: None }, &known, 100).unwrap().unwrap();
        assert!(said.contains("no worktree, Space or Claude"), "{said}");
        assert_eq!(slots(&dir).unwrap().len(), before, "no worktree is claimed or made");
        assert!(!crate::repo::branch_exists(&dir, "search"), "no branch either");
        let queued = load(&state, &peers::address("search", &name_of(&dir))).unwrap();
        assert_eq!((queued.after, queued.by, queued.at), (vec![Dep { address: billing, repo: dir.clone() }], By::User, 100));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn starting_after_tasks_already_done_starts_at_once() {
        let state = crate::sh::test_dir("after-done");
        let known = known(&[], Vec::new(), &[end("billing@api", 50, "done", None)]);
        assert_eq!(plan("web-billing@web", "Use it", &["billing@api"], &known).unwrap(), Plan::Now);
        assert_eq!(plan("web-billing@web", "Use it", &["billing"], &known).unwrap(), Plan::Now);
        let asked = Asked { name: "web-billing", prompt: "Use it", after: &["billing"], mode: None, profile: None, backlog: None, title: None, why: None, starts_at: None };
        assert_eq!(queue_in(&state, &Caller::Lead, &[PathBuf::from("/repos/web")], &asked, &known, 100).unwrap(), None, "the caller starts it as usual");
        assert!(all(&state).is_empty() && !dir(&state).join("web-billing@web.json").exists());
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_lead_hears_when_a_queued_task_starts() {
        let state = crate::sh::test_dir("after-lead");
        save(&state, &wait_for("web-billing", "web", &["billing@api"])).unwrap();
        let (_, told) = ticked(&state, &[end("billing@api", 200, "done", None)], 210);
        assert!(matches!(told.as_slice(), [Told::Started { address, text, .. }] if address == "web-billing@web" && text.starts_with("web-billing@web started: billing@api is done.")));
        let source = include_str!("after.rs");
        let real = &source[source.find("\nfn tell_for_real(").unwrap()..];
        let real = &real[..real.find("\n}\n").unwrap()];
        assert!(real.contains("messages::tell_lead(&text, Some(\"after\"))"), "a blocked or dropped chain reaches the lead through the log, held when it has no session");
        assert!(real.contains("Told::Started { address, repo, text } => (crate::permit::away_since().map(|_| (crate::notify::Event::Summary, address, repo)), text, false)") && real.contains("messages::log_for_lead(&text, Some(\"after\"))"), "a start is a notice for the lead's digest, not a turn");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_queued_task_starts_once_across_a_board_and_the_keeper() {
        let state = crate::sh::test_dir("after-once");
        save(&state, &wait_for("web-billing", "web", &["billing@api"])).unwrap();
        let log = [end("billing@api", 200, "done", None)];
        let count = RefCell::new(0);
        tick_in(&state, &log, 210, &mut |_, _, _| {
            *count.borrow_mut() += 1;
            let (inner, _) = ticked(&state, &log, 210);
            assert!(inner.is_empty(), "the other ticker skips the round while one runs");
            begun("Started.")
        }, &mut |_| {});
        assert_eq!(*count.borrow(), 1);
        ticked(&state, &log, 220);
        assert_eq!(*count.borrow(), 1, "a started wait is gone");
        save(&state, &wait_for("web-billing", "web", &["billing@api"])).unwrap();
        assert!(claim(&state, "web-billing@web") && !claim(&state, "web-billing@web"), "one claim wins");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_task_pushed_but_under_ci_running_keeps_its_followers_waiting() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let (state, wait) = waiting_on(&dir, "billing");
        let address = peers::address("billing", &name_of(&dir));
        crate::ci::save(&state, &crate::ci::Watch { address: address.clone(), task: "billing".to_string(), repo: dir.clone(), slug: "acme/api".to_string(), sha: "abcdef0".to_string(), started_at: 10, checked_at: None, close: true, runs: Vec::new(), state: crate::ci::State::Running, error: None, pushed: crate::ci::Pushed::default() }).unwrap();
        assert_eq!(next(&wait, &Facts::read(&state, &[], std::slice::from_ref(&wait)), 500), Next::Wait, "open under CI running");
        untouched(&state, &wait);
        let ended = [end(&address, 200, "done", None)];
        assert_eq!(Facts::read(&state, &ended, std::slice::from_ref(&wait)).state(&address, 100), DepState::CiPending);
        assert!(ticked(&state, &ended, 500).0.is_empty(), "closed as done with its CI still running: it waits for green");
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 0);
        item.ci = crate::ci::load(&state, &address);
        assert_eq!(label(&DepState::Open, Some(&item)), "ci running");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_whose_ci_failed_keeps_its_followers_waiting_and_one_closed_with_red_ci_waits_for_you() {
        let wait = wait_for("web-billing", "web", &["billing@api"]);
        let mut failed = facts(&[], &["billing@api"]);
        failed.ci.insert("billing@api".to_string(), false);
        assert_eq!(next(&wait, &failed, 300), Next::Wait, "red CI on an open task is that task's own business");
        let mut done_red = facts(&[end("billing@api", 200, "done", None)], &[]);
        done_red.ci.insert("billing@api".to_string(), false);
        assert_eq!(next(&wait, &done_red, 300), Next::Blocked("billing@api landed with red CI, so web-billing did not start".to_string()));
        assert_eq!(label(&DepState::CiFailed, None), "ci failed on what it landed");
        let state = crate::sh::test_dir("after-red");
        save(&state, &wait).unwrap();
        crate::ci::save(&state, &crate::ci::Watch { address: "billing@api".to_string(), task: "billing".to_string(), repo: PathBuf::from("/repos/api"), slug: "acme/api".to_string(), sha: "abcdef0".to_string(), started_at: 10, checked_at: None, close: false, runs: Vec::new(), state: crate::ci::State::Failed { at: 20, what: "CI · test".to_string(), url: String::new() }, error: None, pushed: crate::ci::Pushed::default() }).unwrap();
        let (started, told) = ticked(&state, &[end("billing@api", 200, "done", None)], 210);
        assert!(started.is_empty() && matches!(told.as_slice(), [Told::Blocked { .. }]));
        assert!(load(&state, "web-billing@web").unwrap().blocked.is_some(), "it waits for you under Needs you");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_task_closed_as_done_while_its_ci_runs_keeps_its_followers_waiting_until_green() {
        let state = crate::sh::test_dir("after-ci");
        let watch = crate::ci::Watch { address: "billing@api".to_string(), task: "billing".to_string(), repo: PathBuf::from("/repos/api"), slug: "acme/api".to_string(), sha: "abcdef0".to_string(), started_at: 10, checked_at: None, close: true, runs: Vec::new(), state: crate::ci::State::Running, error: None, pushed: crate::ci::Pushed::default() };
        crate::ci::save(&state, &watch).unwrap();
        settle_ci(&state, "billing@api");
        assert!(crate::ci::load(&state, "billing@api").is_none(), "nothing waits on it: the watch goes with the task");
        crate::ci::save(&state, &watch).unwrap();
        save(&state, &wait_for("web-billing", "web", &["billing@api"])).unwrap();
        settle_ci(&state, "billing@api");
        assert_eq!(crate::ci::load(&state, "billing@api").map(|kept| (kept.close, kept.running())), Some((false, true)), "kept, closing nothing");
        let log = [end("billing@api", 200, "done", None)];
        assert!(ticked(&state, &log, 210).0.is_empty(), "done, but CI is not green yet");
        crate::ci::forget(&state, "billing@api");
        assert_eq!(ticked(&state, &log, 220).0.len(), 1, "green: the watch is gone and it starts");
        let source = include_str!("finish.rs");
        let close = &source[source.find("\npub(super) fn close_task(").unwrap()..];
        assert!(close[..close.find("\n}\n").unwrap()].contains("super::after::settle_ci(&state, &address)"));
        fs::remove_dir_all(state).unwrap();
    }

    fn waiting_on(dir: &Path, task: &str) -> (PathBuf, Wait) {
        let state = state_of(dir);
        let address = peers::address(task, &name_of(dir));
        let wait = Wait { task: "search".to_string(), repo: dir.to_path_buf(), after: vec![Dep { address, repo: dir.to_path_buf() }], prompt: "Search".to_string(), mode: None, profile: None, backlog: None, by: By::User, at: 100, blocked: None, gone_since: None, why: None, starts_at: None };
        save(&state, &wait).unwrap();
        (state, wait)
    }

    fn untouched(state: &Path, wait: &Wait) {
        let before = fs::read_to_string(file(state, &wait.address())).unwrap();
        let (started, told) = ticked(state, &[], 500);
        assert!(started.is_empty() && told.is_empty());
        assert_eq!(fs::read_to_string(file(state, &wait.address())).unwrap(), before, "its file is as it was");
    }

    #[test]
    fn a_partial_push_does_not_count_as_landed() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let first = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), &first]).unwrap();
        let (state, wait) = waiting_on(&dir, "billing");
        assert_eq!(next(&wait, &Facts::read(&state, &[], std::slice::from_ref(&wait)), 500), Next::Wait, "the rest of billing is still open");
        untouched(&state, &wait);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_that_waits_for_its_tests_keeps_its_followers_waiting() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let (state, wait) = waiting_on(&dir, "billing");
        let address = peers::address("billing", &name_of(&dir));
        super::super::intent::save(&state, &super::super::intent::Intent { by: By::User, work: super::super::intent::Work::Tests, address: address.clone(), repo: dir.clone(), task: "billing".to_string(), fingerprint: "abc".to_string(), through: super::super::intent::Through::All, at: 90, seen: None, hand: None }).unwrap();
        assert_eq!(Facts::read(&state, &[], std::slice::from_ref(&wait)).state(&address, 100), DepState::Open);
        untouched(&state, &wait);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_in_review_or_with_findings_keeps_its_followers_waiting() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let (state, wait) = waiting_on(&dir, "billing");
        let address = peers::address("billing", &name_of(&dir));
        crate::state::write(&crate::review::verdict_file(&state, &address), "verdict: changes\nreviewed: abc\n\n- one\n- two\n").unwrap();
        assert_eq!(next(&wait, &Facts::read(&state, &[], std::slice::from_ref(&wait)), 500), Next::Wait);
        untouched(&state, &wait);
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        item.verdict = Some(crate::status::tests::in_review(false, 2, false, false));
        assert_eq!(label(&DepState::Open, Some(&item)), "to review");
        assert_eq!(ticked(&state, &[end(&address, 200, "done", None)], 500).0.len(), 1, "it is the end line that starts it, not the review");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_whose_rebase_conflicts_keeps_its_followers_waiting() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        upstream_commit(&dir, "d", "theirs", "upstream changed d too");
        assert!(git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "rebase", &upstream]).is_err());
        assert!(crate::repo::rebase_in_progress(&slot.path));
        let (state, wait) = waiting_on(&dir, "billing");
        untouched(&state, &wait);
        assert!(crate::repo::rebase_in_progress(&slot.path), "billing's rebase is its own Claude's");
        git(&slot.path, &["rebase", "--abort"]).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn an_end_before_the_wait_was_set_does_not_count() {
        let wait = wait_for("web-billing", "web", &["billing@api"]);
        assert_eq!(next(&wait, &facts(&[end("billing@api", 50, "done", None)], &["billing@api"]), 300), Next::Wait, "an earlier billing by that name");
        assert_eq!(next(&wait, &facts(&[end("billing@api", 50, "done", None)], &[]), 300), Next::Missing);
        assert_eq!(next(&wait, &facts(&[end("billing@api", 50, "dropped", None)], &["billing@api"]), 300), Next::Wait, "an earlier one's drop blocks nothing either");
    }

    #[test]
    fn a_task_done_after_being_pushed_elsewhere_counts_as_landed() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        land_on_main(&dir, &upstream, |dir| pick(dir, &fourth));
        let (state, wait) = waiting_on(&dir, "billing");
        super::super::finish::finish(&dir, &slot, &upstream, &state, None).unwrap();
        let address = peers::address("billing", &name_of(&dir));
        assert_eq!(next(&wait, &Facts::read(&state, &[], std::slice::from_ref(&wait)), 500), Next::Missing, "freed, its end not logged yet");
        assert_eq!(next(&wait, &Facts::read(&state, &[end(&address, 200, "done", None)], std::slice::from_ref(&wait)), 500), Next::Start);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_without_commits_that_ends_done_counts_as_finished() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&slot.path, &["switch", "--quiet", "--create", "research", &upstream]).unwrap();
        let research = super::super::pool::find_task(&dir, Some("research"), &dir).unwrap();
        let (state, wait) = waiting_on(&dir, "research");
        super::super::finish::finish(&dir, &research, &upstream, &state, None).expect("done takes a task with no commits");
        let address = peers::address("research", &name_of(&dir));
        assert_eq!(next(&wait, &Facts::read(&state, &[end(&address, 200, "done", None)], std::slice::from_ref(&wait)), 500), Next::Start);
        git(&slot.path, &["switch", "--quiet", "billing"]).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_task_after_one_that_was_dropped_waits_for_you_with_the_reason() {
        let state = crate::sh::test_dir("after-dropped");
        save(&state, &wait_for("web-billing", "web", &["billing@api"])).unwrap();
        let (started, told) = ticked(&state, &[end("billing@api", 200, "dropped", None)], 210);
        assert!(started.is_empty());
        let blocked = load(&state, "web-billing@web").unwrap().blocked.unwrap();
        assert_eq!(blocked, "billing@api was dropped, so web-billing did not start");
        assert!(matches!(told.as_slice(), [Told::Blocked { text, .. }] if text.contains("waits for you under Needs you") && text.contains("kelpie after web-billing --now") && text.contains("--drop")));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_task_after_a_routine_run_closed_with_ok_starts_and_one_dropped_otherwise_still_waits_for_you() {
        let wait = wait_for("follow-up", "api", &["nightly-0101@api"]);
        let ok = Message { kind: Some(super::super::finish::ROUTINE_OK.to_string()), ..end("nightly-0101@api", 200, "dropped", None) };
        assert_eq!(next(&wait, &facts(&[ok], &[]), 300), Next::Start, "a run that found all well finished");
        assert_eq!(next(&wait, &facts(&[end("nightly-0101@api", 200, "dropped", None)], &[]), 300), Next::Blocked("nightly-0101@api was dropped, so follow-up did not start".to_string()));
        let address = format!("routine-{}@api", std::process::id());
        messages::log_end(task_part(&address), "api", "dropped", None, Some(super::super::finish::ROUTINE_OK)).unwrap();
        assert_eq!(latest_ends(&messages::load()).remove(&address).unwrap().kind.as_deref(), Some("routine-ok"), "the end line carries the mark");
        assert!(include_str!("../routines.rs").contains("crate::tasks::drop_routine_ok(cwd)"), "kelpie routine ok closes with the mark");
    }

    #[test]
    fn a_task_after_one_that_was_reset_waits_for_you_with_the_reason() {
        let wait = wait_for("web-billing", "web", &["billing@api", "search@api"]);
        assert_eq!(next(&wait, &facts(&[end("billing@api", 200, "done", None), end("search@api", 200, "reset", None)], &[]), 300), Next::Blocked("search@api was reset, so web-billing did not start".to_string()));
    }

    #[test]
    fn a_task_after_one_that_vanished_without_an_end_waits_for_you() {
        let state = crate::sh::test_dir("after-gone");
        save(&state, &wait_for("web-billing", "web", &["billing@api"])).unwrap();
        let (started, told) = ticked(&state, &[], 1_000);
        assert!(started.is_empty() && told.is_empty(), "a task between its release and its end line is not gone yet");
        assert_eq!(load(&state, "web-billing@web").unwrap().gone_since, Some(1_000));
        ticked(&state, &[], 1_000 + GRACE - 1);
        assert_eq!(load(&state, "web-billing@web").unwrap().blocked, None);
        let (_, told) = ticked(&state, &[], 1_000 + GRACE);
        assert_eq!(load(&state, "web-billing@web").unwrap().blocked.as_deref(), Some("billing@api is gone without being done, so web-billing did not start"));
        assert_eq!(told.len(), 1);
        save(&state, &Wait { gone_since: Some(5), ..wait_for("web-billing", "web", &["billing@api"]) }).unwrap();
        let mut open = Facts::read(&state, &[], &all(&state));
        open.open.insert("billing@api".to_string());
        assert_eq!(next(&load(&state, "web-billing@web").unwrap(), &open, 9_999), Next::Wait, "back again, it waits as before");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_blocked_queued_task_tells_the_lead_and_notifies_once() {
        let state = crate::sh::test_dir("after-once-told");
        save(&state, &wait_for("web-billing", "web", &["billing@api"])).unwrap();
        let log = [end("billing@api", 200, "dropped", None)];
        let first = ticked(&state, &log, 210).1;
        let second = ticked(&state, &log, 230).1;
        assert_eq!((first.len(), second.len()), (1, 0));
        let source = include_str!("after.rs");
        let real = &source[source.find("\nfn tell_for_real(").unwrap()..];
        assert!(real[..real.find("\n}\n").unwrap()].contains("Told::Blocked { address, repo, text } => (Some((crate::notify::Event::NeedsYou, address, repo)), text, true)"), "the user hears it as needs-you, and the lead live");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_blocked_queued_task_never_starts_by_itself() {
        let state = crate::sh::test_dir("after-stays");
        save(&state, &Wait { blocked: Some("billing@api was dropped, so web-billing did not start".to_string()), ..wait_for("web-billing", "web", &["billing@api"]) }).unwrap();
        let (started, told) = ticked(&state, &[end("billing@api", 300, "done", None)], 310);
        assert!(started.is_empty() && told.is_empty(), "a new billing that is done does not undo the user's say");
        assert!(load(&state, "web-billing@web").unwrap().blocked.is_some());
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_queued_task_that_fails_to_start_waits_for_you_with_the_error() {
        let state = crate::sh::test_dir("after-failed");
        save(&state, &wait_for("web-billing", "web", &["billing@api"])).unwrap();
        let mut told = Vec::new();
        let notes = tick_in(&state, &[end("billing@api", 200, "done", None)], 210, &mut |_, _, _| Err("herdr is not running".to_string()), &mut |said| told.push(said));
        assert_eq!(load(&state, "web-billing@web").unwrap().blocked.as_deref(), Some("kelpie could not start it: herdr is not running"));
        assert!(!claimed_file(&state, "web-billing@web").exists());
        assert!(matches!(told.as_slice(), [Told::Blocked { .. }]) && notes[0].contains("herdr is not running"), "{notes:?}");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_claim_left_by_a_start_that_never_returned_goes_back_to_you_or_away_once_the_task_runs() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(CLAIM_HOLDS + 5);
        let lost = Wait { task: "search".to_string(), repo: dir.clone(), after: vec![Dep { address: "x@y".to_string(), repo: dir.clone() }], prompt: "Search".to_string(), mode: None, profile: None, backlog: None, by: By::User, at: 100, blocked: None, gone_since: None, why: None, starts_at: None };
        let running = Wait { task: "billing".to_string(), ..lost.clone() };
        for wait in [&lost, &running] {
            crate::state::write(&claimed_file(&state, &wait.address()), wait.to_json().to_string()).unwrap();
            fs::File::options().write(true).open(claimed_file(&state, &wait.address())).unwrap().set_modified(old).unwrap();
        }
        let fresh = Wait { task: "fresh".to_string(), ..lost.clone() };
        crate::state::write(&claimed_file(&state, &fresh.address()), fresh.to_json().to_string()).unwrap();
        let (started, told) = ticked(&state, &[], 500);
        assert!(started.is_empty());
        assert_eq!(load(&state, &lost.address()).unwrap().blocked.as_deref(), Some("kelpie was interrupted while starting it"), "the lost wait is the user's again, under Needs you");
        assert!(matches!(told.as_slice(), [Told::Blocked { address, .. }] if *address == lost.address()));
        assert!(load(&state, &running.address()).is_none() && !claimed_file(&state, &running.address()).exists(), "a claim whose task runs is just cleared");
        assert!(claimed_file(&state, &fresh.address()).exists(), "a start still under way keeps its claim");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn starting_a_queued_name_by_hand_is_refused_and_a_wait_whose_task_runs_anyway_is_dropped() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let state = state_of(&dir);
        save(&state, &Wait { task: "search".to_string(), repo: dir.clone(), ..wait_for("search", "web", &["other@web"]) }).unwrap();
        let search = peers::address("search", &name_of(&dir));
        assert_eq!(queued_refusal(&state, &search).unwrap_err(), "– It is queued\n  next: kelpie after search --now starts it; --drop drops the wait.\n  task: search");
        assert!(queued_refusal(&state, "search@elsewhere").is_ok() && queued_refusal(&state, &peers::address("billing", &name_of(&dir))).is_ok(), "begin refuses by address");
        assert!(queued_name_refusal(&state, "Search").unwrap_err().contains("kelpie after search --now starts it"), "kelpie start refuses the name in every repo");
        let main = include_str!("../main.rs");
        let start = &main[main.find("Some(\"start\") => {").unwrap()..];
        assert!(start.find("tasks::refuse_queued_name(name)?").unwrap() < start.find("tasks::start(repo").unwrap());
        let source = include_str!("launch.rs");
        let begin = &source[source.find("\npub fn begin(").unwrap()..];
        let begin = &begin[..begin.find("\n}\n").unwrap()];
        assert!(begin.find("queued_refusal(").unwrap() < begin.find("fetch(").unwrap(), "every start, the board's too, is refused before a worktree is claimed");
        forget(&state, &peers::address("search", &name_of(&dir)));
        let mut told = Vec::new();
        save(&state, &Wait { task: "billing".to_string(), repo: dir.clone(), ..wait_for("billing", "web", &["other@web"]) }).unwrap();
        tick_in(&state, &[end("other@web", 200, "done", None)], 210, &mut |_, _, _| panic!("a task that runs is never launched again"), &mut |said| told.push(said));
        let address = peers::address("billing", &name_of(&dir));
        assert!(load(&state, &address).is_none(), "its wait is dropped");
        assert!(told.iter().any(|said| matches!(said, Told::Dropped { text } if text.starts_with(&format!("{address} was started while it was queued")))));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_wait_with_halves_in_two_repos_starts_each_half_in_a_round_and_with_now() {
        let state = crate::sh::test_dir("after-halves");
        let halves = [wait_for("web-billing", "api", &["billing@x"]), wait_for("web-billing", "web", &["billing@x"])];
        let starts = RefCell::new(Vec::new());
        let mut begin = |repo: &Path, task: &str, _: &Start| -> Result<Begun> {
            queued_refusal(&state, &peers::address(task, &name_of(repo)))?;
            starts.borrow_mut().push(peers::address(task, &name_of(repo)));
            begun("Started.")
        };
        for half in &halves {
            save(&state, half).unwrap();
        }
        tick_in(&state, &[end("billing@x", 200, "done", None)], 210, &mut begin, &mut |_| {});
        assert_eq!(*starts.borrow(), ["web-billing@api", "web-billing@web"], "neither half's start is refused by the other's wait");
        assert!(all(&state).is_empty(), "no half is left blocked");
        for half in &halves {
            save(&state, half).unwrap();
        }
        now_in(&state, &Caller::User, "web-billing", &[], &mut begin, &mut |_| {}).unwrap();
        assert_eq!(starts.borrow().len(), 4, "--now starts both halves");
        for half in &halves {
            save(&state, half).unwrap();
        }
        let mut first_fails = |repo: &Path, task: &str, _: &Start| -> Result<Begun> {
            if name_of(repo) == "api" { Err("herdr is not running".to_string()) } else { starts.borrow_mut().push(peers::address(task, &name_of(repo))); begun("Started.") }
        };
        assert!(now_in(&state, &Caller::User, "web-billing", &[], &mut first_fails, &mut |_| {}).unwrap_err().contains("herdr is not running"));
        assert_eq!(starts.borrow().len(), 5, "a half that fails does not keep the other from starting");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn dropping_or_starting_a_wait_blocked_on_red_ci_lets_that_watch_go() {
        let state = crate::sh::test_dir("after-red-release");
        let red = crate::ci::Watch { address: "billing@api".to_string(), task: "billing".to_string(), repo: PathBuf::from("/repos/api"), slug: "acme/api".to_string(), sha: "abcdef0".to_string(), started_at: 10, checked_at: None, close: false, runs: Vec::new(), state: crate::ci::State::Failed { at: 20, what: "CI · test".to_string(), url: String::new() }, error: None, pushed: crate::ci::Pushed::default() };
        let blocked = Wait { blocked: Some("billing@api landed with red CI, so web-billing did not start".to_string()), ..wait_for("web-billing", "web", &["billing@api"]) };
        crate::ci::save(&state, &red).unwrap();
        save(&state, &blocked).unwrap();
        save(&state, &wait_for("search", "web", &["billing@api"])).unwrap();
        drop_in(&state, &Caller::User, "web-billing").unwrap();
        assert!(crate::ci::load(&state, "billing@api").is_some(), "search still waits on it");
        drop_in(&state, &Caller::User, "search").unwrap();
        assert!(crate::ci::load(&state, "billing@api").is_none(), "no wait names it: it goes");
        crate::ci::save(&state, &red).unwrap();
        save(&state, &blocked).unwrap();
        now_in(&state, &Caller::User, "web-billing", &[], &mut |_, _, _| begun("Started."), &mut |_| {}).unwrap();
        assert!(crate::ci::load(&state, "billing@api").is_none(), "started anyway: it goes too");
        let running = crate::ci::Watch { state: crate::ci::State::Running, ..red.clone() };
        crate::ci::save(&state, &running).unwrap();
        save(&state, &wait_for("web-billing", "web", &["billing@api"])).unwrap();
        drop_in(&state, &Caller::User, "web-billing").unwrap();
        assert!(crate::ci::load(&state, "billing@api").is_some(), "a running watch goes on until green");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_new_task_by_a_done_tasks_name_keeps_the_ci_watch_a_wait_needs() {
        let state = crate::sh::test_dir("after-new-name");
        let watch = crate::ci::Watch { address: "billing@api".to_string(), task: "billing".to_string(), repo: PathBuf::from("/repos/api"), slug: "acme/api".to_string(), sha: "abcdef0".to_string(), started_at: 10, checked_at: None, close: false, runs: Vec::new(), state: crate::ci::State::Running, error: None, pushed: crate::ci::Pushed::default() };
        crate::ci::save(&state, &watch).unwrap();
        save(&state, &wait_for("web-billing", "web", &["billing@api"])).unwrap();
        forget_unwaited_watch(&state, "billing@api");
        assert!(crate::ci::load(&state, "billing@api").is_some(), "the wait still needs it");
        forget(&state, "web-billing@web");
        forget_unwaited_watch(&state, "billing@api");
        assert!(crate::ci::load(&state, "billing@api").is_none(), "nothing waits on it: a leftover goes");
        assert!(include_str!("launch.rs").contains("super::after::forget_unwaited_watch(&state_dir()?, &address);") && !include_str!("launch.rs").contains("crate::ci::forget("));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_wait_in_a_circle_is_refused_and_changes_nothing() {
        let state = crate::sh::test_dir("after-circle");
        let b = Wait { task: "b".to_string(), repo: PathBuf::from("/repos/x"), ..wait_for("b", "x", &["a@x"]) };
        save(&state, &b).unwrap();
        let known = known(&["a@x"], vec![b], &[]);
        let error = plan("a@x", "Do a", &["b"], &known).unwrap_err();
        assert_eq!(error, "a@x cannot wait in a circle: a@x → b@x → a@x");
        let asked = Asked { name: "a", prompt: "Do a", after: &["b"], mode: None, profile: None, backlog: None, title: None, why: None, starts_at: None };
        assert!(queue_in(&state, &Caller::User, &[PathBuf::from("/repos/x")], &asked, &known, 100).is_err());
        assert_eq!(all(&state).iter().map(Wait::address).collect::<Vec<_>>(), ["b@x"]);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_wait_on_itself_is_refused() {
        let known = known(&["web-billing@api"], Vec::new(), &[]);
        for asked in ["web-billing", "web-billing@api", "Web Billing"] {
            assert_eq!(plan("web-billing@web", "Use it", &[asked], &known).unwrap_err(), "web-billing@web cannot wait for itself", "{asked}");
        }
    }

    #[test]
    fn a_wait_on_an_unknown_task_is_refused_and_changes_nothing() {
        let state = crate::sh::test_dir("after-unknown");
        let known = known(&["billing@api"], Vec::new(), &[end("search@api", 50, "dropped", None)]);
        for asked in ["nothing", "billing@web", "search"] {
            assert!(plan("web-billing@web", "Use it", &[asked], &known).unwrap_err().contains("no task, queued task or task done by that name"), "{asked}");
        }
        let asked = Asked { name: "web-billing", prompt: "Use it", after: &["billing", "nothing"], mode: None, profile: None, backlog: None, title: None, why: None, starts_at: None };
        assert!(queue_in(&state, &Caller::User, &[PathBuf::from("/repos/web")], &asked, &known, 100).is_err());
        assert!(all(&state).is_empty(), "one unknown name refuses the whole wait");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_wait_for_a_name_already_running_or_queued_is_refused() {
        let running = known(&["billing@api", "web-billing@web"], Vec::new(), &[]);
        assert_eq!(plan("web-billing@web", "Use it", &["billing"], &running).unwrap_err(), "web-billing is already running (web-billing@web), so it cannot wait");
        assert_eq!(plan("web-billing@api", "Use it", &["billing"], &running).unwrap_err(), "web-billing is already running (web-billing@web), so it cannot wait", "a half in another repo is the same task");
        let queued = known(&["billing@api"], vec![wait_for("web-billing", "web", &["billing@api"])], &[]);
        for me in ["web-billing@web", "web-billing@api"] {
            assert!(plan(me, "Use it", &["billing"], &queued).unwrap_err().starts_with("web-billing is already queued (web-billing@web)"), "{me}");
        }
    }

    #[test]
    fn only_the_user_or_the_lead_can_queue_or_change_a_wait() {
        let state = crate::sh::test_dir("after-who");
        let known = known(&["billing@api"], Vec::new(), &[]);
        let asked = Asked { name: "web-billing", prompt: "Use it", after: &["billing"], mode: None, profile: None, backlog: None, title: None, why: None, starts_at: None };
        for caller in [Caller::Task("billing@api".to_string()), Caller::Agent] {
            assert!(queue_in(&state, &caller, &[PathBuf::from("/repos/web")], &asked, &known, 100).unwrap_err().contains("only the user or the lead"));
            assert!(all(&state).is_empty());
        }
        assert!(queue_in(&state, &Caller::Lead, &[PathBuf::from("/repos/web")], &asked, &known, 100).unwrap().is_some());
        assert_eq!(load(&state, "web-billing@web").unwrap().by, By::Lead);
        for caller in [Caller::Task("billing@api".to_string()), Caller::Agent] {
            assert!(now_in(&state, &caller, "web-billing", &[], &mut |_, _, _| panic!("never started"), &mut |_| {}).is_err());
            assert!(drop_in(&state, &caller, "web-billing").is_err());
            assert!(load(&state, "web-billing@web").is_some());
        }
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_tasks_settings_deny_changing_a_wait() {
        let written: Value = serde_json::from_str(&crate::agent::claude::settings("/bin/kelpie", "billing@app", None, None, None)).unwrap();
        assert!(written["permissions"]["deny"].as_array().unwrap().contains(&json!("Bash(/bin/kelpie after:*)")));
    }

    #[test]
    fn a_queued_task_needs_a_prompt() {
        let known = known(&["billing@api"], Vec::new(), &[]);
        for prompt in ["", "   "] {
            assert!(plan("web-billing@web", prompt, &["billing"], &known).unwrap_err().contains("needs its prompt to wait"));
        }
    }

    #[test]
    fn starting_a_queued_task_now_tells_its_claude_what_is_not_done() {
        let state = crate::sh::test_dir("after-now");
        let mut wait = wait_for("web-billing", "web", &["billing@api", "search@api"]);
        wait.after[1].repo = PathBuf::from("/nowhere");
        save(&state, &wait).unwrap();
        let mut prompts = Vec::new();
        let said = now_in(&state, &Caller::User, "web-billing", &[end("search@api", 200, "done", Some("abc Index invoices"))], &mut |_, _, options| { prompts.push((options.prompt.unwrap().to_string(), options.focus)); begun("Started web-billing.") }, &mut |_| {}).unwrap();
        assert_eq!(said, "Started web-billing.");
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].0.starts_with("This task waited for billing@api, which was not done (gone) when the user started it anyway"), "{}", prompts[0].0);
        assert!(prompts[0].0.contains("This task waited for search@api, which is done. Its commits (subjects as that task wrote them):\n> abc Index invoices") && prompts[0].0.ends_with("\n\nUse the rounding field"));
        assert!(!prompts[0].1);
        assert!(all(&state).is_empty() && !claimed_file(&state, "web-billing@web").exists());
        assert!(now_in(&state, &Caller::User, "web-billing", &[], &mut |_, _, _| panic!("nothing is queued"), &mut |_| {}).unwrap_err().contains("is not queued"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn dropping_a_wait_puts_its_backlog_item_back() {
        let state = crate::sh::test_dir("after-drop");
        save(&state, &Wait { backlog: Some("local:Fix login".to_string()), ..wait_for("web-billing", "web", &["billing@api"]) }).unwrap();
        assert!(held_items(&state).contains("local:Fix login"), "the Backlog column hides it while it waits");
        let said = drop_in(&state, &Caller::User, "web-billing").unwrap();
        assert_eq!(said, "✓ Dropped the wait; nothing was started\n  next: Its backlog item is back in the Backlog column.\n  task: web-billing@web\n  item: local:Fix login");
        assert!(held_items(&state).is_empty() && all(&state).is_empty());
        let source = include_str!("started.rs");
        assert!(source.contains("super::after::held_items(&state)"), "started_items takes in what waits hold");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_wait_file_that_does_not_read_as_one_is_removed_unread() {
        let state = crate::sh::test_dir("after-broken");
        let good = wait_for("web-billing", "web", &["billing@api"]);
        let mut forged = good.to_json();
        forged["by"] = json!("agent");
        for (name, text) in [("web-billing@web.json", "not json".to_string()), ("web-billing@web.json", forged.to_string()), ("other@web.json", good.to_json().to_string())] {
            crate::state::write(&dir(&state).join(name), text).unwrap();
            let (started, _) = ticked(&state, &[end("billing@api", 200, "done", None)], 210);
            assert!(started.is_empty(), "{name}");
            assert!(!dir(&state).join(name).exists(), "{name} is removed unread");
        }
        for broken in [json!({ "after": [] }), json!({ "prompt": "" }), json!({ "task": "../x" })] {
            let mut bad = good.to_json();
            bad.as_object_mut().unwrap().extend(broken.as_object().unwrap().clone());
            crate::state::write(&file(&state, "web-billing@web"), bad.to_string()).unwrap();
            assert!(all(&state).is_empty(), "{broken}");
        }
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_lead_role_says_start_after_rather_than_holding_the_order_itself() {
        let role = include_str!("../../roles/lead.md");
        let starting = &role[role.find("- **Starting a task**").unwrap()..];
        let starting = &starting[..starting.find("- **Holding tasks to the rules**").unwrap()];
        assert!(starting.contains("--after <task[@repo]>") && starting.contains("never hold the order yourself"));
        assert!(starting.contains("on your own initiative") && starting.contains("say so in your next summary"));
        assert!(starting.contains("kelpie after <task> --now") && starting.contains("on their word"), "a blocked wait is the user's");
    }

    #[test]
    fn a_queued_task_keeps_its_profile_and_starts_with_it() {
        let state = crate::sh::test_dir("after-profile");
        let wait = Wait { profile: Some("designer".to_string()), ..wait_for("web-billing", "web", &["billing@api"]) };
        assert_eq!(Wait::from_json(&wait.to_json()), Some(wait.clone()));
        save(&state, &wait).unwrap();
        let started = RefCell::new(Vec::new());
        tick_in(&state, &[end("billing@api", 200, "done", None)], 210, &mut |_, _, options| { started.borrow_mut().push(options.profile.map(str::to_string)); begun("Started.") }, &mut |_| {});
        assert_eq!(started.into_inner(), [Some("designer".to_string())]);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_queued_tasks_start_never_takes_focus() {
        let state = crate::sh::test_dir("after-focus");
        save(&state, &Wait { mode: Some("auto".to_string()), ..wait_for("web-billing", "web", &["billing@api"]) }).unwrap();
        let (started, _) = ticked(&state, &[end("billing@api", 200, "done", None)], 210);
        assert!(matches!(started.as_slice(), [Launched { focus: false, from: None, note: true, .. }]), "fresh from the default branch, with its note, and the user stays where they are");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_keeper_runs_the_after_check_and_the_board_does_when_it_is_off() {
        let keeper = crate::keeper::routines();
        let after = keeper.iter().find(|routine| routine.name == "after").expect("the keeper runs the after check");
        assert_eq!(after.every, EVERY);
        let threads = include_str!("../board/threads.rs");
        let runner = &threads[threads.find("pub(super) fn spawn_routine_runner(").unwrap()..];
        let runner = &runner[..runner.find("\n}\n").unwrap()];
        assert!(runner.contains("crate::keeper::Owner::Board") && runner.contains("crate::tasks::after_tick(&state)"), "with the keeper off the board runs it");
    }
}
