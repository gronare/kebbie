use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use kelpie_hub::{Config, Hub, serve};
use kelpie_hub_fakes::PairError;
use kelpie_hub_fakes::phone::PhoneState;
use kelpie_hub_protocol::{Board, Group, Id};

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
        config.anonymous_devices = true;
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

    fn pair(&self, state: &Path, accept: bool) -> (crate::sh::Result<Id>, Result<Id, PairError>, PhoneState) {
        let (code_out, code_in) = mpsc::channel::<String>();
        let (digits_out, digits_in) = mpsc::channel::<String>();
        let machine_state = state.to_path_buf();
        let machine = std::thread::spawn(move || {
            super::pair::pair(&machine_state, WAIT, |code| code_out.send(code.to_string()).unwrap(), |digits| accept && digits_in.recv_timeout(WAIT).is_ok_and(|phone| phone == digits), crate::messages::now())
        });
        let code = code_in.recv_timeout(WAIT).unwrap();
        let mut phone = PhoneState::new(&self.url);
        let phone_side = self.runtime.block_on(phone.pair(&code, WAIT, async move |digits: String| {
            let _ = digits_out.send(digits);
            true
        }));
        (machine.join().unwrap(), phone_side, phone)
    }

    fn boards(&self, phone: &PhoneState) -> Vec<(u64, Board)> {
        self.runtime.block_on(phone.boards()).unwrap().into_iter().map(|entry| (entry.rev, entry.board.unwrap())).collect()
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
    assert!(shown.contains(&hub.url) && shown.contains("mac") && shown.contains("kelpie hub add-device"), "{shown}");
    let (signing, encryption) = joined.secret().to_bytes();
    let file = std::fs::read_to_string(state.join("hub/identity.json")).unwrap();
    for secret in [joined.token().to_string(), format!("{:?}", kelpie_hub_api::Bytes32(signing)), format!("{:?}", kelpie_hub_api::Bytes32(encryption))] {
        assert!(file.contains(&secret), "the file keeps the secret it guards");
        assert!(!shown.contains(&secret), "status never prints a secret");
    }
    let again = super::join(&state, &hub.url, &hub.hub.mint_join_code().unwrap(), "mac", now()).err().unwrap();
    assert!(again.contains("kelpie hub leave first"), "{again}");
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
fn a_phone_pairs_after_the_digits_match_and_reads_the_board_the_keeper_posts() {
    let hub = TestHub::start();
    let state = hub.joined("mac");
    let (machine, phone_side, phone) = hub.pair(&state, true);
    let device = machine.unwrap();
    assert_eq!(phone_side.unwrap(), identity::load(&state).unwrap().unwrap().machine);
    let paired = identity::load(&state).unwrap().unwrap();
    assert_eq!(paired.devices.len(), 1);
    assert_eq!((&paired.devices[0].id, paired.devices[0].keys), (&device, phone.public()), "the machine keeps the keys the phone committed to");

    let mut working = task(Some("working"), Some("working"), 1);
    working.name = "search".to_string();
    let first = board_of(&[asking("refunds", "greenhouse"), working.clone()]);
    let at = now();
    let one = relay::relay(&state, &first, at).unwrap().posted.unwrap();
    let seen = hub.boards(&phone);
    assert_eq!(seen.len(), 1);
    let (rev, opened) = &seen[0];
    assert_eq!(*rev, one);
    assert_eq!(opened.items, first.items, "the phone opens exactly the board the machine sealed");
    assert_eq!(opened.items[0].question.as_ref().unwrap().text, "Keep the old endpoint for a release?");

    assert_eq!(relay::relay(&state, &first, at + 1).unwrap().posted, None, "an unchanged board is not posted again");
    assert_eq!(hub.boards(&phone)[0].0, one);

    working.git.as_mut().unwrap().ahead = 2;
    let second = board_of(&[asking("refunds", "greenhouse"), working]);
    let two = relay::relay(&state, &second, at + 2).unwrap().posted.unwrap();
    assert!(two > one);
    let (rev, opened) = &hub.boards(&phone)[0];
    assert_eq!((*rev, opened.items[1].git.as_ref().unwrap().ahead), (two, 2));

    let three = relay::relay(&state, &second, at + 2 + HEARTBEAT).unwrap().posted.expect("the heartbeat posts the same board so the phone sees the machine awake");
    assert!(three > two);
    assert_eq!(hub.boards(&phone)[0].0, three);

    std::fs::remove_file(state.join("hub/relay.json")).unwrap();
    let four = relay::relay(&state, &second, at + 3 + HEARTBEAT).unwrap().posted.unwrap();
    assert!(four > three, "a lost relay.json never rolls the revision back: {four} after {three}");
    assert_eq!(hub.boards(&phone)[0].0, four);
}

#[test]
fn digits_the_user_does_not_confirm_pair_nothing() {
    let hub = TestHub::start();
    let state = hub.joined("mac");
    let (machine, phone_side, _) = hub.pair(&state, false);
    assert!(machine.unwrap_err().contains("the digits did not match"));
    assert!(phone_side.is_err(), "the phone sees the pairing refused");
    assert!(identity::load(&state).unwrap().unwrap().devices.is_empty());
    assert_eq!(relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap(), relay::Outcome::default(), "nothing is posted with no phone to read it");
}

#[test]
fn each_moment_a_task_needs_you_is_noticed_once() {
    let hub = TestHub::start();
    let state = hub.joined("mac");
    hub.pair(&state, true).0.unwrap();
    let at = now();
    let waiting = asking("refunds", "greenhouse");
    let first = relay::relay(&state, &board_of(std::slice::from_ref(&waiting)), at).unwrap();
    assert!(first.notices.is_empty(), "what already waited when the phone paired is on the board, not pushed");

    let mut asked_again = waiting.clone();
    asked_again.question.as_mut().unwrap().id = "1761070000-4411-1-question".to_string();
    let mut ready = task(Some("idle"), Some("ready"), 2);
    ready.name = "dates".to_string();
    ready.git.as_mut().unwrap().head = Some("1".repeat(40));
    let board = board_of(&[asked_again.clone(), ready.clone()]);
    assert_eq!(board.items[1].group, Group::ReadyToPush);
    let second = relay::relay(&state, &board, at + 1).unwrap();
    assert_eq!(second.notices, ["question:refunds@greenhouse:1761070000-4411-1-question", &format!("ready:dates@web:{}", "1".repeat(40))]);
    assert!(relay::load(&state).error.is_none(), "the hub took both notices");
    assert!(relay::relay(&state, &board, at + 2).unwrap().notices.is_empty(), "the same moment is not noticed twice");

    ready.git.as_mut().unwrap().head = Some("2".repeat(40));
    let third = relay::relay(&state, &board_of(&[asked_again, ready]), at + 3).unwrap();
    assert_eq!(third.notices, [format!("ready:dates@web:{}", "2".repeat(40))], "new commits ready to push are a new moment");
}

#[test]
fn a_notice_fits_one_push_and_names_no_task_in_its_header() {
    let hub = TestHub::start();
    let state = hub.joined("mac");
    hub.pair(&state, true).0.unwrap();
    let joined = identity::load(&state).unwrap().unwrap();
    assert_eq!(relay::PUSH_ENVELOPE_BYTES, kelpie_hub::webpush::MAX_PUSH_PLAINTEXT);
    let mut long = asking("räksmörgås-🦀", "greenhouse");
    let question = long.question.as_mut().unwrap();
    question.text = "Ska vi behålla den gamla ändpunkten för 🦀 en release till? ".repeat(200);
    question.question.as_mut().unwrap().options = (0..4).map(|n| (format!("{n} ✓ åäö 🦀 ").repeat(40), "betyder".to_string())).collect();
    let pending = relay::notices(&board_of(&[long]));
    let envelope = relay::seal_notice(&joined, &pending[0], 1, now()).unwrap();
    let cut = envelope.for_recipient(&joined.devices[0].id).unwrap().to_json();
    assert!(cut.len() <= relay::PUSH_ENVELOPE_BYTES, "{} bytes", cut.len());
    assert!(cut.len() > relay::PUSH_ENVELOPE_BYTES / 2, "shortened only as far as it must: {} bytes", cut.len());
    let header = serde_json::to_string(&envelope.header).unwrap();
    assert!(!header.contains("greenhouse") && !header.contains("räksm"), "{header}");
    assert!(relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).is_ok());
    let mut asked = asking("räksmörgås-🦀", "greenhouse");
    asked.question.as_mut().unwrap().text = "🦀 åäö ".repeat(1_500);
    asked.question.as_mut().unwrap().id = "1761070000-1-0-question".to_string();
    let posted = relay::relay(&state, &board_of(&[asked]), now() + 1).unwrap();
    assert_eq!(posted.notices.len(), 1, "the hub took the shortened notice");
    assert!(relay::load(&state).error.is_none(), "{:?}", relay::load(&state).error);
}

