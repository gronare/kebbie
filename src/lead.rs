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

pub fn state_location() -> Result<PathBuf> {
    #[cfg(test)]
    assert!(std::env::var_os(STATE_DIR_VAR).is_some_and(|own| !own.is_empty()), "a test reached kelpie's state dir without {STATE_DIR_VAR}; pass it a temp dir instead");
    state_path(|name| std::env::var(name).ok())
}

pub fn state_dir() -> Result<PathBuf> {
    let dir = state_location()?;
    crate::state::close_up(&dir)?;
    Ok(dir)
}

pub fn roles_dir() -> Result<PathBuf> {
    let dir = state_dir()?.join("roles");
    crate::state::private_dir(&dir)?;
    Ok(dir)
}

// herdr refuses a start line longer than the terminal's line limit, so Claude's settings go in as a file.
pub fn write_settings(dir: &Path, name: &str, settings: &str) -> Result<PathBuf> {
    let file = dir.join(format!("{name}.settings.json"));
    crate::state::write(&file, settings)?;
    Ok(file)
}

fn executable() -> Result<PathBuf> {
    crate::peers::kelpie_command().map(PathBuf::from)
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
                crate::state::write(&previous_file, focused)?;
            }
            repair(&lead)?;
            herdr::focus_workspace(&lead)?;
            Ok("In the lead.".to_string())
        }
        None => {
            if let Some(focused) = focused {
                crate::state::write(&previous_file, focused)?;
            }
            create()
        }
    }
}

// Each of the user's prompts carries what sessions told each other since the lead last looked; the lead pushes only through kelpie push, which checks the user's permission.
fn settings(kelpie: &str) -> String {
    let allow = ["peers", "tell", "answered", "messages", "status", "areas", "approve", "review", "land", "push", "decide", "defer", "notify", "issues", "issue"].map(|command| format!("Bash({kelpie} {command}:*)"));
    json!({
        "hooks": { "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": format!("{} messages --unread", crate::sh::quote(kelpie)) }] }] },
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
    zoomed: bool,
    focused: Option<String>,
}

impl Places {
    // herdr's zoom shows the focused pane alone, so the chat is hidden while the zoom is on the board.
    fn chat_hidden(&self) -> bool {
        self.zoomed && self.chat.is_some() && self.board.is_some() && self.focused == self.board
    }
}

fn places(lead: &str) -> Result<Places> {
    let first = herdr::panes_in(lead)?.iter().find_map(|pane| pane["pane_id"].as_str().map(str::to_string)).ok_or("The lead Space has no panes")?;
    Ok(places_in(&herdr::layout(&first)?))
}

// herdr gives the split's rects whether or not the tab is zoomed, so a hidden chat still sits on the right.
fn places_in(layout: &Value) -> Places {
    let mut panes: Vec<(u64, u64, String)> = layout["panes"].as_array().into_iter().flatten()
        .filter_map(|pane| Some((pane["rect"]["x"].as_u64()?, pane["rect"]["y"].as_u64()?, pane["pane_id"].as_str()?.to_string())))
        .collect();
    let right = panes.iter().map(|(x, _, _)| *x).max().unwrap_or(0);
    let chat = (right > 0).then(|| panes.iter().find(|(x, _, _)| *x == right).map(|(_, _, id)| id.clone())).flatten();
    panes.retain(|(x, _, _)| *x < right || right == 0);
    panes.sort_by_key(|(_, y, _)| *y);
    let mut left = panes.into_iter().map(|(_, _, id)| id);
    Places { board: left.next(), messages: left.next(), chat, zoomed: layout["zoomed"] == true, focused: layout["focused_pane_id"].as_str().map(str::to_string) }
}

fn runs_kelpie(pane: &str, command: &str) -> bool {
    herdr::foreground(pane).ok().flatten().is_some_and(|argv| argv.iter().any(|arg| arg.ends_with("kelpie")) && argv.iter().any(|arg| arg == command))
}

