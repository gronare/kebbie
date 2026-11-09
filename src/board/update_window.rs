use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use crate::issue_recheck::Fixed;
use crate::sh::Result;
use crate::update_progress::{self as progress, Overall, Progress, State, Summary};
use super::keys_window::Scroll;
use super::widgets::{GREEN, MAUVE, MUTED, PEACH, RED, TEXT, bold, centered, modal_block, style, wrap};
use super::{App, Mode};

const WIDTH: u16 = 66;

pub(super) struct Run {
    result: Receiver<Result<String>>,
    pub(super) outcome: Option<Result<String>>,
    details: bool,
    file: PathBuf,
    overall: Cell<Overall>,
    fixed: Option<Fixed>,
    fixed_read: Option<Instant>,
    scroll: Cell<Scroll>,
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

fn running(progress: Option<&Progress>, overall: &Cell<Overall>, now: u64, width: usize) -> Vec<Line<'static>> {
    match progress.filter(|progress| !progress.steps.is_empty()) {
        Some(progress) => {
            let mut bar = overall.get();
            let shown = bar.advance(progress, now);
            overall.set(bar);
            let mut lines = vec![Line::default(), Line::from(Span::styled(progress::overall_line(shown, width), style(MAUVE))), Line::default()];
            lines.extend(step_lines(progress, now, width));
            lines
        }
        None => vec![muted("▸ Starting the update…")],
    }
}

pub(super) fn finished(progress: Option<&Progress>, bugs: &[String], restart: &[String], width: usize) -> (String, Vec<Line<'static>>) {
    let summary = progress.and_then(|progress| progress.summary.clone()).unwrap_or_default();
    let title = match (summary.version.as_str(), summary.from.as_str(), summary.head.as_str()) {
        ("", _, _) => "kebbie".to_string(),
        (version, from, head) if !from.is_empty() && !head.is_empty() && from != head => format!("kebbie {version} · {from} → {head}"),
        (version, _, _) => format!("kebbie {version}"),
    };
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
    if !restart.is_empty() {
        lines.push(Line::default());
        lines.push(heading("Restarting"));
        lines.extend(restart.iter().flat_map(|said| indented(said, width)));
    }
    (title, lines)
}

fn failed(error: &str, progress: Option<&Progress>, now: u64, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled("✗ The update stopped", bold(RED)))];
    lines.extend(wrap(error.lines().next().unwrap_or_default(), width, 3).into_iter().map(|part| Line::from(Span::styled(part, style(TEXT)))));
    if let Some(progress) = progress {
        lines.push(Line::default());
        lines.extend(step_lines(progress, now, width));
    }
    lines
}

fn details(text: &str) -> Vec<Line<'static>> {
    text.lines().map(|line| Line::from(Span::styled(line.to_string(), style(TEXT)))).collect()
}

fn footer(d: Option<&str>, scrolls: bool, more: bool) -> Line<'static> {
    let mut line = match d {
        Some(d) => keys(&[("↑ ↓", "scroll"), ("d", d), ("any other key", "closes")][usize::from(!scrolls)..]),
        None => muted("esc hides this; the update keeps going"),
    };
    if more {
        line.spans.push(Span::styled(" · ↓ more", style(MUTED)));
    }
    line
}

fn rows(lines: &[Line<'static>], width: u16) -> u16 {
    Paragraph::new(lines.to_vec()).wrap(Wrap { trim: false }).line_count(width) as u16
}

fn draw_scrolled(frame: &mut Frame, title: String, body: Vec<Line<'static>>, d: Option<&str>, scroll: &Cell<Scroll>) {
    super::link::note_lines(&body);
    let screen = frame.area();
    let width = WIDTH.min(screen.width.saturating_sub(4));
    let inner = width.saturating_sub(4);
    let body_rows = rows(&body, inner);
    let room = screen.height.saturating_sub(5 + rows(&[footer(d, true, true)], inner)).max(1);
    let mut at = scroll.get();
    at.fit(body_rows.saturating_sub(room));
    scroll.set(at);
    let foot = vec![Line::default(), footer(d, at.scrolls(), at.more())];
    let visible = body_rows.min(room);
    let area = centered(screen, width, visible + rows(&foot, inner) + 2);
    let block = modal_block(title);
    let inside = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let shown = visible.min(inside.height);
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }).scroll((at.at(), 0)), Rect { height: shown, ..inside });
    frame.render_widget(Paragraph::new(foot).wrap(Wrap { trim: false }), Rect { y: inside.y + shown, height: inside.height - shown, ..inside });
}

