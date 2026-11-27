use std::fs;
use std::io::Write;
use std::process::Stdio;

use crate::scenarios::lab::{Lab, stderr, stdout};

#[test]
fn a_task_build_on_a_disk_below_the_limit_never_runs_and_the_task_is_told_to_free_space_not_to_wait() {
    let lab = Lab::new("disk-full");
    fs::write(&lab.config, format!("{}[disk]\ntrim_below = \"90%\"\nbuilds_below = \"89%\"\n", fs::read_to_string(&lab.config).unwrap())).unwrap();
    crate::executable::write(lab.root.join("bin/cargo"), format!("#!/bin/sh\ntouch '{}'\n", lab.root.join("cargo-ran").display())).unwrap();
    let billing = lab.start("billing", "Add refunds", &[]);
    lab.keeper_round();

    let built = lab.command().current_dir(&billing).args(["build-guard", "cargo", "build"]).output().unwrap();
    assert_eq!(built.status.code(), Some(75), "{}\n{}", stdout(&built), stderr(&built));
    assert!(stderr(&built).starts_with("error: kebbie: the disk under this worktree has") && stderr(&built).contains("(disk.builds_below)"), "{}", stderr(&built));
    assert!(!lab.root.join("cargo-ran").exists(), "the build never starts");

    let input = serde_json::json!({ "hook_event_name": "PostToolUseFailure", "tool_name": "Bash", "tool_input": { "command": "cargo test --locked" }, "error": format!("Exit code 75\n{}", stderr(&built)), "cwd": billing });
    let mut hook = lab.command().args(["hook", "evidence", "billing@app"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    hook.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
    let told = hook.wait_with_output().unwrap();
    assert!(stdout(&told).contains("the disk under the worktree was too full for a build, so it never ran; free space on that disk, then run it again"), "{}\n{}", stdout(&told), stderr(&told));
    assert!(!stdout(&told).contains("tells it when its turn comes"), "a task the disk held back waits for no turn: {}", stdout(&told));
    let runs = fs::read_to_string(lab.state.join("evidence/billing@app.jsonl")).unwrap_or_default();
    assert!(!runs.contains("cargo test"), "a build that never ran is no test run: {runs}");
}
