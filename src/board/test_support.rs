use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::Color;
use ratatui::layout::Size;
use ratatui_image::picker::Picker;

use crate::backlog::Backlog;
use crate::config::Source;
use crate::messages::{self, Message};
use crate::report;
use crate::status::Item;
use crate::tasks;
use super::detail::Detail;
use super::draw::draw;
use super::images::{Graphics, spawn_decoder};
use super::message_log::Fold;
use super::App;

pub(super) fn press(app: &mut App, event: KeyEvent) {
    app.handle_key(event, None);
}

pub(super) fn carded(current: bool) -> App {
    let mut app = test_app(vec![served("search-filters")]);
    with_detail(&mut app);
    let text = "Filters show on the search page.\nPrice and size, both kept in the URL.\nA third line stays in the window.\n![the filters](shots/filters.png)\n![empty](shots/empty.png)".to_string();
    let card = report::Card { sha: "abcdef0123".to_string(), text, file: PathBuf::from("/state/cards/x.md") };
    std::rc::Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card = Some((card, current));
    app
}

pub(super) const QUESTION: &str = r#"{"id":"q1","at":10,"from":"billing@web","to":"lead","text":"Which queue for refund mails?","state":"asked","question":{"options":[{"label":"Sidekiq","meaning":"reuse the jobs"},{"label":"Solid Queue","meaning":"one less service"},{"label":"Inline","meaning":"no queue at all"}],"recommend":2}}"#;
pub(super) const PLAN: &str = r#"{"id":"q2","at":20,"from":"payload@api","to":"lead","text":"Approve this plan?","state":"asked","question":{"options":[{"label":"Approve","meaning":"build it as planned"},{"label":"Approve with a comment","meaning":"build it, taking the comment in"},{"label":"Ask for changes","meaning":"revise and ask again"},{"label":"Reject with a comment","meaning":"drop this approach"}],"recommend":1,"plan":"/state/plans/payload@api-20.md"}}"#;

pub(super) fn test_app(tasks: Vec<Item>) -> App {
    let (refresh, _) = mpsc::channel();
    let mut app = App::new(vec![Source::Local], Backlog { updated: None, entries: Vec::new() }, false, refresh);
    for item in &tasks {
        if let Some(agent) = &item.agent {
            app.screens.insert(agent.pane.clone(), (Instant::now(), String::new()));
        }
    }
    app.tasks = tasks;
    app.loaded = true;
    app.graphics = Graphics::Off;
    app.fold = Fold::Hidden;
    app
}

pub(super) fn served(name: &str) -> Item {
    let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
    item.name = name.to_string();
    item.server = true;
    item.url = Some("https://wt2.web.test".to_string());
    item.path = Some(PathBuf::from("/work/wt2.web"));
    item
}

pub(super) fn with_detail(app: &mut App) {
    let detail = Detail {
        branch: "search-filters".to_string(), changed: vec!["app/filters.rb".to_string()], dev_server: true, dev_worker: true, server_name: "server".to_string(), worker_name: "worker".to_string(),
        commits: vec![tasks::Commit { sha: "a1f3c2e0000".to_string(), message: "Wire the price filter".to_string() }], card: None, tests: None,
    };
    app.details.insert(PathBuf::from("/work/wt2.web"), std::rc::Rc::new(detail));
}

pub(super) fn render(app: &mut App, width: u16, height: u16) -> (Vec<String>, ratatui::buffer::Buffer) {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let rows = (0..height).map(|y| (0..width).map(|x| {
        let symbol = buffer[(x, y)].symbol();
        symbol.split("\x1b\\").nth(1).map_or(symbol, |inner| inner.split('\x1b').next().unwrap_or(inner)).to_string()
    }).collect::<String>()).collect();
    (rows, buffer)
}

pub(super) fn find(rows: &[String], text: &str) -> Vec<(usize, usize)> {
    rows.iter().enumerate().filter_map(|(y, row)| row.find(text).map(|byte| (row[..byte].chars().count(), y))).collect()
}

pub(super) fn some_messages(count: usize) -> Vec<Message> {
    let lines: Vec<String> = (0..count).map(|at| format!(r#"{{"id":"m{at}","at":{},"from":"task{at}@web","to":"lead","text":"note number {at}","state":"sent"}}"#, 100 + at)).collect();
    messages::fold(&lines.join("\n"))
}

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

pub(super) fn click(app: &mut App, x: u16, y: u16) -> Option<KeyEvent> {
    let hit = app.hit_at(x, y);
    app.work_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE }, hit)
}

pub(super) fn imaged(dir: &Path) -> (App, PathBuf, PathBuf) {
    std::fs::create_dir_all(dir).unwrap();
    let (wide, tall) = (dir.join("wide.png"), dir.join("tall.png"));
    image::RgbImage::from_pixel(40, 20, image::Rgb([200, 30, 30])).save(&wide).unwrap();
    image::RgbImage::from_pixel(20, 40, image::Rgb([30, 30, 200])).save(&tall).unwrap();
    let mut app = test_app(vec![served("search-filters")]);
    with_detail(&mut app);
    let text = format!("Refunds show.\n![wide]({})\n![tall]({})", wide.display(), tall.display());
    let card = report::Card { sha: "abcdef0123".to_string(), text, file: PathBuf::from("/state/cards/x.md") };
    std::rc::Rc::get_mut(app.details.get_mut(Path::new("/work/wt2.web")).unwrap()).unwrap().card = Some((card, true));
    app.graphics = Graphics::Ready(Picker::halfblocks());
    (app.image_requests, app.image_results) = spawn_decoder().map(|(requests, results)| (Some(requests), Some(results))).unwrap();
    for file in [&wide, &tall] {
        app.image_size(file, Size::new(10, 10));
    }
    let waited = Instant::now();
    while [&wide, &tall].iter().any(|file| app.decoded.get(*file).is_none_or(Option::is_none)) && waited.elapsed() < Duration::from_secs(5) {
        thread::sleep(Duration::from_millis(10));
        app.take_images();
    }
    app.handle_normal(key(KeyCode::Char('s')));
    (app, wide, tall)
}

pub(super) fn coloured(cell: &ratatui::buffer::Cell, (r, g, b): (u8, u8, u8)) -> bool {
    let near = |color: Color| matches!(color, Color::Rgb(red, green, blue) if red.abs_diff(r) < 8 && green.abs_diff(g) < 8 && blue.abs_diff(b) < 8);
    near(cell.fg) || near(cell.bg)
}

pub(super) fn three_questions() -> Vec<Message> {
    let log = ["a", "b", "c"].iter().enumerate().map(|(index, name)| {
        format!(r#"{{"id":"{name}","at":{at},"from":"{name}@web","to":"lead","text":"Question {name}?","state":"asked","question":{{"options":[{{"label":"One","meaning":"the first"}},{{"label":"Two","meaning":"the second"}}],"recommend":2}}}}"#, at = index + 1)
    }).collect::<Vec<_>>().join("\n");
    messages::fold(&log)
}

pub(super) const ESCAPES: &str = "\x1b]8;;https://evil.example\x1b\\click\x1b]8;;\x1b\\ \x1b[31mred\x1b[0m \x07 \u{9b}2J";

pub(super) fn no_escape_reaches_the_terminal(buffer: &ratatui::buffer::Buffer, what: &str) {
    let leaked: Vec<String> = buffer.content.iter().map(|cell| cell.symbol().to_string())
        .filter(|symbol| symbol.chars().any(|character| character.is_control() || character == '\u{9b}')).collect();
    assert!(leaked.is_empty(), "{what}: {leaked:?}");
}
