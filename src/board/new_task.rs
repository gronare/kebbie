use std::path::{Path, PathBuf};
use std::sync::Arc;

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
use super::widgets::{MAUVE, MUTED, RED, SURFACE, TEAL, TEXT, modal, style, wrap};
use super::{App, Mode};

pub(super) type Start = Arc<dyn Fn(&Path, &str, &str) -> Result<String> + Send + Sync>;
pub(super) type File = Arc<dyn Fn(&[Source], &str, &str) -> Result<String> + Send + Sync>;

// How a new task starts and how its description is filed, apart so tests start nothing.
pub(super) struct Io {
    pub start: Start,
    pub file: File,
}

impl Default for Io {
    fn default() -> Self {
        Io { start: Arc::new(start), file: Arc::new(file) }
    }
}

// The new-task dialog: the description, the repos it can go to, and the choice once ⏎ was pressed.
pub(super) struct NewTask {
    pub text: String,
    pub repos: Vec<PathBuf>,
    pub repo: usize,
    pub choice: Option<Choice>,
    pub backlog: bool,
    pub failed: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Choice {
    Start,
    Backlog,
}

const HEAD: &str = "New · what do you want done?";

fn start(repo: &Path, name: &str, text: &str) -> Result<String> {
    start_described(repo, name, text, None)
}

// The description started as a task, its first words kept as the row's title until the task's agent names it.
pub(crate) fn start_described(repo: &Path, name: &str, text: &str, device: Option<&str>) -> Result<String> {
    titled(&crate::lead::state_dir()?, repo, name, text, || start_in_background(repo, name, Some(text), &StartWith { device, ..StartWith::default() }))
}

fn titled(state: &Path, repo: &Path, name: &str, text: &str, start: impl FnOnce() -> Result<String>) -> Result<String> {
    let started = start()?;
    crate::status::keep_title(state, &peers::address(name, &name_of(repo)), &backlog_file::placeholder_title(text));
    Ok(started)
}

// The description filed as the phone files an idea: a draft under its first words that the lead writes out.
fn file(sources: &[Source], repo: &str, text: &str) -> Result<String> {
    let title = backlog_file::placeholder_title(text);
    backlog_file::file_item(sources, &Filing { priority: "", repo, line: &title, words: Some(text.trim()), body: None, by: "The user" }, false)
}

// A task name from the description's first words as kebbie start keeps it, made unique with -2, -3 and on against what is taken.
pub(crate) fn task_name(text: &str, taken: impl Fn(&str) -> bool) -> String {
    let words = suggested_name(text);
    let base = crate::tasks::slug(&words).or_else(|_| crate::tasks::slug(&format!("task {words}"))).unwrap_or_else(|_| "task".to_string());
    (1..).map(|count| numbered(&base, count)).find(|name| !taken(name)).expect("an unused name")
}

fn numbered(base: &str, count: usize) -> String {
    if count == 1 {
        return base.to_string();
    }
    let suffix = format!("-{count}");
    let kept: String = base.chars().take(40 - suffix.len()).collect();
    format!("{}{suffix}", kept.trim_end_matches('-'))
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

// The typed text wrapped as typed: every space stays, so one typed at the end shows at once, and a word longer than the line breaks inside it.
pub(super) fn typed_lines(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut rest: Vec<char> = paragraph.chars().collect();
        while rest.len() > width {
            let cut = rest[..=width].iter().rposition(|character| *character == ' ').filter(|at| *at > 0).map_or(width, |at| at + 1);
            out.push(rest.drain(..cut).collect());
        }
        out.push(rest.into_iter().collect());
    }
    out
}

const TEXT_ROWS: usize = 6;

pub(super) fn draw(frame: &mut Frame, new: &NewTask) {
    let width = 88usize.min(frame.area().width.saturating_sub(8) as usize);
    let mut lines: Vec<Line<'static>> = typed_lines(&new.text, width.saturating_sub(1)).into_iter().map(|text| Line::from(Span::styled(text, style(TEXT)))).collect();
    if new.choice.is_none()
        && let Some(last) = lines.last_mut()
    {
        last.spans.push(Span::styled("▏", style(MAUVE)));
    }
    while lines.len() < TEXT_ROWS {
        lines.push(Line::default());
    }
    let repo = new.repos.get(new.repo).map(|repo| name_of(repo)).unwrap_or_default();
    let mut where_ = vec![said("repo "), Span::styled(repo, style(TEAL))];
    if new.choice.is_none() && new.repos.len() > 1 {
        where_.push(said(" · tab switches"));
    }
    lines.extend([Line::default(), Line::from(where_), Line::default()]);
    if let Some(failed) = &new.failed {
        for line in failed.lines() {
            lines.extend(wrap(line, width, usize::MAX).into_iter().map(|text| Line::from(Span::styled(text, style(RED)))));
        }
        lines.push(Line::default());
    }
    match new.choice {
        None => lines.push(Line::from(vec![keyed("⏎"), said(" continue · "), keyed("shift-⏎"), said(" new line · "), keyed("esc"), said(" cancel")])),
        Some(choice) if new.backlog => {
            let mut row = option("s", "Start now", choice == Choice::Start);
            row.push(Span::raw("  "));
            row.extend(option("b", "Add to the backlog", choice == Choice::Backlog));
            lines.extend([Line::from(row), Line::default()]);
            lines.push(Line::from(vec![keyed("← →"), said(" choose · "), keyed("⏎"), said(" do it · "), keyed("s"), said(" / "), keyed("b"), said(" directly · "), keyed("esc"), said(" back to the text")]));
        }
        Some(_) => {
            lines.extend([Line::from(option("s", "Start now", true)), Line::default()]);
            lines.push(Line::from(vec![keyed("⏎"), said(" / "), keyed("s"), said(" start · "), keyed("esc"), said(" back to the text")]));
        }
    }
    modal(frame, HEAD.to_string(), lines, width as u16 + 4);
}

impl App {
    // n on the Work page: the dialog opens on the selected task's repo, else the one a new task starts in today.
    pub(super) fn open_new_task(&mut self) {
        self.space_repos(std::time::Duration::ZERO);
        let first = self.any_repo();
        let backlog = crate::config::filing(&self.sources).is_some();
        self.mode = Mode::NewTask(Box::new(NewTask { text: String::new(), repos: vec![first], repo: 0, choice: None, backlog, failed: None }));
        self.add_space_repos();
    }

