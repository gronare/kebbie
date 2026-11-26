use std::path::Path;

use crate::ci::Watch;
use crate::messages::{self, Message};
use crate::repo::name_of;

// The open tasks of a repo whose latest report says blocked, but the one whose push the news is about.
pub(crate) fn blocked_in(log: &[Message], repo: &str, except: &str) -> Vec<String> {
    let mut blocked: Vec<String> = messages::latest_reports(log).into_iter()
        .filter(|(address, message)| address != except && address.rsplit_once('@').is_some_and(|(_, place)| place == repo) && message.report.as_ref().is_some_and(|report| report.state == "blocked"))
        .map(|(address, _)| address)
        .collect();
    blocked.sort();
    blocked
}

// A blocked report does not say what it waits on, so each task hears the news and judges whether it was the one.
pub(crate) fn tell(repo: &Path, except: &str, text: &str) -> Vec<String> {
    let told = blocked_in(&messages::load(), &name_of(repo), except);
    for address in &told {
        if let Err(error) = messages::hand_to(address, text) {
            eprintln!("Could not tell {address} that what it waited on is clear: {error}");
        }
    }
    told
}

fn short(sha: &str) -> &str {
    &sha[..7.min(sha.len())]
}

const GO_ON: &str = "go on now: rebase onto the default branch and carry on, and report again.";

pub(crate) fn main_green(green: &Watch, reds: &[Watch]) -> String {
    let reds: Vec<String> = reds.iter().map(|red| format!("{} ({})", red.address, red.short())).collect();
    format!(
        "main is green again in {} ({}, pushed by {}): the red CI of {} no longer holds its pushes. If you reported blocked on main's red CI, {GO_ON}",
        name_of(&green.repo), short(&green.sha), green.address, crate::tasks::and_list(&reds)
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Ci {
    Green,
    NoneRan,
    Unwatched,
}

pub(crate) fn landed(address: &str, sha: &str, target: &str, ci: Ci) -> String {
    let ci = match ci {
        Ci::Green => ", and its CI is green",
        Ci::NoneRan => "; no CI ran for it",
        Ci::Unwatched => "; kebbie watches no CI for it",
    };
    format!("{address} landed on {target} ({}){ci}. If you reported blocked waiting for it, {GO_ON}", short(sha))
}

pub(crate) fn dismissed(red: &Watch) -> String {
    format!(
        "The user dismissed the red CI of {} ({}) in {}: its pushes no longer wait on it, though main's CI did not go green. If you reported blocked on it, {GO_ON}",
        red.address, red.short(), name_of(&red.repo)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(from: &str, state: &str, at: u64) -> String {
        serde_json::json!({ "id": format!("r{at}"), "at": at, "from": from, "to": "lead", "text": "", "state": "logged", "report": { "state": state, "next": "nothing" } }).to_string()
    }

    #[test]
    fn the_blocked_are_the_repos_open_tasks_whose_latest_report_says_blocked() {
        let ended = serde_json::json!({ "id": "e1", "at": 9, "from": "kebbie", "to": "gone@app", "text": "gone ended (done)", "state": "ended" }).to_string();
        let log = messages::fold(&[
            report("copy@app", "blocked", 1),
            report("billing@app", "blocked", 2),
            report("rounding@app", "blocked", 3),
            report("rounding@app", "working", 4),
            report("gone@app", "blocked", 5),
            ended,
            report("payload@web", "blocked", 6),
            report("app-fix@app2", "blocked", 7),
        ].join("\n"));
        assert_eq!(blocked_in(&log, "app", "billing@app"), ["copy@app"], "not the pusher, nor one that went on, ended, or works in another repo");
    }
}
