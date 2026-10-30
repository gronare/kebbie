mod act;
mod read;
mod watch;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::sh::Result;

pub use act::{addresses_line, fix_message};
pub use read::run as read_run;
pub use watch::{after_push, begin, tick};

pub const KIND: &str = "comment";
pub const EVERY: u64 = 60;
pub const DEFAULT_DAYS: u64 = 7;
const PRUNE_AFTER: u64 = 30 * 86_400;
const BODY_LIMIT: usize = 4_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Holds,
    Opinion,
    Praise,
    Question,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Holds => "holds",
            Kind::Opinion => "opinion",
            Kind::Praise => "praise",
            Kind::Question => "question",
        }
    }

    fn of_key(key: &str) -> Option<Kind> {
        [Kind::Holds, Kind::Opinion, Kind::Praise, Kind::Question].into_iter().find(|kind| kind.key() == key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Unread,
    Read,
    Failed,
    Reopened,
    Reply,
    Sent,
    Dismissed,
    Done,
}

impl Status {
    pub fn key(self) -> &'static str {
        match self {
            Status::Unread => "unread",
            Status::Read => "read",
            Status::Failed => "failed",
            Status::Reopened => "reopened",
            Status::Reply => "reply",
            Status::Sent => "sent",
            Status::Dismissed => "dismissed",
            Status::Done => "done",
        }
    }

    fn of_key(key: &str) -> Option<Status> {
        [Status::Unread, Status::Read, Status::Failed, Status::Reopened, Status::Reply, Status::Sent, Status::Dismissed, Status::Done].into_iter().find(|status| status.key() == key)
    }

    pub fn open(self) -> bool {
        !matches!(self, Status::Sent | Status::Dismissed | Status::Done)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    pub kind: Option<Kind>,
    pub reading: String,
    pub recommendation: String,
    pub proposal: String,
    pub test: String,
    pub subject: String,
    pub body: String,
    pub reply: String,
    pub question: String,
}

impl Reading {
    fn to_json(&self) -> Value {
        json!({ "kind": self.kind.map(Kind::key), "reading": self.reading, "recommendation": self.recommendation, "proposal": self.proposal, "test": self.test, "subject": self.subject, "body": self.body, "reply": self.reply, "question": self.question })
    }

    fn from_json(value: &Value) -> Reading {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
        Reading { kind: value["kind"].as_str().and_then(Kind::of_key), reading: text("reading"), recommendation: text("recommendation"), proposal: text("proposal"), test: text("test"), subject: text("subject"), body: text("body"), reply: text("reply"), question: text("question") }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub id: u64,
    pub url: String,
    pub author: String,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub commit: String,
    pub created_at: String,
    pub seen_at: u64,
    pub repo: PathBuf,
    pub slug: String,
    pub task: String,
    pub address: String,
    pub pushed_at: u64,
    pub hunk: String,
    pub status: Status,
    pub reading: Option<Reading>,
    pub error: Option<String>,
    pub answer: Option<String>,
    pub words: Option<String>,
    pub reply: Option<String>,
    pub fix: Option<String>,
    pub fixing: Option<String>,
    pub changed_at: u64,
}

impl Comment {
    pub fn short(&self) -> &str {
        &self.commit[..7.min(self.commit.len())]
    }

    pub fn kind(&self) -> Option<Kind> {
        self.reading.as_ref().and_then(|reading| reading.kind)
    }

    pub fn place(&self) -> String {
        match (&self.path, self.line) {
            (Some(path), Some(line)) => format!("{path}:{line}"),
            (Some(path), None) => path.clone(),
            _ => "the commit as a whole".to_string(),
        }
    }

    pub fn fix_short(&self) -> Option<&str> {
        self.fix.as_deref().map(|fix| &fix[..7.min(fix.len())])
    }

    // The reply the owner can copy: their own words, else the reading's draft with the fix's sha where it goes.
    pub fn reply_text(&self) -> String {
        if let Some(reply) = self.reply.as_ref().filter(|reply| !reply.trim().is_empty()) {
            return reply.clone();
        }
        let draft = self.reading.as_ref().map(|reading| reading.reply.trim().to_string()).unwrap_or_default();
        match self.fix_short() {
            Some(fix) if draft.contains("{fix}") => draft.replace("{fix}", fix),
            Some(fix) if draft.is_empty() => format!("Thanks, fixed in {fix}."),
            Some(fix) if !draft.contains(fix) => format!("{draft}\n\nFixed in {fix}."),
            _ => draft.replace("{fix}", "the fix"),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id, "url": self.url, "author": self.author, "body": self.body, "path": self.path, "line": self.line, "commit": self.commit,
            "created_at": self.created_at, "seen_at": self.seen_at, "repo": self.repo, "slug": self.slug, "task": self.task, "address": self.address,
            "pushed_at": self.pushed_at, "hunk": self.hunk, "status": self.status.key(), "reading": self.reading.as_ref().map(Reading::to_json),
            "error": self.error, "answer": self.answer, "words": self.words, "reply": self.reply, "fix": self.fix, "fixing": self.fixing, "changed_at": self.changed_at,
        })
    }

    pub fn from_json(value: &Value) -> Option<Comment> {
        let text = |key: &str| value[key].as_str().map(str::to_string);
        Some(Comment {
            id: value["id"].as_u64()?, url: text("url").unwrap_or_default(), author: text("author")?, body: text("body").unwrap_or_default(),
            path: text("path"), line: value["line"].as_u64(), commit: text("commit")?, created_at: text("created_at").unwrap_or_default(), seen_at: value["seen_at"].as_u64().unwrap_or(0),
            repo: PathBuf::from(text("repo")?), slug: text("slug").unwrap_or_default(), task: text("task").unwrap_or_default(), address: text("address").unwrap_or_default(),
            pushed_at: value["pushed_at"].as_u64().unwrap_or(0), hunk: text("hunk").unwrap_or_default(), status: Status::of_key(value["status"].as_str()?)?,
            reading: value.get("reading").filter(|reading| reading.is_object()).map(Reading::from_json),
            error: text("error"), answer: text("answer"), words: text("words"), reply: text("reply"), fix: text("fix"), fixing: text("fixing"), changed_at: value["changed_at"].as_u64().unwrap_or(0),
        })
    }
}

pub(crate) fn dir(state: &Path) -> PathBuf {
    state.join("comments")
}

fn file(state: &Path, id: u64) -> PathBuf {
    dir(state).join(format!("{id}.json"))
}

pub fn save(state: &Path, comment: &Comment) -> Result<()> {
    crate::state::write(&file(state, comment.id), serde_json::to_string_pretty(&comment.to_json()).unwrap_or_default())
}

pub fn load(state: &Path, id: u64) -> Option<Comment> {
    Comment::from_json(&serde_json::from_str(&fs::read_to_string(file(state, id)).ok()?).ok()?)
}

pub fn exists(state: &Path, id: u64) -> bool {
    file(state, id).exists()
}

// Every comment kebbie keeps, oldest first.
pub fn all(state: &Path) -> Vec<Comment> {
    let Ok(entries) = fs::read_dir(dir(state)) else { return Vec::new() };
    let mut comments: Vec<Comment> = entries.filter_map(|entry| {
        let name = entry.ok()?.file_name().to_string_lossy().to_string();
        load(state, name.strip_suffix(".json")?.parse().ok()?)
    }).collect();
    comments.sort_by_key(|comment| (comment.seen_at, comment.id));
    comments
}

// The cards the board shows: none goes on its own, only once sent, done or dismissed.
pub fn open(state: &Path) -> Vec<Comment> {
    all(state).into_iter().filter(|comment| comment.status.open()).collect()
}

pub fn board() -> Vec<Comment> {
    crate::lead::state_dir().map(|state| open(&state)).unwrap_or_default()
}

fn found(state: &Path, id: &str) -> Result<Comment> {
    id.trim().parse().ok().and_then(|id| load(state, id)).ok_or_else(|| format!("No comment {id}; kebbie comments lists them"))
}

pub fn update(state: &Path, id: u64, change: impl FnOnce(&mut Comment)) -> Result<Comment> {
    let mut comment = load(state, id).ok_or_else(|| format!("No comment {id}; kebbie comments lists them"))?;
    change(&mut comment);
    comment.changed_at = crate::messages::now();
    save(state, &comment)?;
    Ok(comment)
}

pub fn set_status(state: &Path, id: u64, status: Status) -> Result<Comment> {
    update(state, id, |comment| comment.status = status)
}

pub(crate) fn prune(state: &Path, now: u64) {
    for comment in all(state).into_iter().filter(|comment| !comment.status.open() && now.saturating_sub(comment.changed_at) > PRUNE_AFTER) {
        let _ = fs::remove_file(file(state, comment.id));
    }
}

// Another person's text: no control character, bidi override or zero-width character reaches a terminal or a prompt, and it stays short.
pub fn clean(text: &str, limit: usize) -> String {
    let hidden = |character: char| matches!(character, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}');
    let kept: String = text.chars().filter(|character| (*character == '\n' || *character == '\t' || !character.is_control()) && !hidden(*character)).collect();
    let kept = kept.replace('\t', "    ");
    if kept.chars().count() <= limit {
        return kept;
    }
    format!("{}…", kept.chars().take(limit).collect::<String>())
}

// A GitHub login, as it may go into a commit message.
pub fn login(text: &str) -> String {
    text.chars().filter(|character| character.is_ascii_alphanumeric() || *character == '-').take(39).collect()
}

pub fn row_word(comment: &Comment) -> String {
    match (comment.status, comment.kind()) {
        (Status::Unread, _) => "reading…".to_string(),
        (Status::Failed, _) => "not read · ⏎ reads it again".to_string(),
        (Status::Reopened, _) => format!("fixing in {}", comment.fixing.as_deref().unwrap_or(&comment.task).split('@').next().unwrap_or_default()),
        (Status::Reply, _) => "reply ready · y copy".to_string(),
        (_, Some(Kind::Holds)) => "holds · reopen with a fix".to_string(),
        (_, Some(Kind::Opinion)) => "opinion · reply, no change".to_string(),
        (_, Some(Kind::Praise)) => "praise · nothing to do".to_string(),
        (_, Some(Kind::Question)) => "unclear · answer its question".to_string(),
        (_, None) => "read".to_string(),
    }
}

pub fn list(state: &Path, as_json: bool) -> String {
    let comments = open(state);
    if as_json {
        return serde_json::to_string_pretty(&comments.iter().map(Comment::to_json).collect::<Vec<_>>()).unwrap_or_default();
    }
    if comments.is_empty() {
        return "No comments on pushed commits wait for you.".to_string();
    }
    comments.iter().map(|comment| format!("{}  {} on {} · {}  {}", comment.id, comment.author, comment.short(), comment.task, row_word(comment))).collect::<Vec<_>>().join("\n")
}

pub fn show(state: &Path, id: &str) -> Result<String> {
    let comment = found(state, id)?;
    let mut lines = vec![format!("{} on {} · {} · {}", comment.author, comment.short(), comment.place(), comment.task), comment.url.clone(), String::new()];
    lines.extend(comment.body.lines().map(|line| format!("> {line}")));
    lines.push(String::new());
    lines.push(format!("status: {}", row_word(&comment)));
    if let Some(reading) = &comment.reading {
        for (label, text) in [("reading", &reading.reading), ("recommendation", &reading.recommendation), ("proposal", &reading.proposal), ("test", &reading.test), ("question", &reading.question)] {
            if !text.is_empty() {
                lines.push(format!("{label}: {text}"));
            }
        }
        if reading.kind == Some(Kind::Holds) {
            lines.push(String::new());
            lines.push("the fix's commit message:".to_string());
            lines.extend(fix_message(&comment).lines().map(|line| format!("  {line}")));
        }
    }
    if let Some(error) = &comment.error {
        lines.push(format!("error: {error}"));
    }
    let reply = comment.reply_text();
    if !reply.is_empty() {
        lines.push(String::new());
        lines.push("reply to send:".to_string());
        lines.extend(reply.lines().map(|line| format!("  {line}")));
    }
    Ok(lines.join("\n"))
}

pub fn dismiss_cli(state: &Path, id: &str) -> Result<String> {
    let comment = found(state, id)?;
    set_status(state, comment.id, Status::Dismissed)?;
    Ok(format!("Dismissed {}'s comment on {}; nothing was posted.", comment.author, comment.short()))
}

pub fn sent_cli(state: &Path, id: &str) -> Result<String> {
    let comment = found(state, id)?;
    set_status(state, comment.id, Status::Sent)?;
    Ok(format!("Marked the reply to {}'s comment on {} sent.", comment.author, comment.short()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn comment(id: u64, kind: Option<Kind>, status: Status) -> Comment {
        let reading = kind.map(|kind| Reading {
            kind: Some(kind), reading: "The refund total ignores partial refunds.".to_string(), recommendation: "Fix it.".to_string(),
            proposal: "Sum the refunded amounts instead of counting refunds.".to_string(), test: "A test with two partial refunds.".to_string(),
            subject: "Count partial refunds in the refund total".to_string(), body: "The total counted refunds, not their amounts.".to_string(),
            reply: "Good catch, fixed in {fix}.".to_string(), question: if kind == Kind::Question { "Which total does the report mean?".to_string() } else { String::new() },
        });
        Comment {
            id, url: format!("https://github.com/acme/app/commit/3f2a9c1#commitcomment-{id}"), author: "johan".to_string(), body: "This misses partial refunds.".to_string(),
            path: Some("app/models/refund.rb".to_string()), line: Some(42), commit: "3f2a9c1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(), created_at: "2026-10-30T10:00:00Z".to_string(), seen_at: id,
            repo: PathBuf::from("/src/app"), slug: "acme/app".to_string(), task: "refunds-in-part".to_string(), address: "refunds-in-part@app".to_string(), pushed_at: 1,
            hunk: "@@ -40,3 +40,3 @@\n   def total\n-    refunds.count\n+    refunds.size\n".to_string(), status, reading, error: None, answer: None, words: None, reply: None, fix: None, fixing: None, changed_at: 1,
        }
    }

    #[test]
    fn a_comment_round_trips_through_its_state_file_and_the_board_shows_only_open_ones() {
        let state = crate::sh::test_dir("comments-file");
        let mut kept = comment(11, Some(Kind::Holds), Status::Read);
        kept.fix = Some("7c9e2f1bbbb".to_string());
        save(&state, &kept).unwrap();
        assert_eq!(load(&state, 11).as_ref(), Some(&kept));
        save(&state, &comment(12, None, Status::Unread)).unwrap();
        fs::write(file(&state, 13), "not json").unwrap();
        assert_eq!(all(&state).iter().map(|comment| comment.id).collect::<Vec<_>>(), [11, 12], "a broken record is skipped");
        set_status(&state, 11, Status::Sent).unwrap();
        set_status(&state, 12, Status::Dismissed).unwrap();
        assert!(open(&state).is_empty(), "sent and dismissed leave the board");
        assert_eq!(load(&state, 11).unwrap().status, Status::Sent, "and stay on record, surviving a restart");
        prune(&state, crate::messages::now());
        assert!(load(&state, 11).is_some(), "a fresh one is kept");
        prune(&state, crate::messages::now() + PRUNE_AFTER + 1);
        assert!(all(&state).is_empty(), "an old sent or dismissed one is pruned");
        save(&state, &comment(14, Some(Kind::Holds), Status::Reply)).unwrap();
        prune(&state, crate::messages::now() + PRUNE_AFTER * 10);
        assert_eq!(open(&state).len(), 1, "a card still open never goes on its own");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn another_persons_text_loses_every_escape_and_hidden_character_and_a_login_only_its_letters() {
        let text = "fine\u{1b}]8;;https://evil.example\u{7}click\u{1b}[31m red\u{202e}gnp.exe\u{200b}\r\n\tnext";
        let cleaned = clean(text, 1000);
        assert!(!cleaned.chars().any(|character| character.is_control() && character != '\n'), "{cleaned:?}");
        assert!(!cleaned.contains('\u{202e}') && !cleaned.contains('\u{200b}'), "{cleaned:?}");
        assert!(cleaned.contains("click") && cleaned.contains("\n    next"), "{cleaned:?}");
        assert_eq!(clean(&"x".repeat(10), 4), "xxxx…");
        assert_eq!(login("jo$han\n'; rm -rf /"), "johanrm-rf");
    }

    #[test]
    fn the_reply_names_the_fix_and_the_owners_own_words_win() {
        let mut reply = comment(1, Some(Kind::Holds), Status::Reply);
        reply.fix = Some("7c9e2f1deadbeef".to_string());
        assert_eq!(reply.reply_text(), "Good catch, fixed in 7c9e2f1.");
        reply.reading.as_mut().unwrap().reply = "Thanks!".to_string();
        assert_eq!(reply.reply_text(), "Thanks!\n\nFixed in 7c9e2f1.", "a draft that forgot the sha gets it");
        reply.reply = Some("My own reply".to_string());
        assert_eq!(reply.reply_text(), "My own reply");
        let mut opinion = comment(2, Some(Kind::Opinion), Status::Reply);
        opinion.reading.as_mut().unwrap().reply = "I'd keep it: the size is cached.".to_string();
        assert_eq!(opinion.reply_text(), "I'd keep it: the size is cached.");
    }

    #[test]
    fn dismiss_and_sent_from_the_command_line_keep_the_card_off_the_board() {
        let state = crate::sh::test_dir("comments-cli");
        save(&state, &comment(21, Some(Kind::Praise), Status::Read)).unwrap();
        save(&state, &comment(22, Some(Kind::Opinion), Status::Reply)).unwrap();
        assert!(list(&state, false).contains("21  johan on 3f2a9c1 · refunds-in-part  praise · nothing to do"), "{}", list(&state, false));
        assert!(show(&state, "22").unwrap().contains("reply to send:"));
        assert_eq!(dismiss_cli(&state, "21").unwrap(), "Dismissed johan's comment on 3f2a9c1; nothing was posted.");
        assert!(sent_cli(&state, "22").unwrap().starts_with("Marked the reply"));
        assert!(dismiss_cli(&state, "99").unwrap_err().contains("No comment 99"));
        assert_eq!(list(&state, false), "No comments on pushed commits wait for you.");
        assert_eq!(list(&state, true), "[]");
        fs::remove_dir_all(state).unwrap();
    }
}
