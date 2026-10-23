use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::agent::Agent;
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

// One screen line as plain text, with whether each character is drawn faint (SGR 2).
pub(crate) struct Styled {
    pub(crate) text: String,
    faint: Vec<bool>,
}

// Reads SGR sequences for faint and drops every other escape; 38/48 colour arguments are skipped, so the 2 in 38;2;r;g;b is not taken for faint.
pub(crate) fn styled(line: &str) -> Styled {
    let mut text = String::new();
    let mut faint = Vec::new();
    let mut dim = false;
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut end = None;
                    for next in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            end = Some(next);
                            break;
                        }
                        params.push(next);
                    }
                    if end == Some('m') {
                        dim = sgr_faint(&params, dim);
                    }
                }
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' || (next == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => {}
            _ => {
                text.push(character);
                faint.push(dim);
            }
        }
    }
    Styled { text, faint }
}

fn sgr_faint(params: &str, mut dim: bool) -> bool {
    let mut codes = params.split(';');
    while let Some(code) = codes.next() {
        match code {
            "" | "0" | "22" => dim = false,
            "2" => dim = true,
            "38" | "48" | "58" => match codes.next() {
                Some("5") => { codes.next(); }
                Some("2") => { codes.nth(2); }
                _ => {}
            },
            _ => {}
        }
    }
    dim
}

// What the user has typed into Claude's input box on this screen, read with its styling; a screen without a box it can read is Unreadable, never Empty.
pub fn draft(screen: &str) -> Draft {
    let styled: Vec<Styled> = screen.lines().map(styled).collect();
    let lines: Vec<&str> = styled.iter().map(|line| line.text.trim_end()).collect();
    let Some(prompt) = (1..lines.len()).rev().find(|&at| lines[at].trim_start().trim_start_matches('│').trim_start().starts_with('❯') && is_border(lines[at - 1])) else {
        return Draft::Unreadable;
    };
    let Some(bottom) = (prompt + 1..lines.len()).find(|&at| is_border(lines[at])) else { return Draft::Unreadable };
    let inside = |line: &str| line.trim().trim_matches('│').trim().to_string();
    let first = inside(lines[prompt]).trim_start_matches('❯').trim().to_string();
    let text: Vec<String> = std::iter::once(first).chain(lines[prompt + 1..bottom].iter().map(|line| inside(line))).filter(|line| !line.is_empty()).collect();
    let text = text.join("\n");
    if text.is_empty() || suggested(&styled[prompt..bottom], &['❯', '│']) {
        return Draft::Empty;
    }
    Draft::Typed(text)
}

// An agent draws what nobody typed faint: Claude its suggested next prompt, a fresh conversation's example, the hint about queued messages; Codex its placeholder. One character not faint makes it the user's.
pub(crate) fn suggested(lines: &[Styled], marks: &[char]) -> bool {
    lines.iter().all(|line| line.text.chars().zip(&line.faint).all(|(character, faint)| *faint || character.is_whitespace() || marks.contains(&character)))
}

fn screen(pane: &str) -> Result<String> {
    crate::sh::run(&herdr::bin(), &["pane", "read", pane, "--source", "visible", "--format", "ansi"], None)
}

pub fn draft_in(pane: &str, agent: Agent) -> Draft {
    screen(pane).map(|screen| agent.draft(&screen)).unwrap_or(Draft::Unreadable)
}

const ENTER_AT: [u64; 3] = [5, 10, 20];
const GIVE_UP: u64 = 40;
// Claude reads an inbox message only once the running tool returns, so a long test run must not count as a drop.
const INBOX_GIVE_UP: u64 = 600;

#[derive(Debug, PartialEq)]
pub enum Step {
    Taken,
    Wait,
    Enter,
    GiveUp,
}

// After a prompt is typed: Enter again at 5, 10 and 20 seconds while Claude has not taken it, never the text again, and after 40 seconds it is not delivered.
pub fn step(elapsed: Duration, presses: usize, taken: bool) -> Step {
    step_with(elapsed, presses, taken, false)
}

