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
            KeyCode::Char('y') => {
                self.about("reset", &name);
                self.in_background("Resetting the task…".to_string(), move || tasks::reset(&path, None));
            }
            _ if cancels => {}
            _ => self.mode = Mode::ConfirmReset { path, name, nudged: true },
        }
    }

    pub(super) fn confirm_done(&mut self, path: PathBuf, name: String, key: KeyEvent, cancels: bool) {
        match key.code {
            KeyCode::Char('y') => {
                self.about("done", &name);
                self.in_background(format!("Closing {name}…"), move || tasks::done(&path, None));
            }
            _ if cancels => {}
            _ => self.mode = Mode::ConfirmDone { path, name, nudged: true },
        }
    }

    pub(super) fn confirm_queued(&mut self, name: String, about: String, now: bool, key: KeyEvent, cancels: bool) {
        match key.code {
            KeyCode::Char('y') if now => {
                self.about("start", &name);
                self.in_background(format!("Starting {name}…"), move || tasks::start_now(&name));
            }
            KeyCode::Char('y') => {
                self.about("queued", &name);
                self.show(tasks::drop_wait(&name));
            }
            _ if cancels => {}
            _ => self.mode = Mode::ConfirmQueued { name, about, now, nudged: true },
        }
    }

    pub(super) fn confirm_drop(&mut self, path: PathBuf, name: String, force: bool, key: KeyEvent, cancels: bool) {
        match key.code {
            KeyCode::Char('y') => match tasks::drop(&path, None, force) {
                Err(error) if !force && error.contains(tasks::FORCE) => {
                    let said = crate::notice::Notice::read(&error, true).headline;
                    self.mode = Mode::ConfirmDrop { path, name: format!("{name}: {said}"), force: true, nudged: false };
                }
                result => {
                    self.about("drop", &name);
                    self.show(result);
                }
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

pub(super) fn draw_queued(frame: &mut Frame, name: &str, about: &str, now: bool, nudged: bool) {
    let (title, question) = if now { ("start now", "Start it now?") } else { ("drop the wait", "Drop the wait?") };
    modal(frame, title.to_string(), confirm_lines(name, Some(about), question, nudged), 72);
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

    #[test]
    fn a_queued_task_starts_now_on_enter_and_drops_its_wait_on_the_drop_key_after_asking() {
        let mut item = crate::status::tests::task(None, None, 0);
        (item.name, item.git, item.title) = ("web-billing".to_string(), None, Some("Bill in cents".to_string()));
        let waiting = "Waits for Billing totals (ready to push). Starts by itself once it is pushed and CI is green.";
        item.queued = Some(crate::status::Queued { after: vec![("billing@api".to_string(), "ready to push".to_string())], blocked: None, waiting: waiting.to_string(), names: vec!["Billing totals".to_string()], base: "main".to_string(), at: None, backlog: true });
        let mut app = test_app(vec![item]);
        let (rows, _) = render(&mut app, 200, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("QUEUED") && screen.contains("⧗ Bill in cents") && !screen.contains("billing@api"), "the row has its title and names the other task by its own: {screen}");
        assert!(screen.contains("   Waits for Billing totals (ready to push). Starts by itself") && screen.contains("   once it is pushed and CI is green.") && !screen.contains('…'), "the wait is said whole under the row, wrapped, never cut: {screen}");
        let menu: Vec<_> = crate::board::options::task_choices(app.selected_task().as_ref(), None, None).into_iter().map(|choice| (choice.act, choice.about)).collect();
        assert_eq!(menu, [
            (Some(crate::keys::Act::Go), "from main, without the changes from Billing totals; its Claude is told what is missing (asks first)".to_string()),
            (Some(crate::keys::Act::StartAt), "type a time and it starts then, once what it waits for is done too".to_string()),
            (Some(crate::keys::Act::Drop), "it never starts, and its backlog item goes back to the backlog (asks first)".to_string()),
        ]);
        app.handle_normal(key(KeyCode::Char('g')));
        assert!(matches!(&app.mode, Mode::ConfirmQueued { now: true, nudged: false, name, about } if name == "web-billing" && about.starts_with("Waits for Billing totals (ready to push). Starts") && about.ends_with("Started now, it begins from main, without the changes from Billing totals; its Claude is told what is missing.")), "go asks before it starts the task early");
        let (rows, _) = render(&mut app, 200, 30);
        assert!(rows.join("\n").contains("Start it now?"));
        press(&mut app, key(KeyCode::Char('n')));
        assert!(matches!(app.mode, Mode::Normal));
        app.handle_normal(key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::ConfirmQueued { now: false, .. }), "the drop key asks to drop the wait");
        press(&mut app, key(KeyCode::Esc));
        app.handle_normal(key(KeyCode::Char('p')));
        assert!(matches!(app.mode, Mode::Normal), "a task's keys a queued row does not offer do nothing");
        app.handle_normal(key(KeyCode::Char('s')));
        assert!(app.toast.as_ref().is_some_and(|toast| toast.notice.headline == "It is queued and has no worktree yet"), "the card key, offered on every row, says what a queued row takes");
    }

    #[test]
    fn l_sets_when_a_queued_task_or_a_backlog_item_starts() {
        use crate::board::input::{Purpose, START_AT_LABEL};
        let mut item = crate::status::tests::task(None, None, 0);
        (item.name, item.git) = ("nightly".to_string(), None);
        item.queued = Some(crate::status::Queued { after: Vec::new(), blocked: None, waiting: "Starts 02:00 tomorrow.".to_string(), names: Vec::new(), base: "main".to_string(), at: Some("02:00 tomorrow".to_string()), backlog: false });
        let mut app = test_app(vec![item]);
        let menu: Vec<_> = crate::board::options::task_choices(app.selected_task().as_ref(), None, None).into_iter().map(|choice| (choice.label, choice.about)).collect();
        assert_eq!(menu, [
            ("Start it now".to_string(), "from main now instead of 02:00 tomorrow (asks first)".to_string()),
            ("Start at…".to_string(), "now 02:00 tomorrow; type another time and it starts then".to_string()),
            ("Drop the wait".to_string(), "it never starts and leaves the board (asks first)".to_string()),
        ], "the options box says when it starts");
        app.handle_normal(key(KeyCode::Char('l')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::StartAt(name), label, .. } if name == "nightly" && label == START_AT_LABEL));
        for character in "soonish".chars() {
            press(&mut app, key(KeyCode::Char(character)));
        }
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::Message(text) if text.contains("\"soonish\" is not a time")), "a time kelpie cannot read is refused on the board");
        app.mode = Mode::Normal;
        app.column = crate::board::Column::Backlog;
        app.backlog.entries = vec![crate::backlog::Entry { source: "vault".to_string(), title: "Nightly import".to_string(), priority: "high".to_string(), area: "web".to_string(), reference: "Projects/nightly.md".to_string(), summary: String::new(), age: String::new() }];
        app.backlog_selected = 0;
        app.handle_normal(key(KeyCode::Char('l')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogLaterName(entry), buffer, .. } if entry.reference == "Projects/nightly.md" && buffer == "nightly-import"));
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogLaterAt(_, name), label, .. } if name == "nightly-import" && label == START_AT_LABEL), "after its name, the time");
    }
}
