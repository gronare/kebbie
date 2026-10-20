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
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};
use ratatui_textarea::{TextArea, WrapMode};

use crate::area_page::{self, Effect};
use crate::backlog::{self, Backlog, Entry};
use crate::config::{self, Source};
use crate::fetch::{self, Fetcher};
use crate::herdr;
use crate::messages::{self, Message};
use crate::permit::{self, Permit};
use crate::peers;
use crate::repo::{self, name_of};
use crate::report;
use crate::sh::Result;
use crate::status::{self, Checkout, Group, Item};
use crate::tasks;
use crate::usage::Usage;
use crate::{lead, questions, usage, vault};

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


#[derive(Clone, Copy, PartialEq, Debug)]
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

fn task_choices(item: Option<&Item>, dev_server: Option<&str>, dev_worker: Option<&str>) -> Vec<Choice> {
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
    if let Some(command) = dev_worker {
        choices.push(if item.is_some_and(|item| item.worker) {
            choice('b', "Stop the worker", "ctrl-c to its worker tab")
        } else {
            choice('b', "Start the worker", &format!("{command} in a tab of its Space"))
        });
    }
    if !item.is_some_and(busy) {
        if item.and_then(|item| item.git.as_ref()).is_some_and(|git| git.ahead > 0) {
            choices.push(if item.is_some_and(Item::awaiting_review) {
                choice('v', "Withdraw from review", "stop its reviewer and forget its verdict: push is offered again, and the lead is told")
            } else {
                choice('v', "Send to review", "a separate, read-only Claude reviews its commits; the task waits under To review until it passes them")
            });
        }
        choices.push(choice('t', "Tidy commits", "its Claude reshapes them by the rules skill"));
        if !item.is_some_and(Item::awaiting_review) {
            choices.push(choice('l', "Push", "rebase, then its card and commits: push them all and close, or the first few and keep it open"));
        }
        choices.push(choice('d', "Done", "already pushed elsewhere: close it and free the worktree"));
    }
    if item.is_some_and(Item::is_task) {
        choices.push(if item.is_some_and(|item| item.permit == Some(Permit::Task)) {
            choice('a', "Take back push permission", "the lead no longer pushes it; it waits for you under Ready to push")
        } else {
            choice('a', "Let the lead push it", "the lead may push this task itself once it is landed and approved; done, drop and reset take it back")
        });
    }
    choices.push(choice('z', "Reset", "stop it for good: its work goes, its backlog item comes back (asks first)"));
    choices.push(choice('x', "Drop", "throw the task away (asks first)"));
    choices
}

// While its Claude works, the branch can still move, so the steps that rewrite, push or close it wait.
fn busy(item: &Item) -> bool {
    item.group() == Group::Working
}

fn ongoing_choices(note: &Ongoing) -> Vec<Choice> {
    vec![match note.task {
        Some(_) => choice('g', "Go to its task", "select the task that works on this note"),
        None => choice('g', "Resume as task", "a worktree, a Space and Claude with the note as its prompt"),
    }]
}

fn ongoing_entry(note: &Ongoing) -> Entry {
    Entry {
        source: "vault".to_string(), title: note.title.clone(), priority: "none".to_string(), area: note.area.clone(),
        reference: note.key.clone(), summary: String::new(), age: String::new(),
    }
}

fn backlog_choices(sources: &[Source]) -> Vec<Choice> {
    let mut choices = vec![
        choice('g', "Start as task", "a worktree, a Space and Claude with this as its prompt"),
        choice('R', "Refresh the backlog", &backlog::refresh_label(sources)),
    ];
    if sources.contains(&Source::Local) {
        choices.push(choice('a', "Add an item", "a title and a priority, appended to the local backlog file"));
    }
    choices
}

enum Purpose {
    TaskName,
    TaskPrompt(String),
    Message(String, String),
    BacklogName(Entry),
    BacklogItem,
    BacklogPriority(String),
    Tidy(PathBuf),
}

enum Mode {
    Normal,
    Options(usize),
    Input { purpose: Purpose, label: String, buffer: String },
    Message(String),
    ConfirmDrop { path: PathBuf, name: String, force: bool },
    ConfirmReset { path: PathBuf, name: String },
    ConfirmPush { path: PathBuf, commits: Vec<tasks::Commit>, selected: usize, card: Option<(report::Card, bool)> },
    EditMessage(Box<MessageEditor>),
    Working { label: String, result: Receiver<Result<String>> },
    Questions(Box<QuestionWindow>),
    Help,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Task(usize),
    Entry(usize),
    Choice(usize),
    Commit(usize),
    Tab(Page),
    Question(usize),
    Answer(usize),
    Messages(Option<usize>),
    Key(KeyCode),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Fold {
    Hidden,
    Line,
    Open,
}

impl Fold {
    fn next(self) -> Self {
        match self {
            Fold::Hidden => Fold::Line,
            Fold::Line => Fold::Open,
            Fold::Open => Fold::Hidden,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Fold::Hidden => "hidden",
            Fold::Line => "line",
            Fold::Open => "open",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    List,
    Plan,
    Options,
    Comment,
}

impl Part {
    fn next(self, back: bool) -> Self {
        let order = [Part::List, Part::Plan, Part::Options, Part::Comment];
        let at = order.iter().position(|part| *part == self).unwrap_or(0);
        order[if back { (at + 3) % 4 } else { (at + 1) % 4 }]
    }
}

struct QuestionWindow {
    id: String,
    selected: usize,
    option: usize,
    scroll: u16,
    part: Part,
    comment: TextArea<'static>,
    notice: Option<(bool, String)>,
    answering: Option<(String, Receiver<Result<String>>)>,
}

fn comment_field() -> TextArea<'static> {
    let mut comment = TextArea::default();
    comment.set_placeholder_text("comment (optional)");
    comment.set_cursor_line_style(Style::new());
    comment.set_style(style(TEXT));
    comment
}

impl QuestionWindow {
    fn new(questions: &[Message], selected: usize) -> Self {
        let mut window = Self { id: String::new(), selected: 0, option: 0, scroll: 0, part: Part::Options, comment: comment_field(), notice: None, answering: None };
        window.select(questions, selected);
        window
    }

    // Another question never inherits this one's option or comment.
    fn select(&mut self, questions: &[Message], selected: usize) {
        self.selected = selected.min(questions.len().saturating_sub(1));
        let message = questions.get(self.selected);
        let id = message.map(|message| message.id.clone()).unwrap_or_default();
        if id != self.id {
            self.comment = comment_field();
        }
        self.id = id;
        self.option = message.and_then(|message| message.question.as_ref()).map_or(0, |question| question.recommend.saturating_sub(1));
        self.scroll = 0;
    }

    // The list changes under the window as questions open and close; the selection follows its question, not its row.
    fn follow(&mut self, questions: &[Message]) {
        match questions.iter().position(|message| message.id == self.id) {
            Some(at) => self.selected = at,
            None if self.id.is_empty() => self.select(questions, 0),
            None => {
                self.select(questions, self.selected);
                self.notice = Some((false, "The question you had open was answered or closed elsewhere; its choice and comment are cleared.".to_string()));
            }
        }
    }

