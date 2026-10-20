use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use ratatui::Frame;
use ratatui::layout::Size;

use crate::keys::{self, Act, Ctx};
use crate::report;
use crate::status::Item;
use super::detail::Detail;
use super::images::{ImageAt, Shown, image_shown, preview_note, render_image};
use super::link::file_link;
use super::{App, Column, Mode};
use super::widgets::{GREEN, MANTLE, MAUVE, MUTED, SURFACE, TEAL, TEXT, YELLOW, bold, centered, fit, style, wrap};

impl App {
    pub(super) fn card_of(&self, item: &Item) -> Option<std::rc::Rc<Detail>> {
        self.details.get(item.path.as_ref()?).filter(|detail| detail.card.is_some()).cloned()
    }

    pub(super) fn selected_has_card(&self) -> bool {
        self.column == Column::Work && self.selected_task().is_some_and(|item| self.card_of(&item).is_some())
    }

    fn card_images(&self, path: &Path) -> Vec<PathBuf> {
        let Some((card, _)) = self.details.get(path).and_then(|detail| detail.card.as_ref()) else { return Vec::new() };
        card.text.lines().filter_map(card_image_in).map(|(_, image)| path.join(image)).collect()
    }
}

pub(super) fn card_image_in(line: &str) -> Option<(&str, &str)> {
    line.trim().strip_prefix("![").and_then(|rest| rest.split_once("](")).map(|(alt, rest)| (alt, rest.trim_end_matches(')')))
}

pub(super) fn card_hint() -> Line<'static> {
    Line::from(vec![Span::raw("   "), Span::styled("⏎", style(MAUVE)), Span::styled(" options · ", style(MUTED)), Span::styled(keys::key(Act::Card).to_string(), style(MAUVE)), Span::styled(" card", style(MUTED))]).style(Style::new().bg(SURFACE))
}

pub(super) fn draw_card(frame: &mut Frame, app: &mut App) {
    let Mode::Card { path, name, scroll } = &app.mode else { return };
    let (path, name, scroll) = (path.clone(), name.clone(), *scroll);
    let Some(detail) = app.details.get(&path).cloned() else { return };
    let Some((card, current)) = &detail.card else { return };
    let full = frame.area();
    let area = centered(full, (full.width * 92 / 100).max(40), (full.height * 84 / 100).max(12));
    frame.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {name} · card "), bold(MAUVE))).style(Style::new().bg(MANTLE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [body, foot] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let sha: String = card.sha.chars().take(7).collect();
    let head = if *current { Span::styled(format!("✓ current at {sha}"), style(GREEN)) } else { Span::styled(format!("○ written at {sha}, code changed since"), style(YELLOW)) };
    let mut lines = vec![Line::from(head), Line::default()];
    let mut images = Vec::new();
    let mut links = Vec::new();
    let mut image_lines = Vec::new();
    let viewer = app.viewer.is_some();
    let area = Size::new(body.width, body.height.saturating_sub(2).max(1));
    for text in card.text.lines() {
        match card_image_in(text) {
            Some((alt, image)) => {
                image_lines.push(lines.len());
                lines.push(Line::from(vec![Span::styled("▣ ", style(TEAL)), Span::styled(image.to_string(), style(TEXT)), Span::styled(format!("  {alt}"), style(MUTED))]));
                let file = path.join(image);
                links.extend(file_link(image, &file));
                // An image taller than the rows left under the text is never drawn, so it is sized to fit below what precedes it.
                let left = body.height.saturating_sub(lines.len() as u16 + 1).max(6);
                let area = Size::new(area.width, area.height.min(left));
                let rows = app.image_size(&file, area).map(|size| size.height);
                if let Some(rows) = rows {
                    images.push(ImageAt { line: lines.len(), path: file, rows, area });
                    lines.extend((0..rows).map(|_| Line::default()));
                }
                if image_shown(app.protocol(), rows.is_some()) == Shown::Preview {
                    lines.push(Line::from(Span::styled(preview_note(viewer), style(MUTED))));
                }
            }
            None if text.trim().is_empty() => lines.push(Line::default()),
            None => lines.extend(wrap(text, body.width as usize, usize::MAX).into_iter().map(|part| Line::from(Span::styled(part, style(TEXT))))),
        }
    }
    let scroll = scroll.min(lines.len().saturating_sub(body.height as usize) as u16);
    if let Mode::Card { scroll: kept, .. } = &mut app.mode {
        *kept = scroll;
    }
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
    for image in images {
        let Some(top) = (image.line as u16).checked_sub(scroll) else { continue };
        if top + image.rows > body.height {
            continue;
        }
        if let Some(Some(protocol)) = app.images.get(&(image.path, image.area.width, image.area.height)) {
            let size = protocol.size();
            render_image(frame, protocol, Rect::new(body.x, body.y + top, size.width.min(body.width), size.height));
        }
    }
    let keys = card_keys(!image_lines.is_empty() && app.protocol().is_some(), !image_lines.is_empty() && viewer);
    app.card_image_lines = image_lines;
    app.links.extend(links);
    frame.render_widget(Paragraph::new(Span::styled(keys, style(MUTED))), foot);
}

