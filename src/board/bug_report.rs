use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;
use crate::report_wire::{BugReport, ReportKind};

use crate::hub::report::{self, Scrub};
use crate::issue_recheck::Fixed;
use crate::keys::{self, Act};
use crate::notice::{Notice, Tone};
use crate::sh::Result;
use super::{App, Hit, Mode};
use super::columns::Rows;
use super::widgets::{Choice, FAINT, GREEN, MAUVE, MUTED, PEACH, SURFACE, TEXT, YELLOW, bold, group_line, modal, options_lines, style, wrap};

const PAGE: u16 = 10;
const FRAME_ROWS: u16 = 14;
const FIXED_TITLES: usize = 3;
const SHOWN: &str = "shown";
const HIDDEN: &str = "hidden";

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Offer {
    pub(super) id: String,
    pub(super) report: BugReport,
}

pub(super) fn load_offers() -> Vec<Offer> {
    let Ok(state) = crate::lead::state_dir() else { return Vec::new() };
    let enabled = crate::config::report_settings().is_ok_and(|settings| report::board_offers(&settings, &state));
    offers_in(&state, enabled, &Scrub::current)
}

// The scrub reads the checkouts from disk, so it is made only when a bug waits for the user.
pub(super) fn offers_in(state: &std::path::Path, enabled: bool, scrub: &dyn Fn() -> Scrub) -> Vec<Offer> {
    let issues = report::offers(state, enabled);
    if issues.is_empty() {
        return Vec::new();
    }
    let scrub = scrub();
    issues.iter().map(|issue| Offer { id: issue.id.clone(), report: report::payload(issue, &scrub) }).collect()
}

#[derive(Clone, Debug, Default)]
pub(super) struct OfferPanel {
    pub(super) shown: bool,
    pub(super) focused: bool,
    pub(super) selected: usize,
    pub(super) file: Option<PathBuf>,
    pub(super) on_fixed: bool,
}

type SendFn = Arc<dyn Fn(&str, &[u8; 32]) -> Result<String> + std::marker::Send + Sync>;
type DeclineFn = Arc<dyn Fn(&str) -> Result<String> + std::marker::Send + Sync>;
type SendAllFn = Arc<dyn Fn(&[(String, [u8; 32])]) -> Result<String> + std::marker::Send + Sync>;

#[derive(Clone)]
pub(super) struct Reporter {
    pub(super) send: SendFn,
    pub(super) decline: DeclineFn,
    pub(super) send_all: SendAllFn,
}

impl Default for Reporter {
    fn default() -> Self {
        Reporter {
            send: Arc::new(|id, digest| {
                let settings = crate::config::report_settings()?;
                report::send_in(&crate::lead::state_dir()?, &crate::permit::caller(), id, settings.url(), &Scrub::current(), digest, crate::messages::now())
            }),
            decline: Arc::new(|id| report::decline_in(&crate::lead::state_dir()?, &crate::permit::caller(), id, crate::messages::now())),
            send_all: Arc::new(|read| {
                let settings = crate::config::report_settings()?;
                let pace = report::Pace { clock: &std::time::Instant::now, wait: &std::thread::sleep, say: &|_| {} };
                report::send_all_in(&crate::lead::state_dir()?, &crate::permit::caller(), read, settings.url(), &Scrub::current(), &pace, crate::messages::now())?.notice()
            }),
        }
    }
}

pub(super) fn panel_file() -> Option<PathBuf> {
    Some(crate::lead::state_dir().ok()?.join("board-reports"))
}

pub(super) fn load_shown(file: Option<&Path>) -> bool {
    file.and_then(|file| std::fs::read_to_string(file).ok()).is_some_and(|saved| saved.trim() == SHOWN)
}

fn save_shown(file: Option<&Path>, shown: bool) {
    if let Some(file) = file {
        let _ = crate::state::write(file, format!("{}\n", if shown { SHOWN } else { HIDDEN }));
    }
}

fn counted(offers: &[Offer]) -> String {
    let wishes = offers.iter().filter(|offer| offer.report.kind() == ReportKind::Wish).count();
    crate::issues::kinds(offers.len() - wishes, wishes)
}

pub(super) fn indicator(offers: &[Offer], compact: bool) -> Option<String> {
    (!offers.is_empty()).then(|| if compact { format!("! {}", offers.len()) } else { format!("! {} to send", counted(offers)) })
}

fn folded_text(offers: &[Offer], key: &str, width: usize) -> String {
    let [offer] = offers else { return format!("{} to send · {key} shows them", counted(offers)) };
    let kind = if offer.report.kind() == ReportKind::Wish { "wish" } else { "bug" };
    let tail = format!(" · {key} shows it");
    let prefix = format!("A {kind} to send: ");
    let room = width.saturating_sub(prefix.chars().count() + tail.chars().count() + 3);
    let title = wrap(&offer.report.title, room.max(1), 1).into_iter().next().unwrap_or_default();
    format!("{prefix}{title}{tail}")
}

// The lines, each offer's rows, and the line of the selected offer when the list has the focus, all counted from `first`.
pub(super) fn offer_lines(app: &App, width: usize, first: usize) -> (Vec<Line<'static>>, Rows, Option<usize>) {
    if app.offers.is_empty() {
        return (Vec::new(), Vec::new(), None);
    }
    let panel = &app.offer_panel;
    let focused = panel.focused && !panel.on_fixed;
    let mut rows = Vec::new();
    let mut selected_line = None;
    if !panel.shown {
        let marker = if focused { Span::styled("▌", bold(MAUVE)) } else { Span::raw(" ") };
        let text = folded_text(&app.offers, &keys::map().label(Act::Options), width);
        let line = Line::from(vec![marker, Span::styled("! ", style(YELLOW)), Span::styled(text, style(YELLOW))]);
        if focused {
            selected_line = Some(first + 1);
        }
        rows.push((first + 1, first + 2, Hit::Offer(0)));
        return (vec![Line::default(), if focused { line.style(Style::new().bg(SURFACE)) } else { line }], rows, selected_line);
    }
    let mut lines = vec![Line::default(), group_line("Send to kebbie's maintainers?", app.offers.len(), YELLOW)];
    let hint = if focused { "⏎ asks to send · a sends all · n keeps it here · esc hides them" } else { "bugs and wishes about kebbie, saved here" };
    lines.push(Line::from(Span::styled(hint, style(MUTED))));
    for (index, offer) in app.offers.iter().enumerate() {
        let title = wrap(&offer.report.title, width.saturating_sub(12), 1).into_iter().next().unwrap_or_default();
        let line = Line::from(vec![Span::styled("! ", style(YELLOW)), Span::styled(format!("{}  ", offer.id), style(FAINT)), Span::styled(title, style(TEXT))]);
        let selected = focused && index == panel.selected;
        if selected {
            selected_line = Some(first + lines.len());
        }
        rows.push((first + lines.len(), first + lines.len() + 1, Hit::Offer(index)));
        lines.push(if selected { line.style(Style::new().bg(SURFACE)) } else { line });
    }
    (lines, rows, selected_line)
}

