use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use crate::report_wire::{BugReport, Bytes32, Bytes64, InstallKey, MAX_TEXT_BYTES, MAX_TITLE_CHARS, ReportAccepted, ReportChallenge, ReportUpload, solve};
use serde::{Deserialize, Serialize};

use super::http::{CALL, Client, HubError};
use crate::issues::{self, Disposition, Issue};
use crate::permit::Caller;
use crate::sh::Result;

// A hub asking for more than this would keep a laptop busy for minutes, so kebbie refuses rather than spins.
pub const MAX_WORK: u8 = 26;
pub const DIR: &str = "report";
const KEY_FILE: &str = "install-key.json";
const SHORTENED: &str = "\n\n(shortened)";
const TOKEN_PREFIXES: [&str; 14] = ["khm_", "khd_", "khs_", "kha_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_", "glpat-", "sk-", "xoxb-", "xoxp-"];
const SECRET_NAMES: [&str; 6] = ["token", "secret", "password", "passwd", "api_key", "apikey"];

pub fn dir(state: &Path) -> PathBuf {
    state.join(DIR)
}

const HOME_ROOTS: [&str; 4] = ["/var/home/", "/export/home/", "/Users/", "/home/"];
const TLDS: [&str; 34] = ["com", "net", "org", "io", "dev", "app", "local", "internal", "lan", "home", "arpa", "cloud", "ai", "co", "se", "uk", "de", "us", "eu", "nl", "no", "dk", "fi", "fr", "ch", "at", "be", "xyz", "tech", "site", "online", "test", "example", "corp"];
const FILE_EXTENSIONS: [&str; 24] = ["rs", "toml", "md", "json", "yml", "yaml", "txt", "lock", "sh", "py", "rb", "js", "ts", "tsx", "jsx", "css", "html", "log", "sqlite3", "db", "png", "jpg", "gz", "zip"];
const BRANCH_PREFIXES: [&str; 9] = ["feature/", "feat/", "fix/", "bugfix/", "hotfix/", "release/", "refs/heads/", "refs/remotes/", "origin/"];

// What a report must never carry, as this machine knows it: its home, its user's name, the repos kebbie works in, and which commits are kebbie's own.
#[derive(Clone, Debug, Default)]
pub struct Scrub {
    pub home: Option<String>,
    pub users: Vec<String>,
    pub repos: Vec<String>,
    pub kebbie_root: Option<PathBuf>,
}

impl Scrub {
    pub fn current() -> Scrub {
        let mut repos: Vec<String> = crate::repo::checkouts().iter().map(|path| crate::repo::name_of(path)).collect();
        repos.extend(std::fs::read_dir(crate::config::repos_dir()).into_iter().flatten().flatten()
            .filter_map(|entry| entry.path().file_stem().map(|stem| stem.to_string_lossy().to_string())));
        let mut scrub = Scrub::new(std::env::var("HOME").ok(), repos);
        scrub.users.extend(std::env::var("USER").ok().filter(|user| user.chars().count() >= 2));
        scrub.kebbie_root = crate::lead::install_root().ok();
        scrub
    }

    pub fn new(home: Option<String>, repos: Vec<String>) -> Scrub {
        let mut repos: Vec<String> = repos.into_iter().filter(|name| name.chars().count() >= 2 && name != "kebbie").collect();
        repos.sort_by_key(|name| std::cmp::Reverse(name.len()));
        repos.dedup();
        let home = home.map(|home| home.trim_end_matches('/').to_string()).filter(|home| home.len() > 1);
        let users = home.as_deref().and_then(|home| home.rsplit('/').next()).filter(|user| user.chars().count() >= 2).map(str::to_string).into_iter().collect();
        Scrub { home, users, repos, kebbie_root: None }
    }

    fn kebbie_commit(&self, sha: &str) -> bool {
        self.kebbie_root.as_deref().is_some_and(|root| crate::sh::succeeds("git", &["cat-file", "-e", &format!("{sha}^{{commit}}")], Some(root)))
    }
}

fn word_char(character: char) -> bool {
    character.is_alphanumeric()
}

fn path_char(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '.' | '-' | '_')
}

fn invisible(character: char) -> bool {
    matches!(character, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

// Terminal escapes go whole, with their parameters; every other control character but newline and tab goes too.
pub fn strip_controls(text: &str) -> String {
    let mut out = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\u{1b}' => match characters.peek() {
                Some('[') => {
                    characters.next();
                    while characters.next().is_some_and(|next| !('\u{40}'..='\u{7e}').contains(&next)) {}
                }
                Some(']') => {
                    characters.next();
                    while let Some(next) = characters.next() {
                        if next == '\u{7}' || (next == '\u{1b}' && characters.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {
                    characters.next();
                }
            },
            '\n' | '\t' => out.push(character),
            other if other.is_control() || invisible(other) => {}
            other => out.push(other),
        }
    }
    out
}

// Another account's home, as /Users/<name>, /home/<name> or /var/home/<name>, reads as ~ too.
fn other_homes(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some((start, root)) = HOME_ROOTS.iter().filter_map(|root| rest.find(root).map(|at| (at, root.len()))).min() {
        let before = rest[..start].chars().next_back();
        let name: String = rest[start + root..].chars().take_while(|character| path_char(*character)).collect();
        out.push_str(&rest[..start]);
        if before.is_none_or(|character| !path_char(character) && character != '/') && !name.is_empty() {
            out.push('~');
        } else {
            out.push_str(&rest[start..start + root]);
            out.push_str(&name);
        }
        rest = &rest[start + root + name.len()..];
    }
    out.push_str(rest);
    out
}

// The user's name as a segment of any path, such as /srv/ada/app.
fn user_segments(text: &str, user: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    let needle = format!("/{user}");
    while let Some(at) = rest.find(&needle) {
        let after = rest[at + needle.len()..].chars().next();
        out.push_str(&rest[..at]);
        out.push_str(if after.is_none_or(|character| !path_char(character)) { "/<user>" } else { &needle });
        rest = &rest[at + needle.len()..];
    }
    out.push_str(rest);
    out
}

fn hex(core: &str) -> bool {
    core.chars().all(|character| character.is_ascii_hexdigit())
}

fn opaque(core: &str) -> bool {
    core.len() >= 32 && !hex(core)
        && core.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '+' | '/' | '='))
        && core.chars().any(|character| character.is_ascii_digit())
        && core.chars().any(|character| character.is_ascii_alphabetic())
}

fn aws_key_id(core: &str) -> bool {
    (core.starts_with("AKIA") || core.starts_with("ASIA")) && core.len() >= 16 && core.chars().all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
}

fn host(core: &str) -> bool {
    let labels: Vec<&str> = core.trim_end_matches('.').split('.').collect();
    let Some(last) = labels.last().map(|last| last.to_ascii_lowercase()) else { return false };
    labels.len() >= 2
        && labels.iter().all(|label| !label.is_empty() && label.chars().all(|character| character.is_ascii_alphanumeric() || character == '-'))
        && last.len() >= 2 && last.chars().all(|character| character.is_ascii_alphabetic())
        && (TLDS.contains(&last.as_str()) || labels.len() >= 3 && !FILE_EXTENSIONS.contains(&last.as_str()))
}

fn ip(core: &str) -> bool {
    let bare = match core.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => match core.rsplit_once(':') {
            Some((address, port)) if core.matches(':').count() == 1 && port.chars().all(|character| character.is_ascii_digit()) => address,
            _ => core,
        },
    };
    let bare = bare.split('%').next().unwrap_or_default();
    bare.parse::<std::net::IpAddr>().is_ok()
}

fn email(core: &str) -> bool {
    core.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && !domain.contains(':') && host(domain))
}