fn card_keys(fullscreen: bool, open: bool) -> String {
    let map = keys::map();
    let mut keys = vec!["↑ ↓ scroll".to_string()];
    if fullscreen {
        keys.push(format!("{} fullscreen", map.label(Act::Fullscreen)));
    }
    if open {
        keys.push(format!("{} open", map.label(Act::OpenImage)));
    }
    keys.push(format!("{} push", map.label(Act::Push)));
    keys.push(format!("{} close", map.label(Act::Back)));
    keys.join(" · ")
}

pub(super) fn draw_fullscreen(frame: &mut Frame, app: &mut App) {
    let Mode::Fullscreen { path, index, .. } = &app.mode else { return };
    let files = app.card_images(path);
    let index = *index;
    let Some(file) = files.get(index).cloned() else { return };
    let full = frame.area();
    frame.render_widget(Clear, full);
    frame.render_widget(Block::new().style(Style::new().bg(MANTLE)), full);
    let [body, foot] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(full);
    let shown = file.display().to_string();
    match app.image_size(&file, body.as_size()) {
        Some(size) => {
            if let Some(Some(protocol)) = app.images.get(&(file.clone(), body.width, body.height)) {
                let x = body.x + body.width.saturating_sub(size.width) / 2;
                let y = body.y + body.height.saturating_sub(size.height) / 2;
                render_image(frame, protocol, Rect::new(x, y, size.width.min(body.width), size.height.min(body.height)));
            }
        }
        None => frame.render_widget(Paragraph::new(Line::from(vec![Span::styled("▣ ", style(TEAL)), Span::styled(shown.clone(), style(TEXT))])), body),
    }
    let map = keys::map();
    let mut keys = Vec::new();
    if image_shown(app.protocol(), app.images.get(&(file.clone(), body.width, body.height)).is_some_and(Option::is_some)) == Shown::Preview {
        keys.push("preview".to_string());
    }
    keys.push(format!("{}/{} {shown}", index + 1, files.len()));
    if files.len() > 1 {
        keys.push(format!("{} images", map.label(Act::Switch)));
    }
    if app.viewer.is_some() {
        keys.push(format!("{} open", map.label(Act::OpenImage)));
    }
    keys.push(format!("{} back", map.label(Act::Back)));
    frame.render_widget(Paragraph::new(fit(vec![Span::styled(keys.join(" · "), style(MUTED))], foot.width as usize)), foot);
    app.links.extend(file_link(&shown, &file));
}

const CARD_LINES: usize = 16;

// The sign-off card the task wrote when it reported ready, as the user reads it before pushing; an image shows as its path.
pub(super) fn card_lines(card: &report::Card, current: bool, width: usize) -> Vec<Line<'static>> {
    let mut head = vec![Span::styled("card", bold(TEAL)), Span::styled(format!(" · written at {}", card.sha.chars().take(7).collect::<String>()), style(MUTED))];
    if !current {
        head.push(Span::styled(" · the code changed since", style(YELLOW)));
    }
    let mut body = Vec::new();
    for line in card.text.lines() {
        let image = line.trim().strip_prefix("![").and_then(|rest| rest.split_once("](")).map(|(alt, rest)| (alt, rest.trim_end_matches(')')));
        match image {
            Some((alt, path)) => body.push(Line::from(vec![Span::styled("▣ ", style(TEAL)), Span::styled(path.to_string(), style(TEXT)), Span::styled(format!("  {alt}"), style(MUTED))])),
            None if line.trim().is_empty() => body.push(Line::default()),
            None => body.extend(wrap(line, width, 4).into_iter().map(|part| Line::from(Span::styled(part, style(TEXT))))),
        }
    }
    if body.len() > CARD_LINES {
        body.truncate(CARD_LINES);
        body.push(Line::from(Span::styled(format!("… the rest in {}", card.file.display()), style(MUTED))));
    }
    let mut lines = vec![Line::from(head)];
    lines.extend(body);
    lines
}

