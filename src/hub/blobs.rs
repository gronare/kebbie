use std::collections::BTreeMap;
use std::path::Path;

use kebbie_hub_protocol::{BACKLOG_BODY_MEDIA_TYPE, BlobRef, Board, CardRef, Id, ItemKey, seal_blob, seal_blob_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::http::{CALL, Client, HubError};
use super::identity::Identity;

pub const PLAN_LIMIT: usize = 64 * 1024;
pub const MEDIA_TYPE: &str = "text/markdown";
// The hub keeps a blob 14 days from its machine's last PUT of it, so a blob the board still names is put again well inside that.
pub const REPUT: u64 = 24 * 60 * 60;
pub const REFUSED_WAIT: u64 = 10 * 60;
const FILE: &str = "blobs.json";
const REVIEWS: &str = "reviews.json";
const HUB_KEEPS: u64 = 14 * 24 * 60 * 60;
const NONCE_LEN: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub enum Attachment {
    Plan { address: String, text: String, at: Option<u64> },
    Card { address: String, text: String, sha: String, current: bool },
    Backlog { item: ItemKey, text: String },
}

impl Attachment {
    fn text(&self) -> &str {
        match self {
            Attachment::Plan { text, .. } | Attachment::Card { text, .. } | Attachment::Backlog { text, .. } => text,
        }
    }

    fn media_type(&self) -> &'static str {
        match self {
            Attachment::Backlog { .. } => BACKLOG_BODY_MEDIA_TYPE,
            Attachment::Plan { .. } | Attachment::Card { .. } => MEDIA_TYPE,
        }
    }
}

// The nonce is kept so the same ciphertext, and so the same id, can be put again without a new key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Kept {
    blob: BlobRef,
    nonce: String,
    put_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    blobs: BTreeMap<String, Kept>,
    #[serde(default)]
    refused_at: Option<u64>,
}

