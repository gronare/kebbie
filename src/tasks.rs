use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::herdr;
use crate::repo::{
    ahead_behind, branch_exists, changed_count, default_branch, git, is_clean, main_checkout, name_of, slot_number,
    slot_path, worktrees,
};
use crate::sh::{Result, run_visibly};

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

pub fn slug(name: &str) -> Result<String> {
    let mut slug = String::new();
    for character in name.to_lowercase().chars() {
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
    let script = repo.join("script/set_up_worktree");
    if script.exists() {
        eprintln!("Creating {} with script/set_up_worktree (a couple of minutes the first time)…", name_of(&path));
        run_visibly(&script.to_string_lossy(), &[&format!("wt{number}")], Some(repo))?;
    } else {
        git(repo, &["worktree", "add", "--detach", &path.to_string_lossy(), upstream])?;
    }
    Ok(Slot { number, path, task: None, free: true })
}

// Pushing stays the user's step; the push URL rewrite catches what the deny rule's pattern misses (git -C dir push).
const NO_PUSH: &str = r#"{"env":{"GIT_CONFIG_COUNT":"1","GIT_CONFIG_KEY_0":"url.kelpie-blocks-push:.pushInsteadOf","GIT_CONFIG_VALUE_0":""},"permissions":{"deny":["Bash(git push:*)"]}}"#;

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
    pub dev: bool,
}

// A new pane's shell takes a moment to reach its prompt; until then herdr calls the pane busy.
fn start_claude(name: &str, pane: &str, args: &[&str]) -> Result<()> {
    for _ in 0..30 {
        match herdr::start_agent(name, "claude", pane, args) {
            Err(error) if error.contains("agent_pane_busy") => std::thread::sleep(std::time::Duration::from_secs(1)),
            Err(error) if error.contains("agent_not_ready") => {
                return Err(format!("Claude started in {pane} but is waiting for an answer there (folder trust?); answer it, then send the prompt"));
            }
            result => return result,
        }
    }
    Err(format!("The shell in {pane} never became ready for Claude"))
}

pub fn start(cwd: &Path, name: &str, options: &Start) -> Result<String> {
    let Start { prompt, focus, from, mode, dev } = *options;
    let repo = main_checkout(cwd)?;
    let task = slug(name)?;
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
    let slot = match slots(&repo)?.into_iter().find(|slot| slot.free) {
        Some(slot) => slot,
        None => create_slot(&repo, &upstream)?,
    };
    if branch_exists(&repo, &task) {
        git(&slot.path, &["switch", "--quiet", &task])?;
    } else {
        git(&slot.path, &["switch", "--quiet", "-c", &task, base])?;
    }

    let space = open_space(&repo, &slot, &task, focus)?;
    let claude_pane = match space.root_pane_id {
        Some(pane) => pane,
        None => herdr::create_tab(&space.workspace_id, &slot.path, "claude")?,
    };
    if dev && slot.path.join("dev.yml").exists() {
        herdr::run_in_pane(&herdr::create_tab(&space.workspace_id, &slot.path, "dev")?, "dev")?;
    }
    let agent_name: String = task.chars().take(32).collect();
    let mut agent_args: Vec<&str> = mode.map(|mode| vec!["--permission-mode", mode]).unwrap_or_default();
    agent_args.extend(["--settings", NO_PUSH]);
    start_claude(&agent_name, &claude_pane, &agent_args)?;
    // The prompt goes in as a paste: as an argument, herdr would have to quote it for the shell.
    if let Some(prompt) = prompt
        && let Err(error) = herdr::prompt_agent(&agent_name, prompt)
    {
        return Err(format!("Started {task} in {}, but sending the prompt failed: {error}", slot.path.display()));
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
        return Err(format!("{task} is rebased but not ready: {}.\n{commits}", problems.join("; ")));
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
    run_visibly("git", &["push", "origin", &target], Some(&slot.path))?;
    done(cwd, name).map(|closed| format!("Pushed {ahead} commit(s) to {upstream}. {closed}"))
}

fn free(repo: &Path, slot: &Slot, upstream: &str, delete_flag: &str) -> Result<()> {
    if let Some(workspace) = herdr::workspace_for(&slot.path)? {
        herdr::close_workspace(&workspace)?;
    }
    git(&slot.path, &["switch", "--quiet", "--detach", upstream])?;
    git(repo, &["branch", delete_flag, task_of(slot)]).map(|_| ())
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
    let task = task_of(&slot);
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    if !is_clean(&slot.path)? {
        return Err(format!("{task} has uncommitted changes; land or drop them first"));
    }
    if ahead_behind(&slot.path, &upstream)?.0 > 0 {
        return Err(format!("{task} has commits that are not on {upstream} yet; push them first, or use drop"));
    }
    free(&repo, &slot, &upstream, "-d")?;
    Ok(format!("{task} is done; {} is free again. {}", name_of(&slot.path), fast_forward_main(&repo, &upstream)?))
}

pub fn drop(cwd: &Path, name: Option<&str>, force: bool) -> Result<String> {
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
    if force {
        git(&slot.path, &["reset", "--quiet", "--hard"])?;
        git(&slot.path, &["clean", "-fdq"])?;
    }
    free(&repo, &slot, &upstream, "-D")?;
    Ok(format!("{task} dropped; {} is free again.", name_of(&slot.path)))
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

    // No `dev` here: on a branch behind the default one it dumps the shared database's newer schema into the worktree.
    let space = open_space(&repo, &slot, &task, focus)?;
    if let Some(pane) = pane {
        herdr::move_pane_to_new_tab(pane, &space.workspace_id, "claude", focus)?;
    }
    Ok(format!("Moved {task} to {}. {}", slot.path.display(), fast_forward_main(&repo, &upstream)?))
}
