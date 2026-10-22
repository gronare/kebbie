pub(crate) mod claude;
pub(crate) mod codex;

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::profiles::Profile;
use crate::sh::Result;
use crate::tasks::Session;

pub(crate) use claude::{Started, wait_for_prompt};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Agent {
    Claude,
    Codex,
}

pub struct TaskFiles<'a> {
    pub address: &'a str,
    pub settings: &'a Path,
    pub role: &'a Path,
    pub sandboxed: bool,
    pub tmp: Option<&'a Path>,
}

pub struct Reviewer<'a> {
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub settings: &'a Path,
    pub prompt: &'a Path,
    pub rules: Option<&'a Path>,
}

impl Agent {
    pub const ALL: [Agent; 2] = [Agent::Claude, Agent::Codex];

    pub fn key(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
        }
    }

    pub fn named(name: &str) -> Result<Agent> {
        if let Some(agent) = Agent::ALL.into_iter().find(|agent| agent.key() == name) {
            return Ok(agent);
        }
        let known = Agent::ALL.map(|agent| format!("\"{}\"", agent.key())).join(", ");
        Err(format!("\"{name}\" is not an agent kelpie runs; use {known}"))
    }

    // The kind herdr starts it as and reports for its pane.
    pub fn kind(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    pub fn of(herdr_agent: &Value) -> Option<Agent> {
        Agent::ALL.into_iter().find(|agent| herdr_agent["agent"] == agent.kind())
    }

    pub fn task_settings(self, kelpie: &str, address: &str, gate: Option<&str>, dirs: Option<&crate::sandbox::TaskDirs>, profile: &Profile, sandbox: bool) -> String {
        match self {
            Agent::Claude => claude::with_profile(&crate::inbox::with_inbox(&claude::settings(kelpie, address, gate, dirs, sandbox.then_some(&profile.sandbox)), kelpie, address), profile),
            Agent::Codex => serde_json::json!({ "agent": "codex" }).to_string(),
        }
    }

    pub fn start_args(self, mode: Option<&str>, profile: &Profile, files: &TaskFiles, session: &Session) -> Vec<String> {
        match self {
            Agent::Claude => claude::start_args(mode, profile, files.settings, files.role, session),
            Agent::Codex => codex::start_args(mode, profile, files.address, files.sandboxed, files.tmp, session),
        }
    }

    pub fn start(self, name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<Started> {
        match self {
            Agent::Claude => claude::start_claude(name, pane, args, trusted),
            Agent::Codex => codex::start(name, pane, args, trusted),
        }
    }

    pub fn trusted_by_user(self, dir: &Path) -> bool {
        match self {
            Agent::Claude => claude::trusted_by_user(dir),
            Agent::Codex => codex::trusted_by_user(dir),
        }
    }

    pub fn end(self, pane: &str) -> Result<()> {
        match self {
            Agent::Claude => claude::end_claude(pane),
            Agent::Codex => codex::end(pane),
        }
    }

    pub fn home(self) -> Option<PathBuf> {
        match self {
            Agent::Claude => claude::home(),
            Agent::Codex => codex::home(),
        }
    }

    pub fn transcripts_dir(self, home: &Path, folder: &Path) -> PathBuf {
        match self {
            Agent::Claude => claude::transcripts_dir(home, folder),
            Agent::Codex => home.join("sessions"),
        }
    }

    // The conversation file of this session, looked for first under the folder it ran in.
    pub fn session_transcript(self, home: &Path, folder: Option<&Path>, session: &str) -> Option<PathBuf> {
        match self {
            Agent::Claude => crate::restart::transcript(home, folder, session),
            Agent::Codex => codex::transcript(home, session),
        }
    }

    // The newest conversation the agent began in this folder since a time, as its session id.
    pub fn latest_session(self, home: &Path, folder: &Path, since: std::time::SystemTime) -> Option<String> {
        match self {
            Agent::Claude => crate::tasks::latest_transcript(&claude::transcripts_dir(home, folder), since),
            Agent::Codex => codex::latest_session(home, folder, since),
        }
    }

    pub fn draft(self, screen: &str) -> crate::typing::Draft {
        match self {
            Agent::Claude => crate::typing::draft(screen),
            Agent::Codex => codex::draft(screen),
        }
    }

    pub fn user_messages(self, transcript: &str) -> Vec<String> {
        match self {
            Agent::Claude => claude::user_messages(transcript),
            Agent::Codex => codex::user_messages_at(transcript).into_iter().map(|(_, text)| text).collect(),
        }
    }

    pub fn user_messages_at(self, transcript: &str) -> Vec<(Option<u64>, String)> {
        match self {
            Agent::Claude => claude::user_messages_at(transcript),
            Agent::Codex => codex::user_messages_at(transcript),
        }
    }

    pub fn resume_args(self, argv: &[String], session: &str) -> std::result::Result<Vec<String>, String> {
        match self {
            Agent::Claude => claude::resume_args(argv, session),
            Agent::Codex => codex::resume_args(argv, session),
        }
    }

    pub fn lacks_role(self, argv: &[String]) -> bool {
        match self {
            Agent::Claude => claude::lacks_role(argv),
            Agent::Codex => codex::lacks_role(argv),
        }
    }

    pub fn with_role(self, args: &[String], settings: &Path, role: &Path) -> Vec<String> {
        match self {
            Agent::Claude => claude::with_role(args, settings, role),
            Agent::Codex => codex::with_role(args),
        }
    }

    // kelpie review runs Claude whatever the reviewer profile says; the profile refuses agent = "codex".
    pub fn reviewer_command(self, reviewer: &Reviewer) -> (&'static str, Vec<String>) {
        ("claude", claude::reviewer_args(reviewer))
    }

    pub fn reviewer_settings(self, kelpie: &str, sandbox: bool) -> String {
        claude::reviewer_settings(kelpie, sandbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_is_named_by_its_key_and_its_pane_by_the_kind_herdr_reports() {
        assert_eq!(Agent::named("claude"), Ok(Agent::Claude));
        assert_eq!(Agent::named("codex"), Ok(Agent::Codex));
        assert_eq!(Agent::named("vim").unwrap_err(), "\"vim\" is not an agent kelpie runs; use \"claude\", \"codex\"");
        assert_eq!(Agent::of(&serde_json::json!({ "agent": "claude" })), Some(Agent::Claude));
        assert_eq!(Agent::of(&serde_json::json!({ "agent": "codex" })), Some(Agent::Codex));
        assert_eq!(Agent::of(&serde_json::json!({ "agent": "gemini" })), None, "a pane of an agent kelpie has no adapter for is not kelpie's");
        assert_eq!(Agent::Codex.kind(), "codex", "herdr agent start --kind codex");
    }
}
