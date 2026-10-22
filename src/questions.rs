use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::messages::{self, Lock, Message, Question, Report, USER_LABEL};
use crate::peers;
use crate::report;
use crate::sh::Result;
use crate::status::LEAD_LABEL;

const USAGE: &str = "kelpie ask --option \"<label>: <what it means>\" --option \"<label>: <what it means>\" [--option …] --recommend <n> \"<question>\", where an option ending in \" +comment\" takes the user's written comment, or kelpie ask --plan <vault:<note path> | file> [\"<question>\"]";

pub const COMMENT_MARK: &str = "+comment";

pub const VAULT_PLAN: &str = "vault:";

pub const PLAN_OPTIONS: [(&str, &str); 4] = [
    ("Approve", "build it as planned"),
    ("Approve with a comment", "build it, taking the user's comment into account"),
    ("Ask for changes", "revise the plan as the user says and ask again"),
    ("Reject with a comment", "drop this approach, for the reason the user gives"),
];

#[derive(Debug, PartialEq)]
pub struct Asked {
    pub text: String,
    pub options: Vec<(String, String)>,
    pub recommend: usize,
    pub comment: Vec<usize>,
}

pub const PLAN_COMMENT: [usize; 3] = [1, 2, 3];

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse(options: &[&str], recommend: Option<&str>, plan: bool, text: &str) -> Result<Asked> {
    let text = one_line(text);
    if plan {
        if !options.is_empty() || recommend.is_some() {
            return Err("kelpie ask --plan takes the plan's own options (approve, approve with a comment, ask for changes, reject with a comment); leave out --option and --recommend".to_string());
        }
        let text = if text.is_empty() { "Approve this plan?".to_string() } else { text };
        return Ok(Asked { text, options: PLAN_OPTIONS.iter().map(|(label, meaning)| (label.to_string(), meaning.to_string())).collect(), recommend: 1, comment: PLAN_COMMENT.to_vec() });
    }
    if text.is_empty() {
        return Err(format!("kelpie ask needs the question: {USAGE}"));
    }
    if !(2..=4).contains(&options.len()) {
        return Err(format!("kelpie ask takes two to four --option, not {}: {USAGE}", options.len()));
    }
    let marked: Vec<(&str, bool)> = options.iter().map(|option| match option.trim_end().strip_suffix(COMMENT_MARK) {
        Some(rest) if rest.ends_with(char::is_whitespace) => (rest, true),
        _ => (*option, false),
    }).collect();
    let comment = marked.iter().enumerate().filter(|(_, (_, takes))| *takes).map(|(index, _)| index).collect();
    let options = marked.iter().map(|(option, _)| match option.split_once(':') {
        Some((label, meaning)) if !one_line(label).is_empty() && !one_line(meaning).is_empty() => Ok((one_line(label), one_line(meaning))),
        _ => Err(format!("--option \"{option}\" needs a label and what it means, as \"<label>: <what it means>\", with \" {COMMENT_MARK}\" after it when the option takes the user's comment")),
    }).collect::<Result<Vec<_>>>()?;
    let recommend = recommend.ok_or(format!("kelpie ask needs --recommend <n>, the option you recommend: {USAGE}"))?;
    let recommend: usize = recommend.trim().parse().ok().filter(|n| (1..=options.len()).contains(n))
        .ok_or(format!("--recommend is the number of an option, 1 to {}, not \"{recommend}\"", options.len()))?;
    Ok(Asked { text, options, recommend, comment })
}

// A note's mtime in whole milliseconds: an f64 read back from JSON can differ in its last digit, which would show as a change.
pub fn millis(seconds: f64) -> i64 {
    (seconds * 1000.0).round() as i64
}

// The question's own log line; the task's report beside it puts the task under Needs you.
pub fn record(id: &str, at: u64, from: &str, asked: &Asked, plan: Option<&str>, plan_at: Option<i64>) -> Value {
    let options: Vec<Value> = asked.options.iter().enumerate().map(|(index, (label, meaning))| json!({ "label": label, "meaning": meaning, "comment": asked.comment.contains(&index) })).collect();
    json!({
        "id": id, "at": at, "from": from, "to": USER_LABEL, "text": asked.text, "state": "asked",
        "question": { "options": options, "recommend": asked.recommend, "plan": plan, "plan_at_ms": plan_at },
    })
}

// The task may change or delete its plan file; the user reads the plan as it was asked. kelpie's own plans/ file is moved rather than copied, so it is stored once.
pub fn keep_plan(source: &Path, state: &Path, me: &str, id: &str) -> Result<PathBuf> {
    let dir = state.join("plans");
    crate::state::private_dir(&dir)?;
    let kept = dir.join(format!("{me}-{id}.md"));
    let own = source.parent().and_then(|parent| parent.canonicalize().ok()) == dir.canonicalize().ok();
    if own {
        fs::rename(source, &kept).map_err(|error| format!("{}: {error}", source.display()))?;
    } else {
        crate::state::write(&kept, fs::read(source).map_err(|error| format!("{}: {error}", source.display()))?)?;
    }
    Ok(kept)
}

