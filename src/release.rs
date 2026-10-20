use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config::Channel;
use crate::sh::Result;
use crate::update_check::git;

pub const OVERRIDE_FILE: &str = "update-override.json";

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Branch(String),
    Path(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Override {
    pub source: Source,
    pub sha: String,
}

impl Override {
    pub fn name(&self) -> String {
        match &self.source {
            Source::Branch(branch) => branch.clone(),
            Source::Path(path) => path.display().to_string(),
        }
    }

    pub fn describe(&self, channel: Channel) -> String {
        format!("running {} @ {}, not the {} channel", self.name(), short(&self.sha), channel.key())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Channel,
    Stable,
    From(String),
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

fn override_file(state: &Path) -> PathBuf {
    state.join(OVERRIDE_FILE)
}

pub fn read_override(state: &Path) -> Option<Override> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(override_file(state)).ok()?).ok()?;
    let sha = value["sha"].as_str()?.to_string();
    let source = match (value["branch"].as_str(), value["path"].as_str()) {
        (Some(branch), None) => Source::Branch(branch.to_string()),
        (None, Some(path)) => Source::Path(PathBuf::from(path)),
        _ => return None,
    };
    Some(Override { source, sha })
}

fn write_override(state: &Path, found: &Override) -> Result<()> {
    let value = match &found.source {
        Source::Branch(branch) => json!({ "branch": branch, "sha": found.sha }),
        Source::Path(path) => json!({ "path": path, "sha": found.sha }),
    };
    crate::state::write(&override_file(state), value.to_string())
}

fn clear_override(state: &Path) {
    let _ = std::fs::remove_file(override_file(state));
}

// Only vX.Y.Z counts as a release, so a stray tag such as v1.0-rc or nightly never becomes one.
pub fn release_version(tag: &str) -> Option<(u64, u64, u64)> {
    let mut parts = tag.strip_prefix('v')?.split('.');
    let mut number = || parts.next().filter(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())).and_then(|part| part.parse().ok());
    let version = (number()?, number()?, number()?);
    parts.next().is_none().then_some(version)
}

pub fn highest(tags: &[&str]) -> Option<String> {
    tags.iter().filter_map(|tag| Some((release_version(tag)?, *tag))).max().map(|(_, tag)| tag.to_string())
}

pub fn latest_tag(root: &Path) -> Result<String> {
    git(root, &["fetch", "--quiet", "--tags", "--force", "origin"])?;
    let tags = git(root, &["tag", "--list", "v*"])?;
    highest(&tags.lines().collect::<Vec<_>>()).ok_or_else(|| "origin has no vX.Y.Z release tag yet".to_string())
}

enum Head {
    Branch(String),
    Detached(String),
}

fn head(root: &Path) -> Result<Head> {
    match git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(branch) => Ok(Head::Branch(branch)),
        Err(_) => git(root, &["rev-parse", "HEAD"]).map(Head::Detached),
    }
}

fn sha_of(root: &Path, reference: &str) -> Result<String> {
    git(root, &["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")])
}

fn clean(dir: &Path, what: &str) -> Result<()> {
    let status = git(dir, &["status", "--porcelain"])?;
    if status.is_empty() {
        return Ok(());
    }
    Err(format!("{} has uncommitted changes, so kelpie update leaves it as it is; {what}", dir.display()))
}

fn at_release(root: &Path) -> bool {
    git(root, &["tag", "--points-at", "HEAD"]).is_ok_and(|tags| tags.lines().any(|tag| release_version(tag).is_some()))
}

fn checkout(root: &Path, sha: &str) -> Result<()> {
    git(root, &["checkout", "--quiet", "--detach", sha]).map(|_| ())
}

// A ref starting with - would reach git as an option.
fn refuse_option(reference: &str) -> Result<()> {
    if reference.starts_with('-') {
        return Err(format!("kelpie update --from {reference}: a branch or path cannot start with -"));
    }
    Ok(())
}

