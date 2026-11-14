use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::config::{self, Source};
use crate::sh::Result;

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub source: String,
    pub title: String,
    pub priority: String,
    pub area: String,
    pub reference: String,
    pub summary: String,
    pub age: String,
    pub shape: crate::backlog_file::Shape,
}

pub struct Backlog {
    pub updated: Option<u64>,
    pub vault: VaultRead,
    pub entries: Vec<Entry>,
}

// Where the column's vault rows were read: the saved list alone, the vault snapshot or local index as last synced, or a snapshot the keeper's watch keeps current.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum VaultRead {
    #[default]
    Saved,
    Synced(Option<u64>),
    Live,
}

pub const PRIORITIES: [&str; 5] = ["urgent", "high", "medium", "low", "none"];

fn file() -> Result<PathBuf> {
    Ok(crate::lead::state_dir()?.join("backlog.json"))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().trim().to_string()
}

fn entry(value: &Value) -> Option<Entry> {
    let title = text(value, "title");
    if title.is_empty() {
        return None;
    }
    let priority = text(value, "priority").to_lowercase();
    Some(Entry {
        source: text(value, "source"),
        title,
        priority: if PRIORITIES.contains(&priority.as_str()) { priority } else { "none".to_string() },
        area: text(value, "area"),
        reference: text(value, "ref"),
        summary: text(value, "summary"),
        age: text(value, "age"),
        shape: Default::default(),
    })
}

pub fn load() -> Backlog {
    let (updated, vault, items) = read_all();
    Backlog { updated, vault, entries: items.into_iter().map(|(entry, _)| entry).collect() }
}

// Each entry with its item's markdown where kebbie has it, from the same single read the Backlog column takes.
pub fn with_bodies() -> Bodied {
    read_all().2
}

type Bodied = Vec<(Entry, Option<String>)>;

fn read_all() -> (Option<u64>, VaultRead, Bodied) {
    let sources = config::sources().unwrap_or_default();
    let value: Value = file().ok().and_then(|path| fs::read_to_string(path).ok()).and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    let mut entries: Vec<Entry> = value["entries"].as_array().into_iter().flatten().filter_map(entry).filter(|entry| entry.source != "local").collect();
    let mut local = std::collections::HashMap::new();
    if sources.contains(&Source::Local) {
        crate::backlog_file::migrate_once();
        let today = today();
        for item in crate::backlog_file::items(&crate::backlog_file::dir()) {
            entries.push(item.entry(today));
            local.insert(item.reference(), item);
        }
    }
    let index = crate::vault::Index::open().ok();
    let mut vault = VaultRead::Saved;
    if let Some(index) = index.as_ref().filter(|_| sources.contains(&Source::Vault)) {
        let notes = index.backlog().unwrap_or_default();
        let watched = crate::lead::state_dir().is_ok_and(|state| crate::vault_watch::watching(&state));
        vault = if watched { VaultRead::Live } else { VaultRead::Synced(index.last_sync()) };
        entries = as_indexed(entries, &notes, today());
        entries.extend(missing_notes(&entries, &notes, today()));
    }
    let status = |key: &str| index.as_ref().map_or(Some("backlog".to_string()), |index| index.status_of(key));
    entries = still_backlog(entries, status, &crate::tasks::started_items());
    let mut items: Vec<(Entry, Option<String>)> = entries.into_iter().map(|entry| (entry, None)).collect();
    if let Ok(state) = crate::lead::state_dir() {
        if config::filing(&sources) == Some(Source::Vault) {
            let now = crate::messages::now();
            items.extend(crate::backlog_kept::kept_in(&state).iter().map(|kept| (kept.entry(now), None)));
        }
        shape_all(&mut items, &local, index.as_ref(), &state, &Templates::now());
    }
    sort(&mut items, |(entry, _)| entry, |entry| index.as_ref().filter(|_| entry.source == "vault").and_then(|index| index.changed_at(&entry.reference)));
    (value["updated"].as_u64(), vault, items)
}

pub(crate) fn bodies_file(state: &Path) -> PathBuf {
    state.join("backlog-bodies.json")
}

