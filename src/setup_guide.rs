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
use crate::setup_vault::{self, Failure, Job, Mindin, Signed, State};
use crate::sh::Result;
use crate::vault_cli::{Plugin, Probe};
use crate::vault_found::{Found, Origin};

pub const MINDIN_INSTALL: &str = "curl -fsSL https://mindin.dev/install.sh | sh";

// Without --no-setup the installer asks where the vault lives itself, and then kebbie's own mindin setup asks again.
const MINDIN_INSTALL_ONLY: &str = "curl -fsSL https://mindin.dev/install.sh | sh -s -- --no-setup";

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

    fn step(self) -> &'static str {
        match self {
            Place::Hosted | Place::Server | Place::None => "vault server",
            Place::S3 => "vault bucket",
            Place::Folder => "vault folder",
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
            Step::Signals => "Signals",
            Step::Review => "Review and apply",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Step::Name => "How your phone and your other machines show this one. One word, like mac or devbox.",
            Step::Agents => "The agents kebbie found on this machine. Each one you tick gets kebbie's plugin and its approval rules when you apply. Claude Code runs the lead and the reviewer, so it stays on.",
            Step::Vault => "Optional. A note for every task and a log line for every push. Choose where the vault lives; ⏎ installs mindin when it is missing and signs this machine in to a server, here on this screen.",
            Step::Backlog => "Where work waits and new items are written: the local file or the vault, one of them. Linear can sit beside either; the lead only reads it.",
            Step::Signals => "Both are off until you say yes. Neither ever sends notes, names, paths or your vault's address.",
            Step::Review => "Everything kebbie writes or installs, each tool's own files included. Apart from mindin's install and sign-in on step 3, nothing is written until you press ⏎ here.",
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
    pub mindin_lookup: Vec<PathBuf>,
    pub os: String,
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
            mindin_lookup: lookup,
            os: std::env::consts::OS.to_string(),
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
    pub signals: bool,
    pub usage: bool,
}

impl Answers {
    fn from(facts: &Facts) -> Answers {
        let vault = Where::from(facts);
        let usage = facts.runner_answer().and_then(|probe| probe.usage).unwrap_or(false);
        Answers { name: facts.name.clone(), codex: facts.codex.is_some(), sources: Answers::sources(facts, vault.chosen() != Place::None), vault, signals: facts.signals, usage }
    }

    // Sources this machine never chose follow the vault: with one, new items go to it.
    fn sources(facts: &Facts, vault: bool) -> Vec<(Source, bool)> {
        if !facts.sources_set {
            let filed = if vault { Source::Vault } else { Source::Local };
            return Source::ALL.into_iter().map(|source| (source, source == filed)).collect();
        }
        let read = config::read_sources(&facts.sources);
        let mut sources: Vec<(Source, bool)> = facts.sources.iter().map(|source| (*source, read.contains(source))).collect();
        sources.extend(Source::ALL.into_iter().filter(|source| !facts.sources.contains(source)).map(|source| (source, false)));
        sources
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
    SignIn,
    VaultSetup(Command, Vec<String>),
    MindinSignals(Command, bool),
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
    Retry,
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
    caret: usize,
    sources_touched: bool,
    pub answers: Answers,
    facts: Facts,
    visited: [bool; Step::ALL.len()],
    skipped: [bool; Step::ALL.len()],
    fresh: bool,
    error: Option<String>,
    pub applied: Option<Vec<(String, Outcome)>>,
    pub done: bool,
    pub ending: Vec<String>,
    pub opens_board: bool,
    pub vault_job: Option<Job>,
    mindin: Option<Mindin>,
    signed: Option<(String, Signed, Option<u64>)>,
    planned: Vec<Planned>,
}

fn home(path: &Path) -> String {
    crate::area_page::home_relative(path)
}

fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(at, _)| at)
}

fn with_caret(text: &str, caret: usize) -> String {
    let at = byte_at(text, caret);
    format!("{}▏{}", &text[..at], &text[at..])
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
        let mut guide = Guide { step: Step::Name, cursor: 0, caret: 0, sources_touched: false, answers, facts, visited: [false; Step::ALL.len()], skipped: [false; Step::ALL.len()], fresh: true, error: None, applied: None, done: false, ending: Vec::new(), opens_board: true, vault_job: None, mindin: None, signed: None, planned: Vec::new() };
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
            _ if self.installs() => Some(self.mindin.as_ref().map_or(Command::Installed, |mindin| Command::Found(mindin.command.clone()))),
            _ => Some(self.facts.runner().map_or(Command::Installed, |found| Command::Found(found.command.clone()))),
        }
    }

    fn server_address(&self) -> Option<String> {
        match self.answers.vault.chosen() {
            Place::Hosted => HOSTED_VAULT.map(str::to_string),
            Place::Server => Some(self.answers.vault.address.trim().to_string()).filter(|address| !address.is_empty()),
            _ => None,
        }
    }

    // This machine signed in on this screen to the server chosen now.
    fn signed_here(&self) -> Option<&(String, Signed, Option<u64>)> {
        let address = self.server_address()?;
        self.signed.as_ref().filter(|(signed, ..)| same_url(signed, &address))
    }

    fn signs_in(&self) -> bool {
        self.server_address().is_some() && self.needs_setup() && self.signed_here().is_none()
    }

    // What ⏎ on the vault step runs in the background: mindin's installer when it is missing, then the sign-in to a server.
    fn job_plan(&self, install: bool) -> Option<setup_vault::Plan> {
        if !self.needs_setup() || self.setup_flags().is_none() {
            return None;
        }
        let install = ((self.installs() && self.mindin.is_none()) || install).then(|| setup_vault::Install { script: MINDIN_INSTALL_ONLY.to_string(), lookup: self.facts.mindin_lookup.clone() });
        let sign_in = self.signs_in().then(|| self.setup_flags()).flatten();
        if install.is_none() && sign_in.is_none() {
            return None;
        }
        let command = match self.vault_used() {
            Some(Command::Found(command)) => Some(command),
            _ => None,
        };
        Some(setup_vault::Plan { install, command, sign_in })
    }

    fn start_job(&mut self, install: bool) -> bool {
        let Some(plan) = self.job_plan(install) else { return false };
        self.vault_job = Some(Job::new(plan));
        true
    }

    // The background step moves on: started, polled, and what it finished kept for the plan.
    pub fn tick(&mut self) {
        if let Some(job) = &mut self.vault_job {
            job.launch();
            job.poll();
        }
        self.absorb();
    }

    fn absorb(&mut self) {
        let Some(job) = &self.vault_job else { return };
        if let Some(mindin) = &job.mindin {
            self.mindin = Some(mindin.clone());
        }
        if let (true, Some(signed), Some(address)) = (job.finished(), &job.signed, self.server_address()) {
            self.signed = Some((address, signed.clone(), job.moved));
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
        match (self.facts.runner(), &self.mindin) {
            (_, Some(mindin)) if self.installs() => format!("mindin {}, installed here", mindin.version),
            (Some(found), None) if self.installs() => format!("mindin, installed on ⏎: {} has no setup", found.shown()),
            (Some(found), _) => format!("{} · {}", found.shown(), found.source),
            (None, _) => "mindin, installed on ⏎".to_string(),
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
            Step::Signals => vec![Spot::Check(0), Spot::Check(1)],
            Step::Review => Vec::new(),
        }
    }

    fn spot(&self) -> Option<Spot> {
        self.spots().get(self.cursor).copied()
    }

    fn chosen(&self) -> usize {
        match self.step {
            Step::Vault => Place::ALL.iter().position(|place| *place == self.answers.vault.chosen()).unwrap_or(0),
            _ => 0,
        }
    }

    fn go(&mut self, step: Step) {
        if step == Step::Backlog && !self.sources_touched && !self.facts.sources_set {
            let place = self.place().unwrap_or_else(|| Where::from(&self.facts).chosen());
            self.answers.sources = Answers::sources(&self.facts, place != Place::None);
        }
        self.step = step;
        self.error = None;
        self.fresh = true;
        self.cursor = self.spots().iter().position(|spot| *spot == Spot::Choice(self.chosen())).unwrap_or(0);
        self.caret_to_end();
    }

    fn caret_to_end(&mut self) {
        self.caret = self.typed().map_or(0, |field| self.answers.text(field).chars().count());
    }

    fn move_caret(&mut self, code: KeyCode) {
        let Some(field) = self.typed() else { return };
        let count = self.answers.text(field).chars().count();
        self.fresh = false;
        self.caret = match code {
            KeyCode::Left => self.caret.saturating_sub(1),
            KeyCode::Right => (self.caret + 1).min(count),
            KeyCode::Home => 0,
            _ => count,
        };
    }

    fn erase(&mut self, code: KeyCode) {
        let Some(field) = self.typed() else { return };
        self.fresh = false;
        let at = if code == KeyCode::Backspace { self.caret.checked_sub(1) } else { Some(self.caret) };
        let text = self.answers.text_mut(field);
        if let Some(at) = at.filter(|at| *at < text.chars().count()) {
            text.remove(byte_at(text, at));
            self.caret = at;
        }
    }

    fn choose(&mut self, at: usize) {
        if self.step == Step::Vault {
            self.answers.vault.place = Place::ALL.get(at).copied();
            self.cursor = self.spots().iter().position(|spot| *spot == Spot::Choice(at)).unwrap_or(self.cursor);
        }
    }

    fn typed(&self) -> Option<Field> {
        match self.spot() {
            Some(Spot::Text(field)) => Some(field),
            _ => None,
        }
    }

    fn type_char(&mut self, character: char) {
        self.insert(&character.to_string());
    }

    // A field just entered is replaced by what is typed; once the caret has moved, typing goes in at the caret.
    fn insert(&mut self, typed: &str) {
        let Some(field) = self.typed() else { return };
        if std::mem::replace(&mut self.fresh, false) {
            self.answers.text_mut(field).clear();
            self.caret = 0;
        }
        let caret = self.caret;
        let text = self.answers.text_mut(field);
        text.insert_str(byte_at(text, caret), typed);
        self.caret += typed.chars().count();
    }

    pub fn paste(&mut self, text: &str) {
        self.fresh = false;
        self.insert(text.trim());
    }

    fn reset(&mut self, step: Step) {
        let today = Answers::from(&self.facts);
        match step {
            Step::Vault => self.answers.vault = today.vault,
            Step::Backlog => self.answers.sources = today.sources,
            Step::Signals => (self.answers.signals, self.answers.usage) = (today.signals, today.usage),
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
                KeyCode::Char('r') if !self.failed_rows().is_empty() => Next::Retry,
                KeyCode::Enter => Next::Board,
                KeyCode::Esc => Next::Quit,
                _ => Next::Stay,
            };
        }
        if self.step == Step::Vault && self.vault_job.is_some() {
            return self.job_key(key.code);
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
                self.caret_to_end();
            }
            KeyCode::Char(character) if typing => self.type_char(character),
            KeyCode::Backspace | KeyCode::Delete if typing => self.erase(key.code),
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End if typing => self.move_caret(key.code),
            KeyCode::Char(' ') => match self.spot() {
                Some(Spot::Check(at)) => self.toggle(at),
                Some(Spot::Choice(at)) => self.choose(at),
                _ => {}
            },
            KeyCode::Enter if self.step == Step::Review && self.job_plan(false).is_some() => {
                self.go(Step::Vault);
                self.start_job(false);
            }
            KeyCode::Enter if self.step == Step::Review => return Next::Apply,
            KeyCode::Enter if self.step == Step::Vault && self.check().is_ok() && self.start_job(false) => {}
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

    fn job_key(&mut self, code: KeyCode) -> Next {
        let Some(state) = self.vault_job.as_ref().map(|job| job.state.clone()) else { return Next::Stay };
        match (code, state) {
            (KeyCode::Esc, _) => return Next::Quit,
            (KeyCode::Char(answer @ ('y' | 'n')), State::Move { .. }) => {
                if let Some(job) = &mut self.vault_job {
                    job.answer(answer == 'y');
                }
            }
            (KeyCode::Char('o'), State::Code(code)) => {
                if let (Some(program), Some(job)) = (crate::hub::connect::opener(), &mut self.vault_job) {
                    job.open(program, code.link);
                }
            }
            (KeyCode::Char('c'), State::Code(code)) => {
                let copied = crate::hub::console::copy_here(&code.code).is_ok();
                if let Some(job) = &mut self.vault_job {
                    job.copied = copied;
                }
            }
            (KeyCode::Char('r'), State::Failed(failure)) => {
                self.start_job(failure.kind == "old");
            }
            (KeyCode::Char('e'), state) if state != State::Moving => {
                self.vault_job = None;
                self.go(Step::Vault);
            }
            (KeyCode::Enter, State::Done) => {
                self.absorb();
                self.visited[Step::Vault.index()] = true;
                self.skipped[Step::Vault.index()] = false;
                self.go(self.next_step());
            }
            (KeyCode::Left, _) => {
                if let Some(previous) = self.previous_step() {
                    self.go(previous);
                }
            }
            _ => {}
        }
        Next::Stay
    }

    fn usage_asked(&self) -> bool {
        self.place().is_some_and(|place| place != Place::None) && (self.installs() || self.mindin.is_some() || self.facts.runner_answer().is_some_and(|probe| probe.setup))
    }

    fn toggle(&mut self, at: usize) {
        match self.step {
            Step::Agents if Agent::ALL.get(at) == Some(&Agent::Codex) && self.facts.codex.is_some() => self.answers.codex = !self.answers.codex,
            Step::Signals if at == 0 => self.answers.signals = !self.answers.signals,
            Step::Signals if self.usage_asked() => self.answers.usage = !self.answers.usage,
            Step::Backlog => {
                let Some((source, on)) = self.answers.sources.get(at).copied() else { return };
                self.sources_touched = true;
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
            rows.push(match &self.mindin {
                Some(mindin) => row("mindin", "already", format!("mindin {}, installed on step 3", mindin.version), Act::Nothing),
                None => row("mindin", "to do", self.facts.runner().map_or("installed first, then it runs the vault".to_string(), |found| format!("installed first, since {} has no setup", found.shown())), Act::InstallMindin),
            });
        }
        let step = place.step();
        let usage = self.usage_asked() && !self.skipped(Step::Signals);
        let mut folded = false;
        rows.push(match self.setup_flags() {
            _ if self.place_in() => row(step, "already", format!("{}{}", self.place_shown(), if matches!(place, Place::Hosted | Place::Server) { " · signed in" } else { "" }), Act::Nothing),
            Some(_) if self.server_address().is_some() => match self.signed_here() {
                Some((_, signed, moved)) => row(step, "already", format!("{} · {}{}", self.place_shown(), signed_words(signed), moved.map_or(String::new(), |files| format!(" · {files} files moved in"))), Act::Nothing),
                None => row(step, "to do", format!("{} · signs in on step 3", self.place_shown()), Act::SignIn),
            },
            Some(mut flags) => {
                if usage {
                    flags.extend(["--signals".to_string(), yes_no(self.answers.usage).to_string()]);
                    folded = true;
                }
                row(step, "to do", self.place_shown(), Act::VaultSetup(self.vault_used().unwrap_or(Command::Installed), flags))
            }
            None => row(step, "skipped", format!("{} needs {}", place.label(), place.needs()), Act::Nothing),
        });
        let known = self.facts.runner_answer().and_then(|probe| probe.usage).filter(|_| !self.installs());
        if usage && !folded {
            let said = if self.answers.usage { "on, once a day" } else { "off" };
            rows.push(match known {
                Some(on) if on == self.answers.usage => row("usage data", "already", said.to_string(), Act::Nothing),
                _ => row("usage data", "to do", said.to_string(), Act::MindinSignals(self.vault_used().unwrap_or(Command::Installed), self.answers.usage)),
            });
        }
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
        let on = answers.signals;
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
        let row = |verb, detail: String, act| Planned { group: KEBBIE, step: "vault command", verb, detail, act };
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
                Plugin::Enabled { id, .. } => row("vault plugin", "already", format!("the vault plugin {id}"), Act::Nothing),
                Plugin::Disabled { id, .. } => row("vault plugin", "to do", format!("turn on the vault plugin {id}"), Act::ClaudeVault),
                Plugin::Missing => row("vault plugin", "to do", format!("the vault plugin {}", setup::VAULT_PLUGIN), Act::ClaudeVault),
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
            return if facts.codex.is_some() && facts.codex_plugin != Installed::Missing { vec![row("codex plugin", "to do", "kebbie's plugin out of Codex".to_string(), Act::CodexPlugin(false))] } else { Vec::new() };
        }
        let plugin = format!("kebbie's plugin at {}", self.codex_plugin_shown());
        let rules = self.codex_home_shown(codex_setup::RULES_FILE);
        let config = self.codex_home_shown("config.toml");
        let mut rows = vec![if facts.codex_plugin == Installed::Current { row("codex plugin", "already", plugin, Act::Nothing) } else { row("codex plugin", "to do", plugin, Act::CodexPlugin(true)) }];
        rows.push(if facts.rules_placed == Placed::Current { row("codex approvals", "already", rules, Act::Nothing) } else { row("codex approvals", "to do", rules, Act::Rules(facts.rules.clone())) });
        if let Some(command) = self.vault_used() {
            let detail = format!("[mcp_servers.vault] in {config}");
            rows.push(match self.codex_server_placed(&command) {
                Placed::Current => row("codex vault", "already", detail, Act::Nothing),
                _ => row("codex vault", "to do", detail, Act::CodexVault(command)),
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
            Act::InstallMindin => Some(format!("{MINDIN_INSTALL_ONLY} first, its output kept on this screen")),
            Act::VaultSetup(command, flags) if self.answers.vault.chosen() == Place::S3 && !s3_keys_set() => Some(format!("{} in this terminal; it asks for the access key and the secret, hidden, unless S3_ACCESS_KEY and S3_SECRET_KEY are set", setup_script(&shown_command(command), flags))),
            Act::VaultSetup(command, flags) => Some(format!("{} on this screen{}", setup_script(&shown_command(command), flags), if self.answers.vault.chosen() == Place::Folder { "; the folder must exist" } else { "" })),
            Act::MindinSignals(command, on) => Some(format!("{} on this screen", setup_script(&shown_command(command), &["--signals".to_string(), yes_no(*on).to_string()]))),
            Act::Rules(text) => Some(format!("writes {}, {} lines", self.codex_home_shown(codex_setup::RULES_FILE), text.lines().count())),
            Act::CodexVault(command) if self.codex_server_placed(command) == Placed::Differs => Some(format!("replaces [mcp_servers.vault] in {config}; the rest of the file stays as it is")),
            Act::CodexVault(_) => Some(format!("adds [mcp_servers.vault] to the end of {config}; the rest of the file stays as it is")),
            Act::ClaudeVault => Some(setup::steps(&claude, &self.facts.vault, self.facts.gronare).iter().map(|step| step.join(" ")).collect::<Vec<_>>().join(", then ")),
            _ => None,
        }).collect()
    }

    fn offers_hub(&self) -> bool {
        self.facts.hub.is_none()
    }
}

fn yes_no(on: bool) -> &'static str {
    if on { "yes" } else { "no" }
}

