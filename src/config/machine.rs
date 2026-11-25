use toml::{Table, Value};

use super::{flag, layers, unset_keys, repos_dir, strings, subtable, table, text, AgentUpdates, ApproveFrom, AutoApprove, Channel, HubCommands, HubTrust, LinkNetwork, OngoingAt, ReportText, Source, Worktree};
use crate::sh::Result;
use crate::status::Group;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub keys: Vec<(String, String)>,
    pub sources: Option<Vec<Source>>,
    pub groups: Option<Vec<Group>>,
    pub ongoing: Option<OngoingAt>,
    pub template: Option<String>,
    pub next_question: Option<bool>,
    pub channel: Option<Channel>,
    pub agent_updates: Option<AgentUpdates>,
    pub restart: Restart,
    pub keeper: Option<bool>,
    pub worker: Option<bool>,
    pub link_networks: Option<Vec<LinkNetwork>>,
    pub recover: Option<bool>,
    pub machine_name: Option<String>,
    pub hub_url: Option<String>,
    pub hub_commands: Option<HubCommands>,
    pub hub_trust: Option<HubTrust>,
    pub hub_auto_approve: Option<AutoApprove>,
    pub hub_approve_from: Option<ApproveFrom>,
    pub report: Report,
    pub propose_issues: Option<bool>,
    pub lead_profile: Option<String>,
    pub builds: Builds,
    pub disk: Disk,
    pub writer_profile: Option<String>,
    pub ideas_machine: Option<String>,
    pub worktree: Worktree,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Builds {
    pub slots: Option<u32>,
    pub commands: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Disk {
    pub trim_below: Option<crate::disk::Limit>,
    pub builds_below: Option<crate::disk::Limit>,
}

pub const DEFAULT_BUILD_SLOTS: u32 = 1;
pub const DEFAULT_BUILD_COMMANDS: [&str; 1] = ["cargo"];

pub const DEFAULT_HUB_URL: &str = "https://hub.kebbie.dev";
pub(super) const DEFAULT_KEEPER: bool = true;
pub(super) const DEFAULT_RECOVER: bool = true;
pub(super) const DEFAULT_PROPOSE_ISSUES: bool = true;
pub(super) const REPORT_URL: &str = "https://issues.kebbie.dev";
pub(super) const UNSENT_REPORT_URL: &str = "http://127.0.0.1:9";
pub const INBOX_TOKEN_PREFIX: &str = "kha_";

// The inbox token reads every allowed report on the issues endpoint, so Debug never prints it.
#[derive(Clone, Default, PartialEq)]
pub struct Report {
    pub enabled: Option<bool>,
    pub signals: Option<bool>,
    pub text: Option<ReportText>,
    pub inbox_token: Option<String>,
}

impl std::fmt::Debug for Report {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Report").field("enabled", &self.enabled).field("signals", &self.signals).field("text", &self.text).field("inbox_token", &self.inbox_token.as_ref().map(|_| "set")).finish()
    }
}

// enabled = false, from before signals and text were two keys, still turns text off; only signals = true turns signals on.
impl Report {
    // Every cargo test, unit and spawned binary alike, runs a debug build, so only a release build can reach the production endpoint.
    pub fn url(&self) -> &str {
        if cfg!(debug_assertions) { UNSENT_REPORT_URL } else { REPORT_URL }
    }

    pub fn signals(&self) -> bool {
        self.signals == Some(true)
    }

    pub fn signals_asked(&self) -> bool {
        self.signals.is_some() || self.enabled == Some(false)
    }

