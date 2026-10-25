pub mod board;
pub mod card;
pub mod connect;
pub mod console;
pub mod devices;
pub mod http;
pub mod identity;
pub mod inbox;
pub mod pair;
pub mod qr;
pub mod relay;
pub mod report;
#[cfg(test)]
pub(crate) mod tests;

use std::io::Write;
use std::path::Path;

use kebbie_hub_api::{JoinRequest, Joined, MachineName};
use kebbie_hub_protocol::{Kind, SecretKeys};

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

fn already(joined: &Identity) -> String {
    format!("this machine has joined {} as {} already; kebbie hub leave first", joined.url, joined.name)
}

pub type AskName = dyn Fn(&str) -> Result<String>;

#[derive(Debug, PartialEq)]
pub enum Named {
    Configured(String),
    Chosen(String),
    Host(String),
}

impl Named {
    pub fn name(&self) -> &str {
        match self {
            Named::Configured(name) | Named::Chosen(name) | Named::Host(name) => name,
        }
    }
}

pub fn name_for_join(configured: Option<&str>, given: Option<&str>, ask: Option<&AskName>) -> Result<Named> {
    let host = crate::config::short_hostname();
    let given = given.map(str::trim).filter(|name| !name.is_empty());
    Ok(match (given, configured, ask) {
        (Some(name), Some(configured), _) if name == configured => Named::Configured(name.to_string()),
        (Some(name), _, _) => Named::Chosen(name.to_string()),
        (None, Some(configured), _) => Named::Configured(configured.to_string()),
        (None, None, Some(ask)) => Named::Chosen(Some(ask(&host)?.trim().to_string()).filter(|answer| !answer.is_empty()).unwrap_or(host)),
        (None, None, None) => Named::Host(host),
    })
}