// "vault:Projects/x" names the note Projects/x.md; anything else is a file, kept as a copy.
pub fn plan_reference(plan: &str, cwd: &Path, state: &Path, me: &str, id: &str) -> Result<String> {
    let plan = plan.trim();
    match plan.strip_prefix(VAULT_PLAN) {
        Some(key) => {
            let key = key.trim().trim_start_matches('/');
            if key.is_empty() {
                return Err("kelpie ask --plan vault:<path> needs the note's path in the vault, as vault:Projects/<note>.md".to_string());
            }
            Ok(format!("{VAULT_PLAN}{key}{}", if key.ends_with(".md") { "" } else { ".md" }))
        }
        None => keep_plan(&cwd.join(plan), state, me, id).map(|copy| copy.display().to_string()),
    }
}

// A vault plan is the living note, so the board says when it changed after it was asked; a file plan is kept as asked and never changes.
pub fn plan_changed(question: &Question) -> bool {
    let Ok(index) = crate::vault::Index::open() else { return false };
    plan_changed_in(question, |key| index.changed_at(key))
}

fn plan_changed_in(question: &Question, changed_at: impl Fn(&str) -> Option<f64>) -> bool {
    let Some(key) = question.plan.as_deref().and_then(|plan| plan.strip_prefix(VAULT_PLAN)) else { return false };
    matches!((question.plan_at, changed_at(key).map(millis)), (Some(asked), Some(now)) if now != asked)
}

// The plan's markdown: a vault note's body from the vault index, or the copy of the file it was asked with.
pub fn plan_text(question: &Question) -> Option<String> {
    let plan = question.plan.as_deref()?;
    match plan.strip_prefix(VAULT_PLAN) {
        Some(key) => crate::vault::Index::open().ok()?.body(key),
        None => fs::read_to_string(plan).ok(),
    }
}

// Questions waiting on the user, oldest first: a task's later question, its end, its answer, or a new turn of it settles one.
pub fn open(messages: &[Message], turn_started: impl Fn(&str) -> Option<u64>) -> Vec<&Message> {
    let mut open: Vec<&Message> = Vec::new();
    for message in messages {
        if message.ended {
            open.retain(|question| question.from != message.to);
        } else if let Some(question) = &message.question {
            open.retain(|earlier| earlier.from != message.from || earlier.about != message.about);
            if !question.answered {
                open.push(message);
            }
        }
    }
    open.retain(|question| turn_started(&question.from).is_none_or(|at| at <= question.at));
    open
}

pub fn open_now(messages: &[Message]) -> Vec<&Message> {
    open(messages, report::turn_started)
}

pub fn text(message: &Message) -> String {
    let Some(question) = &message.question else { return message.text.clone() };
    let mut lines = vec![message.text.clone()];
    for (index, (label, meaning)) in question.options.iter().enumerate() {
        let recommended = if index + 1 == question.recommend { " (recommended)" } else { "" };
        lines.push(format!("{}. {label}{recommended}: {meaning}{}", index + 1, takes(&question.comment, index)));
    }
    if let Some(plan) = &question.plan {
        lines.push(format!("plan: {plan}"));
    }
    lines.join("\n")
}

const TAKES_COMMENT: &str = " (with the user's comment)";

fn takes(comment: &[usize], index: usize) -> &'static str {
    if comment.contains(&index) { TAKES_COMMENT } else { "" }
}

fn options_line(question: &Question) -> String {
    question.options.iter().enumerate()
        .map(|(index, (label, meaning))| format!("{label}{}: {meaning}{}", if index + 1 == question.recommend { " (recommended)" } else { "" }, takes(&question.comment, index)))
        .collect::<Vec<_>>().join("; ")
}

// The lead's copy of a new question: it settles a method question itself, and while the user is away keeps one of intent for them.
pub fn question_copy(message: &Message, away: bool) -> String {
    let Some(question) = &message.question else { return message.text.clone() };
    let plan = question.plan.as_ref().map(|plan| format!(" The plan: {plan}.")).unwrap_or_default();
    let then = if away {
        format!(" The user is away: settle a question of method with kelpie decide {from} \"<answer>\", and keep one of intent for them with kelpie defer {from}.", from = message.from)
    } else {
        format!(" A question of method you settle with kelpie decide {from} \"<answer>\"; one of intent the user answers on the board.", from = message.from)
    };
    format!("{} asks the user: {} Options: {}.{plan}{then}", message.from, message.text, options_line(question))
}

pub fn answer_text(message: &Message, choice: Option<usize>, comment: &str) -> Result<String> {
    let question = message.question.as_ref().ok_or("That message is not a question")?;
    let comment = comment.trim();
    match choice {
        Some(index) => {
            let (label, meaning) = question.options.get(index).ok_or_else(|| format!("The question has {} options; there is no option {}", question.options.len(), index + 1))?;
            if comment.is_empty() && question.comment.contains(&index) {
                return Err(format!("{label} takes the user's comment; write it before sending"));
            }
            let comment = if comment.is_empty() { String::new() } else { format!(" Their comment: {comment}") };
            Ok(format!("The user's answer to \"{}\": {label} ({meaning}).{comment}", message.text))
        }
        None if comment.is_empty() => Err("An answer needs an option or the user's own words".to_string()),
        None => Ok(format!("The user's answer to \"{}\", in their own words: {comment}", message.text)),
    }
}