pub(super) fn load_fixed() -> Option<Fixed> {
    crate::issue_recheck::shown(&crate::lead::state_dir().ok()?, crate::messages::now())
}

// The block, its row, and its first line when the cursor is on it, counted from `first`.
pub(super) fn fixed_lines(app: &App, width: usize, first: usize) -> (Vec<Line<'static>>, Rows, Option<usize>) {
    let Some(fixed) = app.fixed.as_ref() else { return (Vec::new(), Vec::new(), None) };
    let focused = app.fixed_focused();
    let block = fixed_block(fixed, !app.questions.is_empty(), width, focused);
    let rows = vec![(first + 1, first + 1 + block.len(), Hit::Fixed)];
    let mut lines = vec![Line::default()];
    lines.extend(block);
    (lines, rows, focused.then_some(first + 1))
}

fn fixed_block(fixed: &Fixed, questions: bool, width: usize, focused: bool) -> Vec<Line<'static>> {
    let marker = if focused { Span::styled("▌", bold(MAUVE)) } else { Span::raw(" ") };
    let mut lines = vec![Line::from(vec![marker, Span::styled(format!("✓ {}", crate::issue_recheck::headline(fixed, crate::config::channel().unwrap_or_default())), style(GREEN))])];
    for title in fixed.titles.iter().take(FIXED_TITLES) {
        let title = wrap(title, width.saturating_sub(5), 1).into_iter().next().unwrap_or_default();
        lines.push(Line::from(vec![Span::raw("   "), Span::styled(title, style(TEXT))]));
    }
    let more = fixed.titles.len().saturating_sub(FIXED_TITLES);
    if more > 0 {
        lines.push(Line::from(Span::styled(format!("   … and {more} more"), style(MUTED))));
    }
    let after = if questions { ", once the questions are answered" } else { "" };
    lines.push(Line::from(Span::styled(format!("   {} shows the list{after} · {} options", keys::key(Act::Inbox), keys::map().label(Act::Options)), style(MUTED))));
    if focused {
        return lines.into_iter().map(|line| line.style(Style::new().bg(SURFACE))).collect();
    }
    lines
}

fn fixed_choices() -> Vec<Choice> {
    let row = |key: char, label: &str, about: &str| Choice { key, act: None, label: label.to_string(), about: about.to_string(), link: None };
    vec![
        row('1', "Show the list", "every title in a window; the block goes once it is shown"),
        row('2', "Hide this", "the block goes without the list and does not come back"),
    ]
}

pub(super) fn draw_fixed_options(frame: &mut Frame, fixed: Option<&Fixed>, index: usize) -> Vec<(Rect, Hit)> {
    let title = fixed.map_or("kebbie".to_string(), |fixed| crate::issue_recheck::headline(fixed, crate::config::channel().unwrap_or_default()).chars().take(56).collect());
    let area = modal(frame, title, options_lines(fixed_choices(), index, 56), 62);
    (0..2u16).map(|choice| (Rect::new(area.x, area.y + 1 + 2 * choice, area.width, 2), Hit::Choice(choice as usize))).filter(|(rect, _)| rect.y + 1 < area.bottom()).collect()
}

impl App {
    fn mark_fixed_seen(&mut self) -> Option<Fixed> {
        let fixed = self.fixed.take()?;
        self.fixed_seen = self.fixed_seen.max(fixed.last);
        if let Ok(state) = crate::lead::state_dir() {
            let _ = crate::issue_recheck::dismiss(&state, &fixed, crate::messages::now());
        }
        self.fixed_changed();
        Some(fixed)
    }

    pub(super) fn fixed_focused(&self) -> bool {
        self.offer_panel.focused && self.offer_panel.on_fixed
    }

    pub(super) fn focus_fixed(&mut self) {
        if self.fixed.is_none() {
            return;
        }
        self.in_messages = false;
        self.comment_panel.focused = false;
        self.column = super::Column::Work;
        (self.offer_panel.focused, self.offer_panel.on_fixed) = (true, true);
    }

    // Gone while the cursor was on it, the cursor goes up a row, as ↑ would take it.
    pub(super) fn fixed_changed(&mut self) {
        if self.fixed_focused() && self.fixed.is_none() {
            self.leave_fixed();
        }
    }

    fn leave_fixed(&mut self) {
        if !self.offers.is_empty() {
            let last = if self.offer_panel.shown { self.offers.len() - 1 } else { 0 };
            return self.focus_offers(last);
        }
        if self.comments_shown() && !(self.comment_boundary()..self.tasks.len()).any(|index| self.shown(super::Column::Work, index)) {
            self.offer_panel.on_fixed = false;
            return self.focus_comments(self.comments.len().saturating_sub(1));
        }
        let stays = self.tasks.is_empty() && self.fixed.is_some();
        (self.offer_panel.focused, self.offer_panel.on_fixed) = (stays, stays);
    }

    fn handle_fixed(&mut self, key: KeyEvent) {
        match keys::map().act(self.ctx(), &key) {
            Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => self.leave_fixed(),
            Some(Act::Options) => self.mode = Mode::FixedOptions(0),
            Some(Act::Back | Act::Close) => (self.offer_panel.focused, self.offer_panel.on_fixed) = (false, false),
            Some(Act::Switch) => (self.offer_panel.focused, self.offer_panel.on_fixed, self.column) = (false, false, super::Column::Backlog),
            Some(Act::Messages) => {
                (self.offer_panel.focused, self.offer_panel.on_fixed) = (false, false);
                self.press_m();
            }
            Some(Act::Help) => self.mode = Mode::Help(Default::default()),
            Some(Act::Inbox) => self.open_questions(None),
            _ => {}
        }
    }

