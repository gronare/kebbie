use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use ratatui::style::Style;
use ratatui::Frame;

use crate::notice::{Notice, Tone};
use crate::sh::Result;
use crate::status::Item;
use crate::tasks;
use super::notices::{self, Subject, tone_color};
use super::widgets::{MANTLE, MUTED, TEXT, bold, cut, style};
use super::{App, Mode, Page};

pub(super) type Pusher = Arc<dyn Fn(&Path, Option<&str>, bool) -> Result<String> + Send + Sync>;

pub(super) fn live_pusher() -> Pusher {
    Arc::new(|path, through, keep_open| tasks::push(path, None, through, keep_open))
}

// The step a push is at: the task it pushes now and what it does there.
type Step = Arc<Mutex<Option<(String, String)>>>;

// A push this board runs in the background, for one task or several in order.
pub(super) struct Pushing {
    pub names: Vec<String>,
    pub paths: Vec<PathBuf>,
    pub label: String,
    step: Step,
    subject: Subject,
    result: Receiver<Result<String>>,
}

impl Pushing {
    fn step(&self) -> Option<(String, String)> {
        self.step.lock().ok().and_then(|step| step.clone())
    }
}

impl App {
    pub(super) fn push_in_background(&mut self, label: String, tasks: Vec<(String, PathBuf)>, run: impl FnOnce(&Pusher) -> Result<String> + Send + 'static) {
        let step: Step = Arc::default();
        let (sender, result) = mpsc::channel();
        let (into, pusher) = (step.clone(), self.pusher.clone());
        thread::spawn(move || {
            let out = tasks::push_watched(move |task, what| {
                if let Ok(mut step) = into.lock() {
                    *step = Some((task.to_string(), what.to_string()));
                }
            }, || run(&pusher));
            let _ = sender.send(out);
        });
        let subject = match tasks.as_slice() {
            [(name, _)] => self.subject_for("push", name),
            _ => Subject::plain("push"),
        };
        let (names, paths) = tasks.into_iter().unzip();
        self.pushes.push(Pushing { names, paths, label, step, subject, result });
        self.mode = Mode::Normal;
        let _ = self.refresh.send(());
    }

    pub(super) fn pushing(&self, path: &Path) -> bool {
        self.pushes.iter().any(|push| push.paths.iter().any(|pushed| pushed == path))
    }

    // An outcome that asks nothing goes by as a toast at once; one that needs the user waits until the board is at rest on its work page, so it never covers a window the user has open.
    pub(super) fn poll_pushes(&mut self) {
        let mut at = 0;
        while at < self.pushes.len() {
            match self.pushes[at].result.try_recv() {
                Ok(result) => {
                    let push = self.pushes.remove(at);
                    let failed = result.is_err();
                    let notice = Notice::read(&result.unwrap_or_else(|error| error), failed);
                    if notice.tone.passes() && notice.you.is_none() {
                        self.toast = Some(notices::Toast { notice, subject: Some(push.subject), at: Instant::now() });
                    } else {
                        self.push_outcomes.push((notice, push.subject));
                    }
                    let _ = self.refresh.send(());
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let push = self.pushes.remove(at);
                    self.push_outcomes.push((Notice::new(Tone::Failed, format!("{} stopped without an answer", push.label)).next("Look at its row: the push line shows where it stands."), push.subject));
                }
                Err(mpsc::TryRecvError::Empty) => at += 1,
            }
        }
        if !self.push_outcomes.is_empty() && self.page == Page::Work && matches!(self.mode, Mode::Normal) {
            let (notice, subject) = self.push_outcomes.remove(0);
            self.shown = Some(subject);
            self.mode = Mode::Message(notice.to_string());
        }
    }

    // What a task's row says while this board pushes it: the step for the task the push is at, plain for one still waiting its turn in the same push.
    pub(super) fn push_note_for(&self, item: &Item) -> Option<String> {
        let path = item.path.as_deref()?;
        let push = self.pushes.iter().find(|push| push.paths.iter().any(|pushed| pushed == path))?;
        Some(match push.step() {
            Some((task, what)) if task == item.name => format!("pushing… {what}"),
            _ => "pushing…".to_string(),
        })
    }

