use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use super::review::{Hit, Panel, View};
use super::review_data::{Entry, File, Mark, short};
use crate::board::widgets::{BLUE, FAINT, GREEN, LAVENDER, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, TEAL, TEXT, YELLOW, bold, cut, fill, fit, modal, split_at_width, style, text_width, wrap};

const MIN_FILE_ROWS: usize = 5;

type Tab = (Vec<Span<'static>>, Option<Hit>);

type TabHit = (usize, usize, usize, Hit);

fn tab(parts: &[(String, ratatui::style::Color)], selected: bool) -> Vec<Span<'static>> {
    let edge = style(if selected { MAUVE } else { FAINT });
    let inside = if selected { Style::new().bg(SURFACE).add_modifier(Modifier::BOLD) } else { Style::new() };
    let mut spans = vec![Span::styled("▕", edge), Span::styled(" ", inside)];
    for (index, (text, color)) in parts.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" ", inside));
        }
        spans.push(Span::styled(text.clone(), inside.fg(*color)));
    }
    spans.extend([Span::styled(" ", inside), Span::styled("▏", edge)]);
    spans
}

fn mark(entry: &Entry) -> (&'static str, ratatui::style::Color) {
    match entry.mark {
        Mark::Committed => ("●", YELLOW),
        Mark::Accepted => ("✓", GREEN),
        Mark::InProgress => ("◐", BLUE),
        Mark::Planned => ("○", MUTED),
    }
}

fn tabs(view: &View) -> Vec<Tab> {
    let all = view.selected.is_none();
    let mut out = vec![(tab(&[("all".to_string(), if all { TEXT } else { SUBTEXT })], all), Some(Hit::All))];
    if view.entries.pushed > 0 {
        out.push((tab(&[(format!("↑ {} pushed", view.entries.pushed), MUTED)], false), None));
    }
    for (index, entry) in view.entries.items.iter().enumerate() {
        let selected = entry.sha.is_some() && entry.sha == view.selected;
        let (symbol, color) = mark(entry);
        let number = if selected { TEXT } else if entry.sha.is_none() { MUTED } else { SUBTEXT };
        out.push((tab(&[(symbol.to_string(), color), (entry.number.to_string(), number)], selected), entry.sha.as_ref().map(|_| Hit::Entry(index))));
    }
    out
}

fn flow(items: Vec<Tab>, width: usize) -> (Vec<Line<'static>>, Vec<TabHit>) {
    let (mut lines, mut hits) = (vec![Vec::new()], Vec::new());
    let mut used = 0;
    for (spans, hit) in items {
        let cells: usize = spans.iter().map(Span::width).sum();
        if used > 0 && used + 1 + cells > width {
            lines.push(Vec::new());
            used = 0;
        }
        if used > 0 {
            lines.last_mut().unwrap().push(Span::raw(" "));
            used += 1;
        }
        if let Some(hit) = hit {
            hits.push((lines.len() - 1, used, cells, hit));
        }
        lines.last_mut().unwrap().extend(spans);
        used += cells;
    }
    (lines.into_iter().map(Line::from).collect(), hits)
}

fn bar() -> Span<'static> {
    Span::styled("▎ ", style(MAUVE))
}