fn scp_remote(core: &str) -> bool {
    let Some((user, rest)) = core.split_once('@') else { return false };
    let Some((host, path)) = rest.split_once(':') else { return false };
    !user.is_empty() && host.contains('.') && !path.is_empty() && !path.starts_with(|character: char| character.is_ascii_digit())
}

fn secret_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_NAMES.iter().any(|secret| name.contains(secret)) || name.ends_with("_key") || name == "key"
}

fn kebbies_own(core: &str) -> bool {
    let without_scheme = issues::REPOSITORY.trim_start_matches("https://");
    core.starts_with(issues::REPOSITORY) || core.starts_with(without_scheme)
}

fn redact_word(core: &str, after_bearer: bool, scrub: &Scrub) -> Option<String> {
    if core.is_empty() || kebbies_own(core) {
        return None;
    }
    let replaced = |label: &str| Some(format!("<{label}>"));
    if after_bearer {
        return replaced("token");
    }
    if core.contains("://") {
        return replaced("url");
    }
    if email(core) {
        return replaced("email");
    }
    if scp_remote(core) {
        return replaced("remote");
    }
    if let Some(split) = core.find(['=', ':']) && split + 1 < core.len() && secret_name(core[..split].trim_start_matches(['-', '$'])) {
        return Some(format!("{}<redacted>", &core[..=split]));
    }
    if TOKEN_PREFIXES.iter().any(|prefix| core.starts_with(prefix) && core.len() > prefix.len() + 6) || aws_key_id(core) || opaque(core) {
        return replaced("token");
    }
    if core.len() >= 32 && hex(core) && !scrub.kebbie_commit(core) {
        return replaced("token");
    }
    if ip(core) {
        return replaced("ip");
    }
    if BRANCH_PREFIXES.iter().any(|prefix| core.starts_with(prefix) && core.len() > prefix.len()) {
        return replaced("branch");
    }
    if let Some((first, _)) = core.split_once('/') && host(first) {
        return replaced("url");
    }
    if host(core) {
        return replaced("host");
    }
    None
}

// A word after which the next one is the secret: Bearer, Basic after Authorization:, or a secret's name ending in : or =.
fn opens_secret(core: &str, previous: &str) -> bool {
    let assigned = core.strip_suffix([':', '=']).is_some_and(|name| secret_name(name.trim_start_matches(['-', '$'])));
    core.eq_ignore_ascii_case("bearer") || core.eq_ignore_ascii_case("token") || (core.eq_ignore_ascii_case("basic") && previous.eq_ignore_ascii_case("authorization:")) || assigned
}

fn redact_words(text: &str, scrub: &Scrub) -> String {
    let mut out = String::new();
    let mut bearer = false;
    let mut previous = String::new();
    let mut word = String::new();
    let mut flush = |word: &mut String, out: &mut String, bearer: &mut bool| {
        if word.is_empty() {
            return;
        }
        let start = word.find(|character: char| !matches!(character, '"' | '\'' | '(' | '[' | '<' | '{' | '`')).unwrap_or(word.len());
        let end = word.rfind(|character: char| !matches!(character, '"' | '\'' | ')' | ']' | '>' | '}' | ',' | '.' | ';' | '!' | '?' | '`')).map_or(start, |end| end + word[end..].chars().next().map_or(1, char::len_utf8));
        let start = if start > 0 && word[..start].ends_with('[') && ip(&word[start - 1..end.max(start)]) { start - 1 } else { start };
        let core = &word[start..end.max(start)];
        let replaced = redact_word(core, *bearer, scrub);
        *bearer = opens_secret(core, &previous);
        previous = core.to_string();
        match replaced {
            Some(replacement) => out.push_str(&format!("{}{replacement}{}", &word[..start], &word[end.max(start)..])),
            None => out.push_str(word),
        }
        word.clear();
    };
    for character in text.chars() {
        if character.is_whitespace() {
            flush(&mut word, &mut out, &mut bearer);
            out.push(character);
        } else {
            word.push(character);
        }
    }
    flush(&mut word, &mut out, &mut bearer);
    out
}

fn redact_repo(text: &str, name: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(name) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + name.len()..].chars().next();
        out.push_str(&rest[..at]);
        if before.is_none_or(|character| !word_char(character)) && after.is_none_or(|character| !word_char(character)) {
            out.push_str("<repo>");
        } else {
            out.push_str(name);
        }
        rest = &rest[at + name.len()..];
    }
    out.push_str(rest);
    out
}

pub fn redact(text: &str, scrub: &Scrub) -> String {
    let mut text = strip_controls(text);
    if let Some(home) = &scrub.home {
        text = text.replace(home.as_str(), "~");
    }
    let mut text = other_homes(&text);
    for user in &scrub.users {
        text = user_segments(&text, user);
    }
    let mut text = redact_words(&text, scrub);
    for name in &scrub.repos {
        text = redact_repo(&text, name);
    }
    text
}

fn shortened(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit - SHORTENED.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{SHORTENED}", text[..end].trim_end())
}

// Exactly what goes out: the user reads this before saying yes, and the hub gets nothing else.
pub fn payload(issue: &Issue, scrub: &Scrub) -> BugReport {
    let title: String = redact(&issue.title, scrub).split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_TITLE_CHARS).collect();
    BugReport { title, text: shortened(&redact(&issue.body, scrub), MAX_TEXT_BYTES), version: issue.built(), os: issues::os() }
}

pub fn preview(report: &BugReport) -> String {
    let text = if report.text.trim().is_empty() { "(no text)" } else { report.text.trim() };
    format!("{}\nkebbie {} · {}\n\n{text}", report.title, report.version, report.os)
}

pub fn offered(issue: &Issue) -> bool {
    issue.is_open() && issue.disposition() == Disposition::Unsent
}

pub fn offers(state: &Path, enabled: bool) -> Vec<Issue> {
    if !enabled {
        return Vec::new();
    }
    sent_known(state, issues::load(&issues::dir(state))).into_iter().filter(offered).collect()
}

#[derive(Serialize, Deserialize)]
struct KeyFile {
    secret: Bytes32,
    public: Bytes32,
}

fn key_file(state: &Path) -> PathBuf {
    dir(state).join(KEY_FILE)
}

pub fn install_key(state: &Path) -> Result<InstallKey> {
    let file = key_file(state);
    match std::fs::read_to_string(&file) {
        Ok(text) => {
            let kept: KeyFile = serde_json::from_str(&text).map_err(|error| format!("{}: {error}", file.display()))?;
            let key = InstallKey::from_bytes(kept.secret.0);
            if key.public() != kept.public.0 {
                return Err(crate::signals::internal("report.install-key", crate::report_wire::SignalKind::CorruptState, format!("{}: the public key does not match the secret one; move the file away and kebbie makes a new one", file.display())));
            }
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let key = InstallKey::from_bytes(super::identity::random32()?);
            let kept = KeyFile { secret: Bytes32(key.to_bytes()), public: Bytes32(key.public()) };
            crate::state::write(&file, serde_json::to_string_pretty(&kept).map_err(|error| error.to_string())?)?;
            Ok(key)
        }
        Err(error) => Err(format!("{}: {error}", file.display())),
    }
}

const BUSY: &str = "the reports endpoint is busy; try again in a few minutes";

enum Refused {
    Busy,
    Other(String),
}

impl From<String> for Refused {
    fn from(error: String) -> Refused {
        Refused::Other(error)
    }
}

fn refused(error: HubError) -> Refused {
    match error.status() {
        Some(429) => Refused::Busy,
        _ => Refused::Other(error.to_string()),
    }
}

pub const UNASKED_EVERY: u64 = 60;

fn deliver(state: &Path, issue: &mut Issue, report: BugReport, url: &str, now: u64) -> Result<String> {
    attempt(state, issue, report, url, now).map_err(|refusal| match refusal {
        Refused::Busy => BUSY.to_string(),
        Refused::Other(error) => error,
    })
}

