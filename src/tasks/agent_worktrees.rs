use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::repo::{git, git_writing};
use super::pool::bytes_in;

struct Agent {
    path: PathBuf,
    branch: Option<String>,
    locked: bool,
}

// A subagent runs inside its parent's process and holds no lock, so a day without a change to its files or its index is the one sign that no agent works in it.
const QUIET: Duration = Duration::from_secs(24 * 60 * 60);

// Claude Code puts an agent's worktree under the main checkout whichever worktree the agent's session ran in, so none of them names its task: one goes only once nothing in it is lost and nothing in it changed for a day.
pub(super) fn clean(repo: &Path, upstream: &str) -> (Vec<(String, u64)>, Vec<String>) {
    clean_at(repo, upstream, SystemTime::now())
}

fn clean_at(repo: &Path, upstream: &str, now: SystemTime) -> (Vec<(String, u64)>, Vec<String>) {
    let (mut removed, mut kept) = (Vec::new(), Vec::new());
    let main = upstream.trim_start_matches("origin/");
    let mut found = agents(repo);
    found.sort_by(|one, other| one.path.cmp(&other.path));
    for agent in found {
        let label = agent.path.strip_prefix(repo).unwrap_or(&agent.path).display().to_string();
        match why_kept(&agent, upstream, main, now) {
            Some(why) => kept.push(format!("{label} ({why})")),
            None => {
                let bytes = bytes_in(&agent.path);
                if let Err(error) = git_writing(repo, &["worktree", "remove", &agent.path.to_string_lossy()]) {
                    kept.push(format!("{label} (git could not remove it: {})", last_line(&error)));
                    continue;
                }
                removed.push((label, bytes));
                if let Some(branch) = &agent.branch
                    && let Err(error) = git(repo, &["branch", "-D", branch]) {
                    kept.push(format!("branch {branch} (git could not delete it: {})", last_line(&error)));
                }
            }
        }
    }
    kept.sort();
    (removed, kept)
}

fn last_line(error: &str) -> &str {
    error.lines().rfind(|line| !line.trim().is_empty()).map_or(error, str::trim)
}

