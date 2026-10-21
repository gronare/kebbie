use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::backlog::{self, Entry};
use crate::config::Source;
use crate::keys::Act;
use crate::permit::Permit;
use crate::status::{Group, Item};
use super::{App, Column, Hit, Mode, Ongoing};
use super::space::{Process, settings_of};
use super::widgets::{Choice, FAINT, GREEN, MAUVE, MUTED, RED, SUBTEXT, SURFACE, TEXT, bold, choice, modal, options_lines, style};

pub(super) fn task_choices(item: Option<&Item>, dev_server: Option<&str>, dev_worker: Option<&str>) -> Vec<Choice> {
    let go = if item.is_some_and(|item| item.agent.is_none()) {
        choice(Act::Go, "Restart it", "reopen its Space and its Claude, resuming its last conversation")
    } else {
        choice(Act::Go, "Go to its Space", "jump to the task's Space")
    };
    let mut choices = vec![
        go,
        choice(Act::Chat, "Message its Claude", "send it a line without leaving"),
        choice(Act::Review, "Open the review", "the review split next to its Claude"),
    ];
    if let Some(command) = dev_server {
        choices.push(if item.is_some_and(|item| item.server) {
            choice(Act::Server, "Stop the dev server", "ctrl-c to its server tab")
        } else {
            choice(Act::Server, "Start the dev server", &format!("{command} in a tab of its Space"))
        });
    }
    if let Some(command) = dev_worker {
        choices.push(if item.is_some_and(|item| item.worker) {
            choice(Act::Worker, "Stop the worker", "ctrl-c to its worker tab")
        } else {
            choice(Act::Worker, "Start the worker", &format!("{command} in a tab of its Space"))
        });
    }
    if !item.is_some_and(busy) {
        if item.and_then(|item| item.git.as_ref()).is_some_and(|git| git.ahead > 0) {
            choices.push(if item.is_some_and(Item::awaiting_review) {
                choice(Act::SendReview, "Withdraw from review", "stop its reviewer and forget its verdict: push is offered again, and the lead is told")
            } else {
                choice(Act::SendReview, "Send to review", "a separate, read-only Claude reviews its commits; the task waits under To review until it passes them")
            });
        }
        choices.push(choice(Act::Tidy, "Tidy commits", "its Claude reshapes them by the rules skill"));
        if !item.is_some_and(Item::awaiting_review) {
            choices.push(choice(Act::Push, "Push", "its card and commits: push them all and close, or the first few and keep it open; rebases first"));
        }
        choices.push(choice(Act::Done, "Done", "already pushed elsewhere: close it and free the worktree"));
    }
    if item.is_some_and(Item::is_task) {
        choices.push(if item.is_some_and(|item| item.permit == Some(Permit::Task)) {
            choice(Act::Permit, "Take back push permission", "the lead no longer pushes it; it waits for you under Ready to push")
        } else {
            choice(Act::Permit, "Let the lead push it", "the lead may push this task itself once its commits are approved; done, drop and reset take it back")
        });
    }
    choices.push(choice(Act::Reset, "Reset", "stop it for good: its work goes, its backlog item comes back (asks first)"));
    choices.push(choice(Act::Drop, "Drop", "throw the task away (asks first)"));
    choices
}

// While its Claude works, the branch can still move, so the steps that rewrite, push or close it wait.
pub(super) fn busy(item: &Item) -> bool {
    item.group() == Group::Working
}

pub(super) fn ongoing_choices(note: &Ongoing) -> Vec<Choice> {
    vec![match note.task {
        Some(_) => choice(Act::Go, "Go to its task", "select the task that works on this note"),
        None => choice(Act::Go, "Resume as task", "a worktree, a Space and Claude with the note as its prompt"),
    }]
}

pub(super) fn ongoing_entry(note: &Ongoing) -> Entry {
    Entry {
        source: "vault".to_string(), title: note.title.clone(), priority: "none".to_string(), area: note.area.clone(),
        reference: note.key.clone(), summary: String::new(), age: String::new(),
    }
}

pub(super) fn backlog_choices(sources: &[Source]) -> Vec<Choice> {
    let mut choices = vec![
        choice(Act::Go, "Start as task", "a worktree, a Space and Claude with this as its prompt"),
        choice(Act::Refresh, "Refresh the backlog", &backlog::refresh_label(sources)),
    ];
    if sources.contains(&Source::Local) {
        choices.push(choice(Act::New, "Add an item", "a title and a priority, appended to the local backlog file"));
    }
    choices
}

