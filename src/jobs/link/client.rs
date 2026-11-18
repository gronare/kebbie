use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kebbie_hub_protocol::{AccountKey, Addresses, Id, Network, PublicKeys, is_commit};

use crate::config::LinkNetwork;
use rustls::{ClientConnection, StreamOwned};

use super::super::cluster::{self, Cluster};
use super::super::{REF, bundle, token};
use super::wire::{Ask, CHUNK, Frame, Link, RemoteAsk, RunAsk, Said, beating};
use super::wire::keep_alive;
use super::unanswered::Unanswered;
use super::{PORT, addresses, relay, timing, tls};
use crate::hub::http::{CALL, Client};
use crate::hub::identity::{self, Identity};
use crate::sh::Result;

const DIAL: Duration = Duration::from_secs(2);
const HANDSHAKE: Duration = Duration::from_secs(5);
const HEAD_START: Duration = Duration::from_millis(300);
pub const STAGGER: Duration = Duration::from_millis(150);
const ESCALATION: [i32; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGKILL];

// The machine a link goes to: its key on the account's list and the addresses it announced by network, most wanted first, or loopback when it is this machine.
#[derive(Clone, Debug)]
pub struct Target {
    pub machine: Id,
    pub name: String,
    pub keys: PublicKeys,
    pub tiers: Vec<Vec<SocketAddr>>,
    pub no_address: Option<String>,
}

impl Target {
    pub fn of(machine: Id, name: String, keys: PublicKeys, dialed: Result<Vec<Vec<SocketAddr>>>) -> Target {
        match dialed {
            Ok(tiers) => Target { machine, name, keys, tiers, no_address: None },
            Err(why) => Target { machine, name, keys, tiers: Vec::new(), no_address: Some(why) },
        }
    }

    pub fn addresses(&self) -> Vec<SocketAddr> {
        self.tiers.concat()
    }
}

fn shown(network: Network) -> &'static str {
    match network {
        Network::Tailscale => "Tailscale",
        Network::Wireguard => "WireGuard",
        _ => "a network this kebbie does not know",
    }
}

// The announced addresses to dial, one tier per network in link.networks' order, skipping a network it leaves out; why there are none when there are none.
pub fn dialable(name: &str, announced: Option<&Addresses>, networks: &[LinkNetwork]) -> Result<Vec<Vec<SocketAddr>>> {
    let announced = announced.ok_or_else(|| format!("{name} announced no address for a link: is [worker] enabled = true there, and its kebbie new enough to take links?"))?;
    if announced.port == 0 {
        return Err(format!("{name} takes no links: [worker] enabled is off there"));
    }
    let known: Vec<_> = announced.addresses.iter().filter(|address| address.network != Network::Other).collect();
    if known.is_empty() {
        return Err(format!("{name} is on no private network kebbie knows (Tailscale or WireGuard), so it has no address to take a link on"));
    }
    let tiers: Vec<Vec<SocketAddr>> = networks.iter().map(|wanted| known.iter().filter(|address| address.network == addresses::network_of(*wanted)).filter_map(|address| address.ip.parse().ok().map(|ip| SocketAddr::new(ip, announced.port))).collect::<Vec<_>>()).filter(|tier| !tier.is_empty()).collect();
    if tiers.is_empty() {
        let mut on: Vec<&str> = known.iter().map(|address| shown(address.network)).collect();
        on.dedup();
        return Err(format!("{name} is on {} only, which link.networks on this machine leaves out", on.join(" and ")));
    }
    Ok(tiers)
}

// Each tier's addresses at once, the next tier after stagger or as soon as every address dialed so far failed, and the first to answer wins.
pub fn dial(tiers: &[Vec<SocketAddr>], wait: Duration, stagger: Duration) -> Result<(TcpStream, SocketAddr)> {
    let (sender, dialed) = mpsc::channel();
    let (mut next, mut pending, mut failed) = (0, 0usize, Vec::new());
    loop {
        let heard = match (pending, next < tiers.len()) {
            (0, false) => break,
            (0, true) => None,
            (_, true) => dialed.recv_timeout(stagger).ok(),
            (_, false) => match dialed.recv() {
                Ok(heard) => Some(heard),
                Err(_) => break,
            },
        };
        let Some((address, result)) = heard else {
            for address in &tiers[next] {
                let (sender, address) = (sender.clone(), *address);
                pending += 1;
                std::thread::spawn(move || {
                    let _ = sender.send((address, TcpStream::connect_timeout(&address, wait)));
                });
            }
            next += 1;
            continue;
        };
        pending -= 1;
        match result {
            Ok(tcp) => return Ok((tcp, address)),
            Err(error) => failed.push(format!("{address} ({error})")),
        }
    }
    Err(format!("none of its addresses answered within {} s: {}", wait.as_secs(), failed.join(", ")))
}

