use std::path::{Path, PathBuf};

use crate::config;
use crate::lead::state_dir;
use crate::peers;
use crate::repo::{ahead_behind, default_branch, git, main_checkout, name_of};
use crate::sh::Result;
use super::pool::{find_task, task_of};

// The change rev makes where it left upstream, without context or line numbers, so a clean rebase, a reworded message or a squash keeps it.
fn code(path: &Path, upstream: &str, rev: &str) -> Result<String> {
    let base = git(path, &["merge-base", rev, upstream])?;
    let diff = git(path, &["diff", "-U0", "--no-color", "--no-ext-diff", &base, rev])?;
    let lines: Vec<&str> = diff.lines().filter(|line| !(line.starts_with("index ") || line.starts_with("@@"))).collect();
    let mut hasher = Fnv::default();
    hasher.write(lines.join("\n").as_bytes());
    Ok(format!("{:016x}", hasher.0))
}

pub fn same_code(path: &Path, upstream: &str, one: &str, other: &str) -> bool {
    one == other || matches!((code(path, upstream, one), code(path, upstream, other)), (Ok(one), Ok(other)) if one == other)
}

// Every message and every added or removed line, without line numbers or context, so a rebase that applies cleanly keeps the fingerprint.
pub(super) fn fingerprint(path: &Path, upstream: &str) -> Result<String> {
    let range = format!("{upstream}..HEAD");
    let messages = git(path, &["log", "--reverse", "--format=%B%x00", &range])?;
    let changes = git(path, &["log", "--reverse", "-p", "-U0", "--no-color", "--no-ext-diff", "--format=%x00", &range])?;
    let lines: Vec<&str> = changes.lines().filter(|line| !(line.starts_with("index ") || line.starts_with("@@"))).collect();
    let mut hasher = Fnv::default();
    hasher.write(messages.as_bytes());
    hasher.write(&[0xff]);
    hasher.write(lines.join("\n").as_bytes());
    Ok(format!("{:016x}", hasher.0))
}

// FNV-1a: stable across Rust versions, unlike DefaultHasher, so an approval survives a kelpie upgrade.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf29ce484222325)
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    }
}

pub(super) fn approval_file(state: &Path, address: &str) -> PathBuf {
    state.join("approved").join(address)
}

pub(super) fn record_approval(state: &Path, address: &str, fingerprint: &str) -> Result<()> {
    crate::state::write(&approval_file(state, address), fingerprint)
}

pub(super) fn approval_holds(state: &Path, address: &str, fingerprint: &str) -> bool {
    std::fs::read_to_string(approval_file(state, address)).is_ok_and(|stored| stored.trim() == fingerprint)
}

fn forget_approval_in(state: &Path, address: &str) {
    let _ = std::fs::remove_file(approval_file(state, address));
}

pub(super) fn forget_approval(task: &str, repo: &Path) {
    if let Ok(state) = state_dir() {
        forget_approval_in(&state, &peers::address(task, &name_of(repo)));
    }
}

// The lead's word that the task's commits hold to the rules skill, until a commit changes.
pub fn approve(cwd: &Path, name: Option<&str>) -> Result<String> {
    approve_as(&crate::permit::caller(), cwd, name)
}

// Read from the Claude above, not the folder, which kelpie -C or a cd sets to anything.
fn approve_as(caller: &crate::permit::Caller, cwd: &Path, name: Option<&str>) -> Result<String> {
    use crate::permit::Caller;
    if !matches!(caller, Caller::Lead | Caller::User) {
        return Err(format!("approve is the lead's check of a task's commits; {} cannot approve them", crate::permit::describe(caller)));
    }
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    let upstream = default_branch(&repo)?;
    if ahead_behind(&slot.path, &upstream)?.0 == 0 {
        return Err(format!("{task} has no commits to approve"));
    }
    let address = peers::address(task, &name_of(&repo));
    record_approval(&state_dir()?, &address, &fingerprint(&slot.path, &upstream)?)?;
    Ok(format!("Approved {address}'s commits against the rules; a new or changed commit takes the approval back."))
}

// What the lead is asked when a task reports ready, or None when there is nothing new for it to check.
pub fn rules_check(cwd: &Path) -> Result<Option<String>> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, None, cwd)?;
    let task = task_of(&slot);
    let upstream = default_branch(&repo)?;
    let address = peers::address(task, &name_of(&repo));
    if ahead_behind(&slot.path, &upstream)?.0 == 0 || approved(&slot.path, &upstream, &address) {
        return Ok(None);
    }
    let rules = config::resolve(Some(&repo))?.settings.rules_skill.value;
    let sha = git(&slot.path, &["rev-parse", "--short", "HEAD"])?;
    Ok(Some(format!(
        "{address} is ready at {sha}: check its commits against `{rules}` (git -C {} log -p {upstream}..HEAD). When they hold, run \
         kelpie -C {} approve {task}; when one breaks a rule, kelpie tell {address} which commit and what to fix.",
        slot.path.display(), repo.display()
    )))
}

