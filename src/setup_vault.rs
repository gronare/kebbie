use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Install {
    pub script: String,
    pub lookup: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub install: Option<Install>,
    pub command: Option<Vec<String>>,
    pub sign_in: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mindin {
    pub command: Vec<String>,
    pub version: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Signed {
    pub server: String,
    pub vault: String,
    pub vault_name: String,
    pub access: String,
    pub layout: Option<String>,
    pub notes: Option<u64>,
    pub already: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Code {
    pub uri: String,
    pub link: String,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub kind: String,
    pub message: String,
    pub errno: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    Code(Code),
    Signed(Signed),
    Move { files: u64, source: String },
    Moved { files: u64 },
    Done,
    Error(Failure),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    Installing(String),
    Installed(Result<Mindin, String>),
    Said(Said),
    Ended { code: Option<i32>, stderr: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Waiting,
    Installing,
    Starting,
    Code(Code),
    Move { files: u64, source: String },
    Moving,
    Done,
    Failed(Failure),
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

// One line of `mindin setup --server URL --json`; anything else on stdout is no event.
pub fn parse(line: &str) -> Option<Said> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    Some(match value["event"].as_str()? {
        "code" => {
            let uri = text(&value, "verification_uri");
            let link = Some(text(&value, "verification_uri_complete")).filter(|link| !link.is_empty()).unwrap_or_else(|| uri.clone());
            Said::Code(Code { uri, link, code: text(&value, "user_code") })
        }
        "signed_in" => Said::Signed(Signed {
            server: text(&value, "server"),
            vault: text(&value, "vault"),
            vault_name: text(&value, "vault_name"),
            access: text(&value, "access"),
            layout: value["layout"].as_str().map(str::to_string),
            notes: value["notes"].as_u64(),
            already: value["already"] == true,
        }),
        "move" => Said::Move { files: value["files"].as_u64().unwrap_or_default(), source: text(&value, "source") },
        "moved" => Said::Moved { files: value["files"].as_u64().unwrap_or_default() },
        "done" => Said::Done,
        "error" => Said::Error(Failure { kind: text(&value, "kind"), message: text(&value, "message"), errno: value["errno"].as_i64() }),
        _ => return None,
    })
}

struct Running {
    receiver: Receiver<Msg>,
    answers: Sender<String>,
    child: Arc<AtomicU32>,
    cancelled: Arc<AtomicBool>,
}

static INSTALLING: Mutex<()> = Mutex::new(());

pub struct Job {
    pub plan: Plan,
    pub state: State,
    pub progress: String,
    pub mindin: Option<Mindin>,
    pub signed: Option<Signed>,
    pub moved: Option<u64>,
    pub started: Instant,
    pub copied: bool,
    pub opened: bool,
    running: Option<Running>,
}

impl Job {
    pub fn new(plan: Plan) -> Job {
        Job { plan, state: State::Waiting, progress: String::new(), mindin: None, signed: None, moved: None, started: Instant::now(), copied: false, opened: false, running: None }
    }

    pub fn launch(&mut self) {
        if self.running.is_some() || self.state != State::Waiting {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let (answers, asked) = mpsc::channel();
        let child = Arc::new(AtomicU32::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let plan = self.plan.clone();
        let (pid, stop) = (child.clone(), cancelled.clone());
        std::thread::spawn(move || work(&plan, &sender, &asked, &pid, &stop));
        self.state = if self.plan.install.is_some() { State::Installing } else { State::Starting };
        self.running = Some(Running { receiver, answers, child, cancelled });
    }

    pub fn poll(&mut self) {
        let Some(running) = &self.running else { return };
        let said: Vec<Msg> = running.receiver.try_iter().collect();
        for msg in said {
            self.take(msg);
        }
    }

    pub fn take(&mut self, msg: Msg) {
        match msg {
            Msg::Installing(line) => {
                self.state = State::Installing;
                self.progress = line;
            }
            Msg::Installed(Ok(mindin)) => {
                self.mindin = Some(mindin);
                self.state = if self.plan.sign_in.is_some() { State::Starting } else { State::Done };
            }
            Msg::Installed(Err(cause)) => self.state = State::Failed(Failure { kind: "install".to_string(), message: cause, errno: None }),
            Msg::Said(Said::Code(code)) => self.state = State::Code(code),
            Msg::Said(Said::Signed(signed)) => {
                self.signed = Some(signed);
                self.state = State::Starting;
            }
            Msg::Said(Said::Move { files, source }) => self.state = State::Move { files, source },
            Msg::Said(Said::Moved { files }) => self.moved = Some(files),
            Msg::Said(Said::Done) => self.state = State::Done,
            Msg::Said(Said::Error(failure)) => self.state = State::Failed(failure),
            Msg::Ended { code, stderr } => {
                if !matches!(self.state, State::Done | State::Failed(_)) {
                    self.state = State::Failed(ended(code, &stderr));
                }
            }
        }
    }

    pub fn answer(&mut self, yes: bool) {
        if !matches!(self.state, State::Move { .. }) {
            return;
        }
        self.state = if yes { State::Moving } else { State::Starting };
        if let Some(running) = &self.running {
            let _ = running.answers.send(if yes { "yes" } else { "no" }.to_string());
        }
    }

    pub fn finished(&self) -> bool {
        self.state == State::Done
    }
}

impl Job {
    fn cancel(&mut self) {
        let Some(running) = &self.running else { return };
        running.cancelled.store(true, Ordering::SeqCst);
        let pid = running.child.load(Ordering::SeqCst);
        if pid != 0 && !matches!(self.state, State::Done | State::Failed(_)) {
            stop(pid);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

// The installer leads its own process group, so its curl and sh go with it; mindin's sign-in is a plain child.
fn stop(pid: u32) {
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

// A mindin too old for --json says so in argparse's words; anything else is its last line.
fn ended(code: Option<i32>, stderr: &str) -> Failure {
    let last = stderr.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or_default();
    if stderr.contains("unrecognized arguments: --json") {
        return Failure { kind: "old".to_string(), message: "this mindin cannot show its sign-in here".to_string(), errno: None };
    }
    let message = match (code, last.strip_prefix("mindin: ").unwrap_or(last)) {
        (_, said) if !said.is_empty() && !said.contains("Traceback") => said.to_string(),
        (Some(code), _) => format!("mindin exited with status {code}"),
        (None, _) => "mindin was stopped".to_string(),
    };
    Failure { kind: "stopped".to_string(), message, errno: None }
}

fn strip_escapes(line: &str) -> String {
    let mut shown = String::new();
    let mut characters = line.chars();
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            for next in characters.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if !character.is_control() {
            shown.push(character);
        }
    }
    shown.trim().to_string()
}

fn version(program: &str) -> String {
    let said = crate::sh::run(program, &["--version"], None).unwrap_or_default();
    said.split_whitespace().find(|word| word.starts_with(|character: char| character.is_ascii_digit())).unwrap_or_default().to_string()
}

pub fn install(install: &Install, said: &dyn Fn(String)) -> Result<Mindin, String> {
    install_watched(install, said, &AtomicU32::new(0), &AtomicBool::new(false))
}

// One installer at a time: one a cancelled job left behind is waited out before the next starts.
fn install_watched(install: &Install, said: &dyn Fn(String), pid: &AtomicU32, cancelled: &AtomicBool) -> Result<Mindin, String> {
    let _alone = INSTALLING.lock().unwrap_or_else(PoisonError::into_inner);
    if cancelled.load(Ordering::SeqCst) {
        return Err("mindin's install was cancelled".to_string());
    }
    let mut child = Command::new("sh").args(["-c", &format!("({}) 2>&1", install.script)]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0).spawn().map_err(|error| format!("sh: {error}"))?;
    pid.store(child.id(), Ordering::SeqCst);
    if cancelled.load(Ordering::SeqCst) {
        stop(child.id());
    }
    let mut last = String::new();
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
            let shown = strip_escapes(&line);
            if !shown.is_empty() {
                last.clone_from(&shown);
                said(shown);
            }
        }
    }
    let status = child.wait().map_err(|error| format!("sh: {error}"));
    pid.store(0, Ordering::SeqCst);
    let status = status?;
    if !status.success() {
        let cause = if last.is_empty() { format!("it exited with status {}", status.code().unwrap_or(-1)) } else { last };
        return Err(format!("mindin's installer stopped: {cause}"));
    }
    let found = crate::doctor::which("mindin", &install.lookup).ok_or("mindin's installer ran, but mindin is neither on PATH nor in ~/.local/bin")?;
    let program = found.to_string_lossy().to_string();
    Ok(Mindin { version: version(&program), command: vec![program] })
}

fn sign_in(command: &[String], flags: &[String], sender: &Sender<Msg>, asked: &Receiver<String>, pid: &AtomicU32, cancelled: &AtomicBool) {
    let Some((program, rest)) = command.split_first() else { return };
    if cancelled.load(Ordering::SeqCst) {
        return;
    }
    let spawned = Command::new(program).args(rest).arg("setup").args(flags).arg("--json").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            let _ = sender.send(Msg::Ended { code: Some(127), stderr: format!("{program}: {error}") });
            return;
        }
    };
    pid.store(child.id(), Ordering::SeqCst);
    if cancelled.load(Ordering::SeqCst) {
        stop(child.id());
    }
    let stderr = child.stderr.take().map(|mut stderr| std::thread::spawn(move || {
        let mut said = String::new();
        let _ = stderr.read_to_string(&mut said);
        said
    }));
    let mut stdin = child.stdin.take();
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
            let Some(said) = parse(&line) else { continue };
            let asks = matches!(said, Said::Move { .. });
            let _ = sender.send(Msg::Said(said));
            if asks {
                let answer = asked.recv().unwrap_or_else(|_| "no".to_string());
                if let Some(input) = stdin.as_mut() {
                    let _ = writeln!(input, "{answer}").and_then(|()| input.flush());
                }
            }
        }
    }
    drop(stdin);
    let code = child.wait().ok().and_then(|status| status.code());
    pid.store(0, Ordering::SeqCst);
    let stderr = stderr.and_then(|reader| reader.join().ok()).unwrap_or_default();
    let _ = sender.send(Msg::Ended { code, stderr });
}

fn work(plan: &Plan, sender: &Sender<Msg>, asked: &Receiver<String>, pid: &AtomicU32, cancelled: &AtomicBool) {
    let mut command = plan.command.clone();
    if let Some(script) = &plan.install {
        let installed = install_watched(script, &|line| {
            let _ = sender.send(Msg::Installing(line));
        }, pid, cancelled);
        if cancelled.load(Ordering::SeqCst) {
            return;
        }
        command = installed.as_ref().ok().map(|mindin| mindin.command.clone()).or(command);
        let failed = installed.is_err();
        let _ = sender.send(Msg::Installed(installed));
        if failed {
            return;
        }
    }
    if let (Some(flags), Some(command)) = (&plan.sign_in, command) {
        sign_in(&command, flags, sender, asked, pid, cancelled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_lines_read_as_events_and_anything_else_is_none() {
        assert_eq!(parse(r#"{"event":"code","verification_uri":"https://v.example/device","verification_uri_complete":"","user_code":"WVVD-XQNR","expires_in":600}"#), Some(Said::Code(Code { uri: "https://v.example/device".to_string(), link: "https://v.example/device".to_string(), code: "WVVD-XQNR".to_string() })));
        let signed = parse(r#"{"event":"signed_in","server":"https://v.example","vault":"work","vault_name":"Work","access":"read and write","already":false,"layout":"default","notes":12}"#);
        assert_eq!(signed, Some(Said::Signed(Signed { server: "https://v.example".to_string(), vault: "work".to_string(), vault_name: "Work".to_string(), access: "read and write".to_string(), layout: Some("default".to_string()), notes: Some(12), already: false })));
        assert_eq!(parse(r#"{"event":"error","kind":"unreachable","message":"[Errno 65] No route to host","errno":65}"#), Some(Said::Error(Failure { kind: "unreachable".to_string(), message: "[Errno 65] No route to host".to_string(), errno: Some(65) })));
        assert_eq!((parse("Anonymous usage data: off"), parse(r#"{"event":"later"}"#)), (None, None));
    }

    #[test]
    fn a_mindin_without_json_is_named_old_and_another_stop_by_its_last_line() {
        assert_eq!(ended(Some(2), "usage: mindin [-h]\nmindin: error: unrecognized arguments: --json\n").kind, "old");
        assert_eq!(ended(Some(1), "mindin: the move stopped: disk full\n").message, "the move stopped: disk full");
        assert_eq!(ended(Some(3), "").message, "mindin exited with status 3");
    }

    #[test]
    fn the_install_and_sign_in_run_with_their_output_captured_and_the_move_answered_on_stdin() {
        let dir = crate::sh::test_dir("setup-vault-job");
        let bin = dir.join("bin");
        let fake = format!("#!/bin/sh\n[ \"$1\" = --version ] && echo 'mindin 0.21.0' && exit 0\necho '{{\"event\":\"signed_in\",\"server\":\"https://v.example\",\"vault\":\"work\",\"vault_name\":\"Work\",\"access\":\"read and write\",\"already\":true}}'\necho '{{\"event\":\"move\",\"files\":4,\"source\":\"the folder /notes\"}}'\nread answer\necho \"$answer\" > '{}'\necho 'Verified 4 files' >&2\necho '{{\"event\":\"moved\",\"files\":4}}'\necho '{{\"event\":\"done\",\"vault\":\"the server https://v.example\"}}'\n", dir.join("answer").display());
        let script = format!("mkdir -p '{0}' && cat > '{0}/mindin' <<'EOF'\n{fake}EOF\nchmod +x '{0}/mindin'; printf '\\033[32m→\\033[0m installing mindin\\n'", bin.display());
        let mut job = Job::new(Plan { install: Some(Install { script, lookup: vec![bin.clone()] }), command: None, sign_in: Some(vec!["--server".to_string(), "https://v.example".to_string()]) });
        job.launch();
        let waited = Instant::now();
        while !matches!(job.state, State::Done | State::Failed(_)) && waited.elapsed().as_secs() < 20 {
            job.poll();
            if matches!(job.state, State::Move { .. }) {
                job.answer(true);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(job.state, State::Done);
        assert_eq!(job.progress, "→ installing mindin", "the installer's lines show without their colours");
        assert_eq!(job.mindin.as_ref().map(|mindin| mindin.version.as_str()), Some("0.21.0"));
        assert_eq!((job.signed.as_ref().map(|signed| signed.vault_name.as_str()), job.moved), (Some("Work"), Some(4)));
        assert_eq!(std::fs::read_to_string(dir.join("answer")).unwrap().trim(), "yes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_job_left_during_its_install_never_signs_in_and_reports_nothing() {
        let dir = crate::sh::test_dir("setup-vault-cancel");
        let bin = dir.join("bin");
        let (signed, began) = (dir.join("signed"), dir.join("began"));
        let fake = format!("#!/bin/sh\n[ \"$1\" = --version ] && echo 'mindin 0.21.0' && exit 0\ntouch '{}'\n", signed.display());
        let script = format!("trap '' TERM; touch '{began}'; echo installing; sleep 1; mkdir -p '{0}' && cat > '{0}/mindin' <<'EOF'\n{fake}EOF\nchmod +x '{0}/mindin'", bin.display(), began = began.display());
        let mut job = Job::new(Plan { install: Some(Install { script, lookup: vec![bin.clone()] }), command: None, sign_in: Some(vec!["--server".to_string(), "https://old.example".to_string()]) });
        job.launch();
        let waited = Instant::now();
        while !began.exists() {
            assert!(waited.elapsed().as_secs() < 20, "the installer never started");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        job.cancel();
        let running = job.running.take().expect("it runs");
        let mut said = Vec::new();
        loop {
            match running.receiver.recv_timeout(std::time::Duration::from_secs(20)) {
                Ok(msg) => said.push(msg),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("the work never ended"),
            }
        }
        assert!(bin.join("mindin").exists(), "the installer that ignores the stop runs to its end");
        assert!(!signed.exists(), "the cancelled job never signs in");
        assert!(!said.iter().any(|msg| matches!(msg, Msg::Installed(_) | Msg::Said(_))), "nothing settles after it was left: {said:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
