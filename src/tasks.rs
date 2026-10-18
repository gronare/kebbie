use std::collections::HashSet;
use std::time::Duration;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config;
use crate::herdr;
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
        "permissions": { "deny": ["Bash(git push:*)"], "allow": allow },
        "hooks": { "UserPromptSubmit": hook, "Stop": hook },
        "attribution": { "commit": "", "pr": "" },
        "includeCoAuthoredBy": false,
    }).to_string()
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

// A new pane's shell takes a moment to reach its prompt; until then herdr calls the pane busy.
pub fn start_claude(name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<()> {
    for _ in 0..30 {
        match herdr::start_agent(name, "claude", pane, args) {
            Err(error) if error.contains("agent_pane_busy") => std::thread::sleep(Duration::from_secs(1)),
            Err(error) if error.contains("agent_not_ready") || error.contains("timeout") => return wait_until_ready(name, pane, trusted),
            result => return result,
        }
    }
    Err(format!("The shell in {pane} never became ready for Claude"))
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
    if let Some(existing) = slots(&repo)?.into_iter().find(|slot| slot.task.as_deref() == Some(task.as_str())) {
        let space = open_space(&repo, &existing, &task, focus)?;
        if focus {
            herdr::focus_workspace(&space.workspace_id)?;
        }
        return Ok(format!("{task} is already running in {}.", existing.path.display()));
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
    // herdr's agent names are global, so a task's half in another repo is named with its repo.
    let taken = herdr::agents()?.iter().any(|agent| agent["name"] == task.as_str());
    let agent_name: String = if taken { format!("{task}_{}", slug(&name_of(&repo))?) } else { task.clone() }.chars().take(32).collect();
    let mut agent_args: Vec<&str> = mode.map(|mode| vec!["--permission-mode", mode]).unwrap_or_default();
    let settings = settings(&peers::kelpie_command()?);
    let role = peers::role_file(&task, &peers::Place { repo: &repo, worktree: &slot.path, setup_skill: &setup_skill, rules_skill: rules_skill.as_deref() })?.to_string_lossy().to_string();
    agent_args.extend(["--settings", &settings, "--append-system-prompt-file", &role]);
    let started = start_claude(&agent_name, &claude_pane, &agent_args, trusted_by_user(&repo));
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
    if git(&slot.path, &["rebase", "--quiet", "--autosquash", &upstream]).is_err() {
        git(&slot.path, &["rebase", "--abort"])?;
        let Some(pane) = claude_of(&slot.path) else {
            return Err(format!("{task} conflicts with {upstream} and has no Claude to resolve it; rebase it in {}", slot.path.display()));
        };
        herdr::prompt_agent(&pane, &format!(
            "kelpie tried to land this task and rebasing onto {upstream} conflicts (already fetched). Rebase this branch onto {upstream} \
             with git rebase --autosquash {upstream} (no -i), resolve each conflict keeping the intent of both sides (read the upstream \
             commits that touched the file), run the tests for the files involved, and continue until the rebase is finished. Do not \
             push. Then tell me in a few lines what conflicted and how you resolved it."
        ))?;
        return Err(format!("{task} conflicts with {upstream}; its Claude is resolving the rebase. Push and close again when it reports back."));
    }
    let range = format!("{upstream}..HEAD");
    let mut problems = Vec::new();
    if !git(&slot.path, &["rev-list", "--merges", &range])?.is_empty() {
        problems.push("it contains merge commits");
    }
    if git(&slot.path, &["log", "--format=%B", &range])?.lines().any(|line| line.to_lowercase().starts_with("co-authored-by:")) {
        problems.push("a commit has a Co-Authored-By trailer");
    }
    let commits = git(&slot.path, &["log", "--oneline", &range])?;
    if !problems.is_empty() {
        let found = problems.join("; ");
        let Some(pane) = claude_of(&slot.path) else {
            return Err(format!("{task} is rebased but not ready: {found}.\n{commits}"));
        };
        herdr::prompt_agent(&pane, &format!(
            "kelpie tried to land this task and it is not ready: {found}. Rewrite this branch's history so it lands cleanly, keeping the \
             final tree identical and without git rebase -i: rebase onto {upstream} to drop merge commits, and remove Co-Authored-By lines \
             with git rebase {upstream} --exec 'git log -1 --format=%B | grep -vi \"^co-authored-by:\" | git commit --amend --quiet -F -'. \
             Check that git diff against the old tip is empty. Do not push. Then report ready."
        ))?;
        return Err(format!("{task} is rebased but not ready: {found}. Its Claude is fixing the history; push and close again when it reports back.\n{commits}"));
    }
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
    let mut prompt = format!(
        "Tidy this branch's commits on top of {upstream} for review before it is pushed: one commit per logical change, the functional \
         change before refactors, fixups and review-response commits squashed into the commit they belong to, messages that say why. \
         Interactive git is not available: use git commit --fixup with git rebase --autosquash, git rebase with GIT_SEQUENCE_EDITOR \
         editing the todo, or git reset --soft and recommit. The final tree must be identical: git diff against the old head must be \
         empty. Do not push. Show the commit list before and after."
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
            "Write one git commit message for these commits squashed into one, from what was actually changed. Subject under 72 characters in the \
             imperative or descriptive style the messages use, a blank line, then a short body saying what changed and why, wrapped at 72. Keep \
             ticket and Slack references from the messages. Plain text only: no preamble, no code fences, no trailers."])
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
    end_life(task_of(&slot), &repo, "done");
    Ok(finished + &close_space(workspace))
}

fn finish(repo: &Path, slot: &Slot, upstream: &str) -> Result<String> {
    let task = task_of(slot);
    if !is_clean(&slot.path)? {
        return Err(format!("{task} has uncommitted changes; land or drop them first"));
    }
    if ahead_behind(&slot.path, upstream)?.0 > 0 {
        return Err(format!("{task} has commits that are not on {upstream} yet; push them first, or use drop"));
    }
    release(repo, slot, upstream)?;
    forget_start(task, repo);
    Ok(format!("{task} is done; {} is free again. {}", name_of(&slot.path), fast_forward_main(repo, upstream)?))
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
}