#[test]
fn leaving_takes_the_phone_off_the_board_and_forgets_the_keys() {
    let hub = TestHub::start();
    let state = hub.joined("mac");
    let phone = hub.pair(&state, true).2;
    relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap();
    assert_eq!(hub.boards(&phone).len(), 1);
    let left = super::leave(&state).unwrap();
    assert!(left.contains("Left") && left.contains("remove-machine"), "{left}");
    assert!(hub.runtime.block_on(phone.boards()).unwrap().is_empty(), "the phone no longer gets this machine's board");
    assert!(identity::load(&state).unwrap().is_none() && !state.join("hub").exists());
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
    let hub = TestHub::start();
    let old = moved_to(&hub.url);
    let state = crate::sh::test_dir("hub-moved");
    let joined = super::join(&state, &old, &hub.hub.mint_join_code().unwrap(), "mac", now()).unwrap();
    assert_eq!(joined.url, hub.url, "a join posted through a 308 reached the hub with its body, and the new address is kept");

    let mut kept = identity::load(&state).unwrap().unwrap();
    kept.url = old.clone();
    identity::save(&state, &kept).unwrap();
    let phone = hub.pair(&state, true).2;
    assert_eq!(identity::load(&state).unwrap().unwrap().url, hub.url, "pairing through the old port moves it too");

    let mut kept = identity::load(&state).unwrap().unwrap();
    kept.url = old;
    identity::save(&state, &kept).unwrap();
    let rev = relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap().posted.expect("a board PUT through a 308 is stored");
    assert_eq!(hub.boards(&phone)[0].0, rev);
    assert_eq!(identity::load(&state).unwrap().unwrap().url, hub.url);
}

