use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::Position;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use ratatui_textarea::{TextArea, WrapMode};

use crate::area_page::{self, Effect};
use crate::backlog::{self, Backlog, Entry};
use crate::config::{self, Source};
use crate::fetch::{self, Fetcher};
use crate::herdr;
use crate::messages;
use crate::repo::{self, name_of};
use crate::sh::Result;
use crate::status::{self, Checkout, Group, Item};
use crate::tasks;

pub const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
pub const SUBTEXT: Color = Color::Rgb(0xa6, 0xad, 0xc8);
pub const MUTED: Color = Color::Rgb(0x6c, 0x70, 0x86);
pub const FAINT: Color = Color::Rgb(0x45, 0x47, 0x5a);
pub const SURFACE: Color = Color::Rgb(0x31, 0x32, 0x44);
pub const MANTLE: Color = Color::Rgb(0x18, 0x18, 0x25);
pub const MAUVE: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
pub const PEACH: Color = Color::Rgb(0xfa, 0xb3, 0x87);
pub const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
pub const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
pub const BLUE: Color = Color::Rgb(0x89, 0xb4, 0xfa);
pub const TEAL: Color = Color::Rgb(0x94, 0xe2, 0xd5);
pub const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);


#[derive(Clone, Copy, PartialEq)]
enum Page {
    Work,
    Areas,
}

#[derive(Clone, Copy, PartialEq)]
enum Column {
    Work,
    Backlog,
}

pub struct Choice {
    pub key: char,
    pub label: String,
    pub about: String,
}

pub fn choice(key: char, label: &str, about: &str) -> Choice {
    Choice { key, label: label.to_string(), about: about.to_string() }
}

fn task_choices(item: Option<&Item>, dev_server: Option<&str>) -> Vec<Choice> {
    let go = if item.is_some_and(|item| item.agent.is_none()) {
        choice('g', "Restart it", "reopen its Space and its Claude, resuming its last conversation")
    } else {
        choice('g', "Go to its Space", "jump to the task's Space")
    };
    let mut choices = vec![
        go,
        choice('p', "Message its Claude", "send it a line without leaving"),
        choice('r', "Open the review", "the review split next to its Claude"),
    ];
    if let Some(command) = dev_server {
        choices.push(if item.is_some_and(|item| item.server) {
            choice('w', "Stop the dev server", "ctrl-c to its server tab")
        } else {
            choice('w', "Start the dev server", &format!("{command} in a tab of its Space"))
        });
    }
    if !item.is_some_and(busy) {
        if item.and_then(|item| item.git.as_ref()).is_some_and(|git| git.ahead > 0) {
            choices.push(choice('v', "Request review", "a separate, read-only Claude reviews its commits in the background; the verdict goes to the lead"));
        }
        choices.extend([
            choice('t', "Tidy commits", "its Claude reshapes them by the rules skill"),
            choice('l', "Push and close", "rebase, check, show the commits, then push if you say so"),
            choice('d', "Done", "already pushed elsewhere: close it and free the worktree"),
        ]);
    }
    choices.push(choice('z', "Reset", "stop it for good: its work goes, its backlog item comes back (asks first)"));
    choices.push(choice('x', "Drop", "throw the task away (asks first)"));
    choices
}

// While its Claude works, the branch can still move, so the steps that rewrite, push or close it wait.
fn busy(item: &Item) -> bool {
    item.group() == Group::Working
}

fn backlog_choices(sources: &[Source]) -> Vec<Choice> {
    let refresh = if sources.is_empty() { "no backlog source is configured".to_string() } else { format!("the lead fetches {} again", backlog::from(sources)) };
    vec![
        choice('g', "Start as task", "a worktree, a Space and Claude with this as its prompt"),
        choice('R', "Refresh the backlog", &refresh),
    ]
}

enum Purpose {
    TaskName,
    TaskPrompt(String),
    Message(String, String),
    BacklogName(Entry),
    Tidy(PathBuf),
}

enum Mode {
    Normal,
    Options(usize),
    Input { purpose: Purpose, label: String, buffer: String },
    Message(String),
    ConfirmDrop { path: PathBuf, name: String, force: bool },
    ConfirmReset { path: PathBuf, name: String },
    ConfirmPush { path: PathBuf, commits: Vec<tasks::Commit>, selected: usize },
    EditMessage(Box<MessageEditor>),
    Working { label: String, result: Receiver<Result<String>> },
    Help,
}

#[derive(Clone, Copy)]
enum Hit {
    Task(usize),
    Entry(usize),
    Choice(usize),
    Commit(usize),
    Tab(Page),
}

enum Rewrite {
    Reword(String),
    Squash,
}

// Commit message editing inside the board, like lazygit's; ctrl-o hands the text to the user's editor.
struct MessageEditor {
    path: PathBuf,
    commits: Vec<tasks::Commit>,
    selected: usize,
    rewrite: Rewrite,
    text: TextArea<'static>,
    draft: Option<Receiver<Result<String>>>,
}

impl MessageEditor {
    fn new(path: PathBuf, commits: Vec<tasks::Commit>, selected: usize, rewrite: Rewrite, message: &str) -> Self {
        let mut editor = Self { path, commits, selected, rewrite, text: TextArea::default(), draft: None };
        editor.set_text(message);
        editor
    }

    fn set_text(&mut self, message: &str) {
        let mut text = TextArea::new(message.lines().map(str::to_string).collect());
        text.set_wrap_mode(WrapMode::WordOrGlyph);
        text.set_cursor_line_style(Style::new());
        text.set_style(style(TEXT));
        self.text = text;
    }

    fn message(&self) -> String {
        self.text.lines().join("\n").trim().to_string()
    }

    // The squash draft arrives from a background Claude; typing before it arrives keeps what was typed.
    fn poll_draft(&mut self) {
        let Some(receiver) = &self.draft else { return };
        let Ok(result) = receiver.try_recv() else { return };
        self.draft = None;
        if self.message().is_empty() {
            match result {
                Ok(message) => self.set_text(&message),
                Err(error) => {
                    let joined: Vec<&str> = self.commits.iter().map(|commit| commit.message.as_str()).collect();
                    self.set_text(&joined.join("\n\n"));
                    self.text.set_placeholder_text(error);
                }
            }
        }
    }
}

struct Snapshot {
    items: Vec<Item>,
    checkouts: Vec<Checkout>,
}

struct App {
    column: Column,
    tasks: Vec<Item>,
    checkouts: Vec<Checkout>,
    backlog: Backlog,
    work_selected: usize,
    backlog_selected: usize,
    mode: Mode,
    overlay: bool,
    loaded: bool,
    error: Option<String>,
    screens: HashMap<String, (Instant, String)>,
    refresh: Sender<()>,
    quit: bool,
    wanted_task: Option<String>,
    hits: Vec<(Rect, Hit)>,
    page: Page,
    areas: area_page::Page,
    sources: Vec<Source>,
    fetch_failures: Vec<String>,
}

pub fn style(color: Color) -> Style {
    Style::new().fg(color)
}

pub fn bold(color: Color) -> Style {
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

fn group_color(group: Group) -> Color {
    match group {
        Group::NeedsYou => PEACH,
        Group::Working => BLUE,
        Group::ReadyToPush => GREEN,
        Group::Idle => MUTED,
    }
}

fn priority_color(priority: &str) -> Color {
    match priority {
        "urgent" => RED,
        "high" => PEACH,
        "medium" => YELLOW,
        _ => MUTED,
    }
}

fn priority_icon(priority: &str) -> &'static str {
    match priority {
        "urgent" | "high" => "▲",
        "medium" => "■",
        _ => "·",
    }
}

fn status_icon(item: &Item) -> Span<'static> {
    match item.agent.as_ref().map(|agent| agent.status.as_str()) {
        Some("blocked") => Span::styled("●", style(PEACH)),
        Some("done") => Span::styled("✓", style(GREEN)),
        Some("working") => Span::styled("◐", style(BLUE)),
        Some(_) => Span::styled("○", style(MUTED)),
        None => Span::styled("·", style(MUTED)),
    }
}

pub fn plural(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

// A task can outlive its Space or its Claude; kelpie start brings them back.
fn absence(item: &Item) -> Option<&'static str> {
    match (&item.workspace, &item.agent) {
        (None, None) => Some("no Space"),
        (Some(_), None) => Some("no Claude"),
        _ => None,
    }
}

fn row_facts(item: &Item) -> String {
    let Some(git) = &item.git else { return String::new() };
    let mut parts: Vec<String> = absence(item).map(str::to_string).into_iter().collect();
    if git.ahead > 0 {
        parts.push(plural(git.ahead, "commit"));
    }
    if git.changed > 0 {
        parts.push(plural(git.changed, "file"));
    }
    if git.behind > 0 {
        parts.push(format!("{} behind", git.behind));
    }
    if item.unchecked() {
        parts.push(crate::status::UNCHECKED.to_string());
    }
    if parts.len() == usize::from(absence(item).is_some()) {
        parts.push("nothing yet".to_string());
    }
    parts.join(" · ")
}

// A worktree pool is per repo, so free worktrees are counted per repo; start makes a new one when a repo has none.
fn free_worktrees(checkouts: &[Checkout]) -> String {
    let free: Vec<String> = checkouts.iter().filter(|checkout| checkout.free_slots > 0)
        .map(|checkout| format!("{} {}", checkout.name, checkout.free_slots)).collect();
    if free.is_empty() { String::new() } else { format!(" · free worktrees: {}", free.join(", ")) }
}

