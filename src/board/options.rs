use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::backlog::{self, Entry};
use crate::config::Source;
use crate::keys::Act;
use crate::permit::Permit;
use crate::status::{Group, Item};
use super::{App, Column, Hit, Mode, Ongoing};
use super::detail::task_links;
use super::space::{Process, settings_of};
use super::widgets::{Choice, choice, menu_lines, modal, options_lines};

pub(super) fn task_choices(item: Option<&Item>, dev_server: Option<&str>, dev_worker: Option<&str>) -> Vec<Choice> {
    if let Some((now, drop)) = item.and_then(|item| item.start_now_means().zip(item.drop_means())) {
        return vec![
            choice(Act::Go, "Start it now", &format!("{now} (asks first)")),
            choice(Act::StartAt, "Start at…", &item.and_then(Item::start_at_means).unwrap_or_default()),
            choice(Act::Drop, "Drop the wait", &format!("{drop} (asks first)")),
        ];
    }
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
        if let Some(git) = item.filter(|item| item.push_waits.is_none()).and_then(|item| item.git.as_ref()).filter(|git| git.behind > 0) {
            choices.push(choice(Act::Rebase, &format!("Rebase onto {}", trunk(git)), "fetch and rebase it now, without pushing; a conflict goes to its Claude"));
        }
        choices.push(choice(Act::Tidy, "Tidy commits", "its Claude reshapes them by the rules skill"));
        if item.is_some_and(Item::in_push_line) {
            choices.push(choice(Act::Push, "Take out of the push line", "nothing of it is pushed; it waits under Ready to push until you push it again, at the end of the line"));
        } else if !item.is_some_and(Item::awaiting_review) {
            choices.push(choice(Act::Push, "Push", "its card and commits: push them all and close, or the first few and keep it open; it joins its repo's push line and is rebased when its turn comes"));
        }
        choices.push(match item.and_then(|item| item.git.as_ref()) {
            Some(git) if item.is_some_and(|item| item.landed.is_some()) => choice(Act::Done, "Done", &format!("its commits landed on {}: close it and free the worktree", trunk(git))),
            Some(git) if git.ahead == 0 && git.uncommitted == 0 => choice(Act::Done, "Done", &format!("close it: everything is on {}, and the worktree is freed", trunk(git))),
            _ => choice(Act::Done, "Done", "already pushed elsewhere: close it and free the worktree"),
        });
    }
    if item.and_then(|item| item.ci.as_ref()).is_some_and(|ci| !ci.running()) {
        choices.push(choice(Act::DismissCi, "Dismiss the red CI", "the failure is not the code's (a deploy window, infra, a flake): kept on record, and the task closes once its commits are on main (asks first)"));
    }
    if item.is_some_and(Item::is_task) {
        choices.push(if item.is_some_and(|item| item.permit == Some(Permit::Task)) {
            choice(Act::Permit, "Take back push permission", "the lead no longer pushes it; it waits for you under Ready to push")
        } else {
            choice(Act::Permit, "Let the lead push it", "the lead may push this task itself once its commits are approved; done, drop and reset take it back")
        });
    }
    if let Some(git) = item.and_then(|item| item.git.as_ref()) {
        let about = if item.is_some_and(|item| item.follows) {
            "on: the keeper rebases it when main moves, while its Claude is idle and its files are committed; never pushes. Choose it to switch off"
        } else {
            "the keeper rebases it whenever main moves, while its Claude is idle and its files are committed; a conflict goes to its Claude, nothing is pushed"
        };
        choices.push(choice(Act::Follow, &format!("Keep rebased on {}", trunk(git)), about));
    }
    choices.push(choice(Act::Reset, "Reset", "stop it for good: its work goes, its backlog item comes back (asks first)"));
    choices.push(choice(Act::Drop, "Drop", "throw the task away (asks first)"));
    choices
}

