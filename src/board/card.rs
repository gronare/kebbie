use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use ratatui::Frame;
use ratatui::layout::Size;
use ratatui_image::picker::ProtocolType;

use crate::keys::{self, Act, Ctx};
use crate::report;
use crate::status::Item;
use super::detail::Detail;
use super::images::{Shown, image_shown, preview_note, render_image};
use super::link::file_link;
use super::{App, Column, Hit, Mode};
use super::widgets::{FAINT, GREEN, MANTLE, MAUVE, MUTED, SURFACE, TEAL, TEXT, YELLOW, bold, centered, fill, fit, section_header, rule, style};

impl App {
    pub(super) fn card_of(&self, item: &Item) -> Option<std::rc::Rc<Detail>> {
        self.details.get(item.path.as_ref()?).filter(|detail| detail.card.is_some()).cloned()
    }

    pub(super) fn selected_has_card(&self) -> bool {
        self.column == Column::Work && self.selected_task().is_some_and(|item| self.card_of(&item).is_some())
    }

    fn card_images(&self, path: &Path) -> Vec<PathBuf> {
        let Some((card, _)) = self.details.get(path).and_then(|detail| detail.card.as_ref()) else { return Vec::new() };
        card_parts(&card.text).1.iter().map(|image| path.join(&image.path)).collect()
    }
}

pub(super) fn card_image_in(line: &str) -> Option<(&str, &str)> {
    line.trim().strip_prefix("![").and_then(|rest| rest.split_once("](")).map(|(alt, rest)| (alt, rest.trim_end_matches(')')))
}

pub(super) struct Section {
    pub(super) label: Option<String>,
    pub(super) inline: bool,
    pub(super) lines: Vec<String>,
}

pub(super) struct CardImage {
    pub(super) alt: String,
    pub(super) path: String,
}

impl CardImage {
    pub(super) fn name(&self) -> String {
        Path::new(&self.path).file_name().map_or_else(|| self.path.clone(), |name| name.to_string_lossy().into_owned())
    }
}

fn section_label(line: &str) -> Option<(String, String)> {
    let (label, rest) = line.trim().split_once(':')?;
    let label = label.trim();
    let words = label.split_whitespace().count();
    let capital = label.chars().next().is_some_and(char::is_uppercase);
    let markdown = label.chars().any(|character| "*_`[]()#>|!<~".contains(character));
    (capital && (1..=3).contains(&words) && !markdown && !rest.starts_with("//")).then(|| (label.to_string(), rest.trim().to_string()))
}

pub(super) fn card_parts(text: &str) -> (Vec<Section>, Vec<CardImage>) {
    let mut sections = vec![Section { label: None, inline: false, lines: Vec::new() }];
    let mut images = Vec::new();
    let mut paragraph = true;
    for line in text.lines() {
        if let Some((alt, path)) = card_image_in(line) {
            images.push(CardImage { alt: alt.to_string(), path: path.to_string() });
            continue;
        }
        let section = sections.last_mut().expect("there is always a section");
        if line.trim().is_empty() {
            if section.lines.last().is_some_and(|last| !last.is_empty()) {
                section.lines.push(String::new());
            }
            paragraph = true;
            continue;
        }
        match paragraph.then(|| section_label(line)).flatten() {
            Some((label, rest)) => sections.push(Section { label: Some(label), inline: !rest.is_empty(), lines: if rest.is_empty() { Vec::new() } else { vec![rest] } }),
            None => section.lines.push(line.to_string()),
        }
        paragraph = false;
    }
    for section in &mut sections {
        while section.lines.last().is_some_and(String::is_empty) {
            section.lines.pop();
        }
    }
    sections.retain(|section| section.label.is_some() || !section.lines.is_empty());
    (sections, images)
}