pub fn suggested_name(title: &str) -> String {
    const FILLER: [&str; 12] = ["a", "an", "the", "and", "of", "for", "to", "on", "in", "with", "s", "before"];
    tasks::ascii_fold(&title.to_lowercase()).split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty() && !FILLER.contains(word)).take(4).collect::<Vec<_>>().join("-")
}

// Claude's last message is the last "● " block on screen, up to its footer lines.
fn last_message(output: &str) -> String {
    let lines: Vec<&str> = output.lines().map(str::trim_end).collect();
    let bullet = |line: &str| ["● ", "⏺ "].iter().any(|marker| line.trim_start().starts_with(marker));
    let Some(start) = lines.iter().rposition(|line| bullet(line)) else { return String::new() };
    let stop = |line: &str| {
        let line = line.trim_start();
        ["✻", "※", "─", "❯", "⏵", "╭", "╰", "· "].iter().any(|marker| line.starts_with(marker))
    };
    lines[start..].iter().take_while(|line| !stop(line))
        .map(|line| line.trim()).filter(|line| !line.is_empty())
        .collect::<Vec<_>>().join(" ").trim_start_matches("● ").trim_start_matches("⏺ ").to_string()
}

// The end of a long message, where Claude puts its question, cut at a word.
fn tail(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let rest: String = text.chars().skip(count - max_chars).collect();
    format!("…{}", rest.split_once(' ').map_or(rest.as_str(), |(_, words)| words))
}

// Fallback for a session that has not reported yet: Claude Code's own "※ recap:" under a finished run, free text that can be switched off.
fn recap(output: &str) -> String {
    let lines: Vec<&str> = output.lines().map(str::trim_end).collect();
    let Some(start) = lines.iter().rposition(|line| line.starts_with("※ recap:")) else { return String::new() };
    let continued = lines[start + 1..].iter().take_while(|line| line.starts_with("  ") && !line.trim().is_empty());
    std::iter::once(&lines[start]).chain(continued).map(|line| line.trim()).collect::<Vec<_>>().join(" ")
        .trim_start_matches("※ recap:").replace("(disable recaps in /config)", "").trim().to_string()
}

pub fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(10);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let last = lines.last_mut().unwrap();
        let keep = width.saturating_sub(1).min(last.chars().count());
        *last = last.chars().take(keep).collect::<String>() + "…";
    }
    lines
}

fn spawn_refresher() -> (Receiver<std::result::Result<Snapshot, String>>, Sender<()>) {
    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    let (refresh_tx, refresh_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        loop {
            let snapshot = status::gather().and_then(|items| Ok(Snapshot { items, checkouts: status::checkouts()? }));
            if snapshot_tx.send(snapshot).is_err() {
                return;
            }
            let _ = refresh_rx.recv_timeout(Duration::from_secs(3));
        }
    });
    (snapshot_rx, refresh_tx)
}

// A tab of that name from an earlier run, with its shell back at the prompt, gets reused.
fn idle_tab_pane(workspace: &str, label: &str) -> Option<String> {
    let tabs: Vec<String> = herdr::tabs_in(workspace).ok()?.into_iter()
        .filter(|tab| tab["label"] == label).filter_map(|tab| tab["tab_id"].as_str().map(str::to_string)).collect();
    herdr::panes_in(workspace).ok()?.into_iter()
        .filter(|pane| pane["tab_id"].as_str().is_some_and(|tab| tabs.iter().any(|candidate| candidate == tab)))
        .filter_map(|pane| pane["pane_id"].as_str().map(str::to_string))
        .find(|pane| matches!(herdr::foreground(pane), Ok(None)))
}

// scripts/sync.sh reports a running dev server from a busy pane in a tab with this label.
const SERVER_TAB: &str = "server";

fn dev_server(item: &Item) -> Option<String> {
    config::resolve(Some(item.repo.as_deref()?)).ok()?.settings.dev_server.value
}

// export rather than a PORT= prefix, so the port reaches every part of a compound command; bash, zsh and fish all take it.
fn server_command(command: &str, port: Option<u16>) -> String {
    port.map_or_else(|| command.to_string(), |port| format!("export PORT={port}; {command}"))
}

fn busy_server_pane(workspace: &str) -> Option<String> {
    let tabs: Vec<String> = herdr::tabs_in(workspace).ok()?.into_iter()
        .filter(|tab| tab["label"] == SERVER_TAB).filter_map(|tab| tab["tab_id"].as_str().map(str::to_string)).collect();
    herdr::panes_in(workspace).ok()?.into_iter()
        .filter(|pane| pane["tab_id"].as_str().is_some_and(|tab| tabs.iter().any(|candidate| candidate == tab)))
        .filter_map(|pane| pane["pane_id"].as_str().map(str::to_string))
        .find(|pane| matches!(herdr::foreground(pane), Ok(Some(_))))
}

fn review_script() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    Some(exe.ancestors().nth(3)?.join("scripts/review.sh"))
}

impl App {
    fn selected_task(&self) -> Option<Item> {
        self.tasks.get(self.work_selected).cloned()
    }

    fn selected_entry(&self) -> Option<Entry> {
        self.backlog.entries.get(self.backlog_selected).cloned()
    }

    fn choices(&self) -> Vec<Choice> {
        match self.column {
            Column::Work => {
                let item = self.tasks.get(self.work_selected);
                task_choices(item, item.and_then(dev_server).as_deref())
            }
            Column::Backlog => backlog_choices(&self.sources),
        }
    }

    fn agent_screen(&mut self, pane: &str) -> String {
        if let Some((at, screen)) = self.screens.get(pane)
            && at.elapsed() < Duration::from_secs(4)
        {
            return screen.clone();
        }
        let screen = herdr::read_agent(pane).unwrap_or_default();
        self.screens.insert(pane.to_string(), (Instant::now(), screen.clone()));
        screen
    }

    fn repo_named(&self, name: &str) -> Option<PathBuf> {
        self.tasks.iter().filter_map(|item| item.repo.clone()).find(|repo| name_of(repo) == name)
            .or_else(|| herdr::workspaces().ok()?.iter().filter_map(|workspace| workspace["worktree"]["repo_root"].as_str())
                .map(PathBuf::from).find(|repo| name_of(repo) == name))
    }

    fn any_repo(&self) -> PathBuf {
        self.selected_task().and_then(|item| item.repo)
            .or_else(|| self.checkouts.first().and_then(|checkout| self.repo_named(&checkout.name)))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    }

    fn lead_pane(&self) -> Option<String> {
        herdr::agents().ok()?.into_iter().find(|agent| agent["name"] == "lead")
            .and_then(|agent| agent["pane_id"].as_str().map(str::to_string))
    }

    // Pages switch only when nothing is open on either.
    fn at_rest(&self) -> bool {
        matches!(self.mode, Mode::Normal) && self.areas.is_normal()
    }

    fn show(&mut self, result: Result<String>) {
        self.mode = Mode::Message(result.unwrap_or_else(|error| error));
        let _ = self.refresh.send(());
    }

    fn move_selection(&mut self, step: isize) {
        let (selected, length) = match self.column {
            Column::Work => (&mut self.work_selected, self.tasks.len()),
            Column::Backlog => (&mut self.backlog_selected, self.backlog.entries.len()),
        };
        *selected = selected.saturating_add_signed(step).min(length.saturating_sub(1));
    }

