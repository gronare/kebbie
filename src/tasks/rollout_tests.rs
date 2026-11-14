use super::*;
use super::super::plan::{Part, Seen, Unpushed, derive};
use crate::tasks::test_support::*;
use crate::repo::git;

fn part(id: &str) -> Part {
    Part { id: id.to_string(), title: id.to_string(), done: true, ..Part::default() }
}

fn stage(id: &str, kind: RequirementKind, subtasks: &[&str]) -> StagePlan {
    StagePlan { subtasks: subtasks.iter().map(|id| id.to_string()).collect(), ..StagePlan::new(id.to_string(), format!("Stage {id}"), kind, (kind != RequirementKind::Deploy).then(|| format!("{id} holds"))) }
}

fn accepted(sha: &str) -> Unpushed {
    Unpushed { sha: sha.repeat(40), subject: format!("commit {sha}"), code: format!("code {sha}"), accepted: true }
}

fn seen(sha: &str, subtask: &str) -> Seen {
    Seen { sha: sha.repeat(40), code: format!("code {sha}"), subject: format!("commit {sha}"), subtask: subtask.to_string() }
}

// Three subtasks, a commit each, every commit accepted: add the column, backfill it, drop the old one.
fn migration(stages: Vec<StagePlan>) -> Subtasks {
    migration_after(stages, &[])
}

fn migration_after(stages: Vec<StagePlan>, landed: &[&str]) -> Subtasks {
    let plan = Plan { base: "base".to_string(), parts: vec![part("add"), part("backfill"), part("drop")], commits: vec![seen("a", "add"), seen("b", "backfill"), seen("c", "drop")], stages, ..Plan::default() };
    let landed: Vec<String> = landed.iter().map(|sha| format!("code {sha}")).collect();
    derive(Some(&plan), &landed, &[accepted("a"), accepted("b"), accepted("c")], false).0
}

fn pushed(mut stage: StagePlan, state: StageState) -> StagePlan {
    stage.through = Some("f".repeat(40));
    stage.pushed_at = Some(10);
    stage.kept = Some(state);
    stage.met_at = (state == StageState::Met).then_some(20);
    stage
}

#[test]
fn a_stage_is_named_with_its_requirement_in_one_word() {
    assert_eq!(spec("Add the column, write both:deploy").unwrap(), ("Add the column, write both".to_string(), RequirementKind::Deploy, None));
    assert_eq!(spec("Backfill:check:\"select count(*) from users where email is null\" is 0").unwrap().2.as_deref(), Some("select count(*) from users where email is null\" is 0"));
    assert_eq!(spec("Drop the old column:verified:'nothing reads users.mail'").unwrap(), ("Drop the old column".to_string(), RequirementKind::Verified, Some("nothing reads users.mail".to_string())));
    assert_eq!(spec("Deploy: a title with a colon:verified:it holds").unwrap().0, "Deploy: a title with a colon", "the first requirement mark ends the title");
    assert!(spec("No requirement").unwrap_err().contains("names no requirement"));
    assert!(spec(":deploy").unwrap_err().contains("needs a title"));
    assert!(spec("Backfill:check:").unwrap_err().contains("what holds"));
}

