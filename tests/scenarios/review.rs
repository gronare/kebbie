use std::path::PathBuf;
use std::time::Duration;

use crate::lab::{Lab, eventually, git, stderr, stdout};

const NEEDED: &str = "needed: yes\nreason: changes how refunds round";

fn assessed(name: &str, mode: &str) -> (Lab, PathBuf) {
    let lab = Lab::new(name);
    lab.config(&format!("[review]\nauto = \"{mode}\"\n"));
    lab.answer("assess", NEEDED);
    let worktree = lab.start("billing", "Round refunds", &[]);
    lab.work(&worktree, "refunds.rb", "Round refunds to the cent");
    lab.ok(&["-C", worktree.to_str().unwrap(), "report", "--state", "ready", "--next", "push", "refunds round"]);
    lab.keeper_round();
    (lab, worktree)
}

fn pushed(lab: &Lab) -> std::process::Output {
    lab.run_as_user(&["-C", lab.app(), "push", "billing"])
}

fn on_origin(lab: &Lab) -> String {
    git(&lab.root.join("origin.git"), &["log", "-1", "--format=%s", "main"])
}

#[test]
fn under_suggest_an_assessor_that_asks_for_a_review_only_suggests_it_and_the_push_stays_free() {
    let (lab, _) = assessed("review-suggest", "suggest");
    assert!(eventually(Duration::from_secs(20), || lab.item("billing")["assessment"] == "review suggested: changes how refunds round"), "{}", lab.item("billing"));
    assert!(lab.to_lead().iter().any(|text| text.contains("billing@app: review suggested: changes how refunds round. Send it to review with kebbie review")), "{:?}", lab.to_lead());
    lab.keeper_round();
    assert!(!lab.claude_roles().contains(&"review".to_string()), "no reviewer runs on a suggestion: {:?}", lab.claude_roles());
    let push = pushed(&lab);
    assert!(push.status.success(), "{}", stderr(&push));
    assert_eq!(on_origin(&lab), "Round refunds to the cent");
}

#[test]
fn under_run_the_reviewer_reads_the_task_at_once_while_its_push_stays_free() {
    let (lab, _) = assessed("review-run", "run");
    assert!(eventually(Duration::from_secs(20), || lab.item("billing")["assessment"] == "review: changes how refunds round"), "{}", lab.item("billing"));
    lab.answer("review", "verdict: changes\n- refunds.rb:1 — rounds half down");
    lab.keeper_round();
    assert!(eventually(Duration::from_secs(20), || lab.item("billing")["verdict"]["verdict"] == "changes"), "{}", lab.item("billing"));
    let push = pushed(&lab);
    assert!(push.status.success(), "an advised review holds nothing: {}\n{}", stdout(&push), stderr(&push));
    assert_eq!(on_origin(&lab), "Round refunds to the cent");
}

#[test]
fn under_require_the_push_waits_for_the_reviewers_pass() {
    let (lab, worktree) = assessed("review-require", "require");
    assert!(eventually(Duration::from_secs(20), || lab.item("billing")["assessment"] == "review: changes how refunds round"), "{}", lab.item("billing"));
    lab.answer("review", "verdict: changes\n- refunds.rb:1 — rounds half down");
    lab.keeper_round();
    assert!(eventually(Duration::from_secs(20), || lab.item("billing")["verdict"]["verdict"] == "changes"), "{}", lab.item("billing"));
    let held = pushed(&lab);
    assert!(!held.status.success() && stderr(&held).contains("Not pushed"), "{}\n{}", stdout(&held), stderr(&held));
    assert_eq!(on_origin(&lab), "first");
    lab.work(&worktree, "refunds.rb", "Round refunds half up");
    lab.answer("review", "verdict: ok");
    lab.ok(&["-C", lab.app(), "review", "billing"]);
    assert!(eventually(Duration::from_secs(20), || lab.item("billing")["verdict"]["verdict"] == "ok"), "{}", lab.item("billing"));
    let push = pushed(&lab);
    assert!(push.status.success(), "{}\n{}", stdout(&push), stderr(&push));
    assert_eq!(on_origin(&lab), "Round refunds half up");
}

