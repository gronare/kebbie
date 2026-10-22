use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::config::{self, Source};
use crate::sh::Result;

#[derive(Clone, Debug)]
pub struct Entry {
    pub source: String,
    pub title: String,
    pub priority: String,
    pub area: String,
    pub reference: String,
    pub summary: String,
    pub age: String,
}

pub struct Backlog {
    pub updated: Option<u64>,
    pub entries: Vec<Entry>,
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
    })
}

pub fn load() -> Backlog {
    let sources = config::sources().unwrap_or_default();
    let value: Value = file().ok().and_then(|path| fs::read_to_string(path).ok()).and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    let mut entries: Vec<Entry> = value["entries"].as_array().into_iter().flatten().filter_map(entry).filter(|entry| entry.source != "local").collect();
    if sources.contains(&Source::Local) {
        entries.extend(crate::backlog_file::read(&crate::backlog_file::path()));
    }
    let index = crate::vault::Index::open().ok();
    if let Some(index) = index.as_ref().filter(|_| sources.contains(&Source::Vault)) {
        let notes = index.backlog().unwrap_or_default();
        entries.extend(missing_notes(&entries, &notes, today()));
    }
    let status = |key: &str| index.as_ref().map_or(Some("backlog".to_string()), |index| index.status_of(key));
    entries = still_backlog(entries, status, &crate::tasks::started_items());
    sort(&mut entries, |entry| index.as_ref().filter(|_| entry.source == "vault").and_then(|index| index.changed_at(&entry.reference)));
    Backlog { updated: value["updated"].as_u64(), entries }
}

// Backlog notes the lead's last snapshot does not have yet: filed since, or put back by a reset.
fn missing_notes(entries: &[Entry], notes: &[[String; 6]], today: i64) -> Vec<Entry> {
    notes.iter().filter(|[key, ..]| !entries.iter().any(|entry| entry.source == "vault" && &entry.reference == key)).map(|[key, title, summary, priority, area, date]| Entry {
        source: "vault".to_string(),
        title: title.clone(),
        priority: if PRIORITIES.contains(&priority.as_str()) { priority.clone() } else { "none".to_string() },
        area: crate::vault::area_name(area),
        reference: key.clone(),
        summary: summary.clone(),
        age: days_since(date, today).map_or(String::new(), |days| format!("{days} d")),
    }).collect()
}

fn today() -> i64 {
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
fn sort(entries: &mut [Entry], changed_at: impl Fn(&Entry) -> Option<f64>) {
    let days = |entry: &Entry| entry.age.split_whitespace().next().and_then(|days| days.parse::<u64>().ok()).unwrap_or(u64::MAX);
    let mut keyed: Vec<(usize, u64, f64, Entry)> = entries.iter().map(|entry| {
        (PRIORITIES.iter().position(|priority| *priority == entry.priority).unwrap_or(PRIORITIES.len()), days(entry), -changed_at(entry).unwrap_or(0.0), entry.clone())
    }).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)));
    for (slot, (_, _, _, entry)) in entries.iter_mut().zip(keyed) {
        *slot = entry;
    }
}

// An item a running task came from, or a vault note closed since the snapshot, leaves the Backlog column without waiting for a refresh; a reset task's note shows again even while it still reads active.
fn still_backlog(entries: Vec<Entry>, status: impl Fn(&str) -> Option<String>, running: &HashSet<String>) -> Vec<Entry> {
    entries.into_iter()
        .filter(|entry| !running.contains(&entry.reference))
        .filter(|entry| entry.source != "vault" || matches!(status(&entry.reference).as_deref(), Some("backlog" | "active")))
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

// The sources the lead fetches and saves with `kelpie backlog save`; the local file is read as it is.
pub fn fetched(sources: &[Source]) -> Vec<Source> {
    sources.iter().copied().filter(|source| *source != Source::Local).collect()
}

fn local_file() -> String {
    crate::backlog_file::path().display().to_string()
}

fn wanted(source: Source) -> &'static str {
    match source {
        Source::Local => "the local backlog file",
        Source::Vault => "the vault backlog",
        Source::Linear => "my open Linear issues",
    }
}

fn reference(source: Source) -> &'static str {
    match source {
        Source::Local => "`local:` and the title",
        Source::Vault => "the vault path",
        Source::Linear => "the Linear id",
    }
}

