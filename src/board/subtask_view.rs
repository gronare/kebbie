use std::path::Path;
use std::rc::Rc;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keys::{self, Act};
use crate::obstacle::{Kind, Obstacle, State};
use crate::peers;
use crate::sh::Result;
use crate::status::Item;
use crate::tasks::Subtask;
use super::review_data::{self as data, File, Range};
use super::steps::{mark_color, walked};
use super::widgets::{GREEN, LAVENDER, MAUVE, MUTED, PEACH, RED, TEAL, TEXT, bold, cut, style, text_width, wrap};
use super::{App, Column, Mode};

// What the subtask view reads beyond the board's own item: the files of the subtask's diff with their read marks, what its commits say past their subjects, and an obstacle open on it.
#[derive(Clone, Debug, Default)]
pub(super) struct Loaded {
    pub files: Vec<File>,
    pub built: Vec<String>,
    pub obstacle: Option<Obstacle>,
}

pub(super) type Load = Rc<dyn Fn(&Path, &str, &Subtask) -> Loaded>;
pub(super) type Accept = Rc<dyn Fn(&Path, &str, &str) -> Result<String>>;

pub(super) struct Io {
    pub load: Load,
    pub accept: Accept,
}

impl Default for Io {
    fn default() -> Self {
        Io { load: Rc::new(load), accept: Rc::new(|path, name, id| crate::tasks::accept(path, Some(name), None, Some(id), true)) }
    }
}

#[derive(Clone)]
pub(super) struct View {
    pub address: String,
    pub id: String,
    pub loaded: Loaded,
    pub scroll: u16,
}

// The subtask's whole diff as the review shows it: under its first commit to its last, or its last commit alone when another subtask's lies between.
fn range_of(subtask: &Subtask) -> Option<Range> {
    let first = subtask.commits.first()?;
    let head = subtask.head()?;
    Some(if subtask.contiguous { Range { from: format!("{}^", first.sha), to: head.to_string() } } else { Range::commit(head) })
}

fn trailer(line: &str) -> bool {
    line.split_once(": ").is_some_and(|(key, _)| !key.is_empty() && key.chars().all(|character| character.is_ascii_alphanumeric() || character == '-'))
}

// A commit message's body as paragraphs, its trailers left out.
pub(super) fn body(message: &str) -> Vec<String> {
    let Some((_, rest)) = message.split_once('\n') else { return Vec::new() };
    rest.split("\n\n").map(|paragraph| paragraph.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>())
        .filter(|lines| !lines.is_empty() && !lines.iter().all(|line| trailer(line)))
        .map(|lines| lines.join(" ")).collect()
}

fn load(path: &Path, address: &str, subtask: &Subtask) -> Loaded {
    let files = range_of(subtask).map(|range| data::range_files(path, &range)).unwrap_or_default();
    let built = subtask.commits.iter().flat_map(|commit| body(&data::message(path, &commit.sha))).collect();
    let obstacle = crate::lead::state_dir().ok().and_then(|state| crate::obstacle::load(&state, address).into_iter().rev().find(|obstacle| obstacle.state.open() && obstacle.on == subtask.title));
    Loaded { files, built, obstacle }
}

fn waits_for_you(obstacle: &Obstacle) -> bool {
    obstacle.state == State::Asked || obstacle.decision.as_ref().is_some_and(|decision| decision.kind == Kind::Ask)
}

fn address_of(item: &Item) -> String {
    peers::address(&item.name, &item.place)
}

fn count(added: &str) -> usize {
    added.parse().unwrap_or_default()
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

fn key(text: String) -> Span<'static> {
    Span::styled(text, style(MAUVE))
}

fn label(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), style(MUTED))
}

fn section(title: &str) -> Line<'static> {
    Line::from(Span::styled(title.to_string(), bold(LAVENDER)))
}

