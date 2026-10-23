use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::backlog;
use crate::keys::{self, Act};
use crate::permit::Permit;
use crate::status::{Checkout, Group, Item};
use crate::tasks;
use super::card::card_hint;
use super::detail::unfolded_lines;
use crate::config::OngoingAt;
use super::{App, BacklogRow, Column, Hit};
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
    parts.extend(item.rules_note());
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
    lines.extend(super::bug_report::offer_lines(&app.offers, !app.questions.is_empty(), width));
    lines.extend(super::bug_report::fixed_lines(app.fixed.as_ref(), !app.questions.is_empty(), !app.offers.is_empty(), width));
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

fn row_index(order: &[BacklogRow], wanted: BacklogRow) -> usize {
    order.iter().position(|row| *row == wanted).unwrap_or(usize::MAX)
}

pub(super) fn backlog_lines(app: &App, width: usize) -> (Vec<Line<'static>>, usize, Rows) {
    let focused = app.column == Column::Backlog && !app.in_messages;
    let right = match app.filter_of(Column::Backlog) {
        Some(filter) => format!("{} / {filter}", plural((0..app.backlog_len()).filter(|index| app.shown(Column::Backlog, *index)).count(), "item")),
        None => format!("{} items", app.backlog.entries.len()),
    };
    let mut lines = column_title("BACKLOG", right, focused, width);
    let mut rows = Vec::new();
    let order = app.backlog_rows();
    let mut selected_line = 0;
    if app.ongoing_at == OngoingAt::Top && !app.ongoing.is_empty() {
        selected_line = ongoing_rows(app, &order, focused, width, &mut lines, &mut rows).unwrap_or(0);
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
    use crate::board::input::Purpose;
    use crate::board::{Column, Mode, Ongoing};
    use ratatui::crossterm::event::KeyCode;

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

    fn two_notes_and_two_entries(app: &mut crate::board::App) {
        app.ongoing = vec![
            Ongoing { key: "Projects/search-filters.md".to_string(), title: "search filters".to_string(), area: "web".to_string(), task: Some("search-filters@web".to_string()), claim: None, ..Ongoing::default() },
            Ongoing { key: "Projects/docs-refresh.md".to_string(), title: "docs refresh".to_string(), area: "shop".to_string(), task: None, claim: None, ..Ongoing::default() },
        ];
        let entry = |title: &str, reference: &str| crate::backlog::Entry { source: "vault".to_string(), title: title.to_string(), priority: "high".to_string(), area: "web".to_string(), reference: reference.to_string(), summary: String::new(), age: String::new() };
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
