use std::os::fd::AsRawFd;
use std::path::Path;

use crate::messages::{self, Message};
use crate::phase::{self, Event, Phase, Why};
use crate::repo::{default_branch, name_of};
use crate::sh::Result;
use super::approval::{approval_holds, covers, fingerprint};
use super::intent::{self, Intent, Next, Work};
use super::pool::{Slot, find_task};
use super::push::{Sent, pushed_fingerprint, rejected};

// How long the head of a line waits for its task's Claude before the pushes behind it go on without it.
pub(crate) const CEILING: u64 = 30 * 60;

pub(super) type Read<'a> = &'a dyn Fn(&Intent, &Slot) -> Option<String>;
pub(super) type Push<'a> = &'a mut dyn FnMut(&Intent, &Slot) -> Result<Sent>;

// Held while one driver works a repo's line, so the keeper, a board and kebbie push never send two pushes of one repo at once.
pub(super) struct Turnstile(std::fs::File, std::path::PathBuf);

thread_local! {
    static HELD: std::cell::RefCell<Vec<std::path::PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Turnstile {
    fn lock_of(state: &Path, repo: &Path) -> std::path::PathBuf {
        state.join("push-line").join(format!("{}.lock", name_of(repo)))
    }

    pub(super) fn try_take(state: &Path, repo: &Path) -> Option<Turnstile> {
        let lock = Self::lock_of(state, repo);
        let file = crate::state::append_to(&lock).ok()?;
        // SAFETY: flock on a descriptor this File owns.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return None;
        }
        HELD.with(|held| held.borrow_mut().push(lock.clone()));
        Some(Turnstile(file, lock))
    }

    // Whether this thread already holds the repo's turnstile, as a push turn does while it closes its task.
    pub(super) fn held_here(state: &Path, repo: &Path) -> bool {
        let lock = Self::lock_of(state, repo);
        HELD.with(|held| held.borrow().contains(&lock))
    }
}

