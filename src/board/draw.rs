use std::time::Duration;

use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::area_page;
use crate::messages;
use super::card::{draw_card, draw_fullscreen};
use super::columns::{backlog_lines, work_lines};
use super::confirm::{draw_done, draw_drop, draw_queued, draw_reset};
use super::detail::draw_panel;
use super::editor::draw_editor;
use super::header::header;
use super::inbox::draw_questions;
use super::input::{draw_filter, draw_input};
use super::link::{link_cells, link_urls, start_noting, take_noted};
use crate::notice::Notice;
use super::notices::{TOAST_FOR, draw_notice, draw_toast};
use super::message_log::{Fold, messages_height, messages_lines};
use super::{App, Column, Hit, Mode, Page};
use super::options::draw_options;
use super::push::draw_push;
use super::status_bar::{help_lines, hint_hits, hint_lines, status_line};
use super::widgets::{MUTED, PEACH, RED, TEXT, modal, render_column, style};

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    app.hits.clear();
    app.links.clear();
    start_noting();
    let full = frame.area();
    let area = full.inner(Margin { horizontal: 2, vertical: 1 });
    let fold = if app.page == Page::Work { app.fold } else { Fold::Hidden };
    let [top, _, body, messages_area, bottom] = Layout::vertical([
        Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(messages_height(fold, full.height)), Constraint::Length(1),
    ]).areas(area);
    let (line, tabs) = header(app, top.width);
    frame.render_widget(Paragraph::new(line), top);
    let tab_hits: Vec<(Rect, Hit)> = tabs.into_iter().map(|(x, width, page)| (Rect::new(top.x + x, top.y, width, 1), Hit::Tab(page))).collect();
    let (full_hints, short_hints) = match app.page {
        Page::Work => hint_lines(app.selected_s_opens(), app.lead_chat.is_some()),
        Page::Areas => area_page::Page::hint_lines(),
        Page::Routines => crate::routine_page::Page::hint_lines(),
    };
    let updates: &[(String, String)] = if app.page == Page::Work { &app.updates } else { &[] };
    let status = match app.notice.as_ref().filter(|(_, at)| at.elapsed() < Duration::from_secs(3)) {
        Some((text, _)) => Line::from(Span::styled(text.clone(), style(PEACH))),
        None => status_line(full_hints, short_hints, updates, app.usage.as_ref(), bottom.width as usize, messages::now()),
    };
    if app.page == Page::Areas {
        frame.render_widget(Paragraph::new(status), bottom);
        app.areas.draw(frame, body);
        app.hits = if app.areas.is_normal() { tab_hits } else { Vec::new() };
        return;
    }
    if app.page == Page::Routines {
        frame.render_widget(Paragraph::new(status), bottom);
        app.routines.draw(frame, body);
        app.hits = if app.routines.is_normal() { tab_hits } else { Vec::new() };
        return;
    }
    let mut tab_hits = tab_hits;
    tab_hits.extend(hint_hits(&status, bottom, app.selected_s_opens(), app.lead_chat.is_some()));
    frame.render_widget(Paragraph::new(status), bottom);
    let (message_lines, first_message) = messages_lines(app, fold, messages_area.height as usize, messages_area.width as usize);
    let shown_messages = message_lines.len();
    frame.render_widget(Paragraph::new(message_lines), messages_area);
    match fold {
        Fold::Hidden => {}
        Fold::Line => tab_hits.push((messages_area, Hit::Messages(None))),
        Fold::Open => {
            tab_hits.push((Rect { height: 1, ..messages_area }, Hit::Messages(None)));
            for row in 1..shown_messages.min(messages_area.height as usize) {
                let index = first_message + row - 1;
                if index < app.messages.len() {
                    tab_hits.push((Rect::new(messages_area.x, messages_area.y + row as u16, messages_area.width, 1), Hit::Messages(Some(index))));
                }
            }
        }
    }

    if let Some(error) = &app.error {
        frame.render_widget(Paragraph::new(Span::styled(error.clone(), style(RED))).wrap(Wrap { trim: false }), body);
    } else if !app.loaded {
        frame.render_widget(Paragraph::new(Span::styled("looking around…", style(MUTED))), body);
    } else if full.width >= 150 {
        let [work_area, _, panel_area, _, backlog_area] = Layout::horizontal([
            Constraint::Percentage(33), Constraint::Length(1), Constraint::Percentage(38), Constraint::Length(2), Constraint::Min(20),
        ]).areas(body);
        let (work, work_selected, work_rows) = work_lines(app, work_area.width as usize, false);
        render_column(frame, work_area, work, work_selected, work_rows, &mut app.hits);
        draw_panel(frame, app, panel_area);
        let (backlog, backlog_selected, backlog_rows) = backlog_lines(app, backlog_area.width as usize);
        render_column(frame, backlog_area, backlog, backlog_selected, backlog_rows, &mut app.hits);
    } else if body.width >= 86 {
        let [work_area, _, backlog_area] = Layout::horizontal([Constraint::Percentage(54), Constraint::Length(4), Constraint::Min(20)]).areas(body);
        let (work, work_selected, work_rows) = work_lines(app, work_area.width as usize, true);
        render_column(frame, work_area, work, work_selected, work_rows, &mut app.hits);
        let (backlog, backlog_selected, backlog_rows) = backlog_lines(app, backlog_area.width as usize);
        render_column(frame, backlog_area, backlog, backlog_selected, backlog_rows, &mut app.hits);
    } else {
        let width = body.width as usize;
        let (mut lines, work_selected, mut rows) = work_lines(app, width, true);
        let offset = lines.len() + 1;
        let (backlog, backlog_selected, backlog_rows) = backlog_lines(app, width);
        lines.push(Line::default());
        lines.extend(backlog);
        rows.extend(backlog_rows.into_iter().map(|(start, end, hit)| (start + offset, end + offset, hit)));
        let selected = if app.column == Column::Work { work_selected } else { offset + backlog_selected };
        render_column(frame, body, lines, selected, rows, &mut app.hits);
    }

    let mut modal_hits = Vec::new();
    match &app.mode {
        Mode::Normal => {}
        Mode::Options(index) => modal_hits = draw_options(frame, app, *index),
        Mode::Questions(_) => modal_hits = draw_questions(frame, app),
        Mode::Card { .. } => modal_hits = draw_card(frame, app),
        Mode::Fullscreen { .. } => draw_fullscreen(frame, app),
        Mode::Help => {
            modal(frame, "keys".to_string(), help_lines(app.lead_chat.is_some()), 72);
        }
        Mode::Message(text) => match Notice::parse(text) {
            Some(notice) => draw_notice(frame, &notice, app.shown.as_ref()),
            None => {
                let mut lines: Vec<Line> = text.lines().map(|line| Line::from(Span::styled(line.to_string(), style(TEXT)))).collect();
                lines.push(Line::from(Span::styled("any key closes", style(MUTED))));
                let title = app.shown.as_ref().map_or("kelpie".to_string(), |subject| subject.kind.clone());
                modal(frame, title, lines, 96);
            }
        },
        Mode::ConfirmReset { name, nudged, .. } => draw_reset(frame, name, *nudged),
        Mode::ConfirmDone { name, nudged, .. } => draw_done(frame, name, *nudged),
        Mode::ConfirmQueued { name, about, now, nudged } => draw_queued(frame, name, about, *now, *nudged),
        Mode::ConfirmDrop { name, force, nudged, .. } => draw_drop(frame, name, *force, *nudged),
        Mode::Filter { buffer } => draw_filter(frame, app, buffer),
        Mode::ConfirmPush { .. } => modal_hits = draw_push(frame, app),
        Mode::Working { label, .. } => {
            modal(frame, "kelpie".to_string(), vec![Line::from(Span::styled(label.clone(), style(TEXT)))], 72);
        }
        Mode::EditMessage(editor) => draw_editor(frame, editor),
        Mode::Input { label, buffer, .. } => draw_input(frame, label, buffer),
    }
    if app.toast.as_ref().is_some_and(|toast| toast.at.elapsed() >= TOAST_FOR) {
        app.toast = None;
    }
    if let Some(toast) = app.toast.as_ref().filter(|_| matches!(app.mode, Mode::Normal)) {
        draw_toast(frame, toast);
    }
    if matches!(app.mode, Mode::Normal) {
        app.hits.extend(tab_hits);
    } else {
        app.hits = modal_hits;
    }
    link_urls(frame.buffer_mut(), &take_noted());
    for link in &app.links {
        link_cells(frame.buffer_mut(), link);
    }
}