    // A click selects a row, a click on the selected row opens its options, and the wheel moves through whatever is in front.
    fn mouse(&mut self, event: MouseEvent, terminal: &mut DefaultTerminal) {
        let hit = self.hits.iter().find(|(area, _)| area.contains(Position::new(event.column, event.row))).map(|(_, hit)| *hit);
        if let (Some(Hit::Tab(page)), MouseEventKind::Down(MouseButton::Left)) = (hit, event.kind) {
            self.page = page;
            return;
        }
        if self.page == Page::Areas {
            if let Effect::Left = self.areas.mouse(event, terminal) {
                self.quit = self.overlay;
            }
            return;
        }
        match event.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Mode::EditMessage(editor) = &mut self.mode {
                    editor.text.input(event);
                    return;
                }
                match hit {
                    Some(Hit::Task(_)) if matches!(self.mode, Mode::Normal) => self.column = Column::Work,
                    Some(Hit::Entry(_)) if matches!(self.mode, Mode::Normal) => self.column = Column::Backlog,
                    _ => {}
                }
                let code = if event.kind == MouseEventKind::ScrollUp { KeyCode::Up } else { KeyCode::Down };
                self.handle(KeyEvent::from(code), terminal);
            }
            MouseEventKind::Down(MouseButton::Left) => match (hit, &mut self.mode) {
                (Some(Hit::Task(index)), Mode::Normal) => {
                    let again = self.column == Column::Work && self.work_selected == index;
                    self.column = Column::Work;
                    self.work_selected = index;
                    if again {
                        self.mode = Mode::Options(0);
                    }
                }
                (Some(Hit::Entry(index)), Mode::Normal) => {
                    let again = self.column == Column::Backlog && self.backlog_selected == index;
                    self.column = Column::Backlog;
                    self.backlog_selected = index;
                    if again {
                        self.mode = Mode::Options(0);
                    }
                }
                (Some(Hit::Choice(index)), Mode::Options(_)) => {
                    if let Some(key) = self.choices().get(index).map(|choice| choice.key) {
                        self.mode = Mode::Normal;
                        self.act(key);
                    }
                }
                (Some(Hit::Commit(index)), Mode::ConfirmPush { selected, .. }) => *selected = index,
                (None, Mode::Options(_) | Mode::Message(_) | Mode::Help) => self.mode = Mode::Normal,
                _ => {}
            },
            _ => {}
        }
    }

    fn handle(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) && self.at_rest() {
            self.page = if self.page == Page::Work { Page::Areas } else { Page::Work };
            return;
        }
        if self.page == Page::Areas {
            let leaving = matches!(key.code, KeyCode::Esc | KeyCode::Char('q'));
            match self.areas.handle(key, terminal) {
                Effect::Left => self.quit = self.overlay,
                Effect::Unhandled if leaving => self.quit = self.overlay,
                _ => {}
            }
            return;
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.handle_normal(key),
            Mode::Options(index) => {
                let count = self.choices().len();
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::Options(index.saturating_sub(1)),
                    KeyCode::Down | KeyCode::Char('j') => self.mode = Mode::Options((index + 1).min(count - 1)),
                    KeyCode::Enter => {
                        if let Some(choice) = self.choices().get(index) {
                            let key = choice.key;
                            self.act(key);
                        }
                    }
                    KeyCode::Char(character) if self.choices().iter().any(|choice| choice.key == character) => self.act(character),
                    KeyCode::Esc => {}
                    _ => self.mode = Mode::Options(index),
                }
            }
            Mode::Help | Mode::Message(_) => {}
            working @ Mode::Working { .. } => self.mode = working,
            Mode::ConfirmReset { path, .. } => {
                if key.code == KeyCode::Char('y') {
                    self.show(tasks::reset(&path, None));
                }
            }
            Mode::ConfirmDrop { path, name, force } => {
                if key.code == KeyCode::Char('y') {
                    match tasks::drop(&path, None, force) {
                        Err(error) if !force && error.contains("--force") => {
                            self.mode = Mode::ConfirmDrop { path, name: format!("{name}: {error}"), force: true };
                        }
                        result => self.show(result),
                    }
                }
            }
            Mode::ConfirmPush { path, commits, selected } => match key.code {
                KeyCode::Char('y') => {
                    let (sender, result) = mpsc::channel();
                    thread::spawn(move || {
                        let _ = sender.send(tasks::push(&path, None));
                    });
                    self.mode = Mode::Working { label: format!("Pushing {} and closing the task…", plural(commits.len(), "commit")), result };
                }
                KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::ConfirmPush { path, commits, selected: selected.saturating_sub(1) },
                KeyCode::Down | KeyCode::Char('j') => {
                    let selected = (selected + 1).min(commits.len().saturating_sub(1));
                    self.mode = Mode::ConfirmPush { path, commits, selected };
                }
                KeyCode::Char('s') if commits.len() > 1 => {
                    let (sender, receiver) = mpsc::channel();
                    let draft_path = path.clone();
                    thread::spawn(move || {
                        let _ = sender.send(tasks::draft_squash_message(&draft_path));
                    });
                    let mut editor = MessageEditor::new(path, commits, 0, Rewrite::Squash, "");
                    editor.text.set_placeholder_text("Claude is drafting a message from the commits and the diff…");
                    editor.draft = Some(receiver);
                    self.mode = Mode::EditMessage(Box::new(editor));
                }
                KeyCode::Char('e') => {
                    let Some(commit) = commits.get(selected) else { return };
                    let (sha, message) = (commit.sha.clone(), commit.message.clone());
                    self.mode = Mode::EditMessage(Box::new(MessageEditor::new(path, commits, selected, Rewrite::Reword(sha), &message)));
                }
                _ => {}
            },
            Mode::EditMessage(mut editor) => {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Esc => {
                        let MessageEditor { path, commits, selected, .. } = *editor;
                        self.mode = Mode::ConfirmPush { path, commits, selected };
                    }
                    KeyCode::Char('s') if control => {
                        let message = editor.message();
                        if message.is_empty() {
                            self.mode = Mode::EditMessage(editor);
                            return;
                        }
                        let MessageEditor { path, selected, rewrite, .. } = *editor;
                        let result = match &rewrite {
                            Rewrite::Reword(sha) => tasks::reword(&path, sha, &message),
                            Rewrite::Squash => tasks::squash(&path, &message),
                        }.and_then(|_| tasks::commits(&path));
                        match result {
                            Ok(fresh) => {
                                let selected = if matches!(rewrite, Rewrite::Squash) { 0 } else { selected.min(fresh.len().saturating_sub(1)) };
                                self.mode = Mode::ConfirmPush { path, commits: fresh, selected };
                            }
                            Err(error) => self.show(Err(error)),
                        }
                    }
                    KeyCode::Char('o') if control => {
                        if let Ok(Some(message)) = edit_in_editor(terminal, &editor.message()) {
                            editor.set_text(&message);
                        }
                        self.mode = Mode::EditMessage(editor);
                    }
                    _ => {
                        editor.text.input(key);
                        self.mode = Mode::EditMessage(editor);
                    }
                }
            }
            Mode::Input { purpose, label, mut buffer } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.submit(purpose, buffer.trim().to_string(), terminal),
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
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Left | KeyCode::Right => {
                self.column = if self.column == Column::Work { Column::Backlog } else { Column::Work };
            }
            KeyCode::Esc | KeyCode::Char('q') if self.overlay => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Enter => {
                let has_selection = match self.column {
                    Column::Work => !self.tasks.is_empty(),
                    Column::Backlog => !self.backlog.entries.is_empty(),
                };
                if has_selection {
                    self.mode = Mode::Options(0);
                }
            }
            KeyCode::Char('R') => self.refresh_backlog(),
            KeyCode::Char('s') => {
                self.mode = Mode::Input { purpose: Purpose::TaskName, label: "New task: name".to_string(), buffer: String::new() };
            }
            KeyCode::Char(character) if self.choices().iter().any(|choice| choice.key == character) => self.act(character),
            _ => {}
        }
    }

    fn refresh_backlog(&mut self) {
        let result = backlog::refresh_prompt(&self.sources).and_then(|prompt| match self.lead_pane() {
            Some(pane) => herdr::prompt_agent(&pane, &prompt).map(|_| "Asked the lead to refresh the backlog; it updates here when it is saved.".to_string()),
            None => Err("No lead is running; prefix+space starts it.".to_string()),
        });
        self.show(result);
    }

    fn act(&mut self, key: char) {
        match self.column {
            Column::Backlog => match key {
                'g' => {
                    if let Some(entry) = self.selected_entry() {
                        let buffer = suggested_name(&entry.title);
                        self.mode = Mode::Input { purpose: Purpose::BacklogName(entry), label: "Start as task: name".to_string(), buffer };
                    }
                }
                'R' => self.refresh_backlog(),
                _ => {}
            },
            Column::Work => {
                let Some(item) = self.selected_task() else { return };
                match key {
                    'g' if item.agent.is_none() => {
                        let Some(path) = item.path.clone() else { return };
                        let (sender, result) = mpsc::channel();
                        let name = item.name.clone();
                        thread::spawn(move || {
                            let options = tasks::Start { prompt: None, focus: true, from: None, mode: None, backlog: None };
                            let _ = sender.send(tasks::start(&path, &name, &options));
                        });
                        self.mode = Mode::Working { label: format!("Restarting {}…", item.name), result };
                    }
                    'g' => {
                        if let Some(workspace) = &item.workspace {
                            let _ = herdr::focus_workspace(workspace);
                            if self.overlay {
                                self.quit = true;
                            }
                        }
                    }
                    'p' => {
                        if let Some(agent) = item.agent {
                            let label = format!("Message to {}", item.name);
                            self.mode = Mode::Input { purpose: Purpose::Message(agent.pane, item.name), label, buffer: String::new() };
                        }
                    }
                    'r' => self.open_review(&item),
                    't' | 'l' | 'd' | 'v' if busy(&item) => self.show(Err(format!("{} is still working; wait until it reports.", item.name))),
                    't' => {
                        if let Some(path) = item.path.clone() {
                            let label = "How should the commits look? (optional, enter to let Claude decide)".to_string();
                            self.mode = Mode::Input { purpose: Purpose::Tidy(path), label, buffer: String::new() };
                        }
                    }
                    'w' => self.toggle_server(&item),
                    'v' => {
                        if let Some(path) = &item.path {
                            self.show(crate::review::request(path, None));
                        }
                    }
                    'z' => {
                        if let Some(path) = item.path.clone() {
                            self.mode = Mode::ConfirmReset { path, name: item.name };
                        }
                    }
                    'l' | 'd' | 'x' => {
                        let Some(path) = item.path.clone() else { return };
                        match key {
                            'l' => match tasks::land(&path, None) {
                                Ok(_) => match tasks::commits(&path) {
                                    Ok(commits) => self.mode = Mode::ConfirmPush { path, commits, selected: 0 },
                                    Err(error) => self.show(Err(error)),
                                },
                                error => self.show(error),
                            },
                            'd' => self.show(tasks::done(&path, None)),
                            _ => self.mode = Mode::ConfirmDrop { path, name: item.name, force: false },
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // The dev server runs in the Space's server tab; stopping it is ctrl-c to that tab.
    fn toggle_server(&mut self, item: &Item) {
        let (Some(workspace), Some(path)) = (&item.workspace, &item.path) else {
            self.mode = Mode::Message(format!("{} has no Space open.", item.name));
            return;
        };
        let Some(command) = dev_server(item) else {
            self.mode = Mode::Message(format!("{} has no worktree.dev_server; kelpie config shows where to set one.", item.place));
            return;
        };
        let result = match busy_server_pane(workspace) {
            Some(pane) => herdr::send_keys(&pane, &["ctrl+c"]).map(|_| format!("Stopping the dev server in {}.", item.name)),
            None => {
                let port = repo::worktree_port(path);
                let on = port.map(|port| format!(" on port {port}")).unwrap_or_default();
                idle_tab_pane(workspace, SERVER_TAB).map(Ok).unwrap_or_else(|| herdr::create_tab(workspace, path, SERVER_TAB))
                    .and_then(|pane| herdr::run_in_pane(&pane, &server_command(&command, port)))
                    .map(|_| format!("Started {command}{on} in the server tab of {}'s Space.", item.name))
            }
        };
        self.show(result);
    }

    // The review opens next to the task's own Claude, so comments reach that agent.
    fn open_review(&mut self, item: &Item) {
        let (Some(path), Some(agent), Some(script)) = (&item.path, &item.agent, review_script()) else {
            self.mode = Mode::Message("A review needs a task with its Claude running.".to_string());
            return;
        };
        let output = std::process::Command::new("sh").arg(&script).arg("split").current_dir(path)
            .env("HERDR_PANE_ID", &agent.pane).output();
        match output {
            Ok(output) if output.status.success() => {
                if let Some(workspace) = &item.workspace {
                    let _ = herdr::focus_workspace(workspace);
                }
                if self.overlay {
                    self.quit = true;
                }
            }
            Ok(output) => self.mode = Mode::Message(String::from_utf8_lossy(&output.stderr).trim().to_string()),
            Err(error) => self.mode = Mode::Message(error.to_string()),
        }
    }

    fn submit(&mut self, purpose: Purpose, text: String, terminal: &mut DefaultTerminal) {
        match purpose {
            Purpose::TaskName | Purpose::BacklogName(_) if text.is_empty() => {}
            Purpose::TaskName => {
                let label = "What should Claude do? (optional)".to_string();
                self.mode = Mode::Input { purpose: Purpose::TaskPrompt(text), label, buffer: String::new() };
            }
            Purpose::TaskPrompt(name) => {
                let repo = self.any_repo();
                let prompt = (!text.is_empty()).then_some(text);
                self.show(start_in_background(&repo, &name, prompt.as_deref(), None));
            }
            Purpose::BacklogName(entry) => {
                let prompt = backlog::task_prompt(&entry, &self.sources);
                let known = self.repo_named(&entry.area);
                let result = known.map_or_else(|| outside(terminal, || repo::locate(&entry.area)), Ok)
                    .and_then(|repo| start_in_background(&repo, &text, Some(&prompt), Some(&entry.reference)));
                self.show(result);
            }
            Purpose::Tidy(path) => {
                let result = tasks::tidy(&path, None, &text);
                self.show(result);
            }
            Purpose::Message(_, _) if text.is_empty() => {}
            Purpose::Message(pane, name) => {
                let result = herdr::prompt_agent(&pane, &text).map(|_| format!("Sent to {name}."));
                self.show(result);
            }
        }
    }
}

// The board stays where it is while the task's worktree and Claude come up; the task shows under Work once its Space exists.
fn start_in_background(repo: &Path, name: &str, prompt: Option<&str>, backlog: Option<&str>) -> Result<String> {
    let dir = crate::lead::state_dir()?.join("starts");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let log = dir.join(format!("{}.log", name.replace('/', "-")));
    let output = std::fs::File::create(&log).map_err(|error| format!("{}: {error}", log.display()))?;
    let errors = output.try_clone().map_err(|error| error.to_string())?;
    let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
    command.arg("-C").arg(repo).args(["start", name, "--no-focus"]);
    if let Some(item) = backlog.filter(|item| !item.is_empty()) {
        command.args(["--backlog", item]);
    }
    if let Some(prompt) = prompt {
        command.arg(prompt);
    }
    // Its own process group, so closing the board's popup does not take the start down with it.
    command.stdin(std::process::Stdio::null()).stdout(output).stderr(errors).process_group(0);
    command.spawn().map_err(|error| format!("kelpie start: {error}"))?;
    Ok(format!("Starting {name} in the background; it shows under Work in a moment (output in {}).", log.display()))
}

// The user's editor on a commit message, in this pane; an empty result keeps the old message.
fn edit_in_editor(terminal: &mut DefaultTerminal, message: &str) -> Result<Option<String>> {
    let file = std::env::temp_dir().join(format!("kelpie-{}-COMMIT_EDITMSG", std::process::id()));
    let help = "\n\n# Edit the commit message. Lines starting with # are ignored; an empty message cancels.\n";
    std::fs::write(&file, format!("{message}{help}")).map_err(|error| error.to_string())?;
    restore_terminal();
    let status = Command::new("sh").args(["-c", "${VISUAL:-${EDITOR:-vi}} \"$1\"", "sh"]).arg(&file).status();
    *terminal = init_terminal();
    let edited = std::fs::read_to_string(&file).map_err(|error| error.to_string());
    let _ = std::fs::remove_file(&file);
    if !status.map_err(|error| error.to_string())?.success() {
        return Ok(None);
    }
    let message = edited?.lines().filter(|line| !line.starts_with('#')).collect::<Vec<_>>().join("\n").trim().to_string();
    Ok((!message.is_empty()).then_some(message))
}

pub fn init_terminal() -> DefaultTerminal {
    let terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    terminal
}

pub fn restore_terminal() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
}

// Leaves the TUI for a step that prints as it goes (creating a worktree takes a while), then comes back; the board shows the result.
pub fn outside<T>(terminal: &mut DefaultTerminal, step: impl FnOnce() -> Result<T>) -> Result<T> {
    restore_terminal();
    let result = step();
    *terminal = init_terminal();
    result
}

fn pill(text: String, color: Color, lit: bool) -> Vec<Span<'static>> {
    let color = if lit { color } else { FAINT };
    vec![Span::styled(format!(" {text} "), Style::new().fg(color).bg(MANTLE)), Span::raw(" ")]
}

fn header(app: &App, width: u16) -> (Line<'static>, Vec<(u16, u16, Page)>) {
    let count = |group: Group| app.tasks.iter().filter(|item| item.group() == group).count();
    let (needs, working, ready) = (count(Group::NeedsYou), count(Group::Working), count(Group::ReadyToPush));
    let mut spans = vec![Span::styled("kelpie", bold(MAUVE)), Span::raw("   ")];
    spans.extend(pill(format!("● {needs} need you"), PEACH, needs > 0));
    spans.extend(pill(format!("◐ {working} working"), BLUE, working > 0));
    spans.extend(pill(format!("✓ {ready} ready to push"), GREEN, ready > 0));
    let questions = app.tasks.iter().filter(|item| item.question.is_some()).count();
    spans.extend(pill(format!("? {}", plural(questions, "question")), YELLOW, questions > 0));
    let sources = backlog::label(&app.sources);
    let fresh = match app.page {
        Page::Work => format!("backlog: {sources}, {}   ", backlog::age(app.backlog.updated)),
        Page::Areas => format!("vault index: {}   ", backlog::age(app.areas.synced)),
    };
    let fresh = format!("{}{fresh}", fetch::failed_note(&app.fetch_failures));
    let tabs = [(" Work ", Page::Work), (" Areas ", Page::Areas)];
    let tabs_width: usize = tabs.iter().map(|(label, _)| label.chars().count()).sum();
    let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    let fresh = if used + fresh.chars().count() + tabs_width < width as usize { fresh } else { String::new() };
    let mut places = Vec::new();
    if used + fresh.chars().count() + tabs_width <= width as usize {
        spans.push(Span::raw(" ".repeat(width as usize - used - fresh.chars().count() - tabs_width)));
        spans.push(Span::styled(fresh, style(MUTED)));
        let mut x = width as usize - tabs_width;
        for (label, page) in tabs {
            let lit = app.page == page;
            spans.push(Span::styled(label, if lit { bold(MAUVE).bg(SURFACE) } else { style(MUTED) }));
            places.push((x as u16, label.chars().count() as u16, page));
            x += label.chars().count();
        }
    }
    (Line::from(spans), places)
}

pub fn column_title(title: &str, right: String, focused: bool, width: usize) -> Vec<Line<'static>> {
    let gap = width.saturating_sub(title.chars().count() + right.chars().count()).max(1);
    vec![
        Line::from(vec![Span::styled(title.to_string(), bold(if focused { MAUVE } else { MUTED })), Span::raw(" ".repeat(gap)), Span::styled(right, style(MUTED))]),
        Line::from(Span::styled("─".repeat(width), style(SURFACE))),
    ]
}

pub fn group_line(title: &str, count: usize, color: Color) -> Line<'static> {
    Line::from(vec![Span::styled(title.to_string(), bold(color)), Span::styled(format!("  {count}"), style(FAINT))])
}

// The name gives way when the column is narrow; the facts stay readable on the right.
pub fn row(selected: bool, focused: bool, icon: Span<'static>, name: &str, place: Option<&str>, facts: &str, width: usize) -> Line<'static> {
    let place_text = place.map(|place| if facts.is_empty() { place.to_string() } else { format!("{place} · ") }).unwrap_or_default();
    let wanted_right = place_text.chars().count() + facts.chars().count();
    let name_floor = name.chars().count().min(width * 45 / 100);
    let right_width = wanted_right.min(width.saturating_sub(name_floor + 5));
    let name_width = width.saturating_sub(right_width + 5);
    let shown: String = if name.chars().count() > name_width && name_width > 1 {
        name.chars().take(name_width - 1).collect::<String>() + "…"
    } else {
        name.to_string()
    };
    let padding = name_width.saturating_sub(shown.chars().count());
    let bar = if selected { Span::styled("▌", style(if focused { MAUVE } else { FAINT })) } else { Span::raw(" ") };
    let name_style = if selected && focused { bold(TEXT) } else { style(TEXT) };
    let facts: String = facts.chars().take(right_width.saturating_sub(place_text.chars().count())).collect();
    let line = Line::from(vec![
        bar, icon, Span::raw(" "), Span::styled(shown, name_style), Span::raw(" ".repeat(padding + 2)),
        Span::styled(place_text, style(TEAL)), Span::styled(facts, style(MUTED)),
    ]);
    if selected && focused { line.style(Style::new().bg(SURFACE)) } else { line }
}

// The selected row with its whole name, wrapped, and the facts on its first line.
fn wrapped_row(icon: Span<'static>, name: &str, facts: &str, width: usize) -> Vec<Line<'static>> {
    let name_width = width.saturating_sub(facts.chars().count() + 5);
    let chunks = wrap(name, name_width, 4);
    let highlight = Style::new().bg(SURFACE);
    chunks.into_iter().enumerate().map(|(index, chunk)| {
        if index == 0 {
            let padding = name_width.saturating_sub(chunk.chars().count());
            Line::from(vec![
                Span::styled("▌", style(MAUVE)), icon.clone(), Span::raw(" "), Span::styled(chunk, bold(TEXT)),
                Span::raw(" ".repeat(padding + 2)), Span::styled(facts.to_string(), style(MUTED)),
            ]).style(highlight)
        } else {
            Line::from(vec![Span::raw("   "), Span::styled(chunk, bold(TEXT))]).style(highlight)
        }
    }).collect()
}

// A line's background only covers its text, so highlighted lines are padded to the column's width.
pub fn fill(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    lines.into_iter().map(|mut line| {
        if line.style.bg == Some(SURFACE) && line.width() < width {
            let missing = width - line.width();
            line.spans.push(Span::raw(" ".repeat(missing)));
        }
        line
    }).collect()
}

pub fn chips(chips: Vec<(String, Color)>) -> Line<'static> {
    let mut spans = vec![Span::raw("   ")];
    for (text, color) in chips {
        spans.push(Span::styled(format!(" {text} "), Style::new().fg(color).bg(MANTLE)));
        spans.push(Span::raw(" "));
    }
    Line::from(spans).style(Style::new().bg(SURFACE))
}

fn ago(at: u64) -> String {
    match messages::age(at).as_str() {
        "now" => "just now".to_string(),
        age => format!("{age} ago"),
    }
}

// What the task's last report asks of the user, under its row.
fn you_line(next: &str, highlighted: bool, width: usize) -> Line<'static> {
    let text = wrap(next, width.saturating_sub(8), 1).into_iter().next().unwrap_or_default();
    let line = Line::from(vec![Span::raw("   "), Span::styled("you: ", style(PEACH)), Span::styled(text, style(SUBTEXT))]);
    if highlighted { line.style(Style::new().bg(SURFACE)) } else { line }
}

pub fn card_text(label: &str, text: &str, accent: Color, width: usize, max_lines: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![Span::raw("   "), Span::styled("┃ ", style(accent)), Span::styled(label.to_string(), style(MUTED))])];
    for text in wrap(text, width.saturating_sub(6), max_lines) {
        lines.push(Line::from(vec![Span::raw("   "), Span::styled("┃ ", style(accent)), Span::styled(text, style(SUBTEXT))]));
    }
    lines.into_iter().map(|line| line.style(Style::new().bg(SURFACE))).collect()
}