#[test]
fn under_require_an_assessor_that_dies_settles_as_not_assessed_and_holds_the_push_no_longer() {
    let lab = Lab::new("review-dead-assessor");
    lab.config("[review]\nauto = \"require\"\n");
    std::fs::write(lab.root.join("claude/assess.die"), "").unwrap();
    let worktree = lab.start("billing", "Round refunds", &[]);
    lab.work(&worktree, "refunds.rb", "Round refunds to the cent");
    lab.ok(&["-C", worktree.to_str().unwrap(), "report", "--state", "ready", "--next", "push", "refunds round"]);
    lab.keeper_round();
    assert!(eventually(Duration::from_secs(10), || lab.claude_roles().contains(&"assess".to_string())), "the assessor ran");
    lab.keeper_round();
    assert!(eventually(Duration::from_secs(10), || lab.keeper_log().contains("review: billing@app: the assessor stopped without an answer")), "{}", lab.keeper_log());
    assert_eq!(lab.item("billing")["assessment"], "review not assessed");
    let push = pushed(&lab);
    assert!(push.status.success(), "a dead assessor holds no push for good: {}\n{}", stdout(&push), stderr(&push));
}

#[test]
fn under_require_a_review_that_gives_no_verdict_keeps_the_push_held() {
    let (lab, _) = assessed("review-no-verdict", "require");
    assert!(eventually(Duration::from_secs(20), || lab.item("billing")["assessment"] == "review: changes how refunds round"), "{}", lab.item("billing"));
    lab.answer("review", "I read the code and have nothing to add.");
    lab.keeper_round();
    assert!(eventually(Duration::from_secs(20), || lab.to_lead().iter().any(|text| text.contains("The review of billing@app failed"))), "{:?}", lab.to_lead());
    assert!(lab.to_lead().iter().any(|text| text.contains("Its push stays held until a review passes")), "{:?}", lab.to_lead());
    let held = pushed(&lab);
    assert!(!held.status.success() && stderr(&held).contains("Not pushed"), "a failed review lets nothing through: {}\n{}", stdout(&held), stderr(&held));
    assert_eq!(on_origin(&lab), "first");
}

#[test]
fn a_second_opinion_runs_beside_the_review_reaches_the_lead_and_never_holds_the_push() {
    let lab = Lab::new("review-second");
    lab.profile("second-look", "kind = \"review\"\nmodel = \"haiku\"\n");
    lab.config("[review]\nsecond = \"second-look\"\n");
    lab.answer("review-haiku", "verdict: changes\n- refunds.rb:1 — name the rounding mode");
    let worktree = lab.start("billing", "Round refunds", &[]);
    lab.work(&worktree, "refunds.rb", "Round refunds to the cent");
    let sent = lab.ok(&["-C", lab.app(), "review", "billing", "--second"]);
    assert!(sent.contains("second opinion: second-look"), "{sent}");
    assert!(eventually(Duration::from_secs(20), || lab.item("billing")["second_opinion"]["points"] == 1 && lab.item("billing")["verdict"]["verdict"] == "ok"), "{}", lab.item("billing"));
    let told = lab.to_lead().into_iter().find(|text| text.contains("second opinion from second-look")).unwrap_or_else(|| panic!("{:?}", lab.to_lead()));
    assert!(told.contains("1 point") && told.contains("A second opinion never holds the push"), "{told}");
    let models: Vec<String> = lab.claude_calls().iter().filter_map(|call| call.split(" --model ").nth(1).map(str::to_string)).collect();
    assert_eq!(models, ["haiku"], "the built-in reviewer passes no model, the second its own: {:?}", lab.claude_calls());
    let push = pushed(&lab);
    assert!(push.status.success(), "{}\n{}", stdout(&push), stderr(&push));
    assert_eq!(on_origin(&lab), "Round refunds to the cent");
}

#[test]
fn review_profile_names_the_reviewer_the_keeper_starts_for_a_ready_task() {
    let lab = Lab::new("review-profile");
    lab.profile("careful", "kind = \"review\"\nmodel = \"opus\"\neffort = \"high\"\n");
    lab.config("[review]\nauto = \"require\"\nprofile = \"careful\"\n");
    lab.answer("assess", NEEDED);
    lab.answer("review-opus", "verdict: ok");
    let worktree = lab.start("billing", "Round refunds", &[]);
    lab.work(&worktree, "refunds.rb", "Round refunds to the cent");
    lab.ok(&["-C", worktree.to_str().unwrap(), "report", "--state", "ready", "--next", "push", "refunds round"]);
    lab.keeper_round();
    assert!(eventually(Duration::from_secs(20), || lab.item("billing")["assessment"] == "review: changes how refunds round"), "{}", lab.item("billing"));
    lab.keeper_round();
    assert!(eventually(Duration::from_secs(20), || lab.item("billing")["verdict"]["verdict"] == "ok"), "{}\n{}", lab.item("billing"), lab.keeper_log());
    let review = lab.claude_calls().into_iter().find(|call| call.starts_with("review ")).expect("the reviewer ran");
    assert!(review.contains("--model opus") && review.contains("--effort high"), "careful's model and effort: {review}");
    assert!(lab.keeper_log().contains("review: started the reviewer for billing@app"), "{}", lab.keeper_log());
    let push = pushed(&lab);
    assert!(push.status.success(), "{}\n{}", stdout(&push), stderr(&push));
}

