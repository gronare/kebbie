use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use kebbie_hub_api::{NoticePosted, NoticeUpload, NotifiedDevice, NotifiedDevices};
use kebbie_hub_protocol::{AccountKey, Board, Envelope, Header, Id, ItemKey, Kind, MAX_NOTICE_BODY_CHARS, MAX_NOTICE_TITLE_CHARS, MAX_RETRACTS, Notice, NoticeCategory, Payload, Prefix, PublicKeys, Target, Trust, Urgency, seal};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::blobs::Attachment;
use super::devices::{self, Locked, Policy};
use super::http::{CALL, Client, HubError};
use super::identity::{self, Identity};
use crate::sh::Result;

pub const DELIVER: &str = "--hub";
pub const FROM_DEVICE: &str = "KEBBIE_FROM_DEVICE";
const FILE: &str = "notices.json";
const LOCK: &str = "notices.lock";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Question,
    Prompt,
    Item,
    Device,
    Ready,
    Ci,
    Started,
}

impl Subject {
    fn prefix(self) -> &'static str {
        match self {
            Subject::Question => "q",
            Subject::Prompt => "p",
            Subject::Item => "i",
            Subject::Device => "d",
            Subject::Ready => "r",
            Subject::Ci => "c",
            Subject::Started => "s",
        }
    }
}

// The hub sees the tag, so it is a digest of the machine and the subject, never the subject itself.
pub fn tag(machine: &Id, subject: Subject, name: &str) -> String {
    let digest = Sha256::new().chain_update(machine.as_str()).chain_update([0]).chain_update(subject.prefix()).chain_update([0]).chain_update(name).finalize();
    format!("{}-{}", subject.prefix(), digest[..12].iter().map(|byte| format!("{byte:02x}")).collect::<String>())
}

// What a notice is about, with what kebbie knows of it; the machine's name is filled in where it is sent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "about", rename_all = "kebab-case")]
pub enum About {
    Question { address: String, text: String, recommended: Option<String> },
    Prompt { address: String, said: String },
    ItemQuestions { item: ItemKey, title: String, first: String, count: usize },
    NewDevice { device: Id, name: String, added: String, from_devices: bool },
    Ready { address: String, does: String, commits: usize, review: Option<String> },
    CiFailed { address: String, what: String, done: Option<String> },
    Started { item: ItemKey, title: String, address: String, recipe: Option<String> },
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { format!("{}…", text.chars().take(max - 1).collect::<String>().trim_end()) }
}

fn sentence(text: &str) -> String {
    let text = one_line(text);
    let text = text.trim_end_matches([',', ';']);
    if text.is_empty() || text.ends_with(['.', '?', '!', ':', '…']) { text.to_string() } else { format!("{text}.") }
}

fn task_title(address: &str) -> String {
    match address.split_once('@') {
        Some((task, repo)) => format!("{task} · {repo}"),
        None => address.to_string(),
    }
}

fn lowered(text: &str) -> String {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && !second.is_uppercase() => first.to_lowercase().chain(text.chars().skip(1)).collect(),
        _ => text.to_string(),
    }
}

impl About {
    pub fn category(&self) -> NoticeCategory {
        match self {
            About::Question { .. } | About::Prompt { .. } => NoticeCategory::Questions,
            About::ItemQuestions { .. } => NoticeCategory::IdeaQuestions,
            About::NewDevice { .. } => NoticeCategory::NewDevice,
            About::Ready { .. } => NoticeCategory::Ready,
            About::CiFailed { .. } => NoticeCategory::CiFailed,
            About::Started { .. } => NoticeCategory::Started,
        }
    }

    pub fn subject(&self) -> (Subject, String) {
        match self {
            About::Question { address, .. } => (Subject::Question, address.clone()),
            About::Prompt { address, .. } => (Subject::Prompt, address.clone()),
            About::ItemQuestions { item, .. } => (Subject::Item, item.as_str().to_string()),
            About::NewDevice { device, .. } => (Subject::Device, device.as_str().to_string()),
            About::Ready { address, .. } => (Subject::Ready, address.clone()),
            About::CiFailed { address, .. } => (Subject::Ci, address.clone()),
            About::Started { item, .. } => (Subject::Started, item.as_str().to_string()),
        }
    }

    fn urgency(&self) -> Urgency {
        match self {
            About::Question { .. } | About::Prompt { .. } | About::NewDevice { .. } => Urgency::High,
            _ => Urgency::Normal,
        }
    }

    pub fn title(&self, machine: &str) -> String {
        let title = match self {
            About::Question { address, .. } | About::Prompt { address, .. } => task_title(address),
            About::ItemQuestions { title, .. } => title.clone(),
            About::NewDevice { name, .. } => format!("{name} wants to make changes on {machine}"),
            About::Ready { address, .. } => format!("{} is ready to push", task_title(address)),
            About::CiFailed { address, .. } => format!("CI failed on {}", task_title(address)),
            About::Started { title, .. } => format!("Started: {title}"),
        };
        cut(&one_line(&title), MAX_NOTICE_TITLE_CHARS)
    }

    pub fn body(&self, machine: &str) -> String {
        let body = match self {
            About::Question { text, recommended, .. } => match recommended {
                Some(recommended) => format!("{} The lead recommends {}", sentence(text), sentence(&lowered(recommended))),
                None => sentence(text),
            },
            About::Prompt { said, .. } => sentence(said),
            About::ItemQuestions { first, count, .. } if *count > 1 => format!("{} (1 of {count})", one_line(first)),
            About::ItemQuestions { first, .. } => one_line(first),
            About::NewDevice { added, from_devices, .. } => {
                let more = if *from_devices { " Or tap to approve it from this device." } else { "" };
                format!("Added {added}. Approve or deny it in kebbie: N on the board.{more}")
            }
            About::Ready { does, commits, review, .. } => {
                let commits = if *commits == 1 { "1 commit".to_string() } else { format!("{commits} commits") };
                match review {
                    Some(review) => format!("{} {commits}, {}", sentence(does), sentence(review)),
                    None => format!("{} {commits}.", sentence(does)),
                }
            }
            About::CiFailed { what, done, .. } => match done {
                Some(done) => format!("{} {}", sentence(what), sentence(done)),
                None => sentence(what),
            },
            About::Started { address, recipe, .. } => {
                let task = address.split_once('@').map_or(address.as_str(), |(task, _)| task);
                match recipe {
                    Some(recipe) => format!("On {machine} as {task}, recipe {recipe}."),
                    None => format!("On {machine} as {task}."),
                }
            }
        };
        cut(&body, MAX_NOTICE_BODY_CHARS)
    }

