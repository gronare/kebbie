use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::messages::{self, Message};
use crate::permit::Caller;
use crate::sh::Result;
use super::approval::covers;
use super::pool::Slot;

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
            Caller::Task(_) | Caller::Agent | Caller::Sandboxed | Caller::LeadSandboxed => None,
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

// What the push waits for: its turn in the repo's line, the task's Claude testing the files the default branch changed under it, resolving the rebase that conflicted, or fixing what turned CI red after the push landed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Work {
    Turn,
    Tests,
    Conflict,
    Ci,
}

impl Work {
    fn key(self) -> &'static str {
        match self {
            Work::Turn => "turn",
            Work::Tests => "tests",
            Work::Conflict => "conflict",
            Work::Ci => "ci",
        }
    }
}

// An authorised push that waits in its repo's line, by `queued`, and for the task's Claude; it goes out for the commits it was recorded for, or for commits the lead approved since. A red CI's push waits outside the line, `queued` None, until its fix is ready.
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
    pub queued: Option<u64>,
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
            "through": through, "at": self.at, "seen": self.seen, "hand": self.hand, "queued": self.queued,
        })
    }

    fn from_json(value: &Value) -> Option<Intent> {
        let by = match value["by"].as_str()? {
            "user" => By::User,
            "lead" => By::Lead,
            _ => return None,
        };
        let work = match value["work"].as_str() {
            Some("turn") => Work::Turn,
            Some("conflict") => Work::Conflict,
            Some("ci") => Work::Ci,
            _ => Work::Tests,
        };
        let through = match &value["through"] {
            Value::Null => Through::All,
            through => Through::First { count: usize::try_from(through["count"].as_u64()?).ok()?, of: usize::try_from(through["of"].as_u64()?).ok()? },
        };
        let at = value["at"].as_u64()?;
        let queued = value["queued"].as_u64().or((work != Work::Ci).then_some(at));
        Some(Intent {
            by,
            work,
            address: value["address"].as_str()?.to_string(),
            repo: PathBuf::from(value["repo"].as_str()?),
            task: value["task"].as_str()?.to_string(),
            fingerprint: value["fingerprint"].as_str().filter(|text| !text.is_empty() || work == Work::Ci)?.to_string(),
            through,
            at,
            seen: value["seen"].as_str().map(str::to_string),
            hand: value["hand"].as_str().map(str::to_string),
            queued,
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

// A push under way, as the board shows it: who asked, what it waits for, since when, how many pushes of its repo go first and the task right in front.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pending {
    pub by: By,
    pub work: Work,
    pub at: u64,
    pub ahead: usize,
    pub after: Option<String>,
}

// Reads only: the board and kelpie status never claim or drop an intent.
pub(crate) fn pending(state: &Path, address: &str) -> Option<Pending> {
    let intent = load(state, address)?;
    let line = line(state, &intent.repo);
    let ahead = line.iter().position(|queued| queued.address == address).unwrap_or(0);
    let after = ahead.checked_sub(1).and_then(|front| line.get(front)).map(|front| front.task.clone());
    Some(Pending { by: intent.by, work: intent.work, at: intent.at, ahead, after })
}

// A repo's pushes in the order they go out: first queued first; a red CI's push waiting for its fix is not in it.
pub(super) fn line(state: &Path, repo: &Path) -> Vec<Intent> {
    let mut line: Vec<Intent> = all(state).into_iter().filter(|intent| intent.repo == repo && intent.queued.is_some()).collect();
    line.sort_by(|one, other| (one.queued, &one.address).cmp(&(other.queued, &other.address)));
    line
}

pub(super) fn repos(state: &Path) -> Vec<PathBuf> {
    let mut repos: Vec<PathBuf> = all(state).into_iter().filter(|intent| intent.queued.is_some()).map(|intent| intent.repo).collect();
    repos.sort();
    repos.dedup();
    repos
}

fn left_file(state: &Path, address: &str) -> PathBuf {
    state.join("push-left").join(address)
}

// Why a push left its line before it went out, for the board's Needs you row until the task is pushed again or ends.
pub(super) fn record_left(state: &Path, address: &str, why: &str) {
    let _ = crate::state::write(&left_file(state, address), why);
}

pub(crate) fn left(state: &Path, address: &str) -> Option<String> {
    std::fs::read_to_string(left_file(state, address)).ok().filter(|why| !why.is_empty())
}

pub(super) fn forget_left(state: &Path, address: &str) {
    let _ = std::fs::remove_file(left_file(state, address));
}

// The pushed commits are on the default branch, so the push waits for a fix whose rules are checked: no commit of the task's is approved by the push itself.
pub(crate) fn after_red_ci(state: &Path, repo: &Path, address: &str, by: By) -> Result<()> {
    let task = address.split('@').next().unwrap_or(address).to_string();
    let log = messages::load();
    let seen = messages::latest_reports(&log).get(address).map(|report| report.id.clone());
    save(state, &Intent { by, work: Work::Ci, address: address.to_string(), repo: repo.to_path_buf(), task, fingerprint: String::new(), through: Through::All, at: messages::now(), seen, hand: last_hand_over(&log, address), queued: None })
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
pub(super) fn answers_hand_over(intent: &Intent, report: &Message, log: &[Message], transcript: Option<&str>) -> bool {
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
pub(super) fn all(state: &Path) -> Vec<Intent> {
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

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::messages::Report;
    use crate::tasks::test_support::*;

    pub(in crate::tasks) fn report(id: &str, at: u64, state: &str) -> Message {
        Message {
            id: id.to_string(), at, from: "billing@app".to_string(), to: crate::status::LEAD_LABEL.to_string(), text: "tests pass".to_string(), held: false,
            report: Some(Report { state: state.to_string(), next: "push".to_string(), head: None }), ended: false, question: None, about: None, kind: None, landed: None, why: None, forced: false, fyi: false,
        }
    }

    pub(in crate::tasks) fn intent(by: By, repo: &Path, fingerprint: &str) -> Intent {
        Intent { by, work: Work::Tests, address: format!("billing@{}", crate::repo::name_of(repo)), repo: repo.to_path_buf(), task: "billing".to_string(), fingerprint: fingerprint.to_string(), through: Through::All, at: 100, seen: Some("before".to_string()), hand: None, queued: Some(100) }
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
        assert_eq!((pending(&state, "billing@app"), pending(&state, "other@app")), (Some(Pending { by: By::Lead, work: Work::Conflict, at: 100, ahead: 0, after: None }), None));
        assert_eq!(load(&state, "billing@app"), Some(written.clone()), "reading what waits leaves the intent in place");
        let mut older = written.to_json();
        older.as_object_mut().unwrap().remove("queued");
        crate::state::write(&file(&state, "billing@app"), older.to_string()).unwrap();
        assert_eq!(load(&state, "billing@app").and_then(|intent| intent.queued), Some(100), "a push under way from before the line holds its place by when it was asked");
        forget(&state, "billing@app");
        assert_eq!(pending(&state, "billing@app"), None);
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

}
