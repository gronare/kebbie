use std::collections::BTreeMap;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kebbie_hub_api::{HubInfo, Inbox, ResultStored, ResultUpload};
use kebbie_hub_protocol::{AccountKey, Answer, AppliedCommands, CommandResult, DeviceDecision, Dropped, Envelope, Header, Id, Idea, ItemAnswer, Kind, Machine, Outcome, Payload, PromptAnswer, PushApproval, Reason, Refusal, StartItem, Trust, VerifiedDevice, authorize, seal};
use serde::{Deserialize, Serialize};

use super::devices::{self, Asked, Decision, Locked, Policy};
use super::http::{CALL, Client, HubError, LONG_POLL};
use super::identity::{self, Identity};
use crate::config::{ApproveFrom, HubCommand, HubCommands};
use crate::notice::Notice;
use crate::permit::Caller;
use crate::sh::Result;

pub const WAIT: u64 = 25;
const LEDGER: &str = "applied.json";
const BRIDGE: &str = "commands.json";
const LOCK: &str = "commands.lock";
const LEDGER_LOCK: &str = "applied.lock";
const IDLE: u64 = 60;
const MAX_BACKOFF: u64 = 300;
pub const STOPPED: &str = "kebbie stopped while applying it; check the board";

// A command marked started and never recorded was cut off mid-apply; it is answered Failed when it comes again, never applied twice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub from: Id,
    pub at: u64,
}

// A command from a device that waits for approval, kept whole so it is authorized again once the device is decided.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Held {
    pub envelope: Envelope,
    pub device: Id,
    pub asked: Asked,
}

// A result recorded and not yet taken by the hub; it leaves only once the hub has it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Unsent {
    pub device: Id,
    pub command: Id,
}

// Held commands are keyed by the device and the command id together, as two devices can pick the same id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub applied: AppliedCommands,
    #[serde(default)]
    pub started: BTreeMap<Id, Started>,
    #[serde(default)]
    pub held: BTreeMap<String, Held>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub unsent: std::collections::BTreeSet<Unsent>,
}

pub fn held_key(device: &Id, command: &Id) -> String {
    format!("{} {}", device.as_str(), command.as_str())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bridge {
    #[serde(default)]
    pub inbox_seq: u64,
}

pub type Done = (Outcome, String);

// What applying a command does, given as functions so tests drive the bridge without touching the user's tasks.
pub struct Hands<'a> {
    pub answer: &'a dyn Fn(&Caller, &Answer) -> Done,
    pub push: &'a dyn Fn(&Caller, &PushApproval) -> Done,
    pub prompt: &'a dyn Fn(&Caller, &PromptAnswer) -> Done,
    pub idea: &'a dyn Fn(&Caller, &Idea) -> Done,
    pub item_answer: &'a dyn Fn(&Caller, &ItemAnswer) -> Done,
    pub start_item: &'a dyn Fn(&Caller, &StartItem) -> Done,
    pub relay: &'a dyn Fn(),
    pub notify: &'a dyn Fn(&str),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Handled {
    Applied(CommandResult),
    Refused(CommandResult),
    Replayed(CommandResult),
    Held(Id),
    Dropped(Dropped),
}

impl Handled {
    pub fn result(&self) -> Option<&CommandResult> {
        match self {
            Handled::Applied(result) | Handled::Refused(result) | Handled::Replayed(result) => Some(result),
            Handled::Held(_) | Handled::Dropped(_) => None,
        }
    }
}

