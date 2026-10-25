use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Instant;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Size;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::config::DEFAULT_HUB_URL;
use crate::hub::card::{self, Copied, Foot, Row, Tone};
use crate::hub::{Sent, board_line, connect, devices, host, identity, qr, relay};
use crate::sh::Result;
use super::widgets::{GREEN, MAUVE, MUTED, PEACH, RED, SURFACE, TEXT, YELLOW, bold, modal, style, text_width};
use super::{App, Mode};

pub(super) type Post = fn() -> Result<()>;
pub(super) type Copy = fn(&str) -> Result<Copied>;

const WIDTH: u16 = 88;

#[derive(Clone)]
pub(super) struct Paths {
    pub(super) state: PathBuf,
    pub(super) config: PathBuf,
    pub(super) post: Post,
    pub(super) copy: Copy,
    pub(super) opener: Option<&'static str>,
    pub(super) glyphs: qr::Glyphs,
}

#[derive(Clone)]
pub(super) struct Waiting {
    pub(super) link: String,
    pub(super) code: String,
    pub(super) name: String,
    pub(super) url: String,
    pub(super) opened: bool,
    pub(super) expires: Instant,
}

pub(super) enum Event {
    Shown(Waiting),
    Connected(Result<Vec<String>>),
    Done(Result<String>),
}

pub(super) struct Flow {
    pub(super) title: &'static str,
    pub(super) waiting: String,
    pub(super) card: Option<Waiting>,
    pub(super) copied: Option<Copied>,
    events: Receiver<Event>,
    stop: Arc<AtomicBool>,
}

impl Drop for Flow {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub(super) enum View {
    Choose { configured: Option<String>, selected: usize, error: Option<String> },
    Own { buffer: String, tested: Option<String>, testing: Option<Receiver<Result<String>>>, error: Option<String> },
    Name { url: String, note: Option<String>, buffer: String },
    Running(Flow),
    Overview { leaving: bool },
    Rename { buffer: String, error: Option<String> },
    Said(String),
}

pub(super) struct Window {
    pub(super) paths: Paths,
    pub(super) view: View,
    pub(super) told: Vec<String>,
}

fn spawn(title: &'static str, waiting: String, step: impl FnOnce(&Sender<Event>, &(dyn Fn() -> bool + Sync)) -> Event + Send + 'static) -> Flow {
    let (sender, events) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    thread::spawn(move || {
        let stop = move || stopped.load(Ordering::Relaxed);
        let last = step(&sender, &stop);
        let _ = sender.send(last);
    });
    Flow { title, waiting, card: None, copied: None, events, stop }
}

const CHOICES: [(&str, &str); 2] = [("k", "kebbie hub"), ("o", "your own hub")];

impl Window {
    pub(super) fn open(paths: Paths) -> Window {
        let view = match identity::load(&paths.state) {
            Ok(Some(_)) => View::Overview { leaving: false },
            Ok(None) => match connect::configured_hub(&paths.config) {
                Ok(configured) => View::Choose { configured, selected: 0, error: None },
                Err(error) => View::Said(error),
            },
            Err(error) => View::Said(error),
        };
        Window { paths, view, told: Vec::new() }
    }

    fn configured_name(&self) -> Option<String> {
        crate::config::load_machine(&self.paths.config).ok()?.machine_name
    }

    fn name_or_start(&mut self) {
        let configured = self.configured_name().is_some();
        let View::Name { url, note, buffer } = &mut self.view else { return };
        if configured {
            let (url, note) = (url.clone(), note.take());
            return self.start(&url, None, note);
        }
        if buffer.is_empty() {
            *buffer = crate::config::short_hostname();
        }
    }

    fn chosen(&mut self, index: usize) {
        self.view = match index {
            0 => View::Name { url: DEFAULT_HUB_URL.to_string(), note: None, buffer: String::new() },
            _ => View::Own { buffer: String::new(), tested: None, testing: None, error: None },
        };
        self.name_or_start();
    }

    fn start(&mut self, url: &str, given: Option<String>, note: Option<String>) {
        let paths = self.paths.clone();
        let prepared = match connect::prepare(&paths.state, &paths.config, given.as_deref(), None) {
            Ok(prepared) => prepared,
            Err(error) => return self.view = View::Said(error),
        };
        let url = url.to_string();
        let waiting = format!("Asking {} for a code…", host(&url));
        self.view = View::Running(spawn("kebbie hub · connect", waiting, move |events, stop| {
            let now = crate::messages::now();
            let name = prepared.named.name().to_string();
            let result = (|| {
                let mut pending = connect::start(&url, &name)?;
                let _ = events.send(Event::Shown(Waiting { link: pending.link(), code: pending.user_code().to_string(), name: name.clone(), url: pending.url.clone(), opened: false, expires: pending.expires() }));
                let connected = pending.wait(&mut connect::sleep_unless(Some(stop)))?;
                let finished = connect::finish(&paths.state, &pending, &connected, Some(stop), paths.post, now)?;
                let lines = [note, finished.remark, connect::name_line(&paths.config, prepared, &name)];
                Ok(lines.into_iter().flatten().collect())
            })();
            Event::Connected(result)
        }));
    }

    fn leave(&mut self) {
        let state = self.paths.state.clone();
        self.view = View::Running(spawn("kebbie hub · leave the hub", "Taking your devices off and forgetting the keys…".to_string(), move |_, _| Event::Done(crate::hub::leave(&state))));
    }

