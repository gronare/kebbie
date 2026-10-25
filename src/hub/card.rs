use std::time::Duration;

use ratatui::style::Color;

use super::qr;
use crate::board::widgets::{GREEN, MAUVE, MUTED, PEACH, RED, YELLOW, text_width};
use crate::config::DEFAULT_HUB_URL;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone {
    Text,
    Title,
    Code,
    Muted,
    Wait,
    Good,
    Bad,
    Key,
    Dots,
}

pub type Piece = (String, Tone);
pub type Row = Vec<Piece>;

pub fn piece(text: impl Into<String>, tone: Tone) -> Piece {
    (text.into(), tone)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Copied {
    Confirmed,
    ByTerminal,
}

pub fn hub_label(url: &str) -> &'static str {
    if super::dev().is_some() {
        "dev hub"
    } else if url == DEFAULT_HUB_URL {
        "kebbie hub"
    } else {
        "your own hub"
    }
}

pub fn hub_shown(url: &str) -> String {
    if super::dev().is_some() { super::dev_label(url) } else { super::host(url).to_string() }
}

// The fragment carries the secret, so the shown link stops at it and c copies the whole one.
pub fn short_link(link: &str) -> String {
    let bare = super::host(link);
    match bare.split_once('#') {
        Some((before, _)) => format!("{before}#…"),
        None => bare.to_string(),
    }
}

pub const NO_ROOM: &str = "A larger terminal shows a QR code of the link here.";

pub fn time_left(left: Duration) -> String {
    let seconds = left.as_secs();
    if seconds >= 60 { format!("{} min left", seconds.div_ceil(60)) } else { format!("{seconds} s left") }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Browser {
    Offered,
    Opened,
    Absent,
}

pub struct Waiting<'a> {
    pub name: &'a str,
    pub url: &'a str,
    pub code: &'a str,
    pub browser: Browser,
    pub left: Duration,
}

pub fn waiting_text(waiting: &Waiting) -> Vec<Row> {
    let mut rows = vec![
        vec![piece(format!("Connect {} to {}", waiting.name, hub_label(waiting.url)), Tone::Title)],
        vec![piece(hub_shown(waiting.url), Tone::Text)],
        Vec::new(),
    ];
    match waiting.browser {
        Browser::Offered => rows.push(vec![piece("⏎", Tone::Key), piece(" open in browser · or scan with your phone", Tone::Text)]),
        Browser::Opened => rows.push(vec![piece("✓ ", Tone::Good), piece("Opened in your browser", Tone::Text)]),
        Browser::Absent => {}
    }
    rows.push(vec![piece("Check the page shows  ", Tone::Text), piece(waiting.code, Tone::Code)]);
    rows.push(Vec::new());
    rows.push(vec![piece("⧗ ", Tone::Wait), piece(format!("Waiting for you · {}", time_left(waiting.left)), Tone::Muted)]);
    rows
}

fn row_width(row: &Row) -> usize {
    row.iter().map(|(text, _)| text_width(text)).sum()
}

