use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::keys::{self, Act};
use crate::messages::{self, Message};
use crate::lead;
use super::{App, Mode};
use super::status_bar::clock;
use super::widgets::{MAUVE, MUTED, SUBTEXT, SURFACE, TEAL, YELLOW, bold, fit, style};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Fold {
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

impl App {
    fn set_fold(&mut self, fold: Fold) {
        self.fold = fold;
        self.in_messages &= fold == Fold::Open;
        if fold == Fold::Open {
            self.seen = self.seen.max(self.messages.last().map_or(0, |message| message.at));
        }
        save_fold(self.fold_file.as_deref(), fold, self.seen);
    }

    pub(super) fn press_m(&mut self) {
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

    pub(super) fn selected_message(&self) -> Option<&Message> {
        self.messages.len().checked_sub(1 + self.message_back).and_then(|index| self.messages.get(index))
    }

    pub(super) fn handle_messages(&mut self, key: KeyEvent) {
        match keys::map().act(self.ctx(), &key) {
            Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => self.message_back = (self.message_back + 1).min(self.messages.len().saturating_sub(1)),
            Some(Act::Move) => self.message_back = self.message_back.saturating_sub(1),
            Some(Act::Back | Act::Close) => self.in_messages = false,
            Some(Act::Messages) => self.press_m(),
            Some(Act::Help) => self.mode = Mode::Help(Default::default()),
            Some(Act::Inbox) => self.open_questions(None),
            Some(Act::Options) => {
                if let Some(text) = self.selected_message().map(|message| format!("{} {} → {}\n\n{}", clock(message.at), message.from, message.to, message.text)) {
                    self.shown = Some(super::notices::Subject::plain("message"));
                    self.mode = Mode::Message(text);
                }
            }
            _ => {}
        }
    }
}

fn message_text(message: &Message) -> String {
    message.text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn messages_height(fold: Fold, height: u16) -> u16 {
    match fold {
        Fold::Hidden => 0,
        Fold::Line => 1,
        Fold::Open => (height / 3).max(4),
    }
}

// The second value is the index of the message on the panel's second line, which the mouse hits are counted from.
pub(super) fn messages_lines(app: &App, fold: Fold, rows: usize, width: usize) -> (Vec<Line<'static>>, usize) {
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

pub(super) fn fold_file() -> Option<PathBuf> {
    Some(lead::state_dir().ok()?.join("board-messages"))
}

pub(super) fn load_fold(file: Option<&Path>, height: u16) -> (Fold, u64) {
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

pub(super) fn save_fold(file: Option<&Path>, fold: Fold, seen: u64) {
    if let Some(file) = file {
        let _ = crate::state::write(file, format!("{}\t{seen}\n", fold.name()));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;

    use super::*;
    use crate::board::test_support::*;
    use crate::messages;
    use crate::board::Mode;
    use crate::board::widgets::SURFACE;

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
}
