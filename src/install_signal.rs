use std::path::{Path, PathBuf};

use kebbie_hub_api::ReportChallenge;
use kebbie_hub_protocol::{Name, solve};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::hub::http::{CALL, Client};
use crate::sh::Result;

const DOMAIN: &[u8] = b"kelpie-install-v1";
const ID: &str = "install-id";
const LEDGER: &str = "install-signal.json";
const ID_BYTES: usize = 16;
pub const CONTAINS: &str = "a random id made for this install alone, kebbie's version, the OS and CPU architecture and the update channel";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Install {
    pub id: String,
    pub version: Name,
    pub os: Name,
    pub arch: Name,
    pub channel: Name,
}

impl Install {
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = DOMAIN.to_vec();
        for text in [self.id.as_str(), self.version.as_str(), self.os.as_str(), self.arch.as_str(), self.channel.as_str()] {
            bytes.extend((text.len() as u32).to_be_bytes());
            bytes.extend(text.as_bytes());
        }
        bytes
    }

    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.bytes()).into()
    }
}

// Unsigned on purpose: the install key would tie this install's daily signal to its reports and error signals.
#[derive(Serialize)]
struct Upload<'a> {
    install: &'a Install,
    challenge: ReportChallenge,
    counter: u64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Ledger {
    day: Option<u64>,
}

