use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use kebbie_hub_api::{AccountMachine, NOT_A_WORKER};
use kebbie_hub_protocol::{AccountKey, Id, Job, JobEnd, JobReport, Payload};
use serde::{Deserialize, Serialize};

use super::{Def, REF, bundle, cluster, token};
use crate::hub::http::{CALL, Client, HubError, pro_line};
use crate::hub::identity::{self, Identity};
use crate::sh::Result;

const DIR: &str = "jobs";
const BASES: &str = "jobs/bases.json";
const POLL: Duration = Duration::from_millis(300);
pub const SILENT: u64 = 600;

// A job this machine sent, kept while kebbie job run follows it: the worker it went to is the only machine whose reports on it are taken.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sent {
    pub id: Id,
    pub worker: Id,
    pub worker_name: String,
    pub repo: String,
    pub name: String,
    pub commit: String,
    pub base: Option<String>,
    pub blobs: Vec<Id>,
    pub at: u64,
    #[serde(default)]
    pub pid: u32,
}

// The reports taken so far: the next seq expected, so one the hub hands over again is not printed twice, and when the last one came.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Heard {
    pub next: u32,
    pub at: u64,
}

pub fn part(state: &Path, id: &Id, kind: &str) -> PathBuf {
    state.join(DIR).join(format!("{id}.{kind}"))
}

fn read<T: for<'de> Deserialize<'de>>(file: &Path) -> Option<T> {
    std::fs::read_to_string(file).ok().and_then(|text| serde_json::from_str(&text).ok())
}

fn write<T: Serialize>(file: &Path, value: &T) -> Result<()> {
    crate::state::write(file, serde_json::to_string(value).map_err(|error| error.to_string())?)
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    libc::pid_t::try_from(pid).is_ok_and(|pid| pid > 0 && unsafe { libc::kill(pid, 0) } == 0)
}

fn all_sent(state: &Path) -> Vec<Sent> {
    std::fs::read_dir(state.join(DIR)).into_iter().flatten().flatten().filter(|entry| entry.file_name().to_string_lossy().ends_with(".sent.json")).filter_map(|entry| read(&entry.path())).collect()
}

pub fn waiting(state: &Path) -> bool {
    all_sent(state).iter().any(|sent| alive(sent.pid))
}

fn gap(lost: u32, worker: &str) -> String {
    match lost {
        0 => String::new(),
        1 => format!("[kebbie: a report from {worker} was lost here, and the output it held with it]\n"),
        lost => format!("[kebbie: {lost} reports from {worker} were lost here, and the output they held with them]\n"),
    }
}

