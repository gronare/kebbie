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
    Arc::new(|path, through, keep_open| tasks::push(path, None, through, keep_open, true))
}

pub(super) type Dropper = Arc<dyn Fn(&Path, bool) -> Result<String> + Send + Sync>;

pub(super) fn live_dropper() -> Dropper {
    Arc::new(|path, force| tasks::drop(path, None, force))
}

// A window a failed job opens in place of its notice, such as the forced drop to confirm.
type AskAgain = Box<dyn FnOnce(&str) -> Option<Mode>>;

type After = Box<dyn FnOnce(&mut App)>;

// The step a job is at: the task it works on now and what it does there.
type Step = Arc<Mutex<Option<(String, String)>>>;

const PUSHING: &str = "pushing";

// Work this board runs behind it, for the tasks it holds (one, several in order, or none); their rows say `doing` until it is done.
pub(super) struct Job {
    tasks: Vec<(String, Option<PathBuf>)>,
    label: String,
    doing: String,
    step: Step,
    subject: Option<Subject>,
    result: Receiver<Result<String>>,
    ask_again: Option<AskAgain>,
    after: Option<After>,
}

impl Job {
    pub(super) fn ask_again(&mut self, ask: impl FnOnce(&str) -> Option<Mode> + 'static) {
        self.ask_again = Some(Box::new(ask));
    }

    // Runs on the board only once the job succeeded, before its outcome shows.
    pub(super) fn then(&mut self, after: impl FnOnce(&mut App) + 'static) {
        self.after = Some(Box::new(after));
    }

    fn step(&self) -> Option<(String, String)> {
        self.step.lock().ok().and_then(|step| step.clone())
    }

    fn holds(&self, name: &str, path: Option<&Path>) -> bool {
        self.tasks.iter().any(|(held, at)| match (at, path) {
            (Some(at), Some(path)) => at == path,
            _ => held == name,
        })
    }
}

impl App {
    pub(super) fn push_in_background(&mut self, label: String, tasks: Vec<(String, PathBuf)>, run: impl FnOnce(&Pusher) -> Result<String> + Send + 'static) {
        let step: Step = Arc::default();
        let (into, pusher) = (step.clone(), self.pusher.clone());
        let subject = match tasks.as_slice() {
            [(name, _)] => self.subject_for("push", name),
            _ => Subject::plain("push"),
        };
        let held = tasks.into_iter().map(|(name, path)| (name, Some(path))).collect();
        self.start_job(held, label, PUSHING, Some(subject), step, move || tasks::push_watched(move |task, what| {
            if let Ok(mut step) = into.lock() {
                *step = Some((task.to_string(), what.to_string()));
            }
        }, || run(&pusher)));
    }

    // Work for no one task, such as the lead's restart or a device's answer; its outcome names what self.subject says.
    pub(super) fn in_background(&mut self, label: String, work: impl FnOnce() -> Result<String> + Send + 'static) -> &mut Job {
        let subject = self.subject.take();
        self.start_job(Vec::new(), label, "working", subject, Step::default(), work)
    }

    // Work on one task; a task the board already works on takes no second job until the first is done.
    pub(super) fn task_in_background(&mut self, name: &str, path: Option<&Path>, doing: &str, label: String, work: impl FnOnce() -> Result<String> + Send + 'static) -> Option<&mut Job> {
        if let Some(already) = self.doing(name, path) {
            self.already(already);
            return None;
        }
        let subject = self.subject.take();
        Some(self.start_job(vec![(name.to_string(), path.map(Path::to_path_buf))], label, doing, subject, Step::default(), work))
    }