fn message(view: &View, width: usize) -> Vec<Line<'static>> {
    let Some((_, entry)) = view.selected_entry() else {
        let what = view.title();
        let text = match (view.entries.committed().next().is_some(), view.uncommitted) {
            (false, _) => format!("Every change {}.", view.since),
            (true, false) => format!("Every unpushed commit as one diff: {what}."),
            (true, true) => format!("Every unpushed commit as one diff: {what}, and the changes not committed yet."),
        };
        return vec![Line::from(Span::styled(text, style(MUTED)))];
    };
    let sha = entry.sha.clone().unwrap_or_default();
    let text = super::review_data::message(&view.root, &sha);
    let mut lines = text.lines();
    let subject = lines.next().unwrap_or(&entry.title).to_string();
    let room = width.saturating_sub(2);
    let mut out = Vec::new();
    let shown = cut(&subject, room.saturating_sub(9));
    out.push(Line::from(vec![bar(), Span::styled(shown, bold(LAVENDER)), Span::raw("  "), Span::styled(short(&sha).to_string(), style(PEACH))]));
    for line in reflow(&lines.collect::<Vec<_>>()) {
        let rows = match line {
            Para::Blank => vec![String::new()],
            Para::Text(text) => wrap(&text, room, usize::MAX),
            Para::Kept(text) => split_at_width(&text, room.max(1)),
        };
        out.extend(rows.into_iter().map(|row| Line::from(vec![bar(), Span::styled(row, style(SUBTEXT))])));
    }
    out
}

#[derive(Debug, PartialEq)]
pub enum Para {
    Blank,
    Text(String),
    Kept(String),
}

fn list_item(line: &str) -> bool {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    line.starts_with("- ") || line.starts_with("* ") || (digits > 0 && (line[digits..].starts_with(". ") || line[digits..].starts_with(") ")))
}

fn trailer(line: &str) -> bool {
    line.split_once(": ").is_some_and(|(key, _)| key.chars().next().is_some_and(|first| first.is_ascii_alphabetic()) && key.chars().all(|character| character.is_ascii_alphanumeric() || character == '-'))
}

// Git bodies are hard-wrapped near 72 columns, so a paragraph's lines are joined before they wrap to the panel; lists, trailers and indented lines keep their own lines.
pub fn reflow(body: &[&str]) -> Vec<Para> {
    let start = body.iter().position(|line| !line.trim().is_empty()).unwrap_or(body.len());
    let end = body.iter().rposition(|line| !line.trim().is_empty()).map_or(start, |last| last + 1);
    let mut out = Vec::new();
    let mut joinable = false;
    for line in &body[start..end] {
        let line = line.trim_end();
        if line.is_empty() {
            out.push(Para::Blank);
            joinable = false;
        } else if line.starts_with([' ', '\t']) || trailer(line) {
            out.push(Para::Kept(line.to_string()));
            joinable = false;
        } else if joinable && !list_item(line) && let Some(Para::Text(text)) = out.last_mut() {
            text.push(' ');
            text.push_str(line);
        } else {
            out.push(Para::Text(line.to_string()));
            joinable = true;
        }
    }
    out
}

fn header(view: &View, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled("review", bold(MAUVE)), Span::styled(format!(" {}", view.branch), style(SUBTEXT)), Span::styled(format!(" · {}", view.since), style(MUTED))];
    if view.entries.task.is_some() {
        let (accepted, waiting) = (view.entries.count(Mark::Accepted), view.entries.count(Mark::Committed));
        if accepted > 0 {
            spans.push(Span::styled(" · ", style(MUTED)));
            spans.push(Span::styled(if waiting == 0 { "all accepted".to_string() } else { format!("{accepted} accepted") }, style(GREEN)));
        }
        if waiting > 0 {
            spans.push(Span::styled(" · ", style(MUTED)));
            spans.push(Span::styled(format!("{waiting} to accept"), style(YELLOW)));
        }
    }
    let used: usize = spans.iter().map(Span::width).sum();
    if used + 8 <= width {
        spans.push(Span::raw(" ".repeat(width - used - 6)));
        spans.push(Span::styled("? keys", style(MUTED)));
    }
    fit(spans, width)
}