pub fn report(state: &Path, from: &Id, report: &JobReport, now: u64) -> Option<String> {
    let Some(sent) = read::<Sent>(&part(state, &report.job, "sent.json")) else {
        return Some(format!("jobs: dropped a report from {from} on {}: no job this machine runs has that id", report.job));
    };
    if &sent.worker != from {
        return Some(format!("jobs: refused a report on {} from {from}: the job went to {} ({})", sent.name, sent.worker_name, sent.worker));
    }
    let _held = crate::state::Locked::wait(&part(state, &sent.id, "lock"));
    let heard_file = part(state, &sent.id, "heard.json");
    let mut heard: Heard = read(&heard_file).unwrap_or_default();
    if report.seq < heard.next {
        return None;
    }
    let text = format!("{}{}", gap(report.seq - heard.next, &sent.worker_name), report.log);
    if !text.is_empty() {
        let appended = crate::state::append_to(&part(state, &sent.id, "log")).and_then(|mut log| log.write_all(text.as_bytes()));
        if let Err(error) = appended {
            return Some(format!("jobs: the output of {} was not kept: {error}", sent.name));
        }
    }
    heard = Heard { next: report.seq + 1, at: now };
    let mut kept = write(&heard_file, &heard);
    if let Some(end) = &report.end {
        kept = kept.and(write(&part(state, &sent.id, "end.json"), end));
    }
    match (kept, &report.end) {
        (Err(error), _) => Some(format!("jobs: a report on {} was not kept: {error}", sent.name)),
        (Ok(()), Some(end)) => Some(format!("jobs: {} on {} ended: {}", sent.name, sent.worker_name, serde_json::to_string(end).unwrap_or_default())),
        (Ok(()), None) => None,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bases {
    #[serde(default)]
    pub sent: BTreeMap<String, String>,
}

impl Bases {
    fn key(worker: &Id, repo: &str) -> String {
        format!("{worker} {repo}")
    }

    pub fn base(&self, worker: &Id, repo: &str) -> Option<&str> {
        self.sent.get(&Bases::key(worker, repo)).map(String::as_str)
    }

    pub fn record(&mut self, worker: &Id, repo: &str, commit: &str) {
        self.sent.insert(Bases::key(worker, repo), commit.to_string());
    }
}

pub fn bases(state: &Path) -> Bases {
    read(&state.join(BASES)).unwrap_or_default()
}

fn record_base(state: &Path, top: &Path, worker: &Id, repo: &str, commit: &str) {
    let _held = crate::state::Locked::wait(&state.join("jobs/bases.lock"));
    let mut kept = bases(state);
    kept.record(worker, repo, commit);
    let _ = write(&state.join(BASES), &kept);
    let _ = crate::repo::git(top, &["update-ref", &format!("refs/kebbie/bases/{worker}"), commit]);
}

pub enum Followed {
    Ended(JobEnd),
    Silent,
    Stopped,
}

// Prints the job's log as it grows and ends with its end; a worker that sends nothing at all for SILENT seconds is given up on, however long the job has run.
pub fn follow(state: &Path, sent: &Sent, print: &mut dyn FnMut(&str), pause: &mut dyn FnMut(), now: &dyn Fn() -> u64, stop: &dyn Fn() -> bool) -> Followed {
    let mut offset = 0u64;
    let drain = |offset: &mut u64, print: &mut dyn FnMut(&str)| {
        let Ok(mut file) = std::fs::File::open(part(state, &sent.id, "log")) else { return };
        let mut text = Vec::new();
        if file.seek(SeekFrom::Start(*offset)).is_ok() && file.read_to_end(&mut text).is_ok() && !text.is_empty() {
            *offset += text.len() as u64;
            print(&String::from_utf8_lossy(&text));
        }
    };
    loop {
        drain(&mut offset, print);
        if let Some(end) = read::<JobEnd>(&part(state, &sent.id, "end.json")) {
            drain(&mut offset, print);
            return Followed::Ended(end);
        }
        let heard = read::<Heard>(&part(state, &sent.id, "heard.json")).map_or(0, |heard| heard.at);
        if now().saturating_sub(heard.max(sent.at)) > SILENT {
            return Followed::Silent;
        }
        if stop() {
            return Followed::Stopped;
        }
        pause();
    }
}

fn delete_blobs(client: &Client, blobs: &[Id]) {
    let mut calls = bundle::Calls::new(&crate::typing::now_ms, &std::thread::sleep, &|| false);
    for blob in blobs {
        let path = format!("/v1/m/blobs/{blob}");
        let _ = calls.call(&mut || client.delete(&path));
    }
}

fn remove_files(state: &Path, id: &Id) {
    for kind in ["sent.json", "heard.json", "end.json", "log", "lock", "scratch.bundle"] {
        let _ = std::fs::remove_file(part(state, id, kind));
    }
}

// A job whose kebbie job run is gone without cleaning up, as one killed outright: its blobs and files go, so the keeper stops polling for it.
pub fn sweep(state: &Path, client: &Client) {
    for sent in all_sent(state).into_iter().filter(|sent| !alive(sent.pid)) {
        delete_blobs(client, &sent.blobs);
        remove_files(state, &sent.id);
    }
}

// What a job run leaves on the hub and here, removed on every way out of it, Ctrl-C included; removing it twice does nothing.
struct Leftovers<'a> {
    state: &'a Path,
    client: &'a Client,
    top: &'a Path,
    refs: Vec<String>,
    ids: Vec<Id>,
    blobs: Vec<Id>,
}

impl<'a> Leftovers<'a> {
    fn new(state: &'a Path, client: &'a Client, top: &'a Path) -> Leftovers<'a> {
        Leftovers { state, client, top, refs: Vec::new(), ids: Vec::new(), blobs: Vec::new() }
    }

    fn clean_job(&mut self) {
        delete_blobs(self.client, &std::mem::take(&mut self.blobs));
        for id in std::mem::take(&mut self.ids) {
            remove_files(self.state, &id);
        }
    }

    fn clean(&mut self) {
        self.clean_job();
        for name in std::mem::take(&mut self.refs) {
            let _ = crate::repo::git(self.top, &["update-ref", "-d", &name]);
        }
    }
}

impl Drop for Leftovers<'_> {
    fn drop(&mut self) {
        self.clean();
    }
}

fn nap(wait: Duration, stop: &dyn Fn() -> bool) {
    let until = std::time::Instant::now() + wait;
    while !stop() && std::time::Instant::now() < until {
        std::thread::sleep(POLL.min(until.saturating_duration_since(std::time::Instant::now())));
    }
}

pub fn no_worker(on: Option<&str>) -> String {
    let which = on.map_or_else(|| "No machine of your hub account runs jobs".to_string(), |on| format!("No machine of your hub account on {on} runs jobs"));
    format!("{which}. On the machine that should run it, set [worker] enabled = true in ~/.config/kebbie/config.toml; its keeper then tells the hub, and kebbie job run finds it")
}

pub struct Asked<'a> {
    pub top: &'a Path,
    pub repo: String,
    pub name: String,
    pub def: &'a Def,
}

