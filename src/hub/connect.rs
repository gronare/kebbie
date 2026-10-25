use std::path::Path;
use std::time::{Duration, Instant};

use kebbie_hub_api::{CONNECT_DENIED, CONNECT_EXPIRED, CONNECT_PENDING, CONNECT_SLOW_DOWN, CONNECT_UNKNOWN, ConnectPoll, ConnectStart, ConnectStarted, Connected, HubInfo};
use kebbie_hub_protocol::{ConnectSecret, SecretKeys};

use super::card::{self, Copied, Foot, Row, Tone, piece};
use super::console::{self, Console, Key, say};
use super::http::{CALL, Client, HubError};
use super::identity::{self, Identity};
use super::pair::{self, Stop};
use super::qr;
use super::{AskName, Named};
use crate::board::widgets::text_width;
use crate::config::DEFAULT_HUB_URL;
use crate::sh::Result;

const SLOW_DOWN_STEP: u64 = 5;
pub const STOPPED: &str = "stopped; this machine is not connected";

pub fn hub_name(url: &str) -> String {
    if url == crate::config::DEFAULT_HUB_URL { format!("the kebbie hub ({url})") } else { url.to_string() }
}

// Nothing answers GET /v1/hub with a HubInfo but a kebbie hub, and hub_url already holds the address to https.
pub fn probe(address: &str) -> Result<String> {
    let address = address.trim();
    let address = if address.contains("://") { address.to_string() } else { format!("https://{address}") };
    let url = super::hub_url(&address)?;
    Client::new(&url, None, Duration::from_secs(10)).get::<HubInfo>("/v1/hub").map_err(|error| format!("{url} does not answer as a kebbie hub: {error}; nothing changed"))?;
    Ok(url)
}

pub fn transport(url: &str) -> &'static str {
    if url.starts_with("https://") { "TLS ok" } else { "plain http, on this machine only" }
}

pub struct Chosen {
    pub url: String,
    pub note: Option<String>,
}

pub fn configured_hub(config: &Path) -> Result<Option<String>> {
    Ok(crate::config::load_machine(config)?.hub_url)
}

// A config.toml linked in from dotfiles is shared with other machines, so kebbie writes no key through it and says where to set it.
pub fn save_own(config: &Path, url: String) -> Chosen {
    let set = format!("[hub] url = {url:?}");
    let note = if super::linked(config) {
        format!("{} is a link, so kebbie writes no key through it; set {set} there or in a config.toml of this machine's own to connect there again", crate::config::shown(config))
    } else {
        crate::routine_file::set_hub_url_in(config, &url).map_or_else(|error| format!("{set} not set in {}: {error}", config.display()), |()| format!("{set} set in {}", config.display()))
    };
    Chosen { url, note: Some(note) }
}

pub enum Polled {
    Pending,
    SlowDown,
    Connected(Connected),
}

pub struct Pending {
    pub url: String,
    pub name: String,
    pub started: ConnectStarted,
    pub interval: u64,
    keys: SecretKeys,
    secret: ConnectSecret,
    client: Client,
    expires: Instant,
    deadline: Instant,
}

pub fn start(url: &str, name: &str) -> Result<Pending> {
    let url = super::hub_url(url)?;
    let keys = SecretKeys::generate();
    let client = Client::new(&url, None, CALL);
    let started: ConnectStarted = client.post("/v1/m/connect", &ConnectStart { name: name.to_string(), keys: keys.public() }).map_err(|error| format!("{} did not start the connect: {error}", hub_name(&url)))?;
    let secret = ConnectSecret::new(&keys.public());
    let expires = Instant::now() + Duration::from_secs(started.expires_in);
    let deadline = expires + Duration::from_secs(started.interval);
    Ok(Pending { url, name: name.to_string(), interval: started.interval, started, keys, secret, client, expires, deadline })
}

impl Pending {
    // The fragment never reaches the hub, so the secret and the fingerprint in it bind the browser to these keys.
    pub fn link(&self) -> String {
        self.secret.link(&self.started.verification_uri_complete)
    }

    pub fn expires(&self) -> Instant {
        self.expires
    }

    pub fn user_code(&self) -> &str {
        &self.started.user_code
    }

