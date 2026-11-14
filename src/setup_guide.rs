use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph, Wrap};
use serde_json::Value;

use crate::board::widgets::{FAINT, GREEN, MANTLE, MAUVE, MUTED, RED, SUBTEXT, SURFACE, TEXT, bold, style};
use crate::config::{self, Source};
use crate::config_edit::{self, Set};
use crate::agent::Agent;
use crate::agent::codex::Installed;
use crate::agent::codex_setup::{self, Placed};
use crate::agent::login::{self, Login};
use crate::doctor::Repair;
use crate::setup::{self, Asking, KeyPlan, Machine, Outcome};
use crate::sh::Result;
use crate::vault_cli::{Plugin, Probe};
use crate::vault_found::{Found, Origin};

pub const MINDIN_INSTALL: &str = "curl -fsSL https://mindin.dev/install.sh | sh";

// The hosted mindin service's address; while it is None, Hosted mindin shows as coming soon and cannot be chosen.
pub const HOSTED_VAULT: Option<&str> = None;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    Hosted,
    Server,
    S3,
    Folder,
    None,
}

impl Place {
    pub const ALL: [Place; 5] = [Place::Hosted, Place::Server, Place::S3, Place::Folder, Place::None];

    fn label(self) -> &'static str {
        match self {
            Place::Hosted => "Hosted mindin",
            Place::Server => "Your own server",
            Place::S3 => "S3 bucket",
            Place::Folder => "Local folder",
            Place::None => "None",
        }
    }

    fn open(self) -> bool {
        self != Place::Hosted || HOSTED_VAULT.is_some()
    }

    fn fields(self) -> &'static [Field] {
        match self {
            Place::Server => &[Field::Address],
            Place::S3 => &[Field::Endpoint, Field::Bucket, Field::Prefix, Field::Region],
            Place::Folder => &[Field::Folder],
            Place::Hosted | Place::None => &[],
        }
    }

    fn needs(self) -> &'static str {
        match self {
            Place::Server => "its address",
            Place::S3 => "an endpoint and a bucket",
            Place::Folder => "a path",
            Place::Hosted | Place::None => "nothing",
        }
    }

    fn of(probe: &Probe) -> Option<Place> {
        match probe.backend.as_str() {
            "server" if HOSTED_VAULT.is_some_and(|hosted| same_url(&probe.server, hosted)) => Some(Place::Hosted),
            "server" => Some(Place::Server),
            "s3" => Some(Place::S3),
            "local" => Some(Place::Folder),
            _ => None,
        }
    }
}

fn same_url(one: &str, other: &str) -> bool {
    let bare = |url: &str| url.trim().trim_end_matches('/').to_ascii_lowercase();
    !one.trim().is_empty() && bare(one) == bare(other)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Field {
    Name,
    Address,
    Endpoint,
    Bucket,
    Prefix,
    Region,
    Folder,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Name => "name",
            Field::Address => "address",
            Field::Endpoint => "endpoint",
            Field::Bucket => "bucket",
            Field::Prefix => "prefix",
            Field::Region => "region",
            Field::Folder => "path",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Field::Address => "https://vault.example.com",
            Field::Endpoint => "the S3 or MinIO address",
            Field::Prefix => "optional",
            Field::Region => "optional, us-east-1 when empty",
            Field::Folder => "the vault's folder",
            Field::Name | Field::Bucket => "",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Step {
    Name,
    Agents,
    Vault,
    Backlog,
    Signals,
    Review,
}

impl Step {
    pub const ALL: [Step; 6] = [Step::Name, Step::Agents, Step::Vault, Step::Backlog, Step::Signals, Step::Review];

    fn index(self) -> usize {
        Step::ALL.iter().position(|step| *step == self).unwrap_or(0)
    }

    fn title(self) -> &'static str {
        match self {
            Step::Name => "This machine",
            Step::Agents => "Agents",
            Step::Vault => "Vault",
            Step::Backlog => "Backlog",
            Step::Signals => "Error signals",
            Step::Review => "Review and apply",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Step::Name => "How your phone and your other machines show this one. One word, like mac or devbox.",
            Step::Agents => "The agents kebbie found on this machine. Each one you tick gets kebbie's plugin and its approval rules when you apply. Claude Code runs the lead and the reviewer, so it stays on.",
            Step::Vault => "Optional. A note for every task and a log line for every push. Choose where the vault lives; when that changes, kebbie runs mindin setup for it as you apply.",
            Step::Backlog => "Where work waits and new items are written: the local file or the vault, one of them. Linear can sit beside either; the lead only reads it.",
            Step::Signals => "Off until you say yes. With them on, when kebbie itself crashes or hits some internal errors, it sends its maintainers an error signal, at most once a day. A signal holds only kebbie's version and commit, your OS and CPU architecture, the part of kebbie that failed, the kind of failure and a count; never your code, paths or text. Once a day it also sends an install signal, so they can count active installs: a random id made for this install, kebbie's version, your OS and CPU architecture and the update channel. Turning them off deletes that id.",
            Step::Review => "Everything kebbie writes or installs, each tool's own files included. Nothing is written until you press ⏎ here.",
        }
    }
}

pub struct Env<'a> {
    pub machine: &'a Machine,
    pub config: PathBuf,
    pub codex_home: Option<PathBuf>,
    pub kebbie: String,
    pub state: PathBuf,
}