    fn comment(&self) -> String {
        self.comment.lines().join("\n").trim().to_string()
    }
}

#[derive(Default)]
struct Detail {
    branch: String,
    commits: Vec<tasks::Commit>,
    changed: Vec<String>,
    card: Option<(report::Card, bool)>,
    dev_server: bool,
    dev_worker: bool,
}

enum Graphics {
    Ready(Picker),
    Off,
}

struct ImageAt {
    line: usize,
    path: PathBuf,
    rows: u16,
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
    card: Option<(report::Card, bool)>,
    rewrite: Rewrite,
    text: TextArea<'static>,
    draft: Option<Receiver<Result<String>>>,
}

impl MessageEditor {
    fn new(path: PathBuf, commits: Vec<tasks::Commit>, selected: usize, card: Option<(report::Card, bool)>, rewrite: Rewrite, message: &str) -> Self {
        let mut editor = Self { path, commits, selected, card, rewrite, text: TextArea::default(), draft: None };
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
    messages: Vec<Message>,
    questions: Vec<Message>,
    usage: Option<Usage>,
    ongoing: Vec<Ongoing>,
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
    links: Vec<Link>,
    page: Page,
    areas: area_page::Page,
    sources: Vec<Source>,
    fetch_failures: Vec<String>,
    away: bool,
    messages: Vec<Message>,
    fold: Fold,
    seen: u64,
    fold_file: Option<PathBuf>,
    in_messages: bool,
    message_back: usize,
    questions: Vec<Message>,
    usage: Option<Usage>,
    ongoing: Vec<Ongoing>,
    details: HashMap<PathBuf, std::rc::Rc<Detail>>,
    detail_requests: Option<Sender<DetailRequest>>,
    detail_results: Option<Receiver<(PathBuf, Detail)>>,
    asked_details: HashMap<PathBuf, Instant>,
    graphics: Graphics,
    images: HashMap<(PathBuf, u16), Option<Protocol>>,
    plans: HashMap<String, Text<'static>>,
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
        Group::ToReview => TEAL,
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
    parts.extend(item.review_note());
    if item.permit == Some(Permit::Task) {
        parts.push("lead may push".to_string());
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

#[derive(Clone)]
struct Ongoing {
    key: String,
    title: String,
    area: String,
    task: Option<String>,
}

fn load_ongoing() -> Vec<Ongoing> {
    if !config::sources().is_ok_and(|sources| sources.contains(&Source::Vault)) {
        return Vec::new();
    }
    let Ok(notes) = vault::Index::open().and_then(|index| index.ongoing()) else { return Vec::new() };
    let links = tasks::started_links();
    notes.into_iter().map(|note| {
        let task = links.iter().find(|(_, key)| **key == note.key).map(|(address, _)| address.clone());
        Ongoing { key: note.key, title: note.title, area: note.area, task }
    }).collect()
}

fn spawn_refresher() -> (Receiver<std::result::Result<Snapshot, String>>, Sender<()>) {
    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    let (refresh_tx, refresh_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        loop {
            let _ = messages::flush();
            let snapshot = status::gather().and_then(|items| {
                let questions = questions::open_now(&messages::load()).into_iter().cloned().collect();
                Ok(Snapshot { checkouts: status::checkouts()?, items, messages: messages::recent(200), questions, usage: usage::load(), ongoing: load_ongoing() })
            });
            if snapshot_tx.send(snapshot).is_err() {
                return;
            }
            let _ = refresh_rx.recv_timeout(Duration::from_secs(3));
        }
    });
    (snapshot_rx, refresh_tx)
}

type DetailRequest = (PathBuf, Option<PathBuf>);

fn spawn_detailer() -> (Sender<DetailRequest>, Receiver<(PathBuf, Detail)>) {
    let (request_tx, request_rx) = mpsc::channel::<DetailRequest>();
    let (detail_tx, detail_rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok((path, repo)) = request_rx.recv() {
            if detail_tx.send((path.clone(), gather_detail(&path, repo.as_deref()))).is_err() {
                return;
            }
        }
    });
    (request_tx, detail_rx)
}

fn gather_detail(path: &Path, repo: Option<&Path>) -> Detail {
    let settings = repo.and_then(|repo| config::resolve(Some(repo)).ok()).map(|resolved| resolved.settings);
    let changed = repo::git(path, &["status", "--porcelain"]).unwrap_or_default().lines()
        .filter_map(|line| line.get(3..)).map(|name| name.rsplit(" -> ").next().unwrap_or(name).to_string()).collect();
    Detail {
        branch: repo::git(path, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default(),
        commits: tasks::commits(path).unwrap_or_default(),
        changed,
        card: report::card_for(path),
        dev_server: settings.as_ref().is_some_and(|settings| settings.dev_server.value.is_some()),
        dev_worker: settings.as_ref().is_some_and(|settings| settings.dev_worker.value.is_some()),
    }
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

#[derive(Clone, Copy, PartialEq)]
enum Process {
    Server,
    Worker,
}

impl Process {
    // scripts/sync.sh reports a running process from a busy pane in a tab with this label.
    fn tab(self, settings: Option<&config::Settings>) -> String {
        match (self, settings) {
            (Process::Server, Some(settings)) => settings.server_name.value.clone(),
            (Process::Worker, Some(settings)) => settings.worker_name.value.clone(),
            (Process::Server, None) => config::DEFAULT_SERVER_NAME.to_string(),
            (Process::Worker, None) => config::DEFAULT_WORKER_NAME.to_string(),
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Process::Server => "the dev server",
            Process::Worker => "the worker",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Process::Server => "worktree.dev_server",
            Process::Worker => "worktree.dev_worker",
        }
    }

    fn command(self, settings: &config::Settings) -> Option<String> {
        match self {
            Process::Server => settings.dev_server.value.clone(),
            Process::Worker => settings.dev_worker.value.clone(),
        }
    }
}

fn settings_of(item: &Item) -> Option<config::Settings> {
    Some(config::resolve(Some(item.repo.as_deref()?)).ok()?.settings)
}

// export rather than a PORT= prefix, so the port reaches every part of a compound command; bash, zsh and fish all take it.
fn server_command(command: &str, port: Option<u16>) -> String {
    port.map_or_else(|| command.to_string(), |port| format!("export PORT={port}; {command}"))
}

fn busy_tab_pane(workspace: &str, label: &str) -> Option<String> {
    let tabs: Vec<String> = herdr::tabs_in(workspace).ok()?.into_iter()
        .filter(|tab| tab["label"] == label).filter_map(|tab| tab["tab_id"].as_str().map(str::to_string)).collect();
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
        self.backlog.entries.get(self.backlog_selected.checked_sub(self.ongoing.len())?).cloned()
    }

    fn choices(&self) -> Vec<Choice> {
        match self.column {
            Column::Work => {
                let item = self.tasks.get(self.work_selected);
                let settings = item.and_then(settings_of);
                let command = |process: Process| settings.as_ref().and_then(|settings| process.command(settings));
                let mut choices = task_choices(item, command(Process::Server).as_deref(), command(Process::Worker).as_deref());
                if item.is_some_and(|item| self.question_of(item).is_some()) {
                    choices.insert(0, choice('q', "Answer its question", "the question window, at this task's question"));
                }
                choices
            }
            Column::Backlog if self.backlog_selected < self.ongoing.len() => ongoing_choices(&self.ongoing[self.backlog_selected]),
            Column::Backlog => backlog_choices(&self.sources),
        }
    }

    fn question_of(&self, item: &Item) -> Option<usize> {
        let address = peers::address(&item.name, &item.place);
        self.questions.iter().position(|question| question.from == address)
    }

    fn task_of(&self, from: &str) -> Option<&Item> {
        self.tasks.iter().find(|item| peers::address(&item.name, &item.place) == from)
    }

    fn open_questions(&mut self, at: Option<usize>) {
        if self.questions.is_empty() {
            self.mode = Mode::Message("No questions are waiting.".to_string());
            return;
        }
        self.mode = Mode::Questions(Box::new(QuestionWindow::new(&self.questions, at.unwrap_or(0))));
    }

    fn set_fold(&mut self, fold: Fold) {
        self.fold = fold;
        self.in_messages &= fold == Fold::Open;
        if fold == Fold::Open {
            self.seen = self.seen.max(self.messages.last().map_or(0, |message| message.at));
        }
        save_fold(self.fold_file.as_deref(), fold, self.seen);
    }

    fn detail(&mut self, item: &Item) -> Option<std::rc::Rc<Detail>> {
        let path = item.path.clone()?;
        let asked = self.asked_details.get(&path).is_some_and(|at| at.elapsed() < Duration::from_secs(4));
        if !asked && let Some(requests) = &self.detail_requests {
            let _ = requests.send((path.clone(), item.repo.clone()));
            self.asked_details.insert(path.clone(), Instant::now());
        }
        self.details.get(&path).cloned()
    }

    // None means the caller draws the path instead.
    fn image_rows(&mut self, path: &Path, width: u16) -> Option<u16> {
        if !path.is_file() {
            return None;
        }
        let Graphics::Ready(picker) = &self.graphics else { return None };
        let key = (path.to_path_buf(), width);
        if !self.images.contains_key(&key) {
            let decoded = image::ImageReader::open(path).ok().and_then(|reader| reader.with_guessed_format().ok()).and_then(|reader| reader.decode().ok());
            let protocol = decoded.and_then(|decoded| picker.new_protocol(decoded, ratatui::layout::Size::new(width, 14), Resize::Fit(None)).ok());
            self.images.insert(key.clone(), protocol);
        }
        self.images.get(&key)?.as_ref().map(|protocol| protocol.size().height)
    }

    fn backlog_len(&self) -> usize {
        self.ongoing.len() + self.backlog.entries.len()
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

    fn press_m(&mut self) {
        match (self.fold, self.in_messages) {
            (Fold::Open, false) => self.enter_messages(),
            (Fold::Line, _) => {
                self.set_fold(Fold::Open);
                self.enter_messages();
            }
            (fold, _) => self.set_fold(fold.next()),
        }
    }

    fn enter_messages(&mut self) {
        self.in_messages = true;
        self.message_back = 0;
    }

    fn selected_message(&self) -> Option<&Message> {
        self.messages.len().checked_sub(1 + self.message_back).and_then(|index| self.messages.get(index))
    }

    fn handle_messages(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.message_back = (self.message_back + 1).min(self.messages.len().saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.message_back = self.message_back.saturating_sub(1),
            KeyCode::Esc => self.in_messages = false,
            KeyCode::Char('m') => self.press_m(),
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('q') => self.open_questions(None),
            KeyCode::Enter => {
                if let Some(message) = self.selected_message() {
                    self.mode = Mode::Message(format!("{} {} → {}\n\n{}", clock(message.at), message.from, message.to, message.text));
                }
            }
            _ => {}
        }
    }

    fn move_selection(&mut self, step: isize) {
        let (selected, length) = match self.column {
            Column::Work => (&mut self.work_selected, self.tasks.len()),
            Column::Backlog => (&mut self.backlog_selected, self.ongoing.len() + self.backlog.entries.len()),
        };
        *selected = selected.saturating_add_signed(step).min(length.saturating_sub(1));
    }

    fn hit_at(&self, column: u16, row: u16) -> Option<Hit> {
        self.hits.iter().find(|(area, _)| area.contains(Position::new(column, row))).map(|(_, hit)| *hit)
    }

    // A click selects a row, a click on the selected row opens its options, and the wheel moves through whatever is in front.
    fn mouse(&mut self, event: MouseEvent, terminal: &mut DefaultTerminal) {
        let hit = self.hit_at(event.column, event.row);
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
        if let Some(key) = self.work_mouse(event, hit) {
            self.handle(key, terminal);
        }
    }

    fn work_mouse(&mut self, event: MouseEvent, hit: Option<Hit>) -> Option<KeyEvent> {
        let normal = matches!(self.mode, Mode::Normal);
        match event.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Mode::EditMessage(editor) = &mut self.mode {
                    editor.text.input(event);
                    return None;
                }
                if let Mode::Questions(window) = &mut self.mode {
                    window.scroll = if event.kind == MouseEventKind::ScrollUp { window.scroll.saturating_sub(3) } else { window.scroll.saturating_add(3) };
                    return None;
                }
                match hit {
                    Some(Hit::Task(_)) if normal => (self.column, self.in_messages) = (Column::Work, false),
                    Some(Hit::Entry(_)) if normal => (self.column, self.in_messages) = (Column::Backlog, false),
                    Some(Hit::Messages(_)) if normal && self.fold == Fold::Open => self.in_messages = true,
                    _ => {}
                }
                Some(KeyEvent::from(if event.kind == MouseEventKind::ScrollUp { KeyCode::Up } else { KeyCode::Down }))
            }
            MouseEventKind::Down(MouseButton::Left) => {
                match (hit, &mut self.mode) {
                    (Some(Hit::Task(index)), Mode::Normal) => {
                        let again = self.column == Column::Work && self.work_selected == index && !self.in_messages;
                        (self.column, self.work_selected, self.in_messages) = (Column::Work, index, false);
                        if again {
                            self.mode = Mode::Options(0);
                        }
                    }
                    (Some(Hit::Entry(index)), Mode::Normal) => {
                        let again = self.column == Column::Backlog && self.backlog_selected == index && !self.in_messages;
                        (self.column, self.backlog_selected, self.in_messages) = (Column::Backlog, index, false);
                        if again {
                            self.mode = Mode::Options(0);
                        }
                    }
                    (Some(Hit::Messages(None)), Mode::Normal) if self.fold == Fold::Open => self.in_messages = true,
                    (Some(Hit::Messages(None)), Mode::Normal) => self.press_m(),
                    (Some(Hit::Messages(Some(index))), Mode::Normal) => {
                        let back = self.messages.len().saturating_sub(index + 1);
                        let again = self.in_messages && self.message_back == back;
                        (self.in_messages, self.message_back) = (true, back);
                        if again {
                            return Some(KeyEvent::from(KeyCode::Enter));
                        }
                    }
                    (Some(Hit::Key(code)), Mode::Normal) => {
                        self.in_messages &= code == KeyCode::Char('m');
                        return Some(KeyEvent::from(code));
                    }
                    (Some(Hit::Choice(index)), Mode::Options(_)) => {
                        if let Some(key) = self.choices().get(index).map(|choice| choice.key) {
                            self.mode = Mode::Normal;
                            self.act(key);
                        }
                    }
                    (Some(Hit::Commit(index)), Mode::ConfirmPush { selected, .. }) => *selected = index,
                    (Some(Hit::Question(index)), Mode::Questions(window)) => window.select(&self.questions, index),
                    (Some(Hit::Answer(index)), Mode::Questions(window)) => {
                        window.option = index;
                        window.part = Part::Options;
                    }
                    (None, Mode::Options(_) | Mode::Message(_) | Mode::Help) => self.mode = Mode::Normal,
                    _ => {}
                }
                None
            }
            _ => None,
        }
    }

    fn handle(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if self.at_rest() {
            match key.code {
                KeyCode::Tab | KeyCode::BackTab => {
                    if let Err(error) = lead::focus_toggle() {
                        self.mode = Mode::Message(error);
                    }
                    return;
                }
                KeyCode::Char('1') => return self.page = Page::Work,
                KeyCode::Char('2') => return self.page = Page::Areas,
                _ => {}
            }
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
            Mode::Questions(window) => self.handle_questions(window, key),
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
            Mode::ConfirmPush { path, commits, selected, card } => match key.code {
                KeyCode::Char(key @ ('y' | 'p')) => {
                    let keep_open = key == 'p';
                    let through = if keep_open { commits.get(selected) } else { commits.last() };
                    let Some(through) = through.map(|commit| commit.sha.clone()) else { return };
                    let count = if keep_open { selected + 1 } else { commits.len() };
                    let (sender, result) = mpsc::channel();
                    thread::spawn(move || {
                        let _ = sender.send(tasks::push(&path, None, Some(&through), keep_open));
                    });
                    let label = if keep_open { format!("Pushing {}; the task stays open…", plural(count, "commit")) } else { format!("Pushing {} and closing the task…", plural(count, "commit")) };
                    self.mode = Mode::Working { label, result };
                }
                KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::ConfirmPush { path, commits, selected: selected.saturating_sub(1), card },
                KeyCode::Down | KeyCode::Char('j') => {
                    let selected = (selected + 1).min(commits.len().saturating_sub(1));
                    self.mode = Mode::ConfirmPush { path, commits, selected, card };
                }
                KeyCode::Char('s') if commits.len() > 1 => {
                    let (sender, receiver) = mpsc::channel();
                    let draft_path = path.clone();
                    thread::spawn(move || {
                        let _ = sender.send(tasks::draft_squash_message(&draft_path));
                    });
                    let mut editor = MessageEditor::new(path, commits, 0, card, Rewrite::Squash, "");
                    editor.text.set_placeholder_text("Claude is drafting a message from the commits and the diff…");
                    editor.draft = Some(receiver);
                    self.mode = Mode::EditMessage(Box::new(editor));
                }
                KeyCode::Char('e') => {
                    let Some(commit) = commits.get(selected) else { return };
                    let (sha, message) = (commit.sha.clone(), commit.message.clone());
                    self.mode = Mode::EditMessage(Box::new(MessageEditor::new(path, commits, selected, card, Rewrite::Reword(sha), &message)));
                }
                _ => {}
            },
            Mode::EditMessage(mut editor) => {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Esc => {
                        let MessageEditor { path, commits, selected, card, .. } = *editor;
                        self.mode = Mode::ConfirmPush { path, commits, selected, card };
                    }
                    KeyCode::Char('s') if control => {
                        let message = editor.message();
                        if message.is_empty() {
                            self.mode = Mode::EditMessage(editor);
                            return;
                        }
                        let MessageEditor { path, selected, rewrite, card, .. } = *editor;
                        let result = match &rewrite {
                            Rewrite::Reword(sha) => tasks::reword(&path, sha, &message),
                            Rewrite::Squash => tasks::squash(&path, &message),
                        }.and_then(|_| tasks::commits(&path));
                        match result {
                            Ok(fresh) => {
                                let selected = if matches!(rewrite, Rewrite::Squash) { 0 } else { selected.min(fresh.len().saturating_sub(1)) };
                                self.mode = Mode::ConfirmPush { path, commits: fresh, selected, card };
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
        if self.in_messages {
            return self.handle_messages(key);
        }
        match key.code {
            KeyCode::Char('m') => self.press_m(),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Left | KeyCode::Right => {
                self.column = if self.column == Column::Work { Column::Backlog } else { Column::Work };
            }
            KeyCode::Esc if self.overlay => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('q') => self.open_questions(None),
            KeyCode::Enter => {
                let has_selection = match self.column {
                    Column::Work => !self.tasks.is_empty(),
                    Column::Backlog => self.backlog_len() > 0,
                };
                if has_selection {
                    self.mode = Mode::Options(0);
                }
            }
            KeyCode::Char('R') => self.refresh_backlog(),
            KeyCode::Char('A') => self.show(permit::away(Some(if permit::away_since().is_some() { "off" } else { "on" }))),
            KeyCode::Char('s') => {
                self.mode = Mode::Input { purpose: Purpose::TaskName, label: "New task: name".to_string(), buffer: String::new() };
            }
            KeyCode::Char(character) if self.choices().iter().any(|choice| choice.key == character) => self.act(character),
            _ => {}
        }
    }

    fn handle_questions(&mut self, mut window: Box<QuestionWindow>, key: KeyEvent) {
        let count = self.questions.len();
        let options = self.questions.get(window.selected).and_then(|message| message.question.as_ref()).map_or(0, |question| question.options.len());
        let typing = window.part == Part::Comment;
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Tab | KeyCode::BackTab => window.part = window.part.next(key.code == KeyCode::BackTab),
            KeyCode::PageUp => window.scroll = window.scroll.saturating_sub(10),
            KeyCode::PageDown => window.scroll = window.scroll.saturating_add(10),
            KeyCode::Enter if window.answering.is_some() => {}
            KeyCode::Enter => {
                let Some(message) = self.questions.get(window.selected) else { return };
                let comment = window.comment();
                let choice = (window.option < options).then_some(window.option);
                if choice.is_none() && comment.is_empty() {
                    window.part = Part::Comment;
                    window.notice = Some((false, "In your own words: write the answer in the comment field first.".to_string()));
                } else {
                    let (sender, result) = mpsc::channel();
                    let id = message.id.clone();
                    let asked = id.clone();
                    thread::spawn(move || {
                        let _ = sender.send(questions::answer(&asked, choice, &comment));
                    });
                    window.answering = Some((id, result));
                    window.notice = Some((true, "Answering…".to_string()));
                }
            }
            KeyCode::Char('g') if !typing => {
                let workspace = self.questions.get(window.selected).and_then(|message| self.task_of(&message.from)).and_then(|item| item.workspace.clone());
                match workspace {
                    Some(workspace) => {
                        let _ = herdr::focus_workspace(&workspace);
                        if self.overlay {
                            self.quit = true;
                        }
                        return;
                    }
                    None => window.notice = Some((false, "Its task has no Space open.".to_string())),
                }
            }
            KeyCode::Up | KeyCode::Down if typing => {}
            KeyCode::Up | KeyCode::Char('k') if !typing => match window.part {
                Part::List => window.select(&self.questions, window.selected.saturating_sub(1)),
                Part::Plan => window.scroll = window.scroll.saturating_sub(1),
                _ => window.option = window.option.saturating_sub(1),
            },
            KeyCode::Down | KeyCode::Char('j') if !typing => match window.part {
                Part::List => window.select(&self.questions, (window.selected + 1).min(count.saturating_sub(1))),
                Part::Plan => window.scroll = window.scroll.saturating_add(1),
                _ => window.option = (window.option + 1).min(options),
            },
            _ => {
                if !typing && !matches!(key.code, KeyCode::Char(_)) {
                    self.mode = Mode::Questions(window);
                    return;
                }
                window.part = Part::Comment;
                window.comment.input(key);
            }
        }
        self.mode = Mode::Questions(window);
    }

    fn poll_answer(&mut self) {
        let Mode::Questions(window) = &mut self.mode else { return };
        let Some((id, receiver)) = &window.answering else { return };
        let Ok(result) = receiver.try_recv() else { return };
        let id = id.clone();
        window.answering = None;
        let text = match result {
            Ok(text) => text,
            Err(error) => {
                window.notice = Some((false, error));
                return;
            }
        };
        self.questions.retain(|question| question.id != id);
        let _ = self.refresh.send(());
        if self.questions.is_empty() {
            self.mode = Mode::Message(text);
            return;
        }
        if let Mode::Questions(window) = &mut self.mode {
            let at = if window.id == id { window.selected } else { self.questions.iter().position(|question| question.id == window.id).unwrap_or(window.selected) };
            window.select(&self.questions, at);
            window.notice = Some((true, text));
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
        if self.column == Column::Backlog && let Some(note) = self.ongoing.get(self.backlog_selected).cloned() {
            match &note.task {
                Some(task) => {
                    if let Some(index) = self.tasks.iter().position(|item| &peers::address(&item.name, &item.place) == task) {
                        self.column = Column::Work;
                        self.work_selected = index;
                    }
                }
                None => {
                    let entry = ongoing_entry(&note);
                    let buffer = suggested_name(&entry.title);
                    self.mode = Mode::Input { purpose: Purpose::BacklogName(entry), label: "Resume as task: name".to_string(), buffer };
                }
            }
            return;
        }
        match self.column {
            Column::Backlog => match key {
                'g' => {
                    if let Some(entry) = self.selected_entry() {
                        let buffer = suggested_name(&entry.title);
                        self.mode = Mode::Input { purpose: Purpose::BacklogName(entry), label: "Start as task: name".to_string(), buffer };
                    }
                }
                'R' => self.refresh_backlog(),
                'a' => {
                    let label = "Add to the backlog: title (— summary and (area) optional)".to_string();
                    self.mode = Mode::Input { purpose: Purpose::BacklogItem, label, buffer: String::new() };
                }
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
                    'q' => {
                        let at = self.question_of(&item);
                        self.open_questions(at);
                    }
                    't' | 'l' | 'd' | 'v' if busy(&item) => self.show(Err(format!("{} is still working; wait until it reports.", item.name))),
                    'l' if item.awaiting_review() => self.show(Err(format!("{} is in review; push opens once its reviewer passes its commits.", item.name))),
                    't' => {
                        if let Some(path) = item.path.clone() {
                            let label = "How should the commits look? (optional, enter to let Claude decide)".to_string();
                            self.mode = Mode::Input { purpose: Purpose::Tidy(path), label, buffer: String::new() };
                        }
                    }
                    'w' => self.toggle(&item, Process::Server),
                    'b' => self.toggle(&item, Process::Worker),
                    'a' if item.is_task() => self.show(permit::toggle_task(&peers::address(&item.name, &item.place))),
                    'v' => {
                        if let Some(path) = &item.path {
                            self.show(if item.awaiting_review() { crate::review::withdraw(path, None) } else { crate::review::request(path, None) });
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
                                    Ok(commits) => {
                                        let card = report::card_for(&path);
                                        self.mode = Mode::ConfirmPush { path, commits, selected: 0, card };
                                    }
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

    // Each process runs in a tab of the Space named after it; stopping it is ctrl-c to that tab.
    fn toggle(&mut self, item: &Item, process: Process) {
        let (Some(workspace), Some(path)) = (&item.workspace, &item.path) else {
            self.mode = Mode::Message(format!("{} has no Space open.", item.name));
            return;
        };
        let settings = settings_of(item);
        let Some(command) = settings.as_ref().and_then(|settings| process.command(settings)) else {
            self.mode = Mode::Message(format!("{} has no {}; kelpie config shows where to set one.", item.place, process.key()));
            return;
        };
        let tab = process.tab(settings.as_ref());
        let tab = tab.as_str();
        let result = match busy_tab_pane(workspace, tab) {
            Some(pane) => herdr::send_keys(&pane, &["ctrl+c"]).map(|_| format!("Stopping {} in {}.", process.noun(), item.name)),
            None => {
                let port = repo::worktree_port(path);
                let on = port.map(|port| format!(" on port {port}")).unwrap_or_default();
                let url = (process == Process::Server).then(|| item.repo.as_deref().and_then(|repo| {
                    config::dev_url(settings.as_ref()?.dev_url.value.as_deref(), repo, path)
                })).flatten();
                let at = url.map(|url| format!(" at {url}")).unwrap_or_default();
                idle_tab_pane(workspace, tab).map(Ok).unwrap_or_else(|| herdr::create_tab(workspace, path, tab))
                    .and_then(|pane| herdr::run_in_pane(&pane, &server_command(&command, port)))
                    .map(|_| format!("Started {command}{on} in the {tab} tab of {}'s Space{at}.", item.name))
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
            Purpose::TaskName | Purpose::BacklogName(_) | Purpose::BacklogItem if text.is_empty() => {}
            Purpose::BacklogItem => {
                let label = "Priority: urgent, high, medium or low (empty for none)".to_string();
                self.mode = Mode::Input { purpose: Purpose::BacklogPriority(text), label, buffer: String::new() };
            }
            Purpose::BacklogPriority(item) => {
                let result = crate::backlog_file::add(Some(&text), &item);
                if result.is_ok() {
                    self.backlog = backlog::load();
                }
                self.show(result);
            }
            Purpose::BacklogName(entry) if entry.area.is_empty() => {
                self.show(Err(format!("\"{}\" names no area, so kelpie cannot tell which repo it is in; add the repo's folder name as (area) at the end of its line.", entry.title)));
            }
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
    static MOUSE_OFF_ON_PANIC: std::sync::Once = std::sync::Once::new();
    MOUSE_OFF_ON_PANIC.call_once(|| {
        let restore = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(std::io::stdout(), DisableMouseCapture);
            restore(info);
        }));
    });
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
    let (needs, working, review, ready) = (count(Group::NeedsYou), count(Group::Working), count(Group::ToReview), count(Group::ReadyToPush));
    let mut spans = vec![Span::styled("kelpie", bold(MAUVE)), Span::raw("   ")];
    spans.extend(pill(format!("● {needs} need you"), PEACH, needs > 0));
    spans.extend(pill(format!("◐ {working} working"), BLUE, working > 0));
    if review > 0 {
        spans.extend(pill(format!("◇ {review} to review"), TEAL, true));
    }
    spans.extend(pill(format!("✓ {ready} ready to push"), GREEN, ready > 0));
    let questions = app.questions.len();
    spans.extend(pill(format!("? {}", plural(questions, "question")), YELLOW, questions > 0));
    if app.away {
        spans.extend(pill("☾ away".to_string(), MAUVE, true));
    }
    let sources = backlog::label(&app.sources);
    let fresh = match app.page {
        Page::Work => format!("backlog: {sources}, {}   ", backlog::freshness(&app.sources, app.backlog.updated)),
        Page::Areas => format!("vault index: {}   ", backlog::age(app.areas.synced)),
    };
    let fresh = format!("{}{fresh}", fetch::failed_note(&app.fetch_failures));
    let tabs = [(" 1 Work ", Page::Work), (" 2 Areas ", Page::Areas)];
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

pub struct Link {
    shown: String,
    url: String,
}

// Spans drop control characters, so the OSC 8 escape goes into the drawn cells instead, each forced to one column; a URL with anything but printable ASCII stays plain text.
fn link_cells(buffer: &mut ratatui::buffer::Buffer, link: &Link) {
    let printable = |text: &str| text.chars().all(|character| character.is_ascii_graphic());
    if !printable(&link.url) || !printable(link.shown.trim_end_matches('…')) {
        return;
    }
    let shown: Vec<String> = link.shown.chars().map(String::from).collect();
    let area = buffer.area;
    let width = area.width as usize;
    if shown.is_empty() || shown.len() > width {
        return;
    }
    for row in 0..area.height as usize {
        let start = row * width;
        let mut x = 0;
        while x + shown.len() <= width {
            let cells = &buffer.content[start + x..start + x + shown.len()];
            if cells.iter().zip(&shown).all(|(cell, symbol)| cell.symbol() == symbol) {
                for cell in &mut buffer.content[start + x..start + x + shown.len()] {
                    let symbol = format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", link.url, cell.symbol());
                    cell.set_symbol(&symbol).set_diff_option(ratatui::buffer::CellDiffOption::ForcedWidth(std::num::NonZeroU16::MIN));
                }
                x += shown.len();
            } else {
                x += 1;
            }
        }
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

// A question asked through kelpie shows from the log; a prompt on screen outranks it.
fn asked_question(item: &Item) -> Option<&messages::Message> {
    item.question.as_ref().filter(|_| item.agent.as_ref().is_none_or(|agent| agent.status != "blocked"))
}

fn cut(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars().take(width.saturating_sub(1)).chain(['…']).collect()
}

fn question_summary(message: &messages::Message, width: usize) -> String {
    let count = message.question.as_ref().map_or(0, |question| question.options.len());
    let plan = message.question.as_ref().is_some_and(|question| question.plan.is_some());
    let suffix = if plan { " · plan to approve · q".to_string() } else { format!(" · {} · q", plural(count, "option")) };
    let one_line = message.text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{}{suffix}", cut(&one_line, width.saturating_sub(suffix.chars().count()).max(8)))
}

pub fn options_hint() -> Line<'static> {
    Line::from(vec![Span::raw("   "), Span::styled("⏎", style(MAUVE)), Span::styled(" options", style(MUTED))]).style(Style::new().bg(SURFACE))
}

type Rows = Vec<(usize, usize, Hit)>;

fn running_dot(item: &Item) -> bool {
    item.server || item.worker
}

fn work_lines(app: &mut App, width: usize, unfold: bool) -> (Vec<Line<'static>>, usize, Rows) {
    let open = app.column == Column::Work;
    let focused = open && !app.in_messages;
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
        let dot = running_dot(item);
        let mut line = row(selected, focused, status_icon(item), &item.name, place, &row_facts(item), width.saturating_sub(if dot { 2 } else { 0 }));
        if dot {
            line.spans.extend([Span::raw(" "), Span::styled("●", style(GREEN))]);
        }
        lines.push(line);
        if let Some(next) = item.next_for_you() {
            lines.push(you_line(next, selected && open && unfold, width));
        }
        if selected && open && unfold {
            let (detail, links, _) = detail_lines(app, item, width.saturating_sub(4), false);
            lines.extend(detail.into_iter().map(|line| {
                let mut spans = vec![Span::raw("    ")];
                spans.extend(line.spans);
                Line::from(spans).style(Style::new().bg(SURFACE))
            }));
            app.links.extend(links);
            lines.push(options_hint());
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

fn permit_chip(permit: Option<Permit>) -> Option<(String, Color)> {
    match permit? {
        Permit::Task => Some(("lead may push".to_string(), GREEN)),
        Permit::Repo => Some(("lead may push once reviewed (repo)".to_string(), TEAL)),
    }
}

const LABEL: usize = 8;

fn field(label: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{label:<LABEL$}"), style(MUTED))];
    spans.extend(value);
    Line::from(spans)
}

fn wrapped_field(label: &str, text: &str, color: Color, width: usize, max_lines: usize) -> Vec<Line<'static>> {
    wrap(text, width.saturating_sub(LABEL), max_lines).into_iter().enumerate()
        .map(|(index, part)| field(if index == 0 { label } else { "" }, vec![Span::styled(part, style(color))])).collect()
}

// The running dev server's address; the shown text is what link_cells finds again on screen.
fn server_line(item: &Item, configured: bool, width: usize) -> Option<(Line<'static>, Option<Link>)> {
    if !item.server {
        return configured.then(|| (field("server", vec![Span::styled("○ off · w starts it", style(MUTED))]), None));
    }
    let Some(url) = &item.url else { return Some((field("server", vec![Span::styled("● running", style(GREEN))]), None)) };
    let room = width.saturating_sub(LABEL + 12).max(2);
    let shown = cut(url, room);
    let line = field("server", vec![Span::styled("● running", style(GREEN)), Span::raw("  "), Span::styled(shown.clone(), style(TEAL))]);
    Some((line, Some(Link { shown, url: url.clone() })))
}

fn worker_line(item: &Item, configured: bool) -> Option<Line<'static>> {
    if item.worker {
        Some(field("worker", vec![Span::styled("● running", style(GREEN))]))
    } else {
        configured.then(|| field("worker", vec![Span::styled("○ off · b starts it", style(MUTED))]))
    }
}

fn review_text(item: &Item) -> (String, Color) {
    if let Some(chip) = item.verdict.as_ref().and_then(verdict_chip) {
        return chip;
    }
    match item.review {
        Some((read, total)) => (format!("{read}/{total} read in the split"), if read == total { GREEN } else { SUBTEXT }),
        None => ("not sent".to_string(), MUTED),
    }
}

fn changed_text(changed: &[String], width: usize) -> String {
    let mut shown: Vec<&str> = Vec::new();
    for name in changed {
        let used: usize = shown.iter().map(|name| name.chars().count() + 2).sum();
        if used + name.chars().count() + 4 > width && !shown.is_empty() {
            break;
        }
        shown.push(name);
    }
    let rest = changed.len() - shown.len();
    let more = if rest > 0 { format!(" +{rest}") } else { String::new() };
    format!("{}{more}", shown.join(", "))
}

fn report_text(app: &mut App, item: &Item, width: usize) -> Option<(String, String, Color)> {
    let agent = item.agent.as_ref()?;
    let screen = app.agent_screen(&agent.pane);
    let question = |text: String| tail(&text, width.saturating_sub(LABEL) * 4);
    let found = match &item.report {
        _ if agent.status == "blocked" => ("asks".to_string(), question(last_message(&screen)), PEACH),
        Some(report) if report.report.as_ref().is_some_and(|report| report.state == "needs-you") && !last_message(&screen).is_empty() => {
            ("asks".to_string(), question(last_message(&screen)), PEACH)
        }
        Some(report) => {
            let state = report.report.as_ref().map(|report| report.state.replace('-', " ")).unwrap_or_default();
            ("report".to_string(), format!("{state} · {} · {}", ago(report.at), report.text), TEXT)
        }
        None => match recap(&screen) {
            recap if !recap.is_empty() => ("recap".to_string(), recap, SUBTEXT),
            _ => ("said".to_string(), last_message(&screen), SUBTEXT),
        },
    };
    (!found.1.is_empty()).then_some(found)
}

fn state_spans(app: &App, item: &Item) -> Vec<Span<'static>> {
    let mut facts: Vec<(String, Color)> = Vec::new();
    facts.extend(absence(item).map(|absent| (absent.to_string(), YELLOW)));
    if let Some(git) = &item.git && git.behind > 0 {
        facts.push((format!("{} behind {}", git.behind, git.upstream.trim_start_matches("origin/")), YELLOW));
    }
    let halves: Vec<String> = app.tasks.iter().filter(|other| other.name == item.name && other.place != item.place).map(|other| other.place.clone()).collect();
    if !halves.is_empty() {
        facts.push((format!("same task in {}", halves.join(", ")), TEAL));
    }
    if item.unchecked() {
        facts.push((status::UNCHECKED.to_string(), YELLOW));
    }
    facts.extend(permit_chip(item.permit));
    let mut spans = Vec::new();
    for (index, (text, color)) in facts.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", style(MUTED)));
        }
        spans.push(Span::styled(text, style(color)));
    }
    spans
}

fn detail_lines(app: &mut App, item: &Item, width: usize, panel: bool) -> (Vec<Line<'static>>, Vec<Link>, Vec<ImageAt>) {
    let detail = app.detail(item).unwrap_or_default();
    let mut lines = Vec::new();
    let mut links = Vec::new();
    let mut images = Vec::new();
    let mut head = vec![Span::styled(item.name.clone(), bold(MAUVE))];
    let mut where_ = Vec::new();
    if let Some(path) = &item.path {
        where_.push(name_of(path));
    }
    if !detail.branch.is_empty() && detail.branch != item.name {
        where_.push(detail.branch.clone());
    }
    if let Some(git) = &item.git {
        where_.push(format!("{} ahead · {} behind", git.ahead, git.behind));
    }
    if !where_.is_empty() {
        head.push(Span::styled(format!(" · {}", where_.join(" · ")), style(MUTED)));
    }
    lines.push(Line::from(head));
    let state = state_spans(app, item);
    if !state.is_empty() {
        lines.push(Line::from(state));
    }
    if panel {
        lines.push(Line::default());
    }
    if let Some(asked) = asked_question(item) {
        lines.push(field("asks", vec![Span::styled(question_summary(asked, width.saturating_sub(LABEL)), style(PEACH))]));
    } else if let Some((label, text, color)) = report_text(app, item, width) {
        lines.extend(wrapped_field(&label, &text, color, width, if panel { 6 } else { 3 }));
    } else if item.agent.is_none() {
        lines.push(field("claude", vec![Span::styled("not running in its Space · g restarts it", style(MUTED))]));
    }
    if let Some((line, link)) = server_line(item, detail.dev_server, width) {
        lines.push(line);
        links.extend(link);
    }
    lines.extend(worker_line(item, detail.dev_worker));
    if item.git.as_ref().is_some_and(|git| git.ahead > 0) || item.verdict.is_some() || item.review.is_some() {
        let (text, color) = review_text(item);
        lines.push(field("review", vec![Span::styled(text, style(color))]));
    }
    if !detail.changed.is_empty() {
        lines.push(field("changed", vec![Span::styled(changed_text(&detail.changed, width.saturating_sub(LABEL)), style(TEXT))]));
    } else if let Some(git) = &item.git && git.changed > 0 {
        lines.push(field("changed", vec![Span::styled(plural(git.changed, "file"), style(TEXT))]));
    }
    if let Some(note) = note_of(app, item) {
        lines.push(field("note", vec![Span::styled(format!("vault: {note}"), style(TEAL))]));
    }
    if !panel {
        return (lines, links, images);
    }
    if !detail.commits.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("commits", style(MUTED))));
        for commit in detail.commits.iter().rev().take(8) {
            let subject = commit.message.lines().next().unwrap_or_default();
            lines.push(Line::from(vec![
                Span::raw("  "), Span::styled(commit.sha.chars().take(7).collect::<String>(), style(MUTED)), Span::raw(" "),
                Span::styled(cut(subject, width.saturating_sub(10)), style(TEXT)),
            ]));
        }
        if detail.commits.len() > 8 {
            lines.push(Line::from(Span::styled(format!("  … {} more", detail.commits.len() - 8), style(MUTED))));
        }
    }
    if let Some((card, current)) = &detail.card {
        lines.push(Line::default());
        let base = item.path.clone().unwrap_or_default();
        for line in card_lines(card, *current, width) {
            let path = card_image(&line).map(|path| base.join(path));
            match path.as_ref().and_then(|path| app.image_rows(path, width as u16).map(|rows| (path.clone(), rows))) {
                Some((path, rows)) => {
                    lines.push(line);
                    images.push(ImageAt { line: lines.len(), path, rows });
                    lines.extend((0..rows).map(|_| Line::default()));
                }
                None => lines.push(line),
            }
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled("⏎", style(MAUVE)), Span::styled(" options", style(MUTED))]));
    (lines, links, images)
}

// An image line of the card, as card_lines draws it: the marker, the path, then its alt text.
fn card_image(line: &Line) -> Option<String> {
    let first = line.spans.first()?;
    (first.content == "▣ ").then(|| line.spans.get(1).map(|span| span.content.to_string())).flatten()
}

fn note_of(app: &App, item: &Item) -> Option<String> {
    let address = peers::address(&item.name, &item.place);
    app.ongoing.iter().find(|note| note.task.as_deref() == Some(address.as_str())).map(|note| note.key.clone())
}

fn draw_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::new().borders(Borders::LEFT | Borders::RIGHT).border_style(style(SURFACE)).padding(Padding::horizontal(2));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(item) = app.selected_task() else {
        frame.render_widget(Paragraph::new(Span::styled("No task selected.", style(MUTED))), inner);
        return;
    };
    let (lines, links, images) = detail_lines(app, &item, inner.width as usize, true);
    frame.render_widget(Paragraph::new(lines), inner);
    app.links.extend(links);
    let resting = matches!(app.mode, Mode::Normal);
    for image in images.into_iter().filter(|_| resting) {
        let top = inner.y + image.line as u16;
        if image.line as u16 + image.rows > inner.height {
            continue;
        }
        if let Some(Some(protocol)) = app.images.get(&(image.path, inner.width)) {
            let size = protocol.size();
            frame.render_widget(Image::new(protocol), Rect::new(inner.x, top, size.width.min(inner.width), size.height));
        }
    }
}

fn ongoing_rows(app: &App, focused: bool, width: usize, lines: &mut Vec<Line<'static>>, rows: &mut Rows) -> Option<usize> {
    if app.ongoing.is_empty() {
        return None;
    }
    let mut selected_line = None;
    lines.push(Line::from(vec![Span::styled("ONGOING", bold(TEAL)), Span::styled(format!("  {}", app.ongoing.len()), style(FAINT)), Span::styled("  vault notes in progress", style(MUTED))]));
    for (index, note) in app.ongoing.iter().enumerate() {
        let selected = index == app.backlog_selected;
        if selected {
            selected_line = Some(lines.len());
        }
        let (icon, facts) = match &note.task {
            Some(task) => (Span::styled("↳", style(TEAL)), format!("task {task}")),
            None => (Span::styled("○", style(PEACH)), "no task · ⏎ resume".to_string()),
        };
        rows.push((lines.len(), lines.len() + 1, Hit::Entry(index)));
        lines.push(row(selected, focused, icon, &note.title, None, &facts, width));
    }
    lines.push(Line::default());
    selected_line
}

fn backlog_lines(app: &App, width: usize) -> (Vec<Line<'static>>, usize, Rows) {
    let focused = app.column == Column::Backlog && !app.in_messages;
    let mut lines = column_title("BACKLOG", format!("{} items", app.backlog.entries.len()), focused, width);
    let mut rows = Vec::new();
    let mut selected_line = ongoing_rows(app, focused, width, &mut lines, &mut rows).unwrap_or(0);
    let offset = app.ongoing.len();
    let mut current: Option<&str> = None;
    for (index, entry) in app.backlog.entries.iter().enumerate() {
        if current != Some(entry.priority.as_str()) {
            if current.is_some() {
                lines.push(Line::default());
            }
            let count = app.backlog.entries.iter().filter(|other| other.priority == entry.priority).count();
            lines.push(group_line(&entry.priority.to_uppercase(), count, priority_color(&entry.priority)));
            current = Some(&entry.priority);
        }
        let selected = index + offset == app.backlog_selected;
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
        rows.push((start, lines.len(), Hit::Entry(index + offset)));
    }
    if app.backlog.entries.is_empty() {
        let empty = backlog::empty_text(&app.sources);
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

pub fn hints(keys: &[(&str, &str)]) -> Line<'static> {
    Line::from(keys.iter().flat_map(|(key, label)| [Span::styled(key.to_string(), style(MAUVE)), Span::styled(format!(" {label}    "), style(MUTED))]).collect::<Vec<_>>())
}

const FULL_HINTS: [(&str, &str, KeyCode); 5] =
    [("⏎", "options", KeyCode::Enter), ("q", "questions", KeyCode::Char('q')), ("m", "messages", KeyCode::Char('m')), ("prefix+tab", "lead", KeyCode::Tab), ("?", "keys", KeyCode::Char('?'))];
const SHORT_HINTS: [(&str, &str, KeyCode); 3] = [("⏎", "options", KeyCode::Enter), ("q", "questions", KeyCode::Char('q')), ("?", "keys", KeyCode::Char('?'))];

fn status_hints(keys: &[(&str, &str, KeyCode)]) -> Line<'static> {
    Line::from(Span::styled(keys.iter().map(|(key, label, _)| format!("{key} {label}")).collect::<Vec<_>>().join(" · "), style(MUTED)))
}

fn hint_lines() -> (Line<'static>, Line<'static>) {
    (status_hints(&FULL_HINTS), status_hints(&SHORT_HINTS))
}

fn hint_hits(line: &Line, area: Rect) -> Vec<(Rect, Hit)> {
    let text = line.to_string();
    let mut hits = Vec::new();
    for (key, label, code) in FULL_HINTS {
        let hint = format!("{key} {label}");
        let Some(byte) = text.find(&hint) else { continue };
        let x = Span::raw(&text[..byte]).width() as u16;
        let width = Span::raw(hint.as_str()).width() as u16;
        if x < area.width {
            hits.push((Rect::new(area.x + x, area.y, width.min(area.width - x), 1), Hit::Key(code)));
        }
    }
    hits
}

fn local(at: u64) -> Option<libc::tm> {
    let time = at as libc::time_t;
    // SAFETY: localtime_r only writes the tm it is given.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        (!libc::localtime_r(&time, &mut tm).is_null()).then_some(tm)
    }
}

fn clock(at: u64) -> String {
    local(at).map_or_else(String::new, |tm| format!("{:02}:{:02}", tm.tm_hour, tm.tm_min))
}

fn resets(at: u64, now: u64) -> String {
    match (local(at), local(now)) {
        (Some(then), Some(today)) if (then.tm_year, then.tm_yday) == (today.tm_year, today.tm_yday) => format!("{:02}:{:02}", then.tm_hour, then.tm_min),
        (Some(then), _) => ["sun", "mon", "tue", "wed", "thu", "fri", "sat"][then.tm_wday.clamp(0, 6) as usize].to_string(),
        _ => String::new(),
    }
}

fn usage_color(used: f64) -> Color {
    if used < 50.0 { GREEN } else if used < 80.0 { YELLOW } else { RED }
}

fn usage_spans(usage: &Usage, wide: bool, now: u64) -> Vec<Span<'static>> {
    let windows = [(if wide { "session" } else { "5h" }, &usage.session), (if wide { "week" } else { "7d" }, &usage.week)];
    let mut spans = Vec::new();
    for (label, window) in windows {
        let Some(window) = window else { continue };
        let used = window.used.clamp(0.0, 100.0);
        let color = usage_color(used);
        if !spans.is_empty() {
            spans.push(Span::styled(if wide { "   " } else { " · " }, style(MUTED)));
        }
        spans.push(Span::styled(format!("{label} "), style(SUBTEXT)));
        if wide {
            let filled = (used / 100.0 * 8.0).round() as usize;
            spans.push(Span::styled("━".repeat(filled), style(color)));
            spans.push(Span::styled("━".repeat(8 - filled), style(FAINT)));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(format!("{used:.0}%"), style(color)));
        if wide {
            spans.push(Span::styled(format!(" resets {}", resets(window.resets_at, now)), style(MUTED)));
        }
    }
    spans
}

fn status_line(full: Line<'static>, short: Line<'static>, usage: Option<&Usage>, width: usize, now: u64) -> Line<'static> {
    let right = |wide: bool| usage.map(|usage| usage_spans(usage, wide, now)).unwrap_or_default();
    let width_of = |spans: &[Span]| spans.iter().map(|span| span.width()).sum::<usize>();
    let left_width = |line: &Line| line.width() - line.spans.last().map_or(0, |span| span.content.len() - span.content.trim_end().len());
    for (left, wide) in [(&full, true), (&short, true), (&full, false), (&short, false)] {
        let spans = right(wide);
        let used = left_width(left) + width_of(&spans);
        if used + 2 <= width || spans.is_empty() && left_width(left) <= width {
            let mut line: Vec<Span> = left.spans.clone();
            if let Some(last) = line.last_mut() {
                last.content = last.content.trim_end().to_string().into();
            }
            line.push(Span::raw(" ".repeat(width.saturating_sub(used))));
            line.extend(spans);
            return Line::from(line);
        }
    }
    short
}

fn message_text(message: &Message) -> String {
    message.text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fit(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut left = width;
    let mut out = Vec::new();
    for span in spans {
        let count = span.content.chars().count();
        if count <= left {
            left -= count;
            out.push(span);
        } else {
            out.push(Span::styled(cut(&span.content, left), span.style));
            break;
        }
    }
    Line::from(out)
}

fn messages_height(fold: Fold, height: u16) -> u16 {
    match fold {
        Fold::Hidden => 0,
        Fold::Line => 1,
        Fold::Open => (height / 3).max(4),
    }
}

// The second value is the index of the message on the panel's second line, which the mouse hits are counted from.
fn messages_lines(app: &App, fold: Fold, rows: usize, width: usize) -> (Vec<Line<'static>>, usize) {
    let new = app.messages.iter().filter(|message| message.at > app.seen).count();
    match fold {
        Fold::Hidden => (Vec::new(), 0),
        Fold::Line => {
            let mut spans = vec![Span::styled("▸ messages", style(MAUVE))];
            if new > 0 {
                spans.push(Span::styled(format!(" · {new} new"), style(YELLOW)));
            }
            if let Some(last) = app.messages.last() {
                spans.push(Span::styled(format!(" · last: {} → {} {}", last.from.clone(), last.to.clone(), messages::age(last.at)), style(MUTED)));
            }
            spans.push(Span::styled(" · m opens", style(MUTED)));
            (vec![fit(spans, width)], 0)
        }
        Fold::Open => {
            let keys = if app.in_messages { "  ↑ ↓ scroll · ⏎ read · esc back · m folds" } else { "  m selects" };
            let mut lines = vec![Line::from(vec![Span::styled("▾ messages", if app.in_messages { bold(MAUVE) } else { style(MAUVE) }), Span::styled(keys, style(MUTED))])];
            let room = rows.saturating_sub(1);
            if app.messages.is_empty() {
                lines.push(Line::from(Span::styled("No messages yet.", style(MUTED))));
            }
            let length = app.messages.len();
            let selected = length.saturating_sub(1 + app.message_back);
            let start = length.saturating_sub(room).min(selected.saturating_sub(room.saturating_sub(1)));
            for (index, message) in app.messages.iter().enumerate().skip(start).take(room) {
                let fresh = message.at > app.seen;
                let line = fit(vec![
                    Span::styled(format!("{} ", clock(message.at)), style(if fresh { YELLOW } else { MUTED })),
                    Span::styled(message.from.clone(), style(TEAL)), Span::styled(" → ", style(MUTED)), Span::styled(message.to.clone(), style(TEAL)),
                    Span::raw("  "), Span::styled(message_text(message), style(SUBTEXT)),
                ], width);
                lines.push(if app.in_messages && index == selected { line.style(Style::new().bg(SURFACE)) } else { line });
            }
            (lines, start)
        }
    }
}

fn fold_file() -> Option<PathBuf> {
    Some(lead::state_dir().ok()?.join("board-messages"))
}

fn load_fold(file: Option<&Path>, height: u16) -> (Fold, u64) {
    fold_from(&file.and_then(|file| std::fs::read_to_string(file).ok()).unwrap_or_default(), height)
}

// A short terminal starts the panel on one line whatever was saved.
fn fold_from(saved: &str, height: u16) -> (Fold, u64) {
    let mut parts = saved.trim().split('\t');
    let fold = match parts.next() {
        Some("hidden") => Fold::Hidden,
        Some("line") => Fold::Line,
        Some("open") => Fold::Open,
        _ => if height < 40 { Fold::Line } else { Fold::Open },
    };
    let seen = parts.next().and_then(|seen| seen.parse().ok()).unwrap_or(0);
    (if fold == Fold::Open && height < 40 { Fold::Line } else { fold }, seen)
}

fn save_fold(file: Option<&Path>, fold: Fold, seen: u64) {
    if let Some(file) = file {
        let _ = std::fs::write(file, format!("{}\t{seen}\n", fold.name()));
    }
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

fn menu_group(key: char) -> &'static str {
    match key {
        'q' | 'g' | 'p' | 'r' => "SESSION",
        'w' | 'b' => "DEV",
        'z' | 'x' => "END IT",
        _ => "BRANCH",
    }
}

// The second value is each choice's line, which the mouse hits are placed on.
fn menu_lines(choices: &[Choice], index: usize, width: usize, running: impl Fn(char) -> bool) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut at = Vec::new();
    let mut group = "";
    for (position, choice) in choices.iter().enumerate() {
        if menu_group(choice.key) != group {
            group = menu_group(choice.key);
            lines.push(Line::from(Span::styled(group, style(MUTED))));
        }
        let active = position == index;
        let ending = matches!(choice.key, 'z' | 'x');
        let label_style = match (active, ending) {
            (true, true) => bold(RED),
            (true, false) => bold(MAUVE),
            (false, true) => style(RED),
            (false, false) => style(TEXT),
        };
        let dot = if running(choice.key) { Span::styled("● ", style(GREEN)) } else { Span::raw("  ") };
        let gap = width.saturating_sub(choice.label.chars().count() + 5).max(1);
        at.push(lines.len());
        lines.push(Line::from(vec![
            Span::raw(if active { "▸ " } else { "  " }), Span::styled(choice.label.clone(), label_style), Span::raw(" ".repeat(gap)), dot, Span::styled(choice.key.to_string(), style(MUTED)),
        ]).style(if active { Style::new().bg(SURFACE) } else { Style::new() }));
    }
    lines.push(Line::from(Span::styled("─".repeat(width), style(SURFACE))));
    if let Some(choice) = choices.get(index) {
        lines.push(Line::from(Span::styled(choice.about.clone(), style(SUBTEXT))));
    }
    lines.push(Line::from(Span::styled("↑ ↓ choose · ⏎ run · a letter runs it directly · esc close", style(FAINT))));
    (lines, at)
}

fn help_lines() -> Vec<Line<'static>> {
    [
        ("⏎", "options for the selected task or backlog item"), ("← →", "switch between work and backlog"), ("1 2", "the work and areas pages"), ("tab", "move focus to the lead's chat"), ("↑ ↓ j k", "move"),
        ("q", "the questions waiting on you: read the plan, answer with a comment"), ("m", "messages at the bottom: one line, then open with ↑ ↓ moving through them and ⏎ reading one (esc back to the tasks), then hidden"),
        ("s", "start a new task"), ("g", "go to the task's Space, or start the backlog item"), ("p", "message the task's Claude"),
        ("r", "open the task's review next to its Claude"), ("w", "start or stop the task's dev server (worktree.dev_server); its address shows in the task's details (worktree.dev_url)"), ("b", "start or stop the task's worker (worktree.dev_worker)"), ("v", "send the task to review: a read-only Claude reads its commits; on a task in review, withdraw it"), ("a", "let the lead push the task itself, or take that back"), ("l", "push: rebase, then its card and commits, all or the first few (on a task)"), ("d", "done: after you pushed"),
        ("z", "reset the task: its work goes, its backlog item returns (asks first)"), ("x", "drop the task (asks first)"), ("R", "ask the lead to refresh the backlog"), ("a", "in the Backlog column: add an item to the local backlog"), ("A", "away mode on or off: the lead settles how-questions and keeps what-questions for you"), ("esc", "close this, or the board when it is an overlay"),
    ].into_iter().map(|(key, text)| Line::from(vec![Span::styled(format!("{key:<9}"), style(MAUVE)), Span::styled(text.to_string(), style(TEXT))])).collect()
}

#[derive(Clone)]
struct PlanStyle;

impl tui_markdown::StyleSheet for PlanStyle {
    fn heading(&self, level: u8) -> Style {
        if level <= 2 { bold(MAUVE) } else { bold(TEXT) }
    }

