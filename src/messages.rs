use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use serde_json::{Value, json};

use crate::board::{FAINT, MAUVE, MUTED, PEACH, SUBTEXT, SURFACE, TEAL, TEXT, YELLOW, bold, style};
use crate::herdr;
use crate::lead::state_dir;
use crate::peers::{self, Session};
use crate::sh::Result;
use crate::status::LEAD_LABEL;

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub id: String,
    pub at: u64,
    pub from: String,
    pub to: String,
    pub text: String,
    pub held: bool,
    pub report: Option<Report>,
}

// What a task's Claude says at the end of a turn: where it stands, and what the user should do next.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub state: String,
    pub next: String,
}

impl Message {
    // A report the user has nothing to do for says "nothing".
    pub fn next_for_you(&self) -> Option<&str> {
        let next = self.report.as_ref()?.next.trim();
        (!next.is_empty() && !next.eq_ignore_ascii_case("nothing")).then_some(next)
    }
}

fn log_file() -> Result<PathBuf> {
    Ok(state_dir()?.join("messages.jsonl"))
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

// The log is append-only: a message line, and for a held one a later line with only its id and "delivered".
pub fn fold(log: &str) -> Vec<Message> {
    let mut messages: Vec<Message> = Vec::new();
    for record in log.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
        let id = record["id"].as_str().unwrap_or_default();
        let held = record["state"] == "held";
        match record["text"].as_str() {
            Some(text) => messages.push(Message {
                id: id.to_string(),
                at: record["at"].as_u64().unwrap_or(0),
                from: record["from"].as_str().unwrap_or_default().to_string(),
                to: record["to"].as_str().unwrap_or_default().to_string(),
                text: text.to_string(),
                held,
                report: record["report"]["state"].as_str().map(|state| Report {
                    state: state.to_string(),
                    next: record["report"]["next"].as_str().unwrap_or_default().to_string(),
                }),
            }),
            None => {
                if let Some(message) = messages.iter_mut().find(|message| message.id == id) {
                    message.held = held;
                }
            }
        }
    }
    messages
}

pub fn load() -> Vec<Message> {
    log_file().and_then(|file| fs::read_to_string(file).map_err(|error| error.to_string())).map(|log| fold(&log)).unwrap_or_default()
}

fn append(record: &Value) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(log_file()?).map_err(|error| error.to_string())?;
    writeln!(file, "{record}").map_err(|error| error.to_string())
}

// Nothing is ever typed into a question: a session waiting for an answer, or with no Claude, gets the message later.
pub fn hold_reason(session: &Session) -> Option<&'static str> {
    match (&session.pane, session.status.as_deref()) {
        (None, _) => Some("has no Claude running"),
        (Some(_), Some("blocked")) => Some("is waiting for an answer"),
        _ => None,
    }
}

fn deliver(session: &Session, from: &str, text: &str) -> std::result::Result<(), &'static str> {
    if let Some(reason) = hold_reason(session) {
        return Err(reason);
    }
    let pane = session.pane.as_deref().unwrap_or_default();
    match herdr::prompt_agent(pane, &format!("[from {from}] {text}")) {
        Ok(()) => Ok(()),
        Err(error) if error.contains("agent_blocked") => Err("is waiting for an answer"),
        Err(_) => Err("could not be reached"),
    }
}

// One sender at a time, so a held message is never delivered twice; a lock older than a minute is left over from a crash.
struct Lock(PathBuf);

impl Lock {
    fn take(wait: bool) -> Result<Option<Lock>> {
        let path = state_dir()?.join("messages.lock");
        for _ in 0..100 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Some(Lock(path))),
                Err(_) if fs::metadata(&path).and_then(|metadata| metadata.modified()).is_ok_and(|time| time.elapsed().unwrap_or_default() > Duration::from_secs(60)) => {
                    let _ = fs::remove_file(&path);
                }
                Err(_) if wait => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => return Ok(None),
            }
        }
        Err("Another kelpie is holding the message log; try again".to_string())
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn flush_held(sessions: &[Session]) -> Result<usize> {
    let mut delivered = 0;
    for message in load().into_iter().filter(|message| message.held) {
        let Some(session) = sessions.iter().find(|session| session.address == message.to) else { continue };
        if deliver(session, &message.from, &message.text).is_ok() {
            append(&json!({ "id": message.id, "at": now(), "state": "delivered" }))?;
            delivered += 1;
        }
    }
    Ok(delivered)
}

// Held messages to sessions that are free again; the messages pane runs this on every tick.
pub fn flush() -> Result<usize> {
    if !load().iter().any(|message| message.held) {
        return Ok(0);
    }
    let Some(_lock) = Lock::take(false)? else { return Ok(0) };
    flush_held(&peers::sessions()?)
}

