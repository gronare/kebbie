use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{self, Source};
use crate::messages::{self, Message};
use crate::repo;
use crate::evidence;
use crate::report;
use crate::status::{self, Checkout, Item};
use crate::tasks;
use crate::usage::Usage;
use crate::{lead, questions, update_check, usage, vault};
use super::detail::Detail;
use super::Ongoing;
use super::space::Process;

pub(super) struct Snapshot {
    pub(super) items: Vec<Item>,
    pub(super) checkouts: Vec<Checkout>,
    pub(super) messages: Vec<Message>,
    pub(super) questions: Vec<Message>,
    pub(super) offers: Vec<super::bug_report::Offer>,
    pub(super) comments: Vec<crate::comments::Comment>,
    pub(super) fixed: Option<crate::issue_recheck::Fixed>,
    pub(super) usage: Option<Usage>,
    pub(super) ongoing: Vec<Ongoing>,
    pub(super) lead_chat: Option<bool>,
    pub(super) restart_auto: bool,
    pub(super) lead_context: Option<f64>,
    pub(super) devices: Vec<crate::hub::devices::Request>,
}

fn load_ongoing() -> Vec<Ongoing> {
    if !config::sources().is_ok_and(|sources| sources.contains(&Source::Vault)) && !crate::vault_cli::connected() {
        return Vec::new();
    }
    let Ok(index) = vault::Index::open() else { return Vec::new() };
    let Ok(notes) = index.ongoing() else { return Vec::new() };
    let claims = index.claims();
    let machine = config::machine();
    let links = tasks::started_links();
    notes.into_iter().map(|note| {
        let (task, also) = linked_tasks(&links, &note.key);
        let claim = claims.get(&note.key).filter(|claim| claim.elsewhere(&machine)).cloned();
        let opening = index.opening(&note.key).or_else(|| index.body(&note.key).map(|body| super::note_card::opening(&body))).unwrap_or_default();
        Ongoing { key: note.key, title: note.title, area: note.area, task, also, claim, status: note.status, summary: note.summary, opening }
    }).collect()
}

// A task across repos links one note from each half, and the links come in no order, so the row names the first address and the rest follow it: the same on every refresh.
fn linked_tasks(links: &std::collections::HashMap<String, Vec<String>>, key: &str) -> (Option<String>, Vec<String>) {
    let mut tasks: Vec<String> = links.iter().filter(|(_, items)| items.iter().any(|item| item == key)).map(|(address, _)| address.clone()).collect();
    tasks.sort();
    let mut tasks = tasks.into_iter();
    (tasks.next(), tasks.collect())
}

// Reading every backlog note through the vault command takes minutes against a vault with no watch, so it runs apart from the snapshots and the board draws without it.
fn spawn_body_reader() {
    thread::spawn(|| {
        loop {
            if config::sources().is_ok_and(|sources| sources.contains(&Source::Vault)) {
                crate::backlog::fetch_vault_bodies();
            }
            thread::sleep(Duration::from_secs(3));
        }
    });
}

pub(super) fn spawn_refresher(board_pane: Option<String>) -> (Receiver<std::result::Result<Snapshot, String>>, Sender<()>) {
    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    let (refresh_tx, refresh_rx) = mpsc::channel::<()>();
    spawn_body_reader();
    thread::spawn(move || {
        let mut recovered: Option<Instant> = None;
        let mut relayed: Option<Instant> = None;
        loop {
            if crate::keeper::current_owner() == crate::keeper::Owner::Board {
                let _ = messages::flush();
                crate::typing::check();
                if relayed.is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(crate::menu::EVERY)) {
                    relayed = Some(Instant::now());
                    if let Ok(state) = crate::lead::state_dir() {
                        let _ = crate::menu::tick(&state);
                    }
                }
                if recovered.is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(30)) {
                    recovered = Some(Instant::now());
                    if let Ok(state) = crate::lead::state_dir() {
                        crate::recover::tick(&state);
                    }
                }
            }
            let snapshot = status::gather_with_checkouts().map(|(items, checkouts)| {
                let questions = questions::open_now(&messages::load()).into_iter().cloned().chain(lead::state_dir().map(|state| crate::kebbie_questions::as_messages(&state)).unwrap_or_default()).collect();
                Snapshot { checkouts, items, messages: messages::recent(200), questions, offers: super::bug_report::load_offers(), comments: crate::comments::board(), fixed: super::bug_report::load_fixed(), usage: usage::load(), ongoing: load_ongoing(), lead_chat: board_pane.as_deref().and_then(lead::chat_hidden), restart_auto: config::restart_settings().map_or(true, |(auto, _)| auto), lead_context: usage::lead_context(), devices: lead::state_dir().map(|state| crate::hub::devices::requests(&state, messages::now())).unwrap_or_default() }
            });
            if snapshot_tx.send(snapshot).is_err() {
                return;
            }
            let _ = refresh_rx.recv_timeout(Duration::from_secs(3));
        }
    });
    (snapshot_rx, refresh_tx)
}