pub(super) fn draw(frame: &mut Frame, app: &App) {
    let Some(run) = &app.update else { return };
    let width = WIDTH.min(frame.area().width.saturating_sub(4)).saturating_sub(4) as usize;
    let progress = progress::load(&run.file);
    let now = crate::messages::now();
    let bugs = run.fixed.as_ref().map(|fixed| fixed.bugs.clone()).unwrap_or_default();
    let restart = progress.as_ref().and_then(|progress| progress.summary.as_ref()).map(|summary| app.restart_lines(summary)).unwrap_or_default();
    let (title, lines, d) = match &run.outcome {
        None => ("Updating kebbie".to_string(), running(progress.as_ref(), &run.overall, now, width), None),
        Some(Ok(text)) if run.details => (finished(progress.as_ref(), &bugs, &restart, width).0, details(text), Some("back")),
        Some(Ok(_)) => {
            let (title, lines) = finished(progress.as_ref(), &bugs, &restart, width);
            (title, lines, Some("details"))
        }
        Some(Err(error)) if run.details => ("kebbie update".to_string(), details(error), Some("back")),
        Some(Err(error)) => ("kebbie update".to_string(), failed(error, progress.as_ref(), now, width), Some("details")),
    };
    draw_scrolled(frame, title, lines, d, &run.scroll);
}

impl App {
    fn progress_file(&self) -> PathBuf {
        self.progress_file.clone().or_else(|| crate::lead::state_dir().ok().map(|state| progress::file(&state))).unwrap_or_default()
    }

    // The board restarts itself onto the new kebbie once this window closes, so it names no board; a restart round older than the update's own says nothing new.
    pub(super) fn restart_lines(&self, summary: &Summary) -> Vec<String> {
        if summary.sessions.is_empty() {
            return summary.restart.iter().filter(|line| !line.starts_with("The board") && !line.starts_with("Open boards")).cloned().collect();
        }
        let live = (self.stale_at >= summary.restarted_at && summary.restarted_at > 0).then_some(self.stale.as_slice());
        crate::update::restart_summary(&crate::update::live_sessions(&summary.sessions, live))
    }

    fn read_fixed(state: &Path, progress: Option<&Progress>) -> Option<Fixed> {
        let progress = progress?;
        let head = progress.summary.as_ref().map(|summary| summary.head.as_str()).unwrap_or_default();
        crate::issue_recheck::for_update(state, progress.started, head, crate::messages::now())
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
        self.update = Some(Run { result, outcome: None, details: false, file, overall: Cell::new(Overall::default()), fixed: None, fixed_read: None, scroll: Cell::default() });
        self.mode = Mode::Update;
    }

