use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::sh::Result;

// The vault as notes-vault-mcp sees it: kelpie reads the server's local SQLite index, read-only, so a vault on S3 needs no credentials here.
pub struct Index {
    db: Connection,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub key: String,
    pub stem: String,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub kind: String,
    pub priority: String,
    pub paths: Vec<String>,
    pub updated: String,
    pub area: String,
}

fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_default()
}

pub fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(path),
    }
}

fn setting(name: &str, options: &Value) -> Option<String> {
    [name.to_string(), format!("CLAUDE_PLUGIN_OPTION_{name}")].iter().find_map(|key| std::env::var(key).ok())
        .or_else(|| options[name.to_lowercase()].as_str().map(str::to_string))
        .filter(|value| !value.is_empty())
}

// The vault plugin's options in Claude Code's settings (pluginConfigs."vault@<marketplace>"), as the server gets them.
fn plugin_options() -> Value {
    let dir = std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|_| home().join(".claude"));
    let settings: Value = std::fs::read_to_string(dir.join("settings.json")).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    settings["pluginConfigs"].as_object().into_iter().flatten()
        .find(|(name, _)| name.starts_with("vault@")).map(|(_, config)| config["options"].clone()).unwrap_or_default()
}

// notes-vault-mcp names its index after the vault: sha1 of the local root, or of endpoint + bucket + prefix.
pub fn index_name(options: &Value) -> Option<String> {
    let seed = match setting("VAULT_PATH", options) {
        Some(root) => expand(&root).canonicalize().unwrap_or_else(|_| expand(&root)).to_string_lossy().to_string(),
        None => setting("S3_ENDPOINT", options)? + &setting("S3_BUCKET", options)? + &setting("S3_PREFIX", options).unwrap_or_default(),
    };
    Some(format!("{}.sqlite", &sha1_smol::Sha1::from(seed).digest().to_string()[..12]))
}

pub fn index_path() -> Result<PathBuf> {
    let options = plugin_options();
    let cache = setting("VAULT_CACHE_DIR", &options).map(|dir| expand(&dir)).unwrap_or_else(|| home().join(".cache/notes-vault-mcp"));
    let name = index_name(&options).ok_or("No vault configured: set the vault plugin's options in Claude Code, or VAULT_PATH")?;
    let path = cache.join(name);
    if path.exists() { Ok(path) } else { Err(format!("No vault index at {}; a Claude session with the vault plugin builds it", path.display())) }
}

// The frontmatter's area link, [[stem]], as the stem the backlog names an area by.
pub fn area_name(link: &str) -> String {
    link.trim().trim_start_matches("[[").trim_end_matches("]]").to_string()
}

fn text(row: &rusqlite::Row, index: usize) -> String {
    row.get::<_, Option<String>>(index).ok().flatten().unwrap_or_default()
}

const NOTE_COLUMNS: &str = "key, stem, title, summary, status, kind, priority, paths, updated, area";

fn note(row: &rusqlite::Row) -> rusqlite::Result<Note> {
    Ok(Note {
        key: text(row, 0),
        stem: text(row, 1),
        title: text(row, 2),
        summary: text(row, 3),
        status: text(row, 4),
        kind: text(row, 5),
        priority: text(row, 6),
        paths: serde_json::from_str(&text(row, 7)).unwrap_or_default(),
        updated: text(row, 8),
        area: area_name(&text(row, 9)),
    })
}

impl Index {
    pub fn open() -> Result<Self> {
        Self::at(&index_path()?)
    }

    pub fn at(path: &Path) -> Result<Self> {
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .map_err(|error| format!("Opening the vault index {}: {error}", path.display()))?;
        let _ = db.busy_timeout(std::time::Duration::from_secs(2));
        Ok(Self { db })
    }

    fn notes(&self, filter: &str, value: &str) -> Result<Vec<Note>> {
        let sql = format!("SELECT {NOTE_COLUMNS} FROM notes WHERE {filter} ORDER BY key");
        let mut statement = self.db.prepare(&sql).map_err(|error| error.to_string())?;
        let rows = statement.query_map([value], note).map_err(|error| error.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())
    }