fn facts(step: &Subtask, loaded: &Loaded) -> Line<'static> {
    let (word, color) = match &loaded.obstacle {
        Some(_) => ("obstacle", PEACH),
        None => (crate::tasks::subtask_word(&step.state), mark_color(&step.state)),
    };
    let mut spans = vec![Span::styled(word.to_string(), style(color))];
    if step.commits.is_empty() {
        spans.push(label(" · no commits yet"));
        return Line::from(spans);
    }
    let files = &loaded.files;
    let added: usize = files.iter().map(|file| count(&file.added)).sum();
    let deleted: usize = files.iter().map(|file| count(&file.deleted)).sum();
    spans.extend([label(&format!(" · {} · {} · ", plural(step.commits.len(), "commit"), plural(files.len(), "file"))), Span::styled(format!("+{added}"), style(GREEN)), Span::raw(" "), Span::styled(format!("−{deleted}"), style(RED))]);
    if loaded.obstacle.is_none() && !files.is_empty() {
        spans.push(label(&format!(" · {} of {} read", files.iter().filter(|file| file.read).count(), files.len())));
    }
    Line::from(spans)
}

fn obstacle_lines(obstacle: &Obstacle, width: usize) -> Vec<Line<'static>> {
    let state = if waits_for_you(obstacle) { "waits for you".to_string() } else { obstacle.state.word().to_string() };
    let mut lines = vec![Line::from(vec![Span::styled(format!("OBSTACLE {}", obstacle.id), bold(PEACH)), label(&format!("  {state}"))])];
    lines.extend(wrap(&obstacle.text, width.saturating_sub(2), 6).into_iter().map(|text| Line::from(vec![Span::raw("  "), Span::styled(text, style(TEXT))])));
    let decided = obstacle.decision.as_ref().map(|decision| {
        let by = decision.by.word();
        let by = by[..1].to_uppercase() + &by[1..];
        let said = if decision.kind == Kind::Ask { if decision.plan.is_empty() { decision.reason.clone() } else { decision.plan.clone() } } else { decision.line() };
        format!("{by}: {said}")
    });
    let asked = obstacle.question.as_ref().map(|question| format!("Asks you: {question}"));
    let said: Vec<String> = decided.into_iter().chain(asked).collect();
    if !said.is_empty() {
        lines.extend(wrap(&said.join(" "), width.saturating_sub(2), 6).into_iter().map(|text| Line::from(vec![Span::raw("  "), Span::styled(text, style(TEXT))])));
    }
    lines
}

fn file_line(file: &File, room: usize) -> Line<'static> {
    let (mark, color) = if file.read { ("✓", GREEN) } else { ("○", MUTED) };
    let path = cut(&file.path, room);
    let mut spans = vec![Span::raw("  "), Span::styled(mark, style(color)), Span::raw(" "), Span::styled(path.clone(), style(TEXT)), Span::raw(" ".repeat(room.saturating_sub(text_width(&path)) + 2))];
    if count(&file.added) > 0 || file.deleted.is_empty() {
        spans.push(Span::styled(format!("+{:<5}", file.added), style(GREEN)));
    } else {
        spans.push(Span::raw(" ".repeat(6)));
    }
    if count(&file.deleted) > 0 {
        spans.push(Span::styled(format!("−{}", file.deleted), style(RED)));
    }
    Line::from(spans)
}

// Everything the view shows above its keys, for the subtask as the board has it now.
pub(super) fn view_lines(item: &Item, step: &Subtask, loaded: &Loaded, width: usize) -> Vec<Line<'static>> {
    let total = item.subtasks.pushed.len() + item.subtasks.items.len();
    let mut lines = vec![
        Line::from(vec![label(&format!("{} › ", item.titled())), Span::styled(format!("Subtask {} of {total}", step.number), bold(LAVENDER))]),
        Line::from(Span::styled(step.title.clone(), bold(TEXT))),
        facts(step, loaded),
    ];
    if let Some(obstacle) = &loaded.obstacle {
        lines.push(Line::default());
        lines.extend(obstacle_lines(obstacle, width));
    }
    if !step.commits.is_empty() {
        lines.extend([Line::default(), section("COMMITS")]);
        lines.extend(step.commits.iter().map(|commit| Line::from(vec![Span::raw("  "), Span::styled(data::short(&commit.sha).to_string(), style(TEAL)), Span::raw("  "), Span::styled(cut(&commit.subject, width.saturating_sub(11)), style(TEXT))])));
    }
    if !loaded.files.is_empty() {
        let room = loaded.files.iter().map(|file| text_width(&file.path)).max().unwrap_or(0).min(width.saturating_sub(20));
        lines.extend([Line::default(), section("FILES")]);
        lines.extend(loaded.files.iter().map(|file| file_line(file, room)));
    }
    if !loaded.built.is_empty() {
        lines.extend([Line::default(), section("BUILT")]);
        for paragraph in &loaded.built {
            lines.extend(wrap(paragraph, width.saturating_sub(2), 12).into_iter().map(|text| Line::from(vec![Span::raw("  "), Span::styled(text, style(TEXT))])));
        }
    }
    lines
}