pub fn role_text(template: &str, sources: &[Source], vault: bool) -> String {
    let backlog::Role { finding, backlog, intake, vault_work, after_push } = backlog::role_texts(sources, vault);
    template.replace("{finding}", &finding).replace("{backlog}", &backlog).replace("{intake}", &intake).replace("{vault_work}", &vault_work).replace("{after_push}", &after_push)
}

// roles/lead.md filled in for the configured backlog sources, written to the state dir for --append-system-prompt-file.
fn role_file() -> Result<PathBuf> {
    let template = fs::read_to_string(install_root()?.join("roles/lead.md")).map_err(|error| format!("roles/lead.md: {error}"))?;
    let role = role_text(&template, &config::sources()?, crate::vault_cli::detect().is_some());
    let file = roles_dir()?.join("lead.md");
    crate::state::write(&file, role)?;
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

fn cd_line(dir: &Path) -> String {
    format!("cd {}", crate::sh::quote(&dir.to_string_lossy()))
}

// herdr's agent start has no cwd and runs Claude where the pane's shell is, and herdr's restore uses the pane's last cwd, so the shell goes to the lead's folder first.
fn start_chat(pane: &str, session: Session) -> Result<()> {
    let role = role_file()?;
    let settings = write_settings(&roles_dir()?, LEAD_LABEL, &settings(&peers::kelpie_command()?))?;
    let args = chat_args(&role, &settings, &session);
    tasks::wait_for_prompt(pane)?;
    herdr::run_in_pane(pane, &cd_line(&lead_dir()?))?;
    tasks::start_claude("lead", pane, &args.iter().map(String::as_str).collect::<Vec<_>>(), true)?;
    crate::state::write(&session_file()?, session_id(&session))
}

fn new_chat(pane: &str) -> Result<()> {
    start_chat(pane, Session::New(tasks::new_session_id()?))
}

// The lead's directory is its own, so its newest conversation there is the lead's; the recorded one wins unless a newer one exists, as after /clear.
type Transcript = (std::time::SystemTime, String);

fn transcripts_in(dir: &Path) -> Vec<Transcript> {
    fs::read_dir(dir).into_iter().flatten().flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "jsonl"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path().file_stem()?.to_string_lossy().to_string())))
        .collect()
}

// claude --resume <id> finds a transcript in another folder's project when exactly one project holds it, and the resumed conversation goes on writing there.
fn transcript_elsewhere(projects: &Path, id: &str) -> Option<Transcript> {
    let mut found = fs::read_dir(projects).ok()?.flatten().filter_map(|project| fs::metadata(project.path().join(format!("{id}.jsonl"))).ok()?.modified().ok());
    let modified = found.next()?;
    found.next().is_none().then(|| (modified, id.to_string()))
}

fn lead_session(transcripts: &[Transcript], recorded: Option<&str>) -> Option<String> {
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
    let mut own: Vec<PathBuf> = [Some(dir.clone()), dir.canonicalize().ok()].into_iter().flatten().collect();
    own.dedup();
    let home = tasks::claude_home();
    let mut transcripts: Vec<Transcript> = home.iter().flat_map(|home| own.iter().flat_map(|dir| transcripts_in(&tasks::transcripts_dir(home, dir)))).collect();
    let moved = home.zip(recorded.as_deref()).filter(|(_, id)| !transcripts.iter().any(|(_, stem)| stem == id)).and_then(|(home, id)| transcript_elsewhere(&home.join("projects"), id));
    transcripts.extend(moved.clone());
    let previous = lead_session(&transcripts, recorded.as_deref());
    let how = match (&previous, &moved) {
        (Some(id), Some((_, from))) if id == from => "resuming its conversation, started before the lead had its own folder",
        (Some(_), _) => "resuming its conversation",
        (None, _) => "with a new conversation, as none of its own was found",
    };
    let session = match previous {
        Some(id) => Session::Resume(id),
        None => Session::New(tasks::new_session_id()?),
    };
    if agent.is_some() {
        tasks::end_claude(&pane)?;
    }
    close_messages(places(&lead)?.messages.as_deref())?;
    start_chat(&pane, session)?;
    Ok(format!("Restarted the lead with its current role and settings, {how}."))
}

