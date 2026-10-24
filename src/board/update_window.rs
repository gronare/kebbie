use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};

use crate::sh::Result;
use crate::update_progress::{self as progress, Progress, State};
use super::widgets::{GREEN, MAUVE, MUTED, PEACH, RED, TEXT, bold, modal, style, wrap};
use super::{App, Mode};

const WIDTH: u16 = 66;

pub(super) struct Run {
    result: Receiver<Result<String>>,
    pub(super) outcome: Option<Result<String>>,
    details: bool,
    file: PathBuf,
}

impl Run {
    pub(super) fn running(&self) -> bool {
        self.outcome.is_none()
    }
}

fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), bold(PEACH)))
}

fn muted(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(text.into(), style(MUTED)))
}

fn keys(pairs: &[(&str, &str)]) -> Line<'static> {
    super::hub_window::keys(pairs)
}

fn bullet(text: &str, width: usize) -> Vec<Line<'static>> {
    wrap(text, width.saturating_sub(3), usize::MAX).into_iter().enumerate()
        .map(|(index, part)| Line::from(vec![Span::styled(if index == 0 { " • " } else { "   " }, style(MUTED)), Span::styled(part, style(TEXT))])).collect()
}

fn indented(text: &str, width: usize) -> Vec<Line<'static>> {
    wrap(text, width.saturating_sub(1), usize::MAX).into_iter().map(|part| Line::from(Span::styled(format!(" {part}"), style(TEXT)))).collect()
}

pub(super) fn step_lines(progress: &Progress, now: u64, width: usize) -> Vec<Line<'static>> {
    progress::lines(progress, now, width).into_iter().map(|(state, text)| {
        let (mark, rest) = text.split_at(text.char_indices().nth(1).map_or(text.len(), |(at, _)| at));
        let (mark_style, text_style) = match state {
            State::Done => (style(GREEN), style(TEXT)),
            State::Current => (style(MAUVE), bold(TEXT)),
            State::Failed => (style(RED), style(RED)),
            State::Pending => (style(MUTED), style(MUTED)),
        };
        Line::from(vec![Span::styled(mark.to_string(), mark_style), Span::styled(rest.to_string(), text_style)])
    }).collect()
}

fn running(progress: Option<&Progress>, now: u64, width: usize) -> Vec<Line<'static>> {
    let mut lines = match progress.filter(|progress| !progress.steps.is_empty()) {
        Some(progress) => step_lines(progress, now, width),
        None => vec![muted("▸ Starting the update…")],
    };
    lines.push(Line::default());
    lines.push(muted("esc hides this; the update keeps going"));
    lines
}

pub(super) fn finished(progress: Option<&Progress>, bugs: &[String], width: usize) -> (String, Vec<Line<'static>>) {
    let summary = progress.and_then(|progress| progress.summary.clone()).unwrap_or_default();
    let title = if summary.version.is_empty() { "kelpie".to_string() } else { format!("kelpie {}", summary.version) };
    let mut lines = vec![Line::from(Span::styled("✓ Updated", bold(GREEN))), Line::default()];
    let fixed: Vec<String> = summary.fixed.iter().cloned().chain(bugs.iter().map(|title| crate::update::bug_line(title))).collect();
    if summary.new.is_empty() && fixed.is_empty() {
        lines.push(muted("No notes for this update"));
    }
    if !summary.new.is_empty() {
        lines.push(heading("What's new"));
        lines.extend(summary.new.iter().flat_map(|entry| bullet(entry, width)));
    }
    if !fixed.is_empty() {
        lines.push(heading("Fixed"));
        lines.extend(fixed.iter().flat_map(|entry| bullet(entry, width)));
    }
    if !summary.restart.is_empty() {
        lines.push(Line::default());
        lines.push(heading("Restarting"));
        lines.extend(summary.restart.iter().flat_map(|said| indented(said, width)));
    }
    lines.push(Line::default());
    lines.push(keys(&[("d", "details"), ("any other key", "closes")]));
    (title, lines)
}

