use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::comments::{self, Comment, Kind, Reopen, Status};
use crate::keys::{self, Act};
use crate::notice::{Notice, Tone};
use crate::sh::Result;
use crate::status::Group;
use super::columns::Rows;
use super::input::Purpose;
use super::widgets::{BLUE, FAINT, GREEN, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, TEAL, TEXT, YELLOW, bold, group_line, modal, plural, row, style, text_width, wrap};
use super::{App, Column, Hit, Mode};

const WIDTH: u16 = 92;
const FRAME_ROWS: u16 = 6;
const PAGE: u16 = 10;

pub(super) type Copier = Arc<dyn Fn(&str) -> Result<String> + Send + Sync>;
pub(super) type Reopener = Arc<dyn Fn(&Reopen) -> Result<String> + Send + Sync>;

#[derive(Clone, Debug, Default)]
pub(super) struct CommentPanel {
    pub(super) focused: bool,
    pub(super) selected: usize,
    pub(super) opened: bool,
    pub(super) folded_at: u64,
    pub(super) file: Option<PathBuf>,
}

// A clipboard program where there is one, else the terminal through OSC 52, which also reaches a laptop over ssh.
pub(super) fn default_copier() -> Copier {
    Arc::new(|text| match crate::hub::console::copy_here(text)? {
        crate::hub::card::Copied::Confirmed => Ok("Copied the reply".to_string()),
        crate::hub::card::Copied::ByTerminal => Ok("Sent the reply to your terminal's clipboard".to_string()),
    })
}

pub(super) fn default_reopener() -> Reopener {
    Arc::new(|reopen| match reopen {
        Reopen::Tell { address, .. } => comments::tell(reopen).map(|_| Notice::new(Tone::Moving, format!("Told {address} to fix it")).next("The card turns to its reply once the fix is pushed.").to_string()),
        Reopen::Start { repo, name, prompt } => super::input::start_in_background(repo, name, Some(prompt), None, None, false),
    })
}

pub(super) fn panel_file() -> Option<PathBuf> {
    Some(crate::lead::state_dir().ok()?.join("board-comments"))
}

pub(super) fn load_folded(file: Option<&Path>) -> u64 {
    file.and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| text.trim().parse().ok()).unwrap_or(0)
}

fn icon(comment: &Comment) -> Span<'static> {
    let (text, color) = match (comment.status, comment.kind()) {
        (Status::Unread, _) => ("·", MUTED),
        (Status::Failed, _) => ("!", RED),
        (Status::Reopened, _) => ("↻", TEAL),
        (Status::Reply, _) => ("↩", GREEN),
        (_, Some(Kind::Holds)) => ("◆", PEACH),
        (_, Some(Kind::Opinion)) => ("◇", BLUE),
        (_, Some(Kind::Praise)) => ("✓", GREEN),
        (_, Some(Kind::Question)) => ("?", YELLOW),
        (_, None) => ("·", MUTED),
    };
    Span::styled(text, style(color))
}

// The longest title that leaves the row's state whole: the task, or the fix, goes before the commit does.
pub(super) fn title(comment: &Comment, facts: &str, width: usize) -> String {
    let (author, short) = (&comment.author, comment.short());
    let titles = match comment.fix_short().filter(|_| comment.status == Status::Reply) {
        Some(fix) => vec![format!("{author} on {short} · fixed in {fix}"), format!("{author} · fixed in {fix}"), format!("{author} on {short}"), author.clone()],
        None => vec![format!("{author} on {short} · {}", comment.task), format!("{author} on {short}"), author.clone()],
    };
    let fits = |title: &String| text_width(title) + text_width(facts) + 5 <= width;
    titles.iter().find(|title| fits(title)).or(titles.last()).cloned().unwrap_or_default()
}

fn quote(comment: &Comment, width: usize) -> String {
    let first = comment.body.split_whitespace().collect::<Vec<_>>().join(" ");
    let room = width.saturating_sub(6);
    let shown: String = if first.chars().count() > room { first.chars().take(room.saturating_sub(1)).collect::<String>() + "…" } else { first };
    format!("\"{shown}\"")
}

// Folded, the section is one row that ⏎ opens; open, a header and two lines a card, and esc folds it again.
pub(super) fn comment_lines(app: &App, width: usize, first: usize, focused_column: bool) -> (Vec<Line<'static>>, Rows, Option<usize>) {
    if !app.comments_shown() {
        return (Vec::new(), Vec::new(), None);
    }
    let panel = &app.comment_panel;
    let focused = panel.focused && focused_column;
    let mut rows = Vec::new();
    let mut selected_line = None;
    if !app.comments_open() {
        let waiting = app.comments.iter().any(Comment::needs_decision);
        let marker = if focused { Span::styled("▌", bold(MAUVE)) } else { Span::raw(" ") };
        let text = format!("{} · {} shows them", plural(app.comments.len(), "comment"), keys::map().label(Act::Options));
        let line = Line::from(vec![marker, Span::styled("◇ ", style(MUTED)), Span::styled(text, style(if waiting { PEACH } else { MUTED }))]);
        if panel.focused {
            selected_line = Some(first);
        }
        rows.push((first, first + 1, Hit::Comment(0)));
        return (vec![if panel.focused { line.style(Style::new().bg(SURFACE)) } else { line }], rows, selected_line);
    }
    let mut header = group_line("COMMENTS", app.comments.len(), MAUVE);
    header.spans.push(Span::styled(format!("  {} hides them", keys::map().label(Act::Back)), style(FAINT)));
    let mut lines = vec![header];
    for (index, comment) in app.comments.iter().enumerate() {
        let selected = panel.focused && index == panel.selected;
        let start = first + lines.len();
        if selected {
            selected_line = Some(start);
        }
        let facts = comments::row_word(comment);
        lines.push(row(selected, focused, icon(comment), &title(comment, &facts, width), None, &facts, width));
        let quoted = Line::from(vec![Span::raw("   "), Span::styled(quote(comment, width), style(FAINT))]);
        lines.push(if selected && focused { quoted.style(Style::new().bg(SURFACE)) } else { quoted });
        rows.push((start, start + 2, Hit::Comment(index)));
    }
    (lines, rows, selected_line)
}

