use std::path::{Path, PathBuf};

use crate::backlog::{Entry, PRIORITIES};
use crate::config::Source;
use crate::notice::{Notice, Tone};
use crate::sh::Result;

pub const PREFIX: &str = "local:";
pub const SECTIONS: [&str; 6] = ["Background", "Goal", "What we know", "Open questions", "Out of scope", "Source"];
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

#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub text: String,
    pub blocks: bool,
}

// Blocking questions waiting on the user, answers of theirs the lead has yet to fold in, and the state those leave; Linear has none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub state: Option<State>,
    pub waiting: usize,
    pub with_lead: usize,
    pub kept: bool,
    pub filed: String,
}

// `- [ ] ` under Open questions blocks the start until the user answers it; a plain bullet is settled during the work.
pub fn questions_in(body: &str) -> Vec<Question> {
    section(body, "Open questions").lines().filter_map(|line| {
        let line = line.trim();
        let (blocks, text) = match line.strip_prefix("- [ ] ").or_else(|| line.strip_prefix("* [ ] ")) {
            Some(text) => (true, text),
            None => (false, line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))?),
        };
        let text = text.trim();
        (!text.is_empty() && !text.starts_with("[x]") && !text.starts_with("[X]")).then(|| Question { text: text.to_string(), blocks })
    }).collect()
}

// The lead marks an item drafting while it writes it out; an item short of a section is to write; a blocking question keeps it drafting.
pub fn state_of(drafting: bool, short: bool, body: &str) -> State {
    if drafting {
        State::Drafting
    } else if short || !missing_sections(body).is_empty() {
        State::ToWrite
    } else if questions_in(body).iter().any(|question| question.blocks) {
        State::Drafting
    } else {
        State::Ready
    }
}

pub fn shape_of(state: State, body: &str, answered: &[String], filed: &str) -> Shape {
    let blocking: Vec<Question> = questions_in(body).into_iter().filter(|question| question.blocks).collect();
    let with_lead = blocking.iter().filter(|question| answered.contains(&question.text)).count();
    Shape { state: Some(state), waiting: blocking.len() - with_lead, with_lead, kept: false, filed: filed.to_string() }
}

pub fn first_line(body: &str) -> String {
    ["Goal", "Background"].iter().find_map(|heading| section(body, heading).lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string)).unwrap_or_default()
}

impl Item {
    pub fn to_write(&self) -> bool {
        self.status == DRAFT || !missing_sections(&self.body).is_empty()
    }

    pub fn state(&self) -> State {
        state_of(self.status == DRAFTING, self.status == DRAFT, &self.body)
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
        Entry {
            source: "local".to_string(),
            title: self.title.clone(),
            priority: self.priority.clone(),
            area: self.repo.clone(),
            reference: self.reference(),
            summary: if self.to_write() { String::new() } else { first_line(&self.body) },
            age: crate::backlog::epoch_day(&self.filed).map_or(String::new(), |day| format!("{} d", (today - day).max(0))),
            shape: shape_of(self.state(), &self.body, &[], &self.filed),
        }
    }
}

pub fn section<'a>(body: &'a str, heading: &str) -> &'a str {
    let marker = format!("## {heading}\n");
    let Some(start) = body.find(&marker).map(|at| at + marker.len()) else { return "" };
    let rest = &body[start..];
    rest.find("\n## ").map_or(rest, |end| &rest[..end])
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

fn draft_body(by: &str, filed: &str, line: &str) -> String {
    let mut body = String::new();
    for heading in SECTIONS {
        let text = match heading {
            "Background" => TO_WRITE.to_string(),
            "Source" => format!("{by}, {filed}: \"{}\"", line.trim()),
            _ => String::new(),
        };
        body.push_str(&format!("## {heading}\n\n{text}{}", if text.is_empty() { "" } else { "\n\n" }));
    }
    body
}

pub fn missing_sections(body: &str) -> Vec<&'static str> {
    SECTIONS.into_iter().filter(|heading| !body.lines().any(|line| line.trim_end() == format!("## {heading}"))).collect()
}

