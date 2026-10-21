use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::backlog;
use crate::fetch;
use crate::keys::{self, Act};
use crate::status::Group;
use super::{App, Page};
use super::widgets::{BLUE, FAINT, GREEN, MANTLE, MAUVE, MUTED, PEACH, SURFACE, TEAL, YELLOW, bold, plural, style};

fn pill(text: String, color: Color, lit: bool) -> Vec<Span<'static>> {
    let color = if lit { color } else { FAINT };
    vec![Span::styled(format!(" {text} "), Style::new().fg(color).bg(MANTLE)), Span::raw(" ")]
}

fn pills(app: &App, compact: bool) -> Vec<Span<'static>> {
    let count = |group: Group| app.tasks.iter().filter(|item| item.group() == group).count();
    let (needs, working, review, ready) = (count(Group::NeedsYou), count(Group::Working), count(Group::ToReview), count(Group::ReadyToPush));
    let ci = count(Group::CiRunning);
    let questions = app.questions.len();
    let mut spans = vec![Span::styled("kelpie", bold(MAUVE)), Span::raw(if compact { "  " } else { "   " })];
    if compact {
        for (shown, color, lit) in [(format!("● {needs}"), PEACH, needs > 0), (format!("◐ {working}"), BLUE, working > 0), (format!("◇ {review}"), TEAL, review > 0), (format!("✓ {ready}"), GREEN, ready > 0), (format!("⧗ {ci}"), YELLOW, ci > 0), (format!("? {questions}"), YELLOW, questions > 0), (format!("↻ {}", app.stale.len()), MAUVE, !app.stale.is_empty()), ("☾".to_string(), MAUVE, app.away)] {
            if lit {
                spans.extend(pill(shown, color, true));
            }
        }
        return spans;
    }
    spans.extend(pill(format!("● {needs} need you"), PEACH, needs > 0));
    spans.extend(pill(format!("◐ {working} working"), BLUE, working > 0));
    if review > 0 {
        spans.extend(pill(format!("◇ {review} to review"), TEAL, true));
    }
    spans.extend(pill(format!("✓ {ready} ready to push"), GREEN, ready > 0));
    if ci > 0 {
        spans.extend(pill(format!("⧗ {ci} CI running"), YELLOW, true));
    }
    spans.extend(pill(format!("? {}", plural(questions, "question")), YELLOW, questions > 0));
    if !app.stale.is_empty() {
        spans.extend(pill(format!("↻ {} stale", app.stale.len()), MAUVE, true));
    }
    if app.away {
        spans.extend(pill("☾ away".to_string(), MAUVE, true));
    }
    if app.lead_chat == Some(true) {
        spans.extend(pill(format!("◧ chat hidden · {}", keys::map().label(Act::HideChat)), MAUVE, true));
    }
    spans
}

pub(super) fn header(app: &App, width: u16) -> (Line<'static>, Vec<(u16, u16, Page)>) {
    let mut spans = pills(app, false);
    let sources = backlog::label(&app.sources);
    let fresh = match app.page {
        Page::Work => format!("backlog: {sources}, {}   ", backlog::freshness(&app.sources, app.backlog.updated)),
        Page::Areas => format!("vault index: {}   ", backlog::age(app.areas.synced)),
        Page::Routines => String::new(),
    };
    let fresh = format!("{}{fresh}", fetch::failed_note(&app.fetch_failures));
    let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    let waiting = app.routines.waiting();
    let warning = if waiting > 0 { format!("⚠{waiting} ") } else { String::new() };
    let full = [(" 1 Work ", Page::Work), (" 2 Areas ", Page::Areas), (" 3 Routines ", Page::Routines)];
    let short = full.map(|(label, page)| if page == app.page { (label, page) } else { (&label[..3], page) });
    let width_of = |tabs: &[(&str, Page)]| tabs.iter().map(|(label, _)| label.chars().count()).sum::<usize>() + warning.chars().count();
    let tabs = if used + width_of(&full) <= width as usize { full } else { short };
    if used + width_of(&tabs) > width as usize {
        spans = pills(app, true);
    }
    let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    let tabs_width = width_of(&tabs);
    let fresh = if used + fresh.chars().count() + tabs_width < width as usize { fresh } else { String::new() };
    let mut places = Vec::new();
    if used + fresh.chars().count() + tabs_width <= width as usize {
        spans.push(Span::raw(" ".repeat(width as usize - used - fresh.chars().count() - tabs_width)));
        spans.push(Span::styled(fresh, style(MUTED)));
        let mut x = width as usize - tabs_width;
        for (label, page) in tabs {
            let lit = app.page == page;
            spans.push(Span::styled(label, if lit { bold(MAUVE).bg(SURFACE) } else { style(MUTED) }));
            let mut taken = label.chars().count();
            if page == Page::Routines && !warning.is_empty() {
                spans.push(Span::styled(warning.clone(), style(PEACH)));
                taken += warning.chars().count();
            }
            places.push((x as u16, taken as u16, page));
            x += taken;
        }
    }
    (Line::from(spans), places)
}
