use std::path::{Path, PathBuf};
use std::rc::Rc;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::backlog_file::{self, Filing};
use crate::config::Source;
use crate::peers;
use crate::repo::name_of;
use crate::sh::Result;
use super::columns::suggested_name;
use super::input::{StartWith, start_in_background};
use super::widgets::{MAUVE, MUTED, SURFACE, TEAL, TEXT, modal, style, wrap};
use super::{App, Mode};

pub(super) type Start = Rc<dyn Fn(&Path, &str, &str) -> Result<String>>;
pub(super) type File = Rc<dyn Fn(&[Source], &str, &str) -> Result<String>>;

// How a new task starts and how its description is filed, apart so tests start nothing.
pub(super) struct Io {
    pub start: Start,
    pub file: File,
}

impl Default for Io {
    fn default() -> Self {
        Io { start: Rc::new(start), file: Rc::new(file) }
    }
}

// The new-task dialog: the description, the repos it can go to, and the choice once ⏎ was pressed.
pub(super) struct NewTask {
    pub text: String,
    pub repos: Vec<PathBuf>,
    pub repo: usize,
    pub choice: Option<Choice>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Choice {
    Start,
    Backlog,
}

const HEAD: &str = "New · what do you want done?";

// The description started as a task, its first words kept as the row's title until the task's agent names it.
fn start(repo: &Path, name: &str, text: &str) -> Result<String> {
    let state = crate::lead::state_dir()?;
    crate::status::keep_title(&state, &peers::address(name, &name_of(repo)), &backlog_file::placeholder_title(text));
    start_in_background(repo, name, Some(text), &StartWith::default())
}

// The description filed as the phone files an idea: a draft under its first words that the lead writes out.
fn file(sources: &[Source], repo: &str, text: &str) -> Result<String> {
    let title = backlog_file::placeholder_title(text);
    backlog_file::file_item(sources, &Filing { priority: "", repo, line: &title, words: Some(text.trim()), body: None, by: "The user" }, false)
}

// A task name from the description's first words, made unique with -2, -3 and on against what is taken.
pub(super) fn task_name(text: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = Some(suggested_name(text)).filter(|name| !name.is_empty()).unwrap_or_else(|| "task".to_string());
    (1..).map(|count| if count == 1 { base.clone() } else { format!("{base}-{count}") }).find(|name| !taken(name)).expect("an unused name")
}

fn branch_exists(repo: &Path, name: &str) -> bool {
    crate::repo::git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")]).is_ok()
}

fn keyed(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), style(MAUVE))
}

fn said(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), style(MUTED))
}

fn option(letter: &str, label: &str, on: bool) -> Vec<Span<'static>> {
    let lit = if on { Style::new().bg(SURFACE) } else { Style::new() };
    vec![Span::styled(" ", lit), Span::styled(letter.to_string(), style(MAUVE).patch(lit)), Span::styled(format!(" {label} "), style(TEXT).patch(lit))]
}

pub(super) fn draw(frame: &mut Frame, new: &NewTask) {
    let width = 64usize.min(frame.area().width.saturating_sub(8) as usize);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for paragraph in new.text.split('\n') {
        let wrapped = wrap(paragraph, width, usize::MAX);
        lines.extend(if wrapped.is_empty() { vec![Line::default()] } else { wrapped.into_iter().map(|text| Line::from(Span::styled(text, style(TEXT)))).collect() });
    }
    if new.choice.is_none()
        && let Some(last) = lines.last_mut()
    {
        last.spans.push(Span::styled("▏", style(MAUVE)));
    }
    let repo = new.repos.get(new.repo).map(|repo| name_of(repo)).unwrap_or_default();
    let mut where_ = vec![said("repo "), Span::styled(repo, style(TEAL))];
    if new.choice.is_none() && new.repos.len() > 1 {
        where_.push(said(" · tab switches"));
    }
    lines.extend([Line::default(), Line::from(where_), Line::default()]);
    match new.choice {
        None => lines.push(Line::from(vec![keyed("⏎"), said(" continue · "), keyed("alt-⏎"), said(" new line · "), keyed("esc"), said(" cancel")])),
        Some(choice) => {
            let mut row = option("s", "Start now", choice == Choice::Start);
            row.push(Span::raw("  "));
            row.extend(option("b", "Add to the backlog", choice == Choice::Backlog));
            lines.extend([Line::from(row), Line::default()]);
            lines.push(Line::from(vec![keyed("← →"), said(" choose · "), keyed("⏎"), said(" do it · "), keyed("s"), said(" / "), keyed("b"), said(" directly · "), keyed("esc"), said(" back to the text")]));
        }
    }
    modal(frame, HEAD.to_string(), lines, width as u16 + 4);
}

