use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::herdr;
use crate::sh::Result;

#[derive(Debug, PartialEq)]
pub enum Draft {
    Empty,
    Typed(String),
    Unreadable,
}

// Claude's input box is a rule of dashes above and below its ❯ line; the top rule can carry a label among its dashes.
pub fn is_border(line: &str) -> bool {
    let line = line.trim().trim_matches(['╭', '╮', '╰', '╯']);
    let dashes = line.chars().filter(|character| *character == '─').count();
    line.starts_with("───") && line.ends_with('─') && dashes >= 8
}

// What the user has typed into Claude's input box on this screen; a screen without a box that can be read counts as typed into.
pub fn draft(screen: &str) -> Draft {
    let lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
    let Some(prompt) = (1..lines.len()).rev().find(|&at| lines[at].trim_start().trim_start_matches('│').trim_start().starts_with('❯') && is_border(lines[at - 1])) else {
        return Draft::Unreadable;
    };
    let Some(bottom) = (prompt + 1..lines.len()).find(|&at| is_border(lines[at])) else { return Draft::Unreadable };
    let inside = |line: &str| line.trim().trim_matches('│').trim().to_string();
    let first = inside(lines[prompt]).trim_start_matches('❯').trim().to_string();
    let text: Vec<String> = std::iter::once(first).chain(lines[prompt + 1..bottom].iter().map(|line| inside(line))).filter(|line| !line.is_empty()).collect();
    let text = text.join("\n");
    if text.is_empty() || placeholder(&text) {
        return Draft::Empty;
    }
    Draft::Typed(text)
}

// A fresh conversation's box shows an example to try, which nobody typed.
fn placeholder(text: &str) -> bool {
    !text.contains('\n') && text.starts_with("Try \"") && text.ends_with('"')
}

fn screen(pane: &str) -> Result<String> {
    crate::sh::run(&herdr::bin(), &["pane", "read", pane, "--source", "visible"], None)
}

const ENTER_AT: [u64; 3] = [5, 10, 20];
const GIVE_UP: u64 = 40;

#[derive(Debug, PartialEq)]
pub enum Step {
    Taken,
    Wait,
    Enter,
    GiveUp,
}