pub(super) fn spawn_update_checker() -> (Receiver<Vec<(String, String)>>, Sender<()>) {
    let (notice_tx, notice_rx) = mpsc::channel();
    let (recheck_tx, recheck_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        loop {
            let places = update_check::Places::current();
            let keeper = crate::keeper::current_owner() == crate::keeper::Owner::Keeper;
            let notice = crate::lead::state_dir().map(|state| {
                let components = if keeper {
                    update_check::cached(&state, places.channel, &update_check::installed(&places))
                } else {
                    update_check::current(&state, messages::now(), places.channel, update_check::installed(&places), || update_check::check(&places))
                };
                update_check::notice(&components, crate::release::read_override(&state).as_ref(), places.channel)
            }).unwrap_or_default();
            if notice_tx.send(notice).is_err() {
                return;
            }
            let _ = recheck_rx.recv_timeout(Duration::from_secs(if keeper { 60 } else { 600 }));
        }
    });
    (notice_rx, recheck_tx)
}

// With the keeper on, the board only keeps one running and shows what it saw; with it off, the full board restarts sessions itself.
pub(super) fn spawn_restarter(overlay: bool) -> Receiver<(u64, Vec<String>)> {
    let (stale_tx, stale_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut memory = crate::restart::Memory::default();
        let mut ensured: Option<Instant> = None;
        let mut ran: Option<Instant> = None;
        loop {
            let stale = match crate::keeper::current_owner() {
                crate::keeper::Owner::Keeper => {
                    if ensured.is_none_or(|at| at.elapsed() >= Duration::from_secs(60)) {
                        crate::keeper::ensure();
                        ensured = Some(Instant::now());
                    }
                    Some(crate::keeper::stale())
                }
                crate::keeper::Owner::Board if !overlay && ran.is_none_or(|at| at.elapsed() >= Duration::from_secs(60)) => {
                    ran = Some(Instant::now());
                    let began = crate::messages::now();
                    crate::restart::run(crate::restart::Mode::Routine { min_idle: 0 }, &mut memory, None).ok().map(|lines| (began, crate::keeper::stale_lines(&lines)))
                }
                crate::keeper::Owner::Board => None,
            };
            if let Some(stale) = stale
                && stale_tx.send(stale).is_err()
            {
                return;
            }
            thread::sleep(Duration::from_secs(5));
        }
    });
    stale_rx
}

// With the keeper off, the full board starts due routines, rebases the tasks that follow main and starts the queued tasks whose wait is over; each one's own lock keeps a second board from doing it too.
pub(super) fn spawn_routine_runner(overlay: bool) {
    if overlay {
        return;
    }
    thread::spawn(move || {
        let mut ran: Option<Instant> = None;
        loop {
            if crate::keeper::current_owner() == crate::keeper::Owner::Board
                && let Ok(state) = crate::lead::state_dir()
            {
                crate::tasks::after_tick(&state);
                crate::rules_check::tick(&state);
                crate::review::tick(&state);
                crate::assessment::tick(&state);
                if ran.is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(crate::routines::EVERY)) {
                    ran = Some(Instant::now());
                    crate::routines::tick(&state);
                    crate::tasks::follow_tick(&state);
                    crate::tasks::landed_tick(&state);
                    let _ = crate::hub::tick();
                    let _ = crate::signals::tick(&state);
                    crate::install_signal::tick(&state);
                    let _ = crate::hub::report::send_unasked_tick(&state);
                    for note in crate::issue_recheck::tick(&state).into_iter().chain(crate::comments::tick(&state)) {
                        crate::keeper::log_line(&state, &note);
                    }
                }
            }
            thread::sleep(Duration::from_secs(crate::tasks::AFTER_EVERY));
        }
    });
}

// Off the UI thread, and gh is asked only while a watch runs.
pub(super) fn spawn_ci_watcher(refresh: Sender<()>) {
    thread::spawn(move || {
        loop {
            if let Ok(state) = crate::lead::state_dir()
                && crate::ci::active(&state)
                && !crate::ci::tick(&state).is_empty()
                && refresh.send(()).is_err()
            {
                return;
            }
            thread::sleep(crate::ci::POLL);
        }
    });
}

pub(super) type DetailRequest = (PathBuf, Option<PathBuf>);

