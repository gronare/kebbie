use std::path::PathBuf;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph, Wrap};
use serde_json::{Value, json};

use crate::board::widgets::{FAINT, GREEN, MANTLE, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, TEXT, bold, style};
use crate::config::{self, Source};
use crate::config_edit::{self, Set};
use crate::doctor::Repair;
use crate::setup::{self, Asking, KeyPlan, Machine, Outcome};
use crate::sh::Result;
use crate::vault_cli::Plugin;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Step {
    Name,
    Agents,
    Keys,
    Backlog,
    Vault,
    Signals,
    Review,
}

impl Step {
    pub const ALL: [Step; 7] = [Step::Name, Step::Agents, Step::Keys, Step::Backlog, Step::Vault, Step::Signals, Step::Review];

    fn index(self) -> usize {
        Step::ALL.iter().position(|step| *step == self).unwrap_or(0)
    }

    fn title(self) -> &'static str {
        match self {
            Step::Name => "Machine name",
            Step::Agents => "Agents",
            Step::Keys => "herdr keys",
            Step::Backlog => "Backlog",
            Step::Vault => "Vault",
            Step::Signals => "Error signals",
            Step::Review => "Review and apply",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Step::Name => "How your phone and your other machines show this one. One word, like mac or devbox.",
            Step::Agents => "Claude Code runs the lead and the reviewer, so it stays on. Turn on Codex to give tasks a codex profile.",
            Step::Keys => "Three keys added to the end of herdr's config.toml. The rest of the file stays as it is. Updating is U on the board.",
            Step::Backlog => "Where work waits. Pick any combination; the first one is where the lead files new ideas.",
            Step::Vault => "Optional. A note for every task and a log line for every push, in a folder of Markdown or an S3 bucket.",
            Step::Signals => "When kebbie itself crashes or hits some internal errors, it sends its maintainers an error signal, at most once a day. A signal holds only kebbie's version and commit, your OS and CPU architecture, the part of kebbie that failed, the kind of failure and a count; never your code, paths or text. Once a day it also sends an install signal, so they can count active installs: a random id made for this install, kebbie's version, your OS and CPU architecture and the update channel.",
            Step::Review => "Nothing is written until you press ⏎ here.",
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

const S3_FIELDS: [(&str, &str); 4] = [("endpoint", "s3_endpoint"), ("bucket", "s3_bucket"), ("key", "s3_access_key"), ("secret", "s3_secret_key")];

#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    pub claude_bin: String,
    pub gronare: bool,
    pub name: String,
    pub name_set: bool,
    pub claude: Option<String>,
    pub kebbie_plugin: bool,
    pub codex: Option<String>,
    pub codex_bin: Option<PathBuf>,
    pub codex_plugin: bool,
    pub keys: KeyPlan,
    pub sources: Vec<Source>,
    pub sources_set: bool,
    pub vault: Plugin,
    pub vault_path: String,
    pub s3: [String; 4],
    pub hub: Option<String>,
    pub signals: bool,
    pub signals_set: bool,
    pub repairs: Vec<Repair>,
}

fn version(program: &str) -> Option<String> {
    let said = crate::sh::run(program, &["--version"], None).ok()?;
    said.split_whitespace().find(|word| word.starts_with(|character: char| character.is_ascii_digit())).map(str::to_string).or(Some(String::new()))
}

fn read_json(path: &std::path::Path) -> Value {
    std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

impl Facts {
    pub fn read(env: &Env) -> Facts {
        let machine = config::check_machine(&env.config).map(|(machine, _)| machine).unwrap_or_default();
        let codex = crate::agent::codex::availability(&env.machine.path, env.codex_home.as_deref(), &env.kebbie);
        let options = read_json(&env.machine.claude_dir.join("settings.json"))["pluginConfigs"].as_object().into_iter().flatten()
            .find(|(id, _)| id.starts_with("vault@")).map(|(_, config)| config["options"].clone()).unwrap_or_default();
        let option = |name: &str| options[name].as_str().unwrap_or_default().to_string();
        let installed = read_json(&env.machine.claude_dir.join("plugins/installed_plugins.json"));
        let herdr = crate::sh::run(&env.machine.herdr, &["plugin", "list", "--json", "--plugin", "kebbie"], None).ok();
        let found = crate::doctor::Found {
            claude_settings: &read_json(&env.machine.claude_dir.join("settings.json")),
            keeper_on: machine.keeper.unwrap_or(true),
            keeper_running: crate::keeper::holder(&env.state).is_some(),
            herdr_list: herdr.as_deref(),
        };
        Facts {
            claude_bin: env.machine.claude.clone(),
            gronare: setup::marketplace_known(&env.machine.claude_dir, "gronare"),
            name: machine.machine_name.clone().unwrap_or_else(config::short_hostname),
            name_set: machine.machine_name.is_some(),
            claude: version(&env.machine.claude),
            kebbie_plugin: installed["plugins"].as_object().is_some_and(|plugins| plugins.keys().any(|id| id.starts_with("kebbie@"))),
            codex: codex.binary.as_ref().map(|_| codex.version.as_deref().and_then(|text| text.split_whitespace().find(|word| word.starts_with(|character: char| character.is_ascii_digit()))).unwrap_or_default().to_string()),
            codex_bin: codex.binary.clone(),
            codex_plugin: codex.plugin != crate::agent::codex::Installed::Missing,
            keys: setup::key_plan(&env.machine.herdr_config),
            sources: machine.sources.clone().unwrap_or_else(|| vec![Source::Local]),
            sources_set: machine.sources.is_some(),
            vault: crate::vault_cli::plugin(&env.machine.claude_dir),
            vault_path: option("vault_path"),
            s3: S3_FIELDS.map(|(_, name)| option(name)),
            hub: crate::hub::identity::load(&env.state).ok().flatten().map(|identity| crate::hub::host(&identity.url).to_string()),
            signals: machine.report.signals(),
            signals_set: machine.report.signals.is_some(),
            repairs: crate::doctor::repairs(&found),
        }
    }

    fn vault_choice(&self) -> usize {
        match (&self.vault, self.vault_path.is_empty(), self.s3[1].is_empty()) {
            (Plugin::Missing, _, _) => 0,
            (_, false, _) => 1,
            (_, true, false) => 2,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum KeyChoice {
    Add,
    Move,
    Skip,
}

fn key_choices(plan: &KeyPlan) -> Vec<KeyChoice> {
    if plan.blocked.is_some() || plan.missing.is_empty() {
        return Vec::new();
    }
    let taken: Vec<_> = plan.missing.iter().filter(|wanted| wanted.taken.is_some()).collect();
    if !taken.is_empty() && taken.iter().all(|wanted| wanted.free.is_some()) { vec![KeyChoice::Add, KeyChoice::Move, KeyChoice::Skip] } else { vec![KeyChoice::Add, KeyChoice::Skip] }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Answers {
    pub name: String,
    pub codex: bool,
    pub keys: usize,
    pub sources: Vec<(Source, bool)>,
    pub vault: usize,
    pub vault_path: String,
    pub s3: [String; 4],
    pub signals: usize,
}

impl Answers {
    fn from(facts: &Facts) -> Answers {
        let mut sources: Vec<(Source, bool)> = facts.sources.iter().map(|source| (*source, true)).collect();
        sources.extend(Source::ALL.into_iter().filter(|source| !facts.sources.contains(source)).map(|source| (source, false)));
        Answers {
            name: facts.name.clone(),
            codex: facts.codex_plugin,
            keys: 0,
            sources,
            vault: facts.vault_choice(),
            vault_path: facts.vault_path.clone(),
            s3: facts.s3.clone(),
            signals: usize::from(!facts.signals),
        }
    }

    fn chosen_sources(&self) -> Vec<Source> {
        self.sources.iter().filter(|(_, on)| *on).map(|(source, _)| *source).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Field {
    Name,
    Path,
    S3(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    Choice(usize),
    Check(usize),
    Field(Field),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Link,
    Plugin(Option<bool>),
    Name(String),
    Keys(Box<KeyPlan>, Vec<toml::Table>),
    Sources(Vec<Source>),
    Vault(Vec<Vec<String>>, Value, String),
    Signals(bool),
    Repair(Repair),
    Nothing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
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

pub struct Guide {
    pub step: Step,
    cursor: usize,
    pub answers: Answers,
    facts: Facts,
    visited: [bool; Step::ALL.len()],
    skipped: [bool; Step::ALL.len()],
    fresh_name: bool,
    error: Option<String>,
    pub applied: Option<Vec<(String, Outcome)>>,
    pub ending: Vec<String>,
}

fn home(path: &std::path::Path) -> String {
    crate::area_page::home_relative(path)
}

fn expand(path: &str) -> String {
    config::skill_path(path.trim()).map_or_else(|| path.trim().to_string(), |path| path.display().to_string())
}

fn source_line(source: Source) -> (&'static str, &'static str) {
    match source {
        Source::Local => ("Local file", "backlog.md beside your config"),
        Source::Vault => ("Vault", "backlog notes in your notes vault"),
        Source::Linear => ("Linear", "issues from a Linear team, needs an API key"),
    }
}

fn what(wanted: &setup::Wanted) -> String {
    let said = setup::text(&wanted.binding, "description");
    said.strip_prefix("kebbie: ").unwrap_or(said).to_string()
}

fn short(wanted: &setup::Wanted) -> String {
    what(wanted).trim_start_matches("open ").trim_start_matches("go to ").to_string()
}

impl Guide {
    pub fn new(facts: Facts, at: Step) -> Guide {
        let answers = Answers::from(&facts);
        let mut guide = Guide { step: Step::Name, cursor: 0, answers, facts, visited: [false; Step::ALL.len()], skipped: [false; Step::ALL.len()], fresh_name: true, error: None, applied: None, ending: Vec::new() };
        guide.go(at);
        guide
    }

    fn spots(&self) -> Vec<Spot> {
        match self.step {
            Step::Name => vec![Spot::Field(Field::Name)],
            Step::Agents => vec![Spot::Check(0), Spot::Check(1)],
            Step::Keys => (0..key_choices(&self.facts.keys).len()).map(Spot::Choice).collect(),
            Step::Backlog => (0..self.answers.sources.len()).map(Spot::Check).collect(),
            Step::Vault => match self.answers.vault {
                1 => vec![Spot::Choice(0), Spot::Choice(1), Spot::Field(Field::Path), Spot::Choice(2)],
                2 => [Spot::Choice(0), Spot::Choice(1), Spot::Choice(2)].into_iter().chain((0..4).map(|at| Spot::Field(Field::S3(at)))).collect(),
                _ => (0..3).map(Spot::Choice).collect(),
            },
            Step::Signals => vec![Spot::Choice(0), Spot::Choice(1)],
            Step::Review => Vec::new(),
        }
    }

    fn spot(&self) -> Option<Spot> {
        self.spots().get(self.cursor).copied()
    }

    fn chosen(&self) -> usize {
        match self.step {
            Step::Keys => self.answers.keys,
            Step::Vault => self.answers.vault,
            Step::Signals => self.answers.signals,
            _ => 0,
        }
    }

    fn go(&mut self, step: Step) {
        self.step = step;
        self.error = None;
        self.fresh_name = true;
        self.cursor = self.spots().iter().position(|spot| *spot == Spot::Choice(self.chosen())).unwrap_or(0);
    }

    fn choose(&mut self, at: usize) {
        match self.step {
            Step::Keys => self.answers.keys = at,
            Step::Vault => self.answers.vault = at,
            Step::Signals => self.answers.signals = at,
            _ => {}
        }
    }

    fn field(&mut self, field: Field) -> &mut String {
        match field {
            Field::Name => &mut self.answers.name,
            Field::Path => &mut self.answers.vault_path,
            Field::S3(at) => &mut self.answers.s3[at],
        }
    }

    fn type_char(&mut self, field: Field, character: char) {
        if field == Field::Name && self.fresh_name {
            self.answers.name.clear();
        }
        self.fresh_name = false;
        self.field(field).push(character);
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(Spot::Field(field)) = self.spot() {
            self.fresh_name = false;
            self.field(field).push_str(text.trim());
        }
    }

    fn reset(&mut self, step: Step) {
        let today = Answers::from(&self.facts);
        match step {
            Step::Name => self.answers.name = today.name,
            Step::Agents => self.answers.codex = today.codex,
            Step::Keys => self.answers.keys = key_choices(&self.facts.keys).iter().position(|choice| *choice == KeyChoice::Skip).unwrap_or(0),
            Step::Backlog => self.answers.sources = today.sources,
            Step::Vault => (self.answers.vault, self.answers.vault_path, self.answers.s3) = (today.vault, today.vault_path, today.s3),
            Step::Signals => self.answers.signals = today.signals,
            Step::Review => {}
        }
    }

    fn check(&self) -> std::result::Result<(), String> {
        match self.step {
            Step::Name => config::valid_machine_name(&self.answers.name).map(|_| ()),
            Step::Vault if self.answers.vault == 1 && self.answers.vault_path.trim().is_empty() => Err("Type the vault folder's path, or pick None.".to_string()),
            Step::Vault if self.answers.vault == 2 && matches!(self.facts.vault, Plugin::Enabled { .. }) && self.answers.s3 == self.facts.s3 => Ok(()),
            Step::Vault if self.answers.vault == 2 => match S3_FIELDS.iter().zip(&self.answers.s3).find(|(_, value)| value.trim().is_empty()) {
                Some(((name, _), _)) => Err(format!("Type the bucket's {name}, or pick None.")),
                None => Ok(()),
            },
            _ => Ok(()),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Next {
        if crate::keys::ctrl_c(&key) {
            return Next::Quit;
        }
        if self.applied.is_some() {
            return match key.code {
                _ if self.ending.is_empty() => Next::Stay,
                KeyCode::Char('h') if self.offers_hub() => Next::Connect,
                _ => Next::Board,
            };
        }
        let spots = self.spots();
        let field = match self.spot() { Some(Spot::Field(field)) => Some(field), _ => None };
        match key.code {
            KeyCode::Esc => return Next::Quit,
            KeyCode::Up | KeyCode::Down if !spots.is_empty() => {
                let count = spots.len();
                self.cursor = if key.code == KeyCode::Up { (self.cursor + count - 1) % count } else { (self.cursor + 1) % count };
                if let Some(Spot::Choice(at)) = self.spot() {
                    self.choose(at);
                }
                self.error = None;
            }
            KeyCode::Char(character) if field.is_some() => self.type_char(field.unwrap_or(Field::Name), character),
            KeyCode::Backspace if field.is_some() => {
                self.fresh_name = false;
                self.field(field.unwrap_or(Field::Name)).pop();
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
                    self.go(Step::ALL[self.step.index() + 1]);
                }
                Err(error) => self.error = Some(error),
            },
            KeyCode::Left if self.step != Step::Name => self.go(Step::ALL[self.step.index() - 1]),
            KeyCode::Char('s') if !matches!(self.step, Step::Name | Step::Review) => {
                self.reset(self.step);
                self.skipped[self.step.index()] = true;
                self.visited[self.step.index()] = false;
                self.go(Step::ALL[self.step.index() + 1]);
            }
            _ => {}
        }
        Next::Stay
    }

    fn toggle(&mut self, at: usize) {
        match self.step {
            Step::Agents if at == 1 && self.facts.codex.is_some() => self.answers.codex = !self.answers.codex,
            Step::Backlog => {
                if let Some((_, on)) = self.answers.sources.get_mut(at) {
                    *on = !*on;
                }
            }
            _ => {}
        }
    }

    fn in_place(&self, step: Step) -> Option<String> {
        let facts = &self.facts;
        match step {
            Step::Name if facts.name_set => Some(format!("machine.name is \"{}\"", facts.name)),
            Step::Agents if facts.kebbie_plugin => Some(format!("kebbie's plugin is in Claude Code{}", if facts.codex_plugin { " and in Codex" } else { "" })),
            Step::Keys if facts.keys.blocked.is_none() && facts.keys.missing.is_empty() => Some(format!("{} binds kebbie's keys", home(&facts.keys.config))),
            Step::Backlog if facts.sources_set => Some(format!("board.backlog.sources is [{}]", facts.sources.iter().map(|source| format!("{:?}", source.key())).collect::<Vec<_>>().join(", "))),
            Step::Vault if matches!(facts.vault, Plugin::Enabled { .. }) && facts.vault_choice() > 0 => Some(match facts.vault_choice() { 1 => format!("the vault plugin points at {}", facts.vault_path), _ => format!("the vault plugin points at the S3 bucket {}", facts.s3[1]) }),
            Step::Signals if facts.signals_set => Some(format!("report.signals is {}", facts.signals)),
            _ => None,
        }
    }

    pub fn plan(&self) -> Vec<Planned> {
        let (facts, answers) = (&self.facts, &self.answers);
        let skipped = |step: Step| self.skipped[step.index()];
        let row = |step, verb, detail: String, act| Planned { step, verb, detail, act };
        let mut rows = vec![row("link", "to do", "~/.local/bin/kebbie".to_string(), Act::Link)];
        let codex = (answers.codex != facts.codex_plugin || answers.codex).then_some(answers.codex).filter(|_| facts.codex.is_some() && !skipped(Step::Agents));
        rows.push(row("plugin", "to do", plugin_shown(codex), Act::Plugin(codex)));
        rows.push(match () {
            _ if skipped(Step::Name) => row("machine", "skipped", "stays as it is".to_string(), Act::Nothing),
            _ if facts.name_set && answers.name.trim() == facts.name => row("machine", "already", format!("\"{}\"", facts.name), Act::Nothing),
            _ => row("machine", "to do", format!("\"{}\"", answers.name.trim()), Act::Name(answers.name.trim().to_string())),
        });
        let choices = key_choices(&facts.keys);
        rows.push(match (choices.get(answers.keys), &facts.keys.blocked) {
            (_, Some(blocked)) => row("keys", "skipped", blocked.clone(), Act::Nothing),
            (None, _) => row("keys", "already", "kebbie's keys are bound in herdr".to_string(), Act::Nothing),
            (Some(KeyChoice::Skip), _) => row("keys", "skipped", "add them by hand from examples/herdr-config.toml".to_string(), Act::Nothing),
            (Some(choice), _) => {
                let bindings: Vec<toml::Table> = facts.keys.missing.iter().filter_map(|wanted| match (&wanted.taken, choice) {
                    (None, _) => Some(wanted.binding.clone()),
                    (Some(_), KeyChoice::Move) => wanted.moved(),
                    _ => None,
                }).collect();
                let shown = bindings.iter().map(|binding| setup::text(binding, "key")).collect::<Vec<_>>().join(", ");
                let unbound = facts.keys.missing.iter().filter(|wanted| wanted.taken.is_some() && *choice == KeyChoice::Add).map(|wanted| format!(", {} unbound", short(wanted)));
                row("keys", "to do", format!("{shown} in herdr{}", unbound.collect::<String>()), Act::Keys(Box::new(facts.keys.clone()), bindings))
            }
        });
        let sources = answers.chosen_sources();
        let listed = sources.iter().map(|source| source.key()).collect::<Vec<_>>().join(", ");
        rows.push(match () {
            _ if skipped(Step::Backlog) => row("backlog", "skipped", "stays as it is".to_string(), Act::Nothing),
            _ if sources == facts.sources => row("backlog", "already", listed, Act::Nothing),
            _ => row("backlog", "to do", listed, Act::Sources(sources)),
        });
        rows.push(self.vault_row());
        let on = answers.signals == 0;
        let said = |on: bool| if on { "on" } else { "off" };
        rows.push(match () {
            _ if skipped(Step::Signals) => row("signals", "skipped", format!("stay {}", said(facts.signals)), Act::Nothing),
            _ if on == facts.signals => row("signals", "already", said(on).to_string(), Act::Nothing),
            _ => row("signals", "to do", said(on).to_string(), Act::Signals(on)),
        });
        rows.extend(facts.repairs.iter().map(|repair| {
            let (step, detail) = repair_shown(*repair);
            row(step, "to do", detail.to_string(), Act::Repair(*repair))
        }));
        rows
    }

    fn vault_row(&self) -> Planned {
        let (facts, answers) = (&self.facts, &self.answers);
        let row = |verb, detail: String, act| Planned { step: "vault", verb, detail, act };
        if self.skipped[Step::Vault.index()] || answers.vault == 0 {
            return row("skipped", "not set up".to_string(), Act::Nothing);
        }
        let (values, shown) = if answers.vault == 1 {
            let path = expand(&answers.vault_path);
            (json!({ "vault_path": path }), format!("folder {}", home(std::path::Path::new(&path))))
        } else {
            let values: serde_json::Map<String, Value> = S3_FIELDS.iter().zip(&answers.s3).map(|((_, name), value)| (name.to_string(), Value::from(value.trim()))).collect();
            (Value::Object(values), format!("S3 bucket {}", answers.s3[1].trim()))
        };
        let same = if answers.vault == 1 { expand(&answers.vault_path) == expand(&facts.vault_path) } else { answers.s3.iter().map(|value| value.trim()).eq(facts.s3.iter().map(|value| value.trim())) };
        if matches!(facts.vault, Plugin::Enabled { .. }) && same {
            return row("already", shown, Act::Nothing);
        }
        let mut steps = setup::steps(&facts.claude_bin, &facts.vault, facts.gronare);
        steps.pop();
        row("to do", format!("{shown}, through the vault plugin"), Act::Vault(steps, values, shown))
    }

    fn offers_hub(&self) -> bool {
        self.facts.hub.is_none()
    }
}

fn plugin_shown(codex: Option<bool>) -> String {
    format!("kebbie in Claude Code{}", match codex { Some(true) => " and Codex", Some(false) => ", out of Codex", None => "" })
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

fn plugin(env: &Env, codex: Option<bool>) -> Outcome {
    let claude = setup::claude_plugin(env.machine);
    let shown = plugin_shown(codex);
    let codex = codex.map(|on| {
        let codex_home = env.codex_home.as_deref().ok_or("neither CODEX_HOME nor HOME is set")?;
        if on { crate::agent::codex::install(codex_home, &env.kebbie).map(|written| written.is_empty()) } else { crate::agent::codex::uninstall(codex_home).map(|_| false) }
    });
    match (claude.verb, codex) {
        (_, Some(Err(error))) => Outcome::new("failed", format!("Codex: {error}")),
        ("failed" | "skipped", _) => claude,
        ("already", None | Some(Ok(true))) => Outcome::new("already", shown),
        _ => Outcome::new("done", shown),
    }
}

fn vault(env: &Env, steps: &[Vec<String>], values: &Value, shown: &str) -> Outcome {
    for step in steps {
        if let Err(error) = crate::sh::run(&step[0], &step[1..].iter().map(String::as_str).collect::<Vec<_>>(), None) {
            return Outcome::new("failed", error);
        }
    }
    let claude = &env.machine.claude;
    match crate::sh::run_with_input(claude, &["plugin", "configure", setup::VAULT_PLUGIN, "--values-stdin"], &values.to_string()) {
        Ok(_) => Outcome::new("done", shown),
        Err(_) => Outcome::new("failed", format!("{claude} plugin configure {} did not take the vault's values; run kebbie setup vault again, or set them in Claude Code's /plugin screen", setup::VAULT_PLUGIN)),
    }
}

fn keys(env: &Env, plan: &KeyPlan, bindings: &[toml::Table], planned: &str) -> Outcome {
    let (outcome, wrote) = setup::add_keys(plan, bindings);
    if !wrote {
        return outcome;
    }
    let reloaded = if setup::reload_herdr(env.machine) == setup::Reload::Done { "herdr reloaded" } else { "restart herdr to use them" };
    when_done(outcome, format!("{}, {reloaded}", planned.replacen(" in herdr", " added", 1)))
}

fn repair(env: &Env, repair: Repair) -> Outcome {
    let said = |result: Result<String>, done: &str| match result {
        Ok(_) => Outcome::new("done", done),
        Err(error) => Outcome::new("failed", error),
    };
    match repair {
        Repair::StatusLine => setup::add_status_line(&env.machine.claude_dir.join("settings.json")),
        Repair::KeeperStart => said(crate::keeper::start_in(&env.state, std::path::Path::new(&env.kebbie), true), "started"),
        Repair::KeeperStop => said(crate::keeper::stop_in(&env.state, std::time::Duration::from_secs(10)), "stopped"),
        Repair::HerdrPlugin => said(crate::sh::run(&env.machine.herdr, &["plugin", "enable", "kebbie"], None), "kebbie's plugin enabled"),
    }
}

pub fn apply(planned: &[Planned], env: &Env, hooks: &mut dyn Hooks) -> Vec<(String, Outcome)> {
    let mut done: Vec<(String, Outcome)> = Vec::new();
    for row in planned {
        let outcome = match &row.act {
            Act::Nothing => Outcome::new(row.verb, row.detail.clone()),
            Act::Link => setup::link_binary(&env.machine.home, &env.machine.path),
            Act::Plugin(codex) => plugin(env, *codex),
            Act::Name(name) => when_done(write_key(env, "machine.name", &Set::Text(name.clone())), row.detail.clone()),
            Act::Keys(plan, bindings) => keys(env, plan, bindings, &row.detail),
            Act::Sources(sources) => when_done(write_key(env, "board.backlog.sources", &Set::List(sources.iter().map(|source| source.key().to_string()).collect())), row.detail.clone()),
            Act::Vault(steps, values, shown) => vault(env, steps, values, shown),
            Act::Signals(on) => when_done(write_key(env, "report.signals", &Set::Bool(*on)), row.detail.clone()),
            Act::Repair(fix) => repair(env, *fix),
        };
        done.push((row.step.to_string(), outcome));
        hooks.progress(&done);
    }
    done
}

pub fn without_asking(env: &Env, asking: Asking) -> Vec<(String, Outcome)> {
    let facts = Facts::read(env);
    let (mut keys, wrote) = setup::keys_without_asking(&facts.keys, asking);
    keys.detail.push_str(&setup::herdr_reload(env.machine, wrote));
    [("link", setup::link_binary(&env.machine.home, &env.machine.path)), ("plugin", setup::claude_plugin(env.machine)), ("keys", keys)].into_iter().map(|(step, outcome)| (step.to_string(), outcome)).collect()
}

const DONE: &str = "Setup done";
const NEXT: &str = "Next steps";
const LEAD: (&str, &str) = ("prefix+space", "tell the lead what you want done");
pub const PHONE: [&str; 3] = ["", "Your board on your phone: kebbie hub connect, or H on the board", "kebbie.dev/hub"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CodexLogin {
    In,
    Out,
    Unknown,
}

// codex login status exits 0 when logged in and 1 when not; anything else is no answer.
pub fn codex_login(codex: Option<&std::path::Path>) -> CodexLogin {
    match codex.map(|codex| crate::sh::attempt(&codex.to_string_lossy(), &["login", "status"], None)) {
        Some(Ok(Ok(_))) => CodexLogin::In,
        Some(Ok(Err(said))) if said.contains("Not logged in") || said.is_empty() => CodexLogin::Out,
        _ => CodexLogin::Unknown,
    }
}

fn next_step(command: &str, why: &str) -> String {
    format!("  {command:<21}{why}")
}

pub fn ending(codex: Option<CodexLogin>, checks: &[crate::doctor::Check]) -> Vec<String> {
    let count = |ok: bool| checks.iter().filter(|check| (check.level == crate::doctor::Level::Ok) == ok).count();
    let open = count(false);
    let left = if open > 0 { format!(", {open} to look at") } else { String::new() };
    let mut lines = vec![String::new(), format!("{DONE} · doctor: {} ok{left}", count(true)), String::new(), NEXT.to_string()];
    match codex {
        Some(CodexLogin::Out) => lines.push(next_step("codex login", "Codex is not logged in, so codex tasks cannot start")),
        Some(CodexLogin::Unknown) => lines.push(next_step("codex login", "if you have not yet, so codex tasks can start")),
        _ => {}
    }
    if open > 0 {
        lines.push(next_step("kebbie doctor", "says what is left and how to fix it"));
    }
    lines.push(next_step(LEAD.0, LEAD.1));
    lines
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

    fn check_mark(on: bool) -> Span<'static> {
        if on { Span::styled("[x]", style(GREEN)) } else { Span::styled("[ ]", style(MUTED)) }
    }

    fn option(&self, spot: Spot, mark: Span<'static>, label: String, about: String) -> Line<'static> {
        let chosen = self.spot() == Some(spot);
        let about = if about.is_empty() { String::new() } else { format!("  · {about}") };
        let line = Line::from(vec![Span::styled(if chosen { "▸ " } else { "  " }, style(MAUVE)), mark, Span::raw(" "), Span::styled(label, if chosen { bold(TEXT) } else { style(TEXT) }), Span::styled(about, style(MUTED))]);
        if chosen { line.style(Style::new().bg(SURFACE)) } else { line }
    }

    fn field_line(&self, field: Field, label: &str, value: &str, secret: bool) -> Line<'static> {
        let chosen = self.spot() == Some(Spot::Field(field));
        let shown = if secret { "•".repeat(value.chars().count()) } else { value.to_string() };
        let caret = if chosen { "▏" } else { "" };
        Line::from(vec![Span::styled(if chosen { "    ▸ " } else { "      " }, style(MAUVE)), Span::styled(format!("{label:<9}"), style(SUBTEXT)), Span::styled(format!("{shown}{caret}"), if chosen { bold(TEXT).bg(MANTLE) } else { style(TEXT) })])
    }

    fn body(&self) -> Vec<Line<'static>> {
        let (facts, answers) = (&self.facts, &self.answers);
        let mut lines = Vec::new();
        match self.step {
            Step::Name => {
                lines.push(self.field_line(Field::Name, "name", &answers.name, false));
                if !facts.name_set {
                    lines.push(Line::from(Span::styled("           from this computer's name", style(MUTED))));
                }
            }
            Step::Agents => {
                let claude = facts.claude.as_ref().map_or("not found · curl -fsSL https://claude.ai/install.sh | bash".to_string(), |version| format!("found {version} · runs the lead, the reviewer and tasks · required"));
                lines.push(self.option(Spot::Check(0), Self::check_mark(true), "Claude Code".to_string(), claude));
                let codex = facts.codex.as_ref().map_or("not found · install Codex CLI first, then run kebbie setup codex".to_string(), |version| format!("found {version} · runs tasks that use a codex profile"));
                lines.push(self.option(Spot::Check(1), Self::check_mark(answers.codex), "Codex".to_string(), codex));
            }
            Step::Keys => {
                let choices = key_choices(&facts.keys);
                let moving = choices.get(answers.keys) == Some(&KeyChoice::Move);
                if let Some(blocked) = &facts.keys.blocked {
                    lines.push(Line::from(Span::styled(blocked.clone(), style(PEACH))));
                }
                for wanted in &facts.keys.missing {
                    let (mark, key, note) = match (&wanted.taken, moving) {
                        (None, _) => (Span::styled("+", style(GREEN)), wanted.key().to_string(), String::new()),
                        (Some(taken), true) => (Span::styled("+", style(GREEN)), wanted.free.clone().unwrap_or_default(), format!(" · moved off {}, which {taken} uses", wanted.key())),
                        (Some(taken), false) => (Span::styled("!", style(PEACH)), wanted.key().to_string(), format!(" · taken by {taken}")),
                    };
                    lines.push(Line::from(vec![Span::raw("  "), mark, Span::styled(format!(" {key:<14}"), style(TEXT)), Span::styled(format!("{}{note}", what(wanted)), style(MUTED))]));
                }
                if !choices.is_empty() {
                    lines.push(Line::default());
                }
                let taken: Vec<&setup::Wanted> = facts.keys.missing.iter().filter(|wanted| wanted.taken.is_some()).collect();
                for (at, choice) in choices.iter().enumerate() {
                    let (label, about) = match choice {
                        KeyChoice::Add if taken.is_empty() => ("Add them".to_string(), format!("to the end of {}", home(&facts.keys.config))),
                        KeyChoice::Add => (format!("Add them, leave {} to {}", taken.iter().map(|wanted| wanted.key()).collect::<Vec<_>>().join(" and "), taken.iter().filter_map(|wanted| wanted.taken.clone()).collect::<Vec<_>>().join(" and ")), "bind the rest yourself later".to_string()),
                        KeyChoice::Move => (format!("Add them, {}", taken.iter().map(|wanted| format!("{} on {}", short(wanted), wanted.free.clone().unwrap_or_default())).collect::<Vec<_>>().join(", ")), "free in your config".to_string()),
                        KeyChoice::Skip => ("Skip".to_string(), "add them by hand".to_string()),
                    };
                    lines.push(self.option(Spot::Choice(at), Self::radio(answers.keys == at), label, about));
                }
            }
            Step::Backlog => {
                let first = answers.sources.iter().position(|(_, on)| *on);
                for (at, (source, on)) in answers.sources.iter().enumerate() {
                    let (label, about) = source_line(*source);
                    let about = if Some(at) == first { format!("{about} · new ideas go here") } else { about.to_string() };
                    lines.push(self.option(Spot::Check(at), Self::check_mark(*on), label.to_string(), about));
                }
            }
            Step::Vault => {
                for (at, (label, about)) in [("None", "kebbie works the same without it"), ("A local folder", ""), ("An S3 bucket", "endpoint, bucket and keys; the secret is masked")].into_iter().enumerate() {
                    lines.push(self.option(Spot::Choice(at), Self::radio(answers.vault == at), label.to_string(), about.to_string()));
                    match (at, answers.vault) {
                        (1, 1) => lines.push(self.field_line(Field::Path, "path", &answers.vault_path, false)),
                        (2, 2) => lines.extend(S3_FIELDS.iter().enumerate().map(|(field, (label, _))| self.field_line(Field::S3(field), label, &answers.s3[field], field == 3))),
                        _ => {}
                    }
                }
                if answers.vault > 0 {
                    let claude = std::path::Path::new(&facts.claude_bin).file_name().map_or(facts.claude_bin.clone(), |name| name.to_string_lossy().to_string());
                    lines.push(Line::default());
                    lines.push(Line::from(Span::styled(format!("runs: {claude} plugin marketplace add {}", setup::VAULT_MARKETPLACE), style(SUBTEXT)).style(Style::new().bg(MANTLE))));
                    lines.push(Line::from(Span::styled(format!("      {claude} plugin install {}", setup::VAULT_PLUGIN), style(SUBTEXT)).style(Style::new().bg(MANTLE))));
                    lines.push(Line::from(vec![Span::styled(format!("      {claude} plugin configure {}", setup::VAULT_PLUGIN), style(SUBTEXT)), Span::styled(format!(" (with the {} above)", if answers.vault == 1 { "path" } else { "bucket" }), style(MUTED))]));
                }
            }
            Step::Signals => {
                let labels = if facts.signals { [("Keep them on", "the default"), ("Turn them off", "writes signals = false under [report]")] } else { [("Turn them on", "writes signals = true under [report]"), ("Keep them off", "signals = false stays under [report]")] };
                for (at, (label, about)) in labels.into_iter().enumerate() {
                    lines.push(self.option(Spot::Choice(at), Self::radio(answers.signals == at), label.to_string(), about.to_string()));
                }
                lines.push(Line::default());
                lines.push(Line::from(Span::styled("A bug report with text is always shown to you first and sent only on your y.", style(SUBTEXT))));
            }
            Step::Review => {
                let rows: Vec<(String, Outcome)> = match &self.applied {
                    Some(done) => {
                        let mut rows = done.clone();
                        rows.extend(self.plan().into_iter().skip(done.len()).map(|row| (row.step.to_string(), Outcome::new("to do", row.detail))));
                        rows
                    }
                    None => self.plan().into_iter().map(|row| (row.step.to_string(), Outcome::new(row.verb, row.detail))).collect(),
                };
                for (step, outcome) in rows {
                    lines.push(Line::from(vec![Span::styled(format!("{:<9}", outcome.verb), style(verb_color(outcome.verb))), Span::styled(format!("{step:<10}"), style(MAUVE)), Span::styled(outcome.detail, style(TEXT))]));
                }
                for said in &self.ending {
                    lines.push(match (said.as_str(), said.get(..23), said.get(23..)) {
                        (_, _, _) if said.starts_with(DONE) => Line::from(vec![Span::styled(DONE, style(GREEN)), Span::styled(said[DONE.len()..].to_string(), style(SUBTEXT))]),
                        (NEXT, _, _) => Line::from(Span::styled(NEXT, bold(MAUVE))),
                        (_, Some(command), Some(why)) => Line::from(vec![Span::styled(command.to_string(), style(TEXT)), Span::styled(why.to_string(), style(SUBTEXT))]),
                        _ => Line::from(Span::styled(said.clone(), style(TEXT))),
                    });
                }
            }
        }
        if let Some(place) = self.in_place(self.step) {
            lines.insert(0, Line::default());
            lines.insert(0, Line::from(vec![Span::styled("✓ in place: ", style(GREEN)), Span::styled(place, style(SUBTEXT))]));
        }
        if let Some(error) = &self.error {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(error.clone(), style(RED))));
        }
        lines
    }

    fn hint(&self) -> String {
        match self.step {
            _ if self.applied.is_some() && self.ending.is_empty() => "applying…".to_string(),
            _ if self.applied.is_some() => "any key opens your board".to_string(),
            Step::Name => "type to change · ⏎ next · esc quit".to_string(),
            Step::Review => "⏎ apply · ← back · esc quit without writing".to_string(),
            step => {
                let toggle = if matches!(step, Step::Agents | Step::Backlog) { "space toggle · " } else { "" };
                let typing = if matches!(self.spot(), Some(Spot::Field(_))) { "type to change · " } else { "" };
                format!("↑ ↓ choose · {toggle}{typing}⏎ next · ← back · s skip · esc quit")
            }
        }
    }

    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area().inner(Margin { horizontal: 2, vertical: 1 });
        let [title, _, body] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(5)]).areas(area);
        let right = "run it again any time · --yes takes the defaults";
        let left = format!("step {} of {}", self.step.index() + 1, Step::ALL.len());
        let gap = (title.width as usize).saturating_sub(14 + left.len() + right.len()).max(2);
        let right = if gap > 2 { right } else { "" };
        frame.render_widget(Paragraph::new(Line::from(vec![Span::styled("kebbie setup", bold(MAUVE)), Span::raw("  "), Span::styled(left, style(MUTED)), Span::raw(" ".repeat(gap)), Span::styled(right, style(MUTED))])), title);
        let main = if body.width >= 72 {
            let [rail, _, main] = Layout::horizontal([Constraint::Length(22), Constraint::Length(2), Constraint::Min(40)]).areas(body);
            let lines: Vec<Line> = Step::ALL.iter().enumerate().map(|(at, step)| {
                let current = *step == self.step;
                let mark = match () {
                    _ if current => Span::styled("▸", style(MAUVE)),
                    _ if self.visited[at] || self.in_place(*step).is_some() => Span::styled("✓", style(GREEN)),
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
        let mut lines = vec![Line::from(Span::styled(self.step.help(), style(SUBTEXT))), Line::default()];
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
        let block = framed(self.step.title());
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
                let mut lines: Vec<String> = rows.iter().map(|(step, outcome)| outcome.line(step)).collect();
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
}

pub fn run(env: &Env, at: Step) -> Result<String> {
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
                        let codex = (guide.answers.codex && !guide.skipped[Step::Agents.index()]).then(|| codex_login(guide.facts.codex_bin.as_deref()));
                        guide.ending = ending(codex, &crate::doctor::checks(&crate::doctor::Env::current()));
                        guide.applied = Some(rows);
                    }
                    next => return Ok(next),
                },
                Event::Paste(text) => guide.paste(&text),
                _ => {}
            }
        }
    })();
    crate::board::terminal::restore_terminal();
    match result? {
        Next::Board => println!("{}\n", guide.text()),
        Next::Connect => {
            println!("{}\n", guide.said(false));
            println!("{}", crate::hub::cli(Some("connect"), &[], None)?);
        }
        _ => return Ok(guide.text()),
    }
    crate::board::run(false)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    struct Fake {
        drawn: usize,
    }

    impl Hooks for Fake {
        fn progress(&mut self, _: &[(String, Outcome)]) {
            self.drawn += 1;
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

    fn screen(guide: &Guide) -> String {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(130, 40)).unwrap();
        terminal.draw(|frame| guide.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height).map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }

    fn plan(guide: &Guide) -> Vec<String> {
        guide.plan().iter().map(|row| format!("{} {} {}", row.verb, row.step, row.detail)).collect()
    }

    fn fresh(lab: &Lab) -> Guide {
        let mut facts = Facts::read(&lab.env());
        facts.name = "mac".to_string();
        facts.claude = Some("2.1.3".to_string());
        facts.codex = Some("0.163.0".to_string());
        Guide::new(facts, Step::Name)
    }

    #[test]
    fn the_guide_walks_seven_steps_with_their_choices_and_writes_nothing_before_apply() {
        let lab = Lab::new("guide-walk");
        let mut guide = fresh(&lab);
        let shown = screen(&guide);
        assert!(shown.contains("kebbie setup  step 1 of 7") && shown.contains("▸ 1 Machine name") && shown.contains("· 7 Review and apply") && !shown.contains("Hub"), "{shown}");
        assert!(shown.contains("▸ name     mac▏") && shown.contains("from this computer's name") && shown.contains("type to change · ⏎ next · esc quit"), "{shown}");
        type_text(&mut guide, "devbox");
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!((guide.step, guide.answers.name.as_str()), (Step::Agents, "devbox"), "typing replaces the name it was given");
        let shown = screen(&guide);
        assert!(shown.contains("✓ 1 Machine name") && shown.contains("[x] Claude Code  · found 2.1.3 · runs the lead, the reviewer and tasks · required") && shown.contains("[ ] Codex  · found 0.163.0"), "{shown}");
        press(&mut guide, &[KeyCode::Char(' ')]);
        assert!(!guide.answers.codex, "space on Claude Code changes nothing: it stays on");
        press(&mut guide, &[KeyCode::Down, KeyCode::Char(' '), KeyCode::Enter]);
        assert!(guide.answers.codex);
        let shown = screen(&guide);
        assert!(shown.contains("+ prefix+space  go to the lead, or back") && shown.contains("+ prefix+t      open the board") && !shown.contains("prefix+u"), "{shown}");
        assert!(shown.contains("(•) Add them  · to the end of") && shown.contains("( ) Skip"), "{shown}");
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Backlog);
        assert!(screen(&guide).contains("[x] Local file  · backlog.md beside your config · new ideas go here"));
        press(&mut guide, &[KeyCode::Down, KeyCode::Char(' '), KeyCode::Up, KeyCode::Char(' '), KeyCode::Enter]);
        assert_eq!(guide.answers.chosen_sources(), [Source::Vault], "the first one checked is where the lead files ideas");
        press(&mut guide, &[KeyCode::Down]);
        assert_eq!(guide.answers.vault, 1, "moving onto an option picks it");
        let shown = screen(&guide);
        assert!(shown.contains("(•) A local folder") && shown.contains("path") && shown.contains("plugin install vault@gronare"), "{shown}");
        press(&mut guide, &[KeyCode::Enter]);
        assert!(screen(&guide).contains("Type the vault folder's path, or pick None."), "an empty path stops the step");
        press(&mut guide, &[KeyCode::Down]);
        type_text(&mut guide, "/notes/vault");
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Signals, "the vault leads straight to the error signals");
        assert!(screen(&guide).contains("never your code, paths or text") && screen(&guide).contains("A bug report with text is always shown to you first"));
        press(&mut guide, &[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(guide.step, Step::Review);
        assert_eq!(plan(&guide), [
            "to do link ~/.local/bin/kebbie",
            "to do plugin kebbie in Claude Code and Codex",
            "to do machine \"devbox\"",
            "to do keys prefix+space, prefix+f, prefix+t in herdr",
            "to do backlog vault",
            "to do vault folder /notes/vault, through the vault plugin",
            "to do signals off",
            "to do usage kebbie's statusLine in Claude Code's settings.json, for the board's usage bar",
            "to do keeper start it, so routines run without an open board",
        ], "the doctor's fixes are rows of the plan like every other");
        let shown = screen(&guide);
        assert!(shown.contains("to do    usage     kebbie's statusLine in Claude Code's settings.json") && shown.contains("⏎ apply · ← back · esc quit without writing"), "{shown}");
        assert_eq!((lab.config(), lab.herdr(), lab.calls()), (String::new(), String::new(), String::new()), "nothing is written or run before ⏎ on the last step");
        assert!(!lab.dir.join("codex/plugins").exists() && !lab.machine.home.join(".local/bin/kebbie").exists() && !lab.machine.claude_dir.join("settings.json").exists());
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Esc)), Next::Quit));
        assert_eq!(guide.text(), "Nothing written: kebbie setup runs the guide again whenever you want.");
    }

    #[test]
    fn apply_writes_and_runs_each_line_then_a_rerun_starts_from_what_is_in_place() {
        let lab = Lab::new("guide-apply");
        let mut guide = fresh(&lab);
        guide.facts.repairs.retain(|repair| *repair != Repair::KeeperStart);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Down, KeyCode::Char(' '), KeyCode::Enter, KeyCode::Enter, KeyCode::Down, KeyCode::Char(' '), KeyCode::Enter, KeyCode::Down, KeyCode::Down]);
        type_text(&mut guide, "/notes/vault");
        press(&mut guide, &[KeyCode::Enter, KeyCode::Down, KeyCode::Enter]);
        assert_eq!(guide.step, Step::Review);
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Enter)), Next::Apply));
        let mut fake = Fake { drawn: 0 };
        let rows = apply(&guide.plan(), &lab.env(), &mut fake);
        let lines: Vec<String> = rows.iter().map(|(step, outcome)| format!("{} {step} {}", outcome.verb, outcome.detail)).collect();
        assert_eq!(lines, [
            "done link ~/.local/bin/kebbie",
            "done plugin kebbie in Claude Code and Codex",
            "done machine \"mac\"",
            "done keys prefix+space, prefix+f, prefix+t added, restart herdr to use them",
            "done backlog local, vault",
            "done vault folder /notes/vault",
            "done signals off",
            "done usage statusLine added to Claude Code's settings.json",
        ], "each line says what it did in a few words");
        assert_eq!(fake.drawn, 8, "each line is drawn as it finishes");
        assert_eq!(lab.config(), "[machine]\nname = \"mac\"\n\n[board.backlog]\nsources = [\"local\", \"vault\"]\n\n[report]\nsignals = false\n", "setup never writes [hub]");
        assert_eq!(lab.herdr().matches("[[keys.command]]").count(), 3);
        assert!(lab.dir.join("codex/plugins/cache/kebbie/kebbie/local/hooks/hooks.json").is_file(), "Codex gets kebbie's plugin");
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(lab.machine.claude_dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(settings["statusLine"]["command"], crate::doctor::STATUS_LINE_COMMAND);
        let calls = lab.calls();
        assert!(calls.contains("plugin marketplace add https://github.com/gronare/claude-plugins.git\nplugin install vault@gronare\nplugin configure vault@gronare --values-stdin\n"), "{calls}");
        assert!(std::fs::read_to_string(lab.dir.join("claude.stdin")).unwrap().contains("{\"vault_path\":\"/notes/vault\"}"), "the vault's values go in on stdin");
        assert!(Facts::read(&lab.env()).repairs.iter().all(|repair| *repair != Repair::StatusLine), "a fix applied is no row the next time");
        let plugins = lab.machine.claude_dir.join("plugins");
        let vault_root = lab.dir.join("vault-plugin");
        std::fs::create_dir_all(vault_root.join("hooks")).unwrap();
        std::fs::write(vault_root.join("hooks/hooks.json"), "uvx notes-vault-mcp==0.6.0 hook session-start").unwrap();
        std::fs::write(plugins.join("installed_plugins.json"), json!({ "plugins": { "kebbie@kebbie": [{ "version": "0.27.0" }], "vault@gronare": [{ "version": "0.3.0", "installPath": vault_root }] } }).to_string()).unwrap();
        std::fs::write(lab.machine.claude_dir.join("settings.json"), json!({ "pluginConfigs": { "vault@gronare": { "options": { "vault_path": "/notes/vault" } } } }).to_string()).unwrap();
        let rerun = Guide::new(Facts::read(&lab.env()), Step::Name);
        let mut signals = Guide::new(Facts::read(&lab.env()), Step::Signals);
        let shown = screen(&signals);
        assert!(shown.contains("( ) Turn them on") && shown.contains("▸ (•) Keep them off"), "signals that are off offer to turn them on: {shown}");
        press(&mut signals, &[KeyCode::Up, KeyCode::Enter]);
        assert_eq!(plan(&signals)[6], "to do signals on");
        assert_eq!(rerun.answers.name, "mac");
        assert_eq!(rerun.answers.chosen_sources(), [Source::Local, Source::Vault]);
        assert_eq!((rerun.answers.signals, rerun.answers.codex, rerun.answers.vault, rerun.answers.vault_path.as_str()), (1, true, 1, "/notes/vault"));
        let shown = screen(&rerun);
        assert!(shown.contains("✓ in place: machine.name is \"mac\"") && shown.contains("✓ 3 herdr keys") && shown.contains("✓ 6 Error signals"), "the rail marks what is in place: {shown}");
        let mut rerun = rerun;
        press(&mut rerun, &[KeyCode::Enter, KeyCode::Enter, KeyCode::Enter, KeyCode::Enter, KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(rerun.step, Step::Review);
        assert_eq!(plan(&rerun)[2..7], ["already machine \"mac\"", "already keys kebbie's keys are bound in herdr", "already backlog local, vault", "already vault folder /notes/vault", "already signals off"]);
    }

    #[test]
    fn s_skips_a_step_and_back_keeps_what_was_chosen() {
        let lab = Lab::new("guide-skip");
        let mut guide = fresh(&lab);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Char('s')]);
        assert_eq!(guide.step, Step::Keys);
        press(&mut guide, &[KeyCode::Char('s'), KeyCode::Down, KeyCode::Char(' '), KeyCode::Left]);
        assert_eq!(guide.step, Step::Keys);
        press(&mut guide, &[KeyCode::Left]);
        assert_eq!(guide.step, Step::Agents);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Char('s'), KeyCode::Enter]);
        assert_eq!(guide.step, Step::Vault);
        assert_eq!(guide.answers.chosen_sources(), [Source::Local, Source::Vault], "← and ⏎ keep the boxes ticked");
        press(&mut guide, &[KeyCode::Down, KeyCode::Char('s')]);
        assert_eq!(guide.answers.vault, 0, "a skip puts the step back to what the machine has");
        press(&mut guide, &[KeyCode::Char('s')]);
        assert_eq!(guide.step, Step::Review);
        let plan = plan(&guide);
        assert!(plan[3].starts_with("skipped keys") && plan[5].starts_with("skipped vault") && plan[6] == "skipped signals stay on", "{plan:?}");
        assert!(plan[4].starts_with("to do backlog"), "{plan:?}");
        press(&mut guide, &[KeyCode::Left]);
        assert_eq!(guide.step, Step::Signals);
        let mut name = fresh(&lab);
        type_text(&mut name, "s");
        assert_eq!((name.step, name.answers.name.as_str()), (Step::Name, "s"), "s types in a text field");
        press(&mut name, &[KeyCode::Backspace, KeyCode::Enter]);
        assert!(screen(&name).contains("machine.name: must be a non-empty string"), "{}", screen(&name));
    }

    #[test]
    fn a_taken_herdr_key_is_named_with_a_free_one_offered() {
        let lab = Lab::new("guide-taken");
        std::fs::create_dir_all(lab.machine.herdr_config.parent().unwrap()).unwrap();
        std::fs::write(&lab.machine.herdr_config, "[keys]\nnew_tab = \"prefix+t\"\n").unwrap();
        let mut guide = fresh(&lab);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Enter]);
        let shown = screen(&guide);
        assert!(shown.contains("! prefix+t      open the board · taken by new_tab"), "{shown}");
        assert!(shown.contains("(•) Add them, leave prefix+t to new_tab  · bind the rest yourself later") && shown.contains("( ) Add them, the board on prefix+y  · free in your config") && shown.contains("( ) Skip  · add them by hand"), "{shown}");
        press(&mut guide, &[KeyCode::Down]);
        assert!(screen(&guide).contains("+ prefix+y      open the board · moved off prefix+t, which new_tab uses"));
        press(&mut guide, &[KeyCode::Enter, KeyCode::Char('s'), KeyCode::Char('s'), KeyCode::Char('s')]);
        assert_eq!(guide.step, Step::Review);
        assert_eq!(plan(&guide)[3], "to do keys prefix+space, prefix+f, prefix+y in herdr");
        let rows = apply(&guide.plan()[3..4], &lab.env(), &mut Fake { drawn: 0 });
        assert_eq!(format!("{} {}", rows[0].1.verb, rows[0].1.detail), "done prefix+space, prefix+f, prefix+y added, restart herdr to use them");
        let herdr = lab.herdr();
        assert!(herdr.starts_with("[keys]\nnew_tab = \"prefix+t\"\n\n[[keys.command]]") && herdr.contains("key = \"prefix+y\"\ntype = \"plugin_action\"\ncommand = \"kebbie.board\""), "only the end of the file changes: {herdr}");
        let mut leave = fresh(&lab);
        std::fs::write(&lab.machine.herdr_config, "[keys]\nnew_tab = \"prefix+t\"\n").unwrap();
        leave.facts.keys = setup::key_plan(&lab.machine.herdr_config);
        press(&mut leave, &[KeyCode::Enter, KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(plan(&leave)[3], "to do keys prefix+space, prefix+f in herdr, the board unbound");
    }

    #[test]
    fn an_s3_vault_masks_its_secret() {
        let lab = Lab::new("guide-s3");
        let mut guide = fresh(&lab);
        guide.go(Step::Vault);
        press(&mut guide, &[KeyCode::Up]);
        assert_eq!(guide.answers.vault, 2);
        for value in ["https://s3.example.com", "notes", "AKIA1", "hunter2"] {
            press(&mut guide, &[KeyCode::Down]);
            type_text(&mut guide, value);
        }
        let shown = screen(&guide);
        assert!(shown.contains("secret   •••••••") && !shown.contains("hunter2"), "{shown}");
        press(&mut guide, &[KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(guide.step, Step::Review);
        let plan = guide.plan();
        assert_eq!(format!("{} {}", plan[5].verb, plan[5].detail), "to do S3 bucket notes, through the vault plugin");
        let rows = apply(&plan[5..6], &lab.env(), &mut Fake { drawn: 0 });
        assert_eq!(format!("{} {}", rows[0].1.verb, rows[0].1.detail), "done S3 bucket notes");
        assert!(!screen(&guide).contains("hunter2"), "the secret is never shown");
        assert!(std::fs::read_to_string(lab.dir.join("claude.stdin")).unwrap().contains("\"s3_secret_key\":\"hunter2\""));
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
        assert_eq!(plan(&guide).last().map(String::as_str), Some("to do keeper start it, so routines run without an open board"));
        assert!(plan(&guide).iter().all(|row| !row.contains("sandbox")), "{:?}", plan(&guide));
    }

    #[test]
    fn yes_takes_the_defaults_and_without_a_terminal_nothing_is_written_to_herdr() {
        let lab = Lab::new("guide-yes");
        let lines: Vec<String> = without_asking(&lab.env(), Asking::Yes).iter().map(|(step, outcome)| outcome.line(step)).collect();
        let verbs: Vec<&str> = lines.iter().map(|line| line.split_whitespace().next().unwrap_or_default()).collect();
        assert_eq!(verbs, ["done", "done", "done"], "{lines:?}");
        assert!(lines[2].contains("added prefix+space, prefix+f, prefix+t to") && lines[2].contains("restart herdr so it reads the new keys"), "{lines:?}");
        assert_eq!(lab.config(), "", "--yes leaves the optional steps as they are");
        assert!(!lab.machine.claude_dir.join("settings.json").exists(), "--yes edits no file of the user's that it did not show");
        let written = lab.herdr();
        let again: Vec<String> = without_asking(&lab.env(), Asking::Yes).iter().map(|(step, outcome)| outcome.line(step)).collect();
        assert!(again[0].starts_with("already") && again[1].starts_with("already") && again[2].starts_with("already"), "{again:?}");
        assert_eq!(lab.herdr(), written);
        let closed = Lab::new("guide-closed");
        let lines: Vec<String> = without_asking(&closed.env(), Asking::Closed).iter().map(|(step, outcome)| outcome.line(step)).collect();
        assert!(lines[2].starts_with("skipped") && lines[2].contains("no terminal to ask in") && lines[2].contains("kebbie setup --yes"), "{lines:?}");
        assert_eq!(closed.herdr(), "");
    }

    fn check(level: crate::doctor::Level, name: &str, fact: &str, fix: &[&str]) -> crate::doctor::Check {
        crate::doctor::Check { level, name: name.to_string(), fact: fact.to_string(), fix: fix.iter().map(|fix| fix.to_string()).collect() }
    }

    #[test]
    fn the_end_counts_the_doctor_and_lists_only_what_needs_the_user_in_two_columns() {
        use crate::doctor::Level;
        let clean = ending(Some(CodexLogin::In), &[check(Level::Ok, "git", "/usr/bin/git", &[]), check(Level::Ok, "jq", "/usr/bin/jq", &[])]);
        assert_eq!(clean, ["", "Setup done · doctor: 2 ok", "", "Next steps", "  prefix+space         tell the lead what you want done"], "the lead is the one step left when nothing else is");
        let checks = [check(Level::Ok, "git", "/usr/bin/git", &[]), check(Level::Fail, "jq", "jq is missing", &["macOS: brew install jq"]), check(Level::Warn, "status line", "statusLine runs \"my-line\"", &["have your statusLine command pipe its stdin to `kebbie statusline`"])];
        assert_eq!(ending(Some(CodexLogin::Out), &checks), [
            "",
            "Setup done · doctor: 1 ok, 2 to look at",
            "",
            "Next steps",
            "  codex login          Codex is not logged in, so codex tasks cannot start",
            "  kebbie doctor        says what is left and how to fix it",
            "  prefix+space         tell the lead what you want done",
        ], "a fix setup cannot make is one line to kebbie doctor, never its text");
        assert!(ending(Some(CodexLogin::Unknown), &[]).contains(&"  codex login          if you have not yet, so codex tasks can start".to_string()));
        let dir = crate::sh::test_dir("guide-codex-login");
        let codex = dir.join("codex");
        crate::executable::write(&codex, "#!/bin/sh\necho 'Not logged in'\nexit 1\n").unwrap();
        assert_eq!(codex_login(Some(&codex)), CodexLogin::Out);
        crate::executable::write(&codex, "#!/bin/sh\necho 'Logged in using ChatGPT'\n").unwrap();
        assert_eq!(codex_login(Some(&codex)), CodexLogin::In);
        crate::executable::write(&codex, "#!/bin/sh\necho 'unknown command' >&2\nexit 2\n").unwrap();
        assert_eq!((codex_login(Some(&codex)), codex_login(None)), (CodexLogin::Unknown, CodexLogin::Unknown));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_guide_opens_at_a_step_and_the_review_shows_each_line_as_it_is_applied() {
        let lab = Lab::new("guide-open");
        let mut guide = Guide::new(Facts::read(&lab.env()), Step::Vault);
        assert_eq!(guide.step, Step::Vault);
        assert!(screen(&guide).contains("▸ 5 Vault"));
        guide.go(Step::Review);
        guide.applied = Some(vec![("link".to_string(), Outcome::new("done", "~/.local/bin/kebbie"))]);
        let shown = screen(&guide);
        assert!(shown.contains("done     link      ~/.local/bin/kebbie") && shown.contains("to do    plugin") && shown.contains("applying…") && !shown.contains("Your board on your phone"), "{shown}");
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
            ("vault".to_string(), Outcome::new("already", "S3 bucket obsidian-work")),
            ("signals".to_string(), Outcome::new("already", "on")),
        ]);
        let ok: Vec<crate::doctor::Check> = (0..36).map(|at| check(crate::doctor::Level::Ok, &format!("check {at}"), "fine", &[])).collect();
        guide.ending = ending(Some(CodexLogin::Out), &ok);
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
}
