use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
use ratatui::layout::Size;
use ratatui_image::picker::{Capability, Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{FilterType, FontSize, Image, Resize};
use ratatui_textarea::{TextArea, WrapMode};

use crate::area_page::{self, Effect};
use crate::backlog::{self, Backlog, Entry};
use crate::config::{self, Source};
use crate::fetch::{self, Fetcher};
use crate::herdr;
use crate::keys::{self, Act, Ctx};
use crate::messages::{self, Message};
use crate::permit::{self, Permit};
use crate::peers;
use crate::repo::{self, name_of};
use crate::report;
use crate::sh::Result;
use crate::status::{self, Checkout, Group, Item};
use crate::tasks;
use crate::usage::Usage;
use crate::{lead, questions, update, update_check, usage, vault};

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
    pub act: Option<Act>,
    pub label: String,
    pub about: String,
}

pub fn choice(act: Act, label: &str, about: &str) -> Choice {
    Choice { key: keys::key(act), act: Some(act), label: label.to_string(), about: about.to_string() }
}

fn task_choices(item: Option<&Item>, dev_server: Option<&str>, dev_worker: Option<&str>) -> Vec<Choice> {
    let go = if item.is_some_and(|item| item.agent.is_none()) {
        choice(Act::Go, "Restart it", "reopen its Space and its Claude, resuming its last conversation")
    } else {
        choice(Act::Go, "Go to its Space", "jump to the task's Space")
    };
    let mut choices = vec![
        go,
        choice(Act::Chat, "Message its Claude", "send it a line without leaving"),
        choice(Act::Review, "Open the review", "the review split next to its Claude"),
    ];
    if let Some(command) = dev_server {
        choices.push(if item.is_some_and(|item| item.server) {
            choice(Act::Server, "Stop the dev server", "ctrl-c to its server tab")
        } else {
            choice(Act::Server, "Start the dev server", &format!("{command} in a tab of its Space"))
        });
    }
    if let Some(command) = dev_worker {
        choices.push(if item.is_some_and(|item| item.worker) {
            choice(Act::Worker, "Stop the worker", "ctrl-c to its worker tab")
        } else {
            choice(Act::Worker, "Start the worker", &format!("{command} in a tab of its Space"))
        });
    }
    if !item.is_some_and(busy) {
        if item.and_then(|item| item.git.as_ref()).is_some_and(|git| git.ahead > 0) {
            choices.push(if item.is_some_and(Item::awaiting_review) {
                choice(Act::SendReview, "Withdraw from review", "stop its reviewer and forget its verdict: push is offered again, and the lead is told")
            } else {
                choice(Act::SendReview, "Send to review", "a separate, read-only Claude reviews its commits; the task waits under To review until it passes them")
            });
        }
        choices.push(choice(Act::Tidy, "Tidy commits", "its Claude reshapes them by the rules skill"));
        if !item.is_some_and(Item::awaiting_review) {
            choices.push(choice(Act::Push, "Push", "rebase, then its card and commits: push them all and close, or the first few and keep it open"));
        }
        choices.push(choice(Act::Done, "Done", "already pushed elsewhere: close it and free the worktree"));
    }
    if item.is_some_and(Item::is_task) {
        choices.push(if item.is_some_and(|item| item.permit == Some(Permit::Task)) {
            choice(Act::Permit, "Take back push permission", "the lead no longer pushes it; it waits for you under Ready to push")
        } else {
            choice(Act::Permit, "Let the lead push it", "the lead may push this task itself once it is landed and approved; done, drop and reset take it back")
        });
    }
    choices.push(choice(Act::Reset, "Reset", "stop it for good: its work goes, its backlog item comes back (asks first)"));
    choices.push(choice(Act::Drop, "Drop", "throw the task away (asks first)"));
    choices
}

// While its Claude works, the branch can still move, so the steps that rewrite, push or close it wait.
fn busy(item: &Item) -> bool {
    item.group() == Group::Working
}

