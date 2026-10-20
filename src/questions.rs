use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::messages::{self, Lock, Message, Question, Report, USER_LABEL};
use crate::peers;
use crate::report;
use crate::sh::Result;
use crate::status::LEAD_LABEL;

const USAGE: &str = "kelpie ask --option \"<label>: <what it means>\" --option \"<label>: <what it means>\" [--option …] --recommend <n> \"<question>\", or kelpie ask --plan <vault:<note path> | file> [\"<question>\"]";

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
pub fn record(id: &str, at: u64, from: &str, asked: &Asked, plan: Option<&str>, plan_at: Option<f64>) -> Value {
    let options: Vec<Value> = asked.options.iter().map(|(label, meaning)| json!({ "label": label, "meaning": meaning })).collect();
    json!({
        "id": id, "at": at, "from": from, "to": USER_LABEL, "text": asked.text, "state": "asked",
        "question": { "options": options, "recommend": asked.recommend, "plan": plan, "plan_at": plan_at },
    })
}

// The task may change or delete its plan file; the user reads the plan as it was asked. kelpie's own plans/ file is moved rather than copied, so it is stored once.
pub fn keep_plan(source: &Path, state: &Path, me: &str, at: u64) -> Result<PathBuf> {
    let dir = state.join("plans");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let kept = dir.join(format!("{me}-{at}.md"));
    let own = source.parent().and_then(|parent| parent.canonicalize().ok()) == dir.canonicalize().ok();
    let stored = if own { fs::rename(source, &kept) } else { fs::copy(source, &kept).map(|_| ()) };
    stored.map_err(|error| format!("{}: {error}", source.display()))?;
    Ok(kept)
}

// "vault:Projects/x" names the note Projects/x.md; anything else is a file, kept as a copy.
pub fn plan_reference(plan: &str, cwd: &Path, state: &Path, me: &str, at: u64) -> Result<String> {
    let plan = plan.trim();
    match plan.strip_prefix(VAULT_PLAN) {
        Some(key) => {
            let key = key.trim().trim_start_matches('/');
            if key.is_empty() {
                return Err("kelpie ask --plan vault:<path> needs the note's path in the vault, as vault:Projects/<note>.md".to_string());
            }
            Ok(format!("{VAULT_PLAN}{key}{}", if key.ends_with(".md") { "" } else { ".md" }))
        }
        None => keep_plan(&cwd.join(plan), state, me, at).map(|copy| copy.display().to_string()),
    }
}

// A vault plan is the living note, so the board says when it changed after it was asked; a file plan is kept as asked and never changes.
pub fn plan_changed(question: &Question) -> bool {
    let Ok(index) = crate::vault::Index::open() else { return false };
    plan_changed_in(question, |key| index.changed_at(key))
}

