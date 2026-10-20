use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::DefaultTerminal;

use crate::sh::Result;

pub fn init_terminal() -> DefaultTerminal {
    let terminal = ratatui::init();
    static MOUSE_OFF_ON_PANIC: std::sync::Once = std::sync::Once::new();
    MOUSE_OFF_ON_PANIC.call_once(|| {
        let restore = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(std::io::stdout(), DisableMouseCapture);
            restore(info);
        }));
    });
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    terminal
}

pub fn restore_terminal() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
}

// Leaves the TUI for a step that prints as it goes (creating a worktree takes a while), then comes back; the board shows the result.
pub fn outside<T>(terminal: &mut DefaultTerminal, step: impl FnOnce() -> Result<T>) -> Result<T> {
    restore_terminal();
    let result = step();
    *terminal = init_terminal();
    result
}
