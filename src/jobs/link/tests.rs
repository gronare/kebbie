use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io::Write;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use kebbie_hub_protocol::{Addresses, LinkAddress, Network, SecretKeys};
use rustls::ClientConnection;

use super::super::cluster;
use super::super::tests::{Machine, commit, git, repo};
use super::client::{self, Ran, Target};
use super::serve::{self, Host};
use super::wire::Link;
use super::wire::{Ask, Frame, Said};
use super::{addresses, relay, tls};
use crate::hub::tests::{member, open_hub};

fn host(machine: &Machine, data: &Path) -> Arc<Host> {
    Arc::new(Host { state: machine.state.clone(), dir: machine.dir.clone(), identity: machine.identity.clone(), account: machine.account.clone(), data: data.to_path_buf() })
}

struct Listening {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn listen(host: Arc<Host>) -> Listening {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let seen = stop.clone();
    serve::listen(host, listener, Arc::new(move || seen.load(Ordering::SeqCst)));
    Listening { address, stop }
}

fn target(of: &Machine, name: &str, address: SocketAddr) -> Target {
    Target { machine: of.identity.machine.clone(), name: name.to_string(), keys: of.identity.public(), addresses: vec![address], no_address: None }
}

fn handshake_as(secret: &SecretKeys, peer: &Target) -> Result<(), String> {
    let mut tcp = TcpStream::connect(peer.addresses[0]).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut connection = ClientConnection::new(tls::client(secret, peer.machine.clone(), peer.keys.signing).unwrap(), tls::name()).unwrap();
    while connection.is_handshaking() {
        connection.complete_io(&mut tcp).map_err(|error| error.to_string())?;
    }
    let mut link = Link::new(Box::new(rustls::StreamOwned::new(connection, tcp)));
    client::ping(&mut link).map(|_| ())
}

fn keeper_log(machine: &Machine) -> String {
    std::fs::read_to_string(machine.state.join(crate::keeper::LOG)).unwrap_or_default()
}

#[test]
fn private_addresses_are_tailnet_and_wireguard_only_with_the_tailnet_first() {
    let ip = |text: &str| text.parse::<IpAddr>().unwrap();
    let interfaces = vec![
        ("lo".to_string(), ip("127.0.0.1")),
        ("eth0".to_string(), ip("192.168.10.11")),
        ("eth0".to_string(), ip("fe80::1")),
        ("wg0".to_string(), ip("10.8.0.2")),
        ("work".to_string(), ip("10.9.0.5")),
        ("tailscale0".to_string(), ip("100.101.102.103")),
        ("tailscale0".to_string(), ip("fd7a:115c:a1e0::1")),
        ("utun4".to_string(), ip("fe80::2")),
    ];
    let found = addresses::classify(&interfaces, &BTreeSet::from(["work".to_string()]), &[ip("100.101.102.103")]);
    let pairs: Vec<(Network, &str)> = found.iter().map(|address| (address.network, address.ip.as_str())).collect();
    assert_eq!(pairs, [(Network::Tailscale, "100.101.102.103"), (Network::Tailscale, "fd7a:115c:a1e0::1"), (Network::Wireguard, "10.8.0.2"), (Network::Wireguard, "10.9.0.5")], "no loopback, no LAN, no link-local, each once");
    assert!(addresses::classify(&[("eth0".to_string(), ip("203.0.113.9"))], &BTreeSet::new(), &[]).is_empty(), "a public interface is never announced");
    let listened = super::listening(&found);
    assert!(listened.iter().all(|address| !address.ip().is_unspecified()) && listened[0] == SocketAddr::from(([127, 0, 0, 1], super::PORT)), "loopback for this machine, the private addresses for the others, never 0.0.0.0: {listened:?}");
}

#[test]
fn a_machine_announces_its_addresses_to_the_others_on_the_account_and_they_trust_only_what_it_signed() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let (devbox, studio) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"));
    let now = crate::messages::now();
    let listed = cluster::fetch(&studio.dir, &studio.client(), &studio.account).unwrap();
    let announced = Addresses { port: 47_011, addresses: vec![LinkAddress { network: Network::Tailscale, ip: "100.101.102.103".into() }, LinkAddress { network: Network::Wireguard, ip: "10.8.0.2".into() }] };
    assert_eq!(addresses::announce(&studio.identity, &studio.account, &studio.client(), &listed, &announced, now).unwrap(), 1, "to every other machine on the list");
    let seen = cluster::fetch(&devbox.dir, &devbox.client(), &devbox.account).unwrap();
    let read = addresses::announced(&devbox.identity, &devbox.client(), &seen, now).unwrap();
    assert_eq!(read.get(&studio.identity.machine), Some(&announced));
    assert_eq!(client::dialable("studio", read.get(&studio.identity.machine)).unwrap(), ["100.101.102.103:47011".parse::<SocketAddr>().unwrap(), "10.8.0.2:47011".parse().unwrap()]);
    let mut unlisted = seen.clone();
    unlisted.keys.remove(&studio.identity.machine);
    assert!(addresses::announced(&devbox.identity, &devbox.client(), &unlisted, now).unwrap().is_empty(), "an announcement from a machine not on the list is dropped");
    addresses::announce(&studio.identity, &studio.account, &studio.client(), &listed, &Addresses { port: 0, addresses: Vec::new() }, now).unwrap();
    let read = addresses::announced(&devbox.identity, &devbox.client(), &seen, now).unwrap();
    assert!(client::dialable("studio", read.get(&studio.identity.machine)).unwrap_err().contains("studio takes no links: [worker] enabled is off there"));
    assert!(client::dialable("mac", None).unwrap_err().contains("announced no address"));
}

