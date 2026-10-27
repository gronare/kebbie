use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::evidence;
use crate::report;
use crate::tasks;
use super::card::{indented, push_card_lines};
use super::detail::tests_spans;
use super::editor::{MessageEditor, Rewrite};
use super::{App, Hit, Mode};
use super::steps::mark_color;
use super::widgets::{GREEN, MAUVE, MUTED, SURFACE, TEXT, YELLOW, bold, centered, cut, fit, modal_block, plural, rule, section_header, style, text_width, wrap};

// What the push window marks on its commits: which the user accepted, their steps, the steps not committed yet, and the last of the accepted run.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Marks {
    accepted: Vec<bool>,
    steps: Vec<Option<tasks::StepState>>,
    numbers: Vec<Option<usize>>,
    stays: Vec<tasks::Step>,
    through: Option<usize>,
    working: bool,
}

fn committed_step<'a>(steps: &'a tasks::Steps, sha: &str) -> Option<&'a tasks::Step> {
    steps.items.iter().find(|step| matches!(&step.state, tasks::StepState::Committed { sha: own, .. } if own == sha))
}

impl Marks {
    pub(super) fn of(commits: &[tasks::Commit], accepted: Vec<bool>, steps: &tasks::Steps, last: Option<&str>) -> Self {
        let step = |commit: &tasks::Commit| committed_step(steps, &commit.sha);
        Self {
            steps: commits.iter().map(|commit| step(commit).map(|step| step.state.clone())).collect(),
            numbers: commits.iter().map(|commit| step(commit).map(|step| step.index + 1)).collect(),
            stays: steps.items.iter().filter(|step| !matches!(step.state, tasks::StepState::Committed { .. })).cloned().collect(),
            through: last.and_then(|last| commits.iter().position(|commit| commit.sha == last)),
            accepted,
            working: false,
        }
    }

    // While its Claude works, the window pushes only the accepted run and rewrites nothing.
    pub(super) fn while_working(self, working: bool) -> Self {
        Self { working, ..self }
    }

    pub(super) fn through(&self) -> Option<usize> {
        self.through
    }

    pub(super) fn last_pushable(&self, commits: usize) -> usize {
        if self.working { self.through.unwrap_or(0) } else { commits.saturating_sub(1) }
    }

    // Reads the task's state, so only the board's own keys call it, never a test.
    pub(super) fn read(path: &Path, commits: &[tasks::Commit], working: bool) -> Self {
        let steps = tasks::steps_for(path, working);
        let marked = tasks::accepted_in(path);
        let accepted = commits.iter().map(|commit| match committed_step(&steps, &commit.sha).map(|step| &step.state) {
            Some(tasks::StepState::Committed { accepted, .. }) => *accepted,
            _ => marked.iter().any(|(sha, accepted)| *sha == commit.sha && *accepted),
        }).collect();
        Self::of(commits, accepted, &steps, tasks::last_of_run(&marked))
    }

    fn any_accepted(&self) -> bool {
        self.accepted.contains(&true)
    }

    fn stepped(&self) -> bool {
        self.numbers.iter().any(Option::is_some) || !self.stays.is_empty()
    }

    pub(super) fn start(&self) -> usize {
        self.through.unwrap_or(0)
    }

    fn mark(&self, index: usize) -> Span<'static> {
        match self.steps.get(index).cloned().flatten() {
            Some(state) => Span::styled(format!("{} ", tasks::step_mark(&state)), style(mark_color(&state))),
            None if self.accepted.get(index) == Some(&true) => Span::styled("✓ ", style(GREEN)),
            None => Span::raw("  "),
        }
    }

    fn number_width(&self) -> usize {
        self.numbers.iter().flatten().copied().chain(self.stays.iter().map(|step| step.index + 1)).map(|number| number.to_string().len()).max().unwrap_or(0)
    }

    fn number(&self, number: Option<usize>) -> Span<'static> {
        let shown = number.map(|number| number.to_string()).unwrap_or_default();
        Span::styled(format!("{shown:>width$} ", width = self.number_width()), style(MUTED))
    }
}

const THROUGH: &str = "  ◂ through here";

