use std::time::Duration;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::config;
use crate::herdr;
use crate::lead::state_dir;
use crate::messages;
use crate::peers;
use crate::repo::{branch_exists, default_branch, git, git_writing, main_checkout, name_of};
use crate::sh::Result;
use super::claude::{start_claude, trusted_by_user};
use super::{fetch, slug};
use super::pool::{Slot, claim_slot, slots};
use super::started::{read_start, start_entry, started_file};

// The deny rules and the env rewrite name the rule; the worktree's own push block (push_block) is what holds when they are slipped.
// gate is the task's address when kelpie keeps its note: its edits then wait for the note's Problem.
pub(super) fn settings(kelpie: &str, address: &str, gate: Option<&str>, tmp: Option<&Path>) -> String {
    let hook = json!([{ "hooks": [{ "type": "command", "command": format!("{} hook", crate::sh::quote(kelpie)) }] }]);
    let allow = ["peers", "tell", "report", "ask"].map(|command| format!("Bash({kelpie} {command}:*)"));
    let evidence = json!([{ "matcher": "Bash", "hooks": [{ "type": "command", "command": format!("{} hook evidence {}", crate::sh::quote(kelpie), crate::sh::quote(address)) }] }]);
    let mut hooks = json!({ "UserPromptSubmit": hook, "Stop": hook, "PostToolUse": evidence, "PostToolUseFailure": evidence });
    if let Some(address) = gate {
        hooks["PreToolUse"] = json!([{ "matcher": "Edit|Write|MultiEdit|NotebookEdit|Bash", "hooks": [{ "type": "command", "command": format!("{} hook gate {}", crate::sh::quote(kelpie), crate::sh::quote(address)), "timeout": 40 }] }]);
    }
    let mut env = json!({ "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "url.kelpie-blocks-push:.pushInsteadOf", "GIT_CONFIG_VALUE_0": "" });
    if let Some(tmp) = tmp {
        for name in TMP_VARS {
            env[name] = json!(tmp.to_string_lossy());
        }
    }
    json!({
        "env": env,
        "permissions": { "deny": ["Bash(git push:*)".to_string(), format!("Bash({kelpie} approve:*)"), format!("Bash({kelpie} push:*)")], "allow": allow },
        "hooks": hooks,
    }).to_string()
}

pub(super) const TMP_VARS: [&str; 4] = ["TMPDIR", "TMP", "TEMP", "CLAUDE_CODE_TMPDIR"];

// A task's temporary files live in its worktree's own git dir, so they go with the task and never land in the repo.
pub(super) fn task_tmp(worktree: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git(worktree, &["rev-parse", "--absolute-git-dir"])?).join("kelpie-tmp"))
}

pub(super) fn clear_tmp(worktree: &Path) {
    if let Ok(dir) = task_tmp(worktree) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

pub(crate) enum Session {
    New(String),
    Resume(String),
}

impl Session {
    pub(crate) fn args(&self) -> [String; 2] {
        match self {
            Session::New(id) => ["--session-id".to_string(), id.clone()],
            Session::Resume(id) => ["--resume".to_string(), id.clone()],
        }
    }
}

fn claude_args(mode: Option<&str>, settings: &Path, role: &Path, session: &Session) -> Vec<String> {
    let mut args: Vec<String> = mode.map(|mode| vec!["--permission-mode".to_string(), mode.to_string()]).unwrap_or_default();
    args.extend(["--settings".to_string(), settings.to_string_lossy().to_string(), "--append-system-prompt-file".to_string(), role.to_string_lossy().to_string()]);
    args.extend(session.args());
    args
}

pub(crate) fn new_session_id() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut random| random.read_exact(&mut bytes)).map_err(|error| error.to_string())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

fn session_file(task: &str, repo: &Path) -> Result<PathBuf> {
    Ok(crate::lead::state_location()?.join("sessions").join(peers::address(task, &name_of(repo))))
}

