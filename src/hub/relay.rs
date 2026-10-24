use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kelpie_hub_api::{BoardStored, BoardUpload, NoticePosted, NoticeUpload};
use kelpie_hub_protocol::{Board, CiState, Envelope, Event, Group, Header, Id, Item, Kind, Notice, Payload, PublicKeys, Prefix, Urgency, seal};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::http::{CALL, Client, HubError};
use super::identity::{self, Identity};
use crate::sh::Result;

pub const HEARTBEAT: u64 = 60;
const FILE: &str = "relay.json";
const FORGET_AFTER: u64 = 86_400;
const TITLE_CHARS: usize = 80;
const BODY_CHARS: usize = 300;
const OPTION_CHARS: usize = 100;
const LONGEST: usize = 1_000;
// A Web Push body is 4096 bytes, of which the aes128gcm header (86), the padding delimiter (1) and the tag (16) are not the envelope.
pub const PUSH_ENVELOPE_BYTES: usize = 4096 - 86 - 1 - 16;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Relay {
    pub rev: u64,
    pub posted_at: Option<u64>,
    pub fingerprint: Option<String>,
    pub error: Option<String>,
    pub error_at: Option<u64>,
    #[serde(default)]
    pub primed: bool,
    #[serde(default)]
    pub notices: BTreeMap<String, u64>,
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

#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub key: String,
    pub urgency: Urgency,
    pub notice: Notice,
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub posted: Option<u64>,
    pub notices: Vec<String>,
}