#[test]
fn the_pushable_run_stops_at_a_stage_that_is_not_met_and_a_subtask_no_stage_names_goes_out_with_the_last() {
    let three = vec![stage("both", RequirementKind::Deploy, &["add"]), stage("fill", RequirementKind::Verified, &["backfill"]), stage("cut", RequirementKind::Check, &[])];
    let fresh = migration(three.clone());
    assert_eq!(fresh.order.iter().map(|row| row.stage.as_deref().unwrap()).collect::<Vec<_>>(), ["both", "fill", "cut"], "drop is in no stage, so it goes out with the last");
    assert_eq!(fresh.stages.iter().map(|stage| stage.state).collect::<Vec<_>>(), [StageState::Open, StageState::Locked, StageState::Locked]);
    assert_eq!(fresh.pushable.as_ref().map(|run| run.count), Some(1), "only the first stage is pushable, every commit accepted");
    assert_eq!(fresh.items.iter().find(|subtask| subtask.id == "backfill").and_then(|subtask| subtask.waits_for.clone()).as_deref(), Some("Stage both to be met"), "an accepted subtask behind the boundary says what it waits for");
    let out = migration(vec![pushed(three[0].clone(), StageState::Pushed), three[1].clone(), three[2].clone()]);
    assert_eq!((out.pushable.as_ref().map(|run| run.count), out.stages[0].state), (Some(1), StageState::Pushed), "pushed and not yet deployed holds the next stage, while a fix in the stage itself may follow");
    let deployed = migration_after(vec![pushed(three[0].clone(), StageState::Met), three[1].clone(), three[2].clone()], &["a"]);
    assert_eq!((deployed.pushable.as_ref().map(|run| run.through.clone()), deployed.stages[1].state), (Some("b".repeat(40)), StageState::Open), "a met stage opens the next");
    let unfinished = migration(vec![pushed(three[0].clone(), StageState::Met), three[1].clone(), three[2].clone()]);
    assert_eq!((unfinished.pushable.as_ref().map(|run| run.count), unfinished.stages[0].state, unfinished.stages[1].state), (Some(1), StageState::Open, StageState::Locked), "a met stage with a commit still to push is open again, and the next waits until it is met with all of it");
    let waiting = migration_after(vec![pushed(three[0].clone(), StageState::Met), pushed(three[1].clone(), StageState::Waiting), three[2].clone()], &["a", "b"]);
    assert_eq!(waiting.stages.iter().map(|stage| stage.state).collect::<Vec<_>>(), [StageState::Met, StageState::Waiting, StageState::Locked]);
    let failed = migration_after(vec![pushed(three[0].clone(), StageState::Met), pushed(three[1].clone(), StageState::Failed), three[2].clone()], &["a", "b"]);
    assert_eq!((failed.stages[2].state, failed.pushable.as_ref().map(|run| run.count)), (StageState::Locked, None), "a failed stage holds the rest");
}

#[test]
fn a_commit_out_of_stage_order_ends_the_pushable_run_and_names_itself() {
    let swapped = [stage("first", RequirementKind::Deploy, &["add", "drop"]), stage("second", RequirementKind::Deploy, &["backfill"])];
    let subtasks = migration_after(vec![pushed(swapped[0].clone(), StageState::Met), swapped[1].clone()], &["a"]);
    assert_eq!(subtasks.out_of_stage, Some("c".repeat(40)), "drop belongs to the first stage but sits after the second's commit");
    assert_eq!(subtasks.pushable.as_ref().map(|run| run.count), None, "the first stage is open again while drop waits, so the second's commit waits behind it");
}

#[test]
fn a_task_with_no_stages_pushes_as_before() {
    let none = migration(Vec::new());
    assert!(none.stages.is_empty() && none.order.iter().all(|row| row.stage.is_none()) && none.out_of_stage.is_none());
    assert_eq!(none.pushable.as_ref().map(|run| run.count), Some(3));
}

#[test]
fn the_plan_file_keeps_each_stage_through_a_save_and_a_load() {
    let stages = vec![pushed(stage("both", RequirementKind::Deploy, &["add"]), StageState::Met), StagePlan { note: Some("half the rows are empty".to_string()), ..pushed(stage("fill", RequirementKind::Verified, &["backfill"]), StageState::Failed) }, stage("cut", RequirementKind::Check, &["drop"])];
    assert_eq!(read(&written(&stages)), stages);
}

