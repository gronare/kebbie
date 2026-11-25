use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::vault_card::Store;
use crate::vault_cli::Wrote;

pub const KEY: &str = "Kebbie/machines.md";
pub const EVERY: u64 = 60;
const REFRESH: u64 = 86_400;
const GONE_AFTER: u64 = 7 * 86_400;

// One machine sharing the vault: its machine.name, the repos it has checkouts of, whether hub.ideas_machine names it, and when it last said so.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub name: String,
    #[serde(default)]
    pub ideas: bool,
    #[serde(default)]
    pub repos: Vec<String>,
    #[serde(default)]
    pub seen: u64,
}

impl Machine {
    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }

    fn same_facts(&self, other: &Machine) -> bool {
        self.name == other.name && self.ideas == other.ideas && self.repos == other.repos
    }
}

fn kept_file(state: &Path) -> PathBuf {
    state.join("vault-machines.json")
}

fn kept(state: &Path) -> Option<Machine> {
    serde_json::from_str(&std::fs::read_to_string(kept_file(state)).ok()?).ok()
}

// The machines in the note's json block; a note without one lists none.
pub fn parse(text: &str) -> Vec<Machine> {
    let Some(start) = text.find("```json\n") else { return Vec::new() };
    let rest = &text[start + 8..];
    rest.find("\n```").and_then(|end| serde_json::from_str(&rest[..end]).ok()).unwrap_or_default()
}

pub fn render(machines: &[Machine]) -> String {
    let json = serde_json::to_string_pretty(machines).unwrap_or_else(|_| "[]".to_string());
    format!("---\ntitle: \"kebbie's machines\"\ntags: [kebbie, machines]\nstatus: active\n---\n\nEach machine sharing this vault keeps its own entry here: the repos it has checkouts of and whether it is the ideas machine. kebbie sends a backlog item added on one machine to the machine these entries pick.\n\n```json\n{json}\n```\n")
}

// The note's machines with this one's entry in place of its old one, and machines silent for a week left out.
pub fn with(machines: &[Machine], me: &Machine, now: u64) -> Vec<Machine> {
    let mut all: Vec<Machine> = machines.iter().filter(|one| !one.is(&me.name) && one.seen + GONE_AFTER > now).cloned().collect();
    all.push(me.clone());
    all.sort_by_key(|one| one.name.to_lowercase());
    all
}

// This machine's entry written when its facts changed or a day passed; a write that lost to another machine's is tried again next round.
pub fn sync(store: &dyn Store, state: &Path, me: &Machine) -> Result<bool, String> {
    if kept(state).is_some_and(|last| last.same_facts(me) && last.seen + REFRESH > me.seen) {
        return Ok(false);
    }
    // The vault answers a missing note as any failed read; a create over one that exists after all loses as a stale write.
    let (text, etag) = match store.read(KEY) {
        Ok(read) => read,
        Err(_) => (String::new(), "*".to_string()),
    };
    match store.write(KEY, &etag, &render(&with(&parse(&text), me, me.seen))) {
        Ok(Wrote::Saved) => {
            crate::state::write(&kept_file(state), serde_json::to_string(me).unwrap_or_default())?;
            Ok(true)
        }
        Ok(Wrote::Stale) => Ok(false),
        Err(error) => Err(format!("{KEY} was not written ({error})")),
    }
}

// This machine as the other machines see it.
pub fn me(now: u64) -> Machine {
    let mut repos: Vec<String> = crate::repo::pooled_checkouts().iter().map(|repo| crate::repo::name_of(repo)).collect();
    repos.sort();
    repos.dedup();
    Machine { name: crate::config::machine(), ideas: crate::config::is_idea_machine(), repos, seen: now }
}

pub fn writes(cli: &crate::vault_cli::Cli) -> bool {
    cli.capabilities.as_ref().is_some_and(|stated| stated.iter().any(|one| one == crate::vault_cli::WRITE))
}

