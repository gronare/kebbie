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
use super::card::{indented, push_card_lines};
use super::detail::tests_spans;
use super::editor::{MessageEditor, Rewrite};
use super::{App, Hit, Mode};
use super::widgets::{MAUVE, MUTED, SURFACE, TEXT, YELLOW, bold, modal, plural, rule, section_header, style, wrap};

// The push dialog's lines at a width, and the line its first commit is on.
pub(super) fn push_lines(card: Option<&(report::Card, bool)>, tests: Option<&evidence::Shown>, commits: &[tasks::Commit], selected: usize, width: usize) -> (Vec<Line<'static>>, usize) {
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
        lines.push(section_header("MESSAGE", vec![Span::styled(commit.sha.chars().take(7).collect::<String>(), style(MUTED))], width));
        let bar = || Span::styled("▎ ", style(MAUVE));
        for line in commit.message.lines() {
            let wrapped = if line.is_empty() { vec![String::new()] } else { wrap(line, width.saturating_sub(4), 6) };
            lines.extend(wrapped.into_iter().map(|part| Line::from(vec![Span::raw("  "), bar(), Span::styled(part, style(TEXT))])));
        }
    }
    lines.push(rule(width));
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
                let label = if keep_open { format!("Rebasing and pushing {}; the task stays open…", plural(count, "commit")) } else { format!("Rebasing and pushing {}, then closing the task…", plural(count, "commit")) };
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

const PUSH_WIDTH: u16 = 96;

pub(super) fn draw_push(frame: &mut Frame, commits: &[tasks::Commit], selected: usize, card: Option<&(report::Card, bool)>, tests: Option<&evidence::Shown>) -> Vec<(Rect, Hit)> {
    let mut modal_hits = Vec::new();
    let width = PUSH_WIDTH.min(frame.area().width.saturating_sub(4));
    let (lines, first_commit) = push_lines(card, tests, commits, selected, width.saturating_sub(4) as usize);
    let area = modal(frame, "push".to_string(), lines, width);
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
    use crate::evidence;
    use crate::report;
    use crate::tasks;

    #[test]
    fn the_push_dialog_parts_the_card_tests_commits_and_message_under_their_own_headers() {
        let text = |lines: &[Line]| lines.iter().map(|line| line.spans.iter().map(|span| span.content.to_string()).collect::<String>()).collect::<Vec<_>>().join("\n");
        let commits = vec![tasks::Commit { sha: "1111111aaaa".to_string(), message: "Add refunds\n\nso a customer gets money back".to_string() }, tasks::Commit { sha: "2222222bbbb".to_string(), message: "Show refunds".to_string() }];
        let card = report::Card { sha: "abcdef0123".to_string(), text: "Refunds show on the **billing** page.\n\n![the refund row](/tmp/kelpie-shots/refund.png)\nTo check: 30 days.".to_string(), file: PathBuf::from("/state/cards/billing@web.md") };
        let tests = evidence::Shown { mark: evidence::Mark::Covered, text: "cargo test · 412 passed · at abcdef0".to_string() };
        let (lines, first) = push_lines(Some(&(card, true)), Some(&tests), &commits, 0, 60);
        let screen = text(&lines);
        let rule = |head: &str| format!("{head} {}", "─".repeat(60 - head.chars().count() - 1));
        assert!(screen.starts_with(&format!("{}\n  Refunds show on the billing page.\n\n  To check: 30 days.\n  ▣ refund.png  the refund row\n\n", rule("CARD ✓ current at abcdef0"))), "{screen}");
        assert!(lines[1].spans.iter().any(|span| span.content == "billing" && span.style.add_modifier.contains(ratatui::style::Modifier::BOLD)), "**billing** is bold without its stars");
        assert!(!screen.contains("/tmp/kelpie-shots"), "an image shows by its file name: {screen}");
        assert!(screen.contains(&format!("{}\n  ✓ cargo test · 412 passed · at abcdef0\n\n{}\n", rule("TESTS"), rule("COMMITS 2"))), "{screen}");
        assert!(text(&lines[first..first + 1]).contains("▸ 1111111 Add refunds"), "the first commit is where the mouse hits are counted from");
        assert!(screen.contains(&format!("{}\n  ▎ Add refunds\n  ▎ \n  ▎ so a customer gets money back\n{}\nPush?  ", rule("MESSAGE 1111111"), "─".repeat(60))), "{screen}");
        let bar = lines.iter().find(|line| line.to_string().contains("so a customer")).unwrap();
        assert_eq!(bar.spans[1].style.fg, Some(MAUVE), "the message hangs off a mauve bar");
        let (lines, _) = push_lines(None, None, &commits, 1, 60);
        let screen = text(&lines);
        assert!(screen.contains("MESSAGE 2222222") && screen.contains("  ▎ Show refunds") && !screen.contains("money back"), "the message follows the selection: {screen}");
        assert!(screen.contains("p push the first 2 and keep it open"), "{screen}");
        let stale = report::Card { sha: "abcdef0123".to_string(), text: (0..40).map(|index| format!("line {index}")).collect::<Vec<_>>().join("\n"), file: PathBuf::from("/state/cards/billing@web.md") };
        let (lines, _) = push_lines(Some(&(stale, false)), None, &commits, 0, 60);
        let screen = text(&lines);
        assert!(screen.contains("CARD ○ written at abcdef0, code changed since") && screen.contains("  … the rest in /state/cards/billing@web.md") && !screen.contains("line 20"), "{screen}");
        let (lines, first) = push_lines(None, None, &commits[..1], 0, 60);
        assert_eq!(first, 1, "under the COMMITS header");
        assert!(text(&lines).contains("y push and close · p push and keep it open"));
    }
}
