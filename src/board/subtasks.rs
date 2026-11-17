use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use ratatui::Frame;

use crate::keys::{self, Act};
use crate::obstacle::{Kind, Obstacle, State};
use crate::peers;
use crate::sh::Result;
use crate::status::Item;
use crate::tasks::{Subtask, SubtaskState};
use super::beside::section;
use super::review_data::{self as data, File, Range};
use super::steps::{mark_color, walked};
use super::widgets::{GREEN, MANTLE, MAUVE, MUTED, PEACH, RED, TEAL, TEXT, bold, centered, cut, fit, plural, rule, style, text_width, wrap};
use super::{App, Column, Mode};

// What a subtask's card reads beyond the board's own item: the files of its diff with their read marks, what its commits say past their subjects, and an obstacle open on it.
#[derive(Clone, Debug, Default)]
pub(super) struct Loaded {
    pub files: Vec<File>,
    pub built: Vec<String>,
    pub obstacle: Option<Obstacle>,
}

pub(super) type Load = Rc<dyn Fn(&Path, &str, &Subtask) -> Loaded>;
pub(super) type Count = Rc<dyn Fn(&Path, &Subtask) -> Option<(usize, usize)>>;
pub(super) type Accept = Rc<dyn Fn(&Path, &str, &str) -> Result<String>>;

pub(super) struct Io {
    pub load: Load,
    pub count: Count,
    pub accept: Accept,
}

impl Default for Io {
    fn default() -> Self {
        Io { load: Rc::new(load), count: Rc::new(read_count), accept: Rc::new(|path, name, id| crate::tasks::accept(path, Some(name), None, Some(id), true)) }
    }
}

// The subtask card window: which subtask of which task, what was read for it, and how far it is scrolled.
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

pub(super) fn load(path: &Path, address: &str, subtask: &Subtask) -> Loaded {
    let files = range_of(subtask).map(|range| data::range_files(path, &range)).unwrap_or_default();
    let built = subtask.commits.iter().flat_map(|commit| body(&data::message(path, &commit.sha))).collect();
    let obstacle = crate::lead::state_dir().ok().and_then(|state| crate::obstacle::load(&state, address).into_iter().rev().find(|obstacle| obstacle.state.open() && obstacle.on == subtask.title));
    Loaded { files, built, obstacle }
}

fn counted(files: &[File]) -> Option<(usize, usize)> {
    (!files.is_empty()).then(|| (files.iter().filter(|file| file.read).count(), files.len()))
}

// How many of the subtask's files are marked read in the review, of how many.
fn read_count(path: &Path, subtask: &Subtask) -> Option<(usize, usize)> {
    counted(&data::range_files(path, &range_of(subtask)?))
}

pub(super) fn address_of(item: &Item) -> String {
    peers::address(&item.name, &item.place)
}

type Key = (String, String);
type Request = (PathBuf, Key, Subtask);
type Answer = (Key, Option<(usize, usize)>);

// Each subtask row's read count, read off the UI thread; a row shows the last count while a new one is read.
#[derive(Default)]
pub(super) struct Reads {
    counts: HashMap<Key, Option<(usize, usize)>>,
    asked: HashMap<Key, (Option<String>, Instant)>,
    requests: Option<Sender<Request>>,
    results: Option<Receiver<Answer>>,
}

impl Reads {
    pub(super) fn spawn(&mut self) {
        let (request_tx, request_rx) = mpsc::channel::<Request>();
        let (answer_tx, answer_rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok((path, key, subtask)) = request_rx.recv() {
                if answer_tx.send((key, read_count(&path, &subtask))).is_err() {
                    return;
                }
            }
        });
        (self.requests, self.results) = (Some(request_tx), Some(answer_rx));
    }

    pub(super) fn take(&mut self) -> bool {
        let Some(results) = &self.results else { return false };
        let mut changed = false;
        while let Ok((key, count)) = results.try_recv() {
            changed |= self.counts.insert(key, count) != Some(count);
        }
        changed
    }
}

