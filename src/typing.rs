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

pub(crate) fn screen(pane: &str) -> Result<String> {
    crate::sh::run(&herdr::bin(), &["pane", "read", pane, "--source", "visible", "--format", "ansi"], None)
}

pub fn draft_in(pane: &str, agent: Agent) -> Draft {
    screen(pane).map(|screen| agent.draft(&screen)).unwrap_or(Draft::Unreadable)
}

const ENTER_AT: [u64; 3] = [5, 10, 20];
// A box read right after the paste can still be empty, before Codex draws the text.
const PASTE_SHOWS_MS: u64 = 1000;
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

// Claude records a prompt submitted while its turn runs as an enqueue without the text, and takes it only when the turn ends; with kebbie's text gone from the box, that enqueue is kebbie's. An enqueue with another text (a task notification) is not. Codex records a queued prompt only once its turn ends, so its box letting go of the text is the take, once settled: the box was seen holding it, or long enough has passed for a paste to show.
fn taken(agent: Agent, transcript: Option<&str>, shown: &Draft, text: &str, since_ms: u64, settled: bool) -> bool {
    let left_box = left_box(shown, text);
    if agent == Agent::Codex {
        return (left_box && settled) || transcript.is_some_and(|transcript| taken_in(transcript, text, since_ms));
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

// A box that still holds what was typed: its start, the window Claude scrolls a long draft to (its end), or the agent's fold of a long paste.
fn holds(box_text: &str, text: &str) -> bool {
    let (shown, text) = (normalized(box_text), normalized(text));
    let start: String = text.chars().take(40).collect();
    let shown_start: String = shown.chars().take(40).collect();
    let tail = |words: &str| words.chars().rev().take(40).collect::<String>();
    let begins = shown_start.chars().count() >= 10 && (shown_start.starts_with(&start) || start.starts_with(&shown_start));
    let window = shown.chars().count() >= 40 && (text.contains(&shown) || tail(&text) == tail(&shown));
    shown.contains("[Pasted text") || shown.contains("[Pasted Content") || begins || window
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
    pub agent: Agent,
    pub seen: bool,
}

impl Typed {
    fn to_json(&self) -> Value {
        json!({ "target": self.target, "label": self.label, "text": self.text, "at_ms": self.at_ms, "presses": self.presses, "inbox": self.inbox, "agent": self.agent.key(), "seen": self.seen })
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
            agent: value["agent"].as_str().and_then(|key| Agent::named(key).ok()).unwrap_or(Agent::Claude),
            seen: value["seen"] == true,
        })
    }
}

struct Place {
    pane: String,
    transcript: Option<PathBuf>,
    agent: Agent,
    running: bool,
}

fn place(target: &str) -> Place {
    let agents = herdr::agents().unwrap_or_default();
    let Some(agent) = agents.iter().find(|agent| agent["pane_id"] == target || agent["name"] == target) else { return Place { pane: target.to_string(), transcript: None, agent: Agent::Claude, running: false } };
    let pane = agent["pane_id"].as_str().unwrap_or(target).to_string();
    let session = agent["agent_session"]["value"].as_str().filter(|id| !id.is_empty());
    let cwd = agent["cwd"].as_str().map(Path::new);
    let kind = Agent::of(agent).unwrap_or(Agent::Claude);
    let transcript = session.zip(kind.home()).and_then(|(session, home)| kind.session_transcript(&home, cwd, session));
    Place { pane, transcript, agent: kind, running: true }
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
    let Place { pane, transcript, agent, .. } = place(&typed.target);
    let transcript = transcript.and_then(|file| std::fs::read_to_string(file).ok());
    if typed.inbox {
        return advance_inbox(typed, transcript.as_deref(), now_ms());
    }
    let mut live = Live { target: typed.target.clone(), pane, transcript: None, agent };
    let shown = live.read();
    let taken = live.settled_taken(typed, &shown, transcript.as_deref(), now_ms());
    let (outcome, press) = move_on(typed, &shown, taken, now_ms());
    if press {
        let _ = live.enter();
    }
    outcome
}