fn load<T: Default + for<'de> Deserialize<'de>>(file: &Path) -> T {
    std::fs::read_to_string(file).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save<T: Serialize>(file: &Path, value: &T) -> Result<()> {
    crate::state::write(file, serde_json::to_string_pretty(value).map_err(|error| error.to_string())?)
}

pub fn ledger(dir: &Path) -> Ledger {
    let mut ledger: Ledger = load(&dir.join(LEDGER));
    ledger.held = std::mem::take(&mut ledger.held).into_values().map(|held| (held_key(&held.device, &held.envelope.header.id), held)).collect();
    ledger
}

pub fn save_ledger(dir: &Path, ledger: &Ledger) -> Result<()> {
    save(&dir.join(LEDGER), ledger)
}

pub fn bridge(dir: &Path) -> Bridge {
    load(&dir.join(BRIDGE))
}

pub fn save_bridge(dir: &Path, bridge: &Bridge) -> Result<()> {
    save(&dir.join(BRIDGE), bridge)
}

// What hub.commands lets this machine take, as the board's accepts and authorize's gate.
pub fn accepts(policy: HubCommands) -> Vec<Kind> {
    policy.taken().into_iter().flat_map(|command| match command {
        HubCommand::Answer => vec![Kind::Answer, Kind::ItemAnswer],
        HubCommand::Push => vec![Kind::Push],
        HubCommand::Prompt => vec![Kind::PromptAnswer],
        HubCommand::Idea => vec![Kind::Idea],
        HubCommand::Start => vec![Kind::StartItem],
    }).collect()
}

// What the machine takes with hub.approve_from: a device's decision on another device only under "devices", and nothing at all under hub.commands = [].
pub fn accepted(policy: HubCommands, from: ApproveFrom) -> Vec<Kind> {
    let mut kinds = accepts(policy);
    if from == ApproveFrom::Devices && !kinds.is_empty() {
        kinds.push(Kind::DeviceDecision);
    }
    kinds
}

thread_local! {
    static ACTING: std::cell::RefCell<Option<Id>> = const { std::cell::RefCell::new(None) };
}

// The device whose command is being applied on this thread, so what it starts never notifies it of its own act.
pub fn acting() -> Option<Id> {
    ACTING.with(|acting| acting.borrow().clone())
}

fn as_device<T>(device: &Id, run: impl FnOnce() -> T) -> T {
    ACTING.with(|acting| *acting.borrow_mut() = Some(device.clone()));
    let done = run();
    ACTING.with(|acting| *acting.borrow_mut() = None);
    done
}

fn device_decision(dir: &Path, machine: &str, policy: Policy, decision: &DeviceDecision, now: u64) -> Done {
    let chosen = if decision.approve { Decision::Approve } else { Decision::Deny };
    match devices::decide_waiting(dir, &decision.device, &decision.fingerprint, chosen, policy, now) {
        Ok(Some(device)) => {
            let held: Vec<Asked> = ledger(dir).held.into_values().filter(|held| held.device == decision.device).map(|held| held.asked).collect();
            (Outcome::Done, plain(&devices::decided(&device, machine, chosen, &held, policy, true)))
        }
        Ok(None) => (Outcome::Refused { reason: Reason::UnknownDevice }, format!("Not decided: no device waits on {machine} with that id and key; it was decided already, or its key changed")),
        Err(error) => (Outcome::Failed, format!("Not decided: {error}")),
    }
}

// A kind hub.commands leaves out, named with what the machine takes and where the user can do it instead.
fn not_taken(kind: Kind, machine: &str, accepts: &[Kind]) -> String {
    let (refused, act, allow) = match kind {
        Kind::Push => ("Not pushed", "Push", "pushes"),
        Kind::PromptAnswer => ("Not chosen", "Choose", "prompt choices"),
        Kind::Answer | Kind::ItemAnswer => ("Not answered", "Answer", "answers"),
        Kind::Idea => ("Not added", "Add it", "ideas"),
        Kind::StartItem => ("Not started", "Start it", "starts"),
        Kind::DeviceDecision if accepts.is_empty() => return format!("Not decided: {machine} takes nothing from the hub. Approve or deny it at the machine: N on the board, or kebbie hub devices."),
        Kind::DeviceDecision => return format!("Not decided: {machine} takes device approvals only at the machine. Approve or deny it there: N on the board, or kebbie hub devices."),
        other => return format!("Not done: {machine} takes no {} from the hub", other.name()),
    };
    let takes = if accepts.is_empty() { "takes nothing from the hub".to_string() } else { format!("takes no {allow} from the hub") };
    format!("{refused}: {machine} {takes}. {act} at the machine, or allow {allow} in its kebbie settings.")
}

fn refusal_text(reason: Reason, kind: Kind, machine: &str, accepts: &[Kind]) -> String {
    match reason {
        Reason::Expired => "It expired before this machine read it; send it again".to_string(),
        Reason::NotAllowed => not_taken(kind, machine, accepts),
        Reason::Undecryptable => "This machine could not open it; reload the page and send it again".to_string(),
        Reason::BadRequest => "This machine could not read what was asked".to_string(),
        Reason::NotApproved => format!("Not delivered: {machine} had not approved this device before it expired. Approve it there with kebbie hub devices approve, then send it again."),
        Reason::Denied => format!("Not delivered: {machine} denied this device; it stays view only there."),
        other => format!("Refused: {}", other.key()),
    }
}

fn result(header: &Header, (outcome, text): Done) -> CommandResult {
    CommandResult { command: header.id.clone(), device: header.from.clone(), outcome, text }
}

// One command from the inbox: checked against the pinned account, hub.commands and the device's approval, applied at most once, and its result recorded before it is sent; a device waiting for approval has its command held.
#[allow(clippy::too_many_arguments)]
pub fn handle(dir: &Path, identity: &Identity, account: &AccountKey, accepts: &[Kind], policy: Policy, envelope: &Envelope, hands: &Hands, now: u64) -> Result<Handled> {
    let header = &envelope.header;
    let secret = identity.secret();
    let _ledger_lock = Locked::wait(dir, LEDGER_LOCK)?;
    let mut ledger = ledger(dir);
    let read = ledger.clone();
    ledger.applied.prune(now);
    ledger.started.retain(|_, started| started.at + AppliedCommands::KEEP_SECONDS > now);
    let (outcome, asking, approved) = devices::change(dir, |known| {
        let approved = known.settle(policy, now);
        let asked = std::cell::RefCell::new(Vec::new());
        let trust = |device: &VerifiedDevice, at: u64| -> Trust {
            asked.borrow_mut().push(device.clone());
            known.trust(policy, device, at)
        };
        let machine = Machine { id: &identity.machine, keys: &secret, account, accepts, trust: &trust };
        let outcome = authorize(&machine, &ledger.applied, envelope, now);
        let asking: Vec<(Id, String)> = asked.into_inner().into_iter().filter(|device| known.seen(policy, device, header.at, now)).map(|device| (device.id().clone(), device.cert().name.clone())).collect();
        Ok((outcome, asking, approved))
    })?;
    devices::settled(dir, &approved);
    for (device, name) in asking {
        (hands.notify)(&format!("{name} wants to make changes on {}: approve or deny it under N on the board, or with kebbie hub devices", identity.name));
        let added = devices::clock(now, &crate::recover::local_offset);
        super::notices::send_in(dir, &super::notices::About::NewDevice { device, name, added, from_devices: accepts.contains(&Kind::DeviceDecision) });
    }
    let handled = match outcome {
        Err(Refusal::Drop(dropped)) => Handled::Dropped(dropped),
        Err(Refusal::Replay(result)) => Handled::Replayed(result),
        Err(Refusal::Waiting(verified)) => {
            let device = verified.device.id().clone();
            ledger.held.entry(held_key(&device, &header.id)).or_insert_with(|| Held { envelope: envelope.clone(), device: device.clone(), asked: Asked::of(&verified.payload, devices::question_label) });
            if ledger != read {
                save_ledger(dir, &ledger)?;
            }
            return Ok(Handled::Held(device));
        }
        Err(Refusal::Answer(reason)) => {
            let reason = if reason == Reason::Expired && ledger.held.contains_key(&held_key(&header.from, &header.id)) { Reason::NotApproved } else { reason };
            Handled::Refused(result(header, (Outcome::Refused { reason }, refusal_text(reason, header.kind, &identity.name, accepts))))
        }
        Ok(_) if ledger.started.get(&header.id).is_some_and(|started| started.from == header.from) => Handled::Refused(result(header, (Outcome::Failed, STOPPED.to_string()))),
        Ok(verified) => {
            ledger.started.insert(header.id.clone(), Started { from: header.from.clone(), at: now });
            save_ledger(dir, &ledger)?;
            let caller = Caller::Hub { account: account.account.as_str().to_string(), name: verified.device.cert().name.clone() };
            let done = as_device(verified.device.id(), || match &verified.payload {
                Payload::Answer(answer) => (hands.answer)(&caller, answer),
                Payload::Push(push) => (hands.push)(&caller, push),
                Payload::PromptAnswer(prompt) => (hands.prompt)(&caller, prompt),
                Payload::Idea(idea) => (hands.idea)(&caller, idea),
                Payload::ItemAnswer(answer) => (hands.item_answer)(&caller, answer),
                Payload::StartItem(start) => (hands.start_item)(&caller, start),
                Payload::DeviceDecision(decision) => device_decision(dir, &identity.name, policy, decision, now),
                _ => (Outcome::Refused { reason: Reason::NotAllowed }, not_taken(header.kind, &identity.name, accepts)),
            });
            Handled::Applied(result(header, done))
        }
    };
    if let Handled::Applied(result) | Handled::Refused(result) = &handled {
        ledger.applied.record(now, result.clone());
    }
    if let Some(result) = handled.result() {
        ledger.unsent.insert(Unsent { device: result.device.clone(), command: result.command.clone() });
    }
    ledger.started.remove(&header.id);
    ledger.held.remove(&held_key(&header.from, &header.id));
    save_ledger(dir, &ledger)?;
    Ok(handled)
}

// The held commands authorized again: a device approved since has them applied, a denied one or one that expired while it waited has them answered.
pub fn release(dir: &Path, identity: &Identity, account: &AccountKey, accepts: &[Kind], policy: Policy, hands: &Hands, now: u64) -> Result<Vec<(Envelope, Handled)>> {
    let held: Vec<Envelope> = ledger(dir).held.into_values().map(|held| held.envelope).collect();
    held.into_iter().map(|envelope| handle(dir, identity, account, accepts, policy, &envelope, hands, now).map(|handled| (envelope, handled))).filter(|done| !matches!(done, Ok((_, Handled::Held(_))))).collect()
}

fn note(header: &Header, handled: &Handled) -> String {
    match handled {
        Handled::Held(device) => format!("commands: {} {} from {device} waits for the device to be approved", header.kind.name(), header.id),
        Handled::Dropped(_) => format!("commands: dropped {} {} from {}", header.kind.name(), header.id, header.from),
        Handled::Applied(result) | Handled::Refused(result) | Handled::Replayed(result) => format!("commands: {} {}: {}", header.kind.name(), header.id, result.text),
    }
}

fn sealed(identity: &Identity, account: &AccountKey, result: &CommandResult) -> Result<Envelope> {
    let header = Header::new(result.command.clone(), Kind::Result, identity.machine.clone(), identity.machine.clone(), crate::messages::now());
    seal(header, &Payload::Result(result.clone()), &identity.secret(), &[account.recipient(), (identity.machine.clone(), identity.public())]).map_err(|error| error.to_string())
}

// A result the hub holds already, or for a command it has forgotten, needs no second post.
fn deliver(client: &Client, identity: &Identity, account: &AccountKey, result: &CommandResult) -> Result<()> {
    let envelope = sealed(identity, account, result)?;
    match client.post::<_, ResultStored>("/v1/m/results", &ResultUpload { envelope }) {
        Ok(_) => Ok(()),
        Err(HubError::Status { status: 404, code }) if code == "unknown-command" => Ok(()),
        Err(HubError::Status { status: 409, code }) if code == "result-differs" => Ok(()),
        Err(error) => Err(format!("the result of {} was not sent: {error}", result.command)),
    }
}

// Every result the hub has yet to take goes out, oldest first; one that fails stays for the next round, and one whose record was pruned is let go.
pub fn send_unsent(dir: &Path, client: &Client, identity: &Identity, account: &AccountKey) -> Result<()> {
    let pending = ledger(dir);
    let mut sent = Vec::new();
    let mut failed = None;
    for unsent in &pending.unsent {
        match pending.applied.result(&unsent.device, &unsent.command) {
            None => sent.push(unsent.clone()),
            Some(result) => match deliver(client, identity, account, result) {
                Ok(()) => sent.push(unsent.clone()),
                Err(error) => {
                    failed = Some(error);
                    break;
                }
            },
        }
    }
    if !sent.is_empty() {
        let _ledger_lock = Locked::wait(dir, LEDGER_LOCK)?;
        let mut ledger = ledger(dir);
        ledger.unsent.retain(|unsent| !sent.contains(unsent));
        save_ledger(dir, &ledger)?;
    }
    failed.map_or(Ok(()), Err)
}

struct Alone(std::fs::File);

impl Alone {
    fn take(dir: &Path) -> Option<Alone> {
        let file = crate::state::append_to(&dir.join(LOCK)).ok()?;
        // SAFETY: flock on a descriptor this File owns.
        (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(Alone(file))
    }
}

impl Drop for Alone {
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

// The commands held for approval first, then one long poll of the inbox and everything it brings; a keeper handing over to its successor holds the lock, so two never poll at once.
pub fn round(dir: &Path, wait: u64, accepts: &[Kind], policy: Policy, hands: &Hands, clock: &dyn Fn() -> u64) -> Result<Vec<String>> {
    let Some(identity) = identity::load_at(dir)? else { return Ok(Vec::new()) };
    let Some(account) = identity.account.as_ref().map(|account| account.key.clone()) else { return Ok(Vec::new()) };
    let _alone = Alone::take(dir).ok_or("another kebbie applies this machine's commands now")?;
    let client = Client::new(&identity.url, Some(identity.token()), CALL);
    let released = release(dir, &identity, &account, accepts, policy, hands, clock())?;
    let mut applied = released.iter().any(|(_, handled)| matches!(handled, Handled::Applied(_)));
    let mut notes: Vec<String> = released.iter().map(|(envelope, handled)| note(&envelope.header, handled)).collect();
    send_unsent(dir, &client, &identity, &account)?;
    let mut bridge = bridge(dir);
    let polling = Client::new(&identity.url, Some(identity.token()), LONG_POLL);
    let inbox: Inbox = polling.get(&format!("/v1/m/inbox?after={}&wait={wait}", bridge.inbox_seq)).map_err(|error| error.to_string())?;
    for message in inbox.messages.iter().filter(|message| message.envelope.header.kind.is_command()) {
        let handled = handle(dir, &identity, &account, accepts, policy, &message.envelope, hands, clock())?;
        applied |= matches!(handled, Handled::Applied(_));
        notes.push(note(&message.envelope.header, &handled));
    }
    send_unsent(dir, &client, &identity, &account)?;
    bridge.inbox_seq = inbox.seq;
    save_bridge(dir, &bridge)?;
    if applied {
        (hands.relay)();
    }
    Ok(notes)
}

pub(super) fn plain(text: &str) -> String {
    let notice = Notice::read(text, false);
    let mut said = notice.headline;
    for more in [notice.next, notice.you].into_iter().flatten() {
        if !said.ends_with(['.', '!', '?', ':']) {
            said.push('.');
        }
        said.push(' ');
        said.push_str(&more);
    }
    said
}

pub fn answered(caller: &Caller, answer: &Answer) -> Done {
    use crate::questions::Unanswered;
    let refused = |reason, text: &str| (Outcome::Refused { reason }, plain(text));
    match crate::questions::asked_by(&answer.question) {
        None => return refused(Reason::AlreadySettled, "That question no longer waits for an answer"),
        Some(from) if from != answer.address => return refused(Reason::BadRequest, &format!("That question is {from}'s, not {}'s", answer.address)),
        Some(_) => {}
    }
    match crate::questions::answer_as(caller, &answer.question, answer.choice.map(usize::from), &answer.comment) {
        Ok(text) => (Outcome::Done, plain(&text)),
        Err(Unanswered::NotAllowed(text)) => refused(Reason::NotAllowed, &text),
        Err(Unanswered::Settled(text)) => refused(Reason::AlreadySettled, &text),
        Err(Unanswered::Invalid(text)) => refused(Reason::BadRequest, &text),
        Err(Unanswered::Failed(text)) => (Outcome::Failed, plain(&text)),
    }
}

pub fn chose(outcome: std::result::Result<String, crate::menu::Unchosen>) -> Done {
    use crate::menu::Unchosen;
    match outcome {
        Ok(text) => (Outcome::Done, plain(&text)),
        Err(Unchosen::Gone(text)) => (Outcome::Refused { reason: Reason::AlreadySettled }, plain(&text)),
        Err(Unchosen::NotAllowed(text)) => (Outcome::Refused { reason: Reason::NotAllowed }, plain(&text)),
        Err(Unchosen::Failed(text)) => (Outcome::Failed, plain(&text)),
    }
}

pub fn pushed(outcome: crate::tasks::DevicePush) -> Done {
    use crate::tasks::DevicePush;
    match outcome {
        DevicePush::Pushed { short, main } => (Outcome::Done, format!("Pushed {short} to {main}")),
        DevicePush::Queued(text) => (Outcome::Done, plain(&text)),
        DevicePush::Conflict { files, main } => (Outcome::Failed, format!("Not pushed yet: conflicts with {main} in {}; its Claude resolves it, then kebbie pushes it", files.join(", "))),
        DevicePush::Tests { files, main } => (Outcome::Failed, format!("Not pushed yet: {main} changed {} too; its Claude runs their tests, then kebbie pushes it", files.join(", "))),
        DevicePush::Settled(text) => (Outcome::Refused { reason: Reason::AlreadySettled }, plain(&text)),
        DevicePush::Refused(text) => (Outcome::Refused { reason: Reason::NotAllowed }, plain(&text)),
        DevicePush::UnknownTask(text) => (Outcome::Refused { reason: Reason::UnknownRepo }, plain(&text)),
        DevicePush::Failed(text) => (Outcome::Failed, plain(&text)),
    }
}

fn commands_on(url: &str) -> bool {
    Client::new(url, None, CALL).get::<HubInfo>("/v1/hub").is_ok_and(|info| info.commands)
}

fn pause(seconds: u64) {
    let until = Instant::now() + Duration::from_secs(seconds);
    while !crate::keeper::stopping() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(500).min(until.saturating_duration_since(Instant::now())));
    }
}

fn backoff(failures: u32) -> u64 {
    (5u64 << failures.min(10)).min(MAX_BACKOFF)
}

fn keeper_hands<T>(state: &Path, run: impl FnOnce(&Hands) -> T) -> T {
    let log = |text: &str| crate::keeper::log_line(state, text);
    let relay = || {
        if let Err(error) = super::tick() {
            log(&format!("commands: the board was not sent after a command: {error}"));
        }
    };
    let prompt = |caller: &Caller, answer: &PromptAnswer| chose(crate::menu::choose_as(caller, &answer.address, &answer.key, &answer.prompt));
    let notify = |text: &str| {
        crate::notify::send(state, &crate::notify::Notice::from(crate::notify::Event::Device, "kebbie", text.to_string()));
    };
    run(&Hands { answer: &answered, push: &|caller, push| pushed(crate::tasks::push_as(caller, &push.address)), prompt: &prompt, idea: &super::backlog::idea, item_answer: &super::backlog::item_answer, start_item: &super::backlog::start_item, relay: &relay, notify: &notify })
}

// The keeper's thread for one identity directory: it idles while the machine has no pinned account or the hub takes no commands, and long-polls otherwise.
fn serve(state: &Path, dir: &Path) {
    let log = |text: &str| crate::keeper::log_line(state, text);
    keeper_hands(state, |hands| {
        let mut failures = 0;
        let mut checked: Option<(bool, u64)> = None;
        let mut closed: Option<String> = None;
        while !crate::keeper::stopping() {
            let now = crate::messages::now();
            let Some(identity) = identity::load_at(dir).ok().flatten().filter(|identity| identity.account.is_some()) else {
                pause(IDLE);
                continue;
            };
            if checked.is_none_or(|(_, at)| now.saturating_sub(at) >= IDLE) {
                checked = Some((commands_on(&identity.url), now));
            }
            if !checked.is_some_and(|(on, _)| on) {
                pause(IDLE);
                continue;
            }
            let (gates, broken) = crate::config::hub_gates();
            if broken != closed {
                log(&closed_line(broken.as_deref()));
                closed = broken;
            }
            match round(dir, WAIT, &accepted(gates.commands, gates.from), Policy { trust: gates.trust, auto: gates.auto }, hands, &crate::messages::now) {
                Ok(notes) => {
                    failures = 0;
                    notes.iter().for_each(|note| log(note));
                }
                Err(error) => {
                    failures += 1;
                    log(&format!("commands: {error}; next try in {}", crate::keeper::span(backoff(failures))));
                    pause(backoff(failures));
                }
            }
        }
    });
}

fn closed_line(error: Option<&str>) -> String {
    match error {
        Some(error) => format!("commands: {error}; until config.toml parses, the hub's gates are closed: no commands are taken and devices are approved only at this machine"),
        None => "commands: config.toml parses again; the hub's gates follow it".to_string(),
    }
}

fn modified(file: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(file).and_then(|metadata| metadata.modified()).ok()
}

// A decision on the board or in a terminal changes devices.json while the inbox poll waits, so this thread releases what it held at once.
fn watch(state: &Path, dir: &Path) {
    let log = |text: &str| crate::keeper::log_line(state, text);
    keeper_hands(state, |hands| {
        let mut seen = modified(&dir.join(devices::FILE));
        let mut decided = devices::load(dir).decisions();
        while !crate::keeper::stopping() {
            std::thread::sleep(Duration::from_secs(1));
            let now = modified(&dir.join(devices::FILE));
            if now == seen {
                continue;
            }
            seen = now;
            let decisions = devices::load(dir).decisions();
            if decisions == decided {
                continue;
            }
            decided = decisions;
            match released(dir, hands) {
                Ok(notes) if notes.is_empty() => {}
                Ok(notes) => {
                    notes.iter().for_each(|note| log(note));
                    (hands.relay)();
                }
                Err(error) => log(&format!("commands: {error}")),
            }
        }
    });
}

pub fn released(dir: &Path, hands: &Hands) -> Result<Vec<String>> {
    let Some(identity) = identity::load_at(dir)? else { return Ok(Vec::new()) };
    let Some(account) = identity.account.as_ref().map(|account| account.key.clone()) else { return Ok(Vec::new()) };
    let pending = ledger(dir);
    if pending.held.is_empty() && pending.unsent.is_empty() {
        return Ok(Vec::new());
    }
    let client = Client::new(&identity.url, Some(identity.token()), CALL);
    let mut notes = Vec::new();
    let gates = crate::config::hub_gates().0;
    for (envelope, handled) in release(dir, &identity, &account, &accepted(gates.commands, gates.from), Policy { trust: gates.trust, auto: gates.auto }, hands, crate::messages::now())? {
        notes.push(note(&envelope.header, &handled));
    }
    send_unsent(dir, &client, &identity, &account)?;
    Ok(notes)
}

// Per identity directory, the real hub's and the dev hub's: one thread long-polls, one releases held commands when a device is decided, beside the keeper's routines, since a long poll would hold up their loop.
pub fn spawn(state: &Path) {
    for dev in [false, true] {
        for (name, run) in [("hub-commands", serve as fn(&Path, &Path)), ("hub-devices", watch)] {
            let (kept, dir): (PathBuf, PathBuf) = (state.to_path_buf(), identity::dir_of(state, dev));
            let name = if dev { format!("{name}-dev") } else { name.to_string() };
            if let Err(error) = std::thread::Builder::new().name(name.clone()).spawn(move || run(&kept, &dir)) {
                crate::keeper::log_line(state, &format!("commands: {name} did not start: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use kebbie_hub_protocol::{DeviceCert, DeviceKey, DeviceKind, Prefix, SecretKeys};

    use super::*;
    use crate::config::{AutoApprove, HubTrust};
    use crate::hub::devices::{Decision, State};

    const ACCOUNT: Policy = Policy { trust: HubTrust::Account, auto: AutoApprove::Day };
    const DEVICE: Policy = Policy { trust: HubTrust::Device, auto: AutoApprove::Day };
    const NEVER: Policy = Policy { trust: HubTrust::Device, auto: AutoApprove::Never };
    use crate::hub::tests::{TestHub, WAIT as LONG, connected_state, member, open_hub};

    fn now() -> u64 {
        crate::messages::now()
    }

    fn word(word: &str) -> HubCommands {
        HubCommands::from_word(word).unwrap()
    }

    struct Seen {
        answers: RefCell<Vec<(Caller, Answer)>>,
        pushes: RefCell<Vec<(Caller, PushApproval)>>,
        prompts: RefCell<Vec<(Caller, PromptAnswer)>>,
        backlog: RefCell<Vec<(Caller, Payload)>>,
        relays: Cell<u32>,
        told: RefCell<Vec<String>>,
    }

    impl Seen {
        fn new() -> Seen {
            Seen { answers: RefCell::new(Vec::new()), pushes: RefCell::new(Vec::new()), prompts: RefCell::new(Vec::new()), backlog: RefCell::new(Vec::new()), relays: Cell::new(0), told: RefCell::new(Vec::new()) }
        }

        fn applied(&self) -> usize {
            self.answers.borrow().len() + self.pushes.borrow().len() + self.prompts.borrow().len() + self.backlog.borrow().len()
        }
    }

    fn with_hands<T>(seen: &Seen, run: impl FnOnce(&Hands) -> T) -> T {
        let answer = |caller: &Caller, answer: &Answer| {
            seen.answers.borrow_mut().push((caller.clone(), answer.clone()));
            (Outcome::Done, "Answered".to_string())
        };
        let push = |caller: &Caller, push: &PushApproval| {
            seen.pushes.borrow_mut().push((caller.clone(), push.clone()));
            (Outcome::Done, "Pushed 9f3c2a1 to main".to_string())
        };
        let prompt = |caller: &Caller, prompt: &PromptAnswer| {
            seen.prompts.borrow_mut().push((caller.clone(), prompt.clone()));
            (Outcome::Done, "Chose it".to_string())
        };
        let idea = |caller: &Caller, idea: &Idea| {
            seen.backlog.borrow_mut().push((caller.clone(), Payload::Idea(idea.clone())));
            (Outcome::Done, "Sent to the lead".to_string())
        };
        let item_answer = |caller: &Caller, answer: &ItemAnswer| {
            seen.backlog.borrow_mut().push((caller.clone(), Payload::ItemAnswer(answer.clone())));
            (Outcome::Done, "With the lead".to_string())
        };
        let start_item = |caller: &Caller, start: &StartItem| {
            seen.backlog.borrow_mut().push((caller.clone(), Payload::StartItem(start.clone())));
            if start.anyway { (Outcome::Done, "Starting retry-red-runs@web".to_string()) } else { (Outcome::Refused { reason: Reason::QuestionsOpen { blocking: 2 } }, "Not started: 2 questions wait on you".to_string()) }
        };
        let relay = || seen.relays.set(seen.relays.get() + 1);
        let notify = |text: &str| seen.told.borrow_mut().push(text.to_string());
        run(&Hands { answer: &answer, push: &push, prompt: &prompt, idea: &idea, item_answer: &item_answer, start_item: &start_item, relay: &relay, notify: &notify })
    }

    fn answer() -> Payload {
        Payload::Answer(Answer { address: "refunds@shop".to_string(), question: "q1".to_string(), choice: Some(0), comment: String::new() })
    }

    fn push() -> Payload {
        Payload::Push(PushApproval { address: "refunds@shop".to_string(), head: "a".repeat(40), card_sha: None })
    }

    fn prompt_answer(key: &str) -> Payload {
        Payload::PromptAnswer(PromptAnswer { address: "hub@kebbie".to_string(), prompt: "3f2a9c1d".to_string(), key: key.to_string() })
    }

    // A browser signed in to the account the machine pinned, sending commands through a real hub.
    struct Account {
        hub: TestHub,
        browser: kebbie_hub_fakes::browser::Browser,
        dir: PathBuf,
        identity: Identity,
        account: Id,
    }

    impl Account {
        fn connected() -> Account {
            let hub = open_hub(|config| config.commands = true);
            let mut browser = member(&hub);
            let state = connected_state(&hub, &mut browser);
            let dir = identity::dir(&state);
            let identity = identity::load_at(&dir).unwrap().unwrap();
            browser.knows(&identity.machine, identity.public());
            let account = browser.account_key().unwrap().account.clone();
            Account { hub, browser, dir, identity, account }
        }

        fn send(&mut self, payload: &Payload) -> Envelope {
            self.hub.runtime.block_on(self.browser.command(&self.identity.machine, payload)).unwrap()
        }

        fn result(&mut self, envelope: &Envelope) -> CommandResult {
            self.hub.runtime.block_on(self.browser.await_result(&envelope.header.id, LONG)).unwrap().expect("a result reached the browser")
        }

        fn round(&self, seen: &Seen, policy: HubCommands) -> Vec<String> {
            self.round_under(seen, policy, ACCOUNT)
        }

        fn round_under(&self, seen: &Seen, policy: HubCommands, trust: Policy) -> Vec<String> {
            with_hands(seen, |hands| round(&self.dir, 0, &accepts(policy), trust, hands, &now)).unwrap()
        }

        fn caller(&self) -> Caller {
            Caller::Hub { account: self.account.as_str().to_string(), name: self.browser.device_name.clone() }
        }
    }

    #[test]
    fn the_accounts_answer_is_applied_once_as_the_user_through_the_hub_and_a_redelivery_only_resends_its_result() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let sent = account.send(&answer());
        account.round(&seen, word("answer-and-push"));
        assert_eq!(seen.answers.borrow().as_slice(), [(account.caller(), Answer { address: "refunds@shop".to_string(), question: "q1".to_string(), choice: Some(0), comment: String::new() })]);
        assert_eq!(seen.relays.get(), 1, "the board goes out again at once");
        let result = account.result(&sent);
        assert_eq!((result.outcome, result.text.as_str()), (Outcome::Done, "Answered"));
        assert_eq!(result.device, sent.header.from, "the result names the device that sent it");
        assert_eq!(ledger(&account.dir).applied.result(&sent.header.from, &sent.header.id), Some(&result), "recorded before it was sent");

        let mut lost = bridge(&account.dir);
        lost.inbox_seq = 0;
        save_bridge(&account.dir, &lost).unwrap();
        let notes = account.round(&seen, word("answer-and-push"));
        assert_eq!(seen.answers.borrow().len(), 1, "a command that comes again is never applied twice: {notes:?}");
        assert_eq!(seen.relays.get(), 1);

        let pushed = account.send(&push());
        account.round(&seen, word("answer-and-push"));
        assert_eq!(seen.pushes.borrow().len(), 1);
        assert_eq!(account.result(&pushed).text, "Pushed 9f3c2a1 to main");
        for file in [LEDGER, BRIDGE] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(account.dir.join(file)).unwrap().permissions().mode() & 0o777, 0o600, "{file} is private");
        }
    }

    #[test]
    fn a_kind_outside_hub_commands_is_refused_naming_the_kind_and_nothing_is_applied() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let pushed = account.send(&push());
        account.round(&seen, word("answer"));
        let refused = account.result(&pushed);
        assert_eq!((refused.outcome, refused.text.as_str()), (Outcome::Refused { reason: Reason::NotAllowed }, "Not pushed: mac takes no pushes from the hub. Push at the machine, or allow pushes in its kebbie settings."));
        let answered = account.send(&answer());
        account.round(&seen, word("answer"));
        assert_eq!(account.result(&answered).outcome, Outcome::Done, "answers still go through");

        let off = account.send(&answer());
        account.round(&seen, HubCommands::OFF);
        assert_eq!(account.result(&off).text, "Not answered: mac takes nothing from the hub. Answer at the machine, or allow answers in its kebbie settings.");
        assert!(seen.pushes.borrow().is_empty() && seen.answers.borrow().len() == 1, "only the allowed answer was applied");
    }

    #[test]
    fn an_expired_command_is_refused_and_nothing_is_applied() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let soon = account.browser.seal_command(&account.identity.machine, &answer(), now(), Some(60)).unwrap();
        account.hub.runtime.block_on(account.browser.send_command(&soon)).unwrap();
        with_hands(&seen, |hands| round(&account.dir, 0, &accepts(word("answer-and-push")), ACCOUNT, hands, &|| now() + 120)).unwrap();
        assert_eq!(account.result(&soon).outcome, Outcome::Refused { reason: Reason::Expired });
        assert!(seen.applied() == 0 && seen.relays.get() == 0, "nothing was applied");
    }

    #[test]
    fn a_command_cut_off_mid_apply_answers_failed_when_it_comes_again_and_is_never_applied_twice() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let sent = account.send(&answer());
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let answer = |_: &Caller, _: &Answer| -> Done {
                seen.answers.borrow_mut().push((Caller::User, Answer { address: String::new(), question: String::new(), choice: None, comment: String::new() }));
                panic!("kebbie stopped here")
            };
            let (push, prompt, relay, notify) = (|_: &Caller, _: &PushApproval| unreachable!(), |_: &Caller, _: &PromptAnswer| unreachable!(), || {}, |_: &str| {});
            let (idea, item_answer, start_item) = (|_: &Caller, _: &Idea| unreachable!(), |_: &Caller, _: &ItemAnswer| unreachable!(), |_: &Caller, _: &StartItem| unreachable!());
            round(&account.dir, 0, &accepts(word("answer-and-push")), ACCOUNT, &Hands { answer: &answer, push: &push, prompt: &prompt, idea: &idea, item_answer: &item_answer, start_item: &start_item, relay: &relay, notify: &notify }, &now)
        }));
        assert!(crashed.is_err());
        assert_eq!(seen.answers.borrow().len(), 1, "applied once, then cut off before it was recorded");
        assert!(ledger(&account.dir).started.contains_key(&sent.header.id));
        account.round(&seen, word("answer-and-push"));
        assert_eq!(seen.answers.borrow().len(), 1, "not applied a second time");
        let result = account.result(&sent);
        assert_eq!((result.outcome, result.text.as_str()), (Outcome::Failed, STOPPED));
        assert!(ledger(&account.dir).started.is_empty());
    }

    struct Phone {
        key: DeviceKey,
        cert: DeviceCert,
    }

    impl Phone {
        fn new(keys: &SecretKeys, account: &AccountKey, name: &str) -> Phone {
            Phone::with_id(keys, account, name, Id::new(Prefix::Device).unwrap())
        }

        fn with_id(keys: &SecretKeys, account: &AccountKey, name: &str, id: Id) -> Phone {
            let key = DeviceKey::generate();
            let cert = DeviceCert::issue(keys, &account.account, id, name, DeviceKind::Web, key.public(), 1_000).unwrap();
            Phone { key, cert }
        }

        fn id(&self) -> &Id {
            &self.cert.device
        }

        fn send(&self, account: &AccountKey, identity: &Identity, payload: &Payload, at: u64) -> Envelope {
            self.send_as(account, identity, payload, Id::new(Prefix::Command).unwrap(), at)
        }

        fn send_as(&self, account: &AccountKey, identity: &Identity, payload: &Payload, command: Id, at: u64) -> Envelope {
            let header = Header::command(command, payload.kind(), self.cert.clone(), identity.machine.clone(), at);
            kebbie_hub_protocol::seal_command(header, payload, &self.key, &identity.public(), account).unwrap()
        }
    }

    struct Machine {
        dir: PathBuf,
        identity: Identity,
        keys: SecretKeys,
        account: AccountKey,
    }

    impl Machine {
        fn new() -> Machine {
            let dir = crate::sh::test_dir("hub-devices");
            let identity = Identity::new("https://hub.example.com", Id::new(Prefix::Machine).unwrap(), "mac", "khm_token".to_string(), &SecretKeys::generate(), now());
            identity::save_at(&dir, &identity).unwrap();
            let keys = SecretKeys::generate();
            let account = AccountKey::new(Id::account([3; 16]), 1, keys.public()).unwrap();
            Machine { dir, identity, keys, account }
        }

        fn phone(&self, name: &str) -> Phone {
            Phone::new(&self.keys, &self.account, name)
        }

        fn handle(&self, seen: &Seen, policy: Policy, envelope: &Envelope, at: u64) -> Handled {
            with_hands(seen, |hands| handle(&self.dir, &self.identity, &self.account, &accepts(word("answer-and-push")), policy, envelope, hands, at)).unwrap()
        }

        fn handle_taking(&self, seen: &Seen, accepts: &[Kind], envelope: &Envelope) -> Handled {
            with_hands(seen, |hands| handle(&self.dir, &self.identity, &self.account, accepts, DEVICE, envelope, hands, now())).unwrap()
        }

        fn release(&self, seen: &Seen, policy: Policy, at: u64) -> Vec<Handled> {
            with_hands(seen, |hands| release(&self.dir, &self.identity, &self.account, &accepts(word("answer-and-push")), policy, hands, at)).unwrap().into_iter().map(|(_, handled)| handled).collect()
        }

        fn state(&self, phone: &Phone) -> Option<State> {
            devices::load(&self.dir).devices.get(phone.id()).map(|device| device.state)
        }
    }

    #[test]
    fn a_forged_command_or_one_another_account_signed_is_dropped_and_nothing_is_applied() {
        let machine = Machine::new();
        let other_keys = SecretKeys::generate();
        let other = AccountKey::new(Id::account([4; 16]), 1, other_keys.public()).unwrap();
        let phone = machine.phone("iPhone 16");
        let genuine = phone.send(&machine.account, &machine.identity, &answer(), now());
        let mut forged = genuine.clone();
        forged.sig = format!("{}{}", if forged.sig.starts_with('A') { 'B' } else { 'A' }, &forged.sig[1..]);
        let posing = AccountKey::new(machine.account.account.clone(), 1, other_keys.public()).unwrap();
        let impostor = Phone::new(&other_keys, &posing, "iPhone 16").send(&posing, &machine.identity, &answer(), now());
        let elsewhere = Phone::new(&other_keys, &other, "iPhone 16").send(&other, &machine.identity, &answer(), now());
        let seen = Seen::new();
        assert_eq!(machine.handle(&seen, ACCOUNT, &forged, now()), Handled::Dropped(Dropped::BadSignature));
        assert_eq!(machine.handle(&seen, ACCOUNT, &impostor, now()), Handled::Dropped(Dropped::BadCertificate), "a device the pinned account's id names, certified by another key");
        assert_eq!(machine.handle(&seen, ACCOUNT, &elsewhere, now()), Handled::Dropped(Dropped::WrongAccount), "a command another account signed");
        assert_eq!(seen.applied(), 0, "nothing was applied");
        assert!(ledger(&machine.dir).applied.is_empty(), "a dropped command leaves no result");
        assert!(devices::load(&machine.dir).devices.is_empty(), "a device that did not verify is never recorded");
        assert!(matches!(machine.handle(&seen, ACCOUNT, &genuine, now()), Handled::Applied(_)), "the genuine command from the pinned account goes through");
        assert_eq!(seen.answers.borrow().len(), 1);
    }

    #[test]
    fn a_new_devices_command_waits_and_goes_in_as_that_device_once_approved() {
        let machine = Machine::new();
        let seen = Seen::new();
        let phone = machine.phone("iPhone 16");
        let sent = phone.send(&machine.account, &machine.identity, &answer(), now());
        assert_eq!(machine.handle(&seen, DEVICE, &sent, now()), Handled::Held(phone.id().clone()));
        assert_eq!(seen.applied(), 0, "nothing is applied while it waits");
        assert_eq!(seen.told.borrow().as_slice(), ["iPhone 16 wants to make changes on mac: approve or deny it under N on the board, or with kebbie hub devices"]);
        assert_eq!(machine.state(&phone), Some(State::Waiting));
        let held = &ledger(&machine.dir).held[&held_key(phone.id(), &sent.header.id)];
        assert_eq!((held.device.clone(), held.asked.described()), (phone.id().clone(), "an answer to refunds".to_string()));
        assert_eq!(machine.handle(&seen, DEVICE, &sent, now()), Handled::Held(phone.id().clone()), "the same command again waits once");
        assert_eq!((ledger(&machine.dir).held.len(), seen.told.borrow().len()), (1, 1), "and is neither kept twice nor told twice");
        assert!(machine.release(&seen, DEVICE, now()).is_empty(), "still waiting, nothing is answered");

        let approved = devices::answer(&machine.dir, "iPhone 16", Decision::Approve, now()).unwrap();
        assert!(approved.starts_with("✓ iPhone 16 makes changes on mac from now on\n  next: Its answer to refunds goes in"), "{approved}");
        assert!(approved.ends_with(&format!("  key: {}", phone.cert.fingerprint())), "{approved}");
        let released = machine.release(&seen, DEVICE, now());
        let [Handled::Applied(result)] = released.as_slice() else { panic!("{released:?}") };
        assert_eq!((result.outcome, result.device.clone()), (Outcome::Done, phone.id().clone()));
        assert_eq!(seen.answers.borrow()[0].0, Caller::Hub { account: machine.account.account.as_str().to_string(), name: "iPhone 16".to_string() }, "the answer names the device");
        assert_eq!(crate::permit::describe(&seen.answers.borrow()[0].0), "the user, from iPhone 16");
        assert!(ledger(&machine.dir).held.is_empty());
        let later = phone.send(&machine.account, &machine.identity, &push(), now());
        assert!(matches!(machine.handle(&seen, DEVICE, &later, now()), Handled::Applied(_)), "an approved device's next command goes in at once");
    }

    #[test]
    fn a_held_command_authorized_again_rewrites_neither_devices_json_nor_the_ledger() {
        let machine = Machine::new();
        let seen = Seen::new();
        let phone = machine.phone("iPhone 16");
        let sent = phone.send(&machine.account, &machine.identity, &answer(), now());
        assert_eq!(machine.handle(&seen, DEVICE, &sent, now()), Handled::Held(phone.id().clone()));
        let stamp = |file: &str| std::fs::metadata(machine.dir.join(file)).and_then(|metadata| metadata.modified()).unwrap();
        let (devices_at, ledger_at, decided) = (stamp(devices::FILE), stamp(LEDGER), devices::load(&machine.dir).decisions());
        for later in [now() + 1, now() + 30, now() + 600] {
            assert!(machine.release(&seen, DEVICE, later).is_empty(), "still waiting");
        }
        assert_eq!(machine.handle(&seen, DEVICE, &sent, now() + 900), Handled::Held(phone.id().clone()), "the inbox bringing it again");
        assert_eq!((stamp(devices::FILE), stamp(LEDGER)), (devices_at, ledger_at), "each write would wake the keeper's watch into another round");
        assert_eq!(devices::load(&machine.dir).decisions(), decided);
        let later = phone.send(&machine.account, &machine.identity, &push(), now() + 60);
        machine.handle(&seen, DEVICE, &later, now() + 60);
        assert_eq!(devices::load(&machine.dir).decisions(), decided, "a new command from the waiting device is no decision, so the watch releases nothing");
        devices::answer(&machine.dir, "iPhone 16", Decision::Approve, now() + 61).unwrap();
        assert_ne!(devices::load(&machine.dir).decisions(), decided, "an approval is");
    }

    #[test]
    fn a_denied_devices_held_command_is_answered_denied_and_so_is_every_later_one() {
        let machine = Machine::new();
        let seen = Seen::new();
        let phone = machine.phone("iPhone 16");
        let sent = phone.send(&machine.account, &machine.identity, &answer(), now());
        machine.handle(&seen, DEVICE, &sent, now());
        let denied = devices::answer(&machine.dir, phone.id().as_str(), Decision::Deny, now()).unwrap();
        assert!(denied.starts_with("✓ iPhone 16 stays view only on mac\n  next: Its answer to refunds is dropped; kebbie hub devices approve \"iPhone 16\" lets it in later."), "{denied}");
        let released = machine.release(&seen, DEVICE, now());
        let [Handled::Refused(result)] = released.as_slice() else { panic!("{released:?}") };
        assert_eq!((result.outcome, result.text.as_str()), (Outcome::Refused { reason: Reason::Denied }, "Not delivered: mac denied this device; it stays view only there."));
        let later = phone.send(&machine.account, &machine.identity, &answer(), now() + 10 * devices::DAY);
        assert!(matches!(machine.handle(&seen, DEVICE, &later, now() + 10 * devices::DAY), Handled::Refused(CommandResult { outcome: Outcome::Refused { reason: Reason::Denied }, .. })), "a denial never lapses into an approval");
        assert_eq!(seen.applied(), 0);
    }

    #[test]
    fn a_command_that_expires_while_its_device_waits_is_answered_not_approved() {
        let machine = Machine::new();
        let seen = Seen::new();
        let phone = machine.phone("iPhone 16");
        let sent = phone.send(&machine.account, &machine.identity, &answer(), now());
        machine.handle(&seen, DEVICE, &sent, now());
        assert!(machine.release(&seen, DEVICE, sent.header.exp - 1).is_empty());
        let released = machine.release(&seen, DEVICE, sent.header.exp);
        let [Handled::Refused(result)] = released.as_slice() else { panic!("{released:?}") };
        assert_eq!(result.outcome, Outcome::Refused { reason: Reason::NotApproved });
        assert_eq!(result.text, "Not delivered: mac had not approved this device before it expired. Approve it there with kebbie hub devices approve, then send it again.");
        assert!(ledger(&machine.dir).held.is_empty() && seen.applied() == 0);
        assert_eq!(machine.state(&phone), Some(State::Waiting), "the device still waits for the user");
    }

    #[test]
    fn a_waiting_device_is_approved_a_day_after_its_first_command_unless_auto_approve_is_never() {
        let machine = Machine::new();
        let seen = Seen::new();
        let phone = machine.phone("iPhone 16");
        let start = now();
        let first = phone.send(&machine.account, &machine.identity, &answer(), start);
        machine.handle(&seen, DEVICE, &first, start);
        let standing = devices::load(&machine.dir).standings(DEVICE, start);
        assert_eq!(standing[phone.id()], kebbie_hub_protocol::DeviceStanding::Waiting { fingerprint: phone.cert.fingerprint(), until: Some(start + devices::DAY) });
        let early = phone.send(&machine.account, &machine.identity, &answer(), start + devices::DAY - 1);
        assert!(matches!(machine.handle(&seen, NEVER, &early, start + devices::DAY + 1), Handled::Held(_)), "never keeps it waiting past the day");
        assert!(matches!(machine.handle(&seen, DEVICE, &early, start + devices::DAY - 1), Handled::Held(_)), "a second short of the day it still waits");
        let late = phone.send(&machine.account, &machine.identity, &push(), start + devices::DAY);
        assert!(matches!(machine.handle(&seen, DEVICE, &late, start + devices::DAY), Handled::Applied(_)), "24h lets it in a day after its first command");
        let device = &devices::load(&machine.dir).devices[phone.id()];
        assert_eq!((device.state, device.at), (State::Approved, start + devices::DAY));
        let released = machine.release(&seen, DEVICE, start + devices::DAY);
        assert_eq!(released.iter().filter(|handled| matches!(handled, Handled::Applied(_))).count(), 1, "what it held and is still live goes in too: {released:?}");
        assert!(released.iter().any(|handled| matches!(handled, Handled::Refused(CommandResult { outcome: Outcome::Refused { reason: Reason::NotApproved }, .. }))), "the first command's hour ran out while it waited: {released:?}");
        assert!(ledger(&machine.dir).held.is_empty());
    }

    #[test]
    fn a_device_whose_key_changed_is_a_new_request_however_it_stood() {
        let machine = Machine::new();
        let seen = Seen::new();
        let phone = machine.phone("iPhone 16");
        let first = phone.send(&machine.account, &machine.identity, &answer(), now());
        machine.handle(&seen, DEVICE, &first, now());
        devices::answer(&machine.dir, "iPhone 16", Decision::Approve, now()).unwrap();
        machine.release(&seen, DEVICE, now());
        let rekeyed = Phone::with_id(&machine.keys, &machine.account, "iPhone 16", phone.id().clone());
        let sent = rekeyed.send(&machine.account, &machine.identity, &answer(), now() + 60);
        assert_eq!(machine.handle(&seen, DEVICE, &sent, now() + 60), Handled::Held(phone.id().clone()), "approval holds for the key it was given");
        let known = devices::load(&machine.dir);
        let device = known.record(phone.id(), &rekeyed.key.public().signing).expect("the new key is a request of its own");
        assert_eq!((device.state, device.first_seen), (State::Waiting, now() + 60));
        assert_eq!(known.record(phone.id(), &phone.key.public().signing).map(|device| device.state), Some(State::Approved), "the first key's record is left as it was decided");
        assert_eq!(seen.told.borrow().len(), 2, "the user is asked again");
        assert!(matches!(machine.handle(&seen, DEVICE, &phone.send(&machine.account, &machine.identity, &push(), now() + 90), now() + 90), Handled::Applied(_)), "the approved device still makes changes");
    }

    #[test]
    fn another_key_under_a_known_id_never_takes_over_its_record_and_approving_by_name_asks_which() {
        let machine = Machine::new();
        let seen = Seen::new();
        let phone = machine.phone("iPhone 16");
        machine.handle(&seen, DEVICE, &phone.send(&machine.account, &machine.identity, &answer(), now()), now());
        let posing = Phone::with_id(&machine.keys, &machine.account, "iPhone 16", phone.id().clone());
        machine.handle(&seen, DEVICE, &posing.send(&machine.account, &machine.identity, &answer(), now() + 60), now() + 60);
        let known = devices::load(&machine.dir);
        assert_eq!(known.records().count(), 2, "two requests, neither merged into the other");
        assert_eq!(known.record(phone.id(), &phone.key.public().signing).map(|device| device.first_seen), Some(now()), "the first key's record keeps its own day");
        let refused = devices::answer(&machine.dir, "iPhone 16", Decision::Approve, now() + 120).unwrap_err();
        let (real, other) = (phone.cert.fingerprint(), posing.cert.fingerprint());
        assert!(refused.starts_with("2 devices answer to \"iPhone 16\"") && refused.contains(&real) && refused.contains(&other), "{refused}");
        assert!(devices::answer(&machine.dir, phone.id().as_str(), Decision::Approve, now() + 120).is_err(), "the id alone names both");
        devices::answer(&machine.dir, &format!("{} {real}", phone.id().as_str()), Decision::Approve, now() + 120).unwrap();
        let known = devices::load(&machine.dir);
        assert_eq!(known.record(phone.id(), &phone.key.public().signing).map(|device| device.state), Some(State::Approved));
        assert_eq!(known.record(phone.id(), &posing.key.public().signing).map(|device| device.state), Some(State::Waiting), "the other key is still to decide");
        let requests = devices::requests_in(&machine.dir, DEVICE, now() + 120);
        assert_eq!(requests.iter().map(|request| request.fingerprint.clone()).collect::<Vec<_>>(), [other], "the board's Next asks about the other key only");
        assert!(matches!(machine.handle(&seen, DEVICE, &posing.send(&machine.account, &machine.identity, &push(), now() + 130), now() + 130), Handled::Held(_)), "its commands still wait");
    }

    #[test]
    fn under_account_trust_every_device_on_the_account_goes_in_at_once_and_is_listed() {
        let machine = Machine::new();
        let seen = Seen::new();
        let phone = machine.phone("iPad");
        let sent = phone.send(&machine.account, &machine.identity, &answer(), now());
        assert!(matches!(machine.handle(&seen, ACCOUNT, &sent, now()), Handled::Applied(_)));
        assert!(seen.told.borrow().is_empty(), "nothing to approve, nothing to tell");
        assert_eq!(machine.state(&phone), Some(State::Approved), "recorded for kebbie hub devices");
        assert!(matches!(devices::load(&machine.dir).standings(ACCOUNT, now())[phone.id()], kebbie_hub_protocol::DeviceStanding::Approved { .. }));
    }

    #[test]
    fn a_name_two_devices_share_is_refused_with_their_ids() {
        let machine = Machine::new();
        let seen = Seen::new();
        let (one, two) = (machine.phone("iPhone"), machine.phone("iPhone"));
        for phone in [&one, &two] {
            machine.handle(&seen, DEVICE, &phone.send(&machine.account, &machine.identity, &answer(), now()), now());
        }
        let refused = devices::answer(&machine.dir, "iPhone", Decision::Approve, now()).unwrap_err();
        assert!(refused.starts_with("2 devices answer to \"iPhone\"; name one by its id and key") && refused.contains(one.id().as_str()) && refused.contains(two.id().as_str()) && refused.contains(&one.cert.fingerprint()), "{refused}");
        assert_eq!((machine.state(&one), machine.state(&two)), (Some(State::Waiting), Some(State::Waiting)), "neither was decided");
        assert!(devices::answer(&machine.dir, one.id().as_str(), Decision::Approve, now()).is_ok());
        assert!(devices::answer(&machine.dir, "iPad", Decision::Approve, now()).unwrap_err().starts_with("No device \"iPad\" has sent this machine a command"));
    }

    #[test]
    fn a_held_answer_reaches_the_browser_once_its_device_is_approved() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let sent = account.send(&answer());
        account.round_under(&seen, word("answer-and-push"), DEVICE);
        assert_eq!(seen.applied(), 0, "held, not applied");
        let name = account.browser.device_name.clone();
        devices::answer(&account.dir, &name, Decision::Approve, now()).unwrap();
        account.round_under(&seen, word("answer-and-push"), DEVICE);
        assert_eq!(seen.answers.borrow().as_slice().iter().map(|(caller, _)| caller.clone()).collect::<Vec<_>>(), [account.caller()]);
        let result = account.result(&sent);
        assert_eq!((result.outcome, result.text.as_str(), result.device.clone()), (Outcome::Done, "Answered", sent.header.from.clone()));
    }

    #[test]
    fn a_released_result_the_hub_did_not_take_is_sent_again_on_the_next_round() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let sent = account.send(&answer());
        account.round_under(&seen, word("answer-and-push"), DEVICE);
        devices::answer(&account.dir, sent.header.from.as_str(), Decision::Approve, now()).unwrap();
        let mut offline = account.identity.clone();
        offline.url = "http://127.0.0.1:9".to_string();
        let key = account.identity.account.as_ref().unwrap().key.clone();
        let released = with_hands(&seen, |hands| release(&account.dir, &offline, &key, &accepts(word("answer-and-push")), DEVICE, hands, now())).unwrap();
        assert!(matches!(released.as_slice(), [(_, Handled::Applied(_))]), "{released:?}");
        let unreachable = Client::new(&offline.url, Some(offline.token()), CALL);
        assert!(send_unsent(&account.dir, &unreachable, &offline, &key).is_err());
        assert_eq!(ledger(&account.dir).unsent.len(), 1, "kept for the next round, not lost with the failed post");
        assert!(ledger(&account.dir).held.is_empty(), "applied once");
        account.round_under(&seen, word("answer-and-push"), DEVICE);
        assert_eq!(seen.answers.borrow().len(), 1, "never applied twice");
        assert_eq!(account.result(&sent).text, "Answered");
        assert!(ledger(&account.dir).unsent.is_empty(), "taken by the hub, it leaves");
    }

    #[test]
    fn held_commands_are_keyed_by_device_and_id_so_two_devices_sharing_an_id_both_wait() {
        let machine = Machine::new();
        let seen = Seen::new();
        let (one, two) = (machine.phone("iPhone"), machine.phone("iPad"));
        let first = one.send(&machine.account, &machine.identity, &answer(), now());
        let second = two.send_as(&machine.account, &machine.identity, &push(), first.header.id.clone(), now());
        assert_eq!(machine.handle(&seen, DEVICE, &first, now()), Handled::Held(one.id().clone()));
        assert_eq!(machine.handle(&seen, DEVICE, &second, now()), Handled::Held(two.id().clone()));
        assert_eq!(ledger(&machine.dir).held.len(), 2, "neither overwrites the other");
        devices::answer(&machine.dir, "iPad", Decision::Approve, now()).unwrap();
        let released = machine.release(&seen, DEVICE, now());
        assert!(matches!(released.as_slice(), [Handled::Applied(result)] if result.device == *two.id()), "{released:?}");
        assert_eq!(seen.pushes.borrow().len(), 1);
        assert_eq!(ledger(&machine.dir).held.keys().cloned().collect::<Vec<_>>(), [held_key(one.id(), &first.header.id)], "the other device's command still waits");
    }

    #[test]
    fn hub_commands_drives_what_the_machine_takes_and_names_what_it_refuses() {
        assert_eq!(accepts(HubCommands::default()), [Kind::Answer, Kind::ItemAnswer, Kind::Push, Kind::PromptAnswer, Kind::Idea, Kind::StartItem], "every kind unless set");
        assert_eq!(accepts(word("answer-and-push")), [Kind::Answer, Kind::ItemAnswer, Kind::Push, Kind::PromptAnswer]);
        assert_eq!(accepts(word("answer")), [Kind::Answer, Kind::ItemAnswer, Kind::PromptAnswer]);
        assert_eq!(accepts(HubCommands::of(&[HubCommand::Idea, HubCommand::Start])), [Kind::Idea, Kind::StartItem]);
        assert!(accepts(HubCommands::OFF).is_empty());
        let off = accepts(HubCommands::OFF);
        assert_eq!(not_taken(Kind::Push, "devbox", &off), "Not pushed: devbox takes nothing from the hub. Push at the machine, or allow pushes in its kebbie settings.");
        assert_eq!(not_taken(Kind::PromptAnswer, "devbox", &off), "Not chosen: devbox takes nothing from the hub. Choose at the machine, or allow prompt choices in its kebbie settings.");
        assert_eq!(not_taken(Kind::StartItem, "devbox", &accepts(word("answer"))), "Not started: devbox takes no starts from the hub. Start it at the machine, or allow starts in its kebbie settings.");
        assert_eq!(not_taken(Kind::Idea, "devbox", &accepts(word("answer"))), "Not added: devbox takes no ideas from the hub. Add it at the machine, or allow ideas in its kebbie settings.");
        assert_eq!(not_taken(Kind::Start, "devbox", &accepts(word("answer-and-push"))), "Not done: devbox takes no start from the hub");
        assert_eq!(crate::hub::commands_line(word("answer")), "commands: answer, prompt (hub.commands = [\"answer\", \"prompt\"])");
        assert_eq!(crate::hub::commands_line(HubCommands::OFF), "commands: off (hub.commands = [])");
    }

    fn decision(phone: &Phone, fingerprint: &str, approve: bool) -> Payload {
        Payload::DeviceDecision(DeviceDecision { device: phone.id().clone(), fingerprint: fingerprint.to_string(), approve })
    }

    #[test]
    fn an_approved_device_approves_a_waiting_one_whose_id_and_key_match_and_its_held_command_goes_in() {
        let machine = Machine::new();
        let seen = Seen::new();
        let devices_too = accepted(word("answer-and-push"), ApproveFrom::Devices);
        let phone = machine.phone("iPhone 16");
        machine.handle(&seen, DEVICE, &phone.send(&machine.account, &machine.identity, &answer(), now()), now());
        devices::answer(&machine.dir, "iPhone 16", Decision::Approve, now()).unwrap();
        machine.release(&seen, DEVICE, now());
        crate::hub::notices::SENT.with(|sent| sent.borrow_mut().clear());
        let tablet = machine.phone("iPad");
        let held = tablet.send(&machine.account, &machine.identity, &answer(), now());
        assert_eq!(machine.handle_taking(&seen, &devices_too, &held), Handled::Held(tablet.id().clone()));
        let told = crate::hub::notices::SENT.with(|sent| sent.borrow().clone());
        assert!(matches!(told.as_slice(), [(crate::hub::notices::About::NewDevice { device, name, from_devices: true, .. }, None)] if device == tablet.id() && name == "iPad"), "the new device is told to the approved ones, with the tap to approve: {told:?}");

        let wrong = phone.send(&machine.account, &machine.identity, &decision(&tablet, "0000-0000", true), now());
        let Handled::Applied(refused) = machine.handle_taking(&seen, &devices_too, &wrong) else { panic!("applied as a refusal") };
        assert_eq!(refused.outcome, Outcome::Refused { reason: Reason::UnknownDevice });
        assert_eq!(machine.state(&tablet), Some(State::Waiting), "a key that does not match decides nothing");

        let right = phone.send(&machine.account, &machine.identity, &decision(&tablet, &tablet.cert.fingerprint(), true), now());
        let Handled::Applied(done) = machine.handle_taking(&seen, &devices_too, &right) else { panic!("applied") };
        assert_eq!(done.outcome, Outcome::Done);
        assert!(done.text.starts_with("iPad makes changes on mac from now on. Its answer to refunds goes in now."), "{}", done.text);
        assert_eq!(machine.state(&tablet), Some(State::Approved));
        assert!(matches!(machine.release(&seen, DEVICE, now()).as_slice(), [Handled::Applied(_)]), "what the approved device held goes in");
        let again = phone.send(&machine.account, &machine.identity, &decision(&tablet, &tablet.cert.fingerprint(), false), now());
        assert!(matches!(machine.handle_taking(&seen, &devices_too, &again), Handled::Applied(CommandResult { outcome: Outcome::Refused { reason: Reason::UnknownDevice }, .. })), "a device decided already is no longer waiting");
    }

    #[test]
    fn a_waiting_device_never_decides_and_under_approve_from_machine_no_device_does() {
        let machine = Machine::new();
        let seen = Seen::new();
        let phone = machine.phone("iPhone 16");
        machine.handle(&seen, DEVICE, &phone.send(&machine.account, &machine.identity, &answer(), now()), now());
        devices::answer(&machine.dir, "iPhone 16", Decision::Approve, now()).unwrap();
        let (tablet, laptop) = (machine.phone("iPad"), machine.phone("Chrome on Mac"));
        machine.handle(&seen, DEVICE, &laptop.send(&machine.account, &machine.identity, &answer(), now()), now());
        let devices_too = accepted(word("answer-and-push"), ApproveFrom::Devices);
        let from_waiting = tablet.send(&machine.account, &machine.identity, &decision(&laptop, &laptop.cert.fingerprint(), true), now());
        assert!(matches!(machine.handle_taking(&seen, &devices_too, &from_waiting), Handled::Refused(CommandResult { outcome: Outcome::Refused { reason: Reason::NotApproved }, .. })));
        let machine_only = accepted(word("answer-and-push"), ApproveFrom::Machine);
        let refused = phone.send(&machine.account, &machine.identity, &decision(&laptop, &laptop.cert.fingerprint(), true), now());
        let Handled::Refused(result) = machine.handle_taking(&seen, &machine_only, &refused) else { panic!("refused") };
        assert_eq!((result.outcome, result.text.as_str()), (Outcome::Refused { reason: Reason::NotAllowed }, "Not decided: mac takes device approvals only at the machine. Approve or deny it there: N on the board, or kebbie hub devices."));
        assert_eq!(machine.state(&laptop), Some(State::Waiting));
        assert!(accepted(HubCommands::OFF, ApproveFrom::Devices).is_empty(), "hub.commands = [] takes nothing, device decisions included");
        let off = phone.send(&machine.account, &machine.identity, &decision(&laptop, &laptop.cert.fingerprint(), true), now());
        let Handled::Refused(result) = machine.handle_taking(&seen, &accepted(HubCommands::OFF, ApproveFrom::Devices), &off) else { panic!("refused") };
        assert_eq!((result.outcome, result.text.as_str()), (Outcome::Refused { reason: Reason::NotAllowed }, "Not decided: mac takes nothing from the hub. Approve or deny it at the machine: N on the board, or kebbie hub devices."));
        assert_eq!(machine.state(&laptop), Some(State::Waiting), "an approved device decides nothing while the machine takes nothing");
        assert!(!accepted(HubCommands::default(), ApproveFrom::Machine).contains(&Kind::DeviceDecision));
    }

    fn idea() -> Payload {
        Payload::Idea(Idea { text: "Rerun a flaky red run once".to_string(), repo: Some("web".to_string()) })
    }

    fn item_answer() -> Payload {
        Payload::ItemAnswer(ItemAnswer { item: kebbie_hub_protocol::ItemKey::parse("vault:Projects/retry.md").unwrap(), index: 0, question: "Which red runs count as flaky?".to_string(), answer: "Only e2e".to_string() })
    }

    fn start_item(anyway: bool) -> Payload {
        Payload::StartItem(StartItem { item: kebbie_hub_protocol::ItemKey::parse("vault:Projects/retry.md").unwrap(), anyway })
    }

    #[test]
    fn the_phones_idea_answer_and_start_go_in_as_the_user_from_that_device_and_their_results_reach_the_browser() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let sent = [idea(), item_answer(), start_item(false), start_item(true)].map(|payload| account.send(&payload));
        account.round(&seen, HubCommands::default());
        let applied: Vec<(Caller, Kind)> = seen.backlog.borrow().iter().map(|(caller, payload)| (caller.clone(), payload.kind())).collect();
        assert_eq!(applied, [Kind::Idea, Kind::ItemAnswer, Kind::StartItem, Kind::StartItem].map(|kind| (account.caller(), kind)));
        let results: Vec<(Outcome, String)> = sent.iter().map(|envelope| account.result(envelope)).map(|result| (result.outcome, result.text)).collect();
        assert_eq!(results[0], (Outcome::Done, "Sent to the lead".to_string()));
        assert_eq!(results[1], (Outcome::Done, "With the lead".to_string()));
        assert_eq!(results[2], (Outcome::Refused { reason: Reason::QuestionsOpen { blocking: 2 } }, "Not started: 2 questions wait on you".to_string()), "the count reaches the browser");
        assert_eq!(results[3], (Outcome::Done, "Starting retry-red-runs@web".to_string()));
        assert_eq!(seen.relays.get(), 1, "the board goes out again at once");
    }

    #[test]
    fn a_backlog_kind_hub_commands_leaves_out_is_refused_and_a_new_devices_waits_like_any_other() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let sent = [idea(), start_item(true)].map(|payload| account.send(&payload));
        account.round(&seen, HubCommands::of(&[HubCommand::Answer, HubCommand::Idea]));
        assert_eq!(account.result(&sent[0]).outcome, Outcome::Done);
        let refused = account.result(&sent[1]);
        assert_eq!((refused.outcome, refused.text.as_str()), (Outcome::Refused { reason: Reason::NotAllowed }, "Not started: mac takes no starts from the hub. Start it at the machine, or allow starts in its kebbie settings."));
        assert_eq!(seen.backlog.borrow().len(), 1, "only the idea was applied");

        let machine = Machine::new();
        let phone = machine.phone("iPhone 16");
        let held = phone.send(&machine.account, &machine.identity, &item_answer(), now());
        assert_eq!(machine.handle(&seen, DEVICE, &held, now()), Handled::Held(phone.id().clone()));
        assert_eq!(ledger(&machine.dir).held[&held_key(&held.header.from, &held.header.id)].asked.described(), "an answer on the backlog item Projects/retry.md, \"Only e2e\"");
        devices::answer(&machine.dir, "iPhone 16", Decision::Approve, now()).unwrap();
        assert!(matches!(machine.release(&seen, DEVICE, now()).as_slice(), [Handled::Applied(_)]));
        assert_eq!(seen.backlog.borrow().len(), 2, "approved, it goes in");
    }

    #[test]
    fn the_accounts_prompt_answer_is_applied_as_the_user_through_the_hub_and_refused_once_ten_minutes_passed() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let sent = account.send(&prompt_answer("y"));
        assert_eq!(sent.header.exp - sent.header.at, 600, "a screen prompt is momentary");
        account.round(&seen, word("answer-and-push"));
        assert_eq!(seen.prompts.borrow().as_slice(), [(account.caller(), PromptAnswer { address: "hub@kebbie".to_string(), prompt: "3f2a9c1d".to_string(), key: "y".to_string() })]);
        assert_eq!(account.result(&sent).text, "Chose it");
        assert_eq!(seen.relays.get(), 1, "the board goes out again at once");

        let late = account.send(&prompt_answer("esc"));
        with_hands(&seen, |hands| round(&account.dir, 0, &accepts(word("answer")), ACCOUNT, hands, &|| now() + 601)).unwrap();
        assert_eq!(account.result(&late).outcome, Outcome::Refused { reason: Reason::Expired });
        assert_eq!(seen.prompts.borrow().len(), 1, "nothing chosen after ten minutes");
    }

    #[test]
    fn a_choice_the_screen_no_longer_offers_is_settled_and_a_text_option_or_unknown_key_not_allowed() {
        use crate::menu::Unchosen;
        assert_eq!(chose(Ok("Chose \"1. Yes\" in hub@kebbie's prompt; the menu is gone.".to_string())), (Outcome::Done, "Chose \"1. Yes\" in hub@kebbie's prompt; the menu is gone.".to_string()));
        assert_eq!(chose(Err(Unchosen::Gone("The prompt 3f2a9c1d is not on the task's screen any more".to_string()))).0, Outcome::Refused { reason: Reason::AlreadySettled });
        assert_eq!(chose(Err(Unchosen::NotAllowed("\"3. Type something.\" takes the user's own words".to_string()))).0, Outcome::Refused { reason: Reason::NotAllowed });
        assert_eq!(chose(Err(Unchosen::Failed("could not confirm the menu went away".to_string()))).0, Outcome::Failed);
    }

    #[test]
    fn a_hub_answer_and_push_reach_the_browser_as_kebbies_own_words() {
        use crate::tasks::DevicePush;
        assert_eq!(pushed(DevicePush::Pushed { short: "9f3c2a1".to_string(), main: "main".to_string() }), (Outcome::Done, "Pushed 9f3c2a1 to main".to_string()));
        assert_eq!(pushed(DevicePush::Conflict { files: vec!["src/a.rs".to_string(), "b".to_string()], main: "main".to_string() }), (Outcome::Failed, "Not pushed yet: conflicts with main in src/a.rs, b; its Claude resolves it, then kebbie pushes it".to_string()));
        let refused = crate::notice::Notice::new(crate::notice::Tone::NotNow, "Not pushed: the review is still running").next("Push it once it is done.").to_string();
        assert_eq!(pushed(DevicePush::Refused(refused)), (Outcome::Refused { reason: Reason::NotAllowed }, "Not pushed: the review is still running. Push it once it is done.".to_string()));
        assert_eq!(pushed(DevicePush::UnknownTask("No task \"x\" in shop's worktrees".to_string())).0, Outcome::Refused { reason: Reason::UnknownRepo });
        let elsewhere = Answer { address: "refunds@shop".to_string(), question: "no-such-question".to_string(), choice: Some(0), comment: String::new() };
        assert_eq!(answered(&Caller::hub("acc_01"), &elsewhere).0, Outcome::Refused { reason: Reason::AlreadySettled });
    }
}