// A message given to Claude Code's inbox is only waited for: there is no Enter to press, and Claude takes it between tool calls.
pub fn step_with(elapsed: Duration, presses: usize, taken: bool, inbox: bool) -> Step {
    let seconds = elapsed.as_secs();
    if taken {
        Step::Taken
    } else if seconds >= if inbox { INBOX_GIVE_UP } else { GIVE_UP } {
        Step::GiveUp
    } else if !inbox && ENTER_AT.get(presses).is_some_and(|at| seconds >= *at) {
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

// Claude took the prompt when its transcript has it, after it was typed, as the user's message, or as a message queued while it worked: enqueued at once, absorbed only when the running tool returns.
pub fn taken_in(transcript: &str, text: &str, since_ms: u64) -> bool {
    normalized(text).is_empty() || said_at(transcript, text, since_ms, true).is_some()
}

// When Claude read the text, as the user's message or a queued one it absorbed; an enqueue alone is not read yet.
pub fn read_at(transcript: &str, text: &str, since_ms: u64) -> Option<u64> {
    said_at(transcript, text, since_ms, false)
}

fn said_at(transcript: &str, text: &str, since_ms: u64, enqueued: bool) -> Option<u64> {
    let wanted: String = normalized(text).chars().take(200).collect();
    transcript.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).find_map(|record| {
        let at = record["timestamp"].as_str().and_then(iso_millis).filter(|at| at + 2000 >= since_ms)?;
        let said = if record["type"] == "user" {
            content_text(&record["message"]["content"])
        } else if let Some(text) = crate::agent::codex::user_text(&record) {
            text
        } else if record["type"] == "attachment" && record["attachment"]["type"] == "queued_command" {
            content_text(&record["attachment"]["prompt"])
        } else if enqueued && record["type"] == "queue-operation" && record["operation"] == "enqueue" {
            content_text(&record["content"])
        } else {
            return None;
        };
        normalized(&said).contains(&wanted).then_some(at)
    })
}

// Claude records a prompt submitted while its turn runs as an enqueue without the text, and takes it only when the turn ends; with kelpie's text gone from the box, that enqueue is kelpie's. An enqueue with another text (a task notification) is not. Codex records a queued prompt only once its turn ends, so its box letting go of the text is the take.
fn taken(agent: Agent, transcript: Option<&str>, shown: &Draft, text: &str, since_ms: u64) -> bool {
    let left_box = match shown {
        Draft::Empty => true,
        Draft::Typed(box_text) => !holds(box_text, text),
        Draft::Unreadable => false,
    };
    if agent == Agent::Codex {
        return left_box || transcript.is_some_and(|transcript| taken_in(transcript, text, since_ms));
    }
    match transcript {
        Some(transcript) => taken_in(transcript, text, since_ms) || (left_box && queued_since(transcript, since_ms)),
        None => *shown == Draft::Empty,
    }
}

fn queued_since(transcript: &str, since_ms: u64) -> bool {
    transcript.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).any(|record| {
        let textless = record["content"].as_str().is_none_or(|content| content.trim().is_empty());
        record["type"] == "queue-operation" && record["operation"] == "enqueue" && textless && record["timestamp"].as_str().and_then(iso_millis).is_some_and(|at| at + 2000 >= since_ms)
    })
}

// A box that still holds what was typed: its start, or the agent's fold of a long paste.
fn holds(box_text: &str, text: &str) -> bool {
    let shown = normalized(box_text);
    let start: String = normalized(text).chars().take(40).collect();
    let shown_start: String = shown.chars().take(40).collect();
    shown.contains("[Pasted text") || shown.contains("[Pasted Content") || (shown_start.chars().count() >= 10 && (shown_start.starts_with(&start) || start.starts_with(&shown_start)))
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_millis() as u64).unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Typed {
    pub target: String,
    pub label: String,
    pub text: String,
    pub at_ms: u64,
    pub presses: usize,
    pub inbox: bool,
}

impl Typed {
    fn to_json(&self) -> Value {
        json!({ "target": self.target, "label": self.label, "text": self.text, "at_ms": self.at_ms, "presses": self.presses, "inbox": self.inbox })
    }

