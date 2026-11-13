use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::{DefaultTerminal, Frame};

use crate::backlog::Entry;
use crate::backlog_file::{Question, State, questions_in, shape_of, state_of};
use crate::backlog_kept::Answer;
use crate::keys::{self, Act};
use crate::notice::{Notice, Tone};
use crate::sh::Result;
use super::widgets::{FAINT, MAUVE, MUTED, PEACH, SUBTEXT, SURFACE, TEAL, TEXT, YELLOW, bold, modal, plural, style, text_width, wrap};
use super::{App, Mode};

const WIDTH: u16 = 88;
const LABEL: usize = 10;
const PAGE: u16 = 10;
const FRAME_ROWS: u16 = 8;
const CARD_KEYS: &str = "a answer · g start · w write it out again · ↑↓ pick · pgdn pgup scroll · esc back";
const LOCAL_KEYS: &str = "a answer · g start · w write it out again · e open in $EDITOR · ↑↓ pick · pgdn pgup scroll · esc back";
const TYPING_KEYS: &str = "⏎ send to the lead · pgup pgdn scroll · esc back to the card";
const UNREAD: &str = "The note's text has not reached this machine yet; the card shows it once the board has read it from the vault.";
pub(super) const GO_AGAIN_FOR: std::time::Duration = std::time::Duration::from_secs(3);

// The selected backlog row's right field, the word in it that is tinted, its line under the row while the lead works on it, and its about line when selected.
pub(super) fn row_facts(entry: &Entry) -> (String, Option<(String, Color)>) {
    let with_age = |word: String| if entry.age.is_empty() { word } else { format!("{word} · {}", entry.age) };
    match entry.shape.state {
        Some(State::Drafting) if entry.shape.waiting > 0 => {
            let questions = plural(entry.shape.waiting, "question");
            (with_age(questions.clone()), Some((questions, YELLOW)))
        }
        Some(State::Drafting) => (with_age("drafting".to_string()), None),
        Some(State::ToWrite) => (with_age("to write".to_string()), None),
        _ => (entry.age.clone(), None),
    }
}

pub(super) fn lead_line(entry: &Entry) -> Option<&'static str> {
    match entry.shape.state {
        Some(State::ToWrite) if entry.shape.kept => Some("the lead files it in the vault"),
        Some(State::ToWrite) => Some("the lead writes it out"),
        Some(State::Drafting) if entry.shape.waiting == 0 => Some("the lead is writing it out from the code, the notes and your chat"),
        _ => None,
    }
}

fn waits(count: usize) -> String {
    if count == 1 { "1 question waits".to_string() } else { format!("{count} questions wait") }
}

pub(super) fn about(entry: &Entry) -> String {
    match entry.shape.state {
        Some(State::Drafting) if entry.shape.waiting > 0 => format!("drafting: {} on you before it can start", waits(entry.shape.waiting)),
        Some(State::Drafting) => "drafting: the lead writes it out".to_string(),
        Some(State::ToWrite) if entry.shape.kept => "to write: the lead writes it out and files it in the vault".to_string(),
        Some(State::ToWrite) => "to write: the lead writes it out".to_string(),
        _ => entry.summary.clone(),
    }
}

pub(super) fn has_card(entry: &Entry) -> bool {
    matches!(entry.source.as_str(), "local" | "vault")
}

// Starting an item whose questions wait on the user is refused once; the same key again within GO_AGAIN_FOR starts it anyway.
pub(super) fn go_refusal(entry: &Entry) -> Option<String> {
    let waiting = entry.shape.waiting;
    (waiting > 0).then(|| format!("Not started: {} on you. Answer them on its card, or press {} again to start anyway.", waits(waiting), keys::map().label(Act::Go)))
}

pub(super) struct ItemCard {
    pub(super) entry: Entry,
    pub(super) path: Option<PathBuf>,
    pub(super) priority: String,
    pub(super) repo: String,
    pub(super) filed: String,
    pub(super) drafting: bool,
    pub(super) short: bool,
    pub(super) body: String,
    pub(super) answers: Vec<Answer>,
    pub(super) pick: usize,
    pub(super) scroll: u16,
    pub(super) follow: bool,
    pub(super) typing: Option<String>,
}

