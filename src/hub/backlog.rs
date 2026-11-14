use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use kebbie_hub_protocol::{BacklogBody, BacklogRow, BacklogState, BodySection, Home, HomeKind, Id, Idea, ItemAnswer, ItemEdit, ItemKey, MAX_FOUND_SOURCES, OpenQuestion, Outcome, Payload, Priority, Reason, StartItem, text_hash};

use super::blobs::{Attachment, PLAN_LIMIT};
use super::commands::{Done, plain};
use crate::backlog::{Entry, Templates};
use crate::item_template::Template;
use crate::backlog_file::{self, Filing, Shelf, State};
use crate::config::Source;
use crate::permit::Caller;
use crate::sh::Result;

const KEPT_LABEL: &str = "kept by kebbie";

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

// A vault note is keyed by its path inside the vault, so every machine sharing the vault names it alike.
fn vault_path(reference: &str, root: Option<&Path>) -> Option<String> {
    let reference = reference.strip_prefix("vault:").unwrap_or(reference);
    if !reference.starts_with(['/', '~']) {
        return Some(reference.to_string());
    }
    let root = root?;
    let full = crate::vault::expand(reference);
    let inside = full.strip_prefix(root).ok().or_else(|| full.strip_prefix(root.canonicalize().ok()?).ok())?;
    Some(inside.to_string_lossy().to_string())
}

// A local item's file name keys it while it fits the protocol's id; a longer or non-ASCII name is keyed by its hash.
fn local_key(reference: &str) -> Option<ItemKey> {
    ItemKey::parse(reference).ok().or_else(|| {
        let stem = reference.strip_prefix(backlog_file::PREFIX)?;
        ItemKey::parse(&format!("{}{}", backlog_file::PREFIX, &sha1_smol::Sha1::from(stem).digest().to_string()[..16])).ok()
    })
}

fn key_for(source: &str, reference: &str, root: Option<&Path>) -> Option<ItemKey> {
    match source {
        "vault" => ItemKey::vault(&vault_path(reference, root)?).ok(),
        "local" => local_key(reference),
        "kept" => ItemKey::parse(reference).ok(),
        _ => None,
    }
}

pub fn key_of(entry: &Entry, root: Option<&Path>) -> Option<ItemKey> {
    key_for(&entry.source, &entry.reference, root)
}

// A running task names the item it came from by its reference alone.
fn started_as(key: &ItemKey, started: &[String], root: Option<&Path>) -> bool {
    started.iter().any(|reference| {
        let source = if reference.starts_with(backlog_file::PREFIX) { "local" } else if reference.starts_with(crate::backlog_kept::PREFIX) { "kept" } else { "vault" };
        key_for(source, reference, root).as_ref() == Some(key)
    })
}

fn priority(word: &str) -> Priority {
    match word {
        "urgent" | "high" => Priority::High,
        "medium" => Priority::Medium,
        "low" => Priority::Low,
        _ => Priority::None,
    }
}

fn state_of(entry: &Entry) -> BacklogState {
    match entry.shape.state {
        Some(State::ToWrite) => BacklogState::ToWrite,
        Some(State::Drafting) => BacklogState::Drafting,
        Some(State::Ready) | None => BacklogState::Ready,
    }
}

fn filed(entry: &Entry, today: i64) -> u64 {
    let day = crate::backlog::epoch_day(&entry.shape.filed).or_else(|| {
        let days: i64 = entry.age.strip_suffix(" d")?.trim().parse().ok()?;
        Some(today - days)
    });
    day.and_then(|day| u64::try_from(day * 86_400).ok()).unwrap_or(0)
}

fn home(entry: &Entry, key: &ItemKey) -> Home {
    match key.home() {
        HomeKind::Vault => Home { kind: HomeKind::Vault, label: key.as_str().trim_start_matches("vault:").to_string() },
        HomeKind::Local => {
            let label = entry.reference.strip_prefix(backlog_file::PREFIX).map(|stem| format!("backlog/{stem}.md")).filter(|label| kebbie_hub_protocol::is_relative_label(label));
            Home { kind: HomeKind::Local, label: label.unwrap_or_else(|| "backlog".to_string()) }
        }
        HomeKind::Kept => Home { kind: HomeKind::Kept, label: KEPT_LABEL.to_string() },
        HomeKind::Found => Home { kind: HomeKind::Found, label: FOUND_LABEL.to_string() },
    }
}

// The item's questions as the phone shows them, each with the user's answer while it waits with the lead.
pub fn open_questions(body: &str, reference: &str, state: &Path, template: &Template) -> Vec<OpenQuestion> {
    questions_with(body, reference, &crate::backlog_kept::answers_in(state), template)
}

fn questions_with(body: &str, reference: &str, answers: &[crate::backlog_kept::Answer], template: &Template) -> Vec<OpenQuestion> {
    template.questions_in(body).into_iter().map(|question| {
        let pending_answer = answers.iter().find(|answer| answer.item == reference && answer.question == question.text).map(|answer| answer.answer.clone());
        OpenQuestion { text: question.text, blocks: question.blocks, pending_answer }
    }).collect()
}

fn waiting(questions: &[OpenQuestion]) -> usize {
    questions.iter().filter(|question| question.blocks && question.pending_answer.is_none()).count()
}

// The body the phone shows and edits: the template's sections in order, each as its text stands, the open questions apart under the section that holds them.
pub fn body_of_item(body: &str, template: &Template, questions: Vec<OpenQuestion>) -> BacklogBody {
    let sections = template.sections.iter().map(|section| BodySection { key: section.key.clone(), heading: section.heading.clone(), text: template.shown(body, &section.key), required: section.required }).collect();
    BacklogBody { sections, questions, questions_key: template.questions.clone() }
}

// The rows of the board's Backlog column and each written item's body as a blob; Linear rows stay on the machine, having no key the phone can send back.
pub fn rows(items: &[(Entry, Option<String>)], state: &Path, root: Option<&Path>, today: i64, templates: &Templates) -> (Vec<BacklogRow>, Vec<Attachment>) {
    let mut seen = BTreeSet::new();
    let (mut rows, mut bodies) = (Vec::new(), Vec::new());
    let answers = crate::backlog_kept::answers_in(state);
    for (entry, body) in items {
        let Some(item) = key_of(entry, root).filter(|item| seen.insert(item.clone())) else { continue };
        let template = templates.of(&entry.source);
        let body = body.as_deref().filter(|body| !body.trim().is_empty());
        let questions = body.map(|body| questions_with(body, &entry.reference, &answers, template)).unwrap_or_default();
        let goal = body.and_then(|body| template.shown(body, template.goal_key()?).lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string))
            .or_else(|| Some(entry.summary.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|words| entry.shape.kept && !words.is_empty()));
        rows.push(BacklogRow {
            item: item.clone(),
            title: entry.title.clone(),
            priority: priority(&entry.priority),
            repo: Some(entry.area.clone()).filter(|area| !area.is_empty()),
            state: state_of(entry),
            filed: filed(entry, today),
            home: home(entry, &item),
            questions: count(questions.len()),
            blocking: count(waiting(&questions)),
            goal,
            body: None,
            found: None,
        });
        if let Some(body) = body {
            let written = body_of_item(body, template, questions);
            if let Ok(text) = serde_json::to_string(&written).map_err(|error| error.to_string()).and_then(|text| if text.len() <= PLAN_LIMIT { Ok(text) } else { Err(String::new()) }) {
                bodies.push(Attachment::Backlog { item, text });
            }
        }
    }
    (rows, bodies)
}

