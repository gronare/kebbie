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

fn text(row: &rusqlite::Row, index: usize) -> String {
    row.get::<_, Option<String>>(index).ok().flatten().unwrap_or_default()
}

const NOTE_COLUMNS: &str = "key, stem, title, summary, status, kind, priority, paths, updated";

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

    // The system notes, one per app or subsystem.
    pub fn areas(&self) -> Result<Vec<Note>> {
        self.notes("folder = ?1 AND valid = 1", "Areas")
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

// An area's log: lines that link it, and every line of its repo's own log note, newest first.
pub fn log_for(logs: &[(String, String)], stem: &str, repo: Option<&str>) -> Vec<LogLine> {
    let link = format!("[[{stem}]]");
    let own: Vec<String> = [Some(stem), repo].into_iter().flatten().map(|name| format!("Log/{name}-log.md")).collect();
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
        let texts: Vec<String> = log_for(&logs, "kelpie", Some("kelpie")).into_iter().map(|line| line.text).collect();
        assert_eq!(texts, ["for kelpie too", "second", "first"]);
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}