    // The dialog opens at once; the repos of the board's checkouts join its list as herdr's Spaces are read.
    pub(super) fn add_space_repos(&mut self) {
        let checkouts: Vec<String> = self.checkouts.iter().map(|checkout| checkout.name.clone()).collect();
        let mut known: Vec<PathBuf> = self.tasks.iter().filter_map(|item| item.repo.clone()).collect();
        known.extend(checkouts.iter().filter_map(|name| self.repo_named(name)));
        let Mode::NewTask(new) = &mut self.mode else { return };
        for repo in known {
            if !new.repos.contains(&repo) {
                new.repos.push(repo);
            }
        }
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
                KeyCode::Esc => {
                    new.choice = None;
                    new.failed = None;
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Tab if new.backlog => new.choice = Some(if choice == Choice::Start { Choice::Backlog } else { Choice::Start }),
                KeyCode::Enter => return self.do_new_task(new, choice),
                KeyCode::Char('s') => return self.do_new_task(new, Choice::Start),
                KeyCode::Char('b') if new.backlog => return self.do_new_task(new, Choice::Backlog),
                _ => {}
            },
        }
        self.mode = Mode::NewTask(new);
    }

    fn do_new_task(&mut self, mut new: Box<NewTask>, choice: Choice) {
        let Some(repo) = new.repos.get(new.repo).cloned() else { return };
        let text = new.text.trim().to_string();
        let job = match choice {
            Choice::Start => {
                let tasks: Vec<String> = self.tasks.iter().filter(|item| item.repo.as_ref() == Some(&repo)).map(|item| item.name.clone()).collect();
                let (shown, start) = (task_name(&text, |name| tasks.iter().any(|task| task == name)), self.new_io.start.clone());
                self.subject = Some(super::notices::Subject { kind: "start".to_string(), title: shown.clone(), task: false });
                self.task_in_background(&shown, None, "starting", format!("Starting {shown}"), move || {
                    let name = task_name(&text, |name| tasks.iter().any(|task| task == name) || branch_exists(&repo, name));
                    start(&repo, &name, &text)
                })
            }
            Choice::Backlog => {
                self.subject = Some(super::notices::Subject::plain("backlog"));
                let (file, sources) = (self.new_io.file.clone(), self.sources.clone());
                Some(self.in_background(format!("Adding it to {}'s backlog", name_of(&repo)), move || file(&sources, &name_of(&repo), &text)))
            }
        };
        if let Some(job) = job {
            job.ask_again(move |error| {
                new.choice = Some(choice);
                new.failed = Some(crate::notice::Notice::read(error, true).to_string());
                Some(Mode::NewTask(new))
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::board::test_support::*;

    fn typed(app: &mut App, text: &str) {
        for character in text.chars() {
            press(app, KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
    }

    fn recorded(app: &mut App) -> Arc<Mutex<Vec<String>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (started, filed) = (seen.clone(), seen.clone());
        app.new_io = Io {
            start: Arc::new(move |repo, name, text| {
                started.lock().unwrap().push(format!("start {} {name} {text}", repo.display()));
                crate::notice::Notice::new(crate::notice::Tone::Done, "Started").ok()
            }),
            file: Arc::new(move |_, repo, text| {
                filed.lock().unwrap().push(format!("file {repo} {text}"));
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
        assert!(screen.contains("repo kebbie") && screen.contains("⏎ continue · shift-⏎ new line · esc cancel") && !screen.contains("name"), "{screen}");
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
        settle_jobs(&mut app);
        assert_eq!(seen.lock().unwrap().as_slice(), ["start /nowhere/kebbie fix-disk-warning-board Fix the disk warning on the board"]);
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
        settle_jobs(&mut app);
        assert_eq!(seen.lock().unwrap().as_slice(), ["file kebbie-hub Rerun a flaky test once"]);
    }

    fn failing(app: &mut App) {
        app.new_io = Io { start: Arc::new(|_, _, _| Err("kebbie start: no such repo".to_string())), file: Arc::new(|_, _, _| Err("the backlog file is read-only".to_string())) };
    }

    #[test]
    fn a_start_or_filing_that_fails_keeps_the_description_and_the_choice_and_says_why() {
        for (letter, choice, said) in [('s', Choice::Start, "no such repo"), ('b', Choice::Backlog, "read-only")] {
            let mut app = app_on("kebbie");
            failing(&mut app);
            press(&mut app, key(KeyCode::Char('n')));
            typed(&mut app, "Fix the disk warning");
            press(&mut app, key(KeyCode::Enter));
            press(&mut app, key(KeyCode::Char(letter)));
            settle_jobs(&mut app);
            assert!(matches!(&app.mode, Mode::NewTask(new) if new.text == "Fix the disk warning" && new.choice == Some(choice)), "{letter}: the dialog comes back");
            assert!(render(&mut app, 100, 30).0.join("\n").contains(said), "{letter}: the error shows in the dialog");
            press(&mut app, key(KeyCode::Esc));
            assert!(matches!(&app.mode, Mode::NewTask(new) if new.choice.is_none() && new.text == "Fix the disk warning"), "{letter}: esc goes back to the text");
        }
    }

    #[test]
    fn with_nowhere_to_file_the_dialog_offers_only_start_now() {
        let mut app = app_on("kebbie");
        app.sources = Vec::new();
        let seen = recorded(&mut app);
        press(&mut app, key(KeyCode::Char('n')));
        typed(&mut app, "Fix the disk warning");
        press(&mut app, key(KeyCode::Enter));
        let screen = render(&mut app, 100, 30).0.join("\n");
        assert!(screen.contains("s Start now") && !screen.contains("backlog") && screen.contains("⏎ / s start · esc back to the text"), "{screen}");
        press(&mut app, key(KeyCode::Char('b')));
        press(&mut app, key(KeyCode::Right));
        assert!(matches!(&app.mode, Mode::NewTask(new) if new.choice == Some(Choice::Start)) && seen.lock().unwrap().is_empty(), "b and → do nothing");
        press(&mut app, key(KeyCode::Enter));
        settle_jobs(&mut app);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_first_words_are_kept_as_the_title_only_once_the_task_started() {
        let state = crate::sh::test_dir("new-task-title");
        let repo = Path::new("/nowhere/kebbie");
        assert!(titled(&state, repo, "fix-disk", "Fix the disk warning", || Err("refused".to_string())).is_err());
        assert_eq!(crate::status::kept_title(&state, "fix-disk@kebbie"), None, "a refused start leaves no title");
        assert!(titled(&state, repo, "fix-disk", "Fix the disk warning", || Ok("started".to_string())).is_ok());
        assert!(crate::status::kept_title(&state, "fix-disk@kebbie").is_some());
    }

    #[test]
    fn the_box_shows_every_typed_space_at_once_and_wraps_long_words_inside_the_line() {
        assert_eq!(super::typed_lines("fix the ", 20), vec!["fix the ".to_string()], "a space typed last shows");
        assert_eq!(super::typed_lines("two  spaces", 20), vec!["two  spaces".to_string()]);
        assert_eq!(super::typed_lines("aaaa bbbb cccc", 10), vec!["aaaa bbbb ".to_string(), "cccc".to_string()]);
        assert_eq!(super::typed_lines("abcdefghijkl", 5), vec!["abcde".to_string(), "fghij".to_string(), "kl".to_string()]);
        assert_eq!(super::typed_lines("one\ntwo", 10), vec!["one".to_string(), "two".to_string()]);
        let mut app = app_on("/work/shop");
        app.open_new_task();
        typed(&mut app, "make it ");
        press(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        typed(&mut app, "x");
        let rows = render(&mut app, 140, 40).0.join("\n");
        assert!(rows.contains("make it ") && rows.contains("shift-⏎ new line"), "{rows}");
        let crate::board::Mode::NewTask(new) = &app.mode else { panic!("the dialog stays") };
        assert_eq!(new.text, "make it \nx");
    }

    #[test]
    fn a_task_name_is_a_short_ascii_slug_of_the_first_words_and_never_one_taken() {
        assert_eq!(task_name("Diskvarningen på boardet ska säga hur mycket som är ledigt", |_| false), "diskvarningen-pa-boardet-ska");
        assert_eq!(task_name("Fix the disk warning", |name| name == "fix-disk-warning"), "fix-disk-warning-2");
        assert_eq!(task_name("Fix the disk warning", |name| ["fix-disk-warning", "fix-disk-warning-2"].contains(&name)), "fix-disk-warning-3");
        assert_eq!(task_name("…!", |_| false), "task");
        assert_eq!(task_name("2fa login breaks", |_| false), "task-2fa-login-breaks", "a name kebbie start refuses for its leading digit");
        let long = task_name("Internationalisation localisation documentation reorganisation", |_| false);
        assert_eq!(crate::tasks::slug(&long).as_deref(), Ok(long.as_str()), "the name start keeps, never one it cuts at 40");
        let again = task_name("Internationalisation localisation documentation reorganisation", |name| name == long);
        assert!(again.ends_with("-2") && again.len() <= 40 && crate::tasks::slug(&again).as_deref() == Ok(again.as_str()), "{again}");
    }
}