fn agents(repo: &Path) -> Vec<Agent> {
    let home = repo.join(".claude").join("worktrees");
    let listed = git(repo, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    listed.split("\n\n").filter_map(|entry| {
        let path = PathBuf::from(entry.lines().find_map(|line| line.strip_prefix("worktree "))?);
        let branch = entry.lines().find_map(|line| line.strip_prefix("branch refs/heads/")).map(str::to_string);
        let locked = entry.lines().any(|line| line == "locked" || line.starts_with("locked "));
        (path.parent() == Some(home.as_path())).then_some(Agent { path, branch, locked })
    }).collect()
}

fn why_kept(agent: &Agent, upstream: &str, main: &str, now: SystemTime) -> Option<String> {
    if agent.locked {
        return Some("locked".to_string());
    }
    match git(&agent.path, &["--no-optional-locks", "status", "--porcelain"]) {
        Ok(status) if !status.is_empty() => return Some("uncommitted changes".to_string()),
        Err(_) => return Some("git could not read it".to_string()),
        Ok(_) => {}
    }
    match git(&agent.path, &["cherry", &crate::repo::tracking(upstream), "HEAD"]) {
        Ok(cherry) => {
            let missing = cherry.lines().filter(|line| line.starts_with('+')).count();
            if missing > 0 {
                return Some(format!("{} not on {main}", crate::board::widgets::plural(missing, "commit")));
            }
        }
        Err(_) => return Some("git could not compare it with main".to_string()),
    }
    let changed = last_change(&agent.path);
    changed.is_none_or(|changed| now.duration_since(changed).unwrap_or_default() < QUIET).then(|| "changed within the last day".to_string())
}

// The newest change to the worktree's files or to its index, HEAD and reflog in git's own folder for it; a symlink is never followed.
fn last_change(path: &Path) -> Option<SystemTime> {
    let own = git(path, &["rev-parse", "--absolute-git-dir"]).ok().map(PathBuf::from)?;
    let git_files = ["index", "HEAD", "logs/HEAD"].into_iter().filter_map(|file| std::fs::symlink_metadata(own.join(file)).and_then(|meta| meta.modified()).ok());
    newest_in(path).into_iter().chain(git_files).max()
}

fn newest_in(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let own = meta.modified().ok();
    if !meta.is_dir() {
        return own;
    }
    let entries = std::fs::read_dir(path).ok()?;
    entries.flatten().filter_map(|entry| newest_in(&entry.path())).chain(own).max()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::test_support::*;

    fn a_day_on() -> SystemTime {
        SystemTime::now() + QUIET + Duration::from_secs(60)
    }

    #[test]
    fn an_agent_at_work_stays_though_it_holds_no_lock_no_process_and_nothing_off_main() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let working = agent_worktree(&dir, &upstream, "agent-working");
        let (removed, kept) = clean_at(&dir, &upstream, SystemTime::now());
        assert!(removed.is_empty() && working.exists(), "{removed:?}");
        assert_eq!(kept, [".claude/worktrees/agent-working (changed within the last day)"]);
        let (removed, _) = clean_at(&dir, &upstream, a_day_on());
        assert_eq!(removed.len(), 1, "a day later nothing in it has changed, so it goes");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn an_agent_worktree_goes_once_its_work_is_on_main_by_patch_and_one_with_anything_left_stays_named() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let landed = agent_worktree(&dir, &upstream, "agent-landed");
        let picked = commit(&landed, "picked", "picked");
        land_on_main(&dir, &upstream, |dir| pick(dir, &picked));
        let dirty = agent_worktree(&dir, &upstream, "agent-dirty");
        std::fs::write(dirty.join("draft"), "draft").unwrap();
        let ahead = agent_worktree(&dir, &upstream, "agent-ahead");
        commit(&ahead, "unlanded", "unlanded");
        let locked = agent_worktree(&dir, &upstream, "agent-locked");
        git(&dir, &["worktree", "lock", "--reason", "claude agent (pid 1)", &locked.to_string_lossy()]).unwrap();
        let (removed, kept) = clean_at(&dir, &upstream, a_day_on());
        assert_eq!(removed.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(), [".claude/worktrees/agent-landed"], "its commit is on main under another sha after the rebase");
        assert!(!landed.exists() && git(&dir, &["rev-parse", "--verify", "--quiet", "refs/heads/worktree-agent-landed"]).is_err(), "the worktree and its branch are gone");
        let main = upstream.trim_start_matches("origin/");
        assert_eq!(kept, [".claude/worktrees/agent-ahead (1 commit not on MAIN)".replace("MAIN", main), ".claude/worktrees/agent-dirty (uncommitted changes)".to_string(), ".claude/worktrees/agent-locked (locked)".to_string()]);
        assert!(dirty.join("draft").exists() && ahead.exists() && locked.exists());
        git(&dir, &["worktree", "unlock", &locked.to_string_lossy()]).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_worktree_or_branch_git_will_not_remove_is_kept_with_gits_reason() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let stuck = agent_worktree(&dir, &upstream, "agent-stuck");
        untouched_for_two_days(&stuck);
        std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o555)).unwrap();
        let held = agent_worktree(&dir, &upstream, "agent-held");
        untouched_for_two_days(&held);
        std::fs::write(dir.join(".git/refs/heads/worktree-agent-held.lock"), "").unwrap();
        let (removed, kept) = clean_at(&dir, &upstream, a_day_on());
        std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(removed.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(), [".claude/worktrees/agent-held"], "{kept:?}");
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept[1].starts_with("branch worktree-agent-held (git could not delete it: ") && kept[0].len() > 50, "{kept:?}");
        assert!(kept[0].starts_with(".claude/worktrees/agent-stuck (git could not remove it: ") && kept[1].len() > 60, "{kept:?}");
        assert!(stuck.exists());
        std::fs::remove_file(dir.join(".git/refs/heads/worktree-agent-held.lock")).unwrap();
        cleanup(dir, origin, &slot);
    }
}