fn file_row(file: &File, selected: bool, width: usize) -> Line<'static> {
    let (added, deleted) = if file.added == "-" { ("bin".to_string(), String::new()) } else { (format!("+{}", file.added), if file.new { "new".to_string() } else { format!("-{}", file.deleted) }) };
    let tag = match (file.read, file.other) {
        (true, _) => Span::styled("✓ read", style(GREEN)),
        (false, true) => Span::styled("other", style(MUTED)),
        _ => Span::raw(""),
    };
    let fixed = 2 + 6 + 6 + tag.width() + 1;
    let path = cut(&file.path, width.saturating_sub(fixed).max(4));
    let gap = width.saturating_sub(fixed + text_width(&path) - 1);
    let path_style = match (selected, file.other) {
        (true, _) => bold(TEXT),
        (false, true) => style(MUTED),
        _ => style(TEXT),
    };
    let line = Line::from(vec![
        Span::styled(if selected { "▌" } else { " " }, style(MAUVE)), Span::raw(" "),
        Span::styled(format!("{added:<5} "), style(GREEN)), Span::styled(format!("{deleted:<5} "), style(if file.new { MUTED } else { RED })),
        Span::styled(path, path_style), Span::raw(" ".repeat(gap)), tag,
    ]);
    if selected { line.style(Style::new().bg(SURFACE)) } else { line }
}

fn keys(view: &View) -> Line<'static> {
    if let Some((note, color)) = &view.note {
        return Line::from(Span::styled(note.clone(), style(*color)));
    }
    let pairs: &[(&str, &str)] = if view.typing {
        &[("type", "to filter"), ("!word", "hides"), ("enter", "keeps it"), ("esc", "clears")]
    } else if view.focus == Panel::Diff {
        &[("tab", "back to the steps"), ("shift-tab", "the files"), ("q", "closes")]
    } else {
        &[("← →", "step"), ("↑ ↓", "file"), ("a", "accept"), ("r", "read"), ("/", "filter"), ("tab", "to the diff")]
    };
    let mut spans = Vec::new();
    for (index, (key, label)) in pairs.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", style(MUTED)));
        }
        spans.push(Span::styled(key.to_string(), bold(SUBTEXT)));
        spans.push(Span::styled(format!(" {label}"), style(MUTED)));
    }
    Line::from(spans)
}

fn panel(focused: bool) -> Block<'static> {
    Block::bordered().border_type(BorderType::Rounded).border_style(style(if focused { BLUE } else { FAINT }))
}

fn help() -> Vec<Line<'static>> {
    let heading = |text: &str| Line::from(Span::styled(text.to_string(), bold(MAUVE)));
    let key = |key: &str, about: &str| Line::from(vec![Span::styled(format!("  {key:<11}"), bold(TEXT)), Span::styled(about.to_string(), style(SUBTEXT))]);
    vec![
        heading("Steps and files"),
        key("← →  h l", "the previous or next step; all is the whole branch"),
        key("↑ ↓  j k", "the previous or next file; the diff shows it at once"),
        key("a", "accept the step's commit, or withdraw the acceptance"),
        key("r", "mark the file read, or unread"),
        key("/", "filter the files: words to keep, !word to hide; esc clears"),
        key("m", "hide or show the commit message"),
        key("tab", "the next panel: steps, files, diff; shift-tab goes back"),
        key("enter", "to the diff"),
        key("mouse", "click a step or a file; the wheel scrolls the files"),
        key("q", "close the review"),
        Line::default(),
        heading("Diff (nvim)"),
        key("drag  V", "select lines"),
        key("c", "comment on the line or selection, typed into Claude's box"),
        key("y", "copy the selection to the clipboard"),
        key("o", "edit the file over the diff; ctrl-q back, leader m comments"),
        key("-", "open the file's folder over the diff"),
        key("u  w", "reload the diff when the file changed; line wrap"),
        key("tab", "back to the steps; shift-tab to the files"),
        key("q", "close the review"),
        Line::default(),
        Line::from(Span::styled("Comments wait in Claude's input box until you press Enter there.", style(MUTED))),
    ]
}

