use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{NewCase, Source};
use crate::issues::{self, Issue};
use crate::sh::Result;

const KEPT: usize = 500;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Seen {
    kind: String,
    summary: String,
    count: u64,
    first: u64,
    last: u64,
    #[serde(default)]
    found_nothing: u32,
}

fn file(state: &Path) -> PathBuf {
    super::dir(state).join("incidents.json")
}

fn load(state: &Path) -> BTreeMap<String, Seen> {
    fs::read_to_string(file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub fn normal(text: &str) -> String {
    let words: Vec<String> = text.split_whitespace().map(|word| {
        let word = word.trim_matches(|character: char| "\"'`()[]{},;".contains(character)).to_lowercase();
        if word.contains('/') {
            return "<path>".to_string();
        }
        let hex = word.len() >= 7 && word.chars().all(|character| character.is_ascii_hexdigit());
        if hex || word.chars().any(|character| character.is_ascii_digit()) && word.chars().filter(|character| character.is_ascii_digit()).count() * 2 >= word.chars().count() {
            return "#".to_string();
        }
        word
    }).filter(|word| !word.is_empty()).collect();
    super::clipped(&words.join(" "), 160)
}

// Each time the watcher found no fault in an incident it must be seen twice as often again, so a harmless one stops costing a run.
fn needed(found_nothing: u32) -> u64 {
    2 << found_nothing.min(12)
}

pub fn found_nothing(state: &Path, fingerprint: &str) -> Result<()> {
    let _lock = super::locked(state)?;
    let mut all = load(state);
    let kind = fingerprint.split_once(':').map_or(fingerprint, |(kind, _)| kind).to_string();
    all.entry(fingerprint.to_string()).or_insert_with(|| Seen { kind, ..Seen::default() }).found_nothing += 1;
    crate::state::write(&file(state), serde_json::to_string_pretty(&all).map_err(|error| error.to_string())?)
}

pub fn issue_with(state: &Path, fingerprint: &str) -> Option<Issue> {
    issues::load(&issues::dir(state)).into_iter().rev().find(|issue| issue.source.is_none() && issue.fingerprints.iter().any(|print| print == fingerprint))
}

pub fn fingerprint(kind: &str, key: &str) -> String {
    format!("{kind}: {}", normal(key))
}

pub fn seen(state: &Path, kind: &str, key: &str, summary: &str, checkout: Option<&Path>, now: u64) -> Result<Option<String>> {
    let print = fingerprint(kind, key);
    if let Some(issue) = issue_with(state, &print) {
        return super::watch::again(state, &issue, Source::Incident, kind, summary, checkout, now).map(|_| Some(issue.id));
    }
    let raised = {
        let _lock = super::locked(state)?;
        let mut all = load(state);
        let entry = all.entry(print.clone()).or_insert_with(|| Seen { kind: kind.to_string(), summary: String::new(), count: 0, first: now, last: now, found_nothing: 0 });
        entry.count += 1;
        entry.last = now;
        entry.summary = super::clipped(summary, 400);
        let raised = entry.count >= needed(entry.found_nothing);
        if raised {
            entry.count = 0;
        }
        if all.len() > KEPT {
            let oldest = all.iter().min_by_key(|(_, seen)| seen.last).map(|(print, _)| print.clone());
            oldest.and_then(|oldest| all.remove(&oldest));
        }
        crate::state::write(&file(state), serde_json::to_string_pretty(&all).map_err(|error| error.to_string())?)?;
        raised
    };
    if !raised {
        return Ok(None);
    }
    super::add_case(state, &NewCase { source: Source::Incident, from: kind, words: summary, context: &format!("fingerprint: {print}"), fingerprint: Some(&print) }, now)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Incident {
    pub kind: &'static str,
    pub key: String,
    pub summary: String,
}

impl Incident {
    pub fn red_ci(repo: &str, job: &str, address: &str, what: &str, log: &str) -> Incident {
        let first = log.lines().find(|line| line.to_lowercase().contains("error")).unwrap_or_default();
        let message = first.rsplit('\t').next().unwrap_or(first).trim();
        let error = message.split_once(' ').filter(|(stamp, _)| stamp.ends_with('Z') && stamp.contains('T')).map_or(message, |(_, rest)| rest);
        Incident { kind: "red CI", key: format!("{repo} {job} {error}"), summary: format!("CI went red for {address} in {repo}: {what}; first error: {}", super::clipped(first, 300)) }
    }

    pub fn stuck(reason: &str) -> Incident {
        Incident { kind: "task stuck", key: reason.to_string(), summary: format!("A task stopped and waits for the user: {reason}") }
    }

    pub fn left_line(address: &str, why: &str) -> Incident {
        Incident { kind: "push left the line", key: why.to_string(), summary: format!("kebbie took {address} out of the push line, unpushed: {why}") }
    }

    pub fn routine_failed(routine: &str, error: &str) -> Incident {
        Incident { kind: "keeper routine failed", key: format!("{routine} {error}"), summary: format!("The keeper's {routine} routine failed: {error}") }
    }

    pub fn kebbie_error(component: &str, kind: &str) -> Incident {
        Incident { kind: "kebbie error", key: format!("{component} {kind}"), summary: format!("kebbie recorded a {kind} in {component}") }
    }

    #[cfg(test)]
    pub fn fingerprint(&self) -> String {
        fingerprint(self.kind, &self.key)
    }

    pub fn seen(&self, state: &Path, checkout: Option<&Path>, now: u64) -> Result<Option<String>> {
        seen(state, self.kind, &self.key, &self.summary, checkout, now)
    }

    // A call site's test hands its code a state dir of its own, which this never sees, so its incidents would land in another test's.
    pub fn note(&self) {
        if cfg!(test) || !super::watch::on() {
            return;
        }
        let Ok(state) = crate::lead::state_dir() else { return };
        if let Err(error) = self.seen(&state, super::watch::kebbie_checkout().as_deref(), crate::messages::now()) {
            eprintln!("kebbie: the fault watcher could not record a {} incident: {error}", self.kind);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faults::watch::{Kind, Verdict};
    use crate::faults::{Case, cases, in_use, occurrences};

    fn state(name: &str) -> PathBuf {
        crate::sh::test_dir(&format!("incidents-{name}"))
    }

    fn new_fault(title: &str) -> Verdict {
        Verdict { case: String::new(), kind: Some(Kind::New), issue_kind: Some("bug".to_string()), title: Some(title.to_string()), happened: Some("CI went red on lint".to_string()), expected: Some("the lint runs before the push".to_string()), cause: Some("the push line runs no lint".to_string()), barrier: Some("role".to_string()), files: None, question: None, task: None, reason: String::new() }
    }

    #[test]
    fn a_fingerprint_keeps_the_words_and_drops_what_changes_between_runs() {
        let one = fingerprint("red CI", "app lint · clippy error: unused variable `x` at src/main.rs:12:5 in 3f9a2c41");
        let two = fingerprint("red CI", "app lint · clippy error: unused variable `x` at src/lib.rs:80:9 in 77aa01be");
        assert_eq!(one, two);
        assert_ne!(one, fingerprint("red CI", "app test · error: a test failed"));
        assert!(one.starts_with("red CI: app lint"), "{one}");
    }

    #[test]
    fn an_incident_becomes_a_case_the_second_time_and_the_count_starts_again() {
        let state = state("incident");
        let seen = |at| seen(&state, "red CI", "app lint clippy unused variable", "CI went red: clippy", None, at).unwrap();
        assert_eq!(seen(10), None, "once is no pattern");
        let raised = seen(20).expect("the second time is a case");
        let raised = cases(&state).into_iter().find(|case| case.id == raised).unwrap();
        assert_eq!((raised.source, raised.fingerprint.as_deref()), (Source::Incident, Some("red CI: app lint clippy unused variable")));
        assert_eq!(seen(30), None, "the count starts again once it is a case");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn an_incident_an_issue_holds_goes_to_that_issue_and_reopens_it_once_its_fix_was_in_use() {
        let state = state("incident-known");
        let raised = Case { id: "c1".to_string(), at: 10, source: Source::Incident, from: "red CI".to_string(), words: "CI went red: clippy".to_string(), context: String::new(), fingerprint: None, settled: None, issue: None, tries: 0, pointed: false };
        let raised = Case { fingerprint: Some(fingerprint("red CI", "app lint clippy unused variable")), ..raised };
        let issue = crate::faults::watch::filed_fault(&state, &raised, &new_fault("Pushes reach CI without the lint run"), 10).unwrap();
        let dir = issues::dir(&state);
        let mut fixed = issues::find(&dir, &issue.id).unwrap();
        fixed.status = "fixed".to_string();
        fixed.history.push("fixed in 0.42.0 (abc1234), checked automatically".to_string());
        issues::save(&dir, &fixed).unwrap();
        assert!(in_use(&fixed));
        assert_eq!(seen(&state, "red CI", "app lint clippy unused variable", "CI went red again", None, 50).unwrap().as_deref(), Some(issue.id.as_str()), "no new case: the issue takes it");
        let back = issues::find(&dir, &issue.id).unwrap();
        assert!(back.is_open() && back.history.iter().any(|line| line.starts_with(issues::CAME_BACK)), "{back:?}");
        assert_eq!(back.barrier.as_deref(), Some("code"), "a role fix did not hold, so the next fix is a code barrier");
        assert!(back.history.iter().any(|line| line == "a role fix did not hold, so the next is a code barrier"), "{:?}", back.history);
        let seen = occurrences(&state, &issue.id);
        assert_eq!(seen.iter().map(|occurrence| occurrence.came_back).collect::<Vec<_>>(), [false, true]);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn an_incident_the_watcher_found_no_fault_in_must_be_seen_twice_as_often_again() {
        let state = state("incident-ignored");
        let seen = |at| seen(&state, "keeper routine failed", "push gh failed with 502", "The keeper's push routine failed", None, at).unwrap();
        let ignore = |id: &str| {
            let raised = cases(&state).into_iter().find(|case| case.id == id).unwrap();
            crate::faults::watch::act(&state, &raised, &Verdict { case: id.to_string(), kind: Some(Kind::Ignore), reason: "a flaky network".to_string(), ..Verdict::default() }, None, 0).unwrap();
            crate::faults::settle(&state, id, "ignored", None).unwrap();
        };
        assert_eq!(seen(1), None);
        ignore(&seen(2).expect("the second time is a case"));
        assert_eq!((3..6).map(seen).collect::<Vec<_>>(), [None, None, None]);
        ignore(&seen(6).expect("the fourth time after a no-fault verdict"));
        assert!((7..14).map(seen).all(|raised| raised.is_none()));
        assert!(seen(14).is_some(), "then the eighth");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn an_incident_matched_to_a_known_issue_lands_on_it_from_then_on() {
        let state = state("incident-matched");
        let dir = issues::dir(&state);
        issues::save(&dir, &Issue { id: "bb2200".to_string(), title: "Pushes reach CI without the lint run".to_string(), kind: "bug".to_string(), status: "open".to_string(), by: crate::faults::BY.to_string(), fix: Some("fault-bb2200".to_string()), ..Issue::default() }).unwrap();
        seen(&state, "red CI", "app lint clippy unused variable", "CI went red", None, 1).unwrap();
        let raised = seen(&state, "red CI", "app lint clippy unused variable", "CI went red", None, 2).unwrap().unwrap();
        let raised = cases(&state).into_iter().find(|case| case.id == raised).unwrap();
        crate::faults::watch::act(&state, &raised, &Verdict { case: raised.id.clone(), kind: Some(Kind::Known("bb2200".to_string())), ..Verdict::default() }, None, 2).unwrap();
        crate::faults::settle(&state, &raised.id, "known", Some("bb2200")).unwrap();
        assert_eq!(seen(&state, "red CI", "app lint clippy unused variable", "CI went red", None, 3).unwrap().as_deref(), Some("bb2200"), "the issue takes the next sighting, with no new case");
        assert!(crate::faults::pending(&state).is_empty());
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn two_red_runs_of_one_cause_give_one_fingerprint() {
        let run = |log: &str| Incident::red_ci("app", "lint · clippy", "billing@app", "CI · lint", log);
        let one = run("lint\tRun clippy\t2026-11-25T03:35:12.1Z error: unused variable `x` at src/main.rs:12:5\nmore");
        let two = run("lint\tRun clippy\t2026-11-26T07:51:40.9Z error: unused variable `x` at src/lib.rs:80:9");
        assert_eq!(one.fingerprint(), two.fingerprint(), "the time, the path and the line change from run to run");
        assert!(one.fingerprint().starts_with("red CI: app lint · clippy error: unused variable"), "{}", one.fingerprint());
        assert_ne!(one.fingerprint(), run("test\tRun tests\t2026-11-26T07:51:40.9Z error: a test failed").fingerprint());
        assert!(one.summary.contains("billing@app") && one.summary.contains("first error:"), "{}", one.summary);
    }

    #[test]
    fn each_incident_kind_counts_apart() {
        let all = [Incident::red_ci("app", "lint", "a@app", "w", "x error: y"), Incident::stuck("the usage limit"), Incident::left_line("a@app", "its tests failed"), Incident::routine_failed("push", "gh failed with 502"), Incident::kebbie_error("keeper.lock", "CorruptState")];
        let prints: std::collections::BTreeSet<String> = all.iter().map(Incident::fingerprint).collect();
        assert_eq!(prints.len(), all.len());
        assert_eq!(Incident::routine_failed("push", "gh failed with 502").fingerprint(), Incident::routine_failed("push", "gh failed with 504").fingerprint());
    }

    #[test]
    fn every_place_that_meets_an_incident_counts_it() {
        for (source, call) in [(include_str!("../ci.rs"), "Incident::red_ci("), (include_str!("../recover.rs"), "Incident::stuck("), (include_str!("../tasks/line.rs"), "Incident::left_line("), (include_str!("../keeper.rs"), "Incident::routine_failed("), (include_str!("../signals.rs"), "Incident::kebbie_error(")] {
            let at = source.find(call).unwrap_or_else(|| panic!("{call} is no longer called, so that incident is never counted"));
            assert!(source[at..].split_once(';').is_some_and(|(called, _)| called.ends_with(".note()")), "{call} builds an incident it never notes");
        }
    }
}
