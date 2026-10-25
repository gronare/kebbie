use std::path::{Path, PathBuf};

use kebbie_hub_api::{AdmittedDevice, AdmittedDevices};
use kebbie_hub_protocol::Id;
use serde::{Deserialize, Serialize};

use super::http::{CALL, Client, HubError};
use super::identity;
use crate::sh::Result;

const FILE: &str = "devices.json";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Listed {
    pub devices: Vec<AdmittedDevice>,
    pub fetched_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outdated: bool,
}

fn file(state: &Path) -> PathBuf {
    identity::dir(state).join(FILE)
}

pub fn load(state: &Path) -> Listed {
    std::fs::read_to_string(file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save(state: &Path, listed: &Listed) -> Result<()> {
    crate::state::write(&file(state), serde_json::to_string_pretty(listed).map_err(|error| error.to_string())?)
}

// The last list the hub gave stays, with the error beside it, so an unreachable hub still shows names and when they were true.
pub fn fetch(state: &Path, now: u64) -> Result<Listed> {
    let identity = identity::load(state)?.ok_or("this machine is not connected to a hub; kebbie hub connect first")?;
    let mut listed = load(state);
    match Client::new(&identity.url, Some(identity.token()), CALL).get::<AdmittedDevices>("/v1/m/devices") {
        Ok(answer) => {
            listed = Listed { devices: answer.devices, fetched_at: Some(now), error: None, outdated: false };
        }
        Err(error) if matches!(error.status(), Some(404 | 405)) => (listed.error, listed.outdated) = (Some("this hub cannot list devices yet; update kebbie-hub".to_string()), true),
        Err(error) => (listed.error, listed.outdated) = (Some(error.to_string()), false),
    }
    save(state, &listed)?;
    Ok(listed)
}

pub const UNNAMED: &str = "Unnamed device";

pub fn named(id: &Id, listed: &Listed) -> Option<String> {
    listed.devices.iter().find(|known| &known.device == id).and_then(|known| known.name.clone())
}

pub fn name_of(device: &AdmittedDevice) -> String {
    device.name.clone().unwrap_or_else(|| format!("{UNNAMED} {}", short(&device.device)))
}

fn called(id: &Id, listed: &Listed) -> String {
    named(id, listed).unwrap_or_else(|| format!("{UNNAMED} {}", short(id)))
}

pub fn short(id: &Id) -> String {
    let text = id.as_str();
    text.chars().skip(text.chars().count().saturating_sub(6)).collect()
}

pub fn find(listed: &Listed, paired: &[identity::Device], wanted: &str) -> Result<Id> {
    let wanted = wanted.trim();
    if let Some(device) = paired.iter().find(|device| device.id.as_str() == wanted) {
        return Ok(device.id.clone());
    }
    let ids: Vec<&Id> = paired.iter().map(|device| &device.id).chain(listed.devices.iter().map(|device| &device.device).filter(|id| !paired.iter().any(|known| &known.id == *id))).collect();
    let named: Vec<&Id> = ids.iter().copied().filter(|id| called(id, listed).eq_ignore_ascii_case(wanted)).collect();
    match named.as_slice() {
        [device] => Ok((*device).clone()),
        [] => {
            let known = ids.iter().map(|id| called(id, listed)).collect::<Vec<_>>();
            let known = if known.is_empty() { "this machine lists no devices".to_string() } else { format!("this machine's devices are {}", known.join(", ")) };
            Err(format!("no device named {wanted:?}; {known}"))
        }
        _ => Err(format!("{} devices are named {wanted:?}; give the id instead, as kebbie hub status shows it", named.len())),
    }
}

pub fn remove(state: &Path, device: &Id) -> Result<String> {
    let identity = identity::load(state)?.ok_or("this machine is not connected to a hub; kebbie hub connect first")?;
    let mut listed = load(state);
    let name = named(device, &listed).unwrap_or_else(|| format!("device {}", short(device)));
    match Client::new(&identity.url, Some(identity.token()), CALL).delete(&format!("/v1/m/devices/{device}")) {
        Ok(()) => {}
        Err(HubError::Status { status: 404, .. }) => {}
        Err(error) => return Err(format!("{name} was not removed: {error}")),
    }
    let mut current = identity::load(state)?.ok_or("this machine left the hub while removing the device")?;
    current.devices.retain(|known| &known.id != device);
    identity::save(state, &current)?;
    listed.devices.retain(|known| &known.device != device);
    save(state, &listed)?;
    Ok(format!("Removed {name}; it gets no more notices from this machine."))
}

pub fn ago(at: u64, now: u64) -> String {
    let seconds = now.saturating_sub(at);
    match seconds {
        0..60 => format!("{seconds} s ago"),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => on_day(at, now),
    }
}

pub fn on_day(at: u64, now: u64) -> String {
    let day = |at: u64| (at as i64 + crate::routines::local_offset(at)).div_euclid(86_400);
    match day(now) - day(at) {
        ..=0 => "today".to_string(),
        1 => "yesterday".to_string(),
        days => format!("{days} days ago"),
    }
}

pub fn facts(device: &AdmittedDevice, now: u64) -> String {
    let mut facts = vec![format!("paired {}", on_day(device.paired_at, now))];
    facts.extend(device.last_seen.map(|at| format!("seen {}", ago(at, now))));
    if device.notifications {
        facts.push("notifications on".to_string());
    }
    facts.join(" · ")
}
