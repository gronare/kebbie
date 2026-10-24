use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::sh::Result;
use crate::typing::Refused;

const SOCKET_VAR: &str = "CLAUDE_CODE_MESSAGING_SOCKET";
const TOKEN_VAR: &str = "CLAUDE_CODE_MESSAGING_TOKEN";
const SENDER: &str = "kebbie";

#[derive(Clone, Debug, PartialEq)]
pub struct Inbox {
    pub socket: PathBuf,
    pub token: Option<String>,
    pub session: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Via {
    Inbox,
    Typed,
    TypedAfterInbox,
}

impl Via {
    pub fn word(self) -> &'static str {
        match self {
            Via::Inbox => "inbox",
            Via::Typed => "typed",
            Via::TypedAfterInbox => "typed after the inbox refused it",
        }
    }
}

fn file_in(dir: &Path, address: &str) -> PathBuf {
    dir.join("inbox").join(format!("{address}.json"))
}

pub fn known_in(dir: &Path, address: &str) -> Option<Inbox> {
    let record: Value = serde_json::from_str(&std::fs::read_to_string(file_in(dir, address)).ok()?).ok()?;
    let socket = record["socket"].as_str().filter(|socket| !socket.is_empty())?;
    Some(Inbox { socket: PathBuf::from(socket), token: record["token"].as_str().map(str::to_string), session: record["session"].as_str().map(str::to_string) })
}

// A session that starts without a socket (a Claude Code before 2.1.224, or one that could not bind it) forgets the inbox an earlier session at the address left.
pub fn record_in(dir: &Path, address: &str, socket: Option<&str>, token: Option<&str>, session: Option<&str>) -> Result<()> {
    let file = file_in(dir, address);
    match socket.filter(|socket| !socket.trim().is_empty()) {
        Some(socket) => crate::state::write(&file, json!({ "socket": socket, "token": token, "session": session, "at": crate::messages::now() }).to_string()),
        None => match std::fs::remove_file(&file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {error}", file.display())),
            _ => Ok(()),
        },
    }
}

// kebbie's SessionStart hook: Claude Code exports the socket and token to hooks before SessionStart runs.
pub fn hook(address: &str, input: &str) -> String {
    let input: Value = serde_json::from_str(input).unwrap_or_default();
    let socket = std::env::var(SOCKET_VAR).ok();
    let token = std::env::var(TOKEN_VAR).ok();
    if let Ok(dir) = crate::lead::state_dir() {
        let _ = hook_in(&dir, address, &input, home_of(address).as_deref(), socket.as_deref(), token.as_deref());
    }
    String::new()
}

// Where the session at this address runs: the lead's folder, or the task's worktree.
fn home_of(address: &str) -> Option<PathBuf> {
    if address == crate::status::LEAD_LABEL {
        return crate::lead::lead_dir().ok();
    }
    let (task, repo) = address.split_once('@')?;
    let repo = crate::repo::checkout_named(repo)?;
    crate::tasks::find_task(&repo, Some(task), &repo).ok().map(|slot| slot.path)
}

// A session records an inbox only for the address whose folder it runs in, so a task cannot take the lead's messages.
pub fn hook_in(dir: &Path, address: &str, input: &Value, home: Option<&Path>, socket: Option<&str>, token: Option<&str>) -> Result<()> {
    let canonical = |path: &Path| std::fs::canonicalize(path).ok();
    let at_home = input["cwd"].as_str().zip(home).is_some_and(|(cwd, home)| canonical(Path::new(cwd)).is_some_and(|cwd| Some(cwd) == canonical(home)));
    if !at_home {
        return Ok(());
    }
    record_in(dir, address, socket, token, input["session_id"].as_str())
}

// One connection's bytes: the auth line, then one user message per line, as Claude Code 2.1.308 took them from a process outside the session.
pub fn frame(text: &str, token: Option<&str>) -> String {
    let auth = token.map(|token| format!("{{\"type\":\"auth\",\"token\":{}}}\n", Value::from(token))).unwrap_or_default();
    format!("{auth}{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":{}}},\"from\":{}}}\n", Value::from(text), Value::from(SENDER))
}

