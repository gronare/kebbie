use std::collections::{HashMap, HashSet};
use std::time::Duration;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::board::widgets::plural;
use crate::config;
use crate::herdr;
use crate::lead::state_dir;
use crate::messages;
use crate::peers;
use crate::repo::{
    ahead_behind, branch_exists, changed_count, default_branch, git, git_writing, is_clean, main_checkout, name_of,
    setup_line, slot_number, slot_path, worktrees,
};
use crate::sh::Result;

// Git is the only state: a slot on a task branch is busy; a clean detached or wt<N> slot is free.
pub struct Slot {
    pub number: usize,
    pub path: PathBuf,
    pub task: Option<String>,
    pub free: bool,
}

pub fn slots(repo: &Path) -> Result<Vec<Slot>> {
    let mut list = Vec::new();
    for worktree in worktrees(repo)? {
        let Some(number) = slot_number(repo, &worktree.path) else { continue };
        if !worktree.path.exists() {
            continue;
        }
        let placeholder = worktree.branch.as_deref().is_none_or(|branch| branch == format!("wt{number}"));
        let free = placeholder && is_clean(&worktree.path)?;
        let task = if placeholder { None } else { worktree.branch };
        list.push(Slot { number, path: worktree.path, task, free });
    }
    list.sort_by_key(|slot| slot.number);
    Ok(list)
}

// Task names are ASCII for git and herdr, so a letter with a diacritic becomes its base letter instead of a gap.
pub fn ascii_fold(text: &str) -> String {
    text.chars().map(|character| match character {
        'å' | 'ä' | 'à' | 'á' | 'â' | 'ã' => "a".to_string(),
        'Å' | 'Ä' | 'À' | 'Á' | 'Â' | 'Ã' => "A".to_string(),
        'ö' | 'ø' | 'ò' | 'ó' | 'ô' | 'õ' => "o".to_string(),
        'Ö' | 'Ø' | 'Ò' | 'Ó' | 'Ô' | 'Õ' => "O".to_string(),
        'é' | 'è' | 'ê' | 'ë' => "e".to_string(),
        'É' | 'È' | 'Ê' | 'Ë' => "E".to_string(),
        'ü' | 'ù' | 'ú' | 'û' => "u".to_string(),
        'Ü' | 'Ù' | 'Ú' | 'Û' => "U".to_string(),
        'í' | 'ì' | 'î' | 'ï' => "i".to_string(),
        'ç' => "c".to_string(),
        'ñ' => "n".to_string(),
        'æ' => "ae".to_string(),
        'Æ' => "AE".to_string(),
        'ß' => "ss".to_string(),
        other => other.to_string(),
    }).collect()
}

pub fn slug(name: &str) -> Result<String> {
    let mut slug = String::new();
    for character in ascii_fold(&name.to_lowercase()).chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.chars().take(40).collect::<String>().trim_end_matches('-').to_string();
    if !slug.starts_with(|character: char| character.is_ascii_lowercase()) {
        return Err(format!("\"{name}\" needs to start with a letter to work as a task name"));
    }
    Ok(slug)
}

pub fn find_task(repo: &Path, name: Option<&str>, cwd: &Path) -> Result<Slot> {
    let wanted = name.map(slug).transpose()?;
    let found = slots(repo)?.into_iter().find(|slot| match &wanted {
        Some(task) => slot.task.as_ref() == Some(task),
        None => slot.task.is_some() && cwd.starts_with(&slot.path),
    });
    found.ok_or_else(|| match name {
        Some(name) => format!("No task \"{name}\" in {}'s worktrees", name_of(repo)),
        None => "Not inside a task's worktree; name the task".to_string(),
    })
}

pub fn task_of(slot: &Slot) -> &str {
    slot.task.as_deref().unwrap_or_default()
}

fn fetch(repo: &Path) -> Result<()> {
    crate::fetch::shared(repo, || git(repo, &["fetch", "--quiet", "origin"]).map(|_| ()))
}

fn create_slot(repo: &Path, upstream: &str) -> Result<Slot> {
    let taken: HashSet<usize> = worktrees(repo)?.iter().filter_map(|worktree| slot_number(repo, &worktree.path)).collect();
    let mut number = 1;
    while taken.contains(&number) || slot_path(repo, number).exists() {
        number += 1;
    }
    let path = slot_path(repo, number);
    git(repo, &["worktree", "add", "--detach", &path.to_string_lossy(), upstream])?;
    Ok(Slot { number, path, task: None, free: true })
}

// The deny rules and the env rewrite name the rule; the worktree's own push block (push_block) is what holds when they are slipped.
// gate is the task's address when kelpie keeps its note: its edits then wait for the note's Problem.
fn settings(kelpie: &str, address: &str, gate: Option<&str>, tmp: Option<&Path>) -> String {
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

const TMP_VARS: [&str; 4] = ["TMPDIR", "TMP", "TEMP", "CLAUDE_CODE_TMPDIR"];

// A task's temporary files live in its worktree's own git dir, so they go with the task and never land in the repo.
fn task_tmp(worktree: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git(worktree, &["rev-parse", "--absolute-git-dir"])?).join("kelpie-tmp"))
}

fn clear_tmp(worktree: &Path) {
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

fn forget_session(task: &str, repo: &Path) {
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

fn open_space(repo: &Path, slot: &Slot, task: &str, focus: bool) -> Result<herdr::Space> {
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

// The backlog item a running task was started from: the Backlog column hides it while the task runs, and a reset brings it back.
fn started_file(task: &str, repo: &Path) -> Result<PathBuf> {
    started_file_in(&crate::lead::state_dir()?, task, repo)
}

fn started_file_in(state: &Path, task: &str, repo: &Path) -> Result<PathBuf> {
    let dir = state.join("started");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join(peers::address(task, &name_of(repo))))
}

pub fn started_items() -> HashSet<String> {
    started_links().into_values().flatten().collect()
}

// Each running task's address (task@repo) and the items it was started from or linked to: a vault note's key (Projects/x.md), a Linear id, or local:<Title>.
pub fn started_links() -> HashMap<String, Vec<String>> {
    let Ok(dir) = crate::lead::state_dir().map(|dir| dir.join("started")) else { return HashMap::new() };
    std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|entry| Some((entry.file_name().to_str()?.to_string(), crate::task_note::items(&std::fs::read_to_string(entry.path()).ok()?))))
        .filter(|(_, items)| !items.is_empty()).collect()
}

pub fn link(cwd: &Path, name: &str, item: &str) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let task = task_of(&find_task(&repo, Some(name), cwd)?).to_string();
    let item = item.trim();
    if item.is_empty() {
        return Err("kelpie link needs the item: a vault note's path, a Linear id, or local:<Title>".to_string());
    }
    let previous = record_start(&started_file(&task, &repo)?, item)?;
    let kept = crate::vault_cli::is_note(item) && crate::vault_cli::detect().is_some();
    Ok(link_text(&task, item, previous.as_deref(), kept))
}

fn link_text(task: &str, item: &str, previous: Option<&str>, kept: bool) -> String {
    let was = previous.filter(|previous| *previous != item).map_or(String::new(), |previous| format!(" (it was linked to {})", previous.replace('\n', ", ")));
    if kept {
        return format!("Linked {task} to {item}{was}: kelpie closes the note when the task is done, and a reset or a drop puts it back in the backlog.");
    }
    format!("Linked {task} to {item}{was}: when it is done the lead is told to log and close it, and a reset puts it back in the backlog.")
}

enum NoteBack {
    Shared,
    Back(String),
    Failed(String),
}

// A note another half still holds stays as it is; the last half puts it back in the backlog.
fn note_to_backlog(cli: &crate::vault_cli::Cli, state: &Path, task: &str, address: &str, note: &str) -> NoteBack {
    if crate::task_note::held_elsewhere(state, task, address, note) {
        return NoteBack::Shared;
    }
    match crate::task_note::to_backlog(cli, note) {
        Ok(path) => NoteBack::Back(path),
        Err(error) => NoteBack::Failed(error),
    }
}

fn read_start(task: &str, repo: &Path) -> Option<String> {
    started_file(task, repo).ok().and_then(|file| std::fs::read_to_string(file).ok()).map(|item| item.trim().to_string()).filter(|item| !item.is_empty())
}

// What start writes in started/: the note on its own line, the backlog item it came from under it when that is not the note.
fn start_entry(note: Option<&str>, backlog: Option<&str>) -> Option<String> {
    let backlog = backlog.filter(|item| Some(*item) != note);
    match (note, backlog) {
        (Some(note), Some(item)) => Some(format!("{note}\n{item}")),
        (Some(only), None) | (None, Some(only)) => Some(only.to_string()),
        (None, None) => None,
    }
}

fn record_start(file: &Path, item: &str) -> Result<Option<String>> {
    let previous = std::fs::read_to_string(file).ok().map(|item| item.trim().to_string());
    crate::state::write(file, item)?;
    Ok(previous)
}

fn forget_start(task: &str, repo: &Path) -> Option<String> {
    forget_start_in(&crate::lead::state_dir().ok()?, task, repo)
}

fn forget_start_in(state: &Path, task: &str, repo: &Path) -> Option<String> {
    let file = started_file_in(state, task, repo).ok()?;
    let item = std::fs::read_to_string(&file).ok();
    let _ = std::fs::remove_file(file);
    item.map(|item| item.trim().to_string())
}

// Stops a task for good: its work is discarded, its worktree and Space go, and the backlog item it came from returns.
pub fn reset(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot).to_string();
    let upstream = default_branch(&repo)?;
    let discarded = git(&slot.path, &["log", "--format=%h %s", &format!("{upstream}..HEAD")]).unwrap_or_default();
    let items = forget_start(&task, &repo).map(|text| crate::task_note::items(&text)).unwrap_or_default();
    let state = state_dir()?;
    let address = peers::address(&task, &name_of(&repo));
    let vault = items.iter().any(|item| crate::vault_cli::is_note(item)).then(crate::vault_cli::detect).flatten();
    let dropped = discard(&repo, Some(&task), true, "reset")?;
    let mut notice = format!("The user reset {task}: it is gone, and its work was discarded and will never be pushed.");
    if !discarded.is_empty() {
        notice.push_str(&format!(" Discarded: {}.", discarded.lines().collect::<Vec<_>>().join("; ")));
    }
    let mut said = String::new();
    for item in &items {
        match vault.as_ref().filter(|_| crate::vault_cli::is_note(item)).map(|cli| note_to_backlog(cli, &state, &task, &address, item)) {
            Some(NoteBack::Shared) => said.push_str(&format!(" Its note {item} stays open for the task's other halves.")),
            Some(NoteBack::Back(path)) => notice.push_str(&format!(" kelpie put its note {path} back in the backlog.")),
            Some(NoteBack::Failed(error)) => {
                said.push_str(&format!(" Its note {item} could not be put back in the backlog ({error}); set its status to backlog in the vault."));
                notice.push_str(&crate::backlog::reset_notice(item));
            }
            None => notice.push_str(&crate::backlog::reset_notice(item)),
        }
    }
    notice.push_str(" Drop any review or recommendation about it.");
    if let Err(error) = messages::notify_lead(&notice) {
        eprintln!("Could not tell the lead: {error}");
    }
    Ok(format!("{dropped}{}{said}", if items.is_empty() { "" } else { " Its backlog item is back in the Backlog column." }))
}

enum Refusal {
    NotYet,
    Starting,
    Final,
}

// A new pane's shell takes a moment to reach its prompt; until then herdr calls the pane busy, or on macOS measures the line against a shorter limit and asks for a retry; a Claude that just exited holds its agent name a moment longer.
fn refusal(error: &str) -> Refusal {
    if error.contains("agent_pane_busy") || error.contains("wait for the shell prompt and retry") || error.contains("agent_name_taken") {
        Refusal::NotYet
    } else if error.contains("agent_not_ready") || error.contains("timeout") {
        Refusal::Starting
    } else {
        Refusal::Final
    }
}

#[derive(Debug, PartialEq)]
enum Launched {
    Ready,
    ShellBack(String),
}

