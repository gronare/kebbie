use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::backlog::Entry;
use crate::keys::{self, Act};
use crate::status::Group;
use super::columns::Rows;
use super::widgets::{SURFACE, YELLOW, plural, row, style};
use super::{App, Column, Hit, Mode, Page};

// One backlog item the user asked to start whose questions hold it, as a row under the Work page's Needs you.
pub(super) fn ask_line(entry: &Entry, selected: bool, focused: bool, place: bool, width: usize) -> Line<'static> {
    let questions = plural(entry.shape.waiting, "question");
    let facts = format!("{questions} · answer on its card");
    let place = (place && !entry.area.is_empty()).then_some(entry.area.as_str());
    let mut line = row(selected, focused, Span::styled("?", style(YELLOW)), &entry.title, place, &facts, width);
    if let Some(last) = line.spans.pop() {
        match last.content.strip_prefix(questions.as_str()) {
            Some(after) => {
                let after = after.to_string();
                line.spans.extend([Span::styled(questions, last.style.fg(YELLOW)), Span::styled(after, last.style)]);
            }
            None => line.spans.push(last),
        }
    }
    if selected && focused { line.style(Style::new().bg(SURFACE)) } else { line }
}

impl App {
    // The backlog items Needs you lists on the Work page: those the user asked to start whose blocking questions hold them, kept by a Work filter on their title or repo.
    pub(super) fn asks_shown(&self) -> Vec<usize> {
        let wanted = self.filter_of(Column::Work).map(str::to_lowercase);
        self.work_asks().into_iter().filter(|index| {
            let entry = &self.backlog.entries[*index];
            wanted.as_deref().is_none_or(|wanted| entry.title.to_lowercase().contains(wanted) || entry.area.to_lowercase().contains(wanted))
        }).collect()
    }

    // The rows sit at the end of NEEDS YOU, before the first task of a group the board orders after it.
    pub(super) fn ask_boundary(&self) -> usize {
        let rank = |group: Group| self.groups.iter().position(|shown| *shown == group).unwrap_or(usize::MAX);
        let needs = rank(Group::NeedsYou);
        self.tasks.iter().position(|item| item.parked.is_some() || rank(item.group()) > needs).unwrap_or(self.tasks.len())
    }

    // The focused row, while there is one to focus; a stale focus is dropped.
    pub(super) fn asks_focused(&mut self) -> Option<usize> {
        let shown = self.asks_shown().len();
        match self.ask_focus {
            Some(_) if shown == 0 => {
                self.ask_focus = None;
                None
            }
            Some(at) => {
                let at = at.min(shown - 1);
                self.ask_focus = Some(at);
                Some(at)
            }
            None => None,
        }
    }

    pub(super) fn focus_asks(&mut self, index: usize) {
        let shown = self.asks_shown().len();
        if shown == 0 {
            return;
        }
        (self.in_messages, self.offer_panel.focused, self.offer_panel.on_fixed, self.comment_panel.focused, self.beside.focused) = (false, false, false, false, false);
        self.column = Column::Work;
        self.ask_focus = Some(index.min(shown - 1));
    }

    fn task_between(&self, from: usize, to: usize) -> bool {
        (from.min(to)..from.max(to)).any(|index| index < self.tasks.len() && self.shown(Column::Work, index))
    }

    // ↑ ↓ from a task into the rows: from the task above them into the first, from the task below into the last; comments drawn nearer in that direction come first.
    pub(super) fn enter_asks(&mut self, down: bool) -> bool {
        let shown = self.asks_shown().len();
        if shown == 0 {
            return false;
        }
        let (boundary, comments) = (self.ask_boundary(), self.comment_boundary());
        if self.comments_shown() && if down { comments < boundary } else { comments >= boundary } && self.enter_comments(down) {
            return true;
        }
        let here = self.work_selected;
        let has_task = self.selected_task().is_some();
        let next = (if down { here + 1..self.tasks.len() } else { 0..here }).filter(|index| self.shown(Column::Work, *index));
        let next = if down { next.min() } else { next.max() };
        let crosses = match (has_task, down) {
            (false, _) => true,
            (true, true) => here < boundary && next.is_none_or(|next| next >= boundary),
            (true, false) => here >= boundary && next.is_none_or(|next| next < boundary),
        };
        if crosses {
            self.focus_asks(if down { 0 } else { shown - 1 });
        }
        crosses
    }