impl App {
    pub(super) fn handle_card(&mut self, path: PathBuf, name: String, scroll: u16, key: KeyEvent) {
        let map = keys::map();
        match map.act(Ctx::Card, &key) {
            Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => self.mode = Mode::Card { path, name, scroll: scroll.saturating_sub(1) },
            Some(Act::Move) => self.mode = Mode::Card { path, name, scroll: scroll.saturating_add(1) },
            Some(Act::Push) => self.act(Act::Push),
            Some(Act::Back | Act::Close) => {}
            Some(act @ (Act::Fullscreen | Act::OpenImage)) => {
                let files = self.card_images(&path);
                let index = self.card_image_lines.iter().position(|line| *line >= scroll as usize).unwrap_or(0).min(files.len().saturating_sub(1));
                match files.get(index) {
                    Some(_) if act == Act::Fullscreen && self.protocol().is_some() => self.mode = Mode::Fullscreen { path, name, scroll, index },
                    Some(file) if act == Act::OpenImage => {
                        let file = file.clone();
                        self.mode = Mode::Card { path, name, scroll };
                        self.open_image(&file);
                    }
                    _ => self.mode = Mode::Card { path, name, scroll },
                }
            }
            _ => match key.code {
                KeyCode::PageUp => self.mode = Mode::Card { path, name, scroll: scroll.saturating_sub(10) },
                KeyCode::PageDown | KeyCode::Char(' ') => self.mode = Mode::Card { path, name, scroll: scroll.saturating_add(10) },
                _ => self.mode = Mode::Card { path, name, scroll },
            },
        }
    }