fn answer_copy(message: &Message, choice: Option<usize>, comment: &str) -> String {
    let picked = choice.and_then(|index| message.question.as_ref()?.options.get(index)).map(|(label, _)| label.clone());
    let comment = comment.trim();
    let answered = match (picked, comment.is_empty()) {
        (Some(label), true) => label,
        (Some(label), false) => format!("{label}, with the comment: {comment}"),
        (None, _) => format!("in their own words: {comment}"),
    };
    format!("{} asked \"{}\"; the user answered on the board: {answered}", message.from, message.text)
}

// The board's answer to a question: the task gets it as a message from the user, the question closes, and the lead gets a one-line copy.
pub fn answer(id: &str, choice: Option<usize>, comment: &str) -> Result<String> {
    let sessions = peers::sessions().unwrap_or_default();
    let _lock = Lock::take(true)?;
    let messages = messages::load();
    let open = open_now(&messages);
    let Some(message) = open.iter().copied().find(|question| question.id == id) else {
        return answer_late(&messages, id, choice, comment, &sessions);
    };
    if message.kind.as_deref() == Some(crate::restart::QUESTION_KIND) {
        let session = message.about.clone().ok_or("That restart question names no session")?;
        let yes = choice == Some(0);
        if choice.is_none() {
            return Err("Answer the restart question with yes or no".to_string());
        }
        messages::append(&json!({ "id": id, "at": messages::now(), "state": "answered" }))?;
        drop(_lock);
        return crate::restart::answer(&session, yes);
    }
    let text = answer_text(message, choice, comment)?;
    let told = match sessions.iter().find(|session| session.address == message.from) {
        Some(task) => messages::send_locked(USER_LABEL, &[task], &text, &sessions, Some(id), Some("answer"))?.join(" "),
        None => {
            messages::hold_for(USER_LABEL, &message.from, &text, Some(id), Some("answer"))?;
            messages::append(&json!({ "id": id, "at": messages::now(), "state": "answered" }))?;
            format!("{} has no session now; held, and delivered when it runs.", message.from)
        }
    };
    messages::copy_to_lead_locked(&answer_copy(message, choice, comment), None, &sessions)?;
    Ok(format!("Answered {}'s question. {told}", message.from))
}

const CLOSED_BEFORE: &str = "(The question had already closed when the user answered it on the board, since a new turn started in your pane; act on it if it still applies.)";

// A task's question nobody has answered yet, though it no longer waits on the board; a restart question's answer means nothing once it has closed.
fn closed_unanswered<'a>(messages: &'a [Message], id: &str) -> Option<&'a Message> {
    messages.iter().find(|message| message.id == id && message.question.as_ref().is_some_and(|question| !question.answered))
        .filter(|message| message.kind.as_deref() != Some(crate::restart::QUESTION_KIND))
}

// The caller holds the lock. A turn that starts in the task's pane closes its question, so an answer given on the board a moment later is delivered as the user's word rather than lost.
fn answer_late(messages: &[Message], id: &str, choice: Option<usize>, comment: &str, sessions: &[peers::Session]) -> Result<String> {
    let message = closed_unanswered(messages, id).ok_or("That question no longer waits for an answer")?;
    let text = format!("{} {CLOSED_BEFORE}", answer_text(message, choice, comment)?);
    let told = match sessions.iter().find(|session| session.address == message.from) {
        Some(task) => messages::send_locked(USER_LABEL, &[task], &text, sessions, None, Some("answer"))?.join(" "),
        None => {
            messages::hold_for(USER_LABEL, &message.from, &text, None, Some("answer"))?;
            format!("{} has no session now; held, and delivered when it runs.", message.from)
        }
    };
    messages::append(&json!({ "id": id, "at": messages::now(), "state": "answered" }))?;
    messages::copy_to_lead_locked(&answer_copy(message, choice, comment), None, sessions)?;
    Ok(format!("Answered {}'s question, which had closed when a turn started in its pane; your answer goes to it as a message. {told}", message.from))
}

pub fn ask(cwd: &Path, options: &[&str], recommend: Option<&str>, plan: Option<&str>, text: &str) -> Result<String> {
    let asked = parse(options, recommend, plan.is_some(), text)?;
    let me = peers::me(cwd).filter(|me| me != LEAD_LABEL).ok_or("kelpie ask runs in a task's worktree")?;
    let sessions = peers::sessions().unwrap_or_default();
    let _lock = Lock::take(true)?;
    if open_now(&messages::load()).iter().any(|question| question.from == me) {
        return Err("Your earlier question still waits for the user; one question at a time, so stop your turn and wait for the answer.".to_string());
    }
    let at = messages::now();
    let plan = plan.map(|plan| plan_reference(plan, cwd, &state_dir()?, &me, &messages::new_id("plan"))).transpose()?;
    let next = if plan.is_some() { format!("read and approve the plan: {}", asked.text) } else { format!("answer: {}", asked.text) };
    let report = Report { state: "needs-you".to_string(), next };
    messages::log_report(&me, &report, &format!("asked the user: {}", asked.text))?;
    report::show_in_sidebar(&report);
    let id = messages::new_id("question");
    let plan_at = plan.as_deref().and_then(|plan| plan.strip_prefix(VAULT_PLAN)).and_then(|key| crate::vault::Index::open().ok()?.changed_at(key)).map(millis);
    let record = record(&id, at, &me, &asked, plan.as_deref(), plan_at);
    messages::append(&record)?;
    let message = messages::fold(&record.to_string()).remove(0);
    messages::copy_to_lead_locked(&question_copy(&message, crate::permit::away_since().is_some()), Some(&id), &sessions)?;
    let (event, said) = if crate::routines::found(&me, &asked.text) { (crate::notify::Event::RoutineFindings, "found and asks") } else { (crate::notify::Event::Question, "asks") };
    crate::notify::send(cwd, &crate::notify::Notice::from(event, &me, format!("{me} {said}: {}", asked.text)));
    Ok(format!(
        "Asked the user; the question waits for them on the board:\n{}\n{}\nStop your turn now, with this question in one line as your last words; the answer arrives as a message from the user, [from {USER_LABEL}].",
        asked.text, options_text(&asked),
    ))
}

