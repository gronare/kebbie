use std::time::{Duration, SystemTime};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::board::widgets::{FAINT, MAUVE, MUTED, PEACH, SUBTEXT, SURFACE, TEAL, TEXT, YELLOW, bold, style};
use crate::sh::Result;
use super::{Message, flush, is_copy, load, now};

// What sessions told each other, the questions and the user's answers, and the task reports that wait on the user.
pub(super) fn shown_in_log(messages: Vec<Message>) -> Vec<Message> {
    messages.into_iter().filter(|message| !is_copy(message) && message.report.as_ref().is_none_or(|report| report.state != "working")).collect()
}

pub fn recent(count: usize) -> Vec<Message> {
    let messages = shown_in_log(load());
    messages[messages.len().saturating_sub(count)..].to_vec()
}

pub fn addressee(message: &Message) -> String {
    if message.question.is_some() {
        return "you · asks".to_string();
    }
    match &message.report {
        Some(report) => format!("you · {}", report.state.replace('-', " ")),
        None => message.to.clone(),
    }
}

pub fn age(at: u64) -> String {
    let seconds = now().saturating_sub(at);
    match seconds {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

pub fn held_note(message: &Message) -> Option<String> {
    if message.forced {
        return Some("forced".to_string());
    }
    message.held.then(|| message.why.as_ref().map_or("held".to_string(), |why| format!("held: {why}")))
}

pub fn print(count: usize) -> String {
    print_lines(&recent(count))
}

pub(super) fn print_lines(messages: &[Message]) -> String {
    if messages.is_empty() {
        return "No messages yet.".to_string();
    }
    messages.iter().map(|message| {
        let held = held_note(message).map(|note| format!("  ({note})")).unwrap_or_default();
        let next = message.next_for_you().map(|next| format!("\n      you: {next}")).unwrap_or_default();
        format!("{:>4}  {} → {}{held}\n      {}{next}", age(message.at), message.from, addressee(message), message.text)
    }).collect::<Vec<_>>().join("\n")
}

pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

fn message_lines(message: &Message, width: usize) -> Vec<Line<'static>> {
    let mut head = vec![
        Span::styled(format!("{:>4} ", age(message.at)), style(FAINT)),
        Span::styled(message.from.clone(), style(TEAL)),
        Span::styled(" → ", style(MUTED)),
        Span::styled(addressee(message), style(if message.report.is_some() { PEACH } else { TEAL })),
    ];
    if let Some(note) = held_note(message) {
        head.push(Span::styled(format!("  {note}"), style(PEACH)));
    }
    let mut lines = vec![Line::from(head)];
    lines.extend(wrap(&message.text, width.saturating_sub(5).max(10)).into_iter().map(|text| Line::from(vec![Span::raw("     "), Span::styled(text, style(SUBTEXT))])));
    if let Some(next) = message.next_for_you() {
        lines.extend(wrap(&format!("you: {next}"), width.saturating_sub(5).max(10)).into_iter().map(|text| Line::from(vec![Span::raw("     "), Span::styled(text, style(YELLOW))])));
    }
    lines.push(Line::default());
    lines
}

fn draw(frame: &mut Frame, messages: &[Message], scroll: usize) {
    let [title, rule, body] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)]).areas(frame.area());
    let width = body.width as usize;
    let held = messages.iter().filter(|message| message.held).count();
    let right = if held > 0 { format!("{held} held · {}", messages.len()) } else { messages.len().to_string() };
    let gap = width.saturating_sub(8 + right.chars().count()).max(1);
    frame.render_widget(Paragraph::new(Line::from(vec![
        Span::styled("MESSAGES", bold(MAUVE)), Span::raw(" ".repeat(gap)), Span::styled(right, style(if held > 0 { PEACH } else { MUTED })),
    ])), title);
    frame.render_widget(Paragraph::new(Line::from(Span::styled("─".repeat(width), style(SURFACE)))), rule);
    let mut lines: Vec<Line> = messages.iter().flat_map(|message| message_lines(message, width)).collect();
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("Sessions' messages to each other show here,", style(MUTED))));
        lines.push(Line::from(Span::styled("and task reports that wait on you.", style(MUTED))));
    }
    let height = body.height as usize;
    let bottom = lines.len().saturating_sub(height);
    let top = bottom.saturating_sub(scroll);
    let shown: Vec<Line> = lines.into_iter().skip(top).take(height).collect();
    frame.render_widget(Paragraph::new(shown).style(style(TEXT)), body);
}

// The log, newest at the bottom; ↑ ↓ scroll back and forth, and the pane delivers held messages as sessions free up.
pub fn run() -> Result<String> {
    let mut terminal = ratatui::init();
    let mut scroll = 0usize;
    let mut flushed = SystemTime::UNIX_EPOCH;
    let result = (|| -> Result<()> {
        loop {
            if flushed.elapsed().unwrap_or_default() > Duration::from_secs(2) {
                let _ = flush();
                crate::typing::check();
                flushed = SystemTime::now();
            }
            let messages = shown_in_log(load());
            terminal.draw(|frame| draw(frame, &messages, scroll)).map_err(|error| error.to_string())?;
            if event::poll(Duration::from_millis(500)).map_err(|error| error.to_string())?
                && let Event::Key(key) = event::read().map_err(|error| error.to_string())?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Up | KeyCode::Char('k') => scroll += 1,
                    KeyCode::Down | KeyCode::Char('j') => scroll = scroll.saturating_sub(1),
                    KeyCode::End | KeyCode::Char('G') => scroll = 0,
                    _ => {}
                }
            }
        }
    })();
    ratatui::restore();
    result.map(|_| String::new())
}