// A vault note's markdown: the local index holds it, else kebbie's copy read through the vault command while the note has not changed since.
pub fn vault_body(index: Option<&crate::vault::Index>, state: &Path, key: &str) -> Option<String> {
    if let Some(body) = index.and_then(|index| index.body(key)) {
        return Some(body);
    }
    let cached: Value = fs::read_to_string(bodies_file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let changed = index.and_then(|index| index.changed_at(key));
    let kept = &cached[key];
    (changed.is_none() || kept["mtime"].as_f64() == changed).then(|| kept["body"].as_str().map(str::to_string)).flatten()
}

// Fills kebbie's copy of the vault backlog notes the index has no body for; the board's refresher runs it, away from the screen, unless the keeper's watch keeps the copy.
pub fn fetch_vault_bodies() {
    let (Ok(state), Some(cli)) = (crate::lead::state_dir(), crate::vault_cli::cli()) else { return };
    fetch_vault_bodies_in(&cli, &state, crate::vault::Index::open);
}

pub fn fetch_vault_bodies_in(cli: &crate::vault_cli::Cli, state: &Path, index: impl FnOnce() -> Result<crate::vault::Index>) {
    if crate::vault_watch::watching(state) {
        return;
    }
    if let Ok(index) = index() {
        fill_vault_bodies(cli, state, &index);
    }
}

pub fn fill_vault_bodies(cli: &crate::vault_cli::Cli, state: &Path, index: &crate::vault::Index) {
    static REFUSED: std::sync::Mutex<Vec<(String, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());
    let mut cached: Value = fs::read_to_string(bodies_file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_else(|| json!({}));
    let mut refused = REFUSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    refused.retain(|(_, at)| at.elapsed() < std::time::Duration::from_secs(300));
    let mut changed = false;
    for [key, ..] in index.backlog().unwrap_or_default() {
        if index.body(&key).is_some() || refused.iter().any(|(held, _)| *held == key) {
            continue;
        }
        let mtime = index.changed_at(&key);
        if mtime.is_some() && cached[&key]["mtime"].as_f64() == mtime {
            continue;
        }
        match cli.read_body(&key) {
            Ok(body) => {
                cached[&key] = json!({ "mtime": mtime, "body": body });
                changed = true;
            }
            Err(_) => refused.push((key, std::time::Instant::now())),
        }
    }
    if changed {
        let _ = crate::state::write(&bodies_file(state), cached.to_string());
    }
}

// The templates a pass reads its items by, read once for the pass.
pub struct Templates {
    pub local: crate::item_template::Template,
    pub vault: crate::item_template::Template,
}

impl Templates {
    pub fn now() -> Templates {
        Templates { local: crate::item_template::local(), vault: crate::item_template::for_vault() }
    }

    pub fn of(&self, source: &str) -> &crate::item_template::Template {
        if source == "vault" { &self.vault } else { &self.local }
    }
}

struct ItemRead {
    state: crate::backlog_file::State,
    body: String,
    filed: String,
}

// An item's state and markdown: a local one from the file already read, a vault one from the index or kebbie's copy.
fn read_item(entry: &Entry, local: &std::collections::HashMap<String, crate::backlog_file::Item>, index: Option<&crate::vault::Index>, state: &Path, template: &crate::item_template::Template) -> Option<ItemRead> {
    let (state_now, body, filed) = match entry.source.as_str() {
        "local" => {
            let item = local.get(&entry.reference).cloned().or_else(|| crate::backlog_file::find(&entry.reference).map(|(_, item)| item))?;
            (item.state(), item.body, item.filed)
        }
        "vault" => {
            let drafting = index.and_then(|index| index.status_of(&entry.reference)).as_deref() == Some("draft");
            match vault_body(index, state, &entry.reference) {
                Some(body) => (crate::backlog_file::state_of(drafting, false, &body, template), body, String::new()),
                None if drafting => (crate::backlog_file::State::Drafting, String::new(), String::new()),
                None => return None,
            }
        }
        _ => return None,
    };
    Some(ItemRead { state: state_now, body, filed })
}

// Each item's state, its questions waiting on the user and the answers of theirs still with the lead, settling the answers once for the whole backlog.
fn shape_all(items: &mut [(Entry, Option<String>)], local: &std::collections::HashMap<String, crate::backlog_file::Item>, index: Option<&crate::vault::Index>, state: &Path, templates: &Templates) {
    let read: Vec<Option<ItemRead>> = items.iter().map(|(entry, _)| read_item(entry, local, index, state, templates.of(&entry.source))).collect();
    let bodies: Vec<(&str, &str, &crate::item_template::Template)> = items.iter().zip(&read).filter_map(|((entry, _), read)| Some((entry.reference.as_str(), read.as_ref()?.body.as_str(), templates.of(&entry.source)))).collect();
    let kept = crate::backlog_kept::settle_all(state, &bodies);
    for ((entry, body), read) in items.iter_mut().zip(read) {
        let Some(read) = read else { continue };
        let template = templates.of(&entry.source);
        let answered: Vec<String> = kept.iter().filter(|answer| answer.item == entry.reference).map(|answer| answer.question.clone()).collect();
        entry.shape = crate::backlog_file::shape_of(read.state, &read.body, &answered, &read.filed, template);
        if entry.summary.is_empty() && read.state == crate::backlog_file::State::Ready {
            entry.summary = template.first_line(&read.body);
        }
        *body = Some(read.body);
    }
}

// Backlog notes the lead's last snapshot does not have yet: filed since, or put back by a reset.
fn indexed([key, title, summary, priority, area, date]: &[String; 6], today: i64) -> Entry {
    Entry {
        source: "vault".to_string(),
        title: title.clone(),
        priority: if PRIORITIES.contains(&priority.as_str()) { priority.clone() } else { "none".to_string() },
        area: crate::vault::area_name(area),
        reference: key.clone(),
        summary: summary.clone(),
        age: days_since(date, today).map_or(String::new(), |days| format!("{days} d")),
        shape: Default::default(),
    }
}

// A saved vault entry the index also holds is drawn as the index has it now, its age counted to today rather than to the save.
fn as_indexed(entries: Vec<Entry>, notes: &[[String; 6]], today: i64) -> Vec<Entry> {
    entries.into_iter().map(|entry| match notes.iter().find(|[key, ..]| entry.source == "vault" && *key == entry.reference) {
        Some(note) => indexed(note, today),
        None => entry,
    }).collect()
}

fn missing_notes(entries: &[Entry], notes: &[[String; 6]], today: i64) -> Vec<Entry> {
    notes.iter().filter(|[key, ..]| !entries.iter().any(|entry| entry.source == "vault" && &entry.reference == key)).map(|note| indexed(note, today)).collect()
}

pub(crate) fn today() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs() as i64 / 86_400)
}

fn days_since(date: &str, today: i64) -> Option<i64> {
    epoch_day(date).map(|day| (today - day).max(0))
}

// Days from 1970-01-01 to a YYYY-MM-DD date, by the civil-calendar formula.
pub(crate) fn epoch_day(date: &str) -> Option<i64> {
    let mut parts = date.get(..10)?.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

// By priority, then newest first: by age in days, and within a day by when the note last changed.
fn sort<T: Clone>(entries: &mut [T], entry_of: impl Fn(&T) -> &Entry, changed_at: impl Fn(&Entry) -> Option<f64>) {
    let days = |entry: &Entry| entry.age.split_whitespace().next().and_then(|days| days.parse::<u64>().ok()).unwrap_or(u64::MAX);
    let mut keyed: Vec<(usize, u64, f64, T)> = entries.iter().map(|item| {
        let entry = entry_of(item);
        (PRIORITIES.iter().position(|priority| *priority == entry.priority).unwrap_or(PRIORITIES.len()), days(entry), -changed_at(entry).unwrap_or(0.0), item.clone())
    }).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)));
    for (slot, (_, _, _, entry)) in entries.iter_mut().zip(keyed) {
        *slot = entry;
    }
}

// An item a running task came from, or a vault note closed since the snapshot, leaves the Backlog column without waiting for a refresh; a reset task's note shows again even while it still reads active. A task started before local items were files names its item local:<Title>.
fn still_backlog(entries: Vec<Entry>, status: impl Fn(&str) -> Option<String>, running: &HashSet<String>) -> Vec<Entry> {
    entries.into_iter()
        .filter(|entry| !running.contains(&entry.reference) && !(entry.source == "local" && running.contains(&crate::backlog_file::reference(&entry.title))))
        .filter(|entry| entry.source != "vault" || matches!(status(&entry.reference).as_deref(), Some("backlog" | "active" | "draft")))
        .collect()
}

