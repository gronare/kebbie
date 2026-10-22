use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
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
use super::card::{WIDE, rich_wrap};
use super::widgets::{BLUE, FAINT, GREEN, MANTLE, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, TEXT, YELLOW, bold, centered, fill, fit, rule, section_header, style};

pub(super) struct QuestionWindow {
    id: String,
    selected: usize,
    pub(super) option: usize,
    pub(super) scroll: u16,
    pub(super) typing: bool,
    pub(super) comment: TextArea<'static>,
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
        let mut window = Self { id: String::new(), selected: 0, option: 0, scroll: 0, typing: false, comment: comment_field(), notice: None, answering: None };
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
            self.typing = false;
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

struct Answer {
    key: char,
    short: String,
    label: String,
    about: String,
    meaning: String,
    recommended: bool,
    choice: Option<usize>,
    takes_comment: bool,
}

const PLAN_KEYS: [(char, &str); 4] = [('a', "approve"), ('c', "comment"), ('r', "changes"), ('x', "reject")];
const OWN_WORDS: &str = "In my own words";

fn answers(message: &Message) -> Vec<Answer> {
    let question = message.question.as_ref();
    let plan = question.is_some_and(|question| question.plan.is_some());
    let mut answers: Vec<Answer> = question.map(|question| question.options.clone()).unwrap_or_default().into_iter().enumerate().map(|(index, (label, meaning))| {
        let planned = questions::PLAN_OPTIONS.iter().position(|(plan_label, _)| *plan_label == label).filter(|_| plan);
        let (key, short) = match planned {
            Some(at) => (PLAN_KEYS[at].0, PLAN_KEYS[at].1.to_string()),
            None => (char::from_digit(index as u32 + 1, 10).unwrap_or(' '), label.clone()),
        };
        let recommended = question.is_some_and(|question| question.recommend == index + 1);
        let about = if recommended { format!("recommended · {meaning}") } else { meaning.clone() };
        Answer { key, short, label, about, meaning, recommended, choice: Some(index), takes_comment: planned.is_some_and(|at| at > 0) }
    }).collect();
    answers.push(Answer {
        key: 'w', short: "own words".to_string(), label: OWN_WORDS.to_string(), about: "the comment is the whole answer".to_string(),
        meaning: "write the answer yourself".to_string(), recommended: false, choice: None, takes_comment: true,
    });
    answers
}

fn plan_file(message: &Message) -> Option<PathBuf> {
    let plan = message.question.as_ref()?.plan.as_deref()?;
    let file = PathBuf::from(plan);
    (!plan.starts_with(questions::VAULT_PLAN) && file.is_file()).then_some(file)
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
        let map = keys::map();
        let count = self.questions.len().max(1);
        let answers = self.questions.get(window.selected).map(answers).unwrap_or_default();
        let planned = self.questions.get(window.selected).and_then(|message| message.question.as_ref()).is_some_and(|question| question.plan.is_some());
        let count_answers = answers.len().max(1);
        let busy = window.answering.is_some();
        if window.typing {
            match key.code {
                KeyCode::Esc => window.typing = false,
                KeyCode::PageUp => window.scroll = window.scroll.saturating_sub(10),
                KeyCode::PageDown => window.scroll = window.scroll.saturating_add(10),
                KeyCode::Enter if !busy => self.send(&mut window, &answers),
                KeyCode::Enter => {}
                _ => {
                    window.comment.input(key);
                }
            }
            self.mode = Mode::Questions(window);
            return;
        }
        let pressed = match key.code {
            KeyCode::Char(character) if !busy => answers.iter().position(|answer| answer.key == character),
            _ => None,
        };
        let open = map.codes(Act::OpenImage).contains(&key.code);
        match key.code {
            _ if pressed.is_some() => {
                window.option = pressed.unwrap_or_default();
                self.choose(&mut window, &answers);
            }
            KeyCode::Esc => return,
            KeyCode::Char(character) if character == keys::key(Act::Close) => return,
            KeyCode::Tab => window.select(&self.questions, (window.selected + 1) % count),
            KeyCode::BackTab => window.select(&self.questions, (window.selected + count - 1) % count),
            KeyCode::Up | KeyCode::Char('k') if !planned => window.option = (window.option + count_answers - 1) % count_answers,
            KeyCode::Down | KeyCode::Char('j') if !planned => window.option = (window.option + 1) % count_answers,
            KeyCode::Up | KeyCode::Char('k') => window.scroll = window.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => window.scroll = window.scroll.saturating_add(1),
            KeyCode::PageUp => window.scroll = window.scroll.saturating_sub(10),
            KeyCode::PageDown => window.scroll = window.scroll.saturating_add(10),
            KeyCode::Left if planned => window.option = (window.option + count_answers - 1) % count_answers,
            KeyCode::Right if planned => window.option = (window.option + 1) % count_answers,
            KeyCode::Enter if !busy => self.choose(&mut window, &answers),
            KeyCode::Char(character) if character == keys::key(Act::Go) => {
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
            _ if open => {
                if let Some(file) = self.questions.get(window.selected).and_then(plan_file) {
                    self.open_file(&file);
                }
            }
            _ => {}
        }
        self.mode = Mode::Questions(window);
    }

    fn choose(&mut self, window: &mut QuestionWindow, answers: &[Answer]) {
        let Some(answer) = answers.get(window.option) else { return };
        if answer.takes_comment {
            window.comment.set_placeholder_text(if answer.choice.is_none() { "your answer" } else { "comment (optional)" });
            window.typing = true;
        } else {
            self.send(window, answers);
        }
    }

    fn send(&mut self, window: &mut QuestionWindow, answers: &[Answer]) {
        let (Some(message), Some(answer)) = (self.questions.get(window.selected), answers.get(window.option)) else { return };
        let comment = if answer.takes_comment { window.comment() } else { String::new() };
        if answer.choice.is_none() && comment.is_empty() {
            window.typing = true;
            window.notice = Some((false, "In your own words: write the answer in the comment field first.".to_string()));
            return;
        }
        let (sender, result) = mpsc::channel();
        let id = message.id.clone();
        let asked = id.clone();
        let choice = answer.choice;
        thread::spawn(move || {
            let _ = sender.send(questions::answer(&asked, choice, &comment));
        });
        window.answering = Some((id, result));
        window.notice = Some((true, "Answering…".to_string()));
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

fn split_address(address: &str) -> (String, String) {
    match address.split_once('@') {
        Some((name, place)) => (name.to_string(), place.to_string()),
        None => (address.to_string(), String::new()),
    }
}

pub(super) fn plan_header(scroll: u16, last: u16, changed: bool, width: usize) -> Line<'static> {
    let mut extra = Vec::new();
    if last > 0 {
        extra.push(Span::styled(format!("{}%", u32::from(scroll) * 100 / u32::from(last)), style(MUTED)));
    }
    if changed {
        extra.push(Span::styled("changed since asked", style(YELLOW)));
    }
    section_header("PLAN", extra, width)
}

fn answer_row(answers: &[Answer], option: usize, area: Rect, hits: &mut Vec<(Rect, Hit)>) -> Line<'static> {
    let mut spans = Vec::new();
    let mut x = 0usize;
    for (index, answer) in answers.iter().enumerate() {
        let active = index == option;
        let ending = answer.label.starts_with("Reject");
        let label = match (active, ending) {
            (true, true) => bold(RED),
            (true, false) => bold(MAUVE),
            (false, true) => style(RED),
            (false, false) => style(TEXT),
        };
        let background = if active { Style::new().bg(SURFACE) } else { Style::new() };
        if index > 0 {
            spans.push(Span::raw(" "));
            x += 1;
        }
        let key = answer.key.to_string();
        let short = format!(" {}", answer.short);
        let width = key.chars().count() + short.chars().count();
        let shown = width.min((area.width as usize).saturating_sub(x));
        if shown > 0 {
            hits.push((Rect::new(area.x + x as u16, area.y, shown as u16, 1), Hit::Answer(index)));
        }
        x += width;
        spans.push(Span::styled(key, style(MAUVE).patch(background)));
        spans.push(Span::styled(short, label.patch(background)));
    }
    fit(spans, area.width as usize)
}

fn question_keys(typing: bool, plan: bool, several: bool, open: bool) -> String {
    let map = keys::map();
    if typing {
        return format!("{} send · pgup pgdn plan · {} back to the answers", map.label(Act::Options), map.label(Act::Back));
    }
    let mut keys = Vec::new();
    if plan {
        keys.push("↑ ↓ scroll".to_string());
    }
    keys.push(format!("{} answer", map.label(Act::Switch)));
    keys.push(format!("{} send", map.label(Act::Options)));
    if several {
        keys.push(format!("{} next", map.label(Act::Lead)));
    }
    if open {
        keys.push(format!("{} open the plan", map.label(Act::OpenImage)));
    }
    keys.push(format!("{} its Space", map.label(Act::Go)));
    keys.push(format!("{} close", map.label(Act::Back)));
    keys.join(" · ")
}

fn question_list(questions: &[Message], selected: usize, area: Rect, hits: &mut Vec<(Rect, Hit)>) -> Vec<Line<'static>> {
    let width = area.width as usize;
    let mut lines = vec![section_header("QUESTIONS", vec![Span::styled(questions.len().to_string(), style(MUTED))], width)];
    for (index, message) in questions.iter().enumerate() {
        let active = index == selected;
        let (name, place) = split_address(&message.from);
        let kind = if message.question.as_ref().is_some_and(|question| question.plan.is_some()) { "plan" } else { "ask " };
        let row = fit(vec![
            Span::raw(if active { "▸ " } else { "  " }), Span::styled(format!("{kind}  "), style(YELLOW)), Span::styled(name, if active { bold(TEXT) } else { style(TEXT) }),
            Span::styled(format!("  {place} · {}", messages::age(message.at)), style(MUTED)),
        ], width);
        let y = area.y + 1 + index as u16;
        if y < area.bottom() {
            hits.push((Rect::new(area.x, y, area.width, 1), Hit::Question(index)));
        }
        lines.push(if active { row.style(Style::new().bg(SURFACE)) } else { row });
    }
    lines
}

