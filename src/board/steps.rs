use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::keys::{self, Act};
use crate::status::Item;
use crate::tasks::{self, Subtask, SubtaskState};
use super::widgets::{BLUE, GREEN, MAUVE, MUTED, RAISED, RED, SURFACE, TEXT, YELLOW, bold, cut, plural, style, text_width};

const INDENT: &str = "    ";

// A task of one subtask is that subtask, so its row would only repeat the task's.
pub(super) fn rows_shown(item: &Item) -> bool {
    item.subtasks.pushed.len() + item.subtasks.items.len() > 1
}

// The subtask rows the cursor walks through: the open ones, as listed.
pub(super) fn walked(item: &Item) -> &[Subtask] {
    if rows_shown(item) { &item.subtasks.items } else { &[] }
}

pub(super) fn subtask_hint(number: usize) -> Line<'static> {
    let key = |key: String| Span::styled(key, style(MAUVE));
    let label = |text: &str| Span::styled(text.to_string(), style(MUTED));
    Line::from(vec![
        key(keys::map().label(Act::Options)), label(&format!(" opens subtask {number} · ")), key(keys::key(Act::Permit).to_string()), label(" accepts it · "),
        key(keys::key(Act::Review).to_string()), label(" reads its diff · "), key(keys::map().label(Act::Back)), label(" back to the task"),
    ])
}

fn waiting(subtask: &Subtask) -> bool {
    subtask.state == SubtaskState::Ready
}

pub(super) fn to_accept(item: &Item) -> usize {
    item.subtasks.items.iter().filter(|subtask| waiting(subtask)).count()
}

// The commits of the pushable run at the top of the push order, which a push may take while the task keeps working.
pub(super) fn accepted_run(item: &Item) -> usize {
    item.subtasks.pushable.as_ref().map_or(0, |run| run.count)
}

pub(super) fn to_accept_note(item: &Item) -> Option<String> {
    let count = item.to_accept.as_ref().map_or_else(|| to_accept(item), |waits| waits.count);
    (count > 0).then(|| format!("{count} to accept"))
}

pub(super) fn accept_wanted(item: &Item) -> Option<String> {
    let subtask = item.subtasks.items.iter().find(|subtask| waiting(subtask))?;
    Some(format!("accept {} · {} review", subtask.number, keys::key(Act::Review)))
}

pub(super) fn mark_color(state: &SubtaskState) -> Color {
    match state {
        SubtaskState::Planned => MUTED,
        SubtaskState::InProgress => BLUE,
        SubtaskState::Ready => YELLOW,
        SubtaskState::Accepted | SubtaskState::Pushed => GREEN,
    }
}

const TITLE_FLOOR: usize = 12;

// The rules part of a ready subtask's label, longest first; the lines under the task's row say it in full, and a check that passed or never ran says nothing here, the task's line says not checked once.
fn rules_forms(item: &Item, state: &SubtaskState) -> Vec<String> {
    use crate::status::RulesShown;
    if *state != SubtaskState::Ready {
        return Vec::new();
    }
    match item.rules_shown() {
        None | Some(RulesShown::Ok | RulesShown::Unchecked) => Vec::new(),
        Some(RulesShown::Checking { .. }) => vec!["checking project rules".to_string(), "checking".to_string()],
        Some(RulesShown::Breaks { count, .. }) => vec![crate::status::breaks(count), plural(count, "break")],
        Some(RulesShown::Overridden { by, .. }) => vec![crate::status::past_the_check(by), "past the rules".to_string()],
    }
}

// The state word, and for an accepted subtask behind one that is not, which number it waits for.
fn state_word(item: &Item, subtask: &Subtask) -> String {
    let waits = subtask.waits_for.as_ref().and_then(|title| item.subtasks.items.iter().find(|other| other.title == *title));
    match (&subtask.state, waits) {
        (SubtaskState::Planned, _) => String::new(),
        (SubtaskState::Accepted, Some(other)) => format!("accepted · waits for {}", other.number),
        (state, _) => tasks::subtask_word(state).to_string(),
    }
}

// What follows the state word, longest first: how many commits the subtask holds, then the rules part; each gives way before the state word does.
fn label_forms(item: &Item, step: &Subtask) -> Vec<String> {
    let count = match step.commits.len() {
        0 => None,
        1 => Some("1 commit".to_string()),
        many => Some(format!("{many} commits")),
    };
    let rules = rules_forms(item, &step.state);
    rules.into_iter().map(|rules| count.as_ref().map_or(rules.clone(), |count| format!("{count} · {rules}"))).chain(count.clone()).collect()
}

