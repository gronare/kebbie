use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keys::{self, Act, Ctx};
use crate::status::Item;
use crate::tasks::{Subtask, SubtaskState};
use super::card::{card_parts, indented, rich_wrap};
use super::columns::group_color;
use super::detail::Detail;
use super::review_data as data;
use super::steps::{rows_shown, walked};
use super::subtask_view::{self, Loaded, address_of, section};
use super::widgets::{MAUVE, MUTED, SURFACE, TEAL, TEXT, YELLOW, ago, bold, cut, fit, plural, style, wrap};
use super::{App, Hit};

type Key = (String, String);
type Request = (PathBuf, String, Subtask);
type Answer = (Key, Option<String>, Loaded);

// The card beside the tasks: what it shows, how far it is scrolled, and the subtasks read off the UI thread.
#[derive(Default)]
pub(super) struct Beside {
    pub(super) focused: bool,
    pub(super) shown: bool,
    pub(super) scroll: u16,
    subject: String,
    loads: HashMap<Key, (Option<String>, Loaded)>,
    asked: HashMap<Key, (Option<String>, Instant)>,
    requests: Option<Sender<Request>>,
    results: Option<Receiver<Answer>>,
}

impl Beside {
    pub(super) fn spawn_loader(&mut self) {
        let (request_tx, request_rx) = mpsc::channel::<Request>();
        let (answer_tx, answer_rx) = mpsc::channel::<Answer>();
        thread::spawn(move || {
            while let Ok((path, address, subtask)) = request_rx.recv() {
                let loaded = subtask_view::load(&path, &address, &subtask);
                if answer_tx.send(((address, subtask.id.clone()), subtask.head().map(str::to_string), loaded)).is_err() {
                    return;
                }
            }
        });
        (self.requests, self.results) = (Some(request_tx), Some(answer_rx));
    }

    pub(super) fn take_loads(&mut self) -> bool {
        let Some(results) = &self.results else { return false };
        let mut changed = false;
        while let Ok((key, head, loaded)) = results.try_recv() {
            self.loads.insert(key, (head, loaded));
            changed = true;
        }
        changed
    }
}

fn heading(title: Vec<Span<'static>>, focused: bool, width: usize) -> Vec<Line<'static>> {
    let mut spans = vec![Span::styled("CARD", bold(if focused { MAUVE } else { MUTED })), Span::raw("   ")];
    spans.extend(title);
    vec![fit(spans, width), Line::from(Span::styled("─".repeat(width), style(SURFACE)))]
}

fn body(text: &str, width: usize) -> Vec<Line<'static>> {
    rich_wrap(text, width.saturating_sub(2), usize::MAX, TEXT).into_iter().map(|line| indented(line, 2)).collect()
}

fn facts(item: &Item, detail: Option<&Detail>) -> Line<'static> {
    let group = item.group();
    let mut spans = vec![Span::styled(group.title().to_lowercase(), style(group_color(group)))];
    let mut parts = vec![item.place.clone()];
    let commits = detail.map(|detail| detail.commits.len()).or_else(|| item.git.as_ref().map(|git| git.ahead)).unwrap_or(0);
    if commits > 0 {
        parts.push(plural(commits, "commit"));
    }
    if rows_shown(item) {
        let steps = &item.subtasks;
        let accepted = steps.pushed.len() + steps.items.iter().filter(|step| matches!(step.state, SubtaskState::Accepted | SubtaskState::Pushed)).count();
        parts.push(format!("{accepted} of {} subtasks accepted", steps.pushed.len() + steps.items.len()));
    }
    match detail.map(|detail| detail.card.as_ref()) {
        None if item.path.is_some() => parts.push("reading the card…".to_string()),
        Some(None) => parts.push("no card yet".to_string()),
        _ => {}
    }
    spans.push(Span::styled(format!(" · {}", parts.join(" · ")), style(MUTED)));
    if detail.and_then(|detail| detail.card.as_ref()).is_some_and(|(_, current)| !current) {
        spans.push(Span::styled(" · code changed since the card", style(YELLOW)));
    }
    Line::from(spans)
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

