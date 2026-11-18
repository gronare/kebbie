use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use crate::phase::{Phase, Why};
use crate::status::{Group, Item};
use super::detail::note_of;
use super::widgets::{MANTLE, MAUVE, MUTED, bold, centered, fit, style};
use super::{App, Mode};

const READ_AGAIN: Duration = Duration::from_secs(30);
const PAGE: u16 = 10;

fn read_key(key: &str) -> String {
    format!("note:{key}")
}

// [[folder/stem#heading|shown]] reads as what Obsidian shows for it; a fenced code block keeps its brackets.
pub(super) fn plain_links(body: &str) -> String {
    let mut fenced = false;
    body.lines().map(|line| {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
            return line.to_string();
        }
        if fenced { line.to_string() } else { unlinked(line) }
    }).collect::<Vec<_>>().join("\n")
}

fn unlinked(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find("[[") {
        let Some(length) = rest[start + 2..].find("]]") else { break };
        let inner = &rest[start + 2..start + 2 + length];
        let before = &rest[..start];
        out.push_str(before.strip_suffix('!').unwrap_or(before));
        out.push_str(&shown(inner));
        rest = &rest[start + 4 + length..];
    }
    out.push_str(rest);
    out
}

fn shown(inner: &str) -> String {
    if let Some((_, alias)) = inner.split_once('|') {
        return alias.trim().to_string();
    }
    let (target, heading) = inner.split_once('#').map_or((inner, None), |(target, heading)| (target, Some(heading)));
    let stem = target.rsplit('/').next().unwrap_or(target);
    let stem = stem.strip_suffix(".md").unwrap_or(stem);
    match heading {
        Some(heading) if stem.is_empty() => heading.to_string(),
        Some(heading) => format!("{stem} › {heading}"),
        None => stem.to_string(),
    }
}

// A task stopped on you whose report sends you to its note, as an investigation's "read the findings in the task note" does.
pub(super) fn reads_note(item: &Item) -> bool {
    let stopped = item.group() == Group::NeedsYou && matches!(item.phase.as_ref().map(|record| &record.phase), Some(Phase::NeedsYou(Why::Report | Why::NothingToPush)));
    let names_note = item.report.as_ref().is_some_and(|report| report.text.split(|character: char| !character.is_alphanumeric()).any(|word| matches!(word.to_lowercase().as_str(), "note" | "notes" | "noten")));
    stopped && item.question.is_none() && names_note
}

impl App {
    pub(super) fn open_task_note(&mut self, item: &Item) {
        if let Some(key) = note_of(self, item) {
            self.mode = Mode::TaskNote { key, name: item.titled(), scroll: 0 };
        }
    }

