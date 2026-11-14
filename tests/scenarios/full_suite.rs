use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use crate::lab::{Lab, git, stderr, stdout};

// What a task's Bash hook hands kebbie once a command ended, as Claude Code sends it.
fn ran(lab: &Lab, worktree: &Path, command: &str, exit: Option<i64>) {
    let input = match exit {
        None => serde_json::json!({ "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": { "command": command }, "tool_response": { "stdout": "ok", "stderr": "" }, "cwd": worktree, "duration_ms": 1200 }),
        Some(code) => serde_json::json!({ "hook_event_name": "PostToolUseFailure", "tool_name": "Bash", "tool_input": { "command": command }, "error": format!("Exit code {code}\nFAILED"), "cwd": worktree }),
    };
    let mut hook = lab.command().args(["hook", "evidence", "billing@app"]).stdin(Stdio::piped()).stdout(Stdio::null()).spawn().unwrap();
    hook.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
    assert!(hook.wait().unwrap().success());
}

#[test]
fn a_repo_on_the_full_suite_takes_a_ready_report_and_a_push_only_after_a_green_full_run_on_head() {
    let lab = Lab::new("full-suite");
    lab.config("[tests]\nbefore_push = \"full\"\n");
    let repos = lab.home.join(".config/kebbie/repos");
    fs::create_dir_all(&repos).unwrap();
    fs::write(repos.join("app.toml"), "[tests]\nfull_command = \"scripts/check.sh\"\n").unwrap();
    let billing = lab.start("billing", "Add refunds", &[]);
    let slot = billing.to_str().unwrap();
    let role = fs::read_to_string(lab.state.join("roles/billing@app.md")).unwrap();
    assert!(role.contains("run `scripts/check.sh` as a command of its own"), "the task is told to run the full suite before ready");
    lab.work(&billing, "refunds.rb", "Add refunds");

    ran(&lab, &billing, "scripts/check.sh billing", None);
    let refused = lab.run_as("billing@app", &["-C", slot, "report", "--state", "ready", "--next", "push", "refunds done"]);
    assert!(!refused.status.success() && stderr(&refused).contains("Run `scripts/check.sh` in your worktree") && stderr(&refused).contains("kebbie recorded no run of it"), "a filtered run is not the full suite: {}", stderr(&refused));
    let pushed = lab.run_as_user(&["-C", lab.app(), "push", "billing"]);
    assert!(!pushed.status.success() && stderr(&pushed).contains("its full suite has no green run on HEAD") && stderr(&pushed).contains("`scripts/check.sh`"), "{}\n{}", stdout(&pushed), stderr(&pushed));
    let lead = lab.run_as("lead", &["-C", lab.app(), "push", "billing"]);
    assert!(!lead.status.success(), "the lead's push waits on it too: {}", stdout(&lead));

    ran(&lab, &billing, "./scripts/check.sh", Some(1));
    let red = lab.run_as("billing@app", &["-C", slot, "report", "--state", "ready", "--next", "push", "refunds done", "--red-cause", "test_refund_rounding fails on main too"]);
    assert!(!red.status.success() && stderr(&red).contains("its latest run failed at"), "a named red cause does not stand in for the full run on HEAD: {}", stderr(&red));

    ran(&lab, &billing, "cd . && ./scripts/check.sh -q", None);
    let ready = lab.run_as("billing@app", &["-C", slot, "report", "--state", "ready", "--next", "push", "refunds done"]);
    assert!(ready.status.success(), "{}", stderr(&ready));

    lab.work(&billing, "refunds.md", "Document refunds");
    let stale = lab.run_as_user(&["-C", lab.app(), "push", "billing"]);
    assert!(!stale.status.success() && stderr(&stale).contains("HEAD holds other code since"), "a commit after the run makes it stale: {}", stderr(&stale));
    ran(&lab, &billing, "scripts/check.sh", None);
    let went = lab.run_as_user(&["-C", lab.app(), "push", "billing"]);
    assert!(went.status.success(), "{}\n{}", stdout(&went), stderr(&went));
    assert_eq!(git(&lab.root.join("origin.git"), &["log", "-1", "--format=%s", "main"]), "Document refunds");
}

#[test]
fn a_repo_on_related_tests_pushes_as_before_whatever_its_full_command() {
    let lab = Lab::new("related-suite");
    let repos = lab.home.join(".config/kebbie/repos");
    fs::create_dir_all(&repos).unwrap();
    fs::write(repos.join("app.toml"), "[tests]\nfull_command = \"scripts/check.sh\"\n").unwrap();
    let billing = lab.start("billing", "Add refunds", &[]);
    lab.work(&billing, "refunds.rb", "Add refunds");
    let went = lab.run_as_user(&["-C", lab.app(), "push", "billing"]);
    assert!(went.status.success(), "{}\n{}", stdout(&went), stderr(&went));
}