    pub(super) fn poll(&mut self) {
        if let View::Own { testing: Some(testing), tested, error, .. } = &mut self.view
            && let Ok(answer) = testing.try_recv()
        {
            match answer {
                Ok(url) => *tested = Some(url),
                Err(why) => *error = Some(why),
            }
            if let View::Own { testing, .. } = &mut self.view {
                *testing = None;
            }
        }
        let View::Running(flow) = &mut self.view else { return };
        let mut next = None;
        while let Ok(event) = flow.events.try_recv() {
            match event {
                Event::Shown(waiting) => flow.card = Some(waiting),
                Event::Connected(Ok(told)) => {
                    self.told = told;
                    next = Some(View::Overview { leaving: false });
                }
                Event::Connected(Err(error)) | Event::Done(Err(error)) => next = Some(View::Said(error)),
                Event::Done(Ok(said)) => next = Some(View::Said(said)),
            }
        }
        if let Some(view) = next {
            self.view = view;
        }
    }

    fn test_own(&mut self) {
        let View::Own { buffer, testing, error, .. } = &mut self.view else { return };
        let (sender, receiver) = mpsc::channel();
        let address = buffer.trim().to_string();
        thread::spawn(move || {
            let _ = sender.send(connect::probe(&address));
        });
        (*testing, *error) = (Some(receiver), None);
    }

    // false closes the window.
    pub(super) fn key(&mut self, key: KeyEvent) -> bool {
        let escape = key.code == KeyCode::Esc;
        let close = escape || key.code == KeyCode::Char('q');
        match &mut self.view {
            View::Own { buffer, tested, testing, error } => match key.code {
                KeyCode::Esc => self.view = View::Choose { configured: None, selected: 1, error: None },
                KeyCode::Enter if testing.is_some() => {}
                KeyCode::Enter if tested.is_some() => {
                    let url = tested.take().unwrap_or_default();
                    let chosen = connect::save_own(&self.paths.config, url);
                    self.view = View::Name { url: chosen.url, note: chosen.note, buffer: String::new() };
                    self.name_or_start();
                }
                KeyCode::Enter if !buffer.trim().is_empty() => self.test_own(),
                KeyCode::Backspace => {
                    buffer.pop();
                    (*tested, *error) = (None, None);
                }
                KeyCode::Char(character) => {
                    buffer.push(character);
                    (*tested, *error) = (None, None);
                }
                _ => {}
            },
            View::Choose { configured: Some(url), .. } => match key.code {
                KeyCode::Enter => {
                    self.view = View::Name { url: url.clone(), note: None, buffer: String::new() };
                    self.name_or_start();
                }
                _ => return !close,
            },
            View::Choose { selected, .. } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(CHOICES.len() - 1),
                KeyCode::Enter => {
                    let index = *selected;
                    self.chosen(index);
                }
                KeyCode::Char('k') => self.chosen(0),
                KeyCode::Char('o') => self.chosen(1),
                _ => return !close,
            },
            View::Name { url, note, buffer } => match key.code {
                KeyCode::Esc => return false,
                KeyCode::Enter if !buffer.trim().is_empty() => {
                    let (url, note, given) = (url.clone(), note.take(), buffer.trim().to_string());
                    self.start(&url, Some(given), note);
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(character) => buffer.push(character),
                _ => {}
            },
            View::Running(flow) => match (key.code, &mut flow.card) {
                (KeyCode::Char('c'), Some(waiting)) => flow.copied = (self.paths.copy)(&waiting.link).ok().or(flow.copied),
                (KeyCode::Enter, Some(waiting)) => {
                    if let Some(program) = self.paths.opener && crate::issues::open_url(program, &waiting.link).is_ok() {
                        waiting.opened = true;
                    }
                }
                _ => return !escape,
            },
            View::Overview { leaving: true } => match key.code {
                KeyCode::Char('y') => self.leave(),
                _ => self.view = View::Overview { leaving: false },
            },
            View::Overview { leaving: false } => match key.code {
                KeyCode::Char('n') => {
                    let current = self.configured_name().or_else(|| identity::load(&self.paths.state).ok().flatten().map(|joined| joined.name)).unwrap_or_default();
                    self.view = View::Rename { buffer: current, error: None };
                }
                KeyCode::Char('l') => self.view = View::Overview { leaving: true },
                _ => return !close,
            },
            View::Rename { buffer, error } => match key.code {
                KeyCode::Esc => self.view = View::Overview { leaving: false },
                KeyCode::Enter => match crate::hub::rename_machine(&self.paths.config, buffer) {
                    Ok(_) => self.view = View::Overview { leaving: false },
                    Err(why) => *error = Some(why),
                },
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(character) => buffer.push(character),
                _ => {}
            },
            View::Said(_) => {
                if !matches!(identity::load(&self.paths.state), Ok(Some(_))) {
                    return false;
                }
                self.view = View::Overview { leaving: false };
            }
        }
        true
    }

    pub(super) fn typing(&self) -> bool {
        matches!(self.view, View::Own { .. } | View::Name { .. } | View::Rename { .. })
    }

    pub(super) fn paste(&mut self, text: &str) -> bool {
        match &mut self.view {
            View::Own { buffer, tested, .. } => {
                buffer.push_str(text.trim());
                *tested = None;
                true
            }
            View::Name { buffer, .. } | View::Rename { buffer, .. } => {
                buffer.push_str(text.trim());
                true
            }
            _ => false,
        }
    }
}

fn text(line: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(line.into(), style(TEXT)))
}