pub fn send(inbox: &Inbox, text: &str) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(&inbox.socket)?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(frame(text, inbox.token.as_deref()).as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)
}

pub type TypeInto<'a> = &'a mut dyn FnMut(&str, &str, &str) -> std::result::Result<(), Refused>;
pub type SessionOf<'a> = &'a dyn Fn(&str) -> Option<String>;

// The inbox of the conversation herdr shows in the pane now; a record from an earlier conversation, or one naming none, is not it.
pub fn current_in(dir: &Path, pane: &str, address: &str, session_of: SessionOf) -> Option<Inbox> {
    known_in(dir, address).filter(|inbox| inbox.session.is_some() && inbox.session == session_of(pane))
}

// Whether a message would reach the pane's conversation through its inbox now: a connection opens and is closed with nothing sent.
pub fn live_in(dir: &Path, pane: &str, address: &str, session_of: SessionOf) -> bool {
    current_in(dir, pane, address, session_of).is_some_and(|inbox| UnixStream::connect(&inbox.socket).is_ok())
}

pub fn prompt_in(dir: &Path, target: &str, text: &str, address: &str, session_of: SessionOf, type_into: TypeInto) -> std::result::Result<Via, Refused> {
    let Some(inbox) = current_in(dir, target, address, session_of) else { return type_into(target, text, address).map(|_| Via::Typed) };
    let at_ms = crate::typing::now_ms();
    match send(&inbox, text) {
        Ok(()) => {
            let _ = crate::typing::watch_inbox_in(dir, target, text, address, at_ms);
            Ok(Via::Inbox)
        }
        Err(error) => {
            if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) {
                let _ = record_in(dir, address, None, None, None);
            }
            type_into(target, text, address).map(|_| Via::TypedAfterInbox)
        }
    }
}

pub fn herdr_session(pane: &str) -> Option<String> {
    let agents = crate::herdr::agents().ok()?;
    let agent = agents.iter().find(|agent| agent["pane_id"] == pane || agent["name"] == pane)?;
    agent["agent_session"]["value"].as_str().filter(|id| !id.is_empty()).map(str::to_string)
}

pub fn prompt(target: &str, text: &str, address: &str) -> std::result::Result<Via, Refused> {
    match crate::lead::state_dir() {
        Ok(dir) => prompt_in(&dir, target, text, address, &herdr_session, &mut crate::typing::prompt),
        Err(_) => crate::typing::prompt(target, text, address).map(|_| Via::Typed),
    }
}

