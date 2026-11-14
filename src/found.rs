use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::permit::Caller;
use crate::sh::Result;

pub const LABEL: &str = "Also noticed:";
pub const SECTION: &str = "Found by tasks";
const MARK: &str = "found:";

fn file(state: &Path) -> PathBuf {
    state.join("found.json")
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    Started,
    Added,
    Dismissed,
    Kept,
}

// What tasks noticed outside their own work, filed once however many tasks found it: each task that found it is a source, and the count is how often it was found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub line: String,
    pub repo: String,
    pub sources: Vec<String>,
    pub count: u32,
    pub first_at: u64,
    pub last_at: u64,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub raised: bool,
}

impl Item {
    // Kept in the backlog is still open: the user can start or add it later.
    pub fn open(&self) -> bool {
        matches!(self.state, State::Open | State::Kept)
    }

    // A second task found it, so it is a problem of its own rather than something each task works around.
    pub fn recurring(&self) -> bool {
        self.sources.len() >= 2
    }

    // What the board's row says after the line: how often and by which tasks.
    pub fn found_by(&self) -> String {
        let tasks: Vec<&str> = self.sources.iter().map(|source| source.split_once('@').map_or(source.as_str(), |(task, _)| task)).collect();
        let times = if self.count > 1 { format!("found {}× · ", self.count) } else { String::new() };
        format!("{times}by {}", tasks.join(", "))
    }

    pub fn prompt(&self) -> String {
        format!("{}\n\nFound outside their own work by {} ({}).", self.line, self.sources.join(", "), if self.count == 1 { "once".to_string() } else { format!("{} times", self.count) })
    }

    // A name for a task started from it: the line's first words as a slug.
    pub fn name(&self) -> String {
        let words: Vec<String> = normal(&self.line).split(' ').filter(|word| word.len() > 2).take(3).map(str::to_string).collect();
        let name = words.join("-");
        if name.is_empty() { format!("found-{}", self.id) } else { name }
    }

    fn to_json(&self) -> serde_json::Value {
        json!({ "id": self.id, "line": self.line, "repo": self.repo, "sources": self.sources, "count": self.count, "first_at": self.first_at, "last_at": self.last_at, "state": self.state, "outcome": self.outcome, "recurring": self.recurring() })
    }
}

