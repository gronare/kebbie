use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::backlog;
use crate::keys::{self, Act};
use crate::permit::Permit;
use crate::status::{Checkout, Group, Item};
use crate::tasks;
use super::card::card_hint;
use super::detail::unfolded_lines;
use super::steps::{accept_wanted, step_lines, to_accept_note};
use crate::config::OngoingAt;
use super::{App, BacklogRow, Column, Hit, item_card};
use super::widgets::{BLUE, FAINT, GREEN, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, TEAL, TEXT, YELLOW, bold, card_text, chips, column_title, cut, group_line, options_hint, plural, row, style, wrap};

pub(super) fn group_color(group: Group) -> Color {
    match group {
        Group::NeedsYou => PEACH,
        Group::Working => BLUE,
        Group::InReview => TEAL,
        Group::ReadyToPush => GREEN,
        Group::CiRunning => YELLOW,
        Group::Idle => MUTED,
        Group::Queued => MAUVE,
        Group::Parked => MUTED,
    }
}

fn priority_color(priority: &str) -> Color {
    match priority {
        "urgent" => RED,
        "high" => PEACH,
        "medium" => YELLOW,
        _ => MUTED,
    }
}

fn priority_icon(priority: &str) -> &'static str {
    match priority {
        "urgent" | "high" => "▲",
        "medium" => "■",
        _ => "·",
    }
}

fn status_icon(item: &Item) -> Span<'static> {
    if item.queued.is_some() {
        return Span::styled("⧗", style(MAUVE));
    }
    match item.agent.as_ref().map(|agent| agent.status.as_str()) {
        Some("blocked") => Span::styled("●", style(PEACH)),
        Some("done") => Span::styled("✓", style(GREEN)),
        Some("working") => Span::styled("◐", style(BLUE)),
        Some(_) => Span::styled("○", style(MUTED)),
        None => Span::styled("·", style(MUTED)),
    }
}

// A task can outlive its Space or its Claude; kebbie start brings them back.
pub(super) fn absence(item: &Item) -> Option<&'static str> {
    if item.queued.is_some() {
        return None;
    }
    match (&item.workspace, &item.agent) {
        (None, None) => Some("no Space"),
        (Some(_), None) => Some("no Claude"),
        _ => None,
    }
}

pub(super) fn row_facts(item: &Item) -> String {
    if item.queued.is_some() {
        return String::new();
    }
    let Some(git) = &item.git else { return String::new() };
    let mut parts: Vec<String> = absence(item).map(str::to_string).into_iter().collect();
    parts.extend(item.git_error().map(|_| crate::status::GIT_UNREAD.to_string()));
    parts.extend(item.ci_note());
    parts.extend(item.recover_note());
    parts.extend(item.push_note());
    if let Some(to_accept) = to_accept_note(item) {
        parts.push(to_accept);
    } else if git.ahead > 0 {
        parts.push(plural(git.ahead, "commit"));
    }
    if git.uncommitted > 0 {
        parts.push(plural(git.uncommitted, "uncommitted file"));
    }
    if git.changed > git.uncommitted {
        parts.push(plural(git.changed - git.uncommitted, "untracked file"));
    }
    if git.behind > 0 {
        parts.push(format!("{} behind", git.behind));
    }
    parts.extend(item.review_note());
    parts.extend(item.unrecorded.clone());
    if item.permit == Some(Permit::Task) {
        parts.push("lead may push".to_string());
    }
    if parts.len() == usize::from(absence(item).is_some()) {
        parts.push("nothing yet".to_string());
    }
    if item.bypass {
        parts.push("bypass".to_string());
    }
    parts.join(" · ")
}

// A worktree pool is per repo, so free worktrees are counted per repo; start makes a new one when a repo has none.
fn free_worktrees(checkouts: &[Checkout]) -> String {
    let free: Vec<String> = checkouts.iter().filter(|checkout| checkout.free_slots > 0)
        .map(|checkout| format!("{} {}", checkout.name, checkout.free_slots)).collect();
    if free.is_empty() { String::new() } else { format!(" · free worktrees: {}", free.join(", ")) }
}

pub fn suggested_name(title: &str) -> String {
    const FILLER: [&str; 12] = ["a", "an", "the", "and", "of", "for", "to", "on", "in", "with", "s", "before"];
    tasks::ascii_fold(&title.to_lowercase()).split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty() && !FILLER.contains(word)).take(4).collect::<Vec<_>>().join("-")
}

// One word of a row's right field in its own colour, such as a backlog item's questions waiting on the user.
fn tinted(mut line: Line<'static>, tint: Option<(String, Color)>) -> Line<'static> {
    let Some((word, color)) = tint else { return line };
    let Some(last) = line.spans.pop() else { return line };
    match last.content.split_once(word.as_str()) {
        Some((before, after)) => {
            let (before, after) = (before.to_string(), after.to_string());
            line.spans.extend([Span::styled(before, last.style), Span::styled(word, last.style.fg(color)), Span::styled(after, last.style)]);
        }
        None => line.spans.push(last),
    }
    line
}

fn key_hint(pairs: &[(String, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw("   ")];
    for (index, (key, label)) in pairs.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", style(MUTED)));
        }
        spans.push(Span::styled(key.clone(), style(MAUVE)));
        spans.push(Span::styled(format!(" {label}"), style(MUTED)));
    }
    Line::from(spans).style(Style::new().bg(SURFACE))
}