#[test]
fn kebbie_review_refuses_at_once_a_review_profile_or_second_opinion_that_is_no_review_profile_and_the_doctor_warns() {
    let lab = Lab::new("review-profile-task");
    lab.profile("designer", "model = \"opus\"\n");
    lab.config("[review]\nprofile = \"designer\"\n");
    let worktree = lab.start("billing", "Round refunds", &[]);
    lab.work(&worktree, "refunds.rb", "Round refunds to the cent");
    let refused = lab.run(&["-C", lab.app(), "review", "billing"]);
    assert!(!refused.status.success() && !stdout(&refused).contains("Review started"), "{}", stdout(&refused));
    assert!(stderr(&refused).contains("review.profile: \"designer\" is a task's profile; a reviewer is a profile with kind = \"review\""), "{}", stderr(&refused));
    let doctor = stdout(&lab.run(&["-C", lab.app(), "doctor"]));
    assert!(doctor.lines().any(|line| line.starts_with("warn") && line.contains("reviewer") && line.contains("review.profile: \"designer\" is a task's profile")), "{doctor}");

    lab.config("[review]\nsecond = \"gone-look\"\n");
    let unknown = lab.run(&["-C", lab.app(), "review", "billing", "--second"]);
    assert!(!unknown.status.success() && stderr(&unknown).contains("review.second: No profile \"gone-look\""), "{}", stderr(&unknown));
    assert!(stdout(&lab.run(&["-C", lab.app(), "doctor"])).contains("review.second: No profile \"gone-look\""));
    let plain = lab.run(&["-C", lab.app(), "review", "billing"]);
    assert!(plain.status.success() && stdout(&plain).contains("Review started"), "a second opinion only offered stops no plain review: {}", stderr(&plain));
    assert!(lab.claude_calls().iter().all(|call| !call.contains("designer")), "no reviewer ran on a task's profile");
}

#[test]
fn a_review_that_finds_a_new_point_each_round_goes_back_twice_by_itself_and_then_waits_for_you() {
    let lab = Lab::new("review-loop");
    lab.config("[review]\nauto = \"suggest\"\n\n[investigate]\nprofile = \"none\"\n");
    let worktree = lab.start("billing", "Round refunds", &[]);
    let mut head = lab.work(&worktree, "refunds.rb", "Round refunds to the cent");
    let round = |n: u32| format!("verdict: changes\n\n- refunds.rb:{n} — case {n} rounds half down\n- scope: refunds.rb:{} — point {n} lies outside the intent", 100 + n);
    let reviewed_at = |head: &str| lab.item("billing")["verdict"]["sha"] == head[..7];
    lab.answer("review", &round(1));
    lab.ok(&["-C", lab.app(), "review", "billing"]);
    assert!(eventually(Duration::from_secs(20), || reviewed_at(&head)), "{}", lab.item("billing"));
    for n in 2..=4 {
        head = lab.work(&worktree, &format!("case{n}.rb"), &format!("Round case {n} half up"));
        lab.answer("review", &round(n));
        lab.ok(&["-C", worktree.to_str().unwrap(), "report", "--state", "ready", "--next", "push", &format!("case {} fixed", n - 1)]);
        lab.keeper_round();
        if n < 4 {
            assert!(eventually(Duration::from_secs(20), || reviewed_at(&head)), "round {n}: {}\n{}", lab.item("billing"), lab.keeper_log());
        }
    }
    let reviews = lab.claude_roles().iter().filter(|role| *role == "review").count();
    assert_eq!(reviews, 3, "the one you sent and two it went back for: {:?}", lab.claude_roles());
    let item = lab.item("billing");
    assert_eq!((item["group"].as_str(), item["next_for_you"].as_str(), item["verdict"]["stopped"].as_bool()), (Some("NEEDS YOU"), Some("the review keeps finding new points; read it yourself or push"), Some(true)), "{item}");
    assert!(lab.to_lead().iter().any(|text| text.contains("billing@app's review was sent back 2 times by itself")), "{:?}", lab.to_lead());
    lab.ok(&["-C", worktree.to_str().unwrap(), "report", "--state", "ready", "--next", "push", "still ready"]);
    lab.keeper_round();
    assert_eq!(lab.claude_roles().iter().filter(|role| *role == "review").count(), 3, "a stopped review stays stopped");
    let push = pushed(&lab);
    assert!(push.status.success(), "{}\n{}", stdout(&push), stderr(&push));
    assert_eq!(on_origin(&lab), "Round case 4 half up");
}
