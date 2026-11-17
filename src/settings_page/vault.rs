use std::time::Duration;

use ratatui::text::{Line, Span};

use super::{Effect, Page};
use crate::board::widgets::{GREEN, MUTED, PEACH, RED, SUBTEXT, TEXT, style};
use crate::sh::Result;
use crate::vault_cli::Probe;

pub(super) const SECTION: &str = "Vault";
const KEY: &str = "vault";
const WAIT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub enum Seen {
    NoCommand,
    Answered { command: Vec<String>, probe: std::result::Result<Probe, String> },
}

pub(super) fn read() -> Seen {
    match crate::vault_cli::cli() {
        Some(cli) => Seen::Answered { probe: cli.probe(false), command: cli.command },
        None => Seen::NoCommand,
    }
}

fn lives(probe: &Probe) -> String {
    match probe.backend.as_str() {
        "server" => format!("a server, {}", probe.server),
        "local" => "a folder on this machine".to_string(),
        "s3" => "an S3 bucket".to_string(),
        _ => "nowhere yet: s chooses where".to_string(),
    }
}

fn signed(probe: &Probe) -> (String, ratatui::style::Color) {
    match (probe.backend.as_str(), probe.configured) {
        ("server", true) => ("signed in".to_string(), GREEN),
        ("server", false) => ("not signed in: s signs this machine in".to_string(), PEACH),
        (_, true) => ("set up, no sign-in needed".to_string(), GREEN),
        (_, false) => ("not set up".to_string(), PEACH),
    }
}

pub(super) fn lines(seen: Option<&Seen>) -> Vec<Line<'static>> {
    let row = |label: &str, value: String, color| Line::from(vec![Span::styled(format!("  {label:<10}"), style(SUBTEXT)), Span::styled(value, style(color))]);
    let mut lines = match seen {
        None => vec![row("vault", "reading what the vault says…".to_string(), MUTED)],
        Some(Seen::NoCommand) => vec![row("vault", "no vault command on this machine: s runs mindin setup".to_string(), PEACH)],
        Some(Seen::Answered { command, probe: Err(error) }) => vec![row("runs", command.join(" "), TEXT), row("vault", format!("did not answer: {error}"), RED)],
        Some(Seen::Answered { command, probe: Ok(probe) }) => {
            let (said, color) = signed(probe);
            let name = if probe.name.is_empty() { "vault" } else { probe.name.as_str() };
            vec![row("lives in", lives(probe), TEXT), row("sign-in", said, color), row("version", format!("{name} {}", probe.version), TEXT), row("runs", command.join(" "), MUTED)]
        }
    };
    lines.push(Line::from(Span::styled("  s runs mindin setup, which asks where the vault lives".to_string(), style(MUTED))));
    lines.push(Line::default());
    lines
}

// mindin setup moves the vault; a vault command without setup, as notes-vault-mcp up to 0.13, leaves it to an installed mindin.
pub(super) fn setup_command(seen: Option<&Seen>) -> Vec<String> {
    match seen {
        Some(Seen::Answered { command, probe: Ok(probe) }) if probe.setup => command.iter().cloned().chain(["setup".to_string()]).collect(),
        _ => vec!["mindin".to_string(), "setup".to_string()],
    }
}

impl Page {
    pub(super) fn on_vault(&self) -> bool {
        self.filter_text().is_empty() && self.sections().get(self.section).is_some_and(|(name, _)| name == SECTION)
    }

    pub(super) fn vault_seen(&mut self) -> Option<Seen> {
        self.vault.get(KEY, WAIT, self.vault_read)
    }

    pub fn spawn_reads(&mut self) {
        self.vault.spawn();
    }

    pub fn take_reads(&mut self) -> bool {
        self.vault.take()
    }

    pub(super) fn vault_setup(&mut self) -> Effect {
        let seen = self.vault_seen();
        Effect::Run(setup_command(seen.as_ref()))
    }

    pub fn ran(&mut self, command: &[String], result: Result<()>) {
        self.vault.forget(KEY);
        match result {
            Ok(()) => self.say(format!("✓ {} done; reading the vault again", command.join(" ")), GREEN),
            Err(error) => self.say(format!("✗ {}: {error}", command.join(" ")), RED),
        }
        self.load();
    }
}

pub fn run(command: &[String]) -> Result<()> {
    let (program, args) = command.split_first().ok_or("no command to run")?;
    crate::sh::run_visibly(program, &args.iter().map(String::as_str).collect::<Vec<_>>(), None)
}
