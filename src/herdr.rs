use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::sh::{Result, attempt, run};

fn herdr() -> String {
    #[cfg(test)]
    assert!(std::env::var_os("HERDR_BIN_PATH").is_some_and(|bin| !bin.is_empty()), "a test reached herdr without HERDR_BIN_PATH; it could act on a live herdr");
    std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string())
}

pub fn bin() -> String {
    herdr()
}

fn call(args: &[&str]) -> Result<Value> {
    let output = attempt(&herdr(), args, None)?.map_err(|stderr| refused(args, &stderr))?;
    if output.is_empty() {
        return Ok(Value::Null);
    }
    let reply: Value = serde_json::from_str(&output).map_err(|error| format!("herdr {}: {error}", args.join(" ")))?;
    Ok(reply["result"].clone())
}

// herdr answers a refusal with {"error":{"code","message"}} on stderr; the arguments are left out, as a start's can run long.
fn refused(args: &[&str], stderr: &str) -> String {
    let command = args.iter().take_while(|arg| !arg.starts_with('-')).take(2).copied().collect::<Vec<_>>().join(" ");
    let error = serde_json::from_str::<Value>(stderr).ok().map(|reply| reply["error"].clone());
    match error.as_ref().and_then(|error| Some((error["message"].as_str()?, error["code"].as_str()?))) {
        Some((message, code)) => format!("herdr {command} refused: {message} ({code})"),
        None if stderr.is_empty() => format!("herdr {command} failed"),
        None => format!("herdr {command} failed: {stderr}"),
    }
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

pub struct Space {
    pub workspace_id: String,
    pub root_pane_id: Option<String>,
}

pub fn open_worktree(repo: &Path, path: &Path, label: &str, focus: bool) -> Result<Space> {
    let repo = repo.to_string_lossy();
    let path = path.to_string_lossy();
    let focus = if focus { "--focus" } else { "--no-focus" };
    let result = call(&["worktree", "open", "--cwd", &repo, "--path", &path, "--label", label, focus])?;
    Ok(Space {
        workspace_id: text(&result["workspace"]["workspace_id"]).ok_or("herdr worktree open returned no workspace")?,
        root_pane_id: text(&result["root_pane"]["pane_id"]),
    })
}

pub fn rename_workspace(workspace_id: &str, label: &str) -> Result<()> {
    call(&["workspace", "rename", workspace_id, label]).map(|_| ())
}

pub fn focus_workspace(workspace_id: &str) -> Result<()> {
    call(&["workspace", "focus", workspace_id]).map(|_| ())
}

pub fn close_workspace(workspace_id: &str) -> Result<()> {
    call(&["workspace", "close", workspace_id]).map(|_| ())
}

pub fn workspace_for(path: &Path) -> Result<Option<String>> {
    let result = call(&["workspace", "list"])?;
    let path = path.to_string_lossy();
    Ok(result["workspaces"].as_array().into_iter().flatten()
        .find(|workspace| workspace["worktree"]["checkout_path"].as_str() == Some(&*path))
        .and_then(|workspace| text(&workspace["workspace_id"])))
}

pub fn create_tab(workspace_id: &str, cwd: &Path, label: &str) -> Result<String> {
    let cwd = cwd.to_string_lossy();
    let result = call(&["tab", "create", "--workspace", workspace_id, "--cwd", &cwd, "--label", label, "--no-focus"])?;
    text(&result["root_pane"]["pane_id"]).ok_or_else(|| "herdr tab create returned no pane".to_string())
}

pub fn run_in_pane(pane_id: &str, command: &str) -> Result<()> {
    call(&["pane", "run", pane_id, command]).map(|_| ())
}

pub fn start_agent(name: &str, kind: &str, pane_id: &str, agent_args: &[&str]) -> Result<()> {
    let mut args = vec!["agent", "start", name, "--kind", kind, "--pane", pane_id, "--"];
    args.extend_from_slice(agent_args);
    call(&args).map(|_| ())
}

pub fn move_pane_to_new_tab(pane_id: &str, workspace_id: &str, label: &str, focus: bool) -> Result<String> {
    let focus = if focus { "--focus" } else { "--no-focus" };
    let result = call(&["pane", "move", pane_id, "--new-tab", "--workspace", workspace_id, "--label", label, focus])?;
    text(&result["move_result"]["pane"]["pane_id"]).ok_or_else(|| "herdr pane move returned no pane".to_string())
}

pub fn workspaces() -> Result<Vec<Value>> {
    Ok(call(&["workspace", "list"])?["workspaces"].as_array().cloned().unwrap_or_default())
}

pub fn agents() -> Result<Vec<Value>> {
    Ok(call(&["agent", "list"])?["agents"].as_array().cloned().unwrap_or_default())
}

pub fn create_workspace(cwd: &Path, label: &str) -> Result<Space> {
    let cwd = cwd.to_string_lossy();
    let result = call(&["workspace", "create", "--cwd", &cwd, "--label", label, "--no-focus"])?;
    Ok(Space {
        workspace_id: text(&result["workspace"]["workspace_id"]).ok_or("herdr workspace create returned no workspace")?,
        root_pane_id: text(&result["root_pane"]["pane_id"]),
    })
}

pub fn split_right(pane_id: &str, cwd: &Path, ratio: &str) -> Result<String> {
    let cwd = cwd.to_string_lossy();
    let result = call(&["pane", "split", pane_id, "--direction", "right", "--ratio", ratio, "--cwd", &cwd, "--focus"])?;
    text(&result["pane"]["pane_id"]).ok_or_else(|| "herdr pane split returned no pane".to_string())
}

// Only what is on screen: reading further back makes herdr scroll an idle agent's pane and restore it.
pub fn read_agent(pane_id: &str) -> Result<String> {
    run(&herdr(), &["agent", "read", pane_id, "--source", "visible"], None)
}

pub fn prompt_agent(pane_id: &str, message: &str) -> Result<()> {
    call(&["agent", "prompt", pane_id, message]).map(|_| ())
}

pub fn report_metadata(pane_id: &str, args: &[&str]) -> Result<()> {
    let mut all = vec!["pane", "report-metadata", pane_id];
    all.extend_from_slice(args);
    call(&all).map(|_| ())
}

pub fn workspace_tokens(workspace_id: &str) -> Result<Value> {
    Ok(call(&["workspace", "get", workspace_id])?["workspace"]["tokens"].clone())
}

pub fn panes_in(workspace_id: &str) -> Result<Vec<Value>> {
    Ok(call(&["pane", "list", "--workspace", workspace_id])?["panes"].as_array().cloned().unwrap_or_default())
}

// The command a pane runs in the foreground, as argv; none while its shell waits at the prompt.
pub fn foreground(pane_id: &str) -> Result<Option<Vec<String>>> {
    let info = call(&["pane", "process-info", "--pane", pane_id])?["process_info"].clone();
    let process = &info["foreground_processes"][0];
    if process.is_null() || process["pid"] == info["shell_pid"] {
        return Ok(None);
    }
    Ok(Some(process["argv"].as_array().into_iter().flatten().filter_map(|arg| arg.as_str().map(str::to_string)).collect()))
}

pub fn process_info(pane_id: &str) -> Result<Value> {
    Ok(call(&["pane", "process-info", "--pane", pane_id])?["process_info"].clone())
}

pub fn send_keys(pane_id: &str, keys: &[&str]) -> Result<()> {
    let mut args = vec!["pane", "send-keys", pane_id];
    args.extend_from_slice(keys);
    call(&args).map(|_| ())
}

// send-text writes its bytes straight into the pane, so the markers make the app there take them as one paste, as a drop on it would.
pub fn paste(pane_id: &str, text: &str) -> Result<()> {
    call(&["pane", "send-text", pane_id, &bracketed(text)]).map(|_| ())
}

fn bracketed(text: &str) -> String {
    format!("\x1b[200~{}\x1b[201~", text.replace("\x1b[201~", ""))
}

pub fn tabs_in(workspace_id: &str) -> Result<Vec<Value>> {
    Ok(call(&["tab", "list", "--workspace", workspace_id])?["tabs"].as_array().cloned().unwrap_or_default())
}

pub fn layout(pane_id: &str) -> Result<Value> {
    Ok(call(&["pane", "layout", "--pane", pane_id])?["layout"].clone())
}

// `pane current` answers for the pane that calls it; the focused pane is the one `pane list` marks focused.
pub fn focused_pane() -> Result<Option<String>> {
    Ok(focused_in(&call(&["pane", "list"])?["panes"]))
}

fn focused_in(panes: &Value) -> Option<String> {
    panes.as_array()?.iter().find(|pane| pane["focused"] == true).and_then(|pane| pane["pane_id"].as_str().map(str::to_string))
}

// herdr focuses a pane by its neighbour: the pane next to `pane_id` on that side ("left", "right", "up", "down").
pub fn focus_beside(pane_id: &str, direction: &str) -> Result<()> {
    call(&["pane", "focus", "--pane", pane_id, "--direction", direction]).map(|_| ())
}

pub fn neighbour(pane_id: &str, direction: &str) -> Result<Option<String>> {
    Ok(text(&call(&["pane", "neighbor", "--pane", pane_id, "--direction", direction])?["neighbor"]["neighbor_pane_id"]))
}

pub fn tab_of(pane_id: &str) -> Result<Option<String>> {
    Ok(text(&call(&["pane", "get", pane_id])?["pane"]["tab_id"]))
}

pub fn rename_tab(tab_id: &str, label: &str) -> Result<()> {
    call(&["tab", "rename", tab_id, label]).map(|_| ())
}

fn synced_label(tab_id: &str) -> Option<String> {
    let state = std::env::var_os("XDG_STATE_HOME").filter(|dir| !dir.is_empty()).map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))?;
    std::fs::read_to_string(state.join("herdr-sync").join(tab_id.replace(':', "_"))).ok().map(|label| label.trim_end_matches('\n').to_string())
}