impl App {
    fn comment_state(&self) -> Option<PathBuf> {
        self.comment_dir.clone().or_else(|| crate::lead::state_dir().ok())
    }

    pub(super) fn comments_shown(&self) -> bool {
        !self.comments.is_empty() && self.filter_of(Column::Work).is_none()
    }

    // Open while a card waits for a choice it has not been folded over, or since ⏎ opened it.
    pub(super) fn comments_open(&self) -> bool {
        self.comment_panel.opened || self.comments.iter().any(|comment| comment.needs_decision() && comment.changed_at > self.comment_panel.folded_at)
    }

    // The section sits under READY TO PUSH, before the first task of a group the board orders after it.
    pub(super) fn comment_boundary(&self) -> usize {
        let rank = |group: Group| self.groups.iter().position(|shown| *shown == group).unwrap_or(usize::MAX);
        let ready = rank(Group::ReadyToPush);
        self.tasks.iter().position(|item| item.parked.is_some() || rank(item.group()) > ready).unwrap_or(self.tasks.len())
    }

    pub(super) fn comments_changed(&mut self) {
        self.comment_panel.selected = self.comment_panel.selected.min(self.comments.len().saturating_sub(1));
        self.comment_panel.focused &= self.comments_shown();
    }

    fn reload_comments(&mut self) {
        if let Some(state) = self.comment_state() {
            self.comments = comments::open(&state);
        }
        self.comments_changed();
    }

    pub(super) fn focus_comments(&mut self, index: usize) {
        self.in_messages = false;
        self.offer_panel.focused = false;
        self.column = Column::Work;
        self.comment_panel.focused = self.comments_shown();
        self.comment_panel.selected = if self.comments_open() { index.min(self.comments.len().saturating_sub(1)) } else { 0 };
    }

    fn shown_task(&self, range: impl DoubleEndedIterator<Item = usize>, last: bool) -> Option<usize> {
        let mut shown = range.filter(|index| self.shown(Column::Work, *index));
        if last { shown.next_back() } else { shown.next() }
    }

    // ↑ ↓ across the section: from the task above it into its first card, from the task below into its last.
    pub(super) fn enter_comments(&mut self, down: bool) -> bool {
        if !self.comments_shown() {
            return false;
        }
        let boundary = self.comment_boundary();
        let here = self.work_selected;
        let has_task = self.selected_task().is_some();
        let next = if down { self.shown_task(here + 1..self.tasks.len(), false) } else { self.shown_task(0..here, true) };
        let crosses = match (has_task, down) {
            (false, _) => true,
            (true, true) => here < boundary && next.is_none_or(|next| next >= boundary),
            (true, false) => here >= boundary && next.is_none_or(|next| next < boundary),
        };
        if crosses {
            self.focus_comments(if down { 0 } else { self.comments.len().saturating_sub(1) });
        }
        crosses
    }

    fn leave_comments(&mut self, down: bool) {
        let boundary = self.comment_boundary();
        let task = if down { self.shown_task(boundary..self.tasks.len(), false) } else { self.shown_task(0..boundary, true) };
        match task {
            Some(index) => {
                self.comment_panel.focused = false;
                self.work_selected = index;
            }
            None if down && !self.offers.is_empty() => self.focus_offers(0),
            None => {}
        }
    }

    pub(super) fn handle_comments(&mut self, key: KeyEvent) {
        let open = self.comments_open();
        if open && key.code == KeyCode::Char('y') {
            if let Some(comment) = self.comments.get(self.comment_panel.selected).cloned().filter(|comment| comment.status == Status::Reply) {
                self.copy_reply(&comment);
            }
            return;
        }
        let last = self.comments.len().saturating_sub(1);
        match keys::map().act(self.ctx(), &key) {
            Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => match self.comment_panel.selected.checked_sub(1).filter(|_| open) {
                Some(above) => self.comment_panel.selected = above,
                None => self.leave_comments(false),
            },
            Some(Act::Move) if open && self.comment_panel.selected < last => self.comment_panel.selected += 1,
            Some(Act::Move) => self.leave_comments(true),
            Some(Act::Options) if open => self.open_comment(),
            Some(Act::Options) => {
                self.comment_panel.opened = true;
                self.comment_panel.selected = 0;
            }
            Some(Act::Back | Act::Close) if open => self.fold_comments(),
            Some(Act::Back | Act::Close) => self.comment_panel.focused = false,
            Some(Act::Switch) => (self.comment_panel.focused, self.column) = (false, Column::Backlog),
            Some(Act::Messages) => {
                self.comment_panel.focused = false;
                self.press_m();
            }
            Some(Act::Help) => self.mode = Mode::Help(Default::default()),
            Some(Act::Inbox) => self.open_questions(None),
            _ => {}
        }
    }

