use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::evidence;
use crate::report;
use crate::tasks;
use super::card::card_lines;
use super::detail::tests_line;
use super::editor::{MessageEditor, Rewrite};
use super::{App, Hit, Mode};
use super::widgets::{MAUVE, MUTED, SURFACE, TEXT, YELLOW, bold, modal, plural, style, wrap};

// The push dialog's lines, and the line its first commit is on.
pub(super) fn push_lines(card: Option<&(report::Card, bool)>, tests: Option<&evidence::Shown>, commits: &[tasks::Commit], selected: usize) -> (Vec<Line<'static>>, usize) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some((card, current)) = card {
        lines.extend(card_lines(card, *current, 88));
        lines.push(Line::default());
    }
    if let Some(tests) = tests {
        lines.push(tests_line(tests, 88));
        lines.push(Line::default());
    }
    let first_commit = lines.len();
    for (index, commit) in commits.iter().enumerate() {
        let subject = commit.message.lines().next().unwrap_or_default().to_string();
        let active = index == selected;
        lines.push(Line::from(vec![
            Span::raw(if active { "▸ " } else { "  " }),
            Span::styled(commit.sha.chars().take(7).collect::<String>(), style(MUTED)),
            Span::raw(" "),
            Span::styled(subject, if active { bold(MAUVE) } else { style(TEXT) }),
        ]).style(if active { Style::new().bg(SURFACE) } else { Style::new() }));
    }
    if let Some(commit) = commits.get(selected) {
        lines.push(Line::default());
        for line in commit.message.lines() {
            let wrapped = if line.is_empty() { vec![String::new()] } else { wrap(line, 88, 6) };
            lines.extend(wrapped.into_iter().map(|part| Line::from(Span::styled(part, style(TEXT)))));
        }
    }
    lines.push(Line::default());
    let keys = if commits.len() > 1 {
        format!("y push all and close · p push the first {} and keep it open · e edit message · s squash into one · ↑ ↓ commit · esc cancel", selected + 1)
    } else {
        "y push and close · p push and keep it open · e edit message · esc cancel".to_string()
    };
    lines.push(Line::from(vec![Span::styled("Push?  ", style(YELLOW)), Span::styled(keys, style(MUTED))]));
    (lines, first_commit)
}

impl App {
    pub(super) fn handle_push(&mut self, path: PathBuf, commits: Vec<tasks::Commit>, selected: usize, card: Option<(report::Card, bool)>, tests: Option<evidence::Shown>, key: KeyEvent) {
        match key.code {
            KeyCode::Char(key @ ('y' | 'p')) => {
                let keep_open = key == 'p';
                let through = if keep_open { commits.get(selected) } else { commits.last() };
                let Some(through) = through.map(|commit| commit.sha.clone()) else { return };
                let count = if keep_open { selected + 1 } else { commits.len() };
                let (sender, result) = mpsc::channel();
                thread::spawn(move || {
                    let _ = sender.send(tasks::push(&path, None, Some(&through), keep_open));
                });
                let label = if keep_open { format!("Pushing {}; the task stays open…", plural(count, "commit")) } else { format!("Pushing {} and closing the task…", plural(count, "commit")) };
                self.mode = Mode::Working { label, result };
            }
            KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::ConfirmPush { path, commits, selected: selected.saturating_sub(1), card, tests },
            KeyCode::Down | KeyCode::Char('j') => {
                let selected = (selected + 1).min(commits.len().saturating_sub(1));
                self.mode = Mode::ConfirmPush { path, commits, selected, card, tests };
            }
            KeyCode::Char('s') if commits.len() > 1 => {
                let (sender, receiver) = mpsc::channel();
                let draft_path = path.clone();
                thread::spawn(move || {
                    let _ = sender.send(tasks::draft_squash_message(&draft_path));
                });
                let mut editor = MessageEditor::new(path, commits, 0, (card, tests), Rewrite::Squash, "");
                editor.text.set_placeholder_text("Claude is drafting a message from the commits and the diff…");
                editor.draft = Some(receiver);
                self.mode = Mode::EditMessage(Box::new(editor));
            }
            KeyCode::Char('e') => {
                let Some(commit) = commits.get(selected) else { return };
                let (sha, message) = (commit.sha.clone(), commit.message.clone());
                self.mode = Mode::EditMessage(Box::new(MessageEditor::new(path, commits, selected, (card, tests), Rewrite::Reword(sha), &message)));
            }
            _ => {}
        }
    }
}

pub(super) fn draw_push(frame: &mut Frame, commits: &[tasks::Commit], selected: usize, card: Option<&(report::Card, bool)>, tests: Option<&evidence::Shown>) -> Vec<(Rect, Hit)> {
    let mut modal_hits = Vec::new();
    let (lines, first_commit) = push_lines(card, tests, commits, selected);
    let area = modal(frame, "push".to_string(), lines, 96);
    for index in 0..commits.len() {
        let y = area.y + 1 + (first_commit + index) as u16;
        if y < area.bottom().saturating_sub(1) {
            modal_hits.push((Rect::new(area.x, y, area.width, 1), Hit::Commit(index)));
        }
    }
    modal_hits
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::text::Line;

    use super::*;
    use crate::report;
    use crate::tasks;

    #[test]
    fn the_push_dialog_shows_the_card_above_the_commits_with_its_images_as_paths() {
        let text = |lines: &[Line]| lines.iter().map(|line| line.spans.iter().map(|span| span.content.to_string()).collect::<String>()).collect::<Vec<_>>().join("\n");
        let commits = vec![tasks::Commit { sha: "1111111aaaa".to_string(), message: "Add refunds\n\nso a customer gets money back".to_string() }, tasks::Commit { sha: "2222222bbbb".to_string(), message: "Show refunds".to_string() }];
        let card = report::Card { sha: "abcdef0123".to_string(), text: "Refunds show on the billing page.\n\n![the refund row](/tmp/refund.png)\nConfirm: 30 days.".to_string(), file: PathBuf::from("/state/cards/billing@web.md") };
        let (lines, first) = push_lines(Some(&(card, true)), None, &commits, 1);
        let screen = text(&lines);
        assert!(screen.starts_with("card · written at abcdef0\nRefunds show on the billing page.\n\n▣ /tmp/refund.png  the refund row\nConfirm: 30 days.\n"), "{screen}");
        assert!(text(&lines[first..first + 1]).contains("1111111 Add refunds"), "the first commit is where the mouse hits are counted from");
        assert!(screen.contains("p push the first 2 and keep it open"), "{screen}");
        assert!(!screen.contains("the code changed since"));
        let stale = report::Card { sha: "abcdef0123".to_string(), text: (0..40).map(|index| format!("line {index}")).collect::<Vec<_>>().join("\n"), file: PathBuf::from("/state/cards/billing@web.md") };
        let (lines, _) = push_lines(Some(&(stale, false)), None, &commits, 0);
        let screen = text(&lines);
        assert!(screen.contains("the code changed since") && screen.contains("… the rest in /state/cards/billing@web.md") && !screen.contains("line 20"), "{screen}");
        let (lines, first) = push_lines(None, None, &commits[..1], 0);
        assert_eq!(first, 0);
        assert!(text(&lines).contains("y push and close · p push and keep it open"));
    }
}
