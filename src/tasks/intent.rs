use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::messages::{self, Message};
use crate::permit::Caller;
use crate::repo::default_branch;
use crate::sh::Result;
use super::approval::{approval_holds, covers, fingerprint};
use super::pool::{Slot, find_task};

// Which of the task's commits a push takes: all of them, or the first `count` of the `of` it had when the user chose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Through {
    All,
    First { count: usize, of: usize },
}

// Only the user or the lead ever push, so an intent names one of them and nothing else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum By {
    User,
    Lead,
}

impl By {
    pub(super) fn of(caller: &Caller) -> Option<By> {
        match caller {
            Caller::User => Some(By::User),
            Caller::Lead => Some(By::Lead),
            Caller::Task(_) | Caller::Agent => None,
        }
    }

    pub(super) fn caller(self) -> Caller {
        match self {
            By::User => Caller::User,
            By::Lead => Caller::Lead,
        }
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            By::User => "user",
            By::Lead => "lead",
        }
    }

    pub(crate) fn word(self) -> &'static str {
        match self {
            By::User => "the user",
            By::Lead => "the lead",
        }
    }

    pub(crate) fn of_word(word: &str) -> Option<By> {
        [By::User, By::Lead].into_iter().find(|by| by.word() == word)
    }
}

// What the task's Claude does before the push goes on: test the files the default branch changed under it, resolve the rebase that conflicted, or fix what turned CI red after the push landed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Work {
    Tests,
    Conflict,
    Ci,
}

impl Work {
    fn key(self) -> &'static str {
        match self {
            Work::Tests => "tests",
            Work::Conflict => "conflict",
            Work::Ci => "ci",
        }
    }
}

// An authorised push that waits for the task's Claude; it goes out for the commits it was recorded for, or for commits the lead approved since.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Intent {
    pub by: By,
    pub work: Work,
    pub address: String,
    pub repo: PathBuf,
    pub task: String,
    pub fingerprint: String,
    pub through: Through,
    pub at: u64,
    pub seen: Option<String>,
    pub hand: Option<String>,
}

impl Intent {
    fn to_json(&self) -> Value {
        let through = match self.through {
            Through::All => Value::Null,
            Through::First { count, of } => json!({ "count": count, "of": of }),
        };
        json!({
            "by": self.by.key(), "work": self.work.key(),
            "address": self.address, "repo": self.repo, "task": self.task, "fingerprint": self.fingerprint,
            "through": through, "at": self.at, "seen": self.seen, "hand": self.hand,
        })
    }

    fn from_json(value: &Value) -> Option<Intent> {
        let by = match value["by"].as_str()? {
            "user" => By::User,
            "lead" => By::Lead,
            _ => return None,
        };
        let work = match value["work"].as_str() {
            Some("conflict") => Work::Conflict,
            Some("ci") => Work::Ci,
            _ => Work::Tests,
        };
        let through = match &value["through"] {
            Value::Null => Through::All,
            through => Through::First { count: usize::try_from(through["count"].as_u64()?).ok()?, of: usize::try_from(through["of"].as_u64()?).ok()? },
        };
        Some(Intent {
            by,
            work,
            address: value["address"].as_str()?.to_string(),
            repo: PathBuf::from(value["repo"].as_str()?),
            task: value["task"].as_str()?.to_string(),
            fingerprint: value["fingerprint"].as_str().filter(|text| !text.is_empty() || work == Work::Ci)?.to_string(),
            through,
            at: value["at"].as_u64()?,
            seen: value["seen"].as_str().map(str::to_string),
            hand: value["hand"].as_str().map(str::to_string),
        })
    }
}

fn dir(state: &Path) -> PathBuf {
    state.join("push-intents")
}

fn file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(address)
}

pub(super) fn save(state: &Path, intent: &Intent) -> Result<()> {
    crate::state::write(&file(state, &intent.address), intent.to_json().to_string())
}

pub(super) fn load(state: &Path, address: &str) -> Option<Intent> {
    Intent::from_json(&serde_json::from_str(&std::fs::read_to_string(file(state, address)).ok()?).ok()?).filter(|intent| intent.address == address)
}

// A push under way, as the board shows it: who asked, what the task's Claude does for it, and since when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pending {
    pub by: By,
    pub work: Work,
    pub at: u64,
}

// Reads only: the board and kelpie status never claim or drop an intent.
pub(crate) fn pending(state: &Path, address: &str) -> Option<Pending> {
    load(state, address).map(|intent| Pending { by: intent.by, work: intent.work, at: intent.at })
}

