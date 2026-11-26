use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::messages::{self, Message};
use crate::notice::{Notice, Tone};
use crate::peers::task_part;
use crate::permit::Caller;
use crate::sh::Result;
use super::after::{self, Dep, DepState, Facts, Known, Lock, and_list};
use super::slug;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Asked {
    pub task: String,
    pub question: String,
    pub seen: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Promise {
    pub id: u64,
    pub text: String,
    pub after: Vec<Dep>,
    pub at: Option<u64>,
    pub answered: Option<Asked>,
    pub by: String,
    pub made: u64,
    pub due: Option<u64>,
    pub broken: Option<String>,
    pub overdue_told: bool,
    pub gone_since: Option<u64>,
}

impl Promise {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id, "text": self.text,
            "after": self.after.iter().map(|dep| json!({ "address": dep.address, "repo": dep.repo })).collect::<Vec<_>>(),
            "at": self.at, "answered": self.answered.as_ref().map(|asked| json!({ "task": asked.task, "question": asked.question, "seen": asked.seen })),
            "by": self.by, "made": self.made, "due": self.due, "broken": self.broken, "overdue_told": self.overdue_told, "gone_since": self.gone_since,
        })
    }

    fn from_json(value: &Value) -> Option<Promise> {
        let after = value["after"].as_array()?.iter().map(|dep| Some(Dep { address: dep["address"].as_str()?.to_string(), repo: PathBuf::from(dep["repo"].as_str()?) })).collect::<Option<Vec<_>>>()?;
        let answered = match &value["answered"] {
            Value::Null => None,
            asked => Some(Asked { task: asked["task"].as_str()?.to_string(), question: asked["question"].as_str()?.to_string(), seen: asked["seen"].as_bool().unwrap_or(false) }),
        };
        let at = value["at"].as_u64();
        Some(Promise {
            id: value["id"].as_u64()?,
            text: value["text"].as_str().filter(|text| !text.trim().is_empty())?.to_string(),
            after,
            at,
            answered,
            by: value["by"].as_str().filter(|by| matches!(*by, "user" | "lead"))?.to_string(),
            made: value["made"].as_u64()?,
            due: value["due"].as_u64(),
            broken: value["broken"].as_str().map(str::to_string),
            overdue_told: value["overdue_told"].as_bool().unwrap_or(false),
            gone_since: value["gone_since"].as_u64(),
        })
    }

    fn conditions(&self, facts: Option<&Facts>, now: u64) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.after.is_empty() {
            let names: Vec<String> = self.after.iter().map(|dep| match facts {
                Some(facts) => format!("{} ({})", dep.address, after::label(&facts.state(&dep.address, self.made), None)),
                None => dep.address.clone(),
            }).collect();
            parts.push(format!("once {} {} landed", and_list(&names), if self.after.len() == 1 { "has" } else { "have" }));
        }
        if let Some(at) = self.at {
            parts.push(format!("at {}", crate::when::shown(at, now, &crate::recover::local_offset)));
        }
        if let Some(asked) = &self.answered {
            parts.push(format!("once the user has answered {}'s question", asked.task));
        }
        and_list(&parts)
    }
}

fn dir(state: &Path) -> PathBuf {
    state.join("promises")
}

fn file(state: &Path, id: u64) -> PathBuf {
    dir(state).join(format!("{id}.json"))
}

fn save(state: &Path, promise: &Promise) -> Result<()> {
    crate::state::write(&file(state, promise.id), serde_json::to_string_pretty(&promise.to_json()).unwrap_or_default())
}

pub(crate) fn load(state: &Path, id: u64) -> Option<Promise> {
    Promise::from_json(&serde_json::from_str(&fs::read_to_string(file(state, id)).ok()?).ok()?).filter(|promise| promise.id == id)
}

pub(crate) fn all(state: &Path) -> Vec<Promise> {
    let Ok(entries) = fs::read_dir(dir(state)) else { return Vec::new() };
    let mut found: Vec<Promise> = entries.flatten().filter_map(|entry| load(state, entry.file_name().to_string_lossy().strip_suffix(".json")?.parse().ok()?)).collect();
    found.sort_by_key(|promise| promise.id);
    found
}

// Ids are never given twice, so a reminder for a closed promise can never be read as one about a later promise.
fn next_id(state: &Path) -> u64 {
    let counter = dir(state).join(".next");
    let stored = fs::read_to_string(&counter).ok().and_then(|text| text.trim().parse::<u64>().ok()).unwrap_or(1);
    let id = all(state).iter().map(|promise| promise.id + 1).max().unwrap_or(1).max(stored);
    let _ = crate::state::write(&counter, (id + 1).to_string());
    id
}

pub(super) fn waited_on(state: &Path, address: &str) -> bool {
    all(state).iter().any(|promise| promise.after.iter().any(|dep| dep.address == address))
}

