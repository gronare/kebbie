use std::path::Path;

use serde_json::Value;

use crate::sh::{Result, run};

fn herdr() -> String {
    std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string())
}

fn call(args: &[&str]) -> Result<Value> {
    let output = run(&herdr(), args, None)?;
    if output.is_empty() {
        return Ok(Value::Null);
    }
    let reply: Value = serde_json::from_str(&output).map_err(|error| format!("herdr {}: {error}", args.join(" ")))?;
    Ok(reply["result"].clone())
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
    let result = call(&["workspace", "create", "--cwd", &cwd, "--label", label, "--focus"])?;
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

pub fn split_down(pane_id: &str, cwd: &Path, ratio: &str) -> Result<String> {
    let cwd = cwd.to_string_lossy();
    let result = call(&["pane", "split", pane_id, "--direction", "down", "--ratio", ratio, "--cwd", &cwd, "--no-focus"])?;
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

pub fn send_keys(pane_id: &str, keys: &[&str]) -> Result<()> {
    let mut args = vec!["pane", "send-keys", pane_id];
    args.extend_from_slice(keys);
    call(&args).map(|_| ())
}

pub fn tabs_in(workspace_id: &str) -> Result<Vec<Value>> {
    Ok(call(&["tab", "list", "--workspace", workspace_id])?["tabs"].as_array().cloned().unwrap_or_default())
}

pub fn layout(pane_id: &str) -> Result<Value> {
    Ok(call(&["pane", "layout", "--pane", pane_id])?["layout"].clone())
}