pub(super) fn rich_wrap(text: &str, width: usize, max_lines: usize, color: Color) -> Vec<Line<'static>> {
    let width = width.max(10);
    let usable = text.matches("**").count() / 2 * 2;
    let mut words: Vec<Vec<(String, bool)>> = Vec::new();
    let mut word: Vec<(String, bool)> = Vec::new();
    let (mut seen, mut strong, mut rest) = (0, false, text);
    while let Some(character) = rest.chars().next() {
        if rest.starts_with("**") && seen < usable {
            (seen, strong, rest) = (seen + 1, !strong, &rest[2..]);
            continue;
        }
        rest = &rest[character.len_utf8()..];
        if character.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        match word.last_mut() {
            Some((part, bolded)) if *bolded == strong => part.push(character),
            _ => word.push((character.to_string(), strong)),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    let mut lines: Vec<(Vec<(String, bool)>, usize)> = Vec::new();
    for word in words {
        let count: usize = word.iter().map(|(part, _)| part.chars().count()).sum();
        match lines.last_mut() {
            Some((parts, used)) if *used + 1 + count <= width => {
                let joined = parts.last().is_some_and(|(_, bolded)| *bolded) && word[0].1;
                parts.push((" ".to_string(), joined));
                parts.extend(word);
                *used += 1 + count;
            }
            _ => lines.push((word, count)),
        }
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some((parts, used)) = lines.last_mut() {
            if *used >= width && let Some((last, _)) = parts.last_mut() {
                last.pop();
            }
            parts.push(("…".to_string(), false));
        }
    }
    lines.into_iter().map(|(parts, _)| {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (part, bolded) in parts {
            let shape = if bolded { bold(color) } else { style(color) };
            match spans.last_mut() {
                Some(span) if span.style == shape => span.content = format!("{}{part}", span.content).into(),
                _ => spans.push(Span::styled(part, shape)),
            }
        }
        Line::from(spans)
    }).collect()
}

fn section_lines(sections: &[Section], width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for section in sections {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        let label = section.label.as_deref().map_or_else(|| "CARD".to_string(), str::to_uppercase);
        lines.push(section_header(&label, Vec::new(), width));
        for text in &section.lines {
            if text.is_empty() {
                lines.push(Line::default());
            } else {
                lines.extend(rich_wrap(text, width, usize::MAX, TEXT));
            }
        }
    }
    lines
}

pub(super) fn card_hint() -> Line<'static> {
    Line::from(vec![Span::raw("   "), Span::styled("⏎", style(MAUVE)), Span::styled(" options · ", style(MUTED)), Span::styled(keys::key(Act::Card).to_string(), style(MAUVE)), Span::styled(" card", style(MUTED))]).style(Style::new().bg(SURFACE))
}

const WIDE: u16 = 120;

