use crate::lab::{Lab, git, stderr, stdout};

#[test]
fn a_task_queued_after_another_waits_on_the_board_and_starts_once_the_first_is_pushed() {
    let lab = Lab::new("queue");
    let billing = lab.start("billing", "Add refunds", &[]);
    let queued = lab.run_as("lead", &["-C", lab.app(), "start", "search", "Search the refunds", "--after", "billing", "--why", "it reads the refunds", "--recipe", "task-after-task: it builds on the refunds"]);
    assert!(queued.status.success(), "{}\n{}", stdout(&queued), stderr(&queued));
    let waiting = lab.item("search");
    assert_eq!(waiting["group"], "QUEUED", "{waiting}");
    assert!(waiting["summary"].as_str().unwrap().contains("it reads the refunds"), "{waiting}");
    assert!(stdout(&lab.run(&["after"])).contains("billing"), "kebbie after names what it waits for");
    lab.keeper_round();
    assert!(lab.prompts_to("search").is_empty() && !lab.worktree(2).exists(), "it waits while billing is open");

    lab.work(&billing, "refunds.rb", "Add refunds");
    let pushed = lab.run_as_user(&["-C", lab.app(), "push", "billing"]);
    assert!(pushed.status.success() && stdout(&pushed).contains("closed the task"), "{}\n{}", stdout(&pushed), stderr(&pushed));
    lab.keeper_round();
    let prompt = lab.prompts_to("search");
    assert!(prompt.len() == 1 && prompt[0].starts_with("This task waited for billing@app, which is done.") && prompt[0].contains("Add refunds\n\nSearch the refunds\n\nRecipe: task after task"), "{prompt:?}\n{}", lab.keeper_log());
    let started = lab.item("search");
    assert_eq!(started["group"], "WORKING", "{started}");
    let worktree = std::path::PathBuf::from(started["path"].as_str().unwrap());
    assert_eq!(git(&worktree, &["log", "-1", "--format=%s"]), "Add refunds", "it starts from the default branch with the first task's work");
}

#[test]
fn a_parked_task_leaves_its_group_until_it_is_unparked_and_only_the_user_or_lead_parks_it() {
    let lab = Lab::new("park");
    lab.start("billing", "Add refunds", &[]);
    let refused = lab.run_as("billing@app", &["-C", lab.app(), "park", "billing"]);
    assert!(!refused.status.success(), "a task does not park itself: {}", stdout(&refused));
    let parked = lab.ok(&["-C", lab.app(), "park", "billing", "--until", "in 2h"]);
    assert!(parked.contains("Parked, "), "{parked}");
    let item = lab.item("billing");
    assert_eq!(item["group"], "PARKED", "{item}");
    assert!(item["parked_until"].as_u64().is_some(), "{item}");
    let back = lab.ok(&["-C", lab.app(), "unpark", "billing"]);
    assert!(back.contains("Back from parking"), "{back}");
    assert_eq!(lab.item("billing")["group"], "WORKING");
    let again = lab.run(&["-C", lab.app(), "unpark", "billing"]);
    assert!(!again.status.success() && stderr(&again).contains("Not parked"), "{}", stderr(&again));
}