pub fn tick(state: &Path) -> Vec<String> {
    let Some(cli) = crate::vault_cli::detect().filter(writes) else { return Vec::new() };
    match sync(&cli, state, &me(crate::messages::now())) {
        Ok(_) => Vec::new(),
        Err(line) => {
            crate::vault_cli::warn_log(state, &line);
            vec![line]
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    // A vault of notes by key, each with an etag that changes on every write.
    #[derive(Default)]
    pub struct Vault {
        pub notes: RefCell<HashMap<String, (String, u32)>>,
        pub writes: RefCell<u32>,
    }

    impl Store for Vault {
        fn read(&self, key: &str) -> Result<(String, String), String> {
            self.notes.borrow().get(key).map(|(text, etag)| (text.clone(), etag.to_string())).ok_or_else(|| format!("mindin: Error executing tool read_file: {key}"))
        }

        fn write(&self, key: &str, etag: &str, text: &str) -> Result<Wrote, String> {
            *self.writes.borrow_mut() += 1;
            let mut notes = self.notes.borrow_mut();
            let current = notes.get(key).map(|(_, etag)| etag.to_string());
            if current.as_deref() != Some(etag) && !(etag == "*" && current.is_none()) {
                return Ok(Wrote::Stale);
            }
            let next = current.and_then(|etag| etag.parse::<u32>().ok()).unwrap_or(0) + 1;
            notes.insert(key.to_string(), (text.to_string(), next));
            Ok(Wrote::Saved)
        }

        fn close(&self, key: &str) -> Result<String, String> {
            self.notes.borrow_mut().remove(key).map(|_| key.to_string()).ok_or_else(|| format!("mindin: Error executing tool close: {key}"))
        }
    }

    pub fn machine(name: &str, ideas: bool, repos: &[&str], seen: u64) -> Machine {
        Machine { name: name.to_string(), ideas, repos: repos.iter().map(|repo| repo.to_string()).collect(), seen }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-vault-machines-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn each_machine_keeps_its_own_entry_and_leaves_the_others_as_they_are() {
        let (vault, mac, devbox) = (Vault::default(), scratch("mac"), scratch("devbox"));
        assert!(sync(&vault, &mac, &machine("mac", false, &["kebbie", "web"], 1_000)).unwrap(), "the first machine creates the note");
        assert!(sync(&vault, &devbox, &machine("devbox", true, &["kebbie"], 1_100)).unwrap());
        let listed = parse(&vault.read(KEY).unwrap().0);
        assert_eq!(listed, vec![machine("devbox", true, &["kebbie"], 1_100), machine("mac", false, &["kebbie", "web"], 1_000)]);
        assert!(!sync(&vault, &mac, &machine("mac", false, &["kebbie", "web"], 1_200)).unwrap(), "unchanged facts within a day are not written again");
        assert!(sync(&vault, &mac, &machine("mac", false, &["kebbie"], 1_300)).unwrap(), "a repo gone is written at once");
        assert!(sync(&vault, &mac, &machine("mac", false, &["kebbie"], 1_300 + REFRESH)).unwrap(), "a day on, the entry is refreshed");
        assert_eq!(parse(&vault.read(KEY).unwrap().0).len(), 2);
    }

    #[test]
    fn a_write_that_loses_to_another_machine_is_tried_again_and_a_silent_machine_drops_out() {
        let (vault, mac) = (Vault::default(), scratch("lost"));
        vault.notes.borrow_mut().insert(KEY.to_string(), (render(&[machine("old", true, &[], 0)]), 7));
        let me = machine("mac", false, &["web"], GONE_AFTER + 1);
        assert_eq!(with(&parse(&vault.read(KEY).unwrap().0), &me, me.seen), vec![me.clone()], "a machine silent for a week is not listed");
        struct Racing<'a>(&'a Vault);
        impl Store for Racing<'_> {
            fn read(&self, key: &str) -> Result<(String, String), String> {
                let read = self.0.read(key);
                self.0.notes.borrow_mut().get_mut(key).unwrap().1 += 1;
                read
            }
            fn write(&self, key: &str, etag: &str, text: &str) -> Result<Wrote, String> {
                self.0.write(key, etag, text)
            }
            fn close(&self, key: &str) -> Result<String, String> {
                self.0.close(key)
            }
        }
        assert!(!sync(&Racing(&vault), &mac, &me).unwrap(), "the stale write is not kept as written");
        assert!(sync(&vault, &mac, &me).unwrap(), "the next round writes it");
    }

    #[test]
    fn a_note_edited_by_hand_without_its_block_lists_no_machines() {
        assert!(parse("---\ntitle: x\n---\n\nsome words\n").is_empty());
        assert_eq!(parse(&render(&[machine("mac", false, &["web"], 5)])), vec![machine("mac", false, &["web"], 5)]);
    }
}
