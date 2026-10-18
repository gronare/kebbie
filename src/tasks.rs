use std::collections::HashSet;
use std::time::Duration;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config;
use crate::herdr;
use crate::lead::state_dir;
use crate::messages;
use crate::peers;
use crate::repo::{
    ahead_behind, branch_exists, changed_count, default_branch, git, is_clean, main_checkout, name_of, push_as_user,
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

fn find_task(repo: &Path, name: Option<&str>, cwd: &Path) -> Result<Slot> {
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

fn task_of(slot: &Slot) -> &str {
    slot.task.as_deref().unwrap_or_default()
}

fn fetch(repo: &Path) -> Result<()> {
    git(repo, &["fetch", "--quiet", "origin"]).map(|_| ())
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

// Pushing stays the user's step; the push URL rewrite catches what the deny rule's pattern misses (git -C dir push).
// The hooks ask for a report at the end of every turn.
fn settings(kelpie: &str) -> String {
    let hook = json!([{ "hooks": [{ "type": "command", "command": format!("'{kelpie}' hook") }] }]);
    let allow = ["peers", "tell", "report"].map(|command| format!("Bash({kelpie} {command}:*)"));
    json!({
        "env": { "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "url.kelpie-blocks-push:.pushInsteadOf", "GIT_CONFIG_VALUE_0": "" },
        "permissions": { "deny": ["Bash(git push:*)".to_string(), format!("Bash({kelpie} approve:*)")], "allow": allow },
        "hooks": { "UserPromptSubmit": hook, "Stop": hook },
    }).to_string()
}

enum Session {
    New(String),
    Resume(String),
}

fn claude_args(mode: Option<&str>, settings: &Path, role: &Path, session: &Session) -> Vec<String> {
    let mut args: Vec<String> = mode.map(|mode| vec!["--permission-mode".to_string(), mode.to_string()]).unwrap_or_default();
    args.extend(["--settings".to_string(), settings.to_string_lossy().to_string(), "--append-system-prompt-file".to_string(), role.to_string_lossy().to_string()]);
    args.extend(match session {
        Session::New(id) => ["--session-id".to_string(), id.clone()],
        Session::Resume(id) => ["--resume".to_string(), id.clone()],
    });
    args
}

fn new_session_id() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut random| random.read_exact(&mut bytes)).map_err(|error| error.to_string())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

fn session_file(task: &str, repo: &Path) -> Result<PathBuf> {
    Ok(crate::lead::state_path(|name| std::env::var(name).ok())?.join("sessions").join(peers::address(task, &name_of(repo))))
}

// The task's Claude session id and permission mode, one per line, so a restart resumes that conversation in the same mode.
fn recorded_session(task: &str, repo: &Path) -> (Option<String>, Option<String>) {
    let text = session_file(task, repo).ok().and_then(|file| std::fs::read_to_string(file).ok()).unwrap_or_default();
    let mut lines = text.lines().map(str::trim).map(|line| (!line.is_empty()).then(|| line.to_string()));
    (lines.next().flatten(), lines.next().flatten())
}

fn forget_session(task: &str, repo: &Path) {
    if let Ok(file) = session_file(task, repo) {
        let _ = std::fs::remove_file(file);
    }
}

// Claude Code keeps a folder's conversations as <session id>.jsonl under its projects dir, the folder's path with every other character than a letter or digit as '-'.
fn transcripts_dir(claude_home: &Path, folder: &Path) -> PathBuf {
    let name: String = folder.to_string_lossy().chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '-' }).collect();
    claude_home.join("projects").join(name)
}

fn claude_home() -> Option<PathBuf> {
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
    let dir = crate::lead::state_dir()?.join("started");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join(peers::address(task, &name_of(repo))))
}

pub fn started_items() -> HashSet<String> {
    let Ok(dir) = crate::lead::state_dir().map(|dir| dir.join("started")) else { return HashSet::new() };
    std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok()).map(|item| item.trim().to_string()).collect()
}

fn forget_start(task: &str, repo: &Path) -> Option<String> {
    let file = started_file(task, repo).ok()?;
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
    let item = forget_start(&task, &repo);
    let dropped = discard(&repo, Some(&task), true, "reset")?;
    let mut notice = format!("The user reset {task}: it is gone, and its work was discarded and will never be pushed.");
    if !discarded.is_empty() {
        notice.push_str(&format!(" Discarded: {}.", discarded.lines().collect::<Vec<_>>().join("; ")));
    }
    if let Some(item) = &item {
        notice.push_str(&format!(" Nobody works on {item} now: set it back to backlog."));
    }
    notice.push_str(" Drop any review or recommendation about it.");
    if let Err(error) = messages::notify_lead(&notice) {
        eprintln!("Could not tell the lead: {error}");
    }
    Ok(format!("{dropped}{}", if item.is_some() { " Its backlog item is back in the Backlog column." } else { "" }))
}