    pub(super) fn handle_fullscreen(&mut self, path: PathBuf, name: String, scroll: u16, index: usize, key: KeyEvent) {
        let map = keys::map();
        let count = self.card_images(&path).len().max(1);
        match map.act(Ctx::Card, &key) {
            Some(Act::Back | Act::Close | Act::Fullscreen) => self.mode = Mode::Card { path, name, scroll },
            Some(Act::Switch) if key.code == KeyCode::Left => self.mode = Mode::Fullscreen { path, name, scroll, index: (index + count - 1) % count },
            Some(Act::Switch) => self.mode = Mode::Fullscreen { path, name, scroll, index: (index + 1) % count },
            Some(Act::OpenImage) => {
                let file = self.card_images(&path).get(index).cloned();
                self.mode = Mode::Fullscreen { path, name, scroll, index };
                if let Some(file) = file {
                    self.open_image(&file);
                }
            }
            _ => self.mode = Mode::Fullscreen { path, name, scroll, index },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::Color;
    use ratatui_image::picker::Picker;

    use crate::board::test_support::*;
    use crate::keys::Act;
    use crate::board::images::{Graphics, spawn_decoder};
    use crate::board::Mode;

    #[test]
    fn s_opens_the_card_window_with_its_text_images_and_whether_it_is_current() {
        let mut app = carded(false);
        assert!(app.choices().iter().any(|choice| choice.act == Some(Act::Card) && choice.label == "Show the card"));
        app.handle_normal(key(KeyCode::Char('s')));
        assert!(matches!(app.mode, Mode::Card { .. }));
        let (rows, _) = render(&mut app, 170, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("search-filters · card") && screen.contains("○ written at abcdef0, code changed since"), "{screen}");
        assert!(screen.contains("A third line stays in the window.") && screen.contains("▣ shots/filters.png  the filters"), "with graphics off an image is its line: {screen}");
        assert!(screen.contains("↑ ↓ scroll · p push · esc close"), "{screen}");
        press(&mut app, key(KeyCode::Down));
        assert!(matches!(app.mode, Mode::Card { scroll: 1, .. }));
        press(&mut app, key(KeyCode::Char('q')));
        assert!(matches!(app.mode, Mode::Normal), "q closes the window");
        let mut fresh = carded(true);
        fresh.handle_normal(key(KeyCode::Char('s')));
        let (rows, _) = render(&mut fresh, 84, 40);
        assert!(rows.join("\n").contains("✓ current at abcdef0"), "the window shows in a narrow board too");
        press(&mut fresh, key(KeyCode::Char('p')));
        assert!(!matches!(fresh.mode, Mode::Card { .. }), "p goes on to push");
    }

    #[test]
    fn a_tall_image_under_the_cards_text_still_draws_in_a_short_window() {
        let dir = crate::sh::test_dir("tall-card");
        let png = dir.join("tall.png");
        image::RgbImage::from_pixel(40, 120, image::Rgb([200, 30, 30])).save(&png).unwrap();
        let mut app = carded(true);
        let text = format!("One.\nTwo.\nThree.\nFour.\n![tall]({})", png.display());
        std::rc::Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card.as_mut().unwrap().0.text = text;
        app.graphics = Graphics::Ready(Picker::halfblocks());
        (app.image_requests, app.image_results) = spawn_decoder().map(|(requests, results)| (Some(requests), Some(results))).unwrap();
        app.handle_normal(key(KeyCode::Char('s')));
        render(&mut app, 120, 30);
        let waited = Instant::now();
        while app.decoded.get(&png).is_none_or(Option::is_none) && waited.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
            app.take_images();
        }
        let (rows, buffer) = render(&mut app, 120, 30);
        let red = Color::Rgb(200, 30, 30);
        assert!((0..30u16).any(|y| (0..120u16).any(|x| buffer[(x, y)].bg == red || buffer[(x, y)].fg == red)), "the image is drawn without scrolling: {}", rows.join("\n"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_card_window_image_takes_the_windows_width_and_a_halfblock_one_says_it_is_a_preview() {
        let dir = PathBuf::from(format!("/tmp/kw{}", std::process::id()));
        let (mut app, wide, _) = imaged(&dir);
        let (rows, buffer) = render(&mut app, 120, 60);
        let screen = rows.join("\n");
        let (x, y) = find(&rows, &format!("▣ {}  wide", wide.display()))[0];
        let red = (200, 30, 30);
        let drawn: Vec<u16> = (0..120).filter(|column| coloured(&buffer[(*column, y as u16 + 1)], red)).collect();
        let body = 120 * 92 / 100 - 4;
        assert_eq!((drawn.first().copied(), drawn.len() as u16), (Some(x as u16), body), "a 40×20 image fills the window's {body} columns: {screen}");
        assert!(screen.contains("preview · f larger"), "{screen}");
        assert!(!screen.contains("o opens the image") && !screen.contains("o open"), "no viewer, no o: {screen}");
        assert!(screen.contains("↑ ↓ scroll · f fullscreen · p push · esc close"), "{screen}");
        assert!(buffer[(x as u16 + 2, y as u16)].symbol().starts_with(&format!("\x1b]8;;file://{}", wide.display())), "the path is a file link");
        app.viewer = Some("true");
        let (rows, _) = render(&mut app, 120, 60);
        let screen = rows.join("\n");
        assert!(screen.contains("preview · f larger · o opens the image") && screen.contains("↑ ↓ scroll · f fullscreen · o open · p push · esc close"), "{screen}");
        press(&mut app, key(KeyCode::Char('o')));
        assert!(matches!(app.mode, Mode::Card { .. }) && app.notice.is_none(), "o hands the image to the viewer and stays in the window");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn f_shows_the_image_alone_over_the_board_and_arrows_go_between_images() {
        let dir = PathBuf::from(format!("/tmp/kf{}", std::process::id()));
        let (mut app, wide, tall) = imaged(&dir);
        render(&mut app, 120, 60);
        press(&mut app, key(KeyCode::Char('f')));
        assert!(matches!(app.mode, Mode::Fullscreen { index: 0, .. }));
        let (rows, buffer) = render(&mut app, 120, 60);
        let screen = rows.join("\n");
        let red: Vec<(u16, u16)> = (0..60).flat_map(|y| (0..120).map(move |x| (x, y))).filter(|at| coloured(&buffer[*at], (200, 30, 30))).collect();
        let columns = red.iter().map(|(x, _)| *x).max().unwrap() - red.iter().map(|(x, _)| *x).min().unwrap() + 1;
        assert_eq!(columns, 120, "the wide image spans the whole board: {screen}");
        assert!(screen.contains(&format!("preview · 1/2 {} · ← → images · esc back", wide.display())), "{screen}");
        assert!(!screen.contains("Refunds show."), "the image is alone: {screen}");
        press(&mut app, key(KeyCode::Right));
        let (rows, buffer) = render(&mut app, 120, 60);
        assert!(rows.join("\n").contains(&format!("2/2 {}", tall.display())));
        let blue: Vec<u16> = (0..60).filter(|y| (0..120).any(|x| coloured(&buffer[(x, *y)], (30, 30, 200)))).collect();
        assert_eq!(blue.len(), 59, "the tall image takes the height above the key line");
        press(&mut app, key(KeyCode::Right));
        assert!(matches!(app.mode, Mode::Fullscreen { index: 0, .. }), "→ wraps around");
        press(&mut app, key(KeyCode::Left));
        assert!(matches!(app.mode, Mode::Fullscreen { index: 1, .. }));
        for back in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('f')] {
            press(&mut app, key(back));
            assert!(matches!(app.mode, Mode::Card { .. }), "{back:?} goes back to the card");
            press(&mut app, key(KeyCode::Char('f')));
        }
        app.graphics = Graphics::Off;
        app.mode = Mode::Card { path: PathBuf::from("/work/wt2.web"), name: "search-filters".to_string(), scroll: 0 };
        let (rows, _) = render(&mut app, 120, 60);
        assert!(rows.join("\n").contains("↑ ↓ scroll · p push · esc close"), "without graphics there is nothing to show larger");
        press(&mut app, key(KeyCode::Char('f')));
        assert!(matches!(app.mode, Mode::Card { .. }));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