impl App {
    // n on the Work page: the dialog opens on the selected task's repo, else the one a new task starts in today.
    pub(super) fn open_new_task(&mut self) {
        let first = self.any_repo();
        let mut repos = vec![first.clone()];
        let known = self.tasks.iter().filter_map(|item| item.repo.clone()).chain(self.checkouts.iter().filter_map(|checkout| self.repo_named(&checkout.name)));
        for repo in known {
            if !repos.contains(&repo) {
                repos.push(repo);
            }
        }
        self.mode = Mode::NewTask(Box::new(NewTask { text: String::new(), repos, repo: 0, choice: None }));
    }

    pub(super) fn handle_new_task(&mut self, mut new: Box<NewTask>, key: KeyEvent) {
        match new.choice {
            None => match key.code {
                KeyCode::Esc => return,
                KeyCode::Enter if key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) => new.text.push('\n'),
                KeyCode::Enter if !new.text.trim().is_empty() => new.choice = Some(Choice::Start),
                KeyCode::Tab if !new.repos.is_empty() => new.repo = (new.repo + 1) % new.repos.len(),
                KeyCode::BackTab if !new.repos.is_empty() => new.repo = (new.repo + new.repos.len() - 1) % new.repos.len(),
                KeyCode::Backspace => {
                    new.text.pop();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => new.text.push(character),
                _ => {}
            },
            Some(choice) => match key.code {
                KeyCode::Esc => new.choice = None,
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => new.choice = Some(if choice == Choice::Start { Choice::Backlog } else { Choice::Start }),
                KeyCode::Enter => return self.do_new_task(&new, choice),
                KeyCode::Char('s') => return self.do_new_task(&new, Choice::Start),
                KeyCode::Char('b') => return self.do_new_task(&new, Choice::Backlog),
                _ => {}
            },
        }
        self.mode = Mode::NewTask(new);
    }