// After a prompt is typed: Enter again at 5, 10 and 20 seconds while Claude has not taken it, never the text again, and after 40 seconds it is not delivered.
pub fn step(elapsed: Duration, presses: usize, taken: bool) -> Step {
    let seconds = elapsed.as_secs();
    if taken {
        Step::Taken
    } else if seconds >= GIVE_UP {
        Step::GiveUp
    } else if ENTER_AT.get(presses).is_some_and(|at| seconds >= *at) {
        Step::Enter
    } else {
        Step::Wait
    }
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().filter_map(|part| part["text"].as_str().or(part["content"].as_str()).or(part.as_str())).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

pub fn iso_millis(stamp: &str) -> Option<u64> {
    let (date, time) = stamp.trim_end_matches('Z').split_once('T')?;
    let mut date = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut clock = clock.split(':').map(|part| part.parse::<i64>().ok());
    let (hour, minute, second) = (clock.next()??, clock.next()??, clock.next()??);
    let millis: i64 = format!("{:0<3}", fraction.chars().take(3).collect::<String>()).parse().ok()?;
    let (y, m) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let era = y.div_euclid(400);
    let of_era = y - era * 400;
    let of_year = (153 * m + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    let days = era * 146_097 + of_cycle - 719_468;
    u64::try_from(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis).ok()
}

// Claude took the prompt when its transcript has it, after it was typed, as the user's message or as a message queued while it worked.
pub fn taken_in(transcript: &str, text: &str, since_ms: u64) -> bool {
    let wanted: String = normalized(text).chars().take(200).collect();
    if wanted.is_empty() {
        return true;
    }
    transcript.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).any(|record| {
        let recent = record["timestamp"].as_str().and_then(iso_millis).is_some_and(|at| at + 2000 >= since_ms);
        let said = if record["type"] == "user" {
            content_text(&record["message"]["content"])
        } else if record["type"] == "attachment" && record["attachment"]["type"] == "queued_command" {
            content_text(&record["attachment"]["prompt"])
        } else {
            return false;
        };
        recent && normalized(&said).contains(&wanted)
    })
}

// A box that still holds what was typed: its start, or Claude's fold of a long paste.
fn holds(box_text: &str, text: &str) -> bool {
    let shown = normalized(box_text);
    let start: String = normalized(text).chars().take(40).collect();
    let shown_start: String = shown.chars().take(40).collect();
    shown.contains("[Pasted text") || (shown_start.chars().count() >= 10 && (shown_start.starts_with(&start) || start.starts_with(&shown_start)))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_millis() as u64).unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Typed {
    pub target: String,
    pub label: String,
    pub text: String,
    pub at_ms: u64,
    pub presses: usize,
}

impl Typed {
    fn to_json(&self) -> Value {
        json!({ "target": self.target, "label": self.label, "text": self.text, "at_ms": self.at_ms, "presses": self.presses })
    }

    fn from_json(value: &Value) -> Option<Typed> {
        Some(Typed {
            target: value["target"].as_str()?.to_string(),
            label: value["label"].as_str().unwrap_or_default().to_string(),
            text: value["text"].as_str()?.to_string(),
            at_ms: value["at_ms"].as_u64()?,
            presses: value["presses"].as_u64().unwrap_or(0) as usize,
        })
    }
}

fn place(target: &str) -> (String, Option<PathBuf>) {
    let agents = herdr::agents().unwrap_or_default();
    let Some(agent) = agents.iter().find(|agent| agent["pane_id"] == target || agent["name"] == target) else { return (target.to_string(), None) };
    let pane = agent["pane_id"].as_str().unwrap_or(target).to_string();
    let session = agent["agent_session"]["value"].as_str().filter(|id| !id.is_empty());
    let cwd = agent["cwd"].as_str().map(Path::new);
    let transcript = session.zip(crate::agent::claude::home()).and_then(|(session, home)| crate::restart::transcript(&home, cwd, session));
    (pane, transcript)
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Taken,
    Waiting,
    NotDelivered,
}

pub fn advance(typed: &mut Typed) -> Outcome {
    let (pane, transcript) = place(&typed.target);
    let shown = screen(&pane).map(|screen| draft(&screen)).unwrap_or(Draft::Unreadable);
    let taken = match transcript.and_then(|file| std::fs::read_to_string(file).ok()) {
        Some(transcript) => taken_in(&transcript, &typed.text, typed.at_ms),
        None => shown == Draft::Empty,
    };
    let elapsed = Duration::from_millis(now_ms().saturating_sub(typed.at_ms));
    match step(elapsed, typed.presses, taken) {
        Step::Taken => Outcome::Taken,
        Step::Wait => Outcome::Waiting,
        Step::Enter => {
            typed.presses += 1;
            if matches!(&shown, Draft::Typed(text) if holds(text, &typed.text)) {
                let _ = herdr::send_keys(&pane, &["enter"]);
            }
            Outcome::Waiting
        }
        Step::GiveUp => Outcome::NotDelivered,
    }
}

pub fn not_delivered(typed: &Typed) -> String {
    format!("Not delivered to {}: Claude did not take what kelpie typed into its input box within {GIVE_UP} s; it may still sit there. It began: {}", typed.label, normalized(&typed.text).chars().take(120).collect::<String>())
}

fn typed_dir() -> Result<PathBuf> {
    let dir = crate::lead::state_dir()?.join("typed");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

fn type_text(target: &str, text: &str) -> Result<Typed> {
    let at_ms = now_ms();
    herdr::prompt_agent(target, text)?;
    Ok(Typed { target: target.to_string(), label: String::new(), text: text.to_string(), at_ms, presses: 0 })
}

// Types into a running conversation only when its input box is empty, so the user's draft is never typed over; the board then checks that Claude took it.
pub fn prompt(target: &str, text: &str, label: &str) -> std::result::Result<(), Refused> {
    let (pane, _) = place(target);
    match screen(&pane).map(|screen| draft(&screen)) {
        Ok(Draft::Empty) => {}
        Ok(Draft::Typed(_)) => return Err(Refused::Draft),
        Ok(Draft::Unreadable) | Err(_) => return Err(Refused::Unreadable),
    }
    let mut typed = type_text(target, text).map_err(Refused::Failed)?;
    typed.label = label.to_string();
    if let Ok(dir) = typed_dir() {
        let _ = crate::state::write(&dir.join(format!("{}.json", crate::messages::new_id("typed"))), typed.to_json().to_string());
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
pub enum Refused {
    Draft,
    Unreadable,
    Failed(String),
}

impl Refused {
    pub fn reason(&self) -> String {
        match self {
            Refused::Draft => "has text typed in its input box".to_string(),
            Refused::Unreadable => "shows no input box kelpie can read".to_string(),
            Refused::Failed(error) => error.clone(),
        }
    }
}

// Two boards look at once (the lead's and an overlay); one presses Enter.
struct CheckLock(PathBuf);

impl CheckLock {
    fn take(path: &Path) -> Option<CheckLock> {
        if std::fs::metadata(path).and_then(|meta| meta.modified()).is_ok_and(|at| at.elapsed().unwrap_or_default() > Duration::from_secs(120)) {
            let _ = std::fs::remove_file(path);
        }
        std::fs::OpenOptions::new().write(true).create_new(true).open(path).ok().map(|_| CheckLock(path.to_path_buf()))
    }
}

impl Drop for CheckLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn check() {
    let Ok(dir) = typed_dir() else { return };
    let Some(_lock) = CheckLock::take(&dir.join("check.lock")) else { return };
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Some(mut typed) = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|value| Typed::from_json(&value)) else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        match advance(&mut typed) {
            Outcome::Taken => {
                let _ = std::fs::remove_file(&path);
            }
            Outcome::Waiting => {
                let _ = crate::state::write(&path, typed.to_json().to_string());
            }
            Outcome::NotDelivered => {
                let _ = std::fs::remove_file(&path);
                crate::restart::tell_board(&not_delivered(&typed));
            }
        }
    }
}

pub fn prompt_confirmed(target: &str, text: &str, label: &str) -> Result<()> {
    let mut typed = type_text(target, text)?;
    typed.label = label.to_string();
    loop {
        match advance(&mut typed) {
            Outcome::Taken => return Ok(()),
            Outcome::Waiting => std::thread::sleep(Duration::from_secs(1)),
            Outcome::NotDelivered => return Err(not_delivered(&typed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &str = "────────────────────────────────────────────────────────────";

    fn screen_with(input: &[&str]) -> String {
        let mut lines = vec!["⏺ Done. The tests pass.", "", RULE];
        lines.extend(input);
        lines.extend([RULE, "  ⏵⏵ accept edits on (shift+tab to cycle)"]);
        lines.join("\n")
    }

    #[test]
    fn an_empty_box_is_empty_and_any_text_in_it_is_the_users_draft() {
        assert_eq!(draft(&screen_with(&["❯ "])), Draft::Empty);
        assert_eq!(draft(&screen_with(&["❯"])), Draft::Empty);
        assert_eq!(draft(&screen_with(&["❯ fix the flaky test"])), Draft::Typed("fix the flaky test".to_string()));
        assert_eq!(draft(&screen_with(&["❯ first line", "  second line"])), Draft::Typed("first line\nsecond line".to_string()));
        assert_eq!(draft(&screen_with(&["❯ Try \"how does src/board.rs work?\""])), Draft::Empty, "the example a fresh conversation shows is nobody's draft");
    }

    #[test]
    fn a_top_border_with_a_label_among_its_dashes_still_frames_the_box() {
        let labelled = "──────────────────── billing · high effort · fast ──";
        assert!(is_border(labelled) && is_border(RULE) && !is_border("── a short note about dashes and more words here ──"));
        let screen = ["⏺ Asked.", "", labelled, "❯ half a sentence", RULE, "  ? for shortcuts"].join("\n");
        assert_eq!(draft(&screen), Draft::Typed("half a sentence".to_string()));
        let empty = ["⏺ Asked.", labelled, "❯ ", RULE].join("\n");
        assert_eq!(draft(&empty), Draft::Empty);
        let boxed = ["╭──────────────────────────────────────╮", "│ ❯ typed in an old box               │", "╰──────────────────────────────────────╯"].join("\n");
        assert_eq!(draft(&boxed), Draft::Typed("typed in an old box".to_string()));
    }

    #[test]
    fn a_screen_without_a_box_that_can_be_read_counts_as_not_empty() {
        assert_eq!(draft(""), Draft::Unreadable);
        assert_eq!(draft("$ ls\nsrc  Cargo.toml\n$ "), Draft::Unreadable);
        assert_eq!(draft("Do you trust this folder?\n❯ 1. Yes, I trust this folder\n  2. No, exit"), Draft::Unreadable, "a menu's cursor is not the input box");
        assert_eq!(draft(&[RULE, "❯ cut off at the bottom"].join("\n")), Draft::Unreadable);
    }

    #[test]
    fn enter_is_pressed_again_at_five_ten_and_twenty_seconds_and_the_prompt_is_given_up_at_forty() {
        let at = Duration::from_secs;
        assert_eq!(step(at(1), 0, false), Step::Wait);
        assert_eq!(step(at(5), 0, false), Step::Enter);
        assert_eq!(step(at(6), 1, false), Step::Wait);
        assert_eq!(step(at(10), 1, false), Step::Enter);
        assert_eq!(step(at(19), 2, false), Step::Wait);
        assert_eq!(step(at(20), 2, false), Step::Enter);
        assert_eq!(step(at(30), 3, false), Step::Wait, "three presses, then only waiting");
        assert_eq!(step(at(40), 3, false), Step::GiveUp);
        assert_eq!(step(at(45), 0, true), Step::Taken, "taken wins at any time");
        let source = include_str!("typing.rs");
        let advance = &source[source.find("pub fn advance").unwrap()..source.find("pub fn not_delivered").unwrap()];
        assert!(advance.contains("send_keys(&pane, &[\"enter\"])") && !advance.contains("prompt_agent"), "a retry presses Enter, it never types the text again");
        assert!(holds("[Pasted text #1 +40 lines]", "long") && holds("[from lead] check the", "[from lead] check the tests") && !holds("my own words", "[from lead] check"));
    }

    #[test]
    fn a_prompt_is_taken_when_the_transcript_has_it_as_a_user_message_or_a_queued_one_after_it_was_typed() {
        let at = iso_millis("2026-10-20T12:00:00.000Z").unwrap();
        assert_eq!(iso_millis("1970-01-01T00:00:01.5Z"), Some(1500));
        assert_eq!(iso_millis("2000-03-01T00:00:00Z"), Some(951_868_800_000));
        let user = json!({ "type": "user", "timestamp": "2026-10-20T12:00:03.100Z", "message": { "role": "user", "content": "[from lead] rebase   onto\nmain please" } }).to_string();
        let queued = json!({ "type": "attachment", "timestamp": "2026-10-20T12:00:04.000Z", "attachment": { "type": "queued_command", "prompt": "[from lead] rebase onto main please" } }).to_string();
        let old = json!({ "type": "user", "timestamp": "2026-10-20T11:00:00.000Z", "message": { "content": [{ "type": "text", "text": "[from lead] rebase onto main please" }] } }).to_string();
        assert!(taken_in(&user, "[from lead] rebase onto main please", at));
        assert!(taken_in(&format!("not json\n{queued}"), "[from lead] rebase onto main please", at));
        assert!(!taken_in(&old, "[from lead] rebase onto main please", at), "the same words an hour earlier are an earlier message");
        assert!(!taken_in(&user, "[from lead] something else", at));
    }
}