fn attempt(state: &Path, issue: &mut Issue, report: BugReport, url: &str, now: u64) -> std::result::Result<String, Refused> {
    let key = install_key(state)?;
    let client = Client::new(&super::hub_url(url)?, None, CALL);
    let challenge: ReportChallenge = client.get("/v1/reports/challenge").map_err(refused)?;
    if challenge.difficulty > MAX_WORK {
        return Err(Refused::Other(format!("the reports endpoint asks for {} bits of work, more than kebbie does ({MAX_WORK}); nothing was sent", challenge.difficulty)));
    }
    let counter = solve(&challenge.nonce.0, &report.digest(), challenge.difficulty);
    let title = report.title.clone();
    let upload = ReportUpload { signature: Bytes64(key.sign(&challenge.nonce.0, &report)), key: Bytes32(key.public()), report, challenge, counter };
    let accepted: ReportAccepted = client.post("/v1/reports", &upload).map_err(refused)?;
    let id = accepted.report.to_string();
    let ledgered = record_sent(state, &issue.id, &id);
    issue.report = Some(id.clone());
    issue.sent = Some(issues::today(now));
    if let Err(error) = issues::save(&issues::dir(state), issue) {
        let kept = if ledgered.is_ok() { "kebbie will not send it again" } else { "kebbie could not record that either, so do not send it again" };
        return Err(Refused::Other(format!("Bug {} went to kebbie's maintainers as report {id}, but saving the issue failed ({error}); {kept}", issue.id)));
    }
    let _ = crate::messages::log_for_user_in(state, &sent_line(&id, &issue.id, &title), SENT_KIND);
    Ok(id)
}

pub const SENT_KIND: &str = "report-sent";
const LEDGER: &str = "sent";

// Appended the moment the hub takes a report, apart from the issue file, so a failed save of the issue never offers the bug again.
fn record_sent(state: &Path, issue: &str, report: &str) -> Result<()> {
    let path = dir(state).join(LEDGER);
    crate::state::append_to(&path).and_then(|mut file| writeln!(file, "{issue} {report}")).map_err(|error| format!("{}: {error}", path.display()))
}

fn sent_ledger(state: &Path) -> std::collections::HashMap<String, String> {
    std::fs::read_to_string(dir(state).join(LEDGER)).unwrap_or_default().lines()
        .filter_map(|line| line.split_once(' ')).map(|(issue, report)| (issue.to_string(), report.trim().to_string())).collect()
}

fn sent_known(state: &Path, mut all: Vec<Issue>) -> Vec<Issue> {
    let ledger = sent_ledger(state);
    for issue in all.iter_mut().filter(|issue| issue.report.is_none()) {
        issue.report = ledger.get(&issue.id).cloned();
    }
    all
}

// A bug that came back is a new report to make, so the send before it stops counting; the lock keeps a send's line from landing mid-rewrite.
pub fn forget_sent(state: &Path, issue: &str) -> Result<Option<String>> {
    let _lock = SendLock::take(state)?;
    let path = dir(state).join(LEDGER);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let (gone, kept): (Vec<&str>, Vec<&str>) = text.lines().partition(|line| line.split_once(' ').is_some_and(|(id, _)| id == issue));
    let Some(last) = gone.last() else { return Ok(None) };
    crate::state::write(&path, kept.iter().map(|line| format!("{line}\n")).collect::<String>())?;
    Ok(last.split_once(' ').map(|(_, report)| report.trim().to_string()))
}

pub fn find_sent(state: &Path, id: &str) -> Result<Issue> {
    let issue = issues::find(&issues::dir(state), id)?;
    Ok(sent_known(state, vec![issue]).remove(0))
}

// Held from a bug's last read to its report id being saved, so two sends on this machine never post the same bug.
struct SendLock(std::fs::File);

impl SendLock {
    fn take(state: &Path) -> Result<SendLock> {
        let path = dir(state).join("send.lock");
        let file = crate::state::append_to(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        // SAFETY: flock on a descriptor this File owns.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(format!("{}: {}", path.display(), std::io::Error::last_os_error()));
        }
        Ok(SendLock(file))
    }
}

impl Drop for SendLock {
    // A child forked by another thread holds a copy of the descriptor until it execs; unlocking releases the lock for every copy at once.
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

enum Skip {
    Already(String),
    NotOurs,
    Changed,
    Missing(String),
}

impl Skip {
    fn refusal(self, id: &str) -> String {
        match self {
            Skip::Already(report) => format!("Bug {id} went to kebbie's maintainers already, as report {report}"),
            Skip::Changed => format!("Bug {id} changed since you read it, so nothing was sent; read it again with kebbie issue report {id}"),
            Skip::NotOurs => format!("{id} is a wish or another install's report; only a bug you ran into goes to kebbie's maintainers"),
            Skip::Missing(error) => error,
        }
    }

    fn why(self, id: &str) -> String {
        match self {
            Skip::Already(report) => format!("went already, as report {report}"),
            Skip::NotOurs => "a wish or another install's report; only a bug you ran into goes".to_string(),
            Skip::Changed => format!("it changed since you read it; kebbie issue report {id} shows it again"),
            Skip::Missing(error) => error,
        }
    }
}

fn sendable(issue: &Issue) -> std::result::Result<(), Skip> {
    match issue.disposition() {
        Disposition::NotOurs => Err(Skip::NotOurs),
        Disposition::Reported(report) => Err(Skip::Already(report)),
        _ => Ok(()),
    }
}

// The bug as it stands now, if it is still the one the user read and nothing has sent it.
fn ready(state: &Path, id: &str, read: &[u8; 32], scrub: &Scrub) -> std::result::Result<(Issue, BugReport), Skip> {
    let issue = find_sent(state, id).map_err(Skip::Missing)?;
    sendable(&issue)?;
    let report = payload(&issue, scrub);
    if report.digest() != *read {
        return Err(Skip::Changed);
    }
    Ok((issue, report))
}

pub fn sent_line(report: &str, issue: &str, title: &str) -> String {
    format!("Sent bug {issue} to kebbie's maintainers as report {report}: \"{title}\", redacted; kebbie issue report {issue} shows the text that went.")
}

// The digest is of the report the user read; anything else that would go out instead is refused.
pub fn send_in(state: &Path, caller: &Caller, id: &str, url: &str, scrub: &Scrub, read: &[u8; 32], now: u64) -> Result<String> {
    crate::permit::user_only_as(caller, "Sending a report to kebbie's maintainers")?;
    let _lock = SendLock::take(state)?;
    let (mut issue, report) = ready(state, id, read, scrub).map_err(|skip| skip.refusal(id))?;
    let sent = deliver(state, &mut issue, report, url, now)?;
    crate::notice::Notice::new(crate::notice::Tone::Done, format!("Sent bug {} to kebbie's maintainers as report {sent}", issue.id)).next("It stays on this machine too; after each update the keeper rechecks it and closes it once a commit plainly fixes it.").ok()
}

// The endpoint takes 5 signed reports per install key in a fixed 5-minute window; a refused try counts too, so a wait spans a whole window.
pub const BURST: usize = 5;
pub const WINDOW: std::time::Duration = std::time::Duration::from_secs(305);
const WAITS: usize = 3;

pub struct Pace<'a> {
    pub clock: &'a dyn Fn() -> std::time::Instant,
    pub wait: &'a dyn Fn(std::time::Duration),
    pub say: &'a dyn Fn(&str),
}

#[derive(Debug, Default, PartialEq)]
pub struct Batch {
    pub sent: Vec<(String, String)>,
    pub already: Vec<(String, String)>,
    pub failed: Vec<(String, String)>,
}

fn bugs(count: usize) -> String {
    if count == 1 { "1 bug".to_string() } else { format!("{count} bugs") }
}

fn minutes(wait: std::time::Duration) -> String {
    let minutes = wait.as_secs().div_ceil(60).max(1);
    if minutes == 1 { "a minute".to_string() } else { format!("{minutes} minutes") }
}