fn commit_line(commit: &tasks::Commit, index: usize, active: bool, marks: &Marks, width: usize) -> Line<'static> {
    let subject = commit.message.lines().next().unwrap_or_default().to_string();
    let mut spans = vec![Span::raw(if active { "▸ " } else { "  " })];
    if marks.any_accepted() || marks.stepped() {
        spans.push(marks.mark(index));
    }
    if marks.stepped() {
        spans.push(marks.number(marks.numbers.get(index).copied().flatten()));
    }
    spans.extend([Span::styled(commit.sha.chars().take(7).collect::<String>(), style(MUTED)), Span::raw(" ")]);
    let through = marks.through == Some(index);
    let used: usize = spans.iter().map(Span::width).sum::<usize>() + if through { text_width(THROUGH) } else { 0 };
    let subject = if through { cut(&subject, width.saturating_sub(used)) } else { subject };
    spans.push(Span::styled(subject, if active { bold(MAUVE) } else { style(TEXT) }));
    if through {
        spans.push(Span::styled(THROUGH, style(GREEN)));
    }
    fit(spans, width).style(if active { Style::new().bg(SURFACE) } else { Style::new() })
}

// A step not committed yet, under the commits, so the user sees what stays.
fn stays_line(step: &tasks::Step, marks: &Marks, width: usize) -> Line<'static> {
    let what = if step.state == tasks::StepState::InProgress { "in progress, stays" } else { "planned" };
    fit(vec![
        Span::raw("  "), Span::styled(format!("{} ", tasks::step_mark(&step.state)), style(MUTED)), marks.number(Some(step.index + 1)), Span::raw(" ".repeat(8)),
        Span::styled(step.title.clone(), style(MUTED)), Span::styled(format!("  {what}"), style(MUTED)),
    ], width)
}

// The push window's body at a width, and the line its first commit is on; the keys are drawn under it by `push_keys`.
pub(super) fn push_lines(card: Option<&(report::Card, bool)>, tests: Option<&evidence::Shown>, commits: &[tasks::Commit], marks: &Marks, selected: usize, width: usize) -> (Vec<Line<'static>>, usize) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some((card, current)) = card {
        lines.extend(push_card_lines(card, *current, width));
        lines.push(Line::default());
    }
    if let Some(tests) = tests {
        lines.push(section_header("TESTS", Vec::new(), width));
        lines.push(indented(Line::from(tests_spans(tests, width.saturating_sub(4))), 2));
        lines.push(Line::default());
    }
    lines.push(section_header("COMMITS", vec![Span::styled(commits.len().to_string(), style(MUTED))], width));
    let first_commit = lines.len();
    for (index, commit) in commits.iter().enumerate() {
        lines.push(commit_line(commit, index, index == selected, marks, width));
    }
    lines.extend(marks.stays.iter().map(|step| stays_line(step, marks, width)));
    if let Some(commit) = commits.get(selected) {
        lines.push(Line::default());
        lines.push(section_header("MESSAGE", vec![Span::styled(commit.sha.chars().take(7).collect::<String>(), style(MUTED))], width));
        let bar = || Span::styled("▎ ", style(MAUVE));
        for line in commit.message.lines() {
            let wrapped = if line.is_empty() { vec![String::new()] } else { wrap(line, width.saturating_sub(4), 6) };
            lines.extend(wrapped.into_iter().map(|part| Line::from(vec![Span::raw("  "), bar(), Span::styled(part, style(TEXT))])));
        }
    }
    (lines, first_commit)
}

pub(super) fn push_keys(commits: usize, selected: usize, through: Option<usize>, scrolls: bool, working: bool) -> Line<'static> {
    if working {
        let mut keys = format!("p push {} accepted, the task keeps working", selected + 1);
        if through.is_some_and(|through| through > 0) {
            keys.push_str(" · ↑ ↓ commit");
        }
        if scrolls {
            keys.push_str(" · pgup pgdn scroll");
        }
        keys.push_str(" · esc cancel");
        return Line::from(vec![Span::styled("Push?  ", style(YELLOW)), Span::styled(keys, style(MUTED))]);
    }
    let keep = if through == Some(selected) {
        format!("p push {} accepted, the task keeps working", selected + 1)
    } else if commits > 1 {
        format!("p push the first {} and keep it open", selected + 1)
    } else {
        "p push and keep it open".to_string()
    };
    let mut keys = if commits > 1 {
        format!("y push all and close · {keep} · e edit message · s squash into one · ↑ ↓ commit")
    } else {
        format!("y push and close · {keep} · e edit message")
    };
    if scrolls {
        keys.push_str(" · pgup pgdn scroll");
    }
    keys.push_str(" · esc cancel");
    Line::from(vec![Span::styled("Push?  ", style(YELLOW)), Span::styled(keys, style(MUTED))])
}

