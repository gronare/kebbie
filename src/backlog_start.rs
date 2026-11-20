use std::path::Path;

use crate::backlog::Entry;
use crate::backlog_file::State;
use crate::backlog_kept::{Start, same_item};
use crate::sh::Result;

pub const EVERY: u64 = 30;

// An ask whose item is not in the backlog this long, and was not started, is let go.
const GONE_AFTER: u64 = crate::backlog_file::ASK_AGAIN_AFTER;

#[derive(Debug, PartialEq)]
pub enum Step {
    Wait,
    Start,
    NoRepo,
    Gone,
}

// What becomes of an ask to start an item once written: it starts once the item is ready and names its repo, and leaves once the item runs or is gone.
pub fn step(start: &Start, entry: Option<&Entry>, running: bool, now: u64) -> Step {
    if running {
        return Step::Gone;
    }
    match entry {
        None if now.saturating_sub(start.at) >= GONE_AFTER => Step::Gone,
        None => Step::Wait,
        Some(entry) if entry.shape.kept || entry.shape.state != Some(State::Ready) || entry.shape.writing.is_some() => Step::Wait,
        Some(entry) if entry.area.trim().is_empty() => Step::NoRepo,
        Some(_) => Step::Start,
    }
}

pub fn no_repo_text(title: &str, reference: &str) -> String {
    format!("The backlog item \"{title}\" ({reference}) is written out and the user asked to start it once written, but it names no repo, so kebbie did not start it. Set its repo, then tell the user to start it with g on the board.")
}

pub fn failed_text(title: &str, reference: &str, why: &str) -> String {
    format!("The backlog item \"{title}\" ({reference}) is written out and the user asked to start it once written, but kebbie could not start it: {why}. Tell the user, so they start it with g on the board once that is fixed.")
}

pub fn run_on(state: &Path, entries: &[Entry], running: &dyn Fn(&str) -> bool, now: u64, start: &dyn Fn(&Entry) -> Result<String>, tell: &dyn Fn(&str) -> Result<()>) -> Vec<String> {
    let mut notes = Vec::new();
    for asked in crate::backlog_kept::starts_in(state) {
        let entry = entries.iter().find(|entry| same_item(&entry.reference, &asked.item));
        let done = match (step(&asked, entry, running(&asked.item), now), entry) {
            (Step::Wait, _) => continue,
            (Step::Gone, _) => format!("start: {} left the backlog, its start let go", asked.item),
            (Step::NoRepo, Some(entry)) => {
                let _ = tell(&no_repo_text(&entry.title, &entry.reference));
                format!("start: {} names no repo; the lead was told", asked.item)
            }
            (Step::Start, Some(entry)) => match start(entry) {
                Ok(task) => format!("start: started {task} from {}", asked.item),
                Err(error) => {
                    let _ = tell(&failed_text(&entry.title, &entry.reference, &error));
                    format!("start: {} did not start ({error})", asked.item)
                }
            },
            (_, None) => continue,
        };
        if let Err(error) = crate::backlog_kept::drop_start(state, &asked.item) {
            notes.push(format!("start: {} stays asked ({error})", asked.item));
            continue;
        }
        notes.push(done);
    }
    notes
}

// The keeper's round: only while an ask waits is the backlog read.
pub fn tick(state: &Path) -> Vec<String> {
    if crate::backlog_kept::starts_in(state).is_empty() {
        return Vec::new();
    }
    let entries = crate::backlog::load().entries;
    let running = crate::tasks::started_items();
    run_on(state, &entries, &|item| running.iter().any(|started| same_item(started, item)), crate::messages::now(), &|entry| crate::hub::backlog::start_entry(entry, None), &crate::backlog_file::tell_lead)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::backlog_file::Shape;

    fn entry(reference: &str, area: &str, shape: Shape) -> Entry {
        Entry { source: "local".to_string(), title: "Retry a flaky run".to_string(), area: area.to_string(), reference: reference.to_string(), shape, ..Entry::default() }
    }

    fn ready() -> Shape {
        Shape { state: Some(State::Ready), start: true, ..Shape::default() }
    }

    #[test]
    fn an_item_asked_to_start_waits_until_it_is_written_with_no_question_on_the_user_and_names_its_repo() {
        let asked = Start { item: "local:retry".to_string(), at: 1_000 };
        for (shape, why) in [(Shape { state: Some(State::ToWrite), ..ready() }, "to write"), (Shape { state: Some(State::Drafting), waiting: 1, ..ready() }, "a question waits"), (Shape { state: Some(State::Drafting), with_lead: 1, ..ready() }, "an answer is with the lead"), (Shape { writing: Some(900), ..ready() }, "being written"), (Shape { kept: true, ..ready() }, "a kept row the lead has yet to file")] {
            assert_eq!(step(&asked, Some(&entry("local:retry", "kebbie", shape)), false, 2_000), Step::Wait, "{why}");
        }
        assert_eq!(step(&asked, Some(&entry("local:retry", "kebbie", ready())), false, 2_000), Step::Start);
        assert_eq!(step(&asked, Some(&entry("local:retry", "", ready())), false, 2_000), Step::NoRepo);
        assert_eq!(step(&asked, Some(&entry("local:retry", "kebbie", ready())), true, 2_000), Step::Gone, "started already, with g");
        assert_eq!(step(&asked, None, false, 2_000), Step::Wait, "not yet in the backlog the keeper read");
        assert_eq!(step(&asked, None, false, 1_000 + GONE_AFTER), Step::Gone);
    }

    #[test]
    fn the_keeper_starts_a_ready_item_once_and_tells_the_lead_when_it_cannot() {
        let state = crate::sh::test_dir("backlog-start-run");
        for item in ["local:retry", "Projects/docs.md", "local:no-repo", "local:later"] {
            crate::backlog_kept::ask_start(&state, item, 1_000).unwrap();
        }
        let entries = [entry("local:retry", "kebbie", ready()), entry("/vault/Projects/docs.md", "web", ready()), entry("local:no-repo", "", ready()), entry("local:later", "kebbie", Shape { state: Some(State::Drafting), waiting: 2, ..ready() })];
        let (started, told) = (RefCell::new(Vec::new()), RefCell::new(Vec::new()));
        let start = |entry: &Entry| -> Result<String> {
            started.borrow_mut().push(entry.reference.clone());
            if entry.area == "web" { Err("no repo web on this machine".to_string()) } else { Ok(format!("retry@{}", entry.area)) }
        };
        let tell = |text: &str| -> Result<()> { told.borrow_mut().push(text.to_string()); Ok(()) };
        let notes = run_on(&state, &entries, &|_| false, 2_000, &start, &tell);
        assert_eq!(started.borrow().as_slice(), ["local:retry", "/vault/Projects/docs.md"], "a vault note matches by its path inside the vault");
        assert!(notes.contains(&"start: started retry@kebbie from local:retry".to_string()), "{notes:?}");
        assert_eq!(told.borrow().as_slice(), [failed_text("Retry a flaky run", "/vault/Projects/docs.md", "no repo web on this machine"), no_repo_text("Retry a flaky run", "local:no-repo")]);
        assert_eq!(crate::backlog_kept::starts_in(&state).iter().map(|start| start.item.as_str()).collect::<Vec<_>>(), ["local:later"], "only the item with questions waiting stays asked");
        assert!(run_on(&state, &entries, &|_| false, 2_000, &start, &tell).is_empty() && started.borrow().len() == 2, "started once");
        let _ = std::fs::remove_dir_all(state);
    }
}