#[test]
fn doctor_says_whether_the_hub_is_joined_reachable_and_posting() {
    let hub = TestHub::start();
    let alone = crate::sh::test_dir("hub-doctor-none");
    let check = super::doctor(&alone, Some("mac"), now(), |_| Ok(()));
    assert_eq!(check.level, crate::doctor::Level::Ok);
    assert!(check.fact.contains("not connected") && check.fact.contains("kelpie hub connect"), "{}", check.fact);
    let state = hub.joined("mac");
    let lonely = super::doctor(&state, Some("mac"), now(), super::reach);
    assert_eq!(lonely.level, crate::doctor::Level::Warn);
    assert!(lonely.fact.contains("no phone paired") && lonely.fix == ["kelpie hub add-device"], "{}", lonely.fact);
    hub.pair(&state, true).0.unwrap();
    relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap();
    let posting = super::doctor(&state, Some("mac"), now(), super::reach);
    assert_eq!(posting.level, crate::doctor::Level::Ok, "{}", posting.fact);
    assert!(posting.fact.contains(&hub.url) && posting.fact.contains("1 paired") && posting.fact.contains("board rev ") && posting.fact.contains(" posted "), "{}", posting.fact);
    let down = super::doctor(&state, Some("mac"), now(), |_| Err("connection refused".to_string()));
    assert!(down.level == crate::doctor::Level::Warn && down.fact.contains("not reachable: connection refused"), "{}", down.fact);
    assert!(super::config_line(Some(&state)).starts_with(&format!("hub.connected = {:?}  (kelpie hub connect)", hub.url)));
    assert!(super::config_line(Some(&alone)).starts_with("hub.connected = none"));
}