// The task's Claude session id and permission mode, one per line, so a restart resumes that conversation in the same mode.
fn recorded_session(task: &str, repo: &Path) -> (Option<String>, Option<String>) {
    let text = session_file(task, repo).ok().and_then(|file| std::fs::read_to_string(file).ok()).unwrap_or_default();
    let mut lines = text.lines().map(str::trim).map(|line| (!line.is_empty()).then(|| line.to_string()));
    (lines.next().flatten(), lines.next().flatten())
}

pub(super) fn forget_session(task: &str, repo: &Path) {
    if let Ok(file) = session_file(task, repo) {
        let _ = std::fs::remove_file(prompt_file(&file));
        let _ = std::fs::remove_file(file);
    }
}

fn prompt_file(session: &Path) -> PathBuf {
    let mut name = session.file_name().unwrap_or_default().to_os_string();
    name.push(".prompt");
    session.with_file_name(name)
}

fn store_prompt(file: &Path, prompt: Option<&str>) -> Result<()> {
    match prompt {
        Some(prompt) => crate::state::write(file, prompt),
        None => std::fs::remove_file(file).or_else(|error| if error.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(error.to_string()) }),
    }
}

fn stored_prompt(file: &Path) -> Option<String> {
    std::fs::read_to_string(file).ok().filter(|prompt| !prompt.trim().is_empty())
}

// A prompt given with the start always goes in; the stored first prompt goes to a new conversation, and to a resumed one that never heard a user turn, as when the first start's shell asked a question before Claude ran.
fn prompt_for(session: &Session, given: Option<&str>, stored: Option<String>, unheard: bool) -> Option<String> {
    if let Some(given) = given {
        return Some(given.to_string());
    }
    match session {
        Session::New(_) => stored,
        Session::Resume(_) if unheard => stored,
        Session::Resume(_) => None,
    }
}

// Claude Code keeps a folder's conversations as <session id>.jsonl under its projects dir, the folder's path with every other character than a letter or digit as '-'.
pub(crate) fn transcripts_dir(claude_home: &Path, folder: &Path) -> PathBuf {
    let name: String = folder.to_string_lossy().chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '-' }).collect();
    claude_home.join("projects").join(name)
}

pub fn claude_home() -> Option<PathBuf> {
    std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).ok().or_else(|| std::env::var("HOME").ok().map(|home| Path::new(&home).join(".claude")))
}

// A slot is reused across tasks, so only a conversation written since this task's branch was created can be this task's.
fn latest_transcript(dir: &Path, since: std::time::SystemTime) -> Option<String> {
    std::fs::read_dir(dir).ok()?.flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "jsonl"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .filter(|(modified, _)| *modified >= since)
        .max_by_key(|(modified, _)| *modified)
        .and_then(|(_, path)| path.file_stem().map(|stem| stem.to_string_lossy().to_string()))
}

fn branch_created(repo: &Path, task: &str) -> Option<std::time::SystemTime> {
    let times = git(repo, &["reflog", "show", "--format=%ct", &format!("refs/heads/{task}")]).ok()?;
    let seconds: u64 = times.lines().last()?.trim().parse().ok()?;
    Some(std::time::UNIX_EPOCH + Duration::from_secs(seconds))
}

fn previous_session(repo: &Path, slot: &Slot, task: &str, claude_home: &Path, recorded: Option<String>) -> Option<String> {
    let dir = transcripts_dir(claude_home, &slot.path);
    recorded.filter(|id| dir.join(format!("{id}.jsonl")).exists())
        .or_else(|| latest_transcript(&dir, branch_created(repo, task)?))
}

// The task's own conversation, where the user's prompt and later word to it are.
pub fn transcript(repo: &Path, slot: &Slot) -> Option<PathBuf> {
    let task = slot.task.as_deref()?;
    let home = claude_home()?;
    let id = previous_session(repo, slot, task, &home, recorded_session(task, repo).0)?;
    Some(transcripts_dir(&home, &slot.path).join(format!("{id}.jsonl")))
}

pub(super) fn open_space(repo: &Path, slot: &Slot, task: &str, focus: bool) -> Result<herdr::Space> {
    let space = herdr::open_worktree(repo, &slot.path, task, focus)?;
    herdr::rename_workspace(&space.workspace_id, task)?;
    Ok(space)
}