fn send(from: &str, recipients: &[&Session], text: &str, sessions: &[Session]) -> Result<Vec<String>> {
    let _lock = Lock::take(true)?;
    flush_held(sessions)?;
    let mut report = Vec::new();
    for (index, session) in recipients.iter().enumerate() {
        let earlier_held = load().iter().any(|message| message.held && message.to == session.address);
        let outcome = if earlier_held { Err("still has earlier messages held") } else { deliver(session, from, text) };
        let id = format!("{}-{}-{index}", now(), std::process::id());
        append(&json!({ "id": id, "at": now(), "from": from, "to": session.address, "text": text, "state": if outcome.is_ok() { "delivered" } else { "held" } }))?;
        report.push(match outcome {
            Ok(()) => format!("Told {}.", session.address),
            Err(reason) => format!("{} {reason}; held, and delivered when it is free.", session.address),
        });
    }
    Ok(report)
}

pub fn tell(cwd: &Path, to: &str, text: &str) -> Result<String> {
    if text.trim().is_empty() {
        return Err("kelpie tell needs a message: kelpie tell <task|task@repo|lead> \"<text>\"".to_string());
    }
    let from = peers::me(cwd).ok_or("kelpie tell runs in a task's worktree or in the lead Space")?;
    let sessions = peers::sessions()?;
    let recipients = peers::resolve(to, &from, &sessions)?;
    Ok(send(&from, &recipients, text.trim(), &sessions)?.join("\n"))
}

// When a task starts in another repo, the halves already running learn of it.
pub fn announce(task: &str, repo: &str) -> Result<()> {
    let sessions = peers::sessions()?;
    let me = peers::address(task, repo);
    let Ok(halves) = peers::resolve(task, &me, &sessions) else { return Ok(()) };
    send("kelpie", &halves, &format!("{me} has started: the same task, in {repo}. Keep it informed of changes it depends on."), &sessions).map(|_| ())
}

// A report goes to the lead through the log only: typed into its chat, every turn of every task would start a turn of the lead.
pub fn log_report(from: &str, report: &Report, text: &str) -> Result<()> {
    let id = format!("{}-{}-report", now(), std::process::id());
    append(&json!({ "id": id, "at": now(), "from": from, "to": LEAD_LABEL, "text": text, "state": "logged", "report": { "state": report.state, "next": report.next } }))
}

// Each session's latest report, by its address.
pub fn latest_reports(messages: &[Message]) -> HashMap<String, Message> {
    messages.iter().filter(|message| message.report.is_some()).map(|message| (message.from.clone(), message.clone())).collect()
}

// What sessions told each other, and the task reports that wait on the user.
fn shown_in_log(messages: Vec<Message>) -> Vec<Message> {
    messages.into_iter().filter(|message| message.report.as_ref().is_none_or(|report| report.state != "working")).collect()
}

fn addressee(message: &Message) -> String {
    match &message.report {
        Some(report) => format!("you · {}", report.state.replace('-', " ")),
        None => message.to.clone(),
    }
}

