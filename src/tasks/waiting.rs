use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::messages::{self, Message};
use crate::notice::{Notice, Tone};
use crate::peers::{self, task_part};
use crate::permit::Caller;
use crate::repo::{default_branch, main_checkout, name_of};
use crate::sh::Result;
use super::after::{self, Dep, DepState, Facts, Known, Lock, and_list};
use super::follow::Step;
use super::pool::{find_task, slots, task_of};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Waiting {
    pub address: String,
    pub repo: PathBuf,
    pub on: Vec<Dep>,
    pub then: Option<String>,
    pub by: String,
    pub at: u64,
    pub gone_since: Option<u64>,
    pub unlisted_since: Option<u64>,
    pub unlisted_told: bool,
}

impl Waiting {
    fn to_json(&self) -> Value {
        json!({
            "address": self.address, "repo": self.repo,
            "on": self.on.iter().map(|dep| json!({ "address": dep.address, "repo": dep.repo })).collect::<Vec<_>>(),
            "then": self.then, "by": self.by, "at": self.at, "gone_since": self.gone_since,
            "unlisted_since": self.unlisted_since, "unlisted_told": self.unlisted_told,
        })
    }

    fn from_json(value: &Value) -> Option<Waiting> {
        let on = value["on"].as_array()?.iter().map(|dep| Some(Dep { address: dep["address"].as_str()?.to_string(), repo: PathBuf::from(dep["repo"].as_str()?) })).collect::<Option<Vec<_>>>()?;
        let by = value["by"].as_str().filter(|by| matches!(*by, "user" | "lead"))?.to_string();
        Some(Waiting {
            address: value["address"].as_str().filter(|address| address.contains('@'))?.to_string(),
            repo: PathBuf::from(value["repo"].as_str()?),
            on: Some(on).filter(|on| !on.is_empty())?,
            then: value["then"].as_str().map(str::to_string),
            by,
            at: value["at"].as_u64()?,
            gone_since: value["gone_since"].as_u64(),
            unlisted_since: value["unlisted_since"].as_u64(),
            unlisted_told: value["unlisted_told"].as_bool().unwrap_or(false),
        })
    }

    fn names(&self) -> String {
        and_list(&self.on.iter().map(|dep| dep.address.clone()).collect::<Vec<_>>())
    }

    fn is_are(&self) -> &'static str {
        if self.on.len() == 1 { "is" } else { "are" }
    }
}

fn dir(state: &Path) -> PathBuf {
    state.join("waiting")
}

fn file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.json"))
}

pub(crate) fn save(state: &Path, wait: &Waiting) -> Result<()> {
    crate::state::write(&file(state, &wait.address), serde_json::to_string_pretty(&wait.to_json()).unwrap_or_default())
}

pub(crate) fn load(state: &Path, address: &str) -> Option<Waiting> {
    Waiting::from_json(&serde_json::from_str(&fs::read_to_string(file(state, address)).ok()?).ok()?).filter(|wait| wait.address == address)
}

fn forget(state: &Path, wait: &Waiting) {
    let _ = fs::remove_file(file(state, &wait.address));
    after::release_deps(state, wait.on.iter());
}

pub(crate) fn all(state: &Path) -> Vec<Waiting> {
    let Ok(entries) = fs::read_dir(dir(state)) else { return Vec::new() };
    let mut found: Vec<Waiting> = entries.flatten().filter_map(|entry| {
        let name = entry.file_name().to_string_lossy().to_string();
        load(state, name.strip_suffix(".json").filter(|_| !name.starts_with('.'))?)
    }).collect();
    found.sort_by(|one, other| one.address.cmp(&other.address));
    found
}

pub(super) fn waited_on(state: &Path, address: &str) -> bool {
    all(state).iter().any(|wait| wait.on.iter().any(|dep| dep.address == address))
}

fn waiter(caller: &Caller) -> Result<&'static str> {
    match caller {
        Caller::User => Ok("user"),
        Caller::Lead => Ok("lead"),
        other => Err(format!("making a task wait for another is the user's call, or the lead's on their word; {} cannot", crate::permit::describe(other))),
    }
}

fn who(by: &str) -> &'static str {
    if by == "lead" { "The lead" } else { "The user" }
}

// Running waits and queued tasks both wait, so a circle through either would leave every task in it waiting for good.
fn circle(me: &str, deps: &[Dep], known: &Known, waiting: &[Waiting]) -> Option<Vec<String>> {
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for wait in &known.waits {
        edges.entry(wait.address()).or_default().extend(wait.after.iter().map(|dep| dep.address.clone()));
    }
    for wait in waiting.iter().filter(|wait| wait.address != me) {
        edges.entry(wait.address.clone()).or_default().extend(wait.on.iter().map(|dep| dep.address.clone()));
    }
    fn walk(at: &str, me: &str, edges: &BTreeMap<String, Vec<String>>, path: &mut Vec<String>) -> bool {
        path.push(at.to_string());
        if at == me {
            return true;
        }
        if path[..path.len() - 1].iter().any(|seen| seen == at) {
            path.pop();
            return false;
        }
        for next in edges.get(at).into_iter().flatten() {
            if walk(next, me, edges, path) {
                return true;
            }
        }
        path.pop();
        false
    }
    deps.iter().find_map(|dep| {
        let mut path = vec![me.to_string()];
        walk(&dep.address, me, &edges, &mut path).then_some(path)
    })
}