pub struct Filing<'a> {
    pub priority: &'a str,
    pub repo: &'a str,
    pub line: &'a str,
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
    let priority = checked_priority(filing.priority)?;
    let (title, _, area) = split_line(filing.line);
    if title.is_empty() {
        return Notice::new(Tone::You, "Not added: the item needs a title").you("Give it a title.").err();
    }
    let repo = Some(filing.repo.trim()).filter(|repo| !repo.is_empty()).unwrap_or(&area).to_string();
    let (status, body) = match filing.body.map(str::trim).filter(|body| !body.is_empty()) {
        Some(body) => {
            let missing = missing_sections(body);
            if !missing.is_empty() {
                return Notice::new(Tone::You, "Not added: the item is not written out").you(format!("Write it with the skill {SKILL}: it needs {}.", missing.iter().map(|heading| format!("## {heading}")).collect::<Vec<_>>().join(", "))).err();
            }
            ("backlog".to_string(), body.to_string())
        }
        None => (DRAFT.to_string(), draft_body(filing.by, filed, filing.line)),
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
        let item = Item { stem: stem.clone(), title, priority, repo: area, status: DRAFT.to_string(), filed: filed.to_string(), body: draft_body(&format!("moved from {OLD_FILE}"), filed, &line) };
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

fn lead_text(who: &str, line: &str, priority: &str, place: &str) -> String {
    let priority = if priority == "none" { String::new() } else { format!(" [{priority}]") };
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
                    if (shelf.tell)(&lead_text(filing.by, filing.line, &item.priority, &format!("in its file {}", path.display()))).is_ok() {
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
            let id = crate::backlog_kept::keep(shelf.state, filing.line, &item.priority, &item.repo, crate::messages::now())?;
            let filed = format!("Then run `kebbie backlog filed {}{id}`, so the row kebbie keeps for it on the board gives way to your note.", crate::backlog_kept::PREFIX);
            let text = if item.to_write() {
                format!("{} {filed}", lead_text(filing.by, filing.line, &item.priority, "as a task note in the vault's backlog"))
            } else {
                format!("{} filed a written-out backlog item; file it as a task note in the vault's backlog as it stands (title \"{}\", priority {}, repo {}). {filed}\n\n{}", filing.by, item.title, item.priority, if item.repo.is_empty() { "not given" } else { &item.repo }, item.body)
            };
            (shelf.tell)(&text)?;
            Notice::new(Tone::Moving, "Sent to the lead").next("It writes the item out and files it in the vault. Its row waits here until the note replaces it.").ok()
        }
        None => Notice::new(Tone::You, "Not added: no backlog takes new items").you(format!("List \"local\" or \"vault\" under [board.backlog] sources in {}.", crate::config::path().display())).err(),
    }
}

pub fn filed(id: &str) -> Result<String> {
    match crate::backlog_kept::release(&crate::lead::state_dir()?, id)? {
        true => Notice::new(Tone::Done, "Its row gives way to the note").detail("kept", id).ok(),
        false => Notice::new(Tone::NotNow, "kebbie keeps no row by that id").next("It was released already, or the id is mistyped.").detail("kept", id).ok(),
    }
}

pub fn add(priority: Option<&str>, repo: Option<&str>, words: &[String], from_stdin: bool) -> Result<String> {
    let line = words.join(" ");
    let body = from_stdin.then(|| crate::issues::read_body(&[], true));
    let caller = crate::permit::caller();
    let by = match &caller {
        crate::permit::Caller::User => "The user".to_string(),
        crate::permit::Caller::Lead | crate::permit::Caller::LeadSandboxed => "The lead".to_string(),
        crate::permit::Caller::Task(task) => format!("The task {task}"),
        _ => "An agent".to_string(),
    };
    let from_lead = matches!(caller, crate::permit::Caller::Lead | crate::permit::Caller::LeadSandboxed);
    file_item(&crate::config::sources()?, &Filing { priority: priority.unwrap_or_default(), repo: repo.unwrap_or_default(), line: &line, body: body.as_deref(), by: &by }, from_lead)
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
        Filing { priority, repo: "", line, body: None, by: "The user" }
    }

    fn written() -> String {
        SECTIONS.iter().map(|heading| format!("## {heading}\n\nSome {heading}.\n")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_quick_line_becomes_a_draft_with_every_section_and_the_line_as_its_source() {
        let item = item_from(&quick("Fix login — the session drops on reload (app)", "High"), "2026-11-12").unwrap();
        assert_eq!((item.title.as_str(), item.priority.as_str(), item.repo.as_str(), item.to_write()), ("Fix login", "high", "app", true));
        assert!(missing_sections(&item.body).is_empty(), "{}", item.body);
        assert!(section(&item.body, "Background").contains("To write: the lead writes this item out") && section(&item.body, "Source").contains("The user, 2026-11-12: \"Fix login — the session drops on reload (app)\""), "{}", item.body);
        let entry = item.entry(crate::backlog::epoch_day("2026-11-14").unwrap());
        assert_eq!((entry.summary.as_str(), entry.shape.state), ("", Some(State::ToWrite)), "a draft has no goal to show yet");
        assert_eq!(item.entry(crate::backlog::epoch_day("2026-11-14").unwrap()).age, "2 d");
        let repo = item_from(&Filing { repo: "web", ..quick("Fix it (app)", "") }, "2026-11-12").unwrap();
        assert_eq!((repo.repo.as_str(), repo.priority.as_str()), ("web", "none"), "--repo wins over the line's own");
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
        assert_eq!(questions_in(&open.body), [Question { text: "Should the old links keep working?".to_string(), blocks: true }, Question { text: "Which test covers it, settled in the work".to_string(), blocks: false }]);
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
        assert!(section(&moved[0].body, "Source").contains("moved from backlog.md, 2026-11-12: \"Fix login — the session drops on reload\""), "{}", moved[0].body);
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