    pub fn open(&self) -> Target {
        match self {
            About::Question { address, .. } | About::Prompt { address, .. } | About::Ready { address, .. } | About::CiFailed { address, .. } | About::Started { address, .. } => Target::Task { address: address.clone() },
            About::ItemQuestions { item, .. } => Target::Item { item: item.clone() },
            About::NewDevice { device, .. } => Target::Device { device: device.clone() },
        }
    }

    pub fn notice(&self, machine: &str, retract: Vec<String>) -> Notice {
        Notice { category: self.category(), title: self.title(machine), body: self.body(machine), open: self.open(), retract }
    }
}

fn short_option(label: &str) -> String {
    label.split([',', '(']).next().unwrap_or(label).trim().to_string()
}

// What an agent's own prompt asks, as the phone says it: the command a permission names, then the options in short.
pub fn prompt_said(agent: &str, menu: &crate::menu::Menu) -> String {
    let lines: Vec<&str> = menu.question.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let command = match lines.as_slice() {
        [first, command, ..] if first.ends_with("command") => Some(command.trim_start_matches("$ ").to_string()),
        _ => lines.iter().find_map(|line| line.strip_prefix("$ ")).map(str::to_string),
    };
    let asks = match command {
        Some(command) => format!("{agent} asks to run {command}."),
        None => format!("{agent} asks: {}", sentence(&lines.join(" "))),
    };
    let mut options: Vec<String> = Vec::new();
    for option in menu.options.iter().map(|label| short_option(label)).filter(|label| !label.is_empty()) {
        if !options.contains(&option) {
            options.push(option);
        }
    }
    let choices = match options.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!(" {} or {last}.", rest.join(", ")),
        Some((only, _)) => format!(" {only}."),
        None => String::new(),
    };
    format!("{asks}{choices}")
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub sent: BTreeMap<String, BTreeSet<Id>>,
    #[serde(default)]
    pub outstanding: BTreeMap<Id, Vec<String>>,
    #[serde(default)]
    pub items: Option<BTreeMap<String, String>>,
}