#[test]
fn a_three_stage_migration_goes_out_through_deploy_then_verified_then_the_drop() {
    let (dir, origin, slot, upstream) = task_with_a_commit("email");
    let state = state_of(&dir);
    let address = peers::address("email", &name_of(&dir));
    let task = Task { path: &slot.path, upstream: &upstream, address: &address };
    super::super::plan::list_at(&state, &slot.path, &upstream, &address, &["Add the column and write both", "Backfill", "Drop the old column"]).unwrap();
    let said = declare_as(&Caller::User, &state, &task, &dir, &["Write both:deploy".to_string(), "Backfill:verified:the backfill has gone through in production".to_string(), "Drop:check:no query reads users.mail".to_string()]).unwrap();
    assert!(said.contains("3 stages"), "{said}");
    for (stage, subtask) in [("write-both", "add-column"), ("backfill", "backfill"), ("drop", "drop-old")] {
        assign_as(&Caller::User, &state, &task, stage, &[subtask.to_string()]).unwrap();
    }
    assert!(declare_as(&Caller::Task("other@app".to_string()), &state, &task, &dir, &["Only:deploy".to_string()]).unwrap_err().contains("its own subtasks"), "a task declares its own stages only");
    let ids = |stage: &str| vec![stage.to_string()];
    let now = || plan::load(&state, &address).map(|plan| shown(&plan.stages)).unwrap();

    mark_pushed(&state, &address, &ids("write-both"), &"1".repeat(40), 100);
    assert_eq!(now()[0].state, StageState::Pushed);
    let rolled = rolled_out(&state, &address, &ids("write-both"), None, 110);
    assert_eq!((rolled.met.as_slice(), rolled.all_met), (&["Write both".to_string()][..], false), "CI green on the pushed commit meets a deploy");
    assert_eq!(now().iter().map(|stage| stage.state).collect::<Vec<_>>(), [StageState::Met, StageState::Open, StageState::Locked]);

    mark_pushed(&state, &address, &ids("backfill"), &"2".repeat(40), 200);
    assert!(crate::kebbie_questions::load(&state).is_empty(), "the verification is asked once the stage is out, not when it is pushed");
    rolled_out(&state, &address, &ids("backfill"), None, 210);
    let asked = crate::kebbie_questions::load(&state);
    assert_eq!((asked.len(), asked[0].stage.as_deref(), now()[1].state), (1, Some("backfill"), StageState::Waiting));
    assert!(asked[0].text.contains("the backfill has gone through in production"), "{}", asked[0].text);

    failed(&state, &address, &ids("backfill"), "you said: half the rows have no email", false);
    assert_eq!((now()[1].state, now()[1].note.as_deref(), now()[2].state), (StageState::Failed, Some("you said: half the rows have no email"), StageState::Locked), "Something is wrong fails the stage and holds the drop");
    assert!(crate::kebbie_questions::load(&state).is_empty(), "a failed stage's question goes");

    mark_pushed(&state, &address, &ids("backfill"), &"3".repeat(40), 300);
    assert_eq!((now()[1].state, now()[1].note.as_deref()), (StageState::Pushed, None), "a fix pushed in the stage asks for its requirement again");
    rolled_out(&state, &address, &ids("backfill"), None, 310);
    assert_eq!(crate::kebbie_questions::load(&state).len(), 1);
    assert!(verified(&state, &address, "backfill", 320).unwrap().starts_with("Backfill is met; Drop can be pushed"));
    assert_eq!(now().iter().map(|stage| stage.state).collect::<Vec<_>>(), [StageState::Met, StageState::Met, StageState::Open]);

    mark_pushed(&state, &address, &ids("drop"), &"4".repeat(40), 400);
    let rolled = rolled_out(&state, &address, &ids("drop"), None, 410);
    assert!(rolled.checking && !rolled.all_met, "a check waits on the task");
    let held = check_held(&state, &address, &ids("drop"), 420);
    assert!(held.all_met && all_met(&state, &address), "the check held: every stage is met");
    cleanup(dir, origin, &slot);
}