// A question the task asked through kelpie, as it asked it: the question, then each option on its own lines.
pub fn question_text(message: &messages::Message) -> String {
    let Some(question) = &message.question else { return message.text.clone() };
    let mut lines = vec![message.text.clone()];
    for (index, (label, meaning)) in question.options.iter().enumerate() {
        let recommended = if index + 1 == question.recommend { " (recommended)" } else { "" };
        lines.push(format!("{}. {label}{recommended}: {meaning}", index + 1));
    }
    if let Some(plan) = &question.plan {
        lines.push(format!("plan: {plan}"));
    }
    lines.join("\n")
}

// A question asked through kelpie shows from the log; a prompt on screen outranks it.
fn asked_question(item: &Item) -> Option<&messages::Message> {
    item.question.as_ref().filter(|_| item.agent.as_ref().is_none_or(|agent| agent.status != "blocked"))
}

fn question_card(message: &messages::Message, width: usize) -> Vec<Line<'static>> {
    let place = if message.question.as_ref().is_some_and(|question| question.given) { "with the lead" } else { "queued for the lead" };
    let text = question_text(message);
    let mut paragraphs = text.lines();
    let mut lines = card_text(&format!("asks · {} · {place}", ago(message.at)), paragraphs.next().unwrap_or_default(), PEACH, width, 4);
    for paragraph in paragraphs {
        lines.extend(card_text("", paragraph, PEACH, width, 2).into_iter().skip(1));
    }
    lines
}

