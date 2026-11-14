mod agents;
mod chooser;
mod draw;
mod profile_edit;
mod wizard;

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;

use crate::board::widgets::{GREEN, MUTED, PEACH, RED, Spot};
use crate::config::{self, Entry, Kind, Scope};
use crate::config_edit::{self, Set};
use crate::profile_wizard::{self, Wizard};
use crate::profiles::{self, Profile, Role};
use crate::sh::Result;

use agents::Agents;
use chooser::{Change, Chooser, Field, Then, Typed};

pub enum Effect {
    Handled,
    Left,
    Open(PathBuf),
    Show(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab {
    Machine,
    Repo,
}

// The page's sections, by the table a key sits in; a key's own line comes from kebbie config's entries.
const SECTIONS: [(&str, &[&str]); 8] = [
    ("Tasks", &["task"]),
    ("Board", &["board"]),
    ("Worktrees", &["worktree"]),
    ("Review & docs", &["review", "ci", "tests", "comments", "docs"]),
    (PROFILES, &[]),
    ("Updates", &["update", "restart", "keeper", "recover"]),
    ("This machine", &["machine", "lead", "hub", "notify", "vault"]),
    ("Reports", &["report", "issues"]),
];

const PROFILES: &str = "Profiles";

fn section_of(key: &str) -> usize {
    let table = key.split('.').next().unwrap_or_default();
    SECTIONS.iter().position(|(_, tables)| tables.contains(&table)).unwrap_or(SECTIONS.len() - 1)
}

enum Mode {
    Normal,
    Edit { typed: Typed, buffer: String },
    Filter { buffer: String },
    Team { key: &'static str, change: Change },
    Choose(Box<Chooser>),
    Menu { name: String, index: usize },
    Wizard(Box<Wizard>),
    Delete { name: String, nudged: bool },
}

pub struct Paths {
    pub machine: PathBuf,
    pub repos_dir: PathBuf,
    pub claude_home: PathBuf,
    pub kebbie: PathBuf,
    pub profiles: PathBuf,
    pub codex_home: PathBuf,
    pub path: Vec<PathBuf>,
    pub kebbie_command: String,
    pub sessions: Option<PathBuf>,
}

impl Paths {
    pub fn current() -> Self {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        Paths {
            machine: config::path(),
            repos_dir: config::repos_dir(),
            claude_home: crate::agent::claude::home().unwrap_or_else(|| home.join(".claude")),
            kebbie: crate::lead::install_root().unwrap_or_else(|_| home.join(".local/share/kebbie/current")),
            profiles: profiles::dir(),
            codex_home: crate::agent::codex::home().unwrap_or_else(|| home.join(".codex")),
            path: std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default(),
            kebbie_command: crate::peers::kebbie_command().unwrap_or_else(|_| "kebbie".to_string()),
            sessions: None,
        }
    }
}

pub struct Page {
    paths: Paths,
    repo: Option<PathBuf>,
    tab: Tab,
    section: usize,
    row: usize,
    filter: String,
    mode: Mode,
    said: Option<(String, Color)>,
    entries: Vec<Entry>,
    error: Option<String>,
    agents: Agents,
    hits: Vec<(Rect, Spot)>,
}

fn edit_text(value: &str) -> String {
    match format!("v = {value}").parse::<toml::Table>().ok().and_then(|mut table| table.remove("v")) {
        Some(toml::Value::String(text)) => text,
        Some(toml::Value::Array(items)) => items.iter().filter_map(toml::Value::as_str).collect::<Vec<_>>().join(", "),
        Some(toml::Value::Integer(number)) => number.to_string(),
        Some(toml::Value::Boolean(on)) => on.to_string(),
        _ => String::new(),
    }
}

fn typed_list(text: &str) -> Result<Vec<String>> {
    let text = text.trim();
    if text.starts_with('[') {
        let parsed: toml::Table = format!("v = {text}").parse().map_err(|_| "a list is \"a\", \"b\" or [\"a\", \"b\"]".to_string())?;
        return parsed["v"].as_array().map(|items| items.iter().filter_map(toml::Value::as_str).map(str::to_string).collect()).ok_or_else(|| "a list of strings".to_string());
    }
    Ok(text.split(',').map(str::trim).filter(|item| !item.is_empty()).map(str::to_string).collect())
}

fn default_skill(key: &str) -> &'static str {
    if key == "task.rules_skill" { config::DEFAULT_RULES_SKILL } else { config::DEFAULT_SETUP_SKILL }
}

fn home_relative(path: &Path) -> String {
    crate::area_page::home_relative(path)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))?.flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|error| format!("{}: {error}", target.display()))?;
        }
    }
    Ok(())
}