pub(super) fn draw_card(frame: &mut Frame, app: &mut App) -> Vec<(Rect, Hit)> {
    let Mode::Card { path, name, scroll, image } = &app.mode else { return Vec::new() };
    let (path, name, scroll, image) = (path.clone(), name.clone(), *scroll, *image);
    let Some(detail) = app.details.get(&path).cloned() else { return Vec::new() };
    let Some((card, current)) = &detail.card else { return Vec::new() };
    let full = frame.area();
    let area = centered(full, (full.width * 92 / 100).max(40), (full.height * 84 / 100).max(12));
    frame.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {name} · card "), bold(MAUVE))).style(Style::new().bg(MANTLE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [status, _, body, line, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    let sha: String = card.sha.chars().take(7).collect();
    let head = if *current { Span::styled(format!("✓ current at {sha}"), style(GREEN)) } else { Span::styled(format!("○ written at {sha}, code changed since"), style(YELLOW)) };
    frame.render_widget(Paragraph::new(Line::from(head)), status);
    let (sections, images) = card_parts(&card.text);
    let image = image.min(images.len().saturating_sub(1));
    let wide = inner.width >= WIDE && !images.is_empty();
    let (text_area, image_area) = if wide {
        let left = (inner.width * 2 / 5).clamp(58, 64);
        let [text, _, bar, _, pane] = Layout::horizontal([Constraint::Length(left), Constraint::Length(2), Constraint::Length(1), Constraint::Length(2), Constraint::Min(1)]).areas(body);
        frame.render_widget(Paragraph::new((0..bar.height).map(|_| Line::from(Span::styled("│", style(FAINT)))).collect::<Vec<_>>()), bar);
        (text, Some(pane))
    } else if images.is_empty() {
        (body, None)
    } else {
        let [text, _, pane] = Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(body.height * 3 / 5)]).areas(body);
        (text, Some(pane))
    };
    let width = text_area.width as usize;
    let mut lines = section_lines(&sections, width);
    let mut list = None;
    if wide {
        lines.push(Line::default());
        lines.push(section_header("IMAGES", Vec::new(), width));
        list = Some(lines.len());
        let longest = images.iter().map(|shown| shown.name().chars().count()).max().unwrap_or(0);
        for (index, shown) in images.iter().enumerate() {
            let active = index == image;
            let name = shown.name();
            let gap = " ".repeat(longest - name.chars().count() + 2);
            let row = fit(vec![
                Span::raw(if active { "▸ " } else { "  " }), Span::styled("▣ ", style(TEAL)), Span::styled(name, if active { bold(TEXT) } else { style(TEXT) }),
                Span::raw(gap), Span::styled(shown.alt.clone(), style(MUTED)),
            ], width);
            lines.push(if active { row.style(Style::new().bg(SURFACE)) } else { row });
        }
    }
    let scroll = scroll.min(lines.len().saturating_sub(text_area.height as usize) as u16);
    if let Mode::Card { scroll: kept, image: chosen, .. } = &mut app.mode {
        (*kept, *chosen) = (scroll, image);
    }
    let mut hits = Vec::new();
    if let Some(first) = list {
        for index in 0..images.len() {
            let Some(top) = ((first + index) as u16).checked_sub(scroll) else { continue };
            if top < text_area.height {
                hits.push((Rect::new(text_area.x, text_area.y + top, text_area.width, 1), Hit::CardImage(index)));
            }
        }
    }
    frame.render_widget(Paragraph::new(fill(lines, width)).scroll((scroll, 0)), text_area);
    if let (Some(pane), Some(shown)) = (image_area, images.get(image)) {
        draw_card_image(frame, app, &path.join(&shown.path), shown, (image, images.len()), pane, wide);
    }
    let keys = card_keys(images.len() > 1, !images.is_empty() && app.protocol().is_some(), !images.is_empty() && app.viewer.is_some());
    frame.render_widget(Paragraph::new(rule(line.width as usize)), line);
    frame.render_widget(Paragraph::new(fit(vec![Span::styled(keys, style(MUTED))], foot.width as usize)), foot);
    hits
}

fn draw_card_image(frame: &mut Frame, app: &mut App, file: &Path, shown: &CardImage, (index, count): (usize, usize), pane: Rect, wide: bool) {
    let name = shown.name();
    let place = Span::styled(format!("{}/{count}", index + 1), style(MUTED));
    let header = if wide {
        fit(vec![place, Span::raw(" "), Span::styled(name.clone(), style(TEXT)), Span::styled(format!("  {}", shown.alt), style(MUTED))], pane.width as usize)
    } else {
        section_header("IMAGE", vec![place, Span::styled(name.clone(), style(TEXT))], pane.width as usize)
    };
    frame.render_widget(Paragraph::new(header), Rect { height: 1, ..pane });
    app.links.extend(file_link(&name, file));
    let note = u16::from(app.protocol() == Some(ProtocolType::Halfblocks));
    let room = Size::new(pane.width, pane.height.saturating_sub(1 + note).max(1));
    let size = app.image_size(file, room);
    let mut below = pane.y + 1;
    if let (Some(size), Some(Some(protocol))) = (size, app.images.get(&(file.to_path_buf(), room.width, room.height))) {
        let rows = size.height.min(room.height);
        render_image(frame, protocol, Rect::new(pane.x, pane.y + 1, size.width.min(pane.width), rows));
        below += rows;
    }
    if image_shown(app.protocol(), size.is_some()) == Shown::Preview && below < pane.bottom() {
        frame.render_widget(Paragraph::new(Span::styled(preview_note(app.viewer.is_some()), style(MUTED))), Rect::new(pane.x, below, pane.width, 1));
    }
}

