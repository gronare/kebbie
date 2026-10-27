use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::keys::{self, Act};
use crate::status::Item;
use crate::tasks::{self, Step, StepState};
use super::widgets::{BLUE, GREEN, MUTED, SURFACE, TEXT, YELLOW, style, text_width};

const INDENT: &str = "    ";

fn waiting(step: &Step) -> bool {
    matches!(step.state, StepState::Committed { accepted: false, .. })
}

pub(super) fn to_accept(item: &Item) -> usize {
    item.steps.items.iter().filter(|step| waiting(step)).count()
}

// The accepted steps at the front of the branch, which a push may take while the task keeps working.
pub(super) fn accepted_run(item: &Item) -> usize {
    item.steps.items.iter().take_while(|step| matches!(step.state, StepState::Committed { accepted: true, .. })).count()
}

pub(super) fn to_accept_note(item: &Item) -> Option<String> {
    let count = to_accept(item);
    (count > 0).then(|| format!("{count} to accept"))
}

pub(super) fn accept_wanted(item: &Item) -> Option<String> {
    let step = item.steps.items.iter().find(|step| waiting(step))?;
    Some(format!("accept {} · {} review", step.index + 1, keys::key(Act::Review)))
}

pub(super) fn mark_color(state: &StepState) -> Color {
    match state {
        StepState::Planned => MUTED,
        StepState::InProgress => BLUE,
        StepState::Committed { accepted: false, .. } => YELLOW,
        StepState::Committed { accepted: true, .. } => GREEN,
    }
}

// The right side of a step's line, longest first; a narrow board takes the next one before it cuts the title.
fn labels(item: &Item, state: &StepState) -> Vec<String> {
    match state {
        StepState::Planned => vec![String::new()],
        StepState::InProgress => vec!["in progress".to_string(), String::new()],
        StepState::Committed { accepted: true, .. } => vec!["accepted".to_string(), String::new()],
        StepState::Committed { accepted: false, .. } => {
            let rules = if item.git.as_ref().is_some_and(|git| git.approved) { Some("rules ✓".to_string()) } else { item.rules_note() };
            let mut labels: Vec<String> = rules.map(|rules| format!("committed · {rules}")).into_iter().collect();
            labels.extend(["committed".to_string(), String::new()]);
            labels
        }
    }
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

// Each label is tried with the title whole before the title is cut; a cut title stands alone, its mark tells the state.
fn fitted(title: &str, labels: &[String], room: usize) -> (String, String) {
    for label in labels {
        let title_room = if label.is_empty() { room } else { room.saturating_sub(text_width(label) + 2) };
        if text_width(title) <= title_room {
            return (title.to_string(), label.clone());
        }
    }
    (whole_words(title, room).unwrap_or_else(|| "…".to_string()), String::new())
}

fn step_line(item: &Item, step: &Step, width: usize) -> Line<'static> {
    let number = format!("{} ", step.index + 1);
    let room = width.saturating_sub(INDENT.len() + 2 + number.len());
    let (title, label) = fitted(&step.title, &labels(item, &step.state), room);
    let color = mark_color(&step.state);
    let planned = step.state == StepState::Planned;
    let gap = room.saturating_sub(text_width(&title) + text_width(&label));
    Line::from(vec![
        Span::raw(INDENT), Span::styled(tasks::step_mark(&step.state), style(color)), Span::raw(" "),
        Span::styled(number, style(MUTED)), Span::styled(title, style(if planned { MUTED } else { TEXT })),
        Span::raw(" ".repeat(gap)), Span::styled(label, style(color)),
    ])
}