pub fn age(updated: Option<u64>) -> String {
    let Some(updated) = updated else { return "never".to_string() };
    let minutes = now().saturating_sub(updated) / 60;
    match minutes {
        0 => "just now".to_string(),
        1..=59 => format!("{minutes} min ago"),
        _ => format!("{} h ago", minutes / 60),
    }
}

// Reads a JSON array of {source, title, priority, area, ref, summary, age} on stdin; the lead keeps it fresh.
pub fn save() -> Result<String> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&input).map_err(|error| format!("Not JSON: {error}"))?;
    let entries: Vec<Value> = value.as_array().ok_or("Expected a JSON array of entries")?.iter()
        .filter(|item| entry(item).is_some()).cloned().collect();
    let count = entries.len();
    let path = file()?;
    crate::state::write(&path, json!({ "updated": now(), "entries": entries }).to_string())?;
    Ok(format!("Saved {count} backlog entries to {}.", path.display()))
}

fn joined(sources: &[Source], part: fn(Source) -> &'static str, word: &str) -> String {
    sources.iter().map(|source| part(*source)).collect::<Vec<_>>().join(word)
}

// The sources the lead fetches and saves with `kebbie backlog save`; the local file is read as it is.
pub fn fetched(sources: &[Source]) -> Vec<Source> {
    sources.iter().copied().filter(|source| *source != Source::Local).collect()
}

fn local_dir() -> String {
    crate::backlog_file::dir().display().to_string()
}

fn wanted(source: Source) -> &'static str {
    match source {
        Source::Local => "the local backlog folder",
        Source::Vault => "the vault backlog",
        Source::Linear => "my open Linear issues",
    }
}

fn reference(source: Source) -> &'static str {
    match source {
        Source::Local => "`local:` and the item's file name",
        Source::Vault => "the vault path",
        Source::Linear => "the Linear id",
    }
}

pub fn from(sources: &[Source]) -> String {
    joined(&fetched(sources), |source| if source == Source::Vault { "the vault" } else { "Linear" }, " and ")
}

// Each source with how current its rows are: the local folder and a watched vault live, a synced vault by its last sync, the rest by the lead's last save.
pub fn freshness(sources: &[Source], backlog: &Backlog) -> String {
    if sources.is_empty() {
        return "no source".to_string();
    }
    sources.iter().map(|source| {
        let read = match (source, backlog.vault) {
            (Source::Local, _) | (Source::Vault, VaultRead::Live) => "live".to_string(),
            (Source::Vault, VaultRead::Synced(at)) => age(at),
            _ => age(backlog.updated),
        };
        format!("{} {read}", source.title())
    }).collect::<Vec<_>>().join(" + ")
}

pub fn not_configured() -> String {
    format!("No backlog source is configured; set [board.backlog] sources = [\"local\"] (or [\"vault\"], with \"linear\" beside either) in {}.", config::path().display())
}

fn nothing_to_refresh(sources: &[Source]) -> String {
    if sources.contains(&Source::Local) { format!("The Backlog column reads {} as it changes; there is nothing for the lead to refresh.", local_dir()) } else { not_configured() }
}

pub fn refresh_label(sources: &[Source]) -> String {
    if fetched(sources).is_empty() { nothing_to_refresh(sources) } else { format!("the lead fetches {} again", from(sources)) }
}

pub fn empty_text(sources: &[Source]) -> String {
    let adds = config::filing(sources).is_some();
    match (sources.contains(&Source::Local), fetched(sources).is_empty()) {
        (false, true) => not_configured(),
        (true, true) => format!("Empty. n adds an item, which the lead writes out, or put a file of your own in {}.", local_dir()),
        (_, false) => format!("Empty. R asks the lead to fill it from {}{}.", from(sources), if adds { "; n adds an item" } else { "" }),
    }
}

pub fn refresh_prompt(configured: &[Source]) -> Result<String> {
    let sources = fetched(configured);
    if sources.is_empty() {
        return Err(nothing_to_refresh(configured));
    }
    Ok(format!(
        "Refresh the kebbie backlog: take {} and pipe them as one JSON array to `$HOME/.local/share/kebbie/current/target/release/kebbie backlog save`. \
         Fields per entry: source ({}), title, priority (urgent, high, medium, low or none), area (the folder name of the repo the work happens in), \
         ref ({}), summary (one line), age (e.g. 3 d). Reply in one line when saved.",
        joined(&sources, wanted, " and "), joined(&sources, Source::key, " or "), joined(&sources, reference, " or "),
    ))
}

pub fn task_prompt(entry: &Entry, sources: &[Source], vault: bool) -> String {
    match entry.source.as_str() {
        "vault" if vault => format!("Work on the vault note {reference} (\"{}\"); kebbie has made it this task's note. Read it with the vault tools first, then ask me what you need decided with kebbie ask, one question at a time, before you plan; then write the plan into that note and put it to me with kebbie ask --plan vault:{reference}, and write no code until I approve it.", entry.title, reference = entry.reference),
        "linear" if sources.contains(&Source::Linear) => format!("Work on Linear issue {}: {}. Read the issue first, then ask me what you need decided with kebbie ask, one question at a time, before you plan; then put the plan to me with kebbie ask --plan, written where your role says, and write no code until I approve it.", entry.reference, entry.title),
        "vault" if sources.contains(&Source::Vault) => format!("Work on the vault note {reference} (\"{}\"). Read it with the vault tools first and set its status to active, then ask me what you need decided with kebbie ask, one question at a time, before you plan; then write the plan into that note and put it to me with kebbie ask --plan vault:{reference}, and write no code until I approve it.", entry.title, reference = entry.reference),
        "local" => match crate::backlog_file::find(&entry.reference) {
            Some((path, item)) => format!("Work on the backlog item \"{}\" ({}). Here it is in full:\n\n{}\n\nAsk me what you need decided with kebbie ask, one question at a time, before you plan, starting with its open questions; then put the plan to me with kebbie ask --plan, written where your role says, and write no code until I approve it.", item.title, path.display(), item.body),
            None => format!("Work on: {}. {} Ask me what you need decided with kebbie ask, one question at a time, before you plan; then put the plan to me with kebbie ask --plan, written where your role says, and write no code until I approve it.", entry.title, entry.summary),
        },
        _ => format!("Work on: {}. {} Ask me what you need decided with kebbie ask, one question at a time, before you plan; then put the plan to me with kebbie ask --plan, written where your role says, and write no code until I approve it.", entry.title, entry.summary),
    }
}

