use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::backlog::Entry;
use crate::backlog_file::{Question, Shape, State};
use crate::item_template::Template;
use crate::sh::Result;

pub const PREFIX: &str = "kept:";
pub const MERGED_DAYS: u64 = 7;
const DAY: u64 = 86_400;

// What kebbie keeps for the backlog until the lead has acted on it: a short item while the lead files it in the vault, the user's answers to an item's open questions while the lead folds them in, and the items the user asked to have written out while the lead writes them.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub id: String,
    pub line: String,
    pub words: String,
    pub priority: String,
    pub repo: String,
    pub at: u64,
    pub merged: Option<Merged>,
}

// The older note the lead filed the item into: its row stays MERGED_DAYS beside the backlog's newest, pointing at that note, since the note itself is not new.
#[derive(Clone, Debug, PartialEq)]
pub struct Merged {
    pub into: String,
    pub title: String,
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

fn writing_file(state: &Path) -> PathBuf {
    state.join("backlog-writing.json")
}

// Held across each read, change and write of one of the files, so the board, the keeper and the CLI never drop each other's change.
pub(crate) struct Locked(std::fs::File);

impl Locked {
    pub(crate) fn take(file: &Path) -> Result<Locked> {
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

fn merged_of(value: &Value) -> Option<Merged> {
    let into = text(value, "into");
    (!into.is_empty()).then(|| Merged { into, title: text(value, "into_title"), at: value["merged_at"].as_u64().unwrap_or(0) })
}

pub fn kept_in(state: &Path) -> Vec<Kept> {
    read(&kept_file(state)).iter().map(|value| Kept { id: text(value, "id"), line: text(value, "line"), words: text(value, "words"), priority: text(value, "priority"), repo: text(value, "repo"), at: value["at"].as_u64().unwrap_or(0), merged: merged_of(value) })
        .filter(|kept| !kept.id.is_empty()).collect()
}

impl Kept {
    // A row waiting for the lead shows until it is released; a merged one for MERGED_DAYS from the merge.
    pub fn shown_at(&self, now: u64) -> bool {
        self.merged.as_ref().is_none_or(|merged| now.saturating_sub(merged.at) < MERGED_DAYS * DAY)
    }
}

// The rows the backlog shows now; a merged row past its days is gone from it.
pub fn shown_in(state: &Path, now: u64) -> Vec<Kept> {
    kept_in(state).into_iter().filter(|kept| kept.shown_at(now)).collect()
}

fn write_kept(state: &Path, kept: &[Kept]) -> Result<()> {
    let values: Vec<Value> = kept.iter().map(|kept| {
        let mut value = json!({ "id": kept.id, "line": kept.line, "words": kept.words, "priority": kept.priority, "repo": kept.repo, "at": kept.at });
        if let Some(merged) = &kept.merged {
            value["into"] = json!(merged.into);
            value["into_title"] = json!(merged.title);
            value["merged_at"] = json!(merged.at);
        }
        value
    }).collect();
    crate::state::write(&kept_file(state), Value::Array(values).to_string())
}

// The user's own words stay with the row, so an idea the lead never filed is not lost with the message that told it.
pub fn keep(state: &Path, line: &str, words: &str, priority: &str, repo: &str, at: u64) -> Result<String> {
    let _locked = Locked::take(&kept_file(state))?;
    let mut kept = kept_in(state);
    let id = (0u32..).map(|attempt| sha1_smol::Sha1::from(format!("{line}\n{at}\n{attempt}")).digest().to_string()[..6].to_string())
        .find(|id| !kept.iter().any(|other| &other.id == id)).unwrap_or_default();
    kept.retain(|other| other.shown_at(at));
    kept.push(Kept { id: id.clone(), line: line.trim().to_string(), words: words.trim().to_string(), priority: priority.to_string(), repo: repo.to_string(), at, merged: None });
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

// The lead filed the kept item into an existing note: the row stays MERGED_DAYS pointing at it; Ok(false) when kebbie keeps no row by that id.
pub fn merge(state: &Path, id: &str, into: &str, title: &str, now: u64) -> Result<bool> {
    let id = id.strip_prefix(PREFIX).unwrap_or(id);
    let _locked = Locked::take(&kept_file(state))?;
    let mut kept: Vec<Kept> = kept_in(state).into_iter().filter(|other| other.shown_at(now)).collect();
    let Some(row) = kept.iter_mut().find(|other| other.id == id) else { return Ok(false) };
    row.merged = Some(Merged { into: into.to_string(), title: title.to_string(), at: now });
    write_kept(state, &kept)?;
    Ok(true)
}

impl Kept {
    pub fn entry(&self, now: u64) -> Entry {
        let (title, _, area) = crate::backlog_file::split_line(&self.line);
        let title = if title.is_empty() { self.line.clone() } else { title };
        let shape = match &self.merged {
            Some(merged) => Shape { state: Some(State::Ready), kept: true, into: merged.into.clone(), ..Shape::default() },
            None => Shape { state: Some(State::ToWrite), kept: true, ..Shape::default() },
        };
        Entry {
            source: "kept".to_string(),
            title: match &self.merged {
                Some(merged) => format!("{title} · merged into {}", if merged.title.is_empty() { &merged.into } else { &merged.title }),
                None => title,
            },
            priority: self.priority.clone(),
            area: if self.repo.is_empty() { area } else { self.repo.clone() },
            reference: format!("{PREFIX}{}", self.id),
            summary: self.words.clone(),
            age: format!("{} d", now.saturating_sub(self.at) / DAY),
            shape,
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
pub fn settle(state: &Path, item: &str, body: &str, template: &Template) -> Vec<Answer> {
    settle_all(state, &[(item, body, template)]).into_iter().filter(|answer| answer.item == item).collect()
}

// The same for the whole backlog, given each item's body and template; an item not listed, or one read without its template's questions heading (half written as it was read), keeps its answers. Every answer still with the lead comes back.
pub fn settle_all(state: &Path, bodies: &[(&str, &str, &Template)]) -> Vec<Answer> {
    let open: Vec<(&str, Vec<Question>)> = bodies.iter().filter(|(_, body, template)| template.questions.as_deref().is_some_and(|key| template.has(body, key))).map(|(item, body, template)| (*item, template.questions_in(body))).collect();
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

// An item the user asked the lead to write out, and when: it shows writing… from the ask until its text is complete.
#[derive(Clone, Debug, PartialEq)]
pub struct Writing {
    pub item: String,
    pub at: u64,
}

pub fn writing_in(state: &Path) -> Vec<Writing> {
    read(&writing_file(state)).iter().map(|value| Writing { item: text(value, "item"), at: value["at"].as_u64().unwrap_or(0) }).filter(|writing| !writing.item.is_empty()).collect()
}

fn write_writing(state: &Path, writing: &[Writing]) -> Result<()> {
    let values: Vec<Value> = writing.iter().map(|writing| json!({ "item": writing.item, "at": writing.at })).collect();
    crate::state::write(&writing_file(state), Value::Array(values).to_string())
}

pub fn ask_writing(state: &Path, item: &str, at: u64) -> Result<()> {
    let _locked = Locked::take(&writing_file(state))?;
    let mut writing = writing_in(state);
    writing.retain(|other| other.item != item);
    writing.push(Writing { item: item.to_string(), at });
    write_writing(state, &writing)
}

// An ask leaves once its item is written (`done`) or gone from a backlog read whole (`listed`, empty when the read found nothing); every ask still open comes back.
pub fn settle_writing(state: &Path, listed: &[&str], done: &[&str]) -> Vec<Writing> {
    let open = |writing: &Writing| !done.contains(&writing.item.as_str()) && (listed.is_empty() || listed.contains(&writing.item.as_str()));
    let (kept, settled): (Vec<Writing>, Vec<Writing>) = writing_in(state).into_iter().partition(open);
    if settled.is_empty() {
        return kept;
    }
    let Ok(_locked) = Locked::take(&writing_file(state)) else { return kept };
    let (kept, settled): (Vec<Writing>, Vec<Writing>) = writing_in(state).into_iter().partition(open);
    if !settled.is_empty() {
        let _ = write_writing(state, &kept);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_row_lives_until_the_lead_says_it_filed_it() {
        let state = crate::sh::test_dir("backlog-kept");
        let id = keep(&state, "ci push retry flaky (web)", " When a CI push goes red on a flaky test, retry it once\n before it reaches me ", "high", "", 1_000).unwrap();
        let rows = kept_in(&state);
        assert_eq!(rows.len(), 1);
        let entry = rows[0].entry(1_000 + 3 * 86_400);
        assert_eq!((entry.title.as_str(), entry.area.as_str(), entry.source.as_str(), entry.age.as_str()), ("ci push retry flaky", "web", "kept", "3 d"));
        assert_eq!(entry.reference, format!("kept:{id}"));
        assert_eq!(entry.summary, "When a CI push goes red on a flaky test, retry it once\n before it reaches me", "the user's words stay in kebbie, whatever becomes of the message to the lead");
        assert!(entry.shape.kept && entry.shape.state == Some(State::ToWrite));
        assert!(release(&state, &format!("kept:{id}")).unwrap());
        assert!(kept_in(&state).is_empty() && !release(&state, &id).unwrap(), "released once");
    }

    #[test]
    fn a_row_merged_into_an_older_note_stays_seven_days_pointing_at_it_then_goes() {
        let state = crate::sh::test_dir("backlog-kept-merged");
        let filed_at = 1_794_744_000;
        let id = keep(&state, "Native app for the Duo", "Make the phone app native, starting from the Duo", "high", "ios", filed_at).unwrap();
        let merged_at = filed_at + 3_600;
        assert!(merge(&state, &format!("kept:{id}"), "Projects/kebbie-ios.md", "kebbie-ios: native iOS app", merged_at).unwrap());
        let entry = shown_in(&state, merged_at)[0].entry(merged_at);
        assert_eq!((entry.title.as_str(), entry.reference.clone(), entry.shape.into.as_str(), entry.shape.state, entry.age.as_str()), ("Native app for the Duo · merged into kebbie-ios: native iOS app", format!("kept:{id}"), "Projects/kebbie-ios.md", Some(State::Ready), "0 d"));
        let last = merged_at + MERGED_DAYS * DAY - 1;
        assert_eq!(shown_in(&state, last).len(), 1, "still there the last second of its seventh day");
        assert!(shown_in(&state, last + 1).is_empty(), "gone after seven days");
        keep(&state, "Next idea", "", "low", "", last + 1).unwrap();
        assert_eq!(kept_in(&state).iter().map(|kept| kept.line.as_str()).collect::<Vec<_>>(), ["Next idea"], "the next write drops the merged row past its days");
        assert!(!merge(&state, "kept:nope00", "Projects/x.md", "X", last).unwrap());
        let _ = std::fs::remove_dir_all(state);
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
        assert_eq!(settle(&state, "local:x", cut, &Template::builtin()).len(), 1, "read while the lead was writing it, its questions only look gone");
        assert_eq!(answers_in(&state).len(), 1);
        assert!(settle(&state, "local:x", &body(""), &Template::builtin()).is_empty(), "read whole without the question, the answer was folded in");
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn the_half_written_guard_reads_the_templates_own_questions_heading() {
        let state = crate::sh::test_dir("backlog-answers-template");
        let template = Template::parse(r#"{"sections":[{"key":"problem","heading":"Problem","required":true},{"key":"risks","heading":"Risks"}],"questions":"risks"}"#).unwrap();
        answer(&state, Answer { item: "Projects/x.md".to_string(), question: "Which job?".to_string(), answer: "e2e".to_string(), at: 1 }).unwrap();
        assert_eq!(settle(&state, "Projects/x.md", "## Problem\n\nIt broke.\n\n## Open questions\n\n", &template).len(), 1, "no Risks heading yet: half written");
        assert_eq!(settle(&state, "Projects/x.md", "## Problem\n\nIt broke.\n\n## Risks\n\n- [ ] Which job?\n", &template).len(), 1);
        assert!(settle(&state, "Projects/x.md", "## Problem\n\nIt broke.\n\n## Risks\n\n", &template).is_empty(), "under Risks and gone: folded in");
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn a_note_in_another_shape_with_its_open_questions_folds_its_answers() {
        let state = crate::sh::test_dir("backlog-answers-shape");
        answer(&state, Answer { item: "Projects/old.md".to_string(), question: "Which job?".to_string(), answer: "e2e".to_string(), at: 1 }).unwrap();
        let older = "## Background\n\nB.\n\n## Open questions\n\n- [ ] Which repo?\n";
        assert!(settle(&state, "Projects/old.md", older, &Template::builtin()).is_empty(), "no Goal or Source heading, yet its questions are all there: the answer was folded in");
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
                        keep(state, &format!("idea {writer} {round}"), "", "low", "", 1_000).unwrap();
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
        assert_eq!(settle(&state, "local:x", &body("- [ ] Which job?"), &Template::builtin()).len(), 1, "a second answer to one question replaces the first");
        assert!(settle(&state, "local:x", &body(""), &Template::builtin()).is_empty());
        assert_eq!(answers_in(&state).len(), 1, "folded in, it leaves; another item's answer stays");
    }
}
