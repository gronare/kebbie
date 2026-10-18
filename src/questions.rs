use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::messages::{self, Lock, Message, Question, Report};
use crate::peers::{self, Session};
use crate::report;
use crate::sh::Result;
use crate::status::LEAD_LABEL;

const USAGE: &str = "kelpie ask --option \"<label>: <what it means>\" --option \"<label>: <what it means>\" [--option …] --recommend <n> \"<question>\", or kelpie ask --plan <file> [\"<question>\"]";

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
}

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
        return Ok(Asked { text, options: PLAN_OPTIONS.iter().map(|(label, meaning)| (label.to_string(), meaning.to_string())).collect(), recommend: 1 });
    }
    if text.is_empty() {
        return Err(format!("kelpie ask needs the question: {USAGE}"));
    }
    if !(2..=4).contains(&options.len()) {
        return Err(format!("kelpie ask takes two to four --option, not {}: {USAGE}", options.len()));
    }
    let options = options.iter().map(|option| match option.split_once(':') {
        Some((label, meaning)) if !one_line(label).is_empty() && !one_line(meaning).is_empty() => Ok((one_line(label), one_line(meaning))),
        _ => Err(format!("--option \"{option}\" needs a label and what it means, as \"<label>: <what it means>\"")),
    }).collect::<Result<Vec<_>>>()?;
    let recommend = recommend.ok_or(format!("kelpie ask needs --recommend <n>, the option you recommend: {USAGE}"))?;
    let recommend: usize = recommend.trim().parse().ok().filter(|n| (1..=options.len()).contains(n))
        .ok_or(format!("--recommend is the number of an option, 1 to {}, not \"{recommend}\"", options.len()))?;
    Ok(Asked { text, options, recommend })
}

// The question's own log line; the task's report beside it puts the task under Needs you.
pub fn record(id: &str, at: u64, from: &str, asked: &Asked, plan: Option<&Path>) -> Value {
    let options: Vec<Value> = asked.options.iter().map(|(label, meaning)| json!({ "label": label, "meaning": meaning })).collect();
    json!({
        "id": id, "at": at, "from": from, "to": LEAD_LABEL, "text": asked.text, "state": "asked",
        "question": { "options": options, "recommend": asked.recommend, "plan": plan.map(|plan| plan.display().to_string()) },
    })
}

// The task may change or delete its plan file; the user reads the copy it asked with.
pub fn keep_plan(source: &Path, state: &Path, me: &str, at: u64) -> Result<PathBuf> {
    let dir = state.join("plans");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let copy = dir.join(format!("{me}-{at}.md"));
    fs::copy(source, &copy).map_err(|error| format!("{}: {error}", source.display()))?;
    Ok(copy)
}

// Questions waiting on the user, oldest first: a task's later question, its end, its answer, or a new turn of it settles one.
pub fn open(messages: &[Message], turn_started: impl Fn(&str) -> Option<u64>) -> Vec<&Message> {
    let mut open: Vec<&Message> = Vec::new();
    for message in messages {
        if message.ended {
            open.retain(|question| question.from != message.to);
        } else if let Some(question) = &message.question {
            open.retain(|earlier| earlier.from != message.from);
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

// The lead relays one question at a time: none while it holds one, else the one that has waited longest.
pub fn next_to_give<'a>(open: &[&'a Message]) -> Option<&'a Message> {
    if open.iter().any(|question| question.question.as_ref().is_some_and(|question| question.given)) {
        return None;
    }
    open.first().copied()
}

pub fn answers(from: &str, question: &Message) -> bool {
    from == LEAD_LABEL && question.question.as_ref().is_some_and(|question| question.given)
}

// The options as the lead puts them to the user: the recommended one first.
pub fn ordered(question: &Question) -> Vec<(String, String, bool)> {
    let mut options: Vec<(String, String, bool)> = question.options.iter().enumerate()
        .map(|(index, (label, meaning))| (label.clone(), meaning.clone(), index + 1 == question.recommend)).collect();
    options.sort_by_key(|(_, _, recommended)| !recommended);
    options
}