pub(crate) struct Asked<'a> {
    pub address: &'a str,
    pub repo: &'a Path,
    pub on: &'a [&'a str],
    pub then: Option<&'a str>,
}

// Every task named is checked before anything is written, so a refusal changes nothing; a wait set again replaces the one before.
pub(crate) fn wait_in(state: &Path, caller: &Caller, asked: &Asked, known: &Known, now: u64, tell: &mut dyn FnMut(&str, &str)) -> Result<String> {
    let by = waiter(caller)?;
    let Asked { address, repo, on, then } = *asked;
    if on.is_empty() {
        return Err(format!("kebbie wait needs a task to wait for: kebbie wait {} --on <task[@repo]> \"<what to do once it lands>\"", task_part(address)));
    }
    let deps: Vec<Dep> = after::resolve(address, on, known)?.into_iter().filter(|dep| dep.address != address).collect();
    if deps.is_empty() {
        return Notice::new(Tone::NotNow, "Nothing to wait for: what it names is done already").detail("task", address).err();
    }
    let _lock = Lock::in_dir(&dir(state), true);
    let waiting = all(state);
    if let Some(path) = circle(address, &deps, known, &waiting) {
        return Err(format!("{address} cannot wait in a circle: {}", path.join(" → ")));
    }
    let then = then.map(str::trim).filter(|then| !then.is_empty()).map(str::to_string);
    let wait = Waiting { address: address.to_string(), repo: repo.to_path_buf(), on: deps, then, by: by.to_string(), at: now, gone_since: None, unlisted_since: None, unlisted_told: false };
    save(state, &wait)?;
    let main = default_branch(repo).map_or("the default branch".to_string(), |branch| branch.trim_start_matches("origin/").to_string());
    let then = wait.then.as_deref().map_or(String::new(), |then| format!(" Once they land: {then}."));
    tell(address, &format!(
        "{} recorded that this task waits for {}.{then} kebbie rebases this task onto {main} and tells you once {} {} pushed and CI is green, and tells you if one is dropped instead. Until then, go on with the work that does not need {}, or report blocked naming {}.",
        who(by), wait.names(), wait.names(), wait.is_are(), if wait.on.len() == 1 { "it" } else { "them" }, if wait.on.len() == 1 { "it" } else { "them" },
    ));
    Notice::new(Tone::Done, format!("It waits for {}", wait.names()))
        .next(format!("It shows under Waiting on. kebbie rebases it and tells its Claude once {} {} pushed and CI is green; kebbie wait {} --off ends the wait.", wait.names(), wait.is_are(), task_part(address)))
        .detail("task", address).ok()
}

pub(crate) fn off_in(state: &Path, caller: &Caller, address: &str, tell: &mut dyn FnMut(&str, &str)) -> Result<String> {
    let by = waiter(caller)?;
    let _lock = Lock::in_dir(&dir(state), true);
    let Some(wait) = load(state, address) else {
        return Notice::new(Tone::NotNow, "It waits for nothing").detail("task", address).err();
    };
    forget(state, &wait);
    tell(address, &format!("{} took this task off its wait for {}: go on without waiting for {}.", who(by), wait.names(), if wait.on.len() == 1 { "it" } else { "them" }));
    Notice::new(Tone::Done, format!("It no longer waits for {}", wait.names())).next("It is back in its group.").detail("task", address).ok()
}

