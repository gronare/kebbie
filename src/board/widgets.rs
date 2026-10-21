use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use crate::keys::{self, Act};
use crate::messages;

pub const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
pub const SUBTEXT: Color = Color::Rgb(0xa6, 0xad, 0xc8);
pub const MUTED: Color = Color::Rgb(0x6c, 0x70, 0x86);
pub const FAINT: Color = Color::Rgb(0x45, 0x47, 0x5a);
pub const SURFACE: Color = Color::Rgb(0x31, 0x32, 0x44);
pub const MANTLE: Color = Color::Rgb(0x18, 0x18, 0x25);
pub const MAUVE: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
pub const PEACH: Color = Color::Rgb(0xfa, 0xb3, 0x87);
pub const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
pub const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
pub const BLUE: Color = Color::Rgb(0x89, 0xb4, 0xfa);
pub const TEAL: Color = Color::Rgb(0x94, 0xe2, 0xd5);
pub const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);

pub struct Choice {
    pub key: char,
    pub act: Option<Act>,
    pub label: String,
    pub about: String,
}

pub fn choice(act: Act, label: &str, about: &str) -> Choice {
    Choice { key: keys::key(act), act: Some(act), label: label.to_string(), about: about.to_string() }
}

pub fn style(color: Color) -> Style {
    Style::new().fg(color)
}

pub fn bold(color: Color) -> Style {
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

pub fn plural(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

pub fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(10);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let last = lines.last_mut().unwrap();
        let keep = width.saturating_sub(1).min(last.chars().count());
        *last = last.chars().take(keep).collect::<String>() + "…";
    }
    lines
}

pub fn column_title(title: &str, right: String, focused: bool, width: usize) -> Vec<Line<'static>> {
    let gap = width.saturating_sub(title.chars().count() + right.chars().count()).max(1);
    vec![
        Line::from(vec![Span::styled(title.to_string(), bold(if focused { MAUVE } else { MUTED })), Span::raw(" ".repeat(gap)), Span::styled(right, style(MUTED))]),
        Line::from(Span::styled("─".repeat(width), style(SURFACE))),
    ]
}

pub(super) fn section_header(label: &str, extra: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(label.to_string(), bold(TEAL))];
    for span in extra {
        spans.push(Span::raw(" "));
        spans.push(span);
    }
    let used: usize = spans.iter().map(Span::width).sum();
    let mut line = fit(spans, width);
    if used + 2 <= width {
        line.spans.push(Span::raw(" "));
        line.spans.push(Span::styled("─".repeat(width - used - 1), style(FAINT)));
    }
    line
}

pub(super) fn rule(width: usize) -> Line<'static> {
    Line::from(Span::styled("─".repeat(width), style(FAINT)))
}

pub fn group_line(title: &str, count: usize, color: Color) -> Line<'static> {
    Line::from(vec![Span::styled(title.to_string(), bold(color)), Span::styled(format!("  {count}"), style(FAINT))])
}

// The name gives way when the column is narrow; the facts stay readable on the right.
pub fn row(selected: bool, focused: bool, icon: Span<'static>, name: &str, place: Option<&str>, facts: &str, width: usize) -> Line<'static> {
    let place_text = place.map(|place| if facts.is_empty() { place.to_string() } else { format!("{place} · ") }).unwrap_or_default();
    let wanted_right = place_text.chars().count() + facts.chars().count();
    let name_floor = name.chars().count().min(width * 45 / 100);
    let right_width = wanted_right.min(width.saturating_sub(name_floor + 5));
    let name_width = width.saturating_sub(right_width + 5);
    let shown: String = if name.chars().count() > name_width && name_width > 1 {
        name.chars().take(name_width - 1).collect::<String>() + "…"
    } else {
        name.to_string()
    };
    let padding = name_width.saturating_sub(shown.chars().count());
    let bar = if selected { Span::styled("▌", style(if focused { MAUVE } else { FAINT })) } else { Span::raw(" ") };
    let name_style = if selected && focused { bold(TEXT) } else { style(TEXT) };
    let facts: String = facts.chars().take(right_width.saturating_sub(place_text.chars().count())).collect();
    let line = Line::from(vec![
        bar, icon, Span::raw(" "), Span::styled(shown, name_style), Span::raw(" ".repeat(padding + 2)),
        Span::styled(place_text, style(TEAL)), Span::styled(facts, style(MUTED)),
    ]);
    if selected && focused { line.style(Style::new().bg(SURFACE)) } else { line }
}

// A line's background only covers its text, so highlighted lines are padded to the column's width.
pub fn fill(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    lines.into_iter().map(|mut line| {
        if line.style.bg == Some(SURFACE) && line.width() < width {
            let missing = width - line.width();
            line.spans.push(Span::raw(" ".repeat(missing)));
        }
        line
    }).collect()
}

pub fn chips(chips: Vec<(String, Color)>) -> Line<'static> {
    let mut spans = vec![Span::raw("   ")];
    for (text, color) in chips {
        spans.push(Span::styled(format!(" {text} "), Style::new().fg(color).bg(MANTLE)));
        spans.push(Span::raw(" "));
    }
    Line::from(spans).style(Style::new().bg(SURFACE))
}

