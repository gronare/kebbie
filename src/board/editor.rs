use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::Receiver;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use ratatui_textarea::{TextArea, WrapMode};

use crate::evidence;
use crate::report;
use crate::sh::Result;
use crate::tasks;
use super::{App, Mode};
use super::terminal::{init_terminal, restore_terminal};
use super::widgets::{MAUVE, MUTED, TEXT, YELLOW, bold, centered, style};

pub(super) enum Rewrite {
    Reword(String),
    Squash,
}

// Commit message editing inside the board, like lazygit's; ctrl-o hands the text to the user's editor.
pub(super) struct MessageEditor {
    path: PathBuf,
    commits: Vec<tasks::Commit>,
    selected: usize,
    card: Option<(report::Card, bool)>,
    tests: Option<evidence::Shown>,
    scroll: super::push::Scroll,
    rewrite: Rewrite,
    pub(super) text: TextArea<'static>,
    pub(super) draft: Option<Receiver<Result<String>>>,
}

impl MessageEditor {
    pub(super) fn new(path: PathBuf, commits: Vec<tasks::Commit>, (selected, scroll): (usize, super::push::Scroll), (card, tests): (Option<(report::Card, bool)>, Option<evidence::Shown>), rewrite: Rewrite, message: &str) -> Self {
        let mut editor = Self { path, commits, selected, card, tests, scroll, rewrite, text: TextArea::default(), draft: None };
        editor.set_text(message);
        editor
    }

    fn set_text(&mut self, message: &str) {
        let mut text = TextArea::new(message.lines().map(str::to_string).collect());
        text.set_wrap_mode(WrapMode::WordOrGlyph);
        text.set_cursor_line_style(Style::new());
        text.set_style(style(TEXT));
        self.text = text;
    }

    fn message(&self) -> String {
        self.text.lines().join("\n").trim().to_string()
    }

    // The squash draft arrives from a background Claude; typing before it arrives keeps what was typed.
    pub(super) fn poll_draft(&mut self) {
        let Some(receiver) = &self.draft else { return };
        let Ok(result) = receiver.try_recv() else { return };
        self.draft = None;
        if self.message().is_empty() {
            match result {
                Ok(message) => self.set_text(&message),
                Err(error) => {
                    let joined: Vec<&str> = self.commits.iter().map(|commit| commit.message.as_str()).collect();
                    self.set_text(&joined.join("\n\n"));
                    self.text.set_placeholder_text(error);
                }
            }
        }
    }
}

// The user's editor on a commit message, in this pane; an empty result keeps the old message.
fn edit_in_editor(terminal: &mut DefaultTerminal, message: &str) -> Result<Option<String>> {
    let file = std::env::temp_dir().join(format!("kelpie-{}-COMMIT_EDITMSG", std::process::id()));
    let help = "\n\n# Edit the commit message. Lines starting with # are ignored; an empty message cancels.\n";
    std::fs::write(&file, format!("{message}{help}")).map_err(|error| error.to_string())?;
    restore_terminal();
    let status = Command::new("sh").args(["-c", "${VISUAL:-${EDITOR:-vi}} \"$1\"", "sh"]).arg(&file).status();
    *terminal = init_terminal();
    let edited = std::fs::read_to_string(&file).map_err(|error| error.to_string());
    let _ = std::fs::remove_file(&file);
    if !status.map_err(|error| error.to_string())?.success() {
        return Ok(None);
    }
    let message = edited?.lines().filter(|line| !line.starts_with('#')).collect::<Vec<_>>().join("\n").trim().to_string();
    Ok((!message.is_empty()).then_some(message))
}

impl App {
    pub(super) fn handle_editor(&mut self, mut editor: Box<MessageEditor>, key: KeyEvent, terminal: Option<&mut DefaultTerminal>) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                let MessageEditor { path, commits, selected, card, tests, scroll, .. } = *editor;
                self.mode = Mode::ConfirmPush { path, commits, selected, card, tests, scroll };
            }
            KeyCode::Char('s') if control => {
                let message = editor.message();
                if message.is_empty() {
                    self.mode = Mode::EditMessage(editor);
                    return;
                }
                let MessageEditor { path, selected, rewrite, card, tests, scroll, .. } = *editor;
                let result = match &rewrite {
                    Rewrite::Reword(sha) => tasks::reword(&path, sha, &message),
                    Rewrite::Squash => tasks::squash(&path, &message),
                }.and_then(|_| tasks::commits(&path));
                match result {
                    Ok(fresh) => {
                        let (selected, scroll) = if matches!(rewrite, Rewrite::Squash) { (0, super::push::Scroll::default()) } else { (selected.min(fresh.len().saturating_sub(1)), scroll) };
                        self.mode = Mode::ConfirmPush { path, commits: fresh, selected, card, tests, scroll };
                    }
                    Err(error) => self.show(Err(error)),
                }
            }
            KeyCode::Char('o') if control => {
                if let Some(Ok(Some(message))) = terminal.map(|terminal| edit_in_editor(terminal, &editor.message())) {
                    editor.set_text(&message);
                }
                self.mode = Mode::EditMessage(editor);
            }
            _ => {
                editor.text.input(key);
                self.mode = Mode::EditMessage(editor);
            }
        }
    }
}

pub(super) fn draw_editor(frame: &mut Frame, editor: &MessageEditor) {
    let area = centered(frame.area(), 80, frame.area().height.saturating_sub(4).min(24));
    frame.render_widget(Clear, area);
    let title = match editor.rewrite { Rewrite::Reword(_) => " commit message ", Rewrite::Squash => " squashed commit message " };
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(title, bold(MAUVE)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [text_area, footer] = Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(inner);
    frame.render_widget(&editor.text, text_area);
    let subject = editor.text.lines().first().map(|line| line.chars().count()).unwrap_or(0);
    let count = Span::styled(format!("subject {subject}/72  "), style(if subject > 72 { YELLOW } else { MUTED }));
    let keys = if editor.draft.is_some() { "drafting…  " } else { "" };
    frame.render_widget(Paragraph::new(vec![
        Line::default(),
        Line::from(vec![count, Span::styled(keys, style(YELLOW)), Span::styled("ctrl-s save · esc back · ctrl-o open in your editor", style(MUTED))]),
    ]), footer);
}
