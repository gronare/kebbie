use std::path::Path;

use toml::Table;

use super::{parse_machine, parse_repo, path, read, short_hostname, unknown_in_file, update, warn_once, AgentUpdates, Channel, Machine, Report, Worktree, DEFAULT_KEEPER, DEFAULT_PROPOSE_ISSUES, DEFAULT_RECOVER, MACHINE_KEYS, REPO_KEYS};
use crate::sh::Result;

pub fn load_machine(path: &Path) -> Result<Machine> {
    check_machine(path).map(|(machine, unknown)| warn_once(machine, &unknown))
}

pub fn load_repo(path: &Path) -> Result<Worktree> {
    check_repo(path).map(|(worktree, unknown)| warn_once(worktree, &unknown))
}

pub fn check_machine(path: &Path) -> Result<(Machine, Vec<String>)> {
    let source = path.display().to_string();
    read(path)?.map_or(Ok((Machine::default(), Vec::new())), |text| Ok((parse_machine(&text, &source)?, unknown_in_file(&text, &source, &MACHINE_KEYS))))
}

pub fn check_repo(path: &Path) -> Result<(Worktree, Vec<String>)> {
    let source = path.display().to_string();
    read(path)?.map_or(Ok((Worktree::default(), Vec::new())), |text| Ok((parse_repo(&text, &source)?, unknown_in_file(&text, &source, &REPO_KEYS))))
}

pub(super) fn table(text: &str, source: &str) -> Result<Table> {
    text.parse().map_err(|error: toml::de::Error| format!("{source}: {}", error.to_string().trim()))
}

pub fn propose_issues_enabled() -> Result<bool> {
    Ok(load_machine(&path())?.propose_issues.unwrap_or(DEFAULT_PROPOSE_ISSUES))
}

pub fn keeper_enabled() -> Result<bool> {
    Ok(load_machine(&path())?.keeper.unwrap_or(DEFAULT_KEEPER))
}

pub fn recover_enabled() -> Result<bool> {
    Ok(load_machine(&path())?.recover.unwrap_or(DEFAULT_RECOVER))
}

// The name other machines on the same vault see this kebbie's claims under.
pub fn machine() -> String {
    load_machine(&path()).ok().and_then(|machine| machine.machine_name).unwrap_or_else(short_hostname)
}

pub fn report_settings() -> Result<Report> {
    Ok(load_machine(&path())?.report)
}

pub fn agent_updates_in(path: &Path) -> AgentUpdates {
    load_machine(path).ok().and_then(|machine| machine.agent_updates).unwrap_or_default()
}

pub fn channel() -> Result<Channel> {
    channel_in(&path())
}

// kebbie update reads only [update] channel, so nothing else in config.toml can keep a machine from updating.
pub fn channel_in(path: &Path) -> Result<Channel> {
    let source = path.display().to_string();
    let Some(text) = read(path)? else { return Ok(Channel::default()) };
    update(&table(&text, &source)?).map(Option::unwrap_or_default).map_err(|error| format!("{source}: {error}"))
}
