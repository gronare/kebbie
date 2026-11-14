use std::path::Path;

use serde_json::json;

use crate::peers::Session;
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use super::{DRAFT_HOLD, ROTATE_BYTES, USER_LABEL, append_in, hold_reason, kebbies_own, load_in, new_id, note_lead_prompt_in, note_typed, note_why, now};

pub const DEVICE_KIND: &str = "device";
pub const LEAD_WORKING: &str = "is mid-turn; it goes in once the lead's turn ends";

pub type TypeAsUser<'a> = &'a mut dyn FnMut(&str, &str) -> std::result::Result<(), crate::typing::Refused>;

// What you wrote to the lead on a device, held until the lead is idle with an empty input box; the log line's id is returned.
pub fn hold_for_lead_in(dir: &Path, typed: &str) -> Result<String> {
    let id = new_id(DEVICE_KIND);
    append_in(dir, &json!({ "id": id, "at": now(), "from": USER_LABEL, "to": LEAD_LABEL, "text": typed, "state": "held", "kind": DEVICE_KIND }), ROTATE_BYTES)?;
    Ok(id)
}

pub fn drop_held_in(dir: &Path, id: &str) -> Result<()> {
    if !load_in(dir).iter().any(|message| message.id == id && message.held) {
        return Ok(());
    }
    append_in(dir, &json!({ "id": id, "at": now(), "state": "dropped" }), ROTATE_BYTES)
}

// The caller holds the lock. The oldest message from a device goes in as your own prompt, typed and never through the inbox, which would show it as another session's; one per pass, as the box then holds it.
pub(super) fn deliver_device_in(dir: &Path, sessions: &[Session], type_as_user: TypeAsUser) -> Result<usize> {
    let messages = load_in(dir);
    let Some(message) = messages.iter().find(|message| message.held && message.to == LEAD_LABEL && message.kind.as_deref() == Some(DEVICE_KIND)) else { return Ok(0) };
    let Some(lead) = sessions.iter().find(|session| session.address == LEAD_LABEL) else { return Ok(0) };
    if let Some(why) = hold_reason(lead) {
        note_why(dir, message, why)?;
        return Ok(0);
    }
    if lead.status.as_deref() == Some("working") {
        note_why(dir, message, LEAD_WORKING)?;
        return Ok(0);
    }
    match type_as_user(lead.pane.as_deref().unwrap_or_default(), &message.text) {
        Ok(()) => {
            note_typed(dir, LEAD_LABEL);
            note_lead_prompt_in(dir, &message.text, now())?;
            append_in(dir, &json!({ "id": message.id, "at": now(), "state": "delivered", "via": crate::inbox::Via::Typed.word() }), ROTATE_BYTES)?;
            Ok(1)
        }
        Err(crate::typing::Refused::Draft) => note_why(dir, message, kebbies_own(dir, LEAD_LABEL, DRAFT_HOLD)).map(|_| 0),
        Err(crate::typing::Refused::Unreadable) => note_why(dir, message, "shows no input box to type into").map(|_| 0),
        Err(crate::typing::Refused::Failed(_)) => note_why(dir, message, "could not be reached").map(|_| 0),
    }
}
