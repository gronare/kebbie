use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use kebbie_hub_api::WorkerOs;
use kebbie_hub_protocol::{AccountKey, BlobRef, Id, Job, JobEnd, JobReport, MAX_JOB_LOG_BYTES, MAX_JOB_REASON_CHARS, MAX_MACHINE_MESSAGE_BYTES, Payload, PublicKeys};
use serde::{Deserialize, Serialize};

use super::{bundle, cluster, safe};
use crate::hub::http::{CALL, Client, HubError};
use crate::hub::identity::{self, Identity};
use crate::sh::Result;

const QUEUE: &str = "jobs/queue";
const SEEN: &str = "jobs/seen.json";
const SEEN_LOCK: &str = "jobs/seen.lock";
const KEEP_SEEN: u64 = 2 * 86_400;
const EVERY_MS: u64 = 2_000;
const FULL_GAP_MS: u64 = 1_000;
const HEARTBEAT_MS: u64 = 60_000;
const MAX_HELD: usize = 256 * 1024;
const RENEW: u64 = 600;
const TICK: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Queued {
    pub from: Id,
    pub from_name: String,
    pub id: Id,
    pub job: Job,
    pub at: u64,
    #[serde(default)]
    pub seq: u32,
}

// The jobs this worker took, by sender and id, so one the inbox hands over again is never queued twice.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    #[serde(default)]
    pub seen: BTreeMap<String, u64>,
    #[serde(default)]
    pub queued: u64,
}

impl Seen {
    pub fn first(&mut self, from: &Id, id: &Id, now: u64) -> bool {
        self.seen.retain(|_, at| now.saturating_sub(*at) < KEEP_SEEN);
        let key = format!("{from} {id}");
        if self.seen.contains_key(&key) {
            return false;
        }
        self.seen.insert(key, now);
        true
    }
}

fn load_seen(dir: &Path) -> Seen {
    std::fs::read_to_string(dir.join(SEEN)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub fn take(dir: &Path, from: &Id, from_name: &str, id: &Id, job: Job, now: u64) -> Option<String> {
    let said = format!("{} for {} ({}) from {from_name}", job.name, job.repo, job.task);
    let held = crate::state::Locked::wait(&dir.join(SEEN_LOCK));
    let mut seen = load_seen(dir);
    if !seen.first(from, id, now) {
        return Some(format!("jobs: {said} came again; it is queued or ran already"));
    }
    let queued = Queued { from: from.clone(), from_name: from_name.to_string(), id: id.clone(), job, at: now, seq: 0 };
    seen.queued += 1;
    let file = dir.join(QUEUE).join(format!("{:020}-{id}.json", seen.queued));
    let written = serde_json::to_string(&queued).map_err(|error| error.to_string()).and_then(|text| crate::state::write(&file, text))
        .and_then(|()| serde_json::to_string(&seen).map_err(|error| error.to_string()).and_then(|text| crate::state::write(&dir.join(SEEN), text)));
    drop(held);
    Some(match written {
        Ok(()) => format!("jobs: queued {said}"),
        Err(error) => format!("jobs: {said} was not queued: {error}"),
    })
}

fn all_queued(dir: &Path) -> Vec<(PathBuf, Queued)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join(QUEUE)).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|extension| extension == "json")).collect();
    files.sort();
    files.into_iter().filter_map(|file| match std::fs::read_to_string(&file).ok().and_then(|text| serde_json::from_str::<Queued>(&text).ok()) {
        Some(queued) => Some((file, queued)),
        None => {
            let _ = std::fs::remove_file(&file);
            None
        }
    }).collect()
}

// A job that waited in the queue longer than the protocol lets a job live is refused, not run long after its master stopped asking.
pub fn stale(queued: &Queued, now: u64) -> bool {
    now.saturating_sub(queued.at) > kebbie_hub_protocol::Kind::Job.max_ttl()
}

pub fn next_queued(dir: &Path) -> Option<(PathBuf, Queued)> {
    all_queued(dir).into_iter().next()
}

// The hub takes a report of at most 32 KiB as JSON, where a control character costs six bytes, so the log keeps room for the rest of the report, a 500-character reason included.
const LOG_JSON_BYTES: usize = MAX_MACHINE_MESSAGE_BYTES - 4 * 1024;

fn json_len(character: char) -> usize {
    match character {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        control if u32::from(control) < 0x20 => 6,
        other => other.len_utf8(),
    }
}

