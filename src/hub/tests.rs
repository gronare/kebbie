use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kebbie_hub::{Config, Hub, serve};
use kebbie_hub_fakes::browser::Browser;
use kebbie_hub_protocol::{Board, Id, Kind};

use super::board::tests::asking;
use super::board::build;
use super::identity;
use super::relay::{self, HEARTBEAT};
use crate::status::tests::task;

pub(crate) const WAIT: Duration = Duration::from_secs(15);

pub(crate) struct TestHub {
    pub(crate) url: String,
    pub(crate) hub: Arc<Hub>,
    pub(crate) runtime: tokio::runtime::Runtime,
}

impl TestHub {
    fn start() -> TestHub {
        TestHub::start_with(|_| {})
    }

    pub(crate) fn start_with(configure: impl FnOnce(&mut Config)) -> TestHub {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let mut config = Config::new(crate::sh::test_dir("hub-data"));
        configure(&mut config);
        let hub = Hub::open(config).unwrap();
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        runtime.spawn(serve(listener, Arc::clone(&hub), std::future::pending()));
        TestHub { url, hub, runtime }
    }

    fn joined(&self, name: &str) -> PathBuf {
        let state = crate::sh::test_dir(&format!("hub-{name}"));
        super::join(&state, &self.url, &self.hub.mint_join_code().unwrap(), name, crate::messages::now()).unwrap();
        state
    }

    fn boards(&self, browser: &mut Browser) -> Vec<(u64, Board)> {
        self.runtime.block_on(browser.boards()).unwrap().into_iter().filter_map(|entry| Some((entry.rev?, entry.board.ok()?))).collect()
    }
}

fn now() -> u64 {
    crate::messages::now()
}

fn board_of(items: &[crate::status::Item]) -> Board {
    build(items, &[], Vec::new(), None)
}

#[test]
fn a_machine_joins_and_keeps_its_keys_to_itself() {
    let hub = TestHub::start();
    let state = hub.joined("mac");
    let joined = identity::load(&state).unwrap().unwrap();
    assert_eq!((joined.url.as_str(), joined.name.as_str()), (hub.url.as_str(), "mac"));
    assert!(joined.token().starts_with("khm_"));
    let shown = super::status_text(&state, Some("mac"), now()).unwrap();
    assert!(shown.contains(&hub.url) && shown.contains("mac") && shown.contains("account:  none, so no browser can read this machine's board") && shown.contains("kebbie hub leave, then kebbie hub connect"), "{shown}");
    let (signing, encryption) = joined.secret().to_bytes();
    let file = std::fs::read_to_string(state.join("hub/identity.json")).unwrap();
    for secret in [joined.token().to_string(), format!("{:?}", kebbie_hub_api::Bytes32(signing)), format!("{:?}", kebbie_hub_api::Bytes32(encryption))] {
        assert!(file.contains(&secret), "the file keeps the secret it guards");
        assert!(!shown.contains(&secret), "status never prints a secret");
    }
    let again = super::join(&state, &hub.url, &hub.hub.mint_join_code().unwrap(), "mac", now()).err().unwrap();
    assert!(again.contains("kebbie hub leave first"), "{again}");
    let used = super::join(&crate::sh::test_dir("hub-used"), &hub.url, "AAAA-AAAA", "mac", now()).err().unwrap();
    assert!(used.contains("refused that join code"), "{used}");
}

#[test]
fn plain_http_is_for_a_hub_on_this_machine_only() {
    assert_eq!(super::hub_url("https://hub.example.com/").unwrap(), "https://hub.example.com");
    assert!(super::hub_url("http://127.0.0.1:8470").is_ok() && super::hub_url("http://localhost:8470").is_ok());
    assert!(super::hub_url("http://hub.example.com").unwrap_err().contains("takes https"));
    assert!(super::hub_url("hub.example.com").unwrap_err().contains("not a hub address"));
}

#[test]
fn a_machine_connected_from_the_link_pins_the_account_and_seals_its_board_to_it() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let state = connected_state(&hub, &mut browser);
    let pinned = identity::load(&state).unwrap().unwrap();
    let key = browser.account_key().unwrap().clone();
    let account = pinned.account.as_ref().expect("the machine pinned the account the browser handed over");
    assert_eq!((&account.key, account.fingerprints), (&key, key.fingerprints()));
    let shown = super::status_text(&state, Some("mac"), now()).unwrap();
    let (signing, encryption) = account.prints();
    assert!(shown.contains(&format!("account:  {} · signing key {signing} · encryption key {encryption}", key.account)), "{shown}");

    let mut working = task(Some("working"), Some("working"), 1);
    working.name = "search".to_string();
    let first = board_of(&[asking("refunds", "greenhouse"), working.clone()]);
    let at = now();
    let one = relay::relay(&state, &first, at).unwrap().unwrap();
    let read = hub.runtime.block_on(browser.boards()).unwrap();
    assert_eq!(read.len(), 1);
    let opened = read[0].board.as_ref().unwrap();
    assert_eq!((read[0].rev, &opened.items), (Some(one), &first.items), "the browser opens exactly the board the machine sealed, with the account's key");
    assert_eq!(opened.items[0].question.as_ref().unwrap().text, "Keep the old endpoint for a release?");
    let sealed = relay::seal_board(&identity::load(&state).unwrap().unwrap(), &first, one + 1, at).unwrap();
    assert_eq!(sealed.keys.keys().cloned().collect::<std::collections::BTreeSet<_>>(), [pinned.machine.clone(), key.account.clone()].into(), "sealed to the machine and the account only");

    assert_eq!(relay::relay(&state, &first, at + 1).unwrap(), None, "an unchanged board is not posted again");
    working.git.as_mut().unwrap().ahead = 2;
    let second = board_of(&[asking("refunds", "greenhouse"), working]);
    let two = relay::relay(&state, &second, at + 2).unwrap().unwrap();
    assert!(two > one);
    let (rev, opened) = &hub.boards(&mut browser)[0];
    assert_eq!((*rev, opened.items[1].git.as_ref().unwrap().ahead), (two, 2));

    std::fs::remove_file(state.join("hub/relay.json")).unwrap();
    let three = relay::relay(&state, &second, at + 3).unwrap().unwrap();
    assert!(three > two, "a lost relay.json never rolls the revision back: {three} after {two}");
    assert_eq!(hub.boards(&mut browser)[0].0, three);
}

#[test]
fn an_unchanged_board_is_posted_again_well_within_the_day_a_board_lives() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let state = connected_state(&hub, &mut browser);
    let board = board_of(&[asking("refunds", "greenhouse")]);
    let at = now();
    let identity = identity::load(&state).unwrap().unwrap();
    let sealed = relay::seal_board(&identity, &board, 1, at).unwrap();
    assert!(sealed.header.exp - sealed.header.at <= Kind::Board.max_ttl(), "the hub refuses a board meant to live longer than a day");
    assert!(HEARTBEAT * 60 <= Kind::Board.max_ttl(), "a sleeping-but-alive machine reposts many times within a board's day");
    let one = relay::relay(&state, &board, at).unwrap().unwrap();
    assert_eq!(relay::relay(&state, &board, at + HEARTBEAT - 1).unwrap(), None);
    let two = relay::relay(&state, &board, at + HEARTBEAT).unwrap().expect("the heartbeat posts the same board so the web app sees the machine awake");
    assert!(two > one);
    assert_eq!(hub.boards(&mut browser)[0].0, two);
}

#[test]
fn a_machine_connected_by_a_typed_code_posts_no_board_and_says_to_reconnect_from_the_link() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let state = connected(&hub, &mut browser);
    assert!(identity::load(&state).unwrap().unwrap().account.is_none());
    assert_eq!(relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap(), None, "no board nobody can read");
    let read = hub.runtime.block_on(browser.boards()).unwrap();
    assert!(read.len() == 1 && read[0].rev.is_none(), "the hub holds no board of it: {read:?}");
    let shown = super::status_text(&state, Some("mac"), now()).unwrap();
    assert!(shown.contains("account:  none, so no browser can read this machine's board") && shown.contains("connect it from the link: kebbie hub leave, then kebbie hub connect"), "{shown}");
    let (sent, said) = super::board_line(&relay::load(&state), false, now());
    assert!(sent == super::Sent::Failed && said.contains("kebbie hub leave, then kebbie hub connect"), "{said}");
    let check = super::doctor(&state, Some("mac"), now(), |_| Ok(()));
    assert!(check.level == crate::doctor::Level::Warn && check.fact.contains("no account key") && check.fix == ["kebbie hub leave, then kebbie hub connect, and open the link it shows"], "{} {:?}", check.fact, check.fix);
}