#[test]
fn a_link_opens_only_between_keys_on_the_accounts_list_as_it_is_now() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
#[test]
fn an_address_on_a_network_a_newer_kebbie_announced_is_skipped_not_dialed() {
    let mixed = Addresses { port: 47_011, addresses: vec![LinkAddress { network: Network::Other, ip: "100.64.9.9".into() }, LinkAddress { network: Network::Wireguard, ip: "10.8.0.2".into() }] };
    assert_eq!(client::dialable("studio", Some(&mixed)).unwrap(), ["10.8.0.2:47011".parse::<SocketAddr>().unwrap()]);
    let only_other = Addresses { port: 47_011, addresses: vec![LinkAddress { network: Network::Other, ip: "100.64.9.9".into() }] };
    assert!(client::dialable("studio", Some(&only_other)).unwrap_err().contains("studio is on no private network kebbie knows"), "and with nothing else it has no address, so the hub relays");
}

    let (devbox, studio, laptop) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"), Machine::connected(&hub, &mut browser, "laptop"));
    let root = crate::sh::test_dir("link-pinning");
    let listening = listen(host(&studio, &root.join("data")));
    let studio_at = target(&studio, "studio", listening.address);
    let (mut link, _) = client::connect(&devbox.identity, &studio_at, Duration::from_secs(2)).unwrap();
    assert!(client::ping(&mut link).is_ok(), "a listed machine reaches another");
    assert!(handshake_as(&laptop.identity.secret(), &studio_at).is_ok());
    assert!(handshake_as(&SecretKeys::generate(), &studio_at).is_err(), "a key on no list is refused");
    let impostor = Target { keys: laptop.identity.public(), ..studio_at.clone() };
    assert!(client::connect(&devbox.identity, &impostor, Duration::from_secs(2)).err().unwrap().contains("TLS handshake failed"), "the asker refuses a machine that does not show the key it expects");
    hub.runtime.block_on(browser.remove_machine(&laptop.identity.machine)).unwrap();
    assert!(handshake_as(&laptop.identity.secret(), &studio_at).is_err(), "a machine the account removed is refused at once, as the list is read fresh");
    let log = keeper_log(&studio);
    assert!(log.contains("link: session from devbox (127.0.0.1:") && log.contains(": ping"), "one line per session: {log}");
    assert!(log.contains("link: refused a session from 127.0.0.1:"), "{log}");
    let _ = std::fs::remove_dir_all(root);
}

