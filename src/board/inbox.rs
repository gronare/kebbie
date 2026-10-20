use std::sync::mpsc::{self, Receiver};
use std::thread;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;
use ratatui_textarea::TextArea;

use crate::herdr;
use crate::keys::{self, Act};
use crate::messages::{self, Message};
use crate::peers;
use crate::sh::Result;
use crate::status::Item;
use crate::questions;
use super::{App, Hit, Mode};
use super::widgets::{BLUE, GREEN, MANTLE, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, TEXT, YELLOW, bold, centered, fill, fit, plural, style, wrap};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Part {
    List,
    Plan,
    Options,
    Comment,
}

impl Part {
    fn next(self, back: bool) -> Self {
        let order = [Part::List, Part::Plan, Part::Options, Part::Comment];
        let at = order.iter().position(|part| *part == self).unwrap_or(0);
        order[if back { (at + 3) % 4 } else { (at + 1) % 4 }]
    }
}

pub(super) struct QuestionWindow {
    id: String,
    selected: usize,
    pub(super) option: usize,
    pub(super) scroll: u16,
    pub(super) part: Part,
    comment: TextArea<'static>,
    notice: Option<(bool, String)>,
    answering: Option<(String, Receiver<Result<String>>)>,
}

fn comment_field() -> TextArea<'static> {
    let mut comment = TextArea::default();
    comment.set_placeholder_text("comment (optional)");
    comment.set_cursor_line_style(Style::new());
    comment.set_style(style(TEXT));
    comment
}

impl QuestionWindow {
    fn new(questions: &[Message], selected: usize) -> Self {
        let mut window = Self { id: String::new(), selected: 0, option: 0, scroll: 0, part: Part::Options, comment: comment_field(), notice: None, answering: None };
        window.select(questions, selected);
        window
    }

    // Another question never inherits this one's option or comment.
    pub(super) fn select(&mut self, questions: &[Message], selected: usize) {
        self.selected = selected.min(questions.len().saturating_sub(1));
        let message = questions.get(self.selected);
        let id = message.map(|message| message.id.clone()).unwrap_or_default();
        if id != self.id {
            self.comment = comment_field();
        }
        self.id = id;
        self.option = message.and_then(|message| message.question.as_ref()).map_or(0, |question| question.recommend.saturating_sub(1));
        self.scroll = 0;
    }

    // The list changes under the window as questions open and close; the selection follows its question, not its row.
    pub(super) fn follow(&mut self, questions: &[Message]) {
        match questions.iter().position(|message| message.id == self.id) {
            Some(at) => self.selected = at,
            None if self.id.is_empty() => self.select(questions, 0),
            None => {
                self.select(questions, self.selected);
                self.notice = Some((false, "The question you had open was answered or closed elsewhere; its choice and comment are cleared.".to_string()));
            }
        }
    }

    fn comment(&self) -> String {
        self.comment.lines().join("\n").trim().to_string()
    }
}

impl App {
    pub(super) fn question_of(&self, item: &Item) -> Option<usize> {
        let address = peers::address(&item.name, &item.place);
        self.questions.iter().position(|question| question.from == address)
    }

    fn task_of(&self, from: &str) -> Option<&Item> {
        self.tasks.iter().find(|item| peers::address(&item.name, &item.place) == from)
    }

    pub(super) fn open_questions(&mut self, at: Option<usize>) {
        if self.questions.is_empty() {
            self.mode = Mode::Message("No questions are waiting.".to_string());
            return;
        }
        self.mode = Mode::Questions(Box::new(QuestionWindow::new(&self.questions, at.unwrap_or(0))));
    }

