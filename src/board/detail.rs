use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Frame;
use ratatui::layout::Size;

use crate::herdr;
use crate::keys::{self, Act};
use crate::messages;
use crate::permit::Permit;
use crate::peers;
use crate::repo::name_of;
use crate::evidence;
use crate::report;
use crate::status::{self, Item};
use crate::tasks;
use super::card::{card_image_in, card_lines, rich_wrap};
use super::columns::absence;
use super::images::{ImageAt, render_image};
use super::link::{Link, file_link};
use super::{App, Mode};
use super::widgets::{BLUE, GREEN, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, TEAL, TEXT, YELLOW, ago, bold, cut, plural, style, wrap};

#[derive(Default)]
pub(super) struct Detail {
    pub(super) branch: String,
    pub(super) commits: Vec<tasks::Commit>,
    pub(super) changed: Vec<String>,
    pub(super) card: Option<(report::Card, bool)>,
    pub(super) tests: Option<evidence::Shown>,
    pub(super) dev_server: bool,
    pub(super) dev_worker: bool,
    pub(super) server_name: String,
    pub(super) worker_name: String,
}

// Claude's last message is the last "● " block on screen, up to its footer lines.
fn last_message(output: &str) -> String {
    let lines: Vec<&str> = output.lines().map(str::trim_end).collect();
    let bullet = |line: &str| ["● ", "⏺ "].iter().any(|marker| line.trim_start().starts_with(marker));
    let Some(start) = lines.iter().rposition(|line| bullet(line)) else { return String::new() };
    let stop = |line: &str| {
        let line = line.trim_start();
        ["✻", "※", "─", "❯", "⏵", "╭", "╰", "· "].iter().any(|marker| line.starts_with(marker))
    };
    lines[start..].iter().take_while(|line| !stop(line))
        .map(|line| line.trim()).filter(|line| !line.is_empty())
        .collect::<Vec<_>>().join(" ").trim_start_matches("● ").trim_start_matches("⏺ ").to_string()
}

// The end of a long message, where Claude puts its question, cut at a word.
fn tail(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let rest: String = text.chars().skip(count - max_chars).collect();
    format!("…{}", rest.split_once(' ').map_or(rest.as_str(), |(_, words)| words))
}

// Fallback for a session that has not reported yet: Claude Code's own "※ recap:" under a finished run, free text that can be switched off.
fn recap(output: &str) -> String {
    let lines: Vec<&str> = output.lines().map(str::trim_end).collect();
    let Some(start) = lines.iter().rposition(|line| line.starts_with("※ recap:")) else { return String::new() };
    let continued = lines[start + 1..].iter().take_while(|line| line.starts_with("  ") && !line.trim().is_empty());
    std::iter::once(&lines[start]).chain(continued).map(|line| line.trim()).collect::<Vec<_>>().join(" ")
        .trim_start_matches("※ recap:").replace("(disable recaps in /config)", "").trim().to_string()
}

impl App {
    fn detail(&mut self, item: &Item) -> Option<std::rc::Rc<Detail>> {
        let path = item.path.clone()?;
        let asked = self.asked_details.get(&path).is_some_and(|at| at.elapsed() < Duration::from_secs(4));
        if !asked && let Some(requests) = &self.detail_requests {
            let _ = requests.send((path.clone(), item.repo.clone()));
            self.asked_details.insert(path.clone(), Instant::now());
        }
        self.details.get(&path).cloned()
    }

    fn agent_screen(&mut self, pane: &str) -> String {
        if let Some((at, screen)) = self.screens.get(pane)
            && at.elapsed() < Duration::from_secs(4)
        {
            return screen.clone();
        }
        let screen = herdr::read_agent(pane).unwrap_or_default();
        self.screens.insert(pane.to_string(), (Instant::now(), screen.clone()));
        screen
    }
}

// A question asked through kelpie shows from the log; a prompt on screen outranks it.
fn asked_question(item: &Item) -> Option<&messages::Message> {
    item.question.as_ref().filter(|_| item.agent.as_ref().is_none_or(|agent| agent.status != "blocked"))
}

fn question_summary(message: &messages::Message, width: usize) -> String {
    let count = message.question.as_ref().map_or(0, |question| question.options.len());
    let plan = message.question.as_ref().is_some_and(|question| question.plan.is_some());
    let inbox = keys::key(Act::Card);
    let suffix = if plan { format!(" · plan to approve · {inbox}") } else { format!(" · {} · {inbox}", plural(count, "option")) };
    let one_line = message.text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{}{suffix}", cut(&one_line, width.saturating_sub(suffix.chars().count()).max(8)))
}

