use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use kelpie_hub_protocol::Id;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::config::DEFAULT_HUB_URL;
use crate::hub::connect;
use crate::hub::devices::{self, Listed};
use crate::hub::{Sent, board_line, host, identity, pair, qr, relay};
use crate::sh::Result;
use super::widgets::{GREEN, MAUVE, MUTED, PEACH, RED, TEXT, YELLOW, bold, modal, style, text_width};
use super::{App, Mode};

pub(super) type Post = fn() -> Result<()>;
pub(super) type Fetch = fn(&Path, u64) -> Result<Listed>;

const WIDTH: u16 = 88;

#[derive(Clone)]
pub(super) struct Paths {
    pub(super) state: PathBuf,
    pub(super) config: PathBuf,
    pub(super) post: Post,
    pub(super) fetch: Fetch,
    pub(super) opener: Option<&'static str>,
    pub(super) truecolor: bool,
}

pub(super) enum Event {
    Shown { link: String, code: Option<String> },
    Digits(String),
    Tested(Result<connect::Chosen>),
    Done(Result<String>),
}

pub(super) struct Flow {
    pub(super) title: &'static str,
    pub(super) waiting: String,
    pub(super) link: Option<String>,
    pub(super) code: Option<String>,
    pub(super) digits: Option<String>,
    events: Receiver<Event>,
    answer: Option<Sender<bool>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Flow {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Pick {
    Main,
    Details,
    Removing(usize),
    Naming(usize),
    Leaving,
}

pub(super) enum View {
    Choose { configured: Option<String>, typing: Option<String>, error: Option<String> },
    Name { url: String, note: Option<String>, buffer: String },
    Running(Flow),
    Overview(Pick),
    Label { id: Id, buffer: String },
    Rename { buffer: String, error: Option<String> },
    Said(String),
}

pub(super) struct Window {
    pub(super) paths: Paths,
    pub(super) view: View,
    pub(super) listed: Listed,
    fetching: Option<Receiver<Result<Listed>>>,
}

fn spawn(title: &'static str, waiting: String, step: impl FnOnce(&Sender<Event>, &(dyn Fn() -> bool + Sync), Receiver<bool>) -> Event + Send + 'static) -> Flow {
    let (sender, events) = mpsc::channel();
    let (answer, answers) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    thread::spawn(move || {
        let stop = move || stopped.load(Ordering::Relaxed);
        let last = step(&sender, &stop, answers);
        let _ = sender.send(last);
    });
    Flow { title, waiting, link: None, code: None, digits: None, events, answer: Some(answer), stop }
}

pub(super) struct Row {
    pub(super) id: Id,
    pub(super) name: String,
    pub(super) named: bool,
    pub(super) facts: String,
}

impl Window {
    pub(super) fn open(paths: Paths) -> Window {
        let view = match identity::load(&paths.state) {
            Ok(Some(_)) => View::Overview(Pick::Main),
            Ok(None) => match connect::configured_hub(&paths.config) {
                Ok(configured) => View::Choose { configured, typing: None, error: None },
                Err(error) => View::Said(error),
            },
            Err(error) => View::Said(error),
        };
        let listed = devices::load(&paths.state);
        let mut window = Window { paths, view, listed, fetching: None };
        window.refetch();
        window
    }

    // The hub answers over the network, so the list comes from a thread and the window shows the last one meanwhile.
    fn refetch(&mut self) {
        if !matches!(identity::load(&self.paths.state), Ok(Some(_))) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let (state, fetch) = (self.paths.state.clone(), self.paths.fetch);
        thread::spawn(move || {
            let _ = sender.send(fetch(&state, crate::messages::now()));
        });
        self.fetching = Some(receiver);
    }

    pub(super) fn rows(&self, now: u64) -> Vec<Row> {
        let Ok(Some(joined)) = identity::load(&self.paths.state) else { return Vec::new() };
        joined.devices.iter().map(|paired| {
            let named = devices::named(&paired.id, &self.listed, &joined.devices);
            let facts = self.listed.devices.iter().find(|known| known.device == paired.id).map_or_else(|| format!("paired {}", devices::on_day(paired.paired_at, now)), |known| devices::facts(known, now));
            Row { id: paired.id.clone(), named: named.is_some(), name: named.unwrap_or_else(|| devices::UNNAMED.to_string()), facts }
        }).collect()
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

    fn start(&mut self, url: &str, given: Option<String>, note: Option<String>) {
        let paths = self.paths.clone();
        let prepared = match connect::prepare(&paths.state, &paths.config, given.as_deref(), None) {
            Ok(prepared) => prepared,
            Err(error) => return self.view = View::Said(error),
        };
        let url = url.to_string();
        let waiting = format!("Waiting for you to approve it in the browser at {}…", host(&url));
        self.view = View::Running(spawn("kelpie hub · connect", waiting, move |events, stop, _| {
            let now = crate::messages::now();
            let name = prepared.named.name().to_string();
            let result = (|| {
                let mut pending = connect::start(&url, &name)?;
                let link = pending.link();
                let _ = events.send(Event::Shown { link: link.clone(), code: Some(pending.user_code().to_string()) });
                if let Some(program) = paths.opener {
                    let _ = crate::issues::open_url(program, &link);
                }
                let connected = pending.wait(&mut connect::sleep_unless(Some(stop)))?;
                let line = connect::finish(&paths.state, &pending, &connected, Some(stop), paths.post, now)?;
                let lines = [note, Some(line), connect::name_line(&paths.config, prepared, &name)];
                Ok(lines.into_iter().flatten().collect::<Vec<_>>().join("\n"))
            })();
            Event::Done(result)
        }));
    }

    fn test_own(&mut self, address: String) {
        let config = self.paths.config.clone();
        self.view = View::Running(spawn("kelpie hub · connect", format!("Testing {address}…"), move |_, _, _| Event::Tested(connect::own_hub(&config, &address))));
    }

    fn add_device(&mut self) {
        let paths = self.paths.clone();
        self.view = View::Running(spawn("kelpie hub · add a device", "Waiting for your phone or browser to open the link…".to_string(), move |events, stop, answers| {
            let now = crate::messages::now();
            let result = pair::add_device(&paths.state, pair::TIMEOUT, |added| {
                let _ = events.send(Event::Shown { link: added.url.clone(), code: None });
            }, |digits| {
                let _ = events.send(Event::Digits(digits.to_string()));
                answers.recv().unwrap_or(false)
            }, Some(stop), now);
            Event::Done(result.map(|device| {
                let posted = (paths.post)().err().map_or(String::new(), |error| format!(" The first board did not go out yet: {error}."));
                format!("Added {}; it shows this board from now on.{posted}", devices::added_name(&paths.state, &device, now))
            }))
        }));
    }

    fn remove_device(&mut self, row: Row) {
        let paths = self.paths.clone();
        self.view = View::Running(spawn("kelpie hub · remove a device", format!("Removing {}…", row.name), move |_, _, _| {
            Event::Done(devices::remove(&paths.state, &row.id).map(|said| {
                let posted = (paths.post)().err().map_or(String::new(), |error| format!(" The board did not go out again yet: {error}."));
                format!("{said}{posted}")
            }))
        }));
    }

    fn leave(&mut self) {
        let state = self.paths.state.clone();
        self.view = View::Running(spawn("kelpie hub · disconnect", "Taking your devices off and forgetting the keys…".to_string(), move |_, _, _| Event::Done(crate::hub::leave(&state))));
    }

    pub(super) fn poll(&mut self) {
        if let Some(fetching) = &self.fetching
            && let Ok(answer) = fetching.try_recv()
        {
            match answer {
                Ok(listed) => self.listed = listed,
                Err(error) => self.listed.error = Some(error),
            }
            self.fetching = None;
        }
        let View::Running(flow) = &mut self.view else { return };
        let mut next = None;
        while let Ok(event) = flow.events.try_recv() {
            match event {
                Event::Shown { link, code } => (flow.link, flow.code) = (Some(link), code),
                Event::Digits(digits) => flow.digits = Some(digits),
                Event::Tested(Ok(chosen)) => next = Some(View::Name { url: chosen.url, note: chosen.note, buffer: String::new() }),
                Event::Tested(Err(error)) => next = Some(View::Choose { configured: None, typing: None, error: Some(error) }),
                Event::Done(said) => next = Some(View::Said(said.unwrap_or_else(|error| error))),
            }
        }
        if let Some(view) = next {
            self.view = view;
            self.name_or_start();
        }
    }

    // false closes the window.
    pub(super) fn key(&mut self, key: KeyEvent) -> bool {
        let escape = key.code == KeyCode::Esc;
        let devices = self.rows(crate::messages::now()).len();
        match &mut self.view {
            View::Choose { typing: Some(buffer), .. } => match key.code {
                KeyCode::Esc => self.view = View::Choose { configured: None, typing: None, error: None },
                KeyCode::Enter if !buffer.trim().is_empty() => {
                    let address = buffer.trim().to_string();
                    self.test_own(address);
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(character) => buffer.push(character),
                _ => {}
            },
            View::Choose { configured: Some(url), .. } => match key.code {
                KeyCode::Enter => {
                    self.view = View::Name { url: url.clone(), note: None, buffer: String::new() };
                    self.name_or_start();
                }
                _ => return !escape && key.code != KeyCode::Char('q'),
            },
            View::Choose { typing: None, .. } => match key.code {
                KeyCode::Char('k') | KeyCode::Enter => {
                    self.view = View::Name { url: DEFAULT_HUB_URL.to_string(), note: None, buffer: String::new() };
                    self.name_or_start();
                }
                KeyCode::Char('o') => self.view = View::Choose { configured: None, typing: Some(String::new()), error: None },
                _ => return !escape && key.code != KeyCode::Char('q'),
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
            View::Running(flow) if flow.digits.is_some() => match key.code {
                KeyCode::Char('y') => {
                    flow.digits = None;
                    flow.waiting = "Adding it…".to_string();
                    if let Some(answer) = flow.answer.take() {
                        let _ = answer.send(true);
                    }
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    flow.digits = None;
                    flow.waiting = "Refusing it…".to_string();
                    if let Some(answer) = flow.answer.take() {
                        let _ = answer.send(false);
                    }
                }
                _ => {}
            },
            View::Running(_) => return !escape,
            View::Overview(Pick::Leaving) => match key.code {
                KeyCode::Char('y') => self.leave(),
                _ => self.view = View::Overview(Pick::Main),
            },
            View::Overview(Pick::Naming(index)) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => *index = index.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *index = (*index + 1).min(devices.saturating_sub(1)),
                KeyCode::Enter => {
                    let index = *index;
                    if let Some(row) = self.rows(crate::messages::now()).into_iter().nth(index) {
                        self.view = View::Label { id: row.id, buffer: if row.named { row.name } else { String::new() } };
                    }
                }
                _ => self.view = View::Overview(Pick::Main),
            },
            View::Label { id, buffer } => match key.code {
                KeyCode::Esc => self.view = View::Overview(Pick::Main),
                KeyCode::Enter => {
                    let said = devices::label(&self.paths.state, id, buffer);
                    self.view = match said {
                        Ok(_) => View::Overview(Pick::Main),
                        Err(error) => View::Said(error),
                    };
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(character) => buffer.push(character),
                _ => {}
            },
            View::Rename { buffer, error } => match key.code {
                KeyCode::Esc => self.view = View::Overview(Pick::Main),
                KeyCode::Enter => match crate::hub::rename_machine(&self.paths.config, buffer) {
                    Ok(_) => self.view = View::Overview(Pick::Main),
                    Err(why) => *error = Some(why),
                },
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(character) => buffer.push(character),
                _ => {}
            },
            View::Overview(Pick::Removing(index)) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => *index = index.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *index = (*index + 1).min(devices.saturating_sub(1)),
                KeyCode::Char('y') => {
                    let index = *index;
                    if let Some(row) = self.rows(crate::messages::now()).into_iter().nth(index) {
                        self.remove_device(row);
                    }
                }
                _ => self.view = View::Overview(Pick::Main),
            },
            View::Overview(Pick::Details) => match key.code {
                KeyCode::Char('?') => self.view = View::Overview(Pick::Main),
                _ => return !escape && key.code != KeyCode::Char('q'),
            },
            View::Overview(Pick::Main) => match key.code {
                KeyCode::Char('a') => self.add_device(),
                KeyCode::Char('n') if devices > 0 => {
                    let first = self.rows(crate::messages::now()).iter().position(|row| !row.named).unwrap_or(0);
                    self.view = View::Overview(Pick::Naming(first));
                }
                KeyCode::Char('n') => self.view = View::Said("No device sees this board, so there is none to name.".to_string()),
                KeyCode::Char('m') => {
                    let current = self.configured_name().or_else(|| identity::load(&self.paths.state).ok().flatten().map(|joined| joined.name)).unwrap_or_default();
                    self.view = View::Rename { buffer: current, error: None };
                }
                KeyCode::Char('r') if devices > 0 => self.view = View::Overview(Pick::Removing(0)),
                KeyCode::Char('r') => self.view = View::Said("No device sees this board, so there is none to remove.".to_string()),
                KeyCode::Char('l') => self.view = View::Overview(Pick::Leaving),
                KeyCode::Char('?') => self.view = View::Overview(Pick::Details),
                _ => return !escape && key.code != KeyCode::Char('q'),
            },
            View::Said(_) => {
                if !matches!(identity::load(&self.paths.state), Ok(Some(_))) {
                    return false;
                }
                self.view = View::Overview(Pick::Main);
                self.refetch();
            }
        }
        true
    }

    pub(super) fn typing(&self) -> bool {
        matches!(self.view, View::Choose { typing: Some(_), .. } | View::Name { .. } | View::Label { .. } | View::Rename { .. })
    }

    pub(super) fn paste(&mut self, text: &str) -> bool {
        match &mut self.view {
            View::Choose { typing: Some(buffer), .. } | View::Name { buffer, .. } | View::Label { buffer, .. } | View::Rename { buffer, .. } => {
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

fn heading(line: &str) -> Line<'static> {
    Line::from(Span::styled(line.to_string(), bold(PEACH)))
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

fn option(key: &str, what: String) -> Line<'static> {
    Line::from(vec![Span::styled(format!("{key}  "), style(MAUVE)), Span::styled(what, style(TEXT))])
}

pub(super) fn qr_lines(link: &str, truecolor: bool) -> Vec<Line<'static>> {
    let paper = if truecolor { Style::new().fg(Color::Rgb(0, 0, 0)).bg(Color::Rgb(255, 255, 255)) } else { Style::new() };
    qr::lines(link).unwrap_or_default().into_iter().map(|row| Line::from(Span::styled(row, paper))).collect()
}

fn status(sent: Sent, said: String) -> Line<'static> {
    let dot = match sent {
        Sent::Yes => GREEN,
        Sent::Failed => RED,
        Sent::NotYet => YELLOW,
    };
    Line::from(vec![Span::styled("● ", style(dot)), Span::styled(said, style(TEXT))])
}

fn device_lines(rows: &[Row], selected: Option<usize>) -> Vec<Line<'static>> {
    let column = rows.iter().map(|row| text_width(&row.name)).max().unwrap_or(0) + 2;
    rows.iter().enumerate().map(|(index, row)| {
        let chosen = selected == Some(index);
        let name = format!("{}{}", row.name, " ".repeat(column.saturating_sub(text_width(&row.name))));
        Line::from(vec![
            Span::styled(if chosen { "▸ " } else { "  " }, style(MAUVE)),
            Span::styled(name, if chosen { bold(MAUVE) } else if row.named { style(TEXT) } else { style(MUTED) }),
            Span::styled(row.facts.clone(), style(MUTED)),
        ])
    }).collect()
}

fn overview(window: &Window, pick: Pick) -> Vec<Line<'static>> {
    let Ok(Some(joined)) = identity::load(&window.paths.state) else { return vec![muted("This machine is not connected."), keys(&[("any key", "close")])] };
    let now = crate::messages::now();
    let relay = relay::load(&window.paths.state);
    let rows = window.rows(now);
    let (sent, said) = board_line(&relay, !rows.is_empty(), now);
    let connected = match window.configured_name().filter(|name| *name != joined.name) {
        Some(name) => Line::from(vec![Span::styled(format!("Connected to {} as {name}", host(&joined.url)), style(TEXT)), Span::styled("  renaming on the hub…", style(MUTED))]),
        None => text(format!("Connected to {} as {}", host(&joined.url), joined.name)),
    };
    let mut lines = vec![connected, status(sent, said)];
    if relay.plan_inactive.is_some() {
        lines.push(muted("Everything here keeps working; your devices get the board again once it is active."));
    }
    lines.push(Line::default());
    if pick == Pick::Details {
        lines.push(heading("Details"));
        let column = rows.iter().map(|row| text_width(&row.name)).chain([text_width("Machine id")]).max().unwrap_or(0) + 2;
        let fact = |label: &str, value: String| Line::from(vec![Span::styled(format!("{label}{}", " ".repeat(column.saturating_sub(text_width(label)))), style(MUTED)), Span::styled(value, style(TEXT))]);
        lines.push(fact("Hub", joined.url.clone()));
        lines.push(fact("Machine id", joined.machine.to_string()));
        for row in &rows {
            lines.push(fact(&row.name, row.id.to_string()));
        }
        lines.push(muted(match (&window.listed.error, window.listed.fetched_at) {
            (Some(error), _) => format!("The hub did not list the devices: {error}"),
            (None, Some(at)) => format!("Devices listed by the hub {}", devices::ago(at, now)),
            (None, None) => "Asking the hub for the devices…".to_string(),
        }));
        lines.push(Line::default());
        lines.push(keys(&[("?", "back"), ("esc", "close")]));
        return lines;
    }
    let mut title = vec![Span::styled("Devices that see this board", bold(PEACH))];
    if window.listed.outdated {
        title.push(Span::styled("  update the hub to see device names", style(MUTED)));
    } else if window.listed.error.is_some() {
        let age = window.listed.fetched_at.map_or("the hub did not answer".to_string(), |at| format!("as of {}; the hub did not answer", devices::ago(at, now)));
        title.push(Span::styled(format!("  {age}"), style(MUTED)));
    }
    lines.push(Line::from(title));
    if rows.is_empty() {
        lines.push(muted("  None yet. Add your phone or a browser with a."));
    }
    let selected = match pick {
        Pick::Removing(index) | Pick::Naming(index) => Some(index.min(rows.len().saturating_sub(1))),
        _ => None,
    };
    lines.extend(device_lines(&rows, selected));
    lines.push(Line::default());
    match (pick, selected.and_then(|index| rows.get(index))) {
        (Pick::Removing(_), Some(row)) => {
            lines.push(Line::from(Span::styled(format!("Remove {}? It stops seeing this board.", row.name), style(YELLOW))));
            lines.push(keys(&[("y", "remove"), ("↑↓", "another"), ("esc", "keep it")]));
        }
        (Pick::Naming(_), Some(row)) => {
            lines.push(Line::from(Span::styled(format!("Name {} on this machine?", row.name), style(YELLOW))));
            lines.push(keys(&[("⏎", "name it"), ("↑↓", "another"), ("esc", "back")]));
        }
        (Pick::Leaving, _) => {
            lines.push(Line::from(Span::styled("Disconnect this machine? Your devices lose its board and the keys go.", style(YELLOW))));
            lines.push(keys(&[("y", "disconnect"), ("any other key", "keeps it")]));
        }
        _ => lines.push(keys(&[("a", "add"), ("n", "name device"), ("m", "rename machine"), ("r", "remove"), ("l", "disconnect"), ("esc", "close")])),
    }
    lines
}

fn running(frame: &Frame, flow: &Flow, truecolor: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(link) = &flow.link {
        lines.push(text(if flow.code.is_some() { "Open this link in your browser, or scan the code with your phone:" } else { "Scan the code with your phone, or open this link in a browser:" }));
        lines.push(Line::from(Span::styled(link.clone(), style(TEXT))));
        if let Some(code) = &flow.code {
            lines.push(Line::from(vec![Span::styled("Approve it only when the page shows ", style(TEXT)), Span::styled(code.clone(), style(PEACH)), Span::styled(".", style(TEXT))]));
        }
    }
    match &flow.digits {
        Some(digits) => {
            lines.push(Line::default());
            lines.push(Line::from(vec![Span::styled("Does your phone show ", style(TEXT)), Span::styled(digits.clone(), bold(PEACH)), Span::styled("?", style(TEXT))]));
            lines.push(keys(&[("y", "yes, add it"), ("n", "no")]));
        }
        None => {
            lines.push(muted(flow.waiting.clone()));
        }
    }
    if let Some(link) = flow.link.as_deref().filter(|_| flow.digits.is_none()) {
        let code = qr_lines(link, truecolor);
        let room = frame.area().height.saturating_sub(7) as usize;
        if code.len() + lines.len() < room {
            lines.push(Line::default());
            lines.extend(code);
        } else {
            lines.push(muted("A taller terminal shows a QR code of the link here."));
        }
    }
    if flow.digits.is_none() {
        lines.push(Line::default());
        lines.push(keys(&[("esc", "stop")]));
    }
    lines
}

fn choose(configured: Option<&String>, error: Option<&String>) -> Vec<Line<'static>> {
    let mut lines = vec![text("Show this board on your phone or in a browser."), Line::default()];
    match configured {
        Some(url) => lines.push(text(format!("Connect to {}, as [hub] url says.", host(url)))),
        None => {
            lines.push(heading("Choose a hub"));
            lines.push(option("k", format!("kelpie hub ({}): sign in or sign up in your browser", host(DEFAULT_HUB_URL))));
            lines.push(option("o", "your own hub, at its address".to_string()));
        }
    }
    lines.extend(error.map(|error| Line::from(Span::styled(error.clone(), style(RED)))));
    lines.push(Line::default());
    lines.push(match configured {
        Some(_) => keys(&[("⏎", "connect"), ("esc", "close")]),
        None => keys(&[("k", "kelpie hub"), ("o", "your own"), ("esc", "close")]),
    });
    lines
}

pub(super) fn draw(frame: &mut Frame, window: &Window) {
    let (title, lines) = match &window.view {
        View::Choose { typing: Some(buffer), .. } => ("kelpie hub · your own", vec![text("Your hub's address, like https://hub.example.com:"), typed(buffer), Line::default(), keys(&[("⏎", "test it and save it as [hub] url"), ("esc", "back")])]),
        View::Choose { configured, error, .. } => ("kelpie hub", choose(configured.as_ref(), error.as_ref())),
        View::Name { buffer, url, .. } => ("kelpie hub · name this machine", vec![text(format!("The name {} and your devices show for this machine:", host(url))), typed(buffer), Line::default(), keys(&[("⏎", "connect and save it as [machine] name"), ("esc", "close")])]),
        View::Running(flow) => (flow.title, running(frame, flow, window.paths.truecolor)),
        View::Overview(pick) => ("kelpie hub", overview(window, *pick)),
        View::Label { buffer, .. } => ("kelpie hub · name a device", vec![text("The name this machine shows for the device (the hub has no way to rename it):"), typed(buffer), Line::default(), keys(&[("⏎", "save it on this machine"), ("esc", "back")])]),
        View::Rename { buffer, error } => ("kelpie hub · rename this machine", [text("The name the hub and your devices show for this machine:"), typed(buffer)].into_iter().chain(error.iter().map(|error| Line::from(Span::styled(error.clone(), style(RED))))).chain([Line::default(), keys(&[("⏎", "save it as [machine] name"), ("esc", "back")])]).collect()),
        View::Said(said) => ("kelpie hub", said.lines().map(text).chain([Line::default(), keys(&[("any key", "goes on")])]).collect()),
    };
    modal(frame, title.to_string(), lines, WIDTH);
}

impl App {
    pub(super) fn open_hub(&mut self) {
        let paths = match &self.hub_paths {
            Some(paths) => paths.clone(),
            None => match crate::lead::state_dir() {
                Ok(state) => Paths { state, config: crate::config::path(), post: crate::hub::tick, fetch: devices::fetch, opener: self.viewer, truecolor: qr::truecolor() },
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
    use std::time::{Duration, Instant};

    use kelpie_hub_api::AdmittedDevice;
    use kelpie_hub_protocol::{Prefix, SecretKeys};

    use super::*;
    use crate::board::test_support::*;

    fn cached(state: &Path, _: u64) -> Result<Listed> {
        Ok(devices::load(state))
    }

    fn unreachable(_: &Path, _: u64) -> Result<Listed> {
        Err("could not reach the hub: connection refused".to_string())
    }

    fn paths(config: &str) -> Paths {
        let state = crate::sh::test_dir("board-hub");
        let file = state.join("config.toml");
        std::fs::write(&file, config).unwrap();
        Paths { state, config: file, post: || Ok(()), fetch: cached, opener: None, truecolor: true }
    }

    fn screen(app: &mut App, width: u16, height: u16) -> String {
        render(app, width, height).0.join("\n")
    }

    fn colour_of(app: &mut App, needle: &str) -> Color {
        let (rows, buffer) = render(app, 110, 40);
        let (y, row) = rows.iter().enumerate().find(|(_, row)| row.contains(needle)).unwrap_or_else(|| panic!("{needle} is not shown: {}", rows.join("\n")));
        let x = row[..row.find(needle).unwrap()].chars().count();
        buffer[(x as u16, y as u16)].fg
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

    fn fetched(app: &mut App) {
        until(app, "the device list", |window| window.fetching.is_none());
    }

    struct Joined {
        paths: Paths,
        phone: Id,
        laptop: Id,
    }

    fn joined(fetch: Fetch, relay: relay::Relay) -> Joined {
        let mut paths = paths("[machine]\nname = \"mac\"\n");
        paths.fetch = fetch;
        let now = crate::messages::now();
        let mut machine = identity::Identity::new("https://kelpie.example.com", Id::new(Prefix::Machine).unwrap(), "mac", "khm_test".to_string(), &SecretKeys::generate(), now).unwrap();
        let (laptop, phone) = (Id::new(Prefix::Device).unwrap(), Id::new(Prefix::Device).unwrap());
        for id in [&laptop, &phone] {
            machine.admit(identity::Device { id: id.clone(), keys: SecretKeys::generate().public(), paired_at: now, label: None });
        }
        identity::save(&paths.state, &machine).unwrap();
        let listed = Listed { devices: vec![
            AdmittedDevice { device: laptop.clone(), name: Some("Safari on Mac".to_string()), paired_at: now, last_seen: Some(now - 120), notifications: false },
            AdmittedDevice { device: phone.clone(), name: Some("iPhone".to_string()), paired_at: now, last_seen: None, notifications: true },
        ], fetched_at: Some(now - 300), error: None, outdated: false };
        std::fs::write(identity::dir(&paths.state).join("devices.json"), serde_json::to_string(&listed).unwrap()).unwrap();
        std::fs::write(identity::dir(&paths.state).join("relay.json"), serde_json::to_string(&relay).unwrap()).unwrap();
        Joined { paths, phone, laptop }
    }

    fn opened(joined: &Joined) -> App {
        let mut app = test_app(Vec::new());
        app.hub_paths = Some(joined.paths.clone());
        press(&mut app, key(KeyCode::Char('H')));
        fetched(&mut app);
        app
    }

    #[test]
    fn not_connected_the_window_offers_the_kelpie_hub_or_your_own_with_the_keys_in_the_keys_windows_colours() {
        let mut app = test_app(Vec::new());
        app.hub_paths = Some(paths(""));
        press(&mut app, key(KeyCode::Char('H')));
        let shown = screen(&mut app, 110, 40);
        for line in ["Show this board on your phone or in a browser.", "Choose a hub", &format!("k  kelpie hub ({}): sign in or sign up in your browser", host(DEFAULT_HUB_URL)), "o  your own hub, at its address", "k kelpie hub · o your own · esc close"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        assert_eq!(colour_of(&mut app, "k kelpie hub ·"), MAUVE);
        assert_eq!(colour_of(&mut app, "Choose a hub"), PEACH);
        press(&mut app, key(KeyCode::Char('o')));
        for character in "https://hub.example.com".chars() {
            press(&mut app, key(KeyCode::Char(character)));
        }
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("kelpie hub · your own") && shown.contains("https://hub.example.com▏") && shown.contains("⏎ test it and save it as [hub] url · esc back"), "{shown}");
        press(&mut app, key(KeyCode::Esc));
        assert!(screen(&mut app, 110, 40).contains("o  your own hub"), "esc goes back to the choice");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal), "a second esc closes the window");

        app.hub_paths = Some(paths("[hub]\nurl = \"https://hub.example.com\"\n"));
        press(&mut app, key(KeyCode::Char('H')));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Connect to hub.example.com, as [hub] url says.") && shown.contains("⏎ connect · esc close") && !shown.contains("your own"), "{shown}");
        press(&mut app, key(KeyCode::Enter));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("kelpie hub · name this machine") && shown.contains(&format!("{}▏", crate::config::short_hostname())), "an unset name is asked for once, with the host's: {shown}");
    }

    #[test]
    fn a_running_step_shows_the_link_its_code_and_a_true_black_on_white_qr_code_then_the_digits_to_answer() {
        let mut app = test_app(Vec::new());
        let link = "http://localhost:8470/connect?code=K7QF-M2XD#k=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&m=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".to_string();
        let shown_link = link.clone();
        let flow = spawn("kelpie hub · connect", "Waiting for the browser…".to_string(), move |events, _, answers| {
            let _ = events.send(Event::Shown { link: shown_link, code: Some("K7QF-M2XD".to_string()) });
            let _ = events.send(Event::Digits("123456".to_string()));
            Event::Done(Ok(format!("answered {}", answers.recv().unwrap_or(false))))
        });
        app.mode = Mode::Hub(Box::new(Window { paths: paths(""), view: View::Running(flow), listed: Listed::default(), fetching: None }));
        until(&mut app, "the link", |window| matches!(&window.view, View::Running(flow) if flow.digits.is_some()));
        let View::Running(flow) = &mut window(&mut app).view else { unreachable!() };
        let digits = flow.digits.take();
        let shown = screen(&mut app, 110, 50);
        assert!(shown.contains("Approve it only when the page shows K7QF-M2XD.") && shown.contains("http://localhost:8470/connect?code=K7QF-M2XD") && shown.contains("esc stop"), "{shown}");
        let (rows, buffer) = render(&mut app, 110, 50);
        let (x, y) = rows.iter().enumerate().find_map(|(y, row)| row.find('█').map(|byte| (row[..byte].chars().count(), y))).unwrap_or_else(|| panic!("no QR code: {shown}"));
        let cell = &buffer[(x as u16, y as u16)];
        assert_eq!((cell.fg, cell.bg), (Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255)), "true black on white, which no theme remaps");
        let drawn = rows.iter().filter(|row| row.contains('█')).count();
        assert_eq!(drawn, qr::lines(&link).unwrap().iter().filter(|row| row.contains('█')).count(), "the whole code is drawn, two modules a row");
        window(&mut app).paths.truecolor = false;
        let (rows, buffer) = render(&mut app, 110, 50);
        let (x, y) = rows.iter().enumerate().find_map(|(y, row)| row.find('█').map(|byte| (row[..byte].chars().count(), y))).unwrap();
        assert_eq!((buffer[(x as u16, y as u16)].fg, buffer[(x as u16, y as u16)].bg), (Color::Reset, Color::Reset), "without 24-bit colour the terminal's own colours");
        let short = screen(&mut app, 110, 20);
        assert!(short.contains("A taller terminal shows a QR code") && !short.contains('█'), "{short}");

        let View::Running(flow) = &mut window(&mut app).view else { unreachable!() };
        flow.digits = digits;
        let shown = screen(&mut app, 110, 50);
        assert!(shown.contains("Does your phone show 123456?") && shown.contains("y yes, add it · n no") && !shown.contains('█'), "{shown}");
        press(&mut app, key(KeyCode::Char('y')));
        until(&mut app, "the outcome", |window| matches!(window.view, View::Said(_)));
        assert!(screen(&mut app, 110, 50).contains("answered true"));
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Normal), "a machine still not connected closes the window after the outcome");
    }

