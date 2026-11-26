use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backlog_file::{Filing, Shelf};
use crate::config::Source;
use crate::notice::{Notice, Tone};
use crate::vault_card::Store;
use crate::vault_cli::Wrote;
use crate::vault_machines::Machine;

pub const EVERY: u64 = 30;
pub const WAIT: u64 = 5 * 60;
const TRIES: u32 = 3;

// A backlog item on its way to the machine that writes it out, as kebbie backlog add or n filed it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mail {
    pub id: String,
    pub from: String,
    pub at: u64,
    pub by: String,
    pub line: String,
    #[serde(default)]
    pub words: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub priority: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub start: bool,
    #[serde(default)]
    pub repos: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Here,
    OnlyRepo(String),
    Ideas(String),
}

// The machine that writes an item out: the one machine with its repo, else the ideas machine, else this one.
pub fn target(machines: &[Machine], me: &str, repo: &str) -> Target {
    let repo = repo.trim();
    let holders: Vec<&Machine> = machines.iter().filter(|one| !repo.is_empty() && one.has(repo)).collect();
    let chosen = match holders.as_slice() {
        [only] => Target::OnlyRepo(only.name.clone()),
        _ => machines.iter().find(|one| one.ideas).map_or(Target::Here, |ideas| Target::Ideas(ideas.name.clone())),
    };
    match chosen {
        Target::OnlyRepo(name) | Target::Ideas(name) if name.eq_ignore_ascii_case(me) => Target::Here,
        other => other,
    }
}

fn slug(name: &str) -> String {
    let lowered: String = name.to_lowercase().chars().map(|char| if char.is_alphanumeric() { char } else { '-' }).collect();
    lowered.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-")
}

pub fn inbox_key(machine: &str) -> String {
    format!("{}/inbox-{}.md", crate::vault_card::FOLDER, slug(machine))
}

fn parse(text: &str) -> Vec<Mail> {
    let Some(start) = text.find("```json\n") else { return Vec::new() };
    let rest = &text[start + 8..];
    rest.find("\n```").and_then(|end| serde_json::from_str(&rest[..end]).ok()).unwrap_or_default()
}

fn render(machine: &str, mails: &[Mail]) -> String {
    let json = serde_json::to_string_pretty(mails).unwrap_or_else(|_| "[]".to_string());
    format!("---\ntitle: {}\ntags: [kebbie, inbox]\nstatus: active\n---\n\nBacklog items other machines sent {machine} to write out. kebbie on {machine} takes them from here and files them; nothing here needs you.\n\n```json\n{json}\n```\n", serde_json::to_string(&format!("{machine}'s backlog inbox")).unwrap_or_default())
}

// The vault answers a missing note as any failed read, so an unread inbox counts as none, and a create over a note that exists after all loses as a stale write.
fn read_inbox(store: &dyn Store, machine: &str, exists: bool) -> Result<(Vec<Mail>, String), String> {
    match store.read(&inbox_key(machine)) {
        Ok((text, etag)) => Ok((parse(&text), etag)),
        Err(_) if !exists => Ok((Vec::new(), "*".to_string())),
        Err(error) => Err(format!("{} was not read ({error})", inbox_key(machine))),
    }
}

// The inbox changed by one function, written only over the version it read; a write that lost to another machine's is read again.
fn change(store: &dyn Store, machine: &str, exists: bool, edit: impl Fn(&mut Vec<Mail>) -> bool) -> Result<Vec<Mail>, String> {
    for _ in 0..TRIES {
        let (before, etag) = read_inbox(store, machine, exists)?;
        let mut after = before.clone();
        if !edit(&mut after) {
            return Ok(before);
        }
        match store.write(&inbox_key(machine), &etag, &render(machine, &after)) {
            Ok(Wrote::Saved) => return Ok(before),
            Ok(Wrote::Stale) => continue,
            Err(error) => return Err(format!("{} was not written ({error})", inbox_key(machine))),
        }
    }
    Err(format!("{} kept changing under kebbie, so it was not written", inbox_key(machine)))
}

pub fn post(store: &dyn Store, to: &str, mail: &Mail) -> Result<(), String> {
    change(store, to, false, |mails| {
        mails.retain(|one| one.id != mail.id);
        mails.push(mail.clone());
        true
    }).map(|_| ())
}

// The given items taken out of an inbox in one write; those it no longer held were taken by someone else first.
fn remove(store: &dyn Store, machine: &str, exists: bool, ids: &[String]) -> Result<Vec<String>, String> {
    let before = change(store, machine, exists, |mails| {
        let count = mails.len();
        mails.retain(|mail| !ids.contains(&mail.id));
        mails.len() != count
    })?;
    Ok(ids.iter().filter(|id| !before.iter().any(|mail| &&mail.id == id)).cloned().collect())
}