#[test]
fn hub_url_in_config_toml_names_the_hub_to_connect_to_and_defaults_to_the_kelpie_hub() {
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
    let hub = TestHub::start();
    let state = hub.joined("mac");
    let phone = hub.pair(&state, true).2;
    let port = hub.url.rsplit(':').next().unwrap();
    let elsewhere = moved_to(&format!("http://localhost:{port}"));
    let mut kept = identity::load(&state).unwrap().unwrap();
    kept.url = elsewhere.clone();
    identity::save(&state, &kept).unwrap();
    let refused = relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap_err();
    assert!(refused.contains(&format!("the hub points to http://localhost:{port}")) && refused.contains("kelpie hub move"), "{refused}");
    assert_eq!(identity::load(&state).unwrap().unwrap().url, elsewhere, "the stored address stays");
    assert!(hub.boards(&phone).is_empty(), "nothing reached the hub");
    let fresh = crate::sh::test_dir("hub-elsewhere");
    let join = super::join(&fresh, &elsewhere, &hub.hub.mint_join_code().unwrap(), "mac", now()).err().unwrap();
    assert!(join.contains("the hub points to"), "the join code never follows to another host: {join}");

    assert!(super::move_to(&state, "http://127.0.0.1:9").unwrap_err().contains("does not answer as a kelpie hub"));
    assert!(super::move_to(&state, "http://hub.example.com").unwrap_err().contains("takes https"));
    assert_eq!(identity::load(&state).unwrap().unwrap().url, elsewhere);
    let moved = super::move_to(&state, &hub.url).unwrap();
    assert!(moved.contains(&format!("to {}", hub.url)), "{moved}");
    assert_eq!(identity::load(&state).unwrap().unwrap().url, hub.url);
    let rev = relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap().posted.unwrap();
    assert_eq!(hub.boards(&phone)[0].0, rev);
}

fn reported_bug(state: &Path) -> String {
    let issue = crate::issues::Issue {
        id: "a1b2c3".to_string(), title: "push in greenhouse hid git's error".to_string(), kind: "bug".to_string(), status: "open".to_string(),
        version: "0.26.0".to_string(), commit: "e58e84a".to_string(), filed: "2026-10-23".to_string(), by: "lead".to_string(),
        body: "Ran kelpie push in /Users/carl/work/greenhouse; origin git@github.com:acme/greenhouse.git refused and kelpie said nothing.".to_string(),
        ..Default::default()
    };
    crate::issues::save(&crate::issues::dir(state), &issue).unwrap();
    issue.id
}