#[test]
fn a_deploy_kebbie_cannot_see_is_asked_of_the_user_and_a_pushed_stage_keeps_its_place_and_requirement() {
    let (dir, origin, slot, upstream) = task_with_a_commit("rename");
    let state = state_of(&dir);
    let address = peers::address("rename", &name_of(&dir));
    let task = Task { path: &slot.path, upstream: &upstream, address: &address };
    super::super::plan::list_at(&state, &slot.path, &upstream, &address, &["Expand", "Contract"]).unwrap();
    declare_as(&Caller::User, &state, &task, &dir, &["Expand:deploy".to_string(), "Contract:verified:the old name has no readers".to_string()]).unwrap();
    mark_pushed(&state, &address, &["expand".to_string()], &"1".repeat(40), 100);
    let rolled = rolled_out(&state, &address, &["expand".to_string()], Some("ci.watch is off"), 110);
    assert_eq!(rolled.asked, ["Expand"]);
    let asked = crate::kebbie_questions::load(&state);
    assert!(asked[0].text.contains("cannot see it deploy (ci.watch is off)"), "{}", asked[0].text);
    assert_eq!(crate::kebbie_questions::wire(&asked[0], &plan::load(&state, &address).map(|plan| shown(&plan.stages)).unwrap()).stage, None, "the phone gets no stage for a deploy, which the protocol keeps for a verification");
    assert!(declare_as(&Caller::User, &state, &task, &dir, &["Contract:verified:the old name has no readers".to_string()]).unwrap_err().contains("pushed already"), "a pushed stage stays in the list");
    assert!(declare_as(&Caller::User, &state, &task, &dir, &["Expand:verified:it is out".to_string(), "Contract:deploy".to_string()]).unwrap_err().contains("requirement stays"));
    assert!(declare_as(&Caller::User, &state, &task, &dir, &["Contract:deploy".to_string(), "Expand:deploy".to_string()]).unwrap_err().contains("come first"));
    cleanup(dir, origin, &slot);
}

// A gh that lists the runs of a commit from runs-<sha>.json beside it, and none before that file is there.
fn fake_gh(dir: &Path) -> String {
    let gh = dir.with_extension("gh").join("gh");
    std::fs::create_dir_all(dir.with_extension("gh")).unwrap();
    crate::executable::write(&gh, "#!/bin/sh\nd=$(dirname \"$0\")\ncase \"$1 $2\" in\n  'run list') while [ $# -gt 0 ]; do [ \"$1\" = --commit ] && sha=$2; shift; done; cat \"$d/runs-$sha.json\" 2>/dev/null || echo '[]';;\n  'workflow list') echo '[{\"state\":\"active\"}]';;\n  *) echo '[]';;\nesac\n").unwrap();
    gh.to_string_lossy().to_string()
}

