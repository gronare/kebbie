use std::fs;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::permit::Caller;
use crate::sh::Result;

pub const REVIEWER_BIN: &str = "KEBBIE_REVIEWER_BIN";
pub const REVIEWER_CODEX_BIN: &str = "KEBBIE_REVIEWER_CODEX_BIN";

const KEPT_ENV: [&str; 12] = ["HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "LC_ALL", "PATH", "SSH_AUTH_SOCK", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_DATA_HOME"];
const KEPT_PREFIXES: [&str; 6] = ["KEBBIE_", "CLAUDE_", "ANTHROPIC_", "HERDR_", "CODEX_", "OPENAI_"];

// The child's environment is the keeper's own, never a task's, and claude is the absolute path kebbie resolved from it.
pub fn reviewer_env(own: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    own.filter(|(name, _)| KEPT_ENV.contains(&name.as_str()) || KEPT_PREFIXES.iter().any(|prefix| name.starts_with(prefix))).filter(|(name, _)| name != "KEBBIE_ADDRESS").collect()
}

pub fn claude_path(env: &[(String, String)]) -> Option<PathBuf> {
    let named = env.iter().find(|(name, _)| name == crate::setup::CLAUDE_VAR).map(|(_, value)| value.clone()).filter(|value| !value.is_empty()).unwrap_or_else(|| "claude".to_string());
    if Path::new(&named).is_absolute() {
        return Some(PathBuf::from(named));
    }
    crate::doctor::which(&named, &absolute_path(env))
}

fn absolute_path(env: &[(String, String)]) -> Vec<PathBuf> {
    env.iter().find(|(name, _)| name == "PATH").map(|(_, value)| value.split(':').filter(|dir| Path::new(dir).is_absolute()).map(PathBuf::from).collect::<Vec<_>>()).unwrap_or_default()
}

// A review profile that runs Codex finds codex where the keeper's own PATH has it, as Claude's reviewer finds claude.
pub fn codex_path(env: &[(String, String)]) -> Option<PathBuf> {
    crate::doctor::which("codex", &absolute_path(env))
}

pub fn command(kebbie: &Path, cwd: &Path, args: &[&str], env: &[(String, String)], claude: &Path) -> Command {
    let mut command = Command::new(kebbie);
    command.args(args).env_clear().envs(env.iter().map(|(name, value)| (name, value))).env_remove(REVIEWER_CODEX_BIN).env(REVIEWER_BIN, claude)
        .current_dir(cwd).stdin(Stdio::piped()).process_group(0);
    if let Some(codex) = codex_path(env) {
        command.env(REVIEWER_CODEX_BIN, codex);
    }
    command
}

pub fn launch(command: Command, log: &Path, run_file: &Path, tail: &str) -> Result<u32> {
    launch_by(command, log, run_file, tail, spawn)
}

// For a child the user's or the lead's own command starts and waits for nobody: it runs in tests too.
pub fn launch_now(command: Command, log: &Path, run_file: &Path, tail: &str) -> Result<u32> {
    launch_by(command, log, run_file, tail, spawn_now)
}