fn load(dir: &Path) -> Cache {
    std::fs::read_to_string(dir.join(FILE)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save(dir: &Path, cache: &Cache) {
    if let Ok(text) = serde_json::to_string_pretty(cache) {
        let _ = crate::state::write(&dir.join(FILE), text);
    }
}

fn content_hash(media_type: &str, text: &str) -> String {
    hex(&Sha256::new().chain_update(media_type).chain_update([0]).chain_update(text).finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn nonce_of(hex: &str) -> Option<[u8; NONCE_LEN]> {
    let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|at| hex.get(at..at + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok())).collect::<Option<_>>()?;
    bytes.try_into().ok()
}

pub fn put_with(identity: &Identity) -> impl Fn(&Id, &[u8]) -> Result<(), HubError> {
    let client = Client::new(&identity.url, Some(identity.token()), CALL);
    move |id, ciphertext| client.put_bytes(&format!("/v1/m/blobs/{id}"), ciphertext)
}

pub fn delete_with(identity: &Identity) -> impl Fn(&Id) -> Result<(), HubError> {
    let client = Client::new(&identity.url, Some(identity.token()), CALL);
    move |id| client.delete(&format!("/v1/m/blobs/{id}"))
}

// The review each task was last sent as, by the hash of its plaintext.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Sent {
    hash: String,
    kept: Kept,
}

// An unchanged review goes up as the same blob, which the hub renews; a changed one is a new blob, and the one it replaces is deleted so reviews never fill the machine's quota.
pub fn review(dir: &Path, address: &str, plaintext: &[u8], media_type: &str, now: u64, put: impl Fn(&Id, &[u8]) -> Result<(), HubError>, delete: impl Fn(&Id) -> Result<(), HubError>) -> Result<BlobRef, HubError> {
    let file = dir.join(REVIEWS);
    let mut sent: BTreeMap<String, Sent> = std::fs::read_to_string(&file).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    sent.retain(|_, sent| now.saturating_sub(sent.kept.put_at) < HUB_KEEPS);
    let hash = hex(&Sha256::new().chain_update(media_type).chain_update([0]).chain_update(plaintext).finalize());
    let before = sent.get(address).cloned();
    let same = before.as_ref().filter(|before| before.hash == hash).and_then(|before| Some((before.kept.clone(), seal_blob_with(plaintext, media_type, decode_key(&before.kept.blob.key)?, nonce_of(&before.kept.nonce)?).1)));
    let (blob, nonce) = match same {
        Some((kept, ciphertext)) => {
            put(&kept.blob.id, &ciphertext)?;
            (kept.blob, kept.nonce)
        }
        None => {
            let (blob, ciphertext) = seal_blob(plaintext, media_type);
            put(&blob.id, &ciphertext)?;
            if let Some(before) = before.filter(|before| before.kept.blob.id != blob.id) {
                let _ = delete(&before.kept.blob.id);
            }
            (blob, hex(&ciphertext[..NONCE_LEN]))
        }
    };
    sent.insert(address.to_string(), Sent { hash, kept: Kept { blob: blob.clone(), nonce, put_at: now } });
    if let Ok(text) = serde_json::to_string_pretty(&sent) {
        let _ = crate::state::write(&file, text);
    }
    Ok(blob)
}

// The phone needs a blob on the hub before the board that names it, so an upload that fails leaves its ref off this board and is tried again next round; after a refusal or an unreachable hub the round tries no more.
pub fn attach(dir: &Path, board: &Board, attachments: &[Attachment], now: u64, put: impl Fn(&Id, &[u8]) -> Result<(), HubError>) -> Board {
    let before = load(dir);
    let mut cache = Cache { blobs: BTreeMap::new(), refused_at: before.refused_at.filter(|at| now.saturating_sub(*at) < REFUSED_WAIT) };
    let mut stopped = cache.refused_at.is_some();
    let mut board = board.clone();
    for attachment in attachments {
        let hash = content_hash(attachment.media_type(), attachment.text());
        let kept = match cache.blobs.get(&hash).or_else(|| before.blobs.get(&hash)).cloned() {
            Some(kept) if now.saturating_sub(kept.put_at) < REPUT || stopped => Some(kept),
            Some(kept) => {
                let ciphertext = nonce_of(&kept.nonce).and_then(|nonce| Some(seal_blob_with(attachment.text().as_bytes(), attachment.media_type(), decode_key(&kept.blob.key)?, nonce).1));
                match ciphertext.map(|ciphertext| put(&kept.blob.id, &ciphertext)) {
                    Some(Ok(())) => Some(Kept { put_at: now, ..kept }),
                    Some(Err(error)) => {
                        stop(&mut cache, &mut stopped, &error, now);
                        Some(kept)
                    }
                    None => None,
                }
            }
            None if stopped => None,
            None => {
                let (blob, ciphertext) = seal_blob(attachment.text().as_bytes(), attachment.media_type());
                let nonce = hex(&ciphertext[..NONCE_LEN]);
                match put(&blob.id, &ciphertext) {
                    Ok(()) => Some(Kept { blob, nonce, put_at: now }),
                    Err(error) => {
                        stop(&mut cache, &mut stopped, &error, now);
                        None
                    }
                }
            }
        };
        let Some(kept) = kept else { continue };
        place(&mut board, attachment, &kept.blob);
        cache.blobs.insert(hash, kept);
    }
    if cache != before {
        save(dir, &cache);
    }
    board
}

fn decode_key(key: &str) -> Option<[u8; 32]> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::with_capacity(32);
    for byte in key.bytes() {
        bits = (bits << 6) | ALPHABET.iter().position(|letter| *letter == byte)? as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    out.try_into().ok()
}

fn stop(cache: &mut Cache, stopped: &mut bool, error: &HubError, now: u64) {
    *stopped = true;
    if error.is_final() {
        cache.refused_at = Some(now);
    }
}

fn place(board: &mut Board, attachment: &Attachment, blob: &BlobRef) {
    let address = match attachment {
        Attachment::Plan { address, .. } | Attachment::Card { address, .. } => address,
        Attachment::Backlog { item, .. } => {
            if let Some(row) = board.backlog.iter_mut().flatten().find(|row| &row.item == item) {
                row.body = Some(blob.clone());
            }
            return;
        }
    };
    let Some(item) = board.items.iter_mut().find(|item| &item.address == address) else { return };
    match attachment {
        Attachment::Plan { at, .. } => {
            if let Some(question) = item.question.as_mut() {
                question.plan = Some(blob.clone());
                question.plan_at = *at;
            }
        }
        Attachment::Card { sha, current, .. } => item.card = Some(CardRef { sha: sha.clone(), current: *current, blob: blob.clone() }),
        Attachment::Backlog { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use kebbie_hub_protocol::{REVIEW_MEDIA_TYPE, open_blob};

    use super::*;

    #[test]
    fn a_review_goes_up_as_the_same_blob_while_unchanged_and_a_changed_one_deletes_the_one_it_replaces() {
        let dir = crate::sh::test_dir("review-blobs");
        let (put, deleted) = (RefCell::new(Vec::new()), RefCell::new(Vec::new()));
        let send = |text: &str, now: u64| review(&dir, "retry@shop", text.as_bytes(), REVIEW_MEDIA_TYPE, now, |id, ciphertext| { put.borrow_mut().push((id.clone(), ciphertext.to_vec())); Ok(()) }, |id| { deleted.borrow_mut().push(id.clone()); Ok(()) }).unwrap();
        let first = send("{\"files\":[]}", 100);
        let again = send("{\"files\":[]}", 200);
        assert_eq!(again, first, "the same key and id, so the phone's cached copy still opens");
        assert_eq!((put.borrow()[0].clone(), put.borrow().len()), (put.borrow()[1].clone(), 2), "put again as the same ciphertext, which renews it");
        assert!(deleted.borrow().is_empty());
        let changed = send("{\"files\":[1]}", 300);
        assert_ne!(changed.id, first.id);
        assert_eq!(deleted.borrow().as_slice(), std::slice::from_ref(&first.id));
        assert_eq!(open_blob(&changed, &put.borrow()[2].1).unwrap(), b"{\"files\":[1]}");
        let other = review(&dir, "other@shop", b"{\"files\":[1]}", REVIEW_MEDIA_TYPE, 300, |_, _| Ok(()), |_| panic!("another task's review is not replaced"));
        assert!(other.is_ok());
        let failed = review(&dir, "retry@shop", b"{\"files\":[2]}", REVIEW_MEDIA_TYPE, 400, |_, _| Err(HubError::Transport("down".to_string())), |_| panic!("nothing is deleted when the new one did not go up"));
        assert!(failed.is_err());
        assert_eq!(send("{\"files\":[1]}", 500), changed, "a failed put leaves the review sent before");
    }
}