impl<'a> Env<'a> {
    pub fn current(machine: &'a Machine) -> Result<Env<'a>> {
        Ok(Env { machine, config: config::path(), codex_home: crate::agent::codex::home(), kebbie: crate::peers::kebbie_command().unwrap_or_default(), state: crate::lead::state_dir()? })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    pub claude_bin: String,
    pub claude_dir: PathBuf,
    pub gronare: bool,
    pub name: String,
    pub name_set: bool,
    pub claude: Option<String>,
    pub kebbie_plugin: bool,
    pub codex: Option<String>,
    pub codex_bin: Option<PathBuf>,
    pub claude_login: Login,
    pub codex_login: Login,
    pub claude_stale: Option<String>,
    pub codex_plugin: Installed,
    pub keys: KeyPlan,
    pub sources: Vec<Source>,
    pub sources_set: bool,
    pub vault: Plugin,
    pub hub: Option<String>,
    pub signals: bool,
    pub signals_set: bool,
    pub repairs: Vec<Repair>,
    pub codex_home: Option<PathBuf>,
    pub rules: String,
    pub rules_placed: Placed,
    pub codex_config: Option<String>,
    pub vault_found: Vec<Found>,
    pub vault_command: Option<String>,
}

fn version(program: &str) -> Option<String> {
    let said = crate::sh::run(program, &["--version"], None).ok()?;
    said.split_whitespace().find(|word| word.starts_with(|character: char| character.is_ascii_digit())).map(str::to_string).or(Some(String::new()))
}

fn read_json(path: &Path) -> Value {
    std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

fn words(command: &str) -> Vec<String> {
    command.split_whitespace().map(str::to_string).collect()
}

impl Facts {
    pub fn read(env: &Env) -> Facts {
        let machine = config::check_machine(&env.config).map(|(machine, _)| machine).unwrap_or_default();
        let codex = crate::agent::codex::availability(&env.machine.path, env.codex_home.as_deref(), &env.kebbie);
        let herdr = crate::sh::run(&env.machine.herdr, &["plugin", "list", "--json", "--plugin", "kebbie"], None).ok();
        let channel = config::channel_in(&env.config).unwrap_or_default();
        let checkout = crate::doctor::checkout_version(crate::doctor::herdr_plugin(herdr.as_deref(), channel).1.as_ref());
        let claude_plugin = crate::doctor::claude_plugin(Some(&read_json(&env.machine.claude_dir.join("plugins/installed_plugins.json"))), &read_json(&env.machine.claude_dir.join("settings.json")), checkout.as_deref(), channel);
        let kebbie_plugin = setup::kebbie_plugin_installed(&env.machine.claude_dir);
        let rules = codex_setup::rules_text(&codex_setup::rule_forms(&env.kebbie, &env.machine.home.join(".local/bin/kebbie")));
        let cursor = env.machine.home.join(".cursor");
        let local_bin = env.machine.home.join(".local/bin");
        let places = crate::vault_found::Places { claude_dir: &env.machine.claude_dir, codex_home: env.codex_home.as_deref(), cursor_dir: &cursor, path: &env.machine.path, local_bin: &local_bin };
        let lookup: Vec<PathBuf> = env.machine.path.iter().cloned().chain([local_bin.clone()]).collect();
        let mut vault_found = crate::vault_found::candidates(&places, |command| crate::vault_found::probe_on(&lookup, command));
        let vault_command = machine.worktree.vault_command.clone().filter(|command| !command.trim().is_empty());
        if let Some(command) = vault_command.as_deref().map(words) {
            match vault_found.iter_mut().find(|found| found.command == command) {
                Some(found) if found.origin != Origin::Mindin => found.origin = Origin::Kept,
                Some(_) => {}
                None => vault_found.insert(0, Found { source: "kebbie's [vault] command".to_string(), answer: crate::vault_found::probe_on(&lookup, &command), command, origin: Origin::Kept }),
            }
        }
        let found = crate::doctor::Found {
            claude_settings: &read_json(&env.machine.claude_dir.join("settings.json")),
            keeper_on: machine.keeper.unwrap_or(true),
            keeper_running: crate::keeper::holder(&env.state).is_some(),
            herdr_list: herdr.as_deref(),
        };
        Facts {
            claude_bin: env.machine.claude.clone(),
            claude_dir: env.machine.claude_dir.clone(),
            gronare: setup::marketplace_known(&env.machine.claude_dir, "gronare"),
            name: machine.machine_name.clone().unwrap_or_else(config::short_hostname),
            name_set: machine.machine_name.is_some(),
            claude: version(&env.machine.claude),
            kebbie_plugin,
            claude_stale: (kebbie_plugin && claude_plugin.level != crate::doctor::Level::Ok).then_some(claude_plugin.fact),
            codex: codex.binary.as_ref().map(|_| codex.version.as_deref().and_then(|text| text.split_whitespace().find(|word| word.starts_with(|character: char| character.is_ascii_digit()))).unwrap_or_default().to_string()),
            codex_bin: codex.binary.clone(),
            claude_login: login::claude(crate::doctor::which(&env.machine.claude, &env.machine.path).as_deref(), login::TIMEOUT),
            codex_login: if codex.binary.is_none() { Login::Unknown } else { login::codex(codex.binary.as_deref(), login::TIMEOUT) },
            codex_plugin: codex.plugin.clone(),
            keys: setup::key_plan(&env.machine.herdr_config),
            sources: machine.sources.clone().unwrap_or_else(|| vec![Source::Local]),
            sources_set: machine.sources.is_some(),
            vault: crate::vault_cli::plugin(&env.machine.claude_dir),
            hub: crate::hub::identity::load(&env.state).ok().flatten().map(|identity| crate::hub::host(&identity.url).to_string()),
            signals: machine.report.signals(),
            signals_set: machine.report.signals.is_some(),
            repairs: crate::doctor::repairs(&found),
            rules_placed: env.codex_home.as_deref().map_or(Placed::Missing, |home| codex_setup::rules_placed(home, &rules)),
            rules,
            codex_config: env.codex_home.as_deref().and_then(|home| std::fs::read_to_string(home.join("config.toml")).ok()),
            codex_home: env.codex_home.clone(),
            vault_found,
            vault_command,
        }
    }

    // The one command kebbie runs the vault with: a mindin that answers, else kebbie's own, else the Claude plugin's, else any that answers.
    pub fn runner(&self) -> Option<&Found> {
        let first = |origin: Origin, answered: bool| self.vault_found.iter().find(|found| found.origin == origin && (!answered || found.answer.is_some()));
        first(Origin::Mindin, true).or_else(|| first(Origin::Kept, false)).or_else(|| first(Origin::Plugin, false)).or_else(|| first(Origin::Other, true))
    }

    fn runner_answer(&self) -> Option<&Probe> {
        self.runner().and_then(|found| found.answer.as_ref())
    }

    // Where the vault lives as far as it says: the runner's answer, else the first other command that is set up.
    fn known(&self) -> Option<&Probe> {
        self.runner_answer().filter(|probe| Place::of(probe).is_some()).or_else(|| self.vault_found.iter().filter_map(|found| found.answer.as_ref()).find(|probe| probe.configured && Place::of(probe).is_some()))
    }

    fn place_about(&self, place: Place) -> String {
        let known = self.known().filter(|probe| Place::of(probe) == Some(place));
        let signed = |probe: &Probe| if probe.configured { "signed in" } else { "not signed in" };
        match (place, known) {
            (Place::Hosted, _) if HOSTED_VAULT.is_none() => "coming soon".to_string(),
            (Place::Hosted, Some(probe)) => signed(probe).to_string(),
            (Place::Hosted, None) => "sign in through the browser".to_string(),
            (Place::Server, Some(probe)) => format!("{} · {}", probe.server, signed(probe)),
            (Place::Server, None) => "address".to_string(),
            (Place::S3, Some(_)) | (Place::Folder, Some(_)) => "in use".to_string(),
            (Place::S3, None) => "endpoint, bucket".to_string(),
            (Place::Folder, None) => "path".to_string(),
            (Place::None, _) => "kebbie keeps tasks without one".to_string(),
        }
    }

    fn agent_found(&self, agent: Agent) -> bool {
        match agent {
            Agent::Claude => self.claude.is_some(),
            Agent::Codex => self.codex.is_some(),
        }
    }
}

fn lives(found: &Found) -> String {
    let Some(probe) = &found.answer else { return "did not answer".to_string() };
    match probe.backend.as_str() {
        "server" if !probe.configured => format!("server {} · not signed in: run {} login", probe.server, found.shown()),
        "server" => format!("server {} · signed in", probe.server),
        "local" => "a local folder".to_string(),
        "s3" => "an S3 bucket".to_string(),
        _ if !probe.configured => format!("no vault set up yet: {} setup chooses one", found.shown()),
        backend => backend.to_string(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Found(Vec<String>),
    Installed,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Where {
    pub place: Option<Place>,
    pub address: String,
    pub endpoint: String,
    pub bucket: String,
    pub prefix: String,
    pub region: String,
    pub folder: String,
}

impl Where {
    fn from(facts: &Facts) -> Where {
        let known = facts.known();
        let unknown = if HOSTED_VAULT.is_some() { Place::Hosted } else if facts.vault_command.is_some() { Place::Server } else { Place::None };
        let place = known.and_then(Place::of).unwrap_or(unknown);
        let address = known.filter(|_| place == Place::Server).map(|probe| probe.server.clone()).unwrap_or_default();
        Where { place: Some(place), address, ..Where::default() }
    }

    pub fn chosen(&self) -> Place {
        self.place.unwrap_or(Place::None)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Answers {
    pub name: String,
    pub codex: bool,
    pub sources: Vec<(Source, bool)>,
    pub vault: Where,
    pub signals: usize,
}

impl Answers {
    fn from(facts: &Facts) -> Answers {
        let read = config::read_sources(&facts.sources);
        let mut sources: Vec<(Source, bool)> = facts.sources.iter().map(|source| (*source, read.contains(source))).collect();
        sources.extend(Source::ALL.into_iter().filter(|source| !facts.sources.contains(source)).map(|source| (source, false)));
        Answers { name: facts.name.clone(), codex: facts.codex.is_some(), sources, vault: Where::from(facts), signals: usize::from(!facts.signals) }
    }

    fn text(&self, field: Field) -> &str {
        match field {
            Field::Name => &self.name,
            Field::Address => &self.vault.address,
            Field::Endpoint => &self.vault.endpoint,
            Field::Bucket => &self.vault.bucket,
            Field::Prefix => &self.vault.prefix,
            Field::Region => &self.vault.region,
            Field::Folder => &self.vault.folder,
        }
    }

    fn text_mut(&mut self, field: Field) -> &mut String {
        match field {
            Field::Name => &mut self.name,
            Field::Address => &mut self.vault.address,
            Field::Endpoint => &mut self.vault.endpoint,
            Field::Bucket => &mut self.vault.bucket,
            Field::Prefix => &mut self.vault.prefix,
            Field::Region => &mut self.vault.region,
            Field::Folder => &mut self.vault.folder,
        }
    }

    fn chosen_sources(&self) -> Vec<Source> {
        self.sources.iter().filter(|(_, on)| *on).map(|(source, _)| *source).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    Choice(usize),
    Check(usize),
    Text(Field),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Link,
    ClaudePlugin,
    CodexPlugin(bool),
    Name(String),
    Keys(Box<KeyPlan>, Vec<toml::Table>),
    Sources(Vec<Source>),
    InstallMindin,
    VaultSetup(Command, Vec<String>),
    VaultCommand(Option<Command>),
    ClaudeVault,
    CodexVault(Command),
    Rules(String),
    Signals(bool),
    Repair(Repair),
    Nothing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub group: &'static str,
    pub step: &'static str,
    pub verb: &'static str,
    pub detail: String,
    pub act: Act,
}

pub enum Next {
    Stay,
    Quit,
    Apply,
    Connect,
    Board,
}

const KEBBIE: &str = "kebbie";
const CLAUDE: &str = "Claude Code";
const CODEX: &str = "Codex";
const HERDR: &str = "herdr";
const MINDIN: &str = "mindin";

pub struct Guide {
    pub step: Step,
    cursor: usize,
    pub answers: Answers,
    facts: Facts,
    visited: [bool; Step::ALL.len()],
    skipped: [bool; Step::ALL.len()],
    fresh: bool,
    error: Option<String>,
    pub applied: Option<Vec<(String, Outcome)>>,
    pub done: bool,
    pub ending: Vec<String>,
}

fn home(path: &Path) -> String {
    crate::area_page::home_relative(path)
}

fn source_line(source: Source) -> (&'static str, &'static str) {
    match source {
        Source::Local => ("Local file", "an item file each in backlog/ beside your config"),
        Source::Vault => ("Vault", "backlog notes in your notes vault"),
        Source::Linear => ("Linear", "issues from a Linear team, needs an API key"),
    }
}

fn short(wanted: &setup::Wanted) -> String {
    let said = setup::text(&wanted.binding, "description");
    said.strip_prefix("kebbie: ").unwrap_or(said).trim_start_matches("open ").trim_start_matches("go to ").to_string()
}

impl Guide {
    pub fn new(facts: Facts, at: Step) -> Guide {
        let answers = Answers::from(&facts);
        let mut guide = Guide { step: Step::Name, cursor: 0, answers, facts, visited: [false; Step::ALL.len()], skipped: [false; Step::ALL.len()], fresh: true, error: None, applied: None, done: false, ending: Vec::new() };
        guide.go(at);
        guide
    }

    fn codex_on(&self) -> bool {
        self.answers.codex && self.facts.codex.is_some()
    }

    fn place(&self) -> Option<Place> {
        (!self.skipped(Step::Vault)).then(|| self.answers.vault.chosen())
    }

    // The chosen place is the one the command kebbie runs answers now, so setup has nothing to change.
    fn place_in(&self) -> bool {
        let at = &self.answers.vault;
        let Some(probe) = self.facts.runner_answer().filter(|probe| probe.configured) else { return false };
        let blank = |texts: &[&String]| texts.iter().all(|text| text.trim().is_empty());
        match at.chosen() {
            Place::Hosted => probe.backend == "server" && HOSTED_VAULT.is_some_and(|hosted| same_url(&probe.server, hosted)),
            Place::Server => probe.backend == "server" && same_url(&probe.server, &at.address),
            Place::S3 => probe.backend == "s3" && blank(&[&at.endpoint, &at.bucket, &at.prefix, &at.region]),
            Place::Folder => probe.backend == "local" && blank(&[&at.folder]),
            Place::None => false,
        }
    }

    fn setup_flags(&self) -> Option<Vec<String>> {
        let at = &self.answers.vault;
        let given = |text: &str| Some(text.trim().to_string()).filter(|text| !text.is_empty());
        Some(match at.chosen() {
            Place::Hosted => vec!["--server".to_string(), HOSTED_VAULT?.to_string()],
            Place::Server => vec!["--server".to_string(), given(&at.address)?],
            Place::S3 => {
                let mut flags = vec!["--s3".to_string(), given(&at.bucket)?, "--endpoint".to_string(), given(&at.endpoint)?];
                for (flag, text) in [("--prefix", &at.prefix), ("--region", &at.region)] {
                    flags.extend(given(text).map(|text| [flag.to_string(), text]).into_iter().flatten());
                }
                flags
            }
            Place::Folder => vec!["--folder".to_string(), given(&at.folder)?],
            Place::None => return None,
        })
    }

    fn needs_setup(&self) -> bool {
        self.place().is_some_and(|place| place != Place::None) && !self.place_in()
    }

    // A vault command without `setup`, as notes-vault-mcp up to 0.13, cannot move the vault, so mindin is installed to do it.
    fn installs(&self) -> bool {
        self.needs_setup() && self.setup_flags().is_some() && !self.facts.runner_answer().is_some_and(|probe| probe.setup)
    }

    fn vault_used(&self) -> Option<Command> {
        match self.place()? {
            Place::None => None,
            _ if self.installs() => Some(Command::Installed),
            _ => Some(self.facts.runner().map_or(Command::Installed, |found| Command::Found(found.command.clone()))),
        }
    }

    fn place_shown(&self) -> String {
        let at = &self.answers.vault;
        let blank = at.endpoint.trim().is_empty() && at.bucket.trim().is_empty();
        match at.chosen() {
            Place::Hosted => format!("hosted mindin at {}", HOSTED_VAULT.unwrap_or_default()),
            Place::Server => format!("your own server {}", at.address.trim()),
            Place::S3 if blank => "an S3 bucket".to_string(),
            Place::S3 => {
                let prefix = Some(at.prefix.trim()).filter(|prefix| !prefix.is_empty()).map_or(String::new(), |prefix| format!("/{prefix}"));
                format!("the bucket {}{prefix} at {}", at.bucket.trim(), at.endpoint.trim())
            }
            Place::Folder if at.folder.trim().is_empty() => "a local folder".to_string(),
            Place::Folder => format!("the folder {}", at.folder.trim()),
            Place::None => "no vault".to_string(),
        }
    }

    fn runner_shown(&self) -> String {
        match self.facts.runner() {
            Some(found) if self.installs() => format!("mindin, installed when you apply: {} has no setup", found.shown()),
            Some(found) => format!("{} · {}", found.shown(), found.source),
            None => "mindin, installed when you apply".to_string(),
        }
    }

    fn position(&self) -> usize {
        self.step.index()
    }

    fn next_step(&self) -> Step {
        Step::ALL.get(self.position() + 1).copied().unwrap_or(Step::Review)
    }

    fn previous_step(&self) -> Option<Step> {
        self.position().checked_sub(1).and_then(|at| Step::ALL.get(at).copied())
    }

    fn spots(&self) -> Vec<Spot> {
        match self.step {
            Step::Name => vec![Spot::Text(Field::Name)],
            Step::Agents => (0..Agent::ALL.len()).map(Spot::Check).collect(),
            Step::Vault => Place::ALL.into_iter().enumerate().filter(|(_, place)| place.open()).flat_map(|(at, place)| {
                let fields = if place == self.answers.vault.chosen() { place.fields() } else { &[] };
                std::iter::once(Spot::Choice(at)).chain(fields.iter().map(|field| Spot::Text(*field)))
            }).collect(),
            Step::Backlog => (0..self.answers.sources.len()).map(Spot::Check).collect(),
            Step::Signals => vec![Spot::Choice(0), Spot::Choice(1)],
            Step::Review => Vec::new(),
        }
    }

    fn spot(&self) -> Option<Spot> {
        self.spots().get(self.cursor).copied()
    }

    fn chosen(&self) -> usize {
        match self.step {
            Step::Vault => Place::ALL.iter().position(|place| *place == self.answers.vault.chosen()).unwrap_or(0),
            Step::Signals => self.answers.signals,
            _ => 0,
        }
    }

    fn go(&mut self, step: Step) {
        self.step = step;
        self.error = None;
        self.fresh = true;
        self.cursor = self.spots().iter().position(|spot| *spot == Spot::Choice(self.chosen())).unwrap_or(0);
    }

    fn choose(&mut self, at: usize) {
        match self.step {
            Step::Vault => {
                self.answers.vault.place = Place::ALL.get(at).copied();
                self.cursor = self.spots().iter().position(|spot| *spot == Spot::Choice(at)).unwrap_or(self.cursor);
            }
            Step::Signals => self.answers.signals = at,
            _ => {}
        }
    }

    fn typed(&self) -> Option<Field> {
        match self.spot() {
            Some(Spot::Text(field)) => Some(field),
            _ => None,
        }
    }

    fn type_char(&mut self, character: char) {
        let Some(field) = self.typed() else { return };
        let fresh = std::mem::replace(&mut self.fresh, false);
        let text = self.answers.text_mut(field);
        if fresh {
            text.clear();
        }
        text.push(character);
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(field) = self.typed() {
            self.fresh = false;
            self.answers.text_mut(field).push_str(text.trim());
        }
    }

    fn reset(&mut self, step: Step) {
        let today = Answers::from(&self.facts);
        match step {
            Step::Vault => self.answers.vault = today.vault,
            Step::Backlog => self.answers.sources = today.sources,
            Step::Signals => self.answers.signals = today.signals,
            Step::Name | Step::Agents | Step::Review => {}
        }
    }

    fn skips(&self) -> bool {
        matches!(self.step, Step::Vault | Step::Backlog | Step::Signals)
    }

    fn check(&self) -> std::result::Result<(), String> {
        match self.step {
            Step::Name => config::valid_machine_name(&self.answers.name).map(|_| ()),
            Step::Vault if self.needs_setup() && self.setup_flags().is_none() => {
                let place = self.answers.vault.chosen();
                Err(format!("{} needs {}", place.label(), place.needs()))
            }
            _ => Ok(()),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Next {
        if crate::keys::ctrl_c(&key) {
            return Next::Quit;
        }
        if self.applied.is_some() {
            return match key.code {
                _ if !self.done => Next::Stay,
                KeyCode::Char('h') if self.offers_hub() => Next::Connect,
                _ => Next::Board,
            };
        }
        let spots = self.spots();
        let typing = self.typed().is_some();
        match key.code {
            KeyCode::Esc => return Next::Quit,
            KeyCode::Up | KeyCode::Down if !spots.is_empty() => {
                let count = spots.len();
                self.cursor = if key.code == KeyCode::Up { (self.cursor + count - 1) % count } else { (self.cursor + 1) % count };
                if let Some(Spot::Choice(at)) = self.spot() {
                    self.choose(at);
                }
                self.fresh = true;
                self.error = None;
            }
            KeyCode::Char(character) if typing => self.type_char(character),
            KeyCode::Backspace if typing => {
                self.fresh = false;
                if let Some(field) = self.typed() {
                    self.answers.text_mut(field).pop();
                }
            }
            KeyCode::Char(' ') => match self.spot() {
                Some(Spot::Check(at)) => self.toggle(at),
                Some(Spot::Choice(at)) => self.choose(at),
                _ => {}
            },
            KeyCode::Enter if self.step == Step::Review => return Next::Apply,
            KeyCode::Enter => match self.check() {
                Ok(()) => {
                    self.visited[self.step.index()] = true;
                    self.skipped[self.step.index()] = false;
                    self.go(self.next_step());
                }
                Err(error) => self.error = Some(error),
            },
            KeyCode::Left => {
                if let Some(previous) = self.previous_step() {
                    self.go(previous);
                }
            }
            KeyCode::Char('s') if self.skips() => {
                self.reset(self.step);
                self.skipped[self.step.index()] = true;
                self.visited[self.step.index()] = false;
                self.go(self.next_step());
            }
            _ => {}
        }
        Next::Stay
    }

    fn toggle(&mut self, at: usize) {
        match self.step {
            Step::Agents if Agent::ALL.get(at) == Some(&Agent::Codex) && self.facts.codex.is_some() => self.answers.codex = !self.answers.codex,
            Step::Backlog => {
                let Some((source, on)) = self.answers.sources.get(at).copied() else { return };
                for (other, ticked) in &mut self.answers.sources {
                    if *other == source {
                        *ticked = !on;
                    } else if !on && source != Source::Linear && *other != Source::Linear {
                        *ticked = false;
                    }
                }
            }
            _ => {}
        }
    }

    fn codex_home_shown(&self, file: &str) -> String {
        self.facts.codex_home.as_deref().map_or(format!("~/.codex/{file}"), |codex| home(&codex.join(file)))
    }

    fn codex_plugin_shown(&self) -> String {
        self.facts.codex_home.as_deref().map_or("~/.codex/plugins/cache/kebbie-tasks/kebbie/local".to_string(), |codex| home(&crate::agent::codex::plugin_root(codex)))
    }

    fn claude_plugin_current(&self) -> bool {
        self.facts.kebbie_plugin && self.facts.claude_stale.is_none()
    }

    fn plugin_shown(&self, agent: Agent) -> &'static str {
        match agent {
            Agent::Claude if self.claude_plugin_current() => "kebbie's plugin current",
            Agent::Claude if self.facts.kebbie_plugin => "kebbie's plugin out of date",
            Agent::Codex if self.facts.codex_plugin == Installed::Current => "kebbie's plugin current",
            Agent::Codex if self.facts.codex_plugin != Installed::Missing => "kebbie's plugin out of date",
            _ => "kebbie's plugin missing",
        }
    }

    fn agent_line(&self, agent: Agent) -> String {
        let facts = &self.facts;
        match agent {
            Agent::Claude => facts.claude.as_ref().map_or("not found · curl -fsSL https://claude.ai/install.sh | bash installs it".to_string(), |version| format!("{version} · {} · {} · runs the lead, the reviewer and tasks", facts.claude_login.shown(login::CLAUDE_FIX), self.plugin_shown(agent))),
            Agent::Codex => facts.codex.as_ref().map_or("not found".to_string(), |version| format!("{version} · {} · {} · runs tasks with a codex profile", facts.codex_login.shown(login::CODEX_FIX), self.plugin_shown(agent))),
        }
    }

    fn in_place(&self, step: Step) -> Option<String> {
        let facts = &self.facts;
        match step {
            Step::Name if facts.name_set => Some(format!("machine.name is \"{}\"", facts.name)),
            Step::Agents if self.claude_plugin_current() && (!self.codex_on() || (facts.codex_plugin == Installed::Current && facts.rules_placed == Placed::Current)) => Some("each agent you use has kebbie's plugin".to_string()),
            Step::Vault => facts.vault_command.as_ref().map(|command| format!("vault.command is \"{command}\"")),
            Step::Backlog if facts.sources_set => Some(format!("board.backlog.sources is [{}]", facts.sources.iter().map(|source| format!("{:?}", source.key())).collect::<Vec<_>>().join(", "))),
            Step::Signals if facts.signals_set => Some(format!("report.signals is {}", facts.signals)),
            _ => None,
        }
    }

    fn skipped(&self, step: Step) -> bool {
        self.skipped[step.index()]
    }

    pub fn plan(&self) -> Vec<Planned> {
        let mut rows = self.mindin_rows();
        rows.extend(self.kebbie_rows());
        rows.extend(self.claude_rows());
        rows.extend(self.codex_rows());
        rows.extend(self.herdr_rows());
        rows
    }

    fn mindin_rows(&self) -> Vec<Planned> {
        let row = |step, verb, detail: String, act| Planned { group: MINDIN, step, verb, detail, act };
        let Some(place) = self.place().filter(|place| *place != Place::None) else { return Vec::new() };
        let mut rows = Vec::new();
        if self.installs() {
            let detail = self.facts.runner().map_or("mindin, which then runs the vault".to_string(), |found| format!("mindin first, since {} has no setup", found.shown()));
            rows.push(row("install", "to do", detail, Act::InstallMindin));
        }
        rows.push(match self.setup_flags() {
            _ if self.place_in() => row("setup", "already", format!("{}{}", self.place_shown(), if matches!(place, Place::Hosted | Place::Server) { " · signed in" } else { "" }), Act::Nothing),
            Some(flags) => row("setup", "to do", self.place_shown(), Act::VaultSetup(self.vault_used().unwrap_or(Command::Installed), flags)),
            None => row("setup", "skipped", format!("{} needs {}", place.label(), place.needs()), Act::Nothing),
        });
        rows
    }

    fn repair_rows(&self, group: &'static str) -> Vec<Planned> {
        self.facts.repairs.iter().filter(|repair| repair_group(**repair) == group).map(|repair| {
            let (step, detail) = repair_shown(*repair);
            Planned { group, step, verb: "to do", detail: detail.to_string(), act: Act::Repair(*repair) }
        }).collect()
    }

    fn kebbie_rows(&self) -> Vec<Planned> {
        let (facts, answers) = (&self.facts, &self.answers);
        let row = |step, verb, detail: String, act| Planned { group: KEBBIE, step, verb, detail, act };
        let mut rows = vec![row("link", "to do", "~/.local/bin/kebbie".to_string(), Act::Link)];
        rows.push(match () {
            _ if facts.name_set && answers.name.trim() == facts.name => row("machine", "already", format!("\"{}\"", facts.name), Act::Nothing),
            _ => row("machine", "to do", format!("\"{}\"", answers.name.trim()), Act::Name(answers.name.trim().to_string())),
        });
        rows.push(self.vault_command_row());
        let sources = answers.chosen_sources();
        let listed = sources.iter().map(|source| source.key()).collect::<Vec<_>>().join(", ");
        rows.push(match () {
            _ if self.skipped(Step::Backlog) => row("backlog", "skipped", "stays as it is".to_string(), Act::Nothing),
            _ if sources == facts.sources => row("backlog", "already", listed, Act::Nothing),
            _ => row("backlog", "to do", listed, Act::Sources(sources)),
        });
        let on = answers.signals == 0;
        let said = |on: bool| if on { "on" } else { "off" };
        rows.push(match () {
            _ if self.skipped(Step::Signals) => row("signals", "skipped", format!("stay {}", said(facts.signals)), Act::Nothing),
            _ if on == facts.signals && facts.signals_set => row("signals", "already", said(on).to_string(), Act::Nothing),
            _ => row("signals", "to do", said(on).to_string(), Act::Signals(on)),
        });
        rows.extend(self.repair_rows(KEBBIE));
        rows
    }

    fn vault_command_row(&self) -> Planned {
        let row = |verb, detail: String, act| Planned { group: KEBBIE, step: "vault", verb, detail, act };
        if self.place().is_none() {
            return row("skipped", "stays as it is".to_string(), Act::Nothing);
        }
        match (self.vault_used(), &self.facts.vault_command) {
            (Some(Command::Found(command)), Some(kept)) if command == words(kept) => row("already", format!("vault.command = \"{kept}\""), Act::Nothing),
            (Some(Command::Found(command)), _) => row("to do", format!("vault.command = \"{}\"", command.join(" ")), Act::VaultCommand(Some(Command::Found(command)))),
            (Some(Command::Installed), _) => row("to do", "vault.command = the mindin it installs".to_string(), Act::VaultCommand(Some(Command::Installed))),
            (None, Some(_)) => row("to do", "vault.command out of kebbie's config.toml".to_string(), Act::VaultCommand(None)),
            (None, None) => row("skipped", "no vault; kebbie keeps tasks without one".to_string(), Act::Nothing),
        }
    }

    fn claude_rows(&self) -> Vec<Planned> {
        let facts = &self.facts;
        let row = |step, verb, detail: String, act| Planned { group: CLAUDE, step, verb, detail, act };
        let mut rows = vec![if self.claude_plugin_current() { row("plugin", "already", "kebbie's plugin".to_string(), Act::Nothing) } else { row("plugin", "to do", "kebbie's plugin, through claude plugin install".to_string(), Act::ClaudePlugin) }];
        rows.push(row("approvals", "already", "kebbie gives each Claude task its permissions as it starts it".to_string(), Act::Nothing));
        if self.vault_used().is_some() {
            rows.push(match &facts.vault {
                Plugin::Enabled { id, .. } => row("vault", "already", format!("the vault plugin {id}"), Act::Nothing),
                Plugin::Disabled { id, .. } => row("vault", "to do", format!("turn on the vault plugin {id}"), Act::ClaudeVault),
                Plugin::Missing => row("vault", "to do", format!("the vault plugin {}", setup::VAULT_PLUGIN), Act::ClaudeVault),
            });
        }
        rows.extend(self.repair_rows(CLAUDE));
        rows
    }

    fn codex_server_placed(&self, command: &Command) -> Placed {
        match command {
            Command::Found(command) => codex_setup::server_for(&self.facts.claude_dir, command).map_or(Placed::Missing, |server| codex_setup::vault_placed(self.facts.codex_config.as_deref(), &server)),
            Command::Installed => Placed::Missing,
        }
    }

    fn codex_rows(&self) -> Vec<Planned> {
        let facts = &self.facts;
        let row = |step, verb, detail: String, act| Planned { group: CODEX, step, verb, detail, act };
        if !self.codex_on() {
            return if facts.codex.is_some() && facts.codex_plugin != Installed::Missing { vec![row("plugin", "to do", "kebbie's plugin out of Codex".to_string(), Act::CodexPlugin(false))] } else { Vec::new() };
        }
        let plugin = format!("kebbie's plugin at {}", self.codex_plugin_shown());
        let rules = self.codex_home_shown(codex_setup::RULES_FILE);
        let config = self.codex_home_shown("config.toml");
        let mut rows = vec![if facts.codex_plugin == Installed::Current { row("plugin", "already", plugin, Act::Nothing) } else { row("plugin", "to do", plugin, Act::CodexPlugin(true)) }];
        rows.push(if facts.rules_placed == Placed::Current { row("approvals", "already", rules, Act::Nothing) } else { row("approvals", "to do", rules, Act::Rules(facts.rules.clone())) });
        if let Some(command) = self.vault_used() {
            let detail = format!("[mcp_servers.vault] in {config}");
            rows.push(match self.codex_server_placed(&command) {
                Placed::Current => row("vault", "already", detail, Act::Nothing),
                _ => row("vault", "to do", detail, Act::CodexVault(command)),
            });
        }
        rows
    }

    fn herdr_rows(&self) -> Vec<Planned> {
        let keys = &self.facts.keys;
        let row = |step, verb, detail: String, act| Planned { group: HERDR, step, verb, detail, act };
        let free: Vec<toml::Table> = keys.missing.iter().filter(|wanted| wanted.taken.is_none()).map(|wanted| wanted.binding.clone()).collect();
        let taken: Vec<String> = keys.missing.iter().filter_map(|wanted| {
            let free = wanted.free.as_ref().map_or(String::new(), |free| format!(", {free} is free"));
            Some(format!("{} unbound: {} is {}'s{free}", short(wanted), wanted.key(), wanted.taken.clone()?))
        }).collect();
        let taken = if taken.is_empty() { String::new() } else { format!("; {}", taken.join("; ")) };
        let mut rows = vec![match &keys.blocked {
            Some(blocked) => row("keys", "skipped", blocked.clone(), Act::Nothing),
            None if keys.missing.is_empty() => row("keys", "already", "kebbie's keys are bound in herdr".to_string(), Act::Nothing),
            None if free.is_empty() => row("keys", "skipped", format!("bind them by hand from examples/herdr-config.toml{taken}"), Act::Nothing),
            None => {
                let shown = free.iter().map(|binding| setup::text(binding, "key")).collect::<Vec<_>>().join(", ");
                row("keys", "to do", format!("{shown} at the end of {}{taken}", home(&keys.config)), Act::Keys(Box::new(keys.clone()), free))
            }
        }];
        rows.extend(self.repair_rows(HERDR));
        rows
    }

    fn runs(&self) -> Vec<String> {
        let config = self.codex_home_shown("config.toml");
        let claude = Path::new(&self.facts.claude_bin).file_name().map_or(self.facts.claude_bin.clone(), |name| name.to_string_lossy().to_string());
        self.plan().iter().filter_map(|row| match &row.act {
            Act::InstallMindin => Some(format!("{MINDIN_INSTALL} first, in this terminal")),
            Act::VaultSetup(command, flags) => Some(format!("{} in this terminal{}", setup_script(&shown_command(command), flags), self.setup_asks())),
            Act::Rules(text) => Some(format!("writes {}, {} lines", self.codex_home_shown(codex_setup::RULES_FILE), text.lines().count())),
            Act::CodexVault(command) if self.codex_server_placed(command) == Placed::Differs => Some(format!("replaces [mcp_servers.vault] in {config}; the rest of the file stays as it is")),
            Act::CodexVault(_) => Some(format!("adds [mcp_servers.vault] to the end of {config}; the rest of the file stays as it is")),
            Act::ClaudeVault => Some(setup::steps(&claude, &self.facts.vault, self.facts.gronare).iter().map(|step| step.join(" ")).collect::<Vec<_>>().join(", then ")),
            _ => None,
        }).collect()
    }

    fn setup_asks(&self) -> &'static str {
        match self.answers.vault.chosen() {
            Place::Hosted | Place::Server => "; it signs this machine in through the browser when it is not yet",
            Place::S3 => "; it asks for the access key and the secret, hidden, unless S3_ACCESS_KEY and S3_SECRET_KEY are set",
            Place::Folder => "; the folder must exist",
            Place::None => "",
        }
    }

    fn offers_hub(&self) -> bool {
        self.facts.hub.is_none()
    }
}

fn shown_command(command: &Command) -> Vec<String> {
    match command {
        Command::Found(command) => command.clone(),
        Command::Installed => vec![MINDIN.to_string()],
    }
}

fn shell_word(word: &str) -> String {
    let plain = !word.is_empty() && word.chars().all(|character| character.is_ascii_alphanumeric() || "-_./:=@%+,~".contains(character));
    if plain { word.to_string() } else { crate::sh::quote(word) }
}

fn setup_script(command: &[String], flags: &[String]) -> String {
    command.iter().map(String::as_str).chain(["setup"]).chain(flags.iter().map(String::as_str)).map(shell_word).collect::<Vec<_>>().join(" ")
}

fn repair_group(repair: Repair) -> &'static str {
    match repair {
        Repair::StatusLine => CLAUDE,
        Repair::KeeperStart | Repair::KeeperStop => KEBBIE,
        Repair::HerdrPlugin => HERDR,
    }
}

fn repair_shown(repair: Repair) -> (&'static str, &'static str) {
    match repair {
        Repair::StatusLine => ("usage", "kebbie's statusLine in Claude Code's settings.json, for the board's usage bar"),
        Repair::KeeperStart => ("keeper", "start it, so routines run without an open board"),
        Repair::KeeperStop => ("keeper", "stop it, as [keeper] enabled = false says"),
        Repair::HerdrPlugin => ("herdr", "enable kebbie's herdr plugin"),
    }
}

pub trait Hooks {
    fn progress(&mut self, rows: &[(String, Outcome)]);
    fn outside(&mut self, script: &str) -> Result<()>;
}

fn when_done(outcome: Outcome, detail: impl Into<String>) -> Outcome {
    if outcome.verb == "done" { Outcome::new("done", detail) } else { outcome }
}

fn write_key(env: &Env, key: &str, set: &Set) -> Outcome {
    match config_edit::write(&env.config, true, false, |text| config_edit::set(text, key, set)) {
        Ok(()) => Outcome::new("done", format!("{key} = {} in {}", set.shown(), home(&env.config))),
        Err(error) => Outcome::new("failed", error.lines().next().unwrap_or_default().to_string()),
    }
}

fn remove_key(env: &Env, key: &str) -> Outcome {
    match config_edit::write(&env.config, true, false, |text| config_edit::remove(text, key)) {
        Ok(()) => Outcome::new("done", format!("{key} out of {}", home(&env.config))),
        Err(error) => Outcome::new("failed", error.lines().next().unwrap_or_default().to_string()),
    }
}

fn codex_home<'a>(env: &'a Env) -> Result<&'a Path> {
    env.codex_home.as_deref().ok_or_else(|| "neither CODEX_HOME nor HOME is set".to_string())
}

fn codex_plugin(env: &Env, on: bool, shown: &str) -> Outcome {
    let done = codex_home(env).and_then(|home| if on { crate::agent::codex::install(home, &env.kebbie).map(|written| !written.is_empty()) } else { crate::agent::codex::uninstall(home) });
    match done {
        Ok(true) => Outcome::new("done", shown),
        Ok(false) => Outcome::new("already", shown),
        Err(error) => Outcome::new("failed", format!("Codex: {error}")),
    }
}

fn rules(env: &Env, text: &str) -> Outcome {
    match codex_home(env).and_then(|codex| codex_setup::write_rules(codex, text).map(|_| home(&codex_setup::rules_path(codex)))) {
        Ok(path) => Outcome::new("done", path),
        Err(error) => Outcome::new("failed", error),
    }
}

// The installer puts mindin in uv's tool folder, ~/.local/bin unless uv was told otherwise, which a running PATH may not list yet.
fn installed_mindin(env: &Env) -> Option<PathBuf> {
    let path: Vec<PathBuf> = env.machine.path.iter().cloned().chain([env.machine.home.join(".local/bin")]).collect();
    crate::doctor::which("mindin", &path)
}

fn install_mindin(env: &Env, hooks: &mut dyn Hooks) -> (Outcome, Option<Vec<String>>) {
    if let Err(error) = hooks.outside(MINDIN_INSTALL) {
        return (Outcome::new("failed", format!("mindin's installer stopped: {error}")), None);
    }
    match installed_mindin(env) {
        Some(mindin) => (Outcome::new("done", format!("mindin at {}", home(&mindin))), Some(vec![mindin.to_string_lossy().to_string()])),
        None => (Outcome::new("failed", "mindin's installer ran, but mindin is neither on PATH nor in ~/.local/bin"), None),
    }
}

fn resolved(command: &Command, installed: &Option<Vec<String>>) -> std::result::Result<Vec<String>, Outcome> {
    match command {
        Command::Found(command) => Ok(command.clone()),
        Command::Installed => installed.clone().ok_or_else(|| Outcome::new("skipped", "mindin is not installed, so there is no vault to point at")),
    }
}

fn codex_vault(env: &Env, command: &[String]) -> Outcome {
    let Some(server) = codex_setup::server_for(&env.machine.claude_dir, command) else { return Outcome::new("failed", "the vault command is empty") };
    let result = codex_home(env).and_then(|codex| {
        let path = codex.join("config.toml");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if codex_setup::vault_placed(Some(&text), &server) == Placed::Current {
            return Ok((false, path));
        }
        crate::state::write(&path, codex_setup::with_vault(&text, &server)?)?;
        Ok((true, path))
    });
    match result {
        Ok((true, path)) => Outcome::new("done", format!("[mcp_servers.vault] in {}", home(&path))),
        Ok((false, path)) => Outcome::new("already", format!("[mcp_servers.vault] in {}", home(&path))),
        Err(error) => Outcome::new("failed", error),
    }
}

// Read again here, since mindin's installer may have added the vault plugin itself.
fn claude_vault(env: &Env) -> Outcome {
    let claude_dir = &env.machine.claude_dir;
    let steps = setup::steps(&env.machine.claude, &crate::vault_cli::plugin(claude_dir), setup::marketplace_known(claude_dir, "gronare"));
    if steps.is_empty() {
        return Outcome::new("already", format!("the vault plugin {}", setup::VAULT_PLUGIN));
    }
    for step in &steps {
        if let Err(error) = crate::sh::run(&step[0], &step[1..].iter().map(String::as_str).collect::<Vec<_>>(), None) {
            return Outcome::new("failed", error);
        }
    }
    Outcome::new("done", format!("the vault plugin {}", setup::VAULT_PLUGIN))
}

fn keys(env: &Env, plan: &KeyPlan, bindings: &[toml::Table]) -> Outcome {
    let (outcome, wrote) = setup::add_keys(plan, bindings);
    if !wrote {
        return outcome;
    }
    let shown = bindings.iter().map(|binding| setup::text(binding, "key")).collect::<Vec<_>>().join(", ");
    let reloaded = if setup::reload_herdr(env.machine) == setup::Reload::Done { "herdr reloaded" } else { "restart herdr to use them" };
    when_done(outcome, format!("{shown} added, {reloaded}"))
}

fn repair(env: &Env, repair: Repair) -> Outcome {
    let said = |result: Result<String>, done: &str| match result {
        Ok(_) => Outcome::new("done", done),
        Err(error) => Outcome::new("failed", error),
    };
    match repair {
        Repair::StatusLine => setup::add_status_line(&env.machine.claude_dir.join("settings.json")),
        Repair::KeeperStart => said(crate::keeper::start_in(&env.state, Path::new(&env.kebbie), true), "started"),
        Repair::KeeperStop => said(crate::keeper::stop_in(&env.state, std::time::Duration::from_secs(10)), "stopped"),
        Repair::HerdrPlugin => said(crate::sh::run(&env.machine.herdr, &["plugin", "enable", "kebbie"], None), "kebbie's plugin enabled"),
    }
}

pub fn apply(planned: &[Planned], env: &Env, hooks: &mut dyn Hooks) -> Vec<(String, Outcome)> {
    let mut done: Vec<(String, Outcome)> = Vec::new();
    let mut installed = None;
    for row in planned {
        let outcome = match &row.act {
            Act::Nothing => Outcome::new(row.verb, row.detail.clone()),
            Act::Link => setup::link_binary(&env.machine.home, &env.machine.path),
            Act::ClaudePlugin => setup::claude_plugin(env.machine),
            Act::CodexPlugin(on) => codex_plugin(env, *on, &row.detail),
            Act::Rules(text) => rules(env, text),
            Act::InstallMindin => {
                let (outcome, mindin) = install_mindin(env, hooks);
                installed = mindin;
                outcome
            }
            Act::VaultSetup(command, flags) => match resolved(command, &installed) {
                Ok(command) => match hooks.outside(&setup_script(&command, flags)) {
                    Ok(()) => Outcome::new("done", row.detail.clone()),
                    Err(error) => Outcome::new("failed", format!("mindin setup stopped: {error}")),
                },
                Err(outcome) => outcome,
            },
            Act::VaultCommand(Some(command)) => match resolved(command, &installed) {
                Ok(command) => when_done(write_key(env, "vault.command", &Set::Text(command.join(" "))), format!("vault.command = \"{}\"", command.join(" "))),
                Err(outcome) => outcome,
            },
            Act::VaultCommand(None) => remove_key(env, "vault.command"),
            Act::ClaudeVault => claude_vault(env),
            Act::CodexVault(command) => match resolved(command, &installed) {
                Ok(command) => codex_vault(env, &command),
                Err(outcome) => outcome,
            },
            Act::Name(name) => when_done(write_key(env, "machine.name", &Set::Text(name.clone())), row.detail.clone()),
            Act::Keys(plan, bindings) => keys(env, plan, bindings),
            Act::Sources(sources) => when_done(write_key(env, "board.backlog.sources", &Set::List(sources.iter().map(|source| source.key().to_string()).collect())), row.detail.clone()),
            Act::Signals(on) => when_done(write_key(env, "report.signals", &Set::Bool(*on)), row.detail.clone()),
            Act::Repair(fix) => repair(env, *fix),
        };
        done.push((row.step.to_string(), outcome));
        hooks.progress(&done);
    }
    done
}

struct Plain;

impl Hooks for Plain {
    fn progress(&mut self, _: &[(String, Outcome)]) {}

    fn outside(&mut self, script: &str) -> Result<()> {
        crate::sh::run_visibly("sh", &["-c", script], None)
    }
}

// --yes applies the guide's defaults, every line printed, never answers the error signals question and never runs mindin's installer or setup; a run without a terminal writes only kebbie's own link and plugins and names the rest.
pub fn without_asking(env: &Env, asking: Asking) -> Vec<(String, Outcome)> {
    let facts = Facts::read(env);
    if asking == Asking::Yes {
        let planned: Vec<Planned> = Guide::new(facts.clone(), Step::Review).plan().into_iter().filter(|row| !matches!(row.act, Act::InstallMindin | Act::VaultSetup(..))).collect();
        return planned.iter().map(|row| match &row.act {
            Act::Signals(_) => (label(row), Outcome::new("skipped", "error signals stay off until you say yes in kebbie setup")),
            Act::Keys(..) => {
                let (mut keys, wrote) = setup::keys_without_asking(&facts.keys, asking);
                keys.detail.push_str(&setup::herdr_reload(env.machine, wrote));
                (label(row), keys)
            }
            _ => (label(row), apply(std::slice::from_ref(row), env, &mut Plain).remove(0).1),
        }).collect();
    }
    let (mut keys, wrote) = setup::keys_without_asking(&facts.keys, asking);
    keys.detail.push_str(&setup::herdr_reload(env.machine, wrote));
    let codex = crate::agent::codex::refresh(env.codex_home.as_deref(), &env.kebbie).map(|refreshed| match refreshed {
        Ok(files) if files.is_empty() => Outcome::new("already", "kebbie's Codex plugin is up to date"),
        Ok(files) => Outcome::new("done", format!("kebbie's Codex plugin updated: {}", files.join(", "))),
        Err(error) => Outcome::new("failed", format!("Codex: {error}")),
    });
    let rows = [("link", Some(setup::link_binary(&env.machine.home, &env.machine.path))), ("plugin", Some(setup::claude_plugin(env.machine))), ("codex", codex), ("keys", Some(keys)), ("vault", vault_unasked(&facts, &env.config))];
    rows.into_iter().filter_map(|(step, outcome)| Some((step.to_string(), outcome?))).collect()
}

fn label(row: &Planned) -> String {
    match row.group {
        CLAUDE => format!("claude {}", row.step),
        CODEX => format!("codex {}", row.step),
        _ => row.step.to_string(),
    }
}

// A run that asks nothing names the vault command it found and the line to set, and writes no choice of its own.
fn vault_unasked(facts: &Facts, config: &Path) -> Option<Outcome> {
    if let Some(command) = &facts.vault_command {
        return Some(Outcome::new("already", format!("vault.command = \"{command}\"")));
    }
    let runner = facts.runner()?;
    Some(Outcome::new("skipped", format!("{} runs the vault: kebbie setup --yes sets vault.command = \"{}\" under [vault] in {}", runner.source, runner.shown(), home(config))))
}

pub fn vault_text(env: &Env) -> String {
    let facts = Facts::read(env);
    match facts.runner() {
        Some(found) => format!("Vault: {} · {} · {} · from {}\nkebbie setup --yes uses it, or kebbie setup in a terminal lets you choose.", found.answered(), lives(found), found.shown(), found.source),
        None => format!("Vault: no vault command answers on this machine.\nTo install mindin, which then asks where the vault lives, run: {MINDIN_INSTALL}\nkebbie setup in a terminal offers the same, and then adds the vault to your agents."),
    }
}

fn group_of(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => CLAUDE,
        Agent::Codex => CODEX,
    }
}

pub fn agent_rows(env: &Env, agent: Agent) -> Vec<Planned> {
    let mut guide = Guide::new(Facts::read(env), Step::Review);
    if agent == Agent::Codex {
        guide.facts.codex.get_or_insert_with(String::new);
        guide.answers.codex = true;
    }
    guide.plan().into_iter().filter(|row| row.group == group_of(agent) && row.step != "plugin" && row.verb == "to do").collect()
}

pub fn offers(env: &Env, agent: Agent) -> Vec<String> {
    let rows: Vec<String> = agent_rows(env, agent).iter().map(|row| format!("  {:<10} {}", row.step, row.detail)).collect();
    if rows.is_empty() {
        return rows;
    }
    std::iter::once(format!("kebbie setup {} --yes also writes:", agent.key())).chain(rows).collect()
}

pub fn apply_agent(env: &Env, agent: Agent) -> Vec<String> {
    apply(&agent_rows(env, agent), env, &mut Plain).iter().map(|(step, outcome)| outcome.line(step)).collect()
}

const DONE: &str = "Setup done";
const NEXT: &str = "Next steps";
const LEAD: (&str, &str) = ("prefix+space", "tell the lead what you want done");
pub const PHONE: [&str; 3] = ["", "Your board on your phone: kebbie hub connect, or H on the board", "kebbie.dev/hub"];

fn next_step(command: &str, why: &str) -> String {
    format!("  {command:<21}{why}")
}

// kebbie never writes a shell profile: it names the line and the file for the user's shell.
fn path_step(shell: &str) -> String {
    let line = "export PATH=\"$HOME/.local/bin:$PATH\"";
    let (line, file) = match Path::new(shell).file_name().and_then(|name| name.to_str()) {
        Some("zsh") => (line, "~/.zshrc"),
        Some("bash") if cfg!(target_os = "macos") => (line, "~/.bash_profile"),
        Some("bash") => (line, "~/.bashrc"),
        Some("fish") => ("fish_add_path ~/.local/bin", "~/.config/fish/config.fish"),
        _ => (line, "your shell's profile"),
    };
    next_step("PATH", &format!("add {line} to {file}, then open a new terminal"))
}

pub fn ending(codex: Option<Login>, checks: &[crate::doctor::Check], shell: &str) -> Vec<String> {
    let count = |ok: bool| checks.iter().filter(|check| if ok { check.level == crate::doctor::Level::Ok } else { matches!(check.level, crate::doctor::Level::Warn | crate::doctor::Level::Fail) }).count();
    let open = count(false);
    let left = if open > 0 { format!(", {open} to look at") } else { String::new() };
    let mut lines = vec![String::new(), format!("{DONE} · doctor: {} ok{left}", count(true)), String::new(), NEXT.to_string()];
    if checks.iter().any(|check| check.name == crate::doctor::ON_PATH && check.fact == crate::doctor::NOT_ON_PATH) {
        lines.push(path_step(shell));
    }
    match codex {
        Some(Login::Out) => lines.push(next_step("codex login", "Codex is not logged in, so codex tasks cannot start")),
        Some(Login::Unknown) => lines.push(next_step("codex login", "if you have not yet, so codex tasks can start")),
        _ => {}
    }
    if open > 0 {
        lines.push(next_step("kebbie doctor", "says what is left and how to fix it"));
    }
    lines.push(next_step(LEAD.0, LEAD.1));
    lines
}

fn row_line(step: &str, outcome: &Outcome) -> Line<'static> {
    Line::from(vec![Span::raw("  "), Span::styled(format!("{:<9}", outcome.verb), style(verb_color(outcome.verb))), Span::styled(format!("{step:<11}"), style(MAUVE)), Span::styled(outcome.detail.clone(), style(TEXT))])
}

fn verb_color(verb: &str) -> Color {
    match verb {
        "done" | "already" => GREEN,
        "failed" => RED,
        "to do" => TEXT,
        _ => MUTED,
    }
}

impl Guide {
    fn radio(on: bool) -> Span<'static> {
        if on { Span::styled("(•)", style(GREEN)) } else { Span::styled("( )", style(MUTED)) }
    }

    fn check_mark(on: bool, found: bool) -> Span<'static> {
        match (on, found) {
            (_, false) => Span::styled("[ ]", style(FAINT)),
            (true, true) => Span::styled("[x]", style(GREEN)),
            (false, true) => Span::styled("[ ]", style(MUTED)),
        }
    }

    fn option(&self, spot: Spot, mark: Span<'static>, label: String, about: String, found: bool) -> Line<'static> {
        let chosen = self.spot() == Some(spot);
        let about = if about.is_empty() { String::new() } else { format!("  · {about}") };
        let label_style = match (chosen, found) {
            (_, false) => style(MUTED),
            (true, true) => bold(TEXT),
            (false, true) => style(TEXT),
        };
        let line = Line::from(vec![Span::styled(if chosen { "▸ " } else { "  " }, style(MAUVE)), mark, Span::raw(" "), Span::styled(label, label_style), Span::styled(about, style(MUTED))]);
        if chosen { line.style(Style::new().bg(SURFACE)) } else { line }
    }

    fn place_line(&self, at: usize, place: Place) -> Line<'static> {
        let chosen = self.spot() == Some(Spot::Choice(at));
        let open = place.open();
        let mark = if open { Self::radio(self.answers.vault.chosen() == place) } else { Span::styled("( )", style(FAINT)) };
        let label = match (open, chosen) {
            (false, _) => style(FAINT),
            (true, true) => bold(TEXT),
            (true, false) => style(TEXT),
        };
        let line = Line::from(vec![Span::styled(if chosen { "▸ " } else { "  " }, style(MAUVE)), mark, Span::raw(" "), Span::styled(format!("{:<17}", place.label()), label), Span::styled(self.facts.place_about(place), style(if open { MUTED } else { FAINT }))]);
        if chosen { line.style(Style::new().bg(SURFACE)) } else { line }
    }

    fn field_line(&self, field: Field) -> Line<'static> {
        let chosen = self.spot() == Some(Spot::Text(field));
        let value = self.answers.text(field).to_string();
        let mut spans = vec![Span::styled(if chosen { "      ▸ " } else { "        " }, style(MAUVE)), Span::styled(format!("{:<10}", field.label()), style(SUBTEXT))];
        spans.push(Span::styled(format!("{value}{}", if chosen { "▏" } else { "" }), if chosen { bold(TEXT).bg(MANTLE) } else { style(TEXT) }));
        if value.is_empty() {
            spans.push(Span::styled(field.placeholder().to_string(), style(FAINT)));
        }
        Line::from(spans)
    }

    fn review_lines(&self, rows: Vec<(String, Outcome)>) -> Vec<Line<'static>> {
        let groups: Vec<&'static str> = self.plan().iter().map(|row| row.group).collect();
        let mut lines = Vec::new();
        let mut group = None;
        for (at, (step, outcome)) in rows.into_iter().enumerate() {
            let this = groups.get(at).copied();
            if this.is_some() && this != group {
                group = this;
                if !lines.is_empty() {
                    lines.push(Line::default());
                }
                lines.push(Line::from(Span::styled(this.unwrap_or_default().to_string(), bold(SUBTEXT))));
            }
            lines.push(row_line(&step, &outcome));
        }
        lines
    }