    fn fold_comments(&mut self) {
        self.comment_panel.opened = false;
        self.comment_panel.folded_at = crate::messages::now().max(self.comments.iter().map(|comment| comment.changed_at).max().unwrap_or(0));
        self.comment_panel.selected = 0;
        if let Some(file) = &self.comment_panel.file {
            let _ = crate::state::write(file, format!("{}\n", self.comment_panel.folded_at));
        }
    }

    pub(super) fn open_comment(&mut self) {
        if let Some(comment) = self.comments.get(self.comment_panel.selected) {
            self.mode = Mode::Comment { id: comment.id, scroll: 0, nudged: false };
        }
    }

    fn copy_reply(&mut self, comment: &Comment) {
        let copied = (self.copier)(&comment.reply_text());
        self.subject = Some(super::notices::Subject::plain("comment"));
        self.show(copied.map(|said| Notice::new(Tone::Done, said).next(format!("Paste it as your reply on GitHub ({} opens the comment), then mark it sent.", keys::key(Act::OpenImage))).to_string()));
    }

    fn settle_comment(&mut self, result: Result<Comment>, said: &str) {
        self.subject = Some(super::notices::Subject::plain("comment"));
        let shown = result.map(|_| Notice::new(Tone::Done, said).to_string());
        self.reload_comments();
        self.show(shown);
    }

    fn reopen_comment(&mut self, comment: &Comment, words: Option<&str>) {
        let Some(state) = self.comment_state() else { return };
        let running = |name: &str| crate::tasks::find_task(&comment.repo, Some(name), &comment.repo).is_ok();
        let decided = comments::reopen(comment, words, &crate::messages::load(), &running);
        let result = decided.and_then(|reopen| {
            let said = (self.reopener)(&reopen)?;
            comments::mark_reopened(&state, comment.id, &reopen, words, &crate::repo::name_of(&comment.repo))?;
            Ok(said)
        });
        self.subject = Some(super::notices::Subject::plain("comment"));
        self.reload_comments();
        self.show(result);
    }

    fn open_on_github(&mut self, comment: &Comment) {
        let opened = self.viewer.map(|viewer| crate::issues::open_url(viewer, &comment.url));
        if !matches!(opened, Some(Ok(()))) {
            self.subject = Some(super::notices::Subject::plain("comment"));
            self.show_as(Notice::new(Tone::NotNow, "Open the comment in your browser").detail("url", &comment.url).ok(), true);
        }
    }

    fn ask_words(&mut self, purpose: Purpose, label: String, buffer: String) {
        self.mode = Mode::Input { purpose, label, buffer };
    }

    // Each card's choices by its kind and state; any other key keeps the card and says which keys it takes.
    pub(super) fn handle_comment_window(&mut self, id: u64, scroll: u16, key: KeyEvent, cancels: bool) {
        let Some(index) = self.comments.iter().position(|comment| comment.id == id) else { return };
        let comment = self.comments[index].clone();
        let Some(state) = self.comment_state() else { return };
        let task = comment.task.clone();
        let author = comment.author.clone();
        match (key.code, comment.status, comment.kind()) {
            (KeyCode::Char('o'), _, _) => {
                self.open_on_github(&comment);
                if matches!(self.mode, Mode::Normal) {
                    self.mode = Mode::Comment { id, scroll, nudged: false };
                }
            }
            (KeyCode::Down | KeyCode::Char('j'), _, _) => self.mode = Mode::Comment { id, scroll: scroll.saturating_add(1), nudged: false },
            (KeyCode::Up | KeyCode::Char('k'), _, _) => self.mode = Mode::Comment { id, scroll: scroll.saturating_sub(1), nudged: false },
            (KeyCode::PageDown | KeyCode::Char(' '), _, _) => self.mode = Mode::Comment { id, scroll: scroll.saturating_add(PAGE), nudged: false },
            (KeyCode::PageUp, _, _) => self.mode = Mode::Comment { id, scroll: scroll.saturating_sub(PAGE), nudged: false },
            (KeyCode::Left | KeyCode::Right, _, _) => {
                let next = if key.code == KeyCode::Left { index.checked_sub(1).unwrap_or(self.comments.len() - 1) } else { (index + 1) % self.comments.len() };
                self.comment_panel.selected = next;
                self.mode = Mode::Comment { id: self.comments[next].id, scroll: 0, nudged: false };
            }
            (KeyCode::Char('3'), status, _) if !matches!(status, Status::Reply) => self.settle_comment(comments::set_status(&state, id, Status::Dismissed), &format!("Dismissed {author}'s comment; nothing was posted")),
            (KeyCode::Char('1'), Status::Failed, _) => self.settle_comment(comments::read_again(&state, id), "kebbie reads it again"),
            (KeyCode::Char('1'), Status::Read, Some(Kind::Holds)) | (KeyCode::Char('2'), Status::Read, Some(Kind::Opinion)) => self.reopen_comment(&comment, None),
            (KeyCode::Char('w'), Status::Read, Some(Kind::Holds)) => self.ask_words(Purpose::CommentWords(id), format!("What should {task} change? (the fix still names {author}'s comment)"), String::new()),
            (KeyCode::Char('2'), Status::Read, Some(Kind::Holds)) => self.ask_words(Purpose::CommentReply(id), format!("Reply to {author}, without a change"), String::new()),
            (KeyCode::Char('1'), Status::Read, Some(Kind::Opinion)) | (KeyCode::Char('r'), Status::Read, Some(Kind::Praise)) => self.settle_comment(comments::set_status(&state, id, Status::Reply), "The reply is ready to copy"),
            (KeyCode::Char('w'), Status::Read, Some(Kind::Opinion)) | (KeyCode::Char('w'), Status::Reply, _) => self.ask_words(Purpose::CommentReply(id), format!("Reply to {author}"), comment.reply_text().split_whitespace().collect::<Vec<_>>().join(" ")),
            (KeyCode::Char('d'), Status::Read, Some(Kind::Praise)) => self.settle_comment(comments::set_status(&state, id, Status::Done), &format!("Done with {author}'s comment")),
            (KeyCode::Char('w'), Status::Read, Some(Kind::Question)) => self.ask_words(Purpose::CommentAnswer(id), format!("Your answer to the reading's question about {author}'s comment"), String::new()),
            (KeyCode::Char('y'), Status::Reply, _) => {
                self.copy_reply(&comment);
                if matches!(self.mode, Mode::Normal) {
                    self.mode = Mode::Comment { id, scroll, nudged: false };
                }
            }
            (KeyCode::Char('d'), Status::Reply, _) => self.settle_comment(comments::set_status(&state, id, Status::Sent), &format!("Marked the reply to {author} sent")),
            _ if cancels => {}
            _ => self.mode = Mode::Comment { id, scroll, nudged: true },
        }
    }