// Where the push window's body is scrolled to; free once PgUp, PgDn or the wheel moved it, else it follows the selected commit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Scroll {
    top: u16,
    free: bool,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_push(&mut self, path: PathBuf, commits: Vec<tasks::Commit>, selected: usize, (card, tests, marks): (Option<(report::Card, bool)>, Option<evidence::Shown>, Marks), scroll: Scroll, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y' | 'e' | 's') if marks.working => self.mode = Mode::ConfirmPush { path, commits, selected, card, tests, marks, scroll },
            KeyCode::Char(key @ ('y' | 'p')) => {
                let keep_open = key == 'p';
                let through = if keep_open { commits.get(selected) } else { commits.last() };
                let Some(through) = through.map(|commit| commit.sha.clone()) else { return };
                let count = if keep_open { selected + 1 } else { commits.len() };
                let (sender, result) = mpsc::channel();
                let path_for_subject = path.clone();
                thread::spawn(move || {
                    let _ = sender.send(tasks::push(&path, None, Some(&through), keep_open));
                });
                let label = if keep_open { format!("Rebasing and pushing {}; the task stays open…", plural(count, "commit")) } else { format!("Rebasing and pushing {}, then closing the task…", plural(count, "commit")) };
                self.about_path("push", &path_for_subject);
                self.mode = Mode::Working { label, result };
            }
            KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::ConfirmPush { path, commits, selected: selected.saturating_sub(1), card, tests, marks, scroll: scroll.follow() },
            KeyCode::Down | KeyCode::Char('j') => {
                let selected = (selected + 1).min(marks.last_pushable(commits.len()));
                self.mode = Mode::ConfirmPush { path, commits, selected, card, tests, marks, scroll: scroll.follow() };
            }
            KeyCode::PageUp => self.mode = Mode::ConfirmPush { path, commits, selected, card, tests, marks, scroll: scroll.by(-10) },
            KeyCode::PageDown => self.mode = Mode::ConfirmPush { path, commits, selected, card, tests, marks, scroll: scroll.by(10) },
            KeyCode::Char('s') if commits.len() > 1 => {
                let (sender, receiver) = mpsc::channel();
                let draft_path = path.clone();
                thread::spawn(move || {
                    let _ = sender.send(tasks::draft_squash_message(&draft_path));
                });
                let mut editor = MessageEditor::new(path, commits, (0, Scroll::default()), (card, tests, marks), Rewrite::Squash, "");
                editor.text.set_placeholder_text("Claude is drafting a message from the commits and the diff…");
                editor.draft = Some(receiver);
                self.mode = Mode::EditMessage(Box::new(editor));
            }
            KeyCode::Char('e') => {
                let Some(commit) = commits.get(selected) else { return };
                let (sha, message) = (commit.sha.clone(), commit.message.clone());
                self.mode = Mode::EditMessage(Box::new(MessageEditor::new(path, commits, (selected, scroll), (card, tests, marks), Rewrite::Reword(sha), &message)));
            }
            _ => {}
        }
    }
}

impl Scroll {
    pub(super) fn follow(self) -> Self {
        Self { free: false, ..self }
    }

    pub(super) fn by(self, rows: i32) -> Self {
        Self { top: (i32::from(self.top) + rows).clamp(0, i32::from(u16::MAX)) as u16, free: true }
    }

    fn settle(self, selected: u16, header: u16, height: u16, last: u16) -> Self {
        let mut top = self.top;
        if !self.free {
            top = top.min(selected).max((selected + 1).saturating_sub(height));
            if selected - header < height {
                top = top.min(header);
            }
        }
        Self { top: top.min(last), ..self }
    }
}