// The selected row with its whole name, wrapped, and the facts on its first line.
fn wrapped_row(icon: Span<'static>, name: &str, facts: &str, width: usize) -> Vec<Line<'static>> {
    let name_width = width.saturating_sub(facts.chars().count() + 5);
    let chunks = wrap(name, name_width, 4);
    let highlight = Style::new().bg(SURFACE);
    chunks.into_iter().enumerate().map(|(index, chunk)| {
        if index == 0 {
            let padding = name_width.saturating_sub(chunk.chars().count());
            Line::from(vec![
                Span::styled("▌", style(MAUVE)), icon.clone(), Span::raw(" "), Span::styled(chunk, bold(TEXT)),
                Span::raw(" ".repeat(padding + 2)), Span::styled(facts.to_string(), style(MUTED)),
            ]).style(highlight)
        } else {
            Line::from(vec![Span::raw("   "), Span::styled(chunk, bold(TEXT))]).style(highlight)
        }
    }).collect()
}

// What the task wants of the user, under its row, whole.
fn wanted_lines(wanted: &str, color: Color, highlighted: bool, width: usize) -> Vec<Line<'static>> {
    wrap(wanted, width.saturating_sub(4), 8).into_iter().map(|text| {
        let line = Line::from(vec![Span::raw("   "), Span::styled(text, style(color))]);
        if highlighted { line.style(Style::new().bg(SURFACE)) } else { line }
    }).collect()
}

pub(super) fn how_lines(item: &Item, highlighted: bool, width: usize) -> Vec<Line<'static>> {
    let recipe = item.recipe.as_ref().map(|recipe| if highlighted { format!("{}, because {}", recipe.shown(), recipe.why) } else { recipe.shown() });
    let parts: Vec<String> = recipe.into_iter().chain(review_part(item)).chain(super::send_review::second_part(item)).collect();
    if parts.is_empty() {
        return Vec::new();
    }
    let text = parts.join(" · ");
    let shown = if highlighted { wrap(&text, width.saturating_sub(4), 4) } else { vec![cut(&text, width.saturating_sub(4))] };
    shown.into_iter().map(|text| {
        let line = Line::from(vec![Span::raw("   "), Span::styled(text, style(MUTED))]);
        if highlighted { line.style(Style::new().bg(SURFACE)) } else { line }
    }).collect()
}

// The rules check under the row, each its own line: breaks with the rules the reviewer named, a check under way muted, a push past it by whom; a check that passed says nothing.
pub(super) fn rules_lines(item: &Item, highlighted: bool, width: usize) -> Vec<Line<'static>> {
    use crate::status::RulesShown;
    let breaks = |count: usize, names: &[String]| if names.is_empty() { crate::status::breaks(count) } else { format!("{}: {}", crate::status::breaks(count), names.join(" · ")) };
    let shown: Vec<(String, Color)> = match item.rules_shown() {
        None | Some(RulesShown::Ok) => Vec::new(),
        Some(RulesShown::Breaks { count, names }) => vec![(breaks(count, &names), YELLOW)],
        Some(RulesShown::Overridden { by, breaks: found }) => found.map(|(count, names)| (breaks(count, &names), MUTED)).into_iter().chain([(crate::status::past_the_check(by), MUTED)]).collect(),
        Some(rules) => rules.note().map(|note| (note, MUTED)).into_iter().collect(),
    };
    shown.into_iter().flat_map(|(text, color)| {
        let texts = if highlighted { wrap(&text, width.saturating_sub(4), 4) } else { vec![cut(&text, width.saturating_sub(4))] };
        texts.into_iter().map(move |text| {
            let line = Line::from(vec![Span::raw("   "), Span::styled(text, style(color))]);
            if highlighted { line.style(Style::new().bg(SURFACE)) } else { line }
        })
    }).collect()
}

// A suggested review names the key that sends it; once sent, the suggestion is the review.
fn review_part(item: &Item) -> Option<String> {
    use crate::assessment::Word;
    let word = item.assessment.as_ref()?;
    Some(match word {
        Word::Suggested { reason } if item.verdict.is_some() => Word::Review { reason: reason.clone() }.text(),
        word if word.asks_for_review() && item.verdict.is_none() => format!("{} · {}", word.text(), keys::key(Act::SendReview)),
        word => word.text(),
    })
}

// The facts span is drawn muted whole; one fact in it takes its own colour.
fn tint(line: &mut Line<'static>, fact: &str, color: Color) {
    let Some(at) = line.spans.iter().rposition(|span| span.content.contains(fact)) else { return };
    let span = line.spans.remove(at);
    let (before, after) = span.content.split_once(fact).unwrap_or_default();
    let parts = [(before.to_string(), span.style), (fact.to_string(), span.style.fg(color)), (after.to_string(), span.style)];
    for (offset, (text, style)) in parts.into_iter().enumerate() {
        line.spans.insert(at + offset, Span::styled(text, style));
    }
}

pub(super) type Rows = Vec<(usize, usize, Hit)>;

const ROUTINE_MARKER: &str = " routine";

fn running_dot(item: &Item) -> bool {
    item.server || item.worker
}

