mod agents;
mod chooser;

use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::agent::Agent;
use crate::board::widgets::{self, Answer, BLUE, FAINT, GREEN, MANTLE, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, Spot, TEAL, TEXT, YELLOW, bold, cut, fit, group_line, modal, modal_spots, options_lines, style, text_width, wrap};
use crate::config::{self, Entry, Kind, Layer, Scope};
use crate::config_edit::{self, Set};
use crate::profile_wizard::{self, Pick, Wizard};
use crate::profiles::{self, Edited, Profile, Role};
use crate::sh::Result;

use agents::Agents;
use chooser::{Change, Choice, Chooser, Field, Then, Typed, choice};

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
    ("Review & docs", &["review", "ci", "comments", "docs"]),
    (PROFILES, &[]),
    ("Updates", &["update", "restart", "keeper", "recover"]),
    ("This machine", &["machine", "hub", "notify", "vault"]),
    ("Reports", &["report", "issues"]),
];

const PROFILES: &str = "Profiles";

fn section_of(key: &str) -> usize {
    let table = key.split('.').next().unwrap_or_default();
    SECTIONS.iter().position(|(_, tables)| tables.contains(&table)).unwrap_or(SECTIONS.len() - 1)
}

const POPUP: usize = 64;

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
            Kind::Choice(_) | Kind::Skill | Kind::Profile | Kind::ReviewProfile | Kind::SecondOpinion => self.mode = Mode::Choose(Box::new(self.chooser(&entry))),
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

    fn shown_file(&self, file: &Path) -> String {
        let name = file.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        format!("profiles/{name}")
    }

    fn built_in_without_file(&self, profile: &Profile) -> bool {
        profiles::built_in(&profile.name).is_some() && profile.file.is_none()
    }

    fn set_field(&mut self, field: Field, value: Option<&str>) {
        if field == Field::Model && self.profile().is_some_and(|profile| self.agents.default_reviewer(&profile)) {
            return self.request("review.model", value.map_or(Change::Remove, |model| Change::Set(Set::Text(model.to_string()))));
        }
        _ = self.write_profile(&[(field.key(), value)]);
    }

    fn foot(&self, profile: &Profile, field: Field) -> Vec<String> {
        let key = field.key();
        if field == Field::Model && self.agents.default_reviewer(profile) {
            let file = home_relative(&self.file());
            return vec![format!("This writes review.model in {file}, ahead of {}'s own model, as {} is review.profile.", profile.name, profile.name), "A review already running keeps its model; the next review takes it.".to_string()];
        }
        let running = format!("A task already running keeps its {key}; the next start takes it.");
        if self.built_in_without_file(profile) {
            return vec![format!("{name} is kebbie's: a {key} you set goes in profiles/{name}.toml, which an update never writes, and kebbie's role and rules stay as they ship. d on the row removes that file again.", name = profile.name), running];
        }
        vec![running]
    }

    fn choose(&mut self, field: Field) -> Effect {
        let Some(profile) = self.profile() else { return Effect::Handled };
        let reviewer = self.agents.default_reviewer(&profile);
        let (question, (explain, choices)) = match field {
            Field::Model => {
                let current = if reviewer { self.agents.review_model(&profile).map(str::to_string) } else { profile.model.clone() };
                ("Which model?", self.model_choices(profile.agent, current.as_deref(), reviewer.then_some(&profile)))
            }
            Field::Effort => ("How much effort?", self.effort_choices(profile.agent, self.agents.model(&profile).as_deref(), profile.effort.as_deref())),
        };
        let foot = self.foot(&profile, field);
        self.said = None;
        self.mode = Mode::Choose(Box::new(Chooser::new(format!("{} · {}", field.key(), profile.name), question, explain, choices, foot)));
        Effect::Handled
    }

    fn broken(&self) -> Option<&(String, PathBuf, String)> {
        if !self.on_profiles() {
            return None;
        }
        self.agents.broken.get(self.row.checked_sub(self.agents.profiles.len())?)
    }

    fn open_profile(&mut self) -> Effect {
        if let Some((_, file, _)) = self.broken() {
            return Effect::Open(file.clone());
        }
        let Some(profile) = self.profile() else { return Effect::Handled };
        match profile.file {
            Some(file) => Effect::Open(file),
            None => {
                self.say(format!("{} is kebbie's and has no file of yours until you set something: m, e or r writes profiles/{}.toml", profile.name, profile.name), MUTED);
                Effect::Handled
            }
        }
    }

    pub fn opened(&mut self, file: &Path, result: Result<bool>) {
        self.load();
        let shown = self.shown_file(file);
        match result {
            Ok(_) if file.extension().is_some_and(|extension| extension == "md") => {
                let users: Vec<String> = self.agents.profiles.iter().filter(|profile| profile.role.as_deref() == Some(file)).map(|profile| profile.name.clone()).collect();
                let read = if users.is_empty() { "no profile reads it".to_string() } else { format!("read after kebbie's role by {}", users.join(", ")) };
                self.say(format!("{shown}: {read}"), GREEN);
            }
            Ok(_) => match self.agents.broken.iter().find(|(_, broken, _)| broken == file).map(|(_, _, error)| error) {
                Some(error) => self.say(format!("✗ {}", error.replace(&file.display().to_string(), &shown)), RED),
                None => self.say(format!("{shown} loads"), GREEN),
            },
            Err(error) => self.say(format!("✗ the editor: {error}"), RED),
        }
    }

    pub fn shown(&mut self, result: Result<bool>) {
        if let Err(error) = result {
            self.say(format!("✗ the pager: {error}"), RED);
        }
    }

    fn role(&mut self) -> Effect {
        let Some(profile) = self.profile() else { return Effect::Handled };
        self.said = None;
        if profiles::built_in(&profile.name).is_some() {
            self.mode = Mode::Menu { name: profile.name, index: 0 };
            return Effect::Handled;
        }
        let dir = &self.paths.profiles;
        let current = profiles::role_name(dir, &profile);
        let new = profiles::free_role(dir, &profile.name);
        let (mine, others): (Vec<_>, Vec<_>) = profiles::role_choices(dir, &self.agents.profiles, &profile.name).into_iter().partition(|(file, _)| Some(file) == current.as_ref());
        let mut choices: Vec<Choice> = mine.into_iter().chain(others).map(|(file, about)| choice(&file, &about, Then::Role(Role::File(file.clone())), Some(&file) == current.as_ref())).collect();
        choices.push(choice("New role", &format!("{new}, an empty file"), Then::Role(Role::New(new.clone())), false));
        if let Some(current) = &current {
            choices.push(choice(&format!("Copy of {current}"), "to change it for this profile alone", Then::Role(Role::Copy { from: current.clone(), to: new }), false));
        }
        choices.push(choice("None", "kebbie's role alone", Then::Role(Role::None), current.is_none()));
        let explain = format!("Your lines, read after {}.", profiles::kebbie_role_for(&profile).0);
        let mut chooser = Chooser::new(format!("role · {}", profile.name), "Which role?", explain, choices, vec!["A shared file changes for every profile that uses it.".to_string()]);
        chooser.opens = true;
        self.mode = Mode::Choose(Box::new(chooser));
        Effect::Handled
    }

    // A role file made here takes a name no file has, so it never replaces one the user has.
    fn set_role(&mut self, to: Role) -> Effect {
        let dir = self.paths.profiles.clone();
        let made = match &to {
            Role::New(file) => Some((file.clone(), Ok(String::new()))),
            Role::Copy { from, to } => Some((to.clone(), std::fs::read_to_string(dir.join(from)).map_err(|error| format!("✗ {from}: {error}")))),
            _ => None,
        };
        if let Some((file, text)) = &made {
            let written = text.clone().and_then(|text| if dir.join(file).exists() { Err(format!("✗ {file} is in the folder already")) } else { crate::state::write(&dir.join(file), text).map_err(|error| format!("✗ {file}: {error}")) });
            if let Err(error) = written {
                _ = self.fail(error);
                return Effect::Handled;
            }
        }
        let written = self.write_profile(&[("role", to.file())]);
        match made {
            Some((file, _)) if written.is_err() => {
                let _ = std::fs::remove_file(dir.join(file));
                Effect::Handled
            }
            Some((file, _)) => Effect::Open(dir.join(file)),
            None => Effect::Handled,
        }
    }

    fn menu_choices(&self, name: &str) -> Vec<widgets::Choice> {
        let row = |key: char, label: String, about: String| widgets::Choice { key, act: None, label, about, link: None };
        let review = self.agents.profiles.iter().find(|profile| profile.name == name).is_some_and(|profile| profile.review);
        let file = profiles::kebbie_role(name).1;
        let every = if review { "every review" } else { "every task" };
        let own = self.agents.profiles.iter().find(|profile| profile.name == name).and_then(|profile| profiles::role_name(&self.paths.profiles, profile)).unwrap_or_else(|| format!("{name}.md"));
        let others: Vec<String> = profiles::role_readers(&self.paths.profiles, &own).into_iter().filter(|reader| reader != name).collect();
        let shared = if others.is_empty() { String::new() } else { format!(", shared with {}", others.join(", ")) };
        let mut choices = vec![
            row('s', "Show kebbie's role".to_string(), format!("{file}, read-only")),
            row('a', "Add your lines".to_string(), format!("profiles/{own}{shared}, read after kebbie's role by {every}")),
        ];
        choices.push(row('n', format!("New profile from {name}"), format!("the new-profile steps, starting from {name}")));
        choices
    }

    fn wizard(&mut self, from: &str) {
        let starts: Vec<Profile> = self.agents.profiles.iter().filter(|profile| self.agents.set_up(profile)).cloned().collect();
        let start = starts.iter().position(|profile| profile.name == from).or_else(|| starts.iter().position(|profile| profile.name == profiles::TASK)).unwrap_or(0);
        let dir = &self.paths.profiles;
        let mut taken: Vec<String> = self.agents.profiles.iter().map(|profile| profile.name.clone()).collect();
        taken.extend(std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|entry| entry.file_name().to_str()?.strip_suffix(".toml").map(str::to_string)));
        let agents = self.agents.chosen.iter().map(|agent| (*agent, match agent {
            Agent::Claude => "kebbie's own default".to_string(),
            Agent::Codex => crate::agent::codex::availability(&self.paths.path, Some(&self.paths.codex_home), &self.paths.kebbie_command).short_line(),
        })).collect();
        let files = profiles::role_choices(dir, &self.agents.profiles, "");
        self.said = None;
        self.mode = Mode::Wizard(Box::new(Wizard::new(starts, start, files, dir.clone(), home_relative(dir), taken, agents)));
    }

    fn answers(choices: Vec<Choice>) -> (Vec<Answer<Pick>>, usize) {
        let selected = choices.iter().position(|choice| choice.now).unwrap_or(0);
        let answers = choices.into_iter().map(|choice| Answer::new(&choice.label, &choice.about, match choice.then { Then::TypeModel => Pick::Type, Then::Field(_, value) => Pick::Value(value), _ => Pick::Value(None) })).collect();
        (answers, selected)
    }

    fn offer(&self, wizard: &mut Wizard) {
        let start = wizard.from().filter(|profile| profile.agent == wizard.agent()).cloned();
        match wizard.step {
            profile_wizard::MODEL => {
                let (explain, choices) = self.model_choices(wizard.agent(), start.as_ref().and_then(|profile| profile.model.as_deref()), None);
                let (answers, selected) = Self::answers(choices);
                wizard.offer_models(explain, answers, selected);
            }
            profile_wizard::EFFORT => {
                let model = wizard.model().or_else(|| self.agents.default_model(wizard.agent()).map(str::to_string));
                let (explain, choices) = self.effort_choices(wizard.agent(), model.as_deref(), start.as_ref().and_then(|profile| profile.effort.as_deref()));
                let (answers, selected) = Self::answers(choices);
                wizard.offer_efforts(explain, answers, selected);
            }
            _ => {}
        }
    }

    // The role file goes first, as the profile parses only with its role there; a role file this call wrote goes again when the profile is not written.
    fn add(&mut self, mut wizard: Box<Wizard>, open: bool) -> Effect {
        let name = wizard.name.trim().to_string();
        let dir = self.paths.profiles.clone();
        let file = dir.join(format!("{name}.toml"));
        let Some(made) = wizard.made() else { return Effect::Handled };
        let role = wizard.chosen_role();
        let made_role = match &role {
            Role::New(to) => Ok(Some((to.clone(), String::new()))),
            Role::Copy { from, to } => std::fs::read_to_string(dir.join(from)).map(|text| Some((to.clone(), text))).map_err(|error| format!("{from}: {error}")),
            _ => Ok(None),
        };
        let mut wrote = None;
        let written = made_role.and_then(|made_role| {
            if file.exists() {
                return Err(format!("{name}.toml is in the folder already"));
            }
            if let Some((to, text)) = made_role {
                if dir.join(&to).exists() {
                    return Err(format!("{to} is in the folder already"));
                }
                crate::state::write(&dir.join(&to), text)?;
                wrote = Some(to);
            }
            profiles::parse(&made.text, &name, &file, None)?;
            crate::state::write(&file, &made.text)
        });
        if let Err(error) = written {
            if let Some(to) = wrote {
                let _ = std::fs::remove_file(dir.join(to));
            }
            wizard.error = Some(error.replace(&dir.display().to_string(), "profiles"));
            self.mode = Mode::Wizard(wizard);
            return Effect::Handled;
        }
        self.load();
        if let Some(row) = self.agents.profiles.iter().position(|profile| profile.name == name) {
            self.row = row;
        }
        let role_said = role.file().map_or_else(String::new, |file| format!(", its role {file}"));
        let used = if wizard.review { "review.profile or review.second on this page" } else { "kebbie start --profile {name}, or task.profile" }.replace("{name}", &name);
        self.say(format!("✓ added profiles/{name}.toml{role_said} · {used}"), GREEN);
        match (open, role.file()) {
            (false, _) => Effect::Handled,
            (true, Some(role)) => Effect::Open(dir.join(role)),
            (true, None) => Effect::Open(file),
        }
    }

    fn sessions(&self) -> Option<PathBuf> {
        self.paths.sessions.clone().or_else(|| crate::lead::state_location().ok().map(|state| state.join("sessions")))
    }

    fn kept(&self, profile: &Profile) -> Option<String> {
        if profiles::built_in(&profile.name).is_some() {
            return Some(format!("✗ {} is kebbie's and stays; d removes your settings on it", profile.name));
        }
        let open = self.sessions().map(|sessions| crate::tasks::on_profile(&sessions, &profile.name)).unwrap_or_default();
        if !open.is_empty() {
            return Some(format!("✗ {} is the profile of {}, still open; it can go once {} closes", profile.name, open.join(", "), if open.len() == 1 { "that task" } else { "they" }));
        }
        let machine = config::check_machine(&self.paths.machine).ok().map(|(machine, _)| machine);
        if let Some((file, _)) = crate::doctor::named_profiles(machine.as_ref(), &self.paths.machine, &self.paths.repos_dir).into_iter().find(|(_, named)| *named == profile.name) {
            return Some(format!("✗ task.profile names {} in {file}, so every new task there starts with it; pick another first", profile.name));
        }
        let routines: Vec<String> = crate::routines::load_from(&self.paths.machine, &self.paths.repos_dir).0.into_iter().filter(|routine| routine.profile.as_deref() == Some(profile.name.as_str())).map(|routine| format!("\"{}\"", routine.title)).collect();
        if !routines.is_empty() {
            return Some(format!("✗ {} is the profile the routine {} starts its tasks with; give {} another first", profile.name, routines.join(", "), if routines.len() == 1 { "it" } else { "them" }));
        }
        None
    }

    fn deletion(&self, profile: &Profile) -> (String, String, bool) {
        let file = profile.file.as_deref().map(|file| self.shown_file(file)).unwrap_or_default();
        let Some(role) = profiles::role_name(&self.paths.profiles, profile) else { return (format!("{file} goes."), format!("✓ removed {file}"), false) };
        let others: Vec<String> = profiles::role_readers(&self.paths.profiles, &role).into_iter().filter(|reader| *reader != profile.name).collect();
        match others.as_slice() {
            [] => (format!("{file} goes, and its role {role}, which no other profile reads."), format!("✓ removed {file} and its role {role}"), true),
            others => (format!("{file} goes; its role {role} stays, as {} reads it too.", others.join(", ")), format!("✓ removed {file}; its role {role} stays for {}", others.join(", ")), false),
        }
    }

    fn ask_delete(&mut self) {
        let Some(profile) = self.profile() else { return };
        match self.kept(&profile) {
            Some(why) => self.say(why, RED),
            None => self.mode = Mode::Delete { name: profile.name, nudged: false },
        }
    }

    fn delete(&mut self, name: &str) {
        let Some(profile) = self.agents.profiles.iter().find(|profile| profile.name == name).cloned() else { return };
        if let Some(why) = self.kept(&profile) {
            return self.say(why, RED);
        }
        let (_, said, role_goes) = self.deletion(&profile);
        let removed = profile.file.as_deref().map(std::fs::remove_file).transpose().and_then(|_| match &profile.role {
            Some(role) if role_goes && role.starts_with(&self.paths.profiles) => std::fs::remove_file(role),
            _ => Ok(()),
        });
        self.load();
        match removed {
            Ok(()) => self.say(said, GREEN),
            Err(error) => self.say(format!("✗ {name}: {error}"), RED),
        }
    }

    fn menu(&mut self, key: char) -> Effect {
        let Some(profile) = self.profile() else { return Effect::Handled };
        match key {
            's' => {
                let file = self.paths.kebbie.join(profiles::kebbie_role_for(&profile).1);
                if file.is_file() {
                    return Effect::Show(file);
                }
                self.say(format!("✗ {} is not in kebbie's install", home_relative(&file)), RED);
            }
            'n' => self.wizard(&profile.name),
            'a' => {
                if let Some(role) = profile.role {
                    return Effect::Open(role);
                }
                let file = format!("{}.md", profile.name);
                let path = self.paths.profiles.join(&file);
                let made = !path.exists();
                if made && let Err(error) = crate::state::write(&path, String::new()) {
                    _ = self.fail(format!("✗ {file}: {error}"));
                    return Effect::Handled;
                }
                if self.write_profile(&[("role", Some(&file))]).is_err() {
                    if made {
                        let _ = std::fs::remove_file(&path);
                    }
                    return Effect::Handled;
                }
                return Effect::Open(path);
            }
            _ => {}
        }
        Effect::Handled
    }

    fn summary(&self, profile: &Profile) -> String {
        let model = self.model_said(profile);
        let mut said = vec![format!("{}: {}, {model}", profile.name, profile.agent.key())];
        said.extend(profile.effort.clone());
        let mut text = said.join(", ");
        if let Some(role) = profiles::role_name(&self.paths.profiles, profile) {
            text.push_str(&format!(" · role {role}"));
        }
        if profiles::built_in(&profile.name).is_some() && let Some(file) = &profile.file {
            text.push_str(&format!(" · your settings in {}", self.shown_file(file)));
        }
        text
    }

    fn write_profile(&mut self, changes: &[(&str, Option<&str>)]) -> Result<()> {
        let Some(profile) = self.profile() else { return Ok(()) };
        let name = profile.name.clone();
        let edited = profiles::edit_in(&self.paths.profiles, &name, changes);
        self.load();
        let now = self.agents.profiles.iter().find(|profile| profile.name == name).map(|profile| self.runs(profile)).unwrap_or_default();
        let keys = changes.iter().map(|(key, _)| *key).collect::<Vec<_>>().join(if edited.as_ref().is_ok_and(|edited| *edited == Edited::Unchanged) { " or " } else { " and " });
        match edited {
            Ok(Edited::Wrote(file)) => {
                let said = match changes {
                    [(key, Some(value))] => format!("✓ wrote {key} = {} to {}", Set::Text(value.to_string()).shown(), self.shown_file(&file)),
                    _ => format!("✓ removed {keys} from {}", self.shown_file(&file)),
                };
                self.say(format!("{said} · {now}; a running task keeps what it started with"), GREEN);
            }
            Ok(Edited::Removed(file)) => self.say(format!("✓ removed {}, which held nothing else · {now}", self.shown_file(&file)), GREEN),
            Ok(Edited::Unchanged) if changes.len() > 1 => self.say(format!("{name} has no {keys} of its own · {now}"), MUTED),
            Ok(Edited::Unchanged) => self.say(format!("{name} has that {keys} already · {now}"), MUTED),
            Err(error) => return self.fail(format!("✗ {}", error.lines().next().unwrap_or_default().replace(&self.paths.profiles.display().to_string(), "profiles"))),
        }
        Ok(())
    }

    fn layer_word(&self, layer: Layer) -> (String, Color) {
        match layer {
            Layer::Default => ("default".to_string(), MUTED),
            Layer::Machine => ("machine".to_string(), BLUE),
            Layer::Repo if self.linked() => ("team link".to_string(), PEACH),
            Layer::Repo => (format!("repo {}", self.repo_name()), TEAL),
        }
    }

    pub fn hint_lines(&self) -> (Line<'static>, Line<'static>) {
        let line = |hints: &[&str]| Line::from(Span::styled(hints.join(" · "), style(MUTED)));
        if self.on_profiles() {
            return (
                line(&["↑ ↓ profile", "← → section", "m model", "e effort", "r role", "n new", "⏎ open", "d default", "x delete", "tab machine / repo", "/ search", "esc board"]),
                line(&["↑ ↓ profile", "m model", "e effort", "r role", "n new", "tab repo", "esc board"]),
            );
        }
        let change = match self.current().map(|entry| entry.kind) {
            Some(Kind::Bool) => "space toggle",
            Some(Kind::Choice(_) | Kind::Skill | Kind::Profile | Kind::ReviewProfile | Kind::SecondOpinion) => "⏎ pick",
            _ => "⏎ edit",
        };
        (
            line(&["↑ ↓ key", "← → section", change, "d back to default", "tab machine / repo", "/ search", "esc board"]),
            line(&["↑ ↓ key", change, "d default", "tab repo", "esc board"]),
        )
    }

    fn row_lines(&self, width: usize) -> (Vec<Line<'static>>, usize) {
        let sections = self.sections();
        let Some((_, rows)) = sections.get(self.section) else {
            let empty = if self.filter_text().is_empty() { "no keys" } else { "no key matches" };
            return (vec![Line::from(Span::styled(empty.to_string(), style(MUTED)))], 0);
        };
        let key_width = rows.iter().map(|index| self.entries[*index].key.len()).max().unwrap_or(0).min(width / 2);
        let layer_width = 12usize;
        let value_width = width.saturating_sub(key_width + layer_width + 6);
        let (mut lines, mut selected) = (Vec::new(), 0);
        for (at, index) in rows.iter().enumerate() {
            let entry = &self.entries[*index];
            let chosen = at == self.row;
            let value = match (&self.mode, chosen) {
                (Mode::Edit { buffer, .. }, true) => Span::styled(format!("{buffer}▏"), bold(TEXT)),
                _ if entry.kind == Kind::Bool && entry.value == "true" => Span::styled("● on".to_string(), style(GREEN)),
                _ if entry.kind == Kind::Bool => Span::styled("○ off".to_string(), style(MUTED)),
                _ => Span::styled(cut(&entry.value, value_width), style(TEXT)),
            };
            let (layer, color) = self.layer_word(entry.layer);
            let used = text_width(&value.content);
            let bar = if chosen { Span::styled("▌", style(MAUVE)) } else { Span::raw(" ") };
            let key = Span::styled(format!(" {:<key_width$}  ", cut(entry.key, key_width)), if chosen { bold(TEXT) } else { style(TEXT) });
            let gap = Span::raw(" ".repeat(value_width.saturating_sub(used) + 1));
            let line = Line::from(vec![bar, key, value, gap, Span::styled(format!("{layer:>layer_width$}"), style(color))]);
            if chosen {
                selected = lines.len();
                lines.push(line.style(Style::new().bg(SURFACE)));
                lines.extend(wrap(&entry.about, width.saturating_sub(4), 8).into_iter().map(|text| Line::from(Span::styled(format!("    {text}"), style(SUBTEXT)))));
                lines.push(Line::default());
            } else {
                lines.push(line);
            }
        }
        (lines, selected)
    }

    fn adapter(&self, profile: &Profile, chosen: bool) -> Vec<Span<'static>> {
        let agents = &self.agents;
        let dot = || Span::styled(" · ", style(MUTED));
        let mut spans = vec![Span::styled(profile.agent.key().to_string(), style(if profile.agent == Agent::Codex { BLUE } else { TEAL }))];
        let warned = agents.warning(profile).is_some();
        match (&self.mode, chosen) {
            (Mode::Edit { typed: Typed::Model, buffer }, true) => spans.extend([dot(), Span::styled(format!("{buffer}▏"), bold(TEXT))]),
            _ => {
                if let Some(model) = agents.model(profile) {
                    spans.extend([dot(), Span::styled(model, style(if warned { RED } else if agents.own_model(profile).is_some() { TEXT } else { MUTED }))]);
                }
                if warned {
                    spans.push(Span::styled(" ⚠", style(RED)));
                }
            }
        }
        if let Some(effort) = &profile.effort {
            spans.extend([dot(), Span::styled(effort.clone(), style(TEXT))]);
        }
        spans
    }

    fn role_spans(&self, profile: &Profile) -> Vec<Span<'static>> {
        let own = profiles::role_name(&self.paths.profiles, profile).map(|role| Span::styled(format!("+ {role}"), style(BLUE)));
        match (profiles::built_in(&profile.name).is_some(), own) {
            (true, Some(own)) => vec![Span::styled(format!("{} ", profiles::kebbie_role_for(profile).0), style(MUTED)), own],
            (_, Some(own)) => vec![own],
            (_, None) => vec![Span::styled(profiles::kebbie_role_for(profile).0.to_string(), style(MUTED))],
        }
    }

    fn profile_lines(&self, width: usize) -> (Vec<Line<'static>>, usize) {
        let agents = &self.agents;
        let name_width = agents.profiles.iter().map(|profile| text_width(&profile.name)).max().unwrap_or(0).min(width / 4) + 2;
        let adapters: Vec<Vec<Span<'static>>> = agents.profiles.iter().enumerate().map(|(at, profile)| self.adapter(profile, at == self.row)).collect();
        let adapter_width = adapters.iter().map(|spans| spans.iter().map(Span::width).sum::<usize>()).max().unwrap_or(0) + 3;
        let role_width = agents.profiles.iter().map(|profile| self.role_spans(profile).iter().map(Span::width).sum::<usize>()).max().unwrap_or(0);
        let roles = 2 + name_width + adapter_width + role_width <= width;
        let muted = |text: &str, color: Color| wrap(text, width.saturating_sub(2), 4).into_iter().map(|text| Line::from(Span::styled(format!("  {}", cut(&text, width.saturating_sub(2))), style(color)))).collect::<Vec<_>>();
        let (mut lines, mut selected) = (Vec::new(), 0);
        let kebbies = agents.profiles.iter().filter(|profile| profiles::built_in(&profile.name).is_some()).count();
        let heading = |lines: &mut Vec<Line<'static>>, title: &str, about: String| {
            let mut line = group_line(title, if title == "YOURS" { agents.profiles.len() - kebbies } else { kebbies }, TEAL);
            line.spans.insert(0, Span::raw(" "));
            lines.push(line);
            lines.extend(muted(&about, MUTED));
        };
        heading(&mut lines, "FROM KEBBIE", "follow kebbie's updates; your settings on them are kept".to_string());
        let mut yours = false;
        for (at, profile) in agents.profiles.iter().enumerate() {
            if !yours && profiles::built_in(&profile.name).is_none() {
                yours = true;
                lines.push(Line::default());
                heading(&mut lines, "YOURS", format!("{} · an update never writes here", home_relative(&self.paths.profiles)));
            }
            let chosen = at == self.row;
            let used: usize = adapters[at].iter().map(Span::width).sum();
            let mut spans = vec![
                if chosen { Span::styled("▌", style(MAUVE)) } else { Span::raw(" ") },
                Span::styled(format!(" {:<name_width$}", cut(&profile.name, name_width - 2)), if chosen { bold(TEXT) } else { style(TEXT) }),
            ];
            spans.extend(adapters[at].clone());
            if roles {
                spans.push(Span::raw(" ".repeat(adapter_width.saturating_sub(used))));
                spans.extend(self.role_spans(profile));
            }
            let line = fit(spans, width);
            if !chosen {
                lines.push(line);
                continue;
            }
            selected = lines.len();
            lines.push(line.style(Style::new().bg(SURFACE)));
            let kebbies = profiles::kebbie_role_for(profile).0;
            for (number, text) in wrap(&format!("{kebbies}, from kebbie {}, updates with kebbie", agents.version), width.saturating_sub(11), 4).into_iter().enumerate() {
                lines.push(Line::from(vec![Span::styled(if number == 0 { "    role:  " } else { "           " }, style(SUBTEXT)), Span::styled(text, style(SUBTEXT))]));
            }
            if let Some(role) = profiles::role_name(&self.paths.profiles, profile) {
                lines.push(fit(vec![Span::raw("         "), Span::styled(format!("+ {role}"), style(BLUE)), Span::styled(", yours: read after kebbie's role", style(SUBTEXT))], width));
            }
            let last = if profiles::built_in(&profile.name).is_some() { ("d", "default") } else { ("x", "delete") };
            let mut keys = vec![Span::raw("    ")];
            for (key, word) in [("m", "model"), ("e", "effort"), ("r", "role"), ("n", "new profile"), ("⏎", "open file"), last] {
                let shown = format!(" {word}");
                let fits = keys.iter().map(Span::width).sum::<usize>() + 2 + text_width(key) + text_width(&shown) <= width;
                if keys.len() > 1 && !fits {
                    lines.push(Line::from(std::mem::replace(&mut keys, vec![Span::raw("    ")])));
                }
                keys.extend([Span::raw(if keys.len() == 1 { "" } else { "  " }), Span::styled(key, style(MAUVE)), Span::styled(shown, style(SUBTEXT))]);
            }
            lines.push(Line::from(keys));
            lines.push(Line::default());
        }
        if !yours {
            lines.push(Line::default());
            heading(&mut lines, "YOURS", format!("{} · an update never writes here", home_relative(&self.paths.profiles)));
            lines.extend(muted("none yet: n makes one", MUTED));
        }
        if !agents.broken.is_empty() {
            lines.push(Line::default());
            let mut line = group_line("DOES NOT LOAD", agents.broken.len(), RED);
            line.spans.insert(0, Span::raw(" "));
            lines.push(line);
            lines.extend(muted("kebbie leaves these out until they load; ⏎ opens one", MUTED));
        }
        for (at, (name, file, error)) in agents.broken.iter().enumerate() {
            let chosen = agents.profiles.len() + at == self.row;
            let shown = self.shown_file(file);
            let row = fit(vec![
                if chosen { Span::styled("▌", style(MAUVE)) } else { Span::raw(" ") },
                Span::styled(format!(" {:<name_width$}", cut(name, name_width - 2)), if chosen { bold(RED) } else { style(RED) }),
                Span::styled(shown.clone(), style(MUTED)),
            ], width);
            if !chosen {
                lines.push(row);
                continue;
            }
            selected = lines.len();
            lines.push(row.style(Style::new().bg(SURFACE)));
            let error = error.replace(&file.display().to_string(), &shown).replace(&self.paths.profiles.display().to_string(), "profiles");
            lines.extend(wrap(&error, width.saturating_sub(4), 4).into_iter().map(|text| Line::from(Span::styled(format!("    {text}"), style(RED)))));
            lines.push(Line::from(vec![Span::raw("    "), Span::styled("⏎", style(MAUVE)), Span::styled(" open file", style(SUBTEXT))]));
            lines.push(Line::default());
        }
        lines.push(Line::default());
        lines.extend(muted("dim = the agent's own default:", MUTED));
        let (claude, codex) = (home_relative(&self.paths.claude_home.join("settings.json")), home_relative(&self.paths.codex_home.join("config.toml")));
        lines.extend(muted(&format!("Claude's from {claude}, Codex's from {codex}"), MUTED));
        (lines, selected)
    }

    pub fn draw(&mut self, frame: &mut Frame, body: Rect) {
        let [title, tabs, _, main, said] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(2)]).areas(body);
        let right = "from the board: , · or kebbie config edit";
        let gap = (title.width as usize).saturating_sub(8 + right.len()).max(1);
        frame.render_widget(Paragraph::new(Line::from(vec![Span::styled("Settings", bold(MAUVE)), Span::raw(" ".repeat(gap)), Span::styled(right, style(MUTED))])), title);
        let lit = |on: bool| if on { bold(MANTLE).bg(MAUVE) } else { style(MUTED) };
        let mut spans = vec![Span::styled(" This machine ", lit(self.tab == Tab::Machine)), Span::raw("  ")];
        if self.repo.is_some() {
            spans.push(Span::styled(format!(" Repo: {} ", self.repo_name()), lit(self.tab == Tab::Repo)));
            spans.push(Span::raw("  "));
        }
        let file = home_relative(&self.file());
        let profiles_file = home_relative(&self.paths.profiles.join("<name>.toml"));
        let writes = match (self.tab, self.linked()) {
            (Tab::Machine, _) if self.on_profiles() => format!("writes {profiles_file}"),
            (Tab::Machine, _) => format!("writes {file}"),
            (Tab::Repo, false) => format!("writes {file}, which wins over the machine"),
            (Tab::Repo, true) => format!("{file} links to a team file"),
        };
        spans.push(Span::styled(writes, style(MUTED)));
        frame.render_widget(Paragraph::new(Line::from(spans)), tabs);
        if let Some(error) = &self.error {
            let lines: Vec<Line> = wrap(error, main.width as usize, 6).into_iter().map(|text| Line::from(Span::styled(text, style(RED)))).collect();
            frame.render_widget(Paragraph::new(lines), main);
        } else {
            let [rail, _, rows] = Layout::horizontal([Constraint::Length(20), Constraint::Length(2), Constraint::Min(20)]).areas(main);
            let sections = self.sections();
            let rail_lines: Vec<Line> = sections.iter().enumerate().map(|(at, (name, keys))| {
                let chosen = at == self.section;
                let mark = if chosen { Span::styled("▸ ", style(MAUVE)) } else { Span::raw("  ") };
                let line = Line::from(vec![mark, Span::styled(cut(name, 14), if chosen { bold(TEXT) } else { style(TEXT) }), Span::styled(format!(" {}", keys.len()), style(FAINT))]);
                if chosen { line.style(Style::new().bg(SURFACE)) } else { line }
            }).collect();
            frame.render_widget(Paragraph::new(rail_lines), rail);
            let (lines, selected) = if self.on_profiles() { self.profile_lines(rows.width as usize) } else { self.row_lines(rows.width as usize) };
            let scroll = selected.saturating_sub(rows.height.saturating_sub(10) as usize);
            frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), rows);
        }
        let under = match &self.mode {
            Mode::Filter { buffer } => Some((format!("/ {buffer}▏  · ⏎ keep · esc clear"), TEXT)),
            Mode::Edit { typed, .. } => Some((match typed { Typed::SkillName => "type a skill: <skill> or <plugin>:<skill>", Typed::SkillFile => "type the path of a .md file, starting with / or ~/", Typed::Model => "type the model's id, as the agent takes it", Typed::Value => "editing in place" }.to_string() + " · ⏎ save · esc cancel", SUBTEXT)),
            Mode::Normal if self.said.is_none() && self.on_profiles() => self.profile().map(|profile| match self.agents.warning(&profile) {
                Some(warning) => (warning, YELLOW),
                None => (self.summary(&profile), SUBTEXT),
            }).or_else(|| self.broken().map(|(name, _, _)| (format!("{name} does not load, so no task can start with it; ⏎ opens its file"), RED))),
            _ => self.said.clone(),
        };
        if let Some((text, color)) = under {
            let lines: Vec<Line> = wrap(&text, said.width as usize, 2).into_iter().map(|text| Line::from(Span::styled(text, style(color)))).collect();
            frame.render_widget(Paragraph::new(lines), said);
        }
        self.hits.clear();
        match &self.mode {
            Mode::Choose(chooser) => {
                let width = POPUP.min(frame.area().width.saturating_sub(8) as usize);
                let (lines, spots) = chooser.view(width);
                self.hits = modal_spots(frame, chooser.title.clone(), lines, spots, width).1;
            }
            Mode::Wizard(wizard) => {
                let width = POPUP.min(frame.area().width.saturating_sub(8) as usize);
                let (lines, spots) = wizard.view(width);
                self.hits = modal_spots(frame, "new profile".to_string(), lines, spots, width).1;
            }
            Mode::Delete { name, nudged } => {
                let about = self.agents.profiles.iter().find(|profile| &profile.name == name).map(|profile| self.deletion(profile).0).unwrap_or_default();
                modal(frame, "delete".to_string(), widgets::confirm_lines(name, Some(&about), "Delete this profile?", *nudged), 72);
            }
            Mode::Menu { name, index } => {
                let intro = wrap(&format!("{name} is kebbie's: its role comes with kebbie and an update replaces it, so it is not edited here."), 56, 3);
                let mut lines: Vec<Line> = intro.into_iter().map(|text| Line::from(Span::styled(text, style(TEXT)))).collect();
                lines.push(Line::default());
                let choices = self.menu_choices(name);
                let spots = (0..choices.len()).flat_map(|at| [(lines.len() + 2 * at, Spot::Choice(at)), (lines.len() + 2 * at + 1, Spot::Choice(at))]).collect();
                lines.extend(options_lines(choices, *index, 56));
                self.hits = modal_spots(frame, format!("role · {name}"), lines, spots, 58).1;
            }
            Mode::Team { key, .. } => {
                let target = std::fs::canonicalize(self.file()).map(|path| home_relative(&path)).unwrap_or_default();
                let lines = vec![
                    Line::from(Span::styled(format!("{} links to the team's file {target}.", home_relative(&self.file())), style(TEXT))),
                    Line::default(),
                    Line::from(vec![Span::styled("t", style(MAUVE)), Span::styled("  change the team's file, for everyone who links it", style(TEXT))]),
                    Line::from(vec![Span::styled("o", style(MAUVE)), Span::styled("  override it here: the link becomes a copy of this machine's own", style(TEXT))]),
                    Line::default(),
                    Line::from(Span::styled("esc cancel", style(FAINT))),
                ];
                modal(frame, format!("Change {key}"), lines, 76);
            }
            _ => {}
        }
    }

    fn draw_alone(&mut self, frame: &mut Frame) {
        let area = frame.area().inner(Margin { horizontal: 2, vertical: 1 });
        let [body, hint] = Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).areas(area);
        self.draw(frame, body);
        let (full, short) = self.hint_lines();
        let line = if full.width() <= hint.width as usize { full } else { short };
        let spans: Vec<Span> = line.spans.into_iter().map(|span| Span::styled(span.content.replace("esc board", "esc quit"), span.style)).collect();
        frame.render_widget(Paragraph::new(Line::from(spans)), hint);
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
