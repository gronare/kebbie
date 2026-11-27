use std::time::Duration;

use crate::scenarios::lab::{Lab, eventually, stderr, stdout};

#[test]
fn a_promise_the_lead_makes_after_a_task_waits_until_the_task_lands_then_the_keeper_reminds_the_lead_once() {
    let lab = Lab::new("promise");
    let billing = lab.start("billing", "Add refunds", &[]);
    let made = lab.run_as("lead", &["-C", lab.app(), "promise", "Set tests.lint_command for app", "--after", "billing"]);
    assert!(made.status.success(), "{}\n{}", stdout(&made), stderr(&made));
    let open = stdout(&lab.run_as("lead", &["promises"]));
    assert!(open.contains("Set tests.lint_command for app") && open.contains("billing"), "it waits on billing: {open}");
    lab.keeper_round();
    assert!(!lab.to_lead().iter().any(|text| text.contains("is due: Set tests.lint_command")), "no reminder while billing is open: {:?}", lab.to_lead());
    let mine = lab.run_as("billing@app", &["-C", billing.to_str().unwrap(), "promise", "Rewrite the refunds"]);
    assert!(!mine.status.success(), "a task makes no promise for the lead: {}", stdout(&mine));

    lab.work(&billing, "refunds.rb", "Add refunds");
    let pushed = lab.run_as_user(&["-C", lab.app(), "push", "billing"]);
    assert!(pushed.status.success(), "{}\n{}", stdout(&pushed), stderr(&pushed));
    lab.keeper_round();
    let due = |lab: &Lab| lab.to_lead().iter().filter(|text| text.contains("Promise 1 is due: Set tests.lint_command for app")).count();
    assert!(eventually(Duration::from_secs(10), || due(&lab) == 1), "the keeper reminds the lead once billing landed: {:?}\n{}", lab.to_lead(), lab.keeper_log());
    lab.keeper_round();
    assert_eq!(due(&lab), 1, "a reminder is sent once");

    let done = lab.run_as("lead", &["promise", "done", "1"]);
    assert!(done.status.success(), "{}\n{}", stdout(&done), stderr(&done));
    assert!(!stdout(&lab.run_as("lead", &["promises"])).contains("Set tests.lint_command"), "a kept promise leaves the list");
}
