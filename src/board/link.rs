use std::cell::RefCell;
use std::path::Path;

use ratatui::buffer::Buffer;
use ratatui::text::Line;

pub struct Link {
    pub(super) shown: String,
    pub(super) url: String,
}

// Spans drop control characters, so the OSC 8 escape goes into the drawn cells instead, each forced to one column; a URL with anything but printable ASCII stays plain text.
pub(super) fn link_cells(buffer: &mut Buffer, link: &Link) {
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
                mark(buffer, start + x..start + x + shown.len(), &link.url);
                x += shown.len();
            } else {
                x += 1;
            }
        }
    }
}

fn mark(buffer: &mut Buffer, cells: impl IntoIterator<Item = usize>, url: &str) {
    let id = link_id(url);
    for index in cells {
        let cell = &mut buffer.content[index];
        let symbol = format!("\x1b]8;id={id};{url}\x1b\\{}\x1b]8;;\x1b\\", cell.symbol());
        cell.set_symbol(&symbol).set_diff_option(ratatui::buffer::CellDiffOption::ForcedWidth(std::num::NonZeroU16::MIN));
    }
}

// The id makes a terminal treat every cell of one URL as one link, also across the rows it wraps over.
fn link_id(url: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    format!("k{:012x}", hasher.finish() & 0xffff_ffff_ffff)
}

thread_local! {
    static NOTED: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

// Outside a board frame nothing is collecting, so text wrapped elsewhere leaves nothing behind.
pub(super) fn start_noting() {
    NOTED.with(|noted| *noted.borrow_mut() = Some(Vec::new()));
}

pub(super) fn take_noted() -> Vec<String> {
    NOTED.with(|noted| noted.borrow_mut().take()).unwrap_or_default()
}

pub fn note_urls(text: &str) {
    if !text.contains("://") {
        return;
    }
    NOTED.with(|noted| {
        if let Some(noted) = noted.borrow_mut().as_mut() {
            for url in urls_in(text) {
                if !noted.contains(&url) {
                    noted.push(url);
                }
            }
        }
    });
}

pub fn note_lines(lines: &[Line]) {
    for line in lines {
        note_urls(&line.spans.iter().map(|span| span.content.as_ref()).collect::<String>());
    }
}

// A URL ends at whitespace or an ellipsis; one that runs into any other character beyond printable ASCII is not a URL kelpie links.
pub(super) fn urls_in(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = text;
    while let Some(at) = ["https://", "http://"].iter().filter_map(|scheme| rest.find(scheme)).min() {
        let candidate = &rest[at..];
        let end = candidate.find(|character: char| !character.is_ascii_graphic() || matches!(character, '<' | '>' | '"' | '`')).unwrap_or(candidate.len());
        let stop = candidate[end..].chars().next();
        let mut url = &candidate[..end];
        loop {
            let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '*', '\'']);
            let trimmed = match trimmed.chars().last() {
                Some(')') if trimmed.matches('(').count() < trimmed.matches(')').count() => &trimmed[..trimmed.len() - 1],
                Some(']') if trimmed.matches('[').count() < trimmed.matches(']').count() => &trimmed[..trimmed.len() - 1],
                _ => trimmed,
            };
            if trimmed == url {
                break;
            }
            url = trimmed;
        }
        let ends_cleanly = stop.is_none_or(|character| character.is_whitespace() || character == '…' || character.is_ascii_graphic());
        if ends_cleanly && url.len() > url.find("://").unwrap_or(0) + 3 {
            urls.push(url.to_string());
        }
        rest = &candidate[end.max(1)..];
    }
    urls
}

pub(super) fn link_urls(buffer: &mut Buffer, urls: &[String]) {
    let mut urls: Vec<&String> = urls.iter().collect();
    urls.sort_by_key(|url| std::cmp::Reverse(url.len()));
    for url in urls {
        let chars: Vec<String> = url.chars().map(String::from).collect();
        let (width, height) = (buffer.area.width as usize, buffer.area.height as usize);
        for y in 0..height {
            let mut x = 0;
            while x < width {
                match follow(buffer, &chars, x, y, 0) {
                    Some(cells) => {
                        x += cells.iter().take_while(|&&index| index / width == y).count();
                        mark(buffer, cells, url);
                    }
                    None => x += 1,
                }
            }
        }
    }
}

const LEAST_BEFORE_ELLIPSIS: usize = 10;