#[cfg(test)]
mod tests {
    use crate::board::test_support::*;
    use crate::backlog::Entry;
    use crate::messages;
    use crate::board::message_log::Fold;
    use crate::board::{Column, Mode};

    #[test]
    fn at_170_columns_the_selected_task_has_a_detail_panel_between_work_and_backlog_and_its_link_only_there() {
        let mut app = test_app(vec![served("search-filters"), crate::status::tests::task(Some("working"), None, 0)]);
        with_detail(&mut app);
        let (rows, buffer) = render(&mut app, 170, 40);
        let screen = rows.join("\n");
        let urls = find(&rows, "https://wt2.web.test");
        assert_eq!(urls.len(), 1, "{screen}");
        let (work_x, _) = find(&rows, "WORK")[0];
        let (backlog_x, _) = find(&rows, "BACKLOG")[0];
        assert!(work_x < urls[0].0 && urls[0].0 < backlog_x, "the address is in the middle column: {screen}");
        let (x, y) = urls[0];
        assert_eq!(link_of(&buffer[(x as u16, y as u16)]).map(|link| link.0).as_deref(), Some("https://wt2.web.test"), "the panel's address is a link");
        assert!(screen.contains("search-filters · wt2.web · 2 ahead · 0 behind") && screen.contains("a1f3c2e Wire the price filter"), "{screen}");
        assert!(screen.contains("worker  ○ off · b starts it") && screen.contains("changed app/filters.rb"), "{screen}");
        let row = rows.iter().find(|row| row.contains("▌○ search-filters")).unwrap_or_else(|| panic!("{screen}"));
        assert!(row[..row.find('│').unwrap()].trim_end().ends_with('●'), "a running server is a dot on its row: {row}");
        assert!(!screen.contains(" web https://"), "no web line under the row: {screen}");
    }