// The title of a backlog item by its reference, from the lead's snapshot or the vault index, whatever its status now.
pub fn title_of(item: &str) -> Option<String> {
    if let Some(title) = local_title(item) {
        return Some(title);
    }
    let value: Value = file().ok().and_then(|path| fs::read_to_string(path).ok()).and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    let cached = value["entries"].as_array().into_iter().flatten().filter_map(entry).find(|entry| entry.reference == item).map(|entry| entry.title);
    cached.or_else(|| crate::vault::Index::open().ok()?.title_of(item))
}

// A local item by its reference: its title from its file, or the reference's own words once the file is gone.
fn local_title(item: &str) -> Option<String> {
    let rest = item.strip_prefix(crate::backlog_file::PREFIX)?;
    Some(crate::backlog_file::find(item).map_or_else(|| rest.to_string(), |(_, found)| found.title))
}

fn local_file_of(item: &str) -> String {
    crate::backlog_file::find(item).map_or_else(local_dir, |(path, _)| path.display().to_string())
}

pub fn reset_notice(item: &str) -> String {
    match local_title(item) {
        Some(title) => format!(" Nobody works on the backlog item \"{title}\" now; its file {} shows in the Backlog column again.", local_file_of(item)),
        None => format!(" Nobody works on {item} now: set it back to backlog."),
    }
}

pub fn done_notice(task: &str, item: &str) -> String {
    match local_title(item) {
        Some(title) => format!("{task} is done and its commits are on the default branch. It came from the backlog item \"{title}\": log it, then delete its file {}, or rewrite the item to what is left when work is left.", local_file_of(item)),
        None => format!("{task} is done and its commits are on the default branch. It came from {item}: log it, then close {item}, or update it when work is left."),
    }
}

fn role_wanted(source: Source) -> &'static str {
    match source {
        Source::Local => "the local backlog folder",
        Source::Vault => "the vault backlog (the vault's backlog tool)",
        Source::Linear => "the user's open Linear issues (assigned to them, not done; Linear's MCP)",
    }
}

fn role_finding(source: Source) -> &'static str {
    match source {
        Source::Local => "the local backlog (the board's Backlog column, or `kebbie backlog`) by priority",
        Source::Vault => "the vault backlog by priority",
        Source::Linear => "Linear (assigned to the user, current cycle)",
    }
}

pub struct Role {
    pub finding: String,
    pub backlog: String,
    pub intake: String,
    pub vault_work: String,
    pub after_push: String,
}

const VAULT_WORK: &str = "- **The vault**: keep it current: started tasks and what each push landed in the repo log, the user's decisions in the task's note. New work goes in the backlog as under Taking in ideas.";
const AFTER_PUSH: &str = "- `<who> pushed <task>: <n> commits to <branch> (<sha> <subject>; …)` comes among kebbie's notices after every push, the user's or yours. Write the repo's log line in the vault with `log_append`: what landed, with those shas. When the task is done, close its note (`close`, with `merged_into` when the area note took it in) or append to it what landed.";
const KEPT_VAULT_WORK: &str = "- **The vault**: kebbie keeps each task's note: it opens one when the task starts (or claims the backlog note named with `--backlog`), the task writes what its edits wait for before its first change (its Problem, or with the vault's note templates the task template's required sections) and the rest as it goes, kebbie writes the repo's log line after each push and closes the note at done. Your part is the area notes and the user's decisions that reach beyond one task. New work goes in the backlog as under Taking in ideas.";
const KEPT_AFTER_PUSH: &str = "- `<who> pushed <task>: <n> commits to <branch> (<sha> <subject>; …)` comes among kebbie's notices after every push, the user's or yours; kebbie has written the repo's log line from those commits.";
const OLD_NOTES: &str = "A vault note written before the vault's task template, without its sections, also shows as `to write`. Leave those as they stand: one is written out when it is picked up (`g` on the board, `kebbie start --backlog`), where its task fills the template's sections before it changes code, or when the user opens it and asks you to, by the skill in its own note.";
const KEPT_TASKS: &str = "Every task kebbie starts has its own note in the vault: kebbie opens it at the start, or claims the backlog note named with `--backlog`, and the task writes it from your prompt. Done waits until the note has its solution and decisions (with note templates, the template's required sections), kebbie then closes it, and a reset puts it back in the backlog.";

fn local_role() -> String {
    format!(
        "The Backlog column reads `{dir}` as it changes: one markdown file per item, its header naming the title, priority, repo and status. \
         You keep this folder: file new work as under Taking in ideas, change a priority or rewrite an item in its file, and delete an item's file once its task is pushed or the user drops it. \
         An item's reference is `local:` and its file name without `.md` (to `kebbie start --backlog`); `kebbie backlog add` prints it, and the file name stays when the title changes.\n\n\
         `kebbie backlog` marks each item that is not ready: `to write`, a short item the user added (with `n` on the board, `kebbie backlog add`, or a file of their own) or one moved from the old one-file backlog, and `drafting`, one you are writing out or whose questions wait for the user. \
         When you read the backlog (at your start, at a briefing, when finding work) and it lists an item to write, write it out by the skill `{skill}` in its own file and tell the user you did.",
        dir = local_dir(), skill = crate::backlog_file::SKILL,
    )
}

fn fetched_role(sources: &[Source], also: bool) -> String {
    format!(
        "The board's Backlog tab {}shows what you save with `kebbie backlog save`. When the user asks you to refresh it, or when it is more than a day old at a briefing: \
         take {} and pipe them as one JSON array to `kebbie backlog save`. Each entry: `source` ({}), `title`, `priority` (`urgent`, `high`, `medium`, `low` or `none`), \
         `area` (the folder name of the repo the work happens in; not the vault area when the two differ, since starting the task looks the repo up by this name), \
         `ref` ({}), `summary` (one line) and `age` (e.g. `3 d`, `1 mo`).",
        if also { "also " } else { "" },
        joined(sources, role_wanted, " and "), joined(sources, |source| if source == Source::Vault { "`vault`" } else { "`linear`" }, " or "), joined(sources, reference, " or "),
    )
}

struct Filed {
    save: &'static str,
    grown: &'static str,
    reference: &'static str,
}

