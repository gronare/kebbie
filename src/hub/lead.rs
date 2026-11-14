use std::path::Path;

use kebbie_hub_protocol::{BlobRef, Id, LEAD_THREAD_MEDIA_TYPE, LeadRef, LeadThread, MAX_LEAD_PAGE_TURNS, Outcome, Payload, Reason, seal_blob, seal_blob_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::blobs::{NONCE_LEN, REPUT, decode_key, hex, nonce_of};
use super::commands::{Taken, ToLead, plain};
use super::http::HubError;
use crate::lead_thread::{self, DELIVERED, Sent, Thread};
use crate::permit::Caller;

const FILE: &str = "lead-blobs.json";
// The thread the phone pages back through: the newest thousand turns.
pub const MAX_PAGES: usize = 20;

pub fn take(caller: &Caller, asked: &ToLead, payload: &Payload) -> Taken {
    let Ok(state) = crate::lead::state_dir() else { return Taken::Now((Outcome::Failed, "kebbie could not find its state on this machine".to_string())) };
    take_in(&state, caller, asked, payload, crate::messages::now(), &|| {
        let _ = crate::messages::flush();
    })
}

// A message waits in kebbie's log until the lead is free, and its result goes back once the lead's transcript shows it; away mode switches at once, unless a switch asked for later already decided it.
pub fn take_in(state: &Path, caller: &Caller, asked: &ToLead, payload: &Payload, now: u64, flush: &dyn Fn()) -> Taken {
    let name = match caller {
        Caller::Hub { name, .. } => name.as_str(),
        _ => "a device",
    };
    match payload {
        Payload::LeadMessage(message) => {
            if let Some(sent) = lead_thread::load(state).sent.into_iter().find(|sent| sent.command == asked.command.as_str()) {
                return if sent.delivered.is_some() { Taken::Now((Outcome::Done, DELIVERED.to_string())) } else { Taken::Later("it already waits for the lead".to_string()) };
            }
            if !crate::lead::ever_started(state) {
                return Taken::Now((Outcome::Refused { reason: Reason::NoLead }, "Not delivered: no lead has run on this machine yet; start kebbie there and send it again".to_string()));
            }
            let typed = lead_thread::typed(name, &message.text);
            let held = crate::messages::hold_for_lead_in(state, &typed).and_then(|id| {
                lead_thread::change(state, |store| {
                    store.sent.push(Sent { command: asked.command.to_string(), device: asked.device.to_string(), typed, text: message.text.clone(), at: now, exp: asked.exp, message: id, dir: asked.dir.to_path_buf(), delivered: None });
                })
            });
            if let Err(error) = held {
                return Taken::Now((Outcome::Failed, format!("Not delivered: {error}")));
            }
            flush();
            Taken::Later("held for the lead; it goes in as your word once the lead is free".to_string())
        }
        Payload::Away(away) => {
            let word = |on: bool| if on { "on" } else { "off" };
            if lead_thread::newest_switch(state).is_some_and(|newest| newest > asked.at) {
                let current = crate::permit::away_since_in(state).is_some();
                return Taken::Now((Outcome::Done, format!("Away mode stays {}: a switch asked for after this one decided it.", word(current))));
            }
            match crate::permit::switch_away(state, away.on, now, asked.at, Some((asked.command.as_str(), asked.device.as_str()))) {
                Ok(text) => Taken::Now((Outcome::Done, plain(&text))),
                Err(error) => Taken::Now((Outcome::Failed, format!("Away mode not switched: {error}"))),
            }
        }
        _ => Taken::Now((Outcome::Refused { reason: Reason::BadRequest }, "This machine could not read what was asked".to_string())),
    }
}

// One sealed page, kept by the hash of its plaintext so an unchanged page goes up as the same blob, which the hub renews.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Kept {
    hash: String,
    blob: BlobRef,
    nonce: String,
    put_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    pages: Vec<Kept>,
}

fn load(dir: &Path) -> Cache {
    std::fs::read_to_string(dir.join(FILE)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

// The thread as pages of fifty turns, oldest first, each naming the one before it: pages before the newest never change, so only the newest is sealed again as the lead talks; a page no longer named is deleted from the hub. A page that cannot go up leaves the thread as last sealed.
pub fn seal(dir: &Path, thread: &Thread, now: u64, put: impl Fn(&Id, &[u8]) -> Result<(), HubError>, delete: impl Fn(&Id) -> Result<(), HubError>) -> Option<Box<LeadRef>> {
    let before = load(dir);
    let chunks: Vec<&[kebbie_hub_protocol::Turn]> = if thread.turns.is_empty() { vec![&[]] } else { thread.turns.chunks(MAX_LEAD_PAGE_TURNS).collect() };
    let chunks = &chunks[chunks.len().saturating_sub(MAX_PAGES)..];
    let mut cache = Cache::default();
    let mut older: Option<BlobRef> = None;
    for turns in chunks {
        let page = LeadThread { turns: turns.to_vec(), older: older.take() };
        page.check().ok()?;
        let plaintext = serde_json::to_vec(&page).ok()?;
        let hash = hex(&Sha256::digest(&plaintext));
        let kept = match before.pages.iter().find(|kept| kept.hash == hash) {
            Some(kept) if now.saturating_sub(kept.put_at) < REPUT => kept.clone(),
            Some(kept) => {
                let ciphertext = seal_blob_with(&plaintext, LEAD_THREAD_MEDIA_TYPE, decode_key(&kept.blob.key)?, nonce_of(&kept.nonce)?).1;
                match put(&kept.blob.id, &ciphertext) {
                    Ok(()) => Kept { put_at: now, ..kept.clone() },
                    Err(_) => kept.clone(),
                }
            }
            None => {
                let (blob, ciphertext) = seal_blob(&plaintext, LEAD_THREAD_MEDIA_TYPE);
                if put(&blob.id, &ciphertext).is_err() {
                    return before.pages.last().map(|last| Box::new(LeadRef { thread: last.blob.clone(), working_since: thread.working_since, replied_at: thread.replied_at }));
                }
                Kept { hash, nonce: hex(&ciphertext[..NONCE_LEN]), blob, put_at: now }
            }
        };
        older = Some(kept.blob.clone());
        cache.pages.push(kept);
    }
    for gone in before.pages.iter().filter(|kept| !cache.pages.iter().any(|page| page.blob.id == kept.blob.id)) {
        let _ = delete(&gone.blob.id);
    }
    if cache != before
        && let Ok(text) = serde_json::to_string_pretty(&cache)
    {
        let _ = crate::state::write(&dir.join(FILE), text);
    }
    Some(Box::new(LeadRef { thread: older?, working_since: thread.working_since, replied_at: thread.replied_at }))
}

#[cfg(test)]
mod tests;