pub(super) fn step_lines(item: &Item, highlighted: bool, width: usize) -> Vec<Line<'static>> {
    if !item.steps.listed() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    if item.steps.pushed > 0 {
        lines.push(Line::from(vec![Span::raw(INDENT), Span::styled(format!("↑ {} pushed", item.steps.pushed), style(MUTED))]));
    }
    lines.extend(item.steps.items.iter().map(|step| step_line(item, step, width)));
    if highlighted {
        lines.into_iter().map(|line| line.style(Style::new().bg(SURFACE))).collect()
    } else {
        lines
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::board::test_support::*;
    use crate::phase::Phase;
    use crate::tasks::Steps;

    fn step(index: usize, title: &str, state: StepState) -> Step {
        Step { index, title: title.to_string(), state, extra: false }
    }

    pub(crate) fn committed(sha: &str, accepted: bool) -> StepState {
        StepState::Committed { sha: sha.to_string(), subject: format!("subject of {sha}"), accepted }
    }

    pub(crate) fn stripe() -> Steps {
        Steps { pushed: 1, items: vec![
            step(1, "Charge through a PaymentGateway", committed("2222222bbbb", false)),
            step(2, "Add Stripe as the gateway", StepState::InProgress),
            step(3, "Remove the old payment client", StepState::Planned),
        ] }
    }

    pub(crate) fn stepped(steps: Steps) -> Item {
        let mut item = crate::status::tests::task(Some("working"), Some("working"), 2);
        item.name = "stripe".to_string();
        item.title = Some("Switch the payment provider to Stripe".to_string());
        item.place = "billing".to_string();
        crate::status::tests::in_phase(&mut item, Phase::Working);
        item.steps = steps;
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
        let block: Vec<String> = rows[at..at + 6].iter().map(|row| row.trim().to_string()).collect();
        assert!(block[0].ends_with("billing · 1 to accept"), "the commits count gives way to what waits for you: {screen}");
        assert_eq!(block[1], "↑ 1 pushed", "{screen}");
        assert!(block[2].starts_with("● 2 Charge through a PaymentGateway ") && block[2].ends_with(" committed · rules ✓"), "{screen}");
        assert!(block[3].starts_with("◐ 3 Add Stripe as the gateway ") && block[3].ends_with(" in progress"), "{screen}");
        assert_eq!(block[4], "○ 4 Remove the old payment client", "a planned step has nothing on the right: {screen}");
        assert_eq!(block[5], "accept 2 · r review", "{screen}");
        let indent = |row: usize| rows[row].chars().take_while(|character| *character == ' ').count();
        assert!(indent(at + 1) == indent(at) + 3 && indent(at + 2) == indent(at + 1), "the steps sit indented under the row: {screen}");
        assert!(find(&rows, "WORKING")[0].1 < at && !screen.contains("NEEDS YOU"), "a step to accept leaves the task under Working: {screen}");
        assert_eq!(cell_fg(&rows, &buffer, "1 to accept"), Some(YELLOW));
        assert_eq!(cell_fg(&rows, &buffer, "↑ 1 pushed"), Some(MUTED));
        assert_eq!(cell_fg(&rows, &buffer, "● 2"), Some(YELLOW));
        assert_eq!(cell_fg(&rows, &buffer, "committed · rules ✓"), Some(YELLOW));
        assert_eq!(cell_fg(&rows, &buffer, "◐ 3"), Some(BLUE));
        assert_eq!(cell_fg(&rows, &buffer, "○ 4"), Some(MUTED));
        assert_eq!(cell_fg(&rows, &buffer, "Remove the old"), Some(MUTED), "a planned step is muted whole");
        assert_eq!(cell_fg(&rows, &buffer, "Add Stripe"), Some(TEXT));
    }

    #[test]
    fn an_accepted_step_is_green_and_the_next_one_waiting_is_the_one_to_accept() {
        let mut steps = stripe();
        steps.items[0].state = committed("2222222bbbb", true);
        steps.items[1].state = committed("3333333cccc", false);
        steps.items[2].state = StepState::InProgress;
        let mut item = stepped(steps);
        item.git.as_mut().unwrap().approved = false;
        let mut app = test_app(vec![item]);
        app.work_selected = 9;
        let (rows, buffer) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        let accepted = find(&rows, "✓ 2 Charge through a PaymentGateway")[0].1;
        assert!(rows[accepted].trim_end().ends_with("accepted"), "{screen}");
        assert_eq!(cell_fg(&rows, &buffer, "✓ 2"), Some(GREEN));
        assert_eq!(cell_fg(&rows, &buffer, "accepted"), Some(GREEN));
        let waiting = find(&rows, "● 3 Add Stripe as the gateway")[0].1;
        assert!(rows[waiting].trim_end().ends_with("committed"), "no rules approval and nothing checked under Working says only committed: {screen}");
        assert!(screen.contains("accept 3 · r review") && screen.contains("1 to accept"), "{screen}");
        let mut all = stripe();
        all.items.truncate(1);
        all.items[0].state = committed("2222222bbbb", true);
        let mut app = test_app(vec![stepped(all)]);
        app.work_selected = 9;
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(!screen.contains("to accept") && !screen.contains("accept 2") && screen.contains("2 commits"), "nothing to accept, the row counts its commits as it does without steps: {screen}");
    }

    #[test]
    fn a_task_without_steps_shows_its_row_as_before() {
        let plain = crate::status::tests::task(Some("working"), Some("working"), 2);
        let mut app = test_app(vec![plain.clone()]);
        app.work_selected = 9;
        let (before, _) = render(&mut app, 120, 30);
        let mut with_empty = plain;
        with_empty.steps = Steps::default();
        let mut app = test_app(vec![with_empty]);
        app.work_selected = 9;
        let (after, _) = render(&mut app, 120, 30);
        assert_eq!(before, after);
        let screen = after.join("\n");
        let at = find(&after, "◐ billing")[0].1;
        assert!(after[at].contains("2 commits") && !screen.contains("pushed") && !screen.contains("accept") && !after[at + 1].contains('○'), "{screen}");
    }

    #[test]
    fn a_narrow_board_drops_the_rules_then_the_label_and_cuts_titles_only_between_words() {
        let mut app = test_app(vec![stepped(stripe())]);
        app.work_selected = 9;
        for width in [40u16, 34, 28, 22] {
            let (rows, _) = render(&mut app, width, 30);
            let screen = rows.join("\n");
            let lines: Vec<&String> = rows.iter().filter(|row| ['●', '◐', '○'].iter().any(|mark| row.trim_start().starts_with(&format!("{mark} ")) && row.starts_with("    "))).collect();
            assert_eq!(lines.len(), 3, "{width}: {screen}");
            for line in lines {
                let title: String = line.trim().chars().skip(4).collect();
                let shown = title.split("  ").next().unwrap_or_default().trim_end_matches('…').trim();
                let whole = ["Charge through a PaymentGateway", "Add Stripe as the gateway", "Remove the old payment client"].iter().any(|full| full.split(' ').collect::<Vec<_>>().starts_with(&shown.split(' ').collect::<Vec<_>>()));
                assert!(whole, "{width}: {shown:?} is cut mid-word in:\n{screen}");
            }
        }
        let item = stepped(stripe());
        let labels = labels(&item, &item.steps.items[0].state);
        assert_eq!(fitted("Charge through a PaymentGateway", &labels, 60), ("Charge through a PaymentGateway".to_string(), "committed · rules ✓".to_string()));
        assert_eq!(fitted("Charge through a PaymentGateway", &labels, 45), ("Charge through a PaymentGateway".to_string(), "committed".to_string()), "the rules part goes first");
        assert_eq!(fitted("Charge through a PaymentGateway", &labels, 34), ("Charge through a PaymentGateway".to_string(), String::new()), "then the label, while the title fits whole");
        assert_eq!(fitted("Charge through a PaymentGateway", &labels, 30), ("Charge through a…".to_string(), String::new()), "a title that does not fit alone is cut between words");
        assert_eq!(fitted("Charge through a PaymentGateway", &labels, 5), ("…".to_string(), String::new()));
        let (rows, _) = render(&mut app, 46, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("● 2 Charge through a PaymentGateway") && !screen.contains("committed"), "{screen}");
        assert_eq!(whole_words("Charge through a PaymentGateway", 4), None);
        assert_eq!(whole_words("Charge through a PaymentGateway", 9), Some("Charge…".to_string()));
    }

    #[test]
    fn the_status_line_counts_the_commits_waiting_for_your_accept() {
        let mut second = stripe();
        second.items[1].state = committed("3333333cccc", false);
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