    pub(super) fn handle_task_note(&mut self, key: String, name: String, scroll: u16, event: KeyEvent, cancels: bool) {
        let again = |scroll: u16| Mode::TaskNote { key: key.clone(), name: name.clone(), scroll };
        self.mode = match event.code {
            KeyCode::Down | KeyCode::Char('j') => again(scroll.saturating_add(1)),
            KeyCode::Up | KeyCode::Char('k') => again(scroll.saturating_sub(1)),
            KeyCode::PageDown | KeyCode::Char(' ') => again(scroll.saturating_add(PAGE)),
            KeyCode::PageUp => again(scroll.saturating_sub(PAGE)),
            KeyCode::Home => again(0),
            KeyCode::End => again(u16::MAX),
            _ if cancels => Mode::Normal,
            _ => again(scroll),
        };
    }
}

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let Mode::TaskNote { key, name, scroll } = &app.mode else { return };
    let (key, name, scroll) = (key.clone(), name.clone(), *scroll);
    let asked = key.clone();
    let read = app.plan_reads.get(&read_key(&key), READ_AGAIN, move || crate::questions::vault_text(&asked));
    let full = frame.area();
    let area = centered(full, (full.width * 92 / 100).max(40), (full.height * 84 / 100).max(16));
    frame.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(style(MAUVE)).padding(Padding::horizontal(1))
        .title(Span::styled(format!(" note · {name} "), bold(MAUVE))).style(Style::new().bg(MANTLE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [header, text, foot] = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let width = inner.width as usize;
    let (paragraph, readable) = match read {
        Some(Some(body)) => {
            let rendered = super::inbox::markdown(&plain_links(&body));
            super::link::note_lines(&rendered.lines);
            (Paragraph::new(rendered).wrap(Wrap { trim: false }), true)
        }
        Some(None) => (Paragraph::new(Span::styled("The vault gave no text for this note; it may have moved or closed.", style(MUTED))), false),
        None => (Paragraph::new(Span::styled("Reading the note…", style(MUTED))), false),
    };
    let last = if readable { (paragraph.line_count(text.width) as u16).saturating_sub(text.height) } else { 0 };
    let scroll = scroll.min(last);
    let place = if last > 0 { format!("vault: {key} · {}%", u32::from(scroll) * 100 / u32::from(last)) } else { format!("vault: {key}") };
    frame.render_widget(Paragraph::new(fit(vec![Span::styled(place, style(MUTED))], width)), header);
    frame.render_widget(paragraph.scroll((scroll, 0)), text);
    let more = if last > 0 { "↓↑ pgdn pgup scroll · " } else { "" };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(format!("{more}esc back"), style(MUTED)))), foot);
    if let Mode::TaskNote { scroll: kept, .. } = &mut app.mode {
        *kept = scroll;
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;

    use super::*;
    use crate::board::Ongoing;
    use crate::board::test_support::*;
    use crate::keys::Act;
    use crate::next::tests::named;

    const KEY: &str = "Projects/auth-audit.md";
    const FINDINGS: &str = "## Findings\n\nThe session cookie lacks **SameSite**, see [[Areas/kebbie-board|the board]] and [[auth-tokens#Rotation]].\n\n- rotate on login\n- expire after 30 days\n\n```rust\nlet kept = [[1, 2]];\n```\n\n| Route | Guard |\n| --- | --- |\n| /login | none |\n";

    fn stopped(report: &str) -> Item {
        let mut item = named(crate::status::tests::task(Some("idle"), Some("needs-you"), 0), "auth-audit", "web");
        item.report.as_mut().unwrap().text = report.to_string();
        crate::status::tests::in_phase(&mut item, Phase::NeedsYou(Why::Report));
        item
    }

    fn with_note(item: Item) -> App {
        let mut app = test_app(vec![item]);
        app.ongoing = vec![Ongoing { key: KEY.to_string(), title: "auth audit".to_string(), task: Some("auth-audit@web".to_string()), ..Ongoing::default() }];
        app.plan_reads.put(&read_key(KEY), Some(FINDINGS.to_string()));
        app
    }

    fn screen(app: &mut App, width: u16, height: u16) -> String {
        render(app, width, height).0.join("\n")
    }

    #[test]
    fn a_wikilink_reads_as_the_words_obsidian_shows_and_code_keeps_its_brackets() {
        assert_eq!(plain_links("see [[Areas/kebbie-board|the board]], [[auth-tokens#Rotation]], [[#Steps]], ![[shot.png]] and [[Projects/x.md]]"), "see the board, auth-tokens › Rotation, Steps, shot.png and x");
        assert_eq!(plain_links("```\nlet a = [[1]];\n```\n[[open"), "```\nlet a = [[1]];\n```\n[[open");
    }

    #[test]
    fn the_key_on_a_task_opens_its_note_rendered_and_scrolls_it() {
        let mut app = with_note(stopped("Done looking."));
        press(&mut app, key(KeyCode::Char(crate::keys::key(Act::Note))));
        assert!(matches!(&app.mode, Mode::TaskNote { key, .. } if key == KEY), "{:?}", std::mem::discriminant(&app.mode));
        let shown = screen(&mut app, 110, 40);
        for wanted in ["note · auth-audit", "vault: Projects/auth-audit.md", "Findings", "SameSite", "the board", "auth-tokens › Rotation", "rotate on login", "let kept = [[1, 2]];", "Route", "/login", "esc back"] {
            assert!(shown.contains(wanted), "{wanted:?} in:\n{shown}");
        }
        for unwanted in ["## Findings", "**SameSite**", "[[Areas", "```", "| --- |"] {
            assert!(!shown.contains(unwanted), "{unwanted:?} is markdown left unrendered:\n{shown}");
        }
        assert!(shown.contains('│') || shown.contains('┃'), "the table is drawn with borders:\n{shown}");
        let long: String = (1..=80).map(|line| format!("finding {line:02} of the audit\n\n")).collect();
        app.plan_reads.put(&read_key(KEY), Some(long));
        let top = screen(&mut app, 110, 30);
        assert!(top.contains("finding 01") && !top.contains("finding 80") && top.contains("pgdn pgup scroll"), "{top}");
        press(&mut app, key(KeyCode::PageDown));
        let paged = screen(&mut app, 110, 30);
        assert!(!paged.contains("finding 01 ") && paged.contains("finding 06"), "pgdn moves ten rows on:\n{paged}");
        press(&mut app, key(KeyCode::End));
        let bottom = screen(&mut app, 110, 30);
        assert!(bottom.contains("finding 80") && bottom.contains("100%"), "the end shows the last line and stops there:\n{bottom}");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn a_report_that_sends_you_to_the_note_points_its_row_options_and_next_straight_at_it() {
        let mut app = with_note(stopped("Read the findings in the task note and pick a fix."));
        assert_eq!(app.choices()[0].label, "Read its note", "⏎ on the row opens the options at the note");
        assert!(screen(&mut app, 110, 50).contains("V read its note"), "the selected row names the key");
        app.open_next();
        app.handle_key(key(KeyCode::Enter), None);
        assert!(matches!(&app.mode, Mode::TaskNote { key, .. } if key == KEY), "⏎ in Next opens the note");
        let mut other = with_note(stopped("Blocked on the staging credentials."));
        assert!(other.choices()[0].label != "Read its note" && other.choices().iter().any(|choice| choice.label == "Read its note"), "a report about something else keeps the note among the options");
        assert!(screen(&mut other, 110, 50).contains("V its note"));
    }

    #[test]
    fn without_a_vault_note_nothing_offers_one_and_the_key_does_nothing() {
        let mut app = test_app(vec![stopped("Read the findings in the task note.")]);
        let board = screen(&mut app, 110, 50);
        assert!(!board.contains("its note") && !app.choices().iter().any(|choice| choice.act == Some(Act::Note)), "no row or menu offers a note there is none of:\n{board}");
        app.open_next();
        app.handle_key(key(KeyCode::Enter), None);
        assert!(!matches!(app.mode, Mode::TaskNote { .. }), "Next opens what it opened before");
        app.mode = Mode::Normal;
        press(&mut app, key(KeyCode::Char(crate::keys::key(Act::Note))));
        assert!(matches!(app.mode, Mode::Normal) && app.toast.is_none(), "the key does nothing where there is no note, as a key with nothing to act on does");
    }
}