pub fn age(at: u64) -> String {
    let seconds = now().saturating_sub(at);
    match seconds {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

pub fn print(count: usize) -> String {
    let messages = shown_in_log(load());
    if messages.is_empty() {
        return "No messages yet.".to_string();
    }
    messages[messages.len().saturating_sub(count)..].iter().map(|message| {
        let held = if message.held { "  (held)" } else { "" };
        let next = message.next_for_you().map(|next| format!("\n      you: {next}")).unwrap_or_default();
        format!("{:>4}  {} → {}{held}\n      {}{next}", age(message.at), message.from, addressee(message), message.text)
    }).collect::<Vec<_>>().join("\n")
}

// The lead's copy: what sessions told each other since it last looked, and their latest reports, given to it with each of the user's prompts.
pub fn unread_for_lead() -> Result<String> {
    let mark = state_dir()?.join("messages-lead-read");
    let seen: usize = fs::read_to_string(&mark).ok().and_then(|count| count.trim().parse().ok()).unwrap_or(0);
    let messages = load();
    fs::write(&mark, messages.len().to_string()).map_err(|error| error.to_string())?;
    let (reports, told): (Vec<&Message>, Vec<&Message>) = copies_for_lead(&messages, seen).into_iter().partition(|message| message.report.is_some());
    let mut sections = Vec::new();
    if !told.is_empty() {
        let lines: Vec<String> = told.iter().map(|message| {
            let held = if message.held { " (held)" } else { "" };
            format!("- {} → {}{held}: {}", message.from, message.to, message.text)
        }).collect();
        sections.push(format!("Messages between sessions since you last looked (kelpie messages --print for more):\n{}", lines.join("\n")));
    }
    let latest = latest_reports(&reports.into_iter().cloned().collect::<Vec<_>>());
    if !latest.is_empty() {
        let mut lines: Vec<String> = latest.values().map(report_line).collect();
        lines.sort();
        sections.push(format!("Task reports since you last looked (each task's latest; kelpie status --json has them all):\n{}", lines.join("\n")));
    }
    Ok(sections.join("\n\n"))
}

pub fn report_line(message: &Message) -> String {
    let state = message.report.as_ref().map(|report| report.state.as_str()).unwrap_or_default();
    format!("- {} ({state}): {} — you: {}", message.from, message.text, message.next_for_you().unwrap_or("nothing"))
}

// Messages to or from the lead are already in its chat; reports to it are not.
fn copies_for_lead(messages: &[Message], seen: usize) -> Vec<&Message> {
    messages.iter().skip(seen.min(messages.len()))
        .filter(|message| message.report.is_some() || (message.from != LEAD_LABEL && message.to != LEAD_LABEL)).collect()
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

fn message_lines(message: &Message, width: usize) -> Vec<Line<'static>> {
    let mut head = vec![
        Span::styled(format!("{:>4} ", age(message.at)), style(FAINT)),
        Span::styled(message.from.clone(), style(TEAL)),
        Span::styled(" → ", style(MUTED)),
        Span::styled(addressee(message), style(if message.report.is_some() { PEACH } else { TEAL })),
    ];
    if message.held {
        head.push(Span::styled("  held", style(PEACH)));
    }
    let mut lines = vec![Line::from(head)];
    lines.extend(wrap(&message.text, width.saturating_sub(5).max(10)).into_iter().map(|text| Line::from(vec![Span::raw("     "), Span::styled(text, style(SUBTEXT))])));
    if let Some(next) = message.next_for_you() {
        lines.extend(wrap(&format!("you: {next}"), width.saturating_sub(5).max(10)).into_iter().map(|text| Line::from(vec![Span::raw("     "), Span::styled(text, style(YELLOW))])));
    }
    lines.push(Line::default());
    lines
}

fn draw(frame: &mut Frame, messages: &[Message], scroll: usize) {
    let [title, rule, body] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)]).areas(frame.area());
    let width = body.width as usize;
    let held = messages.iter().filter(|message| message.held).count();
    let right = if held > 0 { format!("{held} held · {}", messages.len()) } else { messages.len().to_string() };
    let gap = width.saturating_sub(8 + right.chars().count()).max(1);
    frame.render_widget(Paragraph::new(Line::from(vec![
        Span::styled("MESSAGES", bold(MAUVE)), Span::raw(" ".repeat(gap)), Span::styled(right, style(if held > 0 { PEACH } else { MUTED })),
    ])), title);
    frame.render_widget(Paragraph::new(Line::from(Span::styled("─".repeat(width), style(SURFACE)))), rule);
    let mut lines: Vec<Line> = messages.iter().flat_map(|message| message_lines(message, width)).collect();
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("Sessions' messages to each other show here,", style(MUTED))));
        lines.push(Line::from(Span::styled("and task reports that wait on you.", style(MUTED))));
    }
    let height = body.height as usize;
    let bottom = lines.len().saturating_sub(height);
    let top = bottom.saturating_sub(scroll);
    let shown: Vec<Line> = lines.into_iter().skip(top).take(height).collect();
    frame.render_widget(Paragraph::new(shown).style(style(TEXT)), body);
}

