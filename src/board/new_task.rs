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
use super::widgets::{MAUVE, MUTED, RED, SURFACE, TEXT, modal, style, wrap};
use super::{App, Mode};

pub(super) type Start = Arc<dyn Fn(&Path, &str, &str) -> Result<String> + Send + Sync>;
pub(super) type File = Arc<dyn Fn(&[Source], &str, bool, &[String]) -> Result<String> + Send + Sync>;

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

// The new-task dialog: the description and the choice once ⏎ was pressed; `fallback` is the repo a start goes to when there is no backlog to write it out in.
pub(super) struct NewTask {
    pub text: String,
    pub fallback: PathBuf,
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

// The description filed as the phone files an idea, with no repo: a draft under its first words that the writer writes out and places in one of `repos`, started once written when `start` asks it.
fn file(sources: &[Source], text: &str, start: bool, repos: &[String]) -> Result<String> {
    let title = backlog_file::placeholder_title(text);
    backlog_file::file_item(sources, &Filing { priority: "", repo: "", line: &title, words: Some(text.trim()), body: None, by: "The user", start, repos }, false)
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

fn option(label: &str, on: bool) -> Vec<Span<'static>> {
    let lit = if on { Style::new().bg(SURFACE) } else { Style::new() };
    vec![Span::styled(format!(" {label} "), style(TEXT).patch(lit))]
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
    lines.push(Line::default());
    if let Some(failed) = &new.failed {
        for line in failed.lines() {
            lines.extend(wrap(line, width, usize::MAX).into_iter().map(|text| Line::from(Span::styled(text, style(RED)))));
        }
        lines.push(Line::default());
    }
    match new.choice {
        None => lines.push(Line::from(vec![keyed("⏎"), said(" continue · "), keyed("shift-⏎"), said(" new line · "), keyed("esc"), said(" cancel")])),
        Some(choice) if new.backlog => {
            let mut row = option("Add to the backlog", choice == Choice::Backlog);
            row.push(Span::raw("  "));
            row.extend(option("Start now", choice == Choice::Start));
            lines.extend([Line::from(row), Line::default()]);
            lines.push(Line::from(vec![keyed("← →"), said(" choose · "), keyed("⏎"), said(" do it · "), keyed("esc"), said(" or typing goes back to the text")]));
        }
        Some(_) => {
            lines.extend([Line::from(option("Start now", true)), Line::default()]);
            lines.push(Line::from(vec![keyed("⏎"), said(" start · "), keyed("esc"), said(" or typing goes back to the text")]));
        }
    }
    modal(frame, HEAD.to_string(), lines, width as u16 + 4);
}

impl App {
    // n on the Work page: herdr's Spaces are read meanwhile, so the repos the writer chooses from are known by ⏎.
    pub(super) fn open_new_task(&mut self) {
        self.space_repos(std::time::Duration::ZERO);
        let fallback = self.any_repo();
        let backlog = crate::config::filing(&self.sources).is_some();
        self.mode = Mode::NewTask(Box::new(NewTask { text: String::new(), fallback, choice: None, backlog, failed: None }));
    }

    // The repos the board knows, by folder name: its tasks', its checkouts' and herdr's Spaces'.
    fn known_repos(&mut self) -> Vec<String> {
        let mut all: Vec<String> = self.tasks.iter().filter_map(|item| item.repo.as_deref().map(name_of)).collect();
        all.extend(self.checkouts.iter().map(|checkout| checkout.name.clone()));
        all.extend(self.space_repos(std::time::Duration::ZERO).iter().map(|repo| name_of(repo)));
        let mut known: Vec<String> = Vec::new();
        for name in all {
            if !name.is_empty() && !known.contains(&name) {
                known.push(name);
            }
        }
        known
    }

