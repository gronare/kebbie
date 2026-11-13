use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::sh::{attempt, run};

pub type Runner = dyn FnMut(&[String]) -> std::result::Result<String, String>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Planned,
    InProgress,
    Ready,
    Accepted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub accepted: bool,
}

// A subtask with its commits, or with task.steps off one commit as an entry of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub commits: Vec<Commit>,
    pub mark: Mark,
    pub number: usize,
    pub title: String,
    pub id: String,
    pub contiguous: bool,
}

impl Entry {
    pub fn head(&self) -> Option<&str> {
        self.commits.last().map(|commit| commit.sha.as_str())
    }

    pub fn holds(&self, sha: &str) -> bool {
        self.commits.iter().any(|commit| commit.sha == sha)
    }

    // The subtask's whole diff, from under its first commit to its last, only while no other subtask's commit lies between them.
    pub fn range(&self) -> Option<Range> {
        if !self.contiguous {
            return None;
        }
        Some(Range { from: format!("{}^", self.commits.first()?.sha), to: self.head()?.to_string() })
    }
}

// What a review of a step or a commit compares: from `from` to `to`, both commits; a file's read mark holds for its contents at `to`.
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    pub from: String,
    pub to: String,
}

impl Range {
    pub fn commit(sha: &str) -> Range {
        Range { from: format!("{sha}^"), to: sha.to_string() }
    }
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
        self.items.iter().enumerate().filter(|(_, entry)| !entry.commits.is_empty())
    }

    pub fn numbered(&self, number: usize) -> Option<usize> {
        self.items.iter().position(|entry| entry.number == number && !entry.commits.is_empty())
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
        return git(root, &["rev-parse", "--verify", "--quiet", "--end-of-options", &format!("{spec}^{{commit}}")]).filter(|_| !spec.starts_with('-')).unwrap_or_else(|| "HEAD".to_string());
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

fn mark_of(state: &str) -> Mark {
    match state {
        "planned" => Mark::Planned,
        "in-progress" => Mark::InProgress,
        "accepted" => Mark::Accepted,
        _ => Mark::Ready,
    }
}

fn commit_of(commit: &Value) -> Option<Commit> {
    Some(Commit { sha: commit["sha"].as_str()?.to_string(), subject: commit["subject"].as_str().unwrap_or_default().to_string(), accepted: commit["accepted"] == true })
}

pub fn from_subtasks(json: &Value, task: &str) -> Option<Entries> {
    if json["listed"] != true {
        return None;
    }
    let items = json["subtasks"].as_array()?.iter().map(|subtask| Entry {
        commits: subtask["commits"].as_array().map(|commits| commits.iter().filter_map(commit_of).collect()).unwrap_or_default(),
        mark: mark_of(subtask["state"].as_str().unwrap_or_default()),
        number: subtask["number"].as_u64().unwrap_or_default() as usize,
        title: subtask["title"].as_str().unwrap_or_default().to_string(),
        id: subtask["id"].as_str().unwrap_or_default().to_string(),
        contiguous: subtask["contiguous"] != false,
    }).collect();
    Some(Entries { pushed: json["pushed"].as_u64().unwrap_or_default() as usize, items, steps: true, task: Some(task.to_string()) })
}

pub fn from_commits(json: &Value, task: &str) -> Option<Entries> {
    let items = json["commits"].as_array()?.iter().filter_map(commit_of).enumerate().map(|(index, commit)| Entry {
        mark: if commit.accepted { Mark::Accepted } else { Mark::Ready },
        number: index + 1,
        title: commit.subject.clone(),
        id: (index + 1).to_string(),
        contiguous: true,
        commits: vec![commit],
    }).collect();
    Some(Entries { pushed: 0, items, steps: false, task: Some(task.to_string()) })
}

pub type Marks = dyn FnMut(&Path) -> Option<Entries>;

// kebbie's own marks, read in-process: the subtasks with task.steps on, else the task's unpushed commits.
pub fn kebbie_marks(root: &Path) -> Option<Entries> {
    let task = branch(root);
    crate::tasks::subtasks_shown_at(root).and_then(|json| from_subtasks(&json, &task))
        .or_else(|| crate::tasks::accepted_listing_at(root).and_then(|json| from_commits(&json, &task)))
}

// The marks are kebbie's, so an acceptance given or withdrawn elsewhere shows here too.
pub fn read_entries(root: &Path, base: &str, marks: &mut Marks) -> Entries {
    if let Some(entries) = marks(root) {
        return entries;
    }
    let log = git(root, &["log", "--reverse", "--format=%H%x09%s", &format!("{base}..HEAD")]).unwrap_or_default();
    let items = log.lines().filter_map(|line| line.split_once('\t')).enumerate().map(|(index, (sha, subject))| Entry { commits: vec![Commit { sha: sha.to_string(), subject: subject.to_string(), accepted: false }], mark: Mark::Ready, number: index + 1, title: subject.to_string(), id: (index + 1).to_string(), contiguous: true }).collect();
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

pub fn changes(root: &Path, base: &str, range: Option<&Range>) -> Vec<(String, String, String, bool)> {
    match range {
        Some(range) => numstat(root, &[&range.from, &range.to]),
        None => [numstat(root, &[base]), untracked(root)].concat(),
    }
}

pub fn marks_file(root: &Path) -> Option<PathBuf> {
    Some(crate::lead::state_location().ok()?.join(root.to_string_lossy().replace('/', "_")))
}

// In a commit or a step a file's contents are its blob at the end, so the last commit's marks carry over to the step and the whole branch; any edit brings a file back.
pub fn fingerprint(root: &Path, path: &str, range: Option<&Range>) -> String {
    let found = match range {
        Some(range) => git(root, &["rev-parse", "--verify", "--quiet", &format!("{}:{path}", range.to)]),
        None if root.join(path).exists() => git(root, &["hash-object", "--", path]),
        None => None,
    };
    found.unwrap_or_else(|| "deleted".to_string())
}

fn mark_line(root: &Path, path: &str, range: Option<&Range>) -> String {
    format!("{path}\t{}", fingerprint(root, path, range))
}

fn marks(root: &Path) -> Vec<String> {
    marks_file(root).and_then(|file| fs::read_to_string(file).ok()).unwrap_or_default().lines().map(str::to_string).collect()
}

#[cfg(test)]
pub fn is_read(root: &Path, path: &str, range: Option<&Range>) -> bool {
    let line = mark_line(root, path, range);
    marks(root).contains(&line)
}

// The board's split and the keeper's phone reviews both set marks, so each rewrite holds the marks' lock.
pub fn set_read(root: &Path, path: &str, range: Option<&Range>, read: bool) -> crate::sh::Result<()> {
    let file = marks_file(root).ok_or("kebbie has no state dir for read marks")?;
    let line = mark_line(root, path, range);
    let _locked = crate::state::Locked::wait(&PathBuf::from(format!("{}.lock", file.display())))?;
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

pub fn files(dir: &Path, root: &Path, base: &str, range: Option<&Range>) -> Vec<File> {
    let changes = changes(root, base, range);
    let scope: Option<Vec<String>> = fs::read_to_string(dir.join("scope")).ok().map(|text| {
        let mut seen = Vec::new();
        for path in text.lines().map(|line| line.trim().trim_start_matches("./").to_string()).filter(|line| !line.is_empty()) {
            if !seen.contains(&path) {
                seen.push(path);
            }
        }
        seen
    });
    let baseline = fs::read_to_string(dir.join("baseline")).ok().filter(|_| range.is_none() && scope.is_none());
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
        let read = read_marks.contains(&mark_line(root, &path, range));
        Some(File { path, added, deleted, new, other, read })
    }).collect()
}

fn tracked(root: &Path, base: &str, path: &str) -> bool {
    git_ok(root, &["ls-files", "--error-unmatch", "--", path]) || git_ok(root, &["cat-file", "-e", &format!("{base}:{path}")])
}

pub fn diff_argv(root: &Path, base: &str, path: &str, range: Option<&Range>) -> Vec<String> {
    let mut argv: Vec<String> = ["git", "--no-replace-objects", "-c", "core.quotePath=false", "diff", "--no-color", "--no-ext-diff", "--text", "--no-textconv"].map(str::to_string).to_vec();
    match range {
        Some(range) => argv.extend([range.from.clone(), range.to.clone(), "--".into(), path.to_string()]),
        None if tracked(root, base, path) => argv.extend([base.to_string(), "--".into(), path.to_string()]),
        None => argv.extend(["--no-index".into(), "/dev/null".into(), path.to_string()]),
    }
    argv
}

// A comment typed from a step's diff names the step's last commit, where the lines it shows stand.
pub fn shown(path: &str, title: &str, range: Option<&Range>, index: usize, count: usize, diff: Vec<String>) -> Value {
    let sha = range.map(|range| range.to.as_str());
    json!({ "path": path, "title": title, "sha": sha.unwrap_or_default(), "short": sha.map(short).unwrap_or_default(), "index": index, "count": count, "diff": diff })
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
        assert_eq!(paths(&files(&repo.dir, &repo.slot, &base, Some(&Range::commit(&repo.shas[1])))), ["b"]);
        fs::write(repo.slot.join("d"), "x\ny\n").unwrap();
        let all = files(&repo.dir, &repo.slot, &base, None);
        let new = all.iter().find(|file| file.path == "d").unwrap();
        assert!(new.new && new.added == "2", "an untracked file counts its lines: {new:?}");
        assert_eq!(since(&repo.dir, &repo.slot), "since origin/main");
        assert_eq!(resolve_base(&repo.slot, "HEAD~1"), sh(&repo.slot, &["rev-parse", "HEAD~1"]), "a base the agent names is read as a commit");
        let target = repo.root.join("truncated");
        for option in [format!("--output={}", target.display()), "--no-index".to_string(), "no-such-ref".to_string()] {
            let base = resolve_base(&repo.slot, &option);
            assert_eq!(base, "HEAD", "{option} is no commit");
            files(&repo.dir, &repo.slot, &base, None);
            read_entries(&repo.slot, &base, &mut |_| None);
        }
        assert!(!target.exists(), "a base never reaches git as an option");
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
        set_read(&repo.slot, "a", Some(&Range::commit(&repo.shas[0])), true).unwrap();
        assert!(is_read(&repo.slot, "a", Some(&Range::commit(&repo.shas[0]))));
        assert!(is_read(&repo.slot, "a", None), "no later commit changed a");
        assert!(files(&repo.dir, &repo.slot, &base, None)[0].read);
        fs::write(repo.slot.join("a"), "base\none\nedited\n").unwrap();
        assert!(!is_read(&repo.slot, "a", None), "an edit brings it back");
        set_read(&repo.slot, "a", Some(&Range::commit(&repo.shas[0])), false).unwrap();
        assert!(!is_read(&repo.slot, "a", Some(&Range::commit(&repo.shas[0]))));
        let file = marks_file(&repo.slot).unwrap();
        assert!(file.starts_with(crate::lead::state_location().unwrap()), "marks live in kebbie's state dir: {}", file.display());
        let _ = fs::remove_dir_all(&repo.root);
    }

    #[test]
    fn marks_set_at_once_from_the_board_and_the_phone_are_all_kept() {
        let repo = repo("read-race");
        let sha = repo.shas[0].clone();
        std::thread::scope(|scope| {
            for n in 0..8 {
                let (slot, sha) = (&repo.slot, &sha);
                scope.spawn(move || set_read(slot, &format!("p{n}"), Some(&Range::commit(sha)), true).unwrap());
            }
        });
        assert!((0..8).all(|n| is_read(&repo.slot, &format!("p{n}"), Some(&Range::commit(&sha)))), "{:?}", marks(&repo.slot));
        let _ = fs::remove_dir_all(&repo.root);
    }

    #[test]
    fn the_diff_is_the_commits_own_or_the_branchs_and_an_untracked_file_is_new() {
        let repo = repo("diff");
        let base = resolve_base(&repo.slot, "fork");
        let shown = |argv: Vec<String>| { let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect(); crate::sh::attempt("git", &args, Some(&repo.slot)).unwrap().unwrap_or_else(|out| out) };
        let own = diff_argv(&repo.slot, &base, "a", Some(&Range::commit(&repo.shas[0])));
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
    fn the_diff_shows_the_stored_contents_whatever_the_attributes_or_replace_refs_say() {
        let repo = repo("diff-stored");
        let base = resolve_base(&repo.slot, "fork");
        let shown = |argv: Vec<String>| String::from_utf8_lossy(&std::process::Command::new(&argv[0]).args(&argv[1..]).current_dir(&repo.slot).output().unwrap().stdout).to_string();
        fs::write(repo.root.join("app/.git/info/attributes"), "* -diff\n").unwrap();
        let forged_tree = sh(&repo.slot, &["rev-parse", &format!("{}^^{{tree}}", repo.shas[0])]);
        let forged = sh(&repo.slot, &["commit-tree", &forged_tree, "-p", &format!("{}^", repo.shas[0]), "-m", "Change a"]);
        sh(&repo.slot, &["replace", &repo.shas[0], &forged]);
        let own = shown(diff_argv(&repo.slot, &base, "a", Some(&Range::commit(&repo.shas[0]))));
        assert!(own.contains("+one"), "{own}");
        let whole = shown(diff_argv(&repo.slot, &base, "a", None));
        assert!(whole.contains("+one"), "{whole}");
        let _ = fs::remove_dir_all(&repo.root);
    }

    #[test]
    fn subtasks_and_commits_read_kebbies_marks() {
        let subtasks = json!({ "listed": true, "pushed": 1, "subtasks": [
            { "id": "change-a", "number": 2, "title": "Change a", "state": "accepted", "contiguous": true, "commits": [{ "sha": "aaa", "subject": "One", "accepted": true }, { "sha": "bbb", "subject": "Two", "accepted": true }] },
            { "id": "docs", "number": 3, "title": "Write docs", "state": "in-progress", "contiguous": false, "commits": [{ "sha": "ccc", "subject": "Three", "accepted": false }, { "sha": "eee", "subject": "Five", "accepted": false }] },
            { "id": "release", "number": 4, "title": "Release", "state": "planned", "contiguous": true, "commits": [] },
        ] });
        let entries = from_subtasks(&subtasks, "billing").unwrap();
        assert_eq!(entries.pushed, 1);
        assert_eq!(entries.items.iter().map(|entry| (entry.number, entry.id.as_str(), entry.mark, entry.commits.len())).collect::<Vec<_>>(), [(2, "change-a", Mark::Accepted, 2), (3, "docs", Mark::InProgress, 2), (4, "release", Mark::Planned, 0)]);
        assert_eq!((entries.items[0].holds("bbb"), entries.items[0].head(), entries.items[0].range()), (true, Some("bbb"), Some(Range { from: "aaa^".to_string(), to: "bbb".to_string() })), "a subtask's diff runs from under its first commit to its last");
        assert_eq!(entries.items[1].range(), None, "never across another subtask's commits");
        assert_eq!(from_subtasks(&json!({ "listed": false, "subtasks": [] }), "billing"), None, "a task without subtasks lists its commits");
        let commits = from_commits(&json!({ "commits": [{ "sha": "aaa", "subject": "One", "accepted": false }, { "sha": "bbb", "subject": "Two", "accepted": true }] }), "billing").unwrap();
        assert_eq!(commits.items.iter().map(|entry| (entry.number, entry.mark, entry.commits.len())).collect::<Vec<_>>(), [(1, Mark::Ready, 1), (2, Mark::Accepted, 1)]);
        assert_eq!(commits.task.as_deref(), Some("billing"));
    }

    #[test]
    fn a_steps_files_and_diff_span_its_commits_and_its_read_marks_hold_at_its_last() {
        let repo = repo("step-range");
        let step = Range { from: format!("{}^", repo.shas[1]), to: repo.shas[2].clone() };
        assert_eq!(paths(&files(&repo.dir, &repo.slot, "HEAD", Some(&step))), ["b", "c"]);
        let argv = diff_argv(&repo.slot, "HEAD", "b", Some(&step));
        assert_eq!(argv[argv.len() - 4..], [format!("{}^", repo.shas[1]), repo.shas[2].clone(), "--".to_string(), "b".to_string()]);
        set_read(&repo.slot, "b", Some(&step), true).unwrap();
        assert!(is_read(&repo.slot, "b", Some(&Range::commit(&repo.shas[1]))), "b is the same in its own commit and at the step's end");
        assert_eq!(shown("b", "step 2", Some(&step), 1, 2, Vec::new())["sha"], json!(repo.shas[2]));
        let _ = fs::remove_dir_all(&repo.root);
    }
}
