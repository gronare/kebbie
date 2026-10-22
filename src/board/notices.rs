use std::time::{Duration, Instant};

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use ratatui::Frame;

use crate::keys::{self, Act};
use crate::notice::{Notice, Tone};
use crate::status::Item;
use super::widgets::{BLUE, FAINT, GREEN, MANTLE, MAUVE, MUTED, PEACH, RED, TEXT, YELLOW, bold, centered, rule, style};

pub(super) const TOAST_FOR: Duration = Duration::from_secs(6);
const WIDTH: u16 = 80;
const TOAST_WIDTH: u16 = 60;
const LABEL: usize = 8;

// What a notice is about: the kind of step and the task's title, for the window's title; `push` tells which keys the window offers.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Subject {
    pub kind: String,
    pub title: String,
    pub task: bool,
}

impl Subject {
    pub fn of(kind: &str, item: &Item) -> Subject {
        Subject { kind: kind.to_string(), title: item.titled(), task: true }
    }

    pub fn plain(kind: &str) -> Subject {
        Subject { kind: kind.to_string(), title: String::new(), task: false }
    }

    fn title(&self) -> String {
        if self.title.is_empty() { format!(" {} ", self.kind) } else { format!(" {} · {} ", self.kind, self.title) }
    }
}

pub(super) struct Toast {
    pub notice: Notice,
    pub subject: Option<Subject>,
    pub at: Instant,
}

pub(super) fn tone_color(tone: Tone) -> Color {
    match tone {
        Tone::Done => GREEN,
        Tone::Moving => BLUE,
        Tone::You => PEACH,
        Tone::NotNow => YELLOW,
        Tone::Failed => RED,
    }
}

// The keys a window offers beyond closing it: a task's Space when the user must act there, and push again for a push.
pub(super) fn offers(notice: &Notice, subject: Option<&Subject>) -> Vec<Act> {
    match subject {
        Some(subject) if subject.task && notice.tone == Tone::You => {
            let mut acts = vec![Act::Go];
            if subject.kind == "push" {
                acts.push(Act::Push);
            }
            acts
        }
        _ => Vec::new(),
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

fn labelled(label: &str, text: &str, label_style: Style, text_style: Style, width: usize) -> Vec<Line<'static>> {
    wrap(text, width.saturating_sub(LABEL).max(10)).into_iter().enumerate().map(|(at, part)| Line::from(vec![
        Span::styled(if at == 0 { format!("{label:<w$}", w = LABEL) } else { " ".repeat(LABEL) }, label_style),
        Span::styled(part, text_style),
    ])).collect()
}

fn headline(notice: &Notice, width: usize) -> Vec<Line<'static>> {
    wrap(&notice.headline, width.saturating_sub(2).max(10)).into_iter().enumerate().map(|(at, part)| {
        let mark = if at == 0 { Span::styled(format!("{} ", notice.tone.glyph()), bold(tone_color(notice.tone))) } else { Span::raw("  ") };
        Line::from(vec![mark, Span::styled(part, bold(TEXT))])
    }).collect()
}

// The window's title names the task, so a detail naming it again goes.
fn shown_for(notice: &Notice, subject: Option<&Subject>) -> Notice {
    let mut shown = notice.clone();
    if subject.is_some_and(|subject| subject.task) {
        shown.details.retain(|(label, _)| label != "task");
    }
    shown
}

pub(super) fn notice_lines(notice: &Notice, width: usize) -> Vec<Line<'static>> {
    let mut lines = headline(notice, width);
    if notice.next.is_some() || notice.you.is_some() {
        lines.push(Line::default());
    }
    if let Some(next) = &notice.next {
        lines.extend(labelled("next", next, style(MUTED), style(TEXT), width));
    }
    if let Some(you) = &notice.you {
        lines.extend(labelled("you", you, bold(PEACH), style(PEACH), width));
    }
    if !notice.details.is_empty() {
        lines.push(Line::default());
        for (label, value) in &notice.details {
            lines.extend(labelled(label, value, style(FAINT), style(MUTED), width));
        }
    }
    lines
}

fn keys_line(acts: &[Act]) -> String {
    let map = keys::map();
    let mut keys: Vec<String> = acts.iter().map(|act| match act {
        Act::Go => format!("{} its Space", map.label(Act::Go)),
        Act::Push => format!("{} push again", map.label(Act::Push)),
        _ => String::new(),
    }).collect();
    keys.push(if acts.is_empty() { "any key closes".to_string() } else { "any other key closes".to_string() });
    keys.join(" · ")
}

fn faint_behind(frame: &mut Frame) {
    for cell in &mut frame.buffer_mut().content {
        cell.set_fg(FAINT).set_bg(Color::Reset);
        cell.modifier = Modifier::empty();
    }
}

