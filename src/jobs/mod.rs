use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kebbie_hub_protocol::{AccountKey, Envelope, Payload};

use crate::hub::http::Client;
use crate::hub::identity::Identity;
use crate::sh::Result;

mod bundle;
mod cluster;
mod worker;
#[cfg(test)]
mod tests;

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

fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share/kebbie/worker")
}

pub fn receive(dir: &Path, identity: &Identity, account: &AccountKey, client: &Client, envelope: &Envelope, now: u64) -> Option<String> {
    let header = &envelope.header;
    let opened = cluster::sender(dir, client, account, &header.from).and_then(|(keys, name)| cluster::open_message(envelope, identity, &keys, now).map(|payload| (payload, name)));
    match opened {
        Err(error) => Some(format!("jobs: dropped {} {} from {}: {error}", header.kind.name(), header.id, header.from)),
        Ok((Payload::Job(job), name)) => worker::take(dir, &header.from, &name, &header.id, job, now),
        Ok((_, _)) => Some(format!("jobs: dropped {} {} from {}: not a job or a report", header.kind.name(), header.id, header.from)),
    }
}

// The keeper's inbox round runs for a hub that takes no commands too while this machine is a worker.
pub fn wants_inbox(_state: &Path) -> bool {
    crate::config::worker_enabled().unwrap_or(false)
}