    pub fn from_json_text(text: &str) -> Option<Typed> {
        Typed::from_json(&serde_json::from_str(text).ok()?)
    }

    fn from_json(value: &Value) -> Option<Typed> {
        Some(Typed {
            target: value["target"].as_str()?.to_string(),
            label: value["label"].as_str().unwrap_or_default().to_string(),
            text: value["text"].as_str()?.to_string(),
            at_ms: value["at_ms"].as_u64()?,
            presses: value["presses"].as_u64().unwrap_or(0) as usize,
            inbox: value["inbox"] == true,
        })
    }
}

struct Place {
    pane: String,
    transcript: Option<PathBuf>,
    agent: Agent,
}

fn place(target: &str) -> Place {
    let agents = herdr::agents().unwrap_or_default();
    let Some(agent) = agents.iter().find(|agent| agent["pane_id"] == target || agent["name"] == target) else { return Place { pane: target.to_string(), transcript: None, agent: Agent::Claude } };
    let pane = agent["pane_id"].as_str().unwrap_or(target).to_string();
    let session = agent["agent_session"]["value"].as_str().filter(|id| !id.is_empty());
    let cwd = agent["cwd"].as_str().map(Path::new);
    let kind = Agent::of(agent).unwrap_or(Agent::Claude);
    let transcript = session.zip(kind.home()).and_then(|(session, home)| kind.session_transcript(&home, cwd, session));
    Place { pane, transcript, agent: kind }
}

pub fn transcript_of(target: &str) -> Option<PathBuf> {
    place(target).transcript
}

// Whether the agent in this pane has taken the text since it was typed, by its transcript; false when there is none to read.
pub fn taken_by(target: &str, text: &str, since_ms: u64) -> bool {
    place(target).transcript.and_then(|file| std::fs::read_to_string(file).ok()).is_some_and(|transcript| taken_in(&transcript, text, since_ms))
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Taken,
    Waiting,
    NotDelivered,
}

pub fn advance(typed: &mut Typed) -> Outcome {
    let Place { pane, transcript, agent } = place(&typed.target);
    let transcript = transcript.and_then(|file| std::fs::read_to_string(file).ok());
    if typed.inbox {
        return advance_inbox(typed, transcript.as_deref(), now_ms());
    }
    let shown = draft_in(&pane, agent);
    let taken = taken(agent, transcript.as_deref(), &shown, &typed.text, typed.at_ms);
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

pub fn advance_inbox(typed: &Typed, transcript: Option<&str>, now_ms: u64) -> Outcome {
    let taken = transcript.is_some_and(|transcript| taken_in(transcript, &typed.text, typed.at_ms));
    match step_with(Duration::from_millis(now_ms.saturating_sub(typed.at_ms)), typed.presses, taken, true) {
        Step::Taken => Outcome::Taken,
        Step::GiveUp => Outcome::NotDelivered,
        Step::Wait | Step::Enter => Outcome::Waiting,
    }
}

pub fn not_delivered(typed: &Typed) -> String {
    if typed.inbox {
        return format!("Not delivered to {}: Claude Code's inbox took it, but its conversation does not show it after {} minutes; the session may hold or drop messages from other sessions (crossSessionInbound). It began: {}", typed.label, INBOX_GIVE_UP / 60, normalized(&typed.text).chars().take(120).collect::<String>());
    }
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
    Ok(Typed { target: target.to_string(), label: String::new(), text: text.to_string(), at_ms, presses: 0, inbox: false })
}

// The board's check follows a message given to a session's inbox the same way, by its transcript alone.
pub fn watch_inbox_in(dir: &Path, target: &str, text: &str, label: &str, at_ms: u64) -> Result<()> {
    let typed = Typed { target: target.to_string(), label: label.to_string(), text: text.to_string(), at_ms, presses: 0, inbox: true };
    crate::state::write(&dir.join("typed").join(format!("{}.json", crate::messages::new_id("inbox"))), typed.to_json().to_string())
}

// Whether the board still waits to see this text taken by the session at this label.
pub fn watching_in(dir: &Path, label: &str, text: &str) -> bool {
    std::fs::read_dir(dir.join("typed")).into_iter().flatten().flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "json"))
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok().and_then(|text| Typed::from_json_text(&text)))
        .any(|typed| typed.label == label && typed.text == text)
}

