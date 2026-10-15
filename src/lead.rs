use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use crate::herdr;
use crate::sh::Result;
use crate::status::LEAD_LABEL;

fn state_dir() -> Result<PathBuf> {
    let base = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .map_err(|_| "Neither XDG_STATE_HOME nor HOME is set".to_string())?;
    let dir = base.join("kelpie");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

fn executable() -> Result<PathBuf> {
    std::env::current_exe().and_then(|path| path.canonicalize()).map_err(|error| error.to_string())
}

// The binary sits in <repo>/target/release, next to roles/.
fn install_root() -> Result<PathBuf> {
    executable()?.ancestors().nth(3).map(PathBuf::from).ok_or_else(|| "Cannot locate kelpie's files".to_string())
}

fn workspace_id(workspace: &Value) -> Option<String> {
    workspace["workspace_id"].as_str().map(str::to_string)
}

pub fn toggle() -> Result<String> {
    let workspaces = herdr::workspaces()?;
    let focused = workspaces.iter().find(|workspace| workspace["focused"] == true).and_then(workspace_id);
    let lead = workspaces.iter().find(|workspace| workspace["label"] == LEAD_LABEL).and_then(workspace_id);
    let previous_file = state_dir()?.join("lead-previous");
    match lead {
        Some(lead) if Some(&lead) == focused.as_ref() => {
            let previous = fs::read_to_string(&previous_file).unwrap_or_default().trim().to_string();
            if workspaces.iter().any(|workspace| workspace["workspace_id"] == previous.as_str()) {
                herdr::focus_workspace(&previous)?;
                return Ok("Back to where you were.".to_string());
            }
            Ok("Already in the lead.".to_string())
        }
        Some(lead) => {
            if let Some(focused) = focused {
                fs::write(&previous_file, focused).map_err(|error| error.to_string())?;
            }
            herdr::focus_workspace(&lead)?;
            Ok("In the lead.".to_string())
        }
        None => {
            if let Some(focused) = focused {
                fs::write(&previous_file, focused).map_err(|error| error.to_string())?;
            }
            create()
        }
    }
}

fn create() -> Result<String> {
    let root = install_root()?;
    let dir = std::env::var("KELPIE_LEAD_DIR").or_else(|_| std::env::var("HOME")).map(PathBuf::from)
        .map_err(|_| "Set KELPIE_LEAD_DIR or HOME".to_string())?;
    let space = herdr::create_workspace(&dir, LEAD_LABEL)?;
    let board = space.root_pane_id.ok_or("herdr created the lead Space without a pane")?;
    herdr::run_in_pane(&board, &format!("'{}' board", executable()?.display()))?;
    let chat = herdr::split_right(&board, &dir, "0.68")?;
    let role = root.join("roles/lead.md");
    if let Err(error) = herdr::start_agent("lead", "claude", &chat, &["--append-system-prompt-file", &role.to_string_lossy()])
        && !error.contains("agent_not_ready")
    {
        return Err(error);
    }
    Ok("Lead Space created: the board on the left, the lead on the right.".to_string())
}
