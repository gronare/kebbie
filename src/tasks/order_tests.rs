use super::*;
use crate::tasks::test_support::*;

fn subjects(path: &Path, upstream: &str) -> Vec<String> {
    git(path, &["log", "--reverse", "--format=%s", &format!("{}..HEAD", crate::repo::tracking(upstream))]).unwrap().lines().map(str::to_string).collect()
}

fn sha_of(path: &Path, subject: &str) -> String {
    git(path, &["log", "--format=%H", "--grep", &format!("^{subject}$")]).unwrap().lines().next().unwrap().to_string()
}

fn read(state: &Path, path: &Path, upstream: &str) -> Subtasks {
    super::super::plan::read_now(state, path, upstream, "billing@app")
}

#[test]
fn a_clean_move_keeps_each_commits_change_subtask_acceptance_and_rules_approval_and_notes_where_it_came_from() {
    let (dir, origin, slot, upstream) = task_with_a_commit("billing");
    let state = state_of(&dir);
    let task = Task { path: &slot.path, upstream: &upstream, address: "billing@app" };
    super::super::plan::list_at(&state, &slot.path, &upstream, "billing@app", &["Model", "Docs"]).unwrap();
    commit(&slot.path, "e", "fifth");
    super::super::plan::done_at(&state, &slot.path, &upstream, "billing@app").unwrap();
    commit(&slot.path, "f", "sixth");
    let before = read(&state, &slot.path, &upstream);
    let codes: Vec<String> = open_commits(&slot.path, &upstream).unwrap().into_iter().map(|open| open.code).collect();
    let fifth = open_commits(&slot.path, &upstream).unwrap().into_iter().find(|open| open.sha == sha_of(&slot.path, "fifth")).unwrap();
    super::super::accept::set_in(&state, "billing@app", &fifth.key, true).unwrap();
    let fingerprint = super::super::approval::fingerprint(&slot.path, &upstream).unwrap();
    super::super::approval::record_approval(&state, "billing@app", &fingerprint, super::super::approval::ApprovedBy::Reviewer).unwrap();

    let sixth = sha_of(&slot.path, "sixth");
    let said = move_as(&state, &task, &sixth[..8], "top", "it prepares the model").unwrap();
    assert!(said.starts_with(&format!("Moved {} to the top", &sixth[..7])), "{said}");
    assert_eq!(subjects(&slot.path, &upstream), ["sixth", "fourth", "fifth"]);
    let mut after: Vec<String> = open_commits(&slot.path, &upstream).unwrap().into_iter().map(|open| open.code).collect();
    after.sort();
    let mut was = codes.clone();
    was.sort();
    assert_eq!(after, was, "a clean move changes no commit's change");
    let now = read(&state, &slot.path, &upstream);
    let owner = |subtasks: &Subtasks, subject: &str| subtasks.order.iter().find(|row| row.subject == subject).map(|row| (row.subtask.clone(), row.accepted));
    for subject in ["fourth", "fifth", "sixth"] {
        assert_eq!(owner(&now, subject), owner(&before, subject).map(|(subtask, _)| (subtask, subject == "fifth")), "{subject} keeps its subtask, and fifth its acceptance");
    }
    assert_eq!(now.order[0].moved.as_ref().map(|moved| (moved.from, moved.reason.as_str())), Some((3, "it prepares the model")));
    assert!(super::super::approval::approval_holds(&state, "billing@app", &super::super::approval::fingerprint(&slot.path, &upstream).unwrap()), "the rules approval covers the same commits in a new order");
    commit(&slot.path, "f", "a changed line");
    assert!(!super::super::approval::approval_holds(&state, "billing@app", &super::super::approval::fingerprint(&slot.path, &upstream).unwrap()), "and falls with a commit the lead never saw");
    cleanup(dir, origin, &slot);
}

#[test]
fn a_move_that_conflicts_leaves_the_branch_as_it_was_and_names_the_commit_and_files() {
    let (dir, origin, slot, upstream) = task_with_a_commit("billing");
    let state = state_of(&dir);
    let task = Task { path: &slot.path, upstream: &upstream, address: "billing@app" };
    super::super::plan::list_at(&state, &slot.path, &upstream, "billing@app", &["Model"]).unwrap();
    commit(&slot.path, "e", "fifth");
    let sixth = commit(&slot.path, "e", "sixth");
    let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
    let refused = move_as(&state, &task, &sixth, "top", "it goes first").unwrap_err();
    assert!(refused.contains("does not apply in the new order") && refused.contains("e"), "{refused}");
    assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), head, "the branch is put back");
    assert!(!crate::repo::rebase_in_progress(&slot.path) && git(&slot.path, &["status", "--porcelain"]).unwrap().is_empty(), "nothing is left half-done");
    assert!(read(&state, &slot.path, &upstream).order.iter().all(|row| row.moved.is_none()), "a refused move records nothing");
    cleanup(dir, origin, &slot);
}