// Types into a running conversation only when its input box is empty, so the user's draft is never typed over; the board then checks that Claude took it.
pub fn prompt(target: &str, text: &str, label: &str) -> std::result::Result<(), Refused> {
    let Place { pane, agent, .. } = place(target);
    match screen(&pane).map(|screen| agent.draft(&screen)) {
        Ok(Draft::Empty) => {}
        Ok(Draft::Typed(_)) => return Err(Refused::Draft),
        Ok(Draft::Unreadable) | Err(_) => return Err(Refused::Unreadable),
    }
    type_and_watch(target, text, label)
}

fn type_and_watch(target: &str, text: &str, label: &str) -> std::result::Result<(), Refused> {
    let mut typed = type_text(target, text).map_err(Refused::Failed)?;
    typed.label = label.to_string();
    if let Ok(dir) = typed_dir() {
        let _ = crate::state::write(&dir.join(format!("{}.json", crate::messages::new_id("typed"))), typed.to_json().to_string());
    }
    Ok(())
}

// Ctrl+U deletes one line of the draft per press and Claude keeps it for Ctrl+Y; Ctrl+C would interrupt a turn and Esc leaves the text.
pub fn clear_draft(read: &mut dyn FnMut() -> Draft, press: &mut dyn FnMut() -> Result<()>) -> std::result::Result<Option<String>, Refused> {
    let cleared = match read() {
        Draft::Empty => return Ok(None),
        Draft::Unreadable => return Err(Refused::Unreadable),
        Draft::Typed(text) => text,
    };
    for _ in 0..=cleared.lines().count() {
        press().map_err(Refused::Failed)?;
        match read() {
            Draft::Empty => return Ok(Some(cleared)),
            Draft::Unreadable => return Err(Refused::Unreadable),
            Draft::Typed(_) => {}
        }
    }
    Err(Refused::Failed(format!("its input box still held text after kelpie cleared it line by line; it began: {}", cleared.lines().next().unwrap_or_default())))
}

