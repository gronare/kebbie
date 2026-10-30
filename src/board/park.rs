use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::keys::{self, Act};
use crate::notice::{Notice, Tone};
use crate::peers;
use crate::status::Item;
use crate::tasks;
use super::widgets::{FAINT, MAUVE, MUTED, PEACH, RED, SURFACE, TEXT, bold, modal, plural, style, text_width};
use super::{App, Column, Hit, Mode};
use ratatui::style::Style;

const CHOICES: [&str; 4] = ["Tomorrow", "Monday", "A date…", "I bring it back"];
const DATE_HINT: &str = "fri · 24 nov · 2026-12-01";
const KEY_GAP: usize = 3;
const AFTER_KEY: usize = 2;
const MIN_WIDTH: usize = 39;

fn now() -> u64 {
    crate::messages::now()
}

fn offset(at: i64) -> i64 {
    crate::recover::local_offset(at)
}

fn monday(now: u64, offset: &dyn Fn(i64) -> i64) -> crate::sh::Result<u64> {
    tasks::park_until("mon", now, offset)?.ok_or_else(|| "Monday is no time".to_string())
}

// A date typed in the window: a day alone is its 09:00, and only a time to come counts.
pub(super) fn typed_date(text: &str, now: u64, offset: &dyn Fn(i64) -> i64) -> Option<u64> {
    (!text.trim().is_empty()).then(|| tasks::park_until(text, now, offset).ok().flatten()).flatten()
}

// Each choice with the time it means, when it means one.
pub(super) fn park_times(now: u64, offset: &dyn Fn(i64) -> i64) -> [Option<String>; 4] {
    let brief = |at: crate::sh::Result<u64>| at.ok().map(|at| crate::when::brief(at, now, offset, false));
    [brief(tasks::park_morning_in(1, now, offset)), brief(monday(now, offset)), None, None]
}

fn window_width(name: &str) -> usize {
    (text_width(&park_title(name)) + 14).max(MIN_WIDTH)
}

fn park_title(name: &str) -> String {
    format!("Park {name} until")
}

fn right_aligned(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let used: usize = left.iter().chain(&right).map(|span| text_width(&span.content)).sum();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used).max(1))));
    spans.extend(right);
    spans
}

pub(super) fn draw_park(frame: &mut Frame, name: &str, index: usize, date: Option<&str>) -> Vec<(Rect, Hit)> {
    let width = window_width(name);
    let inner = width - 4;
    let time_edge = inner - KEY_GAP - 1 - AFTER_KEY;
    let lines = match date {
        Some(typed) => {
            let shown = match typed_date(typed, now(), &offset) {
                Some(at) => Span::styled(crate::when::brief(at, now(), &offset, true), style(TEXT)),
                None if typed.trim().is_empty() => Span::raw(""),
                None => Span::styled("?", style(MUTED)),
            };
            vec![
                Line::from(right_aligned(vec![Span::styled("› ", style(MAUVE)), Span::styled(typed.to_string(), style(TEXT)), Span::styled("▏", style(MAUVE))], vec![shown], time_edge)),
                Line::from(Span::styled(DATE_HINT, style(MUTED))),
                Line::from(Span::styled("⏎ park · esc back", style(FAINT))),
            ]
        }
        None => {
            let times = park_times(now(), &offset);
            let mut lines: Vec<Line<'static>> = CHOICES.iter().zip(times).enumerate().map(|(position, (label, time))| {
                let active = position == index;
                let left = vec![Span::raw(if active { "▸ " } else { "  " }), Span::styled(label.to_string(), if active { bold(MAUVE) } else { style(TEXT) })];
                let right = vec![Span::styled(time.unwrap_or_default(), style(MUTED)), Span::raw(" ".repeat(KEY_GAP)), Span::styled((position + 1).to_string(), style(MUTED)), Span::raw(" ".repeat(AFTER_KEY))];
                let line = Line::from(right_aligned(left, right, inner));
                if active { line.style(Style::new().bg(SURFACE)) } else { line }
            }).collect();
            lines.push(Line::from(Span::styled("⏎ park · esc cancel", style(FAINT))));
            lines
        }
    };
    let area = modal(frame, park_title(name), lines, width as u16);
    if date.is_some() {
        return Vec::new();
    }
    (0..CHOICES.len() as u16).map(|choice| (Rect::new(area.x, area.y + 1 + choice, area.width, 1), Hit::Choice(choice as usize))).filter(|(rect, _)| rect.y + 1 < area.bottom()).collect()
}

