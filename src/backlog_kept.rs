use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::backlog::Entry;
use crate::backlog_file::{Question, Shape, State};
use crate::sh::Result;

pub const PREFIX: &str = "kept:";

// What kebbie keeps for the backlog until the lead has acted on it: a short item while the lead files it in the vault, and the user's answers to an item's open questions while the lead folds them in.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub id: String,
    pub line: String,
    pub priority: String,
    pub repo: String,
    pub at: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub item: String,
    pub question: String,
    pub answer: String,
    pub at: u64,
}

fn kept_file(state: &Path) -> PathBuf {
    state.join("backlog-kept.json")
}

fn answers_file(state: &Path) -> PathBuf {
    state.join("backlog-answers.json")
}

// Held across each read, change and write of one of the two files, so the board, the keeper and the CLI never drop each other's change.
struct Locked(std::fs::File);

impl Locked {
    fn take(file: &Path) -> Result<Locked> {
        let path = file.with_extension("lock");
        let lock = crate::state::append_to(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        // SAFETY: flock on a descriptor this File owns.
        match unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } {
            0 => Ok(Locked(lock)),
            _ => Err(format!("{}: {}", path.display(), std::io::Error::last_os_error())),
        }
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn read(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|value| value.as_array().cloned()).unwrap_or_default()
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

pub fn kept_in(state: &Path) -> Vec<Kept> {
    read(&kept_file(state)).iter().map(|value| Kept { id: text(value, "id"), line: text(value, "line"), priority: text(value, "priority"), repo: text(value, "repo"), at: value["at"].as_u64().unwrap_or(0) })
        .filter(|kept| !kept.id.is_empty()).collect()
}

fn write_kept(state: &Path, kept: &[Kept]) -> Result<()> {
    let values: Vec<Value> = kept.iter().map(|kept| json!({ "id": kept.id, "line": kept.line, "priority": kept.priority, "repo": kept.repo, "at": kept.at })).collect();
    crate::state::write(&kept_file(state), Value::Array(values).to_string())
}

pub fn keep(state: &Path, line: &str, priority: &str, repo: &str, at: u64) -> Result<String> {
    let _locked = Locked::take(&kept_file(state))?;
    let mut kept = kept_in(state);
    let id = (0u32..).map(|attempt| sha1_smol::Sha1::from(format!("{line}\n{at}\n{attempt}")).digest().to_string()[..6].to_string())
        .find(|id| !kept.iter().any(|other| &other.id == id)).unwrap_or_default();
    kept.push(Kept { id: id.clone(), line: line.trim().to_string(), priority: priority.to_string(), repo: repo.to_string(), at });
    write_kept(state, &kept)?;
    Ok(id)
}

pub fn release(state: &Path, id: &str) -> Result<bool> {
    let id = id.strip_prefix(PREFIX).unwrap_or(id);
    let _locked = Locked::take(&kept_file(state))?;
    let mut kept = kept_in(state);
    let before = kept.len();
    kept.retain(|other| other.id != id);
    if kept.len() == before {
        return Ok(false);
    }
    write_kept(state, &kept)?;
    Ok(true)
}

impl Kept {
    pub fn entry(&self, now: u64) -> Entry {
        let (title, _, area) = crate::backlog_file::split_line(&self.line);
        Entry {
            source: "kept".to_string(),
            title: if title.is_empty() { self.line.clone() } else { title },
            priority: self.priority.clone(),
            area: if self.repo.is_empty() { area } else { self.repo.clone() },
            reference: format!("{PREFIX}{}", self.id),
            summary: String::new(),
            age: format!("{} d", now.saturating_sub(self.at) / 86_400),
            shape: Shape { state: Some(State::ToWrite), kept: true, ..Shape::default() },
        }
    }
}

pub fn answers_in(state: &Path) -> Vec<Answer> {
    read(&answers_file(state)).iter().map(|value| Answer { item: text(value, "item"), question: text(value, "question"), answer: text(value, "answer"), at: value["at"].as_u64().unwrap_or(0) })
        .filter(|answer| !answer.item.is_empty()).collect()
}

fn write_answers(state: &Path, answers: &[Answer]) -> Result<()> {
    let values: Vec<Value> = answers.iter().map(|answer| json!({ "item": answer.item, "question": answer.question, "answer": answer.answer, "at": answer.at })).collect();
    crate::state::write(&answers_file(state), Value::Array(values).to_string())
}

pub fn answer(state: &Path, given: Answer) -> Result<()> {
    let _locked = Locked::take(&answers_file(state))?;
    let mut answers = answers_in(state);
    answers.retain(|other| !(other.item == given.item && other.question == given.question));
    answers.push(given);
    write_answers(state, &answers)
}

pub fn answer_text(who: &str, title: &str, wherein: &str, question: &str, answer: &str) -> String {
    format!("{who} answered an open question on the backlog item \"{title}\" ({wherein}). Question: \"{question}\" Answer: \"{answer}\". Fold the answer into the item with the skill `{}` and take the question out.", crate::backlog_file::SKILL)
}

// An answer leaves once its question is gone from the item: the lead has folded it in.
pub fn settle(state: &Path, item: &str, body: &str) -> Vec<Answer> {
    settle_all(state, &[(item, body)]).into_iter().filter(|answer| answer.item == item).collect()
}

// The same for the whole backlog, given each item's body; an item not listed, or one read without its Open questions heading (half written as it was read), keeps its answers. Every answer still with the lead comes back.
pub fn settle_all(state: &Path, bodies: &[(&str, &str)]) -> Vec<Answer> {
    let open: Vec<(&str, Vec<Question>)> = bodies.iter().filter(|(_, body)| !crate::backlog_file::missing_sections(body).contains(&"Open questions")).map(|(item, body)| (*item, crate::backlog_file::questions_in(body))).collect();
    let still_open = |answer: &Answer| open.iter().find(|(item, _)| *item == answer.item).is_none_or(|(_, questions)| questions.iter().any(|question| question.text == answer.question));
    let (kept, folded): (Vec<Answer>, Vec<Answer>) = answers_in(state).into_iter().partition(still_open);
    if folded.is_empty() {
        return kept;
    }
    let Ok(_locked) = Locked::take(&answers_file(state)) else { return kept };
    let (kept, folded): (Vec<Answer>, Vec<Answer>) = answers_in(state).into_iter().partition(still_open);
    if !folded.is_empty() {
        let _ = write_answers(state, &kept);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_row_lives_until_the_lead_says_it_filed_it() {
        let state = crate::sh::test_dir("backlog-kept");
        let id = keep(&state, "ci push retry flaky (web)", "high", "", 1_000).unwrap();
        let rows = kept_in(&state);
        assert_eq!(rows.len(), 1);
        let entry = rows[0].entry(1_000 + 3 * 86_400);
        assert_eq!((entry.title.as_str(), entry.area.as_str(), entry.source.as_str(), entry.age.as_str()), ("ci push retry flaky", "web", "kept", "3 d"));
        assert_eq!(entry.reference, format!("kept:{id}"));
        assert!(entry.shape.kept && entry.shape.state == Some(State::ToWrite));
        assert!(release(&state, &format!("kept:{id}")).unwrap());
        assert!(kept_in(&state).is_empty() && !release(&state, &id).unwrap(), "released once");
    }

    fn body(questions: &str) -> String {
        format!("## Background\n\nB.\n\n## Goal\n\nG.\n\n## What we know\n\nK.\n\n## Open questions\n\n{questions}\n\n## Out of scope\n\nO.\n\n## Source\n\nS.\n")
    }

    #[test]
    fn an_item_read_half_written_keeps_its_answers() {
        let state = crate::sh::test_dir("backlog-answers-half");
        answer(&state, Answer { item: "local:x".to_string(), question: "Which job?".to_string(), answer: "e2e".to_string(), at: 1 }).unwrap();
        let whole = body("- [ ] Which job?");
        let cut = &whole[..whole.find("## Open questions").unwrap()];
        assert_eq!(settle(&state, "local:x", cut).len(), 1, "read while the lead was writing it, its questions only look gone");
        assert_eq!(answers_in(&state).len(), 1);
        assert!(settle(&state, "local:x", &body("")).is_empty(), "read whole without the question, the answer was folded in");
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn a_note_in_another_shape_with_its_open_questions_folds_its_answers() {
        let state = crate::sh::test_dir("backlog-answers-shape");
        answer(&state, Answer { item: "Projects/old.md".to_string(), question: "Which job?".to_string(), answer: "e2e".to_string(), at: 1 }).unwrap();
        let older = "## Background\n\nB.\n\n## Open questions\n\n- [ ] Which repo?\n";
        assert!(settle(&state, "Projects/old.md", older).is_empty(), "no Goal or Source heading, yet its questions are all there: the answer was folded in");
        assert!(answers_in(&state).is_empty());
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn answers_and_kept_rows_written_at_once_are_all_kept() {
        let state = crate::sh::test_dir("backlog-kept-race");
        std::thread::scope(|scope| {
            for writer in 0..8 {
                let state = &state;
                scope.spawn(move || {
                    for round in 0..10 {
                        answer(state, Answer { item: format!("local:{writer}"), question: format!("Q{round}?"), answer: "a".to_string(), at: 1 }).unwrap();
                        keep(state, &format!("idea {writer} {round}"), "low", "", 1_000).unwrap();
                    }
                });
            }
        });
        assert_eq!((answers_in(&state).len(), kept_in(&state).len()), (80, 80), "no writer's change was lost to another's");
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn an_answer_waits_with_the_lead_until_its_question_leaves_the_item() {
        let state = crate::sh::test_dir("backlog-answers");
        let given = |question: &str| Answer { item: "local:x".to_string(), question: question.to_string(), answer: "Only e2e.".to_string(), at: 1 };
        answer(&state, given("Which job?")).unwrap();
        answer(&state, given("Which job?")).unwrap();
        answer(&state, Answer { item: "local:y".to_string(), ..given("Other?") }).unwrap();
        assert_eq!(settle(&state, "local:x", &body("- [ ] Which job?")).len(), 1, "a second answer to one question replaces the first");
        assert!(settle(&state, "local:x", &body("")).is_empty());
        assert_eq!(answers_in(&state).len(), 1, "folded in, it leaves; another item's answer stays");
    }
}