// herdr shows an unnamed tab by its number, and scripts/sync.sh records the last name it gave a tab; any other label is the user's.
pub fn automatic_label(label: &str, synced: Option<&str>, earlier: &[&str]) -> bool {
    label.chars().all(|character| character.is_ascii_digit()) || synced == Some(label) || earlier.contains(&label)
}

// Best effort: a tab that keeps its old name never stops what named it.
pub fn name_tab_of(pane_id: &str, label: &str, earlier: &[&str]) {
    let Ok(Some(tab)) = tab_of(pane_id) else { return };
    let Ok(Some(current)) = call(&["tab", "get", &tab]).map(|result| text(&result["tab"]["label"])) else { return };
    if current != label && automatic_label(&current, synced_label(&tab).as_deref(), earlier) {
        let _ = rename_tab(&tab, label);
    }
}

pub fn focus_tab(tab_id: &str) -> Result<()> {
    call(&["tab", "focus", tab_id]).map(|_| ())
}

// herdr's zoom fills the tab with the pane it names and focuses it; the tab's other panes run on, unseen.
pub fn zoom(pane_id: &str, on: bool) -> Result<()> {
    call(&["pane", "zoom", "--pane", pane_id, if on { "--on" } else { "--off" }]).map(|_| ())
}

pub fn swap_panes(source: &str, target: &str) -> Result<()> {
    call(&["pane", "swap", "--source-pane", source, "--target-pane", target]).map(|_| ())
}