pub fn options_hint() -> Line<'static> {
    Line::from(vec![Span::raw("   "), Span::styled("⏎", style(MAUVE)), Span::styled(" options", style(MUTED))]).style(Style::new().bg(SURFACE))
}

type Rows = Vec<(usize, usize, Hit)>;

fn work_lines(app: &mut App, width: usize) -> (Vec<Line<'static>>, usize, Rows) {
    let focused = app.column == Column::Work;
    let mut lines = column_title("WORK", format!("{}{}", plural(app.tasks.len(), "task"), free_worktrees(&app.checkouts)), focused, width);
    let mut selected_line = 0;
    let mut current: Option<Group> = None;
    let tasks = app.tasks.clone();
    let mut rows = Vec::new();
    for (index, item) in tasks.iter().enumerate() {
        if current != Some(item.group()) {
            if current.is_some() {
                lines.push(Line::default());
            }
            let count = tasks.iter().filter(|other| other.group() == item.group()).count();
            lines.push(group_line(item.group().title(), count, group_color(item.group())));
            current = Some(item.group());
        }
        let selected = index == app.work_selected;
        if selected {
            selected_line = lines.len();
        }
        let place = (app.checkouts.len() > 1).then_some(item.place.as_str());
        let start = lines.len();
        lines.push(row(selected, focused, status_icon(item), &item.name, place, &row_facts(item), width));
        if let Some(next) = item.next_for_you() {
            lines.push(you_line(next, selected && focused, width));
        }
        if selected && focused {
            lines.extend(task_card(app, item, width));
        }
        rows.push((start, lines.len(), Hit::Task(index)));
    }
    if tasks.is_empty() {
        lines.push(Line::from(Span::styled("No tasks. s starts one, or pick one from the backlog.", style(MUTED))));
    }
    let leftovers: Vec<String> = app.checkouts.iter().filter(|checkout| checkout.changed > 0)
        .map(|checkout| format!("{}: {} changed outside tasks", checkout.name, plural(checkout.changed, "file"))).collect();
    if !leftovers.is_empty() {
        lines.push(Line::default());
        for leftover in leftovers {
            lines.push(Line::from(vec![Span::styled("! ", style(YELLOW)), Span::styled(leftover, style(MUTED))]));
        }
    }
    (lines, selected_line, rows)
}

fn verdict_chip(shown: &crate::review::Shown) -> Option<(String, Color)> {
    if shown.running {
        return Some(("review running".to_string(), BLUE));
    }
    let verdict = shown.verdict.as_ref()?;
    let label = if verdict.ok { "review ok".to_string() } else { format!("review: {}", verdict.summary()) };
    Some(match (shown.stale, verdict.ok) {
        (true, _) => (format!("{label} (stale)"), MUTED),
        (false, true) => (label, GREEN),
        (false, false) => (label, PEACH),
    })
}