struct Captured(Rc<RefCell<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Run {
    ran: Result<Ran, String>,
    out: String,
    err: String,
}

fn run_on(asker: &Machine, at: &Target, top: &Path, argv: &[&str], interrupt_after: Option<&str>) -> Run {
    let (out, err) = (Rc::new(RefCell::new(Vec::new())), Rc::new(RefCell::new(Vec::new())));
    let watched = out.clone();
    let interrupts = || u32::from(interrupt_after.is_some_and(|marker| String::from_utf8_lossy(&watched.borrow()).contains(marker)));
    let argv: Vec<String> = argv.iter().map(|word| word.to_string()).collect();
    let ran = client::connect(&asker.identity, at, Duration::from_secs(2)).and_then(|(mut link, _)| client::go(&mut link, &at.name, top, &argv, &mut Captured(out.clone()), &mut Captured(err.clone()), &interrupts));
    let text = |bytes: &Rc<RefCell<Vec<u8>>>| String::from_utf8_lossy(&bytes.borrow()).to_string();
    Run { ran, out: text(&out), err: text(&err) }
}

#[test]
fn a_run_streams_its_output_returns_its_exit_code_and_syncs_only_what_the_worker_lacks() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let (devbox, studio) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"));
    let root = crate::sh::test_dir("link-run");
    let top = repo(&root);
    std::fs::write(top.join(".gitignore"), "target/\n").unwrap();
    commit(&top, "ignore target");
    std::fs::write(top.join("tracked.txt"), "edited\n").unwrap();
    std::fs::write(top.join("new.txt"), "untracked\n").unwrap();
    let listening = listen(host(&studio, &root.join("data")));
    let at = target(&studio, "studio", listening.address);

    let first = run_on(&devbox, &at, &top, &["cat tracked.txt new.txt; echo $KEBBIE_RUN_FROM $KEBBIE_RUN_TASK; echo oops >&2; mkdir -p target; touch target/cache stray.txt; exit 7"], None);
    assert_eq!(first.ran.as_ref().map(|ran| ran.code), Ok(7), "{}", first.err);
    assert_eq!(first.out, "edited\nuntracked\ndevbox billing-refunds\n", "uncommitted and untracked files run too");
    assert_eq!(first.err, "oops\n", "stderr comes apart from stdout");
    assert!(first.ran.unwrap().sent.is_some_and(|bytes| bytes > 0), "the first run sends the history");

    let again = run_on(&devbox, &at, &top, &["ls", "-A"], None);
    assert_eq!(again.ran.as_ref().map(|ran| (ran.code, ran.sent)), Ok((0, None)), "an unchanged worktree sends nothing at all: {}", again.err);
    assert!(again.out.contains("target\n") && !again.out.contains("stray.txt"), "the worktree is cleaned without -x, so ignored build caches survive: {}", again.out);

    for index in 0..20 {
        std::fs::write(top.join(format!("bulk-{index}.txt")), format!("{index}\n").repeat(2_000)).unwrap();
    }
    commit(&top, "bulk");
    let whole = {
        let file = root.join("whole.bundle");
        git(&top, &["bundle", "create", "--quiet", &file.to_string_lossy(), "HEAD"]);
        std::fs::metadata(&file).unwrap().len() as usize
    };
    std::fs::write(top.join("fourth.txt"), "four\n").unwrap();
    let small = run_on(&devbox, &at, &top, &["cat", "fourth.txt"], None);
    assert_eq!((small.ran.as_ref().map(|ran| ran.code), small.out.as_str()), (Ok(0), "four\n"));
    let sent = small.ran.unwrap().sent.unwrap();
    assert!(sent > 0 && sent < whole, "only the objects the worker lacks go: {sent} of {whole} bytes");

    let failed = run_on(&devbox, &at, &top, &["exit 130"], None);
    assert_eq!(failed.ran.map(|ran| ran.code), Ok(130));
    assert!(keeper_log(&studio).contains("link: session from devbox (127.0.0.1:") && keeper_log(&studio).contains(": exited 7"), "{}", keeper_log(&studio));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ctrl_c_on_the_asker_reaches_the_commands_process_group_on_the_worker() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let (devbox, studio) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"));
    let root = crate::sh::test_dir("link-interrupt");
    let top = repo(&root);
    let listening = listen(host(&studio, &root.join("data")));
    let at = target(&studio, "studio", listening.address);
    let started = Instant::now();
    let stopped = run_on(&devbox, &at, &top, &["trap 'echo caught; exit 42' INT; echo ready; while true; do sleep 0.1; done"], Some("ready"));
    assert_eq!(stopped.ran.map(|ran| ran.code), Ok(42), "{}", stopped.err);
    assert_eq!(stopped.out, "ready\ncaught\n");
    assert!(started.elapsed() < Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_run_on_this_machine_over_loopback_takes_a_short_round_trip() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let devbox = Machine::connected(&hub, &mut browser, "devbox");
    let root = crate::sh::test_dir("link-self");
    let top = repo(&root);
    let listening = listen(host(&devbox, &root.join("data")));
    let at = target(&devbox, "devbox", listening.address);
    assert_eq!(run_on(&devbox, &at, &top, &["true"], None).ran.map(|ran| ran.code), Ok(0), "the first run sends the history");
    let mut took: Vec<Duration> = (0..20).map(|_| {
        let started = Instant::now();
        assert_eq!(run_on(&devbox, &at, &top, &["true"], None).ran.map(|ran| ran.code), Ok(0));
        started.elapsed()
    }).collect();
    took.sort();
    eprintln!("kebbie run --on devbox -- true over loopback: median {:?}, fastest {:?}, slowest {:?}", took[took.len() / 2], took[0], took[took.len() - 1]);
    let _ = std::fs::remove_dir_all(root);
}

fn wait_for_log(machine: &Machine, line: &str) {
    let until = Instant::now() + Duration::from_secs(10);
    while !keeper_log(machine).contains(line) {
        assert!(Instant::now() < until, "the keeper log never said {line:?}: {}", keeper_log(machine));
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_run_goes_through_the_hubs_relay_when_no_address_answers() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let (devbox, studio) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"));
    let root = crate::sh::test_dir("link-relay");
    let top = repo(&root);
    let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let at = target(&studio, "studio", closed);
    assert!(client::connect(&devbox.identity, &at, Duration::from_secs(2)).err().unwrap().contains("studio is not waiting for links through the hub either"), "a machine that waits for no relayed link has none");
    studio.work();
    let stop = Arc::new(AtomicBool::new(false));
    let (waiting_host, seen) = (host(&studio, &root.join("data")), stop.clone());
    let waiting = std::thread::spawn(move || relay::wait_for_rings(waiting_host, &|| true, Arc::new(move || seen.load(Ordering::SeqCst))));
    wait_for_log(&studio, "link: waiting for links relayed through the hub");
    let (mut link, route) = client::connect(&devbox.identity, &at, Duration::from_secs(2)).unwrap();
    assert_eq!(route, client::Route::Relay, "the closed address fails and the hub relays");
    assert!(client::ping(&mut link).is_ok());
    let ran = run_on(&devbox, &at, &top, &["echo out; echo err >&2; exit 5"], None);
    assert_eq!((ran.ran.map(|ran| ran.code), ran.out.as_str(), ran.err.as_str()), (Ok(5), "out\n", "err\n"), "the same session runs through the hub");
    let stopped = run_on(&devbox, &at, &top, &["trap 'echo caught; exit 42' INT; echo ready; while true; do sleep 0.1; done"], Some("ready"));
    assert_eq!(stopped.ran.map(|ran| ran.code), Ok(42), "Ctrl-C reaches it through the hub too: {}", stopped.err);
    wait_for_log(&studio, " through the hub): run ");
    stop.store(true, Ordering::SeqCst);
    waiting.join().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_run_gives_up_in_plain_words_when_the_other_machine_goes_silent() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let (devbox, studio) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"));
    let root = crate::sh::test_dir("link-silent");
    let top = repo(&root);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let at = target(&studio, "studio", listener.local_addr().unwrap());
    let frozen = host(&studio, &root.join("data"));
    let allowed = std::collections::BTreeMap::from([(devbox.identity.public().signing, devbox.identity.machine.clone())]);
    let silent = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let (mut link, _) = serve::handshake(&frozen, tcp, allowed).unwrap();
        assert!(matches!(link.expect().unwrap(), Frame::Ask(Ask::Run(_))));
        link.send(&Frame::Said(Said::Started)).unwrap();
        std::thread::sleep(Duration::from_secs(4));
    });
    let (mut link, _) = client::direct(&devbox.identity, &at, Duration::from_secs(2)).unwrap();
    link.silent = Duration::from_secs(1);
    let started = Instant::now();
    let argv = vec!["make".to_string()];
    let ended = client::go(&mut link, "studio", &top, &argv, &mut std::io::sink(), &mut std::io::sink(), &|| 0);
    assert_eq!(ended, Err("kebbie: studio sent nothing for 1 s, so kebbie stopped waiting. The machine or the network between went away; the command may still be running there".to_string()));
    assert!(started.elapsed() < Duration::from_secs(3), "it gives up once the silence passed, not later");
    silent.join().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_relay_the_hubs_plan_lacks_is_refused_in_plain_words_naming_the_free_way() {
    let refused = Ok(relay::Refused { status: 402, code: "pro-required".to_string() });
    assert_eq!(relay::refusal("studio", "https://hub.kebbie.dev/", &refused), "relaying through hub.kebbie.dev needs Hub Pro; a direct link over Tailscale or WireGuard is free");
    let other = Ok(relay::Refused { status: 402, code: "plan-inactive".to_string() });
    assert_eq!(relay::refusal("studio", "https://hub.kebbie.dev", &other), "the hub refused the relayed link: 402 plan-inactive");
}