pub(super) fn hint(steps: &[Subtask], at: usize, waits: bool) -> Line<'static> {
    let (previous, next) = (at.checked_sub(1).and_then(|before| steps.get(before)).map(|step| step.number), steps.get(at + 1).map(|step| step.number));
    let arrows = match (previous, next) {
        (Some(previous), Some(next)) => Some(("← →", format!(" subtask {previous} or {next}"))),
        (Some(previous), None) => Some(("←", format!(" subtask {previous}"))),
        (None, Some(next)) => Some(("→", format!(" subtask {next}"))),
        (None, None) => None,
    };
    let mut parts: Vec<(String, String)> = Vec::new();
    if waits {
        parts.push((keys::map().label(Act::Options), " answers".to_string()));
    }
    parts.extend(arrows.map(|(keys, said)| (keys.to_string(), said)));
    if !waits {
        parts.push((keys::key(Act::Review).to_string(), " reads the diff".to_string()));
        parts.push((keys::key(Act::Permit).to_string(), " accepts".to_string()));
    }
    parts.push((keys::map().label(Act::Back), " back to the board".to_string()));
    let mut spans = Vec::new();
    for (index, (keys, said)) in parts.into_iter().enumerate() {
        if index > 0 {
            spans.push(label(" · "));
        }
        spans.extend([key(keys), label(&said)]);
    }
    Line::from(spans)
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let Mode::Subtask(view) = &app.mode else { return };
    let Some(item) = app.tasks.iter().find(|item| address_of(item) == view.address) else { return };
    let steps = walked(item);
    let Some(at) = steps.iter().position(|step| step.id == view.id) else { return };
    let lines = view_lines(item, &steps[at], &view.loaded, area.width as usize);
    let scroll = view.scroll.min(lines.len().saturating_sub(area.height as usize) as u16);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
}

impl App {
    // Where the cursor is among the task's subtask rows, while that task is the selected one.
    pub(super) fn subtask_at(&self, item: &Item) -> Option<usize> {
        let (address, id) = self.subtask_row.as_ref()?;
        let selected = self.selected_task()?;
        (self.column == Column::Work && address_of(&selected) == *address && address_of(item) == *address).then_some(())?;
        walked(item).iter().position(|step| step.id == *id)
    }

