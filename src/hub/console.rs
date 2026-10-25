use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal;

use super::card::{self, Colour, Copied, Row};
use super::qr::{self, Glyphs};
use crate::sh::Result;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    Open,
    Copy,
    Stop,
}

pub trait Console {
    fn write(&mut self, text: &str) -> Result<()>;
    fn colour(&self) -> Colour;
    fn live(&self) -> bool;
    fn glyphs(&self) -> Glyphs {
        qr::glyphs()
    }
    // None is the user stopping; a key typed first replaces start, so start is the answer only to a bare enter.
    fn edit(&mut self, render: &dyn Fn(&str) -> (Row, Row), start: &str) -> Result<Option<String>>;
    fn key(&mut self, wait: Duration) -> Result<Option<Key>>;
    fn keys(&mut self, _on: bool) -> Result<()> {
        Ok(())
    }
    fn copy(&mut self, text: &str) -> Result<Copied>;
    fn size(&self) -> Option<(usize, usize)> {
        None
    }
    fn arrows(&self) -> bool {
        false
    }
    // None is the user stopping; only a console whose arrows() is true is asked.
    fn pick(&mut self, _render: &dyn Fn(usize) -> Vec<Row>, _count: usize, _shortcuts: &[char]) -> Result<Option<usize>> {
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    At(usize),
    Chosen(usize),
    Stop,
}

pub fn step(selected: usize, count: usize, code: KeyCode, control: bool, shortcuts: &[char]) -> Step {
    match code {
        KeyCode::Char('c' | 'd') if control => Step::Stop,
        KeyCode::Esc | KeyCode::Char('q') => Step::Stop,
        KeyCode::Up => Step::At(selected.saturating_sub(1)),
        KeyCode::Down => Step::At((selected + 1).min(count.saturating_sub(1))),
        KeyCode::Enter => Step::Chosen(selected),
        KeyCode::Char(typed) => shortcuts.iter().position(|key| *key == typed.to_ascii_lowercase()).map_or(Step::At(selected), Step::Chosen),
        _ => Step::At(selected),
    }
}

pub fn pick_with(write: &mut dyn FnMut(&str) -> Result<()>, next: &mut dyn FnMut() -> Result<(KeyCode, bool)>, render: &dyn Fn(usize) -> Vec<Row>, count: usize, shortcuts: &[char], colour: Colour) -> Result<Option<usize>> {
    let paint = |rows: Vec<Row>| rows.iter().map(|row| format!("{}\n", card::paint(row, colour))).collect::<String>();
    let mut selected = 0;
    let mut drawn = render(selected).len();
    write(&paint(render(selected)))?;
    loop {
        let (code, control) = next()?;
        let step = step(selected, count, code, control, shortcuts);
        let shown = match step {
            Step::At(at) => at,
            Step::Chosen(at) => at,
            Step::Stop => return Ok(None),
        };
        if shown != selected || matches!(step, Step::Chosen(_)) {
            selected = shown;
            let rows = render(selected);
            let rows = if matches!(step, Step::Chosen(_)) { rows.into_iter().take(count).collect() } else { rows };
            write(&format!("\x1b[{drawn}F\x1b[J{}", paint(rows.clone())))?;
            drawn = rows.len();
        }
        if let Step::Chosen(at) = step {
            return Ok(Some(at));
        }
    }
}

pub fn say(console: &mut dyn Console, rows: &[Row]) -> Result<()> {
    let colour = console.colour();
    let text = rows.iter().map(|row| format!("{}\n", card::paint(row, colour))).collect::<String>();
    console.write(&text)
}

pub struct Lines<R, W> {
    pub input: R,
    pub output: W,
}

impl<R: BufRead, W: Write> Console for Lines<R, W> {
    fn write(&mut self, text: &str) -> Result<()> {
        self.output.write_all(text.as_bytes()).and_then(|()| self.output.flush()).map_err(|error| error.to_string())
    }

    fn colour(&self) -> Colour {
        Colour::None
    }

    fn live(&self) -> bool {
        false
    }

    fn edit(&mut self, render: &dyn Fn(&str) -> (Row, Row), start: &str) -> Result<Option<String>> {
        let (before, after) = render(start);
        self.write(&format!("{}{}\n", card::paint(&before, Colour::None), card::paint(&after, Colour::None)))?;
        let mut answer = String::new();
        if self.input.read_line(&mut answer).map_err(|error| format!("reading the answer: {error}"))? == 0 {
            return Ok(None);
        }
        let answer = answer.trim();
        Ok(Some(if answer.is_empty() { start.to_string() } else { answer.to_string() }))
    }

    fn key(&mut self, wait: Duration) -> Result<Option<Key>> {
        std::thread::sleep(wait);
        Ok(None)
    }

