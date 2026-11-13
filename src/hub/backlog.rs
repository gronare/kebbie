use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use kebbie_hub_protocol::{BacklogBody, BacklogRow, BacklogState, Home, HomeKind, Id, Idea, ItemAnswer, ItemKey, OpenQuestion, Outcome, Priority, Reason, StartItem};

use super::blobs::{Attachment, PLAN_LIMIT};
use super::commands::{Done, plain};
use crate::backlog::Entry;
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
    }
}

fn section(body: &str, heading: &str) -> String {
    backlog_file::section(body, heading).trim().to_string()
}

// The item's questions as the phone shows them, each with the user's answer while it waits with the lead.
pub fn open_questions(body: &str, reference: &str, state: &Path) -> Vec<OpenQuestion> {
    questions_with(body, reference, &crate::backlog_kept::answers_in(state))
}

fn questions_with(body: &str, reference: &str, answers: &[crate::backlog_kept::Answer]) -> Vec<OpenQuestion> {
    backlog_file::questions_in(body).into_iter().map(|question| {
        let pending_answer = answers.iter().find(|answer| answer.item == reference && answer.question == question.text).map(|answer| answer.answer.clone());
        OpenQuestion { text: question.text, blocks: question.blocks, pending_answer }
    }).collect()
}

fn waiting(questions: &[OpenQuestion]) -> usize {
    questions.iter().filter(|question| question.blocks && question.pending_answer.is_none()).count()
}

// The rows of the board's Backlog column and each written item's body as a blob; Linear rows stay on the machine, having no key the phone can send back.
pub fn rows(items: &[(Entry, Option<String>)], state: &Path, root: Option<&Path>, today: i64) -> (Vec<BacklogRow>, Vec<Attachment>) {
    let mut seen = BTreeSet::new();
    let (mut rows, mut bodies) = (Vec::new(), Vec::new());
    let answers = crate::backlog_kept::answers_in(state);
    for (entry, body) in items {
        let Some(item) = key_of(entry, root).filter(|item| seen.insert(item.clone())) else { continue };
        let body = body.as_deref().filter(|body| !body.trim().is_empty());
        let questions = body.map(|body| questions_with(body, &entry.reference, &answers)).unwrap_or_default();
        let goal = body.and_then(|body| section(body, "Goal").lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string));
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
        });
        if let Some(body) = body {
            let written = BacklogBody { background: section(body, "Background"), goal: section(body, "Goal"), what_we_know: section(body, "What we know"), questions, out_of_scope: section(body, "Out of scope"), source: section(body, "Source") };
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
    rows(&crate::backlog::with_bodies(), state, crate::vault::root().as_deref(), crate::backlog::today())
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
        open_questions(&self.body, &self.entry.reference, state)
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

// The phone's idea is filed as kebbie backlog add from the user files it: a draft item, or a row kebbie keeps while the lead files it in the vault.
pub fn idea_on(shelf: &Shelf, sources: &[Source], caller: &Caller, idea: &Idea) -> Done {
    let line = idea.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let by = who(caller);
    let filing = Filing { priority: "", repo: idea.repo.as_deref().unwrap_or_default(), line: &line, body: None, by: &by };
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
        Ok(task) => (Outcome::Done, format!("Starting {task}: it shows under Work in a moment")),
        Err(error) => (Outcome::Failed, plain(&error)),
    }
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
        let here = rows(&[(entry("vault", "Projects/retry.md"), None)], &one, Some(root), today()).0;
        let there = rows(&[(entry("vault", "/Users/someone/vault/Projects/retry.md"), None)], &two, Some(root), today()).0;
        assert_eq!(here[0].item.as_str(), "vault:Projects/retry.md");
        assert_eq!((here[0].item.clone(), here[0].home.clone()), (there[0].item.clone(), there[0].home.clone()), "the key and its label are the note's path inside the vault");
        assert!(rows(&[(entry("vault", "/elsewhere/Projects/retry.md"), None)], &one, Some(root), today()).0.is_empty(), "a path outside the vault never leaves as a key");
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
        let (rows, bodies) = rows(&items, &state, None, today());
        assert_eq!(rows.len(), 2, "one row per item, and no Linear row");
        let row = &rows[0];
        assert_eq!((row.priority, row.repo.as_deref(), row.state, row.filed), (Priority::High, Some("web"), BacklogState::Drafting, crate::backlog::epoch_day("2026-11-10").unwrap() as u64 * 86_400));
        assert_eq!((row.questions, row.blocking, row.goal.as_deref()), (3, 1, Some("A flaky red run is rerun once by itself.")), "a question answered on the card no longer blocks");
        assert_eq!((rows[1].state, rows[1].home.kind, rows[1].home.label.as_str(), rows[1].repo.as_ref(), rows[1].goal.as_ref()), (BacklogState::ToWrite, HomeKind::Kept, KEPT_LABEL, None, None));
        let [Attachment::Backlog { item, text }] = bodies.as_slice() else { panic!("{bodies:?}") };
        assert_eq!(item, &row.item);
        let body: BacklogBody = serde_json::from_str(text).unwrap();
        assert_eq!((body.goal.as_str(), body.out_of_scope.as_str(), body.what_we_know.as_str()), ("A flaky red run is rerun once by itself.\nThe rest stay red.", "Retrying a refused push.", "- G reruns a red run."));
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
        let idea = Idea { text: "Rerun a flaky\nred run once".to_string(), repo: Some("web".to_string()) };
        let (outcome, text) = idea_on(&shelf, &[Source::Local], &hub(), &idea);
        assert_eq!((outcome, text.as_str()), (Outcome::Done, "Added to the backlog. The lead writes it out; it waits for the lead when none runs."));
        let item = &backlog_file::items(&dir)[0];
        assert_eq!((item.title.as_str(), item.repo.as_str(), item.state()), ("Rerun a flaky red run once", "web", State::ToWrite));
        assert!(backlog_file::section(&item.body, "Source").contains("The user on iPhone 16, "), "{}", item.body);
        assert!(told.borrow()[0].starts_with("The user on iPhone 16 added a quick item to the backlog: \"Rerun a flaky red run once\""), "{:?}", told.borrow());

        let (outcome, text) = idea_on(&shelf, &[Source::Vault], &hub(), &Idea { repo: None, ..idea.clone() });
        assert_eq!((outcome, text.as_str()), (Outcome::Done, "Sent to the lead. It writes the item out and files it in the vault. Its row waits here until the note replaces it."));
        let kept = crate::backlog_kept::kept_in(&state);
        assert_eq!((kept.len(), kept[0].line.as_str(), kept[0].repo.as_str()), (1, "Rerun a flaky red run once", ""));
        assert!(told.borrow()[1].contains(&format!("kebbie backlog filed kept:{}", kept[0].id)), "{:?}", told.borrow());
        assert_eq!(idea_on(&shelf, &[Source::Local], &hub(), &idea).0, Outcome::Failed, "a title already in the backlog");
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
        assert_eq!(open_questions(BODY, "Projects/retry.md", &state)[0].pending_answer.as_deref(), Some("Only e2e"), "the board's card and the phone both show it with the lead");
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
        assert_eq!((outcome, text.as_str()), (Outcome::Done, "Starting retry-flaky-red-run@web: it shows under Work in a moment"));
        assert_eq!(ran.borrow().as_deref(), Some("Projects/retry.md"), "started from the item, as g does");
        assert_eq!(start_on(&desk(&state, Some(found("vault")), &tell), &start(true), |_| Err("kebbie start: no repo".to_string())).0, Outcome::Failed);
    }
}