fn s3_keys_set() -> bool {
    ["S3_ACCESS_KEY", "S3_SECRET_KEY"].iter().all(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()))
}

fn signed_words(signed: &Signed) -> String {
    let vault = [&signed.vault_name, &signed.vault].into_iter().find(|name| !name.is_empty()).map_or("signed in".to_string(), |name| format!("signed in: vault {name}"));
    if signed.access.is_empty() { vault } else { format!("{vault}, {}", signed.access) }
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
fn install_mindin(env: &Env) -> (Outcome, Option<Vec<String>>) {
    let lookup: Vec<PathBuf> = env.machine.path.iter().cloned().chain([env.machine.home.join(".local/bin")]).collect();
    match setup_vault::install(&setup_vault::Install { script: MINDIN_INSTALL_ONLY.to_string(), lookup }, &|_| {}) {
        Ok(mindin) => (Outcome::new("done", format!("mindin {} at {}", mindin.version, home(Path::new(&mindin.command[0])))), Some(mindin.command)),
        Err(cause) => (Outcome::new("failed", cause), None),
    }
}

// A mindin step that asks nothing runs with its output kept, and its last line is the cause when it fails.
fn run_captured(command: &[String], args: &[String]) -> std::result::Result<(), String> {
    let Some((program, rest)) = command.split_first() else { return Err("the vault command is empty".to_string()) };
    let words: Vec<&str> = rest.iter().chain(args).map(String::as_str).collect();
    match crate::sh::attempt(program, &words, None) {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(said)) => Err(stopped(Some(1), None, said.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or_default())),
        Err(error) => Err(error),
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

// kebbie's own answers are written first, so a step outside the screen that never comes back, as a Ctrl-C in mindin's setup, cannot lose them.
pub fn apply(planned: &[Planned], env: &Env, hooks: &mut dyn Hooks) -> Vec<(String, Outcome)> {
    let own = |act: &Act| matches!(act, Act::Name(_) | Act::Sources(_) | Act::Signals(_));
    let mut outcomes: Vec<Option<Outcome>> = vec![None; planned.len()];
    let mut installed = None;
    for first in [true, false] {
        for (at, row) in planned.iter().enumerate().filter(|(_, row)| own(&row.act) == first) {
            outcomes[at] = Some(apply_row(row, env, hooks, &mut installed));
            let shown: Vec<(String, Outcome)> = planned.iter().zip(&outcomes).map_while(|(row, outcome)| Some((row.step.to_string(), outcome.clone()?))).collect();
            hooks.progress(&shown);
        }
    }
    planned.iter().zip(outcomes).filter_map(|(row, outcome)| Some((row.step.to_string(), outcome?))).collect()
}

fn apply_row(row: &Planned, env: &Env, hooks: &mut dyn Hooks, installed: &mut Option<Vec<String>>) -> Outcome {
    match &row.act {
        Act::Nothing => Outcome::new(row.verb, row.detail.clone()),
        Act::Link => setup::link_binary(&env.machine.home, &env.machine.path),
        Act::ClaudePlugin => setup::claude_plugin(env.machine),
        Act::CodexPlugin(on) => codex_plugin(env, *on, &row.detail),
        Act::Rules(text) => rules(env, text),
        Act::InstallMindin => {
            let (outcome, mindin) = install_mindin(env);
            *installed = mindin;
            outcome
        }
        Act::SignIn => Outcome::new("skipped", "the sign-in happens on step 3 Vault"),
        Act::VaultSetup(command, flags) => match resolved(command, installed) {
            Ok(command) if flags.first().is_some_and(|flag| flag == "--s3") && !s3_keys_set() => match hooks.outside(&setup_script(&command, flags)) {
                Ok(()) => Outcome::new("done", row.detail.clone()),
                Err(cause) => Outcome::new("failed", cause),
            },
            Ok(command) => match run_captured(&command, &[&["setup".to_string()], flags.as_slice()].concat()) {
                Ok(()) => Outcome::new("done", row.detail.clone()),
                Err(cause) => Outcome::new("failed", cause),
            },
            Err(outcome) => outcome,
        },
        Act::MindinSignals(command, on) => match resolved(command, installed) {
            Ok(command) => match run_captured(&command, &["setup".to_string(), "--signals".to_string(), yes_no(*on).to_string()]) {
                Ok(()) => Outcome::new("done", row.detail.clone()),
                Err(cause) => Outcome::new("failed", cause),
            },
            Err(outcome) => outcome,
        },
        Act::VaultCommand(Some(command)) => match resolved(command, installed) {
            Ok(command) => when_done(write_key(env, "vault.command", &Set::Text(command.join(" "))), format!("vault.command = \"{}\"", command.join(" "))),
            Err(outcome) => outcome,
        },
        Act::VaultCommand(None) => remove_key(env, "vault.command"),
        Act::ClaudeVault => claude_vault(env),
        Act::CodexVault(command) => match resolved(command, installed) {
            Ok(command) => codex_vault(env, &command),
            Err(outcome) => outcome,
        },
        Act::Name(name) => when_done(write_key(env, "machine.name", &Set::Text(name.clone())), row.detail.clone()),
        Act::Keys(plan, bindings) => keys(env, plan, bindings),
        Act::Sources(sources) => when_done(write_key(env, "board.backlog.sources", &Set::List(sources.iter().map(|source| source.key().to_string()).collect())), row.detail.clone()),
        Act::Signals(on) => when_done(write_key(env, "report.signals", &Set::Bool(*on)), row.detail.clone()),
        Act::Repair(fix) => repair(env, *fix),
    }
}

// Why a step outside the screen stopped, in words: mindin's own last line where it names the cause, else its exit.
fn stopped(code: Option<i32>, signal: Option<i32>, said: &str) -> String {
    let said = said.strip_prefix("mindin: ").unwrap_or(said).trim();
    match () {
        _ if signal == Some(2) || code == Some(130) => "stopped with Ctrl-C".to_string(),
        _ if said.contains("expired before it was approved") => "the sign-in was not finished before its code expired".to_string(),
        _ if said.contains("sign-in was denied") => "the sign-in was denied in the browser".to_string(),
        _ if code == Some(127) => "its command was not found".to_string(),
        _ if !said.is_empty() && !said.contains("Traceback") => said.to_string(),
        _ => match (code, signal) {
            (Some(code), _) => format!("it exited with status {code}"),
            (None, Some(signal)) => format!("it was killed by signal {signal}"),
            (None, None) => "it ended without a status".to_string(),
        },
    }
}

fn run_outside(script: &str) -> Result<()> {
    use std::os::unix::process::ExitStatusExt;
    let (status, said) = crate::sh::run_visibly_told("sh", &["-c", script], None)?;
    if status.success() { Ok(()) } else { Err(stopped(status.code(), status.signal(), &said)) }
}

// After a step outside fails, its output stays on screen until the user has read it.
fn pause(cause: &str, out: &mut dyn std::io::Write, input: &mut dyn std::io::BufRead) {
    let _ = writeln!(out, "\n  \x1b[31m✗\x1b[0m that step did not finish: {cause} · its output is above · ⏎ goes back to kebbie setup");
    let _ = out.flush();
    let _ = input.read_line(&mut String::new());
}

struct Plain;

impl Hooks for Plain {
    fn progress(&mut self, _: &[(String, Outcome)]) {}

    fn outside(&mut self, script: &str) -> Result<()> {
        run_outside(script)
    }
}

// --yes applies the guide's defaults, every line printed, never answers the error signals question and never runs mindin's installer or setup; a run without a terminal writes only kebbie's own link and plugins and names the rest.
pub fn without_asking(env: &Env, asking: Asking) -> Vec<(String, Outcome)> {
    let facts = Facts::read(env);
    if asking == Asking::Yes {
        let planned: Vec<Planned> = Guide::new(facts.clone(), Step::Review).plan().into_iter().filter(|row| !matches!(row.act, Act::InstallMindin | Act::SignIn | Act::VaultSetup(..) | Act::MindinSignals(..))).collect();
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
    let rows = [("link", Some(setup::link_binary(&env.machine.home, &env.machine.path))), ("plugin", Some(setup::claude_plugin(env.machine))), ("codex", codex), ("keys", Some(keys)), ("vault command", vault_unasked(&facts, &env.config))];
    rows.into_iter().filter_map(|(step, outcome)| Some((step.to_string(), outcome?))).collect()
}

fn label(row: &Planned) -> String {
    match row.group {
        CLAUDE => format!("claude {}", row.step),
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
    guide.plan().into_iter().filter(|row| row.group == group_of(agent) && !matches!(row.act, Act::ClaudePlugin | Act::CodexPlugin(_)) && row.verb == "to do").collect()
}

pub fn offers(env: &Env, agent: Agent) -> Vec<String> {
    let rows: Vec<String> = agent_rows(env, agent).iter().map(|row| format!("  {:<15} {}", row.step, row.detail)).collect();
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
const HERDR_START: (&str, &str) = ("herdr", "start it: kebbie's lead and board run inside it");
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

pub fn ending(codex: Option<Login>, checks: &[crate::doctor::Check], shell: &str, in_herdr: bool) -> Vec<String> {
    let count = |ok: bool| checks.iter().filter(|check| if ok { check.level == crate::doctor::Level::Ok } else { matches!(check.level, crate::doctor::Level::Warn | crate::doctor::Level::Fail) }).count();
    let open = count(false);
    let left = if open > 0 { format!(", {open} to look at") } else { String::new() };
    let mut lines = vec![String::new(), format!("{DONE} · doctor: {} ok{left}", count(true)), String::new(), NEXT.to_string()];
    if !in_herdr {
        lines.push(next_step(HERDR_START.0, HERDR_START.1));
    }
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

const DETAIL_AT: usize = 2 + 9 + 16;
const MORE: &str = "…; the summary after setup shows all of it";

// A tool's error read whole joins its lines with nothing between them, as "repository.Please".
fn one_line(detail: &str) -> String {
    detail.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join(" ")
}

fn wrapped(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while !word.is_empty() {
            let used = line.chars().count();
            let room = if used == 0 { width } else { width.saturating_sub(used + 1) };
            let length = word.chars().count();
            if length <= room {
                if used > 0 {
                    line.push(' ');
                }
                line.push_str(&word);
                break;
            }
            if used > 0 {
                lines.push(std::mem::take(&mut line));
                continue;
            }
            let rest = word.split_off(byte_at(&word, width));
            lines.push(std::mem::replace(&mut word, rest));
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

fn capped(mut lines: Vec<String>, width: usize) -> Vec<String> {
    if lines.len() <= 2 {
        return lines;
    }
    lines.truncate(2);
    let more = if width > MORE.chars().count() + 8 { MORE } else { "…" };
    let kept: String = lines[1].chars().take(width.saturating_sub(more.chars().count())).collect();
    lines[1] = format!("{}{more}", kept.trim_end());
    lines
}

// A detail too long for its column goes on under that column, at most two lines when capped.
fn row_lines(step: &str, outcome: &Outcome, width: usize, cap: bool) -> Vec<Line<'static>> {
    let detail = one_line(&outcome.detail);
    let column = width.saturating_sub(DETAIL_AT);
    let details = if column < 16 { vec![detail] } else if cap { capped(wrapped(&detail, column), column) } else { wrapped(&detail, column) };
    let mut details = details.into_iter();
    let first = Line::from(vec![Span::raw("  "), Span::styled(format!("{:<9}", outcome.verb), style(verb_color(outcome.verb))), Span::styled(format!("{step:<16}"), style(MAUVE)), Span::styled(details.next().unwrap_or_default(), style(TEXT))]);
    std::iter::once(first).chain(details.map(|more| Line::from(vec![Span::raw(" ".repeat(DETAIL_AT)), Span::styled(more, style(TEXT))]))).collect()
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
        spans.push(Span::styled(if chosen { with_caret(&value, self.caret) } else { value.clone() }, if chosen { bold(TEXT).bg(MANTLE) } else { style(TEXT) }));
        if value.is_empty() {
            spans.push(Span::styled(field.placeholder().to_string(), style(FAINT)));
        }
        Line::from(spans)
    }

    fn review_lines(&self, rows: Vec<(String, Outcome)>, width: usize) -> Vec<Line<'static>> {
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
            lines.extend(row_lines(&step, &outcome, width, false));
        }
        lines
    }

    fn body(&self, width: usize) -> Vec<Line<'static>> {
        let (facts, answers) = (&self.facts, &self.answers);
        let mut lines = Vec::new();
        match self.step {
            Step::Name => {
                let chosen = self.spot() == Some(Spot::Text(Field::Name));
                let name = if chosen { with_caret(&answers.name, self.caret) } else { answers.name.clone() };
                lines.push(Line::from(vec![Span::styled(if chosen { "    ▸ " } else { "      " }, style(MAUVE)), Span::styled(format!("{:<9}", "name"), style(SUBTEXT)), Span::styled(name, if chosen { bold(TEXT).bg(MANTLE) } else { style(TEXT) })]));
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
            Step::Vault if self.vault_job.is_some() => lines.extend(self.job_lines()),
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
                lines.push(self.option(Spot::Check(0), Self::check_mark(answers.signals, true), "kebbie error signals".to_string(), "on a crash: kebbie's version, your OS, what failed and a count; daily, a random install id".to_string(), true));
                let usage = self.usage_asked();
                let about = if usage { "a random install id, mindin's version, your OS and CPU and how mindin runs" } else { "only with a mindin vault, chosen on step 3" };
                lines.push(self.option(Spot::Check(1), Self::check_mark(answers.usage && usage, usage), "mindin usage data, once a day".to_string(), about.to_string(), usage));
                lines.push(Line::default());
                lines.push(Line::from(Span::styled("mindin gets your answer from here and does not ask again.", style(SUBTEXT))));
                lines.push(Line::from(Span::styled("A bug report with text is always shown to you first and sent only on your y.", style(SUBTEXT))));
            }
            Step::Review if self.done => lines.extend(self.done_lines(width)),
            Step::Review => {
                let rows: Vec<(String, Outcome)> = match &self.applied {
                    Some(done) => {
                        let mut rows = done.clone();
                        rows.extend(self.plan().into_iter().skip(done.len()).map(|row| (row.step.to_string(), Outcome::new("to do", row.detail))));
                        rows
                    }
                    None => self.plan().into_iter().map(|row| (row.step.to_string(), Outcome::new(row.verb, row.detail))).collect(),
                };
                lines.extend(self.review_lines(rows, width));
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
            _ if self.applied.is_some() => {
                let retry = if self.failed_rows().is_empty() { "" } else { "r tries the failed steps again · " };
                let phone = if self.offers_hub() { "h connects your phone · " } else { "" };
                let enter = if self.opens_board { "⏎ opens your board" } else { "⏎ ends setup" };
                format!("{retry}{phone}{enter}")
            }
            Step::Vault if self.vault_job.is_some() => self.job_hint(),
            Step::Name => "type to change · ← → move in the text · ⏎ next · esc quit".to_string(),
            Step::Signals => "↑ ↓ choose · space toggle · ⏎ next · ← back · s skip · esc quit".to_string(),
            Step::Vault if self.typed().is_some() => "type to change · ← → move in the text · ↑ ↓ choose · ⏎ next · esc quit".to_string(),
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
        lines.extend(self.body(boxed.width.saturating_sub(4) as usize));
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

    fn done_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (at, (step, outcome)) in self.applied.iter().flatten().enumerate().filter(|(_, (_, outcome))| !matches!(outcome.verb, "already" | "skipped")) {
            lines.extend(row_lines(step, outcome, width, true));
            if outcome.verb == "failed" {
                let later = self.planned.get(at).and_then(|row| self.later(&row.act)).map_or(String::new(), |command| format!(" · or later: {command}"));
                let column = width.saturating_sub(DETAIL_AT).max(16);
                lines.extend(wrapped(&format!("r tries it again{later}"), column).into_iter().map(|said| Line::from(vec![Span::raw(" ".repeat(DETAIL_AT)), Span::styled(said, style(MUTED))])));
            }
        }
        let failed = !self.failed_rows().is_empty();
        let headline = self.headline();
        for said in &self.ending {
            lines.push(match (said.as_str(), said.get(..23), said.get(23..)) {
                (_, _, _) if said.starts_with(DONE) => Line::from(vec![Span::styled(DONE, style(GREEN)), Span::styled(headline[DONE.len()..].to_string(), style(if failed { RED } else { GREEN })), Span::styled(said[DONE.len()..].to_string(), style(SUBTEXT))]),
                (NEXT, _, _) => Line::from(Span::styled(NEXT, bold(MAUVE))),
                (_, Some(command), Some(why)) => Line::from(vec![Span::styled(command.to_string(), style(TEXT)), Span::styled(why.to_string(), style(SUBTEXT))]),
                _ => Line::from(Span::styled(said.clone(), style(TEXT))),
            });
        }
        lines
    }

    fn spinner(&self) -> &'static str {
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let at = self.vault_job.as_ref().map_or(0, |job| job.started.elapsed().as_millis() / 100) as usize;
        FRAMES[at % FRAMES.len()]
    }

    // Why the background step stopped, in words, and what r does about it.
    fn failure_lines(&self, failure: &Failure) -> Vec<Line<'static>> {
        let server = self.server_address().unwrap_or_default();
        let (said, retry) = match failure.kind.as_str() {
            "expired" => ("The code expired before it was approved.".to_string(), "r gets a new code"),
            "denied" => ("The sign-in was denied in the browser.".to_string(), "r gets a new code"),
            "unreachable" if crate::hub::http::local_network(&self.facts.os, &failure.message, failure.errno) => (format!("{}.", crate::hub::http::LOCAL_NETWORK), "r tries again"),
            "unreachable" => (format!("{} did not answer: {}", crate::hub::host(&server), failure.message), "r tries again"),
            "old" => ("This mindin is too old to sign in from here.".to_string(), "r installs the newest mindin and signs in"),
            "install" => (format!("{}.", failure.message.trim_end_matches('.')), "r runs the installer again"),
            _ => (format!("The sign-in stopped: {}", failure.message), "r tries again"),
        };
        vec![Line::from(vec![Span::styled("✗ ", style(RED)), Span::styled(said, style(TEXT))]), Line::default(), Line::from(vec![Span::styled(retry[..1].to_string(), bold(MAUVE)), Span::styled(retry[1..].to_string(), style(SUBTEXT))])]
    }

    fn job_lines(&self) -> Vec<Line<'static>> {
        let Some(job) = &self.vault_job else { return Vec::new() };
        let mut lines = Vec::new();
        let tick = |text: String| Line::from(vec![Span::styled("✓ ", style(GREEN)), Span::styled(text, style(TEXT))]);
        let waiting = |text: &str| Line::from(vec![Span::styled(format!("{text} "), style(SUBTEXT)), Span::styled(self.spinner(), style(MAUVE))]);
        match (&job.mindin, &job.state) {
            (Some(mindin), _) => lines.push(tick(format!("mindin {} installed", mindin.version))),
            (None, State::Waiting | State::Installing) if job.plan.install.is_some() => {
                lines.push(waiting("installing mindin"));
                if !job.progress.is_empty() {
                    lines.push(Line::from(Span::styled(format!("  {}", job.progress), style(MUTED))));
                }
            }
            _ => {}
        }
        if job.plan.sign_in.is_none() {
            match &job.state {
                State::Done => lines.extend([Line::default(), Line::from(vec![Span::styled("⏎", bold(MAUVE)), Span::styled(format!(" next: {}", self.next_step().title()), style(SUBTEXT))])]),
                State::Failed(failure) => lines.extend(self.failure_lines(failure)),
                _ => {}
            }
            return lines;
        }
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        if let Some(signed) = &job.signed {
            lines.push(tick(signed_words(signed)));
            let about: Vec<String> = signed.layout.iter().map(|layout| format!("layout {layout}")).chain(signed.notes.map(|notes| format!("{notes} notes"))).collect();
            if !about.is_empty() {
                lines.push(Line::from(Span::styled(format!("  {}", about.join(" · ")), style(SUBTEXT))));
            }
            if let Some(files) = job.moved {
                lines.push(tick(format!("moved {files} files into it")));
            }
        }
        match &job.state {
            State::Waiting | State::Installing => {}
            State::Starting if job.signed.is_none() => lines.push(waiting("starting the sign-in")),
            State::Starting => {}
            State::Code(code) => {
                lines.push(Line::from(Span::styled("Sign this machine in", bold(TEXT))));
                lines.push(Line::from(vec![Span::styled("Open ", style(TEXT)), Span::styled(code.uri.clone(), style(MAUVE)), Span::styled(" and enter the code", style(TEXT))]));
                lines.push(Line::default());
                let large: String = code.code.chars().map(|character| character.to_string()).collect::<Vec<_>>().join(" ");
                lines.push(Line::from(vec![Span::raw("    "), Span::styled(large, bold(MAUVE).bg(SURFACE))]));
                lines.push(Line::default());
                lines.push(Line::from(Span::styled("There you pick the vault this machine uses, an existing one or a new one.", style(SUBTEXT))));
                lines.push(Line::default());
                let opened = if job.opened { "  opened" } else { "" };
                let copied = if job.copied { "  copied" } else { "" };
                lines.push(Line::from(vec![Span::styled("o", bold(MAUVE)), Span::styled(format!(" opens the browser{opened}   "), style(SUBTEXT)), Span::styled("c", bold(MAUVE)), Span::styled(format!(" copies the code{copied}"), style(SUBTEXT))]));
                lines.push(waiting("waiting for you to approve"));
            }
            State::Move { files, source } => {
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(format!("This machine's vault was {source}, with {files} files."), style(TEXT))));
                lines.push(Line::from(vec![Span::styled("Bring them into the server vault?  ", style(TEXT)), Span::styled("y", bold(MAUVE)), Span::styled(" brings them · ", style(SUBTEXT)), Span::styled("n", bold(MAUVE)), Span::styled(" leaves them where they are", style(SUBTEXT))]));
            }
            State::Moving => lines.push(waiting("moving the notes")),
            State::Done => lines.extend([Line::default(), Line::from(vec![Span::styled("⏎", bold(MAUVE)), Span::styled(format!(" next: {}", self.next_step().title()), style(SUBTEXT))])]),
            State::Failed(failure) => lines.extend(self.failure_lines(failure)),
        }
        lines
    }

    fn job_hint(&self) -> String {
        match self.vault_job.as_ref().map(|job| &job.state) {
            Some(State::Code(_)) => "o open the browser · c copy the code · e change the vault · ← back · esc quit".to_string(),
            Some(State::Move { .. }) => "y bring the notes · n leave them · esc quit".to_string(),
            Some(State::Moving) => "esc quit".to_string(),
            Some(State::Done) => "⏎ next · e change the vault · ← back · esc quit".to_string(),
            Some(State::Failed(_)) => "r try again · e change the vault · ← back · esc quit".to_string(),
            _ => "e change the vault · ← back · esc quit".to_string(),
        }
    }

    fn failed_rows(&self) -> Vec<usize> {
        self.applied.iter().flatten().enumerate().filter(|(_, (_, outcome))| outcome.verb == "failed").map(|(at, _)| at).collect()
    }

    fn headline(&self) -> String {
        match self.failed_rows().len() {
            0 => DONE.to_string(),
            1 => format!("{DONE}, 1 step failed"),
            failed => format!("{DONE}, {failed} steps failed"),
        }
    }

    // The command that does a failed row's work later, outside this guide.
    fn later(&self, act: &Act) -> Option<String> {
        let claude = Path::new(&self.facts.claude_bin).file_name().map_or(self.facts.claude_bin.clone(), |name| name.to_string_lossy().to_string());
        Some(match act {
            Act::Nothing => return None,
            Act::Link | Act::Keys(..) => "kebbie setup --yes".to_string(),
            Act::ClaudePlugin => "kebbie setup claude --yes".to_string(),
            Act::CodexPlugin(_) | Act::Rules(_) | Act::CodexVault(_) => "kebbie setup codex --yes".to_string(),
            Act::Name(_) | Act::Sources(_) | Act::Signals(_) | Act::VaultCommand(_) => "kebbie setup".to_string(),
            Act::InstallMindin => MINDIN_INSTALL_ONLY.to_string(),
            Act::SignIn => "kebbie setup vault".to_string(),
            Act::VaultSetup(command, flags) => setup_script(&shown_command(command), flags),
            Act::MindinSignals(command, on) => setup_script(&shown_command(command), &["--signals".to_string(), yes_no(*on).to_string()]),
            Act::ClaudeVault => setup::steps(&claude, &self.facts.vault, self.facts.gronare).iter().map(|step| step.join(" ")).collect::<Vec<_>>().join(", then "),
            Act::Repair(_) => "kebbie doctor".to_string(),
        })
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
                let headline = self.headline();
                lines.extend(self.ending.iter().map(|said| said.strip_prefix(DONE).map_or_else(|| said.clone(), |rest| format!("{headline}{rest}"))));
                if phone {
                    lines.extend(PHONE.iter().map(|line| line.to_string()));
                }
                lines.join("\n")
            }
        }
    }
}

