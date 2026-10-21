use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::peers;
use crate::repo::{main_checkout, name_of};
use crate::sh::Result;
use super::pool::{find_task, task_of};

// The backlog item a running task was started from: the Backlog column hides it while the task runs, and a reset brings it back.
pub(super) fn started_file(task: &str, repo: &Path) -> Result<PathBuf> {
    started_file_in(&crate::lead::state_dir()?, task, repo)
}

pub(super) fn started_file_in(state: &Path, task: &str, repo: &Path) -> Result<PathBuf> {
    let dir = state.join("started");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join(peers::address(task, &name_of(repo))))
}

pub fn started_items() -> HashSet<String> {
    let mut items: HashSet<String> = started_links().into_values().flatten().collect();
    items.extend(crate::lead::state_dir().map(|state| super::after::held_items(&state)).unwrap_or_default());
    items
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

pub(super) enum NoteBack {
    Shared,
    Back(String),
    Failed(String),
}

// A note another half still holds stays as it is; the last half puts it back in the backlog.
pub(super) fn note_to_backlog(cli: &crate::vault_cli::Cli, state: &Path, task: &str, address: &str, note: &str) -> NoteBack {
    if crate::task_note::held_elsewhere(state, task, address, note) {
        return NoteBack::Shared;
    }
    match crate::task_note::to_backlog(cli, note) {
        Ok(path) => NoteBack::Back(path),
        Err(error) => NoteBack::Failed(error),
    }
}

pub(super) fn read_start(task: &str, repo: &Path) -> Option<String> {
    started_file(task, repo).ok().and_then(|file| std::fs::read_to_string(file).ok()).map(|item| item.trim().to_string()).filter(|item| !item.is_empty())
}

// What start writes in started/: the note on its own line, the backlog item it came from under it when that is not the note.
pub(super) fn start_entry(note: Option<&str>, backlog: Option<&str>) -> Option<String> {
    let backlog = backlog.filter(|item| Some(*item) != note);
    match (note, backlog) {
        (Some(note), Some(item)) => Some(format!("{note}\n{item}")),
        (Some(only), None) | (None, Some(only)) => Some(only.to_string()),
        (None, None) => None,
    }
}

pub(super) fn record_start(file: &Path, item: &str) -> Result<Option<String>> {
    let previous = std::fs::read_to_string(file).ok().map(|item| item.trim().to_string());
    crate::state::write(file, item)?;
    Ok(previous)
}

pub(super) fn forget_start(task: &str, repo: &Path) -> Option<String> {
    forget_start_in(&crate::lead::state_dir().ok()?, task, repo)
}

pub(super) fn forget_start_in(state: &Path, task: &str, repo: &Path) -> Option<String> {
    let file = started_file_in(state, task, repo).ok()?;
    let item = std::fs::read_to_string(&file).ok();
    let _ = std::fs::remove_file(file);
    item.map(|item| item.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_records_the_item_and_names_the_one_it_replaces() {
        let file = std::env::temp_dir().join(format!("kelpie-link-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        assert_eq!(record_start(&file, "Projects/grown.md").unwrap(), None);
        assert_eq!(record_start(&file, "local:Fix login").unwrap().as_deref(), Some("Projects/grown.md"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "local:Fix login");
        std::fs::remove_file(file).unwrap();
        let source = include_str!("finish.rs");
        let finish = &source[source.find("\npub(super) fn finish(").unwrap()..];
        assert!(finish[..finish.find("\n}\n").unwrap()].contains("done_notice("), "a finished task tells the lead which item it came from");
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
    fn a_start_entry_keeps_the_backlog_item_next_to_the_note() {
        assert_eq!(start_entry(Some("Projects/web-billing.md"), Some("local:Fix login")).as_deref(), Some("Projects/web-billing.md\nlocal:Fix login"), "the local item stays out of the Backlog column and done still has the lead delete its line");
        assert_eq!(start_entry(Some("Projects/web-billing.md"), Some("Projects/web-billing.md")).as_deref(), Some("Projects/web-billing.md"), "a claimed note is stored once");
        assert_eq!(start_entry(None, Some("ABC-12")).as_deref(), Some("ABC-12"));
        assert_eq!(start_entry(None, None), None);
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
}