pub fn load(state: &Path) -> Vec<Item> {
    fs::read_to_string(file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub fn open_in(state: &Path) -> Vec<Item> {
    load(state).into_iter().filter(Item::open).collect()
}

fn save(state: &Path, items: &[Item]) -> Result<()> {
    crate::state::write(&file(state), serde_json::to_string_pretty(items).map_err(|error| error.to_string())?)
}

fn normal(text: &str) -> String {
    text.split(|character: char| !character.is_alphanumeric()).filter(|word| !word.is_empty()).map(str::to_lowercase).collect::<Vec<_>>().join(" ")
}

// The line without kebbie's own mark and without where the task said it filed it.
fn what(line: &str) -> String {
    let line = match line.rfind(&format!("({MARK}")) {
        Some(at) => &line[..at],
        None => line,
    };
    let line = line.trim().trim_end_matches('.');
    let lower = line.to_lowercase();
    let cut = [": filed", " — filed", " - filed", "; filed", ", filed"].iter().filter_map(|tail| lower.rfind(tail)).min();
    cut.map_or(line, |at| &line[..at]).trim().to_string()
}

fn named_id(line: &str) -> Option<String> {
    let at = line.rfind(MARK)?;
    let id: String = line[at + MARK.len()..].chars().take_while(|character| character.is_ascii_alphanumeric()).collect();
    (!id.is_empty()).then_some(id)
}

// Files one line from a task: the item its found: mark names, or an open one with the same words, counts it once more for a new task; else a new item.
pub fn file_line(items: &mut Vec<Item>, address: &str, line: &str, now: u64) -> String {
    let what = what(line);
    let repo = address.split_once('@').map_or("", |(_, repo)| repo).to_string();
    let found = named_id(line).and_then(|id| items.iter().position(|item| item.id == id))
        .or_else(|| items.iter().position(|item| item.open() && item.repo == repo && normal(&item.line) == normal(&what)));
    match found {
        Some(at) => {
            let item = &mut items[at];
            if !item.sources.iter().any(|source| source == address) {
                item.sources.push(address.to_string());
                item.count += 1;
                item.last_at = now;
            }
            item.id.clone()
        }
        None => {
            let id = format!("f{}", items.iter().filter_map(|item| item.id.strip_prefix('f')?.parse::<u32>().ok()).max().unwrap_or(0) + 1);
            items.push(Item { id: id.clone(), line: what, repo, sources: vec![address.to_string()], count: 1, first_at: now, last_at: now, state: State::Open, outcome: None, raised: false });
            id
        }
    }
}

fn label(line: &str) -> Option<&str> {
    let bare = line.trim().trim_matches('*').trim();
    let (head, rest) = bare.split_once(':')?;
    (rest.trim().is_empty() && !head.is_empty() && head.chars().all(|character| character.is_alphabetic() || character == ' ')).then_some(head)
}

// The card as filed, and each item it made recurring: found by a second task, and not yet raised as a problem of its own.
pub struct Filed {
    pub text: String,
    pub recurring: Vec<Item>,
}

// The card's Also noticed lines are filed under Found by tasks as the card is taken, and each line ends with the item it went to; an item a second task finds is marked raised once, for its caller to raise.
pub fn file_card(state: &Path, address: &str, card: &str, now: u64) -> Result<Filed> {
    let mut items = load(state);
    let before = items.clone();
    let mut inside = false;
    let lines: Vec<String> = card.lines().map(|line| {
        if let Some(head) = label(line) {
            inside = format!("{head}:").eq_ignore_ascii_case(LABEL);
            return line.to_string();
        }
        let Some(item) = line.trim_start().strip_prefix("- ").filter(|_| inside) else { return line.to_string() };
        if item.trim().is_empty() {
            return line.to_string();
        }
        let id = file_line(&mut items, address, item, now);
        if named_id(item).is_some() { line.to_string() } else { format!("{} ({MARK}{id})", line.trim_end()) }
    }).collect();
    let mut recurring = Vec::new();
    for item in items.iter_mut().filter(|item| item.open() && item.recurring() && !item.raised && item.sources.iter().any(|source| source == address)) {
        item.raised = true;
        recurring.push(item.clone());
    }
    if items != before {
        save(state, &items)?;
    }
    Ok(Filed { text: lines.join("\n"), recurring })
}

fn change(state: &Path, id: &str, to: State, outcome: &str) -> Result<Item> {
    let mut items = load(state);
    let item = items.iter_mut().find(|item| item.id == id).ok_or_else(|| format!("No item {id} under {SECTION}; kebbie found lists them"))?;
    if !item.open() {
        return Err(format!("{id} is settled already: {}", item.outcome.clone().unwrap_or_default()));
    }
    item.state = to;
    item.outcome = Some(outcome.to_string());
    let changed = item.clone();
    save(state, &items)?;
    Ok(changed)
}

fn users_or_leads(caller: &Caller, what: &str) -> Result<()> {
    match caller {
        Caller::User | Caller::Lead | Caller::Hub { .. } => Ok(()),
        _ => Err(format!("{what} is the user's or the lead's; {} cannot", crate::permit::describe(caller))),
    }
}

pub fn dismiss_in(state: &Path, caller: &Caller, id: &str) -> Result<String> {
    users_or_leads(caller, "dismissing what tasks found")?;
    let item = change(state, id, State::Dismissed, "dismissed")?;
    Ok(crate::notice::Notice::new(crate::notice::Tone::Done, "Dismissed").detail("item", &item.line).to_string())
}

// The running task gets the item as more work, from the user.
pub fn add_in(state: &Path, caller: &Caller, id: &str, task: &str, tell: impl FnOnce(&str, &str) -> Result<String>) -> Result<String> {
    users_or_leads(caller, "adding what a task found to another task")?;
    let item = load(state).into_iter().find(|item| item.id == id && item.open()).ok_or_else(|| format!("No open item {id} under {SECTION}; kebbie found lists them"))?;
    let told = tell(task, &format!("The user adds this to your task, found outside their own work by {}: {}. Plan it in as a part of its own, and tell the lead how it changes your plan.", item.sources.join(", "), item.line))?;
    change(state, id, State::Added, &format!("added to {task}"))?;
    Ok(crate::notice::Notice::new(crate::notice::Tone::Done, "Added to the task").next(told).detail("task", task).to_string())
}

// Keep in backlog: the item stays filed and open, and the card it came from shows the choice.
pub fn keep_in(state: &Path, caller: &Caller, id: &str) -> Result<String> {
    users_or_leads(caller, "keeping what tasks found in the backlog")?;
    let item = change(state, id, State::Kept, "kept in the backlog")?;
    Ok(crate::notice::Notice::new(crate::notice::Tone::Done, "Kept in the backlog").detail("item", &item.line).to_string())
}

pub fn started_in(state: &Path, id: &str, task: &str) -> Result<()> {
    change(state, id, State::Started, &format!("started as {task}")).map(|_| ())
}

fn tell_running(task: &str, text: &str) -> Result<String> {
    let sessions = crate::peers::sessions()?;
    let found = crate::peers::resolve(task, "you", &sessions)?;
    let _lock = crate::messages::Lock::take(true)?;
    crate::messages::send_locked(crate::messages::USER_LABEL, &found, text, &sessions, None, None).map(|said| said.join(" "))
}

// A task from the item in the repo it was found in, its line and who found it as the prompt.
pub fn start(state: &Path, caller: &Caller, id: &str, name: Option<&str>) -> Result<String> {
    users_or_leads(caller, "starting a task from what tasks found")?;
    let item = open_in(state).into_iter().find(|item| item.id == id).ok_or_else(|| format!("No open item {id} under {SECTION}; kebbie found lists them"))?;
    let name = name.map(str::trim).filter(|name| !name.is_empty()).map_or_else(|| item.name(), str::to_string);
    let repo = crate::repo::locate(&item.repo)?;
    let prompt = item.prompt();
    let started = crate::tasks::start(&repo, &name, &crate::tasks::Start { prompt: Some(&prompt), focus: matches!(caller, Caller::User), from: None, mode: None, profile: None, backlog: None, note: true, anyway: false })?;
    started_in(state, id, &name)?;
    Ok(started)
}

pub fn add(state: &Path, caller: &Caller, id: &str, task: &str) -> Result<String> {
    add_in(state, caller, id, task, tell_running)
}

// kebbie found [--json], and its start, add and dismiss.
pub fn command(words: &[String], as_json: bool) -> Result<String> {
    let state = crate::lead::state_dir()?;
    let caller = crate::permit::caller();
    let word = |at: usize| words.get(at).map(String::as_str);
    match word(0) {
        None => Ok(list(&state, as_json)),
        Some("dismiss") => dismiss_in(&state, &caller, word(1).ok_or("kebbie found dismiss needs the item's id")?),
        Some("add") => add(&state, &caller, word(1).ok_or("kebbie found add needs the item's id and the running task")?, word(2).ok_or("kebbie found add needs the running task it goes to")?),
        Some("start") => start(&state, &caller, word(1).ok_or("kebbie found start needs the item's id")?, word(2)),
        Some("keep") => keep_in(&state, &caller, word(1).ok_or("kebbie found keep needs the item's id")?),
        Some(other) => Err(format!("kebbie found knows start, add, keep and dismiss, not {other}: kebbie found [--json], kebbie found start <id> [name], kebbie found add <id> <task>, kebbie found keep <id>, kebbie found dismiss <id>")),
    }
}

pub fn list(state: &Path, as_json: bool) -> String {
    let items = open_in(state);
    if as_json {
        return serde_json::to_string_pretty(&items.iter().map(Item::to_json).collect::<Vec<_>>()).unwrap_or_default();
    }
    if items.is_empty() {
        return format!("Nothing under {SECTION}: what tasks list under Also noticed on their card is filed here.");
    }
    let mut lines = vec![format!("{SECTION}:")];
    lines.extend(items.iter().map(|item| format!("  {} {} · {}{}", item.id, item.line, item.found_by(), if item.state == State::Kept { " · kept" } else { "" })));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: &str = "**Refunds show on the invoice.**\n\nBuilt:\n- Refunds list under each invoice.\n\nAlso noticed:\n- The VAT rounding test is flaky on main: filed in the backlog (low).\n- The README's install step names an old flag.\n";

    #[test]
    fn a_cards_also_noticed_lines_are_filed_once_each_and_end_with_their_item() {
        let state = crate::sh::test_dir("found-card");
        let card = file_card(&state, "billing@app", CARD, 100).unwrap().text;
        assert!(card.contains("- The VAT rounding test is flaky on main: filed in the backlog (low). (found:f1)") && card.contains("- The README's install step names an old flag. (found:f2)"), "{card}");
        assert!(card.contains("- Refunds list under each invoice.\n"), "Built lines are not filed: {card}");
        let items = load(&state);
        assert_eq!(items.iter().map(|item| (item.id.as_str(), item.line.as_str(), item.repo.as_str(), item.count)).collect::<Vec<_>>(), [("f1", "The VAT rounding test is flaky on main", "app", 1), ("f2", "The README's install step names an old flag", "app", 1)]);
        assert_eq!(file_card(&state, "billing@app", &card, 200).unwrap().text, card, "the same card again files nothing new and counts nothing twice");
        assert_eq!(load(&state).iter().map(|item| item.count).collect::<Vec<_>>(), [1, 1]);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_repeat_from_another_task_counts_up_and_adds_its_source_instead_of_a_new_item() {
        let state = crate::sh::test_dir("found-repeat");
        file_card(&state, "billing@app", CARD, 100).unwrap();
        let other = "Built:\n- Search.\n\nAlso noticed:\n- the VAT rounding test is flaky on main.\n- Something else (found:f2)\n";
        let card = file_card(&state, "search@app", other, 300).unwrap().text;
        assert!(card.contains("- the VAT rounding test is flaky on main. (found:f1)") && card.contains("- Something else (found:f2)"), "the task's own mark names its item: {card}");
        let items = load(&state);
        assert_eq!((items.len(), items[0].count, items[0].sources.clone(), items[0].last_at), (2, 2, vec!["billing@app".to_string(), "search@app".to_string()], 300));
        assert_eq!(items[1].count, 2, "a found: mark merges whatever the words");
        assert_eq!(items[0].found_by(), "found 2× · by billing, search");
        assert!(items[0].prompt().contains("billing@app, search@app (2 times)"));
        assert!(file_card(&state, "web-billing@web", "Also noticed:\n- The VAT rounding test is flaky on main\n", 400).unwrap().text.ends_with("(found:f3)"), "the same words in another repo are another item");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_user_or_the_lead_starts_adds_or_dismisses_an_item_and_a_task_cannot() {
        let state = crate::sh::test_dir("found-act");
        file_card(&state, "billing@app", CARD, 100).unwrap();
        assert!(dismiss_in(&state, &Caller::Task("billing@app".to_string()), "f1").unwrap_err().contains("the user's or the lead's"));
        assert!(dismiss_in(&state, &Caller::User, "f1").is_ok());
        assert!(dismiss_in(&state, &Caller::User, "f1").unwrap_err().contains("settled already"));
        let mut told = Vec::new();
        add_in(&state, &Caller::Lead, "f2", "search", |task, text| { told.push((task.to_string(), text.to_string())); Ok("told".to_string()) }).unwrap();
        assert!(told[0].0 == "search" && told[0].1.contains("The user adds this to your task") && told[0].1.contains("names an old flag"), "{told:?}");
        assert!(open_in(&state).is_empty(), "dismissed and added items leave the section");
        assert_eq!(load(&state)[1].outcome.as_deref(), Some("added to search"));
        assert!(list(&state, false).starts_with("Nothing under Found by tasks"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn an_item_named_after_its_line_and_a_line_without_words_gets_its_id() {
        let item = Item { id: "f4".to_string(), line: "The README's install step names an old flag".to_string(), repo: "app".to_string(), sources: vec!["billing@app".to_string()], count: 1, first_at: 0, last_at: 0, state: State::Open, outcome: None, raised: false };
        assert_eq!(item.name(), "the-readme-install");
        assert_eq!(Item { line: "a b".to_string(), ..item }.name(), "found-f4");
    }

    #[test]
    fn an_item_a_second_task_finds_is_raised_once_and_one_kept_in_the_backlog_stays_open() {
        let state = crate::sh::test_dir("found-recurring");
        assert!(file_card(&state, "billing@app", CARD, 100).unwrap().recurring.is_empty());
        let again = file_card(&state, "search@app", "Also noticed:\n- the VAT rounding test is flaky on main.\n", 200).unwrap();
        assert_eq!(again.recurring.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), ["f1"], "found by a second task");
        assert!(load(&state)[0].raised && load(&state)[0].recurring());
        assert!(file_card(&state, "search@app", &again.text, 300).unwrap().recurring.is_empty(), "the same card again raises nothing");
        assert!(file_card(&state, "web@app", "Also noticed:\n- The VAT rounding test is flaky on main\n", 400).unwrap().recurring.is_empty(), "a third task counts up; it was raised once");
        assert_eq!(load(&state)[0].count, 3);
        assert!(keep_in(&state, &Caller::Task("billing@app".to_string()), "f2").unwrap_err().contains("the user's or the lead's"));
        keep_in(&state, &Caller::User, "f2").unwrap();
        assert!(open_in(&state).iter().any(|item| item.id == "f2" && item.state == State::Kept), "kept is still open, to start or add later");
        assert!(list(&state, false).contains("· kept"));
        started_in(&state, "f2", "readme-flag").unwrap();
        assert!(open_in(&state).iter().all(|item| item.id != "f2"));
        fs::remove_dir_all(state).unwrap();
    }
}