pub struct Start<'a> {
    pub prompt: Option<&'a str>,
    pub focus: bool,
    pub from: Option<&'a str>,
    pub mode: Option<&'a str>,
    pub backlog: Option<&'a str>,
}

pub fn start(cwd: &Path, name: &str, options: &Start) -> Result<String> {
    let Start { prompt, focus, from, mode, backlog } = *options;
    let repo = main_checkout(cwd)?;
    let task = slug(name)?;
    let config::Settings { setup_skill, setup_command, rules_skill, .. } = config::resolve(Some(&repo))?.settings;
    let (setup_skill, setup_command, rules_skill) = (setup_skill.value, config::setup_command_in(setup_command.value, &repo), rules_skill.value);
    let vault = crate::vault_cli::detect();
    let place = |slot: &Slot| Launch { repo: &repo, task: &task, mode, setup_skill: &setup_skill, setup_command: setup_command.as_deref(), rules_skill: &rules_skill, worktree: slot.path.clone(), vault: vault.is_some(), note: None };
    if let Some(existing) = slots(&repo)?.into_iter().find(|slot| slot.task.as_deref() == Some(task.as_str())) {
        return restart(&place(&existing), &existing, focus, prompt);
    }
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    let base = from.unwrap_or(&upstream);
    let slot = claim_slot(&repo, &upstream)?;
    if branch_exists(&repo, &task) {
        git_writing(&slot.path, &["switch", "--quiet", &task])?;
    } else {
        git_writing(&slot.path, &["switch", "--quiet", "-c", &task, base])?;
    }

    let address = peers::address(&task, &name_of(&repo));
    let shared = vault.as_ref().map(|_| state_dir().map(|state| crate::task_note::shared(&state, &task, &address))).transpose()?.flatten();
    let (note, vault_said) = match (&vault, shared) {
        (_, Some(note)) => (Some(note.clone()), format!(" Its note is {note}, shared with its other halves.")),
        (Some(cli), None) => match crate::task_note::open(cli, &task, &slot.path, backlog) {
            Ok(note) => (Some(note.clone()), format!(" Its note is {note}.")),
            Err(error) => (None, format!(" Its vault note was not opened ({error}); the task runs without one.")),
        },
        (None, None) => (None, String::new()),
    };
    if note.is_some() {
        let entry = start_entry(note.as_deref(), backlog).unwrap_or_default();
        started_file(&task, &repo).and_then(|file| crate::state::write(&file, entry))?;
    }
    store_prompt(&prompt_file(&session_file(&task, &repo)?), prompt)?;
    let space = open_space(&repo, &slot, &task, false)?;
    let claude_pane = match space.root_pane_id {
        Some(pane) => pane,
        None => herdr::create_tab(&space.workspace_id, &slot.path, "claude")?,
    };
    let (agent_name, started) = launch(&Launch { note: note.as_deref(), ..place(&slot) }, &claude_pane, Session::New(new_session_id()?))?;
    // The prompt goes in as a paste: as an argument, herdr would have to quote it for the shell.
    let prompted = match (&started, prompt) {
        (Ok(()), Some(prompt)) => crate::typing::prompt_confirmed(&agent_name, prompt, &address).map_err(|error| format!("Started {task} in {}, but its prompt did not reach Claude: {error}", slot.path.display())),
        _ => Ok(()),
    };
    if focus {
        herdr::focus_workspace(&space.workspace_id)?;
    }
    started?;
    prompted?;
    if let Some(item) = backlog.filter(|_| note.is_none()) {
        started_file(&task, &repo).and_then(|file| crate::state::write(&file, item))?;
    }
    if let Err(error) = messages::announce(&task, &name_of(&repo)) {
        eprintln!("Could not tell {task}'s other halves it started: {error}");
    }
    Ok(format!("Started {task} in {}.{vault_said}", slot.path.display()))
}

