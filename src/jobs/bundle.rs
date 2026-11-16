use std::path::Path;
use std::time::Duration;

use kebbie_hub_protocol::{BLOB_OVERHEAD, BlobRef, JOB_BUNDLE_MEDIA_TYPE, MAX_BLOB_BYTES, MAX_JOB_BUNDLE_PARTS, open_blob, seal_blob};

use crate::hub::http::{Client, HubError};
use crate::sh::Result;

pub const PART: usize = MAX_BLOB_BYTES - BLOB_OVERHEAD;
const INCOMING: &str = "refs/kebbie/incoming";
const BURST: f64 = 30.0;
const PER_SECOND: f64 = 1.0;
const TRIES: u32 = 6;

// The hub lets a machine's token make 60 calls at once and then 2 a second, its keeper's calls included, so blob calls keep to half of that.
#[derive(Clone, Debug)]
pub struct Pace {
    tokens: f64,
    at_ms: u64,
}

impl Pace {
    pub fn new(now_ms: u64) -> Pace {
        Pace { tokens: BURST, at_ms: now_ms }
    }

    // How long to wait before the next call, which it counts.
    pub fn next(&mut self, now_ms: u64) -> u64 {
        let now = now_ms.max(self.at_ms);
        self.tokens = (self.tokens + (now - self.at_ms) as f64 / 1000.0 * PER_SECOND).min(BURST);
        self.at_ms = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return now - now_ms;
        }
        let wait = ((1.0 - self.tokens) / PER_SECOND * 1000.0).ceil() as u64;
        self.tokens = 0.0;
        self.at_ms = now + wait;
        self.at_ms - now_ms
    }
}

// Blob calls at the pace, each tried again after 1, 2, 4, 8 and 16 s when the hub was busy, down or said 429, as job reports are.
pub struct Calls<'a> {
    pace: Pace,
    clock: &'a dyn Fn() -> u64,
    sleep: &'a dyn Fn(Duration),
}

impl<'a> Calls<'a> {
    pub fn new(clock: &'a dyn Fn() -> u64, sleep: &'a dyn Fn(Duration)) -> Calls<'a> {
        Calls { pace: Pace::new(clock()), clock, sleep }
    }

    pub fn call<T>(&mut self, once: &mut dyn FnMut() -> std::result::Result<T, HubError>) -> std::result::Result<T, HubError> {
        let mut tried = 0;
        loop {
            let wait = self.pace.next((self.clock)());
            if wait > 0 {
                (self.sleep)(Duration::from_millis(wait));
            }
            match once() {
                Err(error) if !error.is_final() && tried + 1 < TRIES => {
                    (self.sleep)(Duration::from_secs(1 << tried));
                    tried += 1;
                }
                done => return done,
            }
        }
    }
}

pub fn split(bundle: &[u8]) -> Result<Vec<&[u8]>> {
    let parts: Vec<&[u8]> = bundle.chunks(PART).collect();
    if parts.len() > MAX_JOB_BUNDLE_PARTS {
        return Err(format!("the commit's history is {} MiB as a git bundle, and a job carries at most {} MiB: this repo is too big to run as a job for now", bundle.len() / (1024 * 1024), MAX_JOB_BUNDLE_PARTS * PART / (1024 * 1024)));
    }
    Ok(parts)
}

pub fn join(parts: Vec<Vec<u8>>) -> Vec<u8> {
    parts.concat()
}

fn is_ancestor(repo: &Path, commit: &str, of: &str) -> bool {
    crate::repo::git(repo, &["merge-base", "--is-ancestor", commit, of]).is_ok()
}

