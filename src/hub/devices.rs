use std::path::{Path, PathBuf};

use kelpie_hub_api::{AdmittedDevice, AdmittedDevices};
use kelpie_hub_protocol::Id;
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
    let identity = identity::load(state)?.ok_or("this machine is not connected to a hub; kelpie hub connect first")?;
    let mut listed = load(state);
    match Client::new(&identity.url, Some(identity.token()), CALL).get::<AdmittedDevices>("/v1/m/devices") {
        Ok(answer) => {
            listed = Listed { devices: answer.devices, fetched_at: Some(now), error: None };
        }
        Err(error) if error.status() == Some(404) => listed.error = Some("this hub cannot list devices yet; update kelpie-hub".to_string()),
        Err(error) => listed.error = Some(error.to_string()),
    }
    save(state, &listed)?;
    Ok(listed)
}

pub fn name_of(device: &AdmittedDevice) -> String {
    device.name.clone().unwrap_or_else(|| format!("Device {}", short(&device.device)))
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
    let named: Vec<&AdmittedDevice> = listed.devices.iter().filter(|device| name_of(device).eq_ignore_ascii_case(wanted)).collect();
    match named.as_slice() {
        [device] => Ok(device.device.clone()),
        [] => {
            let known = listed.devices.iter().map(name_of).collect::<Vec<_>>();
            let known = if known.is_empty() { "this machine lists no devices".to_string() } else { format!("this machine's devices are {}", known.join(", ")) };
            Err(format!("no device named {wanted:?}; {known}"))
        }
        _ => Err(format!("{} devices are named {wanted:?}; give the id instead, as kelpie hub status shows it", named.len())),
    }
}

pub fn remove(state: &Path, device: &Id) -> Result<String> {
    let identity = identity::load(state)?.ok_or("this machine is not connected to a hub; kelpie hub connect first")?;
    let mut listed = load(state);
    let name = listed.devices.iter().find(|known| &known.device == device).map_or_else(|| format!("device {}", short(device)), name_of);
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
    Ok(format!("Removed {name}; it no longer sees this board."))
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

pub fn added_name(state: &Path, device: &Id, now: u64) -> String {
    fetch(state, now).ok().and_then(|listed| listed.devices.into_iter().find(|known| &known.device == device)).map_or_else(|| format!("device {}", short(device)), |known| name_of(&known))
}