    fn body(&self) -> Vec<Line<'static>> {
        let (facts, answers) = (&self.facts, &self.answers);
        let mut lines = Vec::new();
        match self.step {
            Step::Name => {
                let chosen = self.spot() == Some(Spot::Text(Field::Name));
                let caret = if chosen { "▏" } else { "" };
                lines.push(Line::from(vec![Span::styled(if chosen { "    ▸ " } else { "      " }, style(MAUVE)), Span::styled(format!("{:<9}", "name"), style(SUBTEXT)), Span::styled(format!("{}{caret}", answers.name), if chosen { bold(TEXT).bg(MANTLE) } else { style(TEXT) })]));
                if !facts.name_set {
                    lines.push(Line::from(Span::styled("           from this computer's name", style(MUTED))));
                }
            }
            Step::Agents => {
                for (at, agent) in Agent::ALL.into_iter().enumerate() {
                    let found = facts.agent_found(agent);
                    let on = found && (agent == Agent::Claude || answers.codex);
                    let label = if agent == Agent::Claude { "Claude Code" } else { "Codex" };
                    lines.push(self.option(Spot::Check(at), Self::check_mark(on, found), label.to_string(), self.agent_line(agent), found));
                }
            }
            Step::Vault => {
                for (at, place) in Place::ALL.into_iter().enumerate() {
                    lines.push(self.place_line(at, place));
                    if place == answers.vault.chosen() && place.open() {
                        lines.extend(place.fields().iter().map(|field| self.field_line(*field)));
                        if place == Place::S3 {
                            lines.push(Line::from(Span::styled("        the access key and the secret are asked as you apply, the secret hidden", style(MUTED))));
                        }
                    }
                }
                lines.push(Line::default());
                lines.push(Line::from(vec![Span::styled("runs  ", style(MUTED)), Span::styled(self.runner_shown(), style(MUTED))]));
            }
            Step::Backlog => {
                let filed = config::filing(&answers.chosen_sources());
                for (at, (source, on)) in answers.sources.iter().enumerate() {
                    let (label, about) = source_line(*source);
                    let about = if *on && Some(*source) == filed { format!("{about} · new items go here") } else { about.to_string() };
                    lines.push(self.option(Spot::Check(at), Self::check_mark(*on, true), label.to_string(), about, true));
                }
            }
            Step::Signals => {
                let labels = match (facts.signals_set, facts.signals) {
                    (false, _) => [("Yes, turn them on", "writes signals = true under [report]"), ("No, keep them off", "writes signals = false under [report]")],
                    (true, true) => [("Keep them on", "signals = true stays under [report]"), ("Turn them off", "writes signals = false under [report]")],
                    (true, false) => [("Turn them on", "writes signals = true under [report]"), ("Keep them off", "signals = false stays under [report]")],
                };
                for (at, (label, about)) in labels.into_iter().enumerate() {
                    lines.push(self.option(Spot::Choice(at), Self::radio(answers.signals == at), label.to_string(), about.to_string(), true));
                }
                lines.push(Line::default());
                lines.push(Line::from(Span::styled("A bug report with text is always shown to you first and sent only on your y.", style(SUBTEXT))));
            }
            Step::Review if self.done => lines.extend(self.done_lines()),
            Step::Review => {
                let rows: Vec<(String, Outcome)> = match &self.applied {
                    Some(done) => {
                        let mut rows = done.clone();
                        rows.extend(self.plan().into_iter().skip(done.len()).map(|row| (row.step.to_string(), Outcome::new("to do", row.detail))));
                        rows
                    }
                    None => self.plan().into_iter().map(|row| (row.step.to_string(), Outcome::new(row.verb, row.detail))).collect(),
                };
                lines.extend(self.review_lines(rows));
                let runs = if self.applied.is_none() { self.runs() } else { Vec::new() };
                if !runs.is_empty() {
                    lines.push(Line::default());
                    lines.extend(runs.into_iter().map(|said| Line::from(vec![Span::styled("runs: ", style(MUTED)), Span::styled(said, style(MUTED))])));
                }
            }
        }
        if let Some(place) = self.in_place(self.step).filter(|_| !matches!(self.step, Step::Review | Step::Vault)) {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.push(Line::from(vec![Span::styled("✓ in place: ", style(GREEN)), Span::styled(place, style(SUBTEXT))]));
        }
        if let Some(error) = &self.error {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(error.clone(), style(RED))));
        }
        lines
    }

    fn hint(&self) -> String {
        match self.step {
            _ if self.applied.is_some() && !self.done => "applying…".to_string(),
            _ if self.applied.is_some() => "any key opens your board".to_string(),
            Step::Name => "type to change · ⏎ next · esc quit".to_string(),
            Step::Vault if self.typed().is_some() => "type to change · ↑ ↓ move · ⏎ next · ← back · esc quit".to_string(),
            Step::Review => "⏎ apply all · ← back · esc quit without writing".to_string(),
            step => {
                let toggle = if matches!(step, Step::Agents | Step::Backlog) { "space toggle · " } else { "" };
                let skip = if self.skips() { "s skip · " } else { "" };
                format!("↑ ↓ choose · {toggle}⏎ next · ← back · {skip}esc quit")
            }
        }
    }

    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area().inner(Margin { horizontal: 2, vertical: 1 });
        let [title, _, body] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(5)]).areas(area);
        let right = "run it again any time · --yes takes the defaults";
        let left = format!("step {} of {}", self.position() + 1, Step::ALL.len());
        let gap = (title.width as usize).saturating_sub(14 + left.len() + right.len()).max(2);
        let right = if gap > 2 { right } else { "" };
        frame.render_widget(Paragraph::new(Line::from(vec![Span::styled("kebbie setup", bold(MAUVE)), Span::raw("  "), Span::styled(left, style(MUTED)), Span::raw(" ".repeat(gap)), Span::styled(right, style(MUTED))])), title);
        let main = if body.width >= 72 {
            let [rail, _, main] = Layout::horizontal([Constraint::Length(22), Constraint::Length(2), Constraint::Min(40)]).areas(body);
            let lines: Vec<Line> = Step::ALL.iter().enumerate().map(|(at, step)| {
                let current = *step == self.step;
                let mark = match () {
                    _ if current => Span::styled("▸", style(MAUVE)),
                    _ if self.visited[step.index()] || self.in_place(*step).is_some() => Span::styled("✓", style(GREEN)),
                    _ => Span::styled("·", style(MUTED)),
                };
                let line = Line::from(vec![mark, Span::styled(format!(" {} {}", at + 1, step.title()), if current { bold(TEXT) } else { style(SUBTEXT) })]);
                if current { line.style(Style::new().bg(SURFACE)) } else { line }
            }).collect();
            frame.render_widget(Paragraph::new(lines), rail);
            main
        } else {
            body
        };
        let [boxed, hint] = Layout::vertical([Constraint::Min(4), Constraint::Length(2)]).areas(main);
        let mut lines = if self.done { Vec::new() } else { vec![Line::from(Span::styled(self.step.help(), style(SUBTEXT))), Line::default()] };
        lines.extend(self.body());
        let framed = |title: &str| Block::bordered().border_type(BorderType::Rounded).border_style(style(FAINT)).padding(Padding::horizontal(1)).title(Span::styled(format!(" {title} "), bold(MAUVE)));
        let tall = |block: &Block, lines: &[Line<'static>], width: u16| Paragraph::new(lines.to_vec()).wrap(Wrap { trim: false }).line_count(block.inner(Rect { width, height: 3, ..boxed }).width) as u16 + 2;
        let phone = (!self.ending.is_empty() && self.offers_hub()).then(|| {
            let block = framed("Your board on your phone");
            let width = boxed.width.min(72);
            let lines = self.phone();
            let height = tall(&block, &lines, width);
            (block, lines, width, height)
        });
        let below = phone.as_ref().map_or(0, |(_, _, _, height)| height + 1);
        let block = framed(if self.done { "Done" } else { self.step.title() });
        let shown = Rect { height: tall(&block, &lines, boxed.width).min(boxed.height.saturating_sub(below)), ..boxed };
        frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), shown);
        let mut hint_at = hint.y + 1;
        if let Some((block, lines, width, height)) = phone {
            let at = Rect { y: shown.y + shown.height + 1, width, height: height.min(boxed.height.saturating_sub(shown.height + 1)), ..boxed };
            frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), at);
            hint_at = hint_at.min(at.y + at.height + 1);
        }
        frame.render_widget(Paragraph::new(Line::from(Span::styled(self.hint(), style(MUTED)))), Rect { y: hint_at, height: 1, ..hint });
    }

    fn done_lines(&self) -> Vec<Line<'static>> {
        let changed: Vec<(String, Outcome)> = self.applied.iter().flatten().filter(|(_, outcome)| !matches!(outcome.verb, "already" | "skipped")).cloned().collect();
        let mut lines: Vec<Line<'static>> = changed.iter().map(|(step, outcome)| row_line(step, outcome)).collect();
        for said in &self.ending {
            lines.push(match (said.as_str(), said.get(..23), said.get(23..)) {
                (_, _, _) if said.starts_with(DONE) => Line::from(vec![Span::styled(DONE, style(GREEN)), Span::styled(said[DONE.len()..].to_string(), style(SUBTEXT))]),
                (NEXT, _, _) => Line::from(Span::styled(NEXT, bold(MAUVE))),
                (_, Some(command), Some(why)) => Line::from(vec![Span::styled(command.to_string(), style(TEXT)), Span::styled(why.to_string(), style(SUBTEXT))]),
                _ => Line::from(Span::styled(said.clone(), style(TEXT))),
            });
        }
        lines
    }

    fn phone(&self) -> Vec<Line<'static>> {
        vec![
            Line::from(Span::styled(format!("Follow tasks and answer questions from anywhere, end-to-end encrypted. Free on {}, or run your own.", crate::hub::host(config::DEFAULT_HUB_URL)), style(TEXT))),
            Line::default(),
            Line::from(vec![Span::styled("h", bold(MAUVE)), Span::styled("  connect now", style(TEXT)), Span::styled("     later: kebbie hub connect, or H on the board", style(MUTED))]),
            Line::from(Span::styled(PHONE[2], style(SUBTEXT))),
        ]
    }

    fn text(&self) -> String {
        self.said(self.offers_hub())
    }

    fn said(&self, phone: bool) -> String {
        match &self.applied {
            None => "Nothing written: kebbie setup runs the guide again whenever you want.".to_string(),
            Some(rows) => {
                let planned = self.plan();
                let mut lines: Vec<String> = rows.iter().enumerate().map(|(at, (step, outcome))| outcome.line(&planned.get(at).map_or(step.clone(), label))).collect();
                lines.extend(self.ending.iter().cloned());
                if phone {
                    lines.extend(PHONE.iter().map(|line| line.to_string()));
                }
                lines.join("\n")
            }
        }
    }
}