pub(super) fn draw_notice(frame: &mut Frame, notice: &Notice, subject: Option<&Subject>) {
    faint_behind(frame);
    let full = frame.area();
    let width = WIDTH.min(full.width.saturating_sub(4));
    let lines = notice_lines(&shown_for(notice, subject), width.saturating_sub(4) as usize);
    let area = centered(full, width, lines.len() as u16 + 4);
    frame.render_widget(Clear, area);
    let title = subject.map_or(" kelpie ".to_string(), Subject::title);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(title, bold(MAUVE))).style(Style::new().bg(MANTLE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [text, line, foot] = Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    frame.render_widget(Paragraph::new(lines), text);
    frame.render_widget(Paragraph::new(rule(line.width as usize)), line);
    frame.render_widget(Paragraph::new(Span::styled(keys_line(&offers(notice, subject)), style(MUTED))), foot);
}

// Bottom right, above the key line, over whatever the board shows there; it takes no keys.
pub(super) fn draw_toast(frame: &mut Frame, toast: &Toast) {
    let full = frame.area();
    let width = TOAST_WIDTH.min(full.width.saturating_sub(4));
    if width < 20 {
        return;
    }
    let inner_width = width.saturating_sub(4) as usize;
    let mut lines = headline(&toast.notice, inner_width);
    if let Some(next) = &toast.notice.next {
        lines.extend(wrap(next, inner_width.saturating_sub(2)).into_iter().map(|part| Line::from(vec![Span::raw("  "), Span::styled(part, style(MUTED))])));
    }
    let height = (lines.len() as u16 + 2).min(full.height.saturating_sub(3));
    let area = Rect { x: full.right().saturating_sub(width + 2), y: full.bottom().saturating_sub(height + 2), width, height };
    frame.render_widget(Clear, area);
    let title = toast.subject.as_ref().map_or(String::new(), Subject::title);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(tone_color(toast.notice.tone))).padding(Padding::horizontal(1))
        .title(Span::styled(title, style(MUTED))).style(Style::new().bg(MANTLE));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::KeyCode;

    use crate::keys::Act;
    use crate::notice::{Notice, Tone};
    use crate::board::test_support::*;
    use crate::board::Mode;
    use super::{Subject, TOAST_FOR, offers};

    fn titled(name: &str, title: &str) -> crate::status::Item {
        let mut item = served(name);
        item.title = Some(title.to_string());
        item
    }

    #[test]
    fn a_notice_that_needs_the_user_opens_a_window_with_the_title_its_lines_and_the_keys_it_offers() {
        let mut app = test_app(vec![titled("push-finishes-itself", "Push finishes itself")]);
        app.about("push", "push-finishes-itself");
        let notice = Notice::new(Tone::You, "Not pushed: it conflicts with main, and no Claude is there").next("Nothing moves until it is resolved.")
            .you("Start its Claude, then push again.").detail("task", "push-finishes-itself").detail("folder", "~/projects/wt3.kelpie");
        app.show(notice.ok());
        assert!(matches!(app.mode, Mode::Message(_)) && app.toast.is_none());
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains(" push · Push finishes itself "), "the task's title, not its branch: {screen}");
        assert!(screen.contains("! Not pushed: it conflicts with main") && screen.contains("next    Nothing moves") && screen.contains("you     Start its Claude"), "{screen}");
        assert!(screen.contains("folder  ~/projects/wt3.kelpie") && !screen.contains("task    push-finishes-itself"), "the title names the task, so its detail goes: {screen}");
        assert!(screen.contains("its Space") && screen.contains("push again") && screen.contains("any other key closes"), "{screen}");
        let (x, y) = find(&rows, "Not pushed")[0];
        let (_, you) = find(&rows, "you     ")[0];
        assert!(y < you && x > 20, "headline above the action, inside a centred window");
        press(&mut app, key(KeyCode::Char('z')));
        assert!(matches!(app.mode, Mode::Normal), "any other key closes it");
    }

    #[test]
    fn a_notice_that_asks_nothing_goes_by_as_a_toast_that_takes_no_key_and_leaves_by_itself() {
        let mut app = test_app(vec![titled("billing", "Refund mails"), titled("search", "Search filters")]);
        app.about("push", "billing");
        app.show(Notice::new(Tone::Done, "Pushed 3 commits to main").next("CI runs now; kelpie closes the task when it is green.").detail("commit", "e70d5d5").ok());
        assert!(matches!(app.mode, Mode::Normal), "no window to close");
        let (rows, _) = render(&mut app, 120, 30);
        let (x, y) = find(&rows, "✓ Pushed 3 commits to main")[0];
        assert!(x > 50 && y > 20, "bottom right: {x},{y}\n{}", rows.join("\n"));
        assert!(rows.join("\n").contains("CI runs now") && !rows.join("\n").contains("e70d5d5"), "a toast carries the headline and next, not the details");
        let before = app.work_selected;
        press(&mut app, key(KeyCode::Char('j')));
        assert_ne!(app.work_selected, before, "the board takes the key while the toast shows");
        app.toast.as_mut().unwrap().at = Instant::now() - TOAST_FOR - Duration::from_millis(1);
        let (rows, _) = render(&mut app, 120, 30);
        assert!(app.toast.is_none() && find(&rows, "Pushed 3 commits").is_empty(), "it leaves once its time is up");
    }

    #[test]
    fn a_failure_and_text_kelpie_did_not_write_stay_open_and_only_a_push_offers_push_again() {
        let mut app = test_app(vec![titled("billing", "Refund mails")]);
        app.show(Err("git push failed:\n ! [remote rejected] main -> main (pre-receive hook declined)".to_string()));
        let Mode::Message(text) = &app.mode else { panic!("a failure stays") };
        let notice = Notice::parse(text).unwrap();
        assert_eq!((notice.tone, notice.headline.as_str()), (Tone::Failed, "That did not work"));
        let task = |kind: &str| Subject { kind: kind.to_string(), title: "Refund mails".to_string(), task: true };
        let yours = Notice::new(Tone::You, "Commit them first");
        assert_eq!(offers(&yours, Some(&task("push"))), [Act::Go, Act::Push]);
        assert_eq!(offers(&yours, Some(&task("done"))), [Act::Go]);
        assert!(offers(&Notice::new(Tone::Failed, "x"), Some(&task("push"))).is_empty() && offers(&yours, None).is_empty());
    }
}