pub(super) fn work_lines(app: &mut App, width: usize, unfold: bool) -> (Vec<Line<'static>>, usize, Rows) {
    let open = app.column == Column::Work;
    let focused = open && !app.in_messages && !app.offer_panel.focused && !app.comment_panel.focused;
    let on_task = open && !app.comment_panel.focused;
    let boundary = app.comment_boundary();
    let put_comments = |app: &App, lines: &mut Vec<Line<'static>>, rows: &mut Rows, selected_line: &mut usize, after: bool| {
        let (comments, comment_rows, comment_selected) = super::comments::comment_lines(app, width, lines.len() + usize::from(after), open && !app.in_messages);
        if comments.is_empty() {
            return;
        }
        if after {
            lines.push(Line::default());
        }
        rows.extend(comment_rows);
        if let Some(line) = comment_selected {
            *selected_line = line;
        }
        lines.extend(comments);
    };
    let right = match app.filter_of(Column::Work) {
        Some(filter) => format!("{} / {filter}", plural((0..app.tasks.len()).filter(|index| app.shown(Column::Work, *index)).count(), "task")),
        None => format!("{}{}", plural(app.tasks.len(), "task"), free_worktrees(&app.checkouts)),
    };
    let mut lines = column_title("WORK", right, focused, width);
    let mut selected_line = 0;
    let mut current: Option<Group> = None;
    let tasks = app.tasks.clone();
    let mut rows = Vec::new();
    for (index, item) in tasks.iter().enumerate() {
        if index == boundary {
            put_comments(app, &mut lines, &mut rows, &mut selected_line, current.is_some());
            if app.comments_shown() && current.is_none() {
                lines.push(Line::default());
            }
        }
        if !app.shown(Column::Work, index) {
            continue;
        }
        if item.parked.is_some() && current != Some(Group::Parked) {
            if current.is_some() {
                lines.push(Line::default());
            }
            current = Some(Group::Parked);
            let parked = app.parked_items();
            if !app.parked_open {
                let selected = index == app.work_selected;
                if selected {
                    selected_line = lines.len();
                }
                rows.push((lines.len(), lines.len() + 1, Hit::Task(index)));
                lines.push(super::park::collapsed_line(&parked, selected, focused));
                continue;
            }
            lines.push(super::park::open_header(&parked));
        }
        if current != Some(item.group()) {
            if current.is_some() {
                lines.push(Line::default());
            }
            let count = tasks.iter().enumerate().filter(|(other_index, other)| other.group() == item.group() && app.shown(Column::Work, *other_index)).count();
            lines.push(group_line(item.group().title(), count, group_color(item.group())));
            current = Some(item.group());
        }
        let selected = index == app.work_selected;
        if selected {
            selected_line = lines.len();
        }
        let place = (app.checkouts.len() > 1).then_some(item.place.as_str());
        let start = lines.len();
        let dot = running_dot(item);
        let marker = if item.routine.is_some() { ROUTINE_MARKER } else { "" };
        let facts = match item.parked {
            Some(parked) => format!("{} · {}", parked.back_now(), row_facts(item)),
            None => row_facts(item),
        };
        let mut line = row(selected, focused, status_icon(item), &item.titled(), place, &facts, width.saturating_sub(if dot { 2 } else { 0 } + marker.chars().count()));
        if let Some(to_accept) = to_accept_note(item) {
            tint(&mut line, &to_accept, YELLOW);
        }
        if !marker.is_empty() {
            line.spans.insert(4, Span::styled(marker, style(MUTED)));
        }
        if dot {
            line.spans.extend([Span::raw(" "), Span::styled("●", style(GREEN))]);
        }
        lines.push(line);
        lines.extend(how_lines(item, selected && on_task && unfold, width));
        lines.extend(rules_lines(item, selected && on_task && unfold, width));
        lines.extend(step_lines(item, selected && on_task && unfold, width));
        if let Some(wanted) = item.wanted(&keys::key(Act::Go).to_string()).or_else(|| item.verdict_wanted()).or_else(|| accept_wanted(item)) {
            lines.extend(wanted_lines(&wanted, PEACH, selected && on_task && unfold, width));
        } else if let Some(waiting) = item.waiting() {
            lines.extend(wanted_lines(waiting, MUTED, selected && on_task && unfold, width));
        }
        if selected && on_task && unfold {
            let (detail, links) = unfolded_lines(app, item, width.saturating_sub(4));
            lines.extend(detail.into_iter().map(|line| {
                let mut spans = vec![Span::raw("    ")];
                spans.extend(line.spans);
                Line::from(spans).style(Style::new().bg(SURFACE))
            }));
            app.links.extend(links);
            lines.push(app.s_gives(item).map_or_else(options_hint, |gives| card_hint(&gives)));
        }
        rows.push((start, lines.len(), Hit::Task(index)));
    }
    if tasks.is_empty() {
        lines.push(Line::from(Span::styled(format!("No tasks. {} starts one, or pick one from the backlog.", keys::key(Act::New)), style(MUTED))));
    }
    if boundary >= tasks.len() {
        put_comments(app, &mut lines, &mut rows, &mut selected_line, true);
    }
    let (offers, offer_rows, offer_selected) = super::bug_report::offer_lines(app, width, lines.len());
    rows.extend(offer_rows);
    selected_line = offer_selected.unwrap_or(selected_line);
    lines.extend(offers);
    let (fixed, fixed_rows, fixed_selected) = super::bug_report::fixed_lines(app, width, lines.len());
    rows.extend(fixed_rows);
    selected_line = fixed_selected.unwrap_or(selected_line);
    lines.extend(fixed);
    let leftovers: Vec<String> = app.checkouts.iter().filter(|checkout| checkout.changed > 0)
        .map(|checkout| format!("{}: {} changed outside tasks", checkout.name, plural(checkout.changed, "file"))).collect();
    if !leftovers.is_empty() {
        lines.push(Line::default());
        for leftover in leftovers {
            lines.push(Line::from(vec![Span::styled("! ", style(YELLOW)), Span::styled(leftover, style(MUTED))]));
        }
    }
    (lines, selected_line, rows)
}

fn ongoing_rows(app: &App, order: &[BacklogRow], focused: bool, width: usize, lines: &mut Vec<Line<'static>>, rows: &mut Rows) -> Option<usize> {
    let mut selected_line = None;
    let today = backlog::today();
    lines.push(Line::from(vec![Span::styled("ONGOING", bold(TEAL)), Span::styled(format!("  {}", app.ongoing.len()), style(FAINT)), Span::styled("  vault notes in progress", style(MUTED))]));
    for (note_index, note) in app.ongoing.iter().enumerate() {
        let index = row_index(order, BacklogRow::Ongoing(note_index));
        if !app.shown(Column::Backlog, index) {
            continue;
        }
        let selected = index == app.backlog_selected;
        if selected {
            selected_line = Some(lines.len());
        }
        let (icon, facts) = match (&note.task, &note.claim) {
            (Some(task), _) => (Span::styled("↳", style(TEAL)), format!("task {task}")),
            (None, Some(claim)) => (Span::styled("◇", style(if claim.stale(today) { MUTED } else { BLUE })), claim.shown(today)),
            (None, None) => (Span::styled("○", style(PEACH)), "no task · ⏎ details".to_string()),
        };
        rows.push((lines.len(), lines.len() + 1, Hit::Entry(index)));
        lines.push(row(selected, focused, icon, &note.title, None, &facts, width));
    }
    selected_line
}

