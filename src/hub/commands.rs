use std::collections::BTreeMap;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kebbie_hub_api::{HubInfo, Inbox, ResultStored, ResultUpload};
use kebbie_hub_protocol::{AccountKey, Answer, AppliedCommands, CommandResult, Dropped, Envelope, Header, Id, Kind, Machine, Outcome, Payload, PromptAnswer, PushApproval, Reason, Refusal, authorize, seal};
use serde::{Deserialize, Serialize};

use super::http::{CALL, Client, HubError, LONG_POLL};
use super::identity::{self, Identity};
use crate::config::HubCommands;
use crate::notice::Notice;
use crate::permit::Caller;
use crate::sh::Result;

pub const WAIT: u64 = 25;
const LEDGER: &str = "applied.json";
const BRIDGE: &str = "commands.json";
const LOCK: &str = "commands.lock";
const IDLE: u64 = 60;
const MAX_BACKOFF: u64 = 300;
pub const STOPPED: &str = "kebbie stopped while applying it; check the board";

// A command marked started and never recorded was cut off mid-apply; it is answered Failed when it comes again, never applied twice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub from: Id,
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub applied: AppliedCommands,
    #[serde(default)]
    pub started: BTreeMap<Id, Started>,
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

// What hub.commands lets this machine take, as the board's accepts and authorize's gate.
pub fn accepts(policy: HubCommands) -> Vec<Kind> {
    match policy {
        HubCommands::AnswerAndPush => vec![Kind::Answer, Kind::Push, Kind::PromptAnswer],
        HubCommands::Answer => vec![Kind::Answer, Kind::PromptAnswer],
        HubCommands::Off => Vec::new(),
    }
}

// A kind hub.commands leaves out, named with what the machine takes and where the user can do it instead.
fn not_taken(kind: Kind, machine: &str, accepts: &[Kind]) -> String {
    let (refused, act, allow) = match kind {
        Kind::Push => ("Not pushed", "Push", "pushes"),
        Kind::PromptAnswer => ("Not chosen", "Choose", "answers"),
        Kind::Answer => ("Not answered", "Answer", "answers"),
        other => return format!("Not done: {machine} takes no {} from the hub", other.name()),
    };
    let takes = if accepts.is_empty() { "takes nothing from the hub" } else { "takes answers, not pushes" };
    format!("{refused}: {machine} {takes}. {act} at the machine, or allow {allow} in its kebbie settings.")
}

fn refusal_text(reason: Reason, kind: Kind, machine: &str, accepts: &[Kind]) -> String {
    match reason {
        Reason::Expired => "It expired before this machine read it; send it again".to_string(),
        Reason::NotAllowed => not_taken(kind, machine, accepts),
        Reason::Undecryptable => "This machine could not open it; reload the page and send it again".to_string(),
        Reason::BadRequest => "This machine could not read what was asked".to_string(),
        other => format!("Refused: {}", other.key()),
    }
}

fn result(command: &Id, (outcome, text): Done) -> CommandResult {
    CommandResult { command: command.clone(), outcome, text }
}

