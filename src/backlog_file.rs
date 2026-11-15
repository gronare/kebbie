use std::path::{Path, PathBuf};

use crate::backlog::{Entry, PRIORITIES};
use crate::config::Source;
pub use crate::item_template::Question;
use crate::item_template::Template;
use crate::notice::{Notice, Tone};
use crate::sh::Result;

pub const PREFIX: &str = "local:";
pub const SKILL: &str = "kebbie:write-backlog-item";
const DRAFT: &str = "draft";
const DRAFTING: &str = "drafting";
const TO_WRITE: &str = "To write: the lead writes this item out with the skill kebbie:write-backlog-item.";
const OLD_FILE: &str = "backlog.md";

pub fn dir() -> PathBuf {
    crate::config::path().with_file_name("backlog")
}

pub fn reference(stem: &str) -> String {
    format!("{PREFIX}{stem}")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub stem: String,
    pub title: String,
    pub priority: String,
    pub repo: String,
    pub status: String,
    pub filed: String,
    pub body: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    ToWrite,
    Drafting,
    Ready,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            State::ToWrite => "to write",
            State::Drafting => "drafting",
            State::Ready => "ready",
        }
    }
}

// Blocking questions waiting on the user, answers of theirs the lead has yet to fold in, and the state those leave; Linear has none. `into` is the older note a kept row was merged into.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub state: Option<State>,
    pub waiting: usize,
    pub with_lead: usize,
    pub kept: bool,
    pub filed: String,
    pub into: String,
}

// An item short of a heading or of a required section's text is to write; the lead marks one drafting while it writes it out; a blocking question keeps it drafting.
pub fn state_of(drafting: bool, short: bool, body: &str, template: &Template) -> State {
    if drafting {
        State::Drafting
    } else if short || !template.missing(body).is_empty() {
        State::ToWrite
    } else if template.questions_in(body).iter().any(|question| question.blocks) {
        State::Drafting
    } else {
        State::Ready
    }
}

pub fn shape_of(state: State, body: &str, answered: &[String], filed: &str, template: &Template) -> Shape {
    let blocking: Vec<Question> = template.questions_in(body).into_iter().filter(|question| question.blocks).collect();
    let with_lead = blocking.iter().filter(|question| answered.contains(&question.text)).count();
    Shape { state: Some(state), waiting: blocking.len() - with_lead, with_lead, filed: filed.to_string(), ..Shape::default() }
}

impl Item {
    pub fn to_write(&self) -> bool {
        self.status == DRAFT || !crate::item_template::local().missing(&self.body).is_empty()
    }

    pub fn state(&self) -> State {
        state_of(self.status == DRAFTING, self.status == DRAFT, &self.body, &crate::item_template::local())
    }

    pub fn reference(&self) -> String {
        reference(&self.stem)
    }

    pub fn text(&self) -> String {
        let quoted = crate::routine_file::quoted;
        let front: Vec<String> = [("title", &self.title), ("priority", &self.priority), ("repo", &self.repo), ("status", &self.status), ("filed", &self.filed)]
            .into_iter().map(|(key, value)| format!("{key} = {}", quoted(value))).collect();
        format!("+++\n{}\n+++\n\n{}\n", front.join("\n"), self.body.trim())
    }

    pub fn entry(&self, today: i64) -> Entry {
        let template = crate::item_template::local();
        Entry {
            source: "local".to_string(),
            title: self.title.clone(),
            priority: self.priority.clone(),
            area: self.repo.clone(),
            reference: self.reference(),
            summary: if self.to_write() { String::new() } else { template.first_line(&self.body) },
            age: crate::backlog::epoch_day(&self.filed).map_or(String::new(), |day| format!("{} d", (today - day).max(0))),
            shape: shape_of(self.state(), &self.body, &[], &self.filed, &template),
        }
    }
}

fn priority_of(word: &str) -> String {
    let word = word.trim().to_lowercase();
    if PRIORITIES.contains(&word.as_str()) { word } else { "none".to_string() }
}

// A file the user wrote by hand without kebbie's header is an item to write: its first line, or its name, is the title.
fn hand_written(stem: &str, text: &str) -> Option<Item> {
    let first = text.lines().map(str::trim).find(|line| !line.is_empty()).map(|line| line.trim_start_matches('#').trim().to_string());
    let title = first.filter(|line| !line.is_empty()).unwrap_or_else(|| stem.replace('-', " "));
    Some(Item { stem: stem.to_string(), title, priority: "none".to_string(), repo: String::new(), status: DRAFT.to_string(), filed: String::new(), body: text.trim().to_string() })
}

pub fn parse(stem: &str, text: &str) -> Option<Item> {
    let Some(rest) = text.strip_prefix("+++\n") else { return hand_written(stem, text) };
    let (front, body) = rest.split_once("\n+++\n").or_else(|| rest.strip_suffix("\n+++").map(|front| (front, "")))?;
    let table: toml::Table = toml::from_str(front).ok()?;
    let field = |key: &str| table.get(key).and_then(|value| value.as_str()).map(str::trim).unwrap_or_default().to_string();
    let title = field("title");
    (!title.is_empty()).then(|| Item {
        stem: stem.to_string(),
        title,
        priority: priority_of(&field("priority")),
        repo: field("repo"),
        status: Some(field("status")).filter(|status| !status.is_empty()).unwrap_or_else(|| "backlog".to_string()),
        filed: field("filed"),
        body: body.trim().to_string(),
    })
}

// One line as typed: `Title — summary (area)`, where the area is a repo's folder name, so a parenthesis with a space in it stays part of the text.
pub fn split_line(line: &str) -> (String, String, String) {
    let rest = line.trim();
    let (rest, area) = match rest.strip_suffix(')').and_then(|inner| inner.rsplit_once(" (")) {
        Some((before, area)) if !area.is_empty() && !area.contains(char::is_whitespace) => (before.trim(), area),
        _ => (rest, ""),
    };
    let (title, summary) = rest.split_once(" — ").or_else(|| rest.split_once(" -- ")).unwrap_or((rest, ""));
    (title.trim().to_string(), summary.trim().to_string(), area.to_string())
}

// A line of the old one-file backlog: a top-level bullet whose bracket, if any, is a priority; a checkbox, a nested bullet or prose is not an item.
pub fn old_line(line: &str) -> Option<(String, String, String, String)> {
    let trimmed = line.trim_end();
    let rest = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* "))?.trim();
    let (priority, rest) = match rest.strip_prefix('[') {
        Some(inner) => {
            let (word, after) = inner.split_once(']')?;
            let word = word.trim().to_lowercase();
            if !PRIORITIES.contains(&word.as_str()) {
                return None;
            }
            (word, after.trim())
        }
        None => ("none".to_string(), rest),
    };
    let (title, summary, area) = split_line(rest);
    (!title.is_empty()).then_some((priority, title, summary, area))
}

