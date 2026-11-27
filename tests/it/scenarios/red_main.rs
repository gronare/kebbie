use std::fs;

use crate::scenarios::lab::{Lab, git, stderr, stdout};

fn on_github(lab: &Lab) {
    let origin = lab.root.join("origin.git");
    git(&lab.app, &["config", &format!("url.{}.insteadOf", origin.display()), "https://github.com/acme/app.git"]);
    git(&lab.app, &["remote", "set-url", "origin", "https://github.com/acme/app.git"]);
    let answers = lab.root.join("gh-answers");
    fs::create_dir_all(&answers).unwrap();
    crate::executable::write(lab.root.join("gh"), format!(
        "#!/bin/sh\nd='{}'\necho \"$*\" >> \"$d/gh.log\"\ncase \"$1 $2\" in\n  'auth status') exit 0 ;;\n  'run list') cat \"$d/gh-answers/run-list.json\" ;;\n  'run view') case \"$*\" in *--log-failed*) cat \"$d/gh-answers/log.txt\" ;; *) cat \"$d/gh-answers/jobs.json\" ;; esac ;;\n  *) exit 1 ;;\nesac\n",
        lab.root.display()
    )).unwrap();
    fs::write(answers.join("jobs.json"), r#"{"jobs":[{"name":"test","status":"completed","conclusion":"failure","databaseId":8}]}"#).unwrap();
    fs::write(answers.join("log.txt"), "test\tRun tests\trefunds_test.rb failed\n").unwrap();
}

fn ci(lab: &Lab, conclusion: &str) {
    fs::write(lab.root.join("gh-answers/run-list.json"), format!(r#"[{{"databaseId":42,"name":"CI","status":"completed","conclusion":"{conclusion}","workflowName":"CI","url":"https://github.com/acme/app/actions/runs/42"}}]"#)).unwrap();
}

fn green_test_run(lab: &Lab, worktree: &std::path::Path, address: &str) {
    use std::io::Write;
    let input = serde_json::json!({ "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": { "command": "cargo test" }, "tool_response": { "stdout": "ok", "stderr": "" }, "cwd": worktree, "duration_ms": 1200 });
    let mut hook = lab.command().args(["hook", "evidence", address]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn().unwrap();
    hook.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
    assert!(hook.wait().unwrap().success());
}

#[test]
fn red_ci_on_main_holds_the_repos_other_pushes_until_the_task_that_made_it_pushes_its_fix_green() {
    let lab = Lab::new("red-main");
    on_github(&lab);
    let billing = lab.start("billing", "Add refunds", &[]);
    let search = lab.start("search", "Search the invoices", &[]);
    ci(&lab, "failure");
    lab.work(&billing, "refunds.rb", "Add refunds");
    let pushed = lab.run_as_user(&["-C", lab.app(), "push", "billing"]);
    assert!(pushed.status.success(), "{}\n{}", stdout(&pushed), stderr(&pushed));
    let ticked = lab.run(&["ci", "tick"]);
    assert!(stdout(&ticked).contains("CI failed for billing@app"), "{}\n{}", stdout(&ticked), stderr(&ticked));

    lab.work(&search, "search.rb", "Search the invoices");
    let held = lab.run_as_user(&["-C", lab.app(), "push", "search"]);
    assert!(held.status.success() && stdout(&held).contains("held while CI is red on main") && stdout(&held).contains("It goes out once billing fixes it"), "{}\n{}", stdout(&held), stderr(&held));
    let origin = lab.root.join("origin.git");
    assert_eq!(git(&origin, &["log", "-1", "--format=%s", "main"]), "Add refunds", "search waits while main is red");

    lab.work(&billing, "refunds_fix.rb", "Fix the refunds test");
    green_test_run(&lab, &billing, "billing@app");
    let fix = lab.run_as_user(&["-C", lab.app(), "push", "billing"]);
    assert!(fix.status.success(), "{}\n{}", stdout(&fix), stderr(&fix));
    assert_eq!(git(&origin, &["log", "-1", "--format=%s", "main"]), "Fix the refunds test", "the fix goes out while main is red");
    ci(&lab, "success");
    let file = lab.state.join("ci/billing@app.json");
    let mut watch: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    watch["started_at"] = serde_json::json!(watch["started_at"].as_u64().unwrap() - 120);
    fs::write(&file, watch.to_string()).unwrap();
    let green = lab.run(&["ci", "tick"]);
    assert!(stdout(&green).contains("CI is green for"), "{}\n{}", stdout(&green), stderr(&green));
    lab.keeper_round();
    assert!(lab.keeper_log().contains("push: search@app: ↻ Not pushed yet: main moved under it, so its tests run first"), "the hold ended and the push waits only on its tests on the new main: {}", lab.keeper_log());
    green_test_run(&lab, &search, "search@app");
    let ready = lab.run_as("search@app", &["-C", search.to_str().unwrap(), "report", "--state", "ready", "--next", "push", "search done"]);
    assert!(ready.status.success(), "{}\n{}", stdout(&ready), stderr(&ready));
    lab.keeper_round();
    assert_eq!(git(&origin, &["log", "-1", "--format=%s", "main"]), "Search the invoices", "search goes out once main is green: {}\n{}", lab.keeper_log(), fs::read_to_string(lab.state.join("push-intents/search@app")).unwrap_or_default());
}
