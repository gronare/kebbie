use std::io::Write;
use std::path::{Path, PathBuf};

use kebbie_hub_api::{Bytes32, Bytes64, ReportAccepted, ReportChallenge, ReportUpload};
use kebbie_hub_protocol::{BugReport, InstallKey, MAX_TEXT_BYTES, MAX_TITLE_CHARS, solve};
use serde::{Deserialize, Serialize};

use super::http::{CALL, Client, HubError};
use crate::issues::{self, Issue};
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

// The user's name as a segment of any path, such as /srv/carl/app.
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

fn redact_words(text: &str, scrub: &Scrub) -> String {
    let mut out = String::new();
    let mut bearer = false;
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String, bearer: &mut bool| {
        if word.is_empty() {
            return;
        }
        let start = word.find(|character: char| !matches!(character, '"' | '\'' | '(' | '[' | '<' | '{' | '`')).unwrap_or(word.len());
        let end = word.rfind(|character: char| !matches!(character, '"' | '\'' | ')' | ']' | '>' | '}' | ',' | '.' | ';' | '!' | '?' | '`')).map_or(start, |end| end + word[end..].chars().next().map_or(1, char::len_utf8));
        let start = if start > 0 && word[..start].ends_with('[') && ip(&word[start - 1..end.max(start)]) { start - 1 } else { start };
        let core = &word[start..end.max(start)];
        let replaced = redact_word(core, *bearer, scrub);
        *bearer = core.eq_ignore_ascii_case("bearer") || core.eq_ignore_ascii_case("token");
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
    issue.is_open() && issue.kind == "bug" && issue.report.is_none() && issue.declined.is_none() && issue.source.is_none() && issue.github.is_none() && issue.sent.is_none()
}

pub fn offers(state: &Path, enabled: bool) -> Vec<Issue> {
    if !enabled {
        return Vec::new();
    }
    issues::load(&issues::dir(state)).into_iter().filter(offered).collect()
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
                return Err(crate::signals::internal("report.install-key", kebbie_hub_protocol::SignalKind::CorruptState, format!("{}: the public key does not match the secret one; move the file away and kebbie makes a new one", file.display())));
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

fn refused(error: HubError) -> String {
    match error.status() {
        Some(429) => "the reports endpoint is busy; try again in a few minutes".to_string(),
        _ => error.to_string(),
    }
}

pub const UNASKED_EVERY: u64 = 60;

fn deliver(state: &Path, issue: &mut Issue, report: BugReport, url: &str, now: u64) -> Result<String> {
    let key = install_key(state)?;
    let client = Client::new(&super::hub_url(url)?, None, CALL);
    let challenge: ReportChallenge = client.get("/v1/reports/challenge").map_err(refused)?;
    if challenge.difficulty > MAX_WORK {
        return Err(format!("the reports endpoint asks for {} bits of work, more than kebbie does ({MAX_WORK}); nothing was sent", challenge.difficulty));
    }
    let counter = solve(&challenge.nonce.0, &report.digest(), challenge.difficulty);
    let title = report.title.clone();
    let upload = ReportUpload { signature: Bytes64(key.sign(&challenge.nonce.0, &report)), key: Bytes32(key.public()), report, challenge, counter };
    let accepted: ReportAccepted = client.post("/v1/reports", &upload).map_err(refused)?;
    let id = accepted.report.to_string();
    issue.report = Some(id.clone());
    issue.sent = Some(issues::today(now));
    issues::save(&issues::dir(state), issue)?;
    let _ = crate::messages::log_for_user_in(state, &sent_line(&id, &issue.id, &title), SENT_KIND);
    Ok(id)
}

pub const SENT_KIND: &str = "report-sent";

pub fn sent_line(report: &str, issue: &str, title: &str) -> String {
    format!("Sent bug {issue} to kebbie's maintainers as report {report}: \"{title}\", redacted; kebbie issue report {issue} shows the text that went.")
}

// The digest is of the report the user read; anything else that would go out instead is refused.
pub fn send_in(state: &Path, caller: &Caller, id: &str, url: &str, scrub: &Scrub, read: &[u8; 32], now: u64) -> Result<String> {
    crate::permit::user_only_as(caller, "Sending a report to kebbie's maintainers")?;
    let dir = issues::dir(state);
    let mut issue = issues::find(&dir, id)?;
    if let Some(report) = &issue.report {
        return Err(format!("Bug {} went to kebbie's maintainers already, as report {report}", issue.id));
    }
    if issue.kind != "bug" || issue.source.is_some() {
        return Err(format!("{} is a wish or another install's report; only a bug you ran into goes to kebbie's maintainers", issue.id));
    }
    let report = payload(&issue, scrub);
    if report.digest() != *read {
        return Err(format!("Bug {} changed since you read it, so nothing was sent; read it again with kebbie issue report {0}", issue.id));
    }
    let sent = deliver(state, &mut issue, report, url, now)?;
    crate::notice::Notice::new(crate::notice::Tone::Done, format!("Sent bug {} to kebbie's maintainers as report {sent}", issue.id)).next("It stays on this machine too; after each update the keeper rechecks it and closes it once a commit plainly fixes it.").ok()
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
    for mut issue in waiting {
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

fn ask_send() -> bool {
    print!("Send this report to kebbie's maintainers? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes")
}

pub fn cli(id: &str) -> Result<String> {
    let caller = crate::permit::caller();
    crate::permit::user_only_as(&caller, "Sending a report to kebbie's maintainers")?;
    let state = crate::lead::state_dir()?;
    let settings = crate::config::report_settings()?;
    let scrub = Scrub::current();
    let issue = issues::find(&issues::dir(&state), id)?;
    let read = payload(&issue, &scrub);
    let shown = preview(&read);
    if !super::interactive() {
        return Ok(format!("{shown}\n\nThis is the report as it would go to kebbie's maintainers at {}. Run kebbie issue report {} in your own terminal to answer y or n.", settings.url(), issue.id));
    }
    println!("{shown}\n\nThis goes to kebbie's maintainers at {}, signed with this install's key.", settings.url());
    if ask_send() {
        send_in(&state, &caller, id, settings.url(), &scrub, &read.digest(), crate::messages::now())
    } else {
        decline_in(&state, &caller, id, crate::messages::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_792_540_800;

    fn scrub() -> Scrub {
        Scrub::new(Some("/Users/carl".to_string()), vec!["greenhouse".to_string(), "app".to_string(), "kebbie".to_string()])
    }

    #[test]
    fn a_report_names_no_home_repo_remote_or_token() {
        let text = "Ran kebbie push in /Users/carl/work/greenhouse/wt2.greenhouse.\n\
            origin is git@github.com:acme/greenhouse.git, mirror https://gitlab.example.com/acme/app.git\n\
            The task billing@app said: token khm_Q2ZR8HWNx9a1b2c3d4 and GITHUB_TOKEN=ghp_abcdefghijklmnop1234\n\
            Authorization: Bearer abc.def.ghi; key eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9xyz123\n\
            Another user's /home/bob/notes and /Users/alice/.ssh stay out too.\n\
            Kept: commit 869bc49a1f3c2e0000000000000000000000abcd, the apply step, kebbie's own https://github.com/gronare/kebbie/issues/12, ~/.config/kebbie.";
        let redacted = redact(text, &scrub());
        for gone in ["/Users/carl", "greenhouse", "acme", "gitlab.example.com", "billing@app", "khm_Q2ZR", "ghp_abcdef", "abc.def.ghi", "eyJhbGci", "/home/bob", "/Users/alice"] {
            assert!(!redacted.contains(gone), "{gone} is still in:\n{redacted}");
        }
        assert!(redacted.contains("in ~/work/<repo>/wt2.<repo>.") && redacted.contains("origin is <remote>, mirror <url>"), "{redacted}");
        assert!(redacted.contains("billing@<repo>") && redacted.contains("token <token>") && redacted.contains("GITHUB_TOKEN=<redacted>") && redacted.contains("Bearer <token>;"), "{redacted}");
        assert!(redacted.contains("~/notes and ~/.ssh"), "{redacted}");
        assert!(redacted.contains("the apply step") && redacted.contains("https://github.com/gronare/kebbie/issues/12") && redacted.contains("~/.config/kebbie."), "what is not private stays: {redacted}");
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
        leaks("mail carl.green@acme-corp.se about it", &["carl.green", "acme-corp"], "mail <email> about it");
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
        leaks("in greenhouse-bokning and greenhouse_api", &["greenhouse"], "in <repo>-bokning and <repo>_api");
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
        leaks("in /var/home/bob/x, /export/home/eve/y and /srv/carl/work, not /srv/carlos", &["bob", "eve", "/srv/carl/"], "in ~/x, ~/y and /srv/<user>/work, not /srv/carlos");
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
        let report = payload(&bug("push in greenhouse hid an error", "Saw it in /Users/carl/greenhouse."), &scrub());
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
        assert!(issues::show_in(&issues::dir(&state), "a1b2c3").unwrap().contains("kebbie issue report a1b2c3 sends it"));
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

    #[test]
    fn the_install_key_is_made_once_and_kept_private() {
        use std::os::unix::fs::PermissionsExt;
        let state = crate::sh::test_dir("report-key");
        let first = install_key(&state).unwrap();
        assert_eq!(std::fs::metadata(key_file(&state)).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(install_key(&state).unwrap().public(), first.public());
    }
}