pub fn approved(path: &Path, upstream: &str, address: &str) -> bool {
    let Ok(state) = state_dir() else { return false };
    approval_file(&state, address).exists() && fingerprint(path, upstream).is_ok_and(|current| approval_holds(&state, address, &current))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::tasks::test_support::*;
    use crate::repo::{ahead_behind, git};
    use crate::tasks::history::{reword, squash};

    #[test]
    fn a_clean_rebase_keeps_the_fingerprint_and_a_new_or_changed_commit_loses_it() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let approved = fingerprint(&slot.path, &upstream).unwrap();
        land_on_main(&dir, &upstream, |_| ());
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "rebase", "--quiet", &upstream]).unwrap();
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0));
        assert_eq!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        let amend = |args: &[&str]| git(&slot.path, &[&["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--amend"], args].concat()).unwrap();
        amend(&["-m", "fourth, reworded"]);
        assert_ne!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        amend(&["-m", "fourth"]);
        assert_eq!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        std::fs::write(slot.path.join("d"), "changed").unwrap();
        git(&slot.path, &["add", "d"]).unwrap();
        amend(&["--no-edit"]);
        assert_ne!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        std::fs::write(slot.path.join("d"), "d").unwrap();
        git(&slot.path, &["add", "d"]).unwrap();
        amend(&["--no-edit"]);
        assert_eq!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        commit(&slot.path, "e", "fifth");
        assert_ne!(fingerprint(&slot.path, &upstream).unwrap(), approved);
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn an_approval_holds_only_for_the_fingerprint_it_recorded_and_is_gone_once_forgotten() {
        let state = std::env::temp_dir().join(format!("kelpie-approvals-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        assert!(!approval_holds(&state, "billing@app", "abc"));
        record_approval(&state, "billing@app", "abc").unwrap();
        assert!(approval_holds(&state, "billing@app", "abc"));
        assert!(!approval_holds(&state, "billing@app", "abd"));
        assert!(!approval_holds(&state, "billing@web", "abc"));
        forget_approval_in(&state, "billing@app");
        assert!(!approval_holds(&state, "billing@app", "abc"));
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn only_the_lead_or_the_user_approves_whatever_folder_it_runs_from() {
        use crate::permit::Caller;
        for caller in [Caller::Task("billing@app".to_string()), Caller::Agent] {
            let refused = approve_as(&caller, Path::new("/nowhere"), Some("billing")).unwrap_err();
            assert!(refused.contains("lead's check") && refused.contains(&crate::permit::describe(&caller)), "a task's kelpie -C <worktree> approve is refused too: {refused}");
        }
        for caller in [Caller::Lead, Caller::User] {
            assert!(!approve_as(&caller, Path::new("/nowhere"), Some("billing")).unwrap_err().contains("lead's check"));
        }
    }

    #[test]
    fn the_code_survives_a_clean_rebase_a_reword_and_a_squash_and_not_a_changed_line() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        commit(&slot.path, "e", "fifth");
        let reviewed = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        land_on_main(&dir, &upstream, |_| ());
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "rebase", "--quiet", &upstream]).unwrap();
        let head = || git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(head(), reviewed);
        assert!(same_code(&slot.path, &upstream, &reviewed, &head()), "a clean rebase keeps the reviewed code");
        reword(&slot.path, &head(), "fifth, reworded").unwrap();
        assert!(same_code(&slot.path, &upstream, &reviewed, &head()), "a new message keeps it");
        squash(&slot.path, "fourth and fifth").unwrap();
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0));
        assert!(same_code(&slot.path, &upstream, &reviewed, &head()), "a squash keeps it");
        commit(&slot.path, "e", "fifth, changed");
        assert!(!same_code(&slot.path, &upstream, &reviewed, &head()), "a changed line is code the reviewer never read");
        assert!(!same_code(&slot.path, &upstream, "0000000000000000000000000000000000000000", &head()), "a sha that is gone is not the same code");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn the_fingerprint_is_fnv1a_so_an_approval_survives_a_rust_upgrade() {
        let mut hasher = Fnv::default();
        hasher.write(b"");
        assert_eq!(format!("{:016x}", hasher.0), "cbf29ce484222325");
        let mut hasher = Fnv::default();
        hasher.write(b"a");
        assert_eq!(format!("{:016x}", hasher.0), "af63dc4c8601ec8c");
        let mut split = Fnv::default();
        split.write(b"ab");
        split.write(b"c");
        let mut whole = Fnv::default();
        whole.write(b"abc");
        assert_eq!(split.0, whole.0);
    }
}
