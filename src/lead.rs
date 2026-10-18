use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::backlog;
use crate::config::{self, Source};
use crate::herdr;
use crate::peers;
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use crate::tasks;

pub const STATE_DIR_VAR: &str = "KELPIE_STATE_DIR";

pub fn state_path(var: impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
    if let Some(own) = var(STATE_DIR_VAR).filter(|own| !own.is_empty()) {
        return Ok(PathBuf::from(own));
    }
    var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .map(|base| base.join("kelpie"))
        .ok_or_else(|| "Neither XDG_STATE_HOME nor HOME is set".to_string())
}

pub fn state_dir() -> Result<PathBuf> {
    let dir = state_path(|name| std::env::var(name).ok())?;
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

pub fn roles_dir() -> Result<PathBuf> {
    let dir = state_dir()?.join("roles");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

// herdr refuses a start line longer than the terminal's line limit, so Claude's settings go in as a file.
pub fn write_settings(dir: &Path, name: &str, settings: &str) -> Result<PathBuf> {
    let file = dir.join(format!("{name}.settings.json"));
    fs::write(&file, settings).map_err(|error| format!("{}: {error}", file.display()))?;
    Ok(file)
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
    let allow = ["peers", "tell", "answered", "messages", "status", "areas", "approve", "review"].map(|command| format!("Bash({kelpie} {command}:*)"));
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
    let file = roles_dir()?.join("lead.md");
    fs::write(&file, role).map_err(|error| error.to_string())?;
    Ok(file)
}

fn chat_args(role: &Path, settings: &Path) -> Vec<String> {
    ["--append-system-prompt-file", &role.to_string_lossy(), "--settings", &settings.to_string_lossy()].map(str::to_string).to_vec()
}

fn start_chat(pane: &str) -> Result<()> {
    let role = role_file()?;
    let settings = write_settings(&roles_dir()?, LEAD_LABEL, &settings(&peers::kelpie_command()?))?;
    let args = chat_args(&role, &settings);
    tasks::start_claude("lead", pane, &args.iter().map(String::as_str).collect::<Vec<_>>(), true)
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
    let started = start_chat(&chat);
    herdr::focus_workspace(&space.workspace_id)?;
    started?;
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

    #[test]
    fn the_lead_relays_a_tasks_question_to_the_user_and_its_answer_back() {
        let role = include_str!("../roles/lead.md");
        for needed in ["AskUserQuestion", "(recommended)", "kelpie tell <task> \"<answer>\"", "kelpie answered <task>", "The plan to read", "a task or a subagent"] {
            assert!(role.contains(needed), "{needed}");
        }
    }

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |name| vars.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string())
    }

    #[test]
    fn kelpie_state_dir_wins_over_the_xdg_default() {
        let all = [("KELPIE_STATE_DIR", "/tmp/probe"), ("XDG_STATE_HOME", "/x"), ("HOME", "/h")];
        assert_eq!(state_path(env(&all)).unwrap(), PathBuf::from("/tmp/probe"));
        assert_eq!(state_path(env(&all[1..])).unwrap(), PathBuf::from("/x/kelpie"));
        assert_eq!(state_path(env(&all[2..])).unwrap(), PathBuf::from("/h/.local/state/kelpie"));
        assert_eq!(state_path(env(&[("KELPIE_STATE_DIR", ""), ("HOME", "/h")])).unwrap(), PathBuf::from("/h/.local/state/kelpie"));
        assert!(state_path(env(&[])).is_err());
    }

    #[test]
    fn the_review_script_keeps_its_marks_in_kelpie_state_dir() {
        let base = std::env::temp_dir().join(format!("kelpie-state-dir-{}", std::process::id()));
        let (repo, state, home) = (base.join("repo"), base.join("state"), base.join("home"));
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("a.txt"), "a").unwrap();
        let git = std::process::Command::new("git").args(["init", "-q"]).current_dir(&repo).status().unwrap();
        assert!(git.success());
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/review.sh");
        let status = std::process::Command::new("sh").arg(&script).args(["mark-read", "a.txt"]).current_dir(&repo)
            .env("KELPIE_STATE_DIR", &state).env("HOME", &home).env_remove("XDG_STATE_HOME").status().unwrap();
        assert!(status.success());
        let marks: Vec<PathBuf> = fs::read_dir(&state).unwrap().map(|entry| entry.unwrap().path()).collect();
        assert_eq!(marks.len(), 1, "{marks:?}");
        assert!(fs::read_to_string(&marks[0]).unwrap().starts_with("a.txt\t"));
        assert!(!home.exists());
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn the_leads_start_line_carries_its_settings_as_a_short_path_to_the_same_json() {
        let dir = std::env::temp_dir().join(format!("kelpie-lead-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let kelpie = format!("/Users/someone-with-a-long-name/projects/{}/target/release/kelpie", "nested/".repeat(20));
        let file = write_settings(&dir, LEAD_LABEL, &settings(&kelpie)).unwrap();
        assert_eq!(file, dir.join("lead.settings.json"));
        let args = chat_args(&dir.join("lead.md"), &file);
        let line = args.join(" ");
        assert!(!line.contains('{') && line.len() < 512, "{line}");
        assert_eq!(args[args.iter().position(|arg| arg == "--settings").unwrap() + 1], file.to_string_lossy());
        let written: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written, serde_json::from_str::<Value>(&settings(&kelpie)).unwrap());
        assert_eq!(written["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"], format!("'{kelpie}' messages --unread"));
        assert!(written["permissions"]["allow"].as_array().unwrap().contains(&json!(format!("Bash({kelpie} answered:*)"))));
        fs::remove_dir_all(dir).unwrap();
    }
}
