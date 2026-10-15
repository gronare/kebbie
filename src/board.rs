use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::herdr;
use crate::sh::Result;
use crate::status::{self, Group, Item};
use crate::tasks;

const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
const SUBTEXT: Color = Color::Rgb(0xa6, 0xad, 0xc8);
const MUTED: Color = Color::Rgb(0x6c, 0x70, 0x86);
const SURFACE: Color = Color::Rgb(0x31, 0x32, 0x44);
const MAUVE: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
const PEACH: Color = Color::Rgb(0xfa, 0xb3, 0x87);
const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
const BLUE: Color = Color::Rgb(0x89, 0xb4, 0xfa);
const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);

enum Purpose {
    TaskName,
    TaskPrompt(String),
    Message(String, String),
}

enum Mode {
    Normal,
    Input { purpose: Purpose, label: String, buffer: String },
    Message(String),
    ConfirmDrop { path: PathBuf, name: String, force: bool },
    Help,
}

struct App {
    items: Vec<Item>,
    selected: usize,
    mode: Mode,
    overlay: bool,
    loaded: bool,
    error: Option<String>,
    output: HashMap<String, (Instant, Vec<String>)>,
    refresh: Sender<()>,
    quit: bool,
}

fn group_color(group: Group) -> Color {
    match group {
        Group::NeedsYou => PEACH,
        Group::Working => BLUE,
        Group::ReadyToPush => GREEN,
        Group::Idle => MUTED,
    }
}