fn id(now: u64, from: &str, filing: &Filing) -> String {
    crate::trusted_spawn::digest(&format!("{now}\n{from}\n{}\n{}", filing.line, filing.words.unwrap_or_default()))[..12].to_string()
}

pub fn mail_of(filing: &Filing, from: &str, now: u64) -> Mail {
    Mail { id: id(now, from, filing), from: from.to_string(), at: now, by: filing.by.to_string(), line: filing.line.to_string(), words: filing.words.map(str::to_string), body: filing.body.map(str::to_string), priority: filing.priority.to_string(), repo: filing.repo.to_string(), start: filing.start, repos: filing.repos.to_vec() }
}

fn sent_notice(target: &Target) -> Option<Result<String, String>> {
    let (headline, machine) = match target {
        Target::Here => return None,
        Target::Ideas(machine) => (format!("Sent to {machine}, the ideas machine"), machine),
        Target::OnlyRepo(machine) => (format!("Sent to {machine}, the only machine with its repo"), machine),
    };
    Some(Notice::new(Tone::Moving, headline).next(format!("{machine} writes it out and files it in the vault; its row waits on {machine}'s board. If {machine} does not take it within {} minutes, it is added here.", WAIT / 60)).ok())
}

// An item for the vault goes to the machine the rule picks; None files it here, as when the vault or the other machine was not reached.
pub fn send_with(store: &dyn Store, state: &Path, me: &Machine, filing: &Filing, now: u64) -> Option<Result<String, String>> {
    let machines = crate::vault_machines::read(store, me, now);
    let chosen = target(&machines, &me.name, filing.repo);
    let to = match &chosen {
        Target::Here => return None,
        Target::Ideas(machine) | Target::OnlyRepo(machine) => machine.clone(),
    };
    let mail = mail_of(filing, &me.name, now);
    if let Err(error) = post(store, &to, &mail) {
        crate::vault_cli::warn_log(state, &format!("\"{}\" was filed on this machine, as {to}'s inbox was not reached: {error}", filing.line));
        return None;
    }
    if let Err(error) = change_sent(state, |sent| sent.push(Sent { to, mail, unreached: false })) {
        crate::vault_cli::warn_log(state, &format!("\"{}\" is not taken back if it waits: {error}", filing.line));
    }
    sent_notice(&chosen)
}

pub fn send(sources: &[Source], filing: &Filing) -> Option<Result<String, String>> {
    if crate::config::filing(sources) != Some(Source::Vault) {
        return None;
    }
    let cli = crate::vault_cli::detect().filter(crate::vault_machines::writes)?;
    let state = crate::lead::state_dir().ok()?;
    let now = crate::messages::now();
    send_with(&cli, &state, &crate::vault_machines::me(now), filing, now)
}

// An item sent to another machine, kept until that machine takes it or it is taken back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Sent {
    to: String,
    mail: Mail,
    #[serde(default)]
    unreached: bool,
}

fn sent_file(state: &Path) -> PathBuf {
    state.join("idea-mail-sent.json")
}

