use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::backlog;
use crate::keys::{self, Act};
use crate::permit::Permit;
use crate::status::{Checkout, Group, Item};
use crate::tasks;
use super::card::card_hint;
use super::detail::unfolded_lines;
use super::{App, Column, Hit};
use super::widgets::{BLUE, FAINT, GREEN, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, TEAL, TEXT, YELLOW, bold, card_text, chips, column_title, group_line, options_hint, plural, row, style, wrap};

pub(super) fn group_color(group: Group) -> Color {
    match group {
        Group::NeedsYou => PEACH,
        Group::Working => BLUE,
        Group::ToReview => TEAL,
        Group::ReadyToPush => GREEN,
        Group::CiRunning => YELLOW,
        Group::Idle => MUTED,
        Group::Queued => MAUVE,
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

// A task can outlive its Space or its Claude; kelpie start brings them back.
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
    parts.extend(item.ci_note());
    parts.extend(item.recover_note());
    parts.extend(item.push_note());
    if git.ahead > 0 {
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
    parts.extend(item.rules_note().map(str::to_string));
    parts.extend(item.review_note());
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

type Rows = Vec<(usize, usize, Hit)>;

const ROUTINE_MARKER: &str = " routine";

fn running_dot(item: &Item) -> bool {
    item.server || item.worker
}

pub(super) fn work_lines(app: &mut App, width: usize, unfold: bool) -> (Vec<Line<'static>>, usize, Rows) {
    let open = app.column == Column::Work;
    let focused = open && !app.in_messages;
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
        if !app.shown(Column::Work, index) {
            continue;
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
        let mut line = row(selected, focused, status_icon(item), &item.titled(), place, &row_facts(item), width.saturating_sub(if dot { 2 } else { 0 } + marker.chars().count()));
        if !marker.is_empty() {
            line.spans.insert(4, Span::styled(marker, style(MUTED)));
        }
        if dot {
            line.spans.extend([Span::raw(" "), Span::styled("●", style(GREEN))]);
        }
        lines.push(line);
        if let Some(wanted) = item.wanted(&keys::key(Act::Go).to_string()) {
            lines.extend(wanted_lines(&wanted, PEACH, selected && open && unfold, width));
        } else if let Some(waiting) = item.waiting() {
            lines.extend(wanted_lines(waiting, MUTED, selected && open && unfold, width));
        }
        if selected && open && unfold {
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

fn ongoing_rows(app: &App, focused: bool, width: usize, lines: &mut Vec<Line<'static>>, rows: &mut Rows) -> Option<usize> {
    if app.ongoing.is_empty() {
        return None;
    }
    let mut selected_line = None;
    let today = backlog::today();
    lines.push(Line::from(vec![Span::styled("ONGOING", bold(TEAL)), Span::styled(format!("  {}", app.ongoing.len()), style(FAINT)), Span::styled("  vault notes in progress", style(MUTED))]));
    for (index, note) in app.ongoing.iter().enumerate() {
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
            (None, None) => (Span::styled("○", style(PEACH)), "no task · ⏎ resume".to_string()),
        };
        rows.push((lines.len(), lines.len() + 1, Hit::Entry(index)));
        lines.push(row(selected, focused, icon, &note.title, None, &facts, width));
    }
    lines.push(Line::default());
    selected_line
}

pub(super) fn backlog_lines(app: &App, width: usize) -> (Vec<Line<'static>>, usize, Rows) {
    let focused = app.column == Column::Backlog && !app.in_messages;
    let right = match app.filter_of(Column::Backlog) {
        Some(filter) => format!("{} / {filter}", plural((0..app.backlog_len()).filter(|index| app.shown(Column::Backlog, *index)).count(), "item")),
        None => format!("{} items", app.backlog.entries.len()),
    };
    let mut lines = column_title("BACKLOG", right, focused, width);
    let mut rows = Vec::new();
    let mut selected_line = ongoing_rows(app, focused, width, &mut lines, &mut rows).unwrap_or(0);
    let offset = app.ongoing.len();
    let mut current: Option<&str> = None;
    for (index, entry) in app.backlog.entries.iter().enumerate() {
        if !app.shown(Column::Backlog, index + offset) {
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
        let selected = index + offset == app.backlog_selected;
        if selected {
            selected_line = lines.len();
        }
        let icon = Span::styled(priority_icon(&entry.priority), style(priority_color(&entry.priority)));
        let start = lines.len();
        if selected && focused {
            lines.extend(wrapped_row(icon, &entry.title, &entry.age, width));
            let mut facts = vec![(entry.area.clone(), TEAL), (if entry.source == "linear" { entry.reference.clone() } else { "vault note".to_string() }, SUBTEXT)];
            if !entry.age.is_empty() {
                facts.push((format!("{} old", entry.age), SUBTEXT));
            }
            lines.push(chips(facts));
            if !entry.summary.is_empty() {
                lines.extend(card_text("about", &entry.summary, FAINT, width, 2));
            }
            lines.extend(card_text("starts as", &suggested_name(&entry.title), FAINT, width, 2));
            lines.push(options_hint());
        } else {
            lines.push(row(selected, focused, icon, &entry.title, None, &entry.age, width));
        }
        rows.push((start, lines.len(), Hit::Entry(index + offset)));
    }
    if app.backlog.entries.is_empty() {
        let empty = backlog::empty_text(&app.sources);
        lines.push(Line::from(Span::styled(empty, style(MUTED))));
    }
    (lines, selected_line, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::test_support::*;
    use crate::keys::Act;
    use crate::status::Checkout;
    use crate::board::input::Purpose;
    use crate::board::{Column, Mode, Ongoing};

    #[test]
    fn a_suggested_name_keeps_letters_with_diacritics() {
        assert_eq!(suggested_name("kelpie – återanvänd worktree tar med förra uppgiftens databas"), "kelpie-ateranvand-worktree-tar");
        assert_eq!(crate::tasks::slug("Föräldraledighet över jul").unwrap(), "foraldraledighet-over-jul");
    }

    #[test]
    fn free_worktrees_are_counted_per_repo() {
        let checkout = |name: &str, free_slots| Checkout { name: name.to_string(), changed: 0, free_slots, busy_slots: 0 };
        assert_eq!(free_worktrees(&[checkout("kelpie", 1), checkout("web", 0), checkout("api", 2)]), " · free worktrees: kelpie 1, api 2");
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
            Ongoing { key: "Projects/web-billing.md".to_string(), title: "web billing".to_string(), area: "web".to_string(), task: None, claim: Some(claim(&day(2), &day(2))) },
            Ongoing { key: "Projects/web-old.md".to_string(), title: "web old".to_string(), area: "web".to_string(), task: None, claim: Some(claim(&day(30), &day(20))) },
        ];
        let (rows, _) = render(&mut app, 200, 50);
        let screen = rows.join("\n");
        assert!(screen.contains("working on devbox · billing") && !screen.contains("no task · ⏎ resume"), "{screen}");
        assert!(screen.contains("working on devbox · billing · stale 20 d"), "a claim quiet for more than 14 days shows as stale: {screen}");
        app.column = Column::Backlog;
        app.backlog_selected = 0;
        assert_eq!(app.choices()[0].label, "Start here too");
        app.act(Act::Go);
        assert!(matches!(&app.mode, Mode::ConfirmClaimed { entry, about, machine, nudged: false } if machine == "devbox" && entry.reference == "Projects/web-billing.md" && about.starts_with("kelpie working on devbox · billing since")), "go asks first");
        let (rows, _) = render(&mut app, 200, 50);
        assert!(rows.join("\n").contains("Start it here and take the note over from devbox?"), "{}", rows.join("\n"));
        press(&mut app, key(KeyCode::Char('n')));
        assert!(matches!(app.mode, Mode::Normal), "n leaves it to the other machine");
        app.act(Act::Go);
        press(&mut app, key(KeyCode::Char('y')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::ClaimedName(entry), buffer, .. } if entry.reference == "Projects/web-billing.md" && buffer == "web-billing"), "y asks for the task's name, and the start passes --anyway");
        app.mode = Mode::Normal;
        app.backlog.entries = vec![crate::backlog::Entry { source: "vault".to_string(), title: "web billing".to_string(), priority: "high".to_string(), area: "web".to_string(), reference: "Projects/web-billing.md".to_string(), summary: String::new(), age: String::new() }];
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
            Ongoing { key: "Projects/search-filters.md".to_string(), title: "search filters".to_string(), area: "web".to_string(), task: Some("search-filters@web".to_string()), claim: None },
            Ongoing { key: "Projects/docs-refresh.md".to_string(), title: "docs refresh".to_string(), area: "shop".to_string(), task: None, claim: None },
        ];
        let (rows, _) = render(&mut app, 84, 50);
        let screen = rows.join("\n");
        let (_, backlog) = find(&rows, "BACKLOG")[0];
        let (_, ongoing) = find(&rows, "ONGOING")[0];
        assert!(backlog < ongoing && screen.contains("task search-filters@web") && screen.contains("no task · ⏎ resume"), "{screen}");
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
}
