use std::path::Path;

pub struct Link {
    pub(super) shown: String,
    pub(super) url: String,
}

// Spans drop control characters, so the OSC 8 escape goes into the drawn cells instead, each forced to one column; a URL with anything but printable ASCII stays plain text.
pub(super) fn link_cells(buffer: &mut ratatui::buffer::Buffer, link: &Link) {
    let printable = |text: &str| text.chars().all(|character| character.is_ascii_graphic());
    if !printable(&link.url) || !printable(link.shown.trim_end_matches('…')) {
        return;
    }
    let shown: Vec<String> = link.shown.chars().map(String::from).collect();
    let area = buffer.area;
    let width = area.width as usize;
    if shown.is_empty() || shown.len() > width {
        return;
    }
    for row in 0..area.height as usize {
        let start = row * width;
        let mut x = 0;
        while x + shown.len() <= width {
            let cells = &buffer.content[start + x..start + x + shown.len()];
            if cells.iter().zip(&shown).all(|(cell, symbol)| cell.symbol() == symbol) {
                for cell in &mut buffer.content[start + x..start + x + shown.len()] {
                    let symbol = format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", link.url, cell.symbol());
                    cell.set_symbol(&symbol).set_diff_option(ratatui::buffer::CellDiffOption::ForcedWidth(std::num::NonZeroU16::MIN));
                }
                x += shown.len();
            } else {
                x += 1;
            }
        }
    }
}

pub(super) fn file_url(file: &Path) -> String {
    let mut url = String::from("file://");
    for byte in file.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'.' | b'_' | b'~' => url.push(byte as char),
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

pub(super) fn file_link(shown: &str, file: &Path) -> Option<Link> {
    file.is_absolute().then(|| Link { shown: shown.to_string(), url: file_url(file) })
}

#[cfg(test)]
mod tests {
    use ratatui::text::Line;
    use ratatui::widgets::Paragraph;

    use super::*;
    use crate::board::detail::server_line;

    #[test]
    fn the_server_address_is_a_terminal_hyperlink_one_column_per_character() {
        let mut item = crate::status::tests::task(Some("idle"), None, 0);
        item.server = true;
        item.url = Some("http://localhost:3001".to_string());
        let (line, link) = server_line(&item, true, "server", 60).unwrap();
        let link = link.unwrap();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 3)).unwrap();
        terminal.draw(|frame| {
            frame.render_widget(Paragraph::new(vec![Line::from("http://localhost:3001 is elsewhere too"), line]), frame.area());
            link_cells(frame.buffer_mut(), &link);
        }).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(19, 1)].symbol(), "\x1b]8;;http://localhost:3001\x1b\\h\x1b]8;;\x1b\\");
        assert_eq!(buffer[(39, 1)].symbol(), "\x1b]8;;http://localhost:3001\x1b\\1\x1b]8;;\x1b\\");
        assert_eq!((buffer[(18, 1)].symbol(), buffer[(40, 1)].symbol()), (" ", " "), "the link covers the address and nothing else");
        assert!((19..40).all(|x| buffer[(x, 1)].diff_option == ratatui::buffer::CellDiffOption::ForcedWidth(std::num::NonZeroU16::MIN)));
        let mut plain = Link { shown: "https://bücher.example".to_string(), url: "https://bücher.example".to_string() };
        let mut buffer = ratatui::buffer::Buffer::with_lines(["https://bücher.example"]);
        link_cells(&mut buffer, &plain);
        assert_eq!(buffer[(0, 0)].symbol(), "h", "a URL beyond printable ASCII stays plain text");
        plain.url = "https://x.example\x1b]8;;evil".to_string();
        plain.shown = "https://x.example".to_string();
        link_cells(&mut buffer, &plain);
        assert!(!buffer.content.iter().any(|cell| cell.symbol().contains('\x1b')));
    }
}