// The log, newest at the bottom; ↑ ↓ scroll back and forth, and the pane delivers held messages as sessions free up.
pub fn run() -> Result<String> {
    let mut terminal = ratatui::init();
    let mut scroll = 0usize;
    let mut flushed = SystemTime::UNIX_EPOCH;
    let result = (|| -> Result<()> {
        loop {
            if flushed.elapsed().unwrap_or_default() > Duration::from_secs(2) {
                let _ = flush();
                flushed = SystemTime::now();
            }
            let messages = shown_in_log(load());
            terminal.draw(|frame| draw(frame, &messages, scroll)).map_err(|error| error.to_string())?;
            if event::poll(Duration::from_millis(500)).map_err(|error| error.to_string())?
                && let Event::Key(key) = event::read().map_err(|error| error.to_string())?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Up | KeyCode::Char('k') => scroll += 1,
                    KeyCode::Down | KeyCode::Char('j') => scroll = scroll.saturating_sub(1),
                    KeyCode::End | KeyCode::Char('G') => scroll = 0,
                    _ => {}
                }
            }
        }
    })();
    ratatui::restore();
    result.map(|_| String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(pane: Option<&str>, status: Option<&str>) -> Session {
        Session { address: "billing@web".to_string(), task: Some("billing".to_string()), pane: pane.map(str::to_string), status: status.map(str::to_string) }
    }

    #[test]
    fn a_session_waiting_for_an_answer_or_without_claude_is_held() {
        assert_eq!(hold_reason(&session(Some("w1:p1"), Some("blocked"))), Some("is waiting for an answer"));
        assert_eq!(hold_reason(&session(None, None)), Some("has no Claude running"));
        for status in ["idle", "working", "done"] {
            assert_eq!(hold_reason(&session(Some("w1:p1"), Some(status))), None);
        }
    }

    #[test]
    fn the_log_folds_into_messages_with_their_latest_state() {
        let log = [
            r#"{"id":"1","at":10,"from":"billing@api","to":"billing@web","text":"person_id replaces user_id","state":"held"}"#,
            r#"{"id":"2","at":11,"from":"lead","to":"search@api","text":"rebase on main","state":"delivered"}"#,
            "not json",
            r#"{"id":"1","at":20,"state":"delivered"}"#,
            r#"{"id":"3","at":30,"from":"billing@web","to":"billing@api","text":"done","state":"held"}"#,
        ].join("\n");
        let messages = fold(&log);
        assert_eq!(messages.len(), 3);
        assert_eq!((messages[0].text.as_str(), messages[0].held, messages[0].at), ("person_id replaces user_id", false, 10));
        assert!(!messages[1].held);
        assert!(messages[2].held);
    }

    #[test]
    fn a_held_message_is_delivered_once_the_log_records_it() {
        let held = r#"{"id":"1","at":10,"from":"a@x","to":"b@x","text":"hi","state":"held"}"#;
        assert!(fold(held)[0].held);
        assert!(!fold(&format!("{held}\n{{\"id\":\"1\",\"at\":11,\"state\":\"delivered\"}}"))[0].held);
    }

    #[test]
    fn the_lead_is_copied_on_new_messages_between_others() {
        let message = |id: &str, from: &str, to: &str| Message { id: id.to_string(), at: 0, from: from.to_string(), to: to.to_string(), text: String::new(), held: false, report: None };
        let messages = [message("1", "a@x", "b@x"), message("2", "a@x", "lead"), message("3", "lead", "b@x"), message("4", "b@x", "a@x")];
        let ids = |seen| copies_for_lead(&messages, seen).iter().map(|message| message.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(0), ["1", "4"]);
        assert_eq!(ids(1), ["4"]);
        assert!(ids(9).is_empty());
    }

    #[test]
    fn a_report_folds_with_its_state_and_next_step() {
        let log = [
            r#"{"id":"1","at":10,"from":"billing@web","to":"lead","text":"payload renamed","state":"logged","report":{"state":"working","next":"nothing"}}"#,
            r#"{"id":"2","at":20,"from":"billing@web","to":"lead","text":"plan written","state":"logged","report":{"state":"needs-you","next":"approve the plan: keep the branch field?"}}"#,
            r#"{"id":"3","at":30,"from":"search@api","to":"lead","text":"rebase on main","state":"delivered"}"#,
        ].join("\n");
        let messages = fold(&log);
        assert_eq!(messages[1].report, Some(Report { state: "needs-you".to_string(), next: "approve the plan: keep the branch field?".to_string() }));
        assert!(!messages[1].held);
        assert_eq!(messages[0].next_for_you(), None);
        assert_eq!(messages[1].next_for_you(), Some("approve the plan: keep the branch field?"));
        assert_eq!(messages[2].report, None);
        let latest = latest_reports(&messages);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest["billing@web"].text, "plan written");
        let shown: Vec<String> = shown_in_log(messages).into_iter().map(|message| message.id).collect();
        assert_eq!(shown, ["2", "3"]);
    }

    #[test]
    fn the_lead_is_copied_on_reports_to_it() {
        let report = Some(Report { state: "ready".to_string(), next: "push and close".to_string() });
        let messages = [Message { id: "1".to_string(), at: 0, from: "a@x".to_string(), to: "lead".to_string(), text: "done".to_string(), held: false, report }];
        assert_eq!(copies_for_lead(&messages, 0).len(), 1);
        assert_eq!(report_line(&messages[0]), "- a@x (ready): done — you: push and close");
    }

    #[test]
    fn wrap_breaks_on_words_and_keeps_paragraphs() {
        assert_eq!(wrap("one two three four\nfive", 9), ["one two", "three", "four", "five"]);
    }
}
