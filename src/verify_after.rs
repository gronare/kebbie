use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::messages;
use crate::sh::Result;

// What the user verifies once the task's change is out, named with its ready report; asked is kebbie's question once the task would close, landed the commits the push named for its done record, fixing when the user saw it fail while no push of the fix is out yet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Held {
    pub what: String,
    pub asked: Option<String>,
    pub landed: Option<String>,
    pub fixing: Option<u64>,
}

pub const FLAG: &str = "--verify-after-push";

fn file(state: &Path, address: &str) -> PathBuf {
    state.join("verify-after-push").join(format!("{address}.json"))
}

pub fn goes_with(state: &str, verify: Option<&str>, usage: &str, limit: usize) -> Result<Option<String>> {
    let Some(verify) = verify else { return Ok(None) };
    if state != "ready" {
        return Err(format!("{FLAG} goes with --state ready: it names what the user verifies once the push is out. {usage}"));
    }
    let verify = verify.split_whitespace().collect::<Vec<_>>().join(" ");
    if verify.is_empty() {
        return Err(format!("{FLAG} needs what the user verifies once the push is out, and where: {usage}"));
    }
    if verify.chars().count() > limit {
        return Err(format!("{FLAG} is over {limit} characters; the user reads it as one question, so name what to check and where, not its steps"));
    }
    Ok(Some(verify))
}

pub fn load(state: &Path, address: &str) -> Option<Held> {
    let value: Value = serde_json::from_str(&fs::read_to_string(file(state, address)).ok()?).ok()?;
    Some(Held { what: value["what"].as_str()?.to_string(), asked: value["asked"].as_str().map(str::to_string), landed: value["landed"].as_str().map(str::to_string), fixing: value["fixing"].as_u64() })
}

// Held across each read, change and write of the file, which the report, the push or CI watch and the user's answer each rewrite from their own process.
fn locked(state: &Path, address: &str) -> Result<crate::state::Locked> {
    crate::state::Locked::wait(&file(state, address).with_extension("lock"))
}

fn save(state: &Path, address: &str, held: &Held) -> Result<()> {
    crate::state::write(&file(state, address), json!({ "what": held.what, "asked": held.asked, "landed": held.landed, "fixing": held.fixing }).to_string())
}

// A later ready report names the check anew, withdrawing a question about the old one; the commits of a push already out and a fix still to push stay.
pub fn name(state: &Path, address: &str, what: &str) -> Result<()> {
    let _held = locked(state, address)?;
    let held = load(state, address).unwrap_or_default();
    if let Some(id) = &held.asked {
        crate::kebbie_questions::withdraw(state, id);
    }
    save(state, address, &Held { what: what.to_string(), asked: None, ..held })
}

// kebbie's question waits for the user's answer.
pub fn waiting(state: &Path, address: &str) -> bool {
    load(state, address).is_some_and(|held| held.asked.is_some_and(|id| crate::kebbie_questions::load(state).iter().any(|asked| asked.id == id)))
}

// Nothing but the user's Verified closes the task: its question waits, or the fix of what the user saw is not pushed yet.
pub fn holds_open(state: &Path, address: &str) -> bool {
    waiting(state, address) || load(state, address).is_some_and(|held| held.fixing.is_some())
}

// Where kebbie would close the task: with a check named, it asks the user instead and the task waits under Needs you; None closes as before. pushed is the commits of the push that got it there, None when no push of kebbie's did.
pub fn hold(state: &Path, address: &str, pushed: Option<&str>) -> Option<String> {
    let _held = locked(state, address).map_err(|error| eprintln!("kebbie asks {address}'s verification without its lock: {error}")).ok();
    let held = load(state, address)?;
    if waiting(state, address) {
        return Some(waits(&held.what));
    }
    if held.fixing.is_some() && pushed.is_none() {
        return Some(format!("It waits for the fix of what you saw to be pushed; kebbie asks you again then ({}).", held.what));
    }
    let landed = pushed.map(str::to_string).filter(|commits| !commits.is_empty()).or(held.landed.clone());
    let task = crate::status::with_titles_in(state, address);
    let asked = match crate::kebbie_questions::ask_solved(state, address, &format!("{task} is out. Check: {}", held.what)) {
        Ok(asked) => asked,
        Err(error) => {
            eprintln!("kebbie could not ask you to verify {address}, so it closes the task without it: {error}");
            return None;
        }
    };
    if let Err(error) = save(state, address, &Held { asked: Some(asked.id), landed, fixing: None, ..held.clone() }) {
        eprintln!("kebbie could not keep that {address} waits on your verification: {error}");
    }
    crate::phase::record(state, address, "verify", &crate::phase::Event::VerifyAsked);
    if let Err(error) = messages::to_lead(messages::ToLead::Notice, &format!("{address} is out and waits on the user's verification under Needs you: {}. Verified closes it.", held.what)) {
        eprintln!("Could not tell the lead: {error}");
    }
    Some(waits(&held.what))
}