// One look at a typed text: the outcome so far, and whether Enter is pressed again now, only ever on a box that still shows the text.
fn move_on(typed: &mut Typed, shown: &Draft, taken: bool, now_ms: u64) -> (Outcome, bool) {
    match step(Duration::from_millis(now_ms.saturating_sub(typed.at_ms)), typed.presses, taken) {
        Step::Taken => (Outcome::Taken, false),
        Step::Wait => (Outcome::Waiting, false),
        Step::Enter => {
            typed.presses += 1;
            (Outcome::Waiting, matches!(shown, Draft::Typed(text) if holds(text, &typed.text)))
        }
        Step::GiveUp => (Outcome::NotDelivered, false),
    }
}

// The input box of an agent's pane as a start submits its prompt there.
pub(crate) trait InputBox {
    fn read(&mut self) -> Draft;
    fn transcript(&mut self) -> Option<String>;
    fn working(&mut self) -> bool;
    fn paste(&mut self, text: &str) -> Result<()>;
    fn enter(&mut self) -> Result<()>;
    fn now(&mut self) -> u64;
    fn pause(&mut self, ms: u64);

    fn settled_taken(&mut self, typed: &mut Typed, shown: &Draft, transcript: Option<&str>, now_ms: u64) -> bool {
        typed.seen |= matches!(shown, Draft::Typed(text) if holds(text, &typed.text));
        let settled = typed.seen || now_ms.saturating_sub(typed.at_ms) >= PASTE_SHOWS_MS;
        taken(typed.agent, transcript, shown, &typed.text, typed.at_ms, settled) || (typed.seen && left_box(shown, &typed.text) && self.working())
    }
}

struct Live {
    target: String,
    pane: String,
    transcript: Option<PathBuf>,
    agent: Agent,
}

impl Live {
    fn at(target: &str) -> (Live, bool) {
        let Place { pane, transcript, agent, running } = place(target);
        (Live { target: target.to_string(), pane, transcript, agent }, running)
    }
}