fn failed(error: &str, progress: Option<&Progress>, now: u64, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled("✗ The update stopped", bold(RED)))];
    lines.extend(wrap(error.lines().next().unwrap_or_default(), width, 3).into_iter().map(|part| Line::from(Span::styled(part, style(TEXT)))));
    if let Some(progress) = progress {
        lines.push(Line::default());
        lines.extend(step_lines(progress, now, width));
    }
    lines.push(Line::default());
    lines.push(keys(&[("d", "details"), ("any other key", "closes")]));
    lines
}

fn details(text: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = text.lines().map(|line| Line::from(Span::styled(line.to_string(), style(TEXT)))).collect();
    lines.push(Line::default());
    lines.push(keys(&[("d", "back"), ("any other key", "closes")]));
    lines
}

pub(super) fn draw(frame: &mut Frame, app: &App) {
    let Some(run) = &app.update else { return };
    let width = WIDTH.min(frame.area().width.saturating_sub(4)).saturating_sub(4) as usize;
    let progress = progress::load(&run.file);
    let now = crate::messages::now();
    let bugs = app.fixed_bugs(progress.as_ref());
    let (title, lines) = match &run.outcome {
        None => ("Updating kelpie".to_string(), running(progress.as_ref(), now, width)),
        Some(Ok(text)) if run.details => (finished(progress.as_ref(), &bugs, width).0, details(text)),
        Some(Ok(_)) => finished(progress.as_ref(), &bugs, width),
        Some(Err(error)) if run.details => ("kelpie update".to_string(), details(error)),
        Some(Err(error)) => ("kelpie update".to_string(), failed(error, progress.as_ref(), now, width)),
    };
    modal(frame, title, lines, WIDTH);
}

impl App {
    fn progress_file(&self) -> PathBuf {
        self.progress_file.clone().or_else(|| crate::lead::state_dir().ok().map(|state| progress::file(&state))).unwrap_or_default()
    }

    // A fixed line left from an earlier update names another version, so it stays out.
    pub(super) fn fixed_bugs(&self, progress: Option<&Progress>) -> Vec<String> {
        let version = progress.and_then(|progress| progress.summary.as_ref()).map(|summary| summary.version.clone()).unwrap_or_default();
        self.fixed.as_ref().filter(|fixed| !version.is_empty() && fixed.version == version).map(|fixed| fixed.bugs.clone()).unwrap_or_default()
    }