    pub(super) fn choose_fixed(&mut self, index: usize, act: Option<Act>, key: KeyEvent) {
        match (key.code, act) {
            (KeyCode::Char('1'), _) => self.pick_fixed(0),
            (KeyCode::Char('2'), _) => self.pick_fixed(1),
            (_, Some(Act::Move)) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => self.mode = Mode::FixedOptions(0),
            (_, Some(Act::Move)) => self.mode = Mode::FixedOptions(1),
            (_, Some(Act::Options)) => self.pick_fixed(index),
            (_, Some(Act::Back | Act::Close)) => {}
            _ => self.mode = Mode::FixedOptions(index),
        }
    }

    pub(super) fn pick_fixed(&mut self, index: usize) {
        if index == 0 {
            self.open_fixed();
        } else {
            self.mark_fixed_seen();
        }
    }

    pub(super) fn open_fixed(&mut self) -> bool {
        let Some(fixed) = self.mark_fixed_seen() else { return false };
        let notice = fixed.titles.iter().fold(Notice::new(Tone::Done, crate::issue_recheck::headline(&fixed, crate::config::channel().unwrap_or_default())).next("kebbie issues --all lists them, each with the commit that fixed it."), |notice, title| notice.detail("fixed", title));
        self.subject = Some(super::notices::Subject::plain("issues"));
        self.show_as(notice.ok(), true);
        true
    }

    pub(super) fn offers_changed(&mut self) {
        if let Some((going, _)) = &self.sending {
            self.offers.retain(|offer| !going.contains(&offer.id));
        }
        self.offer_panel.selected = self.offer_panel.selected.min(self.offers.len().saturating_sub(1));
        self.offer_panel.focused &= if self.offer_panel.on_fixed { self.fixed.is_some() } else { !self.offers.is_empty() };
    }

    fn set_shown(&mut self, shown: bool) {
        self.offer_panel.shown = shown;
        save_shown(self.offer_panel.file.as_deref(), shown);
    }

    pub(super) fn focus_offers(&mut self, index: usize) {
        self.in_messages = false;
        self.comment_panel.focused = false;
        self.column = super::Column::Work;
        self.offer_panel.focused = !self.offers.is_empty();
        self.offer_panel.on_fixed = false;
        self.offer_panel.selected = index.min(self.offers.len().saturating_sub(1));
    }

    pub(super) fn show_offers(&mut self) {
        self.set_shown(true);
        self.offer_panel.selected = 0;
    }

    // Folded, the list is one row that ⏎ opens; open, esc folds it back with the selection on that row.
    pub(super) fn handle_offers(&mut self, key: KeyEvent) {
        if self.offer_panel.on_fixed {
            return self.handle_fixed(key);
        }
        let shown = self.offer_panel.shown;
        if shown && key.code == KeyCode::Char('n') {
            return self.decline_offer();
        }
        if shown && key.code == KeyCode::Char('a') && self.sending.is_some() {
            self.subject = Some(super::notices::Subject::plain("report"));
            return self.show(Notice::new(Tone::Moving, "A batch of reports is still going to kebbie's maintainers").next("a sends the rest once it has ended; the board says when.").ok());
        }
        if shown && key.code == KeyCode::Char('a') && !self.offers.is_empty() {
            self.mode = Mode::ReportAll { shown: self.offers.clone(), scroll: 0, read: false, nudged: false };
            return;
        }
        match keys::map().act(self.ctx(), &key) {
            Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => match self.offer_panel.selected.checked_sub(1).filter(|_| shown) {
                Some(above) => self.offer_panel.selected = above,
                None if self.comments_shown() && !(self.comment_boundary()..self.tasks.len()).any(|index| self.shown(super::Column::Work, index)) => self.focus_comments(self.comments.len().saturating_sub(1)),
                None => self.offer_panel.focused = self.tasks.is_empty(),
            },
            Some(Act::Move) if !shown || self.offer_panel.selected + 1 >= self.offers.len() => self.focus_fixed(),
            Some(Act::Move) => self.offer_panel.selected += 1,
            Some(Act::Options) if shown => {
                self.open_report();
            }
            Some(Act::Options) => self.show_offers(),
            Some(Act::Back | Act::Close) if shown => self.set_shown(false),
            Some(Act::Back | Act::Close) => self.offer_panel.focused = false,
            Some(Act::Switch) => (self.offer_panel.focused, self.column) = (false, super::Column::Backlog),
            Some(Act::Messages) => {
                self.offer_panel.focused = false;
                self.press_m();
            }
            Some(Act::Help) => self.mode = Mode::Help(Default::default()),
            Some(Act::Inbox) => self.open_questions(None),
            _ => {}
        }
    }

    fn decline_offer(&mut self) {
        let Some(id) = self.offers.get(self.offer_panel.selected).map(|offer| offer.id.clone()) else { return };
        self.decline(&id);
    }

    fn decline(&mut self, id: &str) {
        self.subject = Some(super::notices::Subject::plain("report"));
        self.offers.retain(|offer| offer.id != id);
        self.offers_changed();
        let declined = (self.reporter.decline)(id);
        self.show(declined);
    }

    pub(super) fn open_report(&mut self) -> bool {
        let Some(offer) = self.offers.get(self.offer_panel.selected) else { return false };
        self.mode = Mode::Report { id: offer.id.clone(), shown: Box::new(offer.report.clone()), scroll: 0, read: false, nudged: false };
        true
    }

    // y counts only once the whole text has been on screen, and sends exactly the report that was shown, even when a snapshot has refreshed the list since.
    pub(super) fn confirm_report(&mut self, id: String, shown: Box<BugReport>, scroll: u16, read: bool, key: KeyEvent, cancels: bool) {
        match key.code {
            KeyCode::Char('y') if read => {
                self.subject = Some(super::notices::Subject::plain("report"));
                self.offers.retain(|offer| offer.id != id);
                self.offers_changed();
                let digest = shown.digest();
                let send = self.reporter.send.clone();
                self.in_background("Sending the report to kebbie's maintainers…".to_string(), move || send(&id, &digest));
            }
            KeyCode::Char('n') => self.decline(&id),
            KeyCode::Down | KeyCode::Char('j') => self.mode = Mode::Report { id, shown, scroll: scroll.saturating_add(1), read, nudged: false },
            KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::Report { id, shown, scroll: scroll.saturating_sub(1), read, nudged: false },
            KeyCode::PageDown | KeyCode::Char(' ') => self.mode = Mode::Report { id, shown, scroll: scroll.saturating_add(PAGE), read, nudged: false },
            KeyCode::PageUp => self.mode = Mode::Report { id, shown, scroll: scroll.saturating_sub(PAGE), read, nudged: false },
            _ if cancels => {}
            _ => self.mode = Mode::Report { id, shown, scroll, read, nudged: true },
        }
    }
}

