use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use kebbie_hub_api::WorkerOs;
use kebbie_hub_protocol::{AccountKey, BlobRef, Id, Job, JobEnd, JobReport, MAX_JOB_LOG_BYTES, MAX_JOB_REASON_CHARS, Payload, PublicKeys};
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
    let queued = Queued { from: from.clone(), from_name: from_name.to_string(), id: id.clone(), job, at: now };
    seen.queued += 1;
    let file = dir.join(QUEUE).join(format!("{:020}-{id}.json", seen.queued));
    let written = serde_json::to_string(&queued).map_err(|error| error.to_string()).and_then(|text| crate::state::write(&file, text));
    let kept = serde_json::to_string(&seen).map_err(|error| error.to_string()).and_then(|text| crate::state::write(&dir.join(SEEN), text));
    drop(held);
    Some(match written.and(kept) {
        Ok(()) => format!("jobs: queued {said}"),
        Err(error) => format!("jobs: {said} was not queued: {error}"),
    })
}

pub fn next_queued(dir: &Path) -> Option<(PathBuf, Queued)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join(QUEUE)).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|extension| extension == "json")).collect();
    files.sort();
    files.into_iter().find_map(|file| match std::fs::read_to_string(&file).ok().and_then(|text| serde_json::from_str::<Queued>(&text).ok()) {
        Some(queued) => Some((file, queued)),
        None => {
            let _ = std::fs::remove_file(&file);
            None
        }
    })
}

fn cut(text: &str, max: usize) -> usize {
    let mut at = max.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

// The job's output on its way to the master: sent every 2 s, or once 12 KiB wait, but never faster than once a second, and an empty report each minute of silence so the master knows the job still runs.
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
        (!self.held.is_empty() && (waited >= EVERY_MS || (self.held.len() >= MAX_JOB_LOG_BYTES && waited >= FULL_GAP_MS))) || waited >= HEARTBEAT_MS
    }

    pub fn take(&mut self, now_ms: u64) -> String {
        self.last = now_ms;
        let at = cut(&self.held, MAX_JOB_LOG_BYTES);
        self.held.drain(..at).collect()
    }

    pub fn rest(&mut self) -> Vec<String> {
        let mut parts = Vec::new();
        while !self.held.is_empty() {
            let at = cut(&self.held, MAX_JOB_LOG_BYTES);
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
fn checkout(data: &Path, queued: &Queued, bundle: &[u8]) -> std::result::Result<Option<PathBuf>, String> {
    let job = &queued.job;
    let repo = data.join(safe(&job.repo));
    let keep = format!("refs/kebbie/from/{}", queued.from);
    if bundle::store(&repo.join(".store"), bundle, &job.commit, job.base.as_deref(), &keep).map_err(|error| format!("the worker could not take the commit: {error}"))? == bundle::Stored::NeedsFull {
        return Ok(None);
    }
    let store = repo.join(".store");
    let work = repo.join(safe(&job.task));
    let reused = work.join(".git").exists() && crate::repo::git(&work, &["checkout", "--quiet", "--detach", "--force", &job.commit]).and_then(|_| crate::repo::git(&work, &["clean", "-fdq"])).is_ok();
    if !reused {
        let _ = std::fs::remove_dir_all(&work);
        let _ = crate::repo::git(&store, &["worktree", "prune"]);
        crate::repo::git(&store, &["worktree", "add", "--quiet", "--detach", &work.to_string_lossy(), &job.commit]).map_err(|error| format!("the worker could not check out the commit: {error}"))?;
    }
    Ok(Some(work))
}

fn kill_group(pid: u32) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: signals the process group this worker started for the job.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
    }
}

pub fn execute(queued: &Queued, data: &Path, fetch: &dyn Fn(&[BlobRef]) -> Result<Vec<u8>>, send: &mut dyn FnMut(String, Option<JobEnd>), stop: &dyn Fn() -> bool, clock: &dyn Fn() -> u64) -> JobEnd {
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
    let work = match checkout(data, queued, &bundle) {
        Ok(Some(work)) => work,
        Ok(None) => return end(send, JobEnd::NeedsFull {}),
        Err(error) => return end(send, failed(&error)),
    };
    let child = Command::new("sh").args(["-c", &format!("exec 2>&1\n{{ {}\n}}", job.command)]).current_dir(&work)
        .envs([("KEBBIE_JOB", job.name.as_str()), ("KEBBIE_JOB_TASK", job.task.as_str()), ("KEBBIE_JOB_SHA", job.commit.as_str()), ("KEBBIE_JOB_MASTER", queued.from_name.as_str())])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0).spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => return end(send, failed(&format!("the job's command did not start: {error}"))),
    };
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
    let mut batch = Batch::new(clock());
    let mut stopped = false;
    loop {
        match read.recv_timeout(Duration::from_millis(200)) {
            Ok(text) => batch.add(&text),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !stopped && stop() {
            kill_group(child.id());
            stopped = true;
        }
        if batch.due(clock()) {
            let text = batch.take(clock());
            send(text, None);
        }
    }
    let status = child.wait();
    let ended = match status {
        _ if stopped => failed(&format!("the worker's kebbie stopped while {} ran", job.name)),
        Ok(status) => JobEnd::Exited { code: status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0)), artifacts: Vec::new() },
        Err(error) => failed(&format!("the job's command was lost: {error}")),
    };
    let mut rest = batch.rest();
    let last = rest.pop().unwrap_or_default();
    for text in rest {
        send(text, None);
    }
    send(last, Some(ended.clone()));
    ended
}

fn deliver(client: &Client, identity: &Identity, account: &AccountKey, to: &Id, keys: &PublicKeys, report: &JobReport, halted: &dyn Fn() -> bool) -> Result<()> {
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

pub fn run_one(dir: &Path, identity: &Identity, account: &AccountKey, client: &Client, data: &Path, queued: &Queued, halted: &dyn Fn() -> bool) -> String {
    let said = format!("jobs: {} for {} ({}) from {}", queued.job.name, queued.job.repo, queued.job.task, queued.from_name);
    let keys = match cluster::sender(dir, client, account, &queued.from, false) {
        Ok((keys, _)) => keys,
        Err(error) => return format!("{said} did not run, as no report could go back: {error}"),
    };
    let mut seq = 0u32;
    let mut lost: Option<String> = None;
    let mut send = |log: String, end: Option<JobEnd>| {
        let report = JobReport { job: queued.id.clone(), seq, log, end };
        seq += 1;
        if let Err(error) = deliver(client, identity, account, &queued.from, &keys, &report, halted) {
            lost.get_or_insert(error);
        }
    };
    let end = execute(queued, data, &|parts| bundle::download(client, &mut bundle::Calls::new(&crate::typing::now_ms, &std::thread::sleep, halted), parts), &mut send, halted, &crate::typing::now_ms);
    format!("{said}: {}{}", ended_as(&end), lost.map(|error| format!("; a report did not reach the master: {error}")).unwrap_or_default())
}

pub fn refuse(dir: &Path, identity: &Identity, account: &AccountKey, client: &Client, queued: &Queued, why: &str) -> String {
    let said = format!("jobs: refused {} for {} from {}: {why}", queued.job.name, queued.job.repo, queued.from_name);
    let report = JobReport { job: queued.id.clone(), seq: 0, log: String::new(), end: Some(JobEnd::Refused { reason: reason(why) }) };
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
                let note = if enabled { run_one(&dir, &identity, &account, &client, &super::data_dir(), &queued, &halted) } else { refuse(&dir, &identity, &account, &client, &queued, &format!("{} runs no jobs: worker.enabled is off there", identity.name)) };
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