// Where the next report's log ends: at most 12 KiB of output, and at most LOG_JSON_BYTES once encoded.
fn cut(text: &str) -> usize {
    let mut encoded = 0;
    for (at, character) in text.char_indices() {
        encoded += json_len(character);
        if at + character.len_utf8() > MAX_JOB_LOG_BYTES || encoded > LOG_JSON_BYTES {
            return at;
        }
    }
    text.len()
}

// A report the hub would refuse goes without its log, with a line saying so, so a final report is never lost to its size.
pub fn sendable(mut report: JobReport) -> JobReport {
    if Payload::JobReport(report.clone()).validate().is_err() {
        report.log = format!("[kebbie: {} bytes of output did not fit a report and were left out]\n", report.log.len());
    }
    report
}

// The job's output on its way to the master: sent every 2 s, or once 12 KiB wait, but never faster than once a second.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Batch {
    held: String,
    last: u64,
}

impl Batch {
    pub fn new(now_ms: u64) -> Batch {
        Batch { held: String::new(), last: now_ms }
    }

    pub fn add(&mut self, text: &str) {
        self.held.push_str(text);
        if self.held.len() > MAX_HELD {
            let over = self.held.len() - MAX_HELD;
            let mut at = over;
            while !self.held.is_char_boundary(at) {
                at += 1;
            }
            self.held = format!("[kebbie: {at} bytes of output left out, as the hub took it slower than the job wrote it]\n{}", &self.held[at..]);
        }
    }

    pub fn due(&self, now_ms: u64) -> bool {
        let waited = now_ms.saturating_sub(self.last);
        !self.held.is_empty() && (waited >= EVERY_MS || (self.held.len() >= MAX_JOB_LOG_BYTES && waited >= FULL_GAP_MS))
    }

    pub fn take(&mut self, now_ms: u64) -> String {
        self.last = now_ms;
        let at = cut(&self.held);
        self.held.drain(..at).collect()
    }

    pub fn rest(&mut self) -> Vec<String> {
        let mut parts = Vec::new();
        while !self.held.is_empty() {
            let at = cut(&self.held);
            parts.push(self.held.drain(..at).collect());
        }
        parts
    }
}

pub fn reason(text: &str) -> String {
    let plain: String = text.chars().map(|character| if character.is_control() && character != '\n' { ' ' } else { character }).collect();
    let plain = plain.trim();
    let kept: String = plain.chars().take(MAX_JOB_REASON_CHARS).collect();
    if kept.is_empty() { "no reason was given".to_string() } else { kept }
}

fn failed(text: &str) -> JobEnd {
    JobEnd::Failed { reason: reason(text) }
}

// The job's commit in a folder of the task's own, reused and cleaned per job; None when the store lacks the base the bundle builds on.
fn checkout(data: &Path, queued: &Queued, bundle: &[u8], remote: Option<&str>, say: &mut dyn FnMut(String), stop: &dyn Fn() -> bool) -> std::result::Result<Option<PathBuf>, String> {
    let job = &queued.job;
    let repo = data.join(safe(&job.repo));
    let keep = format!("refs/kebbie/from/{}", queued.from);
    if bundle::store(&repo.join(".store"), bundle, &job.commit, job.base.as_deref(), &keep, remote, say, stop).map_err(|error| format!("the worker could not take the commit: {error}"))? == bundle::Stored::NeedsFull {
        return Ok(None);
    }
    let store = repo.join(".store");
    let work = repo.join(safe(&job.task));
    let reused = work.join(".git").exists() && bundle::git_until(&work, &["checkout", "--quiet", "--detach", "--force", &job.commit], stop, |_| {}).and_then(|_| bundle::git_until(&work, &["clean", "-fdq"], stop, |_| {})).is_ok();
    if !reused {
        let _ = std::fs::remove_dir_all(&work);
        let _ = crate::repo::git(&store, &["worktree", "prune"]);
        bundle::git_until(&store, &["worktree", "add", "--quiet", "--detach", &work.to_string_lossy(), &job.commit], stop, |_| {}).map_err(|error| format!("the worker could not check out the commit: {error}"))?;
    }
    Ok(Some(work))
}

fn signal_group(pid: u32, signal: libc::c_int) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: signals the process group this worker started for the job.
        unsafe { libc::kill(-pid, signal) };
    }
}

