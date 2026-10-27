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
    Hub,
    Signals,
    Review,
}

impl Step {
    pub const ALL: [Step; 8] = [Step::Name, Step::Agents, Step::Keys, Step::Backlog, Step::Vault, Step::Hub, Step::Signals, Step::Review];

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
            Step::Hub => "Hub",
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
            Step::Hub => "Optional. See this machine's board on your phone, end-to-end encrypted.",
            Step::Signals => "When kebbie itself crashes or hits some internal errors, it sends its maintainers an error signal, at most once a day. A signal holds only kebbie's version and commit, your OS and CPU architecture, the part of kebbie that failed, the kind of failure and a count; never your code, paths or text.",
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
    pub hub_url: Option<String>,
    pub signals: bool,
    pub signals_set: bool,
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
            hub_url: machine.hub_url.clone(),
            signals: machine.report.signals(),
            signals_set: machine.report.signals.is_some(),
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
    pub hub: usize,
    pub hub_url: String,
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
            hub: if facts.hub_url.as_deref().is_some_and(|url| url != config::DEFAULT_HUB_URL) { 1 } else { 0 },
            hub_url: facts.hub_url.clone().filter(|url| url != config::DEFAULT_HUB_URL).unwrap_or_default(),
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
    HubUrl,
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
    Hub(Option<String>),
    Signals(bool),
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
}