#[test]
fn an_account_key_the_browser_did_not_commit_to_with_the_machines_secret_is_refused_and_nothing_is_pinned() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let state = crate::sh::test_dir("hub-mismatch");
    let mut pending = super::connect::start(&hub.url, "mac").unwrap();
    let link = pending.link();
    let (before, secret) = link.split_once("#k=").unwrap();
    let swapped = if secret.starts_with('A') { "B" } else { "A" };
    let approval = hub.runtime.block_on(browser.approve_connect(&format!("{before}#k={swapped}{}", &secret[1..]), true)).unwrap();
    let answer = pending.wait(&mut |_| true).unwrap();
    let machine = std::thread::spawn(move || super::connect::finish(&state, &pending, &answer, None, || Ok(()), now()).map(|finished| (finished, state)));
    assert!(hub.runtime.block_on(browser.hand_off(approval, WAIT)).is_err(), "the browser sees the machine refuse");
    let (finished, state) = machine.join().unwrap().unwrap();
    let remark = finished.remark.unwrap();
    assert!(remark.starts_with("No board goes out: the hub handed over an account key the browser did not commit to; nothing was pinned; start again with kebbie hub leave, then kebbie hub connect"), "{remark}");
    assert!(identity::load(&state).unwrap().unwrap().account.is_none());
}

#[test]
fn leaving_removes_the_machine_from_the_hub_and_forgets_the_keys() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let state = connected_state(&hub, &mut browser);
    let token = identity::load(&state).unwrap().unwrap().token().to_string();
    relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap();
    assert_eq!(hub.boards(&mut browser).len(), 1);
    let left = super::leave(&state).unwrap();
    assert!(left.contains("This machine left") && left.contains("The hub removed the machine") && !left.contains("remove-machine"), "{left}");
    assert!(identity::load(&state).unwrap().is_none() && !state.join("hub").exists());
    assert!(hub.boards(&mut browser).is_empty(), "the hub dropped the machine's board");
    let refused = super::http::Client::new(&hub.url, Some(&token), super::http::CALL).post::<_, ()>("/v1/m/leave", &serde_json::json!({})).unwrap_err();
    assert_eq!(refused.status(), Some(401), "the token is gone from the hub");
}

#[test]
fn leaving_a_hub_that_knows_the_token_no_more_says_the_machine_is_gone() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let state = connected_state(&hub, &mut browser);
    let (url, calls) = answering("401 Unauthorized", r#"{"error":"unauthorized"}"#);
    pointed_at(&state, &url);
    let left = super::leave(&state).unwrap();
    assert!(left.contains("The hub removed the machine") && !left.contains("remove-machine"), "{left}");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(identity::load(&state).unwrap().is_none());
}

#[test]
fn leaving_an_older_hub_or_one_that_cannot_be_reached_says_how_its_admin_removes_the_machine() {
    let hub = open_hub(|_| {});
    for (status, said) in [("404 Not Found", "This hub cannot remove a machine that leaves"), ("503 Service Unavailable", "The hub could not be told (the hub answered 503")] {
        let mut browser = member(&hub);
        let state = connected_state(&hub, &mut browser);
        let machine = identity::load(&state).unwrap().unwrap().machine;
        let (url, _) = answering(status, r#"{"error":"nope"}"#);
        pointed_at(&state, &url);
        let left = super::leave(&state).unwrap();
        assert!(left.contains("This machine left") && left.contains(said) && left.contains(&format!("kebbie-hub admin remove-machine {machine}")), "{left}");
        assert!(identity::load(&state).unwrap().is_none(), "leaving forgets the keys whatever the hub answered");
    }
}

fn serving(respond: impl Fn(&str) -> String + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut first = String::new();
            let _ = reader.read_line(&mut first);
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
            let mut stream = reader.into_inner();
            let _ = write!(stream, "{}", respond(&path));
        }
    });
    url
}

fn moved_to(target: &str) -> String {
    let target = target.to_string();
    serving(move |path| format!("HTTP/1.1 308 Permanent Redirect\r\nLocation: {target}{path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"))
}

fn answering(status: &'static str, body: &'static str) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let url = serving(move |_| {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    });
    (url, calls)
}

fn pointed_at(state: &Path, url: &str) {
    let mut identity = identity::load(state).unwrap().unwrap();
    identity.url = url.to_string();
    identity::save(state, &identity).unwrap();
}

#[test]
fn a_hub_that_moved_port_is_followed_with_the_same_method_and_body_and_kept() {
    let hub = open_hub(|_| {});
    let old = moved_to(&hub.url);
    let state = crate::sh::test_dir("hub-moved");
    let joined = super::join(&state, &old, &hub.hub.mint_join_code().unwrap(), "mac", now()).unwrap();
    assert_eq!(joined.url, hub.url, "a join posted through a 308 reached the hub with its body, and the new address is kept");

    let mut browser = member(&hub);
    let state = connected_at(&hub, &old, &mut browser);
    assert_eq!(identity::load(&state).unwrap().unwrap().url, hub.url, "connecting and pinning through the old port moves it");

    pointed_at(&state, &old);
    let rev = relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap().expect("a board PUT through a 308 is stored");
    assert_eq!(hub.boards(&mut browser)[0].0, rev);
    assert_eq!(identity::load(&state).unwrap().unwrap().url, hub.url);
}

#[test]
fn doctor_says_whether_the_hub_is_joined_reachable_and_posting() {
    let hub = open_hub(|_| {});
    let alone = crate::sh::test_dir("hub-doctor-none");
    let check = super::doctor(&alone, Some("mac"), now(), |_| Ok(()));
    assert_eq!(check.level, crate::doctor::Level::Ok);
    assert!(check.fact.contains("not connected") && check.fact.contains("kebbie hub connect"), "{}", check.fact);
    let lonely = super::doctor(&hub.joined("mac"), Some("mac"), now(), super::reach);
    assert_eq!(lonely.level, crate::doctor::Level::Warn);
    assert!(lonely.fact.contains("no account key") && lonely.fact.contains("no browser can read this machine's board"), "{}", lonely.fact);
    let mut browser = member(&hub);
    let state = connected_state(&hub, &mut browser);
    relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap();
    let posting = super::doctor(&state, Some("mac"), now(), super::reach);
    assert_eq!(posting.level, crate::doctor::Level::Ok, "{}", posting.fact);
    assert!(posting.fact.contains(&hub.url) && posting.fact.contains("account key pinned") && posting.fact.contains("board rev ") && posting.fact.contains(" posted "), "{}", posting.fact);
    let down = super::doctor(&state, Some("mac"), now(), |_| Err("connection refused".to_string()));
    assert!(down.level == crate::doctor::Level::Warn && down.fact.contains("not reachable: connection refused"), "{}", down.fact);
    assert!(super::config_line(Some(&state)).starts_with(&format!("hub.connected = {:?}  (kebbie hub connect)", hub.url)));
    assert!(super::config_line(Some(&alone)).starts_with("hub.connected = none"));
}

#[test]
fn hub_url_in_config_toml_names_the_hub_to_connect_to_and_defaults_to_the_kebbie_hub() {
    let set = crate::config::parse_machine("[hub]\nurl = \"https://hub.example.com/\"\n", "config.toml").unwrap();
    assert_eq!(set.hub_url.as_deref(), Some("https://hub.example.com"));
    let unset = crate::config::settings(&crate::config::Worktree::default(), &crate::config::Machine::default()).hub_url;
    assert_eq!((unset.value.as_str(), unset.layer), (crate::config::DEFAULT_HUB_URL, crate::config::Layer::Default));
    assert!(crate::config::parse_machine("[hub]\nurl = \"http://hub.example.com\"\n", "config.toml").unwrap_err().contains("hub.url"));
    assert!(crate::config::parse_repo("[hub]\nurl = \"https://hub.example.com\"\n", "app.toml").unwrap_err().contains("set [hub] url in"));
    let (_, unknown) = crate::config::check_machine(&{ let file = crate::sh::test_dir("hub-url-keys").join("config.toml"); std::fs::write(&file, "[hub]\nurl = \"https://hub.example.com\"\nadress = \"x\"\n").unwrap(); file }).unwrap();
    assert!(unknown.len() == 1 && unknown[0].contains("hub.adress: unknown key; known keys here: url"), "{unknown:?}");
}

