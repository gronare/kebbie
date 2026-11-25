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
            "at": self.at, "answered": self.answered.as_ref().map(|asked| json!({ "task": asked.task, "question": asked.question })),
            "by": self.by, "made": self.made, "due": self.due, "broken": self.broken, "overdue_told": self.overdue_told, "gone_since": self.gone_since,
        })
    }

    fn from_json(value: &Value) -> Option<Promise> {
        let after = value["after"].as_array()?.iter().map(|dep| Some(Dep { address: dep["address"].as_str()?.to_string(), repo: PathBuf::from(dep["repo"].as_str()?) })).collect::<Option<Vec<_>>>()?;
        let answered = match &value["answered"] {
            Value::Null => None,
            asked => Some(Asked { task: asked["task"].as_str()?.to_string(), question: asked["question"].as_str()?.to_string() }),
        };
        let at = value["at"].as_u64();
        Some(Promise {
            id: value["id"].as_u64()?,
            text: value["text"].as_str().filter(|text| !text.trim().is_empty())?.to_string(),
            after: Some(after).filter(|after| !after.is_empty() || at.is_some() || answered.is_some())?,
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
        [question] => Ok(Asked { task: question.from.clone(), question: question.id.clone() }),
        [] => Err(format!("{asked} has no question open to the user, so there is no answer to wait for")),
        _ => Err(format!("{asked} is a task in more than one repo with a question open; name it as task@repo")),
    }
}

pub(crate) fn make_in(state: &Path, caller: &Caller, making: &Making, known: &Known, log: &[Message], now: u64) -> Result<String> {
    let by = promiser(caller)?;
    let text = making.text.trim();
    if text.is_empty() {
        return Err("kebbie promise needs what you will do: kebbie promise \"<what you will do>\" --after <task[@repo]> | --at \"<time>\" | --answered <task>".to_string());
    }
    if making.after.is_empty() && making.at.is_none() && making.answered.is_none() {
        return Err("kebbie promise needs a condition: --after <task[@repo]> (it has landed), --at \"<time>\" or --answered <task> (the user answered its question)".to_string());
    }
    let mut deps: Vec<Dep> = Vec::new();
    for asked in making.after {
        let found = after::resolve("lead", &[asked], known)?;
        if found.is_empty() {
            return Notice::new(Tone::NotNow, format!("{asked} has landed already")).next(format!("Do it now: {text}")).err();
        }
        for dep in found {
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
    }
    let answered = making.answered.map(|asked| open_question(log, asked)).transpose()?;
    let _lock = Lock::in_dir(&dir(state), true);
    let promise = Promise { id: next_id(state), text: text.to_string(), after: deps, at: making.at, answered, by: by.to_string(), made: now, due: None, broken: None, overdue_told: false, gone_since: None };
    save(state, &promise)?;
    Notice::new(Tone::Done, format!("Promise {} is kept by kebbie", promise.id))
        .next(format!("kebbie reminds you {}. Once it is done: kebbie promise done {}.", promise.conditions(None, now), promise.id))
        .detail("promise", text).ok()
}

pub(crate) fn close_in(state: &Path, caller: &Caller, id: &str, why: Option<&str>) -> Result<String> {
    promiser(caller)?;
    let id: u64 = id.trim().parse().map_err(|_| format!("\"{id}\" is not a promise's number; kebbie promises lists them"))?;
    let _lock = Lock::in_dir(&dir(state), true);
    let promise = load(state, id).ok_or_else(|| format!("No open promise {id}; kebbie promises lists them"))?;
    if let Some(why) = why {
        if why.trim().is_empty() {
            return Err(format!("kebbie promise drop {id} needs why: kebbie promise drop {id} \"<why>\""));
        }
    }
    forget(state, &promise);
    let headline = if why.is_some() { format!("Dropped promise {id}") } else { format!("Kept promise {id}") };
    Notice::new(Tone::Done, headline).detail("promise", &promise.text).detail("why", why.unwrap_or_default()).ok()
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
        Known { open: open.iter().map(|address| dep(address)).collect(), waits: Vec::new(), ends: latest_ends(ends) }
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
    fn a_promise_needs_a_condition_that_can_still_come_true() {
        let state = crate::sh::test_dir("promise-refused");
        let landed = known(&[], &[end("bug-triage@kebbie", 50, "done")]);
        assert!(make_in(&state, &Caller::Lead, &making("", &[], Some(5_000), None), &landed, &[], 100).unwrap_err().contains("needs what you will do"));
        assert!(make_in(&state, &Caller::Lead, &making("start the review", &[], None, None), &landed, &[], 100).unwrap_err().contains("needs a condition"));
        let done = make_in(&state, &Caller::Lead, &making("start the review", &["bug-triage"], None, None), &landed, &[], 100).unwrap_err();
        assert!(done.contains("bug-triage has landed already") && done.contains("Do it now: start the review"), "{done}");
        assert!(make_in(&state, &Caller::Lead, &making("start the review", &["fable-review"], None, None), &landed, &[], 100).unwrap_err().contains("no task, queued task or task done by that name"), "a backlog note is not a task");
        assert!(make_in(&state, &Caller::Lead, &making("ask again", &[], None, Some("billing")), &landed, &[], 100).unwrap_err().contains("billing has no question open"));
        assert!(all(&state).is_empty(), "a refusal writes nothing");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_promise_on_an_answer_holds_the_question_open_now() {
        let state = crate::sh::test_dir("promise-answered");
        let log = [question("q1", "billing@web", 90, false)];
        let made = make_in(&state, &Caller::Lead, &making("tell billing which rounding", &[], None, Some("billing")), &known(&["billing@web"], &[]), &log, 100).unwrap();
        assert!(made.contains("once the user has answered billing@web's question"), "{made}");
        assert_eq!(load(&state, 1).unwrap().answered, Some(Asked { task: "billing@web".to_string(), question: "q1".to_string() }));
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