fn forget(state: &Path, promise: &Promise) {
    let _ = fs::remove_file(file(state, promise.id));
    after::release_deps(state, promise.after.iter());
}

fn promiser(caller: &Caller) -> Result<&'static str> {
    match caller {
        Caller::User => Ok("user"),
        Caller::Lead => Ok("lead"),
        other => Err(format!("a promise is the lead's, or the user's; {} cannot make or close one", crate::permit::describe(other))),
    }
}

pub(crate) struct Making<'a> {
    pub text: &'a str,
    pub after: &'a [&'a str],
    pub at: Option<u64>,
    pub answered: Option<&'a str>,
}

// The task's open question to the user, by its id, so a later question of the same task is not taken for its answer.
fn open_question(log: &[Message], asked: &str) -> Result<Asked> {
    let task = slug(asked.split_once('@').map_or(asked, |(task, _)| task))?;
    let repo = asked.split_once('@').map(|(_, repo)| repo);
    let open = crate::questions::open(log, |_| None);
    let found: Vec<&&Message> = open.iter().filter(|question| task_part(&question.from) == task && repo.is_none_or(|repo| question.from.split_once('@').map(|(_, name)| name) == Some(repo))).collect();
    match found[..] {
        [question] => Ok(Asked { task: question.from.clone(), question: question.id.clone(), seen: false }),
        [] => Err(format!("{asked} has no question open to the user, so there is no answer to wait for")),
        _ => Err(format!("{asked} is a task in more than one repo with a question open; name it as task@repo")),
    }
}