    pub fn text(&self) -> ReportText {
        self.text.unwrap_or(if self.enabled == Some(false) { ReportText::Never } else { ReportText::Ask })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Restart {
    pub idle_minutes: Option<u64>,
    pub auto: Option<bool>,
    pub lead_context: Option<u64>,
}

pub const DEFAULT_IDLE_MINUTES: u64 = 10;
pub const DEFAULT_LEAD_CONTEXT: u64 = 75;

fn backlog_sources(backlog: &Table) -> Result<Option<Vec<Source>>> {
    let Some(value) = backlog.get("sources") else { return Ok(None) };
    let mut found = Vec::new();
    for name in strings(value, "board.backlog.sources")? {
        let source = Source::ALL.into_iter().find(|source| source.key() == name)
            .ok_or(format!("board.backlog.sources: \"{name}\" is not a source; use \"local\", \"vault\" or \"linear\""))?;
        if found.contains(&source) {
            return Err(format!("board.backlog.sources: \"{name}\" is listed twice"));
        }
        found.push(source);
    }
    Ok(Some(found))
}

fn groups(value: &Value) -> Result<Vec<Group>> {
    let mut found = Vec::new();
    for name in strings(value, "board.groups")? {
        let name = if name == "to-review" { "in-review".to_string() } else { name };
        let group = Group::ALL.into_iter().find(|group| group.key() == name)
            .ok_or(format!("board.groups: \"{name}\" is not a group; use {}", Group::ALL.map(|group| format!("\"{}\"", group.key())).join(", ")))?;
        if found.contains(&group) {
            return Err(format!("board.groups: \"{name}\" is listed twice"));
        }
        found.push(group);
    }
    Ok(found)
}

fn ongoing_at(backlog: &Table) -> Result<Option<OngoingAt>> {
    let Some(name) = text(backlog, "ongoing", "board.backlog.ongoing")? else { return Ok(None) };
    OngoingAt::ALL.into_iter().find(|at| at.key() == name).map(Some)
        .ok_or(format!("board.backlog.ongoing: \"{name}\" is not a place; use \"top\", \"bottom\" or \"hidden\""))
}

#[derive(Default)]
struct Board {
    sources: Option<Vec<Source>>,
    groups: Option<Vec<Group>>,
    ongoing: Option<OngoingAt>,
    template: Option<String>,
    next_question: Option<bool>,
}

fn board(table: &Table) -> Result<Board> {
    let Some(board) = subtable(table, "board", "board")? else { return Ok(Board::default()) };
    let groups = board.get("groups").map(groups).transpose()?;
    let next_question = flag(board, "next_question", "board.next_question")?;
    let Some(backlog) = subtable(board, "backlog", "board.backlog")? else { return Ok(Board { groups, next_question, ..Board::default() }) };
    let template = text(backlog, "template", "board.backlog.template")?.filter(|path| !path.trim().is_empty());
    Ok(Board { sources: backlog_sources(backlog)?, groups, ongoing: ongoing_at(backlog)?, template, next_question })
}

pub(super) fn update(table: &Table) -> Result<Option<Channel>> {
    let Some(update) = subtable(table, "update", "update")? else { return Ok(None) };
    match text(update, "channel", "update.channel")?.as_deref() {
        None => Ok(None),
        Some("main") => Ok(Some(Channel::Main)),
        Some("release") => Ok(Some(Channel::Release)),
        Some(other) => Err(format!("update.channel: \"{other}\" is not a channel; use \"main\" or \"release\"")),
    }
}

fn agent_updates(table: &Table) -> Result<Option<AgentUpdates>> {
    let Some(update) = subtable(table, "update", "update")? else { return Ok(None) };
    match text(update, "agents", "update.agents")?.as_deref() {
        None => Ok(None),
        Some(given) => AgentUpdates::ALL.into_iter().find(|mode| mode.key() == given).map(Some).ok_or_else(|| format!("update.agents: \"{given}\" is not a choice; use \"ask\", \"auto\" or \"notify\"")),
    }
}

// The board's keys are checked against each other in keys.rs, where a bad entry is left out rather than failing the file.
fn keys(table: &Table) -> Result<Vec<(String, String)>> {
    let Some(keys) = subtable(table, "keys", "keys")? else { return Ok(Vec::new()) };
    Ok(keys.iter().map(|(name, value)| (name.clone(), value.as_str().map_or_else(|| format!("{} {}", value.type_str(), "value"), str::to_string))).collect())
}

fn restart(table: &Table) -> Result<Restart> {
    let Some(restart) = subtable(table, "restart", "restart")? else { return Ok(Restart::default()) };
    let idle_minutes = match restart.get("idle_minutes") {
        None => None,
        Some(value) => Some(value.as_integer().filter(|minutes| (1..=1440).contains(minutes)).ok_or("restart.idle_minutes: must be a whole number of minutes, 1 to 1440")? as u64),
    };
    let auto = flag(restart, "auto", "restart.auto")?;
    let lead_context = match restart.get("lead_context") {
        None => None,
        Some(value) => Some(value.as_integer().filter(|percent| (10..=95).contains(percent)).ok_or("restart.lead_context: must be a whole percentage, 10 to 95")? as u64),
    };
    Ok(Restart { idle_minutes, auto, lead_context })
}

fn builds(table: &Table) -> Result<Builds> {
    let Some(builds) = subtable(table, "builds", "builds")? else { return Ok(Builds::default()) };
    let slots = match builds.get("slots") {
        None => None,
        Some(value) => Some(value.as_integer().filter(|slots| (1..=64).contains(slots)).ok_or("builds.slots: must be a whole number of builds at once, 1 to 64")? as u32),
    };
    let commands = builds.get("commands").map(|value| strings(value, "builds.commands")).transpose()?;
    for command in commands.iter().flatten() {
        let program = command.split_whitespace().next().unwrap_or_default();
        if program.is_empty() || !program.chars().all(|character| character.is_ascii_alphanumeric() || "._-".contains(character)) {
            return Err(format!("builds.commands: \"{command}\" must start with a program's name, such as \"cargo\", \"xcodebuild\" or \"npm run build\""));
        }
    }
    Ok(Builds { slots, commands })
}

fn disk(table: &Table) -> Result<Disk> {
    let Some(disk) = subtable(table, "disk", "disk")? else { return Ok(Disk::default()) };
    let limit = |key: &str| -> Result<Option<crate::disk::Limit>> {
        let name = format!("disk.{key}");
        disk.get(key).map(|value| value.as_str().ok_or(format!("{name}: must be free space as \"30 GB\" or \"15%\"")).and_then(|text| crate::disk::Limit::parse(text, &name))).transpose()
    };
    let (trim_below, builds_below) = (limit("trim_below")?, limit("builds_below")?);
    crate::disk::ordered(trim_below.unwrap_or(crate::disk::DEFAULT_TRIM_BELOW), builds_below.unwrap_or(crate::disk::DEFAULT_BUILDS_BELOW))?;
    Ok(Disk { trim_below, builds_below })
}

fn link_networks(table: &Table) -> Result<Option<Vec<LinkNetwork>>> {
    let Some(link) = subtable(table, "link", "link")? else { return Ok(None) };
    let Some(value) = link.get("networks") else { return Ok(None) };
    let mut networks = Vec::new();
    for given in strings(value, "link.networks")? {
        let network = LinkNetwork::ALL.into_iter().find(|network| network.key() == given).ok_or_else(|| format!("link.networks: \"{given}\" is no private network kebbie links on; list \"tailscale\", \"wireguard\" or both, in the order to try them, as in [\"wireguard\", \"tailscale\"]"))?;
        if networks.contains(&network) {
            return Err(format!("link.networks: \"{given}\" is listed twice"));
        }
        networks.push(network);
    }
    Ok(Some(networks))
}

fn propose_issues(table: &Table) -> Result<Option<bool>> {
    let Some(section) = subtable(table, "issues", "issues")? else { return Ok(None) };
    flag(section, "propose", "issues.propose")
}

fn lead_profile(table: &Table) -> Result<Option<String>> {
    let Some(lead) = subtable(table, "lead", "lead")? else { return Ok(None) };
    let Some(name) = text(lead, "profile", "lead.profile")? else { return Ok(None) };
    if name != super::NONE {
        crate::profiles::name(&name).map_err(|error| format!("lead.profile: {error}"))?;
    }
    Ok(Some(name))
}

fn writer_profile(table: &Table) -> Result<Option<String>> {
    let Some(writer) = subtable(table, "writer", "writer")? else { return Ok(None) };
    let Some(name) = text(writer, "profile", "writer.profile")? else { return Ok(None) };
    if name != super::NONE {
        crate::profiles::name(&name).map_err(|error| format!("writer.profile: {error}"))?;
    }
    Ok(Some(name))
}

fn ideas_machine(table: &Table) -> Result<Option<String>> {
    let Some(hub) = subtable(table, "hub", "hub")? else { return Ok(None) };
    let Some(value) = hub.get("ideas_machine") else { return Ok(None) };
    valid_machine_name(value.as_str().ok_or("hub.ideas_machine: must be a string")?).map(Some).map_err(|error| error.replacen("machine.name", "hub.ideas_machine", 1))
}

fn enabled_in(table: &Table, name: &str) -> Result<Option<bool>> {
    let Some(section) = subtable(table, name, name)? else { return Ok(None) };
    flag(section, "enabled", &format!("{name}.enabled"))
}

fn report(table: &Table) -> Result<Report> {
    let Some(report) = subtable(table, "report", "report")? else { return Ok(Report::default()) };
    let enabled = flag(report, "enabled", "report.enabled")?;
    let signals = flag(report, "signals", "report.signals")?;
    let text = report.get("text").map(|value| value.as_str().and_then(|given| ReportText::ALL.into_iter().find(|mode| mode.key() == given)).ok_or("report.text: must be \"ask\", \"always\" or \"never\"")).transpose()?;
    let inbox_token = match report.get("inbox_token") {
        None => None,
        Some(value) => Some(value.as_str().map(str::trim).filter(|token| token.starts_with(INBOX_TOKEN_PREFIX) && token.len() > INBOX_TOKEN_PREFIX.len()).ok_or("report.inbox_token: must be the kha_ token the issues endpoint issued")?.to_string()),
    };
    Ok(Report { enabled, signals, text, inbox_token })
}

fn hub_url(table: &Table) -> Result<Option<String>> {
    let Some(hub) = subtable(table, "hub", "hub")? else { return Ok(None) };
    text(hub, "url", "hub.url")?.map(|url| crate::hub::hub_url(&url).map_err(|error| format!("hub.url: {error}"))).transpose()
}

fn hub_commands(table: &Table) -> Result<Option<HubCommands>> {
    let Some(hub) = subtable(table, "hub", "hub")? else { return Ok(None) };
    const CHOICES: &str = "list any of \"answer\", \"push\", \"prompt\", \"idea\", \"start\", \"edit\", \"review\" and \"lead\", as in [\"answer\", \"prompt\"]";
    match hub.get("commands") {
        None => Ok(None),
        Some(value) if value.is_array() => HubCommands::from_keys(&strings(value, "hub.commands")?).map(Some).map_err(|given| format!("hub.commands: \"{given}\" is not a kind of command; {CHOICES}")),
        Some(_) => {
            let given = text(hub, "commands", "hub.commands")?.unwrap_or_default();
            HubCommands::from_word(&given).map(Some).ok_or_else(|| format!("hub.commands: \"{given}\" is not a choice; {CHOICES}"))
        }
    }
}

fn hub_choice<T: Copy>(table: &Table, key: &str, choices: &[(&str, T)]) -> Result<Option<T>> {
    let Some(hub) = subtable(table, "hub", "hub")? else { return Ok(None) };
    let Some(given) = text(hub, key, &format!("hub.{key}"))? else { return Ok(None) };
    choices.iter().find(|(name, _)| *name == given).map(|(_, choice)| Some(*choice)).ok_or_else(|| {
        let names: Vec<String> = choices.iter().map(|(name, _)| format!("\"{name}\"")).collect();
        let (last, rest) = names.split_last().expect("a key has choices");
        format!("hub.{key}: \"{given}\" is not a choice; use {} or {last}", rest.join(", "))
    })
}

fn machine_name(table: &Table) -> Result<Option<String>> {
    let Some(machine) = subtable(table, "machine", "machine")? else { return Ok(None) };
    let Some(value) = machine.get("name") else { return Ok(None) };
    valid_machine_name(value.as_str().ok_or("machine.name: must be a string")?).map(Some)
}

// The hub refuses a name past 64 characters, so config.toml does too.
pub const MACHINE_NAME_CHARS: usize = 64;

pub fn valid_machine_name(text: &str) -> Result<String> {
    let name = text.trim();
    if name.is_empty() || name.chars().count() > MACHINE_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(format!("machine.name: must be a non-empty string of at most {MACHINE_NAME_CHARS} characters, without control characters such as a tab or a newline"));
    }
    if name.eq_ignore_ascii_case("localhost") {
        return Err("machine.name: \"localhost\" names every machine; give this one a name of its own, such as \"mac\" or \"devbox\"".to_string());
    }
    Ok(name.to_string())
}

// The host's name up to its first dot, as `hostname -s` gives it.
pub fn short_hostname() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: gethostname writes at most buffer.len() bytes into the buffer this frame owns.
    let named = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0;
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
    let name = if named { String::from_utf8_lossy(&buffer[..end]).to_string() } else { String::new() };
    let short = name.split('.').next().unwrap_or_default().trim().to_string();
    if short.is_empty() { "this-machine".to_string() } else { short }
}

