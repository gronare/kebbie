use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kebbie_hub_api::{AccountMachine, AccountMachines, MessageQueued, MessageUpload};
use kebbie_hub_protocol::{AccountKey, Envelope, Header, Id, Payload, Prefix, PublicKeys, open, seal};

use crate::hub::http::{Client, HubError};
use crate::hub::identity::Identity;
use crate::sh::Result;

const FILE: &str = "jobs/machines.json";
const PIN: &str = "jobs/machines.counter";
const PIN_LOCK: &str = "jobs/machines.lock";
const FRESH: u64 = 300;

// The machines of the account as the account itself signed them, keys and all; the hub's rows beside them are hints for picking a worker, kept only for a machine on the signed list.
#[derive(Clone, Debug)]
pub struct Cluster {
    pub counter: u64,
    pub keys: BTreeMap<Id, PublicKeys>,
    pub rows: Vec<AccountMachine>,
}

impl Cluster {
    pub fn name_of(&self, machine: &Id) -> String {
        self.rows.iter().find(|row| &row.machine == machine).map_or_else(|| machine.to_string(), |row| row.name.clone())
    }
}

pub fn verify(listed: &AccountMachines, account: &AccountKey) -> Result<Cluster> {
    let signed = listed.list.as_ref().filter(|signed| account.verifies_list(&signed.list, &signed.signature.0))
        .ok_or("the hub's list of this account's machines is not signed by the account, so no machine on it is trusted; open the hub in a browser signed in to the account to sign it again")?;
    let keys: BTreeMap<Id, PublicKeys> = signed.list.machines.iter().map(|listed| (listed.machine.clone(), listed.keys)).collect();
    let rows = listed.machines.iter().filter(|row| keys.contains_key(&row.machine)).cloned().collect();
    Ok(Cluster { counter: signed.list.counter, keys, rows })
}

fn file(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

fn pinned(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join(PIN)).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0)
}

// The account raises the counter with every change, so a list below the highest one seen is the hub handing back one the account replaced, as one that still holds a removed machine.
pub fn trust(dir: &Path, listed: &AccountMachines, account: &AccountKey) -> Result<Cluster> {
    let cluster = verify(listed, account)?;
    let _held = crate::state::Locked::wait(&dir.join(PIN_LOCK))?;
    let pin = pinned(dir);
    if cluster.counter < pin {
        return Err(format!("the hub handed over the account's list of machines at version {}, older than version {pin} this machine already saw, so no machine on it is trusted", cluster.counter));
    }
    if cluster.counter > pin {
        crate::state::write(&dir.join(PIN), cluster.counter.to_string())?;
    }
    Ok(cluster)
}

pub fn fetch(dir: &Path, client: &Client, account: &AccountKey) -> std::result::Result<Cluster, HubError> {
    let listed: AccountMachines = client.get("/v1/m/machines")?;
    let cluster = trust(dir, &listed, account).map_err(HubError::Transport)?;
    if let Ok(text) = serde_json::to_string(&listed) {
        let _ = crate::state::write(&file(dir), text);
    }
    Ok(cluster)
}

fn young(file: &Path) -> bool {
    std::fs::metadata(file).and_then(|meta| meta.modified()).ok().and_then(|at| at.elapsed().ok()).is_some_and(|age| age.as_secs() < FRESH)
}

fn cached(dir: &Path, account: &AccountKey) -> Option<Cluster> {
    if !young(&file(dir)) {
        return None;
    }
    let listed: AccountMachines = std::fs::read_to_string(file(dir)).ok().and_then(|text| serde_json::from_str(&text).ok())?;
    trust(dir, &listed, account).ok()
}

// A job's sender is checked against the list as the hub has it now, so a machine the account removed sends none; a report may use a list cached for five minutes.
pub fn sender(dir: &Path, client: &Client, account: &AccountKey, from: &Id, fresh: bool) -> Result<(PublicKeys, String)> {
    let known = |cluster: &Cluster| cluster.keys.get(from).map(|keys| (*keys, cluster.name_of(from)));
    if !fresh && let Some(found) = cached(dir, account).as_ref().and_then(known) {
        return Ok(found);
    }
    let cluster = fetch(dir, client, account).map_err(|error| error.to_string())?;
    known(&cluster).ok_or_else(|| format!("{from} is not on the account's signed list of machines"))
}

pub fn open_message(envelope: &Envelope, identity: &Identity, sender: &PublicKeys, now: u64) -> Result<Payload> {
    let header = &envelope.header;
    if !header.kind.is_machine_message() || header.machine != identity.machine {
        return Err("not a message from one machine to this one".to_string());
    }
    let payload = open(envelope, &identity.machine, &identity.secret(), &header.from, sender, now).map_err(|error| format!("it does not open: {}", error.key()))?;
    payload.validate().map_err(|problem| format!("it does not hold together: {problem}"))?;
    Ok(payload)
}

// Sealed to exactly the target, this machine and the account, and signed with this machine's own key, as the hub takes a machine message.
pub fn seal_to(identity: &Identity, account: &AccountKey, to: &Id, keys: &PublicKeys, id: Id, payload: &Payload, at: u64) -> Result<Envelope> {
    payload.validate().map_err(|problem| problem.to_string())?;
    let header = Header::new(id, payload.kind(), identity.machine.clone(), to.clone(), at);
    let recipients = [(to.clone(), *keys), (identity.machine.clone(), identity.public()), account.recipient()];
    seal(header, payload, &identity.secret(), &recipients).map_err(|error| error.to_string())
}

pub fn post(client: &Client, envelope: &Envelope) -> std::result::Result<MessageQueued, HubError> {
    client.post("/v1/m/messages", &MessageUpload { envelope: envelope.clone() })
}

pub fn new_id() -> Result<Id> {
    Id::new(Prefix::Command).map_err(|error| error.to_string())
}

// A worker's keeper reaches the hub at least every ten minutes, renewing it as a worker, so one not heard from for longer is off or its keeper stopped.
pub const STALE: u64 = 600;

fn workers<'a>(cluster: &'a Cluster, me: &'a Id, on: Option<&'a str>) -> impl Iterator<Item = &'a AccountMachine> {
    let takes = move |row: &&AccountMachine| on.is_none_or(|on| row.worker_os.as_deref().is_some_and(|os| os.eq_ignore_ascii_case(on)) || row.name.eq_ignore_ascii_case(on) || super::token(&row.name) == super::token(on));
    cluster.rows.iter().filter(move |row| row.worker_os.is_some() && &row.machine != me).filter(takes)
}

pub fn pick<'a>(cluster: &'a Cluster, me: &'a Id, on: Option<&'a str>, now: u64) -> Option<&'a AccountMachine> {
    workers(cluster, me, on).filter(|row| row.last_seen.is_some_and(|seen| now.saturating_sub(seen) <= STALE)).max_by_key(|row| row.last_seen.unwrap_or(0))
}

// The worker heard from last of those the hub has not heard from lately.
pub fn quiet<'a>(cluster: &'a Cluster, me: &'a Id, on: Option<&'a str>) -> Option<&'a AccountMachine> {
    workers(cluster, me, on).max_by_key(|row| row.last_seen.unwrap_or(0))
}