// A staged task can always be taken off; one goes on only in a repo that stages, once it is ready and out of review.
pub(super) fn stage_choice(item: &Item, stages: bool) -> Option<Choice> {
    if item.staged {
        return Some(choice(Act::Stage, "Take off local main", "its commits leave the main checkout's main, the other staged tasks stay; nothing is pushed and its branch keeps them"));
    }
    let ready = item.git.as_ref().is_some_and(|git| git.ahead > 0) && !busy(item) && !item.awaiting_review() && item.ci.is_none();
    (stages && ready).then(|| choice(Act::Stage, "Put on local main to try it", "its commits go on the main checkout's main, unpushed, after any staged before it; it closes once you push main"))
}

fn trunk(git: &crate::status::Git) -> &str {
    git.upstream.trim_start_matches("origin/")
}

// While its Claude works, the branch can still move, so the steps that rewrite, push or close it wait.
pub(super) fn busy(item: &Item) -> bool {
    item.group() == Group::Working || item.turn_under_way()
}

pub(super) fn ongoing_choices(note: &Ongoing) -> Vec<Choice> {
    vec![match (&note.task, &note.claim) {
        (Some(_), _) => choice(Act::Go, "Go to its task", "select the task that works on this note"),
        (None, Some(claim)) => choice(Act::Go, "Start here too", &format!("kelpie on {} works on it as {}; a task here too (asks first)", claim.machine, claim.task)),
        (None, None) => choice(Act::Go, "Resume as task", "a worktree, a Space and Claude with the note as its prompt"),
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
        choice(Act::StartAt, "Start at…", "the same, at a time you type: it waits under Queued until then"),
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
                if let Some(staging) = item.and_then(|item| stage_choice(item, settings.as_ref().is_some_and(|settings| settings.stage_on_main.value))) {
                    let after = choices.iter().position(|choice| choice.act == Some(Act::Push)).map_or(choices.len(), |at| at + 1);
                    choices.insert(after, staging);
                }
                if item.is_some_and(|item| self.question_of(item).is_some()) {
                    choices.insert(0, choice(Act::Card, "Answer its question", "the questions window, at this task's question"));
                } else if item.is_some_and(|item| self.card_of(item).is_some()) {
                    let after = choices.iter().position(|choice| choice.act == Some(Act::Review)).map_or(choices.len(), |at| at + 1);
                    choices.insert(after, choice(Act::Card, "Show the card", "its sign-off card with its images; p goes on to push"));
                }
                if let Some(item) = item {
                    let after = choices.iter().rposition(|choice| matches!(menu_group(choice.act), "SESSION" | "DEV")).map_or(0, |at| at + 1);
                    choices.splice(after..after, link_choices(task_links(self, item)));
                }
                choices
            }
            Column::Backlog => match self.backlog_row(self.backlog_selected) {
                Some(super::BacklogRow::Ongoing(note)) => ongoing_choices(&self.ongoing[note]),
                _ => backlog_choices(&self.sources),
            },
        }
    }
}

// Keys 1 to 9, one per link, so a task opens at most nine of its links from here.
pub(super) fn link_choices(links: Vec<(String, String)>) -> Vec<Choice> {
    links.into_iter().zip('1'..='9').map(|((label, url), key)| Choice { key, act: None, label, about: url.clone(), link: Some(url) }).collect()
}

fn menu_group(act: Option<Act>) -> &'static str {
    match act {
        None => "LINKS",
        Some(Act::Inbox | Act::Go | Act::Chat | Act::Review | Act::Card) => "SESSION",
        Some(Act::Server | Act::Worker) => "DEV",
        Some(Act::Reset | Act::Drop) => "END IT",
        _ => "BRANCH",
    }
}

impl App {
    pub(super) fn handle_options(&mut self, index: usize, act: Option<Act>, key: KeyEvent) {
        let choices = self.choices();
        let count = choices.len();
        let typed = match key.code {
            KeyCode::Char(character) => choices.iter().find(|choice| choice.key == character),
            _ => None,
        };
        match (typed, act) {
            (Some(chosen), _) => self.run_choice(chosen),
            (None, Some(Act::Move)) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => self.mode = Mode::Options(index.saturating_sub(1)),
            (None, Some(Act::Move)) => self.mode = Mode::Options((index + 1).min(count.saturating_sub(1))),
            (None, Some(Act::Options)) => {
                if let Some(chosen) = choices.get(index) {
                    self.run_choice(chosen);
                }
            }
            (None, Some(Act::Back | Act::Close)) => {}
            _ => self.mode = Mode::Options(index),
        }
    }