struct Launch<'a> {
    repo: &'a Path,
    task: &'a str,
    mode: Option<&'a str>,
    setup_skill: &'a str,
    setup_command: Option<&'a str>,
    rules_skill: &'a str,
    worktree: PathBuf,
    vault: bool,
    note: Option<&'a str>,
}

// Starts the task's Claude in the pane with its role and settings; the outer error is setup, the inner one is Claude not starting.
fn launch(place: &Launch, pane: &str, session: Session) -> Result<(String, Result<()>)> {
    let Launch { repo, task, mode, setup_skill, setup_command, rules_skill, ref worktree, vault, note } = *place;
    crate::push_block::warn_if_off(crate::push_block::install(worktree, &state_dir()?)?);
    // herdr's agent names are global, so a task's half in another repo is named with its repo.
    let taken = herdr::agents()?.iter().any(|agent| agent["name"] == task);
    let agent_name: String = if taken { format!("{task}_{}", slug(&name_of(repo))?) } else { task.to_string() }.chars().take(32).collect();
    let address = peers::address(task, &name_of(repo));
    let note = note.filter(|_| vault);
    let tmp = task_tmp(worktree)?;
    std::fs::create_dir_all(&tmp).map_err(|error| format!("{}: {error}", tmp.display()))?;
    let settings = crate::lead::write_settings(&crate::lead::roles_dir()?, &address, &settings(&peers::kelpie_command()?, &address, note.map(|_| address.as_str()), Some(&tmp)))?;
    let role = peers::role_file(task, &peers::Place { repo, worktree, setup_skill, setup_command, rules_skill, note })?;
    let agent_args = claude_args(mode, &settings, &role, &session);
    let started = start_claude(&agent_name, pane, &agent_args.iter().map(String::as_str).collect::<Vec<_>>(), trusted_by_user(repo));
    if let (Ok(()), Session::New(id)) = (&started, &session) {
        let record = format!("{id}\n{}\n", mode.unwrap_or_default());
        let file = session_file(task, repo)?;
        crate::state::write(&file, record)?;
    }
    Ok((agent_name, started))
}

fn claude_in(workspace: &str) -> Result<bool> {
    Ok(herdr::agents()?.iter().any(|agent| agent["agent"] == "claude" && agent["workspace_id"].as_str() == Some(workspace)))
}