    pub(super) fn handle_new_task(&mut self, mut new: Box<NewTask>, key: KeyEvent) {
        match new.choice {
            None => match key.code {
                KeyCode::Esc => return,
                KeyCode::Enter if key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) => new.text.push('\n'),
                KeyCode::Enter if !new.text.trim().is_empty() => new.choice = Some(if new.backlog { Choice::Backlog } else { Choice::Start }),
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
                // A ⏎ meant as a new line lands here where the terminal can't tell shift-⏎ from ⏎, so a letter must never pick a choice.
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    (new.choice, new.failed) = (None, None);
                    new.text.push(character);
                }
                KeyCode::Backspace => {
                    (new.choice, new.failed) = (None, None);
                    new.text.pop();
                }
                _ => {}
            },
        }
        self.mode = Mode::NewTask(new);
    }

    // With a backlog both choices file the text for the writer, which picks the repo; Start now also marks it to start once written. Without one, Start now starts it at once in the fallback repo.
    fn do_new_task(&mut self, mut new: Box<NewTask>, choice: Choice) {
        let text = new.text.trim().to_string();
        let job = match choice {
            _ if new.backlog => {
                self.subject = Some(super::notices::Subject::plain("backlog"));
                let (file, sources, repos, start) = (self.new_io.file.clone(), self.sources.clone(), self.known_repos(), choice == Choice::Start);
                let label = if start { "Writing it out to start it" } else { "Adding it to the backlog" };
                Some(self.in_background(label.to_string(), move || file(&sources, &text, start, &repos)))
            }
            Choice::Start => {
                let repo = new.fallback.clone();
                let tasks: Vec<String> = self.tasks.iter().filter(|item| item.repo.as_ref() == Some(&repo)).map(|item| item.name.clone()).collect();
                let (shown, start) = (task_name(&text, |name| tasks.iter().any(|task| task == name)), self.new_io.start.clone());
                self.subject = Some(super::notices::Subject { kind: "start".to_string(), title: shown.clone(), task: false });
                self.task_in_background(&shown, None, "starting", format!("Starting {shown} in {}", name_of(&repo)), move || {
                    let name = task_name(&text, |name| tasks.iter().any(|task| task == name) || branch_exists(&repo, name));
                    start(&repo, &name, &text)
                })
            }
            Choice::Backlog => None,
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
            file: Arc::new(move |_, text, start, repos| {
                filed.lock().unwrap().push(format!("file{} [{}] {text}", if start { " to start" } else { "" }, repos.join(", ")));
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
    fn n_opens_one_box_for_what_you_want_done_with_no_name_or_repo_field() {
        let mut app = app_on("kebbie");
        press(&mut app, key(KeyCode::Char('n')));
        assert!(matches!(app.mode, Mode::NewTask(_)));
        typed(&mut app, "The disk warning says how much is free");
        press(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        typed(&mut app, "and which worktree is largest.");
        let (rows, _) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("New · what do you want done?") && screen.contains("The disk warning says how much is free") && screen.contains("and which worktree is largest."), "alt-⏎ makes a new line: {screen}");
        assert!(screen.contains("⏎ continue · shift-⏎ new line · esc cancel") && !screen.contains("name") && !screen.contains("repo kebbie") && !screen.contains("tab switches"), "{screen}");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal), "esc cancels");
    }

    #[test]
    fn enter_offers_the_backlog_first_or_start_now_which_files_it_for_the_writer_to_start_once_written() {
        let mut app = app_on("kebbie");
        let seen = recorded(&mut app);
        press(&mut app, key(KeyCode::Char('n')));
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.mode, Mode::NewTask(new) if new.choice.is_none()), "nothing to start without a description");
        typed(&mut app, "Fix the disk warning on the board");
        press(&mut app, key(KeyCode::Enter));
        let (rows, _) = render(&mut app, 100, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("Add to the backlog    Start now") && screen.contains("← → choose · ⏎ do it · esc or typing goes back to the text"), "{screen}");
        assert!(matches!(&app.mode, Mode::NewTask(new) if new.choice == Some(Choice::Backlog)), "the backlog is chosen first");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(&app.mode, Mode::NewTask(new) if new.choice.is_none() && new.text == "Fix the disk warning on the board"), "esc goes back to the text");
        press(&mut app, key(KeyCode::Enter));
        press(&mut app, key(KeyCode::Right));
        press(&mut app, key(KeyCode::Enter));
        settle_jobs(&mut app);
        assert_eq!(seen.lock().unwrap().as_slice(), ["file to start [kebbie] Fix the disk warning on the board"], "no task starts before the item is written");
    }

    #[test]
    fn enter_twice_files_it_in_the_backlog_with_the_repos_the_board_knows_for_the_writer() {
        let mut app = app_on("kebbie");
        app.checkouts = vec![crate::status::Checkout { name: "kebbie".to_string(), changed: 0, free_slots: 0, busy_slots: 1 }];
        app.tasks.push({ let mut other = app.tasks[0].clone(); other.name = "hub".to_string(); other.repo = Some(PathBuf::from("/nowhere/kebbie-hub")); other });
        let seen = recorded(&mut app);
        press(&mut app, key(KeyCode::Char('n')));
        typed(&mut app, "Rerun a flaky test once");
        press(&mut app, key(KeyCode::Tab));
        assert!(matches!(&app.mode, Mode::NewTask(new) if new.text == "Rerun a flaky test once"), "tab switches nothing");
        press(&mut app, key(KeyCode::Enter));
        press(&mut app, key(KeyCode::Enter));
        settle_jobs(&mut app);
        assert_eq!(seen.lock().unwrap().as_slice(), ["file [kebbie, kebbie-hub] Rerun a flaky test once"]);
    }

    #[test]
    fn typing_on_after_a_plain_enter_keeps_typing_and_never_starts_a_task() {
        for backlog in [true, false] {
            let mut app = app_on("kebbie");
            if !backlog {
                app.sources = Vec::new();
            }
            let seen = recorded(&mut app);
            press(&mut app, key(KeyCode::Char('n')));
            typed(&mut app, "We are working to get");
            press(&mut app, key(KeyCode::Enter));
            typed(&mut app, "some banners");
            assert!(matches!(&app.mode, Mode::NewTask(new) if new.choice.is_none() && new.text == "We are working to getsome banners"), "backlog {backlog}: the letters are typed");
            assert!(seen.lock().unwrap().is_empty(), "backlog {backlog}: nothing started");
            if backlog {
                press(&mut app, key(KeyCode::Enter));
                press(&mut app, key(KeyCode::Enter));
                settle_jobs(&mut app);
                assert_eq!(seen.lock().unwrap().as_slice(), ["file [kebbie] We are working to getsome banners"], "two plain ⏎ file it, never start it");
            }
        }
    }

    fn failing(app: &mut App) {
        app.new_io = Io { start: Arc::new(|_, _, _| Err("kebbie start: no such repo".to_string())), file: Arc::new(|_, _, _, _| Err("the backlog file is read-only".to_string())) };
    }

    #[test]
    fn a_start_or_filing_that_fails_keeps_the_description_and_the_choice_and_says_why() {
        for (moves, choice, said, backlog) in [(1, Choice::Start, "read-only", true), (0, Choice::Backlog, "read-only", true), (0, Choice::Start, "no such repo", false)] {
            let mut app = app_on("kebbie");
            if !backlog {
                app.sources = Vec::new();
            }
            failing(&mut app);
            press(&mut app, key(KeyCode::Char('n')));
            typed(&mut app, "Fix the disk warning");
            press(&mut app, key(KeyCode::Enter));
            for _ in 0..moves {
                press(&mut app, key(KeyCode::Right));
            }
            press(&mut app, key(KeyCode::Enter));
            settle_jobs(&mut app);
            let letter = format!("{choice:?}");
            assert!(matches!(&app.mode, Mode::NewTask(new) if new.text == "Fix the disk warning" && new.choice == Some(choice)), "{letter}: the dialog comes back");
            assert!(render(&mut app, 100, 30).0.join("\n").contains(said), "{letter}: the error shows in the dialog");
            press(&mut app, key(KeyCode::Esc));
            assert!(matches!(&app.mode, Mode::NewTask(new) if new.choice.is_none() && new.text == "Fix the disk warning"), "{letter}: esc goes back to the text");
        }
    }

    #[test]
    fn with_nowhere_to_file_the_dialog_offers_only_start_now_and_starts_at_once_in_the_selected_tasks_repo() {
        let mut app = app_on("kebbie");
        app.sources = Vec::new();
        let seen = recorded(&mut app);
        press(&mut app, key(KeyCode::Char('n')));
        typed(&mut app, "Fix the disk warning");
        press(&mut app, key(KeyCode::Enter));
        let screen = render(&mut app, 100, 30).0.join("\n");
        assert!(screen.contains("Start now") && !screen.contains("backlog") && screen.contains("⏎ start · esc or typing goes back to the text"), "{screen}");
        press(&mut app, key(KeyCode::Right));
        assert!(matches!(&app.mode, Mode::NewTask(new) if new.choice == Some(Choice::Start)) && seen.lock().unwrap().is_empty(), "→ does nothing");
        press(&mut app, key(KeyCode::Enter));
        settle_jobs(&mut app);
        assert_eq!(seen.lock().unwrap().as_slice(), ["start /nowhere/kebbie fix-disk-warning Fix the disk warning"]);
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
