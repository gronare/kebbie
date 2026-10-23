use std::fs;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

use crate::permit::Caller;
use crate::sh::Result;

pub const REVIEWER_BIN: &str = "KELPIE_REVIEWER_BIN";

const KEPT_ENV: [&str; 12] = ["HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "LC_ALL", "PATH", "SSH_AUTH_SOCK", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_DATA_HOME"];
const KEPT_PREFIXES: [&str; 4] = ["KELPIE_", "CLAUDE_", "ANTHROPIC_", "HERDR_"];

// The child's environment is the keeper's own, never a task's, and claude is the absolute path kelpie resolved from it.
pub fn reviewer_env(own: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    own.filter(|(name, _)| KEPT_ENV.contains(&name.as_str()) || KEPT_PREFIXES.iter().any(|prefix| name.starts_with(prefix))).filter(|(name, _)| name != "KELPIE_ADDRESS").collect()
}

pub fn claude_path(env: &[(String, String)]) -> Option<PathBuf> {
    let named = env.iter().find(|(name, _)| name == crate::setup::CLAUDE_VAR).map(|(_, value)| value.clone()).filter(|value| !value.is_empty()).unwrap_or_else(|| "claude".to_string());
    if Path::new(&named).is_absolute() {
        return Some(PathBuf::from(named));
    }
    let path = env.iter().find(|(name, _)| name == "PATH").map(|(_, value)| value.split(':').filter(|dir| Path::new(dir).is_absolute()).map(PathBuf::from).collect::<Vec<_>>()).unwrap_or_default();
    crate::doctor::which(&named, &path)
}

pub fn command(kelpie: &Path, cwd: &Path, args: &[&str], env: &[(String, String)], claude: &Path) -> Command {
    let mut command = Command::new(kelpie);
    command.args(args).env_clear().envs(env.iter().map(|(name, value)| (name, value))).env(REVIEWER_BIN, claude)
        .current_dir(cwd).stdin(Stdio::piped()).process_group(0);
    command
}

pub fn launch(mut command: Command, log: &Path, run_file: &Path, tail: &str) -> Result<u32> {
    let output = crate::state::create(log).map_err(|error| format!("{}: {error}", log.display()))?;
    let errors = output.try_clone().map_err(|error| error.to_string())?;
    command.stdout(output).stderr(errors);
    let nonce = nonce()?;
    crate::state::write(run_file, format!("{}\n{tail}", digest(&nonce)))?;
    spawn(command, &nonce)
}

pub fn presented() -> String {
    let mut presented = String::new();
    let _ = std::io::stdin().take(256).read_to_string(&mut presented);
    presented
}

pub fn claude_from_keeper() -> Result<PathBuf> {
    std::env::var(REVIEWER_BIN).ok().map(PathBuf::from).filter(|bin| bin.is_absolute()).ok_or_else(|| "the keeper gave no claude path".to_string())
}

fn nonce() -> Result<String> {
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom").and_then(|mut random| random.read_exact(&mut bytes)).map_err(|error| error.to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn digest(nonce: &str) -> String {
    Sha256::digest(nonce.trim().as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

// Only a child the keeper started, with the nonce it handed over and no task in its chain, is trusted; the keeper runs detached, so its chain reads as an agent.
pub fn trusted(caller: &Caller, presented: &str, recorded: Option<&str>) -> bool {
    !matches!(caller, Caller::Task(_) | Caller::Sandboxed | Caller::LeadSandboxed) && !presented.trim().is_empty() && recorded.and_then(|record| record.lines().next()).is_some_and(|hash| hash.trim() == digest(presented))
}

// The nonce goes over stdin, which no other process can read.
#[cfg(not(test))]
fn spawn(mut command: Command, nonce: &str) -> Result<u32> {
    use std::io::Write;
    let program = command.get_args().map(|arg| arg.to_string_lossy().to_string()).collect::<Vec<_>>().join(" ");
    let mut child = command.spawn().map_err(|error| format!("kelpie {program}: {error}"))?;
    let written = child.stdin.take().ok_or("the child has no stdin".to_string()).and_then(|mut stdin| writeln!(stdin, "{nonce}").map_err(|error| error.to_string()));
    let pid = crate::sh::reap(child);
    written.map(|_| pid)
}

#[cfg(test)]
pub(crate) struct Spawned {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub nonce: String,
}

#[cfg(test)]
thread_local! {
    pub(crate) static SPAWNED: std::cell::RefCell<Vec<Spawned>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn spawn(command: Command, nonce: &str) -> Result<u32> {
    let text = |value: &std::ffi::OsStr| value.to_string_lossy().to_string();
    let spawned = Spawned {
        program: text(command.get_program()),
        args: command.get_args().map(text).collect(),
        env: command.get_envs().filter_map(|(name, value)| Some((text(name), text(value?)))).collect(),
        nonce: nonce.to_string(),
    };
    SPAWNED.with(|all| all.borrow_mut().push(spawned));
    Ok(std::process::id())
}
