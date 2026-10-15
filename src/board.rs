use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::backlog::{self, Backlog, Entry};
use crate::herdr;
use crate::repo::name_of;
use crate::sh::Result;
use crate::status::{self, Group, Item};
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
enum View {
    Work,
    Backlog,
}

enum Purpose {
    TaskName,
    TaskPrompt(String),
    Message(String, String),
    BacklogName(Entry),
}

enum Mode {
    Normal,
    Input { purpose: Purpose, label: String, buffer: String },
    Message(String),
    ConfirmDrop { path: PathBuf, name: String, force: bool },
    Help,
}

struct App {
    view: View,
    items: Vec<Item>,
    backlog: Backlog,
    work_selected: usize,
    backlog_selected: usize,
    show_sessions: bool,
    mode: Mode,
    overlay: bool,
    loaded: bool,
    error: Option<String>,
    messages: HashMap<String, (Instant, String)>,
    refresh: Sender<()>,
    quit: bool,
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

fn status_words(status: &str) -> &'static str {
    match status {
        "blocked" => "waiting for your answer",
        "done" => "finished, not looked at yet",
        "working" => "working",
        "idle" => "idle",
        _ => "state unknown",
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

fn spawn_refresher() -> (Receiver<std::result::Result<Vec<Item>, String>>, Sender<()>) {
    let (items_tx, items_rx) = mpsc::channel();
    let (refresh_tx, refresh_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        loop {
            if items_tx.send(status::gather()).is_err() {
                return;
            }
            let _ = refresh_rx.recv_timeout(Duration::from_secs(3));
        }
    });
    (items_rx, refresh_tx)
}

fn review_script() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    Some(exe.ancestors().nth(3)?.join("scripts/review.sh"))
}

impl App {
    fn work(&self) -> Vec<&Item> {
        self.items.iter().filter(|item| item.is_task() || self.show_sessions).collect()
    }

    fn session_count(&self) -> usize {
        self.items.iter().filter(|item| !item.is_task()).count()
    }

    fn selected_item(&self) -> Option<Item> {
        self.work().get(self.work_selected).map(|item| (*item).clone())
    }

    fn selected_entry(&self) -> Option<Entry> {
        self.backlog.entries.get(self.backlog_selected).cloned()
    }

    fn agent_message(&mut self, pane: &str) -> String {
        if let Some((at, message)) = self.messages.get(pane)
            && at.elapsed() < Duration::from_secs(4)
        {
            return message.clone();
        }
        let message = last_message(&herdr::read_agent(pane, 120).unwrap_or_default());
        self.messages.insert(pane.to_string(), (Instant::now(), message.clone()));
        message
    }

    fn repo_named(&self, name: &str) -> Option<PathBuf> {
        self.items.iter().filter_map(|item| item.repo.clone()).find(|repo| name_of(repo) == name)
    }

    fn any_repo(&self) -> PathBuf {
        self.selected_item().and_then(|item| item.repo)
            .or_else(|| self.items.iter().find_map(|item| item.repo.clone()))
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
        let length = match self.view {
            View::Work => self.work().len(),
            View::Backlog => self.backlog.entries.len(),
        };
        let selected = match self.view {
            View::Work => &mut self.work_selected,
            View::Backlog => &mut self.backlog_selected,
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
            KeyCode::Tab | KeyCode::BackTab => self.view = if self.view == View::Work { View::Backlog } else { View::Work },
            KeyCode::Esc | KeyCode::Char('q') if self.overlay => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('s') => {
                self.mode = Mode::Input { purpose: Purpose::TaskName, label: "New task: name".to_string(), buffer: String::new() };
            }
            _ if self.view == View::Backlog => self.handle_backlog(key),
            _ => self.handle_work(key),
        }
    }