#[test]
fn a_task_in_steps_pushes_each_accepted_step_while_it_builds_the_next_and_without_steps_it_pushes_whole() {
    let lab = Lab::new("steps");
    let billing = lab.start("billing", "Add refunds", &[]);
    let slot = billing.to_str().unwrap();
    let planned = lab.run_as("billing@app", &["-C", slot, "steps", "Add refunds", "Document refunds"]);
    assert!(planned.status.success(), "{}", stderr(&planned));
    let first = lab.work(&billing, "refunds.rb", "Add refunds");
    assert!(lab.run_as("billing@app", &["-C", slot, "steps", "done"]).status.success(), "the first step is ready");
    let second = lab.work(&billing, "refunds.md", "Document refunds");
    let refused = lab.run_as("lead", &["-C", lab.app(), "accept", "billing", &first]);
    assert!(!refused.status.success() && stderr(&refused).contains("accepting a commit is the user's call"), "{}", stderr(&refused));
    let accepted = lab.run_as_user(&["-C", lab.app(), "accept", "billing", &first]);
    assert!(accepted.status.success(), "{}", stderr(&accepted));
    let steps: serde_json::Value = serde_json::from_str(&lab.ok(&["-C", lab.app(), "steps", "billing", "--json"])).unwrap();
    assert_eq!((steps["steps"][0]["state"].as_str(), steps["steps"][1]["commits"][0]["accepted"].as_bool(), steps["last_accepted"].as_str()), (Some("accepted"), Some(false), Some(first.as_str())), "{steps}");
    let through = lab.run_as_user(&["-C", lab.app(), "push", "billing", "--through", &first]);
    assert!(through.status.success(), "{}\n{}", stdout(&through), stderr(&through));
    assert_eq!(git(&lab.root.join("origin.git"), &["log", "-1", "--format=%s", "main"]), "Add refunds", "only the accepted step went out");
    assert_eq!(git(&billing, &["rev-parse", "HEAD"]), second, "the task's worktree stays as it was");
    assert!(lab.on_board("billing"), "and the task stays open");

    lab.config("[task]\nsteps = false\n");
    let search = lab.start("search", "Add the search page", &[]);
    let off = lab.run_as("search@app", &["-C", search.to_str().unwrap(), "steps", "Add search"]);
    assert!(!off.status.success() && stderr(&off).contains("set task.steps = true"), "{}", stderr(&off));
    lab.work(&search, "search.rb", "Add the search page");
    let whole = lab.run_as_user(&["-C", lab.app(), "push", "search"]);
    assert!(whole.status.success() && stdout(&whole).contains("closed the task"), "no acceptance is asked for without steps: {}\n{}", stdout(&whole), stderr(&whole));
}

#[test]
fn with_require_accept_a_push_takes_only_accepted_steps_and_the_task_waits_on_you_for_the_next() {
    let lab = Lab::new("require-accept");
    let free = lab.start("free", "Add the footer", &[]);
    lab.work(&free, "footer.rb", "Add the footer");
    let today = lab.run_as_user(&["-C", lab.app(), "push", "free"]);
    assert!(today.status.success() && stdout(&today).contains("closed the task"), "off, a push takes what is committed: {}", stderr(&today));

    lab.config("[task]\nrequire_accept = true\n");
    let billing = lab.start("billing", "Add refunds", &[]);
    let slot = billing.to_str().unwrap();
    assert!(lab.run_as("billing@app", &["-C", slot, "steps", "Add refunds", "Document refunds"]).status.success());
    let first = lab.work(&billing, "refunds.rb", "Add refunds");
    assert!(lab.run_as("billing@app", &["-C", slot, "steps", "done"]).status.success(), "the first step is ready");
    let second = lab.work(&billing, "refunds.md", "Document refunds");
    lab.ok(&["-C", slot, "report", "--state", "ready", "--next", "push", "refunds done"]);
    let item = lab.item("billing");
    assert_eq!(item["group"], "NEEDS YOU", "{item}");
    assert!(item["summary"].as_str().unwrap().contains("2 to accept"), "{item}");
    let next = lab.ok(&["next"]);
    let row = next.lines().find(|line| line.contains(" billing ")).unwrap_or_else(|| panic!("{next}"));
    assert!(!row.contains('↑') && row.contains("accept step 1"), "it is stopped on you, not to push: {next}");

    let held = lab.run_as_user(&["-C", lab.app(), "push", "billing"]);
    assert!(!held.status.success() && stderr(&held).contains("Not pushed: step 1 is not accepted"), "{}\n{}", stdout(&held), stderr(&held));
    assert_eq!(git(&lab.root.join("origin.git"), &["log", "-1", "--format=%s", "main"]), "Add the footer");

    assert!(lab.run_as_user(&["-C", lab.app(), "accept", "billing", &first]).status.success());
    let through = lab.run_as_user(&["-C", lab.app(), "push", "billing", "--through", &first]);
    assert!(through.status.success(), "{}\n{}", stdout(&through), stderr(&through));
    assert_eq!(git(&lab.root.join("origin.git"), &["log", "-1", "--format=%s", "main"]), "Add refunds", "the accepted step goes");
    let rest = lab.run_as_user(&["-C", lab.app(), "push", "billing"]);
    assert!(!rest.status.success() && stderr(&rest).contains("is not accepted"), "the next step waits for its accept: {}", stderr(&rest));

    assert!(lab.run_as_user(&["-C", lab.app(), "accept", "billing", &second]).status.success());
    let all = lab.run_as_user(&["-C", lab.app(), "push", "billing"]);
    assert!(all.status.success() && stdout(&all).contains("closed the task"), "{}\n{}", stdout(&all), stderr(&all));
    assert_eq!(git(&lab.root.join("origin.git"), &["log", "-1", "--format=%s", "main"]), "Document refunds");
}