fn per_repo(key: &str) -> String {
    format!("{key}: each repo opts in on its own, so it lives in its repo config; set it in {}/<repo>.toml", repos_dir().display())
}

pub fn parse_machine(text: &str, source: &str) -> Result<Machine> {
    let table = table(text, source)?;
    let fail = |error: String| format!("{source}: {error}");
    if let Some(docs) = table.get("docs") {
        let required = docs.as_table().is_some_and(|docs| docs.contains_key("required"));
        return Err(fail(if required { per_repo("docs.required") } else { format!("docs: [docs] is a repo config's table, with the one key required; set it in {}/<repo>.toml", repos_dir().display()) }));
    }
    let worktree = layers(&table).map_err(fail)?;
    if let Some(key) = unset_keys(&worktree).first() {
        return Err(fail(format!("{key}: must not be empty; leave it out of config.toml, and set it to \"\" in a repo config to unset it for that repo")));
    }
    if worktree.dev_route.is_some() {
        return Err(fail(per_repo("worktree.dev_route")));
    }
    if worktree.stage_on_main.is_some() {
        return Err(fail(per_repo("worktree.stage_on_main")));
    }
    if worktree.ci_deploy_window.is_some() {
        return Err(fail(per_repo("ci.deploy_window")));
    }
    if worktree.tests_full_command.is_some() {
        return Err(fail(format!("tests.full_command: each repo has its own full suite, so it lives in its repo config; set it in {}/<repo>.toml", repos_dir().display())));
    }
    crate::routines::parse_list(&table, None, source).map_err(fail)?;
    let Board { sources, groups, ongoing, template, next_question } = board(&table).map_err(fail)?;
    Ok(Machine { keys: keys(&table).map_err(fail)?, sources, groups, ongoing, template, next_question, channel: update(&table).map_err(fail)?, agent_updates: agent_updates(&table).map_err(fail)?, restart: restart(&table).map_err(fail)?, keeper: enabled_in(&table, "keeper").map_err(fail)?, worker: enabled_in(&table, "worker").map_err(fail)?, link_networks: link_networks(&table).map_err(fail)?, recover: enabled_in(&table, "recover").map_err(fail)?, machine_name: machine_name(&table).map_err(fail)?, hub_url: hub_url(&table).map_err(fail)?, hub_commands: hub_commands(&table).map_err(fail)?, hub_trust: hub_choice(&table, "trust", &HubTrust::ALL.map(|trust| (trust.key(), trust))).map_err(fail)?, hub_auto_approve: hub_choice(&table, "auto_approve", &AutoApprove::ALL.map(|auto| (auto.key(), auto))).map_err(fail)?, hub_approve_from: hub_choice(&table, "approve_from", &ApproveFrom::ALL.map(|from| (from.key(), from))).map_err(fail)?, report: report(&table).map_err(fail)?, propose_issues: propose_issues(&table).map_err(fail)?, lead_profile: lead_profile(&table).map_err(fail)?, writer_profile: writer_profile(&table).map_err(fail)?, ideas_machine: ideas_machine(&table).map_err(fail)?, builds: builds(&table).map_err(fail)?, disk: disk(&table).map_err(fail)?, worktree })
}