// herdr types Claude's command into the pane's shell, so it is typed only once that shell is idle at its prompt.
pub fn start_claude(name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<()> {
    start_twice(pane, || {
        wait_for_prompt(pane)?;
        type_claude(name, pane, args, trusted)
    })
}

// A line read away by the shell's own startup, as oh-my-zsh's update question takes its first key, leaves the shell at its prompt: one more try then lands.
fn start_twice(pane: &str, mut attempt: impl FnMut() -> Result<Launched>) -> Result<()> {
    let mut shown = String::new();
    for _ in 0..2 {
        match attempt()? {
            Launched::Ready => return Ok(()),
            Launched::ShellBack(line) => shown = line,
        }
    }
    Err(format!("Claude did not start in pane {pane}: its shell is back at the prompt, showing \"{shown}\"; see what the shell printed there"))
}

fn type_claude(name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<Launched> {
    let mut last = String::new();
    for _ in 0..30 {
        match herdr::start_agent(name, "claude", pane, args) {
            Ok(()) => return Ok(Launched::Ready),
            Err(error) => match refusal(&error) {
                Refusal::NotYet => last = error,
                Refusal::Starting => return wait_until_ready(name, pane, trusted),
                Refusal::Final => return Err(format!("herdr would not start Claude in pane {pane}: {error}")),
            },
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err(format!("The shell in pane {pane} never became ready for Claude; herdr said: {last}"))
}

fn last_line(screen: &str) -> &str {
    screen.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or_default()
}

fn asks(line: &str) -> bool {
    let line = line.to_lowercase();
    ["[y/n]", "(y/n)", "[yes/no]", "(yes/no)"].iter().any(|question| line.contains(question))
}

#[derive(Debug, PartialEq)]
enum Prompt {
    Waiting,
    Asking(String),
    Ready,
}

#[derive(Default)]
struct Settle {
    previous: Option<String>,
    quiet: usize,
}

const QUIET_LOOKS: usize = 4;

impl Settle {
    fn look(&mut self, at_shell: bool, screen: &str) -> Prompt {
        let line = last_line(screen);
        let unchanged = self.previous.as_deref() == Some(screen);
        self.previous = Some(screen.to_string());
        if !at_shell || line.is_empty() {
            self.quiet = 0;
            return Prompt::Waiting;
        }
        if asks(line) {
            self.quiet = 0;
            return Prompt::Asking(line.to_string());
        }
        self.quiet = if unchanged { self.quiet + 1 } else { 0 };
        if self.quiet >= QUIET_LOOKS { Prompt::Ready } else { Prompt::Waiting }
    }
}

pub(crate) fn wait_for_prompt(pane: &str) -> Result<()> {
    let look = || Ok((herdr::foreground(pane)?.is_none(), herdr::read_pane(pane)?));
    prompt_with(look, || std::thread::sleep(Duration::from_millis(250)), 120)?
        .map_err(|question| format!("The shell in pane {pane} asks \"{question}\" as it starts; answer it there, then start again"))
}

// A prompt that keeps redrawing never settles, so the wait ends without an error and the check after the start decides.
fn prompt_with(mut look: impl FnMut() -> Result<(bool, String)>, mut pause: impl FnMut(), tries: usize) -> Result<std::result::Result<(), String>> {
    let mut settle = Settle::default();
    let mut asking = None;
    for _ in 0..tries {
        let (at_shell, screen) = look()?;
        match settle.look(at_shell, &screen) {
            Prompt::Ready => return Ok(Ok(())),
            Prompt::Asking(question) => asking = Some(question),
            Prompt::Waiting => asking = None,
        }
        pause();
    }
    Ok(asking.map_or(Ok(()), Err))
}

// Escape clears what is typed or interrupts; Claude Code exits on its second ctrl-c, and a ctrl-c more would reach the shell.
pub fn end_claude(pane: &str) -> Result<()> {
    end_with(|key| herdr::send_keys(pane, &[key]), || Ok(herdr::foreground(pane)?.is_none()), || std::thread::sleep(Duration::from_millis(500)), 10)
        .map_err(|_| format!("Claude in {pane} did not exit after escape and ctrl-c; see what holds it there"))
}

fn end_with(mut send: impl FnMut(&str) -> Result<()>, mut at_shell: impl FnMut() -> Result<bool>, mut pause: impl FnMut(), tries: usize) -> Result<()> {
    send("esc")?;
    pause();
    for _ in 0..tries {
        if at_shell()? {
            return Ok(());
        }
        send("ctrl+c")?;
        pause();
    }
    if at_shell()? { Ok(()) } else { Err("still running".to_string()) }
}

// Claude asks about a folder it has not seen with "No, exit" selected; a prompt sent before it is ready is lost.
fn wait_until_ready(name: &str, pane: &str, trusted: bool) -> Result<Launched> {
    let mut at_shell = 0;
    for _ in 0..120 {
        let screen = herdr::read_pane(pane)?;
        at_shell = if herdr::foreground(pane)?.is_none() { at_shell + 1 } else { 0 };
        if at_shell >= QUIET_LOOKS {
            return Ok(Launched::ShellBack(last_line(&screen).to_string()));
        }
        if screen.contains("trust this folder") {
            if !trusted {
                return Err(format!("Claude in pane {pane} asks whether you trust its folder; you have not trusted the repo in Claude Code, so that answer is yours"));
            }
            herdr::send_keys(pane, &[if screen.contains("❯ Yes, I trust this folder") { "enter" } else { "down" }])?;
        } else if herdr::agent_ready(name).unwrap_or(false) {
            return Ok(Launched::Ready);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!("Claude in pane {pane} never became ready; see what it waits for there"))
}

// Claude Code's own record of the folders the user trusted, counting a folder under a trusted one.
pub fn trusted_by_user(dir: &Path) -> bool {
    let config = std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|_| std::env::var("HOME").map(PathBuf::from)).map(|dir| dir.join(".claude.json"));
    let claude = config.ok().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| serde_json::from_str::<Value>(&text).ok());
    claude.is_some_and(|claude| trusted_in(&claude, dir))
}

fn trusted_in(claude: &Value, dir: &Path) -> bool {
    dir.ancestors().any(|folder| folder.to_str().is_some_and(|folder| claude["projects"][folder]["hasTrustDialogAccepted"] == true))
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

pub fn list(cwd: &Path) -> Result<Vec<Value>> {
    let repo = main_checkout(cwd)?;
    let upstream = default_branch(&repo)?;
    slots(&repo)?.into_iter().map(|slot| {
        let (ahead, behind) = ahead_behind(&slot.path, &upstream)?;
        Ok(json!({
            "slot": name_of(&slot.path),
            "path": slot.path,
            "task": slot.task,
            "free": slot.free,
            "changed": changed_count(&slot.path)?,
            "setup": setup_line(&slot.path),
            "ahead": ahead,
            "behind": behind,
        }))
    }).collect()
}

pub fn land(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    if !is_clean(&slot.path)? {
        return Err(format!("{task} has uncommitted changes in {}; commit or stash them first", slot.path.display()));
    }
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    if ahead_behind(&slot.path, &upstream)?.0 == 0 {
        return Err(format!("{task} has no commits to land"));
    }
    if !rebase_or_hand_over(&slot, &upstream, "kelpie tried to land this task")? {
        return Err(format!("{task} conflicts with {upstream}; its Claude is resolving the rebase. Push again when it reports back."));
    }
    let commits = git(&slot.path, &["log", "--oneline", &format!("{upstream}..HEAD")])?;
    Ok(format!("{task} is rebased onto {upstream} and ready:\n{commits}\n\nPush with:  kelpie push {task}"))
}

// Ok(false) when the rebase conflicted and the task's Claude took it over.
fn rebase_or_hand_over(slot: &Slot, upstream: &str, why: &str) -> Result<bool> {
    if git_writing(&slot.path, &["rebase", "--quiet", upstream]).is_ok() {
        return Ok(true);
    }
    git_writing(&slot.path, &["rebase", "--abort"])?;
    let task = task_of(slot);
    let Some(pane) = claude_of(&slot.path) else {
        return Err(format!("{task} conflicts with {upstream} and has no Claude to resolve it; rebase it in {}", slot.path.display()));
    };
    let text = format!(
        "{why} and rebasing onto {upstream} conflicts (already fetched). Rebase this branch onto {upstream} \
         with git rebase {upstream} (no -i), resolve each conflict keeping the intent of both sides (read the upstream \
         commits that touched the file), run the tests for the files involved, and continue until the rebase is finished. Do not \
         push. Then tell me in a few lines what conflicted and how you resolved it."
    );
    crate::typing::prompt(&pane, &text, task).map_err(|refused| format!("{task} conflicts with {upstream}, and its Claude {}, so kelpie typed nothing; rebase it in {} or clear its box and land again", refused.reason(), slot.path.display()))?;
    Ok(false)
}

fn claude_of(path: &Path) -> Option<String> {
    let workspace = herdr::workspace_for(path).ok()??;
    herdr::agents().ok()?.into_iter()
        .find(|agent| agent["agent"] == "claude" && agent["workspace_id"].as_str() == Some(workspace.as_str()))
        .and_then(|agent| agent["pane_id"].as_str().map(str::to_string))
}

// Agents have no interactive git, so the prompt names the non-interactive ways to reshape history.
pub fn tidy(cwd: &Path, name: Option<&str>, note: &str) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    let pane = claude_of(&slot.path).ok_or_else(|| format!("{task} has no Claude running to tidy its commits"))?;
    let upstream = default_branch(&repo)?;
    let rules = config::resolve(Some(&repo))?.settings.rules_skill.value;
    let mut prompt = format!(
        "Reshape this branch's commits on top of {upstream} for review, following the skill `{rules}`. Interactive git is not \
         available: use git rebase with GIT_SEQUENCE_EDITOR editing the todo, git commit --amend, or git reset --soft and recommit. \
         The final tree must be identical: git diff against the old head must be empty. Do not push. Show the commit list before and after."
    );
    if !note.trim().is_empty() {
        prompt.push_str(&format!(" From the user: {}", note.trim()));
    }
    crate::typing::prompt(&pane, &prompt, task).map_err(|refused| format!("{task}'s Claude {}, so kelpie typed nothing; tidy again once its box is clear", refused.reason()))?;
    Ok(format!("Asked {task}'s Claude to tidy its commits."))
}

// After land: pushes the task's commits through `through` (all of them without it) to the default branch, then closes the task, or with keep_open leaves the rest of its branch on top of what was pushed.
pub fn push(cwd: &Path, name: Option<&str>, through: Option<&str>, keep_open: bool) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot).to_string();
    let upstream = default_branch(&repo)?;
    let (ahead, behind) = ahead_behind(&slot.path, &upstream)?;
    if ahead == 0 || behind > 0 || !is_clean(&slot.path)? {
        return Err(format!("{task} is not landed; run land first"));
    }
    let head = git(&slot.path, &["rev-parse", "HEAD"])?;
    let sha = pushable_through(&slot.path, &upstream, through.unwrap_or(&head), keep_open)?;
    let keep_open = stays_open(keep_open, &sha, &head);
    let address = peers::address(&task, &name_of(&repo));
    let shown = crate::review::shown(&address, &slot.path, &upstream);
    let covered = |reviewed: &str| same_code(&slot.path, &upstream, reviewed, &head) || same_code(&slot.path, &upstream, reviewed, &sha);
    crate::review::gate(shown.as_ref(), covered).map_err(|why| format!("{task} cannot be pushed yet: {why}."))?;
    let was_approved = approved(&slot.path, &upstream, &address);
    let check = Pusher { caller: crate::permit::caller(), permit: crate::permit::of(&address, &name_of(&repo)), reviewed: shown.as_ref().is_some_and(|shown| !shown.pending()), approved: was_approved };
    let pusher = check.allowed().map_err(|why| format!("{task} cannot be pushed by {}: {why}.", crate::permit::describe(&check.caller)))?;
    let state = state_dir()?;
    let pushed = git(&slot.path, &["log", "--reverse", "--format=%h %s", &format!("{upstream}..{sha}")])?;
    let note = started_file(&task, &repo).ok().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| crate::task_note::items(&text).into_iter().next());
    let count = push_prefix(&slot.path, &upstream, &sha, &state).map_err(|error| format!("Pushing {task} failed: {error}"))?;
    let vault = crate::vault_cli::detect();
    let log = vault.as_ref().map(|cli| crate::task_note::log_push(cli, &slot.path, &task, &pushed));
    let logged = log.as_ref().map(crate::task_note::logged_text).unwrap_or_default();
    let target = upstream.trim_start_matches("origin/").to_string();
    let landed = |closed: bool, waits: bool| Landed { pusher, address: &address, target: &target, commits: &pushed, closed, note: note.as_deref(), logged: log.as_ref().map(|log| matches!(log, Ok(Some(_)))), waits };
    let short = &sha[..7.min(sha.len())];
    let told_ci = crate::ci::Pushed { pusher: pusher.to_string(), target: target.clone(), commits: pushed.clone(), note: note.clone(), logged: log.as_ref().map(|log| matches!(log, Ok(Some(_)))) };
    let watch = crate::ci::begin(&state, &repo, &address, &sha, !keep_open, told_ci);
    let ci = match &watch {
        crate::ci::Begin::Watching => format!(" kelpie watches CI for {short}."),
        crate::ci::Begin::NotWatched(reason) => format!(" CI not watched: {reason}."),
        crate::ci::Begin::Off => String::new(),
    };
    if !keep_open {
        if matches!(watch, crate::ci::Begin::Watching) {
            if let Err(error) = messages::notify_lead(&format!("{} pushed {address}: {} to {target}. kelpie watches CI for {short} and closes the task once every run is green; you hear what landed then.", capitalized(pusher), plural(count, "commit"))) {
                eprintln!("Could not tell the lead about the push: {error}");
            }
            return Ok(format!("Pushed {} to {upstream}.{logged} {task} stays open under CI running until CI for {short} is green; kelpie closes it then. {}", plural(count, "commit"), fast_forward_main(&repo, &upstream)?));
        }
        let ready = note_ready(vault.as_ref(), &state, &address, &task);
        if let Err(waits) = &ready {
            tell_lead_pushed(&landed(false, true));
            return Ok(format!("Pushed {} to {upstream}.{logged}{ci} {waits}", plural(count, "commit")));
        }
        let closed = close_task(cwd, name, vault.as_ref(), ready.unwrap_or_default());
        tell_lead_pushed(&landed(closed.is_ok(), false));
        return closed.map(|closed| format!("Pushed {} to {upstream}.{logged}{ci} {closed}", plural(count, "commit")));
    }
    tell_lead_pushed(&landed(false, false));
    let (clean, rest) = match keep_going(&state, &repo, &slot, &upstream, &address, was_approved)? {
        Rest::Rebased { clean, rest } => (clean, rest),
        Rest::NotFetched(error) => return Ok(format!("Pushed {} through {short} to {upstream}.{logged} Fetching afterwards failed ({error}), so the rest of {task} is not rebased onto it yet: run kelpie land {task}.", plural(count, "commit"))),
    };
    let told = format!(
        "{} pushed {} of this task, through {short}, to {target}; they are on {target} for good. Your branch continues from there with {} on top: reshape only those.",
        capitalized(pusher), plural(count, "commit"), plural(rest, "commit")
    );
    if let Err(error) = messages::notify(&address, &told) {
        eprintln!("Could not tell {address} about the push: {error}");
    }
    let after = if clean { format!("{task} stays open with {} on top.", plural(rest, "commit")) } else { format!("{task} stays open; its Claude is resolving the rebase of the rest.") };
    Ok(format!("Pushed {} through {short} to {upstream}.{logged}{ci} {after} {}", plural(count, "commit"), fast_forward_main(&repo, &upstream)?))
}