pub struct Hub<'a> {
    pub state: &'a Path,
    pub dir: &'a Path,
    pub identity: &'a Identity,
    pub account: &'a AccountKey,
}

fn ended(asked: &Asked, worker: &str, followed: Followed) -> Result<String> {
    let name = &asked.name;
    match followed {
        Followed::Ended(JobEnd::Exited { code: 0, .. }) => Ok(format!("kebbie: {name} passed on {worker}")),
        Followed::Ended(JobEnd::Exited { code, .. }) => Err(format!("kebbie: {name} failed on {worker} with exit {code}")),
        Followed::Ended(JobEnd::Failed { reason }) => Err(format!("kebbie: {name} did not run on {worker}: {reason}")),
        Followed::Ended(JobEnd::Refused { reason }) => Err(format!("kebbie: {worker} refused {name}: {reason}")),
        Followed::Ended(JobEnd::NeedsFull {}) => Err(format!("kebbie: {worker} did not take {name}'s commit even with its whole history")),
        Followed::Silent => Err(format!("kebbie: {worker} sent nothing on {name} for {} minutes, so kebbie gave up on it. Is that machine on, its keeper running and [worker] enabled = true there?", SILENT / 60)),
        Followed::Stopped => Err(stopped(name, worker)),
    }
}

fn stopped(name: &str, worker: &str) -> String {
    format!("kebbie: stopped following {name}; its commit is gone from the hub and its files from this machine. If {worker} took it already, it runs to its end there, as kebbie has no way to tell a worker to stop a job")
}

#[allow(clippy::too_many_arguments)]
fn attempt(hub: &Hub, client: &Client, left: &mut Leftovers, asked: &Asked, worker: &AccountMachine, keys: &kebbie_hub_protocol::PublicKeys, snapshot: &str, commit: &str, task: &str, base: Option<&str>, print: &mut dyn FnMut(&str), pause: &mut dyn FnMut(), now: &dyn Fn() -> u64, stop: &dyn Fn() -> bool) -> Result<Followed> {
    let id = cluster::new_id()?;
    left.ids.push(id.clone());
    crate::state::private_dir(&hub.state.join(DIR))?;
    let bytes = bundle::create(asked.top, snapshot, commit, base, &part(hub.state, &id, "scratch"))?;
    let sleep = |wait| nap(wait, stop);
    let mut calls = bundle::Calls::new(&crate::typing::now_ms, &sleep, stop);
    let mut parts = Vec::new();
    let uploaded = bundle::upload(&bytes, &mut calls, &|path, body| client.put_bytes(path, body), &mut parts);
    left.blobs.extend(parts.iter().map(|part| part.id.clone()));
    uploaded.map_err(|error| format!("kebbie: the job's commit did not go up to the hub: {error}"))?;
    if stop() {
        return Ok(Followed::Stopped);
    }
    let sent = Sent { id: id.clone(), worker: worker.machine.clone(), worker_name: worker.name.clone(), repo: asked.repo.clone(), name: asked.name.clone(), commit: commit.to_string(), base: base.map(str::to_string), blobs: left.blobs.clone(), at: now(), pid: std::process::id() };
    write(&part(hub.state, &id, "sent.json"), &sent)?;
    let job = Job { repo: token(&asked.repo), name: token(&asked.name), command: super::command_for(asked.def), task: task.to_string(), commit: commit.to_string(), base: base.map(str::to_string), bundle: parts, on: asked.def.on.as_deref().map(token) };
    let envelope = cluster::seal_to(hub.identity, hub.account, &worker.machine, keys, id.clone(), &Payload::Job(job), now()).map_err(|error| format!("kebbie: the job did not seal: {error}"))?;
    let followed = match cluster::post(client, &envelope) {
        Ok(_) => Ok(follow(hub.state, &sent, print, pause, now, stop)),
        Err(HubError::ProRequired { url }) => Err(pro_line("Running a job on another machine", url.as_deref())),
        Err(HubError::Status { status: 409, code }) if code == NOT_A_WORKER => Err(format!("kebbie: {} stopped running jobs just now. {}", worker.name, no_worker(asked.def.on.as_deref()))),
        Err(error) => Err(format!("kebbie: the job was not sent: {error}")),
    };
    left.clean_job();
    followed
}