    pub fn poll(&mut self) -> Result<Polled> {
        match self.client.post::<_, Connected>("/v1/m/connect/token", &ConnectPoll { device_code: self.started.device_code.clone() }) {
            Ok(connected) => Ok(Polled::Connected(connected)),
            Err(HubError::Status { status: 400, code }) => match code.as_str() {
                CONNECT_PENDING => Ok(Polled::Pending),
                CONNECT_SLOW_DOWN => {
                    self.interval += SLOW_DOWN_STEP;
                    Ok(Polled::SlowDown)
                }
                CONNECT_DENIED => Err("Cancelled in the browser; this machine is not connected".to_string()),
                CONNECT_EXPIRED => Err("The code expired before anyone approved it; connect again for a new one".to_string()),
                CONNECT_UNKNOWN => Err("The hub no longer knows this connect; connect again for a new code".to_string()),
                _ => Err(HubError::Status { status: 400, code }.to_string()),
            },
            Err(HubError::Transport(_)) => Ok(Polled::Pending),
            Err(error) => Err(error.to_string()),
        }
    }

    // sleep returns false when the user stopped waiting.
    pub fn wait(&mut self, sleep: &mut dyn FnMut(Duration) -> bool) -> Result<Connected> {
        loop {
            if let Polled::Connected(connected) = self.poll()? {
                return Ok(connected);
            }
            if Instant::now() >= self.deadline {
                return Err("The code expired before anyone approved it; connect again for a new one".to_string());
            }
            if !sleep(Duration::from_secs(self.interval)) {
                return Err(STOPPED.to_string());
            }
        }
    }

    pub fn save(&self, state: &Path, connected: &Connected, now: u64) -> Result<Identity> {
        if let Some(joined) = identity::load(state)? {
            return Err(super::already(&joined));
        }
        let url = self.client.moved().unwrap_or_else(|| self.url.clone());
        let identity = Identity::new(&url, connected.machine.clone(), &self.name, connected.token.clone(), &self.keys, now)?;
        identity::save(state, &identity)?;
        Ok(identity)
    }

    pub fn secret(&self) -> &ConnectSecret {
        &self.secret
    }
}