fn found_rows(app: &App, order: &[BacklogRow], focused: bool, width: usize, lines: &mut Vec<Line<'static>>, rows: &mut Rows) -> Option<usize> {
    let mut selected_line = None;
    lines.push(Line::from(vec![Span::styled(crate::found::SECTION.to_uppercase(), bold(PEACH)), Span::styled(format!("  {}", app.found.len()), style(FAINT)), Span::styled("  noticed outside their own work", style(MUTED))]));
    for (found_index, item) in app.found.iter().enumerate() {
        let index = row_index(order, BacklogRow::Found(found_index));
        if !app.shown(Column::Backlog, index) {
            continue;
        }
        let selected = index == app.backlog_selected;
        if selected {
            selected_line = Some(lines.len());
        }
        let icon = Span::styled(if item.count > 1 { "◆" } else { "◇" }, style(if item.count > 1 { YELLOW } else { PEACH }));
        let start = lines.len();
        if selected && focused {
            lines.extend(wrapped_row(icon, &item.line, "", width));
            let mut shown = vec![(item.repo.clone(), TEAL), (item.found_by(), SUBTEXT)];
            if item.state == crate::found::State::Kept {
                shown.push(("kept".to_string(), SUBTEXT));
            }
            lines.push(chips(shown));
            lines.push(options_hint());
        } else {
            let times = if item.count > 1 { format!("{}×", item.count) } else { String::new() };
            lines.push(row(selected, focused, icon, &item.line, None, &times, width));
        }
        rows.push((start, lines.len(), Hit::Entry(index)));
    }
    selected_line
}

fn row_index(order: &[BacklogRow], wanted: BacklogRow) -> usize {
    order.iter().position(|row| *row == wanted).unwrap_or(usize::MAX)
}