#[test]
fn a_bug_the_user_sends_reaches_the_maintainers_kelpie_as_one_issue_once_its_key_is_allowed() {
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
    let token = hub.hub.mint_admin_token("maintainer", kelpie_hub::store::AdminScope::Reports).unwrap().unwrap();
    let settings = crate::config::Report { url: Some(hub.url.clone()), inbox_token: Some(token.clone()), ..Default::default() };
    assert!(inbox::pull(&maintainer, &hub.url, &token, now()).unwrap().is_empty(), "an unknown install's report waits in quarantine");
    let check = inbox::doctor(&maintainer, &settings, now()).unwrap();
    assert!(check.level == crate::doctor::Level::Warn && check.fact.contains("1 reports wait in quarantine: kelpie-hub admin reports --quarantine"), "{}", check.fact);

    let key = report::install_key(&user).unwrap().public();
    hub.hub.store().set_key_status(&key, kelpie_hub::store::KeyStatus::Allowed, now()).unwrap();
    let notes = inbox::pull(&maintainer, &hub.url, &token, now()).unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    let arrived = crate::issues::load(&crate::issues::dir(&maintainer));
    assert_eq!(arrived.len(), 1);
    let issue = &arrived[0];
    assert_eq!((issue.title.as_str(), issue.kind.as_str(), issue.source.as_deref(), issue.version.as_str()), ("push in <repo> hid git's error", "bug", Some(format!("report {number}").as_str()), "0.26.0 (e58e84a)"));
    assert!(issue.body.starts_with("Ran kelpie push in ~/work/<repo>; origin <remote> refused") && issue.body.ends_with(&format!("OS: {}", crate::issues::os())), "{}", issue.body);
    for private in ["/Users/carl", "greenhouse", "acme"] {
        assert!(!issue.body.contains(private) && !issue.title.contains(private), "{private} reached the maintainer: {issue:?}");
    }
    assert!(report::offers(&maintainer, true).is_empty(), "a report from elsewhere is never offered back");
    assert!(hub.hub.store().reports(kelpie_hub::store::KeyStatus::Allowed, 0, true, 10).unwrap().is_empty(), "the routine acked it");
    assert_eq!(inbox::doctor(&maintainer, &settings, now()).unwrap().level, crate::doctor::Level::Ok);

    let database = rusqlite::Connection::open(hub.hub.config().database()).unwrap();
    database.execute("UPDATE reports SET acked_at = NULL", []).unwrap();
    assert!(inbox::pull(&maintainer, &hub.url, &token, now()).unwrap().is_empty(), "a report whose ack was lost is acked again, not filed again");
    assert_eq!(crate::issues::load(&crate::issues::dir(&maintainer)).len(), 1);
    assert!(hub.hub.store().reports(kelpie_hub::store::KeyStatus::Allowed, 0, true, 10).unwrap().is_empty());

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
fn a_hub_that_asks_for_more_work_than_kelpie_does_gets_nothing() {
    let hub = TestHub::start_with(|config| config.report_difficulty = crate::hub::report::MAX_WORK + 1);
    let user = crate::sh::test_dir("report-greedy");
    let id = reported_bug(&user);
    let scrub = crate::hub::report::Scrub::default();
    let read = crate::hub::report::payload(&crate::issues::find(&crate::issues::dir(&user), &id).unwrap(), &scrub).digest();
    let refused = crate::hub::report::send_in(&user, &crate::permit::Caller::User, &id, &hub.url, &scrub, &read, now()).unwrap_err();
    assert!(refused.contains("more than kelpie does") && refused.contains("nothing was sent"), "{refused}");
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
    let rest = "# kelpie\n[keeper]\nenabled = true   # keep it\n\n[machine]\n# who am I\n";
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
    let hub = TestHub::start();
    let state = hub.joined("mac");
    let phone = hub.pair(&state, true).2;
    super::sync_name(&state, Some("studio")).unwrap();
    assert_eq!(identity::load(&state).unwrap().unwrap().name, "studio");
    relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap();
    let names: Vec<String> = hub.runtime.block_on(phone.boards()).unwrap().into_iter().map(|entry| entry.name).collect();
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
fn an_older_hub_or_one_that_keeps_another_name_is_said_once_and_a_paired_phone_is_kept() {
    let hub = TestHub::start();
    let state = hub.joined("mac");
    hub.pair(&state, true).0.unwrap();
    let (old, calls) = answering("404 Not Found", "{\"error\":\"not-found\"}");
    pointed_at(&state, &old);
    super::sync_name(&state, Some("studio")).unwrap();
    let file = state.join("hub/identity.json");
    let marked = std::fs::read_to_string(&file).unwrap() + "\n\n";
    std::fs::write(&file, &marked).unwrap();
    super::sync_name(&state, Some("studio")).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), marked, "the same error is not written again");
    let failed = identity::load(&state).unwrap().unwrap();
    assert_eq!((failed.name.as_str(), failed.rename_error.as_deref(), failed.devices.len()), ("mac", Some("this hub cannot rename machines yet; update kelpie-hub"), 1));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2, "an older hub may be updated, so it is asked again");

    let (keeps, calls) = answering("200 OK", "{\"name\":\"Studio\"}");
    pointed_at(&state, &keeps);
    super::sync_name(&state, Some("studio")).unwrap();
    super::sync_name(&state, Some("studio")).unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "a name the hub changed is not sent again until config.toml changes");
    let kept = identity::load(&state).unwrap().unwrap();
    assert_eq!((kept.name.as_str(), kept.rename_stopped.as_deref(), kept.devices.len()), ("Studio", Some("studio"), 1));
    assert!(super::status_text(&state, Some("studio"), now()).unwrap().contains("the hub shows this machine as \"Studio\", not \"studio\": the hub took it as \"Studio\""));
    super::sync_name(&state, Some("loft")).unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2, "a new name is tried");
}

