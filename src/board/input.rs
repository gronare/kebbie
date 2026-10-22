use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};
use ratatui::{DefaultTerminal, Frame};

use crate::backlog::{self, Entry};
use crate::peers;
use crate::repo;
use crate::sh::Result;
use crate::tasks;
use super::{App, Column, Mode};
use super::terminal::outside;
use super::widgets::{MAUVE, MUTED, TEXT, modal, style};

pub(super) enum Purpose {
    TaskName,
    TaskPrompt(String),
    Message(String, String),
    BacklogName(Entry),
    BacklogItem,
    BacklogPriority(String),
    Tidy(PathBuf),
}

impl App {
    pub(super) fn set_filter(&mut self, text: String) {
        let column = self.column;
        self.filter = Some((column, text));
        let length = match column { Column::Work => self.tasks.len(), Column::Backlog => self.backlog_len() };
        if let Some(first) = (0..length).find(|index| self.shown(column, *index)) {
            match column {
                Column::Work => self.work_selected = first,
                Column::Backlog => self.backlog_selected = first,
            }
        }
    }

    fn submit(&mut self, purpose: Purpose, text: String, terminal: Option<&mut DefaultTerminal>) {
        match purpose {
            Purpose::TaskName | Purpose::BacklogName(_) | Purpose::BacklogItem if text.is_empty() => {}
            Purpose::BacklogItem => {
                let label = "Priority: urgent, high, medium or low (empty for none)".to_string();
                self.mode = Mode::Input { purpose: Purpose::BacklogPriority(text), label, buffer: String::new() };
            }
            Purpose::BacklogPriority(item) => {
                let result = crate::backlog_file::add(Some(&text), &item);
                if result.is_ok() {
                    self.backlog = backlog::load();
                }
                self.show(result);
            }
            Purpose::BacklogName(entry) if entry.area.is_empty() => {
                self.show(Err(format!("\"{}\" names no area, so kelpie cannot tell which repo it is in; add the repo's folder name as (area) at the end of its line.", entry.title)));
            }
            Purpose::TaskName => {
                let label = "What should Claude do? (optional)".to_string();
                self.mode = Mode::Input { purpose: Purpose::TaskPrompt(text), label, buffer: String::new() };
            }
            Purpose::TaskPrompt(name) => {
                let repo = self.any_repo();
                let prompt = (!text.is_empty()).then_some(text);
                self.show(start_in_background(&repo, &name, prompt.as_deref(), None));
            }
            Purpose::BacklogName(entry) => {
                let prompt = backlog::task_prompt(&entry, &self.sources, crate::vault_cli::known_on());
                let known = self.repo_named(&entry.area);
                let located = || match terminal {
                    Some(terminal) => outside(terminal, || repo::locate(&entry.area)),
                    None => repo::locate(&entry.area),
                };
                match known.map_or_else(located, Ok) {
                    Ok(repo) => self.show(start_in_background(&repo, &text, Some(&prompt), Some(&entry.reference))),
                    Err(error) => self.show(Err(error)),
                }
            }
            Purpose::Tidy(path) => {
                let result = tasks::tidy(&path, None, &text);
                self.show(result);
            }
            Purpose::Message(_, _) if text.is_empty() => {}
            Purpose::Message(pane, name) => {
                let result = crate::typing::prompt(&pane, &text, &name).map(|_| format!("Sent to {name}.")).map_err(|refused| format!("Nothing was sent: {name}'s Claude {}; your message was:\n{text}", refused.reason()));
                self.show(result);
            }
        }
    }
}

// The board stays where it is while the task's worktree and Claude come up; the task shows under Work once its Space exists.
fn start_in_background(repo: &Path, name: &str, prompt: Option<&str>, backlog: Option<&str>) -> Result<String> {
    let dir = crate::lead::state_dir()?.join("starts");
    crate::state::private_dir(&dir)?;
    let log = dir.join(format!("{}.log", name.replace('/', "-")));
    let output = crate::state::create(&log).map_err(|error| format!("{}: {error}", log.display()))?;
    let errors = output.try_clone().map_err(|error| error.to_string())?;
    let mut command = Command::new(peers::kelpie_command()?);
    command.arg("-C").arg(repo).args(["start", name, "--no-focus"]);
    if let Some(item) = backlog.filter(|item| !item.is_empty()) {
        command.args(["--backlog", item]);
    }
    if let Some(prompt) = prompt {
        command.arg(prompt);
    }
    // Its own process group, so closing the board's popup does not take the start down with it.
    command.stdin(std::process::Stdio::null()).stdout(output).stderr(errors).process_group(0);
    crate::sh::reap(command.spawn().map_err(|error| format!("kelpie start: {error}"))?);
    Ok(format!("Starting {name} in the background; it shows under Work in a moment (output in {}).", log.display()))
}