fn muted(line: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(line.into(), style(MUTED)))
}

fn typed(buffer: &str) -> Line<'static> {
    Line::from(vec![Span::styled(buffer.to_string(), style(TEXT)), Span::styled("▏", style(PEACH))])
}

pub(super) fn keys(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (position, (key, does)) in pairs.iter().enumerate() {
        if position > 0 {
            spans.push(Span::styled(" · ", style(MUTED)));
        }
        spans.push(Span::styled(key.to_string(), style(MAUVE)));
        spans.push(Span::styled(format!(" {does}"), style(MUTED)));
    }
    Line::from(spans)
}

fn tone(tone: Tone) -> Style {
    match tone {
        Tone::Text => style(TEXT),
        Tone::Title => bold(MAUVE),
        Tone::Code => bold(PEACH),
        Tone::Muted => style(MUTED),
        Tone::Wait => style(YELLOW),
        Tone::Good => style(GREEN),
        Tone::Bad => style(RED),
        Tone::Key => style(MAUVE),
        Tone::Dots => Style::new(),
    }
}

fn drawn(rows: Vec<Row>) -> Vec<Line<'static>> {
    rows.into_iter().map(|row| Line::from(row.into_iter().map(|(said, kind)| Span::styled(said, tone(kind))).collect::<Vec<_>>())).collect()
}

fn width(rows: &[Row]) -> usize {
    rows.iter().map(|row| row.iter().map(|(said, _)| text_width(said)).sum::<usize>()).max().unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Place {
    Beside,
    Above,
}

struct Parts {
    text: Vec<Row>,
    foot: Vec<Row>,
    keys: Line<'static>,
}

fn line_count(lines: &[Line<'static>], inner: u16) -> u16 {
    Paragraph::new(lines.to_vec()).wrap(Wrap { trim: false }).line_count(inner) as u16
}

fn laid(pane: Size, parts: &Parts, code: &[String], place: Place) -> Option<(Vec<Line<'static>>, u16)> {
    let room = pane.width.saturating_sub(4);
    let (mut lines, width) = match place {
        Place::Beside => {
            let rows = card::beside(code, card::Tone::Dots, parts.text.clone());
            let wide = (width(&rows) + 4) as u16;
            let width = wide.max(WIDTH).min(room);
            if wide > width {
                return None;
            }
            (drawn(rows), width)
        }
        Place::Above => {
            let width = WIDTH.min(room);
            if code.first().map_or(0, |line| text_width(line)) + 4 > width as usize {
                return None;
            }
            let mut lines = drawn(parts.text.clone());
            lines.push(Line::default());
            lines.extend(code.iter().map(|line| Line::from(Span::styled(line.clone(), tone(card::Tone::Dots)))));
            (lines, width)
        }
    };
    lines.push(Line::default());
    lines.extend(drawn(parts.foot.clone()));
    lines.push(Line::default());
    lines.push(parts.keys.clone());
    (line_count(&lines, width.saturating_sub(4)) + 2 <= pane.height).then_some((lines, width))
}

fn running(pane: Size, flow: &Flow, opener: bool, glyphs: qr::Glyphs) -> (Vec<Line<'static>>, u16) {
    let Some(waiting) = &flow.card else { return (vec![muted(flow.waiting.clone()), Line::default(), keys(&[("esc", "stop")])], WIDTH) };
    let left = waiting.expires.saturating_duration_since(Instant::now());
    let browser = match (waiting.opened, opener) {
        (true, _) => card::Browser::Opened,
        (false, true) => card::Browser::Offered,
        (false, false) => card::Browser::Absent,
    };
    let text = card::waiting_text(&card::Waiting { name: &waiting.name, url: &waiting.url, code: &waiting.code, browser, left });
    let foot = card::footer(&waiting.link, flow.copied.map_or(Foot::Bare, Foot::Copied));
    let pairs: &[(&str, &str)] = if opener { &[("⏎", "open"), ("c", "copy link"), ("esc", "stop")] } else { &[("c", "copy link"), ("esc", "stop")] };
    let parts = Parts { text, foot, keys: keys(pairs) };
    let code = qr::lines(&waiting.link, glyphs).unwrap_or_default();
    if let Some(found) = [Place::Beside, Place::Above].into_iter().find_map(|place| laid(pane, &parts, &code, place)) {
        return found;
    }
    let mut lines = drawn(parts.text);
    lines.push(Line::default());
    lines.extend(drawn(parts.foot));
    lines.push(muted(card::NO_ROOM));
    lines.push(Line::default());
    lines.push(parts.keys);
    (lines, WIDTH)
}

fn choice(index: usize, selected: usize, about: String) -> Line<'static> {
    let (key, what) = CHOICES[index];
    let chosen = index == selected;
    let background = if chosen { Style::new().bg(SURFACE) } else { Style::new() };
    Line::from(vec![
        Span::styled(format!(" {key}  "), if chosen { bold(MAUVE) } else { style(MAUVE) }),
        Span::styled(format!("{what:<15}"), if chosen { bold(TEXT) } else { style(TEXT) }),
        Span::styled(about, style(MUTED)),
    ]).style(background)
}

fn choose(configured: Option<&String>, selected: usize, error: Option<&String>) -> Vec<Line<'static>> {
    let mut lines = vec![text("See this board on your phone, end-to-end encrypted."), Line::default()];
    match configured {
        Some(url) => lines.push(text(format!("Connect to {}, as [hub] url says.", host(url)))),
        None => {
            lines.push(choice(0, selected, format!("{} · free to connect and read", host(DEFAULT_HUB_URL))));
            lines.push(choice(1, selected, "a server you run kebbie-hub on".to_string()));
        }
    }
    lines.extend(error.map(|error| Line::from(Span::styled(error.clone(), style(RED)))));
    lines.push(Line::default());
    lines.push(match configured {
        Some(_) => keys(&[("⏎", "connect"), ("esc", "close")]),
        None => keys(&[("k", "kebbie hub"), ("o", "your own"), ("↑↓ ⏎", "choose"), ("esc", "close")]),
    });
    lines
}