    pub(super) fn start_update(&mut self) {
        if self.update.is_some() {
            return self.mode = Mode::Update;
        }
        let file = self.progress_file();
        let _ = std::fs::remove_file(&file);
        let updater = self.updater.clone();
        let (sender, result) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(updater());
        });
        self.update = Some(Run { result, outcome: None, details: false, file });
        self.mode = Mode::Update;
    }

    pub(super) fn poll_update(&mut self) {
        let Some(run) = self.update.as_mut().filter(|run| run.running()) else { return };
        let Ok(outcome) = run.result.try_recv() else { return };
        run.outcome = Some(outcome);
        self.updates.clear();
        if let Some(recheck) = &self.recheck {
            let _ = recheck.send(());
        }
        if matches!(self.mode, Mode::Normal) && self.page == super::Page::Work {
            self.mode = Mode::Update;
        }
        let _ = self.refresh.send(());
    }

    pub(super) fn handle_update(&mut self, key: KeyEvent) {
        let Some(run) = self.update.as_mut() else { return };
        if run.running() {
            if !matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                self.mode = Mode::Update;
            }
            return;
        }
        if key.code == KeyCode::Char('d') {
            run.details = !run.details;
            self.mode = Mode::Update;
            return;
        }
        self.update = None;
    }

    pub(super) fn update_pill(&self) -> Option<String> {
        let run = self.update.as_ref().filter(|_| !matches!(self.mode, Mode::Update))?;
        let key = crate::keys::map().label(crate::keys::Act::Update);
        Some(if run.running() { format!("⇣ updating kelpie · {key}") } else { format!("✓ kelpie updated · {key}") })
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::board::test_support::*;
    use crate::update_progress::{Meter, Summary, Tracker};

    fn waits_for(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
        let started = Instant::now();
        while !done(app) {
            assert!(started.elapsed() < Duration::from_secs(10), "{what} never came");
            thread::sleep(Duration::from_millis(10));
            app.poll_update();
        }
    }

    fn screen(app: &mut App, width: u16) -> String {
        render(app, width, 40).0.join("\n")
    }

    fn summary(new: &[&str], fixed: &[&str]) -> Summary {
        let owned = |lines: &[&str]| lines.iter().map(|line| line.to_string()).collect();
        Summary { version: "0.27.0".to_string(), new: owned(new), fixed: owned(fixed), restart: vec!["The lead and 2 sessions restart when they're idle.".to_string(), "The board restarts when you open it again.".to_string()] }
    }

    fn updating(app: &mut App, finish: Summary) -> mpsc::Sender<()> {
        let file = crate::sh::test_dir("board-update").join(progress::FILE);
        app.progress_file = Some(file.clone());
        let (release, gate) = mpsc::channel::<()>();
        let gate = std::sync::Mutex::new(gate);
        app.updater = std::sync::Arc::new(move || {
            let mut tracker = Tracker::start(&file, "Fetching main", "Build kelpie", crate::messages::now());
            tracker.done(progress::FETCH, "Fetched main (3 new commits)");
            tracker.begin(progress::BUILD, "Building kelpie", crate::messages::now());
            tracker.meter(Meter::Build { done: 212, total: Some(348), compiling: None });
            gate.lock().unwrap().recv().map_err(|error| error.to_string())?;
            tracker.done(progress::BUILD, "Built kelpie");
            tracker.done(progress::PLUGINS, "Updated the Claude Code and vault plugins");
            tracker.done(progress::RESTART, "Sessions move to the new kelpie");
            tracker.finish(finish.clone());
            Ok("kelpie follows the main channel\nkelpie 0.27.0 rebuilt, 02c7fe0 → 9cbd545\n\nClaude sessions:\n  lead: restarts when idle\nkeeper: restarted on the new kelpie (pid 4242)".to_string())
        });
        release
    }

    #[test]
    fn u_shows_the_steps_with_the_build_bar_and_esc_hides_the_window_while_the_update_keeps_going() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        let release = updating(&mut app, summary(&["The hub window shows your devices by name, and r removes one"], &[]));
        app.updates = vec![("kelpie".to_string(), "0.27.0".to_string())];
        press(&mut app, key(KeyCode::Char('U')));
        assert!(matches!(app.mode, Mode::Update), "the key returns while the update runs");
        waits_for(&mut app, "the build step", |app| progress::load(&app.progress_file()).is_some_and(|progress| progress.meter.is_some()));
        let shown = screen(&mut app, 100);
        for line in ["Updating kelpie", "✓ Fetched main (3 new commits)", "▸ Building kelpie  ████████████░░░░░░░░  212/348 crates 0:0", "  Claude Code and vault plugins", "  Restart sessions", "esc hides this; the update keeps going"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Update), "only esc hides it");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal) && app.update.as_ref().is_some_and(Run::running), "hidden, still running");
        assert!(crate::board::header::header(&app, 200).0.to_string().contains("⇣ updating kelpie · U"), "{}", crate::board::header::header(&app, 200).0);
        press(&mut app, key(KeyCode::Char('U')));
        assert!(matches!(app.mode, Mode::Update), "U opens the window again, not a second update");
        press(&mut app, key(KeyCode::Esc));
        release.send(()).unwrap();
        waits_for(&mut app, "the outcome", |app| app.update.as_ref().is_some_and(|run| !run.running()));
        assert!(matches!(app.mode, Mode::Update) && app.updates.is_empty(), "the summary opens by itself once the board is at rest");
    }

    #[test]
    fn the_finished_window_shows_the_notes_the_bugs_the_recheck_closed_and_one_line_on_restarts_with_the_rest_behind_d() {
        let mut app = test_app(Vec::new());
        let release = updating(&mut app, summary(&["The hub window shows your devices by name, and r removes one"], &[]));
        app.fixed = Some(crate::issue_recheck::Fixed { version: "0.27.0".to_string(), titles: vec!["update stopped on a newer config key".to_string()], bugs: vec!["update stopped on a newer config key".to_string()], wishes: 0, last: 1 });
        press(&mut app, key(KeyCode::Char('U')));
        release.send(()).unwrap();
        waits_for(&mut app, "the outcome", |app| app.update.as_ref().is_some_and(|run| !run.running()));
        let shown = screen(&mut app, 100);
        for line in ["kelpie 0.27.0", "✓ Updated", "What's new", " • The hub window shows your devices by name, and r removes", "   one", "Fixed", " • A bug you ran into: update stopped on a newer config key", "Restarting", " The lead and 2 sessions restart when they're idle.", " The board restarts when you open it again.", "d details · any other key closes"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        assert!(!shown.contains("pid") && !shown.contains("02c7fe0") && !shown.contains("lead: restarts"), "the detail lines wait behind d: {shown}");
        press(&mut app, key(KeyCode::Char('d')));
        let shown = screen(&mut app, 100);
        assert!(shown.contains("kelpie 0.27.0 rebuilt, 02c7fe0 → 9cbd545") && shown.contains("keeper: restarted on the new kelpie (pid 4242)") && shown.contains("d back · any other key closes"), "{shown}");
        press(&mut app, key(KeyCode::Char('d')));
        assert!(screen(&mut app, 100).contains("What's new"), "d again goes back");
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Normal) && app.update.is_none(), "any other key closes it");
    }

    #[test]
    fn an_update_with_nothing_written_says_so_and_never_lists_commits() {
        let mut app = test_app(Vec::new());
        let release = updating(&mut app, summary(&[], &[]));
        press(&mut app, key(KeyCode::Char('U')));
        release.send(()).unwrap();
        waits_for(&mut app, "the outcome", |app| app.update.as_ref().is_some_and(|run| !run.running()));
        let shown = screen(&mut app, 100);
        assert!(shown.contains("No notes for this update") && !shown.contains("What's new") && !shown.contains("Fixed"), "{shown}");
    }

    #[test]
    fn a_failed_update_names_the_step_it_stopped_at() {
        let mut app = test_app(Vec::new());
        let file = crate::sh::test_dir("board-update-failed").join(progress::FILE);
        app.progress_file = Some(file.clone());
        app.updater = std::sync::Arc::new(move || {
            let mut tracker = Tracker::start(&file, "Fetching main", "Build kelpie", crate::messages::now());
            tracker.done(progress::FETCH, "Fetched main (1 new commit)");
            tracker.begin(progress::BUILD, "Building kelpie", crate::messages::now());
            tracker.fail_current();
            Err("sh scripts/build.sh failed: error[E0425]: cannot find value `x`".to_string())
        });
        press(&mut app, key(KeyCode::Char('U')));
        waits_for(&mut app, "the outcome", |app| app.update.as_ref().is_some_and(|run| !run.running()));
        let shown = screen(&mut app, 100);
        assert!(shown.contains("✗ The update stopped") && shown.contains("sh scripts/build.sh failed") && shown.contains("✗ Building kelpie") && shown.contains("  Restart sessions"), "{shown}");
    }

    #[test]
    fn the_board_waits_to_restart_on_the_new_binary_until_the_update_window_is_closed() {
        let mut app = test_app(Vec::new());
        let _release = updating(&mut app, summary(&[], &[]));
        press(&mut app, key(KeyCode::Char('U')));
        press(&mut app, key(KeyCode::Esc));
        assert!(!app.may_restart(), "a restart now would end the update running in this board");
        app.update = None;
        assert!(app.may_restart());
    }
}