pub fn lead_text(message: &Message, waiting_after: usize) -> String {
    let Some(question) = &message.question else { return message.text.clone() };
    let options: Vec<String> = ordered(question).iter().enumerate()
        .map(|(index, (label, meaning, recommended))| format!("{}. {label}{}: {meaning}", index + 1, if *recommended { " (recommended)" } else { "" }))
        .collect();
    let waiting = match waiting_after {
        0 => String::new(),
        1 => " (1 more question waits after this one)".to_string(),
        count => format!(" ({count} more questions wait after this one)"),
    };
    let plan = question.plan.as_ref().map(|plan| format!(" The plan to read: {plan}.")).unwrap_or_default();
    format!(
        "{from} asks the user{waiting}: {text} Options: {options}.{plan} Ask the user with AskUserQuestion, then pass their answer on with kelpie tell {from} \"<answer>\".",
        from = message.from, text = message.text, options = options.join("; "),
    )
}

// The caller holds the message lock.
pub fn release(sessions: &[Session]) -> Result<()> {
    let messages = messages::load();
    let open = open_now(&messages);
    let Some(next) = next_to_give(&open) else { return Ok(()) };
    let Some(lead) = sessions.iter().find(|session| session.address == LEAD_LABEL) else { return Ok(()) };
    messages::send_locked("kelpie", &[lead], &lead_text(next, open.len() - 1), sessions, Some(&next.id))?;
    messages::append(&json!({ "id": next.id, "at": messages::now(), "state": "given" }))
}

fn options_text(asked: &Asked) -> String {
    asked.options.iter().enumerate()
        .map(|(index, (label, meaning))| format!("  {}. {label}{}: {meaning}", index + 1, if index + 1 == asked.recommend { " (recommended)" } else { "" }))
        .collect::<Vec<_>>().join("\n")
}

pub fn ask(cwd: &Path, options: &[&str], recommend: Option<&str>, plan: Option<&str>, text: &str) -> Result<String> {
    let asked = parse(options, recommend, plan.is_some(), text)?;
    let me = peers::me(cwd).filter(|me| me != LEAD_LABEL).ok_or("kelpie ask runs in a task's worktree")?;
    let _lock = Lock::take(true)?;
    if open_now(&messages::load()).iter().any(|question| question.from == me) {
        return Err("Your earlier question still waits for the user; one question at a time, so stop your turn and wait for the answer.".to_string());
    }
    let at = messages::now();
    let plan = plan.map(|plan| keep_plan(&cwd.join(plan), &state_dir()?, &me, at)).transpose()?;
    let next = if plan.is_some() { format!("read and approve the plan: {}", asked.text) } else { format!("answer: {}", asked.text) };
    let report = Report { state: "needs-you".to_string(), next };
    messages::log_report(&me, &report, &format!("asked the user through the lead: {}", asked.text))?;
    report::show_in_sidebar(&report);
    let id = format!("{at}-{}-question", std::process::id());
    messages::append(&record(&id, at, &me, &asked, plan.as_deref()))?;
    let ahead = open_now(&messages::load()).len() - 1;
    if let Ok(sessions) = peers::sessions() {
        release(&sessions)?;
    }
    let queue = match ahead {
        0 => "next".to_string(),
        1 => "after 1 question ahead of it".to_string(),
        count => format!("after {count} questions ahead of it"),
    };
    Ok(format!(
        "Asked the user through the lead, {queue}:\n{}\n{}\nStop your turn now, with this question in one line as your last words; the answer arrives as a message from the lead.",
        asked.text, options_text(&asked),
    ))
}