pub fn draw(frame: &mut Frame, view: &mut View) {
    view.hits.clear();
    let area = frame.area();
    let (width, height) = (area.width as usize, area.height as usize);
    if width < 20 || height < 8 {
        frame.render_widget(Paragraph::new("widen the review"), area);
        return;
    }
    frame.render_widget(Paragraph::new(header(view, width)), Rect::new(area.x, area.y, area.width, 1));
    frame.render_widget(Paragraph::new(keys(view)), Rect::new(area.x, area.y + area.height - 1, area.width, 1));
    let inner = width - 2;
    let (tab_lines, tab_hits) = flow(tabs(view), inner);
    let mut text = tab_lines.clone();
    let files = view.visible().into_iter().cloned().collect::<Vec<File>>();
    let filtering = view.typing || !view.filter.is_empty();
    let file_floor = files.len().clamp(1, MIN_FILE_ROWS) + usize::from(filtering) + 2;
    if !view.collapsed {
        let room = (height - 2).saturating_sub(file_floor + 2 + tab_lines.len() + 1);
        let mut lines = message(view, inner);
        if lines.len() > room && room > 0 {
            let more = lines.len() - room + 1;
            lines.truncate(room - 1);
            lines.push(Line::from(vec![bar(), Span::styled(format!("… {more} more lines · m hides the message"), style(MUTED))]));
        } else if room == 0 {
            lines.clear();
        }
        if !lines.is_empty() {
            text.push(Line::from(Span::styled("─".repeat(inner), style(FAINT))));
            text.extend(lines);
        }
    }
    let steps = Rect::new(area.x, area.y + 1, area.width, (text.len() + 2) as u16);
    frame.render_widget(Paragraph::new(text).block(panel(view.focus == Panel::Steps)), steps);
    view.hits.push((steps, Hit::Panel(Panel::Steps)));
    for (row, x, cells, hit) in tab_hits {
        view.hits.push((Rect::new(steps.x + 1 + x as u16, steps.y + 1 + row as u16, cells as u16, 1), hit));
    }
    let top = steps.y + steps.height;
    let files_area = Rect::new(area.x, top, area.width, (area.y + area.height - 1).saturating_sub(top));
    let title = Line::from(vec![Span::styled(" FILES", bold(TEAL)), Span::styled(format!(" {} · {} ", files.len(), view.title()), style(MUTED))]);
    let block = panel(view.focus == Panel::Files).title(title);
    let list = block.inner(files_area);
    frame.render_widget(block, files_area);
    view.hits.push((files_area, Hit::Panel(Panel::Files)));
    let mut rows = Vec::new();
    if filtering {
        let cursor = if view.typing { "▏" } else { "" };
        rows.push(Line::from(vec![Span::styled("/ ", style(MAUVE)), Span::styled(format!("{}{cursor}", view.filter), style(TEXT)), Span::styled(if view.typing { "" } else { "   esc clears" }, style(MUTED))]));
    }
    let list_rows = (list.height as usize).saturating_sub(rows.len());
    view.list_rows = list_rows;
    if view.follow && list_rows > 0 {
        view.scroll = view.scroll.min(view.file).max((view.file + 1).saturating_sub(list_rows));
    }
    view.scroll = view.scroll.min(files.len().saturating_sub(list_rows));
    if files.is_empty() {
        rows.push(Line::from(Span::styled(if view.filter.is_empty() { "No changes here." } else { "No file matches the filter." }, style(MUTED))));
    }
    let first_row = list.y + rows.len() as u16;
    for (index, file) in files.iter().enumerate().skip(view.scroll).take(list_rows) {
        let selected = index == view.file;
        view.hits.push((Rect::new(list.x, first_row + (index - view.scroll) as u16, list.width, 1), Hit::File(index)));
        rows.push(file_row(file, selected, list.width as usize));
    }
    frame.render_widget(Paragraph::new(fill(rows, list.width as usize)), list);
    if view.help {
        modal(frame, "keys".to_string(), help(), 78);
    }
}
