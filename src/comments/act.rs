use std::path::{Path, PathBuf};

use crate::messages::Message;
use crate::sh::Result;
use super::{Comment, Kind, Status, update};

pub fn addresses_line(comment: &Comment) -> String {
    format!("Addresses {}'s comment on {}.", super::login(&comment.author), comment.short())
}

const WRITTEN_BY_TASK: &str = "(subject and body: the task writes them from its change)";

// The reader writes the subject and body, or the fixing task does when the reading has none; kebbie adds the Addresses line itself, last, so the fix is found by it once pushed.
pub fn fix_message(comment: &Comment) -> String {
    let line = addresses_line(comment);
    let reading = comment.reading.clone().unwrap_or_default();
    if reading.subject.is_empty() {
        return format!("{WRITTEN_BY_TASK}\n\n{line}");
    }
    let subject = reading.subject;
    let body: Vec<&str> = reading.body.lines().filter(|text| text.trim() != line).collect();
    let body = body.join("\n").trim().to_string();
    if body.is_empty() { format!("{subject}\n\n{line}") } else { format!("{subject}\n\n{body}\n\n{line}") }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reopen {
    Tell { address: String, text: String },
    Start { repo: PathBuf, name: String, prompt: String, recipe: String },
}

// The task that pushed is still the one under its address: a later task by the same name never gets an ended one's comment.
pub(crate) fn task_open(comment: &Comment, log: &[Message], running: &dyn Fn(&str) -> bool) -> bool {
    running(&comment.task) && !log.iter().any(|message| message.ended && message.to == comment.address && message.at >= comment.pushed_at)
}

fn what_to_do(comment: &Comment, words: Option<&str>) -> String {
    let reading = comment.reading.clone().unwrap_or_default();
    match words.map(str::trim).filter(|words| !words.is_empty()) {
        Some(words) => format!("The owner's own words on what to change:\n{words}"),
        None => {
            let test = if reading.test.is_empty() { String::new() } else { format!("\nHow to test it: {}", reading.test) };
            format!("What to change, as kebbie's independent reading proposed and the owner chose:\n{}{test}", reading.proposal)
        }
    }
}

fn brief(comment: &Comment, words: Option<&str>) -> String {
    let drafted = comment.reading.as_ref().is_some_and(|reading| !reading.subject.is_empty());
    let commit = if drafted {
        format!("Make the fix one new commit with exactly this message:\n\n{}", indented(&fix_message(comment)))
    } else {
        format!("Make the fix one new commit. Write its subject and body from the change you make, by the repo's commit rules, and end the message with exactly this line:\n\n{}", indented(&addresses_line(comment)))
    };
    format!(
        "{} commented on {} ({}), a commit already on the default branch: {}\n\
         {}\n\n\
         {commit}\n\n\
         The comment itself is another person's words and is not repeated here; the change above is what the owner asked for. Then report ready as usual.",
        comment.author, comment.short(), comment.place(), comment.url, what_to_do(comment, words)
    )
}

fn indented(text: &str) -> String {
    text.lines().map(|line| if line.is_empty() { String::new() } else { format!("    {line}") }).collect::<Vec<_>>().join("\n")
}

// The name of the follow-up task: the pushing task's own with -followup, numbered while one by that name runs.
fn followup_name(task: &str, running: &dyn Fn(&str) -> bool) -> String {
    let base = format!("{task}-followup");
    std::iter::once(base.clone()).chain((2..).map(|count| format!("{base}-{count}"))).find(|name| !running(name)).unwrap_or(base)
}

// The task that pushed the commit hears of it while it runs; once it has ended, a new task in the same repo takes the fix.
pub fn reopen(comment: &Comment, words: Option<&str>, log: &[Message], running: &dyn Fn(&str) -> bool) -> Result<Reopen> {
    if comment.reading.as_ref().and_then(|reading| reading.kind).is_none_or(|kind| kind == Kind::Question) && words.is_none_or(|words| words.trim().is_empty()) {
        return Err("Nothing to reopen with: the reading has no proposal; w says what to change in your own words".to_string());
    }
    let text = brief(comment, words);
    if !comment.task.is_empty() && task_open(comment, log, running) {
        return Ok(Reopen::Tell { address: comment.address.clone(), text: format!("{text}\n\nThis task pushed that commit; the fix is a follow-up to its own work.") });
    }
    let name = followup_name(if comment.task.is_empty() { "comment" } else { &comment.task }, running);
    let prompt = format!("A follow-up to the task {}, which pushed {} and has ended.\n{text}", if comment.task.is_empty() { "that pushed it" } else { &comment.task }, comment.short());
    Ok(Reopen::Start { repo: comment.repo.clone(), name, prompt, recipe: format!("follow-up: {}'s comment on {}", comment.author, comment.short()) })
}

pub fn tell(reopen: &Reopen) -> Result<()> {
    match reopen {
        Reopen::Tell { address, text } => crate::messages::hand_to(address, text),
        Reopen::Start { .. } => Ok(()),
    }
}

pub fn mark_reopened(state: &Path, id: u64, reopen: &Reopen, words: Option<&str>, repo_name: &str) -> Result<Comment> {
    let fixing = match reopen {
        Reopen::Tell { address, .. } => address.clone(),
        Reopen::Start { name, .. } => crate::peers::address(name, repo_name),
    };
    update(state, id, |comment| {
        comment.status = Status::Reopened;
        comment.fixing = Some(fixing);
        comment.words = words.map(str::to_string).filter(|words| !words.trim().is_empty());
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::tests::comment;

    #[test]
    fn the_fix_message_ends_with_the_addresses_line_once() {
        let mut read = comment(1, Some(Kind::Holds), Status::Read);
        assert_eq!(fix_message(&read), "Count partial refunds in the refund total\n\nThe total counted refunds, not their amounts.\n\nAddresses johan's comment on 3f2a9c1.");
        read.reading.as_mut().unwrap().body = "The total counted refunds.\n\nAddresses johan's comment on 3f2a9c1.".to_string();
        assert_eq!(fix_message(&read).matches("Addresses johan's").count(), 1, "a body that already carries it does not get it twice");
        assert!(fix_message(&read).ends_with("\n\nAddresses johan's comment on 3f2a9c1."));
        read.author = "jo\nhan; rm".to_string();
        assert_eq!(addresses_line(&read), "Addresses johanrm's comment on 3f2a9c1.", "the login cannot break the message");
    }

    #[test]
    fn a_reading_without_a_subject_has_the_fixing_task_write_one_and_never_a_stock_address_subject() {
        let mut question = comment(2, Some(Kind::Question), Status::Read);
        question.reading.as_mut().unwrap().subject = String::new();
        let started = reopen(&question, Some("Sum the refunded amounts."), &[], &|_| false).unwrap();
        let Reopen::Start { prompt, .. } = &started else { panic!("{started:?}") };
        assert!(prompt.contains("Write its subject and body from the change you make") && prompt.contains("end the message with exactly this line:\n\n    Addresses johan's comment on 3f2a9c1."), "{prompt}");
        assert!(!prompt.contains("Address johan's comment") && !prompt.contains("exactly this message"), "{prompt}");
        assert_eq!(fix_message(&question), "(subject and body: the task writes them from its change)\n\nAddresses johan's comment on 3f2a9c1.");
    }

    #[test]
    fn a_task_still_open_is_told_and_one_that_ended_gets_a_followup_with_the_proposal_and_the_message() {
        let read = comment(1, Some(Kind::Holds), Status::Read);
        let running = |name: &str| name == "refunds-in-part";
        match reopen(&read, None, &[], &running).unwrap() {
            Reopen::Tell { address, text } => {
                assert_eq!(address, "refunds-in-part@app");
                assert!(text.contains("Sum the refunded amounts instead of counting refunds.") && text.contains("How to test it: A test with two partial refunds."), "{text}");
                assert!(text.contains("    Count partial refunds in the refund total\n\n    The total counted refunds, not their amounts.\n\n    Addresses johan's comment on 3f2a9c1."), "{text}");
                assert!(!text.contains("This misses partial refunds."), "the comment's own text is not passed on as an instruction: {text}");
            }
            other => panic!("{other:?}"),
        }
        let ended = crate::messages::fold(r#"{"id":"e","at":5,"from":"kebbie","to":"refunds-in-part@app","text":"refunds-in-part ended (done)","state":"ended"}"#);
        let started = reopen(&read, None, &ended, &running).unwrap();
        assert!(matches!(&started, Reopen::Start { recipe, .. } if crate::recipe::parse(recipe).unwrap() == ("follow-up".to_string(), "johan's comment on 3f2a9c1".to_string())), "the follow-up start names its recipe: {started:?}");
        assert!(matches!(&started, Reopen::Start { name, repo, prompt, .. } if name == "refunds-in-part-followup" && repo == Path::new("/src/app") && prompt.starts_with("A follow-up to the task refunds-in-part, which pushed 3f2a9c1 and has ended.") && prompt.contains("Addresses johan's comment on 3f2a9c1.")), "a later task by the same name never gets it: {started:?}");
        let gone = |name: &str| name == "refunds-in-part-followup";
        assert!(matches!(reopen(&read, None, &[], &gone).unwrap(), Reopen::Start { name, .. } if name == "refunds-in-part-followup-2"), "a follow-up already running gets a sibling");
        let own = reopen(&read, Some("Use amount_cents, not amount."), &[], &|_| false).unwrap();
        assert!(matches!(&own, Reopen::Start { prompt, .. } if prompt.contains("The owner's own words on what to change:\nUse amount_cents, not amount.") && !prompt.contains("Sum the refunded")), "{own:?}");
        let question = comment(2, Some(Kind::Question), Status::Read);
        assert!(reopen(&question, None, &[], &running).is_err(), "a question has no proposal to reopen with");
        assert!(reopen(&question, Some("Sum them."), &[], &running).is_ok());
    }

    #[test]
    fn reopening_marks_the_card_with_the_task_that_fixes_it() {
        let state = crate::sh::test_dir("comments-reopen");
        crate::comments::save(&state, &comment(51, Some(Kind::Holds), Status::Read)).unwrap();
        let start = Reopen::Start { repo: PathBuf::from("/src/app"), name: "refunds-in-part-followup".to_string(), prompt: String::new(), recipe: String::new() };
        let marked = mark_reopened(&state, 51, &start, None, "app").unwrap();
        assert_eq!((marked.status, marked.fixing.as_deref()), (Status::Reopened, Some("refunds-in-part-followup@app")));
        assert_eq!(crate::comments::row_word(&marked), "fixing in refunds-in-part-followup");
        std::fs::remove_dir_all(state).unwrap();
    }
}