    pub(super) fn handle_questions(&mut self, mut window: Box<QuestionWindow>, key: KeyEvent) {
        let count = self.questions.len();
        let options = self.questions.get(window.selected).and_then(|message| message.question.as_ref()).map_or(0, |question| question.options.len());
        let typing = window.part == Part::Comment;
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Char(character) if !typing && character == keys::key(Act::Close) => return,
            KeyCode::Tab | KeyCode::BackTab => window.part = window.part.next(key.code == KeyCode::BackTab),
            KeyCode::PageUp => window.scroll = window.scroll.saturating_sub(10),
            KeyCode::PageDown => window.scroll = window.scroll.saturating_add(10),
            KeyCode::Enter if window.answering.is_some() => {}
            KeyCode::Enter => {
                let Some(message) = self.questions.get(window.selected) else { return };
                let comment = window.comment();
                let choice = (window.option < options).then_some(window.option);
                if choice.is_none() && comment.is_empty() {
                    window.part = Part::Comment;
                    window.notice = Some((false, "In your own words: write the answer in the comment field first.".to_string()));
                } else {
                    let (sender, result) = mpsc::channel();
                    let id = message.id.clone();
                    let asked = id.clone();
                    thread::spawn(move || {
                        let _ = sender.send(questions::answer(&asked, choice, &comment));
                    });
                    window.answering = Some((id, result));
                    window.notice = Some((true, "Answering…".to_string()));
                }
            }
            KeyCode::Char('g') if !typing => {
                let workspace = self.questions.get(window.selected).and_then(|message| self.task_of(&message.from)).and_then(|item| item.workspace.clone());
                match workspace {
                    Some(workspace) => {
                        let _ = herdr::focus_workspace(&workspace);
                        if self.overlay {
                            self.quit = true;
                        }
                        return;
                    }
                    None => window.notice = Some((false, "Its task has no Space open.".to_string())),
                }
            }
            KeyCode::Up | KeyCode::Down if typing => {}
            KeyCode::Up | KeyCode::Char('k') if !typing => match window.part {
                Part::List => window.select(&self.questions, window.selected.saturating_sub(1)),
                Part::Plan => window.scroll = window.scroll.saturating_sub(1),
                _ => window.option = window.option.saturating_sub(1),
            },
            KeyCode::Down | KeyCode::Char('j') if !typing => match window.part {
                Part::List => window.select(&self.questions, (window.selected + 1).min(count.saturating_sub(1))),
                Part::Plan => window.scroll = window.scroll.saturating_add(1),
                _ => window.option = (window.option + 1).min(options),
            },
            _ => {
                if !typing && !matches!(key.code, KeyCode::Char(_)) {
                    self.mode = Mode::Questions(window);
                    return;
                }
                window.part = Part::Comment;
                window.comment.input(key);
            }
        }
        self.mode = Mode::Questions(window);
    }

    pub(super) fn poll_answer(&mut self) {
        let Mode::Questions(window) = &mut self.mode else { return };
        let Some((id, receiver)) = &window.answering else { return };
        let Ok(result) = receiver.try_recv() else { return };
        let id = id.clone();
        window.answering = None;
        let text = match result {
            Ok(text) => text,
            Err(error) => {
                window.notice = Some((false, error));
                return;
            }
        };
        self.questions.retain(|question| question.id != id);
        let _ = self.refresh.send(());
        if self.questions.is_empty() {
            self.mode = Mode::Message(text);
            return;
        }
        if let Mode::Questions(window) = &mut self.mode {
            let at = if window.id == id { window.selected } else { self.questions.iter().position(|question| question.id == window.id).unwrap_or(window.selected) };
            window.select(&self.questions, at);
            window.notice = Some((true, text));
        }
    }
}

#[derive(Clone)]
struct PlanStyle;

impl tui_markdown::StyleSheet for PlanStyle {
    fn heading(&self, level: u8) -> Style {
        if level <= 2 { bold(MAUVE) } else { bold(TEXT) }
    }

    fn code(&self) -> Style {
        style(PEACH)
    }

    fn link(&self) -> Style {
        style(BLUE).add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        style(SUBTEXT)
    }

    fn heading_meta(&self) -> Style {
        style(MUTED)
    }

    fn metadata_block(&self) -> Style {
        style(MUTED)
    }

    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }

    fn code_block_fence(&self) -> &str {
        ""
    }

    fn list_marker(&self) -> Style {
        style(MAUVE)
    }
}

pub fn markdown(text: &str) -> Text<'static> {
    let rendered = tui_markdown::from_str_with_options(text, &tui_markdown::Options::new(PlanStyle));
    Text::from(rendered.lines.into_iter().map(|line| {
        let spans: Vec<Span<'static>> = line.spans.into_iter().map(|span| Span::styled(span.content.into_owned(), span.style)).collect();
        let mut owned = Line::from(spans).style(line.style);
        owned.alignment = line.alignment;
        owned
    }).collect::<Vec<_>>())
}

const OWN_WORDS: &str = "In my own words";

fn split_address(address: &str) -> (String, String) {
    match address.split_once('@') {
        Some((name, place)) => (name.to_string(), place.to_string()),
        None => (address.to_string(), String::new()),
    }
}