#[test]
fn the_keeper_posts_the_board_and_an_open_board_does_when_it_is_off() {
    assert!(crate::keeper::routines().iter().any(|routine| routine.name == "hub" && routine.every == super::EVERY));
    let threads = include_str!("../board/threads.rs");
    let runner = &threads[threads.find("pub(super) fn spawn_routine_runner(").unwrap()..];
    let runner = &runner[..runner.find("\n}\n").unwrap()];
    assert!(runner.contains("crate::keeper::Owner::Board") && runner.contains("crate::hub::tick()"), "with the keeper off the board posts it");
}

#[test]
fn a_redirect_to_another_host_is_refused_until_the_user_moves_the_hub() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let state = connected_state(&hub, &mut browser);
    let port = hub.url.rsplit(':').next().unwrap();
    let elsewhere = moved_to(&format!("http://localhost:{port}"));
    let mut kept = identity::load(&state).unwrap().unwrap();
    kept.url = elsewhere.clone();
    identity::save(&state, &kept).unwrap();
    let refused = relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap_err();
    assert!(refused.contains(&format!("the hub points to http://localhost:{port}")) && refused.contains("kebbie hub move"), "{refused}");
    assert_eq!(identity::load(&state).unwrap().unwrap().url, elsewhere, "the stored address stays");
    assert!(hub.boards(&mut browser).is_empty(), "nothing reached the hub");
    let fresh = crate::sh::test_dir("hub-elsewhere");
    let join = super::join(&fresh, &elsewhere, &hub.hub.mint_join_code().unwrap(), "mac", now()).err().unwrap();
    assert!(join.contains("the hub points to"), "the join code never follows to another host: {join}");

    assert!(super::move_to(&state, "http://127.0.0.1:9").unwrap_err().contains("does not answer as a kebbie hub"));
    assert!(super::move_to(&state, "http://hub.example.com").unwrap_err().contains("takes https"));
    assert_eq!(identity::load(&state).unwrap().unwrap().url, elsewhere);
    let moved = super::move_to(&state, &hub.url).unwrap();
    assert!(moved.contains(&format!("to {}", hub.url)), "{moved}");
    assert_eq!(identity::load(&state).unwrap().unwrap().url, hub.url);
    let rev = relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap().unwrap();
    assert_eq!(hub.boards(&mut browser)[0].0, rev);
}

fn reported_bug(state: &Path) -> String {
    let issue = crate::issues::Issue {
        id: "a1b2c3".to_string(), title: "push in greenhouse hid git's error".to_string(), kind: "bug".to_string(), status: "open".to_string(),
        version: "0.26.0".to_string(), commit: "e58e84a".to_string(), filed: "2026-10-23".to_string(), by: "lead".to_string(),
        body: "Ran kebbie push in /Users/carl/work/greenhouse; origin git@github.com:acme/greenhouse.git refused and kebbie said nothing.".to_string(),
        ..Default::default()
    };
    crate::issues::save(&crate::issues::dir(state), &issue).unwrap();
    issue.id
}

#[test]
fn a_bug_the_user_sends_reaches_the_maintainers_kebbie_as_one_issue_once_its_key_is_allowed() {
    use crate::hub::{inbox, report};
    use crate::permit::Caller;
    let hub = TestHub::start_with(|config| config.report_difficulty = 8);
    let user = crate::sh::test_dir("report-user");
    let id = reported_bug(&user);
    let scrub = report::Scrub::new(Some("/Users/carl".to_string()), vec!["greenhouse".to_string()]);
    let read = report::payload(&crate::issues::find(&crate::issues::dir(&user), &id).unwrap(), &scrub).digest();
    assert!(report::send_in(&user, &Caller::Lead, &id, &hub.url, &scrub, &read, now()).unwrap_err().contains("the user's call"));
    let sent = report::send_in(&user, &Caller::User, &id, &hub.url, &scrub, &read, now()).unwrap();
    let filed = crate::issues::find(&crate::issues::dir(&user), &id).unwrap();
    let number = filed.report.clone().expect("the issue keeps the report's id");
    assert!(sent.contains(&format!("as report {number}")), "{sent}");
    assert!(report::offers(&user, true).is_empty(), "a sent report is offered no more");
    let again = report::send_in(&user, &Caller::User, &id, &hub.url, &scrub, &read, now()).unwrap_err();
    assert!(again.contains(&format!("already, as report {number}")), "{again}");

    let maintainer = crate::sh::test_dir("report-maintainer");
    let token = hub.hub.mint_admin_token("maintainer", kebbie_hub::store::AdminScope::Reports).unwrap().unwrap();
    let settings = crate::config::Report { inbox_token: Some(token.clone()), ..Default::default() };
    assert!(inbox::pull(&maintainer, &hub.url, &token, now()).unwrap().is_empty(), "an unknown install's report waits in quarantine");
    let check = inbox::doctor(&maintainer, &settings, now()).unwrap();
    assert!(check.level == crate::doctor::Level::Warn && check.fact.contains("1 reports wait in quarantine"), "{}", check.fact);

    let key = report::install_key(&user).unwrap().public();
    hub.hub.store().set_key_status(&key, kebbie_hub::store::KeyStatus::Allowed, now()).unwrap();
    let notes = inbox::pull(&maintainer, &hub.url, &token, now()).unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(crate::messages::load_in(&maintainer).iter().map(|line| (line.kind.as_deref(), line.text.as_str())).collect::<Vec<_>>(), [(Some(inbox::FILED_KIND), "New bug report: push in <repo> hid git's error")]);
    let arrived = crate::issues::load(&crate::issues::dir(&maintainer));
    assert_eq!(arrived.len(), 1);
    let issue = &arrived[0];
    assert_eq!((issue.title.as_str(), issue.kind.as_str(), issue.source.as_deref(), issue.version.as_str()), ("push in <repo> hid git's error", "bug", Some(format!("report {number}").as_str()), "0.26.0 (e58e84a)"));
    assert!(issue.body.starts_with("Ran kebbie push in ~/work/<repo>; origin <remote> refused") && issue.body.ends_with(&format!("OS: {}", crate::issues::os())), "{}", issue.body);
    for private in ["/Users/carl", "greenhouse", "acme"] {
        assert!(!issue.body.contains(private) && !issue.title.contains(private), "{private} reached the maintainer: {issue:?}");
    }
    assert!(report::offers(&maintainer, true).is_empty(), "a report from elsewhere is never offered back");
    assert!(hub.hub.store().reports(kebbie_hub::store::KeyStatus::Allowed, 0, true, 10).unwrap().is_empty(), "the routine acked it");
    assert_eq!(inbox::doctor(&maintainer, &settings, now()).unwrap().level, crate::doctor::Level::Ok);

    let database = rusqlite::Connection::open(hub.hub.config().database()).unwrap();
    database.execute("UPDATE reports SET acked_at = NULL", []).unwrap();
    assert!(inbox::pull(&maintainer, &hub.url, &token, now()).unwrap().is_empty(), "a report whose ack was lost is acked again, not filed again");
    assert_eq!(crate::issues::load(&crate::issues::dir(&maintainer)).len(), 1);
    assert!(hub.hub.store().reports(kebbie_hub::store::KeyStatus::Allowed, 0, true, 10).unwrap().is_empty());

    database.execute("UPDATE reports SET acked_at = NULL", []).unwrap();
    std::fs::remove_file(maintainer.join("report/inbox.json")).unwrap();
    inbox::pull(&maintainer, &hub.url, &token, now()).unwrap();
    assert_eq!(crate::issues::load(&crate::issues::dir(&maintainer)).len(), 1, "a crash before the ledger was written files the report no second time");
}

#[test]
fn the_reports_routine_runs_every_ten_minutes_and_doctor_says_nothing_without_a_token() {
    assert!(crate::keeper::routines().iter().any(|routine| routine.name == "reports" && routine.every == 600));
    let state = crate::sh::test_dir("report-no-token");
    assert!(crate::hub::inbox::doctor(&state, &crate::config::Report::default(), now()).is_none());
}

#[test]
fn a_hub_that_asks_for_more_work_than_kebbie_does_gets_nothing() {
    let hub = TestHub::start_with(|config| config.report_difficulty = crate::hub::report::MAX_WORK + 1);
    let user = crate::sh::test_dir("report-greedy");
    let id = reported_bug(&user);
    let scrub = crate::hub::report::Scrub::default();
    let read = crate::hub::report::payload(&crate::issues::find(&crate::issues::dir(&user), &id).unwrap(), &scrub).digest();
    let refused = crate::hub::report::send_in(&user, &crate::permit::Caller::User, &id, &hub.url, &scrub, &read, now()).unwrap_err();
    assert!(refused.contains("more than kebbie does") && refused.contains("nothing was sent"), "{refused}");
    assert!(crate::issues::find(&crate::issues::dir(&user), &id).unwrap().report.is_none());
}

