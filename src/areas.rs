use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::actions::{self, Action, Work};
use crate::repo::{self, default_branch, git, main_checkout_of, name_of};
use crate::sh::Result;
use crate::vault::{self, Index, LogEntry, LogLine, Note};
use crate::vault_cli::KeptLog;

#[derive(Clone)]
pub struct Release {
    pub tag: String,
    pub age: String,
    pub since: usize,
}

#[derive(Clone)]
pub struct RepoState {
    pub branch: String,
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
    pub changed: usize,
    pub release: Option<Release>,
}

#[derive(Clone)]
pub struct Area {
    pub key: String,
    pub stem: String,
    pub summary: String,
    pub repo: Option<PathBuf>,
    pub open: Vec<Note>,
    pub backlog: Vec<Note>,
    pub log: Vec<LogLine>,
    pub git: Option<RepoState>,
    pub actions: Vec<Action>,
    pub errors: Vec<String>,
    pub sandbox: bool,
}

impl Area {
    pub fn repo_name(&self) -> Option<String> {
        self.repo.as_deref().map(name_of)
    }

    pub fn releases(&self) -> bool {
        self.actions.iter().any(|action| matches!(&action.work, Work::Script(script) if script.tag.is_some()))
    }

    // Commits since the last tag, counted only where the repo has a release action.
    pub fn unreleased(&self) -> usize {
        match (&self.git, self.releases()) {
            (Some(RepoState { release: Some(release), .. }), true) => release.since,
            _ => 0,
        }
    }

    pub fn to_push(&self) -> usize {
        self.git.as_ref().map_or(0, |git| git.ahead)
    }
}

pub struct Areas {
    pub list: Vec<Area>,
    pub synced: Option<u64>,
}

// The system note's path, else where the vault's hooks saw the repo, else a checkout named after the area.
fn repo_for(note: &Note, index: &Index, checkouts: &OnceCell<Vec<PathBuf>>) -> Option<PathBuf> {
    let from_paths = note.paths.iter().map(|path| vault::expand(path)).filter(|path| path.exists())
        .find_map(|path| main_checkout_of(if path.is_dir() { &path } else { path.parent()? }));
    from_paths
        .or_else(|| index.known_root(&note.stem).filter(|root| root.exists()).and_then(|root| main_checkout_of(&root)))
        .or_else(|| checkouts.get_or_init(repo::checkouts).iter().find(|path| name_of(path) == note.stem).cloned())
}

fn count(text: &str) -> usize {
    text.trim().parse().unwrap_or(0)
}

// Main against its upstream, and the last tag on main with the commits since; no fetch, the board's fetcher keeps remotes fresh.
fn repo_state(repo: &Path) -> Result<RepoState> {
    let upstream = default_branch(repo)?;
    let branch = upstream.trim_start_matches("origin/").to_string();
    let counts = git(repo, &["rev-list", "--left-right", "--count", &format!("refs/heads/{branch}...{}", crate::repo::tracking(&upstream))])?;
    let mut numbers = counts.split_whitespace().map(count);
    let (ahead, behind) = (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0));
    let release = git(repo, &["describe", "--tags", "--long", &format!("refs/heads/{branch}")]).ok().as_deref().and_then(described).map(|(tag, since)| Release {
        age: git(repo, &["log", "-1", "--format=%cr", &tag]).unwrap_or_default(),
        since,
        tag,
    });
    Ok(RepoState { branch, upstream, ahead, behind, changed: repo::changed_count(repo)?, release })
}

// `describe --long` prints <tag>-<commits since>-g<sha>, and a tag may itself hold dashes.
fn described(text: &str) -> Option<(String, usize)> {
    let mut parts = text.trim().rsplitn(3, '-');
    let (_, since, tag) = (parts.next()?, parts.next()?, parts.next()?);
    Some((tag.to_string(), since.parse().ok()?))
}

#[derive(Clone)]
struct RepoFacts {
    git: Option<RepoState>,
    actions: Vec<Action>,
    errors: Vec<String>,
    sandbox: bool,
}

fn repo_facts(repo: Option<&Path>) -> RepoFacts {
    let file = repo.map(crate::config::repo_file);
    let (actions, errors) = file.as_deref().map(actions::load_from).unwrap_or_default();
    let sandbox = crate::config::resolve_file(file).map_or(true, |loaded| loaded.settings.sandbox.value);
    RepoFacts { git: repo.and_then(|repo| repo_state(repo).ok()), actions, errors, sandbox }
}