fn waits_for_you(obstacle: &Obstacle) -> bool {
    obstacle.state == State::Asked || obstacle.decision.as_ref().is_some_and(|decision| decision.kind == Kind::Ask)
}

fn count(added: &str) -> usize {
    added.parse().unwrap_or_default()
}

fn label(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), style(MUTED))
}

fn key(text: String) -> Span<'static> {
    Span::styled(text, style(MAUVE))
}

// The keys line under the board while a subtask row is highlighted.
pub(super) fn inside_hint(accept: bool) -> Line<'static> {
    let mut spans = vec![key("↑ ↓".to_string()), label(" subtask · "), key("⏎".to_string()), label(" options · "), key(keys::key(Act::Card).to_string()), label(" card · "), key(keys::key(Act::Review).to_string()), label(" review · ")];
    if accept {
        spans.extend([key(keys::key(Act::Permit).to_string()), label(" accept · ")]);
    }
    spans.extend([key("← esc".to_string()), label(" back")]);
    Line::from(spans)
}

// The keys under a task's subtask rows while one is highlighted, in place of the task's own key line.
pub(super) fn row_hint(accept: bool) -> Line<'static> {
    let mut spans = vec![Span::raw("   "), key("⏎".to_string()), label(" options · "), key(keys::key(Act::Card).to_string()), label(" card · "), key(keys::key(Act::Review).to_string()), label(" review · ")];
    if accept {
        spans.extend([key(keys::key(Act::Permit).to_string()), label(" accept · ")]);
    }
    spans.extend([key("←".to_string()), label(" back")]);
    Line::from(spans).style(Style::new().bg(super::widgets::SURFACE))
}

// What ⏎ offers on a highlighted subtask, in the task menu's shape.
pub(super) fn subtask_choices(item: &Item, step: &Subtask) -> Vec<super::widgets::Choice> {
    use super::widgets::choice;
    let mut choices = vec![
        choice(Act::Card, "Show the card", "what it does, its commits and its files, ← → the other subtasks"),
        choice(Act::Review, "Open the review", "its diff in the review split next to its Claude"),
    ];
    choices.push(choice(Act::Chat, "Message its Claude", "send it a line without leaving"));
    if super::steps::accept_matters(item, step) && step.state == SubtaskState::Ready {
        choices.push(choice(Act::Permit, "Accept", "mark it reviewed, so a push can take it ahead of the subtasks still being built"));
    }
    choices
}

pub(super) fn draw_options(frame: &mut Frame, app: &App, index: usize) -> Vec<(Rect, super::Hit)> {
    let Some(item) = app.selected_task() else { return Vec::new() };
    let Some(at) = app.subtask_at(&item) else { return Vec::new() };
    let step = &walked(&item)[at];
    let group = |act: Option<Act>| if matches!(act, Some(Act::Permit)) { "SUBTASK" } else { "SESSION" };
    let (lines, rows) = super::widgets::menu_lines(&subtask_choices(&item, step), index, 48, |_| false, group);
    let area = super::widgets::modal(frame, format!("{} {}", step.number, step.title.chars().take(42).collect::<String>()), lines, 52);
    rows.into_iter().enumerate().map(|(choice, line)| (Rect::new(area.x, area.y + 1 + line as u16, area.width, 1), super::Hit::Choice(choice))).filter(|(rect, _)| rect.y < area.bottom().saturating_sub(1)).collect()
}