// What the worker lacks of the commit under refname: from base on when the worker has base, else its whole history; nothing when base is the commit or builds on it, as git refuses an empty bundle.
pub fn create(repo: &Path, refname: &str, commit: &str, base: Option<&str>, scratch: &Path) -> Result<Vec<u8>> {
    if base.is_some_and(|base| is_ancestor(repo, commit, base)) {
        return Ok(Vec::new());
    }
    let file = scratch.with_extension("bundle");
    let path = file.to_string_lossy().to_string();
    let excluded = base.map(|base| format!("^{base}"));
    let mut args = vec!["bundle", "create", "--quiet", path.as_str(), refname];
    args.extend(excluded.as_deref());
    let made = crate::repo::git(repo, &args).and_then(|_| std::fs::read(&file).map_err(|error| format!("{}: {error}", file.display())));
    let _ = std::fs::remove_file(&file);
    made
}

pub type Put<'a> = dyn Fn(&str, &[u8]) -> std::result::Result<(), HubError> + 'a;

pub fn upload(bundle: &[u8], calls: &mut Calls, put: &Put) -> std::result::Result<Vec<BlobRef>, String> {
    let mut parts = Vec::new();
    for chunk in split(bundle)? {
        let (blob, ciphertext) = seal_blob(chunk, JOB_BUNDLE_MEDIA_TYPE);
        let path = format!("/v1/m/blobs/{}", blob.id);
        calls.call(&mut || put(&path, &ciphertext)).map_err(|error| hub_said(&error))?;
        parts.push(blob);
    }
    Ok(parts)
}

fn hub_said(error: &HubError) -> String {
    match error {
        HubError::Status { status: 413, code } if code == "blob-quota" => "the hub keeps no more of this machine's blobs: its quota is full".to_string(),
        error => error.to_string(),
    }
}

pub fn download(client: &Client, calls: &mut Calls, parts: &[BlobRef]) -> Result<Vec<u8>> {
    let mut opened = Vec::new();
    for part in parts {
        let path = format!("/v1/m/blobs/{}", part.id);
        let ciphertext = calls.call(&mut || client.get_bytes(&path)).map_err(|error| format!("part {}: {error}", part.id))?;
        opened.push(open_blob(part, &ciphertext).map_err(|error| format!("part {} does not open: {error:?}", part.id))?);
    }
    Ok(join(opened))
}

pub fn has(store: &Path, commit: &str) -> bool {
    crate::repo::git(store, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_ok()
}

#[derive(Debug, PartialEq)]
pub enum Stored {
    Ready,
    NeedsFull,
}

// The bundle's commits in the repo's bare store, kept under keep so the next incremental bundle finds its base; a base the store lacks asks the master for the whole history.
pub fn store(store: &Path, bundle: &[u8], commit: &str, base: Option<&str>, keep: &str) -> Result<Stored> {
    if !store.join("HEAD").is_file() {
        std::fs::create_dir_all(store).map_err(|error| format!("{}: {error}", store.display()))?;
        crate::repo::git(store, &["init", "--quiet", "--bare"])?;
    }
    if base.is_some_and(|base| !has(store, base)) {
        return Ok(Stored::NeedsFull);
    }
    if !bundle.is_empty() {
        let file = store.join(format!("kebbie-job-{}.bundle", std::process::id()));
        std::fs::write(&file, bundle).map_err(|error| format!("{}: {error}", file.display()))?;
        let fetched = crate::repo::git(store, &["fetch", "--quiet", "--no-write-fetch-head", &file.to_string_lossy(), &format!("+{}/*:{INCOMING}/*", super::REF)]);
        let _ = std::fs::remove_file(&file);
        fetched?;
        for incoming in crate::repo::git(store, &["for-each-ref", "--format=%(refname)", INCOMING])?.lines() {
            let _ = crate::repo::git(store, &["update-ref", "-d", incoming]);
        }
    }
    if !has(store, commit) {
        return if base.is_some() { Ok(Stored::NeedsFull) } else { Err(format!("the bundle does not hold {commit}")) };
    }
    crate::repo::git(store, &["update-ref", keep, commit])?;
    Ok(Stored::Ready)
}
