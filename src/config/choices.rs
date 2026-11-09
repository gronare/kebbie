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