    fn leave_asks(&mut self, down: bool) {
        let (boundary, comments) = (self.ask_boundary(), self.comment_boundary());
        let to_comments = self.comments_shown() && if down { comments >= boundary } else { comments < boundary } && !self.task_between(boundary, comments);
        if to_comments {
            return self.focus_comments(if down { 0 } else { self.comments.len().saturating_sub(1) });
        }
        let task = if down { (boundary..self.tasks.len()).find(|index| self.shown(Column::Work, *index)) } else { (0..boundary).rev().find(|index| self.shown(Column::Work, *index)) };
        match task {
            Some(index) => {
                self.ask_focus = None;
                self.work_selected = index;
            }
            None if down && !self.offers.is_empty() => self.focus_offers(0),
            None if down => self.focus_fixed(),
            None => {}
        }
    }

    // From the comments section onto the rows when no task stands between them.
    pub(super) fn asks_after_comments(&mut self, down: bool) -> bool {
        let shown = self.asks_shown().len();
        let (boundary, comments) = (self.ask_boundary(), self.comment_boundary());
        let next = shown > 0 && if down { boundary > comments } else { boundary <= comments } && !self.task_between(boundary, comments);
        if next {
            self.focus_asks(if down { 0 } else { shown - 1 });
        }
        next
    }

    pub(super) fn handle_asks(&mut self, key: KeyEvent) {
        let Some(at) = self.asks_focused() else { return };
        let last = self.asks_shown().len() - 1;
        match keys::map().act(self.ctx(), &key) {
            Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => match at.checked_sub(1) {
                Some(above) => self.ask_focus = Some(above),
                None => self.leave_asks(false),
            },
            Some(Act::Move) if at < last => self.ask_focus = Some(at + 1),
            Some(Act::Move) => self.leave_asks(true),
            Some(Act::Options) => self.open_ask(),
            Some(Act::Go) => {
                if let Some(entry) = self.asks_shown().get(at).and_then(|index| self.backlog.entries.get(*index)).cloned() {
                    self.start_entry(entry);
                }
            }
            Some(Act::Back | Act::Close) => self.ask_focus = None,
            Some(Act::Messages) => {
                self.ask_focus = None;
                self.press_m();
            }
            Some(Act::Help) => self.mode = Mode::Help(Default::default()),
            Some(Act::Inbox) => self.open_questions(None),
            _ => {}
        }
    }

    // ⏎ on a row opens the item's card, where its questions are answered; an item without a card opens on the Backlog page.
    pub(super) fn open_ask(&mut self) {
        let Some(at) = self.asks_focused() else { return };
        let Some(index) = self.asks_shown().get(at).copied() else { return };
        self.open_entry_card(index);
    }

    // The item's card, or the item selected on the Backlog page when it has none.
    pub(super) fn open_entry_card(&mut self, index: usize) {
        let Some(entry) = self.backlog.entries.get(index).cloned() else { return };
        if !self.open_item_card(entry) {
            self.ask_focus = None;
            self.go_page(Page::Backlog);
            self.backlog_selected = self.backlog_rows().iter().position(|row| *row == super::BacklogRow::Entry(index)).unwrap_or(0);
        }
    }
}

// The rows, starting at line `first`, with the selected one's line.
pub(super) fn ask_lines(app: &App, width: usize, first: usize, focused_column: bool) -> (Vec<Line<'static>>, Rows, Option<usize>) {
    let place = app.checkouts.len() > 1;
    let mut lines = Vec::new();
    let mut rows = Vec::new();
    let mut selected_line = None;
    for (at, index) in app.asks_shown().into_iter().enumerate() {
        let selected = app.ask_focus == Some(at);
        let start = first + lines.len();
        if selected {
            selected_line = Some(start);
        }
        lines.push(Line::default());
        lines.push(ask_line(&app.backlog.entries[index], selected, focused_column, place, width));
        rows.push((start + 1, start + 2, Hit::Ask(at)));
        if selected {
            selected_line = Some(start + 1);
        }
    }
    (lines, rows, selected_line)
}