// A push of a task that waits for another goes out before what it waits for, so it is asked for twice.
pub(super) fn refuse_push(state: &Path, address: &str, task: &str) -> Result<()> {
    match load(state, address) {
        Some(wait) => Notice::new(Tone::NotNow, format!("Not pushed: it waits for {}", wait.names()))
            .next(format!("kebbie push {task} --anyway pushes it before them; kebbie wait {task} --off ends the wait.")).detail("task", address).err(),
        None => Ok(()),
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Next {
    Wait,
    Missing,
    Landed,
    Lost(Vec<(Dep, String)>),
}

// A task waited for that is dropped, reset, landed with red CI or gone will never land as the waiter needs, so the waiter hears it rather than waiting for good.
pub(crate) fn next(wait: &Waiting, facts: &Facts, now: u64) -> Next {
    let states: Vec<(&Dep, DepState)> = wait.on.iter().map(|dep| (dep, facts.state(&dep.address, wait.at))).collect();
    let gone_long = wait.gone_since.is_some_and(|since| now.saturating_sub(since) >= after::GRACE);
    let lost: Vec<(Dep, String)> = states.iter().filter_map(|(dep, state)| match state {
        DepState::Ended(how) => Some(((*dep).clone(), format!("was {how}"))),
        DepState::CiFailed => Some(((*dep).clone(), "landed with red CI".to_string())),
        DepState::Gone if gone_long => Some(((*dep).clone(), "is gone without being done".to_string())),
        _ => None,
    }).collect();
    if !lost.is_empty() {
        return Next::Lost(lost);
    }
    if states.iter().any(|(_, state)| *state == DepState::Gone) {
        return Next::Missing;
    }
    if states.iter().all(|(_, state)| *state == DepState::Done) { Next::Landed } else { Next::Wait }
}

pub(crate) enum Told {
    Task { address: String, text: String },
    Lead { text: String, asks: bool },
}

// None leaves the wait for the next round: the waiter's Claude is mid-turn, so a rebase would move the code under it.
pub(crate) type Resume<'a> = &'a mut dyn FnMut(&Waiting) -> Option<String>;

pub(crate) fn tick_in(state: &Path, facts: &Facts, now: u64, resume: Resume, tell: &mut dyn FnMut(Told)) -> Vec<String> {
    let Some(_lock) = Lock::in_dir(&dir(state), false) else { return Vec::new() };
    let mut notes = Vec::new();
    for wait in all(state) {
        if !facts.open.contains(&wait.address) && facts.unlisted.contains(&wait.repo) {
            unlisted(state, wait, now, tell, &mut notes);
            continue;
        }
        if !facts.open.contains(&wait.address) {
            forget(state, &wait);
            continue;
        }
        let wait = match wait.unlisted_since {
            Some(_) => {
                let listed = Waiting { unlisted_since: None, unlisted_told: false, ..wait };
                let _ = save(state, &listed);
                listed
            }
            None => wait,
        };
        match next(&wait, facts, now) {
            Next::Wait if wait.gone_since.is_some() => {
                let _ = save(state, &Waiting { gone_since: None, ..wait });
            }
            Next::Wait => {}
            Next::Missing if wait.gone_since.is_none() => {
                let _ = save(state, &Waiting { gone_since: Some(now), ..wait });
            }
            Next::Missing => {}
            Next::Lost(lost) => {
                let left: Vec<Dep> = wait.on.iter().filter(|dep| !lost.iter().any(|(gone, _)| gone == *dep)).cloned().collect();
                let what = and_list(&lost.iter().map(|(dep, how)| format!("{}, which {how}", dep.address)).collect::<Vec<_>>());
                let rest = if left.is_empty() { "waits for nothing now".to_string() } else { format!("waits only for {} now", and_list(&left.iter().map(|dep| dep.address.clone()).collect::<Vec<_>>())) };
                tell(Told::Task { address: wait.address.clone(), text: format!("This task waited for {what}. That will not land as this task needs, so this task {rest}. Decide how it goes on without it, and ask the user with kebbie ask when that is theirs to choose.") });
                tell(Told::Lead { text: format!("{} waited for {what}; kebbie told it, and it {rest}.", wait.address), asks: true });
                notes.push(format!("waiting: {}: {what}", wait.address));
                if left.is_empty() {
                    forget(state, &wait);
                } else {
                    let _ = save(state, &Waiting { on: left, gone_since: None, ..wait.clone() });
                    after::release_deps(state, lost.iter().map(|(dep, _)| dep));
                }
            }
            Next::Landed => {
                let Some(rebased) = resume(&wait) else { continue };
                let landed: Vec<String> = wait.on.iter().map(|dep| after::landed_lines(&dep.address, facts.ends.get(&dep.address))).collect();
                let then = wait.then.as_deref().map_or("Go on with the work that waited for it.".to_string(), |then| format!("What to do now: {then}"));
                tell(Told::Task { address: wait.address.clone(), text: format!("{}\n\n{rebased} {then}", landed.join("\n")) });
                tell(Told::Lead { text: format!("{} {} done, so kebbie told {} to go on: {rebased}", wait.names(), wait.is_are(), wait.address), asks: false });
                notes.push(format!("waiting: {} goes on", wait.address));
                forget(state, &wait);
            }
        }
    }
    notes
}

// A repo git cannot list says nothing about whether the waiter is gone, so its wait stays; the lead hears once it outlasts the grace a task waited for gets.
fn unlisted(state: &Path, wait: Waiting, now: u64, tell: &mut dyn FnMut(Told), notes: &mut Vec<String>) {
    match wait.unlisted_since {
        None => {
            let _ = save(state, &Waiting { unlisted_since: Some(now), ..wait });
        }
        Some(since) if !wait.unlisted_told && now.saturating_sub(since) >= after::GRACE => {
            tell(Told::Lead { text: format!("kebbie has not been able to list the worktrees of {} for {} seconds, so it cannot tell whether {} is still open. It keeps its wait for {} until it can; `git worktree list` in that repo shows what fails.", wait.repo.display(), now.saturating_sub(since), wait.address, wait.names()), asks: true });
            notes.push(format!("waiting: {}: its repo cannot be listed", wait.address));
            let _ = save(state, &Waiting { unlisted_told: true, ..wait });
        }
        Some(_) => {}
    }
}

fn main_of(repo: &Path) -> String {
    default_branch(repo).unwrap_or_else(|_| "origin/main".to_string())
}

// The rebase follow does, while the task's Claude is idle and its files are committed; otherwise its Claude rebases once it can.
fn resume_live(state: &Path, wait: &Waiting, log: &[Message]) -> Option<String> {
    let upstream = main_of(&wait.repo);
    let yourself = |why: &str| format!("kebbie left the rebase to you ({why}): once your work is committed, rebase onto {upstream} (`git rebase {upstream}`, fetched already).");
    let slot = slots(&wait.repo).ok()?.into_iter().find(|slot| slot.task.as_deref() == Some(task_part(&wait.address)))?;
    if super::fetch(&wait.repo).is_err() {
        return Some(yourself("its fetch failed"));
    }
    let (Ok(workspaces), Ok(agents)) = (crate::herdr::workspaces(), crate::herdr::agents()) else { return None };
    let idle = super::follow::waiting_of(&slot.path, &workspaces, &agents);
    if idle == Some(false) {
        return None;
    }
    let asks = crate::questions::open_now(log).iter().any(|question| question.from == wait.address);
    let told = |text: &str| {
        if let Err(error) = messages::notify(&wait.address, text) {
            eprintln!("Could not tell {} about the rebase: {error}", wait.address);
        }
    };
    Some(match super::follow::step(state, &slot, &wait.address, idle, asks, &super::push::hand_over, &told) {
        Ok(Step::Current) => format!("This task stands on {upstream} already."),
        Ok(Step::Rebased(_)) => format!("kebbie rebased this task onto {upstream}."),
        Ok(Step::Handed) => format!("Rebasing it onto {upstream} conflicted: resolve the rebase as kebbie's message says first."),
        Ok(Step::Waits(why)) => yourself(why),
        Ok(Step::Stuck(why)) | Err(why) => yourself(&why),
    })
}

fn tell_for_real(told: Told) {
    match told {
        Told::Task { address, text } => {
            if let Err(error) = messages::notify(&address, &text) {
                eprintln!("Could not tell {address} what it waited for: {error}");
            }
        }
        Told::Lead { text, asks } => {
            if let Err(error) = messages::to_lead(if asks { messages::ToLead::ChainBlocked } else { messages::ToLead::TaskStarted }, &text) {
                eprintln!("Could not tell the lead: {error}");
            }
        }
    }
}

pub fn tick(state: &Path) -> Vec<String> {
    let waits = all(state);
    if waits.is_empty() {
        return Vec::new();
    }
    let mut repos: Vec<PathBuf> = waits.iter().flat_map(|wait| wait.on.iter().map(|dep| dep.repo.clone()).chain([wait.repo.clone()])).collect();
    repos.sort();
    repos.dedup();
    let log = messages::load();
    let facts = Facts::with_repos(state, &log, &repos);
    tick_in(state, &facts, messages::now(), &mut |wait| resume_live(state, wait, &log), &mut tell_for_real)
}

fn tell_task(address: &str, text: &str) {
    if let Err(error) = messages::notify(address, text) {
        eprintln!("Could not tell {address}: {error}");
    }
}

fn address_at(cwd: &Path, name: &str) -> Result<(PathBuf, String)> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, Some(name), cwd)?;
    let address = peers::address(task_of(&slot), &name_of(&repo));
    Ok((repo, address))
}

