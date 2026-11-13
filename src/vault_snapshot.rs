use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::sh::Result;
use crate::vault::{self, Claim, LogEntry, Note};
use crate::vault_cli::Cli;

pub const EVERY: u64 = 20;
const FILE: &str = "vault-snapshot.json";
const VERDICT: &str = "vault-snapshot-verdict.json";
const RETRY: u64 = 600;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub note: Note,
    pub role: String,
    pub mtime: f64,
    pub date: String,
    pub claim: Option<Claim>,
}

// What the vault command's `snapshot --json` answered, as kebbie last kept it.
#[derive(Debug, PartialEq)]
pub struct Snapshot {
    pub etag: String,
    pub fetched: Option<u64>,
    pub log_key_template: String,
    pub notes: Vec<Entry>,
    pub openings: HashMap<String, String>,
    pub logs: Vec<(String, Vec<LogEntry>)>,
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

fn paths(value: &Value) -> Vec<String> {
    match value {
        Value::String(path) => vec![path.clone()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

fn entry(value: &Value) -> Option<Entry> {
    let key = text(value, "key");
    if key.is_empty() {
        return None;
    }
    let updated = text(value, "updated");
    let claim = match &value["claim"] {
        Value::String(raw) => serde_json::from_str(raw).ok(),
        other => Some(other.clone()),
    }.and_then(|claim| Claim::from_json(&claim, &updated));
    let note = Note {
        stem: text(value, "stem"), title: text(value, "title"), summary: text(value, "summary"), status: text(value, "status"), kind: text(value, "kind"),
        priority: text(value, "priority"), paths: paths(&value["paths"]), area: vault::area_name(&text(value, "area")), updated, key,
    };
    Some(Entry { role: text(value, "role"), mtime: value["mtime"].as_f64().unwrap_or(0.0), date: text(value, "date"), claim, note })
}

// A line's text is what comes before its " | commits" and links, as in the note itself.
fn log_entry(value: &Value) -> Option<LogEntry> {
    let date = text(value, "date");
    let text = text(value, "text");
    let text = text.split(" | ").next().unwrap_or_default().trim().to_string();
    let links = value["links"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    vault::is_date(&date).then_some(LogEntry { date, text, links })
}

pub fn parse(raw: &str, fetched: Option<u64>) -> Result<Snapshot> {
    let value: Value = serde_json::from_str(raw).map_err(|error| format!("vault snapshot: not JSON ({error})"))?;
    let notes = value["notes"].as_array().ok_or("vault snapshot: no notes in its answer")?;
    let mut logs: Vec<(String, Vec<LogEntry>)> = value["logs"].as_object().into_iter().flatten()
        .map(|(key, lines)| (key.clone(), lines.as_array().into_iter().flatten().filter_map(log_entry).collect())).collect();
    logs.sort_by(|one, other| one.0.cmp(&other.0));
    Ok(Snapshot {
        etag: text(&value, "etag"),
        fetched,
        log_key_template: text(&value, "log_key_template"),
        notes: notes.iter().filter_map(entry).collect(),
        openings: value["openings"].as_object().into_iter().flatten().filter_map(|(key, opening)| Some((key.clone(), opening.as_str()?.to_string()))).collect(),
        logs,
    })
}

pub fn file(state: &Path) -> PathBuf {
    state.join(FILE)
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok())
}

fn seconds(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

// The kept snapshot, parsed once per write: readers run every few seconds and the file is a few hundred KB.
fn load(path: &Path) -> Option<Arc<Snapshot>> {
    type Kept = Option<(PathBuf, SystemTime, u64, Arc<Snapshot>)>;
    static KEPT: Mutex<Kept> = Mutex::new(None);
    let meta = std::fs::metadata(path).ok()?;
    let (modified, size) = (meta.modified().ok()?, meta.len());
    let mut kept = KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((at, when, length, snapshot)) = kept.as_ref()
        && at == path && *when == modified && *length == size
    {
        return Some(snapshot.clone());
    }
    let snapshot = Arc::new(parse(&std::fs::read_to_string(path).ok()?, Some(seconds(modified))).ok()?);
    *kept = Some((path.to_path_buf(), modified, size, snapshot.clone()));
    Some(snapshot)
}

fn supported(state: &Path) -> bool {
    read_json(&state.join(VERDICT)).is_some_and(|verdict| verdict["supported"] == true)
}

// The snapshot readers use, only while the installed vault server answers snapshots; else they read the index.
pub fn current(state: &Path) -> Option<Arc<Snapshot>> {
    if !supported(state) {
        return None;
    }
    load(&file(state))
}

#[derive(Debug, PartialEq)]
pub enum Refreshed {
    Unsupported,
    Unchanged,
    Written,
}

fn keep_verdict(cli: &Cli, state: &Path, at: u64, supported: bool) {
    let kept = read_json(&state.join(VERDICT));
    if kept.as_ref().is_some_and(|kept| kept["command"] == cli.cache_key() && kept["supported"] == supported && (supported || kept["at"] == at)) {
        return;
    }
    let _ = crate::state::write(&state.join(VERDICT), json!({ "command": cli.cache_key(), "at": at, "supported": supported }).to_string());
}

// Asks the server for what changed since the kept etag; a server without the command is asked again after RETRY or once its plugin version changes.
pub fn refresh(cli: &Cli, state: &Path, now: u64) -> Result<Refreshed> {
    if !cli.can(crate::vault_cli::SNAPSHOT) {
        return Ok(Refreshed::Unsupported);
    }
    let verdict = read_json(&state.join(VERDICT)).filter(|verdict| verdict["command"] == cli.cache_key());
    if verdict.as_ref().is_some_and(|verdict| verdict["supported"] == false && verdict["at"].as_u64().is_some_and(|then| now.saturating_sub(then) < RETRY)) {
        return Ok(Refreshed::Unsupported);
    }
    let kept = verdict.filter(|verdict| verdict["supported"] == true).and_then(|_| load(&file(state)));
    let answer = match cli.snapshot(kept.as_ref().map(|snapshot| snapshot.etag.as_str()).filter(|etag| !etag.is_empty()))? {
        Err(refusal) if refusal.contains("invalid choice") => {
            keep_verdict(cli, state, now, false);
            return Ok(Refreshed::Unsupported);
        }
        Err(refusal) => return Err(format!("vault snapshot: {refusal}")),
        Ok(answer) => answer,
    };
    let value: Value = serde_json::from_str(answer.trim()).map_err(|error| format!("vault snapshot: not JSON ({error})"))?;
    if value["unchanged"] == true && kept.is_some() {
        keep_verdict(cli, state, now, true);
        let _ = std::fs::File::options().append(true).open(file(state)).and_then(|file| file.set_modified(SystemTime::now()));
        return Ok(Refreshed::Unchanged);
    }
    parse(answer.trim(), None)?;
    crate::state::write(&file(state), answer.trim())?;
    keep_verdict(cli, state, now, true);
    Ok(Refreshed::Written)
}

// One refresh for whichever process keeps the vault fresh: the keeper when it runs, else the board.
pub fn tick(state: &Path) -> Result<()> {
    let Some(cli) = crate::vault_cli::detect() else { return Ok(()) };
    tick_with(&cli, state, seconds(SystemTime::now()))
}

// While the keeper's watch follows this command the snapshot is kept current from its stream; a watch for another command is stopped, and the keeper starts one for this.
pub fn tick_with(cli: &Cli, state: &Path, now: u64) -> Result<()> {
    if let Err(error) = crate::item_template::refresh(cli, state, now) {
        crate::vault_cli::warn_log(state, &format!("the vault's task template was not read: {error}"));
    }
    if crate::vault_watch::live(state, cli) {
        return Ok(());
    }
    if crate::vault_watch::watching(state) {
        crate::vault_watch::stop(state);
    }
    refresh(cli, state, now).map(|_| ())
}

// The board's own refresher, idle while the keeper owns the work.
pub fn spawn_board_refresher() {
    std::thread::spawn(|| {
        let mut said = String::new();
        loop {
            if crate::keeper::current_owner() == crate::keeper::Owner::Board
                && let Ok(state) = crate::lead::state_dir()
            {
                let error = tick(&state).err().unwrap_or_default();
                if !error.is_empty() && error != said {
                    crate::vault_cli::warn_log(&state, &error);
                }
                said = error;
            }
            std::thread::sleep(std::time::Duration::from_secs(EVERY));
        }
    });
}

// For a command run on its own, with no board or keeper keeping the snapshot fresh.
pub fn refresh_if_older(state: &Path, seconds_old: u64) {
    let now = seconds(SystemTime::now());
    if crate::vault_watch::watching(state) || current(state).is_some_and(|snapshot| snapshot.fetched.is_some_and(|at| now.saturating_sub(at) < seconds_old)) {
        return;
    }
    let _ = tick(state);
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::vault_cli::fake::Fake;

    const ANSWER: &str = r#"{"etag":"e1","synced_at":"2026-11-01T18:38:14+00:00","layout":"default","log_key_template":"Log/{repo}-logg.md",
      "notes":[
        {"key":"Areas/dotfiles/chezmoi.md","stem":"chezmoi","role":"system","kind":"system","status":"active","title":"Dotfiles","summary":"s","priority":"","area":"","paths":["~/.dotfiles"],"date":"2026-02-21","updated":"2026-10-22","mtime":5.0,"claim":null},
        {"key":"Projects/old.md","stem":"old","role":"task","kind":"task","status":"active","title":"Old","summary":"","priority":null,"area":"[[chezmoi]]","paths":[],"date":"2026-10-01","updated":"2026-10-02","mtime":10.0,"claim":null},
        {"key":"Projects/new.md","stem":"new","role":"task","kind":"task","status":"draft","title":"New","summary":"","priority":"","area":"chezmoi","paths":[],"date":"2026-10-01","updated":"2026-10-20","mtime":20.0,"claim":{"machine":"devbox","task":"billing","repo":"api","since":"2026-10-19T08:00:00Z"}},
        {"key":"Projects/later.md","stem":"later","role":"task","kind":"task","status":"backlog","title":"Later","summary":"one line","priority":"high","area":"[[chezmoi]]","paths":[],"date":"2026-10-03","updated":"2026-10-03","mtime":30.0,"claim":null}
      ],
      "openings":{"Projects/new.md":"The payload says amount."},
      "logs":{"Log/dotfiles-logg.md":[{"date":"2026-10-18","text":"set up | commits: abc | [[chezmoi]]","links":["chezmoi"]},{"date":"bad","text":"x","links":[]}],"Log/api-logg.md":[{"date":"2026-10-19","text":"api only","links":["api"]}]}}"#;

    fn of(raw: &str) -> vault::Index {
        vault::Index::of_snapshot(Arc::new(parse(raw, Some(100)).unwrap()), None)
    }

    #[test]
    fn the_snapshot_answers_what_the_index_answers() {
        let index = of(ANSWER);
        let areas = index.areas().unwrap();
        assert_eq!(areas.iter().map(|note| note.key.as_str()).collect::<Vec<_>>(), ["Areas/dotfiles/chezmoi.md"], "role, not the folder, makes a system note");
        assert_eq!(areas[0].paths, ["~/.dotfiles"]);
        assert_eq!(index.notes_of("chezmoi").unwrap().iter().map(|note| note.key.as_str()).collect::<Vec<_>>(), ["Projects/later.md", "Projects/new.md", "Projects/old.md"], "an area link with or without brackets");
        assert_eq!(index.ongoing().unwrap().iter().map(|note| note.key.as_str()).collect::<Vec<_>>(), ["Projects/new.md", "Projects/old.md"], "active and draft tasks, newest first");
        assert_eq!(index.backlog().unwrap(), [["Projects/later.md", "Later", "one line", "high", "chezmoi", "2026-10-03"].map(str::to_string)]);
        let claims = index.claims();
        assert_eq!((claims.len(), claims["Projects/new.md"].machine.as_str(), claims["Projects/new.md"].updated.as_str()), (1, "devbox", "2026-10-20"));
        assert_eq!(index.opening("Projects/new.md").as_deref(), Some("The payload says amount."));
        assert_eq!(index.body("Projects/new.md"), None, "a snapshot carries openings, not bodies");
        assert_eq!((index.changed_at("Projects/old.md"), index.status_of("Projects/later.md").as_deref(), index.title_of("Projects/new.md").as_deref()), (Some(10.0), Some("backlog"), Some("New")));
        assert_eq!(index.status_of("Projects/gone.md"), None);
        assert_eq!(index.log_key("dotfiles").as_deref(), Some("Log/dotfiles-logg.md"));
        assert_eq!(index.last_sync(), Some(100));
        assert_eq!(index.known_root("dotfiles"), None, "without a local index no checkout is known");
        let logs = index.logs().unwrap();
        assert_eq!(logs.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(), ["Log/api-logg.md", "Log/dotfiles-logg.md"]);
        assert_eq!(logs[1].1, [LogEntry { date: "2026-10-18".to_string(), text: "set up".to_string(), links: vec!["chezmoi".to_string()] }], "the text loses its commits, a line without a date is dropped");
        let lines = vault::log_for(&logs, "chezmoi", &["Log/api-logg.md".to_string()]);
        assert_eq!(lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), ["api only", "set up"]);
        assert!(parse(r#"{"etag":"x"}"#, None).is_err(), "an answer without notes is no snapshot");
    }

    #[test]
    fn the_snapshot_is_kept_and_refreshed_since_its_etag_and_an_old_server_keeps_the_index() {
        let fake = Fake::new("snapshot");
        let state = fake.state();
        fake.fail("snapshot---json", "notes-vault-mcp: error: argument command: invalid choice: 'snapshot' (choose from serve, init)");
        assert_eq!(refresh(&fake.cli, &state, 1000), Ok(Refreshed::Unsupported));
        assert_eq!(refresh(&fake.cli, &state, 1000 + RETRY - 1), Ok(Refreshed::Unsupported));
        assert_eq!(fake.calls(), ["snapshot --json"], "the verdict is kept, so an old server is asked again only after a while");
        assert!(current(&state).is_none());
        let _ = std::fs::remove_file(fake.dir.join("answers/snapshot---json.err"));
        fake.answer("snapshot---json", ANSWER);
        assert_eq!(refresh(&fake.cli, &state, 1000 + RETRY), Ok(Refreshed::Written));
        assert_eq!(std::fs::metadata(file(&state)).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(current(&state).unwrap().notes.len(), 4);
        fake.answer("snapshot---json", r#"{"unchanged": true, "etag": "e1"}"#);
        assert_eq!(refresh(&fake.cli, &state, 2000), Ok(Refreshed::Unchanged));
        assert_eq!(fake.calls().last().unwrap(), "snapshot --json --since e1");
        assert_eq!(current(&state).unwrap().etag, "e1", "unchanged keeps the snapshot");
        fake.fail("snapshot---json", "vault server unreachable");
        assert!(refresh(&fake.cli, &state, 3000).is_err());
        assert!(current(&state).is_some(), "a failed refresh keeps the last snapshot");
        let other = Cli { plugin: "9.9.9".to_string(), ..fake.cli.clone() };
        fake.fail("snapshot---json", "invalid choice: 'snapshot'");
        assert_eq!(refresh(&other, &state, 3001), Ok(Refreshed::Unsupported), "another plugin version is asked afresh");
        assert!(current(&state).is_none(), "a server without snapshots is read through the index, whatever file is left");
    }

    #[test]
    fn a_vault_that_states_no_snapshot_capability_is_never_asked_for_one() {
        let fake = Fake::new("snapshot-capability");
        let state = fake.state();
        fake.answer("snapshot---json", ANSWER);
        let without = Cli { capabilities: Some(vec!["tasks".to_string(), "log".to_string()]), ..fake.cli.clone() };
        assert_eq!(refresh(&without, &state, 1000), Ok(Refreshed::Unsupported));
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
        let with = Cli { capabilities: Some(vec!["snapshot".to_string()]), ..fake.cli.clone() };
        assert_eq!(refresh(&with, &state, 1000), Ok(Refreshed::Written));
    }
}