// How long a job may run, how long it gets between SIGTERM and SIGKILL, and how long its output is still read once its command ended.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub timeout: Duration,
    pub grace: Duration,
    pub drain: Duration,
}

impl Limits {
    pub const WORKER: Limits = Limits { timeout: Duration::from_secs(super::TIMEOUT_MINUTES * 60), grace: Duration::from_secs(10), drain: Duration::from_secs(2) };
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Why {
    Halted,
    TimedOut,
}

fn span(duration: Duration) -> String {
    match duration.as_secs() {
        seconds if seconds >= 60 => format!("{} minutes", seconds / 60),
        seconds => format!("{seconds} s"),
    }
}

fn start(queued: &Queued, work: &Path, command: &str) -> std::io::Result<std::process::Child> {
    let job = &queued.job;
    Command::new("sh").args(["-c", &format!("exec 2>&1\n{{ {command}\n}}")]).current_dir(work)
        .envs([("KEBBIE_JOB", job.name.as_str()), ("KEBBIE_JOB_TASK", job.task.as_str()), ("KEBBIE_JOB_SHA", job.commit.as_str()), ("KEBBIE_JOB_MASTER", queued.from_name.as_str())])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0).spawn()
}

fn lines_of(child: &mut std::process::Child) -> mpsc::Receiver<String> {
    let (lines, read) = mpsc::channel::<String>();
    if let Some(output) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(output).split(b'\n').map_while(std::result::Result::ok) {
                if lines.send(format!("{}\n", String::from_utf8_lossy(&line))).is_err() {
                    return;
                }
            }
        });
    }
    read
}

// Reads the job's output until its command ended and the output closed, or the drain after the command ended ran out, as when something it put in the background holds the output; a stop or the timeout sends SIGTERM to its process group and SIGKILL after the grace.
fn watch(child: &mut std::process::Child, read: &mpsc::Receiver<String>, batch: &mut Batch, send: &mut dyn FnMut(String, Option<JobEnd>), stop: &dyn Fn() -> bool, clock: &dyn Fn() -> u64, limits: Limits) -> (std::io::Result<std::process::ExitStatus>, Option<Why>, bool) {
    let ms = |duration: Duration| duration.as_millis() as u64;
    let started = clock();
    let (mut why, mut termed, mut killed, mut closed, mut exited) = (None, None::<u64>, false, false, None::<(std::process::ExitStatus, u64)>);
    loop {
        if closed {
            std::thread::sleep(Duration::from_millis(100));
        } else {
            match read.recv_timeout(Duration::from_millis(100)) {
                Ok(text) => batch.add(&text),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => closed = true,
            }
        }
        let now = clock();
        if exited.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => exited = Some((status, now)),
                Ok(None) => {}
                Err(error) => return (Err(error), why, killed),
            }
        }
        if let Some((status, at)) = exited && (closed || now.saturating_sub(at) >= ms(limits.drain)) {
            return (Ok(status), why, killed);
        }
        if why.is_none() {
            why = if stop() { Some(Why::Halted) } else if now.saturating_sub(started) >= ms(limits.timeout) { Some(Why::TimedOut) } else { None };
        }
        if why.is_some() && termed.is_none() {
            signal_group(child.id(), libc::SIGTERM);
            termed = Some(now);
        }
        if let Some(at) = termed && !killed && now.saturating_sub(at) >= ms(limits.grace) {
            signal_group(child.id(), libc::SIGKILL);
            killed = true;
        }
        if batch.due(now) {
            let text = batch.take(now);
            send(text, None);
        }
    }
}