// kebbie's skill as a starting point of the user's own; one already there is kept, never overwritten.
pub fn copy_skill(kebbie: &Path, claude_home: &Path, skill: &str) -> Result<(PathBuf, bool)> {
    let to = claude_home.join("skills").join(skill);
    if to.exists() {
        return Ok((to, false));
    }
    copy_dir(&kebbie.join("skills").join(skill), &to)?;
    Ok((to, true))
}

impl Page {
    pub fn new(paths: Paths, repo: Option<PathBuf>) -> Self {
        Page { paths, repo, tab: Tab::Machine, section: 0, row: 0, filter: String::new(), mode: Mode::Normal, said: None, entries: Vec::new(), error: None, agents: Agents::default(), hits: Vec::new() }
    }

    pub fn is_normal(&self) -> bool {
        matches!(self.mode, Mode::Normal)
    }

    pub fn open(&mut self, repo: Option<PathBuf>) {
        if repo.is_some() && repo != self.repo {
            self.repo = repo;
            self.tab = Tab::Machine;
        }
        self.load();
    }

    fn repo_file(&self) -> Option<PathBuf> {
        self.repo.as_deref().map(|repo| config::repo_file_in(&self.paths.repos_dir, repo))
    }

    fn file(&self) -> PathBuf {
        match (self.tab, self.repo_file()) {
            (Tab::Repo, Some(file)) => file,
            _ => self.paths.machine.clone(),
        }
    }

    fn linked(&self) -> bool {
        self.tab == Tab::Repo && std::fs::symlink_metadata(self.file()).is_ok_and(|meta| meta.file_type().is_symlink())
    }

    fn repo_name(&self) -> String {
        self.repo_file().and_then(|file| file.file_stem().map(|stem| stem.to_string_lossy().to_string())).unwrap_or_default()
    }

    pub fn load(&mut self) {
        let repo = if self.tab == Tab::Repo { self.repo_file() } else { None };
        match config::resolve_at(self.paths.machine.clone(), repo) {
            Ok(loaded) => {
                let tab = self.tab;
                self.entries = config::entries(&loaded).into_iter().filter(|entry| match tab { Tab::Machine => entry.scope != Scope::Repo, Tab::Repo => entry.scope != Scope::Machine }).collect();
                self.error = None;
            }
            Err(error) => {
                self.entries = Vec::new();
                self.error = Some(error);
            }
        }
        self.agents = Agents::load(&self.paths, &self.entries);
        let sections = self.sections();
        self.section = self.section.min(sections.len().saturating_sub(1));
        let rows = sections.get(self.section).map_or(0, |(_, rows)| rows.len());
        self.row = self.row.min(rows.saturating_sub(1));
    }

    fn filter_text(&self) -> &str {
        match &self.mode {
            Mode::Filter { buffer } => buffer,
            _ => &self.filter,
        }
    }

    fn sections(&self) -> Vec<(String, Vec<usize>)> {
        let filter = self.filter_text().to_lowercase();
        if !filter.is_empty() {
            let found = (0..self.entries.len()).filter(|index| self.entries[*index].key.contains(&filter) || self.entries[*index].about.to_lowercase().contains(&filter)).collect();
            return vec![(format!("Search: {filter}"), found)];
        }
        SECTIONS.iter().enumerate().map(|(at, (name, _))| {
            let rows = match (*name, self.tab) {
                (PROFILES, Tab::Machine) if self.error.is_none() => (0..self.agents.profiles.len() + self.agents.broken.len()).collect(),
                (PROFILES, _) => Vec::new(),
                _ => (0..self.entries.len()).filter(|index| section_of(self.entries[*index].key) == at).collect::<Vec<_>>(),
            };
            (name.to_string(), rows)
        }).filter(|(_, rows)| !rows.is_empty()).collect()
    }

    fn on_profiles(&self) -> bool {
        self.filter_text().is_empty() && self.tab == Tab::Machine && self.sections().get(self.section).is_some_and(|(name, _)| name == PROFILES)
    }