    pub(super) fn submit_comment(&mut self, purpose: Purpose, text: String) {
        let Some(state) = self.comment_state() else { return };
        match purpose {
            Purpose::CommentWords(id) => {
                if let Some(comment) = self.comments.iter().find(|comment| comment.id == id).cloned() {
                    self.reopen_comment(&comment, Some(&text));
                }
            }
            Purpose::CommentReply(id) => self.settle_comment(comments::set_reply(&state, id, &text), "The reply is ready to copy"),
            Purpose::CommentAnswer(id) => self.settle_comment(comments::answer(&state, id, &text), "kebbie reads it again with your answer"),
            _ => {}
        }
    }
}

fn status_word(comment: &Comment) -> &'static str {
    match (comment.status, comment.kind()) {
        (Status::Unread, _) => "reading",
        (Status::Failed, _) => "not read",
        (Status::Reopened, _) => "fixing",
        (Status::Reply, _) => "reply ready",
        (_, Some(Kind::Question)) => "unclear",
        (_, Some(kind)) => kind.key(),
        (_, None) => "read",
    }
}

fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), bold(SUBTEXT)))
}

fn text_rows(text: &str, width: usize, indent: &str, color: Color) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    for paragraph in text.lines() {
        if paragraph.trim().is_empty() {
            rows.push(Line::default());
            continue;
        }
        let lead = paragraph.chars().take_while(|character| *character == ' ').count();
        let pad = format!("{indent}{}", " ".repeat(lead));
        for piece in wrap(paragraph.trim_start(), width.saturating_sub(text_width(&pad)), usize::MAX) {
            rows.push(Line::from(vec![Span::raw(pad.clone()), Span::styled(piece, style(color))]));
        }
    }
    rows
}

fn cut(text: &str, width: usize) -> String {
    let mut shown = String::new();
    for character in text.chars() {
        if text_width(&shown) + text_width(&character.to_string()) > width {
            break;
        }
        shown.push(character);
    }
    shown
}

fn choice(key: &str, label: &str, recommended: bool) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{key} "), bold(MAUVE)), Span::styled(label.to_string(), style(TEXT))];
    if recommended {
        spans.push(Span::styled(" (recommended)", style(GREEN)));
    }
    Line::from(spans)
}

fn choices(comment: &Comment) -> Vec<Line<'static>> {
    let task = &comment.task;
    match (comment.status, comment.kind()) {
        (Status::Read, Some(Kind::Holds)) => vec![choice("1", &format!("Reopen {task} with the proposal"), true), choice("2", "Reply without a change", false), choice("3", "Dismiss", false), choice("w", "In my own words", false)],
        (Status::Read, Some(Kind::Opinion)) => vec![choice("1", "Reply without a change", true), choice("2", &format!("Reopen {task} and change it anyway"), false), choice("3", "Dismiss", false), choice("w", "Edit the reply", false)],
        (Status::Read, Some(Kind::Praise)) => vec![choice("d", "Done", true), choice("r", "Reply anyway", false)],
        (Status::Read, _) => vec![choice("w", "Answer the question", true), choice("3", "Dismiss", false)],
        (Status::Failed, _) => vec![choice("1", "Read it again", true), choice("3", "Dismiss", false)],
        (Status::Reply, _) => vec![choice("y", "Copy the reply", true), choice("d", "Mark it sent", false), choice("w", "Edit the reply", false)],
        _ => vec![choice("3", "Dismiss", false)],
    }
}

