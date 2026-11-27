use std::collections::HashMap;
use std::path::Path;

use crate::messages::{self, Message};
use crate::peers;
use crate::permit::Caller;
use crate::repo::name_of;
use crate::sh::Result;
use super::intent::{self, Through};
use super::pool::{Slot, slots};

pub(super) type Join<'a> = &'a mut dyn FnMut(&Path, &Slot, &str) -> Result<()>;

// Each round, a ready task under the user's push permission joins its repo's push line as kebbie's own push, once per ready report and while it left the line no more than REPUSHES times; the line's own checks decide whether its commits may go.
pub(crate) fn tick(state: &Path) -> Vec<String> {
    let reports = messages::latest_reports(&messages::load());
    let mut join = |repo: &Path, slot: &Slot, seen: &str| super::push::join_on(state, &Caller::Keeper, repo, slot, Through::All, Some(seen.to_string()), None);
    let mut notes = Vec::new();
    for repo in crate::repo::pooled_checkouts() {
        let Ok(found) = slots(&repo) else { continue };
        notes.extend(found.iter().filter_map(|slot| consider(state, &repo, slot, &reports, &mut join)));
    }
    notes
}

fn consider(state: &Path, repo: &Path, slot: &Slot, reports: &HashMap<String, Message>, join: Join) -> Option<String> {
    let task = slot.task.as_deref()?;
    let address = peers::address(task, &name_of(repo));
    crate::permit::in_state(state, &address, &name_of(repo))?;
    let report = reports.get(&address).filter(|report| report.report.as_ref().is_some_and(|report| report.state == "ready"))?;
    if intent::load(state, &address).is_some() || super::waiting::load(state, &address).is_some() || last_joined(state, &address).as_deref() == Some(report.id.as_str()) || !intent::lead_may_repush(state, &address) {
        return None;
    }
    join(repo, slot, &report.id).ok()?;
    Some(format!("push: {address}: joined the push line under the user's push permission"))
}

// The report the task's last push was asked on, by anyone: a push that left the line, failed or was taken out waits for the task's next ready report.
pub(super) fn used_up(state: &Path, address: &str, report: &str) {
    let _ = crate::state::write(&file(state, address), report);
}

pub(super) fn forget(state: &Path, address: &str) {
    let _ = std::fs::remove_file(file(state, address));
}

fn file(state: &Path, address: &str) -> std::path::PathBuf {
    state.join("push-permitted").join(address)
}

