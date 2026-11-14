use std::fs;
use std::path::{Path, PathBuf};

use kebbie_hub_protocol::{CLOSE_OPTIONS, KebbieQuestion, KebbieQuestionKind, MAX_KEBBIE_QUESTION_CHARS, MAX_KEBBIE_QUESTIONS, QuestionOption, RequirementKind, Stage, VERIFY_OPTIONS};
use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::messages::{self, Message, Question, USER_LABEL};
use crate::permit::Caller;
use crate::sh::Result;

// kebbie's own questions: a stage's verification, a check only the user can make, and whether to close a task kebbie would otherwise leave to be closed by hand. They sit beside the task's own question and never hold it or its messages.
pub const ID_PREFIX: &str = "kq-";
pub const VERIFY_KIND: &str = "kebbie-verify";
pub const CLOSE_KIND: &str = "kebbie-close";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Verify,
    Close,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Verify => "verify",
            Kind::Close => "close",
        }
    }

    fn labels(self) -> &'static [&'static str] {
        match self {
            Kind::Verify => &VERIFY_OPTIONS,
            Kind::Close => &CLOSE_OPTIONS,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asked {
    pub id: String,
    pub address: String,
    pub kind: Kind,
    pub text: String,
    pub stage: Option<String>,
    pub at: u64,
}

fn file(state: &Path) -> PathBuf {
    state.join("kebbie-questions.json")
}

fn lock(state: &Path) -> PathBuf {
    state.join("kebbie-questions.lock")
}

fn of_json(value: &Value) -> Option<Asked> {
    let kind = match value["kind"].as_str()? {
        "verify" => Kind::Verify,
        "close" => Kind::Close,
        _ => return None,
    };
    Some(Asked { id: value["id"].as_str()?.to_string(), address: value["address"].as_str()?.to_string(), kind, text: value["text"].as_str()?.to_string(), stage: value["stage"].as_str().map(str::to_string), at: value["at"].as_u64().unwrap_or_default() })
}

fn to_json(asked: &Asked) -> Value {
    json!({ "id": asked.id, "address": asked.address, "kind": asked.kind.word(), "text": asked.text, "stage": asked.stage, "at": asked.at })
}

pub fn load(state: &Path) -> Vec<Asked> {
    let Ok(text) = fs::read_to_string(file(state)) else { return Vec::new() };
    serde_json::from_str::<Value>(&text).ok().and_then(|value| value.as_array().map(|list| list.iter().filter_map(of_json).collect())).unwrap_or_default()
}

fn save(state: &Path, all: &[Asked]) -> Result<()> {
    crate::state::write(&file(state), Value::from(all.iter().map(to_json).collect::<Vec<_>>()).to_string())
}

fn changed<T>(state: &Path, change: impl FnOnce(&mut Vec<Asked>) -> T) -> Result<T> {
    let _held = crate::state::Locked::wait(&lock(state))?;
    let mut all = load(state);
    let before = all.clone();
    let out = change(&mut all);
    if all != before {
        save(state, &all)?;
    }
    Ok(out)
}

pub fn is_ours(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

fn new_id(address: &str, kind: Kind, text: &str, at: u64) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{address}\n{}\n{text}\n{at}", kind.word()).as_bytes());
    format!("{ID_PREFIX}{}", digest.iter().take(6).map(|byte| format!("{byte:02x}")).collect::<String>())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_KEBBIE_QUESTION_CHARS).collect()
}

// A question of the same kind about the same stage takes the place of the earlier one; a task holds the protocol's four at most, the oldest giving way.
fn put(state: &Path, address: &str, kind: Kind, text: &str, stage: Option<&str>) -> Result<Asked> {
    let at = messages::now();
    let text = one_line(text);
    if text.is_empty() {
        return Err("kebbie's question needs its text".to_string());
    }
    let asked = Asked { id: new_id(address, kind, &text, at), address: address.to_string(), kind, text, stage: stage.map(str::to_string), at };
    changed(state, |all| {
        all.retain(|one| !(one.address == asked.address && one.kind == asked.kind && one.stage == asked.stage && (asked.stage.is_some() || one.kind == Kind::Close || one.text == asked.text)));
        while all.iter().filter(|one| one.address == asked.address).count() >= MAX_KEBBIE_QUESTIONS {
            let Some(at) = all.iter().position(|one| one.address == asked.address) else { break };
            all.remove(at);
        }
        all.push(asked.clone());
    })?;
    let repo = crate::repo::pooled_checkouts().into_iter().find(|repo| crate::repo::name_of(repo) == asked.address.split_once('@').map_or("", |(_, place)| place));
    if let Some(repo) = repo {
        crate::notify::send(&repo, &crate::notify::Notice::from(crate::notify::Event::Question, &asked.address, format!("{}: {}", asked.address, asked.text)));
    }
    crate::hub::notices::send(&crate::hub::notices::About::Question { address: asked.address.clone(), text: asked.text.clone(), recommended: None }, None);
    Ok(asked)
}

pub fn ask_verify(state: &Path, address: &str, text: &str, stage: Option<&str>) -> Result<Asked> {
    put(state, address, Kind::Verify, text, stage)
}

pub fn ask_close(state: &Path, address: &str, text: &str) -> Result<Asked> {
    put(state, address, Kind::Close, text, None)
}

// kebbie ask --verify from a task: a check only the user can make, asked beside the task's own question so the task builds on.
pub fn ask_from_task(cwd: &Path, text: &str) -> Result<String> {
    let me = crate::peers::me(cwd).filter(|me| me != crate::status::LEAD_LABEL).ok_or("kebbie ask --verify runs in a task's worktree")?;
    let text = one_line(text);
    if text.is_empty() {
        return Err("kebbie ask --verify needs what the user checks and where: kebbie ask --verify \"<what to check, and where>\"".to_string());
    }
    put(&state_dir()?, &me, Kind::Verify, &text, None)?;
    Ok(format!(
        "Asked the user to verify: {text}\nThey answer Verified, Not yet or Something is wrong; the answer reaches you as a message from kebbie. This question holds nothing: go on with the work it does not decide, or report blocked when everything left waits on it."
    ))
}

pub fn settle_stages(state: &Path, address: &str, stages: &[String]) {
    let _ = changed(state, |all| all.retain(|one| !(one.address == address && one.stage.as_ref().is_some_and(|stage| stages.contains(stage)))));
}

// Once its task is closed or dropped, nothing kebbie asked about it waits.
pub fn forget(address: &str) {
    if let Ok(state) = state_dir() {
        let _ = changed(&state, |all| all.retain(|one| one.address != address));
    }
}

fn meanings(asked: &Asked, stages: &[Stage]) -> Vec<String> {
    let stage = asked.stage.as_deref().and_then(|id| stages.iter().find(|stage| stage.id == id));
    let next = stage.and_then(|stage| stages.iter().skip_while(|one| one.id != stage.id).nth(1));
    match (asked.kind, stage) {
        (Kind::Close, _) => vec!["kebbie closes the task as done".to_string(), "The task stays open, and this question goes".to_string()],
        (Kind::Verify, Some(stage)) => vec![
            match next {
                Some(next) => format!("{} is met; {} can be pushed", stage.title, next.title),
                None => format!("{} is met", stage.title),
            },
            "Nothing changes; this question waits".to_string(),
            format!("Tell the task what you see; the stages after {} stay locked", stage.title),
        ],
        (Kind::Verify, None) => vec!["The task hears that it holds".to_string(), "Nothing changes; this question waits".to_string(), "Tell the task what you see".to_string()],
    }
}

// The question as the phone gets it; a stage it names goes along only when the stage waits on the user's verification, as the protocol takes.
pub fn wire(asked: &Asked, stages: &[Stage]) -> KebbieQuestion {
    let options = asked.kind.labels().iter().zip(meanings(asked, stages)).enumerate().map(|(at, (label, meaning))| QuestionOption { label: label.to_string(), meaning, comment: asked.kind == Kind::Verify && at == 2 }).collect();
    let stage = asked.stage.clone().filter(|id| stages.iter().any(|stage| stage.id == *id && stage.requirement.kind == RequirementKind::Verified && stage.state == kebbie_hub_protocol::StageState::Waiting));
    KebbieQuestion { id: asked.id.clone(), kind: match asked.kind { Kind::Verify => KebbieQuestionKind::Verify, Kind::Close => KebbieQuestionKind::Close }, text: asked.text.clone(), options, stage, at: asked.at }
}

// The questions as the board's inbox reads them, beside the tasks' own.
pub fn as_messages(state: &Path) -> Vec<Message> {
    load(state).into_iter().map(|asked| {
        let options = asked.kind.labels().iter().zip(meanings(&asked, &[])).map(|(label, meaning)| (label.to_string(), meaning)).collect();
        let comment = if asked.kind == Kind::Verify { vec![2] } else { Vec::new() };
        Message {
            id: asked.id.clone(), at: asked.at, from: asked.address.clone(), to: USER_LABEL.to_string(), text: asked.text.clone(), held: false, report: None, ended: false,
            question: Some(Question { options, recommend: 0, plan: None, plan_at: None, comment, answered: false, waits: false, relayed: None }),
            about: asked.stage.clone(), kind: Some(match asked.kind { Kind::Verify => VERIFY_KIND, Kind::Close => CLOSE_KIND }.to_string()), landed: None, why: None, forced: false, fyi: false,
        }
    }).collect()
}

#[cfg(test)]
pub fn of(state: &Path, address: &str) -> Vec<Asked> {
    load(state).into_iter().filter(|asked| asked.address == address).collect()
}

// The user's answer, at the board or from the phone: Verified meets a stage or tells the task it holds, Not yet changes nothing, anything else fails the stage or goes to the task in the user's words; Close closes the task, Not yet lets it be.
pub fn answer_as(caller: &Caller, id: &str, choice: Option<usize>, comment: &str) -> std::result::Result<String, String> {
    answer_in(&state_dir()?, caller, id, choice, comment, &close)
}

// What Close does to the task the question names: done, as d on the board.
pub type Closer<'a> = &'a dyn Fn(&str) -> std::result::Result<String, String>;

pub fn answer_in(state: &Path, caller: &Caller, id: &str, choice: Option<usize>, comment: &str, close: Closer) -> std::result::Result<String, String> {
    if !matches!(caller, Caller::User | Caller::Hub { .. }) {
        return Err(format!("answering kebbie's question is the user's call; {} cannot", crate::permit::describe(caller)));
    }
    let asked = load(state).into_iter().find(|asked| asked.id == id).ok_or("That question no longer waits for an answer")?;
    let comment = comment.split_whitespace().collect::<Vec<_>>().join(" ");
    if choice.is_some_and(|choice| choice >= asked.kind.labels().len()) {
        return Err(format!("{} takes {}", asked.text, asked.kind.labels().join(", ")));
    }
    if choice.is_none() && comment.is_empty() {
        return Err("Write your answer in the comment field first.".to_string());
    }
    let keeps = choice == Some(1) && asked.kind == Kind::Verify;
    // Taken off before its action so a second answer finds nothing, and put back when the action is refused so it can be answered again.
    if !keeps {
        changed(state, |all| all.retain(|one| one.id != id))?;
        crate::hub::notices::settle(crate::hub::notices::Subject::Question, &asked.address);
    }
    let acted = (|| -> std::result::Result<String, String> { Ok(match (asked.kind, choice) {
        (Kind::Verify, Some(1)) => format!("Nothing changes; the question waits for {}.", asked.address),
        (Kind::Verify, Some(0)) => match &asked.stage {
            Some(stage) => crate::tasks::rollout::verified(state, &asked.address, stage, messages::now())?,
            None => {
                tell(&asked.address, &format!("The user verified your check: {}. It holds.", asked.text));
                format!("Told {} that it holds.", asked.address)
            }
        },
        (Kind::Verify, _) => {
            let what = if comment.is_empty() { "something is wrong".to_string() } else { comment.clone() };
            match &asked.stage {
                Some(stage) => {
                    crate::tasks::rollout::failed(state, &asked.address, std::slice::from_ref(stage), &format!("you said: {what}"), true);
                    format!("Told {} what you saw; the stages after it stay locked.", asked.address)
                }
                None => {
                    tell(&asked.address, &format!("The user checked \"{}\" and it does not hold: {what}", asked.text));
                    format!("Told {} what you saw.", asked.address)
                }
            }
        }
        (Kind::Close, Some(0)) => close(&asked.address)?,
        (Kind::Close, Some(_)) => format!("{} stays open.", asked.address),
        (Kind::Close, None) => {
            tell(&asked.address, &format!("The user, asked whether to close this task: {comment}"));
            format!("{} stays open; told it what you said.", asked.address)
        }
    }) })();
    let said = match acted {
        Ok(said) => said,
        Err(error) => {
            if !keeps {
                changed(state, |all| if !all.iter().any(|one| one.id == asked.id) { all.push(asked.clone()) })?;
            }
            return Err(error);
        }
    };
    if let Err(error) = messages::to_lead(messages::ToLead::QuestionCopy, &format!("The user answered kebbie's question on {} ({}): {}", asked.address, asked.text, said)) {
        eprintln!("Could not tell the lead: {error}");
    }
    Ok(said)
}

fn tell(address: &str, text: &str) {
    if let Err(error) = messages::hand_to(address, text) {
        eprintln!("Could not tell {address}: {error}");
    }
}

pub fn close(address: &str) -> std::result::Result<String, String> {
    let (task, place) = address.split_once('@').unwrap_or((address, ""));
    let repo = crate::repo::pooled_checkouts().into_iter().find(|repo| crate::repo::name_of(repo) == place).ok_or_else(|| format!("No repo {place} on this machine"))?;
    crate::tasks::done(&repo, Some(task))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-kq-{}-{}", std::process::id(), messages::new_id("t")));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_question_of_the_same_kind_about_the_same_stage_replaces_the_earlier_and_a_task_holds_four_at_most() {
        let state = state();
        put(&state, "billing@app", Kind::Verify, "Has the backfill gone through?", Some("backfill")).unwrap();
        put(&state, "billing@app", Kind::Verify, "Has the backfill gone through, all of it?", Some("backfill")).unwrap();
        assert_eq!(load(&state).iter().map(|one| one.text.as_str()).collect::<Vec<_>>(), ["Has the backfill gone through, all of it?"]);
        for at in 0..5 {
            put(&state, "billing@app", Kind::Verify, &format!("check {at}"), None).unwrap();
        }
        assert_eq!(of(&state, "billing@app").len(), MAX_KEBBIE_QUESTIONS, "the oldest give way");
        assert!(load(&state).iter().all(|one| is_ours(&one.id) && one.id.len() <= kebbie_hub_protocol::MAX_KEBBIE_QUESTION_ID_CHARS));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_board_reads_kebbies_questions_with_their_fixed_answers_and_the_phone_gets_a_stage_only_while_it_waits_on_the_user() {
        let state = state();
        let asked = put(&state, "billing@app", Kind::Verify, "Backfill is out. Has it gone through?", Some("backfill")).unwrap();
        let shown = as_messages(&state);
        let question = shown[0].question.as_ref().unwrap();
        assert_eq!(question.options.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(), VERIFY_OPTIONS);
        assert_eq!((question.comment.as_slice(), shown[0].kind.as_deref()), (&[2][..], Some(VERIFY_KIND)), "Something is wrong takes what the user saw");
        let stage = |state| Stage { id: "backfill".into(), title: "Backfill".into(), state, requirement: kebbie_hub_protocol::Requirement { kind: RequirementKind::Verified, detail: Some("gone through".into()) }, through: None, pushed_at: None, met_at: None, note: None };
        assert_eq!(wire(&asked, &[stage(kebbie_hub_protocol::StageState::Waiting)]).stage.as_deref(), Some("backfill"));
        assert_eq!(wire(&asked, &[stage(kebbie_hub_protocol::StageState::Pushed)]).stage, None);
        let closing = put(&state, "billing@app", Kind::Close, "billing says it is done. Close it?", None).unwrap();
        assert_eq!(wire(&closing, &[]).options.iter().map(|option| (option.label.as_str(), option.comment)).collect::<Vec<_>>(), [("Close", false), ("Not yet", false)]);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_close_the_task_refuses_leaves_the_question_to_answer_again() {
        let state = state();
        let asked = put(&state, "billing@app", Kind::Close, "billing says it is done. Close it?", None).unwrap();
        let refused = answer_in(&state, &Caller::User, &asked.id, Some(0), "", &|_| Err("Not closed: its note lacks its Solution".to_string()));
        assert_eq!(refused.unwrap_err(), "Not closed: its note lacks its Solution");
        assert_eq!(load(&state).iter().map(|one| one.id.as_str()).collect::<Vec<_>>(), [asked.id.as_str()], "nothing else asks again, so the question stays");
        assert!(answer_in(&state, &Caller::User, &asked.id, Some(0), "", &|address| Ok(format!("Closed {address}"))).is_ok());
        assert!(load(&state).is_empty(), "answered once it closed");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn only_the_user_answers_kebbies_question() {
        assert!(answer_as(&Caller::Lead, "kq-1", Some(0), "").unwrap_err().contains("the user's call"));
        assert!(answer_as(&Caller::Task("billing@app".into()), "kq-1", Some(0), "").unwrap_err().contains("the user's call"));
    }
}
