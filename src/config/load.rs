use std::path::Path;

use toml::Table;

use super::{parse_machine, parse_repo, path, read, short_hostname, unknown_in_file, unknown_in_machine_file, update, warn_once, AgentUpdates, ApproveFrom, AutoApprove, Channel, HubCommands, HubTrust, LinkNetwork, Machine, Report, Worktree, DEFAULT_KEEPER, DEFAULT_PROPOSE_ISSUES, DEFAULT_RECOVER, REPO_KEYS};
use crate::sh::Result;

pub fn load_machine(path: &Path) -> Result<Machine> {
    check_machine(path).map(|(machine, unknown)| warn_once(machine, &unknown))
}

pub fn load_repo(path: &Path) -> Result<Worktree> {
    check_repo(path).map(|(worktree, unknown)| warn_once(worktree, &unknown))
}

pub fn check_machine(path: &Path) -> Result<(Machine, Vec<String>)> {
    let source = path.display().to_string();
    read(path)?.map_or(Ok((Machine::default(), Vec::new())), |text| Ok((parse_machine(&text, &source)?, unknown_in_machine_file(&text, &source))))
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

pub fn fault_watcher_enabled() -> Result<bool> {
    fault_watcher_in(&path())
}

pub fn fault_watcher_in(file: &Path) -> Result<bool> {
    Ok(load_machine(file)?.fault_watcher.unwrap_or(false))
}

pub fn lead_profile() -> Result<Option<String>> {
    Ok(load_machine(&path())?.lead_profile.filter(|name| name != super::NONE))
}

pub fn build_settings() -> Result<(u32, Vec<String>)> {
    let builds = load_machine(&path())?.builds;
    Ok((builds.slots.unwrap_or(super::DEFAULT_BUILD_SLOTS), builds.commands.unwrap_or_else(|| super::DEFAULT_BUILD_COMMANDS.map(str::to_string).to_vec())))
}

pub fn disk_limits() -> Result<(crate::disk::Limit, crate::disk::Limit)> {
    let disk = load_machine(&path())?.disk;
    Ok((disk.trim_below.unwrap_or(crate::disk::DEFAULT_TRIM_BELOW), disk.builds_below.unwrap_or(crate::disk::DEFAULT_BUILDS_BELOW)))
}

// Whether new ideas from your devices go to this machine first: hub.ideas_machine names it.
pub fn is_idea_machine() -> bool {
    load_machine(&path()).ok().is_some_and(|machine| names_itself(&machine, &short_hostname()))
}

pub fn names_itself(machine: &Machine, host: &str) -> bool {
    machine.ideas_machine.as_deref().is_some_and(|named| named.eq_ignore_ascii_case(machine.machine_name.as_deref().unwrap_or(host)))
}

pub fn writer_profile() -> String {
    load_machine(&path()).ok().and_then(|machine| machine.writer_profile).unwrap_or_else(|| crate::backlog_writer::WRITER.to_string())
}

pub fn keeper_enabled() -> Result<bool> {
    Ok(load_machine(&path())?.keeper.unwrap_or(DEFAULT_KEEPER))
}

pub fn worker_enabled() -> Result<bool> {
    Ok(load_machine(&path())?.worker.unwrap_or(false))
}

// The private networks a link uses, in the order a dialing machine tries them.
pub fn link_networks() -> Result<Vec<LinkNetwork>> {
    Ok(load_machine(&path())?.link_networks.unwrap_or_else(|| LinkNetwork::ALL.to_vec()))
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HubGates {
    pub commands: HubCommands,
    pub trust: HubTrust,
    pub auto: AutoApprove,
    pub from: ApproveFrom,
}

// What the hub's gates fall back to while config.toml does not parse: no commands, no device approved by time or by account, approvals only at the machine.
pub const CLOSED_GATES: HubGates = HubGates { commands: HubCommands::OFF, trust: HubTrust::Device, auto: AutoApprove::Never, from: ApproveFrom::Machine };

// The command thread reads this each round, so a change takes effect on the next command; the error is why the gates are closed.
pub fn hub_gates_in(path: &Path) -> (HubGates, Option<String>) {
    match load_machine(path) {
        Ok(machine) => (HubGates { commands: machine.hub_commands.unwrap_or_default(), trust: machine.hub_trust.unwrap_or_default(), auto: machine.hub_auto_approve.unwrap_or_default(), from: machine.hub_approve_from.unwrap_or_default() }, None),
        Err(error) => (CLOSED_GATES, Some(error)),
    }
}

pub fn hub_gates() -> (HubGates, Option<String>) {
    hub_gates_in(&path())
}

pub fn hub_commands() -> HubCommands {
    hub_gates().0.commands
}

pub fn hub_trust() -> (HubTrust, AutoApprove) {
    let gates = hub_gates().0;
    (gates.trust, gates.auto)
}

pub fn hub_approve_from() -> ApproveFrom {
    hub_gates().0.from
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
