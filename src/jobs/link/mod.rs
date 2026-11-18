use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kebbie_hub_protocol::{Addresses, LinkAddress};

use super::cluster;
use crate::hub::http::{CALL, Client};
use crate::hub::identity;

pub mod addresses;
mod client;
pub mod relay;
pub mod serve;
pub mod tls;
pub mod wire;
#[cfg(test)]
mod tests;

pub use client::{list_cli, run_cli};

pub const PORT: u16 = 47_011;
const TICK: Duration = Duration::from_secs(2);
const LOOK_EVERY: Duration = Duration::from_secs(60);
const ANNOUNCE_EVERY: Duration = Duration::from_secs(6 * 3_600);

static HALT: AtomicBool = AtomicBool::new(false);
static THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

fn halted() -> bool {
    crate::keeper::stopping() || HALT.load(Ordering::SeqCst)
}

// Where this machine listens: loopback for itself and its announced private addresses for the account's other machines, never a public interface or every interface.
pub fn listening(announced: &[LinkAddress]) -> Vec<SocketAddr> {
    std::iter::once(IpAddr::V4(Ipv4Addr::LOCALHOST)).chain(announced.iter().filter_map(|address| address.ip.parse().ok())).map(|ip| SocketAddr::new(ip, PORT)).collect()
}

struct Listening {
    on: BTreeMap<SocketAddr, (Arc<AtomicBool>, std::thread::JoinHandle<()>)>,
}

impl Listening {
    fn drop_all_but(&mut self, wanted: &[SocketAddr], log: &dyn Fn(&str)) {
        let gone: Vec<SocketAddr> = self.on.keys().filter(|address| !wanted.contains(address)).copied().collect();
        for address in gone {
            if let Some((stop, handle)) = self.on.remove(&address) {
                stop.store(true, Ordering::SeqCst);
                let _ = handle.join();
                log(&format!("link: stopped listening on {address}"));
            }
        }
    }

    fn keep(&mut self, host: &Arc<serve::Host>, wanted: &[SocketAddr], log: &dyn Fn(&str)) {
        self.drop_all_but(wanted, log);
        let new: Vec<SocketAddr> = wanted.iter().filter(|address| !self.on.contains_key(address)).copied().collect();
        for address in &new {
            match TcpListener::bind(address) {
                Ok(listener) => {
                    let stop = Arc::new(AtomicBool::new(false));
                    let seen = stop.clone();
                    let handle = serve::listen(host.clone(), listener, Arc::new(move || halted() || seen.load(Ordering::SeqCst)));
                    self.on.insert(*address, (stop, handle));
                    log(&format!("link: listening on {address}"));
                }
                Err(error) => log(&format!("link: could not listen on {address}: {error}")),
            }
        }
    }
}

// The machine, account, hub and token a Host carries, so listeners and the relay wait never outlive a leave or a reconnect.
type Who = (kebbie_hub_protocol::Id, kebbie_hub_protocol::AccountKey, String, String);

fn who(identity: &identity::Identity, account: &kebbie_hub_protocol::AccountKey) -> Who {
    (identity.machine.clone(), account.clone(), identity.url.clone(), identity.token().to_string())
}

// The keeper's link: while worker.enabled is on, it listens and tells the account's other machines where; once off, it stops and tells them it takes no links.
fn serve(state: &Path) {
    let log = |text: &str| crate::keeper::log_line(state, text);
    let mut listeners = Listening { on: BTreeMap::new() };
    let (mut told, mut looked) = (None::<(Addresses, u64, Instant)>, None::<(bool, Instant)>);
    let mut rings: Option<std::thread::JoinHandle<()>> = None;
    let (mut serving, mut current) = (None::<Who>, Arc::new(AtomicBool::new(false)));
    while !halted() {
        let enabled = crate::config::worker_enabled().unwrap_or(false);
        let dir = identity::dir(state);
        let loaded = identity::load_at(&dir).ok().flatten().and_then(|identity| identity.account.clone().map(|account| (identity, account.key)));
        let who = loaded.as_ref().map(|(identity, account)| who(identity, account));
        if who != serving {
            listeners.drop_all_but(&[], &log);
            current.store(false, Ordering::SeqCst);
            current = Arc::new(AtomicBool::new(true));
            serving = who;
            looked = None;
        }
        if looked.is_none_or(|(was, at)| was != enabled || at.elapsed() >= LOOK_EVERY) {
            looked = Some((enabled, Instant::now()));
            if !enabled {
                listeners.drop_all_but(&[], &log);
            }
            if let Some((identity, account)) = loaded {
                let local = if enabled { addresses::local() } else { Vec::new() };
                let announced = Addresses { port: if enabled { PORT } else { 0 }, addresses: local.clone() };
                let host = Arc::new(serve::Host { state: state.to_path_buf(), dir: dir.clone(), identity: identity.clone(), account: account.clone(), data: super::data_dir() });
                listeners.keep(&host, &if enabled { listening(&local) } else { Vec::new() }, &log);
                if enabled && rings.as_ref().is_none_or(std::thread::JoinHandle::is_finished) {
                    let (host, current) = (host.clone(), current.clone());
                    rings = Some(std::thread::spawn(move || relay::wait_for_rings(host, &|| current.load(Ordering::SeqCst) && crate::config::worker_enabled().unwrap_or(false), Arc::new(halted))));
                }
                let client = Client::new(&identity.url, Some(identity.token()), CALL);
                if let Ok(cluster) = cluster::fetch(&dir, &client, &account) {
                    let due = told.as_ref().is_none_or(|(before, counter, at)| *before != announced || *counter != cluster.counter || at.elapsed() >= ANNOUNCE_EVERY);
                    let worth = enabled || told.as_ref().is_some_and(|(before, _, _)| before.port != 0);
                    if due && worth {
                        match addresses::announce(&identity, &account, &client, &cluster, &announced, crate::messages::now()) {
                            Ok(sent) => {
                                let ips: Vec<&str> = announced.addresses.iter().map(|address| address.ip.as_str()).collect();
                                log(&format!("link: told {sent} machines of the account {}", if enabled { format!("to reach this one on {} port {PORT}", if ips.is_empty() { "no private address".to_string() } else { ips.join(", ") }) } else { "that this one takes no links".to_string() }));
                                told = Some((announced, cluster.counter, Instant::now()));
                            }
                            Err(error) => log(&format!("link: the addresses did not reach the hub: {error}")),
                        }
                    }
                }
            }
        }
        let until = Instant::now() + TICK;
        while !halted() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    listeners.drop_all_but(&[], &log);
    if let Some(rings) = rings {
        let _ = rings.join();
    }
}

pub fn spawn(state: &Path) {
    let kept = state.to_path_buf();
    HALT.store(false, Ordering::SeqCst);
    match std::thread::Builder::new().name("link".to_string()).spawn(move || serve(&kept)) {
        Ok(handle) => *THREAD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle),
        Err(error) => crate::keeper::log_line(state, &format!("link: the link thread did not start: {error}")),
    }
}

pub fn stop() {
    HALT.store(true, Ordering::SeqCst);
    let Some(handle) = THREAD.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() else { return };
    let until = Instant::now() + Duration::from_secs(5);
    while !handle.is_finished() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    if handle.is_finished() {
        let _ = handle.join();
    }
    let until = Instant::now() + serve::SETTLE;
    while serve::live() > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
}
