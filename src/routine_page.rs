use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::board::keys_window;
use crate::board::widgets::{BLUE, Choice, FAINT, GREEN, MAUVE, MUTED, PEACH, RED, SURFACE, TEXT, YELLOW, bold, choice, column_title, cut, menu_lines, modal, render_column, style, wrap};
use crate::keys::{self, Act, Ctx};
use crate::peers;
use crate::routine_file;
use crate::routine_wizard::{self, Repo, Spot, Step, Wizard};
use crate::routines::{self, Outcome, Routine, Schedule, State, WEEKDAYS};
use crate::sh::Result;
use crate::status::Item;

const DAY: i64 = 86_400;

pub enum Effect {
    Handled,
    Unhandled,
    Task(String),
    Card(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    List,
    Detail,
}

#[derive(Clone, Copy)]
pub enum Hit {
    Row(usize),
    Template(usize),
    Choice(usize),
    Key(KeyEvent),
}

struct Template {
    title: &'static str,
    mode: usize,
    when: &'static str,
    about: &'static str,
    prompt: &'static str,
}

const TEMPLATES: [Template; 3] = [
    Template {
        title: "Health check", mode: 0, when: "07:00", about: "look over a repo's service and say what's wrong",
        prompt: "Look over this repo's service: is it up, are its logs free of new errors, did a job or a deploy fail since the last run? Say what is wrong, how serious it is and what you recommend.",
    },
    Template {
        title: "Dependency bump", mode: 1, when: "mon 08:00", about: "list updates and propose which to take",
        prompt: "List the dependencies that have newer versions, read their changelogs for breaking changes, and propose which to take now and which to leave, with the reason for each.",
    },
    Template {
        title: "Keep CI green", mode: 3, when: "every 6h", about: "fix a red main and hand it to the lead to push",
        prompt: "Check CI on the default branch. When it is red, find the cause and fix it in a commit, with the test that shows it fixed.",
    },
];

enum Mode {
    Normal,
    Options(usize),
    Wizard(Box<Wizard>),
    ConfirmDelete { id: String, nudged: bool },
    Working { label: String, result: Receiver<Result<String>> },
    Message(String),
    Help(keys_window::Scroll),
}

struct Gathered {
    rows: Vec<(Routine, State)>,
    errors: Vec<String>,
}

fn gather(machine: &Path, repos: &Path) -> Gathered {
    let (routines, errors) = routines::load_from(machine, repos);
    let dir = crate::lead::state_dir().ok();
    let rows = routines.into_iter().map(|routine| {
        let state = dir.as_deref().map(|dir| routines::load_state(dir, &routine.id)).unwrap_or_default();
        (routine, state)
    }).collect();
    Gathered { rows, errors }
}

fn spawn_refresher(machine: PathBuf, repos: PathBuf) -> (Receiver<Gathered>, Sender<()>) {
    let (sender, receiver) = mpsc::channel();
    let (refresh, wakeup) = mpsc::channel::<()>();
    thread::spawn(move || {
        loop {
            if sender.send(gather(&machine, &repos)).is_err() {
                return;
            }
            let _ = wakeup.recv_timeout(Duration::from_secs(15));
        }
    });
    (receiver, refresh)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Look {
    Waiting,
    Running,
    Failed,
    Findings,
    Ok,
    New,
    Paused,
}

fn look(routine: &Routine, state: &State, open: bool) -> Look {
    if !routine.enabled {
        return Look::Paused;
    }
    match state.last_outcome {
        Some(Outcome::Findings) if open => Look::Waiting,
        Some(Outcome::Findings) => Look::Findings,
        Some(Outcome::Running) => Look::Running,
        Some(Outcome::Failed) => Look::Failed,
        Some(Outcome::Ok) => Look::Ok,
        None => Look::New,
    }
}

fn rank(look: Look) -> u8 {
    match look {
        Look::Waiting => 0,
        Look::Running => 1,
        Look::Failed => 2,
        Look::Paused => 4,
        _ => 3,
    }
}

fn outcome_mark(outcome: Outcome) -> (&'static str, Color) {
    match outcome {
        Outcome::Ok => ("✓", GREEN),
        Outcome::Findings => ("⚠", PEACH),
        Outcome::Failed => ("✗", RED),
        Outcome::Running => ("◐", BLUE),
    }
}

fn mark(look: Look) -> (&'static str, Color) {
    match look {
        Look::Waiting => ("⚠", PEACH),
        Look::Findings => ("⚠", MUTED),
        Look::Running => ("◐", BLUE),
        Look::Failed => ("✗", RED),
        Look::Ok => ("✓", GREEN),
        Look::New => ("·", FAINT),
        Look::Paused => ("‖", MUTED),
    }
}

fn local_day(at: u64, offset: i64) -> i64 {
    (at as i64 + offset).div_euclid(DAY)
}

fn clock(at: u64, offset: i64) -> String {
    let minute = (at as i64 + offset).rem_euclid(DAY) / 60;
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

fn day_word(at: u64, now: u64, offset: i64) -> String {
    let day = local_day(at, offset);
    match day - local_day(now, offset) {
        0 => "today".to_string(),
        -1 => "yesterday".to_string(),
        1 => "tomorrow".to_string(),
        -6..=6 => WEEKDAYS[(day + 3).rem_euclid(7) as usize].to_string(),
        _ => crate::keeper::stamp(u64::try_from(at as i64 + offset).unwrap_or(at))[..10].to_string(),
    }
}

pub fn past_when(at: u64, now: u64, offset: i64) -> String {
    match day_word(at, now, offset).as_str() {
        "yesterday" => "yesterday".to_string(),
        word => format!("{word} {}", clock(at, offset)),
    }
}

pub fn next_when(at: u64, now: u64, offset: i64) -> String {
    match day_word(at, now, offset).as_str() {
        "today" => clock(at, offset),
        word => word.to_string(),
    }
}

pub fn first_when(at: u64, now: u64, offset: i64) -> String {
    format!("{} {}", day_word(at, now, offset), clock(at, offset))
}

pub fn schedule_words(schedule: Schedule) -> String {
    const DAYS: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
    match schedule {
        Schedule::Every(_) => schedule.text(),
        Schedule::Daily(_) => format!("every day at {}", routine_file::when_text(schedule)),
        Schedule::Weekly(day, _) => format!("every {} at {}", DAYS[day as usize], routine_file::when_text(schedule).split_once(' ').map_or("", |(_, time)| time)),
    }
}

fn schedule_short(schedule: Schedule, narrow: bool) -> String {
    match (schedule, narrow) {
        (Schedule::Every(_), true) => schedule.text().trim_start_matches("every ").to_string(),
        (Schedule::Every(_), false) => schedule.text(),
        (Schedule::Daily(_), true) => routine_file::when_text(schedule),
        (Schedule::Daily(_), false) => format!("daily {}", routine_file::when_text(schedule)),
        (Schedule::Weekly(day, _), true) => WEEKDAYS[day as usize].to_string(),
        (Schedule::Weekly(..), false) => routine_file::when_text(schedule),
    }
}

fn repo_name(repo: &str) -> String {
    repo.trim_end_matches('/').rsplit('/').next().unwrap_or(repo).to_string()
}

fn may(routine: &Routine) -> &'static str {
    match (routine.mode, routine.push) {
        (routines::Mode::Report, _) => "reads and reports, changes nothing",
        (routines::Mode::Plan, _) => "proposes a fix and changes nothing until you answer",
        (routines::Mode::Fix, false) => "commits a fix for you to push",
        (routines::Mode::Fix, true) => "commits a fix the lead pushes once reviewed",
    }
}

pub struct Page {
    rows: Vec<(Routine, State)>,
    errors: Vec<String>,
    pub tasks: Vec<Item>,
    loaded: bool,
    selected: Option<String>,
    template: usize,
    focus: Focus,
    mode: Mode,
    updates: Option<(Receiver<Gathered>, Sender<()>)>,
    pub hits: Vec<(Rect, Hit)>,
    machine: PathBuf,
    repos_dir: PathBuf,
    clock: Option<(u64, i64)>,
    width: u16,
}

impl Page {
    pub fn new(machine: PathBuf, repos_dir: PathBuf) -> Self {
        Self {
            rows: Vec::new(), errors: Vec::new(), tasks: Vec::new(), loaded: false, selected: None, template: 0, focus: Focus::List, mode: Mode::Normal,
            updates: None, hits: Vec::new(), machine, repos_dir, clock: None, width: WIDE,
        }
    }

    pub fn is_normal(&self) -> bool {
        matches!(self.mode, Mode::Normal)
    }

    // True when the wizard is on a step with a text field to take the pasted text.
    pub fn paste(&mut self, text: &str) -> bool {
        let Mode::Wizard(wizard) = &mut self.mode else { return false };
        wizard.paste(text)
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.mode, Mode::Normal | Mode::Message(_) | Mode::Help(_))
    }

    fn now(&self) -> (u64, i64) {
        self.clock.unwrap_or_else(|| {
            let now = crate::messages::now();
            (now, routines::local_offset(now))
        })
    }

    #[cfg(test)]
    pub fn shown(&mut self, rows: Vec<(Routine, State)>) {
        self.rows = rows;
        self.loaded = true;
    }

    pub fn refresh(&self) {
        if let Some((_, refresh)) = &self.updates {
            let _ = refresh.send(());
        }
    }

    // Starts reading the routines on the first call, so the tab's ⚠ count is there before the page is opened.
    pub fn tick(&mut self) {
        if self.updates.is_none() {
            self.updates = Some(spawn_refresher(self.machine.clone(), self.repos_dir.clone()));
        }
        let found: Vec<Gathered> = self.updates.as_ref().map(|(updates, _)| updates.try_iter().collect()).unwrap_or_default();
        for found in found {
            self.take(found);
        }
        if let Mode::Working { result, .. } = &self.mode
            && let Ok(result) = result.try_recv()
        {
            self.show(result);
        }
    }

    fn take(&mut self, found: Gathered) {
        self.rows = found.rows;
        self.errors = found.errors;
        self.loaded = true;
    }

    fn reload(&mut self) {
        let found = gather(&self.machine, &self.repos_dir);
        self.take(found);
        self.refresh();
    }

    fn open_task(&self, state: &State) -> Option<&Item> {
        let task = state.last_task.as_deref()?;
        self.tasks.iter().find(|item| peers::address(&item.name, &item.place) == task)
    }

    fn look_of(&self, routine: &Routine, state: &State) -> Look {
        look(routine, state, self.open_task(state).is_some())
    }

    pub fn waiting(&self) -> usize {
        self.rows.iter().filter(|(routine, state)| self.look_of(routine, state) == Look::Waiting).count()
    }

    fn ordered(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        order.sort_by_key(|index| rank(self.look_of(&self.rows[*index].0, &self.rows[*index].1)));
        order
    }

    fn position(&self) -> usize {
        let order = self.ordered();
        self.selected.as_ref().and_then(|id| order.iter().position(|index| &self.rows[*index].0.id == id)).unwrap_or(0)
    }

    fn current(&self) -> Option<(Routine, State)> {
        self.ordered().get(self.position()).map(|index| self.rows[*index].clone())
    }

    fn select(&mut self, position: usize) {
        let order = self.ordered();
        if let Some(index) = order.get(position.min(order.len().saturating_sub(1))) {
            self.selected = Some(self.rows[*index].0.id.clone());
        }
    }

    fn choices(&self, routine: &Routine, state: &State) -> Vec<Choice> {
        let file = routine_file::file_of(routine);
        let file = file.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let mut choices = Vec::new();
        if self.open_task(state).is_some() {
            choices.push(Choice { key: '⏎', act: None, label: "Go to its task".to_string(), about: "select the run's task on the Work page".to_string(), link: None });
        }
        if state.last_start.is_some() {
            choices.push(choice(Act::Card, "Show the report", "its task's sign-off card, or the run's outcome and summary"));
        }
        choices.push(choice(Act::Go, "Run now", "start a run as a task now; never while the last run's task is open"));
        choices.push(choice(Act::Edit, "Edit", &format!("the same five steps with its values, saved in {file}")));
        choices.push(if routine.enabled {
            choice(Act::Pause, "Pause", &format!("off the schedule until you resume it: enabled = false in {file}"))
        } else {
            choice(Act::Pause, "Resume", "back on its schedule")
        });
        choices.push(choice(Act::Delete, "Delete", &format!("its table goes from {file} (asks first)")));
        choices
    }

    fn show(&mut self, result: Result<String>) {
        self.mode = Mode::Message(result.unwrap_or_else(|error| error));
        self.reload();
    }

    fn in_background(&mut self, label: String, step: impl FnOnce() -> Result<String> + Send + 'static) {
        let (sender, result) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(step());
        });
        self.mode = Mode::Working { label, result };
    }

    fn repos(&self) -> Vec<Repo> {
        let mut repos: Vec<Repo> = Vec::new();
        let mut add = |value: String, shown: String| {
            let name = repo_name(&value);
            if !repos.iter().any(|repo| repo.name == name) {
                repos.push(Repo { value, name, shown });
            }
        };
        for repo in self.tasks.iter().filter_map(|item| item.repo.clone()) {
            let shown = crate::area_page::home_relative(&repo);
            add(shown.clone(), shown);
        }
        for (routine, _) in &self.rows {
            add(routine.repo.clone(), routine.repo.clone());
        }
        let mut found = crate::repo::checkouts();
        found.sort_by_key(|path| crate::repo::name_of(path));
        for repo in found {
            let shown = crate::area_page::home_relative(&repo);
            add(shown.clone(), shown);
        }
        repos
    }

    fn start_wizard(&mut self, template: Option<&Template>) {
        let taken = self.rows.iter().map(|(routine, _)| routine.id.clone()).collect();
        let wizard = match template {
            Some(template) => Wizard::new(template.prompt, template.when, template.mode, template.title, self.repos(), taken, self.machine.clone()),
            None => Wizard::new("", "07:00", 0, "", self.repos(), taken, self.machine.clone()),
        };
        self.mode = Mode::Wizard(Box::new(wizard));
    }

    fn edit_wizard(&mut self, routine: &Routine) {
        let file = routine_file::file_of(routine);
        let default_repo = (file != self.machine).then(|| file.file_stem().map(|stem| stem.to_string_lossy().to_string())).flatten();
        self.mode = Mode::Wizard(Box::new(Wizard::edit(routine, self.repos(), file, default_repo)));
    }

    pub fn handle(&mut self, key: KeyEvent, terminal: Option<&mut DefaultTerminal>) -> Effect {
        let act = keys::map().act(Ctx::Routines, &key);
        let leaves = matches!(act, Some(Act::Back | Act::Close));
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => return self.handle_normal(key, act),
            Mode::Options(index) => {
                let Some((routine, state)) = self.current() else { return Effect::Handled };
                let choices = self.choices(&routine, &state);
                match key.code {
                    KeyCode::Char(character) if choices.iter().any(|choice| choice.key == character) => return self.act(choices.iter().find(|choice| choice.key == character).and_then(|choice| choice.act)),
                    KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::Options(index.saturating_sub(1)),
                    KeyCode::Down | KeyCode::Char('j') => self.mode = Mode::Options((index + 1).min(choices.len().saturating_sub(1))),
                    KeyCode::Enter => {
                        if let Some(choice) = choices.get(index) {
                            return self.act(choice.act);
                        }
                    }
                    _ if leaves => {}
                    _ => self.mode = Mode::Options(index),
                }
            }
            Mode::Wizard(mut wizard) => match wizard.key(key) {
                Step::Stay => self.mode = Mode::Wizard(wizard),
                Step::Cancel => {}
                Step::Editor => {
                    if let Some(terminal) = terminal
                        && let Ok(Some(text)) = edit_in_editor(terminal, &wizard.prompt)
                    {
                        wizard.prompt = text;
                    }
                    self.mode = Mode::Wizard(wizard);
                }
                Step::Write { run } => self.write(*wizard, run),
            },
            Mode::ConfirmDelete { id, .. } => match key.code {
                KeyCode::Char('y') => {
                    let result = match self.rows.iter().find(|(routine, _)| routine.id == id) {
                        Some((routine, _)) => routine_file::delete_in(&self.machine, &routine_file::file_of(routine), &id, crate::lead::state_dir().ok().as_deref()),
                        None => Err(format!("No routine \"{id}\" any more.")),
                    };
                    self.show(result);
                }
                KeyCode::Char('n') => {}
                _ if leaves => {}
                _ => self.mode = Mode::ConfirmDelete { id, nudged: true },
            },
            working @ Mode::Working { .. } => self.mode = working,
            Mode::Message(_) => {}
            Mode::Help(scroll) => {
                if let Some(scroll) = scroll.key(&key) {
                    self.mode = Mode::Help(scroll);
                }
            }
        }
        Effect::Handled
    }

    fn write(&mut self, wizard: Wizard, run: bool) {
        let draft = match wizard.draft() {
            Ok(draft) => draft,
            Err(error) => return self.show(Err(error)),
        };
        let written = match &wizard.editing {
            Some(routine) => routine_file::replace_in(&self.machine, &wizard.file, &routine.id, &draft),
            None => routine_file::add_in(&self.machine, &self.repos_dir, &draft),
        };
        self.selected = Some(draft.id.clone());
        match written {
            Ok(said) if run => {
                self.reload();
                let id = draft.id.clone();
                self.in_background(format!("Starting a run of {}…", draft.title), move || routines::run_now(&id).map(|ran| format!("{said}\n{ran}")));
            }
            result => self.show(result),
        }
    }

    fn narrow_detail(&self) -> bool {
        self.focus == Focus::Detail
    }

    fn handle_normal(&mut self, key: KeyEvent, act: Option<Act>) -> Effect {
        if self.loaded && self.rows.is_empty() {
            match (act, key.code) {
                (Some(Act::Move), KeyCode::Up | KeyCode::Char('k')) => self.template = self.template.saturating_sub(1),
                (Some(Act::Move), _) => self.template = (self.template + 1).min(TEMPLATES.len() - 1),
                (Some(Act::Options), _) => self.start_wizard(Some(&TEMPLATES[self.template])),
                (Some(Act::New), _) => self.start_wizard(None),
                (Some(Act::Help), _) => self.mode = Mode::Help(Default::default()),
                _ => return Effect::Unhandled,
            }
            return Effect::Handled;
        }
        if self.current().is_none() {
            return Effect::Unhandled;
        }
        match act {
            Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => self.select(self.position().saturating_sub(1)),
            Some(Act::Move) => self.select(self.position() + 1),
            Some(Act::Options) if self.wide() || self.narrow_detail() => self.mode = Mode::Options(0),
            Some(Act::Options) => self.focus = Focus::Detail,
            Some(Act::Back) if self.narrow_detail() => self.focus = Focus::List,
            Some(Act::New) => self.start_wizard(None),
            Some(Act::Help) => self.mode = Mode::Help(Default::default()),
            Some(act @ (Act::Go | Act::Edit | Act::Pause | Act::Card | Act::Delete)) => return self.act(Some(act)),
            _ => return Effect::Unhandled,
        }
        Effect::Handled
    }

    fn wide(&self) -> bool {
        self.width >= WIDE
    }

    fn act(&mut self, act: Option<Act>) -> Effect {
        let Some((routine, state)) = self.current() else { return Effect::Handled };
        let open = self.open_task(&state).map(|item| (peers::address(&item.name, &item.place), item.path.clone()));
        match act {
            None => match open {
                Some((address, _)) => return Effect::Task(address),
                None => self.show(Ok(format!("{}'s last run's task is closed.", routine.title))),
            },
            Some(Act::Card) => {
                if let Some((address, Some(path))) = &open
                    && crate::report::card_for(path).is_some()
                {
                    return Effect::Card(address.clone());
                }
                self.mode = Mode::Message(report_text(&routine, &state, self.now()));
            }
            Some(Act::Go) => {
                let id = routine.id.clone();
                self.in_background(format!("Starting a run of {}…", routine.title), move || routines::run_now(&id));
            }
            Some(Act::Edit) => self.edit_wizard(&routine),
            Some(Act::Pause) => {
                let result = routine_file::set_enabled_in(&self.machine, &routine_file::file_of(&routine), &routine.id, !routine.enabled);
                self.show(result);
            }
            Some(Act::Delete) => self.mode = Mode::ConfirmDelete { id: routine.id.clone(), nudged: false },
            _ => {}
        }
        Effect::Handled
    }

    pub fn mouse(&mut self, event: MouseEvent, terminal: Option<&mut DefaultTerminal>) -> Effect {
        let hit = self.hits.iter().find(|(area, _)| area.contains(Position::new(event.column, event.row))).map(|(_, hit)| *hit);
        match event.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let code = if event.kind == MouseEventKind::ScrollUp { KeyCode::Up } else { KeyCode::Down };
                self.handle(KeyEvent::from(code), None)
            }
            MouseEventKind::Down(MouseButton::Left) => match (hit, &self.mode) {
                (Some(Hit::Row(position)), Mode::Normal) => {
                    let again = self.position() == position;
                    self.select(position);
                    if again {
                        return self.handle(KeyEvent::from(KeyCode::Enter), None);
                    }
                    Effect::Handled
                }
                (Some(Hit::Template(index)), Mode::Normal) => {
                    let again = self.template == index;
                    self.template = index;
                    if again {
                        self.start_wizard(Some(&TEMPLATES[index]));
                    }
                    Effect::Handled
                }
                (Some(Hit::Choice(index)), Mode::Wizard(_)) => {
                    let Mode::Wizard(wizard) = &mut self.mode else { return Effect::Handled };
                    if wizard.choose(index) { self.handle(KeyEvent::from(KeyCode::Enter), terminal) } else { Effect::Handled }
                }
                (Some(Hit::Key(key)), Mode::Wizard(_)) => self.handle(key, terminal),
                (Some(Hit::Choice(index)), Mode::Options(_)) => {
                    self.mode = Mode::Normal;
                    match self.current().and_then(|(routine, state)| self.choices(&routine, &state).get(index).map(|choice| choice.act)) {
                        Some(act) => self.act(act),
                        None => Effect::Handled,
                    }
                }
                (None, Mode::Options(_) | Mode::Message(_) | Mode::Help(_)) => {
                    self.mode = Mode::Normal;
                    Effect::Handled
                }
                (_, Mode::Normal) => Effect::Unhandled,
                _ => Effect::Handled,
            },
            _ => Effect::Handled,
        }
    }

    fn next_up(&self, now: u64, offset: i64) -> Option<(String, u64)> {
        self.rows.iter().filter(|(routine, state)| routine.enabled && self.open_task(state).is_none())
            .map(|(routine, state)| (routine.id.clone(), routines::next_run(routine.schedule, state, now, offset))).min_by_key(|(_, at)| *at)
    }

    fn row_line(&self, routine: &Routine, state: &State, selected: bool, columns: &Columns) -> Line<'static> {
        let (now, offset) = self.now();
        let Columns { width, narrow, name: name_width, schedule: schedule_width } = *columns;
        let look = self.look_of(routine, state);
        let (symbol, color) = mark(look);
        let (status, status_color) = match look {
            Look::Paused => ("paused".to_string(), MUTED),
            Look::Waiting => ("findings".to_string(), PEACH),
            Look::Running if narrow => ("running".to_string(), BLUE),
            Look::Running => (format!("running {}", crate::keeper::span(now.saturating_sub(state.last_start.unwrap_or(now)))), BLUE),
            Look::Failed => ("failed".to_string(), RED),
            _ => match routines::next_run(routine.schedule, state, now, offset) {
                at if at <= now => ("due now".to_string(), MUTED),
                at => (format!("next {}", next_when(at, now, offset)), MUTED),
            },
        };
        let paused = look == Look::Paused;
        let name = cut(&routine.title, name_width);
        let bar = if selected { Span::styled("▌", style(MAUVE)) } else { Span::raw(" ") };
        let name_style = match (paused, selected) {
            (true, _) => style(MUTED),
            (false, true) => bold(TEXT),
            (false, false) => style(TEXT),
        };
        let schedule = if paused { String::new() } else { schedule_short(routine.schedule, narrow) };
        let mut spans = vec![bar, Span::styled(symbol, style(color)), Span::raw(" "), Span::styled(format!("{name:<name_width$}  "), name_style)];
        if schedule_width > 0 {
            spans.push(Span::styled(format!("{schedule:<schedule_width$}  "), style(MUTED)));
        }
        let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
        spans.push(Span::styled(cut(&status, width.saturating_sub(used)), style(status_color)));
        let line = Line::from(spans);
        if selected { line.style(Style::new().bg(SURFACE)) } else { line }
    }

    fn list_lines(&self, width: usize, narrow: bool) -> (Vec<Line<'static>>, usize, Vec<(usize, usize, Hit)>) {
        let now = self.now();
        let right = match self.next_up(now.0, now.1) {
            Some((id, at)) if at > now.0 => format!("{} · next: {id} {}", self.rows.len(), next_when(at, now.0, now.1)),
            _ => self.rows.len().to_string(),
        };
        let mut lines = column_title("ROUTINES", right, !self.narrow_detail(), width);
        let order = self.ordered();
        let position = self.position();
        let longest = self.rows.iter().map(|(routine, _)| routine.title.chars().count()).max().unwrap_or(0);
        let schedule_width = self.rows.iter().map(|(routine, _)| schedule_short(routine.schedule, narrow).chars().count()).max().unwrap_or(0);
        let status_room = if narrow { 12 } else { 14 };
        let mut name_width = longest.min(width.saturating_sub(5 + schedule_width + 2 + status_room));
        let schedule_width = if name_width < 8 { name_width = longest.min(width.saturating_sub(5 + status_room)).max(4); 0 } else { schedule_width };
        let (mut selected_line, mut rows) = (0, Vec::new());
        for (place, index) in order.iter().enumerate() {
            let (routine, state) = &self.rows[*index];
            if place == position {
                selected_line = lines.len();
            }
            rows.push((lines.len(), lines.len() + 1, Hit::Row(place)));
            lines.push(self.row_line(routine, state, place == position, &Columns { width, narrow, name: name_width, schedule: schedule_width }));
        }
        for error in &self.errors {
            lines.push(Line::default());
            lines.extend(wrap(&format!("ignored: {error}"), width, 3).into_iter().map(|line| Line::from(Span::styled(line, style(RED)))));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(format!("  {} new routine", keys::key(Act::New)), style(MUTED))));
        (lines, selected_line, rows)
    }

    fn detail_lines(&self, routine: &Routine, state: &State, width: usize) -> Vec<Line<'static>> {
        let label = routine_wizard::mode_label(routine.mode, routine.push);
        let paused = if routine.enabled { "" } else { " · paused" };
        let mut lines = vec![Line::from(vec![Span::styled(routine.title.clone(), bold(TEXT)), Span::styled(format!("  · {label} · {}{paused}", repo_name(&routine.repo)), style(MUTED))]), Line::default()];
        lines.push(Line::from(Span::styled("WHAT IT DOES", style(MUTED))));
        lines.extend(wrap(&routine.prompt, width.saturating_sub(2), 8).into_iter().map(|text| Line::from(Span::styled(format!("  {text}"), style(TEXT)))));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("MAY", style(MUTED))));
        lines.push(Line::from(vec![Span::styled(format!("  {label}"), style(GREEN)), Span::styled(format!("  {}", may(routine)), style(TEXT))]));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("RUNS", style(MUTED))));
        lines.extend(runs_lines(routine, state, self.now(), width));
        lines
    }

    pub fn draw(&mut self, frame: &mut Frame, body: Rect) {
        self.hits.clear();
        self.width = body.width;
        if !self.loaded {
            frame.render_widget(Paragraph::new(Span::styled("reading the routines…", style(MUTED))), body);
        } else if self.rows.is_empty() {
            let (lines, selected, rows) = self.empty_lines(body.width as usize);
            render_column(frame, body, lines, selected, rows, &mut self.hits);
        } else if body.width >= WIDE {
            let [list_area, _, detail_area] = Layout::horizontal([Constraint::Percentage(44), Constraint::Length(4), Constraint::Min(30)]).areas(body);
            let (lines, selected, rows) = self.list_lines(list_area.width as usize, false);
            render_column(frame, list_area, lines, selected, rows, &mut self.hits);
            if let Some((routine, state)) = self.current() {
                let lines = self.detail_lines(&routine, &state, detail_area.width as usize);
                render_column::<Hit>(frame, detail_area, lines, 0, Vec::new(), &mut self.hits);
            }
        } else if let (Focus::Detail, Some((routine, state))) = (self.focus, self.current()) {
            let lines = self.detail_lines(&routine, &state, body.width as usize);
            render_column::<Hit>(frame, body, lines, 0, Vec::new(), &mut self.hits);
        } else {
            let (lines, selected, rows) = self.list_lines(body.width as usize, true);
            render_column(frame, body, lines, selected, rows, &mut self.hits);
        }
        self.draw_mode(frame);
    }

    fn empty_lines(&self, width: usize) -> (Vec<Line<'static>>, usize, Vec<(usize, usize, Hit)>) {
        let mut lines = column_title("ROUTINES", String::new(), true, width);
        lines.push(Line::default());
        let about = "A routine is a task kelpie starts on a schedule. It checks something, then reports, proposes a plan or fixes it. A run that finds nothing closes itself; one that finds something lands in NEEDS YOU.";
        lines.extend(wrap(about, width.saturating_sub(2).min(72), 6).into_iter().map(|text| Line::from(Span::styled(format!("  {text}"), style(TEXT)))));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("  START FROM", style(MUTED))));
        let (mut selected_line, mut rows) = (0, Vec::new());
        for (index, template) in TEMPLATES.iter().enumerate() {
            let selected = index == self.template;
            if selected {
                selected_line = lines.len();
            }
            let (mode, _) = routine_wizard::mode_of(template.mode);
            let schedule = routine_file::parse_when(template.when).map(|schedule| schedule_short(schedule, false)).unwrap_or_default();
            let facts = format!("{} · {schedule}", mode.key());
            let bar = if selected { Span::styled("▌", style(MAUVE)) } else { Span::raw(" ") };
            let line = Line::from(vec![
                bar, Span::raw(" "), Span::styled(format!("{:<16}", template.title), if selected { bold(TEXT) } else { style(TEXT) }),
                Span::styled(format!("{facts:<22}"), style(MUTED)), Span::styled(cut(template.about, width.saturating_sub(40)), style(TEXT)),
            ]);
            rows.push((lines.len(), lines.len() + 1, Hit::Template(index)));
            lines.push(if selected { line.style(Style::new().bg(SURFACE)) } else { line });
        }
        for error in &self.errors {
            lines.push(Line::default());
            lines.extend(wrap(&format!("ignored: {error}"), width, 3).into_iter().map(|line| Line::from(Span::styled(line, style(RED)))));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(format!("  ⏎ start from this · {} start blank", keys::key(Act::New)), style(MUTED))));
        (lines, selected_line, rows)
    }

    fn draw_mode(&mut self, frame: &mut Frame) {
        let mut modal_hits = Vec::new();
        match &self.mode {
            Mode::Normal => return,
            Mode::Options(index) => {
                let Some((routine, state)) = self.current() else { return };
                let choices = self.choices(&routine, &state);
                let (menu, at) = menu_lines(&choices, *index, 58, |_| false, |act| if matches!(act, None | Some(Act::Card)) { "LAST RUN" } else { "ROUTINE" });
                let facts = format!("{} · {} · {}", routine_wizard::mode_label(routine.mode, routine.push), schedule_short(routine.schedule, false), repo_name(&routine.repo));
                let mut lines = vec![Line::from(Span::styled(facts, style(MUTED)))];
                lines.extend(menu);
                let area = modal(frame, cut(&routine.title, 54), lines, 62);
                for (choice, line) in at.into_iter().enumerate() {
                    let y = area.y + 2 + line as u16;
                    if y < area.bottom().saturating_sub(1) {
                        modal_hits.push((Rect::new(area.x, y, area.width, 1), Hit::Choice(choice)));
                    }
                }
            }
            Mode::Wizard(wizard) => {
                let (now, offset) = self.now();
                let width = 64.min(frame.area().width.saturating_sub(8) as usize);
                let (lines, spots) = wizard.view(now, offset, width);
                let rows: Vec<u16> = lines.iter().map(|line| Paragraph::new(line.clone()).wrap(Wrap { trim: false }).line_count(width as u16) as u16).collect();
                let area = modal(frame, wizard.title(), lines, width as u16 + 4);
                for (line, spot) in spots {
                    let y = area.y + 1 + rows[..line].iter().sum::<u16>();
                    if y >= area.bottom().saturating_sub(1) {
                        continue;
                    }
                    match spot {
                        Spot::Choice(index) => modal_hits.push((Rect::new(area.x, y, area.width, rows[line]), Hit::Choice(index))),
                        Spot::Key { at, width: hint, key } if at < width => modal_hits.push((Rect::new(area.x + 2 + at as u16, y, hint.min(width - at) as u16, 1), Hit::Key(key))),
                        Spot::Key { .. } => {}
                    }
                }
            }
            Mode::ConfirmDelete { id, nudged } => {
                let found = self.rows.iter().find(|(routine, _)| &routine.id == id);
                let title = found.map_or(id.clone(), |(routine, _)| routine.title.clone());
                let file = found.map(|(routine, _)| crate::area_page::home_relative(&routine_file::file_of(routine))).unwrap_or_default();
                let mut lines = vec![
                    Line::from(Span::styled(title, style(TEXT))), Line::from(Span::styled(format!("Its [[routines]] table goes from {file}; a run's task that is open stays."), style(MUTED))),
                    Line::default(), Line::from(vec![Span::styled("Delete this routine?", style(YELLOW)), Span::styled("  y / n", style(MUTED))]),
                ];
                if *nudged {
                    lines.push(Line::from(Span::styled("press y or n", style(PEACH))));
                }
                modal(frame, "delete".to_string(), lines, 72);
            }
            Mode::Working { label, .. } => {
                modal(frame, "kelpie".to_string(), vec![Line::from(Span::styled(label.clone(), style(TEXT)))], 72);
            }
            Mode::Message(text) => {
                let mut lines: Vec<Line> = text.lines().map(|line| Line::from(Span::styled(line.to_string(), style(TEXT)))).collect();
                lines.push(Line::from(Span::styled("any key to close", style(MUTED))));
                modal(frame, "kelpie".to_string(), lines, 96);
            }
            Mode::Help(scroll) => {
                let mut scroll = *scroll;
                keys_window::draw(frame, keys::map(), Ctx::Routines, false, None, &mut scroll);
                self.mode = Mode::Help(scroll);
            }
        }
        self.hits = modal_hits;
    }

    pub fn hint_lines() -> (Line<'static>, Line<'static>) {
        let map = keys::map();
        let line = |hints: &[(Act, &str)]| Line::from(Span::styled(hints.iter().map(|(act, word)| format!("{} {word}", map.label(*act))).collect::<Vec<_>>().join(" · "), style(MUTED)));
        (
            line(&[(Act::Options, "options"), (Act::New, "new"), (Act::Go, "run now"), (Act::Edit, "edit"), (Act::Pause, "pause"), (Act::Card, "last report"), (Act::Help, "keys")]),
            line(&[(Act::Options, "open"), (Act::New, "new"), (Act::Go, "run"), (Act::Help, "keys")]),
        )
    }
}