impl ItemCard {
    pub(super) fn new(entry: Entry, body: String, answers: Vec<Answer>) -> Self {
        let (priority, repo) = (entry.priority.clone(), entry.area.clone());
        let mut card = Self { entry, path: None, priority, repo, filed: String::new(), drafting: false, short: false, body, answers, pick: 0, scroll: 0, follow: true, typing: None };
        card.pick = card.next_open(0);
        card
    }

    // The item as it is now: a local item from its file, a vault note from the index or kebbie's copy, the answers still with the lead from kebbie's state.
    pub(super) fn load(entry: Entry, state: &Path) -> Option<Self> {
        let mut card = Self::new(entry, String::new(), Vec::new());
        card.reload(state)?;
        card.pick = card.next_open(0);
        Some(card)
    }

    pub(super) fn reload(&mut self, state: &Path) -> Option<()> {
        match self.entry.source.as_str() {
            "local" => {
                let (path, item) = crate::backlog_file::find(&self.entry.reference)?;
                (self.drafting, self.short) = (item.status == "drafting", item.status == "draft");
                (self.path, self.priority, self.repo, self.filed, self.body) = (Some(path), item.priority, item.repo, item.filed, item.body);
                self.entry.title = item.title;
            }
            "vault" => {
                let index = crate::vault::Index::open().ok();
                self.drafting = index.as_ref().and_then(|index| index.status_of(&self.entry.reference)).as_deref() == Some("draft");
                self.body = crate::backlog::vault_body(index.as_ref(), state, &self.entry.reference).unwrap_or_default();
            }
            _ => return None,
        }
        self.answers = crate::backlog_kept::settle(state, &self.entry.reference, &self.body);
        self.pick = self.pick.min(self.questions().len().saturating_sub(1));
        Some(())
    }

    pub(super) fn questions(&self) -> Vec<Question> {
        questions_in(&self.body)
    }

    fn answer_to(&self, question: &Question) -> Option<&Answer> {
        self.answers.iter().find(|answer| answer.question == question.text)
    }

    // The first question at or after `from` that waits on the user, else the one at `from`.
    fn next_open(&self, from: usize) -> usize {
        let questions = self.questions();
        (from..questions.len()).find(|index| self.answer_to(&questions[*index]).is_none()).unwrap_or(from.min(questions.len().saturating_sub(1)))
    }

    pub(super) fn state(&self) -> State {
        if self.body.trim().is_empty() && self.path.is_none() {
            return self.entry.shape.state.unwrap_or(State::Ready);
        }
        state_of(self.drafting, self.short, &self.body)
    }

    pub(super) fn state_text(&self) -> String {
        let answered: Vec<String> = self.answers.iter().map(|answer| answer.question.clone()).collect();
        let shape = shape_of(self.state(), &self.body, &answered, &self.filed);
        let with_lead = (shape.with_lead > 0).then(|| plural(shape.with_lead, "answer") + " with the lead");
        match self.state() {
            State::Drafting if shape.waiting > 0 => format!("drafting: {} on you{}", waits(shape.waiting), with_lead.map_or(String::new(), |text| format!(" · {text}"))),
            State::Drafting => with_lead.map_or("drafting: the lead writes it out".to_string(), |text| format!("drafting: {text}")),
            State::ToWrite => "to write: the lead writes it out".to_string(),
            State::Ready => "ready".to_string(),
        }
    }

    fn place(&self) -> (&'static str, String) {
        match &self.path {
            Some(path) => ("file", path.display().to_string()),
            None => ("note", format!("vault: {}", self.entry.reference)),
        }
    }

    pub(super) fn wherein(&self) -> String {
        self.place().1
    }
}