    fn handle_backlog(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                if let Some(entry) = self.selected_entry() {
                    let buffer = suggested_name(&entry.title);
                    self.mode = Mode::Input { purpose: Purpose::BacklogName(entry), label: "Start as task: name".to_string(), buffer };
                }
            }
            KeyCode::Char('R') => {
                let result = match self.lead_pane() {
                    Some(pane) => herdr::prompt_agent(&pane, REFRESH_BACKLOG)
                        .map(|_| "Asked the lead to refresh the backlog; it updates here when it is saved.".to_string()),
                    None => Err("No lead is running; prefix+space starts it.".to_string()),
                };
                self.show(result);
            }
            _ => {}
        }
    }

    fn handle_work(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('o') {
            self.show_sessions = !self.show_sessions;
            self.work_selected = 0;
            return;
        }
        let Some(item) = self.selected_item() else { return };
        match key.code {
            KeyCode::Enter => {
                if let Some(workspace) = &item.workspace {
                    let _ = herdr::focus_workspace(workspace);
                    if self.overlay {
                        self.quit = true;
                    }
                }
            }
            KeyCode::Char('p') => {
                if let Some(agent) = item.agent {
                    let label = format!("Message to {}", item.name);
                    self.mode = Mode::Input { purpose: Purpose::Message(agent.pane, item.name), label, buffer: String::new() };
                }
            }
            KeyCode::Char('r') => self.open_review(&item),
            KeyCode::Char(command @ ('l' | 'd' | 'x')) => {
                let Some(path) = item.path.clone() else {
                    self.mode = Mode::Message(format!("{} is a session, not a task in a worktree.", item.name));
                    return;
                };
                match command {
                    'l' => self.show(tasks::land(&path, None)),
                    'd' => self.show(tasks::done(&path, None)),
                    _ => self.mode = Mode::ConfirmDrop { path, name: item.name, force: false },
                }
            }
            _ => {}
        }
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
                let result = outside(terminal, || tasks::start(&repo, &name, prompt.as_deref(), true));
                self.show(result);
            }
            Purpose::BacklogName(entry) => {
                let Some(repo) = self.repo_named(&entry.area) else {
                    self.mode = Mode::Message(format!("No checkout of {} is open in herdr; open it as a Space first.", entry.area));
                    return;
                };
                let prompt = match entry.source.as_str() {
                    "linear" => format!("Work on Linear issue {}: {}. Read the issue first, then plan with me before changing code.", entry.reference, entry.title),
                    _ => format!("Work on the vault note {} (\"{}\"). Read it with the vault tools first, then plan with me before changing code.", entry.reference, entry.title),
                };
                let result = outside(terminal, || tasks::start(&repo, &text, Some(&prompt), true));
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

fn header(app: &App, width: u16) -> Line<'static> {
    let tasks = app.items.iter().filter(|item| item.is_task()).count();
    let tab = |label: &str, count: usize, active: bool| -> Vec<Span<'static>> {
        let label_style = if active { bold(MAUVE).add_modifier(Modifier::UNDERLINED) } else { style(MUTED) };
        vec![
            Span::styled(label.to_string(), label_style),
            Span::styled(format!(" {count}"), style(if active { TEXT } else { MUTED })),
            Span::raw("   "),
        ]
    };
    let mut spans = vec![Span::styled("kelpie", bold(MAUVE)), Span::raw("   ")];
    spans.extend(tab("Work", tasks, app.view == View::Work));
    spans.extend(tab("Backlog", app.backlog.entries.len(), app.view == View::Backlog));
    let fresh = format!("backlog {}", backlog::age(app.backlog.updated));
    let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    let gap = (width as usize).saturating_sub(used + fresh.chars().count()).max(1);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(fresh, style(MUTED)));
    Line::from(spans)
}

fn group_line(title: &str, count: usize, color: Color) -> Line<'static> {
    Line::from(vec![Span::styled(title.to_string(), bold(color)), Span::styled(format!("  {count}"), style(FAINT))])
}

