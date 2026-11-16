use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keys::{self, Act, Ctx};
use crate::status::Item;
use crate::tasks::SubtaskState;
use super::card::{card_parts, indented, rich_wrap};
use super::columns::group_color;
use super::detail::Detail;
use super::review_data as data;
use super::steps::{rows_shown, walked};
use super::subtasks::{address_of, read_text};
use super::widgets::{LAVENDER, MAUVE, MUTED, SURFACE, TEAL, TEXT, YELLOW, ago, bold, cut, fit, plural, style, text_width, wrap};
use super::{App, Hit};

// The card beside the tasks: whether it is drawn, whether it has the keys, and how far it is scrolled.
#[derive(Default)]
pub(super) struct Beside {
    pub(super) focused: bool,
    pub(super) shown: bool,
    pub(super) scroll: u16,
    subject: String,
}

// The part of a card the version before it differs in; the card shows what it says now.
const CHANGED: &str = "Changed";

pub(super) fn section(title: &str) -> Line<'static> {
    Line::from(Span::styled(title.to_string(), bold(LAVENDER)))
}

fn heading(title: Vec<Span<'static>>, focused: bool, width: usize) -> Vec<Line<'static>> {
    let mut spans = vec![Span::styled("CARD", bold(if focused { MAUVE } else { MUTED })), Span::raw("   ")];
    spans.extend(title);
    vec![fit(spans, width), Line::from(Span::styled("─".repeat(width), style(SURFACE)))]
}

fn accepted(item: &Item) -> usize {
    let steps = &item.subtasks;
    steps.pushed.len() + steps.items.iter().filter(|step| matches!(step.state, SubtaskState::Accepted | SubtaskState::Pushed)).count()
}

fn body(text: &str, width: usize) -> Vec<Line<'static>> {
    rich_wrap(text, width.saturating_sub(2), usize::MAX, TEXT).into_iter().map(|line| indented(line, 2)).collect()
}

fn facts(item: &Item, detail: Option<&Detail>, width: usize) -> Vec<Line<'static>> {
    let group = item.group();
    let mut chunks = vec![(group.title().to_lowercase(), group_color(group)), (item.place.clone(), MUTED)];
    let commits = detail.map(|detail| detail.commits.len()).or_else(|| item.git.as_ref().map(|git| git.ahead)).unwrap_or(0);
    if commits > 0 {
        chunks.push((plural(commits, "commit"), MUTED));
    }
    if let Some(read) = item.review {
        chunks.push((read_text(read), MUTED));
    }
    if rows_shown(item) {
        chunks.push((format!("{} of {} subtasks accepted", accepted(item), item.subtasks.pushed.len() + item.subtasks.items.len()), MUTED));
    }
    match detail.map(|detail| detail.card.as_ref()) {
        None if item.path.is_some() => chunks.push(("reading the card…".to_string(), MUTED)),
        Some(None) => chunks.push(("no card yet".to_string(), MUTED)),
        _ => {}
    }
    if detail.and_then(|detail| detail.card.as_ref()).is_some_and(|(_, current)| !current) {
        chunks.push(("code changed since the card".to_string(), YELLOW));
    }
    let mut lines: Vec<Line<'static>> = vec![Line::default()];
    for (at, (text, color)) in chunks.into_iter().enumerate() {
        let line = lines.last_mut().expect("there is always a line");
        let joint = if at == 0 { "" } else { " · " };
        if at > 0 && line.width() + joint.len() + text_width(&text) > width {
            lines.push(Line::from(Span::styled(text, style(color))));
            continue;
        }
        line.spans.extend([Span::styled(joint, style(MUTED)), Span::styled(text, style(color))]);
    }
    lines
}

// What the task has said before its card: the recipe it runs and its last report.
fn without_card(item: &Item, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(recipe) = &item.recipe {
        lines.extend([Line::default(), section("RECIPE")]);
        lines.extend(body(&format!("{}, because {}", crate::recipe::shown_name(&recipe.name), recipe.why), width));
    }
    if let Some(report) = &item.report {
        let mut head = section("LAST REPORT");
        head.spans.push(Span::styled(format!("  {}", ago(report.at)), style(MUTED)));
        lines.extend([Line::default(), head]);
        lines.extend(body(&report.text, width));
    }
    if lines.is_empty() {
        lines.extend([Line::default(), Line::from(Span::styled("Nothing reported yet.", style(MUTED)))]);
    }
    lines
}

