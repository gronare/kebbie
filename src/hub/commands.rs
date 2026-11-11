use std::collections::BTreeMap;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kebbie_hub_api::{DeviceListView, HubInfo, Inbox, ResultStored, ResultUpload};
use kebbie_hub_protocol::{AccountKey, Answer, AppliedCommands, CommandResult, DeviceList, Dropped, Envelope, Header, Id, Kind, ListedDevice, Machine, Outcome, Payload, PromptAnswer, PushApproval, Reason, Refusal, Role, TrustedDevice, authorize, seal};
use serde::{Deserialize, Serialize};

use super::http::{CALL, Client, HubError, LONG_POLL};
use super::identity::{self, Identity};
use crate::notice::Notice;
use crate::permit::Caller;
use crate::sh::Result;

pub const WAIT: u64 = 25;
const LEDGER: &str = "applied.json";
const BRIDGE: &str = "commands.json";
const LOCK: &str = "commands.lock";
pub const ACCEPTS: [Kind; 3] = [Kind::Answer, Kind::Push, Kind::PromptAnswer];
const IDLE: u64 = 60;
const MAX_BACKOFF: u64 = 300;
pub const STOPPED: &str = "kebbie stopped while applying it; check the board";
pub const DEVICE_KIND: &str = "hub-device";

// A command marked started and never recorded was cut off mid-apply; it is answered Failed when it comes again, never applied twice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub device: Id,
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub applied: AppliedCommands,
    #[serde(default)]
    pub started: BTreeMap<Id, Started>,
}

// hub_rev is the device list's counter as the hub last reported it, so a list kebbie refused is not fetched again until the hub holds another.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bridge {
    #[serde(default)]
    pub inbox_seq: u64,
    #[serde(default)]
    pub devices: DeviceList,
    #[serde(default)]
    pub hub_rev: Option<u64>,
}

pub type Done = (Outcome, String);

// What applying a command does, given as functions so tests drive the bridge without touching the user's tasks.
pub struct Hands<'a> {
    pub answer: &'a dyn Fn(&Caller, &Answer) -> Done,
    pub push: &'a dyn Fn(&Caller, &PushApproval) -> Done,
    pub prompt: &'a dyn Fn(&Caller, &PromptAnswer) -> Done,
    pub announce: &'a dyn Fn(&str),
    pub relay: &'a dyn Fn(),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Handled {
    Applied(CommandResult),
    Refused(CommandResult),
    Replayed(CommandResult),
    Dropped(Dropped),
}

