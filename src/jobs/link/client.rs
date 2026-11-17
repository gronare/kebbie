use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kebbie_hub_protocol::{AccountKey, Addresses, Id, Network, PublicKeys, is_commit};
use rustls::{ClientConnection, StreamOwned};

use super::super::cluster::{self, Cluster};
use super::super::{REF, bundle, token};
use super::wire::{Ask, CHUNK, Frame, Link, RemoteAsk, RunAsk, Said};
use super::{PORT, addresses, tls};
use crate::hub::http::{CALL, Client};
use crate::hub::identity::{self, Identity};
use crate::sh::Result;

const DIAL: Duration = Duration::from_secs(2);
const HANDSHAKE: Duration = Duration::from_secs(5);
const SYNC_WAIT: Duration = Duration::from_secs(1_800);
const ESCALATION: [i32; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGKILL];

// The machine a link goes to: its key on the account's list and the addresses it announced, or loopback when it is this machine.
#[derive(Clone, Debug)]
pub struct Target {
    pub machine: Id,
    pub name: String,
    pub keys: PublicKeys,
    pub addresses: Vec<SocketAddr>,
}

// The announced addresses to dial, tailnet first; why there are none when there are none.
pub fn dialable(name: &str, announced: Option<&Addresses>) -> Result<Vec<SocketAddr>> {
    let announced = announced.ok_or_else(|| format!("{name} announced no address for a link: is [worker] enabled = true there, and its kebbie new enough to take links?"))?;
    if announced.port == 0 {
        return Err(format!("{name} takes no links: [worker] enabled is off there"));
    }
    if announced.addresses.is_empty() {
        return Err(format!("{name} is on no private network kebbie knows (Tailscale or WireGuard), so it has no address to take a link on"));
    }
    let mut listed: Vec<_> = announced.addresses.iter().filter_map(|address| address.ip.parse().ok().map(|ip| (address.network != Network::Tailscale, SocketAddr::new(ip, announced.port)))).collect();
    listed.sort_by_key(|(later, _)| *later);
    Ok(listed.into_iter().map(|(_, address)| address).collect())
}

fn dial(addresses: &[SocketAddr], wait: Duration) -> Result<(TcpStream, SocketAddr)> {
    let (sender, dialed) = mpsc::channel();
    for address in addresses {
        let (sender, address) = (sender.clone(), *address);
        std::thread::spawn(move || {
            let _ = sender.send((address, TcpStream::connect_timeout(&address, wait)));
        });
    }
    drop(sender);
    let mut failed = Vec::new();
    for (address, dialed) in dialed {
        match dialed {
            Ok(tcp) => return Ok((tcp, address)),
            Err(error) => failed.push(format!("{address} ({error})")),
        }
    }
    Err(format!("none of its addresses answered within {} s: {}", wait.as_secs(), failed.join(", ")))
}

pub fn connect(identity: &Identity, target: &Target, wait: Duration) -> Result<(Link, SocketAddr)> {
    let (mut tcp, address) = dial(&target.addresses, wait)?;
    tcp.set_nodelay(true).map_err(|error| error.to_string())?;
    tcp.set_read_timeout(Some(HANDSHAKE)).map_err(|error| error.to_string())?;
    let mut connection = ClientConnection::new(tls::client(&identity.secret(), target.machine.clone(), target.keys.signing)?, tls::name()).map_err(|error| error.to_string())?;
    while connection.is_handshaking() {
        connection.complete_io(&mut tcp).map_err(|error| format!("{address} answered, but the TLS handshake failed: {error}; only {} with the key the account's list has for it, and a machine on that list, completes it", target.name))?;
    }
    Ok((Link::new(Box::new(StreamOwned::new(connection, tcp))), address))
}

// Times a ping over a link already made, which is the round trip a command's output takes.
pub fn ping(link: &mut Link) -> Result<Duration> {
    let started = Instant::now();
    link.send(&Frame::Ask(Ask::Ping)).map_err(|error| error.to_string())?;
    match link.expect(HANDSHAKE).map_err(|error| error.to_string())? {
        Frame::Said(Said::Pong) => Ok(started.elapsed()),
        other => Err(format!("the ping was answered with {other:?}")),
    }
}

// One shell line from the words after --: a single word is taken as the user's own shell line, several are quoted one by one.
pub fn shell_line(argv: &[String]) -> String {
    match argv {
        [one] => one.clone(),
        words => words.iter().map(|word| crate::sh::quote(word)).collect::<Vec<_>>().join(" "),
    }
}

