use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use kebbie_hub_api::{CONNECT_DENIED, CONNECT_EXPIRED, CONNECT_PENDING, CONNECT_SLOW_DOWN, CONNECT_UNKNOWN, ConnectPoll, ConnectStart, ConnectStarted, Connected, HubInfo};
use kebbie_hub_protocol::{ConnectSecret, SecretKeys};

use super::http::{CALL, Client, HubError};
use super::identity::{self, Identity};
use super::pair::{self, Stop};
use super::qr;
use super::{AskName, Named};
use crate::sh::Result;

const SLOW_DOWN_STEP: u64 = 5;
pub const STOPPED: &str = "stopped; this machine is not connected";

pub fn hub_name(url: &str) -> String {
    if url == crate::config::DEFAULT_HUB_URL { format!("the kebbie hub ({url})") } else { url.to_string() }
}

// Nothing answers GET /v1/hub with a HubInfo but a kebbie hub, and hub_url already holds the address to https.
pub fn probe(address: &str) -> Result<String> {
    let url = super::hub_url(address)?;
    Client::new(&url, None, Duration::from_secs(10)).get::<HubInfo>("/v1/hub").map_err(|error| format!("{url} does not answer as a kebbie hub: {error}; nothing changed"))?;
    Ok(url)
}

pub struct Chosen {
    pub url: String,
    pub note: Option<String>,
}

pub fn configured_hub(config: &Path) -> Result<Option<String>> {
    Ok(crate::config::load_machine(config)?.hub_url)
}

// A config.toml linked in from dotfiles is shared with other machines, so kebbie writes no key through it and says where to set it.
pub fn own_hub(config: &Path, address: &str) -> Result<Chosen> {
    let url = probe(address)?;
    let set = format!("[hub] url = {url:?}");
    let note = if super::linked(config) {
        format!("{} is a link, so kebbie writes no key through it; set {set} there or in a config.toml of this machine's own to connect there again", crate::config::shown(config))
    } else {
        crate::routine_file::set_hub_url_in(config, &url).map_or_else(|error| format!("{set} not set in {}: {error}", config.display()), |()| format!("{set} set in {}", config.display()))
    };
    Ok(Chosen { url, note: Some(note) })
}

pub type AskHub = dyn Fn() -> Result<Option<String>>;

// None from the question means the kebbie hub; an address means the user's own, which is tested before anything is saved.
pub fn choose_hub(config: &Path, ask: Option<&dyn Fn() -> Result<Option<String>>>) -> Result<Chosen> {
    if let Some(url) = configured_hub(config)? {
        return Ok(Chosen { url, note: None });
    }
    match ask.map(|ask| ask()).transpose()?.flatten() {
        Some(address) => own_hub(config, &address),
        None => Ok(Chosen { url: crate::config::DEFAULT_HUB_URL.to_string(), note: None }),
    }
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
    deadline: Instant,
}

pub fn start(url: &str, name: &str) -> Result<Pending> {
    let url = super::hub_url(url)?;
    let keys = SecretKeys::generate();
    let client = Client::new(&url, None, CALL);
    let started: ConnectStarted = client.post("/v1/m/connect", &ConnectStart { name: name.to_string(), keys: keys.public() }).map_err(|error| format!("{} did not start the connect: {error}", hub_name(&url)))?;
    let secret = ConnectSecret::new(&keys.public());
    let deadline = Instant::now() + Duration::from_secs(started.expires_in + started.interval);
    Ok(Pending { url, name: name.to_string(), interval: started.interval, started, keys, secret, client, deadline })
}

impl Pending {
    // The fragment never reaches the hub, so the secret and the fingerprint in it bind the browser to these keys.
    pub fn link(&self) -> String {
        self.secret.link(&self.started.verification_uri_complete)
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

pub fn finish(state: &Path, pending: &Pending, connected: &Connected, stop: Stop, post: impl FnOnce() -> Result<()>, now: u64) -> Result<String> {
    let identity = pending.save(state, connected, now)?;
    let connected_line = format!("Connected to {} as {}", hub_name(&identity.url), identity.name);
    let Some(pairing) = &connected.pairing else {
        return Ok(format!("{connected_line}; kebbie hub add-device or H on the board adds your phone."));
    };
    Ok(match pair::admit_browser(state, pairing, pending.secret(), pair::TIMEOUT, stop, now) {
        Ok(_) => {
            let posted = post().err().map_or(String::new(), |error| format!(" The first board did not go out yet: {error}."));
            format!("{connected_line}; the browser that approved it shows its board.{posted}")
        }
        Err(error) => format!("{connected_line}; the browser was not added ({error}), and kebbie hub add-device adds it."),
    })
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

fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|error| format!("reading the answer: {error}"))?;
    Ok(answer.trim().to_string())
}

fn ask_hub() -> Result<Option<String>> {
    let answer = read_line(&format!("Connect to the kebbie hub ({}) or your own hub? [K/o] ", crate::config::DEFAULT_HUB_URL))?;
    if !answer.to_lowercase().starts_with('o') {
        return Ok(None);
    }
    let address = read_line("Your hub's address (https://…): ")?;
    if address.is_empty() { Err("no address given; nothing changed".to_string()) } else { Ok(Some(address)) }
}

pub fn opener() -> Option<&'static str> {
    crate::board::images::image_viewer(std::env::consts::OS, &|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

pub fn cli(state: &Path, config: &Path, given: Option<&str>, interactive: bool, now: u64) -> Result<String> {
    let ask_name: &AskName = &super::ask_name;
    let ask_hub: &AskHub = &ask_hub;
    let prepared = prepare(state, config, given, interactive.then_some(ask_name))?;
    let chosen = choose_hub(config, interactive.then_some(ask_hub))?;
    if let Some(note) = &chosen.note {
        println!("{note}");
    }
    let mut pending = start(&chosen.url, prepared.named.name())?;
    let link = pending.link();
    println!("Open this link to connect {} to {}, and check that the page shows {}:\n{link}", pending.name, hub_name(&pending.url), pending.user_code());
    if let Some(code) = qr::for_stdout(&link) {
        println!("{code}");
    }
    if let Some(program) = opener() {
        match crate::issues::open_url(program, &link) {
            Ok(()) => println!("Opened in your browser."),
            Err(error) => println!("Could not open a browser ({error}); open the link yourself."),
        }
    }
    println!("Waiting for the browser (10 minutes)…");
    let connected = pending.wait(&mut sleep_unless(None))?;
    let line = finish(state, &pending, &connected, None, super::tick, now)?;
    Ok(match name_line(config, prepared, &pending.name) {
        Some(named) => format!("{line}\n{named}"),
        None => line,
    })
}

pub fn add_device_cli(state: &Path, interactive: bool, now: u64) -> Result<String> {
    if !interactive {
        return Err("kebbie hub add-device asks you to compare six digits with the phone, so it runs in your own terminal".to_string());
    }
    let device = pair::add_device(state, pair::TIMEOUT, |added| {
        println!("On your phone or in a browser, open this link (or scan the code) and add it:\n{}", added.url);
        if let Some(code) = qr::for_stdout(&added.url) {
            println!("{code}");
        }
        println!("Waiting for it (10 minutes)…");
    }, super::ask_digits, None, now)?;
    let posted = super::tick().err().map_or(String::new(), |error| format!(" The first board did not go out yet: {error}."));
    Ok(format!("Added {}; it shows this machine's board from now on.{posted}", super::devices::added_name(state, &device, now)))
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
