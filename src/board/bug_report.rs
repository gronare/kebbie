use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};
use ratatui::Frame;
use kelpie_hub_protocol::BugReport;

use crate::hub::report::{self, Scrub};
use crate::issue_recheck::Fixed;
use crate::keys::{self, Act};
use crate::notice::{Notice, Tone};
use super::{App, Mode};
use super::widgets::{FAINT, GREEN, MUTED, PEACH, TEXT, YELLOW, bold, group_line, modal, style, wrap};

const PAGE: u16 = 10;
const FRAME_ROWS: u16 = 14;
const FIXED_TITLES: usize = 3;

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

pub(super) fn offer_lines(offers: &[Offer], questions: bool, width: usize) -> Vec<Line<'static>> {
    if offers.is_empty() {
        return Vec::new();
    }
    let inbox = keys::key(Act::Inbox);
    let mut lines = vec![Line::default(), group_line("Send to kelpie's maintainers?", offers.len(), YELLOW)];
    let after = if questions { ", once the questions are answered" } else { "" };
    lines.push(Line::from(Span::styled(format!("bugs kelpie filed about itself · {inbox} shows the text and asks{after}"), style(MUTED))));
    for offer in offers {
        let title = wrap(&offer.report.title, width.saturating_sub(12), 1).into_iter().next().unwrap_or_default();
        lines.push(Line::from(vec![Span::styled("! ", style(YELLOW)), Span::styled(format!("{}  ", offer.id), style(FAINT)), Span::styled(title, style(TEXT))]));
    }
    lines
}

pub(super) fn load_fixed() -> Option<Fixed> {
    crate::issue_recheck::shown(&crate::lead::state_dir().ok()?, crate::messages::now())
}

pub(super) fn fixed_lines(fixed: Option<&Fixed>, questions: bool, offers: bool, width: usize) -> Vec<Line<'static>> {
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
    let after = if questions { ", once the questions are answered" } else if offers { ", once the reports above are decided" } else { "" };
    lines.push(Line::from(Span::styled(format!("  {} lists them and clears this{after}", keys::key(Act::Inbox)), style(MUTED))));
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

    pub(super) fn open_report(&mut self) -> bool {
        let Some(offer) = self.offers.first() else { return false };
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
                let digest = offer.report.digest();
                self.in_background("Sending the report to kelpie's maintainers…".to_string(), move || {
                    let settings = crate::config::report_settings()?;
                    report::send_in(&crate::lead::state_dir()?, &crate::permit::caller(), &id, settings.url(), &Scrub::current(), &digest, crate::messages::now())
                });
            }
            KeyCode::Char('n') => {
                self.subject = Some(super::notices::Subject::plain("report"));
                self.offers.retain(|offer| offer.id != id);
                let declined = crate::lead::state_dir().and_then(|state| report::decline_in(&state, &crate::permit::caller(), &id, crate::messages::now()));
                self.show(declined);
            }
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
    let mut lines = vec![Line::from(Span::styled(format!("kelpie issue {} as it would go out{}:", offer.id, if last > 0 { format!(", rows {}–{} of {}", scroll + 1, scroll + view, rows.len()) } else { String::new() }), style(MUTED))), Line::default()];
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

    #[test]
    fn a_bug_kelpie_filed_waits_under_work_and_the_inbox_key_shows_its_text_and_asks() {
        let state = crate::sh::test_dir("board-report");
        filed(&state, "a1b2c3", "bug", "push hid git's error");
        filed(&state, "b2c3d4", "wish", "a dark theme");
        let offers = offers_in(&state, true, &scrub);
        assert_eq!(offers.iter().map(|offer| offer.id.as_str()).collect::<Vec<_>>(), ["a1b2c3"], "a wish is not offered");
        assert!(offers_in(&state, false, &|| panic!("nothing is scrubbed when the offer is off")).is_empty(), "[report] text other than \"ask\" hides it");

        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.offers = offers;
        let (rows, _) = render(&mut app, 120, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("Send to kelpie's maintainers?  1") && screen.contains("! a1b2c3  push hid git's error") && screen.contains("i shows the text and asks"), "{screen}");
        assert!(!screen.contains("a dark theme"), "{screen}");

        press(&mut app, key(KeyCode::Char('i')));
        assert!(matches!(&app.mode, Mode::Report { id, nudged: false, .. } if id == "a1b2c3"), "with no question waiting, the inbox key opens the report");
        let (rows, _) = render(&mut app, 120, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("Send this report to kelpie's maintainers?  y / n") && screen.contains("It failed in ~/work/<repo>."), "the text as it would go out: {screen}");
        assert!(!screen.contains("/Users/carl") && !screen.contains("greenhouse"), "{screen}");
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Report { nudged: true, .. }), "another key keeps the question");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal) && app.offers.len() == 1, "esc decides later");

        app.questions = crate::messages::fold(QUESTION);
        press(&mut app, key(KeyCode::Char('i')));
        assert!(matches!(app.mode, Mode::Questions(_)), "questions come first");
        let hint: String = offer_lines(&app.offers, true, 120).iter().map(|line| line.to_string()).collect::<Vec<_>>().join("\n");
        assert!(hint.contains("i shows the text and asks, once the questions are answered"), "{hint}");
    }

    #[test]
    fn y_sends_only_once_the_whole_text_has_been_on_screen() {
        let state = crate::sh::test_dir("board-report-long");
        let body: String = (1..=80).map(|row| format!("row {row}\n")).collect();
        filed_with(&state, "a1b2c3", "bug", "a long one", &format!("{body}the last row"));
        let mut app = test_app(Vec::new());
        app.offers = offers_in(&state, true, &scrub);
        press(&mut app, key(KeyCode::Char('i')));
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
        app.fixed = Some(Fixed { version: "0.26.3".to_string(), titles: titles.clone(), last: 500 });
        let (rows, _) = render(&mut app, 120, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("✓ kelpie 0.26.3 fixed 4 of your issues") && screen.contains("  i lists them and clears this"), "{screen}");
        assert!(screen.contains("  push hid git's error") && screen.contains("  stage lost a commit") && !screen.contains("a fourth") && screen.contains("… and 1 more"), "{screen}");
        press(&mut app, key(KeyCode::Char('i')));
        assert!(app.fixed.is_none() && app.fixed_seen == 500, "the line goes once it was opened, and a snapshot read before it cannot bring it back");
        let Mode::Message(text) = &app.mode else { panic!("a window that stays until a key") };
        assert!(text.contains("kelpie 0.26.3 fixed 4 of your issues") && text.contains("a fourth") && text.contains("kelpie issues --all"), "{text}");
        app.fixed = Some(Fixed { version: "0.26.3".to_string(), titles: vec!["x".to_string()], last: 600 });
        app.questions = crate::messages::fold(QUESTION);
        let hint = |questions: bool, offers: bool| fixed_lines(app.fixed.as_ref(), questions, offers, 120).iter().map(|line| line.to_string()).collect::<Vec<_>>().join("\n");
        assert!(hint(true, false).contains("i lists them and clears this, once the questions are answered"), "{}", hint(true, false));
        assert!(hint(false, true).contains("i lists them and clears this, once the reports above are decided"), "{}", hint(false, true));
        press(&mut app, key(KeyCode::Esc));
        press(&mut app, key(KeyCode::Char('i')));
        assert!(matches!(app.mode, Mode::Questions(_)) && app.fixed.is_some(), "questions come first");
    }
}