    // Every live backlog note: key, title, summary, priority, area link and date.
    pub fn backlog(&self) -> Result<Vec<[String; 6]>> {
        let sql = "SELECT key, title, summary, priority, area, date FROM notes WHERE status = 'backlog' AND valid = 1 AND folder NOT IN ('Archive', 'Log')";
        let mut statement = self.db.prepare(sql).map_err(|error| error.to_string())?;
        let rows = statement.query_map([], |row| Ok([0, 1, 2, 3, 4, 5].map(|column| text(row, column)))).map_err(|error| error.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())
    }

    pub fn ongoing(&self) -> Result<Vec<Note>> {
        let sql = format!("SELECT {NOTE_COLUMNS} FROM notes WHERE folder = 'Projects' AND status IN ('active', 'draft') AND valid = 1 ORDER BY mtime DESC, key");
        let mut statement = self.db.prepare(&sql).map_err(|error| error.to_string())?;
        let rows = statement.query_map([], note).map_err(|error| error.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())
    }

    pub fn body(&self, key: &str) -> Option<String> {
        self.db.query_row("SELECT body FROM notes_fts WHERE key = ?1", [key], |row| row.get::<_, Option<String>>(0)).ok().flatten()
    }

    pub fn changed_at(&self, key: &str) -> Option<f64> {
        self.db.query_row("SELECT mtime FROM notes WHERE key = ?1", [key], |row| row.get::<_, f64>(0)).ok()
    }

    pub fn status_of(&self, key: &str) -> Option<String> {
        self.db.query_row("SELECT status FROM notes WHERE key = ?1", [key], |row| row.get::<_, String>(0)).ok()
    }

    // The system notes, one per app or subsystem.
    pub fn areas(&self) -> Result<Vec<Note>> {
        self.notes("folder = ?1 AND kind = 'system' AND valid = 1", "Areas")
    }

    // Live notes that name the area, outside the archive and the log.
    pub fn notes_of(&self, stem: &str) -> Result<Vec<Note>> {
        self.notes("area = ?1 AND folder NOT IN ('Archive', 'Log')", &format!("[[{stem}]]"))
    }

    // Every log note's body, by key.
    pub fn logs(&self) -> Result<Vec<(String, String)>> {
        let mut statement = self.db.prepare("SELECT key, body FROM notes_fts WHERE key LIKE 'Log/%'").map_err(|error| error.to_string())?;
        let rows = statement.query_map([], |row| Ok((text(row, 0), text(row, 1)))).map_err(|error| error.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())
    }

    // Checkouts the server's hooks have seen, by repo name.
    pub fn known_root(&self, repo: &str) -> Option<PathBuf> {
        self.db.query_row("SELECT root FROM repos WHERE repo = ?1", [repo], |row| row.get::<_, String>(0)).ok().map(PathBuf::from)
    }