#[test]
fn a_move_is_refused_while_a_push_of_the_task_waits_in_line_or_tracked_files_are_uncommitted() {
    let (dir, origin, slot, upstream) = task_with_a_commit("billing");
    let state = state_of(&dir);
    let task = Task { path: &slot.path, upstream: &upstream, address: "billing@app" };
    super::super::plan::list_at(&state, &slot.path, &upstream, "billing@app", &["Model"]).unwrap();
    let fifth = commit(&slot.path, "e", "fifth");
    let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
    std::fs::write(slot.path.join("d"), "changed").unwrap();
    let dirty = move_as(&state, &task, &fifth, "top", "first").unwrap_err();
    assert!(dirty.contains("Not moved: it has 1 uncommitted file"), "{dirty}");
    git(&slot.path, &["checkout", "--", "d"]).unwrap();
    let intent = super::super::intent::Intent { by: super::super::intent::By::User, work: super::super::intent::Work::Turn, address: "billing@app".to_string(), repo: dir.clone(), task: "billing".to_string(), fingerprint: "x".to_string(), through: super::super::intent::Through::All, at: 1, seen: None, hand: None, queued: Some(1) };
    super::super::intent::save(&state, &intent).unwrap();
    let frozen = move_as(&state, &task, &fifth, "top", "first").unwrap_err();
    assert!(frozen.contains("push of billing@app waits in line") && frozen.contains("kebbie push billing --cancel"), "{frozen}");
    assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), head);
    assert!(move_as(&state, &task, &fifth, &fifth, "x").unwrap_err().contains("goes with"), "a commit is not moved before itself");
    cleanup_intents(&state, "billing@app");
    cleanup(dir, origin, &slot);
}

#[test]
fn a_group_gathers_its_commits_moves_as_one_and_ungroups() {
    let (dir, origin, slot, upstream) = task_with_a_commit("billing");
    let state = state_of(&dir);
    let task = Task { path: &slot.path, upstream: &upstream, address: "billing@app" };
    super::super::plan::list_at(&state, &slot.path, &upstream, "billing@app", &["Model"]).unwrap();
    commit(&slot.path, "e", "fifth");
    commit(&slot.path, "f", "sixth");
    let fourth = sha_of(&slot.path, "fourth");
    let sixth = sha_of(&slot.path, "sixth");
    assert!(group_as(&state, &task, std::slice::from_ref(&fourth), "x").unwrap_err().contains("two commits or more"));
    let said = group_as(&state, &task, &[sixth.clone(), fourth.clone()], "the model and its migration go out together").unwrap();
    assert!(said.contains("as g1: kebbie moved them together"), "{said}");
    assert_eq!(subjects(&slot.path, &upstream), ["fourth", "sixth", "fifth"]);
    let grouped = read(&state, &slot.path, &upstream);
    assert_eq!(grouped.order.iter().map(|row| row.group.as_deref()).collect::<Vec<_>>(), [Some("g1"), Some("g1"), None]);
    let fifth = sha_of(&slot.path, "fifth");
    let sixth = sha_of(&slot.path, "sixth");
    assert!(move_as(&state, &task, &sixth, &fifth, "x").unwrap().contains("is there already"));
    move_as(&state, &task, &fifth, &sixth, "the docs first").unwrap();
    assert_eq!(subjects(&slot.path, &upstream), ["fifth", "fourth", "sixth"], "moving before a group's later commit puts it before the whole group");
    move_as(&state, &task, &sha_of(&slot.path, "sixth"), "top", "the model first").unwrap();
    assert_eq!(subjects(&slot.path, &upstream), ["fourth", "sixth", "fifth"], "a group member moves with its group");
    assert!(group_as(&state, &task, &[sha_of(&slot.path, "fifth"), sha_of(&slot.path, "sixth")], "x").unwrap_err().contains("in the group g1 already"));
    assert!(ungroup_as(&state, &task, "g1").unwrap().contains("Ungrouped g1"));
    assert!(read(&state, &slot.path, &upstream).order.iter().all(|row| row.group.is_none()));
    assert!(ungroup_as(&state, &task, "g1").is_err());
    cleanup(dir, origin, &slot);
}

#[test]
fn reordered_lifts_a_block_out_and_puts_it_before_its_target() {
    let order: Vec<String> = ["a", "b", "c", "d"].iter().map(|one| one.to_string()).collect();
    assert_eq!(reordered(&order, &["c".to_string(), "d".to_string()], Some("b")), ["a", "c", "d", "b"]);
    assert_eq!(reordered(&order, &["b".to_string()], None), ["b", "a", "c", "d"]);
}
