use std::cell::RefCell;
use std::path::{Path, PathBuf};

use kebbie_hub::{Gated, Refusal};
use kebbie_hub_api::{AccountMachine, AccountMachines};
use kebbie_hub_protocol::{AccountKey, Id, Job, JobEnd, JobReport, Kind, ListedMachine, Payload, Prefix, SecretKeys};

use super::cluster::{self, Cluster};
use super::master::{self, Asked, Bases, Followed, Hub, Sent};
use super::worker::{self, Batch, Queued, Seen};
use super::*;
use crate::hub::commands::{self, Hands};
use crate::hub::devices::Policy;
use crate::hub::http::{CALL, Client};
use crate::hub::identity;
use crate::hub::tests::{TestHub, connected_named, member, open_gated_hub, open_hub};

fn machine(seed: u8) -> Id {
    Id::ulid(Prefix::Machine, 1, [seed; 10]).unwrap()
}

fn row(seed: u8, name: &str, os: Option<&str>, seen: Option<u64>) -> AccountMachine {
    AccountMachine { machine: machine(seed), name: name.to_string(), worker_os: os.map(str::to_string), last_seen: seen }
}

#[test]
fn a_repos_jobs_come_from_its_config_on_the_master_as_a_command_or_a_table_that_picks_the_worker() {
    let jobs = |text: &str| parse(&text.parse::<toml::Table>().unwrap());
    let read = jobs("[jobs]\nlint = \"swiftlint\"\n\n[jobs.build]\nrun = \"xcodebuild -scheme App build\"\non = \"macos\"\ntimeout_minutes = 90\n").unwrap();
    assert_eq!(read.get("lint"), Some(&Def { run: "swiftlint".to_string(), on: None, timeout_minutes: None }));
    assert_eq!(read.get("build"), Some(&Def { run: "xcodebuild -scheme App build".to_string(), on: Some("macos".to_string()), timeout_minutes: Some(90) }));
    assert!(jobs("[worktree]\ndev_url = \"x\"\n").unwrap().is_empty(), "a repo without jobs has none");
    for (text, said) in [("jobs = 3\n", "must be a table of jobs"), ("[jobs]\nbuild = \"  \"\n", "jobs.build: must be a non-empty string"), ("[jobs.build]\non = \"macos\"\n", "jobs.build: needs run"), ("[jobs.build]\nrun = \"x\"\nwhere = \"mac\"\n", "jobs.build.where: unknown key"), ("[jobs]\nbuild = 3\n", "must be a command, or a table"), ("[jobs.build]\nrun = \"x\"\ntimeout_minutes = 0\n", "jobs.build.timeout_minutes: must be a whole number of minutes, 1 to 1440")] {
        assert!(jobs(text).unwrap_err().contains(said), "{text}");
    }
}

#[test]
fn names_from_the_other_machine_never_leave_their_folder_and_names_for_the_hub_are_plain() {
    assert_eq!((safe("../../etc"), safe(".."), safe(""), safe("fix/login page"), safe("billing@app")), (".._.._etc".to_string(), "_".to_string(), "_".to_string(), "fix_login_page".to_string(), "billing@app".to_string()), "one folder name, whatever was sent");
    assert_eq!((token("Fix/Login page"), token("..hidden"), token("MacBook-Pro"), token("/"), token(&"a".repeat(80)).len()), ("fix-login-page".to_string(), "hidden".to_string(), "macbook-pro".to_string(), "x".to_string(), 64));
}

#[test]
fn a_worker_is_picked_by_its_system_or_its_name_and_the_one_heard_from_last_wins() {
    let me = machine(1);
    let cluster = Cluster { counter: 1, keys: Default::default(), rows: vec![row(1, "devbox", Some("linux"), Some(500)), row(2, "MacBook", Some("macos"), Some(100)), row(3, "studio", Some("macos"), Some(300)), row(4, "laptop", None, Some(900)), row(5, "old-box", Some("linux"), Some(50))] };
    let picked = |on: Option<&str>| cluster::pick(&cluster, &me, on, 640).map(|row| row.name.clone());
    assert_eq!(picked(Some("macos")).as_deref(), Some("studio"), "of two Macs, the one heard from last");
    assert_eq!(picked(Some("macbook")).as_deref(), Some("MacBook"), "by its name, in any case");
    assert_eq!(picked(Some("linux")).as_deref(), Some("old-box"), "never this machine itself");
    assert_eq!(picked(None).as_deref(), Some("studio"), "any worker, never a machine that is none");
    assert_eq!(picked(Some("windows")), None);
    assert!(master::no_worker(Some("macos")).starts_with("No machine of your hub account on macos runs jobs.") && master::no_worker(None).contains("set [worker] enabled = true"));
}

#[test]
fn a_worker_the_hub_has_not_heard_from_for_ten_minutes_is_not_picked_and_kebbie_says_so() {
    let me = machine(1);
    let cluster = Cluster { counter: 1, keys: Default::default(), rows: vec![row(2, "studio", Some("macos"), Some(1_000)), row(3, "MacBook", Some("macos"), None), row(4, "devbox", Some("linux"), Some(1_500))] };
    assert_eq!(cluster::pick(&cluster, &me, Some("macos"), 1_600).map(|row| row.name.as_str()), Some("studio"));
    assert_eq!(cluster::pick(&cluster, &me, None, 1_600).map(|row| row.name.as_str()), Some("devbox"));
    assert_eq!(cluster::pick(&cluster, &me, Some("macos"), 1_601), None, "ten minutes and a second without a word is too long");
    assert_eq!(master::no_live_worker(&cluster, &me, Some("macos"), 1_000 + 5_400), "No machine of your hub account on macos runs jobs right now. studio is a worker, but the hub last heard from it 90 minutes ago: is it on, and its keeper running?");
    assert_eq!(master::no_live_worker(&cluster, &me, Some("windows"), 1_600), master::no_worker(Some("windows")), "with no worker at all, kebbie says how to make one");
}

#[test]
fn a_list_the_account_did_not_sign_trusts_no_machine() {
    let account = AccountKey { account: Id::account([3; 16]), version: 1, keys: SecretKeys::generate().public() };
    let unsigned = AccountMachines { list: None, machines: vec![row(2, "mac", Some("macos"), Some(1))] };
    assert!(cluster::verify(&unsigned, &account).unwrap_err().contains("not signed by the account"));
}

#[test]
fn the_base_a_worker_has_is_kept_per_worker_and_repo() {
    let mut bases = Bases::default();
    assert_eq!(bases.base(&machine(2), "app"), None);
    bases.record(&machine(2), "app", "a1");
    bases.record(&machine(2), "web", "b1");
    bases.record(&machine(3), "app", "c1");
    bases.record(&machine(2), "app", "a2");
    assert_eq!((bases.base(&machine(2), "app"), bases.base(&machine(2), "web"), bases.base(&machine(3), "app"), bases.base(&machine(3), "web")), (Some("a2"), Some("b1"), Some("c1"), None));
}

#[test]
fn the_remote_a_worker_fetches_from_rides_in_the_command_without_credentials_and_only_as_a_network_url() {
    let def = Def { run: "make test".to_string(), on: None, timeout_minutes: Some(5) };
    let command = command_for(&def, Some("git@github.com:me/app.git"));
    assert_eq!(command, "# kebbie: timeout_minutes = 5\n# kebbie: fetch = git@github.com:me/app.git\nmake test", "the timeout stays the first line, so an older worker still reads it");
    assert_eq!(command_of(&command), Command { timeout: Some(5), fetch: Some("git@github.com:me/app.git".to_string()), run: "make test" });
    assert_eq!(command_of("# kebbie: fetch = /home/me/app\nmake").fetch, None, "a local path on the master means nothing on the worker");
    for (url, fetched) in [
        ("https://github.com/me/app.git", Some("https://github.com/me/app.git")),
        ("https://x-access-token:ghp_secret@github.com/me/app.git", Some("https://github.com/me/app.git")),
        ("https://ghp_secret@github.com/me/app.git", Some("https://github.com/me/app.git")),
        ("ssh://git:pw@host:22/app.git", Some("ssh://git@host:22/app.git")),
        ("ssh://host/app.git", Some("ssh://host/app.git")),
        ("me@host:repos/app.git", Some("me@host:repos/app.git")),
        ("/srv/git/app.git", None),
        ("./app", None),
        ("file:///srv/git/app.git", None),
        ("ext::sh -c touch% /tmp/x", None),
        ("ext::/tmp/x.sh", None),
        ("fd::3", None),
        ("helper::https://host/app.git", None),
        ("https://host/app.git?private_token=secret", None),
        ("https://host/app.git#secret", None),
        ("me@ho$t:app.git", None),
        ("a/b@host:app.git", None),
        ("-uhttps://host/app", None),
        ("https://host/app\ntouch pwned", None),
        ("", None),
    ] {
        assert_eq!(fetchable(url).as_deref(), fetched, "{url}");
    }
}