pub(super) fn draw_questions(frame: &mut Frame, app: &mut App) -> Vec<(Rect, Hit)> {
    let mut hits = Vec::new();
    let selected = match &app.mode { Mode::Questions(window) => window.selected, _ => return hits };
    let plan = app.questions.get(selected).cloned().and_then(|message| {
        let question = message.question.clone()?;
        if !app.plans.contains_key(&message.id) {
            app.plans.insert(message.id.clone(), markdown(&questions::plan_text(&question)?));
        }
        app.plans.get(&message.id).cloned()
    });
    let questions = app.questions.clone();
    let viewer = app.viewer.is_some();
    let Mode::Questions(window) = &mut app.mode else { return hits };
    let Some(message) = questions.get(window.selected) else { return hits };
    let question = message.question.as_ref();
    let planned = question.is_some_and(|question| question.plan.is_some());
    let (name, _) = split_address(&message.from);
    if !planned {
        draw_ask(frame, window, message, questions.len(), &mut hits);
        return hits;
    }
    let full = frame.area();
    let area = centered(full, (full.width * 92 / 100).max(40), (full.height * 84 / 100).max(16));
    frame.render_widget(Clear, area);
    let title = format!(" question {}/{} · {} · {name} ", window.selected + 1, questions.len(), if planned { "plan" } else { "ask" });
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(title, bold(MAUVE))).style(Style::new().bg(MANTLE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [body, line, foot] = Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    let main = if inner.width >= WIDE {
        let left = (inner.width * 3 / 10).clamp(30, 48);
        let [list, _, bar, _, main] = Layout::horizontal([Constraint::Length(left), Constraint::Length(2), Constraint::Length(1), Constraint::Length(2), Constraint::Min(1)]).areas(body);
        frame.render_widget(Paragraph::new((0..bar.height).map(|_| Line::from(Span::styled("│", style(FAINT)))).collect::<Vec<_>>()), bar);
        let lines = question_list(&questions, window.selected, list, &mut hits);
        frame.render_widget(Paragraph::new(fill(lines, list.width as usize)), list);
        main
    } else {
        body
    };
    let width = main.width as usize;
    let answers = answers(message);
    window.option = window.option.min(answers.len().saturating_sub(1));
    let asked: Vec<Line> = rich_wrap(&message.text, width, 4, TEXT).into_iter().map(|line| line.style(Style::new().add_modifier(Modifier::BOLD))).collect();
    let comment_rows = (window.comment.lines().len() as u16 + 2).clamp(3, 8);
    let answer_height = if window.typing { 2 + comment_rows } else { 3 };
    let [asked_area, plan_area, answer_area] = Layout::vertical([Constraint::Length(asked.len() as u16), Constraint::Min(0), Constraint::Length(answer_height)]).areas(main);
    frame.render_widget(Paragraph::new(asked), asked_area);
    if planned && plan_area.height > 0 {
        let [header, text] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(plan_area);
        let changed = question.is_some_and(questions::plan_changed);
        match plan {
            Some(plan) => {
                let paragraph = Paragraph::new(plan).wrap(Wrap { trim: false });
                let last = (paragraph.line_count(text.width) as u16).saturating_sub(text.height);
                window.scroll = window.scroll.min(last);
                frame.render_widget(Paragraph::new(plan_header(window.scroll, last, changed, width)), header);
                frame.render_widget(paragraph.scroll((window.scroll, 0)), text);
            }
            None => {
                frame.render_widget(Paragraph::new(plan_header(0, 0, changed, width)), header);
                let source = question.and_then(|question| question.plan.clone()).unwrap_or_default();
                frame.render_widget(Paragraph::new(fit(vec![Span::styled(format!("not readable yet: {source}"), style(MUTED))], width)), text);
            }
        }
    }
    let [answer_header, choices, rest] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(answer_area);
    frame.render_widget(Paragraph::new(section_header("ANSWER", Vec::new(), width)), answer_header);
    let chosen = answers.get(window.option);
    if window.typing {
        let about = chosen.map_or(Vec::new(), |answer| vec![Span::styled(answer.label.clone(), bold(TEXT)), Span::styled(format!(" · {}", answer.about), style(MUTED))]);
        frame.render_widget(Paragraph::new(fit(about, width)), choices);
        window.comment.set_block(Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)));
        window.comment.set_cursor_style(Style::new().add_modifier(Modifier::REVERSED));
        frame.render_widget(&window.comment, rest);
    } else {
        let row = answer_row(&answers, window.option, choices, &mut hits);
        frame.render_widget(Paragraph::new(row), choices);
        let about = chosen.map(|answer| answer.about.clone()).unwrap_or_default();
        frame.render_widget(Paragraph::new(fit(vec![Span::styled(about, style(MUTED))], width)), Rect { height: 1, ..rest });
    }
    frame.render_widget(Paragraph::new(rule(line.width as usize)), line);
    let mut keys = Vec::new();
    if let Some((ok, notice)) = &window.notice {
        keys.push(Span::styled(format!("{} {} · ", if *ok { "✓" } else { "!" }, notice.split_whitespace().collect::<Vec<_>>().join(" ")), style(if *ok { GREEN } else { RED })));
    }
    let open = viewer && plan_file(message).is_some();
    keys.push(Span::styled(question_keys(window.typing, planned, questions.len() > 1, open), style(MUTED)));
    frame.render_widget(Paragraph::new(fit(keys, foot.width as usize)), foot);
    hits
}

