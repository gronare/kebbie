use std::path::{Path, PathBuf};

use kebbie_hub_api::{AccountMachine, AccountMachines};
use kebbie_hub_protocol::{AccountKey, Id, Job, JobEnd, Prefix, SecretKeys};

use super::cluster;
use super::worker::{self, Batch, Queued, Seen};
use super::*;

fn row(seed: u8, name: &str, os: Option<&str>, seen: Option<u64>) -> AccountMachine {
    AccountMachine { machine: machine(seed), name: name.to_string(), worker_os: os.map(str::to_string), last_seen: seen }
}

fn machine(seed: u8) -> Id {
    Id::ulid(Prefix::Machine, 1, [seed; 10]).unwrap()
}

#[test]
fn a_repos_jobs_come_from_its_config_on_the_master_as_a_command_or_a_table_that_picks_the_worker() {
    let jobs = |text: &str| parse(&text.parse::<toml::Table>().unwrap());
    let read = jobs("[jobs]\nlint = \"swiftlint\"\n\n[jobs.build]\nrun = \"xcodebuild -scheme App build\"\non = \"macos\"\n").unwrap();
    assert_eq!(read.get("lint"), Some(&Def { run: "swiftlint".to_string(), on: None }));
    assert_eq!(read.get("build"), Some(&Def { run: "xcodebuild -scheme App build".to_string(), on: Some("macos".to_string()) }));
    assert!(jobs("[worktree]\ndev_url = \"x\"\n").unwrap().is_empty(), "a repo without jobs has none");
    for (text, said) in [("jobs = 3\n", "must be a table of jobs"), ("[jobs]\nbuild = \"  \"\n", "jobs.build: must be a non-empty string"), ("[jobs.build]\non = \"macos\"\n", "jobs.build: needs run"), ("[jobs.build]\nrun = \"x\"\nwhere = \"mac\"\n", "jobs.build.where: unknown key"), ("[jobs]\nbuild = 3\n", "must be a command, or a table")] {
        assert!(jobs(text).unwrap_err().contains(said), "{text}");
    }
}

#[test]
fn names_from_the_other_machine_never_leave_their_folder_and_names_for_the_hub_are_plain() {
    assert_eq!((safe("../../etc"), safe(".."), safe(""), safe("fix/login page"), safe("billing@app")), (".._.._etc".to_string(), "_".to_string(), "_".to_string(), "fix_login_page".to_string(), "billing@app".to_string()), "one folder name, whatever was sent");
    assert_eq!((token("Fix/Login page"), token("..hidden"), token("MacBook-Pro"), token("/"), token(&"a".repeat(80)).len()), ("fix-login-page".to_string(), "hidden".to_string(), "macbook-pro".to_string(), "x".to_string(), 64));
}

#[test]
fn a_list_the_account_did_not_sign_trusts_no_machine() {
    let account = AccountKey { account: Id::account([3; 16]), version: 1, keys: SecretKeys::generate().public() };
    let unsigned = AccountMachines { list: None, machines: vec![row(2, "mac", Some("macos"), Some(1))] };
    assert!(cluster::verify(&unsigned, &account).unwrap_err().contains("not signed by the account"));
}

#[test]
fn a_bundle_goes_up_in_parts_a_blob_holds_and_comes_back_whole() {
    let bundle: Vec<u8> = (0..3 * bundle::PART + 7).map(|index| (index % 251) as u8).collect();
    let parts = bundle::split(&bundle).unwrap();
    assert_eq!(parts.iter().map(|part| part.len()).collect::<Vec<_>>(), [bundle::PART, bundle::PART, bundle::PART, 7]);
    assert_eq!(bundle::join(parts.iter().map(|part| part.to_vec()).collect()), bundle);
    assert!(bundle::split(&[]).unwrap().is_empty(), "a worker that has the commit gets no bundle");
    let too_big = vec![0u8; kebbie_hub_protocol::MAX_JOB_BUNDLE_PARTS * bundle::PART + 1];
    assert!(bundle::split(&too_big).unwrap_err().contains("too big to run as a job"));
    assert_eq!(bundle::split(&too_big[1..]).unwrap().len(), kebbie_hub_protocol::MAX_JOB_BUNDLE_PARTS);
}

#[test]
fn output_goes_to_the_master_every_two_seconds_or_once_twelve_kib_wait_and_a_silent_job_still_says_it_runs() {
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
    assert!(!batch.due(62_999) && batch.due(63_000) && batch.take(63_000).is_empty(), "a minute of silence sends an empty report");
    batch.add(&"x".repeat(300 * 1024));
    let rest = batch.rest();
    assert!(rest[0].starts_with("[kebbie: ") && rest.concat().len() < 260 * 1024, "the oldest output goes once too much waits");
    assert!(rest.iter().all(|part| part.len() <= kebbie_hub_protocol::MAX_JOB_LOG_BYTES));
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
fn a_worker_with_no_repo_runs_a_bundle_takes_a_later_one_from_its_base_and_asks_for_all_when_it_lacks_it() {
    let root = crate::sh::test_dir("jobs-execute");
    let (top, data) = (repo(&root), root.join("worker"));
    let refname = |sha: &str| {
        git(&top, &["update-ref", &format!("{REF}/x"), sha]);
        format!("{REF}/x")
    };
    let first = git(&top, &["rev-parse", "HEAD"]);
    let full = bundle::create(&top, &refname(&first), &first, None, &root.join("one")).unwrap();
    let queued = |commit: &str, base: Option<&str>, command: &str| Queued { from: machine(1), from_name: "devbox".into(), id: cluster::new_id().unwrap(), job: Job { repo: "app".into(), name: "build".into(), command: command.into(), task: "billing".into(), commit: commit.into(), base: base.map(str::to_string), bundle: vec![kebbie_hub_protocol::BlobRef { id: Id::blob(b"x"), key: "k".repeat(43), media_type: kebbie_hub_protocol::JOB_BUNDLE_MEDIA_TYPE.into(), bytes: 1 }], on: None }, at: 1 };
    let run = |job: &Queued, bundle: &[u8]| {
        let mut said: Vec<(String, Option<JobEnd>)> = Vec::new();
        let bundle = bundle.to_vec();
        let end = worker::execute(job, &data, &move |_| Ok(bundle.clone()), &mut |log, end| said.push((log, end)), &|| false, &crate::typing::now_ms);
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
    std::fs::remove_dir_all(data.join("app/.store")).unwrap();
    let (end, log) = run(&queued(&second, Some(&first), "touch ran"), &incremental);
    assert_eq!((end, log.as_str()), (JobEnd::NeedsFull {}, ""), "a base the store lacks asks for the whole history and runs nothing");
    assert!(!data.join("app/billing/ran").exists());
    let (end, _) = run(&queued(&second, None, "true"), b"not a bundle");
    assert!(matches!(&end, JobEnd::Failed { reason } if reason.starts_with("the worker could not take the commit")), "{end:?}");
    let failing = worker::execute(&queued(&second, None, "true"), &data, &|_| Err("the hub answered 404 not-found".to_string()), &mut |_, _| {}, &|| false, &crate::typing::now_ms);
    assert_eq!(failing, JobEnd::Failed { reason: "the worker could not fetch the bundle: the hub answered 404 not-found".into() });
    let _ = std::fs::remove_dir_all(root);
}