fn bundle_for(top: &Path, refname: &str, commit: &str, haves: &[String], scratch: &Path) -> Result<Vec<u8>> {
    let known: Vec<String> = haves.iter().filter(|have| is_commit(have) && bundle::has(top, have)).map(|have| format!("^{have}")).collect();
    if haves.iter().any(|have| is_commit(have) && crate::repo::git(top, &["merge-base", "--is-ancestor", commit, have]).is_ok()) {
        return Ok(Vec::new());
    }
    let file = scratch.with_extension("bundle");
    let path = file.to_string_lossy().to_string();
    let mut args = vec!["bundle", "create", "--quiet", path.as_str(), refname];
    args.extend(known.iter().map(String::as_str));
    let made = crate::repo::git(top, &args).and_then(|_| std::fs::read(&file).map_err(|error| format!("{}: {error}", file.display())));
    let _ = std::fs::remove_file(&file);
    made
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ran {
    pub code: i32,
    pub sent: Option<usize>,
}

struct Snapshot<'a> {
    top: &'a Path,
    refname: String,
}

impl Drop for Snapshot<'_> {
    fn drop(&mut self) {
        let _ = crate::repo::git(self.top, &["update-ref", "-d", &self.refname]);
    }
}

// The worktree as it stands goes to the target's per-task worktree, then the command runs there with its output streamed here and Ctrl-C forwarded as SIGINT, then SIGTERM, then SIGKILL.
pub fn go(link: &mut Link, top: &Path, argv: &[String], out: &mut dyn Write, err: &mut dyn Write, interrupts: &dyn Fn() -> u32) -> Result<Ran> {
    let id = cluster::new_id()?;
    let commit = super::super::snapshot(top, id.as_str())?;
    let snapshot = Snapshot { top, refname: format!("{REF}/{id}") };
    crate::repo::git(top, &["update-ref", &snapshot.refname, &commit])?;
    let branch = crate::repo::git(top, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let task = token(if branch.is_empty() || branch == "HEAD" { "detached" } else { &branch });
    let remote = bundle::remote_base(top, &commit).map(|remote| RemoteAsk { url: remote.url, base: remote.base });
    let ask = RunAsk { repo: token(&super::super::repo_name(top)), task, commit: commit.clone(), remote, command: shell_line(argv) };
    let lost = |error: std::io::Error| format!("the link was lost: {error}");
    link.send(&Frame::Ask(Ask::Run(ask))).map_err(lost)?;
    let mut sent = None;
    loop {
        match link.expect(SYNC_WAIT).map_err(lost)? {
            Frame::Said(Said::Want { haves }) => {
                let bytes = bundle_for(top, &snapshot.refname, &commit, &haves, &std::env::temp_dir().join(format!("kebbie-link-{id}")))?;
                for chunk in bytes.chunks(CHUNK) {
                    link.send(&Frame::Data(chunk.to_vec())).map_err(lost)?;
                }
                link.send(&Frame::End).map_err(lost)?;
                sent = Some(bytes.len());
            }
            Frame::Err(bytes) => {
                let _ = err.write_all(&bytes).and_then(|()| err.flush());
            }
            Frame::Said(Said::Started) => break,
            Frame::Said(Said::Failed { reason }) => return Err(reason),
            other => return Err(format!("the other machine sent {other:?} before the command started")),
        }
    }
    drop(snapshot);
    let mut forwarded = 0u32;
    loop {
        let asked = interrupts();
        while forwarded < asked && (forwarded as usize) < ESCALATION.len() {
            link.send(&Frame::Signal(ESCALATION[forwarded as usize])).map_err(lost)?;
            forwarded += 1;
        }
        match link.recv(Some(Duration::from_millis(50))).map_err(lost)? {
            None => {}
            Some(Frame::Out(bytes)) => {
                let _ = out.write_all(&bytes).and_then(|()| out.flush());
            }
            Some(Frame::Err(bytes)) => {
                let _ = err.write_all(&bytes).and_then(|()| err.flush());
            }
            Some(Frame::Said(Said::Exited { code })) => return Ok(Ran { code, sent }),
            Some(Frame::Said(Said::Failed { reason })) => return Err(reason),
            Some(other) => return Err(format!("the other machine sent {other:?} while the command ran")),
        }
    }
}

struct Here {
    dir: PathBuf,
    identity: Identity,
    account: AccountKey,
}

fn here() -> Result<Here> {
    let state = crate::lead::state_dir()?;
    let dir = identity::dir(&state);
    let identity = identity::load_at(&dir)?.ok_or("This machine is not connected to a hub, and a link goes to another machine of your hub account: connect it with kebbie hub connect")?;
    let account = identity.account.as_ref().map(|account| account.key.clone()).ok_or("This machine has no hub account pinned, so it cannot tell your other machines from anyone's: kebbie hub leave, then kebbie hub connect")?;
    Ok(Here { dir, identity, account })
}

fn names(row: &kebbie_hub_api::AccountMachine, on: &str) -> bool {
    row.name.eq_ignore_ascii_case(on) || token(&row.name) == token(on)
}

// This machine when on names it, else the worker kebbie job run would pick.
fn target(here: &Here, client: &Client, cluster: &Cluster, on: Option<&str>) -> Result<Target> {
    let me = &here.identity.machine;
    if let Some(row) = cluster.rows.iter().find(|row| &row.machine == me && on.is_some_and(|on| names(row, on))) {
        if !crate::config::worker_enabled().unwrap_or(false) {
            return Err(format!("{} is this machine, and its [worker] enabled is off, so it takes no links", row.name));
        }
        let keys = *cluster.keys.get(me).ok_or("this machine is not on the account's list")?;
        return Ok(Target { machine: me.clone(), name: row.name.clone(), keys, addresses: super::listening(&[]).into_iter().take(1).collect() });
    }
    let now = crate::messages::now();
    let row = cluster::pick(cluster, me, on, now).ok_or_else(|| super::super::master::no_live_worker(cluster, me, on, now))?;
    let keys = *cluster.keys.get(&row.machine).ok_or("the worker is not on the account's list")?;
    let announced = addresses::announced(&here.identity, client, cluster, now)?;
    Ok(Target { machine: row.machine.clone(), name: row.name.clone(), keys, addresses: dialable(&row.name, announced.get(&row.machine))? })
}

static INTERRUPTS: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_interrupt(_: libc::c_int) {
    if INTERRUPTS.fetch_add(1, Ordering::SeqCst) + 1 >= ESCALATION.len() as u32 {
        // SAFETY: signal() is async-signal-safe.
        unsafe { libc::signal(libc::SIGINT, libc::SIG_DFL) };
    }
}

fn catch_interrupt() {
    // SAFETY: the handler only touches an atomic and calls signal(), both async-signal-safe.
    unsafe { libc::signal(libc::SIGINT, on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t) };
}

fn hub_job_hint(top: &Path, why: &str, name: &str) -> String {
    format!("kebbie: no link to {name}: {why}\nTo run it through the hub instead, which is slower but needs no link, add it to [jobs] in {} and run kebbie job run <name>", crate::config::repo_file(top).display())
}

fn run(cwd: &Path, on: Option<&str>, argv: &[String]) -> Result<i32> {
    if argv.is_empty() {
        return Err("kebbie run --on <system or machine> -- <command …> runs the command there, on this worktree as it stands".to_string());
    }
    let top = PathBuf::from(crate::repo::git(cwd, &["rev-parse", "--show-toplevel"])?);
    let here = here()?;
    let client = Client::new(&here.identity.url, Some(here.identity.token()), CALL);
    let cluster = cluster::fetch(&here.dir, &client, &here.account).map_err(|error| format!("kebbie: the hub's list of your machines was not read: {error}"))?;
    let target = target(&here, &client, &cluster, on).map_err(|why| hub_job_hint(&top, &why, on.unwrap_or("a worker")))?;
    let (mut link, address) = connect(&here.identity, &target, DIAL).map_err(|why| hub_job_hint(&top, &why, &target.name))?;
    eprintln!("kebbie: {} runs it, direct on {address}", target.name);
    catch_interrupt();
    let ran = go(&mut link, &top, argv, &mut std::io::stdout(), &mut std::io::stderr(), &|| INTERRUPTS.load(Ordering::SeqCst))?;
    Ok(ran.code)
}

pub fn run_cli(cwd: &Path, on: Option<&str>, argv: &[String]) -> ExitCode {
    match run(cwd, on, argv) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(255)
        }
    }
}