pub(super) fn waiting_counts(parked: &[&Item]) -> Vec<String> {
    let questions = parked.iter().filter(|item| item.question.is_some()).count();
    let red = parked.iter().filter(|item| item.ci.as_ref().is_some_and(|ci| !ci.running())).count();
    let mut counts = Vec::new();
    if questions > 0 {
        counts.push(plural(questions, "question"));
    }
    if red > 0 {
        counts.push(format!("{red} red CI"));
    }
    counts
}

pub(super) fn collapsed_text(parked: &[&Item]) -> String {
    let mut parts = vec![format!("{} parked", parked.len())];
    parts.extend(waiting_counts(parked));
    parts.push(format!("{} shows them", keys::key(Act::Park)));
    parts.join(" · ")
}

pub(super) fn collapsed_line(parked: &[&Item], selected: bool, focused: bool) -> Line<'static> {
    let waiting = !waiting_counts(parked).is_empty();
    let marker = if selected && focused { Span::styled("▌", bold(super::widgets::MAUVE)) } else { Span::raw(" ") };
    let line = Line::from(vec![marker, Span::styled("◌ ", style(MUTED)), Span::styled(collapsed_text(parked), style(if waiting { PEACH } else { MUTED }))]);
    if selected { line.style(Style::new().bg(SURFACE)) } else { line }
}

pub(super) fn open_header(parked: &[&Item]) -> Line<'static> {
    let mut spans = vec![Span::styled("PARKED", bold(MUTED)), Span::styled(format!("  {}", parked.len()), style(FAINT))];
    for count in waiting_counts(parked) {
        spans.push(Span::styled(format!(" · {count}"), style(if count.contains("CI") { RED } else { PEACH })));
    }
    spans.push(Span::styled(format!("  {} hides them", keys::map().label(Act::Back)), style(FAINT)));
    Line::from(spans)
}

impl App {
    pub(super) fn first_parked(&self) -> Option<usize> {
        (0..self.tasks.len()).find(|index| self.tasks[*index].parked.is_some() && self.filtered(Column::Work, *index))
    }

    pub(super) fn parked_hidden(&self, index: usize) -> bool {
        !self.parked_open && self.tasks.get(index).is_some_and(|item| item.parked.is_some()) && self.first_parked() != Some(index)
    }

    // The collapsed parked line stands where its first task is, and keys on it act on the line, never on that task.
    pub(super) fn on_parked_line(&self) -> bool {
        self.column == Column::Work && !self.parked_open && self.first_parked() == Some(self.work_selected)
    }

    pub(super) fn parked_items(&self) -> Vec<&Item> {
        self.tasks.iter().enumerate().filter(|(index, item)| item.parked.is_some() && self.filtered(Column::Work, *index)).map(|(_, item)| item).collect()
    }

    pub(super) fn show_parked(&mut self, open: bool) {
        self.parked_open = open;
        if let Some(first) = self.first_parked().filter(|_| !open && self.tasks.get(self.work_selected).is_some_and(|item| item.parked.is_some())) {
            self.work_selected = first;
        }
    }

    pub(super) fn settle_selection(&mut self) {
        if self.parked_hidden(self.work_selected) && let Some(first) = self.first_parked() {
            self.work_selected = first;
        }
    }

    pub(super) fn press_park(&mut self, item: Item) {
        if self.on_parked_line() {
            return self.show_parked(true);
        }
        let address = peers::address(&item.name, &item.place);
        if item.parked.is_some() {
            self.about("park", &item.name);
            let result = crate::lead::state_dir().and_then(|state| tasks::unpark_address(&state, &address, now()));
            return self.parked_now(&address, None, result);
        }
        self.mode = Mode::Park { address, name: item.name, index: 0, date: None };
    }