fn task_lines(item: &Item, detail: Option<&Detail>, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![facts(item, detail)];
    match detail.and_then(|detail| detail.card.as_ref()) {
        Some((card, _)) => {
            for part in card_parts(&card.text).0 {
                lines.push(Line::default());
                if let Some(label) = &part.label {
                    lines.push(section(&label.to_uppercase()));
                }
                for text in &part.lines {
                    if text.is_empty() { lines.push(Line::default()) } else { lines.extend(body(text, width)) }
                }
            }
        }
        None => lines.extend(without_card(item, width)),
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
    // ← → from a row under the tasks or from the tasks themselves.
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

    // The subtask as its view reads it; the board's own loader reads it on a thread and the last reading shows meanwhile.
    fn subtask_loaded(&mut self, item: &Item, step: &Subtask) -> Option<Loaded> {
        let path = item.path.clone()?;
        let address = address_of(item);
        let Some(requests) = &self.beside.requests else { return Some((self.subtask_io.load)(&path, &address, step)) };
        let key = (address.clone(), step.id.clone());
        let head = step.head().map(str::to_string);
        let fresh = self.beside.asked.get(&key).is_some_and(|(asked, at)| *asked == head && at.elapsed() < Duration::from_secs(4));
        if !fresh {
            let _ = requests.send((path, address, step.clone()));
            self.beside.asked.insert(key.clone(), (head, Instant::now()));
        }
        self.beside.loads.get(&key).map(|(_, loaded)| loaded.clone())
    }

    // The card for what the cursor is on, its title line apart so it stays put while the rest scrolls.
    pub(super) fn beside_lines(&mut self, width: usize) -> (Vec<Line<'static>>, Vec<Line<'static>>, String) {
        let focused = self.beside.focused && !self.in_messages;
        let off_tasks = self.offer_panel.focused || self.comment_panel.focused || self.on_parked_line();
        let Some(item) = self.selected_task().filter(|_| !off_tasks) else {
            let hint = "Not a task: move to one to see its card here.".to_string();
            return (heading(Vec::new(), focused, width), wrap(&hint, width, 3).into_iter().map(|text| Line::from(Span::styled(text, style(MUTED)))).collect(), "none".to_string());
        };
        if let Some(at) = self.subtask_at(&item) {
            let step = walked(&item)[at].clone();
            let loaded = self.subtask_loaded(&item, &step);
            let mut lines = subtask_view::view_lines(&item, &step, &loaded.clone().unwrap_or_default(), width);
            let mut title: Vec<Span<'static>> = lines.remove(0).spans.into_iter().rev().collect();
            title.truncate(1);
            title.push(Span::styled(format!(" · {}", item.titled()), style(MUTED)));
            if loaded.is_none() {
                lines.push(Line::from(Span::styled("reading its files…", style(MUTED))));
            }
            return (heading(title, focused, width), lines, format!("{}:{}", address_of(&item), step.id));
        }
        let detail = self.detail(&item);
        let title = vec![Span::styled(item.titled(), bold(TEXT))];
        (heading(title, focused, width), task_lines(&item, detail.as_deref(), width), address_of(&item))
    }

    pub(super) fn draw_beside(&mut self, frame: &mut Frame, area: Rect) {
        let (head, lines, subject) = self.beside_lines(area.width as usize);
        if subject != self.beside.subject {
            (self.beside.subject, self.beside.scroll) = (subject, 0);
        }
        let room = area.height.saturating_sub(head.len() as u16);
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
    use crate::board::subtask_view::{Io, Loaded};
    use crate::board::test_support::*;
    use crate::board::widgets::{LAVENDER, MAUVE, MUTED};
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
    fn in_the_subtask_list_the_card_is_that_subtask_and_on_the_issues_row_a_hint() {
        let mut steps = stripe();
        holds(&mut steps.items[1], &["3333333cccc"], false);
        let mut item = stepped(steps);
        item.path = Some(PathBuf::from("/work/wt1.billing"));
        let mut app = test_app(vec![item]);
        app.subtask_io = Io { load: Rc::new(|_, _, step| Loaded { files: Vec::new(), built: vec![format!("{} keeps the charge in one place.", step.title)], obstacle: None }), ..Io::default() };
        let (side, _) = card_side(&mut app);
        assert!(side.join("\n").contains("working · billing · 2 commits · 1 of 4 subtasks accepted"), "{}", side.join("\n"));
        press(&mut app, key(KeyCode::Char('S')));
        press(&mut app, key(KeyCode::Down));
        let (side, _) = card_side(&mut app);
        let screen = side.join("\n");
        assert!(screen.contains("CARD   Subtask 3 of 4 · Switch the payment provider") && screen.contains("Add Stripe as the gateway"), "{screen}");
        assert!(screen.contains("Add Stripe as the gateway keeps the charge in one place.") && screen.contains("3333333  subject of 3333333cccc"), "{screen}");
        press(&mut app, key(KeyCode::Esc));
        app.offers = vec![Offer { id: "a1b2c3".to_string(), report: BugReport { kind: None, title: "push hid git's error".to_string(), text: String::new(), version: "0.26.0".to_string(), os: "macos".to_string() } }];
        app.focus_offers(0);
        let (side, _) = card_side(&mut app);
        let screen = side.join("\n");
        assert!(screen.contains("Not a task: move to one to see its card here.") && !screen.contains("Stripe"), "no stale card on the issues row: {screen}");
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