fn running(script: &str) -> String {
    format!("\n  \x1b[2m→ {script}\x1b[0m\n")
}

type Again = (Vec<(String, Outcome)>, Vec<usize>);

struct Screen<'a> {
    terminal: &'a mut ratatui::DefaultTerminal,
    guide: &'a mut Guide,
    again: Option<Again>,
}

impl Hooks for Screen<'_> {
    fn progress(&mut self, rows: &[(String, Outcome)]) {
        self.guide.applied = Some(match &self.again {
            Some((before, at)) => merged(before, at, rows),
            None => rows.to_vec(),
        });
        let guide = &*self.guide;
        let _ = self.terminal.draw(|frame| guide.draw(frame));
    }

    fn outside(&mut self, script: &str) -> Result<()> {
        crate::board::terminal::outside(self.terminal, || {
            println!("{}", running(script));
            let result = run_outside(script);
            if let Err(cause) = &result {
                pause(cause, &mut std::io::stdout(), &mut std::io::stdin().lock());
            }
            result
        })
    }
}

// The rows tried again take the places of the failed ones they were.
fn merged(before: &[(String, Outcome)], at: &[usize], again: &[(String, Outcome)]) -> Vec<(String, Outcome)> {
    let mut rows = before.to_vec();
    for (place, row) in at.iter().zip(again) {
        if let Some(slot) = rows.get_mut(*place) {
            *slot = row.clone();
        }
    }
    rows
}