#[test]
fn a_phone_paired_while_the_rename_is_on_its_way_is_kept() {
    let state = crate::sh::test_dir("hub-rename-race");
    let keys = kelpie_hub_protocol::SecretKeys::generate();
    identity::save(&state, &identity::Identity::new("http://127.0.0.1:9", Id::new(kelpie_hub_protocol::Prefix::Machine).unwrap(), "mac", "khm_x".to_string(), &keys, now()).unwrap()).unwrap();
    let pairing = state.clone();
    let url = serving(move |_| {
        let mut paired = identity::load(&pairing).unwrap().unwrap();
        paired.admit(identity::Device { id: Id::new(kelpie_hub_protocol::Prefix::Device).unwrap(), keys: kelpie_hub_protocol::SecretKeys::generate().public(), paired_at: now() });
        identity::save(&pairing, &paired).unwrap();
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"name\":\"studio\"}".to_string()
    });
    pointed_at(&state, &url);
    super::sync_name(&state, Some("studio")).unwrap();
    let after = identity::load(&state).unwrap().unwrap();
    assert_eq!((after.name.as_str(), after.devices.len()), ("studio", 1));
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
    let stored = hub.hub.store().reports(kelpie_hub::store::KeyStatus::Quarantine, 0, false, 10).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].report.title, "push in <repo> hid git's error");
    assert!(!stored[0].report.text.contains("/Users/carl") && !stored[0].report.text.contains("acme"), "{}", stored[0].report.text);
    let line = crate::messages::load_in(&user).pop().unwrap();
    assert_eq!((line.from.as_str(), line.to.as_str(), line.kind.as_deref()), ("kelpie", crate::messages::USER_LABEL, Some(report::SENT_KIND)));
    assert_eq!(line.text, report::sent_line(&number, &id, "push in <repo> hid git's error"));
    assert!(line.text.starts_with(&format!("Sent bug {id} to kelpie's maintainers as report {number}: \"push in <repo> hid git's error\"")), "{}", line.text);
    assert!(report::send_unasked_in(&user, always, &hub.url, &|| panic!("nothing waits, so nothing is scrubbed"), now()).unwrap().is_empty(), "a sent bug goes once");
    report::arm_in(&user, &crate::permit::Caller::User, false, always, now()).unwrap();
    assert!(!report::armed(&user), "kelpie issue report --ask disarms it");
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
    assert!(line.kind.as_deref() == Some(report::SENT_KIND) && line.text.starts_with(&format!("Sent bug {id} to kelpie's maintainers as report ")), "{line:?}");
}

#[test]
fn error_signals_reach_the_hub_as_counts_signed_with_the_install_key() {
    use kelpie_hub_protocol::{Name, SignalKind};
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
    hub.hub.store().set_key_status(&key, kelpie_hub::store::KeyStatus::Allowed, now()).unwrap();
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
    assert!(!crate::hub::report::board_offers(&Report { text: Some(ReportText::Always), ..Default::default() }, &state), "armed, kelpie sends it without the offer");
    assert!(crate::hub::report::board_offers(&Report { text: Some(ReportText::Ask), ..Default::default() }, &state), "an arming left behind changes nothing under ask");
}

pub(crate) fn open_hub(configure: impl FnOnce(&mut Config)) -> TestHub {
    TestHub::start_with(|config| {
        config.connect_interval = 0;
        config.rate = kelpie_hub::RateLimit { burst: 10_000, per_minute: 100_000 };
        configure(config);
    })
}

