use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::actions::{self, Action, Answers, Step, Work};
use crate::areas::{self, Area, Areas};
use crate::backlog;
use crate::board::terminal;
use crate::board::widgets::{
    self, BLUE, Choice, FAINT, GREEN, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, TEAL, TEXT, YELLOW, bold, card_text, chips, choice, column_title,
    group_line, modal, options_lines, plural, render_column, row, style, wrap,
};
use crate::config::Source;
use crate::herdr;
use crate::keys::{self, Act, Ctx};
use crate::permit;
use crate::repo::{self, git, push_as_user};
use crate::sh::Result;
use crate::status::{Group, Item};
use crate::tasks;

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    List,
    Detail,
}

#[derive(Clone, Copy)]
pub enum Hit {
    Area(usize),
    Action(usize),
    Choice(usize),
}

// What the board does after the page handled an event.
pub enum Effect {
    Handled,
    Unhandled,
    Left,
}

#[derive(Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
enum Standing {
    NeedsYou,
    Active,
    Quiet,
}

impl Standing {
    fn title(self) -> &'static str {
        match self {
            Standing::NeedsYou => "NEEDS YOU",
            Standing::Active => "ACTIVE",
            Standing::Quiet => "QUIET",
        }
    }

    fn color(self) -> Color {
        match self {
            Standing::NeedsYou => PEACH,
            Standing::Active => BLUE,
            Standing::Quiet => MUTED,
        }
    }
}

// An action's next step from `then`, offered once it has finished: repo:action and the answers it carries over.
struct Next {
    target: String,
    answers: Answers,
}

enum Purpose {
    TaskName(String),
    TaskPrompt(String, String),
    Answer { stem: String, action: Box<Action>, answers: Answers, carried: Answers },
    ActionTaskName { stem: String, prompt: String },
}

enum Go {
    Script { repo: PathBuf, steps: Vec<Step>, next: Option<Next> },
    Task { repo: PathBuf, name: String, prompt: String },
}

struct PushWindow {
    repo: PathBuf,
    branch: String,
    tag: Option<String>,
    commits: Vec<String>,
    done: Vec<String>,
    next: Option<Next>,
}

enum After {
    Show(Result<String>, Option<Next>),
    Push(PushWindow),
}

enum Mode {
    Normal,
    Options(usize),
    Input { purpose: Purpose, label: String, buffer: String },
    Confirm { title: String, lines: Vec<Line<'static>>, question: String, go: Go, nudged: bool },
    Push(PushWindow),
    Working { label: String, result: Receiver<After> },
    Message { text: String, next: Option<Next> },
    Log { stem: String, scroll: usize },
    Help,
}

pub struct Page {
    areas: Vec<Area>,
    pub synced: Option<u64>,
    pub tasks: Vec<Item>,
    loaded: bool,
    error: Option<String>,
    selected: Option<String>,
    focus: Focus,
    action: usize,
    mode: Mode,
    updates: Option<(Receiver<Result<Areas>>, Sender<()>)>,
    pub hits: Vec<(Rect, Hit)>,
    sources: Vec<Source>,
}

fn spawn_refresher() -> (Receiver<Result<Areas>>, Sender<()>) {
    let (sender, receiver) = mpsc::channel();
    let (refresh, wakeup) = mpsc::channel::<()>();
    thread::spawn(move || {
        loop {
            if sender.send(areas::gather()).is_err() {
                return;
            }
            let _ = wakeup.recv_timeout(Duration::from_secs(15));
        }
    });
    (receiver, refresh)
}

fn lead_pane() -> Option<String> {
    herdr::agents().ok()?.into_iter().find(|agent| agent["name"] == "lead").and_then(|agent| agent["pane_id"].as_str().map(str::to_string))
}

fn tasks_in<'a>(tasks: &'a [Item], area: &Area) -> Vec<&'a Item> {
    tasks.iter().filter(|item| item.repo.is_some() && item.repo == area.repo).collect()
}

fn standing(area: &Area, tasks: &[Item]) -> Standing {
    let mine = tasks_in(tasks, area);
    if area.to_push() > 0 || area.unreleased() > 0 || mine.iter().any(|item| item.group() == Group::NeedsYou) {
        Standing::NeedsYou
    } else if !mine.is_empty() || area.git.as_ref().is_some_and(|git| git.changed > 0) {
        Standing::Active
    } else {
        Standing::Quiet
    }
}

fn icon(area: &Area, standing: Standing) -> Span<'static> {
    match standing {
        Standing::NeedsYou if area.to_push() > 0 || area.unreleased() > 0 => Span::styled("↑", style(PEACH)),
        Standing::NeedsYou => Span::styled("●", style(PEACH)),
        Standing::Active => Span::styled("◐", style(BLUE)),
        Standing::Quiet => Span::styled("·", style(FAINT)),
    }
}

fn row_facts(area: &Area, tasks: &[Item]) -> String {
    let mut parts = Vec::new();
    if area.unreleased() > 0 {
        parts.push(format!("{} unreleased", area.unreleased()));
    }
    if area.to_push() > 0 {
        parts.push(format!("{} to push", area.to_push()));
    }
    if let Some(git) = area.git.as_ref().filter(|git| git.changed > 0) {
        parts.push(format!("{} changed", plural(git.changed, "file")));
    }
    let running = tasks_in(tasks, area).len();
    if running > 0 {
        parts.push(plural(running, "task"));
    }
    if !area.open.is_empty() {
        parts.push(format!("{} open", area.open.len()));
    }
    if !area.backlog.is_empty() {
        parts.push(format!("{} backlog", area.backlog.len()));
    }
    if area.repo.is_none() {
        parts.push("no repo".to_string());
    }
    parts.into_iter().take(3).collect::<Vec<_>>().join(" · ")
}

