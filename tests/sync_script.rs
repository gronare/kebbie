use std::fs;
use std::process::Command;

#[path = "../src/executable.rs"]
mod executable;
#[path = "../src/test_seal.rs"]
mod test_seal;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/sync.sh");

#[test]
fn a_pane_reads_context_only_from_its_own_sessions_transcript() {
    let root = std::env::temp_dir().canonicalize().unwrap().join(format!("kebbie-sync-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let home = root.join("home");
    let outside = home.join(".claude/outside");
    fs::create_dir_all(home.join(".claude/projects/-src-app")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let usage = r#"{"type":"assistant","message":{"model":"claude-opus-9-1","usage":{"input_tokens":1000,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    fs::write(home.join(".claude/projects/-src-app/0b1c-own.jsonl"), format!("{usage}\n")).unwrap();
    fs::write(outside.join("secret.jsonl"), format!("{usage}\n")).unwrap();
    let panes = r#"{"result":{"panes":[{"pane_id":"p_own","workspace_id":"w1","tab_id":"t1","agent":"claude","terminal_title_stripped":"billing","agent_session":{"value":"0b1c-own"}},{"pane_id":"p_escape","workspace_id":"w1","tab_id":"t1","agent":"claude","terminal_title_stripped":"billing","agent_session":{"value":"../../outside/secret"}}]}}"#;
    let herdr = format!(concat!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{0}/herdr.log'\ncase \"$1 $2\" in\n",
        "  'pane list') cat '{0}/panes.json' ;;\n",
        "  'tab list') echo '{{\"result\":{{\"tabs\":[]}}}}' ;;\n",
        "esac\n"), root.display());
    fs::write(root.join("panes.json"), panes).unwrap();
    executable::write(root.join("herdr"), herdr).unwrap();
    let output = Command::new("sh").arg(SCRIPT).env("HOME", &home).env("HERDR_BIN_PATH", root.join("herdr")).env("KEBBIE_BIN", "false").env("XDG_STATE_HOME", root.join("state")).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let calls = fs::read_to_string(root.join("herdr.log")).unwrap();
    let reported = |pane: &str| calls.lines().find(|call| call.starts_with(&format!("pane report-metadata {pane} "))).unwrap_or_else(|| panic!("{pane} in {calls}")).to_string();
    assert!(reported("p_own").contains("--token model=opus 9.1 --token ctx=1k"), "{calls}");
    assert!(!reported("p_escape").contains("model="), "a session name with a path in it reads no file outside the projects folder: {calls}");
    let _ = fs::remove_dir_all(root);
}