const PUSH_WIDTH: u16 = 96;

pub(super) fn draw_push(frame: &mut Frame, app: &mut App) -> Vec<(Rect, Hit)> {
    let Mode::ConfirmPush { commits, selected, card, tests, marks, scroll, .. } = &mut app.mode else { return Vec::new() };
    let width = PUSH_WIDTH.min(frame.area().width.saturating_sub(4));
    let inner_width = width.saturating_sub(4);
    let (lines, first_commit) = push_lines(card.as_ref(), tests.as_ref(), commits, marks, *selected, inner_width as usize);
    super::link::note_lines(&lines);
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut rows = 0u16;
    for line in &lines {
        starts.push(rows);
        rows = rows.saturating_add(Paragraph::new(line.clone()).wrap(Wrap { trim: false }).line_count(inner_width) as u16);
    }
    starts.push(rows);
    let body_rows = rows;
    let keys_rows = |scrolls: bool| Paragraph::new(push_keys(commits.len(), *selected, marks.through, scrolls, marks.working)).wrap(Wrap { trim: false }).line_count(inner_width) as u16;
    let room = frame.area().height.saturating_sub(4);
    let scrolls = body_rows + 1 + keys_rows(false) > room;
    let keys = push_keys(commits.len(), *selected, marks.through, scrolls, marks.working);
    let foot_rows = keys_rows(scrolls);
    let area = centered(frame.area(), width, body_rows + 1 + foot_rows + 2);
    let block = modal_block("push".to_string());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let [body, line, foot] = Layout::vertical([Constraint::Min(0), Constraint::Length(1), Constraint::Length(foot_rows)]).areas(inner);
    let commit_row = |index: usize| starts[first_commit + index];
    *scroll = scroll.settle(commit_row(*selected), commit_row(0).saturating_sub(1), body.height, body_rows.saturating_sub(body.height));
    let top = scroll.top;
    frame.render_widget(Paragraph::new(lines.clone()).wrap(Wrap { trim: false }).scroll((top, 0)), body);
    frame.render_widget(Paragraph::new(rule(inner_width as usize)), line);
    frame.render_widget(Paragraph::new(keys).wrap(Wrap { trim: false }), foot);
    let mut hits = Vec::new();
    for index in 0..commits.len() {
        let Some(row) = commit_row(index).checked_sub(top) else { continue };
        if row < body.height {
            hits.push((Rect::new(area.x, body.y + row, area.width, 1), Hit::Commit(index)));
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::text::Line;

    use super::*;
    use crate::board::widgets::{GREEN, MUTED, YELLOW};
    use crate::evidence;
    use crate::report;
    use crate::tasks;

    #[test]
    fn the_push_dialog_parts_the_card_tests_commits_and_message_under_their_own_headers() {
        let text = |lines: &[Line]| lines.iter().map(|line| line.spans.iter().map(|span| span.content.to_string()).collect::<String>()).collect::<Vec<_>>().join("\n");
        let commits = vec![tasks::Commit { sha: "1111111aaaa".to_string(), message: "Add refunds\n\nso a customer gets money back".to_string() }, tasks::Commit { sha: "2222222bbbb".to_string(), message: "Show refunds".to_string() }];
        let card = report::Card { sha: "abcdef0123".to_string(), text: "Refunds show on the **billing** page.\n\n![the refund row](/tmp/kebbie-shots/refund.png)\nTo check: 30 days.".to_string(), file: PathBuf::from("/state/cards/billing@web.md") };
        let tests = evidence::Shown { mark: evidence::Mark::Covered, text: "cargo test · 412 passed · at abcdef0".to_string() };
        let (lines, first) = push_lines(Some(&(card, true)), Some(&tests), &commits, &Marks::default(), 0, 60);
        let screen = text(&lines);
        let rule = |head: &str| format!("{head} {}", "─".repeat(60 - head.chars().count() - 1));
        assert!(screen.starts_with(&format!("{}\n  Refunds show on the billing page.\n\n  To check: 30 days.\n  ▣ refund.png  the refund row\n\n", rule("CARD ✓ current at abcdef0"))), "{screen}");
        assert!(lines[1].spans.iter().any(|span| span.content == "billing" && span.style.add_modifier.contains(ratatui::style::Modifier::BOLD)), "**billing** is bold without its stars");
        assert!(!screen.contains("/tmp/kebbie-shots"), "an image shows by its file name: {screen}");
        assert!(screen.contains(&format!("{}\n  ✓ cargo test · 412 passed · at abcdef0\n\n{}\n", rule("TESTS"), rule("COMMITS 2"))), "{screen}");
        assert!(text(&lines[first..first + 1]).contains("▸ 1111111 Add refunds"), "the first commit is where the mouse hits are counted from");
        assert!(screen.ends_with(&format!("{}\n  ▎ Add refunds\n  ▎ \n  ▎ so a customer gets money back", rule("MESSAGE 1111111"))), "{screen}");
        let bar = lines.iter().find(|line| line.to_string().contains("so a customer")).unwrap();
        assert_eq!(bar.spans[1].style.fg, Some(MAUVE), "the message hangs off a mauve bar");
        let (lines, _) = push_lines(None, None, &commits, &Marks::default(), 1, 60);
        let screen = text(&lines);
        assert!(screen.contains("MESSAGE 2222222") && screen.contains("  ▎ Show refunds") && !screen.contains("money back"), "the message follows the selection: {screen}");
        assert!(push_keys(2, 1, None, false, false).to_string().contains("p push the first 2 and keep it open"));
        let stale = report::Card { sha: "abcdef0123".to_string(), text: (0..40).map(|index| format!("line {index}")).collect::<Vec<_>>().join("\n"), file: PathBuf::from("/state/cards/billing@web.md") };
        let (lines, _) = push_lines(Some(&(stale, false)), None, &commits, &Marks::default(), 0, 60);
        let screen = text(&lines);
        assert!(screen.contains("CARD ○ written at abcdef0, code changed since") && screen.contains("  … the rest in /state/cards/billing@web.md") && !screen.contains("line 20"), "{screen}");
        let (lines, first) = push_lines(None, None, &commits[..1], &Marks::default(), 0, 60);
        assert_eq!(first, 1, "under the COMMITS header");
        assert_eq!(lines.len(), 7, "the body ends with the message; the keys are drawn apart");
        assert_eq!(push_keys(1, 0, None, false, false).to_string(), "Push?  y push and close · p push and keep it open · e edit message · esc cancel");
    }

    fn pushing(app: &mut crate::board::App, commits: usize, card_lines: usize) {
        let commits = (0..commits).map(|index| tasks::Commit { sha: format!("{index:07}abc"), message: format!("Commit number {index}\n\nthe body of {index}") }).collect();
        let text = (0..card_lines).map(|index| format!("card line {index}")).collect::<Vec<_>>().join("\n");
        let card = report::Card { sha: "abcdef0123".to_string(), text, file: PathBuf::from("/state/cards/x.md") };
        app.mode = crate::board::Mode::ConfirmPush { path: PathBuf::from("/work/wt2.web"), commits, selected: 0, card: Some((card, true)), tests: None, marks: Marks::default(), scroll: Scroll::default() };
    }

    #[test]
    fn a_tall_push_window_scrolls_its_body_under_keys_that_stay_at_the_bottom() {
        use ratatui::crossterm::event::KeyCode;
        use crate::board::test_support::*;
        let mut app = test_app(vec![served("search-filters")]);
        pushing(&mut app, 60, 18);
        let (rows, _) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("Push?") && screen.contains("esc cancel") && screen.contains("pgup pgdn scroll"), "{screen}");
        assert!(screen.contains("card line 0") && screen.contains("0000000 Commit number 0"), "{screen}");
        for _ in 0..40 {
            press(&mut app, key(KeyCode::Down));
        }
        let (rows, buffer) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("Push?") && screen.contains("esc cancel"), "the keys stay on screen: {screen}");
        let (x, y) = find(&rows, "▸ 0000040 Commit number 40")[0];
        assert_eq!(buffer[(x as u16 + 10, y as u16)].bg, SURFACE, "the selected commit is highlighted");
        assert!(!screen.contains("0000000 ") && !screen.contains("card line 0"), "the top scrolled away: {screen}");
        let keys_row = find(&rows, "Push?")[0].1;
        assert!(y < keys_row && rows[keys_row - 1].contains("────"), "the commit sits above the rule and the keys: {screen}");
        let (x, y) = find(&rows, "0000035 Commit number 35")[0];
        assert_eq!(click(&mut app, x as u16, y as u16), None);
        assert!(matches!(app.mode, crate::board::Mode::ConfirmPush { selected: 35, .. }), "a click on a scrolled commit selects that commit");
        let (rows, _) = render(&mut app, 100, 30);
        assert_eq!(find(&rows, "▸ 0000035 Commit number 35")[0].1, y, "a click on a visible commit does not move the list");
        for _ in 0..3 {
            press(&mut app, key(KeyCode::PageDown));
        }
        let (rows, _) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("MESSAGE 0000035") && screen.contains("the body of 35") && screen.contains("Push?"), "pgdn reads on to the message: {screen}");
        let wheel = ratatui::crossterm::event::MouseEvent { kind: ratatui::crossterm::event::MouseEventKind::ScrollUp, column: 50, row: 10, modifiers: ratatui::crossterm::event::KeyModifiers::NONE };
        assert_eq!(app.work_mouse(wheel, None), None);
        let (wheeled, _) = render(&mut app, 100, 30);
        assert_eq!(find(&wheeled, "MESSAGE 0000035")[0].1, find(&rows, "MESSAGE 0000035")[0].1 + 3, "the wheel scrolls the body, not the selection");
        assert!(matches!(app.mode, crate::board::Mode::ConfirmPush { selected: 35, .. }));
        press(&mut app, key(KeyCode::Up));
        let (rows, _) = render(&mut app, 100, 30);
        assert!(!find(&rows, "▸ 0000034 Commit number 34").is_empty(), "moving the selection brings it back into view: {}", rows.join("\n"));
    }

    #[test]
    fn five_hundred_commits_render_with_hits_on_the_rows_they_are_drawn_on() {
        use ratatui::crossterm::event::KeyCode;
        use crate::board::test_support::*;
        let mut app = test_app(vec![served("search-filters")]);
        pushing(&mut app, 500, 18);
        for _ in 0..450 {
            press(&mut app, key(KeyCode::Down));
        }
        let (rows, _) = render(&mut app, 100, 30);
        let (x, y) = find(&rows, "▸ 0000450 Commit number 450")[0];
        assert_eq!(app.hit_at(x as u16, y as u16), Some(crate::board::Hit::Commit(450)));
        let (x, y) = find(&rows, "0000440 Commit number 440")[0];
        assert_eq!(app.hit_at(x as u16, y as u16), Some(crate::board::Hit::Commit(440)), "{}", rows.join("\n"));
    }

    #[test]
    fn a_wide_character_subject_takes_one_row() {
        use crate::board::test_support::*;
        let mut app = test_app(vec![served("search-filters")]);
        pushing(&mut app, 3, 1);
        if let crate::board::Mode::ConfirmPush { commits, .. } = &mut app.mode {
            commits[0].message = "退款".repeat(40);
        }
        let (rows, _) = render(&mut app, 100, 30);
        let y = find(&rows, "▸ 0000000 退")[0].1;
        assert!(rows[y].contains("…") && rows[y + 1].contains("0000001 Commit number 1"), "{}", rows.join("\n"));
        assert_eq!(app.hit_at(10, (y + 1) as u16), Some(crate::board::Hit::Commit(1)));
    }

    #[test]
    fn editing_a_message_and_backing_out_keeps_the_view() {
        use ratatui::crossterm::event::KeyCode;
        use crate::board::test_support::*;
        let mut app = test_app(vec![served("search-filters")]);
        pushing(&mut app, 60, 18);
        for _ in 0..30 {
            press(&mut app, key(KeyCode::Down));
        }
        press(&mut app, key(KeyCode::PageDown));
        let (before, _) = render(&mut app, 100, 30);
        press(&mut app, key(KeyCode::Char('e')));
        assert!(matches!(app.mode, crate::board::Mode::EditMessage(_)));
        press(&mut app, key(KeyCode::Esc));
        let (after, _) = render(&mut app, 100, 30);
        assert_eq!(before, after);
    }

    #[test]
    fn on_a_short_screen_the_commits_and_keys_win_over_the_card() {
        use crate::board::test_support::*;
        let mut app = test_app(vec![served("search-filters")]);
        pushing(&mut app, 5, 18);
        let (rows, _) = render(&mut app, 100, 14);
        let screen = rows.join("\n");
        assert!(screen.contains("▸ 0000000 Commit number 0") && screen.contains("COMMITS 5") && screen.contains("Push?"), "{screen}");
        assert!(!screen.contains("card line 0"), "{screen}");
    }

    #[test]
    fn a_short_push_window_shows_everything_with_the_keys_under_the_message() {
        use crate::board::test_support::*;
        let mut app = test_app(vec![served("search-filters")]);
        pushing(&mut app, 2, 1);
        let (rows, _) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        let message = find(&rows, "the body of 0")[0].1;
        assert!(rows[message + 1].contains("────") && rows[message + 2].contains("Push?  y push all and close") && rows[message + 3].contains("esc cancel") && rows[message + 4].contains("╰"), "{screen}");
        assert!(screen.contains("card line 0") && !screen.contains("pgup"), "{screen}");
    }

    fn three() -> Vec<tasks::Commit> {
        ["Charge through a PaymentGateway", "Add Stripe as the gateway", "Log the gateway's answers"].iter().enumerate().map(|(index, subject)| tasks::Commit { sha: format!("{0}{0}abcdef{0}{0}", index + 1), message: subject.to_string() }).collect()
    }

    fn window(marks: Marks) -> crate::board::App {
        use crate::board::test_support::*;
        let mut app = test_app(vec![served("search-filters")]);
        app.mode = crate::board::Mode::ConfirmPush { path: PathBuf::from("/work/wt2.web"), commits: three(), selected: marks.start(), card: None, tests: None, marks, scroll: Scroll::default() };
        app
    }

    #[test]
    fn accepted_commits_are_ticked_and_the_window_starts_on_the_last_of_the_accepted_run() {
        use ratatui::crossterm::event::KeyCode;
        use crate::board::test_support::*;
        let commits = three();
        let marks = Marks::of(&commits, vec![true, true, false], &tasks::Steps::default(), Some(&commits[1].sha));
        assert_eq!(marks.start(), 1);
        let mut app = window(marks);
        let (rows, buffer) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        let at = |text: &str| find(&rows, text).first().copied().unwrap_or_else(|| panic!("{text:?} in:\n{screen}"));
        let (x, y) = at("✓ 11abcde Charge through a PaymentGateway");
        assert_eq!(buffer[(x as u16, y as u16)].fg, GREEN);
        assert!(!rows[y].contains('▸') && !rows[y].contains("through here"), "{screen}");
        let (_, y) = at("▸ ✓ 22abcde Add Stripe as the gateway  ◂ through here");
        let (x, _) = at("◂ through here");
        assert_eq!(buffer[(x as u16, y as u16)].fg, GREEN);
        assert!(!find(&rows, "▸ ✓ 22abcde").is_empty() && !find(&rows, "    33abcde Log the gateway's answers").is_empty(), "an unaccepted commit has no mark: {screen}");
        assert!(screen.contains("p push 2 accepted, the task keeps working") && !screen.contains("p push the first"), "{screen}");
        press(&mut app, key(KeyCode::Down));
        let (rows, _) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("p push the first 3 and keep it open") && screen.contains("22abcde Add Stripe as the gateway  ◂ through here"), "the mark stays on the last accepted commit: {screen}");
        let gap = Marks::of(&commits, vec![false, true, false], &tasks::Steps::default(), None);
        assert_eq!(gap.start(), 0, "the first commit is not accepted, so the window starts at the top");
    }

    #[test]
    fn while_its_claude_works_the_window_pushes_only_the_accepted_run_and_rewrites_nothing() {
        use ratatui::crossterm::event::KeyCode;
        use crate::board::test_support::*;
        let commits = three();
        let marks = Marks::of(&commits, vec![true, true, false], &tasks::Steps::default(), Some(&commits[1].sha)).while_working(true);
        let mut app = window(marks);
        let (rows, _) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("Push?  p push 2 accepted, the task keeps working · ↑ ↓ commit · esc cancel"), "{screen}");
        assert!(!screen.contains("y push all") && !screen.contains("e edit message") && !screen.contains("s squash"), "{screen}");
        press(&mut app, key(KeyCode::Down));
        assert!(matches!(app.mode, crate::board::Mode::ConfirmPush { selected: 1, .. }), "the selection stops at the last accepted commit");
        for refused in ['y', 'e', 's'] {
            press(&mut app, key(KeyCode::Char(refused)));
            assert!(matches!(app.mode, crate::board::Mode::ConfirmPush { selected: 1, .. }), "{refused} leaves the window as it is while its Claude works");
        }
        press(&mut app, key(KeyCode::Up));
        let (rows, _) = render(&mut app, 100, 30);
        assert!(rows.join("\n").contains("p push 1 accepted, the task keeps working"));
    }

    #[test]
    fn without_accepted_commits_or_steps_the_window_is_as_before() {
        use crate::board::test_support::*;
        let commits = three();
        let mut plain = window(Marks::default());
        let mut read = window(Marks::of(&commits, vec![false; 3], &tasks::Steps::default(), None));
        let (before, _) = render(&mut plain, 100, 30);
        let (after, _) = render(&mut read, 100, 30);
        assert_eq!(before, after);
        let body = after.join("\n");
        let commits = &body[body.find("COMMITS").unwrap()..body.find("Push?").unwrap()];
        assert!(commits.contains("▸ 11abcde Charge through a PaymentGateway") && !commits.contains('✓') && !commits.contains("through here"), "{body}");
    }

    #[test]
    fn with_steps_each_commit_carries_its_step_and_the_steps_not_committed_show_what_stays() {
        use crate::board::test_support::*;
        use crate::board::steps::tests::committed;
        let commits = three();
        let step = |index: usize, title: &str, state: tasks::StepState| tasks::Step { index, title: title.to_string(), state, extra: false };
        let steps = tasks::Steps { pushed: 1, items: vec![
            step(1, "Charge through a PaymentGateway", committed(&commits[0].sha, true)),
            step(2, "Add Stripe as the gateway", committed(&commits[1].sha, false)),
            step(3, "Log the gateway's answers", committed(&commits[2].sha, false)),
            step(4, "Remove the old payment client", tasks::StepState::InProgress),
            step(5, "Document the gateway", tasks::StepState::Planned),
        ] };
        let mut app = window(Marks::of(&commits, vec![true, false, false], &steps, Some(&commits[0].sha)));
        let (rows, buffer) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        let at = |text: &str| find(&rows, text).first().copied().unwrap_or_else(|| panic!("{text:?} in:\n{screen}"));
        at("▸ ✓ 2 11abcde Charge through a PaymentGateway  ◂ through here");
        let (x, y) = at("● 3 22abcde Add Stripe as the gateway");
        assert_eq!(buffer[(x as u16, y as u16)].fg, YELLOW);
        at("● 4 33abcde Log the gateway's answers");
        let (x, y) = at("◐ 5         Remove the old payment client  in progress, stays");
        assert_eq!(buffer[(x as u16, y as u16)].fg, MUTED);
        assert_eq!(buffer[(x as u16 + 20, y as u16)].fg, MUTED, "what stays is muted whole");
        at("○ 6         Document the gateway  planned");
        assert!(at("◐ 5").1 > at("● 4 33abcde").1 && at("◐ 5").1 < at("MESSAGE").1, "the steps that stay sit under the commits: {screen}");
        assert!(screen.contains("p push 1 accepted, the task keeps working"), "{screen}");
        let (x, y) = at("22abcde Add Stripe");
        assert_eq!(app.hit_at(x as u16, y as u16), Some(crate::board::Hit::Commit(1)), "the lines that stay come after the commits, so the hits still land on them");
    }
}