pub fn after_ci(repo: &Path, task: &str, address: &str, told: &crate::ci::Pushed) -> Result<String> {
    let state = state_dir()?;
    let vault = crate::vault_cli::detect();
    let landed = |closed: bool, waits: bool| Landed { pusher: &told.pusher, address, target: &told.target, commits: &told.commits, closed, note: told.note.as_deref(), logged: told.logged, waits };
    let ready = note_ready(vault.as_ref(), &state, address, task);
    if let Err(waits) = &ready {
        tell_lead_pushed(&landed(false, true));
        return Ok(waits.clone());
    }
    let closed = close_task(repo, Some(task), vault.as_ref(), ready.unwrap_or_default());
    tell_lead_pushed(&landed(closed.is_ok(), false));
    closed
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map(|first| first.to_uppercase().chain(characters).collect()).unwrap_or_default()
}

// Who may push this task now: the user always, the lead only with the user's permission and its own rules check, a task or any other Claude never.
struct Pusher {
    caller: crate::permit::Caller,
    permit: Option<crate::permit::Permit>,
    reviewed: bool,
    approved: bool,
}

impl Pusher {
    fn allowed(&self) -> std::result::Result<&'static str, String> {
        use crate::permit::Caller;
        match &self.caller {
            Caller::User => Ok("the user"),
            Caller::Lead => {
                crate::permit::lead_may_push(self.permit, self.reviewed)?;
                if !self.approved {
                    return Err("its commits have no rules approval of yours that matches them: check them and run kelpie approve first".to_string());
                }
                Ok("the lead")
            }
            Caller::Task(_) | Caller::Agent => Err("no agent pushes; the user pushes from the board, or the lead for a task the user gave push permission".to_string()),
        }
    }
}

struct Landed<'a> {
    pusher: &'a str,
    address: &'a str,
    target: &'a str,
    commits: &'a str,
    closed: bool,
    note: Option<&'a str>,
    logged: Option<bool>,
    waits: bool,
}

// logged is None without the vault, else whether kelpie wrote the repo's log line itself; with the vault kelpie settles the note.
fn pushed_notice(landed: &Landed) -> String {
    let Landed { pusher, address, target, commits, closed, note, logged, waits } = *landed;
    let count = commits.lines().count();
    let mut text = format!("{} pushed {address}: {} to {target} ({}).", capitalized(pusher), plural(count, "commit"), commits.lines().collect::<Vec<_>>().join("; "));
    text.push_str(match (closed, waits) {
        (true, _) => " The task is done and closed.",
        (false, true) => " The task stays open until its note has its solution and decisions; kelpie asked it to write them.",
        (false, false) => " The task stays open with the rest of its branch.",
    });
    match logged {
        Some(true) => return text + " kelpie wrote the repo's log line in the vault from these commits; update the area note with what changed.",
        Some(false) => return text + " kelpie could not write the repo's log line: write it in the vault with what landed and these shas, and update the area note with what changed.",
        None => {}
    }
    text.push_str(" Write the repo's log line in the vault with what landed and these shas");
    match (note, closed) {
        (Some(note), true) => text.push_str(&format!(", and close or update its note {note}.")),
        (Some(note), false) => text.push_str(&format!(", and note in {note} what landed.")),
        (None, true) => text.push_str(", and close or update the task's note if it has one."),
        (None, false) => text.push('.'),
    }
    text
}

fn tell_lead_pushed(landed: &Landed) {
    if let Err(error) = messages::tell_lead(&pushed_notice(landed), Some("push")) {
        eprintln!("Could not tell the lead about the push: {error}");
    }
}

#[derive(Debug, PartialEq)]
enum Rest {
    Rebased { clean: bool, rest: usize },
    NotFetched(String),
}

// After a push that keeps the task open: the rest of its branch goes on top of the new default branch, an approval of the whole branch holds for the rest, and the card, written for what was pushed, goes.
fn keep_going(state: &Path, repo: &Path, slot: &Slot, upstream: &str, address: &str, was_approved: bool) -> Result<Rest> {
    crate::report::forget_card_in(state, address);
    if let Err(error) = fetch(repo) {
        return Ok(Rest::NotFetched(error));
    }
    let clean = rebase_or_hand_over(slot, upstream, "kelpie pushed this task's first commits")?;
    let rest = ahead_behind(&slot.path, upstream)?.0;
    if clean && was_approved && rest > 0 {
        record_approval(state, address, &fingerprint(&slot.path, upstream)?)?;
    }
    Ok(Rest::Rebased { clean, rest })
}

// A push through the last commit leaves nothing to keep the task open for, so it closes like a full push.
fn stays_open(keep_open: bool, sha: &str, head: &str) -> bool {
    keep_open && sha != head
}

// One of the task's commits not yet on upstream; a full push names HEAD, so a commit that arrived after the dialog opened is never pushed unseen.
fn pushable_through(path: &Path, upstream: &str, rev: &str, keep_open: bool) -> Result<String> {
    let sha = git(path, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).map_err(|_| format!("{rev} is not a commit"))?;
    let on_branch = crate::sh::succeeds("git", &["merge-base", "--is-ancestor", &sha, "HEAD"], Some(path));
    let new = git(path, &["rev-list", "--count", &format!("{upstream}..{sha}")])?.parse::<usize>().unwrap_or(0) > 0;
    if !on_branch || !new {
        return Err(format!("{} is not one of the task's commits to push", &sha[..7.min(sha.len())]));
    }
    if !keep_open && sha != git(path, &["rev-parse", "HEAD"])? {
        return Err(format!("The branch has commits after {} that were not shown; open push again to see them", &sha[..7.min(sha.len())]));
    }
    Ok(sha)
}

// Fast-forwards the default branch on origin to sha; the remote-tracking branch follows.
fn push_prefix(path: &Path, upstream: &str, sha: &str, state: &Path) -> Result<usize> {
    let count = git(path, &["rev-list", "--count", &format!("{upstream}..{sha}")])?.parse::<usize>().map_err(|error| error.to_string())?;
    let target = format!("{sha}:refs/heads/{}", upstream.trim_start_matches("origin/"));
    crate::repo::push_from(path, &[&target], state)?;
    Ok(count)
}

// The change rev makes where it left upstream, without context or line numbers, so a clean rebase, a reworded message or a squash keeps it.
fn code(path: &Path, upstream: &str, rev: &str) -> Result<String> {
    let base = git(path, &["merge-base", rev, upstream])?;
    let diff = git(path, &["diff", "-U0", "--no-color", "--no-ext-diff", &base, rev])?;
    let lines: Vec<&str> = diff.lines().filter(|line| !(line.starts_with("index ") || line.starts_with("@@"))).collect();
    let mut hasher = Fnv::default();
    hasher.write(lines.join("\n").as_bytes());
    Ok(format!("{:016x}", hasher.0))
}

pub fn same_code(path: &Path, upstream: &str, one: &str, other: &str) -> bool {
    one == other || matches!((code(path, upstream, one), code(path, upstream, other)), (Ok(one), Ok(other)) if one == other)
}

// Every message and every added or removed line, without line numbers or context, so a rebase that applies cleanly keeps the fingerprint.
fn fingerprint(path: &Path, upstream: &str) -> Result<String> {
    let range = format!("{upstream}..HEAD");
    let messages = git(path, &["log", "--reverse", "--format=%B%x00", &range])?;
    let changes = git(path, &["log", "--reverse", "-p", "-U0", "--no-color", "--no-ext-diff", "--format=%x00", &range])?;
    let lines: Vec<&str> = changes.lines().filter(|line| !(line.starts_with("index ") || line.starts_with("@@"))).collect();
    let mut hasher = Fnv::default();
    hasher.write(messages.as_bytes());
    hasher.write(&[0xff]);
    hasher.write(lines.join("\n").as_bytes());
    Ok(format!("{:016x}", hasher.0))
}

// FNV-1a: stable across Rust versions, unlike DefaultHasher, so an approval survives a kelpie upgrade.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf29ce484222325)
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    }
}

fn approval_file(state: &Path, address: &str) -> PathBuf {
    state.join("approved").join(address)
}

fn record_approval(state: &Path, address: &str, fingerprint: &str) -> Result<()> {
    crate::state::write(&approval_file(state, address), fingerprint)
}

fn approval_holds(state: &Path, address: &str, fingerprint: &str) -> bool {
    std::fs::read_to_string(approval_file(state, address)).is_ok_and(|stored| stored.trim() == fingerprint)
}

fn forget_approval_in(state: &Path, address: &str) {
    let _ = std::fs::remove_file(approval_file(state, address));
}

fn forget_approval(task: &str, repo: &Path) {
    if let Ok(state) = state_dir() {
        forget_approval_in(&state, &peers::address(task, &name_of(repo)));
    }
}