pub(crate) fn member(hub: &TestHub) -> kelpie_hub_fakes::browser::Browser {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let handle = format!("carl{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let mut browser = kelpie_hub_fakes::browser::Browser::new(&hub.url, kelpie_hub::DEV_ORIGIN, &handle);
    hub.runtime.block_on(browser.sign_up_with_invite(&hub.hub.mint_invite().unwrap())).unwrap();
    browser
}

fn named_config(name: &str) -> PathBuf {
    let config = crate::sh::test_dir("hub-connect-config").join("config.toml");
    std::fs::write(&config, format!("[machine]\nname = \"{name}\"\n")).unwrap();
    config
}

fn connected(hub: &TestHub, browser: &mut kelpie_hub_fakes::browser::Browser) -> PathBuf {
    let state = crate::sh::test_dir("hub-connected");
    let mut pending = super::connect::start(&hub.url, "mac").unwrap();
    hub.runtime.block_on(browser.approve_connect(&pending.link(), false)).unwrap();
    let answer = pending.wait(&mut |_| true).unwrap();
    assert_eq!(answer.pairing, None);
    super::connect::finish(&state, &pending, &answer, None, || Ok(()), now()).unwrap();
    state
}

#[test]
fn a_machine_connects_through_the_browser_which_then_shows_its_board_without_digits() {
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
    assert!(answer.pairing.is_some(), "the token answer names the browser's pairing");
    let machine_state = state.clone();
    let machine = std::thread::spawn(move || super::connect::finish(&machine_state, &pending, &answer, None, || Ok(()), now()));
    let paired = hub.runtime.block_on(browser.show_board(approval, WAIT)).unwrap();
    let said = machine.join().unwrap().unwrap();
    assert_eq!(said, format!("Connected to {} as mac; the browser that approved it shows its board.", hub.url));
    assert_eq!(super::connect::name_line(&config, prepared, "mac"), None, "a configured name leaves config.toml alone");
    let identity = identity::load(&state).unwrap().unwrap();
    assert_eq!((identity.url.as_str(), identity.name.as_str(), identity.machine.clone()), (hub.url.as_str(), "mac", paired));
    assert!(identity.token().starts_with("khm_"));
    let device = browser.device.as_ref().unwrap();
    assert_eq!(identity.devices.iter().map(|known| (known.id.clone(), known.keys)).collect::<Vec<_>>(), [(device.device.clone().unwrap(), device.public())]);
    let posted = relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap().posted.unwrap();
    let boards = hub.runtime.block_on(device.boards()).unwrap();
    assert_eq!(boards[0].rev, posted, "the browser reads the board the machine sealed to it");
    let again = super::connect::prepare(&state, &config, None, None).err().unwrap();
    assert!(again.contains("kelpie hub leave first"), "{again}");
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
    assert!(expired.contains("code expired"), "{expired}");
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
    let refused = super::connect::own_hub(&config, &web).err().unwrap();
    assert!(refused.contains("does not answer as a kelpie hub") && refused.contains("nothing changed"), "{refused}");
    assert!(super::connect::own_hub(&config, "http://hub.example.com").err().unwrap().contains("takes https"));
    assert_eq!(std::fs::read_to_string(&config).unwrap(), "# mine\n[machine]\nname = \"mac\"\n", "a hub that fails the test is not saved");
    let chosen = super::connect::choose_hub(&config, Some(&|| Ok(Some(format!("{}/", hub.url))))).unwrap();
    assert_eq!(chosen.url, hub.url);
    assert_eq!(chosen.note.as_deref(), Some(format!("[hub] url = {:?} set in {}", hub.url, config.display()).as_str()));
    let written = std::fs::read_to_string(&config).unwrap();
    assert_eq!(written, format!("# mine\n[machine]\nname = \"mac\"\n\n[hub]\nurl = {:?}\n", hub.url));
    let again = super::connect::choose_hub(&config, Some(&|| panic!("a set [hub] url is not asked again"))).unwrap();
    assert_eq!((again.url, again.note), (hub.url.clone(), None));
    assert_eq!(std::fs::read_to_string(&config).unwrap(), written, "written once");
    let kelpie = super::connect::choose_hub(&dir.join("fresh.toml"), Some(&|| Ok(None))).unwrap();
    assert_eq!(kelpie.url, crate::config::DEFAULT_HUB_URL);
    assert!(!dir.join("fresh.toml").exists(), "the kelpie hub needs no key");

    let shared = dir.join("dotfiles.toml");
    std::fs::write(&shared, "[keeper]\nenabled = true\n").unwrap();
    let linked = dir.join("linked.toml");
    std::os::unix::fs::symlink(&shared, &linked).unwrap();
    let chosen = super::connect::own_hub(&linked, &hub.url).unwrap();
    let note = chosen.note.unwrap();
    assert!(note.contains("is a link") && note.contains(&format!("[hub] url = {:?}", hub.url)), "{note}");
    assert_eq!(std::fs::read_to_string(&shared).unwrap(), "[keeper]\nenabled = true\n");
}

fn add_device(hub: &TestHub, state: &Path, browser: &mut kelpie_hub_fakes::browser::Browser, phone_shows: impl Fn(&str) -> String + Send + 'static, answer: bool) -> (crate::sh::Result<Id>, Result<Id, PairError>) {
    let (link_out, link_in) = mpsc::channel::<String>();
    let (digits_out, digits_in) = mpsc::channel::<String>();
    let machine_state = state.to_path_buf();
    let machine = std::thread::spawn(move || {
        super::pair::add_device(&machine_state, WAIT, |added| link_out.send(added.url.clone()).unwrap(), |digits| answer && digits_in.recv_timeout(WAIT).is_ok_and(|phone| phone == digits), None, now())
    });
    let link = link_in.recv_timeout(WAIT).unwrap();
    assert!(link.starts_with("http://localhost:8470/add?code="), "{link}");
    let phone = hub.runtime.block_on(browser.add_this_phone(&link, WAIT, async move |digits: String| {
        let _ = digits_out.send(phone_shows(&digits));
        true
    }));
    (machine.join().unwrap(), phone)
}

#[test]
fn a_device_added_from_the_machine_is_admitted_only_on_the_users_y_to_matching_digits() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let state = connected(&hub, &mut browser);
    let (declined, _) = add_device(&hub, &state, &mut browser, |digits| digits.to_string(), false);
    assert!(declined.unwrap_err().contains("the digits did not match"));
    let (differ, _) = add_device(&hub, &state, &mut browser, |_| "000000".to_string(), true);
    assert!(differ.unwrap_err().contains("nothing was paired"), "digits that differ admit nothing, whatever the phone tapped");
    assert!(identity::load(&state).unwrap().unwrap().devices.is_empty());
    let (added, phone) = add_device(&hub, &state, &mut browser, |digits| digits.to_string(), true);
    let device = added.unwrap();
    assert_eq!(phone.unwrap(), identity::load(&state).unwrap().unwrap().machine);
    assert_eq!(identity::load(&state).unwrap().unwrap().devices.iter().map(|known| known.id.clone()).collect::<Vec<_>>(), [device]);
}