fn ongoing_choices(note: &Ongoing) -> Vec<Choice> {
    vec![match note.task {
        Some(_) => choice(Act::Go, "Go to its task", "select the task that works on this note"),
        None => choice(Act::Go, "Resume as task", "a worktree, a Space and Claude with the note as its prompt"),
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
        choice(Act::Go, "Start as task", "a worktree, a Space and Claude with this as its prompt"),
        choice(Act::Refresh, "Refresh the backlog", &backlog::refresh_label(sources)),
    ];
    if sources.contains(&Source::Local) {
        choices.push(choice(Act::New, "Add an item", "a title and a priority, appended to the local backlog file"));
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
    ConfirmDrop { path: PathBuf, name: String, force: bool, nudged: bool },
    ConfirmReset { path: PathBuf, name: String, nudged: bool },
    ConfirmDone { path: PathBuf, name: String, nudged: bool },
    Filter { buffer: String },
    Card { path: PathBuf, name: String, scroll: u16 },
    Fullscreen { path: PathBuf, name: String, scroll: u16, index: usize },
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
    server_name: String,
    worker_name: String,
}

enum Graphics {
    Ready(Picker),
    Pending { wanted: bool },
    Off,
}

struct ImageAt {
    line: usize,
    path: PathBuf,
    rows: u16,
    area: Size,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Shown {
    Sharp,
    Preview,
    Path,
}

fn image_shown(protocol: Option<ProtocolType>, decoded: bool) -> Shown {
    match protocol {
        Some(_) if !decoded => Shown::Path,
        Some(ProtocolType::Halfblocks) => Shown::Preview,
        Some(_) => Shown::Sharp,
        None => Shown::Path,
    }
}

// herdr answers the Kitty query but not the cell size while no client is attached, and draws Kitty placeholders once one is.
fn choose_protocol(forced: Option<&str>, tty: bool, herdr: bool, answered: Option<ProtocolType>) -> Option<ProtocolType> {
    match forced {
        Some("off") => None,
        Some("halfblocks") => Some(ProtocolType::Halfblocks),
        Some("kitty") => Some(ProtocolType::Kitty),
        Some("sixel") => Some(ProtocolType::Sixel),
        Some("iterm2") => Some(ProtocolType::Iterm2),
        _ if !tty => None,
        _ if herdr && matches!(answered, None | Some(ProtocolType::Halfblocks)) => Some(ProtocolType::Kitty),
        _ => Some(answered.unwrap_or(ProtocolType::Halfblocks)),
    }
}

const GUESSED_CELL: (u16, u16) = (10, 20);

fn cell_size(answered: Option<(u16, u16)>, window: Option<(u16, u16, u16, u16)>) -> Option<(u16, u16)> {
    let known = |(width, height): (u16, u16)| width > 0 && height > 0;
    answered.filter(|cell| known(*cell)).or_else(|| {
        let (columns, rows, width, height) = window?;
        (columns > 0 && rows > 0).then(|| (width / columns, height / rows)).filter(|cell| known(*cell))
    })
}

fn window_pixels() -> Option<(u16, u16, u16, u16)> {
    let size = ratatui::crossterm::terminal::window_size().ok()?;
    Some((size.columns, size.rows, size.width, size.height))
}

fn picker_for(protocol: ProtocolType, (width, height): (u16, u16)) -> Picker {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(FontSize::new(width, height));
    picker.set_protocol_type(protocol);
    picker
}

// Over SSH a viewer would open on the other machine, so o is offered only where the board sits.
fn image_viewer(os: &str, set: &dyn Fn(&str) -> bool) -> Option<&'static str> {
    if set("SSH_CONNECTION") || set("SSH_TTY") {
        return None;
    }
    match os {
        "macos" => Some("open"),
        "linux" if set("DISPLAY") || set("WAYLAND_DISPLAY") => Some("xdg-open"),
        _ => None,
    }
}

fn openable(file: &Path) -> bool {
    use image::ImageFormat::{Gif, Jpeg, Png, WebP};
    file.is_file() && matches!(image::ImageFormat::from_path(file), Ok(Png | Jpeg | Gif | WebP))
}

fn file_url(file: &Path) -> String {
    let mut url = String::from("file://");
    for byte in file.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'.' | b'_' | b'~' => url.push(byte as char),
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

fn preview_note(viewer: bool) -> String {
    if viewer { format!("preview · {} larger · {} opens the image", keys::map().label(Act::Fullscreen), keys::map().label(Act::OpenImage)) } else { format!("preview · {} larger", keys::map().label(Act::Fullscreen)) }
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
    images: HashMap<(PathBuf, u16, u16), Option<Protocol>>,
    cell_guessed: bool,
    viewer: Option<&'static str>,
    card_image_lines: Vec<usize>,
    decoded: HashMap<PathBuf, Option<image::DynamicImage>>,
    image_requests: Option<Sender<PathBuf>>,
    image_results: Option<Receiver<Decoded>>,
    plans: HashMap<String, Text<'static>>,
    updates: Vec<(String, String)>,
    updater: Updater,
    update_running: bool,
    update_shown: bool,
    recheck: Option<Sender<()>>,
    stale: Vec<String>,
    filter: Option<(Column, String)>,
    ctrl_c: Option<Instant>,
    notice: Option<(String, Instant)>,
}

type Updater = std::sync::Arc<dyn Fn() -> Result<String> + Send + Sync>;

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
    if !config::sources().is_ok_and(|sources| sources.contains(&Source::Vault)) && !crate::vault_cli::plugin_enabled() {
        return Vec::new();
    }
    let Ok(notes) = vault::Index::open().and_then(|index| index.ongoing()) else { return Vec::new() };
    let links = tasks::started_links();
    notes.into_iter().map(|note| {
        let task = links.iter().find(|(_, items)| items.contains(&note.key)).map(|(address, _)| address.clone());
        Ongoing { key: note.key, title: note.title, area: note.area, task }
    }).collect()
}

fn spawn_refresher() -> (Receiver<std::result::Result<Snapshot, String>>, Sender<()>) {
    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    let (refresh_tx, refresh_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        loop {
            let _ = messages::flush();
            crate::typing::check();
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

fn spawn_update_checker() -> (Receiver<Vec<(String, String)>>, Sender<()>) {
    let (notice_tx, notice_rx) = mpsc::channel();
    let (recheck_tx, recheck_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        loop {
            let places = update_check::Places::current();
            let notice = crate::lead::state_dir().map(|state| {
                update_check::notice(&update_check::current(&state, messages::now(), places.channel, update_check::installed(&places), || update_check::check(&places)), crate::release::read_override(&state).as_ref())
            }).unwrap_or_default();
            if notice_tx.send(notice).is_err() {
                return;
            }
            let _ = recheck_rx.recv_timeout(Duration::from_secs(600));
        }
    });
    (notice_rx, recheck_tx)
}

fn spawn_restarter() -> Receiver<Vec<String>> {
    let (stale_tx, stale_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut memory = crate::restart::Memory::default();
        loop {
            if let Ok(lines) = crate::restart::run(crate::restart::Mode::Routine { min_idle: 0 }, &mut memory, None)
                && stale_tx.send(lines.into_iter().filter(|line| line.stale).map(|line| format!("{}: {}", line.label, line.said)).collect()).is_err()
            {
                return;
            }
            thread::sleep(Duration::from_secs(60));
        }
    });
    stale_rx
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

const IMAGE_BYTES: u64 = 20 * 1024 * 1024;

// A card names whatever file its task chose, so it is decoded here, capped in size, and a decoder that panics costs the image only.
fn decode_image(path: &Path) -> Option<image::DynamicImage> {
    if std::fs::metadata(path).ok()?.len() > IMAGE_BYTES {
        return None;
    }
    let mut reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reader.decode().ok())).ok().flatten()
}

type Decoded = (PathBuf, Option<image::DynamicImage>);

fn spawn_decoder() -> Option<(Sender<PathBuf>, Receiver<Decoded>)> {
    let (request_tx, request_rx) = mpsc::channel::<PathBuf>();
    let (image_tx, image_rx) = mpsc::channel();
    thread::Builder::new().name("kelpie-images".to_string()).spawn(move || {
        while let Ok(path) = request_rx.recv() {
            let decoded = decode_image(&path);
            if image_tx.send((path, decoded)).is_err() {
                return;
            }
        }
    }).ok()?;
    Some((request_tx, image_rx))
}

// git status --porcelain -z: "XY path" entries split by NUL, a rename or copy followed by its old path as an entry of its own.
fn changed_files(porcelain: &str) -> Vec<String> {
    let mut entries = porcelain.split('\0').filter(|entry| !entry.is_empty());
    let mut files = Vec::new();
    while let Some(entry) = entries.next() {
        if let Some(path) = entry.get(3..) {
            files.push(path.to_string());
        }
        if entry.starts_with(['R', 'C']) {
            entries.next();
        }
    }
    files
}

fn gather_detail(path: &Path, repo: Option<&Path>) -> Detail {
    let settings = repo.and_then(|repo| config::resolve(Some(repo)).ok()).map(|resolved| resolved.settings);
    let changed = changed_files(&crate::sh::run_untrimmed("git", &["status", "--porcelain", "-z"], Some(path)).unwrap_or_default());
    Detail {
        branch: repo::git(path, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default(),
        commits: tasks::commits(path).unwrap_or_default(),
        changed,
        card: report::card_for(path),
        dev_server: settings.as_ref().is_some_and(|settings| settings.dev_server.value.is_some()),
        dev_worker: settings.as_ref().is_some_and(|settings| settings.dev_worker.value.is_some()),
        server_name: Process::Server.tab(settings.as_ref()),
        worker_name: Process::Worker.tab(settings.as_ref()),
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
        self.setting(settings).value.clone()
    }

    fn setting(self, settings: &config::Settings) -> &config::Setting<Option<String>> {
        match self {
            Process::Server => &settings.dev_server,
            Process::Worker => &settings.dev_worker,
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

// The review split runs scripts/review.sh list, and nvim or fzf under it while a file is open.
fn runs_review(info: &serde_json::Value) -> bool {
    info["foreground_processes"].as_array().into_iter().flatten().any(|process| {
        let argv: Vec<&str> = process["argv"].as_array().into_iter().flatten().filter_map(|arg| arg.as_str()).collect();
        argv.iter().any(|arg| arg.ends_with("review.sh")) && argv.contains(&"list")
    })
}

fn review_pane_in(panes: &[(String, serde_json::Value)]) -> Option<String> {
    panes.iter().find(|(_, info)| runs_review(info)).map(|(pane, _)| pane.clone())
}

fn open_review_pane(workspace: &str) -> Option<String> {
    let panes: Vec<(String, serde_json::Value)> = herdr::panes_in(workspace).ok()?.into_iter()
        .filter_map(|pane| pane["pane_id"].as_str().map(str::to_string))
        .filter_map(|pane| herdr::process_info(&pane).ok().map(|info| (pane, info)))
        .collect();
    review_pane_in(&panes)
}

// herdr focuses a pane only by its neighbour, so the pane is reached from whichever side has one.
fn focus_pane(pane: &str) -> Result<()> {
    if herdr::focused_pane()?.as_deref() == Some(pane) {
        return Ok(());
    }
    if let Some(tab) = herdr::tab_of(pane)? {
        herdr::focus_tab(&tab)?;
    }
    for (side, back) in [("left", "right"), ("right", "left"), ("up", "down"), ("down", "up")] {
        if let Some(neighbour) = herdr::neighbour(pane, side)? {
            herdr::focus_beside(&neighbour, back)?;
            if herdr::focused_pane()?.as_deref() == Some(pane) {
                return Ok(());
            }
        }
    }
    if herdr::focused_pane()?.as_deref() == Some(pane) { Ok(()) } else { Err(format!("herdr did not move the focus to {pane}")) }
}

fn review_script() -> Option<PathBuf> {
    Some(lead::install_root().ok()?.join("scripts/review.sh"))
}

impl App {
    fn selected_task(&self) -> Option<Item> {
        self.tasks.get(self.work_selected).filter(|_| self.shown(Column::Work, self.work_selected)).cloned()
    }

    fn selected_entry(&self) -> Option<Entry> {
        self.backlog.entries.get(self.backlog_selected.checked_sub(self.ongoing.len())?).filter(|_| self.shown(Column::Backlog, self.backlog_selected)).cloned()
    }

    fn filter_of(&self, column: Column) -> Option<&str> {
        self.filter.as_ref().filter(|(on, text)| *on == column && !text.is_empty()).map(|(_, text)| text.as_str())
    }

    fn shown(&self, column: Column, index: usize) -> bool {
        let Some(filter) = self.filter_of(column) else { return true };
        let wanted = filter.to_lowercase();
        let has = |text: &str| text.to_lowercase().contains(&wanted);
        match column {
            Column::Work => self.tasks.get(index).is_some_and(|item| has(&item.name) || has(&item.place)),
            Column::Backlog => match index.checked_sub(self.ongoing.len()) {
                None => self.ongoing.get(index).is_some_and(|note| has(&note.title) || has(&note.area)),
                Some(entry) => self.backlog.entries.get(entry).is_some_and(|entry| has(&entry.title) || has(&entry.area)),
            },
        }
    }

    fn ctx(&self) -> Ctx {
        match self.column {
            Column::Work => Ctx::Work,
            Column::Backlog => Ctx::Backlog,
        }
    }

    fn say(&mut self, text: String) {
        self.notice = Some((text, Instant::now()));
    }

    fn set_filter(&mut self, text: String) {
        let column = self.column;
        self.filter = Some((column, text));
        let length = match column { Column::Work => self.tasks.len(), Column::Backlog => self.backlog_len() };
        if let Some(first) = (0..length).find(|index| self.shown(column, *index)) {
            match column {
                Column::Work => self.work_selected = first,
                Column::Backlog => self.backlog_selected = first,
            }
        }
    }

    fn choices(&self) -> Vec<Choice> {
        match self.column {
            Column::Work => {
                let item = self.tasks.get(self.work_selected);
                let settings = item.and_then(settings_of);
                let command = |process: Process| settings.as_ref().and_then(|settings| process.command(settings));
                let mut choices = task_choices(item, command(Process::Server).as_deref(), command(Process::Worker).as_deref());
                if item.is_some_and(|item| self.question_of(item).is_some()) {
                    choices.insert(0, choice(Act::Inbox, "Answer its question", "the inbox, at this task's question"));
                }
                if item.is_some_and(|item| self.card_of(item).is_some()) {
                    let after = choices.iter().position(|choice| choice.act == Some(Act::Review)).map_or(choices.len(), |at| at + 1);
                    choices.insert(after, choice(Act::Card, "Show the card", "its sign-off card with its images; p goes on to push"));
                }
                choices
            }
            Column::Backlog if self.backlog_selected < self.ongoing.len() => ongoing_choices(&self.ongoing[self.backlog_selected]),
            Column::Backlog => backlog_choices(&self.sources),
        }
    }

    fn card_of(&self, item: &Item) -> Option<std::rc::Rc<Detail>> {
        self.details.get(item.path.as_ref()?).filter(|detail| detail.card.is_some()).cloned()
    }

    fn selected_has_card(&self) -> bool {
        self.column == Column::Work && self.selected_task().is_some_and(|item| self.card_of(&item).is_some())
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
    fn image_size(&mut self, path: &Path, area: Size) -> Option<Size> {
        if !path.is_file() {
            return None;
        }
        self.settle_cell();
        if let Graphics::Pending { wanted } = &mut self.graphics {
            *wanted = true;
        }
        let Graphics::Ready(picker) = &self.graphics else { return None };
        let key = (path.to_path_buf(), area.width, area.height);
        if !self.images.contains_key(&key) {
            let Some(decoded) = self.decoded.get(path) else {
                if let Some(requests) = &self.image_requests {
                    let _ = requests.send(path.to_path_buf());
                }
                self.decoded.insert(path.to_path_buf(), None);
                return None;
            };
            let protocol = decoded.clone().and_then(|decoded| picker.new_protocol(decoded, area, Resize::Scale(Some(FilterType::Lanczos3))).ok());
            self.images.insert(key.clone(), protocol);
        }
        self.images.get(&key)?.as_ref().map(|protocol| protocol.size())
    }

    // A board started while herdr had no client attached learns its cell size once the window reports pixels.
    fn settle_cell(&mut self) {
        if !self.cell_guessed {
            return;
        }
        let Graphics::Ready(picker) = &self.graphics else { return };
        if let Some(cell) = cell_size(None, window_pixels()) {
            self.graphics = Graphics::Ready(picker_for(picker.protocol_type(), cell));
            self.images.clear();
            self.cell_guessed = false;
        }
    }

    fn protocol(&self) -> Option<ProtocolType> {
        match &self.graphics {
            Graphics::Ready(picker) => Some(picker.protocol_type()),
            Graphics::Pending { .. } | Graphics::Off => None,
        }
    }

    fn card_images(&self, path: &Path) -> Vec<PathBuf> {
        let Some((card, _)) = self.details.get(path).and_then(|detail| detail.card.as_ref()) else { return Vec::new() };
        card.text.lines().filter_map(card_image_in).map(|(_, image)| path.join(image)).collect()
    }

    fn open_image(&mut self, file: &Path) {
        let Some(viewer) = self.viewer else { return };
        if !openable(file) {
            return self.say(format!("{} is not an image file kelpie opens", file.display()));
        }
        match Command::new(viewer).arg(file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(mut child) => {
                thread::spawn(move || child.wait());
            }
            Err(error) => self.say(format!("{viewer}: {error}")),
        }
    }

    fn take_images(&mut self) {
        let Some(results) = &self.image_results else { return };
        while let Ok((path, decoded)) = results.try_recv() {
            self.images.retain(|(drawn, _, _), _| *drawn != path);
            self.decoded.insert(path, decoded);
        }
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

    fn in_background(&mut self, label: String, step: impl FnOnce() -> Result<String> + Send + 'static) {
        let (sender, result) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(step());
        });
        self.mode = Mode::Working { label, result };
    }

    fn start_update(&mut self) {
        let updater = self.updater.clone();
        self.update_running = true;
        self.in_background("Updating kelpie, its plugins and the vault plugin…".to_string(), move || updater());
    }

    fn take_working_result(&mut self) {
        let Mode::Working { result, .. } = &self.mode else { return };
        let Ok(result) = result.try_recv() else { return };
        if std::mem::take(&mut self.update_running) {
            self.update_shown = true;
            self.updates.clear();
            if let Some(recheck) = &self.recheck {
                let _ = recheck.send(());
            }
        }
        self.show(result);
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
        match keys::map().act(self.ctx(), &key) {
            Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => self.message_back = (self.message_back + 1).min(self.messages.len().saturating_sub(1)),
            Some(Act::Move) => self.message_back = self.message_back.saturating_sub(1),
            Some(Act::Back | Act::Close) => self.in_messages = false,
            Some(Act::Messages) => self.press_m(),
            Some(Act::Help) => self.mode = Mode::Help,
            Some(Act::Inbox) => self.open_questions(None),
            Some(Act::Options) => {
                if let Some(message) = self.selected_message() {
                    self.mode = Mode::Message(format!("{} {} → {}\n\n{}", clock(message.at), message.from, message.to, message.text));
                }
            }
            _ => {}
        }
    }

    fn move_selection(&mut self, step: isize) {
        let column = self.column;
        let (selected, length) = match column {
            Column::Work => (self.work_selected, self.tasks.len()),
            Column::Backlog => (self.backlog_selected, self.ongoing.len() + self.backlog.entries.len()),
        };
        let mut at = selected;
        let next = loop {
            let Some(moved) = at.checked_add_signed(step).filter(|moved| *moved < length) else { break selected };
            if self.shown(column, moved) {
                break moved;
            }
            at = moved;
        };
        match column {
            Column::Work => self.work_selected = next,
            Column::Backlog => self.backlog_selected = next,
        }
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
                        self.in_messages &= code == keys::map().code(Act::Messages);
                        return Some(KeyEvent::from(code));
                    }
                    (Some(Hit::Choice(index)), Mode::Options(_)) => {
                        if let Some(act) = self.choices().get(index).and_then(|choice| choice.act) {
                            self.mode = Mode::Normal;
                            self.act(act);
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
        self.handle_key(key, Some(terminal));
    }

    // Without a terminal (in tests) the steps that leave the board for an editor or a long start are skipped.
    fn handle_key(&mut self, key: KeyEvent, terminal: Option<&mut DefaultTerminal>) {
        let map = keys::map();
        if keys::ctrl_c(&key) {
            if self.ctrl_c.is_some_and(|at| at.elapsed() < Duration::from_secs(2)) {
                self.quit = true;
            } else {
                self.ctrl_c = Some(Instant::now());
                self.say(format!("{} again to quit", map.label(Act::Quit)));
            }
            return;
        }
        self.notice = None;
        if self.at_rest() {
            let ctx = if self.page == Page::Areas { Ctx::Areas } else { self.ctx() };
            match map.act(ctx, &key) {
                Some(Act::Lead) => {
                    if let Err(error) = lead::focus_toggle() {
                        self.mode = Mode::Message(error);
                    }
                    return;
                }
                Some(Act::WorkPage) => return self.page = Page::Work,
                Some(Act::AreasPage) => return self.page = Page::Areas,
                Some(Act::Inbox) => {
                    self.page = Page::Work;
                    return self.open_questions(None);
                }
                _ => {}
            }
        }
        if self.page == Page::Areas {
            let top = self.areas.is_normal();
            let Some(terminal) = terminal else { return };
            match self.areas.handle(key, terminal) {
                Effect::Left => self.quit = self.overlay,
                Effect::Unhandled if top => self.at_top(map.act(Ctx::Areas, &key)),
                _ => {}
            }
            return;
        }
        let act = map.act(self.ctx(), &key);
        let cancels = matches!(act, Some(Act::Back | Act::Close)) || key.code == KeyCode::Char('n');
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.handle_normal(key),
            Mode::Options(index) => {
                let choices = self.choices();
                let count = choices.len();
                let typed = match key.code {
                    KeyCode::Char(character) => choices.iter().find(|choice| choice.key == character).and_then(|choice| choice.act),
                    _ => None,
                };
                match (typed, act) {
                    (Some(chosen), _) => self.act(chosen),
                    (None, Some(Act::Move)) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => self.mode = Mode::Options(index.saturating_sub(1)),
                    (None, Some(Act::Move)) => self.mode = Mode::Options((index + 1).min(count.saturating_sub(1))),
                    (None, Some(Act::Options)) => {
                        if let Some(chosen) = choices.get(index).and_then(|choice| choice.act) {
                            self.act(chosen);
                        }
                    }
                    (None, Some(Act::Back | Act::Close)) => {}
                    _ => self.mode = Mode::Options(index),
                }
            }
            Mode::Help | Mode::Message(_) => {}
            working @ Mode::Working { .. } => self.mode = working,
            Mode::Questions(window) => self.handle_questions(window, key),
            Mode::ConfirmReset { path, name, .. } => match key.code {
                KeyCode::Char('y') => self.in_background("Resetting the task…".to_string(), move || tasks::reset(&path, None)),
                _ if cancels => {}
                _ => self.mode = Mode::ConfirmReset { path, name, nudged: true },
            },
            Mode::ConfirmDone { path, name, .. } => match key.code {
                KeyCode::Char('y') => self.in_background(format!("Closing {name}…"), move || tasks::done(&path, None)),
                _ if cancels => {}
                _ => self.mode = Mode::ConfirmDone { path, name, nudged: true },
            },
            Mode::ConfirmDrop { path, name, force, .. } => match key.code {
                KeyCode::Char('y') => match tasks::drop(&path, None, force) {
                    Err(error) if !force && error.contains("--force") => {
                        self.mode = Mode::ConfirmDrop { path, name: format!("{name}: {error}"), force: true, nudged: false };
                    }
                    result => self.show(result),
                },
                _ if cancels => {}
                _ => self.mode = Mode::ConfirmDrop { path, name, force, nudged: true },
            },
            Mode::Card { path, name, scroll } => match map.act(Ctx::Card, &key) {
                Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => self.mode = Mode::Card { path, name, scroll: scroll.saturating_sub(1) },
                Some(Act::Move) => self.mode = Mode::Card { path, name, scroll: scroll.saturating_add(1) },
                Some(Act::Push) => self.act(Act::Push),
                Some(Act::Back | Act::Close) => {}
                Some(act @ (Act::Fullscreen | Act::OpenImage)) => {
                    let files = self.card_images(&path);
                    let index = self.card_image_lines.iter().position(|line| *line >= scroll as usize).unwrap_or(0).min(files.len().saturating_sub(1));
                    match files.get(index) {
                        Some(_) if act == Act::Fullscreen && self.protocol().is_some() => self.mode = Mode::Fullscreen { path, name, scroll, index },
                        Some(file) if act == Act::OpenImage => {
                            let file = file.clone();
                            self.mode = Mode::Card { path, name, scroll };
                            self.open_image(&file);
                        }
                        _ => self.mode = Mode::Card { path, name, scroll },
                    }
                }
                _ => match key.code {
                    KeyCode::PageUp => self.mode = Mode::Card { path, name, scroll: scroll.saturating_sub(10) },
                    KeyCode::PageDown | KeyCode::Char(' ') => self.mode = Mode::Card { path, name, scroll: scroll.saturating_add(10) },
                    _ => self.mode = Mode::Card { path, name, scroll },
                },
            },
            Mode::Fullscreen { path, name, scroll, index } => {
                let count = self.card_images(&path).len().max(1);
                match map.act(Ctx::Card, &key) {
                    Some(Act::Back | Act::Close | Act::Fullscreen) => self.mode = Mode::Card { path, name, scroll },
                    Some(Act::Switch) if key.code == KeyCode::Left => self.mode = Mode::Fullscreen { path, name, scroll, index: (index + count - 1) % count },
                    Some(Act::Switch) => self.mode = Mode::Fullscreen { path, name, scroll, index: (index + 1) % count },
                    Some(Act::OpenImage) => {
                        let file = self.card_images(&path).get(index).cloned();
                        self.mode = Mode::Fullscreen { path, name, scroll, index };
                        if let Some(file) = file {
                            self.open_image(&file);
                        }
                    }
                    _ => self.mode = Mode::Fullscreen { path, name, scroll, index },
                }
            }
            Mode::Filter { mut buffer } => match key.code {
                KeyCode::Esc => self.filter = None,
                KeyCode::Enter => {}
                KeyCode::Backspace => {
                    buffer.pop();
                    self.set_filter(buffer.clone());
                    self.mode = Mode::Filter { buffer };
                }
                KeyCode::Char(character) => {
                    buffer.push(character);
                    self.set_filter(buffer.clone());
                    self.mode = Mode::Filter { buffer };
                }
                _ => self.mode = Mode::Filter { buffer },
            },
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
                        if let Some(Ok(Some(message))) = terminal.map(|terminal| edit_in_editor(terminal, &editor.message())) {
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

    // esc and q at the top: an overlay board closes; the full board stays, as it is quit with ctrl+c twice.
    fn at_top(&mut self, act: Option<Act>) {
        match act {
            Some(Act::Back) if self.filter.is_some() && self.page == Page::Work => self.filter = None,
            Some(Act::Back | Act::Close) if self.overlay => self.quit = true,
            Some(Act::Close) => {
                let quit = keys::map().label(Act::Quit);
                self.say(format!("{quit} twice to quit"));
            }
            _ => {}
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        if self.in_messages {
            return self.handle_messages(key);
        }
        let ctx = self.ctx();
        let Some(act) = keys::map().act(ctx, &key) else { return };
        match act {
            Act::Messages => self.press_m(),
            Act::Move => self.move_selection(if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) { -1 } else { 1 }),
            Act::Switch => self.column = if self.column == Column::Work { Column::Backlog } else { Column::Work },
            Act::Back | Act::Close => self.at_top(Some(act)),
            Act::Help => self.mode = Mode::Help,
            Act::Inbox => self.open_questions(None),
            Act::Options => {
                let has_selection = match self.column {
                    Column::Work => self.selected_task().is_some(),
                    Column::Backlog => self.backlog_len() > 0 && self.shown(Column::Backlog, self.backlog_selected),
                };
                if has_selection {
                    self.mode = Mode::Options(0);
                }
            }
            Act::Refresh => self.refresh_backlog(),
            Act::Away => self.show(permit::away(Some(if permit::away_since().is_some() { "off" } else { "on" }))),
            Act::Update => self.start_update(),
            Act::Filter => {
                let buffer = self.filter_of(self.column).unwrap_or_default().to_string();
                self.mode = Mode::Filter { buffer };
            }
            Act::New if ctx == Ctx::Backlog && self.sources.contains(&Source::Local) => self.act(Act::New),
            Act::Card if ctx == Ctx::Work => self.act(Act::Card),
            Act::New => self.mode = Mode::Input { purpose: Purpose::TaskName, label: "New task: name".to_string(), buffer: String::new() },
            other if self.choices().iter().any(|choice| choice.act == Some(other)) => self.act(other),
            _ => {}
        }
    }

    fn handle_questions(&mut self, mut window: Box<QuestionWindow>, key: KeyEvent) {
        let count = self.questions.len();
        let options = self.questions.get(window.selected).and_then(|message| message.question.as_ref()).map_or(0, |question| question.options.len());
        let typing = window.part == Part::Comment;
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Char(character) if !typing && character == keys::key(Act::Close) => return,
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
            Some(pane) => crate::typing::prompt(&pane, &prompt, "the lead").map(|_| "Asked the lead to refresh the backlog; it updates here when it is saved.".to_string()).map_err(|refused| format!("Nothing was typed: the lead {}; ask again once it is clear.", refused.reason())),
            None => Err("No lead is running; prefix+space starts it.".to_string()),
        });
        self.show(result);
    }

    fn act(&mut self, act: Act) {
        if !self.shown(self.column, if self.column == Column::Work { self.work_selected } else { self.backlog_selected }) {
            return;
        }
        if self.column == Column::Backlog && let Some(note) = self.ongoing.get(self.backlog_selected).cloned().filter(|_| act == Act::Go) {
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
            Column::Backlog => match act {
                Act::Go => {
                    if let Some(entry) = self.selected_entry() {
                        let buffer = suggested_name(&entry.title);
                        self.mode = Mode::Input { purpose: Purpose::BacklogName(entry), label: "Start as task: name".to_string(), buffer };
                    }
                }
                Act::Refresh => self.refresh_backlog(),
                Act::New => {
                    let label = "Add to the backlog: title (— summary and (area) optional)".to_string();
                    self.mode = Mode::Input { purpose: Purpose::BacklogItem, label, buffer: String::new() };
                }
                _ => {}
            },
            Column::Work => {
                let Some(item) = self.selected_task() else { return };
                match act {
                    Act::Go if item.agent.is_none() => {
                        let Some(path) = item.path.clone() else { return };
                        let (sender, result) = mpsc::channel();
                        let name = item.name.clone();
                        thread::spawn(move || {
                            let options = tasks::Start { prompt: None, focus: true, from: None, mode: None, backlog: None };
                            let _ = sender.send(tasks::start(&path, &name, &options));
                        });
                        self.mode = Mode::Working { label: format!("Restarting {}…", item.name), result };
                    }
                    Act::Go => {
                        if let Some(workspace) = &item.workspace {
                            let _ = herdr::focus_workspace(workspace);
                            if self.overlay {
                                self.quit = true;
                            }
                        }
                    }
                    Act::Chat => {
                        if let Some(agent) = item.agent {
                            let label = format!("Message to {}", item.name);
                            self.mode = Mode::Input { purpose: Purpose::Message(agent.pane, item.name), label, buffer: String::new() };
                        }
                    }
                    Act::Review => self.open_review(&item),
                    Act::Card => match (item.path.clone(), self.card_of(&item)) {
                        (Some(path), Some(_)) => self.mode = Mode::Card { path, name: item.name.clone(), scroll: 0 },
                        _ => self.show(Ok(format!("{} has no sign-off card yet; it writes one when it reports ready.", item.name))),
                    },
                    Act::Inbox => {
                        let at = self.question_of(&item);
                        self.open_questions(at);
                    }
                    Act::Tidy | Act::Push | Act::Done | Act::SendReview if busy(&item) => self.show(Err(format!("{} is still working; wait until it reports.", item.name))),
                    Act::Push if item.awaiting_review() => self.show(Err(format!("{} is in review; push opens once its reviewer passes its commits.", item.name))),
                    Act::Tidy => {
                        if let Some(path) = item.path.clone() {
                            let label = "How should the commits look? (optional, enter to let Claude decide)".to_string();
                            self.mode = Mode::Input { purpose: Purpose::Tidy(path), label, buffer: String::new() };
                        }
                    }
                    Act::Server => self.toggle(&item, Process::Server),
                    Act::Worker => self.toggle(&item, Process::Worker),
                    Act::Permit if item.is_task() => self.show(permit::toggle_task(&peers::address(&item.name, &item.place))),
                    Act::SendReview => {
                        if let Some(path) = &item.path {
                            self.show(if item.awaiting_review() { crate::review::withdraw(path, None) } else { crate::review::request(path, None) });
                        }
                    }
                    Act::Reset => {
                        if let Some(path) = item.path.clone() {
                            self.mode = Mode::ConfirmReset { path, name: item.name, nudged: false };
                        }
                    }
                    Act::Push | Act::Done | Act::Drop => {
                        let Some(path) = item.path.clone() else { return };
                        match act {
                            Act::Push => match tasks::land(&path, None) {
                                Ok(_) => match tasks::commits(&path) {
                                    Ok(commits) => {
                                        let card = report::card_for(&path);
                                        self.mode = Mode::ConfirmPush { path, commits, selected: 0, card };
                                    }
                                    Err(error) => self.show(Err(error)),
                                },
                                error => self.show(error),
                            },
                            Act::Done => self.mode = Mode::ConfirmDone { path, name: item.name, nudged: false },
                            _ => self.mode = Mode::ConfirmDrop { path, name: item.name, force: false, nudged: false },
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
        let busy = busy_tab_pane(workspace, tab);
        let result = match busy {
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

    // The review opens next to the task's own Claude, so comments reach that agent; one already open in the Space gets the focus instead of a second split.
    fn open_review(&mut self, item: &Item) {
        let (Some(path), Some(agent), Some(script)) = (&item.path, &item.agent, review_script()) else {
            self.mode = Mode::Message("A review needs a task with its Claude running.".to_string());
            return;
        };
        if let Some(workspace) = &item.workspace
            && let Some(pane) = open_review_pane(workspace)
        {
            let _ = herdr::focus_workspace(workspace);
            if let Err(error) = focus_pane(&pane) {
                self.mode = Mode::Message(format!("The review is open in {}'s Space, but it could not get the focus: {error}", item.name));
                return;
            }
            if self.overlay {
                self.quit = true;
            }
            return;
        }
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

    fn submit(&mut self, purpose: Purpose, text: String, terminal: Option<&mut DefaultTerminal>) {
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
                let prompt = backlog::task_prompt(&entry, &self.sources, crate::vault_cli::known_on());
                let known = self.repo_named(&entry.area);
                let located = || match terminal {
                    Some(terminal) => outside(terminal, || repo::locate(&entry.area)),
                    None => repo::locate(&entry.area),
                };
                match known.map_or_else(located, Ok) {
                    Ok(repo) => self.show(start_in_background(&repo, &text, Some(&prompt), Some(&entry.reference))),
                    Err(error) => self.show(Err(error)),
                }
            }
            Purpose::Tidy(path) => {
                let result = tasks::tidy(&path, None, &text);
                self.show(result);
            }
            Purpose::Message(_, _) if text.is_empty() => {}
            Purpose::Message(pane, name) => {
                let result = crate::typing::prompt(&pane, &text, &name).map(|_| format!("Sent to {name}.")).map_err(|refused| format!("Nothing was sent: {name}'s Claude {}; your message was:\n{text}", refused.reason()));
                self.show(result);
            }
        }
    }
}

// The board stays where it is while the task's worktree and Claude come up; the task shows under Work once its Space exists.
fn start_in_background(repo: &Path, name: &str, prompt: Option<&str>, backlog: Option<&str>) -> Result<String> {
    let dir = crate::lead::state_dir()?.join("starts");
    crate::state::private_dir(&dir)?;
    let log = dir.join(format!("{}.log", name.replace('/', "-")));
    let output = crate::state::create(&log).map_err(|error| format!("{}: {error}", log.display()))?;
    let errors = output.try_clone().map_err(|error| error.to_string())?;
    let mut command = Command::new(peers::kelpie_command()?);
    command.arg("-C").arg(repo).args(["start", name, "--no-focus"]);
    if let Some(item) = backlog.filter(|item| !item.is_empty()) {
        command.args(["--backlog", item]);
    }
    if let Some(prompt) = prompt {
        command.arg(prompt);
    }
    // Its own process group, so closing the board's popup does not take the start down with it.
    command.stdin(std::process::Stdio::null()).stdout(output).stderr(errors).process_group(0);
    crate::sh::reap(command.spawn().map_err(|error| format!("kelpie start: {error}"))?);
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
    if !app.stale.is_empty() {
        spans.extend(pill(format!("↻ {} stale", app.stale.len()), MAUVE, true));
    }
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
    let inbox = keys::key(Act::Inbox);
    let suffix = if plan { format!(" · plan to approve · {inbox}") } else { format!(" · {} · {inbox}", plural(count, "option")) };
    let one_line = message.text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{}{suffix}", cut(&one_line, width.saturating_sub(suffix.chars().count()).max(8)))
}

fn card_field(card: &report::Card, current: bool, width: usize) -> Vec<Line<'static>> {
    let (mark, color) = if current { ("✓ ", GREEN) } else { ("○ ", YELLOW) };
    let text: Vec<&str> = card.text.lines().map(str::trim).filter(|line| !line.is_empty() && card_image_in(line).is_none()).take(2).collect();
    let room = width.saturating_sub(LABEL + 2);
    let mut lines = Vec::new();
    for (index, line) in text.iter().enumerate() {
        let mut spans = if index == 0 { vec![Span::styled(mark, style(color))] } else { vec![Span::raw("  ")] };
        spans.push(Span::styled(cut(line, room), style(TEXT)));
        lines.push(field(if index == 0 { "card" } else { "" }, spans));
    }
    if text.is_empty() {
        lines.push(field("card", vec![Span::styled(mark, style(color))]));
    }
    let images = card.text.lines().filter(|line| card_image_in(line).is_some()).count();
    if images > 0 {
        lines.push(field("", vec![Span::styled(format!("▣ {} · {} shows the card", plural(images, "image"), keys::key(Act::Card)), style(MUTED))]));
    }
    lines
}

fn card_image_in(line: &str) -> Option<(&str, &str)> {
    line.trim().strip_prefix("![").and_then(|rest| rest.split_once("](")).map(|(alt, rest)| (alt, rest.trim_end_matches(')')))
}

fn card_hint() -> Line<'static> {
    Line::from(vec![Span::raw("   "), Span::styled("⏎", style(MAUVE)), Span::styled(" options · ", style(MUTED)), Span::styled(keys::key(Act::Card).to_string(), style(MAUVE)), Span::styled(" card", style(MUTED))]).style(Style::new().bg(SURFACE))
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
    let right = match app.filter_of(Column::Work) {
        Some(filter) => format!("{} / {filter}", plural((0..app.tasks.len()).filter(|index| app.shown(Column::Work, *index)).count(), "task")),
        None => format!("{}{}", plural(app.tasks.len(), "task"), free_worktrees(&app.checkouts)),
    };
    let mut lines = column_title("WORK", right, focused, width);
    let mut selected_line = 0;
    let mut current: Option<Group> = None;
    let tasks = app.tasks.clone();
    let mut rows = Vec::new();
    for (index, item) in tasks.iter().enumerate() {
        if !app.shown(Column::Work, index) {
            continue;
        }
        if current != Some(item.group()) {
            if current.is_some() {
                lines.push(Line::default());
            }
            let count = tasks.iter().enumerate().filter(|(other_index, other)| other.group() == item.group() && app.shown(Column::Work, *other_index)).count();
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
            lines.push(if app.card_of(item).is_some() { card_hint() } else { options_hint() });
        }
        rows.push((start, lines.len(), Hit::Task(index)));
    }
    if tasks.is_empty() {
        lines.push(Line::from(Span::styled(format!("No tasks. {} starts one, or pick one from the backlog.", keys::key(Act::New)), style(MUTED))));
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
fn server_line(item: &Item, configured: bool, name: &str, width: usize) -> Option<(Line<'static>, Option<Link>)> {
    if !item.server {
        return configured.then(|| (field(name, vec![Span::styled(format!("○ off · {} starts it", keys::key(Act::Server)), style(MUTED))]), None));
    }
    let Some(url) = &item.url else { return Some((field(name, vec![Span::styled("● running", style(GREEN))]), None)) };
    let room = width.saturating_sub(LABEL + 12).max(2);
    let shown = cut(url, room);
    let line = field(name, vec![Span::styled("● running", style(GREEN)), Span::raw("  "), Span::styled(shown.clone(), style(TEAL))]);
    Some((line, Some(Link { shown, url: url.clone() })))
}

fn worker_line(item: &Item, configured: bool, name: &str) -> Option<Line<'static>> {
    if item.worker {
        Some(field(name, vec![Span::styled("● running", style(GREEN))]))
    } else {
        configured.then(|| field(name, vec![Span::styled(format!("○ off · {} starts it", keys::key(Act::Worker)), style(MUTED))]))
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
        lines.push(field("claude", vec![Span::styled(format!("not running in its Space · {} restarts it", keys::key(Act::Go)), style(MUTED))]));
    }
    if let Some((line, link)) = server_line(item, detail.dev_server, &detail.server_name, width) {
        lines.push(line);
        links.extend(link);
    }
    lines.extend(worker_line(item, detail.dev_worker, &detail.worker_name));
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
        if let Some((card, current)) = &detail.card {
            lines.extend(card_field(card, *current, width));
        }
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
            let area = Size::new(width as u16, 14);
            if let (Some(file), Some(shown)) = (&path, card_image(&line)) {
                links.extend(file_link(&shown, file));
            }
            match path.as_ref().and_then(|path| app.image_size(path, area).map(|size| (path.clone(), size.height))) {
                Some((path, rows)) => {
                    lines.push(line);
                    images.push(ImageAt { line: lines.len(), path, rows, area });
                    lines.extend((0..rows).map(|_| Line::default()));
                }
                None => lines.push(line),
            }
        }
    }
    lines.push(Line::default());
    let mut foot = vec![Span::styled("⏎", style(MAUVE)), Span::styled(" options", style(MUTED))];
    if detail.card.is_some() {
        foot.extend([Span::styled(" · ", style(MUTED)), Span::styled(keys::key(Act::Card).to_string(), style(MAUVE)), Span::styled(" card", style(MUTED))]);
    }
    lines.push(Line::from(foot));
    (lines, links, images)
}

fn file_link(shown: &str, file: &Path) -> Option<Link> {
    file.is_absolute().then(|| Link { shown: shown.to_string(), url: file_url(file) })
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
        if let Some(Some(protocol)) = app.images.get(&(image.path, image.area.width, image.area.height)) {
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
        if !app.shown(Column::Backlog, index) {
            continue;
        }
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
    let right = match app.filter_of(Column::Backlog) {
        Some(filter) => format!("{} / {filter}", plural((0..app.backlog_len()).filter(|index| app.shown(Column::Backlog, *index)).count(), "item")),
        None => format!("{} items", app.backlog.entries.len()),
    };
    let mut lines = column_title("BACKLOG", right, focused, width);
    let mut rows = Vec::new();
    let mut selected_line = ongoing_rows(app, focused, width, &mut lines, &mut rows).unwrap_or(0);
    let offset = app.ongoing.len();
    let mut current: Option<&str> = None;
    for (index, entry) in app.backlog.entries.iter().enumerate() {
        if !app.shown(Column::Backlog, index + offset) {
            continue;
        }
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

use crate::keys::Hints;

fn status_hints(keys: &Hints) -> Line<'static> {
    Line::from(Span::styled(keys.iter().map(|(key, label, _)| format!("{key} {label}")).collect::<Vec<_>>().join(" · "), style(MUTED)))
}

fn work_hints(card: bool) -> (Hints, Hints) {
    let (mut full, mut short) = keys::map().hints(Ctx::Work);
    if card {
        let entry = (keys::key(Act::Card).to_string(), "card", keys::map().code(Act::Card));
        full.insert(1, entry.clone());
        short.insert(1, entry);
    }
    (full, short)
}

fn hint_lines(card: bool) -> (Line<'static>, Line<'static>) {
    let (full, short) = work_hints(card);
    (status_hints(&full), status_hints(&short))
}

fn hint_hits(line: &Line, area: Rect, card: bool) -> Vec<(Rect, Hit)> {
    let text = line.to_string();
    let mut hits = Vec::new();
    let update = keys::map().code(Act::Update);
    let update_hints = [(update_hint(), update), (short_update_hint(), update)];
    for (hint, code) in work_hints(card).0.into_iter().map(|(key, label, code)| (format!("{key} {label}"), code)).chain(update_hints) {
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

fn update_notice(updates: &[(String, String)], long: bool) -> Vec<Span<'static>> {
    let text = if long {
        let names: Vec<String> = updates.iter().map(|(name, version)| format!("{name} {version}")).collect();
        format!("↑ {} · {}", names.join(" · "), update_hint())
    } else {
        short_update_hint()
    };
    vec![Span::raw("   "), Span::styled(text, style(MUTED))]
}

fn update_hint() -> String {
    format!("{} updates", keys::key(Act::Update))
}

fn short_update_hint() -> String {
    format!("↑ update · {}", keys::key(Act::Update))
}

fn status_line(full: Line<'static>, short: Line<'static>, updates: &[(String, String)], usage: Option<&Usage>, width: usize, now: u64) -> Line<'static> {
    let right = |wide: bool| usage.map(|usage| usage_spans(usage, wide, now)).unwrap_or_default();
    let width_of = |spans: &[Span]| spans.iter().map(|span| span.width()).sum::<usize>();
    let left_width = |line: &Line| line.width() - line.spans.last().map_or(0, |span| span.content.len() - span.content.trim_end().len());
    let notices: Vec<Vec<Span<'static>>> = if updates.is_empty() { Vec::new() } else { vec![update_notice(updates, true), update_notice(updates, false)] };
    let short_text = short.to_string();
    let shortest = Line::from(Span::styled(short_text.rsplit_once(" · ").map_or(short_text.as_str(), |(kept, _)| kept).to_string(), short.spans.first().map_or_else(Style::new, |span| span.style)));
    let combos = [(&full, true), (&short, true), (&full, false), (&short, false)];
    let with_notice: Vec<((&Line, bool), Vec<Span>)> = combos[..2].iter().flat_map(|combo| notices.iter().map(move |notice| (*combo, notice.clone())))
        .chain(notices.last().map(|notice| ((&shortest, true), notice.clone())))
        .chain(combos[2..].iter().flat_map(|combo| notices.iter().map(move |notice| (*combo, notice.clone())))).collect();
    let tries = with_notice.into_iter().chain(combos.iter().map(|combo| (*combo, Vec::new())));
    for ((left, wide), notice) in tries {
        let spans = right(wide);
        let used = left_width(left) + width_of(&notice) + width_of(&spans);
        if used + 2 <= width || spans.is_empty() && used <= width {
            let mut line: Vec<Span> = left.spans.clone();
            if let Some(last) = line.last_mut() {
                last.content = last.content.trim_end().to_string().into();
            }
            line.extend(notice);
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
            spans.push(Span::styled(format!(" · {} opens", keys::key(Act::Messages)), style(MUTED)));
            (vec![fit(spans, width)], 0)
        }
        Fold::Open => {
            let messages_key = keys::key(Act::Messages);
            let keys = if app.in_messages { format!("  ↑ ↓ scroll · ⏎ read · esc back · {messages_key} folds") } else { format!("  {messages_key} selects") };
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
        let _ = crate::state::write(file, format!("{}\t{seen}\n", fold.name()));
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

fn confirm_lines(name: &str, about: Option<&str>, question: &str, nudged: bool) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(name.to_string(), style(TEXT)))];
    lines.extend(about.map(|about| Line::from(Span::styled(about.to_string(), style(MUTED)))));
    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled(question.to_string(), style(YELLOW)), Span::styled("  y / n", style(MUTED))]));
    if nudged {
        lines.push(Line::from(Span::styled("press y or n", style(PEACH))));
    }
    lines
}

fn menu_group(act: Option<Act>) -> &'static str {
    match act {
        Some(Act::Inbox | Act::Go | Act::Chat | Act::Review | Act::Card) => "SESSION",
        Some(Act::Server | Act::Worker) => "DEV",
        Some(Act::Reset | Act::Drop) => "END IT",
        _ => "BRANCH",
    }
}

// The second value is each choice's line, which the mouse hits are placed on.
fn menu_lines(choices: &[Choice], index: usize, width: usize, running: impl Fn(Option<Act>) -> bool) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut at = Vec::new();
    let mut group = "";
    for (position, choice) in choices.iter().enumerate() {
        if menu_group(choice.act) != group {
            group = menu_group(choice.act);
            lines.push(Line::from(Span::styled(group, style(MUTED))));
        }
        let active = position == index;
        let ending = matches!(choice.act, Some(Act::Reset | Act::Drop));
        let label_style = match (active, ending) {
            (true, true) => bold(RED),
            (true, false) => bold(MAUVE),
            (false, true) => style(RED),
            (false, false) => style(TEXT),
        };
        let dot = if running(choice.act) { Span::styled("● ", style(GREEN)) } else { Span::raw("  ") };
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
    keys::map().help(&[Ctx::Work, Ctx::Backlog, Ctx::Card]).into_iter().map(|(key, text)| Line::from(vec![Span::styled(format!("{key:<9}"), style(MAUVE)), Span::styled(text.to_string(), style(TEXT))])).collect()
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

fn draw_card(frame: &mut Frame, app: &mut App) {
    let Mode::Card { path, name, scroll } = &app.mode else { return };
    let (path, name, scroll) = (path.clone(), name.clone(), *scroll);
    let Some(detail) = app.details.get(&path).cloned() else { return };
    let Some((card, current)) = &detail.card else { return };
    let full = frame.area();
    let area = centered(full, (full.width * 92 / 100).max(40), (full.height * 84 / 100).max(12));
    frame.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {name} · card "), bold(MAUVE))).style(Style::new().bg(MANTLE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [body, foot] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let sha: String = card.sha.chars().take(7).collect();
    let head = if *current { Span::styled(format!("✓ current at {sha}"), style(GREEN)) } else { Span::styled(format!("○ written at {sha}, code changed since"), style(YELLOW)) };
    let mut lines = vec![Line::from(head), Line::default()];
    let mut images = Vec::new();
    let mut links = Vec::new();
    let mut image_lines = Vec::new();
    let viewer = app.viewer.is_some();
    let area = Size::new(body.width, body.height.saturating_sub(2).max(1));
    for text in card.text.lines() {
        match card_image_in(text) {
            Some((alt, image)) => {
                image_lines.push(lines.len());
                lines.push(Line::from(vec![Span::styled("▣ ", style(TEAL)), Span::styled(image.to_string(), style(TEXT)), Span::styled(format!("  {alt}"), style(MUTED))]));
                let file = path.join(image);
                links.extend(file_link(image, &file));
                // An image taller than the rows left under the text is never drawn, so it is sized to fit below what precedes it.
                let left = body.height.saturating_sub(lines.len() as u16 + 1).max(6);
                let area = Size::new(area.width, area.height.min(left));
                let rows = app.image_size(&file, area).map(|size| size.height);
                if let Some(rows) = rows {
                    images.push(ImageAt { line: lines.len(), path: file, rows, area });
                    lines.extend((0..rows).map(|_| Line::default()));
                }
                if image_shown(app.protocol(), rows.is_some()) == Shown::Preview {
                    lines.push(Line::from(Span::styled(preview_note(viewer), style(MUTED))));
                }
            }
            None if text.trim().is_empty() => lines.push(Line::default()),
            None => lines.extend(wrap(text, body.width as usize, usize::MAX).into_iter().map(|part| Line::from(Span::styled(part, style(TEXT))))),
        }
    }
    let scroll = scroll.min(lines.len().saturating_sub(body.height as usize) as u16);
    if let Mode::Card { scroll: kept, .. } = &mut app.mode {
        *kept = scroll;
    }
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
    for image in images {
        let Some(top) = (image.line as u16).checked_sub(scroll) else { continue };
        if top + image.rows > body.height {
            continue;
        }
        if let Some(Some(protocol)) = app.images.get(&(image.path, image.area.width, image.area.height)) {
            let size = protocol.size();
            frame.render_widget(Image::new(protocol), Rect::new(body.x, body.y + top, size.width.min(body.width), size.height));
        }
    }
    let keys = card_keys(!image_lines.is_empty() && app.protocol().is_some(), !image_lines.is_empty() && viewer);
    app.card_image_lines = image_lines;
    app.links.extend(links);
    frame.render_widget(Paragraph::new(Span::styled(keys, style(MUTED))), foot);
}

fn card_keys(fullscreen: bool, open: bool) -> String {
    let map = keys::map();
    let mut keys = vec!["↑ ↓ scroll".to_string()];
    if fullscreen {
        keys.push(format!("{} fullscreen", map.label(Act::Fullscreen)));
    }
    if open {
        keys.push(format!("{} open", map.label(Act::OpenImage)));
    }
    keys.push(format!("{} push", map.label(Act::Push)));
    keys.push(format!("{} close", map.label(Act::Back)));
    keys.join(" · ")
}

fn draw_fullscreen(frame: &mut Frame, app: &mut App) {
    let Mode::Fullscreen { path, index, .. } = &app.mode else { return };
    let files = app.card_images(path);
    let index = *index;
    let Some(file) = files.get(index).cloned() else { return };
    let full = frame.area();
    frame.render_widget(Clear, full);
    frame.render_widget(Block::new().style(Style::new().bg(MANTLE)), full);
    let [body, foot] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(full);
    let shown = file.display().to_string();
    match app.image_size(&file, body.as_size()) {
        Some(size) => {
            if let Some(Some(protocol)) = app.images.get(&(file.clone(), body.width, body.height)) {
                let x = body.x + body.width.saturating_sub(size.width) / 2;
                let y = body.y + body.height.saturating_sub(size.height) / 2;
                frame.render_widget(Image::new(protocol), Rect::new(x, y, size.width.min(body.width), size.height.min(body.height)));
            }
        }
        None => frame.render_widget(Paragraph::new(Line::from(vec![Span::styled("▣ ", style(TEAL)), Span::styled(shown.clone(), style(TEXT))])), body),
    }
    let map = keys::map();
    let mut keys = Vec::new();
    if image_shown(app.protocol(), app.images.get(&(file.clone(), body.width, body.height)).is_some_and(Option::is_some)) == Shown::Preview {
        keys.push("preview".to_string());
    }
    keys.push(format!("{}/{} {shown}", index + 1, files.len()));
    if files.len() > 1 {
        keys.push(format!("{} images", map.label(Act::Switch)));
    }
    if app.viewer.is_some() {
        keys.push(format!("{} open", map.label(Act::OpenImage)));
    }
    keys.push(format!("{} back", map.label(Act::Back)));
    frame.render_widget(Paragraph::new(fit(vec![Span::styled(keys.join(" · "), style(MUTED))], foot.width as usize)), foot);
    app.links.extend(file_link(&shown, &file));
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
        Page::Work => hint_lines(app.selected_has_card()),
        Page::Areas => area_page::Page::hint_lines(),
    };
    let updates: &[(String, String)] = if app.page == Page::Work { &app.updates } else { &[] };
    let status = match app.notice.as_ref().filter(|(_, at)| at.elapsed() < Duration::from_secs(3)) {
        Some((text, _)) => Line::from(Span::styled(text.clone(), style(PEACH))),
        None => status_line(full_hints, short_hints, updates, app.usage.as_ref(), bottom.width as usize, messages::now()),
    };
    if app.page == Page::Areas {
        frame.render_widget(Paragraph::new(status), bottom);
        app.areas.draw(frame, body);
        app.hits = if app.areas.is_normal() { tab_hits } else { Vec::new() };
        return;
    }
    let mut tab_hits = tab_hits;
    tab_hits.extend(hint_hits(&status, bottom, app.selected_has_card()));
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
            let running = |act: Option<Act>| item.as_ref().is_some_and(|item| (act == Some(Act::Server) && item.server) || (act == Some(Act::Worker) && item.worker));
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
        Mode::Card { .. } => draw_card(frame, app),
        Mode::Fullscreen { .. } => draw_fullscreen(frame, app),
        Mode::Help => {
            modal(frame, "keys".to_string(), help_lines(), 72);
        }
        Mode::Message(text) => {
            let mut lines: Vec<Line> = text.lines().map(|line| Line::from(Span::styled(line.to_string(), style(TEXT)))).collect();
            lines.push(Line::from(Span::styled("any key to close", style(MUTED))));
            modal(frame, "kelpie".to_string(), lines, 96);
        }
        Mode::ConfirmReset { name, nudged, .. } => {
            modal(frame, "reset".to_string(), confirm_lines(name, Some("Its changes and commits go, its Claude stops, its worktree is freed and its Space closes. The backlog item it came from returns."), "Reset this task?", *nudged), 72);
        }
        Mode::ConfirmDone { name, nudged, .. } => {
            modal(frame, "done".to_string(), confirm_lines(name, Some("Its commits are on the default branch already: its worktree is freed and its Space closes."), "Close this task as done?", *nudged), 72);
        }
        Mode::ConfirmDrop { name, force, nudged, .. } => {
            let question = if *force { "Discard its changes and commits?" } else { "Drop this task?" };
            modal(frame, "drop".to_string(), confirm_lines(name, None, question, *nudged), 72);
        }
        Mode::Filter { buffer } => {
            let column = if app.column == Column::Work { "work" } else { "backlog" };
            modal(frame, format!("filter {column}"), vec![
                Line::from(vec![Span::styled(buffer.clone(), style(TEXT)), Span::styled("▏", style(MAUVE))]),
                Line::from(Span::styled("⏎ keep the filter · esc clear it", style(MUTED))),
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

const RESIZE_SETTLE: Duration = Duration::from_secs(1);

// A new terminal size must hold for a second before the board lays out for it, so dragging a pane's edge redraws once, at the end.
#[derive(Default)]
struct Settle {
    drawn: Option<(u16, u16)>,
    pending: Option<((u16, u16), Instant)>,
}

impl Settle {
    fn draw_now(&mut self, size: (u16, u16), now: Instant) -> bool {
        match (self.drawn, self.pending) {
            (None, _) => {
                self.drawn = Some(size);
                true
            }
            (Some(drawn), _) if drawn == size => {
                self.pending = None;
                true
            }
            (Some(_), Some((pending, since))) if pending == size => {
                let settled = now.duration_since(since) >= RESIZE_SETTLE;
                if settled {
                    (self.drawn, self.pending) = (Some(size), None);
                }
                settled
            }
            (Some(_), _) => {
                self.pending = Some((size, now));
                false
            }
        }
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
        let path = peers::own_exe().ok();
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
            graphics: Graphics::Off, images: HashMap::new(), cell_guessed: false, viewer: None, card_image_lines: Vec::new(), decoded: HashMap::new(), image_requests: None, image_results: None, plans: HashMap::new(),
            updates: Vec::new(), updater: std::sync::Arc::new(|| update::run(&crate::doctor::Env::current().claude_dir, false, &crate::release::Request::Channel)), update_running: false, update_shown: false, recheck: None, stale: Vec::new(),
            filter: None, ctrl_c: None, notice: None,
        }
    }
}

// The query waits up to 2 s for an answer, so it never runs inside herdr and elsewhere only once an image is first wanted.
fn asks_terminal(forced: Option<&str>, tty: bool, herdr: bool) -> bool {
    tty && !herdr && !matches!(forced, Some("off" | "halfblocks"))
}

struct GraphicsEnv {
    forced: Option<String>,
    tty: bool,
    herdr: bool,
}

impl GraphicsEnv {
    fn current() -> Self {
        GraphicsEnv {
            forced: std::env::var("KELPIE_IMAGES").ok(),
            tty: std::io::IsTerminal::is_terminal(&std::io::stdin()),
            herdr: std::env::var("TERM_PROGRAM").is_ok_and(|program| program == "herdr"),
        }
    }

    fn startup(&self) -> (Graphics, bool) {
        if asks_terminal(self.forced.as_deref(), self.tty, self.herdr) { (Graphics::Pending { wanted: false }, false) } else { self.resolve(None) }
    }

    fn resolve(&self, asked: Option<Picker>) -> (Graphics, bool) {
        resolve_graphics(self.forced.as_deref(), self.tty, self.herdr, asked)
    }
}

fn resolve_graphics(forced: Option<&str>, tty: bool, herdr: bool, asked: Option<Picker>) -> (Graphics, bool) {
    let Some(protocol) = choose_protocol(forced, tty, herdr, asked.as_ref().map(Picker::protocol_type)) else { return (Graphics::Off, false) };
    let answered = asked.as_ref().and_then(|picker| {
        let reported = picker.capabilities().iter().find_map(|capability| match capability { Capability::CellSize(Some(cell)) => Some(*cell), _ => None });
        reported.or_else(|| (picker.protocol_type() != ProtocolType::Halfblocks).then(|| (picker.font_size().width, picker.font_size().height)))
    });
    let cell = cell_size(answered, window_pixels());
    (Graphics::Ready(picker_for(protocol, cell.unwrap_or(GUESSED_CELL))), cell.is_none() && protocol != ProtocolType::Halfblocks)
}

pub fn run(overlay: bool) -> Result<String> {
    let sources = config::sources()?;
    let (snapshots, refresh) = spawn_refresher();
    let mut app = App::new(sources, backlog::load(), overlay, refresh);
    app.away = permit::away_since().is_some();
    let (update_notices, recheck) = spawn_update_checker();
    app.recheck = Some(recheck);
    let restarter = (!overlay).then(spawn_restarter);
    let (requests, results) = spawn_detailer();
    app.detail_requests = Some(requests);
    app.detail_results = Some(results);
    (app.image_requests, app.image_results) = spawn_decoder().map_or((None, None), |(requests, results)| (Some(requests), Some(results)));
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
    let mut resize = Settle::default();
    let mut terminal = init_terminal();
    let graphics = GraphicsEnv::current();
    (app.graphics, app.cell_guessed) = graphics.startup();
    app.viewer = image_viewer(std::env::consts::OS, &|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    let _ = terminal.clear();
    let result = (|| -> Result<()> {
        while !app.quit {
            if let Mode::EditMessage(editor) = &mut app.mode {
                editor.poll_draft();
            }
            app.poll_answer();
            app.take_working_result();
            if !matches!(app.mode, Mode::Message(_)) {
                app.update_shown = false;
            }
            while let Ok(updates) = update_notices.try_recv() {
                app.updates = updates;
            }
            while let Some(stale) = restarter.as_ref().and_then(|restarter| restarter.try_recv().ok()) {
                app.stale = stale;
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
            app.take_images();
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
            if (matches!(app.mode, Mode::Normal | Mode::Help) || matches!(app.mode, Mode::Message(_)) && !app.update_shown) && app.areas.is_idle() && rebuild.ready() {
                let error = rebuild.restart(&app);
                terminal = init_terminal();
                app.mode = Mode::Message(error);
            }
            if matches!(app.graphics, Graphics::Pending { wanted: true }) {
                (app.graphics, app.cell_guessed) = graphics.resolve(Picker::from_query_stdio().ok());
                app.images.clear();
            }
            let size = ratatui::crossterm::terminal::size().unwrap_or_default();
            if resize.draw_now(size, Instant::now()) {
                terminal.draw(|frame| draw(frame, &mut app)).map_err(|error| error.to_string())?;
            }
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

    fn press(app: &mut App, event: KeyEvent) {
        app.handle_key(event, None);
    }

    #[test]
    fn every_key_in_the_help_the_hints_and_the_menus_is_one_the_table_acts_on_and_every_row_shows_in_help() {
        let map = keys::map();
        let help: Vec<String> = help_lines().iter().map(|line| line.spans[0].content.trim().to_string()).collect();
        for row in keys::TABLE.iter().filter(|row| row.on.contains(&Ctx::Work) || row.on.contains(&Ctx::Backlog)) {
            assert!(help.contains(&map.label(row.act)), "{} is in the table but not in ? help: {help:?}", row.act.id());
        }
        for label in &help {
            assert!(keys::TABLE.iter().any(|row| map.label(row.act) == *label), "{label} is in ? help but not in the table");
        }
        let (full, short) = map.hints(Ctx::Work);
        for (key, _, code) in full.iter().chain(&short) {
            assert!(map.act(Ctx::Work, &KeyEvent::from(*code)).is_some(), "the hint {key} is not a key the board acts on");
        }
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
        item.server = true;
        let menus = [task_choices(Some(&item), Some("bin/dev"), Some("bin/jobs")), backlog_choices(&[Source::Local]), ongoing_choices(&Ongoing { key: String::new(), title: String::new(), area: String::new(), task: None })];
        for choice in menus.iter().flatten() {
            let act = choice.act.expect("a built-in choice names its action");
            assert_eq!(choice.key, map.char(act), "{}'s letter is the table's", choice.label);
            assert!(keys::TABLE.iter().any(|row| row.act == act), "{} is offered but not in the table", choice.label);
        }
    }

    #[test]
    fn esc_and_q_step_back_and_close_only_an_overlay_at_the_top() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.mode = Mode::Help;
        app.handle_normal(key(KeyCode::Esc));
        app.handle_normal(key(KeyCode::Esc));
        assert!(!app.quit, "esc on the full board at the top does nothing");
        app.handle_normal(key(KeyCode::Char('q')));
        assert!(!app.quit && app.notice.as_ref().is_some_and(|(text, _)| text == "ctrl+c twice to quit"), "{:?}", app.notice);
        app.overlay = true;
        app.handle_normal(key(KeyCode::Char('q')));
        assert!(app.quit, "q at the top closes an overlay");
        app.quit = false;
        app.filter = Some((Column::Work, "zzz".to_string()));
        app.handle_normal(key(KeyCode::Esc));
        assert!(app.filter.is_none() && !app.quit, "esc clears the filter first");
        app.handle_normal(key(KeyCode::Esc));
        assert!(app.quit, "then closes the overlay");
    }

    #[test]
    fn ctrl_c_quits_only_when_pressed_twice_within_two_seconds() {
        let mut app = test_app(Vec::new());
        let control_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        press(&mut app, control_c);
        assert!(!app.quit && app.notice.as_ref().is_some_and(|(text, _)| text == "ctrl+c again to quit"));
        app.ctrl_c = Some(Instant::now() - Duration::from_secs(3));
        press(&mut app, control_c);
        assert!(!app.quit, "a press three seconds later starts again");
        press(&mut app, control_c);
        assert!(app.quit);
    }

    #[test]
    fn done_asks_first_and_a_confirmation_takes_only_y_n_or_esc() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        item.path = Some(PathBuf::from("/nowhere/wt1.app"));
        let mut app = test_app(vec![item]);
        app.handle_normal(key(KeyCode::Char('d')));
        assert!(matches!(app.mode, Mode::ConfirmDone { nudged: false, .. }), "d asks before it closes the task");
        press(&mut app, key(KeyCode::Char('k')));
        assert!(matches!(app.mode, Mode::ConfirmDone { nudged: true, .. }), "another key keeps the question");
        let (rows, _) = render(&mut app, 100, 30);
        assert!(rows.join("\n").contains("press y or n"));
        press(&mut app, key(KeyCode::Char('n')));
        assert!(matches!(app.mode, Mode::Normal));
        app.handle_normal(key(KeyCode::Char('x')));
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal), "esc cancels a drop");
        app.handle_normal(key(KeyCode::Char('z')));
        press(&mut app, key(KeyCode::Char('q')));
        assert!(matches!(app.mode, Mode::Normal), "q closes a question like esc");
    }

    #[test]
    fn slash_filters_the_selected_column_by_a_plain_substring_and_esc_clears_it() {
        let named = |name: &str| { let mut item = crate::status::tests::task(Some("idle"), None, 0); item.name = name.to_string(); item };
        let mut app = test_app(vec![named("billing"), named("search-filters"), named("refunds")]);
        app.handle_normal(key(KeyCode::Char('/')));
        for character in "FIL".chars() {
            press(&mut app, key(KeyCode::Char(character)));
        }
        press(&mut app, key(KeyCode::Enter));
        assert_eq!(app.selected_task().map(|item| item.name).as_deref(), Some("search-filters"));
        let (rows, _) = render(&mut app, 112, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("search-filters") && !screen.contains("billing") && !screen.contains("refunds") && screen.contains("1 task / FIL"), "{screen}");
        app.handle_normal(key(KeyCode::Down));
        assert_eq!(app.selected_task().map(|item| item.name).as_deref(), Some("search-filters"), "moving stays on the rows shown");
        app.handle_normal(key(KeyCode::Esc));
        assert!(app.filter.is_none());
        let (rows, _) = render(&mut app, 112, 40);
        assert!(rows.join("\n").contains("billing"));
    }

    #[test]
    fn the_task_menu_and_the_letters_use_the_new_map() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        item.path = Some(PathBuf::from("/nowhere/wt1.app"));
        let mut app = test_app(vec![item]);
        app.handle_normal(key(KeyCode::Char('c')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::Message(..), .. }), "c messages the task's Claude");
        app.mode = Mode::Normal;
        app.handle_normal(key(KeyCode::Char('n')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::TaskName, .. }), "n starts a new task");
        app.mode = Mode::Normal;
        app.handle_normal(key(KeyCode::Char('s')));
        assert!(matches!(&app.mode, Mode::Message(text) if text.contains("no sign-off card")), "s is the card, not a new task");
        app.mode = Mode::Normal;
        app.column = Column::Backlog;
        app.handle_normal(key(KeyCode::Char('n')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogItem, .. }), "n in the Backlog column adds an item");
        app.mode = Mode::Normal;
        app.handle_normal(key(KeyCode::Char('i')));
        assert!(matches!(&app.mode, Mode::Message(text) if text.contains("No questions")), "i is the inbox");
    }

    #[test]
    fn opening_review_focuses_the_split_already_open_in_the_space() {
        let pane = |argv: &[&str]| serde_json::json!({ "foreground_processes": [{ "pid": 7, "argv": argv }] });
        let panes = vec![
            ("p_claude".to_string(), pane(&["claude", "--resume", "x"])),
            ("p_server".to_string(), pane(&["sh", "-c", "bin/dev"])),
            ("p_review".to_string(), pane(&["sh", "/k/scripts/review.sh", "list", "/state/reviews/p_claude"])),
        ];
        assert_eq!(review_pane_in(&panes).as_deref(), Some("p_review"));
        assert_eq!(review_pane_in(&panes[..2]), None, "without a review split one is made");
        assert!(!runs_review(&pane(&["sh", "/k/scripts/review.sh", "split"])), "the split command itself is not the review");
        let source = include_str!("board.rs");
        let open = &source[source.find("    fn open_review(").unwrap()..];
        let open = &open[..open.find("\n    }\n").unwrap()];
        assert!(open.find("open_review_pane(workspace)").unwrap() < open.find("arg(\"split\")").unwrap(), "an open review is looked for before a split is made");
    }

    fn carded(current: bool) -> App {
        let mut app = test_app(vec![served("search-filters")]);
        with_detail(&mut app);
        let text = "Filters show on the search page.\nPrice and size, both kept in the URL.\nA third line stays in the window.\n![the filters](shots/filters.png)\n![empty](shots/empty.png)".to_string();
        let card = report::Card { sha: "abcdef0123".to_string(), text, file: PathBuf::from("/state/cards/x.md") };
        std::rc::Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card = Some((card, current));
        app
    }

    #[test]
    fn a_narrow_row_shows_the_cards_first_lines_and_counts_its_images() {
        let mut app = carded(true);
        let (rows, _) = render(&mut app, 84, 50);
        let screen = rows.join("\n");
        assert!(screen.contains("card    ✓ Filters show on the search page.") && screen.contains("Price and size, both kept in the URL."), "{screen}");
        assert!(!screen.contains("A third line"), "two lines of text under the row: {screen}");
        assert!(screen.contains("▣ 2 images · s shows the card") && screen.contains("⏎ options · s card"), "{screen}");
        assert!(rows[48].contains("s card"), "the hint line names s while the task has a card: {}", rows[48]);
        let mut bare = test_app(vec![served("search-filters")]);
        with_detail(&mut bare);
        let (rows, _) = render(&mut bare, 84, 50);
        assert!(!rows.join("\n").contains("card "), "no card, no card line");
        bare.handle_normal(key(KeyCode::Char('s')));
        assert!(matches!(&bare.mode, Mode::Message(text) if text.contains("has no sign-off card yet")));
    }

    #[test]
    fn s_opens_the_card_window_with_its_text_images_and_whether_it_is_current() {
        let mut app = carded(false);
        assert!(app.choices().iter().any(|choice| choice.act == Some(Act::Card) && choice.label == "Show the card"));
        app.handle_normal(key(KeyCode::Char('s')));
        assert!(matches!(app.mode, Mode::Card { .. }));
        let (rows, _) = render(&mut app, 170, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("search-filters · card") && screen.contains("○ written at abcdef0, code changed since"), "{screen}");
        assert!(screen.contains("A third line stays in the window.") && screen.contains("▣ shots/filters.png  the filters"), "with graphics off an image is its line: {screen}");
        assert!(screen.contains("↑ ↓ scroll · p push · esc close"), "{screen}");
        press(&mut app, key(KeyCode::Down));
        assert!(matches!(app.mode, Mode::Card { scroll: 1, .. }));
        press(&mut app, key(KeyCode::Char('q')));
        assert!(matches!(app.mode, Mode::Normal), "q closes the window");
        let mut fresh = carded(true);
        fresh.handle_normal(key(KeyCode::Char('s')));
        let (rows, _) = render(&mut fresh, 84, 40);
        assert!(rows.join("\n").contains("✓ current at abcdef0"), "the window shows in a narrow board too");
        press(&mut fresh, key(KeyCode::Char('p')));
        assert!(!matches!(fresh.mode, Mode::Card { .. }), "p goes on to push");
    }

    #[test]
    fn a_tall_image_under_the_cards_text_still_draws_in_a_short_window() {
        let dir = crate::sh::test_dir("tall-card");
        let png = dir.join("tall.png");
        image::RgbImage::from_pixel(40, 120, image::Rgb([200, 30, 30])).save(&png).unwrap();
        let mut app = carded(true);
        let text = format!("One.\nTwo.\nThree.\nFour.\n![tall]({})", png.display());
        std::rc::Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card.as_mut().unwrap().0.text = text;
        app.graphics = Graphics::Ready(Picker::halfblocks());
        (app.image_requests, app.image_results) = spawn_decoder().map(|(requests, results)| (Some(requests), Some(results))).unwrap();
        app.handle_normal(key(KeyCode::Char('s')));
        render(&mut app, 120, 30);
        let waited = Instant::now();
        while app.decoded.get(&png).is_none_or(Option::is_none) && waited.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
            app.take_images();
        }
        let (rows, buffer) = render(&mut app, 120, 30);
        let red = Color::Rgb(200, 30, 30);
        assert!((0..30u16).any(|y| (0..120u16).any(|x| buffer[(x, y)].bg == red || buffer[(x, y)].fg == red)), "the image is drawn without scrolling: {}", rows.join("\n"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_changed_list_keeps_a_modified_unstaged_first_file_whole() {
        assert_eq!(changed_files(" M src/search/filters.txt\0M  b.rs\0R  new.rs\0old.rs\0?? notes/a b.md\0"), ["src/search/filters.txt", "b.rs", "new.rs", "notes/a b.md"]);
        let dir = crate::sh::test_dir("changed");
        repo::git(&dir, &["init", "--quiet"]).unwrap();
        std::fs::create_dir_all(dir.join("src/search")).unwrap();
        std::fs::write(dir.join("src/search/filters.txt"), "one").unwrap();
        repo::git(&dir, &["add", "."]).unwrap();
        repo::git(&dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "one"]).unwrap();
        std::fs::write(dir.join("src/search/filters.txt"), "two").unwrap();
        assert_eq!(gather_detail(&dir, None).changed, ["src/search/filters.txt"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_terminal_size_is_drawn_only_once_it_held_for_a_second() {
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        let mut resize = Settle::default();
        assert!(resize.draw_now((100, 40), at(0)), "the first frame draws at once");
        assert!(resize.draw_now((100, 40), at(100)));
        assert!(!resize.draw_now((90, 40), at(200)), "a new size waits");
        assert!(!resize.draw_now((80, 40), at(700)), "a size still moving starts the wait again");
        assert!(!resize.draw_now((80, 40), at(1600)));
        assert!(resize.draw_now((80, 40), at(1700)), "a second at the same size draws it");
        assert!(resize.draw_now((80, 40), at(1800)));
        assert!(!resize.draw_now((120, 50), at(1900)));
        assert!(resize.draw_now((80, 40), at(2000)), "back to the drawn size before it settled draws at once, nothing moved");
        assert!(!resize.draw_now((120, 50), at(2100)), "and the next change waits its own second");
    }

    #[test]
    fn the_backlog_column_adds_an_item_only_with_the_local_source() {
        let keys = |sources: &[Source]| backlog_choices(sources).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(&[Source::Local]), "gRn");
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
        assert_eq!(keys(&crate::status::tests::task(Some("working"), None, 1)), "gcrazx");
        assert_eq!(keys(&crate::status::tests::task(Some("idle"), Some("ready"), 1)), "gcrvtpdazx");
        assert_eq!(keys(&crate::status::tests::task(Some("idle"), Some("ready"), 0)), "gcrtpdazx", "no commits, nothing to review");
    }

    #[test]
    fn a_task_in_review_offers_no_push_until_its_reviewer_passes_it() {
        let keys = |item: &Item| task_choices(Some(item), None, None).iter().map(|choice| choice.key).collect::<String>();
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        item.verdict = Some(crate::status::tests::in_review(false, 2, false, false));
        assert_eq!(keys(&item), "gcrvtdazx");
        assert_eq!(row_facts(&item), "1 commit · review: 2 findings");
        let withdraw = task_choices(Some(&item), None, None).into_iter().find(|choice| choice.key == 'v').unwrap();
        assert_eq!(withdraw.label, "Withdraw from review", "the push stays the user's: a task in review can always be taken out");
        item.verdict = None;
        assert_eq!(keys(&item), "gcrvtpdazx", "withdrawn, it offers push again");
        assert_eq!(task_choices(Some(&item), None, None).into_iter().find(|choice| choice.key == 'v').unwrap().label, "Send to review");
        item.verdict = Some(crate::status::tests::in_review(false, 2, false, false));
        item.verdict = Some(crate::status::tests::in_review(true, 0, false, false));
        assert_eq!(keys(&item), "gcrvtpdazx");
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
        assert!(task_choices(Some(&unchecked), None, None).iter().any(|choice| choice.key == 'p'));
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
        assert_eq!(keys(None), "gcrtpdzx");
        assert_eq!(keys(Some("bin/dev")), "gcrwtpdzx");
        let start = task_choices(None, Some("npm run dev"), None);
        assert_eq!((start[3].label.as_str(), start[3].about.as_str()), ("Start the dev server", "npm run dev in a tab of its Space"));
    }

    #[test]
    fn b_is_offered_only_with_a_dev_worker() {
        let keys = |dev_worker: Option<&str>| task_choices(None, Some("bin/dev"), dev_worker).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(None), "gcrwtpdzx");
        assert_eq!(keys(Some("bin/jobs")), "gcrwbtpdzx");
        assert_eq!(task_choices(None, None, Some("bin/jobs")).iter().map(|choice| choice.key).collect::<String>(), "gcrbtpdzx", "a worker needs no dev server");
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
        assert_eq!(server_line(&item, true, "server", 60).unwrap().0.to_string(), "server  ○ off · w starts it", "an address from config is not a running server");
        assert!(server_line(&item, false, "server", 60).is_none(), "no dev_server, no line");
        assert_eq!(server_line(&item, true, "web", 60).unwrap().0.to_string(), "web     ○ off · w starts it", "the label is worktree.server_name");
        item.server = true;
        let (line, link) = server_line(&item, true, "server", 60).unwrap();
        assert_eq!(line.to_string(), "server  ● running  https://wt1.app.dev");
        let link = link.unwrap();
        assert_eq!((link.shown.as_str(), link.url.as_str()), ("https://wt1.app.dev", "https://wt1.app.dev"));
        item.url = None;
        assert!(server_line(&item, true, "server", 60).unwrap().1.is_none(), "dev_url off, or no port for {{port}}");
        item.url = Some("https://a-very-long-worktree-name.example.dev/path".to_string());
        let (line, link) = server_line(&item, true, "server", 40).unwrap();
        assert!(line.width() <= 40, "{}", line.width());
        let link = link.unwrap();
        assert!(link.shown.ends_with('…') && link.url.ends_with("/path"));
    }

    #[test]
    fn the_server_address_is_a_terminal_hyperlink_one_column_per_character() {
        let mut item = crate::status::tests::task(Some("idle"), None, 0);
        item.server = true;
        item.url = Some("http://localhost:3001".to_string());
        let (line, link) = server_line(&item, true, "server", 60).unwrap();
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
        assert_eq!(question_summary(&asked, 60), "Which queue for refund mails? · 3 options · i");
        assert_eq!(question_summary(&asked, 30), "Which queue f… · 3 options · i");
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
            branch: "search-filters".to_string(), changed: vec!["app/filters.rb".to_string()], dev_server: true, dev_worker: true, server_name: "server".to_string(), worker_name: "worker".to_string(),
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
        assert!(screen.contains("asks    Which queue for refund mails? · 3 options · i"), "{screen}");
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
        let (lines, at) = menu_lines(&choices, 7, 48, |act| act == Some(Act::Server) && item.server);
        let text: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        assert_eq!(text.iter().filter(|line| ["SESSION", "DEV", "BRANCH", "END IT"].contains(&line.as_str())).count(), 4, "{text:#?}");
        assert!(text[at[3]].starts_with("  Stop the dev server") && text[at[3]].ends_with("● w"), "{}", text[at[3]]);
        assert!(text[at[7]].starts_with("▸ Push") && text[at[7]].ends_with("  p"), "{}", text[at[7]]);
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
        assert!(bar.trim_start().starts_with("⏎ options · prefix+tab lead · i inbox · m messages · ? keys"), "{bar}");
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
        assert!(rows[28].trim_start().starts_with("⏎ options · i inbox · ? keys") && rows[28].contains("session ━━━━━━━━ 23% resets"), "the herdr popup keeps the bars: {}", rows[28]);
        let (rows, _) = render(&mut app, 80, 30);
        assert!(rows[28].trim_end().ends_with("5h 23% · 7d 71%"), "{}", rows[28]);
        app.usage = None;
        let (rows, _) = render(&mut app, 96, 30);
        assert!(!rows[28].contains('%'));
        assert_eq!(usage_color(49.0), GREEN);
        assert_eq!(usage_color(80.0), RED);
    }

    #[test]
    fn a_newer_version_shows_muted_beside_the_hints_without_pushing_out_the_usage_bars() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        let now = messages::now();
        app.usage = Some(Usage {
            session: Some(usage::Window { used: 23.0, resets_at: now + 600 }), week: Some(usage::Window { used: 71.4, resets_at: now + 3 * 86_400 }), seen_at: now,
        });
        let (rows, _) = render(&mut app, 170, 30);
        assert!(!rows[28].contains('↑'), "nothing newer, no notice: {}", rows[28]);
        app.updates = vec![("kelpie".to_string(), "0.18.2".to_string())];
        let (rows, _) = render(&mut app, 170, 30);
        assert!(rows[28].contains("m messages · ? keys   ↑ kelpie 0.18.2 · U updates") && rows[28].contains("session ━━━━━━━━ 23% resets"), "{}", rows[28]);
        app.updates.push(("vault".to_string(), "0.2.17".to_string()));
        let (rows, _) = render(&mut app, 170, 30);
        assert!(rows[28].contains("m messages · ? keys   ↑ update · U") && rows[28].contains("session ━━━━━━━━ 23% resets"), "the notice shortens before the hints do: {}", rows[28]);
        let (rows, buffer) = render(&mut app, 200, 30);
        assert!(rows[28].contains("↑ kelpie 0.18.2 · vault 0.2.17 · U updates") && rows[28].contains("session ━━━━━━━━ 23% resets"), "{}", rows[28]);
        let (x, _) = find(&rows[28..29], "↑ kelpie")[0];
        assert_eq!(buffer[(x as u16, 28)].fg, MUTED);
        let (ux, _) = find(&rows[28..29], "U updates")[0];
        assert_eq!(app.hit_at(ux as u16, 28), Some(Hit::Key(KeyCode::Char('U'))));
        let (rows, _) = render(&mut app, 112, 30);
        assert!(rows[28].trim_start().starts_with("⏎ options · i inbox   ↑ update · U") && rows[28].contains("session ━━━━━━━━ 23% resets") && rows[28].contains("week ━━━━━━━━ 71% resets"), "the herdr popup keeps the bars, ? keys makes room: {}", rows[28]);
        let (x, _) = find(&rows[28..29], "↑ update · U")[0];
        assert_eq!(app.hit_at(x as u16, 28), Some(Hit::Key(KeyCode::Char('U'))));
        let (rows, _) = render(&mut app, 40, 30);
        assert!(rows[28].trim_start().starts_with("⏎ options · i inbox · ? keys"), "too narrow for it, the hints stay: {}", rows[28]);
        app.page = Page::Areas;
        let (rows, _) = render(&mut app, 170, 30);
        assert!(!rows[28].contains("updates") && !rows[28].contains("↑ update"), "the Work page only: {}", rows[28]);
    }

    #[test]
    fn u_runs_the_update_in_the_background_and_shows_its_result() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        let (release, gate) = mpsc::channel::<()>();
        let gate = std::sync::Mutex::new(gate);
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = ran.clone();
        app.updater = std::sync::Arc::new(move || {
            gate.lock().unwrap().recv().map_err(|error| error.to_string())?;
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok("kelpie 0.18.1 → 0.18.2".to_string())
        });
        let (recheck, rechecked) = mpsc::channel();
        app.recheck = Some(recheck);
        app.updates = vec![("kelpie".to_string(), "0.18.2".to_string())];
        app.handle_normal(key(KeyCode::Char('U')));
        assert!(matches!(app.mode, Mode::Working { .. }), "the key returns while the update is still running");
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
        release.send(()).unwrap();
        for _ in 0..200 {
            app.take_working_result();
            if matches!(app.mode, Mode::Message(_)) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let Mode::Message(text) = &app.mode else { panic!("the result is shown") };
        assert_eq!(text, "kelpie 0.18.1 → 0.18.2");
        assert!(app.update_shown && app.updates.is_empty() && rechecked.try_recv().is_ok(), "the notice goes and a fresh check is asked for");
        assert!(help_lines().iter().any(|line| line.to_string().starts_with("U ")), "U is in the keys help");
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
        for (hint, code) in [("⏎ options", KeyCode::Enter), ("i inbox", KeyCode::Char('i')), ("? keys", KeyCode::Char('?'))] {
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
        (app.image_requests, app.image_results) = spawn_decoder().map(|(requests, results)| (Some(requests), Some(results))).unwrap();
        let (rows, _) = render(&mut app, 170, 50);
        assert!(rows.join("\n").contains(&format!("▣ {}  after", png.display())), "the path shows while the image decodes off the drawing thread");
        let waited = Instant::now();
        while app.decoded.get(&png).is_none_or(Option::is_none) && waited.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
            app.take_images();
        }
        let (rows, buffer) = render(&mut app, 170, 50);
        let (_, y) = find(&rows, "  after")[0];
        let red = Color::Rgb(200, 30, 30);
        assert!((0..170).any(|x| buffer[(x, y as u16 + 1)].bg == red || buffer[(x, y as u16 + 1)].fg == red), "the image under its line: {}", rows.join("\n"));
        assert!(rows.join("\n").contains("▣ /nowhere/before.png  before"), "a missing file stays a path");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_protocol_follows_the_forced_choice_then_the_tty_then_herdr_then_the_answer() {
        use ProtocolType::{Halfblocks, Iterm2, Kitty, Sixel};
        assert_eq!(choose_protocol(Some("off"), true, true, Some(Kitty)), None);
        assert_eq!(choose_protocol(Some("halfblocks"), true, false, Some(Kitty)), Some(Halfblocks));
        assert_eq!(choose_protocol(Some("kitty"), false, false, None), Some(Kitty), "a forced protocol needs no tty");
        assert_eq!(choose_protocol(Some("sixel"), true, false, None), Some(Sixel));
        assert_eq!(choose_protocol(Some("iterm2"), true, false, None), Some(Iterm2));
        assert_eq!(choose_protocol(None, false, true, None), None, "no tty, no picture");
        assert_eq!(choose_protocol(None, true, true, Some(Halfblocks)), Some(Kitty), "herdr draws Kitty placeholders though its query says halfblocks");
        assert_eq!(choose_protocol(None, true, true, None), Some(Kitty), "herdr with no answer at all");
        assert_eq!(choose_protocol(Some("bogus"), true, true, None), Some(Kitty), "an unknown value is detection");
        assert_eq!(choose_protocol(None, true, false, Some(Sixel)), Some(Sixel), "outside herdr the answer decides");
        assert_eq!(choose_protocol(None, true, false, Some(Halfblocks)), Some(Halfblocks));
        assert_eq!(choose_protocol(None, true, false, None), Some(Halfblocks));
    }

    #[test]
    fn the_board_never_asks_the_terminal_inside_herdr_and_elsewhere_only_once_an_image_is_wanted() {
        assert!(!asks_terminal(None, true, true), "herdr: the protocol comes from the environment");
        assert!(!asks_terminal(Some("kitty"), true, true));
        assert!(asks_terminal(None, true, false) && asks_terminal(Some("kitty"), true, false), "elsewhere the answer gives the cell size");
        assert!(!asks_terminal(Some("off"), true, false) && !asks_terminal(Some("halfblocks"), true, false) && !asks_terminal(None, false, false));
        let herdr = GraphicsEnv { forced: None, tty: true, herdr: true }.startup().0;
        assert!(matches!(&herdr, Graphics::Ready(picker) if picker.protocol_type() == ProtocolType::Kitty), "Kitty at startup with no query inside herdr");
        assert!(matches!(GraphicsEnv { forced: None, tty: true, herdr: false }.startup().0, Graphics::Pending { wanted: false }), "the first frame draws before any query");
        let dir = PathBuf::from(format!("/tmp/kq{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])).save(&png).unwrap();
        let mut app = test_app(vec![]);
        app.graphics = Graphics::Pending { wanted: false };
        assert_eq!(app.image_size(&dir.join("missing.png"), Size::new(10, 10)), None);
        assert!(matches!(app.graphics, Graphics::Pending { wanted: false }), "a missing file wants no query");
        assert_eq!(app.image_size(&png, Size::new(10, 10)), None, "the path shows until the query between frames has answered");
        assert!(matches!(app.graphics, Graphics::Pending { wanted: true }));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_cell_size_is_the_answer_else_the_window_pixels_else_unknown() {
        assert_eq!(cell_size(Some((16, 34)), Some((93, 39, 1488, 1326))), Some((16, 34)));
        assert_eq!(cell_size(None, Some((93, 39, 1488, 1326))), Some((16, 34)), "herdr with a client sets the pane's pixels");
        assert_eq!(cell_size(Some((0, 0)), Some((119, 40, 0, 0))), None, "herdr with no client attached knows no pixels");
        assert_eq!(cell_size(None, None), None);
    }

    #[test]
    fn an_image_is_sharp_with_a_protocol_a_preview_with_halfblocks_and_a_path_otherwise() {
        assert_eq!(image_shown(Some(ProtocolType::Kitty), true), Shown::Sharp);
        assert_eq!(image_shown(Some(ProtocolType::Sixel), true), Shown::Sharp);
        assert_eq!(image_shown(Some(ProtocolType::Iterm2), true), Shown::Sharp);
        assert_eq!(image_shown(Some(ProtocolType::Halfblocks), true), Shown::Preview);
        assert_eq!(image_shown(Some(ProtocolType::Kitty), false), Shown::Path, "an image that fails to decode");
        assert_eq!(image_shown(None, true), Shown::Path, "KELPIE_IMAGES=off or no tty");
    }

    #[test]
    fn o_opens_with_the_system_viewer_where_the_board_sits_and_not_over_ssh() {
        let env = |names: &'static [&'static str]| move |name: &str| names.contains(&name);
        assert_eq!(image_viewer("macos", &env(&[])), Some("open"));
        assert_eq!(image_viewer("linux", &env(&[])), None, "no display on Linux");
        assert_eq!(image_viewer("linux", &env(&["DISPLAY"])), Some("xdg-open"));
        assert_eq!(image_viewer("linux", &env(&["WAYLAND_DISPLAY"])), Some("xdg-open"));
        assert_eq!(image_viewer("macos", &env(&["SSH_CONNECTION"])), None);
        assert_eq!(image_viewer("linux", &env(&["DISPLAY", "SSH_TTY"])), None);
        assert_eq!(image_viewer("windows", &env(&[])), None);
        assert!(!openable(Path::new("/nowhere/x.png")) && !openable(Path::new("/bin/sh")));
        assert_eq!(file_url(Path::new("/tmp/a b/ö.png")), "file:///tmp/a%20b/%C3%B6.png");
    }

    fn imaged(dir: &Path) -> (App, PathBuf, PathBuf) {
        std::fs::create_dir_all(dir).unwrap();
        let (wide, tall) = (dir.join("wide.png"), dir.join("tall.png"));
        image::RgbImage::from_pixel(40, 20, image::Rgb([200, 30, 30])).save(&wide).unwrap();
        image::RgbImage::from_pixel(20, 40, image::Rgb([30, 30, 200])).save(&tall).unwrap();
        let mut app = test_app(vec![served("search-filters")]);
        with_detail(&mut app);
        let text = format!("Refunds show.\n![wide]({})\n![tall]({})", wide.display(), tall.display());
        let card = report::Card { sha: "abcdef0123".to_string(), text, file: PathBuf::from("/state/cards/x.md") };
        std::rc::Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card = Some((card, true));
        app.graphics = Graphics::Ready(Picker::halfblocks());
        (app.image_requests, app.image_results) = spawn_decoder().map(|(requests, results)| (Some(requests), Some(results))).unwrap();
        for file in [&wide, &tall] {
            app.image_size(file, Size::new(10, 10));
        }
        let waited = Instant::now();
        while [&wide, &tall].iter().any(|file| app.decoded.get(*file).is_none_or(Option::is_none)) && waited.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
            app.take_images();
        }
        app.handle_normal(key(KeyCode::Char('s')));
        (app, wide, tall)
    }

    fn coloured(cell: &ratatui::buffer::Cell, (r, g, b): (u8, u8, u8)) -> bool {
        let near = |color: Color| matches!(color, Color::Rgb(red, green, blue) if red.abs_diff(r) < 8 && green.abs_diff(g) < 8 && blue.abs_diff(b) < 8);
        near(cell.fg) || near(cell.bg)
    }

    #[test]
    fn a_card_window_image_takes_the_windows_width_and_a_halfblock_one_says_it_is_a_preview() {
        let dir = PathBuf::from(format!("/tmp/kw{}", std::process::id()));
        let (mut app, wide, _) = imaged(&dir);
        let (rows, buffer) = render(&mut app, 120, 60);
        let screen = rows.join("\n");
        let (x, y) = find(&rows, &format!("▣ {}  wide", wide.display()))[0];
        let red = (200, 30, 30);
        let drawn: Vec<u16> = (0..120).filter(|column| coloured(&buffer[(*column, y as u16 + 1)], red)).collect();
        let body = 120 * 92 / 100 - 4;
        assert_eq!((drawn.first().copied(), drawn.len() as u16), (Some(x as u16), body), "a 40×20 image fills the window's {body} columns: {screen}");
        assert!(screen.contains("preview · f larger"), "{screen}");
        assert!(!screen.contains("o opens the image") && !screen.contains("o open"), "no viewer, no o: {screen}");
        assert!(screen.contains("↑ ↓ scroll · f fullscreen · p push · esc close"), "{screen}");
        assert!(buffer[(x as u16 + 2, y as u16)].symbol().starts_with(&format!("\x1b]8;;file://{}", wide.display())), "the path is a file link");
        app.viewer = Some("true");
        let (rows, _) = render(&mut app, 120, 60);
        let screen = rows.join("\n");
        assert!(screen.contains("preview · f larger · o opens the image") && screen.contains("↑ ↓ scroll · f fullscreen · o open · p push · esc close"), "{screen}");
        press(&mut app, key(KeyCode::Char('o')));
        assert!(matches!(app.mode, Mode::Card { .. }) && app.notice.is_none(), "o hands the image to the viewer and stays in the window");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn f_shows_the_image_alone_over_the_board_and_arrows_go_between_images() {
        let dir = PathBuf::from(format!("/tmp/kf{}", std::process::id()));
        let (mut app, wide, tall) = imaged(&dir);
        render(&mut app, 120, 60);
        press(&mut app, key(KeyCode::Char('f')));
        assert!(matches!(app.mode, Mode::Fullscreen { index: 0, .. }));
        let (rows, buffer) = render(&mut app, 120, 60);
        let screen = rows.join("\n");
        let red: Vec<(u16, u16)> = (0..60).flat_map(|y| (0..120).map(move |x| (x, y))).filter(|at| coloured(&buffer[*at], (200, 30, 30))).collect();
        let columns = red.iter().map(|(x, _)| *x).max().unwrap() - red.iter().map(|(x, _)| *x).min().unwrap() + 1;
        assert_eq!(columns, 120, "the wide image spans the whole board: {screen}");
        assert!(screen.contains(&format!("preview · 1/2 {} · ← → images · esc back", wide.display())), "{screen}");
        assert!(!screen.contains("Refunds show."), "the image is alone: {screen}");
        press(&mut app, key(KeyCode::Right));
        let (rows, buffer) = render(&mut app, 120, 60);
        assert!(rows.join("\n").contains(&format!("2/2 {}", tall.display())));
        let blue: Vec<u16> = (0..60).filter(|y| (0..120).any(|x| coloured(&buffer[(x, *y)], (30, 30, 200)))).collect();
        assert_eq!(blue.len(), 59, "the tall image takes the height above the key line");
        press(&mut app, key(KeyCode::Right));
        assert!(matches!(app.mode, Mode::Fullscreen { index: 0, .. }), "→ wraps around");
        press(&mut app, key(KeyCode::Left));
        assert!(matches!(app.mode, Mode::Fullscreen { index: 1, .. }));
        for back in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('f')] {
            press(&mut app, key(back));
            assert!(matches!(app.mode, Mode::Card { .. }), "{back:?} goes back to the card");
            press(&mut app, key(KeyCode::Char('f')));
        }
        app.graphics = Graphics::Off;
        app.mode = Mode::Card { path: PathBuf::from("/work/wt2.web"), name: "search-filters".to_string(), scroll: 0 };
        let (rows, _) = render(&mut app, 120, 60);
        assert!(rows.join("\n").contains("↑ ↓ scroll · p push · esc close"), "without graphics there is nothing to show larger");
        press(&mut app, key(KeyCode::Char('f')));
        assert!(matches!(app.mode, Mode::Card { .. }));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_image_too_large_or_not_an_image_is_not_decoded() {
        let dir = std::env::temp_dir().join(format!("kelpie-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake.png");
        std::fs::write(&fake, b"\x89PNG\r\n\x1a\nnot really").unwrap();
        assert!(decode_image(&fake).is_none());
        let big = dir.join("big.png");
        std::fs::File::create(&big).unwrap().set_len(IMAGE_BYTES + 1).unwrap();
        assert!(decode_image(&big).is_none());
        assert!(decode_image(&dir.join("missing.png")).is_none());
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
        app.act(Act::Go);
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogName(entry), .. } if entry.reference == "Projects/docs-refresh.md" && entry.source == "vault"));
        app.mode = Mode::Normal;
        app.backlog_selected = 0;
        app.act(Act::Go);
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

    const ESCAPES: &str = "\x1b]8;;https://evil.example\x1b\\click\x1b]8;;\x1b\\ \x1b[31mred\x1b[0m \x07 \u{9b}2J";

    fn no_escape_reaches_the_terminal(buffer: &ratatui::buffer::Buffer, what: &str) {
        let leaked: Vec<String> = buffer.content.iter().map(|cell| cell.symbol().to_string())
            .filter(|symbol| symbol.chars().any(|character| character.is_control() || character == '\u{9b}')).collect();
        assert!(leaked.is_empty(), "{what}: {leaked:?}");
    }

    #[test]
    fn untrusted_text_on_the_board_never_carries_a_terminal_escape_into_a_cell() {
        let mut item = crate::status::tests::task(Some("idle"), Some("needs-you"), 1);
        item.name = format!("task {ESCAPES}");
        item.place = format!("repo{ESCAPES}");
        item.report.as_mut().unwrap().text = format!("report {ESCAPES}");
        item.report.as_mut().unwrap().report.as_mut().unwrap().next = format!("next {ESCAPES}");
        let mut question = messages::fold(QUESTION).remove(0);
        question.text = format!("question {ESCAPES}");
        question.question.as_mut().unwrap().options[0].0 = format!("label {ESCAPES}");
        item.question = Some(question.clone());
        let mut app = test_app(vec![item]);
        app.fold = Fold::Open;
        let mut message = messages::fold(QUESTION).remove(0);
        message.text = format!("message {ESCAPES}");
        message.from = format!("from{ESCAPES}");
        app.messages = vec![message];
        app.questions = vec![question];
        let (rows, buffer) = render(&mut app, 170, 40);
        no_escape_reaches_the_terminal(&buffer, "the work and messages panels");
        assert!(rows.join("\n").contains("task ]8;;https://evil.example\\click]8;"), "the text shows, stripped of its control characters: {}", rows.join("\n"));
        app.open_questions(Some(0));
        let (_, buffer) = render(&mut app, 170, 40);
        no_escape_reaches_the_terminal(&buffer, "the question window");
        app.mode = Mode::Message(format!("result {ESCAPES}"));
        let (_, buffer) = render(&mut app, 100, 30);
        no_escape_reaches_the_terminal(&buffer, "a result window");
        app.mode = Mode::Normal;
        app.column = Column::Backlog;
        app.backlog.entries.push(Entry { source: "local".to_string(), title: format!("entry {ESCAPES}"), priority: "high".to_string(), area: "app".to_string(), reference: "local:x".to_string(), summary: format!("about {ESCAPES}"), age: "3 d".to_string() });
        let (_, buffer) = render(&mut app, 170, 40);
        no_escape_reaches_the_terminal(&buffer, "the backlog column");
    }

    #[test]
    fn a_plan_rendered_as_markdown_carries_no_escape_and_a_card_image_path_is_shown_as_text() {
        let text = markdown(&format!("# Plan {ESCAPES}\n\n- step [link](javascript:alert(1)) {ESCAPES}\n\n```sh\n{ESCAPES}\n```\n"));
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| frame.render_widget(Paragraph::new(text.clone()), frame.area())).unwrap();
        no_escape_reaches_the_terminal(terminal.backend().buffer(), "a markdown plan");
        let card = report::Card { sha: "abc".to_string(), text: format!("![alt {ESCAPES}](../../etc/passwd)\n{ESCAPES}"), file: PathBuf::from("/state/cards/x.md") };
        let lines = card_lines(&card, true, 60);
        let shown: String = lines.iter().map(|line| line.to_string()).collect::<Vec<_>>().join("\n");
        assert!(shown.contains("▣ ../../etc/passwd"), "an image is shown by its path, read by the user before anything opens it: {shown}");
        terminal.draw(|frame| frame.render_widget(Paragraph::new(lines.clone()), frame.area())).unwrap();
        no_escape_reaches_the_terminal(terminal.backend().buffer(), "a card");
    }

    #[test]
    fn bidi_overrides_and_zero_width_characters_in_untrusted_text_never_reach_a_cell() {
        let mut item = crate::status::tests::task(Some("idle"), None, 0);
        item.name = "push\u{202e}tpircs\u{200b}\u{feff}\u{2066}x".to_string();
        let mut app = test_app(vec![item]);
        let (rows, buffer) = render(&mut app, 120, 30);
        for hidden in ['\u{202e}', '\u{200b}', '\u{feff}', '\u{2066}'] {
            assert!(!buffer.content.iter().any(|cell| cell.symbol().contains(hidden)), "{hidden:?} was drawn");
        }
        assert!(rows.join("\n").contains("pushtpircsx"), "{}", rows.join("\n"));
    }

    #[test]
    fn the_messages_panel_fold_is_read_back_from_its_file_and_a_short_terminal_keeps_it_to_a_line() {
        assert_eq!(fold_from("open\t120\n", 50), (Fold::Open, 120));
        assert_eq!(fold_from("open\t120\n", 30), (Fold::Line, 120), "under 40 rows an open panel starts as a line");
        assert_eq!(fold_from("hidden\t7", 30), (Fold::Hidden, 7));
        assert_eq!(fold_from("line", 50), (Fold::Line, 0));
        assert_eq!(fold_from("", 50), (Fold::Open, 0));
        assert_eq!(fold_from("", 39), (Fold::Line, 0));
        assert_eq!(fold_from("garbage\tnot a number", 50), (Fold::Open, 0));
        assert_eq!(Fold::Hidden.next(), Fold::Line);
        assert_eq!(Fold::Line.next(), Fold::Open);
        assert_eq!(Fold::Open.next(), Fold::Hidden);
        assert_eq!(messages_height(Fold::Open, 9), 4);
        assert_eq!(messages_height(Fold::Open, 60), 20);
        assert_eq!((messages_height(Fold::Line, 60), messages_height(Fold::Hidden, 60)), (1, 0));
    }

    #[test]
    fn the_question_windows_parts_cycle_both_ways() {
        let order = [Part::List, Part::Plan, Part::Options, Part::Comment];
        for (index, part) in order.iter().enumerate() {
            assert!(part.next(false) == order[(index + 1) % 4]);
            assert!(part.next(true) == order[(index + 3) % 4]);
        }
    }

    #[test]
    fn text_is_cut_at_a_width_by_characters_and_fitted_span_by_span() {
        assert_eq!(cut("abc", 3), "abc");
        assert_eq!(cut("abcd", 3), "ab…");
        assert_eq!(cut("åäöü", 3), "åä…");
        assert_eq!(cut("abc", 0), "…", "a zero width still yields the ellipsis, never a panic");
        let line = fit(vec![Span::raw("12345"), Span::raw("678"), Span::raw("90")], 7);
        assert_eq!(line.to_string(), "123456…");
        assert_eq!(line.spans.len(), 2, "the span that overflows is the last one");
        assert_eq!(fit(vec![Span::raw("ab")], 10).to_string(), "ab");
        assert_eq!(changed_text(&["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()], 100), "a.rs, b.rs, c.rs");
        assert_eq!(changed_text(&["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()], 14), "a.rs, b.rs +1");
        assert_eq!(changed_text(&["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()], 12), "a.rs +2");
        assert_eq!(changed_text(&["a-very-long-name.rs".to_string(), "b.rs".to_string()], 4), "a-very-long-name.rs +1", "the first name always shows");
        assert_eq!(changed_text(&[], 10), "");
    }

    #[test]
    fn wrap_truncates_at_the_line_limit_with_an_ellipsis_and_keeps_long_words() {
        assert_eq!(wrap("one two three four five six", 9, 2), ["one two", "three fou…"], "the width floor is ten, and the last kept line ends in an ellipsis");
        assert_eq!(wrap("one two three four five six", 12, 2), ["one two", "three four…"]);
        assert_eq!(wrap("supercalifragilistic", 5, 3), ["supercalifragilistic"], "a word is never split");
        assert_eq!(wrap("", 20, 3), Vec::<String>::new());
        assert_eq!(wrap("a  b\n\n c", 20, 3), ["a b c"], "whitespace collapses");
    }

    #[test]
    fn the_task_menu_puts_each_key_in_its_group() {
        assert_eq!(menu_group(Some(Act::Inbox)), "SESSION");
        assert_eq!(menu_group(Some(Act::Server)), "DEV");
        assert_eq!(menu_group(Some(Act::Drop)), "END IT");
        assert_eq!(menu_group(Some(Act::Push)), "BRANCH");
        assert_eq!(menu_group(Some(Act::Permit)), "BRANCH");
        let help = help_lines();
        let labels: Vec<&str> = help.iter().map(|line| line.spans[0].content.trim()).collect();
        for key in ['n', 'g', 'c', 'r', 'w', 'b', 'v', 'a', 'p', 'd', 'z', 'x', 'R', 'A', 'U', 'i', 'm', 'q', '/'] {
            assert!(labels.iter().any(|label| label.split_whitespace().any(|word| word == key.to_string())), "{key} is not in the keys help: {labels:?}");
        }
    }

    #[test]
    fn a_task_started_from_the_board_runs_as_its_own_process_with_the_name_prompt_and_item_as_arguments() {
        let source = include_str!("board.rs");
        let start = &source[source.find("\nfn start_in_background(").unwrap()..];
        let body = &start[..start.find("\n}\n").unwrap()];
        assert!(body.contains(".arg(\"-C\").arg(repo)") && body.contains(".arg(prompt)") && body.contains("[\"--backlog\", item]"), "arguments go as argv, never through a shell: {body}");
        assert!(!body.contains("sh") || !body.contains("\"-c\""), "{body}");
        assert!(body.contains("process_group(0)"));
    }
}
