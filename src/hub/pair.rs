use std::path::Path;
use std::time::{Duration, Instant};

use kebbie_hub_api::{Bytes32, MAX_WAIT_SECONDS, PairingFailure, PairingOpened, PairingRefusal, PairingReply, PairingState, PairingView};
use kebbie_hub_protocol::{AccountFingerprints, ConnectSecret, Id, MachinePairing, pin_account};

use super::http::{Client, HubError, LONG_POLL};
use super::identity::{self, Device, Identity};
use crate::sh::Result;

pub const TIMEOUT: Duration = Duration::from_secs(600);
const STOPPABLE_WAIT: u64 = 2;

pub type Stop<'a> = Option<&'a (dyn Fn() -> bool + Sync)>;

// The six digits are the only proof the phone holds the keys it committed to, so a pairing nobody can answer is refused.
pub fn interactive_only(interactive: bool) -> Result<()> {
    if interactive { Ok(()) } else { Err("kebbie hub pair asks you to compare six digits with the phone, so it runs in your own terminal".to_string()) }
}

fn hub_error(error: HubError) -> String {
    error.to_string()
}

fn refuse(client: &Client, path: &str, reason: PairingFailure) -> Result<()> {
    client.post::<_, PairingView>(&format!("{path}/refuse"), &PairingRefusal { reason }).map(|_| ()).map_err(hub_error)
}

struct Run<'a> {
    client: Client,
    path: String,
    deadline: Instant,
    stop: Stop<'a>,
    again: &'a str,
}