fn ask_name(host: &str) -> Result<String> {
    print!("Name this machine (shown on your phone and to your other machines) [{host}]: ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|error| format!("reading the name: {error}"))?;
    Ok(answer)
}

// A config.toml linked in from dotfiles is shared with other machines, so the one key that must differ per machine is never written through it.
fn linked(config: &Path) -> bool {
    std::fs::symlink_metadata(config).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

pub fn checked_name(config: &Path, configured: Option<&str>, given: Option<&str>, ask: Option<&AskName>) -> Result<Named> {
    let named = name_for_join(configured, given, ask)?;
    if let Named::Chosen(name) = &named {
        crate::config::valid_machine_name(name)?;
        if !linked(config) {
            crate::routine_file::check_machine_name_in(config, name)?;
        }
    }
    Ok(named)
}

pub fn name_detail(config: &Path, named: Named, configured: Option<String>, name: &str) -> Option<(&'static str, String)> {
    let set = format!("[machine] name = {name:?}");
    match named {
        Named::Configured(_) => None,
        Named::Chosen(_) if linked(config) => {
            let after = configured.map_or(String::new(), |configured| format!("; until then the hub shows it as {configured:?} after the next post, the name that file sets"));
            Some(("not set", format!("{} is a link, so kebbie writes no per-machine key through it; set {set} there or in a config.toml of this machine's own{after}", crate::config::shown(config))))
        }
        Named::Chosen(chosen) => Some(match crate::routine_file::set_machine_name_in(config, &chosen) {
            Ok(()) => ("set", format!("{set} in {}", config.display())),
            Err(error) => ("not set", format!("{set} in {}: {error}; set it by hand so the vault and the phone use one name", config.display())),
        }),
        Named::Host(_) => Some(("named", format!("after the host; set [machine] name in {} and the hub shows the new name after the next post", config.display()))),
    }
}

pub fn join_named(state: &Path, config: &Path, url: &str, code: &str, given: Option<&str>, ask: Option<&AskName>, now: u64) -> Result<String> {
    if let Some(joined) = identity::load(state)? {
        return Err(already(&joined));
    }
    let configured = crate::config::load_machine(config)?.machine_name;
    let named = checked_name(config, configured.as_deref(), given, ask)?;
    let joined = join(state, url, code, named.name(), now)?;
    let notice = crate::notice::Notice::new(crate::notice::Tone::Done, format!("Joined {} as {}", joined.url, joined.name)).next("kebbie hub add-device adds your phone; the keeper posts the board once a device is paired.");
    match name_detail(config, named, configured, &joined.name) {
        Some((label, text)) => notice.detail(label, text),
        None => notice,
    }.ok()
}

pub fn rename_machine(config: &Path, name: &str) -> Result<String> {
    let name = crate::config::valid_machine_name(name)?;
    if linked(config) {
        return Err(format!("{} is a link, so kebbie writes no per-machine key through it; set [machine] name = {name:?} there or in a config.toml of this machine's own", crate::config::shown(config)));
    }
    crate::routine_file::check_machine_name_in(config, &name)?;
    crate::routine_file::set_machine_name_in(config, &name)?;
    Ok(name)
}

// Only a name set in config.toml renames the machine on the hub; the host's name, which may drift, never does.
pub fn configured_name(config: &Path) -> Option<String> {
    crate::config::load_machine(config).ok()?.machine_name
}

pub fn sync_name(state: &Path, configured: Option<&str>) -> Result<()> {
    let Some(identity) = identity::load(state)? else { return Ok(()) };
    let unchanged = (identity.name.clone(), None, None);
    let (name, error, stopped) = match configured.filter(|name| *name != identity.name) {
        None => unchanged,
        Some(name) if identity.rename_stopped.as_deref() == Some(name) => return Ok(()),
        Some(name) => match Client::new(&identity.url, Some(identity.token()), CALL).put::<_, MachineName>("/v1/m/name", &MachineName { name: name.to_string() }) {
            Ok(renamed) if renamed.name == name => (renamed.name, None, None),
            Ok(renamed) => (renamed.name.clone(), Some(format!("the hub took it as {:?}; kebbie tries again once [machine] name changes", renamed.name)), Some(name.to_string())),
            Err(error) if error.status() == Some(404) => (identity.name.clone(), Some("this hub cannot rename machines yet; update kebbie-hub".to_string()), None),
            Err(error) => (identity.name.clone(), Some(error.to_string()), None),
        },
    };
    if (&name, &error, &stopped) == (&identity.name, &identity.rename_error, &identity.rename_stopped) {
        return Ok(());
    }
    // A pairing may have saved a device since the load above, so only the name fields are written over a fresh load.
    let Some(mut current) = identity::load(state)? else { return Ok(()) };
    current.name = name;
    current.rename_error = error;
    current.rename_stopped = stopped;
    identity::save(state, &current)
}

fn rename_line(identity: &Identity, configured: Option<&str>) -> Option<String> {
    let configured = configured.filter(|name| *name != identity.name)?;
    Some(match &identity.rename_error {
        Some(error) if identity.rename_stopped.is_some() => format!("the hub shows this machine as {:?}, not {configured:?}: {error}", identity.name),
        Some(error) => format!("the hub still shows this machine as {:?}; renaming it to {configured:?} failed: {error}; kebbie tries again on the next post", identity.name),
        None => format!("the hub shows this machine as {:?} until the keeper's next post or kebbie hub status renames it to {configured:?}", identity.name),
    })
}

pub fn join(state: &Path, url: &str, code: &str, name: &str, now: u64) -> Result<Identity> {
    if let Some(joined) = identity::load(state)? {
        return Err(already(&joined));
    }
    let url = hub_url(url)?;
    let keys = SecretKeys::generate();
    let client = Client::new(&url, None, CALL);
    let joined: Joined = client.post("/v1/m/join", &JoinRequest { code: code.trim().to_string(), name: name.to_string(), keys: keys.public() }).map_err(|error| match error.status() {
        Some(401) => "the hub refused that join code: it is used, expired or mistyped; mint a new one with kebbie-hub admin join-code".to_string(),
        _ => error.to_string(),
    })?;
    let identity = Identity::new(&client.moved().unwrap_or(url), joined.machine, name, joined.token, &keys, now)?;
    identity::save(state, &identity)?;
    Ok(identity)
}

fn ago(at: Option<u64>, now: u64) -> String {
    at.map_or("never".to_string(), |at| format!("{} ago", crate::keeper::span(now.saturating_sub(at))))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sent {
    Yes,
    Failed,
    NotYet,
}

// The newest of the last post and the last failure says how the board stands, so a failure fixed by a later post shows as sent.
pub fn board_line(relay: &relay::Relay, paired: bool, now: u64) -> (Sent, String) {
    let failed_last = relay.error_at.is_some_and(|failed| relay.posted_at.is_none_or(|posted| failed >= posted));
    if let Some(plan) = &relay.plan_inactive {
        return (Sent::Failed, plan.clone());
    }
    match (&relay.error, relay.posted_at) {
        (Some(error), _) if failed_last => {
            let when = relay.error_at.map_or(String::new(), |at| format!(" ({})", devices::ago(at, now)));
            let reason = match error.strip_prefix("could not reach the hub: ") {
                Some(_) => "the hub can't be reached".to_string(),
                None => error.clone(),
            };
            (Sent::Failed, format!("Board not sent: {reason}{when}"))
        }
        (_, Some(_)) => (Sent::Yes, "Board is being sent".to_string()),
        (_, None) if paired => (Sent::NotYet, "Board not sent yet; the keeper sends it within a minute".to_string()),
        (_, None) => (Sent::NotYet, "Nothing sent yet; the board goes out once a device is added".to_string()),
    }
}

pub fn host(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, rest)| rest)
}

pub fn status_text(state: &Path, configured: Option<&str>, now: u64) -> Result<String> {
    let Some(identity) = identity::load(state)? else {
        return Ok("Not connected to a hub. kebbie hub connect, or H on the board, shows this machine's board on your phone.".to_string());
    };
    let relay = relay::load(state);
    let listed = devices::load(state);
    let mut lines = vec![format!("hub:      {}", identity.url), format!("machine:  {} ({})", identity.name, identity.machine)];
    lines.extend(rename_line(&identity, configured).map(|line| format!("name:     {line}")));
    if identity.devices.is_empty() {
        lines.push("devices:  none; kebbie hub add-device adds your phone".to_string());
    }
    for device in &identity.devices {
        let line = match listed.devices.iter().find(|known| known.device == device.id) {
            Some(known) => format!("device:   {} · {} ({})", devices::named(&device.id, &listed, &identity.devices).unwrap_or_else(|| devices::name_of(known)), devices::facts(known, now), device.id),
            None => format!("device:   {} paired {}", device.id, ago(Some(device.paired_at), now)),
        };
        lines.push(line);
    }
    if let Some(error) = &listed.error {
        let known = listed.fetched_at.map_or("names and last reads are unknown".to_string(), |at| format!("names and last reads as of {}", devices::ago(at, now)));
        lines.push(format!("          {known}; the hub did not list them: {error}"));
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
    let mut identity = identity::load(state)?.ok_or("this machine is not connected to a hub; kebbie hub connect first")?;
    let url = hub_url(url)?;
    reach(&url).map_err(|error| format!("{url} does not answer as a kebbie hub: {error}; nothing changed"))?;
    let before = std::mem::replace(&mut identity.url, url);
    identity::save(state, &identity)?;
    crate::notice::Notice::new(crate::notice::Tone::Done, format!("Moved from {before} to {}", identity.url)).next("The keeper posts the board there from now on.").ok()
}

// A device the hub still lists would keep the last board, so leaving takes each one off before the keys go.
pub fn leave(state: &Path) -> Result<String> {
    let Some(identity) = identity::load(state)? else { return Ok("Not connected to a hub; nothing to leave.".to_string()) };
    let client = Client::new(&identity.url, Some(identity.token()), CALL);
    let kept: Vec<String> = identity.devices.iter().filter_map(|device| client.delete(&format!("/v1/m/devices/{}", device.id)).err().filter(|error| error.status() != Some(404)).map(|error| format!("{}: {error}", device.id))).collect();
    identity::forget(state)?;
    let mut notice = crate::notice::Notice::new(crate::notice::Tone::Done, format!("This machine left {}", identity.url)).next(format!("The hub still lists machine {}; its admin removes it with kebbie-hub admin remove-machine {}.", identity.machine, identity.machine));
    for line in &kept {
        notice = notice.detail("not removed", line);
    }
    notice.ok()
}

pub fn tick() -> Result<()> {
    let state = crate::lead::state_dir()?;
    sync_name(&state, configured_name(&crate::config::path()).as_deref())?;
    if identity::load(&state)?.is_none_or(|identity| identity.devices.is_empty()) {
        return Ok(());
    }
    let now = crate::messages::now();
    let board = board::gather(&state, now)?;
    relay::relay(&state, &board, now).map(|_| ())
}

pub fn interactive() -> bool {
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
        None | Some("status") => {
            let configured = configured_name(&crate::config::path());
            sync_name(&state, configured.as_deref())?;
            if identity::load(&state)?.is_some() {
                devices::fetch(&state, now)?;
            }
            status_text(&state, configured.as_deref(), now)
        }
        Some("connect") => {
            crate::permit::user_only("connecting a hub")?;
            connect::cli(&state, &crate::config::path(), name, interactive(), now)
        }
        Some("add-device") => {
            crate::permit::user_only("adding a device")?;
            connect::add_device_cli(&state, interactive(), now)
        }
        Some("join") => {
            crate::permit::user_only("joining a hub")?;
            let (Some(url), Some(code)) = (rest.first(), rest.get(1)) else { return Err("kebbie hub join needs the hub's address and a join code: kebbie hub join https://hub.example.com <join code>".to_string()) };
            let ask: Option<&AskName> = if interactive() { Some(&ask_name) } else { None };
            join_named(&state, &crate::config::path(), url, code, name, ask, now)
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
        Some("remove-device") => {
            crate::permit::user_only("removing a device")?;
            let wanted = rest.first().ok_or("kebbie hub remove-device needs the device's name or id, as kebbie hub status shows it")?;
            let identity = identity::load(&state)?.ok_or("this machine is not connected to a hub; kebbie hub connect first")?;
            let listed = devices::fetch(&state, now)?;
            let device = devices::find(&listed, &identity.devices, wanted)?;
            let said = devices::remove(&state, &device)?;
            let posted = tick().err().map_or(String::new(), |error| format!(" The board did not go out again yet: {error}."));
            crate::notice::Notice::new(crate::notice::Tone::Done, said).next(format!("The other devices keep seeing this board.{posted}")).ok()
        }
        Some("leave") => {
            crate::permit::user_only("leaving the hub")?;
            leave(&state)
        }
        Some("move") => {
            crate::permit::user_only("moving the hub")?;
            let url = rest.first().ok_or("kebbie hub move needs the hub's new address: kebbie hub move https://hub.example.com")?;
            move_to(&state, url)
        }
        Some(other) => Err(format!("kebbie hub knows connect, add-device, remove-device, join, pair, move, status and leave, not {other}")),
    }
}

pub fn config_line(state: Option<&Path>) -> String {
    let identity = state.and_then(|state| identity::load(state).ok().flatten());
    let value = identity.map_or("none".to_string(), |identity| format!("{:?}", identity.url));
    format!("hub.connected = {value}  (kebbie hub connect)\n  The hub this machine posts its board to, sealed to the phones it paired with; kebbie hub connect or join sets it, kebbie hub leave forgets it, kebbie hub move moves it. Kept in kebbie's state with the machine's keys, not in config.toml.")
}

fn check(level: Level, name: &str, fact: impl Into<String>, fix: Vec<String>) -> Check {
    Check { level, name: name.to_string(), fact: fact.into(), fix }
}

pub fn doctor(state: &Path, configured: Option<&str>, now: u64, reach: impl FnOnce(&str) -> std::result::Result<(), String>) -> Check {
    let name = "hub";
    let Ok(Some(identity)) = identity::load(state) else {
        return check(Level::Ok, name, "not connected; kebbie hub connect shows the board on your phone", Vec::new());
    };
    let relay = relay::load(state);
    let posted = relay.posted_at.map_or("no board posted yet".to_string(), |at| format!("board rev {} posted {}", relay.rev, ago(Some(at), now)));
    let fact = format!("{} as {} · {} paired · {posted}", identity.url, identity.name, identity.devices.len());
    if let Err(error) = reach(&identity.url) {
        return check(Level::Warn, name, format!("{fact}; not reachable: {error}"), vec!["check the address and your network; kebbie hub status shows the last error".to_string()]);
    }
    if identity.devices.is_empty() {
        return check(Level::Warn, name, format!("{fact}; no phone paired, so no board goes out"), vec!["kebbie hub add-device".to_string()]);
    }
    if let Some(plan) = &relay.plan_inactive {
        return check(Level::Warn, name, format!("{fact}; {plan}"), vec!["everything on this machine keeps working; the phone gets the board again once the subscription is active".to_string()]);
    }
    if let Some(error) = &relay.error {
        return check(Level::Warn, name, format!("{fact}; last post failed {}: {error}", ago(relay.error_at, now)), vec!["kebbie hub status; the keeper retries by itself".to_string()]);
    }
    match (rename_line(&identity, configured), &identity.rename_error) {
        (Some(line), Some(_)) if identity.rename_stopped.is_some() => check(Level::Warn, name, format!("{fact}; {line}"), vec![format!("set [machine] name = {:?}, or a name the hub keeps as it is", identity.name)]),
        (Some(line), Some(_)) => check(Level::Warn, name, format!("{fact}; {line}"), vec!["kebbie hub status renames it now and shows the error if it fails again".to_string()]),
        (Some(line), None) => check(Level::Ok, name, format!("{fact}; {line}"), Vec::new()),
        (None, _) => check(Level::Ok, name, fact, Vec::new()),
    }
}

pub fn reach(url: &str) -> std::result::Result<(), String> {
    Client::new(url, None, std::time::Duration::from_secs(5)).reachable().map_err(|error| error.to_string())
}