fn fact(label: &str, value: &str, width: usize, lines: &mut Vec<Line<'static>>) {
    if value.trim().is_empty() {
        return;
    }
    for (index, chunk) in wrap(value, width.saturating_sub(LABEL), usize::MAX).into_iter().enumerate() {
        let label = if index == 0 { format!("{label:<LABEL$}") } else { " ".repeat(LABEL) };
        lines.push(Line::from(vec![Span::styled(label, style(MUTED)), Span::styled(chunk, style(TEXT))]));
    }
}

fn question_lines(text: &str, blocks: bool, selected: bool, muted: bool, width: usize) -> Vec<Line<'static>> {
    let mark = match (blocks, muted) {
        (true, false) => Span::styled("?", bold(PEACH)),
        (true, true) => Span::styled("?", style(MUTED)),
        (false, _) => Span::styled("·", style(MUTED)),
    };
    let text_style = if muted { style(MUTED) } else if selected { bold(TEXT) } else { style(TEXT) };
    wrap(text, width.saturating_sub(3), usize::MAX).into_iter().enumerate().map(|(index, chunk)| {
        let lead = if index == 0 {
            vec![if selected { Span::styled("▌", style(MAUVE)) } else { Span::raw(" ") }, mark.clone(), Span::raw(" ")]
        } else {
            vec![Span::raw("   ")]
        };
        let line = Line::from(lead.into_iter().chain([Span::styled(chunk, text_style)]).collect::<Vec<_>>());
        if selected { line.style(Style::new().bg(SURFACE)) } else { line }
    }).collect()
}

fn paragraph(text: &str, width: usize, lines: &mut Vec<Line<'static>>) {
    if let Some(bullet) = text.strip_prefix("- ").or_else(|| text.strip_prefix("* ")) {
        for (index, chunk) in wrap(bullet, width.saturating_sub(2), usize::MAX).into_iter().enumerate() {
            lines.push(Line::from(vec![Span::styled(if index == 0 { "- " } else { "  " }, style(MUTED)), Span::styled(chunk, style(SUBTEXT))]));
        }
    } else {
        lines.extend(wrap(text, width, usize::MAX).into_iter().map(|chunk| Line::from(Span::styled(chunk, style(SUBTEXT)))));
    }
}

// The card's lines and the line each open question starts on.
pub(super) fn card_lines(card: &ItemCard, width: usize) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut lines: Vec<Line<'static>> = wrap(&card.entry.title, width, usize::MAX).into_iter().map(|chunk| Line::from(Span::styled(chunk, bold(TEXT)))).collect();
    lines.push(Line::default());
    fact("priority", &card.priority, width, &mut lines);
    fact("repo", &card.repo, width, &mut lines);
    fact("state", &card.state_text(), width, &mut lines);
    fact("filed", &card.filed, width, &mut lines);
    let (label, place) = card.place();
    fact(label, &place, width, &mut lines);
    let questions = card.questions();
    let mut starts = Vec::new();
    let mut heading = "";
    let mut asked = 0;
    lines.push(Line::default());
    for raw in card.body.lines() {
        let line = raw.trim();
        if let Some(name) = line.strip_prefix("## ") {
            if lines.last().is_some_and(|last| last.width() > 0) {
                lines.push(Line::default());
            }
            heading = if name.trim().is_empty() { " " } else { name.trim() };
            let mut spans = vec![Span::styled(name.trim().to_string(), bold(TEAL))];
            if name.trim() == "Open questions" {
                let waiting = questions.iter().filter(|question| question.blocks && card.answer_to(question).is_none()).count();
                let extra = if waiting == 0 { "none blocks the start".to_string() } else { format!("{} on you before it starts", if waiting == 1 { "1 waits".to_string() } else { format!("{waiting} wait") }) };
                spans.push(Span::styled(format!("  {extra}"), style(MUTED)));
            }
            lines.push(Line::from(spans));
            continue;
        }
        if line.is_empty() || line.starts_with("# ") {
            continue;
        }
        if heading == "Open questions" && let Some(question) = questions.get(asked).filter(|question| line.ends_with(question.text.as_str()) && (line.starts_with("- ") || line.starts_with("* "))) {
            starts.push(lines.len());
            let answer = card.answer_to(question);
            let text = answer.map_or(question.text.clone(), |answer| format!("with the lead: \"{}\"", answer.answer));
            lines.extend(question_lines(&text, question.blocks, asked == card.pick, answer.is_some(), width));
            asked += 1;
            continue;
        }
        paragraph(line, width, &mut lines);
    }
    if card.body.trim().is_empty() {
        paragraph(UNREAD, width, &mut lines);
    }
    while lines.last().is_some_and(|last| last.width() == 0) {
        lines.pop();
    }
    (lines, starts)
}