const ASK_WIDTH: u16 = 80;
const INDENT: usize = 5;

fn ask_keys(typing: bool, options: usize, several: bool, wide: bool) -> String {
    let map = keys::map();
    if typing {
        return format!("{} send · {} back to the answers", map.label(Act::Options), map.label(Act::Back));
    }
    let range = if options > 1 { format!("1–{options}") } else { "1".to_string() };
    let mut keys = vec![format!("{range} answer"), "w own words".to_string()];
    if wide {
        keys.push(format!("↑ ↓ {} pick", map.label(Act::Options)));
    }
    if several {
        keys.push(format!("{} next", map.label(Act::Lead)));
    }
    if wide {
        keys.push(format!("{} its Space", map.label(Act::Go)));
    }
    keys.push(format!("{} close", map.label(Act::Back)));
    keys.join(" · ")
}

struct AskBody {
    lines: Vec<Line<'static>>,
    blocks: Vec<(usize, usize)>,
    field: Option<usize>,
}

fn ask_body(message: &Message, answers: &[Answer], option: usize, typing: bool, comment_rows: usize, width: usize) -> AskBody {
    let mut lines = vec![Line::default()];
    lines.extend(rich_wrap(&message.text, width, usize::MAX, TEXT).into_iter().map(|line| line.style(Style::new().add_modifier(Modifier::BOLD))));
    lines.push(Line::default());
    let mut blocks = Vec::new();
    let mut field = None;
    for (index, answer) in answers.iter().enumerate() {
        let own = answer.choice.is_none();
        if own {
            lines.push(Line::from(Span::styled("┄".repeat(width), style(FAINT))));
        }
        let active = index == option;
        let start = lines.len();
        let label = if active { bold(MAUVE) } else { style(TEXT) };
        let mut head = vec![Span::styled(if active { "▸ " } else { "  " }, style(MAUVE)), Span::styled(answer.key.to_string(), bold(MAUVE)), Span::raw("  "), Span::styled(answer.label.clone(), label)];
        let used: usize = head.iter().map(Span::width).sum();
        let recommended = "recommended";
        let beside = answer.recommended && used + recommended.len() + 2 <= width;
        if beside {
            head.push(Span::raw(" ".repeat(width - used - recommended.len())));
            head.push(Span::styled(recommended, style(GREEN)));
        }
        lines.push(Line::from(head));
        let indent = " ".repeat(INDENT);
        if typing && own {
            field = Some(lines.len());
            lines.extend((0..comment_rows).map(|_| Line::default()));
        } else {
            if answer.recommended && !beside {
                lines.push(Line::from(vec![Span::raw(indent.clone()), Span::styled(recommended, style(GREEN))]));
            }
            let color = if own { MUTED } else { SUBTEXT };
            for line in rich_wrap(&answer.meaning, width.saturating_sub(INDENT + 1), usize::MAX, color) {
                let mut spans = vec![Span::raw(indent.clone())];
                spans.extend(line.spans);
                lines.push(Line::from(spans));
            }
        }
        if active {
            for line in &mut lines[start..] {
                line.style = line.style.bg(SURFACE);
            }
        }
        blocks.push((start, lines.len()));
        if answers.get(index + 1).is_some_and(|next| next.choice.is_some()) {
            lines.push(Line::default());
        }
    }
    lines.push(Line::default());
    AskBody { lines, blocks, field }
}

