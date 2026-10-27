use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::board::widgets::{BLUE, FAINT, GREEN, MANTLE, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, TEAL, TEXT, bold, cut, modal, style, text_width, wrap};
use crate::config::{self, Entry, Kind, Layer, Scope};
use crate::config_edit::{self, Set};
use crate::sh::Result;

pub enum Effect {
    Handled,
    Left,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab {
    Machine,
    Repo,
}

// The page's sections, by the table a key sits in; a key's own line comes from kebbie config's entries.
const SECTIONS: [(&str, &[&str]); 7] = [
    ("Tasks", &["task"]),
    ("Board", &["board"]),
    ("Worktrees", &["worktree"]),
    ("Review & docs", &["review", "ci", "docs"]),
    ("Updates", &["update", "restart", "keeper", "recover"]),
    ("This machine", &["machine", "hub", "notify", "vault"]),
    ("Reports", &["report", "issues"]),
];

fn section_of(key: &str) -> usize {
    let table = key.split('.').next().unwrap_or_default();
    SECTIONS.iter().position(|(_, tables)| tables.contains(&table)).unwrap_or(SECTIONS.len() - 1)
}

#[derive(Clone, Debug, PartialEq)]
enum Change {
    Set(Set),
    Remove,
    Copy(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Typed {
    Value,
    SkillName,
    SkillFile,
}

#[derive(Clone, Debug)]
struct Pick {
    label: String,
    about: String,
    then: Then,
}

#[derive(Clone, Debug)]
enum Then {
    Write(Change),
    Type(Typed),
}

enum Mode {
    Normal,
    Pick { picks: Vec<Pick>, index: usize },
    Edit { typed: Typed, buffer: String },
    Filter { buffer: String },
    Team { key: &'static str, change: Change },
}

pub struct Paths {
    pub machine: PathBuf,
    pub repos_dir: PathBuf,
    pub claude_home: PathBuf,
    pub kebbie: PathBuf,
}

impl Paths {
    pub fn current() -> Self {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        Paths {
            machine: config::path(),
            repos_dir: config::repos_dir(),
            claude_home: crate::agent::claude::home().unwrap_or_else(|| home.join(".claude")),
            kebbie: crate::lead::install_root().unwrap_or_else(|_| home.join(".local/share/kebbie/current")),
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
        Page { paths, repo, tab: Tab::Machine, section: 0, row: 0, filter: String::new(), mode: Mode::Normal, said: None, entries: Vec::new(), error: None }
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
        SECTIONS.iter().enumerate().map(|(at, (name, _))| (name.to_string(), (0..self.entries.len()).filter(|index| section_of(self.entries[*index].key) == at).collect::<Vec<_>>()))
            .filter(|(_, rows)| !rows.is_empty()).collect()
    }

    fn current(&self) -> Option<Entry> {
        let sections = self.sections();
        sections.get(self.section).and_then(|(_, rows)| rows.get(self.row)).map(|index| self.entries[*index].clone())
    }

    fn say(&mut self, text: String, color: Color) {
        self.said = Some((text, color));
    }

    fn picks(&self, entry: &Entry) -> Vec<Pick> {
        let now = edit_text(&entry.value);
        match &entry.kind {
            Kind::Choice(options) => options.iter().map(|option| Pick {
                label: if *option == "none" { "none".to_string() } else { format!("{option:?}") },
                about: if *option == now || (*option == "none" && entry.value == "none") { "now".to_string() } else { String::new() },
                then: Then::Write(if *option == "none" { Change::Remove } else { Change::Set(Set::Text(option.to_string())) }),
            }).collect(),
            Kind::Skill => {
                let default = default_skill(entry.key);
                let skill = default.split_once(':').map_or(default, |(_, skill)| skill);
                vec![
                    Pick { label: format!("{default:?}"), about: "built in".to_string(), then: Then::Write(Change::Set(Set::Text(default.to_string()))) },
                    Pick { label: "Your skill, by name".to_string(), about: "<skill> or <plugin>:<skill>".to_string(), then: Then::Type(Typed::SkillName) },
                    Pick { label: "A Markdown file, by path".to_string(), about: "an absolute or ~ path to a .md file".to_string(), then: Then::Type(Typed::SkillFile) },
                    Pick { label: format!("Copy kebbie's to {}/{skill} and edit it", home_relative(&self.paths.claude_home.join("skills"))), about: format!("then the key names \"{skill}\""), then: Then::Write(Change::Copy(skill)) },
                ]
            }
            _ => Vec::new(),
        }
    }

    pub fn paste(&mut self, text: &str) -> bool {
        match &mut self.mode {
            Mode::Edit { buffer, .. } | Mode::Filter { buffer } => {
                buffer.push_str(text.trim_end_matches(['\n', '\r']));
                true
            }
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
            Mode::Pick { picks, index } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::Pick { index: index.saturating_sub(1), picks },
                KeyCode::Down | KeyCode::Char('j') => self.mode = Mode::Pick { index: (index + 1).min(picks.len().saturating_sub(1)), picks },
                KeyCode::Enter => match (picks.get(index).map(|pick| pick.then.clone()), self.current()) {
                    (Some(Then::Write(change)), Some(entry)) => self.request(entry.key, change),
                    (Some(Then::Type(typed)), Some(entry)) => {
                        let now = edit_text(&entry.value);
                        let keep = match typed { Typed::SkillFile => config::skill_path(&now).is_some(), _ => now != default_skill(entry.key) && config::skill_path(&now).is_none() };
                        self.mode = Mode::Edit { typed, buffer: if keep { now } else { String::new() } };
                    }
                    _ => {}
                },
                KeyCode::Esc | KeyCode::Char('q') => {}
                _ => self.mode = Mode::Pick { picks, index },
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
            Mode::Team { key: name, change } => match key.code {
                KeyCode::Char('t') => self.apply(name, change, false),
                KeyCode::Char('o') => self.apply(name, change, true),
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => {}
                _ => self.mode = Mode::Team { key: name, change },
            },
        }
        Effect::Handled
    }

    fn handle_normal(&mut self, key: KeyEvent) -> Effect {
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
            Kind::Choice(_) | Kind::Skill => {
                let picks = self.picks(&entry);
                let index = picks.iter().position(|pick| pick.about == "now").unwrap_or(0);
                self.mode = Mode::Pick { picks, index };
            }
            Kind::Secret => self.say(format!("{} is never shown here; set it in {} by hand", entry.key, home_relative(&self.paths.machine)), MUTED),
            Kind::Text | Kind::Number | Kind::List => self.mode = Mode::Edit { typed: Typed::Value, buffer: edit_text(&entry.value) },
        }
    }

    fn save_typed(&mut self, typed: Typed, buffer: &str) {
        let Some(entry) = self.current() else { return };
        let text = buffer.trim().to_string();
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
        self.apply(key, change, false);
    }

    fn apply(&mut self, key: &'static str, change: Change, replace_link: bool) {
        let file = self.file();
        let machine = self.tab == Tab::Machine || self.repo.is_none();
        let name = file.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let shown = if machine { name.clone() } else { format!("repos/{name}") };
        let (change, copied) = match change {
            Change::Copy(skill) => match copy_skill(&self.paths.kebbie, &self.paths.claude_home, skill) {
                Ok((to, true)) => (Change::Set(Set::Text(skill.to_string())), format!(" · copied kebbie's skill to {}: edit {}", home_relative(&to), home_relative(&to.join("SKILL.md")))),
                Ok((to, false)) => (Change::Set(Set::Text(skill.to_string())), format!(" · {} was already there, so nothing was copied: edit its SKILL.md", home_relative(&to))),
                Err(error) => return self.say(format!("✗ copying kebbie's {skill}: {error}"), RED),
            },
            other => (other, String::new()),
        };
        let written = match &change {
            Change::Set(set) => config_edit::write(&file, machine, replace_link, |text| config_edit::set(text, key, set)).map(|()| format!("✓ wrote {key} = {} to {shown}", set.shown())),
            _ => config_edit::write(&file, machine, replace_link, |text| config_edit::remove(text, key)).map(|()| format!("✓ removed {key} from {shown}; the layer below applies")),
        };
        match written {
            Ok(said) => {
                let check = if machine { crate::doctor::config_file(&file).0 } else { crate::doctor::repo_file(&file).0 };
                let level = match check.level { crate::doctor::Level::Ok => "ok", crate::doctor::Level::Warn => "warn", crate::doctor::Level::Fail => "fail" };
                let fact = check.fact.replace(&file.display().to_string(), &shown);
                let color = if check.level == crate::doctor::Level::Ok { GREEN } else { PEACH };
                let linked = if replace_link { " (now a file of this machine's own)" } else { "" };
                self.say(format!("{said}{linked}{copied} · doctor: {level}, {fact}"), color);
            }
            Err(error) => self.say(format!("✗ {}", error.lines().next().unwrap_or_default()), RED),
        }
        self.load();
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
        let change = match self.current().map(|entry| entry.kind) {
            Some(Kind::Bool) => "space toggle",
            Some(Kind::Choice(_) | Kind::Skill) => "⏎ pick",
            _ => "⏎ edit",
        };
        let line = |hints: &[&str]| Line::from(Span::styled(hints.join(" · "), style(MUTED)));
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
        let writes = match (self.tab, self.linked()) {
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
            let (lines, selected) = self.row_lines(rows.width as usize);
            let scroll = selected.saturating_sub(rows.height.saturating_sub(10) as usize);
            frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), rows);
        }
        let under = match &self.mode {
            Mode::Filter { buffer } => Some((format!("/ {buffer}▏  · ⏎ keep · esc clear"), TEXT)),
            Mode::Edit { typed, .. } => Some((match typed { Typed::SkillName => "type a skill: <skill> or <plugin>:<skill>", Typed::SkillFile => "type the path of a .md file, starting with / or ~/", Typed::Value => "editing in place" }.to_string() + " · ⏎ save · esc cancel", SUBTEXT)),
            _ => self.said.clone(),
        };
        if let Some((text, color)) = under {
            let lines: Vec<Line> = wrap(&text, said.width as usize, 2).into_iter().map(|text| Line::from(Span::styled(text, style(color)))).collect();
            frame.render_widget(Paragraph::new(lines), said);
        }
        match &self.mode {
            Mode::Pick { picks, index } => {
                let title = self.current().map_or_else(String::new, |entry| entry.key.to_string());
                let mut lines: Vec<Line> = picks.iter().enumerate().map(|(at, pick)| {
                    let chosen = at == *index;
                    let about = if pick.about.is_empty() { String::new() } else { format!("  · {}", pick.about) };
                    Line::from(vec![Span::raw(if chosen { "▸ " } else { "  " }), Span::styled(pick.label.clone(), if chosen { bold(MAUVE) } else { style(TEXT) }), Span::styled(about, style(MUTED))])
                        .style(if chosen { Style::new().bg(SURFACE) } else { Style::new() })
                }).collect();
                lines.push(Line::default());
                lines.push(Line::from(Span::styled("↑ ↓ choose · ⏎ set · esc close", style(FAINT))));
                modal(frame, title, lines, 72);
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
            match ratatui::crossterm::event::read().map_err(|error| error.to_string())? {
                ratatui::crossterm::event::Event::Key(key) if key.kind == ratatui::crossterm::event::KeyEventKind::Press => {
                    if crate::keys::ctrl_c(&key) {
                        return Ok(());
                    }
                    if let Effect::Left = page.handle(key) {
                        return Ok(());
                    }
                }
                ratatui::crossterm::event::Event::Paste(text) => {
                    page.paste(&text);
                }
                _ => {}
            }
        }
    })();
    crate::board::terminal::restore_terminal();
    result.map(|()| page.said.map(|(text, _)| text).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: PathBuf,
        page: Page,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(name: &str, machine: &str, repo_file: Option<&str>) -> Fixture {
        let dir = crate::sh::test_dir(name);
        let repo = dir.join("app");
        std::fs::create_dir_all(&repo).unwrap();
        crate::repo::git(&repo, &["init", "--quiet"]).unwrap();
        std::fs::create_dir_all(dir.join("repos")).unwrap();
        std::fs::write(dir.join("config.toml"), machine).unwrap();
        if let Some(text) = repo_file {
            std::fs::write(dir.join("repos/app.toml"), text).unwrap();
        }
        let kebbie = dir.join("kebbie");
        std::fs::create_dir_all(kebbie.join("skills/work-rules")).unwrap();
        std::fs::write(kebbie.join("skills/work-rules/SKILL.md"), "---\nname: work-rules\n---\n# Work rules\n").unwrap();
        let paths = Paths { machine: dir.join("config.toml"), repos_dir: dir.join("repos"), claude_home: dir.join("claude"), kebbie };
        let mut page = Page::new(paths, Some(repo));
        page.load();
        Fixture { page, dir }
    }

    fn press(page: &mut Page, keys: &[KeyCode]) {
        for code in keys {
            page.handle(KeyEvent::from(*code));
        }
    }

    fn type_text(page: &mut Page, text: &str) {
        for character in text.chars() {
            page.handle(KeyEvent::from(KeyCode::Char(character)));
        }
    }

    fn go_to(page: &mut Page, key: &str) {
        page.filter = String::new();
        let sections = page.sections();
        let (section, row) = sections.iter().enumerate().find_map(|(at, (_, rows))| rows.iter().position(|index| page.entries[*index].key == key).map(|row| (at, row))).unwrap_or_else(|| panic!("{key} is not on the {:?} tab", page.tab));
        (page.section, page.row) = (section, row);
    }

    fn said(page: &Page) -> String {
        page.said.clone().map(|(text, _)| text).unwrap_or_default()
    }

    fn file(fixture: &Fixture, name: &str) -> String {
        std::fs::read_to_string(fixture.dir.join(name)).unwrap_or_default()
    }

    fn screen(page: &mut Page) -> String {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
        terminal.draw(|frame| page.draw_alone(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height).map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn the_page_lists_what_kebbie_config_prints_with_each_layer() {
        let mut fixture = fixture("settings-list", "[task]\nsandbox = false\n", Some("[docs]\nrequired = true\n"));
        let loaded = config::resolve_at(fixture.dir.join("config.toml"), None).unwrap();
        let printed: Vec<&str> = config::entries(&loaded).iter().filter(|entry| entry.scope != Scope::Repo).map(|entry| entry.key).collect();
        let mut listed: Vec<&str> = fixture.page.entries.iter().map(|entry| entry.key).collect();
        let mut expected = printed.clone();
        listed.sort();
        expected.sort();
        assert_eq!(listed, expected, "the machine tab is kebbie config's keys, less the repo-only ones");
        let shown = screen(&mut fixture.page);
        assert!(shown.contains("Settings") && shown.contains(" This machine ") && shown.contains(" Repo: app "), "{shown}");
        assert!(shown.contains("▸ Tasks") && shown.contains("task.rules_skill") && shown.contains("\"kebbie:work-rules\""), "{shown}");
        assert!(shown.contains("task.sandbox") && shown.contains("○ off") && shown.lines().any(|line| line.contains("task.sandbox") && line.ends_with("machine")), "{shown}");
        assert!(shown.lines().any(|line| line.contains("task.rules_skill") && line.ends_with("default")), "{shown}");
        assert!(shown.contains("The skill with the rules for how work is planned"), "the selected key's text shows: {shown}");
        press(&mut fixture.page, &[KeyCode::Tab]);
        assert_eq!(fixture.page.tab, Tab::Repo);
        assert!(fixture.page.entries.iter().all(|entry| entry.scope != Scope::Machine), "a config.toml-only key never shows on the repo tab");
        assert!(fixture.page.entries.iter().any(|entry| entry.key == "docs.required"));
        go_to(&mut fixture.page, "docs.required");
        let shown = screen(&mut fixture.page);
        assert!(shown.lines().any(|line| line.contains("docs.required") && line.contains("● on") && line.ends_with("repo app")), "{shown}");
    }

    #[test]
    fn space_turns_steps_on_for_the_machine_and_off_again_for_one_repo() {
        let mut fixture = fixture("settings-steps", "", None);
        go_to(&mut fixture.page, "task.steps");
        assert!(screen(&mut fixture.page).lines().any(|line| line.contains("task.steps") && line.contains("○ off")), "off by default");
        press(&mut fixture.page, &[KeyCode::Char(' ')]);
        assert_eq!(file(&fixture, "config.toml"), "[task]\nsteps = true\n");
        assert!(said(&fixture.page).starts_with("✓ wrote task.steps = true to config.toml"), "{}", said(&fixture.page));
        press(&mut fixture.page, &[KeyCode::Tab]);
        go_to(&mut fixture.page, "task.steps");
        press(&mut fixture.page, &[KeyCode::Char(' ')]);
        assert_eq!(file(&fixture, "repos/app.toml"), "[task]\nsteps = false\n", "a repo keeps its own way of working");
        assert_eq!(fixture.page.current().unwrap().layer, Layer::Repo);
    }

    #[test]
    fn space_toggles_a_boolean_in_the_file_of_the_tab_keeping_its_comments() {
        let mut fixture = fixture("settings-bool", "# mine\n[task]\nsandbox = true # build box\n", None);
        go_to(&mut fixture.page, "task.sandbox");
        press(&mut fixture.page, &[KeyCode::Char(' ')]);
        assert_eq!(file(&fixture, "config.toml"), "# mine\n[task]\nsandbox = false # build box\n");
        assert!(said(&fixture.page).starts_with("✓ wrote task.sandbox = false to config.toml · doctor: ok, config.toml parses"), "{}", said(&fixture.page));
        press(&mut fixture.page, &[KeyCode::Tab]);
        go_to(&mut fixture.page, "ci.watch");
        press(&mut fixture.page, &[KeyCode::Char(' ')]);
        assert_eq!(file(&fixture, "repos/app.toml"), "[ci]\nwatch = false\n", "the repo tab writes the repo's own file");
        assert!(!file(&fixture, "config.toml").contains("[ci]"));
        assert_eq!(fixture.page.current().unwrap().layer, Layer::Repo);
    }

    #[test]
    fn a_choice_is_picked_from_its_list_and_none_removes_the_key() {
        let mut fixture = fixture("settings-choice", "", None);
        go_to(&mut fixture.page, "task.also_noticed");
        press(&mut fixture.page, &[KeyCode::Enter]);
        let shown = screen(&mut fixture.page);
        assert!(shown.contains("▸ \"backlog\"  · now") && shown.contains("\"ask\""), "{shown}");
        press(&mut fixture.page, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
        assert_eq!(file(&fixture, "config.toml"), "[task]\nalso_noticed = \"ask\"\n");
        go_to(&mut fixture.page, "task.permission_mode");
        press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
        assert!(file(&fixture, "config.toml").contains("permission_mode = \"acceptEdits\""));
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert!(matches!(fixture.page.mode, Mode::Pick { index: 2, .. }), "the pick opens on the value in effect");
        press(&mut fixture.page, &[KeyCode::Up, KeyCode::Up, KeyCode::Enter]);
        assert!(!file(&fixture, "config.toml").contains("permission_mode"), "none leaves it to Claude: the key goes");
    }

    #[test]
    fn text_numbers_and_lists_are_edited_in_place_and_a_bad_value_writes_nothing() {
        let mut fixture = fixture("settings-text", "[restart]\nidle_minutes = 10\n", None);
        go_to(&mut fixture.page, "restart.idle_minutes");
        press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Backspace, KeyCode::Backspace]);
        type_text(&mut fixture.page, "25");
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert_eq!(file(&fixture, "config.toml"), "[restart]\nidle_minutes = 25\n");
        press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Backspace, KeyCode::Backspace]);
        type_text(&mut fixture.page, "0");
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert!(said(&fixture.page).starts_with("✗ ") && said(&fixture.page).contains("1 to 1440"), "{}", said(&fixture.page));
        assert_eq!(file(&fixture, "config.toml"), "[restart]\nidle_minutes = 25\n", "a value kebbie refuses is never written");
        press(&mut fixture.page, &[KeyCode::Enter]);
        type_text(&mut fixture.page, "x");
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert!(said(&fixture.page).contains("\"25x\" is not a whole number"), "{}", said(&fixture.page));
        go_to(&mut fixture.page, "task.test_commands");
        press(&mut fixture.page, &[KeyCode::Enter]);
        type_text(&mut fixture.page, "bin/test, make check");
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert!(file(&fixture, "config.toml").contains("[task]\ntest_commands = [\"bin/test\", \"make check\"]\n"), "{}", file(&fixture, "config.toml"));
        go_to(&mut fixture.page, "machine.name");
        press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Char('u')]);
        fixture.page.handle(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        type_text(&mut fixture.page, "devbox");
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert!(file(&fixture, "config.toml").contains("[machine]\nname = \"devbox\"\n"));
        press(&mut fixture.page, &[KeyCode::Enter]);
        type_text(&mut fixture.page, "zzz");
        press(&mut fixture.page, &[KeyCode::Esc]);
        assert!(file(&fixture, "config.toml").contains("name = \"devbox\""), "esc cancels the edit");
    }