    #[test]
    fn connected_the_window_names_the_hub_the_machine_a_green_sent_board_and_each_device_by_name() {
        let now = crate::messages::now();
        let joined = joined(cached, relay::Relay { posted_at: Some(now - 150), rev: 3, ..Default::default() });
        let mut app = opened(&joined);
        let shown = screen(&mut app, 110, 40);
        for line in ["Connected to kelpie.example.com as mac", "● Board is being sent", "Devices that see this board", "Safari on Mac  paired today · seen 2 min ago", "iPhone         paired today · notifications on", "a add · n name device · m rename machine · r remove · l disconnect · esc close"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        assert!(!shown.contains(joined.phone.as_str()) && !shown.contains("https://") && !shown.contains("mch_"), "no ids and no scheme: {shown}");
        assert_eq!(colour_of(&mut app, "● Board is being sent"), GREEN);
        assert_eq!(colour_of(&mut app, "Devices that see"), PEACH);
        assert_eq!(colour_of(&mut app, "a add ·"), MAUVE);
        assert_eq!(colour_of(&mut app, "paired today · seen"), MUTED);
    }

    #[test]
    fn the_board_line_is_red_with_the_reason_when_posts_fail_and_amber_before_the_first() {
        let now = crate::messages::now();
        let plan = "Your kelpie hub subscription isn't active: https://billing.example.com/start".to_string();
        let mut app = opened(&joined(cached, relay::Relay { posted_at: Some(now - 600), plan_inactive: Some(plan.clone()), error: Some(plan.clone()), error_at: Some(now - 5), ..Default::default() }));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains(&format!("● {plan}")) && shown.contains("Everything here keeps working"), "{shown}");
        assert_eq!(colour_of(&mut app, "● Your kelpie hub"), RED);
        app.hub_banner = Some(plan.clone());
        press(&mut app, key(KeyCode::Esc));
        assert!(screen(&mut app, 110, 40).contains(&format!("{plan} · H hub")), "the board shows a lapsed plan too");

        let mut app = opened(&joined(cached, relay::Relay { posted_at: Some(now - 600), error: Some("could not reach the hub: connection refused".to_string()), error_at: Some(now - 120), ..Default::default() }));
        assert!(screen(&mut app, 110, 40).contains("● Board not sent: the hub can't be reached (2 min ago)"));
        assert_eq!(colour_of(&mut app, "● Board not sent"), RED);
        let mut app = opened(&joined(cached, relay::Relay { posted_at: Some(now - 600), error: Some("the hub answered 400 not-a-board".to_string()), error_at: Some(now - 120), ..Default::default() }));
        assert!(screen(&mut app, 110, 40).contains("● Board not sent: the hub answered 400 not-a-board (2 min ago)"));
        let mut app = opened(&joined(cached, relay::Relay { posted_at: Some(now - 150), error: Some("could not reach the hub: once".to_string()), error_at: Some(now - 300), ..Default::default() }));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("● Board is being sent") && !shown.contains("Board sent"), "a post after the failure wins, and a board that goes out shows no ticking time: {shown}");

