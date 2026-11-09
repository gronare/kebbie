use std::path::Path;

use serde_json::json;

use crate::lead::state_dir;
use crate::peers::{self, Session};
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use super::{Lock, Message, PROMPT_KIND, ROTATE_BYTES, USER_LABEL, append_in, delivered, indented, load_in, new_id, now, typed_form};

pub const FORCED_KIND: &str = "forced";

// The user's or the lead's override for a delivery that is stuck: it clears the user's draft (logged word for word), then types what is held for the session and the new message, in order, as one prompt.
pub fn tell_forced(to: &str, text: Option<&str>) -> Result<String> {
    if text.is_some_and(|text| text.trim().is_empty()) {
        return Err("kebbie tell --force needs a message: kebbie tell --force <task|task@repo|lead> \"<text>\"; kebbie release <task> delivers only what is held".to_string());
    }
    let from = forcer(&crate::permit::caller())?;
    let sessions = peers::sessions()?;
    let recipients = peers::resolve(to, from, &sessions)?;
    let _lock = Lock::take(true)?;
    let dir = state_dir()?;
    let mut report = Vec::new();
    for session in recipients {
        report.push(force_in(&dir, from, session, text.map(str::trim), &mut crate::typing::prompt_forced)?);
    }
    Ok(report.join("\n"))
}

pub(super) fn forcer(caller: &crate::permit::Caller) -> Result<&'static str> {
    match caller {
        crate::permit::Caller::User => Ok(USER_LABEL),
        crate::permit::Caller::Lead => Ok(LEAD_LABEL),
        other => Err(format!("forcing a delivery is the user's or the lead's; {} cannot, since it clears what the user typed", crate::permit::describe(other))),
    }
}

type TypeForced<'a> = &'a mut dyn FnMut(&str, &str, &str) -> std::result::Result<Option<String>, crate::typing::Refused>;

pub(super) fn force_in(dir: &Path, from: &str, session: &Session, text: Option<&str>, type_forced: TypeForced) -> Result<String> {
    let address = &session.address;
    let Some(pane) = session.pane.as_deref() else { return Err(format!("{address} has no Claude running; nothing was forced")) };
    if session.status.as_deref() == Some("blocked") {
        return Err(format!("{address} shows a prompt of Claude's own (a question or a permission), where a key would pick an option; nothing was forced. Answer it in its Space first."));
    }
    let messages = load_in(dir);
    let mut held: Vec<&Message> = messages.iter().filter(|message| message.held && &message.to == address).collect();
    held.sort_by_key(|message| message.kind.as_deref() != Some(PROMPT_KIND));
    let mut parts: Vec<String> = held.iter().map(|message| typed_form(message)).collect();
    parts.extend(text.map(|text| delivered(from, text)));
    if parts.is_empty() {
        return Ok(format!("{address} has nothing held; nothing to release."));
    }
    let cleared = match type_forced(pane, &parts.join("\n\n"), address) {
        Ok(cleared) => cleared,
        Err(refused) => return Err(format!("{address} {}; nothing was forced", refused.reason())),
    };
    if let Some(draft) = &cleared {
        append_in(dir, &json!({ "id": new_id("forced"), "at": now(), "from": "kebbie", "to": address, "text": format!("Cleared {address}'s input box so {from}'s forced delivery could go in; the draft was:\n{draft}"), "state": "logged", "kind": FORCED_KIND, "forced": true }), ROTATE_BYTES)?;
    }
    for message in &held {
        append_in(dir, &json!({ "id": message.id, "at": now(), "state": "delivered", "forced": true }), ROTATE_BYTES)?;
    }
    if let Some(text) = text {
        append_in(dir, &json!({ "id": new_id("forced"), "at": now(), "from": from, "to": address, "text": text, "state": "delivered", "forced": true }), ROTATE_BYTES)?;
    }
    let count = if held.len() == 1 { "1 message".to_string() } else { format!("{} messages", held.len()) };
    let what = match (held.len(), text.is_some()) {
        (0, _) => "your message".to_string(),
        (_, false) => format!("{count} held"),
        (_, true) => format!("{count} held, then yours"),
    };
    let clearing = cleared.map(|draft| format!(" It cleared this draft from the input box (Ctrl+Y in its pane brings it back):\n{}", indented(&draft))).unwrap_or_default();
    Ok(format!("Forced to {address}: {what}.{clearing}"))
}