fn options_text(asked: &Asked) -> String {
    asked.options.iter().enumerate()
        .map(|(index, (label, meaning))| format!("  {}. {label}{}: {meaning}{}", index + 1, if index + 1 == asked.recommend { " (recommended)" } else { "" }, takes(&asked.comment, index)))
        .collect::<Vec<_>>().join("\n")
}

fn of_task(question: &Message, to: &str) -> bool {
    question.from == to || question.from.split_once('@').is_some_and(|(task, _)| task == to)
}

// For a question the user settled elsewhere (in the task's Space, or by dropping it), so it leaves the board.
pub fn answered(to: &str) -> Result<String> {
    answered_as(&crate::permit::caller(), to)
}

// The user and the lead settle any question; a task only its own, so no session hides another's question from the user.
fn answered_as(caller: &crate::permit::Caller, to: &str) -> Result<String> {
    use crate::permit::Caller;
    let own = match caller {
        Caller::User | Caller::Lead => None,
        Caller::Task(address) => Some(address.as_str()),
        Caller::Agent => return Err(format!("marking a question answered is the user's or the lead's; {} cannot", crate::permit::describe(caller))),
    };
    let _lock = Lock::take(true)?;
    let messages = messages::load();
    let settled: Vec<&Message> = open_now(&messages).into_iter().filter(|question| of_task(question, to)).collect();
    if let Some(own) = own && settled.iter().any(|question| question.from != own) {
        return Err(format!("{own} marks only its own question answered; the user or the lead settles the one from {to}"));
    }
    if settled.is_empty() {
        return Err(format!("No question from \"{to}\" waits for the user; kelpie status --json lists each task's question"));
    }
    for question in &settled {
        messages::append(&json!({ "id": question.id, "at": messages::now(), "state": "answered" }))?;
    }
    let from: Vec<&str> = settled.iter().map(|question| question.from.as_str()).collect();
    Ok(format!("Marked the question from {} answered; it leaves the board.", from.join(", ")))
}

// The lead answers a question of method itself whether or not the user is here; keeping a question for the user's return needs them away.
fn lead_settles(caller: &crate::permit::Caller, away: bool, keep: bool) -> Result<()> {
    if !matches!(caller, crate::permit::Caller::Lead) {
        return Err(format!("only the lead settles a question of method or keeps one for the user; {} cannot", crate::permit::describe(caller)));
    }
    if keep && !away {
        return Err("The user is here: the question already waits for them on the board.".to_string());
    }
    Ok(())
}

// A bare task name must not pick one half's question when both halves wait on one.
fn open_from(open: &[&Message], to: &str) -> Result<Message> {
    let found: Vec<&&Message> = open.iter().filter(|question| of_task(question, to)).collect();
    match found.as_slice() {
        [] => Err(format!("No question from \"{to}\" waits for the user; kelpie status --json lists each task's question")),
        [question] => Ok((**question).clone()),
        several => Err(format!("{} each have a question open; name one as task@repo", several.iter().map(|question| question.from.as_str()).collect::<Vec<_>>().join(" and "))),
    }
}

// A how-question the lead answers itself: the answer goes to the task as the lead's call, closes the question on the board, and is recorded, for the user's return when they are away.
pub fn decide(to: &str, answer: &str) -> Result<String> {
    if answer.trim().is_empty() {
        return Err("kelpie decide needs the answer: kelpie decide <task> \"<answer>\"".to_string());
    }
    let away = crate::permit::away_since().is_some();
    lead_settles(&crate::permit::caller(), away, false)?;
    let sessions = peers::sessions()?;
    let _lock = Lock::take(true)?;
    let messages = messages::load();
    let question = open_from(&open_now(&messages), to)?;
    let task = sessions.iter().find(|session| session.address == question.from).ok_or_else(|| format!("{} has no session to answer", question.from))?;
    let text = format!("Your question is about how to build it, so this is the lead's call, not the user's: {}", answer.trim());
    let report = messages::send_locked(LEAD_LABEL, &[task], &text, &sessions, Some(&question.id), Some("decision"))?;
    let recorded = if away { "Recorded as your call for the user's return." } else { "Recorded as your call; it left the board." };
    Ok(format!("{} {recorded}", report.join(" ")))
}

