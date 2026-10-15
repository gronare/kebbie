use std::path::{Path, PathBuf};

use crate::sh::{Result, run, succeeds};

pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
}

pub fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    run("git", args, Some(cwd))
}

pub fn main_checkout(cwd: &Path) -> Result<PathBuf> {
    let common_dir = git(cwd, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    Ok(Path::new(&common_dir).parent().ok_or("git common dir has no parent")?.to_path_buf())
}

// origin/HEAD is unset in some older clones.
pub fn default_branch(repo: &Path) -> Result<String> {
    if let Ok(branch) = git(repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        return Ok(branch);
    }
    for candidate in ["origin/main", "origin/master"] {
        if succeeds("git", &["show-ref", "--verify", "--quiet", &format!("refs/remotes/{candidate}")], Some(repo)) {
            return Ok(candidate.to_string());
        }
    }
    Err(format!("Cannot tell {}'s default branch; run: git remote set-head origin -a", repo.display()))
}

pub fn worktrees(repo: &Path) -> Result<Vec<Worktree>> {
    let mut list = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    for line in git(repo, &["worktree", "list", "--porcelain"])?.lines().chain(std::iter::once("")) {
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
            branch = None;
        } else if let Some(value) = line.strip_prefix("branch ") {
            branch = Some(value.trim_start_matches("refs/heads/").to_string());
        } else if line.is_empty()
            && let Some(path) = path.take()
        {
            list.push(Worktree { path, branch: branch.take() });
        }
    }
    Ok(list)
}

pub fn changed_count(path: &Path) -> Result<usize> {
    Ok(git(path, &["status", "--porcelain"])?.lines().count())
}

pub fn is_clean(path: &Path) -> Result<bool> {
    Ok(changed_count(path)? == 0)
}

pub fn ahead_behind(path: &Path, upstream: &str) -> Result<(usize, usize)> {
    let counts = git(path, &["rev-list", "--left-right", "--count", &format!("HEAD...{upstream}")])?;
    let mut numbers = counts.split_whitespace().map(|count| count.parse::<usize>().unwrap_or(0));
    Ok((numbers.next().unwrap_or(0), numbers.next().unwrap_or(0)))
}

pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    succeeds("git", &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")], Some(repo))
}

pub fn name_of(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default()
}

// devbox derives ports and hostnames from the wt<N>.<app> directory name.
pub fn slot_path(repo: &Path, number: usize) -> PathBuf {
    repo.with_file_name(format!("wt{number}.{}", name_of(repo)))
}

pub fn slot_number(repo: &Path, path: &Path) -> Option<usize> {
    let name = name_of(path);
    let digits = name.strip_suffix(&format!(".{}", name_of(repo)))?.strip_prefix("wt")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}
