use std::collections::BTreeMap;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kebbie_hub_protocol::{AccountKey, Id, is_commit};
use rustls::{ServerConnection, StreamOwned};

use super::super::{bundle, cluster, fetchable, safe, worker};
use super::tls;
use super::wire::{Ask, Frame, Link, RunAsk, Said};
use crate::hub::http::{CALL, Client};
use crate::hub::identity::Identity;
use crate::sh::Result;

const HANDSHAKE: Duration = Duration::from_secs(10);
const IDLE: Duration = Duration::from_secs(60);
const GRACE: Duration = Duration::from_secs(10);
const DRAIN: Duration = Duration::from_secs(2);
const MAX_BUNDLE: usize = 2 * 1024 * 1024 * 1024;
const HAVES: &str = "64";
const SIGNALS: [i32; 5] = [libc::SIGINT, libc::SIGTERM, libc::SIGKILL, libc::SIGHUP, libc::SIGQUIT];

// What a session on this machine needs: whose machine it is, where it logs and where it keeps the repos it runs.
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

// Only a key on the list the hub hands over now, which the account signed and which is no older than any list this machine saw, opens a session.
fn allowed(host: &Host) -> Result<(BTreeMap<[u8; 32], Id>, cluster::Cluster)> {
    let client = Client::new(&host.identity.url, Some(host.identity.token()), CALL);
    let cluster = cluster::fetch(&host.dir, &client, &host.account).map_err(|error| format!("the account's list of machines was not read: {error}"))?;
    Ok((cluster.keys.iter().map(|(machine, keys)| (keys.signing, machine.clone())).collect(), cluster))
}

pub fn handshake(host: &Host, mut tcp: TcpStream, allowed: BTreeMap<[u8; 32], Id>) -> Result<(Link, Id)> {
    tcp.set_read_timeout(Some(HANDSHAKE)).map_err(|error| error.to_string())?;
    tcp.set_nodelay(true).map_err(|error| error.to_string())?;
    let mut connection = ServerConnection::new(tls::server(&host.identity.secret(), allowed.clone())?).map_err(|error| error.to_string())?;
    while connection.is_handshaking() {
        connection.complete_io(&mut tcp).map_err(|error| format!("the TLS handshake failed: {error}"))?;
    }
    let peer = connection.peer_certificates().and_then(|certs| certs.first()).and_then(|cert| tls::key_of(cert)).and_then(|key| allowed.get(&key).cloned()).ok_or("the peer showed no key on the account's list")?;
    Ok((Link::new(Box::new(StreamOwned::new(connection, tcp))), peer))
}

pub fn session(host: &Host, tcp: TcpStream, halted: &dyn Fn() -> bool) {
    let from = tcp.peer_addr().map_or_else(|_| "?".to_string(), |address| address.to_string());
    let opened = allowed(host).and_then(|(allowed, cluster)| handshake(host, tcp, allowed).map(|(link, peer)| (link, cluster.name_of(&peer))));
    let (mut link, name) = match opened {
        Ok(opened) => opened,
        Err(error) => return host.log(&format!("link: refused a session from {from}: {error}")),
    };
    let said = match link.expect(IDLE) {
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

fn receive_bundle(link: &mut Link) -> Result<Vec<u8>> {
    let mut bundle = Vec::new();
    loop {
        match link.expect(IDLE).map_err(|error| error.to_string())? {
            Frame::Data(bytes) if bundle.len() + bytes.len() <= MAX_BUNDLE => bundle.extend(bytes),
            Frame::Data(_) => return Err("the commit is more than 2 GiB as a git bundle".to_string()),
            Frame::End => return Ok(bundle),
            other => return Err(format!("expected the commit, got {other:?}")),
        }
    }
}

// The asked commit in the repo's store under refs/kebbie/tasks/<task>: nothing comes when the store has it, else the history from origin first and then only what the store still lacks.
fn sync(link: &mut Link, ask: &RunAsk, store: &Path, halted: &dyn Fn() -> bool) -> Result<()> {
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
        if let Err(error) = bundle::fetch_remote(store, &url, halted) {
            say(link, &format!("the fetch from {url} failed, so the whole history comes over the link: {error}"));
        }
    }
    link.send(&Frame::Said(Said::Want { haves: haves(store) })).map_err(|error| error.to_string())?;
    let bytes = receive_bundle(link)?;
    let mut said = |text: String| say(link, text.trim().trim_start_matches("[kebbie: ").trim_end_matches(']'));
    match bundle::store(store, &bytes, &ask.commit, None, &keep, None, &mut said, halted)? {
        bundle::Stored::Ready => Ok(()),
        bundle::Stored::NeedsFull => Err("the commit did not arrive whole".to_string()),
    }
}

fn run(host: &Host, link: &mut Link, ask: &RunAsk, from: &str, halted: &dyn Fn() -> bool) -> std::result::Result<String, String> {
    if !is_commit(&ask.commit) || ask.command.trim().is_empty() {
        return Err("the run named no full commit or no command".to_string());
    }
    let repo = host.data.join(safe(&ask.repo));
    sync(link, ask, &repo.join(".store"), halted)?;
    let work = repo.join("tasks").join(safe(&ask.task));
    worker::check_out(&repo.join(".store"), &work, &ask.commit, halted)?;
    link.send(&Frame::Said(Said::Started)).map_err(|error| error.to_string())?;
    let code = exec(link, &work, ask, from, halted)?;
    Ok(format!("exited {code}"))
}

fn pipe_to(sender: mpsc::Sender<(bool, Option<Vec<u8>>)>, mut from: impl Read + Send + 'static, err: bool) {
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
fn exec(link: &mut Link, work: &Path, ask: &RunAsk, from: &str, halted: &dyn Fn() -> bool) -> std::result::Result<i32, String> {
    let mut child = Command::new("sh").args(["-c", &ask.command]).current_dir(work)
        .envs([("KEBBIE_RUN_TASK", ask.task.as_str()), ("KEBBIE_RUN_FROM", from)])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn().map_err(|error| format!("the command did not start: {error}"))?;
    let (sender, output) = mpsc::channel();
    if let Some(out) = child.stdout.take() {
        pipe_to(sender.clone(), out, false);
    }
    if let Some(err) = child.stderr.take() {
        pipe_to(sender, err, true);
    }
    let (mut open, mut gone, mut termed, mut killed, mut exited) = (2, false, None::<Instant>, false, None::<(std::process::ExitStatus, Instant)>);
    loop {
        match output.recv_timeout(Duration::from_millis(5)) {
            Ok((err, Some(bytes))) => gone |= link.send(&if err { Frame::Err(bytes) } else { Frame::Out(bytes) }).is_err(),
            Ok((_, None)) => open -= 1,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => open = 0,
        }
        if !gone {
            match link.recv(Some(Duration::from_millis(1))) {
                Ok(Some(Frame::Signal(signal))) if SIGNALS.contains(&signal) => worker::signal_group(child.id(), signal),
                Ok(_) => {}
                Err(_) => gone = true,
            }
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
                Ok(Some(status)) => exited = Some((status, Instant::now())),
                Ok(None) => {}
                Err(error) => return Err(format!("the command was lost: {error}")),
            }
        }
        if let Some((status, at)) = exited && (open == 0 || at.elapsed() >= DRAIN) {
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
        while !halted() {
            match listener.accept() {
                Ok((tcp, _)) => {
                    let _ = tcp.set_nonblocking(false);
                    let (host, halted) = (host.clone(), halted.clone());
                    std::thread::spawn(move || session(&host, tcp, &*halted));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    })
}