pub fn from(sources: &[Source]) -> String {
    joined(&fetched(sources), |source| if source == Source::Vault { "the vault" } else { "Linear" }, " and ")
}

pub fn label(sources: &[Source]) -> String {
    if sources.is_empty() { "no source".to_string() } else { joined(sources, Source::title, " + ") }
}

pub fn freshness(sources: &[Source], updated: Option<u64>) -> String {
    if fetched(sources).is_empty() { "live".to_string() } else { age(updated) }
}

pub fn not_configured() -> String {
    format!("No backlog source is configured; set [board.backlog] sources = [\"local\"] (or \"vault\", \"linear\", in any combination) in {}.", config::path().display())
}

fn nothing_to_refresh(sources: &[Source]) -> String {
    if sources.contains(&Source::Local) { format!("The Backlog column reads {} as it changes; there is nothing for the lead to refresh.", local_file()) } else { not_configured() }
}

pub fn refresh_label(sources: &[Source]) -> String {
    if fetched(sources).is_empty() { nothing_to_refresh(sources) } else { format!("the lead fetches {} again", from(sources)) }
}

pub fn empty_text(sources: &[Source]) -> String {
    match (sources.contains(&Source::Local), fetched(sources).is_empty()) {
        (false, true) => not_configured(),
        (true, true) => format!("Empty. a adds an item, or write one in {}: {}", local_file(), crate::backlog_file::FORMAT),
        (local, false) => format!("Empty. R asks the lead to fill it from {}{}.", from(sources), if local { "; a adds an item" } else { "" }),
    }
}

pub fn refresh_prompt(configured: &[Source]) -> Result<String> {
    let sources = fetched(configured);
    if sources.is_empty() {
        return Err(nothing_to_refresh(configured));
    }
    Ok(format!(
        "Refresh the kelpie backlog: take {} and pipe them as one JSON array to `$HOME/.local/share/kelpie/current/target/release/kelpie backlog save`. \
         Fields per entry: source ({}), title, priority (urgent, high, medium, low or none), area (the folder name of the repo the work happens in), \
         ref ({}), summary (one line), age (e.g. 3 d). Reply in one line when saved.",
        joined(&sources, wanted, " and "), joined(&sources, Source::key, " or "), joined(&sources, reference, " or "),
    ))
}

pub fn task_prompt(entry: &Entry, sources: &[Source], vault: bool) -> String {
    match entry.source.as_str() {
        "vault" if vault => format!("Work on the vault note {reference} (\"{}\"); kelpie has made it this task's note. Read it with the vault tools first, then ask me what you need decided with kelpie ask, one question at a time, before you plan; then write the plan into that note and put it to me with kelpie ask --plan vault:{reference}, and write no code until I approve it.", entry.title, reference = entry.reference),
        "linear" if sources.contains(&Source::Linear) => format!("Work on Linear issue {}: {}. Read the issue first, then ask me what you need decided with kelpie ask, one question at a time, before you plan; then put the plan to me with kelpie ask --plan, written where your role says, and write no code until I approve it.", entry.reference, entry.title),
        "vault" if sources.contains(&Source::Vault) => format!("Work on the vault note {reference} (\"{}\"). Read it with the vault tools first and set its status to active, then ask me what you need decided with kelpie ask, one question at a time, before you plan; then write the plan into that note and put it to me with kelpie ask --plan vault:{reference}, and write no code until I approve it.", entry.title, reference = entry.reference),
        _ => format!("Work on: {}. {} Ask me what you need decided with kelpie ask, one question at a time, before you plan; then put the plan to me with kelpie ask --plan, written where your role says, and write no code until I approve it.", entry.title, entry.summary),
    }
}

// The title of a backlog item by its reference, from the lead's snapshot or the vault index, whatever its status now.
pub fn title_of(item: &str) -> Option<String> {
    if let Some(title) = local_title(item) {
        return Some(title.to_string());
    }
    let value: Value = file().ok().and_then(|path| fs::read_to_string(path).ok()).and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    let cached = value["entries"].as_array().into_iter().flatten().filter_map(entry).find(|entry| entry.reference == item).map(|entry| entry.title);
    cached.or_else(|| crate::vault::Index::open().ok()?.title_of(item))
}

fn local_title(item: &str) -> Option<&str> {
    item.strip_prefix(crate::backlog_file::PREFIX)
}

