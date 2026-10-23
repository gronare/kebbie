use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::keys::{self, Act};
use super::options::ongoing_choices;
use super::widgets::{MUTED, PEACH, SUBTEXT, TEAL, TEXT, bold, modal, style, wrap};
use super::{App, Mode, Ongoing};

const PAGE: u16 = 10;
const FRAME_ROWS: u16 = 8;
const LABEL: usize = 10;

// A task note opens with Problem and Lösning, so the preamble and the first two sections are what the card shows.
pub(super) fn opening(body: &str) -> String {
    let mut sections = 0;
    let mut kept = Vec::new();
    for line in body.lines() {
        if line.starts_with("# ") {
            continue;
        }
        if line.starts_with("## ") {
            sections += 1;
            if sections > 2 {
                break;
            }
        }
        kept.push(line);
    }
    kept.join("\n").trim().to_string()
}

fn fact(label: &str, value: &str, width: usize, lines: &mut Vec<Line<'static>>) {
    if value.trim().is_empty() {
        return;
    }
    for (index, chunk) in wrap(value, width.saturating_sub(LABEL), usize::MAX).into_iter().enumerate() {
        let label = if index == 0 { format!("{label:<LABEL$}") } else { " ".repeat(LABEL) };
        lines.push(Line::from(vec![Span::styled(label, style(MUTED)), Span::styled(chunk, style(TEXT))]));
    }
}

pub(super) fn note_lines(app: &App, note: &Ongoing, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = wrap(&note.title, width, usize::MAX).into_iter().map(|chunk| Line::from(Span::styled(chunk, bold(TEXT)))).collect();
    lines.push(Line::default());
    fact("area", &note.area, width, &mut lines);
    fact("status", &note.status, width, &mut lines);
    fact("summary", &note.summary, width, &mut lines);
    if let Some(claim) = &note.claim {
        let since = claim.since.get(..10).unwrap_or(&claim.since);
        fact("works on", &format!("{} · {} · since {since}", claim.machine, claim.task), width, &mut lines);
    }
    let running = note.task.as_ref().map(|task| match app.tasks.iter().any(|item| &crate::peers::address(&item.name, &item.place) == task) {
        true => format!("{task} · runs here"),
        false => task.clone(),
    });
    fact("task", running.as_deref().unwrap_or("none"), width, &mut lines);
    fact("note", &format!("vault: {}", note.key), width, &mut lines);
    if !note.opening.is_empty() {
        lines.push(Line::default());
        for line in note.opening.lines() {
            if let Some(heading) = line.strip_prefix("## ") {
                lines.push(Line::from(Span::styled(heading.to_string(), bold(TEAL))));
            } else if line.trim().is_empty() {
                lines.push(Line::default());
            } else {
                lines.extend(wrap(line, width, usize::MAX).into_iter().map(|chunk| Line::from(Span::styled(chunk, style(SUBTEXT)))));
            }
        }
    }
    lines
}

impl App {
    pub(super) fn open_note_card(&mut self) -> bool {
        let Some(note) = self.selected_ongoing() else { return false };
        self.mode = Mode::NoteCard { key: note.key, scroll: 0, nudged: false };
        true
    }

    pub(super) fn handle_note_card(&mut self, key: String, scroll: u16, event: KeyEvent, cancels: bool) {
        let again = |scroll: u16| Mode::NoteCard { key: key.clone(), scroll, nudged: false };
        match (event.code, keys::map().act(self.ctx(), &event)) {
            (_, Some(Act::Go | Act::Options)) => self.act(Act::Go),
            (KeyCode::Down | KeyCode::Char('j'), _) => self.mode = again(scroll.saturating_add(1)),
            (KeyCode::Up | KeyCode::Char('k'), _) => self.mode = again(scroll.saturating_sub(1)),
            (KeyCode::PageDown | KeyCode::Char(' '), _) => self.mode = again(scroll.saturating_add(PAGE)),
            (KeyCode::PageUp, _) => self.mode = again(scroll.saturating_sub(PAGE)),
            _ if cancels => {}
            _ => self.mode = Mode::NoteCard { key, scroll, nudged: true },
        }
    }
}

// Returns the scroll it drew at, held to the last screenful.
pub(super) fn draw_note_card(frame: &mut Frame, app: &App, key: &str, scroll: u16, nudged: bool) -> u16 {
    let Some(note) = app.ongoing.iter().find(|note| note.key == key) else { return scroll };
    let width = 88u16.min(frame.area().width.saturating_sub(4));
    let rows = note_lines(app, note, usize::from(width.saturating_sub(4)).max(20));
    let view = usize::from(frame.area().height.saturating_sub(FRAME_ROWS)).max(3).min(rows.len());
    let last = rows.len().saturating_sub(view);
    let scroll = usize::from(scroll).min(last);
    let mut lines: Vec<Line<'static>> = rows.into_iter().skip(scroll).take(view).collect();
    lines.push(Line::default());
    let go = ongoing_choices(note).first().map_or(String::new(), |choice| format!("{} {} · ", keys::key(Act::Go), choice.label.to_lowercase()));
    let more = if last > 0 { "↓↑ pgdn pgup scroll · " } else { "" };
    lines.push(Line::from(Span::styled(format!("{go}{more}esc back"), style(MUTED))));
    if nudged {
        lines.push(Line::from(Span::styled(format!("press {} or esc", keys::key(Act::Go)), style(PEACH))));
    }
    modal(frame, "ongoing note".to_string(), lines, width);
    scroll as u16
}
