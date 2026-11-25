use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::{DefaultTerminal, Frame};

use crate::config::Second;
use crate::keys::Act;
use crate::notice::{Notice, Tone};
use crate::profiles::{self, Profile};
use crate::review::{self, Request};
use crate::status::Item;
use super::widgets::{Choice, MUTED, modal, options_lines, style};
use super::{App, Hit, Mode};

const WIDTH: usize = 66;

// The menu `v` opens on a task, made once when it opens so drawing reads no file.
pub(super) struct ReviewMenu {
    pub title: String,
    pub facts: String,
    pub choices: Vec<Choice>,
    pub index: usize,
    pub path: PathBuf,
    pub second_file: Option<PathBuf>,
}

// A reviewer as the menu names it: its profile's name and what it runs, or why it does not load.
pub(super) struct Named {
    pub name: String,
    pub runs: String,
}

pub(super) fn runs(profile: &Profile) -> String {
    let mut parts = vec![profile.agent.key().to_string()];
    parts.extend(profile.model.clone());
    parts.extend(profile.effort.clone());
    parts.join(" · ")
}

fn named(name: &str, loaded: crate::sh::Result<Profile>) -> Named {
    Named { name: name.to_string(), runs: loaded.map_or_else(|error| error, |profile| runs(&profile)) }
}

fn row(key: char, label: &str, about: String) -> Choice {
    Choice { key, act: None, label: label.to_string(), about, link: None, run: None }
}

fn facts(item: &Item) -> String {
    let mut facts = Vec::new();
    if let Some(git) = &item.git {
        let commits = if git.ahead == 1 { "1 commit".to_string() } else { format!("{} commits", git.ahead) };
        facts.push(format!("{commits} since {}", git.upstream.trim_start_matches("origin/")));
    }
    facts.extend(item.assessment.as_ref().map(|word| format!("assessor: {}", word.text().replacen(": ", ", ", 1))));
    facts.join(" · ")
}

// While it is in review the first answer takes it out, so v twice withdraws as v once did; s and o come only with a second opinion set.
pub(super) fn review_menu(item: &Item, default: &Named, second: Option<(&Named, bool)>) -> ReviewMenu {
    let mut choices = Vec::new();
    if item.awaiting_review() {
        choices.push(row('v', "Withdraw from review", "stop its reviewer and forget its verdict: push is offered again, and the lead is told".to_string()));
    } else {
        let then = second.filter(|(_, always)| *always).map_or_else(String::new, |(second, _)| format!(", then {} (review.second runs it every time)", second.name));
        choices.push(row('v', "Review", format!("{} · {}{then}", default.name, default.runs)));
        if let Some((second, false)) = second {
            choices.push(row('s', "Review with a second opinion", format!("{}, then {} · {}", default.name, second.name, second.runs)));
        }
    }
    if let Some((second, _)) = second {
        choices.push(row('o', "Second opinion only", format!("{}; the push still waits on the {}", second.name, default.name)));
    }
    let second_file = item.second.as_ref().filter(|shown| shown.points.is_some()).map(|shown| shown.file.clone());
    if let Some(shown) = item.second.as_ref().filter(|shown| shown.points.is_some()) {
        choices.push(row('i', "Read the second opinion", format!("{}'s {}, in the pager", shown.by, review::points_text(shown.points.unwrap_or_default()))));
    }
    let title: String = item.name.chars().take(52).collect();
    ReviewMenu { title: format!("review · {title}"), facts: facts(item), choices, index: 0, path: item.path.clone().unwrap_or_default(), second_file }
}

pub(super) fn menu_lines(menu: &ReviewMenu) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if !menu.facts.is_empty() {
        lines.push(Line::from(Span::styled(menu.facts.clone(), style(MUTED))));
        lines.push(Line::default());
    }
    lines.extend(options_lines(menu.choices.clone(), menu.index, WIDTH));
    lines
}

pub(super) fn draw_review_menu(frame: &mut Frame, menu: &ReviewMenu) -> Vec<(Rect, Hit)> {
    let top = if menu.facts.is_empty() { 0 } else { 2 };
    let area = modal(frame, menu.title.clone(), menu_lines(menu), WIDTH as u16 + 4);
    (0..menu.choices.len()).map(|choice| (Rect::new(area.x, area.y + 1 + top + 2 * choice as u16, area.width, 2), Hit::Choice(choice))).filter(|(rect, _)| rect.y + 1 < area.bottom()).collect()
}