pub fn ledger(dir: &Path) -> Ledger {
    std::fs::read_to_string(dir.join(FILE)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn change<T>(dir: &Path, change: impl FnOnce(&mut Ledger) -> Result<T>) -> Result<T> {
    let _locked = Locked::wait(dir, LOCK)?;
    let before = ledger(dir);
    let mut ledger = before.clone();
    let done = change(&mut ledger)?;
    if ledger != before {
        crate::state::write(&dir.join(FILE), serde_json::to_string_pretty(&ledger).map_err(|error| error.to_string())?)?;
    }
    Ok(done)
}

// A subject settled elsewhere: each device that got its notice has the tag retracted with the next notice it gets, never with a push of its own.
pub fn settle_in(dir: &Path, subject: Subject, name: &str) -> Result<()> {
    let Some(identity) = identity::load_at(dir)? else { return Ok(()) };
    let tag = tag(&identity.machine, subject, name);
    if !ledger(dir).sent.contains_key(&tag) {
        return Ok(());
    }
    change(dir, |ledger| {
        for device in ledger.sent.remove(&tag).unwrap_or_default() {
            let outstanding = ledger.outstanding.entry(device).or_default();
            if !outstanding.contains(&tag) {
                outstanding.push(tag.clone());
            }
        }
        Ok(())
    })
}

pub fn settle(subject: Subject, name: &str) {
    let Ok(state) = crate::lead::state_dir() else { return };
    for dev in [false, true] {
        let _ = settle_in(&identity::dir_of(&state, dev), subject, name);
    }
}

// The devices a notice goes to: certified by the pinned account, subscribed to its category, approved on this machine, and not the one whose act it reports.
pub fn recipients(account: &AccountKey, known: &devices::Devices, policy: Policy, listed: &[NotifiedDevice], category: NoticeCategory, except: Option<&Id>, now: u64) -> Vec<(Id, PublicKeys)> {
    listed.iter()
        .filter(|listed| listed.notifications.as_ref().is_some_and(|categories| categories.contains(&category)))
        .filter(|listed| account.verifies_device(&listed.cert) && Some(&listed.cert.device) != except)
        .filter(|listed| known.trust_of(policy, &listed.cert.device, &listed.cert.signing, now) == Trust::Approved)
        .map(|listed| listed.cert.recipient())
        .collect()
}

// Dropped is a notice the hub refuses as it stands, so sending it again would be refused again; Later is one it could take another time.
#[derive(Clone, Debug, PartialEq)]
pub enum Unposted {
    Dropped(String),
    Later(String),
}

pub fn unposted(error: HubError) -> Unposted {
    match error.status() {
        Some(400 | 409 | 413) => Unposted::Dropped(format!("the hub refused the notice: {error}")),
        _ => Unposted::Later(format!("the notice was not posted: {error}")),
    }
}

pub struct Wire<'a> {
    pub listed: &'a dyn Fn() -> Result<Vec<NotifiedDevice>>,
    pub post: &'a dyn Fn(&Envelope) -> std::result::Result<usize, Unposted>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Delivered {
    pub reached: usize,
    pub pushes: usize,
    pub problems: Vec<String>,
}

// One sealed notice per set of tags to retract, so each device's retracts ride with it and the rest share one envelope; a refused envelope is logged and dropped with its retracts, never sent again.
#[allow(clippy::too_many_arguments)]
pub fn deliver_in(dir: &Path, identity: &Identity, account: &AccountKey, about: &About, except: Option<&Id>, policy: Policy, wire: &Wire, now: u64) -> Result<Delivered> {
    let listed = (wire.listed)()?;
    let to = recipients(account, &devices::load(dir), policy, &listed, about.category(), except, now);
    if to.is_empty() {
        return Ok(Delivered::default());
    }
    let (subject, name) = about.subject();
    let tag = tag(&identity.machine, subject, &name);
    change(dir, |ledger| {
        let mut groups: BTreeMap<Vec<String>, Vec<(Id, PublicKeys)>> = BTreeMap::new();
        for (device, keys) in to {
            let retract: Vec<String> = ledger.outstanding.get(&device).into_iter().flatten().filter(|old| **old != tag).take(MAX_RETRACTS).cloned().collect();
            groups.entry(retract).or_default().push((device, keys));
        }
        let mut delivered = Delivered::default();
        for (retract, group) in groups {
            let payload = Payload::Notice(about.notice(&identity.name, retract.clone()));
            if let Err(problem) = payload.validate() {
                delivered.problems.push(format!("the {} notice was not sent: {problem}", about.category().name()));
                continue;
            }
            let mut header = Header::new(Id::new(Prefix::Notice).map_err(|error| error.to_string())?, Kind::Notice, identity.machine.clone(), identity.machine.clone(), now);
            header.collapse = Some(tag.clone());
            header.urgency = Some(about.urgency());
            let envelope = seal(header, &payload, &identity.secret(), &group).map_err(|error| error.to_string())?;
            let sent = match (wire.post)(&envelope) {
                Ok(pushes) => {
                    delivered.pushes += pushes;
                    delivered.reached += group.len();
                    true
                }
                Err(Unposted::Dropped(problem)) => {
                    delivered.problems.push(problem);
                    false
                }
                Err(Unposted::Later(problem)) => {
                    delivered.problems.push(problem);
                    continue;
                }
            };
            for (device, _) in &group {
                if sent {
                    ledger.sent.entry(tag.clone()).or_default().insert(device.clone());
                }
                if let Some(outstanding) = ledger.outstanding.get_mut(device) {
                    outstanding.retain(|old| !retract.contains(old) && (!sent || *old != tag));
                }
            }
            ledger.outstanding.retain(|_, outstanding| !outstanding.is_empty());
        }
        Ok(delivered)
    })
}

pub fn listed_from(identity: &Identity) -> Result<Vec<NotifiedDevice>> {
    Client::new(&identity.url, Some(identity.token()), CALL).get::<NotifiedDevices>("/v1/m/devices").map(|listing| listing.devices).map_err(|error| format!("the hub's device list: {error}"))
}

pub fn post_to(identity: &Identity, envelope: &Envelope) -> std::result::Result<usize, Unposted> {
    Client::new(&identity.url, Some(identity.token()), CALL).post::<_, NoticePosted>("/v1/m/notices", &NoticeUpload { envelope: envelope.clone() }).map(|posted| posted.pushes).map_err(unposted)
}

fn dirs(state: &Path, only: Option<&Path>) -> Vec<PathBuf> {
    match only {
        Some(dir) => vec![dir.to_path_buf()],
        None => vec![identity::dir_of(state, false), identity::dir_of(state, true)],
    }
}

// The detached half: argv after `notify --hub` is the notice as JSON, then --except <device> and --dir <identity directory> when given.
pub fn deliver(argv: &[String]) {
    let Ok(state) = crate::lead::state_dir() else { return };
    let log = |text: &str| crate::notify::log_error(&state, &format!("hub notice: {text}"));
    let Some(about) = argv.first().and_then(|json| serde_json::from_str::<About>(json).ok()) else { return log("unreadable") };
    let flag = |name: &str| argv.iter().position(|arg| arg == name).and_then(|at| argv.get(at + 1)).cloned();
    let except = flag("--except").and_then(|id| Id::parse(&id).ok());
    let only = flag("--dir").map(PathBuf::from);
    let now = crate::messages::now();
    for dir in dirs(&state, only.as_deref()) {
        let Ok(Some(identity)) = identity::load_at(&dir) else { continue };
        let Some(account) = identity.account.as_ref().map(|account| account.key.clone()) else { continue };
        let wire = Wire { listed: &|| listed_from(&identity), post: &|envelope| post_to(&identity, envelope) };
        match deliver_in(&dir, &identity, &account, &about, except.as_ref(), Policy::current(), &wire, now) {
            Ok(delivered) => delivered.problems.iter().for_each(|problem| log(problem)),
            Err(error) => log(&error),
        }
    }
}

#[cfg(test)]
thread_local! {
    pub static SENT: std::cell::RefCell<Vec<(About, Option<Id>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg_attr(test, allow(dead_code))]
fn connected(state: &Path, only: Option<&Path>) -> bool {
    dirs(state, only).iter().any(|dir| identity::load_at(dir).ok().flatten().is_some_and(|identity| identity.account.is_some()))
}

fn dispatch(about: &About, except: Option<&Id>, only: Option<&Path>) {
    #[cfg(test)]
    {
        let _ = only;
        SENT.with(|sent| sent.borrow_mut().push((about.clone(), except.cloned())));
    }
    #[cfg(not(test))]
    {
        let Ok(state) = crate::lead::state_dir() else { return };
        if !connected(&state, only) {
            return;
        }
        let (Ok(kebbie), Ok(json)) = (crate::peers::own_exe(), serde_json::to_string(about)) else { return };
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let mut command = Command::new(kebbie);
        command.args(["notify", DELIVER, &json]);
        if let Some(except) = except {
            command.args(["--except", except.as_str()]);
        }
        if let Some(only) = only {
            command.arg("--dir").arg(only);
        }
        match command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn() {
            Ok(child) => { crate::sh::reap(child); }
            Err(error) => crate::notify::log_error(&state, &format!("hub notice: {error}")),
        }
    }
}

// Never fails its caller: a machine on no hub sends nothing, and a notice that fails lands in notify.log.
pub fn send(about: &About, except: Option<&Id>) {
    dispatch(about, except, None);
}

pub fn send_in(dir: &Path, about: &About) {
    dispatch(about, None, Some(dir));
}

pub fn from_device() -> Option<Id> {
    std::env::var(FROM_DEVICE).ok().and_then(|id| Id::parse(&id).ok()).filter(|id| id.prefix() == Prefix::Device)
}

// A backlog item started as a task: its questions are settled, and the devices that did not start it hear where it runs.
pub fn item_started(address: &str, reference: &str) {
    let root = crate::vault::root();
    let Some(entry) = crate::backlog::load().entries.into_iter().find(|entry| entry.reference == reference) else { return };
    let Some(item) = super::backlog::key_of(&entry, root.as_deref()) else { return };
    settle(Subject::Item, item.as_str());
    let recipe = crate::lead::state_dir().ok().and_then(|state| crate::recipe::load(&state, address)).map(|recipe| crate::recipe::shown_name(&recipe.name).to_string());
    send(&About::Started { item, title: entry.title, address: address.to_string(), recipe }, from_device().as_ref());
}

#[derive(Deserialize)]
struct Questions {
    #[serde(default)]
    questions: Vec<kebbie_hub_protocol::OpenQuestion>,
}

// The lead's questions on each item, as the board just gathered them: new ones are told, and an item whose questions are all answered settles; the first look only records them.
pub fn items_in(dir: &Path, board: &Board, attachments: &[Attachment]) -> Result<Vec<About>> {
    let mut asking: BTreeMap<String, (ItemKey, String, Vec<String>)> = BTreeMap::new();
    for row in board.backlog.iter().flatten() {
        asking.insert(row.item.as_str().to_string(), (row.item.clone(), row.title.clone(), Vec::new()));
    }
    for attachment in attachments {
        let Attachment::Backlog { item, text } = attachment else { continue };
        let Some((_, _, waiting)) = asking.get_mut(item.as_str()) else { continue };
        let questions = serde_json::from_str::<Questions>(text).map(|body| body.questions).unwrap_or_default();
        waiting.extend(questions.into_iter().filter(|question| question.blocks && question.pending_answer.is_none()).map(|question| question.text));
    }
    let print = |waiting: &[String]| sha1_smol::Sha1::from(waiting.join("\n")).digest().to_string();
    let (told, settled) = change(dir, |ledger| {
        let first = ledger.items.is_none();
        let seen = ledger.items.get_or_insert_with(BTreeMap::new);
        let mut told = Vec::new();
        for (key, (item, title, waiting)) in &asking {
            if waiting.is_empty() {
                continue;
            }
            let now = print(waiting);
            if seen.get(key) != Some(&now) && !first {
                told.push(About::ItemQuestions { item: item.clone(), title: title.clone(), first: waiting[0].clone(), count: waiting.len() });
            }
            seen.insert(key.clone(), now);
        }
        let settled: Vec<String> = seen.keys().filter(|key| asking.get(*key).is_none_or(|(_, _, waiting)| waiting.is_empty())).cloned().collect();
        seen.retain(|key, _| !settled.contains(key));
        Ok((told, settled))
    })?;
    for key in settled {
        settle_in(dir, Subject::Item, &key)?;
    }
    Ok(told)
}

#[cfg(test)]
mod tests {
    use kebbie_hub_api::Bytes32;
    use kebbie_hub_protocol::{BacklogBody, BacklogRow, BacklogState, DeviceCert, DeviceKey, DeviceKind, Home, OpenQuestion, Priority, SecretKeys, open};

    use super::*;
    use crate::config::{AutoApprove, HubTrust};
    use crate::hub::devices::{Device, State};

    const DEVICE: Policy = Policy { trust: HubTrust::Device, auto: AutoApprove::Never };

    fn item(path: &str) -> ItemKey {
        ItemKey::vault(path).unwrap()
    }

    fn texts(about: &About) -> (String, String) {
        (about.title("MacBook"), about.body("MacBook"))
    }

    #[test]
    fn each_notice_says_what_it_is_about_in_its_title_and_the_thing_itself_in_its_body() {
        let asked = crate::questions::parse(&["Per invoice: one rounding, on the total", "Per line: matches the old export"], Some("1"), false, "Round VAT per invoice or per line?").unwrap();
        let question = crate::questions::question_notice("billing@greenhouse", &asked, false);
        assert_eq!(texts(&question), ("billing · greenhouse".to_string(), "Round VAT per invoice or per line? The lead recommends per invoice: one rounding, on the total.".to_string()));
        assert_eq!(texts(&crate::questions::question_notice("billing@greenhouse", &asked, true)).1, "Round VAT per invoice or per line?", "a plan's approve is no recommendation worth a push");
        let prompt = About::Prompt { address: "wt3@kebbie".to_string(), said: "Claude asks to run cargo test --workspace. Yes or No.".to_string() };
        assert_eq!(texts(&prompt), ("wt3 · kebbie".to_string(), "Claude asks to run cargo test --workspace. Yes or No.".to_string()));
        let questions = About::ItemQuestions { item: item("Projects/retry.md"), title: "Retry flaky red runs".to_string(), first: "Which red runs count as flaky?".to_string(), count: 3 };
        assert_eq!(texts(&questions), ("Retry flaky red runs".to_string(), "Which red runs count as flaky? (1 of 3)".to_string()));
        assert_eq!(texts(&About::ItemQuestions { item: item("Projects/retry.md"), title: "Retry flaky red runs".to_string(), first: "Which red runs count as flaky?".to_string(), count: 1 }).1, "Which red runs count as flaky?");
        let device = Id::ulid(Prefix::Device, 1, [2; 10]).unwrap();
        let new = About::NewDevice { device: device.clone(), name: "iPhone 16".to_string(), added: "14:02".to_string(), from_devices: false };
        assert_eq!(texts(&new), ("iPhone 16 wants to make changes on MacBook".to_string(), "Added 14:02. Approve or deny it in kebbie: N on the board.".to_string()));
        assert_eq!(texts(&About::NewDevice { device: device.clone(), name: "iPhone 16".to_string(), added: "14:02".to_string(), from_devices: true }).1, "Added 14:02. Approve or deny it in kebbie: N on the board. Or tap to approve it from this device.");
        let ready = About::Ready { address: "cache@api".to_string(), does: "Caches the price lookup for an hour".to_string(), commits: 3, review: Some("no review: two lines in one file".to_string()) };
        assert_eq!(texts(&ready), ("cache · api is ready to push".to_string(), "Caches the price lookup for an hour. 3 commits, no review: two lines in one file.".to_string()));
        assert_eq!(texts(&About::Ready { address: "cache@api".to_string(), does: "Caches the price lookup for an hour".to_string(), commits: 1, review: None }).1, "Caches the price lookup for an hour. 1 commit.");
        let red = About::CiFailed { address: "web@app".to_string(), what: "ci · e2e · Run playwright".to_string(), done: Some("kebbie sent the task the failing log; it stays under Needs you".to_string()) };
        assert_eq!(texts(&red), ("CI failed on web · app".to_string(), "ci · e2e · Run playwright. kebbie sent the task the failing log; it stays under Needs you.".to_string()));
        let started = About::Started { item: item("Projects/retry.md"), title: "Retry flaky red runs".to_string(), address: "retry@web".to_string(), recipe: Some("feature".to_string()) };
        assert_eq!(texts(&started), ("Started: Retry flaky red runs".to_string(), "On MacBook as retry, recipe feature.".to_string()));
        assert_eq!(texts(&About::Started { item: item("Projects/retry.md"), title: "Retry flaky red runs".to_string(), address: "retry@web".to_string(), recipe: None }).1, "On MacBook as retry.");
        let opened = [question.open(), questions.open(), new.open(), started.open()];
        assert_eq!(opened, [Target::Task { address: "billing@greenhouse".to_string() }, Target::Item { item: item("Projects/retry.md") }, Target::Device { device }, Target::Task { address: "retry@web".to_string() }]);
        assert_eq!([question.category(), prompt.category(), questions.category(), new.category(), ready.category(), red.category(), started.category()], [NoticeCategory::Questions, NoticeCategory::Questions, NoticeCategory::IdeaQuestions, NoticeCategory::NewDevice, NoticeCategory::Ready, NoticeCategory::CiFailed, NoticeCategory::Started]);
    }

    #[test]
    fn a_long_title_or_body_is_cut_to_the_limits_and_every_notice_passes_the_protocols_check() {
        let long = About::Question { address: "billing@greenhouse".to_string(), text: "Why? ".repeat(400), recommended: None };
        let notice = long.notice(&"M".repeat(64), vec!["q-1".to_string()]);
        assert_eq!(notice.body.chars().count(), MAX_NOTICE_BODY_CHARS);
        assert!(notice.body.ends_with('…'));
        let title = About::ItemQuestions { item: item("Projects/a.md"), title: format!("{}\nsecond line", "t".repeat(200)), first: "Q?".to_string(), count: 1 }.title("M");
        assert!(title.chars().count() == MAX_NOTICE_TITLE_CHARS && !title.contains('\n'), "{title}");
        assert_eq!(Payload::Notice(notice).validate(), Ok(()));
    }

    #[test]
    fn an_agents_prompt_reads_as_the_command_it_asks_to_run_and_its_options_in_short() {
        let menu = crate::menu::Menu { question: "Bash command\ncargo test --workspace\nRun the tests\nDo you want to proceed?".to_string(), options: vec!["Yes".to_string(), "Yes, and don’t ask again for: cargo test".to_string(), "No, and tell Claude what to do differently (esc)".to_string()], focused: 0, numbered: true, cut: false };
        assert_eq!(prompt_said("Claude", &menu), "Claude asks to run cargo test --workspace. Yes or No.");
        let trust = crate::menu::Menu { question: "Do you trust this folder?".to_string(), options: vec!["No, exit".to_string(), "Yes, I trust this folder".to_string()], focused: 0, numbered: false, cut: false };
        assert_eq!(prompt_said("Claude", &trust), "Claude asks: Do you trust this folder? No or Yes.");
        assert_eq!(crate::menu::phone_said("nothing to read", crate::agent::Agent::Codex, Some("you to sign in")), "Codex asks you to sign in before it takes the task's prompt.");
    }

    #[test]
    fn a_collapse_tag_names_the_subject_by_digest_per_machine_and_kind() {
        let (mac, devbox) = (Id::ulid(Prefix::Machine, 1, [1; 10]).unwrap(), Id::ulid(Prefix::Machine, 1, [2; 10]).unwrap());
        let tagged = tag(&mac, Subject::Question, "billing@greenhouse");
        assert!(tagged.starts_with("q-") && tagged.len() == 26 && !tagged.contains("billing"), "{tagged}");
        assert_eq!(tagged, tag(&mac, Subject::Question, "billing@greenhouse"));
        assert_ne!(tagged, tag(&devbox, Subject::Question, "billing@greenhouse"));
        assert_ne!(tagged, tag(&mac, Subject::Ready, "billing@greenhouse"), "a question and a ready on one task are two notices");
    }

    struct Phone {
        key: DeviceKey,
        cert: DeviceCert,
    }

    struct Machine {
        dir: PathBuf,
        identity: Identity,
        keys: SecretKeys,
        account: AccountKey,
    }

    impl Machine {
        fn new() -> Machine {
            let dir = crate::sh::test_dir("hub-notices");
            let identity = Identity::new("https://hub.example.com", Id::new(Prefix::Machine).unwrap(), "MacBook", "khm_token".to_string(), &SecretKeys::generate(), 1_000);
            identity::save_at(&dir, &identity).unwrap();
            let keys = SecretKeys::generate();
            let account = AccountKey::new(Id::account([3; 16]), 1, keys.public()).unwrap();
            Machine { dir, identity, keys, account }
        }

        fn phone(&self, name: &str, state: Option<State>) -> Phone {
            let key = DeviceKey::generate();
            let cert = DeviceCert::issue(&self.keys, &self.account.account, Id::new(Prefix::Device).unwrap(), name, DeviceKind::Web, key.public(), 1_000).unwrap();
            if let Some(state) = state {
                let mut known = devices::load(&self.dir);
                known.devices.insert(cert.device.clone(), Device { signing: Bytes32(cert.signing), name: name.to_string(), kind: DeviceKind::Web, first_seen: 1_000, seen: 1_000, state, at: 1_000 });
                devices::save(&self.dir, &known).unwrap();
            }
            Phone { key, cert }
        }

        fn deliver(&self, listed: &[NotifiedDevice], about: &About, except: Option<&Id>) -> Vec<Envelope> {
            let posted = std::cell::RefCell::new(Vec::new());
            let wire = Wire { listed: &|| Ok(listed.to_vec()), post: &|envelope| { posted.borrow_mut().push(envelope.clone()); Ok(1) } };
            let delivered = deliver_in(&self.dir, &self.identity, &self.account, about, except, DEVICE, &wire, 2_000).unwrap();
            assert!(delivered.problems.is_empty(), "{delivered:?}");
            posted.into_inner()
        }

        fn opened(&self, phone: &Phone, envelopes: &[Envelope]) -> Vec<Notice> {
            envelopes.iter().filter_map(|envelope| match open(envelope, &phone.cert.device, phone.key.keys(), &self.identity.machine, &self.identity.public(), 2_000) {
                Ok(Payload::Notice(notice)) => Some(notice),
                _ => None,
            }).collect()
        }
    }

    fn listed(phone: &Phone, categories: Option<&[NoticeCategory]>) -> NotifiedDevice {
        NotifiedDevice { cert: phone.cert.clone(), notifications: categories.map(<[NoticeCategory]>::to_vec) }
    }

    fn question(address: &str) -> About {
        About::Question { address: address.to_string(), text: "Which queue?".to_string(), recommended: None }
    }

    #[test]
    fn a_notice_is_sealed_only_to_devices_approved_here_that_hold_a_subscription_with_its_category_on() {
        let machine = Machine::new();
        let all = NoticeCategory::ALL.to_vec();
        let approved = machine.phone("iPhone 16", Some(State::Approved));
        let waiting = machine.phone("iPad", Some(State::Waiting));
        let denied = machine.phone("Chrome on Mac", Some(State::Denied));
        let unknown = machine.phone("Firefox", None);
        let quiet = machine.phone("Pixel", Some(State::Approved));
        let unsubscribed = machine.phone("Safari", Some(State::Approved));
        let other_keys = SecretKeys::generate();
        let posing = AccountKey::new(machine.account.account.clone(), 1, other_keys.public()).unwrap();
        let forged = Phone { key: DeviceKey::generate(), cert: DeviceCert::issue(&other_keys, &posing.account, approved.cert.device.clone(), "iPhone 16", DeviceKind::Web, DeviceKey::generate().public(), 1_000).unwrap() };
        let listing = [listed(&approved, Some(&all)), listed(&waiting, Some(&all)), listed(&denied, Some(&all)), listed(&unknown, Some(&all)), listed(&quiet, Some(&[NoticeCategory::Ready])), listed(&unsubscribed, None), listed(&forged, Some(&all))];
        let sent = machine.deliver(&listing, &question("billing@greenhouse"), None);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].keys.keys().collect::<Vec<_>>(), [&approved.cert.device], "only the approved, subscribed device with questions on");
        assert_eq!(sent[0].header.collapse.as_deref(), Some(tag(&machine.identity.machine, Subject::Question, "billing@greenhouse").as_str()));
        assert_eq!(sent[0].header.urgency, Some(Urgency::High));
        assert_eq!(sent[0].header.check_shape(), Ok(()));
        assert_eq!(machine.opened(&approved, &sent)[0].body, "Which queue?");
        let ready = machine.deliver(&listing, &About::Ready { address: "cache@api".to_string(), does: "Caches it".to_string(), commits: 2, review: None }, None);
        let mut reached: Vec<&Id> = ready[0].keys.keys().collect();
        reached.sort();
        let mut expected = vec![&approved.cert.device, &quiet.cert.device];
        expected.sort();
        assert_eq!(reached, expected);
        let account = Policy { trust: HubTrust::Account, auto: AutoApprove::Day };
        assert_eq!(recipients(&machine.account, &devices::load(&machine.dir), account, &listing, NoticeCategory::Questions, None, 2_000).len(), 4, "under account trust every certified, subscribed device counts, a forged one never");
    }

    #[test]
    fn the_device_whose_act_it_reports_is_left_out() {
        let machine = Machine::new();
        let (phone, tablet) = (machine.phone("iPhone 16", Some(State::Approved)), machine.phone("iPad", Some(State::Approved)));
        let all = NoticeCategory::ALL.to_vec();
        let started = About::Started { item: item("Projects/retry.md"), title: "Retry".to_string(), address: "retry@web".to_string(), recipe: None };
        let sent = machine.deliver(&[listed(&phone, Some(&all)), listed(&tablet, Some(&all))], &started, Some(&phone.cert.device));
        assert_eq!(sent[0].keys.keys().collect::<Vec<_>>(), [&tablet.cert.device]);
        assert!(machine.deliver(&[listed(&phone, Some(&all))], &started, Some(&phone.cert.device)).is_empty(), "nothing goes out when only the acting device would hear it");
    }

    #[test]
    fn a_settled_subject_is_retracted_with_the_next_notice_to_each_device_that_had_it() {
        let machine = Machine::new();
        let all = NoticeCategory::ALL.to_vec();
        let (phone, tablet) = (machine.phone("iPhone 16", Some(State::Approved)), machine.phone("iPad", Some(State::Approved)));
        let asked = tag(&machine.identity.machine, Subject::Question, "billing@greenhouse");
        machine.deliver(&[listed(&phone, Some(&all))], &question("billing@greenhouse"), None);
        settle_in(&machine.dir, Subject::Question, "billing@greenhouse").unwrap();
        settle_in(&machine.dir, Subject::Question, "billing@greenhouse").unwrap();
        assert_eq!(ledger(&machine.dir).outstanding.get(&phone.cert.device), Some(&vec![asked.clone()]), "settled once, kept until a notice carries it");
        let both = [listed(&phone, Some(&all)), listed(&tablet, Some(&all))];
        let sent = machine.deliver(&both, &question("search@web"), None);
        assert_eq!(sent.len(), 2, "the device with a retract gets its own envelope");
        assert_eq!(machine.opened(&phone, &sent).iter().map(|notice| notice.retract.clone()).collect::<Vec<_>>(), [vec![asked.clone()]]);
        assert_eq!(machine.opened(&tablet, &sent).iter().map(|notice| notice.retract.clone()).collect::<Vec<_>>(), [Vec::<String>::new()], "a device that never had it retracts nothing");
        assert!(ledger(&machine.dir).outstanding.is_empty(), "a retract goes once");
        let again = machine.deliver(&both, &question("search@web"), None);
        assert_eq!(again.len(), 1, "a newer notice on the same subject replaces the older under one tag");
        assert_eq!(again[0].header.collapse, sent[0].header.collapse);

        machine.deliver(&[listed(&phone, Some(&all))], &question("billing@greenhouse"), None);
        settle_in(&machine.dir, Subject::Question, "billing@greenhouse").unwrap();
        let asked_again = machine.deliver(&[listed(&phone, Some(&all))], &question("billing@greenhouse"), None);
        assert_eq!(machine.opened(&phone, &asked_again)[0].retract, Vec::<String>::new(), "a notice never retracts its own tag");
        assert!(settle_in(&machine.dir, Subject::Ready, "never@sent").is_ok() && ledger(&machine.dir).outstanding.is_empty(), "settling what was never sent changes nothing");
    }

    #[test]
    fn a_device_approved_automatically_after_its_day_settles_its_notice_as_a_manual_approval_does() {
        let machine = Machine::new();
        let day = Policy { trust: HubTrust::Device, auto: AutoApprove::Day };
        let phone = machine.phone("iPhone 16", Some(State::Approved));
        let (tablet, laptop) = (machine.phone("iPad", Some(State::Waiting)), machine.phone("Chrome on Mac", Some(State::Waiting)));
        let new = |waiting: &Phone| About::NewDevice { device: waiting.cert.device.clone(), name: waiting.cert.name.clone(), added: "14:02".to_string(), from_devices: true };
        for waiting in [&tablet, &laptop] {
            machine.deliver(&[listed(&phone, Some(&NoticeCategory::ALL))], &new(waiting), None);
        }
        let mut known = devices::load(&machine.dir);
        known.devices.get_mut(&laptop.cert.device).unwrap().first_seen = 1_000 + devices::DAY;
        devices::save(&machine.dir, &known).unwrap();
        devices::settle_due(&machine.dir, day, 1_000 + devices::DAY - 1).unwrap();
        assert!(ledger(&machine.dir).outstanding.is_empty(), "nothing settles before the day is up");
        devices::settle_due(&machine.dir, day, 1_000 + devices::DAY).unwrap();
        assert_eq!(devices::load(&machine.dir).devices[&tablet.cert.device].state, State::Approved);
        assert_eq!(ledger(&machine.dir).outstanding[&phone.cert.device], [tag(&machine.identity.machine, Subject::Device, tablet.cert.device.as_str())], "the auto-approved device's notice goes; the one still waiting keeps its own");
        devices::settle_due(&machine.dir, Policy { trust: HubTrust::Device, auto: AutoApprove::Never }, 1_000 + 3 * devices::DAY).unwrap();
        assert_eq!(devices::load(&machine.dir).devices[&laptop.cert.device].state, State::Waiting, "never keeps it waiting");
        let mut again = devices::load(&machine.dir);
        assert_eq!(again.settle(day, 1_000 + 2 * devices::DAY), std::slice::from_ref(&laptop.cert.device), "a command's own settle names what it approved, for handle to settle its notice");
    }

    #[test]
    fn a_notice_the_hub_refuses_as_it_stands_is_logged_and_dropped_with_its_retracts_and_one_it_could_take_later_keeps_them() {
        for status in [400, 409, 413] {
            assert!(matches!(unposted(HubError::Status { status, code: "x".to_string() }), Unposted::Dropped(_)), "{status}");
        }
        for error in [HubError::Status { status: 429, code: "rate-limited".to_string() }, HubError::Status { status: 503, code: String::new() }, HubError::Transport("no route".to_string())] {
            assert!(matches!(unposted(error), Unposted::Later(_)));
        }
        let machine = Machine::new();
        let phone = machine.phone("iPhone 16", Some(State::Approved));
        let all = [listed(&phone, Some(&NoticeCategory::ALL))];
        machine.deliver(&all, &question("billing@greenhouse"), None);
        settle_in(&machine.dir, Subject::Question, "billing@greenhouse").unwrap();
        let refusing = |answer: Unposted| {
            let tried = std::cell::Cell::new(0);
            let wire = Wire { listed: &|| Ok(all.to_vec()), post: &|_| { tried.set(tried.get() + 1); Err(answer.clone()) } };
            let delivered = deliver_in(&machine.dir, &machine.identity, &machine.account, &question("search@web"), None, DEVICE, &wire, 2_000).unwrap();
            (delivered, tried.get())
        };
        let (later, tried) = refusing(Unposted::Later("the notice was not posted: 429".to_string()));
        assert_eq!((later.reached, later.problems.len(), tried), (0, 1, 1), "posted once, never retried here");
        assert_eq!(ledger(&machine.dir).outstanding[&phone.cert.device].len(), 1, "a retract the hub could take later rides with the next notice");
        let (dropped, tried) = refusing(Unposted::Dropped("the hub refused the notice: 413 too-large".to_string()));
        assert_eq!((dropped.reached, dropped.problems, tried), (0, vec!["the hub refused the notice: 413 too-large".to_string()], 1));
        assert!(ledger(&machine.dir).outstanding.is_empty(), "a refused envelope's retracts go with it, so they never hold up the next notice");
        assert!(!ledger(&machine.dir).sent.contains_key(&tag(&machine.identity.machine, Subject::Question, "search@web")), "nothing to retract for a notice that never went out");
    }

    #[test]
    fn a_notice_goes_through_the_hubs_routes_to_the_device_it_lists_and_a_replay_is_dropped() {
        use crate::hub::tests::{connected_state, member, open_hub};
        let hub = open_hub(|config| {
            config.vapid_subject = "mailto:hub-admin@example.com".to_string();
            config.push_allow_hosts = vec!["127.0.0.1".to_string()];
        });
        let mut browser = member(&hub);
        let state = connected_state(&hub, &mut browser);
        let dir = identity::dir(&state);
        let identity = identity::load_at(&dir).unwrap().unwrap();
        let account = identity.account.as_ref().unwrap().key.clone();
        hub.runtime.block_on(browser.register_device()).unwrap();
        let secret = kebbie_hub_fakes::push::PushSecret::generate("http://127.0.0.1:9/push/phone");
        hub.runtime.block_on(browser.subscribe_push(&secret.subscription(&[NoticeCategory::Questions]))).unwrap();
        let listed = listed_from(&identity).unwrap();
        let device = browser.device_id().clone();
        assert!(listed.iter().any(|listed| listed.cert.device == device && listed.notifications == Some(vec![NoticeCategory::Questions])), "{listed:?}");
        let posted = std::cell::RefCell::new(Vec::new());
        let wire = Wire { listed: &|| listed_from(&identity), post: &|envelope| { posted.borrow_mut().push(envelope.clone()); post_to(&identity, envelope) } };
        let account_trust = Policy { trust: HubTrust::Account, auto: AutoApprove::Day };
        let delivered = deliver_in(&dir, &identity, &account, &question("billing@greenhouse"), None, account_trust, &wire, crate::messages::now()).unwrap();
        assert_eq!((delivered.reached, delivered.pushes, delivered.problems.clone()), (1, 1, Vec::<String>::new()), "{delivered:?}");
        let envelope = posted.borrow()[0].clone();
        browser.knows(&identity.machine, identity.public());
        assert_eq!(browser.open_notice(&envelope, crate::messages::now()).unwrap().title, "billing · greenhouse", "the listed device opens it");
        assert!(matches!(post_to(&identity, &envelope), Err(Unposted::Dropped(problem)) if problem.contains("409")), "a replayed envelope is refused and dropped");
        let ready = deliver_in(&dir, &identity, &account, &About::Ready { address: "cache@api".to_string(), does: "Caches it".to_string(), commits: 1, review: None }, None, account_trust, &wire, crate::messages::now()).unwrap();
        assert_eq!(ready, Delivered::default(), "a category the device left off sends nothing");
    }

    fn row(key: &ItemKey, title: &str) -> BacklogRow {
        BacklogRow { item: key.clone(), title: title.to_string(), priority: Priority::Medium, repo: None, state: BacklogState::Ready, filed: 0, home: Home { kind: kebbie_hub_protocol::HomeKind::Vault, label: key.as_str().trim_start_matches("vault:").to_string() }, questions: 0, blocking: 0, goal: None, body: None }
    }

    fn body(key: &ItemKey, questions: &[(&str, bool, Option<&str>)]) -> Attachment {
        let questions = questions.iter().map(|(text, blocks, answer)| OpenQuestion { text: text.to_string(), blocks: *blocks, pending_answer: answer.map(str::to_string) }).collect();
        let written = BacklogBody { background: String::new(), goal: String::new(), what_we_know: String::new(), questions, out_of_scope: String::new(), source: String::new() };
        Attachment::Backlog { item: key.clone(), text: serde_json::to_string(&written).unwrap() }
    }

    #[test]
    fn the_leads_new_questions_on_an_item_are_told_once_and_settle_when_answered() {
        let machine = Machine::new();
        let key = item("Projects/retry.md");
        let board = Board { rev: 0, kebbie: String::new(), away_since: None, checkouts: Vec::new(), routines: Vec::new(), items: Vec::new(), sessions_outside_tasks: 0, next: None, counts: None, later: None, accepts: None, devices: None, backlog: Some(vec![row(&key, "Retry flaky red runs")]) };
        let one = [body(&key, &[("Which red runs count as flaky?", true, None)])];
        assert!(items_in(&machine.dir, &board, &one).unwrap().is_empty(), "the first look only records what waits");
        let two = [body(&key, &[("Which red runs count as flaky?", true, None), ("How long to wait?", false, None), ("Show a rerun on the row?", true, None)])];
        assert_eq!(items_in(&machine.dir, &board, &two).unwrap(), [About::ItemQuestions { item: key.clone(), title: "Retry flaky red runs".to_string(), first: "Which red runs count as flaky?".to_string(), count: 2 }]);
        assert!(items_in(&machine.dir, &board, &two).unwrap().is_empty(), "told once");
        let written = std::fs::metadata(machine.dir.join(FILE)).and_then(|metadata| metadata.modified()).unwrap();
        items_in(&machine.dir, &board, &two).unwrap();
        assert_eq!(std::fs::metadata(machine.dir.join(FILE)).and_then(|metadata| metadata.modified()).unwrap(), written, "the hub's tick leaves notices.json alone while nothing changed");
        let phone = machine.phone("iPhone 16", Some(State::Approved));
        machine.deliver(&[listed(&phone, Some(&NoticeCategory::ALL))], &items_in_about(&key), None);
        let answered = [body(&key, &[("Which red runs count as flaky?", true, Some("Only e2e")), ("Show a rerun on the row?", true, Some("Yes"))])];
        assert!(items_in(&machine.dir, &board, &answered).unwrap().is_empty());
        assert_eq!(ledger(&machine.dir).outstanding[&phone.cert.device], [tag(&machine.identity.machine, Subject::Item, key.as_str())], "answered on the phone or the board, its notice goes");
    }

    fn items_in_about(key: &ItemKey) -> About {
        About::ItemQuestions { item: key.clone(), title: "Retry flaky red runs".to_string(), first: "Which?".to_string(), count: 2 }
    }

    #[test]
    fn the_device_list_reads_each_certificate_and_the_categories_it_takes() {
        let machine = Machine::new();
        let phone = machine.phone("iPhone 16", None);
        let json = serde_json::json!({ "devices": [{ "cert": phone.cert, "notifications": ["questions", "ready"] }, { "cert": phone.cert }] });
        let listing: NotifiedDevices = serde_json::from_value(json).unwrap();
        assert_eq!(listing.devices[0].notifications, Some(vec![NoticeCategory::Questions, NoticeCategory::Ready]));
        assert_eq!(listing.devices[1].notifications, None);
        let about = About::NewDevice { device: phone.cert.device.clone(), name: "iPhone 16".to_string(), added: "14:02".to_string(), from_devices: true };
        assert_eq!(serde_json::from_str::<About>(&serde_json::to_string(&about).unwrap()).unwrap(), about, "the notice crosses to the detached sender as JSON");
    }
}