impl App {
    // One y sends every bug the window showed, in the background, so a paced batch never holds the board.
    pub(super) fn confirm_report_all(&mut self, shown: Vec<Offer>, scroll: u16, read: bool, key: KeyEvent, cancels: bool) {
        match key.code {
            KeyCode::Char('y') if read => {
                let read: Vec<(String, [u8; 32])> = shown.iter().map(|offer| (offer.id.clone(), offer.report.digest())).collect();
                let (count, named) = (read.len(), counted(&shown));
                let (sender, result) = std::sync::mpsc::channel();
                let send_all = self.reporter.send_all.clone();
                let going = read.iter().map(|(id, _)| id.clone()).collect();
                std::thread::spawn(move || {
                    let _ = sender.send(send_all(&read));
                });
                self.sending = Some((going, result));
                self.offers_changed();
                let later = if count > report::BURST { format!("; {} go at once, then {} every 5 minutes", report::BURST, report::BURST) } else { String::new() };
                self.subject = Some(super::notices::Subject::plain("report"));
                self.show(Notice::new(Tone::Moving, format!("Sending {named} to kebbie's maintainers{later}")).next("Each one sent is a line under messages; the board says when all have gone.").ok());
            }
            KeyCode::Down | KeyCode::Char('j') => self.mode = Mode::ReportAll { shown, scroll: scroll.saturating_add(1), read, nudged: false },
            KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::ReportAll { shown, scroll: scroll.saturating_sub(1), read, nudged: false },
            KeyCode::PageDown | KeyCode::Char(' ') => self.mode = Mode::ReportAll { shown, scroll: scroll.saturating_add(PAGE), read, nudged: false },
            KeyCode::PageUp => self.mode = Mode::ReportAll { shown, scroll: scroll.saturating_sub(PAGE), read, nudged: false },
            _ if cancels => {}
            _ => self.mode = Mode::ReportAll { shown, scroll, read, nudged: true },
        }
    }

    // The batch's outcome waits for the board to be at rest, so it never covers a window the user has open.
    pub(super) fn poll_sending(&mut self) {
        if !matches!(self.mode, Mode::Normal) {
            return;
        }
        let Some((_, receiver)) = &self.sending else { return };
        let Ok(result) = receiver.try_recv() else { return };
        self.sending = None;
        self.subject = Some(super::notices::Subject::plain("report"));
        self.show(result);
    }
}

fn preview_rows(report: &BugReport, width: usize) -> Vec<String> {
    report::preview(report).lines().flat_map(|line| if line.is_empty() { vec![String::new()] } else { wrap(line, width, usize::MAX) }).collect()
}

// Returns the scroll it drew at and whether the text's last row was on screen.
pub(super) fn draw_report(frame: &mut Frame, id: &str, shown: &BugReport, scroll: u16, read: bool, nudged: bool) -> (u16, bool) {
    let width = 88u16.min(frame.area().width.saturating_sub(4));
    let rows = preview_rows(shown, usize::from(width.saturating_sub(4)).max(20));
    let ask = Ask {
        head: &|range| format!("{} {id} as it would go to kebbie's maintainers{range}:", if shown.kind() == ReportKind::Wish { "Wish" } else { "Bug" }),
        question: "Send this report to kebbie's maintainers?",
        answers: "  y / n",
        keys: ["y sends it, signed with this install's key · n keeps it here only · esc decides later", "↓ pgdn read on: y counts once you have read to the end · n keeps it here only · esc decides later"],
        nudges: ["press y or n", "read to the end first, or press n"],
    };
    draw_preview(frame, &rows, width, &ask, scroll, read, nudged)
}

pub(super) fn draw_all(frame: &mut Frame, shown: &[Offer], scroll: u16, read: bool, nudged: bool) -> (u16, bool) {
    let width = 88u16.min(frame.area().width.saturating_sub(4));
    let wrap_width = usize::from(width.saturating_sub(4)).max(20);
    let reports: Vec<(String, BugReport)> = shown.iter().map(|offer| (offer.id.clone(), offer.report.clone())).collect();
    let rows: Vec<String> = report::all_preview(&reports).lines().flat_map(|line| if line.is_empty() { vec![String::new()] } else { wrap(line, wrap_width, usize::MAX) }).collect();
    let count = counted(shown);
    let question = format!("Send all {count} to kebbie's maintainers?");
    let ask = Ask {
        head: &|range| format!("{count} as they would go to kebbie's maintainers{range}:"),
        question: &question,
        answers: "  y / esc",
        keys: ["y sends them all, each signed with this install's key · esc decides later", "↓ pgdn read on: y counts once you have read to the end · esc decides later"],
        nudges: ["press y or esc", "read to the end first, or press esc"],
    };
    draw_preview(frame, &rows, width, &ask, scroll, read, nudged)
}