fn sent(state: &Path) -> Vec<Sent> {
    std::fs::read_to_string(sent_file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

// kebbie backlog add and the keeper's round both change the list, so each change reads and writes it under one lock.
fn change_sent(state: &Path, edit: impl FnOnce(&mut Vec<Sent>)) -> Result<(), String> {
    let _held = crate::state::Locked::wait(&state.join("idea-mail-sent.lock"))?;
    let mut all = sent(state);
    edit(&mut all);
    crate::state::write(&sent_file(state), serde_json::to_string(&all).unwrap_or_default())
}

#[derive(Debug, Default, PartialEq)]
pub struct Reclaimed {
    pub back: Vec<String>,
    pub unreached: Vec<String>,
    pub failed: Vec<String>,
}

// An item still in the other machine's inbox after WAIT is filed here; one gone from it was taken there. An inbox that cannot be read proves neither, so its item waits and the lead hears it once. An item is kept here before it leaves the inbox, so a failed save loses nothing.
pub fn reclaim(store: &dyn Store, state: &Path, now: u64) -> Reclaimed {
    let mut lines = Reclaimed::default();
    let (back, failed) = settle_held(store, state, true);
    lines.back.extend(back);
    lines.failed.extend(failed);
    let (mut settled, mut told) = (Vec::new(), Vec::new());
    for one in sent(state) {
        if now < one.mail.at + WAIT {
            continue;
        }
        let id = one.mail.id.clone();
        let waiting = match read_inbox(store, &one.to, true) {
            Ok((mails, _)) => mails.iter().any(|mail| mail.id == id),
            Err(error) => {
                if !one.unreached {
                    lines.unreached.push(format!("{}'s inbox could not be read for {} minutes, so \"{}\" waits there until it can be read ({error})", one.to, WAIT / 60, one.mail.line));
                    told.push(id);
                }
                continue;
            }
        };
        if !waiting {
            settled.push(id);
            continue;
        }
        if let Err(error) = take(state, std::slice::from_ref(&one.mail)) {
            lines.failed.push(error);
            continue;
        }
        match remove(store, &one.to, true, std::slice::from_ref(&id)) {
            Ok(gone) if gone.is_empty() => {
                lines.back.push(format!("{} did not take \"{}\" within {} minutes: added here", one.to, one.mail.line, WAIT / 60));
                settled.push(id);
            }
            Ok(gone) => {
                if let Err(error) = drop_pending(state, &gone) {
                    lines.failed.push(error);
                }
                settled.push(id);
            }
            Err(error) => {
                if let Err(error) = hold(state, &one.to, std::slice::from_ref(&one.mail), true) {
                    lines.failed.push(error);
                }
                lines.failed.push(format!("\"{}\" may still wait in {}'s inbox, which answered a write with an error ({error}); kebbie keeps its copy until a read of that inbox shows where it is", one.mail.line, one.to));
            }
        }
    }
    if !settled.is_empty() || !told.is_empty() {
        let saved = change_sent(state, |sent| {
            sent.retain(|one| !settled.contains(&one.mail.id));
            sent.iter_mut().filter(|one| told.contains(&one.mail.id)).for_each(|one| one.unreached = true);
        });
        if let Err(error) = saved {
            lines.failed.push(error);
        }
    }
    lines
}

fn pending_file(state: &Path) -> PathBuf {
    state.join("idea-mail-pending.json")
}

fn pending(state: &Path) -> Vec<Mail> {
    std::fs::read_to_string(pending_file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save_pending(state: &Path, mails: &[Mail]) -> Result<(), String> {
    crate::state::write(&pending_file(state), serde_json::to_string(mails).unwrap_or_default())
}

fn keep_pending(state: &Path, mails: &[Mail]) -> Result<(), String> {
    let mut all = pending(state);
    all.extend(mails.iter().filter(|mail| !all.iter().any(|one| one.id == mail.id)).cloned().collect::<Vec<_>>());
    save_pending(state, &all)
}

fn drop_pending(state: &Path, ids: &[String]) -> Result<(), String> {
    let mut all = pending(state);
    all.retain(|mail| !ids.contains(&mail.id));
    save_pending(state, &all)
}

// Mail read from its inbox here is pending, so a copy still held from an earlier erring write would file it a second time.
fn take(state: &Path, mails: &[Mail]) -> Result<(), String> {
    keep_pending(state, mails)?;
    let mut all = held(state);
    let count = all.len();
    all.retain(|one| !mails.iter().any(|mail| mail.id == one.mail.id));
    if all.len() == count { Ok(()) } else { save_held(state, &all) }
}

// A copy taken out of an inbox whose write answered with an error, which may have saved all the same.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Held {
    inbox: String,
    mail: Mail,
    #[serde(default)]
    back: bool,
}

fn held_file(state: &Path) -> PathBuf {
    state.join("idea-mail-held.json")
}

fn held(state: &Path) -> Vec<Held> {
    std::fs::read_to_string(held_file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save_held(state: &Path, all: &[Held]) -> Result<(), String> {
    crate::state::write(&held_file(state), serde_json::to_string(all).unwrap_or_default())
}

// The copies move out of pending, where they would be filed now, only once they are saved here; a failed save leaves them pending, filed here rather than lost.
fn hold(state: &Path, inbox: &str, mails: &[Mail], back: bool) -> Result<(), String> {
    let mut all = held(state);
    all.extend(mails.iter().filter(|mail| !all.iter().any(|one| one.mail.id == mail.id)).map(|mail| Held { inbox: inbox.to_string(), mail: mail.clone(), back }).collect::<Vec<_>>());
    save_held(state, &all)?;
    drop_pending(state, &mails.iter().map(|mail| mail.id.clone()).collect::<Vec<_>>())
}

// A held copy is let go only on a read of its inbox: one the inbox still has waits there, one gone from it left by the write that erred and is filed here; an unread inbox keeps it held.
fn settle_held(store: &dyn Store, state: &Path, taken_back: bool) -> (Vec<String>, Vec<String>) {
    let (mut back, mut failed, mut left) = (Vec::new(), Vec::new(), Vec::new());
    let all = held(state);
    if !all.iter().any(|one| one.back == taken_back) {
        return (back, failed);
    }
    for one in all {
        if one.back != taken_back {
            left.push(one);
            continue;
        }
        match read_inbox(store, &one.inbox, true) {
            Ok((mails, _)) if mails.iter().any(|mail| mail.id == one.mail.id) => {}
            Ok(_) => match keep_pending(state, std::slice::from_ref(&one.mail)) {
                Ok(()) if one.back => back.push(format!("{} did not take \"{}\" within {} minutes: added here", one.inbox, one.mail.line, WAIT / 60)),
                Ok(()) => {}
                Err(error) => {
                    failed.push(error);
                    left.push(one);
                }
            },
            Err(_) => left.push(one),
        }
    }
    if let Err(error) = save_held(state, &left) {
        failed.push(error);
    }
    (back, failed)
}

fn by_of(mail: &Mail) -> String {
    format!("{} on {}", mail.by, mail.from)
}

// The items in this machine's inbox are kept here before they leave it and until each is filed, so a failed save loses nothing and one the lead's inbox refused is filed again next round; the lines name those.
pub fn receive(store: &dyn Store, state: &Path, me: &str, file: &dyn Fn(&Filing) -> Result<String, String>) -> Result<Vec<String>, String> {
    let (_, mut lines) = settle_held(store, state, false);
    let (inbox, _) = read_inbox(store, me, false)?;
    if !inbox.is_empty() {
        take(state, &inbox)?;
        let ids: Vec<String> = inbox.iter().map(|mail| mail.id.clone()).collect();
        match remove(store, me, false, &ids) {
            Ok(gone) => drop_pending(state, &gone)?,
            Err(error) => {
                hold(state, me, &inbox, false)?;
                return Err(error);
            }
        }
    }
    let waiting = pending(state);
    let mut left = Vec::new();
    for mail in waiting {
        let by = if mail.from.eq_ignore_ascii_case(me) { mail.by.clone() } else { by_of(&mail) };
        let filing = Filing { priority: &mail.priority, repo: &mail.repo, line: &mail.line, words: mail.words.as_deref(), body: mail.body.as_deref(), by: &by, start: mail.start, repos: &mail.repos };
        match file(&filing) {
            Ok(_) => {}
            Err(error) => {
                lines.push(format!("\"{}\" from {} was not filed yet ({error})", mail.line, mail.from));
                left.push(mail);
            }
        }
    }
    save_pending(state, &left)?;
    Ok(lines)
}

pub fn tick(state: &Path) -> Vec<String> {
    let Some(cli) = crate::vault_cli::detect().filter(crate::vault_machines::writes) else { return Vec::new() };
    let Ok(sources) = crate::config::sources() else { return Vec::new() };
    let dir = crate::backlog_file::dir();
    let caller = crate::permit::caller();
    let tell = |text: &str| crate::messages::to_lead(crate::messages::ToLead::BacklogItem, text);
    let write = |text: &str| crate::backlog_writer::hand(state, text, &caller, &tell);
    let shelf = Shelf { dir: &dir, state, tell: &tell, write: &write };
    let file = |filing: &Filing| crate::backlog_file::file_item_on(&shelf, &sources, filing, false);
    let Reclaimed { back, unreached, mut failed } = reclaim(&cli, state, crate::messages::now());
    for line in back.iter().chain(&unreached) {
        let _ = crate::messages::to_lead(crate::messages::ToLead::Notice, &format!("{line}. Tell the user, as they were told it went there."));
    }
    failed.extend(receive(&cli, state, &crate::config::machine(), &file).unwrap_or_else(|error| vec![error]));
    for line in &failed {
        crate::vault_cli::warn_log(state, line);
    }
    back.into_iter().chain(unreached).chain(failed).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_machines::tests::{Vault, machine};

    fn filing<'a>(line: &'a str, repo: &'a str) -> Filing<'a> {
        Filing { priority: "high", repo, line, words: Some("the user's words"), body: None, by: "The user", start: true, repos: &[] }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-idea-mail-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_one_machine_with_the_repo_takes_it_else_the_ideas_machine_else_this_one() {
        let machines = [machine("mac", false, &["kebbie", "greenhouse"], 1), machine("devbox", true, &["kebbie", "web"], 1)];
        assert_eq!(target(&machines, "mac", "web"), Target::OnlyRepo("devbox".to_string()), "only devbox has web");
        assert_eq!(target(&machines, "mac", "kebbie"), Target::Ideas("devbox".to_string()), "both have kebbie");
        assert_eq!(target(&machines, "mac", ""), Target::Ideas("devbox".to_string()), "no repo named");
        assert_eq!(target(&machines, "mac", "nowhere"), Target::Ideas("devbox".to_string()), "no machine has it");
        assert_eq!(target(&machines, "mac", "greenhouse"), Target::Here, "only this machine has greenhouse");
        assert_eq!(target(&machines, "devbox", "kebbie"), Target::Here, "the ideas machine files its own");
        assert_eq!(target(&[machine("mac", false, &[], 1), machine("box", false, &[], 1)], "mac", ""), Target::Here, "no ideas machine named");
    }

    #[test]
    fn an_item_added_on_the_mac_is_filed_on_the_ideas_machine_and_the_mac_says_so() {
        let vault = Vault::default();
        vault.notes.borrow_mut().insert(crate::vault_machines::KEY.to_string(), (crate::vault_machines::render(&[machine("devbox", true, &["kebbie"], 100)]), 1));
        let mac = machine("mac", false, &["kebbie"], 100);
        let said = send_with(&vault, &scratch("mac-sent"), &mac, &filing("Retry flaky runs", "kebbie"), 100).unwrap().unwrap();
        assert!(said.contains("Sent to devbox, the ideas machine"), "{said}");
        assert_eq!(send_with(&vault, &scratch("devbox-sent"), &machine("devbox", true, &["kebbie"], 100), &filing("Mine", "kebbie"), 100), None, "on the ideas machine it is filed in place");
        let state = scratch("receive");
        let filed = std::cell::RefCell::new(Vec::new());
        let file = |filing: &Filing| {
            filed.borrow_mut().push((filing.line.to_string(), filing.by.to_string(), filing.words.map(str::to_string), filing.priority.to_string(), filing.start));
            Ok("filed".to_string())
        };
        let lines = receive(&vault, &state, "devbox", &file).unwrap();
        assert_eq!(filed.borrow().as_slice(), [("Retry flaky runs".to_string(), "The user on mac".to_string(), Some("the user's words".to_string()), "high".to_string(), true)]);
        assert!(lines.is_empty(), "a filed item needs no line: {lines:?}");
        assert!(read_inbox(&vault, "devbox", true).unwrap().0.is_empty(), "the inbox is empty once taken");
        assert!(receive(&vault, &state, "devbox", &file).unwrap().is_empty() && filed.borrow().len() == 1, "nothing is filed twice");
    }

    #[test]
    fn an_item_the_receiver_could_not_file_is_filed_next_round() {
        let (vault, state) = (Vault::default(), scratch("retry"));
        post(&vault, "devbox", &mail_of(&filing("Later", ""), "mac", 5)).unwrap();
        let refused = |_: &Filing| Err("the lead's inbox is full".to_string());
        assert!(receive(&vault, &state, "devbox", &refused).unwrap()[0].contains("was not filed yet"));
        let filed = std::cell::Cell::new(0);
        let file = |_: &Filing| {
            filed.set(filed.get() + 1);
            Ok(String::new())
        };
        receive(&vault, &state, "devbox", &file).unwrap();
        receive(&vault, &state, "devbox", &file).unwrap();
        assert_eq!(filed.get(), 1, "kept until filed, then gone");
    }

    #[test]
    fn an_item_the_ideas_machine_did_not_take_in_five_minutes_is_added_where_it_was_added() {
        let (vault, mac, devbox) = (Vault::default(), scratch("mac-back"), scratch("devbox-back"));
        vault.notes.borrow_mut().insert(crate::vault_machines::KEY.to_string(), (crate::vault_machines::render(&[machine("devbox", true, &[], 100)]), 1));
        let me = machine("mac", false, &[], 100);
        let said = send_with(&vault, &mac, &me, &filing("Asleep", ""), 100).unwrap().unwrap();
        assert!(said.contains("within 5 minutes, it is added here"), "{said}");
        assert_eq!(reclaim(&vault, &mac, 100 + WAIT - 1), Reclaimed::default(), "within the wait it stays with devbox");
        assert_eq!(reclaim(&vault, &mac, 100 + WAIT).back, vec!["devbox did not take \"Asleep\" within 5 minutes: added here".to_string()]);
        let filed = std::cell::RefCell::new(Vec::new());
        let file = |filing: &Filing| {
            filed.borrow_mut().push(filing.line.to_string());
            Ok(String::new())
        };
        receive(&vault, &mac, "mac", &file).unwrap();
        assert_eq!(filed.borrow().as_slice(), ["Asleep".to_string()], "filed here");
        receive(&vault, &devbox, "devbox", &file).unwrap();
        assert_eq!(filed.borrow().len(), 1, "devbox, waking, finds nothing to file");
        assert_eq!(reclaim(&vault, &mac, 100 + 2 * WAIT), Reclaimed::default(), "taken back once");
    }

    #[test]
    fn an_inbox_that_cannot_be_read_keeps_its_item_waiting_and_the_lead_hears_it_once() {
        let (vault, mac) = (Vault::default(), scratch("mac-unread"));
        vault.notes.borrow_mut().insert(crate::vault_machines::KEY.to_string(), (crate::vault_machines::render(&[machine("devbox", true, &[], 100)]), 1));
        send_with(&vault, &mac, &machine("mac", false, &[], 100), &filing("Waiting", ""), 100).unwrap().unwrap();
        let inbox = vault.notes.borrow_mut().remove(&inbox_key("devbox")).unwrap();
        let first = reclaim(&vault, &mac, 100 + WAIT);
        assert!(first.back.is_empty() && first.unreached.len() == 1, "{first:?}");
        assert_eq!(reclaim(&vault, &mac, 100 + WAIT + 30), Reclaimed::default(), "said once");
        assert!(pending(&mac).is_empty(), "nothing filed here on a failed read");
        vault.notes.borrow_mut().insert(inbox_key("devbox"), inbox);
        assert_eq!(reclaim(&vault, &mac, 100 + WAIT + 60).back.len(), 1, "taken back once the inbox reads and holds it");
    }

    #[test]
    fn a_failed_read_after_the_item_was_taken_files_nothing_twice() {
        let (vault, mac, devbox) = (Vault::default(), scratch("mac-taken-unread"), scratch("devbox-taken-unread"));
        vault.notes.borrow_mut().insert(crate::vault_machines::KEY.to_string(), (crate::vault_machines::render(&[machine("devbox", true, &[], 100)]), 1));
        send_with(&vault, &mac, &machine("mac", false, &[], 100), &filing("Taken", ""), 100).unwrap().unwrap();
        receive(&vault, &devbox, "devbox", &|_: &Filing| Ok(String::new())).unwrap();
        vault.notes.borrow_mut().remove(&inbox_key("devbox")).unwrap();
        let round = reclaim(&vault, &mac, 100 + WAIT);
        assert!(round.back.is_empty() && pending(&mac).is_empty(), "{round:?}");
        vault.notes.borrow_mut().insert(inbox_key("devbox"), (render("devbox", &[]), 9));
        assert_eq!(reclaim(&vault, &mac, 100 + WAIT + 30), Reclaimed::default());
        assert!(sent(&mac).is_empty(), "settled once the inbox reads without it");
    }

    #[test]
    fn an_item_sent_while_a_round_runs_stays_on_the_list() {
        let (vault, mac) = (Vault::default(), scratch("mac-race"));
        vault.notes.borrow_mut().insert(crate::vault_machines::KEY.to_string(), (crate::vault_machines::render(&[machine("devbox", true, &[], 100)]), 1));
        send_with(&vault, &mac, &machine("mac", false, &[], 100), &filing("Old", ""), 100).unwrap().unwrap();
        struct Sending<'a> {
            vault: &'a Vault,
            state: &'a Path,
            sent: std::cell::Cell<bool>,
        }
        impl Store for Sending<'_> {
            fn read(&self, key: &str) -> Result<(String, String), String> {
                if key == inbox_key("devbox") && !self.sent.replace(true) {
                    send_with(self.vault, self.state, &machine("mac", false, &[], 100 + WAIT), &filing("New", ""), 100 + WAIT).unwrap().unwrap();
                }
                self.vault.read(key)
            }
            fn write(&self, key: &str, etag: &str, text: &str) -> Result<Wrote, String> {
                self.vault.write(key, etag, text)
            }
            fn close(&self, key: &str) -> Result<String, String> {
                self.vault.close(key)
            }
        }
        let store = Sending { vault: &vault, state: &mac, sent: std::cell::Cell::new(false) };
        assert_eq!(reclaim(&store, &mac, 100 + WAIT).back.len(), 1, "the old one comes back");
        assert_eq!(sent(&mac).iter().map(|one| one.mail.line.as_str()).collect::<Vec<_>>(), ["New"], "the one sent meanwhile is still on the list");
    }

    #[test]
    fn an_item_is_kept_here_before_it_leaves_its_inbox_so_a_failed_save_loses_nothing() {
        let (vault, devbox) = (Vault::default(), scratch("devbox-unsaved"));
        post(&vault, "devbox", &mail_of(&filing("Kept", ""), "mac", 5)).unwrap();
        let unwritable = devbox.join("a-file");
        std::fs::write(&unwritable, "").unwrap();
        assert!(receive(&vault, &unwritable, "devbox", &|_: &Filing| Ok(String::new())).is_err());
        assert_eq!(read_inbox(&vault, "devbox", true).unwrap().0.len(), 1, "the receiver left it in its inbox");
    }

    #[test]
    fn an_item_is_kept_here_before_it_is_taken_back_so_a_failed_save_loses_nothing() {
        let (vault, mac) = (Vault::default(), scratch("mac-unsaved"));
        vault.notes.borrow_mut().insert(crate::vault_machines::KEY.to_string(), (crate::vault_machines::render(&[machine("devbox", true, &[], 100)]), 1));
        send_with(&vault, &mac, &machine("mac", false, &[], 100), &filing("Back", ""), 100).unwrap().unwrap();
        std::fs::create_dir_all(mac.join("idea-mail-pending.json")).unwrap();
        assert_eq!(reclaim(&vault, &mac, 100 + WAIT).failed.len(), 1, "the save failed");
        assert!(read_inbox(&vault, "devbox", true).unwrap().0.iter().any(|mail| mail.line == "Back"), "the sender left it in the inbox");
        std::fs::remove_dir_all(mac.join("idea-mail-pending.json")).unwrap();
        assert_eq!(reclaim(&vault, &mac, 100 + WAIT).back.len(), 1, "taken back once the save works");
    }

    struct Erring<'a> {
        vault: &'a Vault,
        saves: bool,
        reads: std::cell::Cell<bool>,
    }

    impl Store for Erring<'_> {
        fn read(&self, key: &str) -> Result<(String, String), String> {
            if !self.reads.get() && key.contains("inbox-") {
                return Err("timed out".to_string());
            }
            self.vault.read(key)
        }
        fn write(&self, key: &str, etag: &str, text: &str) -> Result<Wrote, String> {
            if key.contains("inbox-") {
                if self.saves {
                    self.vault.write(key, etag, text)?;
                }
                return Err("timed out".to_string());
            }
            self.vault.write(key, etag, text)
        }
        fn close(&self, key: &str) -> Result<String, String> {
            self.vault.close(key)
        }
    }

    fn lines_filed(state: &Path, me: &str, vault: &Vault) -> Vec<String> {
        let filed = std::cell::RefCell::new(Vec::new());
        receive(vault, state, me, &|filing: &Filing| {
            filed.borrow_mut().push(filing.line.to_string());
            Ok(String::new())
        }).unwrap();
        filed.into_inner()
    }

    #[test]
    fn a_take_back_whose_write_saved_but_answered_an_error_files_the_item_here_once_the_inbox_is_read() {
        for saves in [true, false] {
            let (vault, mac) = (Vault::default(), scratch(&format!("mac-erred-{saves}")));
            vault.notes.borrow_mut().insert(crate::vault_machines::KEY.to_string(), (crate::vault_machines::render(&[machine("devbox", true, &[], 100)]), 1));
            send_with(&vault, &mac, &machine("mac", false, &[], 100), &filing("Slow", ""), 100).unwrap().unwrap();
            let store = Erring { vault: &vault, saves, reads: std::cell::Cell::new(true) };
            let round = reclaim(&store, &mac, 100 + WAIT);
            assert!(round.back.is_empty() && round.failed.len() == 1, "{round:?}");
            assert!(lines_filed(&mac, "mac", &vault).is_empty(), "nothing is filed while it is unknown whether the write saved");
            assert_eq!(read_inbox(&vault, "devbox", true).unwrap().0.is_empty(), saves);
            store.reads.set(false);
            assert!(reclaim(&store, &mac, 100 + WAIT + 30).back.is_empty(), "an unread inbox keeps the copy held");
            let next = reclaim(&vault, &mac, 100 + WAIT + 60);
            assert_eq!(next.back.len(), 1, "taken back once, whether the earlier write saved or not: {next:?}");
            assert_eq!(lines_filed(&mac, "mac", &vault), ["Slow".to_string()], "saves={saves}");
            assert!(read_inbox(&vault, "devbox", true).unwrap().0.is_empty() && sent(&mac).is_empty() && held(&mac).is_empty());
        }
    }

    #[test]
    fn a_receive_whose_write_saved_but_answered_an_error_files_the_item_once_the_inbox_is_read() {
        for saves in [true, false] {
            let (vault, devbox) = (Vault::default(), scratch(&format!("devbox-erred-{saves}")));
            post(&vault, "devbox", &mail_of(&filing("Slow", ""), "mac", 5)).unwrap();
            let store = Erring { vault: &vault, saves, reads: std::cell::Cell::new(true) };
            assert!(receive(&store, &devbox, "devbox", &|_: &Filing| -> Result<String, String> { panic!("nothing is filed while the write is in doubt") }).is_err());
            assert_eq!(held(&devbox).len(), 1, "its copy is held");
            assert_eq!(lines_filed(&devbox, "devbox", &vault), ["Slow".to_string()], "filed once, whether the earlier write saved or not (saves={saves})");
            assert!(lines_filed(&devbox, "devbox", &vault).is_empty() && held(&devbox).is_empty() && read_inbox(&vault, "devbox", true).unwrap().0.is_empty());
        }
    }

    struct FirstReadFails<'a> {
        vault: &'a Vault,
        failing: std::cell::Cell<u32>,
    }

    impl Store for FirstReadFails<'_> {
        fn read(&self, key: &str) -> Result<(String, String), String> {
            if key.contains("inbox-") && self.failing.get() > 0 {
                self.failing.set(self.failing.get() - 1);
                return Err("timed out".to_string());
            }
            self.vault.read(key)
        }
        fn write(&self, key: &str, etag: &str, text: &str) -> Result<Wrote, String> {
            self.vault.write(key, etag, text)
        }
        fn close(&self, key: &str) -> Result<String, String> {
            self.vault.close(key)
        }
    }

    fn filed_through(store: &dyn Store, state: &Path, me: &str) -> Vec<String> {
        let filed = std::cell::RefCell::new(Vec::new());
        receive(store, state, me, &|filing: &Filing| {
            filed.borrow_mut().push(filing.line.to_string());
            Ok(String::new())
        }).unwrap();
        filed.into_inner()
    }

    #[test]
    fn a_held_copy_goes_once_the_usual_round_takes_the_same_item() {
        let (vault, devbox) = (Vault::default(), scratch("devbox-held-taken"));
        post(&vault, "devbox", &mail_of(&filing("Slow", ""), "mac", 5)).unwrap();
        let erring = Erring { vault: &vault, saves: false, reads: std::cell::Cell::new(true) };
        assert!(receive(&erring, &devbox, "devbox", &|_: &Filing| -> Result<String, String> { panic!("nothing is filed while the write is in doubt") }).is_err());
        assert_eq!(held(&devbox).len(), 1);
        let flaky = FirstReadFails { vault: &vault, failing: std::cell::Cell::new(1) };
        assert_eq!(filed_through(&flaky, &devbox, "devbox"), ["Slow".to_string()], "the held copy's read failed, the round's own read took it");
        assert!(held(&devbox).is_empty(), "the copy it held goes with it");
        assert!(lines_filed(&devbox, "devbox", &vault).is_empty(), "filed once");

        let (vault, mac) = (Vault::default(), scratch("mac-held-taken"));
        vault.notes.borrow_mut().insert(crate::vault_machines::KEY.to_string(), (crate::vault_machines::render(&[machine("devbox", true, &[], 100)]), 1));
        send_with(&vault, &mac, &machine("mac", false, &[], 100), &filing("Back", ""), 100).unwrap().unwrap();
        let erring = Erring { vault: &vault, saves: false, reads: std::cell::Cell::new(true) };
        assert_eq!(reclaim(&erring, &mac, 100 + WAIT).failed.len(), 1);
        let flaky = FirstReadFails { vault: &vault, failing: std::cell::Cell::new(1) };
        assert_eq!(reclaim(&flaky, &mac, 100 + WAIT + 30).back.len(), 1, "taken back by the round's own read");
        assert!(held(&mac).is_empty(), "the copy it held goes with it");
        assert!(reclaim(&vault, &mac, 100 + WAIT + 60).back.is_empty(), "not taken back twice");
        assert_eq!(lines_filed(&mac, "mac", &vault), ["Back".to_string()], "filed once");
    }

    #[test]
    fn the_receiver_files_with_the_repos_the_sender_knew() {
        let (vault, devbox) = (Vault::default(), scratch("devbox-repos"));
        let repos = ["kebbie".to_string(), "web".to_string()];
        post(&vault, "devbox", &mail_of(&Filing { repos: &repos, ..filing("No repo", "") }, "mac", 5)).unwrap();
        let seen = std::cell::RefCell::new(Vec::new());
        receive(&vault, &devbox, "devbox", &|filing: &Filing| {
            seen.borrow_mut().extend(filing.repos.iter().cloned());
            Ok(String::new())
        }).unwrap();
        assert_eq!(seen.borrow().as_slice(), repos);
    }

    #[test]
    fn an_item_the_ideas_machine_took_is_not_taken_back() {
        let (vault, mac, devbox) = (Vault::default(), scratch("mac-took"), scratch("devbox-took"));
        vault.notes.borrow_mut().insert(crate::vault_machines::KEY.to_string(), (crate::vault_machines::render(&[machine("devbox", true, &[], 100)]), 1));
        send_with(&vault, &mac, &machine("mac", false, &[], 100), &filing("Taken", ""), 100).unwrap().unwrap();
        receive(&vault, &devbox, "devbox", &|_: &Filing| Ok(String::new())).unwrap();
        assert_eq!(reclaim(&vault, &mac, 100 + WAIT), Reclaimed::default());
        let filed = std::cell::Cell::new(0);
        receive(&vault, &mac, "mac", &|_: &Filing| {
            filed.set(filed.get() + 1);
            Ok(String::new())
        }).unwrap();
        assert_eq!(filed.get(), 0, "the mac files nothing");
    }

    #[test]
    fn a_machine_name_with_spaces_has_an_inbox_key_the_vault_takes() {
        assert_eq!(inbox_key("Proxmox devbox"), "Kebbie/inbox-proxmox-devbox.md");
    }
}