impl Handled {
    pub fn result(&self) -> Option<&CommandResult> {
        match self {
            Handled::Applied(result) | Handled::Refused(result) | Handled::Replayed(result) => Some(result),
            Handled::Dropped(_) => None,
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
    load(&dir.join(LEDGER))
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

// Only a list the pinned account signed, at a counter above the one held, is taken; anything else leaves the held list as it is.
pub fn verified(account: &AccountKey, held: &DeviceList, view: &DeviceListView) -> std::result::Result<Option<DeviceList>, String> {
    let Some(signed) = &view.list else { return Ok(None) };
    if !account.verifies_device_list(&signed.list, &signed.signature.0) {
        return Err(format!("the hub's device list (counter {}) is not signed by this machine's account; kebbie keeps the list it trusts", signed.list.counter));
    }
    if signed.list.counter < held.counter {
        return Err(format!("the hub's device list went back from counter {} to {}; kebbie keeps the newer one", held.counter, signed.list.counter));
    }
    Ok((signed.list.counter > held.counter).then(|| signed.list.clone()))
}

fn can(listed: &ListedDevice) -> String {
    match listed.role {
        Role::Control => format!("'{}' can now answer and push here", listed.name),
        Role::Read => format!("'{}' is listed here read-only: it cannot answer or push", listed.name),
    }
}

fn cannot(listed: &ListedDevice) -> String {
    match listed.role {
        Role::Control => format!("'{}' can no longer answer and push here", listed.name),
        Role::Read => format!("'{}' is no longer listed here", listed.name),
    }
}

// Each device the new list adds, drops or changes the role of, as the line the board and the lead read.
pub fn changes(before: &DeviceList, after: &DeviceList) -> Vec<String> {
    let mut lines = Vec::new();
    for listed in &after.devices {
        match before.listed(&listed.device) {
            None => lines.push(can(listed)),
            Some(earlier) if earlier.role != listed.role && listed.role == Role::Control => lines.push(can(listed)),
            Some(earlier) if earlier.role != listed.role => lines.push(cannot(earlier)),
            Some(_) => {}
        }
    }
    lines.extend(before.devices.iter().filter(|earlier| after.listed(&earlier.device).is_none()).map(cannot));
    lines
}

fn refusal_text(reason: Reason) -> String {
    match reason {
        Reason::Expired => "It expired before this machine read it; send it again".to_string(),
        Reason::NotAllowed => "This device may read this machine but not answer or push here".to_string(),
        Reason::Undecryptable => "This machine could not open it; reload the page and send it again".to_string(),
        Reason::BadRequest => "This machine could not read what was asked".to_string(),
        other => format!("Refused: {}", other.key()),
    }
}

fn result(command: &Id, (outcome, text): Done) -> CommandResult {
    CommandResult { command: command.clone(), outcome, text }
}

// One command from the inbox: checked against the verified devices, applied at most once, and its result recorded before it is sent.
pub fn handle(dir: &Path, identity: &Identity, devices: &[TrustedDevice], envelope: &Envelope, hands: &Hands, now: u64) -> Result<Handled> {
    let header = &envelope.header;
    let secret = identity.secret();
    let machine = Machine { id: &identity.machine, keys: &secret, devices, accepts: &ACCEPTS };
    let mut ledger = ledger(dir);
    ledger.applied.prune(now);
    ledger.started.retain(|_, started| started.at + AppliedCommands::KEEP_SECONDS > now);
    let handled = match authorize(&machine, &ledger.applied, envelope, now) {
        Err(Refusal::Drop(dropped)) => return Ok(Handled::Dropped(dropped)),
        Err(Refusal::Replay(result)) => return Ok(Handled::Replayed(result)),
        Err(Refusal::Answer(reason)) => Handled::Refused(result(&header.id, (Outcome::Refused { reason }, refusal_text(reason)))),
        Ok(_) if ledger.started.get(&header.id).is_some_and(|started| started.device == header.from) => Handled::Refused(result(&header.id, (Outcome::Failed, STOPPED.to_string()))),
        Ok(accepted) => {
            ledger.started.insert(header.id.clone(), Started { device: header.from.clone(), at: now });
            save_ledger(dir, &ledger)?;
            let caller = Caller::Hub { device: accepted.device.id.to_string(), name: accepted.device.name.clone() };
            let done = match &accepted.payload {
                Payload::Answer(answer) => (hands.answer)(&caller, answer),
                Payload::Push(push) => (hands.push)(&caller, push),
                Payload::PromptAnswer(prompt) => (hands.prompt)(&caller, prompt),
                _ => (Outcome::Refused { reason: Reason::NotAllowed }, "This machine takes answers, pushes and prompt answers only".to_string()),
            };
            Handled::Applied(result(&header.id, done))
        }
    };
    if let Some(result) = handled.result() {
        ledger.applied.record(&header.from, now, result.clone());
    }
    ledger.started.remove(&header.id);
    save_ledger(dir, &ledger)?;
    Ok(handled)
}

fn sealed(identity: &Identity, device: &TrustedDevice, result: &CommandResult) -> Result<Envelope> {
    let header = Header::new(result.command.clone(), Kind::Result, identity.machine.clone(), identity.machine.clone(), crate::messages::now());
    seal(header, &Payload::Result(result.clone()), &identity.secret(), &[(device.id.clone(), device.keys), (identity.machine.clone(), identity.public())]).map_err(|error| error.to_string())
}

// A result the hub holds already, or for a command it has forgotten, needs no second post.
fn deliver(client: &Client, identity: &Identity, device: &TrustedDevice, result: &CommandResult) -> Result<()> {
    let envelope = sealed(identity, device, result)?;
    match client.post::<_, ResultStored>("/v1/m/results", &ResultUpload { envelope }) {
        Ok(_) => Ok(()),
        Err(HubError::Status { status: 404, code }) if code == "unknown-command" => Ok(()),
        Err(HubError::Status { status: 409, code }) if code == "result-differs" => Ok(()),
        Err(error) => Err(format!("the result of {} was not sent: {error}", result.command)),
    }
}

fn refresh(client: &Client, account: &AccountKey, bridge: &mut Bridge, rev: u64, hands: &Hands, notes: &mut Vec<String>) -> Result<()> {
    let view: DeviceListView = client.get("/v1/m/device-list").map_err(|error| error.to_string())?;
    match verified(account, &bridge.devices, &view) {
        Ok(Some(list)) => {
            for line in changes(&bridge.devices, &list) {
                (hands.announce)(&line);
                notes.push(format!("commands: {line}"));
            }
            bridge.devices = list;
        }
        Ok(None) => {}
        Err(refused) => notes.push(format!("commands: {refused}")),
    }
    bridge.hub_rev = Some(rev);
    Ok(())
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

// One long poll of the inbox and everything it brings; a keeper handing over to its successor holds the lock, so two never apply the same command.
pub fn round(dir: &Path, wait: u64, hands: &Hands, clock: &dyn Fn() -> u64) -> Result<Vec<String>> {
    let Some(identity) = identity::load_at(dir)? else { return Ok(Vec::new()) };
    let Some(account) = identity.account.as_ref().map(|account| account.key.clone()) else { return Ok(Vec::new()) };
    let _alone = Alone::take(dir).ok_or("another kebbie applies this machine's commands now")?;
    let mut bridge = bridge(dir);
    let held = bridge.hub_rev.unwrap_or(bridge.devices.counter);
    let polling = Client::new(&identity.url, Some(identity.token()), LONG_POLL);
    let inbox: Inbox = polling.get(&format!("/v1/m/inbox?after={}&wait={wait}&devices={held}", bridge.inbox_seq)).map_err(|error| error.to_string())?;
    let client = Client::new(&identity.url, Some(identity.token()), CALL);
    let mut notes = Vec::new();
    if let Some(rev) = inbox.device_list_rev.filter(|rev| bridge.hub_rev != Some(*rev)) {
        refresh(&client, &account, &mut bridge, rev, hands, &mut notes)?;
        save_bridge(dir, &bridge)?;
    }
    let devices = bridge.devices.trusted();
    let mut applied = false;
    for message in inbox.messages.iter().filter(|message| message.envelope.header.kind.is_command()) {
        let header = &message.envelope.header;
        let handled = handle(dir, &identity, &devices, &message.envelope, hands, clock())?;
        applied |= matches!(handled, Handled::Applied(_));
        match handled.result() {
            None => notes.push(format!("commands: dropped {} {} from {}", header.kind.name(), header.id, header.from)),
            Some(result) => {
                let device = devices.iter().find(|device| device.id == header.from).ok_or_else(|| format!("{} left the device list mid-round", header.from))?;
                deliver(&client, &identity, device, result)?;
                notes.push(format!("commands: {} {} from {}: {}", header.kind.name(), header.id, device.name, result.text));
            }
        }
    }
    bridge.inbox_seq = inbox.seq;
    save_bridge(dir, &bridge)?;
    if applied {
        (hands.relay)();
    }
    Ok(notes)
}

fn plain(text: &str) -> String {
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

fn announce(state: &Path, line: &str) {
    let _ = crate::messages::log_for_user_in(state, line, DEVICE_KIND);
    if let Err(error) = crate::messages::to_lead(crate::messages::ToLead::Permission, &format!("{line}, from the user's kebbie hub account.")) {
        eprintln!("Could not tell the lead: {error}");
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

// The keeper's thread for one identity directory: it idles while the machine has no pinned account or the hub takes no commands, and long-polls otherwise.
fn serve(state: &Path, dir: &Path) {
    let log = |text: &str| crate::keeper::log_line(state, text);
    let relay = || {
        if let Err(error) = super::tick() {
            log(&format!("commands: the board was not sent after a command: {error}"));
        }
    };
    let told = |line: &str| announce(state, line);
    let prompt = |caller: &Caller, answer: &PromptAnswer| chose(crate::menu::choose_as(caller, &answer.address, &answer.key, &answer.prompt));
    let hands = Hands { answer: &answered, push: &|caller, push| pushed(crate::tasks::push_as(caller, &push.address)), prompt: &prompt, announce: &told, relay: &relay };
    let mut failures = 0;
    let mut checked: Option<(bool, u64)> = None;
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
        match round(dir, WAIT, &hands, &crate::messages::now) {
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
}

// One thread per identity directory, the real hub's and the dev hub's, beside the keeper's routines, since a long poll would hold up their loop.
pub fn spawn(state: &Path) {
    for dev in [false, true] {
        let (kept, dir): (PathBuf, PathBuf) = (state.to_path_buf(), identity::dir_of(state, dev));
        let name = if dev { "hub-commands-dev" } else { "hub-commands" };
        if let Err(error) = std::thread::Builder::new().name(name.to_string()).spawn(move || serve(&kept, &dir)) {
            crate::keeper::log_line(state, &format!("commands: {name} did not start: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use kebbie_hub_api::{Bytes64, SignedDeviceList};
    use kebbie_hub_fakes::phone::PhoneState;
    use kebbie_hub_protocol::{Prefix, SecretKeys};

    use super::*;
    use crate::hub::tests::{TestHub, WAIT as LONG, connected_state, member, open_hub};

    fn now() -> u64 {
        crate::messages::now()
    }

    struct Seen {
        answers: RefCell<Vec<(Caller, Answer)>>,
        pushes: RefCell<Vec<(Caller, PushApproval)>>,
        prompts: RefCell<Vec<(Caller, PromptAnswer)>>,
        told: RefCell<Vec<String>>,
        relays: Cell<u32>,
    }

    impl Seen {
        fn new() -> Seen {
            Seen { answers: RefCell::new(Vec::new()), pushes: RefCell::new(Vec::new()), prompts: RefCell::new(Vec::new()), told: RefCell::new(Vec::new()), relays: Cell::new(0) }
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
        let announce = |line: &str| seen.told.borrow_mut().push(line.to_string());
        let relay = || seen.relays.set(seen.relays.get() + 1);
        run(&Hands { answer: &answer, push: &push, prompt: &prompt, announce: &announce, relay: &relay })
    }

    fn answer() -> Payload {
        Payload::Answer(Answer { address: "refunds@shop".to_string(), question: "q1".to_string(), choice: Some(0), comment: String::new() })
    }

    struct Phone {
        hub: TestHub,
        browser: kebbie_hub_fakes::browser::Browser,
        dir: PathBuf,
        identity: Identity,
        phone: PhoneState,
    }

    impl Phone {
        fn connected() -> Phone {
            let hub = open_hub(|config| config.commands = true);
            let mut browser = member(&hub);
            let state = connected_state(&hub, &mut browser);
            let dir = identity::dir(&state);
            let identity = identity::load_at(&dir).unwrap().unwrap();
            let mut phone = hub.runtime.block_on(browser.enable_device()).unwrap();
            phone.knows(&identity.machine, identity.public());
            Phone { hub, browser, dir, identity, phone }
        }

        fn send(&self, payload: &Payload) -> Envelope {
            self.hub.runtime.block_on(self.phone.command(&self.identity.machine, payload)).unwrap()
        }

        fn result(&self, envelope: &Envelope) -> CommandResult {
            self.hub.runtime.block_on(self.phone.await_result(&envelope.header.id, LONG)).unwrap().expect("a result reached the phone")
        }

        fn round(&self, seen: &Seen) -> Vec<String> {
            with_hands(seen, |hands| round(&self.dir, 0, hands, &now)).unwrap()
        }

        fn relist(&mut self, role: Option<Role>) {
            let device = self.phone.device.clone().unwrap();
            let held = self.hub.runtime.block_on(self.browser.device_list()).unwrap();
            let list = match role {
                Some(role) => held.with(ListedDevice { device, name: "browser".to_string(), keys: self.phone.public(), role }),
                None => held.without(&device),
            };
            self.hub.runtime.block_on(self.browser.publish_devices(&list)).unwrap();
        }
    }

    #[test]
    fn a_listed_phones_answer_is_applied_once_as_the_user_from_that_phone_and_a_redelivery_only_resends_its_result() {
        let mut phone = Phone::connected();
        let seen = Seen::new();
        phone.round(&seen);
        assert_eq!(seen.told.borrow().as_slice(), ["'browser' can now answer and push here"], "the board and the lead hear of the new device");
        let sent = phone.send(&answer());
        phone.round(&seen);
        let device = phone.phone.device.clone().unwrap();
        assert_eq!(seen.answers.borrow().as_slice(), [(Caller::Hub { device: device.to_string(), name: "browser".to_string() }, Answer { address: "refunds@shop".to_string(), question: "q1".to_string(), choice: Some(0), comment: String::new() })]);
        assert_eq!(seen.relays.get(), 1, "the board goes out again at once");
        let result = phone.result(&sent);
        assert_eq!((result.outcome, result.text.as_str()), (Outcome::Done, "Answered"));
        assert_eq!(ledger(&phone.dir).applied.result(&device, &sent.header.id), Some(&result), "recorded before it was sent");

        let mut lost = bridge(&phone.dir);
        lost.inbox_seq = 0;
        save_bridge(&phone.dir, &lost).unwrap();
        let notes = phone.round(&seen);
        assert_eq!(seen.answers.borrow().len(), 1, "a command that comes again is never applied twice: {notes:?}");
        assert_eq!(seen.relays.get(), 1);

        let push = phone.send(&Payload::Push(PushApproval { address: "refunds@shop".to_string(), head: "a".repeat(40), card_sha: None }));
        phone.round(&seen);
        assert_eq!(seen.pushes.borrow().len(), 1);
        assert_eq!(phone.result(&push).text, "Pushed 9f3c2a1 to main");

        phone.relist(None);
        phone.round(&seen);
        assert_eq!(seen.told.borrow().last().map(String::as_str), Some("'browser' can no longer answer and push here"));
        for file in [LEDGER, BRIDGE] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(phone.dir.join(file)).unwrap().permissions().mode() & 0o777, 0o600, "{file} is private");
        }
    }

    #[test]
    fn an_expired_command_or_one_from_a_read_only_phone_is_refused_and_nothing_is_applied() {
        let mut phone = Phone::connected();
        let seen = Seen::new();
        let soon = phone.phone.seal_command(&phone.identity.machine, &answer(), now(), Some(60)).unwrap();
        phone.hub.runtime.block_on(phone.phone.send_command(&soon)).unwrap();
        with_hands(&seen, |hands| round(&phone.dir, 0, hands, &|| now() + 120)).unwrap();
        assert_eq!(phone.result(&soon).outcome, Outcome::Refused { reason: Reason::Expired });

        phone.relist(Some(Role::Read));
        phone.round(&seen);
        assert_eq!(seen.told.borrow().last().map(String::as_str), Some("'browser' can no longer answer and push here"));
        let read = phone.send(&answer());
        phone.round(&seen);
        assert_eq!(phone.result(&read).outcome, Outcome::Refused { reason: Reason::NotAllowed });
        assert!(seen.answers.borrow().is_empty() && seen.relays.get() == 0, "nothing was applied");
    }

    #[test]
    fn a_command_cut_off_mid_apply_answers_failed_when_it_comes_again_and_is_never_applied_twice() {
        let phone = Phone::connected();
        let seen = Seen::new();
        phone.round(&seen);
        let sent = phone.send(&answer());
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let answer = |_: &Caller, _: &Answer| -> Done {
                seen.answers.borrow_mut().push((Caller::User, Answer { address: String::new(), question: String::new(), choice: None, comment: String::new() }));
                panic!("kebbie stopped here")
            };
            let (push, prompt, announce, relay) = (|_: &Caller, _: &PushApproval| unreachable!(), |_: &Caller, _: &PromptAnswer| unreachable!(), |_: &str| {}, || {});
            round(&phone.dir, 0, &Hands { answer: &answer, push: &push, prompt: &prompt, announce: &announce, relay: &relay }, &now)
        }));
        assert!(crashed.is_err());
        assert_eq!(seen.answers.borrow().len(), 1, "applied once, then cut off before it was recorded");
        assert!(ledger(&phone.dir).started.contains_key(&sent.header.id));
        phone.round(&seen);
        assert_eq!(seen.answers.borrow().len(), 1, "not applied a second time");
        let result = phone.result(&sent);
        assert_eq!((result.outcome, result.text.as_str()), (Outcome::Failed, STOPPED));
        assert!(ledger(&phone.dir).started.is_empty());
    }

    fn offline() -> (PathBuf, Identity, PhoneState, ListedDevice) {
        let dir = crate::sh::test_dir("hub-commands");
        let identity = Identity::new("https://hub.example.com", Id::new(Prefix::Machine).unwrap(), "mac", "khm_token".to_string(), &SecretKeys::generate(), now());
        let device = Id::new(Prefix::Device).unwrap();
        let mut phone = PhoneState::new("https://hub.example.com").signed_in(device.clone(), "khd_token");
        phone.knows(&identity.machine, identity.public());
        let listed = ListedDevice { device, name: "iPhone Duo".to_string(), keys: phone.public(), role: Role::Control };
        (dir, identity, phone, listed)
    }

    #[test]
    fn a_forged_command_or_one_from_a_phone_off_the_verified_list_is_dropped_and_nothing_is_applied() {
        let (dir, identity, phone, listed) = offline();
        let seen = Seen::new();
        let sealed = phone.seal_command(&identity.machine, &answer(), now(), None).unwrap();
        let mut forged = sealed.clone();
        forged.sig = format!("{}{}", if forged.sig.starts_with('A') { 'B' } else { 'A' }, &forged.sig[1..]);
        let devices = [listed.trusted()];
        with_hands(&seen, |hands| {
            assert_eq!(handle(&dir, &identity, &devices, &forged, hands, now()).unwrap(), Handled::Dropped(Dropped::BadSignature));
            assert_eq!(handle(&dir, &identity, &[], &sealed, hands, now()).unwrap(), Handled::Dropped(Dropped::UnknownDevice));
            assert!(matches!(handle(&dir, &identity, &devices, &sealed, hands, now()).unwrap(), Handled::Applied(_)), "the genuine command from a listed phone goes through");
        });
        assert_eq!(seen.answers.borrow().len(), 1);
    }

    #[test]
    fn only_a_list_the_pinned_account_signed_at_a_higher_counter_is_trusted_and_its_changes_are_told() {
        let (_, _, _, listed) = offline();
        let account_keys = SecretKeys::generate();
        let account = AccountKey::new(Id::account([3; 16]), 1, account_keys.public()).unwrap();
        let signed = |list: &DeviceList, keys: &SecretKeys| DeviceListView { list: Some(SignedDeviceList { list: list.clone(), signature: Bytes64(list.sign(keys, &account.account).unwrap()) }) };
        let first = DeviceList::default().with(listed.clone());
        assert_eq!(verified(&account, &DeviceList::default(), &signed(&first, &account_keys)), Ok(Some(first.clone())));
        assert!(verified(&account, &DeviceList::default(), &signed(&first, &SecretKeys::generate())).unwrap_err().contains("not signed by this machine's account"), "another account's list");
        let second = first.with(ListedDevice { role: Role::Read, ..listed.clone() });
        assert!(verified(&account, &second, &signed(&first, &account_keys)).unwrap_err().contains("went back"), "a lower counter");
        assert_eq!(verified(&account, &first, &signed(&first, &account_keys)), Ok(None), "the same list changes nothing");
        assert_eq!(verified(&account, &first, &DeviceListView { list: None }), Ok(None));
        assert_eq!(changes(&DeviceList::default(), &first), ["'iPhone Duo' can now answer and push here"]);
        assert_eq!(changes(&first, &second), ["'iPhone Duo' can no longer answer and push here"]);
        assert_eq!(changes(&second, &second.with(listed.clone())), ["'iPhone Duo' can now answer and push here"]);
        assert_eq!(changes(&first, &first.without(&listed.device)), ["'iPhone Duo' can no longer answer and push here"]);
    }

    fn prompt_answer(key: &str) -> Payload {
        Payload::PromptAnswer(PromptAnswer { address: "hub@kebbie".to_string(), prompt: "3f2a9c1d".to_string(), key: key.to_string() })
    }

    #[test]
    fn a_listed_phones_prompt_answer_is_applied_as_the_user_from_that_phone_and_refused_once_ten_minutes_passed() {
        let phone = Phone::connected();
        let seen = Seen::new();
        phone.round(&seen);
        let sent = phone.send(&prompt_answer("y"));
        assert_eq!(sent.header.exp - sent.header.at, 600, "a screen prompt is momentary");
        phone.round(&seen);
        let device = phone.phone.device.clone().unwrap();
        assert_eq!(seen.prompts.borrow().as_slice(), [(Caller::Hub { device: device.to_string(), name: "browser".to_string() }, PromptAnswer { address: "hub@kebbie".to_string(), prompt: "3f2a9c1d".to_string(), key: "y".to_string() })]);
        assert_eq!(phone.result(&sent).text, "Chose it");
        assert_eq!(seen.relays.get(), 1, "the board goes out again at once");

        let late = phone.send(&prompt_answer("esc"));
        with_hands(&seen, |hands| round(&phone.dir, 0, hands, &|| now() + 601)).unwrap();
        assert_eq!(phone.result(&late).outcome, Outcome::Refused { reason: Reason::Expired });
        assert_eq!(seen.prompts.borrow().len(), 1, "nothing chosen after ten minutes");
        assert!(ACCEPTS.contains(&Kind::PromptAnswer));
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
    fn a_devices_answer_and_push_reach_the_phone_as_kebbies_own_words() {
        use crate::tasks::DevicePush;
        assert_eq!(pushed(DevicePush::Pushed { short: "9f3c2a1".to_string(), main: "main".to_string() }), (Outcome::Done, "Pushed 9f3c2a1 to main".to_string()));
        assert_eq!(pushed(DevicePush::Conflict { files: vec!["src/a.rs".to_string(), "b".to_string()], main: "main".to_string() }), (Outcome::Failed, "Not pushed yet: conflicts with main in src/a.rs, b; its Claude resolves it, then kebbie pushes it".to_string()));
        let refused = crate::notice::Notice::new(crate::notice::Tone::NotNow, "Not pushed: the review is still running").next("Push it once it is done.").to_string();
        assert_eq!(pushed(DevicePush::Refused(refused)), (Outcome::Refused { reason: Reason::NotAllowed }, "Not pushed: the review is still running. Push it once it is done.".to_string()));
        assert_eq!(pushed(DevicePush::UnknownTask("No task \"x\" in shop's worktrees".to_string())).0, Outcome::Refused { reason: Reason::UnknownRepo });
        let phone = Caller::Hub { device: "dev_01".to_string(), name: "iPhone Duo".to_string() };
        let elsewhere = Answer { address: "refunds@shop".to_string(), question: "no-such-question".to_string(), choice: Some(0), comment: String::new() };
        assert_eq!(answered(&phone, &elsewhere).0, Outcome::Refused { reason: Reason::AlreadySettled });
    }
}