// The card's body, scrolled as one; its choices and keys stay under it.
pub(super) fn body_lines(comment: &Comment, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(format!("{} on {} · {} · {}", comment.author, comment.short(), comment.place(), comment.task), style(MUTED)))];
    lines.extend(text_rows(&comment.body, width, "┃ ", TEXT).into_iter().map(|line| {
        let mut spans = line.spans;
        if let Some(first) = spans.first_mut() {
            *first = Span::styled(first.content.to_string(), style(MAUVE));
        }
        Line::from(spans)
    }));
    if !comment.hunk.is_empty() {
        lines.push(Line::default());
        for text in comment.hunk.lines() {
            let color = if text.starts_with("@@") { TEAL } else if text.starts_with('+') { GREEN } else if text.starts_with('-') { RED } else { SUBTEXT };
            lines.push(Line::from(Span::styled(cut(text, width), style(color))));
        }
    }
    let reading = comment.reading.clone().unwrap_or_default();
    let section = |lines: &mut Vec<Line<'static>>, title: &str, text: &str, color: Color| {
        if !text.trim().is_empty() {
            lines.push(Line::default());
            lines.push(heading(title));
            lines.extend(text_rows(text, width, "  ", color));
        }
    };
    match comment.status {
        Status::Unread => section(&mut lines, "Independent reading", "kebbie is reading it now; the card fills in when it is done.", MUTED),
        Status::Failed => section(&mut lines, "Independent reading", &format!("The reading gave no answer: {}", comment.error.as_deref().unwrap_or("no reason given")), PEACH),
        _ => section(&mut lines, "Independent reading", &reading.reading, TEXT),
    }
    let holds = reading.kind == Some(Kind::Holds);
    match comment.status {
        Status::Read if holds => {
            let test = if reading.test.is_empty() { String::new() } else { format!("\nTest: {}", reading.test) };
            section(&mut lines, "Proposal", &format!("{}{test}", reading.proposal), TEXT);
        }
        Status::Read if reading.kind == Some(Kind::Question) => section(&mut lines, "Question", &reading.question, YELLOW),
        Status::Read if reading.kind == Some(Kind::Opinion) => {
            section(&mut lines, "Recommendation", &reading.recommendation, TEXT);
            section(&mut lines, "Reply to send", &comment.reply_text(), TEXT);
        }
        Status::Reopened => section(&mut lines, "Reopened", &format!("{} fixes it; the card turns to its reply once the fix is pushed.", comment.fixing.as_deref().unwrap_or(&comment.task)), TEAL),
        _ => {}
    }
    if holds && matches!(comment.status, Status::Read | Status::Reopened) {
        section(&mut lines, "The fix's commit message", &comments::fix_message(comment), SUBTEXT);
    }
    if comment.status == Status::Reply {
        if let Some(fix) = comment.fix_short() {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(format!("Fixed in {fix}, on the default branch"), bold(GREEN))));
        }
        lines.push(Line::default());
        lines.push(heading("Reply to send"));
        for paragraph in comment.reply_text().lines() {
            lines.extend(if paragraph.is_empty() { vec![Line::default()] } else { wrap(paragraph, width, usize::MAX).into_iter().map(|piece| Line::from(Span::styled(piece, style(TEXT)))).collect() });
        }
    }
    lines
}

pub(super) fn window_title(comment: &Comment, index: usize, count: usize) -> String {
    format!("comment {}/{count} · {} · {}", index + 1, status_word(comment), comment.task)
}

