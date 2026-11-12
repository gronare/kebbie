use std::path::{Path, PathBuf};

use super::{group_order, load_machine, load_repo, path, repo_file, short_hostname, AgentUpdates, AlsoNoticed, ApproveFrom, AutoApprove, Channel, HubCommands, HubTrust, Machine, OngoingAt, ReportText, ReviewAuto, Second, Source, read_sources, Worktree, DEFAULT_HUB_URL, DEFAULT_IDLE_MINUTES, DEFAULT_KEEPER, DEFAULT_PROPOSE_ISSUES, DEFAULT_RECOVER, NONE};
use crate::sh::Result;
use crate::status::Group;

pub const DEFAULT_SETUP_SKILL: &str = "kebbie:set-up-worktree";
pub const DEFAULT_RULES_SKILL: &str = "kebbie:work-rules";
pub const DEFAULT_DEV_URL: &str = "http://localhost:{port}";
pub const DEFAULT_SERVER_NAME: &str = "server";
pub const DEFAULT_WORKER_NAME: &str = "worker";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layer {
    Repo,
    Machine,
    Default,
}

impl Layer {
    pub fn label(self) -> &'static str {
        match self {
            Layer::Repo => "repo config",
            Layer::Machine => "machine config",
            Layer::Default => "default",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Setting<T> {
    pub value: T,
    pub layer: Layer,
}

impl<T> Setting<T> {
    fn map<U>(self, change: impl FnOnce(T) -> U) -> Setting<U> {
        Setting { value: change(self.value), layer: self.layer }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub sources: Setting<Vec<Source>>,
    pub groups: Setting<Vec<Group>>,
    pub ongoing: Setting<OngoingAt>,
    pub setup_skill: Setting<String>,
    pub setup_command: Setting<Option<String>>,
    pub dev_server: Setting<Option<String>>,
    pub dev_worker: Setting<Option<String>>,
    pub dev_url: Setting<Option<String>>,
    pub dev_route: Setting<bool>,
    pub stage_on_main: Setting<bool>,
    pub rules_skill: Setting<String>,
    pub test_commands: Setting<Vec<String>>,
    pub also_noticed: Setting<AlsoNoticed>,
    pub permission_mode: Setting<Option<String>>,
    pub profile: Setting<Option<String>>,
    pub sandbox: Setting<bool>,
    pub steps: Setting<bool>,
    pub require_accept: Setting<bool>,
    pub review_model: Setting<Option<String>>,
    pub review_auto: Setting<ReviewAuto>,
    pub review_profile: Setting<String>,
    pub review_second: Setting<Second>,
    pub notify_command: Setting<Option<String>>,
    pub vault_command: Setting<Option<String>>,
    pub server_name: Setting<String>,
    pub worker_name: Setting<String>,
    pub channel: Setting<Channel>,
    pub agent_updates: Setting<AgentUpdates>,
    pub idle_minutes: Setting<u64>,
    pub auto_restart: Setting<bool>,
    pub ci_watch: Setting<bool>,
    pub comments_watch: Setting<bool>,
    pub comments_days: Setting<u64>,
    pub clean_on_release: Setting<Vec<String>>,
    pub docs_required: Setting<bool>,
    pub keeper: Setting<bool>,
    pub recover: Setting<bool>,
    pub machine_name: Setting<String>,
    pub hub_url: Setting<String>,
    pub hub_commands: Setting<HubCommands>,
    pub hub_trust: Setting<HubTrust>,
    pub hub_auto_approve: Setting<AutoApprove>,
    pub hub_approve_from: Setting<ApproveFrom>,
    pub report_signals: Setting<bool>,
    pub report_text: Setting<ReportText>,
    pub report_inbox: Setting<bool>,
    pub propose_issues: Setting<bool>,
}

pub struct Loaded {
    pub machine: PathBuf,
    pub repo: Option<PathBuf>,
    pub settings: Settings,
}

fn pick<T: Clone>(repo: Option<&T>, machine: Option<&T>) -> Option<Setting<T>> {
    match (repo, machine) {
        (Some(value), _) => Some(Setting { value: value.clone(), layer: Layer::Repo }),
        (None, Some(value)) => Some(Setting { value: value.clone(), layer: Layer::Machine }),
        (None, None) => None,
    }
}

fn or_default<T>(setting: Option<Setting<T>>, value: T) -> Setting<T> {
    setting.unwrap_or(Setting { value, layer: Layer::Default })
}

pub fn settings(repo: &Worktree, machine: &Machine) -> Settings {
    let optional = |setting: Option<Setting<String>>| or_default(setting.map(|setting| Setting { value: Some(setting.value), layer: setting.layer }), None);
    Settings {
        sources: or_default(pick(None, machine.sources.as_ref()).map(|setting| Setting { value: read_sources(&setting.value), layer: setting.layer }), vec![Source::Local]),
        groups: pick(None, machine.groups.as_ref()).map_or(Setting { value: Group::ALL.to_vec(), layer: Layer::Default }, |setting| Setting { value: group_order(&setting.value), layer: setting.layer }),
        ongoing: or_default(pick(None, machine.ongoing.as_ref()), OngoingAt::default()),
        setup_skill: or_default(pick(repo.setup_skill.as_ref(), machine.worktree.setup_skill.as_ref()), DEFAULT_SETUP_SKILL.to_string()),
        setup_command: optional(pick(repo.setup_command.as_ref(), machine.worktree.setup_command.as_ref())),
        dev_server: optional(pick(repo.dev_server.as_ref(), machine.worktree.dev_server.as_ref())),
        dev_worker: optional(pick(repo.dev_worker.as_ref(), machine.worktree.dev_worker.as_ref())),
        dev_url: or_default(pick(repo.dev_url.as_ref(), machine.worktree.dev_url.as_ref())
            .map(|setting| Setting { value: Some(setting.value).filter(|url| !url.is_empty()), layer: setting.layer }), Some(DEFAULT_DEV_URL.to_string())),
        dev_route: or_default(pick(repo.dev_route.as_ref(), None), false),
        stage_on_main: or_default(pick(repo.stage_on_main.as_ref(), None), false),
        rules_skill: or_default(pick(repo.rules_skill.as_ref(), machine.worktree.rules_skill.as_ref()), DEFAULT_RULES_SKILL.to_string()),
        test_commands: or_default(pick(repo.test_commands.as_ref(), machine.worktree.test_commands.as_ref()), Vec::new()),
        also_noticed: or_default(pick(repo.also_noticed.as_ref(), machine.worktree.also_noticed.as_ref()), AlsoNoticed::default()),
        permission_mode: optional(pick(repo.permission_mode.as_ref(), machine.worktree.permission_mode.as_ref())),
        profile: optional(pick(repo.profile.as_ref(), machine.worktree.profile.as_ref())).map(|profile| profile.filter(|name| name != NONE)),
        sandbox: or_default(pick(repo.sandbox.as_ref(), machine.worktree.sandbox.as_ref()), true),
        steps: or_default(pick(repo.steps.as_ref(), machine.worktree.steps.as_ref()), true),
        require_accept: or_default(pick(repo.require_accept.as_ref(), machine.worktree.require_accept.as_ref()), false),
        review_model: optional(pick(repo.review_model.as_ref(), machine.worktree.review_model.as_ref())),
        review_auto: or_default(pick(repo.review_auto.as_ref(), machine.worktree.review_auto.as_ref()), ReviewAuto::default()),
        review_profile: or_default(pick(repo.review_profile.as_ref(), machine.worktree.review_profile.as_ref()), crate::profiles::REVIEWER.to_string())
            .map(|name| if name == NONE { crate::profiles::REVIEWER.to_string() } else { name }),
        review_second: or_default(pick(repo.review_second.as_ref(), machine.worktree.review_second.as_ref()), Second::default()),
        notify_command: optional(pick(None, machine.worktree.notify_command.as_ref())),
        vault_command: optional(pick(None, machine.worktree.vault_command.as_ref())),
        server_name: or_default(pick(None, machine.worktree.server_name.as_ref()), DEFAULT_SERVER_NAME.to_string()),
        worker_name: or_default(pick(None, machine.worktree.worker_name.as_ref()), DEFAULT_WORKER_NAME.to_string()),
        channel: or_default(pick(None, machine.channel.as_ref()), Channel::default()),
        agent_updates: or_default(pick(None, machine.agent_updates.as_ref()), AgentUpdates::default()),
        idle_minutes: or_default(pick(None, machine.restart.idle_minutes.as_ref()), DEFAULT_IDLE_MINUTES),
        auto_restart: or_default(pick(None, machine.restart.auto.as_ref()), true),
        ci_watch: or_default(pick(repo.ci_watch.as_ref(), machine.worktree.ci_watch.as_ref()), true),
        comments_watch: or_default(pick(repo.comments_watch.as_ref(), machine.worktree.comments_watch.as_ref()), true),
        comments_days: or_default(pick(repo.comments_days.as_ref(), machine.worktree.comments_days.as_ref()), crate::comments::DEFAULT_DAYS),
        clean_on_release: or_default(pick(repo.clean_on_release.as_ref(), machine.worktree.clean_on_release.as_ref()), vec!["target".to_string()]),
        docs_required: or_default(pick(repo.docs_required.as_ref(), None), false),
        keeper: or_default(pick(None, machine.keeper.as_ref()), DEFAULT_KEEPER),
        recover: or_default(pick(None, machine.recover.as_ref()), DEFAULT_RECOVER),
        machine_name: pick(None, machine.machine_name.as_ref()).unwrap_or_else(|| Setting { value: short_hostname(), layer: Layer::Default }),
        hub_url: or_default(pick(None, machine.hub_url.as_ref()), DEFAULT_HUB_URL.to_string()),
        hub_commands: or_default(pick(None, machine.hub_commands.as_ref()), HubCommands::default()),
        hub_trust: or_default(pick(None, machine.hub_trust.as_ref()), HubTrust::default()),
        hub_auto_approve: or_default(pick(None, machine.hub_auto_approve.as_ref()), AutoApprove::default()),
        hub_approve_from: or_default(pick(None, machine.hub_approve_from.as_ref()), ApproveFrom::default()),
        report_signals: or_default(pick(None, machine.report.signals.as_ref().or((machine.report.enabled == Some(false)).then_some(&false))), false),
        report_text: or_default(pick(None, machine.report.text.as_ref().or((machine.report.enabled == Some(false)).then_some(&ReportText::Never))), ReportText::Ask),
        report_inbox: or_default(pick(None, machine.report.inbox_token.as_ref().map(|_| &true)), false),
        propose_issues: or_default(pick(None, machine.propose_issues.as_ref()), DEFAULT_PROPOSE_ISSUES),
    }
}

// A command naming a path applies only in a repo that has that file, so one machine-wide value leaves other repos on the default setup.
pub fn setup_command_in(command: Option<String>, repo: &Path) -> Option<String> {
    command.filter(|command| {
        let program = command.split_whitespace().next().unwrap_or(command);
        !program.contains('/') || repo.join(program).exists()
    })
}

pub fn resolve(repo: Option<&Path>) -> Result<Loaded> {
    resolve_file(repo.map(repo_file))
}

pub fn resolve_file(repo: Option<PathBuf>) -> Result<Loaded> {
    resolve_at(path(), repo)
}

pub fn resolve_at(machine: PathBuf, repo: Option<PathBuf>) -> Result<Loaded> {
    let own = repo.as_deref().map(load_repo).transpose()?.unwrap_or_default();
    Ok(Loaded { settings: settings(&own, &load_machine(&machine)?), machine, repo })
}

pub fn sources() -> Result<Vec<Source>> {
    Ok(resolve(None)?.settings.sources.value)
}

pub fn board_layout() -> Result<(Vec<Group>, OngoingAt)> {
    let settings = resolve(None)?.settings;
    Ok((settings.groups.value, settings.ongoing.value))
}

pub fn restart_settings() -> Result<(bool, u64)> {
    let settings = resolve(None)?.settings;
    Ok((settings.auto_restart.value, settings.idle_minutes.value))
}