    fn profile(&self) -> Option<Profile> {
        if !self.on_profiles() {
            return None;
        }
        self.agents.profiles.get(self.row).cloned()
    }

    fn current(&self) -> Option<Entry> {
        if self.on_profiles() {
            return None;
        }
        let sections = self.sections();
        sections.get(self.section).and_then(|(_, rows)| rows.get(self.row)).map(|index| self.entries[*index].clone())
    }

    fn say(&mut self, text: String, color: Color) {
        self.said = Some((text, color));
    }

    pub fn paste(&mut self, text: &str) -> bool {
        match &mut self.mode {
            Mode::Edit { buffer, .. } | Mode::Filter { buffer } => {
                buffer.push_str(text.trim_end_matches(['\n', '\r']));
                true
            }
            Mode::Wizard(wizard) => wizard.paste(text),
            _ => false,
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> Effect {
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => return self.handle_normal(key),
            Mode::Filter { mut buffer } => match key.code {
                KeyCode::Enter => {
                    self.filter = buffer;
                    (self.section, self.row) = (0, 0);
                }
                KeyCode::Esc => self.filter.clear(),
                KeyCode::Backspace => {
                    buffer.pop();
                    self.mode = Mode::Filter { buffer };
                    (self.section, self.row) = (0, 0);
                }
                KeyCode::Char(character) => {
                    buffer.push(character);
                    self.mode = Mode::Filter { buffer };
                    (self.section, self.row) = (0, 0);
                }
                _ => self.mode = Mode::Filter { buffer },
            },
            Mode::Edit { typed, mut buffer } => match key.code {
                KeyCode::Enter => self.save_typed(typed, &buffer),
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    buffer.pop();
                    self.mode = Mode::Edit { typed, buffer };
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => self.mode = Mode::Edit { typed, buffer: String::new() },
                KeyCode::Char(character) => {
                    buffer.push(character);
                    self.mode = Mode::Edit { typed, buffer };
                }
                _ => self.mode = Mode::Edit { typed, buffer },
            },
            Mode::Choose(mut chooser) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    chooser.index = chooser.index.saturating_sub(1);
                    self.mode = Mode::Choose(chooser);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    chooser.index = (chooser.index + 1).min(chooser.choices.len().saturating_sub(1));
                    self.mode = Mode::Choose(chooser);
                }
                KeyCode::Enter => {
                    if let Some(choice) = chooser.choices.into_iter().nth(chooser.index) {
                        return self.then(choice.then);
                    }
                }
                KeyCode::Char('o') if chooser.opens => match chooser.choices.get(chooser.index).map(|choice| &choice.then) {
                    Some(Then::Role(Role::File(file))) => return Effect::Open(self.paths.profiles.join(file)),
                    _ => {
                        self.say("o opens a role file; this one has none yet".to_string(), MUTED);
                        self.mode = Mode::Choose(chooser);
                    }
                },
                KeyCode::Esc | KeyCode::Char('q') => {}
                _ => self.mode = Mode::Choose(chooser),
            },
            Mode::Menu { name, index } => {
                let keys: Vec<char> = self.menu_choices(&name).iter().map(|choice| choice.key).collect();
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::Menu { name, index: index.saturating_sub(1) },
                    KeyCode::Down | KeyCode::Char('j') => self.mode = Mode::Menu { index: (index + 1).min(keys.len().saturating_sub(1)), name },
                    KeyCode::Enter => return keys.get(index).map_or(Effect::Handled, |key| self.menu(*key)),
                    KeyCode::Char(key) if keys.contains(&key) => return self.menu(key),
                    KeyCode::Esc | KeyCode::Char('q') => {}
                    _ => self.mode = Mode::Menu { name, index },
                }
            }
            Mode::Wizard(mut wizard) => {
                let step = wizard.step;
                match wizard.key(key) {
                    profile_wizard::Step::Cancel => {}
                    profile_wizard::Step::Write { open } => return self.add(wizard, open),
                    profile_wizard::Step::Stay => {
                        if wizard.step > step {
                            self.offer(&mut wizard);
                        }
                        self.mode = Mode::Wizard(wizard);
                    }
                }
            }
            Mode::Delete { name, .. } => match key.code {
                KeyCode::Char('y') => self.delete(&name),
                KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => {}
                _ => self.mode = Mode::Delete { name, nudged: true },
            },
            Mode::Team { key: name, change } => match key.code {
                KeyCode::Char('t') => _ = self.apply(name, change, false),
                KeyCode::Char('o') => _ = self.apply(name, change, true),
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => {}
                _ => self.mode = Mode::Team { key: name, change },
            },
        }
        Effect::Handled
    }

    fn then(&mut self, then: Then) -> Effect {
        match (then, self.current()) {
            (Then::Field(field, value), _) => self.set_field(field, value.as_deref()),
            (Then::TypeModel, _) => self.mode = Mode::Edit { typed: Typed::Model, buffer: self.profile().and_then(|profile| self.agents.own_model(&profile)).unwrap_or_default() },
            (Then::Role(to), _) => return self.set_role(to),
            (Then::Write(change), Some(entry)) => self.request(entry.key, change),
            (Then::Type(typed), Some(entry)) => {
                let now = edit_text(&entry.value);
                let keep = match typed { Typed::SkillFile => config::skill_path(&now).is_some(), _ => now != default_skill(entry.key) && config::skill_path(&now).is_none() };
                self.mode = Mode::Edit { typed, buffer: if keep { now } else { String::new() } };
            }
            _ => {}
        }
        Effect::Handled
    }

    pub fn pointer(&mut self, event: MouseEvent) -> Effect {
        match event.kind {
            MouseEventKind::ScrollUp => self.handle(KeyCode::Up.into()),
            MouseEventKind::ScrollDown => self.handle(KeyCode::Down.into()),
            _ => self.mouse(event),
        }
    }

    fn mouse(&mut self, event: MouseEvent) -> Effect {
        if event.kind != MouseEventKind::Down(MouseButton::Left) {
            return Effect::Handled;
        }
        let hit = self.hits.iter().find(|(area, _)| area.contains(Position::new(event.column, event.row))).map(|(_, spot)| *spot);
        if let (Some(Spot::Choice(at)), Mode::Menu { index, .. }) = (hit, &mut self.mode) {
            *index = at;
            return self.handle(KeyCode::Enter.into());
        }
        if let (Some(Spot::Choice(at)), Mode::Wizard(wizard)) = (hit, &mut self.mode) {
            return if wizard.choose(at) { self.handle(KeyCode::Enter.into()) } else { Effect::Handled };
        }
        let closable = matches!(self.mode, Mode::Choose(_) | Mode::Menu { .. });
        let selected = match &mut self.mode {
            Mode::Choose(chooser) => Some(&mut chooser.index),
            _ => None,
        };
        match (hit, selected) {
            (Some(Spot::Choice(at)), Some(index)) if *index != at => *index = at,
            (Some(Spot::Choice(_)), _) => return self.handle(KeyCode::Enter.into()),
            (Some(Spot::Key { key, .. }), _) => return self.handle(key),
            (None, _) if closable => self.mode = Mode::Normal,
            _ => {}
        }
        Effect::Handled
    }

    fn handle_normal(&mut self, key: KeyEvent) -> Effect {
        if self.on_profiles() {
            match key.code {
                KeyCode::Char('m') => return self.choose(Field::Model),
                KeyCode::Char('e') => return self.choose(Field::Effort),
                KeyCode::Char('r') => return self.role(),
                KeyCode::Char('n') => {
                    let name = self.profile().map(|profile| profile.name).unwrap_or_default();
                    self.wizard(&name);
                    return Effect::Handled;
                }
                KeyCode::Char('x') => {
                    self.ask_delete();
                    return Effect::Handled;
                }
                KeyCode::Char('d') => {
                    let reviewed = self.profile().and_then(|profile| self.agents.review_model(&profile).map(str::to_string)).is_some();
                    let removed = reviewed.then(|| {
                        let done = self.apply("review.model", Change::Remove, false).is_ok();
                        (self.said.take().map(|(text, _)| text).unwrap_or_default(), done)
                    });
                    _ = self.write_profile(&[("model", None), ("effort", None)]);
                    if let Some((removed, done)) = removed && let Some((text, color)) = self.said.take() {
                        self.say(format!("{removed} · {text}"), if done { color } else { RED });
                    }
                    return Effect::Handled;
                }
                KeyCode::Enter => return self.open_profile(),
                KeyCode::Char(' ') => return Effect::Handled,
                _ => {}
            }
        }
        let sections = self.sections();
        let rows = sections.get(self.section).map_or(0, |(_, rows)| rows.len());
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.row = if self.row == 0 { rows.saturating_sub(1) } else { self.row - 1 };
                self.said = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.row = if self.row + 1 >= rows { 0 } else { self.row + 1 };
                self.said = None;
            }
            KeyCode::Left | KeyCode::Right if !sections.is_empty() => {
                let count = sections.len();
                self.section = if key.code == KeyCode::Left { (self.section + count - 1) % count } else { (self.section + 1) % count };
                self.row = 0;
                self.said = None;
            }
            KeyCode::Tab | KeyCode::BackTab => self.switch_tab(),
            KeyCode::Char('/') => self.mode = Mode::Filter { buffer: self.filter.clone() },
            KeyCode::Char(' ') => match self.current() {
                Some(entry) if entry.kind == Kind::Bool => self.request(entry.key, Change::Set(Set::Bool(entry.value != "true"))),
                Some(entry) => self.say(format!("space toggles a true or false key; ⏎ changes {}", entry.key), MUTED),
                None => {}
            },
            KeyCode::Enter => self.enter(),
            KeyCode::Char('d') => {
                if let Some(entry) = self.current() {
                    self.request(entry.key, Change::Remove);
                }
            }
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                (self.section, self.row) = (0, 0);
            }
            KeyCode::Esc | KeyCode::Char('q') => return Effect::Left,
            _ => {}
        }
        Effect::Handled
    }

    fn switch_tab(&mut self) {
        if self.repo.is_none() {
            return self.say("No repo here: select a task on the board, or run kebbie config edit --repo <path>".to_string(), PEACH);
        }
        self.tab = if self.tab == Tab::Machine { Tab::Repo } else { Tab::Machine };
        (self.section, self.row, self.said) = (0, 0, None);
        self.load();
    }

    fn enter(&mut self) {
        let Some(entry) = self.current() else { return };
        match entry.kind {
            Kind::Bool => self.request(entry.key, Change::Set(Set::Bool(entry.value != "true"))),
            Kind::Choice(_) | Kind::Skill | Kind::Profile | Kind::LeadProfile | Kind::ReviewProfile | Kind::SecondOpinion => self.mode = Mode::Choose(Box::new(self.chooser(&entry))),
            Kind::Secret => self.say(format!("{} is never shown here; set it in {} by hand", entry.key, home_relative(&self.paths.machine)), MUTED),
            Kind::Text | Kind::Number | Kind::List => self.mode = Mode::Edit { typed: Typed::Value, buffer: edit_text(&entry.value) },
        }
    }

    fn save_typed(&mut self, typed: Typed, buffer: &str) {
        let text = buffer.trim().to_string();
        if typed == Typed::Model {
            return self.set_field(Field::Model, Some(&text));
        }
        let Some(entry) = self.current() else { return };
        let set = match (typed, &entry.kind) {
            (Typed::Value, Kind::Number) => match text.parse::<i64>() {
                Ok(number) => Set::Number(number),
                Err(_) => return self.say(format!("✗ {}: \"{text}\" is not a whole number", entry.key), RED),
            },
            (Typed::Value, Kind::List) => match typed_list(&text) {
                Ok(items) => Set::List(items),
                Err(error) => return self.say(format!("✗ {}: {error}", entry.key), RED),
            },
            (Typed::SkillFile, _) if config::skill_path(&text).is_none() => return self.say(format!("✗ {}: a file goes by its absolute or ~ path, such as ~/code/team/RULES.md", entry.key), RED),
            _ => Set::Text(text),
        };
        self.request(entry.key, Change::Set(set));
    }

    fn request(&mut self, key: &'static str, change: Change) {
        if self.linked() {
            self.mode = Mode::Team { key, change };
            return;
        }
        _ = self.apply(key, change, false);
    }

    fn apply(&mut self, key: &'static str, change: Change, replace_link: bool) -> Result<()> {
        let file = self.file();
        let machine = self.tab == Tab::Machine || self.repo.is_none();
        let name = file.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let shown = if machine { name.clone() } else { format!("repos/{name}") };
        let (change, copied) = match change {
            Change::Copy(skill) => match copy_skill(&self.paths.kebbie, &self.paths.claude_home, skill) {
                Ok((to, true)) => (Change::Set(Set::Text(skill.to_string())), format!(" · copied kebbie's skill to {}: edit {}", home_relative(&to), home_relative(&to.join("SKILL.md")))),
                Ok((to, false)) => (Change::Set(Set::Text(skill.to_string())), format!(" · {} was already there, so nothing was copied: edit its SKILL.md", home_relative(&to))),
                Err(error) => return self.fail(format!("✗ copying kebbie's {skill}: {error}")),
            },
            other => (other, String::new()),
        };
        let written = match &change {
            Change::Set(set) => config_edit::write(&file, machine, replace_link, |text| config_edit::set(text, key, set)).map(|()| format!("✓ wrote {key} = {} to {shown}", set.shown())),
            _ => config_edit::write(&file, machine, replace_link, |text| config_edit::remove(text, key)).map(|()| format!("✓ removed {key} from {shown}; the layer below applies")),
        };
        let done = match written {
            Ok(said) => {
                let check = if machine { crate::doctor::config_file(&file).0 } else { crate::doctor::repo_file(&file).0 };
                let level = match check.level { crate::doctor::Level::Ok => "ok", crate::doctor::Level::Skip => "skip", crate::doctor::Level::Warn => "warn", crate::doctor::Level::Fail => "fail" };
                let fact = check.fact.replace(&file.display().to_string(), &shown);
                let color = if check.level == crate::doctor::Level::Ok { GREEN } else { PEACH };
                let linked = if replace_link { " (now a file of this machine's own)" } else { "" };
                self.say(format!("{said}{linked}{copied} · doctor: {level}, {fact}"), color);
                Ok(())
            }
            Err(error) => self.fail(format!("✗ {}", error.lines().next().unwrap_or_default())),
        };
        self.load();
        done
    }

    fn fail(&mut self, text: String) -> Result<()> {
        self.say(text.clone(), RED);
        Err(text)
    }
}