fn dir(state: &Path) -> PathBuf {
    crate::hub::report::dir(state)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_id(text: &str) -> bool {
    text.len() == ID_BYTES * 2 && text.chars().all(|character| matches!(character, '0'..='9' | 'a'..='f'))
}

pub fn install_id(state: &Path) -> Result<String> {
    let file = dir(state).join(ID);
    if let Ok(text) = std::fs::read_to_string(&file)
        && is_id(text.trim())
    {
        return Ok(text.trim().to_string());
    }
    let id = hex(&crate::hub::identity::random32()?[..ID_BYTES]);
    crate::state::write(&file, format!("{id}\n"))?;
    Ok(id)
}

fn name(text: &str) -> Name {
    Name::new(text).unwrap_or_else(|| Name::new("unknown").expect("unknown is a name"))
}

pub fn payload(state: &Path, channel: crate::config::Channel) -> Result<Install> {
    Ok(Install { id: install_id(state)?, version: name(env!("CARGO_PKG_VERSION")), os: name(std::env::consts::OS), arch: name(std::env::consts::ARCH), channel: name(channel.key()) })
}

// The day is written down before the send, so a failed or interrupted one is dropped rather than tried again that day.
pub fn tick_in(state: &Path, enabled: bool, now: u64, channel: crate::config::Channel, send: &mut dyn FnMut(&Install) -> Result<()>) -> bool {
    if !enabled {
        let _ = std::fs::remove_file(dir(state).join(ID));
        let _ = std::fs::remove_file(dir(state).join(LEDGER));
        return false;
    }
    let file = dir(state).join(LEDGER);
    let today = now / crate::signals::DAY;
    let ledger: Ledger = std::fs::read_to_string(&file).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    if ledger.day == Some(today) {
        return false;
    }
    let Ok(text) = serde_json::to_string(&Ledger { day: Some(today) }) else { return false };
    if crate::state::write(&file, text).is_err() {
        return false;
    }
    let Ok(install) = payload(state, channel) else { return false };
    let _ = send(&install);
    true
}

pub fn send(url: &str, install: &Install) -> Result<()> {
    let client = Client::new(&crate::hub::hub_url(url)?, None, CALL);
    let challenge: ReportChallenge = client.get("/v1/reports/challenge").map_err(|error| error.to_string())?;
    if challenge.difficulty > crate::hub::report::MAX_WORK {
        return Err(format!("the signals endpoint asks for {} bits of work, more than kebbie does", challenge.difficulty));
    }
    let counter = solve(&challenge.nonce.0, &install.digest(), challenge.difficulty);
    client.post::<_, serde_json::Value>("/v1/signals", &Upload { install, challenge, counter }).map(|_| ()).map_err(|error| error.to_string())
}

pub fn tick(state: &Path) {
    let Ok(settings) = crate::config::report_settings() else { return };
    let channel = crate::config::channel().unwrap_or_default();
    let url = settings.url().to_string();
    let _ = crate::signals::one_sender(state, || {
        tick_in(state, settings.signals(), crate::messages::now(), channel, &mut |install| send(&url, install));
        Ok(Vec::new())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Channel;

    const NOW: u64 = 1_792_540_800;

    fn keys(value: &serde_json::Value) -> Vec<String> {
        value.as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn the_daily_signal_holds_the_install_id_version_os_arch_and_channel_and_nothing_else() {
        let state = crate::sh::test_dir("install-signal-shape");
        let install = payload(&state, Channel::Main).unwrap();
        let wire = serde_json::to_value(&install).unwrap();
        assert_eq!(keys(&wire), ["arch", "channel", "id", "os", "version"]);
        assert_eq!((wire["version"].as_str(), wire["os"].as_str(), wire["arch"].as_str(), wire["channel"].as_str()), (Some(env!("CARGO_PKG_VERSION")), Some(std::env::consts::OS), Some(std::env::consts::ARCH), Some("main")));
        let challenge = ReportChallenge { nonce: kebbie_hub_api::Bytes32([1; 32]), difficulty: 0, expires: NOW, tag: kebbie_hub_api::Bytes32([2; 32]) };
        let upload = serde_json::to_value(Upload { install: &install, challenge, counter: 7 }).unwrap();
        assert_eq!(keys(&upload), ["challenge", "counter", "install"], "no key and no signature go with it");
    }

    #[test]
    fn the_install_id_is_random_made_once_and_kept() {
        let state = crate::sh::test_dir("install-signal-id");
        let id = install_id(&state).unwrap();
        assert!(is_id(&id), "{id}");
        assert_eq!(install_id(&state).unwrap(), id, "kept across sends");
        assert_ne!(install_id(&crate::sh::test_dir("install-signal-other")).unwrap(), id, "another install makes its own");
        crate::state::write(&dir(&state).join(ID), "/Users/carl\n").unwrap();
        assert!(is_id(&install_id(&state).unwrap()), "a file that holds anything else is replaced");
    }

    #[test]
    fn the_digest_matches_the_endpoints_vector() {
        let install = Install { id: "0123456789abcdef0123456789abcdef".to_string(), version: name("0.28.1"), os: name("macos"), arch: name("aarch64"), channel: name("release") };
        assert_eq!(hex(&install.digest()), "776613dfb50beeff592247c55ccb8fd39a310351863c22ca67e4b5d381d650a2");
    }

    #[test]
    fn it_goes_at_most_once_a_utc_day_and_a_failed_send_is_dropped() {
        let state = crate::sh::test_dir("install-signal-daily");
        let mut sent = Vec::new();
        assert!(tick_in(&state, true, NOW, Channel::Release, &mut |install| { sent.push(install.clone()); Err("the endpoint is down".to_string()) }));
        assert!(!tick_in(&state, true, NOW + 3600, Channel::Release, &mut |_| panic!("a second send the same day")), "a failed send is not tried again that day");
        assert!(tick_in(&state, true, NOW + crate::signals::DAY, Channel::Release, &mut |install| { sent.push(install.clone()); Ok(()) }));
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].id, sent[1].id, "the same install from day to day");
        assert!(crate::messages::load_in(&state).is_empty(), "it goes quietly");
    }

    #[test]
    fn with_signals_off_nothing_is_sent_and_the_id_is_forgotten() {
        let state = crate::sh::test_dir("install-signal-off");
        let id = install_id(&state).unwrap();
        assert!(!tick_in(&state, false, NOW, Channel::Release, &mut |_| panic!("signals are off")));
        assert!(!dir(&state).join(ID).exists());
        assert_ne!(install_id(&state).unwrap(), id, "turned on again, it is a new install");
    }

    #[test]
    fn the_keeper_runs_it_every_hour() {
        assert!(crate::keeper::routines().iter().any(|routine| routine.name == "install" && routine.every == crate::signals::EVERY));
    }
}