fn task_card(app: &mut App, item: &Item, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut facts = Vec::new();
    if let Some(absent) = absence(item) {
        facts.push((absent.to_string(), YELLOW));
    }
    if let Some(git) = &item.git {
        if git.ahead > 0 {
            facts.push((format!("{} to push", plural(git.ahead, "commit")), SUBTEXT));
        }
        if git.changed > 0 {
            facts.push((format!("{} changed", plural(git.changed, "file")), SUBTEXT));
        }
        if git.behind > 0 {
            facts.push((format!("{} behind {}", git.behind, git.upstream.trim_start_matches("origin/")), YELLOW));
        }
    }
    let halves: Vec<String> = app.tasks.iter().filter(|other| other.name == item.name && other.place != item.place).map(|other| other.place.clone()).collect();
    if !halves.is_empty() {
        facts.push((format!("same task in {}", halves.join(", ")), TEAL));
    }
    if let Some((read, total)) = item.review {
        facts.push((format!("review {read}/{total} read"), if read == total { GREEN } else { SUBTEXT }));
    }
    if let Some(chip) = item.verdict.as_ref().and_then(verdict_chip) {
        facts.push(chip);
    }
    if item.server {
        facts.push(("dev server running".to_string(), GREEN));
    }
    if !facts.is_empty() {
        lines.push(chips(facts));
    }
    if let Some(asked) = asked_question(item) {
        lines.extend(question_card(asked, width));
    } else {
        match &item.agent {
            Some(agent) => {
                let screen = app.agent_screen(&agent.pane);
                let question = |text: String| tail(&text, width.saturating_sub(6) * 6);
                let (label, text, accent, max_lines) = match &item.report {
                    _ if agent.status == "blocked" => ("asks".to_string(), question(last_message(&screen)), PEACH, 6),
                    Some(report) if report.report.as_ref().is_some_and(|report| report.state == "needs-you") && !last_message(&screen).is_empty() => {
                        (format!("asks · reported {}", ago(report.at)), question(last_message(&screen)), PEACH, 6)
                    }
                    Some(report) => (format!("reported {}", ago(report.at)), report.text.clone(), FAINT, 3),
                    None => match recap(&screen) {
                        recap if !recap.is_empty() => ("recap (no report yet)".to_string(), recap, FAINT, 3),
                        _ => ("last said".to_string(), last_message(&screen), FAINT, 3),
                    },
                };
                if !text.is_empty() {
                    lines.extend(card_text(&label, &text, accent, width, max_lines));
                }
            }
            None => lines.extend(card_text("no Claude", "No Claude is running in its Space.", FAINT, width, 1)),
        }
    }
    lines.push(options_hint());
    lines
}

fn backlog_lines(app: &App, width: usize) -> (Vec<Line<'static>>, usize, Rows) {
    let focused = app.column == Column::Backlog;
    let mut lines = column_title("BACKLOG", format!("{} items", app.backlog.entries.len()), focused, width);
    let mut selected_line = 0;
    let mut current: Option<&str> = None;
    let mut rows = Vec::new();
    for (index, entry) in app.backlog.entries.iter().enumerate() {
        if current != Some(entry.priority.as_str()) {
            if current.is_some() {
                lines.push(Line::default());
            }
            let count = app.backlog.entries.iter().filter(|other| other.priority == entry.priority).count();
            lines.push(group_line(&entry.priority.to_uppercase(), count, priority_color(&entry.priority)));
            current = Some(&entry.priority);
        }
        let selected = index == app.backlog_selected;
        if selected {
            selected_line = lines.len();
        }
        let icon = Span::styled(priority_icon(&entry.priority), style(priority_color(&entry.priority)));
        let start = lines.len();
        if selected && focused {
            lines.extend(wrapped_row(icon, &entry.title, &entry.age, width));
            let mut facts = vec![(entry.area.clone(), TEAL), (if entry.source == "linear" { entry.reference.clone() } else { "vault note".to_string() }, SUBTEXT)];
            if !entry.age.is_empty() {
                facts.push((format!("{} old", entry.age), SUBTEXT));
            }
            lines.push(chips(facts));
            if !entry.summary.is_empty() {
                lines.extend(card_text("about", &entry.summary, FAINT, width, 2));
            }
            lines.extend(card_text("starts as", &suggested_name(&entry.title), FAINT, width, 2));
            lines.push(options_hint());
        } else {
            lines.push(row(selected, focused, icon, &entry.title, None, &entry.age, width));
        }
        rows.push((start, lines.len(), Hit::Entry(index)));
    }
    if app.backlog.entries.is_empty() {
        let empty = if app.sources.is_empty() { backlog::not_configured() } else { format!("Empty. R asks the lead to fill it from {}.", backlog::from(&app.sources)) };
        lines.push(Line::from(Span::styled(empty, style(MUTED))));
    }
    (lines, selected_line, rows)
}

pub fn render_column<H: Copy>(frame: &mut Frame, area: Rect, lines: Vec<Line<'static>>, selected_line: usize, rows: Vec<(usize, usize, H)>, hits: &mut Vec<(Rect, H)>) {
    let lines = fill(lines, area.width as usize);
    let scroll = selected_line.saturating_sub(area.height.saturating_sub(6) as usize);
    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), area);
    for (start, end, hit) in rows {
        let top = start.max(scroll) - scroll;
        let bottom = end.saturating_sub(scroll).min(area.height as usize);
        if bottom > top {
            hits.push((Rect::new(area.x, area.y + top as u16, area.width, (bottom - top) as u16), hit));
        }
    }
}

fn hint_line() -> Line<'static> {
    Line::from([("⏎", "options"), ("← →", "work · backlog"), ("↑ ↓", "move"), ("tab", "work · areas"), ("s", "new task"), ("?", "all keys")]
        .into_iter().flat_map(|(key, label)| [Span::styled(key.to_string(), style(MAUVE)), Span::styled(format!(" {label}    "), style(MUTED))])
        .collect::<Vec<_>>())
}

pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

// Sized to the wrapped text, so a long line in a narrow pane cannot push the last line, often the keys, out of the window.
pub fn modal(frame: &mut Frame, title: String, lines: Vec<Line<'static>>, width: u16) -> Rect {
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), bold(MAUVE)));
    let width = width.min(frame.area().width.saturating_sub(4));
    let rows = Paragraph::new(lines.clone()).wrap(Wrap { trim: false }).line_count(width.saturating_sub(4));
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    let area = centered(frame.area(), width, rows as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
    area
}

pub fn options_lines(choices: Vec<Choice>, index: usize, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (position, choice) in choices.iter().enumerate() {
        let active = position == index;
        let label_style = if active { bold(MAUVE) } else { style(TEXT) };
        let key = choice.key.to_string();
        let gap = width.saturating_sub(choice.label.chars().count() + key.chars().count() + 2).max(1);
        let background = if active { Style::new().bg(SURFACE) } else { Style::new() };
        lines.push(Line::from(vec![
            Span::raw(if active { "▸ " } else { "  " }), Span::styled(choice.label.clone(), label_style),
            Span::raw(" ".repeat(gap)), Span::styled(key, style(MUTED)),
        ]).style(background));
        lines.push(Line::from(vec![Span::raw("  "), Span::styled(choice.about.clone(), style(MUTED))]).style(background));
    }
    lines.push(Line::from(Span::styled("↑ ↓ choose · ⏎ run · esc close", style(FAINT))));
    lines
}

fn help_lines() -> Vec<Line<'static>> {
    [
        ("⏎", "options for the selected task or backlog item"), ("← →", "switch between work and backlog"), ("tab", "switch between the work and areas pages"), ("↑ ↓ j k", "move"),
        ("s", "start a new task"), ("g", "go to the task's Space, or start the backlog item"), ("p", "message the task's Claude"),
        ("r", "open the task's review next to its Claude"), ("w", "start or stop the task's dev server (worktree.dev_server)"), ("v", "request an independent, read-only review of the task's commits"), ("l", "ready to push: rebase and check (on a task)"), ("d", "done: after you pushed"),
        ("z", "reset the task: its work goes, its backlog item returns (asks first)"), ("x", "drop the task (asks first)"), ("R", "ask the lead to refresh the backlog"), ("esc", "close this, or the board when it is an overlay"),
    ].into_iter().map(|(key, text)| Line::from(vec![Span::styled(format!("{key:<9}"), style(MAUVE)), Span::styled(text.to_string(), style(TEXT))])).collect()
}

