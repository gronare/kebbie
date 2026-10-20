use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::tasks;
use super::{App, Mode};
use super::widgets::{MUTED, PEACH, TEXT, YELLOW, modal, style};

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

impl App {
    pub(super) fn confirm_reset(&mut self, path: PathBuf, name: String, key: KeyEvent, cancels: bool) {
        match key.code {
            KeyCode::Char('y') => self.in_background("Resetting the task…".to_string(), move || tasks::reset(&path, None)),
            _ if cancels => {}
            _ => self.mode = Mode::ConfirmReset { path, name, nudged: true },
        }
    }

    pub(super) fn confirm_done(&mut self, path: PathBuf, name: String, key: KeyEvent, cancels: bool) {
        match key.code {
            KeyCode::Char('y') => self.in_background(format!("Closing {name}…"), move || tasks::done(&path, None)),
            _ if cancels => {}
            _ => self.mode = Mode::ConfirmDone { path, name, nudged: true },
        }
    }

    pub(super) fn confirm_drop(&mut self, path: PathBuf, name: String, force: bool, key: KeyEvent, cancels: bool) {
        match key.code {
            KeyCode::Char('y') => match tasks::drop(&path, None, force) {
                Err(error) if !force && error.contains("--force") => {
                    self.mode = Mode::ConfirmDrop { path, name: format!("{name}: {error}"), force: true, nudged: false };
                }
                result => self.show(result),
            },
            _ if cancels => {}
            _ => self.mode = Mode::ConfirmDrop { path, name, force, nudged: true },
        }
    }
}

pub(super) fn draw_reset(frame: &mut Frame, name: &str, nudged: bool) {
    modal(frame, "reset".to_string(), confirm_lines(name, Some("Its changes and commits go, its Claude stops, its worktree is freed and its Space closes. The backlog item it came from returns."), "Reset this task?", nudged), 72);
}

pub(super) fn draw_done(frame: &mut Frame, name: &str, nudged: bool) {
    modal(frame, "done".to_string(), confirm_lines(name, Some("Its commits are on the default branch already: its worktree is freed and its Space closes."), "Close this task as done?", nudged), 72);
}

pub(super) fn draw_drop(frame: &mut Frame, name: &str, force: bool, nudged: bool) {
    let question = if force { "Discard its changes and commits?" } else { "Drop this task?" };
    modal(frame, "drop".to_string(), confirm_lines(name, None, question, nudged), 72);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::crossterm::event::KeyCode;

    use crate::board::test_support::*;
    use crate::board::Mode;

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
}
