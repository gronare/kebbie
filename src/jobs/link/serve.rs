use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use kebbie_hub_protocol::{AccountKey, Id, is_commit};
use rustls::{ServerConnection, StreamOwned};

use super::super::{bundle, cluster, fetchable, safe, worker};
use super::tls;
use super::wire::{Ask, Frame, Link, RunAsk, Said, Timed, beating, keep_alive};
use crate::hub::http::{CALL, Client};
use crate::hub::identity::Identity;
use crate::sh::Result;

const HANDSHAKE: Duration = Duration::from_secs(10);
const PENDING: usize = 8;
pub const LISTED: Duration = Duration::from_secs(3);
const GRACE: Duration = Duration::from_secs(10);
const DRAIN: Duration = Duration::from_secs(2);
// How long a stopping keeper waits for its sessions: each command's grace between SIGTERM and SIGKILL, its drain, and a margin.
pub const SETTLE: Duration = GRACE.saturating_add(DRAIN).saturating_add(Duration::from_secs(3));
const QUEUED: usize = 4;
const MAX_BUNDLE: usize = 2 * 1024 * 1024 * 1024;
const HAVES: &str = "64";
// A job's worktree is <repo>/<task> and the hub's names start with a letter or digit, so a link's under a dot never meets one.
const LINKS: &str = ".links";
const SIGNALS: [i32; 5] = [libc::SIGINT, libc::SIGTERM, libc::SIGKILL, libc::SIGHUP, libc::SIGQUIT];

// What a session on this machine needs: whose machine it is, where it logs and where it keeps the repos it runs.
#[derive(Clone)]
pub struct Host {
    pub state: PathBuf,
    pub dir: PathBuf,
    pub identity: Identity,
    pub account: AccountKey,
    pub data: PathBuf,
}

impl Host {
    fn log(&self, text: &str) {
        crate::keeper::log_line(&self.state, text);
    }
}

type Listed = (Instant, std::result::Result<cluster::Cluster, String>);

static LISTS: Mutex<BTreeMap<PathBuf, Listed>> = Mutex::new(BTreeMap::new());
static LIVE: AtomicUsize = AtomicUsize::new(0);

// The account's list as the hub hands it over, which the account signed and which is no older than any list this machine saw, read again once LISTED has passed.
fn listed(host: &Host) -> Result<cluster::Cluster> {
    let mut lists = LISTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((at, list)) = lists.get(&host.dir) && at.elapsed() < LISTED {
        return list.clone();
    }
    let client = Client::new(&host.identity.url, Some(host.identity.token()), CALL);
    let list = cluster::fetch(&host.dir, &client, &host.account).map_err(|error| format!("the account's list of machines was not read: {error}"));
    lists.insert(host.dir.clone(), (Instant::now(), list.clone()));
    list
}

#[cfg(test)]
pub fn list_read(dir: &Path) -> bool {
    LISTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains_key(dir)
}

fn machine_of(list: &cluster::Cluster, key: &[u8; 32]) -> Option<Id> {
    list.keys.iter().find(|(_, keys)| &keys.signing == key).map(|(machine, _)| machine.clone())
}

// Only a key on the account's list opens a session, and the list is asked for only once a peer presents a key.
fn on_the_list(host: &Host) -> tls::Lookup {
    let host = host.clone();
    Arc::new(move |key| listed(&host).ok().and_then(|list| machine_of(&list, key)))
}

// Reads through a deadline for the whole handshake, so a peer that trickles a byte at a time cannot hold it open past HANDSHAKE.
pub struct Deadline<'a, T> {
    pub inner: &'a mut T,
    pub until: Instant,
}

impl<T: Read + Timed> Read for Deadline<'_, T> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the handshake took too long"));
        }
        self.inner.wait_at_most(Some(left))?;
        self.inner.read(buffer)
    }
}