// The title whole, or as many whole words as fit with an ellipsis; None when not even one word does.
pub(super) fn whole_words(text: &str, room: usize) -> Option<String> {
    if text_width(text) <= room {
        return Some(text.to_string());
    }
    let mut kept = String::new();
    for word in text.split_whitespace() {
        let next = if kept.is_empty() { word.to_string() } else { format!("{kept} {word}") };
        if text_width(&next) + 1 > room {
            break;
        }
        kept = next;
    }
    (!kept.is_empty()).then(|| format!("{kept}…"))
}

fn shortened(title: &str, room: usize) -> String {
    whole_words(title, room).unwrap_or_else(|| cut(title, room.max(1)))
}

// The state word always stands; the rules part shortens, then goes, before the title is cut below TITLE_FLOOR.
fn fitted(title: &str, state: &str, rules: &[String], room: usize) -> (String, String) {
    if state.is_empty() || room < text_width(state) + 3 {
        return (shortened(title, room), String::new());
    }
    let labels: Vec<String> = rules.iter().map(|rules| format!("{state} · {rules}")).chain([state.to_string()]).collect();
    let title_room = |label: &String| room.saturating_sub(text_width(label) + 2);
    let floor = text_width(title).min(TITLE_FLOOR);
    let label = labels.iter().find(|label| title_room(label) >= floor).unwrap_or(&labels[labels.len() - 1]);
    (shortened(title, title_room(label)), label.clone())
}

fn step_line(item: &Item, step: &Subtask, width: usize, on: bool) -> Line<'static> {
    let number = format!("{} ", step.number);
    let room = width.saturating_sub(INDENT.len() + 2 + number.len());
    let (title, label) = fitted(&step.title, &state_word(item, step), &label_forms(item, step), room);
    let color = mark_color(&step.state);
    let planned = step.state == SubtaskState::Planned;
    let gap = room.saturating_sub(text_width(&title) + text_width(&label));
    Line::from(vec![
        Span::raw(INDENT), Span::styled(tasks::subtask_mark(&step.state), style(color)), Span::raw(" "),
        Span::styled(number, style(MUTED)), Span::styled(title, if on { bold(TEXT) } else { style(if planned { MUTED } else { TEXT }) }),
        Span::raw(" ".repeat(gap)), Span::styled(label, style(color)),
    ])
}

// The task's stages under its subtasks, each with where it stands; the pushable run stops at the first not met.
fn stage_lines(stages: &[kebbie_hub_protocol::Stage], width: usize) -> Vec<Line<'static>> {
    stages.iter().enumerate().map(|(at, stage)| {
        let color = match stage.state {
            kebbie_hub_protocol::StageState::Met => GREEN,
            kebbie_hub_protocol::StageState::Failed => RED,
            kebbie_hub_protocol::StageState::Pushed | kebbie_hub_protocol::StageState::Waiting => YELLOW,
            kebbie_hub_protocol::StageState::Open | kebbie_hub_protocol::StageState::Locked => MUTED,
        };
        let word = tasks::rollout::stage_word(stage, stages);
        let head = format!("stage {} ", at + 1);
        let room = width.saturating_sub(INDENT.len() + 2 + head.len());
        let (title, label) = fitted(&stage.title, &word, &[], room);
        let gap = room.saturating_sub(text_width(&title) + text_width(&label));
        Line::from(vec![
            Span::raw(INDENT), Span::styled(tasks::rollout::mark(stage.state), style(color)), Span::raw(" "), Span::styled(head, style(MUTED)),
            Span::styled(title, style(TEXT)), Span::raw(" ".repeat(gap)), Span::styled(label, style(color)),
        ])
    }).collect()
}

// The line the subtask under the cursor sits on, counted from the first of step_lines.
pub(super) fn cursor_line(item: &Item, at: usize) -> usize {
    usize::from(!item.subtasks.pushed.is_empty()) + at
}