pub fn stem_for(title: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut stem = String::new();
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            stem.push(character);
        } else if !stem.is_empty() && !stem.ends_with('-') {
            stem.push('-');
        }
    }
    let stem: String = stem.trim_end_matches('-').chars().take(60).collect();
    let stem = if stem.trim_end_matches('-').is_empty() { "item".to_string() } else { stem.trim_end_matches('-').to_string() };
    (1..).map(|n| if n == 1 { stem.clone() } else { format!("{stem}-{n}") }).find(|candidate| !taken(candidate)).unwrap_or(stem)
}

fn file(dir: &Path, stem: &str) -> PathBuf {
    dir.join(format!("{stem}.md"))
}

pub fn items(dir: &Path) -> Vec<Item> {
    let mut found: Vec<Item> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|entry| {
        let name = entry.file_name().to_str()?.to_string();
        let stem = name.strip_suffix(".md").filter(|stem| !stem.starts_with('.'))?;
        parse(stem, &std::fs::read_to_string(entry.path()).ok()?)
    }).collect();
    found.sort_by(|one, other| one.stem.cmp(&other.stem));
    found
}

pub fn find(item: &str) -> Option<(PathBuf, Item)> {
    let stem = item.strip_prefix(PREFIX)?;
    let dir = dir();
    let path = file(&dir, stem);
    let found = std::fs::read_to_string(&path).ok().and_then(|text| parse(stem, &text))
        .or_else(|| items(&dir).into_iter().find(|item| item.title == stem))?;
    Some((file(&dir, &found.stem), found))
}

// A draft holds every heading: the first section says the lead writes it out, Source keeps the line or the user's words as typed.
fn draft_body(by: &str, filed: &str, line: &str, words: Option<&str>, template: &Template) -> String {
    let first = template.sections.first().map_or("background", |section| section.key.as_str());
    let source = if template.heading("source").is_some() { "source" } else { first };
    let said = match words {
        Some(words) => format!("{by}, {filed}, in their words:\n\n{}", quoted_block(words)),
        None => format!("{by}, {filed}: \"{}\"", line.trim()),
    };
    let texts = if source == first { vec![(first, format!("{TO_WRITE}\n\n{said}"))] } else { vec![(first, TO_WRITE.to_string()), (source, said)] };
    template.body_with(&texts)
}

// The user's words as a quote, line breaks kept, so a line of theirs never reads as one of the template's headings.
fn quoted_block(words: &str) -> String {
    words.trim().lines().map(|line| if line.trim().is_empty() { ">".to_string() } else { format!("> {}", line.trim_end()) }).collect::<Vec<_>>().join("\n")
}

// A description's first words as a title the lead replaces when it writes the item out.
pub fn placeholder_title(words: &str) -> String {
    const WORDS: usize = 6;
    let all: Vec<&str> = words.split_whitespace().collect();
    let title = all.iter().take(WORDS).copied().collect::<Vec<_>>().join(" ");
    if all.len() > WORDS { format!("{}…", title.trim_end_matches(|c: char| c.is_ascii_punctuation())) } else { title }
}

// `words` is a description in the user's own words, and `line` then only a placeholder title the lead replaces.
pub struct Filing<'a> {
    pub priority: &'a str,
    pub repo: &'a str,
    pub line: &'a str,
    pub words: Option<&'a str>,
    pub body: Option<&'a str>,
    pub by: &'a str,
}

fn checked_priority(priority: &str) -> Result<String> {
    match priority.trim().to_lowercase() {
        empty if empty.is_empty() => Ok("none".to_string()),
        word if PRIORITIES.contains(&word.as_str()) => Ok(word),
        other => Notice::new(Tone::You, format!("Not added: \"{other}\" is not a priority")).you("Use urgent, high, medium or low, or leave it empty.").err(),
    }
}

// The item as it will be filed: written out when a body comes with it, else a draft the lead writes out.
pub fn item_from(filing: &Filing, filed: &str) -> Result<Item> {
    item_from_with(filing, filed, &crate::item_template::local())
}

pub fn item_from_with(filing: &Filing, filed: &str, template: &Template) -> Result<Item> {
    let priority = checked_priority(filing.priority)?;
    let (title, _, area) = split_line(filing.line);
    if title.is_empty() {
        return Notice::new(Tone::You, "Not added: the item needs a title").you("Give it a title.").err();
    }
    let repo = Some(filing.repo.trim()).filter(|repo| !repo.is_empty()).unwrap_or(&area).to_string();
    let (status, body) = match filing.body.map(str::trim).filter(|body| !body.is_empty()) {
        Some(body) => {
            let missing = template.missing(body);
            if !missing.is_empty() {
                return Notice::new(Tone::You, "Not added: the item is not written out").you(format!("Write it with the skill {SKILL}: it needs {}; kebbie backlog template prints the sections.", missing.join(", "))).err();
            }
            ("backlog".to_string(), body.to_string())
        }
        None => (DRAFT.to_string(), draft_body(filing.by, filed, filing.line, filing.words, template)),
    };
    Ok(Item { stem: String::new(), title, priority, repo, status, filed: filed.to_string(), body })
}

pub fn add_in(dir: &Path, filing: &Filing, filed: &str) -> Result<(PathBuf, Item)> {
    let mut item = item_from(filing, filed)?;
    let existing = items(dir);
    if existing.iter().any(|other| other.title == item.title) {
        return Notice::new(Tone::You, format!("Not added: \"{}\" is already in the backlog", item.title)).you("Give this one another title.").err();
    }
    item.stem = stem_for(&item.title, |stem| file(dir, stem).exists());
    let path = file(dir, &item.stem);
    crate::state::write(&path, item.text())?;
    Ok((path, item))
}

// The one-file backlog of earlier versions becomes one draft item per line, once; the file stays as a backup beside the folder.
pub fn migrate(dir: &Path, filed: &str) -> Result<usize> {
    let old = dir.with_file_name(OLD_FILE);
    let Ok(text) = std::fs::read_to_string(&old) else { return Ok(0) };
    let mut moved = 0;
    for (priority, title, summary, area) in text.lines().filter_map(old_line) {
        if items(dir).iter().any(|item| item.title == title) {
            continue;
        }
        let line = if summary.is_empty() { title.clone() } else { format!("{title} — {summary}") };
        let stem = stem_for(&title, |stem| file(dir, stem).exists());
        let item = Item { stem: stem.clone(), title, priority, repo: area, status: DRAFT.to_string(), filed: filed.to_string(), body: draft_body(&format!("moved from {OLD_FILE}"), filed, &line, None, &crate::item_template::local()) };
        crate::state::write(&file(dir, &stem), item.text())?;
        moved += 1;
    }
    let backup = (0..).map(|n| if n == 0 { old.with_file_name(format!("{OLD_FILE}.bak")) } else { old.with_file_name(format!("{OLD_FILE}.bak.{n}")) }).find(|path| !path.exists()).unwrap_or_else(|| old.with_file_name(format!("{OLD_FILE}.bak")));
    std::fs::rename(&old, &backup).map_err(|error| format!("{}: {error}", old.display()))?;
    Ok(moved)
}