fn reviewers(item: &Item) -> (Named, Option<(Named, bool)>) {
    let settings = super::space::settings_of(item);
    let dir = profiles::dir();
    let default_name = settings.as_ref().map_or_else(|| profiles::REVIEWER.to_string(), |settings| settings.review_profile.value.clone());
    let default = match &settings {
        Some(settings) => named(&default_name, review::default_reviewer(&dir, settings)),
        None => named(&default_name, profiles::built_in(profiles::REVIEWER).ok_or_else(|| "the built-in reviewer is missing".to_string())),
    };
    let second = settings.as_ref().map(|settings| settings.review_second.value.clone()).unwrap_or_default();
    let second = match &second {
        Second::None => None,
        Second::Offered(name) | Second::Always(name) => Some((named(name, profiles::for_review(&dir, name)), second.always())),
    };
    (default, second)
}

impl App {
    pub(super) fn open_review_menu(&mut self, item: &Item) {
        let (default, second) = reviewers(item);
        self.mode = Mode::ReviewMenu(Box::new(review_menu(item, &default, second.as_ref().map(|(named, always)| (named, *always)))));
    }

    pub(super) fn handle_review_menu(&mut self, mut menu: Box<ReviewMenu>, act: Option<Act>, key: KeyEvent, terminal: Option<&mut DefaultTerminal>) {
        let typed = match key.code {
            KeyCode::Char(character) => menu.choices.iter().position(|choice| choice.key == character),
            _ => None,
        };
        match (typed, act) {
            (Some(at), _) => self.pick_review(&menu, at, terminal),
            (None, Some(Act::Move)) if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) => {
                menu.index = menu.index.saturating_sub(1);
                self.mode = Mode::ReviewMenu(menu);
            }
            (None, Some(Act::Move)) => {
                menu.index = (menu.index + 1).min(menu.choices.len().saturating_sub(1));
                self.mode = Mode::ReviewMenu(menu);
            }
            (None, Some(Act::Options)) => self.pick_review(&menu, menu.index, terminal),
            (None, Some(Act::Back | Act::Close)) => {}
            _ => self.mode = Mode::ReviewMenu(menu),
        }
    }

    fn review_in_background(&mut self, path: PathBuf, doing: &str, step: impl FnOnce(&std::path::Path) -> crate::sh::Result<String> + Send + 'static) {
        let name = self.name_at(&path);
        self.about("review", &name);
        let held = path.clone();
        self.task_in_background(&name, Some(held.as_path()), doing, format!("{name}: {doing}"), move || step(&path));
    }

    pub(super) fn pick_review(&mut self, menu: &ReviewMenu, at: usize, terminal: Option<&mut DefaultTerminal>) {
        let Some(choice) = menu.choices.get(at) else { return };
        let path = menu.path.clone();
        self.mode = Mode::Normal;
        match (choice.key, choice.label.as_str()) {
            ('v', "Withdraw from review") => self.review_in_background(path, "withdrawing it from review", move |path| review::withdraw(path, None)),
            ('v', _) => self.review_in_background(path, "sending it to review", move |path| review::request(path, None, Request::Review)),
            ('s', _) => self.review_in_background(path, "sending it to review", move |path| review::request(path, None, Request::WithSecond)),
            ('o', _) => self.review_in_background(path, "asking a second opinion", move |path| review::request(path, None, Request::SecondOnly)),
            ('i', _) => match (menu.second_file.clone(), terminal) {
                (Some(file), Some(terminal)) => {
                    if let Err(error) = super::terminal::outside(terminal, || crate::settings_page::show_in_pager(&file)) {
                        self.show(Err(format!("the pager: {error}")));
                    }
                }
                (Some(file), None) => self.show(Notice::new(Tone::Done, "The second opinion").next("No terminal to page it in here.").detail("its points", file.display().to_string()).ok()),
                (None, _) => {}
            },
            _ => {}
        }
    }
}