impl Guide {
    // Only the failed rows run again, each in its place on the done list.
    fn retried(&self) -> (Vec<(String, Outcome)>, Vec<usize>, Vec<Planned>) {
        let at = self.failed_rows();
        let rows = at.iter().filter_map(|place| self.planned.get(*place).cloned()).collect();
        (self.applied.clone().unwrap_or_default(), at, rows)
    }
}

fn ask(env: &Env, at: Step, board_follows: bool) -> Result<(Next, Guide)> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};
    let mut guide = Guide::new(Facts::read(env), at);
    let in_herdr = in_herdr();
    guide.opens_board = opens_board(in_herdr, board_follows);
    let mut terminal = crate::board::terminal::init_terminal();
    let result = (|| -> Result<Next> {
        loop {
            guide.tick();
            terminal.draw(|frame| guide.draw(frame)).map_err(|error| error.to_string())?;
            if !event::poll(std::time::Duration::from_millis(100)).map_err(|error| error.to_string())? {
                continue;
            }
            match event::read().map_err(|error| error.to_string())? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match guide.key(key) {
                    Next::Stay => {}
                    Next::Retry => {
                        let (before, at, rows) = guide.retried();
                        let again = apply(&rows, env, &mut Screen { terminal: &mut terminal, guide: &mut guide, again: Some((before.clone(), at.clone())) });
                        guide.applied = Some(merged(&before, &at, &again));
                    }
                    Next::Apply => {
                        let planned = guide.plan();
                        guide.planned = planned.clone();
                        guide.applied = Some(Vec::new());
                        let rows = apply(&planned, env, &mut Screen { terminal: &mut terminal, guide: &mut guide, again: None });
                        let codex = guide.codex_on().then(|| login::codex(guide.facts.codex_bin.as_deref(), login::TIMEOUT));
                        guide.ending = ending(codex, &crate::doctor::checks(&crate::doctor::Env::current()), &std::env::var("SHELL").unwrap_or_default(), in_herdr);
                        guide.ending.splice(0..0, crate::backlog_style::sync_here());
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

pub fn in_herdr() -> bool {
    std::env::var("HERDR_PANE_ID").is_ok_and(|pane| !pane.is_empty())
}

#[derive(Debug, PartialEq)]
enum After {
    Lead,
    Board,
    Done,
}

// kebbie board opens the board after the guide wherever it runs; kebbie setup opens it only inside herdr.
fn opens_board(in_herdr: bool, board_follows: bool) -> bool {
    in_herdr || board_follows
}

// Outside herdr the board has no Spaces to show, so setup ends with the line that starts herdr.
fn after(next: &Next, in_herdr: bool) -> After {
    match next {
        Next::Board | Next::Connect if !in_herdr => After::Done,
        Next::Board => After::Lead,
        Next::Connect => After::Board,
        _ => After::Done,
    }
}

pub fn run(env: &Env, at: Step) -> Result<String> {
    let (next, guide) = ask(env, at, false)?;
    if let Next::Connect = next {
        println!("{}\n", guide.said(false));
        println!("{}", crate::hub::cli(Some("connect"), &[], None)?);
    }
    match after(&next, in_herdr()) {
        After::Lead => {
            println!("{}\n", guide.text());
            crate::lead::show()
        }
        After::Board => crate::board::run(false),
        After::Done if matches!(next, Next::Connect) => Ok(String::new()),
        After::Done => Ok(guide.text()),
    }
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
        let (next, guide) = ask(&env, Step::Name, true)?;
        match next {
            Next::Connect => {
                println!("{}\n", guide.said(false));
                println!("{}", crate::hub::cli(Some("connect"), &[], None)?);
            }
            Next::Quit => {}
            _ => println!("{}\n", guide.text()),
        }
        if !matches!(next, Next::Quit) && in_herdr() {
            let _ = crate::lead::restart(false, false, false);
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
        config: Option<PathBuf>,
        config_outside: Vec<String>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake { drawn: 0, ran: Vec::new(), installs: None, config: None, config_outside: Vec::new() }
        }
    }

    impl Hooks for Fake {
        fn progress(&mut self, _: &[(String, Outcome)]) {
            self.drawn += 1;
        }

        fn outside(&mut self, script: &str) -> Result<()> {
            self.ran.push(script.to_string());
            if let Some(config) = &self.config {
                self.config_outside.push(std::fs::read_to_string(config).unwrap_or_default());
            }
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

    fn signed(name: &str) -> Signed {
        Signed { server: "https://notes.example.org".to_string(), vault: name.to_lowercase(), vault_name: name.to_string(), access: "read and write".to_string(), layout: Some("default".to_string()), notes: Some(128), already: false }
    }

    fn feed(guide: &mut Guide, msgs: Vec<setup_vault::Msg>) {
        for msg in msgs {
            guide.vault_job.as_mut().expect("a job runs").take(msg);
        }
        guide.absorb();
    }

    fn finish_job(guide: &mut Guide) {
        use setup_vault::{Msg, Said};
        let mindin = Mindin { command: vec!["/opt/mindin".to_string()], version: "0.21.0".to_string() };
        feed(guide, vec![Msg::Installed(Ok(mindin)), Msg::Said(Said::Signed(signed("Work"))), Msg::Said(Said::Done)]);
    }

    fn vault_rows(guide: &Guide) -> Vec<String> {
        plan(guide).into_iter().filter(|row| row.starts_with("mindin ") || row.contains(" vault ")).collect()
    }

    #[test]
    fn the_guide_has_six_steps_shows_each_agents_status_and_writes_nothing_before_apply() {
        let lab = Lab::new("guide-walk");
        let mut guide = fresh(&lab);
        let shown = screen(&guide);
        assert!(shown.contains("kebbie setup  step 1 of 6") && shown.contains("▸ 1 This machine") && shown.contains("· 2 Agents") && shown.contains("· 3 Vault") && shown.contains("· 4 Backlog") && shown.contains("· 5 Signals") && shown.contains("· 6 Review and apply"), "{shown}");
        for gone in ["Plugin", "Login", "Approvals", "Updates", "herdr keys", "Vault for kebbie", "Vault in Codex"] {
            assert!(!shown.contains(gone), "{gone} is no step of its own: {shown}");
        }
        assert!(shown.contains("▸ name     mac▏") && shown.contains("from this computer's name") && shown.contains("type to change · ← → move in the text · ⏎ next · esc quit"), "{shown}");
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
        assert!(screen(&guide).contains("( ) Hosted mindin    coming soon") && screen(&guide).contains("▸ (•) None             kebbie keeps tasks without one") && screen(&guide).contains("runs  mindin, installed on ⏎"), "{}", screen(&guide));
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Backlog);
        assert!(screen(&guide).contains("[x] Local file  · an item file each in backlog/ beside your config · new items go here"), "{}", screen(&guide));
        press(&mut guide, &[KeyCode::Down, KeyCode::Char(' ')]);
        assert_eq!(guide.answers.chosen_sources(), [Source::Vault], "ticking the vault unticks the local file: new items go to one place");
        press(&mut guide, &[KeyCode::Down, KeyCode::Char(' ')]);
        assert_eq!(guide.answers.chosen_sources(), [Source::Vault, Source::Linear], "Linear sits beside either");
        press(&mut guide, &[KeyCode::Char(' '), KeyCode::Enter]);
        assert_eq!(guide.step, Step::Signals);
        assert!(screen(&guide).contains("Neither ever sends notes, names, paths or your vault's address.") && screen(&guide).contains("A bug report with text is always shown to you first"));
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Review);
        let codex = format!("{}/codex", lab.dir.display());
        assert_eq!(plan(&guide), [
            "kebbie to do link ~/.local/bin/kebbie".to_string(),
            "kebbie to do machine \"devbox\"".to_string(),
            "kebbie skipped vault command no vault; kebbie keeps tasks without one".to_string(),
            "kebbie to do backlog vault".to_string(),
            "kebbie to do signals off".to_string(),
            "kebbie to do keeper start it, so routines run without an open board".to_string(),
            "Claude Code to do plugin kebbie's plugin, through claude plugin install".to_string(),
            "Claude Code already approvals kebbie gives each Claude task its permissions as it starts it".to_string(),
            "Claude Code to do usage kebbie's statusLine in Claude Code's settings.json, for the board's usage bar".to_string(),
            format!("Codex to do codex plugin kebbie's plugin at {codex}/plugins/cache/kebbie-tasks/kebbie/local"),
            format!("Codex to do codex approvals {codex}/rules/kebbie.rules"),
            format!("herdr to do keys prefix+space, prefix+f, prefix+t at the end of {}", lab.machine.herdr_config.display()),
        ], "every write is a row of the plan, each tool's under its name, herdr's keys among them");
        let shown = screen(&guide);
        assert!(shown.contains("│ kebbie") && shown.contains("│ Claude Code") && shown.contains("│ Codex") && shown.contains("│ herdr"), "the rows are grouped by the tool they write: {shown}");
        assert!(shown.contains(&format!("runs: writes {codex}/rules/kebbie.rules, 48 lines")) && shown.contains("⏎ apply all · ← back · esc quit without writing"), "{shown}");
        assert_eq!((lab.config(), lab.herdr(), lab.calls()), (String::new(), String::new(), String::new()), "nothing is written or run before ⏎ on the last step");
        assert!(!lab.dir.join("codex").exists() && !lab.machine.home.join(".local/bin/kebbie").exists() && !lab.machine.claude_dir.join("settings.json").exists());
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Esc)), Next::Quit));
        assert_eq!(guide.text(), "Nothing written: kebbie setup runs the guide again whenever you want.");
    }

    #[test]
    fn the_arrow_keys_move_inside_a_text_field_and_typing_goes_in_at_the_caret() {
        let lab = Lab::new("guide-caret");
        let mut guide = fresh(&lab);
        press(&mut guide, &[KeyCode::Left]);
        assert!(screen(&guide).contains("▸ name     ma▏c"), "{}", screen(&guide));
        type_text(&mut guide, "r");
        assert_eq!(guide.answers.name, "marc", "a moved caret keeps the name and types into it");
        press(&mut guide, &[KeyCode::Home, KeyCode::Char('x'), KeyCode::Delete]);
        assert_eq!(guide.answers.name, "xarc");
        press(&mut guide, &[KeyCode::Right, KeyCode::Backspace, KeyCode::End, KeyCode::Char('h')]);
        assert_eq!((guide.answers.name.as_str(), guide.step), ("xrch", Step::Name));
        let mut vault = vault_step(&lab, vec![found("mindin on PATH", "mindin")], None);
        press(&mut vault, &[KeyCode::Down]);
        assert_eq!(vault.typed(), Some(Field::Address));
        press(&mut vault, &[KeyCode::Left, KeyCode::Left, KeyCode::Left, KeyCode::Char('x')]);
        assert_eq!((vault.step, vault.answers.vault.address.as_str()), (Step::Vault, "https://vault.example.xcom"), "← in a field moves in the text, never back a step");
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
            "skipped vault command no vault; kebbie keeps tasks without one".to_string(),
            "done backlog vault".to_string(),
            "done signals off".to_string(),
            format!("done plugin installed kebbie@kebbie in Claude Code from {}", lab.machine.home.join(".local/share/kebbie/current").display()),
            "already approvals kebbie gives each Claude task its permissions as it starts it".to_string(),
            "done usage statusLine added to Claude Code's settings.json".to_string(),
            format!("done codex plugin kebbie's plugin at {codex}/plugins/cache/kebbie-tasks/kebbie/local"),
            format!("done codex approvals {codex}/rules/kebbie.rules"),
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
        assert!(shown.contains("✓ in place: machine.name is \"mac\"") && shown.lines().any(|line| line.contains("✓") && line.contains("5 Signals")), "the rail marks what is in place: {shown}");
        rerun.go(Step::Review);
        let plan = plan(&rerun);
        for row in ["kebbie already machine \"mac\"", "herdr already keys kebbie's keys are bound in herdr", "kebbie already backlog vault", "kebbie already signals off", "Claude Code already plugin kebbie's plugin"] {
            assert!(plan.iter().any(|line| line == row), "{row} missing from {plan:?}");
        }
        assert!(plan.iter().any(|line| line.starts_with("Codex already codex plugin")) && plan.iter().any(|line| line.starts_with("Codex already codex approvals")), "{plan:?}");
    }

    #[test]
    fn the_name_is_written_before_mindin_runs_so_a_setup_stopped_there_keeps_it_on_the_next_run() {
        let lab = Lab::new("guide-name-first");
        let mut guide = vault_step(&lab, vec![answering(found("kebbie's [vault] command", "/opt/old-vault"), |probe| probe.setup = false)], Some("/opt/old-vault"));
        guide.go(Step::Name);
        type_text(&mut guide, "work");
        press(&mut guide, &[KeyCode::Enter, KeyCode::Enter]);
        guide.answers.vault = Where { place: Some(Place::S3), endpoint: "https://s3.example.com".to_string(), bucket: "notes".to_string(), ..Where::default() };
        guide.mindin = Some(Mindin { command: vec!["/opt/mindin".to_string()], version: "0.21.0".to_string() });
        guide.go(Step::Review);
        let mut fake = Fake { config: Some(lab.dir.join("kebbie/config.toml")), ..Fake::new() };
        let rows = apply(&guide.plan(), &lab.env(), &mut fake);
        assert_eq!(fake.ran, ["/opt/mindin setup --s3 notes --endpoint https://s3.example.com --signals no"], "only a bucket's setup leaves the screen, to ask for its keys hidden");
        assert!(fake.config_outside.iter().all(|config| config.contains("[machine]\nname = \"work\"\n")), "the name is in config.toml before anything leaves the screen: {:?}", fake.config_outside);
        assert_eq!(rows.iter().map(|(step, _)| step.as_str()).collect::<Vec<_>>(), guide.plan().iter().map(|row| row.step).collect::<Vec<_>>(), "the rows still come back in the plan's order");
        assert_eq!(fake.drawn, rows.len());
        let rerun = Guide::new(Facts::read(&lab.env()), Step::Name);
        assert_eq!(rerun.answers.name, "work", "a run that starts again starts from the name this machine has");
        assert!(plan(&rerun).iter().any(|row| row == "kebbie already machine \"work\""), "{:?}", plan(&rerun));
    }

    #[test]
    fn the_backlog_follows_the_vault_chosen_and_a_rerun_starts_from_the_vault_this_machine_uses() {
        let lab = Lab::new("guide-backlog-follows");
        let mut guide = fresh(&lab);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Enter]);
        assert_eq!((guide.step, guide.answers.vault.chosen()), (Step::Vault, Place::None));
        press(&mut guide, &[KeyCode::Up]);
        assert_eq!(guide.answers.vault.chosen(), Place::Folder);
        press(&mut guide, &[KeyCode::Up, KeyCode::Up]);
        assert_eq!(guide.answers.vault.chosen(), Place::Server);
        press(&mut guide, &[KeyCode::Down]);
        type_text(&mut guide, "https://notes.example.org");
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Vault, "⏎ installs mindin and signs in before the next step");
        finish_job(&mut guide);
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Backlog);
        assert_eq!(guide.answers.chosen_sources(), [Source::Vault], "a vault chosen a step before is where new items go");
        assert!(screen(&guide).contains("[x] Vault  · backlog notes in your notes vault · new items go here"), "{}", screen(&guide));
        let rerun = vault_step(&lab, vec![found("mindin on PATH", "mindin")], None);
        assert_eq!((rerun.answers.vault.chosen(), rerun.answers.vault.address.as_str()), (Place::Server, "https://vault.example.com"), "the server mindin answers with is chosen");
        assert_eq!(rerun.answers.chosen_sources(), [Source::Vault]);
        let mut chosen = vault_step(&lab, vec![found("mindin on PATH", "mindin")], None);
        chosen.facts.sources = vec![Source::Local];
        chosen.facts.sources_set = true;
        chosen.answers = Answers::from(&chosen.facts);
        chosen.go(Step::Backlog);
        assert_eq!(chosen.answers.chosen_sources(), [Source::Local], "sources this machine chose stay as they are");
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
        assert!(rows.iter().any(|(step, outcome)| step == "codex plugin" && outcome.verb == "done" && outcome.detail.contains("kebbie-tasks")), "{rows:?}");
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
        assert!(rows.iter().any(|(step, outcome)| step == "codex plugin" && outcome.verb == "done" && outcome.detail.contains("kebbie-tasks")), "{rows:?}");
        assert!(!home.join("plugins/cache/kebbie/kebbie/local").exists());
    }

    #[test]
    fn unticking_codex_takes_kebbies_plugin_out_of_codex() {
        let lab = Lab::new("guide-codex-off");
        crate::agent::codex::install(&lab.dir.join("codex"), "/opt/kebbie").unwrap();
        let mut guide = fresh(&lab);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Down, KeyCode::Char(' ')]);
        assert!(!guide.answers.codex);
        assert_eq!(plan(&guide).iter().filter(|row| row.starts_with("Codex ")).cloned().collect::<Vec<_>>(), ["Codex to do codex plugin kebbie's plugin out of Codex"]);
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
        assert_eq!(guide.answers.chosen_sources(), [Source::Vault], "the vault the machine keeps is where new items go");
        press(&mut guide, &[KeyCode::Down, KeyCode::Down, KeyCode::Char(' '), KeyCode::Left]);
        assert_eq!(guide.step, Step::Vault);
        press(&mut guide, &[KeyCode::Left]);
        assert_eq!(guide.step, Step::Agents, "← goes back one step");
        guide.go(Step::Backlog);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Char('s')]);
        assert_eq!(guide.step, Step::Review);
        assert_eq!(guide.answers.chosen_sources(), [Source::Vault, Source::Linear], "← and ⏎ keep the boxes ticked");
        let plan = plan(&guide);
        assert!(plan.contains(&"kebbie skipped vault command stays as it is".to_string()) && plan.contains(&"kebbie skipped signals stay off".to_string()) && plan.contains(&"kebbie to do backlog vault, linear".to_string()), "{plan:?}");
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
            "mindin already vault server your own server https://notes.example.com · signed in".to_string(),
            format!("kebbie already vault command vault.command = \"{kept}\""),
            "Claude Code to do vault plugin the vault plugin vault@gronare".to_string(),
            format!("Codex to do codex vault [mcp_servers.vault] in {}/codex/config.toml", lab.dir.display()),
        ], "the place in use runs nothing");
        guide.go(Step::Vault);
        press(&mut guide, &[KeyCode::Down]);
        type_text(&mut guide, "https://notes.example.org");
        assert!(screen(&guide).contains("▸ address   https://notes.example.org▏"), "typing replaces the address it was given: {}", screen(&guide));
        assert!(screen(&guide).contains("runs  mindin, installed on ⏎: uv run -q --project /src/notes-vault-mcp notes-vault-mcp has no setup"), "{}", screen(&guide));
        press(&mut guide, &[KeyCode::Enter]);
        let job = guide.vault_job.as_ref().expect("⏎ starts mindin's install and sign-in in the background");
        assert_eq!((job.plan.install.as_ref().map(|install| install.script.as_str()), job.plan.sign_in.clone()), (Some(MINDIN_INSTALL_ONLY), Some(vec!["--server".to_string(), "https://notes.example.org".to_string()])));
        let mindin = lab.dir.join("bin/mindin").display().to_string();
        std::fs::create_dir_all(lab.dir.join("bin")).unwrap();
        crate::executable::write(lab.dir.join("bin/mindin"), "#!/bin/sh\n").unwrap();
        feed(&mut guide, vec![setup_vault::Msg::Installed(Ok(Mindin { command: vec![mindin.clone()], version: "0.21.0".to_string() })), setup_vault::Msg::Said(setup_vault::Said::Signed(signed("Work"))), setup_vault::Msg::Said(setup_vault::Said::Done)]);
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Backlog);
        guide.go(Step::Review);
        let rows = vault_rows(&guide);
        assert_eq!(rows[..4], [
            "mindin already mindin mindin 0.21.0, installed on step 3".to_string(),
            "mindin already vault server your own server https://notes.example.org · signed in: vault Work, read and write".to_string(),
            "mindin to do usage data off".to_string(),
            format!("kebbie to do vault command vault.command = \"{mindin}\""),
        ], "a vault command older than mindin cannot move the vault, so mindin was installed on step 3: {rows:?}");
        assert!(!screen(&guide).contains("in this terminal"), "{}", screen(&guide));
        let vault: Vec<Planned> = guide.plan().into_iter().filter(|row| row.group == MINDIN || matches!(row.step, "vault command" | "codex vault")).collect();
        let mut fake = Fake::new();
        let done = apply(&vault, &lab.env(), &mut fake);
        assert!(fake.ran.is_empty(), "no mindin step leaves the screen: {:?}", fake.ran);
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
        assert_eq!(vault_rows(&guide)[..2], ["mindin to do vault folder the folder /srv/my notes".to_string(), "kebbie to do vault command vault.command = \"mindin\"".to_string()]);
        let said = lab.dir.join("mindin-said");
        let mindin = lab.dir.join("bin/mindin").display().to_string();
        std::fs::create_dir_all(lab.dir.join("bin")).unwrap();
        crate::executable::write(lab.dir.join("bin/mindin"), format!("#!/bin/sh\nprintf '%s|' \"$@\" > '{}'\n", said.display())).unwrap();
        let rows: Vec<Planned> = guide.plan().into_iter().filter(|row| row.group == MINDIN).map(|row| match row.act {
            Act::VaultSetup(_, flags) => Planned { act: Act::VaultSetup(Command::Found(vec![mindin.clone()]), flags), ..row },
            _ => row,
        }).collect();
        assert!(screen(&guide).contains("runs: mindin setup --folder '/srv/my notes' --signals no on this screen; the folder must exist"), "{}", screen(&guide));
        let mut fake = Fake::new();
        let done = apply(&rows, &lab.env(), &mut fake);
        assert!(fake.ran.is_empty() && done[0].1.verb == "done", "mindin's setup for a folder asks nothing, so it runs on this screen: {done:?}");
        assert_eq!(std::fs::read_to_string(&said).unwrap(), "setup|--folder|/srv/my notes|--signals|no|", "no installer, a path with a space reaches mindin whole, and the usage answer goes along");
        press(&mut guide, &[KeyCode::Left, KeyCode::Left, KeyCode::Left, KeyCode::Down, KeyCode::Down]);
        assert_eq!(guide.answers.vault.chosen(), Place::None);
        guide.go(Step::Review);
        assert_eq!(vault_rows(&guide), ["kebbie skipped vault command no vault; kebbie keeps tasks without one"], "None runs nothing in the vault and adds it to no agent");
        let mut kept = vault_step(&lab, vec![found("kebbie's [vault] command", "/opt/bin/mindin")], Some("/opt/bin/mindin"));
        press(&mut kept, &[KeyCode::Up]);
        kept.go(Step::Review);
        let without: Vec<Planned> = kept.plan().into_iter().filter(|row| row.group == MINDIN || row.step == "vault command").collect();
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
        assert_eq!(vault_rows(&guide)[0], "mindin already vault bucket an S3 bucket", "the bucket the vault does not name stays as it is");
        guide.go(Step::Vault);
        press(&mut guide, &[KeyCode::Down]);
        type_text(&mut guide, "https://s3.example.com");
        press(&mut guide, &[KeyCode::Down]);
        type_text(&mut guide, "notes");
        press(&mut guide, &[KeyCode::Down]);
        type_text(&mut guide, "team");
        press(&mut guide, &[KeyCode::Enter]);
        guide.go(Step::Review);
        assert_eq!(vault_rows(&guide)[..2], ["mindin to do vault bucket the bucket notes/team at https://s3.example.com".to_string(), "kebbie already vault command vault.command = \"uvx mindin\"".to_string()]);
        assert!(screen(&guide).contains("runs: uvx mindin setup --s3 notes --endpoint https://s3.example.com --prefix team --signals no in this terminal; it asks for the access key and the secret, hidden"), "{}", screen(&guide));
    }

    #[test]
    fn with_nothing_found_a_server_chosen_installs_mindin_and_signs_in_on_the_vault_step() {
        use setup_vault::{Code, Failure, Msg, Said};
        let lab = Lab::new("guide-mindin");
        let mut guide = fresh(&lab);
        guide.go(Step::Vault);
        let shown = screen(&guide);
        assert!(shown.contains("( ) Your own server  address") && shown.contains("▸ (•) None") && shown.contains("runs  mindin, installed on ⏎"), "{shown}");
        press(&mut guide, &[KeyCode::Up, KeyCode::Up, KeyCode::Up, KeyCode::Down]);
        assert_eq!((guide.answers.vault.chosen(), guide.typed()), (Place::Server, Some(Field::Address)));
        type_text(&mut guide, "https://notes.example.org");
        guide.go(Step::Review);
        assert_eq!(plan(&guide)[..2], ["mindin to do mindin installed first, then it runs the vault".to_string(), "mindin to do vault server your own server https://notes.example.org · signs in on step 3".to_string()]);
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Enter)), Next::Stay), "⏎ on the review does not apply a server not yet signed in to");
        assert_eq!(guide.step, Step::Vault, "it goes back to the vault step and starts there");
        assert!(guide.vault_job.as_ref().is_some_and(|job| job.plan.install.is_some() && job.plan.command.is_none()));
        assert!(screen(&guide).contains("installing mindin"), "{}", screen(&guide));
        feed(&mut guide, vec![Msg::Installing("→ installing mindin with uv".to_string()), Msg::Installed(Ok(Mindin { command: vec!["/opt/mindin".to_string()], version: "0.21.0".to_string() })), Msg::Said(Said::Code(Code { uri: "https://notes.example.org/device".to_string(), link: "https://notes.example.org/device?code=WVVD-XQNR".to_string(), code: "WVVD-XQNR".to_string() }))]);
        let shown = screen(&guide);
        for line in ["✓ mindin 0.21.0 installed", "Sign this machine in", "Open https://notes.example.org/device and enter the code", "W V V D - X Q N R", "There you pick the vault this machine uses, an existing one or a new one.", "o opens the browser", "c copies the code", "waiting for you to approve", "o open the browser · c copy the code"] {
            assert!(shown.contains(line), "{line:?} missing from:\n{shown}");
        }
        assert!(!shown.contains("( ) None"), "the sign-in takes the box: {shown}");
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Vault, "⏎ waits for the approval");
        feed(&mut guide, vec![Msg::Said(Said::Signed(signed("Work"))), Msg::Said(Said::Done)]);
        let shown = screen(&guide);
        assert!(shown.contains("✓ signed in: vault Work, read and write") && shown.contains("layout default · 128 notes") && shown.contains("⏎ next: Backlog"), "{shown}");
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Backlog);
        guide.go(Step::Review);
        assert!(plan(&guide).contains(&"mindin already vault server your own server https://notes.example.org · signed in: vault Work, read and write".to_string()), "{:?}", plan(&guide));
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Enter)), Next::Apply), "signed in, the review applies");

        let mut failing = fresh(&lab);
        failing.answers.vault = Where { place: Some(Place::Server), address: "https://192.168.1.20".to_string(), ..Where::default() };
        failing.go(Step::Vault);
        press(&mut failing, &[KeyCode::Enter]);
        let code = Msg::Said(Said::Code(Code { uri: "https://192.168.1.20/device".to_string(), link: String::new(), code: "AAAA-BBBB".to_string() }));
        let error = |kind: &str, message: &str, errno| Msg::Said(Said::Error(Failure { kind: kind.to_string(), message: message.to_string(), errno }));
        feed(&mut failing, vec![Msg::Installed(Ok(Mindin { command: vec!["/opt/mindin".to_string()], version: "0.21.0".to_string() })), code.clone(), error("expired", "the code expired before it was approved; run login again", None)]);
        assert!(screen(&failing).contains("✗ The code expired before it was approved.") && screen(&failing).contains("r gets a new code"), "{}", screen(&failing));
        press(&mut failing, &[KeyCode::Char('r')]);
        assert!(failing.vault_job.as_ref().is_some_and(|job| job.plan.install.is_none() && job.plan.command == Some(vec!["/opt/mindin".to_string()]) && job.state == State::Waiting), "r asks for a new code with the mindin just installed");
        feed(&mut failing, vec![code, error("denied", "the sign-in was denied; no key was stored", None)]);
        assert!(screen(&failing).contains("✗ The sign-in was denied in the browser."), "{}", screen(&failing));
        press(&mut failing, &[KeyCode::Char('r')]);
        feed(&mut failing, vec![error("unreachable", "[Errno 65] No route to host", Some(65))]);
        assert!(screen(&failing).contains("✗ 192.168.1.20 did not answer: [Errno 65] No route to host"), "off macOS error 65 is no permission: {}", screen(&failing));
        failing.facts.os = "macos".to_string();
        let shown = screen(&failing);
        assert!(shown.contains("System Settings → Privacy & Security → Local Network, then restart the terminal") && !shown.contains("did not answer"), "{shown}");
        press(&mut failing, &[KeyCode::Char('e')]);
        assert!(failing.vault_job.is_none() && screen(&failing).contains("(•) Your own server"), "e goes back to the choice");
    }

    #[test]
    fn a_local_vault_this_machine_had_is_offered_to_move_into_the_server_vault_in_the_same_box() {
        use setup_vault::{Msg, Said};
        let lab = Lab::new("guide-mindin-move");
        let mut guide = vault_step(&lab, vec![answering(found("mindin on PATH", "/opt/mindin"), |probe| {
            probe.backend = "local".to_string();
            probe.server.clear();
        })], None);
        guide.answers.vault = Where { place: Some(Place::Server), address: "https://notes.example.org".to_string(), ..Where::default() };
        press(&mut guide, &[KeyCode::Enter]);
        assert!(guide.vault_job.as_ref().is_some_and(|job| job.plan.install.is_none() && job.plan.command == Some(vec!["/opt/mindin".to_string()])), "a mindin with setup signs in without an install");
        feed(&mut guide, vec![Msg::Said(Said::Signed(signed("Work"))), Msg::Said(Said::Move { files: 120, source: "the folder /srv/notes".to_string() })]);
        let shown = screen(&guide);
        assert!(shown.contains("This machine's vault was the folder /srv/notes, with 120 files.") && shown.contains("Bring them into the server vault?  y brings them · n leaves them where they are"), "{shown}");
        press(&mut guide, &[KeyCode::Char('y')]);
        assert_eq!(guide.vault_job.as_ref().map(|job| job.state.clone()), Some(State::Moving));
        feed(&mut guide, vec![Msg::Said(Said::Moved { files: 120 }), Msg::Said(Said::Done)]);
        assert!(screen(&guide).contains("✓ moved 120 files into it"), "{}", screen(&guide));
        press(&mut guide, &[KeyCode::Enter]);
        guide.go(Step::Review);
        assert!(plan(&guide).iter().any(|row| row.ends_with("signed in: vault Work, read and write · 120 files moved in")), "{:?}", plan(&guide));
    }

    #[test]
    fn a_failed_step_outside_names_its_cause_and_waits_for_enter_before_the_screen_returns() {
        assert_eq!(stopped(Some(1), None, "mindin: the code expired before it was approved; run login again"), "the sign-in was not finished before its code expired");
        assert_eq!(stopped(Some(1), None, "mindin: the sign-in was denied; no key was stored"), "the sign-in was denied in the browser");
        assert_eq!(stopped(None, Some(2), "KeyboardInterrupt"), "stopped with Ctrl-C");
        assert_eq!(stopped(Some(2), None, "mindin: no folder at /srv/notes; make it first, or run setup in a terminal"), "no folder at /srv/notes; make it first, or run setup in a terminal");
        assert_eq!((stopped(Some(127), None, "sh: mindin: not found"), stopped(Some(3), None, "")), ("its command was not found".to_string(), "it exited with status 3".to_string()));
        assert_eq!(run_outside("echo 'mindin: the code expired before it was approved; run login again' >&2; exit 1"), Err("the sign-in was not finished before its code expired".to_string()));
        assert_eq!(run_outside("true"), Ok(()));
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"\nleft for the screen".to_vec());
        pause("stopped with Ctrl-C", &mut out, &mut input);
        let said = String::from_utf8(out).unwrap();
        assert_eq!(said.trim_start_matches('\n').lines().count(), 1, "one line: {said:?}");
        assert!(said.starts_with("\n  ") && said.contains("did not finish: stopped with Ctrl-C") && said.contains("⏎ goes back to kebbie setup"), "{said:?}");
        assert_eq!(input.position(), 1, "it reads up to the ⏎ and no further");
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
        assert!(plan(&guide).iter().any(|row| row.starts_with("Codex already codex approvals")), "{:?}", plan(&guide));
        crate::agent::codex_setup::write_rules(&lab.dir.join("codex"), "prefix_rule(pattern=[\"kebbie\"], decision=\"allow\")\n").unwrap();
        let guide = fresh(&lab);
        assert!(plan(&guide).iter().any(|row| row.starts_with("Codex to do codex approvals")));
        let rows = apply(&guide.plan(), &lab.env(), &mut Fake::new());
        assert!(rows.iter().any(|(step, outcome)| step == "codex approvals" && outcome.verb == "done"), "{rows:?}");
        assert_eq!(std::fs::read_to_string(lab.dir.join("codex/rules/kebbie.rules")).unwrap(), rules);
    }

    #[test]
    fn kebbie_setup_codex_without_a_terminal_names_what_yes_also_writes_and_writes_it_with_yes() {
        let lab = Lab::new("guide-codex-offers");
        let named = offers(&lab.env(), Agent::Codex);
        assert_eq!(named[0], "kebbie setup codex --yes also writes:");
        assert!(named.iter().any(|line| line.contains("codex approvals") && line.contains("rules/kebbie.rules")) && named.iter().all(|line| !line.contains("plugin")), "{named:?}");
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
    fn both_signals_start_off_and_the_review_passes_mindins_answer_to_mindin_on_this_screen() {
        let lab = Lab::new("guide-signals-consent");
        let said = lab.dir.join("mindin-said");
        let mindin = lab.dir.join("bin/mindin");
        std::fs::create_dir_all(lab.dir.join("bin")).unwrap();
        crate::executable::write(&mindin, format!("#!/bin/sh\necho \"$@\" >> '{}'\necho 'Anonymous usage data: on'\n", said.display())).unwrap();
        let mut guide = vault_step(&lab, vec![found("mindin on PATH", &mindin.display().to_string())], None);
        assert!(!guide.facts.signals_set && !guide.facts.signals);
        guide.go(Step::Signals);
        let shown = screen(&guide);
        assert!(!guide.answers.signals && !guide.answers.usage, "both are off until you say yes");
        for line in ["Both are off until you say yes. Neither ever sends notes, names, paths or your vault's address.", "▸ [ ] kebbie error signals", "[ ] mindin usage data, once a day  · a random install id", "mindin gets your answer from here and does not ask again."] {
            assert!(shown.contains(line), "{line:?} missing from:\n{shown}");
        }
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Review);
        assert!(plan(&guide).contains(&"kebbie to do signals off".to_string()) && plan(&guide).contains(&"mindin to do usage data off".to_string()), "a No is written too, for each: {:?}", plan(&guide));
        guide.go(Step::Signals);
        press(&mut guide, &[KeyCode::Char(' '), KeyCode::Down, KeyCode::Char(' ')]);
        assert!(guide.answers.signals && guide.answers.usage);
        guide.go(Step::Review);
        let rows: Vec<Planned> = guide.plan().into_iter().filter(|row| matches!(row.step, "signals" | "usage data")).collect();
        assert_eq!(rows.iter().map(|row| format!("{} {}", row.verb, row.detail)).collect::<Vec<_>>(), ["to do on, once a day", "to do on"]);
        assert!(lab.config().is_empty(), "nothing is written before the review applies");
        let mut fake = Fake::new();
        let done = apply(&rows, &lab.env(), &mut fake);
        assert!(done.iter().all(|(_, outcome)| outcome.verb == "done"), "{done:?}");
        assert!(lab.config().contains("[report]\nsignals = true\n"), "{}", lab.config());
        assert_eq!(std::fs::read_to_string(&said).unwrap(), "setup --signals yes\n", "mindin hears the answer and asks nothing");
        assert!(fake.ran.is_empty(), "no step leaves the screen: {:?}", fake.ran);
        let mut none = fresh(&lab);
        none.go(Step::Signals);
        assert!(screen(&none).contains("[ ] mindin usage data, once a day  · only with a mindin vault, chosen on step 3"), "{}", screen(&none));
        press(&mut none, &[KeyCode::Down, KeyCode::Char(' ')]);
        assert!(!none.answers.usage && plan(&none).iter().all(|row| !row.contains("usage data")), "without a vault there is no mindin to tell");
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
        let step = |shell: &str| ending(None, &missing, shell, true)[4].clone();
        assert_eq!(step("/bin/zsh"), "  PATH                 add export PATH=\"$HOME/.local/bin:$PATH\" to ~/.zshrc, then open a new terminal");
        assert!(step("/usr/bin/fish").contains("fish_add_path ~/.local/bin to ~/.config/fish/config.fish"));
        assert!(step("").contains("to your shell's profile"));
        let found = [check(Level::Ok, ON_PATH, "~/.local/bin/kebbie → kebbie", &[])];
        assert!(!ending(None, &found, "/bin/zsh", true).iter().any(|line| line.contains("PATH")), "on PATH, no line");
    }

    #[test]
    fn the_end_counts_the_doctor_and_lists_only_what_needs_the_user_in_two_columns() {
        use crate::doctor::Level;
        let clean = ending(Some(Login::In), &[check(Level::Ok, "git", "/usr/bin/git", &[]), check(Level::Ok, "jq", "/usr/bin/jq", &[])], "/bin/zsh", true);
        assert_eq!(clean, ["", "Setup done · doctor: 2 ok", "", "Next steps", "  prefix+space         tell the lead what you want done"], "the lead is the one step left when nothing else is");
        let unchecked = ending(Some(Login::In), &[check(Level::Ok, "git", "/usr/bin/git", &[]), check(Level::Skip, "claude login", "login not checked", &[])], "/bin/zsh", true);
        assert_eq!(unchecked[1], "Setup done · doctor: 1 ok", "a check that got no answer is neither ok nor to look at");
        let checks = [check(Level::Ok, "git", "/usr/bin/git", &[]), check(Level::Fail, "jq", "jq is missing", &["macOS: brew install jq"]), check(Level::Warn, "status line", "statusLine runs \"my-line\"", &["have your statusLine command pipe its stdin to `kebbie statusline`"])];
        assert_eq!(ending(Some(Login::Out), &checks, "/bin/zsh", true), [
            "",
            "Setup done · doctor: 1 ok, 2 to look at",
            "",
            "Next steps",
            "  codex login          Codex is not logged in, so codex tasks cannot start",
            "  kebbie doctor        says what is left and how to fix it",
            "  prefix+space         tell the lead what you want done",
        ], "a fix setup cannot make is one line to kebbie doctor, never its text");
        assert!(ending(Some(Login::Unknown), &[], "/bin/zsh", true).contains(&"  codex login          if you have not yet, so codex tasks can start".to_string()));
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
        assert!(shown.contains("done     link            ~/.local/bin/kebbie") && shown.contains("to do    machine ") && shown.contains("applying…") && !shown.contains("Your board on your phone"), "{shown}");
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
        guide.ending = ending(Some(Login::Out), &ok, "/bin/zsh", true);
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
            "h connects your phone · ⏎ opens your board",
        ] {
            assert!(shown.contains(line), "{line:?} missing from:\n{shown}");
        }
        assert!(!shown.contains("already"), "the done list names what changed: {shown}");
        assert!(shown.contains("Follow tasks and answer questions from anywhere, end-to-end") && shown.contains("Free on hub.kebbie.dev, or run your own."), "{shown}");
        assert!(guide.text().ends_with("  prefix+space         tell the lead what you want done\n\nYour board on your phone: kebbie hub connect, or H on the board\nkebbie.dev/hub"), "{}", guide.text());
        assert!(!guide.said(false).contains("kebbie.dev/hub"), "the connect that h runs follows the summary instead");
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Char('h'))), Next::Connect));
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Enter)), Next::Board), "⏎ opens the board");
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Char('x'))), Next::Stay) && matches!(guide.key(KeyEvent::from(KeyCode::Char('r'))), Next::Stay), "another key does nothing, and r only with a failed step");
        guide.facts.hub = Some("hub.kebbie.dev".to_string());
        let shown = screen(&guide);
        assert!(!shown.contains("Your board on your phone") && shown.contains("⏎ opens your board") && !shown.contains("h connects"), "a connected machine is offered nothing: {shown}");
        assert!(!guide.text().contains("kebbie.dev/hub"));
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Char('h'))), Next::Stay), "h does nothing once connected");
    }

    #[test]
    fn setup_outside_herdr_ends_with_how_to_start_herdr_and_never_opens_the_board() {
        let lab = Lab::new("guide-outside-herdr");
        let mut guide = finished(&lab);
        guide.opens_board = opens_board(false, false);
        guide.ending = ending(Some(Login::In), &[], "/bin/zsh", false);
        let shown = screen(&guide);
        assert!(shown.contains("herdr                start it: kebbie's lead and board run inside it") && shown.contains("h connects your phone · ⏎ ends setup") && !shown.contains("opens your board"), "{shown}");
        assert!(guide.text().contains("Next steps\n  herdr                start it: kebbie's lead and board run inside it\n  prefix+space         tell the lead what you want done"), "{}", guide.text());
        assert_eq!((after(&Next::Board, false), after(&Next::Connect, false)), (After::Done, After::Done), "outside herdr neither ⏎ nor h opens the board");
        assert_eq!((after(&Next::Board, true), after(&Next::Connect, true), after(&Next::Quit, true)), (After::Lead, After::Board, After::Done));
        assert!(!ending(Some(Login::In), &[], "/bin/zsh", true).iter().any(|line| line.contains("start it")), "inside herdr there is nothing to start");
        use crate::doctor::{Level, NOT_ON_PATH, ON_PATH};
        let checks = [check(Level::Warn, ON_PATH, NOT_ON_PATH, &[]), check(Level::Fail, "jq", "jq is missing", &[])];
        let steps: Vec<String> = ending(Some(Login::Out), &checks, "/bin/zsh", false).iter().skip(4).map(|line| line.split_whitespace().next().unwrap_or_default().to_string()).collect();
        assert_eq!(steps, ["herdr", "PATH", "codex", "kebbie", "prefix+space"], "herdr comes first, before every other next step");
        guide.opens_board = opens_board(false, true);
        let shown = screen(&guide);
        assert!(shown.contains("herdr                start it") && shown.contains("⏎ opens your board") && !shown.contains("ends setup"), "kebbie board outside herdr opens the board after its guide, and its hint says so: {shown}");
        assert!(opens_board(true, false) && opens_board(true, true), "inside herdr ⏎ always leads on to the board");
    }

    #[test]
    fn a_failed_step_is_counted_in_the_headline_named_with_its_retry_and_r_runs_only_the_failed_rows() {
        let lab = Lab::new("guide-retry");
        let mut guide = finished(&lab);
        let codex = lab.dir.join("codex");
        guide.planned = vec![
            Planned { group: KEBBIE, step: "machine", verb: "to do", detail: "\"Hetzner devbox\"".to_string(), act: Act::Name("Hetzner devbox".to_string()) },
            Planned { group: CODEX, step: "codex approvals", verb: "to do", detail: "rules".to_string(), act: Act::Rules(guide.facts.rules.clone()) },
        ];
        guide.applied = Some(vec![("machine".to_string(), Outcome::new("done", "\"Hetzner devbox\"")), ("codex approvals".to_string(), Outcome::new("failed", "Codex's home is not writable"))]);
        let shown = screen(&guide);
        assert!(shown.contains("Setup done, 1 step failed · doctor: 36 ok"), "{shown}");
        assert!(shown.contains("failed   codex approvals Codex's home is not writable") && shown.contains("r tries it again · or later: kebbie setup codex --yes"), "the failed row says why, and how to run it again: {shown}");
        assert!(shown.contains("r tries the failed steps again · h connects your phone · ⏎ opens your board"), "{shown}");
        assert!(guide.text().contains("Setup done, 1 step failed · doctor: 36 ok"), "{}", guide.text());
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Char('r'))), Next::Retry));
        let (before, at, rows) = guide.retried();
        assert_eq!((at.clone(), rows.iter().map(|row| row.step).collect::<Vec<_>>()), (vec![1], vec!["codex approvals"]), "only the failed row runs again");
        let again = apply(&rows, &lab.env(), &mut Fake::new());
        guide.applied = Some(merged(&before, &at, &again));
        assert!(lab.config().is_empty(), "the machine row that was done is not written again");
        assert!(codex.join("rules/kebbie.rules").is_file());
        assert_eq!(guide.applied.as_ref().unwrap().iter().map(|(step, outcome)| format!("{} {step}", outcome.verb)).collect::<Vec<_>>(), ["done machine", "done codex approvals"]);
        let shown = screen(&guide);
        assert!(shown.contains("Setup done · doctor: 36 ok") && !shown.contains("r tries"), "{shown}");
    }

    #[test]
    fn a_long_detail_wraps_under_its_column_and_the_done_list_caps_it_at_two_lines() {
        let lab = Lab::new("guide-wrap");
        let mut guide = finished(&lab);
        let error = "claude plugin install vault@gronare failed: ✘ Failed to install plugin \"vault@gronare\": Failed to clone repository\nfatal: Could not read from remote repository.\n\nPlease make sure you have the correct access rights\nand the repository exists.";
        guide.applied.as_mut().unwrap().push(("vault plugin".to_string(), Outcome::new("failed", error)));
        let shown = render(&guide, 110, 40);
        let rows: Vec<&str> = shown.lines().skip_while(|line| !line.contains("failed   vault plugin")).take(3).collect();
        let column = rows[0][..rows[0].find("claude plugin").unwrap()].chars().count();
        assert_eq!(rows[1].chars().position(|character| !matches!(character, ' ' | '│')), Some(column), "the second line starts under the detail, not at the box's edge:\n{shown}");
        assert!(rows[1].trim_end_matches([' ', '│']).ends_with(MORE) && !rows[2].contains("repository exists"), "two lines, then where the rest is:\n{shown}");
        assert_eq!(one_line(error).matches("repository. Please").count(), 1, "a newline in a tool's error becomes a space");
        assert!(!one_line(error).contains("rightsand"));
        assert_eq!(wrapped("abcdefghij kl", 4), ["abcd", "efgh", "ij", "kl"], "a word longer than the column is split");
        guide.done = false;
        guide.applied = Some(vec![("vault plugin".to_string(), Outcome::new("failed", error))]);
        let review = render(&guide, 70, 40);
        assert!(review.contains("and the repository exists.") && !review.contains(MORE), "the review before the end is never cut short:\n{review}");
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