fn answer_box(question: &str, typed: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![Span::styled("ANSWER", bold(TEAL)), Span::styled(format!(" {}", "─".repeat(width.saturating_sub(7))), style(FAINT))])];
    lines.extend(wrap(question, width, usize::MAX).into_iter().map(|chunk| Line::from(Span::styled(chunk, bold(TEXT)))));
    lines.push(Line::from(Span::styled(format!("╭{}╮", "─".repeat(width.saturating_sub(2))), style(MAUVE))));
    let chunks = if typed.is_empty() { vec![String::new()] } else { wrap(typed, width.saturating_sub(3), usize::MAX) };
    let last = chunks.len() - 1;
    for (index, chunk) in chunks.into_iter().enumerate() {
        let used = text_width(&chunk) + usize::from(index == last);
        let mut spans = vec![Span::styled("│", style(MAUVE)), Span::styled(chunk, style(TEXT))];
        if index == last {
            spans.push(Span::styled(" ", Style::new().bg(TEXT)));
        }
        spans.push(Span::raw(" ".repeat(width.saturating_sub(2 + used))));
        spans.push(Span::styled("│", style(MAUVE)));
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(Span::styled(format!("╰{}╯", "─".repeat(width.saturating_sub(2))), style(MAUVE))));
    lines
}

pub(super) fn draw_item_card(frame: &mut Frame, card: &mut ItemCard) {
    let width = WIDTH.min(frame.area().width.saturating_sub(4));
    let inner = usize::from(width.saturating_sub(4)).max(20);
    let (rows, starts) = card_lines(card, inner);
    let keys = match (&card.typing, &card.path) {
        (Some(_), _) => TYPING_KEYS,
        (None, Some(_)) => LOCAL_KEYS,
        (None, None) => CARD_KEYS,
    };
    let key_lines: Vec<Line<'static>> = wrap(keys, inner, usize::MAX).into_iter().map(|chunk| Line::from(Span::styled(chunk, style(MUTED)))).collect();
    let tail = match (&card.typing, card.questions().get(card.pick)) {
        (Some(typed), Some(question)) => {
            let mut tail = answer_box(&question.text, typed, inner);
            tail.push(Line::default());
            tail
        }
        _ => Vec::new(),
    };
    let room = usize::from(frame.area().height.saturating_sub(FRAME_ROWS)).saturating_sub(key_lines.len() + tail.len()).max(3);
    let view = room.min(rows.len());
    let last = rows.len().saturating_sub(view);
    let mut scroll = usize::from(card.scroll).min(last);
    if card.follow && let Some(at) = starts.get(card.pick) {
        if *at < scroll {
            scroll = *at;
        } else if *at + 2 > scroll + view {
            scroll = (*at + 2).saturating_sub(view).min(last);
        }
    }
    card.scroll = scroll as u16;
    let mut lines: Vec<Line<'static>> = rows.into_iter().skip(scroll).take(view).collect();
    lines.push(Line::default());
    lines.extend(tail);
    lines.extend(key_lines);
    modal(frame, "backlog item".to_string(), lines, width);
}

impl App {
    pub(super) fn open_item_card(&mut self, entry: Entry) -> bool {
        if !has_card(&entry) {
            return false;
        }
        let Ok(state) = crate::lead::state_dir() else { return false };
        match ItemCard::load(entry, &state) {
            Some(card) => {
                self.mode = Mode::ItemCard(Box::new(card));
                true
            }
            None => false,
        }
    }