// Returns the scroll it drew at.
pub(super) fn draw_comment(frame: &mut Frame, app: &App, id: u64, scroll: u16, nudged: bool) -> u16 {
    let Some(index) = app.comments.iter().position(|comment| comment.id == id) else { return scroll };
    let comment = &app.comments[index];
    let width = WIDTH.min(frame.area().width.saturating_sub(4));
    let inner = usize::from(width.saturating_sub(4)).max(20);
    let body = body_lines(comment, inner);
    let choices = choices(comment);
    let view = usize::from(frame.area().height.saturating_sub(FRAME_ROWS + choices.len() as u16 + 2)).max(3).min(body.len());
    let last = body.len().saturating_sub(view);
    let scroll = usize::from(scroll).min(last);
    let mut lines: Vec<Line<'static>> = body.into_iter().skip(scroll).take(view).collect();
    lines.push(Line::default());
    lines.extend(choices);
    let moves = if app.comments.len() > 1 { " · ← → other comments" } else { "" };
    let scrolls = if last > 0 { format!("↑ ↓ scroll, rows {}–{} · ", scroll + 1, scroll + view) } else { String::new() };
    lines.push(Line::from(Span::styled(format!("{scrolls}o open on GitHub{moves} · esc close"), style(MUTED))));
    if nudged {
        lines.push(Line::from(Span::styled("press one of the keys above", style(PEACH))));
    }
    modal(frame, window_title(comment, index, app.comments.len()), lines, width);
    scroll as u16
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use ratatui::crossterm::event::KeyCode;

    use super::*;
    use crate::board::test_support::*;
    use crate::comments::tests::comment;

    fn three() -> Vec<Comment> {
        let mut opinion = comment(2, Some(Kind::Opinion), Status::Read);
        opinion.author = "anna".to_string();
        opinion.body = "I'd call it size, not count.".to_string();
        opinion.reading.as_mut().unwrap().reading = "A naming preference; count is the word the model uses elsewhere.".to_string();
        opinion.reading.as_mut().unwrap().recommendation = "Keep count; reply with the reason.".to_string();
        opinion.reading.as_mut().unwrap().reply = "Thanks! I kept count, the word the other models use.".to_string();
        let mut praise = comment(3, Some(Kind::Praise), Status::Read);
        praise.author = "maria".to_string();
        praise.body = "Nice cleanup!".to_string();
        praise.reading.as_mut().unwrap().reading = "A thank-you; nothing to change.".to_string();
        vec![comment(1, Some(Kind::Holds), Status::Read), opinion, praise]
    }

    fn with_comments(tasks: Vec<crate::status::Item>, comments: Vec<Comment>) -> (App, PathBuf) {
        let state = crate::sh::test_dir("board-comments");
        for one in &comments {
            comments::save(&state, one).unwrap();
        }
        let mut app = test_app(tasks);
        app.comment_dir = Some(state.clone());
        app.comments = comments::open(&state);
        app.copier = Arc::new(|_| Ok("Copied the reply".to_string()));
        (app, state)
    }

    fn ready() -> crate::status::Item {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
        item.name = "ready-one".to_string();
        item
    }

    fn rows_of(app: &mut App, width: u16, height: u16) -> Vec<String> {
        render(app, width, height).0.into_iter().map(|row| row.trim_end().to_string()).collect()
    }

    fn work_part(rows: &[String], width: usize) -> Vec<String> {
        rows.iter().map(|row| row.chars().take(width).collect::<String>().trim_end().to_string()).collect()
    }

    #[test]
    fn the_section_sits_under_ready_to_push_with_a_row_and_a_quote_per_comment() {
        let mut working = crate::status::tests::task(Some("working"), None, 0);
        working.name = "busy-one".to_string();
        let mut idle = crate::status::tests::task(Some("idle"), None, 0);
        idle.name = "quiet-one".to_string();
        let (mut app, state) = with_comments(Vec::new(), three());
        app.set_tasks(vec![working, ready(), idle]);
        let rows = rows_of(&mut app, 100, 34);
        let work = work_part(&rows, 56);
        let at = |text: &str| work.iter().position(|row| row.contains(text)).unwrap_or_else(|| panic!("no {text}:\n{}", rows.join("\n")));
        assert!(at("READY TO PUSH") < at("COMMENTS") && at("COMMENTS") < at("WAITING FOR WORK"), "{}", rows.join("\n"));
        let comments = at("COMMENTS");
        assert_eq!(&work[comments..comments + 7], [
            "  COMMENTS  3  esc hides them",
            "   ◆ johan on 3f2a9c1        holds · reopen with a fix",
            "     \"This misses partial refunds.\"",
            "   ◇ anna on 3f2a9c1        opinion · reply, no change",
            "     \"I'd call it size, not count.\"",
            "   ✓ maria on 3f2a9c1           praise · nothing to do",
            "     \"Nice cleanup!\"",
        ], "{}", rows.join("\n"));
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn arrows_cross_the_section_and_its_keys_never_reach_a_task() {
        let mut idle = crate::status::tests::task(Some("idle"), None, 0);
        idle.name = "quiet-one".to_string();
        let mut first = ready();
        first.path = Some(PathBuf::from("/nowhere/wt1.app"));
        let (mut app, state) = with_comments(Vec::new(), three());
        app.set_tasks(vec![first, idle]);
        assert_eq!(app.selected_task().map(|item| item.name).as_deref(), Some("ready-one"));
        assert!(rows_of(&mut app, 100, 34).join("\n").contains("branch  ready-one"));
        press(&mut app, key(KeyCode::Down));
        assert!(app.comment_panel.focused && app.comment_panel.selected == 0, "↓ from the last ready task goes into the comments");
        assert!(!rows_of(&mut app, 100, 34).join("\n").contains("branch  ready-one"), "the task above folds its details while a card is selected");
        for (code, selected) in [(KeyCode::Down, 1), (KeyCode::Down, 2), (KeyCode::Up, 1)] {
            press(&mut app, key(code));
            assert_eq!(app.comment_panel.selected, selected);
        }
        for letter in ['d', 'p', 'x', 'z', 'Z', 'C', 't', 'M'] {
            press(&mut app, key(KeyCode::Char(letter)));
            assert!(matches!(app.mode, Mode::Normal) && app.comment_panel.focused, "{letter} on a comment row is no task action");
        }
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Down));
        assert!(!app.comment_panel.focused && app.selected_task().map(|item| item.name).as_deref() == Some("quiet-one"), "↓ from the last card goes on to the next task");
        press(&mut app, key(KeyCode::Up));
        assert!(app.comment_panel.focused && app.comment_panel.selected == 2, "↑ from below comes back to the last card");
        press(&mut app, key(KeyCode::Esc));
        let rows = rows_of(&mut app, 100, 34);
        assert!(rows.iter().any(|row| row.contains("  ▌◇ 3 comments · ⏎ shows them")) && !rows.join("\n").contains("COMMENTS"), "esc folds the section to one selected row:\n{}", rows.join("\n"));
        press(&mut app, key(KeyCode::Enter));
        assert!(app.comments_open() && app.comment_panel.focused, "⏎ opens it again");
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Comment { id: 1, .. }), "⏎ on a card opens its window");
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_section_starts_open_only_with_a_card_that_waits_for_a_choice() {
        let (mut app, state) = with_comments(vec![ready()], vec![comment(1, Some(Kind::Praise), Status::Read), comment(2, None, Status::Unread)]);
        let rows = rows_of(&mut app, 100, 34);
        assert!(rows.iter().any(|row| row.contains("◇ 2 comments · ⏎ shows them")), "praise and a reading under way wait for nothing:\n{}", rows.join("\n"));
        let mut holds = comment(3, Some(Kind::Holds), Status::Read);
        holds.changed_at = 50;
        app.comments.push(holds);
        app.comment_panel.folded_at = 40;
        assert!(app.comments_open(), "a card read after the fold opens it");
        app.comment_panel.folded_at = 60;
        assert!(!app.comments_open(), "folded after it, it stays folded");
        std::fs::remove_dir_all(state).unwrap();
    }

    fn window(app: &mut App) -> Vec<String> {
        rows_of(app, 100, 34).into_iter().map(|row| row.trim().trim_matches('│').trim_end().to_string()).collect()
    }

    #[test]
    fn each_kind_of_card_offers_its_own_choices() {
        let (mut app, state) = with_comments(vec![ready()], three());
        app.mode = Mode::Comment { id: 1, scroll: 0, nudged: false };
        let holds = window(&mut app).join("\n");
        for wanted in ["comment 1/3 · holds · refunds-in-part", "johan on 3f2a9c1 · app/models/refund.rb:42 · refunds-in-part", "┃ This misses partial refunds.", "-    refunds.count", "Independent reading", "Proposal", "Test: A test with two partial refunds.", "The fix's commit message", "Addresses johan's comment on 3f2a9c1.", "1 Reopen refunds-in-part with the proposal (recommended)", "2 Reply without a change", "3 Dismiss", "w In my own words", "o open on GitHub · ← → other comments · esc close"] {
            assert!(holds.contains(wanted), "{wanted:?} in\n{holds}");
        }
        press(&mut app, key(KeyCode::Right));
        let opinion = window(&mut app).join("\n");
        for wanted in ["comment 2/3 · opinion · refunds-in-part", "Recommendation", "Keep count; reply with the reason.", "Reply to send", "Thanks! I kept count", "1 Reply without a change (recommended)", "2 Reopen refunds-in-part and change it anyway", "w Edit the reply"] {
            assert!(opinion.contains(wanted), "{wanted:?} in\n{opinion}");
        }
        press(&mut app, key(KeyCode::Right));
        let praise = window(&mut app).join("\n");
        assert!(praise.contains("comment 3/3 · praise") && praise.contains("d Done (recommended)") && praise.contains("r Reply anyway") && !praise.contains("1 Reopen"), "{praise}");
        press(&mut app, key(KeyCode::Char('x')));
        assert!(window(&mut app).join("\n").contains("press one of the keys above"), "another key keeps the card");
        press(&mut app, key(KeyCode::Char('d')));
        assert_eq!(comments::load(&state, 3).unwrap().status, Status::Done, "d on praise is done, and it leaves the board");
        assert!(app.comments.iter().all(|comment| comment.id != 3));
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn reopening_tells_the_task_or_starts_a_followup_and_the_card_waits_for_the_fix() {
        let (mut app, state) = with_comments(vec![ready()], three());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = asked.clone();
        app.reopener = Arc::new(move |reopen| {
            seen.lock().unwrap().push(reopen.clone());
            Ok("Starting in the background".to_string())
        });
        app.mode = Mode::Comment { id: 1, scroll: 0, nudged: false };
        press(&mut app, key(KeyCode::Char('1')));
        let reopened = asked.lock().unwrap().clone();
        assert!(matches!(&reopened[..], [Reopen::Start { name, prompt, .. }] if name == "refunds-in-part-followup" && prompt.contains("Addresses johan's comment on 3f2a9c1.")), "the task that pushed has ended, so a follow-up starts: {reopened:?}");
        let card = comments::load(&state, 1).unwrap();
        assert_eq!((card.status, card.fixing.as_deref()), (Status::Reopened, Some("refunds-in-part-followup@app")));
        let rows = rows_of(&mut app, 100, 34);
        assert!(rows.iter().any(|row| row.contains("↻ johan") && row.trim_end().ends_with("fixing in refunds-in-part-followup")), "{}", rows.join("\n"));
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_card_whose_fix_is_pushed_shows_its_reply_y_copies_it_and_d_marks_it_sent() {
        let mut fixed = comment(1, Some(Kind::Holds), Status::Reply);
        fixed.fix = Some("7c9e2f1abcdef".to_string());
        let (mut app, state) = with_comments(vec![ready()], vec![fixed]);
        let copied = Arc::new(Mutex::new(Vec::new()));
        let seen = copied.clone();
        app.copier = Arc::new(move |text| {
            seen.lock().unwrap().push(text.to_string());
            Ok("Copied the reply".to_string())
        });
        let rows = rows_of(&mut app, 100, 34);
        assert!(rows.iter().any(|row| row.contains("↩ johan · fixed in 7c9e2f1  ") && row.contains("reply ready · y copy")), "{}", rows.join("\n"));
        let wide = render(&mut app, 120, 34).0.join("\n");
        assert!(wide.contains("↩ johan on 3f2a9c1 · fixed in 7c9e2f1"), "with room the row names the commit too: {wide}");
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Char('y')));
        assert_eq!(copied.lock().unwrap().as_slice(), ["Good catch, fixed in 7c9e2f1."], "y on the row copies the reply");
        app.toast = None;
        app.mode = Mode::Comment { id: 1, scroll: 0, nudged: false };
        let shown = window(&mut app).join("\n");
        for wanted in ["comment 1/1 · reply ready · refunds-in-part", "Fixed in 7c9e2f1, on the default branch", "Reply to send", "Good catch, fixed in 7c9e2f1.", "y Copy the reply (recommended)", "d Mark it sent", "w Edit the reply", "o open on GitHub · esc close"] {
            assert!(shown.contains(wanted), "{wanted:?} in\n{shown}");
        }
        press(&mut app, key(KeyCode::Char('y')));
        assert_eq!(copied.lock().unwrap().len(), 2);
        app.mode = Mode::Comment { id: 1, scroll: 0, nudged: false };
        press(&mut app, key(KeyCode::Char('3')));
        assert_eq!(comments::load(&state, 1).unwrap().status, Status::Reply, "a ready reply is not dismissed by accident; it goes once sent");
        app.mode = Mode::Comment { id: 1, scroll: 0, nudged: false };
        press(&mut app, key(KeyCode::Char('w')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::CommentReply(1), buffer, .. } if buffer == "Good catch, fixed in 7c9e2f1."), "w edits the reply, starting from it");
        app.mode = Mode::Input { purpose: Purpose::CommentReply(1), label: String::new(), buffer: "Fixed in 7c9e2f1, thanks!".to_string() };
        press(&mut app, key(KeyCode::Enter));
        assert_eq!(comments::load(&state, 1).unwrap().reply_text(), "Fixed in 7c9e2f1, thanks!");
        app.mode = Mode::Comment { id: 1, scroll: 0, nudged: false };
        press(&mut app, key(KeyCode::Char('d')));
        assert_eq!(comments::load(&state, 1).unwrap().status, Status::Sent);
        let rows = rows_of(&mut app, 100, 34).join("\n");
        assert!(app.comments.is_empty() && !rows.contains("COMMENTS") && !rows.contains("johan on"), "sent, it leaves the board: {rows}");
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_click_selects_a_card_and_a_second_click_opens_it() {
        let (mut app, state) = with_comments(vec![ready()], three());
        let rows = rows_of(&mut app, 100, 34);
        let (x, y) = find(&rows, "anna on 3f2a9c1")[0];
        click(&mut app, x as u16, y as u16);
        assert!(app.comment_panel.focused && app.comment_panel.selected == 1 && matches!(app.mode, Mode::Normal));
        click(&mut app, x as u16, y as u16);
        assert!(matches!(app.mode, Mode::Comment { id: 2, .. }));
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn untrusted_comment_text_reaches_no_cell_as_an_escape() {
        let mut bad = comment(1, Some(Kind::Holds), Status::Read);
        bad.body = format!("look {ESCAPES}");
        bad.author = format!("jo{ESCAPES}");
        let (mut app, state) = with_comments(vec![ready()], vec![bad]);
        let (_, buffer) = render(&mut app, 100, 34);
        no_escape_reaches_the_terminal(&buffer, "the comments section");
        app.mode = Mode::Comment { id: 1, scroll: 0, nudged: false };
        let (_, buffer) = render(&mut app, 100, 34);
        no_escape_reaches_the_terminal(&buffer, "the comment window");
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_question_is_answered_in_the_window_and_read_again_with_the_answer() {
        let (mut app, state) = with_comments(vec![ready()], vec![comment(1, Some(Kind::Question), Status::Read)]);
        app.mode = Mode::Comment { id: 1, scroll: 0, nudged: false };
        let shown = window(&mut app).join("\n");
        assert!(shown.contains("comment 1/1 · unclear · refunds-in-part") && shown.contains("Which total does the report mean?") && shown.contains("w Answer the question (recommended)"), "{shown}");
        press(&mut app, key(KeyCode::Char('1')));
        assert!(matches!(app.mode, Mode::Comment { nudged: true, .. }), "a question offers no reopen");
        press(&mut app, key(KeyCode::Char('w')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::CommentAnswer(1), .. }));
        app.mode = Mode::Input { purpose: Purpose::CommentAnswer(1), label: String::new(), buffer: "The amount.".to_string() };
        press(&mut app, key(KeyCode::Enter));
        let asked = comments::load(&state, 1).unwrap();
        assert_eq!((asked.status, asked.answer.as_deref()), (Status::Unread, Some("The amount.")), "the answer goes to a fresh reading");
        std::fs::remove_dir_all(state).unwrap();
    }
}