// A task whose Space or Claude is gone gets them back: its last conversation resumed, or a new one given the task's first prompt again.
fn restart(place: &Launch, slot: &Slot, focus: bool, given: Option<&str>) -> Result<String> {
    let task = place.task;
    let open = herdr::workspace_for(&slot.path)?;
    if let Some(workspace) = &open
        && claude_in(workspace)?
    {
        if focus {
            herdr::focus_workspace(workspace)?;
        }
        return Ok(format!("{task} is already running in {}.", slot.path.display()));
    }
    let space = open_space(place.repo, slot, task, false)?;
    let pane = match (&open, space.root_pane_id) {
        (None, Some(pane)) => pane,
        _ => herdr::create_tab(&space.workspace_id, &slot.path, "claude")?,
    };
    let (recorded, mode) = recorded_session(task, place.repo);
    let previous = claude_home().and_then(|home| previous_session(place.repo, slot, task, &home, recorded));
    let note = read_start(task, place.repo).and_then(|text| crate::task_note::note_in(&text));
    let place = Launch { mode: place.mode.or(mode.as_deref()), worktree: place.worktree.clone(), note: note.as_deref(), ..*place };
    let session = match previous {
        Some(id) => Session::Resume(id),
        None => Session::New(new_session_id()?),
    };
    let resumed = matches!(session, Session::Resume(_));
    let prompt_path = session_file(task, place.repo).ok().map(|file| prompt_file(&file));
    let stored = prompt_path.as_deref().and_then(stored_prompt);
    if let (Some(given), None, Some(path)) = (given, &stored, &prompt_path) {
        store_prompt(path, Some(given))?;
    }
    let unheard = match &session {
        Session::Resume(id) => claude_home().and_then(|home| std::fs::read_to_string(transcripts_dir(&home, &slot.path).join(format!("{id}.jsonl"))).ok()).is_none_or(|text| crate::review::user_messages(&text).is_empty()),
        Session::New(_) => true,
    };
    let prompt = prompt_for(&session, given, stored, unheard);
    let (agent_name, started) = launch(&place, &pane, session)?;
    let prompted = match (&started, &prompt) {
        (Ok(()), Some(prompt)) => crate::typing::prompt_confirmed(&agent_name, prompt, &peers::address(task, &name_of(place.repo))).map_err(|error| format!("Restarted {task} in {}, but its prompt did not reach Claude: {error}", slot.path.display())),
        _ => Ok(()),
    };
    if focus {
        herdr::focus_workspace(&space.workspace_id)?;
    }
    started?;
    prompted?;
    let how = match (resumed, prompt.is_some()) {
        (true, true) => "resuming its last conversation, and sent it the prompt",
        (true, false) => "resuming its last conversation",
        (false, true) => "with a new conversation, as none of its own was found, and sent it the task's prompt again",
        (false, false) => "with a new conversation, as none of its own was found; it had no prompt to send again",
    };
    Ok(format!("Restarted {task} in {}, {how}.", slot.path.display()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::*;
    use crate::tasks::test_support::*;
    use crate::peers;

    #[test]
    fn edits_are_gated_only_in_a_task_started_with_the_vault_on() {
        let off: Value = serde_json::from_str(&settings("/bin/kelpie", "billing@app", None, None)).unwrap();
        assert!(off["hooks"].get("PreToolUse").is_none(), "without the vault no edit hook runs");
        for event in ["PostToolUse", "PostToolUseFailure"] {
            assert_eq!(off["hooks"][event][0]["matcher"], "Bash");
            assert_eq!(off["hooks"][event][0]["hooks"][0]["command"], "'/bin/kelpie' hook evidence 'billing@app'", "every task records its test runs, passed and failed");
        }
        let on: Value = serde_json::from_str(&settings("/bin/kelpie", "billing@app", Some("billing@app"), None)).unwrap();
        assert_eq!(on["hooks"]["PreToolUse"][0]["matcher"], "Edit|Write|MultiEdit|NotebookEdit|Bash", "a Bash command that writes a file waits for the Problem too");
        assert_eq!(on["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "'/bin/kelpie' hook gate 'billing@app'", "the hook is told its task, so it runs no git and no herdr to find it");
        assert_eq!(on["hooks"]["Stop"], off["hooks"]["Stop"]);
    }

    #[test]
    fn every_task_worktree_gets_the_push_block_before_its_claude_or_its_work_arrives() {
        let source = [include_str!("launch.rs"), include_str!("pool.rs")].concat();
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).or_else(|| source.find(&format!("\npub(super) fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("launch").find("push_block::install(").unwrap() < body("launch").find("start_claude(").unwrap(), "a start and a restart both block pushes before Claude runs");
        assert!(body("move_work").find("push_block::install(").unwrap() < body("move_work").find("stash\", \"pop").unwrap(), "moved work lands in a blocked worktree");
    }

    #[test]
    fn a_tasks_start_line_carries_its_settings_as_a_short_path_to_the_same_json() {
        let dir = std::env::temp_dir().join(format!("kelpie-task-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kelpie = format!("/Users/someone-with-a-long-name/projects/{}/target/release/kelpie", "nested/".repeat(20));
        let file = crate::lead::write_settings(&dir, &peers::address("billing", "app"), &settings(&kelpie, "billing@app", None, None)).unwrap();
        assert_eq!(file, dir.join("billing@app.settings.json"));
        let args = claude_args(Some("acceptEdits"), &file, &dir.join("billing@app.md"), &Session::New("0b6a".to_string()));
        let line = args.join(" ");
        assert!(!line.contains('{') && line.len() < 512, "{line}");
        assert_eq!(args[..2], ["--permission-mode", "acceptEdits"]);
        assert_eq!(args[args.iter().position(|arg| arg == "--settings").unwrap() + 1], file.to_string_lossy());
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written, serde_json::from_str::<Value>(&settings(&kelpie, "billing@app", None, None)).unwrap());
        assert_eq!(written["permissions"]["deny"], json!(["Bash(git push:*)", format!("Bash({kelpie} approve:*)"), format!("Bash({kelpie} push:*)")]), "a task never pushes nor approves its own commits");
        assert_eq!(written["hooks"]["Stop"][0]["hooks"][0]["command"], format!("'{kelpie}' hook"));
        assert!(written["permissions"]["allow"].as_array().unwrap().contains(&json!(format!("Bash({kelpie} ask:*)"))), "a task asks the user without a permission prompt");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_restart_sends_the_stored_prompt_to_a_new_or_unheard_conversation_and_a_given_prompt_always() {
        let dir = crate::sh::test_dir("task-prompt");
        let file = prompt_file(&dir.join("sessions").join("billing@my.app"));
        assert_eq!(file, dir.join("sessions").join("billing@my.app.prompt"));
        assert_eq!(stored_prompt(&file), None);
        store_prompt(&file, Some("Fix the invoice rounding")).unwrap();
        assert_eq!(stored_prompt(&file).as_deref(), Some("Fix the invoice rounding"));
        let (new, resume) = (Session::New("1f0c".to_string()), Session::Resume("1f0c".to_string()));
        assert_eq!(prompt_for(&new, None, stored_prompt(&file), true).as_deref(), Some("Fix the invoice rounding"));
        assert_eq!(prompt_for(&resume, None, stored_prompt(&file), false), None, "a resumed conversation that heard its prompt already has it");
        assert_eq!(prompt_for(&resume, None, stored_prompt(&file), true).as_deref(), Some("Fix the invoice rounding"), "a first start whose shell asked a question left a conversation with no user turn");
        assert_eq!(prompt_for(&resume, Some("Use the new card"), stored_prompt(&file), false).as_deref(), Some("Use the new card"), "a prompt given on a resume is never dropped");
        store_prompt(&file, None).unwrap();
        assert_eq!(prompt_for(&new, None, stored_prompt(&file), true), None);
        std::fs::remove_dir_all(dir).unwrap();
        let source = include_str!("launch.rs");
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).or_else(|| source.find(&format!("\npub(super) fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("start").find("store_prompt(").unwrap() < body("start").find("launch(").unwrap(), "the prompt is kept even when the first start fails");
        assert!(body("start").contains("restart(&place(&existing), &existing, focus, prompt)"), "start's prompt reaches the resume path");
        assert!(body("restart").contains("prompt_for(&session, given, stored, unheard)") && body("restart").contains("typing::prompt_confirmed(&agent_name, prompt"));
        assert!(body("restart").contains("user_messages(&text).is_empty()"));
        assert!(body("forget_session").contains("prompt_file("));
    }

    #[test]
    fn a_new_task_names_its_session_and_a_restart_resumes_it_with_the_same_role_and_settings() {
        let (settings, role) = (Path::new("/state/roles/billing@app.settings.json"), Path::new("/state/roles/billing@app.md"));
        let new = claude_args(Some("acceptEdits"), settings, role, &Session::New("1f0c".to_string()));
        let resumed = claude_args(Some("acceptEdits"), settings, role, &Session::Resume("1f0c".to_string()));
        assert_eq!(new[new.len() - 2..], ["--session-id", "1f0c"]);
        assert_eq!(resumed[resumed.len() - 2..], ["--resume", "1f0c"]);
        assert_eq!(new[..new.len() - 2], resumed[..resumed.len() - 2]);
        assert!(resumed.contains(&"--append-system-prompt-file".to_string()) && resumed.contains(&"--settings".to_string()));
        assert!(!resumed.iter().any(|arg| arg == "--continue" || arg == "-p"));
        let id = new_session_id().unwrap();
        assert_eq!((id.len(), id.matches('-').count(), &id[14..15]), (36, 4, "4"));
        assert_ne!(id, new_session_id().unwrap());
    }

    #[test]
    fn a_restart_resumes_the_conversation_recorded_for_the_task_or_else_the_newest_one_since_its_branch_began() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let home = dir.with_extension("claude");
        let transcripts = transcripts_dir(&home, &slot.path);
        assert_eq!(transcripts.parent().unwrap(), home.join("projects"));
        assert!(!transcripts.file_name().unwrap().to_string_lossy().contains(['/', '.']));
        std::fs::create_dir_all(&transcripts).unwrap();
        assert_eq!(previous_session(&dir, &slot, "billing", &home, None), None);
        let created = branch_created(&dir, "billing").unwrap();
        let write = |id: &str, at: std::time::SystemTime| {
            let file = std::fs::File::create(transcripts.join(format!("{id}.jsonl"))).unwrap();
            file.set_modified(at).unwrap();
        };
        write("previous-task", created - Duration::from_secs(60));
        assert_eq!(previous_session(&dir, &slot, "billing", &home, None), None);
        write("first", created + Duration::from_secs(10));
        write("latest", created + Duration::from_secs(20));
        assert_eq!(previous_session(&dir, &slot, "billing", &home, None).as_deref(), Some("latest"));
        assert_eq!(previous_session(&dir, &slot, "billing", &home, Some("first".to_string())).as_deref(), Some("first"));
        assert_eq!(previous_session(&dir, &slot, "billing", &home, Some("gone".to_string())).as_deref(), Some("latest"));
        std::fs::remove_dir_all(home).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_transcripts_folder_is_the_worktree_path_with_every_other_character_as_a_dash() {
        assert_eq!(transcripts_dir(Path::new("/home/ada/.claude"), Path::new("/src/wt1.app")), PathBuf::from("/home/ada/.claude/projects/-src-wt1-app"));
        assert_eq!(transcripts_dir(Path::new("/c"), Path::new("/Users/Ada Ö/x_y")).file_name().unwrap(), "-Users-Ada---x-y");
        assert!(!transcripts_dir(Path::new("/c"), Path::new("/a/../b")).file_name().unwrap().to_string_lossy().contains('.'));
    }

    #[test]
    fn a_tasks_settings_deny_push_and_approve_in_every_spelling_claude_code_matches() {
        let on: Value = serde_json::from_str(&settings("/bin/kelpie", "billing@app", Some("billing@app"), None)).unwrap();
        let deny: Vec<&str> = on["permissions"]["deny"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert_eq!(deny, ["Bash(git push:*)", "Bash(/bin/kelpie approve:*)", "Bash(/bin/kelpie push:*)"]);
        let allow: Vec<&str> = on["permissions"]["allow"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(!allow.iter().any(|rule| rule.contains("push") || rule.contains("approve") || rule.contains("decide") || rule.contains("away")), "{allow:?}");
        assert_eq!(on["env"]["GIT_CONFIG_KEY_0"], "url.kelpie-blocks-push:.pushInsteadOf");
        assert_eq!(on["env"]["GIT_CONFIG_VALUE_0"], "");
        assert_eq!(on["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 40);
    }

    #[test]
    fn the_hook_commands_run_from_a_path_with_an_apostrophe() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("kelpie-hook-quote-{}", std::process::id())).join("o'brien");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let kelpie = dir.join("kelpie");
        std::fs::write(&kelpie, "#!/bin/sh\necho ran \"$@\"\n").unwrap();
        std::fs::set_permissions(&kelpie, std::fs::Permissions::from_mode(0o755)).unwrap();
        let written: Value = serde_json::from_str(&settings(&kelpie.to_string_lossy(), "billing@it's", Some("billing@it's"), None)).unwrap();
        for command in [&written["hooks"]["Stop"][0]["hooks"][0]["command"], &written["hooks"]["PreToolUse"][0]["hooks"][0]["command"], &written["hooks"]["PostToolUse"][0]["hooks"][0]["command"]] {
            let output = std::process::Command::new("sh").args(["-c", command.as_str().unwrap()]).output().unwrap();
            assert!(output.status.success(), "{command}: {}", String::from_utf8_lossy(&output.stderr));
            assert!(String::from_utf8_lossy(&output.stdout).starts_with("ran hook"), "{command}");
        }
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }
}