fn join_with(hub: &TestHub, config: &Path, given: Option<&str>, ask: Option<&super::AskName>) -> (PathBuf, crate::sh::Result<String>) {
    let state = crate::sh::test_dir("hub-named");
    let shown = super::join_named(&state, config, &hub.url, &hub.hub.mint_join_code().unwrap(), given, ask, now());
    (state, shown)
}

#[test]
fn a_join_goes_by_the_configured_machine_name() {
    let hub = TestHub::start();
    let config = crate::sh::test_dir("hub-named-config").join("config.toml");
    let before = "# mine\n[machine]\nname = \"devbox\"\n";
    std::fs::write(&config, before).unwrap();
    let asked: &super::AskName = &|_| panic!("a configured name is never asked for");
    let (state, shown) = join_with(&hub, &config, None, Some(asked));
    assert!(shown.unwrap().contains("as devbox"));
    assert_eq!(identity::load(&state).unwrap().unwrap().name, "devbox");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
}

#[test]
fn a_join_name_given_with_name_is_written_to_config_toml_and_nothing_else_changes() {
    let hub = TestHub::start();
    let dir = crate::sh::test_dir("hub-named-given");
    let config = dir.join("config.toml");
    let rest = "# kebbie\n[keeper]\nenabled = true   # keep it\n\n[machine]\n# who am I\n";
    let after = "\n[keys]\ninbox = \"I\"";
    std::fs::write(&config, format!("{rest}name = \"old\"  # first{after}")).unwrap();
    let (state, shown) = join_with(&hub, &config, Some("studio"), None);
    let shown = shown.unwrap();
    assert!(shown.contains(&format!("[machine] name = \"studio\" in {}", config.display())), "{shown}");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), format!("{rest}name = \"studio\"  # first{after}"));
    assert_eq!(identity::load(&state).unwrap().unwrap().name, "studio");
    let bare = dir.join("bare.toml");
    std::fs::write(&bare, "[keeper]\nenabled = false").unwrap();
    join_with(&hub, &bare, None, Some(&|host: &str| Ok(format!("  {host}-2\n")))).1.unwrap();
    assert_eq!(std::fs::read_to_string(&bare).unwrap(), format!("[keeper]\nenabled = false\n\n[machine]\nname = \"{}-2\"\n", crate::config::short_hostname()));
    let refused = join_with(&hub, &dir.join("fresh.toml"), Some("localhost"), None);
    assert!(refused.1.unwrap_err().contains("\"localhost\" names every machine"));
    assert!(identity::load(&refused.0).unwrap().is_none() && !dir.join("fresh.toml").exists(), "a name config.toml refuses joins nothing");
}

#[test]
fn a_join_with_no_name_and_no_terminal_goes_by_the_host_and_says_how_to_name_it() {
    let hub = TestHub::start();
    let config = crate::sh::test_dir("hub-named-host").join("config.toml");
    let (state, shown) = join_with(&hub, &config, None, None);
    let shown = shown.unwrap();
    assert_eq!(identity::load(&state).unwrap().unwrap().name, crate::config::short_hostname());
    assert!(shown.contains("set [machine] name in") && shown.contains("after the next post"), "{shown}");
    assert!(!config.exists(), "the host's name is not written for the user");
}

#[test]
fn a_join_never_writes_the_machine_name_through_a_linked_config_toml() {
    let hub = TestHub::start();
    let dir = crate::sh::test_dir("hub-named-link");
    let shared = dir.join("dotfiles.toml");
    std::fs::write(&shared, "[keeper]\nenabled = true\n").unwrap();
    let config = dir.join("config.toml");
    std::os::unix::fs::symlink(&shared, &config).unwrap();
    let (state, shown) = join_with(&hub, &config, Some("studio"), None);
    let shown = shown.unwrap();
    assert!(shown.contains(&format!("{} -> {}", config.display(), shared.display())) && shown.contains("is a link") && shown.contains("[machine] name = \"studio\""), "{shown}");
    assert_eq!(std::fs::read_to_string(&shared).unwrap(), "[keeper]\nenabled = true\n");
    assert_eq!(identity::load(&state).unwrap().unwrap().name, "studio");
}

#[test]
fn a_name_set_after_joining_renames_the_machine_on_the_hub_and_a_failure_shows_until_it_works() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let state = connected_state(&hub, &mut browser);
    super::sync_name(&state, Some("studio")).unwrap();
    assert_eq!(identity::load(&state).unwrap().unwrap().name, "studio");
    relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap();
    let names: Vec<String> = hub.runtime.block_on(browser.boards()).unwrap().into_iter().map(|entry| entry.name).collect();
    assert_eq!(names, ["studio"]);
    assert!(!super::status_text(&state, Some("studio"), now()).unwrap().contains("name:"));

    let mut moved = identity::load(&state).unwrap().unwrap();
    let url = moved.url.clone();
    moved.url = "http://127.0.0.1:9".to_string();
    identity::save(&state, &moved).unwrap();
    super::sync_name(&state, Some("loft")).unwrap();
    let failed = identity::load(&state).unwrap().unwrap();
    assert_eq!(failed.name, "studio");
    let shown = super::status_text(&state, Some("loft"), now()).unwrap();
    assert!(shown.contains("the hub still shows this machine as \"studio\"; renaming it to \"loft\" failed: "), "{shown}");
    let warned = super::doctor(&state, Some("loft"), now(), |_| Ok(()));
    assert_eq!(warned.level, crate::doctor::Level::Warn, "{}", warned.fact);
    assert!(warned.fact.contains("renaming it to \"loft\" failed"), "{}", warned.fact);

    moved.url = url;
    identity::save(&state, &moved).unwrap();
    super::sync_name(&state, Some("loft")).unwrap();
    let renamed = identity::load(&state).unwrap().unwrap();
    assert_eq!((renamed.name.as_str(), renamed.rename_error.as_deref()), ("loft", None));
}

#[test]
fn an_unset_machine_name_never_renames_the_machine_even_when_the_host_name_drifts() {
    let hub = TestHub::start();
    let state = hub.joined("mac");
    let before = std::fs::read_to_string(state.join("hub/identity.json")).unwrap();
    let mut moved = identity::load(&state).unwrap().unwrap();
    moved.url = "http://127.0.0.1:9".to_string();
    identity::save(&state, &moved).unwrap();
    super::sync_name(&state, None).unwrap();
    let after = identity::load(&state).unwrap().unwrap();
    assert_eq!((after.name.as_str(), after.rename_error.as_deref()), ("mac", None), "no call goes out, so none fails");
    assert!(!super::status_text(&state, None, now()).unwrap().contains("name:"));
    assert!(!super::doctor(&state, None, now(), |_| Ok(())).fact.contains("rename"));
    assert!(before.contains("\"name\": \"mac\""));
}

#[test]
fn an_older_hub_or_one_that_keeps_another_name_is_said_once() {
    let hub = TestHub::start();
    let state = hub.joined("mac");
    let (old, calls) = answering("404 Not Found", "{\"error\":\"not-found\"}");
    pointed_at(&state, &old);
    super::sync_name(&state, Some("studio")).unwrap();
    let file = state.join("hub/identity.json");
    let marked = std::fs::read_to_string(&file).unwrap() + "\n\n";
    std::fs::write(&file, &marked).unwrap();
    super::sync_name(&state, Some("studio")).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), marked, "the same error is not written again");
    let failed = identity::load(&state).unwrap().unwrap();
    assert_eq!((failed.name.as_str(), failed.rename_error.as_deref()), ("mac", Some("this hub cannot rename machines yet; update kebbie-hub")));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2, "an older hub may be updated, so it is asked again");

    let (keeps, calls) = answering("200 OK", "{\"name\":\"Studio\"}");
    pointed_at(&state, &keeps);
    super::sync_name(&state, Some("studio")).unwrap();
    super::sync_name(&state, Some("studio")).unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "a name the hub changed is not sent again until config.toml changes");
    let kept = identity::load(&state).unwrap().unwrap();
    assert_eq!((kept.name.as_str(), kept.rename_stopped.as_deref()), ("Studio", Some("studio")));
    assert!(super::status_text(&state, Some("studio"), now()).unwrap().contains("the hub shows this machine as \"Studio\", not \"studio\": the hub took it as \"Studio\""));
    super::sync_name(&state, Some("loft")).unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2, "a new name is tried");
}