pub struct Guide {
    pub step: Step,
    cursor: usize,
    pub answers: Answers,
    facts: Facts,
    visited: [bool; 8],
    skipped: [bool; 8],
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
        let mut guide = Guide { step: Step::Name, cursor: 0, answers, facts, visited: [false; 8], skipped: [false; 8], fresh_name: true, error: None, applied: None, ending: Vec::new() };
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
            Step::Hub if self.facts.hub.is_some() => Vec::new(),
            Step::Hub if self.answers.hub == 1 => vec![Spot::Choice(0), Spot::Choice(1), Spot::Field(Field::HubUrl), Spot::Choice(2)],
            Step::Hub => (0..3).map(Spot::Choice).collect(),
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
            Step::Hub => self.answers.hub,
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
            Step::Hub => self.answers.hub = at,
            Step::Signals => self.answers.signals = at,
            _ => {}
        }
    }

    fn field(&mut self, field: Field) -> &mut String {
        match field {
            Field::Name => &mut self.answers.name,
            Field::Path => &mut self.answers.vault_path,
            Field::S3(at) => &mut self.answers.s3[at],
            Field::HubUrl => &mut self.answers.hub_url,
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
            Step::Hub => (self.answers.hub, self.answers.hub_url) = (2, today.hub_url),
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
            Step::Hub if self.answers.hub == 1 => crate::hub::hub_url(&self.answers.hub_url).map(|_| ()).map_err(|error| format!("The hub's address: {error}")),
            _ => Ok(()),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Next {
        if crate::keys::ctrl_c(&key) {
            return Next::Quit;
        }
        if self.applied.is_some() {
            return if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) { Next::Quit } else { Next::Stay };
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
            Step::Hub => facts.hub.as_ref().map(|host| format!("connected to {host}; kebbie hub leave, then this step, connects elsewhere")),
            Step::Signals if facts.signals_set => Some(format!("report.signals is {}", facts.signals)),
            _ => None,
        }
    }

    pub fn plan(&self) -> Vec<Planned> {
        let (facts, answers) = (&self.facts, &self.answers);
        let skipped = |step: Step| self.skipped[step.index()];
        let row = |step, verb, detail: String, act| Planned { step, verb, detail, act };
        let mut rows = vec![row("link", "to do", "~/.local/bin/kebbie → the installed binary".to_string(), Act::Link)];
        let codex = (answers.codex != facts.codex_plugin || answers.codex).then_some(answers.codex).filter(|_| facts.codex.is_some() && !skipped(Step::Agents));
        rows.push(row("plugin", "to do", format!("kebbie in Claude Code{}", match codex { Some(true) => ", and in Codex", Some(false) => ", and out of Codex", None => "" }), Act::Plugin(codex)));
        rows.push(match () {
            _ if skipped(Step::Name) => row("machine", "skipped", "machine.name stays as it is".to_string(), Act::Nothing),
            _ if facts.name_set && answers.name.trim() == facts.name => row("machine", "already", format!("machine.name = \"{}\"", facts.name), Act::Nothing),
            _ => row("machine", "to do", format!("machine.name = \"{}\"", answers.name.trim()), Act::Name(answers.name.trim().to_string())),
        });
        let choices = key_choices(&facts.keys);
        rows.push(match (choices.get(answers.keys), &facts.keys.blocked) {
            (_, Some(blocked)) => row("keys", "skipped", blocked.clone(), Act::Nothing),
            (None, _) => row("keys", "already", format!("{} binds kebbie's keys", home(&facts.keys.config)), Act::Nothing),
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
        let listed = format!("board.backlog.sources = [{}]", sources.iter().map(|source| format!("{:?}", source.key())).collect::<Vec<_>>().join(", "));
        rows.push(match () {
            _ if skipped(Step::Backlog) => row("backlog", "skipped", "board.backlog.sources stays as it is".to_string(), Act::Nothing),
            _ if sources == facts.sources => row("backlog", "already", listed, Act::Nothing),
            _ => row("backlog", "to do", listed, Act::Sources(sources)),
        });
        rows.push(self.vault_row());
        rows.push(match (&facts.hub, answers.hub) {
            (Some(host), _) => row("hub", "already", format!("connected to {host}"), Act::Nothing),
            _ if skipped(Step::Hub) || answers.hub == 2 => row("hub", "skipped", format!("optional; {}", setup::LATER), Act::Nothing),
            (None, 0) => row("hub", "to do", format!("connect to {} in your browser", crate::hub::host(config::DEFAULT_HUB_URL)), Act::Hub(None)),
            (None, _) => row("hub", "to do", format!("connect to {} in your browser", answers.hub_url.trim()), Act::Hub(Some(answers.hub_url.trim().to_string()))),
        });
        let on = answers.signals == 0;
        rows.push(match () {
            _ if skipped(Step::Signals) => row("signals", "skipped", format!("{} as they are", if facts.signals { "on" } else { "off" }), Act::Nothing),
            _ if on == facts.signals => row("signals", "already", if on { "on".to_string() } else { "report.signals = false".to_string() }, Act::Nothing),
            _ => row("signals", "to do", format!("report.signals = {on}"), Act::Signals(on)),
        });
        rows
    }

    fn vault_row(&self) -> Planned {
        let (facts, answers) = (&self.facts, &self.answers);
        let row = |verb, detail: String, act| Planned { step: "vault", verb, detail, act };
        if self.skipped[Step::Vault.index()] || answers.vault == 0 {
            return row("skipped", "not set up; kebbie works the same without it".to_string(), Act::Nothing);
        }
        let (values, shown) = if answers.vault == 1 {
            let path = expand(&answers.vault_path);
            (json!({ "vault_path": path }), format!("vault at {}", home(std::path::Path::new(&path))))
        } else {
            let values: serde_json::Map<String, Value> = S3_FIELDS.iter().zip(&answers.s3).map(|((_, name), value)| (name.to_string(), Value::from(value.trim()))).collect();
            (Value::Object(values), format!("vault in the S3 bucket {}", answers.s3[1].trim()))
        };
        let same = if answers.vault == 1 { expand(&answers.vault_path) == expand(&facts.vault_path) } else { answers.s3.iter().map(|value| value.trim()).eq(facts.s3.iter().map(|value| value.trim())) };
        if matches!(facts.vault, Plugin::Enabled { .. }) && same {
            return row("already", format!("the vault plugin is installed, {shown}"), Act::Nothing);
        }
        let mut steps = setup::steps(&facts.claude_bin, &facts.vault, facts.gronare);
        steps.pop();
        row("to do", format!("plugin installed, {shown}"), Act::Vault(steps, values, shown))
    }
}

pub trait Hooks {
    fn progress(&mut self, rows: &[(String, Outcome)]);
    fn connect(&mut self, url: &str) -> Result<String>;
}

fn write_key(env: &Env, key: &str, set: &Set) -> Outcome {
    match config_edit::write(&env.config, true, false, |text| config_edit::set(text, key, set)) {
        Ok(()) => Outcome::new("done", format!("{key} = {} in {}", set.shown(), home(&env.config))),
        Err(error) => Outcome::new("failed", error.lines().next().unwrap_or_default().to_string()),
    }
}

fn plugin(env: &Env, codex: Option<bool>) -> Outcome {
    let claude = setup::claude_plugin(env.machine);
    let Some(on) = codex else { return claude };
    let Some(codex_home) = env.codex_home.as_deref() else { return Outcome::new("failed", format!("{}; Codex: neither CODEX_HOME nor HOME is set", claude.detail)) };
    let codex = if on {
        crate::agent::codex::install(codex_home, &env.kebbie).map(|written| if written.is_empty() { "kebbie's plugin is current in Codex".to_string() } else { format!("installed kebbie's plugin in Codex at {}", home(&crate::agent::codex::plugin_root(codex_home))) })
    } else {
        crate::agent::codex::uninstall(codex_home).map(|_| "removed kebbie's plugin from Codex".to_string())
    };
    match codex {
        Ok(said) => Outcome::new(if claude.verb == "already" && said.ends_with("current in Codex") { "already" } else if claude.verb == "failed" { "failed" } else { "done" }, format!("{}; {said}", claude.detail)),
        Err(error) => Outcome::new("failed", format!("{}; Codex: {error}", claude.detail)),
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
        Ok(_) => Outcome::new("done", format!("the vault plugin is installed, {shown}")),
        Err(_) => Outcome::new("failed", format!("{claude} plugin configure {} did not take the vault's values; run kebbie setup vault again, or set them in Claude Code's /plugin screen", setup::VAULT_PLUGIN)),
    }
}

fn hub(env: &Env, own: &Option<String>, hooks: &mut dyn Hooks) -> Outcome {
    let (url, note) = match own {
        Some(address) => match crate::hub::connect::probe(address) {
            Ok(url) => {
                let chosen = crate::hub::connect::save_own(&env.config, url);
                (chosen.url, chosen.note)
            }
            Err(error) => return Outcome::new("failed", format!("{address} does not answer as a kebbie hub: {error}")),
        },
        None => {
            if crate::hub::connect::configured_hub(&env.config).ok().flatten().is_some_and(|url| url != config::DEFAULT_HUB_URL) {
                let _ = config_edit::write(&env.config, true, false, |text| config_edit::remove(text, "hub.url"));
            }
            (config::DEFAULT_HUB_URL.to_string(), None)
        }
    };
    let result = hooks.connect(&url);
    let host = crate::hub::identity::load(&env.state).ok().flatten().map(|identity| crate::hub::host(&identity.url).to_string());
    let mut outcome = setup::hub_connected(result, host);
    if let Some(note) = note.filter(|_| outcome.verb == "done") {
        outcome.detail = format!("{}; {note}", outcome.detail);
    }
    outcome
}

pub fn apply(planned: &[Planned], env: &Env, hooks: &mut dyn Hooks) -> Vec<(String, Outcome)> {
    let mut done: Vec<(String, Outcome)> = Vec::new();
    for row in planned {
        let outcome = match &row.act {
            Act::Nothing => Outcome::new(row.verb, row.detail.clone()),
            Act::Link => setup::link_binary(&env.machine.home, &env.machine.path),
            Act::Plugin(codex) => plugin(env, *codex),
            Act::Name(name) => write_key(env, "machine.name", &Set::Text(name.clone())),
            Act::Keys(plan, bindings) => {
                let (mut outcome, wrote) = setup::add_keys(plan, bindings);
                outcome.detail.push_str(&setup::herdr_reload(env.machine, wrote));
                outcome
            }
            Act::Sources(sources) => write_key(env, "board.backlog.sources", &Set::List(sources.iter().map(|source| source.key().to_string()).collect())),
            Act::Vault(steps, values, shown) => vault(env, steps, values, shown),
            Act::Hub(own) => hub(env, own, hooks),
            Act::Signals(on) => write_key(env, "report.signals", &Set::Bool(*on)),
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
    let hub = match (&facts.hub, asking) {
        (Some(host), _) => Outcome::new("already", format!("connected to {host}")),
        (None, Asking::Closed) => Outcome::new("skipped", format!("optional, and no terminal to ask in; {}", setup::LATER)),
        (None, Asking::Yes) => Outcome::new("skipped", format!("optional; {}", setup::LATER)),
    };
    [("link", setup::link_binary(&env.machine.home, &env.machine.path)), ("plugin", setup::claude_plugin(env.machine)), ("keys", keys), ("hub", hub)].into_iter().map(|(step, outcome)| (step.to_string(), outcome)).collect()
}

const LAST: &str = "Press prefix+space in herdr and tell the lead what you want done.";
const NEXT: &str = "Next steps";

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

// What is still the user's after setup, one line each with its command or key; the lead is always last.
pub fn ending(rows: &[(String, Outcome)], codex: Option<CodexLogin>, checks: &[crate::doctor::Check]) -> Vec<String> {
    let mut left: Vec<String> = Vec::new();
    let mut add = |line: String| if !left.contains(&line) { left.push(line) };
    match codex {
        Some(CodexLogin::Out) => add("codex login: Codex is not logged in, so a codex task cannot start".to_string()),
        Some(CodexLogin::Unknown) => add("codex login, if you have not yet".to_string()),
        _ => {}
    }
    let open = |step: &str| rows.iter().any(|(name, outcome)| name == step && matches!(outcome.verb, "skipped" | "failed"));
    if open("hub") {
        add("kebbie hub connect, or H on the board, to see this board on your phone".to_string());
    }
    if rows.iter().any(|(name, outcome)| name == "vault" && outcome.verb == "failed") {
        add("kebbie setup vault to try the vault again, or set its options in Claude Code's /plugin screen".to_string());
    }
    for check in checks.iter().filter(|check| check.level != crate::doctor::Level::Ok) {
        if check.fix.is_empty() {
            add(format!("{}: {}", check.name, check.fact));
        }
        for fix in &check.fix {
            add(format!("{fix}  · {}", check.name));
        }
    }
    let total = crate::doctor::summary(checks).lines().last().unwrap_or_default().to_string();
    let mut lines = vec![String::new(), format!("Setup done. {total}"), String::new()];
    if !left.is_empty() {
        lines.push(NEXT.to_string());
        lines.extend(left.into_iter().map(|line| format!("  {line}")));
        lines.push(format!("  {LAST}"));
    } else {
        lines.push(LAST.to_string());
    }
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
            Step::Hub if facts.hub.is_some() => {}
            Step::Hub => {
                for (at, (label, about)) in [("The hosted hub", format!("{} · free to read · opens your browser to sign in", crate::hub::host(config::DEFAULT_HUB_URL))), ("My own hub", "the address of a kebbie-hub you run".to_string()), ("Later", "H on the board sets it up any time".to_string())].into_iter().enumerate() {
                    lines.push(self.option(Spot::Choice(at), Self::radio(answers.hub == at), label.to_string(), about));
                    if at == 1 && answers.hub == 1 {
                        lines.push(self.field_line(Field::HubUrl, "address", &answers.hub_url, false));
                    }
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
                    lines.push(Line::from(vec![Span::styled(format!("{:<8}", outcome.verb), style(verb_color(outcome.verb))), Span::styled(format!("{step:<9}"), style(MAUVE)), Span::styled(outcome.detail, style(TEXT))]));
                }
                for said in &self.ending {
                    lines.push(match said.as_str() {
                        _ if said.starts_with("Setup done.") => Line::from(vec![Span::styled("Setup done.", style(GREEN)), Span::styled(said["Setup done.".len()..].to_string(), style(SUBTEXT))]),
                        NEXT => Line::from(Span::styled(NEXT, bold(MAUVE))),
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
            _ if self.applied.is_some() => "esc close".to_string(),
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
        let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(FAINT)).padding(Padding::horizontal(1)).title(Span::styled(format!(" {} ", self.step.title()), bold(MAUVE)));
        let inner = block.inner(boxed);
        let height = Paragraph::new(lines.clone()).wrap(Wrap { trim: false }).line_count(inner.width) as u16;
        let shown = Rect { height: (height + 2).min(boxed.height), ..boxed };
        frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), shown);
        frame.render_widget(Paragraph::new(Line::from(Span::styled(self.hint(), style(MUTED)))), Rect { y: hint.y + 1, height: 1, ..hint });
    }

    fn text(&self) -> String {
        match &self.applied {
            None => "Nothing written: kebbie setup runs the guide again whenever you want.".to_string(),
            Some(rows) => {
                let mut lines: Vec<String> = rows.iter().map(|(step, outcome)| outcome.line(step)).collect();
                lines.extend(self.ending.iter().cloned());
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

    // kebbie hub connect draws its own card and reads its own keys, so the guide steps out of the terminal while it runs.
    fn connect(&mut self, url: &str) -> Result<String> {
        crate::board::terminal::restore_terminal();
        let result = crate::hub::connect_to(url);
        *self.terminal = crate::board::terminal::init_terminal();
        result
    }
}

pub fn run(env: &Env, at: Step) -> Result<String> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};
    let mut guide = Guide::new(Facts::read(env), at);
    let mut terminal = crate::board::terminal::init_terminal();
    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|frame| guide.draw(frame)).map_err(|error| error.to_string())?;
            match event::read().map_err(|error| error.to_string())? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match guide.key(key) {
                    Next::Quit => return Ok(()),
                    Next::Stay => {}
                    Next::Apply => {
                        let planned = guide.plan();
                        guide.applied = Some(Vec::new());
                        let rows = apply(&planned, env, &mut Screen { terminal: &mut terminal, guide: &mut guide });
                        let codex = (guide.answers.codex && !guide.skipped[Step::Agents.index()]).then(|| codex_login(guide.facts.codex_bin.as_deref()));
                        guide.ending = ending(&rows, codex, &crate::doctor::checks(&crate::doctor::Env::current()));
                        guide.applied = Some(rows);
                    }
                },
                Event::Paste(text) => guide.paste(&text),
                _ => {}
            }
        }
    })();
    crate::board::terminal::restore_terminal();
    result.map(|()| guide.text())
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
        connects: usize,
        drawn: usize,
        hubs: Vec<String>,
    }

    impl Hooks for Fake {
        fn progress(&mut self, _: &[(String, Outcome)]) {
            self.drawn += 1;
        }

        fn connect(&mut self, url: &str) -> Result<String> {
            self.connects += 1;
            self.hubs.push(url.to_string());
            Ok(String::new())
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
    fn the_guide_walks_eight_steps_with_their_choices_and_writes_nothing_before_apply() {
        let lab = Lab::new("guide-walk");
        let mut guide = fresh(&lab);
        let shown = screen(&guide);
        assert!(shown.contains("kebbie setup  step 1 of 8") && shown.contains("▸ 1 Machine name") && shown.contains("· 8 Review and apply"), "{shown}");
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
        assert_eq!(guide.step, Step::Hub);
        assert!(screen(&guide).contains("(•) The hosted hub  · hub.kebbie.dev · free to read · opens your browser to sign in"));
        press(&mut guide, &[KeyCode::Up, KeyCode::Enter]);
        assert_eq!((guide.step, guide.answers.hub), (Step::Signals, 2), "up from the first option is Later");
        assert!(screen(&guide).contains("never your code, paths or text") && screen(&guide).contains("A bug report with text is always shown to you first"));
        press(&mut guide, &[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(guide.step, Step::Review);
        assert_eq!(plan(&guide), [
            "to do link ~/.local/bin/kebbie → the installed binary",
            "to do plugin kebbie in Claude Code, and in Codex",
            "to do machine machine.name = \"devbox\"",
            "to do keys prefix+space, prefix+f, prefix+t in herdr",
            "to do backlog board.backlog.sources = [\"vault\"]",
            "to do vault plugin installed, vault at /notes/vault",
            "skipped hub optional; later: H on the board",
            "to do signals report.signals = false",
        ]);
        assert!(screen(&guide).contains("⏎ apply · ← back · esc quit without writing"));
        assert_eq!((lab.config(), lab.herdr(), lab.calls()), (String::new(), String::new(), String::new()), "nothing is written or run before ⏎ on the last step");
        assert!(!lab.dir.join("codex/plugins").exists() && !lab.machine.home.join(".local/bin/kebbie").exists());
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Esc)), Next::Quit));
        assert_eq!(guide.text(), "Nothing written: kebbie setup runs the guide again whenever you want.");
    }

    #[test]
    fn apply_writes_and_runs_each_line_then_a_rerun_starts_from_what_is_in_place() {
        let lab = Lab::new("guide-apply");
        let mut guide = fresh(&lab);
        press(&mut guide, &[KeyCode::Enter, KeyCode::Down, KeyCode::Char(' '), KeyCode::Enter, KeyCode::Enter, KeyCode::Down, KeyCode::Char(' '), KeyCode::Enter, KeyCode::Down, KeyCode::Down]);
        type_text(&mut guide, "/notes/vault");
        press(&mut guide, &[KeyCode::Enter, KeyCode::Enter, KeyCode::Down, KeyCode::Enter]);
        assert_eq!(guide.step, Step::Review);
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Enter)), Next::Apply));
        let mut fake = Fake { connects: 0, drawn: 0, hubs: Vec::new() };
        let rows = apply(&guide.plan(), &lab.env(), &mut fake);
        let verbs: Vec<String> = rows.iter().map(|(step, outcome)| format!("{} {step}", outcome.verb)).collect();
        assert_eq!(verbs, ["done link", "done plugin", "done machine", "done keys", "done backlog", "done vault", "done hub", "done signals"], "{rows:?}");
        assert_eq!((fake.connects, fake.drawn), (1, 8), "the hosted hub connects through the browser, and each line is drawn as it finishes");
        assert_eq!(fake.hubs, [config::DEFAULT_HUB_URL], "the hub chosen in step 6 goes to the connect, so it shows no picker");
        assert_eq!(rows[6].1.detail, "connected");
        assert_eq!(lab.config(), "[machine]\nname = \"mac\"\n\n[board.backlog]\nsources = [\"local\", \"vault\"]\n\n[report]\nsignals = false\n");
        assert_eq!(lab.herdr().matches("[[keys.command]]").count(), 3);
        assert!(lab.dir.join("codex/plugins/cache/kebbie/kebbie/local/hooks/hooks.json").is_file(), "Codex gets kebbie's plugin");
        let calls = lab.calls();
        assert!(calls.contains("plugin marketplace add https://github.com/gronare/claude-plugins.git\nplugin install vault@gronare\nplugin configure vault@gronare --values-stdin\n"), "{calls}");
        assert!(std::fs::read_to_string(lab.dir.join("claude.stdin")).unwrap().contains("{\"vault_path\":\"/notes/vault\"}"), "the vault's values go in on stdin");
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
        assert_eq!(plan(&signals)[7], "to do signals report.signals = true");
        assert_eq!(rerun.answers.name, "mac");
        assert_eq!(rerun.answers.chosen_sources(), [Source::Local, Source::Vault]);
        assert_eq!((rerun.answers.signals, rerun.answers.codex, rerun.answers.vault, rerun.answers.vault_path.as_str()), (1, true, 1, "/notes/vault"));
        let shown = screen(&rerun);
        assert!(shown.contains("✓ in place: machine.name is \"mac\"") && shown.contains("✓ 3 herdr keys") && shown.contains("✓ 7 Error signals"), "the rail marks what is in place: {shown}");
        let mut rerun = rerun;
        press(&mut rerun, &[KeyCode::Enter, KeyCode::Enter, KeyCode::Enter, KeyCode::Enter, KeyCode::Enter, KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(rerun.step, Step::Review);
        let plan = plan(&rerun);
        assert!(plan[2].starts_with("already machine") && plan[3].starts_with("already keys") && plan[4].starts_with("already backlog") && plan[5].starts_with("already vault") && plan[7].starts_with("already signals"), "{plan:?}");
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
        press(&mut guide, &[KeyCode::Char('s'), KeyCode::Char('s')]);
        assert_eq!(guide.step, Step::Review);
        let plan = plan(&guide);
        assert!(plan[3].starts_with("skipped keys") && plan[5].starts_with("skipped vault") && plan[6].starts_with("skipped hub") && plan[7].starts_with("skipped signals"), "{plan:?}");
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
        press(&mut guide, &[KeyCode::Enter, KeyCode::Char('s'), KeyCode::Char('s'), KeyCode::Char('s'), KeyCode::Char('s')]);
        assert!(plan(&guide)[3].starts_with("to do keys prefix+space, prefix+f, prefix+y in herdr"), "{:?}", plan(&guide));
        let rows = apply(&guide.plan()[3..4], &lab.env(), &mut Fake { connects: 0, drawn: 0, hubs: Vec::new() });
        assert_eq!(rows[0].1.verb, "done");
        let herdr = lab.herdr();
        assert!(herdr.starts_with("[keys]\nnew_tab = \"prefix+t\"\n\n[[keys.command]]") && herdr.contains("key = \"prefix+y\"\ntype = \"plugin_action\"\ncommand = \"kebbie.board\""), "only the end of the file changes: {herdr}");
        let mut leave = fresh(&lab);
        std::fs::write(&lab.machine.herdr_config, "[keys]\nnew_tab = \"prefix+t\"\n").unwrap();
        leave.facts.keys = setup::key_plan(&lab.machine.herdr_config);
        press(&mut leave, &[KeyCode::Enter, KeyCode::Enter, KeyCode::Enter]);
        assert!(plan(&leave)[3].starts_with("to do keys prefix+space, prefix+f in herdr, the board unbound"), "{:?}", plan(&leave));
    }

    #[test]
    fn an_s3_vault_masks_its_secret_and_a_connected_hub_is_left_as_it_is() {
        let lab = Lab::new("guide-s3");
        let mut guide = fresh(&lab);
        guide.facts.hub = Some("hub.kebbie.dev".to_string());
        guide.go(Step::Vault);
        press(&mut guide, &[KeyCode::Up]);
        assert_eq!(guide.answers.vault, 2);
        for value in ["https://s3.example.com", "notes", "AKIA1", "hunter2"] {
            press(&mut guide, &[KeyCode::Down]);
            type_text(&mut guide, value);
        }
        let shown = screen(&guide);
        assert!(shown.contains("secret   •••••••") && !shown.contains("hunter2"), "{shown}");
        press(&mut guide, &[KeyCode::Enter]);
        assert_eq!(guide.step, Step::Hub);
        assert!(screen(&guide).contains("✓ in place: connected to hub.kebbie.dev"));
        press(&mut guide, &[KeyCode::Enter, KeyCode::Enter]);
        let plan = guide.plan();
        assert_eq!(format!("{} {}", plan[5].verb, plan[5].detail), "to do plugin installed, vault in the S3 bucket notes");
        assert_eq!(format!("{} {}", plan[6].verb, plan[6].detail), "already connected to hub.kebbie.dev");
        let rows = apply(&plan[5..7], &lab.env(), &mut Fake { connects: 0, drawn: 0, hubs: Vec::new() });
        assert!(rows.iter().all(|(_, outcome)| !outcome.detail.contains("hunter2")), "the secret is never shown: {rows:?}");
        assert!(std::fs::read_to_string(lab.dir.join("claude.stdin")).unwrap().contains("\"s3_secret_key\":\"hunter2\""));
    }

    #[test]
    fn yes_takes_the_defaults_and_without_a_terminal_nothing_is_written_to_herdr() {
        let lab = Lab::new("guide-yes");
        let lines: Vec<String> = without_asking(&lab.env(), Asking::Yes).iter().map(|(step, outcome)| outcome.line(step)).collect();
        let verbs: Vec<&str> = lines.iter().map(|line| line.split_whitespace().next().unwrap_or_default()).collect();
        assert_eq!(verbs, ["done", "done", "done", "skipped"], "{lines:?}");
        assert!(lines[2].contains("added prefix+space, prefix+f, prefix+t to") && lines[2].contains("restart herdr so it reads the new keys"), "{lines:?}");
        assert_eq!(lines[3], setup::line("skipped", "hub", "optional; later: H on the board"));
        assert_eq!(lab.config(), "", "--yes leaves the optional steps as they are");
        let written = lab.herdr();
        let again: Vec<String> = without_asking(&lab.env(), Asking::Yes).iter().map(|(step, outcome)| outcome.line(step)).collect();
        assert!(again[0].starts_with("already") && again[1].starts_with("already") && again[2].starts_with("already"), "{again:?}");
        assert_eq!(lab.herdr(), written);
        let closed = Lab::new("guide-closed");
        let lines: Vec<String> = without_asking(&closed.env(), Asking::Closed).iter().map(|(step, outcome)| outcome.line(step)).collect();
        assert!(lines[2].starts_with("skipped") && lines[2].contains("no terminal to ask in") && lines[2].contains("kebbie setup --yes"), "{lines:?}");
        assert!(lines[3].contains("no terminal to ask in"));
        assert_eq!(closed.herdr(), "");
    }

    fn check(level: crate::doctor::Level, name: &str, fact: &str, fix: &[&str]) -> crate::doctor::Check {
        crate::doctor::Check { level, name: name.to_string(), fact: fact.to_string(), fix: fix.iter().map(|fix| fix.to_string()).collect() }
    }

    #[test]
    fn the_end_lists_what_is_left_one_line_each_with_its_command_and_nothing_when_nothing_is() {
        use crate::doctor::Level;
        let done = |step: &str, verb: &'static str| (step.to_string(), Outcome::new(verb, "x"));
        let clean = ending(&[done("hub", "done"), done("vault", "done")], Some(CodexLogin::In), &[check(Level::Ok, "git", "/usr/bin/git", &[])]);
        assert_eq!(clean, ["", "Setup done. kebbie doctor: 0 failed, 0 warnings, 1 ok", "", "Press prefix+space in herdr and tell the lead what you want done."], "no next steps when nothing is left");
        let checks = [check(Level::Warn, "vault", "no vault is configured", &["kebbie setup vault walks through it"]), check(Level::Fail, "jq", "jq is missing", &["macOS: brew install jq", "Linux: sudo apt install jq"]), check(Level::Warn, "keeper", "the keeper is not running", &[])];
        let left = ending(&[done("hub", "skipped"), done("vault", "failed")], Some(CodexLogin::Out), &checks);
        assert_eq!(left[3..], [
            "Next steps",
            "  codex login: Codex is not logged in, so a codex task cannot start",
            "  kebbie hub connect, or H on the board, to see this board on your phone",
            "  kebbie setup vault to try the vault again, or set its options in Claude Code's /plugin screen",
            "  kebbie setup vault walks through it  · vault",
            "  macOS: brew install jq  · jq",
            "  Linux: sudo apt install jq  · jq",
            "  keeper: the keeper is not running",
            "  Press prefix+space in herdr and tell the lead what you want done.",
        ]);
        assert!(ending(&[done("hub", "failed")], Some(CodexLogin::Unknown), &[]).contains(&"  codex login, if you have not yet".to_string()));
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
        guide.applied = Some(vec![("link".to_string(), Outcome::new("done", "linked"))]);
        let shown = screen(&guide);
        assert!(shown.contains("done    link     linked") && shown.contains("to do   plugin") && shown.contains("applying…"), "{shown}");
        guide.ending = ending(&[("hub".to_string(), Outcome::new("skipped", "optional"))], None, &[]);
        let shown = screen(&guide);
        assert!(shown.contains("Setup done. kebbie doctor: 0 failed, 0 warnings, 0 ok") && shown.contains("Next steps") && shown.contains("Press prefix+space in herdr and tell the lead what you want done.") && shown.contains("esc close"), "{shown}");
        assert!(guide.text().ends_with("Next steps\n  kebbie hub connect, or H on the board, to see this board on your phone\n  Press prefix+space in herdr and tell the lead what you want done."), "{}", guide.text());
        assert!(matches!(guide.key(KeyEvent::from(KeyCode::Esc)), Next::Quit));
    }
}