fn ask_offset(block: (usize, usize), rows: usize, view: usize) -> usize {
    let (start, end) = block;
    if rows <= view || end <= view {
        return 0;
    }
    (end - view).min(start)
}

fn draw_ask(frame: &mut Frame, window: &mut QuestionWindow, message: &Message, count: usize, hits: &mut Vec<(Rect, Hit)>) {
    let full = frame.area();
    for cell in &mut frame.buffer_mut().content {
        cell.set_fg(FAINT).set_bg(ratatui::style::Color::Reset);
        cell.modifier = Modifier::empty();
    }
    let width = ASK_WIDTH.min(full.width.saturating_sub(4));
    let inner_width = width.saturating_sub(4) as usize;
    let answers = answers(message);
    window.option = window.option.min(answers.len().saturating_sub(1));
    let comment_rows = (window.comment.lines().len() + 2).clamp(3, 8);
    let body = ask_body(message, &answers, window.option, window.typing, comment_rows, inner_width);
    let area = centered(full, width, body.lines.len() as u16 + 4);
    frame.render_widget(Clear, area);
    let (name, _) = split_address(&message.from);
    let title = if width >= 70 { format!(" question {}/{count} · ask · {name} ", window.selected + 1) } else { format!(" {}/{count} · {name} ", window.selected + 1) };
    let age = messages::age(message.at);
    let age = if age == "now" { " now ".to_string() } else { format!(" {age} ago ") };
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(title, bold(MAUVE))).title_top(Line::from(Span::styled(age, style(MUTED))).right_aligned()).style(Style::new().bg(MANTLE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [text, line, foot] = Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    let view = text.height as usize;
    let offset = ask_offset(body.blocks.get(window.option).copied().unwrap_or_default(), body.lines.len(), view);
    frame.render_widget(Paragraph::new(body.lines[offset.min(body.lines.len())..].to_vec()), text);
    let visible = |row: usize| (offset..offset + view).contains(&row);
    for (index, (start, end)) in body.blocks.iter().enumerate() {
        let first = (*start).max(offset);
        let last = (*end).min(offset + view);
        if first < last {
            hits.push((Rect::new(text.x, text.y + (first - offset) as u16, text.width, (last - first) as u16), Hit::Answer(index)));
        }
    }
    if let Some(at) = body.field.filter(|at| visible(*at)) {
        let y = text.y + (at - offset) as u16;
        let rows = (comment_rows as u16).min(text.bottom().saturating_sub(y));
        let field = Rect::new(text.x + INDENT as u16, y, text.width.saturating_sub(INDENT as u16), rows);
        window.comment.set_block(Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)));
        window.comment.set_cursor_style(Style::new().add_modifier(Modifier::REVERSED));
        frame.render_widget(&window.comment, field);
    }
    frame.render_widget(Paragraph::new(rule(line.width as usize)), line);
    let mut keys = Vec::new();
    if let Some((ok, notice)) = &window.notice {
        keys.push(Span::styled(format!("{} {} · ", if *ok { "✓" } else { "!" }, notice.split_whitespace().collect::<Vec<_>>().join(" ")), style(if *ok { GREEN } else { RED })));
    }
    let options = answers.iter().filter(|answer| answer.choice.is_some()).count();
    keys.push(Span::styled(ask_keys(window.typing, options, count > 1, inner_width >= 70), style(MUTED)));
    frame.render_widget(Paragraph::new(fit(keys, foot.width as usize)), foot);
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
    use crate::board::{App, Hit, Mode};
    use crate::board::widgets::{FAINT, MANTLE, SURFACE, YELLOW};

    fn press(app: &mut App, code: KeyCode) {
        let Mode::Questions(window) = std::mem::replace(&mut app.mode, Mode::Normal) else { panic!("the questions window is open") };
        app.handle_questions(window, KeyEvent::from(code));
    }

    fn long_plan(app: &mut App) {
        app.questions = messages::fold(&[QUESTION, PLAN].join("\n"));
        let steps: String = (1..=40).map(|step| format!("- plan step {step}\n")).collect();
        app.plans.insert("q2".to_string(), markdown(&format!("## Problem\n\nThe payload says *amount*.\n\n{steps}")));
    }

    fn row_of(rows: &[String], text: &str) -> usize {
        find(rows, text).first().unwrap_or_else(|| panic!("{text} is not on screen:\n{}", rows.join("\n"))).1
    }

    #[test]
    fn in_a_narrow_window_the_plan_takes_the_height_and_the_answers_sit_on_one_row_above_the_keys() {
        let mut app = test_app(Vec::new());
        long_plan(&mut app);
        app.open_questions(Some(1));
        let (rows, _) = render(&mut app, 60, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("question 2/2 · plan · payload"), "{screen}");
        assert!(screen.contains("Approve this plan?") && screen.contains("PLAN 0% ─"), "{screen}");
        assert!(screen.contains("Problem") && !screen.contains("## Problem") && screen.contains("The payload says amount."), "{screen}");
        let plan_lines = rows.iter().filter(|row| row.contains("plan step")).count();
        assert!(plan_lines >= 12, "the plan gets most of the window, {plan_lines} lines: {screen}");
        assert!(!screen.contains("/state/plans") && !screen.contains("waiting ·"), "no path line and no list: {screen}");
        let answers = row_of(&rows, "a approve");
        assert!(rows[answers].contains("c comment") && rows[answers].contains("r changes"), "{screen}");
        assert_eq!(row_of(&rows, "recommended · build it as planned"), answers + 1, "the selected answer's meaning under the row");
        assert!(rows[answers].contains("x reject w own words"), "every answer fits a 60-column window: {screen}");
        assert!(rows[answers + 2].contains("────") && rows[answers + 3].contains("│ ↑ ↓ scroll · ← → answer · ⏎ send"), "the footer: a rule, then the keys: {screen}");
        assert!(rows[answers + 4].contains("╰"), "the keys are the window's last inner row: {screen}");
        assert_eq!(row_of(&rows, "ANSWER ─"), answers - 1);
        assert_eq!(app.hits.iter().filter(|(_, hit)| matches!(hit, Hit::Question(_))).count(), 0, "no list in a narrow window");
        assert_eq!(app.hits.iter().filter(|(_, hit)| matches!(hit, Hit::Answer(_))).count(), 5, "four plan answers and in my own words");
    }

    #[test]
    fn left_and_right_move_the_selected_answer_and_its_meaning_and_a_click_selects_one() {
        let mut app = test_app(Vec::new());
        long_plan(&mut app);
        app.open_questions(Some(1));
        let (_, buffer) = render(&mut app, 60, 30);
        let surface = |buffer: &ratatui::buffer::Buffer, rows: &[String], text: &str| {
            let (x, y) = find(rows, text)[0];
            buffer[(x as u16, y as u16)].bg == SURFACE
        };
        let rows: Vec<String> = (0..30).map(|y| (0..60).map(|x| buffer[(x, y)].symbol()).collect()).collect();
        assert!(surface(&buffer, &rows, "approve") && !surface(&buffer, &rows, "comment"));
        press(&mut app, KeyCode::Right);
        let (rows, buffer) = render(&mut app, 60, 30);
        assert!(surface(&buffer, &rows, "comment") && !surface(&buffer, &rows, "approve"), "the selection moved");
        assert!(rows.join("\n").contains("build it, taking the comment in") && !rows.join("\n").contains("build it as planned"), "the meaning follows the selection");
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        let (rows, _) = render(&mut app, 60, 30);
        assert!(rows.join("\n").contains("the comment is the whole answer"), "left from the first wraps to in my own words");
        let (x, y) = find(&rows, "x reject")[0];
        assert_eq!(app.hit_at(x as u16 + 1, y as u16), Some(Hit::Answer(3)));
        click(&mut app, x as u16 + 1, y as u16);
        let (rows, _) = render(&mut app, 60, 30);
        assert!(rows.join("\n").contains("drop this approach"), "a click selects the answer");
    }

    #[test]
    fn an_answer_that_takes_a_comment_opens_the_comment_box_in_place_of_the_row_and_esc_returns() {
        let mut app = test_app(Vec::new());
        long_plan(&mut app);
        app.open_questions(Some(1));
        let (before, _) = render(&mut app, 60, 30);
        let plan_before = before.iter().filter(|row| row.contains("plan step")).count();
        press(&mut app, KeyCode::Char('c'));
        let (rows, _) = render(&mut app, 60, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("Approve with a comment · build it, taking") && screen.contains("comment (optional)"), "{screen}");
        assert!(!screen.contains("a approve") && screen.contains("esc back to the answers"), "{screen}");
        let plan_after = rows.iter().filter(|row| row.contains("plan step")).count();
        assert_eq!(plan_before - plan_after, 2, "the plan gives up only the box's rows: {screen}");
        press(&mut app, KeyCode::Char('a'));
        let Mode::Questions(window) = &app.mode else { panic!() };
        assert!(window.answering.is_none() && window.comment() == "a", "a letter in the box is text, not an answer");
        press(&mut app, KeyCode::PageDown);
        let Mode::Questions(window) = &app.mode else { panic!() };
        assert!(window.scroll > 0 && window.typing, "pgdn scrolls the plan while typing");
        press(&mut app, KeyCode::Esc);
        let (rows, _) = render(&mut app, 60, 30);
        let screen = rows.join("\n");
        assert!(matches!(app.mode, Mode::Questions(_)) && screen.contains("a approve") && !screen.contains("comment (optional)"), "esc goes back to the answers: {screen}");
    }

    #[test]
    fn a_paste_goes_into_the_comment_box_and_never_answers_the_question() {
        let mut app = test_app(Vec::new());
        long_plan(&mut app);
        app.open_questions(Some(1));
        assert!(app.take_paste("a".to_string()).is_some(), "with the box closed the paste is passed on, not taken as the answer a");
        let Mode::Questions(window) = &app.mode else { panic!() };
        assert!(window.answering.is_none() && !window.typing);
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(app.take_paste("see /var/x.png".to_string()), None);
        let Mode::Questions(window) = &app.mode else { panic!() };
        assert!(window.answering.is_none() && window.comment() == "see /var/x.png");
    }

    #[test]
    fn tab_moves_to_the_next_question_and_the_title_counts_it() {
        let mut app = test_app(Vec::new());
        long_plan(&mut app);
        app.open_questions(Some(0));
        let screen = render(&mut app, 100, 30).0.join("\n");
        assert!(screen.contains("question 1/2 · ask · billing") && screen.contains("tab next"), "{screen}");
        assert!(screen.contains("1  Sidekiq") && screen.contains("2  Solid Queue") && screen.contains("w  In my own words") && !screen.contains("PLAN"), "a plain question lists its options: {screen}");
        assert!(render(&mut app, 60, 30).0.join("\n").contains("╭ 1/2 · billing "), "a narrow title keeps the count and the task");
        press(&mut app, KeyCode::Tab);
        assert!(render(&mut app, 60, 30).0.join("\n").contains("question 2/2 · plan · payload"));
        press(&mut app, KeyCode::Tab);
        assert!(render(&mut app, 100, 30).0.join("\n").contains("question 1/2 · ask · billing"), "tab wraps around");
    }

    #[test]
    fn a_wide_window_keeps_the_list_on_the_left_of_a_bar() {
        let mut app = test_app(Vec::new());
        long_plan(&mut app);
        app.open_questions(Some(1));
        let (rows, buffer) = render(&mut app, 160, 40);
        let screen = rows.join("\n");
        let (list_x, header) = find(&rows, "QUESTIONS 2")[0];
        let (x, y) = find(&rows, "▸ plan  payload")[0];
        assert_eq!(y, header + 2, "{screen}");
        assert!(buffer[(x as u16, y as u16)].bg == SURFACE, "the selected question on the surface, as the card's images");
        let bar = rows[y].chars().enumerate().position(|(at, character)| character == '│' && at > list_x).expect("a bar right of the list");
        assert_eq!(buffer[(bar as u16, y as u16)].fg, FAINT);
        let (plan_x, _) = find(&rows, "Approve this plan?")[0];
        assert!(plan_x > bar && find(&rows, "a approve")[0].0 > bar, "the question and answers right of the bar: {screen}");
        assert_eq!(app.hits.iter().filter(|(_, hit)| matches!(hit, Hit::Question(_))).count(), 2);
        click(&mut app, list_x as u16 + 4, (header + 1) as u16);
        assert!(render(&mut app, 160, 40).0.join("\n").contains("question 1/2 · ask · billing"), "a click on a question opens it");
    }

    #[test]
    fn the_plan_header_shows_the_scroll_position_and_a_change_since_asked() {
        let header = plan_header(19, 50, true, 60);
        let shown = header.to_string();
        assert!(shown.starts_with("PLAN 38% changed since asked ─"), "{shown}");
        assert!(header.spans.iter().any(|span| span.content == "changed since asked" && span.style.fg == Some(YELLOW)));
        assert!(plan_header(0, 0, false, 60).to_string().starts_with("PLAN ──"), "a plan that fits shows no position");
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
    fn in_my_own_words_comes_last_and_opens_a_field_under_its_row_with_the_options_kept() {
        let mut app = test_app(Vec::new());
        app.questions = three_questions();
        app.open_questions(Some(0));
        let (rows, _) = render(&mut app, 60, 30);
        let own = row_of(&rows, "w  In my own words");
        assert!(row_of(&rows, "2  Two") < own && row_of(&rows, "┄┄┄") == own - 1, "{}", rows.join("\n"));
        assert_eq!(app.hits.iter().filter(|(_, hit)| matches!(hit, Hit::Answer(_))).count(), 3);
        press(&mut app, KeyCode::Char('w'));
        let (rows, _) = render(&mut app, 60, 30);
        let screen = rows.join("\n");
        assert!(row_of(&rows, "your answer") > row_of(&rows, "w  In my own words") && screen.contains("1  One") && screen.contains("2  Two"), "{screen}");
        assert!(screen.contains("esc back to the answers"), "{screen}");
        press(&mut app, KeyCode::Enter);
        let Mode::Questions(window) = &app.mode else { panic!("the window stays open") };
        assert!(window.answering.is_none() && window.typing, "an empty answer in your own words is refused");
        assert!(window.notice.as_ref().is_some_and(|(ok, text)| !ok && text.contains("own words")));
    }

    const LONG_ASK: &str = r#"{"id":"q9","at":10,"from":"routines@kelpie","to":"lead","text":"The keeper runs the routines when no board is open, and an open board runs them itself, so a routine can start twice when a board opens in the middle of a keeper run. Which side should own the routine schedule from now on?","state":"asked","question":{"options":[{"label":"Keeper only","meaning":"the keeper runs every routine whether a board is open or not; the board shows each routine's last and next run"},{"label":"Board first","meaning":"an open board runs the routines and the keeper stands down while a board answers on its socket"},{"label":"Both, with a lock","meaning":"either side may run a routine, but takes a lock file in the state directory first and the other skips it"},{"label":"Per routine","meaning":"each routine names its owner in its file; a routine without one stays with the keeper as today"}],"recommend":1}}"#;

    fn window_rows(rows: &[String]) -> (usize, usize) {
        let top = rows.iter().position(|row| row.contains('╭')).expect("a window");
        let bottom = rows.iter().rposition(|row| row.contains('╰')).expect("a window");
        (top, bottom)
    }

    #[test]
    fn a_plain_question_sizes_its_window_to_its_content_and_shows_every_meaning_in_full() {
        let mut app = test_app(vec![served("search-filters")]);
        app.questions = messages::fold(QUESTION);
        app.open_questions(Some(0));
        let (rows, buffer) = render(&mut app, 140, 40);
        let screen = rows.join("\n");
        let (top, bottom) = window_rows(&rows);
        assert_eq!(bottom - top + 1, 19, "a short question gets a short window: {screen}");
        let left = rows[top].chars().position(|character| character == '╭').unwrap();
        let right = rows[top].chars().position(|character| character == '╮').unwrap();
        assert_eq!(right - left + 1, 80, "{screen}");
        assert!(buffer[(left as u16, (top - 1) as u16)].bg != MANTLE && buffer[(left as u16, (bottom + 1) as u16)].bg != MANTLE, "the board shows above and below");
        for meaning in ["reuse the jobs", "one less service", "no queue at all", "write the answer yourself"] {
            assert!(screen.contains(meaning), "{meaning}: {screen}");
        }
        let recommended = row_of(&rows, "2  Solid Queue");
        assert!(rows[recommended].contains("recommended") && rows[recommended].contains("▸"), "the recommended answer is marked and selected first: {screen}");
        let mut app = test_app(Vec::new());
        app.questions = messages::fold(LONG_ASK);
        app.open_questions(Some(0));
        let (rows, _) = render(&mut app, 60, 44);
        let screen = rows.join("\n");
        let (top, bottom) = window_rows(&rows);
        let text = rows[top + 1..bottom].iter().map(|row| row.trim().trim_matches('│')).collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
        let asked = messages::fold(LONG_ASK).remove(0);
        assert!(text.contains(&asked.text), "the whole question: {screen}");
        for (_, meaning) in &asked.question.unwrap().options {
            assert!(text.contains(meaning.as_str()), "{meaning}: {screen}");
        }
        assert!(!text.contains('…'), "nothing is cut: {screen}");
    }

    #[test]
    fn the_board_behind_a_plain_question_is_dimmed_and_a_plan_keeps_its_window() {
        let mut app = test_app(vec![served("search-filters")]);
        long_plan(&mut app);
        app.open_questions(Some(0));
        let (rows, buffer) = render(&mut app, 140, 40);
        let (x, y) = find(&rows, "WORK")[0];
        let cell = &buffer[(x as u16, y as u16)];
        assert!(cell.fg == FAINT && !cell.modifier.contains(Modifier::BOLD), "the board behind is drawn faint");
        app.open_questions(Some(1));
        let (rows, buffer) = render(&mut app, 140, 40);
        let (x, y) = find(&rows, "WORK")[0];
        assert!(buffer[(x as u16, y as u16)].fg != FAINT, "a plan's window leaves the board as it is");
        let (top, bottom) = window_rows(&rows);
        assert_eq!(bottom - top + 1, 40 * 84 / 100, "a plan keeps its tall window");
    }

    #[test]
    fn up_and_down_move_the_selected_answer_and_left_and_right_do_not() {
        let mut app = test_app(Vec::new());
        app.questions = messages::fold(QUESTION);
        app.open_questions(Some(0));
        let selected = |app: &mut App, label: &str| {
            let (rows, buffer) = render(app, 100, 30);
            let (x, y) = find(&rows, label)[0];
            rows[y].contains('▸') && buffer[(x as u16, y as u16)].bg == SURFACE && buffer[(x as u16, y as u16 + 1)].bg == SURFACE
        };
        assert!(selected(&mut app, "2  Solid Queue"));
        press(&mut app, KeyCode::Down);
        assert!(selected(&mut app, "3  Inline") && !selected(&mut app, "2  Solid Queue"), "the block under the label moves with it");
        press(&mut app, KeyCode::Right);
        assert!(selected(&mut app, "3  Inline"), "right does nothing in a list");
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        assert!(selected(&mut app, "1  Sidekiq"));
        press(&mut app, KeyCode::Up);
        assert!(selected(&mut app, "w  In my own words"), "up from the first wraps to in my own words");
        press(&mut app, KeyCode::Enter);
        let Mode::Questions(window) = &app.mode else { panic!() };
        assert!(window.typing, "enter on in my own words opens its field");
    }

    #[test]
    fn a_question_taller_than_the_pane_scrolls_to_the_selected_answer_and_keeps_title_and_keys() {
        let mut app = test_app(Vec::new());
        app.questions = messages::fold(LONG_ASK);
        app.open_questions(Some(0));
        let (rows, _) = render(&mut app, 60, 24);
        assert!(rows.join("\n").contains("Which side should own"), "the question shows first: {}", rows.join("\n"));
        for _ in 0..3 {
            press(&mut app, KeyCode::Down);
        }
        let (rows, _) = render(&mut app, 60, 24);
        let screen = rows.join("\n");
        assert!(screen.contains("▸ 4  Per routine") && screen.contains("stays with the keeper as"), "{screen}");
        assert!(screen.contains("1/1 · routines") && screen.contains("1–4 answer · w own words"), "{screen}");
        let (x, y) = find(&rows, "4  Per routine")[0];
        assert_eq!(app.hit_at(x as u16, y as u16), Some(Hit::Answer(3)), "the hits follow the scroll");
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
}