impl<T: Write> Write for Deadline<'_, T> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.inner.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub fn handshake<T: Read + Write + Timed + Send + 'static>(host: &Host, mut transport: T, allowed: tls::Lookup) -> Result<(Link, Id)> {
    let mut connection = ServerConnection::new(tls::server(&host.identity.secret(), allowed.clone())?).map_err(|error| error.to_string())?;
    let until = Instant::now() + HANDSHAKE;
    while connection.is_handshaking() {
        connection.complete_io(&mut Deadline { inner: &mut transport, until }).map_err(|error| format!("the TLS handshake failed: {error}"))?;
    }
    let peer = connection.peer_certificates().and_then(|certs| certs.first()).and_then(|cert| tls::key_of(cert)).and_then(|key| allowed(&key)).ok_or("the peer showed no key on the account's list")?;
    Ok((Link::new(Box::new(StreamOwned::new(connection, transport))), peer))
}

// A connection not yet through its handshake, of at most PENDING per listener, so strangers cannot pile up threads.
pub struct Pending(Arc<AtomicUsize>);

impl Pending {
    fn take(count: &Arc<AtomicUsize>) -> Option<Pending> {
        let taken = Pending(count.clone());
        (count.fetch_add(1, Ordering::SeqCst) < PENDING).then_some(taken)
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Live;

impl Live {
    fn enter() -> Live {
        LIVE.fetch_add(1, Ordering::SeqCst);
        Live
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

// How many sessions run now in this process, which the keeper waits on as it stops.
pub fn live() -> usize {
    LIVE.load(Ordering::SeqCst)
}

pub fn session<T: Read + Write + Timed + Send + 'static>(host: &Host, from: String, transport: T, halted: &(dyn Fn() -> bool + Sync)) {
    serve(host, from, transport, halted, None);
}

fn serve<T: Read + Write + Timed + Send + 'static>(host: &Host, from: String, transport: T, halted: &(dyn Fn() -> bool + Sync), pending: Option<Pending>) {
    let _live = Live::enter();
    let opened = handshake(host, transport, on_the_list(host));
    drop(pending);
    let (mut link, name) = match opened.and_then(|(link, peer)| listed(host).map(|list| (link, list.name_of(&peer)))) {
        Ok(opened) => opened,
        Err(error) => return host.log(&format!("link: refused a session from {from}: {error}")),
    };
    let said = match link.expect_unless(&|| halted()) {
        Ok(Frame::Ask(Ask::Ping)) => link.send(&Frame::Said(Said::Pong)).map(|()| "ping".to_string()).map_err(|error| error.to_string()),
        Ok(Frame::Ask(Ask::Run(ask))) => {
            let what = format!("run {:?} in {} ({}) at {}", ask.command, ask.repo, ask.task, &ask.commit[..12.min(ask.commit.len())]);
            let ended = run(host, &mut link, &ask, &name, halted);
            if let Err(reason) = &ended {
                let _ = link.send(&Frame::Said(Said::Failed { reason: reason.clone() }));
            }
            Ok(format!("{what}: {}", ended.unwrap_or_else(|reason| format!("failed: {reason}"))))
        }
        Ok(other) => Err(format!("it began with {other:?}, not a question")),
        Err(error) => Err(error.to_string()),
    };
    host.log(&format!("link: session from {name} ({from}): {}", said.unwrap_or_else(|error| format!("dropped: {error}"))));
}

fn say(link: &mut Link, text: &str) {
    let _ = link.send(&Frame::Err(format!("[kebbie: {text}]\n").into_bytes()));
}

fn haves(store: &Path) -> Vec<String> {
    crate::repo::git(store, &["for-each-ref", "--sort=-committerdate", "--count", HAVES, "--format=%(objectname)", "refs/kebbie/"]).unwrap_or_default().lines().map(str::to_string).collect()
}

fn receive_bundle(link: &mut Link, halted: &(dyn Fn() -> bool + Sync)) -> Result<Vec<u8>> {
    let mut bundle = Vec::new();
    loop {
        match link.expect_unless(&|| halted()).map_err(|error| error.to_string())? {
            Frame::Data(bytes) if bundle.len() + bytes.len() <= MAX_BUNDLE => bundle.extend(bytes),
            Frame::Data(_) => return Err("the commit is more than 2 GiB as a git bundle".to_string()),
            Frame::End => return Ok(bundle),
            other => return Err(format!("expected the commit, got {other:?}")),
        }
    }
}

// The asked commit in the repo's store under refs/kebbie/tasks/<task>: nothing comes when the store has it, else the history from origin first and then only what the store still lacks.
fn sync(link: &mut Link, ask: &RunAsk, store: &Path, halted: &(dyn Fn() -> bool + Sync)) -> Result<()> {
    bundle::open_store(store)?;
    let keep = format!("refs/kebbie/tasks/{}", safe(&ask.task));
    if bundle::has(store, &ask.commit) {
        return crate::repo::git(store, &["update-ref", &keep, &ask.commit]).map(|_| ());
    }
    if let Some(remote) = &ask.remote
        && let Some(url) = fetchable(&remote.url)
        && is_commit(&remote.base)
        && !bundle::has(store, &remote.base)
    {
        say(link, &format!("fetching the history from {url}; only what is new comes on later runs"));
        if let Err(error) = beating(link, || bundle::fetch_remote(store, &url, halted)) {
            say(link, &format!("the fetch from {url} failed, so the whole history comes over the link: {error}"));
        }
    }
    link.send(&Frame::Said(Said::Want { haves: haves(store) })).map_err(|error| error.to_string())?;
    let bytes = receive_bundle(link, halted)?;
    let (stored, lines) = beating(link, || {
        let mut lines = Vec::new();
        let stored = bundle::store(store, &bytes, &ask.commit, None, &keep, None, &mut |text| lines.push(text), halted);
        (stored, lines)
    });
    for line in lines {
        say(link, line.trim().trim_start_matches("[kebbie: ").trim_end_matches(']'));
    }
    match stored? {
        bundle::Stored::Ready => Ok(()),
        bundle::Stored::NeedsFull => Err("the commit did not arrive whole".to_string()),
    }
}

fn run(host: &Host, link: &mut Link, ask: &RunAsk, from: &str, halted: &(dyn Fn() -> bool + Sync)) -> std::result::Result<String, String> {
    if !is_commit(&ask.commit) || ask.command.trim().is_empty() {
        return Err("the run named no full commit or no command".to_string());
    }
    let repo = host.data.join(safe(&ask.repo));
    let work = repo.join(LINKS).join(safe(&ask.task));
    let _held = match Held::try_take(&work) {
        Some(held) => held,
        None => {
            say(link, "waiting for the run before this one in the same task on this machine");
            beating(link, || Held::take(&work, halted)).ok_or("the keeper stopped before the run before this one ended")?
        }
    };
    sync(link, ask, &repo.join(".store"), halted)?;
    beating(link, || worker::check_out(&repo.join(".store"), &work, &ask.commit, halted))?;
    link.send(&Frame::Said(Said::Started)).map_err(|error| error.to_string())?;
    let code = exec(link, &work, ask, from, halted)?;
    Ok(format!("exited {code}"))
}

static HELD: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

// One run at a time in a task's worktree, from the sync to the command's end, so no run cleans or replaces the tree another runs in.
struct Held(PathBuf);

impl Held {
    fn try_take(work: &Path) -> Option<Held> {
        HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(work.to_path_buf()).then(|| Held(work.to_path_buf()))
    }

    fn take(work: &Path, halted: &(dyn Fn() -> bool + Sync)) -> Option<Held> {
        loop {
            if let Some(held) = Held::try_take(work) {
                return Some(held);
            }
            if halted() {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.0);
    }
}

fn pipe_to(sender: mpsc::SyncSender<(bool, Option<Vec<u8>>)>, mut from: impl Read + Send + 'static, err: bool) {
    std::thread::spawn(move || {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match from.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if sender.send((err, Some(buffer[..read].to_vec()))).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = sender.send((err, None));
    });
}

// Streams the command's output as it comes and forwards the asker's signals to its process group; the asker gone or the keeper stopping ends it with SIGTERM, then SIGKILL after the grace.
fn exec(link: &mut Link, work: &Path, ask: &RunAsk, from: &str, halted: &(dyn Fn() -> bool + Sync)) -> std::result::Result<i32, String> {
    let mut child = Command::new("sh").args(["-c", &ask.command]).current_dir(work)
        .envs([("KEBBIE_RUN_TASK", ask.task.as_str()), ("KEBBIE_RUN_FROM", from)])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn().map_err(|error| format!("the command did not start: {error}"))?;
    let (sender, output) = mpsc::sync_channel(QUEUED);
    if let Some(out) = child.stdout.take() {
        pipe_to(sender.clone(), out, false);
    }
    if let Some(err) = child.stderr.take() {
        pipe_to(sender, err, true);
    }
    let (mut open, mut gone, mut termed, mut killed, mut exited) = (2, false, None::<Instant>, false, None::<std::process::ExitStatus>);
    let mut caught_up = None::<Instant>;
    loop {
        match output.recv_timeout(Duration::from_millis(5)) {
            Ok((err, Some(bytes))) => gone |= link.send(&if err { Frame::Err(bytes) } else { Frame::Out(bytes) }).is_err(),
            Ok((_, None)) => open -= 1,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if exited.is_some() && caught_up.is_none() {
                    caught_up = Some(Instant::now());
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => open = 0,
        }
        if !gone {
            match link.recv(Some(Duration::from_millis(1))) {
                Ok(Some(Frame::Signal(signal))) if SIGNALS.contains(&signal) => worker::signal_group(child.id(), signal),
                Ok(_) => {}
                Err(_) => gone = true,
            }
            gone |= link.gone_quiet() || link.beat_if_due().is_err();
        }
        if (gone || halted()) && termed.is_none() {
            worker::signal_group(child.id(), libc::SIGTERM);
            termed = Some(Instant::now());
        }
        if let Some(at) = termed && !killed && at.elapsed() >= GRACE {
            worker::signal_group(child.id(), libc::SIGKILL);
            killed = true;
        }
        if exited.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => exited = Some(status),
                Ok(None) => {}
                Err(error) => return Err(format!("the command was lost: {error}")),
            }
        }
        if let Some(status) = exited && (open == 0 || gone || caught_up.is_some_and(|at| at.elapsed() >= DRAIN)) {
            worker::signal_group(child.id(), libc::SIGKILL);
            let code = status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
            if gone {
                return Err(format!("the asker went away; the command ended with {code}"));
            }
            link.send(&Frame::Said(Said::Exited { code })).map_err(|error| error.to_string())?;
            return Ok(code);
        }
    }
}

// One thread per address, each taking sessions until halted; a session runs on a thread of its own.
pub fn listen(host: Arc<Host>, listener: TcpListener, halted: Arc<dyn Fn() -> bool + Send + Sync>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _ = listener.set_nonblocking(true);
        let (waiting, mut crowded) = (Arc::new(AtomicUsize::new(0)), None::<Instant>);
        while !halted() {
            match listener.accept() {
                Ok((tcp, from)) => {
                    let Some(pending) = Pending::take(&waiting) else {
                        if crowded.is_none_or(|at| at.elapsed() >= Duration::from_secs(60)) {
                            host.log(&format!("link: turned away {from} and any others for a minute: {PENDING} connections are already in their handshake"));
                            crowded = Some(Instant::now());
                        }
                        continue;
                    };
                    let _ = tcp.set_nonblocking(false);
                    let _ = tcp.set_nodelay(true);
                    keep_alive(&tcp);
                    let (host, halted) = (host.clone(), halted.clone());
                    std::thread::spawn(move || serve(&host, from.to_string(), tcp, &*halted, Some(pending)));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    })
}