fn source_of(from: &str, root: &Path) -> Result<Source> {
    refuse_option(from)?;
    let path = Path::new(from);
    if path.is_dir() {
        let path = path.canonicalize().map_err(|error| format!("{from}: {error}"))?;
        if root.canonicalize().ok().as_deref() == Some(path.as_path()) {
            return Err(format!("{} is the install root itself; kelpie update builds it without --from", path.display()));
        }
        return Ok(Source::Path(path));
    }
    git(root, &["check-ref-format", "--branch", from]).map_err(|_| format!("kelpie update --from {from}: not a directory, nor a branch name"))?;
    Ok(Source::Branch(from.to_string()))
}

fn fetch(root: &Path, source: &Source) -> Result<String> {
    match source {
        Source::Branch(branch) => {
            git(root, &["fetch", "--quiet", "origin", &format!("refs/heads/{branch}")]).map_err(|_| format!("origin has no branch {branch}"))?;
        }
        Source::Path(path) => {
            git(path, &["rev-parse", "--git-dir"]).map_err(|_| format!("{} is not a git checkout", path.display()))?;
            clean(path, "only commits are installed, so commit them first")?;
            git(root, &["fetch", "--quiet", &path.to_string_lossy(), "HEAD"])?;
        }
    }
    sha_of(root, "FETCH_HEAD")
}

fn install_override(root: &Path, state: &Path, source: Source, channel: Channel) -> Result<String> {
    clean(root, "commit or stash them there, or install from another checkout")?;
    let sha = fetch(root, &source)?;
    checkout(root, &sha)?;
    let found = Override { source, sha };
    write_override(state, &found)?;
    Ok(format!("{}; kelpie update --stable returns to it", found.describe(channel)))
}

fn release(root: &Path, stable: bool) -> Result<String> {
    clean(root, "the release channel checks out tags there, so it must not be a checkout you work in")?;
    let tag = latest_tag(root)?;
    let target = sha_of(root, &tag)?;
    let (place, expected) = match head(root)? {
        Head::Detached(sha) if sha == target => return Ok(format!("at {tag}, the latest release")),
        Head::Detached(sha) => (format!("detached at {}", short(&sha)), at_release(root)),
        Head::Branch(branch) => (format!("on branch {branch}"), false),
    };
    if !expected && !stable {
        return Err(format!("{} is {place}, not on a release tag. The release channel checks out tags there, so it must not be a checkout you develop in. kelpie update --stable moves it to {tag} (a branch keeps its commits), or set [update] channel = \"main\" in the machine config.", root.display()));
    }
    checkout(root, &target)?;
    Ok(format!("checked out {tag}"))
}

fn default_branch(root: &Path) -> Result<String> {
    git(root, &["fetch", "--quiet", "origin"])?;
    let head = git(root, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]).ok()
        .or_else(|| ["origin/main", "origin/master"].into_iter().find(|branch| sha_of(root, branch).is_ok()).map(str::to_string))
        .ok_or_else(|| format!("{} has no origin default branch", root.display()))?;
    Ok(head.trim_start_matches("origin/").to_string())
}

fn back_to_main(root: &Path) -> Result<String> {
    clean(root, "commit or stash them first")?;
    let branch = default_branch(root)?;
    git(root, &["checkout", "--quiet", &branch])?;
    git(root, &["merge", "--quiet", "--ff-only", &format!("origin/{branch}")]).map_err(|_| format!("{branch} cannot fast-forward to origin/{branch}"))?;
    Ok(format!("on {branch} at {}", short(&git(root, &["rev-parse", "HEAD"])?)))
}

// A linked checkout may be where kelpie is being developed, so it only fast-forwards a clean tree.
pub fn pull(root: &Path) -> String {
    let git = |args: &[&str]| git(root, args);
    if !git(&["status", "--porcelain"]).unwrap_or_default().is_empty() {
        return "uncommitted changes in the linked checkout, not pulled".to_string();
    }
    if let Ok(Head::Detached(sha)) = head(root) {
        return format!("detached at {}, not pulled; kelpie update --stable returns to the default branch", short(&sha));
    }
    if git(&["rev-parse", "--abbrev-ref", "@{upstream}"]).is_err() {
        return "the linked checkout has no upstream, not pulled".to_string();
    }
    match git(&["pull", "--ff-only", "--quiet"]) {
        Ok(_) => format!("pulled to {}", git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default()),
        Err(_) => "the linked checkout cannot fast-forward, not pulled".to_string(),
    }
}