// A local item's body from its file, a vault note's from the index or kebbie's copy; with where the lead finds it.
fn body_of(entry: &Entry, index: Option<&crate::vault::Index>, state: &Path) -> Option<(String, String)> {
    match entry.source.as_str() {
        "local" => backlog_file::find(&entry.reference).map(|(path, item)| (item.body, path.display().to_string())),
        "vault" => crate::backlog::vault_body(index, state, &entry.reference).map(|body| (body, format!("vault: {}", entry.reference))),
        _ => None,
    }
}

pub fn gather(state: &Path) -> (Vec<BacklogRow>, Vec<Attachment>) {
    let (mut rows, bodies) = rows(&crate::backlog::with_bodies(), state, crate::vault::root().as_deref(), crate::backlog::today(), &Templates::now());
    rows.extend(found_rows(&crate::found::open_in(state)));
    (rows, bodies)
}

const FOUND_LABEL: &str = "found by tasks";

// What tasks found outside their own work, open ones only, each with the tasks that found it; an id the protocol cannot carry stays on the machine.
pub fn found_rows(items: &[crate::found::Item]) -> Vec<BacklogRow> {
    items.iter().filter_map(|found| {
        let item = ItemKey::parse(&format!("found:{}", found.id)).ok()?;
        let sources: Vec<String> = found.sources.iter().take(MAX_FOUND_SOURCES).cloned().collect();
        let count = u32::try_from(sources.len()).unwrap_or(u32::MAX).max(found.count);
        Some(BacklogRow {
            item,
            title: found.line.clone(),
            priority: Priority::None,
            repo: Some(found.repo.clone()).filter(|repo| !repo.is_empty()),
            state: BacklogState::Ready,
            filed: found.first_at,
            home: Home { kind: HomeKind::Found, label: FOUND_LABEL.to_string() },
            questions: 0,
            blocking: 0,
            goal: None,
            body: None,
            found: Some(kebbie_hub_protocol::Found { sources, count }),
        })
    }).collect()
}

// The phone's start, add, keep or dismiss of a found item, as kebbie found does it from the user; the card it came from shows the choice.
pub fn found_on(state: &Path, caller: &Caller, payload: &Payload, start: impl FnOnce(&str, Option<&str>) -> Result<String>, add: impl FnOnce(&str, &str) -> Result<String>) -> Done {
    let item = match payload {
        Payload::FoundStart(found) => &found.item,
        Payload::FoundAdd(found) => &found.item,
        Payload::FoundDismiss(found) => &found.item,
        Payload::FoundKeep(found) => &found.item,
        _ => return refused(Reason::BadRequest, "Not a found item's command"),
    };
    let id = item.as_str().trim_start_matches("found:").to_string();
    if !crate::found::open_in(state).iter().any(|open| open.id == id) {
        return refused(Reason::AlreadySettled, "That item was started, added or dismissed already");
    }
    let outcome = match payload {
        Payload::FoundStart(found) => start(&id, found.name.as_deref()),
        Payload::FoundAdd(found) => add(&id, &found.task),
        Payload::FoundKeep(_) => crate::found::keep_in(state, caller, &id),
        _ => crate::found::dismiss_in(state, caller, &id),
    };
    match outcome {
        Ok(said) => (Outcome::Done, plain(&said)),
        Err(error) => (Outcome::Failed, plain(&error)),
    }
}

pub fn found(caller: &Caller, payload: &Payload) -> Done {
    match crate::lead::state_dir() {
        Ok(state) => found_on(&state, caller, payload, |id, name| crate::found::start(&state, caller, id, name), |id, task| crate::found::add(&state, caller, id, task)),
        Err(error) => (Outcome::Failed, error),
    }
}

// An item the phone named, as this machine's backlog has it now.
#[derive(Clone, Debug)]
pub struct Found {
    pub entry: Entry,
    pub body: String,
    pub wherein: String,
}

impl Found {
    fn questions(&self, state: &Path) -> Vec<OpenQuestion> {
        open_questions(&self.body, &self.entry.reference, state, &crate::item_template::for_source(&self.entry.source))
    }
}

// What the hub's backlog commands read and do on this machine, given as values and functions so tests drive them apart from the user's backlog.
pub struct Desk<'a> {
    pub state: &'a Path,
    pub found: Option<Found>,
    pub started: bool,
    pub tell: &'a dyn Fn(&str) -> Result<()>,
}

fn who(caller: &Caller) -> String {
    match caller {
        Caller::Hub { name, .. } => format!("The user on {name}"),
        _ => "The user".to_string(),
    }
}

fn refused(reason: Reason, text: &str) -> Done {
    (Outcome::Refused { reason }, text.to_string())
}

// The phone's idea is a description filed as kebbie backlog add files a short item: a draft under its first words, or a row kebbie keeps while the lead files it in the vault; the lead titles it as it writes it out.
pub fn idea_on(shelf: &Shelf, sources: &[Source], caller: &Caller, idea: &Idea) -> Done {
    let title = backlog_file::placeholder_title(&idea.text);
    let by = who(caller);
    let filing = Filing { priority: "", repo: idea.repo.as_deref().unwrap_or_default(), line: &title, words: Some(idea.text.trim()), body: None, by: &by };
    match backlog_file::file_item_on(shelf, sources, &filing, false) {
        Ok(text) => (Outcome::Done, plain(&text)),
        Err(text) => (Outcome::Failed, plain(&text)),
    }
}

// The answer waits with the item as one given on its card does, and the lead hears it at once.
pub fn answer_on(desk: &Desk, caller: &Caller, answer: &ItemAnswer) -> Done {
    let Some(found) = desk.found.as_ref() else { return refused(Reason::UnknownItem, "That item is no longer in this machine's backlog") };
    let questions = found.questions(desk.state);
    let Some(question) = usize::try_from(answer.index).ok().and_then(|index| questions.get(index)).filter(|question| question.text == answer.question) else {
        return refused(Reason::QuestionChanged, "That question changed on the item since; read it again and answer what it asks now");
    };
    let given = crate::backlog_kept::Answer { item: found.entry.reference.clone(), question: question.text.clone(), answer: answer.answer.trim().to_string(), at: crate::messages::now() };
    if let Err(error) = crate::backlog_kept::answer(desk.state, given) {
        return (Outcome::Failed, format!("The answer was not kept: {error}"));
    }
    match (desk.tell)(&crate::backlog_kept::answer_text(&who(caller), &found.entry.title, &found.wherein, &question.text, answer.answer.trim())) {
        Ok(()) => (Outcome::Done, "With the lead: it folds your answer into the item".to_string()),
        Err(error) => (Outcome::Failed, format!("The answer is kept with the item, but the lead did not hear of it: {error}")),
    }
}

// Started as g on the board starts it: questions waiting on the user hold it unless the phone says start anyway, and the lead picks the recipe and whether it queues.
pub fn start_on(desk: &Desk, start: &StartItem, run: impl FnOnce(&Found) -> Result<String>) -> Done {
    if desk.started {
        return refused(Reason::AlreadyStarted, "Already started: it runs as a task");
    }
    let Some(found) = desk.found.as_ref() else { return refused(Reason::UnknownItem, "That item is no longer in this machine's backlog") };
    if found.entry.shape.kept {
        return refused(Reason::BadRequest, "Not started: the lead has yet to write it out and file it in the vault");
    }
    let blocking = waiting(&found.questions(desk.state));
    if blocking > 0 && !start.anyway {
        let waits = if blocking == 1 { "1 question waits".to_string() } else { format!("{blocking} questions wait") };
        return (Outcome::Refused { reason: Reason::QuestionsOpen { blocking: count(blocking) } }, format!("Not started: {waits} on you. Answer them on the item, or start it anyway."));
    }
    if found.entry.area.is_empty() {
        return refused(Reason::UnknownRepo, "Not started: kebbie cannot tell which repo it is in. Name the repo in the item's header, or as the area of its note.");
    }
    match run(found) {
        Ok(task) => (Outcome::Done, format!("Starting {task}. It shows under Work in a moment; if it cannot start, a notice says why")),
        Err(error) => (Outcome::Failed, plain(&error)),
    }
}