// The pushed commits are on the default branch, so the push waits for a fix the lead checks: no commit of the task's is approved by the push itself.
pub(crate) fn after_red_ci(state: &Path, repo: &Path, address: &str, by: By) -> Result<()> {
    let task = address.split('@').next().unwrap_or(address).to_string();
    let log = messages::load();
    let seen = messages::latest_reports(&log).get(address).map(|report| report.id.clone());
    save(state, &Intent { by, work: Work::Ci, address: address.to_string(), repo: repo.to_path_buf(), task, fingerprint: String::new(), through: Through::All, at: messages::now(), seen, hand: last_hand_over(&log, address) })
}

fn last_hand_over(log: &[Message], address: &str) -> Option<String> {
    log.iter().rev().find(|message| message.to == address && message.kind.as_deref() == Some(messages::HAND_KIND)).map(|message| message.id.clone())
}

pub(super) fn note_hand_over(state: &Path, address: &str) -> Result<()> {
    let Some(mut intent) = load(state, address) else { return Ok(()) };
    intent.hand = last_hand_over(&messages::load(), address);
    save(state, &intent)
}

// A ready report answers the hand-over only when the task's Claude read it before reporting; without a transcript to tell, a report after the hand-over went out does.
fn answers_hand_over(intent: &Intent, report: &Message, log: &[Message], transcript: Option<&str>) -> bool {
    let Some(hand) = intent.hand.as_ref().and_then(|id| log.iter().find(|message| &message.id == id)) else { return true };
    if hand.held {
        return false;
    }
    match transcript {
        Some(transcript) => crate::typing::read_at(transcript, &messages::typed_form(hand), hand.at * 1000).is_some_and(|read| report.at * 1000 > read),
        None => report.at > hand.at,
    }
}

pub(super) fn forget(state: &Path, address: &str) {
    let _ = std::fs::remove_file(file(state, address));
}

// A file that does not read as an intent of the user or the lead is removed unread.
fn all(state: &Path) -> Vec<Intent> {
    let Ok(entries) = std::fs::read_dir(dir(state)) else { return Vec::new() };
    let mut intents = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        match load(state, &name) {
            Some(intent) => intents.push(intent),
            None => forget(state, &name),
        }
    }
    intents.sort_by(|one, other| one.address.cmp(&other.address));
    intents
}

// Moving the file away is atomic, so a board and a keeper never push the same intent twice.
fn claim(state: &Path, address: &str) -> bool {
    let claimed = dir(state).join(format!(".{address}.claimed"));
    let taken = std::fs::rename(file(state, address), &claimed).is_ok();
    let _ = std::fs::remove_file(claimed);
    taken
}

#[derive(Debug, PartialEq)]
pub(super) enum Next {
    Wait,
    Push,
    Drop(String),
}

// The push completes only on a ready report written after the intent, with the commits the intent was recorded for or ones the lead approved since; changed commits wait for that approval.
pub(super) fn next(intent: &Intent, report: Option<&Message>, slot: Option<&Slot>, current: Option<&str>, approved: bool) -> Next {
    if slot.is_none() {
        return Next::Drop(format!("{} is gone, so kelpie dropped the push {} asked for", intent.address, intent.by.word()));
    }
    let ready = report.is_some_and(|report| report.at > intent.at && intent.seen.as_deref() != Some(report.id.as_str()) && report.report.as_ref().is_some_and(|report| report.state == "ready"));
    if !ready {
        return Next::Wait;
    }
    if current.is_some_and(|current| covers(&intent.fingerprint, current)) || approved {
        return Next::Push;
    }
    Next::Wait
}

pub fn complete(state: &Path) -> Vec<String> {
    let sessions = crate::peers::sessions().unwrap_or_default();
    let read = |intent: &Intent, slot: &Slot| {
        let pane = sessions.iter().find(|session| session.address == intent.address).and_then(|session| session.pane.clone());
        let file = pane.and_then(|pane| crate::typing::transcript_of(&pane)).or_else(|| super::launch::transcript(&intent.repo, slot).map(|(_, file)| file))?;
        std::fs::read_to_string(file).ok()
    };
    complete_in(state, &messages::load(), read, |intent, slot| super::push::push_task(&intent.by.caller(), &intent.repo, slot, intent.through))
}

