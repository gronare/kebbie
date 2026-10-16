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

use crate::board::{FAINT, MAUVE, MUTED, PEACH, SUBTEXT, SURFACE, TEAL, TEXT, bold, style};
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
}

fn log_file() -> Result<PathBuf> {
    Ok(state_dir()?.join("messages.jsonl"))
}

fn now() -> u64 {
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

fn age(at: u64) -> String {
    let seconds = now().saturating_sub(at);
    match seconds {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

pub fn print(count: usize) -> String {
    let messages = load();
    if messages.is_empty() {
        return "No messages yet.".to_string();
    }
    messages[messages.len().saturating_sub(count)..].iter().map(|message| {
        let held = if message.held { "  (held)" } else { "" };
        format!("{:>4}  {} → {}{held}\n      {}", age(message.at), message.from, message.to, message.text)
    }).collect::<Vec<_>>().join("\n")
}

// The lead's copy: what sessions told each other since it last looked, given to it with each of the user's prompts.
pub fn unread_for_lead() -> Result<String> {
    let mark = state_dir()?.join("messages-lead-read");
    let seen: usize = fs::read_to_string(&mark).ok().and_then(|count| count.trim().parse().ok()).unwrap_or(0);
    let messages = load();
    fs::write(&mark, messages.len().to_string()).map_err(|error| error.to_string())?;
    let unread = copies_for_lead(&messages, seen);
    if unread.is_empty() {
        return Ok(String::new());
    }
    let lines: Vec<String> = unread.iter().map(|message| {
        let held = if message.held { " (held)" } else { "" };
        format!("- {} → {}{held}: {}", message.from, message.to, message.text)
    }).collect();
    Ok(format!("Messages between sessions since you last looked (kelpie messages --print for more):\n{}", lines.join("\n")))
}

// Messages to or from the lead are already in its chat.
fn copies_for_lead(messages: &[Message], seen: usize) -> Vec<&Message> {
    messages.iter().skip(seen.min(messages.len())).filter(|message| message.from != LEAD_LABEL && message.to != LEAD_LABEL).collect()
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
        Span::styled(message.to.clone(), style(TEAL)),
    ];
    if message.held {
        head.push(Span::styled("  held", style(PEACH)));
    }
    let mut lines = vec![Line::from(head)];
    lines.extend(wrap(&message.text, width.saturating_sub(5).max(10)).into_iter().map(|text| Line::from(vec![Span::raw("     "), Span::styled(text, style(SUBTEXT))])));
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
        lines.push(Line::from(Span::styled("Sessions' messages to each other show here.", style(MUTED))));
        lines.push(Line::from(Span::styled("They send them with kelpie tell.", style(MUTED))));
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
            let messages = load();
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
        Session { address: "posthog@relay".to_string(), task: Some("posthog".to_string()), pane: pane.map(str::to_string), status: status.map(str::to_string) }
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
            r#"{"id":"1","at":10,"from":"posthog@auctionet","to":"posthog@relay","text":"person_id replaces user_id","state":"held"}"#,
            r#"{"id":"2","at":11,"from":"lead","to":"turnstile@auctionet","text":"rebase on main","state":"delivered"}"#,
            "not json",
            r#"{"id":"1","at":20,"state":"delivered"}"#,
            r#"{"id":"3","at":30,"from":"posthog@relay","to":"posthog@auctionet","text":"done","state":"held"}"#,
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
        let message = |id: &str, from: &str, to: &str| Message { id: id.to_string(), at: 0, from: from.to_string(), to: to.to_string(), text: String::new(), held: false };
        let messages = [message("1", "a@x", "b@x"), message("2", "a@x", "lead"), message("3", "lead", "b@x"), message("4", "b@x", "a@x")];
        let ids = |seen| copies_for_lead(&messages, seen).iter().map(|message| message.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(0), ["1", "4"]);
        assert_eq!(ids(1), ["4"]);
        assert!(ids(9).is_empty());
    }

    #[test]
    fn wrap_breaks_on_words_and_keeps_paragraphs() {
        assert_eq!(wrap("one two three four\nfive", 9), ["one two", "three", "four", "five"]);
    }
}