pub fn go(hub: &Hub, asked: &Asked, print: &mut dyn FnMut(&str), pause: &mut dyn FnMut(), now: &dyn Fn() -> u64, stop: &dyn Fn() -> bool) -> Result<String> {
    let client = Client::new(&hub.identity.url, Some(hub.identity.token()), CALL);
    sweep(hub.state, &client);
    let cluster = cluster::fetch(hub.dir, &client, hub.account).map_err(|error| format!("kebbie: the hub's list of your machines was not read: {error}"))?;
    let worker = cluster::pick(&cluster, &hub.identity.machine, asked.def.on.as_deref()).cloned().ok_or_else(|| no_worker(asked.def.on.as_deref()))?;
    let keys = *cluster.keys.get(&worker.machine).ok_or_else(|| no_worker(asked.def.on.as_deref()))?;
    let mut left = Leftovers::new(hub.state, &client, asked.top);
    let snapshot_id = cluster::new_id()?;
    let commit = super::snapshot(asked.top, snapshot_id.as_str())?;
    let snapshot = format!("{REF}/{snapshot_id}");
    left.refs.push(snapshot.clone());
    crate::repo::git(asked.top, &["update-ref", &snapshot, &commit])?;
    let branch = crate::repo::git(asked.top, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let task = token(if branch.is_empty() || branch == "HEAD" { "detached" } else { &branch });
    print(&format!("kebbie: {} runs {} for {} on {}\n", worker.name, asked.name, asked.repo, &commit[..12.min(commit.len())]));
    let mut full = false;
    let followed = loop {
        let base = if full { None } else { bases(hub.state).base(&worker.machine, &asked.repo).filter(|base| bundle::has(asked.top, base)).map(str::to_string) };
        match attempt(hub, &client, &mut left, asked, &worker, &keys, &snapshot, &commit, &task, base.as_deref(), print, pause, now, stop) {
            Ok(Followed::Ended(JobEnd::NeedsFull {})) if !full => {
                print(&format!("kebbie: {} lacks the commit this job builds on, so it gets the whole history\n", worker.name));
                full = true;
            }
            other => break other,
        }
    };
    left.clean();
    if stop() {
        return Err(stopped(&asked.name, &worker.name));
    }
    let followed = followed?;
    if matches!(followed, Followed::Ended(JobEnd::Exited { .. })) {
        record_base(hub.state, asked.top, &worker.machine, &asked.repo, &commit);
    }
    ended(asked, &worker.name, followed)
}

fn listing(repo: &str, file: &Path, names: &[&String]) -> String {
    let listed = if names.is_empty() { "none yet".to_string() } else { names.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(", ") };
    format!("{repo}'s jobs, from [jobs] in {}: {listed}", file.display())
}

pub fn run(cwd: &Path, name: Option<&str>) -> Result<String> {
    let state = crate::lead::state_dir()?;
    let top = PathBuf::from(crate::repo::git(cwd, &["rev-parse", "--show-toplevel"])?);
    let repo = super::repo_name(&top);
    let file = crate::config::repo_file(&top);
    let jobs = super::defined(&file)?;
    let names: Vec<&String> = jobs.keys().collect();
    let Some(name) = name else { return Ok(listing(&repo, &file, &names)) };
    let def = jobs.get(name).ok_or_else(|| format!("{repo} has no job {name}. {}. Add one there as {name} = \"<command>\", or as [jobs.{name}] with run = \"<command>\" and on = \"macos\"", listing(&repo, &file, &names)))?;
    if crate::keeper::holder(&state).is_none() {
        return Err("kebbie job run needs this machine's keeper running, as the keeper is what receives the job's output from the worker: start it with kebbie keeper --start".to_string());
    }
    let dir = identity::dir(&state);
    let identity = identity::load_at(&dir)?.ok_or("This machine is not connected to a hub, and a job runs on another machine of your hub account: connect it with kebbie hub connect")?;
    let account = identity.account.as_ref().map(|account| account.key.clone()).ok_or("This machine has no hub account pinned, so it cannot tell your other machines from anyone's: kebbie hub leave, then kebbie hub connect")?;
    let hub = Hub { state: &state, dir: &dir, identity: &identity, account: &account };
    let asked = Asked { top: &top, repo, name: name.to_string(), def };
    let mut out = std::io::stdout();
    catch_interrupt();
    go(&hub, &asked, &mut |text| {
        let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
    }, &mut || nap(POLL, &interrupted), &crate::messages::now, &interrupted)
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

// A second Ctrl-C ends kebbie at once, as the first one's clean-up may wait on the hub.
extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
    // SAFETY: signal() is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
    }
}

fn catch_interrupt() {
    // SAFETY: the handler only stores to an atomic and calls signal(), both async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}