fn filed_in(source: Source) -> Filed {
    match source {
        Source::Vault => Filed {
            save: "write it as a task note in the vault's Projects/ with the vault tools",
            grown: "write a task note for it in the vault's Projects/ by the skill, status `active`",
            reference: "<the note's path>",
        },
        _ => Filed {
            save: "save it with `kebbie backlog add --priority <priority> --repo <repo> \"<Title>\" -`, the item's sections on stdin",
            grown: "write an item for it by the skill and save it with `kebbie backlog add`",
            reference: "<the ref kebbie backlog add printed>",
        },
    }
}

fn intake_role(sources: &[Source], vault: bool) -> String {
    let skill = crate::backlog_file::SKILL;
    let Some(source) = config::filing(sources) else {
        return format!(
            "When the user brings an idea rather than a task to start, draft a task prompt for it, show it, and start it with `kebbie start` on yes. \
             No backlog takes new items, so there is nowhere to file it: say so, and that `\"local\"` or `\"vault\"` under `[board.backlog] sources` in {} turns one on.",
            config::path().display(),
        );
    };
    let Filed { save, grown, reference } = filed_in(source);
    let start = if vault && source == Source::Vault {
        "start it with the written item as its prompt, as under Starting a task: `kebbie start <name> \"<prompt>\" --repo <repo>`; kebbie makes the task's note from it".to_string()
    } else {
        format!("file it the same way, then start it with a prompt written as under Starting a task: `kebbie start <name> \"<prompt>\" --repo <repo> --backlog {reference}`")
    };
    let steps = format!(
        "Every item in the backlog is written out by the skill `{skill}`, whoever files it, at the moment it is filed: it reads what bears on the work first, keeps the item the user's intent, and puts what you could not find out under Open questions.\n\n\
         When the user brings an idea rather than a task to start, take it in:\n\n\
         1. Write it out with the skill `{skill}`.\n\
         2. Show the user the item, and ask with the AskUserQuestion tool: start it now as a task, file it in the backlog, or change it first.\n\
         3. On file: {save}. On start: {start}. On change: rewrite it and ask again.\n\n\
         The idea is taken in when the user has picked start or file and the item exists.\n\n\
         A short item the user adds outside this chat (`n` on the board, `kebbie backlog add`, a file of their own) reaches you as a message from kebbie, or as `to write` in `kebbie backlog`: write it out by the skill at once and save it where it is, then show the user the written item. \
         An item's questions belong to the item: they stay under its Open questions, never in `kebbie ask` and never under Needs you. The user answers them on the item's card on the board, and kebbie brings you each answer with its question: fold the answer into the item and take the question out; the item is ready once no question blocks it.",
    );
    let steps = if source == Source::Vault { format!("{steps}\n\n{OLD_NOTES}") } else { steps };
    if vault {
        return format!("{steps}\n\n{KEPT_TASKS}");
    }
    format!(
        "{steps}\n\n\
         A task started without a backlog item stays a plain task. When it grows into a project (it spans sessions or carries decisions), {grown}, and link it: \
         `kebbie -C <repo> link <task> {reference}`. A linked task counts as started from that item: kebbie tells you when it is done, so you log it and close or update the item, \
         and a reset puts the item back in the backlog.",
    )
}

// Filled into roles/lead.md: {finding}, {backlog}, {intake}, {vault_work} and {after_push}; vault is whether kebbie keeps the tasks' notes itself.
pub fn role_texts(sources: &[Source], vault: bool) -> Role {
    let intake = intake_role(sources, vault);
    let (vault_work, after_push) = if vault { (KEPT_VAULT_WORK, KEPT_AFTER_PUSH) } else { (VAULT_WORK, AFTER_PUSH) };
    let (vault_work, after_push) = (vault_work.to_string(), after_push.to_string());
    if sources.is_empty() {
        let backlog = format!("No backlog source is configured (`[board.backlog] sources` in {}), so the Backlog tab stays empty and there is nothing to refresh.", config::path().display());
        return Role { finding: "ask the user what to work on".to_string(), backlog, intake, vault_work, after_push };
    }
    let local = sources.contains(&Source::Local);
    let fetched = fetched(sources);
    let mut parts = Vec::new();
    if local {
        parts.push(local_role());
    }
    if !fetched.is_empty() {
        parts.push(fetched_role(&fetched, local));
    }
    Role { finding: joined(sources, role_finding, " and "), backlog: parts.join("\n\n"), intake, vault_work, after_push }
}

pub fn show() -> Result<String> {
    let sources = config::sources()?;
    let backlog = load();
    if backlog.entries.is_empty() {
        return Ok(empty_text(&sources));
    }
    let mut lines = vec![if fetched(&sources).is_empty() { format!("from {}", local_dir()) } else { format!("updated {}", age(backlog.updated)) }];
    for entry in &backlog.entries {
        let item = (entry.source == "local").then(|| crate::backlog_file::find(&entry.reference)).flatten();
        let state = item.as_ref().map(|(_, item)| item.state()).filter(|state| *state != crate::backlog_file::State::Ready).map_or(String::new(), |state| format!("  {}", state.word()));
        let file = item.as_ref().map_or(String::new(), |(path, _)| format!("  {}", path.display()));
        lines.push(format!("  [{}] {}  ({} · {}){state}{file}", entry.priority, entry.title, entry.reference, entry.area));
    }
    Ok(lines.join("\n"))
}

fn matches_all(text: &str, words: &[String]) -> bool {
    let text = text.to_lowercase();
    words.iter().all(|word| text.contains(word.as_str()))
}

// kebbie backlog --search: the items whose words hold every word asked, from the backlog as kebbie last read it and from Found by tasks; it reads, and fetches or writes nothing.
pub fn search(query: Option<&str>) -> Result<String> {
    let query = query.map(str::trim).filter(|query| !query.is_empty()).ok_or("kebbie backlog --search needs the words to look for: kebbie backlog --search \"<words>\"")?;
    let found = crate::lead::state_dir().map(|state| crate::found::open_in(&state)).unwrap_or_default();
    Ok(search_in(&load(), &found, query))
}