fn home_relative(path: &std::path::Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match path.strip_prefix(&home) {
        Ok(rest) if !home.is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

fn commits_between(repo: &std::path::Path, range: &str) -> Vec<String> {
    git(repo, &["log", "--format=%h %s", range]).unwrap_or_default().lines().map(str::to_string).collect()
}

fn kv(label: &str, value: String, color: Color) -> Line<'static> {
    Line::from(vec![Span::styled(format!("{label:<9}"), style(MUTED)), Span::styled(value, style(color))])
}

// Paragraphs stay apart; each wraps on its own.
fn text_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (index, paragraph) in text.split("\n\n").enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        lines.extend(wrap(paragraph, width, 8).into_iter().map(|line| Line::from(Span::styled(line, style(TEXT)))));
    }
    lines
}

fn first_next(action: &Action, answers: &Answers) -> Option<Next> {
    action.then.first().map(|target| Next { target: target.clone(), answers: answers.clone() })
}

impl Page {
    pub fn new(sources: Vec<Source>) -> Self {
        Self {
            areas: Vec::new(), synced: None, tasks: Vec::new(), loaded: false, error: None, selected: None, focus: Focus::List, action: 0,
            mode: Mode::Normal, updates: None, hits: Vec::new(), sources,
        }
    }

    pub fn is_normal(&self) -> bool {
        matches!(self.mode, Mode::Normal)
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.mode, Mode::Normal | Mode::Message { .. } | Mode::Help | Mode::Log { .. })
    }

    pub fn summary(&self) -> String {
        let repos = self.areas.iter().filter(|area| area.repo.is_some()).count();
        format!("{} · {} with a repo", plural(self.areas.len(), "area"), repos)
    }

    pub fn repos(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.areas.iter().filter_map(|area| area.repo.clone())
    }

    pub fn refresh(&self) {
        if let Some((_, refresh)) = &self.updates {
            let _ = refresh.send(());
        }
    }

    // Starts gathering the first time the page shows, then picks up what the background thread found and what a running action did.
    pub fn tick(&mut self) {
        if self.updates.is_none() {
            self.updates = Some(spawn_refresher());
        }
        if let Some((updates, _)) = &self.updates {
            while let Ok(update) = updates.try_recv() {
                match update {
                    Ok(found) => {
                        self.areas = found.list;
                        self.synced = found.synced;
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error),
                }
                self.loaded = true;
            }
        }
        if let Mode::Working { result, .. } = &self.mode
            && let Ok(after) = result.try_recv()
        {
            self.mode = match after {
                After::Show(result, next) => Mode::Message { text: result.unwrap_or_else(|error| error), next },
                After::Push(window) => Mode::Push(window),
            };
            self.refresh();
        }
    }

    fn ordered(&self) -> Vec<(usize, Standing)> {
        let mut order: Vec<(usize, Standing)> = self.areas.iter().enumerate().map(|(index, area)| (index, standing(area, &self.tasks))).collect();
        order.sort_by(|one, other| (one.1, self.areas[one.0].repo.is_none()).cmp(&(other.1, self.areas[other.0].repo.is_none())));
        order
    }

    fn position(&self) -> usize {
        let order = self.ordered();
        self.selected.as_ref().and_then(|key| order.iter().position(|(index, _)| &self.areas[*index].key == key)).unwrap_or(0)
    }

    fn current(&self) -> Option<Area> {
        self.ordered().get(self.position()).map(|(index, _)| self.areas[*index].clone())
    }

    fn area(&self, stem: &str) -> Option<Area> {
        self.areas.iter().find(|area| area.stem == stem).cloned()
    }

    fn select(&mut self, position: usize) {
        let order = self.ordered();
        if let Some((index, _)) = order.get(position.min(order.len().saturating_sub(1))) {
            self.selected = Some(self.areas[*index].key.clone());
            self.action = 0;
        }
    }

    fn choices(&self, area: &Area) -> Vec<Choice> {
        let mut choices = Vec::new();
        if area.repo.is_some() {
            choices.push(choice(Act::Go, "Open its Space", "the main checkout's Space, opened if needed"));
            choices.push(choice(Act::New, "Start a task here", "a worktree, a Space and Claude, told the area's notes"));
            choices.push(if area.repo_name().is_some_and(|repo| permit::repo_permitted(&repo)) {
                choice(Act::Permit, "Take back the lead's push permission", "the lead pushes only tasks you gave their own permission")
            } else {
                choice(Act::Permit, "Let the lead push reviewed tasks", "the lead may push any task here once its reviewer passed its current code")
            });
        }
        let refresh = backlog::refresh_label(&self.sources);
        choices.push(choice(Act::AreaRefresh, "Refresh its backlog", &refresh));
        choices.push(choice(Act::Log, "Show its log", "the vault log, newest first"));
        if let Some(git) = area.git.as_ref().filter(|git| git.ahead > 0) {
            choices.push(Choice { key: keys::key(Act::Push), act: Some(Act::Push), label: format!("Push {}", git.branch), about: format!("{} on {} to push: shows them, asks first", plural(git.ahead, "commit"), git.branch) });
        }
        for action in &area.actions {
            choices.push(Choice { key: action.key, act: None, label: action.title.clone(), about: action.about.clone() });
        }
        choices
    }

    fn is_repo_action(area: &Area, key: char) -> bool {
        area.actions.iter().any(|action| action.key == key)
    }

    fn show(&mut self, result: Result<String>) {
        self.mode = Mode::Message { text: result.unwrap_or_else(|error| error), next: None };
        self.refresh();
    }

    pub fn handle(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) -> Effect {
        let map = keys::map();
        let act = map.act(Ctx::Areas, &key);
        let leaves = matches!(act, Some(Act::Back | Act::Close));
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => return self.handle_normal(key),
            Mode::Options(index) => {
                let Some(area) = self.current() else { return Effect::Handled };
                let choices = self.choices(&area);
                match key.code {
                    KeyCode::Char(character) if choices.iter().any(|choice| choice.key == character) => return self.act(character),
                    KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::Options(index.saturating_sub(1)),
                    KeyCode::Down | KeyCode::Char('j') => self.mode = Mode::Options((index + 1).min(choices.len().saturating_sub(1))),
                    KeyCode::Enter => {
                        if let Some(choice) = choices.get(index) {
                            return self.act(choice.key);
                        }
                    }
                    _ if leaves => {}
                    _ => self.mode = Mode::Options(index),
                }
            }
            Mode::Help => {}
            Mode::Message { next, .. } => {
                if key.code == KeyCode::Char('n')
                    && let Some(next) = next
                {
                    self.open_next(next);
                }
            }
            working @ Mode::Working { .. } => self.mode = working,
            Mode::Log { stem, scroll } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::Log { stem, scroll: scroll.saturating_sub(1) },
                KeyCode::Down | KeyCode::Char('j') => self.mode = Mode::Log { stem, scroll: scroll + 1 },
                KeyCode::PageUp => self.mode = Mode::Log { stem, scroll: scroll.saturating_sub(10) },
                KeyCode::PageDown | KeyCode::Char(' ') => self.mode = Mode::Log { stem, scroll: scroll + 10 },
                _ => {}
            },
            Mode::Confirm { title, lines, question, go, .. } => match key.code {
                KeyCode::Char('y') => match go {
                    Go::Script { repo, steps, next } => {
                        let (sender, result) = mpsc::channel();
                        thread::spawn(move || {
                            let _ = sender.send(run_script(repo, steps, next));
                        });
                        self.mode = Mode::Working { label: format!("{title}…"), result };
                    }
                    Go::Task { repo, name, prompt } => {
                        let options = tasks::Start { prompt: Some(&prompt), focus: true, from: None, mode: None, backlog: None, note: true };
                        let result = terminal::outside(terminal, || tasks::start(&repo, &name, &options));
                        self.show(result);
                    }
                },
                KeyCode::Char('n') => {}
                _ if leaves => {}
                _ => self.mode = Mode::Confirm { title, lines, question, go, nudged: true },
            },
            Mode::Push(window) => match key.code {
                KeyCode::Char('y') => {
                    let (sender, result) = mpsc::channel();
                    let label = format!("Pushing {} to origin…", window.tag.as_ref().map_or(window.branch.clone(), |tag| format!("{} and {tag}", window.branch)));
                    thread::spawn(move || {
                        let _ = sender.send(push(window));
                    });
                    self.mode = Mode::Working { label, result };
                }
                KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => {
                    if !window.done.is_empty() {
                        let text = format!("Done here, not pushed:\n{}", window.done.join("\n"));
                        self.mode = Mode::Message { text, next: None };
                    }
                }
                _ => self.mode = Mode::Push(window),
            },
            Mode::Input { purpose, label, mut buffer } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.submit(purpose, buffer.trim().to_string()),
                KeyCode::Backspace => {
                    buffer.pop();
                    self.mode = Mode::Input { purpose, label, buffer };
                }
                KeyCode::Char(character) => {
                    buffer.push(character);
                    self.mode = Mode::Input { purpose, label, buffer };
                }
                _ => self.mode = Mode::Input { purpose, label, buffer },
            },
        }
        Effect::Handled
    }

    fn handle_normal(&mut self, key: KeyEvent) -> Effect {
        let Some(area) = self.current() else { return Effect::Unhandled };
        let choices = self.choices(&area);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if self.focus == Focus::Detail => self.action = self.action.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') if self.focus == Focus::Detail => self.action = (self.action + 1).min(choices.len().saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => self.select(self.position().saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.select(self.position() + 1),
            KeyCode::Left | KeyCode::Right => self.focus = if self.focus == Focus::List { Focus::Detail } else { Focus::List },
            KeyCode::Enter if self.focus == Focus::Detail => {
                if let Some(choice) = choices.get(self.action) {
                    return self.act(choice.key);
                }
            }
            KeyCode::Enter => self.mode = Mode::Options(0),
            KeyCode::Char(character) if character == keys::key(Act::Help) => self.mode = Mode::Help,
            KeyCode::Char(character) if choices.iter().any(|choice| choice.key == character) => return self.act(character),
            _ => return Effect::Unhandled,
        }
        Effect::Handled
    }

    fn act(&mut self, key: char) -> Effect {
        let Some(area) = self.current() else { return Effect::Handled };
        let act = self.choices(&area).into_iter().find(|choice| choice.key == key).and_then(|choice| choice.act);
        if act.is_none() && Self::is_repo_action(&area, key) {
            let action = area.actions.iter().find(|action| action.key == key).cloned().unwrap();
            self.begin(&area, action, Vec::new());
            return Effect::Handled;
        }
        let Some(repo) = area.repo.clone() else {
            match act {
                Some(Act::AreaRefresh) => self.refresh_backlog(&area),
                Some(Act::Log) => self.mode = Mode::Log { stem: area.stem, scroll: 0 },
                _ => self.mode = Mode::Message { text: format!("{} has no repo on this machine.", area.stem), next: None },
            }
            return Effect::Handled;
        };
        match act {
            Some(Act::Go) => {
                let result = match herdr::workspace_for(&repo) {
                    Ok(Some(workspace)) => herdr::focus_workspace(&workspace),
                    Ok(None) => herdr::create_workspace(&repo, &area.stem).map(|_| ()),
                    Err(error) => Err(error),
                };
                return match result {
                    Ok(()) => Effect::Left,
                    Err(error) => {
                        self.show(Err(error));
                        Effect::Handled
                    }
                };
            }
            Some(Act::New) => self.mode = Mode::Input { purpose: Purpose::TaskName(area.stem), label: "New task here: name".to_string(), buffer: String::new() },
            Some(Act::AreaRefresh) => self.refresh_backlog(&area),
            Some(Act::Log) => self.mode = Mode::Log { stem: area.stem, scroll: 0 },
            Some(Act::Permit) => self.show(permit::toggle_repo(&repo::name_of(&repo))),
            Some(Act::Push) => match &area.git {
                Some(git) if git.ahead > 0 => {
                    let commits = commits_between(&repo, &format!("{}..refs/heads/{}", git.upstream, git.branch));
                    self.mode = Mode::Push(PushWindow { repo, branch: git.branch.clone(), tag: None, commits, done: Vec::new(), next: None });
                }
                Some(git) => self.show(Ok(format!("{} is level with {}; nothing to push.", git.branch, git.upstream))),
                None => self.show(Err(format!("Cannot read {}'s main branch.", area.stem))),
            },
            _ => {}
        }
        Effect::Handled
    }

    fn refresh_backlog(&mut self, area: &Area) {
        let vault = self.sources.contains(&Source::Vault);
        let result = backlog::refresh_prompt(&self.sources).and_then(|prompt| match lead_pane() {
            Some(pane) if vault => crate::typing::prompt(&pane, &format!("{prompt} Then check that the vault backlog for [[{}]] is current: close what is done, park what is stale.", area.stem), "the lead")
                .map(|_| format!("Asked the lead to refresh the backlog and [[{}]]'s notes; this page catches up once the vault index does.", area.stem)).map_err(|refused| format!("Nothing was typed: the lead {}; ask again once it is clear.", refused.reason())),
            Some(pane) => crate::typing::prompt(&pane, &prompt, "the lead").map(|_| "Asked the lead to refresh the backlog; the board's Backlog tab updates when it is saved.".to_string()).map_err(|refused| format!("Nothing was typed: the lead {}; ask again once it is clear.", refused.reason())),
            None => Err("No lead is running; prefix+space starts it.".to_string()),
        });
        self.show(result);
    }

    // Asks each of the action's questions in turn, prefilled from a previous action's answers, or the next patch for a version.
    fn begin(&mut self, area: &Area, action: Action, carried: Answers) {
        self.ask(area.stem.clone(), action, Vec::new(), carried);
    }

    fn ask(&mut self, stem: String, action: Action, answers: Answers, carried: Answers) {
        let Some(name) = action.asks.get(answers.len()).cloned() else {
            self.proceed(&stem, action, answers);
            return;
        };
        let current = match (&action.work, self.area(&stem).and_then(|area| area.repo)) {
            (Work::Script(script), Some(repo)) if name == "version" => actions::current_version(&repo, script),
            _ => None,
        };
        let buffer = actions::answer(&carried, &name).map(str::to_string)
            .or_else(|| current.as_deref().map(actions::next_patch)).unwrap_or_default();
        let label = match current {
            Some(current) => format!("{} · {name} (now {current})", action.title),
            None => format!("{} · {name}", action.title),
        };
        self.mode = Mode::Input { purpose: Purpose::Answer { stem, action: Box::new(action), answers, carried }, label, buffer };
    }

    fn proceed(&mut self, stem: &str, action: Action, answers: Answers) {
        let Some(area) = self.area(stem) else { return };
        let Some(repo) = area.repo.clone() else {
            self.show(Err(format!("{stem} has no repo on this machine.")));
            return;
        };
        match &action.work {
            Work::Task(template) => match actions::render(template, &answers) {
                Ok(prompt) => {
                    let values: Vec<&str> = answers.iter().map(|(_, value)| value.as_str()).collect();
                    let buffer = tasks::slug(&format!("{} {}", action.id, values.join(" "))).unwrap_or_default();
                    let prompt = format!("{}\n\n{prompt}", areas::task_context(&area));
                    self.mode = Mode::Input { purpose: Purpose::ActionTaskName { stem: stem.to_string(), prompt }, label: format!("{}: task name", action.title), buffer };
                }
                Err(error) => self.show(Err(error)),
            },
            Work::Script(script) => {
                let planned = actions::ready(&repo).and_then(|branch| actions::plan(script, &repo, &branch, &answers));
                match planned {
                    Ok(steps) => self.confirm_script(&area, repo, &action, &answers, steps),
                    Err(error) => self.show(Err(format!("{}: {error}", action.title))),
                }
            }
        }
    }

    fn confirm_script(&mut self, area: &Area, repo: PathBuf, action: &Action, answers: &Answers, steps: Vec<Step>) {
        let mut lines = Vec::new();
        for (name, value) in answers {
            lines.push(kv(name, value.clone(), TEXT));
        }
        if let Some(release) = area.git.as_ref().and_then(|git| git.release.as_ref().map(|release| (git, release)))
            && action.asks.iter().any(|ask| ask == "version")
        {
            let (git, release) = release;
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(format!("since {} ({})", release.tag, release.age), style(MUTED))));
            for commit in commits_between(&repo, &format!("{}..refs/heads/{}", release.tag, git.branch)).into_iter().take(12) {
                lines.push(Line::from(Span::styled(format!("  {commit}"), style(TEXT))));
            }
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("will", style(MUTED))));
        for step in &steps {
            let color = if matches!(step, Step::Push { .. }) { YELLOW } else { TEXT };
            lines.push(Line::from(Span::styled(format!("  {}", step.describe()), style(color))));
        }
        let next = first_next(action, answers);
        if let Some(next) = &next {
            lines.push(Line::from(vec![Span::styled("next  ", style(MUTED)), Span::styled(format!("{} (offered after, never run on its own)", next.target), style(TEAL))]));
        }
        let title = format!("{} · {}", action.title, area.stem);
        self.mode = Mode::Confirm { title, lines, question: format!("{}?", action.title), go: Go::Script { repo, steps, next }, nudged: false };
    }

    // A `then` target is repo:action; its area is found by repo name, its action by id.
    fn open_next(&mut self, next: Next) {
        let Some((repo_name, id)) = next.target.split_once(':') else { return };
        let found = self.areas.iter().find(|area| area.repo_name().as_deref() == Some(repo_name))
            .and_then(|area| area.actions.iter().find(|action| action.id == id).map(|action| (area.clone(), action.clone())));
        match found {
            Some((area, action)) => {
                self.selected = Some(area.key.clone());
                self.begin(&area, action, next.answers);
            }
            None => self.show(Err(format!("No action {id} in a repo named {repo_name}; is it in {}?", crate::config::repos_dir().join(format!("{repo_name}.toml")).display()))),
        }
    }

    fn submit(&mut self, purpose: Purpose, text: String) {
        match purpose {
            Purpose::TaskName(_) | Purpose::ActionTaskName { .. } if text.is_empty() => {}
            Purpose::TaskName(stem) => {
                self.mode = Mode::Input { purpose: Purpose::TaskPrompt(stem, text), label: "What should Claude do? (the area's notes are added)".to_string(), buffer: String::new() };
            }
            Purpose::TaskPrompt(stem, name) => {
                let Some(area) = self.area(&stem) else { return };
                let Some(repo) = area.repo.clone() else { return };
                let mut prompt = areas::task_context(&area);
                if !text.is_empty() {
                    prompt = format!("{prompt}\n\n{text}");
                }
                let lines = text_lines(&prompt, 88);
                self.mode = Mode::Confirm { title: format!("new task in {stem}"), lines, question: format!("Start {name}?"), go: Go::Task { repo, name, prompt }, nudged: false };
            }
            Purpose::Answer { stem, action, mut answers, carried } => {
                if text.is_empty() {
                    return;
                }
                let name = action.asks[answers.len()].clone();
                answers.push((name, text));
                self.ask(stem, *action, answers, carried);
            }
            Purpose::ActionTaskName { stem, prompt } => {
                let Some(repo) = self.area(&stem).and_then(|area| area.repo) else { return };
                let mut lines = vec![kv("task", text.clone(), TEXT), kv("in", repo::name_of(&repo), TEAL), Line::default()];
                lines.extend(text_lines(&prompt, 88));
                lines.push(Line::default());
                lines.push(Line::from(Span::styled("you review it and push it like any task", style(MUTED))));
                self.mode = Mode::Confirm { title: format!("start a task · {stem}"), lines, question: "Start the task?".to_string(), go: Go::Task { repo, name: text, prompt }, nudged: false };
            }
        }
    }

    pub fn mouse(&mut self, event: MouseEvent, terminal: &mut DefaultTerminal) -> Effect {
        let hit = self.hits.iter().find(|(area, _)| area.contains(Position::new(event.column, event.row))).map(|(_, hit)| *hit);
        match event.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if matches!(self.mode, Mode::Normal) {
                    match hit {
                        Some(Hit::Area(_)) => self.focus = Focus::List,
                        Some(Hit::Action(_)) => self.focus = Focus::Detail,
                        _ => {}
                    }
                }
                let code = if event.kind == MouseEventKind::ScrollUp { KeyCode::Up } else { KeyCode::Down };
                self.handle(KeyEvent::from(code), terminal)
            }
            MouseEventKind::Down(MouseButton::Left) => match (hit, &self.mode) {
                (Some(Hit::Area(position)), Mode::Normal) => {
                    let again = self.focus == Focus::List && self.position() == position;
                    self.focus = Focus::List;
                    self.select(position);
                    if again {
                        self.mode = Mode::Options(0);
                    }
                    Effect::Handled
                }
                (Some(Hit::Action(index)), Mode::Normal) => {
                    self.focus = Focus::Detail;
                    self.action = index;
                    match self.current().and_then(|area| self.choices(&area).get(index).map(|choice| choice.key)) {
                        Some(key) => self.act(key),
                        None => Effect::Handled,
                    }
                }
                (Some(Hit::Choice(index)), Mode::Options(_)) => {
                    self.mode = Mode::Normal;
                    match self.current().and_then(|area| self.choices(&area).get(index).map(|choice| choice.key)) {
                        Some(key) => self.act(key),
                        None => Effect::Handled,
                    }
                }
                (None, Mode::Options(_) | Mode::Message { .. } | Mode::Help | Mode::Log { .. }) => {
                    self.mode = Mode::Normal;
                    Effect::Handled
                }
                (_, Mode::Normal) => Effect::Unhandled,
                _ => Effect::Handled,
            },
            _ => Effect::Handled,
        }
    }

    fn list_lines(&self, width: usize) -> (Vec<Line<'static>>, usize, Vec<(usize, usize, Hit)>) {
        let focused = self.focus == Focus::List;
        let mut lines = column_title("AREAS", self.summary(), focused, width);
        let order = self.ordered();
        let position = self.position();
        let (mut selected_line, mut rows, mut current) = (0, Vec::new(), None);
        for (place, (index, standing)) in order.iter().enumerate() {
            let area = &self.areas[*index];
            if current != Some(*standing) {
                if current.is_some() {
                    lines.push(Line::default());
                }
                let count = order.iter().filter(|(_, other)| other == standing).count();
                lines.push(group_line(standing.title(), count, standing.color()));
                current = Some(*standing);
            }
            if place == position {
                selected_line = lines.len();
            }
            rows.push((lines.len(), lines.len() + 1, Hit::Area(place)));
            lines.push(row(place == position, focused, icon(area, *standing), &area.stem, None, &row_facts(area, &self.tasks), width));
        }
        if order.is_empty() {
            lines.push(Line::from(Span::styled("No system notes in the vault's Areas/.", style(MUTED))));
        }
        (lines, selected_line, rows)
    }

    fn detail_lines(&self, area: &Area, width: usize) -> (Vec<Line<'static>>, usize, Vec<(usize, usize, Hit)>) {
        let mut lines = vec![Line::from(Span::styled(area.stem.clone(), bold(TEXT)))];
        let place = area.repo.as_deref().map_or("no repo on this machine".to_string(), home_relative);
        lines.push(Line::from(Span::styled(format!("{place} · [[{}]]", area.stem), style(MUTED))));
        for text in wrap(&area.summary, width, 3) {
            lines.push(Line::from(Span::styled(text, style(SUBTEXT))));
        }
        let mut facts = Vec::new();
        if let Some(git) = &area.git {
            if area.unreleased() > 0 && let Some(release) = &git.release {
                facts.push((format!("{} since {}", plural(release.since, "commit"), release.tag), YELLOW));
            }
            if git.ahead > 0 {
                facts.push((format!("{} on {} to push", plural(git.ahead, "commit"), git.branch), PEACH));
            }
            if git.behind > 0 {
                facts.push((format!("{} behind {}", git.behind, git.upstream), YELLOW));
            }
            if git.ahead == 0 && git.behind == 0 {
                facts.push((format!("{} level with origin", git.branch), GREEN));
            }
            if git.changed > 0 {
                facts.push((format!("{} changed in the main checkout", plural(git.changed, "file")), YELLOW));
            }
        }
        if !facts.is_empty() {
            lines.push(Line::default());
            lines.push(chips(facts).style(Style::new()));
        }
        lines.push(Line::default());
        if let Some(git) = &area.git {
            let release = git.release.as_ref().map_or("no tags yet".to_string(), |release| format!("{}, {} · {} since", release.tag, release.age, plural(release.since, "commit")));
            lines.push(kv("release", release, TEXT));
        }
        let mine = tasks_in(&self.tasks, area);
        if !mine.is_empty() {
            let names: Vec<String> = mine.iter().map(|item| format!("{} · {}", item.name, item.group().title().to_lowercase())).collect();
            lines.push(kv("tasks", names.join(", "), TEXT));
        }
        if !area.open.is_empty() {
            let titles: Vec<&str> = area.open.iter().map(|note| note.title.as_str()).collect();
            for (index, text) in wrap(&titles.join(" · "), width.saturating_sub(9), 2).into_iter().enumerate() {
                lines.push(kv(if index == 0 { "open" } else { "" }, text, TEXT));
            }
        }
        lines.push(kv("backlog", plural(area.backlog.len(), "item"), TEXT));
        if let Some(repo) = area.repo_name() {
            let (text, color) = if permit::repo_permitted(&repo) { ("the lead may push reviewed tasks", GREEN) } else { ("you, and the lead for tasks you permit", TEXT) };
            lines.push(kv("pushes", text.to_string(), color));
        }
        if let Some(line) = area.log.first() {
            lines.push(Line::default());
            lines.extend(card_text("last log", &format!("[{}] {}", line.date, line.text), FAINT, width, 2).into_iter().map(|line| line.style(Style::new())));
        }
        for error in &area.errors {
            lines.extend(wrap(error, width, 3).into_iter().map(|line| Line::from(Span::styled(line, style(RED)))));
        }
        lines.push(Line::default());
        let focused = self.focus == Focus::Detail;
        lines.extend(column_title("ACTIONS", "⏎ or the key".to_string(), focused, width));
        let (mut selected_line, mut rows) = (0, Vec::new());
        for (index, choice) in self.choices(area).iter().enumerate() {
            let active = focused && index == self.action;
            if active {
                selected_line = lines.len();
            }
            let background = if active { Style::new().bg(SURFACE) } else { Style::new() };
            let source = if Self::is_repo_action(area, choice.key) { "  repo config" } else { "" };
            let used = choice.label.chars().count() + source.chars().count() + 3;
            rows.push((lines.len(), lines.len() + 2, Hit::Action(index)));
            lines.push(Line::from(vec![
                Span::raw(if active { "▸ " } else { "  " }), Span::styled(choice.label.clone(), if active { bold(MAUVE) } else { style(TEXT) }),
                Span::styled(source, style(TEAL)), Span::raw(" ".repeat(width.saturating_sub(used).max(1))), Span::styled(choice.key.to_string(), style(MUTED)),
            ]).style(background));
            let about: String = choice.about.chars().take(width.saturating_sub(2)).collect();
            lines.push(Line::from(vec![Span::raw("  "), Span::styled(about, style(MUTED))]).style(background));
        }
        (lines, selected_line, rows)
    }

    pub fn draw(&mut self, frame: &mut Frame, body: Rect) {
        self.hits.clear();
        if let Some(error) = &self.error {
            frame.render_widget(ratatui::widgets::Paragraph::new(Span::styled(error.clone(), style(RED))).wrap(ratatui::widgets::Wrap { trim: false }), body);
        } else if !self.loaded {
            frame.render_widget(ratatui::widgets::Paragraph::new(Span::styled("reading the vault…", style(MUTED))), body);
        } else if body.width >= 86 {
            let [list_area, _, detail_area] = Layout::horizontal([Constraint::Percentage(44), Constraint::Length(4), Constraint::Min(30)]).areas(body);
            let (lines, selected, rows) = self.list_lines(list_area.width as usize);
            render_column(frame, list_area, lines, selected, rows, &mut self.hits);
            if let Some(area) = self.current() {
                let (lines, selected, rows) = self.detail_lines(&area, detail_area.width as usize);
                render_column(frame, detail_area, lines, selected, rows, &mut self.hits);
            }
        } else {
            let width = body.width as usize;
            let (mut lines, list_selected, mut rows) = self.list_lines(width);
            if let Some(area) = self.current() {
                let offset = lines.len() + 1;
                let (detail, detail_selected, detail_rows) = self.detail_lines(&area, width);
                lines.push(Line::default());
                lines.extend(detail);
                rows.extend(detail_rows.into_iter().map(|(start, end, hit)| (start + offset, end + offset, hit)));
                let selected = if self.focus == Focus::List { list_selected } else { offset + detail_selected };
                render_column(frame, body, lines, selected, rows, &mut self.hits);
            } else {
                render_column(frame, body, lines, list_selected, rows, &mut self.hits);
            }
        }
        self.draw_mode(frame);
    }

    fn draw_mode(&mut self, frame: &mut Frame) {
        let mut modal_hits = Vec::new();
        match &self.mode {
            Mode::Normal => return,
            Mode::Options(index) => {
                let Some(area) = self.current() else { return };
                let choices = self.choices(&area);
                let count = choices.len();
                let window = modal(frame, area.stem.clone(), options_lines(choices, *index, 56), 62);
                for choice in 0..count {
                    let y = window.y + 1 + 2 * choice as u16;
                    if y + 1 < window.bottom() {
                        modal_hits.push((Rect::new(window.x, y, window.width, 2), Hit::Choice(choice)));
                    }
                }
            }
            Mode::Help => {
                let mut keys = keys::map().help(&[Ctx::Areas], false);
                keys.push(("A–Z".to_string(), "the repo's own actions, from its repo config (kelpie config shows where)"));
                let lines = keys.into_iter().map(|(key, text)| Line::from(vec![Span::styled(format!("{key:<9}"), style(MAUVE)), Span::styled(text.to_string(), style(TEXT))])).collect();
                modal(frame, "keys".to_string(), lines, 76);
            }
            Mode::Message { text, next } => {
                let mut lines: Vec<Line> = text.lines().map(|line| Line::from(Span::styled(line.to_string(), style(TEXT)))).collect();
                if let Some(next) = next {
                    lines.push(Line::default());
                    lines.push(Line::from(vec![Span::styled("next  ", style(MUTED)), Span::styled(next.target.clone(), style(TEAL)), Span::styled("  n opens it", style(MUTED))]));
                }
                lines.push(Line::from(Span::styled("any key to close", style(MUTED))));
                modal(frame, "kelpie".to_string(), lines, 96);
            }
            Mode::Working { label, .. } => {
                modal(frame, "kelpie".to_string(), vec![Line::from(Span::styled(label.clone(), style(TEXT)))], 72);
            }
            Mode::Input { label, buffer, .. } => {
                modal(frame, label.clone(), vec![
                    Line::from(vec![Span::styled(buffer.clone(), style(TEXT)), Span::styled("▏", style(MAUVE))]),
                    Line::from(Span::styled("enter to continue · esc to cancel", style(MUTED))),
                ], 72);
            }
            Mode::Confirm { title, lines, question, nudged, .. } => {
                let mut lines = lines.clone();
                lines.push(Line::default());
                lines.push(Line::from(vec![Span::styled(format!("{question}  "), style(YELLOW)), Span::styled("y / n", style(MUTED))]));
                if *nudged {
                    lines.push(Line::from(Span::styled("press y or n", style(PEACH))));
                }
                modal(frame, title.clone(), lines, 96);
            }
            Mode::Push(window) => {
                let mut lines: Vec<Line> = window.done.iter().map(|done| Line::from(Span::styled(format!("✓ {done}"), style(GREEN)))).collect();
                if !lines.is_empty() {
                    lines.push(Line::default());
                }
                lines.push(Line::from(Span::styled(format!("{} on {} not on origin", plural(window.commits.len(), "commit"), window.branch), style(MUTED))));
                lines.extend(window.commits.iter().take(20).map(|commit| Line::from(Span::styled(format!("  {commit}"), style(TEXT)))));
                if let Some(tag) = &window.tag {
                    lines.push(kv("tag", tag.clone(), TEXT));
                }
                lines.push(Line::default());
                let what = window.tag.as_ref().map_or(window.branch.clone(), |tag| format!("{} and {tag}", window.branch));
                lines.push(Line::from(vec![Span::styled(format!("Push {what} to origin, as you?  "), style(YELLOW)), Span::styled("y / n", style(MUTED))]));
                modal(frame, format!("push · {}", repo::name_of(&window.repo)), lines, 96);
            }
            Mode::Log { stem, scroll } => {
                let log = self.area(stem).map(|area| area.log).unwrap_or_default();
                let height = frame.area().height.saturating_sub(8) as usize;
                let mut text: Vec<Line> = Vec::new();
                for line in &log {
                    for (index, part) in wrap(&line.text, 80, 4).into_iter().enumerate() {
                        let date = if index == 0 { format!("{}  ", line.date) } else { " ".repeat(12) };
                        text.push(Line::from(vec![Span::styled(date, style(MUTED)), Span::styled(part, style(TEXT))]));
                    }
                }
                if text.is_empty() {
                    text.push(Line::from(Span::styled("No log lines for this area.", style(MUTED))));
                }
                let scroll = (*scroll).min(text.len().saturating_sub(height));
                let mut shown: Vec<Line> = text.into_iter().skip(scroll).take(height).collect();
                shown.push(Line::from(Span::styled("↑ ↓ scroll · esc close", style(FAINT))));
                modal(frame, format!("log · {stem}"), shown, 100);
            }
        }
        self.hits = modal_hits;
    }

    pub fn hint_lines() -> (Line<'static>, Line<'static>) {
        let (full, short) = keys::map().hints(Ctx::Areas, false);
        let words = |hints: Vec<(String, &'static str, KeyCode)>| hints.into_iter().map(|(key, word, _)| (key, if word == "options" { "actions" } else { word })).collect::<Vec<_>>();
        let line = |hints: Vec<(String, &'static str)>| widgets::hints(&hints.iter().map(|(key, word)| (key.as_str(), *word)).collect::<Vec<_>>());
        (line(words(full)), line(words(short)))
    }
}

// Runs the local steps; a plan that pushes then asks before anything leaves the machine.
fn run_script(repo: PathBuf, steps: Vec<Step>, next: Option<Next>) -> After {
    match actions::run_local(&repo, &steps) {
        Err(error) => After::Show(Err(error), None),
        Ok(done) => match steps.iter().find_map(|step| match step { Step::Push { branch, tag } => Some((branch.clone(), tag.clone())), _ => None }) {
            Some((branch, tag)) => {
                let upstream = repo::default_branch(&repo).unwrap_or_else(|_| format!("origin/{branch}"));
                let commits = commits_between(&repo, &format!("{upstream}..refs/heads/{branch}"));
                After::Push(PushWindow { repo, branch, tag, commits, done, next })
            }
            None => After::Show(Ok(done.iter().map(|line| format!("✓ {line}")).collect::<Vec<_>>().join("\n")), next),
        },
    }
}

fn push(window: PushWindow) -> After {
    let mut refspecs = vec![window.branch.as_str()];
    if let Some(tag) = &window.tag {
        refspecs.push(tag);
    }
    let what = refspecs.join(" and ");
    match push_as_user(&window.repo, &refspecs) {
        Ok(()) => After::Show(Ok(format!("Pushed {what} to origin ({}).", plural(window.commits.len(), "commit"))), window.next),
        Err(error) => After::Show(Err(format!("Pushing {what} failed: {error}")), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(key: &str, stem: &str) -> Area {
        Area { key: key.to_string(), stem: stem.to_string(), summary: String::new(), repo: None, open: Vec::new(), backlog: Vec::new(), log: Vec::new(), git: None, actions: Vec::new(), errors: Vec::new() }
    }

    #[test]
    fn moving_down_reaches_an_area_whose_name_another_area_shares() {
        let mut page = Page::new(Vec::new());
        page.areas = vec![area("Areas/a.md", "a"), area("Areas/santa.md", "santa"), area("Areas/apps/santa.md", "santa")];
        for step in 1..3 {
            page.select(page.position() + 1);
            assert_eq!(page.position(), step);
        }
    }

    fn with_git(mut area: Area, ahead: usize, changed: usize) -> Area {
        area.repo = Some(PathBuf::from("/src/app"));
        area.git = Some(crate::areas::RepoState { branch: "main".to_string(), upstream: "origin/main".to_string(), ahead, behind: 0, changed, release: None });
        area
    }

    #[test]
    fn an_area_needs_you_with_commits_to_push_is_active_with_tasks_or_changes_and_quiet_otherwise() {
        let quiet = area("Areas/app.md", "app");
        assert!(standing(&quiet, &[]) == Standing::Quiet);
        assert!(standing(&with_git(quiet.clone(), 0, 2), &[]) == Standing::Active);
        assert!(standing(&with_git(quiet.clone(), 1, 0), &[]) == Standing::NeedsYou);
        let mut mine = crate::status::tests::task(Some("working"), None, 1);
        mine.repo = Some(PathBuf::from("/src/app"));
        assert!(standing(&with_git(quiet.clone(), 0, 0), std::slice::from_ref(&mine)) == Standing::Active);
        mine.agent.as_mut().unwrap().status = "blocked".to_string();
        assert!(standing(&with_git(quiet.clone(), 0, 0), std::slice::from_ref(&mine)) == Standing::NeedsYou, "a task that needs you makes its area need you");
        let mut elsewhere = mine.clone();
        elsewhere.repo = Some(PathBuf::from("/src/other"));
        assert!(standing(&with_git(quiet.clone(), 0, 0), &[elsewhere]) == Standing::Quiet, "a task in another repo is not this area's");
        assert_eq!(row_facts(&with_git(quiet.clone(), 2, 1), &[mine.clone()]), "2 to push · 1 file changed · 1 task", "three facts at most");
        assert_eq!(row_facts(&quiet, &[]), "no repo");
    }

    #[test]
    fn paths_under_home_are_shown_with_a_tilde_and_paragraphs_wrap_apart() {
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(home_relative(&PathBuf::from(&home).join("src/app")), "~/src/app");
        assert_eq!(home_relative(&PathBuf::from("/opt/app")), "/opt/app");
        let lines = text_lines("one two\n\nthree", 20);
        assert_eq!(lines.iter().map(|line| line.to_string()).collect::<Vec<_>>(), ["one two", "", "three"]);
        let action = Action { id: "release".to_string(), key: 'R', title: "Release".to_string(), about: String::new(), asks: vec!["version".to_string()], work: Work::Script(crate::actions::Script { bump: Vec::new(), run: Vec::new(), commit: None, tag: None, push: true }), then: vec!["plugins:bump-pin".to_string()], source: String::new() };
        let next = first_next(&action, &vec![("version".to_string(), "0.5.1".to_string())]).unwrap();
        assert_eq!((next.target.as_str(), next.answers.len()), ("plugins:bump-pin", 1));
        assert!(first_next(&Action { then: Vec::new(), ..action }, &Vec::new()).is_none());
    }

    #[test]
    fn a_repos_own_actions_are_marked_as_such_in_the_choices() {
        let mut app = with_git(area("Areas/app.md", "app"), 1, 0);
        app.actions = crate::actions::parse("[[actions]]\nkey = \"B\"\ntitle = \"Build\"\nrun = \"cargo build\"\n", "app.toml").0;
        let page = Page::new(vec![Source::Local]);
        let choices = page.choices(&app);
        let keys: String = choices.iter().map(|choice| choice.key).collect();
        assert_eq!(keys, "gnablpB");
        assert!(Page::is_repo_action(&app, 'B') && !Page::is_repo_action(&app, 'p'));
        assert_eq!(choices.iter().find(|choice| choice.key == 'p').unwrap().label, "Push main");
        let keys: String = page.choices(&area("Areas/notes.md", "notes")).iter().map(|choice| choice.key).collect();
        assert_eq!(keys, "bl", "without a repo there is nothing to open, start or push");
    }
}