pub(crate) fn shown_line(wait: &Waiting, facts: &Facts) -> String {
    let deps: Vec<String> = wait.on.iter().map(|dep| format!("{} ({})", dep.address, after::label(&facts.state(&dep.address, wait.at), None))).collect();
    let then = wait.then.as_deref().map_or(String::new(), |then| format!("; once they land: {then}"));
    format!("{} waits for {}{then}", wait.address, and_list(&deps))
}

pub fn command(cwd: &Path, name: Option<&str>, on: &[&str], off: bool, then: &[String]) -> Result<String> {
    let state = crate::lead::state_dir()?;
    let Some(name) = name else {
        let waits = all(&state);
        if waits.is_empty() {
            return Ok("No task waits for another.".to_string());
        }
        let repos: Vec<PathBuf> = waits.iter().map(|wait| wait.repo.clone()).collect();
        let facts = Facts::with_repos(&state, &messages::load(), &repos);
        return Ok(waits.iter().map(|wait| shown_line(wait, &facts)).collect::<Vec<_>>().join("\n"));
    };
    let (repo, address) = address_at(cwd, name)?;
    let caller = crate::permit::caller();
    if off {
        if !on.is_empty() || !then.is_empty() {
            return Err("kebbie wait --off takes the task's name only".to_string());
        }
        return off_in(&state, &caller, &address, &mut |to, text| tell_task(to, text));
    }
    if on.is_empty() {
        return match load(&state, &address) {
            Some(wait) => Ok(shown_line(&wait, &Facts::with_repos(&state, &messages::load(), &[repo]))),
            None => Ok(format!("{address} waits for nothing: kebbie wait {} --on <task[@repo]> makes it wait.", task_part(&address))),
        };
    }
    let known = after::known(&state, std::slice::from_ref(&repo));
    let then = then.join(" ");
    wait_in(&state, &caller, &Asked { address: &address, repo: &repo, on, then: Some(then.as_str()).filter(|then| !then.trim().is_empty()) }, &known, messages::now(), &mut |to, text| tell_task(to, text))
}

