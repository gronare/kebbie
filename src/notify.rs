use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config;
use crate::lead::state_dir;
use crate::messages;
use crate::sh::Result;

pub const DEBOUNCE_SECONDS: u64 = 60;
pub const TIMEOUT: Duration = Duration::from_secs(10);
pub const DELIVER: &str = "--deliver";
const PROGRAM_NAME: &str = "kelpie-notify";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    NeedsYou,
    Ready,
    Question,
    Summary,
    CiFailed,
    CiGreen,
    ApiError,
}

impl Event {
    pub fn key(self) -> &'static str {
        match self {
            Event::NeedsYou => "needs-you",
            Event::Ready => "ready",
            Event::Question => "question",
            Event::Summary => "summary",
            Event::CiFailed => "ci-failed",
            Event::CiGreen => "ci-green",
            Event::ApiError => "api-error",
        }
    }

    pub fn of_state(state: &str) -> Option<Event> {
        match state {
            "needs-you" => Some(Event::NeedsYou),
            "ready" => Some(Event::Ready),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub event: Event,
    pub task: Option<String>,
    pub repo: Option<String>,
    pub text: String,
}

impl Notice {
    // A task's address is task@repo; the lead and kelpie itself have no task.
    pub fn from(event: Event, address: &str, text: String) -> Notice {
        let (task, repo) = match address.split_once('@') {
            Some((task, repo)) => (Some(task.to_string()), Some(repo.to_string())),
            None => (None, None),
        };
        Notice { event, task, repo, text }
    }

    // A question is also reported as needs-you in the same turn, so both share one minute and the user hears it once.
    fn debounce_key(&self) -> String {
        let event = if self.event == Event::Question { Event::NeedsYou } else { self.event };
        format!("{}:{}@{}", event.key(), self.task.as_deref().unwrap_or_default(), self.repo.as_deref().unwrap_or_default())
    }
}

#[derive(Debug, PartialEq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

// The text is $1 to the command, so it is the last argument without being glued onto the user's own command line.
pub fn invocation(command: &str, notice: &Notice) -> Invocation {
    let env = [
        ("KELPIE_EVENT", notice.event.key().to_string()),
        ("KELPIE_TASK", notice.task.clone().unwrap_or_default()),
        ("KELPIE_REPO", notice.repo.clone().unwrap_or_default()),
        ("KELPIE_TEXT", notice.text.clone()),
    ].into_iter().map(|(name, value)| (name.to_string(), value)).collect();
    Invocation {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), command.to_string(), PROGRAM_NAME.to_string(), notice.text.clone()],
        env,
    }
}

pub fn due(last: Option<u64>, now: u64) -> bool {
    last.is_none_or(|at| now.saturating_sub(at) >= DEBOUNCE_SECONDS)
}

fn sent_file(state: &Path) -> PathBuf {
    state.join("notify-sent.json")
}

fn log_file(state: &Path) -> PathBuf {
    state.join("notify.log")
}

pub fn claim(state: &Path, notice: &Notice, now: u64) -> bool {
    let file = sent_file(state);
    let mut sent: BTreeMap<String, u64> = fs::read_to_string(&file).ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().map(|map| map.iter().filter_map(|(key, at)| Some((key.clone(), at.as_u64()?))).collect()))
        .unwrap_or_default();
    let key = notice.debounce_key();
    if !due(sent.get(&key).copied(), now) {
        return false;
    }
    sent.retain(|_, at| !due(Some(*at), now));
    sent.insert(key, now);
    let _ = crate::state::write(&file, serde_json::to_string(&sent).unwrap_or_default());
    true
}

pub fn log_error(state: &Path, text: &str) {
    if let Ok(mut file) = crate::state::append_to(&log_file(state)) {
        let _ = writeln!(file, "{} {}", messages::now(), text.replace('\n', " "));
    }
}

#[derive(Debug, PartialEq)]
pub enum Sent {
    Off,
    Held,
    Started,
    Failed,
}

// Never fails its caller: no command means no notification, and every error lands in the log.
pub fn send(cwd: &Path, notice: &Notice) -> Sent {
    let Ok(state) = state_dir() else { return Sent::Failed };
    let command = match config::resolve(Some(cwd)) {
        Ok(loaded) => loaded.settings.notify_command.value,
        Err(error) => {
            log_error(&state, &format!("{}: config: {error}", notice.event.key()));
            return Sent::Failed;
        }
    };
    let Some(command) = command else { return Sent::Off };
    if !claim(&state, notice, messages::now()) {
        return Sent::Held;
    }
    match spawn(&invocation(&command, notice)) {
        Ok(()) => Sent::Started,
        Err(error) => {
            log_error(&state, &format!("{}: {error}", notice.event.key()));
            Sent::Failed
        }
    }
}

