use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::after::{Dep, Known};
use super::pool::slots;
use crate::peers::{self, task_part};
use crate::repo::name_of;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Running {
    pub address: String,
    pub repo: PathBuf,
    pub files: Vec<String>,
    pub prompt: String,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Behind {
    pub dep: Dep,
    pub shared: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Mention {
    Path(String),
    Module(String),
}

const LEADING: &[&str] = &["crate", "self", "super"];

fn mentions(prompt: &str) -> Vec<Mention> {
    let mut found = Vec::new();
    for word in prompt.split(|character: char| character.is_whitespace() || "`'\"()[]{}<>,;!?|*=".contains(character)) {
        let word = word.trim_matches(|character: char| ".:".contains(character));
        let word = word.split_once(':').filter(|(_, line)| !line.starts_with(':') && line.chars().all(|character| character.is_ascii_digit() || character == ':')).map_or(word, |(file, _)| file);
        let mention = if word.contains("::") {
            module_of(word).map(Mention::Module)
        } else if word.contains("://") || word.starts_with('-') {
            None
        } else if word.contains('/') {
            let path = word.trim_start_matches("./").trim_end_matches('/');
            (!path.is_empty() && !path.starts_with('/') && !path.starts_with('~')).then(|| Mention::Path(path.to_string()))
        } else {
            has_extension(word).then(|| Mention::Path(word.to_string()))
        };
        if let Some(mention) = mention.filter(|mention| !found.contains(mention)) {
            found.push(mention);
        }
    }
    found
}

// A git ref such as origin/main or prose such as and/or names no file: a slash word counts when it has an extension or starts in one of the repo's directories, a module when src/ has it.
fn real(mention: &Mention, tracked: &[String]) -> bool {
    match mention {
        Mention::Path(path) if path.contains('/') => {
            let (top, last) = (path.split('/').next().unwrap_or_default(), path.rsplit('/').next().unwrap_or_default());
            has_extension(last) || tracked.iter().any(|file| file.starts_with(&format!("{top}/")))
        }
        Mention::Path(_) => true,
        Mention::Module(module) => tracked.iter().any(|file| file == &format!("src/{module}.rs") || file.starts_with(&format!("src/{module}/"))),
    }
}

fn has_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, ext)| stem.len() > 1 && !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|character| character.is_ascii_alphabetic()))
}

fn real_mentions(prompt: &str, tracked: &[String]) -> Vec<Mention> {
    mentions(prompt).into_iter().filter(|mention| real(mention, tracked)).collect()
}