#[test]
fn a_bundle_goes_up_in_parts_a_blob_holds_and_comes_back_whole() {
    let bundle: Vec<u8> = (0..3 * bundle::PART + 7).map(|index| (index % 251) as u8).collect();
    let parts = bundle::split(&bundle).unwrap();
    assert_eq!(parts.iter().map(|part| part.len()).collect::<Vec<_>>(), [bundle::PART, bundle::PART, bundle::PART, 7]);
    assert_eq!(bundle::join(parts.iter().map(|part| part.to_vec()).collect()), bundle);
    assert!(bundle::split(&[]).unwrap().is_empty(), "a worker that has the commit gets no bundle");
    let too_big = vec![0u8; kebbie_hub_protocol::MAX_JOB_BUNDLE_PARTS * bundle::PART + 1];
    assert!(bundle::split(&too_big).unwrap_err().ends_with("and a job carries at most 31.9 MiB"));
    assert_eq!((bundle::mib(45 * 1024 * 1024 + 400 * 1024), bundle::mib(1024)), ("45.3 MiB".to_string(), "0.0 MiB".to_string()));
    assert_eq!(bundle::split(&too_big[1..]).unwrap().len(), kebbie_hub_protocol::MAX_JOB_BUNDLE_PARTS);
}

#[test]
fn blob_calls_keep_to_half_the_hubs_rate_limit_and_wait_out_a_busy_hub() {
    let mut pace = bundle::Pace::new(0);
    let waits: Vec<u64> = (0..33).map(|_| pace.next(0)).collect();
    assert!(waits[..30].iter().all(|wait| *wait == 0) && waits[30..] == [1_000, 2_000, 3_000], "thirty at once, then one a second: {waits:?}");
    assert_eq!(pace.next(13_000), 0, "ten seconds later ten more may go");
    let slept = RefCell::new(Vec::new());
    let sleep = |wait: std::time::Duration| slept.borrow_mut().push(wait.as_secs());
    let puts = std::cell::Cell::new(0);
    let busy_twice = |_: &str, _: &[u8]| {
        puts.set(puts.get() + 1);
        if puts.get() <= 2 { Err(crate::hub::http::HubError::Status { status: 429, code: "rate-limited".into() }) } else { Ok(()) }
    };
    let bundle = vec![7u8; 2 * bundle::PART + 1];
    let mut parts = Vec::new();
    bundle::upload(&bundle, &mut bundle::Calls::new(&|| 0, &sleep, &|| false), &busy_twice, &mut parts).unwrap();
    assert_eq!((parts.len(), puts.get(), slept.borrow().clone()), (3, 5, vec![1, 2]), "a 429 is waited out and tried again");
    puts.set(0);
    let mut parts = Vec::new();
    let refused = bundle::upload(&bundle, &mut bundle::Calls::new(&|| 0, &sleep, &|| false), &|_, _| {
        puts.set(puts.get() + 1);
        if puts.get() == 1 { Ok(()) } else { Err(crate::hub::http::HubError::Status { status: 413, code: "blob-quota".into() }) }
    }, &mut parts);
    assert!(refused.unwrap_err().contains("quota is full") && puts.get() == 2, "a refusal that would repeat is not tried again");
    assert_eq!(parts.len(), 1, "the part that went up before the refusal is known, so it is deleted");
}

#[test]
fn output_goes_to_the_master_every_two_seconds_or_once_twelve_kib_wait() {
    let mut batch = Batch::new(0);
    assert!(!batch.due(10_000), "nothing to send yet");
    batch.add("compiling\n");
    assert!(!batch.due(1_999) && batch.due(2_000));
    assert_eq!(batch.take(2_000), "compiling\n");
    batch.add(&"é".repeat(7_000));
    assert!(!batch.due(2_999) && batch.due(3_000), "a full batch goes after a second, not two");
    let first = batch.take(3_000);
    assert!(first.len() <= kebbie_hub_protocol::MAX_JOB_LOG_BYTES && first.len() > kebbie_hub_protocol::MAX_JOB_LOG_BYTES - 2 && first.chars().all(|character| character == 'é'), "cut at a character, never inside one");
    assert_eq!(batch.rest().concat().chars().count() + first.chars().count(), 7_000);
    assert!(!batch.due(63_000), "an empty batch is never due: the heartbeat is the supervisor's");
    batch.add(&"x".repeat(300 * 1024));
    let rest = batch.rest();
    assert!(rest[0].starts_with("[kebbie: ") && rest.concat().len() < 260 * 1024, "the oldest output goes once too much waits");
    assert!(rest.iter().all(|part| part.len() <= kebbie_hub_protocol::MAX_JOB_LOG_BYTES));
}

#[test]
fn output_heavy_with_control_characters_is_cut_by_its_size_as_json_and_a_final_report_always_goes() {
    let fits = |log: &str| Payload::JobReport(JobReport { job: cluster::new_id().unwrap(), seq: 7, log: log.into(), end: Some(JobEnd::Failed { reason: "é".repeat(kebbie_hub_protocol::MAX_JOB_REASON_CHARS) }) }).validate().is_ok();
    let colored = "\u{1b}[31m\u{1b}\u{7}\u{1b}\u{e}\u{1b}\u{7}\u{1b}\u{f}red\r\n".repeat(2_000);
    let mut batch = Batch::new(0);
    batch.add(&colored);
    let first = batch.take(3_000);
    let rest = batch.rest();
    assert!(fits(&first) && rest.iter().all(|part| fits(part)), "each part fits a report as the hub counts it");
    assert_eq!(format!("{first}{}", rest.concat()), colored, "and nothing is lost between them");
    let too_big = JobReport { job: cluster::new_id().unwrap(), seq: 3, log: "\u{1b}".repeat(kebbie_hub_protocol::MAX_JOB_LOG_BYTES), end: Some(JobEnd::Exited { code: 0, artifacts: Vec::new() }) };
    assert!(Payload::JobReport(too_big.clone()).validate().is_err());
    let sent = worker::sendable(too_big);
    assert!(Payload::JobReport(sent.clone()).validate().is_ok() && sent.end == Some(JobEnd::Exited { code: 0, artifacts: Vec::new() }) && sent.log.contains("did not fit a report"), "a report the hub would refuse still carries its end");
}

