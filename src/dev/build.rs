use std::path::Path;

use crate::sh::Result;

pub const WAIT_AT_MOST: u64 = 45 * 60;
pub const RETRY_AFTER: u64 = 10 * 60;

// What a check after a push in kebbie's own repo needs from the installed kebbie, as closures so a test never runs a real update.
pub struct Builds<'a> {
    pub needed: &'a dyn Fn(&Path) -> bool,
    pub holds: &'a dyn Fn(&Path, &str) -> bool,
    pub update: &'a dyn Fn() -> Result<()>,
    pub running: &'a dyn Fn() -> bool,
}

pub fn live<T>(with: impl FnOnce(&Builds) -> T) -> T {
    with(&Builds { needed: &|repo| super::on() && kebbie_repo(repo), holds: &installed_holds, update: &start_update, running: &updating })
}

fn kebbie_repo(repo: &Path) -> bool {
    crate::faults::watch::kebbie_checkout().is_some_and(|checkout| crate::update::same_folder(&checkout, repo))
}

// The installed binary's own commit, asked of the binary, since a pulled checkout that failed to build runs the old code.
fn installed_built() -> Option<String> {
    let root = crate::lead::install_root().ok()?;
    crate::update_check::binary_version(&root.join("target/release/kebbie"))?.1
}

pub fn installed_holds(repo: &Path, sha: &str) -> bool {
    installed_built().is_some_and(|built| holds_in(repo, &built, sha))
}

// Asked of kebbie's full checkout, never of the install's one-commit clone, which cannot place an older commit.
pub fn holds_in(repo: &Path, built: &str, sha: &str) -> bool {
    let known = |commit: &str| crate::sh::succeeds("git", &["cat-file", "-e", &format!("{commit}^{{commit}}")], Some(repo));
    if !known(built) {
        let _ = crate::sh::run("git", &["fetch", "--quiet", "origin"], Some(repo));
    }
    known(built) && known(sha) && crate::sh::succeeds("git", &["merge-base", "--is-ancestor", sha, built], Some(repo))
}

fn pid_file() -> Result<std::path::PathBuf> {
    Ok(crate::lead::state_dir()?.join("ci").join("update.pid"))
}

// The update kebbie started is still running, so a second push waits for it rather than starting another beside it.
fn updating() -> bool {
    let pid = pid_file().ok().and_then(|file| std::fs::read_to_string(file).ok()).unwrap_or_default().trim().to_string();
    !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()) && crate::sh::run("ps", &["-p", &pid, "-o", "args="], None).is_ok_and(|args| args.split_whitespace().any(|word| word == "update"))
}

fn start_update() -> Result<()> {
    use std::os::unix::process::CommandExt;
    if updating() {
        return Ok(());
    }
    let file = crate::lead::state_dir()?.join("ci").join("update.log");
    let log = crate::state::append_to(&file).map_err(|error| format!("{}: {error}", file.display()))?;
    let mut command = std::process::Command::new(crate::peers::own_exe()?);
    command.arg("update").stdin(std::process::Stdio::null()).stdout(log.try_clone().map_err(|error| error.to_string())?).stderr(log)
        .env_remove("HERDR_PANE_ID").env_remove("HERDR_TAB_ID").env_remove("HERDR_WORKSPACE_ID");
    // SAFETY: setsid is async-signal-safe and touches nothing but the child, which outlives the keeper the update restarts.
    unsafe { command.pre_exec(|| if libc::setsid() == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) }) };
    let child = command.spawn().map_err(|error| format!("kebbie update: {error}"))?;
    crate::state::write(&pid_file()?, child.id().to_string())
}