fn plan_changed_in(question: &Question, changed_at: impl Fn(&str) -> Option<f64>) -> bool {
    let Some(key) = question.plan.as_deref().and_then(|plan| plan.strip_prefix(VAULT_PLAN)) else { return false };
    matches!((question.plan_at, changed_at(key)), (Some(asked), Some(now)) if now != asked)
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

pub fn text(message: &Message) -> String {
    let Some(question) = &message.question else { return message.text.clone() };
    let mut lines = vec![message.text.clone()];
    for (index, (label, meaning)) in question.options.iter().enumerate() {
        let recommended = if index + 1 == question.recommend { " (recommended)" } else { "" };
        lines.push(format!("{}. {label}{recommended}: {meaning}", index + 1));
    }
    if let Some(plan) = &question.plan {
        lines.push(format!("plan: {plan}"));
    }
    lines.join("\n")
}

fn options_line(question: &Question) -> String {
    question.options.iter().enumerate()
        .map(|(index, (label, meaning))| format!("{label}{}: {meaning}", if index + 1 == question.recommend { " (recommended)" } else { "" }))
        .collect::<Vec<_>>().join("; ")
}

// The lead's copy of a new question: while the user is away it settles a method question itself, so the copy says how.
pub fn question_copy(message: &Message, away: bool) -> String {
    let Some(question) = &message.question else { return message.text.clone() };
    let plan = question.plan.as_ref().map(|plan| format!(" The plan: {plan}.")).unwrap_or_default();
    let then = if away {
        format!(" The user is away: settle a question of method with kelpie decide {from} \"<answer>\", and keep one of intent for them with kelpie defer {from}.", from = message.from)
    } else {
        " The user answers it on the board.".to_string()
    };
    format!("{} asks the user: {} Options: {}.{plan}{then}", message.from, message.text, options_line(question))
}

pub fn answer_text(message: &Message, choice: Option<usize>, comment: &str) -> Result<String> {
    let question = message.question.as_ref().ok_or("That message is not a question")?;
    let comment = comment.trim();
    match choice {
        Some(index) => {
            let (label, meaning) = question.options.get(index).ok_or_else(|| format!("The question has {} options; there is no option {}", question.options.len(), index + 1))?;
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
    let message = open.iter().copied().find(|question| question.id == id).ok_or("That question no longer waits for an answer")?;
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

pub fn ask(cwd: &Path, options: &[&str], recommend: Option<&str>, plan: Option<&str>, text: &str) -> Result<String> {
    let asked = parse(options, recommend, plan.is_some(), text)?;
    let me = peers::me(cwd).filter(|me| me != LEAD_LABEL).ok_or("kelpie ask runs in a task's worktree")?;
    let sessions = peers::sessions().unwrap_or_default();
    let _lock = Lock::take(true)?;
    if open_now(&messages::load()).iter().any(|question| question.from == me) {
        return Err("Your earlier question still waits for the user; one question at a time, so stop your turn and wait for the answer.".to_string());
    }
    let at = messages::now();
    let plan = plan.map(|plan| plan_reference(plan, cwd, &state_dir()?, &me, at)).transpose()?;
    let next = if plan.is_some() { format!("read and approve the plan: {}", asked.text) } else { format!("answer: {}", asked.text) };
    let report = Report { state: "needs-you".to_string(), next };
    messages::log_report(&me, &report, &format!("asked the user: {}", asked.text))?;
    report::show_in_sidebar(&report);
    let id = messages::new_id("question");
    let plan_at = plan.as_deref().and_then(|plan| plan.strip_prefix(VAULT_PLAN)).and_then(|key| crate::vault::Index::open().ok()?.changed_at(key));
    let record = record(&id, at, &me, &asked, plan.as_deref(), plan_at);
    messages::append(&record)?;
    let message = messages::fold(&record.to_string()).remove(0);
    messages::copy_to_lead_locked(&question_copy(&message, crate::permit::away_since().is_some()), Some(&id), &sessions)?;
    crate::notify::send(cwd, &crate::notify::Notice::from(crate::notify::Event::Question, &me, format!("{me} asks: {}", asked.text)));
    Ok(format!(
        "Asked the user; the question waits for them on the board:\n{}\n{}\nStop your turn now, with this question in one line as your last words; the answer arrives as a message from the user, [from {USER_LABEL}].",
        asked.text, options_text(&asked),
    ))
}

fn options_text(asked: &Asked) -> String {
    asked.options.iter().enumerate()
        .map(|(index, (label, meaning))| format!("  {}. {label}{}: {meaning}", index + 1, if index + 1 == asked.recommend { " (recommended)" } else { "" }))
        .collect::<Vec<_>>().join("\n")
}

fn of_task(question: &Message, to: &str) -> bool {
    question.from == to || question.from.split_once('@').is_some_and(|(task, _)| task == to)
}

// For a question the user settled elsewhere (in the task's Space, or by dropping it), so it leaves the board.
pub fn answered(to: &str) -> Result<String> {
    let _lock = Lock::take(true)?;
    let messages = messages::load();
    let settled: Vec<&Message> = open_now(&messages).into_iter().filter(|question| of_task(question, to)).collect();
    if settled.is_empty() {
        return Err(format!("No question from \"{to}\" waits for the user; kelpie status --json lists each task's question"));
    }
    for question in &settled {
        messages::append(&json!({ "id": question.id, "at": messages::now(), "state": "answered" }))?;
    }
    let from: Vec<&str> = settled.iter().map(|question| question.from.as_str()).collect();
    Ok(format!("Marked the question from {} answered; it leaves the board.", from.join(", ")))
}

// The lead settles a question on its own only while the user is away.
fn lead_while_away() -> Result<()> {
    match crate::permit::caller() {
        crate::permit::Caller::Lead => {}
        other => return Err(format!("only the lead settles a question while the user is away; {} cannot", crate::permit::describe(&other))),
    }
    if crate::permit::away_since().is_none() {
        return Err("The user is here: they answer the question on the board.".to_string());
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

// A how-question the lead answers itself while the user is away: the answer goes to the task as the lead's call, closes the question on the board, and is recorded for the user's return.
pub fn decide(to: &str, answer: &str) -> Result<String> {
    if answer.trim().is_empty() {
        return Err("kelpie decide needs the answer: kelpie decide <task> \"<answer>\"".to_string());
    }
    lead_while_away()?;
    let sessions = peers::sessions()?;
    let _lock = Lock::take(true)?;
    let messages = messages::load();
    let question = open_from(&open_now(&messages), to)?;
    let task = sessions.iter().find(|session| session.address == question.from).ok_or_else(|| format!("{} has no session to answer", question.from))?;
    let text = format!("The user is away, so this is the lead's call on your question, not theirs: {}", answer.trim());
    let report = messages::send_locked(LEAD_LABEL, &[task], &text, &sessions, Some(&question.id), Some("decision"))?;
    Ok(format!("{} Recorded as your call for the user's return.", report.join(" ")))
}

// A what-question the lead keeps for the user while they are away; it stays on the board, and their return brief lists it first.
pub fn defer(to: &str) -> Result<String> {
    lead_while_away()?;
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
    fn the_leads_copy_of_a_question_says_how_to_settle_it_only_while_the_user_is_away() {
        let messages = log(&[record("q1", 10, "billing@web", &asked(), Some("vault:Projects/billing.md"), Some(5.0))]);
        let here = question_copy(&messages[0], false);
        assert!(here.starts_with("billing@web asks the user: Which queue should retries use? Options: Sidekiq: reuse the jobs we have; Solid Queue (recommended): one less service."), "{here}");
        assert!(here.contains("The plan: vault:Projects/billing.md.") && here.ends_with("The user answers it on the board."), "{here}");
        let away = question_copy(&messages[0], true);
        assert!(away.contains("kelpie decide billing@web \"<answer>\"") && away.contains("kelpie defer billing@web"), "{away}");
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
        let copy = keep_plan(&source, &base.join("state"), "billing@web", 10).unwrap();
        fs::write(&source, "changed").unwrap();
        assert_eq!(copy, base.join("state/plans/billing@web-10.md"));
        assert_eq!(fs::read_to_string(&copy).unwrap(), "# Plan\n1. rename");
        assert!(keep_plan(&base.join("missing.md"), &base.join("state"), "billing@web", 11).is_err());
        let question = Question { options: Vec::new(), recommend: 1, plan: Some(copy.display().to_string()), plan_at: None, answered: false, waits: false };
        assert_eq!(plan_text(&question).as_deref(), Some("# Plan\n1. rename"));
        let own = base.join("state/plans/billing@web.md");
        fs::write(&own, "# Own plan").unwrap();
        let kept = keep_plan(&own, &base.join("state"), "billing@web", 12).unwrap();
        assert!(!own.exists(), "kelpie's own plan file is moved, so the plan is stored once");
        assert_eq!(fs::read_to_string(kept).unwrap(), "# Own plan");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_vault_plan_reads_as_changed_once_its_note_changed_after_it_was_asked() {
        let question = |plan: &str, plan_at: Option<f64>| Question { options: Vec::new(), recommend: 1, plan: Some(plan.to_string()), plan_at, answered: false, waits: false };
        let note = |at: f64| move |key: &str| (key == "Projects/billing.md").then_some(at);
        assert!(!plan_changed_in(&question("vault:Projects/billing.md", Some(5.0)), note(5.0)));
        assert!(plan_changed_in(&question("vault:Projects/billing.md", Some(5.0)), note(9.0)));
        assert!(!plan_changed_in(&question("vault:Projects/billing.md", None), note(9.0)), "unknown when asked is not a change");
        assert!(!plan_changed_in(&question("/state/plans/billing@web-10.md", Some(5.0)), note(9.0)), "a kept file never changes");
        let asked = log(&[record("q1", 10, "billing@web", &asked(), Some("vault:Projects/billing.md"), Some(5.0))]);
        assert_eq!(asked[0].question.as_ref().unwrap().plan_at, Some(5.0));
    }

    #[test]
    fn a_vault_plan_names_the_note_and_copies_nothing() {
        let state = std::env::temp_dir().join(format!("kelpie-vault-plan-{}", std::process::id()));
        let reference = |plan: &str| plan_reference(plan, Path::new("/nowhere"), &state, "billing@web", 10);
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
        let kept = plan_reference("docs/plan.md", &cwd, &state, "billing@app", 7).unwrap();
        assert_eq!(kept, state.join("plans/billing@app-7.md").display().to_string());
        assert_eq!(fs::read_to_string(&kept).unwrap(), "# plan");
        assert!(cwd.join("docs/plan.md").exists(), "a file of the task's own is copied, not moved");
        let outside = base.join("elsewhere.md");
        fs::write(&outside, "outside").unwrap();
        assert!(plan_reference("../elsewhere.md", &cwd, &state, "billing@app", 8).is_ok(), "any file the task can read may be its plan; the board shows the user what it is");
        assert_eq!(plan_reference("vault:/Projects/x", &cwd, &state, "billing@app", 9).unwrap(), "vault:Projects/x.md");
        fs::remove_dir_all(base).unwrap();
    }
}