    fn push_line(&self, push: &Pushing) -> String {
        let title = |name: &str| self.tasks.iter().find(|item| item.name == name).map_or_else(|| name.to_string(), Item::titled);
        match (push.step(), push.names.len()) {
            (Some((_, what)), 1) => format!("{}… {what}", push.label),
            (Some((task, what)), _) => format!("{}… {}: {what}", push.label, title(&task)),
            (None, _) => format!("{}…", push.label),
        }
    }

    fn subject_for(&self, kind: &str, name: &str) -> Subject {
        self.tasks.iter().find(|item| item.name == name).map_or_else(|| Subject { kind: kind.to_string(), title: name.to_string(), task: true }, |item| Subject::of(kind, item))
    }

    pub(super) fn push_label(&self, name: &str) -> String {
        format!("Pushing {}", self.subject_for("push", name).title)
    }
}

// One line per push under way, bottom right where toasts show, above a toast that shows at the same time.
pub(super) fn draw_pushes(frame: &mut Frame, app: &App, below: Option<u16>) {
    if app.pushes.is_empty() {
        return;
    }
    let full = frame.area();
    let width = notices::TOAST_WIDTH.min(full.width.saturating_sub(4));
    if width < 20 {
        return;
    }
    let inner = width.saturating_sub(4) as usize;
    let lines: Vec<Line> = app.pushes.iter().map(|push| Line::from(vec![
        Span::styled(format!("{} ", Tone::Moving.glyph()), bold(tone_color(Tone::Moving))),
        Span::styled(cut(&app.push_line(push), inner.saturating_sub(2)), style(TEXT)),
    ])).collect();
    let bottom = below.unwrap_or_else(|| full.bottom().saturating_sub(2));
    let height = (lines.len() as u16 + 2).min(bottom.saturating_sub(1));
    if height < 3 {
        return;
    }
    let area = Rect { x: full.right().saturating_sub(width + 2), y: bottom.saturating_sub(height), width, height };
    frame.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(tone_color(Tone::Moving))).padding(Padding::horizontal(1))
        .title(Span::styled(" push ", style(MUTED))).style(Style::new().bg(MANTLE));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::KeyCode;

    use crate::notice::{Notice, Tone};
    use crate::board::test_support::*;
    use crate::board::{App, Mode, Page};
    use crate::board::push::{Marks, Scroll};

    fn titled(name: &str, title: &str, path: &str) -> crate::status::Item {
        let mut item = served(name);
        item.title = Some(title.to_string());
        item.path = Some(PathBuf::from(path));
        item
    }

    type Calls = Arc<Mutex<Vec<(PathBuf, Option<String>, bool)>>>;

    // The push window open on billing's three commits, with a pusher that says its steps and then waits for the test to let it finish.
    fn pushing_app(outcome: crate::sh::Result<String>) -> (App, mpsc::Sender<()>, Calls) {
        let mut app = test_app(vec![titled("billing", "Refund mails", "/work/wt1.web"), titled("search", "Search filters", "/work/wt2.web"), titled("grid", "Fix the grid", "/work/wt3.web")]);
        let (go, wait) = mpsc::channel::<()>();
        let wait = Arc::new(Mutex::new(wait));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        app.pusher = Arc::new(move |path, through, keep_open| {
            seen.lock().unwrap().push((path.to_path_buf(), through.map(str::to_string), keep_open));
            crate::tasks::push_step("billing", "rebasing onto main");
            let _ = wait.lock().unwrap().recv();
            crate::tasks::push_step("billing", "pushing to main");
            outcome.clone()
        });
        let commits: Vec<crate::tasks::Commit> = ["Add the refund mail", "Send it on refund", "Log what was sent"].iter().enumerate()
            .map(|(index, subject)| crate::tasks::Commit { sha: format!("{0}{0}abcdef{0}{0}", index + 1), message: subject.to_string() }).collect();
        app.mode = Mode::ConfirmPush { path: PathBuf::from("/work/wt1.web"), commits, selected: 0, card: None, tests: None, marks: Marks::default(), scroll: Scroll::default() };
        (app, go, calls)
    }

    fn until(app: &mut App, done: impl Fn(&App) -> bool) {
        let start = Instant::now();
        while !done(app) && start.elapsed() < Duration::from_secs(10) {
            app.poll_pushes();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(done(app), "timed out");
    }

    #[test]
    fn a_push_runs_behind_the_board_with_its_step_in_a_notice_and_on_its_row_and_goes_by_as_a_toast() {
        let (mut app, go, calls) = pushing_app(Notice::new(Tone::Done, "Pushed 3 commits to main").next("CI runs now; kebbie closes the task when it is green.").ok());
        press(&mut app, key(KeyCode::Char('y')));
        assert!(matches!(app.mode, Mode::Normal), "no window waits on the push");
        until(&mut app, |app| app.push_note_for(&app.tasks[0]).is_some_and(|note| note.contains("rebasing")));
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("↻ Pushing Refund mails… rebasing onto main"), "the notice says what it does: {screen}");
        let (_, row) = find(&rows, "Refund mails")[0];
        assert!(rows[row].contains("pushing… rebasing onto main"), "its row says so too: {}", rows[row]);
        let before = app.work_selected;
        press(&mut app, key(KeyCode::Char('j')));
        assert_ne!(app.work_selected, before, "j moves while the push runs");
        press(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.work_selected, before, "k moves back");
        press(&mut app, key(KeyCode::Char('2')));
        assert_eq!(app.page, Page::Areas, "the page switches while the push runs");
        press(&mut app, key(KeyCode::Char('1')));
        assert_eq!(app.page, Page::Work);
        assert!(!app.may_restart(), "a rebuilt kebbie waits for the push");
        go.send(()).unwrap();
        until(&mut app, |app| app.pushes.is_empty());
        assert_eq!(*calls.lock().unwrap(), [(PathBuf::from("/work/wt1.web"), Some("33abcdef33".to_string()), false)], "y pushes every commit and closes the task");
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(matches!(app.mode, Mode::Normal) && screen.contains("✓ Pushed 3 commits to main") && screen.contains(" push · Refund mails "), "the outcome goes by as a toast naming the task: {screen}");
        assert!(!screen.contains("pushing…") && !screen.contains("rebasing onto main"), "the row and the notice let go once it is done: {screen}");
    }

    #[test]
    fn a_failure_waits_for_the_window_the_user_has_open_then_offers_its_actions() {
        let refused = Notice::new(Tone::You, "Not pushed: its commits changed after you chose push").you("Open push again to see the new ones.").detail("task", "billing");
        let (mut app, go, _) = pushing_app(refused.err());
        press(&mut app, key(KeyCode::Char('y')));
        press(&mut app, key(KeyCode::Char('?')));
        assert!(matches!(app.mode, Mode::Help(_)), "another window opens while the push runs");
        go.send(()).unwrap();
        until(&mut app, |app| app.pushes.is_empty());
        app.poll_pushes();
        assert!(matches!(app.mode, Mode::Help(_)), "the failure does not cover the open window");
        press(&mut app, key(KeyCode::Esc));
        app.poll_pushes();
        let Mode::Message(text) = &app.mode else { panic!("the failure opens once the board is at rest: {:?}", std::mem::discriminant(&app.mode)) };
        assert!(text.contains("commits changed after you chose push"));
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains(" push · Refund mails ") && screen.contains("push again") && screen.contains("its Space"), "it offers its actions: {screen}");
    }

    #[test]
    fn two_pushes_run_at_once_each_on_its_own_line_and_a_task_pushing_takes_no_second_push() {
        let (mut app, go, calls) = pushing_app(Notice::new(Tone::Done, "Pushed").ok());
        press(&mut app, key(KeyCode::Char('y')));
        let commits = vec![crate::tasks::Commit { sha: "aaaabbbb".to_string(), message: "Filter by price".to_string() }];
        app.mode = Mode::ConfirmPush { path: PathBuf::from("/work/wt2.web"), commits, selected: 0, card: None, tests: None, marks: Marks::default(), scroll: Scroll::default() };
        press(&mut app, key(KeyCode::Char('y')));
        until(&mut app, |_| calls.lock().unwrap().len() == 2);
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("Pushing Refund mails…") && screen.contains("Pushing Search filters…"), "one line each: {screen}");
        app.work_selected = 0;
        press(&mut app, key(KeyCode::Char('p')));
        assert!(matches!(app.mode, Mode::Normal) && app.toast.as_ref().is_some_and(|toast| toast.notice.headline.starts_with("Already pushing")), "p on a task being pushed says so");
        go.send(()).unwrap();
        go.send(()).unwrap();
        until(&mut app, |app| app.pushes.is_empty());
    }
}
