use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

const WAIT: Duration = Duration::from_millis(150);
const LIFT: u8 = 6;
const MOCHA_BASE: (u8, u8, u8) = (0x1e, 0x1e, 0x2e);

// OSC 11 asks for the background and DA1 follows, since every terminal answers DA1, so one without OSC 11 ends the wait at once; raw mode must be on and the event loop not yet reading.
pub(super) fn ask() -> Option<(u8, u8, u8)> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return None;
    }
    let mut tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok()?;
    tty.write_all(b"\x1b]11;?\x07\x1b[c").ok()?;
    tty.flush().ok()?;
    let mut got = Vec::new();
    let end = Instant::now() + WAIT;
    while !answered(&got) {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut ready = libc::pollfd { fd: tty.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd on a descriptor this function owns, for the length given.
        if unsafe { libc::poll(&mut ready, 1, left.as_millis() as i32) } <= 0 {
            break;
        }
        let mut chunk = [0u8; 256];
        match tty.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => got.extend_from_slice(&chunk[..read]),
        }
    }
    parse(&String::from_utf8_lossy(&got))
}

fn answered(got: &[u8]) -> bool {
    let text = String::from_utf8_lossy(got);
    text.split("\x1b[?").skip(1).any(|reply| reply.contains('c'))
}

// rgb:RRRR/GGGG/BBBB, each part one to four hex digits scaled to eight bits.
fn parse(reply: &str) -> Option<(u8, u8, u8)> {
    let rest = &reply[reply.find("]11;rgb:")? + 8..];
    let end = rest.find(['\x07', '\x1b']).unwrap_or(rest.len());
    let parts: Vec<u8> = rest[..end].split('/').map(|part| {
        let digits = part.len().clamp(1, 4) as u32;
        u32::from_str_radix(part, 16).ok().filter(|_| !part.is_empty() && part.len() <= 4).map(|value| (value * 255 / (16u32.pow(digits) - 1)) as u8)
    }).collect::<Option<_>>()?;
    match parts[..] {
        [red, green, blue] => Some((red, green, blue)),
        _ => None,
    }
}

// The same few levels on every channel keeps the background's hue; up on a dark background, down on a light one; with no answer, Mocha's base, which the board's colours already assume.
pub(super) fn focus_tint(background: Option<(u8, u8, u8)>) -> Color {
    let (red, green, blue) = background.unwrap_or(MOCHA_BASE);
    let dark = 0.299 * f32::from(red) + 0.587 * f32::from(green) + 0.114 * f32::from(blue) < 128.0;
    let shift = |part: u8| if dark { part.saturating_add(LIFT) } else { part.saturating_sub(LIFT) };
    Color::Rgb(shift(red), shift(green), shift(blue))
}

// Every cell of the pane and a cell of padding left, right and above it on the terminal's own background takes the tint; a cell with a colour of its own, such as the selection, keeps it.
pub(super) fn paint(buffer: &mut Buffer, area: Rect, tint: Color) {
    let padded = Rect::new(area.x.saturating_sub(1), area.y.saturating_sub(1), area.width + (area.x.min(1)) + 1, area.height + area.y.min(1));
    let area = padded.intersection(buffer.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buffer[(x, y)];
            if cell.bg == Color::Reset {
                cell.set_bg(tint);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminals_answer_reads_as_its_background_in_any_digit_count() {
        assert_eq!(parse("\x1b]11;rgb:1e1e/1e1e/2e2e\x07\x1b[?62;4c"), Some((0x1e, 0x1e, 0x2e)));
        assert_eq!(parse("\x1b]11;rgb:ff/fa/f0\x1b\\"), Some((0xff, 0xfa, 0xf0)));
        assert_eq!(parse("\x1b]11;rgb:f/0/8\x07"), Some((255, 0, 136)));
        assert_eq!((parse("\x1b[?62c"), parse("\x1b]11;rgb:zz/00/00\x07")), (None, None), "no answer, or one kebbie cannot read, is no background");
        assert!(answered(b"\x1b]11;rgb:0/0/0\x07\x1b[?1;2c") && !answered(b"\x1b]11;rgb:0/0/0\x07"), "the wait ends on DA1's answer");
    }

    #[test]
    fn the_focus_tint_is_a_step_lighter_on_a_dark_background_and_darker_on_a_light_one() {
        assert_eq!(focus_tint(Some((0, 0, 0))), Color::Rgb(6, 6, 6));
        assert_eq!(focus_tint(Some((255, 255, 255))), Color::Rgb(249, 249, 249));
        assert_eq!(focus_tint(Some((0x1e, 0x1e, 0x2e))), Color::Rgb(36, 36, 52), "the hue stays: every channel moves alike");
        assert_eq!(focus_tint(None), Color::Rgb(36, 36, 52), "with no answer, a step lighter than Mocha's base");
    }
}