pub fn migrate_once() {
    let dir = dir();
    if dir.with_file_name(OLD_FILE).is_file() {
        let _ = migrate(&dir, &crate::issues::today(crate::messages::now())).inspect_err(|error| eprintln!("Could not move {OLD_FILE} into item files: {error}"));
    }
}

fn told_file(state: &Path) -> PathBuf {
    state.join("backlog-told")
}

fn told(state: &Path) -> Vec<String> {
    std::fs::read_to_string(told_file(state)).unwrap_or_default().lines().map(str::to_string).collect()
}

// A stem whose file is gone leaves the list, so it stays as long as the backlog and a new item under an old name is told.
fn mark_told(state: &Path, dir: &Path, stems: &[String]) {
    let mut all: Vec<String> = told(state).into_iter().filter(|stem| file(dir, stem).exists()).collect();
    all.extend(stems.iter().filter(|stem| !all.contains(stem)).cloned().collect::<Vec<_>>());
    let _ = crate::state::write(&told_file(state), all.join("\n") + "\n");
}

// Items to write the lead has not heard of (a file the user wrote by hand, or one moved from the old file), each told once.
pub fn untold(items: &[Item], told: &[String]) -> Vec<Item> {
    items.iter().filter(|item| item.state() == State::ToWrite && !told.contains(&item.stem)).cloned().collect()
}

pub fn tell_new_drafts() {
    let Ok(state) = crate::lead::state_dir() else { return };
    let dir = dir();
    let new = untold(&items(&dir), &told(&state));
    if new.is_empty() {
        return;
    }
    let listed: Vec<String> = new.iter().map(|item| format!("\"{}\" ({})", item.title, file(&dir, &item.stem).display())).collect();
    let text = format!("The backlog has {} item{} to write out: {}. Write each out now with the skill `{SKILL}` in its own file, show the user the written item, and say it waited for you if it did.", new.len(), if new.len() == 1 { "" } else { "s" }, listed.join("; "));
    if crate::messages::to_lead(crate::messages::ToLead::BacklogItem, &text).is_ok() {
        mark_told(&state, &dir, &new.iter().map(|item| item.stem.clone()).collect::<Vec<_>>());
    }
}

// At most this many items to write go to the lead at once when the user writes out all of them, so a vault of old notes does not bury it: the next go once it has written these, each needing the code and the notes read.
pub const BATCH: usize = 5;
const QUEUE: &str = "backlog-write-queue.json";

// An item waiting to be written out, as the lead is told of it: its reference, its title and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Unwritten {
    pub reference: String,
    pub title: String,
    pub place: String,
}

pub fn unwritten(entries: &[Entry], dir: &Path) -> Vec<Unwritten> {
    entries.iter().filter(|entry| entry.shape.state == Some(State::ToWrite) && !entry.shape.kept).filter_map(|entry| {
        let place = match entry.source.as_str() {
            "local" => file(dir, entry.reference.strip_prefix(PREFIX)?).display().to_string(),
            "vault" => format!("vault: {}", entry.reference),
            _ => return None,
        };
        Some(Unwritten { reference: entry.reference.clone(), title: entry.title.clone(), place })
    }).collect()
}

pub fn batch_text(new: &[Unwritten], later: usize) -> String {
    let listed: Vec<String> = new.iter().map(|item| format!("\"{}\" ({})", item.title, item.place)).collect();
    let rest = match later {
        0 => String::new(),
        1 => " One more waits; kebbie sends it once these are written.".to_string(),
        more => format!(" {more} more wait; kebbie sends the next {BATCH} once these are written."),
    };
    format!("The user asks you to write out every backlog item marked to write. Here {}: {}. Write each out now with the skill `{SKILL}` where it is (its file, or its vault note), and show the user the written items.{rest}", if new.len() == 1 { "is the first".to_string() } else { format!("are {}", new.len()) }, listed.join("; "))
}

// What the user queued with "write out all": every item then to write, and those already sent to the lead; kept in kebbie's state so it outlives a restart.
#[derive(Debug, Default, PartialEq)]
struct Queue {
    items: Vec<Unwritten>,
    sent: Vec<String>,
}

fn queue_file(state: &Path) -> PathBuf {
    state.join(QUEUE)
}