pub fn execute(queued: &Queued, data: &Path, fetch: &dyn Fn(&[BlobRef]) -> Result<Vec<u8>>, send: &mut dyn FnMut(String, Option<JobEnd>), stop: &dyn Fn() -> bool, clock: &dyn Fn() -> u64, limits: Limits) -> JobEnd {
    let end = |send: &mut dyn FnMut(String, Option<JobEnd>), end: JobEnd| {
        send(String::new(), Some(end.clone()));
        end
    };
    let job = &queued.job;
    let bundle = if job.bundle.is_empty() { Vec::new() } else {
        match fetch(&job.bundle) {
            Ok(bundle) => bundle,
            Err(error) => return end(send, failed(&format!("the worker could not fetch the bundle: {error}"))),
        }
    };
    let command = super::command_of(&job.command);
    let timeout = command.timeout.map_or(limits.timeout, |minutes| Duration::from_secs(minutes * 60));
    let deadline = clock().saturating_add(timeout.as_millis() as u64);
    let late = || clock() >= deadline;
    let checked = checkout(data, queued, &bundle, command.fetch.as_deref(), &mut |text| send(text, None), &|| stop() || late());
    let work = match checked {
        Ok(Some(work)) => work,
        Ok(None) => return end(send, JobEnd::NeedsFull {}),
        Err(_) if stop() => return end(send, failed(&format!("the worker's kebbie stopped while it took the commit of {}, so it stopped the job", job.name))),
        Err(_) if late() => return end(send, failed(&format!("{} ran past its timeout of {} while the worker took its commit, so the worker stopped it", job.name, span(timeout)))),
        Err(error) => return end(send, failed(&error)),
    };
    let limits = Limits { timeout: Duration::from_millis(deadline.saturating_sub(clock())), ..limits };
    let mut child = match start(queued, &work, command.run) {
        Ok(child) => child,
        Err(error) => return end(send, failed(&format!("the job's command did not start: {error}"))),
    };
    let read = lines_of(&mut child);
    let mut batch = Batch::new(clock());
    let (status, why, killed) = watch(&mut child, &read, &mut batch, send, stop, clock, limits);
    signal_group(child.id(), libc::SIGKILL);
    while let Ok(text) = read.try_recv() {
        batch.add(&text);
    }
    let how = if killed { format!(", with SIGKILL as it outlasted SIGTERM by {}", span(limits.grace)) } else { String::new() };
    let ended = match (why, status) {
        (Some(Why::Halted), _) => failed(&format!("the worker's kebbie stopped while {} ran, so it stopped the job{how}", job.name)),
        (Some(Why::TimedOut), _) => failed(&format!("{} ran past its timeout of {}, so the worker stopped it{how}", job.name, span(timeout))),
        (None, Ok(status)) => JobEnd::Exited { code: status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0)), artifacts: Vec::new() },
        (None, Err(error)) => failed(&format!("the job's command was lost: {error}")),
    };
    let mut rest = batch.rest();
    let last = rest.pop().unwrap_or_default();
    for text in rest {
        send(text, None);
    }
    send(last, Some(ended.clone()));
    ended
}

fn deliver(client: &Client, identity: &Identity, account: &AccountKey, to: &Id, keys: &PublicKeys, report: &JobReport, halted: &(dyn Fn() -> bool + Sync)) -> Result<()> {
    let mut last = String::new();
    for attempt in 0..5u32 {
        let envelope = cluster::seal_to(identity, account, to, keys, cluster::new_id()?, &Payload::JobReport(report.clone()), crate::messages::now())?;
        match cluster::post(client, &envelope) {
            Ok(_) => return Ok(()),
            Err(error) if error.is_final() => return Err(error.to_string()),
            Err(error) => last = error.to_string(),
        }
        if halted() && report.end.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_secs(1 << attempt));
    }
    Err(last)
}

fn ended_as(end: &JobEnd) -> String {
    match end {
        JobEnd::Exited { code, .. } => format!("exited {code}"),
        JobEnd::NeedsFull {} => "asked for the whole history".to_string(),
        JobEnd::Refused { reason } => format!("refused: {reason}"),
        JobEnd::Failed { reason } => format!("failed: {reason}"),
    }
}

// The reports on one job, numbered on from the ones it had while queued, and when the last one went.
struct Outbox {
    seq: u32,
    last_ms: u64,
    lost: Option<String>,
    ended: bool,
}