// A detached kelpie waits on the command, so the report returns at once and the timeout still holds.
fn spawn(invocation: &Invocation) -> Result<()> {
    let kelpie = crate::peers::own_exe().map_err(|error| format!("kelpie's own path: {error}"))?;
    Command::new(kelpie).arg("notify").arg(DELIVER).arg(&invocation.program).args(&invocation.args)
        .envs(invocation.env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .process_group(0)
        .spawn().map(|child| { crate::sh::reap(child); }).map_err(|error| error.to_string())
}

pub fn run_with_timeout(program: &str, args: &[String], timeout: Duration) -> std::result::Result<(), String> {
    let mut child = Command::new(program).args(args)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped())
        .process_group(0)
        .spawn().map_err(|error| format!("{program}: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
                }
                let detail = stderr.trim();
                let code = status.code().map_or("a signal".to_string(), |code| code.to_string());
                return Err(if detail.is_empty() { format!("exited with {code}") } else { format!("exited with {code}: {detail}") });
            }
            None if started.elapsed() >= timeout => {
                let _ = Command::new("kill").args(["-s", "KILL", "--", &format!("-{}", child.id())]).stderr(Stdio::null()).status();
                let _ = child.wait();
                return Err(format!("did not finish within {} seconds and was stopped", timeout.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

// The detached half: argv after `notify --deliver` is the program and its arguments.
pub fn deliver(argv: &[String]) {
    let Ok(state) = state_dir() else { return };
    let Some((program, args)) = argv.split_first() else { return };
    let event = std::env::var("KELPIE_EVENT").unwrap_or_default();
    if let Err(error) = run_with_timeout(program, args, TIMEOUT) {
        log_error(&state, &format!("{event}: notify.command {error}"));
    }
}

pub fn notify(cwd: &Path, text: &str) -> Result<String> {
    notify_as(&crate::permit::caller(), cwd, text)
}

// The lead's word to the user, or the user's own; a task reaches the user through its report and its questions.
fn notify_as(caller: &crate::permit::Caller, cwd: &Path, text: &str) -> Result<String> {
    use crate::permit::Caller;
    if !matches!(caller, Caller::User | Caller::Lead) {
        return Err(format!("kelpie notify is the lead's word to the user; {} cannot send one, its report and its questions reach the user", crate::permit::describe(caller)));
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err("kelpie notify needs the text: kelpie notify \"<what the user should know>\"".to_string());
    }
    config::resolve(Some(cwd))?;
    let log = log_file(&state_dir()?);
    match send(cwd, &Notice { event: Event::Summary, task: None, repo: None, text }) {
        Sent::Off => Ok("No notify.command is set, so nothing was sent; kelpie config shows the key.".to_string()),
        Sent::Held => Ok(format!("Not sent: a notification like it went out less than {DEBOUNCE_SECONDS} seconds ago.")),
        Sent::Started => Ok(format!("Handed to notify.command; if it fails, the error is in {}.", log.display())),
        Sent::Failed => Err(format!("notify.command could not be started; the error is in {}", log.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_lead_and_the_user_send_a_notification() {
        use crate::permit::Caller;
        for caller in [Caller::Task("billing@app".to_string()), Caller::Agent] {
            let refused = notify_as(&caller, Path::new("/"), "push everything").unwrap_err();
            assert!(refused.contains("kelpie notify is the lead's word to the user"), "{refused}");
        }
        assert!(notify_as(&Caller::Lead, Path::new("/"), " ").unwrap_err().contains("needs the text"), "the lead gets past the gate");
    }

    fn notice(event: Event, address: &str, text: &str) -> Notice {
        Notice::from(event, address, text.to_string())
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-notify-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_command_gets_the_event_as_environment_and_the_text_as_its_last_argument() {
        let built = invocation("curl -d \"$KELPIE_TEXT\" ntfy.sh/mine", &notice(Event::NeedsYou, "billing@app", "billing@app needs you: answer which queue"));
        assert_eq!(built.program, "sh");
        assert_eq!(built.args, ["-c", "curl -d \"$KELPIE_TEXT\" ntfy.sh/mine", "kelpie-notify", "billing@app needs you: answer which queue"]);
        assert_eq!(built.env, [
            ("KELPIE_EVENT".to_string(), "needs-you".to_string()),
            ("KELPIE_TASK".to_string(), "billing".to_string()),
            ("KELPIE_REPO".to_string(), "app".to_string()),
            ("KELPIE_TEXT".to_string(), "billing@app needs you: answer which queue".to_string()),
        ]);
        let summary = invocation("x", &Notice { event: Event::Summary, task: None, repo: None, text: "3 tasks ready".to_string() });
        assert_eq!(summary.env[..3], [("KELPIE_EVENT".to_string(), "summary".to_string()), ("KELPIE_TASK".to_string(), String::new()), ("KELPIE_REPO".to_string(), String::new())]);
        assert_eq!(Notice::from(Event::Question, "lead", "x".to_string()).task, None);
        assert_eq!(Event::of_state("ready"), Some(Event::Ready));
        assert_eq!(Event::of_state("working"), None);
        assert_eq!(Event::of_state("blocked"), None);
    }

    #[test]
    fn the_text_reaches_the_shell_as_dollar_one_and_in_the_environment_unquoted() {
        let dir = temp("shell");
        let out = dir.join("out");
        let built = invocation(&format!("printf '%s|%s|%s' \"$1\" \"$KELPIE_EVENT\" \"$KELPIE_TEXT\" > {}", out.display()), &notice(Event::Ready, "a@b", "it's \"done\"; $(rm -rf /)"));
        let status = Command::new(&built.program).args(&built.args).envs(built.env.iter().map(|(name, value)| (name, value))).status().unwrap();
        assert!(status.success());
        assert_eq!(fs::read_to_string(&out).unwrap(), "it's \"done\"; $(rm -rf /)|ready|it's \"done\"; $(rm -rf /)");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn one_notification_per_task_and_event_within_a_minute() {
        assert!(due(None, 1_000));
        assert!(!due(Some(1_000), 1_059));
        assert!(due(Some(1_000), 1_060));
        let state = temp("debounce");
        let ready = notice(Event::Ready, "billing@app", "ready");
        assert!(claim(&state, &ready, 1_000));
        assert!(!claim(&state, &ready, 1_030), "the same task and event within the minute is held");
        assert!(claim(&state, &notice(Event::NeedsYou, "billing@app", "needs you"), 1_030), "another event of the same task goes out");
        assert!(!claim(&state, &notice(Event::Question, "billing@app", "asks"), 1_031), "a question right after its needs-you report is the same news");
        assert!(claim(&state, &notice(Event::Ready, "billing@web", "ready"), 1_030), "the same task name in another repo goes out");
        assert!(claim(&state, &notice(Event::Ready, "search@app", "ready"), 1_030), "another task goes out");
        assert!(claim(&state, &ready, 1_060), "after a minute it goes out again");
        assert!(!claim(&state, &ready, 1_061));
        fs::write(sent_file(&state), "not json").unwrap();
        assert!(claim(&state, &ready, 1_062), "a broken record never stops a notification");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_failing_or_hanging_command_is_an_error_to_log_not_a_panic() {
        assert_eq!(run_with_timeout("sh", &["-c".to_string(), "exit 0".to_string()], TIMEOUT), Ok(()));
        let failed = run_with_timeout("sh", &["-c".to_string(), "echo no route >&2; exit 7".to_string()], TIMEOUT).unwrap_err();
        assert!(failed.contains("7") && failed.contains("no route"), "{failed}");
        let started = Instant::now();
        let hung = run_with_timeout("sh", &["-c".to_string(), "sleep 30".to_string()], Duration::from_millis(200)).unwrap_err();
        assert!(hung.contains("did not finish"), "{hung}");
        assert!(started.elapsed() < Duration::from_secs(5), "the timeout stops the command");
        assert!(run_with_timeout("/no/such/program", &[], TIMEOUT).is_err());
        let state = temp("log");
        log_error(&state, "ready: notify.command exited with 7:\nno route");
        log_error(&state, "ready: again");
        let log = fs::read_to_string(log_file(&state)).unwrap();
        assert_eq!(log.lines().count(), 2);
        assert!(log.lines().next().unwrap().ends_with("ready: notify.command exited with 7: no route"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_text_is_never_part_of_the_command_line_the_shell_parses() {
        let hostile = "x\"; touch /tmp/kelpie-pwned; echo \"";
        let built = invocation("printf '%s' \"$1\"", &notice(Event::Ready, "a@b", hostile));
        assert_eq!(built.args[1], "printf '%s' \"$1\"", "the user's command is byte for byte theirs");
        assert_eq!(built.args[3], hostile);
        let dir = temp("hostile");
        let out = dir.join("out");
        let script = format!("printf '%s' \"$1\" > {}", out.display());
        let built = invocation(&script, &notice(Event::Ready, "a@b", hostile));
        assert!(Command::new(&built.program).args(&built.args).envs(built.env.iter().map(|(name, value)| (name, value))).status().unwrap().success());
        assert_eq!(fs::read_to_string(&out).unwrap(), hostile);
        assert!(!Path::new("/tmp/kelpie-pwned").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_sent_record_keeps_only_claims_still_within_the_minute() {
        let state = temp("prune");
        assert!(claim(&state, &notice(Event::Ready, "a@x", ""), 1000));
        assert!(claim(&state, &notice(Event::Ready, "b@x", ""), 1030));
        let record: serde_json::Value = serde_json::from_str(&fs::read_to_string(sent_file(&state)).unwrap()).unwrap();
        assert_eq!(record.as_object().unwrap().len(), 2);
        assert!(claim(&state, &notice(Event::Ready, "c@x", ""), 1070));
        let record: serde_json::Value = serde_json::from_str(&fs::read_to_string(sent_file(&state)).unwrap()).unwrap();
        assert_eq!(record.as_object().unwrap().keys().collect::<Vec<_>>(), ["ready:b@x", "ready:c@x"], "the minute-old claim is gone");
        assert!(claim(&state, &notice(Event::Summary, "lead", ""), 1070));
        assert!(!claim(&state, &notice(Event::Summary, "kelpie", ""), 1071), "summaries share one key, whoever sends them");
        fs::remove_dir_all(state).unwrap();
    }
}