pub fn ago(at: u64) -> String {
    match messages::age(at).as_str() {
        "now" => "just now".to_string(),
        age => format!("{age} ago"),
    }
}

pub fn card_text(label: &str, text: &str, accent: Color, width: usize, max_lines: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![Span::raw("   "), Span::styled("┃ ", style(accent)), Span::styled(label.to_string(), style(MUTED))])];
    for text in wrap(text, width.saturating_sub(6), max_lines) {
        lines.push(Line::from(vec![Span::raw("   "), Span::styled("┃ ", style(accent)), Span::styled(text, style(SUBTEXT))]));
    }
    lines.into_iter().map(|line| line.style(Style::new().bg(SURFACE))).collect()
}

pub fn cut(text: &str, width: usize) -> String {
    if Span::raw(text).width() <= width {
        return text.to_string();
    }
    let mut used = 0;
    let mut shown: String = text.chars().take_while(|ch| {
        used += Span::raw(ch.encode_utf8(&mut [0; 4]).to_string()).width();
        used < width
    }).collect();
    shown.push('…');
    shown
}

pub fn options_hint() -> Line<'static> {
    Line::from(vec![Span::raw("   "), Span::styled("⏎", style(MAUVE)), Span::styled(" options", style(MUTED))]).style(Style::new().bg(SURFACE))
}

pub fn render_column<H: Copy>(frame: &mut Frame, area: Rect, lines: Vec<Line<'static>>, selected_line: usize, rows: Vec<(usize, usize, H)>, hits: &mut Vec<(Rect, H)>) {
    let lines = fill(lines, area.width as usize);
    let scroll = selected_line.saturating_sub(area.height.saturating_sub(6) as usize);
    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), area);
    for (start, end, hit) in rows {
        let top = start.max(scroll) - scroll;
        let bottom = end.saturating_sub(scroll).min(area.height as usize);
        if bottom > top {
            hits.push((Rect::new(area.x, area.y + top as u16, area.width, (bottom - top) as u16), hit));
        }
    }
}

pub fn hints(keys: &[(&str, &str)]) -> Line<'static> {
    Line::from(keys.iter().flat_map(|(key, label)| [Span::styled(key.to_string(), style(MAUVE)), Span::styled(format!(" {label}    "), style(MUTED))]).collect::<Vec<_>>())
}

pub(super) fn fit(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut left = width;
    let mut out = Vec::new();
    for span in spans {
        let count = span.width();
        if count <= left {
            left -= count;
            out.push(span);
        } else {
            out.push(Span::styled(cut(&span.content, left), span.style));
            break;
        }
    }
    Line::from(out)
}

pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

// Sized to the wrapped text, so a long line in a narrow pane cannot push the last line, often the keys, out of the window.
pub fn modal(frame: &mut Frame, title: String, lines: Vec<Line<'static>>, width: u16) -> Rect {
    let block = modal_block(title);
    let width = width.min(frame.area().width.saturating_sub(4));
    let rows = Paragraph::new(lines.clone()).wrap(Wrap { trim: false }).line_count(width.saturating_sub(4));
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    let area = centered(frame.area(), width, rows as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
    area
}

pub fn modal_block(title: String) -> Block<'static> {
    Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), bold(MAUVE)))
}

pub fn options_lines(choices: Vec<Choice>, index: usize, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (position, choice) in choices.iter().enumerate() {
        let active = position == index;
        let label_style = if active { bold(MAUVE) } else { style(TEXT) };
        let key = choice.key.to_string();
        let gap = width.saturating_sub(choice.label.chars().count() + key.chars().count() + 2).max(1);
        let background = if active { Style::new().bg(SURFACE) } else { Style::new() };
        lines.push(Line::from(vec![
            Span::raw(if active { "▸ " } else { "  " }), Span::styled(choice.label.clone(), label_style),
            Span::raw(" ".repeat(gap)), Span::styled(key, style(MUTED)),
        ]).style(background));
        lines.push(Line::from(vec![Span::raw("  "), Span::styled(choice.about.clone(), style(MUTED))]).style(background));
    }
    lines.push(Line::from(Span::styled("↑ ↓ choose · ⏎ run · esc close", style(FAINT))));
    lines
}

