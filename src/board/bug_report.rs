use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;
use kelpie_hub_protocol::BugReport;

use crate::hub::report::{self, Scrub};
use crate::issue_recheck::Fixed;
use crate::keys::{self, Act};
use crate::notice::{Notice, Tone};
use crate::sh::Result;
use super::{App, Hit, Mode};
use super::columns::Rows;
use super::widgets::{FAINT, GREEN, MUTED, PEACH, SURFACE, TEXT, YELLOW, bold, group_line, modal, style, wrap};

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
}

type SendFn = Arc<dyn Fn(&str, &[u8; 32]) -> Result<String> + std::marker::Send + Sync>;
type DeclineFn = Arc<dyn Fn(&str) -> Result<String> + std::marker::Send + Sync>;

#[derive(Clone)]
pub(super) struct Reporter {
    pub(super) send: SendFn,
    pub(super) decline: DeclineFn,
}

impl Default for Reporter {
    fn default() -> Self {
        Reporter {
            send: Arc::new(|id, digest| {
                let settings = crate::config::report_settings()?;
                report::send_in(&crate::lead::state_dir()?, &crate::permit::caller(), id, settings.url(), &Scrub::current(), digest, crate::messages::now())
            }),
            decline: Arc::new(|id| report::decline_in(&crate::lead::state_dir()?, &crate::permit::caller(), id, crate::messages::now())),
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

pub(super) fn indicator(offers: usize, compact: bool) -> Option<String> {
    let key = keys::key(Act::Reports);
    (offers > 0).then(|| if compact { format!("! {offers}") } else { format!("! {} to send · {key}", super::widgets::plural(offers, "bug")) })
}

// The lines, each offer's rows, and the line of the selected offer when the list has the focus, all counted from `first`.
pub(super) fn offer_lines(app: &App, width: usize, first: usize) -> (Vec<Line<'static>>, Rows, Option<usize>) {
    if !app.offers_visible() {
        return (Vec::new(), Vec::new(), None);
    }
    let key = keys::key(Act::Reports);
    let panel = &app.offer_panel;
    let mut lines = vec![Line::default(), group_line("Send to kelpie's maintainers?", app.offers.len(), YELLOW)];
    let hint = if panel.focused { format!("↑ ↓ pick · ⏎ shows the text and asks · n keeps it here only · esc back · {key} hides") } else { format!("bugs in kelpie you ran into · {key} selects · {key} again hides") };
    lines.push(Line::from(Span::styled(hint, style(MUTED))));
    let mut rows = Vec::new();
    let mut selected_line = None;
    for (index, offer) in app.offers.iter().enumerate() {
        let title = wrap(&offer.report.title, width.saturating_sub(12), 1).into_iter().next().unwrap_or_default();
        let line = Line::from(vec![Span::styled("! ", style(YELLOW)), Span::styled(format!("{}  ", offer.id), style(FAINT)), Span::styled(title, style(TEXT))]);
        let selected = panel.focused && index == panel.selected;
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

pub(super) fn fixed_lines(fixed: Option<&Fixed>, questions: bool, width: usize) -> Vec<Line<'static>> {
    let Some(fixed) = fixed else { return Vec::new() };
    let mut lines = vec![Line::default(), Line::from(Span::styled(format!("✓ {}", crate::issue_recheck::headline(fixed)), style(GREEN)))];
    for title in fixed.titles.iter().take(FIXED_TITLES) {
        let title = wrap(title, width.saturating_sub(4), 1).into_iter().next().unwrap_or_default();
        lines.push(Line::from(vec![Span::raw("  "), Span::styled(title, style(TEXT))]));
    }
    let more = fixed.titles.len().saturating_sub(FIXED_TITLES);
    if more > 0 {
        lines.push(Line::from(Span::styled(format!("  … and {more} more"), style(MUTED))));
    }
    let after = if questions { ", once the questions are answered" } else { "" };
    lines.push(Line::from(Span::styled(format!("  {} shows the list{after}", keys::key(Act::Inbox)), style(MUTED))));
    lines
}

impl App {
    pub(super) fn open_fixed(&mut self) -> bool {
        let Some(fixed) = self.fixed.take() else { return false };
        self.fixed_seen = self.fixed_seen.max(fixed.last);
        if let Ok(state) = crate::lead::state_dir() {
            let _ = crate::issue_recheck::dismiss(&state, &fixed, crate::messages::now());
        }
        let notice = fixed.titles.iter().fold(Notice::new(Tone::Done, crate::issue_recheck::headline(&fixed)).next("kelpie issues --all lists them, each with the commit that fixed it."), |notice, title| notice.detail("fixed", title));
        self.subject = Some(super::notices::Subject::plain("issues"));
        self.show_as(notice.ok(), true);
        true
    }

    pub(super) fn offers_visible(&self) -> bool {
        self.offer_panel.shown && !self.offers.is_empty()
    }

    pub(super) fn offers_changed(&mut self) {
        self.offer_panel.selected = self.offer_panel.selected.min(self.offers.len().saturating_sub(1));
        self.offer_panel.focused &= self.offers_visible();
    }

    fn set_shown(&mut self, shown: bool) {
        self.offer_panel.shown = shown;
        save_shown(self.offer_panel.file.as_deref(), shown);
    }

    pub(super) fn focus_offers(&mut self, index: usize) {
        if !self.offer_panel.shown {
            self.set_shown(true);
        }
        self.in_messages = false;
        self.column = super::Column::Work;
        self.offer_panel.focused = !self.offers.is_empty();
        self.offer_panel.selected = index.min(self.offers.len().saturating_sub(1));
    }

    pub(super) fn press_reports(&mut self) {
        match (self.offer_panel.shown, self.offer_panel.focused) {
            (true, true) => {
                self.offer_panel.focused = false;
                self.set_shown(false);
            }
            (true, false) if self.offers.is_empty() => self.set_shown(false),
            (_, false) if self.offers.is_empty() => {
                self.set_shown(true);
                self.say("No bugs wait to be sent to kelpie's maintainers".to_string());
            }
            _ => self.focus_offers(0),
        }
    }

    pub(super) fn handle_offers(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('n') {
            return self.decline_offer();
        }
        match keys::map().act(self.ctx(), &key) {
            Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => match self.offer_panel.selected.checked_sub(1) {
                Some(above) => self.offer_panel.selected = above,
                None => self.offer_panel.focused = self.tasks.is_empty(),
            },
            Some(Act::Move) => self.offer_panel.selected = (self.offer_panel.selected + 1).min(self.offers.len().saturating_sub(1)),
            Some(Act::Options) => {
                self.open_report();
            }
            Some(Act::Back | Act::Close) => self.offer_panel.focused = false,
            Some(Act::Switch) => (self.offer_panel.focused, self.column) = (false, super::Column::Backlog),
            Some(Act::Reports) => self.press_reports(),
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
        self.mode = Mode::Report { id: offer.id.clone(), scroll: 0, read: false, nudged: false };
        true
    }

    // y counts only once the whole text has been on screen, and sends exactly the report that was shown.
    pub(super) fn confirm_report(&mut self, id: String, scroll: u16, read: bool, key: KeyEvent, cancels: bool) {
        let Some(offer) = self.offers.iter().find(|offer| offer.id == id).cloned() else { return };
        match key.code {
            KeyCode::Char('y') if read => {
                self.subject = Some(super::notices::Subject::plain("report"));
                self.offers.retain(|offer| offer.id != id);
                self.offers_changed();
                let digest = offer.report.digest();
                let send = self.reporter.send.clone();
                self.in_background("Sending the report to kelpie's maintainers…".to_string(), move || send(&id, &digest));
            }
            KeyCode::Char('n') => self.decline(&id),
            KeyCode::Down | KeyCode::Char('j') => self.mode = Mode::Report { id, scroll: scroll.saturating_add(1), read, nudged: false },
            KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::Report { id, scroll: scroll.saturating_sub(1), read, nudged: false },
            KeyCode::PageDown | KeyCode::Char(' ') => self.mode = Mode::Report { id, scroll: scroll.saturating_add(PAGE), read, nudged: false },
            KeyCode::PageUp => self.mode = Mode::Report { id, scroll: scroll.saturating_sub(PAGE), read, nudged: false },
            _ if cancels => {}
            _ => self.mode = Mode::Report { id, scroll, read, nudged: true },
        }
    }
}

fn preview_rows(report: &BugReport, width: usize) -> Vec<String> {
    report::preview(report).lines().flat_map(|line| if line.is_empty() { vec![String::new()] } else { wrap(line, width, usize::MAX) }).collect()
}

// Returns the scroll it drew at and whether the text's last row was on screen.
pub(super) fn draw_report(frame: &mut Frame, app: &App, id: &str, scroll: u16, read: bool, nudged: bool) -> (u16, bool) {
    let Some(offer) = app.offers.iter().find(|offer| offer.id == id) else { return (scroll, read) };
    let width = 88u16.min(frame.area().width.saturating_sub(4));
    let rows = preview_rows(&offer.report, usize::from(width.saturating_sub(4)).max(20));
    let view = usize::from(frame.area().height.saturating_sub(FRAME_ROWS)).max(3).min(rows.len());
    let last = rows.len().saturating_sub(view);
    let scroll = usize::from(scroll).min(last);
    let read = read || scroll >= last;
    let mut lines = vec![Line::from(Span::styled(format!("Bug {} as it would go to kelpie's maintainers{}:", offer.id, if last > 0 { format!(", rows {}–{} of {}", scroll + 1, scroll + view, rows.len()) } else { String::new() }), style(MUTED))), Line::default()];
    lines.extend(rows.iter().enumerate().skip(scroll).take(view).map(|(index, row)| Line::from(Span::styled(row.clone(), if index == 0 { bold(TEXT) } else { style(TEXT) }))));
    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled("Send this report to kelpie's maintainers?", style(YELLOW)), Span::styled("  y / n", style(MUTED))]));
    let keys = if read { "y sends it, signed with this install's key · n keeps it here only · esc decides later" } else { "↓ pgdn read on: y counts once you have read to the end · n keeps it here only · esc decides later" };
    lines.push(Line::from(Span::styled(keys, style(MUTED))));
    if nudged {
        lines.push(Line::from(Span::styled(if read { "press y or n" } else { "read to the end first, or press n" }, style(PEACH))));
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
        filed_with(state, id, kind, title, "It failed in /Users/carl/work/greenhouse.");
    }

    fn filed_with(state: &std::path::Path, id: &str, kind: &str, title: &str, body: &str) {
        let issue = Issue { id: id.to_string(), title: title.to_string(), kind: kind.to_string(), status: "open".to_string(), version: "0.26.0".to_string(), by: "lead".to_string(), body: body.to_string(), ..Issue::default() };
        issues::save(&issues::dir(state), &issue).unwrap();
    }

    fn scrub() -> Scrub {
        Scrub::new(Some("/Users/carl".to_string()), vec!["greenhouse".to_string()])
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

    #[test]
    fn the_bugs_to_send_stay_hidden_behind_a_count_in_the_header_until_b_shows_them() {
        let state = crate::sh::test_dir("board-report");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        filed(&state, "b2c3d4", "wish", "a dark theme");
        let offers = offers_in(&state, true, &scrub);
        assert_eq!(offers.iter().map(|offer| offer.id.as_str()).collect::<Vec<_>>(), ["a1b2c3"], "a wish is not offered");
        assert!(offers_in(&state, false, &|| panic!("nothing is scrubbed when the offer is off")).is_empty(), "[report] text other than \"ask\" hides it");

        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        assert!(!screen(&mut app).contains("to send"), "no bugs, no count");
        app.offers = offers;
        let shown = screen(&mut app);
        assert!(shown.contains("! 1 bug to send · B") && !shown.contains("Send to kelpie's maintainers?") && !shown.contains("push hid git's error"), "hidden at first: {shown}");

        press(&mut app, key(KeyCode::Char('B')));
        let shown = screen(&mut app);
        assert!(shown.contains("Send to kelpie's maintainers?  1") && shown.contains("! a1b2c3  push hid git's error") && shown.contains("⏎ shows the text and asks"), "{shown}");
        assert!(app.offer_panel.focused, "B shows the list with its first bug selected");
        press(&mut app, key(KeyCode::Esc));
        assert!(!app.offer_panel.focused && app.offer_panel.shown && screen(&mut app).contains("! a1b2c3"), "esc goes back to the tasks and leaves the list shown");
        press(&mut app, key(KeyCode::Down));
        assert!(app.offer_panel.focused, "↓ from the last task goes on into the list");
        press(&mut app, key(KeyCode::Char('B')));
        assert!(!app.offer_panel.shown && !screen(&mut app).contains("! a1b2c3"), "B again hides it");

        app.questions = crate::messages::fold(QUESTION);
        press(&mut app, key(KeyCode::Char('B')));
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::Report { id, nudged: false, .. } if id == "a1b2c3"), "a question waiting does not stand in the way");
        let shown = screen(&mut app);
        assert!(shown.contains("Send this report to kelpie's maintainers?  y / n") && shown.contains("It failed in ~/work/<repo>."), "the text as it would go out: {shown}");
        assert!(!shown.contains("/Users/carl") && !shown.contains("greenhouse"), "{shown}");
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Report { nudged: true, .. }), "another key keeps the question");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal) && app.offers.len() == 1 && app.offer_panel.focused, "esc decides later, back in the list");
        press(&mut app, key(KeyCode::Char('i')));
        assert!(matches!(app.mode, Mode::Questions(_)), "i is the questions");
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
        press(&mut app, key(KeyCode::Char('B')));
        assert!(load_shown(Some(&file)));
        press(&mut app, key(KeyCode::Char('B')));
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
        app.offer_panel.shown = true;
        let (rows, _) = render(&mut app, 120, 40);
        let (x, y) = find(&rows, "b2c3d4")[0];
        click(&mut app, x as u16, y as u16);
        assert!(app.offer_panel.focused && app.offer_panel.selected == 1 && matches!(app.mode, Mode::Normal), "a click selects the bug");
        click(&mut app, x as u16, y as u16);
        assert!(matches!(&app.mode, Mode::Report { id, .. } if id == "b2c3d4"), "a second click opens it");
        press(&mut app, key(KeyCode::Char('n')));
        assert_eq!(declined.lock().unwrap().as_slice(), ["b2c3d4"]);
        assert!(!screen(&mut app).contains("the board froze on resize") && offers_in(&state, true, &scrub).len() == 1, "n in the window keeps it here and takes it off the list");
        press(&mut app, key(KeyCode::Esc));
        press(&mut app, key(KeyCode::Char('B')));
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
    fn y_at_the_end_sends_the_selected_bug_to_the_hub_and_it_leaves_the_list() {
        let hub = crate::hub::tests::TestHub::start_with(|config| config.report_difficulty = 8);
        let state = crate::sh::test_dir("board-report-send");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        filed(&state, "b2c3d4", "bug", "the board froze on resize");
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.offers = offers_in(&state, true, &scrub);
        let (send_state, url) = (state.clone(), hub.url.clone());
        app.reporter.send = Arc::new(move |id, digest| report::send_in(&send_state, &crate::permit::Caller::User, id, &url, &scrub(), digest, crate::messages::now()));
        press(&mut app, key(KeyCode::Char('B')));
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
        assert!(sent.report.is_some(), "the hub took it: {:?} {}", app.toast.as_ref().map(|toast| toast.notice.headline.clone()), screen(&mut app));
        app.offers = offers_in(&state, true, &scrub);
        app.offers_changed();
        let shown = screen(&mut app);
        assert!(!shown.contains("the board froze on resize") && shown.contains("! a1b2c3") && shown.contains("! 1 bug to send"), "the sent bug is gone, the other stays: {shown}");
    }

    #[test]
    fn y_sends_only_once_the_whole_text_has_been_on_screen() {
        let state = crate::sh::test_dir("board-report-long");
        let body: String = (1..=80).map(|row| format!("row {row}\n")).collect();
        filed_with(&state, "a1b2c3", "bug", "a long one", &format!("{body}the last row"));
        let mut app = test_app(Vec::new());
        app.offers = offers_in(&state, true, &scrub);
        app.reporter.send = Arc::new(|_, _| Ok("sent".to_string()));
        press(&mut app, key(KeyCode::Char('B')));
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
    fn the_issues_an_update_fixed_show_under_work_until_the_inbox_key_lists_and_clears_them() {
        let titles: Vec<String> = ["push hid git's error", "the board froze on resize", "stage lost a commit", "a fourth"].iter().map(|title| title.to_string()).collect();
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.fixed = Some(Fixed { version: "0.26.3".to_string(), titles: titles.clone(), bugs: titles.clone(), wishes: 0, last: 500 });
        let (rows, _) = render(&mut app, 120, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("✓ Fixed in kelpie 0.26.3: 4 bugs you ran into") && screen.contains("  i shows the list"), "{screen}");
        assert!(screen.contains("  push hid git's error") && screen.contains("  stage lost a commit") && !screen.contains("a fourth") && screen.contains("… and 1 more"), "{screen}");
        press(&mut app, key(KeyCode::Char('i')));
        assert!(app.fixed.is_none() && app.fixed_seen == 500, "the line goes once it was opened, and a snapshot read before it cannot bring it back");
        let Mode::Message(text) = &app.mode else { panic!("a window that stays until a key") };
        assert!(text.contains("Fixed in kelpie 0.26.3: 4 bugs you ran into") && text.contains("a fourth") && text.contains("kelpie issues --all"), "{text}");
        app.fixed = Some(Fixed { version: "0.26.3".to_string(), titles: vec!["x".to_string()], bugs: vec!["x".to_string()], wishes: 0, last: 600 });
        app.questions = crate::messages::fold(QUESTION);
        let hint = |questions: bool| fixed_lines(app.fixed.as_ref(), questions, 120).iter().map(|line| line.to_string()).collect::<Vec<_>>().join("\n");
        assert!(hint(true).contains("i shows the list, once the questions are answered"), "{}", hint(true));
        assert!(hint(false).ends_with("i shows the list"), "{}", hint(false));
        press(&mut app, key(KeyCode::Esc));
        press(&mut app, key(KeyCode::Char('i')));
        assert!(matches!(app.mode, Mode::Questions(_)) && app.fixed.is_some(), "questions come first");
    }
}