pub(super) fn read_text((read, total): (usize, usize)) -> String {
    format!("{read}/{total} read")
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
    spans.push(label(&format!(" · {}", plural(step.commits.len(), "commit"))));
    if let Some(read) = counted(files) {
        spans.push(label(&format!(" · {} · ", read_text(read))));
        spans.extend([Span::styled(format!("+{added}"), style(GREEN)), Span::raw(" "), Span::styled(format!("−{deleted}"), style(RED))]);
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
    let (mark, color) = if file.read { ("●", GREEN) } else { ("○", MUTED) };
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

// The subtask's card: its state line, its title, what its commits say it does, its commits and its files with their read marks.
pub(super) fn subtask_card(step: &Subtask, loaded: &Loaded, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![facts(step, loaded), Line::default(), Line::from(Span::styled(step.title.clone(), bold(TEXT)))];
    if let Some(obstacle) = &loaded.obstacle {
        lines.push(Line::default());
        lines.extend(obstacle_lines(obstacle, width));
    }
    if !loaded.built.is_empty() {
        lines.extend([Line::default(), section("WHAT IT DOES")]);
        for paragraph in &loaded.built {
            lines.extend(wrap(paragraph, width.saturating_sub(2), 12).into_iter().map(|text| Line::from(vec![Span::raw("  "), Span::styled(text, style(TEXT))])));
        }
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
    lines
}

// The card window's frame, the same for a task's card and a subtask's: its title, and the areas for its lines and, under a rule, its keys.
pub(super) fn card_window(frame: &mut Frame, title: String) -> (Rect, Rect, Rect) {
    let full = frame.area();
    let area = centered(full, (full.width * 92 / 100).max(40), (full.height * 84 / 100).max(12));
    frame.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), bold(MAUVE))).style(Style::new().bg(MANTLE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [body, line, foot] = Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    frame.render_widget(Paragraph::new(rule(line.width as usize)), line);
    (body, line, foot)
}

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let Mode::SubtaskCard(view) = &app.mode else { return };
    let Some(item) = app.tasks.iter().find(|item| address_of(item) == view.address) else { return };
    let steps = walked(item);
    let Some(at) = steps.iter().position(|step| step.id == view.id) else { return };
    let total = item.subtasks.pushed.len() + item.subtasks.items.len();
    let (body, _, foot) = card_window(frame, format!("▸ {} › {} of {total}", cut(&item.titled(), 60), steps[at].number));
    let lines = subtask_card(&steps[at], &view.loaded, body.width as usize);
    let scroll = view.scroll.min(lines.len().saturating_sub(body.height as usize) as u16);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
    let accept = if super::steps::accept_matters(item, &steps[at]) { format!(" · {} accept", keys::key(Act::Permit)) } else { String::new() };
    let keys = format!("← → subtask · ↑ ↓ scroll · {} review{accept} · {} close", keys::key(Act::Review), keys::map().label(Act::Back));
    frame.render_widget(Paragraph::new(fit(vec![Span::styled(keys, style(MUTED))], foot.width as usize)), foot);
    if let Mode::SubtaskCard(view) = &mut app.mode {
        view.scroll = scroll;
    }
}

impl App {
    // Where the cursor is among the task's subtask rows, while it is inside the selected task.
    pub(super) fn subtask_at(&self, item: &Item) -> Option<usize> {
        let (address, id) = self.subtask_row.as_ref()?;
        let selected = self.selected_task()?;
        (matches!(self.mode, Mode::Subtasks | Mode::SubtaskOptions(_)) && self.column == Column::Work && address_of(&selected) == *address && address_of(item) == *address).then_some(())?;
        walked(item).iter().position(|step| step.id == *id)
    }

    // Inside a task the keys at the bottom are the subtask's.
    pub(super) fn subtask_keys(&self) -> Option<Line<'static>> {
        let item = self.selected_task()?;
        self.subtask_at(&item).map(|at| inside_hint(super::steps::accept_matters(&item, &walked(&item)[at])))
    }

    // The task under the cursor has subtask rows to step into, and the cursor is on the task itself.
    pub(super) fn selected_steps(&self) -> bool {
        matches!(self.mode, Mode::Normal) && self.column == Column::Work && !self.offer_panel.focused && !self.comment_panel.focused && !self.on_parked_line() && self.selected_task().is_some_and(|item| !walked(&item).is_empty())
    }

    // The read count of each of the task's subtask rows, as far as it is read yet.
    pub(super) fn subtask_reads(&mut self, item: &Item) -> Vec<Option<(usize, usize)>> {
        let Some(path) = item.path.clone() else { return vec![None; walked(item).len()] };
        let address = address_of(item);
        walked(item).to_vec().into_iter().map(|step| {
            if step.commits.is_empty() {
                return None;
            }
            let key = (address.clone(), step.id.clone());
            let Some(requests) = &self.reads.requests else { return (self.subtask_io.count)(&path, &step) };
            let head = step.head().map(str::to_string);
            let fresh = self.reads.asked.get(&key).is_some_and(|(asked, at)| *asked == head && at.elapsed() < Duration::from_secs(4));
            if !fresh {
                let _ = requests.send((path.clone(), key.clone(), step.clone()));
                self.reads.asked.insert(key.clone(), (head, Instant::now()));
            }
            self.reads.counts.get(&key).copied().flatten()
        }).collect()
    }

    fn put_cursor(&mut self, item: &Item, step: &Subtask) {
        self.subtask_row = Some((address_of(item), step.id.clone()));
    }

    // → on a task with subtask rows: the cursor goes onto them, on the first that waits for you.
    pub(super) fn open_subtasks(&mut self, item: &Item) {
        let steps = walked(item);
        let Some(first) = steps.iter().find(|step| step.state == SubtaskState::Ready).or_else(|| steps.first()).cloned() else { return };
        self.put_cursor(item, &first);
        self.mode = Mode::Subtasks;
    }

    // Inside the task ↑ ↓ stay among its subtasks; ← or esc go back to the task.
    pub(super) fn handle_subtasks(&mut self, act: Option<Act>, key: &KeyEvent) {
        let Some(item) = self.selected_task() else { return self.subtask_row = None };
        let steps = walked(&item).to_vec();
        self.mode = Mode::Subtasks;
        let Some(at) = self.subtask_at(&item) else {
            self.mode = Mode::Normal;
            return self.subtask_row = None;
        };
        match act {
            Some(Act::Move) => {
                let to = if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) { at.saturating_sub(1) } else { (at + 1).min(steps.len() - 1) };
                self.put_cursor(&item, &steps[to]);
            }
            Some(Act::Card) => self.open_subtask(&item, &steps[at]),
            Some(Act::Options) => self.mode = Mode::SubtaskOptions(0),
            Some(Act::Permit) => self.accept_subtask(&item, &steps[at], Mode::Subtasks),
            Some(Act::Review) => self.open_review_at(&item, Some(steps[at].number)),
            Some(Act::Back | Act::Close) => self.leave_subtasks(),
            Some(Act::Switch) if key.code == KeyCode::Left => self.leave_subtasks(),
            _ => {}
        }
    }

    fn leave_subtasks(&mut self) {
        self.subtask_row = None;
        self.mode = Mode::Normal;
    }

    fn open_subtask(&mut self, item: &Item, step: &Subtask) {
        let Some(path) = item.path.clone() else { return };
        let address = address_of(item);
        let loaded = (self.subtask_io.load)(&path, &address, step);
        self.mode = Mode::SubtaskCard(Box::new(View { address, id: step.id.clone(), loaded, scroll: 0 }));
    }

    fn accept_subtask(&mut self, item: &Item, step: &Subtask, back: Mode) {
        let Some(path) = item.path.clone() else { return };
        self.about("accept", &item.name);
        let result = (self.subtask_io.accept)(&path, &item.name, &step.id);
        self.show(result);
        if matches!(self.mode, Mode::Normal) {
            self.mode = back;
        }
    }

    // The subtask's menu: a choice's key or ⏎ on it runs it on that subtask, esc goes back to its row.
    pub(super) fn handle_subtask_options(&mut self, index: usize, act: Option<Act>, key: KeyEvent) {
        let Some(item) = self.selected_task() else { return self.leave_subtasks() };
        self.mode = Mode::Subtasks;
        let Some(at) = self.subtask_at(&item) else { return self.leave_subtasks() };
        let step = walked(&item)[at].clone();
        let choices = subtask_choices(&item, &step);
        let typed = match key.code {
            KeyCode::Char(character) if character != ' ' => choices.iter().find(|choice| choice.key == character),
            _ => None,
        };
        let chosen = match (typed, act) {
            (Some(chosen), _) => chosen.act,
            (None, Some(Act::Options)) => choices.get(index).and_then(|choice| choice.act),
            (None, Some(Act::Move)) => {
                let to = if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) { index.saturating_sub(1) } else { (index + 1).min(choices.len().saturating_sub(1)) };
                return self.mode = Mode::SubtaskOptions(to);
            }
            (None, Some(Act::Back | Act::Close)) => return,
            _ => return self.mode = Mode::SubtaskOptions(index),
        };
        match chosen {
            Some(Act::Card) => self.open_subtask(&item, &step),
            Some(Act::Review) => self.open_review_at(&item, Some(step.number)),
            Some(Act::Permit) => self.accept_subtask(&item, &step, Mode::Subtasks),
            Some(Act::Chat) => {
                self.leave_subtasks();
                self.act(Act::Chat);
            }
            _ => {}
        }
    }

    // The subtask's card window: ← → the task's other subtasks, r reads its diff, a accepts it, esc goes back to its row.
    pub(super) fn handle_subtask_card(&mut self, mut view: Box<View>, key: KeyEvent, act: Option<Act>) {
        let Some(item) = self.tasks.iter().find(|item| address_of(item) == view.address).cloned() else { return self.leave_subtasks() };
        let steps = walked(&item).to_vec();
        let Some(at) = steps.iter().position(|step| step.id == view.id) else { return self.leave_subtasks() };
        match act {
            Some(Act::Back | Act::Close | Act::Card) => self.mode = Mode::Subtasks,
            Some(Act::Switch) => {
                let to = if key.code == KeyCode::Left { at.saturating_sub(1) } else { (at + 1).min(steps.len() - 1) };
                self.put_cursor(&item, &steps[to]);
                if to == at {
                    return self.mode = Mode::SubtaskCard(view);
                }
                self.open_subtask(&item, &steps[to]);
            }
            Some(Act::Move) => {
                view.scroll = if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) { view.scroll.saturating_sub(1) } else { view.scroll.saturating_add(1) };
                self.mode = Mode::SubtaskCard(view);
            }
            Some(Act::Permit) => {
                let step = steps[at].clone();
                self.accept_subtask(&item, &step, Mode::SubtaskCard(view));
            }
            Some(Act::Review) => {
                self.mode = Mode::SubtaskCard(view);
                self.open_review_at(&item, Some(steps[at].number));
            }
            _ => {
                match key.code {
                    KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(10),
                    KeyCode::PageDown | KeyCode::Char(' ') => view.scroll = view.scroll.saturating_add(10),
                    _ => {}
                }
                self.mode = Mode::SubtaskCard(view);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::board::steps::tests::{held, holds, step, stepped, stripe};
    use crate::board::test_support::*;
    use crate::tasks::Subtasks;

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
            count: Rc::new(|_, step| (!step.commits.is_empty()).then_some((1, 2))),
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
    fn a_task_of_one_subtask_shows_no_subtask_row_and_right_does_not_step_in() {
        let only = Subtasks { items: vec![held(0, "Charge through a PaymentGateway", &["2222222bbbb"], false)], ..Subtasks::default() };
        let mut app = test_app(vec![worked(stepped(only))]);
        let (_, shown) = screen(&mut app);
        assert!(!shown.contains("1 Charge through a PaymentGateway"), "{shown}");
        assert!(shown.contains("Switch the payment provider to Stripe") && !shown.contains("to accept"), "the row stays, and a lone subtask has nothing for accepting to put ahead: {shown}");
        key(&mut app, KeyCode::Right);
        assert!(app.subtask_row.is_none() && !matches!(app.mode, Mode::Subtasks), "no row to step into");
        let mut two = stripe();
        two.items.truncate(1);
        let mut app = test_app(vec![worked(stepped(two))]);
        let (_, shown) = screen(&mut app);
        assert!(shown.contains("↑ 1 pushed") && shown.contains("2 Charge through a PaymentGateway"), "one pushed and one open are two: {shown}");
    }

    #[test]
    fn up_and_down_move_task_to_task_and_never_stop_on_a_subtask_row() {
        let (mut app, _) = two_tasks();
        assert!(screen(&mut app).1.contains("↑ ↓ task · → subtasks · s card · ⏎ options"), "{}", screen(&mut app).1);
        key(&mut app, KeyCode::Down);
        assert!(app.work_selected == 1 && app.subtask_row.is_none(), "down goes to the next task");
        key(&mut app, KeyCode::Char('k'));
        assert!(app.work_selected == 0 && app.subtask_row.is_none(), "up goes back to the task, not onto its last subtask");
    }

    #[test]
    fn right_marks_a_subtask_row_on_the_board_without_fading_it_and_left_or_esc_goes_back() {
        let (mut app, _) = two_tasks();
        key(&mut app, KeyCode::Right);
        assert!(matches!(app.mode, Mode::Subtasks));
        let (rows, buffer) = render(&mut app, 120, 40);
        let shown = rows.join("\n");
        let row = bar_row(&rows, "2 Charge through a PaymentGateway");
        let (x, y) = find(&rows, "2 Charge through a PaymentGateway")[0];
        assert_eq!(buffer[(x as u16, y as u16)].bg, crate::board::widgets::RAISED, "the cursor starts on the first that waits for you, lit: {shown}");
        assert!(row.contains("▌│▌ ● 2 Charge"), "a mark beside the highlighted subtask: {shown}");
        let (task_x, task_y) = find(&rows, "search")[0];
        assert_ne!(buffer[(task_x as u16, task_y as u16)].fg, crate::board::widgets::FAINT, "nothing fades: {shown}");
        assert!(rows.iter().any(|row| row.contains("CARD   Switch the payment provider to Stripe ›")) && rows.iter().any(|row| row.contains("WHAT IT DOES")) && !shown.contains("SUBTASKS"), "the column is the subtask's card: {shown}");
        assert!(shown.contains("↑ ↓ subtask · ⏎ options · s card · r review · a accept · ← esc back"), "{shown}");
        assert!(rows.iter().any(|row| row.contains("╰ ⏎ options · s card · r review · a accept · ← back")), "the task's key line under its rows gives the subtask's keys: {shown}");
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Down);
        assert_eq!((app.subtask_row.clone().unwrap().1, app.work_selected), ("s4".to_string(), 0), "down steps the subtasks, the last stays the last");
        key(&mut app, KeyCode::Up);
        assert_eq!(app.subtask_row.clone().unwrap().1, "s3");
        key(&mut app, KeyCode::Left);
        assert!(matches!(app.mode, Mode::Normal) && app.subtask_row.is_none() && app.work_selected == 0, "← goes back to the task");
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal) && app.subtask_row.is_none(), "esc goes back too");
    }

    #[test]
    fn on_a_narrow_board_right_steps_into_the_rows_the_same_way() {
        let (mut app, _) = two_tasks();
        key(&mut app, KeyCode::Right);
        let (rows, buffer) = render(&mut app, 80, 40);
        let (x, y) = find(&rows, "2 Charge through a PaymentGateway")[0];
        assert_eq!(buffer[(x as u16, y as u16)].bg, crate::board::widgets::RAISED, "{}", rows.join("\n"));
        assert!(!rows.join("\n").contains("CARD"), "no card column under 86: {}", rows.join("\n"));
    }

    #[test]
    fn enter_inside_a_task_opens_the_subtasks_own_options_and_the_tasks_have_no_subtask_entry() {
        let (mut app, accepted) = two_tasks();
        key(&mut app, KeyCode::Enter);
        assert!(app.choices().iter().all(|choice| choice.label != "Its subtasks"), "no subtask entry in the options");
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::SubtaskOptions(0)), "⏎ opens the subtask's options");
        let (_, shown) = screen(&mut app);
        for line in ["╭ 3 Add Stripe as the gateway", "│ SESSION", "▸ Show the card", "Open the review", "Message its Claude", "│ SUBTASK", "Accept"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        let keys: Vec<char> = subtask_choices(&app.tasks[0], &walked(&app.tasks[0])[1]).iter().map(|choice| choice.key).collect();
        assert_eq!(keys, ['s', 'r', 'c', 'a'], "the keys the subtask's rows already take");
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Subtasks) && app.subtask_row.clone().unwrap().1 == "s3", "esc closes the menu on its row");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('a'));
        assert_eq!(accepted.borrow().as_slice(), ["/work/wt1.billing stripe s3"]);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('s'));
        assert!(matches!(&app.mode, Mode::SubtaskCard(view) if view.id == "s3"), "s in the menu opens its card");
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert!(app.choices().is_empty() || !matches!(app.mode, Mode::Options(_)), "the task's own menu stays shut");
        let (_, shown) = screen(&mut app);
        assert!(!shown.contains("│ SUBTASK"), "a subtask that is not ready has nothing to accept: {shown}");
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('S'));
        assert!(matches!(app.mode, Mode::Subtasks), "S is no key");
    }

    #[test]
    fn left_and_right_in_a_subtasks_card_step_to_the_tasks_other_subtasks_and_the_board_follows() {
        let (mut app, _) = two_tasks();
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Char('s'));
        assert!(screen(&mut app).1.contains("› 2 of 4"), "{}", screen(&mut app).1);
        key(&mut app, KeyCode::Right);
        let (_, shown) = screen(&mut app);
        assert!(shown.contains("▸ Switch the payment provider to Stripe › 3 of 4") && shown.contains("Add Stripe as the gateway keeps the charge in one place.") && shown.contains("← → subtask"), "{shown}");
        assert!(matches!(&app.mode, Mode::SubtaskCard(view) if view.id == "s3") && app.subtask_row.clone().unwrap().1 == "s3", "the window stays open and the row under it follows");
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Right);
        assert!(screen(&mut app).1.contains("› 4 of 4"), "the last stays the last");
        key(&mut app, KeyCode::Left);
        assert!(screen(&mut app).1.contains("› 3 of 4"));
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Subtasks) && app.subtask_row.clone().unwrap().1 == "s3", "esc leaves the cursor where the window was");
    }

    #[test]
    fn s_on_a_subtask_opens_its_card_with_its_commits_and_files_and_a_there_accepts_it() {
        let (mut app, accepted) = two_tasks();
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Char('s'));
        assert!(matches!(app.mode, Mode::SubtaskCard(_)));
        let (rows, shown) = screen(&mut app);
        assert!(shown.contains("▸ Switch the payment provider to Stripe › 3 of 4") && shown.contains("Add Stripe as the gateway"), "{shown}");
        assert!(shown.contains("ready · 1 commit · 1/2 read · +102 −12"), "{shown}");
        let commits = find(&rows, "COMMITS")[0].1;
        assert!(rows[commits + 1].contains("3333333  subject of 3333333cccc"), "{shown}");
        let files = find(&rows, "FILES")[0].1;
        assert!(rows[files + 1].contains("● app/gateway.rb") && rows[files + 1].contains("+38") && rows[files + 1].contains("−12"), "{shown}");
        assert!(rows[files + 2].contains("○ app/stripe.rb") && !rows[files + 2].contains('−'), "{shown}");
        assert!(shown.contains("WHAT IT DOES") && shown.contains("Add Stripe as the gateway keeps the charge in one place."), "{shown}");
        assert!(shown.contains("↑ ↓ scroll · r review · a accept · esc close"), "{shown}");
        key(&mut app, KeyCode::Char('a'));
        assert_eq!(accepted.borrow().as_slice(), ["/work/wt1.billing stripe s3"]);
        assert!(matches!(app.mode, Mode::SubtaskCard(_)), "accepting keeps the card");
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Subtasks) && app.subtask_row.is_some(), "esc goes back to the row");
    }

    #[test]
    fn a_inside_the_task_accepts_the_subtask_under_the_cursor_and_on_the_task_row_stays_the_tasks() {
        let (mut app, accepted) = two_tasks();
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Char('a'));
        assert_eq!(accepted.borrow().as_slice(), ["/work/wt1.billing stripe s3"], "the subtask under the cursor, not the first that waits");
        assert!(matches!(app.mode, Mode::Subtasks), "the cursor stays on the row");
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('a'));
        assert_eq!(accepted.borrow().len(), 1, "on the task's row a is the task's own key");
    }

    #[test]
    fn a_commit_body_is_what_the_subtask_does_without_its_trailers() {
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