pub fn close_pane(pane_id: &str) -> Result<()> {
    call(&["pane", "close", pane_id]).map(|_| ())
}

fn socket() -> Result<PathBuf> {
    std::env::var("HERDR_SOCKET_PATH").map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".config/herdr/herdr.sock")))
        .map_err(|_| "Neither HERDR_SOCKET_PATH nor HOME is set".to_string())
}

// The CLI has no command for the order of Spaces, so this one goes to the socket API.
pub fn move_workspace(workspace_id: &str, insert_index: usize) -> Result<()> {
    let path = socket()?;
    let mut stream = UnixStream::connect(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let request = json!({ "id": "kebbie", "method": "workspace.move", "params": { "workspace_id": workspace_id, "insert_index": insert_index } });
    writeln!(stream, "{request}").map_err(|error| error.to_string())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|error| error.to_string())?;
    let reply: Value = serde_json::from_str(&line).map_err(|error| format!("herdr workspace.move: {error}"))?;
    match reply.get("error") {
        Some(error) => Err(format!("herdr workspace.move: {error}")),
        None => Ok(()),
    }
}

pub fn read_pane(pane_id: &str) -> Result<String> {
    run(&herdr(), &["pane", "read", pane_id], None)
}

pub fn agent_ready(name: &str) -> Result<bool> {
    let agent = call(&["agent", "get", name])?["agent"].clone();
    Ok(agent["interactive_ready"] == true && agent["agent_status"] != "blocked")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_tab_label_herdr_the_sync_or_an_earlier_kebbie_gave_is_renamed() {
        assert!(automatic_label("2", None, &[]), "herdr's number for an unnamed tab");
        assert!(automatic_label("✻ kelpie", Some("✻ kelpie"), &[]), "the last name the sidebar sync gave it");
        assert!(automatic_label("claude", None, &["claude", "codex"]), "the agent's name an earlier kebbie gave a task's tab");
        assert!(!automatic_label("notes", Some("✻ kelpie"), &["claude"]), "a name the user typed stays");
        assert!(!automatic_label("✻ kelpie", None, &[]), "without the sync's record a name is the user's");
    }

    #[test]
    fn the_focused_pane_is_the_one_pane_list_marks_focused_not_the_caller() {
        let panes = serde_json::json!([{ "pane_id": "w1:p1", "focused": false }, { "pane_id": "w3:p2", "focused": true }]);
        assert_eq!(focused_in(&panes).as_deref(), Some("w3:p2"));
        assert_eq!(focused_in(&serde_json::json!([{ "pane_id": "w1:p1", "focused": false }])), None);
    }

    #[test]
    fn a_paste_reaches_the_pane_framed_as_one_bracketed_paste_that_cannot_end_early() {
        let dropped = r"/var/folders/T/Screenshot\ 2026-10-22\ at\ 10.02.13.png";
        assert_eq!(bracketed(dropped), format!("\x1b[200~{dropped}\x1b[201~"));
        assert_eq!(bracketed("a\x1b[201~b"), "\x1b[200~ab\x1b[201~", "an end marker inside the text would end the paste and type the rest as keys");
    }

    #[test]
    fn a_refusal_says_herdrs_message_and_code_without_the_arguments() {
        let args = ["agent", "start", "billing", "--kind", "claude", "--pane", "p_1", "--", "--settings", "/state/roles/billing@app.settings.json"];
        let stderr = r#"{"error":{"code":"agent_start_input_failed","message":"input contains a 1100-byte line but the terminal is in canonical mode with a 1024-byte limit; wait for the shell prompt and retry"},"id":"cli:agent:start"}"#;
        assert_eq!(refused(&args, stderr), "herdr agent start refused: input contains a 1100-byte line but the terminal is in canonical mode with a 1024-byte limit; wait for the shell prompt and retry (agent_start_input_failed)");
        assert_eq!(refused(&args, "boom"), "herdr agent start failed: boom");
        assert_eq!(refused(&["workspace", "list"], ""), "herdr workspace list failed");
    }

    #[test]
    fn every_herdr_call_goes_through_argv_and_a_missing_herdr_is_one_error() {
        let source = include_str!("herdr.rs");
        assert!(!source.contains("\"sh\"") && !source.contains("-c\""), "no shell between kebbie and herdr");
        let text = text(&serde_json::json!("x"));
        assert_eq!(text.as_deref(), Some("x"));
        assert_eq!(super::text(&serde_json::json!(5)), None);
        assert!(std::env::var("HERDR_BIN_PATH").is_ok_and(|bin| bin == "false"), "tests run with HERDR_BIN_PATH=false, so nothing here reaches a real herdr");
        let failed = call(&["workspace", "list"]).unwrap_err();
        assert_eq!(failed, "herdr workspace list failed");
        assert_eq!(bin(), "false");
    }
}
