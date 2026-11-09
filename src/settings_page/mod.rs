mod agents;
mod chooser;
mod profile_edit;
mod wizard;

use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::agent::Agent;
use crate::board::widgets::{self, BLUE, FAINT, GREEN, MANTLE, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, Spot, TEAL, TEXT, YELLOW, bold, cut, fit, group_line, modal, modal_spots, options_lines, style, text_width, wrap};
use crate::config::{self, Entry, Kind, Layer, Scope};
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