struct Screen<'a> {
    terminal: &'a mut ratatui::DefaultTerminal,
    guide: &'a mut Guide,
}

impl Hooks for Screen<'_> {
    fn progress(&mut self, rows: &[(String, Outcome)]) {
        self.guide.applied = Some(rows.to_vec());
        let guide = &*self.guide;
        let _ = self.terminal.draw(|frame| guide.draw(frame));
    }

    fn outside(&mut self, script: &str) -> Result<()> {
        crate::board::terminal::outside(self.terminal, || {
            println!("\nkebbie setup runs: {script}\n");
            crate::sh::run_visibly("sh", &["-c", script], None)
        })
    }
}

fn ask(env: &Env, at: Step) -> Result<(Next, Guide)> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};
    let mut guide = Guide::new(Facts::read(env), at);
    let mut terminal = crate::board::terminal::init_terminal();
    let result = (|| -> Result<Next> {
        loop {
            terminal.draw(|frame| guide.draw(frame)).map_err(|error| error.to_string())?;
            match event::read().map_err(|error| error.to_string())? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match guide.key(key) {
                    Next::Stay => {}
                    Next::Apply => {
                        let planned = guide.plan();
                        guide.applied = Some(Vec::new());
                        let rows = apply(&planned, env, &mut Screen { terminal: &mut terminal, guide: &mut guide });
                        let codex = guide.codex_on().then(|| login::codex(guide.facts.codex_bin.as_deref(), login::TIMEOUT));
                        guide.ending = ending(codex, &crate::doctor::checks(&crate::doctor::Env::current()), &std::env::var("SHELL").unwrap_or_default());
                        guide.applied = Some(rows);
                        guide.done = true;
                    }
                    next => return Ok(next),
                },
                Event::Paste(text) => guide.paste(&text),
                _ => {}
            }
        }
    })();
    crate::board::terminal::restore_terminal();
    Ok((result?, guide))
}