// Each machine of the account and how a link reaches it now: direct on which address and in what round trip, or why not.
pub fn list_cli() -> Result<String> {
    let here = here()?;
    let client = Client::new(&here.identity.url, Some(here.identity.token()), CALL);
    let cluster = cluster::fetch(&here.dir, &client, &here.account).map_err(|error| format!("the hub's list of your machines was not read: {error}"))?;
    let announced = addresses::announced(&here.identity, &client, &cluster, crate::messages::now());
    let mut lines = Vec::new();
    for row in &cluster.rows {
        let Some(keys) = cluster.keys.get(&row.machine).copied() else { continue };
        let me = row.machine == here.identity.machine;
        let addresses = if me {
            if crate::config::worker_enabled().unwrap_or(false) { Ok(vec![SocketAddr::from(([127, 0, 0, 1], PORT))]) } else { Err("this machine; its [worker] enabled is off, so it takes no links".to_string()) }
        } else {
            announced.as_ref().map_err(Clone::clone).and_then(|announced| dialable(&row.name, announced.get(&row.machine)))
        };
        let path = addresses.and_then(|addresses| {
            let target = Target { machine: row.machine.clone(), name: row.name.clone(), keys, addresses };
            let (mut link, address) = connect(&here.identity, &target, DIAL)?;
            let rtt = ping(&mut link)?;
            Ok(format!("direct on {address}, {:.1} ms", rtt.as_secs_f64() * 1000.0))
        });
        lines.push(format!("{}{}: {}", row.name, if me { " (this machine)" } else { "" }, path.unwrap_or_else(|why| format!("down: {why}"))));
    }
    Ok(if lines.is_empty() { "No machine is on your hub account's list".to_string() } else { lines.join("\n") })
}