// The name gives way when the pane is narrow; place and facts stay readable on the right.
fn row(selected: bool, icon: Span<'static>, name: &str, place: &str, facts: &str, width: usize) -> Line<'static> {
    let facts = if facts.is_empty() { String::new() } else { format!(" · {facts}") };
    let right_width = (place.chars().count() + facts.chars().count()).min(width / 2);
    let name_width = width.saturating_sub(right_width + 6);
    let name_count = name.chars().count();
    let mut shown: String = name.chars().take(name_width).collect();
    if name_count > name_width && name_width > 1 {
        shown = name.chars().take(name_width - 1).collect::<String>() + "…";
    }
    let padding = name_width.saturating_sub(shown.chars().count());
    let bar = if selected { Span::styled("▌", style(MAUVE)) } else { Span::raw(" ") };
    let name_style = if selected { bold(TEXT) } else { style(TEXT) };
    let right: String = format!("{place}{facts}").chars().take(right_width).collect();
    let (place_part, facts_part) = match right.split_once(" · ") {
        Some((place, facts)) => (place.to_string(), format!(" · {facts}")),
        None => (right, String::new()),
    };
    let line = Line::from(vec![
        bar, icon, Span::raw(" "), Span::styled(shown, name_style), Span::raw(" ".repeat(padding + 2)),
        Span::styled(place_part, style(TEAL)), Span::styled(facts_part, style(MUTED)),
    ]);
    if selected { line.style(Style::new().bg(SURFACE)) } else { line }
}

fn work_lines(app: &App, width: usize) -> (Vec<Line<'static>>, usize) {
    let mut lines = Vec::new();
    let mut selected_line = 0;
    let mut current: Option<Option<Group>> = None;
    let work = app.work();
    let section = |item: &Item| item.is_task().then(|| item.group());
    for (index, item) in work.iter().enumerate() {
        let key = section(item);
        if current != Some(key) {
            if current.is_some() {
                lines.push(Line::default());
            }
            let count = work.iter().filter(|other| section(other) == key).count();
            lines.push(if item.is_task() {
                group_line(item.group().title(), count, group_color(item.group()))
            } else {
                group_line("OTHER SESSIONS", count, MUTED)
            });
            current = Some(key);
        }
        if index == app.work_selected {
            selected_line = lines.len();
        }
        let facts = if item.is_task() { row_facts(item) } else { String::new() };
        lines.push(row(index == app.work_selected, status_icon(item), &item.name, &item.place, &facts, width));
    }
    if work.is_empty() {
        lines.push(Line::from(Span::styled("No tasks yet. s starts one; tab shows the backlog.", style(MUTED))));
    }
    let sessions = app.session_count();
    if !app.show_sessions && sessions > 0 {
        lines.push(Line::default());
        let text = format!("+ {sessions} other Claude {}", if sessions == 1 { "session" } else { "sessions" });
        let gap = width.saturating_sub(text.chars().count() + 12).max(1);
        lines.push(Line::from(vec![
            Span::styled(format!("  {text}"), style(MUTED)), Span::raw(" ".repeat(gap)),
            Span::styled("o", style(MAUVE)), Span::styled(" to show", style(MUTED)),
        ]));
    }
    (lines, selected_line)
}

fn backlog_lines(app: &App, width: usize) -> (Vec<Line<'static>>, usize) {
    let mut lines = Vec::new();
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
        if index == app.backlog_selected {
            selected_line = lines.len();
        }
        let icon = Span::styled(priority_icon(&entry.priority), style(priority_color(&entry.priority)));
        let facts = if entry.age.is_empty() { entry.source.clone() } else { format!("{} · {}", entry.source, entry.age) };
        lines.push(row(index == app.backlog_selected, icon, &entry.title, &entry.area, &facts, width));
    }
    (lines, selected_line)
}

fn chip(text: String, color: Color) -> Vec<Span<'static>> {
    vec![Span::styled(format!(" {text} "), Style::new().fg(color).bg(MANTLE)), Span::raw(" ")]
}

// Wrapped by hand so every line keeps the margin, and cut to the lines the box has left.
fn quote(who: &str, text: String, accent: Color, width: usize, room: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![Span::styled("┃ ", style(accent)), Span::styled(who.to_string(), style(MUTED))])];
    let width = width.saturating_sub(2).max(10);
    let mut wrapped: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            wrapped.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        wrapped.push(current);
    }
    let room = room.saturating_sub(1).max(1);
    if wrapped.len() > room {
        wrapped.truncate(room);
        let last = wrapped.last_mut().unwrap();
        let keep = width.saturating_sub(1).min(last.chars().count());
        *last = last.chars().take(keep).collect::<String>() + "…";
    }
    lines.extend(wrapped.into_iter().map(|text| Line::from(vec![Span::styled("┃ ", style(accent)), Span::styled(text, style(SUBTEXT))])));
    lines
}

fn title_line(name: &str, right: String, width: usize) -> Line<'static> {
    let gap = width.saturating_sub(name.chars().count() + right.chars().count()).max(1);
    Line::from(vec![Span::styled(name.to_string(), bold(MAUVE)), Span::raw(" ".repeat(gap)), Span::styled(right, style(MUTED))])
}

