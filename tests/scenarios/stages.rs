use std::fs;

use crate::lab::{Lab, git, stderr, stdout};

#[test]
fn a_task_in_stages_pushes_its_first_stage_alone_and_asks_you_for_the_deploy_kebbie_cannot_see() {
    let lab = Lab::new("stages");
    let worktree = lab.start("email", "Rename users.mail to users.email", &["--recipe", "rollout-in-stages: the old column goes once nothing reads it"]);
    let slot = worktree.to_str().unwrap();
    lab.ok(&["-C", slot, "subtasks", "Add the column", "Drop the old column"]);
    let first = lab.work(&worktree, "add.rb", "Add the email column");
    lab.ok(&["-C", slot, "subtasks", "done"]);
    lab.work(&worktree, "drop.rb", "Drop the mail column");
    lab.ok(&["-C", slot, "subtasks", "done"]);
    lab.ok(&["-C", slot, "stages", "Add the column:deploy", "Drop the old column:verified:nothing reads users.mail"]);
    lab.ok(&["-C", slot, "stages", "assign", "add-column", "add-column"]);
    lab.ok(&["-C", slot, "stages", "assign", "drop-old", "drop-old"]);
    for subtask in ["add-column", "drop-old"] {
        let accepted = lab.run_as_user(&["-C", lab.app(), "accept", "email", "--subtask", subtask]);
        assert!(accepted.status.success(), "{}\n{}", stdout(&accepted), stderr(&accepted));
    }
    let item = lab.item("email");
    assert_eq!(item["stages"].as_array().unwrap().iter().map(|stage| stage["state"].as_str().unwrap()).collect::<Vec<_>>(), ["open", "locked"], "{item}");
    assert_eq!(item["pushable"]["count"], 1, "every commit is accepted, and the run stops at the drop's stage: {item}");

    let all = lab.run_as_user(&["-C", lab.app(), "push", "email"]);
    assert!(!all.status.success() && (stdout(&all) + &stderr(&all)).contains("Not pushed: Drop the old column waits until Add the column is met"), "{}\n{}", stdout(&all), stderr(&all));
    let pushed = lab.run_as_user(&["-C", lab.app(), "push", "email", "--through", &first]);
    assert!(pushed.status.success(), "{}\n{}", stdout(&pushed), stderr(&pushed));
    assert_eq!(git(&lab.root.join("origin.git"), &["log", "-1", "--format=%s", "main"]), "Add the email column", "the first stage went out alone");
    let item = lab.item("email");
    assert_eq!(item["stages"][0]["state"], "waiting", "{item}");
    assert!(item["summary"].as_str().unwrap().contains("stage 1 of 2 pushed · waits on your word that it deployed"), "{}", item["summary"]);
    let asked: serde_json::Value = serde_json::from_str(&fs::read_to_string(lab.state.join("kebbie-questions.json")).unwrap()).unwrap();
    assert!(asked[0]["text"].as_str().unwrap().contains("cannot see it deploy (CI is not watched: origin is not on GitHub)"), "origin is a local path, so kebbie asks you: {asked}");
    assert!(lab.on_board("email"), "the task stays open for its next stage");
}