// The task's panel names a second opinion while it reads; its points reach the lead as messages.
pub(super) fn second_part(item: &Item) -> Option<String> {
    let shown = item.second.as_ref()?;
    let by = if shown.by.is_empty() { "second opinion".to_string() } else { shown.by.clone() };
    shown.running.then(|| format!("second opinion: {by} reading…"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::test_support::*;
    use crate::review::SecondShown;

    fn billing() -> Item {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 3);
        item.name = "billing-vat".to_string();
        item.path = Some(PathBuf::from("/work/wt1.app"));
        item.assessment = Some(crate::assessment::Word::Suggested { reason: "touches money".to_string() });
        item
    }

    fn reviewer() -> Named {
        Named { name: "reviewer".to_string(), runs: "claude · opus · max".to_string() }
    }

    fn codex_review() -> Named {
        Named { name: "codex-review".to_string(), runs: "codex · gpt-6.1-sol".to_string() }
    }

    fn keys(menu: &ReviewMenu) -> String {
        menu.choices.iter().map(|choice| choice.key).collect()
    }

    fn drawn(menu: ReviewMenu) -> String {
        let mut app = test_app(vec![billing()]);
        app.mode = Mode::ReviewMenu(Box::new(menu));
        let (rows, _) = render(&mut app, 100, 30);
        rows.into_iter().filter(|row| row.contains('│') || row.contains('╭') || row.contains('╰')).map(|row| row.trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn v_offers_the_default_reviewer_and_a_second_opinion_only_when_review_second_names_one() {
        let alone = review_menu(&billing(), &reviewer(), None);
        assert_eq!((keys(&alone), alone.title.as_str()), ("v".to_string(), "review · billing-vat"));
        assert_eq!(alone.facts, "3 commits since main · assessor: review suggested, touches money");
        assert_eq!(alone.choices[0].about, "reviewer · claude · opus · max");
        let offered = review_menu(&billing(), &reviewer(), Some((&codex_review(), false)));
        assert_eq!(keys(&offered), "vso");
        assert_eq!(offered.choices[1].about, "reviewer, then codex-review · codex · gpt-6.1-sol");
        assert_eq!(offered.choices[2].about, "codex-review; the push still waits on the reviewer");
        let shown = drawn(offered);
        for line in ["╭ review · billing-vat", "│ 3 commits since main · assessor: review suggested, touches money", "│ ▸ Review", "│   Review with a second opinion", "│   Second opinion only", "│ ↑ ↓ choose · ⏎ run · esc close"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        let always = review_menu(&billing(), &reviewer(), Some((&codex_review(), true)));
        assert_eq!(keys(&always), "vo", "review.second = \"always …\" runs it with v, so s would say the same");
        assert!(always.choices[0].about.ends_with("then codex-review (review.second runs it every time)"), "{}", always.choices[0].about);
    }

    #[test]
    fn a_task_in_review_gets_withdraw_as_the_first_answer_so_v_twice_withdraws() {
        let mut item = billing();
        item.verdict = Some(crate::status::tests::in_review(false, 2, false, false));
        let menu = review_menu(&item, &reviewer(), Some((&codex_review(), false)));
        assert_eq!((keys(&menu), menu.choices[0].label.as_str()), ("vo".to_string(), "Withdraw from review"));
        let mut app = test_app(vec![billing()]);
        press(&mut app, KeyEvent::from(KeyCode::Char('v')));
        let Mode::ReviewMenu(opened) = &app.mode else { panic!("v opens the review menu") };
        assert_eq!((keys(opened), opened.choices[0].label.as_str()), ("v".to_string(), "Review"), "with no repo config: the built-in reviewer, no second opinion");
        press(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal), "esc closes it and sends nothing");
    }

    #[test]
    fn a_second_opinion_shows_on_the_tasks_row_while_it_reads_and_its_points_are_the_leads() {
        let mut item = billing();
        item.assessment = None;
        item.recipe = Some(crate::recipe::Recipe { name: "small-fix".to_string(), why: "one function".to_string(), by: "lead".to_string() });
        item.verdict = Some(crate::status::tests::in_review(true, 0, false, false));
        item.second = Some(SecondShown { by: "codex-review".to_string(), points: Some(2), running: false, stale: false, file: PathBuf::from("/state/second-opinions/billing@web.md") });
        assert!(!item.awaiting_review(), "its reviewer passed it; the second opinion holds nothing");
        assert_eq!(second_part(&item), None, "its points reach the lead");
        let mut app = test_app(vec![item.clone()]);
        let (rows, _) = render(&mut app, 240, 20);
        assert!(!rows.join("\n").contains("second opinion"), "{}", rows.join("\n"));
        let menu = review_menu(&item, &reviewer(), Some((&codex_review(), false)));
        assert_eq!((keys(&menu), menu.choices[3].about.as_str()), ("vsoi".to_string(), "codex-review's 2 points, in the pager"));
        item.second = Some(SecondShown { points: None, running: true, ..item.second.clone().unwrap() });
        assert_eq!(second_part(&item).as_deref(), Some("second opinion: codex-review reading…"));
        assert!(!review_menu(&item, &reviewer(), None).choices.iter().any(|choice| choice.key == 'i'), "nothing to read while it reads");
    }
}