impl Batch {
    fn skipped(&mut self, id: &str, skip: Skip) {
        match skip {
            Skip::Already(report) => self.already.push((id.to_string(), report)),
            skip => self.failed.push((id.to_string(), skip.why(id))),
        }
    }

    pub fn notice(&self) -> Result<String> {
        let headline = match (self.sent.len(), self.failed.len()) {
            (0, 0) => "Sent nothing: every bug had gone to kebbie's maintainers already".to_string(),
            (sent, 0) => format!("Sent {} to kebbie's maintainers", bugs(sent)),
            (0, failed) => format!("Sent none of {} to kebbie's maintainers", bugs(failed)),
            (sent, failed) => format!("Sent {} to kebbie's maintainers; {failed} did not go", bugs(sent)),
        };
        let tone = if self.failed.is_empty() { crate::notice::Tone::Done } else { crate::notice::Tone::Failed };
        let mut notice = crate::notice::Notice::new(tone, headline);
        for (issue, report) in &self.sent {
            notice = notice.detail("sent", format!("{issue} as report {report}"));
        }
        for (issue, report) in &self.already {
            notice = notice.detail("already sent", format!("{issue} as report {report}"));
        }
        for (issue, why) in &self.failed {
            notice = notice.detail("not sent", format!("{issue}: {why}"));
        }
        if !self.failed.is_empty() {
            notice = notice.next("kebbie issue report --all sends the ones left; none goes twice.");
        }
        if self.failed.is_empty() { notice.ok() } else { notice.err() }
    }
}

pub fn send_all_in(state: &Path, caller: &Caller, read: &[(String, [u8; 32])], url: &str, scrub: &Scrub, pace: &Pace, now: u64) -> Result<Batch> {
    crate::permit::user_only_as(caller, "Sending reports to kebbie's maintainers")?;
    let mut batch = Batch::default();
    let mut recent: std::collections::VecDeque<std::time::Instant> = std::collections::VecDeque::new();
    let mut stuck = false;
    for (id, digest) in read {
        if stuck {
            batch.failed.push((id.clone(), "not tried, the reports endpoint stayed busy".to_string()));
            continue;
        }
        if let Err(skip) = ready(state, id, digest, scrub) {
            batch.skipped(id, skip);
            continue;
        }
        let mut waits = 0;
        loop {
            if recent.len() >= BURST {
                let until = recent[0] + WINDOW;
                let left = until.saturating_duration_since((pace.clock)());
                if !left.is_zero() {
                    (pace.say)(&format!("Sent {}; the endpoint takes {BURST} per 5 minutes, so the next goes in {}…", bugs(batch.sent.len()), minutes(left)));
                    (pace.wait)(left);
                }
                recent.pop_front();
                continue;
            }
            let lock = SendLock::take(state)?;
            let (mut issue, report) = match ready(state, id, digest, scrub) {
                Ok(ready) => ready,
                Err(skip) => {
                    batch.skipped(id, skip);
                    break;
                }
            };
            match attempt(state, &mut issue, report, url, now) {
                Ok(sent) => {
                    recent.push_back((pace.clock)());
                    (pace.say)(&format!("Sent bug {} as report {sent}.", issue.id));
                    batch.sent.push((issue.id.clone(), sent));
                    break;
                }
                Err(Refused::Busy) if waits < WAITS => {
                    drop(lock);
                    waits += 1;
                    (pace.say)(&format!("The reports endpoint is busy; waiting {} before bug {} goes…", minutes(WINDOW), issue.id));
                    (pace.wait)(WINDOW);
                    recent.clear();
                }
                Err(Refused::Busy) => {
                    batch.failed.push((issue.id.clone(), format!("the reports endpoint stayed busy after {WAITS} waits of 5 minutes")));
                    stuck = true;
                    break;
                }
                Err(Refused::Other(error)) => {
                    batch.failed.push((issue.id.clone(), error));
                    break;
                }
            }
        }
    }
    Ok(batch)
}

const ARMED: &str = "always-armed";
pub const ARM_COMMAND: &str = "kebbie issue report --always";
pub const DISARM_COMMAND: &str = "kebbie issue report --ask";

// The lead can write config.toml, so text = "always" alone sends nothing; it waits for the user's own arming, which only a user-only command writes.
pub fn armed(state: &Path) -> bool {
    dir(state).join(ARMED).exists()
}

pub fn board_offers(settings: &crate::config::Report, state: &Path) -> bool {
    match settings.text() {
        crate::config::ReportText::Ask => true,
        crate::config::ReportText::Always => !armed(state),
        crate::config::ReportText::Never => false,
    }
}

pub fn arm_in(state: &Path, caller: &Caller, on: bool, text: crate::config::ReportText, now: u64) -> Result<String> {
    let file = dir(state).join(ARMED);
    if !on {
        crate::permit::user_only_as(caller, "Choosing how reports go to kebbie's maintainers")?;
        if file.exists() {
            std::fs::remove_file(&file).map_err(|error| format!("{}: {error}", file.display()))?;
        }
        return Ok("kebbie now shows you each bug before it goes to its maintainers, and sends it only on your yes.".to_string());
    }
    crate::permit::user_only_as(caller, "Letting kebbie send reports to its maintainers without asking")?;
    crate::state::write(&file, issues::today(now))?;
    Ok(if text == crate::config::ReportText::Always {
        format!("kebbie now sends each bug you run into to its maintainers without asking, redacted, and each send is a line under messages. {DISARM_COMMAND} stops it.")
    } else {
        format!("Armed, but [report] text is \"{}\" in config.toml; with text = \"always\" there, kebbie sends bug reports without asking. {DISARM_COMMAND} disarms it.", text.key())
    })
}

pub fn arm_cli(on: bool) -> Result<String> {
    let text = crate::config::report_settings()?.text();
    arm_in(&crate::lead::state_dir()?, &crate::permit::caller(), on, text, crate::messages::now())
}

// Only text = "always" in config.toml together with the user's arming sends a report nobody was asked about; it goes out redacted like any other.
pub fn send_unasked_in(state: &Path, text: crate::config::ReportText, url: &str, scrub: &dyn Fn() -> Scrub, now: u64) -> Result<Vec<String>> {
    if text != crate::config::ReportText::Always || !armed(state) {
        return Ok(Vec::new());
    }
    let waiting = offers(state, true);
    if waiting.is_empty() {
        return Ok(Vec::new());
    }
    let scrub = scrub();
    let mut notes = Vec::new();
    for waited in waiting {
        let _lock = SendLock::take(state)?;
        let Some(mut issue) = find_sent(state, &waited.id).ok().filter(offered) else { continue };
        let report = payload(&issue, &scrub);
        let sent = deliver(state, &mut issue, report, url, now)?;
        notes.push(format!("reports: sent bug {} to kebbie's maintainers as report {sent}, as [report] text = \"always\" says", issue.id));
    }
    Ok(notes)
}

pub fn send_unasked_tick(state: &Path) -> Result<Vec<String>> {
    let settings = crate::config::report_settings()?;
    if settings.text() != crate::config::ReportText::Always {
        return Ok(Vec::new());
    }
    crate::signals::one_sender(state, || send_unasked_in(state, settings.text(), settings.url(), &Scrub::current, crate::messages::now()))
}

pub fn decline_in(state: &Path, caller: &Caller, id: &str, now: u64) -> Result<String> {
    crate::permit::user_only_as(caller, "Deciding on a report to kebbie's maintainers")?;
    let dir = issues::dir(state);
    let mut issue = issues::find(&dir, id)?;
    issue.declined = Some(issues::today(now));
    issues::save(&dir, &issue)?;
    crate::notice::Notice::new(crate::notice::Tone::Done, format!("Kept bug {} on this machine only", issue.id)).next(format!("kebbie issue report {} sends it to kebbie's maintainers later.", issue.id)).ok()
}

