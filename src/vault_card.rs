use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::status::{Group, Item};
use crate::vault_cli::{Cli, Wrote};

pub const FOLDER: &str = "Kebbie";
pub const MARKER: &str = "kebbie-card";
pub const EVERY: u64 = 30;

pub trait Store {
    fn read(&self, key: &str) -> Result<(String, String), String>;
    fn write(&self, key: &str, etag: &str, text: &str) -> Result<Wrote, String>;
    fn close(&self, key: &str) -> Result<String, String>;
}

impl Store for Cli {
    fn read(&self, key: &str) -> Result<(String, String), String> {
        self.read_note(key)
    }

    fn write(&self, key: &str, etag: &str, text: &str) -> Result<Wrote, String> {
        self.write_note(key, etag, text)
    }

    fn close(&self, key: &str) -> Result<String, String> {
        Cli::close(self, key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Push {
    Ready,
    Pushed,
    Waiting,
}

impl Push {
    fn word(self) -> &'static str {
        match self {
            Push::Ready => "ready",
            Push::Pushed => "pushed",
            Push::Waiting => "waiting",
        }
    }

    pub fn of(group: Group) -> Push {
        match group {
            Group::ReadyToPush => Push::Ready,
            Group::CiRunning => Push::Pushed,
            _ => Push::Waiting,
        }
    }
}

// What kebbie last wrote for a task: the note's key, the card's text and the push state, so an unchanged card is never written again.
#[derive(Clone, Debug, PartialEq)]
struct Kept {
    key: String,
    up: String,
    title: String,
    text: String,
    push: String,
}

fn kept_file(state: &Path, address: &str) -> PathBuf {
    state.join("vault-cards").join(format!("{address}.json"))
}

fn load(state: &Path, address: &str) -> Option<Kept> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(kept_file(state, address)).ok()?).ok()?;
    let field = |name: &str| value[name].as_str().map(str::to_string);
    Some(Kept { key: field("key")?, up: field("up")?, title: field("title")?, text: field("text")?, push: field("push")? })
}

fn save(state: &Path, address: &str, kept: &Kept) {
    let value = json!({ "key": kept.key, "up": kept.up, "title": kept.title, "text": kept.text, "push": kept.push });
    if let Err(error) = crate::state::write(&kept_file(state, address), value.to_string()) {
        crate::vault_cli::warn_log(state, &format!("{address}: the card's vault record was not kept ({error})"));
    }
}

fn forget(state: &Path, address: &str) {
    let _ = std::fs::remove_file(kept_file(state, address));
    let _ = std::fs::remove_file(said_file(state, address));
}

fn stem(note: &str) -> &str {
    let name = note.rsplit('/').next().unwrap_or(note);
    name.strip_suffix(".md").unwrap_or(name)
}

// One card note per task half: the task note's stem, and the repo when the stem does not already start with it.
pub fn key_for(note: &str, address: &str) -> String {
    let stem = stem(note);
    let repo = address.rsplit_once('@').map_or("", |(_, repo)| repo);
    if repo.is_empty() || stem.starts_with(&format!("{repo}-")) {
        format!("{FOLDER}/{stem}-card.md")
    } else {
        format!("{FOLDER}/{stem}-card-{repo}.md")
    }
}

fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

fn render(kept: &Kept, address: &str, date: Option<&str>) -> String {
    let date = date.map(|date| format!("date: {date}\n")).unwrap_or_default();
    format!(
        "---\ntitle: {}\n{date}tags: [kebbie, card]\nstatus: active\nup: {}\n{MARKER}: {}\npush: {}\n---\n\n{}\n",
        quoted(&kept.title), quoted(&format!("[[{}]]", stem(&kept.up))), quoted(address), kept.push, kept.text.trim_end()
    )
}

