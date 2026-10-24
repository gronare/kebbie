use std::io::Write;

use ratatui::crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{Clear, ClearType, EnterAlternateScreen};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::CellDiffOption;
use ratatui::{DefaultTerminal, Terminal};

use crate::sh::Result;

// Bracketed paste makes a paste, and so a dropped file's path, arrive as one paste rather than as keys the board would act on.
fn modes_on(out: &mut impl Write) {
    let _ = execute!(out, EnableMouseCapture, EnableBracketedPaste);
}

fn modes_off(out: &mut impl Write) {
    let _ = execute!(out, DisableMouseCapture, DisableBracketedPaste);
}

pub fn init_terminal() -> DefaultTerminal {
    open(false)
}

fn open(renewed: bool) -> DefaultTerminal {
    let _ = ratatui::crossterm::terminal::enable_raw_mode();
    restore_on_panic();
    enter(&mut std::io::stdout(), renewed);
    match Terminal::new(CrosstermBackend::new(std::io::stdout())) {
        Ok(terminal) if renewed => repaint_all(terminal),
        Ok(terminal) => terminal,
        Err(_) => ratatui::init(),
    }
}

// The old board left the alternate screen on, and entering it again makes herdr's terminal erase it, so a renewed board neither enters nor clears it.
fn enter(out: &mut impl Write, renewed: bool) {
    if !renewed {
        let _ = execute!(out, EnterAlternateScreen, Clear(ClearType::All));
    }
    modes_on(out);
}

fn restore_on_panic() {
    static RESTORE_ON_PANIC: std::sync::Once = std::sync::Once::new();
    RESTORE_ON_PANIC.call_once(|| {
        let restore = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            modes_off(&mut std::io::stdout());
            ratatui::restore();
            restore(info);
        }));
    });
}

pub fn continue_terminal() -> DefaultTerminal {
    open(true)
}

// The new board's first frame must write every cell, as the screen still holds the old board's.
fn repaint_all<B: Backend>(mut terminal: Terminal<B>) -> Terminal<B> {
    for cell in &mut terminal.current_buffer_mut().content {
        cell.set_diff_option(CellDiffOption::AlwaysUpdate);
    }
    terminal.swap_buffers();
    terminal
}

pub fn restore_terminal() {
    modes_off(&mut std::io::stdout());
    ratatui::restore();
}

// Leaves the TUI for a step that prints as it goes (creating a worktree takes a while), then comes back; the board shows the result.
pub fn outside<T>(terminal: &mut DefaultTerminal, step: impl FnOnce() -> Result<T>) -> Result<T> {
    restore_terminal();
    let result = step();
    *terminal = init_terminal();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_renewed_board_paints_over_the_shell_left_on_the_screen() {
        use ratatui::backend::TestBackend;
        use ratatui::widgets::Paragraph;
        let shell = ["lead 5m41s ❯ kelpie", "board ❯ kelpie boar", "~ ❯ ls -la /Users/g"];
        let mut terminal = repaint_all(Terminal::new(TestBackend::with_lines(shell)).unwrap());
        terminal.draw(|frame| frame.render_widget(Paragraph::new("Work"), frame.area())).unwrap();
        terminal.backend().assert_buffer_lines(["Work               ", "                   ", "                   "]);
        terminal.draw(|frame| frame.render_widget(Paragraph::new("Backlog"), frame.area())).unwrap();
        terminal.backend().assert_buffer_lines(["Backlog            ", "                   ", "                   "]);
    }

    #[test]
    fn a_renewed_board_neither_enters_nor_clears_the_alternate_screen_the_old_one_left_on() {
        let (mut fresh, mut renewed) = (Vec::new(), Vec::new());
        enter(&mut fresh, false);
        enter(&mut renewed, true);
        let (fresh, renewed) = (String::from_utf8(fresh).unwrap(), String::from_utf8(renewed).unwrap());
        assert!(fresh.contains("\x1b[?1049h") && fresh.contains("\x1b[2J"), "{fresh:?}");
        assert!(!renewed.contains("\x1b[?1049h") && !renewed.contains("\x1b[2J"), "herdr erases the pane on either, and it blinks: {renewed:?}");
        assert!(renewed.contains("\x1b[?2004h") && renewed.contains("\x1b[?1000h"), "{renewed:?}");
    }

    #[test]
    fn the_board_turns_bracketed_paste_on_with_the_mouse_and_off_again() {
        let (mut on, mut off) = (Vec::new(), Vec::new());
        modes_on(&mut on);
        modes_off(&mut off);
        let (on, off) = (String::from_utf8(on).unwrap(), String::from_utf8(off).unwrap());
        assert!(on.contains("\x1b[?2004h") && on.contains("\x1b[?1000h"), "{on:?}");
        assert!(off.contains("\x1b[?2004l") && off.contains("\x1b[?1000l"), "{off:?}");
        let source = include_str!("terminal.rs");
        let restore = &source[source.find("pub fn restore_terminal").unwrap()..];
        assert!(restore[..restore.find('}').unwrap()].contains("modes_off("));
        let hook = &source[source.find("set_hook").unwrap()..];
        assert!(hook[..hook.find("restore(info)").unwrap()].contains("modes_off("), "a panic leaves the terminal without paste or mouse modes");
    }
}