// The task's card as both the column beside the tasks and the card window draw it: its state line, its card's parts, its commits.
pub(super) fn task_card(item: Option<&Item>, detail: Option<&Detail>, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = item.map(|item| facts(item, detail, width)).unwrap_or_default();
    match detail.and_then(|detail| detail.card.as_ref()) {
        Some((card, _)) => {
            for part in card_parts(&card.text).0.into_iter().filter(|part| part.label.as_deref() != Some(CHANGED)) {
                lines.push(Line::default());
                if let Some(label) = &part.label {
                    lines.push(section(&label.to_uppercase()));
                }
                for text in &part.lines {
                    if text.is_empty() { lines.push(Line::default()) } else { lines.extend(body(text, width)) }
                }
            }
        }
        None => lines.extend(item.map(|item| without_card(item, width)).unwrap_or_default()),
    }
    let commits = detail.map(|detail| detail.commits.as_slice()).unwrap_or_default();
    if !commits.is_empty() {
        lines.extend([Line::default(), section("COMMITS")]);
        lines.extend(commits.iter().map(|commit| Line::from(vec![
            Span::raw("  "), Span::styled(data::short(&commit.sha).to_string(), style(TEAL)), Span::raw("  "),
            Span::styled(cut(commit.message.lines().next().unwrap_or_default(), width.saturating_sub(11)), style(TEXT)),
        ])));
    }
    lines
}

impl App {
    // → on a task: onto its subtask rows when it has them, else its card beside the tasks to scroll it.
    pub(super) fn enter_task(&mut self) {
        let Some(item) = self.selected_task() else { return self.focus_beside() };
        if !walked(&item).is_empty() {
            return self.open_subtasks(&item);
        }
        self.focus_beside();
    }

    // ← → from a row under the tasks.
    pub(super) fn focus_beside(&mut self) {
        if !self.beside.shown {
            return;
        }
        (self.offer_panel.focused, self.offer_panel.on_fixed, self.comment_panel.focused, self.column, self.beside.focused) = (false, false, false, super::Column::Work, true);
    }

    // The keys the focused card takes: it scrolls, and ← → or esc hand the keys back to the tasks.
    pub(super) fn beside_key(&mut self, key: &KeyEvent) -> bool {
        if !self.beside.focused || !self.beside.shown || self.column != super::Column::Work {
            return false;
        }
        let scroll = &mut self.beside.scroll;
        match keys::map().act(Ctx::Work, key) {
            Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => *scroll = scroll.saturating_sub(1),
            Some(Act::Move) => *scroll = scroll.saturating_add(1),
            Some(Act::Switch | Act::Back) => self.beside.focused = false,
            _ => match key.code {
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown | KeyCode::Char(' ') => *scroll = scroll.saturating_add(10),
                _ => return false,
            },
        }
        true
    }

    // The card for the task the cursor is on, its title line apart so it stays put while the rest scrolls.
    pub(super) fn beside_lines(&mut self, width: usize) -> (Vec<Line<'static>>, Vec<Line<'static>>, String) {
        let focused = self.beside.focused && !self.in_messages;
        let off_tasks = self.offer_panel.focused || self.comment_panel.focused || self.on_parked_line();
        let Some(item) = self.selected_task().filter(|_| !off_tasks) else {
            let hint = "Not a task: move to one to see its card here.".to_string();
            return (heading(Vec::new(), focused, width), wrap(&hint, width, 3).into_iter().map(|text| Line::from(Span::styled(text, style(MUTED)))).collect(), "none".to_string());
        };
        let detail = self.detail(&item);
        let title = vec![Span::styled(item.titled(), bold(TEXT))];
        (heading(title, focused, width), task_card(Some(&item), detail.as_deref(), width), address_of(&item))
    }

