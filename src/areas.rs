use std::path::{Path, PathBuf};

use crate::actions::{self, Action, Work};
use crate::repo::{self, default_branch, git, main_checkout, name_of};
use crate::sh::Result;
use crate::vault::{self, Index, LogLine, Note};

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
fn repo_for(note: &Note, index: &Index) -> Option<PathBuf> {
    let from_paths = note.paths.iter().map(|path| vault::expand(path)).filter(|path| path.exists())
        .find_map(|path| main_checkout(if path.is_dir() { &path } else { path.parent()? }).ok());
    from_paths
        .or_else(|| index.known_root(&note.stem).filter(|root| root.exists()).and_then(|root| main_checkout(&root).ok()))
        .or_else(|| repo::checkout_named(&note.stem))
}

fn count(text: &str) -> usize {
    text.trim().parse().unwrap_or(0)
}

// Main against its upstream, and the last tag on main with the commits since; no fetch, the board's fetcher keeps remotes fresh.
fn repo_state(repo: &Path) -> Result<RepoState> {
    let upstream = default_branch(repo)?;
    let branch = upstream.trim_start_matches("origin/").to_string();
    let counts = git(repo, &["rev-list", "--left-right", "--count", &format!("refs/heads/{branch}...{upstream}")])?;
    let mut numbers = counts.split_whitespace().map(count);
    let (ahead, behind) = (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0));
    let release = git(repo, &["describe", "--tags", "--abbrev=0", &format!("refs/heads/{branch}")]).ok().map(|tag| Release {
        age: git(repo, &["log", "-1", "--format=%cr", &tag]).unwrap_or_default(),
        since: git(repo, &["rev-list", "--count", &format!("{tag}..refs/heads/{branch}")]).map(|text| count(&text)).unwrap_or(0),
        tag,
    });
    Ok(RepoState { branch, upstream, ahead, behind, changed: repo::changed_count(repo)?, release })
}

pub fn gather() -> Result<Areas> {
    let index = Index::open()?;
    let logs = index.logs()?;
    let mut list = Vec::new();
    for note in index.areas()? {
        let repo = repo_for(&note, &index);
        let notes = index.notes_of(&note.stem)?;
        let (open, backlog): (Vec<Note>, Vec<Note>) = notes.into_iter()
            .filter(|other| other.kind == "task" && ["active", "draft", "backlog"].contains(&other.status.as_str()))
            .partition(|other| other.status != "backlog");
        let (actions, errors) = repo.as_deref().map(actions::load).unwrap_or_default();
        list.push(Area {
            log: vault::log_for(&logs, &note.stem, repo.as_deref().map(name_of).as_deref()),
            git: repo.as_deref().and_then(|repo| repo_state(repo).ok()),
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
    Ok(Areas { list, synced: index.last_sync() })
}

// The Areas page as text, for the lead and the command line.
pub fn print() -> Result<String> {
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
        Note { key: key.to_string(), stem: String::new(), title: String::new(), summary: String::new(), status: "active".to_string(), kind: "task".to_string(), priority: String::new(), paths: Vec::new(), updated: String::new() }
    }

    fn area() -> Area {
        Area { key: "Areas/kelpie.md".to_string(), stem: "kelpie".to_string(), summary: String::new(), repo: None, open: Vec::new(), backlog: Vec::new(), log: Vec::new(), git: None, actions: Vec::new(), errors: Vec::new() }
    }

    #[test]
    fn a_task_in_an_area_is_pointed_at_its_notes() {
        let mut area = area();
        assert_eq!(task_context(&area), "This task is in the area [[kelpie]]. Read its system note Areas/kelpie.md with the vault tools first.");
        area.open = vec![note("Projects/kelpie-a.md"), note("Projects/kelpie-b.md")];
        assert!(task_context(&area).ends_with("first, and its open notes: Projects/kelpie-a.md, Projects/kelpie-b.md."));
    }

    #[test]
    fn unreleased_commits_count_only_with_a_release_action() {
        let mut area = area();
        area.git = Some(RepoState { branch: "main".to_string(), upstream: "origin/main".to_string(), ahead: 0, behind: 0, changed: 0,
            release: Some(Release { tag: "v0.5.0".to_string(), age: "10 days ago".to_string(), since: 2 }) });
        assert_eq!(area.unreleased(), 0);
        area.actions = actions::parse("[[actions]]\nkey = \"R\"\ntitle = \"Release\"\nask = \"version\"\nbump = [\"a\"]\ntag = \"v{version}\"\n", "x").0;
        assert_eq!(area.unreleased(), 2);
    }
}