pub fn run(env: &Env, at: Step) -> Result<String> {
    let (next, guide) = ask(env, at)?;
    match next {
        Next::Board if std::env::var("HERDR_PANE_ID").is_ok_and(|pane| !pane.is_empty()) => {
            println!("{}\n", guide.text());
            return crate::lead::show();
        }
        Next::Board => println!("{}\n", guide.text()),
        Next::Connect => {
            println!("{}\n", guide.said(false));
            println!("{}", crate::hub::cli(Some("connect"), &[], None)?);
        }
        _ => return Ok(guide.text()),
    }
    crate::board::run(false)
}

pub fn first_run(claude_dir: &Path, terminal: bool) -> bool {
    terminal && !setup::kebbie_plugin_installed(claude_dir)
}

// A board opened before kebbie's Claude Code plugin is installed, as after herdr's own install, starts in the guide; quitting it opens the board as it is.
pub fn board(overlay: bool) -> Result<String> {
    let claude_dir = crate::agent::claude::home().unwrap_or_default();
    if first_run(&claude_dir, setup::in_terminal()) && crate::permit::user_only("kebbie setup").is_ok() {
        let machine = setup::machine()?;
        let env = Env::current(&machine)?;
        let (next, guide) = ask(&env, Step::Name)?;
        match next {
            Next::Connect => {
                println!("{}\n", guide.said(false));
                println!("{}", crate::hub::cli(Some("connect"), &[], None)?);
            }
            Next::Quit => {}
            _ => println!("{}\n", guide.text()),
        }
        if !matches!(next, Next::Quit) && std::env::var("HERDR_PANE_ID").is_ok_and(|pane| !pane.is_empty()) {
            let _ = crate::lead::restart(false, false);
        }
    }
    crate::board::run(overlay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Lab {
        dir: PathBuf,
        machine: Machine,
        log: PathBuf,
    }

    impl Drop for Lab {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl Lab {
        fn new(name: &str) -> Lab {
            let dir = crate::sh::test_dir(name);
            let (machine, log) = setup::tests::machine(&dir);
            Lab { dir, machine, log }
        }

        fn env(&self) -> Env<'_> {
            Env { machine: &self.machine, config: self.dir.join("kebbie/config.toml"), codex_home: Some(self.dir.join("codex")), kebbie: "/opt/kebbie".to_string(), state: self.dir.join("state") }
        }

        fn calls(&self) -> String {
            std::fs::read_to_string(&self.log).unwrap_or_default()
        }

        fn config(&self) -> String {
            std::fs::read_to_string(self.dir.join("kebbie/config.toml")).unwrap_or_default()
        }

        fn herdr(&self) -> String {
            std::fs::read_to_string(&self.machine.herdr_config).unwrap_or_default()
        }

        fn codex_config(&self) -> String {
            std::fs::read_to_string(self.dir.join("codex/config.toml")).unwrap_or_default()
        }
    }

    struct Fake {
        drawn: usize,
        ran: Vec<String>,
        installs: Option<PathBuf>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake { drawn: 0, ran: Vec::new(), installs: None }
        }
    }

    impl Hooks for Fake {
        fn progress(&mut self, _: &[(String, Outcome)]) {
            self.drawn += 1;
        }

        fn outside(&mut self, script: &str) -> Result<()> {
            self.ran.push(script.to_string());
            if let Some(bin) = &self.installs {
                std::fs::create_dir_all(bin).unwrap();
                crate::executable::write(bin.join("mindin"), "#!/bin/sh\n").unwrap();
            }
            Ok(())
        }
    }

    fn press(guide: &mut Guide, codes: &[KeyCode]) {
        for code in codes {
            guide.key(KeyEvent::from(*code));
        }
    }

    fn type_text(guide: &mut Guide, text: &str) {
        for character in text.chars() {
            guide.key(KeyEvent::from(KeyCode::Char(character)));
        }
    }

    fn render(guide: &Guide, width: u16, height: u16) -> String {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| guide.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height).map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }

    fn screen(guide: &Guide) -> String {
        render(guide, 230, 40)
    }

    fn plan(guide: &Guide) -> Vec<String> {
        guide.plan().iter().map(|row| format!("{} {} {} {}", row.group, row.verb, row.step, row.detail)).collect()
    }

    fn fresh(lab: &Lab) -> Guide {
        let mut facts = Facts::read(&lab.env());
        facts.name = "mac".to_string();
        facts.claude = Some("2.1.3".to_string());
        facts.codex = Some("0.163.0".to_string());
        Guide::new(facts, Step::Name)
    }

    fn found(source: &str, command: &str) -> Found {
        let answer = Probe { name: "mindin".to_string(), version: "0.14.0".to_string(), configured: true, backend: "server".to_string(), server: "https://vault.example.com".to_string(), setup: true, ..Probe::default() };
        let origin = match source {
            _ if source.starts_with("mindin") => Origin::Mindin,
            _ if source.starts_with("kebbie's") => Origin::Kept,
            _ if source.contains("vault plugin") => Origin::Plugin,
            _ => Origin::Other,
        };
        Found { source: source.to_string(), command: command.split(' ').map(str::to_string).collect(), answer: Some(answer), origin }
    }

    fn answering(found: Found, change: impl FnOnce(&mut Probe)) -> Found {
        let mut found = found;
        change(found.answer.as_mut().unwrap());
        found
    }

    fn vault_step(lab: &Lab, found: Vec<Found>, kept: Option<&str>) -> Guide {
        let mut guide = fresh(lab);
        guide.facts.vault_found = found;
        guide.facts.vault_command = kept.map(str::to_string);
        guide.answers = Answers::from(&guide.facts);
        guide.go(Step::Vault);
        guide
    }

    fn vault_rows(guide: &Guide) -> Vec<String> {
        plan(guide).into_iter().filter(|row| row.starts_with("mindin ") || row.contains(" vault ")).collect()
    }

    #[test]
    fn the_guide_has_six_steps_shows_each_agents_status_and_writes_nothing_before_apply() {
        let lab = Lab::new("guide-walk");
        let mut guide = fresh(&lab);
        let shown = screen(&guide);
        assert!(shown.contains("kebbie setup  step 1 of 6") && shown.contains("▸ 1 This machine") && shown.contains("· 2 Agents") && shown.contains("· 3 Vault") && shown.contains("· 4 Backlog") && shown.contains("· 5 Error signals") && shown.contains("· 6 Review and apply"), "{shown}");
        for gone in ["Plugin", "Login", "Approvals", "Updates", "herdr keys", "Vault for kebbie", "Vault in Codex"] {
            assert!(!shown.contains(gone), "{gone} is no step of its own: {shown}");
        }
        assert!(shown.contains("▸ name     mac▏") && shown.contains("from this computer's name") && shown.contains("type to change · ⏎ next · esc quit"), "{shown}");
        type_text(&mut guide, "devbox");
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!((guide.step, guide.answers.name.as_str()), (Step::Agents, "devbox"), "typing replaces the name it was given");
        guide.facts.codex_login = Login::Out;
        let shown = screen(&guide);
        assert!(shown.contains("✓ 1 This machine") && shown.contains("▸ [x] Claude Code  · 2.1.3 · signed in · kebbie's plugin missing · runs the lead, the reviewer and tasks"), "{shown}");
        assert!(shown.contains("[x] Codex  · 0.163.0 · not signed in: run codex login · kebbie's plugin missing · runs tasks with a codex profile"), "a found agent is ticked by default and its row says how to sign in: {shown}");
        assert!(shown.contains("Each one you tick gets kebbie's plugin and its approval rules when you apply."), "{shown}");
        assert!(!shown.contains("Install kebbie's plugin") && !shown.contains("I'll add it myself") && !shown.contains("s skip"), "status, not questions: {shown}");
        press(&mut guide, &[KeyCode::Char(' ')]);
        assert!(guide.answers.codex, "space on Claude Code changes nothing: it stays on");
        press(&mut guide, &[KeyCode::Down, KeyCode::Char(' ')]);
        assert!(!guide.answers.codex && screen(&guide).contains("[ ] Codex"), "Codex can be left out");
        press(&mut guide, &[KeyCode::Char(' ')]);
        guide.facts.codex = None;
        assert!(screen(&guide).contains("[ ] Codex  · not found"), "{}", screen(&guide));
        press(&mut guide, &[KeyCode::Char(' ')]);
        guide.facts.codex = Some("0.163.0".to_string());
        assert!(guide.answers.codex, "an agent that is not found cannot be ticked or unticked");
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Vault);
        assert!(screen(&guide).contains("( ) Hosted mindin    coming soon") && screen(&guide).contains("▸ (•) None             kebbie keeps tasks without one") && screen(&guide).contains("runs  mindin, installed when you apply"), "{}", screen(&guide));
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Backlog);
        assert!(screen(&guide).contains("[x] Local file  · an item file each in backlog/ beside your config · new items go here"), "{}", screen(&guide));
        press(&mut guide, &[KeyCode::Down, KeyCode::Char(' ')]);
        assert_eq!(guide.answers.chosen_sources(), [Source::Vault], "ticking the vault unticks the local file: new items go to one place");
        press(&mut guide, &[KeyCode::Down, KeyCode::Char(' ')]);
        assert_eq!(guide.answers.chosen_sources(), [Source::Vault, Source::Linear], "Linear sits beside either");
        press(&mut guide, &[KeyCode::Char(' '), KeyCode::Enter]);
        assert_eq!(guide.step, Step::Signals);
        assert!(screen(&guide).contains("never your code, paths or text") && screen(&guide).contains("A bug report with text is always shown to you first"));
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Review);
        let codex = format!("{}/codex", lab.dir.display());
        assert_eq!(plan(&guide), [
            "kebbie to do link ~/.local/bin/kebbie".to_string(),
            "kebbie to do machine \"devbox\"".to_string(),
            "kebbie skipped vault no vault; kebbie keeps tasks without one".to_string(),
            "kebbie to do backlog vault".to_string(),
            "kebbie to do signals off".to_string(),
            "kebbie to do keeper start it, so routines run without an open board".to_string(),
            "Claude Code to do plugin kebbie's plugin, through claude plugin install".to_string(),
            "Claude Code already approvals kebbie gives each Claude task its permissions as it starts it".to_string(),
            "Claude Code to do usage kebbie's statusLine in Claude Code's settings.json, for the board's usage bar".to_string(),
            format!("Codex to do plugin kebbie's plugin at {codex}/plugins/cache/kebbie-tasks/kebbie/local"),
            format!("Codex to do approvals {codex}/rules/kebbie.rules"),
            format!("herdr to do keys prefix+space, prefix+f, prefix+t at the end of {}", lab.machine.herdr_config.display()),
        ], "every write is a row of the plan, each tool's under its name, herdr's keys among them");
        let shown = screen(&guide);
        assert!(shown.contains("│ kebbie") && shown.contains("│ Claude Code") && shown.contains("│ Codex") && shown.contains("│ herdr"), "the rows are grouped by the tool they write: {shown}");
        assert!(shown.contains(&format!("runs: writes {codex}/rules/kebbie.rules, 45 lines")) && shown.contains("⏎ apply all · ← back · esc quit without writing"), "{shown}");
        assert_eq!((lab.config(), lab.herdr(), lab.calls()), (String::new(), String::new(), String::new()), "nothing is written or run before ⏎ on the last step");
        assert!(!lab.dir.join("codex").exists() && !lab.machine.home.join(".local/bin/kebbie").exists() && !lab.machine.claude_dir.join("settings.json").exists());
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Esc)), Next::Quit));
        assert_eq!(guide.text(), "Nothing written: kebbie setup runs the guide again whenever you want.");
    }

    #[test]
    fn apply_installs_each_ticked_agents_plugin_and_rules_then_a_rerun_starts_from_what_is_in_place() {
        let lab = Lab::new("guide-apply");
        let mut guide = fresh(&lab);
        guide.facts.repairs.retain(|repair| *repair != Repair::KeeperStart);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Enter, KeyCode::Enter, KeyCode::Down, KeyCode::Char(' '), KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(guide.step, Step::Review);
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Enter)), Next::Apply));
        let mut fake = Fake::new();
        let planned = guide.plan();
        let rows = apply(&planned, &lab.env(), &mut fake);
        let lines: Vec<String> = rows.iter().map(|(step, outcome)| format!("{} {step} {}", outcome.verb, outcome.detail)).collect();
        let codex = format!("{}/codex", lab.dir.display());
        assert_eq!(lines, [
            "done link ~/.local/bin/kebbie".to_string(),
            "done machine \"mac\"".to_string(),
            "skipped vault no vault; kebbie keeps tasks without one".to_string(),
            "done backlog vault".to_string(),
            "done signals off".to_string(),
            format!("done plugin installed kebbie@kebbie in Claude Code from {}", lab.machine.home.join(".local/share/kebbie/current").display()),
            "already approvals kebbie gives each Claude task its permissions as it starts it".to_string(),
            "done usage statusLine added to Claude Code's settings.json".to_string(),
            format!("done plugin kebbie's plugin at {codex}/plugins/cache/kebbie-tasks/kebbie/local"),
            format!("done approvals {codex}/rules/kebbie.rules"),
            "done keys prefix+space, prefix+f, prefix+t added, restart herdr to use them".to_string(),
        ], "each line says what it did in a few words");
        assert_eq!(fake.drawn, 11, "each line is drawn as it finishes");
        assert!(fake.ran.is_empty(), "nothing runs outside the screen without the mindin install");
        assert_eq!(lab.config(), "[machine]\nname = \"mac\"\n\n[board.backlog]\nsources = [\"vault\"]\n\n[report]\nsignals = false\n", "setup never writes [hub] nor how agents update");
        assert_eq!(lab.herdr().matches("[[keys.command]]").count(), 3);
        assert!(lab.dir.join("codex/plugins/cache/kebbie-tasks/kebbie/local/hooks/hooks.json").is_file() && lab.dir.join("codex/rules/kebbie.rules").is_file(), "a ticked Codex gets kebbie's plugin and its rules without a question");
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(lab.machine.claude_dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(settings["statusLine"]["command"], crate::doctor::STATUS_LINE_COMMAND);
        assert!(Facts::read(&lab.env()).repairs.iter().all(|repair| *repair != Repair::StatusLine), "a fix applied is no row the next time");
        let mut rerun = Guide::new(Facts::read(&lab.env()), Step::Name);
        rerun.facts.codex = Some("0.163.0".to_string());
        rerun.answers.codex = true;
        assert_eq!(rerun.answers.name, "mac");
        assert_eq!(rerun.answers.chosen_sources(), [Source::Vault]);
        let shown = screen(&rerun);
        assert!(shown.contains("✓ in place: machine.name is \"mac\"") && shown.lines().any(|line| line.contains("✓") && line.contains("Error signals")), "the rail marks what is in place: {shown}");
        rerun.go(Step::Review);
        let plan = plan(&rerun);
        for row in ["kebbie already machine \"mac\"", "herdr already keys kebbie's keys are bound in herdr", "kebbie already backlog vault", "kebbie already signals off", "Claude Code already plugin kebbie's plugin"] {
            assert!(plan.iter().any(|line| line == row), "{row} missing from {plan:?}");
        }
        assert!(plan.iter().any(|line| line.starts_with("Codex already plugin")) && plan.iter().any(|line| line.starts_with("Codex already approvals")), "{plan:?}");
    }

    #[test]
    fn a_stale_codex_plugin_shows_out_of_date_on_agents_and_apply_rewrites_it() {
        let lab = Lab::new("guide-stale-codex");
        let plugins = lab.machine.claude_dir.join("plugins");
        std::fs::write(plugins.join("installed_plugins.json"), json!({ "plugins": { "kebbie@kebbie": [{ "version": "0.27.0" }] } }).to_string()).unwrap();
        crate::agent::codex::install(&lab.dir.join("codex"), "/elsewhere/kebbie").unwrap();
        let mut guide = fresh(&lab);
        guide.go(Step::Agents);
        assert!(screen(&guide).contains("[x] Codex  · 0.163.0") && screen(&guide).contains("kebbie's plugin out of date"), "{}", screen(&guide));
        let rows = apply(&guide.plan(), &lab.env(), &mut Fake::new());
        assert!(rows.iter().any(|(step, outcome)| step == "plugin" && outcome.verb == "done" && outcome.detail.contains("kebbie-tasks")), "{rows:?}");
        assert_eq!(crate::agent::codex::installed(&lab.dir.join("codex"), "/opt/kebbie"), Installed::Current);
        let mut rerun = Guide::new(Facts::read(&lab.env()), Step::Agents);
        rerun.facts.codex = Some("0.163.0".to_string());
        assert!(screen(&rerun).contains("0.163.0 · sign-in not checked · kebbie's plugin current"), "{}", screen(&rerun));
    }

    #[test]
    fn a_machine_with_kebbies_codex_plugin_under_kebbie_at_kebbie_moves_it_on_apply() {
        let lab = Lab::new("guide-old-id-codex");
        let home = lab.dir.join("codex");
        crate::agent::codex::install(&home, "/opt/kebbie").unwrap();
        std::fs::create_dir_all(home.join("plugins/cache/kebbie/kebbie")).unwrap();
        std::fs::rename(home.join("plugins/cache/kebbie-tasks/kebbie/local"), home.join("plugins/cache/kebbie/kebbie/local")).unwrap();
        let guide = fresh(&lab);
        assert!(guide.answers.codex);
        let rows = apply(&guide.plan(), &lab.env(), &mut Fake::new());
        assert!(rows.iter().any(|(step, outcome)| step == "plugin" && outcome.verb == "done" && outcome.detail.contains("kebbie-tasks")), "{rows:?}");
        assert!(!home.join("plugins/cache/kebbie/kebbie/local").exists());
    }

    #[test]
    fn unticking_codex_takes_kebbies_plugin_out_of_codex() {
        let lab = Lab::new("guide-codex-off");
        crate::agent::codex::install(&lab.dir.join("codex"), "/opt/kebbie").unwrap();
        let mut guide = fresh(&lab);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Down, KeyCode::Char(' ')]);
        assert!(!guide.answers.codex);
        assert_eq!(plan(&guide).iter().filter(|row| row.starts_with("Codex ")).cloned().collect::<Vec<_>>(), ["Codex to do plugin kebbie's plugin out of Codex"]);
    }

    #[test]
    fn s_skips_the_vault_backlog_and_signals_and_back_keeps_what_was_chosen() {
        let lab = Lab::new("guide-skip");
        let mut guide = fresh(&lab);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Char('s')]);
        assert_eq!(guide.step, Step::Agents, "the agents have no skip: each one ticked is set up");
        guide.facts.vault_found = vec![found("mindin on PATH", "mindin")];
        guide.answers = Answers::from(&guide.facts);
        guide.go(Step::Vault);
        press(&mut guide, &[KeyCode::Down, KeyCode::Down, KeyCode::Char('s')]);
        assert_eq!((guide.step, guide.answers.vault.chosen()), (Step::Backlog, Place::Server), "a skip puts the step back to what the machine has");
        press(&mut guide, &[KeyCode::Down, KeyCode::Char(' '), KeyCode::Left]);
        assert_eq!(guide.step, Step::Vault);
        press(&mut guide, &[KeyCode::Left]);
        assert_eq!(guide.step, Step::Agents, "← goes back one step");
        guide.go(Step::Backlog);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Char('s')]);
        assert_eq!(guide.step, Step::Review);
        assert_eq!(guide.answers.chosen_sources(), [Source::Vault], "← and ⏎ keep the boxes ticked");
        let plan = plan(&guide);
        assert!(plan.contains(&"kebbie skipped vault stays as it is".to_string()) && plan.contains(&"kebbie skipped signals stay off".to_string()) && plan.contains(&"kebbie to do backlog vault".to_string()), "{plan:?}");
        assert!(plan.iter().all(|row| !row.ends_with(" vault [mcp_servers.vault]") && !row.contains("the vault plugin")), "a skipped vault adds it to no agent: {plan:?}");
        press(&mut guide, &[KeyCode::Left]);
        assert_eq!(guide.step, Step::Signals);
        let mut name = fresh(&lab);
        type_text(&mut name, "s");
        assert_eq!((name.step, name.answers.name.as_str()), (Step::Name, "s"), "s types in a text field");
        press(&mut name, &[KeyCode::Backspace, KeyCode::Enter]);
        assert!(screen(&name).contains("machine.name: must be a non-empty string"), "{}", screen(&name));
    }

    #[test]
    fn a_taken_herdr_key_is_named_on_review_and_the_free_ones_are_added() {
        let lab = Lab::new("guide-taken");
        std::fs::create_dir_all(lab.machine.herdr_config.parent().unwrap()).unwrap();
        std::fs::write(&lab.machine.herdr_config, "[keys]\nnew_tab = \"prefix+t\"\n").unwrap();
        let mut guide = fresh(&lab);
        guide.go(Step::Review);
        let keys: Vec<Planned> = guide.plan().into_iter().filter(|row| row.step == "keys").collect();
        assert_eq!(format!("{} {}", keys[0].verb, keys[0].detail), format!("to do prefix+space, prefix+f at the end of {}; the board unbound: prefix+t is new_tab's, prefix+y is free", lab.machine.herdr_config.display()));
        let rows = apply(&keys, &lab.env(), &mut Fake::new());
        assert_eq!(format!("{} {}", rows[0].1.verb, rows[0].1.detail), "done prefix+space, prefix+f added, restart herdr to use them");
        assert!(lab.herdr().starts_with("[keys]\nnew_tab = \"prefix+t\"\n\n[[keys.command]]") && !lab.herdr().contains("kebbie.board"), "only the end of the file changes: {}", lab.herdr());
    }

    #[test]
    fn the_vault_step_chooses_where_the_vault_lives_and_two_commands_for_one_server_show_once() {
        let lab = Lab::new("guide-vault");
        std::fs::create_dir_all(lab.dir.join("codex")).unwrap();
        std::fs::write(lab.dir.join("codex/config.toml"), "# mine\nmodel_reasoning_effort = \"high\"\n").unwrap();
        let old = |probe: &mut Probe| {
            probe.name = "notes-vault-mcp".to_string();
            probe.version = "0.13.0".to_string();
            probe.server = "https://notes.example.com".to_string();
            probe.setup = false;
        };
        let kept = "uv run -q --project /src/notes-vault-mcp notes-vault-mcp";
        let mut guide = vault_step(&lab, vec![answering(found("kebbie's [vault] command", kept), old), answering(found("the Claude Code vault plugin vault@gronare", "uvx notes-vault-mcp==0.13.0"), old)], Some(kept));
        let shown = screen(&guide);
        for line in [
            "( ) Hosted mindin    coming soon",
            "▸ (•) Your own server  https://notes.example.com · signed in",
            "address   https://notes.example.com",
            "( ) S3 bucket        endpoint, bucket",
            "( ) Local folder     path",
            "( ) None             kebbie keeps tasks without one",
            "runs  uv run -q --project /src/notes-vault-mcp notes-vault-mcp · kebbie's [vault] command",
        ] {
            assert!(shown.contains(line), "{line:?} missing from:\n{shown}");
        }
        assert_eq!(shown.matches("notes.example.com · signed in").count(), 1, "one place, however many commands answer for it: {shown}");
        assert!(!shown.contains("uvx notes-vault-mcp"), "the command is kebbie's choice, not a row: {shown}");
        press(&mut guide, &[KeyCode::Up]);
        assert_eq!(guide.answers.vault.chosen(), Place::None, "Hosted mindin cannot be chosen while it is coming soon, so ↑ wraps past it");
        press(&mut guide, &[KeyCode::Down]);
        assert_eq!(guide.answers.vault.chosen(), Place::Server);
        guide.go(Step::Review);
        assert_eq!(vault_rows(&guide), [
            "mindin already setup your own server https://notes.example.com · signed in".to_string(),
            format!("kebbie already vault vault.command = \"{kept}\""),
            "Claude Code to do vault the vault plugin vault@gronare".to_string(),
            format!("Codex to do vault [mcp_servers.vault] in {}/codex/config.toml", lab.dir.display()),
        ], "the place in use runs nothing");
        guide.go(Step::Vault);
        press(&mut guide, &[KeyCode::Down]);
        type_text(&mut guide, "https://notes.example.org");
        assert!(screen(&guide).contains("▸ address   https://notes.example.org▏"), "typing replaces the address it was given: {}", screen(&guide));
        assert!(screen(&guide).contains("runs  mindin, installed when you apply: uv run -q --project /src/notes-vault-mcp notes-vault-mcp has no setup"), "{}", screen(&guide));
        press(&mut guide, &[KeyCode::Enter]);
        guide.go(Step::Review);
        let rows = vault_rows(&guide);
        assert_eq!(rows[..3], [
            format!("mindin to do install mindin first, since {kept} has no setup"),
            "mindin to do setup your own server https://notes.example.org".to_string(),
            "kebbie to do vault vault.command = the mindin it installs".to_string(),
        ], "a vault command older than mindin cannot move the vault: {rows:?}");
        assert!(screen(&guide).contains("runs: mindin setup --server https://notes.example.org in this terminal; it signs this machine in through the browser when it is not yet"), "{}", screen(&guide));
        let vault: Vec<Planned> = guide.plan().into_iter().filter(|row| row.group == MINDIN || row.step == "vault").collect();
        let mut fake = Fake { installs: Some(lab.machine.home.join(".local/bin")), ..Fake::new() };
        let done = apply(&vault, &lab.env(), &mut fake);
        let mindin = lab.machine.home.join(".local/bin/mindin").display().to_string();
        assert_eq!(fake.ran, [MINDIN_INSTALL.to_string(), format!("{mindin} setup --server https://notes.example.org")], "the installer, then mindin's setup, each outside the guide's screen");
        assert!(done.iter().all(|(_, outcome)| outcome.verb != "failed"), "{done:?}");
        assert_eq!(lab.config(), format!("[vault]\ncommand = \"{mindin}\"\n"));
        assert!(lab.codex_config().starts_with("# mine\nmodel_reasoning_effort = \"high\"\n") && lab.codex_config().contains(&format!("command = \"{mindin}\"")), "{}", lab.codex_config());
    }

    #[test]
    fn a_mindin_that_answers_is_the_command_and_moves_the_vault_without_an_install() {
        let lab = Lab::new("guide-vault-mindin");
        let mut guide = vault_step(&lab, vec![found("the Claude Code vault plugin vault@gronare", "uvx mindin==0.14.0"), found("mindin on PATH", "mindin")], None);
        assert!(screen(&guide).contains("runs  mindin · mindin on PATH"), "a mindin found comes before the plugin's: {}", screen(&guide));
        press(&mut guide, &[KeyCode::Down, KeyCode::Down]);
        assert_eq!(guide.answers.vault.chosen(), Place::S3, "the address field sits between the server and the next place");
        press(&mut guide, &[KeyCode::Up, KeyCode::Up, KeyCode::Up]);
        assert_eq!(guide.answers.vault.chosen(), Place::Folder, "↑ from S3 chooses the server, then wraps past the closed Hosted mindin to None");
        press(&mut guide, &[KeyCode::Enter]);
        assert!(guide.step == Step::Vault && screen(&guide).contains("Local folder needs a path"), "{}", screen(&guide));
        press(&mut guide, &[KeyCode::Down]);
        type_text(&mut guide, "/srv/my notes");
        press(&mut guide, &[KeyCode::Enter]);
        guide.go(Step::Review);
        assert_eq!(vault_rows(&guide)[..2], ["mindin to do setup the folder /srv/my notes".to_string(), "kebbie to do vault vault.command = \"mindin\"".to_string()]);
        let mut fake = Fake::new();
        apply(&guide.plan().into_iter().filter(|row| row.group == MINDIN).collect::<Vec<_>>(), &lab.env(), &mut fake);
        assert_eq!(fake.ran, ["mindin setup --folder '/srv/my notes'"], "no installer, and a path with a space reaches the shell whole");
        press(&mut guide, &[KeyCode::Left, KeyCode::Left, KeyCode::Left, KeyCode::Down, KeyCode::Down]);
        assert_eq!(guide.answers.vault.chosen(), Place::None);
        guide.go(Step::Review);
        assert_eq!(vault_rows(&guide), ["kebbie skipped vault no vault; kebbie keeps tasks without one"], "None runs nothing in the vault and adds it to no agent");
        let mut kept = vault_step(&lab, vec![found("kebbie's [vault] command", "/opt/bin/mindin")], Some("/opt/bin/mindin"));
        press(&mut kept, &[KeyCode::Up]);
        kept.go(Step::Review);
        let without: Vec<Planned> = kept.plan().into_iter().filter(|row| row.group == MINDIN || row.step == "vault").collect();
        assert_eq!(without.iter().map(|row| row.act.clone()).collect::<Vec<_>>(), [Act::VaultCommand(None)], "None takes kebbie's line out and nothing else");
    }

    #[test]
    fn an_s3_vault_is_chosen_as_it_is_and_a_new_bucket_runs_setup_with_its_flags() {
        let lab = Lab::new("guide-vault-s3");
        let s3 = answering(found("kebbie's [vault] command", "uvx mindin"), |probe| {
            probe.backend = "s3".to_string();
            probe.server.clear();
        });
        let mut guide = vault_step(&lab, vec![s3], Some("uvx mindin"));
        let shown = screen(&guide);
        for line in ["( ) Your own server  address", "▸ (•) S3 bucket        in use", "endpoint  the S3 or MinIO address", "bucket", "prefix    optional", "region    optional, us-east-1 when empty", "the access key and the secret are asked as you apply, the secret hidden", "runs  uvx mindin · kebbie's [vault] command"] {
            assert!(shown.contains(line), "{line:?} missing from:\n{shown}");
        }
        guide.go(Step::Review);
        assert_eq!(vault_rows(&guide)[0], "mindin already setup an S3 bucket", "the bucket the vault does not name stays as it is");
        guide.go(Step::Vault);
        press(&mut guide, &[KeyCode::Down]);
        type_text(&mut guide, "https://s3.example.com");
        press(&mut guide, &[KeyCode::Down]);
        type_text(&mut guide, "notes");
        press(&mut guide, &[KeyCode::Down]);
        type_text(&mut guide, "team");
        press(&mut guide, &[KeyCode::Enter]);
        guide.go(Step::Review);
        assert_eq!(vault_rows(&guide)[..2], ["mindin to do setup the bucket notes/team at https://s3.example.com".to_string(), "kebbie already vault vault.command = \"uvx mindin\"".to_string()]);
        assert!(screen(&guide).contains("runs: uvx mindin setup --s3 notes --endpoint https://s3.example.com --prefix team in this terminal; it asks for the access key and the secret, hidden"), "{}", screen(&guide));
    }

    #[test]
    fn with_nothing_found_a_place_chosen_installs_mindin_first() {
        let lab = Lab::new("guide-mindin");
        let mut guide = fresh(&lab);
        guide.go(Step::Vault);
        let shown = screen(&guide);
        assert!(shown.contains("( ) Your own server  address") && shown.contains("▸ (•) None") && shown.contains("runs  mindin, installed when you apply"), "{shown}");
        press(&mut guide, &[KeyCode::Up, KeyCode::Up, KeyCode::Up, KeyCode::Down]);
        assert_eq!((guide.answers.vault.chosen(), guide.typed()), (Place::Server, Some(Field::Address)));
        type_text(&mut guide, "https://vault.example.com");
        press(&mut guide, &[KeyCode::Enter]);
        guide.go(Step::Review);
        let rows = plan(&guide);
        assert_eq!(rows[..2], ["mindin to do install mindin, which then runs the vault".to_string(), "mindin to do setup your own server https://vault.example.com".to_string()], "the install and the setup come first, as the rest points at them");
        assert!(rows.contains(&"kebbie to do vault vault.command = the mindin it installs".to_string()) && rows.contains(&"Claude Code to do vault the vault plugin vault@gronare".to_string()), "{rows:?}");
        assert!(screen(&guide).contains(&format!("runs: {MINDIN_INSTALL} first, in this terminal")), "{}", screen(&guide));
        let vault: Vec<Planned> = guide.plan().into_iter().filter(|row| row.group == MINDIN || row.step == "vault").collect();
        let none = apply(&vault, &lab.env(), &mut Fake::new());
        assert!(none[0].1.verb == "failed" && none[1].1.verb == "skipped" && none[2].1.detail.contains("mindin is not installed"), "an installer that leaves no mindin runs no setup and points nothing at it: {none:?}");
        let yes: Vec<String> = Guide::new(guide.facts.clone(), Step::Review).plan().iter().map(|row| row.step.to_string()).collect();
        assert!(!yes.contains(&"setup".to_string()), "the default on a bare machine is no vault: {yes:?}");
    }

    #[test]
    fn hosted_mindin_opens_by_its_address_alone() {
        assert_eq!(HOSTED_VAULT, None, "hosted mindin is not live yet");
        assert!(!Place::Hosted.open() && Place::ALL[0] == Place::Hosted, "it is listed first, closed");
        assert!(same_url("https://Mindin.example.com/", "https://mindin.example.com") && !same_url("", ""), "an empty address is no place");
    }

    #[test]
    fn a_rules_file_in_place_reads_already_and_one_of_other_rules_is_rewritten() {
        let lab = Lab::new("guide-codex-rules");
        crate::agent::codex::install(&lab.dir.join("codex"), "/opt/kebbie").unwrap();
        let guide = fresh(&lab);
        let rules = guide.facts.rules.clone();
        crate::agent::codex_setup::write_rules(&lab.dir.join("codex"), &rules).unwrap();
        let mut guide = fresh(&lab);
        guide.go(Step::Review);
        assert!(plan(&guide).iter().any(|row| row.starts_with("Codex already approvals")), "{:?}", plan(&guide));
        crate::agent::codex_setup::write_rules(&lab.dir.join("codex"), "prefix_rule(pattern=[\"kebbie\"], decision=\"allow\")\n").unwrap();
        let guide = fresh(&lab);
        assert!(plan(&guide).iter().any(|row| row.starts_with("Codex to do approvals")));
        let rows = apply(&guide.plan(), &lab.env(), &mut Fake::new());
        assert!(rows.iter().any(|(step, outcome)| step == "approvals" && outcome.verb == "done"), "{rows:?}");
        assert_eq!(std::fs::read_to_string(lab.dir.join("codex/rules/kebbie.rules")).unwrap(), rules);
    }

    #[test]
    fn kebbie_setup_codex_without_a_terminal_names_what_yes_also_writes_and_writes_it_with_yes() {
        let lab = Lab::new("guide-codex-offers");
        let named = offers(&lab.env(), Agent::Codex);
        assert_eq!(named[0], "kebbie setup codex --yes also writes:");
        assert!(named.iter().any(|line| line.contains("approvals") && line.contains("rules/kebbie.rules")) && named.iter().all(|line| !line.contains("plugin")), "{named:?}");
        assert!(!lab.dir.join("codex/rules").exists(), "naming them writes nothing");
        let applied = apply_agent(&lab.env(), Agent::Codex);
        assert!(applied.iter().any(|line| line.starts_with("done") && line.contains("rules/kebbie.rules")), "{applied:?}");
        assert!(lab.dir.join("codex/rules/kebbie.rules").is_file());
        assert!(offers(&lab.env(), Agent::Claude).iter().all(|line| !line.contains("vault")), "Claude Code without a vault gets no vault plugin");
    }

    #[test]
    fn a_doctor_warning_that_needs_the_users_choice_is_no_row() {
        let lab = Lab::new("guide-repairs");
        std::fs::create_dir_all(lab.dir.join("kebbie")).unwrap();
        std::fs::write(lab.dir.join("kebbie/config.toml"), "[task]\nsandbox = false\n").unwrap();
        std::fs::write(lab.machine.claude_dir.join("settings.json"), "{\"statusLine\": {\"type\": \"command\", \"command\": \"my-line\"}}").unwrap();
        let mut guide = fresh(&lab);
        assert_eq!(guide.facts.repairs, [Repair::KeeperStart], "a statusLine of the user's own and a task.sandbox = false they chose stay with kebbie doctor");
        guide.go(Step::Review);
        assert!(plan(&guide).contains(&"kebbie to do keeper start it, so routines run without an open board".to_string()));
        assert!(plan(&guide).iter().all(|row| !row.contains("sandbox") && !row.contains("statusLine")), "{:?}", plan(&guide));
    }

    #[test]
    fn error_signals_start_on_no_and_only_the_review_writes_the_answer() {
        let lab = Lab::new("guide-signals-consent");
        let mut guide = fresh(&lab);
        assert!(!guide.facts.signals_set && !guide.facts.signals);
        guide.go(Step::Signals);
        let shown = screen(&guide);
        assert_eq!(guide.answers.signals, 1, "No is the answer until you pick yes");
        assert!(shown.contains("Off until you say yes") && shown.contains("Yes, turn them on") && shown.contains("No, keep them off") && shown.contains("a random id made for this install"), "{shown}");
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!((guide.step, guide.answers.signals), (Step::Review, 1), "⏎ takes the No");
        let review = screen(&guide);
        assert!(plan(&guide).contains(&"kebbie to do signals off".to_string()) && review.lines().any(|line| line.contains("signals") && line.contains("off") && !line.contains("Error signals")), "a No is written too and listed on the review: {review}");
        guide.answers.signals = 0;
        let rows: Vec<Planned> = guide.plan().into_iter().filter(|row| row.step == "signals").collect();
        assert_eq!(rows.iter().map(|row| format!("{} {}", row.verb, row.detail)).collect::<Vec<_>>(), ["to do on"]);
        assert!(lab.config().is_empty(), "nothing is written before the review applies");
        apply(&rows, &lab.env(), &mut Fake::new());
        assert!(lab.config().contains("[report]\nsignals = true\n"), "{}", lab.config());
    }

    #[test]
    fn yes_never_turns_error_signals_on_nor_answers_for_you() {
        let lab = Lab::new("guide-yes-signals");
        let lines: Vec<String> = without_asking(&lab.env(), Asking::Yes).iter().map(|(step, outcome)| outcome.line(step)).collect();
        let signals = lines.iter().find(|line| line.split_whitespace().nth(1) == Some("signals")).cloned().unwrap_or_default();
        assert!(signals.starts_with("skipped") && signals.contains("stay off until you say yes in kebbie setup"), "{lines:?}");
        assert!(!lab.config().contains("signals"), "{}", lab.config());
        assert!(!crate::config::load_machine(&lab.env().config).unwrap().report.signals());
    }

    #[test]
    fn yes_takes_the_defaults_and_prints_each_write_while_without_a_terminal_nothing_is_written_to_herdr() {
        let lab = Lab::new("guide-yes");
        let lines: Vec<String> = without_asking(&lab.env(), Asking::Yes).iter().map(|(step, outcome)| outcome.line(step)).collect();
        let line = |step: &str| lines.iter().find(|line| line.split_whitespace().nth(1) == Some(step)).cloned().unwrap_or_default();
        assert!(line("link").starts_with("done") && line("claude").contains("installed kebbie@kebbie") && lines.iter().any(|line| line.contains("claude usage")), "an agent's lines carry its name: {lines:?}");
        assert!(line("keys").starts_with("done") && line("keys").contains("added prefix+space, prefix+f, prefix+t to") && line("keys").contains("restart herdr so it reads the new keys"), "{lines:?}");
        assert!(lab.config().starts_with("[machine]\nname = "), "the default name is written and printed: {}", lab.config());
        assert!(lines.iter().all(|line| !line.contains("mindin")), "--yes never runs an installer: {lines:?}");
        let written = lab.herdr();
        let again: Vec<String> = without_asking(&lab.env(), Asking::Yes).iter().map(|(step, outcome)| outcome.line(step)).collect();
        assert!(again.iter().find(|line| line.contains(" keys ")).unwrap().starts_with("already"), "{again:?}");
        assert_eq!(lab.herdr(), written);
        let closed = Lab::new("guide-closed");
        let lines: Vec<String> = without_asking(&closed.env(), Asking::Closed).iter().map(|(step, outcome)| outcome.line(step)).collect();
        assert!(lines[2].starts_with("skipped") && lines[2].contains("no terminal to ask in") && lines[2].contains("kebbie setup --yes"), "{lines:?}");
        assert_eq!((closed.herdr(), closed.config()), (String::new(), String::new()));
        assert!(!closed.machine.claude_dir.join("settings.json").exists(), "a run without a terminal edits no file of the user's it did not show");
        crate::agent::codex::install(&closed.dir.join("codex"), "/old/kebbie").unwrap();
        let lines: Vec<String> = without_asking(&closed.env(), Asking::Closed).iter().map(|(step, outcome)| outcome.line(step)).collect();
        assert!(lines.iter().any(|line| line.starts_with("done") && line.contains("kebbie's Codex plugin updated: bin/kebbie-hook")), "an installed plugin follows kebbie: {lines:?}");
    }

    #[test]
    fn a_run_that_asks_nothing_names_the_vault_command_it_found_and_writes_none() {
        let lab = Lab::new("guide-vault-unasked");
        let mut facts = fresh(&lab).facts;
        let config = lab.dir.join("kebbie/config.toml");
        assert_eq!(vault_unasked(&facts, &config), None);
        facts.vault_found = vec![found("mindin on PATH", "mindin"), found("Codex's [mcp_servers.notes]", "uvx mindin")];
        let said = vault_unasked(&facts, &config).unwrap();
        assert!(said.verb == "skipped" && said.detail.starts_with("mindin on PATH runs the vault: kebbie setup --yes sets vault.command = \"mindin\" under [vault] in "), "{said:?}");
        facts.vault_command = Some("uvx mindin".to_string());
        assert_eq!(vault_unasked(&facts, &config).unwrap().verb, "already");
        assert_eq!(lab.config(), "");
    }

    #[test]
    fn where_the_vault_lives_reads_from_its_status() {
        let lab = Lab::new("guide-vault-text");
        let mut facts = fresh(&lab).facts;
        facts.vault_found = vec![found("Codex's [mcp_servers.notes]", "uvx mindin"), found("the Claude Code vault plugin vault@gronare", "uvx mindin==0.14.0")];
        assert_eq!(facts.runner().map(Found::shown), Some("uvx mindin==0.14.0".to_string()), "the plugin's command comes before one found elsewhere");
        let mut server = found("x", "mindin");
        assert_eq!(lives(&server), "server https://vault.example.com · signed in");
        let probe = server.answer.as_mut().unwrap();
        probe.configured = false;
        assert_eq!(lives(&server), "server https://vault.example.com · not signed in: run mindin login");
        let at = |backend: &str, configured: bool| lives(&Found { answer: Some(crate::vault_cli::Probe { backend: backend.to_string(), configured, ..crate::vault_cli::Probe::default() }), ..found("x", "mindin") });
        assert_eq!((at("local", true), at("s3", true), at("", false)), ("a local folder".to_string(), "an S3 bucket".to_string(), "no vault set up yet: mindin setup chooses one".to_string()));
        assert_eq!(lives(&Found { answer: None, ..found("x", "mindin") }), "did not answer");
    }

    fn check(level: crate::doctor::Level, name: &str, fact: &str, fix: &[&str]) -> crate::doctor::Check {
        crate::doctor::Check::new(level, name, fact, fix.iter().map(|fix| fix.to_string()).collect())
    }

    #[test]
    fn the_end_names_the_path_line_and_the_file_for_the_users_shell_when_kebbie_is_not_on_path() {
        use crate::doctor::{Level, NOT_ON_PATH, ON_PATH};
        let missing = [check(Level::Warn, ON_PATH, NOT_ON_PATH, &[])];
        let step = |shell: &str| ending(None, &missing, shell)[4].clone();
        assert_eq!(step("/bin/zsh"), "  PATH                 add export PATH=\"$HOME/.local/bin:$PATH\" to ~/.zshrc, then open a new terminal");
        assert!(step("/usr/bin/fish").contains("fish_add_path ~/.local/bin to ~/.config/fish/config.fish"));
        assert!(step("").contains("to your shell's profile"));
        let found = [check(Level::Ok, ON_PATH, "~/.local/bin/kebbie → kebbie", &[])];
        assert!(!ending(None, &found, "/bin/zsh").iter().any(|line| line.contains("PATH")), "on PATH, no line");
    }

    #[test]
    fn the_end_counts_the_doctor_and_lists_only_what_needs_the_user_in_two_columns() {
        use crate::doctor::Level;
        let clean = ending(Some(Login::In), &[check(Level::Ok, "git", "/usr/bin/git", &[]), check(Level::Ok, "jq", "/usr/bin/jq", &[])], "/bin/zsh");
        assert_eq!(clean, ["", "Setup done · doctor: 2 ok", "", "Next steps", "  prefix+space         tell the lead what you want done"], "the lead is the one step left when nothing else is");
        let unchecked = ending(Some(Login::In), &[check(Level::Ok, "git", "/usr/bin/git", &[]), check(Level::Skip, "claude login", "login not checked", &[])], "/bin/zsh");
        assert_eq!(unchecked[1], "Setup done · doctor: 1 ok", "a check that got no answer is neither ok nor to look at");
        let checks = [check(Level::Ok, "git", "/usr/bin/git", &[]), check(Level::Fail, "jq", "jq is missing", &["macOS: brew install jq"]), check(Level::Warn, "status line", "statusLine runs \"my-line\"", &["have your statusLine command pipe its stdin to `kebbie statusline`"])];
        assert_eq!(ending(Some(Login::Out), &checks, "/bin/zsh"), [
            "",
            "Setup done · doctor: 1 ok, 2 to look at",
            "",
            "Next steps",
            "  codex login          Codex is not logged in, so codex tasks cannot start",
            "  kebbie doctor        says what is left and how to fix it",
            "  prefix+space         tell the lead what you want done",
        ], "a fix setup cannot make is one line to kebbie doctor, never its text");
        assert!(ending(Some(Login::Unknown), &[], "/bin/zsh").contains(&"  codex login          if you have not yet, so codex tasks can start".to_string()));
    }

    #[test]
    fn the_guide_opens_at_a_step_and_the_review_shows_each_line_as_it_is_applied() {
        let lab = Lab::new("guide-open");
        let mut guide = Guide::new(Facts::read(&lab.env()), Step::Vault);
        assert_eq!(guide.step, Step::Vault);
        assert!(screen(&guide).contains("▸ 3 Vault"), "{}", screen(&guide));
        guide.go(Step::Review);
        guide.applied = Some(vec![("link".to_string(), Outcome::new("done", "~/.local/bin/kebbie"))]);
        let shown = screen(&guide);
        assert!(shown.contains("done     link       ~/.local/bin/kebbie") && shown.contains("to do    machine ") && shown.contains("applying…") && !shown.contains("Your board on your phone"), "{shown}");
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Char('h'))), Next::Stay), "keys wait while it applies");
    }

    fn finished(lab: &Lab) -> Guide {
        let mut guide = fresh(lab);
        guide.facts.repairs.clear();
        guide.visited = [true; Step::ALL.len()];
        guide.facts.name = "Hetzner devbox".to_string();
        guide.facts.name_set = true;
        guide.answers.name = guide.facts.name.clone();
        guide.go(Step::Review);
        guide.applied = Some(vec![
            ("link".to_string(), Outcome::new("done", "~/.local/bin/kebbie")),
            ("plugin".to_string(), Outcome::new("done", "kebbie in Claude Code and Codex")),
            ("machine".to_string(), Outcome::new("done", "\"Hetzner devbox\"")),
            ("keys".to_string(), Outcome::new("done", "prefix+f added, herdr reloaded")),
            ("backlog".to_string(), Outcome::new("already", "vault, linear")),
            ("signals".to_string(), Outcome::new("already", "on")),
        ]);
        let ok: Vec<crate::doctor::Check> = (0..36).map(|at| check(crate::doctor::Level::Ok, &format!("check {at}"), "fine", &[])).collect();
        guide.ending = ending(Some(Login::Out), &ok, "/bin/zsh");
        guide.done = true;
        guide
    }

    #[test]
    fn the_finished_guide_offers_the_hub_in_a_box_and_h_connects_while_any_other_key_opens_the_board() {
        let lab = Lab::new("guide-finished");
        let mut guide = finished(&lab);
        let shown = screen(&guide);
        for line in [
            "Setup done · doctor: 36 ok",
            "codex login          Codex is not logged in, so codex tasks cannot start",
            "prefix+space         tell the lead what you want done",
            "╭ Your board on your phone ",
            "│ h  connect now     later: kebbie hub connect, or H on the board",
            "│ kebbie.dev/hub",
            "any key opens your board",
        ] {
            assert!(shown.contains(line), "{line:?} missing from:\n{shown}");
        }
        assert!(!shown.contains("already"), "the done list names what changed: {shown}");
        assert!(shown.contains("Follow tasks and answer questions from anywhere, end-to-end") && shown.contains("Free on hub.kebbie.dev, or run your own."), "{shown}");
        assert!(guide.text().ends_with("  prefix+space         tell the lead what you want done\n\nYour board on your phone: kebbie hub connect, or H on the board\nkebbie.dev/hub"), "{}", guide.text());
        assert!(!guide.said(false).contains("kebbie.dev/hub"), "the connect that h runs follows the summary instead");
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Char('h'))), Next::Connect));
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Char('x'))), Next::Board), "any other key opens the board");
        guide.facts.hub = Some("hub.kebbie.dev".to_string());
        let shown = screen(&guide);
        assert!(!shown.contains("Your board on your phone") && shown.contains("any key opens your board"), "a connected machine is offered nothing: {shown}");
        assert!(!guide.text().contains("kebbie.dev/hub"));
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Char('h'))), Next::Board), "h is any other key once connected");
    }

    #[test]
    fn a_board_opened_before_setup_starts_in_the_guide() {
        let dir = crate::sh::test_dir("first-run");
        assert!(first_run(&dir, true), "no installed_plugins.json, as after herdr plugin install alone");
        std::fs::create_dir_all(dir.join("plugins")).unwrap();
        std::fs::write(dir.join("plugins/installed_plugins.json"), json!({ "plugins": { "vault@gronare": [{ "version": "0.3.0" }] } }).to_string()).unwrap();
        assert!(first_run(&dir, true), "another plugin is not kebbie's");
        assert!(!first_run(&dir.join("missing"), false), "without a terminal the board never asks");
        std::fs::write(dir.join("plugins/installed_plugins.json"), json!({ "plugins": { "kebbie@kebbie": [{ "version": "0.29.1" }] } }).to_string()).unwrap();
        assert!(!first_run(&dir, true), "set up: the board opens straight away");
        let main = include_str!("main.rs");
        assert!(main.contains("Some(\"board\") => setup_guide::board("), "kebbie board, which herdr's startup, the lead Space and the board popup run, goes through the first-run check");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