pub(super) fn draw_questions(frame: &mut Frame, app: &mut App) -> Vec<(Rect, Hit)> {
    let mut hits = Vec::new();
    let full = frame.area();
    let area = centered(full, (full.width * 92 / 100).max(40), (full.height * 84 / 100).max(16));
    frame.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(" questions ", bold(MAUVE))).style(Style::new().bg(MANTLE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let selected = match &app.mode { Mode::Questions(window) => window.selected, _ => return hits };
    let plan = app.questions.get(selected).cloned().and_then(|message| {
        let question = message.question.clone()?;
        if !app.plans.contains_key(&message.id) {
            app.plans.insert(message.id.clone(), markdown(&questions::plan_text(&question)?));
        }
        app.plans.get(&message.id).cloned()
    });
    let questions = app.questions.clone();
    let Mode::Questions(window) = &mut app.mode else { return hits };
    let [title_area, _, body, _, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(4), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    let mut title = vec![Span::styled(format!("{} waiting · answered here, the lead gets a copy", questions.len()), style(MUTED))];
    if let Some((ok, notice)) = &window.notice {
        title.push(Span::styled(format!("   {} {}", if *ok { "✓" } else { "!" }, notice.split_whitespace().collect::<Vec<_>>().join(" ")), style(if *ok { GREEN } else { RED })));
    }
    frame.render_widget(Paragraph::new(fit(title, title_area.width as usize)), title_area);
    let wide = body.width >= 90;
    let (list_area, main) = if wide {
        let [list, _, main] = Layout::horizontal([Constraint::Percentage(30), Constraint::Length(2), Constraint::Min(20)]).areas(body);
        (list, main)
    } else {
        let height = (questions.len() as u16 * 2).min(body.height / 3).max(2);
        let [list, _, main] = Layout::vertical([Constraint::Length(height), Constraint::Length(1), Constraint::Min(3)]).areas(body);
        (list, main)
    };
    let list_block = if wide { Block::new().borders(Borders::RIGHT).border_style(style(SURFACE)) } else { Block::new().borders(Borders::BOTTOM).border_style(style(SURFACE)) };
    let list_inner = list_block.inner(list_area);
    frame.render_widget(list_block, list_area);
    let mut list = Vec::new();
    for (index, message) in questions.iter().enumerate() {
        let active = index == window.selected;
        let (name, place) = split_address(&message.from);
        let kind = if message.question.as_ref().is_some_and(|question| question.plan.is_some()) { "plan" } else { "ask " };
        let background = if active && window.part == Part::List { Style::new().bg(SURFACE) } else { Style::new() };
        list.push(Line::from(vec![Span::raw(if active { "▸ " } else { "  " }), Span::styled(format!("{kind}  "), style(YELLOW)), Span::styled(name, if active { bold(TEXT) } else { style(TEXT) })]).style(background));
        list.push(Line::from(Span::styled(format!("        {place} · {}", messages::age(message.at)), style(MUTED))).style(background));
        let y = list_inner.y + 2 * index as u16;
        if y + 1 < list_inner.bottom() {
            hits.push((Rect::new(list_inner.x, y, list_inner.width, 2), Hit::Question(index)));
        }
    }
    frame.render_widget(Paragraph::new(fill(list, list_inner.width as usize)), list_inner);
    let Some(message) = questions.get(window.selected) else { return hits };
    let mut options = message.question.as_ref().map(|question| question.options.clone()).unwrap_or_default();
    options.push((OWN_WORDS.to_string(), "the comment is the whole answer".to_string()));
    let recommend = message.question.as_ref().map_or(0, |question| question.recommend);
    let asked = wrap(&message.text, main.width as usize, 3);
    let plan_height = if plan.is_some() { Constraint::Min(4) } else { Constraint::Length(0) };
    let [asked_area, _, plan_area, _, options_area, _, comment_area] = Layout::vertical([
        Constraint::Length(asked.len() as u16), Constraint::Length(1), plan_height, Constraint::Length(u16::from(plan.is_some())),
        Constraint::Length(options.len() as u16), Constraint::Length(1), Constraint::Length(3),
    ]).areas(main);
    frame.render_widget(Paragraph::new(asked.into_iter().map(|line| Line::from(Span::styled(line, bold(TEXT)))).collect::<Vec<_>>()), asked_area);
    if let Some(plan) = plan {
        let paragraph = Paragraph::new(plan).wrap(Wrap { trim: false });
        let mut source = vec![Span::styled(format!(" {} ", message.question.as_ref().and_then(|question| question.plan.clone()).unwrap_or_default()), style(MUTED))];
        if message.question.as_ref().is_some_and(questions::plan_changed) {
            source.push(Span::styled("changed since asked ", style(YELLOW)));
        }
        let box_block = Block::bordered().border_type(BorderType::Rounded).border_style(style(if window.part == Part::Plan { MAUVE } else { SURFACE })).padding(Padding::horizontal(1))
            .title(Line::from(source));
        let plan_inner = box_block.inner(plan_area);
        let total = paragraph.line_count(plan_inner.width) as u16;
        window.scroll = window.scroll.min(total.saturating_sub(plan_inner.height));
        let below = total.saturating_sub(plan_inner.height + window.scroll);
        let box_block = if below > 0 { box_block.title_bottom(Span::styled(format!(" ↓ {} · pgdn ", plural(below as usize, "more line")), style(MUTED))) } else { box_block };
        frame.render_widget(paragraph.scroll((window.scroll, 0)).block(box_block), plan_area);
    }
    let mut option_lines = Vec::new();
    for (index, (label, meaning)) in options.iter().enumerate() {
        let active = index == window.option;
        let color = if label.starts_with("Reject") { RED } else if index + 1 == recommend { GREEN } else { TEXT };
        let about = if index + 1 == recommend { format!("recommended · {meaning}") } else { meaning.clone() };
        let background = if active && window.part == Part::Options { Style::new().bg(SURFACE) } else { Style::new() };
        option_lines.push(fit(vec![
            Span::raw(if active { "▸ " } else { "  " }), Span::styled(label.clone(), if active { style(color).add_modifier(Modifier::BOLD) } else { style(color) }),
            Span::raw("  "), Span::styled(about, style(MUTED)),
        ], options_area.width as usize).style(background));
        let y = options_area.y + index as u16;
        if y < options_area.bottom() {
            hits.push((Rect::new(options_area.x, y, options_area.width, 1), Hit::Answer(index)));
        }
    }
    frame.render_widget(Paragraph::new(fill(option_lines, options_area.width as usize)), options_area);
    let typing = window.part == Part::Comment;
    window.comment.set_block(Block::bordered().border_type(BorderType::Rounded).border_style(style(if typing { MAUVE } else { SURFACE })));
    window.comment.set_cursor_style(if typing { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() });
    frame.render_widget(&window.comment, comment_area);
    let keys = "↑ ↓ choose · tab questions / plan / options / comment · pgup pgdn plan · ⏎ answer · g go to its Space · esc close";
    frame.render_widget(Paragraph::new(fit(vec![Span::styled(keys, style(MUTED))], foot.width as usize)), foot);
    hits
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    use ratatui::widgets::Paragraph;

    use super::*;
    use crate::board::test_support::*;
    use crate::messages;
    use crate::report;
    use crate::board::card::card_lines;
    use crate::board::{Hit, Mode};

    #[test]
    fn the_question_window_lists_every_question_and_shows_the_selected_plan_as_markdown() {
        let mut app = test_app(Vec::new());
        app.questions = messages::fold(&[QUESTION, PLAN].join("\n"));
        app.plans.insert("q2".to_string(), markdown(&format!("## Problem\n\nThe payload says *amount*.\n\n- one release per repo\n- the fallback goes\n\n{}", "more\n\n".repeat(30))));
        app.open_questions(Some(1));
        let (rows, _) = render(&mut app, 170, 44);
        let screen = rows.join("\n");
        assert!(screen.contains("2 waiting · answered here, the lead gets a copy"), "{screen}");
        assert!(screen.contains("ask   billing") && screen.contains("▸ plan  payload"), "{screen}");
        assert!(screen.contains("Problem") && !screen.contains("## Problem") && screen.contains("The payload says amount."), "{screen}");
        assert!(screen.contains("one release per repo") && screen.contains("more lines · pgdn"), "{screen}");
        assert!(screen.contains("▸ Approve  recommended · build it as planned") && screen.contains("Reject with a comment"), "{screen}");
        assert!(screen.contains("comment (optional)"), "{screen}");
        let questions = app.hits.iter().filter(|(_, hit)| matches!(hit, Hit::Question(_))).count();
        let answers = app.hits.iter().filter(|(_, hit)| matches!(hit, Hit::Answer(_))).count();
        assert_eq!((questions, answers), (2, 5), "four options and in my own words");
        assert!(screen.contains(" /state/plans/payload@api-20.md ") && !screen.contains("changed since asked"), "the plan's source above it: {screen}");
        let (rows, _) = render(&mut app, 80, 40);
        assert!(rows.join("\n").contains("Approve this plan?"), "a narrow window stacks the list above the question");
    }

    #[test]
    fn the_question_window_follows_its_question_when_others_close_and_clears_when_its_own_does() {
        let mut questions = three_questions();
        let mut window = QuestionWindow::new(&questions, 1);
        window.option = 0;
        window.comment.insert_str("use the first");
        questions.remove(0);
        window.follow(&questions);
        assert_eq!((window.selected, window.id.as_str(), window.option, window.comment().as_str()), (0, "b", 0, "use the first"), "B stays selected with its choice");
        assert!(window.notice.is_none());
        questions.remove(0);
        window.follow(&questions);
        assert_eq!((window.id.as_str(), window.option, window.comment().as_str()), ("c", 1, ""), "C does not inherit B's choice or comment");
        assert!(window.notice.as_ref().is_some_and(|(ok, text)| !ok && text.contains("answered or closed elsewhere")));
        let mut window = QuestionWindow::new(&three_questions(), 0);
        window.option = 0;
        window.comment.insert_str("for a");
        window.select(&three_questions(), 1);
        assert_eq!((window.option, window.comment().as_str()), (1, ""), "moving in the list resets the option and comment");
    }

    #[test]
    fn in_my_own_words_needs_a_comment_and_is_offered_after_the_options() {
        let mut app = test_app(Vec::new());
        app.questions = three_questions();
        app.open_questions(Some(0));
        let (rows, _) = render(&mut app, 170, 40);
        let screen = rows.join("\n");
        let (_, two) = find(&rows, "Two  recommended · the second")[0];
        let (_, own) = find(&rows, "In my own words  the comment is the whole answer")[0];
        assert_eq!(own, two + 1, "{screen}");
        assert_eq!(app.hits.iter().filter(|(_, hit)| matches!(hit, Hit::Answer(_))).count(), 3);
        let Mode::Questions(mut window) = std::mem::replace(&mut app.mode, Mode::Normal) else { panic!() };
        window.option = 2;
        app.handle_questions(window, KeyEvent::from(KeyCode::Enter));
        let Mode::Questions(window) = &app.mode else { panic!("the window stays open") };
        assert!(window.answering.is_none() && window.part == Part::Comment, "an empty answer in your own words is refused");
        assert!(window.notice.as_ref().is_some_and(|(ok, text)| !ok && text.contains("own words")));
    }

    #[test]
    fn a_plan_that_cannot_be_read_yet_is_read_again_on_the_next_draw() {
        let mut app = test_app(Vec::new());
        app.questions = messages::fold(&PLAN.replace("/state/plans/payload@api-20.md", "/nowhere/kelpie-plan.md"));
        app.open_questions(None);
        render(&mut app, 170, 40);
        assert!(app.plans.is_empty(), "nothing is cached for a plan that is not there");
    }

    #[test]
    fn a_plan_rendered_as_markdown_carries_no_escape_and_a_card_image_path_is_shown_as_text() {
        let text = markdown(&format!("# Plan {ESCAPES}\n\n- step [link](javascript:alert(1)) {ESCAPES}\n\n```sh\n{ESCAPES}\n```\n"));
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| frame.render_widget(Paragraph::new(text.clone()), frame.area())).unwrap();
        no_escape_reaches_the_terminal(terminal.backend().buffer(), "a markdown plan");
        let card = report::Card { sha: "abc".to_string(), text: format!("![alt {ESCAPES}](../../etc/passwd)\n{ESCAPES}"), file: PathBuf::from("/state/cards/x.md") };
        let lines = card_lines(&card, true, 60);
        let shown: String = lines.iter().map(|line| line.to_string()).collect::<Vec<_>>().join("\n");
        assert!(shown.contains("▣ ../../etc/passwd"), "an image is shown by its path, read by the user before anything opens it: {shown}");
        terminal.draw(|frame| frame.render_widget(Paragraph::new(lines.clone()), frame.area())).unwrap();
        no_escape_reaches_the_terminal(terminal.backend().buffer(), "a card");
    }

    #[test]
    fn the_question_windows_parts_cycle_both_ways() {
        let order = [Part::List, Part::Plan, Part::Options, Part::Comment];
        for (index, part) in order.iter().enumerate() {
            assert!(part.next(false) == order[(index + 1) % 4]);
            assert!(part.next(true) == order[(index + 3) % 4]);
        }
    }
}
