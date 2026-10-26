use serde_json::Value;

use crate::herdr;
use crate::lead;
use crate::sh::Result;
use crate::status::Item;
use super::{App, Mode, Page};

#[derive(Debug, PartialEq)]
pub(super) enum Target {
    Lead,
    Task { workspace: String, pane: String },
    Nowhere,
}

// A terminal hands a dropped file to the focused pane as a paste of its path, so the Claude of the Space on screen is where it was meant to go.
pub(super) fn target(workspaces: &[Value], tasks: &[Item]) -> Target {
    let Some(shown) = workspaces.iter().find(|workspace| workspace["focused"] == true).and_then(|workspace| workspace["workspace_id"].as_str()) else {
        return Target::Nowhere;
    };
    if lead::lead_workspace(workspaces).as_deref() == Some(shown) {
        return Target::Lead;
    }
    tasks.iter().find(|item| item.workspace.as_deref() == Some(shown))
        .and_then(|item| Some(Target::Task { workspace: shown.to_string(), pane: item.agent.as_ref()?.pane.clone() }))
        .unwrap_or(Target::Nowhere)
}

impl App {
    // An open text field takes the paste; otherwise it comes back to be passed on, and never reaches the board as keys.
    pub(super) fn take_paste(&mut self, text: String) -> Option<String> {
        match &mut self.mode {
            Mode::Filter { buffer } => {
                buffer.push_str(&text);
                let buffer = buffer.clone();
                self.set_filter(buffer);
                None
            }
            Mode::Input { buffer, .. } => {
                buffer.push_str(&text);
                None
            }
            Mode::EditMessage(editor) => {
                editor.text.insert_str(&text);
                None
            }
            Mode::Hub(window) if window.typing() => {
                window.paste(&text);
                None
            }
            Mode::Questions(window) if window.typing => {
                window.comment.insert_str(&text);
                None
            }
            Mode::Normal if self.page == Page::Areas && self.areas.paste(&text) => None,
            Mode::Normal if self.page == Page::Routines && self.routines.paste(&text) => None,
            Mode::Normal if self.page == Page::Settings && self.settings.paste(&text) => None,
            _ => Some(text),
        }
    }

    pub(super) fn paste(&mut self, text: String) {
        let Some(text) = self.take_paste(text) else { return };
        let result = herdr::workspaces().and_then(|workspaces| self.pass_on(target(&workspaces, &self.tasks), &text));
        if let Err(error) = result {
            self.subject = Some(super::notices::Subject::plain("paste"));
            self.show(Err(error));
        }
    }

    fn pass_on(&mut self, target: Target, text: &str) -> Result<()> {
        match target {
            Target::Lead => lead::paste_into_chat(text)?,
            Target::Task { workspace, pane } => {
                herdr::paste(&pane, text)?;
                herdr::focus_workspace(&workspace)?;
                super::space::focus_pane(&pane)?;
            }
            Target::Nowhere => return crate::notice::Notice::new(crate::notice::Tone::NotNow, "The paste has nowhere to go").next("This Space has no Claude, and no text field is open on the board.").err(),
        }
        if self.overlay {
            self.quit = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::crossterm::event::KeyCode;
    use serde_json::json;

    use super::*;
    use super::super::Column;
    use super::super::editor::{MessageEditor, Rewrite};
    use super::super::input::Purpose;
    use super::super::test_support::{press, served, test_app};

    const DROPPED: &str = r"/var/folders/T/Screenshot\ 2026-10-22\ at\ 10.02.13.png";

    fn spaces(focused: &str) -> Vec<Value> {
        ["w_lead", "w_task", "w_other"].iter().map(|id| json!({ "workspace_id": id, "label": if *id == "w_lead" { "lead" } else { id }, "focused": *id == focused })).collect()
    }

    #[test]
    fn a_paste_on_the_idle_board_is_passed_on_and_never_read_as_keys() {
        let mut app = test_app(vec![served("one"), served("two")]);
        app.work_selected = 1;
        assert_eq!(app.take_paste(DROPPED.to_string()).as_deref(), Some(DROPPED));
        assert!(matches!(app.mode, Mode::Normal), "the leading / must not open the filter");
        assert!(app.filter.is_none() && app.work_selected == 1 && !app.quit);
    }

    #[test]
    fn an_open_text_field_takes_the_paste_unchanged() {
        let mut app = test_app(vec![served("one")]);
        press(&mut app, KeyCode::Char('/').into());
        assert_eq!(app.take_paste("one".to_string()), None);
        assert!(matches!(&app.mode, Mode::Filter { buffer } if buffer == "one"));
        assert!(app.filter.as_ref().is_some_and(|(column, text)| *column == Column::Work && text == "one"), "the pasted filter filters at once");

        app.mode = Mode::Input { purpose: Purpose::TaskPrompt("billing".to_string()), label: String::new(), buffer: "see ".to_string() };
        assert_eq!(app.take_paste(DROPPED.to_string()), None);
        assert!(matches!(&app.mode, Mode::Input { buffer, .. } if *buffer == format!("see {DROPPED}")));
    }

    #[test]
    fn the_commit_message_editor_takes_the_paste_at_its_cursor() {
        let mut app = test_app(Vec::new());
        app.mode = Mode::EditMessage(Box::new(MessageEditor::new(PathBuf::from("/work/wt2.web"), Vec::new(), (0, crate::board::push::Scroll::default()), (None, None), Rewrite::Squash, "")));
        assert_eq!(app.take_paste("Fix the filter".to_string()), None);
        let Mode::EditMessage(editor) = &app.mode else { panic!() };
        assert_eq!(editor.text.lines(), ["Fix the filter"]);
    }

    #[test]
    fn the_board_hands_a_paste_event_to_the_paste_handling() {
        let source = include_str!("mod.rs");
        assert!(source.contains("Event::Paste(text) => app.paste(text)"), "without it a paste is dropped by the event loop");
    }

    #[test]
    fn the_paste_goes_to_the_claude_of_the_space_on_screen() {
        let mut task = served("billing");
        task.workspace = Some("w_task".to_string());
        let tasks = [task.clone()];
        assert_eq!(target(&spaces("w_lead"), &tasks), Target::Lead);
        assert_eq!(target(&spaces("w_task"), &tasks), Target::Task { workspace: "w_task".to_string(), pane: task.agent.clone().unwrap().pane });
        assert_eq!(target(&spaces("w_other"), &tasks), Target::Nowhere, "a Space without a task's Claude takes no paste");
        let mut idle = task;
        idle.agent = None;
        assert_eq!(target(&spaces("w_task"), &[idle]), Target::Nowhere, "a task whose Claude is not running takes no paste");
    }
}
