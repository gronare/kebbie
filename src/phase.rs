use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::messages;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Report,
    Question,
    NothingToPush,
    Unreported,
    Stopped,
    Failed,
    Unreadable,
    LeftLine,
    NotStaged,
}

impl Why {
    const ALL: [Why; 9] = [Why::Report, Why::Question, Why::NothingToPush, Why::Unreported, Why::Stopped, Why::Failed, Why::Unreadable, Why::LeftLine, Why::NotStaged];

    fn key(self) -> &'static str {
        match self {
            Why::Report => "report",
            Why::Question => "question",
            Why::NothingToPush => "nothing-to-push",
            Why::Unreported => "unreported",
            Why::Stopped => "stopped",
            Why::Failed => "failed",
            Why::Unreadable => "unreadable",
            Why::LeftLine => "left-line",
            Why::NotStaged => "not-staged",
        }
    }

    fn of_key(key: &str) -> Option<Why> {
        Why::ALL.into_iter().find(|why| why.key() == key)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Working,
    Idle,
    NeedsYou(Why),
    Ready { head: Option<String> },
    Staged { head: Option<String> },
    Pushing { ahead: usize },
    CiRunning { sha: String },
    CiRed { sha: String },
    Closing,
}

impl Phase {
    pub fn key(&self) -> &'static str {
        match self {
            Phase::Starting => "starting",
            Phase::Working => "working",
            Phase::Idle => "idle",
            Phase::NeedsYou(_) => "needs-you",
            Phase::Ready { .. } => "ready",
            Phase::Staged { .. } => "staged",
            Phase::Pushing { .. } => "pushing",
            Phase::CiRunning { .. } => "ci-running",
            Phase::CiRed { .. } => "ci-red",
            Phase::Closing => "closing",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub phase: Phase,
    pub since: u64,
    pub by: String,
}

impl Record {
    pub fn to_json(&self) -> Value {
        let (why, head, sha) = match &self.phase {
            Phase::NeedsYou(why) => (Some(why.key()), None, None),
            Phase::Ready { head } | Phase::Staged { head } => (None, head.as_deref(), None),
            Phase::CiRunning { sha } | Phase::CiRed { sha } => (None, None, Some(sha.as_str())),
            _ => (None, None, None),
        };
        let ahead = match &self.phase {
            Phase::Pushing { ahead } => Some(*ahead),
            _ => None,
        };
        json!({ "name": self.phase.key(), "why": why, "head": head, "sha": sha, "ahead": ahead, "since": self.since, "by": self.by })
    }