enum Refusal {
    NotYet,
    Starting,
    Final,
}

// A new pane's shell takes a moment to reach its prompt; until then herdr calls the pane busy, or on macOS measures the line against a shorter limit and asks for a retry.
fn refusal(error: &str) -> Refusal {
    if error.contains("agent_pane_busy") || error.contains("wait for the shell prompt and retry") {
        Refusal::NotYet
    } else if error.contains("agent_not_ready") || error.contains("timeout") {
        Refusal::Starting
    } else {
        Refusal::Final
    }
}

pub fn start_claude(name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<()> {
    let mut last = String::new();
    for _ in 0..30 {
        match herdr::start_agent(name, "claude", pane, args) {
            Ok(()) => return Ok(()),
            Err(error) => match refusal(&error) {
                Refusal::NotYet => last = error,
                Refusal::Starting => return wait_until_ready(name, pane, trusted),
                Refusal::Final => return Err(format!("herdr would not start Claude in {pane}: {error}")),
            },
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err(format!("The shell in {pane} never became ready for Claude; herdr said: {last}"))
}

// Claude asks about a folder it has not seen with "No, exit" selected; a prompt sent before it is ready is lost.
fn wait_until_ready(name: &str, pane: &str, trusted: bool) -> Result<()> {
    for _ in 0..120 {
        let screen = herdr::read_pane(pane)?;
        if screen.contains("trust this folder") {
            if !trusted {
                return Err(format!("Claude in {pane} asks whether you trust its folder; you have not trusted the repo in Claude Code, so that answer is yours"));
            }
            herdr::send_keys(pane, &[if screen.contains("❯ Yes, I trust this folder") { "enter" } else { "down" }])?;
        } else if herdr::agent_ready(name)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!("Claude in {pane} never became ready; see what it waits for there"))
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
    let config::Settings { setup_skill, rules_skill, .. } = config::resolve(Some(&repo))?.settings;
    let (setup_skill, rules_skill) = (setup_skill.value, rules_skill.value);
    let place = |slot: &Slot| Launch { repo: &repo, task: &task, mode, setup_skill: &setup_skill, rules_skill: &rules_skill, worktree: slot.path.clone() };
    if let Some(existing) = slots(&repo)?.into_iter().find(|slot| slot.task.as_deref() == Some(task.as_str())) {
        return restart(&place(&existing), &existing, focus);
    }
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    let base = from.unwrap_or(&upstream);
    let slot = claim_slot(&repo, &upstream)?;
    if branch_exists(&repo, &task) {
        git(&slot.path, &["switch", "--quiet", &task])?;
    } else {
        git(&slot.path, &["switch", "--quiet", "-c", &task, base])?;
    }

    let space = open_space(&repo, &slot, &task, false)?;
    let claude_pane = match space.root_pane_id {
        Some(pane) => pane,
        None => herdr::create_tab(&space.workspace_id, &slot.path, "claude")?,
    };
    let (agent_name, started) = launch(&place(&slot), &claude_pane, Session::New(new_session_id()?))?;
    // The prompt goes in as a paste: as an argument, herdr would have to quote it for the shell.
    let prompted = match (&started, prompt) {
        (Ok(()), Some(prompt)) => herdr::prompt_agent(&agent_name, prompt).map_err(|error| format!("Started {task} in {}, but sending the prompt failed: {error}", slot.path.display())),
        _ => Ok(()),
    };
    if focus {
        herdr::focus_workspace(&space.workspace_id)?;
    }
    started?;
    prompted?;
    if let Some(item) = backlog {
        started_file(&task, &repo).and_then(|file| std::fs::write(file, item).map_err(|error| error.to_string()))?;
    }
    if let Err(error) = messages::announce(&task, &name_of(&repo)) {
        eprintln!("Could not tell {task}'s other halves it started: {error}");
    }
    Ok(format!("Started {task} in {}.", slot.path.display()))
}

struct Launch<'a> {
    repo: &'a Path,
    task: &'a str,
    mode: Option<&'a str>,
    setup_skill: &'a str,
    rules_skill: &'a str,
    worktree: PathBuf,
}

// Starts the task's Claude in the pane with its role and settings; the outer error is setup, the inner one is Claude not starting.
fn launch(place: &Launch, pane: &str, session: Session) -> Result<(String, Result<()>)> {
    let Launch { repo, task, mode, setup_skill, rules_skill, ref worktree } = *place;
    // herdr's agent names are global, so a task's half in another repo is named with its repo.
    let taken = herdr::agents()?.iter().any(|agent| agent["name"] == task);
    let agent_name: String = if taken { format!("{task}_{}", slug(&name_of(repo))?) } else { task.to_string() }.chars().take(32).collect();
    let settings = crate::lead::write_settings(&crate::lead::roles_dir()?, &peers::address(task, &name_of(repo)), &settings(&peers::kelpie_command()?))?;
    let role = peers::role_file(task, &peers::Place { repo, worktree, setup_skill, rules_skill })?;
    let agent_args = claude_args(mode, &settings, &role, &session);
    let started = start_claude(&agent_name, pane, &agent_args.iter().map(String::as_str).collect::<Vec<_>>(), trusted_by_user(repo));
    if let (Ok(()), Session::New(id)) = (&started, &session) {
        let record = format!("{id}\n{}\n", mode.unwrap_or_default());
        let file = session_file(task, repo)?;
        std::fs::create_dir_all(file.parent().ok_or("session file has no folder")?).and_then(|_| std::fs::write(file, record)).map_err(|error| error.to_string())?;
    }
    Ok((agent_name, started))
}

fn claude_in(workspace: &str) -> Result<bool> {
    Ok(herdr::agents()?.iter().any(|agent| agent["agent"] == "claude" && agent["workspace_id"].as_str() == Some(workspace)))
}

// A task whose Space or Claude is gone gets them back, its last conversation resumed and its first prompt not sent again.
fn restart(place: &Launch, slot: &Slot, focus: bool) -> Result<String> {
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
    let place = Launch { mode: place.mode.or(mode.as_deref()), worktree: place.worktree.clone(), ..*place };
    let resumed = previous.is_some();
    let session = match previous {
        Some(id) => Session::Resume(id),
        None => Session::New(new_session_id()?),
    };
    let (_, started) = launch(&place, &pane, session)?;
    if focus {
        herdr::focus_workspace(&space.workspace_id)?;
    }
    started?;
    let how = if resumed { "resuming its last conversation" } else { "with a new conversation, as none of its own was found" };
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
    if git(&slot.path, &["rebase", "--quiet", &upstream]).is_err() {
        git(&slot.path, &["rebase", "--abort"])?;
        let Some(pane) = claude_of(&slot.path) else {
            return Err(format!("{task} conflicts with {upstream} and has no Claude to resolve it; rebase it in {}", slot.path.display()));
        };
        herdr::prompt_agent(&pane, &format!(
            "kelpie tried to land this task and rebasing onto {upstream} conflicts (already fetched). Rebase this branch onto {upstream} \
             with git rebase {upstream} (no -i), resolve each conflict keeping the intent of both sides (read the upstream \
             commits that touched the file), run the tests for the files involved, and continue until the rebase is finished. Do not \
             push. Then tell me in a few lines what conflicted and how you resolved it."
        ))?;
        return Err(format!("{task} conflicts with {upstream}; its Claude is resolving the rebase. Push and close again when it reports back."));
    }
    let commits = git(&slot.path, &["log", "--oneline", &format!("{upstream}..HEAD")])?;
    let target = upstream.trim_start_matches("origin/");
    Ok(format!(
        "{task} is rebased onto {upstream} and ready:\n{commits}\n\nPush with:  kelpie push {task}   (or git -C {} push origin HEAD:{target}, then kelpie done {task})",
        slot.path.display()
    ))
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
    herdr::prompt_agent(&pane, &prompt)?;
    Ok(format!("Asked {task}'s Claude to tidy its commits."))
}

// After land: pushes the rebased task to the default branch and closes it.
pub fn push(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let upstream = default_branch(&repo)?;
    let (ahead, behind) = ahead_behind(&slot.path, &upstream)?;
    if ahead == 0 || behind > 0 || !is_clean(&slot.path)? {
        return Err(format!("{} is not landed; run land first", task_of(&slot)));
    }
    let target = format!("HEAD:{}", upstream.trim_start_matches("origin/"));
    push_as_user(&slot.path, &[&target]).map_err(|error| format!("Pushing {} failed: {error}", task_of(&slot)))?;
    done(cwd, name).map(|closed| format!("Pushed {ahead} commit(s) to {upstream}. {closed}"))
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
    let file = approval_file(state, address);
    std::fs::create_dir_all(file.parent().unwrap_or(state)).map_err(|error| error.to_string())?;
    std::fs::write(file, fingerprint).map_err(|error| error.to_string())
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
    let here = std::env::current_dir().map_err(|error| error.to_string())?;
    if peers::me(&here).is_some_and(|me| me != crate::status::LEAD_LABEL) {
        return Err("approve is the lead's check; a task cannot approve its own commits".to_string());
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
    git(path, &["rebase", "--quiet", "--onto", &new, sha]).map(|_| ())
}

// One commit with the branch's final tree on the default branch, written by the first commit's author.
pub fn squash(path: &Path, message: &str) -> Result<()> {
    let upstream = default_branch(&main_checkout(path)?)?;
    let base = git(path, &["merge-base", "HEAD", &upstream])?;
    let first = git(path, &["rev-list", "--reverse", &format!("{base}..HEAD")])?.lines().next().map(str::to_string)
        .ok_or("Nothing to squash")?;
    let new = commit_like(path, "HEAD^{tree}", &base, &first, message)?;
    git(path, &["reset", "--quiet", "--soft", &new]).map(|_| ())
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
    git(&slot.path, &["switch", "--quiet", "--detach", upstream])?;
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
    Ok(match git(repo, &["merge", "--quiet", "--ff-only", upstream]) {
        Ok(_) => format!("{name} fast-forwarded to {upstream}."),
        Err(_) => format!("{name} could not fast-forward to {upstream}."),
    })
}

pub fn done(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    let workspace = herdr::workspace_for(&slot.path)?;
    let finished = finish(&repo, &slot, &upstream)?;
    forget_session(task_of(&slot), &repo);
    forget_approval(task_of(&slot), &repo);
    end_life(task_of(&slot), &repo, "done");
    Ok(finished + &close_space(workspace))
}

fn finish(repo: &Path, slot: &Slot, upstream: &str) -> Result<String> {
    let task = task_of(slot);
    if !is_clean(&slot.path)? {
        return Err(format!("{task} has uncommitted changes; land or drop them first"));
    }
    let missing = missing_from(&slot.path, upstream)?;
    if !missing.is_empty() {
        return Err(format!("{task} has changes that are not on {upstream} yet; push them first, or use drop:\n{}", missing.join("\n")));
    }
    release(repo, slot, upstream)?;
    forget_start(task, repo);
    Ok(format!("{task} is done; {} is free again. {}", name_of(&slot.path), fast_forward_main(repo, upstream)?))
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
        git(&slot.path, &["reset", "--quiet", "--hard"])?;
        git(&slot.path, &["clean", "-fdq"])?;
    }
    release(&repo, &slot, &upstream)?;
    forget_start(task, &repo);
    forget_session(task, &repo);
    forget_approval(task, &repo);
    end_life(task, &repo, how);
    Ok(format!("{task} dropped; {} is free again.{}", name_of(&slot.path), close_space(workspace)))
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
    let setup_skill = config::resolve(Some(&repo))?.settings.setup_skill.value;
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
        git(&repo, &args)?;
    }
    let stashed = |error: String| -> String {
        if files.is_empty() { error } else { format!("{error}\nThe moved files are kept in the stash \"{message}\" (git stash list).") }
    };

    if existing && current == task {
        git(&repo, &["switch", "--quiet", local]).map_err(stashed)?;
    } else if !existing {
        git(&repo, &["branch", &task]).map_err(stashed)?;
        if ahead_behind(&repo, &upstream)?.0 > 0 {
            git(&repo, &["reset", "--quiet", "--keep", &upstream]).map_err(stashed)?;
        }
    }

    let slot = claim_slot(&repo, &upstream).map_err(stashed)?;
    git(&slot.path, &["switch", "--quiet", &task]).map_err(stashed)?;
    if let Some(index) = stash_index(&repo, &message)? {
        git(&slot.path, &["stash", "pop", "--quiet", &format!("stash@{{{index}}}")]).map_err(stashed)?;
    }

    let space = open_space(&repo, &slot, &task, focus)?;
    if let Some(pane) = pane {
        herdr::move_pane_to_new_tab(pane, &space.workspace_id, "claude", focus)?;
    }
    let main = fast_forward_main(&repo, &upstream)?;
    Ok(format!("Moved {task} to {}. {main}\nSet it up: run the skill {setup_skill} in {}.", slot.path.display(), slot.path.display()))
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
    fn done_drop_and_reset_forget_the_approval() {
        let source = include_str!("tasks.rs");
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("done").contains("forget_approval("));
        assert!(body("discard").contains("forget_approval("));
        assert!(body("reset").contains("discard("));
        assert!(source.contains("\npub fn drop(cwd: &Path, name: Option<&str>, force: bool) -> Result<String> {\n    discard("));
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
        let file = crate::lead::write_settings(&dir, &peers::address("billing", "app"), &settings(&kelpie)).unwrap();
        assert_eq!(file, dir.join("billing@app.settings.json"));
        let args = claude_args(Some("acceptEdits"), &file, &dir.join("billing@app.md"), &Session::New("0b6a".to_string()));
        let line = args.join(" ");
        assert!(!line.contains('{') && line.len() < 512, "{line}");
        assert_eq!(args[..2], ["--permission-mode", "acceptEdits"]);
        assert_eq!(args[args.iter().position(|arg| arg == "--settings").unwrap() + 1], file.to_string_lossy());
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written, serde_json::from_str::<Value>(&settings(&kelpie)).unwrap());
        assert_eq!(written["permissions"]["deny"], json!(["Bash(git push:*)", format!("Bash({kelpie} approve:*)")]), "a task never pushes nor approves its own commits");
        assert_eq!(written["hooks"]["Stop"][0]["hooks"][0]["command"], format!("'{kelpie}' hook"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn start_retries_while_herdr_asks_for_the_prompt_and_stops_on_any_other_refusal() {
        assert!(matches!(refusal("herdr agent start refused: agent pane p_1 is busy (agent_pane_busy)"), Refusal::NotYet));
        assert!(matches!(refusal("herdr agent start refused: input contains a 1100-byte line but the terminal is in canonical mode with a 1024-byte limit; wait for the shell prompt and retry (agent_start_input_failed)"), Refusal::NotYet));
        assert!(matches!(refusal("herdr agent start refused: not ready (agent_not_ready)"), Refusal::Starting));
        assert!(matches!(refusal("herdr agent start refused: agent target pane p_1 not found (agent_pane_not_found)"), Refusal::Final));
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

    fn cleanup(dir: PathBuf, origin: PathBuf, slot: &Slot) {
        git(&dir, &["worktree", "remove", "--force", &slot.path.to_string_lossy()]).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(origin).unwrap();
    }

    #[test]
    fn done_frees_a_pushed_task_whose_branch_has_no_upstream_while_the_local_branch_is_behind() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&dir, &["update-ref", &format!("refs/remotes/{upstream}"), "billing"]).unwrap();
        assert_eq!(ahead_behind(&dir, &upstream).unwrap(), (0, 1));
        finish(&dir, &slot, &upstream).unwrap();
        assert!(!branch_exists(&dir, "billing"));
        assert!(slots(&dir).unwrap().iter().any(|free| free.path == slot.path && free.free));
        assert_eq!(ahead_behind(&dir, &upstream).unwrap(), (0, 0));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn done_refuses_a_task_with_commits_not_on_upstream_and_leaves_it_as_it_was() {
        let (dir, origin, slot, upstream) = task_with_a_commit("search");
        assert!(finish(&dir, &slot, &upstream).is_err());
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
        finish(&dir, &slot, &upstream).unwrap();
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
        finish(&dir, &slot, &upstream).unwrap();
        assert!(!branch_exists(&dir, "billing"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn done_refuses_a_task_with_one_change_missing_from_upstream_and_names_that_commit() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fifth = commit(&slot.path, "e", "fifth");
        let fourth = git(&slot.path, &["rev-parse", "HEAD~1"]).unwrap();
        land_on_main(&dir, &upstream, |dir| pick(dir, &fourth));
        let error = finish(&dir, &slot, &upstream).err().unwrap();
        assert!(error.contains(&fifth[..7]) && error.contains("fifth"), "{error}");
        assert!(!error.contains("fourth") && !error.contains(&fourth[..7]), "{error}");
        assert!(branch_exists(&dir, "billing"));
        assert_eq!(git(&slot.path, &["branch", "--show-current"]).unwrap(), "billing");
        cleanup(dir, origin, &slot);
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
}
