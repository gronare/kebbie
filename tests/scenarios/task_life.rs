use std::time::Duration;

use crate::lab::{Lab, eventually, git, stderr, stdout};

fn lives(lab: &Lab, profile: &str) {
    let unnamed = lab.run_as("lead", &["-C", lab.app(), "start", "search", "Add the search page", "--profile", profile]);
    assert!(!unnamed.status.success() && stderr(&unnamed).contains("--recipe \"<recipe>: <why>\""), "{}", stderr(&unnamed));
    assert!(!lab.worktree(1).exists(), "a refused start claims no worktree");

    let started = lab.run_as("lead", &["-C", lab.app(), "start", "search", "Add the search page", "--profile", profile, "--recipe", "small-fix: one page and its test"]);
    assert!(started.status.success() && stdout(&started).starts_with("✓ Started"), "{}\n{}", stdout(&started), stderr(&started));
    let prompt = lab.prompts_to("search");
    assert!(prompt.len() == 1 && prompt[0].starts_with("Add the search page") && prompt[0].contains("Recipe: small fix, because one page and its test."), "the prompt reaches its agent once, with its recipe: {prompt:?}");
    let item = lab.item("search");
    assert_eq!((item["group"].as_str(), item["recipe"]["name"].as_str(), item["profile"].as_str()), (Some("WORKING"), Some("small-fix"), Some(profile)), "{item}");

    let worktree = lab.worktree(1);
    let slot = worktree.to_str().unwrap();
    lab.work(&worktree, "search.rb", "Add the search page");
    let report = lab.run(&["-C", slot, "report", "--state", "ready", "--next", "push and close", "search page done"]);
    assert!(report.status.success(), "{}", stderr(&report));
    assert!(stdout(&report).contains("An independent assessor reads your work"), "{}", stdout(&report));
    let item = lab.item("search");
    assert_eq!((item["group"].as_str(), item["report"]["state"].as_str(), item["assessment"].as_str()), (Some("READY TO PUSH"), Some("ready"), Some("review being assessed")), "{item}");

    lab.keeper_round();
    assert!(eventually(Duration::from_secs(120), || lab.item("search")["assessment"] == "no review: one small function and its test"), "{}\n{}", lab.item("search"), lab.keeper_log());
    let again = lab.run(&["-C", slot, "report", "--state", "ready", "--next", "push and close", "search page done"]);
    assert!(again.status.success() && !stdout(&again).contains("assessor"), "the same code is not assessed twice: {}", stdout(&again));
    lab.keeper_round();
    assert_eq!(lab.claude_roles().iter().filter(|role| *role == "assess").count(), 1, "{:?}", lab.claude_roles());

    let review = lab.run(&["-C", lab.app(), "review", "search"]);
    assert!(review.status.success() && stdout(&review).contains("Review started"), "v on the board: {}\n{}", stdout(&review), stderr(&review));
    assert!(eventually(Duration::from_secs(120), || lab.item("search")["verdict"]["verdict"] == "ok"), "{}", lab.item("search"));
    assert!(eventually(Duration::from_secs(120), || lab.to_lead().iter().any(|text| text.contains("search@app reviewed at"))), "the lead hears the verdict: {:?}", lab.to_lead());

    let pushed = lab.run_as_user(&["-C", lab.app(), "push", "search"]);
    assert!(pushed.status.success(), "{}\n{}", stdout(&pushed), stderr(&pushed));
    assert!(stdout(&pushed).contains("closed the task"), "without gh the push closes the task: {}", stdout(&pushed));
    assert_eq!(git(&lab.root.join("origin.git"), &["log", "-1", "--format=%s", "main"]), "Add the search page");
    assert!(git(&lab.app, &["branch", "--list", "search"]).is_empty(), "its branch is gone");
    assert!(!lab.on_board("search"), "and so is its card");
    assert!(lab.herdr_calls().iter().any(|call| call[..2] == ["workspace", "close"] && call[2] == "w_search"), "its Space closed");
}

#[test]
fn a_claude_task_runs_from_the_leads_start_through_its_report_review_and_push_to_closed() {
    let lab = Lab::new("claude-life");
    lives(&lab, "task");
    let start = lab.herdr_calls().into_iter().find(|call| call[..2] == ["agent", "start"]).unwrap();
    assert!(start.contains(&"claude".to_string()) && start.iter().any(|arg| arg == "--append-system-prompt-file"), "{start:?}");
    let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(lab.state.join("roles/search@app.settings.json")).unwrap()).unwrap();
    assert_eq!(settings["advisorModel"], "fable", "the task consults the first advisor the account takes");
}

#[test]
fn a_codex_profile_task_runs_the_same_life_once_kebbies_codex_plugin_is_set_up() {
    let lab = Lab::new("codex-life");
    lab.profile("coder", "agent = \"codex\"\nfor = \"Codex work\"\n");
    let unset = lab.run(&["-C", lab.app(), "start", "search", "x", "--profile", "coder"]);
    assert!(!unset.status.success() && stderr(&unset).contains("Codex is not set up: kebbie setup codex"), "{}", stderr(&unset));
    lab.ok(&["setup", "codex", "--yes"]);
    lives(&lab, "coder");
    let start = lab.herdr_calls().into_iter().find(|call| call[..2] == ["agent", "start"]).unwrap();
    assert!(start.contains(&"codex".to_string()) && start.iter().any(|arg| arg.contains("check_for_update_on_startup=false")), "{start:?}");
}
