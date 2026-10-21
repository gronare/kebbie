pub(crate) mod claude;

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::profiles::Profile;
use crate::sh::Result;
use crate::tasks::Session;

pub(crate) use claude::{Started, wait_for_prompt};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Agent {
    Claude,
}

const COMING: [&str; 1] = ["codex"];

pub struct Reviewer<'a> {
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub settings: &'a Path,
    pub prompt: &'a Path,
    pub rules: Option<&'a Path>,
}

impl Agent {
    pub const ALL: [Agent; 1] = [Agent::Claude];

    pub fn key(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
        }
    }

    pub fn named(name: &str) -> Result<Agent> {
        if let Some(agent) = Agent::ALL.into_iter().find(|agent| agent.key() == name) {
            return Ok(agent);
        }
        let known = Agent::ALL.map(|agent| format!("\"{}\"", agent.key())).join(", ");
        if COMING.contains(&name) {
            return Err(format!("\"{name}\" is not supported yet; kelpie runs {known}"));
        }
        Err(format!("\"{name}\" is not an agent kelpie runs; use {known}"))
    }

    // The kind herdr starts it as and reports for its pane.
    pub fn kind(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
        }
    }

    pub fn of(herdr_agent: &Value) -> Option<Agent> {
        Agent::ALL.into_iter().find(|agent| herdr_agent["agent"] == agent.kind())
    }

    pub fn task_settings(self, kelpie: &str, address: &str, gate: Option<&str>, tmp: Option<&Path>, profile: &Profile) -> String {
        match self {
            Agent::Claude => claude::with_profile(&claude::settings(kelpie, address, gate, tmp), profile),
        }
    }

    pub fn start_args(self, mode: Option<&str>, profile: &Profile, settings: &Path, role: &Path, session: &Session) -> Vec<String> {
        match self {
            Agent::Claude => claude::start_args(mode, profile, settings, role, session),
        }
    }

    pub fn start(self, name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<Started> {
        match self {
            Agent::Claude => claude::start_claude(name, pane, args, trusted),
        }
    }

    pub fn trusted_by_user(self, dir: &Path) -> bool {
        match self {
            Agent::Claude => claude::trusted_by_user(dir),
        }
    }

    pub fn end(self, pane: &str) -> Result<()> {
        match self {
            Agent::Claude => claude::end_claude(pane),
        }
    }

    pub fn home(self) -> Option<PathBuf> {
        match self {
            Agent::Claude => claude::home(),
        }
    }

    pub fn transcripts_dir(self, home: &Path, folder: &Path) -> PathBuf {
        match self {
            Agent::Claude => claude::transcripts_dir(home, folder),
        }
    }

    pub fn user_messages(self, transcript: &str) -> Vec<String> {
        match self {
            Agent::Claude => claude::user_messages(transcript),
        }
    }

    pub fn user_messages_at(self, transcript: &str) -> Vec<(Option<u64>, String)> {
        match self {
            Agent::Claude => claude::user_messages_at(transcript),
        }
    }

    pub fn resume_args(self, argv: &[String], session: &str) -> std::result::Result<Vec<String>, String> {
        match self {
            Agent::Claude => claude::resume_args(argv, session),
        }
    }

    pub fn lacks_role(self, argv: &[String]) -> bool {
        match self {
            Agent::Claude => claude::lacks_role(argv),
        }
    }

    pub fn with_role(self, args: &[String], settings: &Path, role: &Path) -> Vec<String> {
        match self {
            Agent::Claude => claude::with_role(args, settings, role),
        }
    }

    pub fn reviewer_command(self, reviewer: &Reviewer) -> (&'static str, Vec<String>) {
        match self {
            Agent::Claude => ("claude", claude::reviewer_args(reviewer)),
        }
    }

    pub fn reviewer_settings(self, kelpie: &str) -> String {
        match self {
            Agent::Claude => claude::reviewer_settings(kelpie),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_is_named_by_its_key_and_codex_is_refused_as_not_yet_supported() {
        assert_eq!(Agent::named("claude"), Ok(Agent::Claude));
        assert_eq!(Agent::named("codex").unwrap_err(), "\"codex\" is not supported yet; kelpie runs \"claude\"");
        assert_eq!(Agent::named("vim").unwrap_err(), "\"vim\" is not an agent kelpie runs; use \"claude\"");
        assert_eq!(Agent::of(&serde_json::json!({ "agent": "claude" })), Some(Agent::Claude));
        assert_eq!(Agent::of(&serde_json::json!({ "agent": "codex" })), None, "a pane of an agent kelpie has no adapter for is not kelpie's");
    }
}
