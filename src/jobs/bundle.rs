use std::path::Path;

use kebbie_hub_protocol::{BLOB_OVERHEAD, BlobRef, JOB_BUNDLE_MEDIA_TYPE, MAX_BLOB_BYTES, MAX_JOB_BUNDLE_PARTS, open_blob, seal_blob};

use crate::hub::http::{Client, HubError};
use crate::sh::Result;

pub const PART: usize = MAX_BLOB_BYTES - BLOB_OVERHEAD;
const INCOMING: &str = "refs/kebbie/incoming";

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

// What the worker lacks of the commit under refname: from base on when the worker has base, else its whole history; nothing when base is the commit itself.
pub fn create(repo: &Path, refname: &str, commit: &str, base: Option<&str>, scratch: &Path) -> Result<Vec<u8>> {
    if base == Some(commit) {
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

pub fn upload(client: &Client, bundle: &[u8]) -> std::result::Result<Vec<BlobRef>, String> {
    let mut parts = Vec::new();
    for chunk in split(bundle)? {
        let (blob, ciphertext) = seal_blob(chunk, JOB_BUNDLE_MEDIA_TYPE);
        client.put_bytes(&format!("/v1/m/blobs/{}", blob.id), &ciphertext).map_err(|error| hub_said(&error))?;
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

pub fn download(client: &Client, parts: &[BlobRef]) -> Result<Vec<u8>> {
    let mut opened = Vec::new();
    for part in parts {
        let ciphertext = client.get_bytes(&format!("/v1/m/blobs/{}", part.id)).map_err(|error| format!("part {}: {error}", part.id))?;
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