pub fn reset_notice(item: &str) -> String {
    match local_title(item) {
        Some(title) => format!(" Nobody works on the backlog item \"{title}\" now; its line in {} shows in the Backlog column again.", local_file()),
        None => format!(" Nobody works on {item} now: set it back to backlog."),
    }
}

pub fn done_notice(task: &str, item: &str) -> String {
    match local_title(item) {
        Some(title) => format!("{task} is done and its commits are on the default branch. It came from the backlog item \"{title}\": log it, then delete its line from {}, or reword the line when work is left.", local_file()),
        None => format!("{task} is done and its commits are on the default branch. It came from {item}: log it, then close {item}, or update it when work is left."),
    }
}

fn role_wanted(source: Source) -> &'static str {
    match source {
        Source::Local => "the local backlog file",
        Source::Vault => "the vault backlog (the vault's backlog tool)",
        Source::Linear => "the user's open Linear issues (assigned to them, not done; Linear's MCP)",
    }
}

fn role_finding(source: Source) -> &'static str {
    match source {
        Source::Local => "the local backlog (the board's Backlog column, or `kelpie backlog`) by priority",
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
const AFTER_PUSH: &str = "- `[from kelpie] <who> pushed <task>: <n> commits to <branch> (<sha> <subject>; …)` follows every push, the user's or yours. Write the repo's log line in the vault with `log_append`: what landed, with those shas. When the task is done, close its note (`close`, with `merged_into` when the area note took it in) or append to it what landed.";
const KEPT_VAULT_WORK: &str = "- **The vault**: kelpie keeps each task's note: it opens one when the task starts (or claims the backlog note named with `--backlog`), the task writes its Problem before its first change and its solution and decisions as it goes, kelpie writes the repo's log line after each push and closes the note at done. Your part is the area notes and the user's decisions that reach beyond one task. New work goes in the backlog as under Taking in ideas.";
const KEPT_AFTER_PUSH: &str = "- `[from kelpie] <who> pushed <task>: <n> commits to <branch> (<sha> <subject>; …)` follows every push, the user's or yours; kelpie has written the repo's log line from those commits. Update the area note with what the push changed in it.";
const KEPT_TASKS: &str = "Every task kelpie starts has its own note in the vault: kelpie opens it at the start, or claims the backlog note named with `--backlog`, and the task writes it from your prompt. Done waits until the note has its solution and decisions, kelpie then closes it, and a reset puts it back in the backlog.";

fn local_role() -> String {
    format!(
        "The Backlog column reads `{}` as it changes. One item per line: `{}`, where the priority is `urgent`, `high`, `medium` or `low` (none when left out), \
         the summary and area are optional, and the area is the folder name of the repo the work happens in. You keep this file: add new work with \
         `kelpie backlog add --priority <priority> \"<Title> — <summary> (<area>)\"`, change a priority or reword an item in place, and delete an item's line once its task is pushed or the user drops it. \
         The title is the item's reference (`local:<Title>` to `kelpie start --backlog`), so keep titles unique and leave the title of an item a task runs as it is. \
         Lines that are not items are the user's; leave them as they are.",
        local_file(), crate::backlog_file::FORMAT,
    )
}

fn fetched_role(sources: &[Source], also: bool) -> String {
    format!(
        "The board's Backlog tab {}shows what you save with `kelpie backlog save`. When the user asks you to refresh it, or when it is more than a day old at a briefing: \
         take {} and pipe them as one JSON array to `kelpie backlog save`. Each entry: `source` ({}), `title`, `priority` (`urgent`, `high`, `medium`, `low` or `none`), \
         `area` (the folder name of the repo the work happens in; not the vault area when the two differ, since starting the task looks the repo up by this name), \
         `ref` ({}), `summary` (one line) and `age` (e.g. `3 d`, `1 mo`).",
        if also { "also " } else { "" },
        joined(sources, role_wanted, " and "), joined(sources, |source| if source == Source::Vault { "`vault`" } else { "`linear`" }, " or "), joined(sources, reference, " or "),
    )
}

struct Intake {
    draft: &'static str,
    start: &'static str,
    file: &'static str,
    grown: &'static str,
    reference: &'static str,
}

fn intake_for(source: Source) -> Intake {
    match source {
        Source::Local => Intake {
            draft: "one line for the local backlog: `- [priority] Title — one-line summary (area)`",
            start: "add it with `kelpie backlog add`, then start the task",
            file: "add it with `kelpie backlog add`",
            grown: "add a line for it with `kelpie backlog add`",
            reference: "\"local:<Title>\"",
        },
        Source::Vault => Intake {
            draft: "a task note for the vault's Projects/: title, area, priority, a one-line summary, and a body of problem, solution from the user's side, decisions, test decisions and what is out of scope",
            start: "write the note with the vault tools, status `active`, then start the task",
            file: "write the note with the vault tools, status `backlog`",
            grown: "write a task note for it in the vault's Projects/ with status `active`",
            reference: "<the note's path>",
        },
        Source::Linear => Intake {
            draft: "a Linear issue: title, priority, team, and a description of the problem, the change from the user's side and what done looks like",
            start: "create the issue with Linear's MCP, assigned to the user and in progress, then start the task",
            file: "create the issue with Linear's MCP, assigned to the user",
            grown: "create a Linear issue for it, assigned to the user and in progress",
            reference: "<the issue id>",
        },
    }
}

fn intake_role(sources: &[Source], vault: bool) -> String {
    let Some(first) = sources.first() else {
        return format!(
            "When the user brings an idea rather than a task to start, draft a task prompt for it, show it, and start it with `kelpie start` on yes. \
             No backlog source is configured, so there is nowhere to file it: say so, and that `[board.backlog] sources` in {} turns one on.",
            config::path().display(),
        );
    };
    let Intake { draft, start, file, grown, reference } = intake_for(*first);
    if vault {
        let file = if *first == Source::Vault { "write a task note with the vault tools in Projects/, status `backlog`" } else { file };
        let backlog = if *first == Source::Vault { String::new() } else { format!(", or `--backlog {reference}` once it is filed") };
        return format!(
            "When the user brings an idea rather than a task to start, take it in:\n\n\
             1. Draft it as {draft}. Read what bears on it first (the area's notes, the code, the tasks that touch it), so the draft names the real problem and the repo.\n\
             2. Show the user the draft, and ask with the AskUserQuestion tool: start it now as a task, file it in the backlog, or change it first.\n\
             3. On start: start it with a prompt written as under Starting a task: `kelpie start <name> \"<prompt>\" --repo <repo>`{backlog}. On file: {file}. On change: redraft and ask again.\n\n\
             The idea is taken in when the user has picked start or file and the task or the item exists.\n\n{KEPT_TASKS}",
        );
    }
    format!(
        "When the user brings an idea rather than a task to start, take it in:\n\n\
         1. Draft it as {draft}. Read what bears on it first (the area's notes, the code, the tasks that touch it), so the draft names the real problem and the repo.\n\
         2. Show the user the draft, and ask with the AskUserQuestion tool: start it now as a task, file it in the backlog, or change it first.\n\
         3. On start: {start} with a prompt written as under Starting a task: `kelpie start <name> \"<prompt>\" --repo <repo> --backlog {reference}`. On file: {file}. On change: redraft and ask again.\n\n\
         The idea is taken in when the user has picked start or file and the item exists.\n\n\
         A task started without a backlog item stays a plain task. When it grows into a project (it spans sessions or carries decisions), {grown}, and link it: \
         `kelpie -C <repo> link <task> {reference}`. A linked task counts as started from that item: kelpie tells you when it is done, so you log it and close or update the item, \
         and a reset puts the item back in the backlog.",
    )
}

// Filled into roles/lead.md: {finding}, {backlog}, {intake}, {vault_work} and {after_push}; vault is whether kelpie keeps the tasks' notes itself.
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
    let mut lines = vec![if fetched(&sources).is_empty() { format!("from {}", local_file()) } else { format!("updated {}", age(backlog.updated)) }];
    for entry in &backlog.entries {
        lines.push(format!("  [{}] {}  ({} · {})", entry.priority, entry.title, entry.source, entry.area));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str) -> Entry {
        Entry { source: source.to_string(), title: "Fix it".to_string(), priority: "high".to_string(), area: "app".to_string(), reference: "ABC-1".to_string(), summary: "It broke.".to_string(), age: "3 d".to_string() }
    }

    #[test]
    fn the_refresh_prompt_names_only_the_configured_sources() {
        let both = refresh_prompt(&[Source::Vault, Source::Linear]).unwrap();
        assert!(both.starts_with("Refresh the kelpie backlog: take the vault backlog and my open Linear issues and pipe"));
        assert!(both.contains("source (vault or linear)") && both.contains("ref (the vault path or the Linear id)"));
        let vault = refresh_prompt(&[Source::Vault]).unwrap();
        assert!(vault.contains("take the vault backlog and pipe") && vault.contains("source (vault)") && !vault.contains("Linear"));
        let linear = refresh_prompt(&[Source::Linear]).unwrap();
        assert!(linear.contains("take my open Linear issues and pipe") && !linear.contains("vault"));
        let none = refresh_prompt(&[]).unwrap_err();
        assert!(none.starts_with("No backlog source is configured") && none.contains("[board.backlog] sources"));
        let local = refresh_prompt(&[Source::Local]).unwrap_err();
        assert!(local.contains("reads") && local.contains("backlog.md") && local.contains("nothing for the lead to refresh"), "{local}");
        assert_eq!(refresh_prompt(&[Source::Local, Source::Vault]).unwrap(), vault, "the lead never saves the local file's items");
    }

    #[test]
    fn the_board_texts_tell_a_local_backlog_from_a_fetched_one() {
        assert_eq!(freshness(&[Source::Local], None), "live");
        assert_eq!(freshness(&[Source::Local, Source::Vault], None), "never");
        assert_eq!(from(&[Source::Local, Source::Linear]), "Linear");
        assert!(refresh_label(&[Source::Local]).contains("nothing for the lead to refresh"));
        assert_eq!(refresh_label(&[Source::Local, Source::Vault]), "the lead fetches the vault again");
        assert!(empty_text(&[Source::Local]).starts_with("Empty. a adds an item, or write one in ") && empty_text(&[Source::Local]).ends_with(crate::backlog_file::FORMAT));
        assert_eq!(empty_text(&[Source::Vault]), "Empty. R asks the lead to fill it from the vault.");
        assert_eq!(empty_text(&[Source::Local, Source::Vault]), "Empty. R asks the lead to fill it from the vault; a adds an item.");
        assert!(empty_text(&[]).starts_with("No backlog source is configured"));
    }

    #[test]
    fn a_reset_or_finished_local_item_is_told_by_its_title_and_file() {
        let reset = reset_notice("local:Fix login");
        assert!(reset.contains("\"Fix login\"") && reset.contains("backlog.md") && !reset.contains("set it back"), "{reset}");
        assert_eq!(reset_notice("Projects/x.md"), " Nobody works on Projects/x.md now: set it back to backlog.");
        let done = done_notice("login", "local:Fix login");
        assert!(done.starts_with("login is done") && done.contains("delete its line from") && done.contains("\"Fix login\""), "{done}");
        let note = done_notice("login", "Projects/x.md");
        assert!(note.contains("It came from Projects/x.md: log it, then close Projects/x.md, or update it when work is left."), "{note}");
    }

    #[test]
    fn a_local_item_leaves_the_backlog_while_its_task_runs() {
        let items = crate::backlog_file::entries("- [high] Fix login (app)\n- Other\n");
        let running = HashSet::from([crate::backlog_file::reference("Fix login")]);
        let kept: Vec<String> = still_backlog(items, |_| None, &running).into_iter().map(|entry| entry.title).collect();
        assert_eq!(kept, ["Other"], "a local item needs no vault status to stay");
    }

    #[test]
    fn labels_and_the_role_follow_the_sources() {
        assert_eq!(label(&[Source::Vault, Source::Linear]), "vault + Linear");
        assert_eq!(label(&[]), "no source");
        assert_eq!(from(&[Source::Vault]), "the vault");
        let Role { finding, backlog, intake, .. } = role_texts(&[Source::Vault], false);
        assert_eq!(finding, "the vault backlog by priority");
        assert!(!backlog.contains("Linear") && backlog.contains("`source` (`vault`)") && !backlog.contains("backlog.md"));
        assert!(intake.contains("task note for the vault's Projects/") && intake.contains("--backlog <the note's path>") && !intake.contains("Linear") && !intake.contains("local:"));
        let Role { finding, backlog, intake, .. } = role_texts(&[], false);
        assert!(!finding.contains("Linear") && !backlog.contains("Linear") && backlog.contains("No backlog source") && intake.contains("nowhere to file it"));
        assert!(role_texts(&[Source::Linear, Source::Vault], false).backlog.contains("Linear issues (assigned to them, not done; Linear's MCP) and the vault backlog"));
        assert!(role_texts(&[Source::Linear, Source::Vault], false).intake.contains("Draft it as a Linear issue"), "the first source listed takes the ideas in");
    }

    #[test]
    fn with_the_vault_on_kelpie_keeps_the_notes_and_the_log_so_the_lead_does_not() {
        let on = role_texts(&[Source::Vault], true);
        assert!(!on.after_push.contains("log_append") && on.after_push.contains("kelpie has written the repo's log line") && on.after_push.contains("area note"), "{}", on.after_push);
        assert!(on.vault_work.contains("kelpie keeps each task's note") && !on.vault_work.contains("repo log, the user's"), "{}", on.vault_work);
        assert!(!on.intake.contains("plain task") && on.intake.contains("Every task kelpie starts has its own note in the vault") && !on.intake.contains("status `active`"), "{}", on.intake);
        let off = role_texts(&[Source::Vault], false);
        assert!(off.after_push.contains("log_append") && off.intake.contains("plain task"), "without the vault plugin the lead logs as before");
        assert!(task_prompt(&entry("vault"), &[Source::Vault], true).contains("kelpie has made it this task's note") && !task_prompt(&entry("vault"), &[Source::Vault], true).contains("set its status"));
    }

    #[test]
    fn with_the_local_source_the_lead_keeps_the_file_itself() {
        let Role { finding, backlog, intake, .. } = role_texts(&[Source::Local], false);
        assert!(finding.starts_with("the local backlog"));
        for needed in ["backlog.md", "kelpie backlog add --priority", "delete an item's line once its task is pushed", "`local:<Title>`", "leave them as they are"] {
            assert!(backlog.contains(needed), "{needed}: {backlog}");
        }
        assert!(!backlog.contains("kelpie backlog save") && !backlog.contains("vault") && !backlog.contains("Linear"), "{backlog}");
        assert!(intake.contains("--backlog \"local:<Title>\"") && intake.contains("kelpie -C <repo> link <task> \"local:<Title>\"") && !intake.contains("vault"), "{intake}");
        let both = role_texts(&[Source::Local, Source::Vault], false).backlog;
        assert!(both.contains("backlog.md") && both.contains("The board's Backlog tab also shows what you save") && both.contains("`source` (`vault`)") && !both.contains("`local`)"), "{both}");
    }

    #[test]
    fn the_newest_comes_first_within_a_priority() {
        let note = |reference: &str, priority: &str, age: &str| Entry { reference: reference.to_string(), priority: priority.to_string(), age: age.to_string(), ..entry("vault") };
        let mut entries = vec![note("old", "high", "9 d"), note("today-early", "high", "0 d"), note("low", "low", "0 d"), note("today-late", "high", "0 d")];
        sort(&mut entries, |entry| Some(match entry.reference.as_str() { "today-early" => 100.0, "today-late" => 200.0, _ => 50.0 }));
        let order: Vec<&str> = entries.iter().map(|entry| entry.reference.as_str()).collect();
        assert_eq!(order, ["today-late", "today-early", "old", "low"]);
    }

    #[test]
    fn backlog_notes_missing_from_the_snapshot_are_added_with_their_age() {
        let snapshot = vec![Entry { reference: "Projects/known.md".to_string(), ..entry("vault") }];
        let row = |key: &str| [key.to_string(), "Title".to_string(), "Sum".to_string(), "high".to_string(), "[[kelpie]]".to_string(), "2026-10-15".to_string()];
        let added = missing_notes(&snapshot, &[row("Projects/known.md"), row("Projects/new.md")], epoch_day("2026-10-18").unwrap());
        assert_eq!(added.len(), 1);
        assert_eq!((added[0].reference.as_str(), added[0].area.as_str(), added[0].age.as_str()), ("Projects/new.md", "kelpie", "3 d"));
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
        assert!(task_prompt(&entry("vault"), &[Source::Vault], false).contains("write the plan into that note and put it to me with kelpie ask --plan vault:ABC-1"));
        for source in ["linear", "vault", "other"] {
            let prompt = task_prompt(&entry(source), &[Source::Vault, Source::Linear], false);
            assert!(prompt.contains("with kelpie ask, one question at a time") && prompt.contains("kelpie ask --plan"), "{prompt}");
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