fn own(buffer: &str, tested: Option<&String>, testing: bool, error: Option<&String>) -> Vec<Line<'static>> {
    let mut lines = vec![text("Address of the server that runs kebbie-hub"), typed(buffer)];
    if testing {
        lines.push(muted("Testing it…"));
    }
    if let Some(url) = tested {
        lines.push(Line::from(vec![Span::styled("✓ ", style(GREEN)), Span::styled("answers as a kebbie hub   ", style(TEXT)), Span::styled(connect::transport(url), style(MUTED))]));
    }
    lines.extend(error.map(|error| Line::from(Span::styled(error.clone(), style(RED)))));
    lines.push(Line::default());
    lines.push(keys(&[("⏎", if tested.is_some() { "connect" } else { "test it" }), ("esc", "back")]));
    lines
}

fn status(sent: Sent) -> Style {
    style(match sent {
        Sent::Yes => GREEN,
        Sent::Failed => RED,
        Sent::NotYet => YELLOW,
    })
}

fn overview(window: &Window, leaving: bool) -> Vec<Line<'static>> {
    let Ok(Some(joined)) = identity::load(&window.paths.state) else { return vec![muted("This machine is not connected."), keys(&[("any key", "close")])] };
    let now = crate::messages::now();
    let relay = relay::load(&window.paths.state);
    let (sent, said) = board_line(&relay, !joined.devices.is_empty(), now);
    let said = match (sent, relay.posted_at) {
        (Sent::Yes, Some(at)) => format!("board sent {}", devices::ago(at, now)),
        _ => said,
    };
    let renaming = window.configured_name().filter(|name| *name != joined.name);
    let mut first = vec![Span::styled("● ", status(sent)), Span::styled(format!("{} on {}", renaming.as_deref().unwrap_or(&joined.name), host(&joined.url)), style(TEXT))];
    if renaming.is_some() {
        first.push(Span::styled("  renaming on the hub…", style(MUTED)));
    }
    let mut lines = Vec::new();
    if sent == Sent::Yes {
        first.push(Span::styled(format!("   {said}"), style(MUTED)));
        lines.push(Line::from(first));
    } else {
        lines.push(Line::from(first));
        lines.push(text(format!("  {said}")));
    }
    if relay.plan_inactive.is_some() {
        lines.push(muted("Everything here keeps working; your devices get the board again once it is active."));
    }
    lines.extend(window.told.iter().map(muted));
    lines.push(Line::default());
    lines.extend(drawn(card::connected_text(&joined.name, &joined.url, "this").split_off(2).into_iter().map(|row| row.into_iter().map(|(said, kind)| (said.trim_start().to_string(), kind)).collect()).collect()));
    lines.push(Line::default());
    if leaving {
        lines.push(Line::from(Span::styled("Leave the hub? Your devices lose this board, and the keys go.", style(YELLOW))));
        lines.push(keys(&[("y", "leave the hub"), ("any other key", "keeps it")]));
    } else {
        lines.push(keys(&[("n", "rename"), ("l", "leave the hub"), ("esc", "close")]));
    }
    lines
}

pub(super) fn draw(frame: &mut Frame, window: &Window) {
    let (title, lines, width) = match &window.view {
        View::Own { buffer, tested, testing, error } => ("kebbie hub · your own", own(buffer, tested.as_ref(), testing.is_some(), error.as_ref()), WIDTH),
        View::Choose { configured, selected, error } => ("kebbie hub", choose(configured.as_ref(), *selected, error.as_ref()), WIDTH),
        View::Name { buffer, .. } => ("kebbie hub · name this machine", vec![text("Your phone and your other machines show it."), typed(buffer), Line::default(), keys(&[("⏎", "connect and save it as [machine] name"), ("esc", "close")])], WIDTH),
        View::Running(flow) => {
            let (lines, width) = running(frame.area().as_size(), flow, window.paths.opener.is_some(), window.paths.glyphs);
            (flow.title, lines, width)
        }
        View::Overview { leaving } => ("kebbie hub", overview(window, *leaving), WIDTH),
        View::Rename { buffer, error } => ("kebbie hub · rename this machine", [text("The name the hub and your devices show for this machine:"), typed(buffer)].into_iter().chain(error.iter().map(|error| Line::from(Span::styled(error.clone(), style(RED))))).chain([Line::default(), keys(&[("⏎", "save it as [machine] name"), ("esc", "back")])]).collect(), WIDTH),
        View::Said(said) => ("kebbie hub", said.lines().map(text).chain([Line::default(), keys(&[("any key", "goes on")])]).collect(), WIDTH),
    };
    modal(frame, title.to_string(), lines, width);
}