// What an edit changes once its base holds: the title, or the body with one section's text replaced.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Title(String),
    Body(String),
}

// An edit lands only on the text the phone saw, hashing to its base; a required section is never emptied, and a section the body lacks is added at its end.
pub fn change_for(title: &str, body: &str, template: &Template, edit: &ItemEdit) -> std::result::Result<Change, Done> {
    let changed = || refused(Reason::SectionChanged, "Not saved: it changed on the machine since you opened it; read it again and edit what it says now");
    let Some(key) = edit.section.as_deref() else {
        return if text_hash(title) == edit.base { Ok(Change::Title(edit.text.trim().to_string())) } else { Err(changed()) };
    };
    let Some(section) = template.sections.iter().find(|section| section.key == key) else { return Err(changed()) };
    if text_hash(&template.shown(body, key)) != edit.base {
        return Err(changed());
    }
    if section.required && edit.text.trim().is_empty() {
        return Err(refused(Reason::BadRequest, &format!("Not saved: {} is required, so it cannot be left empty", section.heading)));
    }
    let edited = template.replace(body, key, &edit.text).unwrap_or_else(|| format!("{}\n\n## {}\n\n{}\n", body.trim_end(), section.heading, edit.text.trim()));
    Ok(Change::Body(edited))
}

fn saved(edit: &ItemEdit, template: &Template) -> Done {
    let what = edit.section.as_deref().and_then(|key| template.heading(key)).unwrap_or("the title");
    (Outcome::Done, format!("Saved {what}"))
}

// The item file with the change made and the rest as it stands: its header's other keys, and a hand-written file's own shape for a body edit.
fn edited_file(text: &str, item: &backlog_file::Item, change: Change) -> std::result::Result<String, String> {
    let header = text.strip_prefix("+++\n").and_then(|rest| rest.split_once("\n+++\n").or_else(|| rest.strip_suffix("\n+++").map(|front| (front, ""))));
    match (header, change) {
        (Some((front, _)), Change::Title(title)) => {
            let mut table: toml_edit::DocumentMut = front.parse().map_err(|error: toml_edit::TomlError| error.to_string())?;
            table["title"] = toml_edit::value(title);
            Ok(format!("+++\n{}\n+++\n\n{}\n", table.to_string().trim_end(), item.body.trim()))
        }
        (Some((front, _)), Change::Body(body)) => Ok(format!("+++\n{front}\n+++\n\n{}\n", body.trim())),
        (None, Change::Title(title)) => Ok(backlog_file::Item { title, ..item.clone() }.text()),
        (None, Change::Body(body)) => Ok(format!("{}\n", body.trim())),
    }
}

// A local item is its file: the edit is read against it and written only while the file still holds the text it was read from.
pub fn edit_local(path: &Path, template: &Template, edit: &ItemEdit) -> Done {
    let changed = || refused(Reason::SectionChanged, "Not saved: it changed on the machine since you opened it; read it again and edit what it says now");
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default();
    let Some((text, item)) = std::fs::read_to_string(path).ok().and_then(|text| backlog_file::parse(&stem, &text).map(|item| (text, item))) else {
        return refused(Reason::UnknownItem, "That item is no longer in this machine's backlog");
    };
    let written = match change_for(&item.title, &item.body, template, edit).map(|change| edited_file(&text, &item, change)) {
        Err(done) => return done,
        Ok(Err(error)) => return (Outcome::Failed, format!("Not saved: its header does not read ({error})")),
        Ok(Ok(written)) => written,
    };
    if std::fs::read_to_string(path).ok().as_deref() != Some(text.as_str()) {
        return changed();
    }
    match crate::state::write(path, written) {
        Ok(()) => saved(edit, template),
        Err(error) => (Outcome::Failed, format!("Not saved: {error}")),
    }
}

// The frontmatter and the text after it, as a vault note is stored.
fn split_note(text: &str) -> (&str, &str) {
    let Some(rest) = text.strip_prefix("---\n") else { return ("", text) };
    match rest.find("\n---\n") {
        Some(end) => text.split_at(4 + end + 5),
        None => ("", text),
    }
}

// The note's title as its frontmatter's YAML gives it; None when the frontmatter does not parse.
fn title_in(front: &str) -> Option<String> {
    let Some(yaml) = front.strip_prefix("---\n").and_then(|rest| rest.strip_suffix("---\n")) else { return Some(String::new()) };
    let documents = yaml_rust2::YamlLoader::load_from_str(yaml).ok()?;
    Some(match documents.first().map(|document| &document["title"]) {
        Some(yaml_rust2::Yaml::String(text) | yaml_rust2::Yaml::Real(text)) => text.clone(),
        Some(yaml_rust2::Yaml::Integer(number)) => number.to_string(),
        Some(yaml_rust2::Yaml::Boolean(flag)) => flag.to_string(),
        _ => String::new(),
    })
}

// The frontmatter with its title set, the lines a multi-line title continues on replaced too; a quoted JSON string is a YAML string.
fn retitled(front: &str, title: &str) -> String {
    let quoted = serde_json::to_string(title).unwrap_or_default();
    let mut lines: Vec<&str> = front.lines().collect();
    let line = format!("title: {quoted}");
    match lines.iter().position(|line| line.starts_with("title:")) {
        Some(at) => {
            let end = at + 1 + lines[at + 1..].iter().take_while(|line| line.starts_with([' ', '\t'])).count();
            lines.splice(at..end, [line.as_str()]);
        }
        None if lines.len() >= 2 => lines.insert(1, &line),
        None => {}
    }
    format!("{}\n", lines.join("\n"))
}

pub trait NoteStore {
    fn read(&self, key: &str) -> Result<(String, String)>;
    fn write(&self, key: &str, etag: &str, text: &str) -> Result<crate::vault_cli::Wrote>;
}

impl NoteStore for crate::vault_cli::Cli {
    fn read(&self, key: &str) -> Result<(String, String)> {
        self.read_note(key)
    }

    fn write(&self, key: &str, etag: &str, text: &str) -> Result<crate::vault_cli::Wrote> {
        self.write_note(key, etag, text)
    }
}

// A vault note is read as stored with its etag, edited, and written back against that etag, so a change made in between is never written over.
pub fn edit_note(store: &dyn NoteStore, key: &str, template: &Template, edit: &ItemEdit) -> Done {
    let (text, etag) = match store.read(key) {
        Ok(read) => read,
        Err(error) => return (Outcome::Failed, format!("Not saved: the vault did not give the note ({error})")),
    };
    let (front, body) = split_note(&text);
    let Some(title) = title_in(front) else { return (Outcome::Failed, "Not saved: the note's frontmatter does not read as YAML; edit it in the vault".to_string()) };
    let edited = match change_for(&title, body, template, edit) {
        Err(done) => return done,
        Ok(Change::Title(title)) => format!("{}{body}", retitled(front, &title)),
        Ok(Change::Body(body)) => format!("{front}{body}"),
    };
    match store.write(key, &etag, &edited) {
        Ok(crate::vault_cli::Wrote::Saved) => saved(edit, template),
        Ok(crate::vault_cli::Wrote::Stale) => refused(Reason::SectionChanged, "Not saved: the note changed in the vault while it was saved; read it again and edit what it says now"),
        Err(error) => (Outcome::Failed, format!("Not saved: {error}")),
    }
}