impl Drop for Turnstile {
    // A child forked by another thread holds a copy of the descriptor until it execs; unlocking releases the lock for every copy at once.
    fn drop(&mut self) {
        HELD.with(|held| held.borrow_mut().retain(|lock| *lock != self.1));
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[derive(Debug, Default)]
pub(super) struct Driven {
    pub notes: Vec<String>,
    pub mine: Option<Result<String>>,
}

#[derive(Debug, PartialEq)]
enum Turn {
    Waits,
    Handed(String),
    Retry(String),
    Pushed(String),
    Out(String),
}

pub fn complete(state: &Path) -> Vec<String> {
    let read = live_read();
    let mut push = super::push::live_push(state);
    complete_in(state, &messages::load(), messages::now(), &read, &mut push)
}

pub(super) fn live_read() -> impl Fn(&Intent, &Slot) -> Option<String> {
    let sessions = crate::peers::sessions().unwrap_or_default();
    move |intent: &Intent, slot: &Slot| {
        let pane = sessions.iter().find(|session| session.address == intent.address).and_then(|session| session.pane.clone());
        let file = pane.and_then(|pane| crate::typing::transcript_of(&pane)).or_else(|| super::launch::transcript(&intent.repo, slot).map(|(_, file)| file))?;
        std::fs::read_to_string(file).ok()
    }
}

pub(super) fn complete_in(state: &Path, log: &[Message], now: u64, read: Read, push: Push) -> Vec<String> {
    let mut notes = fixes(state, log, now, read);
    for repo in intent::repos(state) {
        notes.extend(drive(state, &repo, None, log, now, read, push).notes);
    }
    notes
}

// A red CI's push waits outside the line for its fix; a fix the lead approved joins the line at its end.
fn fixes(state: &Path, log: &[Message], now: u64, read: Read) -> Vec<String> {
    let reports = messages::latest_reports(log);
    let mut notes = Vec::new();
    for waiting in intent::all(state).into_iter().filter(|waiting| waiting.queued.is_none()) {
        let slot = find_task(&waiting.repo, Some(&waiting.task), &waiting.repo).ok();
        let current = slot.as_ref().and_then(|slot| default_branch(&waiting.repo).and_then(|upstream| fingerprint(&slot.path, &upstream)).ok());
        let approved = current.as_deref().is_some_and(|current| approval_holds(state, &waiting.address, current));
        let report = reports.get(&waiting.address).filter(|report| slot.as_ref().is_none_or(|slot| intent::answers_hand_over(&waiting, report, log, read(&waiting, slot).as_deref())));
        match intent::next(&waiting, report, slot.as_ref(), current.as_deref(), approved) {
            Next::Wait => {}
            Next::Drop(why) => {
                intent::forget(state, &waiting.address);
                phase::forget(state, &waiting.address);
                tell_lead(&format!("{why}."));
                notes.push(format!("push: {why}"));
            }
            Next::Push => {
                let joined = Intent { work: Work::Turn, fingerprint: current.unwrap_or_default(), at: now, queued: Some(now), ..waiting.clone() };
                if intent::save(state, &joined).is_ok() {
                    notes.push(format!("push: {}: its fix joined the push line", waiting.address));
                }
            }
        }
    }
    notes
}

// Works the head of a repo's line until one waits; with `mine`, only while that push is the head, and the outcome of its turn comes back.
pub(super) fn drive(state: &Path, repo: &Path, mine: Option<&str>, log: &[Message], now: u64, read: Read, push: Push) -> Driven {
    let mut driven = Driven::default();
    let Some(_turnstile) = Turnstile::try_take(state, repo) else { return driven };
    let mut seen = Vec::new();
    while let Some(head) = intent::line(state, repo).into_iter().next() {
        let ours = mine == Some(head.address.as_str());
        if (mine.is_some() && !ours) || seen.contains(&head.address) {
            break;
        }
        seen.push(head.address.clone());
        let turn = turn(state, &head, log, now, read, push);
        let goes_on = matches!(turn, Turn::Pushed(_) | Turn::Out(_));
        let (note, said) = match turn {
            Turn::Waits => break,
            Turn::Handed(said) | Turn::Retry(said) | Turn::Pushed(said) => (said.clone(), Ok(said)),
            Turn::Out(why) => (why.clone(), Err(why)),
        };
        driven.notes.push(format!("push: {}: {note}", head.address));
        if ours {
            driven.mine = Some(said);
            break;
        }
        if !goes_on {
            break;
        }
    }
    restamp(state, repo);
    driven
}

fn turn(state: &Path, head: &Intent, log: &[Message], now: u64, read: Read, push: Push) -> Turn {
    let Ok(slot) = find_task(&head.repo, Some(&head.task), &head.repo) else {
        let why = format!("{} is gone, so kebbie dropped the push {} asked for", head.address, head.by.word());
        intent::forget(state, &head.address);
        phase::forget(state, &head.address);
        tell_lead(&format!("{why}."));
        return Turn::Out(why);
    };
    if crate::repo::rebase_in_progress(&slot.path) {
        if now.saturating_sub(head.at) >= CEILING {
            let whose = if head.work == Work::Turn { "a rebase in its worktree" } else { "its Claude's rebase" };
            return leave(state, head, &format!("{whose} did not finish within {} minutes, so the pushes behind it went on", CEILING / 60));
        }
        return Turn::Waits;
    }
    let upstream = default_branch(&head.repo);
    let approved = upstream.as_ref().ok().and_then(|upstream| fingerprint(&slot.path, upstream).ok()).is_some_and(|whole| approval_holds(state, &head.address, &whole));
    let current = upstream.as_ref().ok().and_then(|upstream| pushed_fingerprint(&slot.path, upstream, head.through).ok());
    if head.work == Work::Turn {
        if !approved && !current.as_deref().is_some_and(|current| covers(&head.fingerprint, current)) {
            return leave(state, head, &format!("its commits changed after {} chose push; push it again once they are checked", head.by.word()));
        }
    } else {
        let reports = messages::latest_reports(log);
        let report = reports.get(&head.address).filter(|report| intent::answers_hand_over(head, report, log, read(head, &slot).as_deref()));
        let since = |state: &str| report.is_some_and(|report| report.at > head.at && head.seen.as_deref() != Some(report.id.as_str()) && report.report.as_ref().is_some_and(|report| report.state == state));
        if since("needs-you") {
            return leave(state, head, "its Claude reported needs-you instead of ready");
        }
        match intent::next(head, report, Some(&slot), current.as_deref(), approved) {
            Next::Push => {}
            Next::Wait if now.saturating_sub(head.at) >= CEILING => {
                let why = if since("ready") { "the lead did not approve its new commits" } else { "its Claude did not report ready" };
                return leave(state, head, &format!("{why} within {} minutes, so the pushes behind it went on", CEILING / 60));
            }
            Next::Wait => return Turn::Waits,
            Next::Drop(why) => return Turn::Out(why),
        }
    }
    match push(head, &slot) {
        Ok(Sent::Pushed(said)) => {
            intent::forget(state, &head.address);
            Turn::Pushed(said)
        }
        Ok(Sent::Handed(said)) => Turn::Handed(said),
        Err(error) if rejected(&error) => {
            let checked = default_branch(&head.repo).and_then(|upstream| pushed_fingerprint(&slot.path, &upstream, head.through)).unwrap_or_else(|_| head.fingerprint.clone());
            let _ = intent::save(state, &Intent { work: Work::Turn, fingerprint: checked, at: now, ..head.clone() });
            Turn::Retry(format!("the push {} asked for was rejected while the default branch kept moving; it stays at the head of the line and kebbie tries again next round", head.by.word()))
        }
        Err(error) => {
            intent::forget(state, &head.address);
            phase::record(state, &head.address, "push", &Event::PushFailed);
            let text = format!("kebbie could not finish the push of {} that {} asked for: {error}", head.address, head.by.word());
            tell_lead(&text);
            Turn::Out(text)
        }
    }
}

// The push leaves its line for good; a task already under Needs you keeps its own reason there.
fn leave(state: &Path, head: &Intent, why: &str) -> Turn {
    intent::forget(state, &head.address);
    intent::record_left(state, &head.address, why);
    phase::update(state, &head.address, "push", |current| (!matches!(current, Some(Phase::NeedsYou(_)))).then_some(Phase::NeedsYou(Why::LeftLine)));
    tell_lead(&format!("kebbie took {} out of the push line, unpushed: {why}. It waits for the user under Needs you; a push from there puts it at the end of the line.", head.address));
    Turn::Out(format!("out of the push line: {why}"))
}

pub(super) fn restamp(state: &Path, repo: &Path) {
    for (ahead, queued) in intent::line(state, repo).iter().enumerate() {
        phase::update(state, &queued.address, "push", |current| matches!(current, Some(Phase::Pushing { .. })).then_some(Phase::Pushing { ahead }));
    }
}

fn tell_lead(text: &str) {
    if let Err(error) = messages::to_lead(messages::ToLead::PushFailed, text) {
        eprintln!("Could not tell the lead: {error}");
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::permit::Caller;
    use crate::repo::git;
    use crate::tasks::intent::{By, Through, forget, load, save};
    use crate::tasks::intent::tests::{intent, report};
    use crate::tasks::test_support::*;

    fn pushed(_: &Intent, _: &Slot) -> Result<Sent> {
        Ok(Sent::Pushed(String::new()))
    }

    #[test]
    fn a_waiting_push_completes_only_on_a_ready_report_written_after_its_claude_read_the_hand_over() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let current = fingerprint(&slot.path, &upstream).unwrap();
        let address = format!("billing@{}", crate::repo::name_of(&dir));
        let hand = |held: bool| { let mut message = report("h1", 150, "ready"); (message.from, message.to, message.text, message.report, message.held, message.kind) = ("kebbie".to_string(), address.clone(), "run the tests".to_string(), None, held, Some(messages::HAND_KIND.to_string())); message };
        let ready = |at: u64| { let mut message = report("r1", at, "ready"); message.from = address.clone(); message };
        let read_at = |seconds: u64| json!({ "type": "user", "timestamp": format!("1970-01-01T00:{:02}:{:02}.000Z", seconds / 60, seconds % 60), "message": { "content": "[from kebbie] run the tests" } }).to_string();
        let pushes = |log: Vec<Message>, transcript: Option<String>| {
            save(&state, &Intent { hand: Some("h1".to_string()), ..intent(By::User, &dir, &current) }).unwrap();
            let mut pushed = false;
            complete_in(&state, &log, 300, &|_, _| transcript.clone(), &mut |_, _| { pushed = true; Ok(Sent::Pushed(String::new())) });
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
    fn kebbie_pushes_for_the_recorded_pusher_once_and_changed_commits_once_the_lead_approves_them() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let current = fingerprint(&slot.path, &upstream).unwrap();
        let address = format!("billing@{}", crate::repo::name_of(&dir));
        let ready = |id: &str| { let mut message = report(id, 200, "ready"); message.from = address.clone(); vec![message] };
        save(&state, &intent(By::Lead, &dir, &current)).unwrap();
        let mut pushed = Vec::new();
        complete_in(&state, &[], 300, &|_, _| None, &mut |intent, _| { pushed.push(intent.by); Ok(Sent::Pushed(String::new())) });
        assert!(pushed.is_empty() && load(&state, &address).is_some(), "no report yet: it waits");
        complete_in(&state, &ready("r1"), 300, &|_, _| None, &mut |intent, _| { pushed.push(intent.by); Ok(Sent::Pushed(String::new())) });
        assert_eq!(pushed, [By::Lead], "pushed as the lead that asked, under its own checks");
        assert!(load(&state, &address).is_none());
        complete_in(&state, &ready("r1"), 300, &|_, _| None, &mut |intent, _| { pushed.push(intent.by); Ok(Sent::Pushed(String::new())) });
        assert_eq!(pushed.len(), 1, "an intent pushes once");
        save(&state, &intent(By::User, &dir, &current)).unwrap();
        commit(&slot.path, "e", "a fix while testing");
        complete_in(&state, &ready("r2"), 300, &|_, _| None, &mut |intent, _| { pushed.push(intent.by); Ok(Sent::Pushed(String::new())) });
        assert_eq!(pushed.len(), 1, "changed commits are never pushed on the old commits' word");
        assert!(load(&state, &address).is_some(), "the push stays under way while the fix is checked");
        crate::tasks::approval::record_approval(&state, &address, &fingerprint(&slot.path, &upstream).unwrap()).unwrap();
        complete_in(&state, &ready("r2"), 300, &|_, _| None, &mut |intent, _| { pushed.push(intent.by); Ok(Sent::Pushed(String::new())) });
        assert_eq!(pushed, [By::Lead, By::User], "the lead's approval of the fix lets the user's push go out");
        assert!(load(&state, &address).is_none());
        let current = fingerprint(&slot.path, &upstream).unwrap();
        save(&state, &intent(By::User, &dir, &current)).unwrap();
        let rejected = "Pushing billing failed: ! [rejected]        4333c9f -> main (non-fast-forward)";
        let notes = complete_in(&state, &ready("r3"), 300, &|_, _| None, &mut |_, _| Err(rejected.to_string()));
        assert!(load(&state, &address).is_some_and(|waiting| waiting.work == Work::Turn) && notes[0].contains("tries again"), "a push main kept outrunning keeps the head for the next round, never dropped: {notes:?}");
        complete_in(&state, &ready("r3"), 300, &|_, _| None, &mut |_, _| Err("the reviewer has not passed it".to_string()));
        assert!(load(&state, &address).is_none(), "any other failure goes to the lead");
        assert_eq!(phase::load(&state, &address).map(|record| record.phase), Some(Phase::NeedsYou(Why::Failed)));
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
        let notes = complete_in(&state, &[], 300, &|_, _| None, &mut |_, _| { pushed += 1; Ok(Sent::Pushed(String::new())) });
        assert!(notes.is_empty() && load(&state, &address).is_some(), "the push waits through the rebase: {notes:?}");
        git(&slot.path, &["rebase", "--continue"]).unwrap();
        let mut ready = report("r1", 200, "ready");
        ready.from = address.clone();
        complete_in(&state, &[ready], 300, &|_, _| None, &mut |_, _| { pushed += 1; Ok(Sent::Pushed(String::new())) });
        assert_eq!(pushed, 1, "the task's ready report sends the push");
        assert!(load(&state, &address).is_none());
        cleanup(dir, origin, &slot);
    }

    struct Line {
        state: std::path::PathBuf,
        addresses: Vec<String>,
    }

    impl Drop for Line {
        fn drop(&mut self) {
            for address in &self.addresses {
                forget(&self.state, address);
                intent::forget_left(&self.state, address);
                phase::forget(&self.state, address);
            }
        }
    }

    type Typed = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    // The push a turn really makes, onto the temp origin, with what kebbie hands the task's Claude written down instead of typed.
    fn real_push(state: &Path) -> (Typed, impl FnMut(&Intent, &Slot) -> Result<Sent> + '_) {
        let typed: Typed = Default::default();
        let kept = typed.clone();
        (typed, move |intent: &Intent, slot: &Slot| {
            let hand = |_: &Slot, text: &str| { kept.borrow_mut().push(text.to_string()); Ok(()) };
            let upstream = default_branch(&intent.repo)?;
            super::super::push::push_task_with(state, &intent.by.caller(), &intent.repo, slot, intent.through, intent.queued, &hand, &mut |_, out| {
                super::super::push::push_prefix(&slot.path, &upstream, &out.sha, state).map(|count| format!("pushed {count}"))
            })
        })
    }

    fn head(slot: &Slot) -> String {
        git(&slot.path, &["rev-parse", "HEAD"]).unwrap()
    }

    fn landed(origin: &Path, upstream: &str) -> String {
        git(origin, &["log", "--format=%s", upstream.trim_start_matches("origin/")]).unwrap()
    }

    fn phase_of(state: &Path, address: &str) -> Option<Phase> {
        phase::load(state, address).map(|record| record.phase)
    }

    fn drop_worktree(dir: &Path, slot: &Slot) {
        git(dir, &["worktree", "remove", "--force", &slot.path.to_string_lossy()]).unwrap();
    }

    #[test]
    fn a_repos_pushes_go_out_one_at_a_time_in_the_order_asked_and_the_next_is_rebased_only_when_its_turn_comes() {
        let (dir, origin, alpha, upstream) = task_with_a_commit("alpha");
        let beta = another_task(&dir, "beta", "e");
        let state = crate::lead::state_dir().unwrap();
        let (first, second) = (crate::peers::address("alpha", &name_of(&dir)), crate::peers::address("beta", &name_of(&dir)));
        let _line = Line { state: state.clone(), addresses: vec![first.clone(), second.clone()] };
        super::super::push::join(&state, &Caller::User, &dir, &alpha, Through::All).unwrap();
        super::super::push::join(&state, &Caller::User, &dir, &beta, Through::All).unwrap();
        assert_eq!(intent::line(&state, &dir).iter().map(|queued| queued.task.as_str()).collect::<Vec<_>>(), ["alpha", "beta"]);
        let waiting = intent::pending(&state, &second).unwrap();
        assert_eq!((waiting.ahead, waiting.after.as_deref()), (1, Some("alpha")), "the board says 2nd in line after alpha");
        assert_eq!(phase_of(&state, &second), Some(Phase::Pushing { ahead: 1 }));
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        let before = head(&beta);
        let (typed, mut push) = real_push(&state);
        let driven = drive(&state, &dir, Some(&second), &[], 1000, &|_, _| None, &mut push);
        assert!(driven.mine.is_none() && !landed(&origin, &upstream).contains("fourth"), "kebbie push of beta leaves alpha's turn to the keeper and says where beta stands: {driven:?}");
        let driven = drive(&state, &dir, Some(&first), &[], 1000, &|_, _| None, &mut push);
        assert!(matches!(&driven.mine, Some(Ok(said)) if said == "pushed 1"), "kebbie push pushes its own task at the head of the line: {driven:?}");
        assert!(landed(&origin, &upstream).starts_with("fourth\nsomeone else's work\n"));
        assert_eq!(head(&beta), before, "beta is never rebased before its turn");
        assert_eq!(phase_of(&state, &second), Some(Phase::Pushing { ahead: 0 }), "beta moved up");
        let driven = drive(&state, &dir, None, &[], 1000, &|_, _| None, &mut push);
        assert_eq!(driven.notes, [format!("push: {second}: pushed 1")]);
        assert!(landed(&origin, &upstream).starts_with("beta\nfourth\nsomeone else's work\n"), "beta went out on top of alpha: {}", landed(&origin, &upstream));
        assert!(intent::line(&state, &dir).is_empty() && typed.borrow().is_empty());
        drop_worktree(&dir, &beta);
        cleanup(dir, origin, &alpha);
    }

    #[test]
    fn a_conflict_hands_the_head_to_its_claude_and_the_line_waits_behind_it() {
        let (dir, origin, alpha, upstream) = task_with_a_commit("alpha");
        let beta = another_task(&dir, "beta", "e");
        let state = crate::lead::state_dir().unwrap();
        let (first, second) = (crate::peers::address("alpha", &name_of(&dir)), crate::peers::address("beta", &name_of(&dir)));
        let _line = Line { state: state.clone(), addresses: vec![first.clone(), second.clone()] };
        upstream_commit(&dir, "d", "theirs", "d on main");
        super::super::push::join(&state, &Caller::User, &dir, &alpha, Through::All).unwrap();
        super::super::push::join(&state, &Caller::User, &dir, &beta, Through::All).unwrap();
        let before = head(&beta);
        let (typed, mut push) = real_push(&state);
        drive(&state, &dir, None, &[], 1000, &|_, _| None, &mut push);
        assert_eq!(typed.borrow().len(), 1, "alpha's Claude resolves the conflict");
        let waiting = load(&state, &first).unwrap();
        assert_eq!((waiting.work, intent::line(&state, &dir)[0].address.as_str()), (Work::Conflict, first.as_str()), "alpha keeps the head while its Claude works");
        let driven = drive(&state, &dir, None, &[], 1000 + CEILING - 1, &|_, _| None, &mut push);
        assert!(driven.notes.is_empty() && head(&beta) == before && typed.borrow().len() == 1, "beta waits, untouched: {driven:?}");
        assert!(!landed(&origin, &upstream).contains("beta"));
        drop_worktree(&dir, &beta);
        cleanup(dir, origin, &alpha);
    }

    #[test]
    fn a_head_its_claude_leaves_waiting_past_the_ceiling_goes_to_needs_you_and_the_next_push_goes_out() {
        let (dir, origin, alpha, upstream) = task_with_a_commit("alpha");
        let beta = another_task(&dir, "beta", "e");
        let state = crate::lead::state_dir().unwrap();
        let (first, second) = (crate::peers::address("alpha", &name_of(&dir)), crate::peers::address("beta", &name_of(&dir)));
        let _line = Line { state: state.clone(), addresses: vec![first.clone(), second.clone()] };
        upstream_commit(&dir, "d", "theirs", "d on main");
        super::super::push::join(&state, &Caller::User, &dir, &alpha, Through::All).unwrap();
        super::super::push::join(&state, &Caller::User, &dir, &beta, Through::All).unwrap();
        let (_, mut push) = real_push(&state);
        drive(&state, &dir, None, &[], 1000, &|_, _| None, &mut push);
        let handed = load(&state, &first).unwrap().at;
        let driven = drive(&state, &dir, None, &[], handed + CEILING, &|_, _| None, &mut push);
        assert!(driven.notes[0].contains("its Claude did not report ready within 30 minutes"), "{driven:?}");
        assert!(load(&state, &first).is_none() && phase_of(&state, &first) == Some(Phase::NeedsYou(Why::LeftLine)), "alpha is under Needs you, out of the line");
        assert!(intent::left(&state, &first).is_some_and(|why| why.contains("did not report ready")), "the row says why");
        assert!(landed(&origin, &upstream).starts_with("beta\nd on main\n"), "beta went out without waiting for alpha: {}", landed(&origin, &upstream));
        let mut needs = report("r1", handed + 1, "needs-you");
        needs.from = first.clone();
        super::super::push::join(&state, &Caller::User, &dir, &alpha, Through::All).unwrap();
        assert!(intent::left(&state, &first).is_none(), "pushed again, it is back in the line and the old reason goes");
        drive(&state, &dir, None, &[], 2000, &|_, _| None, &mut push);
        let handed = load(&state, &first).unwrap();
        needs.at = handed.at + 1;
        let driven = drive(&state, &dir, None, &[needs], handed.at + 2, &|_, _| None, &mut push);
        assert!(driven.notes[0].contains("reported needs-you") && load(&state, &first).is_none(), "a red test leaves the line at once: {driven:?}");
        drop_worktree(&dir, &beta);
        cleanup(dir, origin, &alpha);
    }

    #[test]
    fn a_push_leaves_its_line_when_taken_back_dropped_or_its_commits_change_and_the_next_moves_up() {
        let (dir, origin, alpha, upstream) = task_with_a_commit("alpha");
        let beta = another_task(&dir, "beta", "e");
        let gamma = another_task(&dir, "gamma", "f");
        let state = crate::lead::state_dir().unwrap();
        let address = |task: &str| crate::peers::address(task, &name_of(&dir));
        let _line = Line { state: state.clone(), addresses: vec![address("alpha"), address("beta"), address("gamma")] };
        for slot in [&alpha, &beta, &gamma] {
            super::super::push::join(&state, &Caller::User, &dir, slot, Through::All).unwrap();
        }
        let said = super::super::push::cancel(&alpha.path, None).unwrap();
        assert!(said.contains("Taken out of the push line"), "{said}");
        assert_eq!(phase_of(&state, &address("alpha")), Some(Phase::Ready { head: Some(head(&alpha)) }), "back under Ready to push, unpushed");
        assert_eq!(phase_of(&state, &address("beta")), Some(Phase::Pushing { ahead: 0 }), "beta moved up");
        super::super::finish::forget_address(&state, &address("beta"));
        assert_eq!(intent::line(&state, &dir).iter().map(|queued| queued.task.as_str()).collect::<Vec<_>>(), ["gamma"], "a dropped, reset or done task leaves the line");
        assert_eq!(phase_of(&state, &address("gamma")), Some(Phase::Pushing { ahead: 0 }), "gamma moved up when beta ended");
        commit(&gamma.path, "g", "after push was chosen");
        let (_, mut push) = real_push(&state);
        let driven = drive(&state, &dir, None, &[], 1000, &|_, _| None, &mut push);
        assert!(driven.notes[0].contains("its commits changed after the user chose push"), "{driven:?}");
        assert_eq!(phase_of(&state, &address("gamma")), Some(Phase::NeedsYou(Why::LeftLine)));
        assert!(!landed(&origin, &upstream).contains("gamma"), "never pushed unseen commits");
        drop_worktree(&dir, &beta);
        drop_worktree(&dir, &gamma);
        cleanup(dir, origin, &alpha);
    }

    #[test]
    fn a_head_that_is_rejected_or_handed_over_keeps_its_place_before_one_asked_for_later() {
        let (dir, origin, alpha, upstream) = task_with_a_commit("alpha");
        let beta = another_task(&dir, "beta", "e");
        let state = crate::lead::state_dir().unwrap();
        let (first, second) = (crate::peers::address("alpha", &name_of(&dir)), crate::peers::address("beta", &name_of(&dir)));
        let _line = Line { state: state.clone(), addresses: vec![first.clone(), second.clone()] };
        super::super::push::join(&state, &Caller::User, &dir, &alpha, Through::All).unwrap();
        super::super::push::join(&state, &Caller::User, &dir, &beta, Through::All).unwrap();
        let rejected = "! [rejected] main (non-fast-forward)";
        let later = messages::now() + 100;
        drive(&state, &dir, None, &[], later, &|_, _| None, &mut |_, _| Err(rejected.to_string()));
        assert!(load(&state, &first).unwrap().at == later, "the retry is recorded as new");
        assert_eq!((intent::line(&state, &dir)[0].address.as_str(), intent::pending(&state, &second).unwrap().ahead), (first.as_str(), 1), "rejected, alpha still goes first");
        upstream_commit(&dir, "d", "theirs", "d on main");
        let (typed, mut push) = real_push(&state);
        drive(&state, &dir, None, &[], later, &|_, _| None, &mut push);
        assert_eq!(typed.borrow().len(), 1);
        assert_eq!((intent::line(&state, &dir)[0].address.as_str(), intent::pending(&state, &second).unwrap().ahead), (first.as_str(), 1), "handed over, alpha still goes first");
        let said = super::super::push::queued_notice(&state, &second, "beta").unwrap();
        assert!(said.contains("2nd in line after alpha, which waits for its Claude to resolve a conflict"), "{said}");
        assert!(!landed(&origin, &upstream).contains("beta"));
        drop_worktree(&dir, &beta);
        cleanup(dir, origin, &alpha);
    }

    #[test]
    fn a_head_found_mid_rebase_waits_untouched_up_to_the_ceiling_and_a_task_mid_rebase_cannot_join() {
        let (dir, origin, alpha, upstream) = task_with_a_commit("alpha");
        let beta = another_task(&dir, "beta", "e");
        let state = crate::lead::state_dir().unwrap();
        let (first, second) = (crate::peers::address("alpha", &name_of(&dir)), crate::peers::address("beta", &name_of(&dir)));
        let _line = Line { state: state.clone(), addresses: vec![first.clone(), second.clone()] };
        super::super::push::join(&state, &Caller::User, &dir, &alpha, Through::All).unwrap();
        super::super::push::join(&state, &Caller::User, &dir, &beta, Through::All).unwrap();
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        assert!(git(&alpha.path, &["rebase", "--exec", "false", &upstream]).is_err());
        let (typed, mut push) = real_push(&state);
        let at = load(&state, &first).unwrap().at;
        let driven = drive(&state, &dir, None, &[], at + CEILING - 1, &|_, _| None, &mut push);
        assert!(driven.notes.is_empty() && typed.borrow().is_empty() && crate::repo::rebase_in_progress(&alpha.path), "the rebase under way is left alone, never aborted for a made-up conflict: {driven:?}");
        assert!(load(&state, &first).is_some() && !landed(&origin, &upstream).contains("beta"));
        let driven = drive(&state, &dir, None, &[], at + CEILING, &|_, _| None, &mut push);
        assert!(driven.notes[0].contains("a rebase in its worktree did not finish within 30 minutes"), "{driven:?}");
        assert_eq!(phase_of(&state, &first), Some(Phase::NeedsYou(Why::LeftLine)));
        assert!(landed(&origin, &upstream).starts_with("beta\n"), "an abandoned rebase never holds the line: {}", landed(&origin, &upstream));
        drop_worktree(&dir, &beta);
        let refused = super::super::push::join(&state, &Caller::User, &dir, &alpha, Through::All).unwrap_err();
        assert!(refused.contains("a rebase is under way"), "{refused}");
        git(&alpha.path, &["rebase", "--abort"]).unwrap();
        cleanup(dir, origin, &alpha);
    }

    #[test]
    fn a_push_is_taken_back_only_by_who_may_and_never_mid_turn_or_once_its_commits_landed() {
        let (dir, origin, alpha, _) = task_with_a_commit("alpha");
        let state = crate::lead::state_dir().unwrap();
        let first = crate::peers::address("alpha", &name_of(&dir));
        let _line = Line { state: state.clone(), addresses: vec![first.clone()] };
        let cancel = |caller: Caller| super::super::push::cancel_as(&state, &caller, &dir, &alpha);
        intent::after_red_ci(&state, &dir, &first, By::User).unwrap();
        phase::record(&state, &first, "ci", &Event::CiRed { sha: "abc", pushing: true });
        assert!(cancel(Caller::User).unwrap_err().contains("its commits are on main"), "a red CI's fix is no push to take back");
        assert!(load(&state, &first).is_some() && phase_of(&state, &first) == Some(Phase::Pushing { ahead: 0 }), "its fix flow goes on");
        forget(&state, &first);
        super::super::push::join(&state, &Caller::User, &dir, &alpha, Through::All).unwrap();
        assert!(cancel(Caller::Lead).unwrap_err().contains("the user asked for this push"));
        let held = Turnstile::try_take(&state, &dir).unwrap();
        assert!(cancel(Caller::User).unwrap_err().contains("kebbie is pushing it now"));
        drop(held);
        assert!(load(&state, &first).is_some(), "refused, it keeps its place");
        assert!(cancel(Caller::User).unwrap().contains("Taken out of the push line") && load(&state, &first).is_none());
        cleanup(dir, origin, &alpha);
    }

    #[test]
    fn each_repo_has_its_own_line() {
        let (dir, origin, alpha, _) = task_with_a_commit("alpha");
        let (other, other_origin, delta, other_upstream) = task_in_another_repo("delta");
        let state = crate::sh::test_dir("line-repos");
        upstream_commit(&dir, "d", "theirs", "d on main");
        super::super::push::join(&state, &Caller::User, &dir, &alpha, Through::All).unwrap();
        super::super::push::join(&state, &Caller::User, &other, &delta, Through::All).unwrap();
        let (typed, mut push) = real_push(&state);
        complete_in(&state, &[], 1000, &|_, _| None, &mut push);
        assert_eq!(typed.borrow().len(), 1, "alpha waits for its Claude");
        assert!(landed(&other_origin, &other_upstream).starts_with("fourth\n"), "delta's repo did not wait for alpha's");
        drop(push);
        std::fs::remove_dir_all(state).unwrap();
        cleanup(other, other_origin, &delta);
        cleanup(dir, origin, &alpha);
    }

    #[test]
    fn a_red_cis_fix_waits_outside_the_line_and_joins_its_end_once_the_lead_approved_it() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = crate::peers::address("billing", &name_of(&dir));
        intent::after_red_ci(&state, &dir, &address, By::User).unwrap();
        assert!(intent::line(&state, &dir).is_empty() && intent::repos(&state).is_empty(), "a push waiting for a fix holds no repo's line");
        let mut ready = report("r1", load(&state, &address).unwrap().at + 1, "ready");
        ready.from = address.clone();
        let mut pushed = 0;
        complete_in(&state, std::slice::from_ref(&ready), 1000, &|_, _| None, &mut |_, _| { pushed += 1; Ok(Sent::Pushed(String::new())) });
        assert_eq!(pushed, 0, "the fix waits for the lead's check");
        crate::tasks::approval::record_approval(&state, &address, &fingerprint(&slot.path, &upstream).unwrap()).unwrap();
        complete_in(&state, &[ready], 1000, &|_, _| None, &mut |intent, _| { assert_eq!((intent.work, intent.queued), (Work::Turn, Some(1000))); pushed += 1; Ok(Sent::Pushed(String::new())) });
        assert!(pushed == 1 && load(&state, &address).is_none());
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_driver_that_finds_the_line_held_leaves_it_to_the_one_holding_it() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        save(&state, &intent(By::User, &dir, &fingerprint(&slot.path, &upstream).unwrap())).unwrap();
        let held = Turnstile::try_take(&state, &dir).unwrap();
        assert!(Turnstile::try_take(&state, &dir).is_none());
        let mut ready = report("r1", 200, "ready");
        ready.from = crate::peers::address("billing", &name_of(&dir));
        assert!(drive(&state, &dir, None, std::slice::from_ref(&ready), 300, &|_, _| None, &mut pushed).notes.is_empty(), "two drivers never push one repo at once");
        drop(held);
        assert_eq!(drive(&state, &dir, None, &[ready], 300, &|_, _| None, &mut pushed).notes.len(), 1);
        cleanup(dir, origin, &slot);
    }
}