fn frontmatter_value(text: &str, name: &str) -> Option<String> {
    let rest = text.strip_prefix("---\n")?;
    let front = &rest[..rest.find("\n---")?];
    front.lines().find_map(|line| line.strip_prefix(&format!("{name}:"))).map(|value| value.trim().to_string())
}

// The card note kebbie writes for this address, and nothing else: a note without the marker naming this address is left as it is.
fn ours(text: &str, address: &str) -> bool {
    frontmatter_value(text, MARKER).is_some_and(|value| value.trim_matches(['"', '\'']) == address)
}

fn body(text: &str) -> &str {
    let after = text.strip_prefix("---\n").and_then(|rest| rest.find("\n---").map(|end| &rest[end + 4..])).unwrap_or(text);
    after.trim()
}

// The stored note with only its frontmatter `push:` line set to the wanted state; None when it has no such line.
fn with_push(stored: &str, push: &str) -> Option<String> {
    let rest = stored.strip_prefix("---\n")?;
    let end = rest.find("\n---")? + 4;
    let mut found = false;
    let front: Vec<String> = rest[..end - 4].lines().map(|line| match line.strip_prefix("push:") {
        Some(_) if !found => {
            found = true;
            format!("push: {push}")
        }
        _ => line.to_string(),
    }).collect();
    found.then(|| format!("---\n{}{}", front.join("\n"), &rest[end - 4..]))
}

// None when nothing needed saying; a line when the vault refused or a note in the way is not kebbie's, or not as kebbie left it, said once until it changes.
fn sync(store: &dyn Store, state: &Path, address: &str, wanted: Kept, create: bool) -> Option<String> {
    let last = load(state, address);
    if last.as_ref() == Some(&wanted) {
        return None;
    }
    let (written, fresh) = match store.read(&wanted.key) {
        Ok((stored, _)) if !ours(&stored, address) => return once(state, address, format!("{}: not kebbie's card, so the card was not written there", wanted.key)),
        Ok((stored, _)) if last.as_ref().is_none_or(|last| body(&stored) != last.text.trim()) => return once(state, address, format!("{}: the card note was changed outside kebbie, so kebbie leaves it as it is", wanted.key)),
        Ok((stored, etag)) if last.as_ref().is_some_and(|last| last.text == wanted.text && last.title == wanted.title && last.up == wanted.up) => match with_push(&stored, &wanted.push) {
            Some(patched) => (store.write(&wanted.key, &etag, &patched), false),
            None => return once(state, address, format!("{}: the card note has no push line, so kebbie leaves it as it is", wanted.key)),
        },
        Ok((stored, etag)) => (store.write(&wanted.key, &etag, &render(&wanted, address, frontmatter_value(&stored, "date").as_deref())), false),
        Err(_) if !create => return None,
        Err(_) => (store.write(&wanted.key, "*", &render(&wanted, address, None)), true),
    };
    match written {
        Ok(Wrote::Saved) => {
            save(state, address, &wanted);
            let _ = std::fs::remove_file(said_file(state, address));
            None
        }
        Ok(Wrote::Stale) if fresh => lost_create(store, state, address, &wanted),
        Ok(Wrote::Stale) => None,
        Err(error) => once(state, address, format!("{}: the card was not written ({error})", wanted.key)),
    }
}

// A create that lost to a note written meanwhile: kebbie's own card from another round is kept as written, anyone else's is said once.
fn lost_create(store: &dyn Store, state: &Path, address: &str, wanted: &Kept) -> Option<String> {
    match store.read(&wanted.key) {
        Ok((stored, _)) if ours(&stored, address) && body(&stored) == wanted.text.trim() => {
            save(state, address, wanted);
            None
        }
        Ok((stored, _)) if ours(&stored, address) => None,
        Ok(_) => once(state, address, format!("{}: not kebbie's card, so the card was not written there", wanted.key)),
        Err(error) => once(state, address, format!("{}: the card was not written, as the vault holds a note there kebbie could not read ({error})", wanted.key)),
    }
}