// What a waiting task's row shows: each task it waits for with that task's state, and what ends the wait.
#[derive(Clone, Debug, PartialEq)]
pub struct WaitsOn {
    pub after: Vec<(String, String)>,
    pub names: Vec<String>,
    pub line: String,
}

fn waits_line(names: &[(String, String)], then: Option<&str>) -> String {
    let listed = and_list(&names.iter().map(|(name, label)| format!("{name} ({label})")).collect::<Vec<_>>());
    let then = then.map_or(String::new(), |then| format!(": {}", then.trim_end_matches('.')));
    let it = if names.len() == 1 { "it is" } else { "they are" };
    format!("Waits for {listed}{then}. kebbie rebases it and tells its Claude once {it} pushed and CI is green.")
}

// Each waiting task gets what it waits for, and each task waited for, by a running or a queued task, the titles of those it blocks.
pub(crate) fn mark(state: &Path, log: &[Message], items: &mut [crate::status::Item]) {
    let address = |item: &crate::status::Item| peers::address(&item.name, &item.place);
    let waits = all(state);
    if !waits.is_empty() {
        let open: HashSet<String> = items.iter().filter(|item| item.git.is_some()).map(address).collect();
        let queued = items.iter().filter(|item| item.queued.is_some()).map(address).collect();
        let ci = crate::ci::all(state).into_iter().map(|watch| (watch.address.clone(), watch.running())).collect();
        let facts = Facts { ends: after::latest_ends(log), open, queued, ci, unlisted: HashSet::new() };
        let shown: Vec<(String, WaitsOn)> = waits.iter().map(|wait| {
            let deps: Vec<(String, String, String)> = wait.on.iter().map(|dep| {
                let item = items.iter().find(|item| item.git.is_some() && address(item) == dep.address);
                let label = after::label(&facts.state(&dep.address, wait.at), item);
                let title = item.and_then(|item| item.title.clone()).or_else(|| crate::status::kept_title(state, &dep.address)).unwrap_or_else(|| task_part(&dep.address).to_string());
                let name = if dep.repo == wait.repo { title } else { format!("{title} in {}", name_of(&dep.repo)) };
                (dep.address.clone(), label, name)
            }).collect();
            let names: Vec<(String, String)> = deps.iter().map(|(_, label, name)| (name.clone(), label.clone())).collect();
            let line = waits_line(&names, wait.then.as_deref());
            (wait.address.clone(), WaitsOn { after: deps.iter().map(|(address, label, _)| (address.clone(), label.clone())).collect(), names: deps.into_iter().map(|(_, _, name)| name).collect(), line })
        }).collect();
        for item in items.iter_mut().filter(|item| item.git.is_some()) {
            item.waits_on = shown.iter().find(|(waiter, _)| *waiter == address(item)).map(|(_, waits)| waits.clone());
        }
    }
    let waiting: Vec<(String, String, Vec<String>)> = items.iter().filter_map(|item| {
        let on: Vec<String> = match (&item.waits_on, &item.queued) {
            (Some(waits), _) => waits.after.iter().map(|(address, _)| address.clone()).collect(),
            (None, Some(queued)) => queued.after.iter().filter(|(_, state)| state != "done").map(|(address, _)| address.clone()).collect(),
            (None, None) => return None,
        };
        Some((address(item), item.titled(), on))
    }).collect();
    for item in items.iter_mut() {
        let me = address(item);
        item.blocks = waiting.iter().filter(|(waiter, _, on)| *waiter != me && on.contains(&me)).map(|(_, title, _)| title.clone()).collect();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    use super::*;
    use super::after::{Wait, latest_ends};
    use super::super::intent::By;

    fn dep(address: &str) -> Dep {
        Dep { address: address.to_string(), repo: PathBuf::from(format!("/repos/{}", address.split_once('@').unwrap().1)) }
    }

    fn known(open: &[&str], waits: Vec<Wait>, ends: &[Message]) -> Known {
        Known { open: open.iter().map(|address| dep(address)).collect(), waits, ends: latest_ends(ends) }
    }

    fn end(address: &str, at: u64, how: &str, landed: Option<&str>) -> Message {
        Message { id: format!("end-{address}-{at}"), at, from: "kebbie".to_string(), to: address.to_string(), text: format!("{} ended ({how})", task_part(address)), held: false, report: None, ended: true, question: None, about: None, kind: None, landed: landed.map(str::to_string), why: None, forced: false, fyi: false }
    }

    fn facts(ends: &[Message], open: &[&str]) -> Facts {
        Facts { ends: latest_ends(ends), open: open.iter().map(|address| address.to_string()).collect(), queued: HashSet::new(), ci: HashMap::new(), unlisted: HashSet::new() }
    }

    fn asked<'a>(address: &'a str, on: &'a [&'a str], then: Option<&'a str>) -> Asked<'a> {
        Asked { address, repo: Path::new("/repos/web"), on, then }
    }

    type Heard = (Vec<String>, Vec<(String, String)>, Vec<(String, bool)>);

    fn told(state: &Path, facts: &Facts, now: u64, resumed: Option<&str>) -> Heard {
        let (tasks, lead) = (RefCell::new(Vec::new()), RefCell::new(Vec::new()));
        let notes = tick_in(state, facts, now, &mut |_| resumed.map(str::to_string), &mut |told| match told {
            Told::Task { address, text } => tasks.borrow_mut().push((address, text)),
            Told::Lead { text, asks } => lead.borrow_mut().push((text, asks)),
        });
        (notes, tasks.into_inner(), lead.into_inner())
    }

    #[test]
    fn a_running_task_waits_for_another_and_is_told_so() {
        let state = crate::sh::test_dir("waiting-set");
        let known = known(&["billing@web", "rounding@api"], Vec::new(), &[]);
        let mut said = Vec::new();
        let done = wait_in(&state, &Caller::Lead, &asked("billing@web", &["rounding@api"], Some("use the rounding field in the totals")), &known, 100, &mut |to, text| said.push((to.to_string(), text.to_string()))).unwrap();
        assert!(done.contains("It waits for rounding@api") && done.contains("kebbie wait billing --off ends the wait"), "{done}");
        assert_eq!(load(&state, "billing@web").map(|wait| (wait.on, wait.then, wait.by)), Some((vec![dep("rounding@api")], Some("use the rounding field in the totals".to_string()), "lead".to_string())), "read back as the keeper reads it");
        assert_eq!(said.len(), 1);
        assert_eq!(said[0].0, "billing@web");
        assert!(said[0].1.starts_with("The lead recorded that this task waits for rounding@api. Once they land: use the rounding field in the totals.") && said[0].1.contains("tells you once rounding@api is pushed and CI is green") && said[0].1.contains("report blocked naming it"), "{}", said[0].1);
        assert!(waited_on(&state, "rounding@api") && !waited_on(&state, "billing@web"));
        let off = off_in(&state, &Caller::User, "billing@web", &mut |to, text| said.push((to.to_string(), text.to_string()))).unwrap();
        assert!(off.contains("It no longer waits for rounding@api"), "{off}");
        assert!(said[1].1.starts_with("The user took this task off its wait for rounding@api"), "{}", said[1].1);
        assert_eq!(load(&state, "billing@web"), None);
        assert!(off_in(&state, &Caller::User, "billing@web", &mut |_, _| {}).unwrap_err().contains("It waits for nothing"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_waiting_task_shows_under_waiting_on_and_what_it_waits_for_shows_it_blocks() {
        use crate::next::tests::{asking, named, queued_after, ready};
        use crate::status::Group;
        let state = crate::sh::test_dir("waiting-shown");
        save(&state, &Waiting { address: "billing@web".to_string(), repo: PathBuf::from("/repos/web"), on: vec![Dep { address: "rounding@web".to_string(), repo: PathBuf::from("/repos/web") }], then: Some("use the rounding field".to_string()), by: "lead".to_string(), at: 100, gone_since: None, unlisted_since: None, unlisted_told: false }).unwrap();
        crate::status::keep_title(&state, "rounding@web", "Rounding field");
        let mut rounding = ready("rounding", "web", 10);
        rounding.title = Some("Rounding field".to_string());
        let mut billing = ready("billing", "web", 5);
        billing.title = Some("Bill in cents".to_string());
        let mut items = vec![rounding, billing, queued_after("search", "web", &["rounding@web"])];
        assert_eq!(items[1].group(), Group::ReadyToPush, "without its wait it is ready to push");
        mark(&state, &[], &mut items);
        let waits = items[1].waits_on.clone().expect("the waiter shows its wait");
        assert_eq!(waits.after, [("rounding@web".to_string(), "ready to push".to_string())]);
        assert_eq!(waits.names, ["Rounding field"]);
        assert_eq!(items[1].group(), Group::WaitingOn);
        assert_eq!(items[1].summary(), "Waits for Rounding field (ready to push): use the rounding field. kebbie rebases it and tells its Claude once it is pushed and CI is green.");
        assert_eq!(items[0].blocks, ["Bill in cents".to_string(), "search".to_string()], "a running and a queued task both wait on it");
        assert!(items[1].blocks.is_empty() && items[0].waits_on.is_none());
        let json = items[1].to_json();
        assert_eq!((json["group"].as_str(), json["waits_on"][0]["address"].as_str()), (Some("WAITING ON"), Some("rounding@web")));
        let mut asks = vec![named(asking("billing", "web", 1), "billing", "web")];
        mark(&state, &[], &mut asks);
        assert_eq!(asks[0].group(), Group::NeedsYou, "a question on its screen still wants the user");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_waiting_task_is_pushed_only_when_asked_for_anyway() {
        let state = crate::sh::test_dir("waiting-push");
        assert_eq!(refuse_push(&state, "billing@web", "billing"), Ok(()));
        save(&state, &Waiting { address: "billing@web".to_string(), repo: PathBuf::from("/repos/web"), on: vec![dep("rounding@api")], then: None, by: "user".to_string(), at: 100, gone_since: None, unlisted_since: None, unlisted_told: false }).unwrap();
        let refused = refuse_push(&state, "billing@web", "billing").unwrap_err();
        assert!(refused.contains("Not pushed: it waits for rounding@api") && refused.contains("kebbie push billing --anyway pushes it before them"), "{refused}");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_wait_is_refused_for_a_task_its_claude_an_unknown_task_nothing_left_and_a_circle() {
        let state = crate::sh::test_dir("waiting-refused");
        let known_now = known(&["billing@web", "rounding@api", "search@web"], Vec::new(), &[end("old@web", 50, "done", None)]);
        let nothing = |_: &str, _: &str| {};
        assert!(wait_in(&state, &Caller::Task("billing@web".to_string()), &asked("billing@web", &["rounding@api"], None), &known_now, 100, &mut nothing.clone()).unwrap_err().contains("the user's call, or the lead's"));
        assert!(wait_in(&state, &Caller::User, &asked("billing@web", &["nope"], None), &known_now, 100, &mut nothing.clone()).unwrap_err().contains("no task, queued task or task done by that name"));
        assert!(wait_in(&state, &Caller::User, &asked("billing@web", &["old"], None), &known_now, 100, &mut nothing.clone()).unwrap_err().contains("Nothing to wait for"), "a task done already needs no wait");
        assert!(wait_in(&state, &Caller::User, &asked("billing@web", &["billing"], None), &known_now, 100, &mut nothing.clone()).unwrap_err().contains("cannot wait for itself"));
        wait_in(&state, &Caller::User, &asked("billing@web", &["rounding@api"], None), &known_now, 100, &mut nothing.clone()).unwrap();
        let circle = wait_in(&state, &Caller::User, &Asked { address: "rounding@api", repo: Path::new("/repos/api"), on: &["billing@web"], then: None }, &known_now, 100, &mut nothing.clone()).unwrap_err();
        assert_eq!(circle, "rounding@api cannot wait in a circle: rounding@api → billing@web → rounding@api");
        let queued = Wait { task: "search".to_string(), repo: PathBuf::from("/repos/web"), after: vec![dep("billing@web")], prompt: "p".to_string(), mode: None, profile: None, backlog: None, by: By::User, at: 1, blocked: None, gone_since: None, why: None, starts_at: None };
        let through_queue = known(&["billing@web", "rounding@api"], vec![queued], &[]);
        let circle = wait_in(&state, &Caller::User, &Asked { address: "rounding@api", repo: Path::new("/repos/api"), on: &["search@web"], then: None }, &through_queue, 100, &mut nothing.clone()).unwrap_err();
        assert!(circle.contains("rounding@api → search@web → billing@web → rounding@api"), "a queued task's wait closes the circle too: {circle}");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn once_every_task_it_waits_for_lands_the_waiter_is_rebased_and_told_what_landed_and_what_to_do() {
        let state = crate::sh::test_dir("waiting-landed");
        save(&state, &Waiting { address: "billing@web".to_string(), repo: PathBuf::from("/repos/web"), on: vec![dep("rounding@api"), dep("totals@web")], then: Some("use the rounding field".to_string()), by: "lead".to_string(), at: 100, gone_since: None, unlisted_since: None, unlisted_told: false }).unwrap();
        let open = ["billing@web", "totals@web"];
        let (notes, tasks, lead) = told(&state, &facts(&[end("rounding@api", 200, "done", Some("Add the rounding field"))], &open), 300, Some("kebbie rebased this task onto origin/main."));
        assert!(notes.is_empty() && tasks.is_empty() && lead.is_empty(), "one of two landed: it waits on");
        let landed = [end("rounding@api", 200, "done", Some("Add the rounding field")), end("totals@web", 250, "done", None)];
        let (_, tasks, _) = told(&state, &facts(&landed, &["billing@web"]), 300, None);
        assert!(tasks.is_empty() && load(&state, "billing@web").is_some(), "its Claude mid-turn, the wait stays for the next round");
        let (notes, tasks, lead) = told(&state, &facts(&landed, &["billing@web"]), 300, Some("kebbie rebased this task onto origin/main."));
        assert_eq!(notes, ["waiting: billing@web goes on"]);
        let (to, text) = &tasks[0];
        assert_eq!(to, "billing@web");
        assert!(text.starts_with("This task waited for rounding@api, which is done. Its commits (subjects as that task wrote them):\n> Add the rounding field\nThis task waited for totals@web, which is done;"), "{text}");
        assert!(text.ends_with("\n\nkebbie rebased this task onto origin/main. What to do now: use the rounding field"), "{text}");
        assert_eq!(lead, [("rounding@api and totals@web are done, so kebbie told billing@web to go on: kebbie rebased this task onto origin/main.".to_string(), false)]);
        assert_eq!(load(&state, "billing@web"), None, "the wait is over");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_task_waited_for_that_is_dropped_tells_the_waiter_and_the_lead_and_leaves_the_rest_of_the_wait() {
        let state = crate::sh::test_dir("waiting-lost");
        save(&state, &Waiting { address: "billing@web".to_string(), repo: PathBuf::from("/repos/web"), on: vec![dep("rounding@api"), dep("totals@web")], then: None, by: "user".to_string(), at: 100, gone_since: None, unlisted_since: None, unlisted_told: false }).unwrap();
        let (_, tasks, lead) = told(&state, &facts(&[end("rounding@api", 200, "dropped", None)], &["billing@web", "totals@web"]), 300, Some("unused"));
        assert!(tasks[0].1.starts_with("This task waited for rounding@api, which was dropped. That will not land as this task needs, so this task waits only for totals@web now."), "{}", tasks[0].1);
        assert_eq!(lead, [("billing@web waited for rounding@api, which was dropped; kebbie told it, and it waits only for totals@web now.".to_string(), true)]);
        assert_eq!(load(&state, "billing@web").map(|wait| wait.on), Some(vec![dep("totals@web")]));
        let (_, tasks, _) = told(&state, &facts(&[], &["billing@web"]), 300, Some("unused"));
        assert!(tasks.is_empty() && load(&state, "billing@web").and_then(|wait| wait.gone_since) == Some(300), "a task gone for a moment is given its grace");
        let (_, tasks, _) = told(&state, &facts(&[], &["billing@web"]), 300 + after::GRACE, Some("unused"));
        assert!(tasks[0].1.contains("totals@web, which is gone without being done") && tasks[0].1.contains("so this task waits for nothing now"), "{}", tasks[0].1);
        assert_eq!(load(&state, "billing@web"), None);
        save(&state, &Waiting { address: "search@web".to_string(), repo: PathBuf::from("/repos/web"), on: vec![dep("totals@web")], then: None, by: "user".to_string(), at: 100, gone_since: None, unlisted_since: None, unlisted_told: false }).unwrap();
        let (notes, tasks, _) = told(&state, &facts(&[], &["totals@web"]), 300, Some("unused"));
        assert!(notes.is_empty() && tasks.is_empty() && load(&state, "search@web").is_none(), "a waiter that ended takes its wait along");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_wait_whose_repo_cannot_be_listed_stays_and_the_lead_hears_once_it_outlasts_the_grace() {
        let state = crate::sh::test_dir("waiting-unlisted");
        let repo = state.join("web");
        fs::create_dir_all(&repo).unwrap();
        let unlisted = Facts::with_repos(&state, &[], std::slice::from_ref(&repo));
        assert!(unlisted.unlisted.contains(&repo) && unlisted.open.is_empty(), "a folder git cannot list is unlisted, not empty");
        save(&state, &Waiting { address: "billing@web".to_string(), repo: repo.clone(), on: vec![dep("rounding@api")], then: None, by: "user".to_string(), at: 100, gone_since: None, unlisted_since: None, unlisted_told: false }).unwrap();
        let (notes, tasks, lead) = told(&state, &unlisted, 300, Some("unused"));
        assert!(notes.is_empty() && tasks.is_empty() && lead.is_empty(), "nothing is said within the grace");
        assert_eq!(load(&state, "billing@web").and_then(|wait| wait.unlisted_since), Some(300), "the wait stays, its grace started");
        let (_, _, lead) = told(&state, &unlisted, 300 + after::GRACE - 1, Some("unused"));
        assert!(lead.is_empty());
        let (notes, tasks, lead) = told(&state, &unlisted, 300 + after::GRACE, Some("unused"));
        assert!(tasks.is_empty() && notes == ["waiting: billing@web: its repo cannot be listed"], "{notes:?}");
        assert!(lead.len() == 1 && lead[0].1 && lead[0].0.contains("cannot tell whether billing@web is still open. It keeps its wait for rounding@api until it can"), "{lead:?}");
        let (_, _, lead) = told(&state, &unlisted, 300 + after::GRACE + 15, Some("unused"));
        assert!(lead.is_empty() && load(&state, "billing@web").is_some(), "the lead hears it once, and the wait still stays");
        told(&state, &facts(&[], &["billing@web", "rounding@api"]), 400, Some("unused"));
        assert_eq!(load(&state, "billing@web").map(|wait| (wait.unlisted_since, wait.unlisted_told)), Some((None, false)), "listed again, the grace is over");
        told(&state, &facts(&[], &["rounding@api"]), 400, Some("unused"));
        assert_eq!(load(&state, "billing@web"), None, "a listed repo without the waiter still ends its wait");
        fs::remove_dir_all(state).unwrap();
    }
}