    pub(super) fn handle_item_card(&mut self, mut card: Box<ItemCard>, event: KeyEvent, terminal: Option<&mut DefaultTerminal>) {
        if let Some(typed) = card.typing.take() {
            return self.handle_answer(card, typed, event);
        }
        let count = card.questions().len();
        match (event.code, keys::map().act(self.ctx(), &event)) {
            (KeyCode::Esc | KeyCode::Char('q'), _) => return,
            (KeyCode::Down | KeyCode::Char('j'), _) if count > 0 => (card.pick, card.follow) = ((card.pick + 1).min(count - 1), true),
            (KeyCode::Up | KeyCode::Char('k'), _) if count > 0 => (card.pick, card.follow) = (card.pick.saturating_sub(1), true),
            (KeyCode::Down | KeyCode::Char('j'), _) => card.scroll = card.scroll.saturating_add(1),
            (KeyCode::Up | KeyCode::Char('k'), _) => card.scroll = card.scroll.saturating_sub(1),
            (KeyCode::PageDown | KeyCode::Char(' '), _) => (card.scroll, card.follow) = (card.scroll.saturating_add(PAGE), false),
            (KeyCode::PageUp, _) => (card.scroll, card.follow) = (card.scroll.saturating_sub(PAGE), false),
            (KeyCode::Char('a') | KeyCode::Enter, _) if count > 0 => (card.typing, card.follow) = (Some(String::new()), true),
            (_, Some(Act::Go)) => return self.start_entry(card.entry.clone()),
            (KeyCode::Char('w'), _) => {
                let result = crate::messages::to_lead(crate::messages::ToLead::BacklogItem, &rewrite_text(&card));
                self.subject = Some(super::notices::Subject::plain("backlog"));
                return self.show(result.and_then(|_| Notice::new(Tone::Moving, "Sent to the lead").next("It writes the item out again; the card shows it once it is saved.").ok()));
            }
            (KeyCode::Char('e'), _) if card.path.is_some() => {
                if let (Some(terminal), Some(path)) = (terminal, card.path.clone()) {
                    let _ = super::terminal::outside(terminal, || std::process::Command::new("sh").args(["-c", "${VISUAL:-${EDITOR:-vi}} \"$1\"", "sh"]).arg(&path).status().map_err(|error| error.to_string()));
                    if let Ok(state) = crate::lead::state_dir() {
                        card.reload(&state);
                    }
                }
            }
            _ => {}
        }
        self.mode = Mode::ItemCard(card);
    }

    fn handle_answer(&mut self, mut card: Box<ItemCard>, mut typed: String, event: KeyEvent) {
        match event.code {
            KeyCode::Esc => {}
            KeyCode::Enter if typed.trim().is_empty() => card.typing = Some(typed),
            KeyCode::Enter => {
                let sent = crate::lead::state_dir().and_then(|state| send_answer(&state, &mut card, typed.trim(), &mut |text| crate::messages::to_lead(crate::messages::ToLead::BacklogItem, text)));
                if let Err(error) = sent {
                    self.subject = Some(super::notices::Subject::plain("backlog"));
                    return self.show(Err(error));
                }
            }
            KeyCode::Backspace => {
                typed.pop();
                card.typing = Some(typed);
            }
            KeyCode::PageDown => (card.scroll, card.follow, card.typing) = (card.scroll.saturating_add(PAGE), false, Some(typed)),
            KeyCode::PageUp => (card.scroll, card.follow, card.typing) = (card.scroll.saturating_sub(PAGE), false, Some(typed)),
            KeyCode::Char(character) => {
                typed.push(character);
                card.typing = Some(typed);
            }
            _ => card.typing = Some(typed),
        }
        self.mode = Mode::ItemCard(card);
    }