fn answered_yes() -> bool {
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes")
}

struct Asked {
    caller: Caller,
    state: PathBuf,
    settings: crate::config::Report,
}

fn asked(what: &str) -> Result<Asked> {
    let caller = crate::permit::caller();
    crate::permit::user_only_as(&caller, what)?;
    Ok(Asked { caller, state: crate::lead::state_dir()?, settings: crate::config::report_settings()? })
}

struct Question {
    shown: String,
    elsewhere: String,
    goes: String,
    ask: String,
}

enum Answer {
    Yes,
    No,
    Elsewhere(String),
}

fn put(question: Question) -> Answer {
    if !super::interactive() {
        return Answer::Elsewhere(format!("{}\n\n{}", question.shown, question.elsewhere));
    }
    println!("{}\n\n{}", question.shown, question.goes);
    print!("{} [y/N] ", question.ask);
    if answered_yes() { Answer::Yes } else { Answer::No }
}

pub fn cli(id: &str) -> Result<String> {
    let Asked { caller, state, settings } = asked("Sending a report to kebbie's maintainers")?;
    report_in(&state, &caller, id, settings.url(), &Scrub::current(), &put, crate::messages::now())
}

fn report_in(state: &Path, caller: &Caller, id: &str, url: &str, scrub: &Scrub, put: &dyn Fn(Question) -> Answer, now: u64) -> Result<String> {
    let issue = find_sent(state, id)?;
    let read = payload(&issue, scrub);
    match sendable(&issue) {
        Err(Skip::Already(report)) => return Ok(format!("{}\n\n{}.", preview(&read), Skip::Already(report).refusal(&issue.id))),
        Err(skip) => return Err(skip.refusal(&issue.id)),
        Ok(()) => {}
    }
    let question = Question {
        shown: preview(&read),
        elsewhere: format!("This is the report as it would go to kebbie's maintainers at {url}. Run kebbie issue report {} in your own terminal to answer y or n.", issue.id),
        goes: format!("This goes to kebbie's maintainers at {url}, signed with this install's key."),
        ask: "Send this report to kebbie's maintainers?".to_string(),
    };
    match put(question) {
        Answer::Elsewhere(text) => Ok(text),
        Answer::Yes => send_in(state, caller, id, url, scrub, &read.digest(), now),
        Answer::No => decline_in(state, caller, id, now),
    }
}

fn left_out(issue: &Issue) -> bool {
    issue.is_open() && issue.disposition().left_out()
}

pub fn all_preview(reports: &[(String, BugReport)]) -> String {
    reports.iter().map(|(id, report)| format!("── bug {id} ──\n{}", preview(report))).collect::<Vec<_>>().join("\n\n")
}

