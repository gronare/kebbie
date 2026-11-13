use std::collections::BTreeMap;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use kebbie_hub_api::Bytes32;
use kebbie_hub_protocol::{DeviceKind, DeviceStanding, Id, Kind, Payload, Trust, VerifiedDevice, device_fingerprint};
use serde::{Deserialize, Serialize};

use super::identity;
use crate::config::{AutoApprove, HubTrust};
use crate::notice::{Notice, Tone};
use crate::sh::Result;

pub(super) const FILE: &str = "devices.json";
const LOCK: &str = "devices.lock";
pub const DAY: u64 = 86_400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Waiting,
    Approved,
    Denied,
}

// The key is kept whole: a device whose key changed is another request, and the fingerprint is only what a person compares.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub signing: Bytes32,
    pub name: String,
    pub kind: DeviceKind,
    pub first_seen: u64,
    pub seen: u64,
    pub state: State,
    pub at: u64,
}

// A device id that arrives with another key never takes over the record its first key made: each other key is a request of its own, decided apart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Devices {
    #[serde(default)]
    pub devices: BTreeMap<Id, Device>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rekeyed: Vec<Rekeyed>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rekeyed {
    pub id: Id,
    #[serde(flatten)]
    pub device: Device,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Policy {
    pub trust: HubTrust,
    pub auto: AutoApprove,
}

impl Policy {
    pub fn current() -> Policy {
        let (trust, auto) = crate::config::hub_trust();
        Policy { trust, auto }
    }
}

pub fn kind_name(kind: DeviceKind) -> String {
    format!("{} app", kind.name())
}

impl Device {
    pub fn fingerprint(&self) -> String {
        device_fingerprint(&self.signing.0)
    }

    pub fn until(&self, policy: Policy) -> Option<u64> {
        (policy.trust == HubTrust::Device && self.state == State::Waiting && policy.auto == AutoApprove::Day).then_some(self.first_seen + DAY)
    }

    // Where it stands under the policy now, and since when: hub.trust = "account" lets every device in without touching what was decided.
    pub fn standing(&self, policy: Policy, now: u64) -> (State, u64) {
        match (policy.trust, self.state) {
            (HubTrust::Account, State::Approved) => (State::Approved, self.at),
            (HubTrust::Account, _) => (State::Approved, self.first_seen),
            (HubTrust::Device, State::Waiting) => match self.until(policy) {
                Some(until) if now >= until => (State::Approved, until),
                _ => (State::Waiting, self.first_seen),
            },
            (HubTrust::Device, state) => (state, self.at),
        }
    }
}

impl Devices {
    pub fn records(&self) -> impl Iterator<Item = (&Id, &Device)> {
        self.devices.iter().chain(self.rekeyed.iter().map(|rekeyed| (&rekeyed.id, &rekeyed.device)))
    }

    fn records_mut(&mut self) -> impl Iterator<Item = (&Id, &mut Device)> {
        self.devices.iter_mut().chain(self.rekeyed.iter_mut().map(|rekeyed| (&rekeyed.id, &mut rekeyed.device)))
    }

    pub fn record(&self, id: &Id, signing: &[u8; 32]) -> Option<&Device> {
        self.records().find(|(known, device)| *known == id && device.signing.0 == *signing).map(|(_, device)| device)
    }

    fn record_mut(&mut self, id: &Id, signing: &[u8; 32]) -> Option<&mut Device> {
        self.records_mut().find(|(known, device)| *known == id && device.signing.0 == *signing).map(|(_, device)| device)
    }

    pub fn trust(&self, policy: Policy, device: &VerifiedDevice, now: u64) -> Trust {
        self.trust_of(policy, device.id(), device.signing(), now)
    }

    pub fn trust_of(&self, policy: Policy, id: &Id, signing: &[u8; 32], now: u64) -> Trust {
        if policy.trust == HubTrust::Account {
            return Trust::Approved;
        }
        match self.record(id, signing) {
            None => Trust::Waiting,
            Some(known) => match known.standing(policy, now).0 {
                State::Approved => Trust::Approved,
                State::Waiting => Trust::Waiting,
                State::Denied => Trust::Denied,
            },
        }
    }

    // Records a device whose command was verified, seen as of when it sent it so a held command authorized again changes nothing; true when it is a new request to approve.
    pub fn seen(&mut self, policy: Policy, device: &VerifiedDevice, sent: u64, now: u64) -> bool {
        let cert = device.cert();
        if let Some(known) = self.record_mut(device.id(), device.signing()) {
            (known.seen, known.name, known.kind) = (sent.min(now).max(known.seen), cert.name.clone(), cert.kind);
            return false;
        }
        let state = if policy.trust == HubTrust::Account { State::Approved } else { State::Waiting };
        let record = Device { signing: Bytes32(*device.signing()), name: cert.name.clone(), kind: cert.kind, first_seen: now, seen: now, state, at: now };
        if self.devices.contains_key(device.id()) {
            self.rekeyed.push(Rekeyed { id: device.id().clone(), device: record });
        } else {
            self.devices.insert(device.id().clone(), record);
        }
        state == State::Waiting
    }

    // The waiting devices whose day is up are approved; their ids come back so their notices settle as a manual approval's do.
    pub fn settle(&mut self, policy: Policy, now: u64) -> Vec<Id> {
        let mut approved = Vec::new();
        for (id, device) in self.records_mut() {
            if device.state == State::Waiting && policy.trust == HubTrust::Device && device.until(policy).is_some_and(|until| now >= until) {
                (device.state, device.at) = (State::Approved, device.first_seen + DAY);
                approved.push(id.clone());
            }
        }
        approved
    }

    // What a decision changes, so the keeper's watch releases held commands on an approval or a denial and on nothing else.
    pub fn decisions(&self) -> Vec<(Id, String, State, u64)> {
        self.records().map(|(id, device)| (id.clone(), device.fingerprint(), device.state, device.at)).collect()
    }

    fn due(&self, policy: Policy, now: u64) -> bool {
        policy.trust == HubTrust::Device && self.records().any(|(_, device)| device.state == State::Waiting && device.until(policy).is_some_and(|until| now >= until))
    }

    // The phone's board carries one standing per id, its first key's; another key under that id is decided at the machine.
    pub fn standings(&self, policy: Policy, now: u64) -> BTreeMap<Id, DeviceStanding> {
        self.devices.iter().map(|(id, device)| {
            let fingerprint = device.fingerprint();
            let standing = match device.standing(policy, now) {
                (State::Waiting, _) => DeviceStanding::Waiting { fingerprint, until: device.until(policy) },
                (State::Approved, at) => DeviceStanding::Approved { fingerprint, at },
                (State::Denied, at) => DeviceStanding::Denied { fingerprint, at },
            };
            (id.clone(), standing)
        }).collect()
    }

    // A device named by "<id> <key>", its key, its id or its name; any that names two records is refused with what tells them apart.
    pub fn find(&self, who: &str) -> Result<(Id, [u8; 32])> {
        let who = who.trim();
        let all: Vec<(&Id, &Device)> = self.records().collect();
        let matching = |test: &dyn Fn(&Id, &Device) -> bool| all.iter().filter(|(id, device)| test(id, device)).copied().collect::<Vec<_>>();
        let mut found = matching(&|id, device| format!("{} {}", id.as_str(), device.fingerprint()) == who);
        for test in [&(|_: &Id, device: &Device| device.fingerprint() == who) as &dyn Fn(&Id, &Device) -> bool, &|id, _| id.as_str() == who, &|_, device| device.name == who, &|_, device| device.name.eq_ignore_ascii_case(who)] {
            if found.is_empty() {
                found = matching(test);
            }
        }
        match found.as_slice() {
            [(id, device)] => Ok(((*id).clone(), device.signing.0)),
            [] => Err(format!("No device {who:?} has sent this machine a command; kebbie hub devices lists them")),
            many => Err(format!("{} devices answer to {who:?}; name one by its id and key, compared with the key under Account on the device: {}", many.len(), many.iter().map(|(id, device)| format!("{} ({} {})", device.name, id.as_str(), device.fingerprint())).collect::<Vec<_>>().join(", "))),
        }
    }
}

pub(super) struct Locked(std::fs::File);

impl Locked {
    pub(super) fn wait(dir: &Path, name: &str) -> Result<Locked> {
        let file = crate::state::append_to(&dir.join(name)).map_err(|error| format!("{}: {error}", dir.join(name).display()))?;
        // SAFETY: flock on a descriptor this File owns.
        match unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } {
            0 => Ok(Locked(file)),
            _ => Err(format!("{}: {}", dir.join(name).display(), std::io::Error::last_os_error())),
        }
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

pub fn load(dir: &Path) -> Devices {
    std::fs::read_to_string(dir.join(FILE)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub(super) fn save(dir: &Path, devices: &Devices) -> Result<()> {
    crate::state::write(&dir.join(FILE), serde_json::to_string_pretty(devices).map_err(|error| error.to_string())?)
}

// Every change to the file goes through here, so a decision made on the board or in a terminal is never written over by the bridge; an unchanged file is not rewritten, as the keeper's watch fires on each write.
pub fn change<T>(dir: &Path, change: impl FnOnce(&mut Devices) -> Result<T>) -> Result<T> {
    let _locked = Locked::wait(dir, LOCK)?;
    let before = load(dir);
    let mut devices = before.clone();
    let done = change(&mut devices)?;
    if devices != before {
        save(dir, &devices)?;
    }
    Ok(done)
}

// What a held command asks for, kept with it so the board can say it without opening the command again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asked {
    pub kind: Kind,
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
}

impl Asked {
    pub fn of(payload: &Payload, label: impl Fn(&str, usize) -> Option<String>) -> Asked {
        let task = |address: &str| address.split_once('@').map_or(address, |(task, _)| task).to_string();
        match payload {
            Payload::Answer(answer) => {
                let said = answer.choice.and_then(|choice| label(&answer.question, usize::from(choice))).or_else(|| Some(answer.comment.trim().to_string()).filter(|comment| !comment.is_empty()));
                Asked { kind: Kind::Answer, task: task(&answer.address), said }
            }
            Payload::Push(push) => Asked { kind: Kind::Push, task: task(&push.address), said: None },
            Payload::PromptAnswer(prompt) => Asked { kind: Kind::PromptAnswer, task: task(&prompt.address), said: None },
            Payload::Idea(idea) => Asked { kind: Kind::Idea, task: String::new(), said: Some(short(&idea.text)) },
            Payload::ItemAnswer(answer) => Asked { kind: Kind::ItemAnswer, task: item(&answer.item), said: Some(short(&answer.answer)) },
            Payload::StartItem(start) => Asked { kind: Kind::StartItem, task: item(&start.item), said: None },
            Payload::ItemEdit(edit) => Asked { kind: Kind::ItemEdit, task: item(&edit.item), said: None },
            Payload::ReviewOpen(open) => Asked { kind: Kind::ReviewOpen, task: task(&open.address), said: None },
            Payload::ReviewComments(comments) => Asked { kind: Kind::ReviewComments, task: task(&comments.address), said: None },
            Payload::ReviewRead(read) => Asked { kind: Kind::ReviewRead, task: task(&read.address), said: Some(read.path.clone()) },
            Payload::AcceptStep(accept) => Asked { kind: Kind::AcceptStep, task: task(&accept.address), said: None },
            Payload::AcceptSubtask(accept) => Asked { kind: Kind::AcceptSubtask, task: task(&accept.address), said: None },
            Payload::FoundStart(start) => Asked { kind: Kind::FoundStart, task: item(&start.item), said: None },
            Payload::FoundAdd(add) => Asked { kind: Kind::FoundAdd, task: item(&add.item), said: Some(task(&add.task)) },
            Payload::FoundDismiss(dismiss) => Asked { kind: Kind::FoundDismiss, task: item(&dismiss.item), said: None },
            other => Asked { kind: other.kind(), task: String::new(), said: None },
        }
    }

    fn noun(&self) -> String {
        match self.kind {
            Kind::Answer => format!("answer to {}", self.task),
            Kind::Push => format!("push of {}", self.task),
            Kind::PromptAnswer => format!("choice on {}'s prompt", self.task),
            Kind::Idea => "idea for the backlog".to_string(),
            Kind::ItemAnswer => format!("answer on the backlog item {}", self.task),
            Kind::StartItem => format!("start of the backlog item {}", self.task),
            Kind::ItemEdit => format!("edit of the backlog item {}", self.task),
            Kind::ReviewOpen => format!("look at {}'s review", self.task),
            Kind::ReviewComments => format!("set of review comments for {}", self.task),
            Kind::ReviewRead => format!("read mark in {}'s review", self.task),
            Kind::AcceptStep | Kind::AcceptSubtask => format!("acceptance of {}'s subtask", self.task),
            Kind::FoundStart => format!("start of {}, found by tasks", self.task),
            Kind::FoundAdd => format!("addition of {}, found by tasks, to a task", self.task),
            Kind::FoundDismiss => format!("dismissal of {}, found by tasks", self.task),
            other => other.name().to_string(),
        }
    }

    pub fn described(&self) -> String {
        let article = if matches!(self.kind, Kind::Answer | Kind::ItemAnswer | Kind::Idea) { "an" } else { "a" };
        match &self.said {
            Some(said) => format!("{article} {}, {said:?}", self.noun()),
            None => format!("{article} {}", self.noun()),
        }
    }

    pub fn its(&self) -> String {
        format!("its {}", self.noun())
    }
}

fn short(text: &str) -> String {
    let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.chars().count() <= 60 { words } else { format!("{}…", words.chars().take(59).collect::<String>()) }
}

fn item(key: &kebbie_hub_protocol::ItemKey) -> String {
    key.as_str().split_once(':').map_or(key.as_str(), |(_, rest)| rest).to_string()
}

pub fn question_label(question: &str, choice: usize) -> Option<String> {
    crate::messages::load().into_iter().find(|message| message.id == question)?.question?.options.get(choice).map(|(label, _)| label.clone())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    pub asked: Asked,
    pub at: u64,
    pub expires: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub dir: PathBuf,
    pub machine: String,
    pub id: Id,
    pub name: String,
    pub kind: DeviceKind,
    pub fingerprint: String,
    pub first_seen: u64,
    pub until: Option<u64>,
    pub sent: Vec<Sent>,
}

impl Request {
    // The board names a request by its id and key together, as two can share an id.
    pub fn key(&self) -> String {
        format!("{} {}", self.id.as_str(), self.fingerprint)
    }
}

pub fn requests_in(dir: &Path, policy: Policy, now: u64) -> Vec<Request> {
    let Some(identity) = identity::load_at(dir).ok().flatten() else { return Vec::new() };
    let held = super::commands::ledger(dir).held;
    let devices = load(dir);
    devices.records().filter(|(_, device)| device.standing(policy, now).0 == State::Waiting).map(|(id, device)| {
        let sent = held.values().filter(|held| held.device == *id).map(|held| Sent { asked: held.asked.clone(), at: held.envelope.header.at, expires: held.envelope.header.exp }).collect();
        Request { dir: dir.to_path_buf(), machine: identity.name.clone(), until: device.until(policy), fingerprint: device.fingerprint(), name: device.name.clone(), kind: device.kind, first_seen: device.first_seen, id: id.clone(), sent }
    }).collect()
}

pub fn requests(state: &Path, now: u64) -> Vec<Request> {
    let policy = Policy::current();
    [false, true].into_iter().flat_map(|dev| requests_in(&identity::dir_of(state, dev), policy, now)).collect()
}

pub fn clock(at: u64, offset: &dyn Fn(i64) -> i64) -> String {
    let local = at as i64 + offset(at as i64);
    let seconds = local.rem_euclid(DAY as i64);
    format!("{:02}:{:02}", seconds / 3600, seconds % 3600 / 60)
}

fn day_of(at: u64, offset: &dyn Fn(i64) -> i64) -> i64 {
    (at as i64 + offset(at as i64)).div_euclid(DAY as i64)
}

fn date(at: u64, offset: &dyn Fn(i64) -> i64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (_, month, day) = crate::when::civil(day_of(at, offset));
    format!("{day} {}", MONTHS[month as usize - 1])
}

pub fn added(at: u64, now: u64, offset: &dyn Fn(i64) -> i64) -> String {
    if day_of(at, offset) == day_of(now, offset) { format!("today {}", clock(at, offset)) } else { date(at, offset) }
}

fn seen_at(at: u64, now: u64, offset: &dyn Fn(i64) -> i64) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    match day_of(now, offset) - day_of(at, offset) {
        0 => clock(at, offset),
        1..=6 => format!("{} {}", WEEKDAYS[crate::recover::weekday(day_of(at, offset)) as usize], clock(at, offset)),
        _ => date(at, offset),
    }
}

pub fn auto_in(until: u64, now: u64) -> String {
    match until.saturating_sub(now) {
        0..60 => "approved automatically within a minute".to_string(),
        left => format!("approved automatically in {}", crate::next::waited(left)),
    }
}

pub fn trust_line(policy: Policy) -> [String; 2] {
    let trust = match policy.trust {
        HubTrust::Device => "each device you approve on this machine",
        HubTrust::Account => "any device signed in to your account, with nothing to approve",
    };
    let new = match (policy.trust, policy.auto) {
        (HubTrust::Account, _) => format!("takes its commands at once; hub.auto_approve = {:?} applies under hub.trust = \"device\"", policy.auto.key()),
        (HubTrust::Device, AutoApprove::Day) => "approved automatically 24 h after its first command (hub.auto_approve = \"24h\")".to_string(),
        (HubTrust::Device, AutoApprove::Never) => "waits until you approve it (hub.auto_approve = \"never\")".to_string(),
    };
    [format!("trust:    {trust} (hub.trust = {:?})", policy.trust.key()), format!("new:      {new}")]
}

pub fn list(devices: &Devices, policy: Policy, now: u64, offset: &dyn Fn(i64) -> i64) -> String {
    let mut lines = trust_line(policy).to_vec();
    if devices.records().next().is_none() {
        lines.push("  No device has sent this machine a command yet.".to_string());
    }
    let mut shown: Vec<&Device> = devices.records().map(|(_, device)| device).collect();
    shown.sort_by_key(|device| (device.standing(policy, now).0 != State::Waiting, std::cmp::Reverse(device.seen)));
    let rows: Vec<[String; 6]> = shown.iter().map(|device| {
        let (state, at) = device.standing(policy, now);
        let (mark, standing) = match state {
            State::Waiting => ('⧗', match device.until(policy) {
                Some(until) => format!("waiting · {}", auto_in(until, now)),
                None => "waiting · approve it to let it make changes".to_string(),
            }),
            State::Approved => ('✓', format!("approved {}", added(at, now, offset))),
            State::Denied => ('✗', format!("denied {}", added(at, now, offset))),
        };
        [mark.to_string(), device.name.clone(), kind_name(device.kind), format!("added {}", added(device.first_seen, now, offset)), format!("seen {}", seen_at(device.seen, now, offset)), standing]
    }).collect();
    let width = |column: usize| rows.iter().map(|row| crate::board::widgets::text_width(&row[column])).max().unwrap_or(0);
    let widths = [width(1), width(2), width(3), width(4)];
    let pad = |text: &str, width: usize| format!("{text}{}", " ".repeat(width.saturating_sub(crate::board::widgets::text_width(text))));
    lines.extend(rows.iter().map(|row| format!("  {} {}  {}  {}  {}  {}", row[0], pad(&row[1], widths[0]), pad(&row[2], widths[1]), pad(&row[3], widths[2]), pad(&row[4], widths[3]), row[5])));
    if policy.trust == HubTrust::Device {
        lines.push("kebbie hub devices approve <name> lets one in, deny <name> keeps it out; N on the board asks too.".to_string());
    }
    lines.join("\n")
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Decision {
    Approve,
    Deny,
}

pub fn decide(dir: &Path, who: &str, decision: Decision, now: u64) -> Result<(Id, Device)> {
    let decided = change(dir, |devices| {
        let (id, signing) = devices.find(who)?;
        let device = devices.record_mut(&id, &signing).expect("find returns a known record");
        (device.state, device.at) = (if decision == Decision::Approve { State::Approved } else { State::Denied }, now);
        Ok((id, device.clone()))
    })?;
    let _ = super::notices::settle_in(dir, super::notices::Subject::Device, decided.0.as_str());
    Ok(decided)
}

pub fn settled(dir: &Path, approved: &[Id]) {
    for id in approved {
        let _ = super::notices::settle_in(dir, super::notices::Subject::Device, id.as_str());
    }
}

// The keeper approves a device whose day is up without waiting for its next command, so its notice goes from the other devices on time; the file is written only then.
pub fn settle_due(dir: &Path, policy: Policy, now: u64) -> Result<()> {
    if !load(dir).due(policy, now) {
        return Ok(());
    }
    let approved = change(dir, |devices| Ok(devices.settle(policy, now)))?;
    settled(dir, &approved);
    Ok(())
}

// A decision sent from another device names the waiting device by id and the fingerprint its user compared; anything else matches nothing.
pub fn decide_waiting(dir: &Path, id: &Id, fingerprint: &str, decision: Decision, policy: Policy, now: u64) -> Result<Option<Device>> {
    let decided = change(dir, |devices| {
        let Some((_, device)) = devices.records_mut().find(|(known, device)| *known == id && device.fingerprint() == fingerprint && device.standing(policy, now).0 == State::Waiting) else { return Ok(None) };
        (device.state, device.at) = (if decision == Decision::Approve { State::Approved } else { State::Denied }, now);
        Ok(Some(device.clone()))
    })?;
    if decided.is_some() {
        let _ = super::notices::settle_in(dir, super::notices::Subject::Device, id.as_str());
    }
    Ok(decided)
}

pub fn decided(device: &Device, machine: &str, decision: Decision, held: &[Asked], policy: Policy, keeper: bool) -> String {
    let capital = |text: String| text[..1].to_uppercase() + &text[1..];
    let (what, plural) = match held {
        [] => (None, false),
        [one] => (Some(capital(one.its())), false),
        many => (Some(format!("Its {} commands", many.len())), true),
    };
    let notice = match decision {
        Decision::Approve => {
            let goes = if plural { "go in" } else { "goes in" };
            let next = match (&what, keeper) {
                (None, _) => "Its next command goes in at once.".to_string(),
                (Some(what), true) => format!("{what} {goes} now."),
                (Some(what), false) => format!("{what} {goes} once the keeper runs: kebbie keeper --start."),
            };
            Notice::new(Tone::Done, format!("{} makes changes on {machine} from now on", device.name)).next(next)
        }
        Decision::Deny => {
            let dropped = what.map_or(String::new(), |what| format!("{what} {} dropped; ", if plural { "are" } else { "is" }));
            Notice::new(Tone::Done, format!("{} stays view only on {machine}", device.name)).next(format!("{dropped}kebbie hub devices approve {:?} lets it in later.", device.name))
        }
    };
    let notice = if policy.trust == HubTrust::Account { notice.detail("trust", "hub.trust = \"account\" takes every device on your account, so this counts once it is \"device\"") } else { notice };
    notice.detail("key", device.fingerprint()).to_string()
}

// Approve or deny one device in an identity directory, answered as a notice; the keeper's bridge sees the file change and releases what it held.
pub fn answer(dir: &Path, who: &str, decision: Decision, now: u64) -> Result<String> {
    let identity = identity::load_at(dir)?.ok_or("This machine is not connected to a hub; kebbie hub connect first")?;
    let (id, device) = decide(dir, who, decision, now)?;
    let held: Vec<Asked> = super::commands::ledger(dir).held.into_values().filter(|held| held.device == id).map(|held| held.asked).collect();
    let keeper = dir.parent().is_some_and(|state| crate::keeper::holder(state).is_some());
    Ok(decided(&device, &identity.name, decision, &held, Policy::current(), keeper))
}

pub fn cli(state: &Path, rest: &[String], now: u64) -> Result<String> {
    let dir = identity::dir(state);
    let decision = match rest.first().map(String::as_str) {
        None => {
            identity::load_at(&dir)?.ok_or("This machine is not connected to a hub; kebbie hub connect first")?;
            return Ok(list(&load(&dir), Policy::current(), now, &crate::recover::local_offset));
        }
        Some("approve") => Decision::Approve,
        Some("deny") => Decision::Deny,
        Some(other) => return Err(format!("kebbie hub devices knows approve and deny, not {other}")),
    };
    crate::permit::user_only(if decision == Decision::Approve { "approving a device" } else { "denying a device" })?;
    let who = rest[1..].join(" ");
    if who.trim().is_empty() {
        return Err(format!("kebbie hub devices {} needs the device's name or id: kebbie hub devices {0} \"iPhone 16\"", rest[0]));
    }
    answer(&dir, &who, decision, now)
}

#[cfg(test)]
mod tests {
    use kebbie_hub_protocol::Prefix;

    use super::*;

    const DEVICE: Policy = Policy { trust: HubTrust::Device, auto: AutoApprove::Day };

    fn at(month: u32, day: u32, hour: u64, minute: u64) -> u64 {
        crate::recover::days_from_civil(2026, month, day) as u64 * DAY + hour * 3600 + minute * 60
    }

    fn device(name: &str, first_seen: u64, seen: u64, state: State, decided: u64) -> (Id, Device) {
        (Id::new(Prefix::Device).unwrap(), Device { signing: Bytes32([name.len() as u8; 32]), name: name.to_string(), kind: DeviceKind::Web, first_seen, seen, state, at: decided })
    }

    fn three() -> Devices {
        Devices { devices: [
            device("iPhone 16", at(11, 11, 13, 58), at(11, 11, 14, 9), State::Waiting, at(11, 11, 13, 58)),
            device("iPad", at(10, 2, 10, 0), at(11, 10, 9, 14), State::Approved, at(10, 2, 10, 5)),
            device("Chrome on Mac", at(9, 28, 8, 0), at(9, 30, 8, 0), State::Denied, at(9, 28, 8, 1)),
        ].into_iter().collect(), ..Devices::default() }
    }

    #[test]
    fn a_config_that_does_not_parse_lets_no_device_in_by_time_or_by_account() {
        let dir = crate::sh::test_dir("hub-gates-broken");
        let file = dir.join("config.toml");
        let waiting = || Devices { devices: [device("iPhone 16", at(11, 1, 9, 0), at(11, 1, 9, 0), State::Waiting, at(11, 1, 9, 0))].into_iter().collect(), ..Devices::default() };
        let policy_in = |text: &str| {
            std::fs::write(&file, text).unwrap();
            let (gates, error) = crate::config::hub_gates_in(&file);
            (Policy { trust: gates.trust, auto: gates.auto }, gates, error)
        };
        let (policy, gates, error) = policy_in("[hub]
trust = \"account\"
auto_approve = \"24h\"
[restart]
idle_minutes = \"soon\"
");
        assert!(error.is_some_and(|error| error.contains("idle_minutes")), "the error says why the gates are closed");
        assert_eq!(gates, crate::config::CLOSED_GATES);
        assert!(super::super::commands::accepted(gates.commands, gates.from).is_empty(), "no command kind and no device decision from a phone");
        assert!(waiting().settle(policy, at(11, 3, 9, 0)).is_empty(), "a device two days in waits on");
        let (id, device) = waiting().devices.into_iter().next().unwrap();
        assert_eq!(Devices { devices: [(id.clone(), device.clone())].into_iter().collect(), ..Devices::default() }.trust_of(policy, &id, &device.signing.0, at(11, 3, 9, 0)), Trust::Waiting, "trust = \"account\" in a broken file lets nothing in");
        let (policy, _, error) = policy_in("[hub]
auto_approve = \"24h\"
");
        assert!(error.is_none());
        assert_eq!(waiting().settle(policy, at(11, 3, 9, 0)).len(), 1, "the same device is approved once the file parses");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn kebbie_hub_devices_lists_the_trust_then_each_device_waiting_first_with_its_standing() {
        let shown = list(&three(), DEVICE, at(11, 11, 14, 10), &|_| 0);
        println!("{shown}");
        assert_eq!(shown, [
            "trust:    each device you approve on this machine (hub.trust = \"device\")",
            "new:      approved automatically 24 h after its first command (hub.auto_approve = \"24h\")",
            "  ⧗ iPhone 16      web app  added today 13:58  seen 14:09      waiting · approved automatically in 23 h",
            "  ✓ iPad           web app  added 2 Oct        seen Tue 09:14  approved 2 Oct",
            "  ✗ Chrome on Mac  web app  added 28 Sep       seen 30 Sep     denied 28 Sep",
            "kebbie hub devices approve <name> lets one in, deny <name> keeps it out; N on the board asks too.",
        ].join("\n"));
        let never = list(&three(), Policy { trust: HubTrust::Device, auto: AutoApprove::Never }, at(11, 11, 14, 10), &|_| 0);
        assert!(never.contains("new:      waits until you approve it (hub.auto_approve = \"never\")") && never.contains("waiting · approve it to let it make changes"), "{never}");
        let account = list(&three(), Policy { trust: HubTrust::Account, auto: AutoApprove::Day }, at(11, 11, 14, 10), &|_| 0);
        assert!(account.starts_with("trust:    any device signed in to your account, with nothing to approve (hub.trust = \"account\")") && !account.contains('⧗') && !account.contains("lets one in"), "{account}");
        assert!(list(&Devices::default(), DEVICE, 0, &|_| 0).contains("  No device has sent this machine a command yet."));
    }

    #[test]
    fn the_board_carries_each_devices_standing_with_its_fingerprint_and_when_it_is_let_in() {
        let devices = three();
        let standings = devices.standings(DEVICE, at(11, 11, 14, 10));
        let by_name = |name: &str| standings[devices.devices.iter().find(|(_, device)| device.name == name).unwrap().0].clone();
        assert_eq!(by_name("iPhone 16"), DeviceStanding::Waiting { fingerprint: device_fingerprint(&[9; 32]), until: Some(at(11, 12, 13, 58)) });
        assert_eq!(by_name("iPad"), DeviceStanding::Approved { fingerprint: device_fingerprint(&[4; 32]), at: at(10, 2, 10, 5) });
        assert_eq!(by_name("Chrome on Mac"), DeviceStanding::Denied { fingerprint: device_fingerprint(&[13; 32]), at: at(9, 28, 8, 1) });
        let later = devices.standings(DEVICE, at(11, 12, 13, 58));
        assert!(later.values().any(|standing| matches!(standing, DeviceStanding::Approved { at: when, .. } if *when == at(11, 12, 13, 58))), "a day after its first command it stands approved: {later:?}");
        let never = devices.standings(Policy { trust: HubTrust::Device, auto: AutoApprove::Never }, at(12, 1, 0, 0));
        assert!(never.values().any(|standing| matches!(standing, DeviceStanding::Waiting { until: None, .. })), "{never:?}");
    }

    #[test]
    fn approving_says_what_the_held_commands_do_now_and_the_key_to_compare() {
        let (_, phone) = device("iPhone 16", 0, 0, State::Approved, 0);
        let answer = Asked { kind: Kind::Answer, task: "billing".to_string(), said: Some("Per invoice".to_string()) };
        assert_eq!(decided(&phone, "MacBook", Decision::Approve, std::slice::from_ref(&answer), DEVICE, true), format!("✓ iPhone 16 makes changes on MacBook from now on\n  next: Its answer to billing goes in now.\n  key: {}", phone.fingerprint()));
        assert!(decided(&phone, "MacBook", Decision::Approve, &[], DEVICE, true).contains("next: Its next command goes in at once."));
        assert!(decided(&phone, "MacBook", Decision::Approve, std::slice::from_ref(&answer), DEVICE, false).contains("next: Its answer to billing goes in once the keeper runs: kebbie keeper --start."));
        assert!(decided(&phone, "MacBook", Decision::Deny, &[answer.clone(), answer.clone()], DEVICE, true).starts_with("✓ iPhone 16 stays view only on MacBook\n  next: Its 2 commands are dropped; kebbie hub devices approve \"iPhone 16\" lets it in later."));
        assert_eq!(answer.described(), "an answer to billing, \"Per invoice\"");
        assert_eq!(Asked { kind: Kind::Push, task: "cache".to_string(), said: None }.described(), "a push of cache");
    }
}
