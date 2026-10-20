use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::sh::Result;

pub const BIN_VAR: &str = "KELPIE_VAULT_BIN";
pub const NEEDS_UPDATE: &str = "claude plugin update vault@gronare";
const TIMEOUT: Duration = Duration::from_secs(30);
const STATUS_TTL: u64 = 600;

#[derive(Clone, Debug, PartialEq)]
pub struct Cli {
    pub command: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct Opened {
    pub path: String,
    pub created: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    pub configured: bool,
    pub writable: Option<bool>,
    pub tasks: bool,
    pub version: String,
    pub backend: String,
}

impl Probe {
    pub fn on(&self) -> bool {
        self.configured && self.writable != Some(false) && self.tasks
    }
}

fn field(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

impl Cli {
    pub fn new(command: Vec<String>) -> Self {
        Self { command }
    }

    pub fn shown(&self) -> String {
        self.command.join(" ")
    }

    fn output(&self, args: &[&str]) -> Result<std::result::Result<String, String>> {
        let answer = self.run(&self.command, args)?;
        match (&answer, self.refreshed()) {
            (Err(error), Some(refreshed)) if error.contains("unsatisfiable") => self.run(&refreshed, args),
            _ => Ok(answer),
        }
    }

    // uv keeps PyPI's index for a while, so right after a vault release uvx calls the plugin's new pin unsatisfiable until it refreshes.
    fn refreshed(&self) -> Option<Vec<String>> {
        let (program, rest) = self.command.split_first()?;
        (Path::new(program).file_name()? == "uvx").then(|| [program.clone(), "--refresh".to_string()].into_iter().chain(rest.iter().cloned()).collect())
    }

    fn run(&self, command: &[String], args: &[&str]) -> Result<std::result::Result<String, String>> {
        let shown = command.join(" ");
        let (program, rest) = command.split_first().ok_or("the vault command is empty")?;
        let mut command = Command::new(program);
        command.args(rest).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| format!("{program}: {error}"))?;
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        let mut stderr = child.stderr.take().ok_or("no stderr")?;
        let out = std::thread::spawn(move || { let mut text = String::new(); let _ = stdout.read_to_string(&mut text); text });
        let err = std::thread::spawn(move || { let mut text = String::new(); let _ = stderr.read_to_string(&mut text); text });
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                break status;
            }
            if started.elapsed() > TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{shown} {} gave no answer in {} s", args.join(" "), TIMEOUT.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let (stdout, stderr) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
        Ok(if status.success() { Ok(stdout) } else { Err(stderr.trim().lines().last().unwrap_or("failed").to_string()) })
    }

    fn call(&self, args: &[&str]) -> Result<Value> {
        let stdout = self.output(args)?.map_err(|error| format!("vault {}: {error}", args[..2.min(args.len())].join(" ")))?;
        serde_json::from_str(stdout.trim()).map_err(|error| format!("vault {}: not JSON ({error})", args[..2.min(args.len())].join(" ")))
    }

    pub fn open(&self, name: &str, cwd: &Path, branch: Option<&str>, note: Option<&str>) -> Result<Opened> {
        let cwd = cwd.to_string_lossy();
        let mut args = vec!["task", "open", "--name", name, "--cwd", &cwd];
        args.extend(branch.map(|branch| ["--branch", branch]).into_iter().flatten());
        args.extend(note.map(|note| ["--note", note]).into_iter().flatten());
        let value = self.call(&args)?;
        let path = field(&value, "path");
        if path.is_empty() {
            return Err("vault task open: no path in its answer".to_string());
        }
        Ok(Opened { path, created: value["created"] == true })
    }

    pub fn missing(&self, note: &str, need: &[&str]) -> Result<Vec<String>> {
        let value = self.call(&["task", "check", note, "--need", &need.join(",")])?;
        let missing = value["missing"].as_array().ok_or("vault task check: no missing list in its answer")?;
        Ok(missing.iter().filter_map(Value::as_str).map(str::to_string).collect())
    }