// A menu in groups, as the task menu draws it; the second value is each choice's line, which the mouse hits are placed on.
pub fn menu_lines(choices: &[Choice], index: usize, width: usize, running: impl Fn(Option<Act>) -> bool, group_of: impl Fn(Option<Act>) -> &'static str) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut at = Vec::new();
    let mut group = "";
    for (position, choice) in choices.iter().enumerate() {
        if group_of(choice.act) != group {
            group = group_of(choice.act);
            lines.push(Line::from(Span::styled(group, style(MUTED))));
        }
        let active = position == index;
        let ending = matches!(choice.act, Some(Act::Reset | Act::Drop | Act::Delete));
        let label_style = match (active, ending) {
            (true, true) => bold(RED),
            (true, false) => bold(MAUVE),
            (false, true) => style(RED),
            (false, false) => style(TEXT),
        };
        let dot = if running(choice.act) { Span::styled("● ", style(GREEN)) } else { Span::raw("  ") };
        let gap = width.saturating_sub(choice.label.chars().count() + 5).max(1);
        at.push(lines.len());
        lines.push(Line::from(vec![
            Span::raw(if active { "▸ " } else { "  " }), Span::styled(choice.label.clone(), label_style), Span::raw(" ".repeat(gap)), dot, Span::styled(choice.key.to_string(), style(MUTED)),
        ]).style(if active { Style::new().bg(SURFACE) } else { Style::new() }));
    }
    lines.push(Line::from(Span::styled("─".repeat(width), style(SURFACE))));
    let rows = |about: &str| Paragraph::new(about.to_string()).wrap(Wrap { trim: false }).line_count(width as u16);
    let tallest = choices.iter().map(|choice| rows(&choice.about)).max().unwrap_or(0);
    if let Some(choice) = choices.get(index) {
        lines.push(Line::from(Span::styled(choice.about.clone(), style(SUBTEXT))));
        lines.extend((rows(&choice.about)..tallest).map(|_| Line::default()));
    }
    lines.push(Line::from(Span::styled("↑ ↓ choose · ⏎ run · a letter runs it directly · esc close", style(FAINT))));
    (lines, at)
}

#[cfg(test)]
mod tests {
    use ratatui::text::{Line, Span};

    use super::*;
    use crate::board::detail::changed_text;

    #[test]
    fn a_modal_keeps_its_last_line_when_a_long_line_wraps() {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 20)).unwrap();
        let lines = vec![Line::from("x ".repeat(80)), Line::default(), Line::from("y push · esc cancel")];
        terminal.draw(|frame| { modal(frame, "push".to_string(), lines, 96); }).unwrap();
        let screen: String = terminal.backend().buffer().content().iter().map(|cell| cell.symbol()).collect();
        assert!(screen.contains("y push · esc cancel"), "{screen}");
    }

    #[test]
    fn a_menu_keeps_its_height_while_the_choice_moves() {
        let choices = vec![choice(Act::Go, "Go", "short"), choice(Act::Chat, "Chat", &"a long help text that wraps ".repeat(6))];
        let heights: Vec<usize> = (0..choices.len()).map(|index| {
            let (lines, _) = menu_lines(&choices, index, 48, |_| false, |_| "SESSION");
            Paragraph::new(lines).wrap(Wrap { trim: false }).line_count(48)
        }).collect();
        assert_eq!(heights[0], heights[1], "a centred popup that changes height moves every row on each step");
    }

    #[test]
    fn text_is_cut_at_a_width_by_characters_and_fitted_span_by_span() {
        assert_eq!(cut("abc", 3), "abc");
        assert_eq!(cut("abcd", 3), "ab…");
        assert_eq!(cut("åäöü", 3), "åä…");
        assert_eq!(cut("abc", 0), "…", "a zero width still yields the ellipsis, never a panic");
        assert_eq!(cut("退款显示在页面", 7), "退款显…", "a wide character takes two cells");
        assert_eq!(fit(vec![Span::raw("ab"), Span::raw("退款显示")], 7).to_string(), "ab退款…", "the cut line fits its seven cells");
        let line = fit(vec![Span::raw("12345"), Span::raw("678"), Span::raw("90")], 7);
        assert_eq!(line.to_string(), "123456…");
        assert_eq!(line.spans.len(), 2, "the span that overflows is the last one");
        assert_eq!(fit(vec![Span::raw("ab")], 10).to_string(), "ab");
        assert_eq!(changed_text(&["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()], 100), "a.rs, b.rs, c.rs");
        assert_eq!(changed_text(&["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()], 14), "a.rs, b.rs +1");
        assert_eq!(changed_text(&["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()], 12), "a.rs +2");
        assert_eq!(changed_text(&["a-very-long-name.rs".to_string(), "b.rs".to_string()], 4), "a-very-long-name.rs +1", "the first name always shows");
        assert_eq!(changed_text(&[], 10), "");
    }

    #[test]
    fn wrap_truncates_at_the_line_limit_with_an_ellipsis_and_keeps_long_words() {
        assert_eq!(wrap("one two three four five six", 9, 2), ["one two", "three fou…"], "the width floor is ten, and the last kept line ends in an ellipsis");
        assert_eq!(wrap("one two three four five six", 12, 2), ["one two", "three four…"]);
        assert_eq!(wrap("supercalifragilistic", 5, 3), ["supercalifragilistic"], "a word is never split");
        assert_eq!(wrap("", 20, 3), Vec::<String>::new());
        assert_eq!(wrap("a  b\n\n c", 20, 3), ["a b c"], "whitespace collapses");
    }
}