// For a question the user settled elsewhere (in the task's Space, or by dropping it), so the next one goes to the lead.
pub fn answered(to: &str) -> Result<String> {
    let _lock = Lock::take(true)?;
    let messages = messages::load();
    let open = open_now(&messages);
    let settled: Vec<&&Message> = open.iter().filter(|question| question.from == to || question.from.split_once('@').is_some_and(|(task, _)| task == to)).collect();
    if settled.is_empty() {
        return Err(format!("No question from \"{to}\" waits for the user; kelpie status --json lists each task's question"));
    }
    for question in &settled {
        messages::append(&json!({ "id": question.id, "at": messages::now(), "state": "answered" }))?;
    }
    if let Ok(sessions) = peers::sessions() {
        release(&sessions)?;
    }
    let from: Vec<&str> = settled.iter().map(|question| question.from.as_str()).collect();
    Ok(format!("Marked the question from {} answered; the next question, if any, goes to the lead.", from.join(", ")))
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
    fn a_question_takes_two_to_four_labelled_options_and_one_recommendation() {
        assert_eq!(asked(), Asked {
            text: "Which queue should retries use?".to_string(),
            options: vec![("Sidekiq".to_string(), "reuse the jobs we have".to_string()), ("Solid Queue".to_string(), "one less service".to_string())],
            recommend: 2,
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
    fn the_lead_gets_the_tasks_question_with_the_same_options_the_recommended_one_first() {
        let messages = log(&[record("q1", 10, "billing@web", &asked(), Some(Path::new("/state/plans/billing@web-10.md")))]);
        let question = messages[0].question.as_ref().unwrap();
        assert_eq!((question.options.len(), question.recommend, question.given, question.answered), (2, 2, false, false));
        let text = lead_text(&messages[0], 2);
        assert!(text.starts_with("billing@web asks the user (2 more questions wait after this one): Which queue should retries use?"), "{text}");
        assert!(text.contains("Options: 1. Solid Queue (recommended): one less service; 2. Sidekiq: reuse the jobs we have."), "{text}");
        assert!(text.contains("The plan to read: /state/plans/billing@web-10.md."), "{text}");
        assert!(text.contains("AskUserQuestion") && text.contains("kelpie tell billing@web \"<answer>\""), "{text}");
    }

    #[test]
    fn the_lead_gets_the_oldest_question_and_the_next_only_once_it_has_passed_one_on() {
        let none = |_: &str| None;
        let questions = [record("q1", 10, "billing@web", &asked(), None), record("q2", 20, "search@api", &asked(), None)];
        let waiting = log(&questions);
        assert_eq!(next_to_give(&open(&waiting, none)).map(|question| question.id.as_str()), Some("q1"));
        let given = log(&[questions[0].clone(), questions[1].clone(), state("q1", "given")]);
        let open_given = open(&given, none);
        assert_eq!(open_given.len(), 2);
        assert!(next_to_give(&open_given).is_none());
        assert!(answers(LEAD_LABEL, open_given[0]) && !answers("search@api", open_given[0]) && !answers(LEAD_LABEL, open_given[1]));
        let passed = log(&[questions[0].clone(), questions[1].clone(), state("q1", "given"), state("q1", "answered")]);
        assert_eq!(next_to_give(&open(&passed, none)).map(|question| question.id.as_str()), Some("q2"));
    }

    #[test]
    fn a_question_is_settled_by_its_tasks_end_a_later_question_or_a_new_turn() {
        let first = record("q1", 10, "billing@web", &asked(), None);
        let end = json!({ "id": "e", "at": 30, "from": "kelpie", "to": "billing@web", "text": "billing ended (done)", "state": "ended" });
        assert!(open(&log(&[first.clone(), end]), |_| None).is_empty());
        let later = log(&[first.clone(), record("q2", 20, "billing@web", &asked(), None)]);
        assert_eq!(open(&later, |_| None).iter().map(|question| question.id.as_str()).collect::<Vec<_>>(), ["q2"]);
        let one = log(&[first]);
        assert_eq!(open(&one, |_| Some(5)).len(), 1);
        assert_eq!(open(&one, |_| Some(10)).len(), 1);
        assert!(open(&one, |_| Some(11)).is_empty());
    }

    #[test]
    fn a_task_waiting_on_its_question_gets_only_the_leads_messages() {
        assert_eq!(question_hold("search@api", true), Some("waits for the user's answer to its question"));
        assert_eq!(question_hold("kelpie", true), Some("waits for the user's answer to its question"));
        assert_eq!(question_hold(LEAD_LABEL, true), None);
        assert_eq!(question_hold("search@api", false), None);
    }

    #[test]
    fn a_plan_is_kept_under_the_state_dir_as_it_was_asked() {
        let base = std::env::temp_dir().join(format!("kelpie-plan-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let source = base.join("plan.md");
        fs::write(&source, "# Plan\n1. rename").unwrap();
        let copy = keep_plan(&source, &base.join("state"), "billing@web", 10).unwrap();
        fs::write(&source, "changed").unwrap();
        assert_eq!(copy, base.join("state/plans/billing@web-10.md"));
        assert_eq!(fs::read_to_string(&copy).unwrap(), "# Plan\n1. rename");
        assert!(keep_plan(&base.join("missing.md"), &base.join("state"), "billing@web", 11).is_err());
        fs::remove_dir_all(base).unwrap();
    }
}