// Whatever was quit or closed since the lead Space was made comes back when the user goes there: the board, or the lead's Claude.
fn repair(lead: &str) -> Result<()> {
    let places = places(lead)?;
    let board = places.board.ok_or("The lead Space has lost its board pane; close the Space and open the lead again")?;
    start_board_at_shell(&board)?;
    close_messages(places.messages.as_deref())?;
    match places.chat {
        Some(pane) if herdr::foreground(&pane)?.is_none() => new_chat(&pane)?,
        Some(_) => {}
        None => new_chat(&herdr::split_right(&board, &lead_dir()?, "0.68")?)?,
    }
    Ok(())
}

fn board_command() -> Result<String> {
    Ok(format!("{} board", crate::sh::quote(&executable()?.to_string_lossy())))
}

fn start_board_at_shell(board: &str) -> Result<bool> {
    if runs_kelpie(board, "board") || herdr::foreground(board)?.is_some() {
        return Ok(false);
    }
    herdr::run_in_pane(board, &board_command()?)?;
    Ok(true)
}

const REVIVE_TRIES: usize = 30;
const REVIVE_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);

// herdr's startup hook must not wait on herdr, so the waiting half runs in its own session.
pub fn revive_board_detached() -> Result<String> {
    use std::os::unix::process::CommandExt;
    let file = state_dir()?.join("lead.log");
    let log = crate::state::append_to(&file).map_err(|error| format!("{}: {error}", file.display()))?;
    let mut command = std::process::Command::new(executable()?);
    command.args(["lead", "--revive-board", "--wait"]).stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|error| error.to_string())?).stderr(log)
        .env_remove("HERDR_PANE_ID").env_remove("HERDR_TAB_ID").env_remove("HERDR_WORKSPACE_ID");
    // SAFETY: setsid is async-signal-safe and touches nothing but the child.
    unsafe { command.pre_exec(|| if libc::setsid() == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) }) };
    command.spawn().map_err(|error| format!("lead --revive-board: {error}"))?;
    Ok(String::new())
}

fn lead_board() -> Option<String> {
    let lead = lead_workspace(&herdr::workspaces().ok()?)?;
    places(&lead).ok()?.board
}

fn wait_for_board(mut look: impl FnMut() -> Option<String>, mut pause: impl FnMut(), tries: usize) -> Option<String> {
    for _ in 0..tries {
        if let Some(board) = look() {
            return Some(board);
        }
        pause();
    }
    None
}

// herdr restores the lead Space but not the board in it, and resumes the chat's Claude itself, so only the board pane is touched.
pub fn revive_board() -> Result<String> {
    let Some(board) = wait_for_board(lead_board, || std::thread::sleep(REVIVE_PAUSE), REVIVE_TRIES) else { return Ok(String::new()) };
    Ok(if start_board_at_shell(&board)? { format!("board: started in {board} after herdr's restore") } else { String::new() })
}

// A build still writing the binary changes its mtime between looks; only the same mtime on two looks is a finished one.
fn settled(mut modified: impl FnMut() -> Option<std::time::SystemTime>, mut pause: impl FnMut(), tries: usize) -> Option<std::time::SystemTime> {
    let mut last = modified();
    for _ in 0..tries {
        pause();
        let now = modified();
        if now.is_some() && now == last {
            return now;
        }
        last = now;
    }
    None
}