    // On a subtask row and in the subtask view the keys at the bottom are the subtask's.
    pub(super) fn subtask_keys(&self) -> Option<Line<'static>> {
        if let Mode::Subtask(view) = &self.mode {
            let item = self.tasks.iter().find(|item| address_of(item) == view.address)?;
            let steps = walked(item);
            let at = steps.iter().position(|step| step.id == view.id)?;
            return Some(hint(steps, at, view.loaded.obstacle.as_ref().is_some_and(waits_for_you)));
        }
        let item = self.selected_task().filter(|_| matches!(self.mode, Mode::Normal))?;
        let at = self.subtask_at(&item)?;
        Some(super::steps::subtask_hint(walked(&item)[at].number))
    }

    fn put_cursor(&mut self, item: &Item, step: &Subtask) {
        self.subtask_row = Some((address_of(item), step.id.clone()));
    }

    // The keys a subtask row takes; false leaves the key to the task.
    pub(super) fn subtask_key(&mut self, act: Act, key: &KeyEvent) -> bool {
        if self.column != Column::Work || self.on_parked_line() {
            return false;
        }
        let Some(item) = self.selected_task() else { return false };
        let steps = walked(&item).to_vec();
        let down = !matches!(key.code, KeyCode::Up | KeyCode::Char('k'));
        match (act, self.subtask_at(&item)) {
            (Act::Move, None) if down && !steps.is_empty() => self.put_cursor(&item, &steps[0]),
            (Act::Move, Some(at)) if down && at + 1 < steps.len() => self.put_cursor(&item, &steps[at + 1]),
            (Act::Move, Some(at)) if !down && at > 0 => self.put_cursor(&item, &steps[at - 1]),
            (Act::Move, Some(_)) if !down => self.subtask_row = None,
            (Act::Move, _) => {
                self.subtask_row = None;
                return false;
            }
            (Act::Options, Some(at)) => self.open_subtask(&item, &steps[at]),
            (Act::Permit, Some(at)) => self.accept_subtask(&item, &steps[at]),
            (Act::Review, Some(at)) => self.open_review_at(&item, Some(steps[at].number)),
            (Act::Back, Some(_)) => self.subtask_row = None,
            _ => return false,
        }
        true
    }

    // Up onto another task lands on its last subtask row, the way down left it.
    pub(super) fn landed(&mut self, down: bool, before: usize) {
        if down || self.work_selected == before {
            return;
        }
        if let Some(item) = self.selected_task()
            && let Some(last) = walked(&item).last()
        {
            self.put_cursor(&item, &last.clone());
        }
    }

    fn open_subtask(&mut self, item: &Item, step: &Subtask) {
        let Some(path) = item.path.clone() else { return };
        let address = address_of(item);
        let loaded = (self.subtask_io.load)(&path, &address, step);
        self.mode = Mode::Subtask(Box::new(View { address, id: step.id.clone(), loaded, scroll: 0 }));
    }

    fn accept_subtask(&mut self, item: &Item, step: &Subtask) {
        let Some(path) = item.path.clone() else { return };
        self.about("accept", &item.name);
        let result = (self.subtask_io.accept)(&path, &item.name, &step.id);
        self.show(result);
    }

    pub(super) fn handle_subtask(&mut self, mut view: Box<View>, key: KeyEvent, act: Option<Act>) {
        let Some(item) = self.tasks.iter().find(|item| address_of(item) == view.address).cloned() else { return };
        let steps = walked(&item).to_vec();
        let Some(at) = steps.iter().position(|step| step.id == view.id) else { return };
        let waits = view.loaded.obstacle.as_ref().is_some_and(waits_for_you);
        match act {
            Some(Act::Back | Act::Close) => {
                if let Some(index) = self.tasks.iter().position(|task| address_of(task) == view.address) {
                    self.work_selected = index;
                }
                self.column = Column::Work;
                self.subtask_row = Some((view.address.clone(), view.id.clone()));
            }
            Some(Act::Switch) => {
                let to = if key.code == KeyCode::Left { at.checked_sub(1) } else { Some(at + 1).filter(|next| *next < steps.len()) };
                if let (Some(to), Some(path)) = (to, item.path.as_deref()) {
                    view = Box::new(View { address: view.address.clone(), id: steps[to].id.clone(), loaded: (self.subtask_io.load)(path, &view.address, &steps[to]), scroll: 0 });
                }
                self.mode = Mode::Subtask(view);
            }
            Some(Act::Move) => {
                view.scroll = if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) { view.scroll.saturating_sub(1) } else { view.scroll.saturating_add(1) };
                self.mode = Mode::Subtask(view);
            }
            Some(Act::Options) if waits => {
                let about = view.loaded.obstacle.as_ref().map(|obstacle| crate::obstacle::question_about(&view.address, &obstacle.id));
                let asked = self.questions.iter().position(|question| question.about.is_some() && question.about == about).or_else(|| self.question_of(&item));
                self.subtask_row = Some((view.address.clone(), view.id.clone()));
                match asked {
                    Some(asked) => self.open_questions(Some(asked)),
                    None => self.mode = Mode::Subtask(view),
                }
            }
            Some(Act::Permit) => {
                self.mode = Mode::Subtask(view.clone());
                self.accept_subtask(&item, &steps[at]);
                if matches!(self.mode, Mode::Normal) {
                    self.mode = Mode::Subtask(view);
                }
            }
            Some(Act::Review) => {
                self.mode = Mode::Subtask(view);
                self.open_review_at(&item, Some(steps[at].number));
            }
            _ => self.mode = Mode::Subtask(view),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::board::steps::tests::{held, holds, step, stepped, stripe};
    use crate::board::test_support::*;
    use crate::tasks::{SubtaskState, Subtasks};

    fn key(app: &mut App, code: KeyCode) {
        press(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn files() -> Vec<File> {
        let file = |path: &str, added: &str, deleted: &str, read: bool| File { path: path.to_string(), added: added.to_string(), deleted: deleted.to_string(), new: false, other: false, read };
        vec![file("app/gateway.rb", "38", "12", true), file("app/stripe.rb", "64", "0", false)]
    }

    fn worked(mut item: Item) -> Item {
        item.path = Some(PathBuf::from("/work/wt1.billing"));
        item
    }

    // Two tasks under Working: the first with a pushed subtask and three open ones, the second plain.
    fn two_tasks() -> (App, Rc<RefCell<Vec<String>>>) {
        let mut steps = stripe();
        holds(&mut steps.items[1], &["3333333cccc"], false);
        let first = worked(stepped(steps));
        let mut second = crate::status::tests::task(Some("working"), None, 1);
        second.name = "search".to_string();
        crate::status::tests::in_phase(&mut second, crate::phase::Phase::Working);
        let mut app = test_app(vec![first, second]);
        let accepted = Rc::new(RefCell::new(Vec::new()));
        let seen = accepted.clone();
        app.subtask_io = Io {
            load: Rc::new(|_, _, step| Loaded { files: if step.commits.is_empty() { Vec::new() } else { files() }, built: vec![format!("{} keeps the charge in one place.", step.title)], obstacle: None }),
            accept: Rc::new(move |path, name, id| {
                seen.borrow_mut().push(format!("{} {name} {id}", path.display()));
                crate::notice::Notice::new(crate::notice::Tone::Done, format!("Accepted {id}")).ok()
            }),
        };
        (app, accepted)
    }

    fn screen(app: &mut App) -> (Vec<String>, String) {
        let (rows, _) = render(app, 120, 40);
        let screen = rows.join("\n");
        (rows, screen)
    }

    fn bar_row<'a>(rows: &'a [String], text: &str) -> &'a String {
        rows.iter().find(|row| row.contains(text)).unwrap_or_else(|| panic!("no {text}: {}", rows.join("\n")))
    }

    #[test]
    fn a_task_of_one_subtask_shows_no_subtask_row_and_keeps_its_other_lines() {
        let only = Subtasks { items: vec![held(0, "Charge through a PaymentGateway", &["2222222bbbb"], false)], ..Subtasks::default() };
        let mut app = test_app(vec![worked(stepped(only))]);
        let (_, shown) = screen(&mut app);
        assert!(!shown.contains("1 Charge through a PaymentGateway"), "{shown}");
        assert!(shown.contains("Switch the payment provider to Stripe") && shown.contains("1 to accept"), "the row and what waits stay: {shown}");
        key(&mut app, KeyCode::Down);
        assert_eq!(app.subtask_row, None, "no row to walk into");
        let (_, shown) = screen(&mut app);
        assert!(!shown.contains("opens subtask"), "{shown}");
        let mut two = stripe();
        two.items.truncate(1);
        let mut app = test_app(vec![worked(stepped(two))]);
        let (_, shown) = screen(&mut app);
        assert!(shown.contains("↑ 1 pushed") && shown.contains("2 Charge through a PaymentGateway"), "one pushed and one open are two: {shown}");
    }

    #[test]
    fn down_walks_into_the_subtask_rows_one_by_one_and_on_to_the_next_task_and_up_walks_back() {
        let (mut app, _) = two_tasks();
        key(&mut app, KeyCode::Down);
        let (rows, shown) = screen(&mut app);
        assert!(bar_row(&rows, "2 Charge through a PaymentGateway").contains('▌'), "{shown}");
        assert!(shown.contains("⏎ opens subtask 2 · a accepts it · r reads its diff · esc back to the task"), "{shown}");
        assert!(!shown.contains("⏎ options"), "the task's own hint gives way: {shown}");
        key(&mut app, KeyCode::Char('j'));
        let (_, shown) = screen(&mut app);
        assert!(shown.contains("opens subtask 3"), "{shown}");
        key(&mut app, KeyCode::Down);
        let (_, shown) = screen(&mut app);
        assert!(shown.contains("opens subtask 4") && app.work_selected == 0, "{shown}");
        key(&mut app, KeyCode::Down);
        let (_, shown) = screen(&mut app);
        assert!(app.work_selected == 1 && app.subtask_row.is_none() && !shown.contains("opens subtask"), "after the last subtask the next task: {shown}");
        key(&mut app, KeyCode::Up);
        let (_, shown) = screen(&mut app);
        assert!(app.work_selected == 0 && shown.contains("opens subtask 4"), "up lands on the last subtask: {shown}");
        key(&mut app, KeyCode::Char('k'));
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Up);
        assert!(app.work_selected == 0 && app.subtask_row.is_none(), "up past the first subtask is the task's row");
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Esc);
        assert!(app.subtask_row.is_none() && app.work_selected == 0, "esc goes back to the task");
    }

    #[test]
    fn the_cursor_follows_a_long_list_down_the_column() {
        let steps = Subtasks { items: (0..18).map(|index| held(index, &format!("Subtask number {index}"), &[&format!("{index:07}aaaa")], false)).collect(), ..Subtasks::default() };
        let mut app = test_app(vec![worked(stepped(steps))]);
        for _ in 0..17 {
            key(&mut app, KeyCode::Down);
        }
        let (rows, _) = render(&mut app, 120, 20);
        assert!(bar_row(&rows, "18 Subtask number 17").contains('▌'), "{}", rows.join("\n"));
    }

    #[test]
    fn enter_on_a_subtask_row_opens_it_over_the_board_with_its_commits_files_and_what_was_built() {
        let (mut app, _) = two_tasks();
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Subtask(_)));
        let (rows, shown) = screen(&mut app);
        assert!(!shown.contains("WORK ") && !shown.contains("BACKLOG") && !shown.contains("⏎ options"), "the view fills the board, its keys at the bottom: {shown}");
        assert!(shown.contains("Switch the payment provider to Stripe › Subtask 3 of 4") && shown.contains("Add Stripe as the gateway"), "{shown}");
        assert!(shown.contains("ready · 1 commit · 2 files · +102 −12 · 1 of 2 read"), "{shown}");
        let commits = find(&rows, "COMMITS")[0].1;
        assert_eq!(rows[commits + 1].trim(), "3333333  subject of 3333333cccc", "{shown}");
        let files = find(&rows, "FILES")[0].1;
        assert!(rows[files + 1].trim().starts_with("✓ app/gateway.rb") && rows[files + 1].contains("+38") && rows[files + 1].contains("−12"), "{shown}");
        assert!(rows[files + 2].trim().starts_with("○ app/stripe.rb") && !rows[files + 2].contains('−'), "{shown}");
        assert!(shown.contains("BUILT") && shown.contains("Add Stripe as the gateway keeps the charge in one place."), "{shown}");
        assert!(shown.contains("← → subtask 2 or 4 · r reads the diff · a accepts · esc back to the board"), "{shown}");
        key(&mut app, KeyCode::Right);
        let (_, shown) = screen(&mut app);
        assert!(shown.contains("Subtask 4 of 4") && shown.contains("planned · no commits yet") && !shown.contains("FILES") && shown.contains("← subtask 3 · r"), "{shown}");
        key(&mut app, KeyCode::Right);
        assert!(screen(&mut app).1.contains("Subtask 4 of 4"), "the last stays the last");
        key(&mut app, KeyCode::Left);
        key(&mut app, KeyCode::Left);
        let (_, shown) = screen(&mut app);
        assert!(shown.contains("Subtask 2 of 4") && shown.contains("→ subtask 3 · r"), "{shown}");
    }

    #[test]
    fn a_subtask_with_an_obstacle_shows_it_first_and_enter_answers_it_when_it_waits_for_you() {
        let (mut app, _) = two_tasks();
        let obstacle: Obstacle = serde_json::from_value(serde_json::json!({
            "id": "o3", "at": 10, "source": "task", "text": "The gateway has no refund call, so refunds cannot go through it.", "on": "Add Stripe as the gateway", "state": "asked",
            "question": "Add a refund call before this one?",
            "decision": { "kind": "ask", "by": "investigator", "at": 12, "reason": "two ways", "plan": "Add the refund call to the gateway first, then switch." },
        })).unwrap();
        let held_obstacle = obstacle.clone();
        app.subtask_io.load = Rc::new(move |_, _, _| Loaded { files: files(), built: Vec::new(), obstacle: Some(held_obstacle.clone()) });
        let mut question = crate::messages::fold(QUESTION).remove(0);
        question.from = "kebbie".to_string();
        question.about = Some(crate::obstacle::question_about("stripe@billing", "o3"));
        app.questions = vec![question];
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        let (rows, shown) = screen(&mut app);
        let at = find(&rows, "OBSTACLE o3  waits for you")[0].1;
        assert!(at < find(&rows, "COMMITS")[0].1 && shown.contains("obstacle · 1 commit · 2 files · +102 −12"), "the obstacle comes first: {shown}");
        assert!(rows[at + 1].contains("The gateway has no refund call") && shown.contains("Investigator: Add the refund call to the gateway first, then switch. Asks you: Add a refund call before this one?"), "{shown}");
        assert!(shown.contains("⏎ answers · ← → subtask 2 or 4 · esc back to the board"), "{shown}");
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Questions(_)), "enter answers it in the questions window");
    }

    #[test]
    fn esc_returns_to_the_same_subtask_row_with_its_new_state() {
        let (mut app, accepted) = two_tasks();
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('a'));
        assert!(matches!(app.mode, Mode::Subtask(_)), "accepting keeps the view open");
        assert_eq!(accepted.borrow().as_slice(), ["/work/wt1.billing stripe s3"]);
        let (_, shown) = screen(&mut app);
        assert!(shown.contains("2 to accept"), "{shown}");
        holds(&mut app.tasks[0].subtasks.items[1], &["3333333cccc"], true);
        key(&mut app, KeyCode::Esc);
        let (rows, shown) = screen(&mut app);
        assert!(matches!(app.mode, Mode::Normal) && app.work_selected == 0, "{shown}");
        let row = bar_row(&rows, "3 Add Stripe as the gateway");
        assert!(row.contains('▌') && row.contains("✓ 3") && row.contains("accepted · 1 commit"), "{shown}");
        assert!(shown.contains("opens subtask 3") && shown.contains("1 to accept") && !shown.contains("2 to accept"), "{shown}");
    }

    #[test]
    fn a_on_a_subtask_row_accepts_that_subtask_and_r_opens_the_review_on_it() {
        let (mut app, accepted) = two_tasks();
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Char('a'));
        assert_eq!(accepted.borrow().as_slice(), ["/work/wt1.billing stripe s3"], "the subtask under the cursor, not the first that waits");
        assert!(app.subtask_row.is_some(), "the cursor stays on the row");
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('a'));
        assert_eq!(accepted.borrow().len(), 1, "on the task's row a is the task's own key");
    }

    #[test]
    fn a_commit_body_is_what_was_built_without_its_trailers() {
        assert_eq!(body("Split notes\n\nnotes.py held three things;\neach has its own module now.\n\nClaude-Session: https://example.com\n"), ["notes.py held three things; each has its own module now."]);
        assert!(body("Only a subject").is_empty());
        let mut planned = step(0, "Later", SubtaskState::Planned);
        assert_eq!(range_of(&planned), None);
        holds(&mut planned, &["aaa", "bbb"], false);
        assert_eq!(range_of(&planned), Some(Range { from: "aaa^".to_string(), to: "bbb".to_string() }));
        planned.contiguous = false;
        assert_eq!(range_of(&planned), Some(Range::commit("bbb")));
    }
}