fn card_field(card: &report::Card, current: bool, width: usize) -> Vec<Line<'static>> {
    let (mark, color) = if current { ("✓ ", GREEN) } else { ("○ ", YELLOW) };
    let text: Vec<&str> = card.text.lines().map(str::trim).filter(|line| !line.is_empty() && card_image_in(line).is_none()).take(2).collect();
    let room = width.saturating_sub(LABEL + 2);
    let mut lines = Vec::new();
    for (index, line) in text.iter().enumerate() {
        let mut spans = if index == 0 { vec![Span::styled(mark, style(color))] } else { vec![Span::raw("  ")] };
        spans.extend(rich_wrap(line, room, 1, TEXT).into_iter().next().map(|first| first.spans).unwrap_or_default());
        lines.push(field(if index == 0 { "card" } else { "" }, spans));
    }
    if text.is_empty() {
        lines.push(field("card", vec![Span::styled(mark, style(color))]));
    }
    let images = card.text.lines().filter(|line| card_image_in(line).is_some()).count();
    if images > 0 {
        lines.push(field("", vec![Span::styled(format!("▣ {} · {} shows the card", plural(images, "image"), keys::key(Act::Card)), style(MUTED))]));
    }
    lines
}

fn verdict_chip(shown: &crate::review::Shown) -> Option<(String, Color)> {
    if shown.running {
        return Some(("review running".to_string(), BLUE));
    }
    let verdict = shown.verdict.as_ref()?;
    let label = if verdict.ok { "review ok".to_string() } else { format!("review: {}", verdict.summary()) };
    Some(match (shown.stale, verdict.ok) {
        (true, _) => (format!("{label} (stale)"), MUTED),
        (false, true) => (label, GREEN),
        (false, false) => (label, PEACH),
    })
}

pub(super) fn permit_chip(permit: Option<Permit>) -> Option<(String, Color)> {
    match permit? {
        Permit::Task => Some(("lead may push".to_string(), GREEN)),
        Permit::Repo => Some(("lead may push once reviewed (repo)".to_string(), TEAL)),
    }
}

pub(super) fn follow_chip(item: &Item) -> Option<(String, Color)> {
    item.follows.then(|| ("follows main".to_string(), BLUE))
}

pub(super) fn bypass_chip(item: &Item) -> Option<(String, Color)> {
    item.bypass.then(|| ("bypass permissions: Claude asks for no permission".to_string(), PEACH))
}

const LABEL: usize = 8;

pub(super) fn field(label: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{label:<LABEL$}"), style(MUTED))];
    spans.extend(value);
    Line::from(spans)
}

fn wrapped_field(label: &str, text: &str, color: Color, width: usize, max_lines: usize) -> Vec<Line<'static>> {
    wrap(text, width.saturating_sub(LABEL), max_lines).into_iter().enumerate()
        .map(|(index, part)| field(if index == 0 { label } else { "" }, vec![Span::styled(part, style(color))])).collect()
}

// The running dev server's address; the shown text is what link_cells finds again on screen.
pub(super) fn server_line(item: &Item, configured: bool, name: &str, width: usize) -> Option<(Line<'static>, Option<Link>)> {
    if !item.server {
        return configured.then(|| (field(name, vec![Span::styled(format!("○ off · {} starts it", keys::key(Act::Server)), style(MUTED))]), None));
    }
    let Some(url) = &item.url else { return Some((field(name, vec![Span::styled("● running", style(GREEN))]), None)) };
    let room = width.saturating_sub(LABEL + 12).max(2);
    let shown = cut(url, room);
    let line = field(name, vec![Span::styled("● running", style(GREEN)), Span::raw("  "), Span::styled(shown.clone(), style(TEAL))]);
    Some((line, Some(Link { shown, url: url.clone() })))
}

fn ci_lines(item: &Item, width: usize) -> (Vec<Line<'static>>, Vec<Link>) {
    let Some(ci) = &item.ci else { return (Vec::new(), Vec::new()) };
    let (mark, color) = if ci.running() { ("◐ ", YELLOW) } else { ("● ", PEACH) };
    let mut lines = vec![field("ci", vec![Span::styled(mark, style(color)), Span::styled(ci.note(), style(TEXT))])];
    let mut links = Vec::new();
    let failed_url = match &ci.state { crate::ci::State::Failed { url, .. } => Some(url.clone()), crate::ci::State::Running => None };
    let mut urls: Vec<(String, String)> = ci.runs.iter().map(|run| (run.title().to_string(), run.url.clone())).collect();
    if urls.is_empty() && let Some(url) = failed_url {
        urls.push(("run".to_string(), url));
    }
    for (title, url) in urls.into_iter().filter(|(_, url)| !url.is_empty()).take(4) {
        let title = cut(&title, 16);
        let shown = cut(&url, width.saturating_sub(LABEL + title.chars().count() + 2).max(2));
        lines.push(field("", vec![Span::styled(title, style(MUTED)), Span::raw("  "), Span::styled(shown.clone(), style(TEAL))]));
        links.push(Link { shown, url });
    }
    (lines, links)
}