fn waits(what: &str) -> String {
    format!("It waits on your verification under Needs you ({what}); Verified closes it.")
}

// The user saw something wrong: the task fixes it, and only the push of the fix asks again.
pub fn reopen(state: &Path, address: &str) {
    let _held = locked(state, address).map_err(|error| eprintln!("kebbie reopens {address}'s verification without its lock: {error}")).ok();
    if let Some(held) = load(state, address) {
        let _ = save(state, address, &Held { asked: None, fixing: Some(messages::now()), ..held });
    }
}

// Since when the user waits for a fix, and the commits they saw fail, which a commit made in that same second may still be.
pub fn fixing(state: &Path, address: &str) -> Option<(u64, Vec<String>)> {
    let held = load(state, address)?;
    Some((held.fixing?, held.landed.unwrap_or_default().lines().filter_map(|line| line.split_whitespace().next()).map(str::to_string).collect()))
}

pub fn head_line(path: &Path) -> Option<String> {
    crate::repo::git(path, &["log", "-1", "--format=%h %s"]).ok().filter(|line| !line.is_empty())
}

pub fn forget(state: &Path, address: &str) {
    let _held = locked(state, address);
    let _ = fs::remove_file(file(state, address));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_for_the_user_goes_with_a_ready_report_on_one_line() {
        assert_eq!(goes_with("ready", Some(" the invoice\n page loads "), "usage", 300), Ok(Some("the invoice page loads".to_string())));
        assert_eq!(goes_with("working", None, "usage", 300), Ok(None));
        assert!(goes_with("needs-you", Some("it loads"), "usage", 300).unwrap_err().contains("goes with --state ready"));
        assert!(goes_with("ready", Some("  "), "usage", 300).unwrap_err().contains("needs what the user verifies"));
        assert!(goes_with("ready", Some(&"x".repeat(301)), "usage", 300).unwrap_err().contains("over 300 characters"));
    }

    #[test]
    fn naming_the_check_again_keeps_the_commits_already_out_and_asks_anew() {
        let state = crate::sh::test_dir("verify-after-name");
        assert_eq!(load(&state, "billing@app"), None);
        name(&state, "billing@app", "the invoice page loads").unwrap();
        save(&state, "billing@app", &Held { what: "the invoice page loads".to_string(), asked: Some("kq-1".to_string()), landed: Some("abc fix".to_string()), fixing: Some(5) }).unwrap();
        name(&state, "billing@app", "the refund page loads").unwrap();
        assert_eq!(load(&state, "billing@app"), Some(Held { what: "the refund page loads".to_string(), asked: None, landed: Some("abc fix".to_string()), fixing: Some(5) }));
        forget(&state, "billing@app");
        assert_eq!(load(&state, "billing@app"), None);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_user_is_asked_once_the_push_is_green_and_verified_closes_while_something_wrong_goes_to_the_task() {
        use crate::ci::{Pushed, State, Watch};
        use crate::kebbie_questions::{Kind, answer_in};
        use crate::permit::Caller;
        use crate::phase::{Phase, Why};
        let (dir, origin, slot, _) = crate::tasks::test_support::task_with_a_commit("billing");
        let state = crate::lead::state_dir().unwrap();
        let address = crate::peers::address("billing", &crate::repo::name_of(&dir));
        crate::repo::git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--amend", "-m", "Add refunds\n\nThe billing page lists refunds now."]).unwrap();
        assert!(crate::report::report(&slot.path, Some("working"), Some("nothing"), None, &[], None, Some("the refunds page loads"), None, "refunds").unwrap_err().contains("goes with --state ready"));
        let said = crate::report::report(&slot.path, Some("ready"), Some("push and close"), None, &[], None, Some("the refunds page loads"), None, "refunds done").unwrap();
        assert!(said.contains("asks the user to verify it once the push is out"), "{said}");
        assert!(crate::kebbie_questions::load(&state).iter().all(|asked| asked.address != address), "nothing is asked before the push");
        let gh = state.join("gh-green");
        crate::executable::write(&gh, "#!/bin/sh\necho '[{\"databaseId\":1,\"name\":\"CI\",\"workflowName\":\"CI\",\"status\":\"completed\",\"conclusion\":\"success\",\"url\":\"u\"}]'\n").unwrap();
        let push = |sha: &str, commits: &str| {
            let pushed = Pushed { target: "main".to_string(), commits: commits.to_string(), ..Pushed::default() };
            crate::ci::save(&state, &Watch { address: address.clone(), repo: dir.clone(), slug: "acme/app".to_string(), sha: sha.to_string(), started_at: 0, checked_at: None, close: true, runs: Vec::new(), state: State::Running, error: None, erring_since: None, following: None, pushed }).unwrap();
            crate::phase::record(&state, &address, "push", &crate::phase::Event::PushOut { sha });
            crate::ci::tick_with(&state, &gh.to_string_lossy(), 100)
        };
        let solved = || crate::kebbie_questions::load(&state).into_iter().find(|asked| asked.address == address && asked.kind == Kind::Solved);
        let events = push("abcdef0123", "abcdef0 Add refunds");
        assert!(events.len() == 1 && events[0].text.contains("waits on your verification under Needs you (the refunds page loads)"), "{events:?}");
        assert!(crate::tasks::find_task(&dir, Some("billing"), &dir).is_ok(), "green CI leaves the task open for the user's verification");
        assert!(crate::ci::load(&state, &address).is_none(), "the watch is done with the push");
        assert_eq!(crate::phase::load(&state, &address).map(|record| record.phase), Some(Phase::NeedsYou(Why::Verify)));
        let asked = solved().expect("kebbie asks the user");
        assert!(asked.text.ends_with("is out. Check: the refunds page loads"), "{}", asked.text);
        assert_eq!(load(&state, &address).and_then(|held| held.landed).as_deref(), Some("abcdef0 Add refunds"), "the done record keeps what the push named");
        assert_eq!(hold(&state, &address, None).map(|_| crate::kebbie_questions::load(&state).iter().filter(|one| one.address == address).count()), Some(1), "a second close asks nothing more");
        let never = |_: &str| -> std::result::Result<String, String> { panic!("Something is wrong never closes the task") };
        assert!(answer_in(&state, &Caller::User, &asked.id, Some(1), "", &never).unwrap().contains("waits"), "Not yet keeps the question");
        assert!(solved().is_some());
        let told = answer_in(&state, &Caller::User, &asked.id, Some(2), "the totals are off by one", &never).unwrap();
        assert!(told.contains("stays open to fix it"), "{told}");
        assert!(solved().is_none() && !waiting(&state, &address));
        assert_eq!(crate::phase::load(&state, &address).map(|record| record.phase), Some(Phase::Working), "out of Needs you while the task fixes it");
        let heard: Vec<String> = crate::messages::load().into_iter().filter(|message| message.to == address).map(|message| message.text).collect();
        assert!(heard.iter().any(|text| text.contains("it does not hold: the totals are off by one") && text.contains(FLAG)), "{heard:?}");
        let events = push("1234567abc", "1234567 Fix the totals");
        assert!(events[0].text.contains("waits on your verification"), "the push of the fix asks again: {events:?}");
        assert!(crate::tasks::find_task(&dir, Some("billing"), &dir).is_ok());
        let again = solved().expect("asked again");
        let closed = std::cell::RefCell::new(None);
        let close = |closing: &str| -> std::result::Result<String, String> {
            *closed.borrow_mut() = Some((closing.to_string(), load(&state, closing).and_then(|held| held.landed)));
            Ok(format!("Closed {closing}"))
        };
        assert_eq!(answer_in(&state, &Caller::User, &again.id, Some(0), "", &close).unwrap(), format!("Closed {address}"));
        assert_eq!(closed.into_inner(), Some((address.clone(), Some("1234567 Fix the totals".to_string()))), "Verified closes the task with the commits of the push");
        forget(&state, &address);
        crate::kebbie_questions::forget(&address);
        crate::tasks::test_support::cleanup(dir, origin, &slot);
    }

    #[test]
    fn naming_the_check_anew_withdraws_the_question_about_the_old_one_and_kebbie_asks_one_at_a_time() {
        use crate::kebbie_questions::Kind;
        let state = crate::sh::test_dir("verify-after-rename");
        let solved = || crate::kebbie_questions::load(&state).into_iter().filter(|asked| asked.kind == Kind::Solved).map(|asked| asked.text).collect::<Vec<_>>();
        name(&state, "billing@app", "the refunds page loads").unwrap();
        assert!(hold(&state, "billing@app", Some("abc Add refunds")).is_some());
        assert_eq!(solved().len(), 1);
        name(&state, "billing@app", "the refunds page shows totals").unwrap();
        assert!(solved().is_empty(), "the question about the old check goes");
        hold(&state, "billing@app", None);
        assert_eq!(solved().len(), 1);
        assert!(solved()[0].ends_with("Check: the refunds page shows totals"), "{:?}", solved());
        crate::kebbie_questions::ask_solved(&state, "billing@app", "billing is out. Check: something else").unwrap();
        assert_eq!(solved(), ["billing is out. Check: something else"], "a second question about the task takes the place of the first");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn naming_the_check_waits_while_another_process_asks_and_withdraws_the_question_it_asked() {
        use crate::kebbie_questions::Kind;
        let state = crate::sh::test_dir("verify-after-locked");
        let solved = || crate::kebbie_questions::load(&state).into_iter().filter(|asked| asked.kind == Kind::Solved).count();
        name(&state, "billing@app", "the refunds page loads").unwrap();
        let asking = locked(&state, "billing@app").unwrap();
        let naming = {
            let state = state.clone();
            std::thread::spawn(move || name(&state, "billing@app", "the refunds page shows totals"))
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!naming.is_finished(), "name waits for the lock the asking process holds");
        let asked = crate::kebbie_questions::ask_solved(&state, "billing@app", "billing is out. Check: the refunds page loads").unwrap();
        save(&state, "billing@app", &Held { asked: Some(asked.id), ..load(&state, "billing@app").unwrap() }).unwrap();
        drop(asking);
        naming.join().unwrap().unwrap();
        assert_eq!(solved(), 0, "name read the question the other process asked and withdrew it");
        assert_eq!(load(&state, "billing@app").map(|held| (held.what, held.asked)), Some(("the refunds page shows totals".to_string(), None)));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_ready_report_with_nothing_to_push_asks_the_users_check_and_not_whether_to_close() {
        use crate::kebbie_questions::Kind;
        use crate::phase::{Phase, Why};
        let (dir, origin, slot, upstream) = crate::tasks::test_support::task_with_a_commit("billing");
        let state = crate::lead::state_dir().unwrap();
        let address = crate::peers::address("billing", &crate::repo::name_of(&dir));
        crate::repo::git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--amend", "-m", "Add refunds\n\nThe billing page lists refunds now."]).unwrap();
        crate::repo::git(&slot.path, &["push", "--quiet", "origin", &format!("HEAD:{}", upstream.trim_start_matches("origin/"))]).unwrap();
        crate::repo::git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        let asked = || crate::kebbie_questions::load(&state).into_iter().filter(|asked| asked.address == address).map(|asked| asked.kind).collect::<Vec<_>>();
        crate::report::report(&slot.path, Some("ready"), Some("nothing"), None, &[], None, Some("the refunds page loads"), None, "refunds out").unwrap();
        assert_eq!(asked(), [Kind::Solved]);
        let phase = || crate::phase::load(&state, &address).map(|record| record.phase);
        assert_eq!(phase(), Some(Phase::NeedsYou(Why::Verify)));
        crate::report::report(&slot.path, Some("ready"), Some("nothing"), None, &[], None, None, None, "still out").unwrap();
        assert_eq!(phase(), Some(Phase::NeedsYou(Why::Verify)), "a ready report while the user's verification waits keeps the row on it");
        assert_eq!(asked(), [Kind::Solved]);
        crate::kebbie_questions::forget(&address);
        reopen(&state, &address);
        crate::report::report(&slot.path, Some("ready"), Some("nothing"), None, &[], None, Some("the refunds page loads"), None, "nothing to fix").unwrap();
        assert!(asked().is_empty(), "after Something is wrong, only the push of a fix asks again, and nothing asks to close it meanwhile");
        forget(&state, &address);
        crate::tasks::test_support::cleanup(dir, origin, &slot);
    }
}