impl App {
    pub(super) fn choices(&self) -> Vec<Choice> {
        match self.column {
            Column::Work => {
                let item = self.tasks.get(self.work_selected);
                let settings = item.and_then(settings_of);
                let command = |process: Process| settings.as_ref().and_then(|settings| process.command(settings));
                let mut choices = task_choices(item, command(Process::Server).as_deref(), command(Process::Worker).as_deref());
                if item.is_some_and(|item| self.question_of(item).is_some()) {
                    choices.insert(0, choice(Act::Inbox, "Answer its question", "the inbox, at this task's question"));
                }
                if item.is_some_and(|item| self.card_of(item).is_some()) {
                    let after = choices.iter().position(|choice| choice.act == Some(Act::Review)).map_or(choices.len(), |at| at + 1);
                    choices.insert(after, choice(Act::Card, "Show the card", "its sign-off card with its images; p goes on to push"));
                }
                choices
            }
            Column::Backlog if self.backlog_selected < self.ongoing.len() => ongoing_choices(&self.ongoing[self.backlog_selected]),
            Column::Backlog => backlog_choices(&self.sources),
        }
    }
}

fn menu_group(act: Option<Act>) -> &'static str {
    match act {
        Some(Act::Inbox | Act::Go | Act::Chat | Act::Review | Act::Card) => "SESSION",
        Some(Act::Server | Act::Worker) => "DEV",
        Some(Act::Reset | Act::Drop) => "END IT",
        _ => "BRANCH",
    }
}

// The second value is each choice's line, which the mouse hits are placed on.
fn menu_lines(choices: &[Choice], index: usize, width: usize, running: impl Fn(Option<Act>) -> bool) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut at = Vec::new();
    let mut group = "";
    for (position, choice) in choices.iter().enumerate() {
        if menu_group(choice.act) != group {
            group = menu_group(choice.act);
            lines.push(Line::from(Span::styled(group, style(MUTED))));
        }
        let active = position == index;
        let ending = matches!(choice.act, Some(Act::Reset | Act::Drop));
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
    if let Some(choice) = choices.get(index) {
        lines.push(Line::from(Span::styled(choice.about.clone(), style(SUBTEXT))));
    }
    lines.push(Line::from(Span::styled("↑ ↓ choose · ⏎ run · a letter runs it directly · esc close", style(FAINT))));
    (lines, at)
}

impl App {
    pub(super) fn handle_options(&mut self, index: usize, act: Option<Act>, key: KeyEvent) {
        let choices = self.choices();
        let count = choices.len();
        let typed = match key.code {
            KeyCode::Char(character) => choices.iter().find(|choice| choice.key == character).and_then(|choice| choice.act),
            _ => None,
        };
        match (typed, act) {
            (Some(chosen), _) => self.act(chosen),
            (None, Some(Act::Move)) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => self.mode = Mode::Options(index.saturating_sub(1)),
            (None, Some(Act::Move)) => self.mode = Mode::Options((index + 1).min(count.saturating_sub(1))),
            (None, Some(Act::Options)) => {
                if let Some(chosen) = choices.get(index).and_then(|choice| choice.act) {
                    self.act(chosen);
                }
            }
            (None, Some(Act::Back | Act::Close)) => {}
            _ => self.mode = Mode::Options(index),
        }
    }
}