#[test]
fn a_lapsed_plan_is_said_on_status_doctor_and_add_device_and_nothing_local_stops() {
    const BILLING: &str = "https://billing.example.com/start";
    let hub = open_hub(|config| {
        config.plans = true;
        config.billing_url = Some(BILLING.to_string());
    });
    let mut browser = member(&hub);
    let account = hub.runtime.block_on(browser.account()).unwrap().account;
    hub.hub.store().set_plan(&account, kelpie_hub_api::PlanStatus::Active, None).unwrap();
    let state = connected(&hub, &mut browser);
    let (added, _) = add_device(&hub, &state, &mut browser, |digits| digits.to_string(), true);
    added.unwrap();
    hub.hub.store().set_plan(&account, kelpie_hub_api::PlanStatus::Inactive, None).unwrap();
    let said = format!("Your kelpie hub subscription isn't active: {BILLING}");
    let failed = relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now()).unwrap_err();
    assert_eq!(failed, said);
    assert_eq!(relay::load(&state).plan_inactive.as_deref(), Some(said.as_str()));
    assert!(super::status_text(&state, Some("mac"), now()).unwrap().contains(&said));
    let check = super::doctor(&state, Some("mac"), now(), |_| Ok(()));
    assert!(check.level == crate::doctor::Level::Warn && check.fact.contains(&said) && check.fix[0].contains("keeps working"), "{}", check.fact);
    let refused = super::pair::add_device(&state, WAIT, |_| panic!("no link without a plan"), |_| true, None, now()).unwrap_err();
    assert_eq!(refused, said);
    assert_eq!(identity::load(&state).unwrap().unwrap().devices.len(), 1, "the paired device stays");
    hub.hub.store().set_plan(&account, kelpie_hub_api::PlanStatus::Active, None).unwrap();
    relay::relay(&state, &board_of(&[asking("refunds", "greenhouse")]), now() + HEARTBEAT).unwrap();
    assert_eq!(relay::load(&state).plan_inactive, None, "a post that goes through clears it");
}