fn work_detail(app: &mut App, width: usize, height: usize) -> Vec<Line<'static>> {
    let Some(item) = app.selected_item() else {
        return vec![Line::from(Span::styled("Nothing selected.", style(MUTED)))];
    };
    let mut lines = vec![title_line(&item.name, item.place.clone(), width)];
    let blocked = item.agent.as_ref().is_some_and(|agent| agent.status == "blocked");
    if let Some(agent) = &item.agent {
        let mut spans = vec![status_icon(&item), Span::raw(" "), Span::styled(status_words(&agent.status), style(TEXT))];
        if let Some(context) = &agent.context {
            spans.push(Span::styled(format!(" · {context} context"), style(MUTED)));
        }
        lines.push(Line::from(spans));
    }
    let mut chips = Vec::new();
    if let Some(path) = &item.path {
        chips.extend(chip(name_of(path), SUBTEXT));
    }
    if let Some(git) = &item.git {
        if git.ahead > 0 {
            chips.extend(chip(plural(git.ahead, "commit"), SUBTEXT));
        }
        if git.changed > 0 {
            chips.extend(chip(format!("{} changed", plural(git.changed, "file")), SUBTEXT));
        }
        if git.behind > 0 {
            chips.extend(chip(format!("{} behind {}", git.behind, git.upstream.trim_start_matches("origin/")), YELLOW));
        }
    }
    if let Some((read, total)) = item.review {
        chips.extend(chip(format!("review {read}/{total} read"), if read == total { GREEN } else { SUBTEXT }));
    }
    if item.web {
        chips.extend(chip("web ●".to_string(), GREEN));
    }
    if !chips.is_empty() {
        lines.push(Line::from(chips));
    }
    match &item.agent {
        Some(agent) => {
            let message = app.agent_message(&agent.pane);
            if !message.is_empty() {
                let (who, accent) = if blocked { ("asks", PEACH) } else { ("last said", FAINT) };
                let room = height.saturating_sub(lines.len());
                lines.extend(quote(who, message, accent, width, room));
            }
        }
        None => lines.push(Line::from(Span::styled("No Claude running in its Space.", style(MUTED)))),
    }
    lines
}

fn backlog_detail(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
    let Some(entry) = app.selected_entry() else {
        return vec![
            Line::from(Span::styled("The backlog is empty.", style(SUBTEXT))),
            Line::from(Span::styled("R asks the lead to fill it from the vault and Linear.", style(MUTED))),
        ];
    };
    let mut lines = vec![title_line(&entry.title, format!("{} · {}", entry.priority, entry.area), width)];
    let mut chips = chip(if entry.source == "linear" { entry.reference.clone() } else { "vault note".to_string() }, SUBTEXT);
    if !entry.age.is_empty() {
        chips.extend(chip(format!("{} old", entry.age), SUBTEXT));
    }
    chips.extend(chip(format!("⏎ start as {}", suggested_name(&entry.title)), GREEN));
    lines.push(Line::from(chips));
    let about = if entry.summary.is_empty() {
        format!("the {}'s context and plan, in a free worktree of {}", if entry.source == "linear" { "issue" } else { "note" }, entry.area)
    } else {
        entry.summary.clone()
    };
    let room = height.saturating_sub(lines.len());
    lines.extend(quote("the task's Claude starts with", about, FAINT, width, room));
    lines
}

fn footer(app: &App, width: usize) -> Line<'static> {
    let mut keys: Vec<(&str, &str)> = match app.view {
        View::Backlog => vec![("⏎", "start as task"), ("R", "refresh"), ("s", "new task"), ("tab", "work")],
        View::Work => {
            let task = app.selected_item().is_some_and(|item| item.is_task());
            let mut keys = vec![("⏎", "go"), ("p", "message")];
            if task {
                keys.extend([("r", "review"), ("l", "ready to push"), ("d", "done"), ("x", "drop")]);
            }
            keys.extend([("s", "start"), ("o", "sessions"), ("tab", "backlog")]);
            keys
        }
    };
    let mut tail = vec![("?", "help")];
    if app.overlay {
        tail.push(("esc", "close"));
    }
    let length = |keys: &[(&str, &str)]| keys.iter().map(|(key, label)| key.chars().count() + label.chars().count() + 4).sum::<usize>();
    while !keys.is_empty() && length(&keys) + length(&tail) > width {
        keys.pop();
    }
    Line::from(keys.into_iter().chain(tail).flat_map(|(key, label)| [
        Span::styled(key.to_string(), style(MAUVE)),
        Span::styled(format!(" {label}   "), style(MUTED)),
    ]).collect::<Vec<_>>())
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

fn modal(frame: &mut Frame, title: &str, lines: Vec<Line<'static>>, width: u16) {
    let height = lines.len() as u16 + 2;
    let area = centered(frame.area(), width, height);
    frame.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), bold(MAUVE)));
    frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), area);
}