    fn do_new_task(&mut self, new: &NewTask, choice: Choice) {
        let Some(repo) = new.repos.get(new.repo).cloned() else { return };
        let text = new.text.trim().to_string();
        match choice {
            Choice::Start => {
                let tasks: Vec<String> = self.tasks.iter().filter(|item| item.repo.as_ref() == Some(&repo)).map(|item| item.name.clone()).collect();
                let name = task_name(&text, |name| tasks.iter().any(|task| task == name) || branch_exists(&repo, name));
                self.subject = Some(super::notices::Subject { kind: "start".to_string(), title: name.clone(), task: false });
                let result = (self.new_io.start)(&repo, &name, &text);
                self.show(result);
            }
            Choice::Backlog => {
                let result = (self.new_io.file)(&self.sources, &name_of(&repo), &text);
                self.subject = Some(super::notices::Subject::plain("backlog"));
                if result.is_ok() {
                    self.backlog = crate::backlog::load();
                }
                self.show(result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::board::test_support::*;

    fn typed(app: &mut App, text: &str) {
        for character in text.chars() {
            press(app, KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
    }

    fn recorded(app: &mut App) -> Rc<RefCell<Vec<String>>> {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let (started, filed) = (seen.clone(), seen.clone());
        app.new_io = Io {
            start: Rc::new(move |repo, name, text| {
                started.borrow_mut().push(format!("start {} {name} {text}", repo.display()));
                crate::notice::Notice::new(crate::notice::Tone::Done, "Started").ok()
            }),
            file: Rc::new(move |_, repo, text| {
                filed.borrow_mut().push(format!("file {repo} {text}"));
                crate::notice::Notice::new(crate::notice::Tone::Done, "Added").ok()
            }),
        };
        seen
    }

    fn app_on(repo: &str) -> App {
        let mut item = crate::status::tests::task(Some("idle"), None, 0);
        item.name = "disk-warning".to_string();
        item.repo = Some(PathBuf::from(format!("/nowhere/{repo}")));
        test_app(vec![item])
    }

    #[test]
    fn n_opens_one_box_for_what_you_want_done_on_the_selected_tasks_repo_with_no_name_field() {
        let mut app = app_on("kebbie");
        press(&mut app, key(KeyCode::Char('n')));
        assert!(matches!(app.mode, Mode::NewTask(_)));
        typed(&mut app, "The disk warning says how much is free");
        press(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        typed(&mut app, "and which worktree is largest.");
        let (rows, _) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("New · what do you want done?") && screen.contains("The disk warning says how much is free") && screen.contains("and which worktree is largest."), "alt-⏎ makes a new line: {screen}");
        assert!(screen.contains("repo kebbie") && screen.contains("⏎ continue · alt-⏎ new line · esc cancel") && !screen.contains("name"), "{screen}");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal), "esc cancels");
    }

    #[test]
    fn enter_offers_start_now_or_the_backlog_and_s_starts_it_under_a_name_from_its_first_words() {
        let mut app = app_on("kebbie");
        let seen = recorded(&mut app);
        press(&mut app, key(KeyCode::Char('n')));
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::NewTask(new) if new.choice.is_none()), "nothing to start without a description");
        typed(&mut app, "Fix the disk warning on the board");
        press(&mut app, key(KeyCode::Enter));
        let (rows, _) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("s Start now") && screen.contains("b Add to the backlog") && screen.contains("← → choose · ⏎ do it · s / b directly · esc back to the text"), "{screen}");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(&app.mode, Mode::NewTask(new) if new.choice.is_none() && new.text == "Fix the disk warning on the board"), "esc goes back to the text");
        press(&mut app, key(KeyCode::Enter));
        press(&mut app, key(KeyCode::Char('s')));
        assert_eq!(seen.borrow().as_slice(), ["start /nowhere/kebbie fix-disk-warning-board Fix the disk warning on the board"]);
    }

    #[test]
    fn b_or_right_and_enter_files_it_in_the_backlog_and_tab_switches_the_repo() {
        let mut app = app_on("kebbie");
        app.checkouts = vec![crate::status::Checkout { name: "kebbie".to_string(), changed: 0, free_slots: 0, busy_slots: 1 }];
        app.tasks.push({ let mut other = app.tasks[0].clone(); other.name = "hub".to_string(); other.repo = Some(PathBuf::from("/nowhere/kebbie-hub")); other });
        let seen = recorded(&mut app);
        press(&mut app, key(KeyCode::Char('n')));
        typed(&mut app, "Rerun a flaky test once");
        press(&mut app, key(KeyCode::Tab));
        assert!(render(&mut app, 100, 30).0.join("\n").contains("repo kebbie-hub · tab switches"));
        press(&mut app, key(KeyCode::Enter));
        press(&mut app, key(KeyCode::Right));
        press(&mut app, key(KeyCode::Enter));
        assert_eq!(seen.borrow().as_slice(), ["file kebbie-hub Rerun a flaky test once"]);
    }

    #[test]
    fn a_task_name_is_a_short_ascii_slug_of_the_first_words_and_never_one_taken() {
        assert_eq!(task_name("Diskvarningen på boardet ska säga hur mycket som är ledigt", |_| false), "diskvarningen-pa-boardet-ska");
        assert_eq!(task_name("Fix the disk warning", |name| name == "fix-disk-warning"), "fix-disk-warning-2");
        assert_eq!(task_name("Fix the disk warning", |name| ["fix-disk-warning", "fix-disk-warning-2"].contains(&name)), "fix-disk-warning-3");
        assert_eq!(task_name("…!", |_| false), "task");
    }
}