fn module_of(path: &str) -> Option<String> {
    let root = path.split("::").find(|segment| !LEADING.contains(segment))?;
    (!root.is_empty() && root.chars().all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_')).then(|| root.to_string())
}

// The module a Rust file under src/ belongs to at the crate's top, so src/tasks/after.rs and src/tasks.rs are both tasks.
fn file_module(file: &str) -> Option<String> {
    let inside = file.strip_prefix("src/")?.strip_suffix(".rs")?;
    let root = inside.split('/').next()?;
    (!["main", "lib", "mod"].contains(&root)).then(|| root.to_string())
}

fn path_meets(path: &str, other: &str) -> bool {
    path == other || path.ends_with(&format!("/{other}")) || path.starts_with(&format!("{other}/"))
}

fn touches(mention: &Mention, file: &str) -> bool {
    match mention {
        Mention::Path(path) => path_meets(file, path),
        Mention::Module(module) => file_module(file).as_deref() == Some(module),
    }
}

fn shared_name(mine: &Mention, theirs: &Mention) -> Option<String> {
    match (mine, theirs) {
        (Mention::Path(mine), Mention::Path(theirs)) if path_meets(mine, theirs) => Some(mine.clone()),
        (Mention::Path(mine), Mention::Path(theirs)) if path_meets(theirs, mine) => Some(theirs.clone()),
        (Mention::Module(mine), Mention::Module(theirs)) if mine == theirs => Some(format!("{mine}::")),
        (Mention::Path(path), Mention::Module(module)) | (Mention::Module(module), Mention::Path(path)) if file_module(path).as_deref() == Some(module) => Some(path.clone()),
        _ => None,
    }
}

fn shared(mine: &[Mention], files: &[String], prompt: &str, tracked: &[String]) -> Vec<String> {
    let theirs = real_mentions(prompt, tracked);
    let mut shared: Vec<String> = files.iter().filter(|file| mine.iter().any(|mention| touches(mention, file))).cloned().collect();
    for name in mine.iter().flat_map(|mention| theirs.iter().filter_map(move |other| shared_name(mention, other))) {
        if !shared.iter().any(|known| path_meets(known, &name) || name.strip_suffix("::").is_some_and(|module| file_module(known).as_deref() == Some(module))) {
            shared.push(name);
        }
    }
    shared
}

pub(crate) fn names_files(prompt: &str) -> bool {
    !mentions(prompt).is_empty()
}

// A task started seconds ago has no changes yet, so what its start prompt names counts too.
pub(crate) fn behind(me: &str, prompt: &str, repo: &Path, known: &Known) -> Vec<Behind> {
    let mine = task_part(me);
    if known.open.iter().any(|dep| task_part(&dep.address) == mine) || known.waits.iter().any(|wait| wait.task == mine) {
        return Vec::new();
    }
    let tracked = known.tracked.get(repo).map(Vec::as_slice).unwrap_or_default();
    let mentions = real_mentions(prompt, tracked);
    if mentions.is_empty() {
        return Vec::new();
    }
    let running = known.running.iter().filter(|running| running.repo == repo && task_part(&running.address) != mine)
        .map(|running| (Dep { address: running.address.clone(), repo: running.repo.clone() }, shared(&mentions, &running.files, &running.prompt, tracked)));
    let queued = known.waits.iter().filter(|wait| wait.repo == repo)
        .map(|wait| (Dep { address: wait.address(), repo: wait.repo.clone() }, shared(&mentions, &[], &wait.prompt, tracked)));
    running.chain(queued).filter(|(_, shared)| !shared.is_empty()).map(|(dep, shared)| Behind { dep, shared }).collect()
}

fn listed(names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(3).map(String::as_str).collect();
    let more = names.len().saturating_sub(shown.len());
    format!("{}{}", shown.join(", "), if more > 0 { format!(" and {more} more") } else { String::new() })
}

pub(crate) fn said(behind: &[Behind]) -> String {
    behind.iter().map(|behind| format!("behind {}: both change {}", behind.dep.address, listed(&behind.shared))).collect::<Vec<_>>().join("; ")
}

// The Queued row names the tasks it waits for already, so its reason names only the files.
pub(crate) fn why(behind: &[Behind]) -> String {
    let mut names: Vec<String> = Vec::new();
    for name in behind.iter().flat_map(|behind| behind.shared.iter()) {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    format!("{} {}", if behind.len() == 1 { "both change" } else { "they all change" }, listed(&names))
}

pub(crate) fn running(repos: &[PathBuf]) -> Vec<Running> {
    let mut found = Vec::new();
    for repo in repos {
        let base = crate::repo::default_branch(repo).ok();
        for slot in slots(repo).unwrap_or_default() {
            let Some(task) = slot.task else { continue };
            let files = base.as_deref().map(|base| changed_in(&slot.path, base)).unwrap_or_default();
            let prompt = super::launch::start_prompt(&task, repo).unwrap_or_default();
            found.push(Running { address: peers::address(&task, &name_of(repo)), repo: repo.clone(), files, prompt });
        }
    }
    found
}

pub(crate) fn tracked(repos: &[PathBuf]) -> HashMap<PathBuf, Vec<String>> {
    repos.iter().map(|repo| (repo.clone(), crate::repo::git(repo, &["ls-files"]).unwrap_or_default().lines().map(str::to_string).collect())).collect()
}

fn changed_in(worktree: &Path, base: &str) -> Vec<String> {
    let git = |args: &[&str]| crate::repo::git(worktree, args).unwrap_or_default();
    let fork = git(&["merge-base", &crate::repo::tracking(base), "HEAD"]);
    if fork.is_empty() {
        return Vec::new();
    }
    let mut files: Vec<String> = git(&["diff", "--name-only", "--no-renames", &fork]).lines().chain(git(&["ls-files", "--others", "--exclude-standard"]).lines()).map(str::to_string).collect();
    files.sort();
    files.dedup();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn running(address: &str, files: &[&str], prompt: &str) -> Running {
        Running { address: address.to_string(), repo: PathBuf::from("/repos/kebbie"), files: files.iter().map(|file| file.to_string()).collect(), prompt: prompt.to_string() }
    }

    fn known(running: Vec<Running>) -> Known {
        Known { open: running.iter().map(|running| Dep { address: running.address.clone(), repo: running.repo.clone() }).collect(), waits: Vec::new(), ends: HashMap::new(), running, tracked: HashMap::from([(PathBuf::from("/repos/kebbie"), TREE.map(String::from).to_vec())]) }
    }

    const TREE: [&str; 7] = ["src/status.rs", "src/notify.rs", "src/board/view.rs", "src/tasks/mod.rs", "src/tasks/after.rs", "docs/tutorial.md", "docs/how-to/push.md"];

    fn addresses(found: &[Behind]) -> Vec<&str> {
        found.iter().map(|behind| behind.dep.address.as_str()).collect()
    }

    #[test]
    fn a_prompt_naming_a_file_a_running_task_changes_goes_behind_it() {
        let known = known(vec![running("board-colours@kebbie", &["src/status.rs", "src/board/view.rs"], ""), running("docs-pass@kebbie", &["docs/tutorial.md"], "")]);
        let found = behind("count-faults@kebbie", "Show the count in src/status.rs, next to the CI line.", Path::new("/repos/kebbie"), &known);
        assert_eq!(found, vec![Behind { dep: Dep { address: "board-colours@kebbie".into(), repo: PathBuf::from("/repos/kebbie") }, shared: vec!["src/status.rs".into()] }]);
        assert_eq!(said(&found), "behind board-colours@kebbie: both change src/status.rs");
        assert_eq!(why(&found), "both change src/status.rs");
    }

    #[test]
    fn a_bare_file_name_a_line_a_directory_and_a_module_path_each_name_the_files_under_them() {
        let known = known(vec![running("a@kebbie", &["src/board/view.rs"], ""), running("b@kebbie", &["docs/how-to/push.md"], ""), running("c@kebbie", &["src/tasks/after.rs"], "")]);
        let found = |prompt: &str| addresses(&behind("new@kebbie", prompt, Path::new("/repos/kebbie"), &known)).join(" ");
        assert_eq!(found("Fix the colour in `view.rs`"), "a@kebbie");
        assert_eq!(found("The colour at src/board/view.rs:42 is off"), "a@kebbie");
        assert_eq!(found("Rewrite docs/how-to/ for the new push"), "b@kebbie");
        assert_eq!(found("Call crate::tasks::queue_as from the watcher"), "c@kebbie");
    }

    #[test]
    fn a_task_started_seconds_ago_is_matched_by_what_its_prompt_names() {
        let known = known(vec![running("first@kebbie", &[], "Count the faults in src/status.rs"), running("other@kebbie", &[], "Fix tasks::queue_as")]);
        let found = behind("second@kebbie", "Show the count from status.rs on the board", Path::new("/repos/kebbie"), &known);
        assert_eq!(addresses(&found), ["first@kebbie"]);
        assert_eq!(why(&found), "both change src/status.rs", "the precise path of the two is shown");
        assert_eq!(addresses(&behind("third@kebbie", "Touch src/tasks/after.rs", Path::new("/repos/kebbie"), &known)), ["other@kebbie"], "a path meets a module its prompt names");
    }

    #[test]
    fn a_queued_task_on_the_same_files_is_waited_for_too() {
        let mut known = known(Vec::new());
        known.waits.push(super::super::after::Wait { task: "queued".into(), repo: PathBuf::from("/repos/kebbie"), after: Vec::new(), prompt: "Count faults in src/status.rs".into(), mode: None, profile: None, backlog: None, by: super::super::intent::By::Lead, at: 1, blocked: None, gone_since: None, why: None, starts_at: None });
        let found = behind("next@kebbie", "Show src/status.rs on the board", Path::new("/repos/kebbie"), &known);
        assert_eq!(addresses(&found), ["queued@kebbie"]);
    }

    #[test]
    fn a_prompt_naming_no_file_or_a_file_nobody_works_on_goes_behind_nothing() {
        let known = known(vec![running("a@kebbie", &["src/status.rs"], "Make the board calmer")]);
        for prompt in ["Make the board calmer, e.g. fewer colours.", "Fix src/notify.rs", "See https://example.com/src/status.rs", "Run kebbie start --now"] {
            assert!(behind("new@kebbie", prompt, Path::new("/repos/kebbie"), &known).is_empty(), "{prompt}");
        }
        assert!(!names_files("Make the board calmer, e.g. fewer colours."));
    }

    #[test]
    fn a_git_ref_prose_with_a_slash_or_a_crate_outside_the_repo_names_no_file() {
        let known = known(vec![running("first@kebbie", &[], "Rebase onto origin/main, and/or tidy the CI/CD notes; std::fs reads it, serde_json::Value too")]);
        let behind_it = |prompt: &str| behind("second@kebbie", prompt, Path::new("/repos/kebbie"), &known);
        for prompt in ["The main branch is origin/main", "Pick one and/or the other", "Fix the CI/CD line", "Read it with std::fs::read", "Parse it as serde_json::Value", "Keep upstream/main and refs/heads/main apart"] {
            assert!(behind_it(prompt).is_empty(), "{prompt}");
        }
        assert_eq!(addresses(&behind("third@kebbie", "Rebase onto origin/main and fix docs/how-to/ and crate::tasks", Path::new("/repos/kebbie"), &self::known(vec![running("first@kebbie", &[], "Fix docs/how-to/ on origin/main")]))), ["first@kebbie"], "a directory the repo has still counts");
        assert_eq!(why(&behind("third@kebbie", "Fix docs/how-to/ on origin/main", Path::new("/repos/kebbie"), &self::known(vec![running("first@kebbie", &[], "Fix docs/how-to/ on origin/main")]))), "both change docs/how-to");
    }

    #[test]
    fn only_tasks_in_the_same_repo_count_and_never_the_task_itself() {
        let known = known(vec![running("again@kebbie", &["src/status.rs"], "")]);
        assert!(behind("again@kebbie", "Fix src/status.rs", Path::new("/repos/kebbie"), &known).is_empty(), "a running task restarted under its own name waits for nothing");
        assert!(behind("other@web", "Fix src/status.rs", Path::new("/repos/web"), &known).is_empty());
    }

    #[test]
    fn the_reason_names_at_most_three_files() {
        let found = vec![Behind { dep: Dep { address: "a@kebbie".into(), repo: PathBuf::from("/r") }, shared: ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"].map(String::from).to_vec() }];
        assert_eq!(said(&found), "behind a@kebbie: both change a.rs, b.rs, c.rs and 2 more");
    }

    #[test]
    fn a_task_worktree_reports_its_commits_its_edits_and_its_new_files() {
        let dir = std::env::temp_dir().join(format!("kebbie-overlap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| crate::repo::git(&dir, args).unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("kept.rs"), "a").unwrap();
        std::fs::write(dir.join("edited.rs"), "a").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        std::fs::write(dir.join("committed.rs"), "b").unwrap();
        git(&["add", "committed.rs"]);
        git(&["commit", "-qm", "work"]);
        std::fs::write(dir.join("edited.rs"), "b").unwrap();
        std::fs::write(dir.join("new.rs"), "b").unwrap();
        assert_eq!(changed_in(&dir, "origin/main"), ["committed.rs", "edited.rs", "new.rs"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
