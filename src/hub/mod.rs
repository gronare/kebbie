pub mod board;
pub mod http;
pub mod identity;
pub mod pair;
pub mod relay;
#[cfg(test)]
mod tests;

use std::io::Write;
use std::path::Path;

use kelpie_hub_api::{JoinRequest, Joined};
use kelpie_hub_protocol::{Kind, SecretKeys};

use crate::doctor::{Check, Level};
use crate::sh::Result;
use http::{CALL, Client};
use identity::Identity;

pub const EVERY: u64 = 15;

// Polling the inbox marks queued commands delivered, so it stays unpolled until a kind listed here is applied through protocol::authorize.
pub const ACCEPTS: &[Kind] = &[];

fn loopback(host: &str) -> bool {
    let name = host.rsplit_once(':').filter(|(name, port)| !name.is_empty() && port.chars().all(|character| character.is_ascii_digit())).map_or(host, |(name, _)| name);
    matches!(name, "localhost" | "127.0.0.1" | "[::1]")
}

// The token travels in every call, so plain http is for a hub on this machine only.
pub fn hub_url(url: &str) -> Result<String> {
    let url = url.trim().trim_end_matches('/');
    let host = |rest: &str| rest.split(['/', '?', '#']).next().unwrap_or_default().to_string();
    match url.split_once("://") {
        Some(("https", rest)) if !host(rest).is_empty() => Ok(url.to_string()),
        Some(("http", rest)) if loopback(&host(rest)) => Ok(url.to_string()),
        Some(("http", _)) => Err(format!("{url}: a hub on another machine takes https, since the machine's token goes with every call")),
        _ => Err(format!("{url} is not a hub address; it looks like https://hub.example.com")),
    }
}

pub fn default_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: gethostname writes at most buffer.len() bytes into the buffer it is given.
    let written = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0;
    let text = if written { String::from_utf8_lossy(&buffer[..buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len())]).to_string() } else { String::new() };
    let short = text.split('.').next().unwrap_or_default().trim().to_string();
    if short.is_empty() { "kelpie".to_string() } else { short }
}

pub fn join(state: &Path, url: &str, code: &str, name: &str, now: u64) -> Result<Identity> {
    if let Some(joined) = identity::load(state)? {
        return Err(format!("this machine has joined {} as {} already; kelpie hub leave first", joined.url, joined.name));
    }
    let url = hub_url(url)?;
    let keys = SecretKeys::generate();
    let client = Client::new(&url, None, CALL);
    let joined: Joined = client.post("/v1/m/join", &JoinRequest { code: code.trim().to_string(), name: name.to_string(), keys: keys.public() }).map_err(|error| match error.status() {
        Some(401) => "the hub refused that join code: it is used, expired or mistyped; mint a new one with kelpie-hub admin join-code".to_string(),
        _ => error.to_string(),
    })?;
    let identity = Identity::new(&client.moved().unwrap_or(url), joined.machine, name, joined.token, &keys, now)?;
    identity::save(state, &identity)?;
    Ok(identity)
}

fn ago(at: Option<u64>, now: u64) -> String {
    at.map_or("never".to_string(), |at| format!("{} ago", crate::keeper::span(now.saturating_sub(at))))
}

pub fn status_text(state: &Path, now: u64) -> Result<String> {
    let Some(identity) = identity::load(state)? else {
        return Ok("Not joined to a hub. kelpie hub join <url> <join code> shows this machine's board on your phone.".to_string());
    };
    let relay = relay::load(state);
    let mut lines = vec![format!("hub:      {}", identity.url), format!("machine:  {} ({})", identity.name, identity.machine)];
    if identity.devices.is_empty() {
        lines.push("devices:  none; kelpie hub pair pairs your phone".to_string());
    }
    for device in &identity.devices {
        lines.push(format!("device:   {} paired {}", device.id, ago(Some(device.paired_at), now)));
    }
    lines.push(match relay.posted_at {
        Some(at) => format!("board:    rev {} posted {}", relay.rev, ago(Some(at), now)),
        None => "board:    not posted yet".to_string(),
    });
    if let Some(error) = &relay.error {
        lines.push(format!("error:    {error} ({})", ago(relay.error_at, now)));
    }
    let commands = if ACCEPTS.is_empty() { "none; the phone reads the board".to_string() } else { ACCEPTS.iter().map(|kind| kind.name()).collect::<Vec<_>>().join(", ") };
    lines.push(format!("commands: {commands}"));
    Ok(lines.join("\n"))
}

// Nothing here proves the new address is the same hub, so no token goes there until the user has chosen it.
pub fn move_to(state: &Path, url: &str) -> Result<String> {
    let mut identity = identity::load(state)?.ok_or("this machine has not joined a hub; kelpie hub join <url> <join code> first")?;
    let url = hub_url(url)?;
    reach(&url).map_err(|error| format!("{url} does not answer as a kelpie hub: {error}; nothing changed"))?;
    let before = std::mem::replace(&mut identity.url, url);
    identity::save(state, &identity)?;
    crate::notice::Notice::new(crate::notice::Tone::Done, format!("Moved from {before} to {}", identity.url)).next("The keeper posts the board there from now on.").ok()
}