#[test]
fn an_account_pinned_while_the_rename_is_on_its_way_is_kept() {
    let state = crate::sh::test_dir("hub-rename-race");
    let keys = kebbie_hub_protocol::SecretKeys::generate();
    identity::save(&state, &identity::Identity::new("http://127.0.0.1:9", Id::new(kebbie_hub_protocol::Prefix::Machine).unwrap(), "mac", "khm_x".to_string(), &keys, now())).unwrap();
    let pinning = state.clone();
    let key = kebbie_hub_protocol::AccountKey::new(Id::new(kebbie_hub_protocol::Prefix::Account).unwrap(), 1, kebbie_hub_protocol::SecretKeys::generate().public()).unwrap();
    let url = serving(move |_| {
        let mut pinned = identity::load(&pinning).unwrap().unwrap();
        pinned.pin(key.clone(), now());
        identity::save(&pinning, &pinned).unwrap();
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"name\":\"studio\"}".to_string()
    });
    pointed_at(&state, &url);
    super::sync_name(&state, Some("studio")).unwrap();
    let after = identity::load(&state).unwrap().unwrap();
    assert_eq!((after.name.as_str(), after.account.is_some()), ("studio", true));
}

#[test]
fn with_text_always_a_bug_goes_out_unasked_redacted_and_each_send_is_a_line_under_messages() {
    use crate::hub::report;
    let hub = TestHub::start_with(|config| config.report_difficulty = 8);
    let user = crate::sh::test_dir("report-always");
    let id = reported_bug(&user);
    let scrub = || report::Scrub::new(Some("/Users/carl".to_string()), vec!["greenhouse".to_string()]);
    let always = crate::config::ReportText::Always;
    assert!(report::send_unasked_in(&user, always, &hub.url, &|| panic!("unarmed, nothing is scrubbed"), now()).unwrap().is_empty(), "text = always alone sends nothing");
    for caller in [crate::permit::Caller::Lead, crate::permit::Caller::Task("a@b".to_string()), crate::permit::Caller::Agent, crate::permit::Caller::Sandboxed, crate::permit::Caller::LeadSandboxed] {
        assert!(report::arm_in(&user, &caller, true, always, now()).unwrap_err().contains("is the user's call"), "{caller:?}");
    }
    assert!(!report::armed(&user) && report::send_unasked_in(&user, always, &hub.url, &scrub, now()).unwrap().is_empty(), "no Claude can arm it");
    assert!(report::send_unasked_in(&user, crate::config::ReportText::Ask, &hub.url, &scrub, now()).unwrap().is_empty());
    assert!(report::arm_in(&user, &crate::permit::Caller::User, true, always, now()).unwrap().contains("without asking"));
    let notes = report::send_unasked_in(&user, always, &hub.url, &scrub, now()).unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    let number = crate::issues::find(&crate::issues::dir(&user), &id).unwrap().report.expect("the issue keeps the report's id");
    let stored = hub.hub.store().reports(kebbie_hub::store::KeyStatus::Quarantine, 0, false, 10).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].report.title, "push in <repo> hid git's error");
    assert!(!stored[0].report.text.contains("/Users/carl") && !stored[0].report.text.contains("acme"), "{}", stored[0].report.text);
    let line = crate::messages::load_in(&user).pop().unwrap();
    assert_eq!((line.from.as_str(), line.to.as_str(), line.kind.as_deref()), ("kebbie", crate::messages::USER_LABEL, Some(report::SENT_KIND)));
    assert_eq!(line.text, report::sent_line(&number, &id, "push in <repo> hid git's error"));
    assert!(line.text.starts_with(&format!("Sent bug {id} to kebbie's maintainers as report {number}: \"push in <repo> hid git's error\"")), "{}", line.text);
    assert!(report::send_unasked_in(&user, always, &hub.url, &|| panic!("nothing waits, so nothing is scrubbed"), now()).unwrap().is_empty(), "a sent bug goes once");
    report::arm_in(&user, &crate::permit::Caller::User, false, always, now()).unwrap();
    assert!(!report::armed(&user), "kebbie issue report --ask disarms it");
}

#[test]
fn a_bug_the_user_sends_is_a_line_under_messages_too() {
    use crate::hub::report;
    let hub = TestHub::start_with(|config| config.report_difficulty = 8);
    let user = crate::sh::test_dir("report-line");
    let id = reported_bug(&user);
    let scrub = report::Scrub::default();
    let read = report::payload(&crate::issues::find(&crate::issues::dir(&user), &id).unwrap(), &scrub).digest();
    report::send_in(&user, &crate::permit::Caller::User, &id, &hub.url, &scrub, &read, now()).unwrap();
    let line = crate::messages::load_in(&user).pop().unwrap();
    assert!(line.kind.as_deref() == Some(report::SENT_KIND) && line.text.starts_with(&format!("Sent bug {id} to kebbie's maintainers as report ")), "{line:?}");
}

#[test]
fn error_signals_reach_the_hub_as_counts_signed_with_the_install_key() {
    use kebbie_hub_protocol::{Name, SignalKind};
    let hub = TestHub::start_with(|config| config.report_difficulty = 8);
    let user = crate::sh::test_dir("signals-hub");
    for _ in 0..3 {
        crate::signals::record_in(&user, &Name::new("keeper.push").unwrap(), SignalKind::InternalError);
    }
    let url = hub.url.clone();
    let notes = crate::signals::tick_in(&user, true, now(), &mut |signals| crate::signals::send(&user, &url, signals)).unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    let rows = hub.hub.store().signal_rows(0).unwrap();
    assert_eq!(rows.iter().map(|row| (row.component.as_str(), row.kind.as_str(), row.count, row.vetted)).collect::<Vec<_>>(), [("keeper.push", "internal-error", 3, false)]);
    assert_eq!((rows[0].os.as_str(), rows[0].arch.as_str(), rows[0].version.as_str()), (std::env::consts::OS, std::env::consts::ARCH, env!("CARGO_PKG_VERSION")));
    let key = crate::hub::report::install_key(&user).unwrap().public();
    hub.hub.store().set_key_status(&key, kebbie_hub::store::KeyStatus::Allowed, now()).unwrap();
    crate::signals::record_in(&user, &Name::new("keeper.push").unwrap(), SignalKind::InternalError);
    crate::signals::tick_in(&user, true, now() + crate::signals::DAY, &mut |signals| crate::signals::send(&user, &url, signals)).unwrap();
    assert!(hub.hub.store().signal_rows(0).unwrap().iter().any(|row| row.vetted && row.count == 1), "the key a report was allowed for counts its signals as vetted");
}

#[test]
fn the_board_offers_a_bug_with_text_ask_and_with_always_until_it_is_armed() {
    use crate::config::{Report, ReportText};
    let state = crate::sh::test_dir("report-board-offers");
    for (text, offers) in [(None, true), (Some(ReportText::Ask), true), (Some(ReportText::Always), true), (Some(ReportText::Never), false)] {
        assert_eq!(crate::hub::report::board_offers(&Report { text, ..Default::default() }, &state), offers, "{text:?} unarmed");
    }
    crate::hub::report::arm_in(&state, &crate::permit::Caller::User, true, ReportText::Always, now()).unwrap();
    assert!(!crate::hub::report::board_offers(&Report { text: Some(ReportText::Always), ..Default::default() }, &state), "armed, kebbie sends it without the offer");
    assert!(crate::hub::report::board_offers(&Report { text: Some(ReportText::Ask), ..Default::default() }, &state), "an arming left behind changes nothing under ask");
}

pub(crate) fn open_hub(configure: impl FnOnce(&mut Config)) -> TestHub {
    TestHub::start_with(|config| {
        config.connect_interval = 0;
        config.rate = kebbie_hub::RateLimit { burst: 10_000, per_minute: 100_000 };
        configure(config);
    })
}

pub(crate) fn member(hub: &TestHub) -> Browser {
    let mut browser = Browser::new(&hub.url, kebbie_hub::DEV_ORIGIN);
    hub.runtime.block_on(browser.sign_up_with_invite(&hub.hub.mint_invite().unwrap())).unwrap();
    browser
}

fn named_config(name: &str) -> PathBuf {
    let config = crate::sh::test_dir("hub-connect-config").join("config.toml");
    std::fs::write(&config, format!("[machine]\nname = \"{name}\"\n")).unwrap();
    config
}

fn connected(hub: &TestHub, browser: &mut Browser) -> PathBuf {
    let state = crate::sh::test_dir("hub-connected");
    let mut pending = super::connect::start(&hub.url, "mac").unwrap();
    hub.runtime.block_on(browser.approve_connect(&pending.link(), false)).unwrap();
    let answer = pending.wait(&mut |_| true).unwrap();
    assert_eq!(answer.pairing, None);
    super::connect::finish(&state, &pending, &answer, None, || Ok(()), now()).unwrap();
    state
}