fn kept_log(state: Option<&Path>, name: &str, now: std::time::SystemTime, unasked: &mut Vec<String>) -> Option<String> {
    match state.map(|state| crate::vault_cli::kept_log_note(state, name, now)) {
        Some(KeptLog::Known(path)) => Some(path),
        Some(KeptLog::Unasked) => {
            unasked.push(name.to_string());
            None
        }
        _ => None,
    }
}

// The snapshot's template names a repo's own log note, else what the vault said before; until it has said, a guess stands in and the name goes to `unasked`.
fn own_logs(index: &Index, state: Option<&Path>, logs: &[(String, Vec<LogEntry>)], names: [Option<&str>; 2], unasked: &mut Vec<String>) -> Vec<String> {
    let now = std::time::SystemTime::now();
    let mut own: Vec<String> = Vec::new();
    for name in names.into_iter().flatten() {
        let named = index.log_key(name).or_else(|| kept_log(state, name, now, unasked)).or_else(|| vault::own_log_guess(logs, name));
        own.extend(named.filter(|key| !own.contains(key)));
    }
    own
}

pub fn gather() -> Result<Areas> {
    let index = Index::open()?;
    let state = crate::lead::state_dir().ok();
    gather_from(&index, state.as_deref(), repo_facts, |unasked| if let Some(state) = &state { crate::vault_cli::ask_log_notes_later(state, unasked) })
}

// Repo-keyed facts are read once per repo, however many areas share it, and the vault command is never waited on.
fn gather_from(index: &Index, state: Option<&Path>, facts_of: impl Fn(Option<&Path>) -> RepoFacts + Sync, ask: impl FnOnce(Vec<String>)) -> Result<Areas> {
    let logs = index.logs()?;
    let checkouts = OnceCell::new();
    let notes: Vec<(Note, Option<PathBuf>)> = index.areas()?.into_iter().map(|note| {
        let repo = repo_for(&note, index, &checkouts);
        (note, repo)
    }).collect();
    let mut repos: Vec<Option<PathBuf>> = notes.iter().map(|(_, repo)| repo.clone()).collect();
    repos.sort();
    repos.dedup();
    let facts: HashMap<Option<PathBuf>, RepoFacts> = repos.iter().cloned().zip(crate::sh::in_parallel(&repos, 16, |repo| facts_of(repo.as_deref()))).collect();
    let mut unasked = Vec::new();
    let mut list = Vec::new();
    for (note, repo) in notes {
        let (open, backlog): (Vec<Note>, Vec<Note>) = index.notes_of(&note.stem)?.into_iter()
            .filter(|other| other.kind == "task" && ["active", "draft", "backlog"].contains(&other.status.as_str()))
            .partition(|other| other.status != "backlog");
        let RepoFacts { git, actions, errors, sandbox } = facts[&repo].clone();
        list.push(Area {
            sandbox,
            log: vault::log_for(&logs, &note.stem, &own_logs(index, state, &logs, [Some(note.stem.as_str()), repo.as_deref().map(name_of).as_deref()], &mut unasked)),
            git,
            key: note.key,
            stem: note.stem,
            summary: note.summary,
            repo,
            open,
            backlog,
            actions,
            errors,
        });
    }
    unasked.sort();
    unasked.dedup();
    if !unasked.is_empty() {
        ask(unasked);
    }
    Ok(Areas { list, synced: index.last_sync() })
}

// The Areas page as text, for the lead and the command line.
pub fn print() -> Result<String> {
    if let Ok(state) = crate::lead::state_dir() {
        crate::vault_snapshot::refresh_if_older(&state, 60);
    }
    let mut lines = Vec::new();
    for area in gather()?.list {
        let mut facts = Vec::new();
        if let Some(git) = &area.git {
            facts.push(format!("{} ↑{} ↓{}", git.branch, git.ahead, git.behind));
            if let Some(release) = &git.release {
                facts.push(format!("{} +{}", release.tag, release.since));
            }
        }
        facts.push(format!("{} open, {} backlog", area.open.len(), area.backlog.len()));
        if area.repo_name().is_some_and(|repo| crate::permit::repo_permitted(&repo)) {
            facts.push("you may push its reviewed tasks".to_string());
        }
        if !area.sandbox {
            facts.push("task sandbox off".to_string());
        }
        let keys: Vec<String> = area.actions.iter().map(|action| format!("{} {}", action.key, action.title)).collect();
        if !keys.is_empty() {
            facts.push(format!("actions: {}", keys.join(", ")));
        }
        let place = area.repo.as_deref().map_or("no repo".to_string(), |repo| repo.display().to_string());
        lines.push(format!("{}  ({place})\n    {}", area.stem, facts.join(" · ")));
        if let Some(line) = area.log.first() {
            lines.push(format!("    [{}] {}", line.date, line.text));
        }
        lines.extend(area.errors.iter().map(|error| format!("    ! {error}")));
    }
    Ok(lines.join("\n"))
}