// Runs the job while a second thread sends an empty report each minute nothing else went, from the bundle's fetch on, and keeps the jobs queued behind it reported on too, so their masters do not give up.
#[allow(clippy::too_many_arguments)]
pub fn supervise(queued: &Queued, data: &Path, fetch: &dyn Fn(&[BlobRef]) -> Result<Vec<u8>>, post: &(dyn Fn(&JobReport) -> Result<()> + Sync), beat_queued: &(dyn Fn() + Sync), every: Duration, halted: &(dyn Fn() -> bool + Sync), clock: &(dyn Fn() -> u64 + Sync), limits: Limits) -> (JobEnd, Option<String>) {
    let outbox = Mutex::new(Outbox { seq: queued.seq, last_ms: clock(), lost: None, ended: false });
    let send = |log: String, end: Option<JobEnd>| {
        let mut out = outbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if out.ended {
            return;
        }
        out.ended = end.is_some();
        let report = sendable(JobReport { job: queued.id.clone(), seq: out.seq, log, end });
        out.seq += 1;
        if let Err(error) = post(&report) {
            out.lost.get_or_insert(error);
        }
        out.last_ms = clock();
    };
    let done = AtomicBool::new(false);
    let end = std::thread::scope(|scope| {
        scope.spawn(|| {
            let every_ms = every.as_millis() as u64;
            let mut queue_beat = clock();
            while !done.load(Ordering::SeqCst) {
                std::thread::sleep(every.min(Duration::from_millis(200)));
                let now = clock();
                let last = outbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).last_ms;
                if !done.load(Ordering::SeqCst) && now.saturating_sub(last) >= every_ms {
                    send(String::new(), None);
                }
                if now.saturating_sub(queue_beat) >= every_ms {
                    beat_queued();
                    queue_beat = now;
                }
            }
        });
        let end = execute(queued, data, fetch, &mut |log, end| send(log, end), halted, clock, limits);
        done.store(true, Ordering::SeqCst);
        end
    });
    (end, outbox.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner).lost)
}

