use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kebbie_hub_protocol::{AccountKey, Envelope, Payload};

use crate::hub::http::Client;
use crate::hub::identity::Identity;
use crate::sh::Result;

mod bundle;
mod cluster;
mod master;
mod worker;
#[cfg(test)]
mod tests;

pub use master::run;
pub use worker::{spawn, stop};

pub const REF: &str = "refs/kebbie/jobs";

#[derive(Clone, Debug, PartialEq)]
pub struct Def {
    pub run: String,
    pub on: Option<String>,
}

pub fn parse(table: &toml::Table) -> Result<BTreeMap<String, Def>> {
    let Some(jobs) = table.get("jobs") else { return Ok(BTreeMap::new()) };
    let jobs = jobs.as_table().ok_or("jobs: must be a table of jobs, as [jobs] with build = \"xcodebuild -scheme App build\"")?;
    jobs.iter().map(|(name, value)| {
        let text = |value: &toml::Value, key: &str| value.as_str().map(str::trim).filter(|text| !text.is_empty()).map(str::to_string).ok_or_else(|| format!("jobs.{key}: must be a non-empty string"));
        let def = match value {
            toml::Value::String(_) => Def { run: text(value, name)?, on: None },
            toml::Value::Table(job) => {
                if let Some(unknown) = job.keys().find(|key| !["run", "on"].contains(&key.as_str())) {
                    return Err(format!("jobs.{name}.{unknown}: unknown key; known keys here: run, on"));
                }
                let run = job.get("run").ok_or_else(|| format!("jobs.{name}: needs run, the command the worker runs"))?;
                Def { run: text(run, &format!("{name}.run"))?, on: job.get("on").map(|on| text(on, &format!("{name}.on"))).transpose()? }
            }
            _ => return Err(format!("jobs.{name}: must be a command, or a table with run and on")),
        };
        Ok((name.clone(), def))
    }).collect()
}

pub fn defined(file: &Path) -> Result<BTreeMap<String, Def>> {
    let Ok(text) = std::fs::read_to_string(file) else { return Ok(BTreeMap::new()) };
    let table: toml::Table = text.parse().map_err(|error| format!("{}: {error}", file.display()))?;
    parse(&table).map_err(|error| format!("{}: {error}", file.display()))
}

// A repo, task or machine name becomes a folder or file name on another machine, so it keeps only what cannot leave its folder.
fn safe(name: &str) -> String {
    let kept: String = name.chars().map(|character| if character.is_ascii_alphanumeric() || "-_.@".contains(character) { character } else { '_' }).collect();
    if kept.is_empty() || kept.chars().all(|character| character == '.') { "_".to_string() } else { kept }
}

// The hub takes a repo, job, task or worker as a plain name: lowercase letters, digits and - _ . from a letter or digit on, at most 64 bytes.
fn token(name: &str) -> String {
    let mapped: String = name.chars().map(|character| match character.to_ascii_lowercase() {
        lower @ ('a'..='z' | '0'..='9' | '-' | '_' | '.') => lower,
        _ => '-',
    }).collect();
    let trimmed: String = mapped.trim_start_matches(|character: char| !character.is_ascii_alphanumeric()).chars().take(64).collect();
    if trimmed.is_empty() { "x".to_string() } else { trimmed }
}

fn repo_name(top: &Path) -> String {
    crate::repo::main_checkout(top).map_or_else(|_| crate::repo::name_of(top), |main| crate::repo::name_of(&main))
}

fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share/kebbie/worker")
}

// The snapshot takes HEAD's date, so a worktree that has not changed is the same commit and the worker that has it is sent nothing.
fn git_with_index(worktree: &Path, index: &Path, args: &[&str], date: &str) -> Result<String> {
    let output = crate::sh::git_command().current_dir(worktree).args(args).env("GIT_INDEX_FILE", index)
        .envs([("GIT_AUTHOR_NAME", "kebbie"), ("GIT_AUTHOR_EMAIL", "kebbie@localhost"), ("GIT_COMMITTER_NAME", "kebbie"), ("GIT_COMMITTER_EMAIL", "kebbie@localhost"), ("GIT_AUTHOR_DATE", date), ("GIT_COMMITTER_DATE", date)])
        .output().map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    if output.status.success() { Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()) } else { Err(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim())) }
}

// The worktree as it stands, uncommitted and untracked files included, as a commit of its own: the branch, the index and the files are left as they are.
fn snapshot(worktree: &Path, id: &str) -> Result<String> {
    let head = crate::repo::git(worktree, &["rev-parse", "HEAD"])?;
    if crate::repo::git(worktree, &["status", "--porcelain"])?.is_empty() {
        return Ok(head);
    }
    let index = std::env::temp_dir().join(format!("kebbie-job-index-{id}"));
    let date = format!("{} +0000", crate::repo::git(worktree, &["log", "-1", "--format=%ct", "HEAD"])?);
    let made = (|| {
        git_with_index(worktree, &index, &["read-tree", "HEAD"], &date)?;
        git_with_index(worktree, &index, &["add", "--all"], &date)?;
        let tree = git_with_index(worktree, &index, &["write-tree"], &date)?;
        git_with_index(worktree, &index, &["commit-tree", &tree, "-p", &head, "-m", "The worktree as a kebbie job ran it"], &date)
    })();
    let _ = std::fs::remove_file(&index);
    made
}

pub fn receive(dir: &Path, identity: &Identity, account: &AccountKey, client: &Client, envelope: &Envelope, now: u64) -> Option<String> {
    let header = &envelope.header;
    let opened = cluster::sender(dir, client, account, &header.from).and_then(|(keys, name)| cluster::open_message(envelope, identity, &keys, now).map(|payload| (payload, name)));
    match opened {
        Err(error) => Some(format!("jobs: dropped {} {} from {}: {error}", header.kind.name(), header.id, header.from)),
        Ok((Payload::Job(job), name)) => worker::take(dir, &header.from, &name, &header.id, job, now),
        Ok((Payload::JobReport(report), _)) => master::report(&state_of(dir), &header.from, &report, now),
        Ok((_, _)) => Some(format!("jobs: dropped {} {} from {}: not a job or a report", header.kind.name(), header.id, header.from)),
    }
}

// The keeper's inbox round runs for a hub that takes no commands too while this machine is a worker or waits on a job's output.
pub fn wants_inbox(state: &Path) -> bool {
    crate::config::worker_enabled().unwrap_or(false) || master::waiting(state)
}

// Job state lives beside the identity folders: <state>/hub is the identity of the real hub, <state>/hub-dev that of the dev hub.
fn state_of(dir: &Path) -> PathBuf {
    dir.parent().map_or_else(|| dir.to_path_buf(), Path::to_path_buf)
}