pub(super) fn backlog_lines(app: &App, width: usize) -> (Vec<Line<'static>>, usize, Rows) {
    let focused = app.column == Column::Backlog && !app.in_messages;
    let right = match app.filter_of(Column::Backlog) {
        Some(filter) => format!("{} / {filter}", plural((0..app.backlog_len()).filter(|index| app.shown(Column::Backlog, *index)).count(), "item")),
        None => format!("{} items", app.backlog.entries.len() + app.found.len()),
    };
    let mut lines = column_title("BACKLOG", right, focused, width);
    let mut rows = Vec::new();
    let order = app.backlog_rows();
    let mut selected_line = 0;
    if app.ongoing_at == OngoingAt::Top && !app.ongoing.is_empty() {
        selected_line = ongoing_rows(app, &order, focused, width, &mut lines, &mut rows).unwrap_or(0);
        lines.push(Line::default());
    }
    if !app.found.is_empty() {
        if let Some(line) = found_rows(app, &order, focused, width, &mut lines, &mut rows) {
            selected_line = line;
        }
        lines.push(Line::default());
    }
    let mut current: Option<&str> = None;
    for (entry_index, entry) in app.backlog.entries.iter().enumerate() {
        let index = row_index(&order, BacklogRow::Entry(entry_index));
        if !app.shown(Column::Backlog, index) {
            continue;
        }
        if current != Some(entry.priority.as_str()) {
            if current.is_some() {
                lines.push(Line::default());
            }
            let count = app.backlog.entries.iter().filter(|other| other.priority == entry.priority).count();
            lines.push(group_line(&entry.priority.to_uppercase(), count, priority_color(&entry.priority)));
            current = Some(&entry.priority);
        }
        let selected = index == app.backlog_selected;
        if selected {
            selected_line = lines.len();
        }
        let icon = Span::styled(priority_icon(&entry.priority), style(priority_color(&entry.priority)));
        let start = lines.len();
        if selected && focused {
            lines.extend(wrapped_row(icon, &entry.title, &entry.age, width));
            let kind = match entry.source.as_str() { "linear" => entry.reference.clone(), "local" => "local item".to_string(), "kept" => "kept by kebbie".to_string(), _ => "vault note".to_string() };
            let mut facts = vec![(entry.area.clone(), TEAL), (kind, SUBTEXT)];
            if !entry.age.is_empty() {
                facts.push((format!("{} old", entry.age), SUBTEXT));
            }
            lines.push(chips(facts));
            let about = item_card::about(entry);
            if !about.is_empty() {
                lines.extend(card_text("about", &about, FAINT, width, 2));
            }
            if !entry.shape.kept {
                lines.extend(card_text("starts as", &suggested_name(&entry.title), FAINT, width, 2));
            }
            lines.push(if item_card::has_card(entry) { key_hint(&[(keys::map().label(Act::Options), "card"), (keys::map().label(Act::Go), "start")]) } else { options_hint() });
        } else {
            let (facts, tint) = item_card::row_facts(entry);
            lines.push(tinted(row(selected, focused, icon, &entry.title, None, &facts, width), tint));
            if let Some(working) = item_card::lead_line(entry) {
                lines.extend(wanted_lines(working, MUTED, false, width));
            }
        }
        rows.push((start, lines.len(), Hit::Entry(index)));
    }
    if app.backlog.entries.is_empty() {
        let empty = backlog::empty_text(&app.sources);
        lines.push(Line::from(Span::styled(empty, style(MUTED))));
    }
    if app.ongoing_at == OngoingAt::Bottom && !app.ongoing.is_empty() {
        lines.push(Line::default());
        if let Some(line) = ongoing_rows(app, &order, focused, width, &mut lines, &mut rows) {
            selected_line = line;
        }
    }
    (lines, selected_line, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::test_support::*;
    use crate::keys::Act;
    use crate::status::Checkout;

    #[test]
    fn found_by_tasks_heads_the_backlog_with_how_often_and_by_whom_and_its_options() {
        let mut app = test_app(Vec::new());
        let item = |id: &str, line: &str, sources: &[&str]| crate::found::Item { id: id.to_string(), line: line.to_string(), repo: "app".to_string(), sources: sources.iter().map(|source| source.to_string()).collect(), count: sources.len() as u32, first_at: 0, last_at: 0, state: crate::found::State::Open, outcome: None, raised: false };
        app.found = vec![item("f1", "The VAT rounding test is flaky on main", &["billing@app", "search@app"]), item("f2", "The README's install step names an old flag", &["billing@app"])];
        app.column = Column::Backlog;
        app.backlog_selected = 0;
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        println!("{screen}");
        assert!(screen.contains("FOUND BY TASKS  2  noticed outside their own work"), "{screen}");
        assert!(screen.contains("BACKLOG") && screen.contains("2 items"), "{screen}");
        assert!(screen.contains("◆ The VAT rounding test is flaky on main") && screen.contains("app") && screen.contains("found 2× · by billing, search"), "the selected item shows whole, with its repo and who found it: {screen}");
        assert!(screen.contains("◇ The README's install step names"), "{screen}");
        let labels: Vec<String> = app.choices().into_iter().map(|choice| choice.label).collect();
        assert_eq!(labels, ["Start a task", "Add to a running task", "Keep in backlog", "Dismiss"], "no task of the item's is running here");
        app.act(Act::Go);
        assert!(matches!(&app.mode, crate::board::Mode::Input { purpose: crate::board::input::Purpose::FoundName(id), buffer, .. } if id == "f1" && buffer == "the-vat-rounding"), "start asks for a name, suggested from the line");
    }
    use crate::board::input::Purpose;
    use crate::board::{Column, Mode, Ongoing};
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn a_suggested_name_keeps_letters_with_diacritics() {
        assert_eq!(suggested_name("kebbie – återanvänd worktree tar med förra uppgiftens databas"), "kebbie-ateranvand-worktree-tar");
        assert_eq!(crate::tasks::slug("Föräldraledighet över jul").unwrap(), "foraldraledighet-over-jul");
    }

    #[test]
    fn the_assessments_word_shows_muted_under_the_task_row_and_a_task_without_one_shows_none() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
        item.assessment = Some(crate::assessment::Word::Review { reason: "changes how VAT is rounded".to_string() });
        let mut app = test_app(vec![item.clone()]);
        app.work_selected = 9;
        let (rows, buffer) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        let at = find(&rows, "billing")[0].1;
        assert_eq!(rows[at + 1].trim(), "review: changes how VAT is rounded", "{screen}");
        let (x, y) = find(&rows, "review: changes")[0];
        assert_eq!(buffer[(x as u16, y as u16)].fg, MUTED);
        let (narrow, _) = render(&mut app, 40, 30);
        let line = narrow.iter().find(|row| row.contains("review: ")).unwrap();
        assert!(line.trim_end().ends_with('…') && line.trim_end().chars().count() <= 40, "a long line is cut to the column: {}", narrow.join("\n"));
        item.assessment = Some(crate::assessment::Word::NotAssessed);
        let mut app = test_app(vec![item.clone()]);
        app.work_selected = 9;
        let (rows, _) = render(&mut app, 120, 30);
        assert_eq!(rows[at + 1].trim(), "review not assessed · v", "no answer is a suggestion too: {}", rows.join("\n"));
        item.assessment = Some(crate::assessment::Word::Suggested { reason: "changes how VAT is rounded".to_string() });
        let mut app = test_app(vec![item.clone()]);
        app.work_selected = 9;
        let (rows, _) = render(&mut app, 120, 30);
        assert_eq!(rows[at + 1].trim(), "review suggested: changes how VAT is rounded · v", "a suggestion names the key that sends the task to review: {}", rows.join("\n"));
        item.verdict = Some(crate::status::tests::in_review(false, 0, false, true));
        let mut app = test_app(vec![item.clone()]);
        app.work_selected = 9;
        let (rows, _) = render(&mut app, 120, 30);
        assert_eq!(rows[at + 1].trim(), "review: changes how VAT is rounded", "once sent, it is the review: {}", rows.join("\n"));
        item.assessment = None;
        item.verdict = None;
        let mut app = test_app(vec![item]);
        let (rows, _) = render(&mut app, 120, 30);
        assert!(!rows.join("\n").contains("review: "), "{}", rows.join("\n"));
    }

    #[test]
    fn the_rules_check_shows_on_its_own_line_under_the_row_and_a_pass_shows_nothing() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
        let shown = |item: &Item| {
            let mut app = test_app(vec![item.clone()]);
            app.work_selected = 9;
            let (rows, buffer) = render(&mut app, 120, 30);
            let at = find(&rows, "billing")[0].1;
            let below = rows[at + 1].trim().to_string();
            let color = find(&rows, &below).first().map(|(x, y)| buffer[(*x as u16, *y as u16)].fg);
            (rows.join("\n"), below, color)
        };
        let (screen, _, _) = shown(&item);
        assert!(!screen.contains("rules") && !screen.contains("approved"), "the reviewer's pass says nothing: {screen}");
        item.git.as_mut().unwrap().approved = false;
        item.rules = Some(crate::rules_check::Shown::Checking);
        let (screen, below, color) = shown(&item);
        assert_eq!((below.as_str(), color), ("checking project rules", Some(MUTED)), "{screen}");
        assert!(!row_facts(&item).contains("rules"), "the row's facts leave it to its line: {}", row_facts(&item));
        item.rules = Some(crate::rules_check::Shown::Broken(2, vec!["Body says why".to_string(), "One change".to_string()]));
        let (screen, below, color) = shown(&item);
        assert_eq!((below.as_str(), color), ("breaks 2 project rules: Body says why · One change", Some(YELLOW)), "{screen}");
        item.rules = Some(crate::rules_check::Shown::Broken(1, Vec::new()));
        assert_eq!(shown(&item).1, "breaks 1 project rule", "the reviewer named no rule");
        item.git.as_mut().unwrap().approved = true;
        item.git.as_mut().unwrap().approved_by = Some(crate::tasks::ApprovedBy::Lead);
        let (screen, below, _) = shown(&item);
        assert_eq!(below, "breaks 1 project rule", "{screen}");
        assert!(screen.contains("pushed past the rules check by the lead") && !screen.contains("approved"), "{screen}");
        item.rules = None;
        item.git.as_mut().unwrap().approved_by = Some(crate::tasks::ApprovedBy::User);
        let (screen, below, color) = shown(&item);
        assert_eq!((below.as_str(), color), ("pushed past the rules check by you", Some(MUTED)), "{screen}");
    }

    #[test]
    fn a_tasks_recipe_leads_the_line_under_its_row_and_the_open_task_says_why() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
        item.recipe = Some(crate::recipe::Recipe { name: "small-fix".to_string(), why: "one function and its test".to_string(), by: "the lead".to_string() });
        item.assessment = Some(crate::assessment::Word::Review { reason: "changes VAT rounding".to_string() });
        let mut app = test_app(vec![item.clone()]);
        let (open, _) = render(&mut app, 120, 30);
        let at = find(&open, "billing")[0].1;
        assert_eq!((open[at + 1].trim(), open[at + 2].trim()), ("recipe: small fix, because one function and its test ·", "review: changes VAT rounding"), "the open task shows the why whole: {}", open.join("\n"));
        app.work_selected = 9;
        let (rows, _) = render(&mut app, 120, 30);
        assert_eq!(rows[at + 1].trim(), "recipe: small fix · review: changes VAT rounding", "{}", rows.join("\n"));
        item.assessment = None;
        let mut app = test_app(vec![item.clone()]);
        app.work_selected = 9;
        let (rows, _) = render(&mut app, 120, 30);
        assert_eq!(rows[find(&rows, "billing")[0].1 + 1].trim(), "recipe: small fix", "{}", rows.join("\n"));
        item.recipe = None;
        let mut app = test_app(vec![item]);
        let (rows, _) = render(&mut app, 120, 30);
        assert!(!rows.join("\n").contains("recipe:"), "a task started by hand without a recipe shows none: {}", rows.join("\n"));
    }

    #[test]
    fn free_worktrees_are_counted_per_repo() {
        let checkout = |name: &str, free_slots| Checkout { name: name.to_string(), changed: 0, free_slots, busy_slots: 0 };
        assert_eq!(free_worktrees(&[checkout("kebbie", 1), checkout("web", 0), checkout("api", 2)]), " · free worktrees: kebbie 1, api 2");
        assert_eq!(free_worktrees(&[checkout("web", 0)]), "");
    }

    #[test]
    fn a_note_another_machine_works_on_says_which_and_asks_before_it_starts_here() {
        use ratatui::crossterm::event::KeyCode;
        let today = crate::backlog::today();
        let day = |back: i64| { let (year, month, date) = crate::when::civil(today - back); format!("{year:04}-{month:02}-{date:02}") };
        let claim = |since: &str, updated: &str| crate::vault::Claim { machine: "devbox".to_string(), task: "billing".to_string(), repo: "web".to_string(), since: format!("{since}T08:00:00Z"), updated: updated.to_string() };
        let mut app = test_app(vec![served("search-filters")]);
        app.ongoing = vec![
            Ongoing { key: "Projects/web-billing.md".to_string(), title: "web billing".to_string(), area: "web".to_string(), task: None, claim: Some(claim(&day(2), &day(2))), ..Ongoing::default() },
            Ongoing { key: "Projects/web-old.md".to_string(), title: "web old".to_string(), area: "web".to_string(), task: None, claim: Some(claim(&day(30), &day(20))), ..Ongoing::default() },
        ];
        let (rows, _) = render(&mut app, 200, 50);
        let screen = rows.join("\n");
        assert!(screen.contains("working on devbox · billing") && !screen.contains("no task · ⏎ details"), "{screen}");
        assert!(screen.contains("working on devbox · billing · stale 20 d"), "a claim quiet for more than 14 days shows as stale: {screen}");
        app.column = Column::Backlog;
        app.backlog_selected = 0;
        assert_eq!(app.choices()[0].label, "Start here too");
        app.act(Act::Go);
        assert!(matches!(&app.mode, Mode::ConfirmClaimed { entry, about, machine, nudged: false } if machine == "devbox" && entry.reference == "Projects/web-billing.md" && about.starts_with("kebbie working on devbox · billing since")), "go asks first");
        let (rows, _) = render(&mut app, 200, 50);
        assert!(rows.join("\n").contains("Start it here and take the note over from devbox?"), "{}", rows.join("\n"));
        press(&mut app, key(KeyCode::Char('n')));
        assert!(matches!(app.mode, Mode::Normal), "n leaves it to the other machine");
        app.act(Act::Go);
        press(&mut app, key(KeyCode::Char('y')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::ClaimedName(entry), buffer, .. } if entry.reference == "Projects/web-billing.md" && buffer == "web-billing"), "y asks for the task's name, and the start passes --anyway");
        app.mode = Mode::Normal;
        app.backlog.entries = vec![crate::backlog::Entry { source: "vault".to_string(), title: "web billing".to_string(), priority: "high".to_string(), area: "web".to_string(), reference: "Projects/web-billing.md".to_string(), summary: String::new(), age: String::new(), shape: Default::default() }];
        app.backlog_selected = 2;
        app.act(Act::Go);
        assert!(matches!(app.mode, Mode::ConfirmClaimed { .. }), "the same note in the Backlog column asks too");
        let source = include_str!("input.rs");
        assert!(source.contains("Purpose::ClaimedName(entry) => (entry, text, None, true)") && source.contains("command.arg(\"--anyway\")"));
    }

    #[test]
    fn ongoing_notes_head_the_backlog_with_their_task_or_a_resume() {
        let mut app = test_app(vec![served("search-filters")]);
        app.ongoing = vec![
            Ongoing { key: "Projects/search-filters.md".to_string(), title: "search filters".to_string(), area: "web".to_string(), task: Some("search-filters@web".to_string()), claim: None, ..Ongoing::default() },
            Ongoing { key: "Projects/docs-refresh.md".to_string(), title: "docs refresh".to_string(), area: "shop".to_string(), task: None, claim: None, ..Ongoing::default() },
        ];
        let (rows, _) = render(&mut app, 84, 50);
        let screen = rows.join("\n");
        let (_, backlog) = find(&rows, "BACKLOG")[0];
        let (_, ongoing) = find(&rows, "ONGOING")[0];
        assert!(backlog < ongoing && screen.contains("task search-filters@web") && screen.contains("no task · ⏎ details"), "{screen}");
        assert!(screen.contains("note    vault: Projects/search-filters.md"), "the task shows its note: {screen}");
        app.column = Column::Backlog;
        app.backlog_selected = 1;
        assert_eq!(app.choices()[0].label, "Resume as task");
        app.act(Act::Go);
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogName(entry), .. } if entry.reference == "Projects/docs-refresh.md" && entry.source == "vault"));
        app.mode = Mode::Normal;
        app.backlog_selected = 0;
        app.act(Act::Go);
        assert!(app.column == Column::Work && app.work_selected == 0, "a note with a task goes to it");
    }

    #[test]
    fn each_backlog_row_says_its_state_and_the_selected_one_its_card_or_options() {
        use crate::backlog::Entry;
        use crate::backlog_file::{Shape, State};
        let mut app = test_app(Vec::new());
        let item = |title: &str, reference: &str, source: &str, age: &str, shape: Shape| Entry { source: source.to_string(), title: title.to_string(), priority: "high".to_string(), area: "web".to_string(), reference: reference.to_string(), summary: "You stay signed in when you reload a page.".to_string(), age: age.to_string(), shape };
        app.backlog.entries = vec![
            item("Rounding per invoice", "Projects/rounding.md", "vault", "3 d", Shape { state: Some(State::Drafting), waiting: 2, ..Shape::default() }),
            item("Retry a failed CI push", "Projects/retry.md", "vault", "0 d", Shape { state: Some(State::Drafting), ..Shape::default() }),
            item("Session drops on reload", "Projects/session.md", "vault", "6 d", Shape { state: Some(State::Ready), ..Shape::default() }),
            item("ci push retry flaky", "kept:abc123", "kept", "0 d", Shape { state: Some(State::ToWrite), kept: true, ..Shape::default() }),
        ];
        app.backlog_selected = 2;
        let (rows, buffer) = render(&mut app, 124, 30);
        let screen = rows.join("\n");
        for wanted in ["Rounding per invoice", "2 questions · 3 d", "drafting · 0 d", "the lead is writing it out from the code, the", "Session drops on reload", "to write · 0 d", "the lead files it in the vault"] {
            assert!(screen.contains(wanted), "{wanted}:\n{screen}");
        }
        let (x, y) = find(&rows, "2 questions · 3 d")[0];
        assert!(coloured(&buffer[(x as u16, y as u16)], (0xf9, 0xe2, 0xaf)) && !coloured(&buffer[(x as u16 + 12, y as u16)], (0xf9, 0xe2, 0xaf)), "the questions alone are yellow");
        app.column = Column::Backlog;
        app.backlog_selected = 0;
        let screen = render(&mut app, 124, 30).0.join("\n");
        assert!(screen.contains("drafting: 2 questions wait on you before it") && screen.contains("⏎ card · g start") && screen.contains("vault note"), "{screen}");
        app.backlog_selected = 2;
        let screen = render(&mut app, 124, 30).0.join("\n");
        assert!(screen.contains("You stay signed in when you reload a page.") && screen.contains("starts as"), "a ready item's about is its goal: {screen}");
        app.backlog_selected = 3;
        let screen = render(&mut app, 124, 30).0.join("\n");
        assert!(screen.contains("kept by kebbie") && screen.contains("to write: the lead writes it out and files it") && screen.contains("⏎ options") && !screen.contains("starts as"), "{screen}");
        press(&mut app, key(KeyCode::Char('g')));
        assert!(matches!(app.mode, Mode::Normal) && app.notice.as_ref().is_some_and(|(text, _)| text.contains("has yet to write it out")), "a kept row has nothing to start yet");
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Options(_)), "a kept row has no card: ⏎ is its options");
    }

    fn two_notes_and_two_entries(app: &mut crate::board::App) {
        app.ongoing = vec![
            Ongoing { key: "Projects/search-filters.md".to_string(), title: "search filters".to_string(), area: "web".to_string(), task: Some("search-filters@web".to_string()), claim: None, ..Ongoing::default() },
            Ongoing { key: "Projects/docs-refresh.md".to_string(), title: "docs refresh".to_string(), area: "shop".to_string(), task: None, claim: None, ..Ongoing::default() },
        ];
        let entry = |title: &str, reference: &str| crate::backlog::Entry { source: "vault".to_string(), title: title.to_string(), priority: "high".to_string(), area: "web".to_string(), reference: reference.to_string(), summary: String::new(), age: String::new(), shape: Default::default() };
        app.backlog.entries = vec![entry("nightly import", "Projects/nightly.md"), entry("price alerts", "Projects/alerts.md")];
    }

    #[test]
    fn work_groups_follow_the_configured_order_and_the_rest_keep_theirs() {
        let mut working = crate::status::tests::task(Some("working"), None, 0);
        working.name = "busy-one".to_string();
        let mut blocked = crate::status::tests::task(Some("blocked"), None, 0);
        blocked.name = "stuck-one".to_string();
        let items = vec![blocked, working, served("ready-one")];
        let groups: Vec<Group> = items.iter().map(Item::group).collect();
        assert!(groups.windows(2).all(|pair| pair[0] < pair[1]), "three groups in today's order: {groups:?}");
        let mut app = test_app(Vec::new());
        app.groups = crate::config::group_order(&[groups[2]]);
        app.set_tasks(items);
        let (rows, _) = render(&mut app, 84, 40);
        let at = |group: Group| find(&rows, group.title())[0].1;
        assert!(at(groups[2]) < at(groups[0]) && at(groups[0]) < at(groups[1]), "the listed group first, the others after it in today's order:\n{}", rows.join("\n"));
        assert_eq!(app.selected_task().map(|item| item.name).as_deref(), Some("ready-one"), "the top row is the first task");
    }

    #[test]
    fn ongoing_at_the_bottom_follows_the_backlog_and_keys_clicks_and_filters_act_on_the_row_shown() {
        let mut app = test_app(vec![served("search-filters")]);
        two_notes_and_two_entries(&mut app);
        app.ongoing_at = crate::config::OngoingAt::Bottom;
        app.column = Column::Backlog;
        let (rows, _) = render(&mut app, 84, 50);
        let (_, ongoing) = find(&rows, "ONGOING")[0];
        let (_, alerts) = find(&rows, "price alerts")[0];
        assert!(find(&rows, "HIGH")[0].1 < alerts && alerts < ongoing && ongoing < find(&rows, "docs refresh")[0].1, "{}", rows.join("\n"));
        app.backlog_selected = 0;
        assert_eq!(app.choices()[0].label, "Start as task");
        app.act(Act::Go);
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogName(entry), .. } if entry.reference == "Projects/nightly.md"), "the top row is the first backlog entry");
        app.mode = Mode::Normal;
        let (rows, _) = render(&mut app, 84, 50);
        let (x, y) = find(&rows, "docs refresh")[0];
        click(&mut app, x as u16, y as u16);
        assert_eq!(app.selected_ongoing().map(|note| note.key).as_deref(), Some("Projects/docs-refresh.md"), "a click selects the note it lands on");
        let (rows, _) = render(&mut app, 84, 50);
        let (x, y) = find(&rows, "docs refresh")[0];
        click(&mut app, x as u16, y as u16);
        assert!(matches!(&app.mode, Mode::NoteCard { key, .. } if key == "Projects/docs-refresh.md"), "a second click opens its card");
        app.mode = Mode::Normal;
        app.set_filter("docs".to_string());
        app.act(Act::Go);
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogName(entry), label, .. } if entry.reference == "Projects/docs-refresh.md" && label.starts_with("Resume")), "filtering selects the note shown");
        app.mode = Mode::Normal;
        app.set_filter("alerts".to_string());
        let (rows, _) = render(&mut app, 84, 50);
        assert!(!rows.join("\n").contains("docs refresh"), "{}", rows.join("\n"));
        app.act(Act::Go);
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogName(entry), .. } if entry.reference == "Projects/alerts.md"));
        app.mode = Mode::Normal;
        app.set_filter(String::new());
        app.backlog_selected = 2;
        app.act(Act::Go);
        assert!(app.column == Column::Work && app.work_selected == 0, "the first note, below the entries, goes to its task");
    }

    #[test]
    fn hidden_ongoing_leaves_only_the_backlog() {
        let mut app = test_app(vec![served("search-filters")]);
        two_notes_and_two_entries(&mut app);
        app.ongoing_at = crate::config::OngoingAt::Hidden;
        app.column = Column::Backlog;
        let (rows, _) = render(&mut app, 84, 50);
        let screen = rows.join("\n");
        assert!(!screen.contains("ONGOING") && !screen.contains("docs refresh") && screen.contains("price alerts"), "{screen}");
        app.move_selection(1);
        app.move_selection(1);
        assert_eq!(app.backlog_selected, 1, "the selection stops at the last entry");
        assert_eq!(app.selected_entry().map(|entry| entry.reference).as_deref(), Some("Projects/alerts.md"));
    }

    #[test]
    fn enter_on_an_ongoing_note_opens_a_card_with_what_the_note_is_about() {
        let body = "# Search filters that remember\n\n## Problem\nFilters reset on every visit, see https://example.com/issues/12 for the report.\n\n## Lösning\nKeep price and size in the URL.\n\n## Beslut\nNot for the card.\n";
        let mut app = test_app(vec![served("search-filters")]);
        let title = "search filters that remember price and size across visits and devices";
        let claim = crate::vault::Claim { machine: "devbox".to_string(), task: "billing".to_string(), repo: "web".to_string(), since: "2026-10-19T08:00:00Z".to_string(), updated: "2026-10-20".to_string() };
        app.ongoing = vec![Ongoing {
            key: "Projects/search-filters.md".to_string(), title: title.to_string(), area: "web".to_string(), task: Some("search-filters@web".to_string()), claim: Some(claim),
            status: "active".to_string(), summary: "Filters survive a reload.".to_string(), opening: super::super::note_card::opening(body),
        }];
        app.column = Column::Backlog;
        let (rows, _) = render(&mut app, 84, 50);
        assert!(!rows.join("\n").contains(title), "the row cuts the title");
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::NoteCard { key, .. } if key == "Projects/search-filters.md"));
        let (rows, buffer) = render(&mut app, 120, 50);
        let screen: String = rows.iter().map(|row| row.trim().trim_matches('│').trim()).collect::<Vec<_>>().join("\n");
        for wanted in [title, "area      web", "status    active", "summary   Filters survive a reload.", "works on  devbox · billing · since 2026-10-19", "task      search-filters@web · runs here", "note      vault: Projects/search-filters.md", "Problem", "Lösning", "Keep price and size in the URL.", "g go to its task"] {
            assert!(screen.contains(wanted), "{wanted:?} in:\n{screen}");
        }
        assert!(!screen.contains("Not for the card") && !screen.contains("Search filters that remember\n"), "the card shows the opening sections, not the heading or the rest:\n{screen}");
        assert_whole_link(&buffer, "https://example.com/issues/12", 1, "a URL in the note");
        press(&mut app, key(KeyCode::Char('g')));
        assert!(app.column == Column::Work && app.work_selected == 0 && matches!(app.mode, Mode::Normal), "g in the card does what the note offers");
    }

    #[test]
    fn a_long_note_scrolls_in_its_card() {
        let mut app = test_app(Vec::new());
        let body: String = (1..=60).map(|line| format!("line {line} of the problem\n")).collect();
        app.ongoing = vec![Ongoing { key: "Projects/long.md".to_string(), title: "long".to_string(), area: "web".to_string(), opening: super::super::note_card::opening(&format!("## Problem\n{body}")), ..Ongoing::default() }];
        app.column = Column::Backlog;
        press(&mut app, key(KeyCode::Enter));
        let (rows, _) = render(&mut app, 100, 30);
        assert!(!rows.join("\n").contains("line 60 ") && rows.join("\n").contains("scroll"), "{}", rows.join("\n"));
        for _ in 0..100 {
            press(&mut app, key(KeyCode::Down));
            render(&mut app, 100, 30);
        }
        let (rows, _) = render(&mut app, 100, 30);
        assert!(rows.join("\n").contains("line 60 of the problem"), "{}", rows.join("\n"));
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }
}