fn read_queue(state: &Path) -> Queue {
    let value: serde_json::Value = std::fs::read_to_string(queue_file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let text = |item: &serde_json::Value, key: &str| item[key].as_str().unwrap_or_default().to_string();
    let items = value["items"].as_array().into_iter().flatten().map(|item| Unwritten { reference: text(item, "reference"), title: text(item, "title"), place: text(item, "place") }).filter(|item| !item.reference.is_empty()).collect();
    let sent = value["sent"].as_array().into_iter().flatten().filter_map(|sent| sent.as_str().map(str::to_string)).collect();
    Queue { items, sent }
}

fn write_queue(state: &Path, queue: &Queue) -> Result<()> {
    if queue.items.is_empty() {
        return match std::fs::remove_file(queue_file(state)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        };
    }
    let items: Vec<serde_json::Value> = queue.items.iter().map(|item| serde_json::json!({ "reference": item.reference, "title": item.title, "place": item.place })).collect();
    crate::state::write(&queue_file(state), serde_json::json!({ "items": items, "sent": queue.sent }).to_string())
}

// How many items the queue still holds, sent to the lead or waiting their turn; 0 when none runs.
pub fn queued(state: &Path) -> usize {
    read_queue(state).items.len()
}

// Queues every item to write and sends the lead the first batch; a queue already running is left as it is.
pub fn write_all_in(state: &Path, dir: &Path, entries: &[Entry], tell: &dyn Fn(&str) -> Result<()>) -> Result<String> {
    {
        let _locked = crate::backlog_kept::Locked::take(&queue_file(state))?;
        let running = read_queue(state).items.len();
        if running > 0 {
            return Notice::new(Tone::NotNow, format!("Already writing them out: {running} wait")).next(format!("The lead gets {BATCH} at a time, the next once it has written those.")).ok();
        }
        let items = unwritten(entries, dir);
        if items.is_empty() {
            return Notice::new(Tone::NotNow, "Nothing to write out").next("No item in the backlog is marked to write.").ok();
        }
        write_queue(state, &Queue { items, sent: Vec::new() })?;
    }
    let count = queued(state);
    pump_in(state, entries, tell)?;
    Notice::new(Tone::Moving, format!("Writing out {count} item{}", if count == 1 { "" } else { "s" })).next(format!("The lead gets {BATCH} at a time, the next once it has written those; the Backlog page shows how many wait.")).ok()
}

// One round of the queue: an item written out or gone leaves it, and the lead gets as many new ones as it has written of those it was sent.
pub fn pump_in(state: &Path, entries: &[Entry], tell: &dyn Fn(&str) -> Result<()>) -> Result<()> {
    if !queue_file(state).exists() {
        return Ok(());
    }
    let _locked = crate::backlog_kept::Locked::take(&queue_file(state))?;
    let mut queue = read_queue(state);
    let before = queue.items.len();
    queue.items.retain(|item| entries.iter().find(|entry| entry.reference == item.reference).is_some_and(|entry| matches!(entry.shape.state, Some(State::ToWrite) | None)));
    queue.sent.retain(|sent| queue.items.iter().any(|item| &item.reference == sent));
    let unsent: Vec<Unwritten> = queue.items.iter().filter(|item| !queue.sent.contains(&item.reference)).cloned().collect();
    let room = BATCH.saturating_sub(queue.sent.len()).min(unsent.len());
    let mut changed = queue.items.len() != before;
    if room > 0 && tell(&batch_text(&unsent[..room], unsent.len() - room)).is_ok() {
        queue.sent.extend(unsent[..room].iter().map(|item| item.reference.clone()));
        changed = true;
    }
    if changed { write_queue(state, &queue) } else { Ok(()) }
}

// The board's round and the keeper's vault round each move the queue on.
pub fn pump_queue(entries: &[Entry]) {
    if let Ok(state) = crate::lead::state_location() {
        let _ = pump_in(&state, entries, &|text| crate::messages::to_lead(crate::messages::ToLead::BacklogItem, text));
    }
}

pub fn pump_queue_now() {
    if crate::lead::state_location().is_ok_and(|state| queue_file(&state).exists()) {
        pump_queue(&crate::backlog::load().entries);
    }
}

pub fn write_all_cli() -> Result<String> {
    let state = crate::lead::state_dir()?;
    write_all_in(&state, &dir(), &crate::backlog::load().entries, &|text| crate::messages::to_lead(crate::messages::ToLead::BacklogItem, text))
}

// The user asked for the item to be written out, whatever its state; the lead hears it at once.
pub fn write_out_text(who: &str, title: &str, place: &str, words: Option<&str>) -> String {
    let words = words.map(str::trim).filter(|words| !words.is_empty()).map_or(String::new(), |words| format!(" In their words when they added it:\n\n{}\n\n", quoted_block(words)));
    format!("{who} asks you to write out the backlog item \"{title}\" ({place}) with the skill `{SKILL}`, even where it looks complete:{words} read it, the code and the notes again, write it out in place, and show the user the written item.")
}

fn lead_text(filing: &Filing, priority: &str, place: &str) -> String {
    let (who, line) = (filing.by, filing.line);
    let priority = if priority == "none" { String::new() } else { format!(" [{priority}]") };
    if let Some(words) = filing.words {
        return format!("{who} added an idea to the backlog{priority}, in their words:\n\n{}\n\nIts title \"{}\" is a placeholder. Write it out now with the skill `{SKILL}` from what you know (your conversation with the user, the code, the notes), give it a short title in the user's words, show the user the written item, and save it {place}. If it reached you only now, say that it waited for you.", quoted_block(words), line.trim());
    }
    format!("{who} added a quick item to the backlog: \"{}\"{priority}. Write it out now with the skill `{SKILL}` from what you know (your conversation with the user, the code, the notes), show the user the written item, and save it {place}. If it reached you only now, say that it waited for you.", line.trim())
}

// Where an item is filed and how the lead hears of it: the backlog folder, kebbie's state and the lead's inbox.
pub struct Shelf<'a> {
    pub dir: &'a Path,
    pub state: &'a Path,
    pub tell: &'a dyn Fn(&str) -> Result<()>,
}

// Files one item where new items go: an item file without the vault, the lead's to file with it. A quick one-liner goes to the lead to write out.
pub fn file_item(sources: &[Source], filing: &Filing, from_lead: bool) -> Result<String> {
    let (dir, state) = (dir(), crate::lead::state_dir()?);
    file_item_on(&Shelf { dir: &dir, state: &state, tell: &|text| crate::messages::to_lead(crate::messages::ToLead::BacklogItem, text) }, sources, filing, from_lead)
}

pub fn file_item_on(shelf: &Shelf, sources: &[Source], filing: &Filing, from_lead: bool) -> Result<String> {
    let filed = crate::issues::today(crate::messages::now());
    match crate::config::filing(sources) {
        Some(Source::Local) => {
            let (path, item) = add_in(shelf.dir, filing, &filed)?;
            let mut shown = Notice::new(Tone::Done, "Added to the backlog").detail("item", format!("\"{}\" [{}]", item.title, item.priority)).detail("ref", item.reference()).detail("file", path.display().to_string());
            if item.to_write() {
                if from_lead {
                    mark_told(shelf.state, shelf.dir, std::slice::from_ref(&item.stem));
                    shown = shown.next(format!("Write it out now with the skill {SKILL}, in that file."));
                } else {
                    if (shelf.tell)(&lead_text(filing, &item.priority, &format!("in its file {}", path.display()))).is_ok() {
                        mark_told(shelf.state, shelf.dir, std::slice::from_ref(&item.stem));
                    }
                    shown = shown.next("The lead writes it out; it waits for the lead when none runs.");
                }
            }
            shown.ok()
        }
        Some(_) => {
            let item = item_from(filing, &filed)?;
            if from_lead {
                return Notice::new(Tone::You, "Not added: new items go to the vault here").you(format!("Write it as a task note in the vault with the skill {SKILL}.")).err();
            }
            let id = crate::backlog_kept::keep(shelf.state, filing.line, filing.words.unwrap_or_default(), &item.priority, &item.repo, crate::messages::now())?;
            let filed = format!("Then run `kebbie backlog filed {}{id} <the note's path>`, so the row kebbie keeps for it on the board gives way to your note; when you filed it into an existing note, the row stays a week pointing at that note.", crate::backlog_kept::PREFIX);
            let text = if item.to_write() {
                format!("{} {filed}", lead_text(filing, &item.priority, "as a task note in the vault's backlog"))
            } else {
                format!("{} filed a written-out backlog item; file it as a task note in the vault's backlog as it stands (title \"{}\", priority {}, repo {}). {filed}\n\n{}", filing.by, item.title, item.priority, if item.repo.is_empty() { "not given" } else { &item.repo }, item.body)
            };
            if let Err(error) = (shelf.tell)(&text) {
                let _ = crate::backlog_kept::release(shelf.state, &id);
                return Err(error);
            }
            Notice::new(Tone::Moving, "Sent to the lead").next("It writes the item out and files it in the vault. Its row waits here until the note replaces it.").ok()
        }
        None => Notice::new(Tone::You, "Not added: no backlog takes new items").you(format!("List \"local\" or \"vault\" under [board.backlog] sources in {}.", crate::config::path().display())).err(),
    }
}