    pub fn from_json(value: &Value) -> Option<Record> {
        let text = |key: &str| value[key].as_str().map(str::to_string);
        let phase = match value["name"].as_str()? {
            "starting" => Phase::Starting,
            "working" => Phase::Working,
            "idle" => Phase::Idle,
            "needs-you" => Phase::NeedsYou(Why::of_key(value["why"].as_str()?)?),
            "ready" => Phase::Ready { head: text("head") },
            "staged" => Phase::Staged { head: text("head") },
            "pushing" => Phase::Pushing { ahead: value["ahead"].as_u64().and_then(|ahead| usize::try_from(ahead).ok()).unwrap_or(0) },
            "ci-running" => Phase::CiRunning { sha: text("sha")? },
            "ci-red" => Phase::CiRed { sha: text("sha")? },
            "closing" => Phase::Closing,
            _ => return None,
        };
        Some(Record { phase, since: value["since"].as_u64().unwrap_or_default(), by: text("by").unwrap_or_default() })
    }
}

// What an actor did to a task; `next` turns it into the task's phase, and nothing else moves one.
#[derive(Debug)]
pub enum Event<'a> {
    Started { prompted: bool },
    TurnBegan { by_user: bool },
    TurnEndedUnreported,
    Reported { state: &'a str, head: Option<&'a str>, committed: bool, pushing: bool },
    Asked,
    Answered,
    PushQueued { ahead: usize },
    PushHanded,
    PushOut { sha: &'a str },
    PushFailed,
    Staged { head: Option<&'a str> },
    Unstaged { head: Option<&'a str> },
    StageGaveUp,
    KeptOpen { rest: usize, clean: bool, head: Option<&'a str> },
    Closing,
    CiRed { sha: &'a str, pushing: bool },
    CiUnreadable,
    Stopped,
    Dismissed { kept_open: bool },
}

// A delivered message opens a line with its sender's header and indents every line under it, so no sender's text can open one; a prompt with no header is one the user typed.
pub fn by_user(prompt: &str) -> bool {
    let prompt = prompt.trim_start();
    let prompt = match prompt.strip_prefix("<cross-session-message") {
        Some(wrapped) => wrapped.split_once('>').map_or("", |(_, rest)| rest.trim_start()),
        None => prompt,
    };
    let sender = |line: &str| line.strip_prefix("[from ")?.split_once(']').map(|(sender, _)| sender.to_string());
    prompt.lines().next().and_then(sender).is_none() || prompt.lines().filter_map(sender).any(|sender| sender == messages::USER_LABEL)
}

// None leaves the phase as it is: a turn reading a notice, or a report while CI runs, is no move.
pub fn next(current: Option<&Phase>, event: &Event) -> Option<Phase> {
    Some(match event {
        Event::Started { prompted: true } => Phase::Starting,
        Event::Started { prompted: false } => Phase::Idle,
        Event::TurnBegan { by_user } => match current {
            Some(Phase::Starting) => Phase::Working,
            Some(Phase::Idle | Phase::NeedsYou(_) | Phase::CiRed { .. }) if *by_user => Phase::Working,
            _ => return None,
        },
        Event::TurnEndedUnreported => match current {
            Some(Phase::Starting | Phase::Working) => Phase::NeedsYou(Why::Unreported),
            _ => return None,
        },
        Event::Reported { state, head, committed, pushing } => match (*state, current) {
            ("needs-you", _) => Phase::NeedsYou(Why::Report),
            _ if *pushing => Phase::Pushing { ahead: match current { Some(Phase::Pushing { ahead }) => *ahead, _ => 0 } },
            (_, Some(Phase::Closing)) => return None,
            ("blocked", _) => Phase::Idle,
            (_, Some(Phase::CiRunning { .. })) => return None,
            ("working", _) => Phase::Working,
            ("ready", Some(Phase::Staged { .. })) => return None,
            ("ready", _) if *committed => Phase::Ready { head: head.map(str::to_string) },
            ("ready", _) => Phase::NeedsYou(Why::NothingToPush),
            _ => return None,
        },
        Event::Asked => Phase::NeedsYou(Why::Question),
        Event::Answered => match current {
            Some(Phase::NeedsYou(_)) => Phase::Working,
            _ => return None,
        },
        Event::PushQueued { ahead } => Phase::Pushing { ahead: *ahead },
        Event::PushHanded => Phase::Pushing { ahead: 0 },
        Event::PushOut { sha } => Phase::CiRunning { sha: sha.to_string() },
        Event::PushFailed => Phase::NeedsYou(Why::Failed),
        Event::Staged { head } => Phase::Staged { head: head.map(str::to_string) },
        Event::StageGaveUp => Phase::NeedsYou(Why::NotStaged),
        Event::Unstaged { head } => match current {
            Some(Phase::Staged { .. }) => Phase::Ready { head: head.map(str::to_string) },
            _ => return None,
        },
        Event::KeptOpen { clean: false, .. } => Phase::Working,
        Event::KeptOpen { rest: 0, .. } => Phase::Idle,
        Event::KeptOpen { head, .. } => Phase::Ready { head: head.map(str::to_string) },
        Event::Closing => Phase::Closing,
        Event::CiRed { pushing: true, .. } => Phase::Pushing { ahead: 0 },
        Event::CiRed { sha, .. } => Phase::CiRed { sha: sha.to_string() },
        Event::CiUnreadable => match current {
            Some(Phase::CiRunning { .. }) => Phase::NeedsYou(Why::Unreadable),
            _ => return None,
        },
        Event::Stopped => Phase::NeedsYou(Why::Stopped),
        Event::Dismissed { kept_open: true } => Phase::Idle,
        Event::Dismissed { kept_open: false } => Phase::Closing,
    })
}

fn dir(state: &Path) -> PathBuf {
    state.join("phase")
}

fn file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.json"))
}

pub fn load(state: &Path, address: &str) -> Option<Record> {
    Record::from_json(&serde_json::from_str(&fs::read_to_string(file(state, address)).ok()?).ok()?)
}

fn save(state: &Path, address: &str, record: &Record) {
    let _ = crate::state::write(&file(state, address), record.to_json().to_string());
}

pub fn forget(state: &Path, address: &str) {
    let _ = fs::remove_file(file(state, address));
}

// Every writer reads, decides and writes under it, so the keeper and a board never lose each other's move.
struct Lock(fs::File);

impl Lock {
    fn take(state: &Path) -> Option<Lock> {
        crate::state::private_dir(&dir(state)).ok()?;
        let file = crate::state::append_to(&dir(state).join(".lock")).ok()?;
        // SAFETY: flock on a descriptor this File owns.
        (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0).then_some(Lock(file))
    }
}

impl Drop for Lock {
    // A child forked by another thread holds a copy of the descriptor until it execs; unlocking releases the lock for every copy at once.
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

pub fn update(state: &Path, address: &str, by: &str, change: impl FnOnce(Option<&Phase>) -> Option<Phase>) -> Option<Phase> {
    let _lock = Lock::take(state)?;
    let current = load(state, address);
    let moved = change(current.as_ref().map(|record| &record.phase))?;
    if current.as_ref().is_none_or(|record| record.phase != moved) {
        save(state, address, &Record { phase: moved.clone(), since: messages::now(), by: by.to_string() });
    }
    Some(moved)
}

pub fn record(state: &Path, address: &str, by: &str, event: &Event) -> Option<Phase> {
    update(state, address, by, |current| next(current, event))
}

// A push git refused puts back the phase it had before, unless something moved it since the push wrote `written`.
pub fn put_back(state: &Path, address: &str, written: &Phase, before: Option<Record>) {
    let Some(_lock) = Lock::take(state) else { return };
    if load(state, address).is_none_or(|record| &record.phase != written) {
        return;
    }
    match before {
        Some(record) => save(state, address, &record),
        None => forget(state, address),
    }
}

// The phase of a task whose worktree is gone goes with it, so a later task by that name starts from its own; one written since `began` is a start the caller did not see yet.
pub fn sweep(state: &Path, live: &[String], began: u64) {
    let Ok(entries) = fs::read_dir(dir(state)) else { return };
    let Some(_lock) = Lock::take(state) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(address) = name.strip_suffix(".json").filter(|_| !name.starts_with('.')).filter(|address| !live.iter().any(|live| live == address) && load(state, address).is_none_or(|record| record.since < began)) {
            forget(state, address);
        }
    }
}

// A task from before phases were kept gets one, derived once from what it showed then; a writer that got there first wins.
pub fn current(state: &Path, address: &str, derive: impl FnOnce() -> Phase) -> Record {
    if let Some(record) = load(state, address) {
        return record;
    }
    let derived = Record { phase: derive(), since: messages::now(), by: "migration".to_string() };
    let Some(_lock) = Lock::take(state) else { return derived };
    if let Some(record) = load(state, address) {
        return record;
    }
    save(state, address, &derived);
    derived
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phase_reads_back_as_written_with_when_and_by_whom() {
        let state = crate::sh::test_dir("phase-file");
        for phase in [Phase::Starting, Phase::Working, Phase::Idle, Phase::Ready { head: Some("abc".to_string()) }, Phase::Ready { head: None }, Phase::Staged { head: Some("abc".to_string()) }, Phase::Pushing { ahead: 0 }, Phase::Pushing { ahead: 2 }, Phase::CiRunning { sha: "abc".to_string() }, Phase::CiRed { sha: "def".to_string() }, Phase::Closing]
            .into_iter().chain(Why::ALL.map(Phase::NeedsYou)) {
            let record = Record { phase, since: 7, by: "report".to_string() };
            save(&state, "billing@app", &record);
            assert_eq!(load(&state, "billing@app"), Some(record));
        }
        forget(&state, "billing@app");
        assert_eq!(load(&state, "billing@app"), None);
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_phase_changes_only_by_an_event_and_keeps_its_since_when_the_event_leaves_it() {
        let state = crate::sh::test_dir("phase-record");
        assert_eq!(record(&state, "billing@app", "start", &Event::Started { prompted: true }), Some(Phase::Starting));
        let started = load(&state, "billing@app").unwrap();
        assert_eq!(started.by, "start");
        assert_eq!(record(&state, "billing@app", "turn", &Event::TurnBegan { by_user: false }), Some(Phase::Working), "the prompt kebbie typed for the user starts the work");
        save(&state, "billing@app", &Record { phase: Phase::Ready { head: Some("abc".to_string()) }, since: 5, by: "report".to_string() });
        assert_eq!(record(&state, "billing@app", "turn", &Event::TurnBegan { by_user: true }), None);
        assert_eq!(load(&state, "billing@app").map(|record| (record.since, record.by)), Some((5, "report".to_string())), "a turn on a ready task writes nothing");
        record(&state, "billing@app", "report", &Event::Reported { state: "ready", head: Some("abc"), committed: true, pushing: false });
        assert_eq!(load(&state, "billing@app").map(|record| record.since), Some(5), "the same phase again keeps when it began");
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn migration_writes_the_derived_phase_once_and_never_over_a_writers() {
        let state = crate::sh::test_dir("phase-migrate");
        let derived = current(&state, "billing@app", || Phase::Ready { head: None });
        assert_eq!((derived.phase.clone(), derived.by.as_str()), (Phase::Ready { head: None }, "migration"));
        assert_eq!(current(&state, "billing@app", || panic!("derived again")).phase, Phase::Ready { head: None }, "kept: later reads never derive");
        record(&state, "billing@app", "ci", &Event::PushOut { sha: "abc" });
        assert_eq!(current(&state, "billing@app", || Phase::Idle).phase, Phase::CiRunning { sha: "abc".to_string() });
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn only_the_users_own_prompt_is_the_users() {
        let kebbie = messages::delivered("kebbie", "kebbie was updated\nrestart when you can");
        let lead = messages::delivered("lead", "fix the second commit");
        assert!(by_user("add a refund column") && by_user(&messages::delivered("you", "go on")), "typed in its Space, or sent as the user's answer");
        assert!(!by_user(&kebbie) && !by_user(&lead) && !by_user(&messages::delivered("billing@api", "the field is in")), "kebbie, the lead or another task");
        assert!(!by_user(&messages::delivered("lead", "[from you] go on")), "a sender's own text never opens a line");
        assert!(!by_user(&format!("{lead}\n\n{kebbie}")), "a batch with no word of the user's");
        assert!(by_user(&format!("{lead}\n\n{kebbie}\n\n{}", messages::delivered("you", "go on"))), "a batch the user's word is in");
        assert!(!by_user(&format!("<cross-session-message from=\"kebbie\">{kebbie}</cross-session-message>")), "a wrapper some version may put around the inbox's content");
        assert!(!by_user(&format!("<cross-session-message from=\"kebbie\">\n{kebbie}\n</cross-session-message>")));
        assert!(by_user("Why did this come in?\n[from kebbie] kebbie was updated"), "the user quoting kebbie's line is still the user");
    }

    #[test]
    fn a_report_moves_a_task_except_while_its_push_is_out_or_it_closes() {
        let reported = |current: Phase, state: &str, committed: bool, pushing: bool| next(Some(&current), &Event::Reported { state, head: Some("abc"), committed, pushing });
        let ci = Phase::CiRunning { sha: "abc".to_string() };
        assert_eq!(reported(ci.clone(), "ready", false, false), None);
        assert_eq!(reported(ci.clone(), "working", false, false), None);
        assert_eq!(reported(ci.clone(), "blocked", false, false), Some(Phase::Idle));
        assert_eq!(reported(ci, "needs-you", false, false), Some(Phase::NeedsYou(Why::Report)));
        assert_eq!(reported(Phase::Closing, "ready", false, false), None);
        assert_eq!(reported(Phase::Working, "ready", true, true), Some(Phase::Pushing { ahead: 0 }), "a push waits for it");
        assert_eq!(reported(Phase::Pushing { ahead: 2 }, "ready", true, true), Some(Phase::Pushing { ahead: 2 }), "a report keeps its place in the push line");
        assert_eq!(reported(Phase::Pushing { ahead: 0 }, "needs-you", true, true), Some(Phase::NeedsYou(Why::Report)));
        assert_eq!(reported(Phase::Working, "ready", true, false), Some(Phase::Ready { head: Some("abc".to_string()) }));
        assert_eq!(reported(Phase::Working, "ready", false, false), Some(Phase::NeedsYou(Why::NothingToPush)));
        assert_eq!(reported(Phase::CiRed { sha: "abc".to_string() }, "ready", true, false), Some(Phase::Ready { head: Some("abc".to_string()) }), "the fix, after the red run");
        assert_eq!(next(Some(&Phase::Ready { head: None }), &Event::TurnEndedUnreported), None, "a turn on a ready task owes no report of its own");
        assert_eq!(next(Some(&Phase::Working), &Event::TurnEndedUnreported), Some(Phase::NeedsYou(Why::Unreported)));
        assert_eq!(next(Some(&Phase::NeedsYou(Why::Question)), &Event::Answered), Some(Phase::Working));
        assert_eq!(next(Some(&Phase::Ready { head: None }), &Event::Answered), None);
        let staged = Phase::Staged { head: Some("abc".to_string()) };
        assert_eq!(reported(staged.clone(), "ready", true, false), None, "a ready report leaves it on local main");
        assert_eq!(reported(staged.clone(), "working", true, false), Some(Phase::Working));
        assert_eq!(next(Some(&staged), &Event::TurnBegan { by_user: true }), None);
        assert_eq!(next(Some(&staged), &Event::Unstaged { head: Some("def") }), Some(Phase::Ready { head: Some("def".to_string()) }));
        assert_eq!(next(Some(&Phase::Working), &Event::Unstaged { head: None }), None, "taken off main while it works, it keeps working");
    }

    #[test]
    fn every_actor_that_moves_a_task_writes_its_phase() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let body = |file: &str, name: &str| {
            let source = std::fs::read_to_string(root.join(file)).unwrap();
            let start = [format!("\nfn {name}("), format!("\npub fn {name}("), format!("\npub(super) fn {name}("), format!("\npub(crate) fn {name}(")].iter().find_map(|head| source.find(head.as_str())).unwrap_or_else(|| panic!("{file}: {name}"));
            source[start..start + source[start..].find("\n}\n").unwrap()].to_string()
        };
        for (file, name, event) in [
            ("questions.rs", "answer", "Event::Answered"), ("tasks/push.rs", "prepare", "Event::PushHanded"), ("tasks/push.rs", "deliver", "Event::KeptOpen"), ("tasks/launch.rs", "restart", "Event::Started"), ("tasks/push.rs", "after_ci", "Event::Closing"),
            ("tasks/line.rs", "turn", "Event::PushFailed"), ("tasks/push.rs", "join", "Event::PushQueued"), ("tasks/line.rs", "leave", "Why::LeftLine"), ("recover.rs", "apply", "Event::Stopped"), ("tasks/red_ci.rs", "dismiss_as", "Event::Dismissed"), ("tasks/finish.rs", "forget_address", "phase::forget("), ("tasks/stage.rs", "stage_in", "Event::Staged"), ("tasks/stage.rs", "unstage_in", "Event::Unstaged"),
        ] {
            assert!(body(file, name).contains(event), "{file} {name} writes {event}");
        }
        let deliver = body("tasks/push.rs", "deliver");
        assert!(deliver.find("Event::PushOut").unwrap() < deliver.find("push_prefix(").unwrap(), "CI running is written before the push, so the push never shows anywhere else");
    }

    #[test]
    fn a_refused_push_puts_back_the_phase_only_while_nothing_moved_it_since() {
        let state = crate::sh::test_dir("phase-put-back");
        let ci = Phase::CiRunning { sha: "abc".to_string() };
        let before = Record { phase: Phase::Ready { head: Some("abc".to_string()) }, since: 3, by: "report".to_string() };
        record(&state, "billing@app", "push", &Event::PushOut { sha: "abc" });
        put_back(&state, "billing@app", &ci, Some(before.clone()));
        assert_eq!(load(&state, "billing@app"), Some(before));
        record(&state, "billing@app", "push", &Event::PushOut { sha: "abc" });
        record(&state, "billing@app", "report", &Event::Reported { state: "needs-you", head: None, committed: true, pushing: false });
        put_back(&state, "billing@app", &ci, None);
        assert_eq!(load(&state, "billing@app").map(|record| record.phase), Some(Phase::NeedsYou(Why::Report)), "the report during the push stands");
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_phase_of_a_task_whose_worktree_is_gone_is_swept() {
        let state = crate::sh::test_dir("phase-sweep");
        record(&state, "billing@app", "start", &Event::Started { prompted: true });
        record(&state, "search@app", "start", &Event::Started { prompted: false });
        save(&state, "search@app", &Record { phase: Phase::Idle, since: 10, by: "start".to_string() });
        save(&state, "rates@app", &Record { phase: Phase::Starting, since: 20, by: "start".to_string() });
        sweep(&state, &["billing@app".to_string()], 20);
        assert_eq!((load(&state, "search@app"), load(&state, "rates@app").map(|record| record.phase)), (None, Some(Phase::Starting)), "a start since the slots were read stays");
        assert!(load(&state, "billing@app").is_some());
        assert!(dir(&state).join(".lock").exists(), "the lock stays");
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn writers_in_parallel_never_lose_a_move() {
        let state = crate::sh::test_dir("phase-race");
        let count = |phase: Option<&Phase>| match phase {
            Some(Phase::Ready { head: Some(head) }) => head.parse::<usize>().unwrap(),
            _ => 0,
        };
        let writers: Vec<_> = (0..8).map(|writer| {
            let state = state.clone();
            std::thread::spawn(move || for _ in 0..40 {
                update(&state, "billing@app", if writer % 2 == 0 { "keeper" } else { "board" }, |phase| Some(Phase::Ready { head: Some((count(phase) + 1).to_string()) }));
            })
        }).collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(count(load(&state, "billing@app").as_ref().map(|record| &record.phase)), 8 * 40);
        std::fs::remove_dir_all(state).unwrap();
    }
}
