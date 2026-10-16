use std::collections::HashMap;
use std::io::BufRead;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::backlog::{self, Backlog, Entry};
use crate::herdr;
use crate::repo::{self, name_of};
use crate::sh::Result;
use crate::status::{self, Checkout, Group, Item};
use crate::tasks;

const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
const SUBTEXT: Color = Color::Rgb(0xa6, 0xad, 0xc8);
const MUTED: Color = Color::Rgb(0x6c, 0x70, 0x86);
const FAINT: Color = Color::Rgb(0x45, 0x47, 0x5a);
const SURFACE: Color = Color::Rgb(0x31, 0x32, 0x44);
const MANTLE: Color = Color::Rgb(0x18, 0x18, 0x25);
const MAUVE: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
const PEACH: Color = Color::Rgb(0xfa, 0xb3, 0x87);
const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
const BLUE: Color = Color::Rgb(0x89, 0xb4, 0xfa);
const TEAL: Color = Color::Rgb(0x94, 0xe2, 0xd5);
const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);

const REFRESH_BACKLOG: &str = "Refresh the kelpie backlog: take the vault backlog and my open Linear issues and pipe them as one JSON array to `$HOME/.local/share/kelpie/current/target/release/kelpie backlog save`. Fields per entry: source (vault or linear), title, priority (urgent, high, medium, low or none), area (the repo, e.g. auctionet), ref (vault path or Linear id), summary (one line), age (e.g. 3 d). Reply in one line when saved.";

#[derive(Clone, Copy, PartialEq)]
enum Column {
    Work,
    Backlog,
}

struct Choice {
    key: char,
    label: String,
    about: String,
}

fn choice(key: char, label: &str, about: &str) -> Choice {
    Choice { key, label: label.to_string(), about: about.to_string() }
}

fn task_choices(item: Option<&Item>) -> Vec<Choice> {
    let mut choices = vec![
        choice('g', "Go to its Space", "jump to the task's Space"),
        choice('p', "Message its Claude", "send it a line without leaving"),
        choice('r', "Open the review", "the review split next to its Claude"),
    ];
    if item.and_then(|item| item.path.as_ref()).is_some_and(|path| path.join("dev.yml").exists()) {
        let item = item.unwrap();
        choices.push(if item.web {
            choice('w', "Stop the web server", "ctrl-c to its dev web")
        } else {
            choice('w', "Start the web server", "dev web in a tab of its Space")
        });
        choices.push(if item.jobs {
            choice('b', "Stop the worker", "ctrl-c to its dev worker")
        } else {
            choice('b', "Start the worker", "dev worker in a tab of its Space")
        });
    }
    choices.extend([
        choice('l', "Push and close", "rebase, check, show the commits, then push if you say so"),
        choice('d', "Done", "already pushed elsewhere: close it and free the worktree"),
        choice('x', "Drop", "throw the task away (asks first)"),
    ]);
    choices
}

fn backlog_choices() -> Vec<Choice> {
    vec![
        choice('g', "Start as task", "a worktree, a Space and Claude with this as its prompt"),
        choice('R', "Refresh the backlog", "the lead fetches the vault and Linear again"),
    ]
}

enum Purpose {
    TaskName,
    TaskPrompt(String),
    Message(String, String),
    BacklogName(Entry),
}

enum Mode {
    Normal,
    Options(usize),
    Input { purpose: Purpose, label: String, buffer: String },
    Message(String),
    ConfirmDrop { path: PathBuf, name: String, force: bool },
    ConfirmPush { path: PathBuf, summary: String },
    Help,
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
    messages: HashMap<String, (Instant, String)>,
    refresh: Sender<()>,
    quit: bool,
    wanted_task: Option<String>,
}

fn style(color: Color) -> Style {
    Style::new().fg(color)
}