#[test]
fn a_job_the_inbox_hands_over_again_is_queued_once_and_jobs_wait_in_the_order_they_came() {
    let mut seen = Seen::default();
    let (from, id) = (machine(1), cluster::new_id().unwrap());
    assert!(seen.first(&from, &id, 1_000) && !seen.first(&from, &id, 2_000));
    assert!(seen.first(&machine(2), &id, 2_000), "the same id from another machine is another job");
    assert!(seen.first(&from, &id, 1_000 + 2 * 86_400), "forgotten once no hub could still hand it over");
    let dir = crate::sh::test_dir("jobs-queue");
    let job = |name: &str| Job { repo: "app".into(), name: name.into(), command: "true".into(), task: "billing".into(), commit: "a".repeat(40), base: None, bundle: Vec::new(), on: None };
    let (one, two) = (cluster::new_id().unwrap(), cluster::new_id().unwrap());
    assert!(worker::take(&dir, &from, "devbox", &one, job("build"), 10).unwrap().starts_with("jobs: queued build for app (billing) from devbox"));
    assert!(worker::take(&dir, &from, "devbox", &two, job("lint"), 11).unwrap().starts_with("jobs: queued lint"));
    assert!(worker::take(&dir, &from, "devbox", &one, job("build"), 12).unwrap().contains("came again"));
    let (file, first) = worker::next_queued(&dir).unwrap();
    assert_eq!(first.job.name, "build");
    std::fs::remove_file(file).unwrap();
    assert_eq!(worker::next_queued(&dir).unwrap().1.job.name, "lint");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_job_that_could_not_be_queued_is_queued_when_the_inbox_hands_it_over_again() {
    let dir = crate::sh::test_dir("jobs-queue-fails");
    let (from, id) = (machine(1), cluster::new_id().unwrap());
    let job = Job { repo: "app".into(), name: "build".into(), command: "true".into(), task: "billing".into(), commit: "a".repeat(40), base: None, bundle: Vec::new(), on: None };
    std::fs::create_dir_all(dir.join("jobs")).unwrap();
    std::fs::write(dir.join("jobs/queue"), "not a folder").unwrap();
    assert!(worker::take(&dir, &from, "devbox", &id, job.clone(), 10).unwrap().contains("was not queued"));
    std::fs::remove_file(dir.join("jobs/queue")).unwrap();
    assert!(worker::take(&dir, &from, "devbox", &id, job, 11).unwrap().starts_with("jobs: queued build"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_job_that_waited_in_the_queue_past_the_day_a_job_lives_is_refused_not_run() {
    let job = Job { repo: "app".into(), name: "build".into(), command: "true".into(), task: "billing".into(), commit: "a".repeat(40), base: None, bundle: Vec::new(), on: None };
    let queued = Queued { from: machine(1), from_name: "devbox".into(), id: cluster::new_id().unwrap(), job, at: 1_000, seq: 0 };
    assert!(!worker::stale(&queued, 1_000 + 86_400));
    assert!(worker::stale(&queued, 1_001 + 86_400));
}

fn sent(worker: Id) -> Sent {
    Sent { id: cluster::new_id().unwrap(), worker, worker_name: "studio".into(), repo: "app".into(), name: "build".into(), commit: "a".repeat(40), base: None, blobs: Vec::new(), at: 100, pid: std::process::id() }
}

#[test]
fn only_the_jobs_own_worker_reports_on_it_and_each_report_counts_once() {
    let state = crate::sh::test_dir("jobs-report");
    let job = sent(machine(2));
    std::fs::create_dir_all(state.join("jobs")).unwrap();
    std::fs::write(master::part(&state, &job.id, "sent.json"), serde_json::to_string(&job).unwrap()).unwrap();
    let report = |seq: u32, log: &str, end: Option<JobEnd>| JobReport { job: job.id.clone(), seq, log: log.into(), end };
    let log = || std::fs::read_to_string(master::part(&state, &job.id, "log")).unwrap_or_default();
    assert_eq!(master::report(&state, &machine(2), &report(0, "compiling\n", None), 110), None);
    let forged = master::report(&state, &machine(9), &report(1, "forged\n", Some(JobEnd::Exited { code: 0, artifacts: Vec::new() })), 111).unwrap();
    assert!(forged.starts_with("jobs: refused a report on build from") && forged.contains("the job went to studio"), "{forged}");
    assert!(!master::part(&state, &job.id, "end.json").exists() && log() == "compiling\n", "another machine's report ends nothing and prints nothing");
    assert_eq!(master::report(&state, &machine(2), &report(0, "compiling\n", None), 112), None);
    assert_eq!(log(), "compiling\n", "a report handed over again is not printed twice");
    assert_eq!(master::report(&state, &machine(2), &report(3, "linking\n", None), 113), None);
    assert_eq!(log(), "compiling\n[kebbie: 2 reports from studio were lost here, and the output they held with them]\nlinking\n", "a report that never came shows where it is missing");
    assert!(master::report(&state, &machine(2), &report(4, "", Some(JobEnd::Exited { code: 65, artifacts: Vec::new() })), 113).unwrap().contains("ended"));
    assert!(master::report(&state, &machine(2), &JobReport { job: cluster::new_id().unwrap(), seq: 0, log: String::new(), end: None }, 114).unwrap().contains("no job this machine runs has that id"));
    let mut printed = String::new();
    let followed = master::follow(&state, &job, &mut |text| printed.push_str(text), &mut || {}, &|| 120, &|| false);
    assert!(matches!(followed, Followed::Ended(JobEnd::Exited { code: 65, .. })) && printed == log());
    let _ = std::fs::remove_dir_all(state);
}

#[test]
fn a_report_numbered_the_last_there_is_is_refused_and_counting_never_wraps() {
    let state = crate::sh::test_dir("jobs-report-last");
    let job = sent(machine(2));
    std::fs::create_dir_all(state.join("jobs")).unwrap();
    std::fs::write(master::part(&state, &job.id, "sent.json"), serde_json::to_string(&job).unwrap()).unwrap();
    let said = master::report(&state, &machine(2), &JobReport { job: job.id.clone(), seq: u32::MAX, log: "late\n".into(), end: None }, 110).unwrap();
    assert!(said.contains("refused"), "{said}");
    assert_eq!(master::report(&state, &machine(2), &JobReport { job: job.id.clone(), seq: 0, log: "first\n".into(), end: None }, 111), None);
    assert_eq!(std::fs::read_to_string(master::part(&state, &job.id, "log")).unwrap(), "first\n");
    let _ = std::fs::remove_dir_all(state);
}

#[test]
fn a_worker_that_sends_nothing_for_ten_minutes_is_given_up_on_while_one_that_reports_is_waited_for() {
    let state = crate::sh::test_dir("jobs-silent");
    let job = sent(machine(2));
    std::fs::create_dir_all(state.join("jobs")).unwrap();
    std::fs::write(master::part(&state, &job.id, "sent.json"), serde_json::to_string(&job).unwrap()).unwrap();
    let clock = std::cell::Cell::new(100);
    let mut pauses = 0;
    let followed = master::follow(&state, &job, &mut |_| {}, &mut || {
        pauses += 1;
        clock.set(clock.get() + 300);
        if pauses <= 4 {
            master::report(&state, &machine(2), &JobReport { job: job.id.clone(), seq: pauses, log: String::new(), end: None }, clock.get());
        }
    }, &|| clock.get(), &|| false);
    assert!(matches!(followed, Followed::Silent));
    assert_eq!((pauses, clock.get()), (7, 2_200), "twenty minutes of reports are waited for, then ten silent ones end it");
    let _ = std::fs::remove_dir_all(state);
}

fn git(dir: &Path, args: &[&str]) -> String {
    crate::repo::git(dir, args).unwrap()
}

fn repo(root: &Path) -> PathBuf {
    let top = root.join("app");
    std::fs::create_dir_all(&top).unwrap();
    git(&top, &["init", "--quiet", "-b", "main"]);
    std::fs::write(top.join("tracked.txt"), "committed\n").unwrap();
    commit(&top, "one");
    git(&top, &["checkout", "--quiet", "-b", "Billing/Refunds"]);
    top
}

fn commit(top: &Path, message: &str) {
    git(top, &["add", "."]);
    git(top, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", message]);
}

#[test]
fn a_snapshot_holds_uncommitted_and_untracked_files_and_leaves_the_worktree_as_it_was() {
    let root = crate::sh::test_dir("jobs-snapshot");
    let top = repo(&root);
    let head = git(&top, &["rev-parse", "HEAD"]);
    assert_eq!(snapshot(&top, "clean").unwrap(), head, "a clean worktree runs as its commit");
    std::fs::write(top.join("tracked.txt"), "edited\n").unwrap();
    std::fs::write(top.join("new.txt"), "untracked\n").unwrap();
    let sha = snapshot(&top, "dirty").unwrap();
    assert!(sha != head && git(&top, &["rev-parse", "HEAD"]) == head && git(&top, &["status", "--porcelain"]).lines().count() == 2, "the branch, the index and the files are left as they were");
    assert_eq!(git(&top, &["show", &format!("{sha}:new.txt")]), "untracked");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(snapshot(&top, "again").unwrap(), sha, "an unchanged worktree is the same commit, so a worker that has it needs nothing sent");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_worker_with_no_repo_runs_a_bundle_takes_a_later_one_from_its_base_and_asks_for_all_when_it_lacks_it() {
    let root = crate::sh::test_dir("jobs-execute");
    let (top, data) = (repo(&root), root.join("worker"));
    let refname = |sha: &str| {
        git(&top, &["update-ref", &format!("{REF}/x"), sha]);
        format!("{REF}/x")
    };
    let first = git(&top, &["rev-parse", "HEAD"]);
    let full = bundle::create(&top, &refname(&first), &first, None, &root.join("one")).unwrap();
    let queued = |commit: &str, base: Option<&str>, command: &str| Queued { from: machine(1), from_name: "devbox".into(), id: cluster::new_id().unwrap(), job: Job { repo: "app".into(), name: "build".into(), command: command.into(), task: "billing".into(), commit: commit.into(), base: base.map(str::to_string), bundle: vec![kebbie_hub_protocol::BlobRef { id: Id::blob(b"x"), key: "k".repeat(43), media_type: kebbie_hub_protocol::JOB_BUNDLE_MEDIA_TYPE.into(), bytes: 1 }], on: None }, at: 1, seq: 0 };
    let run = |job: &Queued, bundle: &[u8]| {
        let mut said: Vec<(String, Option<JobEnd>)> = Vec::new();
        let bundle = bundle.to_vec();
        let end = worker::execute(job, &data, &move |_| Ok(bundle.clone()), &mut |log, end| said.push((log, end)), &|| false, &crate::typing::now_ms, worker::Limits::WORKER);
        (end, said.into_iter().map(|(log, _)| log).collect::<String>())
    };
    assert!(!data.exists(), "the worker starts with nothing: the master brings the repo");
    let (end, log) = run(&queued(&first, None, "cat tracked.txt; echo to-stderr >&2; echo $KEBBIE_JOB_TASK $KEBBIE_JOB_MASTER; exit 3"), &full);
    assert_eq!((end, log.as_str()), (JobEnd::Exited { code: 3, artifacts: Vec::new() }, "committed\nto-stderr\nbilling devbox\n"), "stderr is in the log and the job's own exit comes back");
    std::fs::write(data.join("app/billing/left-over.txt"), "from the last run").unwrap();
    std::fs::write(top.join("second.txt"), "two\n").unwrap();
    commit(&top, "two");
    let second = git(&top, &["rev-parse", "HEAD"]);
    let incremental = bundle::create(&top, &refname(&second), &second, Some(&first), &root.join("two")).unwrap();
    assert!(incremental.len() < full.len() + 200 && !incremental.is_empty());
    let (end, log) = run(&queued(&second, Some(&first), "ls"), &incremental);
    assert_eq!((end, log.as_str()), (JobEnd::Exited { code: 0, artifacts: Vec::new() }, "second.txt\ntracked.txt\n"), "the task's folder is reused and holds the new commit alone");
    assert!(bundle::create(&top, &refname(&second), &second, Some(&second), &root.join("same")).unwrap().is_empty(), "a worker that has the commit gets no bundle");
    assert_eq!(run(&queued(&second, Some(&second), "true"), &[]).0, JobEnd::Exited { code: 0, artifacts: Vec::new() });
    assert!(bundle::create(&top, &refname(&first), &first, Some(&second), &root.join("older")).unwrap().is_empty(), "nor does one that has a later commit than the job's");
    assert_eq!(run(&queued(&first, Some(&second), "ls"), &[]), (JobEnd::Exited { code: 0, artifacts: Vec::new() }, "tracked.txt\n".to_string()), "and it runs the older commit from its store");
    std::fs::remove_dir_all(data.join("app/.store")).unwrap();
    let (end, log) = run(&queued(&second, Some(&first), "touch ran"), &incremental);
    assert_eq!((end, log.as_str()), (JobEnd::NeedsFull {}, ""), "a base the store lacks asks for the whole history and runs nothing");
    assert!(!data.join("app/billing/ran").exists());
    let (end, _) = run(&queued(&second, None, "true"), b"not a bundle");
    assert!(matches!(&end, JobEnd::Failed { reason } if reason.starts_with("the worker could not take the commit")), "{end:?}");
    let failing = worker::execute(&queued(&second, None, "true"), &data, &|_| Err("the hub answered 404 not-found".to_string()), &mut |_, _| {}, &|| false, &crate::typing::now_ms, worker::Limits::WORKER);
    assert_eq!(failing, JobEnd::Failed { reason: "the worker could not fetch the bundle: the hub answered 404 not-found".into() });
    let _ = std::fs::remove_dir_all(root);
}

const ORIGIN: &str = "https://example.invalid/app.git";

// A store whose git reads ORIGIN as the bare repo at path, as a worker's git reaches the real origin.
fn store_reaching(data: &Path, path: &Path) {
    let store = data.join("app/.store");
    std::fs::create_dir_all(&store).unwrap();
    git(&store, &["init", "--quiet", "--bare"]);
    git(&store, &["config", &format!("url.{}.insteadOf", path.display()), ORIGIN]);
}

fn origin_of(top: &Path, root: &Path) -> PathBuf {
    let origin = root.join("origin.git");
    git(root, &["init", "--quiet", "--bare", &origin.to_string_lossy()]);
    git(top, &["push", "--quiet", &origin.to_string_lossy(), "HEAD:refs/heads/main"]);
    git(top, &["remote", "add", "origin", ORIGIN]);
    git(top, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    origin
}

#[test]
fn a_job_builds_on_the_newest_of_its_commits_that_origin_has() {
    let root = crate::sh::test_dir("jobs-remote-base");
    let top = repo(&root);
    let one = git(&top, &["rev-parse", "HEAD"]);
    assert_eq!(bundle::remote_base(&top, &one), None, "a repo with no origin has no remote base");
    git(&top, &["remote", "add", "origin", &root.join("elsewhere").to_string_lossy()]);
    assert_eq!(bundle::remote_base(&top, &one), None, "nor one whose origin is a path the worker cannot reach");
    git(&top, &["remote", "set-url", "origin", "https://ghp_secret@example.invalid/app.git"]);
    assert_eq!(bundle::remote_base(&top, &one), None, "nor one whose origin holds none of the job");
    git(&top, &["update-ref", "refs/remotes/origin/main", &one]);
    let remote = |base: &str| Some(bundle::Remote { url: ORIGIN.to_string(), base: base.to_string() });
    assert_eq!(bundle::remote_base(&top, &one), remote(&one), "a commit origin has is its own base, and the URL goes without its token");
    std::fs::write(top.join("two.txt"), "two\n").unwrap();
    commit(&top, "two");
    let two = git(&top, &["rev-parse", "HEAD"]);
    std::fs::write(top.join("three.txt"), "three\n").unwrap();
    commit(&top, "three");
    let three = git(&top, &["rev-parse", "HEAD"]);
    assert_eq!(bundle::remote_base(&top, &three), remote(&one));
    git(&top, &["update-ref", "refs/remotes/origin/billing", &two]);
    assert_eq!(bundle::remote_base(&top, &three), remote(&two), "the newest commit origin has on any branch");
    assert_eq!(master::sources(Some(one.clone()), remote(&two)), [master::Source::Known(one.clone()), master::Source::Remote(remote(&two).unwrap()), master::Source::Whole], "the commit the worker took last first, then origin, then the whole history");
    assert_eq!(master::sources(None, None), [master::Source::Whole]);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_worker_fetches_the_history_from_the_jobs_remote_and_asks_for_all_when_the_fetch_fails() {
    let root = crate::sh::test_dir("jobs-fetch");
    let top = repo(&root);
    let origin = origin_of(&top, &root);
    let first = git(&top, &["rev-parse", "HEAD"]);
    std::fs::write(top.join("second.txt"), "two\n").unwrap();
    commit(&top, "two");
    let second = git(&top, &["rev-parse", "HEAD"]);
    git(&top, &["update-ref", &format!("{REF}/x"), &second]);
    let rest = bundle::create(&top, &format!("{REF}/x"), &second, Some(&first), &root.join("rest")).unwrap();
    let def = Def { run: "ls".to_string(), on: None, timeout_minutes: None };
    let queued = Queued { from: machine(1), from_name: "devbox".into(), id: cluster::new_id().unwrap(), job: Job { repo: "app".into(), name: "build".into(), command: command_for(&def, Some(ORIGIN)), task: "billing".into(), commit: second.clone(), base: Some(first.clone()), bundle: vec![kebbie_hub_protocol::BlobRef { id: Id::blob(b"x"), key: "k".repeat(43), media_type: kebbie_hub_protocol::JOB_BUNDLE_MEDIA_TYPE.into(), bytes: 1 }], on: None }, at: 1, seq: 0 };
    let run = |data: &Path| {
        let mut said = String::new();
        let rest = rest.clone();
        let end = worker::execute(&queued, data, &move |_| Ok(rest.clone()), &mut |log, _| said.push_str(&log), &|| false, &crate::typing::now_ms, worker::Limits::WORKER);
        (end, said)
    };
    let data = root.join("worker");
    store_reaching(&data, &origin);
    let (end, log) = run(&data);
    assert_eq!(end, JobEnd::Exited { code: 0, artifacts: Vec::new() }, "{log}");
    assert_eq!(log, format!("[kebbie: fetching the history up to {} from {ORIGIN}; only what is new comes on later jobs]\nsecond.txt\ntracked.txt\n", &first[..12]), "the bundle carries only the commit origin lacks");
    let unreachable = root.join("unreachable");
    store_reaching(&unreachable, &root.join("missing.git"));
    let (end, log) = run(&unreachable);
    assert_eq!(end, JobEnd::NeedsFull {}, "a worker that cannot fetch asks for the whole history");
    assert!(log.contains(&format!("[kebbie: the fetch from {ORIGIN} failed: ")), "{log}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_workers_fetch_runs_no_remote_helper_even_when_its_git_config_allows_one() {
    let root = crate::sh::test_dir("jobs-fetch-helper");
    let store = root.join("store");
    let marker = root.join("ran");
    std::fs::create_dir_all(&store).unwrap();
    git(&store, &["init", "--quiet", "--bare"]);
    git(&store, &["config", "protocol.ext.allow", "always"]);
    git(&store, &["config", &format!("url.ext::sh -c touch% {}.insteadOf", marker.display()), ORIGIN]);
    let stored = bundle::store(&store, &[], &"a".repeat(40), Some(&"b".repeat(40)), "refs/kebbie/keep", Some(ORIGIN), &mut |_| {}, &|| false);
    assert_eq!(stored, Ok(bundle::Stored::NeedsFull));
    assert!(!marker.exists(), "the ext:: helper the config rewrote origin to never ran");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_fetch_from_a_remote_that_never_answers_ends_when_the_worker_stops_it() {
    let root = crate::sh::test_dir("jobs-fetch-hung");
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    git(&store, &["init", "--quiet", "--bare"]);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("git://127.0.0.1:{}/app.git", listener.local_addr().unwrap().port());
    std::thread::spawn(move || {
        let held: Vec<_> = listener.incoming().take(4).collect();
        std::thread::sleep(std::time::Duration::from_secs(60));
        drop(held);
    });
    let (sent, got) = std::sync::mpsc::channel();
    let started = std::time::Instant::now();
    std::thread::spawn(move || {
        let stored = bundle::store(&store, &[], &"a".repeat(40), Some(&"b".repeat(40)), "refs/kebbie/keep", Some(&url), &mut |_| {}, &|| started.elapsed() >= std::time::Duration::from_secs(1));
        let _ = sent.send(stored);
    });
    let stored = got.recv_timeout(std::time::Duration::from_secs(15)).expect("the fetch ends once stopped");
    assert!(stored.is_err_and(|error| error.contains("was stopped")), "a stopped fetch is no reason to ask for the whole history");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_workers_ssh_never_asks_and_keeps_the_users_own_options() {
    assert_eq!(bundle::batch_ssh(None), "ssh -oBatchMode=yes");
    assert_eq!(bundle::batch_ssh(Some("ssh -i ~/.ssh/deploy")), "ssh -i ~/.ssh/deploy -oBatchMode=yes");
}

#[test]
fn a_worker_reports_on_a_job_from_the_bundles_fetch_on_and_on_the_jobs_queued_behind_it() {
    let root = crate::sh::test_dir("jobs-heartbeat");
    let (top, data) = (repo(&root), root.join("worker"));
    let first = git(&top, &["rev-parse", "HEAD"]);
    git(&top, &["update-ref", &format!("{REF}/x"), &first]);
    let full = bundle::create(&top, &format!("{REF}/x"), &first, None, &root.join("one")).unwrap();
    let blob = kebbie_hub_protocol::BlobRef { id: Id::blob(b"x"), key: "k".repeat(43), media_type: kebbie_hub_protocol::JOB_BUNDLE_MEDIA_TYPE.into(), bytes: 1 };
    let queued = Queued { from: machine(1), from_name: "devbox".into(), id: cluster::new_id().unwrap(), job: Job { repo: "app".into(), name: "build".into(), command: "echo done".into(), task: "billing".into(), commit: first.clone(), base: None, bundle: vec![blob], on: None }, at: 1, seq: 4 };
    let reports = std::sync::Mutex::new(Vec::<JobReport>::new());
    let beats = std::sync::atomic::AtomicUsize::new(0);
    let slow_fetch = |_: &[kebbie_hub_protocol::BlobRef]| {
        std::thread::sleep(std::time::Duration::from_millis(400));
        Ok(full.clone())
    };
    let (end, lost) = worker::supervise(&queued, &data, &slow_fetch, &|report| {
        reports.lock().unwrap().push(report.clone());
        Ok(())
    }, &|| {
        beats.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }, std::time::Duration::from_millis(50), &|| false, &crate::typing::now_ms, worker::Limits::WORKER);
    let reports = reports.into_inner().unwrap();
    assert_eq!((end, lost), (JobEnd::Exited { code: 0, artifacts: Vec::new() }, None));
    assert!(reports.iter().filter(|report| report.log.is_empty() && report.end.is_none()).count() >= 3, "empty reports go while the bundle comes down: {reports:?}");
    assert_eq!(reports.iter().map(|report| report.seq).collect::<Vec<_>>(), (4..4 + reports.len() as u32).collect::<Vec<_>>(), "numbered on from the reports it had while queued");
    assert!(reports.last().unwrap().end.is_some() && reports.iter().filter(|report| report.end.is_some()).count() == 1, "nothing goes after the end");
    assert!(beats.load(std::sync::atomic::Ordering::SeqCst) >= 3, "and the jobs queued behind it are reported on meanwhile");

    let dir = root.join("queue");
    let job = |name: &str| Job { repo: "app".into(), name: name.into(), command: "true".into(), task: "billing".into(), commit: first.clone(), base: None, bundle: Vec::new(), on: None };
    worker::take(&dir, &machine(1), "devbox", &cluster::new_id().unwrap(), job("build"), 10);
    worker::take(&dir, &machine(1), "devbox", &cluster::new_id().unwrap(), job("lint"), 11);
    let (running, _) = worker::next_queued(&dir).unwrap();
    let posted = RefCell::new(Vec::new());
    let post = |waiting: &Queued, report: &JobReport| {
        posted.borrow_mut().push((waiting.job.name.clone(), report.seq, report.log.clone()));
        Ok(())
    };
    worker::beat_queued(&dir, Some(&running), "studio", &post);
    worker::beat_queued(&dir, Some(&running), "studio", &post);
    assert_eq!(posted.borrow().clone(), [("lint".to_string(), 0, "[kebbie: studio took this job; it waits for the job before it to end]\n".to_string()), ("lint".to_string(), 1, String::new())], "only the waiting job, saying once why it waits");
    std::fs::remove_file(running).unwrap();
    assert_eq!(worker::next_queued(&dir).unwrap().1.seq, 2, "its own reports go on from there");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_hung_job_is_stopped_at_its_timeout_with_its_process_group_and_a_job_that_leaves_a_process_behind_ends_with_its_command() {
    let root = crate::sh::test_dir("jobs-timeout");
    let (top, data) = (repo(&root), root.join("worker"));
    let first = git(&top, &["rev-parse", "HEAD"]);
    git(&top, &["update-ref", &format!("{REF}/x"), &first]);
    let full = bundle::create(&top, &format!("{REF}/x"), &first, None, &root.join("one")).unwrap();
    let blob = kebbie_hub_protocol::BlobRef { id: Id::blob(b"x"), key: "k".repeat(43), media_type: kebbie_hub_protocol::JOB_BUNDLE_MEDIA_TYPE.into(), bytes: 1 };
    let second = std::time::Duration::from_secs(1);
    let run = |command: &str, timeout: std::time::Duration, stop: &dyn Fn() -> bool| {
        let queued = Queued { from: machine(1), from_name: "devbox".into(), id: cluster::new_id().unwrap(), job: Job { repo: "app".into(), name: "build".into(), command: command.into(), task: "billing".into(), commit: first.clone(), base: None, bundle: vec![blob.clone()], on: None }, at: 1, seq: 0 };
        let mut log = String::new();
        let began = std::time::Instant::now();
        let end = worker::execute(&queued, &data, &|_| Ok(full.clone()), &mut |text, _| log.push_str(&text), stop, &crate::typing::now_ms, worker::Limits { timeout, grace: second, drain: second });
        (end, log, began.elapsed())
    };
    let (end, log, took) = run("sleep 31 & echo begun", 60 * second, &|| false);
    assert_eq!((end, log.as_str()), (JobEnd::Exited { code: 0, artifacts: Vec::new() }, "begun\n"));
    assert!(took < 4 * second, "a process left in the background holding the output does not hold the worker: {took:?}");
    assert!(std::process::Command::new("pgrep").args(["-fx", "sleep 31"]).output().unwrap().stdout.is_empty(), "and it goes with the job's process group");
    let (end, log, took) = run("echo begun; sleep 30", second, &|| false);
    assert_eq!((end, log.as_str()), (JobEnd::Failed { reason: "build ran past its timeout of 1 s, so the worker stopped it".into() }, "begun\n"));
    assert!(took < 3 * second, "{took:?}");
    let (end, _, took) = run("trap '' TERM; sleep 30", second, &|| false);
    assert_eq!(end, JobEnd::Failed { reason: "build ran past its timeout of 1 s, so the worker stopped it, with SIGKILL as it outlasted SIGTERM by 1 s".into() });
    assert!(took < 4 * second, "{took:?}");
    let (end, _, _) = run("sleep 30", 60 * second, &|| true);
    assert_eq!(end, JobEnd::Failed { reason: "the worker's kebbie stopped while it took the commit of build, so it stopped the job".into() });
    let stopping = std::time::Instant::now();
    let (end, _, _) = run("sleep 30", 60 * second, &|| stopping.elapsed() >= second);
    assert_eq!(end, JobEnd::Failed { reason: "the worker's kebbie stopped while build ran, so it stopped the job".into() });
    let def = Def { run: "make".to_string(), on: None, timeout_minutes: Some(90) };
    assert_eq!(command_of(&command_for(&def, None)), Command { timeout: Some(90), fetch: None, run: "make" }, "the job's own timeout goes along in its command");
    assert_eq!(command_of("make"), Command { timeout: None, fetch: None, run: "make" });
    let _ = std::fs::remove_dir_all(root);
}

fn inbox_round(dir: &Path) -> Vec<String> {
    let hands = Hands {
        answer: &|_, _| unreachable!(),
        push: &|_, _| unreachable!(),
        prompt: &|_, _| unreachable!(),
        idea: &|_, _| unreachable!(),
        item_answers: &|_, _| unreachable!(),
        start_item: &|_, _| unreachable!(),
        item_edit: &|_, _| unreachable!(),
        item_write: &|_, _| unreachable!(),
        review_open: &|_, _| unreachable!(),
        review_comments: &|_, _| unreachable!(),
        review_read: &|_, _| unreachable!(),
        accept_subtask: &|_, _| unreachable!(),
        found: &|_, _| unreachable!(),
        review_send: &|_, _| unreachable!(),
        lead: &|_, _, _| unreachable!(),
        relay: &|| {},
        notify: &|_| {},
    };
    commands::round(dir, 0, &[], Policy { trust: crate::config::HubTrust::Account, auto: crate::config::AutoApprove::Day }, &hands, &crate::messages::now).unwrap()
}

struct Machine {
    state: PathBuf,
    dir: PathBuf,
    identity: identity::Identity,
    account: AccountKey,
}

impl Machine {
    fn connected(hub: &TestHub, browser: &mut kebbie_hub_fakes::browser::Browser, name: &str) -> Machine {
        let state = connected_named(hub, &hub.url, browser, name);
        let dir = identity::dir(&state);
        let identity = identity::load_at(&dir).unwrap().unwrap();
        let account = identity.account.clone().unwrap().key;
        Machine { state, dir, identity, account }
    }

    fn client(&self) -> Client {
        Client::new(&self.identity.url, Some(self.identity.token()), CALL)
    }

    fn work(&self) {
        worker::register(&self.client(), true, None, crate::messages::now()).unwrap();
    }

    fn hub(&self) -> Hub<'_> {
        Hub { state: &self.state, dir: &self.dir, identity: &self.identity, account: &self.account }
    }
}

// The worker's keeper in one step: its inbox round queues what came, then it runs the oldest queued job to its end.
fn worker_round(worker: &Machine, data: &Path, seen: &RefCell<Vec<Queued>>) {
    inbox_round(&worker.dir);
    if let Some((file, queued)) = worker::next_queued(&worker.dir) {
        worker::run_one(&worker.dir, &worker.identity, &worker.account, &worker.client(), data, &file, &queued, &|| false);
        std::fs::remove_file(file).unwrap();
        seen.borrow_mut().push(queued);
    }
}

#[test]
fn a_master_runs_its_worktree_as_it_stands_on_a_worker_of_its_hub_account_end_to_end() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let (master, worker) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"));
    worker.work();
    let root = crate::sh::test_dir("jobs-e2e");
    let (top, data) = (repo(&root), root.join("worker-data"));
    std::fs::write(top.join("tracked.txt"), "edited\n").unwrap();
    std::fs::write(top.join("new.txt"), "untracked\n").unwrap();
    let ran: RefCell<Vec<Queued>> = RefCell::new(Vec::new());
    let run = |command: &str| {
        let def = Def { run: command.to_string(), on: Some(worker::os()), timeout_minutes: None };
        let asked = Asked { top: &top, repo: "app".to_string(), name: "build".to_string(), def: &def };
        let mut printed = String::new();
        let ended = master::go(&master.hub(), &asked, &mut |text| printed.push_str(text), &mut || {
            worker_round(&worker, &data, &ran);
            inbox_round(&master.dir);
        }, &crate::messages::now, &|| false);
        (ended, printed)
    };
    let (ended, printed) = run("cat tracked.txt new.txt; echo $KEBBIE_JOB $KEBBIE_JOB_TASK $KEBBIE_JOB_MASTER");
    assert_eq!(ended, Ok("kebbie: build passed on studio".to_string()), "{printed}");
    assert!(printed.starts_with("kebbie: studio runs build for app on ") && printed.ends_with("edited\nuntracked\nbuild billing-refunds devbox\n"), "uncommitted and untracked files run too: {printed}");
    let first = ran.borrow()[0].job.commit.clone();
    assert_eq!(ran.borrow()[0].job.base, None, "the first job carries the whole history");
    assert!(git(&top, &["for-each-ref", REF]).is_empty() && std::fs::read_dir(master.state.join("jobs")).unwrap().flatten().all(|entry| !entry.file_name().to_string_lossy().starts_with("cmd_")), "the snapshot's ref and the job's files go when it ends");
    let blob = ran.borrow()[0].job.bundle[0].id.clone();
    assert!(master.client().get_bytes(&format!("/v1/m/blobs/{blob}")).is_err(), "and so do its blobs on the hub");

    let (ended, printed) = run("echo failing; exit 3");
    assert_eq!((ended, printed.ends_with("failing\n")), (Err("kebbie: build failed on studio with exit 3".to_string()), true), "{printed}");
    assert_eq!(ran.borrow()[1].job.base.as_deref(), Some(first.as_str()), "the next job builds on what the worker took");
    assert!(ran.borrow()[1].job.bundle.is_empty(), "an unchanged worktree sends no bundle at all");

    std::fs::write(top.join("third.txt"), "three\n").unwrap();
    commit(&top, "three");
    let (ended, printed) = run("ls");
    assert_eq!(ended, Ok("kebbie: build passed on studio".to_string()), "{printed}");
    assert!(printed.ends_with("new.txt\nthird.txt\ntracked.txt\n"), "{printed}");
    assert_eq!(ran.borrow()[2].job.base.as_deref(), Some(first.as_str()));
    assert!(!ran.borrow()[2].job.bundle.is_empty());

    std::fs::remove_dir_all(data.join("app/.store")).unwrap();
    std::fs::write(top.join("fourth.txt"), "four\n").unwrap();
    let (ended, printed) = run("cat fourth.txt");
    assert_eq!(ended, Ok("kebbie: build passed on studio".to_string()), "{printed}");
    assert!(printed.contains("lacks the commit this job builds on, so it gets the whole history\n") && printed.ends_with("four\n"), "{printed}");
    assert_eq!(ran.borrow().iter().skip(3).map(|queued| queued.job.base.is_some()).collect::<Vec<_>>(), [true, false], "sent once from the base, then once whole");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_worker_fetches_a_jobs_history_from_origin_and_gets_it_whole_through_the_hub_when_it_cannot_end_to_end() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let (master, worker) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"));
    worker.work();
    let root = crate::sh::test_dir("jobs-e2e-origin");
    let top = repo(&root);
    let origin = origin_of(&top, &root);
    let pushed = git(&top, &["rev-parse", "HEAD"]);
    std::fs::write(top.join("local.txt"), "not pushed\n").unwrap();
    let ran: RefCell<Vec<Queued>> = RefCell::new(Vec::new());
    let run = |data: &Path| {
        let def = Def { run: "cat local.txt".to_string(), on: Some(worker::os()), timeout_minutes: None };
        let asked = Asked { top: &top, repo: "app".to_string(), name: "build".to_string(), def: &def };
        let mut printed = String::new();
        let ended = master::go(&master.hub(), &asked, &mut |text| printed.push_str(text), &mut || {
            worker_round(&worker, data, &ran);
            inbox_round(&master.dir);
        }, &crate::messages::now, &|| false);
        (ended, printed)
    };
    let fetching = root.join("fetching");
    store_reaching(&fetching, &origin);
    let (ended, printed) = run(&fetching);
    assert_eq!(ended, Ok("kebbie: build passed on studio".to_string()), "{printed}");
    assert!(printed.contains(&format!("fetching the history up to {} from {ORIGIN}", &pushed[..12])) && printed.ends_with("not pushed\n"), "{printed}");
    let first = ran.borrow()[0].job.clone();
    assert_eq!((first.base.as_deref(), command_of(&first.command).fetch.as_deref()), (Some(pushed.as_str()), Some(ORIGIN)), "the first job builds on what origin has");

    let cut_off = root.join("cut-off");
    store_reaching(&cut_off, &root.join("missing.git"));
    let (ended, printed) = run(&cut_off);
    assert_eq!(ended, Ok("kebbie: build passed on studio".to_string()), "{printed}");
    assert!(printed.contains(&format!("kebbie: studio lacks the commit this job builds on, so it fetches the history from {ORIGIN}\n")) && printed.contains(&format!("kebbie: studio did not fetch the history from {ORIGIN}, so it gets the whole history\n")), "{printed}");
    assert_eq!(ran.borrow().iter().skip(1).map(|queued| (queued.job.base.is_some(), command_of(&queued.job.command).fetch.is_some())).collect::<Vec<_>>(), [(true, false), (true, true), (false, false)], "from the commit it took, then from origin, then whole");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_job_too_big_for_the_hub_says_what_to_do_about_it() {
    let remote = bundle::Remote { url: ORIGIN.to_string(), base: "a".repeat(40) };
    let mib = 1024 * 1024;
    assert_eq!(master::too_big("app", "studio", None, None, 45 * mib), "kebbie: app's history is 45.0 MiB as a git bundle, and the hub carries at most 31.9 MiB per job. A worker fetches the history itself from origin when origin is an https or ssh URL that holds part of this branch: push the branch there, and only what is not pushed goes through the hub");
    assert!(master::too_big("app", "studio", Some(&remote), None, 45 * mib).starts_with(&format!("kebbie: studio did not fetch app's history from {ORIGIN}, and the whole history is 45.0 MiB")));
    assert!(master::too_big("app", "studio", Some(&remote), Some(40 * mib), 45 * mib).starts_with(&format!("kebbie: what this job holds that {ORIGIN} lacks, uncommitted files included, is 40.0 MiB")));
}

#[test]
fn a_report_from_a_machine_that_is_not_the_jobs_worker_is_refused_end_to_end() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let (master, worker, stranger) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"), Machine::connected(&hub, &mut browser, "intruder"));
    worker.work();
    stranger.work();
    let cluster = cluster::fetch(&master.dir, &master.client(), &master.account).unwrap();
    let decoy = Job { repo: "app".into(), name: "lint".into(), command: "true".into(), task: "x".into(), commit: "a".repeat(40), base: None, bundle: Vec::new(), on: None };
    let to_stranger = cluster::seal_to(&master.identity, &master.account, &stranger.identity.machine, &cluster.keys[&stranger.identity.machine], cluster::new_id().unwrap(), &Payload::Job(decoy), crate::messages::now()).unwrap();
    cluster::post(&master.client(), &to_stranger).unwrap();
    let root = crate::sh::test_dir("jobs-e2e-forged");
    let (top, data) = (repo(&root), root.join("worker-data"));
    let def = Def { run: "echo real".to_string(), on: Some("studio".to_string()), timeout_minutes: None };
    let asked = Asked { top: &top, repo: "app".to_string(), name: "build".to_string(), def: &def };
    let ran = RefCell::new(Vec::new());
    let forged = RefCell::new(Vec::new());
    let mut printed = String::new();
    let ended = master::go(&master.hub(), &asked, &mut |text| printed.push_str(text), &mut || {
        if forged.borrow().is_empty() {
            let sent: Sent = std::fs::read_dir(master.state.join("jobs")).unwrap().flatten().find(|entry| entry.file_name().to_string_lossy().ends_with(".sent.json")).map(|entry| serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap()).unwrap();
            let keys = cluster::sender(&stranger.dir, &stranger.client(), &stranger.account, &master.identity.machine, false).unwrap().0;
            let report = JobReport { job: sent.id.clone(), seq: 0, log: "forged\n".into(), end: Some(JobEnd::Exited { code: 0, artifacts: Vec::new() }) };
            let envelope = cluster::seal_to(&stranger.identity, &stranger.account, &master.identity.machine, &keys, cluster::new_id().unwrap(), &Payload::JobReport(report), crate::messages::now()).unwrap();
            cluster::post(&stranger.client(), &envelope).unwrap();
            forged.borrow_mut().extend(inbox_round(&master.dir));
        }
        worker_round(&worker, &data, &ran);
        inbox_round(&master.dir);
    }, &crate::messages::now, &|| false);
    assert_eq!(ended, Ok("kebbie: build passed on studio".to_string()), "{printed}");
    assert!(printed.ends_with("real\n") && !printed.contains("forged"), "{printed}");
    assert!(forged.borrow().iter().any(|note| note.starts_with("jobs: refused a report on build from") && note.contains("the job went to studio")), "{:?}", forged.borrow());
    for machine in [&master, &worker] {
        let seq = commands::bridge(&machine.dir).inbox_seq;
        let left: kebbie_hub_api::Inbox = machine.client().get(&format!("/v1/m/inbox?after={seq}&wait=0")).unwrap();
        assert!(seq > 0 && left.messages.is_empty(), "the round moves past jobs and reports as past commands");
        assert!(commands::ledger(&machine.dir).unsent.is_empty() && commands::ledger(&machine.dir).started.is_empty(), "a job or a report is never answered as a command");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn only_machines_on_the_accounts_signed_list_send_or_take_jobs() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let (master, worker) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"));
    worker.work();
    let mut listed: AccountMachines = master.client().get("/v1/m/machines").unwrap();
    let outsider = machine(7);
    listed.machines.push(AccountMachine { machine: outsider.clone(), name: "outsider".into(), worker_os: Some("macos".into()), last_seen: Some(u64::MAX) });
    let cluster = cluster::verify(&listed, &master.account).unwrap();
    assert!(!cluster.rows.iter().any(|row| row.machine == outsider), "a row the hub lists beside the signed list is no machine of the account");
    assert_eq!(cluster::pick(&cluster, &master.identity.machine, None, crate::messages::now()).map(|row| row.name.as_str()), Some("studio"), "even when the hub says it was seen last");
    let mut tampered = listed.clone();
    tampered.list.as_mut().unwrap().list.machines.push(ListedMachine { machine: outsider.clone(), keys: SecretKeys::generate().public() });
    assert!(cluster::verify(&tampered, &master.account).is_err(), "a list changed after the account signed it trusts no one");
    assert!(cluster::sender(&worker.dir, &worker.client(), &worker.account, &outsider, true).unwrap_err().contains("is not on the account's signed list"));
    assert!(cluster::sender(&worker.dir, &worker.client(), &worker.account, &master.identity.machine, true).is_ok());
}

#[test]
fn a_machine_the_account_removed_sends_no_more_jobs_and_a_list_older_than_one_seen_is_refused() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let (master, worker) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"));
    let from = master.identity.machine.clone();
    assert!(cluster::sender(&worker.dir, &worker.client(), &worker.account, &from, false).is_ok());
    let before: AccountMachines = worker.client().get("/v1/m/machines").unwrap();
    hub.runtime.block_on(browser.remove_machine(&from)).unwrap();
    assert!(cluster::sender(&worker.dir, &worker.client(), &worker.account, &from, false).is_ok(), "a report may still use the list cached a moment ago");
    assert!(cluster::sender(&worker.dir, &worker.client(), &worker.account, &from, true).unwrap_err().contains("is not on the account's signed list"), "a job is checked against the list as it is now");
    assert!(cluster::trust(&worker.dir, &before, &worker.account).unwrap_err().contains("older than version"), "the hub handing back the list from before the removal is refused");
    std::fs::write(worker.dir.join("jobs/machines.json"), serde_json::to_string(&before).unwrap()).unwrap();
    assert!(cluster::sender(&worker.dir, &worker.client(), &worker.account, &from, false).unwrap_err().contains("is not on the account's signed list"), "and so is a cache rolled back to it");
}

fn job_files(state: &Path) -> Vec<String> {
    std::fs::read_dir(state.join("jobs")).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().to_string()).filter(|name| name.starts_with("cmd_")).collect()
}

#[test]
fn a_job_run_stopped_with_ctrl_c_leaves_nothing_behind_here_or_on_the_hub() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let (master, worker) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"));
    worker.work();
    let root = crate::sh::test_dir("jobs-interrupted");
    let top = repo(&root);
    std::fs::write(top.join("new.txt"), "untracked\n").unwrap();
    let def = Def { run: "true".to_string(), on: None, timeout_minutes: None };
    let asked = Asked { top: &top, repo: "app".to_string(), name: "build".to_string(), def: &def };
    let blobs = RefCell::new(Vec::new());
    let interrupted = std::cell::Cell::new(false);
    let ended = master::go(&master.hub(), &asked, &mut |_| {}, &mut || {
        let sent: Sent = std::fs::read_dir(master.state.join("jobs")).unwrap().flatten().find(|entry| entry.file_name().to_string_lossy().ends_with(".sent.json")).map(|entry| serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap()).unwrap();
        blobs.borrow_mut().extend(sent.blobs);
        assert!(master::waiting(&master.state), "the keeper polls while kebbie job run follows the job");
        interrupted.set(true);
    }, &crate::messages::now, &|| interrupted.get());
    assert!(ended.unwrap_err().starts_with("kebbie: stopped following build; its commit is gone from the hub"));
    assert!(!blobs.borrow().is_empty() && blobs.borrow().iter().all(|blob| master.client().get_bytes(&format!("/v1/m/blobs/{blob}")).is_err()), "the job's blobs are deleted");
    assert!(git(&top, &["for-each-ref", REF]).is_empty() && job_files(&master.state).is_empty() && !master::waiting(&master.state), "and its ref and files, so the keeper stops polling");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_job_whose_kebbie_job_run_was_killed_outright_is_swept_by_the_next_one() {
    let hub = open_hub(|_| {});
    let mut browser = member(&hub);
    let master = Machine::connected(&hub, &mut browser, "devbox");
    let mut parts = Vec::new();
    bundle::upload(b"a bundle", &mut bundle::Calls::new(&|| 0, &|_| {}, &|| false), &|path, body| master.client().put_bytes(path, body), &mut parts).unwrap();
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    gone.wait().unwrap();
    let left = Sent { blobs: parts.iter().map(|part| part.id.clone()).collect(), pid: gone.id(), ..sent(machine(2)) };
    std::fs::create_dir_all(master.state.join("jobs")).unwrap();
    std::fs::write(master::part(&master.state, &left.id, "sent.json"), serde_json::to_string(&left).unwrap()).unwrap();
    std::fs::write(master::part(&master.state, &left.id, "log"), "half\n").unwrap();
    assert!(!master::waiting(&master.state), "the keeper does not poll for a job run that is gone");
    master::sweep(&master.state, &master.client());
    assert!(job_files(&master.state).is_empty() && master.client().get_bytes(&format!("/v1/m/blobs/{}", left.blobs[0])).is_err());
}

#[test]
fn a_hub_whose_plan_has_no_jobs_says_they_are_part_of_hub_pro() {
    let hub = open_gated_hub(|_| {}, Some(Box::new(|_: &kebbie_hub::store::Store, _: &str, gated: Gated| match gated { Gated::Command(Kind::Job) => Err(Refusal { url: Some("https://hub.example.com/billing".to_string()), pro: true }), _ => Ok(()) })));
    let mut browser = member(&hub);
    let (master, worker) = (Machine::connected(&hub, &mut browser, "devbox"), Machine::connected(&hub, &mut browser, "studio"));
    worker.work();
    let root = crate::sh::test_dir("jobs-pro");
    let top = repo(&root);
    let def = Def { run: "true".to_string(), on: None, timeout_minutes: None };
    let asked = Asked { top: &top, repo: "app".to_string(), name: "build".to_string(), def: &def };
    let ended = master::go(&master.hub(), &asked, &mut |_| {}, &mut || panic!("nothing to wait for"), &crate::messages::now, &|| false);
    assert_eq!(ended, Err("Running a job on another machine is part of Hub Pro: https://hub.example.com/billing".to_string()));
    assert!(git(&top, &["for-each-ref", REF]).is_empty(), "the snapshot's ref goes with the refusal too");
    let _ = std::fs::remove_dir_all(root);
}