pub(crate) fn connected_state(hub: &TestHub, browser: &mut Browser) -> PathBuf {
    connected_at(hub, &hub.url, browser)
}

fn connected_at(hub: &TestHub, url: &str, browser: &mut Browser) -> PathBuf {
    let state = crate::sh::test_dir("hub-connected-account");
    let mut pending = super::connect::start(url, "mac").unwrap();
    let approval = hub.runtime.block_on(browser.approve_connect(&pending.link(), true)).unwrap();
    let answer = pending.wait(&mut |_| true).unwrap();
    let machine_state = state.clone();
    let machine = std::thread::spawn(move || super::connect::finish(&machine_state, &pending, &answer, None, || Ok(()), now()));
    hub.runtime.block_on(browser.hand_off(approval, WAIT)).unwrap();
    assert_eq!(machine.join().unwrap().unwrap().remark, None);
    state
}

#[test]
fn a_machine_connects_through_the_browser_which_hands_over_the_account_key_without_digits() {
    let hub = open_hub(|_| {});
    let config = named_config("mac");
    let state = crate::sh::test_dir("hub-connect");
    let prepared = super::connect::prepare(&state, &config, None, Some(&|_: &str| panic!("a configured name is not asked for"))).unwrap();
    let mut pending = super::connect::start(&hub.url, prepared.named.name()).unwrap();
    let link = pending.link();
    assert!(link.starts_with(&format!("{}#k=", pending.started.verification_uri_complete)) && link.contains(pending.user_code()) && link.contains("&m="), "{link}");
    let mut browser = member(&hub);
    let shown = hub.runtime.block_on(browser.connect_request(pending.user_code())).unwrap();
    assert_eq!(shown.name, "mac");
    let approval = hub.runtime.block_on(browser.approve_connect(&link, true)).unwrap();
    let answer = pending.wait(&mut |_| panic!("an approved connect answers the first poll")).unwrap();
    assert!(answer.pairing.is_some(), "the token answer names the account handoff");
    let machine_state = state.clone();
    let machine = std::thread::spawn(move || super::connect::finish(&machine_state, &pending, &answer, None, || Ok(()), now()));
    let handed = hub.runtime.block_on(browser.hand_off(approval, WAIT)).unwrap();
    let finished = machine.join().unwrap().unwrap();
    assert_eq!((finished.identity.name.as_str(), finished.remark), ("mac", None), "the account can read the board, so nothing more is said");
    assert_eq!(super::connect::name_line(&config, prepared, "mac"), None, "a configured name leaves config.toml alone");
    let identity = identity::load(&state).unwrap().unwrap();
    assert_eq!((identity.url.as_str(), identity.name.as_str(), identity.machine.clone()), (hub.url.as_str(), "mac", handed));
    assert!(identity.token().starts_with("khm_"));
    assert_eq!(identity.account.map(|account| account.fingerprints), browser.account_key().map(|key| key.fingerprints()));
    let posted = relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap().unwrap();
    assert_eq!(hub.boards(&mut browser)[0].0, posted, "the browser reads the board the machine sealed to the account");
    let again = super::connect::prepare(&state, &config, None, None).err().unwrap();
    assert!(again.contains("kebbie hub leave first"), "{again}");
}

#[test]
fn a_connect_waits_while_pending_slows_down_when_asked_and_ends_on_a_denial_or_an_expired_code() {
    let hub = TestHub::start_with(|config| config.connect_interval = 5);
    let mut pending = super::connect::start(&hub.url, "mac").unwrap();
    assert!(matches!(pending.poll().unwrap(), super::connect::Polled::Pending));
    assert!(matches!(pending.poll().unwrap(), super::connect::Polled::SlowDown));
    assert_eq!(pending.interval, 10, "slow_down adds five seconds");
    let mut slept = Vec::new();
    let stopped = pending.wait(&mut |wait| {
        slept.push(wait);
        false
    }).err().unwrap();
    assert_eq!((stopped.as_str(), slept), (super::connect::STOPPED, vec![Duration::from_secs(15)]), "the wait sleeps the grown interval");

    let hub = open_hub(|_| {});
    let mut pending = super::connect::start(&hub.url, "mac").unwrap();
    let mut browser = member(&hub);
    hub.runtime.block_on(browser.deny_connect(pending.user_code())).unwrap();
    let denied = pending.wait(&mut |_| true).err().unwrap();
    assert!(denied.starts_with("Cancelled in the browser"), "{denied}");

    let hub = open_hub(|config| config.connect_ttl = 0);
    let mut pending = super::connect::start(&hub.url, "mac").unwrap();
    let expired = pending.wait(&mut |_| true).err().unwrap();
    // With a zero lifetime the hub may sweep the code before the first poll, so it can also answer that it no longer knows it.
    assert!(expired.contains("code expired") || expired.contains("no longer knows this connect"), "{expired}");
    let state = crate::sh::test_dir("hub-connect-expired");
    assert!(identity::load(&state).unwrap().is_none());
}

#[test]
fn an_own_hub_is_tested_before_it_is_saved_once_and_never_through_a_linked_config() {
    let hub = open_hub(|_| {});
    let dir = crate::sh::test_dir("hub-own");
    let config = dir.join("config.toml");
    std::fs::write(&config, "# mine\n[machine]\nname = \"mac\"\n").unwrap();
    let web = serving(|_| "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: 15\r\nconnection: close\r\n\r\n<html>hi</html>".to_string());
    let refused = super::connect::probe(&web).err().unwrap();
    assert!(refused.contains("does not answer as a kebbie hub") && refused.contains("nothing changed"), "{refused}");
    assert!(super::connect::probe("http://hub.example.com").err().unwrap().contains("takes https"));
    assert_eq!(std::fs::read_to_string(&config).unwrap(), "# mine\n[machine]\nname = \"mac\"\n", "a hub that fails the test is not saved");
    let chosen = super::connect::save_own(&config, super::connect::probe(&format!("{}/", hub.url)).unwrap());
    assert_eq!(chosen.url, hub.url);
    assert_eq!(chosen.note.as_deref(), Some(format!("[hub] url = {:?} set in {}", hub.url, config.display()).as_str()));
    let written = std::fs::read_to_string(&config).unwrap();
    assert_eq!(written, format!("# mine\n[machine]\nname = \"mac\"\n\n[hub]\nurl = {:?}\n", hub.url));
    assert_eq!(super::connect::configured_hub(&config).unwrap(), Some(hub.url.clone()), "a set [hub] url is not asked again");
    assert!(super::connect::probe("hub.invalid").err().unwrap().starts_with("https://hub.invalid does not answer"), "a bare address is tried over https");

    let shared = dir.join("dotfiles.toml");
    std::fs::write(&shared, "[keeper]\nenabled = true\n").unwrap();
    let linked = dir.join("linked.toml");
    std::os::unix::fs::symlink(&shared, &linked).unwrap();
    let chosen = super::connect::save_own(&linked, super::connect::probe(&hub.url).unwrap());
    let note = chosen.note.unwrap();
    assert!(note.contains("is a link") && note.contains(&format!("[hub] url = {:?}", hub.url)), "{note}");
    assert_eq!(std::fs::read_to_string(&shared).unwrap(), "[keeper]\nenabled = true\n");
}

#[test]
fn a_lapsed_plan_still_posts_the_board_and_status_and_doctor_say_nothing_of_it() {
    let hub = open_hub(|config| {
        config.plans = true;
        config.billing_url = Some("https://billing.example.com/start".to_string());
    });
    let mut browser = member(&hub);
    let account = hub.runtime.block_on(browser.account()).unwrap().account;
    hub.hub.store().set_plan(&account, kebbie_hub_api::PlanStatus::Active, None).unwrap();
    let state = connected_state(&hub, &mut browser);
    hub.hub.store().set_plan(&account, kebbie_hub_api::PlanStatus::Inactive, None).unwrap();
    let rev = relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap().expect("reading the board is free, so a lapsed plan posts it");
    assert_eq!(hub.boards(&mut browser)[0].0, rev);
    let relayed = relay::load(&state);
    assert_eq!((relayed.plan_inactive, relayed.error), (None, None));
    let shown = super::status_text(&state, Some("mac"), now()).unwrap();
    assert!(shown.contains("board:    rev") && !shown.contains("subscription"), "{shown}");
    let check = super::doctor(&state, Some("mac"), now(), |_| Ok(()));
    assert!(check.level == crate::doctor::Level::Ok && !check.fact.contains("subscription"), "{}", check.fact);
}