    fn code(&self) -> Style {
        style(PEACH)
    }

    fn link(&self) -> Style {
        style(BLUE).add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        style(SUBTEXT)
    }

    fn heading_meta(&self) -> Style {
        style(MUTED)
    }

    fn metadata_block(&self) -> Style {
        style(MUTED)
    }

    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }

    fn code_block_fence(&self) -> &str {
        ""
    }

    fn list_marker(&self) -> Style {
        style(MAUVE)
    }
}

pub fn markdown(text: &str) -> Text<'static> {
    let rendered = tui_markdown::from_str_with_options(text, &tui_markdown::Options::new(PlanStyle));
    Text::from(rendered.lines.into_iter().map(|line| {
        let spans: Vec<Span<'static>> = line.spans.into_iter().map(|span| Span::styled(span.content.into_owned(), span.style)).collect();
        let mut owned = Line::from(spans).style(line.style);
        owned.alignment = line.alignment;
        owned
    }).collect::<Vec<_>>())
}

const OWN_WORDS: &str = "In my own words";

fn split_address(address: &str) -> (String, String) {
    match address.split_once('@') {
        Some((name, place)) => (name.to_string(), place.to_string()),
        None => (address.to_string(), String::new()),
    }
}

fn draw_questions(frame: &mut Frame, app: &mut App) -> Vec<(Rect, Hit)> {
    let mut hits = Vec::new();
    let full = frame.area();
    let area = centered(full, (full.width * 92 / 100).max(40), (full.height * 84 / 100).max(16));
    frame.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(" questions ", bold(MAUVE))).style(Style::new().bg(MANTLE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let selected = match &app.mode { Mode::Questions(window) => window.selected, _ => return hits };
    let plan = app.questions.get(selected).cloned().and_then(|message| {
        let question = message.question.clone()?;
        if !app.plans.contains_key(&message.id) {
            app.plans.insert(message.id.clone(), markdown(&questions::plan_text(&question)?));
        }
        app.plans.get(&message.id).cloned()
    });
    let questions = app.questions.clone();
    let Mode::Questions(window) = &mut app.mode else { return hits };
    let [title_area, _, body, _, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(4), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    let mut title = vec![Span::styled(format!("{} waiting · answered here, the lead gets a copy", questions.len()), style(MUTED))];
    if let Some((ok, notice)) = &window.notice {
        title.push(Span::styled(format!("   {} {}", if *ok { "✓" } else { "!" }, notice.split_whitespace().collect::<Vec<_>>().join(" ")), style(if *ok { GREEN } else { RED })));
    }
    frame.render_widget(Paragraph::new(fit(title, title_area.width as usize)), title_area);
    let wide = body.width >= 90;
    let (list_area, main) = if wide {
        let [list, _, main] = Layout::horizontal([Constraint::Percentage(30), Constraint::Length(2), Constraint::Min(20)]).areas(body);
        (list, main)
    } else {
        let height = (questions.len() as u16 * 2).min(body.height / 3).max(2);
        let [list, _, main] = Layout::vertical([Constraint::Length(height), Constraint::Length(1), Constraint::Min(3)]).areas(body);
        (list, main)
    };
    let list_block = if wide { Block::new().borders(Borders::RIGHT).border_style(style(SURFACE)) } else { Block::new().borders(Borders::BOTTOM).border_style(style(SURFACE)) };
    let list_inner = list_block.inner(list_area);
    frame.render_widget(list_block, list_area);
    let mut list = Vec::new();
    for (index, message) in questions.iter().enumerate() {
        let active = index == window.selected;
        let (name, place) = split_address(&message.from);
        let kind = if message.question.as_ref().is_some_and(|question| question.plan.is_some()) { "plan" } else { "ask " };
        let background = if active && window.part == Part::List { Style::new().bg(SURFACE) } else { Style::new() };
        list.push(Line::from(vec![Span::raw(if active { "▸ " } else { "  " }), Span::styled(format!("{kind}  "), style(YELLOW)), Span::styled(name, if active { bold(TEXT) } else { style(TEXT) })]).style(background));
        list.push(Line::from(Span::styled(format!("        {place} · {}", messages::age(message.at)), style(MUTED))).style(background));
        let y = list_inner.y + 2 * index as u16;
        if y + 1 < list_inner.bottom() {
            hits.push((Rect::new(list_inner.x, y, list_inner.width, 2), Hit::Question(index)));
        }
    }
    frame.render_widget(Paragraph::new(fill(list, list_inner.width as usize)), list_inner);
    let Some(message) = questions.get(window.selected) else { return hits };
    let mut options = message.question.as_ref().map(|question| question.options.clone()).unwrap_or_default();
    options.push((OWN_WORDS.to_string(), "the comment is the whole answer".to_string()));
    let recommend = message.question.as_ref().map_or(0, |question| question.recommend);
    let asked = wrap(&message.text, main.width as usize, 3);
    let plan_height = if plan.is_some() { Constraint::Min(4) } else { Constraint::Length(0) };
    let [asked_area, _, plan_area, _, options_area, _, comment_area] = Layout::vertical([
        Constraint::Length(asked.len() as u16), Constraint::Length(1), plan_height, Constraint::Length(u16::from(plan.is_some())),
        Constraint::Length(options.len() as u16), Constraint::Length(1), Constraint::Length(3),
    ]).areas(main);
    frame.render_widget(Paragraph::new(asked.into_iter().map(|line| Line::from(Span::styled(line, bold(TEXT)))).collect::<Vec<_>>()), asked_area);
    if let Some(plan) = plan {
        let paragraph = Paragraph::new(plan).wrap(Wrap { trim: false });
        let mut source = vec![Span::styled(format!(" {} ", message.question.as_ref().and_then(|question| question.plan.clone()).unwrap_or_default()), style(MUTED))];
        if message.question.as_ref().is_some_and(questions::plan_changed) {
            source.push(Span::styled("changed since asked ", style(YELLOW)));
        }
        let box_block = Block::bordered().border_type(BorderType::Rounded).border_style(style(if window.part == Part::Plan { MAUVE } else { SURFACE })).padding(Padding::horizontal(1))
            .title(Line::from(source));
        let plan_inner = box_block.inner(plan_area);
        let total = paragraph.line_count(plan_inner.width) as u16;
        window.scroll = window.scroll.min(total.saturating_sub(plan_inner.height));
        let below = total.saturating_sub(plan_inner.height + window.scroll);
        let box_block = if below > 0 { box_block.title_bottom(Span::styled(format!(" ↓ {} · pgdn ", plural(below as usize, "more line")), style(MUTED))) } else { box_block };
        frame.render_widget(paragraph.scroll((window.scroll, 0)).block(box_block), plan_area);
    }
    let mut option_lines = Vec::new();
    for (index, (label, meaning)) in options.iter().enumerate() {
        let active = index == window.option;
        let color = if label.starts_with("Reject") { RED } else if index + 1 == recommend { GREEN } else { TEXT };
        let about = if index + 1 == recommend { format!("recommended · {meaning}") } else { meaning.clone() };
        let background = if active && window.part == Part::Options { Style::new().bg(SURFACE) } else { Style::new() };
        option_lines.push(fit(vec![
            Span::raw(if active { "▸ " } else { "  " }), Span::styled(label.clone(), if active { style(color).add_modifier(Modifier::BOLD) } else { style(color) }),
            Span::raw("  "), Span::styled(about, style(MUTED)),
        ], options_area.width as usize).style(background));
        let y = options_area.y + index as u16;
        if y < options_area.bottom() {
            hits.push((Rect::new(options_area.x, y, options_area.width, 1), Hit::Answer(index)));
        }
    }
    frame.render_widget(Paragraph::new(fill(option_lines, options_area.width as usize)), options_area);
    let typing = window.part == Part::Comment;
    window.comment.set_block(Block::bordered().border_type(BorderType::Rounded).border_style(style(if typing { MAUVE } else { SURFACE })));
    window.comment.set_cursor_style(if typing { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() });
    frame.render_widget(&window.comment, comment_area);
    let keys = "↑ ↓ choose · tab questions / plan / options / comment · pgup pgdn plan · ⏎ answer · g go to its Space · esc close";
    frame.render_widget(Paragraph::new(fit(vec![Span::styled(keys, style(MUTED))], foot.width as usize)), foot);
    hits
}

fn draw(frame: &mut Frame, app: &mut App) {
    app.hits.clear();
    app.links.clear();
    let full = frame.area();
    let area = full.inner(Margin { horizontal: 2, vertical: 1 });
    let fold = if app.page == Page::Work { app.fold } else { Fold::Hidden };
    let [top, _, body, messages_area, bottom] = Layout::vertical([
        Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(messages_height(fold, full.height)), Constraint::Length(1),
    ]).areas(area);
    let (line, tabs) = header(app, top.width);
    frame.render_widget(Paragraph::new(line), top);
    let tab_hits: Vec<(Rect, Hit)> = tabs.into_iter().map(|(x, width, page)| (Rect::new(top.x + x, top.y, width, 1), Hit::Tab(page))).collect();
    let (full_hints, short_hints) = match app.page {
        Page::Work => hint_lines(),
        Page::Areas => area_page::Page::hint_lines(),
    };
    let status = status_line(full_hints, short_hints, app.usage.as_ref(), bottom.width as usize, messages::now());
    if app.page == Page::Areas {
        frame.render_widget(Paragraph::new(status), bottom);
        app.areas.draw(frame, body);
        app.hits = if app.areas.is_normal() { tab_hits } else { Vec::new() };
        return;
    }
    let mut tab_hits = tab_hits;
    tab_hits.extend(hint_hits(&status, bottom));
    frame.render_widget(Paragraph::new(status), bottom);
    let (message_lines, first_message) = messages_lines(app, fold, messages_area.height as usize, messages_area.width as usize);
    let shown_messages = message_lines.len();
    frame.render_widget(Paragraph::new(message_lines), messages_area);
    match fold {
        Fold::Hidden => {}
        Fold::Line => tab_hits.push((messages_area, Hit::Messages(None))),
        Fold::Open => {
            tab_hits.push((Rect { height: 1, ..messages_area }, Hit::Messages(None)));
            for row in 1..shown_messages.min(messages_area.height as usize) {
                let index = first_message + row - 1;
                if index < app.messages.len() {
                    tab_hits.push((Rect::new(messages_area.x, messages_area.y + row as u16, messages_area.width, 1), Hit::Messages(Some(index))));
                }
            }
        }
    }

    if let Some(error) = &app.error {
        frame.render_widget(Paragraph::new(Span::styled(error.clone(), style(RED))).wrap(Wrap { trim: false }), body);
    } else if !app.loaded {
        frame.render_widget(Paragraph::new(Span::styled("looking around…", style(MUTED))), body);
    } else if full.width >= 150 {
        let [work_area, _, panel_area, _, backlog_area] = Layout::horizontal([
            Constraint::Percentage(33), Constraint::Length(1), Constraint::Percentage(38), Constraint::Length(2), Constraint::Min(20),
        ]).areas(body);
        let (work, work_selected, work_rows) = work_lines(app, work_area.width as usize, false);
        render_column(frame, work_area, work, work_selected, work_rows, &mut app.hits);
        draw_panel(frame, app, panel_area);
        let (backlog, backlog_selected, backlog_rows) = backlog_lines(app, backlog_area.width as usize);
        render_column(frame, backlog_area, backlog, backlog_selected, backlog_rows, &mut app.hits);
    } else if body.width >= 86 {
        let [work_area, _, backlog_area] = Layout::horizontal([Constraint::Percentage(54), Constraint::Length(4), Constraint::Min(20)]).areas(body);
        let (work, work_selected, work_rows) = work_lines(app, work_area.width as usize, true);
        render_column(frame, work_area, work, work_selected, work_rows, &mut app.hits);
        let (backlog, backlog_selected, backlog_rows) = backlog_lines(app, backlog_area.width as usize);
        render_column(frame, backlog_area, backlog, backlog_selected, backlog_rows, &mut app.hits);
    } else {
        let width = body.width as usize;
        let (mut lines, work_selected, mut rows) = work_lines(app, width, true);
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
        Mode::Options(index) if app.column == Column::Work => {
            let item = app.selected_task();
            let title: String = item.as_ref().map(|item| item.name.clone()).unwrap_or_default().chars().take(44).collect();
            let running = |key: char| item.as_ref().is_some_and(|item| (key == 'w' && item.server) || (key == 'b' && item.worker));
            let (lines, at) = menu_lines(&app.choices(), *index, 48, running);
            let area = modal(frame, title, lines, 52);
            for (choice, line) in at.into_iter().enumerate() {
                let y = area.y + 1 + line as u16;
                if y < area.bottom().saturating_sub(1) {
                    modal_hits.push((Rect::new(area.x, y, area.width, 1), Hit::Choice(choice)));
                }
            }
        }
        Mode::Options(index) => {
            let title = match app.backlog_selected.checked_sub(app.ongoing.len()) {
                Some(_) => app.selected_entry().map(|entry| entry.title).unwrap_or_default(),
                None => app.ongoing.get(app.backlog_selected).map(|note| note.title.clone()).unwrap_or_default(),
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
        Mode::Questions(_) => modal_hits = draw_questions(frame, app),
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
        Mode::ConfirmPush { commits, selected, card, .. } => {
            let (lines, first_commit) = push_lines(card.as_ref(), commits, *selected);
            let area = modal(frame, "push".to_string(), lines, 96);
            for index in 0..commits.len() {
                let y = area.y + 1 + (first_commit + index) as u16;
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
    for link in &app.links {
        link_cells(frame.buffer_mut(), link);
    }
}

const CARD_LINES: usize = 16;

// The sign-off card the task wrote when it reported ready, as the user reads it before pushing; an image shows as its path.
fn card_lines(card: &report::Card, current: bool, width: usize) -> Vec<Line<'static>> {
    let mut head = vec![Span::styled("card", bold(TEAL)), Span::styled(format!(" · written at {}", card.sha.chars().take(7).collect::<String>()), style(MUTED))];
    if !current {
        head.push(Span::styled(" · the code changed since", style(YELLOW)));
    }
    let mut body = Vec::new();
    for line in card.text.lines() {
        let image = line.trim().strip_prefix("![").and_then(|rest| rest.split_once("](")).map(|(alt, rest)| (alt, rest.trim_end_matches(')')));
        match image {
            Some((alt, path)) => body.push(Line::from(vec![Span::styled("▣ ", style(TEAL)), Span::styled(path.to_string(), style(TEXT)), Span::styled(format!("  {alt}"), style(MUTED))])),
            None if line.trim().is_empty() => body.push(Line::default()),
            None => body.extend(wrap(line, width, 4).into_iter().map(|part| Line::from(Span::styled(part, style(TEXT))))),
        }
    }
    if body.len() > CARD_LINES {
        body.truncate(CARD_LINES);
        body.push(Line::from(Span::styled(format!("… the rest in {}", card.file.display()), style(MUTED))));
    }
    let mut lines = vec![Line::from(head)];
    lines.extend(body);
    lines
}

// The push dialog's lines, and the line its first commit is on.
fn push_lines(card: Option<&(report::Card, bool)>, commits: &[tasks::Commit], selected: usize) -> (Vec<Line<'static>>, usize) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some((card, current)) = card {
        lines.extend(card_lines(card, *current, 88));
        lines.push(Line::default());
    }
    let first_commit = lines.len();
    for (index, commit) in commits.iter().enumerate() {
        let subject = commit.message.lines().next().unwrap_or_default().to_string();
        let active = index == selected;
        lines.push(Line::from(vec![
            Span::raw(if active { "▸ " } else { "  " }),
            Span::styled(commit.sha.chars().take(7).collect::<String>(), style(MUTED)),
            Span::raw(" "),
            Span::styled(subject, if active { bold(MAUVE) } else { style(TEXT) }),
        ]).style(if active { Style::new().bg(SURFACE) } else { Style::new() }));
    }
    if let Some(commit) = commits.get(selected) {
        lines.push(Line::default());
        for line in commit.message.lines() {
            let wrapped = if line.is_empty() { vec![String::new()] } else { wrap(line, 88, 6) };
            lines.extend(wrapped.into_iter().map(|part| Line::from(Span::styled(part, style(TEXT)))));
        }
    }
    lines.push(Line::default());
    let keys = if commits.len() > 1 {
        format!("y push all and close · p push the first {} and keep it open · e edit message · s squash into one · ↑ ↓ commit · esc cancel", selected + 1)
    } else {
        "y push and close · p push and keep it open · e edit message · esc cancel".to_string()
    };
    lines.push(Line::from(vec![Span::styled("Push?  ", style(YELLOW)), Span::styled(keys, style(MUTED))]));
    (lines, first_commit)
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

impl App {
    fn new(sources: Vec<Source>, backlog: Backlog, overlay: bool, refresh: Sender<()>) -> Self {
        App {
            column: Column::Work, tasks: Vec::new(), checkouts: Vec::new(), backlog, work_selected: 0, backlog_selected: 0,
            mode: Mode::Normal, overlay, loaded: false, error: None, screens: HashMap::new(), refresh, quit: false, wanted_task: None, hits: Vec::new(), links: Vec::new(),
            page: Page::Work, areas: area_page::Page::new(sources.clone()), sources, fetch_failures: Vec::new(), away: false,
            messages: Vec::new(), fold: Fold::Line, seen: 0, fold_file: None, in_messages: false, message_back: 0, questions: Vec::new(), usage: None, ongoing: Vec::new(),
            details: HashMap::new(), detail_requests: None, detail_results: None, asked_details: HashMap::new(),
            graphics: Graphics::Off, images: HashMap::new(), plans: HashMap::new(),
        }
    }
}

fn detect_graphics() -> Graphics {
    match std::env::var("KELPIE_IMAGES").as_deref() {
        Ok("off") => Graphics::Off,
        Ok("halfblocks") => Graphics::Ready(Picker::halfblocks()),
        _ if !std::io::IsTerminal::is_terminal(&std::io::stdin()) => Graphics::Ready(Picker::halfblocks()),
        _ => Graphics::Ready(Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())),
    }
}

pub fn run(overlay: bool) -> Result<String> {
    let sources = config::sources()?;
    let (snapshots, refresh) = spawn_refresher();
    let mut app = App::new(sources, backlog::load(), overlay, refresh);
    app.away = permit::away_since().is_some();
    let (requests, results) = spawn_detailer();
    app.detail_requests = Some(requests);
    app.detail_results = Some(results);
    let height = ratatui::crossterm::terminal::size().map_or(50, |(_, height)| height);
    app.fold_file = fold_file();
    (app.fold, app.seen) = load_fold(app.fold_file.as_deref(), height);
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
    // The query reads stdin, so it runs once here, before the first event is read.
    app.graphics = detect_graphics();
    let _ = terminal.clear();
    let result = (|| -> Result<()> {
        while !app.quit {
            if let Mode::EditMessage(editor) = &mut app.mode {
                editor.poll_draft();
            }
            app.poll_answer();
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
                        let kept = app.selected_message().filter(|_| app.message_back > 0).map(|message| message.id.clone());
                        app.messages = snapshot.messages;
                        if let Some(index) = kept.and_then(|id| app.messages.iter().position(|message| message.id == id)) {
                            app.message_back = app.messages.len() - 1 - index;
                        }
                        app.message_back = app.message_back.min(app.messages.len().saturating_sub(1));
                        app.questions = snapshot.questions;
                        app.usage = snapshot.usage;
                        app.ongoing = snapshot.ongoing;
                        app.backlog_selected = app.backlog_selected.min(app.backlog_len().saturating_sub(1));
                        let newest = app.messages.last().map_or(0, |message| message.at);
                        if app.fold == Fold::Open && newest > app.seen {
                            app.seen = newest;
                            save_fold(app.fold_file.as_deref(), app.fold, app.seen);
                        }
                        if let Mode::Questions(window) = &mut app.mode {
                            window.follow(&app.questions);
                        }
                        app.error = None;
                    }
                    Err(error) => app.error = Some(error),
                }
                app.loaded = true;
            }
            if let Some(results) = &app.detail_results {
                while let Ok((path, detail)) = results.try_recv() {
                    app.details.insert(path, std::rc::Rc::new(detail));
                }
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
            app.away = permit::away_since().is_some();
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
    fn the_backlog_column_adds_an_item_only_with_the_local_source() {
        let keys = |sources: &[Source]| backlog_choices(sources).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(&[Source::Local]), "gRa");
        assert_eq!(keys(&[Source::Vault, Source::Linear]), "gR");
    }

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
        let keys = |item: &Item| task_choices(Some(item), None, None).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(&crate::status::tests::task(Some("working"), None, 1)), "gprazx");
        assert_eq!(keys(&crate::status::tests::task(Some("idle"), Some("ready"), 1)), "gprvtldazx");
        assert_eq!(keys(&crate::status::tests::task(Some("idle"), Some("ready"), 0)), "gprtldazx", "no commits, nothing to review");
    }

    #[test]
    fn a_task_in_review_offers_no_push_until_its_reviewer_passes_it() {
        let keys = |item: &Item| task_choices(Some(item), None, None).iter().map(|choice| choice.key).collect::<String>();
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        item.verdict = Some(crate::status::tests::in_review(false, 2, false, false));
        assert_eq!(keys(&item), "gprvtdazx");
        assert_eq!(row_facts(&item), "1 commit · review: 2 findings");
        let withdraw = task_choices(Some(&item), None, None).into_iter().find(|choice| choice.key == 'v').unwrap();
        assert_eq!(withdraw.label, "Withdraw from review", "the push stays the user's: a task in review can always be taken out");
        item.verdict = None;
        assert_eq!(keys(&item), "gprvtldazx", "withdrawn, it offers push again");
        assert_eq!(task_choices(Some(&item), None, None).into_iter().find(|choice| choice.key == 'v').unwrap().label, "Send to review");
        item.verdict = Some(crate::status::tests::in_review(false, 2, false, false));
        item.verdict = Some(crate::status::tests::in_review(true, 0, false, false));
        assert_eq!(keys(&item), "gprvtldazx");
        assert_eq!(row_facts(&item), "1 commit · reviewed");
    }

    #[test]
    fn the_push_dialog_shows_the_card_above_the_commits_with_its_images_as_paths() {
        let text = |lines: &[Line]| lines.iter().map(|line| line.spans.iter().map(|span| span.content.to_string()).collect::<String>()).collect::<Vec<_>>().join("\n");
        let commits = vec![tasks::Commit { sha: "1111111aaaa".to_string(), message: "Add refunds\n\nso a customer gets money back".to_string() }, tasks::Commit { sha: "2222222bbbb".to_string(), message: "Show refunds".to_string() }];
        let card = report::Card { sha: "abcdef0123".to_string(), text: "Refunds show on the billing page.\n\n![the refund row](/tmp/refund.png)\nConfirm: 30 days.".to_string(), file: PathBuf::from("/state/cards/billing@web.md") };
        let (lines, first) = push_lines(Some(&(card, true)), &commits, 1);
        let screen = text(&lines);
        assert!(screen.starts_with("card · written at abcdef0\nRefunds show on the billing page.\n\n▣ /tmp/refund.png  the refund row\nConfirm: 30 days.\n"), "{screen}");
        assert!(text(&lines[first..first + 1]).contains("1111111 Add refunds"), "the first commit is where the mouse hits are counted from");
        assert!(screen.contains("p push the first 2 and keep it open"), "{screen}");
        assert!(!screen.contains("the code changed since"));
        let stale = report::Card { sha: "abcdef0123".to_string(), text: (0..40).map(|index| format!("line {index}")).collect::<Vec<_>>().join("\n"), file: PathBuf::from("/state/cards/billing@web.md") };
        let (lines, _) = push_lines(Some(&(stale, false)), &commits, 0);
        let screen = text(&lines);
        assert!(screen.contains("the code changed since") && screen.contains("… the rest in /state/cards/billing@web.md") && !screen.contains("line 20"), "{screen}");
        let (lines, first) = push_lines(None, &commits[..1], 0);
        assert_eq!(first, 0);
        assert!(text(&lines).contains("y push and close · p push and keep it open"));
    }

    #[test]
    fn a_ready_task_the_lead_has_not_approved_still_offers_push_and_close() {
        let mut unchecked = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        unchecked.git.as_mut().unwrap().approved = false;
        assert!(unchecked.group() == crate::status::Group::NeedsYou);
        assert!(task_choices(Some(&unchecked), None, None).iter().any(|choice| choice.key == 'l'));
        assert_eq!(row_facts(&unchecked), "1 commit · rules not checked by the lead");
    }

    #[test]
    fn a_task_without_its_claude_shows_why_and_g_restarts_it() {
        let gone = crate::status::tests::task(None, None, 1);
        assert_eq!(row_facts(&gone), "no Space · 1 commit");
        assert_eq!(task_choices(Some(&gone), None, None)[0].label, "Restart it");
        let mut shell_only = crate::status::tests::task(None, None, 0);
        shell_only.workspace = Some("w1".to_string());
        assert_eq!(row_facts(&shell_only), "no Claude · nothing yet");
        let running = crate::status::tests::task(Some("idle"), None, 0);
        assert_eq!(row_facts(&running), "nothing yet");
        assert_eq!(task_choices(Some(&running), None, None)[0].label, "Go to its Space");
    }

    #[test]
    fn the_user_gives_and_takes_back_the_leads_push_permission_per_task_and_the_card_shows_it() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        let permission = |item: &Item| task_choices(Some(item), None, None).into_iter().find(|choice| choice.key == 'a').map(|choice| choice.label);
        assert_eq!(permission(&item).as_deref(), Some("Let the lead push it"));
        assert!(!row_facts(&item).contains("lead may push"));
        item.permit = Some(Permit::Task);
        assert_eq!(permission(&item).as_deref(), Some("Take back push permission"));
        assert!(row_facts(&item).ends_with("lead may push"), "{}", row_facts(&item));
        assert_eq!(permit_chip(item.permit), Some(("lead may push".to_string(), GREEN)));
        item.permit = Some(Permit::Repo);
        assert_eq!(permission(&item).as_deref(), Some("Let the lead push it"), "the repo's permission is switched on the Areas page");
        assert_eq!(permit_chip(item.permit), Some(("lead may push once reviewed (repo)".to_string(), TEAL)));
        assert_eq!(permit_chip(None), None);
        let mut session = item.clone();
        session.git = None;
        assert_eq!(permission(&session), None, "a session outside a task has nothing to push");
    }

    #[test]
    fn w_is_offered_only_with_a_dev_server() {
        let keys = |dev_server: Option<&str>| task_choices(None, dev_server, None).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(None), "gprtldzx");
        assert_eq!(keys(Some("bin/dev")), "gprwtldzx");
        let start = task_choices(None, Some("npm run dev"), None);
        assert_eq!((start[3].label.as_str(), start[3].about.as_str()), ("Start the dev server", "npm run dev in a tab of its Space"));
    }

    #[test]
    fn b_is_offered_only_with_a_dev_worker() {
        let keys = |dev_worker: Option<&str>| task_choices(None, Some("bin/dev"), dev_worker).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(None), "gprwtldzx");
        assert_eq!(keys(Some("bin/jobs")), "gprwbtldzx");
        assert_eq!(task_choices(None, None, Some("bin/jobs")).iter().map(|choice| choice.key).collect::<String>(), "gprbtldzx", "a worker needs no dev server");
        let mut item = crate::status::tests::task(Some("idle"), None, 0);
        let labels = |item: &Item| task_choices(Some(item), Some("bin/dev"), Some("bin/jobs")).into_iter().filter(|choice| "wb".contains(choice.key)).map(|choice| choice.label).collect::<Vec<_>>();
        assert_eq!(labels(&item), ["Start the dev server", "Start the worker"]);
        item.server = true;
        item.worker = true;
        assert_eq!(labels(&item), ["Stop the dev server", "Stop the worker"]);
    }

    #[test]
    fn the_server_line_links_the_address_only_while_the_dev_server_runs() {
        let mut item = crate::status::tests::task(Some("idle"), None, 0);
        item.url = Some("https://wt1.app.dev".to_string());
        assert_eq!(server_line(&item, true, 60).unwrap().0.to_string(), "server  ○ off · w starts it", "an address from config is not a running server");
        assert!(server_line(&item, false, 60).is_none(), "no dev_server, no line");
        item.server = true;
        let (line, link) = server_line(&item, true, 60).unwrap();
        assert_eq!(line.to_string(), "server  ● running  https://wt1.app.dev");
        let link = link.unwrap();
        assert_eq!((link.shown.as_str(), link.url.as_str()), ("https://wt1.app.dev", "https://wt1.app.dev"));
        item.url = None;
        assert!(server_line(&item, true, 60).unwrap().1.is_none(), "dev_url off, or no port for {{port}}");
        item.url = Some("https://a-very-long-worktree-name.example.dev/path".to_string());
        let (line, link) = server_line(&item, true, 40).unwrap();
        assert!(line.width() <= 40, "{}", line.width());
        let link = link.unwrap();
        assert!(link.shown.ends_with('…') && link.url.ends_with("/path"));
    }

    #[test]
    fn the_server_address_is_a_terminal_hyperlink_one_column_per_character() {
        let mut item = crate::status::tests::task(Some("idle"), None, 0);
        item.server = true;
        item.url = Some("http://localhost:3001".to_string());
        let (line, link) = server_line(&item, true, 60).unwrap();
        let link = link.unwrap();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 3)).unwrap();
        terminal.draw(|frame| {
            frame.render_widget(Paragraph::new(vec![Line::from("http://localhost:3001 is elsewhere too"), line]), frame.area());
            link_cells(frame.buffer_mut(), &link);
        }).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(19, 1)].symbol(), "\x1b]8;;http://localhost:3001\x1b\\h\x1b]8;;\x1b\\");
        assert_eq!(buffer[(39, 1)].symbol(), "\x1b]8;;http://localhost:3001\x1b\\1\x1b]8;;\x1b\\");
        assert_eq!((buffer[(18, 1)].symbol(), buffer[(40, 1)].symbol()), (" ", " "), "the link covers the address and nothing else");
        assert!((19..40).all(|x| buffer[(x, 1)].diff_option == ratatui::buffer::CellDiffOption::ForcedWidth(std::num::NonZeroU16::MIN)));
        let mut plain = Link { shown: "https://bücher.example".to_string(), url: "https://bücher.example".to_string() };
        let mut buffer = ratatui::buffer::Buffer::with_lines(["https://bücher.example"]);
        link_cells(&mut buffer, &plain);
        assert_eq!(buffer[(0, 0)].symbol(), "h", "a URL beyond printable ASCII stays plain text");
        plain.url = "https://x.example\x1b]8;;evil".to_string();
        plain.shown = "https://x.example".to_string();
        link_cells(&mut buffer, &plain);
        assert!(!buffer.content.iter().any(|cell| cell.symbol().contains('\x1b')));
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
    fn a_question_asked_through_kelpie_shows_on_one_line_with_its_option_count_and_a_prompt_on_screen_over_it() {
        let asked = messages::fold(QUESTION).remove(0);
        let mut item = crate::status::tests::task(Some("idle"), Some("needs-you"), 0);
        assert!(asked_question(&item).is_none(), "a question in plain text keeps the screen fallback");
        item.question = Some(asked.clone());
        assert_eq!(asked_question(&item).map(|message| message.id.as_str()), Some("q1"));
        item.agent.as_mut().unwrap().status = "blocked".to_string();
        assert!(asked_question(&item).is_none());
        assert_eq!(question_summary(&asked, 60), "Which queue for refund mails? · 3 options · q");
        assert_eq!(question_summary(&asked, 30), "Which queue f… · 3 options · q");
    }

    #[test]
    fn the_recap_is_the_last_one_on_screen_with_its_continuation() {
        let screen = "※ recap: An older one.\n\n● Done.\n\n※ recap: We're adding search; the branch is rebased. Next, leave your\n  comments in the split. (disable recaps in /config)\n\n❯ \n";
        assert_eq!(recap(screen), "We're adding search; the branch is rebased. Next, leave your comments in the split.");
        assert_eq!(recap("● Nothing to recap.\n"), "");
    }

    const QUESTION: &str = r#"{"id":"q1","at":10,"from":"billing@web","to":"lead","text":"Which queue for refund mails?","state":"asked","question":{"options":[{"label":"Sidekiq","meaning":"reuse the jobs"},{"label":"Solid Queue","meaning":"one less service"},{"label":"Inline","meaning":"no queue at all"}],"recommend":2}}"#;
    const PLAN: &str = r#"{"id":"q2","at":20,"from":"payload@api","to":"lead","text":"Approve this plan?","state":"asked","question":{"options":[{"label":"Approve","meaning":"build it as planned"},{"label":"Approve with a comment","meaning":"build it, taking the comment in"},{"label":"Ask for changes","meaning":"revise and ask again"},{"label":"Reject with a comment","meaning":"drop this approach"}],"recommend":1,"plan":"/state/plans/payload@api-20.md"}}"#;

    fn test_app(tasks: Vec<Item>) -> App {
        let (refresh, _) = mpsc::channel();
        let mut app = App::new(vec![Source::Local], Backlog { updated: None, entries: Vec::new() }, false, refresh);
        for item in &tasks {
            if let Some(agent) = &item.agent {
                app.screens.insert(agent.pane.clone(), (Instant::now(), String::new()));
            }
        }
        app.tasks = tasks;
        app.loaded = true;
        app.graphics = Graphics::Off;
        app.fold = Fold::Hidden;
        app
    }

    fn served(name: &str) -> Item {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
        item.name = name.to_string();
        item.server = true;
        item.url = Some("https://wt2.web.test".to_string());
        item.path = Some(PathBuf::from("/work/wt2.web"));
        item
    }

    fn with_detail(app: &mut App) {
        let detail = Detail {
            branch: "search-filters".to_string(), changed: vec!["app/filters.rb".to_string()], dev_server: true, dev_worker: true,
            commits: vec![tasks::Commit { sha: "a1f3c2e0000".to_string(), message: "Wire the price filter".to_string() }], card: None,
        };
        app.details.insert(PathBuf::from("/work/wt2.web"), std::rc::Rc::new(detail));
    }

    fn render(app: &mut App, width: u16, height: u16) -> (Vec<String>, ratatui::buffer::Buffer) {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows = (0..height).map(|y| (0..width).map(|x| {
            let symbol = buffer[(x, y)].symbol();
            symbol.split("\x1b\\").nth(1).map_or(symbol, |inner| inner.split('\x1b').next().unwrap_or(inner)).to_string()
        }).collect::<String>()).collect();
        (rows, buffer)
    }

    fn find(rows: &[String], text: &str) -> Vec<(usize, usize)> {
        rows.iter().enumerate().filter_map(|(y, row)| row.find(text).map(|byte| (row[..byte].chars().count(), y))).collect()
    }

    #[test]
    fn at_170_columns_the_selected_task_has_a_detail_panel_between_work_and_backlog_and_its_link_only_there() {
        let mut app = test_app(vec![served("search-filters"), crate::status::tests::task(Some("working"), None, 0)]);
        with_detail(&mut app);
        let (rows, buffer) = render(&mut app, 170, 40);
        let screen = rows.join("\n");
        let urls = find(&rows, "https://wt2.web.test");
        assert_eq!(urls.len(), 1, "{screen}");
        let (work_x, _) = find(&rows, "WORK")[0];
        let (backlog_x, _) = find(&rows, "BACKLOG")[0];
        assert!(work_x < urls[0].0 && urls[0].0 < backlog_x, "the address is in the middle column: {screen}");
        let (x, y) = urls[0];
        assert!(buffer[(x as u16, y as u16)].symbol().starts_with("\x1b]8;;https://wt2.web.test"), "the panel's address is a link");
        assert!(screen.contains("search-filters · wt2.web · 2 ahead · 0 behind") && screen.contains("a1f3c2e Wire the price filter"), "{screen}");
        assert!(screen.contains("worker  ○ off · b starts it") && screen.contains("changed app/filters.rb"), "{screen}");
        let row = rows.iter().find(|row| row.contains("▌○ search-filters")).unwrap_or_else(|| panic!("{screen}"));
        assert!(row[..row.find('│').unwrap()].trim_end().ends_with('●'), "a running server is a dot on its row: {row}");
        assert!(!screen.contains(" web https://"), "no web line under the row: {screen}");
    }

    #[test]
    fn below_86_columns_backlog_sits_under_work_and_the_details_unfold_under_the_selected_row() {
        let mut app = test_app(vec![served("search-filters"), crate::status::tests::task(Some("working"), None, 0)]);
        with_detail(&mut app);
        let (rows, _) = render(&mut app, 84, 50);
        let screen = rows.join("\n");
        let (_, work_y) = find(&rows, "WORK")[0];
        let (_, backlog_y) = find(&rows, "BACKLOG")[0];
        let (_, row_y) = find(&rows, "▌○ search-filters")[0];
        let (_, url_y) = find(&rows, "https://wt2.web.test")[0];
        assert!(work_y < row_y && row_y < url_y && url_y < backlog_y, "{screen}");
        assert!(!screen.contains("a1f3c2e"), "commits stay in the wide panel: {screen}");
        assert!(rows[row_y].trim_end().ends_with('●'), "{}", rows[row_y]);
    }

    #[test]
    fn a_task_card_shows_its_question_on_one_line_with_how_many_options_it_has() {
        let mut item = crate::status::tests::task(Some("idle"), Some("needs-you"), 0);
        item.question = Some(messages::fold(QUESTION).remove(0));
        let mut app = test_app(vec![item]);
        let (rows, _) = render(&mut app, 170, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("asks    Which queue for refund mails? · 3 options · q"), "{screen}");
        assert!(!screen.contains("reuse the jobs") && !screen.contains("one less service"), "{screen}");
    }

    #[test]
    fn the_messages_panel_folds_from_hidden_to_one_line_to_open() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.messages = messages::fold(&[QUESTION, PLAN].join("\n"));
        app.messages[1].text = "Approve this plan for the payload rename?".to_string();
        app.seen = 15;
        let (rows, _) = render(&mut app, 96, 48);
        assert!(find(&rows, "▸ messages").is_empty() && find(&rows, "▾ messages").is_empty());
        app.fold = Fold::Line;
        let (rows, _) = render(&mut app, 96, 48);
        let line = find(&rows, "▸ messages · 1 new · last: payload@api → lead");
        assert_eq!(line.len(), 1, "{}", rows.join("\n"));
        assert_eq!(line[0].1, 45, "one line above the status bar");
        app.fold = Fold::Open;
        let (rows, _) = render(&mut app, 96, 48);
        let screen = rows.join("\n");
        let (_, header) = find(&rows, "▾ messages")[0];
        assert_eq!(header, 46 - 16, "a third of the height");
        assert!(screen.contains("billing@web → lead  Which queue for refund mails?") && screen.contains("payload@api → lead  Approve this plan for the payload rename?"), "{screen}");
        assert_eq!(fold_from("", 30).0, Fold::Line, "a short terminal starts on one line");
        assert_eq!(fold_from("", 50).0, Fold::Open);
        assert_eq!(fold_from("open\t42\n", 30), (Fold::Line, 42));
        assert_eq!(fold_from("hidden\t42\n", 50), (Fold::Hidden, 42));
    }

    #[test]
    fn the_task_menu_groups_its_choices_and_says_only_what_the_highlighted_one_does() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
        item.server = true;
        let choices = task_choices(Some(&item), Some("bin/dev"), Some("bin/jobs"));
        let (lines, at) = menu_lines(&choices, 7, 48, |key| key == 'w' && item.server);
        let text: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        assert_eq!(text.iter().filter(|line| ["SESSION", "DEV", "BRANCH", "END IT"].contains(&line.as_str())).count(), 4, "{text:#?}");
        assert!(text[at[3]].starts_with("  Stop the dev server") && text[at[3]].ends_with("● w"), "{}", text[at[3]]);
        assert!(text[at[7]].starts_with("▸ Push") && text[at[7]].ends_with("  l"), "{}", text[at[7]]);
        assert!(at.iter().all(|line| text[*line].chars().count() == 48));
        assert!(text.contains(&choices[7].about) && !text.contains(&choices[6].about), "only the highlighted choice's description");
        let mut app = test_app(vec![item]);
        app.mode = Mode::Options(2);
        let (rows, _) = render(&mut app, 96, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("SESSION") && screen.contains("BRANCH") && screen.contains("END IT"), "{screen}");
        for (rect, hit) in &app.hits {
            let Hit::Choice(index) = hit else { panic!("only choices are hit in the menu") };
            let label = &app.choices()[*index].label;
            assert!(rows[rect.y as usize].contains(label.as_str()), "the hit for {label} is on its line");
        }
        assert_eq!(app.hits.len(), app.choices().len());
    }

    #[test]
    fn the_status_bar_shows_plan_usage_as_the_mockup_draws_it_wide_and_short() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        let now = messages::now();
        app.usage = Some(Usage {
            session: Some(usage::Window { used: 23.0, resets_at: now + 600 }), week: Some(usage::Window { used: 71.4, resets_at: now + 3 * 86_400 }), seen_at: now,
        });
        let (rows, buffer) = render(&mut app, 170, 30);
        let bar = &rows[28];
        assert!(bar.trim_start().starts_with("⏎ options · q questions · m messages · prefix+tab lead · ? keys"), "{bar}");
        assert!(bar.contains(&format!("session ━━━━━━━━ 23% resets {}   week ━━━━━━━━ 71% resets ", clock(now + 600))), "{bar}");
        let (x, _) = find(&rows[28..29], "session ━")[0];
        let fills: Vec<Color> = (0..8).map(|cell| buffer[(x as u16 + 8 + cell, 28)].fg).collect();
        assert_eq!(fills, [GREEN, GREEN, FAINT, FAINT, FAINT, FAINT, FAINT, FAINT], "23% fills two of eight on a surface1 track");
        assert_eq!(buffer[(x as u16, 28)].fg, SUBTEXT);
        let (x, _) = find(&rows[28..29], "23%")[0];
        assert_eq!(buffer[(x as u16, 28)].fg, GREEN);
        assert_eq!(buffer[(x as u16 + 4, 28)].fg, MUTED, "the reset time is faint");
        let (x, _) = find(&rows[28..29], "71%")[0];
        assert_eq!(buffer[(x as u16, 28)].fg, YELLOW);
        let (rows, _) = render(&mut app, 112, 30);
        assert!(rows[28].trim_start().starts_with("⏎ options · q questions · ? keys") && rows[28].contains("session ━━━━━━━━ 23% resets"), "the herdr popup keeps the bars: {}", rows[28]);
        let (rows, _) = render(&mut app, 80, 30);
        assert!(rows[28].trim_end().ends_with("5h 23% · 7d 71%"), "{}", rows[28]);
        app.usage = None;
        let (rows, _) = render(&mut app, 96, 30);
        assert!(!rows[28].contains('%'));
        assert_eq!(usage_color(49.0), GREEN);
        assert_eq!(usage_color(80.0), RED);
    }

    fn some_messages(count: usize) -> Vec<Message> {
        let lines: Vec<String> = (0..count).map(|at| format!(r#"{{"id":"m{at}","at":{},"from":"task{at}@web","to":"lead","text":"note number {at}","state":"sent"}}"#, 100 + at)).collect();
        messages::fold(&lines.join("\n"))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn click(app: &mut App, x: u16, y: u16) -> Option<KeyEvent> {
        let hit = app.hit_at(x, y);
        app.work_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE }, hit)
    }

    #[test]
    fn m_opens_the_messages_into_a_selection_that_arrows_move_and_enter_reads_and_esc_leaves() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.messages = some_messages(5);
        app.fold = Fold::Line;
        app.handle_normal(key(KeyCode::Char('m')));
        assert!(app.fold == Fold::Open && app.in_messages && app.selected_message().unwrap().id == "m4", "m from one line opens the panel at the newest message");
        app.handle_normal(key(KeyCode::Up));
        app.handle_normal(key(KeyCode::Char('k')));
        assert_eq!(app.selected_message().unwrap().id, "m2");
        for _ in 0..9 {
            app.handle_normal(key(KeyCode::Up));
        }
        assert_eq!(app.selected_message().unwrap().id, "m0", "stops at the oldest");
        app.handle_normal(key(KeyCode::Char('j')));
        app.handle_normal(key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Normal), "a task's letters do nothing while the messages are selected");
        let (rows, buffer) = render(&mut app, 112, 48);
        let (x, y) = find(&rows, "task1@web → lead  note number 1")[0];
        assert_eq!(buffer[(x as u16, y as u16)].bg, SURFACE, "the selected message is highlighted");
        assert!(rows.iter().any(|row| row.contains("▾ messages  ↑ ↓ scroll · ⏎ read · esc back · m folds")));
        app.handle_normal(key(KeyCode::Enter));
        let Mode::Message(text) = &app.mode else { panic!("enter reads the message") };
        assert!(text.contains("task1@web → lead") && text.ends_with("note number 1"), "{text}");
        app.mode = Mode::Normal;
        app.handle_normal(key(KeyCode::Esc));
        assert!(!app.in_messages && app.fold == Fold::Open && !app.quit, "esc leaves the panel open and the board up");
        app.handle_normal(key(KeyCode::Char('m')));
        assert!(app.in_messages, "m on an open panel goes into it");
        app.handle_normal(key(KeyCode::Char('m')));
        assert!(app.fold == Fold::Hidden && !app.in_messages);
        app.handle_normal(key(KeyCode::Char('m')));
        assert!(app.fold == Fold::Line && !app.in_messages);
    }

    #[test]
    fn the_mouse_hits_rows_messages_and_status_hints_where_they_are_drawn() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0), served("search-filters")]);
        app.messages = some_messages(3);
        app.fold = Fold::Line;
        let (rows, _) = render(&mut app, 112, 40);
        let (x, y) = find(&rows, "▸ messages")[0];
        for (hint, code) in [("⏎ options", KeyCode::Enter), ("q questions", KeyCode::Char('q')), ("? keys", KeyCode::Char('?'))] {
            let (hx, hy) = find(&rows, hint).into_iter().last().unwrap();
            assert_eq!(app.hit_at(hx as u16, hy as u16), Some(Hit::Key(code)), "{hint}");
            assert_eq!(app.hit_at((hx + hint.chars().count() - 1) as u16, hy as u16), Some(Hit::Key(code)), "the end of {hint}");
            assert_eq!(click(&mut app, hx as u16, hy as u16), Some(key(code)));
        }
        assert_eq!(click(&mut app, x as u16, y as u16), None);
        assert!(app.fold == Fold::Open && app.in_messages, "a click on the messages line opens them, selected");
        let (rows, _) = render(&mut app, 112, 40);
        let (x, y) = find(&rows, "note number 0")[0];
        assert_eq!(app.hit_at(x as u16, y as u16), Some(Hit::Messages(Some(0))));
        assert_eq!(click(&mut app, x as u16, y as u16), None);
        assert_eq!(app.selected_message().unwrap().id, "m0");
        assert_eq!(click(&mut app, x as u16, y as u16), Some(key(KeyCode::Enter)), "a second click reads it");
        let (x, y) = find(&rows, "search-filters")[0];
        assert_eq!(app.hit_at(x as u16, y as u16), Some(Hit::Task(1)));
        click(&mut app, x as u16, y as u16);
        assert!(!app.in_messages && app.work_selected == 1, "a click on a task takes the selection back to it");
        let (rows, _) = render(&mut app, 112, 40);
        let (x, y) = find(&rows, "▾ messages  m selects")[0];
        click(&mut app, x as u16, y as u16);
        assert!(app.in_messages && app.fold == Fold::Open, "a click on the open panel's title goes into it");
        app.in_messages = false;
        let (x, y) = find(&rows, "note number 2")[0];
        let wheel = app.work_mouse(MouseEvent { kind: MouseEventKind::ScrollUp, column: x as u16, row: y as u16, modifiers: KeyModifiers::NONE }, app.hit_at(x as u16, y as u16));
        assert!(wheel == Some(key(KeyCode::Up)) && app.in_messages, "the wheel over the messages scrolls them");
    }

    #[test]
    fn the_question_window_lists_every_question_and_shows_the_selected_plan_as_markdown() {
        let mut app = test_app(Vec::new());
        app.questions = messages::fold(&[QUESTION, PLAN].join("\n"));
        app.plans.insert("q2".to_string(), markdown(&format!("## Problem\n\nThe payload says *amount*.\n\n- one release per repo\n- the fallback goes\n\n{}", "more\n\n".repeat(30))));
        app.open_questions(Some(1));
        let (rows, _) = render(&mut app, 170, 44);
        let screen = rows.join("\n");
        assert!(screen.contains("2 waiting · answered here, the lead gets a copy"), "{screen}");
        assert!(screen.contains("ask   billing") && screen.contains("▸ plan  payload"), "{screen}");
        assert!(screen.contains("Problem") && !screen.contains("## Problem") && screen.contains("The payload says amount."), "{screen}");
        assert!(screen.contains("one release per repo") && screen.contains("more lines · pgdn"), "{screen}");
        assert!(screen.contains("▸ Approve  recommended · build it as planned") && screen.contains("Reject with a comment"), "{screen}");
        assert!(screen.contains("comment (optional)"), "{screen}");
        let questions = app.hits.iter().filter(|(_, hit)| matches!(hit, Hit::Question(_))).count();
        let answers = app.hits.iter().filter(|(_, hit)| matches!(hit, Hit::Answer(_))).count();
        assert_eq!((questions, answers), (2, 5), "four options and in my own words");
        assert!(screen.contains(" /state/plans/payload@api-20.md ") && !screen.contains("changed since asked"), "the plan's source above it: {screen}");
        let (rows, _) = render(&mut app, 80, 40);
        assert!(rows.join("\n").contains("Approve this plan?"), "a narrow window stacks the list above the question");
    }

    #[test]
    fn a_card_image_draws_in_the_panel_with_graphics_and_shows_its_path_without() {
        let dir = PathBuf::from(format!("/tmp/kc{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("after.png");
        image::RgbImage::from_pixel(40, 20, image::Rgb([200, 30, 30])).save(&png).unwrap();
        let card = |text: String| report::Card { sha: "abcdef0123".to_string(), text, file: PathBuf::from("/state/cards/x.md") };
        let mut app = test_app(vec![served("search-filters")]);
        with_detail(&mut app);
        let text = format!("Refunds show.\n![after]({})\n![before](/nowhere/before.png)", png.display());
        std::rc::Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card = Some((card(text), true));
        let (rows, _) = render(&mut app, 170, 50);
        let screen = rows.join("\n");
        assert!(screen.contains(&format!("▣ {}  after", png.display())) && screen.contains("▣ /nowhere/before.png  before"), "{screen}");
        app.graphics = Graphics::Ready(Picker::halfblocks());
        let (rows, buffer) = render(&mut app, 170, 50);
        let (_, y) = find(&rows, "  after")[0];
        let red = Color::Rgb(200, 30, 30);
        assert!((0..170).any(|x| buffer[(x, y as u16 + 1)].bg == red || buffer[(x, y as u16 + 1)].fg == red), "the image under its line: {}", rows.join("\n"));
        assert!(rows.join("\n").contains("▣ /nowhere/before.png  before"), "a missing file stays a path");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ongoing_notes_head_the_backlog_with_their_task_or_a_resume() {
        let mut app = test_app(vec![served("search-filters")]);
        app.ongoing = vec![
            Ongoing { key: "Projects/search-filters.md".to_string(), title: "search filters".to_string(), area: "web".to_string(), task: Some("search-filters@web".to_string()) },
            Ongoing { key: "Projects/docs-refresh.md".to_string(), title: "docs refresh".to_string(), area: "shop".to_string(), task: None },
        ];
        let (rows, _) = render(&mut app, 84, 50);
        let screen = rows.join("\n");
        let (_, backlog) = find(&rows, "BACKLOG")[0];
        let (_, ongoing) = find(&rows, "ONGOING")[0];
        assert!(backlog < ongoing && screen.contains("task search-filters@web") && screen.contains("no task · ⏎ resume"), "{screen}");
        assert!(screen.contains("note    vault: Projects/search-filters.md"), "the task shows its note: {screen}");
        app.column = Column::Backlog;
        app.backlog_selected = 1;
        assert_eq!(app.choices()[0].label, "Resume as task");
        app.act('g');
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogName(entry), .. } if entry.reference == "Projects/docs-refresh.md" && entry.source == "vault"));
        app.mode = Mode::Normal;
        app.backlog_selected = 0;
        app.act('g');
        assert!(app.column == Column::Work && app.work_selected == 0, "a note with a task goes to it");
    }

    #[test]
    fn at_112_columns_work_and_backlog_sit_side_by_side_with_the_details_unfolded_in_work() {
        let mut app = test_app(vec![served("search-filters"), crate::status::tests::task(Some("working"), None, 0)]);
        with_detail(&mut app);
        let (rows, buffer) = render(&mut app, 112, 40);
        let screen = rows.join("\n");
        let (work_x, work_y) = find(&rows, "WORK")[0];
        let (backlog_x, backlog_y) = find(&rows, "BACKLOG")[0];
        assert!(work_y == backlog_y && work_x < backlog_x, "{screen}");
        let (_, row_y) = find(&rows, "▌○ search-filters")[0];
        let urls = find(&rows, "https://wt2.web.test");
        assert_eq!(urls.len(), 1, "{screen}");
        let (url_x, url_y) = urls[0];
        assert!(url_y > row_y && url_x < backlog_x, "the details unfold under the row, in Work: {screen}");
        assert!(buffer[(url_x as u16, url_y as u16)].symbol().starts_with("\x1b]8;;https://wt2.web.test"));
        assert!(!screen.contains("a1f3c2e"), "commits stay in the wide panel: {screen}");
    }

    fn three_questions() -> Vec<Message> {
        let log = ["a", "b", "c"].iter().enumerate().map(|(index, name)| {
            format!(r#"{{"id":"{name}","at":{at},"from":"{name}@web","to":"lead","text":"Question {name}?","state":"asked","question":{{"options":[{{"label":"One","meaning":"the first"}},{{"label":"Two","meaning":"the second"}}],"recommend":2}}}}"#, at = index + 1)
        }).collect::<Vec<_>>().join("\n");
        messages::fold(&log)
    }

    #[test]
    fn the_question_window_follows_its_question_when_others_close_and_clears_when_its_own_does() {
        let mut questions = three_questions();
        let mut window = QuestionWindow::new(&questions, 1);
        window.option = 0;
        window.comment.insert_str("use the first");
        questions.remove(0);
        window.follow(&questions);
        assert_eq!((window.selected, window.id.as_str(), window.option, window.comment().as_str()), (0, "b", 0, "use the first"), "B stays selected with its choice");
        assert!(window.notice.is_none());
        questions.remove(0);
        window.follow(&questions);
        assert_eq!((window.id.as_str(), window.option, window.comment().as_str()), ("c", 1, ""), "C does not inherit B's choice or comment");
        assert!(window.notice.as_ref().is_some_and(|(ok, text)| !ok && text.contains("answered or closed elsewhere")));
        let mut window = QuestionWindow::new(&three_questions(), 0);
        window.option = 0;
        window.comment.insert_str("for a");
        window.select(&three_questions(), 1);
        assert_eq!((window.option, window.comment().as_str()), (1, ""), "moving in the list resets the option and comment");
    }

    #[test]
    fn in_my_own_words_needs_a_comment_and_is_offered_after_the_options() {
        let mut app = test_app(Vec::new());
        app.questions = three_questions();
        app.open_questions(Some(0));
        let (rows, _) = render(&mut app, 170, 40);
        let screen = rows.join("\n");
        let (_, two) = find(&rows, "Two  recommended · the second")[0];
        let (_, own) = find(&rows, "In my own words  the comment is the whole answer")[0];
        assert_eq!(own, two + 1, "{screen}");
        assert_eq!(app.hits.iter().filter(|(_, hit)| matches!(hit, Hit::Answer(_))).count(), 3);
        let Mode::Questions(mut window) = std::mem::replace(&mut app.mode, Mode::Normal) else { panic!() };
        window.option = 2;
        app.handle_questions(window, KeyEvent::from(KeyCode::Enter));
        let Mode::Questions(window) = &app.mode else { panic!("the window stays open") };
        assert!(window.answering.is_none() && window.part == Part::Comment, "an empty answer in your own words is refused");
        assert!(window.notice.as_ref().is_some_and(|(ok, text)| !ok && text.contains("own words")));
    }

    #[test]
    fn a_plan_that_cannot_be_read_yet_is_read_again_on_the_next_draw() {
        let mut app = test_app(Vec::new());
        app.questions = messages::fold(&PLAN.replace("/state/plans/payload@api-20.md", "/nowhere/kelpie-plan.md"));
        app.open_questions(None);
        render(&mut app, 170, 40);
        assert!(app.plans.is_empty(), "nothing is cached for a plan that is not there");
    }
}
