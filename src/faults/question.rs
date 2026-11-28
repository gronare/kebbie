use std::path::Path;

use serde_json::json;

use super::Case;
use crate::messages::{self, Lock, Message};
use crate::permit::Caller;
use crate::sh::Result;

pub const QUESTION_KIND: &str = "fault";
const FROM: &str = "kebbie";

fn about(case: &str) -> String {
    format!("{QUESTION_KIND}:{case}")
}

pub fn case_of(about: Option<&str>) -> Option<&str> {
    about?.strip_prefix(QUESTION_KIND)?.strip_prefix(':')
}

// The watcher's question, checked as a task's kebbie ask is, so a question the board cannot show goes back to the lead instead.
pub fn asked(question: &str, options: &[String], recommend: Option<&str>) -> Result<crate::questions::Asked> {
    let options: Vec<&str> = options.iter().map(String::as_str).collect();
    crate::questions::parse(&options, recommend, false, question).map_err(|error| format!("its question cannot go on the board: {error}"))
}

pub fn put(case: &Case, asked: &crate::questions::Asked) -> Result<String> {
    let id = messages::new_id(QUESTION_KIND);
    let mut record = crate::questions::record(&id, messages::now(), FROM, asked, None, None);
    record["kind"] = json!(QUESTION_KIND);
    record["about"] = json!(about(&case.id));
    {
        let _lock = Lock::take(true)?;
        messages::append(&record)?;
    }
    crate::hub::notices::send(&crate::questions::question_notice(FROM, asked, false), None);
    Ok(id)
}

// The user's answer goes back to the watcher as the same case, with the question and the answer beside its words, and is weighed at once.
pub fn answer_question(message: &Message, choice: Option<usize>, comment: &str, caller: &Caller) -> Result<String> {
    let case = case_of(message.about.as_deref()).ok_or("That question names no fault watcher case")?.to_string();
    let text = crate::questions::answer_text(message, choice, comment)?;
    let state = crate::lead::state_dir()?;
    {
        let _lock = Lock::take(true)?;
        messages::append(&json!({ "id": message.id, "at": messages::now(), "state": "answered" }))?;
        let place = if matches!(caller, Caller::Hub { .. }) { "from the phone" } else { "on the board" };
        messages::copy_to_lead_locked(&format!("The user answered the fault watcher's question {place}: {text} The watcher weighs the case again with it, and its verdict comes to you."), None, &crate::peers::sessions().unwrap_or_default())?;
    }
    crate::hub::notices::settle(crate::hub::notices::Subject::Question, FROM);
    answered_in(&state, &case, &message.text, &text, messages::now())?;
    crate::notice::Notice::new(crate::notice::Tone::Done, "Answered").next("The fault watcher weighs it again with your answer.").ok()
}

pub fn answered_in(state: &Path, case: &str, question: &str, answer: &str, now: u64) -> Result<String> {
    super::add_answered(state, case, &format!("The fault watcher asked the user: {}\n{answer}", crate::plan_brief::title(question)), now)
}