    pub(super) fn draw_beside(&mut self, frame: &mut Frame, area: Rect) {
        let (head, lines, subject) = self.beside_lines(area.width as usize);
        let room = area.height.saturating_sub(head.len() as u16);
        if subject != self.beside.subject {
            (self.beside.subject, self.beside.scroll) = (subject, 0);
        }
        self.beside.scroll = self.beside.scroll.min(lines.len().saturating_sub(room as usize) as u16);
        let top = Rect { height: (head.len() as u16).min(area.height), ..area };
        let rest = Rect { y: area.y + top.height, height: room, ..area };
        frame.render_widget(Paragraph::new(head), top);
        frame.render_widget(Paragraph::new(lines).scroll((self.beside.scroll, 0)), rest);
        self.beside.shown = true;
        self.hits.push((area, Hit::Beside));
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use ratatui::crossterm::event::KeyCode;

    use crate::board::bug_report::Offer;
    use crate::board::steps::tests::{holds, stepped, stripe};
    use crate::board::test_support::*;
    use crate::board::widgets::{FAINT, LAVENDER, MAUVE, MUTED};
    use super::task_card;
    use crate::board::{App, Column, Page};
    use crate::report;
    use crate::report_wire::BugReport;

    const CARD: &str = "Built: Filters show on the search page and stay in the URL.\n\nDecisions: Price and size only, the rest waits.\n\nAlso noticed: The search page loads its fonts twice.";

    fn with_card(app: &mut App) {
        let card = report::Card { sha: "abcdef0123".to_string(), text: CARD.to_string(), file: PathBuf::from("/state/cards/x.md"), data: None };
        Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card = Some((card, true));
    }

    fn carded_app() -> App {
        let mut app = test_app(vec![served("search-filters")]);
        with_detail(&mut app);
        with_card(&mut app);
        app
    }

    // The rows right of the Work column at 140 wide, where the card stands.
    fn card_side(app: &mut App) -> (Vec<String>, ratatui::buffer::Buffer) {
        let (rows, buffer) = render(app, 140, 40);
        (rows.iter().map(|row| row.chars().skip(78).collect::<String>().trim_end().to_string()).collect(), buffer)
    }

    #[test]
    fn a_task_with_a_card_shows_its_facts_its_card_sections_and_its_commits_beside_the_tasks() {
        let mut app = carded_app();
        let (side, buffer) = card_side(&mut app);
        let screen = side.join("\n");
        assert!(side.iter().any(|row| row.trim() == "CARD   search-filters"), "{screen}");
        assert!(screen.contains("ready to push · web · 1 commit") && !screen.contains("no card yet"), "{screen}");
        for (label, text) in [("BUILT", "Filters show on the search page and stay in the URL."), ("DECISIONS", "Price and size only, the rest waits."), ("ALSO NOTICED", "The search page loads its fonts twice."), ("COMMITS", "a1f3c2e  Wire the price filter")] {
            let at = side.iter().position(|row| row.trim() == label).unwrap_or_else(|| panic!("{label}: {screen}"));
            assert_eq!(side[at + 1].trim(), text, "{screen}");
        }
        let (rows, _) = render(&mut app, 140, 40);
        let (x, y) = find(&rows, "BUILT")[0];
        assert_eq!(buffer[(x as u16, y as u16)].fg, LAVENDER, "a section is named in lavender");
        let (x, y) = find(&rows, "CARD   search-filters")[0];
        assert_eq!(buffer[(x as u16, y as u16)].fg, MUTED, "the column is not focused");
        assert!(!rows.join("\n").contains("BACKLOG") && !rows[1].contains("backlog:"), "the backlog has its own page: {}", rows.join("\n"));
    }

    #[test]
    fn a_task_without_a_card_shows_its_recipe_and_last_report_with_its_age() {
        let mut item = served("search-filters");
        item.recipe = Some(crate::recipe::Recipe { name: "small-fix".to_string(), why: "a cancelled run never ends the watch".to_string(), by: "the lead".to_string() });
        let at = crate::messages::now() - 240;
        item.report.as_mut().unwrap().at = at;
        item.report.as_mut().unwrap().text = "Cause proven: Cancelled kept the watch Running.".to_string();
        let mut app = test_app(vec![item]);
        with_detail(&mut app);
        let (side, _) = card_side(&mut app);
        let screen = side.join("\n");
        assert!(screen.contains("ready to push · web · 1 commit · no card yet"), "{screen}");
        let recipe = side.iter().position(|row| row.trim() == "RECIPE").unwrap_or_else(|| panic!("{screen}"));
        assert_eq!(side[recipe + 1].trim(), "small fix, because a cancelled run never ends the watch");
        let report = side.iter().position(|row| row.trim() == "LAST REPORT  4m ago").unwrap_or_else(|| panic!("{screen}"));
        assert_eq!(side[report + 1].trim(), "Cause proven: Cancelled kept the watch Running.");
        assert!(!screen.contains("BUILT"), "{screen}");
    }

    #[test]
    fn inside_a_task_the_column_stays_its_card_and_on_the_issues_row_a_hint() {
        let mut steps = stripe();
        holds(&mut steps.items[1], &["3333333cccc"], false);
        let mut item = stepped(steps);
        item.path = Some(PathBuf::from("/work/wt1.billing"));
        item.review = Some((1, 3));
        let mut app = test_app(vec![item]);
        let (before, _) = card_side(&mut app);
        assert!(before.join("\n").contains("working · billing · 2 commits · 1/3 read") && before.iter().any(|row| row.trim().starts_with("1 of 4 subtasks accepted")), "the state line carries the read count and wraps whole parts: {}", before.join("\n"));
        press(&mut app, key(KeyCode::Right));
        press(&mut app, key(KeyCode::Down));
        let (side, _) = card_side(&mut app);
        assert_eq!(side[..10], before[..10], "stepping the subtasks leaves the card as it is");
        press(&mut app, key(KeyCode::Esc));
        app.offers = vec![Offer { id: "a1b2c3".to_string(), report: BugReport { kind: None, title: "push hid git's error".to_string(), text: String::new(), version: "0.26.0".to_string(), os: "macos".to_string() } }];
        app.focus_offers(0);
        let (side, _) = card_side(&mut app);
        let screen = side.join("\n");
        assert!(screen.contains("Not a task: move to one to see its card here.") && !screen.contains("Stripe"), "no stale card on the issues row: {screen}");
    }

    #[test]
    fn the_column_and_the_card_window_draw_the_same_card_without_its_changed_part() {
        let mut app = carded_app();
        let card = &mut Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card.as_mut().unwrap().0;
        card.text = format!("**Filters stay in the URL**\n\nChanged: version 1 → 2\n- − Built: old line\n\n{CARD}");
        let width = 60;
        let item = app.tasks[0].clone();
        let detail = app.details.get(Path::new("/work/wt2.web")).cloned();
        let lines: Vec<String> = task_card(Some(&item), detail.as_deref(), width).iter().map(|line| line.to_string()).collect();
        assert!(lines[0].starts_with("ready to push · web · 1 commit") && lines.iter().any(|line| line == "BUILT") && !lines.iter().any(|line| line.contains("CHANGED") || line.contains("old line")), "{lines:#?}");
        app.handle_normal(key(KeyCode::Char('s')));
        let (rows, _) = render(&mut app, 100, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("▸ search-filters") && screen.contains("ready to push · web · 1 commit") && screen.contains("BUILT") && screen.contains("a1f3c2e  Wire the price filter") && !screen.contains("CHANGED"), "{screen}");
    }

    #[test]
    fn the_card_follows_j_and_k_and_scrolls_only_once_focused() {
        let mut other = served("checkout-copy");
        other.path = Some(PathBuf::from("/work/wt3.web"));
        let mut app = test_app(vec![served("search-filters"), other]);
        with_detail(&mut app);
        with_card(&mut app);
        let heading = |app: &mut App| card_side(app).0.into_iter().find(|row| row.trim_start().starts_with("CARD")).unwrap_or_default();
        assert!(heading(&mut app).ends_with("search-filters"));
        press(&mut app, key(KeyCode::Char('j')));
        assert!(heading(&mut app).ends_with("checkout-copy") && app.work_selected == 1, "j moves the card along");
        let screen = card_side(&mut app).0.join("\n");
        assert!(screen.contains("reading the card…") && !screen.contains("BUILT"), "a card not read yet is not the last one's: {screen}");
        press(&mut app, key(KeyCode::Char('k')));
        assert!(heading(&mut app).ends_with("search-filters"));
        press(&mut app, key(KeyCode::Right));
        let (rows, buffer) = render(&mut app, 140, 12);
        let (x, y) = find(&rows, "CARD   search-filters")[0];
        assert_eq!(buffer[(x as u16, y as u16)].fg, MAUVE, "focused, the column's name lights");
        let (x, y) = find(&rows, "checkout-copy")[0];
        assert_ne!(buffer[(x as u16, y as u16)].fg, FAINT, "nothing fades");
        press(&mut app, key(KeyCode::Char('j')));
        render(&mut app, 140, 12);
        assert!(app.beside.scroll == 1 && app.work_selected == 0, "focused, j scrolls the card and leaves the cursor");
        press(&mut app, key(KeyCode::Left));
        press(&mut app, key(KeyCode::Char('j')));
        assert!(app.work_selected == 1 && app.column == Column::Work, "← gives the keys back to the tasks");
        render(&mut app, 140, 12);
        assert_eq!(app.beside.scroll, 0, "another task's card starts at its top");
        assert_eq!(app.page, Page::Work);
    }
}