struct Ask<'a> {
    head: &'a dyn Fn(String) -> String,
    question: &'a str,
    answers: &'a str,
    keys: [&'a str; 2],
    nudges: [&'a str; 2],
}

fn draw_preview(frame: &mut Frame, rows: &[String], width: u16, ask: &Ask, scroll: u16, read: bool, nudged: bool) -> (u16, bool) {
    let view = usize::from(frame.area().height.saturating_sub(FRAME_ROWS)).max(3).min(rows.len());
    let last = rows.len().saturating_sub(view);
    let scroll = usize::from(scroll).min(last);
    let read = read || scroll >= last;
    let mut lines = vec![Line::from(Span::styled((ask.head)(if last > 0 { format!(", rows {}–{} of {}", scroll + 1, scroll + view, rows.len()) } else { String::new() }), style(MUTED))), Line::default()];
    lines.extend(rows.iter().enumerate().skip(scroll).take(view).map(|(index, row)| Line::from(Span::styled(row.clone(), if index == 0 { bold(TEXT) } else { style(TEXT) }))));
    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled(ask.question.to_string(), style(YELLOW)), Span::styled(ask.answers.to_string(), style(MUTED))]));
    lines.push(Line::from(Span::styled(ask.keys[usize::from(!read)].to_string(), style(MUTED))));
    if nudged {
        lines.push(Line::from(Span::styled(ask.nudges[usize::from(!read)].to_string(), style(PEACH))));
    }
    modal(frame, "report".to_string(), lines, width);
    (scroll as u16, read)
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;

    use super::*;
    use crate::board::test_support::*;
    use crate::issues::{self, Issue};

    fn filed(state: &std::path::Path, id: &str, kind: &str, title: &str) {
        filed_with(state, id, kind, title, "It failed in /Users/ada/work/shop.");
    }

    fn filed_with(state: &std::path::Path, id: &str, kind: &str, title: &str, body: &str) {
        let issue = Issue { id: id.to_string(), title: title.to_string(), kind: kind.to_string(), status: "open".to_string(), version: "0.26.0".to_string(), by: "lead".to_string(), body: body.to_string(), ..Issue::default() };
        issues::save(&issues::dir(state), &issue).unwrap();
    }

    fn scrub() -> Scrub {
        Scrub::new(Some("/Users/ada".to_string()), vec!["shop".to_string()])
    }

    fn screen(app: &mut App) -> String {
        render(app, 120, 40).0.join("\n")
    }

    fn settle(app: &mut App) {
        let start = std::time::Instant::now();
        while matches!(app.mode, Mode::Working { .. }) && start.elapsed() < std::time::Duration::from_secs(30) {
            app.take_working_result();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn row_of(app: &mut App, text: &str) -> String {
        let (rows, _) = render(app, 120, 40);
        let (_, y) = find(&rows, text).first().copied().unwrap_or_else(|| panic!("no {text}: {}", rows.join("\n")));
        rows[y].chars().take(60).collect::<String>().trim_end().to_string()
    }

    #[test]
    fn the_bugs_to_send_fold_into_one_row_that_enter_opens_and_esc_folds_again() {
        let state = crate::sh::test_dir("board-report");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        let offers = offers_in(&state, true, &scrub);
        assert_eq!(offers.iter().map(|offer| offer.id.as_str()).collect::<Vec<_>>(), ["a1b2c3"]);
        assert!(offers_in(&state, false, &|| panic!("nothing is scrubbed when the offer is off")).is_empty(), "[report] text other than \"ask\" hides it");

        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        assert!(!screen(&mut app).contains("to send"), "no bugs, no count");
        app.offers = offers;
        let shown = screen(&mut app);
        assert!(shown.contains(" ! A bug to send: push hid git's error · ⏎ shows it") && !shown.contains("Send to kebbie's maintainers?") && !shown.contains("! a1b2c3"), "folded at first: {shown}");
        assert_eq!(row_of(&mut app, "to send: "), "   ! A bug to send: push hid git's error · ⏎ shows it", "one row under the tasks, not selected");

        press(&mut app, key(KeyCode::Char('B')));
        assert!(!app.offer_panel.shown && !app.offer_panel.focused, "B is no key any more");
        press(&mut app, key(KeyCode::Down));
        assert!(app.offer_panel.focused && !app.offer_panel.shown, "↓ from the last task selects the folded row");
        assert_eq!(row_of(&mut app, "to send: "), "  ▌! A bug to send: push hid git's error · ⏎ shows it");
        press(&mut app, key(KeyCode::Enter));
        let shown = screen(&mut app);
        assert!(shown.contains("Send to kebbie's maintainers?  1") && shown.contains("! a1b2c3  push hid git's error") && shown.contains("  ⏎ asks to send · a sends all · n keeps it here · esc hides them "), "{shown}");
        assert!(app.offer_panel.focused && app.offer_panel.shown && matches!(app.mode, Mode::Normal), "⏎ opens the list with its first bug selected");
        press(&mut app, key(KeyCode::Esc));
        assert!(app.offer_panel.focused && !app.offer_panel.shown && !screen(&mut app).contains("! a1b2c3"), "esc folds it with the selection on its row");
        press(&mut app, key(KeyCode::Esc));
        assert!(!app.offer_panel.focused, "esc on the row goes back to the tasks");
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Char('n')));
        assert_eq!(app.offers.len(), 1, "n on the folded row declines no bug it hides");
        press(&mut app, key(KeyCode::Up));
        assert!(!app.offer_panel.focused, "↑ from the row goes back to the tasks");

        app.questions = crate::messages::fold(QUESTION);
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Enter));
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::Report { id, nudged: false, .. } if id == "a1b2c3"), "a question waiting does not stand in the way");
        let shown = screen(&mut app);
        assert!(shown.contains("Send this report to kebbie's maintainers?  y / n") && shown.contains("It failed in ~/work/<repo>."), "the text as it would go out: {shown}");
        assert!(!shown.contains("/Users/ada") && !shown.contains("shop"), "{shown}");
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Report { nudged: true, .. }), "another key keeps the question");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal) && app.offers.len() == 1 && app.offer_panel.focused, "esc decides later, back in the list");
        press(&mut app, key(KeyCode::Char('i')));
        assert!(matches!(app.mode, Mode::Questions(_)), "i is the questions");
    }

    #[test]
    fn a_single_wish_waiting_is_named_by_its_title_and_cut_to_the_width() {
        let state = crate::sh::test_dir("board-report-one-wish");
        filed(&state, "b2c3d4", "wish", "Route a file drop to the pane under the pointer");
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.offers = offers_in(&state, true, &scrub);
        assert_eq!(folded_text(&app.offers, "⏎", 120), "A wish to send: Route a file drop to the pane under the pointer · ⏎ shows it");
        assert!(screen(&mut app).contains(" ! A wish to send: Route a"), "the board row carries it");
        let narrow = folded_text(&app.offers, "⏎", 40);
        assert!(narrow.starts_with("A wish to send: Route a") && narrow.ends_with(" · ⏎ shows it") && narrow.chars().count() <= 37, "{narrow}");
    }

    #[test]
    fn a_wish_waits_beside_the_bugs_and_its_window_shows_it_redacted_as_a_wish() {
        let state = crate::sh::test_dir("board-report-wish");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        filed(&state, "b2c3d4", "wish", "a dark theme for shop");
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.offers = offers_in(&state, true, &scrub);
        assert_eq!(row_of(&mut app, "to send · ⏎"), "   ! 1 bug and 1 wish to send · ⏎ shows them");
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Enter));
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::Report { id, shown, .. } if id == "b2c3d4" && shown.kind == Some(ReportKind::Wish)), "the wish goes as a wish");
        let shown = screen(&mut app);
        assert!(shown.contains("Wish b2c3d4 as it would go to kebbie's maintainers") && shown.contains("a dark theme for <repo>") && shown.contains("It failed in ~/work/<repo>."), "{shown}");
        assert!(!shown.contains("/Users/ada") && !shown.contains("shop"), "{shown}");
    }

    #[test]
    fn whether_the_list_shows_is_kept_for_the_next_board() {
        let state = crate::sh::test_dir("board-report-kept");
        let file = state.join("board-reports");
        assert!(!load_shown(Some(&file)), "hidden when nothing was saved");
        let mut app = test_app(Vec::new());
        app.offer_panel.file = Some(file.clone());
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        app.offers = offers_in(&state, true, &scrub);
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Enter));
        assert!(load_shown(Some(&file)));
        press(&mut app, key(KeyCode::Esc));
        assert!(!load_shown(Some(&file)));
    }

    #[test]
    fn a_click_picks_a_bug_a_second_click_opens_it_and_n_keeps_one_here_only() {
        let state = crate::sh::test_dir("board-report-click");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        filed(&state, "b2c3d4", "bug", "the board froze on resize");
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.offers = offers_in(&state, true, &scrub);
        let declined = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = declined.clone();
        let decline_state = state.clone();
        app.reporter.decline = Arc::new(move |id| {
            seen.lock().unwrap().push(id.to_string());
            report::decline_in(&decline_state, &crate::permit::Caller::User, id, crate::messages::now())
        });
        let (rows, _) = render(&mut app, 120, 40);
        let (x, y) = find(&rows, "2 bugs to send · ⏎")[0];
        click(&mut app, x as u16, y as u16);
        assert!(app.offer_panel.focused && !app.offer_panel.shown, "a click selects the folded row");
        click(&mut app, x as u16, y as u16);
        assert!(app.offer_panel.shown && app.offer_panel.selected == 0, "a click on the selected row opens it, as ⏎ does");
        let (rows, _) = render(&mut app, 120, 40);
        let (x, y) = find(&rows, "b2c3d4")[0];
        click(&mut app, x as u16, y as u16);
        assert!(app.offer_panel.focused && app.offer_panel.selected == 1 && matches!(app.mode, Mode::Normal), "a click selects the bug");
        click(&mut app, x as u16, y as u16);
        assert!(matches!(&app.mode, Mode::Report { id, .. } if id == "b2c3d4"), "a second click opens it");
        press(&mut app, key(KeyCode::Char('n')));
        assert_eq!(declined.lock().unwrap().as_slice(), ["b2c3d4"]);
        assert!(!screen(&mut app).contains("the board froze on resize") && offers_in(&state, true, &scrub).len() == 1, "n in the window keeps it here and takes it off the list");
        press(&mut app, key(KeyCode::Char('n')));
        assert!(app.offers.is_empty() && offers_in(&state, true, &scrub).is_empty() && !app.offer_panel.focused, "n in the list keeps the selected one here");
        assert!(!screen(&mut app).contains("to send"), "and with none left the count goes");
    }

    #[test]
    fn a_bug_sent_to_github_or_opened_for_it_is_not_offered() {
        let state = crate::sh::test_dir("board-report-github");
        let linked = Issue { id: "a1b2c3".to_string(), title: "linked".to_string(), kind: "bug".to_string(), status: "open".to_string(), github: Some(format!("{}/issues/7", issues::REPOSITORY)), ..Issue::default() };
        let opened = Issue { id: "b2c3d4".to_string(), title: "opened".to_string(), kind: "bug".to_string(), status: "open".to_string(), sent: Some("2026-10-23".to_string()), ..Issue::default() };
        for issue in [linked, opened] {
            issues::save(&issues::dir(&state), &issue).unwrap();
        }
        let mut app = test_app(Vec::new());
        app.offers = offers_in(&state, true, &scrub);
        assert!(app.offers.is_empty() && !screen(&mut app).contains("to send"));
    }

    #[test]
    fn y_at_the_end_sends_the_selected_bug_to_the_maintainers_and_it_leaves_the_list() {
        let hub = crate::report_wire::fake::FakeIssues::start(8);
        let state = crate::sh::test_dir("board-report-send");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        filed(&state, "b2c3d4", "bug", "the board froze on resize");
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.offers = offers_in(&state, true, &scrub);
        let (send_state, url) = (state.clone(), hub.url.clone());
        app.reporter.send = Arc::new(move |id, digest| report::send_in(&send_state, &crate::permit::Caller::User, id, &url, &scrub(), digest, crate::messages::now()));
        app.offer_panel.shown = true;
        app.focus_offers(0);
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::Report { id, .. } if id == "b2c3d4"), "⏎ opens the selected one");
        for _ in 0..5 {
            press(&mut app, key(KeyCode::PageDown));
            render(&mut app, 120, 40);
        }
        press(&mut app, key(KeyCode::Char('y')));
        assert!(matches!(app.mode, Mode::Working { .. }));
        settle(&mut app);
        let sent = issues::find(&issues::dir(&state), "b2c3d4").unwrap();
        assert!(sent.report.is_some(), "the endpoint took it: {:?} {}", app.toast.as_ref().map(|toast| toast.notice.headline.clone()), screen(&mut app));
        app.offers = offers_in(&state, true, &scrub);
        app.offers_changed();
        let shown = screen(&mut app);
        assert!(!shown.contains("the board froze on resize") && shown.contains("! a1b2c3") && shown.contains("! 1 bug to send"), "the sent bug is gone, the other stays: {shown}");
    }

    #[test]
    fn a_in_the_list_shows_every_bug_and_one_y_sends_them_all_in_the_background() {
        let state = crate::sh::test_dir("board-report-all");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        filed(&state, "b2c3d4", "bug", "the board froze on resize");
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.offers = offers_in(&state, true, &scrub);
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (seen, (release, gate)) = (asked.clone(), std::sync::mpsc::channel::<()>());
        let gate = std::sync::Mutex::new(gate);
        app.reporter.send_all = Arc::new(move |read| {
            seen.lock().unwrap().extend(read.iter().map(|(id, digest)| (id.clone(), *digest)));
            let _ = gate.lock().unwrap().recv();
            Notice::new(Tone::Done, "Sent 2 bugs to kebbie's maintainers").ok()
        });
        let digests: Vec<(String, [u8; 32])> = app.offers.iter().map(|offer| (offer.id.clone(), offer.report.digest())).collect();
        press(&mut app, key(KeyCode::Char('a')));
        assert!(matches!(app.mode, Mode::Normal), "a on a list that is not open does nothing");
        app.offer_panel.shown = true;
        app.focus_offers(0);
        assert!(screen(&mut app).contains("⏎ asks to send · a sends all · n keeps it here · esc hides them"));
        press(&mut app, key(KeyCode::Char('a')));
        assert!(matches!(app.mode, Mode::ReportAll { .. }));
        let (rows, _) = render(&mut app, 120, 16);
        let shown = rows.join("\n");
        assert!(shown.contains("2 bugs as they would go to kebbie's maintainers, rows 1–") && shown.contains("── bug a1b2c3 ──") && shown.contains("Send all 2 bugs to kebbie's maintainers?  y / esc"), "{shown}");
        press(&mut app, key(KeyCode::Char('y')));
        assert!(matches!(app.mode, Mode::ReportAll { nudged: true, read: false, .. }) && asked.lock().unwrap().is_empty(), "y before the end sends nothing");
        for _ in 0..10 {
            press(&mut app, key(KeyCode::PageDown));
            render(&mut app, 120, 16);
        }
        let shown = render(&mut app, 120, 16).0.join("\n");
        assert!(shown.contains("It failed in ~/work/<repo>.") && !shown.contains("/Users/ada"), "the last bug's redacted text: {shown}");
        press(&mut app, key(KeyCode::Char('y')));
        assert!(matches!(app.mode, Mode::Normal) && app.offers.is_empty(), "the board stays free while they go");
        assert!(app.toast.as_ref().is_some_and(|toast| toast.notice.headline == "Sending 2 bugs to kebbie's maintainers"), "{:?}", app.toast.as_ref().map(|toast| toast.notice.headline.clone()));
        app.offers = offers_in(&state, true, &scrub);
        app.offers_changed();
        assert!(app.offers.is_empty(), "a snapshot taken while they go does not offer them again");
        release.send(()).unwrap();
        let start = std::time::Instant::now();
        while app.sending.is_some() && start.elapsed() < std::time::Duration::from_secs(10) {
            app.poll_sending();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(*asked.lock().unwrap(), digests, "exactly the reports shown, once");
        assert!(app.toast.as_ref().is_some_and(|toast| toast.notice.headline == "Sent 2 bugs to kebbie's maintainers"));
    }

    #[test]
    fn a_while_a_batch_is_still_going_starts_no_second_one() {
        let state = crate::sh::test_dir("board-report-all-twice");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        let mut app = test_app(Vec::new());
        app.offers = offers_in(&state, true, &scrub);
        let (_still, result) = std::sync::mpsc::channel();
        app.sending = Some((vec!["b2c3d4".to_string()], result));
        app.offer_panel.shown = true;
        app.focus_offers(0);
        press(&mut app, key(KeyCode::Char('a')));
        assert!(!matches!(app.mode, Mode::ReportAll { .. }), "no second window while the first batch goes");
        assert!(app.sending.as_ref().is_some_and(|(going, _)| going == &["b2c3d4"]), "the batch in flight is kept");
        assert!(screen(&mut app).contains("A batch of reports is still going"), "{}", screen(&mut app));
    }

    #[test]
    fn y_sends_only_once_the_whole_text_has_been_on_screen() {
        let state = crate::sh::test_dir("board-report-long");
        let body: String = (1..=80).map(|row| format!("row {row}\n")).collect();
        filed_with(&state, "a1b2c3", "bug", "a long one", &format!("{body}the last row"));
        let mut app = test_app(Vec::new());
        app.offers = offers_in(&state, true, &scrub);
        app.reporter.send = Arc::new(|_, _| Ok("sent".to_string()));
        app.offer_panel.shown = true;
        app.focus_offers(0);
        press(&mut app, key(KeyCode::Enter));
        let (rows, _) = render(&mut app, 120, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("rows 1–") && !screen.contains("the last row"), "{screen}");
        press(&mut app, key(KeyCode::Char('y')));
        assert!(matches!(app.mode, Mode::Report { nudged: true, read: false, .. }) && app.offers.len() == 1, "y before the end sends nothing");
        let (rows, _) = render(&mut app, 120, 40);
        assert!(rows.join("\n").contains("read to the end first, or press n"));
        for _ in 0..20 {
            press(&mut app, key(KeyCode::PageDown));
            render(&mut app, 120, 40);
        }
        let (rows, _) = render(&mut app, 120, 40);
        assert!(rows.join("\n").contains("the last row"), "{}", rows.join("\n"));
        assert!(matches!(app.mode, Mode::Report { read: true, .. }));
        press(&mut app, key(KeyCode::Char('y')));
        assert!(matches!(app.mode, Mode::Working { .. }) && app.offers.is_empty(), "y sends once the end was on screen");
    }

    #[test]
    fn a_bug_edited_while_its_window_is_open_is_refused_on_y_instead_of_sending_the_unread_text() {
        let state = crate::sh::test_dir("board-report-edited");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        let mut app = test_app(Vec::new());
        app.offers = offers_in(&state, true, &scrub);
        let send_state = state.clone();
        app.reporter.send = Arc::new(move |id, digest| report::send_in(&send_state, &crate::permit::Caller::User, id, "http://127.0.0.1:9", &scrub(), digest, crate::messages::now()));
        app.offer_panel.shown = true;
        app.focus_offers(0);
        press(&mut app, key(KeyCode::Enter));
        render(&mut app, 120, 40);
        filed_with(&state, "a1b2c3", "bug", "push hid git's error", "It failed in /Users/ada/work/shop, and the token was khs_0123456789abcdef.");
        app.offers = offers_in(&state, true, &scrub);
        app.offers_changed();
        let shown = screen(&mut app);
        assert!(shown.contains("It failed in ~/work/<repo>.") && !shown.contains("<token>"), "the window keeps the text that was read: {shown}");
        press(&mut app, key(KeyCode::Char('y')));
        settle(&mut app);
        let shown = screen(&mut app);
        assert!(shown.contains("changed since you read it"), "{shown}");
        assert!(issues::find(&issues::dir(&state), "a1b2c3").unwrap().report.is_none());
    }

    #[test]
    fn y_on_the_send_all_window_sends_the_bugs_it_showed_and_not_one_that_arrived_after() {
        let state = crate::sh::test_dir("board-report-all-arrived");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        let mut app = test_app(Vec::new());
        app.offers = offers_in(&state, true, &scrub);
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = asked.clone();
        app.reporter.send_all = Arc::new(move |read| {
            seen.lock().unwrap().extend(read.iter().map(|(id, _)| id.clone()));
            Notice::new(Tone::Done, "Sent 1 bug to kebbie's maintainers").ok()
        });
        app.offer_panel.shown = true;
        app.focus_offers(0);
        press(&mut app, key(KeyCode::Char('a')));
        render(&mut app, 120, 40);
        filed(&state, "b2c3d4", "bug", "the board froze on resize");
        app.offers = offers_in(&state, true, &scrub);
        app.offers_changed();
        let shown = screen(&mut app);
        assert!(shown.contains("Send all 1 bug to kebbie's maintainers?") && !shown.contains("── bug b2c3d4 ──"), "{shown}");
        press(&mut app, key(KeyCode::Char('y')));
        let start = std::time::Instant::now();
        while app.sending.is_some() && start.elapsed() < std::time::Duration::from_secs(10) {
            app.poll_sending();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(*asked.lock().unwrap(), ["a1b2c3"]);
        assert_eq!(app.offers.iter().map(|offer| offer.id.as_str()).collect::<Vec<_>>(), ["b2c3d4"], "the unread bug still waits");
    }

    #[test]
    fn the_issues_an_update_fixed_show_under_work_until_the_inbox_key_lists_and_clears_them() {
        let titles: Vec<String> = ["push hid git's error", "the board froze on resize", "stage lost a commit", "a fourth"].iter().map(|title| title.to_string()).collect();
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.fixed = Some(Fixed { version: "0.26.3".to_string(), titles: titles.clone(), bugs: titles.clone(), wishes: 0, last: 500 });
        let (rows, _) = render(&mut app, 120, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("✓ Fixed in kebbie 0.26.3: 4 bugs you ran into") && screen.contains("   i shows the list · ⏎ options"), "{screen}");
        assert!(screen.contains("  push hid git's error") && screen.contains("  stage lost a commit") && !screen.contains("a fourth") && screen.contains("… and 1 more"), "{screen}");
        press(&mut app, key(KeyCode::Char('i')));
        assert!(app.fixed.is_none() && app.fixed_seen == 500, "the line goes once it was opened, and a snapshot read before it cannot bring it back");
        let Mode::Message(text) = &app.mode else { panic!("a window that stays until a key") };
        assert!(text.contains("Fixed in kebbie 0.26.3: 4 bugs you ran into") && text.contains("a fourth") && text.contains("kebbie issues --all"), "{text}");
        app.fixed = Some(Fixed { version: "0.26.3".to_string(), titles: vec!["x".to_string()], bugs: vec!["x".to_string()], wishes: 0, last: 600 });
        app.questions = crate::messages::fold(QUESTION);
        let hint = |questions: bool| fixed_block(app.fixed.as_ref().unwrap(), questions, 120, false).iter().map(|line| line.to_string()).collect::<Vec<_>>().join("\n");
        assert!(hint(true).contains("i shows the list, once the questions are answered · ⏎ options"), "{}", hint(true));
        assert!(hint(false).ends_with("i shows the list · ⏎ options"), "{}", hint(false));
        press(&mut app, key(KeyCode::Esc));
        press(&mut app, key(KeyCode::Char('i')));
        assert!(matches!(app.mode, Mode::Questions(_)) && app.fixed.is_some(), "questions come first");
    }

    fn wish_done() -> Fixed {
        Fixed { version: "main".to_string(), titles: vec!["a dark theme for shop".to_string()], bugs: Vec::new(), wishes: 1, last: 700 }
    }

    #[test]
    fn the_cursor_steps_onto_the_fixed_block_and_its_options_hide_it_for_good() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.fixed = Some(wish_done());
        press(&mut app, key(KeyCode::Char('j')));
        assert!(app.fixed_focused(), "↓ from the last task selects the block");
        assert!(screen(&mut app).contains("▌✓ "), "the block is marked as selected");
        press(&mut app, key(KeyCode::Char('k')));
        assert!(!app.offer_panel.focused && app.work_selected == 0, "↑ goes back to the task");
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::FixedOptions(0)), "⏎ on the block opens its options");
        let shown = screen(&mut app);
        assert!(shown.contains("Show the list") && shown.contains("Hide this"), "{shown}");
        press(&mut app, key(KeyCode::Char('j')));
        assert!(matches!(app.mode, Mode::FixedOptions(1)));
        press(&mut app, key(KeyCode::Enter));
        assert!(app.fixed.is_none() && app.fixed_seen == 700 && matches!(app.mode, Mode::Normal), "Hide this clears it without the list");
        assert!(!app.offer_panel.focused && app.work_selected == 0, "the cursor goes back to the task");
        assert!(!screen(&mut app).contains("a dark theme for shop"), "the block is gone");
        app.fixed = Some(wish_done()).filter(|fixed| fixed.last > app.fixed_seen);
        app.fixed_changed();
        assert!(app.fixed.is_none(), "a snapshot read before it cannot bring it back");
    }

    #[test]
    fn show_the_list_in_the_fixed_blocks_options_opens_the_list() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.fixed = Some(wish_done());
        press(&mut app, key(KeyCode::Char('j')));
        press(&mut app, key(KeyCode::Enter));
        press(&mut app, key(KeyCode::Enter));
        let Mode::Message(text) = &app.mode else { panic!("Show the list opens the list") };
        assert!(text.contains("a dark theme for shop") && text.contains("kebbie issues --all"), "{text}");
        assert!(app.fixed.is_none(), "shown, the block goes");
    }

    #[test]
    fn the_cursor_passes_the_issues_row_on_its_way_to_the_fixed_block() {
        let state = crate::sh::test_dir("board-fixed-under-offers");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.offers = offers_in(&state, true, &scrub);
        app.fixed = Some(wish_done());
        press(&mut app, key(KeyCode::Char('j')));
        assert!(app.offer_panel.focused && !app.fixed_focused(), "the issues row comes first");
        press(&mut app, key(KeyCode::Char('j')));
        assert!(app.fixed_focused(), "then the block");
        press(&mut app, key(KeyCode::Char('k')));
        assert!(app.offer_panel.focused && !app.fixed_focused(), "↑ goes back to the issues row");
    }

    #[test]
    fn x_is_no_board_key() {
        assert!(Act::EVERY.iter().all(|act| act.id() != "dismiss_fixed"));
        for ctx in [crate::keys::Ctx::Work, crate::keys::Ctx::Backlog] {
            assert_eq!(keys::map().act(ctx, &key(KeyCode::Char('X'))), None, "{ctx:?}");
        }
    }
}
