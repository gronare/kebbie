use std::path::{Path, PathBuf};

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
pub struct Relay {
    pub rev: u64,
    pub posted_at: Option<u64>,
    pub fingerprint: Option<String>,
    pub error: Option<String>,
    pub error_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_inactive: Option<String>,
}

fn file(state: &Path) -> PathBuf {
    identity::dir(state).join(FILE)
}

pub fn load(state: &Path) -> Relay {
    std::fs::read_to_string(file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save(state: &Path, relay: &Relay) -> Result<()> {
    crate::state::write(&file(state), serde_json::to_string_pretty(relay).map_err(|error| error.to_string())?)
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

fn failed(state: &Path, relay: &mut Relay, error: &HubError, now: u64) -> String {
    relay.plan_inactive = matches!(error, HubError::PlanInactive { .. }).then(|| error.to_string());
    relay.error = Some(error.to_string());
    relay.error_at = Some(now);
    let _ = save(state, relay);
    error.to_string()
}

fn keep_move(state: &Path, client: &Client) {
    if let (Some(moved), Ok(Some(mut identity))) = (client.moved(), identity::load(state))
        && identity.url != moved
    {
        identity.url = moved;
        let _ = identity::save(state, &identity);
    }
}

// The heartbeat reposts an unchanged board, since the web app counts a machine silent for three minutes as asleep and the hub keeps a board a day at most; a revision of at least the time keeps it rising when relay.json is lost, as the web app refuses one that goes back.
pub fn relay(state: &Path, board: &Board, now: u64) -> Result<Option<u64>> {
    let Some(identity) = identity::load(state)?.filter(|identity| identity.account.is_some()) else { return Ok(None) };
    let mut relay = load(state);
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
            keep_move(state, &client);
            return Err(failed(state, &mut relay, &error, now));
        }
    };
    relay.rev = relay.rev.max(stored.rev);
    relay.posted_at = Some(now);
    relay.fingerprint = Some(print);
    relay.error = None;
    relay.error_at = None;
    relay.plan_inactive = None;
    save(state, &relay)?;
    keep_move(state, &client);
    Ok(Some(stored.rev))
}