// How a link reached its machine.
#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    Direct(SocketAddr),
    Relay,
}

impl std::fmt::Display for Route {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Route::Direct(address) => write!(formatter, "direct on {address}"),
            Route::Relay => write!(formatter, "relayed through the hub"),
        }
    }
}

fn secure<T: std::io::Read + std::io::Write + super::wire::Timed + Send + 'static>(identity: &Identity, target: &Target, mut transport: T, path: &Route) -> Result<Link> {
    transport.wait_at_most(Some(HANDSHAKE)).map_err(|error| error.to_string())?;
    let mut connection = ClientConnection::new(tls::client(&identity.secret(), target.machine.clone(), target.keys.signing)?, tls::name()).map_err(|error| error.to_string())?;
    while connection.is_handshaking() {
        connection.complete_io(&mut transport).map_err(|error| format!("{path} answered, but the TLS handshake failed: {error}; only {} with the key the account's list has for it, and a machine on that list, completes it", target.name))?;
    }
    timing::mark("tls handshake");
    Ok(Link::new(Box::new(StreamOwned::new(connection, transport))))
}

pub fn direct(identity: &Identity, target: &Target, wait: Duration) -> Result<(Link, Route)> {
    if target.tiers.is_empty() {
        return Err(target.no_address.clone().unwrap_or_else(|| format!("{} announced no address", target.name)));
    }
    let (tcp, address) = dial(&target.tiers, wait, STAGGER)?;
    tcp.set_nodelay(true).map_err(|error| error.to_string())?;
    keep_alive(&tcp);
    let path = Route::Direct(address);
    Ok((secure(identity, target, tcp, &path)?, path))
}

enum Racer {
    Direct,
    Relay,
}

fn race(sender: &mpsc::Sender<(Racer, Result<(Link, Route)>)>, racer: Racer, identity: &Identity, target: &Target, wait: Duration) {
    let (sender, identity, target) = (sender.clone(), identity.clone(), target.clone());
    std::thread::spawn(move || {
        let linked = match racer {
            Racer::Direct => direct(&identity, &target, wait),
            Racer::Relay => relay::ask(&identity, &target.machine, &target.name).and_then(|transport| secure(&identity, &target, transport, &Route::Relay)).map(|link| (link, Route::Relay)),
        };
        let _ = sender.send((racer, linked));
    });
}

// Direct and through the hub's relay at once, the relay after the direct addresses had HEAD_START unless they lost to it lately, and the first link made wins; this machine itself only directly.
pub fn connect(identity: &Identity, target: &Target, wait: Duration, unanswered: &Unanswered) -> Result<(Link, Route)> {
    if target.machine == identity.machine {
        return direct(identity, target, wait);
    }
    let (sender, linked) = mpsc::channel();
    let dialing = !target.tiers.is_empty();
    let lost_lately = dialing && unanswered.lately(&target.machine, &target.addresses());
    let mut why_direct = target.no_address.clone().unwrap_or_else(|| format!("{} announced no address", target.name));
    let mut why_relay = None::<String>;
    if dialing {
        race(&sender, Racer::Direct, identity, target, wait);
    }
    let mut relaying = !dialing || lost_lately;
    if relaying {
        race(&sender, Racer::Relay, identity, target, wait);
    }
    let (mut direct_open, mut relay_open) = (dialing, relaying);
    while direct_open || relay_open {
        let next = if relaying { linked.recv().ok() } else { linked.recv_timeout(HEAD_START).ok() };
        let Some((racer, result)) = next else {
            race(&sender, Racer::Relay, identity, target, wait);
            (relaying, relay_open) = (true, true);
            continue;
        };
        match (racer, result) {
            (Racer::Direct, Ok(linked)) => {
                unanswered.forget(&target.machine);
                return Ok(linked);
            }
            (Racer::Relay, Ok(linked)) => {
                if dialing {
                    unanswered.remember(&target.machine, &target.addresses());
                }
                return Ok(linked);
            }
            (Racer::Direct, Err(why)) => {
                (why_direct, direct_open) = (why, false);
                if !relaying {
                    race(&sender, Racer::Relay, identity, target, wait);
                    (relaying, relay_open) = (true, true);
                }
            }
            (Racer::Relay, Err(why)) => (why_relay, relay_open) = (Some(why), false),
        }
    }
    Err(format!("{why_direct}; and {}", why_relay.unwrap_or_default()))
}

