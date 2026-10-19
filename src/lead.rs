use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::backlog;
use crate::config::{self, Source};
use crate::herdr;
use crate::peers;
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use crate::tasks::{self, Session};

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

// Each of the user's prompts carries what sessions told each other since the lead last looked; the lead pushes only through kelpie push, which checks the user's permission.
fn settings(kelpie: &str) -> String {
    let allow = ["peers", "tell", "answered", "messages", "status", "areas", "approve", "review", "land", "push", "decide", "defer", "notify"].map(|command| format!("Bash({kelpie} {command}:*)"));
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

// The panes by where they sit: the chat on the right, the board on the left, and whatever pane is under the board.
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
    let backlog::Role { finding, backlog, intake } = backlog::role_texts(sources);
    template.replace("{finding}", &finding).replace("{backlog}", &backlog).replace("{intake}", &intake)
}

// roles/lead.md filled in for the configured backlog sources, written to the state dir for --append-system-prompt-file.
fn role_file() -> Result<PathBuf> {
    let template = fs::read_to_string(install_root()?.join("roles/lead.md")).map_err(|error| format!("roles/lead.md: {error}"))?;
    let role = role_text(&template, &config::sources()?);
    let file = roles_dir()?.join("lead.md");
    fs::write(&file, role).map_err(|error| error.to_string())?;
    Ok(file)
}

fn chat_args(role: &Path, settings: &Path, session: &Session) -> Vec<String> {
    let mut args = ["--append-system-prompt-file", &role.to_string_lossy(), "--settings", &settings.to_string_lossy()].map(str::to_string).to_vec();
    args.extend(session.args());
    args
}

fn session_file() -> Result<PathBuf> {
    Ok(state_dir()?.join("lead-session"))
}

fn session_id(session: &Session) -> &str {
    match session {
        Session::New(id) | Session::Resume(id) => id,
    }
}

fn start_chat(pane: &str, session: Session) -> Result<()> {
    let role = role_file()?;
    let settings = write_settings(&roles_dir()?, LEAD_LABEL, &settings(&peers::kelpie_command()?))?;
    let args = chat_args(&role, &settings, &session);
    tasks::start_claude("lead", pane, &args.iter().map(String::as_str).collect::<Vec<_>>(), true)?;
    fs::write(session_file()?, session_id(&session)).map_err(|error| error.to_string())
}

fn new_chat(pane: &str) -> Result<()> {
    start_chat(pane, Session::New(tasks::new_session_id()?))
}

// The lead's directory is its own, so its newest conversation there is the lead's; the recorded one wins unless a newer one exists, as after /clear.
fn lead_session(dir: &Path, recorded: Option<&str>) -> Option<String> {
    let transcripts: Vec<(std::time::SystemTime, String)> = fs::read_dir(dir).ok()?.flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "jsonl"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path().file_stem()?.to_string_lossy().to_string())))
        .collect();
    let newest = transcripts.iter().map(|(modified, _)| *modified).max()?;
    recorded.filter(|id| transcripts.iter().any(|(modified, stem)| stem == id && *modified == newest)).map(str::to_string)
        .or_else(|| transcripts.iter().find(|(modified, _)| *modified == newest).map(|(_, stem)| stem.clone()))
}

#[derive(Debug, PartialEq)]
enum Restart {
    Nothing,
    Working,
    Asking,
    Go,
}

fn restart_verdict(agent: Option<&Value>) -> Restart {
    let Some(agent) = agent.filter(|agent| agent["agent"] == "claude") else { return Restart::Nothing };
    match agent["agent_status"].as_str().unwrap_or_default() {
        "blocked" => Restart::Asking,
        "idle" | "done" if agent["interactive_ready"] == true => Restart::Go,
        _ => Restart::Working,
    }
}

pub fn lead_workspace(workspaces: &[Value]) -> Option<String> {
    workspaces.iter().find(|workspace| workspace["label"] == LEAD_LABEL).and_then(workspace_id)
}