pub(super) fn spawn_detailer() -> (Sender<DetailRequest>, Receiver<(PathBuf, Detail)>) {
    let (request_tx, request_rx) = mpsc::channel::<DetailRequest>();
    let (detail_tx, detail_rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok((path, repo)) = request_rx.recv() {
            if detail_tx.send((path.clone(), gather_detail(&path, repo.as_deref()))).is_err() {
                return;
            }
        }
    });
    (request_tx, detail_rx)
}

// git status --porcelain -z: "XY path" entries split by NUL, a rename or copy followed by its old path as an entry of its own.
fn changed_files(porcelain: &str) -> Vec<String> {
    let mut entries = porcelain.split('\0').filter(|entry| !entry.is_empty());
    let mut files = Vec::new();
    while let Some(entry) = entries.next() {
        if let Some(path) = entry.get(3..) {
            files.push(path.to_string());
        }
        if entry.starts_with(['R', 'C']) {
            entries.next();
        }
    }
    files
}

fn gather_detail(path: &Path, repo: Option<&Path>) -> Detail {
    let settings = repo.and_then(|repo| config::resolve(Some(repo)).ok()).map(|resolved| resolved.settings);
    let changed = changed_files(&crate::sh::run_untrimmed("git", &["status", "--porcelain", "-z"], Some(path)).unwrap_or_default());
    Detail {
        branch: repo::git(path, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default(),
        commits: tasks::commits(path).unwrap_or_default(),
        changed,
        card: report::card_for(path),
        tests: evidence::for_path(path),
        dev_server: settings.as_ref().is_some_and(|settings| settings.dev_server.value.is_some()),
        dev_worker: settings.as_ref().is_some_and(|settings| settings.dev_worker.value.is_some()),
        server_name: Process::Server.tab(settings.as_ref()),
        worker_name: Process::Worker.tab(settings.as_ref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo;

    #[test]
    fn a_note_two_halves_of_a_task_link_names_the_same_half_on_every_refresh_and_both_cards_find_it() {
        let note = "Projects/hub-commands.md".to_string();
        for _ in 0..20 {
            let links: std::collections::HashMap<String, Vec<String>> = [("hub-commands@kebbie-hub", &note), ("hub-commands@kebbie", &note), ("other@kebbie", &"Projects/other.md".to_string())].into_iter().map(|(address, item)| (address.to_string(), vec![item.clone()])).collect();
            assert_eq!(linked_tasks(&links, &note), (Some("hub-commands@kebbie".to_string()), vec!["hub-commands@kebbie-hub".to_string()]), "a map's order changes from one build to the next; the row does not");
        }
        assert_eq!(linked_tasks(&std::collections::HashMap::new(), &note), (None, Vec::new()));
        let detail = include_str!("detail.rs");
        let note_of = &detail[detail.find("fn note_of(").unwrap()..];
        assert!(note_of[..note_of.find("\n}\n").unwrap()].contains("|| note.also.contains(&address)"), "the other half's card shows the note too");
    }

    #[test]
    fn the_board_draws_its_first_snapshot_without_waiting_for_the_vaults_backlog_texts() {
        let source = include_str!("threads.rs");
        let refresher = &source[source.find("pub(super) fn spawn_refresher(").unwrap()..];
        let refresher = &refresher[..refresher.find("\n}\n").unwrap()];
        assert!(refresher.contains("spawn_body_reader();") && !refresher.contains("fetch_vault_bodies"), "the texts are read on a thread of their own, never before a snapshot");
        let bodies = include_str!("../backlog.rs");
        let fill = &bodies[bodies.find("pub fn fill_vault_bodies(").unwrap()..];
        let read = &fill[fill.find("Ok(body) =>").unwrap()..fill.find("Err(_) => refused.push").unwrap()];
        assert!(read.contains("cached = kept();") && read.contains("crate::state::write(&bodies_file(state)"), "each text is kept as it is read, on top of what another writer kept meanwhile");
    }

    #[test]
    fn the_changed_list_keeps_a_modified_unstaged_first_file_whole() {
        assert_eq!(changed_files(" M src/search/filters.txt\0M  b.rs\0R  new.rs\0old.rs\0?? notes/a b.md\0"), ["src/search/filters.txt", "b.rs", "new.rs", "notes/a b.md"]);
        let dir = crate::sh::test_dir("changed");
        repo::git(&dir, &["init", "--quiet"]).unwrap();
        std::fs::create_dir_all(dir.join("src/search")).unwrap();
        std::fs::write(dir.join("src/search/filters.txt"), "one").unwrap();
        repo::git(&dir, &["add", "."]).unwrap();
        repo::git(&dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "one"]).unwrap();
        std::fs::write(dir.join("src/search/filters.txt"), "two").unwrap();
        assert_eq!(gather_detail(&dir, None).changed, ["src/search/filters.txt"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