fn status_icon(item: &Item) -> Span<'static> {
    match item.agent.as_ref().map(|agent| agent.status.as_str()) {
        Some("blocked") => Span::styled("●", Style::new().fg(PEACH)),
        Some("done") => Span::styled("✓", Style::new().fg(GREEN)),
        Some("working") => Span::styled("◐", Style::new().fg(BLUE)),
        Some(_) => Span::styled("○", Style::new().fg(MUTED)),
        None => Span::styled("·", Style::new().fg(MUTED)),
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

impl App {
    fn selected_item(&self) -> Option<&Item> {
        self.items.get(self.selected)
    }

    fn agent_output(&mut self, pane: &str) -> Vec<String> {
        if let Some((at, lines)) = self.output.get(pane)
            && at.elapsed() < Duration::from_secs(4)
        {
            return lines.clone();
        }
        let lines: Vec<String> = herdr::read_agent(pane, 80).unwrap_or_default().lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty() && !line.contains("──") && !line.contains("╌") && !line.trim_start().starts_with('❯'))
            .map(str::to_string)
            .collect();
        let tail = lines[lines.len().saturating_sub(16)..].to_vec();
        self.output.insert(pane.to_string(), (Instant::now(), tail.clone()));
        tail
    }

    fn repo_for_start(&self) -> PathBuf {
        self.selected_item().and_then(|item| item.repo.clone())
            .or_else(|| self.items.iter().find_map(|item| item.repo.clone()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    }

    fn show(&mut self, result: Result<String>) {
        self.mode = Mode::Message(result.unwrap_or_else(|error| error));
        let _ = self.refresh.send(());
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
        let item = self.selected_item().cloned();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(self.items.len().saturating_sub(1)),
            KeyCode::Esc | KeyCode::Char('q') if self.overlay => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('r') => {
                let _ = self.refresh.send(());
            }
            KeyCode::Char('s') => {
                self.mode = Mode::Input { purpose: Purpose::TaskName, label: "New task: name".to_string(), buffer: String::new() };
            }
            KeyCode::Enter => {
                if let Some(workspace) = item.and_then(|item| item.workspace) {
                    let _ = herdr::focus_workspace(&workspace);
                    if self.overlay {
                        self.quit = true;
                    }
                }
            }
            KeyCode::Char('p') => {
                if let Some(item) = item
                    && let Some(agent) = item.agent
                {
                    let label = format!("Message to {}", item.name);
                    self.mode = Mode::Input { purpose: Purpose::Message(agent.pane, item.name), label, buffer: String::new() };
                }
            }
            KeyCode::Char(command @ ('l' | 'd' | 'x')) => {
                let Some(item) = item else { return };
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

    fn submit(&mut self, purpose: Purpose, text: String, terminal: &mut DefaultTerminal) {
        match purpose {
            Purpose::TaskName if text.is_empty() => {}
            Purpose::TaskName => {
                self.mode = Mode::Input {
                    purpose: Purpose::TaskPrompt(text),
                    label: "What should Claude do? (optional)".to_string(),
                    buffer: String::new(),
                };
            }
            Purpose::TaskPrompt(name) => {
                let repo = self.repo_for_start();
                let prompt = (!text.is_empty()).then_some(text);
                let result = outside(terminal, || tasks::start(&repo, &name, prompt.as_deref(), true));
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

fn header(app: &App) -> Line<'static> {
    let tasks = app.items.iter().filter(|item| item.is_task()).count();
    let needs = app.items.iter().filter(|item| item.group() == Group::NeedsYou).count();
    let working = app.items.iter().filter(|item| item.group() == Group::Working).count();
    let mut spans = vec![Span::styled(" kelpie ", Style::new().fg(MAUVE).add_modifier(Modifier::BOLD))];
    let mut part = |text: String, color: Color| {
        spans.push(Span::styled(" · ", Style::new().fg(MUTED)));
        spans.push(Span::styled(text, Style::new().fg(color)));
    };
    part(format!("{tasks} {}", if tasks == 1 { "task" } else { "tasks" }), SUBTEXT);
    if needs > 0 {
        part(format!("{needs} need you"), PEACH);
    }
    if working > 0 {
        part(format!("{working} working"), BLUE);
    }
    Line::from(spans)
}

fn list_lines(app: &App, width: u16) -> (Vec<Line<'static>>, usize) {
    let mut lines = Vec::new();
    let mut selected_line = 0;
    let mut current: Option<Group> = None;
    for (index, item) in app.items.iter().enumerate() {
        let group = item.group();
        if current != Some(group) {
            if current.is_some() {
                lines.push(Line::default());
            }
            let count = app.items.iter().filter(|other| other.group() == group).count();
            lines.push(Line::from(vec![
                Span::styled(format!(" {}", group.title()), Style::new().fg(group_color(group)).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {count}"), Style::new().fg(MUTED)),
            ]));
            current = Some(group);
        }
        let selected = index == app.selected;
        if selected {
            selected_line = lines.len();
        }
        let background = if selected { Style::new().bg(SURFACE) } else { Style::new() };
        let bar = if selected { Span::styled("▌", Style::new().fg(MAUVE)) } else { Span::raw(" ") };
        let name_style = if selected { Style::new().fg(TEXT).add_modifier(Modifier::BOLD) } else { Style::new().fg(TEXT) };
        let place = format!("{} ", item.place);
        let used = 4 + item.name.chars().count() + place.chars().count();
        let gap = (width as usize).saturating_sub(used).max(1);
        lines.push(Line::from(vec![
            bar.clone(), Span::raw(" "), status_icon(item), Span::raw(" "),
            Span::styled(item.name.clone(), name_style),
            Span::raw(" ".repeat(gap)),
            Span::styled(place, Style::new().fg(MUTED)),
        ]).style(background));
        lines.push(Line::from(vec![
            bar, Span::raw("   "),
            Span::styled(item.summary(), Style::new().fg(SUBTEXT)),
        ]).style(background));
    }
    (lines, selected_line)
}

fn detail_lines(app: &mut App) -> Vec<Line<'static>> {
    let Some(item) = app.selected_item().cloned() else { return Vec::new() };
    let mut lines = vec![Line::from(Span::styled(
        match &item.path {
            Some(path) => format!("task in {} · {}", item.place, path.file_name().unwrap_or_default().to_string_lossy()),
            None => format!("session in {}", item.place),
        },
        Style::new().fg(SUBTEXT),
    ))];
    if item.is_task() {
        lines.push(Line::from(Span::styled(item.summary(), Style::new().fg(TEXT))));
    }
    if let Some(agent) = &item.agent {
        let mut spans = vec![status_icon(&item), Span::raw(" "), Span::styled(status_words(&agent.status), Style::new().fg(TEXT))];
        if let Some(context) = &agent.context {
            spans.push(Span::styled(format!("  ·  {context} context"), Style::new().fg(MUTED)));
        }
        lines.push(Line::from(spans));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Last from the agent", Style::new().fg(MUTED).add_modifier(Modifier::BOLD))));
        let pane = agent.pane.clone();
        for line in app.agent_output(&pane) {
            lines.push(Line::from(Span::styled(line, Style::new().fg(SUBTEXT))));
        }
    } else {
        lines.push(Line::from(Span::styled("No agent running in its Space.", Style::new().fg(MUTED))));
    }
    lines
}

fn footer(app: &App) -> Line<'static> {
    let mut keys = vec![("⏎", "go"), ("s", "start"), ("p", "message"), ("l", "ready to push"), ("d", "done"), ("x", "drop"), ("?", "help")];
    if app.overlay {
        keys.push(("esc", "close"));
    }
    Line::from(keys.into_iter().flat_map(|(key, label)| [
        Span::styled(format!(" {key}"), Style::new().fg(MAUVE)),
        Span::styled(format!(" {label} "), Style::new().fg(MUTED)),
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
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), Style::new().fg(MAUVE).add_modifier(Modifier::BOLD)));
    frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), area);
}

fn help_lines() -> Vec<Line<'static>> {
    [
        ("⏎", "go to the task's or session's Space"), ("j k ↑ ↓", "move"), ("s", "start a task in a free worktree"),
        ("p", "send the agent a message without leaving"), ("l", "rebase and check; shows the push command"),
        ("d", "after you pushed: close the task, free its worktree"), ("x", "drop the task (asks first)"),
        ("r", "refresh now"), ("esc", "close this, or the board when it is an overlay"),
    ].into_iter().map(|(key, text)| Line::from(vec![
        Span::styled(format!(" {key:<9}"), Style::new().fg(MAUVE)),
        Span::styled(text.to_string(), Style::new().fg(TEXT)),
    ])).collect()
}

fn draw(frame: &mut Frame, app: &mut App) {
    let [top, body, bottom] = Layout::vertical([Constraint::Length(2), Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
    frame.render_widget(Paragraph::new(header(app)), top);
    frame.render_widget(Paragraph::new(footer(app)), bottom);

    let wide = body.width >= 90;
    let [list_area, detail_area] = if wide {
        Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)]).areas(body)
    } else {
        [body, Rect::default()]
    };

    if let Some(error) = &app.error {
        frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" {error}"), Style::new().fg(RED)))).wrap(Wrap { trim: false }), list_area);
    } else if !app.loaded {
        frame.render_widget(Paragraph::new(Span::styled(" looking around…", Style::new().fg(MUTED))), list_area);
    } else if app.items.is_empty() {
        frame.render_widget(Paragraph::new(vec![
            Line::from(Span::styled(" Nothing running yet.", Style::new().fg(SUBTEXT))),
            Line::from(Span::styled(" Press s to start a task.", Style::new().fg(MUTED))),
        ]), list_area);
    } else {
        let (lines, selected_line) = list_lines(app, list_area.width.saturating_sub(1));
        let scroll = selected_line.saturating_sub(list_area.height.saturating_sub(3) as usize) as u16;
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), list_area);
    }

    if wide && let Some(item) = app.selected_item().cloned() {
        let title = Span::styled(format!(" {} ", item.name), Style::new().fg(MAUVE).add_modifier(Modifier::BOLD));
        let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(MUTED)).title(title).padding(Padding::horizontal(1));
        let lines = detail_lines(app);
        let inner_height = detail_area.height.saturating_sub(2) as usize;
        let scroll = lines.len().saturating_sub(inner_height) as u16;
        frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }).scroll((scroll, 0)), detail_area);
    }

    match &app.mode {
        Mode::Normal => {}
        Mode::Help => modal(frame, "keys", help_lines(), 64),
        Mode::Message(text) => {
            let mut lines: Vec<Line> = text.lines().map(|line| Line::from(Span::styled(line.to_string(), Style::new().fg(TEXT)))).collect();
            lines.push(Line::from(Span::styled("any key to close", Style::new().fg(MUTED))));
            modal(frame, "kelpie", lines, 96);
        }
        Mode::ConfirmDrop { name, force, .. } => {
            let question = if *force { "Discard its changes and commits?" } else { "Drop this task?" };
            modal(frame, "drop", vec![
                Line::from(Span::styled(name.clone(), Style::new().fg(TEXT))),
                Line::default(),
                Line::from(vec![Span::styled(question, Style::new().fg(YELLOW)), Span::styled("  y / n", Style::new().fg(MUTED))]),
            ], 72);
        }
        Mode::Input { label, buffer, .. } => modal(frame, label, vec![
            Line::from(vec![Span::styled(buffer.clone(), Style::new().fg(TEXT)), Span::styled("▏", Style::new().fg(MAUVE))]),
            Line::from(Span::styled("enter to continue · esc to cancel", Style::new().fg(MUTED))),
        ], 72),
    }
}

pub fn run(overlay: bool) -> Result<String> {
    let (items_rx, refresh) = spawn_refresher();
    let mut app = App {
        items: Vec::new(), selected: 0, mode: Mode::Normal, overlay, loaded: false, error: None,
        output: HashMap::new(), refresh, quit: false,
    };
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        while !app.quit {
            while let Ok(update) = items_rx.try_recv() {
                match update {
                    Ok(items) => {
                        let previous = app.selected_item().map(|item| (item.name.clone(), item.place.clone()));
                        app.items = items;
                        app.selected = previous
                            .and_then(|(name, place)| app.items.iter().position(|item| item.name == name && item.place == place))
                            .unwrap_or(app.selected.min(app.items.len().saturating_sub(1)));
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