pub fn open_in_editor(file: &Path) -> Result<bool> {
    std::process::Command::new("sh").args(["-c", "${VISUAL:-${EDITOR:-vi}} \"$1\"", "sh"]).arg(file).status().map(|status| status.success()).map_err(|error| error.to_string())
}

// kebbie's own files are shown, never edited: an update would replace the change.
pub fn show_in_pager(file: &Path) -> Result<bool> {
    std::process::Command::new("sh").args(["-c", "${PAGER:-less} \"$1\"", "sh"]).arg(file).status().map(|status| status.success()).map_err(|error| error.to_string())
}

pub fn edit(cwd: &Path, repo: Option<&str>) -> Result<String> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("kebbie config edit needs a terminal; kebbie config prints every key with its value".to_string());
    }
    crate::permit::user_only("kebbie config edit")?;
    let repo = match repo {
        Some(path) => {
            let path = PathBuf::from(path);
            crate::repo::main_checkout(&path).map_err(|_| format!("--repo {}: not a git repository", path.display()))?;
            Some(path)
        }
        None => crate::repo::main_checkout(cwd).ok().map(|_| cwd.to_path_buf()),
    };
    let mut page = Page::new(Paths::current(), repo);
    page.load();
    let mut terminal = crate::board::terminal::init_terminal();
    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|frame| page.draw_alone(frame)).map_err(|error| error.to_string())?;
            let effect = match ratatui::crossterm::event::read().map_err(|error| error.to_string())? {
                ratatui::crossterm::event::Event::Key(key) if key.kind == ratatui::crossterm::event::KeyEventKind::Press => {
                    if crate::keys::ctrl_c(&key) {
                        return Ok(());
                    }
                    page.handle(key)
                }
                ratatui::crossterm::event::Event::Mouse(event) => page.pointer(event),
                ratatui::crossterm::event::Event::Paste(text) => {
                    page.paste(&text);
                    Effect::Handled
                }
                _ => Effect::Handled,
            };
            match effect {
                Effect::Left => return Ok(()),
                Effect::Open(file) => {
                    let status = crate::board::terminal::outside(&mut terminal, || open_in_editor(&file));
                    page.opened(&file, status);
                }
                Effect::Show(file) => {
                    let status = crate::board::terminal::outside(&mut terminal, || show_in_pager(&file));
                    page.shown(status);
                }
                Effect::Handled => {}
            }
        }
    })();
    crate::board::terminal::restore_terminal();
    result.map(|()| page.said.map(|(text, _)| text).unwrap_or_default())
}

#[cfg(test)]
mod tests;