// The task's latest recorded test run: green on the code as it is, yellow on older code, red when it failed.
pub(super) fn tests_line(tests: &evidence::Shown, width: usize) -> Line<'static> {
    field("tests", tests_spans(tests, width.saturating_sub(LABEL + 2)))
}

pub(super) fn tests_spans(tests: &evidence::Shown, width: usize) -> Vec<Span<'static>> {
    let color = match tests.mark {
        evidence::Mark::Covered => GREEN,
        evidence::Mark::Stale => YELLOW,
        evidence::Mark::Failed => RED,
    };
    vec![Span::styled(format!("{} ", tests.symbol()), style(color)), Span::styled(cut(&tests.text, width), style(color))]
}

fn worker_line(item: &Item, configured: bool, name: &str) -> Option<Line<'static>> {
    if item.worker {
        Some(field(name, vec![Span::styled("● running", style(GREEN))]))
    } else {
        configured.then(|| field(name, vec![Span::styled(format!("○ off · {} starts it", keys::key(Act::Worker)), style(MUTED))]))
    }
}

fn review_text(item: &Item) -> (String, Color) {
    if let Some(chip) = item.verdict.as_ref().and_then(verdict_chip) {
        return chip;
    }
    match item.review {
        Some((read, total)) => (format!("{read}/{total} read in the split"), if read == total { GREEN } else { SUBTEXT }),
        None => ("not sent".to_string(), MUTED),
    }
}

pub(super) fn changed_text(changed: &[String], width: usize) -> String {
    let mut shown: Vec<&str> = Vec::new();
    for name in changed {
        let used: usize = shown.iter().map(|name| name.chars().count() + 2).sum();
        if used + name.chars().count() + 4 > width && !shown.is_empty() {
            break;
        }
        shown.push(name);
    }
    let rest = changed.len() - shown.len();
    let more = if rest > 0 { format!(" +{rest}") } else { String::new() };
    format!("{}{more}", shown.join(", "))
}

fn report_text(app: &mut App, item: &Item, width: usize) -> Option<(String, String, Color)> {
    let agent = item.agent.as_ref()?;
    let screen = app.agent_screen(&agent.pane);
    let question = |text: String| tail(&text, width.saturating_sub(LABEL) * 4);
    let found = match &item.report {
        _ if agent.status == "blocked" => ("asks".to_string(), question(last_message(&screen)), PEACH),
        Some(report) if report.report.as_ref().is_some_and(|report| report.state == "needs-you") && !last_message(&screen).is_empty() => {
            ("asks".to_string(), question(last_message(&screen)), PEACH)
        }
        Some(report) => {
            let state = report.report.as_ref().map(|report| report.state.replace('-', " ")).unwrap_or_default();
            ("report".to_string(), format!("{state} · {} · {}", ago(report.at), report.text), TEXT)
        }
        None => match recap(&screen) {
            recap if !recap.is_empty() => ("recap".to_string(), recap, SUBTEXT),
            _ => ("said".to_string(), last_message(&screen), SUBTEXT),
        },
    };
    (!found.1.is_empty()).then_some(found)
}

fn state_spans(app: &App, item: &Item) -> Vec<Span<'static>> {
    let mut facts: Vec<(String, Color)> = Vec::new();
    facts.extend(absence(item).map(|absent| (absent.to_string(), YELLOW)));
    facts.extend(item.push_note().map(|note| (note, MUTED)));
    if let Some(git) = &item.git && git.behind > 0 {
        facts.push((format!("{} behind {}", git.behind, git.upstream.trim_start_matches("origin/")), MUTED));
    }
    if let Some(git) = &item.git && git.uncommitted > 0 {
        facts.push((format!("{}, not ready to push", plural(git.uncommitted, "uncommitted file")), YELLOW));
    }
    let halves: Vec<String> = app.tasks.iter().filter(|other| other.name == item.name && other.place != item.place).map(|other| other.place.clone()).collect();
    if !halves.is_empty() {
        facts.push((format!("same task in {}", halves.join(", ")), TEAL));
    }
    if item.unchecked() {
        facts.push((status::UNCHECKED.to_string(), YELLOW));
    }
    facts.extend(permit_chip(item.permit));
    facts.extend(follow_chip(item));
    facts.extend(bypass_chip(item));
    let mut spans = Vec::new();
    for (index, (text, color)) in facts.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", style(MUTED)));
        }
        spans.push(Span::styled(text, style(color)));
    }
    spans
}