fn launch_by(mut command: Command, log: &Path, run_file: &Path, tail: &str, spawn: fn(Command, &str) -> Result<u32>) -> Result<u32> {
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

pub fn codex_from_keeper() -> Result<PathBuf> {
    std::env::var(REVIEWER_CODEX_BIN).ok().map(PathBuf::from).filter(|bin| bin.is_absolute()).ok_or_else(|| "the keeper found no codex on its PATH".to_string())
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

// A task asks from its own process; the keeper's tick starts the child from its own environment, with a nonce only the child reads.
pub struct Queue {
    pub dir: fn(&Path) -> PathBuf,
    pub args: &'static [&'static str],
    pub is_ours: fn(&str) -> bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    pub worktree: PathBuf,
    #[serde(default)]
    pub payload: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

impl Ask {
    // A marker an older kebbie wrote is lines: a pid alone, or the worktree among other lines.
    fn read(text: &str) -> Ask {
        serde_json::from_str(text).unwrap_or_else(|_| Ask {
            worktree: text.lines().map(str::trim).find(|line| line.starts_with('/')).map(PathBuf::from).unwrap_or_default(),
            payload: String::new(),
            pid: text.lines().next().map(str::trim).filter(|line| !line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit())).and_then(|pid| pid.parse().ok()),
        })
    }
}

const STARTING: std::time::Duration = std::time::Duration::from_secs(60);

impl Queue {
    pub fn file(&self, state: &Path, address: &str, kind: &str) -> PathBuf {
        (self.dir)(state).join(format!("{address}.{kind}"))
    }

    fn write(&self, state: &Path, address: &str, kind: &str, ask: &Ask) -> Result<()> {
        crate::state::write(&self.file(state, address, kind), serde_json::to_string(ask).map_err(|error| error.to_string())?)
    }

    fn read(&self, state: &Path, address: &str, kind: &str) -> Option<Ask> {
        fs::read_to_string(self.file(state, address, kind)).ok().map(|text| Ask::read(&text))
    }

    // Wanted, or started and not settled yet, whether or not its child still runs.
    pub fn asked(&self, state: &Path, address: &str) -> Option<Ask> {
        self.wanted(state, address).or_else(|| self.read(state, address, "running"))
    }

    pub fn want(&self, state: &Path, address: &str, worktree: &Path, payload: &str) -> Result<()> {
        self.write(state, address, "wanted", &Ask { worktree: worktree.to_path_buf(), payload: payload.to_string(), pid: None })
    }

    pub fn wanted(&self, state: &Path, address: &str) -> Option<Ask> {
        self.read(state, address, "wanted")
    }

    pub fn unwant(&self, state: &Path, address: &str) {
        let _ = fs::remove_file(self.file(state, address, "wanted"));
    }

    // The marker's pid while that process is still this queue's child; a pid another program has since a reboot is no child.
    pub fn pid(&self, state: &Path, address: &str) -> Option<String> {
        let pid = self.read(state, address, "running")?.pid?.to_string();
        crate::sh::run("ps", &["-p", &pid, "-o", "args="], None).ok().filter(|args| (self.is_ours)(args)).map(|_| pid)
    }

    // A marker whose child is gone, or that never got its pid within a minute of the start.
    pub fn dead(&self, state: &Path) -> Vec<(String, Ask)> {
        self.addresses(state, "running").into_iter().filter_map(|address| {
            let ask = self.read(state, &address, "running")?;
            let starting = ask.pid.is_none() && fs::metadata(self.file(state, &address, "running")).and_then(|meta| meta.modified()).ok().and_then(|at| at.elapsed().ok()).is_some_and(|age| age < STARTING);
            (!starting && self.pid(state, &address).is_none()).then_some((address, ask))
        }).collect()
    }

    pub fn done(&self, state: &Path, address: &str) {
        let _ = fs::remove_file(self.file(state, address, "running"));
    }

    // A child still running is stopped first, so nothing it settles lands for a task that is gone.
    pub fn forget(&self, state: &Path, address: &str) {
        if let Some(pid) = self.pid(state, address) {
            crate::review::stop_group(&pid);
        }
        for kind in ["wanted", "running", "run"] {
            let _ = fs::remove_file(self.file(state, address, kind));
        }
    }

    fn addresses(&self, state: &Path, kind: &str) -> Vec<String> {
        let suffix = format!(".{kind}");
        fs::read_dir((self.dir)(state)).into_iter().flatten().flatten().filter_map(|entry| entry.file_name().to_string_lossy().strip_suffix(&suffix).map(str::to_string)).collect()
    }

    pub fn tick(&self, state: &Path, env: &[(String, String)], log: impl Fn(&str) -> PathBuf) -> Vec<(String, Ask, Result<()>)> {
        let mut started = Vec::new();
        for address in self.addresses(state, "wanted") {
            if self.pid(state, &address).is_some() {
                continue;
            }
            let Some(ask) = self.wanted(state, &address) else { continue };
            self.unwant(state, &address);
            if !crate::rules_check::worktree_is(&ask.worktree, &address) {
                continue;
            }
            let begun = claude_path(env).ok_or_else(|| "kebbie found no claude on the keeper's PATH".to_string())
                .and_then(|claude| self.begin(state, &address, &ask, self.command(&crate::peers::own_exe()?, &ask.worktree, env, &claude), &log(&address), launch));
            started.push((address, ask, begun));
        }
        started
    }

    // The user's or the lead's own request starts its child at once, from the environment the keeper gives one.
    #[allow(clippy::too_many_arguments)]
    pub fn start_now(&self, state: &Path, address: &str, worktree: &Path, payload: &str, kebbie: &Path, env: &[(String, String)], log: &Path) -> Result<()> {
        let claude = claude_path(env).ok_or("kebbie found no claude on your PATH")?;
        let ask = Ask { worktree: worktree.to_path_buf(), payload: payload.to_string(), pid: None };
        self.begin(state, address, &ask, self.command(kebbie, worktree, env, &claude), log, launch_now)
    }

    pub(crate) fn command(&self, kebbie: &Path, worktree: &Path, env: &[(String, String)], claude: &Path) -> Command {
        let place = worktree.to_string_lossy();
        let args: Vec<&str> = ["-C", place.as_ref()].into_iter().chain(self.args.iter().copied()).collect();
        command(kebbie, worktree, &args, env, claude)
    }

    // The marker is there before the child, so a second ask right after the first finds it starting.
    fn begin(&self, state: &Path, address: &str, ask: &Ask, command: Command, log: &Path, launch: fn(Command, &Path, &Path, &str) -> Result<u32>) -> Result<()> {
        self.write(state, address, "running", ask)?;
        match launch(command, log, &self.file(state, address, "run"), &ask.payload) {
            Ok(pid) => {
                if self.read(state, address, "running").is_some_and(|running| running.pid.is_none()) {
                    self.write(state, address, "running", &Ask { pid: Some(pid), ..ask.clone() })?;
                }
                Ok(())
            }
            Err(error) => {
                self.done(state, address);
                Err(error)
            }
        }
    }

    // The child's half: only the keeper's start with its nonce, and no task in the chain, gets the payload the keeper recorded.
    pub fn take(&self, state: &Path, address: &str, presented: &str, caller: &Caller) -> Option<String> {
        let record = fs::read_to_string(self.file(state, address, "run")).ok();
        let _ = fs::remove_file(self.file(state, address, "run"));
        if !trusted(caller, presented, record.as_deref()) {
            return None;
        }
        let ask = self.read(state, address, "running").unwrap_or_default();
        let _ = self.write(state, address, "running", &Ask { pid: Some(std::process::id()), ..ask });
        Some(record.unwrap_or_default().lines().skip(1).collect::<Vec<_>>().join("\n"))
    }
}

// A helper the keeper started (the investigator, the writer) runs the profile's agent with that helper's tools in cwd; its answer is Claude's output or Codex's last message.
#[allow(clippy::too_many_arguments)]
pub fn ask_helper(helper: crate::agent::Helper, profile: &crate::profiles::Profile, prompt: &Path, answer: &Path, cwd: &Path, brief: &str, kebbie: &str, timeout: std::time::Duration) -> Result<String> {
    let _ = fs::remove_file(answer);
    let agent = profile.agent;
    let reader = crate::agent::Reviewer { model: profile.model.as_deref(), effort: profile.effort.as_deref(), settings: prompt, prompt, rules: None, docs: None, answer };
    let (program, args) = agent.investigator_command(&reader, kebbie, helper);
    let bin = match agent {
        crate::agent::Agent::Codex => codex_from_keeper()?,
        crate::agent::Agent::Claude => claude_from_keeper()?,
    };
    let input = agent.reviewer_input(prompt, brief)?;
    let mut command = Command::new(&bin);
    command.args(&args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = command.spawn().map_err(|error| format!("{program}: {error}"))?;
    use std::io::Write;
    child.stdin.take().ok_or(format!("{program} has no stdin"))?.write_all(input.as_bytes()).map_err(|error| error.to_string())?;
    let output = crate::review::wait_within(child, timeout).map_err(|error| format!("{program} {error}"))?;
    if !output.status.success() {
        return Err(format!("{program} stopped with {}: {}", output.status, String::from_utf8_lossy(&output.stderr).trim()));
    }
    if agent == crate::agent::Agent::Codex {
        return fs::read_to_string(answer).map_err(|error| format!("codex left no answer in {}: {error}", answer.display()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(not(test))]
fn spawn(command: Command, nonce: &str) -> Result<u32> {
    spawn_now(command, nonce)
}

// The nonce goes over stdin, which no other process can read.
fn spawn_now(mut command: Command, nonce: &str) -> Result<u32> {
    use std::io::Write;
    let program = command.get_args().map(|arg| arg.to_string_lossy().to_string()).collect::<Vec<_>>().join(" ");
    let mut child = command.spawn().map_err(|error| format!("kebbie {program}: {error}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    const QUEUED: Queue = Queue { dir: |state| state.join("queued"), args: &["review", "--assess"], is_ours: |args| args.contains(" review --assess") };

    #[test]
    fn a_queued_child_takes_its_payload_from_the_keepers_record_once() {
        let state = crate::sh::test_dir("trusted-queue");
        QUEUED.want(&state, "billing@app", Path::new("/src/wt1.app"), "require").unwrap();
        let ask = QUEUED.wanted(&state, "billing@app").unwrap();
        assert_eq!((ask.worktree.as_path(), ask.payload.as_str()), (Path::new("/src/wt1.app"), "require"));
        let begin = || {
            QUEUED.begin(&state, "billing@app", &ask, Command::new("/bin/kebbie"), &state.join("child.log"), launch).unwrap();
            QUEUED.write(&state, "billing@app", "running", &Ask { payload: "suggest".to_string(), ..ask.clone() }).unwrap();
            SPAWNED.with(|all| all.borrow_mut().pop().unwrap().nonce)
        };
        begin();
        assert_eq!(QUEUED.take(&state, "billing@app", "guess", &Caller::User), None, "no nonce, no payload");
        let nonce = begin();
        assert_eq!(QUEUED.take(&state, "billing@app", &nonce, &Caller::User).as_deref(), Some("require"), "the child reads what the keeper recorded, not the marker beside it");
        assert_eq!(QUEUED.take(&state, "billing@app", &nonce, &Caller::User), None, "a record is taken once");
        assert_eq!(QUEUED.dead(&state).iter().map(|(address, _)| address.as_str()).collect::<Vec<_>>(), ["billing@app"], "its pid is no child of this queue");
        QUEUED.write(&state, "fresh@app", "running", &Ask::default()).unwrap();
        assert!(!QUEUED.dead(&state).iter().any(|(address, _)| address == "fresh@app"), "a child just starting has no pid yet");
        QUEUED.forget(&state, "billing@app");
        QUEUED.forget(&state, "fresh@app");
        assert!(QUEUED.dead(&state).is_empty() && QUEUED.asked(&state, "billing@app").is_none());
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_keeper_hands_its_own_codex_to_a_reviewer_and_never_one_from_the_environment_it_was_given() {
        let dir = crate::sh::test_dir("trusted-codex");
        crate::executable::write(dir.join("codex"), "#!/bin/sh\n").unwrap();
        let env = vec![("PATH".to_string(), format!("relative:{}", dir.display())), (REVIEWER_CODEX_BIN.to_string(), "/tmp/evil/codex".to_string()), ("CODEX_HOME".to_string(), "/home/ada/.codex".to_string())];
        assert_eq!(codex_path(&env), Some(dir.join("codex")));
        let built = command(Path::new("/bin/kebbie"), &dir, &["review", "--second-opinion"], &reviewer_env(env.clone().into_iter()), Path::new("/opt/claude"));
        let envs: Vec<(String, Option<String>)> = built.get_envs().map(|(name, value)| (name.to_string_lossy().to_string(), value.map(|value| value.to_string_lossy().to_string()))).collect();
        assert!(envs.contains(&(REVIEWER_CODEX_BIN.to_string(), Some(dir.join("codex").display().to_string()))), "{envs:?}");
        assert!(!envs.contains(&(REVIEWER_CODEX_BIN.to_string(), Some("/tmp/evil/codex".to_string()))), "{envs:?}");
        assert!(reviewer_env(env.into_iter()).iter().any(|(name, _)| name == "CODEX_HOME"), "Codex's sign-in stays where its home is");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_marker_an_older_kebbie_wrote_still_reads() {
        assert_eq!(Ask::read("12345\n"), Ask { worktree: PathBuf::new(), payload: String::new(), pid: Some(12345) });
        assert_eq!(Ask::read("12345\nrequire\n").pid, Some(12345));
        assert_eq!(Ask::read("/src/wt1.app\nrequire\n").worktree, Path::new("/src/wt1.app"));
        assert_eq!(Ask::read("abc123\n0\n/src/wt1.app\n").worktree, Path::new("/src/wt1.app"));
        assert_eq!(Ask::read("$(id)").pid, None);
    }
}