#[cfg(test)]
mod tests {
    use crate::backlog::Entry;
    use crate::backlog_file::{Shape, State};
    use crate::board::test_support::*;
    use crate::board::{Column, Mode};
    use ratatui::crossterm::event::KeyCode;

    fn item(title: &str, priority: &str, age: &str, waiting: usize) -> Entry {
        let shape = Shape { state: Some(if waiting > 0 { State::Drafting } else { State::Ready }), waiting, start: true, ..Shape::default() };
        Entry { source: "vault".to_string(), title: title.to_string(), priority: priority.to_string(), area: "web".to_string(), reference: format!("Projects/{}.md", title.to_lowercase().replace(' ', "-")), age: age.to_string(), shape, ..Entry::default() }
    }

    fn backlog() -> Vec<Entry> {
        vec![item("Rounding per invoice", "high", "5 d", 2), item("Session drops on reload", "low", "9 d", 0), item("Links show used pages", "medium", "1 d", 1)]
    }

    #[test]
    fn needs_you_on_the_work_page_lists_each_item_you_asked_to_start_whose_questions_wait_with_no_task_there() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), Some("ready"), 2)]);
        app.backlog.entries = backlog();
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        let header = find(&rows, "NEEDS YOU  2");
        assert_eq!(header.len(), 1, "{screen}");
        let (newest, older) = (find(&rows, "Links show used pages")[0].1, find(&rows, "Rounding per invoice")[0].1);
        assert!(header[0].1 < newest && newest < older && older < find(&rows, "READY TO PUSH")[0].1, "newest first, above the tasks' groups: {screen}");
        assert!(rows[older].contains("2 questions · answer on its card") && rows[newest].contains("1 question · answer on its card"), "{screen}");
        assert!(!screen.contains("Session drops on reload"), "an item no question holds stays on the Backlog page: {screen}");
        assert!(screen.contains("● 2 need you"), "the count holds them: {screen}");
    }

    #[test]
    fn an_item_whose_questions_wait_but_nobody_asked_to_start_shows_only_under_the_backlog_pages_needs_you() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), Some("needs-you"), 0)]);
        app.backlog.entries = vec![Entry { shape: Shape { start: false, ..item("Dark mode", "medium", "2 d", 1).shape }, ..item("Dark mode", "medium", "2 d", 1) }];
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(!screen.contains("Dark mode") && screen.contains("NEEDS YOU  1") && screen.contains("● 1 need you"), "Needs you on the Work page holds the task alone: {screen}");
        assert!(screen.contains("2 ?1"), "the Backlog tab still marks the question: {screen}");
        press(&mut app, key(KeyCode::Down));
        assert_eq!(app.ask_focus, None, "no row to step onto");
        app.column = Column::Backlog;
        let screen = render(&mut app, 120, 30).0.join("\n");
        assert!(screen.contains("NEEDS YOU  1") && screen.contains("Dark mode"), "the Backlog page lists it under its own Needs you: {screen}");
    }

    #[test]
    fn the_rows_follow_the_tasks_under_needs_you_and_arrows_and_enter_reach_the_items_card() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), Some("needs-you"), 0), crate::status::tests::task(Some("idle"), Some("ready"), 2)]);
        app.backlog.entries = backlog();
        app.card_reader = true;
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert_eq!(find(&rows, "NEEDS YOU  3").len(), 1, "one group, its count with the items: {screen}");
        assert!(find(&rows, "Links show used pages")[0].1 < find(&rows, "READY TO PUSH")[0].1, "{screen}");
        app.work_selected = 0;
        press(&mut app, key(KeyCode::Down));
        assert_eq!(app.ask_focus, Some(0), "↓ from the last task under Needs you steps onto the first item");
        press(&mut app, key(KeyCode::Down));
        assert_eq!(app.ask_focus, Some(1));
        press(&mut app, key(KeyCode::Enter));
        let Mode::ItemCard(card) = &app.mode else { panic!("⏎ opens the item's card") };
        assert_eq!(card.entry.reference, "Projects/rounding-per-invoice.md");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal) && app.ask_focus == Some(1), "esc goes back to the row");
        press(&mut app, key(KeyCode::Down));
        assert_eq!((app.ask_focus, app.work_selected), (None, 1), "↓ from the last item goes on to the next task");
        press(&mut app, key(KeyCode::Up));
        assert_eq!(app.ask_focus, Some(1), "↑ comes back onto the last item");
        press(&mut app, key(KeyCode::Up));
        press(&mut app, key(KeyCode::Up));
        assert_eq!((app.ask_focus, app.work_selected), (None, 0), "↑ from the first item goes back to the task above");
    }

    #[test]
    fn answering_the_last_question_from_work_hands_the_cursor_back_to_the_task_it_came_from() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), Some("ready"), 2)]);
        app.backlog.entries = vec![item("Links show used pages", "medium", "1 d", 1)];
        press(&mut app, key(KeyCode::Up));
        assert_eq!(app.ask_focus, Some(0));
        app.backlog.entries[0].shape.waiting = 0;
        let (rows, _) = render(&mut app, 120, 30);
        assert_eq!(app.ask_focus, None, "{}", rows.join("\n"));
        assert!(rows.iter().any(|row| row.trim_start().starts_with("▌○ billing")), "the task is selected again: {}", rows.join("\n"));
    }

    #[test]
    fn the_rows_show_with_no_task_at_all_and_a_work_filter_keeps_those_it_matches() {
        let mut app = test_app(Vec::new());
        app.backlog.entries = backlog();
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("NEEDS YOU  2") && screen.contains("Rounding per invoice") && screen.contains("No tasks."), "{screen}");
        press(&mut app, key(KeyCode::Down));
        assert_eq!(app.ask_focus, Some(0), "with no task the first item takes the cursor");
        app.filter = Some((Column::Work, "rounding".to_string()));
        assert_eq!(app.asks_shown(), [0]);
        let screen = render(&mut app, 120, 30).0.join("\n");
        assert!(screen.contains("Rounding per invoice") && !screen.contains("Links show used pages"), "{screen}");
    }

    #[test]
    fn g_on_a_row_starts_its_item_after_the_same_warning_as_the_backlog_page() {
        let mut app = test_app(Vec::new());
        app.backlog.entries = vec![item("Rounding per invoice", "high", "5 d", 2)];
        press(&mut app, key(KeyCode::Down));
        assert_eq!(app.ask_focus, Some(0));
        press(&mut app, key(KeyCode::Char('g')));
        assert!(matches!(app.mode, Mode::Normal) && app.notice.as_ref().is_some_and(|(text, _)| text.starts_with("Not started: 2 questions wait on you")), "the waiting questions are named first");
        press(&mut app, key(KeyCode::Char('g')));
        assert!(matches!(&app.mode, Mode::Input { label, .. } if label == "Start as task: name"), "g again starts it");
    }

    #[test]
    fn a_click_on_a_row_selects_it_a_second_opens_its_card_and_the_wheel_over_it_steps_onto_the_rows() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), Some("ready"), 2)]);
        app.backlog.entries = backlog();
        app.card_reader = true;
        let (rows, _) = render(&mut app, 120, 30);
        let (x, y) = find(&rows, "Rounding per invoice")[0];
        assert_eq!(app.hit_at(x as u16, y as u16), Some(crate::board::Hit::Ask(1)));
        click(&mut app, x as u16, y as u16);
        assert!(matches!(app.mode, Mode::Normal) && app.ask_focus == Some(1), "the first click selects the row");
        click(&mut app, x as u16, y as u16);
        let Mode::ItemCard(card) = &app.mode else { panic!("the second click opens the item's card") };
        assert_eq!(card.entry.reference, "Projects/rounding-per-invoice.md");
        app.mode = Mode::Normal;
        app.ask_focus = None;
        let (rows, _) = render(&mut app, 120, 30);
        let (x, y) = find(&rows, "Links show used pages")[0];
        let wheel = app.work_mouse(ratatui::crossterm::event::MouseEvent { kind: ratatui::crossterm::event::MouseEventKind::ScrollDown, column: x as u16, row: y as u16, modifiers: ratatui::crossterm::event::KeyModifiers::NONE }, app.hit_at(x as u16, y as u16));
        assert!(wheel == Some(key(KeyCode::Down)) && app.ask_focus == Some(0), "the wheel over a row focuses the rows and moves within them");
    }
}