// A new conversation's transcript is written with its first message, so it is looked for again until it is there.
impl InputBox for Live {
    fn read(&mut self) -> Draft {
        draft_in(&self.pane, self.agent)
    }
    fn transcript(&mut self) -> Option<String> {
        if self.transcript.is_none() {
            self.transcript = place(&self.target).transcript;
        }
        self.transcript.as_ref().and_then(|file| std::fs::read_to_string(file).ok())
    }
    fn working(&mut self) -> bool {
        herdr::agents().unwrap_or_default().iter().any(|agent| agent["pane_id"] == self.pane.as_str() && agent["agent_status"] == "working")
    }
    fn paste(&mut self, text: &str) -> Result<()> {
        herdr::paste(&self.pane, text)
    }
    fn enter(&mut self) -> Result<()> {
        herdr::send_keys(&self.pane, &["enter"])
    }
    fn now(&mut self) -> u64 {
        now_ms()
    }
    fn pause(&mut self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

fn left_box(shown: &Draft, text: &str) -> bool {
    match shown {
        Draft::Empty => true,
        Draft::Typed(box_text) => !holds(box_text, text),
        Draft::Unreadable => false,
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Submitted {
    Taken,
    Busy(String),
    Unsent(Typed),
}

// A new Claude can still be drawing its banner and notices when herdr calls it ready, so the box is waited for this long before the prompt is held instead.
const BOX_SHOWS_MS: u64 = 30_000;
const PASTE_WAIT_MS: u64 = 5_000;
const LOOK_MS: u64 = 250;

// A start's prompt goes in as one bracketed paste into an empty box, and Enter is pressed apart from it once the box shows the paste, as herdr's Enter 300 ms after its text was seen lost on a Claude still starting; Enter is pressed again at 5, 10 and 20 s while the box holds it.
pub(crate) fn submit_start(input: &mut dyn InputBox, typed: Typed) -> Result<Submitted> {
    let began = input.now();
    loop {
        match input.read() {
            Draft::Empty => break,
            Draft::Typed(held) => return Ok(Submitted::Busy(format!("already holds \"{}\"", held.lines().next().unwrap_or_default().chars().take(60).collect::<String>()))),
            Draft::Unreadable if input.now().saturating_sub(began) >= BOX_SHOWS_MS => return Ok(Submitted::Busy("is not one kebbie can read as empty".to_string())),
            Draft::Unreadable => input.pause(LOOK_MS),
        }
    }
    let mut typed = Typed { at_ms: input.now(), presses: 0, seen: false, inbox: false, ..typed };
    input.paste(&typed.text)?;
    while !typed.seen {
        if matches!(input.read(), Draft::Typed(text) if holds(&text, &typed.text)) {
            typed.seen = true;
        } else if input.now().saturating_sub(typed.at_ms) >= PASTE_WAIT_MS {
            return Ok(Submitted::Busy(format!("did not show the prompt kebbie pasted within {} s", PASTE_WAIT_MS / 1000)));
        } else {
            input.pause(LOOK_MS);
        }
    }
    input.enter()?;
    loop {
        input.pause(1000);
        let shown = input.read();
        let transcript = input.transcript();
        let now = input.now();
        let taken = input.settled_taken(&mut typed, &shown, transcript.as_deref(), now);
        match move_on(&mut typed, &shown, taken, now) {
            (Outcome::Taken, _) => return Ok(Submitted::Taken),
            (Outcome::NotDelivered, _) => return Ok(Submitted::Unsent(typed)),
            (Outcome::Waiting, true) => input.enter()?,
            (Outcome::Waiting, false) => {}
        }
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
    format!("Not delivered to {}: {} did not take what kebbie typed into its input box within {GIVE_UP} s; it may still sit there. It began: {}", typed.label, typed.agent.name(), normalized(&typed.text).chars().take(120).collect::<String>())
}

fn typed_dir() -> Result<PathBuf> {
    let dir = crate::lead::state_dir()?.join("typed");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

fn type_text(target: &str, text: &str, agent: Agent) -> Result<Typed> {
    let at_ms = now_ms();
    herdr::prompt_agent(target, text)?;
    Ok(Typed { target: target.to_string(), label: String::new(), text: text.to_string(), at_ms, presses: 0, inbox: false, agent, seen: false })
}

// The board's check follows a message given to a session's inbox the same way, by its transcript alone.
pub fn watch_inbox_in(dir: &Path, target: &str, text: &str, label: &str, at_ms: u64) -> Result<()> {
    let typed = Typed { target: target.to_string(), label: label.to_string(), text: text.to_string(), at_ms, presses: 0, inbox: true, agent: Agent::Claude, seen: false };
    crate::state::write(&dir.join("typed").join(format!("{}.json", crate::messages::new_id("inbox"))), typed.to_json().to_string())
}

// Whether a typed text carries this part, whitespace aside, as a batch of messages carries each of them.
pub fn holds_part(typed: &str, part: &str) -> bool {
    let part = normalized(part);
    !part.is_empty() && normalized(typed).contains(&part)
}

// Whether the board still waits to see a text that carries this part taken by the session at this label.
pub fn watching_part_in(dir: &Path, label: &str, part: &str) -> bool {
    std::fs::read_dir(dir.join("typed")).into_iter().flatten().flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "json"))
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok().and_then(|text| Typed::from_json_text(&text)))
        .any(|typed| typed.label == label && holds_part(&typed.text, part))
}

// Types into a running conversation only when its input box is empty, so the user's draft is never typed over; the board then checks that Claude took it.
pub fn prompt(target: &str, text: &str, label: &str) -> std::result::Result<(), Refused> {
    let Place { pane, agent, .. } = place(target);
    match screen(&pane).map(|screen| agent.draft(&screen)) {
        Ok(Draft::Empty) => {}
        Ok(Draft::Typed(_)) => return Err(Refused::Draft),
        Ok(Draft::Unreadable) | Err(_) => return Err(Refused::Unreadable),
    }
    type_and_watch(target, text, label, agent)
}

fn type_and_watch(target: &str, text: &str, label: &str, agent: Agent) -> std::result::Result<(), Refused> {
    let mut typed = type_text(target, text, agent).map_err(Refused::Failed)?;
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
    Err(Refused::Failed(format!("its input box still held text after kebbie cleared it line by line; it began: {}", cleared.lines().next().unwrap_or_default())))
}

// Types even over the user's draft, which it clears first and returns, so the caller can log it; used only when the user or the lead forces a delivery.
pub fn prompt_forced(target: &str, text: &str, label: &str) -> std::result::Result<Option<String>, Refused> {
    let Place { pane, agent, .. } = place(target);
    let mut read = || screen(&pane).map(|screen| agent.draft(&screen)).unwrap_or(Draft::Unreadable);
    let mut press = || herdr::send_keys(&pane, &["ctrl+u"]);
    let cleared = clear_draft(&mut read, &mut press)?;
    type_and_watch(target, text, label, agent)?;
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
            Refused::Unreadable => "shows no input box kebbie can read".to_string(),
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
    if let Ok(state) = crate::lead::state_dir() {
        sweep_unsent_in(&state, &mut |typed| match Live::at(&typed.target) {
            (mut live, true) => live.read(),
            (_, false) => Draft::Empty,
        });
    }
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
                let again = if again { format!(" kebbie holds it and sends it again once {} is free.", typed.label) } else { String::new() };
                crate::restart::tell_board(&format!("{}{again}", not_delivered(&typed)));
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Unsent {
    Busy(String),
    Asks(crate::agent::Started),
    Unsubmitted,
}

// A start types only into an empty box, as what kebbie pastes would join a draft: Some says why its prompt is not under way, and a prompt left typed but not sent is kept for the board to send.
pub fn prompt_into_empty(target: &str, text: &str, label: &str) -> Result<Option<Unsent>> {
    let (mut live, running) = Live::at(target);
    if !running {
        return Err(format!("no agent runs as {target}: it did not start in its pane, or its shell asked something as it started"));
    }
    if live.agent == Agent::Codex {
        if let Some(unsent) = unsent_into_codex(&screen(&live.pane).unwrap_or_default()) {
            return Ok(Some(unsent));
        }
        return prompt_confirmed(target, text, label).map(|()| None);
    }
    let typed = Typed { target: target.to_string(), label: label.to_string(), text: text.to_string(), at_ms: 0, presses: 0, inbox: false, agent: live.agent, seen: false };
    match submit_start(&mut live, typed)? {
        Submitted::Taken => Ok(None),
        Submitted::Busy(busy) => Ok(Some(Unsent::Busy(busy))),
        Submitted::Unsent(typed) => {
            mark_unsent_in(&crate::lead::state_dir()?, &typed)?;
            Ok(Some(Unsent::Unsubmitted))
        }
    }
}

fn unsent_file(state: &Path, address: &str) -> PathBuf {
    state.join("unsent").join(format!("{}.json", address.replace('/', "_")))
}

pub(crate) fn mark_unsent_in(state: &Path, typed: &Typed) -> Result<()> {
    crate::state::write(&unsent_file(state, &typed.label), typed.to_json().to_string())
}

// A start prompt kebbie typed into the task's box and could not get sent; the board puts the task under Needs you until it is.
pub(crate) fn unsent_in(state: &Path, address: &str) -> Option<Typed> {
    std::fs::read_to_string(unsent_file(state, address)).ok().and_then(|text| Typed::from_json_text(&text))
}

pub(crate) fn forget_unsent(state: &Path, address: &str) {
    let _ = std::fs::remove_file(unsent_file(state, address));
}

// An unsent prompt is settled once the box no longer shows it: sent by the user's Enter, or cleared; a box kebbie cannot read keeps it.
fn sweep_unsent_in(state: &Path, read: &mut dyn FnMut(&Typed) -> Draft) {
    for entry in std::fs::read_dir(state.join("unsent")).into_iter().flatten().flatten() {
        let Some(typed) = std::fs::read_to_string(entry.path()).ok().and_then(|text| Typed::from_json_text(&text)) else {
            let _ = std::fs::remove_file(entry.path());
            continue;
        };
        if left_box(&read(&typed), &typed.text) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

const SEND_AGAIN_MS: u64 = 10_000;

// The board's ⏎ on a task whose start prompt sits unsent: Enter on the box that shows it, then wait for Claude to take it.
pub(crate) fn send_again(input: &mut dyn InputBox, typed: &Typed) -> Result<bool> {
    let shown = input.read();
    if !matches!(&shown, Draft::Typed(text) if holds(text, &typed.text)) {
        return Ok(left_box(&shown, &typed.text));
    }
    let mut typed = Typed { at_ms: input.now(), presses: 0, seen: true, ..typed.clone() };
    input.enter()?;
    while input.now().saturating_sub(typed.at_ms) < SEND_AGAIN_MS {
        input.pause(500);
        let shown = input.read();
        let transcript = input.transcript();
        let now = input.now();
        if input.settled_taken(&mut typed, &shown, transcript.as_deref(), now) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn send_unsent(address: &str) -> Result<String> {
    use crate::notice::{Notice, Tone};
    let state = crate::lead::state_dir()?;
    let Some(typed) = unsent_in(&state, address) else { return Notice::new(Tone::NotNow, "Nothing to send").next("Its start prompt is no longer waiting in its input box.").detail("task", address).ok() };
    let (mut live, running) = Live::at(&typed.target);
    if !running {
        forget_unsent(&state, address);
        return Notice::new(Tone::NotNow, "Its Claude is not running").next("g restarts it with its prompt.").detail("task", address).err();
    }
    if send_again(&mut live, &typed)? {
        forget_unsent(&state, address);
        return Notice::new(Tone::Done, "Sent its start prompt").detail("task", address).ok();
    }
    Notice::new(Tone::You, "Its start prompt is still not sent").you("Press Enter in its Space.").detail("task", address).err()
}

// A start-up question's menu opens its focused option with the same › as the composer, so the question is read before the box.
fn unsent_into_codex(screen: &str) -> Option<Unsent> {
    let plain = screen.lines().map(|line| styled(line).text).collect::<Vec<_>>().join("\n");
    if let Some(asked) = crate::agent::codex::startup_question(&plain) {
        return Some(Unsent::Asks(asked));
    }
    match Agent::Codex.draft(screen) {
        Draft::Empty => None,
        Draft::Typed(held) => Some(Unsent::Busy(format!("already holds \"{}\"", held.lines().next().unwrap_or_default().chars().take(60).collect::<String>()))),
        Draft::Unreadable => Some(Unsent::Busy("is not one kebbie can read as empty".to_string())),
    }
}

pub fn prompt_confirmed(target: &str, text: &str, label: &str) -> Result<()> {
    let mut typed = type_text(target, text, place(target).agent)?;
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

    const CODEX_TRUST: &str = "  \x1b[1mFolder access\x1b[0m\n\n  You are in /Users/me/src/app\n\n  Trust this folder? Codex can read and edit files here.\n\n\x1b[36m› 1. Trust and continue\x1b[0m\n  2. Quit\n\n  \x1b[2menter continue · esc quit\x1b[0m";

    #[test]
    fn a_codex_start_up_question_is_reported_as_that_question_and_never_as_text_in_its_composer() {
        assert_eq!(unsent_into_codex(CODEX_TRUST), Some(Unsent::Asks(crate::agent::Started::AwaitsTrust)), "the trust menu's \"› 1. Trust and continue\" is no draft");
        assert_eq!(unsent_into_codex("\x1b[1mTrust this folder?\x1b[0m\n› 1. Trust and \x1b[1mcontinue\x1b[0m"), Some(Unsent::Asks(crate::agent::Started::AwaitsTrust)), "styling between the words is dropped before the question is read");
        assert_eq!(unsent_into_codex("› \x1b[2mAsk Codex to do anything\x1b[0m\n\n  100% context left"), None);
        assert_eq!(unsent_into_codex("› check the deploy\n\n  100% context left"), Some(Unsent::Busy("already holds \"check the deploy\"".to_string())));
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
        let advance = &source[source.find("pub fn advance").unwrap()..source.find("\nfn move_on(").unwrap()];
        assert!(advance.contains("live.enter()") && !advance.contains("prompt_agent") && !advance.contains("paste("), "a retry presses Enter, it never types the text again");
        assert!(source.contains("fn enter(&mut self) -> Result<()> {\n        herdr::send_keys(&self.pane, &[\"enter\"])"));
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
        assert!(taken(Agent::Claude, Some(&queued_for_the_turns_end), &Draft::Empty, answer, typed_at, true), "submitted while a turn ran: Claude records the enqueue without the text and takes it when the turn ends, 17 minutes later in the case seen");
        assert!(taken(Agent::Claude, Some(&queued_for_the_turns_end), &Draft::Typed("my own words".to_string()), answer, typed_at, true), "the user typing after it does not undo it");
        assert!(!taken(Agent::Claude, Some(&queued_for_the_turns_end), &Draft::Typed("[from you] The user's answer to \"Treat".to_string()), answer, typed_at, true), "an enqueue while kebbie's text still sits in the box is someone else's");
        assert!(!taken(Agent::Claude, Some(&queued_for_the_turns_end), &Draft::Unreadable, answer, typed_at, true));
        assert!(!taken(Agent::Claude, Some(&queued_for_the_turns_end), &Draft::Empty, answer, typed_at + 60_000, true), "an enqueue from before kebbie typed is not its");
        assert!(taken(Agent::Claude, None, &Draft::Empty, answer, typed_at, true) && !taken(Agent::Claude, None, &Draft::Typed(answer.to_string()), answer, typed_at, true), "without a transcript, an empty box is the sign");
        let notification = json!({ "type": "queue-operation", "operation": "enqueue", "timestamp": "2026-10-22T08:39:48.500Z", "content": "<task-notification>\n<task-type>artifact-auto-react</task-type>" }).to_string();
        assert!(!taken(Agent::Claude, Some(&notification), &Draft::Empty, answer, typed_at, true), "a notification Claude queued after kebbie typed is not kebbie's text: the CI log seen logged delivered and never taken");
        let old_queue = json!({ "type": "queue-operation", "operation": "enqueue", "timestamp": "2026-10-20T11:00:00.000Z", "content": "[from lead] rebase onto main please" }).to_string();
        assert!(!taken_in(&old_queue, "[from lead] rebase onto main please", at));
    }

    #[test]
    fn a_text_kebbie_could_not_deliver_names_the_agent_it_was_typed_to() {
        let typed = |agent: Agent| Typed { target: "w1:p1".to_string(), label: "billing@app".to_string(), text: "Fix the flaky test".to_string(), at_ms: 0, presses: 3, inbox: false, agent, seen: false };
        assert!(not_delivered(&typed(Agent::Codex)).starts_with("Not delivered to billing@app: Codex did not take what kebbie typed"), "{}", not_delivered(&typed(Agent::Codex)));
        assert!(not_delivered(&typed(Agent::Claude)).contains(": Claude did not take"));
        assert_eq!(Typed::from_json_text(&typed(Agent::Codex).to_json().to_string()).map(|typed| typed.agent), Some(Agent::Codex), "a check after a restart still names it");
        assert_eq!(Typed::from_json_text(r#"{"target":"w1:p1","text":"x","at_ms":1}"#).map(|typed| typed.agent), Some(Agent::Claude), "a text an older kebbie typed");
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
        assert!(taken(Agent::Codex, Some(""), &Draft::Empty, text, at, true), "a prompt Codex queued mid-turn is in no rollout until the turn ends, but its box is empty");
        assert!(!taken(Agent::Codex, Some(""), &Draft::Typed(text.to_string()), text, at, true));
        assert!(!taken(Agent::Codex, Some(""), &Draft::Typed("[Pasted Content 1005 chars]".to_string()), text, at, true), "Codex folds a long paste");
        assert!(taken(Agent::Codex, Some(&legacy), &Draft::Unreadable, text, at, true));
        assert!(!taken(Agent::Claude, Some(""), &Draft::Empty, text, at, true), "Claude's rule stays its own");
        assert!(!taken(Agent::Codex, Some(""), &Draft::Empty, text, at, false), "a box read right after the paste, before Codex drew the text, has not let it go");
        assert!(taken(Agent::Codex, Some(&legacy), &Draft::Empty, text, at, false), "the rollout says it whatever the box shows");
    }

    // A Claude in a pane, as a start meets it: its box unreadable while it draws its banner, a paste shown as Claude shows it or swallowed, and Enter dropped this many times.
    struct Claude {
        now: u64,
        drawn_at: u64,
        held: Option<String>,
        drops: usize,
        shows: fn(&str) -> String,
        pastes: Vec<String>,
        enters: Vec<u64>,
        sent: bool,
        swallows: bool,
    }

    impl Claude {
        fn new(drawn_at: u64, drops: usize, shows: fn(&str) -> String) -> Claude {
            Claude { now: 0, drawn_at, held: None, drops, shows, pastes: Vec::new(), enters: Vec::new(), sent: false, swallows: false }
        }
    }

    impl InputBox for Claude {
        fn read(&mut self) -> Draft {
            if self.now < self.drawn_at {
                return Draft::Unreadable;
            }
            match &self.held {
                Some(text) => draft(&screen_with(&[&(self.shows)(text)])),
                None => Draft::Empty,
            }
        }
        fn transcript(&mut self) -> Option<String> {
            None
        }
        fn working(&mut self) -> bool {
            self.sent
        }
        fn paste(&mut self, text: &str) -> Result<()> {
            assert!(self.now >= self.drawn_at, "pasted before the box was drawn");
            self.pastes.push(text.to_string());
            self.held = (!self.swallows).then(|| text.to_string());
            Ok(())
        }
        fn enter(&mut self) -> Result<()> {
            self.enters.push(self.now);
            if self.held.is_some() && self.drops > 0 {
                self.drops -= 1;
            } else if self.held.take().is_some() {
                self.sent = true;
            }
            Ok(())
        }
        fn now(&mut self) -> u64 {
            self.now
        }
        fn pause(&mut self, ms: u64) {
            self.now += ms;
        }
    }

    const QUEUED_PROMPT: &str = "This task waited for partner-first-launch@kattmat-ios, which is done. Its commits (subjects as that task wrote them):\n> b37c18ef Add the partner onboarding screen\n> 929e513c Ship the first-launch flag\n\nWire the first launch of the partner app to the onboarding screen and keep the flag off for existing users; the tests in the iOS target cover both paths and run on the simulator.";

    // Claude 2.1.311 and 2.1.325 on this Mac draw a long typed draft scrolled to its end, the box's first line cut mid-word.
    fn scrolled(text: &str) -> String {
        let flat = normalized(text);
        let cut = flat.char_indices().nth(flat.chars().count().saturating_sub(120)).map_or(0, |(at, _)| at);
        format!("❯ {}", &flat[cut..])
    }

    fn folded(_: &str) -> String {
        "❯ [Pasted text #1 +5 lines]".to_string()
    }

    fn typed_for(text: &str) -> Typed {
        Typed { target: "partner-first-launch".to_string(), label: "partner-first-launch@kattmat-ios".to_string(), text: text.to_string(), at_ms: 0, presses: 0, inbox: false, agent: Agent::Claude, seen: false }
    }

    #[test]
    fn a_long_prompt_claude_shows_scrolled_to_its_end_still_counts_as_kebbies_text_in_the_box() {
        let shown = draft(&screen_with(&["❯ r sentence for length. Filler sentence for length. Filler sentence for length.", "  Reply with only the word OK and do nothing else."]));
        let text = format!("This task waited for billing@api, which is done.\n\n{}Reply with only the word OK and do nothing else.", "Filler sentence for length. ".repeat(40));
        assert!(matches!(&shown, Draft::Typed(box_text) if holds(box_text, &text)), "the box as herdr read it on a live Claude: {shown:?}");
        assert!(!holds("fix the flaky test", &text) && !holds("Reply with only", &text), "a short draft of the user's own is never taken for kebbie's text");
        let mut typed = Typed { at_ms: 0, ..typed_for(&text) };
        assert_eq!(move_on(&mut typed, &shown, false, 5_000), (Outcome::Waiting, true), "Enter is pressed again on the scrolled box");
    }

    #[test]
    fn a_start_prompt_waits_for_the_drawn_box_is_pasted_once_and_enter_follows_once_the_paste_shows() {
        let mut claude = Claude::new(3_000, 1, scrolled);
        assert_eq!(submit_start(&mut claude, typed_for(QUEUED_PROMPT)), Ok(Submitted::Taken));
        assert_eq!(claude.pastes, [QUEUED_PROMPT], "pasted once, never typed again");
        assert!(claude.enters.len() == 2 && claude.enters[0] >= 3_000 && claude.enters[1] >= 5_000, "the dropped Enter is pressed again at 5 s: {:?}", claude.enters);
        let mut folding = Claude::new(0, 0, folded);
        assert_eq!(submit_start(&mut folding, typed_for(QUEUED_PROMPT)), Ok(Submitted::Taken));
        assert_eq!(folding.enters.len(), 1);
        let source = include_str!("typing.rs");
        let body = &source[source.find("pub(crate) fn submit_start(").unwrap()..];
        let body = &body[..body.find("\n}\n").unwrap()];
        assert!(body.contains("input.paste(") && !body.contains("prompt_agent"), "herdr's agent prompt sends Enter 300 ms after the text, which a starting Claude can lose");
    }

    #[test]
    fn a_resumed_claude_that_goes_busy_on_an_inbox_message_and_loses_the_paste_is_busy_so_its_prompt_is_held() {
        let mut resumed = Claude::new(0, 0, scrolled);
        resumed.swallows = true;
        assert_eq!(submit_start(&mut resumed, typed_for(QUEUED_PROMPT)), Ok(Submitted::Busy(format!("did not show the prompt kebbie pasted within {} s", PASTE_WAIT_MS / 1000))), "the 8f8dea4 release smoke's restart lost its prompt here");
        assert!(resumed.enters.is_empty() && resumed.now >= PASTE_WAIT_MS, "nothing is sent into a box that never showed the paste");
    }

    #[test]
    fn a_start_prompt_claude_never_takes_is_pressed_four_times_then_reported_unsent() {
        let mut claude = Claude::new(0, usize::MAX, scrolled);
        let Ok(Submitted::Unsent(typed)) = submit_start(&mut claude, typed_for(QUEUED_PROMPT)) else { panic!("an Enter that never lands is reported") };
        assert_eq!((claude.pastes.len(), claude.enters.len(), typed.presses), (1, 4, 3), "Enter once after the paste, again at 5, 10 and 20 s");
        assert!(claude.now >= GIVE_UP * 1000 && claude.held.as_deref() == Some(QUEUED_PROMPT));
        let mut drafted = Claude::new(0, 0, scrolled);
        drafted.held = Some("my own half sentence".to_string());
        assert_eq!(submit_start(&mut drafted, typed_for(QUEUED_PROMPT)), Ok(Submitted::Busy("already holds \"my own half sentence\"".to_string())));
        assert!(drafted.pastes.is_empty() && drafted.enters.is_empty(), "nothing joins the user's draft");
        let mut never = Claude::new(u64::MAX, 0, scrolled);
        assert_eq!(submit_start(&mut never, typed_for(QUEUED_PROMPT)), Ok(Submitted::Busy("is not one kebbie can read as empty".to_string())));
    }

    #[test]
    fn an_unsent_start_prompt_stays_marked_until_its_box_lets_it_go_and_the_boards_enter_sends_it() {
        let state = crate::sh::test_dir("typing-unsent");
        let typed = typed_for(QUEUED_PROMPT);
        mark_unsent_in(&state, &typed).unwrap();
        assert_eq!(unsent_in(&state, &typed.label).map(|kept| kept.text), Some(QUEUED_PROMPT.to_string()));
        sweep_unsent_in(&state, &mut |_| Draft::Unreadable);
        sweep_unsent_in(&state, &mut |kept| draft(&screen_with(&[&scrolled(&kept.text)])));
        assert!(unsent_in(&state, &typed.label).is_some(), "a box kebbie cannot read, or one that still shows it, keeps it under Needs you");
        sweep_unsent_in(&state, &mut |_| Draft::Empty);
        assert!(unsent_in(&state, &typed.label).is_none(), "the user's own Enter settles it");
        let mut claude = Claude::new(0, 0, scrolled);
        claude.held = Some(QUEUED_PROMPT.to_string());
        assert_eq!(send_again(&mut claude, &typed), Ok(true));
        assert_eq!(claude.enters.len(), 1);
        let mut stuck = Claude::new(0, usize::MAX, scrolled);
        stuck.held = Some(QUEUED_PROMPT.to_string());
        assert_eq!(send_again(&mut stuck, &typed), Ok(false));
        let mut gone = Claude::new(0, 0, scrolled);
        assert_eq!(send_again(&mut gone, &typed), Ok(true), "an empty box has nothing left to send");
        assert!(gone.enters.is_empty());
        let _ = std::fs::remove_dir_all(&state);
    }
}
