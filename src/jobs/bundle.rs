use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use kebbie_hub_protocol::{BLOB_OVERHEAD, BlobRef, JOB_BUNDLE_MEDIA_TYPE, MAX_BLOB_BYTES, MAX_JOB_BUNDLE_PARTS, open_blob, seal_blob};

use super::fetchable;
use crate::hub::http::{Client, HubError};
use crate::sh::Result;

pub const PART: usize = MAX_BLOB_BYTES - BLOB_OVERHEAD;
const INCOMING: &str = "refs/kebbie/incoming";
const FROM_REMOTE: &str = "refs/kebbie/origin";
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
    stop: &'a dyn Fn() -> bool,
}

impl<'a> Calls<'a> {
    pub fn new(clock: &'a dyn Fn() -> u64, sleep: &'a dyn Fn(Duration), stop: &'a dyn Fn() -> bool) -> Calls<'a> {
        Calls { pace: Pace::new(clock()), clock, sleep, stop }
    }

    pub fn call<T>(&mut self, once: &mut dyn FnMut() -> std::result::Result<T, HubError>) -> std::result::Result<T, HubError> {
        let mut tried = 0;
        loop {
            let wait = self.pace.next((self.clock)());
            if wait > 0 {
                (self.sleep)(Duration::from_millis(wait));
            }
            if (self.stop)() {
                return Err(HubError::Transport("kebbie was stopped".to_string()));
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

pub const MAX_BYTES: usize = MAX_JOB_BUNDLE_PARTS * PART;

pub fn mib(bytes: usize) -> String {
    let tenths = bytes * 10 / (1024 * 1024);
    format!("{}.{} MiB", tenths / 10, tenths % 10)
}

pub fn split(bundle: &[u8]) -> Result<Vec<&[u8]>> {
    if bundle.len() > MAX_BYTES {
        return Err(format!("the job's commit is {} as a git bundle, and a job carries at most {}", mib(bundle.len()), mib(MAX_BYTES)));
    }
    Ok(bundle.chunks(PART).collect())
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

// Each part is in uploaded once the hub has it, so a job that fails half way up still deletes the parts that went.
pub fn upload(bundle: &[u8], calls: &mut Calls, put: &Put, uploaded: &mut Vec<BlobRef>) -> std::result::Result<(), String> {
    for chunk in split(bundle)? {
        let (blob, ciphertext) = seal_blob(chunk, JOB_BUNDLE_MEDIA_TYPE);
        let path = format!("/v1/m/blobs/{}", blob.id);
        calls.call(&mut || put(&path, &ciphertext)).map_err(|error| hub_said(&error))?;
        uploaded.push(blob);
    }
    Ok(())
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

// Where a worker fetches the bulk of a job's history itself: origin's URL and the newest commit of the job that origin has, as its remote-tracking branches last saw it.
#[derive(Clone, Debug, PartialEq)]
pub struct Remote {
    pub url: String,
    pub base: String,
}

pub fn remote_base(repo: &Path, commit: &str) -> Option<Remote> {
    let url = fetchable(&crate::repo::git(repo, &["remote", "get-url", "origin"]).ok()?)?;
    let walked = crate::repo::git(repo, &["rev-list", "--boundary", commit, "--not", "--remotes=origin"]).ok()?;
    if walked.is_empty() {
        return Some(Remote { url, base: commit.to_string() });
    }
    let edges: Vec<&str> = walked.lines().filter_map(|line| line.strip_prefix('-')).collect();
    if edges.is_empty() {
        return None;
    }
    let mut args = vec!["log", "--no-walk", "--format=%H"];
    args.extend(&edges);
    let newest = crate::repo::git(repo, &args).ok()?.lines().next()?.to_string();
    Some(Remote { url, base: newest })
}

// Network protocols only, whatever the worker's git config allows, as GIT_ALLOW_PROTOCOL overrides every protocol.*.allow there; tests stand in for origin with a local repo.
pub const ALLOWED_PROTOCOLS: &str = if cfg!(test) { "https:http:ssh:git:file" } else { "https:http:ssh:git" };

// A git call on the worker, in a process group of its own that is killed once stop says so: the job's deadline or the keeper going.
pub fn git_until(dir: &Path, args: &[&str], stop: &dyn Fn() -> bool, prepare: impl FnOnce(&mut std::process::Command)) -> Result<String> {
    let mut command = crate::sh::git_command();
    command.current_dir(dir).args(args);
    prepare(&mut command);
    match crate::sh::output_until(command, stop).map_err(|error| format!("git {}: {error}", args.join(" ")))? {
        None => Err(format!("git {} was stopped", args.first().copied().unwrap_or_default())),
        Some(done) if done.code == Some(0) => Ok(done.stdout.trim().to_string()),
        Some(done) => Err(format!("git {} failed: {}", args.join(" "), done.stderr.trim())),
    }
}

// ssh as the user set it up, and never asking: BatchMode on top of their own GIT_SSH_COMMAND, core.sshCommand or GIT_SSH.
pub fn batch_ssh(own: Option<&str>) -> String {
    format!("{} -oBatchMode=yes", own.map(str::trim).filter(|own| !own.is_empty()).unwrap_or("ssh"))
}

fn own_ssh(store: &Path) -> Option<String> {
    std::env::var("GIT_SSH_COMMAND").ok().filter(|own| !own.trim().is_empty())
        .or_else(|| crate::repo::git(store, &["config", "--get", "core.sshCommand"]).ok().filter(|own| !own.is_empty()))
        .or_else(|| std::env::var("GIT_SSH").ok().filter(|own| !own.is_empty()).map(|own| crate::sh::quote(&own)))
}

// Fetches every branch of the remote into the store, with the worker's own credentials and never a prompt: no terminal, no askpass program, ssh in batch mode.
pub fn fetch_remote(store: &Path, url: &str, stop: &dyn Fn() -> bool) -> Result<()> {
    let ssh = batch_ssh(own_ssh(store).as_deref());
    git_until(store, &["fetch", "--quiet", "--no-tags", "--no-write-fetch-head", url, &format!("+refs/heads/*:{FROM_REMOTE}/*")], stop, |command| {
        command.envs([("GIT_TERMINAL_PROMPT", "0"), ("GIT_ASKPASS", "true"), ("GCM_INTERACTIVE", "never"), ("SSH_ASKPASS_REQUIRE", "never"), ("GIT_ALLOW_PROTOCOL", ALLOWED_PROTOCOLS), ("GIT_SSH_COMMAND", ssh.as_str())]).env_remove("SSH_ASKPASS");
    }).map(|_| ())
}

pub fn open_store(store: &Path) -> Result<()> {
    if !store.join("HEAD").is_file() {
        std::fs::create_dir_all(store).map_err(|error| format!("{}: {error}", store.display()))?;
        crate::repo::git(store, &["init", "--quiet", "--bare"])?;
    }
    Ok(())
}

static BUNDLES: AtomicU64 = AtomicU64::new(0);

// A file name no other store call in any kebbie process takes, as a link's runs and a job can store into one repo at once.
pub(super) fn scratch_bundle(store: &Path) -> PathBuf {
    store.join(format!("kebbie-job-{}-{}.bundle", std::process::id(), BUNDLES.fetch_add(1, Ordering::Relaxed)))
}

// The bundle's commits in the repo's bare store, kept under keep so the next incremental bundle finds its base; a base the store lacks is fetched from the job's remote when it names one, and else asks the master for the whole history.
#[allow(clippy::too_many_arguments)]
pub fn store(store: &Path, bundle: &[u8], commit: &str, base: Option<&str>, keep: &str, remote: Option<&str>, say: &mut dyn FnMut(String), stop: &dyn Fn() -> bool) -> Result<Stored> {
    open_store(store)?;
    if let Some(base) = base.filter(|base| !has(store, base)) {
        let Some(url) = remote else { return Ok(Stored::NeedsFull) };
        say(format!("[kebbie: fetching the history up to {} from {url}; only what is new comes on later jobs]\n", &base[..12.min(base.len())]));
        if let Err(error) = fetch_remote(store, url, stop) {
            if stop() {
                return Err(format!("the fetch from {url} was stopped"));
            }
            say(format!("[kebbie: the fetch from {url} failed: {error}]\n"));
            return Ok(Stored::NeedsFull);
        }
        if !has(store, base) {
            say(format!("[kebbie: {url} does not hold {base}]\n"));
            return Ok(Stored::NeedsFull);
        }
    }
    if !bundle.is_empty() {
        let file = scratch_bundle(store);
        std::fs::write(&file, bundle).map_err(|error| format!("{}: {error}", file.display()))?;
        let fetched = git_until(store, &["fetch", "--quiet", "--no-write-fetch-head", &file.to_string_lossy(), &format!("+{}/*:{INCOMING}/*", super::REF)], stop, |_| {});
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