    pub fn close(&self, note: &str) -> Result<String> {
        self.call(&["task", "close", note]).map(|value| field(&value, "path"))
    }

    pub fn backlog(&self, note: &str) -> Result<String> {
        self.call(&["task", "backlog", note]).map(|value| field(&value, "path"))
    }

    pub fn log(&self, cwd: &Path, branch: Option<&str>, line: &str, commits: &[&str]) -> Result<String> {
        let cwd = cwd.to_string_lossy();
        let mut args = vec!["log", "append", "--cwd", &cwd];
        args.extend(branch.map(|branch| ["--branch", branch]).into_iter().flatten());
        args.extend(["--line", line]);
        for commit in commits {
            args.extend(["--commit", commit]);
        }
        self.call(&args).map(|value| field(&value, "path"))
    }

    pub fn log_path(&self, repo: &str) -> Result<String> {
        self.call(&["log", "path", "--repo", repo]).map(|value| field(&value, "path")).and_then(|path| if path.is_empty() { Err("vault log path: no path".to_string()) } else { Ok(path) })
    }

    // A vault server older than the task commands answers status but refuses `task --help`; writes are tried only with try_write, which doctor alone asks for.
    pub fn probe(&self, try_write: bool) -> Result<Probe> {
        let status = self.call(if try_write { &["status", "--probe"] } else { &["status"] })?;
        let tasks = matches!(self.output(&["task", "--help"]), Ok(Ok(_)));
        Ok(Probe { configured: status["configured"] == true, writable: status["writable"].as_bool(), tasks, version: field(&status, "version"), backend: field(&status, "backend") })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Plugin {
    Missing,
    Disabled { id: String, version: String },
    Enabled { id: String, version: String, root: PathBuf },
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok())
}

pub fn hooks_of(root: &Path) -> String {
    std::fs::read_to_string(root.join("hooks/hooks.json")).unwrap_or_default()
}

pub fn installed(claude_home: &Path) -> Option<(String, String, PathBuf)> {
    let installed = read_json(&claude_home.join("plugins/installed_plugins.json")).unwrap_or_default();
    installed["plugins"].as_object().into_iter().flatten().filter(|(id, _)| id.starts_with("vault@")).find_map(|(id, installs)| {
        let entry = installs.as_array()?.iter().find(|entry| entry["installPath"].as_str().is_some_and(|root| hooks_of(Path::new(root)).contains("notes-vault-mcp")))?;
        Some((id.clone(), field(entry, "version"), PathBuf::from(field(entry, "installPath"))))
    })
}

pub fn plugin(claude_home: &Path) -> Plugin {
    let settings = read_json(&claude_home.join("settings.json")).unwrap_or_default();
    match installed(claude_home) {
        None => Plugin::Missing,
        Some((id, version, _)) if settings["enabledPlugins"][&id] == false => Plugin::Disabled { id, version },
        Some((id, version, root)) => Plugin::Enabled { id, version, root },
    }
}

// "uvx notes-vault-mcp==0.5.1 hook session-start" -> the words before `hook`, so kelpie runs the plugin's own pin.
pub fn hook_prefix(hooks: &str) -> Option<Vec<String>> {
    fn commands(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(map) => map.iter().for_each(|(key, value)| if key == "command" { found.extend(value.as_str().map(str::to_string)) } else { commands(value, found) }),
            Value::Array(items) => items.iter().for_each(|item| commands(item, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    commands(&serde_json::from_str(hooks).ok()?, &mut found);
    found.iter().filter(|command| command.contains("notes-vault-mcp")).find_map(|command| {
        let words: Vec<String> = command.split_whitespace().map(str::to_string).collect();
        let at = words.iter().position(|word| word == "hook")?;
        (at > 0).then(|| words[..at].to_vec())
    })
}

pub fn command(env_bin: Option<String>, configured: Option<String>, plugin: &Plugin) -> Option<Vec<String>> {
    if let Some(bin) = env_bin.filter(|bin| !bin.is_empty()) {
        return Some(vec![bin]);
    }
    if let Some(configured) = configured {
        return Some(configured.split_whitespace().map(str::to_string).collect());
    }
    match plugin {
        Plugin::Enabled { root, .. } => hook_prefix(&hooks_of(root)),
        _ => None,
    }
}

fn claude_home() -> PathBuf {
    crate::tasks::claude_home().unwrap_or_default()
}

pub fn cli() -> Option<Cli> {
    let env_bin = std::env::var(BIN_VAR).ok().filter(|bin| !bin.is_empty());
    #[cfg(test)]
    env_bin.as_ref()?;
    let plugin = if env_bin.is_some() { Plugin::Missing } else { plugin(&claude_home()) };
    if env_bin.is_none() && !matches!(plugin, Plugin::Enabled { .. }) {
        return None;
    }
    let configured = crate::config::resolve(None).ok().and_then(|loaded| loaded.settings.vault_command.value);
    let command = command(env_bin, configured, &plugin)?;
    Some(Cli::new(command))
}

// Cheap, for the board: the plugin is installed and enabled, without asking the vault.
pub fn plugin_enabled() -> bool {
    !cfg!(test) && matches!(plugin(&claude_home()), Plugin::Enabled { .. })
}

fn status_file(state: &Path) -> PathBuf {
    state.join("vault-status.json")
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

// The probe's verdict for this command, kept a while so a start, a push or an edit does not wait on uvx each time.
pub fn on_with(cli: &Cli, state: &Path, at: u64, probe: impl FnOnce(&Cli) -> Result<Probe>) -> bool {
    let cached = read_json(&status_file(state)).filter(|cached| cached["command"] == cli.shown() && cached["at"].as_u64().is_some_and(|then| at.saturating_sub(then) < STATUS_TTL));
    if let Some(cached) = cached {
        return cached["on"] == true;
    }
    let on = probe(cli).is_ok_and(|probe| probe.on());
    let _ = std::fs::write(status_file(state), json!({ "command": cli.shown(), "at": at, "on": on }).to_string());
    on
}

// The vault kelpie keeps tasks in: the plugin is there, enabled and configured, and its server has the task commands; the board lives for days, so the verdict is the kept one and not the process's first.
pub fn detect() -> Option<Cli> {
    let cli = cli()?;
    let state = crate::lead::state_dir().ok()?;
    on_with(&cli, &state, now(), |cli| cli.probe(false)).then_some(cli)
}

// The last verdict kept for this command, at any age, without running the vault: for the board's own thread.
pub fn known_on_in(cli: &Cli, state: &Path) -> bool {
    read_json(&status_file(state)).is_some_and(|cached| cached["command"] == cli.shown() && cached["on"] == true)
}

pub fn known_on() -> bool {
    cli().zip(crate::lead::state_dir().ok()).is_some_and(|(cli, state)| known_on_in(&cli, &state))
}

pub fn is_note(item: &str) -> bool {
    item.ends_with(".md") && !item.starts_with(crate::backlog_file::PREFIX)
}

pub fn log_note(cli: &Cli, state: &Path, repo: &str) -> Option<String> {
    let file = state.join("vault-logs").join(repo);
    if let Some(known) = std::fs::read_to_string(&file).ok().map(|path| path.trim().to_string()).filter(|path| !path.is_empty()) {
        return Some(known);
    }
    let path = cli.log_path(repo).ok()?;
    let _ = std::fs::create_dir_all(file.parent()?).and_then(|_| std::fs::write(&file, &path));
    Some(path)
}

pub fn warn_log(state: &Path, text: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(state.join("vault.log")) {
        let _ = writeln!(file, "{} {text}", now());
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // A vault command that records each argv as one line in argv.log and answers with <dir>/answers/<word>-<word>.json, or fails with <word>-<word>.err.
    pub struct Fake {
        pub dir: PathBuf,
        pub cli: Cli,
    }

    impl Fake {
        pub fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("kelpie-fake-vault-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("answers")).unwrap();
            let script = dir.join("vault");
            std::fs::write(&script, format!(
                "#!/bin/sh\nd='{}'\nprintf '%s\\n' \"$*\" >> \"$d/argv.log\"\nkey=\"$1-$2\"\nif [ -f \"$d/answers/$key.err\" ]; then cat \"$d/answers/$key.err\" >&2; exit 1; fi\nif [ -f \"$d/answers/$key.json\" ]; then cat \"$d/answers/$key.json\"; exit 0; fi\nif [ -f \"$d/answers/$1.json\" ]; then cat \"$d/answers/$1.json\"; exit 0; fi\nexit 0\n",
                dir.display()
            )).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            let cli = Cli::new(vec![script.to_string_lossy().to_string()]);
            Self { dir, cli }
        }

        pub fn answer(&self, key: &str, json: &str) {
            let _ = std::fs::remove_file(self.dir.join("answers").join(format!("{key}.err")));
            std::fs::write(self.dir.join("answers").join(format!("{key}.json")), json).unwrap();
        }

        pub fn fail(&self, key: &str, error: &str) {
            std::fs::write(self.dir.join("answers").join(format!("{key}.err")), error).unwrap();
        }

        pub fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.join("argv.log")).unwrap_or_default().lines().map(str::to_string).collect()
        }

        pub fn state(&self) -> PathBuf {
            let state = self.dir.join("state");
            std::fs::create_dir_all(&state).unwrap();
            state
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;

    #[test]
    fn the_command_is_the_plugins_own_pin_read_from_its_hooks() {
        let hooks = r#"{"hooks":{"SessionStart":[{"matcher":"startup","hooks":[{"type":"command","command":"uvx notes-vault-mcp==0.5.1 hook session-start","timeout":30}]}]}}"#;
        assert_eq!(hook_prefix(hooks), Some(vec!["uvx".to_string(), "notes-vault-mcp==0.5.1".to_string()]));
        assert_eq!(hook_prefix(r#"{"hooks":{"Stop":[{"hooks":[{"command":"sh other.sh hook"}]}]}}"#), None);
        assert_eq!(hook_prefix("not json"), None);
    }

    #[test]
    fn a_new_pin_uv_has_not_seen_is_retried_once_with_refresh() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("kelpie-vault-refresh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let uvx = dir.join("uvx");
        std::fs::write(&uvx, "#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/argv.log\"\nif [ \"$1\" = --refresh ]; then echo '{\"missing\":[]}'; else echo 'requirements are unsatisfiable.' >&2; exit 1; fi\n").unwrap();
        std::fs::set_permissions(&uvx, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cli = Cli::new(vec![uvx.to_string_lossy().to_string(), "notes-vault-mcp==9.9.9".to_string()]);
        assert_eq!(cli.missing("Projects/x.md", &["problem"]), Ok(vec![]));
        let calls = std::fs::read_to_string(dir.join("argv.log")).unwrap();
        assert_eq!(calls.lines().collect::<Vec<_>>(), ["notes-vault-mcp==9.9.9 task check Projects/x.md --need problem", "--refresh notes-vault-mcp==9.9.9 task check Projects/x.md --need problem"]);
        assert_eq!(Cli::new(vec!["/bin/false".to_string()]).refreshed(), None, "only uvx refreshes");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_env_override_beats_the_machine_config_which_beats_the_plugin() {
        let dir = std::env::temp_dir().join(format!("kelpie-vault-command-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("hooks")).unwrap();
        std::fs::write(dir.join("hooks/hooks.json"), r#"{"hooks":{"Stop":[{"hooks":[{"command":"uvx notes-vault-mcp==0.6.0 hook stop"}]}]}}"#).unwrap();
        let plugin = Plugin::Enabled { id: "vault@gronare".to_string(), version: "0.3.0".to_string(), root: dir.clone() };
        assert_eq!(command(None, None, &plugin), Some(vec!["uvx".to_string(), "notes-vault-mcp==0.6.0".to_string()]));
        assert_eq!(command(None, Some("uv run notes-vault-mcp".to_string()), &plugin), Some(vec!["uv".to_string(), "run".to_string(), "notes-vault-mcp".to_string()]));
        assert_eq!(command(Some("/tmp/fake vault".to_string()), Some("uv run x".to_string()), &plugin), Some(vec!["/tmp/fake vault".to_string()]));
        assert_eq!(command(None, None, &Plugin::Missing), None);
        assert_eq!(command(None, None, &Plugin::Disabled { id: "vault@gronare".to_string(), version: "0.3.0".to_string() }), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_plugin_counts_only_when_installed_and_not_disabled() {
        let home = std::env::temp_dir().join(format!("kelpie-vault-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let root = home.join("plugins/cache/gronare/vault/0.2.15");
        std::fs::create_dir_all(root.join("hooks")).unwrap();
        assert_eq!(plugin(&home), Plugin::Missing);
        std::fs::write(root.join("hooks/hooks.json"), r#"{"hooks":{"Stop":[{"hooks":[{"command":"uvx notes-vault-mcp==0.5.1 hook stop"}]}]}}"#).unwrap();
        std::fs::write(home.join("plugins/installed_plugins.json"), json!({ "plugins": { "vault@gronare": [{ "installPath": root, "version": "0.2.15" }] } }).to_string()).unwrap();
        assert_eq!(plugin(&home), Plugin::Enabled { id: "vault@gronare".to_string(), version: "0.2.15".to_string(), root: root.clone() });
        std::fs::write(home.join("settings.json"), r#"{"enabledPlugins":{"vault@gronare":false}}"#).unwrap();
        assert_eq!(plugin(&home), Plugin::Disabled { id: "vault@gronare".to_string(), version: "0.2.15".to_string() });
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn each_call_sends_the_interface_arguments_and_reads_the_json_answer() {
        let fake = Fake::new("calls");
        fake.answer("task-open", r#"{"path":"Projects/kelpie-billing.md","created":true,"repo":"kelpie","area":"kelpie"}"#);
        fake.answer("task-check", r#"{"missing":["solution"]}"#);
        fake.answer("log-append", r#"{"path":"Log/kelpie-logg.md"}"#);
        let opened = fake.cli.open("billing", Path::new("/src/wt1.kelpie"), Some("billing"), Some("Projects/x.md")).unwrap();
        assert_eq!(opened, Opened { path: "Projects/kelpie-billing.md".to_string(), created: true });
        assert_eq!(fake.cli.missing("Projects/kelpie-billing.md", &["solution", "decisions"]).unwrap(), ["solution"]);
        assert_eq!(fake.cli.log(Path::new("/src/wt1.kelpie"), Some("billing"), "billing: Add refunds", &["a1b2c3d"]).unwrap(), "Log/kelpie-logg.md");
        assert_eq!(fake.calls(), [
            "task open --name billing --cwd /src/wt1.kelpie --branch billing --note Projects/x.md",
            "task check Projects/kelpie-billing.md --need solution,decisions",
            "log append --cwd /src/wt1.kelpie --branch billing --line billing: Add refunds --commit a1b2c3d",
        ]);
        fake.fail("task-close", "Traceback\nNoteNotFound: Projects/kelpie-billing.md");
        assert_eq!(fake.cli.close("Projects/kelpie-billing.md").unwrap_err(), "vault task close: NoteNotFound: Projects/kelpie-billing.md");
    }

    #[test]
    fn an_old_vault_that_answers_status_but_lacks_the_task_commands_is_read_only() {
        let fake = Fake::new("probe");
        fake.answer("status", r#"{"configured":true,"version":"0.5.1","backend":"s3","writable":true}"#);
        fake.fail("task---help", "invalid choice: 'task'");
        let probe = fake.cli.probe(false).unwrap();
        assert!(probe.configured && !probe.tasks && !probe.on(), "{probe:?}");
        let _ = std::fs::remove_file(fake.dir.join("answers/task---help.err"));
        assert!(fake.cli.probe(false).unwrap().on());
        fake.answer("status", r#"{"configured":true,"version":"0.6.0","backend":"local","writable":null}"#);
        assert!(fake.cli.probe(false).unwrap().on(), "plain status leaves writable unknown, which is no reason to stay off");
        fake.answer("status", r#"{"configured":false,"version":"0.6.0","backend":null,"writable":null}"#);
        assert!(!fake.cli.probe(false).unwrap().on());
        fake.answer("status", r#"{"configured":true,"version":"0.6.0","backend":"s3","writable":false}"#);
        assert!(!fake.cli.probe(true).unwrap().on());
        assert!(fake.calls().contains(&"status".to_string()) && fake.calls().last().unwrap() == "task --help" && fake.calls().contains(&"status --probe".to_string()), "{:?}", fake.calls());
    }

    #[test]
    fn the_verdict_is_kept_for_a_while_per_command() {
        let fake = Fake::new("cache");
        let state = fake.state();
        let probes = std::cell::Cell::new(0);
        let probe = |_: &Cli| { probes.set(probes.get() + 1); Ok(Probe { configured: true, writable: None, tasks: true, version: "0.6.0".to_string(), backend: "local".to_string() }) };
        assert!(on_with(&fake.cli, &state, 1000, probe));
        assert!(on_with(&fake.cli, &state, 1000 + STATUS_TTL - 1, |_| unreachable!()));
        assert!(on_with(&fake.cli, &state, 1000 + STATUS_TTL, probe));
        assert_eq!(probes.get(), 2);
        let other = Cli::new(vec!["other".to_string()]);
        assert!(!on_with(&other, &state, 1000 + STATUS_TTL, |_| Err("no uvx".to_string())), "a failing vault is off, and another command is probed afresh");
        assert!(!known_on_in(&fake.cli, &state), "the kept verdict is the other command's");
        assert!(on_with(&fake.cli, &state, 5000, probe));
        assert!(known_on_in(&fake.cli, &state), "the board reads the kept verdict, however old, and runs nothing");
    }

    #[test]
    fn without_the_override_tests_never_reach_the_real_plugin() {
        assert!(std::env::var_os(BIN_VAR).is_none() && cli().is_none() && detect().is_none() && !plugin_enabled() && !known_on(), "vault off: no vault command is ever run");
    }

    #[test]
    fn a_vault_note_is_a_markdown_path_and_not_a_local_item_or_a_linear_id() {
        assert!(is_note("Projects/kelpie-billing.md"));
        assert!(!is_note("ABC-12"));
        assert!(!is_note("local:Fix login.md"));
    }

    #[test]
    fn a_repos_log_note_is_asked_once_and_then_kept() {
        let fake = Fake::new("logs");
        let state = fake.state();
        fake.answer("log-path", r#"{"path":"Log/kelpie-logg.md"}"#);
        assert_eq!(log_note(&fake.cli, &state, "kelpie").as_deref(), Some("Log/kelpie-logg.md"));
        assert_eq!(log_note(&fake.cli, &state, "kelpie").as_deref(), Some("Log/kelpie-logg.md"));
        assert_eq!(fake.calls(), ["log path --repo kelpie"]);
    }

    #[test]
    fn a_vault_command_that_will_not_run_is_an_error_and_not_a_panic() {
        let dir = std::env::temp_dir().join(format!("kelpie-vault-missing-{}", std::process::id()));
        let missing = Cli::new(vec![dir.join("nothing-here").to_string_lossy().to_string()]);
        assert!(missing.probe(false).is_err());
    }
}