// A device the hub still lists would keep the last board, so leaving takes each one off before the keys go.
pub fn leave(state: &Path) -> Result<String> {
    let Some(identity) = identity::load(state)? else { return Ok("Not joined to a hub; nothing to leave.".to_string()) };
    let client = Client::new(&identity.url, Some(identity.token()), CALL);
    let kept: Vec<String> = identity.devices.iter().filter_map(|device| client.delete(&format!("/v1/m/devices/{}", device.id)).err().filter(|error| error.status() != Some(404)).map(|error| format!("{}: {error}", device.id))).collect();
    identity::forget(state)?;
    let mut notice = crate::notice::Notice::new(crate::notice::Tone::Done, format!("Left {}", identity.url)).next(format!("The hub still lists machine {}; its admin removes it with kelpie-hub admin remove-machine {}.", identity.machine, identity.machine));
    for line in &kept {
        notice = notice.detail("not removed", line);
    }
    notice.ok()
}

pub fn tick() -> Result<()> {
    let state = crate::lead::state_dir()?;
    if identity::load(&state)?.is_none_or(|identity| identity.devices.is_empty()) {
        return Ok(());
    }
    let now = crate::messages::now();
    let board = board::gather(&state, now)?;
    relay::relay(&state, &board, now).map(|_| ())
}

fn interactive() -> bool {
    // SAFETY: isatty only reads the descriptor number.
    unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 }
}

fn ask_digits(digits: &str) -> bool {
    print!("Does the phone show {digits}? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes")
}

pub fn cli(action: Option<&str>, rest: &[String], name: Option<&str>) -> Result<String> {
    let state = crate::lead::state_dir()?;
    let now = crate::messages::now();
    match action {
        None | Some("status") => status_text(&state, now),
        Some("join") => {
            crate::permit::user_only("joining a hub")?;
            let (Some(url), Some(code)) = (rest.first(), rest.get(1)) else { return Err("kelpie hub join needs the hub's address and a join code: kelpie hub join https://hub.example.com <join code>".to_string()) };
            let name = name.map_or_else(default_name, str::to_string);
            let joined = join(&state, url, code, &name, now)?;
            crate::notice::Notice::new(crate::notice::Tone::Done, format!("Joined {} as {}", joined.url, joined.name)).next("kelpie hub pair pairs your phone; the keeper posts the board once a device is paired.").ok()
        }
        Some("pair") => {
            crate::permit::user_only("pairing a device")?;
            pair::interactive_only(interactive())?;
            let device = pair::pair(&state, pair::TIMEOUT, |code| {
                println!("Open the hub on your phone, tap Pair and enter this code: {code}");
                println!("Waiting for the phone (10 minutes)…");
            }, ask_digits, now)?;
            let posted = tick().err().map_or(String::new(), |error| format!(" The first board did not go out yet: {error}."));
            crate::notice::Notice::new(crate::notice::Tone::Done, format!("Paired {device}")).next(format!("The phone shows this machine's board from now on.{posted}")).ok()
        }
        Some("leave") => {
            crate::permit::user_only("leaving the hub")?;
            leave(&state)
        }
        Some("move") => {
            crate::permit::user_only("moving the hub")?;
            let url = rest.first().ok_or("kelpie hub move needs the hub's new address: kelpie hub move https://hub.example.com")?;
            move_to(&state, url)
        }
        Some(other) => Err(format!("kelpie hub knows join, pair, move, status and leave, not {other}")),
    }
}

pub fn config_line(state: Option<&Path>) -> String {
    let identity = state.and_then(|state| identity::load(state).ok().flatten());
    let value = identity.map_or("none".to_string(), |identity| format!("{:?}", identity.url));
    format!("hub.url = {value}  (kelpie hub join)\n  The hub this machine posts its board to, sealed to the phones it paired with; kelpie hub join sets it, kelpie hub leave forgets it, kelpie hub move moves it. Kept in kelpie's state with the machine's keys, not in config.toml.")
}

fn check(level: Level, name: &str, fact: impl Into<String>, fix: Vec<String>) -> Check {
    Check { level, name: name.to_string(), fact: fact.into(), fix }
}

pub fn doctor(state: &Path, now: u64, reach: impl FnOnce(&str) -> std::result::Result<(), String>) -> Check {
    let name = "hub";
    let Ok(Some(identity)) = identity::load(state) else {
        return check(Level::Ok, name, "not joined; kelpie hub join <url> <join code> shows the board on your phone", Vec::new());
    };
    let relay = relay::load(state);
    let posted = relay.posted_at.map_or("no board posted yet".to_string(), |at| format!("board rev {} posted {}", relay.rev, ago(Some(at), now)));
    let fact = format!("{} as {} · {} paired · {posted}", identity.url, identity.name, identity.devices.len());
    if let Err(error) = reach(&identity.url) {
        return check(Level::Warn, name, format!("{fact}; not reachable: {error}"), vec!["check the address and your network; kelpie hub status shows the last error".to_string()]);
    }
    if identity.devices.is_empty() {
        return check(Level::Warn, name, format!("{fact}; no phone paired, so no board goes out"), vec!["kelpie hub pair".to_string()]);
    }
    if let Some(error) = &relay.error {
        return check(Level::Warn, name, format!("{fact}; last post failed {}: {error}", ago(relay.error_at, now)), vec!["kelpie hub status; the keeper retries by itself".to_string()]);
    }
    check(Level::Ok, name, fact, Vec::new())
}

pub fn reach(url: &str) -> std::result::Result<(), String> {
    Client::new(url, None, std::time::Duration::from_secs(5)).reachable().map_err(|error| error.to_string())
}
