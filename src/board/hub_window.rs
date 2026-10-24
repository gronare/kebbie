use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::config::DEFAULT_HUB_URL;
use crate::hub::connect::{self, hub_name};
use crate::hub::{identity, pair, qr, relay};
use crate::sh::Result;
use super::widgets::{MUTED, PEACH, RED, TEXT, YELLOW, modal, style};
use super::{App, Mode};

pub(super) type Post = fn() -> Result<()>;

#[derive(Clone)]
pub(super) struct Paths {
    pub(super) state: PathBuf,
    pub(super) config: PathBuf,
    pub(super) post: Post,
    pub(super) opener: Option<&'static str>,
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

pub(super) enum View {
    Choose { configured: Option<String>, typing: Option<String>, error: Option<String> },
    Name { url: String, note: Option<String>, buffer: String },
    Running(Flow),
    Overview { leaving: bool },
    Said(String),
}

pub(super) struct Window {
    pub(super) paths: Paths,
    pub(super) view: View,
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

impl Window {
    pub(super) fn open(paths: Paths) -> Window {
        let view = match identity::load(&paths.state) {
            Ok(Some(_)) => View::Overview { leaving: false },
            Ok(None) => match connect::configured_hub(&paths.config) {
                Ok(configured) => View::Choose { configured, typing: None, error: None },
                Err(error) => View::Said(error),
            },
            Err(error) => View::Said(error),
        };
        Window { paths, view }
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
        let waiting = format!("Waiting for the browser to approve it at {}…", hub_name(&url));
        self.view = View::Running(spawn("Connect a hub", waiting, move |events, stop, _| {
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
        self.view = View::Running(spawn("Connect a hub", format!("Testing {address}…"), move |_, _, _| Event::Tested(connect::own_hub(&config, &address))));
    }

    fn add_device(&mut self) {
        let paths = self.paths.clone();
        self.view = View::Running(spawn("Add a phone or browser", "Waiting for the phone to open the link…".to_string(), move |events, stop, answers| {
            let result = pair::add_device(&paths.state, pair::TIMEOUT, |added| {
                let _ = events.send(Event::Shown { link: added.url.clone(), code: None });
            }, |digits| {
                let _ = events.send(Event::Digits(digits.to_string()));
                answers.recv().unwrap_or(false)
            }, Some(stop), crate::messages::now());
            Event::Done(result.map(|device| {
                let posted = (paths.post)().err().map_or(String::new(), |error| format!(" The first board did not go out yet: {error}."));
                format!("Added {device}; it shows this machine's board from now on.{posted}")
            }))
        }));
    }

    fn leave(&mut self) {
        let state = self.paths.state.clone();
        self.view = View::Running(spawn("Leave the hub", "Taking the phones off and forgetting the keys…".to_string(), move |_, _, _| Event::Done(crate::hub::leave(&state))));
    }

    pub(super) fn poll(&mut self) {
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
            View::Overview { leaving: true, .. } => match key.code {
                KeyCode::Char('y') => self.leave(),
                _ => self.view = View::Overview { leaving: false },
            },
            View::Overview { leaving: false, .. } => match key.code {
                KeyCode::Char('a') => self.add_device(),
                KeyCode::Char('l') => self.view = View::Overview { leaving: true },
                _ => return !escape && key.code != KeyCode::Char('q'),
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
        matches!(self.view, View::Choose { typing: Some(_), .. } | View::Name { .. })
    }

    pub(super) fn paste(&mut self, text: &str) -> bool {
        match &mut self.view {
            View::Choose { typing: Some(buffer), .. } | View::Name { buffer, .. } => {
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

// A phone's camera wants dark modules on light whatever the terminal's colours, so the code is drawn black on white.
pub(super) fn qr_lines(link: &str) -> Vec<Line<'static>> {
    let paper = Style::new().fg(Color::Black).bg(Color::White);
    qr::lines(link).unwrap_or_default().into_iter().map(|row| Line::from(Span::styled(row, paper))).collect()
}

fn overview(paths: &Paths, leaving: bool) -> Vec<Line<'static>> {
    let Ok(Some(identity)) = identity::load(&paths.state) else { return vec![muted("Not connected to a hub."), muted("any key closes")] };
    let now = crate::messages::now();
    let mut lines = vec![text(format!("hub      {}", hub_name(&identity.url))), text(format!("machine  {} ({})", identity.name, identity.machine))];
    if let Some(plan) = relay::load(&paths.state).plan_inactive {
        lines.push(Line::from(Span::styled(plan, style(YELLOW))));
        lines.push(muted("Everything on this machine keeps working; the phone gets the board again once it is active."));
    }
    lines.push(Line::default());
    if identity.devices.is_empty() {
        lines.push(muted("No phone or browser shows this machine's board yet."));
    }
    for device in &identity.devices {
        lines.push(text(format!("device   {} paired {} ago", device.id, crate::keeper::span(now.saturating_sub(device.paired_at)))));
    }
    lines.push(Line::default());
    lines.push(if leaving {
        Line::from(Span::styled("Leave the hub? Each phone loses this machine's board and the keys go. y leave · any other key keeps it", style(YELLOW)))
    } else {
        muted("a add a phone or browser · l leave the hub · esc close")
    });
    lines
}

fn running(frame: &Frame, flow: &Flow) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(link) = &flow.link {
        lines.push(text(if flow.code.is_some() { "Open this link in your browser, or scan the code with your phone:" } else { "On your phone, scan the code, or open this link in a browser, and add it:" }));
        lines.push(text(link.clone()));
        if let Some(code) = &flow.code {
            lines.push(Line::from(vec![Span::styled("The page shows ", style(TEXT)), Span::styled(code.clone(), style(PEACH)), Span::styled("; approve it only when it does.", style(TEXT))]));
        }
    }
    match &flow.digits {
        Some(digits) => lines.push(Line::from(vec![Span::styled("Does the phone show ", style(TEXT)), Span::styled(digits.clone(), style(PEACH)), Span::styled("?  y yes, add it · n no", style(TEXT))])),
        None => lines.push(muted(format!("{}  esc stops", flow.waiting))),
    }
    if let Some(link) = flow.link.as_deref().filter(|_| flow.digits.is_none()) {
        let code = qr_lines(link);
        let room = frame.area().height.saturating_sub(6) as usize;
        if code.len() + lines.len() < room {
            lines.push(Line::default());
            lines.extend(code);
        } else {
            lines.push(muted("A taller terminal shows a QR code of the link here."));
        }
    }
    lines
}

pub(super) fn draw(frame: &mut Frame, window: &Window) {
    let (title, lines) = match &window.view {
        View::Choose { configured: Some(url), error, .. } => {
            let mut lines = vec![text("Show this machine's board on your phone through a kelpie hub."), Line::default(), text(format!("[hub] url names {}.", hub_name(url)))];
            lines.extend(error.iter().map(|error| Line::from(Span::styled(error.clone(), style(RED)))));
            lines.push(Line::default());
            lines.push(muted("⏎ connect there · esc close"));
            ("Connect a hub", lines)
        }
        View::Choose { typing: None, error, .. } => {
            let mut lines = vec![text("Show this machine's board on your phone through a kelpie hub."), Line::default(),
                text(format!("k  the kelpie hub ({DEFAULT_HUB_URL}), with an account you make in the browser")),
                text("o  your own kelpie hub, at its address")];
            lines.extend(error.iter().map(|error| Line::from(Span::styled(error.clone(), style(RED)))));
            lines.push(Line::default());
            lines.push(muted("k or ⏎ the kelpie hub · o your own · esc close"));
            ("Connect a hub", lines)
        }
        View::Choose { typing: Some(buffer), .. } => ("Your own hub", vec![text("The hub's address, such as https://hub.example.com:"), typed(buffer), muted("⏎ tests it and saves it as [hub] url · esc back")]),
        View::Name { buffer, url, .. } => ("Name this machine", vec![text(format!("The name {} and your phone show for this machine:", hub_name(url))), typed(buffer), muted("⏎ connects and saves it as [machine] name · esc close")]),
        View::Running(flow) => (flow.title, running(frame, flow)),
        View::Overview { leaving } => ("kelpie hub", overview(&window.paths, *leaving)),
        View::Said(said) => ("kelpie hub", said.lines().map(text).chain([muted("any key goes on")]).collect()),
    };
    modal(frame, title.to_string(), lines, 76);
}

impl App {
    pub(super) fn open_hub(&mut self) {
        let paths = match &self.hub_paths {
            Some(paths) => paths.clone(),
            None => match crate::lead::state_dir() {
                Ok(state) => Paths { state, config: crate::config::path(), post: crate::hub::tick, opener: self.viewer },
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

    use kelpie_hub_protocol::{Id, Prefix, SecretKeys};

    use super::*;
    use crate::board::test_support::*;

    fn paths(config: &str) -> Paths {
        let state = crate::sh::test_dir("board-hub");
        let file = state.join("config.toml");
        std::fs::write(&file, config).unwrap();
        Paths { state, config: file, post: || Ok(()), opener: None }
    }

    fn screen(app: &mut App, width: u16, height: u16) -> String {
        render(app, width, height).0.join("\n")
    }

    fn window(app: &mut App) -> &mut Window {
        match &mut app.mode {
            Mode::Hub(window) => window,
            _ => panic!("the hub window is not open"),
        }
    }

    fn until(app: &mut App, what: &str, done: impl Fn(&View) -> bool) {
        let started = Instant::now();
        while !done(&window(app).view) {
            assert!(started.elapsed() < crate::hub::tests::WAIT, "{what} never came");
            std::thread::sleep(Duration::from_millis(20));
            app.poll_hub();
        }
    }

    #[test]
    fn h_opens_the_hub_window_with_the_kelpie_hub_or_your_own_when_not_connected() {
        let mut app = test_app(Vec::new());
        app.hub_paths = Some(paths(""));
        press(&mut app, key(KeyCode::Char('H')));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Connect a hub") && shown.contains(&format!("k  the kelpie hub ({DEFAULT_HUB_URL})")) && shown.contains("o  your own kelpie hub"), "{shown}");
        press(&mut app, key(KeyCode::Char('o')));
        for character in "https://hub.example.com".chars() {
            press(&mut app, key(KeyCode::Char(character)));
        }
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Your own hub") && shown.contains("https://hub.example.com▏") && shown.contains("saves it as [hub] url"), "{shown}");
        press(&mut app, key(KeyCode::Esc));
        assert!(screen(&mut app, 110, 40).contains("o  your own kelpie hub"), "esc goes back to the choice");
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal), "a second esc closes the window");

        app.hub_paths = Some(paths("[hub]\nurl = \"https://hub.example.com\"\n"));
        press(&mut app, key(KeyCode::Char('H')));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("[hub] url names https://hub.example.com") && shown.contains("⏎ connect there") && !shown.contains("your own"), "{shown}");
        press(&mut app, key(KeyCode::Enter));
        let shown = screen(&mut app, 110, 40);
        assert!(shown.contains("Name this machine") && shown.contains(&format!("{}▏", crate::config::short_hostname())), "an unset name is asked for once, with the host's: {shown}");
    }

    #[test]
    fn a_running_step_shows_the_link_its_code_and_a_black_on_white_qr_code_then_the_digits_to_answer() {
        let mut app = test_app(Vec::new());
        let link = "http://localhost:8470/connect?code=K7QF-M2XD#k=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&m=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".to_string();
        let shown_link = link.clone();
        let flow = spawn("Connect a hub", "Waiting for the browser…".to_string(), move |events, _, answers| {
            let _ = events.send(Event::Shown { link: shown_link, code: Some("K7QF-M2XD".to_string()) });
            let _ = events.send(Event::Digits("123456".to_string()));
            Event::Done(Ok(format!("answered {}", answers.recv().unwrap_or(false))))
        });
        app.mode = Mode::Hub(Box::new(Window { paths: paths(""), view: View::Running(flow) }));
        until(&mut app, "the link", |view| matches!(view, View::Running(flow) if flow.digits.is_some()));
        let View::Running(flow) = &mut window(&mut app).view else { unreachable!() };
        let digits = flow.digits.take();
        let shown = screen(&mut app, 110, 50);
        assert!(shown.contains("The page shows K7QF-M2XD; approve it only when it does.") && shown.contains("http://localhost:8470/connect?code=K7QF-M2XD"), "{shown}");
        let (rows, buffer) = render(&mut app, 110, 50);
        let (x, y) = rows.iter().enumerate().find_map(|(y, row)| row.find('█').map(|byte| (row[..byte].chars().count(), y))).unwrap_or_else(|| panic!("no QR code: {shown}"));
        let cell = &buffer[(x as u16, y as u16)];
        assert_eq!((cell.fg, cell.bg), (Color::Black, Color::White), "dark modules on light, whatever the terminal's colours");
        assert_eq!(rows.iter().filter(|row| row.contains('█')).count(), qr::lines(&link).unwrap().iter().filter(|row| row.contains('█')).count(), "the whole code is drawn");
        let short = screen(&mut app, 110, 20);
        assert!(short.contains("A taller terminal shows a QR code") && !short.contains('█'), "{short}");

        let View::Running(flow) = &mut window(&mut app).view else { unreachable!() };
        flow.digits = digits;
        let shown = screen(&mut app, 110, 50);
        assert!(shown.contains("Does the phone show 123456?  y yes, add it · n no") && !shown.contains('█'), "{shown}");
        press(&mut app, key(KeyCode::Char('y')));
        until(&mut app, "the outcome", |view| matches!(view, View::Said(_)));
        assert!(screen(&mut app, 110, 50).contains("answered true"));
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Normal), "a machine still not connected closes the window after the outcome");
    }

    #[test]
    fn connected_the_window_shows_the_hub_the_machine_its_devices_and_a_lapsed_plan_which_the_board_shows_too() {
        let mut app = test_app(Vec::new());
        let paths = paths("[machine]\nname = \"mac\"\n");
        let mut joined = identity::Identity::new("https://hub.example.com", Id::new(Prefix::Machine).unwrap(), "mac", "khm_test".to_string(), &SecretKeys::generate(), crate::messages::now()).unwrap();
        let device = Id::new(Prefix::Device).unwrap();
        joined.admit(identity::Device { id: device.clone(), keys: SecretKeys::generate().public(), paired_at: crate::messages::now() - 120 });
        identity::save(&paths.state, &joined).unwrap();
        let plan = "Your kelpie hub subscription isn't active: https://billing.example.com/start".to_string();
        std::fs::write(identity::dir(&paths.state).join("relay.json"), serde_json::to_string(&relay::Relay { plan_inactive: Some(plan.clone()), ..Default::default() }).unwrap()).unwrap();
        app.hub_paths = Some(paths);
        press(&mut app, key(KeyCode::Char('H')));
        let shown = screen(&mut app, 110, 40);
        for line in ["hub      https://hub.example.com", "machine  mac (", &format!("device   {device} paired 2m ago"), "Your kelpie hub subscription isn't active:", "https://billing.example.com/start", "a add a phone or browser · l leave the hub · esc close"] {
            assert!(shown.contains(line), "{line}: {shown}");
        }
        press(&mut app, key(KeyCode::Char('l')));
        assert!(screen(&mut app, 110, 40).contains("Leave the hub? Each phone loses this machine's board"));
        press(&mut app, key(KeyCode::Char('n')));
        assert!(screen(&mut app, 110, 40).contains("l leave the hub"), "anything but y keeps it");
        press(&mut app, key(KeyCode::Esc));
        app.hub_banner = Some(plan.clone());
        let board = screen(&mut app, 110, 40);
        assert!(board.contains(&format!("{plan} · H hub")), "{board}");
    }

    #[test]
    fn the_board_connects_a_hub_and_the_approving_browser_reads_the_machines_board() {
        let hub = crate::hub::tests::open_hub(|_| {});
        let mut browser = crate::hub::tests::member(&hub);
        let mut app = test_app(Vec::new());
        let paths = paths(&format!("[machine]\nname = \"mac\"\n\n[hub]\nurl = \"{}\"\n", hub.url));
        let state = paths.state.clone();
        app.hub_paths = Some(paths);
        press(&mut app, key(KeyCode::Char('H')));
        press(&mut app, key(KeyCode::Enter));
        until(&mut app, "the link", |view| matches!(view, View::Running(flow) if flow.link.is_some()));
        let View::Running(flow) = &window(&mut app).view else { unreachable!() };
        let (link, code) = (flow.link.clone().unwrap(), flow.code.clone().unwrap());
        assert!(screen(&mut app, 110, 50).contains(&format!("The page shows {code}")));
        let approval = hub.runtime.block_on(browser.approve_connect(&link, true)).unwrap();
        hub.runtime.block_on(browser.show_board(approval, crate::hub::tests::WAIT)).unwrap();
        until(&mut app, "the outcome", |view| matches!(view, View::Said(_)));
        let View::Said(said) = &window(&mut app).view else { unreachable!() };
        assert_eq!(said, &format!("Connected to {} as mac; the browser that approved it shows its board.", hub.url));
        press(&mut app, key(KeyCode::Char('x')));
        let device = browser.device.as_ref().unwrap().device.clone().unwrap();
        assert!(screen(&mut app, 110, 50).contains(&format!("device   {device} paired")), "the outcome gives way to the overview");
        assert_eq!(identity::load(&state).unwrap().unwrap().devices[0].id, device);
    }
}
