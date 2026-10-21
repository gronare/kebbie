use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use ratatui::DefaultTerminal;

use crate::area_page::Effect;
use crate::keys::{self, Act};
use super::message_log::Fold;
use super::{App, Column, Hit, Mode, Page};

impl App {
    pub(super) fn hit_at(&self, column: u16, row: u16) -> Option<Hit> {
        self.hits.iter().find(|(area, _)| area.contains(Position::new(column, row))).map(|(_, hit)| *hit)
    }

    // A click selects a row, a click on the selected row opens its options, and the wheel moves through whatever is in front.
    pub(super) fn mouse(&mut self, event: MouseEvent, terminal: &mut DefaultTerminal) {
        let hit = self.hit_at(event.column, event.row);
        if let (Some(Hit::Tab(page)), MouseEventKind::Down(MouseButton::Left)) = (hit, event.kind) {
            self.page = page;
            return;
        }
        if self.page == Page::Routines {
            let effect = self.routines.mouse(event, Some(terminal));
            return self.after_routines(effect, None);
        }
        if self.page == Page::Areas {
            if let Effect::Left = self.areas.mouse(event, terminal) {
                self.quit = self.overlay;
            }
            return;
        }
        if let Some(key) = self.work_mouse(event, hit) {
            self.handle(key, terminal);
        }
    }

    pub(super) fn work_mouse(&mut self, event: MouseEvent, hit: Option<Hit>) -> Option<KeyEvent> {
        let normal = matches!(self.mode, Mode::Normal);
        match event.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Mode::EditMessage(editor) = &mut self.mode {
                    editor.text.input(event);
                    return None;
                }
                if let Mode::Questions(window) = &mut self.mode {
                    window.scroll = if event.kind == MouseEventKind::ScrollUp { window.scroll.saturating_sub(3) } else { window.scroll.saturating_add(3) };
                    return None;
                }
                if let Mode::ConfirmPush { scroll, .. } = &mut self.mode {
                    *scroll = scroll.by(if event.kind == MouseEventKind::ScrollUp { -3 } else { 3 });
                    return None;
                }
                match hit {
                    Some(Hit::Task(_)) if normal => (self.column, self.in_messages) = (Column::Work, false),
                    Some(Hit::Entry(_)) if normal => (self.column, self.in_messages) = (Column::Backlog, false),
                    Some(Hit::Messages(_)) if normal && self.fold == Fold::Open => self.in_messages = true,
                    _ => {}
                }
                Some(KeyEvent::from(if event.kind == MouseEventKind::ScrollUp { KeyCode::Up } else { KeyCode::Down }))
            }
            MouseEventKind::Down(MouseButton::Left) => {
                match (hit, &mut self.mode) {
                    (Some(Hit::Task(index)), Mode::Normal) => {
                        let again = self.column == Column::Work && self.work_selected == index && !self.in_messages;
                        (self.column, self.work_selected, self.in_messages) = (Column::Work, index, false);
                        if again {
                            self.mode = Mode::Options(0);
                        }
                    }
                    (Some(Hit::Entry(index)), Mode::Normal) => {
                        let again = self.column == Column::Backlog && self.backlog_selected == index && !self.in_messages;
                        (self.column, self.backlog_selected, self.in_messages) = (Column::Backlog, index, false);
                        if again {
                            self.mode = Mode::Options(0);
                        }
                    }
                    (Some(Hit::Messages(None)), Mode::Normal) if self.fold == Fold::Open => self.in_messages = true,
                    (Some(Hit::Messages(None)), Mode::Normal) => self.press_m(),
                    (Some(Hit::Messages(Some(index))), Mode::Normal) => {
                        let back = self.messages.len().saturating_sub(index + 1);
                        let again = self.in_messages && self.message_back == back;
                        (self.in_messages, self.message_back) = (true, back);
                        if again {
                            return Some(KeyEvent::from(KeyCode::Enter));
                        }
                    }
                    (Some(Hit::Key(code)), Mode::Normal) => {
                        self.in_messages &= code == keys::map().code(Act::Messages);
                        return Some(KeyEvent::from(code));
                    }
                    (Some(Hit::Choice(index)), Mode::Options(_)) => {
                        if let Some(act) = self.choices().get(index).and_then(|choice| choice.act) {
                            self.mode = Mode::Normal;
                            self.act(act);
                        }
                    }
                    (Some(Hit::Commit(index)), Mode::ConfirmPush { selected, .. }) => *selected = index,
                    (Some(Hit::CardImage(index)), Mode::Card { image, .. }) => *image = index,
                    (Some(Hit::Question(index)), Mode::Questions(window)) => window.select(&self.questions, index),
                    (Some(Hit::Answer(index)), Mode::Questions(window)) => {
                        window.option = index;
                        window.typing = false;
                    }
                    (None, Mode::Options(_) | Mode::Message(_) | Mode::Help) => self.mode = Mode::Normal,
                    _ => {}
                }
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseEvent, MouseEventKind};

    use crate::board::test_support::*;
    use crate::board::message_log::Fold;
    use crate::board::Hit;

    #[test]
    fn the_mouse_hits_rows_messages_and_status_hints_where_they_are_drawn() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0), served("search-filters")]);
        app.messages = some_messages(3);
        app.fold = Fold::Line;
        let (rows, _) = render(&mut app, 112, 40);
        let (x, y) = find(&rows, "▸ messages")[0];
        for (hint, code) in [("⏎ options", KeyCode::Enter), ("i inbox", KeyCode::Char('i')), ("? keys", KeyCode::Char('?'))] {
            let (hx, hy) = find(&rows, hint).into_iter().last().unwrap();
            assert_eq!(app.hit_at(hx as u16, hy as u16), Some(Hit::Key(code)), "{hint}");
            assert_eq!(app.hit_at((hx + hint.chars().count() - 1) as u16, hy as u16), Some(Hit::Key(code)), "the end of {hint}");
            assert_eq!(click(&mut app, hx as u16, hy as u16), Some(key(code)));
        }
        assert_eq!(click(&mut app, x as u16, y as u16), None);
        assert!(app.fold == Fold::Open && app.in_messages, "a click on the messages line opens them, selected");
        let (rows, _) = render(&mut app, 112, 40);
        let (x, y) = find(&rows, "note number 0")[0];
        assert_eq!(app.hit_at(x as u16, y as u16), Some(Hit::Messages(Some(0))));
        assert_eq!(click(&mut app, x as u16, y as u16), None);
        assert_eq!(app.selected_message().unwrap().id, "m0");
        assert_eq!(click(&mut app, x as u16, y as u16), Some(key(KeyCode::Enter)), "a second click reads it");
        let (x, y) = find(&rows, "search-filters")[0];
        assert_eq!(app.hit_at(x as u16, y as u16), Some(Hit::Task(1)));
        click(&mut app, x as u16, y as u16);
        assert!(!app.in_messages && app.work_selected == 1, "a click on a task takes the selection back to it");
        let (rows, _) = render(&mut app, 112, 40);
        let (x, y) = find(&rows, "▾ messages  m selects")[0];
        click(&mut app, x as u16, y as u16);
        assert!(app.in_messages && app.fold == Fold::Open, "a click on the open panel's title goes into it");
        app.in_messages = false;
        let (x, y) = find(&rows, "note number 2")[0];
        let wheel = app.work_mouse(MouseEvent { kind: MouseEventKind::ScrollUp, column: x as u16, row: y as u16, modifiers: KeyModifiers::NONE }, app.hit_at(x as u16, y as u16));
        assert!(wheel == Some(key(KeyCode::Up)) && app.in_messages, "the wheel over the messages scrolls them");
    }
}