pub(super) fn complete_in(state: &Path, log: &[Message], read: impl Fn(&Intent, &Slot) -> Option<String>, mut push: impl FnMut(&Intent, &Slot) -> Result<String>) -> Vec<String> {
    let reports = messages::latest_reports(log);
    let mut notes = Vec::new();
    for intent in all(state) {
        let slot = find_task(&intent.repo, Some(&intent.task), &intent.repo).ok();
        let current = slot.as_ref().and_then(|slot| default_branch(&intent.repo).and_then(|upstream| fingerprint(&slot.path, &upstream)).ok());
        let approved = current.as_deref().is_some_and(|current| approval_holds(state, &intent.address, current));
        let report = reports.get(&intent.address).filter(|report| slot.as_ref().is_none_or(|slot| answers_hand_over(&intent, report, log, read(&intent, slot).as_deref())));
        match next(&intent, report, slot.as_ref(), current.as_deref(), approved) {
            Next::Wait => {}
            Next::Drop(why) => {
                forget(state, &intent.address);
                tell_lead(&format!("{why}."));
                notes.push(format!("push: {why}"));
            }
            Next::Push => {
                let Some(slot) = slot.filter(|_| claim(state, &intent.address)) else { continue };
                let said = match push(&intent, &slot) {
                    Ok(said) => said,
                    Err(error) if super::push::rejected(&error) && save(state, &intent).is_ok() => {
                        format!("the push of {} that {} asked for was rejected while the default branch kept moving; kelpie tries again next round", intent.address, intent.by.word())
                    }
                    Err(error) => {
                        let text = format!("kelpie could not finish the push of {} that {} asked for: {error}", intent.address, intent.by.word());
                        tell_lead(&text);
                        text
                    }
                };
                notes.push(format!("push: {}: {said}", intent.address));
            }
        }
    }
    notes
}

