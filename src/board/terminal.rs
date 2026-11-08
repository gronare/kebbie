use std::io::Write;

use ratatui::crossterm::event::{DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{Clear, ClearType, EnterAlternateScreen};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::CellDiffOption;
use ratatui::{DefaultTerminal, Terminal};

use crate::sh::Result;

// Bracketed paste makes a paste, and so a dropped file's path, arrive as one paste rather than as keys the board would act on.
fn modes_on(out: &mut impl Write) {
    let _ = execute!(out, EnableMouseCapture, EnableBracketedPaste, EnableFocusChange);
}

fn modes_off(out: &mut impl Write) {
    let _ = execute!(out, DisableMouseCapture, DisableBracketedPaste, DisableFocusChange);
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

// Every cell of this frame is written, blank ones too, over whatever the screen holds; no clear, so nothing blinks. Cells an image owns keep their skip.
pub fn write_every_cell(buffer: &mut ratatui::buffer::Buffer) {
    for cell in &mut buffer.content {
        if cell.diff_option == CellDiffOption::None {
            cell.set_diff_option(CellDiffOption::AlwaysUpdate);
        }
    }
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

// A new file of the user's alone, in kebbie's private state, so no other user can plant a link at its name or read the text.
fn edit_file(dir: &std::path::Path, name: &str, text: &str) -> Result<std::path::PathBuf> {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    crate::state::private_dir(dir)?;
    let file = dir.join(format!("{}-{}-{name}", std::process::id(), COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    create_private(&file, text)?;
    Ok(file)
}

fn create_private(file: &std::path::Path, text: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(file)
        .and_then(|mut out| out.write_all(text.as_bytes()))
        .map_err(|error| format!("{}: {error}", file.display()))
}

// The user's editor on a text, in this pane; the edited text, or None when the editor failed.
pub fn edit_in_editor(terminal: &mut DefaultTerminal, name: &str, text: &str) -> Result<Option<String>> {
    let file = edit_file(&crate::lead::state_dir()?.join("edit"), name, text)?;
    let status = outside(terminal, || std::process::Command::new("sh").args(["-c", "${VISUAL:-${EDITOR:-vi}} \"$1\"", "sh"]).arg(&file).status().map_err(|error| error.to_string()));
    let edited = std::fs::read_to_string(&file).map_err(|error| error.to_string());
    let _ = std::fs::remove_file(&file);
    if !status?.success() {
        return Ok(None);
    }
    edited.map(Some)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_file_an_editor_opens_is_new_private_and_refuses_a_planted_link() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::sh::test_dir("edit-file").join("edit");
        let file = super::edit_file(&dir, "COMMIT_EDITMSG", "Fix the header").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "Fix the header");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(file.file_name().unwrap().to_string_lossy().ends_with("-COMMIT_EDITMSG"), "the name tells the editor what it edits");
        assert_ne!(super::edit_file(&dir, "COMMIT_EDITMSG", "x").unwrap(), file, "a new name each time");
        let target = dir.parent().unwrap().join("victim");
        std::fs::write(&target, "keep me").unwrap();
        let planted = dir.join("planted");
        std::os::unix::fs::symlink(&target, &planted).unwrap();
        assert!(super::create_private(&planted, "x").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep me", "a link at the name is never written through");
    }

    #[test]
    fn a_frame_that_writes_every_cell_wipes_text_the_screen_kept_without_a_clear() {
        use ratatui::backend::TestBackend;
        use ratatui::widgets::Paragraph;
        let mut terminal = ratatui::Terminal::new(TestBackend::new(12, 1)).unwrap();
        terminal.draw(|frame| frame.render_widget(Paragraph::new("board"), frame.area())).unwrap();
        let stale = ratatui::buffer::Buffer::with_lines(["stale stale!"]);
        ratatui::backend::Backend::draw(terminal.backend_mut(), stale.content.iter().enumerate().map(|(x, cell)| (x as u16, 0, cell))).unwrap();
        terminal.draw(|frame| frame.render_widget(Paragraph::new("board"), frame.area())).unwrap();
        terminal.backend().assert_buffer_lines(["stale stale!"]);
        terminal.draw(|frame| {
            frame.render_widget(Paragraph::new("board"), frame.area());
            super::write_every_cell(frame.buffer_mut());
        }).unwrap();
        let shown: String = terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();
        assert_eq!(shown, "board       ");
    }

    use super::*;

    #[test]
    fn a_renewed_board_paints_over_the_shell_left_on_the_screen() {
        use ratatui::backend::TestBackend;
        use ratatui::widgets::Paragraph;
        let shell = ["lead 5m41s ❯ kebbie", "board ❯ kebbie boar", "~ ❯ ls -la /Users/g"];
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
