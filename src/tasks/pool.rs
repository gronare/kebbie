use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config;
use crate::herdr;
use crate::lead::state_dir;
use crate::repo::{ahead_behind, branch_exists, changed_count, default_branch, git, git_writing, is_clean, main_checkout, name_of, setup_line, slot_number, slot_path, worktree_port, worktrees};
use crate::sh::Result;
use super::launch::{clear_tmp, open_space};
use super::{fast_forward_main, fetch, slug};

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

pub(super) fn create_slot(repo: &Path, upstream: &str) -> Result<Slot> {
    let taken: HashSet<usize> = worktrees(repo)?.iter().filter_map(|worktree| slot_number(repo, &worktree.path)).collect();
    let mut number = 1;
    while taken.contains(&number) || slot_path(repo, number).exists() {
        number += 1;
    }
    let path = slot_path(repo, number);
    git(repo, &["worktree", "add", "--detach", &path.to_string_lossy(), upstream])?;
    if let Some(git_dir) = crate::repo::git_dir_of(&path) {
        crate::sandbox::deny_new_slot(&crate::lead::roles_dir()?, &name_of(repo), &git_dir.canonicalize().unwrap_or(git_dir))?;
    }
    Ok(Slot { number, path, task: None, free: true })
}

pub fn list(cwd: &Path) -> Result<Vec<Value>> {
    let repo = main_checkout(cwd)?;
    let upstream = default_branch(&repo)?;
    let template = config::resolve(Some(&repo)).ok().and_then(|loaded| loaded.settings.dev_url.value);
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
            "port": worktree_port(&slot.path),
            "url": config::dev_url(template.as_deref(), &repo, &slot.path),
        }))
    }).collect()
}

// -D because callers have already checked the commits are on upstream; git's -d checks the main checkout's local branch, which is still behind right after a push.
pub(super) fn release(repo: &Path, slot: &Slot, upstream: &str) -> Result<Option<String>> {
    git_writing(&slot.path, &["switch", "--quiet", "--detach", upstream])?;
    clear_tmp(&slot.path);
    git(repo, &["branch", "-D", task_of(slot)])?;
    let folders = config::resolve(Some(repo)).map(|loaded| loaded.settings.clean_on_release.value).unwrap_or_else(|_| vec!["target".to_string()]);
    let removed = clean_build_output(&slot.path, &folders);
    if removed.is_empty() {
        return Ok(None);
    }
    let freed = removed.iter().map(|(folder, bytes)| format!("{folder}/ ({})", size(*bytes))).collect::<Vec<_>>().join(", ");
    Ok(Some(format!("removed {freed}")))
}

// Only output git ignores and tracks nothing in is removed, and target only in a Rust worktree; a symlink is never followed.
pub(super) fn clean_build_output(worktree: &Path, folders: &[String]) -> Vec<(String, u64)> {
    let mut removed = Vec::new();
    for folder in folders {
        let path = worktree.join(folder);
        if !std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) || (folder == "target" && !worktree.join("Cargo.toml").is_file()) {
            continue;
        }
        let tracked = git(worktree, &["ls-files", "--", folder]).map(|files| !files.is_empty()).unwrap_or(true);
        let ignored = crate::sh::succeeds("git", &["check-ignore", "--quiet", &format!("{folder}/")], Some(worktree));
        if tracked || !ignored {
            continue;
        }
        let bytes = bytes_in(&path);
        if std::fs::remove_dir_all(&path).is_ok() {
            removed.push((folder.clone(), bytes));
        }
    }
    removed
}

fn bytes_in(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries.flatten().map(|entry| match entry.metadata() {
        Ok(meta) if meta.is_dir() => bytes_in(&entry.path()),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }).sum()
}

fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn stash_index(repo: &Path, message: &str) -> Result<Option<usize>> {
    Ok(git(repo, &["stash", "list", "--format=%gs"])?.lines().position(|line| line.ends_with(message)))
}

