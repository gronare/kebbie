use std::fs;
use std::path::Path;

use serde_json::json;

use crate::scenarios::lab::{Lab, stdout};

fn with_session(lab: &Lab, task: &str, worktree: &Path, codex: bool) {
    let session = format!("s-{task}");
    lab.agent(task, |agent| {
        agent["agent_session"] = json!({ "value": session });
        agent["cwd"] = json!(worktree.display().to_string());
    });
    fs::write(lab.root.join(format!("cwd-p_{task}")), worktree.display().to_string()).unwrap();
    let line = "{\"type\":\"user\",\"timestamp\":\"2026-01-01T00:00:00.000Z\",\"message\":{\"content\":\"hi\"}}\n";
    let file = if codex {
        lab.home.join(".codex/sessions/2026/10/31").join(format!("rollout-2026-10-31T00-00-00-{session}.jsonl"))
    } else {
        let folder: String = worktree.to_string_lossy().chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '-' }).collect();
        lab.home.join(".claude/projects").join(folder).join(format!("{session}.jsonl"))
    };
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, line).unwrap();
}

// What kebbie update leaves behind: an update time after each session's start, and each task's record of the older kebbie it began on.
fn kebbie_updated(lab: &Lab) {
    let later = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 60;
    fs::create_dir_all(lab.state.join("restart")).unwrap();
    fs::write(lab.state.join("restart/updated"), later.to_string()).unwrap();
    for entry in fs::read_dir(lab.state.join("roles")).unwrap().flatten().filter(|entry| entry.file_name().to_string_lossy().ends_with(".known.json")) {
        let mut record: serde_json::Value = serde_json::from_str(&fs::read_to_string(entry.path()).unwrap()).unwrap();
        record["started"] = json!("kebbie 0.29.0 (0123abc 2026-10-01)");
        fs::write(entry.path(), record.to_string()).unwrap();
    }
}

fn resumed(lab: &Lab, task: &str) -> Option<Vec<String>> {
    lab.herdr_calls().into_iter().filter(|call| call[..2] == ["agent", "start"] && call[2] == task).nth(1)
}

#[test]
fn after_a_kebbie_update_restart_resumes_an_idle_claude_task_and_an_idle_codex_task_in_their_own_conversations() {
    let lab = Lab::new("restart-update");
    lab.profile("coder", "agent = \"codex\"\n");
    lab.ok(&["setup", "codex", "--yes"]);
    let claude = lab.start("search", "Add the search page", &[]);
    let codex = lab.start("filters", "Add the filters", &["--profile", "coder"]);
    with_session(&lab, "search", &claude, false);
    with_session(&lab, "filters", &codex, true);
    let before = stdout(&lab.run(&["restart"]));
    assert!(before.contains("search: up to date"), "nothing to restart before the update: {before}");
    kebbie_updated(&lab);
    let said = lab.ok(&["restart"]);
    for task in ["search", "filters"] {
        let line = said.lines().find(|line| line.starts_with(&format!("  {task}: "))).unwrap_or_else(|| panic!("{said}"));
        assert!(line.starts_with(&format!("  {task}: restarted, as ")) && line.ends_with("kebbie update"), "{said}");
    }
    let search = resumed(&lab, "search").expect("search started again");
    assert!(search.windows(2).any(|pair| pair == ["--resume", "s-search"]) && search.iter().any(|arg| arg.ends_with("roles/search@app.md")), "the same conversation and role: {search:?}");
    let filters = resumed(&lab, "filters").expect("filters started again");
    assert!(filters.contains(&"codex".to_string()) && filters.iter().any(|arg| arg == "s-filters"), "Codex resumes its own session: {filters:?}");
    assert!(lab.herdr_calls().iter().any(|call| call[..2] == ["pane", "send-keys"] && call[2] == "p_search"), "the old Claude was ended first");
}

#[test]
fn after_a_kebbie_update_restart_leaves_a_busy_task_a_drafted_one_and_a_codex_task_whose_profile_is_gone() {
    let lab = Lab::new("restart-skips");
    lab.profile("coder", "agent = \"codex\"\n");
    lab.ok(&["setup", "codex", "--yes"]);
    for (task, codex) in [("busy", false), ("drafted", false), ("orphan", true)] {
        let worktree = lab.start(task, &format!("Work on {task}"), if codex { &["--profile", "coder"] } else { &[] });
        with_session(&lab, task, &worktree, codex);
    }
    lab.agent("busy", |agent| agent["agent_status"] = json!("working"));
    lab.screen("p_drafted", &crate::scenarios::lab::CLAUDE_SCREEN.replace("❯ ", "❯ half a thought of my own"));
    fs::remove_file(lab.home.join(".config/kebbie/profiles/coder.toml")).unwrap();
    kebbie_updated(&lab);
    let said = lab.ok(&["restart"]);
    assert!(said.contains("  busy: restarts when idle, as "), "{said}");
    assert!(said.contains("  drafted: its input box holds text or cannot be read: restarts once it is empty, as "), "{said}");
    let orphan = said.lines().find(|line| line.starts_with("  orphan: ")).unwrap_or_else(|| panic!("{said}"));
    assert!(orphan.starts_with("  orphan: skipped: ") && orphan.contains("coder"), "{orphan}");
    for task in ["busy", "drafted", "orphan"] {
        assert!(resumed(&lab, task).is_none(), "{task} was not restarted");
    }
}