        let mut app = opened(&joined(cached, relay::Relay::default()));
        assert!(screen(&mut app, 110, 40).contains("● Board not sent yet; the keeper sends it within a minute"));
        assert_eq!(colour_of(&mut app, "● Board not sent yet"), YELLOW);
    }

    #[test]
    fn an_unreachable_hub_shows_the_last_list_with_its_age() {
        let mut app = opened(&joined(unreachable, relay::Relay::default()));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Devices that see this board  as of 5 min ago; the hub did not answer") && shown.contains("Safari on Mac"), "{shown}");
    }

    #[test]
    fn question_mark_shows_the_hub_url_the_machine_id_and_each_device_id_and_toggles_back() {
        let joined = joined(cached, relay::Relay::default());
        let mut app = opened(&joined);
        let machine = identity::load(&joined.paths.state).unwrap().unwrap().machine;
        press(&mut app, key(KeyCode::Char('?')));
        let shown = screen(&mut app, 110, 40);
        for line in ["Details", "Hub            https://kelpie.example.com", &format!("Machine id     {machine}"), &format!("Safari on Mac  {}", joined.laptop), &format!("iPhone         {}", joined.phone), "Devices listed by the hub 5 min ago", "? back · esc close"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        press(&mut app, key(KeyCode::Char('?')));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Devices that see this board") && !shown.contains(joined.phone.as_str()), "{shown}");
        press(&mut app, key(KeyCode::Char('l')));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Disconnect this machine? Your devices lose its board and the keys go.") && shown.contains("y disconnect · any other key keeps it"), "{shown}");
        press(&mut app, key(KeyCode::Char('n')));
        assert!(screen(&mut app, 110, 40).contains("l disconnect · esc close"), "anything but y keeps it");
    }

    #[test]
    fn r_picks_a_device_with_the_arrows_and_y_removes_it_from_this_machine_on_the_hub() {
        let hub = crate::hub::tests::open_hub(|_| {});
        let mut browser = crate::hub::tests::member(&hub);
        let state = crate::hub::tests::connected_state(&hub, &mut browser);
        let mut app = test_app(Vec::new());
        let config = state.join("config.toml");
        std::fs::write(&config, "[machine]\nname = \"mac\"\n").unwrap();
        app.hub_paths = Some(Paths { state: state.clone(), config, post: || Ok(()), fetch: devices::fetch, opener: None, truecolor: true });
        press(&mut app, key(KeyCode::Char('H')));
        fetched(&mut app);
        assert!(screen(&mut app, 110, 40).contains("browser  paired today"), "the name the hub holds");
        press(&mut app, key(KeyCode::Char('r')));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("▸ browser") && shown.contains("Remove browser? It stops seeing this board.") && shown.contains("y remove · ↑↓ another · esc keep it"), "{shown}");
        assert_eq!(colour_of(&mut app, "browser  paired"), MAUVE, "the picked device");
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Char('y')));
        until(&mut app, "the removal", |window| matches!(window.view, View::Said(_)));
        assert!(screen(&mut app, 110, 40).contains("Removed browser; it no longer sees this board."));
        assert!(identity::load(&state).unwrap().unwrap().devices.is_empty(), "the machine seals to it no more");
        let machine = identity::load(&state).unwrap().unwrap().machine;
        assert!(hub.hub.store().admitted_device_views(&machine).unwrap().is_empty());
        press(&mut app, key(KeyCode::Char('x')));
        fetched(&mut app);
        assert!(screen(&mut app, 110, 40).contains("None yet. Add your phone or a browser with a."));
    }

    #[test]
    fn the_board_connects_a_hub_and_the_approving_browser_reads_the_machines_board() {
        let hub = crate::hub::tests::open_hub(|_| {});
        let mut browser = crate::hub::tests::member(&hub);
        let mut app = test_app(Vec::new());
        let mut paths = paths(&format!("[machine]\nname = \"mac\"\n\n[hub]\nurl = \"{}\"\n", hub.url));
        paths.fetch = devices::fetch;
        let state = paths.state.clone();
        app.hub_paths = Some(paths);
        press(&mut app, key(KeyCode::Char('H')));
        press(&mut app, key(KeyCode::Enter));
        until(&mut app, "the link", |window| matches!(&window.view, View::Running(flow) if flow.link.is_some()));
        let View::Running(flow) = &window(&mut app).view else { unreachable!() };
        let (link, code) = (flow.link.clone().unwrap(), flow.code.clone().unwrap());
        assert!(screen(&mut app, 110, 50).contains(&format!("Approve it only when the page shows {code}.")));
        let approval = hub.runtime.block_on(browser.approve_connect(&link, true)).unwrap();
        hub.runtime.block_on(browser.show_board(approval, crate::hub::tests::WAIT)).unwrap();
        until(&mut app, "the outcome", |window| matches!(window.view, View::Said(_)));
        let View::Said(said) = &window(&mut app).view else { unreachable!() };
        assert_eq!(said, &format!("Connected to {} as mac; the browser that approved it shows its board.", hub.url));
        press(&mut app, key(KeyCode::Char('x')));
        fetched(&mut app);
        let shown = screen(&mut app, 110, 50);
        assert!(shown.contains(&format!("Connected to {} as mac", host(&hub.url))) && shown.contains("browser  paired today"), "the outcome gives way to the overview: {shown}");
        let device = browser.device.as_ref().unwrap().device.clone().unwrap();
        assert_eq!(identity::load(&state).unwrap().unwrap().devices[0].id, device);
    }
    fn outdated(state: &Path, _: u64) -> Result<Listed> {
        Ok(Listed { error: Some("this hub cannot list devices yet; update kelpie-hub".to_string()), outdated: true, ..devices::load(state) })
    }

    #[test]
    fn a_hub_without_the_device_list_asks_for_an_update_and_an_unreachable_one_says_it_did_not_answer() {
        let shown = screen(&mut opened(&joined(outdated, relay::Relay::default())), 110, 40);
        assert!(shown.contains("Devices that see this board  update the hub to see device names") && !shown.contains("did not answer"), "{shown}");
        let shown = screen(&mut opened(&joined(unreachable, relay::Relay::default())), 110, 40);
        assert!(shown.contains("the hub did not answer") && !shown.contains("update the hub"), "{shown}");
    }

    #[test]
    fn a_device_without_a_name_reads_unnamed_and_n_names_it_on_this_machine() {
        let joined = joined(cached, relay::Relay::default());
        let mut listed = devices::load(&joined.paths.state);
        listed.devices[1].name = None;
        std::fs::write(identity::dir(&joined.paths.state).join("devices.json"), serde_json::to_string(&listed).unwrap()).unwrap();
        let mut app = opened(&joined);
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Unnamed device") && !shown.contains("Device "), "{shown}");
        assert_eq!(colour_of(&mut app, "Unnamed device"), MUTED);
        assert!(shown.lines().filter(|row| row.contains("esc close")).count() == 1 && shown.contains("a add · n name device · m rename machine · r remove · l disconnect · esc close"), "the keys stay on one line: {shown}");
        press(&mut app, key(KeyCode::Char('n')));
        assert!(screen(&mut app, 110, 40).contains("Name Unnamed device on this machine?"), "n picks the first device without a name");
        press(&mut app, key(KeyCode::Enter));
        for character in "Kitchen iPad".chars() {
            press(&mut app, key(KeyCode::Char(character)));
        }
        press(&mut app, key(KeyCode::Enter));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Kitchen iPad   paired today") && !shown.contains("Unnamed"), "{shown}");
        let saved = identity::load(&joined.paths.state).unwrap().unwrap();
        assert_eq!(saved.devices.iter().find(|device| device.id == joined.phone).and_then(|device| device.label.as_deref()), Some("Kitchen iPad"), "kept in identity.json, as the hub has no machine-side rename");
        assert_eq!(devices::find(&devices::load(&joined.paths.state), &saved.devices, "kitchen ipad").unwrap(), joined.phone, "kelpie hub remove finds it by that name too");
    }

    #[test]
    fn m_renames_this_machine_in_config_toml_and_shows_the_name_at_once_until_the_hub_has_it() {
        let joined = joined(cached, relay::Relay::default());
        let mut app = opened(&joined);
        press(&mut app, key(KeyCode::Char('m')));
        assert!(screen(&mut app, 110, 40).contains("mac▏"), "prefilled with [machine] name");
        for _ in 0..3 {
            press(&mut app, key(KeyCode::Backspace));
        }
        press(&mut app, key(KeyCode::Enter));
        assert!(screen(&mut app, 110, 40).contains("machine.name: must be a non-empty string"), "the config's own check");
        for character in "Studio Mac".chars() {
            press(&mut app, key(KeyCode::Char(character)));
        }
        press(&mut app, key(KeyCode::Enter));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Connected to kelpie.example.com as Studio Mac  renaming on the hub…"), "{shown}");
        assert_eq!(crate::config::load_machine(&joined.paths.config).unwrap().machine_name.as_deref(), Some("Studio Mac"));
        let mut renamed = identity::load(&joined.paths.state).unwrap().unwrap();
        renamed.name = "Studio Mac".to_string();
        identity::save(&joined.paths.state, &renamed).unwrap();
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("as Studio Mac") && !shown.contains("renaming"), "the keeper's rename confirmed it: {shown}");
    }

    #[test]
    fn m_refuses_to_write_through_a_linked_config_and_says_where_to_set_it() {
        let joined = joined(cached, relay::Relay::default());
        let real = joined.paths.state.join("team.toml");
        std::fs::rename(&joined.paths.config, &real).unwrap();
        std::os::unix::fs::symlink(&real, &joined.paths.config).unwrap();
        let mut app = opened(&joined);
        press(&mut app, key(KeyCode::Char('m')));
        press(&mut app, key(KeyCode::Char('2')));
        press(&mut app, key(KeyCode::Enter));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("is a link") && shown.contains("[machine] name = \"mac2\""), "{shown}");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "[machine]\nname = \"mac\"\n", "the linked file is left as it was");
    }

}