pub(super) fn detail_lines(app: &mut App, item: &Item, width: usize, panel: bool) -> (Vec<Line<'static>>, Vec<Link>, Vec<ImageAt>) {
    let detail = app.detail(item).unwrap_or_default();
    let mut lines = Vec::new();
    let mut links = Vec::new();
    let mut images = Vec::new();
    let mut head = vec![Span::styled(item.titled(), bold(MAUVE))];
    let mut where_ = Vec::new();
    if let Some(path) = &item.path {
        where_.push(name_of(path));
    }
    if !detail.branch.is_empty() && detail.branch != item.name {
        where_.push(detail.branch.clone());
    }
    if let Some(git) = &item.git {
        where_.push(format!("{} ahead · {} behind", git.ahead, git.behind));
    }
    if !where_.is_empty() {
        head.push(Span::styled(format!(" · {}", where_.join(" · ")), style(MUTED)));
    }
    lines.push(Line::from(head));
    let state = state_spans(app, item);
    if !state.is_empty() {
        lines.push(Line::from(state));
    }
    if panel {
        lines.push(Line::default());
    }
    if let Some(asked) = asked_question(item) {
        lines.push(field("asks", vec![Span::styled(question_summary(asked, width.saturating_sub(LABEL)), style(PEACH))]));
    } else if let Some((label, text, color)) = report_text(app, item, width) {
        lines.extend(wrapped_field(&label, &text, color, width, if panel { 6 } else { 3 }));
    } else if item.agent.is_none() {
        lines.push(field("claude", vec![Span::styled(format!("not running in its Space · {} restarts it", keys::key(Act::Go)), style(MUTED))]));
    }
    if let Some((line, link)) = server_line(item, detail.dev_server, &detail.server_name, width) {
        lines.push(line);
        links.extend(link);
    }
    lines.extend(worker_line(item, detail.dev_worker, &detail.worker_name));
    let (ci, ci_links) = ci_lines(item, width);
    lines.extend(ci);
    links.extend(ci_links);
    if item.git.as_ref().is_some_and(|git| git.ahead > 0) || item.verdict.is_some() || item.review.is_some() {
        let (text, color) = review_text(item);
        lines.push(field("review", vec![Span::styled(text, style(color))]));
    }
    if let Some(tests) = &detail.tests {
        lines.push(tests_line(tests, width));
    }
    if !detail.changed.is_empty() {
        lines.push(field("changed", vec![Span::styled(changed_text(&detail.changed, width.saturating_sub(LABEL)), style(TEXT))]));
    } else if let Some(git) = &item.git && git.changed > 0 {
        lines.push(field("changed", vec![Span::styled(plural(git.changed, "file"), style(TEXT))]));
    }
    if let Some(note) = note_of(app, item) {
        lines.push(field("note", vec![Span::styled(format!("vault: {note}"), style(TEAL))]));
    }
    if !panel {
        if let Some((card, current)) = &detail.card {
            lines.extend(card_field(card, *current, width));
        }
        return (lines, links, images);
    }
    if !detail.commits.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("commits", style(MUTED))));
        for commit in detail.commits.iter().rev().take(8) {
            let subject = commit.message.lines().next().unwrap_or_default();
            lines.push(Line::from(vec![
                Span::raw("  "), Span::styled(commit.sha.chars().take(7).collect::<String>(), style(MUTED)), Span::raw(" "),
                Span::styled(cut(subject, width.saturating_sub(10)), style(TEXT)),
            ]));
        }
        if detail.commits.len() > 8 {
            lines.push(Line::from(Span::styled(format!("  … {} more", detail.commits.len() - 8), style(MUTED))));
        }
    }
    if let Some((card, current)) = &detail.card {
        lines.push(Line::default());
        let base = item.path.clone().unwrap_or_default();
        for line in card_lines(card, *current, width) {
            let path = card_image(&line).map(|path| base.join(path));
            let area = Size::new(width as u16, 14);
            if let (Some(file), Some(shown)) = (&path, card_image(&line)) {
                links.extend(file_link(&shown, file));
            }
            match path.as_ref().and_then(|path| app.image_size(path, area).map(|size| (path.clone(), size.height))) {
                Some((path, rows)) => {
                    lines.push(line);
                    images.push(ImageAt { line: lines.len(), path, rows, area });
                    lines.extend((0..rows).map(|_| Line::default()));
                }
                None => lines.push(line),
            }
        }
    }
    lines.push(Line::default());
    let mut foot = vec![Span::styled("⏎", style(MAUVE)), Span::styled(" options", style(MUTED))];
    if let Some(opens) = app.s_opens(item) {
        foot.extend([Span::styled(" · ", style(MUTED)), Span::styled(keys::key(Act::Card).to_string(), style(MAUVE)), Span::styled(format!(" {opens}"), style(MUTED))]);
    }
    lines.push(Line::from(foot));
    (lines, links, images)
}