fn green(dir: &Path, sha: &str) {
    std::fs::write(dir.with_extension("gh").join(format!("runs-{sha}.json")), r#"[{"databaseId":1,"name":"CI","workflowName":"CI","status":"completed","conclusion":"success","url":"https://github.com/acme/app/actions/runs/1"}]"#).unwrap();
}

fn watching(state: &Path, repo: &Path, address: &str, sha: &str, close: bool, pushed: crate::ci::Pushed) -> crate::ci::Begin {
    let watch = crate::ci::Watch { address: address.to_string(), repo: repo.to_path_buf(), slug: "acme/app".to_string(), sha: sha.to_string(), started_at: 0, checked_at: None, close, runs: Vec::new(), state: crate::ci::State::Running, error: None, erring_since: None, pushed };
    crate::ci::save(state, &watch).unwrap();
    crate::ci::Begin::Watching
}

fn accept_all(state: &Path, address: &str, path: &Path, upstream: &str) {
    for open in super::super::accept::open_commits(path, upstream).unwrap() {
        super::super::accept::set_in(state, address, &open.key, true).unwrap();
    }
}

#[test]
fn a_column_rename_goes_out_in_three_stages_through_ci_a_not_yet_something_wrong_a_fix_your_verification_and_the_tasks_check() {
    use super::super::intent::Through;
    use super::super::push::{Out, deliver};
    let (dir, origin, slot, upstream) = task_with_a_commit("email");
    let state = state_of(&dir);
    let address = peers::address("email", &name_of(&dir));
    let task = Task { path: &slot.path, upstream: &upstream, address: &address };
    let gh = fake_gh(&dir);
    let open = || super::super::open_commits_of(&slot.path, &upstream).unwrap();
    let read = || plan::read_now(&state, &slot.path, &upstream, &address);
    let states = || read().stages.iter().map(|stage| stage.state).collect::<Vec<_>>();
    let later = || messages::now() + 120;
    plan::list_at(&state, &slot.path, &upstream, &address, &["Add the column", "Backfill", "Drop the old column"]).unwrap();
    plan::done_at(&state, &slot.path, &upstream, &address).unwrap();
    commit(&slot.path, "e", "Backfill the email column");
    plan::done_at(&state, &slot.path, &upstream, &address).unwrap();
    commit(&slot.path, "f", "Drop the old mail column");
    plan::done_at(&state, &slot.path, &upstream, &address).unwrap();
    declare_as(&Caller::User, &state, &task, &dir, &["Add the column:deploy".to_string(), "Backfill:verified:every row has its email in production".to_string(), "Drop the old column:check:no query reads users.mail".to_string()]).unwrap();
    for id in ["add-column", "backfill", "drop-old"] {
        assign_as(&Caller::User, &state, &task, id, &[id.to_string()]).unwrap();
    }
    accept_all(&state, &address, &slot.path, &upstream);
    assert_eq!((states(), read().pushable.map(|run| run.count)), (vec![StageState::Open, StageState::Locked, StageState::Locked], Some(1)), "every commit is accepted, and only the first stage can go");
    let past = refuse_past_stage(&Caller::User, &state, &address, "email", &slot.path, &upstream, Some(2), "Not pushed").unwrap_err();
    assert!(past.contains("Not pushed: Backfill waits until Add the column is met"), "{past}");

    let first = open()[0].clone();
    deliver(&dir, &slot, &upstream, Through::First { count: 1, of: 3 }, &Out { sha: first.clone(), rebased: false }, "the user", &state, &watching).unwrap();
    assert_eq!(git(&origin, &["rev-parse", "HEAD"]).unwrap(), first, "the first stage went out alone");
    assert_eq!((states()[0], crate::ci::load(&state, &address).map(|watch| watch.pushed.stages)), (StageState::Pushed, Some(vec!["add-column".to_string()])), "CI is followed for the stage");
    assert!(crate::ci::tick_with(&state, &gh, later()).is_empty() || states()[0] == StageState::Pushed, "no run yet: the stage waits");
    green(&dir, &first);
    crate::ci::tick_with(&state, &gh, later());
    assert_eq!(states(), [StageState::Met, StageState::Open, StageState::Locked], "CI green on the pushed commit meets the deploy and opens the backfill");
    assert!(crate::ci::load(&state, &address).is_none(), "and the watch is done");

    let second = open()[0].clone();
    deliver(&dir, &slot, &upstream, Through::First { count: 1, of: 2 }, &Out { sha: second.clone(), rebased: false }, "the user", &state, &watching).unwrap();
    assert!(crate::kebbie_questions::load(&state).is_empty(), "the verification waits for the stage to be out");
    green(&dir, &second);
    crate::ci::tick_with(&state, &gh, later());
    let asked = crate::kebbie_questions::load(&state);
    assert_eq!((asked.len(), states()[1]), (1, StageState::Waiting), "out: kebbie asks you");
    assert!(asked[0].text.contains("every row has its email in production"), "{}", asked[0].text);
    assert!(crate::questions::open_now(&messages::load()).iter().all(|question| question.from != address), "kebbie's question is not the task's own, so it holds none of its messages");

    let not_yet = crate::kebbie_questions::answer_in(&state, &Caller::User, &asked[0].id, Some(1), "", &|_| unreachable!("no close card here")).unwrap();
    assert!(not_yet.contains("Nothing changes"), "{not_yet}");
    assert_eq!((crate::kebbie_questions::load(&state).len(), states()[1]), (1, StageState::Waiting), "Not yet leaves the question and the stage as they are");
    crate::kebbie_questions::answer_in(&state, &Caller::User, &asked[0].id, Some(2), "half the rows still have no email", &|_| unreachable!("no close card here")).unwrap();
    assert_eq!((states(), read().stages[1].note.clone()), (vec![StageState::Met, StageState::Failed, StageState::Locked], Some("you said: half the rows still have no email".to_string())), "Something is wrong fails the stage and holds the drop");
    assert!(crate::kebbie_questions::load(&state).is_empty());
    assert!(messages::load().iter().any(|message| message.to == address && message.text.starts_with("Stage Backfill failed: you said: half the rows still have no email")), "the task hears it");
    assert_eq!(read().pushable, None, "nothing goes past a failed stage");

    plan::add_as(&Caller::Task(address.clone()), &state, &task, "Backfill the missed rows", "half the rows still had no email", None, Some("backfill"), 400).unwrap();
    let fix = commit(&slot.path, "g", "Backfill the rows the first run missed");
    plan::done_at(&state, &slot.path, &upstream, &address).unwrap();
    accept_all(&state, &address, &slot.path, &upstream);
    assert_eq!(read().out_of_stage.as_deref(), Some(fix.as_str()), "the fix sits after the drop, out of stage order");
    let drop = open()[0].clone();
    super::super::order::move_as(&state, &task, &fix, &drop, "the fix belongs to the backfill").unwrap();
    accept_all(&state, &address, &slot.path, &upstream);
    let moved = read();
    assert_eq!((moved.out_of_stage.clone(), moved.pushable.map(|run| run.count)), (None, Some(1)), "moved up, the fix is the stage's own to push");
    let fixed = open()[0].clone();
    deliver(&dir, &slot, &upstream, Through::First { count: 1, of: 2 }, &Out { sha: fixed.clone(), rebased: false }, "the user", &state, &watching).unwrap();
    assert_eq!((states()[1], read().stages[1].note.clone()), (StageState::Pushed, None), "the fix's push asks for the stage's requirement again");
    green(&dir, &fixed);
    crate::ci::tick_with(&state, &gh, later());
    let again = crate::kebbie_questions::load(&state);
    assert_eq!(again.len(), 1, "asked again");
    let verified = crate::kebbie_questions::answer_in(&state, &Caller::User, &again[0].id, Some(0), "", &|_| unreachable!("no close card here")).unwrap();
    assert!(verified.starts_with("Backfill is met; Drop the old column can be pushed"), "{verified}");
    assert_eq!(states(), [StageState::Met, StageState::Met, StageState::Open]);

    let last = open()[0].clone();
    let said = deliver(&dir, &slot, &upstream, Through::All, &Out { sha: last.clone(), rebased: false }, "the user", &state, &watching).unwrap();
    assert!(said.contains("closes the task once every stage is met"), "{said}");
    let watch = crate::ci::load(&state, &address).unwrap();
    assert_eq!((watch.close, watch.pushed.check.as_ref().map(|check| check.what.as_str())), (true, Some("no query reads users.mail")), "the last stage's check rides on the watch");
    green(&dir, &last);
    crate::ci::tick_with(&state, &gh, later());
    assert_eq!(states()[2], StageState::Waiting, "deployed: the task runs its check");
    assert!(messages::load().iter().any(|message| message.to == address && message.text.contains("Run the check you named for after the push now: no query reads users.mail")));
    let checking = crate::ci::load(&state, &address).unwrap();
    crate::ci::save(&state, &crate::ci::Watch { pushed: crate::ci::Pushed { check: Some(crate::ci::Check { passed: true, ..checking.pushed.check.clone().unwrap() }), ..checking.pushed.clone() }, ..checking }).unwrap();
    crate::ci::tick_with(&state, &gh, later());
    assert!(all_met(&state, &address), "the check held: every stage is met, and kebbie closes the task as after a green push");
    let _ = std::fs::remove_dir_all(dir.with_extension("gh"));
    let _ = super::super::pool::find_task(&dir, Some("email"), &dir).map(|slot| cleanup(dir.clone(), origin.clone(), &slot));
}
