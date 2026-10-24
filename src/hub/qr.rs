use qrcodegen::{QrCode, QrCodeEcc};

const BORDER: i32 = 2;

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

// Black on white in a terminal, whatever its own colours, so a phone's camera reads it.
pub fn terminal(text: &str) -> Option<String> {
    let lines = lines(text)?;
    Some(lines.iter().map(|line| format!("\x1b[30;107m{line}\x1b[0m")).collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_draws_as_a_square_of_half_blocks_two_modules_a_row() {
        let link = "https://kelpie.example.com/connect?code=K7QF-M2XD#k=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&m=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
        let drawn = lines(link).unwrap();
        let size = QrCode::encode_text(link, QrCodeEcc::Low).unwrap().size() as usize;
        assert!(drawn.iter().all(|line| line.chars().count() == size + 4), "every row is the code and its border wide");
        assert_eq!(drawn.len(), (size + 5) / 2, "two modules a row");
        assert!(drawn[0].chars().all(|cell| cell == ' '), "the border is light");
        assert!(drawn[1].contains('█') && drawn.iter().all(|line| line.chars().all(|cell| " ▀▄█".contains(cell))));
        assert!(terminal(link).unwrap().lines().all(|line| line.starts_with("\x1b[30;107m") && line.ends_with("\x1b[0m")));
    }
}