pub fn beside(code: &[String], tone: Tone, text: Vec<Row>) -> Vec<Row> {
    if code.is_empty() {
        return text;
    }
    let mut text = text.into_iter();
    let mut rows: Vec<Row> = code.iter().map(|line| {
        let mut row = vec![piece(line.clone(), tone), piece("   ", Tone::Text)];
        row.extend(text.next().unwrap_or_default());
        row
    }).collect();
    rows.extend(text);
    rows
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Foot {
    Key,
    Copied(Copied),
    Whole,
    Bare,
}

pub fn footer(link: &str, foot: Foot) -> Vec<Row> {
    let mut link_row = vec![piece(short_link(link), Tone::Text)];
    match foot {
        Foot::Copied(_) => link_row.extend([piece("   ✓ ", Tone::Good), piece("copied", Tone::Muted)]),
        Foot::Key => link_row.extend([piece("   c", Tone::Key), piece(" copies it", Tone::Muted)]),
        Foot::Whole => link_row = vec![piece(link, Tone::Text)],
        Foot::Bare => {}
    }
    let mut rows = vec![vec![piece("No browser here? Scan the code, or open", Tone::Text)], link_row];
    if foot == Foot::Copied(Copied::ByTerminal) {
        rows.push(vec![piece("If your terminal did not take it, the whole link:", Tone::Muted)]);
        rows.push(vec![piece(link, Tone::Muted)]);
    }
    rows
}

pub fn connected_text(name: &str, url: &str, whose: &str, readable: bool) -> Vec<Row> {
    let mut rows = vec![vec![piece("✓  ", Tone::Good), piece(format!("{name} is connected to {}", hub_shown(url)), Tone::Text)], Vec::new()];
    if readable {
        rows.push(vec![piece("   Sign in with your passkey on any phone or browser", Tone::Text)]);
        rows.push(vec![piece(format!("   to see {whose} board:  "), Tone::Text), piece(super::host(url), Tone::Title)]);
    } else {
        rows.push(vec![piece(format!("   No browser can read {whose} board; connect it from the link:"), Tone::Text)]);
        rows.push(vec![piece(format!("   {}", super::RECONNECT), Tone::Title)]);
    }
    rows
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Colour {
    None,
    Basic,
    True,
}

pub fn colour_for(tty: bool) -> Colour {
    match (tty, std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()), qr::truecolor()) {
        (false, _, _) | (true, true, _) => Colour::None,
        (true, false, true) => Colour::True,
        (true, false, false) => Colour::Basic,
    }
}

fn sgr(color: Color, ground: u8) -> String {
    match color {
        Color::Rgb(red, green, blue) => format!("{ground}8;2;{red};{green};{blue}"),
        Color::Indexed(index) => format!("{ground}8;5;{index}"),
        _ => String::new(),
    }
}

fn codes(tone: Tone, colour: Colour) -> Option<String> {
    let (true_colour, basic, bold) = match tone {
        Tone::Text => return None,
        Tone::Title => (MAUVE, "35", true),
        Tone::Code => (PEACH, "33", true),
        Tone::Muted => (MUTED, "90", false),
        Tone::Wait => (YELLOW, "33", false),
        Tone::Good => (GREEN, "32", false),
        Tone::Bad => (RED, "31", false),
        Tone::Key => (MAUVE, "35", false),
        Tone::Dots => return None,
    };
    let colour = match colour {
        Colour::None => return None,
        Colour::Basic => basic.to_string(),
        Colour::True => sgr(true_colour, 3),
    };
    Some(if bold { format!("1;{colour}") } else { colour })
}

pub fn paint(row: &Row, colour: Colour) -> String {
    row.iter().map(|(text, tone)| match codes(*tone, colour) {
        Some(codes) => format!("\x1b[{codes}m{text}\x1b[0m"),
        None => text.clone(),
    }).collect()
}

pub fn boxed(rows: &[Row], colour: Colour) -> Vec<String> {
    let inner = rows.iter().map(row_width).max().unwrap_or(0) + 4;
    let edge = |row: Row| paint(&row, colour);
    let blank = format!("│{}│", " ".repeat(inner));
    let mut lines = vec![format!("╭{}╮", "─".repeat(inner)), blank.clone()];
    lines.extend(rows.iter().map(|row| format!("│  {}{}│", edge(row.clone()), " ".repeat(inner - 2 - row_width(row)))));
    lines.push(blank);
    lines.push(format!("╰{}╯", "─".repeat(inner)));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://hub.kebbie.dev/connect?code=Z78K-XF53#k=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&m=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

    fn plain(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|row| paint(row, Colour::None)).collect()
    }

    #[test]
    fn the_link_shown_stops_at_the_secret_and_the_time_left_counts_down_in_minutes_then_seconds() {
        assert_eq!(short_link(LINK), "hub.kebbie.dev/connect?code=Z78K-XF53#…");
        assert_eq!(time_left(Duration::from_secs(600)), "10 min left");
        assert_eq!(time_left(Duration::from_secs(530)), "9 min left");
        assert_eq!(time_left(Duration::from_secs(42)), "42 s left");
    }

    #[test]
    fn the_waiting_card_names_the_machine_the_hub_the_code_offers_enter_to_open_and_says_opened_only_once_it_was() {
        let waiting = Waiting { name: "mac", url: DEFAULT_HUB_URL, code: "Z78K-XF53", browser: Browser::Offered, left: Duration::from_secs(530) };
        assert_eq!(plain(&waiting_text(&waiting)), ["Connect mac to kebbie hub", "hub.kebbie.dev", "", "⏎ open in browser · or scan with your phone", "Check the page shows  Z78K-XF53", "", "⧗ Waiting for you · 9 min left"]);
        assert_eq!(plain(&waiting_text(&Waiting { browser: Browser::Opened, ..waiting }))[3], "✓ Opened in your browser");
        let own = Waiting { url: "https://hub.example.com", browser: Browser::Absent, ..waiting };
        let shown = plain(&waiting_text(&own));
        assert!(shown.contains(&"Connect mac to your own hub".to_string()) && !shown.iter().any(|line| line.contains("Opened") || line.contains('⏎')), "{shown:?}");
    }

    #[test]
    fn the_card_paints_the_code_peach_and_bold_the_title_mauve_and_the_wait_muted_with_a_yellow_hourglass() {
        let rows = waiting_text(&Waiting { name: "mac", url: DEFAULT_HUB_URL, code: "Z78K-XF53", browser: Browser::Absent, left: Duration::from_secs(600) });
        let painted = rows.iter().map(|row| paint(row, Colour::True)).collect::<Vec<_>>().join("\n");
        assert!(painted.contains("\x1b[1;38;2;250;179;135mZ78K-XF53\x1b[0m"), "{painted:?}");
        assert!(painted.contains("\x1b[1;38;2;203;166;247mConnect mac to kebbie hub\x1b[0m"));
        assert!(painted.contains("\x1b[38;2;249;226;175m⧗ \x1b[0m\x1b[38;2;108;112;134mWaiting for you · 10 min left\x1b[0m"));
        let basic = rows.iter().map(|row| paint(row, Colour::Basic)).collect::<Vec<_>>().join("\n");
        assert!(basic.contains("\x1b[1;33mZ78K-XF53") && !basic.contains("38;2"), "{basic:?}");
        assert!(!rows.iter().map(|row| paint(row, Colour::None)).any(|line| line.contains('\x1b')), "plain without colour");
    }

    #[test]
    fn the_box_frames_every_row_at_one_width() {
        let code = qr::lines(LINK, qr::Glyphs::Octant).unwrap();
        let rows = beside(&code, Tone::Dots, waiting_text(&Waiting { name: "mac", url: DEFAULT_HUB_URL, code: "Z78K-XF53", browser: Browser::Opened, left: Duration::from_secs(530) }));
        let lines = boxed(&rows, Colour::None);
        let width = text_width(&lines[0]);
        assert!(lines.iter().all(|line| text_width(line) == width), "{}", lines.join("\n"));
        assert!(lines[0].starts_with('╭') && lines.last().unwrap().starts_with('╰'));
        let row = lines.iter().find(|line| line.contains("Check the page shows")).unwrap();
        assert!(row.starts_with(&format!("│  {}   ", code[row_index(&lines, row) - 2])), "the code, then the text to its right: {row}");
        assert_eq!(lines.len(), code.len() + 4, "the text fits beside the code");
        assert!(code[0].chars().count().abs_diff(code.len() * 2) <= 1, "square modules: {} × {}", code[0].chars().count(), code.len());
        assert!(lines[2].starts_with(&format!("│  {}   Connect mac to kebbie hub", code[0])), "the title on the code's first row: {}", lines[2]);
    }

    #[test]
    fn the_code_is_painted_in_the_terminals_own_colours_without_a_paper() {
        for glyphs in [qr::Glyphs::Octant, qr::Glyphs::Braille] {
            let code = qr::lines(LINK, glyphs).unwrap();
            let row = vec![piece(code[1].clone(), Tone::Dots)];
            for colour in [Colour::True, Colour::Basic, Colour::None] {
                assert_eq!(paint(&row, colour), code[1], "{glyphs:?} {colour:?}");
            }
        }
    }

    fn row_index(lines: &[String], row: &str) -> usize {
        lines.iter().position(|line| line == row).unwrap()
    }

    #[test]
    fn the_footer_offers_c_and_after_a_copy_the_terminal_could_not_confirm_shows_the_whole_link() {
        assert_eq!(plain(&footer(LINK, Foot::Key)), ["No browser here? Scan the code, or open", "hub.kebbie.dev/connect?code=Z78K-XF53#…   c copies it"]);
        assert_eq!(plain(&footer(LINK, Foot::Copied(Copied::Confirmed))), ["No browser here? Scan the code, or open", "hub.kebbie.dev/connect?code=Z78K-XF53#…   ✓ copied"]);
        let asked = plain(&footer(LINK, Foot::Copied(Copied::ByTerminal)));
        assert_eq!(asked.last().unwrap(), LINK);
        assert_eq!(plain(&footer(LINK, Foot::Bare))[1], "hub.kebbie.dev/connect?code=Z78K-XF53#…");
        assert_eq!(plain(&footer(LINK, Foot::Whole))[1], LINK, "without a terminal to copy from, the whole link");
    }

    #[test]
    fn the_connected_card_names_the_machine_and_the_hub_and_no_account() {
        assert_eq!(plain(&connected_text("mac", DEFAULT_HUB_URL, "its", true)), ["✓  mac is connected to hub.kebbie.dev", "", "   Sign in with your passkey on any phone or browser", "   to see its board:  hub.kebbie.dev"]);
        assert_eq!(plain(&connected_text("mac", DEFAULT_HUB_URL, "its", false)), ["✓  mac is connected to hub.kebbie.dev", "", "   No browser can read its board; connect it from the link:", "   kebbie hub leave, then kebbie hub connect"], "a machine with no account key says so instead of sending you to sign in");
    }
}