fn card_keys(switch: bool, fullscreen: bool, open: bool) -> String {
    let map = keys::map();
    let mut keys = Vec::new();
    if switch {
        keys.push(format!("{} image", map.label(Act::Switch)));
    }
    keys.push("↑ ↓ scroll".to_string());
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

pub(super) fn push_card_lines(card: &report::Card, current: bool, width: usize) -> Vec<Line<'static>> {
    let sha: String = card.sha.chars().take(7).collect();
    let mark = if current { Span::styled(format!("✓ current at {sha}"), style(GREEN)) } else { Span::styled(format!("○ written at {sha}, code changed since"), style(YELLOW)) };
    let (sections, images) = card_parts(&card.text);
    let room = width.saturating_sub(2);
    let mut body = Vec::new();
    for section in &sections {
        if !body.is_empty() {
            body.push(Line::default());
        }
        let mut texts = section.lines.clone();
        if let Some(label) = &section.label {
            match texts.first_mut() {
                Some(first) if section.inline => *first = format!("{label}: {first}"),
                _ => texts.insert(0, format!("{label}:")),
            }
        }
        for text in texts {
            if text.is_empty() {
                body.push(Line::default());
            } else {
                body.extend(rich_wrap(&text, room, 4, TEXT));
            }
        }
    }
    for image in &images {
        body.push(Line::from(vec![Span::styled("▣ ", style(TEAL)), Span::styled(image.name(), style(TEXT)), Span::styled(format!("  {}", image.alt), style(MUTED))]));
    }
    if body.len() > CARD_LINES {
        body.truncate(CARD_LINES);
        body.push(Line::from(Span::styled(format!("… the rest in {}", card.file.display()), style(MUTED))));
    }
    let mut lines = vec![section_header("CARD", vec![mark], width)];
    lines.extend(body.into_iter().map(|line| indented(line, 2)));
    lines
}

