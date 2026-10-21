use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::messages::{self, Message};
use crate::permit::Caller;
use crate::repo::default_branch;
use crate::sh::Result;
use super::approval::fingerprint;
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

    pub(super) fn word(self) -> &'static str {
        match self {
            By::User => "the user",
            By::Lead => "the lead",
        }
    }
}

// An authorised push that waits for the task's Claude to test the files the default branch changed under it; it holds for these commits only.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Intent {
    pub by: By,
    pub address: String,
    pub repo: PathBuf,
    pub task: String,
    pub fingerprint: String,
    pub through: Through,
    pub at: u64,
    pub seen: Option<String>,
}

impl Intent {
    fn to_json(&self) -> Value {
        let through = match self.through {
            Through::All => Value::Null,
            Through::First { count, of } => json!({ "count": count, "of": of }),
        };
        json!({
            "by": self.by.key(),
            "address": self.address, "repo": self.repo, "task": self.task, "fingerprint": self.fingerprint,
            "through": through, "at": self.at, "seen": self.seen,
        })
    }

    fn from_json(value: &Value) -> Option<Intent> {
        let by = match value["by"].as_str()? {
            "user" => By::User,
            "lead" => By::Lead,
            _ => return None,
        };
        let through = match &value["through"] {
            Value::Null => Through::All,
            through => Through::First { count: usize::try_from(through["count"].as_u64()?).ok()?, of: usize::try_from(through["of"].as_u64()?).ok()? },
        };
        Some(Intent {
            by,
            address: value["address"].as_str()?.to_string(),
            repo: PathBuf::from(value["repo"].as_str()?),
            task: value["task"].as_str()?.to_string(),
            fingerprint: value["fingerprint"].as_str().filter(|text| !text.is_empty())?.to_string(),
            through,
            at: value["at"].as_u64()?,
            seen: value["seen"].as_str().map(str::to_string),
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

// Reads only: the board and kelpie status never claim or drop an intent.
pub(crate) fn pending(state: &Path, address: &str) -> Option<By> {
    load(state, address).map(|intent| intent.by)
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

// The push completes only on a ready report written after the intent, with the commits the intent was recorded for.
pub(super) fn next(intent: &Intent, report: Option<&Message>, slot: Option<&Slot>, current: Option<&str>) -> Next {
    if slot.is_none() {
        return Next::Drop(format!("{} is gone, so kelpie dropped the push {} asked for", intent.address, intent.by.word()));
    }
    let ready = report.is_some_and(|report| report.at > intent.at && intent.seen.as_deref() != Some(report.id.as_str()) && report.report.as_ref().is_some_and(|report| report.state == "ready"));
    if !ready {
        return Next::Wait;
    }
    if current != Some(intent.fingerprint.as_str()) {
        return Next::Drop(format!(
            "{} changed its commits while testing the files the default branch changed under it, so kelpie did not push it for {}: check its commits against the rules again (kelpie approve) and push it as usual",
            intent.address, intent.by.word()
        ));
    }
    Next::Push
}

pub fn complete(state: &Path) -> Vec<String> {
    complete_in(state, &messages::load(), |intent, slot| super::push::push_task(&intent.by.caller(), &intent.repo, slot, intent.through))
}

pub(super) fn complete_in(state: &Path, log: &[Message], mut push: impl FnMut(&Intent, &Slot) -> Result<String>) -> Vec<String> {
    let reports = messages::latest_reports(log);
    let mut notes = Vec::new();
    for intent in all(state) {
        let slot = find_task(&intent.repo, Some(&intent.task), &intent.repo).ok();
        let current = slot.as_ref().and_then(|slot| default_branch(&intent.repo).and_then(|upstream| fingerprint(&slot.path, &upstream)).ok());
        match next(&intent, reports.get(&intent.address), slot.as_ref(), current.as_deref()) {
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
    use crate::tasks::test_support::*;

    fn report(id: &str, at: u64, state: &str) -> Message {
        Message {
            id: id.to_string(), at, from: "billing@app".to_string(), to: crate::status::LEAD_LABEL.to_string(), text: "tests pass".to_string(), held: false,
            report: Some(Report { state: state.to_string(), next: "push".to_string() }), ended: false, question: None, about: None, kind: None, landed: None,
        }
    }

    fn intent(by: By, repo: &Path, fingerprint: &str) -> Intent {
        Intent { by, address: format!("billing@{}", crate::repo::name_of(repo)), repo: repo.to_path_buf(), task: "billing".to_string(), fingerprint: fingerprint.to_string(), through: Through::All, at: 100, seen: Some("before".to_string()) }
    }

    #[test]
    fn an_intent_names_the_user_or_the_lead_and_reads_back_as_written() {
        let state = std::env::temp_dir().join(format!("kelpie-intents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        let mut written = intent(By::Lead, Path::new("/repos/app"), "abc");
        written.through = Through::First { count: 1, of: 2 };
        save(&state, &written).unwrap();
        assert_eq!(load(&state, "billing@app"), Some(written.clone()));
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
        assert_eq!((pending(&state, "billing@app"), pending(&state, "other@app")), (Some(By::Lead), None));
        assert_eq!(load(&state, "billing@app"), Some(written.clone()), "reading what waits leaves the intent in place");
        assert!(claim(&state, "billing@app") && !claim(&state, "billing@app"), "one claim wins");
        assert_eq!(pending(&state, "billing@app"), None, "a claimed intent waits no more");
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_push_completes_only_on_a_new_ready_report_with_the_same_commits() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let waiting = intent(By::User, &dir, "abc");
        assert_eq!(next(&waiting, None, Some(&slot), Some("abc")), Next::Wait);
        assert_eq!(next(&waiting, Some(&report("before", 200, "ready")), Some(&slot), Some("abc")), Next::Wait, "the report it was pushed on is not the check");
        assert_eq!(next(&waiting, Some(&report("r1", 90, "ready")), Some(&slot), Some("abc")), Next::Wait);
        assert_eq!(next(&waiting, Some(&report("r1", 200, "needs-you")), Some(&slot), Some("abc")), Next::Wait, "a question keeps the push waiting");
        assert_eq!(next(&waiting, Some(&report("r1", 200, "ready")), Some(&slot), Some("abc")), Next::Push);
        assert!(matches!(next(&waiting, Some(&report("r1", 200, "ready")), Some(&slot), Some("abd")), Next::Drop(why) if why.contains("changed its commits") && why.contains("kelpie approve")));
        assert!(matches!(next(&waiting, Some(&report("r1", 200, "ready")), None, Some("abc")), Next::Drop(why) if why.contains("is gone")));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn kelpie_pushes_for_the_recorded_pusher_once_and_drops_an_intent_whose_commits_changed() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let current = fingerprint(&slot.path, &upstream).unwrap();
        let address = format!("billing@{}", crate::repo::name_of(&dir));
        let ready = |id: &str| { let mut message = report(id, 200, "ready"); message.from = address.clone(); vec![message] };
        save(&state, &intent(By::Lead, &dir, &current)).unwrap();
        let mut pushed = Vec::new();
        complete_in(&state, &[], |intent, _| { pushed.push(intent.by); Ok(String::new()) });
        assert!(pushed.is_empty() && load(&state, &address).is_some(), "no report yet: it waits");
        complete_in(&state, &ready("r1"), |intent, _| { pushed.push(intent.by); Ok(String::new()) });
        assert_eq!(pushed, [By::Lead], "pushed as the lead that asked, under its own checks");
        assert!(load(&state, &address).is_none());
        complete_in(&state, &ready("r1"), |intent, _| { pushed.push(intent.by); Ok(String::new()) });
        assert_eq!(pushed.len(), 1, "an intent pushes once");
        save(&state, &intent(By::User, &dir, &current)).unwrap();
        commit(&slot.path, "e", "a fix while testing");
        let notes = complete_in(&state, &ready("r2"), |intent, _| { pushed.push(intent.by); Ok(String::new()) });
        assert_eq!(pushed.len(), 1, "changed commits are never pushed on an old intent");
        assert!(load(&state, &address).is_none() && notes.iter().any(|note| note.contains("changed its commits")), "{notes:?}");
        cleanup(dir, origin, &slot);
    }
}