pub(super) fn draw_options(frame: &mut Frame, app: &App, index: usize) -> Vec<(Rect, Hit)> {
    let mut modal_hits = Vec::new();
    if app.column == Column::Work {
            let item = app.selected_task();
            let title: String = item.as_ref().map(|item| item.name.clone()).unwrap_or_default().chars().take(44).collect();
            let running = |act: Option<Act>| item.as_ref().is_some_and(|item| (act == Some(Act::Server) && item.server) || (act == Some(Act::Worker) && item.worker));
            let (lines, at) = menu_lines(&app.choices(), index, 48, running);
            let area = modal(frame, title, lines, 52);
            for (choice, line) in at.into_iter().enumerate() {
                let y = area.y + 1 + line as u16;
                if y < area.bottom().saturating_sub(1) {
                    modal_hits.push((Rect::new(area.x, y, area.width, 1), Hit::Choice(choice)));
                }
            }
    } else {
            let title = match app.backlog_selected.checked_sub(app.ongoing.len()) {
                Some(_) => app.selected_entry().map(|entry| entry.title).unwrap_or_default(),
                None => app.ongoing.get(app.backlog_selected).map(|note| note.title.clone()).unwrap_or_default(),
            };
            let title: String = title.chars().take(52).collect();
            let count = app.choices().len();
            let area = modal(frame, title, options_lines(app.choices(), index, 56), 62);
            for choice in 0..count {
                let y = area.y + 1 + 2 * choice as u16;
                if y + 1 < area.bottom() {
                    modal_hits.push((Rect::new(area.x, y, area.width, 2), Hit::Choice(choice)));
                }
            }
    }
    modal_hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::test_support::*;
    use crate::config::Source;
    use crate::keys::Act;
    use crate::permit::Permit;
    use crate::status::Item;
    use crate::board::columns::row_facts;
    use crate::board::detail::permit_chip;
    use crate::board::{Hit, Mode};
    use crate::board::status_bar::help_lines;
    use crate::board::widgets::{GREEN, TEAL};

    #[test]
    fn the_backlog_column_adds_an_item_only_with_the_local_source() {
        let keys = |sources: &[Source]| backlog_choices(sources).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(&[Source::Local]), "gRn");
        assert_eq!(keys(&[Source::Vault, Source::Linear]), "gR");
    }

    #[test]
    fn a_working_task_offers_no_tidy_push_or_done() {
        let keys = |item: &Item| task_choices(Some(item), None, None).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(&crate::status::tests::task(Some("working"), None, 1)), "gcrazx");
        assert_eq!(keys(&crate::status::tests::task(Some("idle"), Some("ready"), 1)), "gcrvtpdazx");
        assert_eq!(keys(&crate::status::tests::task(Some("idle"), Some("ready"), 0)), "gcrtpdazx", "no commits, nothing to review");
    }

    #[test]
    fn a_task_in_review_offers_no_push_until_its_reviewer_passes_it() {
        let keys = |item: &Item| task_choices(Some(item), None, None).iter().map(|choice| choice.key).collect::<String>();
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        item.verdict = Some(crate::status::tests::in_review(false, 2, false, false));
        assert_eq!(keys(&item), "gcrvtdazx");
        assert_eq!(row_facts(&item), "1 commit · review: 2 findings");
        let withdraw = task_choices(Some(&item), None, None).into_iter().find(|choice| choice.key == 'v').unwrap();
        assert_eq!(withdraw.label, "Withdraw from review", "the push stays the user's: a task in review can always be taken out");
        item.verdict = None;
        assert_eq!(keys(&item), "gcrvtpdazx", "withdrawn, it offers push again");
        assert_eq!(task_choices(Some(&item), None, None).into_iter().find(|choice| choice.key == 'v').unwrap().label, "Send to review");
        item.verdict = Some(crate::status::tests::in_review(false, 2, false, false));
        item.verdict = Some(crate::status::tests::in_review(true, 0, false, false));
        assert_eq!(keys(&item), "gcrvtpdazx");
        assert_eq!(row_facts(&item), "1 commit · reviewed");
    }

    #[test]
    fn a_ready_task_the_lead_has_not_approved_still_offers_push_and_close() {
        let mut unchecked = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        unchecked.git.as_mut().unwrap().approved = false;
        assert!(unchecked.group() == crate::status::Group::NeedsYou);
        assert!(task_choices(Some(&unchecked), None, None).iter().any(|choice| choice.key == 'p'));
        assert_eq!(row_facts(&unchecked), "1 commit · rules not checked by the lead");
    }

    #[test]
    fn a_task_without_its_claude_shows_why_and_g_restarts_it() {
        let gone = crate::status::tests::task(None, None, 1);
        assert_eq!(row_facts(&gone), "no Space · 1 commit");
        assert_eq!(task_choices(Some(&gone), None, None)[0].label, "Restart it");
        let mut shell_only = crate::status::tests::task(None, None, 0);
        shell_only.workspace = Some("w1".to_string());
        assert_eq!(row_facts(&shell_only), "no Claude · nothing yet");
        let running = crate::status::tests::task(Some("idle"), None, 0);
        assert_eq!(row_facts(&running), "nothing yet");
        assert_eq!(task_choices(Some(&running), None, None)[0].label, "Go to its Space");
    }

    #[test]
    fn the_user_gives_and_takes_back_the_leads_push_permission_per_task_and_the_card_shows_it() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        let permission = |item: &Item| task_choices(Some(item), None, None).into_iter().find(|choice| choice.key == 'a').map(|choice| choice.label);
        assert_eq!(permission(&item).as_deref(), Some("Let the lead push it"));
        assert!(!row_facts(&item).contains("lead may push"));
        item.permit = Some(Permit::Task);
        assert_eq!(permission(&item).as_deref(), Some("Take back push permission"));
        assert!(row_facts(&item).ends_with("lead may push"), "{}", row_facts(&item));
        assert_eq!(permit_chip(item.permit), Some(("lead may push".to_string(), GREEN)));
        item.permit = Some(Permit::Repo);
        assert_eq!(permission(&item).as_deref(), Some("Let the lead push it"), "the repo's permission is switched on the Areas page");
        assert_eq!(permit_chip(item.permit), Some(("lead may push once reviewed (repo)".to_string(), TEAL)));
        assert_eq!(permit_chip(None), None);
        let mut session = item.clone();
        session.git = None;
        assert_eq!(permission(&session), None, "a session outside a task has nothing to push");
    }

    #[test]
    fn w_is_offered_only_with_a_dev_server() {
        let keys = |dev_server: Option<&str>| task_choices(None, dev_server, None).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(None), "gcrtpdzx");
        assert_eq!(keys(Some("bin/dev")), "gcrwtpdzx");
        let start = task_choices(None, Some("npm run dev"), None);
        assert_eq!((start[3].label.as_str(), start[3].about.as_str()), ("Start the dev server", "npm run dev in a tab of its Space"));
    }

    #[test]
    fn b_is_offered_only_with_a_dev_worker() {
        let keys = |dev_worker: Option<&str>| task_choices(None, Some("bin/dev"), dev_worker).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(None), "gcrwtpdzx");
        assert_eq!(keys(Some("bin/jobs")), "gcrwbtpdzx");
        assert_eq!(task_choices(None, None, Some("bin/jobs")).iter().map(|choice| choice.key).collect::<String>(), "gcrbtpdzx", "a worker needs no dev server");
        let mut item = crate::status::tests::task(Some("idle"), None, 0);
        let labels = |item: &Item| task_choices(Some(item), Some("bin/dev"), Some("bin/jobs")).into_iter().filter(|choice| "wb".contains(choice.key)).map(|choice| choice.label).collect::<Vec<_>>();
        assert_eq!(labels(&item), ["Start the dev server", "Start the worker"]);
        item.server = true;
        item.worker = true;
        assert_eq!(labels(&item), ["Stop the dev server", "Stop the worker"]);
    }

    #[test]
    fn the_task_menu_groups_its_choices_and_says_only_what_the_highlighted_one_does() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
        item.server = true;
        let choices = task_choices(Some(&item), Some("bin/dev"), Some("bin/jobs"));
        let (lines, at) = menu_lines(&choices, 7, 48, |act| act == Some(Act::Server) && item.server);
        let text: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        assert_eq!(text.iter().filter(|line| ["SESSION", "DEV", "BRANCH", "END IT"].contains(&line.as_str())).count(), 4, "{text:#?}");
        assert!(text[at[3]].starts_with("  Stop the dev server") && text[at[3]].ends_with("● w"), "{}", text[at[3]]);
        assert!(text[at[7]].starts_with("▸ Push") && text[at[7]].ends_with("  p"), "{}", text[at[7]]);
        assert!(at.iter().all(|line| text[*line].chars().count() == 48));
        assert!(text.contains(&choices[7].about) && !text.contains(&choices[6].about), "only the highlighted choice's description");
        let mut app = test_app(vec![item]);
        app.mode = Mode::Options(2);
        let (rows, _) = render(&mut app, 96, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("SESSION") && screen.contains("BRANCH") && screen.contains("END IT"), "{screen}");
        for (rect, hit) in &app.hits {
            let Hit::Choice(index) = hit else { panic!("only choices are hit in the menu") };
            let label = &app.choices()[*index].label;
            assert!(rows[rect.y as usize].contains(label.as_str()), "the hit for {label} is on its line");
        }
        assert_eq!(app.hits.len(), app.choices().len());
    }

    #[test]
    fn the_task_menu_puts_each_key_in_its_group() {
        assert_eq!(menu_group(Some(Act::Inbox)), "SESSION");
        assert_eq!(menu_group(Some(Act::Server)), "DEV");
        assert_eq!(menu_group(Some(Act::Drop)), "END IT");
        assert_eq!(menu_group(Some(Act::Push)), "BRANCH");
        assert_eq!(menu_group(Some(Act::Permit)), "BRANCH");
        let help = help_lines(false);
        let labels: Vec<&str> = help.iter().map(|line| line.spans[0].content.trim()).collect();
        for key in ['n', 'g', 'c', 'r', 'w', 'b', 'v', 'a', 'p', 'd', 'z', 'x', 'R', 'A', 'U', 'i', 'm', 'q', '/'] {
            assert!(labels.iter().any(|label| label.split_whitespace().any(|word| word == key.to_string())), "{key} is not in the keys help: {labels:?}");
        }
    }
}