const WIDE: u16 = 86;

#[derive(Clone, Copy)]
struct Columns {
    width: usize,
    narrow: bool,
    name: usize,
    schedule: usize,
}

fn word(outcome: Outcome, start: u64, now: u64) -> (String, Color) {
    match outcome {
        Outcome::Ok => ("ok".to_string(), MUTED),
        Outcome::Findings => ("findings".to_string(), PEACH),
        Outcome::Failed => ("failed".to_string(), RED),
        Outcome::Running => (format!("running {}", crate::keeper::span(now.saturating_sub(start))), BLUE),
    }
}

fn runs_lines(routine: &Routine, state: &State, (now, offset): (u64, i64), width: usize) -> Vec<Line<'static>> {
    let mut runs: Vec<(u64, Outcome, Option<String>)> = Vec::new();
    if let (Some(start), Some(outcome)) = (state.last_start, state.last_outcome) {
        runs.push((start, outcome, state.last_summary.clone()));
    }
    runs.extend(state.earlier.iter().map(|past| (past.start, past.outcome, past.summary.clone())));
    if runs.is_empty() {
        let next = if routine.enabled { format!(" · next {}", first_when(routines::next_run(routine.schedule, state, now, offset), now, offset)) } else { String::new() };
        return vec![Line::from(Span::styled(format!("  not run yet{next}"), style(MUTED)))];
    }
    runs.into_iter().map(|(start, outcome, summary)| {
        let (symbol, color) = outcome_mark(outcome);
        let (said, said_color) = word(outcome, start, now);
        let head = format!("  {symbol} {:<14}{said:<12}", past_when(start, now, offset));
        let room = width.saturating_sub(head.chars().count());
        Line::from(vec![
            Span::raw("  "), Span::styled(symbol, style(color)), Span::raw(" "), Span::styled(format!("{:<14}", past_when(start, now, offset)), style(TEXT)),
            Span::styled(format!("{said:<12}"), style(said_color)), Span::styled(cut(summary.as_deref().unwrap_or_default(), room), style(TEXT)),
        ])
    }).collect()
}