fn draw(frame: &mut Frame, app: &mut App) {
    app.hits.clear();
    let area = frame.area().inner(Margin { horizontal: 2, vertical: 1 });
    let [top, _, body, bottom] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).areas(area);
    let (line, tabs) = header(app, top.width);
    frame.render_widget(Paragraph::new(line), top);
    let tab_hits: Vec<(Rect, Hit)> = tabs.into_iter().map(|(x, width, page)| (Rect::new(top.x + x, top.y, width, 1), Hit::Tab(page))).collect();
    if app.page == Page::Areas {
        frame.render_widget(Paragraph::new(area_page::Page::hint_line()), bottom);
        app.areas.draw(frame, body);
        app.hits = if app.areas.is_normal() { tab_hits } else { Vec::new() };
        return;
    }
    frame.render_widget(Paragraph::new(hint_line()), bottom);

    if let Some(error) = &app.error {
        frame.render_widget(Paragraph::new(Span::styled(error.clone(), style(RED))).wrap(Wrap { trim: false }), body);
    } else if !app.loaded {
        frame.render_widget(Paragraph::new(Span::styled("looking around…", style(MUTED))), body);
    } else if body.width >= 86 {
        let [work_area, _, backlog_area] = Layout::horizontal([Constraint::Percentage(54), Constraint::Length(4), Constraint::Min(20)]).areas(body);
        let (work, work_selected, work_rows) = work_lines(app, work_area.width as usize);
        render_column(frame, work_area, work, work_selected, work_rows, &mut app.hits);
        let (backlog, backlog_selected, backlog_rows) = backlog_lines(app, backlog_area.width as usize);
        render_column(frame, backlog_area, backlog, backlog_selected, backlog_rows, &mut app.hits);
    } else {
        let width = body.width as usize;
        let (mut lines, work_selected, mut rows) = work_lines(app, width);
        let offset = lines.len() + 1;
        let (backlog, backlog_selected, backlog_rows) = backlog_lines(app, width);
        lines.push(Line::default());
        lines.extend(backlog);
        rows.extend(backlog_rows.into_iter().map(|(start, end, hit)| (start + offset, end + offset, hit)));
        let selected = if app.column == Column::Work { work_selected } else { offset + backlog_selected };
        render_column(frame, body, lines, selected, rows, &mut app.hits);
    }

    let mut modal_hits = Vec::new();
    match &app.mode {
        Mode::Normal => {}
        Mode::Options(index) => {
            let title = match app.column {
                Column::Work => app.selected_task().map(|item| item.name).unwrap_or_default(),
                Column::Backlog => app.selected_entry().map(|entry| entry.title).unwrap_or_default(),
            };
            let title: String = title.chars().take(52).collect();
            let count = app.choices().len();
            let area = modal(frame, title, options_lines(app.choices(), *index, 56), 62);
            for choice in 0..count {
                let y = area.y + 1 + 2 * choice as u16;
                if y + 1 < area.bottom() {
                    modal_hits.push((Rect::new(area.x, y, area.width, 2), Hit::Choice(choice)));
                }
            }
        }
        Mode::Help => {
            modal(frame, "keys".to_string(), help_lines(), 72);
        }
        Mode::Message(text) => {
            let mut lines: Vec<Line> = text.lines().map(|line| Line::from(Span::styled(line.to_string(), style(TEXT)))).collect();
            lines.push(Line::from(Span::styled("any key to close", style(MUTED))));
            modal(frame, "kelpie".to_string(), lines, 96);
        }
        Mode::ConfirmReset { name, .. } => {
            modal(frame, "reset".to_string(), vec![
                Line::from(Span::styled(name.clone(), style(TEXT))),
                Line::from(Span::styled("Its changes and commits go, its Claude stops, its worktree is freed and its Space closes. The backlog item it came from returns.", style(MUTED))),
                Line::default(),
                Line::from(vec![Span::styled("Reset this task?", style(YELLOW)), Span::styled("  y / n", style(MUTED))]),
            ], 72);
        }
        Mode::ConfirmDrop { name, force, .. } => {
            let question = if *force { "Discard its changes and commits?" } else { "Drop this task?" };
            modal(frame, "drop".to_string(), vec![
                Line::from(Span::styled(name.clone(), style(TEXT))),
                Line::default(),
                Line::from(vec![Span::styled(question, style(YELLOW)), Span::styled("  y / n", style(MUTED))]),
            ], 72);
        }
        Mode::ConfirmPush { commits, selected, .. } => {
            let mut lines: Vec<Line> = Vec::new();
            for (index, commit) in commits.iter().enumerate() {
                let subject = commit.message.lines().next().unwrap_or_default().to_string();
                let active = index == *selected;
                lines.push(Line::from(vec![
                    Span::raw(if active { "▸ " } else { "  " }),
                    Span::styled(commit.sha.chars().take(7).collect::<String>(), style(MUTED)),
                    Span::raw(" "),
                    Span::styled(subject, if active { bold(MAUVE) } else { style(TEXT) }),
                ]).style(if active { Style::new().bg(SURFACE) } else { Style::new() }));
            }
            if let Some(commit) = commits.get(*selected) {
                lines.push(Line::default());
                for line in commit.message.lines() {
                    let wrapped = if line.is_empty() { vec![String::new()] } else { wrap(line, 88, 6) };
                    lines.extend(wrapped.into_iter().map(|part| Line::from(Span::styled(part, style(TEXT)))));
                }
            }
            lines.push(Line::default());
            lines.push(Line::from(vec![
                Span::styled("Push these and close the task?  ", style(YELLOW)),
                Span::styled(if commits.len() > 1 { "y push · e edit message · s squash into one · ↑ ↓ commit · esc cancel" } else { "y push · e edit message · esc cancel" }, style(MUTED)),
            ]));
            let area = modal(frame, "push".to_string(), lines, 96);
            for index in 0..commits.len() {
                let y = area.y + 1 + index as u16;
                if y < area.bottom().saturating_sub(1) {
                    modal_hits.push((Rect::new(area.x, y, area.width, 1), Hit::Commit(index)));
                }
            }
        }
        Mode::Working { label, .. } => {
            modal(frame, "kelpie".to_string(), vec![Line::from(Span::styled(label.clone(), style(TEXT)))], 72);
        }
        Mode::EditMessage(editor) => {
            let area = centered(frame.area(), 80, frame.area().height.saturating_sub(4).min(24));
            frame.render_widget(Clear, area);
            let title = match editor.rewrite { Rewrite::Reword(_) => " commit message ", Rewrite::Squash => " squashed commit message " };
            let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
                .title(Span::styled(title, bold(MAUVE)));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let [text_area, footer] = Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(inner);
            frame.render_widget(&editor.text, text_area);
            let subject = editor.text.lines().first().map(|line| line.chars().count()).unwrap_or(0);
            let count = Span::styled(format!("subject {subject}/72  "), style(if subject > 72 { YELLOW } else { MUTED }));
            let keys = if editor.draft.is_some() { "drafting…  " } else { "" };
            frame.render_widget(Paragraph::new(vec![
                Line::default(),
                Line::from(vec![count, Span::styled(keys, style(YELLOW)), Span::styled("ctrl-s save · esc back · ctrl-o open in your editor", style(MUTED))]),
            ]), footer);
        }
        Mode::Input { label, buffer, .. } => {
            modal(frame, label.clone(), vec![
            Line::from(vec![Span::styled(buffer.clone(), style(TEXT)), Span::styled("▏", style(MAUVE))]),
            Line::from(Span::styled("enter to continue · esc to cancel", style(MUTED))),
        ], 72);
        }
    }
    if matches!(app.mode, Mode::Normal) {
        app.hits.extend(tab_hits);
    } else {
        app.hits = modal_hits;
    }
}

const RESTART_STATE: &str = "KELPIE_BOARD_STATE";

fn built(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().filter(|metadata| metadata.len() > 0).and_then(|metadata| metadata.modified().ok())
}

// A rebuilt binary takes over the open board, keeping the column and selection; it must look the same on two checks so a half-written one never runs.
struct Rebuild {
    path: Option<PathBuf>,
    started: Option<SystemTime>,
    last: Option<SystemTime>,
    checked: Instant,
}

impl Rebuild {
    fn new() -> Self {
        let path = std::env::current_exe().ok();
        let started = path.as_deref().and_then(built);
        Self { path, started, last: started, checked: Instant::now() }
    }

    fn ready(&mut self) -> bool {
        if self.checked.elapsed() < Duration::from_secs(2) {
            return false;
        }
        self.checked = Instant::now();
        let now = self.path.as_deref().and_then(built);
        let ready = now.is_some() && now != self.started && now == self.last;
        self.last = now;
        ready
    }

    fn restart(&self, app: &App) -> String {
        let Some(path) = &self.path else { return "Cannot tell where kelpie's binary is".to_string() };
        let column = match (app.page, app.column) {
            (Page::Areas, _) => "areas",
            (_, Column::Backlog) => "backlog",
            _ => "work",
        };
        let task = app.selected_task().map(|item| item.name).unwrap_or_default();
        let state = format!("{column}\t{}\t{task}", app.backlog_selected);
        restore_terminal();
        let error = Command::new(path).args(std::env::args_os().skip(1)).env(RESTART_STATE, state).exec();
        format!("Restarting on the rebuilt kelpie failed: {error}")
    }
}