pub(super) fn step_lines(item: &Item, highlighted: bool, width: usize, cursor: Option<usize>) -> Vec<Line<'static>> {
    if !item.subtasks.listed() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    if rows_shown(item) {
        if !item.subtasks.pushed.is_empty() {
            lines.push(Line::from(vec![Span::raw(INDENT), Span::styled(format!("↑ {} pushed", item.subtasks.pushed.len()), style(MUTED))]));
        }
        lines.extend(item.subtasks.items.iter().enumerate().map(|(at, step)| step_line(item, step, width, cursor == Some(at))));
    }
    lines.extend(stage_lines(&item.subtasks.stages, width));
    let on = cursor.filter(|_| rows_shown(item)).map(|at| cursor_line(item, at));
    lines.into_iter().enumerate().map(|(at, line)| match (on == Some(at), highlighted) {
        (true, _) => line.style(Style::new().bg(RAISED)),
        (false, true) => line.style(Style::new().bg(SURFACE)),
        (false, false) => line,
    }).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::board::test_support::*;
    use crate::phase::Phase;
    use crate::tasks::Subtasks;

    pub(crate) fn step(index: usize, title: &str, state: SubtaskState) -> Subtask {
        Subtask { id: format!("s{}", index + 1), number: index + 1, title: title.to_string(), state, commits: Vec::new(), pushed: Vec::new(), extra: false, added: None, contiguous: true, waits_for: None }
    }

    // The subtask done and holding the commits shas, all accepted or none.
    pub(crate) fn holds(step: &mut Subtask, shas: &[&str], accepted: bool) {
        step.state = if accepted { SubtaskState::Accepted } else { SubtaskState::Ready };
        step.commits = shas.iter().map(|sha| tasks::SubtaskCommit { sha: sha.to_string(), subject: format!("subject of {sha}"), accepted }).collect();
    }

    pub(crate) fn held(index: usize, title: &str, shas: &[&str], accepted: bool) -> Subtask {
        let mut held = step(index, title, SubtaskState::Planned);
        holds(&mut held, shas, accepted);
        held
    }

    pub(crate) fn pushed_one() -> Vec<Subtask> {
        vec![Subtask { pushed: vec!["1111111aaaa".to_string()], ..step(0, "Say what a gateway is", SubtaskState::Pushed) }]
    }

    pub(crate) use crate::tasks::ordered;

    pub(crate) fn stripe() -> Subtasks {
        ordered(Subtasks { pushed: pushed_one(), items: vec![
            held(1, "Charge through a PaymentGateway", &["2222222bbbb"], false),
            step(2, "Add Stripe as the gateway", SubtaskState::InProgress),
            step(3, "Remove the old payment client", SubtaskState::Planned),
        ], ..Subtasks::default() })
    }

    pub(crate) fn stepped(steps: Subtasks) -> Item {
        let mut item = crate::status::tests::task(Some("working"), Some("working"), 2);
        item.name = "stripe".to_string();
        item.title = Some("Switch the payment provider to Stripe".to_string());
        item.place = "billing".to_string();
        crate::status::tests::in_phase(&mut item, Phase::Working);
        item.subtasks = ordered(steps);
        item
    }

    fn two_repos(app: &mut crate::board::App) {
        let checkout = |name: &str| crate::status::Checkout { name: name.to_string(), changed: 0, free_slots: 0, busy_slots: 1 };
        app.checkouts = vec![checkout("billing"), checkout("web")];
    }

    fn cell_fg(rows: &[String], buffer: &ratatui::buffer::Buffer, text: &str) -> Option<Color> {
        let (x, y) = find(rows, text)[0];
        Some(buffer[(x as u16, y as u16)].fg)
    }

    #[test]
    fn a_task_with_steps_lists_them_under_its_row_in_their_colours_and_stays_under_working() {
        let mut app = test_app(vec![stepped(stripe())]);
        two_repos(&mut app);
        app.work_selected = 9;
        let (rows, buffer) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        let at = find(&rows, "◐ Switch the payment provider to Stripe")[0].1;
        let block: Vec<String> = rows[at..at + 6].iter().map(|row| untied(row).to_string()).collect();
        assert!(block[0].ends_with("billing · 1 to accept"), "the commits count gives way to what waits for you: {screen}");
        assert_eq!(block[1], "↑ 1 pushed", "{screen}");
        assert!(block[2].starts_with("● 2 Charge through a PaymentGateway ") && block[2].ends_with(" ready · 1 commit"), "{screen}");
        assert!(block[3].starts_with("◐ 3 Add Stripe as the gateway ") && block[3].ends_with(" in progress"), "{screen}");
        assert_eq!(block[4], "○ 4 Remove the old payment client", "a planned step has nothing on the right: {screen}");
        assert_eq!(block[5], "accept 2 · r review", "{screen}");
        let column = |row: usize, mark: char| rows[row].chars().position(|character| character == mark).unwrap_or_default();
        assert!(column(at + 1, '↑') == column(at, '◐') + 3 && column(at + 2, '●') == column(at + 1, '↑'), "the steps sit indented under the row: {screen}");
        assert_eq!(column(at + 1, '│'), column(at, '◐'), "on the rule under the status glyph: {screen}");
        assert!(find(&rows, "WORKING")[0].1 < at && !screen.contains("NEEDS YOU"), "a step to accept leaves the task under Working: {screen}");
        assert_eq!(cell_fg(&rows, &buffer, "1 to accept"), Some(YELLOW));
        assert_eq!(cell_fg(&rows, &buffer, "↑ 1 pushed"), Some(MUTED));
        assert_eq!(cell_fg(&rows, &buffer, "● 2"), Some(YELLOW));
        assert_eq!(cell_fg(&rows, &buffer, "ready · 1 commit"), Some(YELLOW));
        assert!(!screen.contains("rules"), "a check that passed says nothing: {screen}");
        assert_eq!(cell_fg(&rows, &buffer, "◐ 3"), Some(BLUE));
        assert_eq!(cell_fg(&rows, &buffer, "○ 4"), Some(MUTED));
        assert_eq!(cell_fg(&rows, &buffer, "Remove the old"), Some(MUTED), "a planned step is muted whole");
        assert_eq!(cell_fg(&rows, &buffer, "Add Stripe"), Some(TEXT));
    }

    #[test]
    fn an_accepted_step_is_green_and_the_next_one_waiting_is_the_one_to_accept() {
        let mut steps = stripe();
        holds(&mut steps.items[0], &["2222222bbbb"], true);
        holds(&mut steps.items[1], &["3333333cccc"], false);
        steps.items[2].state = SubtaskState::InProgress;
        let mut item = stepped(steps);
        item.git.as_mut().unwrap().approved = false;
        let mut app = test_app(vec![item]);
        app.work_selected = 9;
        let (rows, buffer) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        let accepted = find(&rows, "✓ 2 Charge through a PaymentGateway")[0].1;
        assert!(rows[accepted].trim_end().ends_with("accepted · 1 commit"), "{screen}");
        assert_eq!(cell_fg(&rows, &buffer, "✓ 2"), Some(GREEN));
        assert_eq!(cell_fg(&rows, &buffer, "accepted · 1 commit"), Some(GREEN));
        let waiting = find(&rows, "● 3 Add Stripe as the gateway")[0].1;
        assert!(rows[waiting].trim_end().ends_with("ready · 1 commit"), "no rules approval and nothing checked under Working says only its state: {screen}");
        assert!(screen.contains("accept 3 · r review") && screen.contains("1 to accept"), "{screen}");
        let mut all = stripe();
        all.items.truncate(1);
        holds(&mut all.items[0], &["2222222bbbb"], true);
        let mut app = test_app(vec![stepped(all)]);
        app.work_selected = 9;
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(!screen.contains("to accept") && !screen.contains("accept 2") && screen.contains("2 commits"), "nothing to accept, the row counts its commits as it does without steps: {screen}");
    }

    #[test]
    fn a_ready_task_with_a_step_to_accept_waits_under_needs_you_only_while_accepting_is_required() {
        let mut item = stepped(stripe());
        crate::status::tests::in_phase(&mut item, Phase::Ready { head: None });
        let mut app = test_app(vec![item.clone()]);
        app.work_selected = 9;
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("READY TO PUSH") && !screen.contains("NEEDS YOU") && screen.contains("accept 2 · r review"), "accepting is optional without task.require_accept: {screen}");
        item.to_accept = Some(crate::status::tests::waiting_to_accept(2, 1));
        let mut app = test_app(vec![item]);
        app.work_selected = 9;
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        let at = find(&rows, "Switch the payment provider to Stripe")[0].1;
        assert!(find(&rows, "NEEDS YOU")[0].1 < at && !screen.contains("READY TO PUSH"), "{screen}");
        assert!(rows[at].trim_end().ends_with(" 1 to accept") && screen.contains("accept subtask 2 · r review"), "{screen}");
    }

    #[test]
    fn a_task_without_steps_shows_its_row_as_before() {
        let plain = crate::status::tests::task(Some("working"), Some("working"), 2);
        let mut app = test_app(vec![plain.clone()]);
        app.work_selected = 9;
        let (before, _) = render(&mut app, 120, 30);
        let mut with_empty = plain;
        with_empty.subtasks = Subtasks::default();
        let mut app = test_app(vec![with_empty]);
        app.work_selected = 9;
        let (after, _) = render(&mut app, 120, 30);
        assert_eq!(before, after);
        let screen = after.join("\n");
        let at = find(&after, "◐ billing")[0].1;
        assert!(after[at].contains("2 commits") && !screen.contains("pushed") && !screen.contains("accept") && !after[at + 1].contains('○'), "{screen}");
    }

    const LONG: &str = "Round VAT once per invoice and rate instead of on every line";

    fn long_steps() -> Subtasks {
        Subtasks { items: vec![
            held(0, LONG, &["2222222bbbb"], false),
            step(1, LONG, SubtaskState::InProgress),
            step(2, LONG, SubtaskState::Planned),
        ], ..Subtasks::default() }
    }

    fn step_rows(rows: &[String]) -> Vec<String> {
        rows.iter().map(|row| untied(row)).filter(|row| ['●', '◐', '○', '✓'].iter().any(|mark| row.starts_with(&format!("{mark} ")) && row.chars().nth(2).is_some_and(|digit| digit.is_ascii_digit()))).map(str::to_string).collect()
    }

    #[test]
    fn a_long_step_title_gives_way_so_its_state_always_shows() {
        let mut app = test_app(vec![stepped(long_steps())]);
        app.work_selected = 9;
        let (rows, _) = render(&mut app, 100, 30);
        let lines = step_rows(&rows);
        assert_eq!(lines, ["● 1 Round VAT once per…         ready · 1 commit", "◐ 2 Round VAT once per invoice and…  in progress", "○ 3 Round VAT once per invoice and rate instead…"], "{}", rows.join("\n"));
        let (rows, _) = render(&mut app, 60, 30);
        assert_eq!(step_rows(&rows), ["● 1 Round VAT once per invoice…     ready · 1 commit", "◐ 2 Round VAT once per invoice and…      in progress", "○ 3 Round VAT once per invoice and rate instead of…"], "{}", rows.join("\n"));
        for width in [100u16, 80, 60, 45] {
            let (rows, _) = render(&mut app, width, 30);
            let screen = rows.join("\n");
            let lines = step_rows(&rows);
            assert_eq!(lines.len(), 3, "{width}: {screen}");
            assert!(lines[0].ends_with(" ready · 1 commit") || lines[0].ends_with(" ready"), "{width}: the state word stands, the count gives way first: {screen}");
            assert!(lines[1].ends_with(" in progress"), "{width}: {screen}");
            for line in &lines {
                let title = line.chars().skip(4).collect::<String>().split("  ").next().unwrap_or_default().to_string();
                assert!(title == LONG || title.ends_with('…') && LONG.starts_with(title.trim_end_matches('…')), "{width}: a title is whole or cut with …: {screen}");
            }
            assert!(lines[0].contains('…'), "{width}: {screen}");
            assert!(lines.iter().all(|line| text_width(line) <= usize::from(width)), "{width}: {screen}");
        }
    }

    #[test]
    fn the_title_is_cut_before_the_state_and_the_rules_part_shortens_first() {
        let breaks = vec!["breaks 2 project rules".to_string(), "2 breaks".to_string()];
        let fit = |room| fitted("Charge through a PaymentGateway", "committed", &breaks, room);
        assert_eq!(fit(70), ("Charge through a PaymentGateway".to_string(), "committed · breaks 2 project rules".to_string()), "room for both, nothing cut");
        assert_eq!(fit(60), ("Charge through a…".to_string(), "committed · breaks 2 project rules".to_string()), "the title is cut before the label changes");
        assert_eq!(fit(45), ("Charge through a…".to_string(), "committed · 2 breaks".to_string()), "the rules part shortens before the title goes below its floor");
        assert_eq!(fit(30), ("Charge through a…".to_string(), "committed".to_string()), "then it goes, the state word stays");
        assert_eq!(fit(20), ("Charge…".to_string(), "committed".to_string()));
        assert_eq!(fit(13), ("C…".to_string(), "committed".to_string()), "a word too long for the room is cut inside it");
        assert_eq!(fit(11), ("Charge…".to_string(), String::new()), "only a row too narrow for the state word drops it");
        assert_eq!(fitted("Charge through a PaymentGateway", "", &[], 20), ("Charge through a…".to_string(), String::new()), "a planned step has no label");
        assert_eq!(whole_words("Charge through a PaymentGateway", 4), None);
        assert_eq!(whole_words("Charge through a PaymentGateway", 9), Some("Charge…".to_string()));
    }

    #[test]
    fn a_committed_step_says_its_rules_in_the_task_rows_words_shortened() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
        let state = SubtaskState::Ready;
        assert!(rules_forms(&item, &state).is_empty(), "a check that passed says nothing");
        item.git.as_mut().unwrap().approved = false;
        assert!(rules_forms(&item, &state).is_empty(), "not checked is the task's, said once on its line");
        item.rules = Some(crate::rules_check::Shown::Broken(1, Vec::new()));
        assert_eq!(rules_forms(&item, &state), vec!["breaks 1 project rule", "1 break"]);
        item.rules = Some(crate::rules_check::Shown::Broken(3, Vec::new()));
        assert_eq!(rules_forms(&item, &state), vec!["breaks 3 project rules", "3 breaks"]);
        item.rules = Some(crate::rules_check::Shown::Checking);
        assert_eq!(rules_forms(&item, &state), vec!["checking project rules", "checking"]);
        item.rules = Some(crate::rules_check::Shown::Lead);
        assert_eq!(rules_forms(&item, &state), vec!["checking project rules", "checking"]);
        item.git.as_mut().unwrap().approved = true;
        item.git.as_mut().unwrap().approved_by = Some(crate::tasks::ApprovedBy::User);
        assert_eq!(rules_forms(&item, &state), vec!["pushed past the rules check by you", "past the rules"]);
        assert!(rules_forms(&item, &SubtaskState::Accepted).is_empty() && rules_forms(&item, &SubtaskState::InProgress).is_empty());
        let mut working = stepped(stripe());
        working.git.as_mut().unwrap().approved = false;
        assert!(rules_forms(&working, &state).is_empty(), "under Working nothing was checked yet, the step says only committed");
    }

    #[test]
    fn a_task_with_many_ready_unchecked_subtasks_says_not_checked_once() {
        let steps = Subtasks { items: (0..16).map(|index| held(index, &format!("Subtask {index}"), &[&format!("{index:07}aaaa")], false)).collect(), ..Subtasks::default() };
        let mut item = stepped(steps);
        crate::status::tests::in_phase(&mut item, Phase::Ready { head: None });
        item.git.as_mut().unwrap().approved = false;
        assert_eq!(item.rules_shown(), Some(crate::status::RulesShown::Unchecked));
        let mut app = test_app(vec![item]);
        app.work_selected = 9;
        let (rows, _) = render(&mut app, 120, 40);
        let screen = rows.join("\n");
        assert_eq!(screen.matches("not checked").count(), 1, "{screen}");
        assert!(screen.contains(crate::status::UNCHECKED), "{screen}");
        assert!(step_rows(&rows).iter().all(|row| row.ends_with("ready · 1 commit")), "{screen}");
    }

    #[test]
    fn the_status_line_counts_the_commits_waiting_for_your_accept() {
        let mut second = stripe();
        holds(&mut second.items[1], &["3333333cccc"], false);
        let mut other = stepped(second);
        other.name = "other".to_string();
        let mut app = test_app(vec![stepped(stripe()), other, crate::status::tests::task(Some("idle"), None, 3)]);
        let (rows, buffer) = render(&mut app, 200, 30);
        assert!(rows[1].contains("✓ 1 ready to push   ◆ 3 to accept   ? 0 questions"), "{}", rows[1]);
        assert_eq!(cell_fg(&rows, &buffer, "◆ 3 to accept"), Some(YELLOW));
        let (rows, buffer) = render(&mut app, 60, 30);
        assert!(rows[1].contains("◐ 2   ✓ 1   ◆ 3 ") && !rows[1].contains("to accept"), "{}", rows[1]);
        assert_eq!(cell_fg(&rows, &buffer, "◆ 3"), Some(YELLOW));
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 3)]);
        let (wide, _) = render(&mut app, 200, 30);
        let (narrow, _) = render(&mut app, 60, 30);
        assert!(!wide[1].contains('◆') && !narrow[1].contains('◆'), "nothing to accept, no count: {}\n{}", wide[1], narrow[1]);
    }
}