    pub(super) fn poll_update(&mut self) {
        if let Some(run) = self.update.as_mut().filter(|run| matches!(run.outcome, Some(Ok(_))) && run.fixed_read.is_none_or(|at| at.elapsed() >= Duration::from_secs(2)))
            && let Some(state) = run.file.parent()
        {
            run.fixed = Self::read_fixed(state, progress::load(&run.file).as_ref());
            run.fixed_read = Some(Instant::now());
        }
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
        if let Some(moved) = run.scroll.get().step(&key) {
            run.scroll.set(moved);
            self.mode = Mode::Update;
            return;
        }
        if run.running() {
            if !matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                self.mode = Mode::Update;
            }
            return;
        }
        if key.code == KeyCode::Char('d') {
            run.details = !run.details;
            run.scroll.set(Scroll::default());
            self.mode = Mode::Update;
            return;
        }
        let succeeded = matches!(run.outcome, Some(Ok(_)));
        if let (Some(fixed), Some(state)) = (&run.fixed, run.file.parent()) {
            let _ = crate::issue_recheck::dismiss(state, fixed, crate::messages::now());
            self.fixed_seen = self.fixed_seen.max(fixed.last);
            self.fixed = self.fixed.take().filter(|shown| shown.last > self.fixed_seen);
        }
        self.restart_after_update = succeeded;
        let state = run.file.parent().map(Path::to_path_buf);
        self.update = None;
        if let (true, Some(state)) = (succeeded, state) {
            self.offer_agent_updates(&state);
        }
    }

    pub(super) fn update_pill(&self) -> Option<String> {
        let run = self.update.as_ref().filter(|_| !matches!(self.mode, Mode::Update))?;
        let key = crate::keys::map().label(crate::keys::Act::Update);
        Some(if run.running() { format!("⇣ updating kebbie · {key}") } else { format!("✓ kebbie updated · {key}") })
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
        let line = |label: &str, said: &str| crate::restart::Line { label: label.to_string(), said: said.to_string(), stale: true, reasons: Vec::new() };
        Summary { version: "main".to_string(), new: owned(new), fixed: owned(fixed), restart: vec!["The lead and 2 sessions restart when they're idle".to_string(), "Open boards restart on the new kebbie when you next open them".to_string()], sessions: vec![line("lead", "restarts when idle"), line("billing@app", "restarts when idle"), line("api@app", "restarts when idle")], restarted_at: 1000, head: "9cbd545".to_string(), from: "02c7fe0".to_string() }
    }

    fn updating(app: &mut App, finish: Summary) -> mpsc::Sender<()> {
        let file = crate::sh::test_dir("board-update").join(progress::FILE);
        app.progress_file = Some(file.clone());
        let (release, gate) = mpsc::channel::<()>();
        let gate = std::sync::Mutex::new(gate);
        app.updater = std::sync::Arc::new(move || {
            let mut tracker = Tracker::start(&file, "Fetching main", "Build kebbie", crate::messages::now());
            tracker.done(progress::FETCH, "Fetched main (3 new commits)");
            tracker.begin(progress::BUILD, "Building kebbie", crate::messages::now());
            tracker.meter(Meter::Build { done: 212, total: Some(348), compiling: None });
            gate.lock().unwrap().recv().map_err(|error| error.to_string())?;
            tracker.done(progress::BUILD, "Built kebbie");
            tracker.done(progress::PLUGINS, "Updated the Claude Code and vault plugins");
            tracker.done(progress::RESTART, "Sessions move to the new kebbie");
            tracker.finish(finish.clone());
            Ok("kebbie follows the main channel\nkebbie main rebuilt, 02c7fe0 → 9cbd545\n\nClaude and Codex sessions:\n  lead: restarts when idle\nkeeper: restarted on the new kebbie (pid 4242)".to_string())
        });
        release
    }

    #[test]
    fn u_shows_the_steps_with_the_build_bar_and_esc_hides_the_window_while_the_update_keeps_going() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        let release = updating(&mut app, summary(&["The hub window shows your devices by name, and r removes one"], &[]));
        app.updates = vec![("kebbie".to_string(), "main, 3 new".to_string())];
        press(&mut app, key(KeyCode::Char('U')));
        assert!(matches!(app.mode, Mode::Update), "the key returns while the update runs");
        waits_for(&mut app, "the build step", |app| progress::load(&app.progress_file()).is_some_and(|progress| progress.meter.is_some()));
        let shown = screen(&mut app, 100);
        assert!(shown.lines().any(|line| line.contains('█') && line.contains('%') && !line.contains("crates")), "one bar for the whole update above the steps: {shown}");
        for line in ["Updating kebbie", "✓ Fetched main (3 new commits)", "▸ Building kebbie  ████████████░░░░░░░░  212/348 crates 0:0", "  Claude Code and vault plugins", "  Restart sessions", "esc hides this; the update keeps going"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Update), "only esc hides it");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal) && app.update.as_ref().is_some_and(Run::running), "hidden, still running");
        assert!(crate::board::header::header(&app, 200).0.to_string().contains("⇣ updating kebbie · U"), "{}", crate::board::header::header(&app, 200).0);
        press(&mut app, key(KeyCode::Char('U')));
        assert!(matches!(app.mode, Mode::Update), "U opens the window again, not a second update");
        press(&mut app, key(KeyCode::Esc));
        release.send(()).unwrap();
        waits_for(&mut app, "the outcome", |app| app.update.as_ref().is_some_and(|run| !run.running()));
        assert!(matches!(app.mode, Mode::Update) && app.updates.is_empty(), "the summary opens by itself once the board is at rest");
    }

    fn closed_by_recheck(state: &Path, id: &str, rechecked: String) {
        let issue = crate::issues::Issue { id: id.to_string(), title: format!("bug {id}"), kind: "bug".to_string(), status: "fixed".to_string(), rechecked: crate::issue_recheck::Rechecked::parse(&rechecked), ..crate::issues::Issue::default() };
        crate::issues::save(&crate::issues::dir(state), &issue).unwrap();
    }

    #[test]
    fn the_finished_window_shows_the_notes_the_bugs_this_update_closed_and_the_restarts_with_the_rest_behind_d() {
        let mut app = test_app(Vec::new());
        let release = updating(&mut app, summary(&["The hub window shows your devices by name, and r removes one"], &[]));
        let state = app.progress_file().parent().unwrap().to_path_buf();
        let now = crate::messages::now();
        closed_by_recheck(&state, "earlier", format!("1111111 fixed {} 0.27.0", now - 3600));
        press(&mut app, key(KeyCode::Char('U')));
        release.send(()).unwrap();
        waits_for(&mut app, "the outcome", |app| app.update.as_ref().is_some_and(|run| !run.running()));
        closed_by_recheck(&state, "ours", format!("9cbd545 fixed {} 0.27.0", crate::messages::now()));
        app.update.as_mut().unwrap().fixed_read = None;
        app.poll_update();
        let shown = screen(&mut app, 100);
        for line in ["kebbie main · 02c7fe0 → 9cbd545", "✓ Updated", "What's new", " • The hub window shows your devices by name, and r removes", "   one", "Fixed", " • A bug you ran into: bug ours", "Restarting", " The lead and 2 sessions restart when they're idle", "d details · any other key closes"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        assert!(!shown.contains("bug earlier"), "a bug an earlier update closed was shown then: {shown}");
        assert!(!shown.contains("board") && !shown.contains("Open boards"), "the board restarts itself once this closes, so it names no board: {shown}");
        assert!(!shown.contains("pid") && !shown.contains("rebuilt") && !shown.contains("lead: restarts"), "the detail lines wait behind d: {shown}");
        press(&mut app, key(KeyCode::Char('d')));
        let shown = screen(&mut app, 100);
        assert!(shown.contains("kebbie main rebuilt, 02c7fe0 → 9cbd545") && shown.contains("keeper: restarted on the new kebbie (pid 4242)") && shown.contains("d back · any other key closes"), "{shown}");
        press(&mut app, key(KeyCode::Char('d')));
        assert!(screen(&mut app, 100).contains("What's new"), "d again goes back");
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Normal) && app.update.is_none(), "any other key closes it");
        assert!(app.restart_after_update, "closing the summary lets the board restart onto the new kebbie at once");
        assert!(crate::issue_recheck::for_update(&state, 0, "9cbd545", crate::messages::now()).is_none(), "a bug the summary showed is never shown again");
    }

    #[test]
    fn a_changelog_longer_than_the_screen_scrolls_under_a_keys_line_that_stays_and_the_wheel_never_closes_it() {
        let mut app = test_app(Vec::new());
        let fixed: Vec<String> = (1..=24).map(|n| format!("Fixed entry number {n}")).collect();
        let fixed: Vec<&str> = fixed.iter().map(String::as_str).collect();
        let release = updating(&mut app, summary(&["The hub window shows your devices by name"], &fixed));
        press(&mut app, key(KeyCode::Char('U')));
        release.send(()).unwrap();
        waits_for(&mut app, "the outcome", |app| app.update.as_ref().is_some_and(|run| !run.running()));
        let shown = render(&mut app, 80, 20).0.join("\n");
        assert!(shown.contains("↑ ↓ scroll · d details · any other key closes · ↓ more") && shown.contains("✓ Updated") && !shown.contains("Fixed entry number 24"), "{shown}");
        for _ in 0..40 {
            press(&mut app, key(KeyCode::Down));
        }
        assert!(matches!(app.mode, Mode::Update), "↓ scrolls instead of closing");
        let shown = render(&mut app, 80, 20).0.join("\n");
        assert!(shown.contains("Fixed entry number 24") && shown.contains("The lead and 2 sessions restart") && !shown.contains("✓ Updated"), "{shown}");
        assert!(shown.contains("↑ ↓ scroll · d details · any other key closes") && !shown.contains("↓ more"), "{shown}");
        press(&mut app, key(KeyCode::Home));
        let wheel = |app: &mut App| app.work_mouse(ratatui::crossterm::event::MouseEvent { kind: ratatui::crossterm::event::MouseEventKind::ScrollDown, column: 40, row: 10, modifiers: ratatui::crossterm::event::KeyModifiers::NONE }, None);
        for _ in 0..3 {
            if let Some(scrolled) = wheel(&mut app) {
                press(&mut app, scrolled);
            }
        }
        assert!(matches!(app.mode, Mode::Update) && app.update.is_some(), "the wheel scrolls and keeps the window open");
        let shown = render(&mut app, 80, 20).0.join("\n");
        assert!(!shown.contains("✓ Updated") && shown.contains("↓ more"), "the wheel moved the body: {shown}");
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Normal) && app.update.is_none(), "any other key closes it");
    }

    #[test]
    fn the_title_names_the_commits_on_main_and_the_version_on_release() {
        let title = |summary: Summary| finished(Some(&Progress { summary: Some(summary), ..Progress::default() }), &[], &[], 80).0;
        assert_eq!(title(summary(&[], &[])), "kebbie main · 02c7fe0 → 9cbd545");
        assert_eq!(title(Summary { from: String::new(), ..summary(&[], &[]) }), "kebbie main", "without the old commit the title names main alone");
        assert_eq!(title(Summary { version: "v0.28.0".to_string(), from: String::new(), ..summary(&[], &[]) }), "kebbie v0.28.0", "the release channel keeps its version");
        assert_eq!(title(Summary::default()), "kebbie");
    }

    #[test]
    fn the_restarts_follow_the_rounds_after_the_update_while_the_window_is_open() {
        let mut app = test_app(Vec::new());
        let shown = summary(&[], &[]);
        app.stale = vec!["lead: restarts when idle".to_string()];
        app.stale_at = 999;
        assert_eq!(app.restart_lines(&shown), ["The lead and 2 sessions restart when they're idle"], "a round from before the update says nothing about it");
        app.stale_at = 1010;
        assert_eq!(app.restart_lines(&shown), ["✓ 2 sessions are on the new kebbie", "The lead restarts when it's idle"]);
        app.stale = Vec::new();
        assert_eq!(app.restart_lines(&shown), ["✓ The lead and 2 sessions are on the new kebbie"]);
        let old = Summary { sessions: Vec::new(), ..summary(&[], &[]) };
        assert_eq!(app.restart_lines(&old), ["The lead and 2 sessions restart when they're idle"], "a summary an older kebbie wrote keeps its words, without the board");
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
            let mut tracker = Tracker::start(&file, "Fetching main", "Build kebbie", crate::messages::now());
            tracker.done(progress::FETCH, "Fetched main (1 new commit)");
            tracker.begin(progress::BUILD, "Building kebbie", crate::messages::now());
            tracker.fail_current();
            Err("sh scripts/build.sh failed: error[E0425]: cannot find value `x`".to_string())
        });
        press(&mut app, key(KeyCode::Char('U')));
        waits_for(&mut app, "the outcome", |app| app.update.as_ref().is_some_and(|run| !run.running()));
        let shown = screen(&mut app, 100);
        assert!(shown.contains("✗ The update stopped") && shown.contains("sh scripts/build.sh failed") && shown.contains("✗ Building kebbie") && shown.contains("  Restart sessions"), "{shown}");
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