pub fn run(overlay: bool) -> Result<String> {
    let sources = config::sources()?;
    let (snapshots, refresh) = spawn_refresher();
    let mut app = App {
        column: Column::Work, tasks: Vec::new(), checkouts: Vec::new(), backlog: backlog::load(), work_selected: 0, backlog_selected: 0,
        mode: Mode::Normal, overlay, loaded: false, error: None, screens: HashMap::new(), refresh, quit: false, wanted_task: None, hits: Vec::new(),
        page: Page::Work, areas: area_page::Page::new(sources.clone()), sources, fetch_failures: Vec::new(),
    };
    if let Ok(state) = std::env::var(RESTART_STATE) {
        let mut parts = state.splitn(3, '\t');
        match parts.next() {
            Some("backlog") => app.column = Column::Backlog,
            Some("areas") => app.page = Page::Areas,
            _ => {}
        }
        app.backlog_selected = parts.next().and_then(|index| index.parse().ok()).unwrap_or(0);
        app.wanted_task = parts.next().filter(|name| !name.is_empty()).map(str::to_string);
    }
    let mut rebuild = Rebuild::new();
    let mut fetcher = Fetcher::new();
    let mut terminal = init_terminal();
    let result = (|| -> Result<()> {
        while !app.quit {
            if let Mode::EditMessage(editor) = &mut app.mode {
                editor.poll_draft();
            }
            if let Mode::Working { result, .. } = &app.mode
                && let Ok(result) = result.try_recv()
            {
                app.show(result);
            }
            while let Ok(update) = snapshots.try_recv() {
                match update {
                    Ok(snapshot) => {
                        let previous = app.selected_task().map(|item| (item.name, item.place));
                        app.tasks = snapshot.items.into_iter().filter(Item::is_task).collect();
                        app.areas.tasks = app.tasks.clone();
                        app.checkouts = snapshot.checkouts;
                        let wanted = app.wanted_task.take();
                        let position = previous.and_then(|(name, place)| app.tasks.iter().position(|item| item.name == name && item.place == place))
                            .or_else(|| wanted.and_then(|name| app.tasks.iter().position(|item| item.name == name)));
                        app.work_selected = position.unwrap_or(app.work_selected.min(app.tasks.len().saturating_sub(1)));
                        if let Ok(sources) = config::sources() {
                            app.sources = sources;
                        }
                        app.backlog = backlog::load();
                        app.backlog_selected = app.backlog_selected.min(app.backlog.entries.len().saturating_sub(1));
                        app.error = None;
                    }
                    Err(error) => app.error = Some(error),
                }
                app.loaded = true;
            }
            if app.page == Page::Areas {
                app.areas.tick();
            }
            let mut repos: Vec<PathBuf> = app.tasks.iter().filter_map(|item| item.repo.clone()).chain(app.areas.repos()).collect();
            repos.sort();
            repos.dedup();
            if fetcher.tick(repos) {
                let _ = app.refresh.send(());
                app.areas.refresh();
            }
            app.fetch_failures = fetcher.failures();
            if matches!(app.mode, Mode::Normal | Mode::Message(_) | Mode::Help) && app.areas.is_idle() && rebuild.ready() {
                let error = rebuild.restart(&app);
                terminal = init_terminal();
                app.mode = Mode::Message(error);
            }
            terminal.draw(|frame| draw(frame, &mut app)).map_err(|error| error.to_string())?;
            if event::poll(Duration::from_millis(250)).map_err(|error| error.to_string())? {
                match event::read().map_err(|error| error.to_string())? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => app.handle(key, &mut terminal),
                    Event::Mouse(mouse) => app.mouse(mouse, &mut terminal),
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    restore_terminal();
    result.map(|_| String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modal_keeps_its_last_line_when_a_long_line_wraps() {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 20)).unwrap();
        let lines = vec![Line::from("x ".repeat(80)), Line::default(), Line::from("y push · esc cancel")];
        terminal.draw(|frame| { modal(frame, "push".to_string(), lines, 96); }).unwrap();
        let screen: String = terminal.backend().buffer().content().iter().map(|cell| cell.symbol()).collect();
        assert!(screen.contains("y push · esc cancel"), "{screen}");
    }

    #[test]
    fn a_suggested_name_keeps_letters_with_diacritics() {
        assert_eq!(suggested_name("kelpie – återanvänd worktree tar med förra uppgiftens databas"), "kelpie-ateranvand-worktree-tar");
        assert_eq!(crate::tasks::slug("Föräldraledighet över jul").unwrap(), "foraldraledighet-over-jul");
    }

    #[test]
    fn free_worktrees_are_counted_per_repo() {
        let checkout = |name: &str, free_slots| Checkout { name: name.to_string(), changed: 0, free_slots, busy_slots: 0 };
        assert_eq!(free_worktrees(&[checkout("kelpie", 1), checkout("web", 0), checkout("api", 2)]), " · free worktrees: kelpie 1, api 2");
        assert_eq!(free_worktrees(&[checkout("web", 0)]), "");
    }

    #[test]
    fn a_working_task_offers_no_tidy_push_or_done() {
        let keys = |item: &Item| task_choices(Some(item), None).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(&crate::status::tests::task(Some("working"), None, 1)), "gprzx");
        assert_eq!(keys(&crate::status::tests::task(Some("idle"), Some("ready"), 1)), "gprvtldzx");
        assert_eq!(keys(&crate::status::tests::task(Some("idle"), Some("ready"), 0)), "gprtldzx", "no commits, nothing to review");
    }

    #[test]
    fn a_ready_task_the_lead_has_not_approved_still_offers_push_and_close() {
        let mut unchecked = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        unchecked.git.as_mut().unwrap().approved = false;
        assert!(unchecked.group() == crate::status::Group::NeedsYou);
        assert!(task_choices(Some(&unchecked), None).iter().any(|choice| choice.key == 'l'));
        assert_eq!(row_facts(&unchecked), "1 commit · rules not checked by the lead");
    }

    #[test]
    fn a_task_without_its_claude_shows_why_and_g_restarts_it() {
        let gone = crate::status::tests::task(None, None, 1);
        assert_eq!(row_facts(&gone), "no Space · 1 commit");
        assert_eq!(task_choices(Some(&gone), None)[0].label, "Restart it");
        let mut shell_only = crate::status::tests::task(None, None, 0);
        shell_only.workspace = Some("w1".to_string());
        assert_eq!(row_facts(&shell_only), "no Claude · nothing yet");
        let running = crate::status::tests::task(Some("idle"), None, 0);
        assert_eq!(row_facts(&running), "nothing yet");
        assert_eq!(task_choices(Some(&running), None)[0].label, "Go to its Space");
    }

    #[test]
    fn w_is_offered_only_with_a_dev_server() {
        let keys = |dev_server: Option<&str>| task_choices(None, dev_server).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(None), "gprtldzx");
        assert_eq!(keys(Some("bin/dev")), "gprwtldzx");
        let start = task_choices(None, Some("npm run dev"));
        assert_eq!((start[3].label.as_str(), start[3].about.as_str()), ("Start the dev server", "npm run dev in a tab of its Space"));
    }

    #[test]
    fn the_card_shows_the_review_verdict_and_marks_it_stale_once_head_moves() {
        let shown = |ok: bool, findings: usize, stale: bool, running: bool| crate::review::Shown {
            verdict: Some(crate::review::Verdict { ok, sha: "abc".to_string(), findings }), stale, running, file: PathBuf::from("/state/verdicts/billing@web.md"),
        };
        assert_eq!(verdict_chip(&shown(true, 0, false, false)), Some(("review ok".to_string(), GREEN)));
        assert_eq!(verdict_chip(&shown(false, 3, false, false)), Some(("review: 3 findings".to_string(), PEACH)));
        assert_eq!(verdict_chip(&shown(false, 3, true, false)), Some(("review: 3 findings (stale)".to_string(), MUTED)));
        assert_eq!(verdict_chip(&shown(true, 0, false, true)), Some(("review running".to_string(), BLUE)));
        assert_eq!(verdict_chip(&crate::review::Shown { verdict: None, stale: false, running: false, file: PathBuf::new() }), None);
    }

    #[test]
    fn w_exports_the_worktrees_port_only_when_one_is_recorded() {
        assert_eq!(server_command("bin/dev", Some(3002)), "export PORT=3002; bin/dev");
        assert_eq!(server_command("bin/dev", None), "bin/dev");
    }

    #[test]
    fn the_question_is_the_end_of_claudes_last_message_in_either_bullet() {
        let screen = "⏺ Read the note.\n\n⏺ Fråga 1: Ska push spärras?\n  A (min rekommendation): visa redo först efter godkännande.\n\n✻ Crunched for 24s\n❯ \n";
        assert_eq!(last_message(screen), "Fråga 1: Ska push spärras? A (min rekommendation): visa redo först efter godkännande.");
        assert_eq!(tail("one two three four five", 10), "…four five");
        assert_eq!(tail("short", 10), "short");
    }

    #[test]
    fn the_card_shows_a_question_asked_through_kelpie_from_the_log_and_a_prompt_on_screen_over_it() {
        let log = r#"{"id":"q1","at":10,"from":"billing@web","to":"lead","text":"Which queue?","state":"asked","question":{"options":[{"label":"Sidekiq","meaning":"reuse the jobs"},{"label":"Solid Queue","meaning":"one less service"}],"recommend":2,"plan":"/state/plans/billing@web-10.md"}}"#;
        let asked = messages::fold(log).remove(0);
        assert_eq!(question_text(&asked), "Which queue?\n1. Sidekiq: reuse the jobs\n2. Solid Queue (recommended): one less service\nplan: /state/plans/billing@web-10.md");
        let mut item = crate::status::tests::task(Some("idle"), Some("needs-you"), 0);
        assert!(asked_question(&item).is_none(), "a question in plain text keeps the screen fallback");
        item.question = Some(asked.clone());
        assert_eq!(asked_question(&item).map(|message| message.id.as_str()), Some("q1"));
        item.agent.as_mut().unwrap().status = "blocked".to_string();
        assert!(asked_question(&item).is_none());
        let card: String = question_card(&asked, 60).iter().flat_map(|line| line.spans.iter().map(|span| span.content.to_string())).collect();
        assert!(card.contains("queued for the lead") && card.contains("2. Solid Queue (recommended): one less service"), "{card}");
    }

    #[test]
    fn the_recap_is_the_last_one_on_screen_with_its_continuation() {
        let screen = "※ recap: An older one.\n\n● Done.\n\n※ recap: We're adding search; the branch is rebased. Next, leave your\n  comments in the split. (disable recaps in /config)\n\n❯ \n";
        assert_eq!(recap(screen), "We're adding search; the branch is rebased. Next, leave your comments in the split.");
        assert_eq!(recap("● Nothing to recap.\n"), "");
    }
}