fn tell_lead(text: &str) {
    if let Err(error) = messages::tell_lead(text, Some("push")) {
        eprintln!("Could not tell the lead: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::Report;
    use crate::repo::git;
    use crate::tasks::test_support::*;

    fn report(id: &str, at: u64, state: &str) -> Message {
        Message {
            id: id.to_string(), at, from: "billing@app".to_string(), to: crate::status::LEAD_LABEL.to_string(), text: "tests pass".to_string(), held: false,
            report: Some(Report { state: state.to_string(), next: "push".to_string(), head: None }), ended: false, question: None, about: None, kind: None, landed: None, why: None, forced: false, fyi: false,
        }
    }

    fn intent(by: By, repo: &Path, fingerprint: &str) -> Intent {
        Intent { by, work: Work::Tests, address: format!("billing@{}", crate::repo::name_of(repo)), repo: repo.to_path_buf(), task: "billing".to_string(), fingerprint: fingerprint.to_string(), through: Through::All, at: 100, seen: Some("before".to_string()), hand: None }
    }

    #[test]
    fn an_intent_names_the_user_or_the_lead_and_reads_back_as_written() {
        let state = std::env::temp_dir().join(format!("kelpie-intents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        let mut written = intent(By::Lead, Path::new("/repos/app"), "abc");
        written.through = Through::First { count: 1, of: 2 };
        written.work = Work::Conflict;
        save(&state, &written).unwrap();
        assert_eq!(load(&state, "billing@app"), Some(written.clone()));
        let mut older = written.to_json();
        older.as_object_mut().unwrap().remove("work");
        crate::state::write(&file(&state, "billing@app"), older.to_string()).unwrap();
        assert_eq!(load(&state, "billing@app").map(|intent| intent.work), Some(Work::Tests), "an intent written before conflicts were recorded waits for tests");
        save(&state, &written).unwrap();
        for by in ["agent", "task", "", "User"] {
            let mut forged = written.to_json();
            forged["by"] = json!(by);
            crate::state::write(&file(&state, "billing@app"), forged.to_string()).unwrap();
            assert_eq!(load(&state, "billing@app"), None, "{by:?} never pushes");
            assert!(all(&state).is_empty() && !file(&state, "billing@app").exists(), "a file that is no intent of the user or the lead is removed unread");
        }
        assert_eq!(By::of(&Caller::Agent), None);
        assert_eq!(By::of(&Caller::Task("billing@app".to_string())), None);
        assert_eq!(By::of(&Caller::Lead).map(By::caller), Some(Caller::Lead));
        save(&state, &written).unwrap();
        assert_eq!((pending(&state, "billing@app"), pending(&state, "other@app")), (Some(Pending { by: By::Lead, work: Work::Conflict, at: 100 }), None));
        assert_eq!(load(&state, "billing@app"), Some(written.clone()), "reading what waits leaves the intent in place");
        assert!(claim(&state, "billing@app") && !claim(&state, "billing@app"), "one claim wins");
        assert_eq!(pending(&state, "billing@app"), None, "a claimed intent waits no more");
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_push_completes_only_on_a_new_ready_report_with_the_same_commits_or_approved_ones() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let waiting = intent(By::User, &dir, "abc");
        assert_eq!(next(&waiting, None, Some(&slot), Some("abc"), false), Next::Wait);
        assert_eq!(next(&waiting, Some(&report("before", 200, "ready")), Some(&slot), Some("abc"), false), Next::Wait, "the report it was pushed on is not the check");
        assert_eq!(next(&waiting, Some(&report("r1", 90, "ready")), Some(&slot), Some("abc"), false), Next::Wait);
        assert_eq!(next(&waiting, Some(&report("r1", 200, "needs-you")), Some(&slot), Some("abc"), true), Next::Wait, "a question or a red test keeps the push waiting");
        assert_eq!(next(&waiting, Some(&report("r1", 200, "ready")), Some(&slot), Some("abc"), false), Next::Push);
        assert_eq!(next(&waiting, Some(&report("r1", 200, "ready")), Some(&slot), Some("abd"), false), Next::Wait, "changed commits wait for the lead's rules check");
        assert_eq!(next(&waiting, Some(&report("r1", 200, "ready")), Some(&slot), Some("abd"), true), Next::Push, "the lead approved the changed commits");
        assert!(matches!(next(&waiting, Some(&report("r1", 200, "ready")), None, Some("abc"), true), Next::Drop(why) if why.contains("is gone")));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_waiting_push_completes_only_on_a_ready_report_written_after_its_claude_read_the_hand_over() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let current = fingerprint(&slot.path, &upstream).unwrap();
        let address = format!("billing@{}", crate::repo::name_of(&dir));
        let hand = |held: bool| { let mut message = report("h1", 150, "ready"); (message.from, message.to, message.text, message.report, message.held, message.kind) = ("kelpie".to_string(), address.clone(), "run the tests".to_string(), None, held, Some(messages::HAND_KIND.to_string())); message };
        let ready = |at: u64| { let mut message = report("r1", at, "ready"); message.from = address.clone(); message };
        let read_at = |seconds: u64| json!({ "type": "user", "timestamp": format!("1970-01-01T00:{:02}:{:02}.000Z", seconds / 60, seconds % 60), "message": { "content": "[from kelpie] run the tests" } }).to_string();
        let pushes = |log: Vec<Message>, transcript: Option<String>| {
            save(&state, &Intent { hand: Some("h1".to_string()), ..intent(By::User, &dir, &current) }).unwrap();
            let mut pushed = false;
            complete_in(&state, &log, |_, _| transcript.clone(), |_, _| { pushed = true; Ok(String::new()) });
            forget(&state, &address);
            pushed
        };
        assert!(!pushes(vec![hand(true), ready(200)], None), "the hand-over is still held behind a draft: the report is from an earlier turn");
        assert!(!pushes(vec![hand(false), ready(200)], Some(String::new())), "typed, but its Claude has not read it yet, as when the forced report after a notice comes first");
        assert!(!pushes(vec![hand(false), ready(200)], Some(read_at(201))), "read after that report");
        assert!(pushes(vec![hand(false), ready(200)], Some(read_at(160))), "read, then the tests, then ready");
        assert!(pushes(vec![hand(false), ready(200)], None), "no transcript to tell: a report after the hand-over went out answers it");
        assert!(!pushes(vec![hand(false), ready(140)], None), "and one before it does not");
        assert!(pushes(vec![ready(200)], None), "a hand-over the log no longer has holds nothing up");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_red_ci_after_a_push_waits_as_that_push_and_sends_only_a_fix_the_lead_approved() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = format!("billing@{}", crate::repo::name_of(&dir));
        after_red_ci(&state, &dir, &address, By::User).unwrap();
        let red = load(&state, &address).expect("a red CI's push has no commits of its own yet, and still waits");
        assert_eq!((red.by, red.work, red.fingerprint.as_str(), red.through, red.task.as_str()), (By::User, Work::Ci, "", Through::All, "billing"));
        let mut forged = red.to_json();
        forged["work"] = json!("tests");
        crate::state::write(&file(&state, &address), forged.to_string()).unwrap();
        assert_eq!(load(&state, &address), None, "only a red CI's push waits without commits");
        let ready = report("r1", red.at + 1, "ready");
        assert_eq!(next(&red, Some(&ready), Some(&slot), Some(""), false), Next::Wait, "ready without a fix: nothing to push");
        assert_eq!(next(&red, Some(&ready), Some(&slot), Some("fix"), false), Next::Wait, "the fix waits for the lead's rules check");
        assert_eq!(next(&red, Some(&ready), Some(&slot), Some("fix"), true), Next::Push);
        assert_eq!(next(&red, Some(&report("r1", red.at + 1, "needs-you")), Some(&slot), Some("fix"), true), Next::Wait, "a fix that needs the user waits for them");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn kelpie_pushes_for_the_recorded_pusher_once_and_changed_commits_once_the_lead_approves_them() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let current = fingerprint(&slot.path, &upstream).unwrap();
        let address = format!("billing@{}", crate::repo::name_of(&dir));
        let ready = |id: &str| { let mut message = report(id, 200, "ready"); message.from = address.clone(); vec![message] };
        save(&state, &intent(By::Lead, &dir, &current)).unwrap();
        let mut pushed = Vec::new();
        complete_in(&state, &[], |_, _| None, |intent, _| { pushed.push(intent.by); Ok(String::new()) });
        assert!(pushed.is_empty() && load(&state, &address).is_some(), "no report yet: it waits");
        complete_in(&state, &ready("r1"), |_, _| None, |intent, _| { pushed.push(intent.by); Ok(String::new()) });
        assert_eq!(pushed, [By::Lead], "pushed as the lead that asked, under its own checks");
        assert!(load(&state, &address).is_none());
        complete_in(&state, &ready("r1"), |_, _| None, |intent, _| { pushed.push(intent.by); Ok(String::new()) });
        assert_eq!(pushed.len(), 1, "an intent pushes once");
        save(&state, &intent(By::User, &dir, &current)).unwrap();
        commit(&slot.path, "e", "a fix while testing");
        complete_in(&state, &ready("r2"), |_, _| None, |intent, _| { pushed.push(intent.by); Ok(String::new()) });
        assert_eq!(pushed.len(), 1, "changed commits are never pushed on the old commits' word");
        assert!(load(&state, &address).is_some(), "the push stays under way while the lead checks the fix");
        super::super::approval::record_approval(&state, &address, &fingerprint(&slot.path, &upstream).unwrap()).unwrap();
        complete_in(&state, &ready("r2"), |_, _| None, |intent, _| { pushed.push(intent.by); Ok(String::new()) });
        assert_eq!(pushed, [By::Lead, By::User], "the lead's approval of the fix lets the user's push go out");
        assert!(load(&state, &address).is_none());
        let current = fingerprint(&slot.path, &upstream).unwrap();
        save(&state, &intent(By::User, &dir, &current)).unwrap();
        let rejected = "Pushing billing failed: ! [rejected]        4333c9f -> main (non-fast-forward)";
        let notes = complete_in(&state, &ready("r3"), |_, _| None, |_, _| Err(rejected.to_string()));
        assert!(load(&state, &address).is_some() && notes[0].contains("tries again"), "a push main kept outrunning waits for the next round, never dropped: {notes:?}");
        complete_in(&state, &ready("r3"), |_, _| None, |_, _| Err("the reviewer has not passed it".to_string()));
        assert!(load(&state, &address).is_none(), "any other failure goes to the lead");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_waiting_on_a_task_that_is_rebasing_stays_and_goes_out_once_the_task_is_ready() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = format!("billing@{}", crate::repo::name_of(&dir));
        save(&state, &intent(By::User, &dir, &fingerprint(&slot.path, &upstream).unwrap())).unwrap();
        assert!(git(&slot.path, &["rebase", "--exec", "false", &upstream]).is_err(), "the rebase stops part-way");
        assert!(crate::repo::rebase_in_progress(&slot.path));
        assert_eq!(find_task(&dir, Some("billing"), &dir).map(|found| found.path).ok(), Some(slot.path.clone()), "the rebasing worktree is still the task's");
        let mut pushed = 0;
        let notes = complete_in(&state, &[], |_, _| None, |_, _| { pushed += 1; Ok(String::new()) });
        assert!(notes.is_empty() && load(&state, &address).is_some(), "the push waits through the rebase: {notes:?}");
        git(&slot.path, &["rebase", "--continue"]).unwrap();
        let mut ready = report("r1", 200, "ready");
        ready.from = address.clone();
        complete_in(&state, &[ready], |_, _| None, |_, _| { pushed += 1; Ok(String::new()) });
        assert_eq!(pushed, 1, "the task's ready report sends the push");
        assert!(load(&state, &address).is_none());
        cleanup(dir, origin, &slot);
    }
}