pub fn lead_agent<'a>(agents: &'a [Value], lead: &str) -> Option<&'a Value> {
    agents.iter().find(|agent| agent["agent"] == "claude" && agent["workspace_id"].as_str() == Some(lead))
}

pub fn restart() -> Result<String> {
    let nothing = "No lead is running, so there is nothing to restart.".to_string();
    let Some(lead) = lead_workspace(&herdr::workspaces()?) else { return Ok(nothing) };
    let agents = herdr::agents()?;
    let agent = lead_agent(&agents, &lead);
    let pane = match restart_verdict(agent) {
        Restart::Working => return Err("The lead is working; run kelpie lead --restart when it is idle.".to_string()),
        Restart::Asking => return Err("The lead is waiting on a question; answer it, then run kelpie lead --restart when the lead is idle.".to_string()),
        Restart::Go => agent.and_then(|agent| agent["pane_id"].as_str()).ok_or("The lead's Claude has no pane")?.to_string(),
        // Its Claude already gone, the lead's chat pane at a shell prompt still takes the resumed conversation.
        Restart::Nothing => match places(&lead)?.chat.filter(|pane| herdr::foreground(pane).ok().flatten().is_none()) {
            Some(pane) => pane,
            None => return Ok(nothing),
        },
    };
    let dir = lead_dir()?;
    let recorded = fs::read_to_string(session_file()?).ok().map(|id| id.trim().to_string()).filter(|id| !id.is_empty());
    let previous = tasks::claude_home().and_then(|home| {
        let canonical = dir.canonicalize().ok();
        [Some(dir.clone()), canonical].into_iter().flatten().find_map(|dir| lead_session(&tasks::transcripts_dir(&home, &dir), recorded.as_deref()))
    });
    let resumed = previous.is_some();
    let session = match previous {
        Some(id) => Session::Resume(id),
        None => Session::New(tasks::new_session_id()?),
    };
    if agent.is_some() {
        tasks::end_claude(&pane)?;
    }
    close_messages(places(&lead)?.messages.as_deref())?;
    start_chat(&pane, session)?;
    let how = if resumed { "resuming its conversation" } else { "with a new conversation, as none of its own was found" };
    Ok(format!("Restarted the lead with its current role and settings, {how}."))
}

// Whatever was quit or closed since the lead Space was made comes back when the user goes there: the board, or the lead's Claude.
fn repair(lead: &str) -> Result<()> {
    let executable = executable()?.display().to_string();
    let places = places(lead)?;
    let board = places.board.ok_or("The lead Space has lost its board pane; close the Space and open the lead again")?;
    if !runs_kelpie(&board, "board") && herdr::foreground(&board)?.is_none() {
        herdr::run_in_pane(&board, &format!("'{executable}' board"))?;
    }
    close_messages(places.messages.as_deref())?;
    match places.chat {
        Some(pane) if herdr::foreground(&pane)?.is_none() => new_chat(&pane)?,
        Some(_) => {}
        None => new_chat(&herdr::split_right(&board, &lead_dir()?, "0.68")?)?,
    }
    Ok(())
}

// The board shows the messages, so a messages pane under it in a lead Space goes; a pane of the user's own there stays.
fn close_messages(pane: Option<&str>) -> Result<()> {
    match pane {
        Some(pane) if runs_kelpie(pane, "messages") => herdr::close_pane(pane),
        _ => Ok(()),
    }
}

fn create() -> Result<String> {
    let dir = lead_dir()?;
    let space = herdr::create_workspace(&dir, LEAD_LABEL)?;
    let board = space.root_pane_id.ok_or("herdr created the lead Space without a pane")?;
    herdr::run_in_pane(&board, &format!("'{}' board", executable()?.display()))?;
    let chat = herdr::split_right(&board, &dir, "0.68")?;
    let started = new_chat(&chat);
    herdr::focus_workspace(&space.workspace_id)?;
    started?;
    Ok("Lead Space created: the board on the left, the lead on the right.".to_string())
}

#[derive(Debug, PartialEq)]
enum Hop {
    ToChat,
    ToBoard,
}