    #[test]
    fn below_86_columns_backlog_sits_under_work_and_the_details_unfold_under_the_selected_row() {
        let mut app = test_app(vec![served("search-filters"), crate::status::tests::task(Some("working"), None, 0)]);
        with_detail(&mut app);
        let (rows, _) = render(&mut app, 84, 50);
        let screen = rows.join("\n");
        let (_, work_y) = find(&rows, "WORK")[0];
        let (_, backlog_y) = find(&rows, "BACKLOG")[0];
        let (_, row_y) = find(&rows, "▌○ search-filters")[0];
        let (_, url_y) = find(&rows, "https://wt2.web.test")[0];
        assert!(work_y < row_y && row_y < url_y && url_y < backlog_y, "{screen}");
        assert!(!screen.contains("a1f3c2e"), "commits stay in the wide panel: {screen}");
        assert!(rows[row_y].trim_end().ends_with('●'), "{}", rows[row_y]);
    }

    #[test]
    fn at_112_columns_work_and_backlog_sit_side_by_side_with_the_details_unfolded_in_work() {
        let mut app = test_app(vec![served("search-filters"), crate::status::tests::task(Some("working"), None, 0)]);
        with_detail(&mut app);
        let (rows, buffer) = render(&mut app, 112, 40);
        let screen = rows.join("\n");
        let (work_x, work_y) = find(&rows, "WORK")[0];
        let (backlog_x, backlog_y) = find(&rows, "BACKLOG")[0];
        assert!(work_y == backlog_y && work_x < backlog_x, "{screen}");
        let (_, row_y) = find(&rows, "▌○ search-filters")[0];
        let urls = find(&rows, "https://wt2.web.test");
        assert_eq!(urls.len(), 1, "{screen}");
        let (url_x, url_y) = urls[0];
        assert!(url_y > row_y && url_x < backlog_x, "the details unfold under the row, in Work: {screen}");
        assert_eq!(link_of(&buffer[(url_x as u16, url_y as u16)]).map(|link| link.0).as_deref(), Some("https://wt2.web.test"));
        assert!(!screen.contains("a1f3c2e"), "commits stay in the wide panel: {screen}");
    }