// A what-question the lead keeps for the user while they are away; it stays on the board, and their return brief lists it first.
pub fn defer(to: &str) -> Result<String> {
    lead_settles(&crate::permit::caller(), crate::permit::away_since().is_some(), true)?;
    let _lock = Lock::take(true)?;
    let messages = messages::load();
    let question = open_from(&open_now(&messages), to)?;
    messages::append(&json!({ "id": question.id, "at": messages::now(), "state": "waits" }))?;
    Ok(format!("Kept {}'s question for the user; it waits for them on the board, and their summary lists it when they are back.", question.from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{fold, question_hold};

    fn asked() -> Asked {
        parse(&["Sidekiq: reuse the jobs we have", "Solid Queue: one less service"], Some("2"), false, "Which queue  should retries use?").unwrap()
    }

    fn log(lines: &[Value]) -> Vec<Message> {
        fold(&lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n"))
    }

    fn state(id: &str, state: &str) -> Value {
        json!({ "id": id, "at": 50, "state": state })
    }

    #[test]
    fn an_answer_given_on_the_board_after_a_new_turn_closed_the_question_still_reaches_the_task() {
        let log = [
            r#"{"id":"q1","at":10,"from":"billing@web","to":"you","text":"Probe?","state":"asked","question":{"options":[{"label":"Yes","meaning":"run it"},{"label":"No","meaning":"skip"}],"recommend":1}}"#,
            r#"{"id":"q2","at":11,"from":"shop@web","to":"you","text":"Which?","state":"asked","question":{"options":[["A","a"],["B","b"]],"recommend":1}}"#,
            r#"{"id":"q2","at":12,"state":"answered"}"#,
            r#"{"id":"q3","at":13,"from":"kelpie","to":"you","text":"Restart?","state":"asked","kind":"restart","question":{"options":[["Yes","y"],["No","n"]],"recommend":1}}"#,
        ].join("\n");
        let messages = fold(&log);
        assert!(open(&messages, |task| (task == "billing@web").then_some(20)).iter().all(|question| question.id != "q1"), "a new turn in the pane closed it");
        let late = closed_unanswered(&messages, "q1").unwrap();
        assert_eq!(format!("{} {CLOSED_BEFORE}", answer_text(late, Some(0), "").unwrap()), "The user's answer to \"Probe?\": Yes (run it). (The question had already closed when the user answered it on the board, since a new turn started in your pane; act on it if it still applies.)");
        assert!(closed_unanswered(&messages, "q2").is_none(), "answered once on the board, a second answer is refused");
        assert!(closed_unanswered(&messages, "nope").is_none());
        assert!(closed_unanswered(&messages, "q3").is_none(), "a restart question's late yes would restart nothing it still means");
        let source = include_str!("questions.rs");
        let answer = &source[source.find("pub fn answer(").unwrap()..source.find("pub const CLOSED_BEFORE").or_else(|| source.find("const CLOSED_BEFORE")).unwrap()];
        assert!(answer.contains("else {\n        return answer_late("), "a question no longer open goes to answer_late instead of an error that drops the answer");
    }

    #[test]
    fn a_question_takes_two_to_four_labelled_options_and_one_recommendation() {
        assert_eq!(asked(), Asked {
            text: "Which queue should retries use?".to_string(),
            options: vec![("Sidekiq".to_string(), "reuse the jobs we have".to_string()), ("Solid Queue".to_string(), "one less service".to_string())],
            recommend: 2,
            comment: Vec::new(),
        });
        let two = ["A: one", "B: two"];
        assert!(parse(&["A: one"], Some("1"), false, "Q?").is_err());
        assert!(parse(&["A: 1", "B: 2", "C: 3", "D: 4", "E: 5"], Some("1"), false, "Q?").is_err());
        assert!(parse(&["A one", "B: two"], Some("1"), false, "Q?").is_err());
        assert!(parse(&["A:", "B: two"], Some("1"), false, "Q?").is_err());
        assert!(parse(&two, None, false, "Q?").is_err());
        assert!(parse(&two, Some("3"), false, "Q?").is_err());
        assert!(parse(&two, Some("0"), false, "Q?").is_err());
        assert!(parse(&two, Some("1"), false, " ").is_err());
    }

    #[test]
    fn a_plan_question_offers_the_four_plan_answers_with_approve_recommended() {
        let plan = parse(&[], None, true, "").unwrap();
        assert_eq!(plan.text, "Approve this plan?");
        let labels: Vec<&str> = plan.options.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["Approve", "Approve with a comment", "Ask for changes", "Reject with a comment"]);
        assert_eq!(plan.recommend, 1);
        assert!(parse(&["A: one", "B: two"], Some("1"), true, "").is_err());
        assert!(parse(&[], Some("2"), true, "").is_err());
    }

    #[test]
    fn the_task_reads_the_option_as_asked_and_the_users_comment_in_their_words() {
        let messages = log(&[record("q1", 10, "billing@web", &asked(), None, None)]);
        assert_eq!(answer_text(&messages[0], Some(0), "").unwrap(), "The user's answer to \"Which queue should retries use?\": Sidekiq (reuse the jobs we have).");
        assert_eq!(answer_text(&messages[0], Some(1), " keep it in one file ").unwrap(), "The user's answer to \"Which queue should retries use?\": Solid Queue (one less service). Their comment: keep it in one file");
        assert_eq!(answer_text(&messages[0], None, "neither, use cron").unwrap(), "The user's answer to \"Which queue should retries use?\", in their own words: neither, use cron");
        assert!(answer_text(&messages[0], Some(2), "").is_err());
        assert!(answer_text(&messages[0], None, "  ").is_err());
        assert_eq!(answer_copy(&messages[0], Some(1), "fine"), "billing@web asked \"Which queue should retries use?\"; the user answered on the board: Solid Queue, with the comment: fine");
        assert_eq!(answer_copy(&messages[0], None, "use cron"), "billing@web asked \"Which queue should retries use?\"; the user answered on the board: in their own words: use cron");
    }

    #[test]
    fn the_leads_copy_of_a_question_says_it_settles_one_of_method_and_keeps_one_of_intent_only_while_the_user_is_away() {
        let messages = log(&[record("q1", 10, "billing@web", &asked(), Some("vault:Projects/billing.md"), Some(5000))]);
        let here = question_copy(&messages[0], false);
        assert!(here.starts_with("billing@web asks the user: Which queue should retries use? Options: Sidekiq: reuse the jobs we have; Solid Queue (recommended): one less service."), "{here}");
        assert!(here.contains("The plan: vault:Projects/billing.md.") && here.contains("kelpie decide billing@web \"<answer>\"") && here.ends_with("the user answers on the board."), "{here}");
        assert!(!here.contains("kelpie defer"), "{here}");
        let away = question_copy(&messages[0], true);
        assert!(away.contains("kelpie decide billing@web \"<answer>\"") && away.contains("kelpie defer billing@web"), "{away}");
    }

    #[test]
    fn the_lead_settles_a_question_of_method_while_the_user_is_here_and_keeps_one_only_while_they_are_away() {
        use crate::permit::Caller;
        assert!(lead_settles(&Caller::Lead, false, false).is_ok(), "the lead decides a how-question with the user present");
        assert!(lead_settles(&Caller::Lead, true, false).is_ok());
        assert!(lead_settles(&Caller::Lead, false, true).unwrap_err().contains("The user is here"));
        assert!(lead_settles(&Caller::Lead, true, true).is_ok());
        for caller in [Caller::User, Caller::Task("billing@web".to_string()), Caller::Agent] {
            assert!(lead_settles(&caller, true, false).unwrap_err().contains("only the lead settles"));
        }
    }

    #[test]
    fn every_open_question_waits_on_the_board_at_once_and_an_answer_or_a_deferral_settles_only_its_own() {
        let none = |_: &str| None;
        let questions = [record("q1", 10, "billing@web", &asked(), None, None), record("q2", 20, "search@api", &asked(), None, None)];
        assert_eq!(open(&log(&questions), none).len(), 2);
        let answered = log(&[questions[0].clone(), questions[1].clone(), state("q1", "answered")]);
        assert_eq!(open(&answered, none).iter().map(|question| question.id.as_str()).collect::<Vec<_>>(), ["q2"]);
        let kept = log(&[questions[0].clone(), questions[1].clone(), state("q1", "waits")]);
        let open_kept = open(&kept, none);
        assert_eq!(open_kept.len(), 2, "a kept question still waits on the user");
        assert!(open_kept[0].question.as_ref().unwrap().waits);
        assert_eq!(open_from(&open_kept, "search").unwrap().id, "q2");
        assert_eq!(open_from(&open_kept, "billing@web").unwrap().id, "q1");
        assert!(open_from(&open_kept, "payments").is_err());
    }

    #[test]
    fn a_bare_task_name_settles_nothing_while_both_its_halves_wait_on_a_question() {
        let both = log(&[record("q1", 10, "billing@web", &asked(), None, None), record("q2", 20, "billing@api", &asked(), None, None)]);
        let open = open(&both, |_: &str| None);
        let refused = open_from(&open, "billing").unwrap_err();
        assert!(refused.contains("billing@web and billing@api") && refused.contains("task@repo"), "{refused}");
        assert_eq!(open_from(&open, "billing@api").unwrap().id, "q2");
    }

    #[test]
    fn a_question_is_settled_by_its_tasks_end_a_later_question_or_a_new_turn() {
        let first = record("q1", 10, "billing@web", &asked(), None, None);
        let end = json!({ "id": "e", "at": 30, "from": "kelpie", "to": "billing@web", "text": "billing ended (done)", "state": "ended" });
        assert!(open(&log(&[first.clone(), end]), |_| None).is_empty());
        let later = log(&[first.clone(), record("q2", 20, "billing@web", &asked(), None, None)]);
        assert_eq!(open(&later, |_| None).iter().map(|question| question.id.as_str()).collect::<Vec<_>>(), ["q2"]);
        let one = log(&[first]);
        assert_eq!(open(&one, |_| Some(5)).len(), 1);
        assert_eq!(open(&one, |_| Some(10)).len(), 1);
        assert!(open(&one, |_| Some(11)).is_empty());
    }

    #[test]
    fn a_task_waiting_on_its_question_gets_only_the_answer_to_it() {
        assert_eq!(question_hold(None, Some("q1")), Some("waits for the user's answer to its question"), "a plain message, the lead's too, would start a turn and drop the question");
        assert_eq!(question_hold(Some("q0"), Some("q1")), Some("waits for the user's answer to its question"));
        assert_eq!(question_hold(Some("q1"), Some("q1")), None, "the user's answer or the lead's decide");
        assert_eq!(question_hold(None, None), None);
    }

    #[test]
    fn a_plan_is_kept_under_the_state_dir_as_it_was_asked() {
        let base = std::env::temp_dir().join(format!("kelpie-plan-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let source = base.join("plan.md");
        fs::write(&source, "# Plan\n1. rename").unwrap();
        let copy = keep_plan(&source, &base.join("state"), "billing@web", "10").unwrap();
        fs::write(&source, "changed").unwrap();
        assert_eq!(copy, base.join("state/plans/billing@web-10.md"));
        assert_eq!(fs::read_to_string(&copy).unwrap(), "# Plan\n1. rename");
        assert!(keep_plan(&base.join("missing.md"), &base.join("state"), "billing@web", "11").is_err());
        let question = Question { options: Vec::new(), recommend: 1, plan: Some(copy.display().to_string()), plan_at: None, comment: Vec::new(), answered: false, waits: false };
        assert_eq!(plan_text(&question).as_deref(), Some("# Plan\n1. rename"));
        let own = base.join("state/plans/billing@web.md");
        fs::write(&own, "# Own plan").unwrap();
        let kept = keep_plan(&own, &base.join("state"), "billing@web", "12").unwrap();
        assert!(!own.exists(), "kelpie's own plan file is moved, so the plan is stored once");
        assert_eq!(fs::read_to_string(kept).unwrap(), "# Own plan");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn two_plans_asked_in_the_same_second_are_kept_apart() {
        let base = std::env::temp_dir().join(format!("kelpie-plan-twice-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let (first, second) = (base.join("first.md"), base.join("second.md"));
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let kept: Vec<PathBuf> = [&first, &second].iter().map(|source| keep_plan(source, &base.join("state"), "billing@web", &messages::new_id("plan")).unwrap()).collect();
        assert_ne!(kept[0], kept[1]);
        assert_eq!((fs::read_to_string(&kept[0]).unwrap(), fs::read_to_string(&kept[1]).unwrap()), ("first".to_string(), "second".to_string()));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_vault_plan_reads_as_changed_once_its_note_changed_after_it_was_asked() {
        let question = |plan: &str, plan_at: Option<i64>| Question { options: Vec::new(), recommend: 1, plan: Some(plan.to_string()), plan_at, comment: Vec::new(), answered: false, waits: false };
        let note = |at: f64| move |key: &str| (key == "Projects/billing.md").then_some(at);
        assert!(!plan_changed_in(&question("vault:Projects/billing.md", Some(5000)), note(5.0)));
        assert!(plan_changed_in(&question("vault:Projects/billing.md", Some(5000)), note(9.0)));
        assert!(!plan_changed_in(&question("vault:Projects/billing.md", None), note(9.0)), "unknown when asked is not a change");
        assert!(!plan_changed_in(&question("/state/plans/billing@web-10.md", Some(5000)), note(9.0)), "a kept file never changes");
        let asked = log(&[record("q1", 10, "billing@web", &asked(), Some("vault:Projects/billing.md"), Some(5000))]);
        assert_eq!(asked[0].question.as_ref().unwrap().plan_at, Some(5000));
    }

    #[test]
    fn a_plans_mtime_survives_the_log_round_trip_and_an_old_seconds_value_still_reads() {
        let mtime = 1_760_000_000.123_456_7_f64;
        let line = record("q1", 10, "billing@web", &asked(), Some("vault:Projects/billing.md"), Some(millis(mtime))).to_string();
        let back = log(&[serde_json::from_str(&line).unwrap()]).remove(0).question.unwrap();
        assert!(!plan_changed_in(&back, |_| Some(mtime)), "the same mtime read back is no change");
        let old = line.replace(&format!("\"plan_at_ms\":{}", millis(mtime)), &format!("\"plan_at\":{mtime}"));
        assert!(old.contains("\"plan_at\":"), "{old}");
        let back = log(&[serde_json::from_str(&old).unwrap()]).remove(0).question.unwrap();
        assert_eq!(back.plan_at, Some(millis(mtime)));
        assert!(!plan_changed_in(&back, |_| Some(mtime)));
    }

    #[test]
    fn a_vault_plan_names_the_note_and_copies_nothing() {
        let state = std::env::temp_dir().join(format!("kelpie-vault-plan-{}", std::process::id()));
        let reference = |plan: &str| plan_reference(plan, Path::new("/nowhere"), &state, "billing@web", "10");
        assert_eq!(reference("vault:Projects/payload-rename.md").unwrap(), "vault:Projects/payload-rename.md");
        assert_eq!(reference(" vault:/Projects/payload-rename ").unwrap(), "vault:Projects/payload-rename.md");
        assert!(reference("vault:").is_err());
        assert!(!state.exists(), "a vault plan writes nothing under the state dir");
        assert!(reference("plan.md").is_err(), "a file must exist to be kept");
        let _ = fs::remove_dir_all(state);
    }

    #[test]
    fn the_question_text_lists_the_options_numbered_with_the_recommendation_and_the_plan() {
        let messages = log(&[record("q1", 10, "billing@web", &asked(), Some("vault:Projects/billing.md"), None)]);
        assert_eq!(text(&messages[0]), "Which queue should retries use?\n1. Sidekiq: reuse the jobs we have\n2. Solid Queue (recommended): one less service\nplan: vault:Projects/billing.md");
        let plain = log(&[json!({ "id": "m", "at": 1, "from": "a@x", "to": "lead", "text": "hello", "state": "delivered" })]);
        assert_eq!(text(&plain[0]), "hello");
        assert!(answer_text(&plain[0], Some(0), "").unwrap_err().contains("not a question"));
    }

    #[test]
    fn a_question_belongs_to_its_task_by_address_or_bare_name() {
        let messages = log(&[record("q1", 10, "billing@web", &asked(), None, None)]);
        assert!(of_task(&messages[0], "billing@web") && of_task(&messages[0], "billing"));
        assert!(!of_task(&messages[0], "billing@api") && !of_task(&messages[0], "bill") && !of_task(&messages[0], "web"));
    }

    #[test]
    fn the_options_are_one_line_each_and_odd_spacing_in_them_is_collapsed() {
        let asked = parse(&["  A  :  one\nline ", "B: two"], Some(" 1 "), false, "Q\n\n?").unwrap();
        assert_eq!(asked.options[0], ("A".to_string(), "one line".to_string()));
        assert_eq!(asked.text, "Q ?");
        let colon_in_meaning = parse(&["A: at 10:30", "B: two"], Some("1"), false, "Q?").unwrap();
        assert_eq!(colon_in_meaning.options[0], ("A".to_string(), "at 10:30".to_string()), "the first colon splits");
        assert!(parse(&["A: one", "B: two"], Some("1x"), false, "Q?").unwrap_err().contains("--recommend is the number"));
    }

    #[test]
    fn a_plan_file_is_kept_by_its_path_relative_to_where_ask_ran_and_a_vault_plan_is_only_named() {
        let base = std::env::temp_dir().join(format!("kelpie-plan-ref-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (cwd, state) = (base.join("wt1.app"), base.join("state"));
        fs::create_dir_all(cwd.join("docs")).unwrap();
        fs::write(cwd.join("docs/plan.md"), "# plan").unwrap();
        let kept = plan_reference("docs/plan.md", &cwd, &state, "billing@app", "7").unwrap();
        assert_eq!(kept, state.join("plans/billing@app-7.md").display().to_string());
        assert_eq!(fs::read_to_string(&kept).unwrap(), "# plan");
        assert!(cwd.join("docs/plan.md").exists(), "a file of the task's own is copied, not moved");
        let outside = base.join("elsewhere.md");
        fs::write(&outside, "outside").unwrap();
        assert!(plan_reference("../elsewhere.md", &cwd, &state, "billing@app", "8").is_ok(), "any file the task can read may be its plan; the board shows the user what it is");
        assert_eq!(plan_reference("vault:/Projects/x", &cwd, &state, "billing@app", "9").unwrap(), "vault:Projects/x.md");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn an_option_ending_in_the_comment_mark_takes_the_users_comment_and_the_answer_needs_it() {
        let asked = parse(&["Approve: build it", "Approve with changes: build it, taking your comment in +comment", "Tell me: write it   +comment  "], Some("1"), false, "Mock ok?").unwrap();
        assert_eq!(asked.options[1], ("Approve with changes".to_string(), "build it, taking your comment in".to_string()), "the mark is not part of the meaning");
        assert_eq!(asked.options[2].1, "write it");
        assert_eq!(asked.comment, [1, 2]);
        assert_eq!(parse(&["A: x+comment", "B: y"], Some("1"), false, "Q?").unwrap().comment, Vec::<usize>::new(), "only a mark after a space counts");
        let messages = log(&[record("q1", 10, "web@x", &asked, None, None)]);
        assert_eq!(messages[0].question.as_ref().unwrap().comment, [1, 2], "the mark survives the log");
        assert!(text(&messages[0]).contains("2. Approve with changes: build it, taking your comment in (with the user's comment)"), "{}", text(&messages[0]));
        assert!(question_copy(&messages[0], true).contains("Tell me: write it (with the user's comment)"), "the lead settling it while the user is away sees it too");
        assert!(answer_text(&messages[0], Some(1), " ").unwrap_err().contains("Approve with changes takes the user's comment"));
        assert!(answer_text(&messages[0], Some(1), "smaller").unwrap().ends_with("Their comment: smaller"));
        assert!(answer_text(&messages[0], Some(0), "").is_ok(), "an option without the mark needs no comment");
    }

    #[test]
    fn a_plan_takes_a_comment_on_its_three_commented_answers_also_from_a_record_written_before_the_mark() {
        let plan = parse(&[], None, true, "").unwrap();
        assert_eq!(plan.comment, PLAN_COMMENT);
        let old = log(&[json!({ "id": "q", "at": 1, "from": "a@x", "to": "you", "text": "Approve this plan?", "state": "asked", "question": { "options": PLAN_OPTIONS.iter().map(|(label, meaning)| json!({ "label": label, "meaning": meaning })).collect::<Vec<_>>(), "recommend": 1, "plan": "vault:Projects/a.md" } })]);
        assert_eq!(old[0].question.as_ref().unwrap().comment, PLAN_COMMENT);
        assert!(answer_text(&old[0], Some(2), "").is_err() && answer_text(&old[0], Some(0), "").is_ok());
    }
}