impl App {
    pub(super) fn open_hub(&mut self) {
        let paths = match &self.hub_paths {
            Some(paths) => paths.clone(),
            None => match crate::lead::state_dir() {
                Ok(state) => Paths { state, config: crate::config::path(), post: crate::hub::tick, copy: crate::hub::console::copy_here, opener: self.viewer, glyphs: qr::glyphs() },
                Err(error) => return self.show(Err(error)),
            },
        };
        self.mode = Mode::Hub(Box::new(Window::open(paths)));
    }

    pub(super) fn handle_hub(&mut self, mut window: Box<Window>, key: KeyEvent) {
        if window.key(key) {
            self.mode = Mode::Hub(window);
        }
        let _ = self.refresh.send(());
    }

    pub(super) fn poll_hub(&mut self) {
        if let Mode::Hub(window) = &mut self.mode {
            window.poll();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;

    use kebbie_hub_protocol::{Id, Prefix, SecretKeys};

    use ratatui::style::Color;

    use super::*;
    use crate::board::test_support::*;

    static COPIED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn copied(text: &str) -> Result<Copied> {
        COPIED.lock().unwrap().push(text.to_string());
        Ok(Copied::Confirmed)
    }

    fn paths(config: &str) -> Paths {
        let state = crate::sh::test_dir("board-hub");
        let file = state.join("config.toml");
        std::fs::write(&file, config).unwrap();
        Paths { state, config: file, post: || Ok(()), copy: copied, opener: None, glyphs: qr::Glyphs::Octant }
    }

    fn screen(app: &mut App, width: u16, height: u16) -> String {
        render(app, width, height).0.join("\n")
    }

    fn cell_of(app: &mut App, needle: &str) -> ratatui::buffer::Cell {
        let (rows, buffer) = render(app, 110, 50);
        let (y, row) = rows.iter().enumerate().find(|(_, row)| row.contains(needle)).unwrap_or_else(|| panic!("{needle} is not shown: {}", rows.join("\n")));
        let x = row[..row.find(needle).unwrap()].chars().count();
        buffer[(x as u16, y as u16)].clone()
    }

    fn colour_of(app: &mut App, needle: &str) -> Color {
        cell_of(app, needle).fg
    }

    fn window(app: &mut App) -> &mut Window {
        match &mut app.mode {
            Mode::Hub(window) => window,
            _ => panic!("the hub window is not open"),
        }
    }

    fn until(app: &mut App, what: &str, done: impl Fn(&Window) -> bool) {
        let started = Instant::now();
        while !done(window(app)) {
            assert!(started.elapsed() < crate::hub::tests::WAIT, "{what} never came");
            std::thread::sleep(Duration::from_millis(20));
            app.poll_hub();
        }
    }

    fn typed_in(app: &mut App, text: &str) {
        for character in text.chars() {
            press(app, key(KeyCode::Char(character)));
        }
    }

    fn joined(relay: relay::Relay) -> Paths {
        let paths = paths("[machine]\nname = \"mac\"\n");
        let now = crate::messages::now();
        let mut machine = identity::Identity::new("https://kebbie.example.com", Id::new(Prefix::Machine).unwrap(), "mac", "khm_test".to_string(), &SecretKeys::generate(), now).unwrap();
        machine.admit(identity::Device { id: Id::new(Prefix::Device).unwrap(), keys: SecretKeys::generate().public(), paired_at: now, label: None });
        identity::save(&paths.state, &machine).unwrap();
        std::fs::write(identity::dir(&paths.state).join("relay.json"), serde_json::to_string(&relay).unwrap()).unwrap();
        paths
    }

    fn opened(paths: Paths) -> App {
        let mut app = test_app(Vec::new());
        app.hub_paths = Some(paths);
        press(&mut app, key(KeyCode::Char('H')));
        app
    }

    #[test]
    fn not_connected_the_window_offers_the_kebbie_hub_or_your_own_and_highlights_the_row_the_arrows_pick() {
        let mut app = opened(paths(""));
        let shown = screen(&mut app, 110, 40);
        for line in ["╭ kebbie hub ─", "See this board on your phone, end-to-end encrypted.", " k  kebbie hub     hub.kebbie.dev · free to connect and read", " o  your own hub   a server you run kebbie-hub on", "k kebbie hub · o your own · ↑↓ ⏎ choose · esc close"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        assert_eq!((cell_of(&mut app, "kebbie hub     hub").bg, cell_of(&mut app, "your own hub   a").bg), (SURFACE, Color::Reset), "the kebbie hub is picked first");
        assert_eq!(colour_of(&mut app, "k kebbie hub ·"), MAUVE);
        press(&mut app, key(KeyCode::Down));
        assert_eq!((cell_of(&mut app, "kebbie hub     hub").bg, cell_of(&mut app, "your own hub   a").bg), (Color::Reset, SURFACE));
        press(&mut app, key(KeyCode::Enter));
        assert!(screen(&mut app, 110, 40).contains("╭ kebbie hub · your own ─"), "⏎ chooses the picked row");
        press(&mut app, key(KeyCode::Esc));
        press(&mut app, key(KeyCode::Up));
        press(&mut app, key(KeyCode::Enter));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("kebbie hub · name this machine") && shown.contains("Your phone and your other machines show it.") && shown.contains(&format!("{}▏", crate::config::short_hostname())), "an unset name is asked for once, with the host's: {shown}");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));

        let mut app = opened(paths("[hub]\nurl = \"https://hub.example.com\"\n"));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Connect to hub.example.com, as [hub] url says.") && shown.contains("⏎ connect · esc close") && !shown.contains("your own"), "{shown}");
    }

