use std::path::Path;

use crate::repo::{default_branch, git, git_writing, main_checkout};
use crate::sh::Result;

pub struct Commit {
    pub sha: String,
    pub message: String,
}

// Oldest first, as they will land.
pub fn commits(path: &Path) -> Result<Vec<Commit>> {
    let upstream = default_branch(&main_checkout(path)?)?;
    let log = git(path, &["log", "--reverse", "--format=%H%x00%B%x1e", &format!("{upstream}..HEAD")])?;
    Ok(log.split('\x1e').filter_map(|entry| {
        let (sha, message) = entry.trim_start().split_once('\0')?;
        Some(Commit { sha: sha.to_string(), message: message.trim().to_string() })
    }).collect())
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

// Same tree, parent and author with a new message, then the later commits replayed on top; their trees are unchanged, so nothing can conflict.
pub fn reword(path: &Path, sha: &str, message: &str) -> Result<()> {
    let new = commit_like(path, &format!("{sha}^{{tree}}"), &format!("{sha}^"), sha, message)?;
    git_writing(path, &["rebase", "--quiet", "--onto", &new, sha]).map(|_| ())
}

// One commit with the branch's final tree on the default branch, written by the first commit's author.
pub fn squash(path: &Path, message: &str) -> Result<()> {
    let upstream = default_branch(&main_checkout(path)?)?;
    let base = git(path, &["merge-base", "HEAD", &upstream])?;
    let first = git(path, &["rev-list", "--reverse", &format!("{base}..HEAD")])?.lines().next().map(str::to_string)
        .ok_or("Nothing to squash")?;
    let new = commit_like(path, "HEAD^{tree}", &base, &first, message)?;
    git_writing(path, &["reset", "--quiet", "--soft", &new]).map(|_| ())
}

// A one-off Claude with no tools, settings, hooks or MCP, run outside the repo, so it only writes text.
pub fn draft_squash_message(path: &Path) -> Result<String> {
    let upstream = default_branch(&main_checkout(path)?)?;
    let range = format!("{upstream}..HEAD");
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