pub(super) fn claim_slot(repo: &Path, upstream: &str) -> Result<Slot> {
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
        return Err(format!("{task} already has a worktree; use kebbie start {task} to go there"));
    }
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    let local = upstream.trim_start_matches("origin/");
    let current = git(&repo, &["branch", "--show-current"])?;
    let existing = branch_exists(&repo, &task);
    let staged = super::stage::load(&state_dir()?, &repo);
    if !existing && !staged.is_empty() {
        let tasks: Vec<String> = staged.into_iter().map(|entry| entry.task).collect();
        return Err(format!("{} on local {local}, and kebbie move would take their commits along; push {local}, or take them off with kebbie unstage, first", super::and_list(&tasks).to_string() + if tasks.len() == 1 { " is staged" } else { " are staged" }));
    }
    if !existing && current != local {
        return Err(format!("The main checkout is on {current}, not {local}; to move that work, use its branch name: kebbie move {current}"));
    }

    let message = format!("kebbie move {task}");
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
        herdr::move_pane_to_new_tab(pane, &space.workspace_id, &task, focus)?;
    }
    let main = fast_forward_main(&repo, &upstream)?;
    Ok(format!("Moved {task} to {}. {main}.\nSet it up: {} in {}{prepare}.", slot.path.display(), config::follow(&setup_skill.value), slot.path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::test_support::*;
    use crate::repo::{git, is_clean, name_of, setup_line, slot_path};
    use crate::tasks::started::started_file_in;

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn a_released_worktree_loses_only_the_listed_build_output_git_ignores_and_tracks_nothing_in() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let worktree = &slot.path;
        std::fs::write(worktree.join(".gitignore"), "target/\nnode_modules/\ndist/\n").unwrap();
        write(&worktree.join("dist/keep.js"), 1);
        git(worktree, &["add", "--force", ".gitignore", "dist/keep.js"]).unwrap();
        git(worktree, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", "dist"]).unwrap();
        write(&worktree.join("target/debug/kebbie"), 3000);
        write(&worktree.join("node_modules/.cache/babel"), 10);
        write(&worktree.join("node_modules/left-pad/index.js"), 1);
        write(&worktree.join("dist/bundle.js"), 5);
        write(&worktree.join("build/out.o"), 5);
        let all = ["target", "node_modules/.cache", "dist", "build"].map(str::to_string);
        assert_eq!(clean_build_output(worktree, &all), [("node_modules/.cache".to_string(), 10)], "target only where there is a Cargo.toml; dist holds a tracked file; build is not ignored");
        assert!(worktree.join("target/debug/kebbie").exists() && worktree.join("dist/bundle.js").exists() && worktree.join("build/out.o").exists() && worktree.join("node_modules/left-pad/index.js").exists());
        std::fs::write(worktree.join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(clean_build_output(worktree, &[]), [], "[] keeps everything");
        assert_eq!(clean_build_output(worktree, &all), [("target".to_string(), 3000)]);
        assert!(!worktree.join("target").exists() && worktree.join("dist/keep.js").exists());
        let outside = dir.with_extension("outside");
        write(&outside.join("precious"), 4);
        std::os::unix::fs::symlink(&outside, worktree.join("target")).unwrap();
        assert_eq!(clean_build_output(worktree, &all), [], "a symlinked target is never followed");
        assert!(outside.join("precious").exists());
        std::fs::remove_dir_all(outside).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn releasing_a_worktree_frees_its_target_by_default_and_says_how_much() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        upstream_commit(&dir, ".gitignore", "target/\n", "ignore target");
        upstream_commit(&dir, "Cargo.toml", "[package]\n", "a Rust repo");
        git(&slot.path, &["rebase", "--quiet", &upstream]).unwrap();
        write(&slot.path.join("target/release/kebbie"), 2_500_000);
        let freed = release(&dir, &slot, &upstream).unwrap();
        assert_eq!(freed.as_deref(), Some("removed target/ (2.5 MB)"));
        assert!(!slot.path.join("target").exists() && slot.path.join("Cargo.toml").exists());
        assert_eq!(release(&dir, &find_free(&dir, &slot), &upstream).ok().flatten(), None, "nothing left to free");
        cleanup(dir, origin, &slot);
    }

    fn find_free(dir: &Path, slot: &Slot) -> Slot {
        git(&slot.path, &["switch", "--quiet", "--create", "again"]).unwrap();
        find_task(dir, Some("again"), dir).unwrap()
    }

    #[test]
    fn sizes_read_in_the_unit_that_fits() {
        assert_eq!([size(999), size(1000), size(2_500_000), size(33_500_000_000)], ["999 B", "1.0 KB", "2.5 MB", "33.5 GB"]);
    }

    #[test]
    fn a_new_slot_is_denied_to_the_sandboxed_tasks_already_running_in_its_repo() {
        let dir = repo();
        let first = create_slot(&dir, "HEAD").unwrap();
        let roles = crate::lead::roles_dir().unwrap();
        let file = roles.join(format!("billing@{}.settings.json", name_of(&dir)));
        std::fs::write(&file, serde_json::json!({ "permissions": { "deny": [] }, "sandbox": { "filesystem": { "denyWrite": [] } } }).to_string()).unwrap();
        let second = create_slot(&dir, "HEAD").unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let git_dir = crate::repo::git_dir_of(&second.path).unwrap().canonicalize().unwrap();
        assert_eq!(written["sandbox"]["filesystem"]["denyWrite"], json!([git_dir.display().to_string()]), "the running task's shell cannot write the new slot's git dir");
        assert_eq!(written["permissions"]["deny"], json!([format!("Edit(/{}/**)", git_dir.display())]));
        std::fs::remove_file(file).unwrap();
        for slot in [first, second] {
            git(&dir, &["worktree", "remove", "--force", &slot.path.to_string_lossy()]).unwrap();
        }
        std::fs::remove_dir_all(dir).unwrap();
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
}