fn report_text(routine: &Routine, state: &State, (now, offset): (u64, i64)) -> String {
    match (state.last_start, state.last_outcome) {
        (Some(start), Some(outcome)) => {
            let summary = state.last_summary.as_deref().map(|summary| format!("\n{summary}")).unwrap_or_default();
            format!("{}, {}: {}{summary}", routine.title, past_when(start, now, offset), word(outcome, start, now).0)
        }
        _ => format!("{} has not run yet.", routine.title),
    }
}

fn edit_in_editor(terminal: &mut DefaultTerminal, text: &str) -> Result<Option<String>> {
    let file = std::env::temp_dir().join(format!("kelpie-{}-routine-prompt.md", std::process::id()));
    std::fs::write(&file, text).map_err(|error| error.to_string())?;
    let status = crate::board::terminal::outside(terminal, || std::process::Command::new("sh").args(["-c", "${VISUAL:-${EDITOR:-vi}} \"$1\"", "sh"]).arg(&file).status().map_err(|error| error.to_string()));
    let edited = std::fs::read_to_string(&file).map_err(|error| error.to_string());
    let _ = std::fs::remove_file(&file);
    if !status?.success() {
        return Ok(None);
    }
    let text = edited?.trim().to_string();
    Ok((!text.is_empty()).then_some(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routines::Past;

    const MONDAY_0630_UTC: u64 = 1_792_391_400;

    fn routine(id: &str, title: &str, schedule: Schedule, mode: routines::Mode) -> Routine {
        Routine {
            id: id.to_string(), title: title.to_string(), repo: "~/homelab-gitops".to_string(), schedule, prompt: format!("Check {title}."), mode, push: false, enabled: true, profile: None,
            source: "/c/config.toml".to_string(),
        }
    }

    fn ran(outcome: Outcome, start: u64, task: &str, summary: &str) -> State {
        State { seen: Some(1), last_start: Some(start), last_task: Some(task.to_string()), last_outcome: Some(outcome), last_summary: Some(summary.to_string()), earlier: Vec::new() }
    }

    fn open_task(name: &str) -> Item {
        let mut item = crate::status::tests::task(Some("idle"), Some("needs-you"), 0);
        item.name = name.to_string();
        item.place = "homelab-gitops".to_string();
        item
    }

    fn page_with_rows() -> Page {
        let mut page = Page::new(PathBuf::from("/c/config.toml"), PathBuf::from("/c/repos"));
        page.clock = Some((MONDAY_0630_UTC, 0));
        let mut health = ran(Outcome::Findings, MONDAY_0630_UTC - 1800, "cluster-health-20261019-0600@homelab-gitops", "Longhorn volume degraded, cert in 5 d");
        health.earlier = vec![
            Past { start: MONDAY_0630_UTC - 86_400 - 1800, outcome: Outcome::Ok, summary: Some("all well".to_string()) },
            Past { start: MONDAY_0630_UTC - 2 * 86_400 - 1800, outcome: Outcome::Failed, summary: Some("kubectl: connection refused".to_string()) },
        ];
        let mut disk = routine("disk", "Disk on the devbox", Schedule::Daily(480), routines::Mode::Report);
        disk.enabled = false;
        page.rows = vec![
            (routine("backups", "Backups", Schedule::Every(6 * 3600), routines::Mode::Report), ran(Outcome::Ok, MONDAY_0630_UTC - 3600, "backups-x@homelab-gitops", "all copied")),
            (disk, State::default()),
            (routine("app-updates", "App updates", Schedule::Daily(425), routines::Mode::Fix), ran(Outcome::Running, MONDAY_0630_UTC - 180, "app-updates-20261019-0627@homelab-gitops", "")),
            (routine("cluster-health", "Cluster health", Schedule::Daily(420), routines::Mode::Report), health),
            (routine("certificates", "Certificates", Schedule::Weekly(0, 480), routines::Mode::Plan), State { seen: Some(MONDAY_0630_UTC - 60), ..State::default() }),
        ];
        page.tasks = vec![open_task("cluster-health-20261019-0600"), open_task("app-updates-20261019-0627")];
        page.loaded = true;
        page
    }

    fn render(page: &mut Page, width: u16, height: u16) -> Vec<String> {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| page.draw(frame, frame.area())).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string()).collect()
    }

    fn press(page: &mut Page, code: KeyCode) -> Effect {
        page.handle(KeyEvent::from(code), None)
    }

    #[test]
    fn the_list_marks_each_last_run_puts_the_one_that_needs_you_first_and_shows_its_detail() {
        let mut page = page_with_rows();
        assert_eq!(page.waiting(), 1, "only findings in an open task wait for the user");
        let rows = render(&mut page, 130, 24);
        let screen = rows.join("\n");
        assert!(rows[0].starts_with("ROUTINES") && rows[0].contains("5 · next: certificates 08:00"), "{screen}");
        let want = [
            "▌⚠ Cluster health      daily 07:00  findings",
            " ◐ App updates         daily 07:05  running 3m",
            " ✓ Backups             every 6h     next 11:30",
            " · Certificates        mon 08:00    next 08:00",
            " ‖ Disk on the devbox               paused",
        ];
        for (row, want) in rows[2..7].iter().zip(want) {
            assert!(row.starts_with(want), "{want:?} is not the row {row:?}:\n{screen}");
        }
        assert!(screen.contains("n new routine"));
        for line in ["Cluster health  · report · homelab-gitops", "WHAT IT DOES", "  Check Cluster health.", "MAY", "  report  reads and reports, changes nothing", "RUNS",
            "  ⚠ today 06:00   findings    Longhorn volume degraded, cert in 5 d", "  ✓ yesterday     ok          all well", "  ✗ sat 06:00     failed      kubectl: connection refused"] {
            assert!(screen.contains(line), "{line:?} is not on the page:\n{screen}");
        }
        press(&mut page, KeyCode::Down);
        let screen = render(&mut page, 130, 24).join("\n");
        assert!(screen.contains("▌◐ App updates") && screen.contains("App updates  · fix · homelab-gitops") && screen.contains("◐ today 06:27   running 3m"), "{screen}");
        page.tasks.clear();
        assert_eq!(page.waiting(), 0, "once its task is closed the findings no longer wait");
    }

    #[test]
    fn a_narrow_board_shows_the_list_alone_and_enter_opens_the_routine_on_the_whole_width() {
        let mut page = page_with_rows();
        let rows = render(&mut page, 60, 24);
        let screen = rows.join("\n");
        assert!(screen.contains("▌⚠ Cluster health      07:00  findings") && screen.contains(" ◐ App updates         07:05  running") && screen.contains(" ✓ Backups             6h     next 11:30") && screen.contains(" · Certificates        mon    next 08:00"), "{screen}");
        assert!(!screen.contains("WHAT IT DOES"));
        page.width = 60;
        press(&mut page, KeyCode::Enter);
        let screen = render(&mut page, 60, 24).join("\n");
        assert!(screen.contains("WHAT IT DOES") && !screen.contains("ROUTINES"), "{screen}");
        assert!(page.is_normal(), "⏎ on the narrow list opens the routine, not the menu");
        press(&mut page, KeyCode::Enter);
        assert!(matches!(page.mode, Mode::Options(0)), "⏎ on the open routine is its menu");
        press(&mut page, KeyCode::Esc);
        press(&mut page, KeyCode::Esc);
        assert!(render(&mut page, 60, 24).join("\n").contains("ROUTINES"), "esc goes back to the list");
        assert!(matches!(press(&mut page, KeyCode::Esc), Effect::Unhandled), "and at the list esc is the board's");
    }

    #[test]
    fn enter_opens_the_menu_of_the_mock_and_its_first_choice_goes_to_the_runs_task() {
        let mut page = page_with_rows();
        press(&mut page, KeyCode::Enter);
        let screen = render(&mut page, 130, 30).join("\n");
        let at = |text: &str| screen.find(text).unwrap_or_else(|| panic!("{text:?} is not in the menu:\n{screen}"));
        assert!(at("report · daily 07:00 · homelab-gitops") < at("LAST RUN") && at("LAST RUN") < at("Go to its task") && at("Go to its task") < at("Show the report") && at("Show the report") < at("│ ROUTINE ")
            && at("│ ROUTINE ") < at("Run now") && at("Run now") < at("Edit") && at("Edit") < at("Pause") && at("Pause") < at("Delete"), "{screen}");
        assert!(screen.contains("↑ ↓ choose · ⏎ run · a letter runs it directly · esc close"));
        let keys: String = page.choices(&page.rows[3].0, &page.rows[3].1).iter().map(|choice| choice.key).collect();
        assert_eq!(keys, "⏎sgepD");
        assert!(matches!(press(&mut page, KeyCode::Enter), Effect::Task(address) if address == "cluster-health-20261019-0600@homelab-gitops"));
        page.selected = Some("disk".to_string());
        let (disk, state) = page.current().unwrap();
        assert_eq!(page.choices(&disk, &state).iter().map(|choice| (choice.key, choice.label.as_str())).collect::<Vec<_>>(), [('g', "Run now"), ('e', "Edit"), ('p', "Resume"), ('D', "Delete")], "never run and paused");
        press(&mut page, KeyCode::Char('s'));
        assert!(matches!(&page.mode, Mode::Message(text) if text == "Disk on the devbox has not run yet."));
    }

    #[test]
    fn the_empty_page_explains_itself_and_starts_from_a_template_or_blank() {
        let mut page = Page::new(PathBuf::from("/c/config.toml"), PathBuf::from("/c/repos"));
        page.loaded = true;
        let screen = render(&mut page, 120, 20).join("\n");
        assert!(screen.contains("A routine is a task kelpie starts on a schedule.") && screen.contains("START FROM"), "{screen}");
        assert!(screen.contains("▌ Health check    report · daily 07:00  look over a repo's service and say what's wrong"), "{screen}");
        assert!(screen.contains("  Dependency bump plan · mon 08:00      list updates and propose which to take"), "{screen}");
        assert!(screen.contains("  Keep CI green   fix · every 6h        fix a red main and hand it to the lead to push"), "{screen}");
        assert!(screen.contains("⏎ start from this · n start blank"));
        press(&mut page, KeyCode::Down);
        press(&mut page, KeyCode::Down);
        press(&mut page, KeyCode::Enter);
        let Mode::Wizard(wizard) = &page.mode else { panic!("⏎ starts from the template") };
        assert_eq!((wizard.title.as_str(), wizard.when.as_str(), wizard.mode), ("Keep CI green", "every 6h", 3));
        press(&mut page, KeyCode::Esc);
        press(&mut page, KeyCode::Char('n'));
        let Mode::Wizard(wizard) = &page.mode else { panic!("n starts blank") };
        assert!(wizard.prompt.is_empty() && wizard.when == "07:00");
    }

    fn typed(page: &mut Page, text: &str) {
        for character in text.chars() {
            press(page, KeyCode::Char(character));
        }
    }

    #[test]
    fn the_popup_pause_edit_and_delete_write_config_toml_and_the_page_follows() {
        let dir = crate::sh::test_dir("routine-page");
        let (machine, repos) = (dir.join("config.toml"), dir.join("repos"));
        std::fs::write(&machine, "[keys]\ninbox = \"I\"\n").unwrap();
        let mut page = Page::new(machine.clone(), repos.clone());
        page.clock = Some((MONDAY_0630_UTC, 0));
        page.reload();
        assert!(page.rows.is_empty());
        press(&mut page, KeyCode::Char('n'));
        typed(&mut page, "Check every node.");
        press(&mut page, KeyCode::Enter);
        typed(&mut page, "~/homelab-gitops-for-this-test");
        press(&mut page, KeyCode::Enter);
        press(&mut page, KeyCode::Enter);
        press(&mut page, KeyCode::Enter);
        if let Mode::Wizard(wizard) = &mut page.mode {
            wizard.title = "Node check".to_string();
        }
        press(&mut page, KeyCode::Enter);
        assert!(render(&mut page, 120, 30).join("\n").contains("y add · g add and run now"));
        press(&mut page, KeyCode::Char('y'));
        assert!(matches!(&page.mode, Mode::Message(text) if text.starts_with("Added the routine node-check")), "nothing is written before y");
        let text = std::fs::read_to_string(&machine).unwrap();
        assert!(text.starts_with("[keys]\ninbox = \"I\"\n\n[[routines]]\nid = \"node-check\"\ntitle = \"Node check\"\nrepo = \"~/homelab-gitops-for-this-test\"\nat = \"07:00\"\nmode = \"report\"\nprompt = \"\"\"Check every node.\"\"\"\n"), "{text}");
        assert_eq!(page.rows.len(), 1, "the page reads the file again at once");
        press(&mut page, KeyCode::Esc);
        press(&mut page, KeyCode::Char('p'));
        assert!(std::fs::read_to_string(&machine).unwrap().contains("enabled = false") && !page.rows[0].0.enabled);
        press(&mut page, KeyCode::Esc);
        assert!(render(&mut page, 120, 30).join("\n").contains("‖ Node check"));
        press(&mut page, KeyCode::Char('p'));
        press(&mut page, KeyCode::Esc);
        assert!(page.rows[0].0.enabled);
        press(&mut page, KeyCode::Char('e'));
        let Mode::Wizard(wizard) = &mut page.mode else { panic!("e opens the steps") };
        assert_eq!(wizard.prompt, "Check every node.");
        wizard.when = "mon 08:00".to_string();
        wizard.step = routine_wizard::STEPS;
        press(&mut page, KeyCode::Char('y'));
        assert_eq!(page.rows[0].0.schedule, Schedule::Weekly(0, 480));
        press(&mut page, KeyCode::Esc);
        press(&mut page, KeyCode::Char('D'));
        press(&mut page, KeyCode::Char('x'));
        assert!(render(&mut page, 120, 30).join("\n").contains("Delete this routine?") && matches!(page.mode, Mode::ConfirmDelete { nudged: true, .. }));
        press(&mut page, KeyCode::Char('y'));
        assert!(page.rows.is_empty());
        assert_eq!(std::fs::read_to_string(&machine).unwrap(), "[keys]\ninbox = \"I\"\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn spot(page: &mut Page, text: &str) -> (u16, u16) {
        let rows = render(page, 120, 40);
        let (y, row) = rows.iter().enumerate().find(|(_, row)| row.contains(text)).unwrap_or_else(|| panic!("{text:?} is not on the screen:\n{}", rows.join("\n")));
        (row[..row.find(text).unwrap()].chars().count() as u16, y as u16)
    }

    fn mouse(page: &mut Page, kind: MouseEventKind, (column, row): (u16, u16)) -> Effect {
        page.mouse(MouseEvent { kind, column, row, modifiers: ratatui::crossterm::event::KeyModifiers::NONE }, None)
    }

    fn click_on(page: &mut Page, text: &str) -> Effect {
        let at = spot(page, text);
        mouse(page, MouseEventKind::Down(MouseButton::Left), at)
    }

    fn step(page: &Page) -> usize {
        let Mode::Wizard(wizard) = &page.mode else { panic!("the steps are closed") };
        wizard.step
    }

    #[test]
    fn the_steps_take_clicks_on_choices_and_hints_and_the_wheel_moves_through_a_list() {
        let dir = crate::sh::test_dir("routine-page-mouse");
        let machine = dir.join("config.toml");
        let mut page = Page::new(machine.clone(), dir.join("repos"));
        page.clock = Some((MONDAY_0630_UTC, 0));
        page.loaded = true;
        press(&mut page, KeyCode::Char('n'));
        click_on(&mut page, "esc cancel");
        assert!(page.is_normal(), "a click on esc cancel closes the steps");
        press(&mut page, KeyCode::Char('n'));
        let Mode::Wizard(wizard) = &mut page.mode else { panic!("n opens the steps") };
        wizard.repos = ["homelab-gitops", "greenhouse", "kelpie"].iter().map(|name| Repo { value: format!("~/{name}"), name: name.to_string(), shown: format!("~/{name}") }).collect();
        typed(&mut page, "Check every node.");
        click_on(&mut page, "⏎ next");
        assert_eq!(step(&page), 1, "a click on ⏎ next is ⏎");
        click_on(&mut page, "greenhouse");
        let Mode::Wizard(wizard) = &page.mode else { panic!() };
        assert_eq!((wizard.step, wizard.repo().as_deref()), (1, Some("~/greenhouse")), "a click selects the repo");
        assert!(render(&mut page, 120, 40).join("\n").contains("▌greenhouse"));
        let list = spot(&mut page, "kelpie");
        mouse(&mut page, MouseEventKind::ScrollDown, list);
        let Mode::Wizard(wizard) = &page.mode else { panic!() };
        assert_eq!(wizard.repo().as_deref(), Some("~/kelpie"), "the wheel moves down the list");
        mouse(&mut page, MouseEventKind::ScrollUp, list);
        click_on(&mut page, "greenhouse");
        assert_eq!(step(&page), 2, "a click on the selected repo goes on");
        click_on(&mut page, "⏎ next");
        click_on(&mut page, "Fix and push");
        let Mode::Wizard(wizard) = &page.mode else { panic!() };
        assert_eq!((wizard.step, wizard.mode), (3, 3), "a click selects what it may do");
        click_on(&mut page, "Fix and push");
        assert_eq!(step(&page), 4);
        click_on(&mut page, "⏎ next");
        assert_eq!(step(&page), routine_wizard::STEPS);
        click_on(&mut page, "← back");
        assert_eq!(step(&page), 4, "a click on ← back goes back");
        click_on(&mut page, "⏎ next");
        let at = spot(&mut page, "g add and run now");
        assert!(matches!(page.hits.iter().find(|(area, _)| area.contains(Position::new(at.0 + 3, at.1))), Some((_, Hit::Key(key))) if *key == KeyEvent::from(KeyCode::Char('g'))), "g add and run now is g");
        click_on(&mut page, "y add");
        assert!(matches!(&page.mode, Mode::Message(text) if text.starts_with("Added the routine check-every-node")), "a click on y add writes it");
        let text = std::fs::read_to_string(&machine).unwrap();
        assert!(text.contains("repo = \"~/greenhouse\"\nat = \"07:00\"\nmode = \"fix\"\npush = true"), "{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn times_read_as_today_yesterday_a_weekday_or_a_date() {
        let now = MONDAY_0630_UTC;
        assert_eq!(past_when(now - 1800, now, 0), "today 06:00");
        assert_eq!(past_when(now - 86_400, now, 0), "yesterday");
        assert_eq!(past_when(now - 2 * 86_400, now, 0), "sat 06:30");
        assert_eq!(past_when(now - 30 * 86_400, now, 0), "2026-09-19 06:30");
        assert_eq!(next_when(now + 3600, now, 0), "07:30");
        assert_eq!(next_when(now + 86_400, now, 0), "tomorrow");
        assert_eq!(next_when(now + 3 * 86_400, now, 0), "thu");
        assert_eq!(first_when(now + 86_400, now, 7200), "tomorrow 08:30");
        assert_eq!(schedule_words(Schedule::Weekly(0, 480)), "every monday at 08:00");
        assert_eq!(schedule_words(Schedule::Daily(420)), "every day at 07:00");
        let (full, short) = Page::hint_lines();
        assert_eq!(full.to_string(), "⏎ options · n new · g run now · e edit · p pause · s last report · ? keys");
        assert_eq!(short.to_string(), "⏎ open · n new · g run · ? keys");
        for (hint, _) in [(&full, ()), (&short, ())] {
            for part in hint.to_string().split(" · ") {
                let key = part.split(' ').next().unwrap();
                let code = if key == "⏎" { KeyCode::Enter } else { KeyCode::Char(key.chars().next().unwrap()) };
                assert!(keys::map().act(Ctx::Routines, &KeyEvent::from(code)).is_some(), "the hint {part} is a key the page acts on");
            }
        }
    }
}