// What a task started in the area is told, before what the user typed.
pub fn task_context(area: &Area) -> String {
    let mut text = format!("This task is in the area [[{}]]. Read its system note Areas/{}.md with the vault tools first", area.stem, area.stem);
    if !area.open.is_empty() {
        let open: Vec<&str> = area.open.iter().map(|note| note.key.as_str()).collect();
        text.push_str(&format!(", and its open notes: {}", open.join(", ")));
    }
    text.push('.');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(key: &str) -> Note {
        Note { key: key.to_string(), stem: String::new(), title: String::new(), summary: String::new(), status: "active".to_string(), kind: "task".to_string(), priority: String::new(), paths: Vec::new(), updated: String::new(), area: String::new() }
    }

    fn area() -> Area {
        Area { key: "Areas/kebbie.md".to_string(), stem: "kebbie".to_string(), summary: String::new(), repo: None, open: Vec::new(), backlog: Vec::new(), log: Vec::new(), git: None, actions: Vec::new(), errors: Vec::new(), sandbox: true }
    }

    #[test]
    fn a_task_in_an_area_is_pointed_at_its_notes() {
        let mut area = area();
        assert_eq!(task_context(&area), "This task is in the area [[kebbie]]. Read its system note Areas/kebbie.md with the vault tools first.");
        area.open = vec![note("Projects/kebbie-a.md"), note("Projects/kebbie-b.md")];
        assert!(task_context(&area).ends_with("first, and its open notes: Projects/kebbie-a.md, Projects/kebbie-b.md."));
    }

    #[test]
    fn unreleased_commits_count_only_with_a_release_action() {
        let mut area = area();
        area.git = Some(RepoState { branch: "main".to_string(), upstream: "origin/main".to_string(), ahead: 0, behind: 0, changed: 0,
            release: Some(Release { tag: "v0.5.0".to_string(), age: "10 days ago".to_string(), since: 2 }) });
        assert_eq!(area.unreleased(), 0);
        area.actions = actions::parse("[[actions]]\nkey = \"R\"\ntitle = \"Release\"\nask = \"version\"\nbump = [\"a\"]\ntag = \"v{version}\"\n", "x").0;
        assert_eq!(area.unreleased(), 2);
        assert_eq!(area.to_push(), 0);
        assert_eq!(area.repo_name(), None);
        area.repo = Some(PathBuf::from("/src/app"));
        assert_eq!(area.repo_name().as_deref(), Some("app"));
    }

    #[test]
    fn a_repos_state_is_read_from_its_main_branch_against_origin_and_its_last_tag() {
        let dir = std::env::temp_dir().join(format!("kebbie-areas-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (main, origin) = (dir.join("app"), dir.join("origin.git"));
        std::fs::create_dir_all(&main).unwrap();
        let commit = |message: &str| {
            std::fs::write(main.join(message), message).unwrap();
            git(&main, &["add", message]).unwrap();
            git(&main, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", message]).unwrap();
        };
        git(&main, &["init", "--quiet", "--initial-branch=main"]).unwrap();
        commit("one");
        git(&dir, &["init", "--quiet", "--bare", "--initial-branch=main", origin.to_str().unwrap()]).unwrap();
        git(&main, &["remote", "add", "origin", origin.to_str().unwrap()]).unwrap();
        git(&main, &["push", "--quiet", "origin", "main"]).unwrap();
        assert_eq!(default_branch(&main).unwrap(), "origin/main", "without origin/HEAD the default branch is origin/main");
        git(&main, &["remote", "set-head", "origin", "main"]).unwrap();
        git(&main, &["tag", "v0.1.0"]).unwrap();
        commit("two");
        std::fs::write(main.join("dirty"), "x").unwrap();
        let state = repo_state(&main).unwrap();
        assert_eq!((state.branch.as_str(), state.upstream.as_str(), state.ahead, state.behind, state.changed), ("main", "origin/main", 1, 0, 1));
        let release = state.release.unwrap();
        assert_eq!((release.tag.as_str(), release.since), ("v0.1.0", 1));
        assert!(!release.age.is_empty());
        assert_eq!(described("v1-rc-3-g2d4dfd1"), Some(("v1-rc".to_string(), 3)), "a tag may hold dashes");
        assert_eq!(count(" 3 \n"), 3);
        assert_eq!(count("x"), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_repos_own_log_is_the_snapshots_template_else_the_kept_answer_else_a_guess_while_it_is_asked() {
        let dir = crate::sh::test_dir("areas-own-logs");
        let state = dir.join("state");
        std::fs::create_dir_all(state.join("vault-logs")).unwrap();
        std::fs::write(state.join("vault-logs/api"), "Log/api-changes.md").unwrap();
        std::fs::write(state.join("vault-logs/web"), "").unwrap();
        rusqlite::Connection::open(dir.join("index.sqlite")).unwrap().execute_batch("CREATE TABLE notes (key TEXT);").unwrap();
        let index = Index::at(&dir.join("index.sqlite")).unwrap();
        let logs: Vec<(String, Vec<LogEntry>)> = ["Log/web-logg.md", "Log/app-logg.md"].iter().map(|key| (key.to_string(), Vec::new())).collect();
        let mut unasked = Vec::new();
        assert_eq!(own_logs(&index, Some(&state), &logs, [Some("api"), Some("web")], &mut unasked), ["Log/api-changes.md", "Log/web-logg.md"], "kept, then a refusal guessed");
        assert!(unasked.is_empty(), "a kept refusal is not asked again on every gather");
        assert_eq!(own_logs(&index, Some(&state), &logs, [Some("app"), None], &mut unasked), ["Log/app-logg.md"]);
        assert_eq!(unasked, ["app"]);
        let snapshot = crate::vault_snapshot::parse(r#"{"log_key_template":"Log/{repo}-log.md","notes":[]}"#, None).unwrap();
        let mut none = Vec::new();
        assert_eq!(own_logs(&Index::of_snapshot(std::sync::Arc::new(snapshot), None), Some(&state), &logs, [Some("api"), None], &mut none), ["Log/api-log.md"]);
        assert!(none.is_empty(), "with the snapshot's template nothing is asked");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn many_areas_on_one_repo_read_that_repo_once() {
        let dir = crate::sh::test_dir("areas-once");
        let (shared, other) = (dir.join("shared"), dir.join("other"));
        for repo in [&shared, &other] {
            std::fs::create_dir_all(repo.join("lib")).unwrap();
            git(repo, &["init", "--quiet"]).unwrap();
        }
        let path = dir.join("index.sqlite");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE notes (key TEXT PRIMARY KEY, version TEXT, mtime REAL, folder TEXT, stem TEXT, title TEXT, summary TEXT, status TEXT,
              kind TEXT, area TEXT, tags TEXT, paths TEXT, date TEXT, updated TEXT, superseded_by TEXT, valid INT, error TEXT, size INT, priority TEXT, source TEXT);
            CREATE TABLE repos (repo TEXT PRIMARY KEY, root TEXT, seen_at TEXT);
            CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT);
            CREATE VIRTUAL TABLE notes_fts USING fts5(title, summary, tags, body, stem, stemmed, key UNINDEXED);").unwrap();
        let add = |stem: String, place: Option<PathBuf>| {
            let paths = place.map(|place| format!("[\"{}\"]", place.display()));
            db.execute("INSERT INTO notes (key, folder, stem, title, status, kind, paths, valid) VALUES (?1, 'Areas', ?2, ?2, 'active', 'system', ?3, 1)", rusqlite::params![format!("Areas/{stem}.md"), stem, paths]).unwrap();
        };
        for number in 0..12 {
            add(format!("shared-{number}"), Some(if number % 2 == 0 { shared.clone() } else { shared.join("lib") }));
        }
        for number in 0..3 {
            add(format!("other-{number}"), Some(other.join("lib")));
        }
        add("kebbie-test-area-without-a-repo".to_string(), None);
        drop(db);
        let reads = std::sync::Mutex::new(Vec::new());
        let areas = gather_from(&Index::at(&path).unwrap(), None, |repo| {
            reads.lock().unwrap().push(repo.map(Path::to_path_buf));
            RepoFacts { git: None, actions: Vec::new(), errors: vec![repo.map_or("none".to_string(), name_of)], sandbox: repo.is_none() }
        }, |_| unreachable!("without a state dir nothing is asked")).unwrap();
        let mut reads = reads.into_inner().unwrap();
        reads.sort();
        assert_eq!(reads.len(), 3, "one read per repo and one for the areas without: {reads:?}");
        assert_eq!(areas.list.len(), 16);
        for area in &areas.list {
            let expected = if area.stem.starts_with("shared") { "shared" } else if area.stem.starts_with("other") { "other" } else { "none" };
            assert_eq!(area.errors, [expected], "{} carries its own repo's facts", area.stem);
            assert_eq!(area.sandbox, expected == "none");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
