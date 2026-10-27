use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::sh::{attempt, run};

pub type Runner = dyn FnMut(&[String]) -> std::result::Result<String, String>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Planned,
    InProgress,
    Committed,
    Accepted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub sha: Option<String>,
    pub mark: Mark,
    pub number: usize,
    pub title: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entries {
    pub pushed: usize,
    pub items: Vec<Entry>,
    pub steps: bool,
    pub task: Option<String>,
}

impl Entries {
    pub fn committed(&self) -> impl Iterator<Item = (usize, &Entry)> {
        self.items.iter().enumerate().filter(|(_, entry)| entry.sha.is_some())
    }

    pub fn find(&self, sha: &str) -> Option<usize> {
        self.items.iter().position(|entry| entry.sha.as_deref() == Some(sha))
    }

    pub fn count(&self, mark: Mark) -> usize {
        self.items.iter().filter(|entry| entry.mark == mark).count()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub path: String,
    pub added: String,
    pub deleted: String,
    pub new: bool,
    pub other: bool,
    pub read: bool,
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    run("git", args, Some(root)).ok()
}

fn git_ok(root: &Path, args: &[&str]) -> bool {
    attempt("git", args, Some(root)).is_ok_and(|result| result.is_ok())
}

pub fn toplevel(cwd: &Path) -> Option<PathBuf> {
    git(cwd, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

fn in_linked_worktree(root: &Path) -> bool {
    let dir = git(root, &["rev-parse", "--path-format=absolute", "--git-dir"]);
    dir.is_some() && dir != git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
}

fn default_branch(root: &Path) -> Option<String> {
    crate::repo::default_branch(root).ok()
}

// A worktree's base is under its first commit not yet on the default branch, else its fork point as the board reads it, resolved on every read, so neither a rebase nor a push of its first commits counts what is on the default branch as the task's.
pub fn resolve_base(root: &Path, spec: &str) -> String {
    if spec != "fork" {
        return spec.to_string();
    }
    let fork = in_linked_worktree(root).then(|| default_branch(root)).flatten()
        .and_then(|branch| crate::tasks::rest_base(root, &branch).or_else(|| crate::status::fork_point(root, &branch).ok()));
    fork.unwrap_or_else(|| "HEAD".to_string())
}

pub fn read_text(file: &Path) -> Option<String> {
    fs::read_to_string(file).ok().map(|text| text.trim().to_string()).filter(|text| !text.is_empty())
}

pub fn base_spec(dir: &Path) -> String {
    read_text(&dir.join("base")).unwrap_or_else(|| "fork".to_string())
}

pub fn since(dir: &Path, root: &Path) -> String {
    match base_spec(dir).as_str() {
        "fork" => default_branch(root).map_or_else(|| "since HEAD".to_string(), |branch| format!("since {branch}")),
        base => read_text(&dir.join("label")).unwrap_or_else(|| format!("against {base}")),
    }
}

pub fn branch(root: &Path) -> String {
    git(root, &["branch", "--show-current"]).unwrap_or_default()
}

pub fn kebbie_bin() -> String {
    std::env::var("KEBBIE_BIN").ok().filter(|bin| !bin.is_empty())
        .or_else(|| crate::peers::own_exe().ok().map(|exe| exe.to_string_lossy().to_string()))
        .unwrap_or_else(|| "kebbie".to_string())
}

fn mark_of(state: &str, accepted: bool) -> Mark {
    match (state, accepted) {
        ("planned", _) => Mark::Planned,
        ("in-progress", _) => Mark::InProgress,
        (_, true) => Mark::Accepted,
        _ => Mark::Committed,
    }
}

pub fn from_steps(json: &Value, task: &str) -> Option<Entries> {
    if json["listed"] != true {
        return None;
    }
    let items = json["steps"].as_array()?.iter().map(|step| Entry {
        sha: step["sha"].as_str().map(str::to_string),
        mark: mark_of(step["state"].as_str().unwrap_or_default(), step["accepted"] == true),
        number: step["index"].as_u64().unwrap_or_default() as usize + 1,
        title: step["title"].as_str().unwrap_or_default().to_string(),
    }).collect();
    Some(Entries { pushed: json["pushed"].as_u64().unwrap_or_default() as usize, items, steps: true, task: Some(task.to_string()) })
}

pub fn from_commits(json: &Value, task: &str) -> Option<Entries> {
    let items = json["commits"].as_array()?.iter().enumerate().map(|(index, commit)| Entry {
        sha: commit["sha"].as_str().map(str::to_string),
        mark: if commit["accepted"] == true { Mark::Accepted } else { Mark::Committed },
        number: index + 1,
        title: commit["subject"].as_str().unwrap_or_default().to_string(),
    }).collect();
    Some(Entries { pushed: 0, items, steps: false, task: Some(task.to_string()) })
}

pub type Marks = dyn FnMut(&Path) -> Option<Entries>;

// kebbie's own marks, read in-process: the steps with task.steps on, else the task's unpushed commits.
pub fn kebbie_marks(root: &Path) -> Option<Entries> {
    let task = branch(root);
    crate::tasks::steps_shown_at(root).and_then(|json| from_steps(&json, &task))
        .or_else(|| crate::tasks::accepted_listing_at(root).and_then(|json| from_commits(&json, &task)))
}

// The marks are kebbie's, so an acceptance given or withdrawn elsewhere shows here too.
pub fn read_entries(root: &Path, base: &str, marks: &mut Marks) -> Entries {
    if let Some(entries) = marks(root) {
        return entries;
    }
    let log = git(root, &["log", "--reverse", "--format=%H%x09%s", &format!("{base}..HEAD")]).unwrap_or_default();
    let items = log.lines().filter_map(|line| line.split_once('\t')).enumerate().map(|(index, (sha, subject))| Entry { sha: Some(sha.to_string()), mark: Mark::Committed, number: index + 1, title: subject.to_string() }).collect();
    Entries { items, ..Entries::default() }
}

// What the view's reading stands on: HEAD and the default branch, git status, each changed file's mtime and the folders kebbie keeps its marks in, not the index, which git status itself rewrites; while it stays the same, a refresh reads nothing again.
#[derive(Clone, Debug, PartialEq)]
pub struct Stamp {
    tips: Option<String>,
    status: String,
    mtimes: Vec<Option<std::time::SystemTime>>,
}

impl Stamp {
    pub fn uncommitted(&self) -> bool {
        !self.status.is_empty()
    }
}

fn mtime(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

// The folders kebbie writes acceptances, steps and phases into, each file by a rename, so a folder's mtime moves with any of them.
pub fn kebbie_folders() -> Vec<PathBuf> {
    crate::lead::state_location().map(|state| ["accepted", "steps", "phase"].map(|folder| state.join(folder)).to_vec()).unwrap_or_default()
}

// The paths of git status --porcelain=v2 -z, whose entries never start with a space and end in the path.
fn changed_paths(status: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        let fields = match entry.as_bytes().first() {
            Some(b'1') => 9,
            Some(b'2') => 10,
            Some(b'u') => 11,
            _ => 2,
        };
        if let Some(path) = entry.splitn(fields, ' ').nth(fields - 1) {
            paths.push(path.to_string());
        }
        if entry.starts_with('2') {
            entries.next();
        }
    }
    paths
}

pub fn stamp(root: &Path, branch: Option<&str>, folders: &[PathBuf]) -> Stamp {
    let tips = git(root, &["rev-parse", "HEAD", branch.unwrap_or("HEAD")]);
    let status = git(root, &["status", "--porcelain=v2", "-z"]).unwrap_or_default();
    let changed = changed_paths(&status).into_iter().map(|path| root.join(path));
    let mtimes = folders.iter().cloned().chain(marks_file(root)).chain(changed).map(|path| mtime(&path)).collect();
    Stamp { tips, status, mtimes }
}

pub fn message(root: &Path, sha: &str) -> String {
    git(root, &["log", "-1", "--format=%B", sha]).unwrap_or_default()
}

pub fn short(sha: &str) -> &str {
    &sha[..7.min(sha.len())]
}

fn numstat(root: &Path, range: &[&str]) -> Vec<(String, String, String, bool)> {
    let args = [&["-c", "core.quotePath=false", "diff", "--numstat", "--no-renames"], range].concat();
    git(root, &args).unwrap_or_default().lines().filter_map(|line| {
        let mut fields = line.splitn(3, '\t');
        let (added, deleted, path) = (fields.next()?, fields.next()?, fields.next()?);
        Some((path.to_string(), added.to_string(), deleted.to_string(), false))
    }).collect()
}

fn untracked(root: &Path) -> Vec<(String, String, String, bool)> {
    git(root, &["-c", "core.quotePath=false", "ls-files", "--others", "--exclude-standard"]).unwrap_or_default().lines().map(|path| {
        let lines = fs::read(root.join(path)).map(|bytes| bytes.iter().filter(|byte| **byte == b'\n').count()).unwrap_or_default();
        (path.to_string(), lines.to_string(), String::new(), true)
    }).collect()
}

pub fn changes(root: &Path, base: &str, commit: Option<&str>) -> Vec<(String, String, String, bool)> {
    match commit {
        Some(sha) => numstat(root, &[&format!("{sha}^"), sha]),
        None => [numstat(root, &[base]), untracked(root)].concat(),
    }
}

pub fn marks_file(root: &Path) -> Option<PathBuf> {
    Some(crate::lead::state_location().ok()?.join(root.to_string_lossy().replace('/', "_")))
}

// In a commit a file's contents are its blob there, so the last commit's marks carry over to the whole branch; any edit brings a file back.
pub fn fingerprint(root: &Path, path: &str, commit: Option<&str>) -> String {
    let found = match commit {
        Some(sha) => git(root, &["rev-parse", "--verify", "--quiet", &format!("{sha}:{path}")]),
        None if root.join(path).exists() => git(root, &["hash-object", "--", path]),
        None => None,
    };
    found.unwrap_or_else(|| "deleted".to_string())
}

fn mark_line(root: &Path, path: &str, commit: Option<&str>) -> String {
    format!("{path}\t{}", fingerprint(root, path, commit))
}

fn marks(root: &Path) -> Vec<String> {
    marks_file(root).and_then(|file| fs::read_to_string(file).ok()).unwrap_or_default().lines().map(str::to_string).collect()
}

#[cfg(test)]
pub fn is_read(root: &Path, path: &str, commit: Option<&str>) -> bool {
    let line = mark_line(root, path, commit);
    marks(root).contains(&line)
}

pub fn set_read(root: &Path, path: &str, commit: Option<&str>, read: bool) -> crate::sh::Result<()> {
    let file = marks_file(root).ok_or("kebbie has no state dir for read marks")?;
    let line = mark_line(root, path, commit);
    let mut kept: Vec<String> = marks(root).into_iter().filter(|kept| *kept != line).collect();
    if read {
        kept.push(line);
    }
    crate::state::write(&file, kept.iter().map(|line| format!("{line}\n")).collect::<String>())
}

fn sort_by_mtime(root: &Path, mut paths: Vec<String>) -> Vec<String> {
    paths.sort_by_key(|path| std::cmp::Reverse(fs::metadata(root.join(path)).and_then(|meta| meta.modified()).ok()));
    paths
}

pub fn files(dir: &Path, root: &Path, base: &str, commit: Option<&str>) -> Vec<File> {
    let changes = changes(root, base, commit);
    let scope: Option<Vec<String>> = fs::read_to_string(dir.join("scope")).ok().map(|text| {
        let mut seen = Vec::new();
        for path in text.lines().map(|line| line.trim().trim_start_matches("./").to_string()).filter(|line| !line.is_empty()) {
            if !seen.contains(&path) {
                seen.push(path);
            }
        }
        seen
    });
    let baseline = fs::read_to_string(dir.join("baseline")).ok().filter(|_| commit.is_none() && scope.is_none());
    let order: Vec<(String, bool)> = match (&scope, &baseline) {
        (Some(scope), _) => scope.iter().filter(|path| changes.iter().any(|change| &change.0 == *path)).map(|path| (path.clone(), false))
            .chain(changes.iter().filter(|change| !scope.contains(&change.0)).map(|change| (change.0.clone(), true))).collect(),
        (None, Some(baseline)) => {
            let unchanged = |path: &str| baseline.lines().any(|line| line == format!("{path}\t{}", fingerprint(root, path, None)));
            let new_work: Vec<String> = changes.iter().map(|change| change.0.clone()).filter(|path| !unchanged(path)).collect();
            sort_by_mtime(root, new_work.clone()).into_iter().map(|path| (path, false))
                .chain(changes.iter().filter(|change| !new_work.contains(&change.0)).map(|change| (change.0.clone(), true))).collect()
        }
        (None, None) => changes.iter().map(|change| (change.0.clone(), false)).collect(),
    };
    let read_marks = marks(root);
    order.into_iter().filter_map(|(path, other)| {
        let (_, added, deleted, new) = changes.iter().find(|change| change.0 == path)?.clone();
        let read = read_marks.contains(&mark_line(root, &path, commit));
        Some(File { path, added, deleted, new, other, read })
    }).collect()
}

fn tracked(root: &Path, base: &str, path: &str) -> bool {
    git_ok(root, &["ls-files", "--error-unmatch", "--", path]) || git_ok(root, &["cat-file", "-e", &format!("{base}:{path}")])
}

pub fn diff_argv(root: &Path, base: &str, path: &str, commit: Option<&str>) -> Vec<String> {
    let mut argv: Vec<String> = ["git", "-c", "core.quotePath=false", "diff", "--no-color", "--no-ext-diff"].map(str::to_string).to_vec();
    match commit {
        Some(sha) => argv.extend([format!("{sha}^"), sha.to_string(), "--".into(), path.to_string()]),
        None if tracked(root, base, path) => argv.extend([base.to_string(), "--".into(), path.to_string()]),
        None => argv.extend(["--no-index".into(), "/dev/null".into(), path.to_string()]),
    }
    argv
}

pub fn shown(path: &str, title: &str, commit: Option<&str>, index: usize, count: usize, diff: Vec<String>) -> Value {
    json!({ "path": path, "title": title, "sha": commit.unwrap_or_default(), "short": commit.map(short).unwrap_or_default(), "index": index, "count": count, "diff": diff })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub struct Repo {
        pub root: PathBuf,
        pub slot: PathBuf,
        pub dir: PathBuf,
        pub shas: Vec<String>,
    }

    pub fn sh(dir: &Path, args: &[&str]) -> String {
        run("git", &[&["-c", "user.name=Ada", "-c", "user.email=ada@example.com"], args].concat(), Some(dir)).unwrap()
    }

    pub fn commit(dir: &Path, file: &str, contents: &str, message: &str) -> String {
        if let Some(parent) = dir.join(file).parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(dir.join(file), contents).unwrap();
        sh(dir, &["add", file]);
        sh(dir, &["commit", "--quiet", "-m", message]);
        sh(dir, &["rev-parse", "HEAD"])
    }

    pub fn repo(name: &str) -> Repo {
        let root = crate::sh::test_dir(&format!("review-{name}")).canonicalize().unwrap();
        let (app, origin, slot) = (root.join("app"), root.join("origin.git"), root.join("wt1.app"));
        fs::create_dir_all(&app).unwrap();
        sh(&app, &["init", "--quiet", "--initial-branch=main"]);
        commit(&app, "a", "base\n", "first");
        sh(&root, &["init", "--quiet", "--bare", "--initial-branch=main", &origin.to_string_lossy()]);
        sh(&app, &["remote", "add", "origin", &origin.to_string_lossy()]);
        sh(&app, &["push", "--quiet", "origin", "main"]);
        sh(&app, &["remote", "set-head", "origin", "main"]);
        sh(&app, &["worktree", "add", "--quiet", "-b", "billing", &slot.to_string_lossy()]);
        let shas = vec![
            commit(&slot, "a", "base\none\n", "Change a"),
            commit(&slot, "b", "two\n", "Add b\n\nWhy b exists.\nIt pays."),
            commit(&slot, "c", "three\n", "Add c"),
        ];
        let dir = root.join("reviews/p_claude");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("base"), "fork\n").unwrap();
        fs::write(dir.join("agent"), "p_claude\n").unwrap();
        Repo { root, slot, dir, shas }
    }

    fn paths(files: &[File]) -> Vec<String> {
        files.iter().map(|file| file.path.clone()).collect()
    }

    #[test]
    fn a_commit_lists_its_own_files_and_the_whole_branch_lists_everything_since_the_fork_point() {
        let repo = repo("files");
        let base = resolve_base(&repo.slot, "fork");
        assert_eq!(paths(&files(&repo.dir, &repo.slot, &base, None)), ["a", "b", "c"]);
        assert_eq!(paths(&files(&repo.dir, &repo.slot, &base, Some(&repo.shas[1]))), ["b"]);
        fs::write(repo.slot.join("d"), "x\ny\n").unwrap();
        let all = files(&repo.dir, &repo.slot, &base, None);
        let new = all.iter().find(|file| file.path == "d").unwrap();
        assert!(new.new && new.added == "2", "an untracked file counts its lines: {new:?}");
        assert_eq!(since(&repo.dir, &repo.slot), "since origin/main");
        assert_eq!(resolve_base(&repo.slot, "HEAD~1"), "HEAD~1");
        let _ = fs::remove_dir_all(&repo.root);
    }

    #[test]
    fn the_scope_comes_first_in_its_order_and_the_rest_are_other() {
        let repo = repo("scope");
        fs::write(repo.dir.join("scope"), "./c\nb\nc\n").unwrap();
        let base = resolve_base(&repo.slot, "fork");
        let listed = files(&repo.dir, &repo.slot, &base, None);
        assert_eq!(paths(&listed), ["c", "b", "a"]);
        assert_eq!(listed.iter().map(|file| file.other).collect::<Vec<_>>(), [false, false, true]);
        let _ = fs::remove_dir_all(&repo.root);
    }

    #[test]
    fn a_file_read_in_a_commit_stays_read_in_the_whole_branch_while_its_contents_are_the_same() {
        let repo = repo("read");
        let base = resolve_base(&repo.slot, "fork");
        set_read(&repo.slot, "a", Some(&repo.shas[0]), true).unwrap();
        assert!(is_read(&repo.slot, "a", Some(&repo.shas[0])));
        assert!(is_read(&repo.slot, "a", None), "no later commit changed a");
        assert!(files(&repo.dir, &repo.slot, &base, None)[0].read);
        fs::write(repo.slot.join("a"), "base\none\nedited\n").unwrap();
        assert!(!is_read(&repo.slot, "a", None), "an edit brings it back");
        set_read(&repo.slot, "a", Some(&repo.shas[0]), false).unwrap();
        assert!(!is_read(&repo.slot, "a", Some(&repo.shas[0])));
        let file = marks_file(&repo.slot).unwrap();
        assert!(file.starts_with(crate::lead::state_location().unwrap()), "marks live in kebbie's state dir: {}", file.display());
        let _ = fs::remove_dir_all(&repo.root);
    }

    #[test]
    fn the_diff_is_the_commits_own_or_the_branchs_and_an_untracked_file_is_new() {
        let repo = repo("diff");
        let base = resolve_base(&repo.slot, "fork");
        let shown = |argv: Vec<String>| { let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect(); crate::sh::attempt("git", &args, Some(&repo.slot)).unwrap().unwrap_or_else(|out| out) };
        let own = diff_argv(&repo.slot, &base, "a", Some(&repo.shas[0]));
        assert_eq!(own[own.len() - 4..], [format!("{}^", repo.shas[0]), repo.shas[0].clone(), "--".to_string(), "a".to_string()]);
        assert!(shown(own).contains("+one"));
        commit(&repo.slot, "a", "base\none\nfour\n", "Change a again");
        let whole = shown(diff_argv(&repo.slot, &base, "a", None));
        assert!(whole.contains("+one") && whole.contains("+four"), "{whole}");
        fs::write(repo.slot.join("e"), "fresh\n").unwrap();
        assert!(diff_argv(&repo.slot, &base, "e", None).contains(&"--no-index".to_string()));
        let _ = fs::remove_dir_all(&repo.root);
    }

    #[test]
    fn steps_and_commits_read_kebbies_marks() {
        let steps = json!({ "listed": true, "pushed": 1, "steps": [
            { "index": 1, "title": "Change a", "state": "committed", "sha": "aaa", "accepted": true },
            { "index": 2, "title": "Write docs", "state": "in-progress" },
            { "index": 3, "title": "Release", "state": "planned" },
        ] });
        let entries = from_steps(&steps, "billing").unwrap();
        assert_eq!(entries.pushed, 1);
        assert_eq!(entries.items.iter().map(|entry| (entry.number, entry.mark)).collect::<Vec<_>>(), [(2, Mark::Accepted), (3, Mark::InProgress), (4, Mark::Planned)]);
        assert_eq!(from_steps(&json!({ "listed": false, "steps": [] }), "billing"), None, "a task without steps lists its commits");
        let commits = from_commits(&json!({ "commits": [{ "sha": "aaa", "subject": "One", "accepted": false }, { "sha": "bbb", "subject": "Two", "accepted": true }] }), "billing").unwrap();
        assert_eq!(commits.items.iter().map(|entry| (entry.number, entry.mark)).collect::<Vec<_>>(), [(1, Mark::Committed), (2, Mark::Accepted)]);
        assert_eq!(commits.task.as_deref(), Some("billing"));
    }
}