impl App {
    pub(super) fn handle_filter(&mut self, mut buffer: String, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.filter = None,
            KeyCode::Enter => {}
            KeyCode::Backspace => {
                buffer.pop();
                self.set_filter(buffer.clone());
                self.mode = Mode::Filter { buffer };
            }
            KeyCode::Char(character) => {
                buffer.push(character);
                self.set_filter(buffer.clone());
                self.mode = Mode::Filter { buffer };
            }
            _ => self.mode = Mode::Filter { buffer },
        }
    }

    pub(super) fn handle_input(&mut self, purpose: Purpose, label: String, mut buffer: String, key: KeyEvent, terminal: Option<&mut DefaultTerminal>) {
        match key.code {
            KeyCode::Esc => {}
            KeyCode::Enter => self.submit(purpose, buffer.trim().to_string(), terminal),
            KeyCode::Backspace => {
                buffer.pop();
                self.mode = Mode::Input { purpose, label, buffer };
            }
            KeyCode::Char(character) => {
                buffer.push(character);
                self.mode = Mode::Input { purpose, label, buffer };
            }
            _ => self.mode = Mode::Input { purpose, label, buffer },
        }
    }
}

pub(super) fn draw_filter(frame: &mut Frame, app: &App, buffer: &str) {
    let column = if app.column == Column::Work { "work" } else { "backlog" };
    modal(frame, format!("filter {column}"), vec![
        Line::from(vec![Span::styled(buffer.to_string(), style(TEXT)), Span::styled("▏", style(MAUVE))]),
        Line::from(Span::styled("⏎ keep the filter · esc clear it", style(MUTED))),
    ], 72);
}

pub(super) fn draw_input(frame: &mut Frame, label: &str, buffer: &str) {
    modal(frame, label.to_string(), vec![
        Line::from(vec![Span::styled(buffer.to_string(), style(TEXT)), Span::styled("▏", style(MAUVE))]),
        Line::from(Span::styled("enter to continue · esc to cancel", style(MUTED))),
    ], 72);
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;

    use crate::board::test_support::*;

    #[test]
    fn slash_filters_the_selected_column_by_a_plain_substring_and_esc_clears_it() {
        let named = |name: &str| { let mut item = crate::status::tests::task(Some("idle"), None, 0); item.name = name.to_string(); item };
        let mut app = test_app(vec![named("billing"), named("search-filters"), named("refunds")]);
        app.handle_normal(key(KeyCode::Char('/')));
        for character in "FIL".chars() {
            press(&mut app, key(KeyCode::Char(character)));
        }
        press(&mut app, key(KeyCode::Enter));
        assert_eq!(app.selected_task().map(|item| item.name).as_deref(), Some("search-filters"));
        let (rows, _) = render(&mut app, 112, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("search-filters") && !screen.contains("billing") && !screen.contains("refunds") && screen.contains("1 task / FIL"), "{screen}");
        app.handle_normal(key(KeyCode::Down));
        assert_eq!(app.selected_task().map(|item| item.name).as_deref(), Some("search-filters"), "moving stays on the rows shown");
        app.handle_normal(key(KeyCode::Esc));
        assert!(app.filter.is_none());
        let (rows, _) = render(&mut app, 112, 40);
        assert!(rows.join("\n").contains("billing"));
    }

    #[test]
    fn a_task_started_from_the_board_runs_as_its_own_process_with_the_name_prompt_and_item_as_arguments() {
        let source = include_str!("input.rs");
        let start = &source[source.find("\nfn start_in_background(").unwrap()..];
        let body = &start[..start.find("\n}\n").unwrap()];
        assert!(body.contains(".arg(\"-C\").arg(repo)") && body.contains(".arg(prompt)") && body.contains("[\"--backlog\", item]"), "arguments go as argv, never through a shell: {body}");
        assert!(!body.contains("sh") || !body.contains("\"-c\""), "{body}");
        assert!(body.contains("process_group(0)"));
    }
}