pub enum Plan {
    Herdr,
    Done(String),
}

// The git half of kelpie update: what the install root holds afterwards. Herdr means the main channel's GitHub install, which herdr updates itself.
pub fn install(root: &Path, state: &Path, channel: Channel, request: &Request, herdr_installed: bool) -> Result<Plan> {
    let active = read_override(state);
    match (request, active) {
        (Request::From(from), _) => install_override(root, state, source_of(from, root)?, channel).map(Plan::Done),
        (Request::Channel, Some(active)) => install_override(root, state, active.source, channel).map(|line| Plan::Done(format!("{line} (refreshed)"))),
        (request, _) => {
            let stable = *request == Request::Stable;
            let line = match channel {
                Channel::Release => release(root, stable)?,
                Channel::Main if stable => back_to_main(root)?,
                Channel::Main if herdr_installed => return Ok(Plan::Herdr),
                Channel::Main => pull(root),
            };
            if stable {
                clear_override(state);
            }
            Ok(Plan::Done(line))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-release-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn run_git(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", "-c", "core.hooksPath=/dev/null"]).args(args).current_dir(cwd).output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn release_commit(repo: &Path, version: &str) -> String {
        std::fs::create_dir_all(repo.join(".claude-plugin")).unwrap();
        std::fs::write(repo.join(".claude-plugin/plugin.json"), format!(r#"{{"name":"kelpie","version":"{version}"}}"#)).unwrap();
        run_git(repo, &["add", "-A"]);
        run_git(repo, &["commit", "--quiet", "-m", &format!("kelpie {version}")]);
        run_git(repo, &["rev-parse", "HEAD"])
    }

    struct Fixture {
        origin: PathBuf,
        root: PathBuf,
        state: PathBuf,
    }

    fn fixture(name: &str) -> Fixture {
        let dir = scratch(name);
        let origin = dir.join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        run_git(&origin, &["init", "--quiet", "-b", "main"]);
        release_commit(&origin, "0.20.9");
        run_git(&origin, &["tag", "v0.20.9"]);
        run_git(&dir, &["clone", "--quiet", origin.to_str().unwrap(), "root"]);
        Fixture { origin, root: dir.join("root"), state: dir.join("state") }
    }

    fn version_in(root: &Path) -> String {
        let text = std::fs::read_to_string(root.join(".claude-plugin/plugin.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["version"].as_str().unwrap().to_string()
    }

    fn done(plan: Result<Plan>) -> String {
        match plan.unwrap() {
            Plan::Done(line) => line,
            Plan::Herdr => panic!("herdr was asked to update"),
        }
    }

    #[test]
    fn only_vxyz_tags_are_releases_and_they_compare_by_number() {
        assert_eq!(release_version("v0.20.10"), Some((0, 20, 10)));
        for bad in ["0.20.1", "v0.20", "v0.20.1.1", "v0.20.1-rc1", "v0.x.1", "v.1.2", "nightly", "v", "v1..2"] {
            assert_eq!(release_version(bad), None, "{bad}");
        }
        assert_eq!(highest(&["v0.20.9", "v0.20.10", "v0.3.0", "nightly", "v9.0.0-rc1", "v1.0"]).as_deref(), Some("v0.20.10"));
        assert_eq!(highest(&["v0.21.0", "v0.20.99"]).as_deref(), Some("v0.21.0"));
        assert_eq!(highest(&["nightly"]), None);
    }

    #[test]
    fn the_release_channel_checks_out_the_highest_tag_and_ignores_main() {
        let fixture = fixture("highest");
        let ten = release_commit(&fixture.origin, "0.20.10");
        run_git(&fixture.origin, &["tag", "v0.20.10"]);
        release_commit(&fixture.origin, "0.21.0-pre");
        run_git(&fixture.origin, &["tag", "v0.21.0-rc1"]);
        release_commit(&fixture.origin, "0.22.0-dev");
        assert!(done(install(&fixture.root, &fixture.state, Channel::Release, &Request::Stable, false)).contains("checked out v0.20.10"));
        assert_eq!(run_git(&fixture.root, &["rev-parse", "HEAD"]), ten);
        assert_eq!(version_in(&fixture.root), "0.20.10");
        assert_eq!(done(install(&fixture.root, &fixture.state, Channel::Release, &Request::Channel, false)), "at v0.20.10, the latest release");
        let eleven = release_commit(&fixture.origin, "0.20.11");
        run_git(&fixture.origin, &["tag", "v0.20.11"]);
        assert_eq!(done(install(&fixture.root, &fixture.state, Channel::Release, &Request::Channel, false)), "checked out v0.20.11", "from one release tag to the next needs no --stable");
        assert_eq!(run_git(&fixture.root, &["rev-parse", "HEAD"]), eleven);
    }

    #[test]
    fn the_release_channel_refuses_a_development_checkout() {
        let fixture = fixture("refuse");
        let refused = install(&fixture.root, &fixture.state, Channel::Release, &Request::Channel, false).err().unwrap();
        assert!(refused.contains("is on branch main, not on a release tag") && refused.contains("kelpie update --stable moves it to v0.20.9") && refused.contains("channel = \"main\""), "{refused}");
        assert_eq!(run_git(&fixture.root, &["symbolic-ref", "--short", "HEAD"]), "main", "nothing moved");
        std::fs::write(fixture.root.join("work.rs"), "unsaved").unwrap();
        let dirty = install(&fixture.root, &fixture.state, Channel::Release, &Request::Stable, false).err().unwrap();
        assert!(dirty.contains("has uncommitted changes, so kelpie update leaves it as it is"), "{dirty}");
        assert_eq!(std::fs::read_to_string(fixture.root.join("work.rs")).unwrap(), "unsaved");
        std::fs::remove_file(fixture.root.join("work.rs")).unwrap();
        run_git(&fixture.root, &["checkout", "--quiet", "--detach", "HEAD"]);
        std::fs::write(fixture.root.join("x"), "x").unwrap();
        run_git(&fixture.root, &["add", "x"]);
        run_git(&fixture.root, &["commit", "--quiet", "-m", "local"]);
        let off_tag = install(&fixture.root, &fixture.state, Channel::Release, &Request::Channel, false).err().unwrap();
        assert!(off_tag.contains("is detached at") && off_tag.contains("not on a release tag"), "{off_tag}");
    }

    #[test]
    fn the_main_channel_fast_forwards_the_default_branch() {
        let fixture = fixture("main");
        let next = release_commit(&fixture.origin, "0.21.0");
        assert!(done(install(&fixture.root, &fixture.state, Channel::Main, &Request::Channel, false)).starts_with("pulled to"));
        assert_eq!(run_git(&fixture.root, &["rev-parse", "HEAD"]), next);
        assert!(matches!(install(&fixture.root, &fixture.state, Channel::Main, &Request::Channel, true).unwrap(), Plan::Herdr), "a GitHub install on main is herdr's to update, as before");
        std::fs::write(fixture.root.join("work.rs"), "unsaved").unwrap();
        assert_eq!(done(install(&fixture.root, &fixture.state, Channel::Main, &Request::Channel, false)), "uncommitted changes in the linked checkout, not pulled");
    }

    #[test]
    fn from_a_branch_installs_it_records_the_override_and_stable_returns_to_the_channel() {
        let fixture = fixture("branch");
        run_git(&fixture.origin, &["checkout", "--quiet", "-b", "try-this"]);
        let tip = release_commit(&fixture.origin, "0.21.0-try");
        run_git(&fixture.origin, &["checkout", "--quiet", "main"]);
        let line = done(install(&fixture.root, &fixture.state, Channel::Release, &Request::From("try-this".to_string()), false));
        assert_eq!(line, format!("running try-this @ {}, not the release channel; kelpie update --stable returns to it", &tip[..7]));
        assert_eq!(run_git(&fixture.root, &["rev-parse", "HEAD"]), tip);
        assert_eq!(read_override(&fixture.state), Some(Override { source: Source::Branch("try-this".to_string()), sha: tip.clone() }));
        run_git(&fixture.origin, &["checkout", "--quiet", "try-this"]);
        let newer = release_commit(&fixture.origin, "0.21.0-try2");
        run_git(&fixture.origin, &["checkout", "--quiet", "main"]);
        assert!(done(install(&fixture.root, &fixture.state, Channel::Release, &Request::Channel, false)).ends_with("(refreshed)"), "a plain update keeps the override and fetches its branch again");
        assert_eq!(run_git(&fixture.root, &["rev-parse", "HEAD"]), newer);
        assert_eq!(done(install(&fixture.root, &fixture.state, Channel::Release, &Request::Stable, false)), "checked out v0.20.9");
        assert_eq!(read_override(&fixture.state), None);
        assert_eq!(version_in(&fixture.root), "0.20.9");
        let missing = install(&fixture.root, &fixture.state, Channel::Release, &Request::From("no-such-branch".to_string()), false).err().unwrap();
        assert_eq!(missing, "origin has no branch no-such-branch");
        assert_eq!(read_override(&fixture.state), None, "a failed --from records nothing");
    }

    #[test]
    fn from_a_path_installs_its_commits_and_refuses_its_uncommitted_work() {
        let fixture = fixture("path");
        let dev = fixture.origin.parent().unwrap().join("dev");
        run_git(fixture.origin.parent().unwrap(), &["clone", "--quiet", fixture.origin.to_str().unwrap(), "dev"]);
        let mine = release_commit(&dev, "0.21.0-mine");
        let from = Request::From(dev.to_string_lossy().to_string());
        assert!(done(install(&fixture.root, &fixture.state, Channel::Release, &from, false)).starts_with(&format!("running {} @ {}", dev.display(), &mine[..7])));
        assert_eq!(run_git(&fixture.root, &["rev-parse", "HEAD"]), mine, "a commit that exists only in the other checkout");
        assert_eq!(read_override(&fixture.state), Some(Override { source: Source::Path(dev.clone()), sha: mine }));
        std::fs::write(dev.join("half.rs"), "half").unwrap();
        let refused = install(&fixture.root, &fixture.state, Channel::Release, &from, false).err().unwrap();
        assert!(refused.contains("has uncommitted changes") && refused.contains("only commits are installed"), "{refused}");
        let itself = install(&fixture.root, &fixture.state, Channel::Release, &Request::From(fixture.root.to_string_lossy().to_string()), false).err().unwrap();
        assert!(itself.contains("is the install root itself"), "{itself}");
        assert_eq!(done(install(&fixture.root, &fixture.state, Channel::Main, &Request::Stable, false)), format!("on main at {}", &run_git(&fixture.origin, &["rev-parse", "HEAD"])[..7]), "--stable on main returns to the default branch");
        assert_eq!(read_override(&fixture.state), None);
    }

    #[test]
    fn a_ref_that_starts_with_a_dash_is_refused_before_git_sees_it() {
        let fixture = fixture("dash");
        for reference in ["--upload-pack=touch /tmp/pwned", "-b", "--"] {
            let refused = install(&fixture.root, &fixture.state, Channel::Release, &Request::From(reference.to_string()), false).err().unwrap();
            assert_eq!(refused, format!("kelpie update --from {reference}: a branch or path cannot start with -"));
        }
        assert!(install(&fixture.root, &fixture.state, Channel::Release, &Request::From("a..b".to_string()), false).err().unwrap().contains("not a directory, nor a branch name"));
        assert_eq!(read_override(&fixture.state), None);
    }

    #[test]
    fn an_override_file_that_cannot_be_read_is_no_override() {
        let state = scratch("override-file");
        std::fs::write(override_file(&state), "not json").unwrap();
        assert_eq!(read_override(&state), None);
        std::fs::write(override_file(&state), r#"{"branch":"x","path":"/y","sha":"abc"}"#).unwrap();
        assert_eq!(read_override(&state), None);
        std::fs::write(override_file(&state), r#"{"branch":"x","sha":"0123456789"}"#).unwrap();
        assert_eq!(read_override(&state).unwrap().describe(Channel::Main), "running x @ 0123456, not the main channel");
    }
}