pub fn search_in(backlog: &Backlog, found: &[crate::found::Item], query: &str) -> String {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut lines: Vec<String> = backlog.entries.iter().filter(|entry| matches_all(&format!("{} {} {} {}", entry.title, entry.summary, entry.reference, entry.area), &words))
        .map(|entry| format!("  [{}] {}  ({} · {})", entry.priority, entry.title, entry.reference, entry.area)).collect();
    lines.extend(found.iter().filter(|item| matches_all(&format!("{} {}", item.line, item.repo), &words)).map(|item| format!("  [found] {}  (found:{} · {})", item.line, item.id, item.found_by())));
    if lines.is_empty() {
        return format!("Nothing in the backlog or under {} holds \"{query}\". Try other words before you file it as new.", crate::found::SECTION);
    }
    let head = format!("{} for \"{query}\", from the backlog as kebbie last read it and from {}:", if lines.len() == 1 { "1 item".to_string() } else { format!("{} items", lines.len()) }, crate::found::SECTION);
    std::iter::once(head).chain(lines).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_lists_the_items_holding_every_word_from_the_backlog_and_found_by_tasks() {
        let backlog = Backlog { updated: None, vault: VaultRead::Saved, entries: vec![Entry { title: "The suite is red inside a task".to_string(), summary: "cargo test fails in a worktree".to_string(), reference: "local:suite-red".to_string(), ..entry("local") }, entry("vault")] };
        let found = [crate::found::Item { id: "f3".to_string(), line: "The suite is red in a task's sandbox".to_string(), repo: "kebbie".to_string(), sources: vec!["a@kebbie".to_string(), "b@kebbie".to_string()], count: 2, first_at: 0, last_at: 0, state: crate::found::State::Open, outcome: None, raised: true }];
        let said = search_in(&backlog, &found, "Suite RED");
        assert_eq!(said, "2 items for \"Suite RED\", from the backlog as kebbie last read it and from Found by tasks:\n  [high] The suite is red inside a task  (local:suite-red · app)\n  [found] The suite is red in a task's sandbox  (found:f3 · found 2× · by a, b)");
        assert!(search_in(&backlog, &found, "worktree").contains("suite-red"), "the summary counts");
        assert!(search_in(&backlog, &found, "vault rounding").starts_with("Nothing in the backlog"));
    }

    fn entry(source: &str) -> Entry {
        Entry { source: source.to_string(), title: "Fix it".to_string(), priority: "high".to_string(), area: "app".to_string(), reference: "ABC-1".to_string(), summary: "It broke.".to_string(), age: "3 d".to_string(), shape: Default::default() }
    }

    #[test]
    fn the_backlog_is_shaped_from_the_files_read_once_and_its_answers_settled_in_one_pass() {
        let state = crate::sh::test_dir("backlog-one-pass");
        let item = |stem: &str, questions: &str| crate::backlog_file::parse(stem, &format!("+++\ntitle = \"{stem}\"\npriority = \"high\"\nrepo = \"app\"\nstatus = \"backlog\"\nfiled = \"2026-11-01\"\n+++\n\n## Background\n\nB.\n\n## Goal\n\nG.\n\n## What we know\n\nK.\n\n## Open questions\n\n{questions}\n\n## Out of scope\n\nO.\n\n## Source\n\nS.\n")).unwrap();
        let (one, two) = (item("not-on-disk-one", "- [ ] Which job?"), item("not-on-disk-two", "- [ ] Which repo?"));
        let given = |item: &str, question: &str| crate::backlog_kept::Answer { item: item.to_string(), question: question.to_string(), answer: "e2e".to_string(), at: 1 };
        for answer in [given(&one.reference(), "Which job?"), given(&two.reference(), "Folded already?"), given("local:elsewhere", "Kept?")] {
            crate::backlog_kept::answer(&state, answer).unwrap();
        }
        let local: std::collections::HashMap<String, crate::backlog_file::Item> = [one.clone(), two.clone()].into_iter().map(|item| (item.reference(), item)).collect();
        let mut items = vec![(one.entry(today()), None), (two.entry(today()), None)];
        shape_all(&mut items, &local, None, &state, &Templates { local: crate::item_template::Template::builtin(), vault: crate::item_template::Template::builtin() });
        assert_eq!(items[0].1.as_deref(), Some(one.body.as_str()), "the body comes from the file the listing read, not a second read: these files are not on disk");
        assert_eq!((items[0].0.shape.waiting, items[0].0.shape.with_lead), (0, 1), "{:?}", items[0].0.shape);
        assert_eq!((items[1].0.shape.waiting, items[1].0.shape.with_lead), (1, 0), "{:?}", items[1].0.shape);
        let left: Vec<String> = crate::backlog_kept::answers_in(&state).into_iter().map(|answer| answer.question).collect();
        assert_eq!(left, ["Which job?", "Kept?"], "the folded answer leaves; one not in the backlog stays");
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn the_refresh_prompt_names_only_the_configured_sources() {
        let both = refresh_prompt(&[Source::Vault, Source::Linear]).unwrap();
        assert!(both.starts_with("Refresh the kebbie backlog: take the vault backlog and my open Linear issues and pipe"));
        assert!(both.contains("source (vault or linear)") && both.contains("ref (the vault path or the Linear id)"));
        let vault = refresh_prompt(&[Source::Vault]).unwrap();
        assert!(vault.contains("take the vault backlog and pipe") && vault.contains("source (vault)") && !vault.contains("Linear"));
        let linear = refresh_prompt(&[Source::Linear]).unwrap();
        assert!(linear.contains("take my open Linear issues and pipe") && !linear.contains("vault"));
        let none = refresh_prompt(&[]).unwrap_err();
        assert!(none.starts_with("No backlog source is configured") && none.contains("[board.backlog] sources"));
        let local = refresh_prompt(&[Source::Local]).unwrap_err();
        assert!(local.contains("reads") && local.contains("backlog") && local.contains("nothing for the lead to refresh"), "{local}");
        assert_eq!(refresh_prompt(&[Source::Local, Source::Vault]).unwrap(), vault, "the lead never saves the local file's items");
    }

    #[test]
    fn the_header_dates_each_source_by_what_its_rows_were_read_from() {
        let hours_ago = |hours: u64| Some(now() - hours * 3600);
        let backlog = |vault: VaultRead| Backlog { updated: hours_ago(562), vault, entries: Vec::new() };
        assert_eq!(freshness(&[Source::Vault], &backlog(VaultRead::Synced(Some(now() - 120)))), "vault 2 min ago", "vault rows come from the snapshot, not the lead's old save");
        assert_eq!(freshness(&[Source::Vault], &backlog(VaultRead::Live)), "vault live");
        assert_eq!(freshness(&[Source::Vault], &backlog(VaultRead::Saved)), "vault 562 h ago", "without an index the saved list is all there is");
        assert_eq!(freshness(&[Source::Vault, Source::Linear], &backlog(VaultRead::Synced(hours_ago(1)))), "vault 1 h ago + Linear 562 h ago", "Linear's rows are only the lead's save");
        assert_eq!(freshness(&[Source::Local], &backlog(VaultRead::Saved)), "local live");
        assert_eq!(freshness(&[], &backlog(VaultRead::Saved)), "no source");
    }

    #[test]
    fn a_saved_vault_entry_the_index_holds_is_drawn_as_the_index_has_it_today() {
        let saved = vec![Entry { reference: "Projects/known.md".to_string(), title: "Old title".to_string(), age: "27 d".to_string(), ..entry("vault") }, Entry { reference: "Projects/gone.md".to_string(), age: "3 d".to_string(), ..entry("vault") }, entry("linear")];
        let row = [ "Projects/known.md", "New title", "Sum", "high", "[[kebbie]]", "2026-09-01"].map(str::to_string);
        let shown = as_indexed(saved, &[row], epoch_day("2026-11-14").unwrap());
        assert_eq!(shown.iter().map(|entry| (entry.title.as_str(), entry.age.as_str())).collect::<Vec<_>>(), [("New title", "74 d"), ("Fix it", "3 d"), ("Fix it", "3 d")]);
    }

    #[test]
    fn the_board_texts_tell_a_local_backlog_from_a_fetched_one() {
        assert_eq!(from(&[Source::Local, Source::Linear]), "Linear");
        assert!(refresh_label(&[Source::Local]).contains("nothing for the lead to refresh"));
        assert_eq!(refresh_label(&[Source::Local, Source::Vault]), "the lead fetches the vault again");
        assert!(empty_text(&[Source::Local]).starts_with("Empty. n adds an item, which the lead writes out, or put a file of your own in "), "{}", empty_text(&[Source::Local]));
        assert_eq!(empty_text(&[Source::Vault]), "Empty. R asks the lead to fill it from the vault; n adds an item.");
        assert_eq!(empty_text(&[Source::Linear]), "Empty. R asks the lead to fill it from Linear.", "Linear is only read, so nothing is added to it");
        assert!(empty_text(&[]).starts_with("No backlog source is configured"));
    }

    #[test]
    fn a_reset_or_finished_local_item_is_told_by_its_title_and_file() {
        let reset = reset_notice("local:Fix login");
        assert!(reset.contains("\"Fix login\"") && reset.contains("its file") && reset.contains("backlog") && !reset.contains("set it back"), "{reset}");
        assert_eq!(reset_notice("Projects/x.md"), " Nobody works on Projects/x.md now: set it back to backlog.");
        let done = done_notice("login", "local:Fix login");
        assert!(done.starts_with("login is done") && done.contains("delete its file") && done.contains("rewrite the item to what is left") && done.contains("\"Fix login\""), "{done}");
        let note = done_notice("login", "Projects/x.md");
        assert!(note.contains("It came from Projects/x.md: log it, then close Projects/x.md, or update it when work is left."), "{note}");
    }

    #[test]
    fn a_local_item_leaves_the_backlog_while_its_task_runs() {
        let item = |title: &str| Entry { source: "local".to_string(), title: title.to_string(), reference: crate::backlog_file::reference(&crate::backlog_file::stem_for(title, |_| false)), ..entry("local") };
        let running = HashSet::from([crate::backlog_file::reference("fix-login"), crate::backlog_file::reference("Older one")]);
        let kept: Vec<String> = still_backlog(vec![item("Fix login"), item("Other"), item("Older one")], |_| None, &running).into_iter().map(|entry| entry.title).collect();
        assert_eq!(kept, ["Other"], "a local item needs no vault status to stay, and a task started from its title before the move still holds it");
    }

    #[test]
    fn labels_and_the_role_follow_the_sources() {
        assert_eq!(from(&[Source::Vault]), "the vault");
        let Role { finding, backlog, intake, .. } = role_texts(&[Source::Vault], false);
        assert_eq!(finding, "the vault backlog by priority");
        assert!(!backlog.contains("Linear") && backlog.contains("`source` (`vault`)") && !backlog.contains("backlog.md"));
        assert!(intake.contains("task note in the vault's Projects/") && intake.contains("--backlog <the note's path>") && !intake.contains("Linear") && !intake.contains("local:"), "{intake}");
        let Role { finding, backlog, intake, .. } = role_texts(&[], false);
        assert!(!finding.contains("Linear") && !backlog.contains("Linear") && backlog.contains("No backlog source") && intake.contains("nowhere to file it"));
        assert!(role_texts(&[Source::Linear, Source::Vault], false).backlog.contains("Linear issues (assigned to them, not done; Linear's MCP) and the vault backlog"));
        let linear_first = role_texts(&[Source::Linear, Source::Vault], false).intake;
        assert!(linear_first.contains("task note in the vault's Projects/") && !linear_first.contains("Linear issue"), "Linear is only read; new items go to the vault: {linear_first}");
        assert!(role_texts(&[Source::Linear], false).intake.contains("nowhere to file it"));
    }

    #[test]
    fn with_the_vault_on_kebbie_keeps_the_notes_and_the_log_so_the_lead_does_not() {
        let on = role_texts(&[Source::Vault], true);
        assert!(!on.after_push.contains("log_append") && on.after_push.contains("kebbie has written the repo's log line") && !on.after_push.contains("area note"), "kebbie logs each push; no area-note chore rides on it: {}", on.after_push);
        assert!(on.vault_work.contains("kebbie keeps each task's note") && !on.vault_work.contains("repo log, the user's"), "{}", on.vault_work);
        assert!(!on.intake.contains("plain task") && on.intake.contains("Every task kebbie starts has its own note in the vault") && !on.intake.contains("status `active`") && on.intake.contains("kebbie makes the task's note from it"), "{}", on.intake);
        let off = role_texts(&[Source::Vault], false);
        assert!(off.after_push.contains("log_append") && off.intake.contains("plain task"), "without the vault plugin the lead logs as before");
        assert!(task_prompt(&entry("vault"), &[Source::Vault], true).contains("kebbie has made it this task's note") && !task_prompt(&entry("vault"), &[Source::Vault], true).contains("set its status"));
    }

    #[test]
    fn with_the_local_source_the_lead_keeps_the_file_itself() {
        let Role { finding, backlog, intake, .. } = role_texts(&[Source::Local], false);
        assert!(finding.starts_with("the local backlog"));
        for needed in ["one markdown file per item", "delete an item's file once its task is pushed", "`local:` and its file name", "`to write`", "`drafting`", "write it out by the skill `kebbie:write-backlog-item` in its own file and tell the user you did"] {
            assert!(backlog.contains(needed), "{needed}: {backlog}");
        }
        assert!(!backlog.contains("kebbie backlog save") && !backlog.contains("vault") && !backlog.contains("Linear"), "{backlog}");
        assert!(intake.contains("kebbie backlog add --priority <priority> --repo <repo> \"<Title>\" -") && intake.contains("--backlog <the ref kebbie backlog add printed>") && intake.contains("kebbie -C <repo> link <task> <the ref kebbie backlog add printed>") && !intake.contains("vault"), "{intake}");
        for needed in ["Write it out with the skill `kebbie:write-backlog-item`", "A short item the user adds outside this chat", "never in `kebbie ask` and never under Needs you", "fold the answer into the item"] {
            assert!(intake.contains(needed), "{needed}: {intake}");
        }
        let local_and_linear = role_texts(&[Source::Local, Source::Linear], false).backlog;
        assert!(local_and_linear.contains("one markdown file per item") && local_and_linear.contains("The board's Backlog tab also shows what you save") && local_and_linear.contains("`source` (`linear`)"), "{local_and_linear}");
    }

    #[test]
    fn the_newest_comes_first_within_a_priority() {
        let note = |reference: &str, priority: &str, age: &str| Entry { reference: reference.to_string(), priority: priority.to_string(), age: age.to_string(), ..entry("vault") };
        let mut entries = vec![note("old", "high", "9 d"), note("today-early", "high", "0 d"), note("low", "low", "0 d"), note("today-late", "high", "0 d")];
        sort(&mut entries, |entry| entry, |entry| Some(match entry.reference.as_str() { "today-early" => 100.0, "today-late" => 200.0, _ => 50.0 }));
        let order: Vec<&str> = entries.iter().map(|entry| entry.reference.as_str()).collect();
        assert_eq!(order, ["today-late", "today-early", "old", "low"]);
    }

    #[test]
    fn backlog_notes_missing_from_the_snapshot_are_added_with_their_age() {
        let snapshot = vec![Entry { reference: "Projects/known.md".to_string(), ..entry("vault") }];
        let row = |key: &str| [key.to_string(), "Title".to_string(), "Sum".to_string(), "high".to_string(), "[[kebbie]]".to_string(), "2026-10-15".to_string()];
        let added = missing_notes(&snapshot, &[row("Projects/known.md"), row("Projects/new.md")], epoch_day("2026-10-18").unwrap());
        assert_eq!(added.len(), 1);
        assert_eq!((added[0].reference.as_str(), added[0].area.as_str(), added[0].age.as_str()), ("Projects/new.md", "kebbie", "3 d"));
        assert_eq!((epoch_day("1970-01-01"), epoch_day("2000-03-01")), (Some(0), Some(11_017)));
    }

    #[test]
    fn a_running_or_closed_item_leaves_the_backlog_and_a_reset_one_returns() {
        let note = |reference: &str| Entry { reference: reference.to_string(), ..entry("vault") };
        let entries = vec![note("Projects/open.md"), note("Projects/running.md"), note("Projects/reset.md"), note("Projects/closed.md"), entry("linear")];
        let status = |key: &str| match key { "Projects/open.md" => Some("backlog".to_string()), "Projects/running.md" | "Projects/reset.md" => Some("active".to_string()), _ => None };
        let running = HashSet::from(["Projects/running.md".to_string()]);
        let kept: Vec<String> = still_backlog(entries, status, &running).into_iter().map(|entry| entry.reference).collect();
        assert_eq!(kept, ["Projects/open.md", "Projects/reset.md", "ABC-1"]);
    }

    #[test]
    fn a_task_prompt_reads_the_source_only_when_it_is_configured() {
        assert!(task_prompt(&entry("linear"), &[Source::Linear], false).starts_with("Work on Linear issue ABC-1: Fix it."));
        assert!(task_prompt(&entry("linear"), &[Source::Vault], false).starts_with("Work on: Fix it. It broke."));
        assert!(task_prompt(&entry("vault"), &[Source::Vault], false).starts_with("Work on the vault note ABC-1"));
        assert!(task_prompt(&entry("vault"), &[Source::Vault], false).contains("set its status to active"));
        assert!(task_prompt(&entry("vault"), &[Source::Vault], false).contains("write the plan into that note and put it to me with kebbie ask --plan vault:ABC-1"));
        for source in ["linear", "vault", "other"] {
            let prompt = task_prompt(&entry(source), &[Source::Vault, Source::Linear], false);
            assert!(prompt.contains("with kebbie ask, one question at a time") && prompt.contains("kebbie ask --plan"), "{prompt}");
        }
    }

    #[test]
    fn the_days_since_a_date_follow_the_civil_calendar() {
        assert_eq!(epoch_day("1970-01-01"), Some(0));
        assert_eq!(epoch_day("1970-01-02"), Some(1));
        assert_eq!(epoch_day("2000-03-01"), Some(11_017));
        assert_eq!(epoch_day("2026-10-20T12:00:00"), Some(20_746), "a timestamp reads by its date");
        assert_eq!(epoch_day("2026-10"), None);
        assert_eq!(epoch_day("yesterday"), None);
        assert_eq!(days_since("2026-10-10", 20_746), Some(10));
        assert_eq!(days_since("2026-10-30", 20_746), Some(0), "a date ahead of today is not negative");
    }

    #[test]
    fn a_saved_entry_needs_a_title_and_an_unknown_priority_reads_as_none() {
        assert!(super::entry(&json!({ "source": "vault", "priority": "high" })).is_none());
        let loose = super::entry(&json!({ "title": "  X  ", "priority": " URGENT ", "ref": " Projects/x.md ", "age": 3 })).unwrap();
        assert_eq!((loose.title.as_str(), loose.priority.as_str(), loose.reference.as_str(), loose.age.as_str()), ("X", "urgent", "Projects/x.md", ""));
        assert_eq!(super::entry(&json!({ "title": "X", "priority": "soon" })).unwrap().priority, "none");
        assert_eq!(age(None), "never");
        assert_eq!(age(Some(now())), "just now");
        assert_eq!(age(Some(now() - 120)), "2 min ago");
        assert_eq!(age(Some(now() - 7200)), "2 h ago");
    }
}