type Approve<'a> = Box<dyn FnOnce(&str) + 'a>;

struct Script<'a> {
    answers: std::collections::VecDeque<String>,
    said: String,
    keys: std::collections::VecDeque<Option<super::console::Key>>,
    copies: Vec<String>,
    on_copied: Option<Approve<'a>>,
    picks: Option<std::collections::VecDeque<ratatui::crossterm::event::KeyCode>>,
}

impl<'a> Script<'a> {
    fn new(answers: &[&str]) -> Script<'a> {
        Script { answers: answers.iter().map(|answer| answer.to_string()).collect(), said: String::new(), keys: Default::default(), copies: Vec::new(), on_copied: None, picks: None }
    }
}

impl super::console::Console for Script<'_> {
    fn write(&mut self, text: &str) -> crate::sh::Result<()> {
        self.said.push_str(text);
        Ok(())
    }

    fn colour(&self) -> super::card::Colour {
        super::card::Colour::None
    }

    fn live(&self) -> bool {
        true
    }

    fn glyphs(&self) -> super::qr::Glyphs {
        super::qr::Glyphs::Octant
    }

    fn edit(&mut self, render: &dyn Fn(&str) -> (super::card::Row, super::card::Row), start: &str) -> crate::sh::Result<Option<String>> {
        let (before, after) = render(start);
        self.said.push_str(&format!("{}{}\n", super::card::paint(&before, super::card::Colour::None), super::card::paint(&after, super::card::Colour::None)));
        Ok(self.answers.pop_front().map(|answer| if answer.is_empty() { start.to_string() } else { answer }))
    }

    fn key(&mut self, _: Duration) -> crate::sh::Result<Option<super::console::Key>> {
        if let (Some(link), Some(approve)) = (self.copies.first(), self.on_copied.take_if(|_| self.keys.is_empty())) {
            approve(link);
        }
        Ok(self.keys.pop_front().flatten())
    }

    fn arrows(&self) -> bool {
        self.picks.is_some()
    }

    fn pick(&mut self, render: &dyn Fn(usize) -> Vec<super::card::Row>, count: usize, shortcuts: &[char]) -> crate::sh::Result<Option<usize>> {
        let mut picks = self.picks.take().unwrap_or_default();
        let said = &mut self.said;
        super::console::pick_with(&mut |text| { said.push_str(text); Ok(()) }, &mut || picks.pop_front().map(|code| (code, false)).ok_or_else(|| "no more keys".to_string()), render, count, shortcuts, super::card::Colour::None)
    }

    fn copy(&mut self, text: &str) -> crate::sh::Result<super::card::Copied> {
        self.copies.push(text.to_string());
        Ok(super::card::Copied::Confirmed)
    }
}

#[test]
fn the_hub_is_chosen_first_with_k_the_answer_to_enter_and_your_own_is_tested_before_it_is_saved() {
    let hub = open_hub(|_| {});
    let config = crate::sh::test_dir("hub-choose").join("config.toml");
    let mut console = Script::new(&[""]);
    let chosen = super::connect::choose_on(&mut console, &config).unwrap().unwrap();
    assert_eq!(chosen.url, crate::config::DEFAULT_HUB_URL);
    assert_eq!(console.said, "\n  Hub\n\n  k  kebbie hub     hub.kebbie.dev · free to connect and read\n  o  your own hub   a server you run kebbie-hub on\n\n  › k\n");
    let mut console = Script::new(&["o", "http://hub.example.com", &hub.url]);
    let chosen = super::connect::choose_on(&mut console, &config).unwrap().unwrap();
    assert_eq!(chosen.url, hub.url);
    assert!(console.said.contains("\n  Address of the server that runs kebbie-hub\n  › \n  ✗ http://hub.example.com: a hub on another machine takes https"), "{}", console.said);
    assert!(console.said.ends_with("  ✓ answers as a kebbie hub   plain http, on this machine only\n"), "{}", console.said);
    assert_eq!(super::connect::configured_hub(&config).unwrap(), Some(hub.url.clone()));
    assert!(super::connect::choose_on(&mut Script::new(&[]), &config).unwrap().is_none(), "no answer stops");
}

#[test]
fn in_a_terminal_the_hub_is_picked_with_the_arrows_and_enter_and_k_or_o_choose_at_once() {
    use ratatui::crossterm::event::KeyCode;
    let hub = open_hub(|_| {});
    let config = crate::sh::test_dir("hub-pick").join("config.toml");
    let mut console = Script::new(&[]);
    console.picks = Some([KeyCode::Down, KeyCode::Up, KeyCode::Enter].into());
    assert_eq!(super::connect::choose_on(&mut console, &config).unwrap().unwrap().url, crate::config::DEFAULT_HUB_URL);
    let first = "▸ k  kebbie hub     hub.kebbie.dev · free to connect and read\n  o  your own hub   a server you run kebbie-hub on\n\n  ↑↓ choose · ⏎ connect · k kebbie hub · o your own · esc cancel\n";
    let second = "  k  kebbie hub     hub.kebbie.dev · free to connect and read\n▸ o  your own hub   a server you run kebbie-hub on\n\n  ↑↓ choose · ⏎ connect · k kebbie hub · o your own · esc cancel\n";
    assert_eq!(console.said, format!("\n  Hub\n\n{first}\x1b[4F\x1b[J{second}\x1b[4F\x1b[J{first}\x1b[4F\x1b[J{}", &first[..first.find("\n\n").unwrap() + 1]));
    let mut console = Script::new(&[&hub.url]);
    console.picks = Some([KeyCode::Char('o')].into());
    assert_eq!(super::connect::choose_on(&mut console, &config).unwrap().unwrap().url, hub.url, "o goes straight to your own hub");
    let mut console = Script::new(&[]);
    console.picks = Some([KeyCode::Esc].into());
    assert!(super::connect::choose_on(&mut console, &config).unwrap().is_none(), "esc cancels");
}

#[test]
fn connect_names_the_machine_after_the_hub_then_draws_the_card_and_ends_on_the_connected_card() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let dir = crate::sh::test_dir("hub-connect-card");
    let (state, config) = (dir.join("state"), dir.join("config.toml"));
    std::fs::write(&config, format!("[hub]\nurl = \"{}\"\n", hub.url)).unwrap();
    let host = crate::config::short_hostname();
    let mut console = Script::new(&[""]);
    console.keys = [Some(super::console::Key::Open), Some(super::console::Key::Copy), None].into();
    let (opener, opened) = (dir.join("open"), dir.join("opened"));
    crate::executable::write(&opener, format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", opened.display())).unwrap();
    let opener = opener.display().to_string();
    console.on_copied = Some(Box::new(|link: &str| {
        hub.runtime.block_on(browser.approve_connect(link, false)).unwrap();
    }));
    let run = super::connect::Run { state: &state, config: &config, given: None, hub: None, interactive: true, opener: Some(&opener), post: || Ok(()), now: now() };
    let said = super::connect::connect_on(&mut console, &run).unwrap();
    let shown = &console.said;
    assert!(shown.starts_with(&format!("\n  Name this machine\n  Your phone and your other machines show it.\n\n  › {host}        ⏎ keeps {host:?}\n")), "the name comes after the hub, with the host's kept on ⏎: {shown}");
    let address = super::host(&hub.url);
    let code = shown.split("Check the page shows  ").nth(1).and_then(|rest| rest.get(..9)).unwrap_or_else(|| panic!("no code: {shown}"));
    assert!(code.chars().nth(4) == Some('-'), "{code}");
    let short = super::card::short_link(&console.copies[0]);
    assert!(short.ends_with(&format!("/connect?code={code}#…")), "{short}");
    for line in [format!("Connect {host} to your own hub"), address.to_string(), "⧗ Waiting for you · 10 min left".to_string(), "  No browser here? Scan the code, or open".to_string(), format!("  {short}   c copies it"), format!("  {short}   ✓ copied")] {
        assert!(shown.contains(&line), "{line}: {shown}");
    }
    let offered = shown.find("⏎ open in browser · or scan with your phone").unwrap_or_else(|| panic!("the card offers ⏎: {shown}"));
    let done_at = shown.find("✓ Opened in your browser").unwrap_or_else(|| panic!("⏎ opened it: {shown}"));
    assert!(offered < done_at, "the browser opens on ⏎, not before: {shown}");
    assert_eq!(std::fs::read_to_string(&opened).unwrap(), console.copies[0], "⏎ opens the whole link");
    assert!(!shown.contains("#k="), "the secret is never drawn");
    assert!(console.copies[0].contains("#k="), "c copies the whole link");
    let code_rows = super::qr::lines(&console.copies[0], super::qr::Glyphs::Octant).unwrap();
    assert!(code_rows.iter().all(|row| shown.contains(&format!("│  {row}   "))), "the octant code sits in the card, text to its right");
    assert!(shown.contains(&format!("│  {}   Connect {host} to your own hub", code_rows[0])), "the title on the code's first row");
    assert!(shown.contains("\x1b[J"), "the card is drawn again in place when it changes");
    let done = shown.rsplit("\x1b[J").next().unwrap();
    for line in [format!("│  ✓  {host} is connected to {address}"), "│     No browser can read its board; connect it from the link:".to_string(), "│     kebbie hub leave, then kebbie hub connect".to_string()] {
        assert!(done.contains(&line), "{line}: {done}");
    }
    assert!(done.lines().nth(1).is_some_and(|line| line.starts_with('╭')) && !done.contains("Waiting"), "the connected card replaces the waiting one: {done}");
    assert!(!shown.contains("carl"), "no account name anywhere");
    assert!(said.contains("No board goes out: no browser can read this machine's board, as it holds no account key, which only connecting from the link kebbie hub connect shows hands over. Connect it from the link: kebbie hub leave, then kebbie hub connect, and open the link it shows.") && said.contains(&format!("[machine] name = {host:?}")), "{said}");
    assert_eq!(identity::load(&state).unwrap().unwrap().name, host);
}

