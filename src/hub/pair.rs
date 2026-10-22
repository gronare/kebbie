use std::path::Path;
use std::time::{Duration, Instant};

use kelpie_hub_api::{Bytes32, MAX_WAIT_SECONDS, PairingFailure, PairingOpened, PairingRefusal, PairingReply, PairingState, PairingView};
use kelpie_hub_protocol::{Id, MachinePairing};

use super::http::{Client, HubError, LONG_POLL};
use super::identity::{self, Device};
use crate::sh::Result;

pub const TIMEOUT: Duration = Duration::from_secs(600);

// The six digits are the only proof the phone holds the keys it committed to, so a pairing nobody can answer is refused.
pub fn interactive_only(interactive: bool) -> Result<()> {
    if interactive { Ok(()) } else { Err("kelpie hub pair asks you to compare six digits with the phone, so it runs in your own terminal".to_string()) }
}

fn hub_error(error: HubError) -> String {
    error.to_string()
}

fn refuse(client: &Client, path: &str, reason: PairingFailure) -> Result<()> {
    client.post::<_, PairingView>(&format!("{path}/refuse"), &PairingRefusal { reason }).map(|_| ()).map_err(hub_error)
}

fn next(client: &Client, path: &str, seen: PairingState, deadline: Instant) -> Result<PairingView> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            let _ = refuse(client, path, PairingFailure::Timeout);
            return Err("pairing timed out; start again with kelpie hub pair".to_string());
        }
        let wait = left.as_secs().clamp(1, MAX_WAIT_SECONDS);
        let view: PairingView = client.get(&format!("{path}?seen={}&wait={wait}", seen.name())).map_err(hub_error)?;
        if view.state != seen {
            return Ok(view);
        }
    }
}

fn ended(view: &PairingView) -> String {
    match view.reason {
        Some(reason) => format!("pairing {}: {}; start again with kelpie hub pair", view.state.name(), reason.name()),
        None => format!("pairing {}; start again with kelpie hub pair", view.state.name()),
    }
}

// One invocation takes exactly one commitment; a second chance would let a relay try substitute keys.
pub fn pair(state: &Path, timeout: Duration, show_code: impl FnOnce(&str), confirm: impl FnOnce(&str) -> bool, now: u64) -> Result<Id> {
    let identity = identity::load(state)?.ok_or("this machine has not joined a hub; kelpie hub join <url> <join code> first")?;
    let client = Client::new(&identity.url, Some(identity.token()), LONG_POLL);
    let opened: PairingOpened = client.post("/v1/m/pairings", &serde_json::Value::Null).map_err(hub_error)?;
    show_code(&opened.code);
    let path = format!("/v1/m/pairings/{}", opened.pairing);
    let deadline = Instant::now() + timeout;
    let committed = next(&client, &path, PairingState::Open, deadline)?;
    if committed.state != PairingState::Committed {
        return Err(ended(&committed));
    }
    let commitment = committed.commitment.ok_or("the hub left out the phone's commitment")?;
    let pairing = MachinePairing::answer(identity.public(), commitment.0, &opened.code);
    let (keys, nonce) = pairing.reply();
    client.post::<_, PairingView>(&format!("{path}/reply"), &PairingReply { keys, nonce: Bytes32(nonce) }).map_err(hub_error)?;
    let revealed = next(&client, &path, PairingState::Replied, deadline)?;
    if revealed.state != PairingState::Revealed {
        return Err(ended(&revealed));
    }
    let device = revealed.device.ok_or("the hub left out the device id")?;
    let device_keys = revealed.device_keys.ok_or("the hub left out the device's keys")?;
    let device_nonce = revealed.device_nonce.ok_or("the hub left out the device's nonce")?;
    let Ok(digits) = pairing.comparison_code(&device_keys, &device_nonce.0) else {
        refuse(&client, &path, PairingFailure::Mismatch)?;
        return Err("pairing refused: the phone revealed keys it did not commit to; start again with kelpie hub pair".to_string());
    };
    if !confirm(&digits) {
        refuse(&client, &path, PairingFailure::Declined)?;
        return Err("pairing refused: the digits did not match; nothing was paired".to_string());
    }
    let admitted: PairingView = client.post(&format!("{path}/admit"), &serde_json::Value::Null).map_err(hub_error)?;
    if admitted.state != PairingState::Admitted {
        return Err(ended(&admitted));
    }
    let mut current = identity::load(state)?.ok_or("this machine left the hub while pairing")?;
    current.admit(Device { id: device.clone(), keys: device_keys, paired_at: now });
    if let Some(moved) = client.moved() {
        current.url = moved;
    }
    identity::save(state, &current)?;
    Ok(device)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_runs_only_where_someone_can_compare_the_digits() {
        assert!(interactive_only(true).is_ok());
        assert!(interactive_only(false).unwrap_err().contains("your own terminal"));
    }
}