    #[test]
    fn d_removes_the_key_so_the_layer_below_applies() {
        let mut fixture = fixture("settings-remove", "[ci]\nwatch = false\n", Some("# team\n[ci]\nwatch = true\n"));
        press(&mut fixture.page, &[KeyCode::Tab]);
        go_to(&mut fixture.page, "ci.watch");
        assert_eq!(fixture.page.current().unwrap().layer, Layer::Repo);
        press(&mut fixture.page, &[KeyCode::Char('d')]);
        assert_eq!(file(&fixture, "repos/app.toml"), "# team\n[ci]\n");
        let entry = fixture.page.current().unwrap();
        assert_eq!((entry.value.as_str(), entry.layer), ("false", Layer::Machine), "the machine's value applies once the repo's goes");
        assert!(said(&fixture.page).starts_with("✓ removed ci.watch from repos/app.toml"), "{}", said(&fixture.page));
    }

    #[test]
    fn a_value_from_a_team_link_asks_whether_to_change_the_team_file_or_override_it_here() {
        let mut fixture = fixture("settings-team", "", None);
        let team = fixture.dir.join("team.toml");
        std::fs::write(&team, "[ci]\nwatch = true\n").unwrap();
        std::os::unix::fs::symlink(&team, fixture.dir.join("repos/app.toml")).unwrap();
        fixture.page.load();
        press(&mut fixture.page, &[KeyCode::Tab]);
        go_to(&mut fixture.page, "ci.watch");
        assert!(screen(&mut fixture.page).lines().any(|line| line.contains("ci.watch") && line.ends_with("team link")));
        press(&mut fixture.page, &[KeyCode::Char(' ')]);
        let shown = screen(&mut fixture.page);
        assert!(shown.contains("change the team's file") && shown.contains("override it here"), "{shown}");
        assert_eq!(std::fs::read_to_string(&team).unwrap(), "[ci]\nwatch = true\n", "nothing is written before the answer");
        press(&mut fixture.page, &[KeyCode::Char('t')]);
        assert_eq!(std::fs::read_to_string(&team).unwrap(), "[ci]\nwatch = false\n");
        press(&mut fixture.page, &[KeyCode::Char(' '), KeyCode::Char('o')]);
        let own = fixture.dir.join("repos/app.toml");
        assert!(!std::fs::symlink_metadata(&own).unwrap().file_type().is_symlink());
        assert_eq!((std::fs::read_to_string(&own).unwrap(), std::fs::read_to_string(&team).unwrap()), ("[ci]\nwatch = true\n".to_string(), "[ci]\nwatch = false\n".to_string()));
    }