    fn copy(&mut self, _: &str) -> Result<Copied> {
        Err("no terminal to copy to".to_string())
    }
}

struct Raw;

impl Raw {
    fn on() -> Result<Raw> {
        terminal::enable_raw_mode().map_err(|error| format!("reading keys from the terminal: {error}"))?;
        Ok(Raw)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

pub struct Tty {
    colour: Colour,
    raw: Option<Raw>,
}

impl Tty {
    pub fn new() -> Tty {
        Tty { colour: card::colour_for(true), raw: None }
    }

}

impl Console for Tty {
    fn write(&mut self, text: &str) -> Result<()> {
        let mut stdout = std::io::stdout();
        let text = if self.raw.is_some() { text.replace('\n', "\r\n") } else { text.to_string() };
        stdout.write_all(text.as_bytes()).and_then(|()| stdout.flush()).map_err(|error| error.to_string())
    }

    fn colour(&self) -> Colour {
        self.colour
    }

    fn live(&self) -> bool {
        true
    }

    fn size(&self) -> Option<(usize, usize)> {
        terminal::size().ok().map(|(columns, rows)| (columns as usize, rows as usize))
    }

    fn arrows(&self) -> bool {
        true
    }

    fn pick(&mut self, render: &dyn Fn(usize) -> Vec<Row>, count: usize, shortcuts: &[char]) -> Result<Option<usize>> {
        let _raw = Raw::on()?;
        let colour = self.colour;
        let mut write = |text: &str| {
            let mut stdout = std::io::stdout();
            stdout.write_all(text.replace('\n', "\r\n").as_bytes()).and_then(|()| stdout.flush()).map_err(|error| error.to_string())
        };
        let mut next = || loop {
            if let Event::Key(key) = event::read().map_err(|error| error.to_string())? && key.kind == KeyEventKind::Press {
                return Ok((key.code, key.modifiers.contains(KeyModifiers::CONTROL)));
            }
        };
        pick_with(&mut write, &mut next, render, count, shortcuts, colour)
    }

    fn edit(&mut self, render: &dyn Fn(&str) -> (Row, Row), start: &str) -> Result<Option<String>> {
        let _raw = Raw::on()?;
        let mut buffer = start.to_string();
        let mut fresh = true;
        loop {
            let (before, after) = render(&buffer);
            let back = after.iter().map(|(text, _)| crate::board::widgets::text_width(text)).sum::<usize>();
            let back = if back > 0 { format!("\x1b[{back}D") } else { String::new() };
            print!("\r\x1b[2K{}{}{back}", card::paint(&before, self.colour), card::paint(&after, self.colour));
            let _ = std::io::stdout().flush();
            let Event::Key(key) = event::read().map_err(|error| error.to_string())? else { continue };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let control = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Enter => {
                    print!("\r\x1b[2K{}\r\n", card::paint(&before, self.colour));
                    return Ok(Some(buffer));
                }
                KeyCode::Esc => return Ok(None).inspect(|_| print!("\r\n")),
                KeyCode::Char('c' | 'd') if control => return Ok(None).inspect(|_| print!("\r\n")),
                KeyCode::Char('u') if control => buffer.clear(),
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(character) if !control => {
                    if fresh {
                        buffer.clear();
                    }
                    buffer.push(character);
                }
                _ => {}
            }
            fresh = false;
        }
    }

    fn key(&mut self, wait: Duration) -> Result<Option<Key>> {
        if !event::poll(wait).map_err(|error| error.to_string())? {
            return Ok(None);
        }
        let Event::Key(key) = event::read().map_err(|error| error.to_string())? else { return Ok(None) };
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        Ok(match key.code {
            _ if key.kind != KeyEventKind::Press => None,
            KeyCode::Char('c') if control => Some(Key::Stop),
            KeyCode::Char('c' | 'C') => Some(Key::Copy),
            KeyCode::Enter => Some(Key::Open),
            KeyCode::Esc | KeyCode::Char('q') => Some(Key::Stop),
            _ => None,
        })
    }

    // Raw mode stays on while kebbie waits, so ⏎, c and esc arrive without enter; dropping the guard gives the terminal back.
    fn keys(&mut self, on: bool) -> Result<()> {
        match (on, self.raw.is_some()) {
            (true, false) => self.raw = Some(Raw::on()?),
            (false, true) => self.raw = None,
            _ => {}
        }
        Ok(())
    }

    fn copy(&mut self, text: &str) -> Result<Copied> {
        copy_here(text)
    }
}

fn set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

// Over SSH a clipboard program would fill the other machine's clipboard, so there the terminal is asked through OSC 52.
pub fn clipboard_program(os: &str, set: &dyn Fn(&str) -> bool) -> Option<(&'static str, &'static [&'static str])> {
    if set("SSH_CONNECTION") || set("SSH_TTY") {
        return None;
    }
    match os {
        "macos" => Some(("pbcopy", &[])),
        "linux" if set("WAYLAND_DISPLAY") => Some(("wl-copy", &[])),
        "linux" if set("DISPLAY") => Some(("xclip", &["-selection", "clipboard"])),
        _ => None,
    }
}

fn pipe(program: &str, args: &[&str], text: &str) -> std::io::Result<bool> {
    let mut child = Command::new(program).args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    Ok(child.wait()?.success())
}

pub fn copy(text: &str, terminal: &mut dyn Write) -> Result<Copied> {
    if let Some((program, args)) = clipboard_program(std::env::consts::OS, &set)
        && matches!(pipe(program, args, text), Ok(true))
    {
        return Ok(Copied::Confirmed);
    }
    terminal.write_all(osc52(text).as_bytes()).map_err(|error| error.to_string())?;
    Ok(Copied::ByTerminal)
}

pub fn copy_here(text: &str) -> Result<Copied> {
    let mut stdout = std::io::stdout();
    let copied = copy(text, &mut stdout);
    let _ = stdout.flush();
    copied
}

pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    bytes.chunks(3).flat_map(|chunk| {
        let value = chunk.iter().enumerate().fold(0u32, |value, (index, byte)| value | (u32::from(*byte) << (16 - 8 * index)));
        (0..4).map(move |index| if index <= chunk.len() { ALPHABET[(value >> (18 - 6 * index) & 63) as usize] as char } else { '=' })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hub::card::{Tone, piece};

    fn rows(selected: usize) -> Vec<Row> {
        let mut rows: Vec<Row> = ["kebbie hub", "your own hub"].iter().enumerate().map(|(index, what)| vec![piece(if index == selected { "▸ " } else { "  " }, Tone::Key), piece(*what, Tone::Text)]).collect();
        rows.push(vec![piece("↑↓ ⏎", Tone::Key)]);
        rows
    }

    #[test]
    fn arrows_move_the_mark_enter_chooses_the_marked_row_shortcuts_choose_at_once_and_esc_stops() {
        assert_eq!(step(0, 2, KeyCode::Down, false, &['k', 'o']), Step::At(1));
        assert_eq!(step(1, 2, KeyCode::Down, false, &['k', 'o']), Step::At(1), "the last row stays the last");
        assert_eq!(step(0, 2, KeyCode::Up, false, &['k', 'o']), Step::At(0));
        assert_eq!(step(1, 2, KeyCode::Enter, false, &['k', 'o']), Step::Chosen(1));
        assert_eq!(step(1, 2, KeyCode::Char('k'), false, &['k', 'o']), Step::Chosen(0));
        assert_eq!(step(0, 2, KeyCode::Char('O'), false, &['k', 'o']), Step::Chosen(1));
        assert_eq!(step(0, 2, KeyCode::Esc, false, &['k', 'o']), Step::Stop);
        assert_eq!(step(0, 2, KeyCode::Char('c'), true, &['k', 'o']), Step::Stop);
        assert_eq!(step(0, 2, KeyCode::Char('x'), false, &['k', 'o']), Step::At(0));
    }

    #[test]
    fn the_rows_are_drawn_again_in_place_and_the_chosen_one_stays_without_the_hint() {
        let mut said = String::new();
        let mut keys = vec![(KeyCode::Down, false), (KeyCode::Down, false), (KeyCode::Enter, false)].into_iter();
        let chosen = pick_with(&mut |text| { said.push_str(text); Ok(()) }, &mut || Ok(keys.next().unwrap()), &rows, 2, &['k', 'o'], Colour::None).unwrap();
        assert_eq!(chosen, Some(1));
        assert_eq!(said, "▸ kebbie hub\n  your own hub\n↑↓ ⏎\n\x1b[3F\x1b[J  kebbie hub\n▸ your own hub\n↑↓ ⏎\n\x1b[3F\x1b[J  kebbie hub\n▸ your own hub\n", "a key that moves nothing draws nothing");
        let mut keys = vec![(KeyCode::Esc, false)].into_iter();
        assert_eq!(pick_with(&mut |_| Ok(()), &mut || Ok(keys.next().unwrap()), &rows, 2, &['k', 'o'], Colour::None).unwrap(), None);
    }

    #[test]
    fn a_copy_asks_the_terminal_by_osc_52_in_base64() {
        assert_eq!(osc52("hub.kebbie.dev/c#k=1"), "\x1b]52;c;aHViLmtlYmJpZS5kZXYvYyNrPTE=\x07");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b"a"), "YQ==");
    }

    #[test]
    fn the_clipboard_program_is_the_one_where_the_terminal_sits_and_none_over_ssh() {
        let env = |names: &'static [&'static str]| move |name: &str| names.contains(&name);
        assert_eq!(clipboard_program("macos", &env(&[])).map(|(program, _)| program), Some("pbcopy"));
        assert_eq!(clipboard_program("macos", &env(&["SSH_TTY"])), None);
        assert_eq!(clipboard_program("linux", &env(&["WAYLAND_DISPLAY"])).map(|(program, _)| program), Some("wl-copy"));
        assert_eq!(clipboard_program("linux", &env(&["DISPLAY"])), Some(("xclip", &["-selection", "clipboard"][..])));
        assert_eq!(clipboard_program("linux", &env(&[])), None);
    }
}
