use std::io::Write;

use ratatui::crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::DefaultTerminal;

use crate::sh::Result;

// Bracketed paste makes a paste, and so a dropped file's path, arrive as one paste rather than as keys the board would act on.
fn modes_on(out: &mut impl Write) {
    let _ = execute!(out, EnableMouseCapture, EnableBracketedPaste);
}

fn modes_off(out: &mut impl Write) {
    let _ = execute!(out, DisableMouseCapture, DisableBracketedPaste);
}

pub fn init_terminal() -> DefaultTerminal {
    let terminal = ratatui::init();
    static MODES_OFF_ON_PANIC: std::sync::Once = std::sync::Once::new();
    MODES_OFF_ON_PANIC.call_once(|| {
        let restore = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            modes_off(&mut std::io::stdout());
            restore(info);
        }));
    });
    modes_on(&mut std::io::stdout());
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