    #[test]
    fn a_skill_key_offers_built_in_a_name_a_file_or_a_copy_of_kebbies_to_edit() {
        let mut fixture = fixture("settings-skill", "", None);
        go_to(&mut fixture.page, "task.rules_skill");
        press(&mut fixture.page, &[KeyCode::Enter]);
        let shown = screen(&mut fixture.page);
        assert!(shown.contains("\"kebbie:work-rules\"  · built in") && shown.contains("Your skill, by name") && shown.contains("A Markdown file, by path") && shown.contains("Copy kebbie's to"), "{shown}");
        press(&mut fixture.page, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
        type_text(&mut fixture.page, "~/code/team/RULES.md");
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert_eq!(file(&fixture, "config.toml"), "[task]\nrules_skill = \"~/code/team/RULES.md\"\n");
        press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Enter]);
        type_text(&mut fixture.page, "team-rules");
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert_eq!(file(&fixture, "config.toml"), "[task]\nrules_skill = \"team-rules\"\n");
        press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
        let copied = fixture.dir.join("claude/skills/work-rules/SKILL.md");
        assert_eq!(std::fs::read_to_string(&copied).unwrap(), "---\nname: work-rules\n---\n# Work rules\n");
        assert_eq!(file(&fixture, "config.toml"), "[task]\nrules_skill = \"work-rules\"\n");
        assert!(said(&fixture.page).contains("copied kebbie's skill to") && said(&fixture.page).contains("work-rules/SKILL.md"), "{}", said(&fixture.page));
        std::fs::write(&copied, "mine").unwrap();
        press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
        assert_eq!(std::fs::read_to_string(&copied).unwrap(), "mine", "a copy already there is the user's and stays");
        press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(file(&fixture, "config.toml"), "[task]\nrules_skill = \"kebbie:work-rules\"\n");
        press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
        type_text(&mut fixture.page, "RULES.md");
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert!(said(&fixture.page).contains("a file goes by its absolute or ~ path"), "{}", said(&fixture.page));
        press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Enter]);
        type_text(&mut fixture.page, "team/rules");
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert!(said(&fixture.page).contains("not a skill name or a Markdown file"), "{}", said(&fixture.page));
        assert_eq!(file(&fixture, "config.toml"), "[task]\nrules_skill = \"kebbie:work-rules\"\n");
    }

    #[test]
    fn slash_filters_the_keys_tab_switches_and_esc_leaves() {
        let mut fixture = fixture("settings-filter", "", None);
        press(&mut fixture.page, &[KeyCode::Char('/')]);
        type_text(&mut fixture.page, "idle_m");
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert_eq!(fixture.page.current().map(|entry| entry.key), Some("restart.idle_minutes"));
        assert_eq!(fixture.page.sections()[0].1.len(), 1);
        assert!(screen(&mut fixture.page).contains("Search: idle_m"));
        assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Esc)), Effect::Handled), "the first esc clears the search");
        assert!(fixture.page.filter.is_empty());
        assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Esc)), Effect::Left));
        let mut alone = Page::new(Paths { machine: fixture.dir.join("config.toml"), repos_dir: fixture.dir.join("repos"), claude_home: fixture.dir.join("claude"), kebbie: fixture.dir.join("kebbie") }, None);
        alone.load();
        press(&mut alone, &[KeyCode::Tab]);
        assert_eq!(alone.tab, Tab::Machine);
        assert!(said(&alone).contains("No repo here"));
    }

    #[test]
    fn a_file_that_does_not_parse_is_shown_and_left_alone() {
        let mut fixture = fixture("settings-broken", "[task]\nsandbox = \"yes\"\n", None);
        assert!(fixture.page.entries.is_empty());
        assert!(screen(&mut fixture.page).contains("task.sandbox: must be true or false"));
        press(&mut fixture.page, &[KeyCode::Char(' '), KeyCode::Char('d')]);
        assert_eq!(file(&fixture, "config.toml"), "[task]\nsandbox = \"yes\"\n");
    }
}
