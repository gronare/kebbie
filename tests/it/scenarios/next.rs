use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::scenarios::lab::{Lab, git, stderr, stdout};

fn ready(lab: &Lab, task: &str, subject: &str) -> PathBuf {
    let worktree = lab.start(task, subject, &[]);
    std::fs::create_dir_all(worktree.join("docs")).unwrap();
    lab.work(&worktree, &format!("docs/{task}.md"), subject);
    lab.ok(&["-C", worktree.to_str().unwrap(), "report", "--state", "ready", "--next", "push", subject]);
    worktree
}

fn rows(next: &str) -> Vec<(String, String, String)> {
    next.lines().filter(|line| line.starts_with(|character: char| character.is_ascii_digit()) && !line.contains(" working")).map(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        (words[1].to_string(), words[2].to_string(), words[3..].join(" "))
    }).collect()
}

fn group_of(lab: &Lab, group: &str) -> BTreeSet<String> {
    lab.status()["items"].as_array().unwrap().iter().filter(|item| item["group"] == group).filter_map(|item| item["name"].as_str().map(str::to_string)).collect()
}

// A question, three tasks ready to push (one a queued task waits on), one working, one queued and one parked.
fn mixed(lab: &Lab) -> PathBuf {
    let asking = lab.start("asking", "Pick the queue", &[]);
    lab.ok(&["-C", asking.to_str().unwrap(), "ask", "--option", "Sidekiq: reuse the jobs", "--option", "Solid Queue: one less service", "--recommend", "2", "Which queue?"]);
    let typo = ready(lab, "copy-typo", "Fix the typo");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ready(lab, "zeta", "Rename zeta");
    ready(lab, "card", "Add the card");
    lab.ok(&["-C", lab.app(), "start", "search", "Search the cards", "--after", "card"]);
    lab.start("busy", "Keep working", &[]);
    lab.start("later", "Park this", &[]);
    lab.ok(&["-C", lab.app(), "park", "later"]);
    typo
}

#[test]
fn kebbie_next_numbers_what_needs_you_in_the_boards_groups_with_what_others_wait_on_first() {
    let lab = Lab::new("next-order");
    mixed(&lab);
    let next = lab.ok(&["next"]);
    let shown = rows(&next);
    let order: Vec<&str> = shown.iter().map(|(_, name, _)| name.as_str()).collect();
    assert_eq!(order, ["asking", "card", "copy-typo", "zeta"], "{next}");
    assert_eq!(shown.iter().map(|(icon, _, _)| icon.as_str()).collect::<Vec<_>>(), ["?", "↑", "↑", "↑"], "{next}");
    assert!(shown[0].2.starts_with("answer its question") && shown[1].2.starts_with("push · ") && shown[1].2.contains(" waits on it") && shown[2].2.starts_with("push"), "{next}");
    assert!(next.lines().last().unwrap().starts_with("1 working · 1 queued · 1 parked"), "{next}");
    let stopped: BTreeSet<String> = shown.iter().filter(|(icon, _, _)| icon != "↑").map(|(_, name, _)| name.clone()).collect();
    let pushes: BTreeSet<String> = shown.iter().filter(|(icon, _, _)| icon == "↑").map(|(_, name, _)| name.clone()).collect();
    assert_eq!((stopped, pushes), (group_of(&lab, "NEEDS YOU"), group_of(&lab, "READY TO PUSH")), "the list holds what the board's groups hold, nothing more");
    for (group, count) in [("WORKING", 1), ("QUEUED", 1), ("PARKED", 1)] {
        assert_eq!(group_of(&lab, group).len(), count, "{group}");
    }
}

#[test]
fn pushing_in_kebbie_nexts_order_stops_at_the_first_refusal_and_leaves_the_rest_ready() {
    let lab = Lab::new("next-push");
    mixed(&lab);
    let hook = lab.root.join("origin.git/hooks/pre-receive");
    crate::executable::write(&hook, "#!/bin/sh\nwhile read old new ref; do git log --format=%s \"$old..$new\" | grep -q typo && { echo 'main takes no typo fixes today' >&2; exit 1; }; done\nexit 0\n").unwrap();
    let order: Vec<String> = rows(&lab.ok(&["next"])).into_iter().filter(|(icon, _, _)| icon == "↑").map(|(_, name, _)| name).collect();
    assert_eq!(order, ["card", "copy-typo", "zeta"]);
    let mut refused = None;
    for task in &order {
        let push = lab.run_as_user(&["-C", lab.app(), "push", task]);
        if !push.status.success() {
            refused = Some((task.clone(), stderr(&push)));
            break;
        }
        assert!(stdout(&push).contains("closed the task"), "{}", stdout(&push));
    }
    let (task, why) = refused.expect("copy-typo is refused");
    assert_eq!(task, "copy-typo");
    assert!(why.contains("Not pushed: git refused the push"), "{why}");
    let origin = git(&lab.root.join("origin.git"), &["log", "--format=%s", "main"]);
    assert!(origin.contains("Add the card") && !origin.contains("Fix the typo") && !origin.contains("Rename zeta"), "{origin}");
    assert_eq!(group_of(&lab, "READY TO PUSH"), ["zeta".to_string()].into(), "the one behind it stays ready");
    assert_eq!(lab.item("copy-typo")["group"], "NEEDS YOU", "{}", lab.item("copy-typo"));
}