    // g on an item: its questions waiting on the user hold the start once, and g again within the notice's time starts it anyway.
    pub(super) fn start_entry(&mut self, entry: Entry) {
        if entry.shape.kept {
            return self.say("Not started: the lead has yet to write it out and file it in the vault".to_string());
        }
        if let Some(refusal) = go_refusal(&entry) {
            let again = self.go_again.take().is_some_and(|(reference, at)| reference == entry.reference && at.elapsed() < GO_AGAIN_FOR);
            if !again {
                self.go_again = Some((entry.reference.clone(), std::time::Instant::now()));
                return self.say(refusal);
            }
        }
        if !self.ask_if_claimed(&entry) {
            let buffer = super::columns::suggested_name(&entry.title);
            self.mode = Mode::Input { purpose: super::input::Purpose::BacklogName(entry), label: "Start as task: name".to_string(), buffer };
        }
    }
}

fn rewrite_text(card: &ItemCard) -> String {
    format!("The user asks you to write out the backlog item \"{}\" ({}) again with the skill `{}`: read it, the code and the notes again, and rewrite it in place.", card.entry.title, card.wherein(), crate::backlog_file::SKILL)
}

// The answer is kept with the item until the lead folds it in, and the lead hears it at once; the pick moves on to the next question waiting.
pub(super) fn send_answer(state: &Path, card: &mut ItemCard, answer: &str, tell: &mut dyn FnMut(&str) -> Result<()>) -> Result<()> {
    let question = card.questions().get(card.pick).cloned().ok_or("This item has no open question to answer.")?;
    let given = Answer { item: card.entry.reference.clone(), question: question.text.clone(), answer: answer.to_string(), at: crate::messages::now() };
    crate::backlog_kept::answer(state, given.clone())?;
    tell(&crate::backlog_kept::answer_text("The user", &card.entry.title, &card.wherein(), &question.text, answer))?;
    card.answers.retain(|other| other.question != given.question);
    card.answers.push(given);
    card.pick = card.next_open(card.pick);
    card.follow = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog_file::Shape;
    use crate::board::test_support::*;

    const BODY: &str = "## Background\n\nA red run waits under Needs you.\n\n## Goal\n\nA flaky red run is rerun once by itself.\n\n## What we know\n\n- G reruns a red run.\n\n## Open questions\n\n- [ ] Which red runs count as flaky?\n- [ ] Should a rerun show on the row?\n- How long to wait; the work settles it.\n\n## Out of scope\n\nRetrying a refused push.\n\n## Source\n\nYou, 2026-11-12, on the board: \"ci push retry flaky\"";

    fn entry(reference: &str, shape: Shape) -> Entry {
        Entry { source: "vault".to_string(), title: "Retry a failed CI push".to_string(), priority: "high".to_string(), area: "web".to_string(), reference: reference.to_string(), age: "0 d".to_string(), shape, ..Entry::default() }
    }

    fn drafting(waiting: usize) -> Shape {
        Shape { state: Some(State::Drafting), waiting, ..Shape::default() }
    }

    fn card(reference: &str) -> ItemCard {
        ItemCard::new(entry(reference, drafting(2)), BODY.to_string(), Vec::new())
    }

    fn screen(card: &mut ItemCard) -> Vec<String> {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(124, 46)).unwrap();
        terminal.draw(|frame| draw_item_card(frame, card)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..46).map(|y| (0..124).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>()).collect()
    }

    #[test]
    fn a_row_says_its_state_in_its_right_field_and_under_it_while_the_lead_works() {
        let questions = entry("Projects/a.md", Shape { state: Some(State::Drafting), waiting: 2, ..Shape::default() });
        assert_eq!(row_facts(&questions), ("2 questions · 0 d".to_string(), Some(("2 questions".to_string(), YELLOW))));
        assert_eq!(about(&questions), "drafting: 2 questions wait on you before it can start");
        assert_eq!(lead_line(&questions), None, "the user, not the lead, holds it");
        let one = entry("Projects/a.md", drafting(1));
        assert_eq!(about(&one), "drafting: 1 question waits on you before it can start");
        let writing = entry("Projects/b.md", drafting(0));
        assert_eq!((row_facts(&writing).0.as_str(), about(&writing).as_str()), ("drafting · 0 d", "drafting: the lead writes it out"));
        assert_eq!(lead_line(&writing), Some("the lead is writing it out from the code, the notes and your chat"));
        let kept = Entry { source: "kept".to_string(), shape: Shape { state: Some(State::ToWrite), kept: true, ..Shape::default() }, ..entry("kept:abc123", Shape::default()) };
        assert_eq!((row_facts(&kept).0.as_str(), lead_line(&kept), about(&kept).as_str()), ("to write · 0 d", Some("the lead files it in the vault"), "to write: the lead writes it out and files it in the vault"));
        assert!(!has_card(&kept) && has_card(&writing));
        let ready = Entry { summary: "You stay signed in.".to_string(), age: "6 d".to_string(), shape: Shape { state: Some(State::Ready), ..Shape::default() }, ..entry("local:x", Shape::default()) };
        assert_eq!((row_facts(&ready).0.as_str(), about(&ready).as_str(), lead_line(&ready)), ("6 d", "You stay signed in.", None));
    }

    #[test]
    fn the_card_shows_the_facts_the_sections_and_each_question_by_whether_it_blocks() {
        let mut shown = card("Projects/retry.md");
        let rows = screen(&mut shown);
        let all = rows.join("\n");
        for wanted in [" backlog item ", "Retry a failed CI push", "priority  high", "repo      web", "state     drafting: 2 questions wait on you", "note      vault: Projects/retry.md", "Background", "- G reruns a red run.", "Open questions  2 wait on you before it starts", "▌? Which red runs count as flaky?", "  ? Should a rerun show on the row?", "  · How long to wait; the work settles it.", "Out of scope", "a answer · g start · w write it out again · ↑↓ pick · pgdn pgup scroll · esc back"] {
            assert!(all.contains(wanted), "{wanted}:\n{all}");
        }
        assert!(!all.contains("$EDITOR"), "a vault note opens in no editor here");
        shown.path = Some(PathBuf::from("/c/backlog/retry.md"));
        assert!(screen(&mut shown).join("\n").contains("e open in $EDITOR"));
    }

    #[test]
    fn an_answer_waits_with_the_lead_until_the_lead_folds_it_in() {
        let state = crate::sh::test_dir("item-card-answer");
        let mut shown = card("Projects/answer.md");
        shown.typing = Some("Only the e2e job.".to_string());
        let typing = screen(&mut shown).join("\n");
        assert!(typing.contains("ANSWER") && typing.contains("│Only the e2e job.") && typing.contains("⏎ send to the lead · pgup pgdn scroll · esc back to the card"), "{typing}");
        shown.typing = None;
        let mut told = Vec::new();
        send_answer(&state, &mut shown, "Only the e2e job.", &mut |text| { told.push(text.to_string()); Ok(()) }).unwrap();
        assert!(told[0].contains("Question: \"Which red runs count as flaky?\" Answer: \"Only the e2e job.\"") && told[0].contains("kebbie:write-backlog-item"), "{told:?}");
        assert_eq!(shown.pick, 1, "the pick moves on to the next question waiting");
        let sent = screen(&mut shown).join("\n");
        assert!(sent.contains("?  with the lead: \"Only the e2e job.\"") || sent.contains("? with the lead: \"Only the e2e job.\""), "{sent}");
        assert!(sent.contains("state     drafting: 1 question waits on you · 1 answer with the lead") && sent.contains("▌? Should a rerun show on the row?"), "{sent}");
        let kept = crate::backlog_kept::settle(&state, "Projects/answer.md", BODY);
        assert_eq!(kept.len(), 1, "kept while the question stands");
        let folded = BODY.replace("- [ ] Which red runs count as flaky?\n", "");
        assert!(crate::backlog_kept::settle(&state, "Projects/answer.md", &folded).is_empty(), "folded in, it leaves");
        let mut after = ItemCard::new(entry("Projects/answer.md", drafting(1)), folded, Vec::new());
        assert!(screen(&mut after).join("\n").contains("state     drafting: 1 question waits on you\n") || screen(&mut after).join("\n").contains("drafting: 1 question waits on you "));
    }

    #[test]
    fn g_on_an_item_with_questions_waiting_is_refused_once_and_starts_on_g_again() {
        let mut app = test_app(Vec::new());
        app.column = super::super::Column::Backlog;
        app.backlog.entries = vec![entry("Projects/rounding.md", drafting(2))];
        press(&mut app, key(KeyCode::Char('g')));
        assert!(matches!(app.mode, Mode::Normal), "not started");
        assert_eq!(app.notice.as_ref().map(|(text, _)| text.as_str()), Some("Not started: 2 questions wait on you. Answer them on its card, or press g again to start anyway."));
        let (rows, buffer) = render(&mut app, 124, 30);
        let (x, y) = find(&rows, "Not started: 2 questions").first().copied().unwrap();
        assert!(coloured(&buffer[(x as u16, y as u16)], (0xfa, 0xb3, 0x87)), "peach on the status line");
        press(&mut app, key(KeyCode::Char('g')));
        assert!(matches!(&app.mode, Mode::Input { label, buffer, .. } if label == "Start as task: name" && buffer == "retry-failed-ci-push"), "g again starts it anyway");
        app.mode = Mode::Normal;
        app.go_again = Some(("Projects/rounding.md".to_string(), std::time::Instant::now() - GO_AGAIN_FOR));
        press(&mut app, key(KeyCode::Char('g')));
        assert!(matches!(app.mode, Mode::Normal), "a g after the notice is gone asks again");
        app.backlog.entries = vec![entry("Projects/ready.md", Shape { state: Some(State::Ready), ..Shape::default() })];
        press(&mut app, key(KeyCode::Char('g')));
        assert!(matches!(app.mode, Mode::Input { .. }), "a ready item starts at once");
    }

    #[test]
    fn n_with_the_vault_keeps_a_row_until_the_lead_files_the_note() {
        let mut app = test_app(Vec::new());
        app.sources = vec![crate::config::Source::Vault];
        app.column = super::super::Column::Backlog;
        app.backlog.entries = vec![entry("Projects/other.md", Shape::default())];
        press(&mut app, key(KeyCode::Char('n')));
        assert!(matches!(&app.mode, Mode::Input { label, .. } if label == "Add to the backlog: a line, the lead writes it out"));
        for character in "ci push retry flaky kept-row-test".chars() {
            press(&mut app, key(KeyCode::Char(character)));
        }
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::Input { label, .. } if label == "Priority: urgent, high, medium or low (empty for none)"));
        for character in "high".chars() {
            press(&mut app, key(KeyCode::Char(character)));
        }
        press(&mut app, key(KeyCode::Enter));
        let toast = app.toast.as_ref().map(|toast| toast.notice.clone()).unwrap();
        assert_eq!((toast.headline.as_str(), toast.next.as_deref()), ("Sent to the lead", Some("It writes the item out and files it in the vault. Its row waits here until the note replaces it.")));
        let state = crate::lead::state_dir().unwrap();
        let kept = crate::backlog_kept::kept_in(&state).into_iter().find(|kept| kept.line.ends_with("kept-row-test")).expect("kebbie keeps a row for it");
        assert_eq!(kept.priority, "high");
        assert!(crate::backlog_file::filed(&format!("kept:{}", kept.id)).unwrap().contains("Its row gives way to the note"));
        assert!(!crate::backlog_kept::kept_in(&state).iter().any(|other| other.id == kept.id), "the lead's word clears it");
    }

    #[test]
    fn enter_opens_the_card_and_o_the_options_in_the_backlog_column() {
        let mut app = test_app(Vec::new());
        app.column = super::super::Column::Backlog;
        app.backlog.entries = vec![Entry { source: "linear".to_string(), reference: "ABC-1".to_string(), ..entry("ABC-1", Shape::default()) }];
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Options(_)), "a Linear issue has no card: ⏎ is its options");
        app.mode = Mode::Normal;
        press(&mut app, key(KeyCode::Char('o')));
        assert!(matches!(app.mode, Mode::Options(_)), "o opens an item's options");
        app.mode = Mode::Normal;
        app.column = super::super::Column::Work;
        press(&mut app, key(KeyCode::Char('o')));
        assert!(matches!(app.mode, Mode::Normal), "o means nothing under Work");
    }
}