// Cells of a URL from its char at index `at` onwards, starting at (x, y): a row that ends the text before the URL is done continues on the next row, after its indent and frame.
fn follow(buffer: &Buffer, url: &[String], x: usize, y: usize, at: usize) -> Option<Vec<usize>> {
    let width = buffer.area.width as usize;
    let symbol = |x: usize| buffer.content[y * width + x].symbol();
    let mut cells = Vec::new();
    let (mut x, mut at) = (x, at);
    while x < width && at < url.len() && symbol(x) == url[at] {
        cells.push(y * width + x);
        (x, at) = (x + 1, at + 1);
    }
    if cells.is_empty() {
        return None;
    }
    if at == url.len() {
        return url_ends(buffer, x, y).then_some(cells);
    }
    if x < width && symbol(x) == "…" {
        return (at >= LEAST_BEFORE_ELLIPSIS).then(|| { cells.push(y * width + x); cells });
    }
    if !text_ends(buffer, x, y) || y + 1 >= buffer.area.height as usize {
        return None;
    }
    let next = y + 1;
    let gap = |x: usize| x == 0 || blank(buffer.content[next * width + x - 1].symbol());
    (0..width).filter(|&x| gap(x)).find_map(|x| follow(buffer, url, x, next, at)).map(|rest| { cells.extend(rest); cells })
}

// The URL stops before x: the row ends, or a blank, an ellipsis or punctuation that urls_in trims followed by a blank does.
fn url_ends(buffer: &Buffer, x: usize, y: usize) -> bool {
    let width = buffer.area.width as usize;
    let trimmed = |symbol: &str| matches!(symbol, "." | "," | ";" | ":" | "!" | "?" | "*" | "'" | ")" | "]" | "<" | ">" | "\"" | "`");
    let after = (x..width).find(|&x| !trimmed(buffer.content[y * width + x].symbol())).unwrap_or(width);
    after == width || blank(buffer.content[y * width + after].symbol())
}

// The row's text stops at x: the row ends, a frame character follows, or a gap wider than one space between words does.
fn text_ends(buffer: &Buffer, x: usize, y: usize) -> bool {
    let width = buffer.area.width as usize;
    let spaces = (x..width).take_while(|&x| matches!(buffer.content[y * width + x].symbol(), " " | "")).count();
    x + spaces == width || spaces >= 2 || !buffer.content[y * width + x + spaces].symbol().is_ascii()
}