// A vault note the phone may edit is one this machine's backlog lists, named by its path inside the vault.
fn listed_note(entries: &[Entry], item: &ItemKey, root: Option<&Path>) -> Option<String> {
    entries.iter().filter(|entry| entry.source == "vault").find(|entry| key_of(entry, root).as_ref() == Some(item)).and_then(|entry| vault_path(&entry.reference, root))
}

fn writes(cli: &crate::vault_cli::Cli) -> bool {
    cli.capabilities.as_ref().is_some_and(|stated| stated.iter().any(|one| one == crate::vault_cli::WRITE))
}

// A task name from the item's title that no branch or running task in the repo has yet, so the start never resumes another task.
fn free_name(repo: &Path, title: &str) -> String {
    let base = Some(crate::board::suggested_name(title)).filter(|name| !name.is_empty()).unwrap_or_else(|| "item".to_string());
    let running: Vec<String> = crate::tasks::started_links().into_keys().collect();
    let repo_name = crate::repo::name_of(repo);
    (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
        .find(|name| !crate::repo::branch_exists(repo, name) && !running.contains(&crate::peers::address(name, &repo_name)))
        .unwrap_or(base)
}

fn start_found(found: &Found) -> Result<String> {
    let repo: PathBuf = crate::repo::locate(&found.entry.area)?;
    let name = free_name(&repo, &found.entry.title);
    let prompt = crate::backlog::task_prompt(&found.entry, &crate::config::sources()?, crate::vault_cli::known_on());
    let device = super::commands::acting();
    crate::board::start_in_background(&repo, &name, Some(&prompt), &crate::board::StartWith { backlog: Some(&found.entry.reference), device: device.as_ref().map(Id::as_str), ..Default::default() })?;
    Ok(crate::peers::address(&name, &crate::repo::name_of(&repo)))
}

fn desk_for<'a>(state: &'a Path, key: &ItemKey, tell: &'a dyn Fn(&str) -> Result<()>) -> Desk<'a> {
    let root = crate::vault::root();
    let started: Vec<String> = crate::tasks::started_items().into_iter().collect();
    let entry = crate::backlog::load().entries.into_iter().find(|entry| key_of(entry, root.as_deref()).as_ref() == Some(key));
    let index = crate::vault::Index::open().ok();
    let found = entry.map(|entry| {
        let (body, wherein) = body_of(&entry, index.as_ref(), state).unwrap_or_default();
        Found { entry, body, wherein }
    });
    Desk { state, found, started: started_as(key, &started, root.as_deref()), tell }
}

fn tell_lead(text: &str) -> Result<()> {
    crate::messages::to_lead(crate::messages::ToLead::BacklogItem, text)
}

pub fn idea(caller: &Caller, idea: &Idea) -> Done {
    match crate::config::sources() {
        Ok(sources) => {
            let (dir, state) = (backlog_file::dir(), crate::lead::state_dir());
            match state {
                Ok(state) => idea_on(&Shelf { dir: &dir, state: &state, tell: &tell_lead }, &sources, caller, idea),
                Err(error) => (Outcome::Failed, error),
            }
        }
        Err(error) => (Outcome::Failed, error),
    }
}

pub fn item_answer(caller: &Caller, answer: &ItemAnswer) -> Done {
    match crate::lead::state_dir() {
        Ok(state) => answer_on(&desk_for(&state, &answer.item, &tell_lead), caller, answer),
        Err(error) => (Outcome::Failed, error),
    }
}

pub fn item_edit(_: &Caller, edit: &ItemEdit) -> Done {
    let unknown = || refused(Reason::UnknownItem, "That item is no longer in this machine's backlog");
    match edit.item.home() {
        HomeKind::Kept => refused(Reason::BadRequest, "Not saved: the lead has yet to write it out and file it in the vault"),
        HomeKind::Found => refused(Reason::BadRequest, "Not saved: a found item is the tasks' own words; start it, add it to a task or dismiss it"),
        HomeKind::Local => {
            let root = crate::vault::root();
            let entry = crate::backlog::load().entries.into_iter().find(|entry| key_of(entry, root.as_deref()).as_ref() == Some(&edit.item));
            match entry.and_then(|entry| backlog_file::find(&entry.reference)) {
                Some((path, _)) => edit_local(&path, &crate::item_template::local(), edit),
                None => unknown(),
            }
        }
        HomeKind::Vault => {
            let root = crate::vault::root();
            let Some(key) = listed_note(&crate::backlog::load().entries, &edit.item, root.as_deref()) else { return unknown() };
            match crate::vault_cli::detect() {
                Some(cli) if writes(&cli) => edit_note(&cli, &key, &crate::item_template::for_vault(), edit),
                Some(_) => (Outcome::Failed, "Not saved: this machine's vault command states no write capability, so kebbie cannot write notes through it; edit the note in the vault".to_string()),
                None => (Outcome::Failed, "Not saved: this machine reaches no vault now".to_string()),
            }
        }
    }
}

pub fn start_item(_: &Caller, start: &StartItem) -> Done {
    match crate::lead::state_dir() {
        Ok(state) => start_on(&desk_for(&state, &start.item, &tell_lead), start, start_found),
        Err(error) => (Outcome::Failed, error),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use kebbie_hub_protocol::{BACKLOG_BODY_MEDIA_TYPE, Board, open_blob};

    use super::*;
    use crate::backlog_file::Shape;

    const BODY: &str = "## Background\n\nA red run waits under Needs you.\n\n## Goal\n\nA flaky red run is rerun once by itself.\nThe rest stay red.\n\n## What we know\n\n- G reruns a red run.\n\n## Open questions\n\n- [ ] Which red runs count as flaky?\n- [ ] Should a rerun show on the row?\n- How long to wait; the work settles it.\n\n## Out of scope\n\nRetrying a refused push.\n\n## Source\n\nThe user, 2026-11-12: \"retry flaky runs\"";

    fn entry(source: &str, reference: &str) -> Entry {
        let shape = Shape { state: Some(State::Drafting), waiting: 2, filed: "2026-11-10".to_string(), ..Shape::default() };
        Entry { source: source.to_string(), title: "Retry a flaky red run".to_string(), priority: "urgent".to_string(), area: "web".to_string(), reference: reference.to_string(), age: "2 d".to_string(), shape, ..Entry::default() }
    }

    fn templates() -> Templates {
        Templates { local: Template::builtin(), vault: Template::builtin() }
    }

    fn hub() -> Caller {
        Caller::Hub { account: "acc_01".to_string(), name: "iPhone 16".to_string() }
    }

    fn today() -> i64 {
        crate::backlog::epoch_day("2026-11-12").unwrap()
    }

    #[test]
    fn a_vault_note_has_one_key_on_every_machine_sharing_the_vault_and_a_local_item_one_the_protocol_takes() {
        let root = Path::new("/Users/someone/vault");
        let (one, two) = (crate::sh::test_dir("backlog-one"), crate::sh::test_dir("backlog-two"));
        let here = rows(&[(entry("vault", "Projects/retry.md"), None)], &one, Some(root), today(), &templates()).0;
        let there = rows(&[(entry("vault", "/Users/someone/vault/Projects/retry.md"), None)], &two, Some(root), today(), &templates()).0;
        assert_eq!(here[0].item.as_str(), "vault:Projects/retry.md");
        assert_eq!((here[0].item.clone(), here[0].home.clone()), (there[0].item.clone(), there[0].home.clone()), "the key and its label are the note's path inside the vault");
        assert!(rows(&[(entry("vault", "/elsewhere/Projects/retry.md"), None)], &one, Some(root), today(), &templates()).0.is_empty(), "a path outside the vault never leaves as a key");
        let ascii = key_of(&entry("local", "local:retry-flaky"), None).unwrap();
        let unicode = key_of(&entry("local", "local:åtgärda-något"), None).unwrap();
        assert_eq!(ascii.as_str(), "local:retry-flaky");
        assert!(unicode.as_str().starts_with("local:") && unicode != key_of(&entry("local", "local:åtgärda-annat"), None).unwrap(), "{unicode}");
        assert!(started_as(&unicode, &["local:åtgärda-något".to_string()], None) && started_as(&here[0].item, &["Projects/retry.md".to_string()], None));
        assert!(key_of(&entry("linear", "ABC-1"), None).is_none(), "a Linear row has no key the phone could send back");
    }

    #[test]
    fn a_row_carries_its_state_counts_and_goal_and_its_body_goes_as_a_blob_with_answers_still_with_the_lead() {
        let state = crate::sh::test_dir("backlog-rows");
        crate::backlog_kept::answer(&state, crate::backlog_kept::Answer { item: "Projects/retry.md".to_string(), question: "Should a rerun show on the row?".to_string(), answer: "Yes, as a dot".to_string(), at: 1 }).unwrap();
        let kept = Entry { source: "kept".to_string(), reference: "kept:a1b2c3".to_string(), area: String::new(), shape: Shape { state: Some(State::ToWrite), kept: true, ..Shape::default() }, ..entry("kept", "") };
        let items = [(entry("vault", "Projects/retry.md"), Some(BODY.to_string())), (entry("vault", "Projects/retry.md"), None), (kept, None), (entry("linear", "ABC-1"), None)];
        let (rows, bodies) = rows(&items, &state, None, today(), &templates());
        assert_eq!(rows.len(), 2, "one row per item, and no Linear row");
        let row = &rows[0];
        assert_eq!((row.priority, row.repo.as_deref(), row.state, row.filed), (Priority::High, Some("web"), BacklogState::Drafting, crate::backlog::epoch_day("2026-11-10").unwrap() as u64 * 86_400));
        assert_eq!((row.questions, row.blocking, row.goal.as_deref()), (3, 1, Some("A flaky red run is rerun once by itself.")), "a question answered on the card no longer blocks");
        assert_eq!((rows[1].state, rows[1].home.kind, rows[1].home.label.as_str(), rows[1].repo.as_ref(), rows[1].goal.as_ref()), (BacklogState::ToWrite, HomeKind::Kept, KEPT_LABEL, None, None));
        let [Attachment::Backlog { item, text }] = bodies.as_slice() else { panic!("{bodies:?}") };
        assert_eq!(item, &row.item);
        let body: BacklogBody = serde_json::from_str(text).unwrap();
        let part = |key: &str| body.section(key).map(|section| (section.heading.as_str(), section.text.as_str(), section.required));
        assert_eq!(body.sections.iter().map(|section| section.key.as_str()).collect::<Vec<_>>(), ["background", "goal", "known", "questions", "scope", "source"], "the template's sections in order");
        assert_eq!((part("goal"), part("scope"), part("known")), (Some(("Goal", "A flaky red run is rerun once by itself.\nThe rest stay red.", true)), Some(("Out of scope", "Retrying a refused push.", false)), Some(("What we know", "- G reruns a red run.", false))));
        assert_eq!((body.questions_key.as_deref(), part("questions").map(|(_, text, _)| text)), (Some("questions"), Some("")), "the questions go apart");
        assert_eq!(body.questions.iter().map(|question| (question.blocks, question.pending_answer.as_deref())).collect::<Vec<_>>(), [(true, None), (true, Some("Yes, as a dot")), (false, None)]);

        let mut board = crate::hub::board::build(&[], &[], Vec::new(), None, &crate::hub::board::Seen::default());
        board.backlog = Some(rows);
        let put = RefCell::new(Vec::new());
        let dir = crate::sh::test_dir("backlog-blobs");
        let attached = super::super::blobs::attach(&dir, &board, &bodies, 1_000, |id, ciphertext| { put.borrow_mut().push((id.clone(), ciphertext.to_vec())); Ok(()) });
        let blob = attached.backlog.as_ref().unwrap()[0].body.clone().expect("the row names its body once it is on the hub");
        assert_eq!(blob.media_type, BACKLOG_BODY_MEDIA_TYPE);
        assert_eq!(open_blob(&blob, &put.borrow()[0].1).unwrap(), text.as_bytes());
        assert!(attached.backlog.as_ref().unwrap()[1].body.is_none());
        let again = super::super::blobs::attach(&dir, &board, &bodies, 1_060, |_, _| panic!("an unchanged body is not put again"));
        assert_eq!(again.backlog.as_ref().unwrap()[0].body, Some(blob.clone()));
        let parsed: Board = serde_json::from_value(serde_json::to_value(&attached).unwrap()).unwrap();
        assert_eq!(parsed.backlog, attached.backlog, "the board holds to the protocol's rules for rows");
    }

    #[test]
    fn an_idea_from_the_phone_is_filed_as_kebbie_backlog_add_files_it_and_the_lead_hears_where_it_came_from() {
        let (dir, state) = (crate::sh::test_dir("idea-dir"), crate::sh::test_dir("idea-state"));
        let told = RefCell::new(Vec::new());
        let tell = |text: &str| { told.borrow_mut().push(text.to_string()); Ok(()) };
        let shelf = Shelf { dir: &dir, state: &state, tell: &tell };
        let idea = Idea { text: "When a run goes red on a flaky test,\nrerun it once by itself.\n\n## Not the push".to_string(), repo: Some("web".to_string()) };
        let (outcome, text) = idea_on(&shelf, &[Source::Local], &hub(), &idea);
        assert_eq!((outcome, text.as_str()), (Outcome::Done, "Added to the backlog. The lead writes it out; it waits for the lead when none runs."));
        let item = &backlog_file::items(&dir)[0];
        assert_eq!((item.title.as_str(), item.repo.as_str(), item.state()), ("When a run goes red on…", "web", State::ToWrite), "a placeholder of its first words, to write");
        let source = Template::builtin().text(&item.body, "source");
        assert!(source.contains("The user on iPhone 16, ") && source.contains("> When a run goes red on a flaky test,\n> rerun it once by itself.\n>\n> ## Not the push"), "the description in the user's words, its lines kept: {}", item.body);
        assert!(Template::builtin().sections.iter().all(|section| Template::builtin().has(&item.body, &section.key)), "{}", item.body);
        assert!(!item.body.contains("\n## Not the push"), "a line of theirs never reads as a heading");
        let first = told.borrow()[0].clone();
        assert!(first.starts_with("The user on iPhone 16 added an idea to the backlog, in their words:\n\n> When a run goes red on a flaky test,\n> rerun it once by itself."), "{first}");
        assert!(first.contains("Its title \"When a run goes red on…\" is a placeholder") && first.contains("give it a short title in the user's words"), "{first}");

        let (outcome, text) = idea_on(&shelf, &[Source::Vault], &hub(), &Idea { repo: None, ..idea.clone() });
        assert_eq!((outcome, text.as_str()), (Outcome::Done, "Sent to the lead. It writes the item out and files it in the vault. Its row waits here until the note replaces it."));
        let kept = crate::backlog_kept::kept_in(&state);
        assert_eq!((kept.len(), kept[0].line.as_str(), kept[0].repo.as_str()), (1, "When a run goes red on…", ""), "the board's row carries the placeholder until the note replaces it");
        assert!(told.borrow()[1].contains(&format!("kebbie backlog filed kept:{}", kept[0].id)), "{:?}", told.borrow());
        assert!(told.borrow()[1].contains("> rerun it once by itself.") && told.borrow()[1].contains("is a placeholder"), "the lead files the vault note from the user's words: {:?}", told.borrow());
        assert_eq!(idea_on(&shelf, &[Source::Local], &hub(), &idea).0, Outcome::Failed, "a title already in the backlog");
        assert_eq!(kept[0].words, idea.text.trim(), "the user's words stay with the row, whatever becomes of the message to the lead");
        let (rows, _) = rows(&[(kept[0].entry(0), None)], &state, None, 0, &templates());
        assert_eq!(rows[0].goal.as_deref(), Some("When a run goes red on a flaky test, rerun it once by itself. ## Not the push"), "the phone shows them on the row");
        let silent = |_: &str| -> Result<()> { Err("the lead is not running".to_string()) };
        let unheard = Shelf { dir: &dir, state: &state, tell: &silent };
        assert_eq!(idea_on(&unheard, &[Source::Vault], &hub(), &Idea { text: "Another idea".to_string(), repo: None }).0, Outcome::Failed);
        assert_eq!(crate::backlog_kept::kept_in(&state).len(), 1, "a row is kept only once the lead was told, so trying again adds no second one");
    }

    fn desk<'a>(state: &'a Path, found: Option<Found>, tell: &'a dyn Fn(&str) -> Result<()>) -> Desk<'a> {
        Desk { state, found, started: false, tell }
    }

    fn found(source: &str) -> Found {
        Found { entry: entry(source, "Projects/retry.md"), body: BODY.to_string(), wherein: "vault: Projects/retry.md".to_string() }
    }

    fn answering(index: u32, question: &str) -> ItemAnswer {
        ItemAnswer { item: ItemKey::vault("Projects/retry.md").unwrap(), index, question: question.to_string(), answer: " Only e2e ".to_string() }
    }

    #[test]
    fn an_answer_matches_the_question_by_place_and_words_and_waits_with_the_lead_as_one_given_on_the_card() {
        let state = crate::sh::test_dir("item-answer");
        let told = RefCell::new(Vec::new());
        let tell = |text: &str| { told.borrow_mut().push(text.to_string()); Ok(()) };
        assert_eq!(answer_on(&desk(&state, None, &tell), &hub(), &answering(0, "Which red runs count as flaky?")).0, Outcome::Refused { reason: Reason::UnknownItem });
        for changed in [answering(1, "Which red runs count as flaky?"), answering(0, "Which runs are flaky?"), answering(7, "Which red runs count as flaky?")] {
            assert_eq!(answer_on(&desk(&state, Some(found("vault")), &tell), &hub(), &changed).0, Outcome::Refused { reason: Reason::QuestionChanged }, "{changed:?}");
        }
        assert!(told.borrow().is_empty() && crate::backlog_kept::answers_in(&state).is_empty(), "nothing kept, nothing told");
        let (outcome, text) = answer_on(&desk(&state, Some(found("vault")), &tell), &hub(), &answering(0, "Which red runs count as flaky?"));
        assert_eq!((outcome, text.as_str()), (Outcome::Done, "With the lead: it folds your answer into the item"));
        let kept = crate::backlog_kept::answers_in(&state);
        assert_eq!((kept[0].item.as_str(), kept[0].question.as_str(), kept[0].answer.as_str()), ("Projects/retry.md", "Which red runs count as flaky?", "Only e2e"));
        assert_eq!(told.borrow()[0], crate::backlog_kept::answer_text("The user on iPhone 16", "Retry a flaky red run", "vault: Projects/retry.md", "Which red runs count as flaky?", "Only e2e"));
        assert_eq!(open_questions(BODY, "Projects/retry.md", &state, &Template::builtin())[0].pending_answer.as_deref(), Some("Only e2e"), "the board's card and the phone both show it with the lead");
    }

    #[test]
    fn a_start_is_refused_while_questions_wait_unless_anyway_and_otherwise_starts_as_g_does() {
        let state = crate::sh::test_dir("start-item");
        let tell = |_: &str| Ok(());
        let start = |anyway| StartItem { item: ItemKey::vault("Projects/retry.md").unwrap(), anyway };
        let never = |_: &Found| -> Result<String> { panic!("not started") };
        assert_eq!(start_on(&Desk { started: true, ..desk(&state, Some(found("vault")), &tell) }, &start(true), never).0, Outcome::Refused { reason: Reason::AlreadyStarted });
        assert_eq!(start_on(&desk(&state, None, &tell), &start(true), never).0, Outcome::Refused { reason: Reason::UnknownItem });
        let kept = Found { entry: Entry { shape: Shape { kept: true, ..Shape::default() }, ..entry("kept", "kept:a1b2c3") }, body: String::new(), wherein: String::new() };
        assert_eq!(start_on(&desk(&state, Some(kept), &tell), &start(true), never).0, Outcome::Refused { reason: Reason::BadRequest });
        let (outcome, text) = start_on(&desk(&state, Some(found("vault")), &tell), &start(false), never);
        assert_eq!((outcome, text.as_str()), (Outcome::Refused { reason: Reason::QuestionsOpen { blocking: 2 } }, "Not started: 2 questions wait on you. Answer them on the item, or start it anyway."));
        crate::backlog_kept::answer(&state, crate::backlog_kept::Answer { item: "Projects/retry.md".to_string(), question: "Which red runs count as flaky?".to_string(), answer: "e2e".to_string(), at: 1 }).unwrap();
        assert_eq!(start_on(&desk(&state, Some(found("vault")), &tell), &start(false), never).0, Outcome::Refused { reason: Reason::QuestionsOpen { blocking: 1 } }, "an answer with the lead no longer blocks");
        let nowhere = Found { entry: Entry { area: String::new(), ..entry("vault", "Projects/retry.md") }, ..found("vault") };
        assert_eq!(start_on(&desk(&state, Some(nowhere), &tell), &start(true), never).0, Outcome::Refused { reason: Reason::UnknownRepo });
        let ran = RefCell::new(None);
        let (outcome, text) = start_on(&desk(&state, Some(found("vault")), &tell), &start(true), |found| { *ran.borrow_mut() = Some(found.entry.reference.clone()); Ok("retry-flaky-red-run@web".to_string()) });
        assert_eq!((outcome, text.as_str()), (Outcome::Done, "Starting retry-flaky-red-run@web. It shows under Work in a moment; if it cannot start, a notice says why"));
        assert_eq!(ran.borrow().as_deref(), Some("Projects/retry.md"), "started from the item, as g does");
        assert_eq!(start_on(&desk(&state, Some(found("vault")), &tell), &start(true), |_| Err("kebbie start: no repo".to_string())).0, Outcome::Failed);
    }

    fn editing(section: Option<&str>, text: &str, base: &str) -> ItemEdit {
        ItemEdit { item: ItemKey::parse("local:retry").unwrap(), section: section.map(str::to_string), text: text.to_string(), base: text_hash(base) }
    }

    #[test]
    fn an_edit_lands_only_on_the_text_the_phone_saw_and_never_empties_a_required_section() {
        let template = Template::builtin();
        let goal = template.shown(BODY, "goal");
        let Ok(Change::Body(body)) = change_for("Retry", BODY, &template, &editing(Some("goal"), "Rerun it once.", &goal)) else { panic!("applied") };
        assert_eq!(template.text(&body, "goal"), "Rerun it once.");
        assert_eq!(template.questions_in(&body), template.questions_in(BODY), "the rest stands");
        assert_eq!(change_for("Retry", BODY, &template, &editing(Some("goal"), "x", "an older goal")).unwrap_err().0, Outcome::Refused { reason: Reason::SectionChanged });
        assert_eq!(change_for("Retry", BODY, &template, &editing(Some("goal"), "  ", &goal)).unwrap_err(), (Outcome::Refused { reason: Reason::BadRequest }, "Not saved: Goal is required, so it cannot be left empty".to_string()));
        assert!(matches!(change_for("Retry", BODY, &template, &editing(Some("scope"), "", "Retrying a refused push.")), Ok(Change::Body(_))), "an optional section may be emptied");
        assert_eq!(change_for("Retry", BODY, &template, &editing(Some("risks"), "x", "")).unwrap_err().0, Outcome::Refused { reason: Reason::SectionChanged }, "a section the template does not have");
        assert_eq!(change_for("Retry", BODY, &template, &editing(None, "Retry flaky runs", "Retry")).unwrap(), Change::Title("Retry flaky runs".to_string()));
        assert_eq!(change_for("Retry", BODY, &template, &editing(None, "New", "Old")).unwrap_err().0, Outcome::Refused { reason: Reason::SectionChanged });
        let Ok(Change::Body(old)) = change_for("Retry", "## Problem\n\nIt broke.\n", &template, &editing(Some("goal"), "Works.", "")) else { panic!("an older note takes the section") };
        assert_eq!((template.text(&old, "goal"), old.starts_with("## Problem\n\nIt broke.")), ("Works.".to_string(), true));
    }

    #[test]
    fn a_local_item_is_edited_in_its_file_and_keeps_the_header_keys_kebbie_does_not_read() {
        let dir = crate::sh::test_dir("edit-local");
        let path = dir.join("retry.md");
        let item = backlog_file::Item { stem: "retry".to_string(), title: "Retry".to_string(), priority: "high".to_string(), repo: "web".to_string(), status: "backlog".to_string(), filed: "2026-11-12".to_string(), body: BODY.to_string() };
        std::fs::write(&path, item.text().replacen("+++\n", "+++\nowner = \"ops\" # who asked\n", 1)).unwrap();
        let template = Template::builtin();
        let done = edit_local(&path, &template, &editing(Some("known"), "- It reruns once.", "- G reruns a red run."));
        assert_eq!(done, (Outcome::Done, "Saved What we know".to_string()));
        let saved = backlog_file::parse("retry", &std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!((template.text(&saved.body, "known"), saved.priority.as_str()), ("- It reruns once.".to_string(), "high"));
        assert_eq!(edit_local(&path, &template, &editing(None, "Retry flaky runs", "Retry")).0, Outcome::Done);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(backlog_file::parse("retry", &text).unwrap().title, "Retry flaky runs");
        assert!(text.starts_with("+++\nowner = \"ops\" # who asked\ntitle = \"Retry flaky runs\"\n"), "{text}");
        assert_eq!(edit_local(&path, &template, &editing(None, "Again", "Retry")).0, Outcome::Refused { reason: Reason::SectionChanged });
        let hand = dir.join("by-hand.md");
        std::fs::write(&hand, "Fix the login\n\n## Goal\n\nIt works.\n").unwrap();
        assert_eq!(edit_local(&hand, &template, &editing(Some("goal"), "It works again.", "It works.")).0, Outcome::Done);
        assert_eq!(std::fs::read_to_string(&hand).unwrap(), "Fix the login\n\n## Goal\n\nIt works again.\n", "a hand-written file keeps its shape");
        assert_eq!(edit_local(&dir.join("gone.md"), &template, &editing(Some("goal"), "x", "")).0, Outcome::Refused { reason: Reason::UnknownItem });
    }

    #[test]
    fn the_phone_edits_only_a_vault_note_the_backlog_lists() {
        let root = Path::new("/Users/someone/vault");
        let entries = [entry("vault", "/Users/someone/vault/Projects/retry.md"), entry("local", "local:other")];
        let key = |path: &str| ItemKey::vault(path).unwrap();
        assert_eq!(listed_note(&entries, &key("Projects/retry.md"), Some(root)).as_deref(), Some("Projects/retry.md"));
        for elsewhere in ["Areas/kebbie.md", "Log/web-logg.md", "Projects/held-by-another-machine.md"] {
            assert_eq!(listed_note(&entries, &key(elsewhere), Some(root)), None, "{elsewhere} is not in the backlog");
        }
    }

    #[test]
    fn the_rows_goal_is_the_templates_goal_or_else_its_first_required_section() {
        let template = Template { sections: vec![crate::item_template::Section { key: "context".to_string(), heading: "Context".to_string(), required: false }, crate::item_template::Section { key: "problem".to_string(), heading: "Problem".to_string(), required: true }], questions: None };
        let state = crate::sh::test_dir("backlog-goal");
        let templates = Templates { local: template.clone(), vault: template };
        let rows = rows(&[(entry("vault", "Projects/retry.md"), Some("## Context\n\nSome.\n\n## Problem\n\nIt breaks.\n".to_string()))], &state, None, today(), &templates).0;
        assert_eq!(rows[0].goal.as_deref(), Some("It breaks."));
    }

    struct Store {
        text: String,
        written: RefCell<Vec<(String, String)>>,
        answer: Result<crate::vault_cli::Wrote>,
    }

    impl Store {
        fn holding(text: &str) -> Store {
            Store { text: text.to_string(), written: RefCell::new(Vec::new()), answer: Ok(crate::vault_cli::Wrote::Saved) }
        }
    }

    impl NoteStore for Store {
        fn read(&self, _: &str) -> Result<(String, String)> {
            Ok((self.text.clone(), "e1".to_string()))
        }

        fn write(&self, _: &str, etag: &str, text: &str) -> Result<crate::vault_cli::Wrote> {
            self.written.borrow_mut().push((etag.to_string(), text.to_string()));
            match &self.answer {
                Ok(crate::vault_cli::Wrote::Saved) => Ok(crate::vault_cli::Wrote::Saved),
                Ok(crate::vault_cli::Wrote::Stale) => Ok(crate::vault_cli::Wrote::Stale),
                Err(error) => Err(error.clone()),
            }
        }
    }

    #[test]
    fn a_vault_note_is_written_back_against_the_etag_it_was_read_with() {
        let note = format!("---\ntitle: \"Retry \\\"flaky\\\" runs\"\nstatus: backlog\n---\n{BODY}");
        let store = Store::holding(&note);
        let template = Template::builtin();
        let goal = template.shown(BODY, "goal");
        assert_eq!(edit_note(&store, "Projects/retry.md", &template, &editing(Some("goal"), "Once.", &goal)), (Outcome::Done, "Saved Goal".to_string()));
        let (etag, written) = store.written.borrow()[0].clone();
        assert_eq!(etag, "e1");
        assert!(written.starts_with("---\ntitle: \"Retry \\\"flaky\\\" runs\"\nstatus: backlog\n---\n## Background"), "{written}");
        assert_eq!(template.text(&written, "goal"), "Once.");
        assert_eq!(edit_note(&store, "Projects/retry.md", &template, &editing(None, "Retry once", "Retry \"flaky\" runs")).0, Outcome::Done, "the title as the note stores it");
        assert!(store.written.borrow()[1].1.starts_with("---\ntitle: \"Retry once\"\nstatus: backlog\n---\n"), "{}", store.written.borrow()[1].1);
        let raced = Store { answer: Ok(crate::vault_cli::Wrote::Stale), ..Store::holding(&note) };
        assert_eq!(edit_note(&raced, "Projects/retry.md", &template, &editing(Some("goal"), "Once.", &goal)).0, Outcome::Refused { reason: Reason::SectionChanged });
        let broken = Store { answer: Err("vault write: mindin: error: argument --etag: the etag changed since it was read".to_string()), ..Store::holding(&note) };
        assert_eq!(edit_note(&broken, "Projects/retry.md", &template, &editing(Some("goal"), "Once.", &goal)).0, Outcome::Failed, "an error that only mentions the etag is no change in the vault");
    }

    #[test]
    fn a_vault_notes_title_is_read_and_set_as_its_yaml_holds_it() {
        let template = Template::builtin();
        let folded = Store::holding(&format!("---\nstatus: backlog\ntitle: >-\n  Retry a flaky\n  red run\nkind: task\n---\n{BODY}"));
        assert_eq!(edit_note(&folded, "Projects/retry.md", &template, &editing(None, "Retry once", "Retry a flaky red run")).0, Outcome::Done, "a folded title reads as one line");
        assert_eq!(folded.written.borrow()[0].1, format!("---\nstatus: backlog\ntitle: \"Retry once\"\nkind: task\n---\n{BODY}"), "its continuation lines go with it");
        let single = Store::holding(&format!("---\ntitle: 'It''s: a # title' # comment\n---\n{BODY}"));
        assert_eq!(edit_note(&single, "Projects/retry.md", &template, &editing(None, "New", "It's: a # title")).0, Outcome::Done);
        let broken = Store::holding(&format!("---\ntitle: [unclosed\n---\n{BODY}"));
        assert_eq!(edit_note(&broken, "Projects/retry.md", &template, &editing(None, "New", "")).0, Outcome::Failed);
        assert!(broken.written.borrow().is_empty(), "nothing is written over a frontmatter that does not read");
    }

    #[test]
    fn what_tasks_found_reaches_the_phone_as_rows_of_its_own_and_the_phone_starts_adds_or_dismisses_them() {
        use kebbie_hub_protocol::{FoundAdd, FoundDismiss, FoundStart};
        let state = crate::sh::test_dir("hub-found");
        crate::found::file_card(&state, "billing@app", "Also noticed:\n- The VAT rounding test is flaky on main\n- The README names an old flag\n", 100).unwrap();
        crate::found::file_card(&state, "search@app", "Also noticed:\n- The VAT rounding test is flaky on main\n", 200).unwrap();
        let rows = found_rows(&crate::found::open_in(&state));
        let shown: Vec<(&str, &str, Option<&str>, u64, HomeKind)> = rows.iter().map(|row| (row.item.as_str(), row.title.as_str(), row.repo.as_deref(), row.filed, row.home.kind)).collect();
        assert_eq!(shown, [("found:f1", "The VAT rounding test is flaky on main", Some("app"), 100, HomeKind::Found), ("found:f2", "The README names an old flag", Some("app"), 100, HomeKind::Found)]);
        let found: Vec<Option<(Vec<String>, u32)>> = rows.iter().map(|row| row.found.as_ref().map(|found| (found.sources.clone(), found.count))).collect();
        assert_eq!(found, [Some((vec!["billing@app".to_string(), "search@app".to_string()], 2)), Some((vec!["billing@app".to_string()], 1))], "each with the tasks that found it and how often");
        let board = serde_json::json!({ "rev": 1, "kebbie": "1", "away_since": null, "checkouts": [], "routines": [], "items": [], "sessions_outside_tasks": 0, "backlog": rows });
        assert!(serde_json::from_value::<Board>(board).is_ok(), "the hub reads them");
        let key = |id: &str| ItemKey::parse(&format!("found:{id}")).unwrap();
        let caller = hub();
        let never = |_: &str, _: Option<&str>| -> Result<String> { panic!("not started") };
        let no_add = |_: &str, _: &str| -> Result<String> { panic!("not added") };
        let started = std::cell::RefCell::new(None);
        let (outcome, _) = found_on(&state, &caller, &Payload::FoundStart(FoundStart { item: key("f2"), name: Some("readme-flag".to_string()) }), |id, name| { *started.borrow_mut() = Some((id.to_string(), name.map(str::to_string))); Ok("Started".to_string()) }, no_add);
        assert_eq!((outcome, started.into_inner()), (Outcome::Done, Some(("f2".to_string(), Some("readme-flag".to_string())))));
        let added = std::cell::RefCell::new(None);
        found_on(&state, &caller, &Payload::FoundAdd(FoundAdd { item: key("f1"), task: "search@app".to_string() }), never, |id, task| { *added.borrow_mut() = Some((id.to_string(), task.to_string())); Ok("Added".to_string()) });
        assert_eq!(added.into_inner(), Some(("f1".to_string(), "search@app".to_string())));
        assert_eq!(found_on(&state, &caller, &Payload::FoundDismiss(FoundDismiss { item: key("f1") }), never, no_add).0, Outcome::Done);
        assert_eq!(found_on(&state, &caller, &Payload::FoundDismiss(FoundDismiss { item: key("f1") }), never, no_add).0, Outcome::Refused { reason: Reason::AlreadySettled }, "settled once");
        assert_eq!(found_on(&state, &caller, &Payload::FoundDismiss(FoundDismiss { item: key("f9") }), never, no_add).0, Outcome::Refused { reason: Reason::AlreadySettled });
        assert!(found_rows(&crate::found::open_in(&state)).iter().all(|row| row.item.as_str() != "found:f1"), "a dismissed item leaves the phone");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn keep_in_backlog_from_the_phone_leaves_the_item_filed_and_the_card_shows_it_kept() {
        use kebbie_hub_protocol::FoundKeep;
        let state = crate::sh::test_dir("hub-found-keep");
        let card = crate::found::file_card(&state, "billing@app", "**Refunds.**\n\nBuilt:\n- x\n\nAlso noticed:\n- The README names an old flag\n", 100).unwrap().text;
        let parsed = crate::card::parse(&card).unwrap();
        let data = crate::card::next_version(&parsed, Vec::new(), None, 100);
        let key = ItemKey::parse("found:f1").unwrap();
        let never = |_: &str, _: Option<&str>| -> Result<String> { panic!("not started") };
        let no_add = |_: &str, _: &str| -> Result<String> { panic!("not added") };
        assert_eq!(found_on(&state, &hub(), &Payload::FoundKeep(FoundKeep { item: key.clone() }), never, no_add).0, Outcome::Done);
        assert_eq!(found_rows(&crate::found::open_in(&state)).len(), 1, "a kept item stays on the phone's backlog");
        let wired = crate::card::wire(&data, &crate::found::load(&state)).unwrap();
        assert_eq!(wired.noticed[0].chosen, Some(kebbie_hub_protocol::NoticedChoice::Kept), "the card shows the choice, read from the item");
        let task = Caller::Task("billing@app".to_string());
        assert_eq!(found_on(&state, &task, &Payload::FoundKeep(FoundKeep { item: key }), never, no_add).0, Outcome::Failed, "a task keeps nothing");
        let _ = std::fs::remove_dir_all(&state);
    }
}