// Types even over the user's draft, which it clears first and returns, so the caller can log it; used only when the user or the lead forces a delivery.
pub fn prompt_forced(target: &str, text: &str, label: &str) -> std::result::Result<Option<String>, Refused> {
    let Place { pane, agent, .. } = place(target);
    let mut read = || screen(&pane).map(|screen| agent.draft(&screen)).unwrap_or(Draft::Unreadable);
    let mut press = || herdr::send_keys(&pane, &["ctrl+u"]);
    let cleared = clear_draft(&mut read, &mut press)?;
    type_and_watch(target, text, label)?;
    Ok(cleared)
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
                let again = crate::messages::hold_again(&typed.label, &typed.text).unwrap_or(false);
                let again = if again { format!(" kelpie holds it and sends it again once {} is free.", typed.label) } else { String::new() };
                crate::restart::tell_board(&format!("{}{again}", not_delivered(&typed)));
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
    }

    #[test]
    fn what_claude_draws_faint_in_the_box_is_nobodys_draft_and_one_character_not_faint_is_the_users() {
        let rule = "\x1b[0m\x1b[38;2;136;136;136m────────────────────────────────────────\x1b[0m\r";
        let boxed = |line: &str| ["\x1b[0m\x1b[38;2;153;153;153m✻\x1b[0m done\r", rule, line, rule, "  \x1b[0m\x1b[38;2;255;193;7m⏵⏵ auto mode on\x1b[0m"].join("\n");
        assert_eq!(draft(&boxed("❯\u{a0}\x1b[0m\x1b[2mgodkänt, skriv planen\x1b[0m\r")), Draft::Empty, "the suggestion herdr read on a live pane");
        assert_eq!(draft(&boxed("\x1b[39m❯\u{a0}\x1b[2mTry \"write a test for main.rs\"\x1b[0m")), Draft::Empty, "a fresh conversation's example");
        assert_eq!(draft(&boxed("\x1b[38;5;246m❯\u{a0}\x1b[2m\x1b[39mPress up to edit queued messages\x1b[0m")), Draft::Empty, "the hint while a message waits in Claude's queue");
        assert_eq!(draft(&boxed("\x1b[39m❯\u{a0}fix the flaky test")), Draft::Typed("fix the flaky test".to_string()), "typed text is drawn plain");
        assert_eq!(draft(&boxed("\x1b[39m❯\u{a0}y\x1b[2mes, save it\x1b[0m")), Draft::Typed("yes, save it".to_string()), "one typed character makes the box the user's");
        assert_eq!(draft(&boxed("❯ \x1b[38;2;255;193;7mcoloured but typed\x1b[0m")), Draft::Typed("coloured but typed".to_string()), "a 2 inside a 38;2 colour is not faint");
        assert_eq!(draft(&boxed("❯ \x1b[2mfaint\x1b[22m bold-off ends it")), Draft::Typed("faint bold-off ends it".to_string()));
        assert_eq!(draft(&boxed("❯ \x1b]8;;https://x\x07link\x1b]8;;\x07")), Draft::Typed("link".to_string()), "an OSC link is dropped, its text kept");
        assert_eq!(draft(&boxed("❯ Try \"how does src/board.rs work?\"")), Draft::Typed("Try \"how does src/board.rs work?\"".to_string()), "without styling nothing is taken for a suggestion");
        let source = include_str!("typing.rs");
        assert!(source.contains("\"--source\", \"visible\", \"--format\", \"ansi\""), "the box is read with its styling, or a suggestion reads as typed");
    }

    #[test]
    fn a_forced_delivery_clears_the_draft_with_ctrl_u_line_by_line_and_returns_what_it_cleared() {
        let screens = std::cell::RefCell::new(vec![Draft::Typed("line a\nline b".to_string()), Draft::Typed("line a".to_string()), Draft::Empty].into_iter());
        let presses = std::cell::Cell::new(0);
        let cleared = clear_draft(&mut || screens.borrow_mut().next().unwrap(), &mut || { presses.set(presses.get() + 1); Ok(()) });
        assert_eq!((cleared, presses.get()), (Ok(Some("line a\nline b".to_string())), 2));
        assert_eq!(clear_draft(&mut || Draft::Empty, &mut || panic!("an empty box is not pressed")), Ok(None));
        assert_eq!(clear_draft(&mut || Draft::Unreadable, &mut || panic!("a box it cannot see is not pressed")), Err(Refused::Unreadable));
        let stuck = clear_draft(&mut || Draft::Typed("won't go".to_string()), &mut || Ok(()));
        assert!(matches!(stuck, Err(Refused::Failed(ref error)) if error.contains("still held text")), "{stuck:?}");
        let source = include_str!("typing.rs");
        let forced = &source[source.find("pub fn prompt_forced").unwrap()..];
        assert!(forced.contains("&[\"ctrl+u\"]") && !forced[..forced.find("\n}\n").unwrap()].contains("ctrl+c"), "Ctrl+C would interrupt Claude's turn");
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
        let enqueued = json!({ "type": "queue-operation", "operation": "enqueue", "timestamp": "2026-10-20T12:00:01.000Z", "content": "[from lead] rebase onto main please" }).to_string();
        assert!(taken_in(&enqueued, "[from lead] rebase onto main please", at), "Claude queued it mid-turn; it is absorbed only when the running tool returns, often after 40 s");
        let dequeued = json!({ "type": "queue-operation", "operation": "remove", "timestamp": "2026-10-20T12:00:01.000Z", "content": "[from lead] rebase onto main please" }).to_string();
        assert!(!taken_in(&dequeued, "[from lead] rebase onto main please", at));
        assert_eq!(read_at(&enqueued, "[from lead] rebase onto main please", at), None, "queued is not read yet");
        assert_eq!(read_at(&format!("{enqueued}\n{queued}"), "[from lead] rebase onto main please", at), Some(at + 4000), "read when Claude absorbed it");
        assert_eq!(read_at(&user, "[from lead] rebase onto main please", at), Some(at + 3100));
        let typed_at = iso_millis("2026-10-22T08:39:47.300Z").unwrap();
        let queued_for_the_turns_end = json!({ "type": "queue-operation", "operation": "enqueue", "timestamp": "2026-10-22T08:39:47.581Z", "sessionId": "s" }).to_string();
        let answer = "[from you] The user's answer to \"Treat Claude's dim suggestion as an empty box\": Approve";
        assert!(taken(Agent::Claude, Some(&queued_for_the_turns_end), &Draft::Empty, answer, typed_at), "submitted while a turn ran: Claude records the enqueue without the text and takes it when the turn ends, 17 minutes later in the case seen");
        assert!(taken(Agent::Claude, Some(&queued_for_the_turns_end), &Draft::Typed("my own words".to_string()), answer, typed_at), "the user typing after it does not undo it");
        assert!(!taken(Agent::Claude, Some(&queued_for_the_turns_end), &Draft::Typed("[from you] The user's answer to \"Treat".to_string()), answer, typed_at), "an enqueue while kelpie's text still sits in the box is someone else's");
        assert!(!taken(Agent::Claude, Some(&queued_for_the_turns_end), &Draft::Unreadable, answer, typed_at));
        assert!(!taken(Agent::Claude, Some(&queued_for_the_turns_end), &Draft::Empty, answer, typed_at + 60_000), "an enqueue from before kelpie typed is not its");
        assert!(taken(Agent::Claude, None, &Draft::Empty, answer, typed_at) && !taken(Agent::Claude, None, &Draft::Typed(answer.to_string()), answer, typed_at), "without a transcript, an empty box is the sign");
        let notification = json!({ "type": "queue-operation", "operation": "enqueue", "timestamp": "2026-10-22T08:39:48.500Z", "content": "<task-notification>\n<task-type>artifact-auto-react</task-type>" }).to_string();
        assert!(!taken(Agent::Claude, Some(&notification), &Draft::Empty, answer, typed_at), "a notification Claude queued after kelpie typed is not kelpie's text: the CI log seen logged delivered and never taken");
        let old_queue = json!({ "type": "queue-operation", "operation": "enqueue", "timestamp": "2026-10-20T11:00:00.000Z", "content": "[from lead] rebase onto main please" }).to_string();
        assert!(!taken_in(&old_queue, "[from lead] rebase onto main please", at));
    }

    #[test]
    fn a_message_typed_to_codex_is_taken_once_its_rollout_has_it_or_its_composer_lets_it_go() {
        let at = iso_millis("2026-10-22T09:16:00.000Z").unwrap();
        let text = "[from lead] rebase onto main please";
        let legacy = json!({ "timestamp": "2026-10-22T09:16:01.200Z", "type": "event_msg", "payload": { "type": "user_message", "message": text, "images": [] } }).to_string();
        let paginated = json!({ "timestamp": "2026-10-22T09:16:02.000Z", "type": "event_msg", "payload": { "type": "item_completed", "turn_id": "t", "item": { "type": "UserMessage", "id": "u", "content": [{ "type": "text", "text": text, "text_elements": [] }] } } }).to_string();
        assert_eq!(read_at(&legacy, text, at), Some(at + 1200), "Codex's legacy rollout");
        assert_eq!(read_at(&paginated, text, at), Some(at + 2000), "Codex's paginated rollout");
        assert!(!taken_in(&json!({ "timestamp": "2026-10-22T09:16:01.000Z", "type": "event_msg", "payload": { "type": "agent_message", "message": text } }).to_string(), text, at), "the agent saying it is not the user's turn");
        assert!(taken(Agent::Codex, Some(""), &Draft::Empty, text, at), "a prompt Codex queued mid-turn is in no rollout until the turn ends, but its box is empty");
        assert!(!taken(Agent::Codex, Some(""), &Draft::Typed(text.to_string()), text, at));
        assert!(!taken(Agent::Codex, Some(""), &Draft::Typed("[Pasted Content 1005 chars]".to_string()), text, at), "Codex folds a long paste");
        assert!(taken(Agent::Codex, Some(&legacy), &Draft::Unreadable, text, at));
        assert!(!taken(Agent::Claude, Some(""), &Draft::Empty, text, at), "Claude's rule stays its own");
    }
}
