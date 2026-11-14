use crate::sh::Result;
use crate::status::Group;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    Local,
    Vault,
    Linear,
}

impl Source {
    pub const ALL: [Source; 3] = [Source::Local, Source::Vault, Source::Linear];

    pub fn key(self) -> &'static str {
        match self {
            Source::Local => "local",
            Source::Vault => "vault",
            Source::Linear => "linear",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Source::Local => "local",
            Source::Vault => "vault",
            Source::Linear => "Linear",
        }
    }
}

// Where new backlog items and task notes are written: the vault when it is listed, else the local folder; Linear is only read.
pub fn filing(sources: &[Source]) -> Option<Source> {
    [Source::Vault, Source::Local].into_iter().find(|source| sources.contains(source))
}

pub fn mixes_local_and_vault(sources: &[Source]) -> bool {
    sources.contains(&Source::Local) && sources.contains(&Source::Vault)
}

// A config written before the two were exclusive lists both: it reads as the vault alone.
pub fn read_sources(listed: &[Source]) -> Vec<Source> {
    listed.iter().copied().filter(|source| *source != Source::Local || !mixes_local_and_vault(listed)).collect()
}

pub const MIXED_SOURCES: &str = "\"local\" and \"vault\" are two places to file the same items: list one of them (Linear can sit beside either)";

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Channel {
    Main,
    #[default]
    Release,
}