pub(super) fn indented(mut line: Line<'static>, by: usize) -> Line<'static> {
    if !line.spans.is_empty() {
        line.spans.insert(0, Span::raw(" ".repeat(by)));
    }
    line
}

// The sign-off card the task wrote when it reported ready, as the user reads it before pushing; an image shows as its path.
pub(super) fn card_lines(card: &report::Card, current: bool, width: usize) -> Vec<Line<'static>> {
    let mut head = vec![Span::styled("card", bold(TEAL)), Span::styled(format!(" · written at {}", card.sha.chars().take(7).collect::<String>()), style(MUTED))];
    if !current {
        head.push(Span::styled(" · the code changed since", style(YELLOW)));
    }
    let mut body = Vec::new();
    for line in card.text.lines() {
        match card_image_in(line) {
            Some((alt, path)) => body.push(Line::from(vec![Span::styled("▣ ", style(TEAL)), Span::styled(path.to_string(), style(TEXT)), Span::styled(format!("  {alt}"), style(MUTED))])),
            None if line.trim().is_empty() => body.push(Line::default()),
            None => body.extend(rich_wrap(line, width, 4, TEXT)),
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
    pub(super) fn handle_card(&mut self, path: PathBuf, name: String, scroll: u16, image: usize, key: KeyEvent) {
        let map = keys::map();
        let files = self.card_images(&path);
        let count = files.len();
        let (mut scroll, mut image) = (scroll, image.min(count.saturating_sub(1)));
        let mut open = None;
        match map.act(Ctx::Card, &key) {
            Some(Act::Move) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => scroll = scroll.saturating_sub(1),
            Some(Act::Move) => scroll = scroll.saturating_add(1),
            Some(Act::Switch) if count > 0 => image = if key.code == KeyCode::Left { (image + count - 1) % count } else { (image + 1) % count },
            Some(Act::Push) => return self.act(Act::Push),
            Some(Act::Back | Act::Close) => return,
            Some(Act::Fullscreen) if count > 0 && self.protocol().is_some() => {
                self.mode = Mode::Fullscreen { path, name, scroll, index: image };
                return;
            }
            Some(Act::OpenImage) => open = files.get(image).cloned(),
            _ => match key.code {
                KeyCode::PageUp => scroll = scroll.saturating_sub(10),
                KeyCode::PageDown | KeyCode::Char(' ') => scroll = scroll.saturating_add(10),
                _ => {}
            },
        }
        self.mode = Mode::Card { path, name, scroll, image };
        if let Some(file) = open {
            self.open_image(&file);
        }
    }

    pub(super) fn handle_fullscreen(&mut self, path: PathBuf, name: String, scroll: u16, index: usize, key: KeyEvent) {
        let map = keys::map();
        let count = self.card_images(&path).len().max(1);
        match map.act(Ctx::Card, &key) {
            Some(Act::Back | Act::Close | Act::Fullscreen) => self.mode = Mode::Card { path, name, scroll, image: index },
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
    use ratatui::style::{Color, Modifier};
    use ratatui_image::picker::Picker;

    use super::{card_parts, rich_wrap};
    use crate::board::test_support::*;
    use crate::keys::Act;
    use crate::board::images::{Graphics, spawn_decoder};
    use crate::board::widgets::TEXT;
    use crate::board::{App, Mode};

    fn with_text(app: &mut App, text: &str) {
        std::rc::Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card.as_mut().unwrap().0.text = text.to_string();
    }

    const LABELLED: &str = "Refunds show on the **billing page** for every paid invoice.\n\nTo check: /billing on desktop and phone width.\n\nPlease confirm:\nOn the phone the column folds under the amount.\n![the refund row](/tmp/kelpie-shots/refund-row-desktop.png)\n![at 390 px](/tmp/kelpie-shots/refund-row-phone.png)";

    #[test]
    fn a_short_capitalised_label_at_a_paragraphs_start_opens_a_section_and_nothing_else_does() {
        let (sections, images) = card_parts(LABELLED);
        let labels: Vec<Option<&str>> = sections.iter().map(|section| section.label.as_deref()).collect();
        assert_eq!(labels, [None, Some("To check"), Some("Please confirm")]);
        assert_eq!(sections[1].lines, ["/billing on desktop and phone width."], "the rest of the label's line is its first text");
        assert_eq!(sections[2].lines, ["On the phone the column folds under the amount."]);
        assert_eq!(images.iter().map(|image| (image.name(), image.alt.as_str())).collect::<Vec<_>>(), [("refund-row-desktop.png".to_string(), "the refund row"), ("refund-row-phone.png".to_string(), "at 390 px")]);
        for plain in [
            "The filters use three of the parts: price, size and colour.",
            "Https://example.com/refunds is where it lives.",
            "**Bold**: not a label.",
            "lowercase: not a label.",
            "First line.\nTo check: mid-paragraph, so not a label.",
        ] {
            let (sections, _) = card_parts(plain);
            assert!(sections.len() == 1 && sections[0].label.is_none(), "{plain:?} stays one CARD section");
        }
    }

    #[test]
    fn bold_shows_without_its_stars_also_where_it_wraps() {
        let lines = rich_wrap("Plain then **a bold run that is long enough to wrap** and plain.", 20, usize::MAX, TEXT);
        let shown: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        assert_eq!(shown, ["Plain then a bold", "run that is long", "enough to wrap and", "plain."]);
        let bold = |word: &str| lines.iter().flat_map(|line| line.spans.iter()).find(|span| span.content.contains(word)).unwrap().style.add_modifier.contains(Modifier::BOLD);
        assert!(bold("a bold") && bold("run that is long") && bold("enough to wrap"), "the run stays bold on each line it reaches");
        assert!(!bold("Plain then") && !bold("plain."));
        assert_eq!(rich_wrap("an odd ** marker stays", 40, 1, TEXT)[0].to_string(), "an odd ** marker stays");
        let cut = rich_wrap("**The heart is twice as big.** Every thumb", 16, 1, TEXT);
        assert_eq!(cut[0].to_string(), "The heart is…", "a cut line ends in an ellipsis inside its width");
        assert!(cut[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
        let full = rich_wrap("one two three four", 9, 1, TEXT);
        assert!(full[0].to_string().chars().count() <= 10, "{}", full[0]);
    }

    #[test]
    fn s_opens_the_card_window_with_its_text_images_and_whether_it_is_current() {
        let mut app = carded(false);
        assert!(app.choices().iter().any(|choice| choice.act == Some(Act::Card) && choice.label == "Show the card"));
        app.handle_normal(key(KeyCode::Char('s')));
        assert!(matches!(app.mode, Mode::Card { .. }));
        let (rows, _) = render(&mut app, 170, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("search-filters · card") && screen.contains("○ written at abcdef0, code changed since"), "{screen}");
        assert!(screen.contains("A third line stays in the window.") && screen.contains("▸ ▣ filters.png  the filters") && screen.contains("  ▣ empty.png    empty"), "with graphics off the images are listed by name: {screen}");
        assert!(screen.contains("← → image · ↑ ↓ scroll · p push · esc close"), "{screen}");
        press(&mut app, key(KeyCode::Down));
        assert!(matches!(app.mode, Mode::Card { scroll: 1, .. }));
        press(&mut app, key(KeyCode::Char('q')));
        assert!(matches!(app.mode, Mode::Normal), "q closes the window");
        let mut fresh = carded(true);
        with_text(&mut fresh, "One image only.\n![the filters](shots/filters.png)");
        fresh.handle_normal(key(KeyCode::Char('s')));
        let (rows, _) = render(&mut fresh, 84, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("✓ current at abcdef0"), "the window shows in a narrow board too");
        assert!(screen.contains("↑ ↓ scroll · p push · esc close") && !screen.contains("← →"), "one image, nothing to switch: {screen}");
        press(&mut fresh, key(KeyCode::Char('p')));
        assert!(!matches!(fresh.mode, Mode::Card { .. }), "p goes on to push");
    }

    #[test]
    fn a_wide_card_window_puts_the_text_beside_the_image_and_arrows_switch_the_image() {
        let mut app = carded(true);
        with_text(&mut app, LABELLED);
        app.handle_normal(key(KeyCode::Char('s')));
        let (rows, _) = render(&mut app, 160, 48);
        let screen = rows.join("\n");
        let (card_x, card_y) = find(&rows, "CARD ─")[0];
        let (image_x, image_y) = find(&rows, "1/2 refund-row-desktop.png  the refund row")[0];
        assert!(card_y == image_y && image_x > card_x + 58, "text on the left, the image's header on the right: {screen}");
        assert_eq!(rows[card_y + 1].matches('│').count(), 3, "a bar between them, inside the window's borders: {screen}");
        assert!(screen.contains("TO CHECK ─") && screen.contains("PLEASE CONFIRM ─") && screen.contains("IMAGES ─"), "{screen}");
        assert!(screen.contains("billing page for every") && !screen.contains("**"), "{screen}");
        assert!(!screen.contains("/tmp/kelpie-shots"), "the path stays behind the link: {screen}");
        press(&mut app, key(KeyCode::Right));
        let (rows, _) = render(&mut app, 160, 48);
        assert!(rows.join("\n").contains("2/2 refund-row-phone.png  at 390 px") && rows.join("\n").contains("▸ ▣ refund-row-phone.png"));
        press(&mut app, key(KeyCode::Right));
        assert!(matches!(app.mode, Mode::Card { image: 0, .. }), "→ wraps around");
        press(&mut app, key(KeyCode::Left));
        let (rows, _) = render(&mut app, 160, 48);
        assert!(rows.join("\n").contains("2/2 refund-row-phone.png"), "← wraps back");
        let (x, y) = find(&rows, "▣ refund-row-desktop.png")[0];
        click(&mut app, x as u16, y as u16);
        assert!(matches!(app.mode, Mode::Card { image: 0, .. }), "a click on an image in the list selects it");
    }

    #[test]
    fn a_narrow_card_window_stacks_the_text_over_one_image() {
        let mut app = carded(true);
        with_text(&mut app, LABELLED);
        app.handle_normal(key(KeyCode::Char('s')));
        let (rows, _) = render(&mut app, 100, 48);
        let screen = rows.join("\n");
        let (_, card_y) = find(&rows, "CARD ─")[0];
        let (_, image_y) = find(&rows, "IMAGE 1/2 refund-row-desktop.png ─")[0];
        assert!(image_y > card_y + 4 && rows[card_y + 1].matches('│').count() == 2, "{screen}");
        assert!(!screen.contains("IMAGES"), "one image at a time, no list: {screen}");
        press(&mut app, key(KeyCode::Right));
        let (rows, _) = render(&mut app, 100, 48);
        assert!(rows.join("\n").contains("IMAGE 2/2 refund-row-phone.png ─"));
    }

    #[test]
    fn a_tall_image_under_the_cards_text_still_draws_in_a_short_window() {
        let dir = crate::sh::test_dir("tall-card");
        let png = dir.join("tall.png");
        image::RgbImage::from_pixel(40, 120, image::Rgb([200, 30, 30])).save(&png).unwrap();
        let mut app = carded(true);
        with_text(&mut app, &format!("One.\nTwo.\nThree.\nFour.\n![tall]({})", png.display()));
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
    fn every_image_fills_the_pane_whichever_is_selected_and_a_halfblock_one_says_it_is_a_preview() {
        let dir = PathBuf::from(format!("/tmp/kw{}", std::process::id()));
        let (mut app, wide, _) = imaged(&dir);
        let pane = |app: &mut App, colour: (u8, u8, u8)| {
            let (rows, buffer) = render(app, 160, 48);
            let cells: Vec<(u16, u16)> = (0..48).flat_map(|y| (0..160).map(move |x| (x, y))).filter(|at| coloured(&buffer[*at], colour)).collect();
            let span = |pick: fn(&(u16, u16)) -> u16| cells.iter().map(pick).max().unwrap_or(0) + 1 - cells.iter().map(pick).min().unwrap_or(0);
            (rows, cells.len(), span(|at| at.0), span(|at| at.1))
        };
        let (rows, _, columns, _) = pane(&mut app, (200, 30, 30));
        let screen = rows.join("\n");
        let (x, y) = find(&rows, "1/2 wide.png  wide")[0];
        let right = 160 * 92 / 100 - 4 - (x as u16 - (160 - 160 * 92 / 100) / 2 - 2);
        assert_eq!(columns, right, "the first image spans the right pane: {screen}");
        assert!(screen.contains("preview · f larger"), "{screen}");
        assert!(!screen.contains("o opens the image") && !screen.contains("o open"), "no viewer, no o: {screen}");
        assert!(screen.contains("← → image · ↑ ↓ scroll · f fullscreen · p push · esc close"), "{screen}");
        let (rows, buffer) = render(&mut app, 160, 48);
        assert!(buffer[(x as u16 + 4, y as u16)].symbol().starts_with(&format!("\x1b]8;;file://{}", wide.display())), "the file name is a link to the file: {}", rows.join("\n"));
        press(&mut app, key(KeyCode::Right));
        let (rows, _, _, tall_rows) = pane(&mut app, (30, 30, 200));
        let (_, header) = find(&rows, "2/2 tall.png  tall")[0];
        let (_, keys) = find(&rows, "← → image")[0];
        assert_eq!(tall_rows as usize, keys - 1 - header - 2, "the second image takes the pane's height under its header, above the preview note and the rule: {}", rows.join("\n"));
        app.viewer = Some("true");
        let (rows, _) = render(&mut app, 160, 48);
        let screen = rows.join("\n");
        assert!(screen.contains("preview · f larger · o opens the image") && screen.contains("← → image · ↑ ↓ scroll · f fullscreen · o open · p push · esc close"), "{screen}");
        press(&mut app, key(KeyCode::Char('o')));
        assert!(matches!(app.mode, Mode::Card { image: 1, .. }) && app.notice.is_none(), "o hands the image to the viewer and stays in the window");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn f_shows_the_selected_image_alone_over_the_board_and_arrows_go_between_images() {
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
            assert!(matches!(app.mode, Mode::Card { image: 1, .. }), "{back:?} goes back to the card at the image shown");
            press(&mut app, key(KeyCode::Char('f')));
            assert!(matches!(app.mode, Mode::Fullscreen { index: 1, .. }), "f opens at the selected image");
        }
        app.graphics = Graphics::Off;
        app.mode = Mode::Card { path: PathBuf::from("/work/wt2.web"), name: "search-filters".to_string(), scroll: 0, image: 0 };
        let (rows, _) = render(&mut app, 120, 60);
        assert!(rows.join("\n").contains("← → image · ↑ ↓ scroll · p push · esc close"), "without graphics there is nothing to show larger");
        press(&mut app, key(KeyCode::Char('f')));
        assert!(matches!(app.mode, Mode::Card { .. }));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