    fn start_job(&mut self, tasks: Vec<(String, Option<PathBuf>)>, label: String, doing: &str, subject: Option<Subject>, step: Step, work: impl FnOnce() -> Result<String> + Send + 'static) -> &mut Job {
        let (sender, result) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(work());
        });
        let label = label.trim_end_matches('…').to_string();
        self.jobs.push(Job { tasks, label, doing: doing.to_string(), step, subject, result, ask_again: None, after: None });
        let _ = self.refresh.send(());
        self.jobs.last_mut().expect("just pushed")
    }

    // The lead hears of new drafts from a thread, one telling at a time, as the snapshot that finds them comes every few seconds.
    pub(super) fn tell_new_drafts(&self) {
        if self.telling_drafts.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let telling = self.telling_drafts.clone();
        thread::spawn(move || {
            crate::backlog_file::tell_new_drafts();
            telling.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }

    pub(super) fn doing(&self, name: &str, path: Option<&Path>) -> Option<String> {
        self.jobs.iter().find(|job| job.holds(name, path)).map(|job| job.doing.clone())
    }

    // Refuses a second action on a task a job holds; the caller has named the task with about().
    pub(super) fn already(&mut self, doing: String) {
        self.show(Notice::new(Tone::NotNow, format!("Already {doing} it")).next("The board says how it went once it is done.").ok());
    }

    pub(super) fn jobs_stop_line(&self) -> &'static str {
        match self.jobs.iter().all(|job| job.doing == PUSHING) {
            _ if self.jobs.is_empty() => "",
            true => ", and the push under way stops where it is",
            false => ", and what runs in the background stops where it is",
        }
    }

    // An outcome that asks nothing goes by as a toast at once, an empty one says nothing; one that needs the user waits until the board is at rest on its work page, or on a subtask's row or card it goes back to, so it never covers a window the user has open.
    pub(super) fn poll_jobs(&mut self) {
        let mut at = 0;
        while at < self.jobs.len() {
            match self.jobs[at].result.try_recv() {
                Ok(result) => {
                    let mut job = self.jobs.remove(at);
                    self.drop_restarted_lead();
                    if let (Ok(_), Some(after)) = (&result, job.after.take()) {
                        after(self);
                    }
                    if let (Err(error), Some(ask)) = (&result, job.ask_again.take())
                        && let Some(window) = ask(error)
                    {
                        self.outcomes.push((window, job.subject));
                        continue;
                    }
                    let _ = self.refresh.send(());
                    if result.as_ref().is_ok_and(String::is_empty) {
                        continue;
                    }
                    let failed = result.is_err();
                    let notice = Notice::read(&result.unwrap_or_else(|error| error), failed);
                    if notice.tone.passes() && notice.you.is_none() {
                        self.toast = Some(notices::Toast { notice, subject: job.subject, at: Instant::now() });
                    } else {
                        self.outcomes.push((Mode::Message(notice.to_string()), job.subject));
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let job = self.jobs.remove(at);
                    self.outcomes.push((Mode::Message(Notice::new(Tone::Failed, format!("{} stopped without an answer", job.label)).next("Look at its row: it shows where it stands.").to_string()), job.subject));
                }
                Err(mpsc::TryRecvError::Empty) => at += 1,
            }
        }
        let message = matches!(self.outcomes.first(), Some((Mode::Message(_), _)));
        let calm = match self.mode {
            Mode::Normal => true,
            Mode::Subtasks | Mode::SubtaskCard(_) => message,
            _ => false,
        };
        if !self.outcomes.is_empty() && calm && self.page == Page::Work {
            let (window, subject) = self.outcomes.remove(0);
            let back = std::mem::replace(&mut self.mode, Mode::Normal);
            if let Mode::Message(text) = &window && !matches!(back, Mode::Normal) {
                self.after_message = Some((text.clone(), back));
            }
            self.shown = subject.filter(|_| matches!(window, Mode::Message(_)));
            self.mode = window;
        }
    }

    // What a task's row says while a job holds it: the step for the task the job is at, plain for one still waiting its turn in the same job.
    pub(super) fn job_note_for(&self, item: &Item) -> Option<String> {
        let job = self.jobs.iter().find(|job| job.holds(&item.name, item.path.as_deref()))?;
        Some(match job.step() {
            Some((task, what)) if task == item.name => format!("{}… {what}", job.doing),
            _ => format!("{}…", job.doing),
        })
    }

    pub(super) fn job_line(&self, job: &Job) -> String {
        let title = |name: &str| self.tasks.iter().find(|item| item.name == name).map_or_else(|| name.to_string(), Item::titled);
        match (job.step(), job.tasks.len()) {
            (Some((_, what)), 1) => format!("{}… {what}", job.label),
            (Some((task, what)), _) => format!("{}… {}: {what}", job.label, title(&task)),
            (None, _) => format!("{}…", job.label),
        }
    }

    fn subject_for(&self, kind: &str, name: &str) -> Subject {
        self.tasks.iter().find(|item| item.name == name).map_or_else(|| Subject { kind: kind.to_string(), title: name.to_string(), task: true }, |item| Subject::of(kind, item))
    }

    pub(super) fn push_label(&self, name: &str) -> String {
        format!("Pushing {}", self.subject_for("push", name).title)
    }
}

// One line per job under way, the board's and its pages', bottom right where toasts show, above a toast that shows at the same time.
pub(super) fn draw_jobs(frame: &mut Frame, app: &App, below: Option<u16>) {
    let texts = app.under_way();
    if texts.is_empty() {
        return;
    }
    let full = frame.area();
    let width = notices::TOAST_WIDTH.min(full.width.saturating_sub(4));
    if width < 20 {
        return;
    }
    let inner = width.saturating_sub(4) as usize;
    let lines: Vec<Line> = texts.iter().map(|text| Line::from(vec![
        Span::styled(format!("{} ", Tone::Moving.glyph()), bold(tone_color(Tone::Moving))),
        Span::styled(cut(text, inner.saturating_sub(2)), style(TEXT)),
    ])).collect();
    let bottom = below.unwrap_or_else(|| full.bottom().saturating_sub(2));
    let height = (lines.len() as u16 + 2).min(bottom.saturating_sub(1));
    if height < 3 {
        return;
    }
    let area = Rect { x: full.right().saturating_sub(width + 2), y: bottom.saturating_sub(height), width, height };
    frame.render_widget(Clear, area);
    let title = if !app.jobs.is_empty() && app.jobs.iter().all(|job| job.doing == PUSHING) && texts.len() == app.jobs.len() { " push " } else { " under way " };
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(tone_color(Tone::Moving))).padding(Padding::horizontal(1))
        .title(Span::styled(title, style(MUTED))).style(Style::new().bg(MANTLE));
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
            app.poll_jobs();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(done(app), "timed out");
    }

    #[test]
    fn a_push_runs_behind_the_board_with_its_step_in_a_notice_and_on_its_row_and_goes_by_as_a_toast() {
        let (mut app, go, calls) = pushing_app(Notice::new(Tone::Done, "Pushed 3 commits to main").next("CI runs now; kebbie closes the task when it is green.").ok());
        press(&mut app, key(KeyCode::Char('y')));
        assert!(matches!(app.mode, Mode::Normal), "no window waits on the push");
        until(&mut app, |app| app.job_note_for(&app.tasks[0]).is_some_and(|note| note.contains("rebasing")));
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("↻ Pushing Refund mails… rebasing onto main"), "the notice says what it does: {screen}");
        let (_, row) = find(&rows, "Refund mails").into_iter().find(|(x, _)| *x < 60).unwrap();
        assert!(rows[row].contains("pushing… rebasing onto main"), "its row says so too: {}", rows[row]);
        let before = app.work_selected;
        press(&mut app, key(KeyCode::Char('j')));
        assert_ne!(app.work_selected, before, "j moves while the push runs");
        press(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.work_selected, before, "k moves back");
        press(&mut app, key(KeyCode::Char('3')));
        assert_eq!(app.page, Page::Areas, "the page switches while the push runs");
        press(&mut app, key(KeyCode::Char('1')));
        assert_eq!(app.page, Page::Work);
        assert!(!app.may_restart(), "a rebuilt kebbie waits for the push");
        go.send(()).unwrap();
        until(&mut app, |app| app.jobs.is_empty());
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
        until(&mut app, |app| app.jobs.is_empty());
        app.poll_jobs();
        assert!(matches!(app.mode, Mode::Help(_)), "the failure does not cover the open window");
        assert!(!app.may_restart(), "a rebuilt kebbie waits until the failure is shown");
        press(&mut app, key(KeyCode::Esc));
        app.poll_jobs();
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
        until(&mut app, |app| app.jobs.is_empty());
    }

    #[test]
    fn a_task_job_holds_its_row_while_the_board_keeps_taking_keys_and_refuses_a_second_action_on_it() {
        let mut app = test_app(vec![titled("billing", "Refund mails", "/work/wt1.web"), titled("search", "Search filters", "/work/wt2.web")]);
        let (go, wait) = mpsc::channel::<()>();
        app.about("rebase", "billing");
        app.task_in_background("billing", Some(std::path::Path::new("/work/wt1.web")), "rebasing", "Rebasing billing…".to_string(), move || {
            let _ = wait.recv();
            Ok("Rebased billing onto main".to_string())
        });
        assert!(matches!(app.mode, Mode::Normal), "no window waits on it");
        assert_eq!(app.job_note_for(&app.tasks[0]).as_deref(), Some("rebasing…"));
        assert_eq!(app.job_note_for(&app.tasks[1]), None, "another task's row says nothing");
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("↻ Rebasing billing…") && screen.contains(" under way "), "a line says what runs: {screen}");
        let before = app.work_selected;
        press(&mut app, key(KeyCode::Char('j')));
        assert_ne!(app.work_selected, before, "j moves while it runs");
        press(&mut app, key(KeyCode::Char('k')));
        press(&mut app, key(KeyCode::Char('?')));
        assert!(matches!(app.mode, Mode::Help(_)), "a window opens while it runs");
        press(&mut app, key(KeyCode::Esc));
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Normal) && app.toast.as_ref().is_some_and(|toast| toast.notice.headline == "Already rebasing it"), "a second action on it is refused before it asks");
        assert!(!app.may_restart(), "a rebuilt kebbie waits for it");
        go.send(()).unwrap();
        settle_jobs(&mut app);
        assert!(matches!(app.mode, Mode::Normal) && app.toast.as_ref().is_some_and(|toast| toast.notice.headline.contains("Rebased billing")), "the outcome goes by as a toast");
        assert_eq!(app.job_note_for(&app.tasks[0]), None, "the row lets go once it is done");
    }

    #[test]
    fn a_drop_runs_behind_the_board_and_one_that_needs_force_asks_again_once_the_board_is_at_rest() {
        let mut app = test_app(vec![titled("billing", "Refund mails", "/work/wt1.web"), titled("search", "Search filters", "/work/wt2.web")]);
        let (go, wait) = mpsc::channel::<()>();
        let wait = Arc::new(Mutex::new(wait));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        app.dropper = Arc::new(move |path, force| {
            seen.lock().unwrap().push((path.to_path_buf(), force));
            let _ = wait.lock().unwrap().recv();
            if force { Ok("Dropped billing".to_string()) } else { Notice::new(Tone::You, "Not dropped: it has commits nowhere else").you(format!("{} drops them too.", crate::tasks::FORCE)).err() }
        });
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::ConfirmDrop { force: false, .. }), "x asks first");
        press(&mut app, key(KeyCode::Char('y')));
        assert!(matches!(app.mode, Mode::Normal), "no window waits on the drop");
        until(&mut app, |_| calls.lock().unwrap().len() == 1);
        assert_eq!(app.job_note_for(&app.tasks[0]).as_deref(), Some("dropping…"));
        let before = app.work_selected;
        press(&mut app, key(KeyCode::Char('j')));
        assert_ne!(app.work_selected, before, "j moves while the drop runs");
        press(&mut app, key(KeyCode::Char('k')));
        press(&mut app, key(KeyCode::Char('?')));
        go.send(()).unwrap();
        settle_jobs(&mut app);
        assert!(matches!(app.mode, Mode::Help(_)), "the question waits for the window the user has open");
        press(&mut app, key(KeyCode::Esc));
        app.poll_jobs();
        let Mode::ConfirmDrop { force: true, name, .. } = &app.mode else { panic!("the forced drop asks once the board is at rest") };
        assert!(name.contains("commits nowhere else"), "{name}");
        press(&mut app, key(KeyCode::Char('y')));
        go.send(()).unwrap();
        settle_jobs(&mut app);
        assert_eq!(*calls.lock().unwrap(), [(PathBuf::from("/work/wt1.web"), false), (PathBuf::from("/work/wt1.web"), true)]);
        assert!(matches!(app.mode, Mode::Normal) && app.toast.as_ref().is_some_and(|toast| toast.notice.headline == "Dropped billing"), "the drop goes by as a toast");
    }
}
