use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::backlog;
use crate::config::{self, Source};
use crate::herdr;
use crate::peers;
use crate::sh::Result;
use crate::status::LEAD_LABEL;

pub fn state_dir() -> Result<PathBuf> {
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
pub fn install_root() -> Result<PathBuf> {
    executable()?.ancestors().nth(3).map(PathBuf::from).ok_or_else(|| "Cannot locate kelpie's files".to_string())
}

fn workspace_id(workspace: &Value) -> Option<String> {
    workspace["workspace_id"].as_str().map(str::to_string)
}

pub fn keep_first() -> Result<String> {
    let workspaces = herdr::workspaces()?;
    match workspaces.iter().position(|workspace| workspace["label"] == LEAD_LABEL) {
        None => Ok("No lead Space.".to_string()),
        Some(0) => Ok("The lead Space is first.".to_string()),
        Some(index) => {
            herdr::move_workspace(&workspace_id(&workspaces[index]).ok_or("The lead Space has no id")?, 0)?;
            Ok("The lead Space is first again.".to_string())
        }
    }
}

pub fn toggle() -> Result<String> {
    let answer = go()?;
    keep_first()?;
    Ok(answer)
}

fn go() -> Result<String> {
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
            repair(&lead)?;
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

// Each of the user's prompts carries what sessions told each other since the lead last looked.
fn settings(kelpie: &str) -> String {
    let allow = ["peers", "tell", "messages", "status", "areas"].map(|command| format!("Bash({kelpie} {command}:*)"));
    json!({
        "hooks": { "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": format!("'{kelpie}' messages --unread") }] }] },
        "permissions": { "allow": allow },
    }).to_string()
}

// Claude Code never remembers trust for the home directory, so the lead gets a directory of its own.
fn lead_dir() -> Result<PathBuf> {
    let dir = std::env::var("KELPIE_LEAD_DIR").map(PathBuf::from).or_else(|_| state_dir().map(|dir| dir.join("lead")))?;
    fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    Ok(dir)
}

// The panes by where create put them: the chat on the right, the board above the messages on the left.
struct Places {
    board: Option<String>,
    messages: Option<String>,
    chat: Option<String>,
}

fn places(lead: &str) -> Result<Places> {
    let first = herdr::panes_in(lead)?.iter().find_map(|pane| pane["pane_id"].as_str().map(str::to_string)).ok_or("The lead Space has no panes")?;
    let layout = herdr::layout(&first)?;
    let mut panes: Vec<(u64, u64, String)> = layout["panes"].as_array().into_iter().flatten()
        .filter_map(|pane| Some((pane["rect"]["x"].as_u64()?, pane["rect"]["y"].as_u64()?, pane["pane_id"].as_str()?.to_string())))
        .collect();
    let right = panes.iter().map(|(x, _, _)| *x).max().unwrap_or(0);
    let chat = (right > 0).then(|| panes.iter().find(|(x, _, _)| *x == right).map(|(_, _, id)| id.clone())).flatten();
    panes.retain(|(x, _, _)| *x < right || right == 0);
    panes.sort_by_key(|(_, y, _)| *y);
    let mut left = panes.into_iter().map(|(_, _, id)| id);
    Ok(Places { board: left.next(), messages: left.next(), chat })
}

fn runs_kelpie(pane: &str, command: &str) -> bool {
    herdr::foreground(pane).ok().flatten().is_some_and(|argv| argv.iter().any(|arg| arg.ends_with("kelpie")) && argv.iter().any(|arg| arg == command))
}

pub fn role_text(template: &str, sources: &[Source]) -> String {
    let (finding, backlog) = backlog::role_texts(sources);
    template.replace("{finding}", &finding).replace("{backlog}", &backlog)
}

// roles/lead.md filled in for the configured backlog sources, written to the state dir for --append-system-prompt-file.
fn role_file() -> Result<PathBuf> {
    let template = fs::read_to_string(install_root()?.join("roles/lead.md")).map_err(|error| format!("roles/lead.md: {error}"))?;
    let role = role_text(&template, &config::sources()?);
    let dir = state_dir()?.join("roles");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let file = dir.join("lead.md");
    fs::write(&file, role).map_err(|error| error.to_string())?;
    Ok(file)
}

fn start_chat(pane: &str) -> Result<()> {
    let role = role_file()?;
    let settings = settings(&peers::kelpie_command()?);
    match herdr::start_agent("lead", "claude", pane, &["--append-system-prompt-file", &role.to_string_lossy(), "--settings", &settings]) {
        Err(error) if !error.contains("agent_not_ready") => Err(error),
        _ => Ok(()),
    }
}

// Whatever was quit or closed since the lead Space was made comes back when the user goes there: the board, the messages, or the lead's Claude.
fn repair(lead: &str) -> Result<()> {
    let executable = executable()?.display().to_string();
    let places = places(lead)?;
    let board = places.board.ok_or("The lead Space has lost its board pane; close the Space and open the lead again")?;
    if !runs_kelpie(&board, "board") && herdr::foreground(&board)?.is_none() {
        herdr::run_in_pane(&board, &format!("'{executable}' board"))?;
    }
    match places.messages {
        Some(pane) if runs_kelpie(&pane, "messages") => {}
        Some(pane) if herdr::foreground(&pane)?.is_none() => herdr::run_in_pane(&pane, &format!("'{executable}' messages"))?,
        Some(_) => {}
        None => herdr::run_in_pane(&herdr::split_down(&board, &lead_dir()?, "0.65")?, &format!("'{executable}' messages"))?,
    }
    match places.chat {
        Some(pane) if herdr::foreground(&pane)?.is_none() => start_chat(&pane)?,
        Some(_) => {}
        None => start_chat(&herdr::split_right(&board, &lead_dir()?, "0.68")?)?,
    }
    Ok(())
}

fn create() -> Result<String> {
    let dir = lead_dir()?;
    let space = herdr::create_workspace(&dir, LEAD_LABEL)?;
    let board = space.root_pane_id.ok_or("herdr created the lead Space without a pane")?;
    herdr::run_in_pane(&board, &format!("'{}' board", executable()?.display()))?;
    let chat = herdr::split_right(&board, &dir, "0.68")?;
    let messages = herdr::split_down(&board, &dir, "0.65")?;
    herdr::run_in_pane(&messages, &format!("'{}' messages", executable()?.display()))?;
    start_chat(&chat)?;
    Ok("Lead Space created: the board on the left over the sessions' messages, the lead on the right.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lead_role_mentions_linear_only_when_it_is_a_source() {
        let template = include_str!("../roles/lead.md");
        assert!(!role_text(template, &[Source::Vault]).contains("Linear"));
        assert!(!role_text(template, &[]).contains("Linear"));
        let both = role_text(template, &[Source::Vault, Source::Linear]);
        assert!(both.contains("Linear (assigned to the user, current cycle)") && !both.contains("{backlog}") && !both.contains("{finding}"));
    }
}