// The lead's word that the task's commits hold to the rules skill, until a commit changes.
pub fn approve(cwd: &Path, name: Option<&str>) -> Result<String> {
    approve_as(&crate::permit::caller(), cwd, name)
}

// Read from the Claude above, not the folder, which kelpie -C or a cd sets to anything.
fn approve_as(caller: &crate::permit::Caller, cwd: &Path, name: Option<&str>) -> Result<String> {
    use crate::permit::Caller;
    if !matches!(caller, Caller::Lead | Caller::User) {
        return Err(format!("approve is the lead's check of a task's commits; {} cannot approve them", crate::permit::describe(caller)));
    }
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    let upstream = default_branch(&repo)?;
    if ahead_behind(&slot.path, &upstream)?.0 == 0 {
        return Err(format!("{task} has no commits to approve"));
    }
    let address = peers::address(task, &name_of(&repo));
    record_approval(&state_dir()?, &address, &fingerprint(&slot.path, &upstream)?)?;
    Ok(format!("Approved {address}'s commits against the rules; a new or changed commit takes the approval back."))
}

// What the lead is asked when a task reports ready, or None when there is nothing new for it to check.
pub fn rules_check(cwd: &Path) -> Result<Option<String>> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, None, cwd)?;
    let task = task_of(&slot);
    let upstream = default_branch(&repo)?;
    let address = peers::address(task, &name_of(&repo));
    if ahead_behind(&slot.path, &upstream)?.0 == 0 || approved(&slot.path, &upstream, &address) {
        return Ok(None);
    }
    let rules = config::resolve(Some(&repo))?.settings.rules_skill.value;
    let sha = git(&slot.path, &["rev-parse", "--short", "HEAD"])?;
    Ok(Some(format!(
        "{address} is ready at {sha}: check its commits against `{rules}` (git -C {} log -p {upstream}..HEAD). When they hold, run \
         kelpie -C {} approve {task}; when one breaks a rule, kelpie tell {address} which commit and what to fix.",
        slot.path.display(), repo.display()
    )))
}

pub fn approved(path: &Path, upstream: &str, address: &str) -> bool {
    let Ok(state) = state_dir() else { return false };
    approval_file(&state, address).exists() && fingerprint(path, upstream).is_ok_and(|current| approval_holds(&state, address, &current))
}

pub struct Commit {
    pub sha: String,
    pub message: String,
}

// Oldest first, as they will land.
pub fn commits(path: &Path) -> Result<Vec<Commit>> {
    let upstream = default_branch(&main_checkout(path)?)?;
    let log = git(path, &["log", "--reverse", "--format=%H%x00%B%x1e", &format!("{upstream}..HEAD")])?;
    Ok(log.split('\x1e').filter_map(|entry| {
        let (sha, message) = entry.trim_start().split_once('\0')?;
        Some(Commit { sha: sha.to_string(), message: message.trim().to_string() })
    }).collect())
}