pub(crate) fn make_in(state: &Path, caller: &Caller, making: &Making, known: &Known, log: &[Message], now: u64) -> Result<String> {
    let by = promiser(caller)?;
    let text = making.text.trim();
    if text.is_empty() {
        return Err("kebbie promise needs what you will do: kebbie promise \"<what you will do>\" [--after <task[@repo]>] [--at \"<time>\"] [--answered <task>]".to_string());
    }
    let mut deps: Vec<Dep> = Vec::new();
    for asked in making.after {
        for dep in after::resolve("lead", &[asked], known)? {
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
    }
    let answered = making.answered.map(|asked| open_question(log, asked)).transpose()?;
    let _lock = Lock::in_dir(&dir(state), true);
    let due_now = deps.is_empty() && making.at.is_none_or(|at| at <= now) && answered.is_none();
    let promise = Promise { id: next_id(state), text: text.to_string(), after: deps, at: making.at.filter(|_| !due_now), answered, by: by.to_string(), made: now, due: due_now.then_some(now), broken: None, overdue_told: false, gone_since: None };
    save(state, &promise)?;
    let next = match due_now {
        true => format!("It is due now: do it, then kebbie promise done {id}. Until then kebbie lists it to every new conversation of yours and shows it as overdue on the board after two hours.", id = promise.id),
        false => format!("kebbie reminds you {}. Once it is done: kebbie promise done {}.", promise.conditions(None, now), promise.id),
    };
    Notice::new(Tone::Done, format!("Promise {} is kept by kebbie", promise.id)).next(next).detail("promise", text).ok()
}

pub(crate) fn close_in(state: &Path, caller: &Caller, id: &str, why: Option<&str>) -> Result<String> {
    promiser(caller)?;
    let id: u64 = id.trim().parse().map_err(|_| format!("\"{id}\" is not a promise's number; kebbie promises lists them"))?;
    let _lock = Lock::in_dir(&dir(state), true);
    let promise = load(state, id).ok_or_else(|| format!("No open promise {id}; kebbie promises lists them"))?;
    if let Some(why) = why
        && why.trim().is_empty() {
        return Err(format!("kebbie promise drop {id} needs why: kebbie promise drop {id} \"<why>\""));
    }
    forget(state, &promise);
    let headline = if why.is_some() { format!("Dropped promise {id}") } else { format!("Kept promise {id}") };
    Notice::new(Tone::Done, headline).detail("promise", &promise.text).detail("why", why.unwrap_or_default()).ok()
}

pub const EVERY: u64 = 60;
pub(crate) const OVERDUE: u64 = 2 * 3600;

#[derive(Debug, PartialEq)]
pub(crate) enum Next {
    Wait,
    Missing,
    Due,
    Broken(String),
}

// A task dropped, reset, landed with red CI or gone, or a question closed without the user's answer, will never make the promise come true as made, so the lead hears it rather than waiting for good.
pub(crate) fn next(promise: &Promise, facts: &Facts, log: &[Message], now: u64) -> Next {
    let states: Vec<(&Dep, DepState)> = promise.after.iter().map(|dep| (dep, facts.state(&dep.address, promise.made))).collect();
    let gone_long = promise.gone_since.is_some_and(|since| now.saturating_sub(since) >= after::GRACE);
    let mut lost: Vec<String> = states.iter().filter_map(|(dep, state)| match state {
        DepState::Ended(how) => Some(format!("{} was {how}", dep.address)),
        DepState::CiFailed => Some(format!("{} landed with red CI", dep.address)),
        DepState::Gone if gone_long => Some(format!("{} is gone without being done", dep.address)),
        _ => None,
    }).collect();
    let answer = promise.answered.as_ref().map(|asked| match asked.seen { true => Some(true), false => answer_in(asked, log) });
    if let (Some(asked), Some(None)) = (&promise.answered, &answer) {
        lost.push(format!("{}'s question closed without the user's answer", asked.task));
    }
    if !lost.is_empty() {
        return Next::Broken(and_list(&lost));
    }
    if states.iter().any(|(_, state)| *state == DepState::Gone) {
        return Next::Missing;
    }
    let landed = states.iter().all(|(_, state)| *state == DepState::Done);
    let time = promise.at.is_none_or(|at| now >= at);
    if landed && time && answer.flatten().is_none_or(|answered| answered) { Next::Due } else { Next::Wait }
}

// Some(true) once the user answered, Some(false) while it is open, None once it closed otherwise; a rotation of the log keeps open questions but drops answered ones, so the keeper records an answer on the promise the first time it sees it.
fn answer_in(asked: &Asked, log: &[Message]) -> Option<bool> {
    match log.iter().find(|message| message.id == asked.question).and_then(|message| message.question.as_ref()) {
        Some(question) if question.answered => Some(true),
        Some(_) if crate::questions::open(log, |_| None).iter().any(|open| open.id == asked.question) => Some(false),
        _ => None,
    }
}

fn reminder(promise: &Promise) -> String {
    format!("Promise {id} is due: {text}. Do it now, then close it with kebbie promise done {id}; if it no longer makes sense, kebbie promise drop {id} \"<why>\".", id = promise.id, text = promise.text)
}

fn broken_text(promise: &Promise, why: &str) -> String {
    format!("Promise {id} can no longer come true as made: {why}. It was: {text}. Make it again on what holds now with kebbie promise, or drop it with kebbie promise drop {id} \"<why>\".", id = promise.id, text = promise.text)
}

fn overdue_text(promise: &Promise, now: u64) -> String {
    let since = crate::when::shown(promise.due.unwrap_or(now), now, &crate::recover::local_offset);
    format!("Promise {id} has waited for you since {since} and is still open: {text}. Do it and close it with kebbie promise done {id}, or drop it with kebbie promise drop {id} \"<why>\"; the user's board shows it as overdue until then.", id = promise.id, text = promise.text)
}

pub(crate) fn overdue(promise: &Promise, now: u64) -> bool {
    promise.due.is_some_and(|due| now.saturating_sub(due) >= OVERDUE)
}

// What the board shows of the lead's promises: only those overdue, as text and since when, never a task or a question for the user.
pub(crate) fn overdue_now(state: &Path, now: u64) -> Vec<(String, u64)> {
    all(state).into_iter().filter(|promise| overdue(promise, now)).filter_map(|promise| Some((promise.text, promise.due?))).collect()
}

pub(crate) fn overdue_line(overdue: &[(String, u64)], now: u64, width: usize) -> Option<String> {
    let fits = |line: String| (crate::board::widgets::text_width(&line) <= width).then_some(line);
    match overdue {
        [] => None,
        [(text, due)] => fits(format!("The lead's promise is overdue since {}: {text}", crate::when::shown(*due, now, &crate::recover::local_offset))).or_else(|| Some("The lead's promise is overdue · kebbie promises".to_string())),
        many => Some(format!("The lead has {} overdue promises · kebbie promises lists them", many.len())),
    }
}

pub(crate) fn tick_in(state: &Path, facts: &Facts, log: &[Message], now: u64, tell: &mut dyn FnMut(&str)) -> Vec<String> {
    let Some(_lock) = Lock::in_dir(&dir(state), false) else { return Vec::new() };
    let mut notes = Vec::new();
    for promise in all(state) {
        if promise.broken.is_some() {
            continue;
        }
        let promise = match &promise.answered {
            Some(asked) if !asked.seen && answer_in(asked, log) == Some(true) => {
                let seen = Promise { answered: Some(Asked { seen: true, ..asked.clone() }), ..promise.clone() };
                let _ = save(state, &seen);
                seen
            }
            _ => promise,
        };
        if promise.due.is_some() {
            if overdue(&promise, now) && !promise.overdue_told {
                tell(&overdue_text(&promise, now));
                notes.push(format!("promise {}: overdue", promise.id));
                let _ = save(state, &Promise { overdue_told: true, ..promise });
            }
            continue;
        }
        match next(&promise, facts, log, now) {
            Next::Wait if promise.gone_since.is_some() => {
                let _ = save(state, &Promise { gone_since: None, ..promise });
            }
            Next::Wait => {}
            Next::Missing if promise.gone_since.is_none() => {
                let _ = save(state, &Promise { gone_since: Some(now), ..promise });
            }
            Next::Missing => {}
            Next::Broken(why) => {
                tell(&broken_text(&promise, &why));
                notes.push(format!("promise {}: cannot hold", promise.id));
                after::release_deps(state, promise.after.iter());
                let _ = save(state, &Promise { broken: Some(why), ..promise });
            }
            Next::Due => {
                tell(&reminder(&promise));
                notes.push(format!("promise {}: due", promise.id));
                let _ = save(state, &Promise { due: Some(now), ..promise });
            }
        }
    }
    notes
}

pub fn tick(state: &Path) -> Vec<String> {
    tick_from(state, messages::now(), &mut |text| {
        if let Err(error) = messages::to_lead(messages::ToLead::Promise, text) {
            eprintln!("Could not remind the lead: {error}");
        }
    })
}

fn tick_from(state: &Path, now: u64, tell: &mut dyn FnMut(&str)) -> Vec<String> {
    let promises = all(state);
    if promises.is_empty() {
        return Vec::new();
    }
    let log = messages::load_with_rotated_in(state);
    let facts = Facts::with_repos(state, &log, &repos_of(&promises));
    tick_in(state, &facts, &log, now, tell)
}

fn repos_of(promises: &[Promise]) -> Vec<PathBuf> {
    let mut repos: Vec<PathBuf> = promises.iter().flat_map(|promise| promise.after.iter().map(|dep| dep.repo.clone())).collect();
    repos.sort();
    repos.dedup();
    repos
}

pub(crate) fn shown_line(promise: &Promise, facts: Option<&Facts>, now: u64) -> String {
    let offset = &crate::recover::local_offset;
    let state = match (&promise.broken, promise.due) {
        (Some(broken), _) => format!("cannot hold: {broken}"),
        (None, Some(due)) => format!("due since {}", crate::when::shown(due, now, offset)),
        (None, None) => format!("waits: {}", promise.conditions(facts, now)),
    };
    format!("{}. {} — {state}", promise.id, promise.text)
}

pub(crate) fn listing_in(state: &Path, log: &[Message], now: u64) -> String {
    let promises = all(state);
    if promises.is_empty() {
        return "No open promises.".to_string();
    }
    let facts = Facts::with_repos(state, log, &repos_of(&promises));
    promises.iter().map(|promise| shown_line(promise, Some(&facts), now)).collect::<Vec<_>>().join("\n")
}

// The lead's command line carries its first prompt, so a new conversation hears how many promises wait and where to read them, and the text only of those already due, which no reminder repeats until they are overdue.
pub(crate) const FIRST_LINE_KEEPS: &str = " that kebbie keeps for you: kebbie promises lists them, and kebbie reminds you when each is due.";

pub(crate) fn first_prompt_line(state: &Path) -> Option<String> {
    let promises = all(state);
    let count = promises.len();
    let due: Vec<String> = promises.iter().filter(|promise| promise.due.is_some() && promise.broken.is_none()).map(|promise| format!("{}. {}", promise.id, promise.text)).collect();
    let due = match due.is_empty() {
        true => String::new(),
        false => format!(" Due now and not yet done: {}.", due.join("; ")),
    };
    (count > 0).then(|| format!("You have {count} open promise{}{FIRST_LINE_KEEPS}{due}", if count == 1 { "" } else { "s" }))
}

pub fn list() -> Result<String> {
    Ok(listing_in(&crate::lead::state_dir()?, &messages::load(), messages::now()))
}

pub fn command(words: &[String], after_names: &[&str], at: Option<&str>, answered: Option<&str>) -> Result<String> {
    let state = crate::lead::state_dir()?;
    let caller = crate::permit::caller();
    let first = words.first().map(String::as_str);
    let conditions = !after_names.is_empty() || at.is_some() || answered.is_some();
    match (first, words.get(1)) {
        (Some("done"), Some(id)) if !conditions && words.len() == 2 => close_in(&state, &caller, id, None),
        (Some("drop"), Some(id)) if !conditions => close_in(&state, &caller, id, Some(&words[2..].join(" "))),
        _ => {
            let now = messages::now();
            let at = at.map(|text| crate::when::parse(text, now, &crate::recover::local_offset)).transpose().map_err(|error| format!("--at: {error}"))?;
            let known = after::known(&state, &[]);
            make_in(&state, &caller, &Making { text: &words.join(" "), after: after_names, at, answered }, &known, &messages::load(), now)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;
    use super::after::latest_ends;
    use crate::messages::Question;

    fn dep(address: &str) -> Dep {
        Dep { address: address.to_string(), repo: PathBuf::from(format!("/repos/{}", address.split_once('@').unwrap().1)) }
    }

    fn known(open: &[&str], ends: &[Message]) -> Known {
        Known { open: open.iter().map(|address| dep(address)).collect(), waits: Vec::new(), ends: latest_ends(ends), running: Vec::new() }
    }

    pub(super) fn end(address: &str, at: u64, how: &str) -> Message {
        Message { id: format!("end-{address}-{at}"), at, from: "kebbie".to_string(), to: address.to_string(), text: format!("{} ended ({how})", task_part(address)), held: false, report: None, ended: true, question: None, about: None, kind: None, landed: None, why: None, forced: false, fyi: false }
    }

    pub(super) fn question(id: &str, from: &str, at: u64, answered: bool) -> Message {
        let asked = Question { options: vec![("Yes".to_string(), "do it".to_string()), ("No".to_string(), "leave it".to_string())], recommend: 1, plan: None, plan_at: None, comment: Vec::new(), answered, waits: false, relayed: None };
        Message { id: id.to_string(), at, from: from.to_string(), to: "you".to_string(), text: "Should it?".to_string(), held: false, report: None, ended: false, question: Some(asked), about: None, kind: None, landed: None, why: None, forced: false, fyi: false }
    }

    pub(super) fn facts(ends: &[Message], open: &[&str]) -> Facts {
        Facts { ends: latest_ends(ends), open: open.iter().map(|address| address.to_string()).collect(), queued: HashSet::new(), ci: HashMap::new(), unlisted: HashSet::new(), state: None }
    }

    fn making<'a>(text: &'a str, after: &'a [&'a str], at: Option<u64>, answered: Option<&'a str>) -> Making<'a> {
        Making { text, after, at, answered }
    }

    #[test]
    fn the_lead_makes_a_promise_on_a_task_landing_and_closes_it_when_kept() {
        let state = crate::sh::test_dir("promise-make");
        let known = known(&["lint-before-push@kebbie"], &[]);
        let made = make_in(&state, &Caller::Lead, &making("set tests.lint_command", &["lint-before-push"], None, None), &known, &[], 100).unwrap();
        assert!(made.contains("Promise 1 is kept by kebbie") && made.contains("once lint-before-push@kebbie has landed") && made.contains("kebbie promise done 1"), "{made}");
        let kept = load(&state, 1).unwrap();
        assert_eq!((kept.after, kept.by.as_str(), kept.made), (vec![dep("lint-before-push@kebbie")], "lead", 100), "read back as the keeper reads it");
        assert!(waited_on(&state, "lint-before-push@kebbie"), "its CI watch is kept for the promise");
        let second = make_in(&state, &Caller::User, &making("update kebbie", &[], Some(5_000), None), &known, &[], 100).unwrap();
        assert!(second.contains("Promise 2"), "{second}");
        let done = close_in(&state, &Caller::Lead, "1", None).unwrap();
        assert!(done.contains("Kept promise 1"), "{done}");
        assert!(!waited_on(&state, "lint-before-push@kebbie"));
        let third = make_in(&state, &Caller::Lead, &making("again", &[], Some(5_000), None), &known, &[], 100).unwrap();
        assert!(third.contains("Promise 3"), "a closed promise's number is never given again: {third}");
        assert!(close_in(&state, &Caller::Lead, "2", Some(" ")).unwrap_err().contains("needs why"));
        assert!(close_in(&state, &Caller::Lead, "2", Some("the fix went out another way")).unwrap().contains("Dropped promise 2"));
        assert!(close_in(&state, &Caller::Lead, "2", None).unwrap_err().contains("No open promise 2"));
        assert!(close_in(&state, &Caller::Lead, "x", None).unwrap_err().contains("is not a promise's number"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_promise_needs_what_you_will_do_and_conditions_that_can_still_come_true() {
        let state = crate::sh::test_dir("promise-refused");
        let landed = known(&[], &[end("bug-triage@kebbie", 50, "done")]);
        assert!(make_in(&state, &Caller::Lead, &making("", &[], Some(5_000), None), &landed, &[], 100).unwrap_err().contains("needs what you will do"));
        assert!(make_in(&state, &Caller::Lead, &making("start the review", &["fable-review"], None, None), &landed, &[], 100).unwrap_err().contains("no task, queued task or task done by that name"), "a backlog note is not a task");
        assert!(make_in(&state, &Caller::Lead, &making("ask again", &[], None, Some("billing")), &landed, &[], 100).unwrap_err().contains("billing has no question open"));
        assert!(all(&state).is_empty(), "a refusal writes nothing");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_promise_due_at_once_is_kept_until_closed_and_named_to_a_new_conversation() {
        let state = crate::sh::test_dir("promise-now");
        let landed = known(&[], &[end("bug-triage@kebbie", 50, "done")]);
        let made = make_in(&state, &Caller::Lead, &making("run the fable review", &[], None, None), &landed, &[], 100).unwrap();
        assert!(made.contains("Promise 1 is kept by kebbie") && made.contains("It is due now") && made.contains("kebbie promise done 1"), "{made}");
        let on_landed = make_in(&state, &Caller::Lead, &making("start the review", &["bug-triage"], None, None), &landed, &[], 100).unwrap();
        assert!(on_landed.contains("Promise 2 is kept by kebbie") && on_landed.contains("It is due now"), "a task landed already makes the promise due, not refused: {on_landed}");
        let past = make_in(&state, &Caller::Lead, &making("tidy the notes", &[], Some(90), None), &landed, &[], 100).unwrap();
        assert!(past.contains("It is due now"), "{past}");
        assert_eq!(all(&state).iter().map(|promise| (promise.id, promise.due, promise.after.is_empty(), promise.at)).collect::<Vec<_>>(), vec![(1, Some(100), true, None), (2, Some(100), true, None), (3, Some(100), true, None)], "read back from disk, none dropped for want of a condition");
        let first = first_prompt_line(&state).unwrap();
        assert!(first.starts_with("You have 3 open promises") && first.contains(FIRST_LINE_KEEPS) && first.ends_with(" Due now and not yet done: 1. run the fable review; 2. start the review; 3. tidy the notes."), "{first}");
        assert!(listing_in(&state, &[], 200).contains("1. run the fable review — due since "));
        let none = facts(&[], &[]);
        assert!(reminded(&state, &none, &[], 100 + OVERDUE - 1).is_empty(), "the lead heard it when it made it");
        assert_eq!(reminded(&state, &none, &[], 100 + OVERDUE).len(), 3, "left undone, it turns overdue like any due promise");
        assert_eq!(overdue_now(&state, 100 + OVERDUE).len(), 3);
        close_in(&state, &Caller::Lead, "1", None).unwrap();
        assert!(!first_prompt_line(&state).unwrap().contains("run the fable review"), "done closes it");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_promise_on_an_answer_holds_the_question_open_now() {
        let state = crate::sh::test_dir("promise-answered");
        let log = [question("q1", "billing@web", 90, false)];
        let made = make_in(&state, &Caller::Lead, &making("tell billing which rounding", &[], None, Some("billing")), &known(&["billing@web"], &[]), &log, 100).unwrap();
        assert!(made.contains("once the user has answered billing@web's question"), "{made}");
        assert_eq!(load(&state, 1).unwrap().answered, Some(Asked { task: "billing@web".to_string(), question: "q1".to_string(), seen: false }));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn only_the_lead_and_the_user_make_or_close_a_promise() {
        let state = crate::sh::test_dir("promise-callers");
        let known = known(&[], &[]);
        for caller in [Caller::Task("billing@web".to_string()), Caller::Agent, Caller::Sandboxed, Caller::LeadSandboxed] {
            assert!(make_in(&state, &caller, &making("x", &[], Some(5_000), None), &known, &[], 100).unwrap_err().contains("a promise is the lead's, or the user's"));
        }
        make_in(&state, &Caller::Lead, &making("x", &[], Some(5_000), None), &known, &[], 100).unwrap();
        assert!(close_in(&state, &Caller::Task("billing@web".to_string()), "1", None).is_err());
        assert!(load(&state, 1).is_some());
        fs::remove_dir_all(state).unwrap();
    }

    fn promise(id: u64, after: &[&str], at: Option<u64>, answered: Option<(&str, &str)>) -> Promise {
        Promise { id, text: format!("promise {id}"), after: after.iter().map(|address| dep(address)).collect(), at, answered: answered.map(|(task, question)| Asked { task: task.to_string(), question: question.to_string(), seen: false }), by: "lead".to_string(), made: 100, due: None, broken: None, overdue_told: false, gone_since: None }
    }

    fn reminded(state: &Path, facts: &Facts, log: &[Message], now: u64) -> Vec<String> {
        let mut heard = Vec::new();
        tick_in(state, facts, log, now, &mut |text| heard.push(text.to_string()));
        heard
    }

    #[test]
    fn a_promise_is_due_once_every_condition_holds_and_the_lead_hears_it_once() {
        let state = crate::sh::test_dir("promise-due");
        save(&state, &promise(1, &["lint@kebbie"], Some(1_000), None)).unwrap();
        let working = facts(&[], &["lint@kebbie"]);
        assert!(reminded(&state, &working, &[], 2_000).is_empty(), "the task has not landed");
        let landed = facts(&[end("lint@kebbie", 500, "done")], &[]);
        assert!(reminded(&state, &landed, &[], 900).is_empty(), "landed, but its time has not come");
        let heard = reminded(&state, &landed, &[], 1_000);
        assert_eq!(heard, vec!["Promise 1 is due: promise 1. Do it now, then close it with kebbie promise done 1; if it no longer makes sense, kebbie promise drop 1 \"<why>\".".to_string()]);
        assert_eq!(load(&state, 1).unwrap().due, Some(1_000));
        assert!(reminded(&state, &landed, &[], 1_060).is_empty(), "reminded once");
        let landed_before = facts(&[end("lint@kebbie", 50, "done")], &[]);
        save(&state, &promise(2, &["lint@kebbie"], None, None)).unwrap();
        assert!(reminded(&state, &landed_before, &[], 1_100).is_empty(), "an end before the promise was made is an earlier task by that name");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_promise_that_can_no_longer_come_true_is_told_to_the_lead_at_once() {
        let state = crate::sh::test_dir("promise-broken");
        save(&state, &promise(1, &["lint@kebbie"], None, None)).unwrap();
        let heard = reminded(&state, &facts(&[end("lint@kebbie", 200, "dropped")], &[]), &[], 300);
        assert_eq!(heard.len(), 1);
        assert!(heard[0].starts_with("Promise 1 can no longer come true as made: lint@kebbie was dropped. It was: promise 1."), "{}", heard[0]);
        let kept = load(&state, 1).unwrap();
        assert_eq!((kept.broken.as_deref(), kept.due), (Some("lint@kebbie was dropped"), None), "a broken promise is never due, so it is never overdue");
        assert!(reminded(&state, &facts(&[], &[]), &[], 300 + 2 * OVERDUE).iter().all(|heard| !heard.contains("Promise 1")), "the lead hears of a broken promise once, never as overdue");
        assert!(!overdue(&load(&state, 1).unwrap(), 300 + 2 * OVERDUE), "a broken promise is never overdue");
        save(&state, &promise(2, &["red@kebbie"], None, None)).unwrap();
        let mut red = facts(&[end("red@kebbie", 200, "done")], &[]);
        red.ci.insert("red@kebbie".to_string(), false);
        assert!(reminded(&state, &red, &[], 300)[0].contains("red@kebbie landed with red CI"));
        save(&state, &promise(3, &["gone@kebbie"], None, None)).unwrap();
        let gone = facts(&[], &[]);
        assert!(reminded(&state, &gone, &[], 300).is_empty(), "a task missing for a moment gets the grace a wait gets");
        assert!(reminded(&state, &gone, &[], 300 + after::GRACE)[0].contains("gone@kebbie is gone without being done"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_promise_on_an_answer_is_due_once_the_user_answers_that_question() {
        let state = crate::sh::test_dir("promise-answer-due");
        save(&state, &promise(1, &[], None, Some(("billing@web", "q1")))).unwrap();
        let none = facts(&[], &["billing@web"]);
        assert!(reminded(&state, &none, &[question("q1", "billing@web", 90, false)], 200).is_empty(), "still open");
        assert!(reminded(&state, &none, &[question("q1", "billing@web", 90, true)], 200)[0].starts_with("Promise 1 is due"));
        save(&state, &promise(2, &[], None, Some(("billing@web", "q2")))).unwrap();
        let replaced = [question("q2", "billing@web", 90, false), Message { ended: true, to: "billing@web".to_string(), ..end("billing@web", 150, "dropped") }];
        assert!(reminded(&state, &none, &replaced, 200)[0].contains("billing@web's question closed without the user's answer"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn an_answer_the_keeper_saw_holds_after_a_rotation_drops_the_question_from_the_log() {
        let state = crate::sh::test_dir("promise-answer-kept");
        save(&state, &promise(1, &[], Some(1_000), Some(("billing@web", "q1")))).unwrap();
        let none = facts(&[], &["billing@web"]);
        assert!(reminded(&state, &none, &[question("q1", "billing@web", 90, true)], 200).is_empty(), "answered, but its time has not come");
        assert!(load(&state, 1).unwrap().answered.unwrap().seen, "the answer is kept on the promise");
        assert_eq!(reminded(&state, &none, &[], 1_000).len(), 1);
        assert!(load(&state, 1).unwrap().broken.is_none() && load(&state, 1).unwrap().due == Some(1_000), "due, not broken, with the question gone from the log");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_keeper_reads_a_question_answered_before_the_log_rotated() {
        let state = crate::sh::test_dir("promise-rotated");
        let asked = crate::questions::parse(&["Yes: do it", "No: leave it"], Some("1"), false, "Should it?").unwrap();
        messages::append_in(&state, &crate::questions::record("q1", 90, "billing@web", &asked, None, None), 1 << 20).unwrap();
        messages::append_in(&state, &json!({ "id": "q1", "at": 95, "state": "answered" }), 1 << 20).unwrap();
        messages::append_in(&state, &json!({ "id": "m1", "at": 96, "from": "kebbie", "to": "lead", "text": "later", "state": "delivered" }), 0).unwrap();
        assert!(messages::load_in(&state).iter().all(|message| message.id != "q1"), "the rotation dropped the answered question from the current log");
        save(&state, &promise(1, &[], None, Some(("billing@web", "q1")))).unwrap();
        let mut heard = Vec::new();
        tick_from(&state, 200, &mut |text| heard.push(text.to_string()));
        assert!(heard.len() == 1 && heard[0].starts_with("Promise 1 is due"), "the keeper's round reads the rotated log too: {heard:?}");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_due_promise_left_open_two_hours_is_overdue_and_the_lead_hears_once_more() {
        let state = crate::sh::test_dir("promise-overdue");
        save(&state, &Promise { due: Some(1_000), ..promise(1, &[], Some(900), None) }).unwrap();
        let none = facts(&[], &[]);
        assert!(!overdue(&load(&state, 1).unwrap(), 1_000 + OVERDUE - 1));
        assert!(reminded(&state, &none, &[], 1_000 + OVERDUE - 1).is_empty());
        let heard = reminded(&state, &none, &[], 1_000 + OVERDUE);
        assert!(heard.len() == 1 && heard[0].starts_with("Promise 1 has waited for you since") && heard[0].contains("the user's board shows it as overdue until then"), "{heard:?}");
        assert!(overdue(&load(&state, 1).unwrap(), 1_000 + OVERDUE));
        assert!(reminded(&state, &none, &[], 1_000 + 2 * OVERDUE).is_empty(), "told once");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_new_conversation_of_the_lead_hears_of_its_open_promises() {
        let state = crate::sh::test_dir("promise-first");
        assert_eq!(first_prompt_line(&state), None);
        save(&state, &promise(1, &[], Some(900), None)).unwrap();
        assert_eq!(first_prompt_line(&state).unwrap(), "You have 1 open promise that kebbie keeps for you: kebbie promises lists them, and kebbie reminds you when each is due.");
        save(&state, &promise(2, &[], Some(900), None)).unwrap();
        assert!(first_prompt_line(&state).unwrap().starts_with("You have 2 open promises"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_board_shows_a_line_only_for_overdue_promises() {
        let state = crate::sh::test_dir("promise-board");
        save(&state, &promise(1, &[], Some(900), None)).unwrap();
        save(&state, &Promise { due: Some(1_000), ..promise(2, &[], Some(900), None) }).unwrap();
        save(&state, &Promise { broken: Some("lint@kebbie was dropped".to_string()), ..promise(3, &[], Some(900), None) }).unwrap();
        assert!(overdue_now(&state, 1_000 + OVERDUE - 1).is_empty(), "waiting and newly due promises stay off the board");
        let shown = overdue_now(&state, 1_000 + OVERDUE);
        assert_eq!(shown, vec![("promise 2".to_string(), 1_000)]);
        let line = overdue_line(&shown, 1_000 + OVERDUE, 200).unwrap();
        assert!(line.starts_with("The lead's promise is overdue since ") && line.ends_with(": promise 2"), "{line}");
        assert_eq!(overdue_line(&shown, 1_000 + OVERDUE, 20).unwrap(), "The lead's promise is overdue · kebbie promises");
        assert_eq!(overdue_line(&[("a".to_string(), 1), ("b".to_string(), 2)], 1_000, 200).unwrap(), "The lead has 2 overdue promises · kebbie promises lists them");
        assert_eq!(overdue_line(&[], 1_000, 200), None);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_listing_says_what_each_promise_waits_for() {
        let state = crate::sh::test_dir("promise-list");
        assert_eq!(listing_in(&state, &[], 100), "No open promises.");
        let promise = Promise { id: 4, text: "set tests.lint_command".to_string(), after: vec![dep("lint@kebbie")], at: None, answered: None, by: "lead".to_string(), made: 100, due: None, broken: None, overdue_told: false, gone_since: None };
        assert_eq!(shown_line(&promise, Some(&facts(&[], &["lint@kebbie"])), 100), "4. set tests.lint_command — waits: once lint@kebbie (open) has landed");
        let broken = Promise { broken: Some("lint@kebbie was dropped".to_string()), ..promise.clone() };
        assert_eq!(shown_line(&broken, None, 100), "4. set tests.lint_command — cannot hold: lint@kebbie was dropped");
        fs::remove_dir_all(state).unwrap();
    }
}