    #[test]
    fn your_own_hub_is_tested_and_shows_that_it_answers_before_enter_saves_and_connects() {
        let hub = crate::hub::tests::open_hub(|_| {});
        let paths = paths("[machine]\nname = \"mac\"\n");
        let config = paths.config.clone();
        let mut app = opened(paths);
        press(&mut app, key(KeyCode::Char('o')));
        typed_in(&mut app, "http://hub.example.com");
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Address of the server that runs kebbie-hub") && shown.contains("http://hub.example.com▏") && shown.contains("⏎ test it · esc back"), "{shown}");
        press(&mut app, key(KeyCode::Enter));
        until(&mut app, "the test", |window| matches!(&window.view, View::Own { testing: None, .. }));
        assert!(screen(&mut app, 110, 40).contains("takes https"), "a refused address says why");
        assert_eq!(colour_of(&mut app, "http://hub.example.com: a hub"), RED);
        for _ in 0.."http://hub.example.com".len() {
            press(&mut app, key(KeyCode::Backspace));
        }
        typed_in(&mut app, &hub.url);
        press(&mut app, key(KeyCode::Enter));
        until(&mut app, "the test", |window| matches!(&window.view, View::Own { testing: None, .. }));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("✓ answers as a kebbie hub   plain http, on this machine only") && shown.contains("⏎ connect · esc back"), "{shown}");
        assert_eq!(colour_of(&mut app, "✓ answers"), GREEN);
        assert_eq!(crate::config::load_machine(&config).unwrap().hub_url, None, "testing saves nothing");
        press(&mut app, key(KeyCode::Enter));
        assert_eq!(crate::config::load_machine(&config).unwrap().hub_url.as_deref(), Some(hub.url.as_str()), "⏎ saves it as [hub] url");
        until(&mut app, "the card", |window| matches!(&window.view, View::Running(flow) if flow.card.is_some()));
        assert!(screen(&mut app, 110, 50).contains("Connect mac to your own hub"));
    }

    const LINK: &str = "https://hub.kebbie.dev/connect?code=Z78K-XF53#k=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&m=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

    fn waiting_for(app: &mut App, left: Duration) {
        waiting_with(app, left, paths(""));
    }

    fn waiting_with(app: &mut App, left: Duration, paths: Paths) {
        let flow = spawn("kebbie hub · connect", String::new(), move |events, stop| {
            let _ = events.send(Event::Shown(Waiting { link: LINK.to_string(), code: "Z78K-XF53".to_string(), name: "mac".to_string(), url: DEFAULT_HUB_URL.to_string(), opened: false, expires: Instant::now() + left }));
            while !stop() {
                std::thread::sleep(Duration::from_millis(20));
            }
            Event::Done(Err(connect::STOPPED.to_string()))
        });
        app.mode = Mode::Hub(Box::new(Window { paths, view: View::Running(flow), told: Vec::new() }));
        until(app, "the card", |window| matches!(&window.view, View::Running(flow) if flow.card.is_some()));
    }

    #[test]
    fn connecting_shows_the_card_with_the_code_the_time_left_and_a_square_qr_with_the_text_from_its_top_and_c_copies_the_whole_link() {
        let mut app = test_app(Vec::new());
        waiting_for(&mut app, Duration::from_secs(530));
        let shown = screen(&mut app, 110, 50);
        for line in ["╭ kebbie hub · connect ─", "Connect mac to kebbie hub", "hub.kebbie.dev", "Check the page shows  Z78K-XF53", "⧗ Waiting for you · 9 min left", "No browser here? Scan the code, or open", "hub.kebbie.dev/connect?code=Z78K-XF53#…", "c copy link · esc stop"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        assert!(!shown.contains("#k="), "the secret in the fragment is never drawn: {shown}");
        assert!(!shown.contains("Opened") && !shown.contains("⏎ open"), "without a browser here nothing offers to open one: {shown}");
        let code = qr::lines(LINK, qr::Glyphs::Octant).unwrap();
        let (rows, buffer) = render(&mut app, 120, 40);
        let first = rows.iter().position(|row| row.contains(&code[1])).unwrap_or_else(|| panic!("no QR code: {}", rows.join("\n"))) - 1;
        for (offset, line) in code.iter().enumerate() {
            assert!(rows[first + offset].contains(line.as_str()), "the code's row {offset} is drawn whole");
        }
        assert!(code[0].chars().count().abs_diff(code.len() * 2) <= 1, "two modules a column and four a row, so the code is as tall as wide: {} × {}", code[0].chars().count(), code.len());
        assert!(rows[first].contains(&format!("{}   Connect mac to kebbie hub", code[0])), "the title sits on the code's first row: {}", rows[first]);
        let beside = rows.iter().position(|row| row.contains("Check the page shows")).unwrap();
        assert!(rows[beside].contains(&format!("{}   Check the page shows", code[beside - first])), "the text sits to the right of the code: {}", rows[beside]);
        let x = rows[first + 1].find(&code[1]).map(|byte| rows[first + 1][..byte].chars().count()).unwrap() as u16;
        assert_eq!((buffer[(x, first as u16 + 1)].fg, buffer[(x, first as u16 + 1)].bg), (Color::Reset, Color::Reset), "the code in the terminal's own colours, no paper");
        assert_eq!(colour_of(&mut app, "Z78K-XF53"), PEACH);
        assert!(cell_of(&mut app, "Z78K-XF53").modifier.contains(ratatui::style::Modifier::BOLD));
        assert_eq!(colour_of(&mut app, "Connect mac"), MAUVE);
        assert_eq!(colour_of(&mut app, "⧗"), YELLOW);
        assert_eq!(colour_of(&mut app, "Waiting for you"), MUTED);

        press(&mut app, key(KeyCode::Char('c')));
        assert!(COPIED.lock().unwrap().contains(&LINK.to_string()), "c copies the whole link, the secret with it");
        assert!(screen(&mut app, 110, 50).contains("hub.kebbie.dev/connect?code=Z78K-XF53#…   ✓ copied"));
        let short = screen(&mut app, 110, 16);
        assert!(short.contains("A larger terminal shows a QR code of the link here.") && short.contains("Check the page shows  Z78K-XF53") && !short.contains(&code[1]), "{short}");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal), "esc stops waiting");

        waiting_for(&mut app, Duration::from_millis(42_800));
        assert!(screen(&mut app, 110, 50).contains("⧗ Waiting for you · 42 s left"), "the time left counts down");
    }

    #[test]
    fn enter_opens_the_link_in_the_browser_and_nothing_opens_before_it() {
        let dir = crate::sh::test_dir("board-hub-open");
        let (opener, opened) = (dir.join("open"), dir.join("opened"));
        crate::executable::write(&opener, format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", opened.display())).unwrap();
        let mut app = test_app(Vec::new());
        waiting_with(&mut app, Duration::from_secs(530), Paths { opener: Some(Box::leak(opener.display().to_string().into_boxed_str())), ..paths("") });
        let shown = screen(&mut app, 110, 50);
        assert!(shown.contains("⏎ open in browser · or scan with your phone") && shown.contains("⏎ open · c copy link · esc stop") && !shown.contains("Opened"), "{shown}");
        assert!(!opened.exists(), "the card opens nothing by itself");
        press(&mut app, key(KeyCode::Enter));
        assert_eq!(std::fs::read_to_string(&opened).unwrap(), LINK, "⏎ opens the whole link");
        let shown = screen(&mut app, 110, 50);
        assert!(shown.contains("✓ Opened in your browser") && !shown.contains("open in browser · or scan"), "{shown}");
    }

    #[test]
    fn the_code_sits_beside_the_text_under_it_in_a_narrow_pane_and_gives_way_to_a_note_in_a_short_one() {
        let mut app = test_app(Vec::new());
        waiting_for(&mut app, Duration::from_secs(530));
        let code = qr::lines(LINK, qr::Glyphs::Octant).unwrap();
        let wide = screen(&mut app, 120, 40);
        assert!(wide.contains(&format!("{}   Connect mac to kebbie hub", code[0])), "beside the text, from the code's top row: {wide}");
        let narrow = screen(&mut app, 44, 45);
        let rows = narrow.lines().collect::<Vec<_>>();
        let first = rows.iter().position(|row| row.contains(&code[1])).unwrap_or_else(|| panic!("no code in a narrow pane: {narrow}"));
        assert!(rows.iter().position(|row| row.contains("Z78K-XF53")).unwrap() < first, "the text above the code when they do not fit side by side: {narrow}");
        assert!(code.iter().all(|line| narrow.contains(line.as_str())), "every row of the code, whole: {narrow}");
        let tall = screen(&mut app, 110, 21);
        assert!(tall.contains(&code[1]) && !tall.contains("A larger terminal"), "the window takes the pane's height for the code: {tall}");
        let short = screen(&mut app, 110, 20);
        assert!(!short.contains(&code[1]) && short.contains("A larger terminal shows a QR code"), "{short}");
    }

    #[test]
    fn connected_the_window_names_the_machine_on_its_hub_when_the_board_went_out_and_where_to_sign_in() {
        let now = crate::messages::now();
        let mut app = opened(joined(relay::Relay { posted_at: Some(now - 12), rev: 3, ..Default::default() }));
        let shown = screen(&mut app, 110, 40);
        for line in ["╭ kebbie hub ─", "● mac on kebbie.example.com   board sent 12 s ago", "Sign in with your passkey on any phone or browser", "to see this board:  kebbie.example.com", "n rename · l leave the hub · esc close"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        assert!(!shown.contains("Device") && !shown.contains("dev_") && !shown.contains("https://") && !shown.contains("disconnect"), "no device list, no ids, no scheme: {shown}");
        assert_eq!(colour_of(&mut app, "● mac"), GREEN);
        press(&mut app, key(KeyCode::Char('l')));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Leave the hub? Your devices lose this board, and the keys go.") && shown.contains("y leave the hub · any other key keeps it"), "{shown}");
        press(&mut app, key(KeyCode::Char('x')));
        assert!(screen(&mut app, 110, 40).contains("l leave the hub · esc close"), "anything but y keeps it");
    }

    #[test]
    fn the_dot_is_red_with_the_reason_when_posts_fail_and_amber_before_the_first() {
        let now = crate::messages::now();
        let plan = "Your kebbie hub subscription isn't active: https://billing.example.com/start".to_string();
        let mut app = opened(joined(relay::Relay { posted_at: Some(now - 600), plan_inactive: Some(plan.clone()), error: Some(plan.clone()), error_at: Some(now - 5), ..Default::default() }));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("● mac on kebbie.example.com\n") || shown.contains("● mac on kebbie.example.com "), "{shown}");
        assert!(shown.contains(&format!("  {plan}")) && shown.contains("Everything here keeps working"), "{shown}");
        assert_eq!(colour_of(&mut app, "● mac"), RED);
        app.hub_banner = Some(plan.clone());
        press(&mut app, key(KeyCode::Esc));
        assert!(screen(&mut app, 110, 40).contains(&format!("{plan} · H hub")), "the board shows a lapsed plan too");

        let mut app = opened(joined(relay::Relay { posted_at: Some(now - 600), error: Some("could not reach the hub: connection refused".to_string()), error_at: Some(now - 120), ..Default::default() }));
        assert!(screen(&mut app, 110, 40).contains("  Board not sent: the hub can't be reached (2 min ago)"));
        assert_eq!(colour_of(&mut app, "● mac"), RED);
        let mut app = opened(joined(relay::Relay { posted_at: Some(now - 150), error: Some("could not reach the hub: once".to_string()), error_at: Some(now - 300), ..Default::default() }));
        assert!(screen(&mut app, 110, 40).contains("board sent 2 min ago"), "a post after the failure wins");

        let mut app = opened(joined(relay::Relay::default()));
        assert!(screen(&mut app, 110, 40).contains("  Board not sent yet; the keeper sends it within a minute"));
        assert_eq!(colour_of(&mut app, "● mac"), YELLOW);
    }

    #[test]
    fn n_renames_this_machine_in_config_toml_and_shows_the_name_at_once_until_the_hub_has_it() {
        let paths = joined(relay::Relay::default());
        let (state, config) = (paths.state.clone(), paths.config.clone());
        let mut app = opened(paths);
        press(&mut app, key(KeyCode::Char('n')));
        assert!(screen(&mut app, 110, 40).contains("mac▏"), "prefilled with [machine] name");
        for _ in 0..3 {
            press(&mut app, key(KeyCode::Backspace));
        }
        press(&mut app, key(KeyCode::Enter));
        assert!(screen(&mut app, 110, 40).contains("machine.name: must be a non-empty string"), "the config's own check");
        typed_in(&mut app, "Studio Mac");
        press(&mut app, key(KeyCode::Enter));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("● Studio Mac on kebbie.example.com  renaming on the hub…"), "{shown}");
        assert_eq!(crate::config::load_machine(&config).unwrap().machine_name.as_deref(), Some("Studio Mac"));
        let mut renamed = identity::load(&state).unwrap().unwrap();
        renamed.name = "Studio Mac".to_string();
        identity::save(&state, &renamed).unwrap();
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("● Studio Mac on") && !shown.contains("renaming"), "the keeper's rename confirmed it: {shown}");
    }

    #[test]
    fn n_refuses_to_write_through_a_linked_config_and_says_where_to_set_it() {
        let paths = joined(relay::Relay::default());
        let real = paths.state.join("team.toml");
        std::fs::rename(&paths.config, &real).unwrap();
        std::os::unix::fs::symlink(&real, &paths.config).unwrap();
        let mut app = opened(paths);
        press(&mut app, key(KeyCode::Char('n')));
        press(&mut app, key(KeyCode::Char('2')));
        press(&mut app, key(KeyCode::Enter));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("is a link") && shown.contains("[machine] name = \"mac2\""), "{shown}");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "[machine]\nname = \"mac\"\n", "the linked file is left as it was");
    }

    #[test]
    fn the_board_connects_a_hub_and_shows_the_machine_connected_once_the_browser_approves() {
        let hub = crate::hub::tests::open_hub(|_| {});
        let mut browser = crate::hub::tests::member(&hub);
        let paths = paths(&format!("[machine]\nname = \"mac\"\n\n[hub]\nurl = \"{}\"\n", hub.url));
        let state: &Path = &paths.state.clone();
        let mut app = opened(paths);
        press(&mut app, key(KeyCode::Enter));
        until(&mut app, "the card", |window| matches!(&window.view, View::Running(flow) if flow.card.is_some()));
        let View::Running(flow) = &window(&mut app).view else { unreachable!() };
        let waiting = flow.card.clone().unwrap();
        assert!(screen(&mut app, 110, 50).contains(&format!("Check the page shows  {}", waiting.code)));
        let approval = hub.runtime.block_on(browser.approve_connect(&waiting.link, true)).unwrap();
        hub.runtime.block_on(browser.show_board(approval, crate::hub::tests::WAIT)).unwrap();
        until(&mut app, "the outcome", |window| matches!(window.view, View::Overview { .. }));
        let shown = screen(&mut app, 110, 50);
        assert!(shown.contains(&format!("● mac on {}", host(&hub.url))) && shown.contains("n rename · l leave the hub"), "{shown}");
        let device = browser.device.as_ref().unwrap().device.clone().unwrap();
        assert_eq!(identity::load(state).unwrap().unwrap().devices[0].id, device);
    }

}