    pub(super) fn choose_park(&mut self, address: String, name: String, index: usize, date: Option<String>, act: Option<Act>, key: KeyEvent) {
        if let Some(mut typed) = date {
            match key.code {
                KeyCode::Esc => self.mode = Mode::Park { address, name, index, date: None },
                KeyCode::Enter => match typed_date(&typed, now(), &offset) {
                    Some(at) => self.park(address, name, Ok(Some(at))),
                    None => self.mode = Mode::Park { address, name, index, date: Some(typed) },
                },
                KeyCode::Backspace => {
                    typed.pop();
                    self.mode = Mode::Park { address, name, index, date: Some(typed) };
                }
                KeyCode::Char(character) => {
                    typed.push(character);
                    self.mode = Mode::Park { address, name, index, date: Some(typed) };
                }
                _ => self.mode = Mode::Park { address, name, index, date: Some(typed) },
            }
            return;
        }
        match (key.code, act) {
            (KeyCode::Char(digit @ '1'..='4'), _) => self.pick_park(address, name, digit as usize - '1' as usize),
            (_, Some(Act::Move)) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => self.mode = Mode::Park { address, name, index: index.saturating_sub(1), date: None },
            (_, Some(Act::Move)) => self.mode = Mode::Park { address, name, index: (index + 1).min(CHOICES.len() - 1), date: None },
            (_, Some(Act::Options)) => self.pick_park(address, name, index),
            (_, Some(Act::Back | Act::Close)) => {}
            _ => self.mode = Mode::Park { address, name, index, date: None },
        }
    }

    pub(super) fn pick_park(&mut self, address: String, name: String, index: usize) {
        match index {
            0 => self.park(address, name, tasks::park_morning_in(1, now(), &offset).map(Some)),
            1 => self.park(address, name, monday(now(), &offset).map(Some)),
            2 => self.mode = Mode::Park { address, name, index, date: Some(String::new()) },
            _ => self.park(address, name, Ok(None)),
        }
    }

    fn park(&mut self, address: String, name: String, until: crate::sh::Result<Option<u64>>) {
        self.about("park", &name);
        let result = until.and_then(|until| crate::lead::state_dir().and_then(|state| tasks::park_address(&state, &address, until, now(), &offset)).map(|text| (until, text)));
        match result {
            Ok((until, text)) => self.parked_now(&address, Some(tasks::Parked { until }), Ok(text)),
            Err(error) => self.show(Err(error)),
        }
    }

    // The row moves at once; the refresh that follows reads the same from the state.
    fn parked_now(&mut self, address: &str, parked: Option<tasks::Parked>, result: crate::sh::Result<String>) {
        if result.is_ok() {
            let next = self.tasks.iter().enumerate().skip(self.work_selected + 1).chain(self.tasks.iter().enumerate().take(self.work_selected).collect::<Vec<_>>().into_iter().rev())
                .find(|(_, item)| item.parked.is_none() && peers::address(&item.name, &item.place) != address).map(|(_, item)| (item.name.clone(), item.place.clone()));
            let mut items = self.tasks.clone();
            if let Some(item) = items.iter_mut().find(|item| peers::address(&item.name, &item.place) == address) {
                item.parked = parked;
            }
            self.set_tasks(items);
            let keep = if parked.is_some() { next } else { Some(address.split_once('@').map(|(name, place)| (name.to_string(), place.to_string())).unwrap_or_default()) };
            if let Some(index) = keep.and_then(|(name, place)| self.tasks.iter().position(|item| item.name == name && item.place == place)) {
                self.work_selected = index;
            }
            self.settle_selection();
            let _ = self.refresh.send(());
        }
        self.show(result);
    }
}

