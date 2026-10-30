use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::sh::Result;
use super::{Comment, Status, clean, dir, exists, save, BODY_LIMIT};

pub const POLL: u64 = 12 * 60;
const MAX_BACKOFF: u64 = 6 * 60 * 60;
const OWNER_FOR: u64 = 86_400;
const HUNK_LINES: usize = 14;
#[cfg(not(test))]
const PER_PAGE: usize = 100;
#[cfg(test)]
const PER_PAGE: usize = 2;
const MAX_PAGES: u64 = 10;

#[derive(Clone, Debug, PartialEq)]
pub struct Watch {
    pub sha: String,
    pub address: String,
    pub task: String,
    pub repo: PathBuf,
    pub slug: String,
    pub pushed_at: u64,
    pub until: u64,
}

impl Watch {
    fn to_json(&self) -> Value {
        json!({ "sha": self.sha, "address": self.address, "task": self.task, "repo": self.repo, "slug": self.slug, "pushed_at": self.pushed_at, "until": self.until })
    }

    fn from_json(value: &Value) -> Option<Watch> {
        let text = |key: &str| value[key].as_str().map(str::to_string);
        Some(Watch {
            sha: text("sha")?, address: text("address")?, task: text("task").unwrap_or_default(), repo: PathBuf::from(text("repo")?), slug: text("slug")?,
            pushed_at: value["pushed_at"].as_u64()?, until: value["until"].as_u64()?,
        })
    }
}

// One per GitHub repo: its comments come oldest first, so the cursor is the last page read, where the next ones arrive.
#[derive(Clone, Debug, PartialEq)]
pub struct Cursor {
    pub slug: String,
    pub page: Option<u64>,
    pub checked_at: Option<u64>,
    pub failures: u32,
    pub error: Option<String>,
}

impl Cursor {
    fn to_json(&self) -> Value {
        json!({ "slug": self.slug, "page": self.page, "checked_at": self.checked_at, "failures": self.failures, "error": self.error })
    }

    fn from_json(value: &Value) -> Option<Cursor> {
        Some(Cursor { slug: value["slug"].as_str()?.to_string(), page: value["page"].as_u64(), checked_at: value["checked_at"].as_u64(), failures: value["failures"].as_u64().unwrap_or(0) as u32, error: value["error"].as_str().map(str::to_string) })
    }

    // Each failure doubles the wait, so a gh that keeps failing is asked a few times a day at most.
    fn due(&self, now: u64) -> bool {
        let wait = POLL.saturating_mul(1 << self.failures.min(8)).min(MAX_BACKOFF);
        self.checked_at.is_none_or(|at| now.saturating_sub(at) >= wait)
    }
}

fn cursor_file(state: &Path, slug: &str) -> PathBuf {
    dir(state).join("repos").join(format!("{}.json", slug.replace('/', "~")))
}

fn load_cursor(state: &Path, slug: &str) -> Cursor {
    fs::read_to_string(cursor_file(state, slug)).ok().and_then(|text| serde_json::from_str(&text).ok()).and_then(|value| Cursor::from_json(&value))
        .unwrap_or(Cursor { slug: slug.to_string(), page: None, checked_at: None, failures: 0, error: None })
}

fn save_cursor(state: &Path, cursor: &Cursor) {
    let _ = crate::state::write(&cursor_file(state, &cursor.slug), serde_json::to_string_pretty(&cursor.to_json()).unwrap_or_default());
}

fn watches_dir(state: &Path) -> PathBuf {
    dir(state).join("watch")
}

fn watch_file(state: &Path, sha: &str) -> PathBuf {
    watches_dir(state).join(format!("{sha}.json"))
}

fn save_watch(state: &Path, watch: &Watch) -> Result<()> {
    crate::state::write(&watch_file(state, &watch.sha), serde_json::to_string_pretty(&watch.to_json()).unwrap_or_default())
}

pub fn watches(state: &Path) -> Vec<Watch> {
    let Ok(entries) = fs::read_dir(watches_dir(state)) else { return Vec::new() };
    let mut watches: Vec<Watch> = entries.filter_map(|entry| {
        let path = entry.ok()?.path();
        Watch::from_json(&serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?)
    }).collect();
    watches.sort_by(|one, other| one.sha.cmp(&other.sha));
    watches
}