// One command from the inbox: checked against the pinned account and hub.commands, applied at most once, and its result recorded before it is sent.
pub fn handle(dir: &Path, identity: &Identity, account: &AccountKey, accepts: &[Kind], envelope: &Envelope, hands: &Hands, now: u64) -> Result<Handled> {
    let header = &envelope.header;
    let secret = identity.secret();
    let machine = Machine { id: &identity.machine, keys: &secret, account, accepts };
    let mut ledger = ledger(dir);
    ledger.applied.prune(now);
    ledger.started.retain(|_, started| started.at + AppliedCommands::KEEP_SECONDS > now);
    let handled = match authorize(&machine, &ledger.applied, envelope, now) {
        Err(Refusal::Drop(dropped)) => return Ok(Handled::Dropped(dropped)),
        Err(Refusal::Replay(result)) => return Ok(Handled::Replayed(result)),
        Err(Refusal::Answer(reason)) => Handled::Refused(result(&header.id, (Outcome::Refused { reason }, refusal_text(reason, header.kind, &identity.name, accepts)))),
        Ok(_) if ledger.started.get(&header.id).is_some_and(|started| started.from == header.from) => Handled::Refused(result(&header.id, (Outcome::Failed, STOPPED.to_string()))),
        Ok(payload) => {
            ledger.started.insert(header.id.clone(), Started { from: header.from.clone(), at: now });
            save_ledger(dir, &ledger)?;
            let caller = Caller::hub(account.account.as_str());
            let done = match &payload {
                Payload::Answer(answer) => (hands.answer)(&caller, answer),
                Payload::Push(push) => (hands.push)(&caller, push),
                Payload::PromptAnswer(prompt) => (hands.prompt)(&caller, prompt),
                _ => (Outcome::Refused { reason: Reason::NotAllowed }, not_taken(header.kind, &identity.name, accepts)),
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
pub fn round(dir: &Path, wait: u64, accepts: &[Kind], hands: &Hands, clock: &dyn Fn() -> u64) -> Result<Vec<String>> {
    let Some(identity) = identity::load_at(dir)? else { return Ok(Vec::new()) };
    let Some(account) = identity.account.as_ref().map(|account| account.key.clone()) else { return Ok(Vec::new()) };
    let _alone = Alone::take(dir).ok_or("another kebbie applies this machine's commands now")?;
    let mut bridge = bridge(dir);
    let polling = Client::new(&identity.url, Some(identity.token()), LONG_POLL);
    let inbox: Inbox = polling.get(&format!("/v1/m/inbox?after={}&wait={wait}", bridge.inbox_seq)).map_err(|error| error.to_string())?;
    let client = Client::new(&identity.url, Some(identity.token()), CALL);
    let mut notes = Vec::new();
    let mut applied = false;
    for message in inbox.messages.iter().filter(|message| message.envelope.header.kind.is_command()) {
        let header = &message.envelope.header;
        let handled = handle(dir, &identity, &account, accepts, &message.envelope, hands, clock())?;
        applied |= matches!(handled, Handled::Applied(_));
        match handled.result() {
            None => notes.push(format!("commands: dropped {} {} from {}", header.kind.name(), header.id, header.from)),
            Some(result) => {
                deliver(&client, &identity, &account, result)?;
                notes.push(format!("commands: {} {}: {}", header.kind.name(), header.id, result.text));
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
    let prompt = |caller: &Caller, answer: &PromptAnswer| chose(crate::menu::choose_as(caller, &answer.address, &answer.key, &answer.prompt));
    let hands = Hands { answer: &answered, push: &|caller, push| pushed(crate::tasks::push_as(caller, &push.address)), prompt: &prompt, relay: &relay };
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
        match round(dir, WAIT, &accepts(crate::config::hub_commands()), &hands, &crate::messages::now) {
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
        relays: Cell<u32>,
    }

    impl Seen {
        fn new() -> Seen {
            Seen { answers: RefCell::new(Vec::new()), pushes: RefCell::new(Vec::new()), prompts: RefCell::new(Vec::new()), relays: Cell::new(0) }
        }

        fn applied(&self) -> usize {
            self.answers.borrow().len() + self.pushes.borrow().len() + self.prompts.borrow().len()
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
        let relay = || seen.relays.set(seen.relays.get() + 1);
        run(&Hands { answer: &answer, push: &push, prompt: &prompt, relay: &relay })
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
            with_hands(seen, |hands| round(&self.dir, 0, &accepts(policy), hands, &now)).unwrap()
        }
    }

    #[test]
    fn the_accounts_answer_is_applied_once_as_the_user_through_the_hub_and_a_redelivery_only_resends_its_result() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let sent = account.send(&answer());
        account.round(&seen, HubCommands::AnswerAndPush);
        assert_eq!(seen.answers.borrow().as_slice(), [(Caller::hub(account.account.as_str()), Answer { address: "refunds@shop".to_string(), question: "q1".to_string(), choice: Some(0), comment: String::new() })]);
        assert_eq!(seen.relays.get(), 1, "the board goes out again at once");
        let result = account.result(&sent);
        assert_eq!((result.outcome, result.text.as_str()), (Outcome::Done, "Answered"));
        assert_eq!(ledger(&account.dir).applied.result(&account.account, &sent.header.id), Some(&result), "recorded before it was sent");

        let mut lost = bridge(&account.dir);
        lost.inbox_seq = 0;
        save_bridge(&account.dir, &lost).unwrap();
        let notes = account.round(&seen, HubCommands::AnswerAndPush);
        assert_eq!(seen.answers.borrow().len(), 1, "a command that comes again is never applied twice: {notes:?}");
        assert_eq!(seen.relays.get(), 1);

        let pushed = account.send(&push());
        account.round(&seen, HubCommands::AnswerAndPush);
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
        account.round(&seen, HubCommands::Answer);
        let refused = account.result(&pushed);
        assert_eq!((refused.outcome, refused.text.as_str()), (Outcome::Refused { reason: Reason::NotAllowed }, "Not pushed: mac takes answers, not pushes. Push at the machine, or allow pushes in its kebbie settings."));
        let answered = account.send(&answer());
        account.round(&seen, HubCommands::Answer);
        assert_eq!(account.result(&answered).outcome, Outcome::Done, "answers still go through");

        let off = account.send(&answer());
        account.round(&seen, HubCommands::Off);
        assert_eq!(account.result(&off).text, "Not answered: mac takes nothing from the hub. Answer at the machine, or allow answers in its kebbie settings.");
        assert!(seen.pushes.borrow().is_empty() && seen.answers.borrow().len() == 1, "only the allowed answer was applied");
    }

    #[test]
    fn an_expired_command_is_refused_and_nothing_is_applied() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let soon = account.browser.seal_command(&account.identity.machine, &answer(), now(), Some(60)).unwrap();
        account.hub.runtime.block_on(account.browser.send_command(&soon)).unwrap();
        with_hands(&seen, |hands| round(&account.dir, 0, &accepts(HubCommands::AnswerAndPush), hands, &|| now() + 120)).unwrap();
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
            let (push, prompt, relay) = (|_: &Caller, _: &PushApproval| unreachable!(), |_: &Caller, _: &PromptAnswer| unreachable!(), || {});
            round(&account.dir, 0, &accepts(HubCommands::AnswerAndPush), &Hands { answer: &answer, push: &push, prompt: &prompt, relay: &relay }, &now)
        }));
        assert!(crashed.is_err());
        assert_eq!(seen.answers.borrow().len(), 1, "applied once, then cut off before it was recorded");
        assert!(ledger(&account.dir).started.contains_key(&sent.header.id));
        account.round(&seen, HubCommands::AnswerAndPush);
        assert_eq!(seen.answers.borrow().len(), 1, "not applied a second time");
        let result = account.result(&sent);
        assert_eq!((result.outcome, result.text.as_str()), (Outcome::Failed, STOPPED));
        assert!(ledger(&account.dir).started.is_empty());
    }

    fn sealed_by(account: &AccountKey, keys: &SecretKeys, identity: &Identity, payload: &Payload) -> Envelope {
        let header = Header::new(Id::new(Prefix::Command).unwrap(), payload.kind(), account.account.clone(), identity.machine.clone(), now());
        seal(header, payload, keys, &[account.recipient(), (identity.machine.clone(), identity.public())]).unwrap()
    }

    #[test]
    fn a_forged_command_or_one_another_account_signed_is_dropped_and_nothing_is_applied() {
        let dir = crate::sh::test_dir("hub-commands");
        let identity = Identity::new("https://hub.example.com", Id::new(Prefix::Machine).unwrap(), "mac", "khm_token".to_string(), &SecretKeys::generate(), now());
        let (keys, other_keys) = (SecretKeys::generate(), SecretKeys::generate());
        let pinned = AccountKey::new(Id::account([3; 16]), 1, keys.public()).unwrap();
        let other = AccountKey::new(Id::account([4; 16]), 1, other_keys.public()).unwrap();
        let genuine = sealed_by(&pinned, &keys, &identity, &answer());
        let mut forged = genuine.clone();
        forged.sig = format!("{}{}", if forged.sig.starts_with('A') { 'B' } else { 'A' }, &forged.sig[1..]);
        let impostor = sealed_by(&pinned, &other_keys, &identity, &answer());
        let elsewhere = sealed_by(&other, &other_keys, &identity, &answer());
        let all = accepts(HubCommands::AnswerAndPush);
        let seen = Seen::new();
        with_hands(&seen, |hands| {
            assert_eq!(handle(&dir, &identity, &pinned, &all, &forged, hands, now()).unwrap(), Handled::Dropped(Dropped::BadSignature));
            assert_eq!(handle(&dir, &identity, &pinned, &all, &impostor, hands, now()).unwrap(), Handled::Dropped(Dropped::BadSignature), "the pinned account's id signed with another key");
            assert_eq!(handle(&dir, &identity, &pinned, &all, &elsewhere, hands, now()).unwrap(), Handled::Dropped(Dropped::WrongAccount), "a command another account signed");
            assert_eq!(seen.applied(), 0, "nothing was applied");
            assert!(ledger(&dir).applied.is_empty(), "a dropped command leaves no result");
            assert!(matches!(handle(&dir, &identity, &pinned, &all, &genuine, hands, now()).unwrap(), Handled::Applied(_)), "the genuine command from the pinned account goes through");
        });
        assert_eq!(seen.answers.borrow().len(), 1);
    }

    #[test]
    fn hub_commands_drives_what_the_machine_takes_and_names_what_it_refuses() {
        assert_eq!(accepts(HubCommands::AnswerAndPush), [Kind::Answer, Kind::Push, Kind::PromptAnswer]);
        assert_eq!(accepts(HubCommands::Answer), [Kind::Answer, Kind::PromptAnswer]);
        assert!(accepts(HubCommands::Off).is_empty());
        assert_eq!(accepts(HubCommands::default()), accepts(HubCommands::AnswerAndPush), "answer-and-push unless set");
        let off = accepts(HubCommands::Off);
        assert_eq!(not_taken(Kind::Push, "devbox", &off), "Not pushed: devbox takes nothing from the hub. Push at the machine, or allow pushes in its kebbie settings.");
        assert_eq!(not_taken(Kind::PromptAnswer, "devbox", &off), "Not chosen: devbox takes nothing from the hub. Choose at the machine, or allow answers in its kebbie settings.");
        assert_eq!(not_taken(Kind::Start, "devbox", &accepts(HubCommands::AnswerAndPush)), "Not done: devbox takes no start from the hub");
        assert_eq!(crate::hub::commands_line(HubCommands::Answer), "commands: answers and prompt choices; pushes off (hub.commands = \"answer\")");
        assert_eq!(crate::hub::commands_line(HubCommands::Off), "commands: off (hub.commands = \"off\")");
    }

    #[test]
    fn the_accounts_prompt_answer_is_applied_as_the_user_through_the_hub_and_refused_once_ten_minutes_passed() {
        let mut account = Account::connected();
        let seen = Seen::new();
        let sent = account.send(&prompt_answer("y"));
        assert_eq!(sent.header.exp - sent.header.at, 600, "a screen prompt is momentary");
        account.round(&seen, HubCommands::AnswerAndPush);
        assert_eq!(seen.prompts.borrow().as_slice(), [(Caller::hub(account.account.as_str()), PromptAnswer { address: "hub@kebbie".to_string(), prompt: "3f2a9c1d".to_string(), key: "y".to_string() })]);
        assert_eq!(account.result(&sent).text, "Chose it");
        assert_eq!(seen.relays.get(), 1, "the board goes out again at once");

        let late = account.send(&prompt_answer("esc"));
        with_hands(&seen, |hands| round(&account.dir, 0, &accepts(HubCommands::Answer), hands, &|| now() + 601)).unwrap();
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