fn help_lines(view: View) -> Vec<Line<'static>> {
    let keys: Vec<(&str, &str)> = match view {
        View::Work => vec![
            ("⏎", "go to the task's or session's Space"), ("p", "send its Claude a message without leaving"),
            ("r", "open the task's review next to its Claude"), ("l", "rebase and check; shows the push command"),
            ("d", "after you pushed: close the task, free its worktree"), ("x", "drop the task (asks first)"),
            ("s", "start a new task"), ("o", "show or hide Claude sessions outside tasks"),
        ],
        View::Backlog => vec![
            ("⏎", "start the item as a task (name suggested, editable)"), ("R", "ask the lead to refresh the backlog"),
            ("s", "start a task of your own"),
        ],
    };
    keys.into_iter().chain([("tab", "switch between work and backlog"), ("j k ↑ ↓", "move"), ("esc", "close this, or the board when it is an overlay")])
        .map(|(key, text)| Line::from(vec![Span::styled(format!("{key:<9}"), style(MAUVE)), Span::styled(text.to_string(), style(TEXT))]))
        .collect()
}

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area().inner(Margin { horizontal: 1, vertical: 0 });
    let detail_height = 9.min(area.height / 2);
    let [top, rule, body, detail_area, bottom] = Layout::vertical([
        Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(detail_height), Constraint::Length(1),
    ]).areas(area);
    frame.render_widget(Paragraph::new(header(app, top.width)), top);
    frame.render_widget(Paragraph::new(Span::styled("─".repeat(rule.width as usize), style(SURFACE))), rule);
    frame.render_widget(Paragraph::new(footer(app, bottom.width as usize)), bottom);

    let width = body.width as usize;
    if let Some(error) = &app.error {
        frame.render_widget(Paragraph::new(Span::styled(error.clone(), style(RED))).wrap(Wrap { trim: false }), body);
    } else if !app.loaded {
        frame.render_widget(Paragraph::new(Span::styled("looking around…", style(MUTED))), body);
    } else {
        let (lines, selected_line) = match app.view {
            View::Work => work_lines(app, width),
            View::Backlog => backlog_lines(app, width),
        };
        let scroll = selected_line.saturating_sub(body.height.saturating_sub(2) as usize) as u16;
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
    }

    let inner_width = detail_area.width.saturating_sub(4) as usize;
    let inner_height = detail_area.height.saturating_sub(2) as usize;
    let lines = match app.view {
        View::Work => work_detail(app, inner_width, inner_height),
        View::Backlog => backlog_detail(app, inner_width, inner_height),
    };
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(FAINT)).padding(Padding::horizontal(1));
    frame.render_widget(Paragraph::new(lines).block(block), detail_area);

    match &app.mode {
        Mode::Normal => {}
        Mode::Help => modal(frame, "keys", help_lines(app.view), 70),
        Mode::Message(text) => {
            let mut lines: Vec<Line> = text.lines().map(|line| Line::from(Span::styled(line.to_string(), style(TEXT)))).collect();
            lines.push(Line::from(Span::styled("any key to close", style(MUTED))));
            modal(frame, "kelpie", lines, 96);
        }
        Mode::ConfirmDrop { name, force, .. } => {
            let question = if *force { "Discard its changes and commits?" } else { "Drop this task?" };
            modal(frame, "drop", vec![
                Line::from(Span::styled(name.clone(), style(TEXT))),
                Line::default(),
                Line::from(vec![Span::styled(question, style(YELLOW)), Span::styled("  y / n", style(MUTED))]),
            ], 72);
        }
        Mode::Input { label, buffer, .. } => modal(frame, label, vec![
            Line::from(vec![Span::styled(buffer.clone(), style(TEXT)), Span::styled("▏", style(MAUVE))]),
            Line::from(Span::styled("enter to continue · esc to cancel", style(MUTED))),
        ], 72),
    }
}

pub fn run(overlay: bool) -> Result<String> {
    let (items_rx, refresh) = spawn_refresher();
    let mut app = App {
        view: View::Work, items: Vec::new(), backlog: backlog::load(), work_selected: 0, backlog_selected: 0,
        show_sessions: false, mode: Mode::Normal, overlay, loaded: false, error: None, messages: HashMap::new(),
        refresh, quit: false,
    };
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        while !app.quit {
            while let Ok(update) = items_rx.try_recv() {
                match update {
                    Ok(items) => {
                        let previous = app.selected_item().map(|item| (item.name, item.place));
                        app.items = items;
                        let work = app.work();
                        let position = previous.and_then(|(name, place)| work.iter().position(|item| item.name == name && item.place == place));
                        app.work_selected = position.unwrap_or(app.work_selected.min(work.len().saturating_sub(1)));
                        app.backlog = backlog::load();
                        app.backlog_selected = app.backlog_selected.min(app.backlog.entries.len().saturating_sub(1));
                        app.error = None;
                    }
                    Err(error) => app.error = Some(error),
                }
                app.loaded = true;
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