// Without accept, a bypassPermissions session holds kebbie's messages for the user's approval.
pub fn with_inbox(settings: &str, kebbie: &str, address: &str) -> String {
    let mut written: Value = serde_json::from_str(settings).expect("kebbie writes its own settings as JSON");
    written["crossSessionInbound"] = json!("accept");
    if address != crate::status::LEAD_LABEL && let Some(deny) = written["permissions"]["deny"].as_array_mut() {
        deny.push(json!(format!("Bash({kebbie} hook:*)")));
    }
    written["hooks"]["SessionStart"] = json!([{ "hooks": [{ "type": "command", "command": format!("{} hook inbox {}", crate::sh::quote(kebbie), crate::sh::quote(address)) }] }]);
    written.to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::net::UnixListener;

    const PROBED: &str = "{\"type\":\"auth\",\"token\":\"tok-1\"}\n{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"[from kebbie] probe line one\\n  Second line\"},\"from\":\"kebbie\"}\n";

    // A socket path is capped near 104 bytes, longer than a test's temp folder can leave room for.
    pub(crate) fn short_socket() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let socket = PathBuf::from(format!("/tmp/kebbie-{}-{}.sock", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_file(&socket);
        socket
    }

    fn listen(_dir: &Path) -> (PathBuf, std::thread::JoinHandle<String>) {
        let socket = short_socket();
        let listener = UnixListener::bind(&socket).unwrap();
        let path = socket.clone();
        let reader = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = std::fs::remove_file(path);
            let mut got = String::new();
            stream.read_to_string(&mut got).unwrap();
            got
        });
        (socket, reader)
    }

    #[test]
    fn the_frame_is_the_auth_line_then_the_user_line_claude_code_took_in_the_probe() {
        assert_eq!(frame("[from kebbie] probe line one\n  Second line", Some("tok-1")), PROBED);
        for line in PROBED.lines() {
            serde_json::from_str::<Value>(line).unwrap();
        }
        assert_eq!(frame("hi \"there\"", None), "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi \\\"there\\\"\"},\"from\":\"kebbie\"}\n", "no auth line without a token; quotes escaped");
        let message: Value = serde_json::from_str(frame("a\u{2028}b", None).trim_end()).unwrap();
        assert_eq!(message["message"]["content"], "a\u{2028}b");
        assert_eq!(frame("one\ntwo", None).matches('\n').count(), 1, "the text's line breaks stay inside its one JSON line");
    }

    fn same(session: &'static str) -> impl Fn(&str) -> Option<String> {
        move |_: &str| Some(session.to_string())
    }

    #[test]
    fn a_recorded_inbox_of_the_panes_conversation_gets_the_message_nothing_is_typed_and_the_board_watches_for_it() {
        let dir = crate::sh::test_dir("inbox-send");
        let (socket, reader) = listen(&dir);
        record_in(&dir, "billing@app", Some(&socket.to_string_lossy()), Some("tok-1"), Some("s-1")).unwrap();
        let via = prompt_in(&dir, "w1:p1", "[from kebbie] probe line one\n  Second line", "billing@app", &same("s-1"), &mut |_, _, _| panic!("nothing is typed when the inbox takes it"));
        assert_eq!(via, Ok(Via::Inbox));
        assert_eq!(reader.join().unwrap(), PROBED);
        assert!(crate::typing::watching_in(&dir, "billing@app", "[from kebbie] probe line one\n  Second line"), "a message the inbox took is checked against the transcript, as a typed one is");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_record_from_another_conversation_than_the_panes_is_not_used() {
        let dir = crate::sh::test_dir("inbox-stale");
        let socket = short_socket();
        let _listener = UnixListener::bind(&socket).unwrap();
        record_in(&dir, "lead", Some(&socket.to_string_lossy()), None, Some("old")).unwrap();
        let typed = std::cell::Cell::new(0);
        let mut type_into = |_: &str, _: &str, _: &str| { typed.set(typed.get() + 1); Ok(()) };
        assert_eq!(prompt_in(&dir, "w0:p1", "hi", "lead", &same("new"), &mut type_into), Ok(Via::Typed), "after a restart, or a record a task wrote with its own socket");
        assert_eq!(prompt_in(&dir, "w0:p1", "hi", "lead", &|_| None, &mut type_into), Ok(Via::Typed), "herdr naming no conversation proves nothing");
        record_in(&dir, "lead", Some(&socket.to_string_lossy()), None, None).unwrap();
        assert_eq!(prompt_in(&dir, "w0:p1", "hi", "lead", &same("new"), &mut type_into), Ok(Via::Typed), "a record naming no conversation is no one's");
        assert_eq!(typed.get(), 3);
        assert!(!live_in(&dir, "w0:p1", "lead", &same("new")));
        record_in(&dir, "lead", Some(&socket.to_string_lossy()), None, Some("new")).unwrap();
        assert!(live_in(&dir, "w0:p1", "lead", &same("new")));
        let _ = std::fs::remove_file(&socket);
        assert!(!live_in(&dir, "w0:p1", "lead", &same("new")), "a socket nobody listens on is not live");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn without_a_recorded_inbox_or_when_its_socket_is_gone_the_message_is_typed() {
        let dir = crate::sh::test_dir("inbox-fallback");
        let typed = std::cell::RefCell::new(Vec::new());
        let mut type_into = |pane: &str, text: &str, address: &str| { typed.borrow_mut().push((pane.to_string(), text.to_string(), address.to_string())); Ok(()) };
        assert_eq!(prompt_in(&dir, "w1:p1", "hello", "billing@app", &same("s"), &mut type_into), Ok(Via::Typed));
        record_in(&dir, "billing@app", Some(&short_socket().to_string_lossy()), None, Some("s")).unwrap();
        assert_eq!(prompt_in(&dir, "w1:p1", "again", "billing@app", &same("s"), &mut type_into), Ok(Via::TypedAfterInbox));
        assert!(known_in(&dir, "billing@app").is_none(), "a socket nobody listens on is forgotten");
        assert_eq!(typed.borrow().as_slice(), [("w1:p1".to_string(), "hello".to_string(), "billing@app".to_string()), ("w1:p1".to_string(), "again".to_string(), "billing@app".to_string())]);
        assert_eq!(prompt_in(&dir, "w1:p1", "x", "billing@app", &same("s"), &mut |_, _, _| Err(Refused::Draft)), Err(Refused::Draft), "typing keeps its own refusals");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_session_start_records_its_socket_and_one_without_a_socket_forgets_the_old_one() {
        let dir = crate::sh::test_dir("inbox-record");
        record_in(&dir, "lead", Some("/tmp/cc-socks/1.sock"), Some("t"), Some("s")).unwrap();
        assert_eq!(known_in(&dir, "lead"), Some(Inbox { socket: PathBuf::from("/tmp/cc-socks/1.sock"), token: Some("t".to_string()), session: Some("s".to_string()) }));
        assert!(known_in(&dir, "billing@app").is_none(), "each address its own");
        record_in(&dir, "lead", None, None, Some("s2")).unwrap();
        assert!(known_in(&dir, "lead").is_none());
        record_in(&dir, "lead", None, None, None).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_a_session_running_in_the_addresss_own_folder_records_its_inbox() {
        let dir = crate::sh::test_dir("inbox-home");
        let (lead_home, task_home) = (dir.join("lead"), dir.join("wt1.app"));
        std::fs::create_dir_all(&lead_home).unwrap();
        std::fs::create_dir_all(&task_home).unwrap();
        let input = |cwd: &Path| json!({ "cwd": cwd.to_string_lossy(), "session_id": "s-1" });
        hook_in(&dir, "lead", &input(&task_home), Some(&lead_home), Some("/tmp/cc-socks/9.sock"), None).unwrap();
        assert!(known_in(&dir, "lead").is_none(), "a task naming the lead in its own worktree records nothing");
        hook_in(&dir, "lead", &json!({ "session_id": "s-1" }), Some(&lead_home), Some("/tmp/cc-socks/9.sock"), None).unwrap();
        hook_in(&dir, "lead", &input(&lead_home), None, Some("/tmp/cc-socks/9.sock"), None).unwrap();
        assert!(known_in(&dir, "lead").is_none(), "no folder to compare, no record");
        hook_in(&dir, "lead", &input(&lead_home), Some(&lead_home), Some("/tmp/cc-socks/1.sock"), None).unwrap();
        assert_eq!(known_in(&dir, "lead").and_then(|inbox| inbox.session), Some("s-1".to_string()));
        hook_in(&dir, "billing@app", &input(&task_home.join(".")), Some(&task_home), Some("/tmp/cc-socks/2.sock"), None).unwrap();
        assert!(known_in(&dir, "billing@app").is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_settings_accept_messages_from_other_sessions_and_record_the_inbox_at_session_start() {
        let settings = with_inbox(r#"{"hooks":{"Stop":[]},"permissions":{"allow":[]}}"#, "/bin/kel pie", "billing@app");
        let written: Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(written["crossSessionInbound"], "accept", "a bypassPermissions receiver holds a message from kebbie for approval otherwise");
        assert_eq!(written["hooks"]["SessionStart"][0]["hooks"][0]["command"], "'/bin/kel pie' hook inbox 'billing@app'");
        assert!(written["hooks"]["Stop"].is_array() && written["permissions"]["allow"].is_array(), "the rest is kept");
        let task: Value = serde_json::from_str(&with_inbox(r#"{"permissions":{"deny":["Bash(git push:*)"]}}"#, "/bin/kebbie", "billing@app")).unwrap();
        assert_eq!(task["permissions"]["deny"], json!(["Bash(git push:*)", "Bash(/bin/kebbie hook:*)"]), "a task's Bash cannot run kebbie's hooks; Claude Code runs them without Bash");
        let lead: Value = serde_json::from_str(&with_inbox(r#"{"permissions":{"allow":[]}}"#, "/bin/kebbie", "lead")).unwrap();
        assert!(lead["permissions"]["deny"].is_null());
    }
}