    pub fn last_sync(&self) -> Option<u64> {
        self.db.query_row("SELECT v FROM meta WHERE k = 'last_sync'", [], |row| row.get::<_, String>(0)).ok()?
            .parse::<f64>().ok().map(|seconds| seconds as u64)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    pub date: String,
    pub text: String,
}

// "- [2026-10-16] what happened | commits: abc | branch: x | [[area]]" -> the date and what happened.
pub fn log_line(line: &str) -> Option<LogLine> {
    let rest = line.trim().strip_prefix("- [")?;
    let (date, text) = rest.split_once("] ")?;
    let text = text.split(" | ").next()?.trim();
    Some(LogLine { date: date.to_string(), text: text.to_string() })
}

// A name's own log note when the vault command cannot say: Log/<name>-<word>.md, a word and not a month, so the compiled month pages stay out.
pub fn own_log_guess(logs: &[(String, String)], name: &str) -> Option<String> {
    let word_of = |key: &str| key.strip_prefix(&format!("Log/{name}-")).and_then(|rest| rest.strip_suffix(".md")).filter(|word| !word.is_empty() && word.chars().all(|character| character.is_ascii_alphabetic())).map(str::to_string);
    let words: Vec<(String, String)> = logs.iter().filter_map(|(key, _)| Some((word_of(key)?, key.clone()))).collect();
    words.iter().find(|(word, _)| word == "log").or_else(|| words.first()).map(|(_, key)| key.clone())
}

// An area's log: lines that link it, and every line of its repo's own log notes, newest first.
pub fn log_for(logs: &[(String, String)], stem: &str, own: &[String]) -> Vec<LogLine> {
    let link = format!("[[{stem}]]");
    let mut lines: Vec<LogLine> = Vec::new();
    for (key, body) in logs {
        let whole = own.contains(key);
        for line in body.lines().filter(|line| whole || line.contains(&link)) {
            if let Some(parsed) = log_line(line)
                && !lines.contains(&parsed)
            {
                lines.push(parsed);
            }
        }
    }
    lines.reverse();
    lines.sort_by(|one, other| other.date.cmp(&one.date));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_is_named_like_notes_vault_mcp_names_it() {
        let options = serde_json::json!({ "s3_endpoint": "https://minio.example.com", "s3_bucket": "notes" });
        let expected = format!("{}.sqlite", &sha1_smol::Sha1::from("https://minio.example.comnotes").digest().to_string()[..12]);
        assert_eq!(index_name(&options), Some(expected));
        assert_eq!(index_name(&serde_json::json!({ "s3_bucket": "notes" })), None);
    }

    #[test]
    fn a_log_line_loses_its_commits_and_link() {
        let line = "- [2026-10-16] kelpie 0.9.0: sessions message each other | commits: 7f88afc | branch: x | [[kelpie]]";
        assert_eq!(log_line(line), Some(LogLine { date: "2026-10-16".to_string(), text: "kelpie 0.9.0: sessions message each other".to_string() }));
        assert_eq!(log_line("not a log line"), None);
    }

    #[test]
    fn an_areas_log_is_its_own_note_and_lines_that_link_it_newest_first() {
        let logs = vec![
            ("Log/kelpie-log.md".to_string(), "- [2026-10-15] first | [[kelpie]]\n- [2026-10-16] second\n".to_string()),
            ("Log/api-log.md".to_string(), "- [2026-10-14] api only | [[api]]\n- [2026-10-16] for kelpie too | [[kelpie]]\n".to_string()),
        ];
        let texts: Vec<String> = log_for(&logs, "kelpie", &["Log/kelpie-log.md".to_string()]).into_iter().map(|line| line.text).collect();
        assert_eq!(texts, ["for kelpie too", "second", "first"]);
    }

    #[test]
    fn a_repos_own_log_is_found_by_whatever_the_vault_calls_it() {
        let logs: Vec<(String, String)> = ["Log/kelpie-2026-09.md", "Log/kelpie-logg.md", "Log/kelpie-plugin-log.md", "Log/api-log.md"].iter().map(|key| (key.to_string(), String::new())).collect();
        assert_eq!(own_log_guess(&logs, "kelpie").as_deref(), Some("Log/kelpie-logg.md"), "the user's logs are -logg.md, and a month page is no own log");
        assert_eq!(own_log_guess(&logs, "api").as_deref(), Some("Log/api-log.md"));
        assert_eq!(own_log_guess(&logs, "web"), None);
        let both: Vec<(String, String)> = ["Log/kelpie-changes.md", "Log/kelpie-log.md"].iter().map(|key| (key.to_string(), String::new())).collect();
        assert_eq!(own_log_guess(&both, "kelpie").as_deref(), Some("Log/kelpie-log.md"), "without the vault command the -log.md note kelpie always read still wins");
        let source = include_str!("vault.rs");
        assert!(!source.contains(&["\"Log/{name}", "-log.md\""].concat()), "the log note's name is the vault's to say");
    }

    #[test]
    fn reads_areas_and_their_notes_from_an_index() {
        let dir = std::env::temp_dir().join(format!("kelpie-vault-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.sqlite");
        let _ = std::fs::remove_file(&path);
        let db = Connection::open(&path).unwrap();
        db.execute_batch("
            CREATE TABLE notes (key TEXT PRIMARY KEY, version TEXT, mtime REAL, folder TEXT, stem TEXT, title TEXT, summary TEXT, status TEXT,
              kind TEXT, area TEXT, tags TEXT, paths TEXT, date TEXT, updated TEXT, superseded_by TEXT, valid INT, error TEXT, size INT, priority TEXT, source TEXT);
            CREATE TABLE repos (repo TEXT PRIMARY KEY, root TEXT, seen_at TEXT);
            CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT);
            CREATE VIRTUAL TABLE notes_fts USING fts5(title, summary, tags, body, stem, stemmed, key UNINDEXED);
            INSERT INTO notes (key, folder, stem, title, status, kind, paths, valid) VALUES ('Areas/kelpie.md', 'Areas', 'kelpie', 'kelpie', 'active', 'system', '[\"~/personal/kelpie\"]', 1);
            INSERT INTO notes (key, folder, stem, title, status, kind, area, priority, valid) VALUES ('Projects/kelpie-x.md', 'Projects', 'kelpie-x', 'X', 'backlog', 'task', '[[kelpie]]', 'high', 1);
            INSERT INTO notes (key, folder, stem, title, status, kind, area, valid) VALUES ('Archive/kelpie-y.md', 'Archive', 'kelpie-y', 'Y', 'complete', 'task', '[[kelpie]]', 1);
            INSERT INTO repos VALUES ('kelpie', '/src/kelpie', '2026-10-16');
            INSERT INTO meta VALUES ('last_sync', '1792172065.5');
            INSERT INTO notes_fts (key, body) VALUES ('Log/kelpie-log.md', '- [2026-10-16] done');
            INSERT INTO notes (key, folder, stem, title, status, kind, area, mtime, valid) VALUES ('Projects/kelpie-old.md', 'Projects', 'kelpie-old', 'Old', 'active', 'task', '[[other-area]]', 100, 1);
            INSERT INTO notes (key, folder, stem, title, status, kind, area, mtime, valid) VALUES ('Projects/kelpie-new.md', 'Projects', 'kelpie-new', 'New', 'active', 'task', '[[other-area]]', 200, 1);
            INSERT INTO notes (key, folder, stem, title, status, kind, area, mtime, valid) VALUES ('Projects/kelpie-broken.md', 'Projects', 'kelpie-broken', 'Broken', 'active', 'task', '[[other-area]]', 300, 0);
            INSERT INTO notes (key, folder, stem, title, status, kind, area, mtime, valid) VALUES ('Projects/kelpie-done.md', 'Projects', 'kelpie-done', 'Done', 'complete', 'task', '[[other-area]]', 400, 1);
            INSERT INTO notes (key, folder, stem, title, status, kind, area, mtime, valid) VALUES ('Projects/kelpie-draft.md', 'Projects', 'kelpie-draft', 'Draft', 'draft', 'task', '[[other-area]]', 150, 1);
            INSERT INTO notes (key, folder, stem, title, status, kind, mtime, valid) VALUES ('Resources/other.md', 'Resources', 'other', 'Other', 'active', 'howto', 500, 1);
            INSERT INTO notes_fts (key, body) VALUES ('Projects/kelpie-new.md', '## Problem
The payload says amount.');
        ").unwrap();
        drop(db);
        let index = Index::at(&path).unwrap();
        let areas = index.areas().unwrap();
        assert_eq!(areas.len(), 1);
        assert_eq!(areas[0].paths, ["~/personal/kelpie"]);
        let notes = index.notes_of("kelpie").unwrap();
        assert_eq!(notes.iter().map(|note| note.key.as_str()).collect::<Vec<_>>(), ["Projects/kelpie-x.md"]);
        assert_eq!(notes[0].priority, "high");
        assert_eq!(index.known_root("kelpie"), Some(PathBuf::from("/src/kelpie")));
        assert_eq!(index.last_sync(), Some(1792172065));
        assert_eq!(index.logs().unwrap(), [("Log/kelpie-log.md".to_string(), "- [2026-10-16] done".to_string())]);
        let ongoing: Vec<String> = index.ongoing().unwrap().into_iter().map(|note| note.key).collect();
        assert_eq!(ongoing, ["Projects/kelpie-new.md", "Projects/kelpie-draft.md", "Projects/kelpie-old.md"], "active and draft projects, as the vault's Open tasks base shows them, newest first");
        assert_eq!(index.ongoing().unwrap()[0].area, "other-area");
        assert_eq!(notes[0].area, "kelpie", "the [[link]] reads as the area's stem");
        assert_eq!(index.body("Projects/kelpie-new.md").as_deref(), Some("## Problem\nThe payload says amount."));
        assert_eq!(index.body("Projects/missing.md"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tilde_path_expands_under_home_and_an_area_link_loses_its_brackets() {
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(expand("~/personal/kelpie"), PathBuf::from(&home).join("personal/kelpie"));
        assert_eq!(expand("~personal"), PathBuf::from("~personal"), "only ~/ is the home");
        assert_eq!(expand("/abs"), PathBuf::from("/abs"));
        assert_eq!(area_name(" [[kelpie]] "), "kelpie");
        assert_eq!(area_name("kelpie"), "kelpie");
        assert_eq!(area_name("[[a]] [[b]]"), "a]] [[b");
    }

    #[test]
    fn an_areas_log_lines_are_unique_and_sorted_by_date_whatever_note_they_come_from() {
        let logs = vec![
            ("Log/a-log.md".to_string(), "- [2026-10-01] same | [[x]]\n- [2026-10-03] late\nnot a line\n".to_string()),
            ("Log/b-log.md".to_string(), "- [2026-10-01] same | commits: abc | [[x]]\n- [2026-10-02] linked | [[x]]\n- [2026-10-09] other area | [[y]]\n".to_string()),
        ];
        let lines = log_for(&logs, "x", &["Log/a-log.md".to_string()]);
        let shown: Vec<(&str, &str)> = lines.iter().map(|line| (line.date.as_str(), line.text.as_str())).collect();
        assert_eq!(shown, [("2026-10-03", "late"), ("2026-10-02", "linked"), ("2026-10-01", "same")]);
        assert_eq!(log_line("- [2026-10-01]x"), None, "the date needs its closing space");
        assert_eq!(log_line("  - [2026-10-01] indented | [[x]]  "), Some(LogLine { date: "2026-10-01".to_string(), text: "indented".to_string() }));
    }

    #[test]
    fn the_index_is_opened_read_only_so_kelpie_never_writes_the_servers_database() {
        let dir = std::env::temp_dir().join(format!("kelpie-vault-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.sqlite");
        let _ = std::fs::remove_file(&path);
        assert!(Index::at(&path).is_err(), "a missing index is an error, not a new database");
        assert!(!path.exists());
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE notes (key TEXT PRIMARY KEY, mtime REAL, status TEXT); INSERT INTO notes VALUES ('Projects/x.md', 5.5, 'active');").unwrap();
        let index = Index::at(&path).unwrap();
        assert_eq!(index.changed_at("Projects/x.md"), Some(5.5));
        assert_eq!(index.status_of("Projects/x.md").as_deref(), Some("active"));
        assert_eq!(index.status_of("Projects/x.md'; DROP TABLE notes; --"), None, "keys are bound, never spliced");
        assert!(index.db.execute("INSERT INTO notes VALUES ('y', 1, 'x')", []).is_err(), "read-only");
        assert!(index.areas().is_err(), "a schema without the columns is an error, not a panic");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