// The lead filed a kept item: in a new note its row gives way to it; merged into a note filed before the row, the row stays MERGED_DAYS as "<words> · merged into <note>", since that note is not among the newest.
pub fn filed(id: &str, note: Option<&str>) -> Result<String> {
    let state = crate::lead::state_dir()?;
    let index = crate::vault::Index::open().ok();
    filed_in(&state, id, note, crate::messages::now(), |key| index.as_ref().and_then(|index| Some((index.title_of(key).unwrap_or_default(), index.date_of(key)?))))
}

pub fn filed_in(state: &Path, id: &str, note: Option<&str>, now: u64, note_of: impl Fn(&str) -> Option<(String, String)>) -> Result<String> {
    let note = note.map(|note| note.trim().trim_start_matches("vault:").trim()).filter(|note| !note.is_empty());
    let row = crate::backlog_kept::kept_in(state).into_iter().find(|kept| kept.id == id.strip_prefix(crate::backlog_kept::PREFIX).unwrap_or(id));
    let older = |filed_on: &str| row.as_ref().zip(crate::backlog::epoch_day(filed_on)).is_some_and(|(row, day)| day < (row.at / 86_400) as i64);
    let merged = note.and_then(|note| note_of(note).map(|found| (note, found))).filter(|(_, (_, date))| older(date));
    let done = match &merged {
        Some((note, (title, _))) => crate::backlog_kept::merge(state, id, note, title, now)?,
        None => crate::backlog_kept::release(state, id)?,
    };
    match (done, merged) {
        (true, Some((note, (title, _)))) => Notice::new(Tone::Done, "Its row stays a week, pointing at the note it was merged into").detail("kept", id).detail("note", format!("\"{title}\" ({note})")).ok(),
        (true, None) => Notice::new(Tone::Done, "Its row gives way to the note").detail("kept", id).ok(),
        (false, _) => Notice::new(Tone::NotNow, "kebbie keeps no row by that id").next("It was released already, or the id is mistyped.").detail("kept", id).ok(),
    }
}

// Any item the user picks, sent to the lead to write out as it stands now: a merged row's note, a kept row with the user's words; a Linear issue is written in Linear.
pub fn write_out(entry: &Entry, who: &str, dir: &Path, tell: &dyn Fn(&str) -> Result<()>) -> Result<String> {
    let text = match entry.source.as_str() {
        "local" => {
            let stem = entry.reference.strip_prefix(PREFIX).unwrap_or(&entry.reference);
            write_out_text(who, &entry.title, &file(dir, stem).display().to_string(), None)
        }
        "vault" => write_out_text(who, &entry.title, &format!("vault: {}", entry.reference), None),
        "kept" if !entry.shape.into.is_empty() => write_out_text(who, &entry.title, &format!("vault: {}", entry.shape.into), None),
        "kept" => format!("{} File it as a task note in the vault's backlog, then run `kebbie backlog filed {} <the note's path>`.", write_out_text(who, &entry.title, "a row kebbie keeps until it is filed", Some(&entry.summary)), entry.reference),
        _ => return Notice::new(Tone::You, "Not sent: a Linear issue is written in Linear").you("Edit it there.").err(),
    };
    tell(&text)?;
    Notice::new(Tone::Moving, "Sent to the lead").next("It writes the item out in place; the Backlog page shows it once it is saved.").detail("item", entry.title.clone()).ok()
}

// The backlog item by the reference `kebbie backlog` lists, a vault path with or without vault:, or a local item's file name.
pub fn entry_named<'a>(entries: &'a [Entry], named: &str) -> Option<&'a Entry> {
    let named = named.trim();
    let bare = named.strip_prefix("vault:").unwrap_or(named);
    entries.iter().find(|entry| entry.reference == named || entry.reference == bare)
        .or_else(|| entries.iter().find(|entry| entry.source == "local" && entry.reference == reference(bare.trim_end_matches(".md"))))
}

pub fn write_out_cli(named: &str) -> Result<String> {
    if named.trim().is_empty() {
        return Notice::new(Tone::You, "Not sent: name the item").you("kebbie backlog write <ref>, with the reference kebbie backlog lists.").err();
    }
    let backlog = crate::backlog::load();
    let Some(entry) = entry_named(&backlog.entries, named) else {
        return Notice::new(Tone::You, format!("Not sent: no backlog item \"{named}\"")).you("kebbie backlog lists each item with its reference.").err();
    };
    write_out(entry, &caller_name(), &dir(), &|text| crate::messages::to_lead(crate::messages::ToLead::BacklogItem, text))
}

fn caller_name() -> String {
    match crate::permit::caller() {
        crate::permit::Caller::User => "The user".to_string(),
        crate::permit::Caller::Lead | crate::permit::Caller::LeadSandboxed => "The lead".to_string(),
        crate::permit::Caller::Task(task) => format!("The task {task}"),
        _ => "An agent".to_string(),
    }
}