fn bold(color: Color) -> Style {
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

fn plural(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

fn row_facts(item: &Item) -> String {
    let Some(git) = &item.git else { return String::new() };
    let mut parts = Vec::new();
    if git.ahead > 0 {
        parts.push(plural(git.ahead, "commit"));
    }
    if git.changed > 0 {
        parts.push(plural(git.changed, "file"));
    }
    if git.behind > 0 {
        parts.push(format!("{} behind", git.behind));
    }
    if parts.is_empty() { "nothing yet".to_string() } else { parts.join(" · ") }
}

fn suggested_name(title: &str) -> String {
    const FILLER: [&str; 12] = ["a", "an", "the", "and", "of", "for", "to", "on", "in", "with", "s", "before"];
    title.to_lowercase().split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty() && !FILLER.contains(word)).take(4).collect::<Vec<_>>().join("-")
}

// Claude's last message is the last "● " block on screen, up to its footer lines.
fn last_message(output: &str) -> String {
    let lines: Vec<&str> = output.lines().map(str::trim_end).collect();
    let Some(start) = lines.iter().rposition(|line| line.trim_start().starts_with("● ")) else { return String::new() };
    let stop = |line: &str| {
        let line = line.trim_start();
        ["✻", "※", "─", "❯", "⏵", "╭", "╰", "· "].iter().any(|marker| line.starts_with(marker))
    };
    lines[start..].iter().take_while(|line| !stop(line))
        .map(|line| line.trim()).filter(|line| !line.is_empty())
        .collect::<Vec<_>>().join(" ").trim_start_matches("● ").to_string()
}

fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
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

fn address(item: &Item) -> Option<String> {
    let path = item.path.as_ref()?;
    path.join("dev.yml").exists().then(|| format!("https://{}.devbox", name_of(path)))
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
            Column::Work => task_choices(self.tasks.get(self.work_selected)),
            Column::Backlog => backlog_choices(),
        }
    }

    fn agent_message(&mut self, pane: &str) -> String {
        if let Some((at, message)) = self.messages.get(pane)
            && at.elapsed() < Duration::from_secs(4)
        {
            return message.clone();
        }
        let message = last_message(&herdr::read_agent(pane).unwrap_or_default());
        self.messages.insert(pane.to_string(), (Instant::now(), message.clone()));
        message
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

    fn handle(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
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
            Mode::ConfirmPush { path, .. } => {
                if key.code == KeyCode::Char('y') {
                    let result = outside(terminal, || tasks::push(&path, None));
                    self.show(result);
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
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
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
            KeyCode::Char('s') => {
                self.mode = Mode::Input { purpose: Purpose::TaskName, label: "New task: name".to_string(), buffer: String::new() };
            }
            KeyCode::Char(character) if self.choices().iter().any(|choice| choice.key == character) => self.act(character),
            _ => {}
        }
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
                'R' => {
                    let result = match self.lead_pane() {
                        Some(pane) => herdr::prompt_agent(&pane, REFRESH_BACKLOG)
                            .map(|_| "Asked the lead to refresh the backlog; it updates here when it is saved.".to_string()),
                        None => Err("No lead is running; prefix+space starts it.".to_string()),
                    };
                    self.show(result);
                }
                _ => {}
            },
            Column::Work => {
                let Some(item) = self.selected_task() else { return };
                match key {
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
                    'w' => self.toggle_server(&item, "web", item.web),
                    'b' => self.toggle_server(&item, "worker", item.jobs),
                    'l' | 'd' | 'x' => {
                        let Some(path) = item.path.clone() else { return };
                        match key {
                            'l' => match tasks::land(&path, None) {
                                Ok(summary) => {
                                    let summary = summary.split("\n\nPush with:").next().unwrap_or_default().to_string();
                                    self.mode = Mode::ConfirmPush { path, summary };
                                }
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

    // A server runs as `dev web` or `dev worker` in a tab of the task's Space; stopping it is ctrl-c to that tab.
    fn toggle_server(&mut self, item: &Item, kind: &str, running: bool) {
        let (Some(workspace), Some(path)) = (&item.workspace, &item.path) else {
            self.mode = Mode::Message(format!("{} has no Space open.", item.name));
            return;
        };
        let result = if running {
            let pane = herdr::panes_in(workspace).unwrap_or_default().into_iter()
                .filter_map(|pane| pane["pane_id"].as_str().map(str::to_string))
                .find(|pane| herdr::foreground(pane).ok().flatten()
                    .is_some_and(|argv| argv.iter().any(|arg| arg.ends_with("dev")) && argv.iter().any(|arg| arg == kind)));
            match pane {
                Some(pane) => herdr::send_keys(&pane, &["ctrl+c"]).map(|_| format!("Stopping dev {kind} in {}.", item.name)),
                None => Err(format!("No dev {kind} found in {}'s Space.", item.name)),
            }
        } else {
            idle_tab_pane(workspace, kind).map(Ok).unwrap_or_else(|| herdr::create_tab(workspace, path, kind))
                .and_then(|pane| herdr::run_in_pane(&pane, &format!("dev {kind}")))
                .map(|_| format!("Started dev {kind} in the {kind} tab of {}'s Space.", item.name))
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
                let options = tasks::Start { prompt: prompt.as_deref(), focus: true, from: None, mode: None, dev: true };
                let result = outside(terminal, || tasks::start(&repo, &name, &options));
                self.show(result);
            }
            Purpose::BacklogName(entry) => {
                let prompt = match entry.source.as_str() {
                    "linear" => format!("Work on Linear issue {}: {}. Read the issue first, then plan with me before changing code.", entry.reference, entry.title),
                    _ => format!("Work on the vault note {} (\"{}\"). Read it with the vault tools first, then plan with me before changing code.", entry.reference, entry.title),
                };
                let options = tasks::Start { prompt: Some(&prompt), focus: true, from: None, mode: None, dev: true };
                let known = self.repo_named(&entry.area);
                let result = outside(terminal, || {
                    let repo = known.map_or_else(|| repo::locate(&entry.area), Ok)?;
                    tasks::start(&repo, &text, &options)
                });
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

// Leaves the TUI for a step that prints as it goes (creating a worktree takes a while), then comes back.
fn outside(terminal: &mut DefaultTerminal, step: impl FnOnce() -> Result<String>) -> Result<String> {
    ratatui::restore();
    let result = step();
    match &result {
        Ok(output) | Err(output) => println!("\n{output}\n\nPress Enter to return to the board."),
    }
    let _ = std::io::stdin().lock().lines().next();
    *terminal = ratatui::init();
    result
}

fn pill(text: String, color: Color, lit: bool) -> Vec<Span<'static>> {
    let color = if lit { color } else { FAINT };
    vec![Span::styled(format!(" {text} "), Style::new().fg(color).bg(MANTLE)), Span::raw(" ")]
}

fn header(app: &App, width: u16) -> Line<'static> {
    let count = |group: Group| app.tasks.iter().filter(|item| item.group() == group).count();
    let (needs, working, ready) = (count(Group::NeedsYou), count(Group::Working), count(Group::ReadyToPush));
    let mut spans = vec![Span::styled("kelpie", bold(MAUVE)), Span::raw("   ")];
    spans.extend(pill(format!("● {needs} need you"), PEACH, needs > 0));
    spans.extend(pill(format!("◐ {working} working"), BLUE, working > 0));
    spans.extend(pill(format!("✓ {ready} ready to push"), GREEN, ready > 0));
    let sources = if app.backlog.entries.iter().any(|entry| entry.source == "linear") { "vault + Linear" } else { "vault" };
    let fresh = format!("backlog: {sources}, {}", backlog::age(app.backlog.updated));
    let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    if used + fresh.chars().count() < width as usize {
        spans.push(Span::raw(" ".repeat(width as usize - used - fresh.chars().count())));
        spans.push(Span::styled(fresh, style(MUTED)));
    }
    Line::from(spans)
}

fn column_title(title: &str, right: String, focused: bool, width: usize) -> Vec<Line<'static>> {
    let gap = width.saturating_sub(title.chars().count() + right.chars().count()).max(1);
    vec![
        Line::from(vec![Span::styled(title.to_string(), bold(if focused { MAUVE } else { MUTED })), Span::raw(" ".repeat(gap)), Span::styled(right, style(MUTED))]),
        Line::from(Span::styled("─".repeat(width), style(SURFACE))),
    ]
}

fn group_line(title: &str, count: usize, color: Color) -> Line<'static> {
    Line::from(vec![Span::styled(title.to_string(), bold(color)), Span::styled(format!("  {count}"), style(FAINT))])
}

// The name gives way when the column is narrow; the facts stay readable on the right.
fn row(selected: bool, focused: bool, icon: Span<'static>, name: &str, place: Option<&str>, facts: &str, width: usize) -> Line<'static> {
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
fn fill(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    lines.into_iter().map(|mut line| {
        if line.style.bg == Some(SURFACE) && line.width() < width {
            let missing = width - line.width();
            line.spans.push(Span::raw(" ".repeat(missing)));
        }
        line
    }).collect()
}

fn chips(chips: Vec<(String, Color)>) -> Line<'static> {
    let mut spans = vec![Span::raw("   ")];
    for (text, color) in chips {
        spans.push(Span::styled(format!(" {text} "), Style::new().fg(color).bg(MANTLE)));
        spans.push(Span::raw(" "));
    }
    Line::from(spans).style(Style::new().bg(SURFACE))
}

fn card_text(label: &str, text: &str, accent: Color, width: usize, max_lines: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![Span::raw("   "), Span::styled("┃ ", style(accent)), Span::styled(label.to_string(), style(MUTED))])];
    for text in wrap(text, width.saturating_sub(6), max_lines) {
        lines.push(Line::from(vec![Span::raw("   "), Span::styled("┃ ", style(accent)), Span::styled(text, style(SUBTEXT))]));
    }
    lines.into_iter().map(|line| line.style(Style::new().bg(SURFACE))).collect()
}

fn options_hint() -> Line<'static> {
    Line::from(vec![Span::raw("   "), Span::styled("⏎", style(MAUVE)), Span::styled(" options", style(MUTED))]).style(Style::new().bg(SURFACE))
}

fn work_lines(app: &mut App, width: usize) -> (Vec<Line<'static>>, usize) {
    let focused = app.column == Column::Work;
    let free: usize = app.checkouts.iter().map(|checkout| checkout.free_slots).sum();
    let mut lines = column_title("WORK", format!("{} · {} free", plural(app.tasks.len(), "task"), plural(free, "worktree")), focused, width);
    let mut selected_line = 0;
    let mut current: Option<Group> = None;
    let tasks = app.tasks.clone();
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
        lines.push(row(selected, focused, status_icon(item), &item.name, place, &row_facts(item), width));
        if selected && focused {
            lines.extend(task_card(app, item, width));
        }
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
    (lines, selected_line)
}

fn task_card(app: &mut App, item: &Item, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut facts = Vec::new();
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
    if let Some((read, total)) = item.review {
        facts.push((format!("review {read}/{total} read"), if read == total { GREEN } else { SUBTEXT }));
    }
    if !facts.is_empty() {
        lines.push(chips(facts));
    }
    if item.web && let Some(address) = address(item) {
        lines.push(Line::from(vec![Span::raw("   "), Span::styled("web ", style(GREEN)), Span::styled(address, style(TEAL))]).style(Style::new().bg(SURFACE)));
    }
    match &item.agent {
        Some(agent) => {
            let message = app.agent_message(&agent.pane);
            if !message.is_empty() {
                let (label, accent) = if agent.status == "blocked" { ("asks", PEACH) } else { ("last said", FAINT) };
                lines.extend(card_text(label, &message, accent, width, 3));
            }
        }
        None => lines.extend(card_text("no Claude", "No Claude is running in its Space.", FAINT, width, 1)),
    }
    lines.push(options_hint());
    lines
}

fn backlog_lines(app: &App, width: usize) -> (Vec<Line<'static>>, usize) {
    let focused = app.column == Column::Backlog;
    let mut lines = column_title("BACKLOG", format!("{} items", app.backlog.entries.len()), focused, width);
    let mut selected_line = 0;
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
        let selected = index == app.backlog_selected;
        if selected {
            selected_line = lines.len();
        }
        let icon = Span::styled(priority_icon(&entry.priority), style(priority_color(&entry.priority)));
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
    }
    if app.backlog.entries.is_empty() {
        lines.push(Line::from(Span::styled("Empty. R asks the lead to fill it from the vault and Linear.", style(MUTED))));
    }
    (lines, selected_line)
}

fn render_column(frame: &mut Frame, area: Rect, lines: Vec<Line<'static>>, selected_line: usize) {
    let lines = fill(lines, area.width as usize);
    let scroll = selected_line.saturating_sub(area.height.saturating_sub(6) as usize) as u16;
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
}

fn hint_line() -> Line<'static> {
    Line::from([("⏎", "options"), ("← →", "work · backlog"), ("↑ ↓", "move"), ("s", "new task"), ("?", "all keys")]
        .into_iter().flat_map(|(key, label)| [Span::styled(key.to_string(), style(MAUVE)), Span::styled(format!(" {label}    "), style(MUTED))])
        .collect::<Vec<_>>())
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

fn modal(frame: &mut Frame, title: String, lines: Vec<Line<'static>>, width: u16) {
    let height = lines.len() as u16 + 2;
    let area = centered(frame.area(), width, height);
    frame.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), bold(MAUVE)));
    frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), area);
}

