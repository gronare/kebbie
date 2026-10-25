use std::path::Path;
use std::time::{Duration, Instant};

use kebbie_hub_api::{Bytes32, MAX_WAIT_SECONDS, PairingFailure, PairingRefusal, PairingReply, PairingState, PairingView};
use kebbie_hub_protocol::{AccountFingerprints, ConnectSecret, pin_account};

use super::http::{Client, HubError, LONG_POLL};
use super::identity::{self, Identity};
use crate::sh::Result;

pub const TIMEOUT: Duration = Duration::from_secs(600);
const STOPPABLE_WAIT: u64 = 2;

pub type Stop<'a> = Option<&'a (dyn Fn() -> bool + Sync)>;

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
}

impl Run<'_> {
    fn next(&self, seen: PairingState) -> Result<PairingView> {
        loop {
            if self.stop.is_some_and(|stop| stop()) {
                let _ = refuse(&self.client, &self.path, PairingFailure::Declined);
                return Err("stopped; nothing was pinned".to_string());
            }
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let _ = refuse(&self.client, &self.path, PairingFailure::Timeout);
                return Err(format!("the account handoff timed out; start again with {}", super::RECONNECT));
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
            Some(reason) => format!("the account handoff {}: {}; start again with {}", view.state.name(), reason.name(), super::RECONNECT),
            None => format!("the account handoff {}; start again with {}", view.state.name(), super::RECONNECT),
        }
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
            return Err(format!("the hub handed over an account key the browser did not commit to; nothing was pinned; start again with {}", super::RECONNECT));
        };
        let admitted: PairingView = self.client.post(&format!("{}/admit", self.path), &serde_json::Value::Null).map_err(hub_error)?;
        if admitted.state != PairingState::Admitted {
            return Err(self.ended(&admitted));
        }
        let mut current = identity::load(state)?.ok_or("this machine left the hub while connecting")?;
        current.pin(key, now);
        if let Some(moved) = self.client.moved() {
            current.url = moved;
        }
        identity::save(state, &current)?;
        Ok(fingerprints)
    }
}

pub fn pin(state: &Path, pairing: &str, secret: &ConnectSecret, timeout: Duration, stop: Stop, now: u64) -> Result<AccountFingerprints> {
    let identity = identity::load(state)?.ok_or("this machine is not connected to a hub; kebbie hub connect first")?;
    let run = Run { client: Client::new(&identity.url, Some(identity.token()), LONG_POLL), path: format!("/v1/m/pairings/{pairing}"), deadline: Instant::now() + timeout, stop };
    run.pin(state, &identity, secret, now)
}
