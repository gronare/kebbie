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