fn options_lines(choices: Vec<Choice>, index: usize, width: usize) -> Vec<Line<'static>> {
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
        ("⏎", "options for the selected task or backlog item"), ("← → tab", "switch between work and backlog"), ("↑ ↓ j k", "move"),
        ("s", "start a new task"), ("g", "go to the task's Space, or start the backlog item"), ("p", "message the task's Claude"),
        ("r", "open the task's review next to its Claude"), ("w", "start or stop the task's web server"), ("b", "start or stop the task's worker"), ("l", "ready to push: rebase and check (on a task)"), ("d", "done: after you pushed"),
        ("x", "drop the task (asks first)"), ("R", "ask the lead to refresh the backlog"), ("esc", "close this, or the board when it is an overlay"),
    ].into_iter().map(|(key, text)| Line::from(vec![Span::styled(format!("{key:<9}"), style(MAUVE)), Span::styled(text.to_string(), style(TEXT))])).collect()
}

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area().inner(Margin { horizontal: 2, vertical: 1 });
    let [top, _, body, bottom] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).areas(area);
    frame.render_widget(Paragraph::new(header(app, top.width)), top);
    frame.render_widget(Paragraph::new(hint_line()), bottom);

    if let Some(error) = &app.error {
        frame.render_widget(Paragraph::new(Span::styled(error.clone(), style(RED))).wrap(Wrap { trim: false }), body);
    } else if !app.loaded {
        frame.render_widget(Paragraph::new(Span::styled("looking around…", style(MUTED))), body);
    } else if body.width >= 86 {
        let [work_area, _, backlog_area] = Layout::horizontal([Constraint::Percentage(54), Constraint::Length(4), Constraint::Min(20)]).areas(body);
        let (work, work_selected) = work_lines(app, work_area.width as usize);
        render_column(frame, work_area, work, work_selected);
        let (backlog, backlog_selected) = backlog_lines(app, backlog_area.width as usize);
        render_column(frame, backlog_area, backlog, backlog_selected);
    } else {
        let width = body.width as usize;
        let (mut lines, work_selected) = work_lines(app, width);
        let offset = lines.len() + 1;
        let (backlog, backlog_selected) = backlog_lines(app, width);
        lines.push(Line::default());
        lines.extend(backlog);
        let selected = if app.column == Column::Work { work_selected } else { offset + backlog_selected };
        render_column(frame, body, lines, selected);
    }

    match &app.mode {
        Mode::Normal => {}
        Mode::Options(index) => {
            let title = match app.column {
                Column::Work => app.selected_task().map(|item| item.name).unwrap_or_default(),
                Column::Backlog => app.selected_entry().map(|entry| entry.title).unwrap_or_default(),
            };
            let title: String = title.chars().take(52).collect();
            modal(frame, title, options_lines(app.choices(), *index, 56), 62);
        }
        Mode::Help => modal(frame, "keys".to_string(), help_lines(), 72),
        Mode::Message(text) => {
            let mut lines: Vec<Line> = text.lines().map(|line| Line::from(Span::styled(line.to_string(), style(TEXT)))).collect();
            lines.push(Line::from(Span::styled("any key to close", style(MUTED))));
            modal(frame, "kelpie".to_string(), lines, 96);
        }
        Mode::ConfirmDrop { name, force, .. } => {
            let question = if *force { "Discard its changes and commits?" } else { "Drop this task?" };
            modal(frame, "drop".to_string(), vec![
                Line::from(Span::styled(name.clone(), style(TEXT))),
                Line::default(),
                Line::from(vec![Span::styled(question, style(YELLOW)), Span::styled("  y / n", style(MUTED))]),
            ], 72);
        }
        Mode::ConfirmPush { summary, .. } => {
            let mut lines: Vec<Line> = summary.lines().map(|line| Line::from(Span::styled(line.to_string(), style(TEXT)))).collect();
            lines.push(Line::default());
            lines.push(Line::from(vec![Span::styled("Push these and close the task?", style(YELLOW)), Span::styled("  y / n", style(MUTED))]));
            modal(frame, "push".to_string(), lines, 96);
        }
        Mode::Input { label, buffer, .. } => modal(frame, label.clone(), vec![
            Line::from(vec![Span::styled(buffer.clone(), style(TEXT)), Span::styled("▏", style(MAUVE))]),
            Line::from(Span::styled("enter to continue · esc to cancel", style(MUTED))),
        ], 72),
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
        let column = if app.column == Column::Backlog { "backlog" } else { "work" };
        let task = app.selected_task().map(|item| item.name).unwrap_or_default();
        let state = format!("{column}\t{}\t{task}", app.backlog_selected);
        ratatui::restore();
        let error = Command::new(path).args(std::env::args_os().skip(1)).env(RESTART_STATE, state).exec();
        format!("Restarting on the rebuilt kelpie failed: {error}")
    }
}