// In the lead Space focus moves between the board and the chat; from anywhere else it lands on the chat.
fn hop(in_lead: bool, on_chat: bool) -> Option<Hop> {
    match (in_lead, on_chat) {
        (true, true) => Some(Hop::ToBoard),
        (_, false) => Some(Hop::ToChat),
        (false, true) => None,
    }
}

pub fn focus_toggle() -> Result<()> {
    let workspaces = herdr::workspaces()?;
    let lead = lead_workspace(&workspaces).ok_or("No lead Space yet; kelpie lead creates it")?;
    let places = places(&lead)?;
    let board = places.board.ok_or("The lead Space has lost its board pane; kelpie lead brings it back")?;
    let chat = places.chat.ok_or("The lead Space has no chat pane; kelpie lead brings it back")?;
    let focused = workspaces.iter().find(|workspace| workspace["focused"] == true).and_then(workspace_id);
    let in_lead = focused.as_deref() == Some(lead.as_str());
    if !in_lead {
        if let Some(focused) = focused {
            fs::write(state_dir()?.join("lead-previous"), focused).map_err(|error| error.to_string())?;
        }
        herdr::focus_workspace(&lead)?;
    }
    let on_chat = herdr::focused_pane().ok().flatten().as_deref() == Some(chat.as_str());
    match hop(in_lead, on_chat) {
        Some(Hop::ToBoard) => herdr::focus_beside(&chat, "left"),
        Some(Hop::ToChat) => herdr::focus_beside(&board, "right"),
        None => Ok(()),
    }
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
        assert!(both.contains("Linear (assigned to the user, current cycle)") && !both.contains("{backlog}") && !both.contains("{finding}") && !both.contains("{intake}"));
        assert!(!role_text(template, &[Source::Local]).contains("Linear") && role_text(template, &[Source::Local]).contains("kelpie backlog add"));
    }

    #[test]
    fn the_lead_takes_an_idea_in_as_a_draft_and_starts_or_files_it_on_yes() {
        let role = role_text(include_str!("../roles/lead.md"), &[Source::Vault]);
        for needed in ["## Taking in ideas", "Draft it as a task note", "start it now as a task, file it in the backlog", "status `backlog`", "--backlog <the note's path>", "stays a plain task", "kelpie -C <repo> link <task>"] {
            assert!(role.contains(needed), "{needed}");
        }
    }

    #[test]
    fn the_lead_gets_copies_of_the_questions_the_user_answers_on_the_board() {
        let role = include_str!("../roles/lead.md");
        for needed in ["answers them on the board", "copy", "kelpie answered <task>", "kelpie decide <task>", "a task or a subagent"] {
            assert!(role.contains(needed), "{needed}");
        }
        assert!(!role.contains("AskUserQuestion tool (while") && !role.contains("The plan to read"), "the lead never relays a question");
    }

    #[test]
    fn focus_moves_between_the_board_and_the_chat_in_the_lead_and_to_the_chat_from_elsewhere() {
        assert_eq!(hop(true, false), Some(Hop::ToChat));
        assert_eq!(hop(true, true), Some(Hop::ToBoard));
        assert_eq!(hop(false, false), Some(Hop::ToChat));
        assert_eq!(hop(false, true), None);
    }

    #[test]
    fn the_lead_role_carries_the_push_permission_the_vault_log_after_a_push_and_away_mode() {
        let role = include_str!("../roles/lead.md");
        for needed in ["push_permission", "kelpie -C <repo> push <task>", "once its reviewer passed its current code", "log_append", "with those shas", "kelpie away on|off", "kelpie decide <task>", "kelpie defer <task>", "The user is back", "what landed, what you decided and why, and what waits for them"] {
            assert!(role.contains(needed), "{needed}");
        }
        assert!(!role.contains("Nobody pushes without the user: not you"), "the lead pushes under the user's permission");
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
        let args = chat_args(&dir.join("lead.md"), &file, &Session::New("0b5e8c1e-4a52-4c1b-9b7e-2f6a1d3c9e40".to_string()));
        let line = args.join(" ");
        assert!(!line.contains('{') && line.len() < 512, "{line}");
        assert_eq!(args[args.iter().position(|arg| arg == "--settings").unwrap() + 1], file.to_string_lossy());
        assert_eq!(args[args.len() - 2..], ["--session-id".to_string(), "0b5e8c1e-4a52-4c1b-9b7e-2f6a1d3c9e40".to_string()]);
        let written: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written, serde_json::from_str::<Value>(&settings(&kelpie)).unwrap());
        assert_eq!(written["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"], format!("'{kelpie}' messages --unread"));
        assert!(written["permissions"]["allow"].as_array().unwrap().contains(&json!(format!("Bash({kelpie} answered:*)"))));
        assert!(written["permissions"]["deny"].is_null() && written["env"].is_null(), "the lead may push on the user's word, outside kelpie push too");
        assert!(written["permissions"]["allow"].as_array().unwrap().contains(&json!(format!("Bash({kelpie} push:*)"))));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_restart_resumes_the_recorded_conversation_unless_a_newer_one_is_in_the_leads_folder() {
        let dir = std::env::temp_dir().join(format!("kelpie-lead-session-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(lead_session(&dir, Some("recorded")), None);
        let at = |name: &str, seconds: u64| {
            let file = fs::File::create(dir.join(name)).unwrap();
            file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)).unwrap();
        };
        at("older.jsonl", 100);
        at("recorded.jsonl", 200);
        at("newest.txt", 900);
        assert_eq!(lead_session(&dir, Some("recorded")).as_deref(), Some("recorded"));
        assert_eq!(lead_session(&dir, None).as_deref(), Some("recorded"));
        assert_eq!(lead_session(&dir, Some("gone")).as_deref(), Some("recorded"));
        at("after-clear.jsonl", 300);
        assert_eq!(lead_session(&dir, Some("recorded")).as_deref(), Some("after-clear"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_restart_goes_ahead_only_for_an_idle_lead_and_says_why_otherwise() {
        let agent = |status: &str, ready: bool| json!({ "agent": "claude", "agent_status": status, "interactive_ready": ready, "workspace_id": "w_lead", "pane_id": "p_9" });
        assert_eq!(restart_verdict(Some(&agent("idle", true))), Restart::Go);
        assert_eq!(restart_verdict(Some(&agent("done", true))), Restart::Go);
        assert_eq!(restart_verdict(Some(&agent("working", true))), Restart::Working);
        assert_eq!(restart_verdict(Some(&agent("idle", false))), Restart::Working);
        assert_eq!(restart_verdict(Some(&agent("blocked", true))), Restart::Asking);
        assert_eq!(restart_verdict(None), Restart::Nothing);
        assert_eq!(restart_verdict(Some(&json!({ "agent": "codex", "agent_status": "idle" }))), Restart::Nothing);
    }

    #[test]
    fn the_leads_claude_is_the_one_in_the_lead_space() {
        let workspaces = [json!({ "label": "api", "workspace_id": "w_1" }), json!({ "label": LEAD_LABEL, "workspace_id": "w_lead" })];
        assert_eq!(lead_workspace(&workspaces).as_deref(), Some("w_lead"));
        assert_eq!(lead_workspace(&workspaces[..1]), None);
        let agents = [json!({ "agent": "claude", "workspace_id": "w_1", "pane_id": "p_1" }), json!({ "agent": "codex", "workspace_id": "w_lead", "pane_id": "p_2" }), json!({ "agent": "claude", "workspace_id": "w_lead", "pane_id": "p_3" })];
        assert_eq!(lead_agent(&agents, "w_lead").unwrap()["pane_id"], "p_3");
        assert!(lead_agent(&agents[..2], "w_lead").is_none());
    }

    #[test]
    fn every_start_of_the_leads_claude_records_a_session_id() {
        let source = include_str!("lead.rs");
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("start_chat").contains("fs::write(session_file()?"));
        for caller in ["repair", "create"] {
            assert!(body(caller).contains("new_chat(") && !body(caller).contains("start_chat("), "{caller}");
        }
    }
}
