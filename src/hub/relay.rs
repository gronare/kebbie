use std::path::Path;

use kebbie_hub_api::{BoardStored, BoardUpload};
use kebbie_hub_protocol::{Board, Envelope, Header, Id, Kind, Payload, Prefix, seal};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::http::{CALL, Client, HubError};
use super::identity::{self, Identity};
use crate::sh::Result;

pub const HEARTBEAT: u64 = 60;
const FILE: &str = "relay.json";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "Stored", into = "Stored")]
pub struct Relay {
    pub rev: u64,
    pub posted_at: Option<u64>,
    pub fingerprint: Option<String>,
    pub error: Option<String>,
    pub error_at: Option<u64>,
    pub not_enabled: bool,
}

impl Relay {
    pub fn not_enabled_error(&self) -> Option<&str> {
        self.error.as_deref().filter(|_| self.not_enabled)
    }
}

// relay.json repeats the error under plan_inactive, as older kebbies read it there.
#[derive(Serialize, Deserialize)]
struct Stored {
    rev: u64,
    posted_at: Option<u64>,
    fingerprint: Option<String>,
    error: Option<String>,
    error_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan_inactive: Option<String>,
}

impl From<Stored> for Relay {
    fn from(stored: Stored) -> Relay {
        Relay { rev: stored.rev, posted_at: stored.posted_at, fingerprint: stored.fingerprint, error: stored.error, error_at: stored.error_at, not_enabled: stored.plan_inactive.is_some() }
    }
}

impl From<Relay> for Stored {
    fn from(relay: Relay) -> Stored {
        let plan_inactive = relay.not_enabled_error().map(str::to_string);
        Stored { rev: relay.rev, posted_at: relay.posted_at, fingerprint: relay.fingerprint, error: relay.error, error_at: relay.error_at, plan_inactive }
    }
}

pub fn load(state: &Path) -> Relay {
    load_at(&identity::dir(state))
}

fn load_at(dir: &Path) -> Relay {
    std::fs::read_to_string(dir.join(FILE)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save(dir: &Path, relay: &Relay) -> Result<()> {
    crate::state::write(&dir.join(FILE), serde_json::to_string_pretty(relay).map_err(|error| error.to_string())?)
}

fn hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

// Who the board is sealed to counts, so a newly pinned account gets it while nothing on the board changed.
pub fn fingerprint(board: &Board, identity: &Identity) -> String {
    let mut unrevised = board.clone();
    unrevised.rev = 0;
    let account = identity.account.as_ref().map(|account| account.prints().1).unwrap_or_default();
    hash(&format!("{}\n{account}", serde_json::to_string(&unrevised).unwrap_or_default()))
}

// Only the account the machine pinned and the machine itself read a board, so a machine without one posts none.
pub fn seal_board(identity: &Identity, board: &Board, rev: u64, now: u64) -> Result<Envelope> {
    let account = identity.account.as_ref().ok_or_else(|| format!("No board goes out: {}", super::UNREAD))?;
    let mut board = board.clone();
    board.rev = rev;
    let header = Header::new(Id::new(Prefix::Board).map_err(|error| error.to_string())?, Kind::Board, identity.machine.clone(), identity.machine.clone(), now);
    seal(header, &Payload::Board(board), &identity.secret(), &[(identity.machine.clone(), identity.public()), account.key.recipient()]).map_err(|error| error.to_string())
}

fn failed(dir: &Path, relay: &mut Relay, error: &HubError, now: u64) -> String {
    relay.not_enabled = matches!(error, HubError::NotEnabled { .. });
    relay.error = Some(error.to_string());
    relay.error_at = Some(now);
    let _ = save(dir, relay);
    error.to_string()
}

fn keep_move(dir: &Path, client: &Client) {
    if let (Some(moved), Ok(Some(mut identity))) = (client.moved(), identity::load_at(dir))
        && identity.url != moved
    {
        identity.url = moved;
        let _ = identity::save_at(dir, &identity);
    }
}

// The heartbeat reposts an unchanged board, since the web app counts a machine silent for three minutes as asleep and the hub keeps a board a day at most; a revision of at least the time keeps it rising when relay.json is lost, as the web app refuses one that goes back.
#[cfg(test)]
pub fn relay(state: &Path, board: &Board, now: u64) -> Result<Option<u64>> {
    relay_at(&identity::dir(state), board, now)
}

pub fn relay_at(dir: &Path, board: &Board, now: u64) -> Result<Option<u64>> {
    let Some(identity) = identity::load_at(dir)?.filter(|identity| identity.account.is_some()) else { return Ok(None) };
    let mut relay = load_at(dir);
    let print = fingerprint(board, &identity);
    let heartbeat_due = relay.posted_at.is_none_or(|at| now.saturating_sub(at) >= HEARTBEAT);
    if relay.fingerprint.as_deref() == Some(print.as_str()) && !heartbeat_due {
        return Ok(None);
    }
    let client = Client::new(&identity.url, Some(identity.token()), CALL);
    let rev = (relay.rev + 1).max(now);
    let envelope = seal_board(&identity, board, rev, now)?;
    let stored: BoardStored = match client.put("/v1/m/board", &BoardUpload { rev, envelope }) {
        Ok(stored) => stored,
        Err(error) => {
            keep_move(dir, &client);
            return Err(failed(dir, &mut relay, &error, now));
        }
    };
    relay.rev = relay.rev.max(stored.rev);
    relay.posted_at = Some(now);
    relay.fingerprint = Some(print);
    relay.error = None;
    relay.error_at = None;
    relay.not_enabled = false;
    save(dir, &relay)?;
    keep_move(dir, &client);
    Ok(Some(stored.rev))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relay_file_keeps_a_refusal_where_older_kebbies_read_it_and_one_written_before_still_loads() {
        let refusal = "This hub has not enabled answering for your account".to_string();
        let written = serde_json::json!({ "rev": 7, "posted_at": 10, "fingerprint": "ab", "error": refusal, "error_at": 20, "plan_inactive": refusal });
        let relay: Relay = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(relay.not_enabled_error(), Some(refusal.as_str()));
        assert_eq!(serde_json::to_value(&relay).unwrap(), written, "an older kebbie reads the refusal where it wrote it");
        let posted = serde_json::json!({ "rev": 7, "posted_at": 10, "fingerprint": "ab", "error": null, "error_at": null });
        let relay: Relay = serde_json::from_value(posted.clone()).unwrap();
        assert_eq!((relay.not_enabled, relay.not_enabled_error()), (false, None));
        assert_eq!(serde_json::to_value(&relay).unwrap(), posted);
    }
}