pub fn renew_board() -> Result<String> {
    let Some(lead) = lead_workspace(&herdr::workspaces()?) else { return Ok("board: no lead Space".to_string()) };
    let Some(board) = places(&lead)?.board else { return Ok("board: the lead Space has no board pane".to_string()) };
    let info = herdr::process_info(&board)?;
    let process = &info["foreground_processes"][0];
    let argv: Vec<&str> = process["argv"].as_array().into_iter().flatten().filter_map(|arg| arg.as_str()).collect();
    if !(argv.iter().any(|arg| arg.ends_with("kelpie")) && argv.contains(&"board")) || argv.contains(&"--overlay") {
        return Ok("board: not running; it starts with the new kelpie when you open the lead".to_string());
    }
    let exe = executable()?;
    let modified = || fs::metadata(&exe).and_then(|meta| meta.modified()).ok();
    let Some(built) = settled(modified, || std::thread::sleep(std::time::Duration::from_secs(1)), 30) else {
        return Ok("board: the new kelpie is still being written; renew again once the build is done".to_string());
    };
    let built = built.duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0);
    let pid = process["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()).ok_or("board: herdr names no process for it")?;
    let started = crate::restart::started(pid, crate::restart::now()).ok_or("board: cannot read when it started")?;
    if started > built {
        return Ok("board: up to date".to_string());
    }
    for _ in 0..10 {
        if herdr::foreground(&board)?.is_none() {
            herdr::run_in_pane(&board, &board_command()?)?;
            return Ok("board: restarted on the new kelpie".to_string());
        }
        herdr::send_keys(&board, &["ctrl+c"])?;
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err("board: did not quit on ctrl-c; quit it and open the lead again".to_string())
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
    herdr::run_in_pane(&board, &board_command()?)?;
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

struct Lead {
    in_lead: bool,
    board: String,
    chat: String,
    places: Places,
}

fn lead_panes() -> Result<Lead> {
    let workspaces = herdr::workspaces()?;
    let lead = lead_workspace(&workspaces).ok_or("No lead Space yet; kelpie lead creates it")?;
    let places = places(&lead)?;
    let board = places.board.clone().ok_or("The lead Space has lost its board pane; kelpie lead brings it back")?;
    let chat = places.chat.clone().ok_or("The lead Space has no chat pane; kelpie lead brings it back")?;
    let focused = workspaces.iter().find(|workspace| workspace["focused"] == true).and_then(workspace_id);
    let in_lead = focused.as_deref() == Some(lead.as_str());
    if !in_lead {
        if let Some(focused) = focused {
            crate::state::write(&state_dir()?.join("lead-previous"), focused)?;
        }
        herdr::focus_workspace(&lead)?;
    }
    Ok(Lead { in_lead, board, chat, places })
}

// A hop out of a zoom turns it off, so the chat shows again and stays shown on the way back.
pub fn focus_toggle() -> Result<()> {
    let Lead { in_lead, board, chat, places } = lead_panes()?;
    let on_chat = herdr::focused_pane().ok().flatten().as_deref() == Some(chat.as_str());
    match (hop(in_lead, on_chat), places.zoomed) {
        (Some(Hop::ToBoard), true) => herdr::zoom(&board, false),
        (Some(Hop::ToChat), true) => herdr::zoom(&chat, false),
        (Some(Hop::ToBoard), false) => herdr::focus_beside(&chat, "left"),
        (Some(Hop::ToChat), false) => herdr::focus_beside(&board, "right"),
        (None, _) => Ok(()),
    }
}

// A paste the board passes on lands in the chat and the focus follows it there, showing a hidden chat again.
pub fn paste_into_chat(text: &str) -> Result<()> {
    let Lead { board, chat, places, .. } = lead_panes()?;
    herdr::paste(&chat, text)?;
    match (places.zoomed, places.focused.as_deref() == Some(chat.as_str())) {
        (true, _) => herdr::zoom(&chat, false),
        (false, true) => Ok(()),
        (false, false) => herdr::focus_beside(&board, "right"),
    }
}

pub fn toggle_chat() -> Result<String> {
    let Lead { board, places, .. } = lead_panes()?;
    if places.chat_hidden() {
        herdr::zoom(&board, false)?;
        return Ok("The lead's chat is shown again.".to_string());
    }
    herdr::zoom(&board, true)?;
    Ok("The lead's chat is hidden and runs on; kelpie lead --toggle-chat, or h or tab on the board, shows it.".to_string())
}

// None unless this pane is the lead Space's board.
pub fn chat_hidden(board_pane: &str) -> Option<bool> {
    let lead = lead_workspace(&herdr::workspaces().ok()?)?;
    let places = places(&lead).ok()?;
    (places.board.as_deref() == Some(board_pane)).then(|| places.chat_hidden())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_board_renews_only_on_a_binary_whose_mtime_held_between_two_looks() {
        let at = |seconds: u64| Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds));
        let mut looks = vec![at(1), at(2), at(3), at(3)].into_iter();
        let mut pauses = 0;
        assert_eq!(settled(|| looks.next().flatten(), || pauses += 1, 10), at(3));
        assert_eq!(pauses, 3, "a binary still being written is looked at again a second later");
        let mut growing = (0..).map(at);
        assert_eq!(settled(|| growing.next().flatten(), || {}, 5), None, "a build that never settles is not renewed onto");
        assert_eq!(settled(|| None, || {}, 3), None, "a missing binary is never settled");
    }

    #[test]
    fn the_board_is_revived_once_the_lead_space_shows_up_and_never_after_the_wait() {
        let mut looks = 0;
        let mut pauses = 0;
        let late = wait_for_board(|| { looks += 1; (looks == 4).then(|| "w_lead:p1".to_string()) }, || pauses += 1, REVIVE_TRIES);
        assert_eq!((late.as_deref(), looks, pauses), (Some("w_lead:p1"), 4, 3), "a lead Space herdr restores late is still found");
        let (mut looks, mut pauses) = (0, 0);
        assert_eq!(wait_for_board(|| { looks += 1; None }, || pauses += 1, REVIVE_TRIES), None);
        assert_eq!(looks, REVIVE_TRIES, "without a lead Space the wait ends quietly");
        assert!(REVIVE_PAUSE * REVIVE_TRIES as u32 <= std::time::Duration::from_secs(60) && REVIVE_PAUSE >= std::time::Duration::from_secs(1));
    }

    #[test]
    fn herdrs_startup_revives_the_board_apart_from_the_keeper() {
        let script = include_str!("../scripts/herdr-startup.sh");
        let revive = script.lines().find(|line| line.ends_with("kelpie\" lead --revive-board")).expect("the startup revives the board");
        assert!(!revive.contains("&&") && !script.contains("enabled"), "the revive runs whether or not the keeper is enabled");
    }

    #[test]
    fn the_lead_role_mentions_linear_only_when_it_is_a_source() {
        let template = include_str!("../roles/lead.md");
        assert!(!role_text(template, &[Source::Vault], false).contains("Linear"));
        assert!(!role_text(template, &[], false).contains("Linear"));
        let both = role_text(template, &[Source::Vault, Source::Linear], false);
        assert!(both.contains("Linear (assigned to the user, current cycle)") && !both.contains("{backlog}") && !both.contains("{finding}") && !both.contains("{intake}"));
        assert!(!role_text(template, &[Source::Local], false).contains("Linear") && role_text(template, &[Source::Local], false).contains("kelpie backlog add"));
    }

    #[test]
    fn the_lead_takes_an_idea_in_as_a_draft_and_starts_or_files_it_on_yes() {
        let role = role_text(include_str!("../roles/lead.md"), &[Source::Vault], false);
        for needed in ["## Taking in ideas", "Draft it as a task note", "start it now as a task, file it in the backlog", "status `backlog`", "--backlog <the note's path>", "stays a plain task", "kelpie -C <repo> link <task>"] {
            assert!(role.contains(needed), "{needed}");
        }
    }

    #[test]
    fn the_lead_files_what_a_task_noticed_and_could_not_file() {
        let role = role_text(include_str!("../roles/lead.md"), &[Source::Local], false);
        for needed in ["- **Also noticed**: a ready task's sign-off card (`cards/<task>@<repo>.md`", "File each line marked `not filed` the way Taking in ideas files an idea", "## Taking in ideas"] {
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
    fn the_lead_puts_a_tasks_own_prompt_to_the_user_and_answers_only_on_their_choice() {
        let role = include_str!("../roles/lead.md");
        for needed in ["kelpie choose <task> <number> --prompt <id>", "Never choose yourself, not even in away mode", "grants broader access", "verbatim", "types it in this chat", "never defaults to"] {
            assert!(role.contains(needed), "{needed}");
        }
        let written: Value = serde_json::from_str(&settings("/bin/kelpie")).unwrap();
        assert!(!written["permissions"]["allow"].as_array().unwrap().iter().any(|rule| rule.as_str().is_some_and(|rule| rule.contains(" choose"))), "Claude Code asks the user before each kelpie choose the lead runs");
    }

    #[test]
    fn focus_moves_between_the_board_and_the_chat_in_the_lead_and_to_the_chat_from_elsewhere() {
        assert_eq!(hop(true, false), Some(Hop::ToChat));
        assert_eq!(hop(true, true), Some(Hop::ToBoard));
        assert_eq!(hop(false, false), Some(Hop::ToChat));
        assert_eq!(hop(false, true), None);
    }

    #[test]
    fn the_chat_is_hidden_while_the_zoom_is_on_the_board_and_still_sits_on_the_right() {
        let layout = |zoomed: bool, focused: &str| json!({ "zoomed": zoomed, "focused_pane_id": focused, "panes": [
            { "pane_id": "w_lead:p1", "rect": { "x": 0, "y": 0 } },
            { "pane_id": "w_lead:p2", "rect": { "x": 64, "y": 0 } },
        ] });
        let hidden = places_in(&layout(true, "w_lead:p1"));
        assert_eq!((hidden.board.as_deref(), hidden.chat.as_deref()), (Some("w_lead:p1"), Some("w_lead:p2")), "a hidden chat is still the lead's chat, so kelpie lead never splits a second one");
        assert!(hidden.chat_hidden());
        assert!(!places_in(&layout(false, "w_lead:p1")).chat_hidden());
        assert!(!places_in(&layout(true, "w_lead:p2")).chat_hidden(), "a zoom on the chat hides the board, not the chat");
        let alone = places_in(&json!({ "zoomed": true, "focused_pane_id": "w_lead:p1", "panes": [{ "pane_id": "w_lead:p1", "rect": { "x": 0, "y": 0 } }] }));
        assert!(alone.chat.is_none() && !alone.chat_hidden(), "a board without a chat has nothing hidden");
    }

    #[test]
    fn the_lead_role_carries_the_push_permission_the_vault_log_after_a_push_and_away_mode() {
        let role = role_text(include_str!("../roles/lead.md"), &[Source::Vault], false);
        for needed in ["push_permission", "kelpie -C <repo> push <task>", "once its reviewer passed its current code", "log_append", "with those shas", "kelpie away on|off", "kelpie decide <task>", "kelpie defer <task>", "The user is back", "what landed, what you decided and why, and what waits for them"] {
            assert!(role.contains(needed), "{needed}");
        }
        assert!(!role.contains("Nobody pushes without the user: not you"), "the lead pushes under the user's permission");
        let kept = role_text(include_str!("../roles/lead.md"), &[Source::Vault], true);
        assert!(!kept.contains("log_append") && kept.contains("kelpie has written the repo's log line") && kept.contains("Update the area note"), "with the vault on kelpie logs each push, so the lead does not log it twice");
        assert!(!kept.contains("{vault_work}") && !kept.contains("{after_push}") && !kept.contains("plain task"));
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
        assert_eq!(lead_session(&transcripts_in(&dir), Some("recorded")), None);
        let at = |name: &str, seconds: u64| {
            let file = fs::File::create(dir.join(name)).unwrap();
            file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)).unwrap();
        };
        at("older.jsonl", 100);
        at("recorded.jsonl", 200);
        at("newest.txt", 900);
        assert_eq!(lead_session(&transcripts_in(&dir), Some("recorded")).as_deref(), Some("recorded"));
        assert_eq!(lead_session(&transcripts_in(&dir), None).as_deref(), Some("recorded"));
        assert_eq!(lead_session(&transcripts_in(&dir), Some("gone")).as_deref(), Some("recorded"));
        at("after-clear.jsonl", 300);
        assert_eq!(lead_session(&transcripts_in(&dir), Some("recorded")).as_deref(), Some("after-clear"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_conversation_from_before_the_leads_own_folder_is_found_in_exactly_one_other_project_as_claude_finds_it() {
        let projects = std::env::temp_dir().join(format!("kelpie-lead-projects-{}", std::process::id()));
        let _ = fs::remove_dir_all(&projects);
        let at = |project: &str, seconds: u64| {
            fs::create_dir_all(projects.join(project)).unwrap();
            fs::File::create(projects.join(project).join("moved.jsonl")).unwrap().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)).unwrap();
        };
        assert_eq!(transcript_elsewhere(&projects, "moved"), None);
        at("-Users-ada", 500);
        assert_eq!(transcript_elsewhere(&projects, "moved"), Some((std::time::UNIX_EPOCH + std::time::Duration::from_secs(500), "moved".to_string())));
        assert_eq!(lead_session(&[transcript_elsewhere(&projects, "moved").unwrap()], Some("moved")).as_deref(), Some("moved"));
        at("-Users-ada-app", 600);
        assert_eq!(transcript_elsewhere(&projects, "moved"), None, "claude --resume refuses an id that two projects hold");
        fs::remove_dir_all(projects).unwrap();
    }

    #[test]
    fn the_lead_goes_to_its_folder_by_a_quoted_cd() {
        assert_eq!(cd_line(Path::new("/Users/ada/state dir/it's lead")), "cd '/Users/ada/state dir/it'\\''s lead'");
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
        assert!(body("start_chat").contains("crate::state::write(&session_file()?"));
        for caller in ["repair", "create"] {
            assert!(body(caller).contains("new_chat(") && !body(caller).contains("start_chat("), "{caller}");
        }
    }

    #[test]
    fn the_leads_settings_allow_only_kelpies_own_subcommands_by_full_path() {
        let written: Value = serde_json::from_str(&settings("/opt/kelpie")).unwrap();
        let allow: Vec<&str> = written["permissions"]["allow"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(allow.iter().all(|rule| rule.starts_with("Bash(/opt/kelpie ") && rule.ends_with(":*)")), "{allow:?}");
        for never in ["start", "drop", "reset", "done", "away", "tidy", "hook", "backlog"] {
            assert!(!allow.iter().any(|rule| rule.contains(&format!(" {never}:"))), "{never} is asked for, not pre-approved: {allow:?}");
        }
        assert!(allow.iter().any(|rule| rule.contains(" decide:")) && allow.iter().any(|rule| rule.contains(" defer:")));
    }

    #[test]
    fn a_settings_file_is_named_after_its_role_and_written_whole() {
        let dir = std::env::temp_dir().join(format!("kelpie-settings-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = write_settings(&dir, "billing@app.review", "{}").unwrap();
        assert_eq!(file.file_name().unwrap(), "billing@app.review.settings.json");
        assert_eq!(fs::read_to_string(&file).unwrap(), "{}");
        write_settings(&dir, "billing@app.review", "{\"a\":1}").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "{\"a\":1}", "a rewrite replaces, never appends");
        use std::os::unix::fs::PermissionsExt;
        let made = write_settings(&dir.join("missing"), "x", "{}").unwrap();
        assert_eq!(fs::metadata(&made).unwrap().permissions().mode() & 0o777, 0o600, "settings carry the role's hooks and permissions, for the user only");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_state_dir_is_created_on_first_use_under_the_test_override_only() {
        let state = state_location().unwrap();
        assert_eq!(state, PathBuf::from(std::env::var("KELPIE_STATE_DIR").unwrap()), "every test runs against its own state dir");
        assert!(roles_dir().unwrap().starts_with(&state));
    }
}