pub fn add(priority: Option<&str>, repo: Option<&str>, words: &[String], from_stdin: bool) -> Result<String> {
    let line = words.join(" ");
    let body = from_stdin.then(|| crate::issues::read_body(&[], true));
    let caller = crate::permit::caller();
    let by = caller_name();
    let from_lead = matches!(caller, crate::permit::Caller::Lead | crate::permit::Caller::LeadSandboxed);
    file_item(&crate::config::sources()?, &Filing { priority: priority.unwrap_or_default(), repo: repo.unwrap_or_default(), line: &line, words: None, body: body.as_deref(), by: &by }, from_lead)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-backlog-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn quick<'a>(line: &'a str, priority: &'a str) -> Filing<'a> {
        Filing { priority, repo: "", line, words: None, body: None, by: "The user" }
    }

    fn written() -> String {
        Template::builtin().sections.iter().map(|section| format!("## {}\n\nSome {}.\n", section.heading, section.heading)).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_quick_line_becomes_a_draft_with_every_section_and_the_line_as_its_source() {
        let item = item_from(&quick("Fix login — the session drops on reload (app)", "High"), "2026-11-12").unwrap();
        assert_eq!((item.title.as_str(), item.priority.as_str(), item.repo.as_str(), item.to_write()), ("Fix login", "high", "app", true));
        let template = Template::builtin();
        assert!(template.sections.iter().all(|section| template.has(&item.body, &section.key)), "{}", item.body);
        assert_eq!(template.missing(&item.body), ["## Goal"], "a draft's goal waits for the lead");
        assert!(template.text(&item.body, "background").contains("To write: the lead writes this item out") && template.text(&item.body, "source").contains("The user, 2026-11-12: \"Fix login — the session drops on reload (app)\""), "{}", item.body);
        let entry = item.entry(crate::backlog::epoch_day("2026-11-14").unwrap());
        assert_eq!((entry.summary.as_str(), entry.shape.state), ("", Some(State::ToWrite)), "a draft has no goal to show yet");
        assert_eq!(item.entry(crate::backlog::epoch_day("2026-11-14").unwrap()).age, "2 d");
        let repo = item_from(&Filing { repo: "web", ..quick("Fix it (app)", "") }, "2026-11-12").unwrap();
        assert_eq!((repo.repo.as_str(), repo.priority.as_str()), ("web", "none"), "--repo wins over the line's own");
    }

    #[test]
    fn a_description_files_under_its_first_words_and_keeps_the_words_as_its_source() {
        assert_eq!(placeholder_title("Dark mode on the board"), "Dark mode on the board", "a short one stands as it is");
        assert_eq!(placeholder_title("  Let the board\nsort by repo, then by age, newest first "), "Let the board sort by repo…");
        assert_eq!(placeholder_title("Rerun the flaky red test once, by itself"), "Rerun the flaky red test once…", "no comma before the ellipsis");
        let words = "Let the board\nsort by repo, then by age.";
        let item = item_from(&Filing { words: Some(words), ..quick(&placeholder_title(words), "") }, "2026-11-12").unwrap();
        assert_eq!((item.title.as_str(), item.to_write()), ("Let the board sort by repo…", true));
        assert_eq!(Template::builtin().text(&item.body, "source"), "The user, 2026-11-12, in their words:\n\n> Let the board\n> sort by repo, then by age.");
    }

    #[test]
    fn a_written_body_needs_every_section_and_files_as_backlog() {
        let body = written();
        let item = item_from(&Filing { body: Some(&body), ..quick("Fix login", "low") }, "2026-11-12").unwrap();
        assert!(!item.to_write() && item.status == "backlog");
        assert_eq!(item.entry(0).summary, "Some Goal.", "the board shows the goal");
        let short = item_from(&Filing { body: Some("## Background\n\nIt broke.\n"), ..quick("Fix login", "") }, "2026-11-12").unwrap_err();
        assert!(short.contains("not written out") && short.contains("## Goal") && short.contains("## Source") && !short.contains("## Background,"), "{short}");
        assert!(item_from(&quick("Fix", "soon"), "2026-11-12").unwrap_err().contains("\"soon\" is not a priority"));
        assert!(item_from(&quick("  ", ""), "2026-11-12").unwrap_err().contains("needs a title"));
    }

    #[test]
    fn a_configured_template_shapes_the_draft_and_what_a_written_item_needs() {
        let template = Template::parse(r#"{"sections":[{"key":"problem","heading":"Problem","required":true},{"key":"risks","heading":"Risks"}],"questions":"risks"}"#).unwrap();
        let draft = item_from_with(&quick("Fix login", ""), "2026-11-12", &template).unwrap();
        assert_eq!(draft.body, "## Problem\n\nTo write: the lead writes this item out with the skill kebbie:write-backlog-item.\n\nThe user, 2026-11-12: \"Fix login\"\n\n## Risks\n\n", "the line is kept in the first section when the template has no Source");
        let short = item_from_with(&Filing { body: Some("## Problem\n\n## Risks\n\n- [ ] Which?\n"), ..quick("Fix login", "") }, "2026-11-12", &template).unwrap_err();
        assert!(short.contains("## Problem") && !short.contains("## Risks"), "an empty required section: {short}");
        let body = "## Problem\n\nIt broke.\n\n## Risks\n\n- [ ] Which?\n";
        assert!(item_from_with(&Filing { body: Some(body), ..quick("Fix login", "") }, "2026-11-12", &template).is_ok());
        assert_eq!(state_of(false, false, body, &template), State::Drafting, "a blocking question under the template's questions section");
        assert_eq!(state_of(false, false, body, &Template::builtin()), State::ToWrite, "the same body misses kebbie's own sections");
    }

    #[test]
    fn an_item_file_reads_back_as_written() {
        let item = Item { stem: "fix-login".to_string(), title: "Fix \"login\"".to_string(), priority: "high".to_string(), repo: "app".to_string(), status: "backlog".to_string(), filed: "2026-11-12".to_string(), body: written().trim().to_string() };
        let text = item.text();
        assert!(text.starts_with("+++\ntitle = \"Fix \\\"login\\\"\"\npriority = \"high\"\nrepo = \"app\"\nstatus = \"backlog\"\nfiled = \"2026-11-12\"\n+++\n\n## Background"), "{text}");
        assert_eq!(parse("fix-login", &text), Some(item));
        assert_eq!(parse("x", "+++\ntitle = \"X\"\npriority = \"soon\"\n+++\n").map(|item| (item.priority, item.status)), Some(("none".to_string(), "backlog".to_string())));
        assert!(parse("x", "+++\npriority = \"high\"\n+++\n").is_none());
    }

    #[test]
    fn a_file_written_by_hand_is_an_item_to_write() {
        let terse = parse("fix-login", "# Fix login\n\nthe session drops\n").unwrap();
        assert_eq!((terse.title.as_str(), terse.state()), ("Fix login", State::ToWrite));
        assert_eq!(parse("fix-the-login", "\n").map(|item| item.title), Some("fix the login".to_string()), "an empty file is named by its file");
        let sections_missing = parse("x", "+++\ntitle = \"X\"\nstatus = \"backlog\"\n+++\n\n## Background\n\nIt broke.\n").unwrap();
        assert_eq!(sections_missing.state(), State::ToWrite, "an item edited down to fewer sections is written out again");
    }

    #[test]
    fn an_open_question_that_blocks_keeps_the_item_drafting() {
        let with = |questions: &str, status: &str| Item { stem: "x".to_string(), title: "X".to_string(), priority: "high".to_string(), repo: "app".to_string(), status: status.to_string(), filed: String::new(), body: written().replace("Some Open questions.", questions) };
        let open = with("- [ ] Should the old links keep working?\n- Which test covers it, settled in the work\n- [x] Which repo: app", "backlog");
        assert_eq!(Template::builtin().questions_in(&open.body), [Question { text: "Should the old links keep working?".to_string(), blocks: true }, Question { text: "Which test covers it, settled in the work".to_string(), blocks: false }]);
        assert_eq!(open.state(), State::Drafting);
        assert_eq!((open.entry(0).shape.waiting, open.entry(0).summary.as_str()), (1, "Some Goal."));
        assert_eq!(with("- Which test covers it\n", "backlog").state(), State::Ready, "a question the work settles does not hold it");
        assert_eq!(with("None.", "drafting").state(), State::Drafting, "the lead marks it drafting while it researches");
        assert_eq!(with("None.", "drafting").entry(0).shape, Shape { state: Some(State::Drafting), ..Shape::default() });
        let drafting_by_hand = Item { status: "drafting".to_string(), body: "terse".to_string(), ..with("None.", "backlog") };
        assert_eq!(drafting_by_hand.state(), State::Drafting, "the lead on it says drafting even before the sections are there");
    }

    #[test]
    fn the_lead_hears_once_of_each_item_to_write() {
        let draft = parse("by-hand", "Fix the login\n").unwrap();
        let ready = Item { stem: "ready".to_string(), title: "Ready".to_string(), priority: "low".to_string(), repo: "app".to_string(), status: "backlog".to_string(), filed: String::new(), body: written() };
        assert_eq!(untold(&[draft.clone(), ready.clone()], &[]), std::slice::from_ref(&draft));
        let (state, dir) = (crate::sh::test_dir("backlog-told-state"), crate::sh::test_dir("backlog-told-dir"));
        std::fs::write(dir.join("kept.md"), "").unwrap();
        mark_told(&state, &dir, &["kept".to_string(), "gone".to_string()]);
        mark_told(&state, &dir, &["kept".to_string()]);
        assert_eq!(told(&state), ["kept"], "a stem whose file is gone leaves the list, and none is listed twice");
        assert!(untold(&[draft, ready], &["by-hand".to_string()]).is_empty());
    }

    fn waiting(source: &str, reference: &str, state: State) -> Entry {
        Entry { source: source.to_string(), title: format!("Item {reference}"), reference: reference.to_string(), shape: Shape { state: Some(state), ..Shape::default() }, ..Entry::default() }
    }

    #[test]
    fn a_vault_note_to_write_reaches_the_lead_only_when_the_user_writes_out_all() {
        let (state, dir) = (crate::sh::test_dir("backlog-queue-none-state"), crate::sh::test_dir("backlog-queue-none-dir"));
        let notes = [waiting("vault", "Projects/old.md", State::ToWrite)];
        let tell = |_: &str| -> Result<()> { panic!("nothing goes to the lead without the user's action") };
        pump_in(&state, &notes, &tell).unwrap();
        assert_eq!(queued(&state), 0);
        let _ = (std::fs::remove_dir_all(state), std::fs::remove_dir_all(dir));
    }

    #[test]
    fn writing_out_all_sends_the_lead_a_batch_at_a_time_until_the_queue_is_empty() {
        let (state, dir) = (crate::sh::test_dir("backlog-queue-state"), crate::sh::test_dir("backlog-queue-dir"));
        let mut notes: Vec<Entry> = (0..12).map(|n| waiting("vault", &format!("Projects/old-{n:02}.md"), State::ToWrite)).collect();
        notes.push(waiting("vault", "Projects/written.md", State::Ready));
        notes.push(Entry { shape: Shape { state: Some(State::ToWrite), kept: true, ..Shape::default() }, ..waiting("kept", "kept:abc123", State::ToWrite) });
        let heard = std::cell::RefCell::new(Vec::new());
        let tell = |text: &str| -> Result<()> { heard.borrow_mut().push(text.to_string()); Ok(()) };
        assert!(write_all_in(&state, &dir, &notes, &tell).unwrap().contains("Writing out 12 items"));
        assert!(heard.borrow()[0].starts_with("The user asks you to write out every backlog item marked to write. Here are 5: \"Item Projects/old-00.md\" (vault: Projects/old-00.md);") && heard.borrow()[0].ends_with(" 7 more wait; kebbie sends the next 5 once these are written."), "{:?}", heard.borrow());
        assert!(write_all_in(&state, &dir, &notes, &tell).unwrap().contains("Already writing them out: 12 wait"), "a second press starts no second queue");
        pump_in(&state, &notes, &tell).unwrap();
        assert_eq!(heard.borrow().len(), 1, "nothing more while the five sent wait to be written");
        notes[0].shape.state = Some(State::Ready);
        notes[1].shape.state = Some(State::Ready);
        pump_in(&state, &notes, &tell).unwrap();
        assert!(heard.borrow()[1].contains("Here are 2: \"Item Projects/old-05.md\"") && heard.borrow()[1].contains("old-06.md") && heard.borrow()[1].ends_with(" 5 more wait; kebbie sends the next 5 once these are written."), "{:?}", heard.borrow()[1]);
        assert_eq!(queued(&state), 10, "the page shows how many wait");
        for note in notes.iter_mut().filter(|note| note.source == "vault") {
            note.shape.state = Some(State::Ready);
        }
        pump_in(&state, &notes, &tell).unwrap();
        assert_eq!((queued(&state), heard.borrow().len()), (0, 2), "every item written out empties the queue");
        assert!(!queue_file(&state).exists());
        let _ = (std::fs::remove_dir_all(state), std::fs::remove_dir_all(dir));
    }

    #[test]
    fn the_queue_survives_a_restart_and_keeps_a_note_whose_text_is_not_read_yet() {
        let (state, dir) = (crate::sh::test_dir("backlog-queue-restart-state"), crate::sh::test_dir("backlog-queue-restart-dir"));
        let notes: Vec<Entry> = (0..7).map(|n| waiting("vault", &format!("Projects/n{n}.md"), State::ToWrite)).collect();
        write_all_in(&state, &dir, &notes, &|_| Ok(())).unwrap();
        assert_eq!(read_queue(&state).sent.len(), 5, "read back from the file, as after a restart");
        let mut unread = notes.clone();
        unread[5].shape.state = None;
        unread.truncate(6);
        pump_in(&state, &unread, &|_| Ok(())).unwrap();
        assert_eq!(read_queue(&state).items.iter().map(|item| item.reference.as_str()).collect::<Vec<_>>(), ["Projects/n0.md", "Projects/n1.md", "Projects/n2.md", "Projects/n3.md", "Projects/n4.md", "Projects/n5.md"], "a note gone from the backlog leaves; one not read yet stays");
        let _ = (std::fs::remove_dir_all(state), std::fs::remove_dir_all(dir));
    }

    #[test]
    fn writing_out_any_item_sends_the_lead_that_item_even_a_written_one() {
        let dir = crate::sh::test_dir("backlog-write-out");
        let heard = std::cell::RefCell::new(Vec::new());
        let tell = |text: &str| -> Result<()> { heard.borrow_mut().push(text.to_string()); Ok(()) };
        let ready = waiting("vault", "Projects/retry.md", State::Ready);
        let said = write_out(&ready, "The user", &dir, &tell).unwrap();
        assert!(said.contains("Sent to the lead"), "{said}");
        assert_eq!(heard.borrow()[0], write_out_text("The user", "Item Projects/retry.md", "vault: Projects/retry.md", None), "a ready note goes all the same");
        write_out(&waiting("local", "local:fix-login", State::Ready), "The user", &dir, &tell).unwrap();
        assert!(heard.borrow()[1].contains(&format!("({})", dir.join("fix-login.md").display())));
        let merged = Entry { summary: "rerun once".to_string(), shape: Shape { state: Some(State::Ready), kept: true, into: "Projects/older.md".to_string(), ..Shape::default() }, ..waiting("kept", "kept:abc123", State::Ready) };
        write_out(&merged, "The user", &dir, &tell).unwrap();
        assert!(heard.borrow()[2].contains("(vault: Projects/older.md)"), "a merged row sends the note it went into");
        let kept = Entry { summary: "rerun once".to_string(), shape: Shape { state: Some(State::ToWrite), kept: true, ..Shape::default() }, ..waiting("kept", "kept:abc123", State::ToWrite) };
        write_out(&kept, "The user", &dir, &tell).unwrap();
        assert!(heard.borrow()[3].contains("> rerun once") && heard.borrow()[3].ends_with("then run `kebbie backlog filed kept:abc123 <the note's path>`."), "{}", heard.borrow()[3]);
        assert!(write_out(&waiting("linear", "ABC-1", State::Ready), "The user", &dir, &tell).unwrap_err().contains("written in Linear"));
        assert_eq!(heard.borrow().len(), 4);
        let entries = [ready, waiting("local", "local:fix-login", State::Ready)];
        assert_eq!(entry_named(&entries, "vault:Projects/retry.md").map(|entry| entry.reference.as_str()), Some("Projects/retry.md"));
        assert_eq!(entry_named(&entries, "fix-login.md").map(|entry| entry.reference.as_str()), Some("local:fix-login"));
        assert!(entry_named(&entries, "Projects/none.md").is_none());
    }

    #[test]
    fn a_kept_row_filed_into_an_older_note_stays_and_one_filed_into_a_new_note_gives_way() {
        let state = crate::sh::test_dir("backlog-filed");
        let at = 1_794_744_000;
        let notes = |key: &str| match key {
            "Projects/older.md" => Some(("kebbie-ios: native iOS app".to_string(), "2026-10-21".to_string())),
            "Projects/new.md" => Some(("New note".to_string(), "2026-11-15".to_string())),
            _ => None,
        };
        let merged = crate::backlog_kept::keep(&state, "Native app", "Native app for the Duo", "high", "", at).unwrap();
        let fresh = crate::backlog_kept::keep(&state, "Other idea", "", "low", "", at).unwrap();
        assert!(filed_in(&state, &format!("kept:{merged}"), Some("vault:Projects/older.md"), at + 60, notes).unwrap().contains("merged into"));
        assert!(filed_in(&state, &format!("kept:{fresh}"), Some("Projects/new.md"), at + 60, notes).unwrap().contains("gives way"));
        let rows = crate::backlog_kept::shown_in(&state, at + 60);
        assert_eq!(rows.iter().map(|row| (row.id.as_str(), row.merged.as_ref().map(|merged| merged.into.as_str()))).collect::<Vec<_>>(), [(merged.as_str(), Some("Projects/older.md"))]);
        assert!(filed_in(&state, &format!("kept:{merged}"), None, at + 120, notes).unwrap().contains("gives way"), "without a note the row gives way as before");
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn a_write_out_the_user_asks_for_names_the_item_and_their_words() {
        assert_eq!(write_out_text("The user", "Retry", "vault: Projects/retry.md", None), "The user asks you to write out the backlog item \"Retry\" (vault: Projects/retry.md) with the skill `kebbie:write-backlog-item`, even where it looks complete: read it, the code and the notes again, write it out in place, and show the user the written item.");
        assert!(write_out_text("The user", "Retry…", "kept by kebbie", Some("rerun\nonce")).contains("In their words when they added it:\n\n> rerun\n> once\n\n read it"));
    }

    #[test]
    fn a_stem_is_the_title_in_lowercase_words_and_stays_unique() {
        assert_eq!(stem_for("Fix login: the session (app)", |_| false), "fix-login-the-session-app");
        assert_eq!(stem_for("Åtgärda något", |_| false), "åtgärda-något");
        assert_eq!(stem_for("!!!", |_| false), "item");
        assert_eq!(stem_for("Fix", |stem| stem == "fix" || stem == "fix-2"), "fix-3");
    }

    #[test]
    fn adding_writes_one_file_per_item_and_refuses_a_title_already_there() {
        let dir = temp("add");
        let (path, item) = add_in(&dir, &quick("Ship it (app)", "high"), "2026-11-12").unwrap();
        assert_eq!(path, dir.join("ship-it.md"));
        assert_eq!(item.reference(), "local:ship-it");
        add_in(&dir, &quick("Then this", ""), "2026-11-12").unwrap();
        std::fs::write(dir.join(".ship-it.md.1-0.tmp"), "+++\ntitle = \"temp\"\n+++\n").unwrap();
        let titles: Vec<String> = items(&dir).into_iter().map(|item| item.title).collect();
        assert_eq!(titles, ["Ship it", "Then this"]);
        assert!(add_in(&dir, &quick("Ship it", ""), "2026-11-12").unwrap_err().contains("already in the backlog"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_old_one_file_backlog_moves_into_draft_items_once_and_stays_as_a_backup() {
        let root = temp("migrate");
        let dir = root.join("backlog");
        std::fs::create_dir_all(&root).unwrap();
        let old = "# My backlog\n\n- [high] Fix login — the session drops on reload (app)\n- Write the docs\n- [x] done\n  - nested note\n";
        std::fs::write(root.join("backlog.md"), old).unwrap();
        assert_eq!(migrate(&dir, "2026-11-12").unwrap(), 2);
        let moved = items(&dir);
        assert_eq!(moved.iter().map(|item| (item.title.as_str(), item.priority.as_str(), item.repo.as_str(), item.to_write())).collect::<Vec<_>>(), [("Fix login", "high", "app", true), ("Write the docs", "none", "", true)]);
        assert!(Template::builtin().text(&moved[0].body, "source").contains("moved from backlog.md, 2026-11-12: \"Fix login — the session drops on reload\""), "{}", moved[0].body);
        assert!(!root.join("backlog.md").exists() && std::fs::read_to_string(root.join("backlog.md.bak")).unwrap() == old, "the old file is kept, renamed");
        assert_eq!(migrate(&dir, "2026-11-12").unwrap(), 0, "once");
        std::fs::write(root.join("backlog.md"), "- Fix login\n- New one\n").unwrap();
        assert_eq!(migrate(&dir, "2026-11-12").unwrap(), 1, "a title already moved is not moved twice");
        assert!(root.join("backlog.md.bak.1").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_old_line_reads_its_priority_title_summary_and_area() {
        assert_eq!(old_line("- [high] Fix login — the session drops (app)"), Some(("high".to_string(), "Fix login".to_string(), "the session drops".to_string(), "app".to_string())));
        assert_eq!(old_line("- Fix it (the old way)").map(|parts| (parts.1, parts.3)), Some(("Fix it (the old way)".to_string(), String::new())), "a parenthesis with a space is text");
        assert_eq!(old_line("- One — two — three (web)").map(|parts| parts.2), Some("two — three".to_string()));
        for not_an_item in ["# Backlog", "", "Some prose.", "-", "- [high]", "<!-- note -->", "- [x] done", "- [ ] open", "- [later] Maybe", "  - nested", "- [unclosed"] {
            assert!(old_line(not_an_item).is_none(), "{not_an_item:?}");
        }
    }
}