pub fn cli_all() -> Result<String> {
    let Asked { caller, state, settings } = asked("Sending reports to kebbie's maintainers")?;
    let all = sent_known(&state, issues::load(&issues::dir(&state)));
    let waiting: Vec<&Issue> = all.iter().filter(|issue| offered(issue)).collect();
    let already: Vec<String> = all.iter().filter(|issue| issue.is_open()).filter_map(|issue| match issue.disposition() {
        Disposition::Reported(report) => Some(format!("{} as report {report}", issue.id)),
        _ => None,
    }).collect();
    let kept: Vec<&str> = all.iter().filter(|issue| left_out(issue)).map(|issue| issue.id.as_str()).collect();
    let mut aside = Vec::new();
    if !already.is_empty() {
        aside.push(format!("Already sent: {}.", already.join(", ")));
    }
    if !kept.is_empty() {
        aside.push(format!("Left out, kept here or opened on GitHub: {}; kebbie issue report <id> sends one.", kept.join(", ")));
    }
    let aside = aside.join("\n");
    if waiting.is_empty() {
        return crate::notice::Notice::new(crate::notice::Tone::Done, "No bug waits to go to kebbie's maintainers").next(if aside.is_empty() { "kebbie issues lists the bugs on this machine.".to_string() } else { aside }).ok();
    }
    let scrub = Scrub::current();
    let reports: Vec<(String, BugReport)> = waiting.iter().map(|issue| (issue.id.clone(), payload(issue, &scrub))).collect();
    let aside = if aside.is_empty() { String::new() } else { format!("\n\n{aside}") };
    let question = Question {
        shown: format!("{}{aside}", all_preview(&reports)),
        elsewhere: format!("These are the {} as they would go to kebbie's maintainers at {}. Run kebbie issue report --all in your own terminal to answer y or n.", bugs(reports.len()), settings.url()),
        goes: format!("These {} go to kebbie's maintainers at {}, each signed with this install's key; {BURST} go at once, then {BURST} every 5 minutes.", bugs(reports.len()), settings.url()),
        ask: format!("Send all {}?", reports.len()),
    };
    match put(question) {
        Answer::Elsewhere(text) => return Ok(text),
        Answer::No => return crate::notice::Notice::new(crate::notice::Tone::NotNow, "Sent nothing").next("kebbie issue report <id> sends one, n there keeps it here.").ok(),
        Answer::Yes => {}
    }
    let read: Vec<(String, [u8; 32])> = reports.iter().map(|(id, report)| (id.clone(), report.digest())).collect();
    let say = |line: &str| println!("{line}");
    let pace = Pace { clock: &std::time::Instant::now, wait: &std::thread::sleep, say: &say };
    send_all_in(&state, &caller, &read, settings.url(), &scrub, &pace, crate::messages::now())?.notice()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_792_540_800;

    fn scrub() -> Scrub {
        Scrub::new(Some("/Users/ada".to_string()), vec!["shop".to_string(), "app".to_string(), "kebbie".to_string()])
    }

    #[test]
    fn a_report_names_no_home_repo_remote_or_token() {
        let text = "Ran kebbie push in /Users/ada/work/shop/wt2.shop.\n\
            origin is git@github.com:acme/shop.git, mirror https://gitlab.example.com/acme/app.git\n\
            The task billing@app said: token khm_Q2ZR8HWNx9a1b2c3d4 and GITHUB_TOKEN=ghp_abcdefghijklmnop1234\n\
            Authorization: Bearer abc.def.ghi; key eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9xyz123\n\
            Another user's /home/bob/notes and /Users/alice/.ssh stay out too.\n\
            Kept: commit 869bc49a1f3c2e0000000000000000000000abcd, the apply step, kebbie's own https://github.com/gronare/kebbie/issues/12, ~/.config/kebbie.";
        let redacted = redact(text, &scrub());
        for gone in ["/Users/ada", "shop", "acme", "gitlab.example.com", "billing@app", "khm_Q2ZR", "ghp_abcdef", "abc.def.ghi", "eyJhbGci", "/home/bob", "/Users/alice"] {
            assert!(!redacted.contains(gone), "{gone} is still in:\n{redacted}");
        }
        assert!(redacted.contains("in ~/work/<repo>/wt2.<repo>.") && redacted.contains("origin is <remote>, mirror <url>"), "{redacted}");
        assert!(redacted.contains("billing@<repo>") && redacted.contains("token <token>") && redacted.contains("GITHUB_TOKEN=<redacted>") && redacted.contains("Bearer <token>;"), "{redacted}");
        assert!(redacted.contains("~/notes and ~/.ssh"), "{redacted}");
        assert!(redacted.contains("the apply step") && redacted.contains("https://github.com/gronare/kebbie/issues/12") && redacted.contains("~/.config/kebbie."), "what is not private stays: {redacted}");
    }

    #[test]
    fn a_basic_authorization_and_a_secret_whose_value_is_the_next_word_are_redacted() {
        leaks("curl -H 'Authorization: Basic dXNlcjpodW50ZXIy' failed", &["dXNlcjpodW50ZXIy"], "Authorization: Basic <token>'");
        leaks("the config had password: hunter2 in it", &["hunter2"], "password: <token> in it");
        leaks("set API_KEY= s3cr3tvalue then ran it", &["s3cr3tvalue"], "API_KEY= <token> then");
        leaks("a basic test failed", &[], "a basic test failed");
    }

    fn leaks(text: &str, gone: &[&str], shown: &str) {
        let redacted = redact(text, &scrub());
        for leaked in gone {
            assert!(!redacted.contains(leaked), "{leaked} leaked: {redacted}");
        }
        assert!(redacted.contains(shown), "{shown} is not in: {redacted}");
    }

    #[test]
    fn an_email_address_is_redacted() {
        leaks("mail ada.lovelace@acme-corp.example about it", &["ada.lovelace", "acme-corp"], "mail <email> about it");
    }

    #[test]
    fn a_dotted_hostname_is_redacted() {
        leaks("db at db.prod.acme.internal and build.local, reading Cargo.toml", &["db.prod", "acme", "build.local"], "db at <host> and <host>, reading Cargo.toml");
    }

    #[test]
    fn an_ipv4_address_is_redacted_with_or_without_a_port() {
        leaks("reached 10.0.4.17 and 192.168.1.5:8470 on 0.26.0", &["10.0.4.17", "192.168.1.5"], "reached <ip> and <ip> on 0.26.0");
    }

    #[test]
    fn an_ipv6_address_is_redacted_bare_or_bracketed() {
        leaks("from 2001:db8:85a3::8a2e:370:7334 and [fe80::1]:8080", &["2001:db8", "fe80::1"], "from <ip> and <ip>");
    }

    #[test]
    fn a_branch_name_is_redacted() {
        leaks("on feature/refund-mails, behind origin/acme-main", &["refund-mails", "acme-main"], "on <branch>, behind <branch>");
    }

    #[test]
    fn a_scheme_less_repo_address_is_redacted_but_kebbies_own_is_kept() {
        leaks("cloned github.com/acme/billing, see github.com/gronare/kebbie/issues/3", &["acme", "billing"], "cloned <url>, see github.com/gronare/kebbie/issues/3");
    }

    #[test]
    fn a_repo_name_inside_a_dashed_or_underscored_name_is_redacted() {
        leaks("in shop-orders and shop_api", &["shop"], "in <repo>-orders and <repo>_api");
    }

    #[test]
    fn a_long_hex_string_is_redacted_unless_it_is_one_of_kebbies_commits() {
        let root = crate::sh::test_dir("report-kebbie-root");
        let git = |args: &[&str]| crate::sh::run("git", &[&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"], args].concat(), Some(&root)).unwrap();
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        let kebbies = git(&["rev-parse", "HEAD"]);
        let mut scrub = scrub();
        scrub.kebbie_root = Some(root);
        let foreign = "0123456789abcdef0123456789abcdef01234567";
        let redacted = redact(&format!("kebbie at {kebbies}, the project at {foreign}, a key 00112233445566778899aabbccddeeff"), &scrub);
        assert_eq!(redacted, format!("kebbie at {kebbies}, the project at <token>, a key <token>"));
    }

    #[test]
    fn an_aws_key_id_is_redacted() {
        leaks("used AKIAIOSFODNN7EXAMPLE then", &["AKIAIOSFODNN7EXAMPLE"], "used <token> then");
    }

    #[test]
    fn every_home_root_and_the_users_name_in_any_path_are_redacted() {
        leaks("in /var/home/bob/x, /export/home/eve/y and /srv/ada/work, not /srv/adam", &["bob", "eve", "/srv/ada/"], "in ~/x, ~/y and /srv/<user>/work, not /srv/adam");
    }

    #[test]
    fn terminal_escapes_and_control_characters_are_stripped_but_newlines_and_tabs_kept() {
        let redacted = redact("red \u{1b}[31mtext\u{1b}[0m \u{1b}]8;;https://evil.example\u{7}link\u{1b}]8;;\u{7} bell\u{7}\u{0}\u{9b}\u{202e}\nnext\tcell", &scrub());
        assert_eq!(redacted, "red text link bell\nnext\tcell");
    }

    fn bug(title: &str, body: &str) -> Issue {
        Issue { id: "a1b2c3".to_string(), title: title.to_string(), kind: "bug".to_string(), status: "open".to_string(), version: "0.26.0".to_string(), commit: "e58e84a".to_string(), filed: "2026-10-23".to_string(), by: "lead".to_string(), body: body.to_string(), ..Issue::default() }
    }

    #[test]
    fn the_payload_is_the_redacted_issue_with_its_version_and_os_and_nothing_else() {
        let report = payload(&bug("push in shop hid an error", "Saw it in /Users/ada/shop."), &scrub());
        assert_eq!(report, BugReport { title: "push in <repo> hid an error".to_string(), text: "Saw it in ~/<repo>.".to_string(), version: "0.26.0 (e58e84a)".to_string(), os: issues::os() });
        assert_eq!(preview(&report), format!("push in <repo> hid an error\nkebbie 0.26.0 (e58e84a) · {}\n\nSaw it in ~/<repo>.", issues::os()));
        let long = payload(&bug(&"t".repeat(300), &"å".repeat(MAX_TEXT_BYTES)), &scrub());
        assert!(long.check().is_ok() && long.text.ends_with("(shortened)") && long.title.chars().count() == MAX_TITLE_CHARS, "{} {}", long.text.len(), long.title.len());
    }

    #[test]
    fn only_an_open_bug_kebbie_filed_here_and_not_yet_decided_is_offered() {
        let state = crate::sh::test_dir("report-offers");
        let dir = issues::dir(&state);
        let wish = Issue { id: "bbbbbb".to_string(), kind: "wish".to_string(), ..bug("a wish", "") };
        let sent = Issue { id: "cccccc".to_string(), report: Some("4".to_string()), ..bug("sent", "") };
        let declined = Issue { id: "dddddd".to_string(), declined: Some("2026-10-23".to_string()), ..bug("declined", "") };
        let incoming = Issue { id: "eeeeee".to_string(), source: Some("report 9".to_string()), ..bug("from a report", "") };
        let closed = Issue { id: "ffffff".to_string(), status: "fixed".to_string(), ..bug("closed", "") };
        let on_github = Issue { id: "aaaaab".to_string(), github: Some(format!("{}/issues/7", issues::REPOSITORY)), ..bug("on GitHub", "") };
        let opened_for_github = Issue { id: "aaaaac".to_string(), sent: Some("2026-10-23".to_string()), ..bug("opened for GitHub", "") };
        for issue in [bug("offered", ""), wish, sent, declined, incoming, closed, on_github, opened_for_github] {
            issues::save(&dir, &issue).unwrap();
        }
        assert_eq!(offers(&state, true).iter().map(|issue| issue.title.as_str()).collect::<Vec<_>>(), ["offered"]);
        assert!(offers(&state, false).is_empty(), "[report] enabled = false offers nothing");
    }

    #[test]
    fn only_the_user_sends_or_declines_a_report() {
        let state = crate::sh::test_dir("report-who");
        issues::save(&issues::dir(&state), &bug("t", "")).unwrap();
        for caller in [Caller::Lead, Caller::Task("a@b".to_string()), Caller::Agent, Caller::Sandboxed, Caller::LeadSandboxed] {
            let read = payload(&bug("t", ""), &scrub()).digest();
            let refused = send_in(&state, &caller, "a1b2c3", "http://127.0.0.1:9", &scrub(), &read, NOW).unwrap_err();
            assert!(refused.contains("is the user's call"), "{refused}");
            assert!(decline_in(&state, &caller, "a1b2c3", NOW).is_err());
        }
        assert!(!key_file(&state).exists(), "a refused send makes no key and reaches no hub");
        assert_eq!(issues::load(&issues::dir(&state))[0], bug("t", ""));
        decline_in(&state, &Caller::User, "a1b2c3", NOW).unwrap();
        assert!(offers(&state, true).is_empty(), "a no takes it off the board");
        assert!(issues::show_in(&state, "a1b2c3").unwrap().contains("kebbie issue report a1b2c3 sends it"));
    }

    #[test]
    fn a_report_edited_after_it_was_read_is_not_sent() {
        let state = crate::sh::test_dir("report-edited");
        let dir = issues::dir(&state);
        issues::save(&dir, &bug("t", "what the user read")).unwrap();
        let read = payload(&issues::find(&dir, "a1b2c3").unwrap(), &scrub()).digest();
        issues::save(&dir, &bug("t", "what the lead wrote after, with ~/.ssh/id_ed25519")).unwrap();
        let refused = send_in(&state, &Caller::User, "a1b2c3", "http://127.0.0.1:9", &scrub(), &read, NOW).unwrap_err();
        assert!(refused.contains("changed since you read it"), "{refused}");
        assert!(!key_file(&state).exists() && issues::find(&dir, "a1b2c3").unwrap().report.is_none(), "refused before any key or network");
    }

    // A reports endpoint that hands out work-free challenges, takes each upload once, and answers 429 to the post numbered `busy`.
    type Seen = std::sync::Arc<std::sync::Mutex<(usize, usize, Vec<String>)>>;

    fn counting_endpoint(busy: usize) -> (String, Seen) {
        let seen = std::sync::Arc::new(std::sync::Mutex::new((0, 0, Vec::new())));
        let counted = seen.clone();
        let url = crate::hub::tests::serving_requests(move |method, path, body| {
            let mut seen = counted.lock().unwrap();
            let (status, answer) = match (method, path) {
                ("GET", "/v1/reports/challenge") => {
                    seen.0 += 1;
                    ("200 OK", serde_json::to_string(&ReportChallenge { nonce: Bytes32([7; 32]), difficulty: 0, expires: u64::MAX, tag: Bytes32([0; 32]) }).unwrap())
                }
                ("POST", "/v1/reports") => {
                    seen.1 += 1;
                    if seen.1 == busy {
                        ("429 Too Many Requests", r#"{"error":"rate-limited"}"#.to_string())
                    } else {
                        let upload: ReportUpload = serde_json::from_str(body).unwrap();
                        seen.2.push(upload.report.title);
                        ("202 Accepted", format!(r#"{{"report":{}}}"#, seen.2.len()))
                    }
                }
                _ => ("404 Not Found", r#"{"error":"not-found"}"#.to_string()),
            };
            format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len())
        });
        (url, seen)
    }

    #[test]
    fn all_bugs_go_once_each_paced_to_the_endpoints_limit_and_a_busy_answer_is_waited_out() {
        use std::cell::{Cell, RefCell};
        let state = crate::sh::test_dir("report-all");
        let dir = issues::dir(&state);
        let ids: Vec<String> = (0..8).map(|index| format!("b0000{index}")).collect();
        for (index, id) in ids.iter().enumerate() {
            issues::save(&dir, &Issue { id: id.clone(), title: format!("bug {index}"), ..bug("t", "it broke") }).unwrap();
        }
        issues::save(&dir, &Issue { id: "c00000".to_string(), report: Some("41".to_string()), ..bug("gone", "") }).unwrap();
        let mut read: Vec<(String, [u8; 32])> = ids.iter().map(|id| (id.clone(), payload(&issues::find(&dir, id).unwrap(), &scrub()).digest())).collect();
        read.push(("c00000".to_string(), [0; 32]));
        let (url, seen) = counting_endpoint(3);
        let start = std::time::Instant::now();
        let (elapsed, waits, sent_at) = (Cell::new(std::time::Duration::ZERO), RefCell::new(Vec::new()), RefCell::new(Vec::new()));
        let clock = || start + elapsed.get();
        let wait = |pause: std::time::Duration| {
            waits.borrow_mut().push(pause);
            elapsed.set(elapsed.get() + pause);
        };
        let say = |line: &str| if line.starts_with("Sent bug") { sent_at.borrow_mut().push(elapsed.get()) };
        let pace = Pace { clock: &clock, wait: &wait, say: &say };

        let batch = send_all_in(&state, &Caller::User, &read, &url, &scrub(), &pace, NOW).unwrap();
        assert_eq!(batch.sent.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(), ids, "{batch:?}");
        assert_eq!((batch.already, batch.failed), (vec![("c00000".to_string(), "41".to_string())], Vec::new()));
        let (challenges, posts, titles) = seen.lock().unwrap().clone();
        assert_eq!((challenges, posts), (9, 9), "one challenge and one post per bug, and one more of each for the try the 429 refused");
        assert_eq!(titles, (0..8).map(|index| format!("bug {index}")).collect::<Vec<_>>(), "each went once, in order");
        assert_eq!(*waits.borrow(), [WINDOW, WINDOW], "a whole window after the 429, and one more before a sixth within it");
        let sent_at = sent_at.borrow();
        assert!(sent_at.windows(BURST + 1).all(|run| run[BURST] - run[0] >= WINDOW), "never more than {BURST} in a window: {sent_at:?}");
        assert!(ids.iter().all(|id| issues::find(&dir, id).unwrap().report.is_some()));
        assert!(batch.sent.len() == 8 && Batch { sent: vec![("b00000".to_string(), "1".to_string())], ..Batch::default() }.notice().unwrap().contains("Sent 1 bug to kebbie's maintainers"));

        let again = send_all_in(&state, &Caller::User, &read, &url, &scrub(), &pace, NOW).unwrap();
        assert!(again.sent.is_empty() && again.failed.is_empty() && again.already.len() == 9, "{again:?}");
        assert_eq!(seen.lock().unwrap().1, 9, "nothing is sent twice");
        for caller in [Caller::Lead, Caller::Task("a@b".to_string()), Caller::Agent, Caller::Sandboxed] {
            assert!(send_all_in(&state, &caller, &read, &url, &scrub(), &pace, NOW).unwrap_err().contains("is the user's call"));
        }
    }

    #[test]
    fn a_bug_another_send_reported_while_the_batch_waited_is_not_posted_again() {
        let state = crate::sh::test_dir("report-all-raced");
        let dir = issues::dir(&state);
        let ids: Vec<String> = (0..=BURST).map(|index| format!("e0000{index}")).collect();
        for id in &ids {
            issues::save(&dir, &Issue { id: id.clone(), ..bug("t", "it broke") }).unwrap();
        }
        let read: Vec<(String, [u8; 32])> = ids.iter().map(|id| (id.clone(), payload(&issues::find(&dir, id).unwrap(), &scrub()).digest())).collect();
        let (url, seen) = counting_endpoint(0);
        let last = ids[BURST].clone();
        let wait = |_| issues::save(&dir, &Issue { id: last.clone(), report: Some("99".to_string()), ..bug("t", "it broke") }).unwrap();
        let pace = Pace { clock: &std::time::Instant::now, wait: &wait, say: &|_| {} };
        let batch = send_all_in(&state, &Caller::User, &read, &url, &scrub(), &pace, NOW).unwrap();
        assert_eq!(batch.sent.len(), BURST, "{batch:?}");
        assert_eq!(batch.already, [(last, "99".to_string())]);
        assert_eq!(seen.lock().unwrap().1, BURST, "the one sent elsewhere during the wait is not posted");
    }

    #[test]
    fn a_reopened_bug_is_offered_to_kebbies_maintainers_again() {
        let state = crate::sh::test_dir("report-reopened");
        let dir = issues::dir(&state);
        let read = || payload(&issues::find(&dir, "a1b2c3").unwrap(), &scrub()).digest();
        let (url, seen) = counting_endpoint(0);
        issues::save(&dir, &bug("t", "it broke")).unwrap();
        issues::save(&dir, &Issue { id: "dddddd".to_string(), declined: Some("2026-10-20".to_string()), ..bug("kept", "") }).unwrap();
        send_in(&state, &Caller::User, "a1b2c3", &url, &scrub(), &read(), NOW).unwrap();
        for id in ["a1b2c3", "dddddd"] {
            issues::close_in(&dir, &Caller::User, id, Some("fixed")).unwrap();
            issues::reopen_in(&state, &Caller::User, id, "3333333 2026-10-23").unwrap();
        }
        assert_eq!(offers(&state, true).iter().map(|issue| issue.id.as_str()).collect::<Vec<_>>(), ["a1b2c3", "dddddd"]);
        assert!(issues::find(&dir, "a1b2c3").unwrap().history.last().unwrap().ends_with("(3333333) (was report 1)"));
        let again = send_in(&state, &Caller::User, "a1b2c3", &url, &scrub(), &read(), NOW).unwrap();
        assert!(again.contains("as report 2"), "{again}");
        assert_eq!(seen.lock().unwrap().1, 2, "posted again after it came back");
    }

    #[test]
    fn kebbie_issue_report_says_why_a_bug_cannot_go_instead_of_asking() {
        let state = crate::sh::test_dir("report-cli-unsendable");
        let dir = issues::dir(&state);
        let wish = Issue { id: "bbbbbb".to_string(), kind: "wish".to_string(), ..bug("a wish", "") };
        let incoming = Issue { id: "eeeeee".to_string(), source: Some("report 9".to_string()), ..bug("from a report", "") };
        for issue in [bug("t", "it broke"), wish, incoming] {
            issues::save(&dir, &issue).unwrap();
        }
        record_sent(&state, "a1b2c3", "5").unwrap();
        let never = |_: Question| -> Answer { panic!("asked about a bug that cannot go") };
        for id in ["bbbbbb", "eeeeee"] {
            let refused = report_in(&state, &Caller::User, id, "http://127.0.0.1:9", &scrub(), &never, NOW).unwrap_err();
            assert!(refused.contains("only a bug you ran into goes"), "{refused}");
        }
        let shown = report_in(&state, &Caller::User, "a1b2c3", "http://127.0.0.1:9", &scrub(), &never, NOW).unwrap();
        assert!(shown.starts_with("t\nkebbie 0.26.0") && shown.ends_with("went to kebbie's maintainers already, as report 5."), "{shown}");
        assert!(issues::load(&dir).iter().all(|issue| issue.declined.is_none()), "nothing is written");
        let no = |_: Question| Answer::No;
        issues::save(&dir, &Issue { id: "cccccc".to_string(), ..bug("t", "") }).unwrap();
        report_in(&state, &Caller::User, "cccccc", "http://127.0.0.1:9", &scrub(), &no, NOW).unwrap();
        assert!(issues::find(&dir, "cccccc").unwrap().declined.is_some(), "a sendable bug is still asked about, and n keeps it here");
    }

    #[test]
    fn a_send_waits_for_another_one_on_this_machine_and_then_finds_the_bug_gone() {
        let state = crate::sh::test_dir("report-send-locked");
        let dir = issues::dir(&state);
        issues::save(&dir, &bug("t", "it broke")).unwrap();
        let read = payload(&issues::find(&dir, "a1b2c3").unwrap(), &scrub()).digest();
        let held = SendLock::take(&state).unwrap();
        let waiting = state.clone();
        let send = std::thread::spawn(move || send_in(&waiting, &Caller::User, "a1b2c3", "http://127.0.0.1:9", &scrub(), &read, NOW));
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!send.is_finished(), "the second send waits for the lock");
        issues::save(&dir, &Issue { report: Some("7".to_string()), ..bug("t", "it broke") }).unwrap();
        drop(held);
        let refused = send.join().unwrap().unwrap_err();
        assert!(refused.contains("went to kebbie's maintainers already, as report 7"), "{refused}");
    }

    #[test]
    fn a_report_the_hub_took_is_not_offered_or_sent_again_when_saving_the_issue_fails() {
        use std::os::unix::fs::PermissionsExt;
        let state = crate::sh::test_dir("report-save-failed");
        let dir = issues::dir(&state);
        issues::save(&dir, &bug("t", "it broke")).unwrap();
        let read = payload(&issues::find(&dir, "a1b2c3").unwrap(), &scrub()).digest();
        install_key(&state).unwrap();
        let (url, seen) = counting_endpoint(0);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let failed = send_in(&state, &Caller::User, "a1b2c3", &url, &scrub(), &read, NOW);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let failed = failed.unwrap_err();
        assert!(failed.contains("went to kebbie's maintainers as report 1, but saving the issue failed"), "{failed}");
        assert!(issues::find(&dir, "a1b2c3").unwrap().report.is_none(), "the issue file itself was not saved");
        assert!(offers(&state, true).is_empty(), "it is not offered again");
        let shown = issues::show_in(&state, "a1b2c3").unwrap();
        assert!(shown.contains("sent to kebbie's maintainers as report 1"), "kebbie issue show reads the send from the ledger: {shown}");
        let again = send_in(&state, &Caller::User, "a1b2c3", &url, &scrub(), &read, NOW).unwrap_err();
        assert!(again.contains("already, as report 1"), "{again}");
        let batch = send_all_in(&state, &Caller::User, &[("a1b2c3".to_string(), read)], &url, &scrub(), &Pace { clock: &std::time::Instant::now, wait: &|_| {}, say: &|_| {} }, NOW).unwrap();
        assert_eq!(batch.already, [("a1b2c3".to_string(), "1".to_string())]);
        assert_eq!(seen.lock().unwrap().1, 1, "posted once");
    }

    #[test]
    fn an_endpoint_that_stays_busy_fails_the_rest_with_why_after_three_waits() {
        let state = crate::sh::test_dir("report-all-busy");
        let dir = issues::dir(&state);
        for id in ["d00000", "d00001"] {
            issues::save(&dir, &Issue { id: id.to_string(), ..bug("t", "") }).unwrap();
        }
        let read: Vec<(String, [u8; 32])> = ["d00000", "d00001"].iter().map(|id| (id.to_string(), payload(&issues::find(&dir, id).unwrap(), &scrub()).digest())).collect();
        let url = crate::hub::tests::serving_requests(|method, _, _| {
            let (status, body) = if method == "GET" { ("200 OK", serde_json::to_string(&ReportChallenge { nonce: Bytes32([7; 32]), difficulty: 0, expires: u64::MAX, tag: Bytes32([0; 32]) }).unwrap()) } else { ("429 Too Many Requests", r#"{"error":"rate-limited"}"#.to_string()) };
            format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        });
        let waited = std::cell::Cell::new(0);
        let pace = Pace { clock: &std::time::Instant::now, wait: &|_| waited.set(waited.get() + 1), say: &|_| {} };
        let batch = send_all_in(&state, &Caller::User, &read, &url, &scrub(), &pace, NOW).unwrap();
        assert_eq!(waited.get(), WAITS);
        assert!(batch.sent.is_empty() && batch.failed.len() == 2 && batch.failed[0].1.contains("stayed busy after 3 waits") && batch.failed[1].1.contains("not tried"), "{batch:?}");
        let told = batch.notice().unwrap_err();
        assert!(told.contains("Sent none of 2 bugs") && told.contains("kebbie issue report --all sends the ones left"), "{told}");
        assert!(issues::find(&dir, "d00000").unwrap().report.is_none());
    }

    #[test]
    fn the_install_key_is_made_once_and_kept_private() {
        use std::os::unix::fs::PermissionsExt;
        let state = crate::sh::test_dir("report-key");
        let first = install_key(&state).unwrap();
        assert_eq!(std::fs::metadata(key_file(&state)).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(install_key(&state).unwrap().public(), first.public());
    }
}