    pub(super) fn run_choice(&mut self, chosen: &Choice) {
        match (&chosen.link, chosen.act) {
            (Some(url), _) if self.viewer.is_none() => self.say(format!("No browser where the board runs; the address: {url}")),
            (Some(url), _) => self.open_file(url),
            (None, Some(act)) => self.act(act),
            (None, None) => {}
        }
    }
}

fn running(item: &Item, act: Option<Act>) -> bool {
    match act {
        Some(Act::Server) => item.server,
        Some(Act::Worker) => item.worker,
        Some(Act::Follow) => item.follows,
        _ => false,
    }
}

pub(super) fn draw_options(frame: &mut Frame, app: &App, index: usize) -> Vec<(Rect, Hit)> {
    let mut modal_hits = Vec::new();
    if app.column == Column::Work {
            let item = app.selected_task();
            let title: String = item.as_ref().map(|item| item.name.clone()).unwrap_or_default().chars().take(44).collect();
            let on = |act: Option<Act>| item.as_ref().is_some_and(|item| running(item, act));
            let (lines, at) = menu_lines(&app.choices(), index, 48, on, menu_group);
            let area = modal(frame, title, lines, 52);
            for (choice, line) in at.into_iter().enumerate() {
                let y = area.y + 1 + line as u16;
                if y < area.bottom().saturating_sub(1) {
                    modal_hits.push((Rect::new(area.x, y, area.width, 1), Hit::Choice(choice)));
                }
            }
    } else {
            let title = app.selected_ongoing().map(|note| note.title).or_else(|| app.selected_entry().map(|entry| entry.title)).unwrap_or_default();
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
    use crate::board::keys_window::shown;
    use crate::keys::Ctx;
    use crate::board::widgets::{GREEN, TEAL};

    #[test]
    fn the_backlog_column_adds_an_item_only_with_the_local_source() {
        let keys = |sources: &[Source]| backlog_choices(sources).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(&[Source::Local]), "glRn");
        assert_eq!(keys(&[Source::Vault, Source::Linear]), "glR");
    }

    #[test]
    fn done_on_a_task_with_nothing_left_says_everything_is_on_main() {
        let done = |item: &Item| task_choices(Some(item), None, None).into_iter().find(|choice| choice.act == Some(Act::Done)).unwrap().about;
        assert_eq!(done(&crate::status::tests::task(Some("idle"), None, 0)), "close it: everything is on main, and the worktree is freed", "a task kept open after its last push");
        assert_eq!(done(&crate::status::tests::task(Some("idle"), None, 2)), "already pushed elsewhere: close it and free the worktree");
    }

    #[test]
    fn a_working_task_offers_no_tidy_push_or_done() {
        let keys = |item: &Item| task_choices(Some(item), None, None).iter().map(|choice| choice.key).collect::<String>();
        assert_eq!(keys(&crate::status::tests::task(Some("working"), None, 1)), "gcrauzx");
        assert_eq!(keys(&crate::status::tests::task(Some("idle"), Some("ready"), 1)), "gcrvtpdauzx");
        assert_eq!(keys(&crate::status::tests::task(Some("idle"), Some("ready"), 0)), "gcrtpdauzx", "no commits, nothing to review");
    }

    #[test]
    fn a_task_in_review_offers_no_push_until_its_reviewer_passes_it() {
        let keys = |item: &Item| task_choices(Some(item), None, None).iter().map(|choice| choice.key).collect::<String>();
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        item.verdict = Some(crate::status::tests::in_review(false, 2, false, false));
        assert_eq!(keys(&item), "gcrvtdauzx");
        assert_eq!(row_facts(&item), "1 commit · review: 2 findings");
        let withdraw = task_choices(Some(&item), None, None).into_iter().find(|choice| choice.key == 'v').unwrap();
        assert_eq!(withdraw.label, "Withdraw from review", "the push stays the user's: a task in review can always be taken out");
        item.verdict = None;
        assert_eq!(keys(&item), "gcrvtpdauzx", "withdrawn, it offers push again");
        assert_eq!(task_choices(Some(&item), None, None).into_iter().find(|choice| choice.key == 'v').unwrap().label, "Send to review");
        item.verdict = Some(crate::status::tests::in_review(false, 2, false, false));
        item.verdict = Some(crate::status::tests::in_review(true, 0, false, false));
        assert_eq!(keys(&item), "gcrvtpdauzx");
        assert_eq!(row_facts(&item), "1 commit · reviewed");
    }

    #[test]
    fn a_ready_task_whose_claude_takes_a_turn_stays_ready_but_push_waits_for_the_turn() {
        let mut item = crate::status::tests::task(Some("working"), Some("ready"), 1);
        assert!(item.group() == crate::status::Group::ReadyToPush && busy(&item));
        item.agent.as_mut().unwrap().status = "idle".to_string();
        assert!(!busy(&item));
    }

    #[test]
    fn a_ready_task_goes_on_local_main_only_where_the_repo_stages_and_a_staged_one_comes_off() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        assert!(stage_choice(&item, false).is_none(), "a repo without worktree.stage_on_main offers nothing");
        let on = stage_choice(&item, true).unwrap();
        assert_eq!((on.key, on.label.as_str()), ('M', "Put on local main to try it"));
        item.agent.as_mut().unwrap().status = "working".to_string();
        assert!(stage_choice(&item, true).is_none(), "not while its Claude works");
        item.agent.as_mut().unwrap().status = "idle".to_string();
        item.staged = true;
        assert_eq!(stage_choice(&item, false).unwrap().label, "Take off local main", "a staged task comes off whatever the setting says now");
        assert!(shown(crate::keys::map(), Ctx::Work, false).iter().any(|(key, wide)| key == "M" && wide.contains("local main")), "the keys window lists it under PUSH");
    }

    #[test]
    fn a_task_in_the_push_line_offers_to_leave_it_and_no_rebase_and_a_red_cis_fix_offers_neither() {
        use crate::tasks::{PendingPush, PushBy, PushWork};
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        item.git.as_mut().unwrap().behind = 2;
        let labels = |item: &Item| task_choices(Some(item), None, None).into_iter().map(|choice| choice.label).collect::<Vec<_>>();
        assert!(labels(&item).contains(&"Rebase onto main".to_string()) && labels(&item).contains(&"Push".to_string()));
        item.push_waits = Some(PendingPush { by: PushBy::User, work: PushWork::Turn, at: 1, ahead: 1, after: None });
        assert!(labels(&item).contains(&"Take out of the push line".to_string()) && !labels(&item).iter().any(|label| label.starts_with("Rebase")), "{:?}", labels(&item));
        item.push_waits = Some(PendingPush { by: PushBy::User, work: PushWork::Ci, at: 1, ahead: 0, after: None });
        assert!(!labels(&item).contains(&"Take out of the push line".to_string()), "a red CI's fix is not in a line: {:?}", labels(&item));
    }

    #[test]
    fn a_ready_task_the_lead_has_not_approved_still_offers_push_and_close() {
        let mut unchecked = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        unchecked.git.as_mut().unwrap().approved = false;
        unchecked.rules = Some(crate::rules_check::Shown::Lead);
        assert!(unchecked.group() == crate::status::Group::ReadyToPush);
        assert!(task_choices(Some(&unchecked), None, None).iter().any(|choice| choice.key == 'p'));
        assert_eq!(row_facts(&unchecked), "1 commit · lead checking rules");
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
    fn rebase_onto_main_is_offered_only_when_the_task_is_behind_and_not_mid_turn() {
        let keys = |item: &Item| task_choices(Some(item), None, None).iter().map(|choice| choice.key).collect::<String>();
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        assert!(!keys(&item).contains('e'), "up to date: nothing to rebase");
        item.git.as_mut().unwrap().behind = 3;
        assert_eq!(keys(&item), "gcrvetpdauzx");
        let rebase = task_choices(Some(&item), None, None).into_iter().find(|choice| choice.key == 'e').unwrap();
        assert_eq!(rebase.label, "Rebase onto main");
        assert_eq!(menu_group(rebase.act), "BRANCH");
        let mut working = crate::status::tests::task(Some("working"), None, 1);
        working.git.as_mut().unwrap().behind = 3;
        assert!(!keys(&working).contains('e'), "its Claude is mid-turn");
    }

    #[test]
    fn keep_rebased_on_main_is_a_toggle_with_the_running_marker_and_a_chip_in_the_detail() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        let marked = |item: &Item| {
            let choices = task_choices(Some(item), None, None);
            let at = choices.iter().position(|choice| choice.act == Some(Act::Follow)).unwrap();
            let (lines, rows) = menu_lines(&choices, 0, 48, |act| running(item, act), menu_group);
            (choices[at].label.clone(), lines[rows[at]].to_string())
        };
        let (label, line) = marked(&item);
        assert_eq!(label, "Keep rebased on main");
        assert!(line.ends_with("  u") && !line.contains('●'), "off by default: {line}");
        assert_eq!(crate::board::detail::follow_chip(&item), None);
        item.follows = true;
        let (_, line) = marked(&item);
        assert!(line.ends_with("● u"), "{line}");
        assert_eq!(crate::board::detail::follow_chip(&item), Some(("follows main".to_string(), crate::board::widgets::BLUE)));
        assert_eq!(menu_group(Some(Act::Follow)), "BRANCH");
        let mut session = item.clone();
        session.git = None;
        assert!(!task_choices(Some(&session), None, None).iter().any(|choice| choice.act == Some(Act::Follow)), "a session outside a task has no branch");
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
        let (lines, at) = menu_lines(&choices, 7, 48, |act| act == Some(Act::Server) && item.server, menu_group);
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

    fn linked_app() -> App {
        let mut item = served("search-filters");
        item.place = "web".to_string();
        let run = |id: u64, name: &str| crate::ci::Run { id, name: name.to_string(), workflow: name.to_string(), status: "completed".to_string(), conclusion: "failure".to_string(), url: format!("https://github.com/acme/web/actions/runs/{id}") };
        item.ci = Some(crate::ci::Watch {
            address: "search-filters@web".to_string(), task: "search-filters".to_string(), repo: std::path::PathBuf::new(), slug: "acme/web".to_string(), sha: "abcdef0".to_string(),
            started_at: 0, checked_at: None, close: true, runs: vec![run(42, "CI"), run(43, "Lint")], state: crate::ci::State::Running, error: None, erring_since: None, pushed: crate::ci::Pushed::default(),
        });
        let long = format!("https://claude.ai/artifact/{}", "a1b2c3d4".repeat(12));
        item.report.as_mut().unwrap().text = format!("mock at {long} and the PR https://github.com/acme/web/pull/7, also https://docs.example.org/guide");
        let mut app = test_app(vec![item]);
        with_detail(&mut app);
        app.ongoing = vec![Ongoing { key: "Projects/search filters.md".to_string(), title: "search filters".to_string(), area: "web".to_string(), task: Some("search-filters@web".to_string()), claim: None, ..Ongoing::default() }];
        app
    }

    #[test]
    fn the_task_menu_lists_each_link_by_what_it_is_with_its_full_address() {
        let app = linked_app();
        let links: Vec<(char, String, String)> = app.choices().into_iter().filter(|choice| choice.link.is_some()).map(|choice| (choice.key, choice.label, choice.about)).collect();
        let long = format!("https://claude.ai/artifact/{}", "a1b2c3d4".repeat(12));
        assert_eq!(links, [
            ('1', "Open the dev server".to_string(), "https://wt2.web.test".to_string()),
            ('2', "Open the CI run CI".to_string(), "https://github.com/acme/web/actions/runs/42".to_string()),
            ('3', "Open the CI run Lint".to_string(), "https://github.com/acme/web/actions/runs/43".to_string()),
            ('4', "Open the mock".to_string(), long),
            ('5', "Open the pull request".to_string(), "https://github.com/acme/web/pull/7".to_string()),
            ('6', "Open docs.example.org".to_string(), "https://docs.example.org/guide".to_string()),
            ('7', "Open the note".to_string(), "obsidian://open?file=Projects%2Fsearch%20filters.md".to_string()),
        ]);
        let groups: Vec<&str> = app.choices().iter().map(|choice| menu_group(choice.act)).collect();
        let first = groups.iter().position(|group| *group == "LINKS").unwrap();
        assert_eq!((groups[first - 1], groups[first + 7]), ("SESSION", "BRANCH"), "LINKS follows the session and dev choices, before BRANCH: {groups:?}");
    }

    #[test]
    fn a_link_the_row_cuts_is_shown_whole_in_the_menu() {
        let mut app = linked_app();
        let at = app.choices().iter().position(|choice| choice.label == "Open the mock").unwrap();
        app.mode = Mode::Options(at);
        let (rows, _) = render(&mut app, 96, 50);
        let screen: String = rows.iter().map(|row| row.trim_matches(|character: char| character == ' ' || character == '│')).collect();
        assert!(screen.contains(&format!("https://claude.ai/artifact/{}", "a1b2c3d4".repeat(12))), "the whole address on the description line: {}", rows.join("\n"));
        assert!(rows.join("\n").contains("LINKS"));
    }

    #[test]
    fn a_link_key_opens_its_address_and_without_a_browser_says_it() {
        let dir = std::env::temp_dir().join(format!("kelpie-menu-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let opener = dir.join("open");
        let opened = dir.join("opened");
        crate::executable::write(&opener, format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", opened.display())).unwrap();
        let mut app = linked_app();
        app.viewer = Some(Box::leak(opener.display().to_string().into_boxed_str()));
        app.mode = Mode::Options(0);
        press(&mut app, key(KeyCode::Char('7')));
        assert!(matches!(app.mode, Mode::Normal));
        let read = (0..100).find_map(|_| std::fs::read_to_string(&opened).ok().filter(|text| !text.is_empty()).or_else(|| { std::thread::sleep(std::time::Duration::from_millis(20)); None }));
        assert_eq!(read.as_deref(), Some("obsidian://open?file=Projects%2Fsearch%20filters.md"));
        app.viewer = None;
        app.mode = Mode::Options(0);
        press(&mut app, key(KeyCode::Char('5')));
        assert!(app.notice.as_ref().is_some_and(|(text, _)| text.ends_with("https://github.com/acme/web/pull/7")), "over ssh the address is said instead");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn links_are_labelled_by_what_they_are() {
        use crate::board::detail::link_label;
        assert_eq!(link_label("https://github.com/acme/web/actions/runs/9"), "Open the CI run");
        assert_eq!(link_label("https://github.com/acme/web/pull/7"), "Open the pull request");
        assert_eq!(link_label("https://claude.ai/code/artifact/42"), "Open the mock");
        assert_eq!(link_label("https://acme.dev/mocks/checkout.html"), "Open the mock");
        assert_eq!(link_label("http://localhost:3002/admin"), "Open the dev server");
        assert_eq!(link_label("https://staging.acme.dev/x"), "Open staging.acme.dev");
    }

    #[test]
    fn the_task_menu_puts_each_key_in_its_group() {
        assert_eq!(menu_group(Some(Act::Inbox)), "SESSION");
        assert_eq!(menu_group(Some(Act::Server)), "DEV");
        assert_eq!(menu_group(Some(Act::Drop)), "END IT");
        assert_eq!(menu_group(Some(Act::Push)), "BRANCH");
        assert_eq!(menu_group(Some(Act::Permit)), "BRANCH");
        let help = shown(crate::keys::map(), Ctx::Work, false);
        let labels: Vec<&str> = help.iter().map(|(key, _)| key.as_str()).collect();
        for key in ['n', 'g', 'c', 'r', 'w', 'b', 'v', 'a', 'e', 'u', 'p', 'd', 'z', 'x', 'R', 'A', 'U', 'i', 'm', 'q', '/'] {
            assert!(labels.iter().any(|label| label.split_whitespace().any(|word| word == key.to_string())), "{key} is not in the keys help: {labels:?}");
        }
    }
}
