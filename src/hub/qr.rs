use qrcodegen::{QrCode, QrCodeEcc};

const BORDER: i32 = 2;
const PAPER: &str = "\x1b[38;2;0;0;0;48;2;255;255;255m";
const RESET: &str = "\x1b[0m";

// Each character is two modules stacked, the top as ▀ and the bottom as ▄; a dark module is drawn in the foreground, so the code needs dark on light around it.
pub fn lines(text: &str) -> Option<Vec<String>> {
    let code = QrCode::encode_text(text, QrCodeEcc::Low).ok()?;
    let span = -BORDER..code.size() + BORDER;
    Some(span.clone().step_by(2).map(|y| span.clone().map(|x| match (code.get_module(x, y), code.get_module(x, y + 1)) {
        (true, true) => '█',
        (true, false) => '▀',
        (false, true) => '▄',
        (false, false) => ' ',
    }).collect()).collect())
}

// A theme remaps the named black and white, so only 24-bit colour is sure to be black on white.
pub fn truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "truecolor" | "24bit"))
}

// Without 24-bit colour the glyphs go in the terminal's own colours, which a dark theme shows inverted; phone cameras read that too.
pub fn terminal(text: &str, truecolor: bool) -> Option<String> {
    let lines = lines(text)?;
    Some(lines.iter().map(|line| if truecolor { format!("{PAPER}{line}{RESET}") } else { line.clone() }).collect::<Vec<_>>().join("\n"))
}

pub fn for_stdout(text: &str) -> Option<String> {
    // SAFETY: isatty only reads the descriptor number.
    let tty = unsafe { libc::isatty(1) == 1 };
    terminal(text, tty && truecolor())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://kebbie.example.com/connect?code=K7QF-M2XD#k=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&m=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

    fn modules(rows: &[String]) -> Vec<Vec<bool>> {
        rows.iter().flat_map(|row| {
            let top = row.chars().map(|cell| matches!(cell, '█' | '▀')).collect::<Vec<_>>();
            let bottom = row.chars().map(|cell| matches!(cell, '█' | '▄')).collect::<Vec<_>>();
            [top, bottom]
        }).collect()
    }

    #[test]
    fn a_link_draws_as_a_square_of_half_blocks_two_modules_a_row_with_every_module_in_place() {
        let drawn = lines(LINK).unwrap();
        let code = QrCode::encode_text(LINK, QrCodeEcc::Low).unwrap();
        let size = code.size() as usize;
        assert!(drawn.iter().all(|line| line.chars().count() == size + 4), "every row is the code and its border wide");
        assert_eq!(drawn.len(), (size + 5) / 2, "two modules a row, half as tall as wide");
        assert!(drawn[0].chars().all(|cell| cell == ' '), "the quiet zone is light");
        assert!(drawn.iter().all(|line| line.chars().all(|cell| " ▀▄█".contains(cell))));
        let grid = modules(&drawn);
        for y in 0..size {
            for x in 0..size {
                assert_eq!(grid[y + 2][x + 2], code.get_module(x as i32, y as i32), "module {x},{y}");
            }
        }
    }

    #[test]
    fn truecolor_draws_black_on_white_by_rgb_and_the_fallback_has_no_colour_codes() {
        let coloured = terminal(LINK, true).unwrap();
        assert!(coloured.lines().all(|line| line.starts_with("\x1b[38;2;0;0;0;48;2;255;255;255m") && line.ends_with("\x1b[0m")), "{coloured}");
        assert!(!coloured.contains("\x1b[30") && !coloured.contains("107m"), "no named colour a theme could remap");
        let plain = terminal(LINK, false).unwrap();
        assert!(!plain.contains('\x1b'));
        assert_eq!(plain.lines().map(str::to_string).collect::<Vec<_>>(), lines(LINK).unwrap());
    }
}