fn said_file(state: &Path, address: &str) -> PathBuf {
    state.join("vault-cards").join(format!("{address}.said"))
}

fn once(state: &Path, address: &str, line: String) -> Option<String> {
    let file = said_file(state, address);
    if std::fs::read_to_string(&file).is_ok_and(|said| said == line) {
        return None;
    }
    let _ = crate::state::write(&file, &line);
    Some(line)
}

fn wanted_for(item: &Item, state: &Path, note: &str, card: impl Fn(&str) -> Option<String>) -> Option<(Kept, bool)> {
    let address = crate::peers::address(&item.name, &item.place);
    let push = Push::of(item.group());
    let kept = load(state, &address);
    if push == Push::Ready {
        let text = card(&address).filter(|text| !text.trim().is_empty())?;
        let title = item.title.clone().unwrap_or_else(|| item.name.clone());
        return Some((Kept { key: key_for(note, &address), up: note.to_string(), title, text, push: push.word().to_string() }, true));
    }
    kept.map(|kept| (Kept { push: push.word().to_string(), ..kept }, false))
}

// The board's push-ready column decides: a task there gets its card written, one that left it only has the push state of a card already written changed.
pub fn sync_items(store: &dyn Store, state: &Path, items: &[Item], note: impl Fn(&str) -> Option<String>, card: impl Fn(&str) -> Option<String>) -> Vec<String> {
    items.iter().filter(|item| item.is_task() && item.routine.is_none() && item.queued.is_none()).filter_map(|item| {
        let address = crate::peers::address(&item.name, &item.place);
        let note = note(&address)?;
        let (wanted, create) = wanted_for(item, state, &note, &card)?;
        sync(store, state, &address, wanted, create).map(|line| format!("{address}: {line}"))
    }).collect()
}

fn writes(cli: &Cli) -> bool {
    cli.capabilities.as_ref().is_some_and(|stated| stated.iter().any(|one| one == crate::vault_cli::WRITE))
}

fn due(items: &[Item], state: &Path) -> bool {
    items.iter().any(|item| {
        let address = crate::peers::address(&item.name, &item.place);
        crate::task_note::linked(state, &address).is_some() && (item.group() == Group::ReadyToPush || load(state, &address).is_some())
    })
}

pub fn sync_with(state: &Path, items: &[Item]) -> Vec<String> {
    if !due(items, state) {
        return Vec::new();
    }
    let Some(cli) = crate::vault_cli::detect().filter(writes) else { return Vec::new() };
    let lines = sync_items(&cli, state, items, |address| crate::task_note::linked(state, address), |address| crate::report::card_in(state, address).map(|card| card.text));
    for line in &lines {
        crate::vault_cli::warn_log(state, line);
    }
    lines
}