// An empty report on each job waiting behind the one that runs, the first saying it waits, each numbered on in its queue file so the job's own reports follow on.
pub fn beat_queued(dir: &Path, running: Option<&Path>, me: &str, post: &dyn Fn(&Queued, &JobReport) -> Result<()>) {
    for (file, mut queued) in all_queued(dir).into_iter().filter(|(file, _)| Some(file.as_path()) != running) {
        let log = if queued.seq == 0 { format!("[kebbie: {me} took this job; it waits for the job before it to end]\n") } else { String::new() };
        if post(&queued, &JobReport { job: queued.id.clone(), seq: queued.seq, log, end: None }).is_ok() {
            queued.seq += 1;
            if let Ok(text) = serde_json::to_string(&queued) {
                let _ = crate::state::write(&file, text);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_one(dir: &Path, identity: &Identity, account: &AccountKey, client: &Client, data: &Path, file: &Path, queued: &Queued, halted: &(dyn Fn() -> bool + Sync)) -> String {
    let said = format!("jobs: {} for {} ({}) from {}", queued.job.name, queued.job.repo, queued.job.task, queued.from_name);
    let keys = match cluster::sender(dir, client, account, &queued.from, false) {
        Ok((keys, _)) => keys,
        Err(error) => return format!("{said} did not run, as no report could go back: {error}"),
    };
    let posting = Mutex::new(Client::new(&identity.url, Some(identity.token()), CALL));
    let post = |report: &JobReport| deliver(&posting.lock().unwrap_or_else(std::sync::PoisonError::into_inner), identity, account, &queued.from, &keys, report, halted);
    let beat_queued = || {
        let client = posting.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        beat_queued(dir, Some(file), &identity.name, &|waiting, report| cluster::sender(dir, &client, account, &waiting.from, false).and_then(|(keys, _)| deliver(&client, identity, account, &waiting.from, &keys, report, halted)));
    };
    let fetch = |parts: &[BlobRef]| bundle::download(client, &mut bundle::Calls::new(&crate::typing::now_ms, &std::thread::sleep, halted), parts);
    let (end, lost) = supervise(queued, data, &fetch, &post, &beat_queued, Duration::from_millis(HEARTBEAT_MS), halted, &crate::typing::now_ms, Limits::WORKER);
    format!("{said}: {}{}", ended_as(&end), lost.map(|error| format!("; a report did not reach the master: {error}")).unwrap_or_default())
}

pub fn refuse(dir: &Path, identity: &Identity, account: &AccountKey, client: &Client, queued: &Queued, why: &str) -> String {
    let said = format!("jobs: refused {} for {} from {}: {why}", queued.job.name, queued.job.repo, queued.from_name);
    let report = JobReport { job: queued.id.clone(), seq: queued.seq, log: String::new(), end: Some(JobEnd::Refused { reason: reason(why) }) };
    match cluster::sender(dir, client, account, &queued.from, false).and_then(|(keys, _)| deliver(client, identity, account, &queued.from, &keys, &report, &|| true)) {
        Ok(()) => said,
        Err(error) => format!("{said}; the refusal did not reach the master: {error}"),
    }
}

pub fn os() -> String {
    match std::env::consts::OS {
        "macos" => "macos".to_string(),
        "linux" => "linux".to_string(),
        other => super::token(other).replace('.', "-"),
    }
}

pub(super) fn register(client: &Client, enabled: bool, registered: Option<(bool, u64)>, now: u64) -> std::result::Result<Option<(bool, u64)>, HubError> {
    match (enabled, registered) {
        (true, Some((true, at))) if now.saturating_sub(at) < RENEW => Ok(registered),
        (true, _) => client.put::<_, WorkerOs>("/v1/m/worker", &WorkerOs { os: os() }).map(|_| Some((true, now))),
        (false, Some((false, _))) => Ok(registered),
        (false, _) => client.delete("/v1/m/worker").map(|()| Some((false, now))),
    }
}

static HALT: AtomicBool = AtomicBool::new(false);
static REPLACED: AtomicBool = AtomicBool::new(false);
static THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

fn halted() -> bool {
    crate::keeper::stopping() || HALT.load(Ordering::SeqCst)
}

fn pause() {
    let until = Instant::now() + TICK;
    while !halted() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn serve(state: &Path) {
    let log = |text: &str| crate::keeper::log_line(state, text);
    let mut registered: [Option<(bool, u64)>; 2] = [None, None];
    let mut enabled = None;
    while !halted() {
        if let Ok(now_enabled) = crate::config::worker_enabled() {
            enabled = Some(now_enabled);
        }
        let Some(enabled) = enabled else {
            pause();
            continue;
        };
        for (slot, dev) in [false, true].into_iter().enumerate() {
            let dir = identity::dir_of(state, dev);
            let Some((identity, account)) = identity::load_at(&dir).ok().flatten().and_then(|identity| identity.account.clone().map(|account| (identity, account.key))) else { continue };
            let client = Client::new(&identity.url, Some(identity.token()), CALL);
            match register(&client, enabled, registered[slot], crate::messages::now()) {
                Ok(now) => {
                    if now.map(|(on, _)| on) != registered[slot].map(|(on, _)| on) {
                        log(&format!("jobs: {} the hub's workers", if enabled { "this machine is one of" } else { "this machine left" }));
                    }
                    registered[slot] = now;
                }
                Err(error) => log(&format!("jobs: the hub did not take worker.enabled = {enabled}: {error}")),
            }
            if let Some((file, queued)) = next_queued(&dir) {
                let now = crate::messages::now();
                let note = if stale(&queued, now) {
                    refuse(&dir, &identity, &account, &client, &queued, &format!("it waited {} hours in {}'s queue, past the day a job lives", now.saturating_sub(queued.at) / 3_600, identity.name))
                } else if enabled { run_one(&dir, &identity, &account, &client, &super::data_dir(), &file, &queued, &halted) } else { refuse(&dir, &identity, &account, &client, &queued, &format!("{} runs no jobs: worker.enabled is off there", identity.name)) };
                let _ = std::fs::remove_file(file);
                log(&note);
            }
        }
        pause();
    }
    if REPLACED.load(Ordering::SeqCst) {
        return;
    }
    for (slot, dev) in [false, true].into_iter().enumerate() {
        if registered[slot].is_some_and(|(on, _)| on)
            && let Ok(Some(identity)) = identity::load_at(&identity::dir_of(state, dev))
        {
            let _ = Client::new(&identity.url, Some(identity.token()), CALL).delete("/v1/m/worker");
        }
    }
}

pub fn spawn(state: &Path) {
    let kept = state.to_path_buf();
    HALT.store(false, Ordering::SeqCst);
    match std::thread::Builder::new().name("jobs-worker".to_string()).spawn(move || serve(&kept)) {
        Ok(handle) => *THREAD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle),
        Err(error) => crate::keeper::log_line(state, &format!("jobs: the worker thread did not start: {error}")),
    }
}

// The keeper is going: a running job is stopped and its end reported, and unless a newer keeper takes over, the hub hears this machine is no worker.
pub fn stop(replaced: bool) {
    REPLACED.store(replaced, Ordering::SeqCst);
    HALT.store(true, Ordering::SeqCst);
    let Some(handle) = THREAD.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() else { return };
    let until = Instant::now() + Duration::from_secs(20);
    while !handle.is_finished() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    if handle.is_finished() {
        let _ = handle.join();
    }
}