// Each commit kebbie pushed is watched on its own, under the task that pushed it, for comments.watch_days; kebbie doctor says why a repo is not watched.
pub fn begin(state: &Path, repo: &Path, address: &str, shas: &[String], now: u64) {
    let Ok(settings) = crate::config::resolve(Some(repo)).map(|loaded| loaded.settings) else { return };
    if !settings.comments_watch.value || shas.is_empty() {
        return;
    }
    let Ok(slug) = crate::ci::target(repo, &crate::ci::gh()) else { return };
    let task = address.split('@').next().unwrap_or(address).to_string();
    let until = now + settings.comments_days.value * 86_400;
    for sha in shas {
        let _ = save_watch(state, &Watch { sha: sha.clone(), address: address.to_string(), task: task.clone(), repo: repo.to_path_buf(), slug: slug.clone(), pushed_at: now, until });
    }
}

struct Lock {
    _file: fs::File,
}

impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
    }
}

impl Lock {
    fn take(state: &Path) -> Option<Lock> {
        let file = crate::state::append_to(&dir(state).join(".lock")).ok()?;
        // SAFETY: flock on a descriptor this File owns.
        (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(Lock { _file: file })
    }
}

pub fn tick(state: &Path) -> Vec<String> {
    tick_with(state, &crate::ci::gh(), crate::messages::now(), &crate::trusted_spawn::reviewer_env(std::env::vars()))
}

pub(super) fn tick_with(state: &Path, gh: &str, now: u64, env: &[(String, String)]) -> Vec<String> {
    let Some(_lock) = Lock::take(state) else { return Vec::new() };
    let mut notes = poll(state, gh, now);
    notes.extend(settle_fixes(state));
    notes.extend(super::read::tick(state, env));
    super::prune(state, now);
    notes
}

fn host_of(slug: &str) -> Option<&str> {
    (slug.matches('/').count() == 2).then(|| slug.split('/').next().unwrap_or_default())
}

fn api_path(slug: &str) -> &str {
    if host_of(slug).is_some() { slug.split_once('/').map_or(slug, |(_, rest)| rest) } else { slug }
}

fn gh_api(gh: &str, slug: &str, path: &str, headers: bool, repo: &Path) -> Result<String> {
    let mut args = vec!["api"];
    if let Some(host) = host_of(slug) {
        args.extend(["--hostname", host]);
    }
    if headers {
        args.push("--include");
    }
    args.push(path);
    crate::sh::run(gh, &args, Some(repo))
}

// The page the Link header names as last; a repo with one page or none has no Link header.
pub(super) fn last_page(headers: &str) -> u64 {
    let link = headers.lines().take_while(|line| !line.trim().is_empty()).find_map(|line| line.split_once(':').filter(|(name, _)| name.trim().eq_ignore_ascii_case("link")).map(|(_, value)| value.to_string()));
    link.and_then(|link| link.split(',').find(|part| part.contains("rel=\"last\"")).and_then(|part| {
        let url = part.split(';').next()?.trim().trim_matches(['<', '>']);
        url.split(['?', '&']).find_map(|pair| pair.strip_prefix("page=")?.parse().ok())
    })).unwrap_or(1)
}

fn owner_file(state: &Path) -> PathBuf {
    dir(state).join("owner.json")
}

// Who gh is logged in as, per host, asked once a day: their own comments are skipped.
fn owner(state: &Path, gh: &str, slug: &str, repo: &Path, now: u64) -> Result<String> {
    let host = host_of(slug).unwrap_or("github.com").to_string();
    let mut cached: Value = fs::read_to_string(owner_file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_else(|| json!({}));
    if let (Some(login), Some(at)) = (cached[&host]["login"].as_str(), cached[&host]["at"].as_u64()) && now.saturating_sub(at) < OWNER_FOR && !login.is_empty() {
        return Ok(login.to_string());
    }
    let mut args = vec!["api"];
    if host != "github.com" {
        args.extend(["--hostname", host.as_str()]);
    }
    args.extend(["user", "--jq", ".login"]);
    let login = crate::sh::run(gh, &args, Some(repo))?.trim().to_string();
    if login.is_empty() {
        return Err("gh named no login".to_string());
    }
    cached[&host] = json!({ "login": login, "at": now });
    let _ = crate::state::write(&owner_file(state), cached.to_string());
    Ok(login)
}

#[derive(Debug, PartialEq)]
pub(super) struct Found {
    pub id: u64,
    pub commit: String,
    pub url: String,
    pub author: String,
    pub bot: bool,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub created_at: String,
}

// gh --paginate prints one JSON array per page back to back.
pub(super) fn parse_comments(text: &str) -> Result<Vec<Found>> {
    let mut found = Vec::new();
    for page in serde_json::Deserializer::from_str(text).into_iter::<Value>() {
        let page = page.map_err(|error| format!("gh api gave no JSON: {error}"))?;
        let items = page.as_array().ok_or("gh api gave no list")?;
        found.extend(items.iter().filter_map(|item| {
            let text = |key: &str| item[key].as_str().map(str::to_string);
            Some(Found {
                id: item["id"].as_u64()?, commit: text("commit_id").unwrap_or_default(), url: text("html_url").unwrap_or_default(), author: item["user"]["login"].as_str()?.to_string(),
                bot: item["user"]["type"].as_str() == Some("Bot"), body: text("body").unwrap_or_default(), path: text("path").filter(|path| !path.is_empty()),
                line: item["line"].as_u64(), created_at: text("created_at").unwrap_or_default(),
            })
        }));
    }
    Ok(found)
}

// One round asks each repo with a watched commit for the comments since its cursor, once every 12 minutes, whatever number of its commits are watched.
fn poll(state: &Path, gh: &str, now: u64) -> Vec<String> {
    let mut live: Vec<Watch> = Vec::new();
    for watch in watches(state) {
        if now > watch.until {
            let _ = fs::remove_file(watch_file(state, &watch.sha));
        } else {
            live.push(watch);
        }
    }
    let mut slugs: Vec<String> = live.iter().map(|watch| watch.slug.clone()).collect();
    slugs.sort();
    slugs.dedup();
    let mut notes = Vec::new();
    for slug in slugs {
        let mut cursor = load_cursor(state, &slug);
        if !cursor.due(now) {
            continue;
        }
        let watched: Vec<&Watch> = live.iter().filter(|watch| watch.slug == slug).collect();
        let repo = watched[0].repo.clone();
        cursor.checked_at = Some(now);
        match owner(state, gh, &slug, &repo, now).and_then(|owner| read_pages(gh, &slug, &repo, cursor.page).map(|(page, found)| (owner, page, found))) {
            Ok((owner, page, found)) => {
                (cursor.page, cursor.failures, cursor.error) = (Some(page), 0, None);
                for item in found.into_iter().filter(|item| !item.bot && !item.author.eq_ignore_ascii_case(&owner) && !exists(state, item.id)) {
                    let Some(watch) = watched.iter().find(|watch| !item.commit.is_empty() && watch.sha.starts_with(&item.commit)) else { continue };
                    let comment = comment_of(watch, item, now);
                    if save(state, &comment).is_ok() {
                        notes.push(format!("comments: {} commented on {} ({}); reading it", comment.author, comment.short(), comment.task));
                    }
                }
            }
            Err(error) => {
                cursor.failures = cursor.failures.saturating_add(1);
                cursor.error = Some(error);
            }
        }
        save_cursor(state, &cursor);
    }
    notes
}

// From the cursor's page on until a page that is not full; a first round finds the last page from the Link header, so no history is paged through.
fn read_pages(gh: &str, slug: &str, repo: &Path, from: Option<u64>) -> Result<(u64, Vec<Found>)> {
    let base = format!("repos/{}/comments?per_page={PER_PAGE}", api_path(slug));
    let mut page = match from {
        Some(page) => page.max(1),
        None => last_page(&gh_api(gh, slug, &base, true, repo)?),
    };
    let mut found = Vec::new();
    for round in 0..MAX_PAGES {
        let items = parse_comments(&gh_api(gh, slug, &format!("{base}&page={page}"), false, repo)?)?;
        let full = items.len() >= PER_PAGE;
        // Deleted comments move the end of the list back past the cursor.
        if items.is_empty() && page > 1 && round == 0 {
            page -= 1;
            continue;
        }
        found.extend(items);
        if !full {
            break;
        }
        page += 1;
    }
    Ok((page, found))
}

fn comment_of(watch: &Watch, item: Found, now: u64) -> Comment {
    let path = item.path.map(|path| clean(&path, 300));
    let hunk = path.as_deref().map(|path| hunk(&watch.repo, &watch.sha, path, item.line)).unwrap_or_default();
    Comment {
        id: item.id, url: clean(&item.url, 300), author: super::login(&item.author), body: clean(&item.body, BODY_LIMIT), path, line: item.line, commit: watch.sha.clone(),
        created_at: clean(&item.created_at, 40), seen_at: now, repo: watch.repo.clone(), slug: watch.slug.clone(), task: watch.task.clone(), address: watch.address.clone(),
        pushed_at: watch.pushed_at, hunk, status: Status::Unread, reading: None, error: None, answer: None, words: None, reply: None, fix: None, fixing: None, changed_at: now,
    }
}

// The hunk of the commit's diff that holds the commented line, cut to a window around it.
pub(super) fn hunk(repo: &Path, sha: &str, path: &str, line: Option<u64>) -> String {
    let diff = crate::sh::run_untrimmed("git", &["show", "--format=", "--no-color", sha, "--", path], Some(repo)).unwrap_or_default();
    pick_hunk(&diff, line)
}

pub(super) fn pick_hunk(diff: &str, line: Option<u64>) -> String {
    let mut hunks: Vec<(u64, Vec<&str>)> = Vec::new();
    for text in diff.lines() {
        if let Some(header) = text.strip_prefix("@@ ") {
            let start = header.split_whitespace().find_map(|part| part.strip_prefix('+')).and_then(|part| part.split(',').next()?.parse().ok()).unwrap_or(0);
            hunks.push((start, vec![text]));
        } else if let Some((_, lines)) = hunks.last_mut() {
            lines.push(text);
        }
    }
    let Some(target) = line else { return hunks.first().map(|(_, lines)| lines.iter().take(HUNK_LINES).copied().collect::<Vec<_>>().join("\n")).unwrap_or_default() };
    let chosen = hunks.iter().rev().find(|(start, _)| *start <= target).or(hunks.first());
    let Some((start, lines)) = chosen else { return String::new() };
    let mut at = *start;
    let mut index_of_line = 1;
    for (index, text) in lines.iter().enumerate().skip(1) {
        if at >= target && !text.starts_with('-') {
            index_of_line = index;
            break;
        }
        if !text.starts_with('-') {
            at += 1;
        }
        index_of_line = index;
    }
    let from = index_of_line.saturating_sub(HUNK_LINES / 2).max(1);
    let mut shown = vec![lines[0]];
    shown.extend(lines.iter().skip(from).take(HUNK_LINES - 1));
    shown.join("\n")
}

// A fix is pushed once a commit on the default branch carries the comment's Addresses line.
fn settle_fixes(state: &Path) -> Vec<String> {
    let mut notes = Vec::new();
    for comment in super::open(state).into_iter().filter(|comment| comment.status == Status::Reopened) {
        let Some(fix) = fixed_by(&comment) else { continue };
        if super::update(state, comment.id, |comment| {
            comment.fix = Some(fix.clone());
            comment.status = Status::Reply;
        }).is_ok() {
            notes.push(format!("comments: the fix for {}'s comment on {} is pushed as {}; its reply is ready", comment.author, comment.short(), &fix[..7.min(fix.len())]));
        }
    }
    notes
}

fn fixed_by(comment: &Comment) -> Option<String> {
    let upstream = crate::repo::default_branch(&comment.repo).ok()?;
    let grep = format!("--grep={}", super::addresses_line(comment));
    let found = crate::repo::git(&comment.repo, &["log", "--fixed-strings", &grep, "--format=%H", "-n", "1", &upstream]).ok()?;
    Some(found.trim().to_string()).filter(|sha| !sha.is_empty())
}

// Called right after a push, so the card turns to its reply without waiting for the next round.
pub fn after_push(state: &Path) {
    if let Some(_lock) = Lock::take(state) {
        settle_fixes(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::{Kind, load, open};

    const LINK: &str = "HTTP/2.0 200 OK\nLink: <https://api.github.com/repositories/1/comments?per_page=2&page=2>; rel=\"next\", <https://api.github.com/repositories/1/comments?per_page=2&page=3>; rel=\"last\"\n\n[]";
    const PAGE_3: &str = r#"[{"id":101,"commit_id":"abcdef0123","html_url":"https://github.com/acme/app/commit/abc#commitcomment-101","user":{"login":"johan","type":"User"},"body":"This misses partial refunds.","path":"app/models/refund.rb","line":42,"created_at":"2026-10-30T10:00:00Z"},{"id":102,"commit_id":"abcdef0123","user":{"login":"Carl","type":"User"},"body":"my own note"}]"#;
    const PAGE_4: &str = r#"[{"id":103,"commit_id":"abcdef0123","user":{"login":"ci-bot","type":"Bot"},"body":"coverage 90%"},{"id":104,"commit_id":"abcdef0123","user":{"login":"anna","type":"User"},"body":"nice\u001b]8;;x\u0007"}]"#;
    const PAGE_5: &str = r#"[{"id":105,"commit_id":"9999999999","user":{"login":"olle","type":"User"},"body":"on a commit kebbie did not push"}]"#;

    fn fake_gh(state: &Path) -> String {
        let gh = state.join("gh");
        let log = state.join("gh.log");
        let answer = |text: &str| format!("cat <<'JSON'\n{text}\nJSON\n");
        crate::executable::write(&gh, format!("#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in\n*\" user \"*) echo carl ;;\n*--include*) {};;\n*page=3*) {};;\n*page=4*) {};;\n*page=5*) {};;\n*) echo '[]' ;;\nesac\n", log.display(), answer(LINK), answer(PAGE_3), answer(PAGE_4), answer(PAGE_5))).unwrap();
        gh.to_string_lossy().to_string()
    }

    fn watched(state: &Path, sha: &str, until: u64) -> Watch {
        let watch = Watch { sha: sha.to_string(), address: "refunds-in-part@app".to_string(), task: "refunds-in-part".to_string(), repo: state.to_path_buf(), slug: "acme/app".to_string(), pushed_at: 10, until };
        save_watch(state, &watch).unwrap();
        watch
    }

    fn calls(state: &Path) -> Vec<String> {
        fs::read_to_string(state.join("gh.log")).unwrap_or_default().lines().filter(|line| line.contains("/comments")).map(str::to_string).collect()
    }

    #[test]
    fn a_round_asks_each_repo_once_from_its_cursor_and_keeps_only_new_comments_on_watched_commits() {
        let state = crate::sh::test_dir("comments-poll");
        let gh = fake_gh(&state);
        watched(&state, "abcdef0123", 10_000);
        watched(&state, "bbbbbbb111", 10_000);
        let notes = tick_with(&state, &gh, 100, &[]);
        assert_eq!(notes.iter().filter(|note| note.contains("commented on")).count(), 2, "{notes:?}");
        assert!(notes[0].starts_with("comments: johan commented on abcdef0 (refunds-in-part)"), "{notes:?}");
        let saved: Vec<u64> = open(&state).iter().map(|comment| comment.id).collect();
        assert_eq!(saved, [101, 104], "the owner's own comment (any case), the bot's and one on a commit nobody watches are skipped");
        assert_eq!(calls(&state), ["api --include repos/acme/app/comments?per_page=2", "api repos/acme/app/comments?per_page=2&page=3", "api repos/acme/app/comments?per_page=2&page=4", "api repos/acme/app/comments?per_page=2&page=5"], "one repo, two watched commits: the Link header finds the last page, a full page goes on to the next, and no history is paged through");
        assert_eq!(load_cursor(&state, "acme/app").page, Some(5));
        let johan = load(&state, 101).unwrap();
        assert_eq!((johan.path.as_deref(), johan.line, johan.commit.as_str()), (Some("app/models/refund.rb"), Some(42), "abcdef0123"));
        assert!(johan.status == Status::Failed && johan.error.as_deref().is_some_and(|error| error.contains("no claude")), "without a claude to read with, the card says so: {johan:?}");
        assert!(!load(&state, 104).unwrap().body.contains('\u{1b}'), "the body is stored without escapes");
        crate::comments::set_status(&state, 101, Status::Dismissed).unwrap();
        tick_with(&state, &gh, 101, &[]);
        assert_eq!(calls(&state).len(), 4, "a repo checked a moment ago waits");
        let again = tick_with(&state, &gh, 100 + POLL, &[]);
        assert!(!again.iter().any(|note| note.contains("commented on")), "a comment seen once, even dismissed, is never new again: {again:?}");
        assert_eq!(&calls(&state)[4..], ["api repos/acme/app/comments?per_page=2&page=5"], "the next round reads only from the cursor's page");
        assert_eq!(fs::read_to_string(state.join("gh.log")).unwrap().matches(" user ").count(), 1, "who the owner is is asked once a day");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_last_page_comes_from_the_link_header_and_one_page_has_none() {
        assert_eq!(last_page(LINK), 3);
        assert_eq!(last_page("HTTP/2.0 200 OK\nContent-Type: application/json\n\n[{\"link\":\"x?page=9 rel=\\\"last\\\"\"}]"), 1, "only the headers count");
    }

    #[test]
    fn a_failing_gh_backs_off_and_a_watch_ends_after_its_days() {
        let state = crate::sh::test_dir("comments-backoff");
        watched(&state, "abcdef0123", 10_000);
        tick_with(&state, "false", 100, &[]);
        let failed = load_cursor(&state, "acme/app");
        assert_eq!((failed.failures, failed.error.is_some()), (1, true));
        assert!(!failed.due(100 + POLL) && failed.due(100 + 2 * POLL), "one failure doubles the wait");
        let worst = Cursor { failures: 30, ..failed.clone() };
        assert!(worst.due(100 + MAX_BACKOFF), "never longer than the ceiling");
        tick_with(&state, "false", 10_001, &[]);
        assert!(watches(&state).is_empty(), "past comments.watch_days the watch goes");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_ghe_slug_asks_its_own_host() {
        assert_eq!((host_of("git.acme.dev/acme/app"), api_path("git.acme.dev/acme/app")), (Some("git.acme.dev"), "acme/app"));
        assert_eq!((host_of("acme/app"), api_path("acme/app")), (None, "acme/app"));
        assert!(parse_comments("gh: not found").is_err());
    }

    #[test]
    fn the_hunk_shown_is_the_one_holding_the_commented_line() {
        let diff = "diff --git a/x b/x\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n@@ -40,4 +40,4 @@ def total\n   def total\n-    refunds.count\n+    refunds.sum(:amount)\n   end\n";
        let shown = pick_hunk(diff, Some(41));
        assert!(shown.starts_with("@@ -40,4 +40,4 @@") && shown.contains("+    refunds.sum(:amount)") && !shown.contains("+B"), "{shown}");
        assert!(pick_hunk(diff, None).starts_with("@@ -1,3 +1,3 @@"));
        assert_eq!(pick_hunk("", Some(3)), "");
    }

    #[test]
    fn a_reopened_comment_turns_to_its_reply_once_a_commit_on_main_carries_its_addresses_line() {
        let (dir, origin, slot, _) = crate::tasks::test_support::task_with_a_commit("refunds-in-part");
        let state = crate::sh::test_dir("comments-fixed");
        let mut comment = crate::comments::tests::comment(201, Some(Kind::Holds), Status::Reopened);
        comment.repo = dir.clone();
        save(&state, &comment).unwrap();
        settle_fixes(&state);
        assert_eq!(load(&state, 201).unwrap().status, Status::Reopened, "nothing on main addresses it yet");
        let message = format!("Count partial refunds\n\n{}", crate::comments::addresses_line(&comment));
        crate::tasks::test_support::upstream_commit(&dir, "e", "e", &message);
        let fix = crate::repo::git(&dir, &["rev-parse", "HEAD"]).unwrap();
        after_push(&state);
        let ready = load(&state, 201).unwrap();
        assert_eq!((ready.status, ready.fix.as_deref()), (Status::Reply, Some(fix.as_str())));
        assert_eq!(ready.reply_text(), format!("Good catch, fixed in {}.", &fix[..7]));
        crate::tasks::test_support::cleanup(dir, origin, &slot);
        fs::remove_dir_all(state).unwrap();
    }
}