// An image line of the card, as card_lines draws it: the marker, the path, then its alt text.
fn card_image(line: &Line) -> Option<String> {
    let first = line.spans.first()?;
    (first.content == "▣ ").then(|| line.spans.get(1).map(|span| span.content.to_string())).flatten()
}

fn note_of(app: &App, item: &Item) -> Option<String> {
    let address = peers::address(&item.name, &item.place);
    app.ongoing.iter().find(|note| note.task.as_deref() == Some(address.as_str())).map(|note| note.key.clone())
}

pub(super) fn draw_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::new().borders(Borders::LEFT | Borders::RIGHT).border_style(style(SURFACE)).padding(Padding::horizontal(2));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(item) = app.selected_task() else {
        frame.render_widget(Paragraph::new(Span::styled("No task selected.", style(MUTED))), inner);
        return;
    };
    let (lines, links, images) = detail_lines(app, &item, inner.width as usize, true);
    frame.render_widget(Paragraph::new(lines), inner);
    app.links.extend(links);
    let resting = matches!(app.mode, Mode::Normal);
    for image in images.into_iter().filter(|_| resting) {
        let top = inner.y + image.line as u16;
        if image.line as u16 + image.rows > inner.height {
            continue;
        }
        if let Some(Some(protocol)) = app.images.get(&(image.path, image.area.width, image.area.height)) {
            let size = protocol.size();
            render_image(frame, protocol, Rect::new(inner.x, top, size.width.min(inner.width), size.height));
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
    use ratatui::text::Line;
    use ratatui_image::picker::Picker;

    use super::*;
    use crate::board::test_support::*;
    use crate::messages;
    use crate::evidence;
    use crate::report;
    use crate::tasks;
    use crate::board::columns::{group_color, row_facts};
    use crate::board::images::{Graphics, spawn_decoder};
    use crate::board::Mode;
    use crate::board::push::push_lines;
    use crate::board::widgets::{BLUE, GREEN, MUTED, PEACH, RED, YELLOW};

    #[test]
    fn a_narrow_row_shows_the_cards_first_lines_and_counts_its_images() {
        let mut app = carded(true);
        let (rows, _) = render(&mut app, 84, 50);
        let screen = rows.join("\n");
        assert!(screen.contains("card    ✓ Filters show on the search page.") && screen.contains("Price and size, both kept in the URL."), "{screen}");
        assert!(!screen.contains("A third line"), "two lines of text under the row: {screen}");
        assert!(screen.contains("▣ 2 images · s shows the card") && screen.contains("⏎ options · s card"), "{screen}");
        assert!(rows[48].contains("s card"), "the hint line names s while the task has a card: {}", rows[48]);
        let mut bare = test_app(vec![served("search-filters")]);
        with_detail(&mut bare);
        let (rows, _) = render(&mut bare, 84, 50);
        assert!(!rows.join("\n").contains("card "), "no card, no card line");
        bare.handle_normal(key(KeyCode::Char('s')));
        assert!(matches!(&bare.mode, Mode::Message(text) if text.contains("has no sign-off card yet")));
    }

    #[test]
    fn the_task_row_the_panel_and_the_push_dialog_show_the_latest_test_run() {
        let covered = evidence::Shown { mark: evidence::Mark::Covered, text: "cargo test · 493 passed · at a1b2c3d".to_string() };
        let mut app = test_app(vec![served("search-filters")]);
        with_detail(&mut app);
        let (rows, _) = render(&mut app, 84, 50);
        assert!(!rows.join("\n").contains("tests "), "no run recorded, no tests line");
        std::rc::Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().tests = Some(covered.clone());
        for width in [84, 170] {
            let (rows, buffer) = render(&mut app, width, 50);
            let at = rows.iter().position(|row| row.contains("tests   ✓ cargo test · 493 passed · at a1b2c3d")).unwrap_or_else(|| panic!("{}", rows.join("\n")));
            let column = rows[at].chars().position(|char| char == '✓').unwrap() as u16;
            assert_eq!(buffer[(column, at as u16)].fg, GREEN, "a pass on the code as it is is green");
        }
        let commits = vec![tasks::Commit { sha: "1111111aaaa".to_string(), message: "Add refunds".to_string() }];
        let stale = evidence::Shown { mark: evidence::Mark::Stale, text: "cargo test · 493 passed · at a1b2c3d, code changed since".to_string() };
        let (lines, first) = push_lines(None, Some(&stale), &commits, 0, 88);
        assert!(lines[0].to_string().starts_with("TESTS ─"));
        assert_eq!(lines[1].to_string(), "  ○ cargo test · 493 passed · at a1b2c3d, code changed since");
        assert_eq!(lines[1].spans[2].style.fg, Some(YELLOW));
        assert_eq!(first, 4, "the tests section sits above the commits");
        let failed = evidence::Shown { mark: evidence::Mark::Failed, text: "cargo test failed · 2 failed · at a1b2c3d".to_string() };
        let (lines, _) = push_lines(None, Some(&failed), &commits, 0, 88);
        assert_eq!(lines[1].spans[2].style.fg, Some(RED));
    }

    #[test]
    fn a_push_waiting_for_the_tasks_tests_shows_on_its_row_and_in_its_panel_with_who_asked() {
        let mut item = served("search-filters");
        item.agent.as_mut().unwrap().status = "working".to_string();
        let mut app = test_app(vec![item]);
        with_detail(&mut app);
        let (rows, _) = render(&mut app, 170, 40);
        assert!(!rows.join("\n").contains("push waits"), "no intent, nothing waits: {}", rows.join("\n"));
        for (by, word) in [(tasks::PushBy::User, "you"), (tasks::PushBy::Lead, "lead")] {
            app.tasks[0].push_waits = Some(by);
            let shown = format!("push waits for tests · {word}");
            let (rows, buffer) = render(&mut app, 170, 40);
            let at: Vec<usize> = rows.iter().enumerate().filter(|(_, row)| row.contains(&shown)).map(|(at, _)| at).collect();
            assert_eq!(at.len(), 2, "the row and the panel: {}", rows.join("\n"));
            assert!(rows[at[1]].contains(&format!("▌◐ search-filters  {shown} · 2 c")), "the row, in the Work column: {}", rows.join("\n"));
            let panel = rows[at[0]].split('│').nth(1).unwrap();
            assert_eq!(panel.trim(), shown, "the panel's state line: {}", rows.join("\n"));
            let column = rows[at[0]].find(&shown).map(|byte| rows[at[0]][..byte].chars().count()).unwrap() as u16;
            assert_eq!(buffer[(column, at[0] as u16)].fg, MUTED, "muted in the panel");
        }
    }

    #[test]
    fn a_watched_task_shows_its_ci_state_and_links_each_run() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 0);
        assert!(ci_lines(&item, 60).0.is_empty());
        let run = crate::ci::Run { id: 42, name: "CI".to_string(), workflow: "CI".to_string(), status: "in_progress".to_string(), conclusion: String::new(), url: "https://github.com/acme/web/actions/runs/42".to_string() };
        item.ci = Some(crate::ci::Watch {
            address: "billing@web".to_string(), task: "billing".to_string(), repo: PathBuf::new(), slug: "acme/web".to_string(), sha: "abcdef0".to_string(),
            started_at: 0, checked_at: None, close: true, runs: vec![run], state: crate::ci::State::Running, error: None, pushed: crate::ci::Pushed::default(),
        });
        let (lines, links) = ci_lines(&item, 80);
        assert_eq!(lines.iter().map(Line::to_string).collect::<Vec<_>>(), ["ci      ◐ CI running: CI", "        CI  https://github.com/acme/web/actions/runs/42"]);
        assert_eq!((links[0].shown.as_str(), links[0].url.as_str()), ("https://github.com/acme/web/actions/runs/42", "https://github.com/acme/web/actions/runs/42"));
        assert_eq!(row_facts(&item), "CI running: CI");
        assert_eq!(group_color(item.group()), YELLOW);
    }

    #[test]
    fn a_task_started_with_a_profile_other_than_task_shows_it_beside_its_name_on_its_row_and_in_its_panel() {
        let mut app = test_app(vec![served("search-filters")]);
        with_detail(&mut app);
        let (rows, _) = render(&mut app, 170, 50);
        assert!(!rows.join("\n").contains("· task"), "the built-in task is not named: {}", rows.join("\n"));
        app.tasks[0].profile = Some("designer".to_string());
        let (rows, _) = render(&mut app, 170, 50);
        assert!(rows.iter().filter(|row| row.contains("search-filters · designer")).count() >= 2, "on its row and at the head of its panel: {}", rows.join("\n"));
        let mut item = crate::status::tests::task(Some("idle"), None, 0);
        assert_eq!((item.titled(), item.to_json()["profile"].as_str()), ("billing".to_string(), Some("task")));
        item.profile = Some("designer".to_string());
        assert_eq!((item.titled(), item.to_json()["profile"].as_str()), ("billing · designer".to_string(), Some("designer")), "kelpie status --json names it too");
    }

    #[test]
    fn a_task_whose_claude_bypasses_permissions_is_marked_on_its_row_and_named_in_its_panel() {
        let mut app = test_app(vec![served("search-filters")]);
        with_detail(&mut app);
        let (rows, _) = render(&mut app, 170, 50);
        assert!(!rows.join("\n").contains("bypass"), "{}", rows.join("\n"));
        app.tasks[0].bypass = true;
        let (rows, buffer) = render(&mut app, 170, 50);
        assert!(rows.iter().any(|row| row.contains("search-filters") && row.contains("2 commits · bypass")), "{}", rows.join("\n"));
        let at = rows.iter().position(|row| row.contains("bypass permissions: Claude asks for no permission")).unwrap_or_else(|| panic!("{}", rows.join("\n")));
        let column = rows[at].find("bypass permissions").map(|byte| rows[at][..byte].chars().count()).unwrap() as u16;
        assert_eq!(buffer[(column, at as u16)].fg, PEACH);
        let mut fresh = crate::status::tests::task(Some("idle"), None, 0);
        fresh.bypass = true;
        assert_eq!(row_facts(&fresh), "nothing yet · bypass");
    }

    #[test]
    fn the_server_line_links_the_address_only_while_the_dev_server_runs() {
        let mut item = crate::status::tests::task(Some("idle"), None, 0);
        item.url = Some("https://wt1.app.dev".to_string());
        assert_eq!(server_line(&item, true, "server", 60).unwrap().0.to_string(), "server  ○ off · w starts it", "an address from config is not a running server");
        assert!(server_line(&item, false, "server", 60).is_none(), "no dev_server, no line");
        assert_eq!(server_line(&item, true, "web", 60).unwrap().0.to_string(), "web     ○ off · w starts it", "the label is worktree.server_name");
        item.server = true;
        let (line, link) = server_line(&item, true, "server", 60).unwrap();
        assert_eq!(line.to_string(), "server  ● running  https://wt1.app.dev");
        let link = link.unwrap();
        assert_eq!((link.shown.as_str(), link.url.as_str()), ("https://wt1.app.dev", "https://wt1.app.dev"));
        item.url = None;
        assert!(server_line(&item, true, "server", 60).unwrap().1.is_none(), "dev_url off, or no port for {{port}}");
        item.url = Some("https://a-very-long-worktree-name.example.dev/path".to_string());
        let (line, link) = server_line(&item, true, "server", 40).unwrap();
        assert!(line.width() <= 40, "{}", line.width());
        let link = link.unwrap();
        assert!(link.shown.ends_with('…') && link.url.ends_with("/path"));
    }

    #[test]
    fn the_card_shows_the_review_verdict_and_marks_it_stale_once_head_moves() {
        let shown = |ok: bool, findings: usize, stale: bool, running: bool| crate::review::Shown {
            verdict: Some(crate::review::Verdict { ok, sha: "abc".to_string(), findings }), stale, running, file: PathBuf::from("/state/verdicts/billing@web.md"),
        };
        assert_eq!(verdict_chip(&shown(true, 0, false, false)), Some(("review ok".to_string(), GREEN)));
        assert_eq!(verdict_chip(&shown(false, 3, false, false)), Some(("review: 3 findings".to_string(), PEACH)));
        assert_eq!(verdict_chip(&shown(false, 3, true, false)), Some(("review: 3 findings (stale)".to_string(), MUTED)));
        assert_eq!(verdict_chip(&shown(true, 0, false, true)), Some(("review running".to_string(), BLUE)));
        assert_eq!(verdict_chip(&crate::review::Shown { verdict: None, stale: false, running: false, file: PathBuf::new() }), None);
    }

    #[test]
    fn the_question_is_the_end_of_claudes_last_message_in_either_bullet() {
        let screen = "⏺ Read the note.\n\n⏺ Fråga 1: Ska push spärras?\n  A (min rekommendation): visa redo först efter godkännande.\n\n✻ Crunched for 24s\n❯ \n";
        assert_eq!(last_message(screen), "Fråga 1: Ska push spärras? A (min rekommendation): visa redo först efter godkännande.");
        assert_eq!(tail("one two three four five", 10), "…four five");
        assert_eq!(tail("short", 10), "short");
    }

    #[test]
    fn a_question_asked_through_kelpie_shows_on_one_line_with_its_option_count_and_a_prompt_on_screen_over_it() {
        let asked = messages::fold(QUESTION).remove(0);
        let mut item = crate::status::tests::task(Some("idle"), Some("needs-you"), 0);
        assert!(asked_question(&item).is_none(), "a question in plain text keeps the screen fallback");
        item.question = Some(asked.clone());
        assert_eq!(asked_question(&item).map(|message| message.id.as_str()), Some("q1"));
        item.agent.as_mut().unwrap().status = "blocked".to_string();
        assert!(asked_question(&item).is_none());
        assert_eq!(question_summary(&asked, 60), "Which queue for refund mails? · 3 options · s");
        assert_eq!(question_summary(&asked, 30), "Which queue f… · 3 options · s");
    }

    #[test]
    fn the_recap_is_the_last_one_on_screen_with_its_continuation() {
        let screen = "※ recap: An older one.\n\n● Done.\n\n※ recap: We're adding search; the branch is rebased. Next, leave your\n  comments in the split. (disable recaps in /config)\n\n❯ \n";
        assert_eq!(recap(screen), "We're adding search; the branch is rebased. Next, leave your comments in the split.");
        assert_eq!(recap("● Nothing to recap.\n"), "");
    }

    #[test]
    fn a_task_card_shows_its_question_on_one_line_with_how_many_options_it_has() {
        let mut item = crate::status::tests::task(Some("idle"), Some("needs-you"), 0);
        item.question = Some(messages::fold(QUESTION).remove(0));
        let mut app = test_app(vec![item]);
        let (rows, _) = render(&mut app, 170, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("asks    Which queue for refund mails? · 3 options · s"), "{screen}");
        assert!(!screen.contains("reuse the jobs") && !screen.contains("one less service"), "{screen}");
    }

    #[test]
    fn a_card_image_draws_in_the_panel_with_graphics_and_shows_its_path_without() {
        let dir = PathBuf::from(format!("/tmp/kc{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("after.png");
        image::RgbImage::from_pixel(40, 20, image::Rgb([200, 30, 30])).save(&png).unwrap();
        let card = |text: String| report::Card { sha: "abcdef0123".to_string(), text, file: PathBuf::from("/state/cards/x.md") };
        let mut app = test_app(vec![served("search-filters")]);
        with_detail(&mut app);
        let text = format!("Refunds show.\n![after]({})\n![before](/nowhere/before.png)", png.display());
        std::rc::Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card = Some((card(text), true));
        let (rows, _) = render(&mut app, 170, 50);
        let screen = rows.join("\n");
        assert!(screen.contains(&format!("▣ {}  after", png.display())) && screen.contains("▣ /nowhere/before.png  before"), "{screen}");
        app.graphics = Graphics::Ready(Picker::halfblocks());
        (app.image_requests, app.image_results) = spawn_decoder().map(|(requests, results)| (Some(requests), Some(results))).unwrap();
        let (rows, _) = render(&mut app, 170, 50);
        assert!(rows.join("\n").contains(&format!("▣ {}  after", png.display())), "the path shows while the image decodes off the drawing thread");
        let waited = Instant::now();
        while app.decoded.get(&png).is_none_or(Option::is_none) && waited.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
            app.take_images();
        }
        let (rows, buffer) = render(&mut app, 170, 50);
        let (_, y) = find(&rows, "  after")[0];
        let red = Color::Rgb(200, 30, 30);
        assert!((0..170).any(|x| buffer[(x, y as u16 + 1)].bg == red || buffer[(x, y as u16 + 1)].fg == red), "the image under its line: {}", rows.join("\n"));
        assert!(rows.join("\n").contains("▣ /nowhere/before.png  before"), "a missing file stays a path");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