// A commit with the given tree and parent, written by the author of `like`.
fn commit_like(path: &Path, tree: &str, parent: &str, like: &str, message: &str) -> Result<String> {
    let author = git(path, &["show", "-s", "--format=%an%x00%ae%x00%aI", like])?;
    let mut fields = author.split('\0');
    let (name, email, date) = (fields.next().unwrap_or_default(), fields.next().unwrap_or_default(), fields.next().unwrap_or_default());
    let output = std::process::Command::new("git")
        .args(["commit-tree", tree, "-p", parent, "-m", message])
        .env("GIT_AUTHOR_NAME", name).env("GIT_AUTHOR_EMAIL", email).env("GIT_AUTHOR_DATE", date)
        .current_dir(path).output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("git commit-tree failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// Same tree, parent and author with a new message, then the later commits replayed on top; their trees are unchanged, so nothing can conflict.
pub fn reword(path: &Path, sha: &str, message: &str) -> Result<()> {
    let new = commit_like(path, &format!("{sha}^{{tree}}"), &format!("{sha}^"), sha, message)?;
    git_writing(path, &["rebase", "--quiet", "--onto", &new, sha]).map(|_| ())
}

// One commit with the branch's final tree on the default branch, written by the first commit's author.
pub fn squash(path: &Path, message: &str) -> Result<()> {
    let upstream = default_branch(&main_checkout(path)?)?;
    let base = git(path, &["merge-base", "HEAD", &upstream])?;
    let first = git(path, &["rev-list", "--reverse", &format!("{base}..HEAD")])?.lines().next().map(str::to_string)
        .ok_or("Nothing to squash")?;
    let new = commit_like(path, "HEAD^{tree}", &base, &first, message)?;
    git_writing(path, &["reset", "--quiet", "--soft", &new]).map(|_| ())
}

// A one-off Claude with no tools, settings, hooks or MCP, run outside the repo, so it only writes text.
pub fn draft_squash_message(path: &Path) -> Result<String> {
    let upstream = default_branch(&main_checkout(path)?)?;
    let range = format!("{upstream}..HEAD");
    let input = format!(
        "Commit messages, oldest first:\n\n{}\n\nDiff stat:\n{}\n\nDiff:\n{}",
        git(path, &["log", "--reverse", "--format=%B%n---", &range])?,
        git(path, &["diff", "--stat", &range])?,
        git(path, &["diff", &range])?.chars().take(60_000).collect::<String>(),
    );
    let mut child = std::process::Command::new("claude")
        .args(["-p", "--setting-sources", "", "--strict-mcp-config", "--tools", "", "--model", "sonnet", "--no-session-persistence", "--disable-slash-commands",
            "Write one git commit message for these commits squashed into one, from what was actually changed, in the style the \
             messages use. Keep the references and trailers the messages carry, and add none. Plain text only: no preamble, no code fences."])
        .current_dir(std::env::temp_dir())
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .spawn().map_err(|error| format!("claude: {error}"))?;
    use std::io::Write;
    child.stdin.take().ok_or("claude has no stdin")?.write_all(input.as_bytes()).map_err(|error| error.to_string())?;
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    let message = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || message.is_empty() {
        return Err(format!("Claude could not draft a message: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(message)
}

// -D because callers have already checked the commits are on upstream; git's -d checks the main checkout's local branch, which is still behind right after a push.
fn release(repo: &Path, slot: &Slot, upstream: &str) -> Result<()> {
    git_writing(&slot.path, &["switch", "--quiet", "--detach", upstream])?;
    clear_tmp(&slot.path);
    git(repo, &["branch", "-D", task_of(slot)]).map(|_| ())
}

// Last step of done and drop: closing the Space ends the task's Claude, which may be the one running this.
fn close_space(workspace: Option<String>) -> String {
    match workspace.map(|workspace| herdr::close_workspace(&workspace)) {
        Some(Err(error)) => format!(" Its Space could not be closed: {error}"),
        _ => String::new(),
    }
}

fn fast_forward_main(repo: &Path, upstream: &str) -> Result<String> {
    let local = upstream.trim_start_matches("origin/");
    let name = name_of(repo);
    if git(repo, &["branch", "--show-current"])? != local {
        return Ok(format!("{name} left as is (not on {local})."));
    }
    Ok(match git_writing(repo, &["merge", "--quiet", "--ff-only", upstream]) {
        Ok(_) => format!("{name} fast-forwarded to {upstream}."),
        Err(_) => format!("{name} could not fast-forward to {upstream}."),
    })
}

pub fn done(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let task = task_of(&find_task(&repo, name, cwd)?).to_string();
    let state = state_dir()?;
    let address = peers::address(&task, &name_of(&repo));
    let vault = crate::task_note::linked(&state, &address).and_then(|_| crate::vault_cli::detect());
    let said = note_ready(vault.as_ref(), &state, &address, &task)?;
    close_task(cwd, name, vault.as_ref(), said)
}

// Done waits for the task's note to have its solution and decisions: Err is the refusal, the task told what to write; a vault that does not answer lets done go on, and Ok says so.
fn note_ready(vault: Option<&crate::vault_cli::Cli>, state: &Path, address: &str, task: &str) -> Result<String> {
    let (Some(cli), Some(note)) = (vault, crate::task_note::linked(state, address)) else { return Ok(String::new()) };
    match crate::task_note::waits(cli, &note, task) {
        Ok(None) => Ok(String::new()),
        Ok(Some((user, told))) => {
            if let Err(error) = messages::notify(address, &told) {
                eprintln!("Could not tell {address} what its note lacks: {error}");
            }
            Err(user)
        }
        Err(error) => Ok(format!(" Its note {note} could not be checked ({error}), so done went on without it.")),
    }
}

fn close_task(cwd: &Path, name: Option<&str>, vault: Option<&crate::vault_cli::Cli>, said: String) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    let workspace = herdr::workspace_for(&slot.path)?;
    let finished = finish(&repo, &slot, &upstream, &crate::lead::state_dir()?, vault)? + &said;
    forget_session(task_of(&slot), &repo);
    forget_approval(task_of(&slot), &repo);
    crate::permit::forget(&peers::address(task_of(&slot), &name_of(&repo)));
    crate::review::forget(task_of(&slot), &repo);
    crate::report::forget_card(task_of(&slot), &repo);
    crate::evidence::forget(task_of(&slot), &repo);
    crate::ci::forget(&crate::lead::state_dir()?, &peers::address(task_of(&slot), &name_of(&repo)));
    crate::recover::forget(&crate::lead::state_dir()?, &peers::address(task_of(&slot), &name_of(&repo)));
    end_life(task_of(&slot), &repo, "done");
    Ok(finished + &close_space(workspace))
}

fn finish(repo: &Path, slot: &Slot, upstream: &str, state: &Path, vault: Option<&crate::vault_cli::Cli>) -> Result<String> {
    let task = task_of(slot);
    if !is_clean(&slot.path)? {
        return Err(format!("{task} has uncommitted changes; land or drop them first"));
    }
    let missing = missing_from(&slot.path, upstream)?;
    if !missing.is_empty() {
        return Err(format!("{task} has changes that are not on {upstream} yet; push them first, or use drop:\n{}", missing.join("\n")));
    }
    release(repo, slot, upstream)?;
    let address = peers::address(task, &name_of(repo));
    crate::task_note::forget(state, &address);
    let mut said = String::new();
    for item in forget_start_in(state, task, repo).map(|text| crate::task_note::items(&text)).unwrap_or_default() {
        let notice = match vault.filter(|_| crate::vault_cli::is_note(&item)) {
            Some(_) if crate::task_note::held_elsewhere(state, task, &address, &item) => {
                said.push_str(&format!(" Its note {item} stays open for the task's other halves."));
                continue;
            }
            Some(cli) => match crate::task_note::close(cli, &item) {
                Ok(path) => {
                    said.push_str(&format!(" Its note is closed ({path})."));
                    format!("{task} is done and its commits are on the default branch; kelpie closed its note {item}. Update the area note when the task changed what it describes.")
                }
                Err(error) => {
                    said.push_str(&format!(" Its note {item} could not be closed ({error})."));
                    format!("{task} is done and its commits are on the default branch, but kelpie could not close its note {item} ({error}): close it with the vault tools.")
                }
            },
            None => crate::backlog::done_notice(task, &item),
        };
        if let Err(error) = messages::notify_lead(&notice) {
            eprintln!("Could not tell the lead: {error}");
        }
    }
    Ok(format!("{task} is done; {} is free again.{said} {}", name_of(&slot.path), fast_forward_main(repo, upstream)?))
}

// The branch's commits whose change upstream lacks, as "sha subject"; empty when every change landed, however it was cherry-picked or squashed.
fn missing_from(path: &Path, upstream: &str) -> Result<Vec<String>> {
    let unmatched: Vec<String> = git(path, &["cherry", "-v", "--abbrev", upstream, "HEAD"])?.lines()
        .filter_map(|line| line.strip_prefix("+ ")).map(str::to_string).collect();
    if unmatched.is_empty() {
        return Ok(unmatched);
    }
    let merged = git(path, &["merge-tree", "--write-tree", upstream, "HEAD"]);
    if merged.is_ok_and(|tree| tree.lines().next() == git(path, &["rev-parse", &format!("{upstream}^{{tree}}")]).ok().as_deref()) {
        return Ok(Vec::new());
    }
    Ok(unmatched)
}

pub fn drop(cwd: &Path, name: Option<&str>, force: bool) -> Result<String> {
    discard(cwd, name, force, "dropped")
}

fn discard(cwd: &Path, name: Option<&str>, force: bool, how: &str) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    let upstream = default_branch(&repo)?;
    let changed = changed_count(&slot.path)?;
    let (ahead, _) = ahead_behind(&slot.path, &upstream)?;
    if !force && (changed > 0 || ahead > 0) {
        return Err(format!(
            "{task} has {changed} uncommitted change(s) and {ahead} commit(s) not on {upstream}; drop --force discards them"
        ));
    }
    let workspace = herdr::workspace_for(&slot.path)?;
    if force {
        git_writing(&slot.path, &["reset", "--quiet", "--hard"])?;
        git_writing(&slot.path, &["clean", "-fdq"])?;
    }
    release(&repo, &slot, &upstream)?;
    let notes: Vec<String> = forget_start(task, &repo).map(|text| crate::task_note::items(&text)).unwrap_or_default().into_iter().filter(|item| crate::vault_cli::is_note(item)).collect();
    let mut said = String::new();
    if let Ok(state) = state_dir() {
        let address = peers::address(task, &name_of(&repo));
        crate::task_note::forget(&state, &address);
        said = dropped_notes(if notes.is_empty() { None } else { crate::vault_cli::detect() }.as_ref(), &state, task, &address, &notes);
    }
    forget_session(task, &repo);
    forget_approval(task, &repo);
    crate::permit::forget(&peers::address(task, &name_of(&repo)));
    crate::review::forget(task, &repo);
    crate::report::forget_card(task, &repo);
    crate::evidence::forget(task, &repo);
    if let Ok(state) = state_dir() {
        crate::ci::forget(&state, &peers::address(task, &name_of(&repo)));
        crate::recover::forget(&state, &peers::address(task, &name_of(&repo)));
    }
    end_life(task, &repo, how);
    Ok(format!("{task} dropped; {} is free again.{said}{}", name_of(&slot.path), close_space(workspace)))
}

// A dropped task's note goes back to the backlog as a reset's does, so no active note is left without a task; without the vault nothing is asked.
fn dropped_notes(vault: Option<&crate::vault_cli::Cli>, state: &Path, task: &str, address: &str, notes: &[String]) -> String {
    let Some(cli) = vault else { return String::new() };
    notes.iter().map(|note| match note_to_backlog(cli, state, task, address, note) {
        NoteBack::Shared => format!(" Its note {note} stays open for the task's other halves."),
        NoteBack::Back(path) => format!(" kelpie put its note {path} back in the backlog."),
        NoteBack::Failed(error) => format!(" Its note {note} could not be put back in the backlog ({error}); set its status to backlog in the vault."),
    }).collect()
}

fn end_life(task: &str, repo: &Path, how: &str) {
    if let Err(error) = messages::log_end(task, &name_of(repo), how) {
        eprintln!("Could not log the end of {task}: {error}");
    }
}

fn stash_index(repo: &Path, message: &str) -> Result<Option<usize>> {
    Ok(git(repo, &["stash", "list", "--format=%gs"])?.lines().position(|line| line.ends_with(message)))
}

fn claim_slot(repo: &Path, upstream: &str) -> Result<Slot> {
    match slots(repo)?.into_iter().find(|slot| slot.free) {
        Some(slot) => Ok(slot),
        None => create_slot(repo, upstream),
    }
}

pub fn move_work(cwd: &Path, name: &str, files: &[String], pane: Option<&str>, focus: bool) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let task = slug(name)?;
    let config::Settings { setup_skill, setup_command, .. } = config::resolve(Some(&repo))?.settings;
    let prepare = config::setup_command_in(setup_command.value, &repo).map_or(String::new(), |command| format!(", which prepares it with {command}"));
    if slots(&repo)?.iter().any(|slot| slot.task.as_deref() == Some(task.as_str())) {
        return Err(format!("{task} already has a worktree; use kelpie start {task} to go there"));
    }
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    let local = upstream.trim_start_matches("origin/");
    let current = git(&repo, &["branch", "--show-current"])?;
    let existing = branch_exists(&repo, &task);
    if !existing && current != local {
        return Err(format!("The main checkout is on {current}, not {local}; to move that work, use its branch name: kelpie move {current}"));
    }

    let message = format!("kelpie move {task}");
    if !files.is_empty() {
        let mut args = vec!["stash", "push", "--include-untracked", "--message", message.as_str(), "--"];
        args.extend(files.iter().map(String::as_str));
        git_writing(&repo, &args)?;
    }
    let stashed = |error: String| -> String {
        if files.is_empty() { error } else { format!("{error}\nThe moved files are kept in the stash \"{message}\" (git stash list).") }
    };

    if existing && current == task {
        git_writing(&repo, &["switch", "--quiet", local]).map_err(stashed)?;
    } else if !existing {
        git(&repo, &["branch", &task]).map_err(stashed)?;
        if ahead_behind(&repo, &upstream)?.0 > 0 {
            git_writing(&repo, &["reset", "--quiet", "--keep", &upstream]).map_err(stashed)?;
        }
    }

    let slot = claim_slot(&repo, &upstream).map_err(stashed)?;
    crate::push_block::warn_if_off(crate::push_block::install(&slot.path, &state_dir()?).map_err(stashed)?);
    git_writing(&slot.path, &["switch", "--quiet", &task]).map_err(stashed)?;
    if let Some(index) = stash_index(&repo, &message)? {
        git_writing(&slot.path, &["stash", "pop", "--quiet", &format!("stash@{{{index}}}")]).map_err(stashed)?;
    }

    let space = open_space(&repo, &slot, &task, focus)?;
    if let Some(pane) = pane {
        herdr::move_pane_to_new_tab(pane, &space.workspace_id, "claude", focus)?;
    }
    let main = fast_forward_main(&repo, &upstream)?;
    Ok(format!("Moved {task} to {}. {main}\nSet it up: run the skill {} in {}{prepare}.", slot.path.display(), setup_skill.value, slot.path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_rebase_keeps_the_fingerprint_and_a_new_or_changed_commit_loses_it() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let approved = fingerprint(&slot.path, &upstream).unwrap();
        land_on_main(&dir, &upstream, |_| ());
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "rebase", "--quiet", &upstream]).unwrap();
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0));
        assert_eq!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        let amend = |args: &[&str]| git(&slot.path, &[&["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--amend"], args].concat()).unwrap();
        amend(&["-m", "fourth, reworded"]);
        assert_ne!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        amend(&["-m", "fourth"]);
        assert_eq!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        std::fs::write(slot.path.join("d"), "changed").unwrap();
        git(&slot.path, &["add", "d"]).unwrap();
        amend(&["--no-edit"]);
        assert_ne!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        std::fs::write(slot.path.join("d"), "d").unwrap();
        git(&slot.path, &["add", "d"]).unwrap();
        amend(&["--no-edit"]);
        assert_eq!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        commit(&slot.path, "e", "fifth");
        assert_ne!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn an_approval_holds_only_for_the_fingerprint_it_recorded_and_is_gone_once_forgotten() {
        let state = std::env::temp_dir().join(format!("kelpie-approvals-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        assert!(!approval_holds(&state, "billing@app", "abc"));
        record_approval(&state, "billing@app", "abc").unwrap();
        assert!(approval_holds(&state, "billing@app", "abc"));
        assert!(!approval_holds(&state, "billing@app", "abd"));
        assert!(!approval_holds(&state, "billing@web", "abc"));
        forget_approval_in(&state, "billing@app");
        assert!(!approval_holds(&state, "billing@app", "abc"));
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_link_records_the_item_and_names_the_one_it_replaces() {
        let file = std::env::temp_dir().join(format!("kelpie-link-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        assert_eq!(record_start(&file, "Projects/grown.md").unwrap(), None);
        assert_eq!(record_start(&file, "local:Fix login").unwrap().as_deref(), Some("Projects/grown.md"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "local:Fix login");
        std::fs::remove_file(file).unwrap();
        let source = include_str!("tasks.rs");
        let finish = &source[source.find("\nfn finish(").unwrap()..];
        assert!(finish[..finish.find("\n}\n").unwrap()].contains("done_notice("), "a finished task tells the lead which item it came from");
    }

    #[test]
    fn done_drop_and_reset_forget_the_approval() {
        let source = include_str!("tasks.rs");
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("done").contains("close_task(") && body("close_task").contains("forget_approval("));
        assert!(body("discard").contains("forget_approval("));
        assert!(body("reset").contains("discard("));
        assert!(body("close_task").contains("review::forget(") && body("discard").contains("review::forget("), "a later task by the same name must not show this one's review");
        assert!(body("close_task").contains("report::forget_card(") && body("discard").contains("report::forget_card("), "a later task by the same name must not show this one's card");
        assert!(body("close_task").contains("evidence::forget(") && body("discard").contains("evidence::forget("), "a later task by the same name must not show this one's test runs");
        assert!(body("close_task").contains("permit::forget(") && body("discard").contains("permit::forget("), "done, drop and reset take the push permission back");
        assert!(body("finish").contains("task_note::forget(") && body("discard").contains("task_note::forget("), "a later task by the same name must not inherit this one's Problem pass");
        assert!(source.contains("\npub fn drop(cwd: &Path, name: Option<&str>, force: bool) -> Result<String> {\n    discard("));
    }

    #[test]
    fn only_the_user_pushes_freely_the_lead_with_permission_and_its_approval_and_no_agent_at_all() {
        use crate::permit::{Caller, Permit};
        let check = |caller: Caller, permit: Option<Permit>, reviewed: bool, approved: bool| Pusher { caller, permit, reviewed, approved }.allowed();
        assert_eq!(check(Caller::User, None, false, false), Ok("the user"));
        assert_eq!(check(Caller::Lead, Some(Permit::Task), false, true), Ok("the lead"));
        assert!(check(Caller::Lead, None, true, true).unwrap_err().contains("no push permission"));
        assert!(check(Caller::Lead, Some(Permit::Task), true, false).unwrap_err().contains("kelpie approve"));
        assert!(check(Caller::Lead, Some(Permit::Repo), false, true).unwrap_err().contains("reviewer passed"));
        assert_eq!(check(Caller::Lead, Some(Permit::Repo), true, true), Ok("the lead"));
        for agent in [Caller::Task("billing@app".to_string()), Caller::Agent] {
            assert!(check(agent, Some(Permit::Task), true, true).unwrap_err().contains("no agent pushes"), "a permission is the lead's, never a task's");
        }
    }

    #[test]
    fn only_the_lead_or_the_user_approves_whatever_folder_it_runs_from() {
        use crate::permit::Caller;
        for caller in [Caller::Task("billing@app".to_string()), Caller::Agent] {
            let refused = approve_as(&caller, Path::new("/nowhere"), Some("billing")).unwrap_err();
            assert!(refused.contains("lead's check") && refused.contains(&crate::permit::describe(&caller)), "a task's kelpie -C <worktree> approve is refused too: {refused}");
        }
        for caller in [Caller::Lead, Caller::User] {
            assert!(!approve_as(&caller, Path::new("/nowhere"), Some("billing")).unwrap_err().contains("lead's check"));
        }
    }

    #[test]
    fn after_a_push_the_lead_hears_what_landed_and_is_cued_to_log_it_and_settle_the_note() {
        let commits = "a1b2c3d Add refunds\ne4f5a6b Show refunds on the invoice";
        let done = pushed_notice(&Landed { pusher: "the lead", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing-refunds.md"), logged: None, waits: false });
        assert_eq!(done, "The lead pushed billing@app: 2 commits to main (a1b2c3d Add refunds; e4f5a6b Show refunds on the invoice). The task is done and closed. Write the repo's log line in the vault with what landed and these shas, and close or update its note Projects/billing-refunds.md.");
        let open = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits: "a1b2c3d Add refunds", closed: false, note: None, logged: None, waits: false });
        assert!(open.starts_with("The user pushed billing@app: 1 commit to main (a1b2c3d Add refunds). The task stays open"), "{open}");
        assert!(open.ends_with("these shas."), "{open}");
    }

    #[test]
    fn with_the_vault_on_the_lead_hears_kelpie_logged_the_push_and_is_not_asked_to_log_it() {
        let commits = "a1b2c3d Add refunds";
        let kept = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing.md"), logged: Some(true), waits: false });
        assert!(kept.contains("kelpie wrote the repo's log line") && kept.contains("update the area note") && !kept.contains("Write the repo's log line"), "{kept}");
        let waits = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: false, note: Some("Projects/billing.md"), logged: Some(true), waits: true });
        assert!(waits.contains("stays open until its note has its solution and decisions"), "{waits}");
        let failed = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing.md"), logged: Some(false), waits: false });
        assert!(failed.contains("kelpie could not write the repo's log line: write it in the vault with what landed and these shas") && !failed.contains("kelpie wrote"), "a log append that failed leaves the line to the lead: {failed}");
    }

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
    fn link_says_who_settles_the_note() {
        let off = link_text("billing", "Projects/billing.md", Some("local:Fix login"), false);
        assert_eq!(off, "Linked billing to Projects/billing.md (it was linked to local:Fix login): when it is done the lead is told to log and close it, and a reset puts it back in the backlog.", "without the vault the text is today's");
        let on = link_text("billing", "Projects/billing.md", Some("Projects/old.md\nlocal:Fix login"), true);
        assert!(on.contains("kelpie closes the note when the task is done") && on.contains("a reset or a drop puts it back in the backlog") && !on.contains("the lead is told"), "{on}");
        assert!(on.contains("(it was linked to Projects/old.md, local:Fix login)"), "{on}");
    }

    #[test]
    fn drop_puts_the_note_back_in_the_backlog_unless_another_half_holds_it() {
        let fake = crate::vault_cli::fake::Fake::new("drop");
        let state = fake.state();
        fake.answer("task-backlog", r#"{"path":"Projects/billing.md"}"#);
        let notes = ["Projects/billing.md".to_string()];
        assert_eq!(dropped_notes(None, &state, "billing", "billing@app", &notes), "", "without the vault a drop asks nothing");
        assert!(fake.calls().is_empty());
        assert_eq!(dropped_notes(Some(&fake.cli), &state, "billing", "billing@app", &notes), " kelpie put its note Projects/billing.md back in the backlog.");
        std::fs::create_dir_all(state.join("started")).unwrap();
        std::fs::write(state.join("started").join("billing@api"), "Projects/billing.md").unwrap();
        assert!(dropped_notes(Some(&fake.cli), &state, "billing", "billing@app", &notes).contains("stays open for the task's other halves"));
        assert_eq!(fake.calls(), ["task backlog Projects/billing.md"]);
        let source = include_str!("tasks.rs");
        let discard = &source[source.find("\nfn discard(").unwrap()..];
        assert!(discard[..discard.find("\n}\n").unwrap()].contains("dropped_notes("), "drop settles the note");
    }

    #[test]
    fn done_closes_the_tasks_vault_note_and_without_the_vault_asks_nothing() {
        let fake = crate::vault_cli::fake::Fake::new("finish");
        fake.answer("task-close", r#"{"path":"Archive/billing.md"}"#);
        for vault in [None, Some(&fake.cli)] {
            let (dir, origin, slot, upstream) = task_with_a_commit("billing");
            git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
            let state = state_of(&dir);
            record_start(&started_file_in(&state, "billing", &dir).unwrap(), "Projects/billing.md").unwrap();
            let said = finish(&dir, &slot, &upstream, &state, vault).unwrap();
            assert_eq!(said.contains("Its note is closed (Archive/billing.md)."), vault.is_some(), "{said}");
            cleanup(dir, origin, &slot);
        }
        assert_eq!(fake.calls(), ["task close Projects/billing.md"], "the vault is asked only when it is on");
    }

    #[test]
    fn a_start_entry_keeps_the_backlog_item_next_to_the_note() {
        assert_eq!(start_entry(Some("Projects/web-billing.md"), Some("local:Fix login")).as_deref(), Some("Projects/web-billing.md\nlocal:Fix login"), "the local item stays out of the Backlog column and done still has the lead delete its line");
        assert_eq!(start_entry(Some("Projects/web-billing.md"), Some("Projects/web-billing.md")).as_deref(), Some("Projects/web-billing.md"), "a claimed note is stored once");
        assert_eq!(start_entry(None, Some("ABC-12")).as_deref(), Some("ABC-12"));
        assert_eq!(start_entry(None, None), None);
    }

    #[test]
    fn done_on_one_half_leaves_the_shared_note_to_the_other_and_settles_the_backlog_item() {
        let fake = crate::vault_cli::fake::Fake::new("finish-halves");
        fake.answer("task-close", r#"{"path":"Archive/billing.md"}"#);
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
        let state = state_of(&dir);
        record_start(&started_file_in(&state, "billing", &dir).unwrap(), "Projects/billing.md\nlocal:Fix login").unwrap();
        std::fs::write(state.join("started").join("billing@elsewhere"), "Projects/billing.md").unwrap();
        let said = finish(&dir, &slot, &upstream, &state, Some(&fake.cli)).unwrap();
        assert!(said.contains("Its note Projects/billing.md stays open for the task's other halves."), "{said}");
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
        assert!(!started_file_in(&state, "billing", &dir).unwrap().exists());
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_board_never_waits_on_the_vault_on_its_own_thread() {
        let board: String = std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/board")).unwrap().map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap()).collect();
        for call in ["tasks::done(", "tasks::reset(", "tasks::push(", "tasks::start("] {
            for (at, _) in board.match_indices(call) {
                let line = board[..at].rsplit('\n').next().unwrap_or_default();
                assert!(line.contains("move ||") || line.contains("sender.send("), "{call} runs on the board's thread: {line}");
            }
        }
        assert!(!board.contains("vault_cli::detect") && !board.contains("vault_cli::cli()"), "the board reads the kept verdict only");
    }

    #[test]
    fn every_task_worktree_gets_the_push_block_before_its_claude_or_its_work_arrives() {
        let source = include_str!("tasks.rs");
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("launch").find("push_block::install(").unwrap() < body("launch").find("start_claude(").unwrap(), "a start and a restart both block pushes before Claude runs");
        assert!(body("move_work").find("push_block::install(").unwrap() < body("move_work").find("stash\", \"pop").unwrap(), "moved work lands in a blocked worktree");
    }

    #[test]
    fn kelpie_carries_no_commit_style_rules() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let banned = [["auto", "squash"].concat(), ["co-authored", "-by"].concat(), ["fix", "up"].concat(), ["merge", " commit"].concat(), ["co", "authored"].concat(), ["attri", "bution"].concat()];
        let mut files = vec![root.join("README.md")];
        for dir in ["src", "roles", "skills", "hooks", "scripts"] {
            let mut stack = vec![root.join(dir)];
            while let Some(path) = stack.pop() {
                match std::fs::read_dir(&path) {
                    Ok(entries) => stack.extend(entries.filter_map(|entry| entry.ok()).map(|entry| entry.path())),
                    Err(_) => files.push(path),
                }
            }
        }
        assert!(files.len() > 20, "{files:?}");
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap_or_default().to_lowercase();
            for word in &banned {
                assert!(!text.contains(word.as_str()), "{} names a commit rule: {word}", file.display());
            }
        }
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
    fn ending_a_claude_sends_escape_then_ctrl_c_only_until_the_shell_is_back() {
        let run = |exits_after: usize, tries: usize| {
            let keys = std::cell::RefCell::new(Vec::new());
            let result = end_with(|key| { keys.borrow_mut().push(key.to_string()); Ok(()) }, || Ok(keys.borrow().iter().filter(|key| *key == "ctrl+c").count() >= exits_after), || {}, tries);
            (result, keys.into_inner())
        };
        assert_eq!(run(2, 10), (Ok(()), vec!["esc".to_string(), "ctrl+c".to_string(), "ctrl+c".to_string()]));
        assert_eq!(run(0, 10), (Ok(()), vec!["esc".to_string()]));
        let (result, keys) = run(usize::MAX, 3);
        assert!(result.is_err());
        assert_eq!(keys.len(), 4, "{keys:?}");
    }

    #[test]
    fn start_retries_while_herdr_asks_for_the_prompt_and_stops_on_any_other_refusal() {
        assert!(matches!(refusal("herdr agent start refused: agent pane p_1 is busy (agent_pane_busy)"), Refusal::NotYet));
        assert!(matches!(refusal("herdr agent start refused: agent name lead is already used; candidates: … (agent_name_taken)"), Refusal::NotYet), "a Claude that just exited still holds its name");
        assert!(matches!(refusal("herdr agent start refused: input contains a 1100-byte line but the terminal is in canonical mode with a 1024-byte limit; wait for the shell prompt and retry (agent_start_input_failed)"), Refusal::NotYet));
        assert!(matches!(refusal("herdr agent start refused: not ready (agent_not_ready)"), Refusal::Starting));
        assert!(matches!(refusal("herdr agent start refused: agent target pane p_1 not found (agent_pane_not_found)"), Refusal::Final));
    }

    fn waited(looks: &[(bool, &str)], tries: usize) -> (std::result::Result<(), String>, usize) {
        let mut seen = looks.iter().map(|(at_shell, screen)| (*at_shell, screen.to_string()));
        let mut taken = 0;
        let result = prompt_with(|| { taken += 1; Ok(seen.next().unwrap_or((true, String::new()))) }, || {}, tries).unwrap();
        (result, taken)
    }

    #[test]
    fn claude_is_typed_only_once_the_shell_has_shown_the_same_prompt_for_a_while() {
        let prompt = "~/app/.worktrees/1 main\n❯";
        let settled = [(true, prompt); QUIET_LOOKS + 1];
        assert_eq!(waited(&settled, 20), (Ok(()), QUIET_LOOKS + 1));
        let busy_then_prompt = [(false, ""), (true, ""), (false, "Updating"), (true, prompt), (true, prompt), (true, prompt), (true, prompt), (true, prompt)];
        assert_eq!(waited(&busy_then_prompt, 20), (Ok(()), busy_then_prompt.len()), "startup work and an empty screen keep the wait going");
        let redrawn = [(true, "❯"), (true, "❯ 1"), (true, "❯"), (true, "❯ 1")];
        assert_eq!(waited(&redrawn, 4), (Ok(()), 4), "a prompt that never settles ends the wait without an error");
    }

    #[test]
    fn a_shell_asking_a_question_as_it_starts_gets_no_keys_and_the_question_is_named() {
        let omz = "[oh-my-zsh] Would you like to update? [Y/n]";
        let (result, taken) = waited(&[(true, omz); 12], 12);
        assert_eq!((result, taken), (Err(omz.to_string()), 12), "a question that holds still is not a settled prompt");
        let answered = [(true, omz), (true, omz), (false, "git pull"), (true, "❯"), (true, "❯"), (true, "❯"), (true, "❯"), (true, "❯")];
        assert_eq!(waited(&answered, 20), (Ok(()), answered.len()), "once answered in the pane the wait goes on to the prompt");
        assert!(asks("Proceed? (y/N)") && asks("Continue [yes/no]") && !asks("~/app ❯") && !asks("% "));
    }

    #[test]
    fn a_start_whose_line_the_shell_read_away_is_tried_once_more_and_then_named_by_its_pane() {
        let run = |outcomes: Vec<Launched>| {
            let mut outcomes = outcomes.into_iter();
            let mut tries = 0;
            let result = start_twice("w1-2", || { tries += 1; Ok(outcomes.next().unwrap()) });
            (result, tries)
        };
        assert_eq!(run(vec![Launched::Ready]), (Ok(()), 1));
        assert_eq!(run(vec![Launched::ShellBack("zsh: command not found: laude".to_string()), Launched::Ready]), (Ok(()), 2));
        let (result, tries) = run(vec![Launched::ShellBack("❯".to_string()), Launched::ShellBack("❯".to_string())]);
        assert_eq!(tries, 2);
        assert!(result.unwrap_err().starts_with("Claude did not start in pane w1-2: its shell is back at the prompt, showing \"❯\""));
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
        let source = include_str!("tasks.rs");
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("start").find("store_prompt(").unwrap() < body("start").find("launch(").unwrap(), "the prompt is kept even when the first start fails");
        assert!(body("start").contains("restart(&place(&existing), &existing, focus, prompt)"), "start's prompt reaches the resume path");
        assert!(body("restart").contains("prompt_for(&session, given, stored, unheard)") && body("restart").contains("typing::prompt_confirmed(&agent_name, prompt"));
        assert!(body("restart").contains("user_messages(&text).is_empty()"));
        assert!(body("forget_session").contains("prompt_file("));
    }

    #[test]
    fn a_folder_counts_as_trusted_when_it_or_a_folder_above_it_is() {
        let claude = json!({ "projects": { "/home/ada/code": { "hasTrustDialogAccepted": true }, "/home/ada/other": { "hasTrustDialogAccepted": false } } });
        assert!(trusted_in(&claude, Path::new("/home/ada/code")));
        assert!(trusted_in(&claude, Path::new("/home/ada/code/shop")));
        assert!(!trusted_in(&claude, Path::new("/home/ada/other/shop")));
        assert!(!trusted_in(&claude, Path::new("/home/ada")));
        assert!(!trusted_in(&json!({}), Path::new("/home/ada/code")));
    }

    fn repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-test-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "--quiet"]).unwrap();
        git(&dir, &["config", "user.name", "Ada"]).unwrap();
        git(&dir, &["config", "user.email", "ada@example.com"]).unwrap();
        for (file, message) in [("a", "first"), ("b", "second"), ("c", "third")] {
            std::fs::write(dir.join(file), file).unwrap();
            git(&dir, &["add", file]).unwrap();
            git(&dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", message]).unwrap();
        }
        dir
    }

    #[test]
    fn a_slot_is_a_detached_worktree_with_its_setup_note_in_its_git_dir() {
        let dir = repo();
        let slot = create_slot(&dir, "HEAD").unwrap();
        assert!(slot.path.join("a").exists());
        assert_eq!(slot.path, slot_path(&dir, 1));
        assert_eq!(setup_line(&slot.path), None);
        let note = crate::repo::setup_note(&slot.path).unwrap();
        assert!(note.parent().unwrap().canonicalize().unwrap().starts_with(dir.join(".git/worktrees").canonicalize().unwrap()), "{}", note.display());
        std::fs::write(&note, "# wt1.app set up 2026-10-17 by task billing\n- gems: bundle install\n").unwrap();
        assert_eq!(setup_line(&slot.path).as_deref(), Some("wt1.app set up 2026-10-17 by task billing"));
        assert!(is_clean(&slot.path).unwrap());
        git(&dir, &["worktree", "remove", "--force", &slot.path.to_string_lossy()]).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reword_changes_one_message_and_keeps_the_tree_and_author() {
        let dir = repo();
        let tree = git(&dir, &["rev-parse", "HEAD^{tree}"]).unwrap();
        let second = git(&dir, &["rev-parse", "HEAD~1"]).unwrap();
        reword(&dir, &second, "second, reworded\n\nwith a body").unwrap();
        assert_eq!(git(&dir, &["log", "--format=%s", "-3"]).unwrap(), "third\nsecond, reworded\nfirst");
        assert_eq!(git(&dir, &["log", "-1", "--format=%b", "HEAD~1"]).unwrap(), "with a body");
        assert_eq!(git(&dir, &["rev-parse", "HEAD^{tree}"]).unwrap(), tree);
        assert_eq!(git(&dir, &["log", "--format=%an", "-3"]).unwrap(), "Ada\nAda\nAda");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn squash_makes_one_commit_on_the_default_branch_with_the_same_tree() {
        let dir = repo();
        let origin = dir.with_extension("origin");
        let _ = std::fs::remove_dir_all(&origin);
        git(&dir, &["clone", "--quiet", "--bare", ".", &origin.to_string_lossy()]).unwrap();
        git(&dir, &["remote", "add", "origin", &origin.to_string_lossy()]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        let branch = git(&dir, &["branch", "--show-current"]).unwrap();
        git(&dir, &["update-ref", &format!("refs/remotes/origin/{branch}"), "HEAD~2"]).unwrap();
        git(&dir, &["symbolic-ref", "refs/remotes/origin/HEAD", &format!("refs/remotes/origin/{branch}")]).unwrap();
        let tree = git(&dir, &["rev-parse", "HEAD^{tree}"]).unwrap();
        squash(&dir, "second and third").unwrap();
        assert_eq!(git(&dir, &["log", "--format=%s", "-2"]).unwrap(), "second and third\nfirst");
        assert_eq!(git(&dir, &["rev-parse", "HEAD^{tree}"]).unwrap(), tree);
        assert_eq!(git(&dir, &["log", "-1", "--format=%an"]).unwrap(), "Ada");
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(origin).unwrap();
    }

    fn task_with_a_commit(name: &str) -> (PathBuf, PathBuf, Slot, String) {
        let dir = repo();
        let origin = dir.with_extension("origin");
        let _ = std::fs::remove_dir_all(&origin);
        git(&dir, &["clone", "--quiet", "--bare", ".", &origin.to_string_lossy()]).unwrap();
        git(&dir, &["remote", "add", "origin", &origin.to_string_lossy()]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        let branch = git(&dir, &["branch", "--show-current"]).unwrap();
        git(&dir, &["symbolic-ref", "refs/remotes/origin/HEAD", &format!("refs/remotes/origin/{branch}")]).unwrap();
        let slot = create_slot(&dir, "HEAD").unwrap();
        git(&slot.path, &["switch", "--quiet", "--create", name]).unwrap();
        std::fs::write(slot.path.join("d"), "d").unwrap();
        git(&slot.path, &["add", "d"]).unwrap();
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", "fourth"]).unwrap();
        let slot = find_task(&dir, Some(name), &dir).unwrap();
        (dir, origin, slot, format!("origin/{branch}"))
    }

    fn state_of(dir: &Path) -> PathBuf {
        let state = dir.with_extension("state");
        std::fs::create_dir_all(&state).unwrap();
        state
    }

    fn cleanup(dir: PathBuf, origin: PathBuf, slot: &Slot) {
        let _ = std::fs::remove_dir_all(dir.with_extension("state"));
        git(&dir, &["worktree", "remove", "--force", &slot.path.to_string_lossy()]).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(origin).unwrap();
    }

    #[test]
    fn done_frees_a_pushed_task_whose_branch_has_no_upstream_while_the_local_branch_is_behind() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
        assert_eq!(ahead_behind(&dir, &upstream).unwrap(), (0, 1));
        finish(&dir, &slot, &upstream, &state_of(&dir), None).unwrap();
        assert!(!branch_exists(&dir, "billing"));
        assert!(slots(&dir).unwrap().iter().any(|free| free.path == slot.path && free.free));
        assert_eq!(ahead_behind(&dir, &upstream).unwrap(), (0, 0));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_tasks_temp_dir_is_its_own_outside_the_worktree_and_goes_when_the_task_is_done() {
        let (dir, origin, slot, upstream) = task_with_a_commit("scratch");
        let tmp = task_tmp(&slot.path).unwrap();
        assert!(tmp.starts_with(dir.join(".git/worktrees").canonicalize().unwrap()), "{}", tmp.display());
        std::fs::create_dir_all(tmp.join("deep")).unwrap();
        std::fs::write(tmp.join("deep/file"), "x").unwrap();
        assert!(is_clean(&slot.path).unwrap(), "temp files never show as changes in the worktree");
        let written: Value = serde_json::from_str(&settings("/bin/kelpie", "billing@app", None, Some(&tmp))).unwrap();
        for name in TMP_VARS {
            assert_eq!(written["env"][name], json!(tmp.to_string_lossy()), "{name}");
        }
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "scratch"]).unwrap();
        finish(&dir, &slot, &upstream, &state_of(&dir), None).unwrap();
        assert!(!tmp.exists(), "done takes the temp dir and what is in it");
        std::fs::create_dir_all(&tmp).unwrap();
        git(&slot.path, &["switch", "--quiet", "--create", "again"]).unwrap();
        let again = find_task(&dir, Some("again"), &dir).unwrap();
        release(&dir, &again, &upstream).unwrap();
        assert!(!tmp.exists(), "drop and reset free the worktree through the same release");
        let source = include_str!("tasks.rs");
        let discard = &source[source.find("\nfn discard(").unwrap()..];
        assert!(discard[..discard.find("\n}\n").unwrap()].contains("release(&repo, &slot, &upstream)"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn done_refuses_a_task_with_commits_not_on_upstream_and_leaves_it_as_it_was() {
        let (dir, origin, slot, upstream) = task_with_a_commit("search");
        assert!(finish(&dir, &slot, &upstream, &state_of(&dir), None).is_err());
        assert!(branch_exists(&dir, "search"));
        assert_eq!(git(&slot.path, &["branch", "--show-current"]).unwrap(), "search");
        cleanup(dir, origin, &slot);
    }

    fn commit(path: &Path, file: &str, message: &str) -> String {
        std::fs::write(path.join(file), message).unwrap();
        git(path, &["add", file]).unwrap();
        git(path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", message]).unwrap();
        git(path, &["rev-parse", "HEAD"]).unwrap()
    }

    // Main moves on first, so the landed commits cannot come out with the task's own shas.
    fn land_on_main(dir: &Path, upstream: &str, land: impl Fn(&Path)) {
        commit(dir, "elsewhere", "someone else's work");
        land(dir);
        git(dir, &["update-ref", &format!("refs/remotes/{upstream}"), "HEAD"]).unwrap();
    }

    fn pick(dir: &Path, sha: &str) {
        git(dir, &["-c", "user.name=Bo", "-c", "user.email=bo@example.com", "cherry-pick", sha]).unwrap();
    }

    #[test]
    fn done_frees_a_task_whose_commits_were_cherry_picked_onto_upstream() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fifth = commit(&slot.path, "e", "fifth");
        let fourth = git(&slot.path, &["rev-parse", "HEAD~1"]).unwrap();
        land_on_main(&dir, &upstream, |dir| { pick(dir, &fourth); pick(dir, &fifth); });
        assert!(ahead_behind(&slot.path, &upstream).unwrap().0 > 0);
        finish(&dir, &slot, &upstream, &state_of(&dir), None).unwrap();
        assert!(!branch_exists(&dir, "billing"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn done_frees_a_task_whose_commits_were_squashed_into_one_on_upstream() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        commit(&slot.path, "d", "fifth, rewriting the fourth");
        land_on_main(&dir, &upstream, |dir| {
            git(dir, &["merge", "--squash", "--quiet", "billing"]).unwrap();
            git(dir, &["-c", "user.name=Bo", "-c", "user.email=bo@example.com", "commit", "--quiet", "-m", "billing, squashed"]).unwrap();
        });
        assert!(git(&slot.path, &["cherry", &upstream, "HEAD"]).unwrap().contains('+'));
        finish(&dir, &slot, &upstream, &state_of(&dir), None).unwrap();
        assert!(!branch_exists(&dir, "billing"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn done_refuses_a_task_with_one_change_missing_from_upstream_and_names_that_commit() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fifth = commit(&slot.path, "e", "fifth");
        let fourth = git(&slot.path, &["rev-parse", "HEAD~1"]).unwrap();
        land_on_main(&dir, &upstream, |dir| pick(dir, &fourth));
        let error = finish(&dir, &slot, &upstream, &state_of(&dir), None).err().unwrap();
        assert!(error.contains(&fifth[..7]) && error.contains("fifth"), "{error}");
        assert!(!error.contains("fourth") && !error.contains(&fourth[..7]), "{error}");
        assert!(branch_exists(&dir, "billing"));
        assert_eq!(git(&slot.path, &["branch", "--show-current"]).unwrap(), "billing");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_code_survives_a_clean_rebase_a_reword_and_a_squash_and_not_a_changed_line() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        commit(&slot.path, "e", "fifth");
        let reviewed = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        land_on_main(&dir, &upstream, |_| ());
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "rebase", "--quiet", &upstream]).unwrap();
        let head = || git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(head(), reviewed);
        assert!(same_code(&slot.path, &upstream, &reviewed, &head()), "a clean rebase keeps the reviewed code");
        reword(&slot.path, &head(), "fifth, reworded").unwrap();
        assert!(same_code(&slot.path, &upstream, &reviewed, &head()), "a new message keeps it");
        squash(&slot.path, "fourth and fifth").unwrap();
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0));
        assert!(same_code(&slot.path, &upstream, &reviewed, &head()), "a squash keeps it");
        commit(&slot.path, "e", "fifth, changed");
        assert!(!same_code(&slot.path, &upstream, &reviewed, &head()), "a changed line is code the reviewer never read");
        assert!(!same_code(&slot.path, &upstream, "0000000000000000000000000000000000000000", &head()), "a sha that is gone is not the same code");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn push_takes_one_of_the_tasks_own_commits_and_a_full_push_only_up_to_head() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let fifth = commit(&slot.path, "e", "fifth");
        assert_eq!(pushable_through(&slot.path, &upstream, &fourth, true).unwrap(), fourth);
        assert_eq!(pushable_through(&slot.path, &upstream, &fourth[..10], true).unwrap(), fourth);
        assert_eq!(pushable_through(&slot.path, &upstream, &fifth, false).unwrap(), fifth);
        let unseen = pushable_through(&slot.path, &upstream, &fourth, false).unwrap_err();
        assert!(unseen.contains("not shown"), "a commit that arrived after the dialog opened is never pushed unseen: {unseen}");
        assert!(pushable_through(&slot.path, &upstream, &upstream, true).unwrap_err().contains("not one of the task's commits"));
        let elsewhere = commit(&dir, "x", "on main only");
        assert!(pushable_through(&slot.path, &upstream, &elsewhere, true).unwrap_err().contains("not one of the task's commits"));
        assert!(pushable_through(&slot.path, &upstream, "no-such-ref", true).unwrap_err().contains("is not a commit"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_of_the_first_commits_leaves_the_task_open_with_the_rest_on_top_and_its_approval() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        let state = dir.with_extension("state");
        let _ = std::fs::remove_dir_all(&state);
        record_approval(&state, "billing@app", &fingerprint(&slot.path, &upstream).unwrap()).unwrap();
        crate::report::save_card(&state, "billing@app", &fourth, "Refunds on the billing page.").unwrap();
        assert_eq!(push_prefix(&slot.path, &upstream, &fourth, &state).unwrap(), 1);
        let branch = upstream.trim_start_matches("origin/");
        assert_eq!(git(&origin, &["rev-parse", branch]).unwrap(), fourth, "origin has the first commit");
        assert_eq!(keep_going(&state, &dir, &slot, &upstream, "billing@app", true).unwrap(), Rest::Rebased { clean: true, rest: 1 });
        assert!(branch_exists(&dir, "billing"), "the task stays open");
        assert_eq!(git(&slot.path, &["branch", "--show-current"]).unwrap(), "billing");
        assert!(slots(&dir).unwrap().iter().any(|busy| busy.path == slot.path && !busy.free), "its worktree stays the task's");
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0), "the rest sits on top of what was pushed");
        assert_eq!(git(&slot.path, &["log", "--format=%s", &format!("{upstream}..HEAD")]).unwrap(), "fifth", "no pushed commit twice");
        assert!(approval_holds(&state, "billing@app", &fingerprint(&slot.path, &upstream).unwrap()), "an approval of the whole branch holds for the rest");
        assert!(crate::report::card_in(&state, "billing@app").is_none(), "the card was for what was pushed");
        assert!(matches!(keep_going(&state.join("other"), &dir, &slot, &upstream, "billing@app", false).unwrap(), Rest::Rebased { rest: 1, .. }));
        assert!(!approval_file(&state.join("other"), "billing@app").exists(), "no approval is made up for a branch the lead never approved");
        std::fs::remove_dir_all(state).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_partial_push_that_landed_is_reported_even_when_the_fetch_after_it_fails() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        let state = dir.with_extension("state");
        let _ = std::fs::remove_dir_all(&state);
        crate::report::save_card(&state, "billing@app", &fourth, "Refunds on the billing page.").unwrap();
        push_prefix(&slot.path, &upstream, &fourth, &state).unwrap();
        git(&dir, &["remote", "set-url", "origin", &dir.with_extension("gone").to_string_lossy()]).unwrap();
        assert!(matches!(keep_going(&state, &dir, &slot, &upstream, "billing@app", true).unwrap(), Rest::NotFetched(_)), "the commits are on origin, so the push is no failure");
        assert!(crate::report::card_in(&state, "billing@app").is_none(), "the card was for what was pushed");
        git(&dir, &["remote", "set-url", "origin", &origin.to_string_lossy()]).unwrap();
        std::fs::remove_dir_all(state).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_through_the_last_commit_closes_the_task_like_a_full_push() {
        assert!(!stays_open(true, "abc", "abc"), "p on the last commit, or --through HEAD, leaves no commit to keep open for");
        assert!(stays_open(true, "abc", "def"));
        assert!(!stays_open(false, "abc", "def"));
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
    fn a_slug_is_lower_case_ascii_letters_digits_and_dashes_that_start_with_a_letter() {
        assert_eq!(slug("Fix Login").unwrap(), "fix-login");
        assert_eq!(slug("  --fix--login--  ").unwrap(), "fix-login");
        assert_eq!(slug("Föräldraledighet över jul").unwrap(), "foraldraledighet-over-jul");
        assert_eq!(slug("Straße/Ærø/çañon").unwrap(), "strasse-aero-canon");
        assert_eq!(slug("../../etc/passwd").unwrap(), "etc-passwd", "a path is a name: it never leaves the pool");
        assert_eq!(slug("a; rm -rf / #").unwrap(), "a-rm-rf");
        assert_eq!(slug("x$(id)`id`").unwrap(), "x-id-id");
        assert_eq!(slug(&format!("a{}", "b".repeat(100))).unwrap().len(), 40, "forty characters at most");
        assert_eq!(slug(&format!("{}-c", "a".repeat(39))).unwrap(), "a".repeat(39), "a cut never ends in a dash");
        assert_eq!(slug("-x").unwrap(), "x", "leading punctuation is dropped, not kept as a dash");
        assert_eq!(slug("@app").unwrap(), "app", "so an address-like name is its task part");
        for bad in ["", "123", "1-x", "!", "日本語", "  "] {
            assert!(slug(bad).unwrap_err().contains("needs to start with a letter"), "{bad:?}");
        }
        assert_eq!(ascii_fold("ÅÄÖ åäö ÉÈ ü ñ ß Æ"), "AAO aao EE u n ss AE");
        assert_eq!(ascii_fold("日本語"), "日本語", "other scripts are left for the slug to drop");
    }

    #[test]
    fn a_task_address_and_its_files_follow_the_slug_not_the_name_given() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        assert_eq!(find_task(&dir, Some("Billing"), &dir).unwrap().path, slot.path);
        assert_eq!(find_task(&dir, Some("BILLING!"), &dir).unwrap().task.as_deref(), Some("billing"));
        assert!(find_task(&dir, Some("../billing"), &dir).is_ok(), "the slug of a path is the task");
        assert!(find_task(&dir, Some("search"), &dir).err().unwrap().contains("No task \"search\""));
        assert!(find_task(&dir, None, &dir).err().unwrap().contains("name the task"));
        assert_eq!(find_task(&dir, None, &slot.path.join("src")).unwrap().task.as_deref(), Some("billing"), "a folder under the worktree is that task");
        let state = state_of(&dir);
        let file = started_file_in(&state, "billing", &dir).unwrap();
        assert_eq!(file, state.join("started").join(format!("billing@{}", name_of(&dir))));
        assert!(file.parent().unwrap().starts_with(&state));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_transcripts_folder_is_the_worktree_path_with_every_other_character_as_a_dash() {
        assert_eq!(transcripts_dir(Path::new("/home/ada/.claude"), Path::new("/src/wt1.app")), PathBuf::from("/home/ada/.claude/projects/-src-wt1-app"));
        assert_eq!(transcripts_dir(Path::new("/c"), Path::new("/Users/Ada Ö/x_y")).file_name().unwrap(), "-Users-Ada---x-y");
        assert!(!transcripts_dir(Path::new("/c"), Path::new("/a/../b")).file_name().unwrap().to_string_lossy().contains('.'));
    }

    #[test]
    fn capitalized_takes_the_first_character_up_and_leaves_the_rest() {
        assert_eq!(capitalized("the lead"), "The lead");
        assert_eq!(capitalized(""), "");
        assert_eq!(capitalized("ärlig"), "Ärlig");
        assert_eq!(capitalized("ßx"), "SSx");
    }

    #[test]
    fn the_settle_counts_quiet_looks_only_at_a_shell_prompt_that_asks_nothing() {
        let mut settle = Settle::default();
        assert_eq!(settle.look(false, "❯"), Prompt::Waiting);
        assert_eq!(settle.look(true, ""), Prompt::Waiting, "an empty screen is not a prompt");
        assert_eq!(settle.look(true, "Proceed? [y/N]"), Prompt::Asking("Proceed? [y/N]".to_string()));
        for _ in 0..QUIET_LOOKS {
            assert_eq!(settle.look(true, "❯"), Prompt::Waiting, "the first look at a new screen starts the count, the quiet ones add to it");
        }
        assert_eq!(settle.look(true, "❯"), Prompt::Ready);
        assert_eq!(settle.look(true, "❯ x"), Prompt::Waiting, "a change starts the count again");
        assert_eq!(last_line("a\n\n  b  \n\n"), "b");
        assert_eq!(last_line(""), "");
    }

    #[test]
    fn a_start_entry_and_its_items_round_trip_through_the_started_file() {
        let file = std::env::temp_dir().join(format!("kelpie-started-entry-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let entry = start_entry(Some("Projects/x.md"), Some("local:Fix login")).unwrap();
        std::fs::write(&file, &entry).unwrap();
        assert_eq!(crate::task_note::items(&std::fs::read_to_string(&file).unwrap()), ["Projects/x.md", "local:Fix login"]);
        assert_eq!(crate::task_note::note_in(&entry).as_deref(), Some("Projects/x.md"));
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn the_fingerprint_is_fnv1a_so_an_approval_survives_a_rust_upgrade() {
        let mut hasher = Fnv::default();
        hasher.write(b"");
        assert_eq!(format!("{:016x}", hasher.0), "cbf29ce484222325");
        let mut hasher = Fnv::default();
        hasher.write(b"a");
        assert_eq!(format!("{:016x}", hasher.0), "af63dc4c8601ec8c");
        let mut split = Fnv::default();
        split.write(b"ab");
        split.write(b"c");
        let mut whole = Fnv::default();
        whole.write(b"abc");
        assert_eq!(split.0, whole.0);
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

    #[test]
    fn a_slot_is_free_only_on_its_placeholder_branch_or_detached_and_clean() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let slots_now = slots(&dir).unwrap();
        assert_eq!(slots_now.len(), 1);
        assert!(!slots_now[0].free && slots_now[0].task.as_deref() == Some("billing"));
        git(&slot.path, &["switch", "--quiet", "--detach"]).unwrap();
        assert!(slots(&dir).unwrap()[0].free, "a detached clean slot is free");
        std::fs::write(slot.path.join("scratch"), "x").unwrap();
        assert!(!slots(&dir).unwrap()[0].free, "an untracked file makes it busy, so a start never takes it");
        assert_eq!(slots(&dir).unwrap()[0].task, None, "but it is no task either");
        std::fs::remove_file(slot.path.join("scratch")).unwrap();
        git(&slot.path, &["switch", "--quiet", "-c", "wt1"]).unwrap();
        assert!(slots(&dir).unwrap()[0].free, "the wt<N> placeholder branch is free");
        git(&slot.path, &["switch", "--quiet", "billing"]).unwrap();
        git(&dir, &["branch", "-D", "wt1"]).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn missing_from_names_only_the_commits_whose_change_upstream_lacks() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "--short", "HEAD"]).unwrap();
        let missing = missing_from(&slot.path, &upstream).unwrap();
        assert_eq!(missing.len(), 1);
        assert!(missing[0].starts_with(&fourth) && missing[0].ends_with("fourth"), "{missing:?}");
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
        assert!(missing_from(&slot.path, &upstream).unwrap().is_empty());
        cleanup(dir, origin, &slot);
    }
}