pub(super) fn not_on_the_line() -> crate::sh::Result<String> {
    Notice::new(Tone::NotNow, "That is the parked line").next(format!("{} shows the parked tasks first.", keys::key(Act::Park))).ok()
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;

    use super::*;
    use crate::board::test_support::*;
    use crate::status::Group;

    fn red_ci(item: &mut Item) {
        item.ci = Some(crate::ci::Watch {
            address: format!("{}@{}", item.name, item.place), task: item.name.clone(), repo: std::path::PathBuf::new(), slug: "acme/web".to_string(), sha: "abc".to_string(), started_at: 1, checked_at: None, close: true, runs: Vec::new(),
            state: crate::ci::State::Failed { at: 2, what: "CI · test".to_string(), url: String::new() }, error: None, erring_since: None, pushed: crate::ci::Pushed::default(),
        });
    }

    fn board_with_two_parked() -> crate::board::App {
        let mut asking = served("refund-mails");
        asking.question = crate::messages::fold(QUESTION).into_iter().next();
        asking.parked = Some(tasks::Parked { until: None });
        let mut red = served("nightly-import");
        red_ci(&mut red);
        red.parked = Some(tasks::Parked { until: Some(now() + 3600) });
        let mut app = test_app(Vec::new());
        app.set_tasks(vec![red, served("search-filters"), asking]);
        app
    }

    #[test]
    fn parked_tasks_leave_their_groups_for_one_line_that_counts_what_waits() {
        let mut app = board_with_two_parked();
        assert_eq!(app.tasks.iter().map(|item| (item.name.as_str(), item.group())).collect::<Vec<_>>(), [("search-filters", Group::ReadyToPush), ("nightly-import", Group::Parked), ("refund-mails", Group::Parked)], "a question or red CI keeps a parked task parked, last");
        let (rows, _) = render(&mut app, 110, 30);
        let screen = rows.join("\n");
        let line = find(&rows, "2 parked")[0];
        assert!(rows[line.1].contains("◌ 2 parked · 1 question · 1 red CI · Z shows them"), "{screen}");
        assert!(find(&rows, "READY TO PUSH")[0].1 < line.1 && !screen.contains("NEEDS YOU") && !screen.contains("nightly-import") && !screen.contains("refund-mails"), "{screen}");
        press(&mut app, key(KeyCode::Down));
        assert!(app.on_parked_line(), "the line is the last row");
        press(&mut app, key(KeyCode::Down));
        assert!(app.on_parked_line(), "nothing hides below it");
        press(&mut app, key(KeyCode::Char('g')));
        let (rows, _) = render(&mut app, 110, 30);
        assert!(rows.join("\n").contains("That is the parked line"), "a task key on the line never reaches a hidden task");
        app.mode = Mode::Normal;
        press(&mut app, key(KeyCode::Char('Z')));
        assert!(app.parked_open && app.selected_task().is_some_and(|item| item.name == "nightly-import"));
        let (rows, _) = render(&mut app, 110, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("PARKED  2 · 1 question · 1 red CI  esc hides them"), "{screen}");
        let back = format!("back {}", crate::when::shown(now() + 3600, now(), &offset));
        assert!(rows[find(&rows, "nightly-import")[0].1].contains(&back) && rows[find(&rows, "refund-mails")[0].1].contains("back when you unpark it"), "{screen}");
        assert!(!screen.contains("answer a question"), "outside Needs you it asks nothing of you: {screen}");
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Esc));
        assert!(!app.parked_open && app.on_parked_line(), "esc folds them back into the line");
        press(&mut app, key(KeyCode::Enter));
        assert!(app.parked_open && matches!(app.mode, Mode::Normal), "⏎ on the line shows them too");
    }

    fn window(rows: &[String], name: &str, height: usize) -> Vec<String> {
        let (left, top) = find(rows, &format!("╭ Park {name} until"))[0];
        rows[top..top + height].iter().map(|row| row.chars().skip(left).take(39).collect()).collect()
    }

    #[test]
    fn the_park_window_offers_tomorrow_first_with_the_time_each_choice_means_and_parks_with_enter() {
        let mut app = test_app(vec![served("search-filters"), served("parkwin-next")]);
        press(&mut app, key(KeyCode::Char('Z')));
        assert!(matches!(&app.mode, Mode::Park { address, index: 0, date: None, .. } if address == "search-filters@web"));
        let (rows, _) = render(&mut app, 110, 30);
        let [tomorrow, monday, ..] = park_times(now(), &offset);
        let row = |marker: &str, label: &str, time: &str, key: char| format!("│ {marker}{label}{}{time}   {key}   │", " ".repeat(29 - 2 - label.chars().count() - time.chars().count()));
        assert_eq!(window(&rows, "search-filters", 7), [
            "╭ Park search-filters until ──────────╮".to_string(),
            row("▸ ", "Tomorrow", tomorrow.as_deref().unwrap(), '1'),
            row("  ", "Monday", monday.as_deref().unwrap(), '2'),
            row("  ", "A date…", "", '3'),
            row("  ", "I bring it back", "", '4'),
            "│ ⏎ park · esc cancel                 │".to_string(),
            "╰─────────────────────────────────────╯".to_string(),
        ], "{}", rows.join("\n"));
        assert_eq!(tomorrow, Some(crate::when::brief(tasks::park_morning_in(1, now(), &offset).unwrap(), now(), &offset, false)), "the weekday of tomorrow at 09:00");
        press(&mut app, key(KeyCode::Enter));
        let state = crate::lead::state_dir().unwrap();
        assert_eq!(tasks::park_of(&state, "search-filters@web", now()), tasks::park_morning_in(1, now(), &offset).ok().map(|until| tasks::Parked { until: Some(until) }), "Z then ⏎ parks it until tomorrow at 09:00");
        assert_eq!(app.selected_task().map(|item| item.name).as_deref(), Some("parkwin-next"), "the selection stays on the work in front of you");
        assert!(app.tasks.iter().any(|item| item.name == "search-filters" && item.group() == Group::Parked));
        app.mode = Mode::Normal;
        app.show_parked(true);
        app.work_selected = app.tasks.iter().position(|item| item.name == "search-filters").unwrap();
        press(&mut app, key(KeyCode::Char('Z')));
        assert_eq!(tasks::park_of(&state, "search-filters@web", now()), None, "Z on a parked task brings it back now");
        assert!(app.tasks.iter().any(|item| item.name == "search-filters" && item.group() == Group::ReadyToPush));
    }

    #[test]
    fn a_date_turns_the_window_into_a_field_that_reads_the_date_as_you_type() {
        let mut app = test_app(vec![served("parkwin-date")]);
        press(&mut app, key(KeyCode::Char('Z')));
        press(&mut app, key(KeyCode::Char('3')));
        assert!(matches!(&app.mode, Mode::Park { date: Some(typed), .. } if typed.is_empty()));
        for character in "thu".chars() {
            press(&mut app, key(KeyCode::Char(character)));
        }
        let (rows, _) = render(&mut app, 110, 30);
        let thursday = crate::when::brief(typed_date("thu", now(), &offset).unwrap(), now(), &offset, true);
        assert_eq!(window(&rows, "parkwin-date", 5), [
            "╭ Park parkwin-date until ────────────╮".to_string(),
            format!("│ › thu▏{}{thursday}       │", " ".repeat(29 - 6 - thursday.chars().count())),
            "│ fri · 24 nov · 2026-12-01           │".to_string(),
            "│ ⏎ park · esc back                   │".to_string(),
            "╰─────────────────────────────────────╯".to_string(),
        ], "{}", rows.join("\n"));
        assert!(thursday.starts_with("thu ") && thursday.ends_with(" 09:00"), "{thursday}");
        for _ in 0..3 {
            press(&mut app, key(KeyCode::Backspace));
        }
        for character in "soon".chars() {
            press(&mut app, key(KeyCode::Char(character)));
        }
        let (rows, _) = render(&mut app, 110, 30);
        assert_eq!(window(&rows, "parkwin-date", 2)[1], format!("│ › soon▏{}?       │", " ".repeat(29 - 7 - 1)), "a date it cannot read is a dim ?");
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::Park { date: Some(typed), .. } if typed == "soon"), "⏎ on a date it cannot read parks nothing");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(&app.mode, Mode::Park { index: 2, date: None, .. }), "esc goes back to the four choices");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn monday_a_date_or_until_brought_back() {
        let state = crate::lead::state_dir().unwrap();
        let park = |choice: char, typed: Option<&str>| {
            let mut app = test_app(vec![served("parkwin-pick")]);
            press(&mut app, key(KeyCode::Char('Z')));
            press(&mut app, key(KeyCode::Char(choice)));
            if let Some(typed) = typed {
                for character in typed.chars() {
                    press(&mut app, key(KeyCode::Char(character)));
                }
                press(&mut app, key(KeyCode::Enter));
            }
            let parked = tasks::park_of(&state, "parkwin-pick@web", now());
            tasks::unpark_address(&state, "parkwin-pick@web", now()).ok();
            parked
        };
        assert_eq!(park('2', None), Some(tasks::Parked { until: tasks::park_until("mon 09:00", now(), &offset).unwrap() }), "Monday at 09:00, as a later start reads mon");
        assert_eq!(park('3', Some("2027-01-04")), Some(tasks::Parked { until: tasks::park_until("2027-01-04 09:00", now(), &offset).unwrap() }), "the date the way a later start takes it, at 09:00");
        assert_eq!(park('4', None), Some(tasks::Parked { until: None }));
        assert_eq!(typed_date("unparked", now(), &offset), None, "the field takes a date, not the way to park until brought back");
    }

    #[test]
    fn a_parked_task_comes_back_by_itself_when_its_time_comes() {
        let state = crate::lead::state_dir().unwrap();
        tasks::park_address(&state, "parkwin-time@web", Some(now() + 60), now(), &offset).unwrap();
        assert!(tasks::park_of(&state, "parkwin-time@web", now()).is_some());
        assert_eq!(tasks::park_of(&state, "parkwin-time@web", now() + 60), None, "the board's next refresh reads it back in its group");
        let mut item = served("parkwin-time");
        item.parked = tasks::park_of(&state, "parkwin-time@web", now() + 60);
        assert_eq!(item.group(), Group::ReadyToPush);
        tasks::unpark_address(&state, "parkwin-time@web", now()).ok();
    }
}