fn blank(symbol: &str) -> bool {
    symbol.chars().all(|character| character == ' ' || !character.is_ascii())
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

    use std::path::{Path, PathBuf};

    use ratatui::crossterm::event::KeyCode;

    use super::*;
    use crate::board::detail::server_line;
    use crate::board::message_log::Fold;
    use crate::board::test_support::*;
    use crate::board::Mode;
    use crate::messages;

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
        let id = link_id("http://localhost:3001");
        assert_eq!(buffer[(19, 1)].symbol(), format!("\x1b]8;id={id};http://localhost:3001\x1b\\h\x1b]8;;\x1b\\"));
        assert_eq!(buffer[(39, 1)].symbol(), format!("\x1b]8;id={id};http://localhost:3001\x1b\\1\x1b]8;;\x1b\\"));
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

    const LONG: &str = "https://github.com/gronare/kelpie/actions/runs/18734523412/job/53412345678";

    #[test]
    fn a_url_is_read_out_of_prose_without_its_punctuation_and_only_when_it_is_plain_ascii() {
        assert_eq!(urls_in("CI failed for abc on main: CI · test (https://github.com/a/b/actions/runs/1). The last"), ["https://github.com/a/b/actions/runs/1"]);
        assert_eq!(urls_in("see **https://x.example/a_(b)**, then http://localhost:3001/!"), ["https://x.example/a_(b)", "http://localhost:3001/"]);
        assert_eq!(urls_in("cut https://x.example/abc… here"), ["https://x.example/abc"]);
        assert!(urls_in("https://bücher.example and https://x.example\x1b]8;;evil and https:// alone").is_empty(), "beyond ASCII, into an escape, or no host");
    }

    #[test]
    fn a_url_wrapped_over_rows_inside_a_frame_is_one_link_and_prose_after_a_url_is_not_joined() {
        let mut buffer = Buffer::with_lines([
            "│ log: https://x.example/one/ │",
            "│ two/three                   │",
            "│ https://x.example/one/ and  │",
            "│ two/three                   │",
        ]);
        link_urls(&mut buffer, &["https://x.example/one/two/three".to_string()]);
        assert!((7..29).chain(33..42).all(|index| link_of(&buffer.content[index]).is_some_and(|(url, _)| url == "https://x.example/one/two/three")), "both rows of the first URL");
        assert_eq!(link_of(&buffer[(29, 0)]), None, "the space after it");
        assert!((2..4).all(|y| (0..31).all(|x| link_of(&buffer[(x, y)]).is_none())), "a URL followed by a word on its row does not continue below");
    }

    #[test]
    fn a_url_links_only_where_it_ends_so_a_longer_url_it_starts_is_never_cut_short() {
        let short = "https://x.example/pr/12".to_string();
        let long = "https://x.example/pr/12/files".to_string();
        let mut buffer = Buffer::with_lines(["https://x.example/pr/12/files and https://x.example/pr/12. done"]);
        link_urls(&mut buffer, std::slice::from_ref(&short));
        assert!((0..29).all(|x| link_of(&buffer[(x, 0)]).is_none()), "the start of a longer URL is not the shorter one");
        assert!((34..57).all(|x| link_of(&buffer[(x, 0)]).is_some_and(|(url, _)| url == short)), "the short one before its full stop");
        let mut buffer = Buffer::with_lines(["https://x.example/pr/12/files and https://x.example/pr/12 done"]);
        link_urls(&mut buffer, &[short.clone(), long.clone()]);
        assert!((0..29).all(|x| link_of(&buffer[(x, 0)]).is_some_and(|(url, _)| url == long)), "the longer URL links whole, whichever was noted first");
        assert!((34..57).all(|x| link_of(&buffer[(x, 0)]).is_some_and(|(url, _)| url == short)));
    }

    #[test]
    fn a_wrapped_report_url_is_one_link_in_the_detail_panel_and_in_the_unfolded_row() {
        let mut item = served("search-filters");
        item.report.as_mut().unwrap().text = format!("pushed; CI is running at {LONG} now");
        let mut app = test_app(vec![item]);
        with_detail(&mut app);
        for (width, rows) in [(170, 2), (84, 1), (60, 1)] {
            let (_, buffer) = render(&mut app, width, 50);
            assert_whole_link(&buffer, LONG, rows, &format!("the report at {width} columns"));
        }
    }

    #[test]
    fn a_cut_ci_run_url_and_a_cut_next_step_link_the_whole_url() {
        let mut item = served("search-filters");
        let run = crate::ci::Run { id: 42, name: "CI".to_string(), workflow: "CI".to_string(), status: "in_progress".to_string(), conclusion: String::new(), url: LONG.to_string() };
        item.ci = Some(crate::ci::Watch {
            address: "billing@web".to_string(), task: "billing".to_string(), repo: PathBuf::new(), slug: "acme/web".to_string(), sha: "abcdef0".to_string(),
            started_at: 0, checked_at: None, close: true, runs: vec![run], state: crate::ci::State::Running, error: None, erring_since: None, pushed: crate::ci::Pushed::default(),
        });
        crate::status::tests::rederive(&mut item);
        let mut app = test_app(vec![item]);
        with_detail(&mut app);
        for width in [170, 84, 60] {
            let (_, buffer) = render(&mut app, width, 50);
            assert_whole_link(&buffer, LONG, 1, &format!("the ci line at {width} columns"));
        }
        let mut item = served("search-filters");
        let pull = "https://github.com/gronare/kelpie/pull/123/files?diff=split&w=1";
        item.report.as_mut().unwrap().report.as_mut().unwrap().state = "needs-you".to_string();
        item.report.as_mut().unwrap().report.as_mut().unwrap().next = format!("{pull} then push and close");
        crate::status::tests::rederive(&mut item);
        let mut app = test_app(vec![item]);
        with_detail(&mut app);
        for (width, rows) in [(170, 1), (60, 2)] {
            let (_, buffer) = render(&mut app, width, 50);
            assert_whole_link(&buffer, pull, rows, &format!("the you line at {width} columns"));
        }
    }

    #[test]
    fn a_url_in_the_messages_panel_and_in_the_opened_message_is_one_link() {
        let mut app = test_app(vec![served("search-filters")]);
        let mut messages = some_messages(1);
        messages[0].text = format!("CI failed for abcdef0 on main: CI · test ({LONG}). The last lines of its log:");
        app.messages = messages;
        app.fold = Fold::Open;
        let (_, buffer) = render(&mut app, 100, 50);
        assert_whole_link(&buffer, LONG, 1, "the messages panel row");
        app.mode = Mode::Message(format!("10:00 kelpie → billing@web\n\n{}", app.messages[0].text));
        let (_, buffer) = render(&mut app, 50, 30);
        assert_whole_link(&buffer, LONG, 2, "the opened message");
    }

    #[test]
    fn a_url_in_a_question_and_on_a_sign_off_card_is_one_link_where_it_wraps() {
        let mut question = messages::fold(QUESTION).remove(0);
        question.text = format!("Which run is the flaky one, {LONG} or the one before?");
        let mut app = test_app(vec![served("search-filters")]);
        app.questions = vec![question];
        app.open_questions(Some(0));
        let (_, buffer) = render(&mut app, 60, 30);
        assert_whole_link(&buffer, LONG, 2, "the question window");
        let mut app = carded(true);
        let card = &mut std::rc::Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card.as_mut().unwrap().0;
        card.text = format!("The run: {LONG}\nA second line.");
        app.handle_normal(key(KeyCode::Char('s')));
        assert!(matches!(app.mode, Mode::Card { .. }));
        let (_, buffer) = render(&mut app, 60, 40);
        assert_whole_link(&buffer, LONG, 2, "the sign-off card");
    }
}