impl Run<'_> {
    fn next(&self, seen: PairingState) -> Result<PairingView> {
        loop {
            if self.stop.is_some_and(|stop| stop()) {
                let _ = refuse(&self.client, &self.path, PairingFailure::Declined);
                return Err("stopped; nothing was paired".to_string());
            }
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let _ = refuse(&self.client, &self.path, PairingFailure::Timeout);
                return Err(format!("pairing timed out; start again with {}", self.again));
            }
            let longest = if self.stop.is_some() { STOPPABLE_WAIT } else { MAX_WAIT_SECONDS };
            let view: PairingView = self.client.get(&format!("{}?seen={}&wait={}", self.path, seen.name(), left.as_secs().clamp(1, longest))).map_err(hub_error)?;
            if view.state != seen {
                return Ok(view);
            }
        }
    }

    fn ended(&self, view: &PairingView) -> String {
        match view.reason {
            Some(reason) => format!("pairing {}: {}; start again with {}", view.state.name(), reason.name(), self.again),
            None => format!("pairing {}; start again with {}", view.state.name(), self.again),
        }
    }

    // One invocation takes exactly one commitment; a second chance would let a relay try substitute keys.
    fn finish(&self, state: &Path, identity: &Identity, code: &str, confirm: impl FnOnce(&str) -> bool, now: u64) -> Result<Id> {
        let committed = self.next(PairingState::Open)?;
        if committed.state != PairingState::Committed {
            return Err(self.ended(&committed));
        }
        let commitment = committed.commitment.ok_or("the hub left out the phone's commitment")?;
        let pairing = MachinePairing::answer(identity.public(), commitment.0, code);
        let (keys, nonce) = pairing.reply();
        self.client.post::<_, PairingView>(&format!("{}/reply", self.path), &PairingReply { keys, nonce: Bytes32(nonce) }).map_err(hub_error)?;
        let revealed = self.next(PairingState::Replied)?;
        if revealed.state != PairingState::Revealed {
            return Err(self.ended(&revealed));
        }
        let device = revealed.device.ok_or("the hub left out the device id")?;
        let device_keys = revealed.device_keys.ok_or("the hub left out the device's keys")?;
        let device_nonce = revealed.device_nonce.ok_or("the hub left out the device's nonce")?;
        let Ok(digits) = pairing.comparison_code(&device_keys, &device_nonce.0) else {
            refuse(&self.client, &self.path, PairingFailure::Mismatch)?;
            return Err(format!("pairing refused: the device revealed keys it did not commit to; start again with {}", self.again));
        };
        if !confirm(&digits) {
            refuse(&self.client, &self.path, PairingFailure::Declined)?;
            return Err("pairing refused: the digits did not match; nothing was paired".to_string());
        }
        let admitted: PairingView = self.client.post(&format!("{}/admit", self.path), &serde_json::Value::Null).map_err(hub_error)?;
        if admitted.state != PairingState::Admitted {
            return Err(self.ended(&admitted));
        }
        self.keep(state, |current| current.admit(Device { id: device.clone(), keys: device_keys, paired_at: now }))?;
        Ok(device)
    }

    fn keep(&self, state: &Path, change: impl FnOnce(&mut Identity)) -> Result<()> {
        let mut current = identity::load(state)?.ok_or("this machine left the hub while pairing")?;
        change(&mut current);
        if let Some(moved) = self.client.moved() {
            current.url = moved;
        }
        identity::save(state, &current)
    }

    // The secret reached the browser past the hub, in the link's fragment, so only an account key that hashes with it to the commitment is pinned.
    fn pin(&self, state: &Path, identity: &Identity, secret: &ConnectSecret, now: u64) -> Result<AccountFingerprints> {
        let committed = self.next(PairingState::Open)?;
        if committed.state != PairingState::Committed {
            return Err(self.ended(&committed));
        }
        let commitment = committed.commitment.ok_or("the hub left out the browser's commitment")?;
        self.client.post::<_, PairingView>(&format!("{}/reply", self.path), &PairingReply { keys: identity.public(), nonce: Bytes32(identity::random32()?) }).map_err(hub_error)?;
        let revealed = self.next(PairingState::Replied)?;
        if revealed.state != PairingState::Revealed {
            return Err(self.ended(&revealed));
        }
        let key = revealed.account_key.ok_or("the hub left out the account's key")?;
        let nonce = revealed.account_nonce.ok_or("the hub left out the account's nonce")?;
        let named = committed.account.as_ref().is_none_or(|account| *account == key.account);
        let Some(fingerprints) = pin_account(&commitment.0, &key, &nonce.0, &secret.pairing_code()).ok().filter(|_| named) else {
            refuse(&self.client, &self.path, PairingFailure::Mismatch)?;
            return Err(format!("the hub handed over an account key the browser did not commit to; nothing was pinned; start again with {}", self.again));
        };
        let admitted: PairingView = self.client.post(&format!("{}/admit", self.path), &serde_json::Value::Null).map_err(hub_error)?;
        if admitted.state != PairingState::Admitted {
            return Err(self.ended(&admitted));
        }
        self.keep(state, |current| current.pin(key, now))?;
        Ok(fingerprints)
    }
}

fn joined(state: &Path) -> Result<Identity> {
    identity::load(state)?.ok_or_else(|| "this machine is not connected to a hub; kebbie hub connect first".to_string())
}

fn run<'a>(identity: &Identity, pairing: &str, timeout: Duration, stop: Stop<'a>, again: &'a str) -> Run<'a> {
    Run { client: Client::new(&identity.url, Some(identity.token()), LONG_POLL), path: format!("/v1/m/pairings/{pairing}"), deadline: Instant::now() + timeout, stop, again }
}

pub fn pair(state: &Path, timeout: Duration, show_code: impl FnOnce(&str), confirm: impl FnOnce(&str) -> bool, now: u64) -> Result<Id> {
    let identity = joined(state)?;
    let opened: PairingOpened = Client::new(&identity.url, Some(identity.token()), LONG_POLL).post("/v1/m/pairings", &serde_json::Value::Null).map_err(hub_error)?;
    show_code(&opened.code);
    run(&identity, &opened.pairing, timeout, None, "kebbie hub pair").finish(state, &identity, &opened.code, confirm, now)
}

pub fn pin(state: &Path, pairing: &str, secret: &ConnectSecret, timeout: Duration, stop: Stop, now: u64) -> Result<AccountFingerprints> {
    let identity = joined(state)?;
    run(&identity, pairing, timeout, stop, "kebbie hub leave, then kebbie hub connect").pin(state, &identity, secret, now)
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