fn clip(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(chars.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

fn hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn task_of(address: &str) -> &str {
    address.rsplit_once('@').map_or(address, |(task, _)| task)
}

// One notice per task and moment it needs you, keyed by what makes that moment new: the question asked, the head ready, the run that failed.
fn pending(item: &Item) -> Option<Pending> {
    let task = task_of(&item.address);
    let body = item.next_for_you.clone().unwrap_or_else(|| item.summary.clone());
    let notice = |event: Event, title: String, body: String| Notice { event, address: Some(item.address.clone()), title, body, question: None, options: Vec::new(), recommend: None, board_rev: 0, retract: None };
    if let Some(question) = &item.question {
        let mut asked = notice(Event::Question, format!("{task} asks"), question.text.clone());
        asked.question = Some(question.text.clone());
        asked.options = question.options.iter().take(usize::from(kelpie_hub_protocol::MAX_OPTIONS)).map(|option| option.label.clone()).collect();
        asked.recommend = question.recommend;
        return Some(Pending { key: format!("question:{}:{}", item.address, question.id), urgency: Urgency::High, notice: asked });
    }
    if let Some(ci) = item.ci.as_ref().filter(|ci| ci.state == CiState::Failed) {
        let failure = ci.failure.clone().unwrap_or_default();
        let key = format!("ci-failed:{}:{}:{}", item.address, item.head.as_deref().unwrap_or_default(), hash(&failure));
        return Some(Pending { key, urgency: Urgency::High, notice: notice(Event::CiFailed, format!("{task}: CI failed"), if failure.is_empty() { body } else { failure }) });
    }
    match item.group {
        Group::NeedsYou => {
            let at = item.report.as_ref().map_or(0, |report| report.at);
            Some(Pending { key: format!("needs-you:{}:{at}:{}", item.address, hash(&body)), urgency: Urgency::High, notice: notice(Event::NeedsYou, format!("{task} needs you"), body) })
        }
        Group::ReadyToPush => {
            let key = format!("ready:{}:{}", item.address, item.head.as_deref().unwrap_or_default());
            Some(Pending { key, urgency: Urgency::Normal, notice: notice(Event::Ready, format!("{task} is ready to push"), item.summary.clone()) })
        }
        _ => None,
    }
}

pub fn notices(board: &Board) -> Vec<Pending> {
    board.items.iter().filter_map(pending).collect()
}

fn recipients(identity: &Identity) -> Vec<(Id, PublicKeys)> {
    identity.devices.iter().map(|device| (device.id.clone(), device.keys)).collect()
}

// Who the board is sealed to counts, so a newly paired phone gets it while nothing on the board changed.
pub fn fingerprint(board: &Board, identity: &Identity) -> String {
    let mut unrevised = board.clone();
    unrevised.rev = 0;
    let devices: Vec<&str> = identity.devices.iter().map(|device| device.id.as_str()).collect();
    hash(&format!("{}\n{}", serde_json::to_string(&unrevised).unwrap_or_default(), devices.join(",")))
}

fn sealed(identity: &Identity, payload: &Payload, header: Header, with_machine: bool) -> Result<Envelope> {
    let mut to = recipients(identity);
    if with_machine {
        to.insert(0, (identity.machine.clone(), identity.public()));
    }
    seal(header, payload, &identity.secret(), &to).map_err(|error| error.to_string())
}

pub fn seal_board(identity: &Identity, board: &Board, rev: u64, now: u64) -> Result<Envelope> {
    let mut board = board.clone();
    board.rev = rev;
    let header = Header::new(Id::new(Prefix::Board).map_err(|error| error.to_string())?, Kind::Board, identity.machine.clone(), identity.machine.clone(), now);
    sealed(identity, &Payload::Board(board), header, true)
}

fn shortened(notice: &Notice, keep: usize) -> Notice {
    Notice {
        title: clip(&notice.title, keep.min(TITLE_CHARS)),
        body: clip(&notice.body, keep.min(BODY_CHARS)),
        question: notice.question.as_deref().map(|question| clip(question, keep)),
        options: notice.options.iter().map(|option| clip(option, keep.min(OPTION_CHARS))).collect(),
        ..notice.clone()
    }
}

fn fits(identity: &Identity, envelope: &Envelope) -> bool {
    identity.devices.iter().all(|device| envelope.for_recipient(&device.id).is_some_and(|cut| cut.to_json().len() <= PUSH_ENVELOPE_BYTES))
}

// Text is measured where it ends up, each device's cut of the sealed envelope, so a multibyte question shortens until it fits one push.
pub fn seal_notice(identity: &Identity, pending: &Pending, board_rev: u64, now: u64) -> Result<Envelope> {
    let mut keep = LONGEST;
    while keep >= 8 {
        let mut notice = shortened(&pending.notice, keep);
        notice.board_rev = board_rev;
        let mut header = Header::new(Id::new(Prefix::Notice).map_err(|error| error.to_string())?, Kind::Notice, identity.machine.clone(), identity.machine.clone(), now);
        header.urgency = Some(pending.urgency);
        header.collapse = notice.address.as_deref().map(|address| identity.collapse_tag(address));
        let envelope = sealed(identity, &Payload::Notice(notice), header, false)?;
        if fits(identity, &envelope) {
            return Ok(envelope);
        }
        keep = keep * 3 / 4;
    }
    Err(format!("the notice about {} does not fit one push", pending.notice.address.as_deref().unwrap_or("kelpie")))
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

// The heartbeat reposts an unchanged board, since the phone counts a machine silent for three minutes as asleep; a revision of at least the time keeps it rising when relay.json is lost, as the phone refuses one that goes back.
pub fn relay(state: &Path, board: &Board, now: u64) -> Result<Outcome> {
    let Some(identity) = identity::load(state)? else { return Ok(Outcome::default()) };
    if identity.devices.is_empty() {
        return Ok(Outcome::default());
    }
    let mut relay = load(state);
    let client = Client::new(&identity.url, Some(identity.token()), CALL);
    let mut outcome = Outcome::default();
    let print = fingerprint(board, &identity);
    let heartbeat_due = relay.posted_at.is_none_or(|at| now.saturating_sub(at) >= HEARTBEAT);
    if relay.fingerprint.as_deref() != Some(print.as_str()) || heartbeat_due {
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
        outcome.posted = Some(stored.rev);
    }
    let current = notices(board);
    for pending in &current {
        if relay.notices.contains_key(&pending.key) {
            continue;
        }
        if relay.primed {
            let envelope = match seal_notice(&identity, pending, relay.rev, now) {
                Ok(envelope) => envelope,
                Err(error) => {
                    relay.error = Some(error);
                    relay.notices.insert(pending.key.clone(), now);
                    continue;
                }
            };
            match client.post::<_, NoticePosted>("/v1/m/notices", &NoticeUpload { envelope }) {
                Ok(_) => outcome.notices.push(pending.key.clone()),
                Err(error) if error.is_final() => relay.error = Some(format!("notice {}: {error}", pending.notice.title)),
                Err(error) => {
                    keep_move(state, &client);
                    return Err(failed(state, &mut relay, &error, now));
                }
            }
        }
        relay.notices.insert(pending.key.clone(), now);
    }
    relay.notices.retain(|key, at| current.iter().any(|pending| &pending.key == key) || now.saturating_sub(*at) < FORGET_AFTER);
    relay.primed = true;
    save(state, &relay)?;
    keep_move(state, &client);
    Ok(outcome)
}