pub fn run(overlay: bool) -> Result<String> {
    let (snapshots, refresh) = spawn_refresher();
    let mut app = App {
        column: Column::Work, tasks: Vec::new(), checkouts: Vec::new(), backlog: backlog::load(), work_selected: 0, backlog_selected: 0,
        mode: Mode::Normal, overlay, loaded: false, error: None, messages: HashMap::new(), refresh, quit: false, wanted_task: None,
    };
    if let Ok(state) = std::env::var(RESTART_STATE) {
        let mut parts = state.splitn(3, '\t');
        if parts.next() == Some("backlog") {
            app.column = Column::Backlog;
        }
        app.backlog_selected = parts.next().and_then(|index| index.parse().ok()).unwrap_or(0);
        app.wanted_task = parts.next().filter(|name| !name.is_empty()).map(str::to_string);
    }
    let mut rebuild = Rebuild::new();
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        while !app.quit {
            while let Ok(update) = snapshots.try_recv() {
                match update {
                    Ok(snapshot) => {
                        let previous = app.selected_task().map(|item| (item.name, item.place));
                        app.tasks = snapshot.items.into_iter().filter(Item::is_task).collect();
                        app.checkouts = snapshot.checkouts;
                        let wanted = app.wanted_task.take();
                        let position = previous.and_then(|(name, place)| app.tasks.iter().position(|item| item.name == name && item.place == place))
                            .or_else(|| wanted.and_then(|name| app.tasks.iter().position(|item| item.name == name)));
                        app.work_selected = position.unwrap_or(app.work_selected.min(app.tasks.len().saturating_sub(1)));
                        app.backlog = backlog::load();
                        app.backlog_selected = app.backlog_selected.min(app.backlog.entries.len().saturating_sub(1));
                        app.error = None;
                    }
                    Err(error) => app.error = Some(error),
                }
                app.loaded = true;
            }
            if matches!(app.mode, Mode::Normal | Mode::Message(_) | Mode::Help) && rebuild.ready() {
                let error = rebuild.restart(&app);
                terminal = ratatui::init();
                app.mode = Mode::Message(error);
            }
            terminal.draw(|frame| draw(frame, &mut app)).map_err(|error| error.to_string())?;
            if event::poll(Duration::from_millis(250)).map_err(|error| error.to_string())?
                && let Event::Key(key) = event::read().map_err(|error| error.to_string())?
                && key.kind == KeyEventKind::Press
            {
                app.handle(key, &mut terminal);
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result.map(|_| String::new())
}