    #[test]
    fn untrusted_text_on_the_board_never_carries_a_terminal_escape_into_a_cell() {
        let mut item = crate::status::tests::task(Some("idle"), Some("needs-you"), 1);
        item.name = format!("task {ESCAPES}");
        item.place = format!("repo{ESCAPES}");
        item.report.as_mut().unwrap().text = format!("report {ESCAPES}");
        item.report.as_mut().unwrap().report.as_mut().unwrap().next = format!("next {ESCAPES}");
        let mut question = messages::fold(QUESTION).remove(0);
        question.text = format!("question {ESCAPES}");
        question.question.as_mut().unwrap().options[0].0 = format!("label {ESCAPES}");
        item.question = Some(question.clone());
        let mut app = test_app(vec![item]);
        app.fold = Fold::Open;
        let mut message = messages::fold(QUESTION).remove(0);
        message.text = format!("message {ESCAPES}");
        message.from = format!("from{ESCAPES}");
        app.messages = vec![message];
        app.questions = vec![question];
        let (rows, buffer) = render(&mut app, 170, 40);
        no_escape_reaches_the_terminal(&buffer, "the work and messages panels");
        assert!(rows.join("\n").contains("task ]8;;https://evil.example\\click]8;"), "the text shows, stripped of its control characters: {}", rows.join("\n"));
        app.open_questions(Some(0));
        let (_, buffer) = render(&mut app, 170, 40);
        no_escape_reaches_the_terminal(&buffer, "the question window");
        app.mode = Mode::Message(format!("result {ESCAPES}"));
        let (_, buffer) = render(&mut app, 100, 30);
        no_escape_reaches_the_terminal(&buffer, "a result window");
        app.mode = Mode::Normal;
        app.column = Column::Backlog;
        app.backlog.entries.push(Entry { source: "local".to_string(), title: format!("entry {ESCAPES}"), priority: "high".to_string(), area: "app".to_string(), reference: "local:x".to_string(), summary: format!("about {ESCAPES}"), age: "3 d".to_string() });
        let (_, buffer) = render(&mut app, 170, 40);
        no_escape_reaches_the_terminal(&buffer, "the backlog column");
    }

    #[test]
    fn bidi_overrides_and_zero_width_characters_in_untrusted_text_never_reach_a_cell() {
        let mut item = crate::status::tests::task(Some("idle"), None, 0);
        item.name = "push\u{202e}tpircs\u{200b}\u{feff}\u{2066}x".to_string();
        let mut app = test_app(vec![item]);
        let (rows, buffer) = render(&mut app, 120, 30);
        for hidden in ['\u{202e}', '\u{200b}', '\u{feff}', '\u{2066}'] {
            assert!(!buffer.content.iter().any(|cell| cell.symbol().contains(hidden)), "{hidden:?} was drawn");
        }
        assert!(rows.join("\n").contains("pushtpircsx"), "{}", rows.join("\n"));
    }
}