fn last_joined(state: &Path, address: &str) -> Option<String> {
    std::fs::read_to_string(file(state, address)).ok().map(|id| id.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::pool::task_of;
    use crate::tasks::intent::tests::{intent, report};
    use crate::tasks::intent::{By, save};
    use crate::tasks::test_support::*;

    fn ready_of(address: &str, id: &str, state: &str) -> HashMap<String, Message> {
        let mut message = report(id, 200, state);
        message.from = address.to_string();
        HashMap::from([(address.to_string(), message)])
    }

    #[test]
    fn a_ready_task_under_a_permission_joins_the_line_once_per_ready_report_and_nothing_else_does() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = peers::address("billing", &name_of(&dir));
        let mut joined = Vec::new();
        let run = |reports: &HashMap<String, Message>, joined: &mut Vec<String>| consider(&state, &dir, &slot, reports, &mut |_, slot, seen| { joined.push(task_of(slot).to_string()); used_up(&state, &address, seen); Ok(()) });
        assert_eq!(run(&ready_of(&address, "r1", "ready"), &mut joined), None, "no permission: it waits for the user under Ready to push");
        crate::permit::set_task_in(&state, &address, true).unwrap();
        assert_eq!(run(&ready_of(&address, "r1", "needs-you"), &mut joined), None, "a task that asks something after its approval stays");
        assert!(run(&ready_of(&address, "r1", "ready"), &mut joined).is_some_and(|note| note.contains("joined the push line")), "a task ready before the permission goes once the permission is set");
        assert_eq!(run(&ready_of(&address, "r1", "ready"), &mut joined), None, "a push that left the line, failed or was taken out is not sent again for the same report");
        save(&state, &intent(By::Lead, &dir, "abc")).unwrap();
        assert_eq!(run(&ready_of(&address, "r2", "ready"), &mut joined), None, "a push already in the line keeps its place");
        intent::forget(&state, &address);
        assert!(run(&ready_of(&address, "r2", "ready"), &mut joined).is_some(), "its next ready report puts it in the line again");
        crate::permit::set_task_in(&state, &address, false).unwrap();
        crate::permit::set_repo_in(&state, &name_of(&dir), true).unwrap();
        assert!(run(&ready_of(&address, "r3", "ready"), &mut joined).is_some(), "the repo's permission covers it as the task's does");
        assert_eq!(joined, ["billing", "billing", "billing"]);
        let refused = consider(&state, &dir, &slot, &ready_of(&address, "r4", "ready"), &mut |_, _, _| Err("no rules approval".to_string()));
        assert!(refused.is_none() && last_joined(&state, &address).as_deref() == Some("r3"), "a push the line's checks refuse is tried again next round");
        for time in 0..=intent::REPUSHES {
            intent::record_left(&state, &address, &format!("left {time}"));
        }
        assert_eq!(run(&ready_of(&address, "r6", "ready"), &mut joined), None, "a push that left the line more than REPUSHES times is the user's again");
        intent::forget_left_count(&state, &address);
        let wait = crate::tasks::waiting::Waiting { address: address.clone(), repo: dir.clone(), on: vec![crate::tasks::after::Dep { address: "payload@api".to_string(), repo: dir.clone() }], then: None, by: "user".to_string(), at: 1, gone_since: None, unlisted_since: None, unlisted_told: false };
        crate::tasks::waiting::save(&state, &wait).unwrap();
        assert_eq!(run(&ready_of(&address, "r5", "ready"), &mut joined), None, "a task told to wait for another with kebbie wait stays");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_the_user_or_the_lead_asked_for_uses_up_its_report_so_kebbie_never_queues_it_again_after_it_left_the_line() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = peers::address("billing", &name_of(&dir));
        crate::permit::set_task_in(&state, &address, true).unwrap();
        crate::tasks::push::join_on(&state, &Caller::User, &dir, &slot, Through::All, Some("u1".to_string()), None).unwrap();
        intent::forget(&state, &address);
        let mut joined = 0;
        assert_eq!(consider(&state, &dir, &slot, &ready_of(&address, "u1", "ready"), &mut |_, _, _| { joined += 1; Ok(()) }), None, "the user took it out of the line, or it left it, on this same report");
        assert_eq!(joined, 0);
        assert!(consider(&state, &dir, &slot, &ready_of(&address, "u2", "ready"), &mut |_, _, _| { joined += 1; Ok(()) }).is_some(), "its next ready report puts it in the line");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_ready_report_a_handed_over_push_went_out_on_is_used_up_even_when_the_push_then_fails() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = peers::address("billing", &name_of(&dir));
        crate::permit::set_task_in(&state, &address, true).unwrap();
        let current = crate::tasks::approval::fingerprint(&slot.path, &upstream).unwrap();
        save(&state, &intent::Intent { address: address.clone(), seen: Some("r1".to_string()), ..intent(By::Keeper, &dir, &current) }).unwrap();
        let answered = { let mut message = report("r2", 200, "ready"); message.from = address.clone(); message };
        let notes = crate::tasks::line::complete_in(&state, std::slice::from_ref(&answered), 300, &|_, _| None, &|_| false, &mut |_, _| Err("remote hung up".to_string()));
        assert!(intent::load(&state, &address).is_none() && notes.iter().any(|note| note.contains("could not finish the push")), "{notes:?}");
        assert_eq!(consider(&state, &dir, &slot, &ready_of(&address, "r2", "ready"), &mut |_, _, _| Ok(())), None, "the tests' ready report was used up by the push that failed, so kebbie waits for the next one");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_ready_answer_that_waited_for_its_rules_approval_and_then_left_the_line_is_used_up() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = peers::address("billing", &name_of(&dir));
        crate::permit::set_task_in(&state, &address, true).unwrap();
        save(&state, &intent::Intent { address: address.clone(), seen: Some("r1".to_string()), ..intent(By::Keeper, &dir, "changed since") }).unwrap();
        save(&state, &intent::Intent { address: format!("copy@{}", name_of(&dir)), task: "copy".to_string(), queued: Some(150), ..intent(By::User, &dir, "other") }).unwrap();
        let answered = { let mut message = report("r2", 200, "ready"); message.from = address.clone(); message };
        let later = 100 + crate::tasks::line::CEILING + 1;
        let notes = crate::tasks::line::complete_in(&state, std::slice::from_ref(&answered), later, &|_, _| None, &|_| false, &mut |_, _| panic!("no rules approval, so nothing goes out"));
        assert!(intent::load(&state, &address).is_none() && notes.iter().any(|note| note.contains("out of the push line")), "{notes:?}");
        assert_eq!(consider(&state, &dir, &slot, &ready_of(&address, "r2", "ready"), &mut |_, _, _| Ok(())), None, "the approval that comes later does not send it again on the report it left the line with");
        cleanup(dir, origin, &slot);
    }
}