// Times a ping over a link already made, which is the round trip a command's output takes.
pub fn ping(link: &mut Link) -> Result<Duration> {
    let started = Instant::now();
    link.send(&Frame::Ask(Ask::Ping)).map_err(|error| error.to_string())?;
    match link.expect().map_err(|error| error.to_string())? {
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

fn quiet(name: &str, seconds: u64) -> String {
    format!("kebbie: {name} sent nothing for {seconds} s, so kebbie stopped waiting. The machine or the network between went away; the command may still be running there")
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
pub fn go(link: &mut Link, name: &str, top: &Path, argv: &[String], out: &mut dyn Write, err: &mut dyn Write, interrupts: &dyn Fn() -> u32) -> Result<Ran> {
    let id = cluster::new_id()?;
    let commit = super::super::snapshot(top, id.as_str())?;
    timing::mark("snapshot");
    let snapshot = Snapshot { top, refname: format!("{REF}/{id}") };
    crate::repo::git(top, &["update-ref", &snapshot.refname, &commit])?;
    let branch = crate::repo::git(top, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let task = token(if branch.is_empty() || branch == "HEAD" { "detached" } else { &branch });
    let remote = bundle::remote_base(top, &commit).map(|remote| RemoteAsk { url: remote.url, base: remote.base });
    timing::mark("ref, branch, remote base");
    let ask = RunAsk { repo: token(&super::super::repo_name(top)), task, commit: commit.clone(), remote, command: shell_line(argv) };
    let silent = link.silent.as_secs();
    let lost = |error: std::io::Error| if error.kind() == std::io::ErrorKind::TimedOut { quiet(name, silent) } else { format!("kebbie: the link to {name} was lost: {error}") };
    link.send(&Frame::Ask(Ask::Run(ask))).map_err(lost)?;
    let mut sent = None;
    loop {
        if interrupts() > 0 {
            return Err(format!("kebbie: stopped before the command started on {name}"));
        }
        let Some(frame) = link.recv(Some(Duration::from_millis(50))).map_err(lost)? else {
            if link.gone_quiet() {
                return Err(quiet(name, silent));
            }
            link.beat_if_due().map_err(lost)?;
            continue;
        };
        match frame {
            Frame::Said(Said::Want { haves }) => {
                let scratch = std::env::temp_dir().join(format!("kebbie-link-{id}"));
                timing::mark("worker wants commit");
                let bytes = beating(link, || bundle_for(top, &snapshot.refname, &commit, &haves, &scratch))?;
                timing::mark("bundle made");
                for chunk in bytes.chunks(CHUNK) {
                    link.send(&Frame::Data(chunk.to_vec())).map_err(lost)?;
                }
                link.send(&Frame::End).map_err(lost)?;
                sent = Some(bytes.len());
                timing::mark("bundle sent");
            }
            Frame::Err(bytes) => {
                let _ = err.write_all(&bytes).and_then(|()| err.flush());
            }
            Frame::Said(Said::Started) => {
                timing::mark("worker started command");
                break;
            }
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
            None if link.gone_quiet() => return Err(quiet(name, silent)),
            None => link.beat_if_due().map_err(lost)?,
            Some(Frame::Out(bytes)) => {
                timing::mark("output");
                let _ = out.write_all(&bytes).and_then(|()| out.flush());
            }
            Some(Frame::Err(bytes)) => {
                let _ = err.write_all(&bytes).and_then(|()| err.flush());
            }
            Some(Frame::Said(Said::Exited { code })) => {
                timing::mark("exited");
                return Ok(Ran { code, sent });
            }
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

impl Here {
    fn unanswered(&self) -> Unanswered {
        Unanswered::at(&self.dir.join("unanswered"))
    }
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
        return Ok(Target { machine: me.clone(), name: row.name.clone(), keys, tiers: vec![super::listening(&[]).into_iter().take(1).collect()], no_address: None });
    }
    let now = crate::messages::now();
    let row = cluster::pick(cluster, me, on, now).ok_or_else(|| super::super::master::no_live_worker(cluster, me, on, now))?;
    let keys = *cluster.keys.get(&row.machine).ok_or("the worker is not on the account's list")?;
    let dialed = addresses::announced(&here.identity, client, cluster, now).and_then(|announced| dialable(&row.name, announced.get(&row.machine), &super::networks()));
    Ok(Target::of(row.machine.clone(), row.name.clone(), keys, dialed))
}

static INTERRUPTS: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_interrupt(_: libc::c_int) {
    if INTERRUPTS.fetch_add(1, Ordering::SeqCst) + 1 >= ESCALATION.len() as u32 {
        // SAFETY: signal() is async-signal-safe.
        unsafe { libc::signal(libc::SIGINT, libc::SIG_DFL) };
    }
}

// SIGTERM or SIGHUP asks as two Ctrl-C would: SIGINT then SIGTERM to the command, or a clean stop before it started; a second one kills it.
extern "C" fn on_terminate(signal: libc::c_int) {
    if INTERRUPTS.fetch_max(2, Ordering::SeqCst) >= 2 {
        INTERRUPTS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: signal() is async-signal-safe.
        unsafe { libc::signal(signal, libc::SIG_DFL) };
    }
}

fn catch_interrupt() {
    // SAFETY: the handlers only touch an atomic and call signal(), both async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_terminate as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_terminate as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}

fn hub_job_hint(top: &Path, why: &str, name: &str) -> String {
    format!("kebbie: no link to {name}: {why}\nTo run it through the hub instead, which is slower but needs no link, add it to [jobs] in {} and run kebbie job run <name>", crate::config::repo_file(top).display())
}

fn run(cwd: &Path, on: Option<&str>, argv: &[String]) -> Result<i32> {
    if argv.is_empty() {
        return Err("kebbie run --on <system or machine> -- <command …> runs the command there, on this worktree as it stands".to_string());
    }
    timing::mark("start");
    let top = PathBuf::from(crate::repo::git(cwd, &["rev-parse", "--show-toplevel"])?);
    let here = here()?;
    timing::mark("identity");
    let client = Client::new(&here.identity.url, Some(here.identity.token()), CALL);
    let cluster = cluster::fetch(&here.dir, &client, &here.account).map_err(|error| format!("kebbie: the hub's list of your machines was not read: {error}"))?;
    timing::mark("machine list");
    let target = target(&here, &client, &cluster, on).map_err(|why| hub_job_hint(&top, &why, on.unwrap_or("a worker")))?;
    timing::mark("announced addresses");
    let (mut link, route) = connect(&here.identity, &target, DIAL, &here.unanswered()).map_err(|why| hub_job_hint(&top, &why, &target.name))?;
    timing::mark("link made");
    eprintln!("kebbie: {} runs it, {route}", target.name);
    catch_interrupt();
    let ran = go(&mut link, &target.name, &top, argv, &mut std::io::stdout(), &mut std::io::stderr(), &|| INTERRUPTS.load(Ordering::SeqCst))?;
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

// Each machine of the account and how a link reaches it now: direct on which address or through the hub, in what round trip, or why not.
pub fn list_cli() -> Result<String> {
    let here = here()?;
    let client = Client::new(&here.identity.url, Some(here.identity.token()), CALL);
    let cluster = cluster::fetch(&here.dir, &client, &here.account).map_err(|error| format!("the hub's list of your machines was not read: {error}"))?;
    let announced = addresses::announced(&here.identity, &client, &cluster, crate::messages::now());
    let networks = super::networks();
    let mut lines = Vec::new();
    for row in &cluster.rows {
        let Some(keys) = cluster.keys.get(&row.machine).copied() else { continue };
        let me = row.machine == here.identity.machine;
        let target = if me {
            if crate::config::worker_enabled().unwrap_or(false) { Ok(Target::of(row.machine.clone(), row.name.clone(), keys, Ok(vec![vec![SocketAddr::from(([127, 0, 0, 1], PORT))]]))) } else { Err("this machine; its [worker] enabled is off, so it takes no links".to_string()) }
        } else {
            Ok(Target::of(row.machine.clone(), row.name.clone(), keys, announced.as_ref().map_err(Clone::clone).and_then(|announced| dialable(&row.name, announced.get(&row.machine), &networks))))
        };
        let path = target.and_then(|target| {
            let (mut link, route) = connect(&here.identity, &target, DIAL, &here.unanswered())?;
            let rtt = ping(&mut link)?;
            Ok(format!("{route}, {:.1} ms", rtt.as_secs_f64() * 1000.0))
        });
        lines.push(format!("{}{}: {}", row.name, if me { " (this machine)" } else { "" }, path.unwrap_or_else(|why| format!("down: {why}"))));
    }
    Ok(if lines.is_empty() { "No machine is on your hub account's list".to_string() } else { lines.join("\n") })
}