impl Channel {
    pub fn key(self) -> &'static str {
        match self {
            Channel::Main => "main",
            Channel::Release => "release",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AgentUpdates {
    #[default]
    Ask,
    Auto,
    Notify,
}

impl AgentUpdates {
    pub const ALL: [AgentUpdates; 3] = [AgentUpdates::Ask, AgentUpdates::Auto, AgentUpdates::Notify];

    pub fn key(self) -> &'static str {
        match self {
            AgentUpdates::Ask => "ask",
            AgentUpdates::Auto => "auto",
            AgentUpdates::Notify => "notify",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HubCommand {
    Answer,
    Push,
    Prompt,
    Idea,
    Start,
    Edit,
    Review,
}

impl HubCommand {
    pub const ALL: [HubCommand; 7] = [HubCommand::Answer, HubCommand::Push, HubCommand::Prompt, HubCommand::Idea, HubCommand::Start, HubCommand::Edit, HubCommand::Review];

    pub fn key(self) -> &'static str {
        match self {
            HubCommand::Answer => "answer",
            HubCommand::Push => "push",
            HubCommand::Prompt => "prompt",
            HubCommand::Idea => "idea",
            HubCommand::Start => "start",
            HubCommand::Edit => "edit",
            HubCommand::Review => "review",
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

// The kinds of command this machine takes from the hub; the three single words of earlier versions still read as the sets they named.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HubCommands(u8);

impl Default for HubCommands {
    fn default() -> Self {
        HubCommands::of(&HubCommand::ALL)
    }
}

impl HubCommands {
    pub const OFF: HubCommands = HubCommands(0);

    pub fn of(commands: &[HubCommand]) -> HubCommands {
        HubCommands(commands.iter().fold(0, |bits, command| bits | command.bit()))
    }

    pub fn takes(self, command: HubCommand) -> bool {
        self.0 & command.bit() != 0
    }

    pub fn taken(self) -> Vec<HubCommand> {
        HubCommand::ALL.into_iter().filter(|command| self.takes(*command)).collect()
    }

    pub fn keys(self) -> Vec<&'static str> {
        self.taken().into_iter().map(HubCommand::key).collect()
    }

    pub fn from_word(word: &str) -> Option<HubCommands> {
        match word {
            "answer-and-push" => Some(HubCommands::of(&[HubCommand::Answer, HubCommand::Push, HubCommand::Prompt])),
            "answer" => Some(HubCommands::of(&[HubCommand::Answer, HubCommand::Prompt])),
            "off" => Some(HubCommands::OFF),
            _ => None,
        }
    }

    pub fn from_keys(keys: &[String]) -> std::result::Result<HubCommands, String> {
        keys.iter().map(|key| HubCommand::ALL.into_iter().find(|command| command.key() == key).ok_or_else(|| key.clone())).collect::<std::result::Result<Vec<_>, _>>().map(|commands| HubCommands::of(&commands))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum HubTrust {
    #[default]
    Device,
    Account,
}

impl HubTrust {
    pub const ALL: [HubTrust; 2] = [HubTrust::Device, HubTrust::Account];

    pub fn key(self) -> &'static str {
        match self {
            HubTrust::Device => "device",
            HubTrust::Account => "account",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AutoApprove {
    #[default]
    Day,
    Never,
}

impl AutoApprove {
    pub const ALL: [AutoApprove; 2] = [AutoApprove::Day, AutoApprove::Never];

    pub fn key(self) -> &'static str {
        match self {
            AutoApprove::Day => "24h",
            AutoApprove::Never => "never",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ApproveFrom {
    #[default]
    Devices,
    Machine,
}

impl ApproveFrom {
    pub const ALL: [ApproveFrom; 2] = [ApproveFrom::Devices, ApproveFrom::Machine];

    pub fn key(self) -> &'static str {
        match self {
            ApproveFrom::Devices => "devices",
            ApproveFrom::Machine => "machine",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AlsoNoticed {
    #[default]
    Backlog,
    Fix,
    Ask,
}

impl AlsoNoticed {
    pub const ALL: [AlsoNoticed; 3] = [AlsoNoticed::Backlog, AlsoNoticed::Fix, AlsoNoticed::Ask];

    pub fn key(self) -> &'static str {
        match self {
            AlsoNoticed::Backlog => "backlog",
            AlsoNoticed::Fix => "fix",
            AlsoNoticed::Ask => "ask",
        }
    }
}

// What kebbie does with work the assessor says needs a review: suggest it, start the reviewer, or also hold the push until it passes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ReviewAuto {
    #[default]
    Suggest,
    Run,
    Require,
}

impl ReviewAuto {
    pub const ALL: [ReviewAuto; 3] = [ReviewAuto::Suggest, ReviewAuto::Run, ReviewAuto::Require];

    pub fn key(self) -> &'static str {
        match self {
            ReviewAuto::Suggest => "suggest",
            ReviewAuto::Run => "run",
            ReviewAuto::Require => "require",
        }
    }
}

// tests.before_push: a ready report and a push wait on the task's own related tests, or on a green run of the repo's full suite on HEAD.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum BeforePush {
    #[default]
    Related,
    Full,
}

impl BeforePush {
    pub const ALL: [BeforePush; 2] = [BeforePush::Related, BeforePush::Full];

    pub fn key(self) -> &'static str {
        match self {
            BeforePush::Related => "related",
            BeforePush::Full => "full",
        }
    }
}

// review.second: no second opinion, one offered when a task is sent to review, or one that runs with every review.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Second {
    #[default]
    None,
    Offered(String),
    Always(String),
}

pub const ALWAYS: &str = "always";

// The word a profile setting takes for its default at that layer, over a lower layer's profile.
pub const NONE: &str = "none";

impl Second {
    pub fn profile(&self) -> Option<&str> {
        match self {
            Second::None => None,
            Second::Offered(name) | Second::Always(name) => Some(name),
        }
    }

    pub fn always(&self) -> bool {
        matches!(self, Second::Always(_))
    }

    pub fn key(&self) -> String {
        match self {
            Second::None => NONE.to_string(),
            Second::Offered(name) => name.clone(),
            Second::Always(name) => format!("{ALWAYS} {name}"),
        }
    }

    // A profile's name, or "always" and the name; a profile name has no space, so the two never meet.
    pub fn parse(text: &str) -> Result<Second> {
        let (always, name) = match text.split_once(' ') {
            Some((ALWAYS, name)) => (true, name.trim()),
            Some(_) => return Err(format!("review.second: \"{text}\" is not a choice; name a review profile, or \"always <profile>\" to run it with every review")),
            None => (false, text),
        };
        match (name, always) {
            (NONE, false) => return Ok(Second::None),
            (NONE, true) => return Err(format!("review.second: \"{text}\" names no review profile to run; name one after \"always\", or set \"none\" for no second opinion")),
            _ => {}
        }
        crate::profiles::name(name).map_err(|error| format!("review.second: {error}"))?;
        Ok(if always { Second::Always(name.to_string()) } else { Second::Offered(name.to_string()) })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum OngoingAt {
    #[default]
    Top,
    Bottom,
    Hidden,
}

impl OngoingAt {
    pub const ALL: [OngoingAt; 3] = [OngoingAt::Top, OngoingAt::Bottom, OngoingAt::Hidden];

    pub fn key(self) -> &'static str {
        match self {
            OngoingAt::Top => "top",
            OngoingAt::Bottom => "bottom",
            OngoingAt::Hidden => "hidden",
        }
    }
}

pub fn group_order(listed: &[Group]) -> Vec<Group> {
    listed.iter().copied().chain(Group::ALL.into_iter().filter(|group| !listed.contains(group))).collect()
}

pub const PERMISSION_MODES: [&str; 6] = ["default", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions"];

pub fn permission_mode(name: &str) -> Result<&str> {
    PERMISSION_MODES.iter().find(|mode| **mode == name).map(|_| name)
        .ok_or(format!("\"{name}\" is not a Claude Code permission mode; use {}", PERMISSION_MODES.map(|mode| format!("\"{mode}\"")).join(", ")))
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ReportText {
    #[default]
    Ask,
    Always,
    Never,
}

impl ReportText {
    pub const ALL: [ReportText; 3] = [ReportText::Ask, ReportText::Always, ReportText::Never];

    pub fn key(self) -> &'static str {
        match self {
            ReportText::Ask => "ask",
            ReportText::Always => "always",
            ReportText::Never => "never",
        }
    }
}