// An open board without the keeper syncs from its own snapshot, apart from its refresh, one round at a time.
pub fn sync_apart(state: PathBuf, items: Vec<Item>) {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        sync_with(&state, &items);
        RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

pub fn tick(state: &Path) -> Vec<String> {
    sync_with(state, &crate::status::gather().unwrap_or_default())
}

// A task that ends takes its card note with it into the vault's archive; None when there was nothing to archive or it went.
pub fn archive(store: &dyn Store, state: &Path, address: &str) -> Option<String> {
    let kept = load(state, address)?;
    forget(state, address);
    let said = match store.read(&kept.key) {
        Ok((stored, _)) if ours(&stored, address) => store.close(&kept.key).err().map(|error| format!("{} was not archived ({error})", kept.key)),
        _ => None,
    };
    if let Some(line) = &said {
        crate::vault_cli::warn_log(state, &format!("{address}: {line}"));
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phase::Phase;
    use crate::status::tests::{in_phase, task};
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Vault {
        notes: RefCell<HashMap<String, (String, u32)>>,
        writes: RefCell<Vec<String>>,
        closed: RefCell<Vec<String>>,
    }

    impl Store for Vault {
        fn read(&self, key: &str) -> Result<(String, String), String> {
            self.notes.borrow().get(key).map(|(text, version)| (text.clone(), version.to_string())).ok_or_else(|| format!("mindin: no such note: {key}"))
        }

        fn write(&self, key: &str, etag: &str, text: &str) -> Result<Wrote, String> {
            let mut notes = self.notes.borrow_mut();
            let current = notes.get(key).map(|(_, version)| version.to_string());
            if current.as_deref() != Some(etag) && !(etag == "*" && current.is_none()) {
                return Ok(Wrote::Stale);
            }
            let version = notes.get(key).map_or(1, |(_, version)| version + 1);
            notes.insert(key.to_string(), (text.to_string(), version));
            self.writes.borrow_mut().push(key.to_string());
            Ok(Wrote::Saved)
        }

        fn close(&self, key: &str) -> Result<String, String> {
            let (text, version) = self.notes.borrow_mut().remove(key).ok_or("no such note")?;
            let archived = format!("Archive/{}", key.rsplit('/').next().unwrap_or(key));
            self.notes.borrow_mut().insert(archived.clone(), (text.replace("status: active", "status: complete"), version));
            self.closed.borrow_mut().push(key.to_string());
            Ok(archived)
        }
    }

    fn state(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-vault-card-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at(phase: Phase) -> Item {
        let mut item = task(Some("idle"), Some("ready"), 2);
        in_phase(&mut item, phase);
        item
    }

    fn ready() -> Item {
        at(Phase::Ready { head: Some("abc".to_string()) })
    }

    const NOTE: &str = "Projects/web-billing.md";
    const KEY: &str = "Kebbie/web-billing-card.md";

    fn run(vault: &Vault, state: &Path, item: &Item, card: &str) -> Vec<String> {
        sync_items(vault, state, std::slice::from_ref(item), |_| Some(NOTE.to_string()), |_| Some(card.to_string()))
    }

    #[test]
    fn a_push_ready_task_gets_one_card_note_linked_up_to_its_task_note() {
        let (vault, state) = (Vault::default(), state("ready"));
        assert!(run(&vault, &state, &ready(), "## Built\nThe board shows cards.").is_empty());
        let (text, _) = vault.read(KEY).unwrap();
        assert!(text.starts_with("---\ntitle: \"billing\"\ntags: [kebbie, card]\nstatus: active\nup: \"[[web-billing]]\"\nkebbie-card: \"billing@web\"\npush: ready\n---\n\n## Built\nThe board shows cards.\n"), "{text}");
        assert_eq!(*vault.writes.borrow(), [KEY]);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn ready_then_red_then_ready_keeps_one_note_and_writes_only_what_changed() {
        let (vault, state) = (Vault::default(), state("cycle"));
        let card = "## Built\nThe board shows cards.";
        run(&vault, &state, &ready(), card);
        run(&vault, &state, &ready(), card);
        assert_eq!(vault.writes.borrow().len(), 1, "an unchanged card is not written again");
        run(&vault, &state, &at(Phase::CiRed { sha: "abc".to_string() }), card);
        assert!(vault.read(KEY).unwrap().0.contains("push: waiting\n"), "leaving push-ready changes the state field in the same note");
        run(&vault, &state, &at(Phase::CiRed { sha: "abc".to_string() }), "a card rewritten while red");
        run(&vault, &state, &ready(), card);
        assert!(vault.read(KEY).unwrap().0.contains("push: ready\n"));
        run(&vault, &state, &ready(), card);
        assert_eq!(vault.writes.borrow().len(), 3, "red and ready again, nothing more");
        assert_eq!(vault.notes.borrow().len(), 1, "one note for the task through every round");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_task_waiting_on_a_question_gets_no_card_note() {
        let (vault, state) = (Vault::default(), state("question"));
        let mut asking = ready();
        asking.question = Some(crate::messages::fold(r#"{"id":"q1","at":10,"from":"billing@web","to":"you","text":"Which queue?","state":"asked","question":{"options":[{"label":"A","meaning":"one"},{"label":"B","meaning":"two"}],"recommend":1}}"#).remove(0));
        assert_ne!(asking.group(), Group::ReadyToPush);
        run(&vault, &state, &asking, "## Built\nA card written before the question.");
        assert!(vault.notes.borrow().is_empty() && vault.writes.borrow().is_empty(), "a card reported over a waiting question is no push-ready");
        run(&vault, &state, &at(Phase::Working), "## Built\nStill working.");
        assert!(vault.notes.borrow().is_empty(), "leaving a state never creates a note");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_note_in_the_way_that_kebbie_did_not_write_is_left_as_it_is() {
        let (vault, state) = (Vault::default(), state("theirs"));
        vault.notes.borrow_mut().insert(KEY.to_string(), ("---\ntitle: Mine\n---\n\nThe user's own words.\n".to_string(), 1));
        let said = run(&vault, &state, &ready(), "## Built\nX.");
        assert_eq!(said, ["billing@web: Kebbie/web-billing-card.md: not kebbie's card, so the card was not written there"]);
        assert!(vault.writes.borrow().is_empty());
        assert!(run(&vault, &state, &ready(), "## Built\nX.").is_empty(), "the refusal is said once, not every round");
        vault.notes.borrow_mut().insert(KEY.to_string(), ("---\ntitle: Other\nkebbie-card: refunds@web\n---\n\nAnother task's card.\n".to_string(), 1));
        run(&vault, &state, &ready(), "## Built\nX.");
        assert!(vault.writes.borrow().is_empty(), "another task's card is not this one's either");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_rewrite_keeps_the_stored_date_and_reads_the_marker_however_the_vault_quoted_it() {
        let (vault, state) = (Vault::default(), state("date"));
        vault.notes.borrow_mut().insert(KEY.to_string(), ("---\ntitle: billing\ndate: '2026-11-01'\nkebbie-card: billing@web\n---\n\nold\n".to_string(), 4));
        save(&state, "billing@web", &Kept { key: KEY.to_string(), up: NOTE.to_string(), title: "billing".to_string(), text: "old".to_string(), push: "ready".to_string() });
        assert!(run(&vault, &state, &ready(), "new").is_empty());
        let (text, _) = vault.read(KEY).unwrap();
        assert!(text.contains("date: '2026-11-01'\n") && text.ends_with("\n\nnew\n"), "{text}");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_card_note_the_user_wrote_in_is_never_rewritten_and_said_once() {
        let (vault, state) = (Vault::default(), state("edited"));
        run(&vault, &state, &ready(), "## Built\nX.");
        let edited = vault.read(KEY).unwrap().0.replace("X.\n", "X.\n\nMy own notes on this.\n");
        vault.notes.borrow_mut().insert(KEY.to_string(), (edited.clone(), 7));
        let said = run(&vault, &state, &ready(), "## Built\nY.");
        assert_eq!(said, ["billing@web: Kebbie/web-billing-card.md: the card note was changed outside kebbie, so kebbie leaves it as it is"]);
        assert!(run(&vault, &state, &at(Phase::CiRed { sha: "abc".to_string() }), "## Built\nY.").is_empty(), "said once");
        assert_eq!(vault.read(KEY).unwrap().0, edited);
        assert_eq!(vault.writes.borrow().len(), 1);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_card_note_without_kebbies_record_is_not_rewritten() {
        let (vault, state) = (Vault::default(), state("unrecorded"));
        let stored = "---\ntitle: billing\nkebbie-card: billing@web\npush: ready\n---\n\nSomething kebbie cannot vouch for.\n";
        vault.notes.borrow_mut().insert(KEY.to_string(), (stored.to_string(), 1));
        assert_eq!(run(&vault, &state, &ready(), "new").len(), 1);
        assert_eq!(vault.read(KEY).unwrap().0, stored);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn leaving_push_ready_patches_only_the_push_line() {
        let (vault, state) = (Vault::default(), state("patch"));
        run(&vault, &state, &ready(), "## Built\nX.");
        let tagged = vault.read(KEY).unwrap().0.replace("tags: [kebbie, card]", "tags: [kebbie, card, mine]\nrating: 5");
        vault.notes.borrow_mut().insert(KEY.to_string(), (tagged.clone(), 3));
        run(&vault, &state, &at(Phase::CiRed { sha: "abc".to_string() }), "## Built\nX.");
        assert_eq!(vault.read(KEY).unwrap().0, tagged.replace("push: ready", "push: waiting"));
        let _ = std::fs::remove_dir_all(&state);
    }

    struct Racing {
        vault: Vault,
        meanwhile: String,
    }

    impl Store for Racing {
        fn read(&self, key: &str) -> Result<(String, String), String> {
            self.vault.read(key)
        }

        fn write(&self, key: &str, etag: &str, text: &str) -> Result<Wrote, String> {
            if etag == "*" && self.vault.read(key).is_err() {
                self.vault.notes.borrow_mut().insert(key.to_string(), (self.meanwhile.clone(), 1));
            }
            self.vault.write(key, etag, text)
        }

        fn close(&self, key: &str) -> Result<String, String> {
            self.vault.close(key)
        }
    }

    #[test]
    fn a_create_that_loses_to_kebbies_own_card_says_nothing_and_to_another_note_says_whose() {
        let state = state("race");
        let own = Racing { vault: Vault::default(), meanwhile: "---\nkebbie-card: \"billing@web\"\npush: ready\n---\n\n## Built\nX.\n".to_string() };
        assert!(sync_items(&own, &state, std::slice::from_ref(&ready()), |_| Some(NOTE.to_string()), |_| Some("## Built\nX.".to_string())).is_empty());
        assert!(run(&own.vault, &state, &ready(), "## Built\nX.").is_empty() && own.vault.writes.borrow().is_empty(), "the card the other round wrote is kept as kebbie's");
        let _ = std::fs::remove_dir_all(&state);
        let state = self::state("race-theirs");
        let theirs = Racing { vault: Vault::default(), meanwhile: "---\ntitle: Mine\n---\n\nMine.\n".to_string() };
        let said = sync_items(&theirs, &state, std::slice::from_ref(&ready()), |_| Some(NOTE.to_string()), |_| Some("## Built\nX.".to_string()));
        assert_eq!(said, ["billing@web: Kebbie/web-billing-card.md: not kebbie's card, so the card was not written there"]);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_task_that_ends_archives_its_card_note_and_forgets_it() {
        let (vault, state) = (Vault::default(), state("archive"));
        run(&vault, &state, &ready(), "## Built\nX.");
        assert_eq!(archive(&vault, &state, "billing@web"), None);
        assert_eq!(*vault.closed.borrow(), [KEY]);
        assert!(vault.read("Archive/web-billing-card.md").unwrap().0.contains("status: complete"));
        assert_eq!(archive(&vault, &state, "billing@web"), None, "nothing kept, nothing to archive");
        assert_eq!(vault.closed.borrow().len(), 1);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_card_note_is_keyed_by_its_task_note_and_repo() {
        assert_eq!(key_for("Projects/web-billing.md", "billing@web"), "Kebbie/web-billing-card.md");
        assert_eq!(key_for("Projects/web-billing.md", "billing@api"), "Kebbie/web-billing-card-api.md", "a second repo's half has its own card");
    }

    #[test]
    fn the_keeper_runs_the_card_sync() {
        assert!(crate::keeper::routines().iter().any(|routine| routine.name == "cards" && routine.every == EVERY));
    }
}