#[test]
fn a_hub_the_setup_guide_chose_goes_straight_to_the_card_without_the_picker() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let dir = crate::sh::test_dir("hub-connect-chosen");
    let (state, config) = (dir.join("state"), dir.join("config.toml"));
    std::fs::write(&config, "[machine]\nname = \"mac\"\n").unwrap();
    let mut console = Script::new(&[]);
    console.keys = [Some(super::console::Key::Copy), None].into();
    console.on_copied = Some(Box::new(|link: &str| {
        hub.runtime.block_on(browser.approve_connect(link, false)).unwrap();
    }));
    let run = super::connect::Run { state: &state, config: &config, given: None, hub: Some(&hub.url), interactive: true, opener: None, post: || Ok(()), now: now() };
    super::connect::connect_on(&mut console, &run).unwrap();
    assert!(!console.said.contains("kebbie hub     hub.kebbie.dev") && !console.said.contains("  Hub\n") && !console.said.contains("Address of the server"), "no picker: {}", console.said);
    assert!(console.said.contains("Connect mac to"), "the card comes first: {}", console.said);
    assert_eq!(identity::load(&state).unwrap().unwrap().url, hub.url);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), "[machine]\nname = \"mac\"\n", "the guide saves an own hub itself; the connect writes no [hub] url");
}

#[test]
fn kebbie_hub_dev_set_to_anything_but_0_or_false_picks_the_dev_hub_on_localhost_8470_unless_kebbie_hub_dev_url_names_another() {
    let env = |pairs: &'static [(&'static str, &'static str)]| move |name: &str| pairs.iter().find(|(set, _)| *set == name).map(|(_, value)| value.to_string());
    for (pairs, dev) in [
        (&[][..], None),
        (&[("KEBBIE_HUB_DEV", "")][..], None),
        (&[("KEBBIE_HUB_DEV", "0")], None),
        (&[("KEBBIE_HUB_DEV", "False")], None),
        (&[("KEBBIE_HUB_DEV_URL", "http://localhost:9000")], None),
        (&[("KEBBIE_HUB_DEV", "1")], Some("http://localhost:8470")),
        (&[("KEBBIE_HUB_DEV", "yes"), ("KEBBIE_HUB_DEV_URL", "")], Some("http://localhost:8470")),
        (&[("KEBBIE_HUB_DEV", "1"), ("KEBBIE_HUB_DEV_URL", " http://127.0.0.1:9000/ ")], Some("http://127.0.0.1:9000")),
    ] {
        assert_eq!(super::dev_in(env(pairs)).as_deref(), dev, "{pairs:?}");
    }
    assert_eq!(super::dev_label(super::DEV_URL), "dev hub · localhost:8470");
}

fn real_identity(state: &Path) -> Vec<u8> {
    let machine = identity::Identity::new("https://hub.example.com", Id::new(kebbie_hub_protocol::Prefix::Machine).unwrap(), "mac", "khm_real".to_string(), &kebbie_hub_protocol::SecretKeys::generate(), 1);
    identity::save_at(&identity::dir_of(state, false), &machine).unwrap();
    std::fs::read(identity::dir_of(state, false).join("identity.json")).unwrap()
}

#[test]
fn on_the_dev_hub_connect_and_status_use_the_dev_address_and_identity_and_leave_the_real_ones_byte_for_byte() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let dir = crate::sh::test_dir("hub-dev-connect");
    let (state, config) = (dir.join("state"), dir.join("config.toml"));
    let settings = "[machine]\nname = \"mac\"\n\n[hub]\nurl = \"https://hub.example.com\"\n";
    std::fs::write(&config, settings).unwrap();
    let real = real_identity(&state);
    let said = super::on_dev_hub(&hub.url, || {
        let mut console = Script::new(&[]);
        console.keys = [Some(super::console::Key::Copy), None].into();
        console.on_copied = Some(Box::new(|link: &str| {
            hub.runtime.block_on(browser.approve_connect(link, false)).unwrap();
        }));
        let run = super::connect::Run { state: &state, config: &config, given: None, hub: None, interactive: true, opener: None, post: || Ok(()), now: now() };
        super::connect::connect_on(&mut console, &run).unwrap();
        console.said
    });
    let shown = format!("dev hub · {}", super::host(&hub.url));
    assert!(said.contains("Connect mac to dev hub") && said.contains(&shown) && said.contains(&format!("mac is connected to {shown}")) && !said.contains("hub.example.com"), "{said}");
    let dev = identity::load_at(&identity::dir_of(&state, true)).unwrap().unwrap();
    assert_eq!((dev.url.as_str(), dev.name.as_str()), (hub.url.as_str(), "mac"));
    assert_eq!(std::fs::read(state.join("hub/identity.json")).unwrap(), real, "the real identity is left byte for byte");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), settings, "[hub] url is neither read nor written");
    let status = super::on_dev_hub(&hub.url, || super::status_text(&state, Some("mac"), now()).unwrap());
    assert!(status.starts_with(&format!("{shown} (KEBBIE_HUB_DEV is set")) && status.contains(&format!("hub:      {}", hub.url)), "{status}");
    let status = super::status_text(&state, Some("mac"), now()).unwrap();
    assert!(status.starts_with("hub:      https://hub.example.com") && !status.contains("dev hub"), "without the variable the real one: {status}");
    let none = super::on_dev_hub(super::DEV_URL, || super::status_text(&crate::sh::test_dir("hub-dev-none"), None, now()).unwrap());
    assert!(none.starts_with("dev hub · localhost:8470 (KEBBIE_HUB_DEV is set") && none.contains("Not connected to a hub"), "{none}");
}

#[test]
fn the_keeper_posts_the_board_to_the_real_hub_and_the_dev_hub_each_with_its_own_identity_and_leaves_the_real_identity_as_it_was() {
    let (real_hub, dev_hub) = (open_hub(|_| {}), open_hub(|_| {}));
    let (mut real_browser, mut dev_browser) = (member(&real_hub), member(&dev_hub));
    let state = connected_state(&real_hub, &mut real_browser);
    let dev_state = connected_state(&dev_hub, &mut dev_browser);
    std::fs::rename(identity::dir_of(&dev_state, false), identity::dir_of(&state, true)).unwrap();
    let real = std::fs::read(state.join("hub/identity.json")).unwrap();
    super::tick_in(&state, Some("mac"), now(), || Ok(board_of(&[asking("refunds", "greenhouse")]))).unwrap();
    assert_eq!(real_hub.boards(&mut real_browser).len(), 1, "the real hub has its board");
    assert_eq!(dev_hub.boards(&mut dev_browser).len(), 1, "the dev hub has its board");
    assert_eq!(std::fs::read(state.join("hub/identity.json")).unwrap(), real);
    assert!(identity::dir_of(&state, true).join("relay.json").exists() && identity::dir_of(&state, false).join("relay.json").exists(), "each hub keeps its own relay record");
    assert_eq!(super::on_dev_hub(&dev_hub.url, || identity::load(&state).unwrap().unwrap().url), dev_hub.url);
    assert_eq!(identity::load(&state).unwrap().unwrap().url, real_hub.url);
}
