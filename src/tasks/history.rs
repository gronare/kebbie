use std::path::Path;

use crate::repo::{default_branch, git, git_writing, main_checkout};
use crate::sh::Result;

pub struct Commit {
    pub sha: String,
    pub message: String,
}

// Oldest first, as they will land.
// The task's commits not yet on the default branch, so one already pushed rebased elsewhere is gone from the list before its Claude rebases.
pub fn commits(path: &Path) -> Result<Vec<Commit>> {
    let upstream = default_branch(&main_checkout(path)?)?;
    let open = super::accept::open_commits(path, &upstream)?;
    let log = git(path, &["log", "--reverse", "--format=%H%x00%B%x1e", &format!("{upstream}..HEAD")])?;
    Ok(log.split('\x1e').filter_map(|entry| {
        let (sha, message) = entry.trim_start().split_once('\0')?;
        Some(Commit { sha: sha.to_string(), message: message.trim().to_string() })
    }).filter(|commit| open.iter().any(|open| open.sha == commit.sha)).collect())
}

// A commit with the given tree and parent, written by the author of `like`.
fn commit_like(path: &Path, tree: &str, parent: &str, like: &str, message: &str) -> Result<String> {
    let author = git(path, &["show", "-s", "--format=%an%x00%ae%x00%aI", like])?;
    let mut fields = author.split('\0');
    let (name, email, date) = (fields.next().unwrap_or_default(), fields.next().unwrap_or_default(), fields.next().unwrap_or_default());
    let output = std::process::Command::new("git")
        .args(crate::repo::unhooked(Some(path), &["commit-tree", tree, "-p", parent, "-m", message]))
        .env("GIT_AUTHOR_NAME", name).env("GIT_AUTHOR_EMAIL", email).env("GIT_AUTHOR_DATE", date)
        .current_dir(path).output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("git commit-tree failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// Same tree, parent and author with a new message, then the later commits replayed on top; their trees are unchanged, so nothing can conflict. A commit already on the default branch is never rewritten; a repo without one has none.
pub fn reword(path: &Path, sha: &str, message: &str) -> Result<()> {
    let full = git(path, &["rev-parse", "--verify", "--quiet", &format!("{sha}^{{commit}}")]).map_err(|_| format!("{sha} is not a commit"))?;
    if let Ok(upstream) = main_checkout(path).and_then(|repo| default_branch(&repo)) && !super::accept::open_commits(path, &upstream)?.iter().any(|open| open.sha == full) {
        return Err(format!("{} is not one of the task's commits to push; it is on the default branch already", &full[..7.min(full.len())]));
    }
    let new = commit_like(path, &format!("{sha}^{{tree}}"), &format!("{sha}^"), sha, message)?;
    git_writing(path, &["rebase", "--quiet", "--onto", &new, sha]).map(|_| ())
}

// One commit with the branch's final tree on the commit under the first one not yet on the default branch, written by that commit's author, so a commit already pushed is never rewritten.
pub fn squash(path: &Path, message: &str) -> Result<()> {
    let upstream = default_branch(&main_checkout(path)?)?;
    let first = super::accept::open_commits(path, &upstream)?.into_iter().next().map(|open| open.sha).ok_or("Nothing to squash")?;
    let base = git(path, &["rev-parse", &format!("{first}^")])?;
    let new = commit_like(path, "HEAD^{tree}", &base, &first, message)?;
    git_writing(path, &["reset", "--quiet", "--soft", &new]).map(|_| ())
}

// A one-off Claude with no tools, settings, hooks or MCP, run outside the repo, so it only writes text.
pub fn draft_squash_message(path: &Path) -> Result<String> {
    let upstream = default_branch(&main_checkout(path)?)?;
    let range = format!("{}..HEAD", super::accept::rest_base(path, &upstream).unwrap_or(upstream));
    let input = format!(
        "Commit messages, oldest first:\n\n{}\n\nDiff stat:\n{}\n\nDiff:\n{}",
        git(path, &["log", "--reverse", "--format=%B%n---", &range])?,
        git(path, &["diff", "--stat", &range])?,
        git(path, &["diff", &range])?.chars().take(60_000).collect::<String>(),
    );
    let mut child = std::process::Command::new("claude")
        .args(["-p", "--setting-sources", "", "--strict-mcp-config", "--tools", "", "--model", "sonnet", "--no-session-persistence", "--disable-slash-commands",
            "Write one git commit message for these commits squashed into one, from what was actually changed, in the style the \
             messages use. Keep the references and trailers the messages carry, and add none. Plain text only: no preamble, no code fences."])
        .current_dir(std::env::temp_dir())
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .spawn().map_err(|error| format!("claude: {error}"))?;
    use std::io::Write;
    child.stdin.take().ok_or("claude has no stdin")?.write_all(input.as_bytes()).map_err(|error| error.to_string())?;
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    let message = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || message.is_empty() {
        return Err(format!("Claude could not draft a message: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::test_support::*;
    use crate::repo::git;

    #[test]
    fn reword_changes_one_message_and_keeps_the_tree_and_author() {
        let dir = repo();
        let tree = git(&dir, &["rev-parse", "HEAD^{tree}"]).unwrap();
        let second = git(&dir, &["rev-parse", "HEAD~1"]).unwrap();
        reword(&dir, &second, "second, reworded\n\nwith a body").unwrap();
        assert_eq!(git(&dir, &["log", "--format=%s", "-3"]).unwrap(), "third\nsecond, reworded\nfirst");
        assert_eq!(git(&dir, &["log", "-1", "--format=%b", "HEAD~1"]).unwrap(), "with a body");
        assert_eq!(git(&dir, &["rev-parse", "HEAD^{tree}"]).unwrap(), tree);
        assert_eq!(git(&dir, &["log", "--format=%an", "-3"]).unwrap(), "Ada\nAda\nAda");
        std::fs::remove_dir_all(dir).unwrap();
    }

    // The branch after a push of its first commit rebased elsewhere: main has fourth's change under another sha, the branch still holds fourth and fifth.
    fn pushed_elsewhere() -> (std::path::PathBuf, std::path::PathBuf, crate::tasks::Slot, String) {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        pick(&dir, &fourth);
        git(&dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        (dir, origin, slot, fourth)
    }

    #[test]
    fn squash_and_reword_never_rewrite_a_commit_whose_change_is_on_the_default_branch() {
        let (dir, origin, slot, fourth) = pushed_elsewhere();
        assert!(reword(&slot.path, &fourth, "fourth, reworded").unwrap_err().contains("on the default branch already"));
        commit(&slot.path, "f", "sixth");
        let tree = git(&slot.path, &["rev-parse", "HEAD^{tree}"]).unwrap();
        squash(&slot.path, "fifth and sixth").unwrap();
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD^"]).unwrap(), fourth, "the pushed commit stays as it was, under the squash");
        assert_eq!((git(&slot.path, &["log", "-1", "--format=%s"]).unwrap(), git(&slot.path, &["rev-parse", "HEAD^{tree}"]).unwrap() == tree), ("fifth and sixth".to_string(), true));
        assert_eq!(git(&slot.path, &["diff", "--name-only", &fourth, "HEAD"]).unwrap(), "e\nf");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn squash_makes_one_commit_on_the_default_branch_with_the_same_tree() {
        let dir = repo();
        let origin = dir.with_extension("origin");
        let _ = std::fs::remove_dir_all(&origin);
        git(&dir, &["clone", "--quiet", "--bare", ".", &origin.to_string_lossy()]).unwrap();
        git(&dir, &["remote", "add", "origin", &origin.to_string_lossy()]).unwrap();
        git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
        let branch = git(&dir, &["branch", "--show-current"]).unwrap();
        git(&dir, &["update-ref", &format!("refs/remotes/origin/{branch}"), "HEAD~2"]).unwrap();
        git(&dir, &["symbolic-ref", "refs/remotes/origin/HEAD", &format!("refs/remotes/origin/{branch}")]).unwrap();
        let tree = git(&dir, &["rev-parse", "HEAD^{tree}"]).unwrap();
        squash(&dir, "second and third").unwrap();
        assert_eq!(git(&dir, &["log", "--format=%s", "-2"]).unwrap(), "second and third\nfirst");
        assert_eq!(git(&dir, &["rev-parse", "HEAD^{tree}"]).unwrap(), tree);
        assert_eq!(git(&dir, &["log", "-1", "--format=%an"]).unwrap(), "Ada");
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(origin).unwrap();
    }
}