pub fn sleep_unless(stop: Stop<'_>) -> impl FnMut(Duration) -> bool + '_ {
    move |wait: Duration| {
        let until = Instant::now() + wait;
        loop {
            if stop.is_some_and(|stop| stop()) {
                return false;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            std::thread::sleep(left.min(Duration::from_millis(200)));
        }
    }
}

pub struct Finished {
    pub identity: Identity,
    pub remark: Option<String>,
}

pub fn finish(state: &Path, pending: &Pending, connected: &Connected, stop: Stop, post: impl FnOnce() -> Result<()>, now: u64) -> Result<Finished> {
    let identity = pending.save(state, connected, now)?;
    let Some(pairing) = &connected.pairing else {
        return Ok(Finished { identity, remark: Some(format!("No board goes out: {}. Connect it from the link: {}, and open the link it shows.", super::UNREAD, super::RECONNECT)) });
    };
    let remark = match pair::pin(state, pairing, pending.secret(), pair::TIMEOUT, stop, now) {
        Ok(_) => post().err().map(|error| format!("The first board did not go out yet: {error}.")),
        Err(error) => Some(format!("No board goes out: {error}.")),
    };
    Ok(Finished { identity: identity::load(state)?.unwrap_or(identity), remark })
}

pub struct Prepared {
    pub named: Named,
    pub configured: Option<String>,
}

pub fn prepare(state: &Path, config: &Path, given: Option<&str>, ask: Option<&AskName>) -> Result<Prepared> {
    if let Some(joined) = identity::load(state)? {
        return Err(super::already(&joined));
    }
    let configured = crate::config::load_machine(config)?.machine_name;
    let named = super::checked_name(config, configured.as_deref(), given, ask)?;
    Ok(Prepared { named, configured })
}

pub fn name_line(config: &Path, prepared: Prepared, name: &str) -> Option<String> {
    super::name_detail(config, prepared.named, prepared.configured, name).map(|(label, text)| format!("{label}: {text}"))
}

pub fn opener() -> Option<&'static str> {
    crate::board::images::image_viewer(std::env::consts::OS, &|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

fn line(text: impl Into<String>, tone: Tone) -> Row {
    vec![piece(text, tone)]
}

fn prompt(buffer: &str) -> Row {
    line(format!("  › {buffer}"), Tone::Text)
}

const HUBS: [(&str, &str); 2] = [("k", "kebbie hub"), ("o", "your own hub")];

fn hub_abouts() -> [String; 2] {
    [format!("{} · free to connect and read", super::host(DEFAULT_HUB_URL)), "a server you run kebbie-hub on".to_string()]
}

pub fn hub_rows(selected: usize) -> Vec<Row> {
    let mut rows: Vec<Row> = HUBS.iter().zip(hub_abouts()).enumerate().map(|(index, ((key, what), about))| {
        let chosen = index == selected;
        vec![piece(if chosen { "▸ " } else { "  " }, Tone::Key), piece(format!("{key}  "), Tone::Key), piece(format!("{what:<15}"), if chosen { Tone::Title } else { Tone::Text }), piece(about, Tone::Muted)]
    }).collect();
    rows.push(Vec::new());
    rows.push(vec![piece("  ↑↓", Tone::Key), piece(" choose · ", Tone::Muted), piece("⏎", Tone::Key), piece(" connect · ", Tone::Muted), piece("k", Tone::Key), piece(" kebbie hub · ", Tone::Muted), piece("o", Tone::Key), piece(" your own · ", Tone::Muted), piece("esc", Tone::Key), piece(" cancel", Tone::Muted)]);
    rows
}

pub fn choose_on(console: &mut dyn Console, config: &Path) -> Result<Option<Chosen>> {
    let kebbie = || Chosen { url: DEFAULT_HUB_URL.to_string(), note: None };
    if console.arrows() {
        say(console, &[Vec::new(), line("  Hub", Tone::Title), Vec::new()])?;
        return match console.pick(&hub_rows, HUBS.len(), &['k', 'o'])? {
            None => Ok(None),
            Some(0) => Ok(Some(kebbie())),
            Some(_) => own_on(console, config),
        };
    }
    let option = |key: &str, what: &str, about: String| vec![piece(format!("  {key}  "), Tone::Key), piece(format!("{what:<15}"), Tone::Text), piece(about, Tone::Muted)];
    let [first, second] = hub_abouts();
    say(console, &[
        Vec::new(),
        line("  Hub", Tone::Title),
        Vec::new(),
        option(HUBS[0].0, HUBS[0].1, first),
        option(HUBS[1].0, HUBS[1].1, second),
        Vec::new(),
    ])?;
    loop {
        let Some(answer) = console.edit(&|buffer| (prompt(buffer), Vec::new()), "k")? else { return Ok(None) };
        match answer.trim().to_lowercase().as_str() {
            "k" | "" => return Ok(Some(kebbie())),
            "o" => return own_on(console, config),
            _ => say(console, &[line("  k is the kebbie hub, o your own", Tone::Muted)])?,
        }
    }
}

fn own_on(console: &mut dyn Console, config: &Path) -> Result<Option<Chosen>> {
    say(console, &[Vec::new(), line("  Address of the server that runs kebbie-hub", Tone::Text)])?;
    loop {
        let Some(address) = console.edit(&|buffer| (prompt(buffer), Vec::new()), "")?.filter(|address| !address.trim().is_empty()) else { return Ok(None) };
        match probe(&address) {
            Ok(url) => {
                say(console, &[vec![piece("  ✓ ", Tone::Good), piece("answers as a kebbie hub   ", Tone::Text), piece(transport(&url), Tone::Muted)]])?;
                return Ok(Some(save_own(config, url)));
            }
            Err(error) => say(console, &[vec![piece("  ✗ ", Tone::Bad), piece(error, Tone::Text)]])?,
        }
    }
}

pub fn name_on(console: &mut dyn Console, default: &str) -> Result<Option<String>> {
    say(console, &[Vec::new(), line("  Name this machine", Tone::Title), line("  Your phone and your other machines show it.", Tone::Muted), Vec::new()])?;
    let keeps = |buffer: &str| if buffer == default { line(format!("        ⏎ keeps {default:?}"), Tone::Muted) } else { Vec::new() };
    console.edit(&|buffer| (prompt(buffer), keeps(buffer)), default)
}

fn indented(rows: Vec<Row>) -> Vec<Row> {
    rows.into_iter().map(|row| if row.is_empty() { row } else { std::iter::once(piece("  ", Tone::Text)).chain(row).collect() }).collect()
}

#[derive(Default)]
struct Drawn {
    lines: Vec<String>,
}

impl Drawn {
    fn draw(&mut self, console: &mut dyn Console, lines: &[String]) -> Result<()> {
        if self.lines == lines {
            return Ok(());
        }
        let up = if self.lines.is_empty() { String::new() } else { format!("\x1b[{}F\x1b[J", self.lines.len()) };
        self.lines = lines.to_vec();
        console.write(&format!("{up}{}\n", lines.join("\n")))
    }
}

pub struct Run<'a> {
    pub state: &'a Path,
    pub config: &'a Path,
    pub given: Option<&'a str>,
    pub interactive: bool,
    pub opener: Option<&'a str>,
    pub post: fn() -> Result<()>,
    pub now: u64,
}

pub fn connect_on(console: &mut dyn Console, run: &Run) -> Result<String> {
    if let Some(joined) = identity::load(run.state)? {
        return Err(super::already(&joined));
    }
    let chosen = match configured_hub(run.config)? {
        Some(url) => Chosen { url, note: None },
        None if run.interactive => choose_on(console, run.config)?.ok_or(STOPPED)?,
        None => Chosen { url: DEFAULT_HUB_URL.to_string(), note: None },
    };
    let asked = match (run.given, crate::config::load_machine(run.config)?.machine_name) {
        (None, None) if run.interactive => Some(name_on(console, &crate::config::short_hostname())?.ok_or(STOPPED)?),
        _ => None,
    };
    let prepared = prepare(run.state, run.config, run.given.or(asked.as_deref()), None)?;
    let mut pending = start(&chosen.url, prepared.named.name())?;
    let link = pending.link();
    let glyphs = console.glyphs();
    let code = qr::lines(&link, glyphs).unwrap_or_default();
    let (name, url, user_code, expires) = (pending.name.clone(), pending.url.clone(), pending.user_code().to_string(), pending.expires());
    let (live, size) = (console.live(), console.size());
    let card = |colour, copied: Option<Copied>, browser| {
        let left = expires.saturating_duration_since(Instant::now());
        let text = card::waiting_text(&card::Waiting { name: &name, url: &url, code: &user_code, browser, left });
        let fits = |boxed: &[String]| size.is_none_or(|(columns, rows)| text_width(&boxed[0]) <= columns && boxed.len() + 4 <= rows);
        let mut stacked = text.clone();
        stacked.push(Vec::new());
        stacked.extend(code.iter().map(|line| vec![piece(line.clone(), card::Tone::Dots)]));
        let drawn = [card::beside(&code, card::Tone::Dots, text.clone()), stacked].into_iter().map(|rows| card::boxed(&rows, colour)).find(|boxed| fits(boxed));
        let foot = match (live, copied) {
            (_, Some(copied)) => Foot::Copied(copied),
            (true, None) => Foot::Key,
            (false, None) => Foot::Whole,
        };
        let mut lines = vec![String::new()];
        let fits = drawn.is_some();
        lines.extend(drawn.unwrap_or_else(|| card::boxed(&text, colour)));
        let mut foot = card::footer(&link, foot);
        if !fits {
            foot.push(vec![piece(card::NO_ROOM, Tone::Muted)]);
        }
        lines.extend(indented(foot).iter().map(|row| card::paint(row, colour)));
        lines
    };
    let colour = console.colour();
    let opener = run.opener.filter(|_| live);
    let mut browser = if opener.is_some() { card::Browser::Offered } else { card::Browser::Absent };
    let mut drawn = Drawn::default();
    drawn.draw(console, &card(colour, None, browser))?;
    console.keys(true)?;
    let mut copied = None;
    let waited = pending.wait(&mut |interval| {
        for _ in 0..interval.as_secs().max(1) {
            match console.key(Duration::from_secs(1)) {
                Ok(Some(Key::Stop)) | Err(_) => return false,
                Ok(Some(Key::Copy)) => copied = console.copy(&link).ok().or(copied),
                Ok(Some(Key::Open)) => {
                    if let Some(program) = opener && crate::issues::open_url(program, &link).is_ok() {
                        browser = card::Browser::Opened;
                    }
                }
                Ok(None) => {}
            }
            if live && drawn.draw(console, &card(colour, copied, browser)).is_err() {
                return false;
            }
        }
        true
    });
    console.keys(false)?;
    let finished = finish(run.state, &pending, &waited?, None, run.post, run.now)?;
    let done = card::boxed(&card::connected_text(&finished.identity.name, &finished.identity.url, "its", finished.identity.account.is_some()), colour);
    if live { drawn.draw(console, &[String::new()])? } else { console.write("\n")? };
    console.write(&format!("{}\n", done.join("\n")))?;
    let lines = [chosen.note, finished.remark, name_line(run.config, prepared, &pending.name)];
    Ok(lines.into_iter().flatten().collect::<Vec<_>>().join("\n"))
}

pub fn cli(state: &Path, config: &Path, given: Option<&str>, interactive: bool, now: u64) -> Result<String> {
    let run = Run { state, config, given, interactive, opener: opener(), post: super::tick, now };
    if interactive {
        return connect_on(&mut console::Tty::new(), &run);
    }
    let stdin = std::io::stdin();
    connect_on(&mut console::Lines { input: stdin.lock(), output: std::io::stdout() }, &run)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kebbie_hub_is_named_and_any_other_hub_goes_by_its_address() {
        assert_eq!(hub_name(crate::config::DEFAULT_HUB_URL), format!("the kebbie hub ({})", crate::config::DEFAULT_HUB_URL));
        assert_eq!(hub_name("https://hub.example.com"), "https://hub.example.com");
    }
}
