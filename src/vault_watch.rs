use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::sh::Result;
use crate::vault_cli::{self, Cli};

const FILE: &str = "vault-watch.json";
const FIRST_PAUSE: u64 = 1;
const LONGEST_PAUSE: u64 = 300;
const UNABLE_PAUSE: u64 = 60;
const LOG_LINES: usize = 50;
const OPENING_MAX: usize = 600;
const OPEN_STATUSES: [&str; 2] = ["active", "draft"];
const ROLES: [&str; 2] = ["system", "task"];

static CHILD: Mutex<Option<u32>> = Mutex::new(None);

fn file(state: &Path) -> PathBuf {
    state.join(FILE)
}

fn read(state: &Path) -> Value {
    std::fs::read_to_string(file(state)).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_else(|| json!({}))
}

fn keep(state: &Path, value: &Value) {
    let _ = crate::state::write(&file(state), value.to_string());
}

fn alive(pid: u64) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    pid > 0 && unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

// The vault states every call the watch needs: snapshot for a reset, watch for the stream.
pub fn capable(cli: &Cli) -> bool {
    cli.capabilities.is_some() && [vault_cli::SNAPSHOT, vault_cli::WATCH].iter().all(|capability| cli.can(capability))
}

// A keeper is following the vault: its watch child and the keeper itself still run.
pub fn watching(state: &Path) -> bool {
    let kept = read(state);
    kept["keeper"].as_u64().is_some_and(alive) && kept["child"].as_u64().is_some_and(alive)
}

// The watch runs for this command, so its kept files are current without asking the vault.
pub fn live(state: &Path, cli: &Cli) -> bool {
    capable(cli) && read(state)["command"] == cli.cache_key() && watching(state)
}

pub fn kept_cursor(state: &Path, cli: &Cli) -> Option<String> {
    let kept = read(state);
    (kept["command"] == cli.cache_key()).then(|| kept["cursor"].as_str().map(str::to_string)).flatten().filter(|cursor| !cursor.is_empty())
}

fn record(state: &Path, cli: &Cli, cursor: Option<&str>, child: Option<u32>) {
    keep(state, &json!({ "command": cli.cache_key(), "cursor": cursor, "keeper": std::process::id(), "child": child }));
}

fn forget_child(state: &Path) {
    let mut kept = read(state);
    if kept.get("child").is_some_and(|child| !child.is_null()) {
        kept["child"] = Value::Null;
        keep(state, &kept);
    }
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

fn as_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Array(items) => items.iter().map(as_text).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

fn stem_of(key: &str) -> &str {
    let name = key.rsplit('/').next().unwrap_or(key);
    name.strip_suffix(".md").unwrap_or(name)
}

fn folder_of(key: &str) -> &str {
    key.split_once('/').map_or("", |(folder, _)| folder)
}

fn paths(raw: &Value) -> Vec<String> {
    let values: Vec<String> = match raw {
        Value::Null => Vec::new(),
        Value::Array(items) => items.iter().map(as_text).collect(),
        other => as_text(other).split(", ").map(str::to_string).collect(),
    };
    values.into_iter().map(|path| path.trim().to_string()).filter(|path| !path.is_empty()).collect()
}

fn claim(raw: &Value) -> Value {
    let Some(fields) = raw.as_object() else { return Value::Null };
    let claim: Map<String, Value> = ["machine", "task", "repo", "since"].iter().map(|name| (name.to_string(), Value::String(as_text(fields.get(*name).unwrap_or(&Value::Null)).trim().to_string()))).collect();
    if ["machine", "task", "since"].iter().all(|name| claim[*name].as_str().is_some_and(|text| !text.is_empty())) { Value::Object(claim) } else { Value::Null }
}

// The snapshot's row for a note, as the vault's own snapshot builds it from the frontmatter.
fn row(key: &str, role: &str, frontmatter: &Value, mtime: f64) -> Value {
    let field = |name: &str| as_text(&frontmatter[name]);
    let title = field("title");
    json!({
        "key": key, "stem": stem_of(key), "role": role, "kind": field("kind"), "status": field("status"),
        "title": if title.is_empty() { stem_of(key).to_string() } else { title }, "summary": field("summary"), "priority": field("priority"),
        "area": field("area"), "paths": paths(&frontmatter["path"]), "date": field("date"), "updated": field("updated"), "mtime": mtime,
        "claim": claim(&frontmatter["kebbie"]),
    })
}

// The first paragraph after any leading heading, as the vault's snapshot cuts a task's opening.
pub fn opening(body: &str) -> String {
    let mut paragraph: Vec<&str> = Vec::new();
    for line in body.lines() {
        let text = line.trim();
        if text.is_empty() {
            if paragraph.is_empty() { continue } else { break }
        }
        if text.starts_with('#') {
            if paragraph.is_empty() { continue } else { break }
        }
        paragraph.push(text);
    }
    let joined = paragraph.join(" ");
    if joined.chars().count() <= OPENING_MAX {
        return joined;
    }
    format!("{}…", joined.chars().take(OPENING_MAX - 1).collect::<String>().trim_end())
}

fn links_in(text: &str) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    for raw in text.split("[[").skip(1).filter_map(|rest| rest.split_once("]]").map(|(link, _)| link)).filter(|link| !link.is_empty() && !link.contains('\n')) {
        let target = raw.split('|').next().unwrap_or_default().split('#').next().unwrap_or_default().trim();
        let stem = target.rsplit('/').next().unwrap_or_default().to_string();
        if !stem.is_empty() && !links.contains(&stem) {
            links.push(stem);
        }
    }
    links
}

// "- [2026-10-16] text" lines, the last 50, shaped as the snapshot keeps a log note.
pub fn log_lines(body: &str) -> Vec<Value> {
    let lines: Vec<Value> = body.lines().filter_map(|line| {
        let rest = line.trim_start().strip_prefix(['-', '*'])?.strip_prefix(" [")?;
        let (date, text) = (rest.get(..10)?, rest.get(10..)?.strip_prefix(']')?.trim());
        let digits = date.char_indices().all(|(at, character)| if at == 4 || at == 7 { character == '-' } else { character.is_ascii_digit() });
        digits.then(|| json!({ "date": date, "text": text, "links": links_in(text) }))
    }).collect();
    lines[lines.len().saturating_sub(LOG_LINES)..].to_vec()
}

#[derive(Debug, Default, PartialEq)]
pub struct Applied {
    pub snapshot: bool,
    pub full: bool,
    pub bodies: Vec<(String, String)>,
    pub gone: Vec<String>,
}

fn known_log(template: &str, key: &str) -> bool {
    template.split_once("{repo}").is_some_and(|(before, after)| key.len() > before.len() + after.len() && key.starts_with(before) && key.ends_with(after))
}

// A note the kept snapshot does not have may belong on it; only the vault knows its role, so it is asked for a snapshot.
fn may_belong(snapshot: &Value, key: &str, frontmatter: &Value) -> bool {
    let kind = as_text(&frontmatter["kind"]);
    let folders: BTreeSet<&str> = snapshot["notes"].as_array().into_iter().flatten().filter_map(|note| note["key"].as_str()).map(folder_of).collect();
    ROLES.contains(&kind.as_str()) || folders.contains(folder_of(key)) || known_log(snapshot["log_key_template"].as_str().unwrap_or_default(), key)
}

fn remove(snapshot: &mut Value, key: &str) -> bool {
    let mut removed = false;
    if let Some(notes) = snapshot["notes"].as_array_mut() {
        let before = notes.len();
        notes.retain(|note| note["key"] != key);
        removed |= notes.len() != before;
    }
    for map in ["openings", "logs"] {
        removed |= snapshot[map].as_object_mut().is_some_and(|map| map.remove(key).is_some());
    }
    removed
}

fn write(snapshot: &mut Value, key: &str, note: &Value, mtime: f64, applied: &mut Applied) {
    let (frontmatter, body) = (&note["frontmatter"], note["body"].as_str());
    if note.get("error").is_some() || !frontmatter.is_object() || body.is_none() {
        applied.full = true;
        return;
    }
    let body = body.unwrap_or_default();
    if let Some(lines) = snapshot["logs"].get_mut(key) {
        *lines = Value::Array(log_lines(body));
        applied.snapshot = true;
        return;
    }
    let Some(index) = snapshot["notes"].as_array().and_then(|notes| notes.iter().position(|row| row["key"] == key)) else {
        if may_belong(snapshot, key, frontmatter) {
            applied.full = true;
            applied.bodies.push((key.to_string(), body.to_string()));
        }
        return;
    };
    applied.bodies.push((key.to_string(), body.to_string()));
    let old = &snapshot["notes"][index];
    if old["kind"].as_str().unwrap_or_default() != as_text(&frontmatter["kind"]) {
        applied.full = true;
        return;
    }
    let role = old["role"].as_str().unwrap_or_default().to_string();
    let fresh = row(key, &role, frontmatter, mtime);
    let open = role == "task" && OPEN_STATUSES.contains(&fresh["status"].as_str().unwrap_or_default());
    snapshot["notes"][index] = fresh;
    let cut = if open { opening(body) } else { String::new() };
    if !snapshot["openings"].is_object() {
        snapshot["openings"] = json!({});
    }
    if let Some(openings) = snapshot["openings"].as_object_mut() {
        if cut.is_empty() { openings.remove(key) } else { openings.insert(key.to_string(), Value::String(cut)) };
    }
    applied.snapshot = true;
}

// One batch of the vault's changes applied to the kept snapshot's JSON; what it cannot place from the batch alone it marks for a full snapshot.
pub fn apply(snapshot: &mut Value, changes: &[Value], mtime: f64) -> Applied {
    let mut applied = Applied::default();
    for change in changes {
        let key = change["key"].as_str().unwrap_or_default();
        match change["action"].as_str() {
            Some("write") if !key.is_empty() => write(snapshot, key, &change["note"], mtime, &mut applied),
            Some("delete") if !key.is_empty() => {
                applied.snapshot |= remove(snapshot, key);
                applied.gone.push(key.to_string());
            }
            _ => applied.full = true,
        }
    }
    if applied.snapshot {
        snapshot["etag"] = Value::String(String::new());
    }
    applied
}

// The backlog notes' bodies a batch carried, under the mtime the kept snapshot now gives them, so the backlog reads them without asking the vault.
fn keep_bodies(state: &Path, applied: &Applied) -> Result<()> {
    if applied.bodies.is_empty() && applied.gone.is_empty() {
        return Ok(());
    }
    let Some(snapshot) = crate::vault_snapshot::current(state) else { return Ok(()) };
    let path = crate::backlog::bodies_file(state);
    let mut cached: Value = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    let mut changed = false;
    let Some(map) = cached.as_object_mut() else { return Ok(()) };
    for key in &applied.gone {
        changed |= map.remove(key).is_some();
    }
    for (key, body) in &applied.bodies {
        let entry = snapshot.notes.iter().find(|entry| &entry.note.key == key).filter(|entry| crate::vault::backlog_item(&entry.note.status, &entry.note.priority));
        match entry {
            Some(entry) => {
                let fresh = json!({ "mtime": entry.mtime, "body": body });
                changed |= map.get(key) != Some(&fresh);
                map.insert(key.clone(), fresh);
            }
            None => changed |= map.remove(key).is_some(),
        }
    }
    if changed {
        crate::state::write(&path, cached.to_string())?;
    }
    Ok(())
}

fn full_snapshot(cli: &Cli, state: &Path) -> Result<()> {
    match crate::vault_snapshot::refresh(cli, state, now() as u64)? {
        crate::vault_snapshot::Refreshed::Unsupported => Err("the vault answers no snapshot".to_string()),
        _ => Ok(()),
    }
}

// One line of `watch --json`: a reset takes a full snapshot and fills the backlog bodies once, a batch is applied; the answer is the cursor to keep.
pub fn handle(cli: &Cli, state: &Path, line: &str) -> Result<String> {
    let value: Value = serde_json::from_str(line).map_err(|error| format!("vault watch: not JSON ({error})"))?;
    let cursor = value["cursor"].as_str().filter(|cursor| !cursor.is_empty()).ok_or("vault watch: no cursor in its line")?.to_string();
    if value["reset"] == true || crate::vault_snapshot::current(state).is_none() {
        full_snapshot(cli, state)?;
        if let Ok(index) = crate::vault::Index::of_state(state) {
            crate::backlog::fill_vault_bodies(cli, state, &index);
        }
    }
    let changes = value["changes"].as_array().cloned().unwrap_or_default();
    if changes.is_empty() {
        return Ok(cursor);
    }
    let path = crate::vault_snapshot::file(state);
    let mut snapshot: Value = std::fs::read_to_string(&path).ok().and_then(|raw| serde_json::from_str(&raw).ok()).ok_or("vault watch: no kept snapshot")?;
    let applied = apply(&mut snapshot, &changes, now());
    if applied.full {
        full_snapshot(cli, state)?;
    } else if applied.snapshot {
        crate::state::write(&path, snapshot.to_string())?;
    }
    keep_bodies(state, &applied)?;
    Ok(cursor)
}

fn kill_group(pid: u32) {
    // SAFETY: the child was started as the leader of its own process group, so the group holds only it and what it started.
    unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGTERM) };
}

// Runs one `watch --json --body` child from the kept cursor until it exits or a line cannot be applied; the answer says why it ended.
pub fn follow(cli: &Cli, state: &Path) -> String {
    let cursor = kept_cursor(state, cli);
    let mut args = vec!["watch", "--json", "--body"];
    args.extend(cursor.as_deref().map(|cursor| ["--since", cursor]).into_iter().flatten());
    let Some((program, rest)) = cli.command.split_first() else { return "has no command".to_string() };
    let mut held = CHILD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if held.is_some() {
        return "already runs".to_string();
    }
    let spawned = Command::new(program).args(rest).args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return format!("did not start: {program}: {error}"),
    };
    *held = Some(child.id());
    drop(held);
    record(state, cli, cursor.as_deref(), Some(child.id()));
    if let Some(stderr) = child.stderr.take() {
        let state = state.to_path_buf();
        std::thread::spawn(move || BufReader::new(stderr).lines().map_while(std::result::Result::ok).filter(|line| !line.trim().is_empty()).for_each(|line| vault_cli::warn_log(&state, &format!("vault watch: {line}"))));
    }
    let mut ended = None;
    let mut kept = cursor;
    for line in child.stdout.take().map(|out| BufReader::new(out).lines()).into_iter().flatten() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match handle(cli, state, &line) {
            Ok(cursor) => {
                if kept.as_deref() != Some(cursor.as_str()) {
                    record(state, cli, Some(&cursor), Some(child.id()));
                    kept = Some(cursor);
                }
            }
            Err(error) => {
                ended = Some(error);
                kill_group(child.id());
                break;
            }
        }
    }
    let status = child.wait();
    *CHILD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    forget_child(state);
    ended.unwrap_or_else(|| status.map_or_else(|error| error.to_string(), |status| format!("ended ({status})")))
}

// A watch child a former keeper left behind is stopped, so the vault is never followed twice.
fn stop_leftover(state: &Path) {
    let Some(pid) = read(state)["child"].as_u64().filter(|pid| alive(*pid)) else { return };
    let command = crate::sh::attempt("ps", &["-o", "command=", "-p", &pid.to_string()], None).ok().and_then(std::result::Result::ok).unwrap_or_default();
    // SAFETY: getpgid only reads the process's group.
    let leader = unsafe { libc::getpgid(pid as libc::pid_t) } == pid as libc::pid_t;
    if leader && command.contains(" watch --json") {
        kill_group(pid as u32);
    }
    forget_child(state);
}

// Follows the vault while the keeper runs: one child at a time, started again after it exits, sooner when it ran a long while.
pub fn supervise(state: &Path, detect: impl Fn() -> Option<Cli>, stopped: impl Fn() -> bool, pause: impl Fn(u64)) {
    stop_leftover(state);
    let mut wait = FIRST_PAUSE;
    while !stopped() {
        let Some(cli) = detect().filter(capable) else {
            forget_child(state);
            pause(UNABLE_PAUSE);
            continue;
        };
        let started = Instant::now();
        let ended = crate::keeper::unpanicked(|| Ok(follow(&cli, state))).unwrap_or_else(|panicked| {
            stop(state);
            *CHILD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            panicked
        });
        if stopped() {
            break;
        }
        if started.elapsed() >= Duration::from_secs(LONGEST_PAUSE) {
            wait = FIRST_PAUSE;
        }
        vault_cli::warn_log(state, &format!("vault watch {ended}; again in {wait} s"));
        pause(wait);
        wait = (wait * 2).min(LONGEST_PAUSE);
    }
    forget_child(state);
}

fn pause_unless_stopped(seconds: u64) {
    let until = Instant::now() + Duration::from_secs(seconds);
    while !crate::keeper::stopping() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200).min(until.saturating_duration_since(Instant::now())));
    }
}

// The keeper's follower, started once per keeper process.
pub fn spawn(state: &Path) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let kept = state.to_path_buf();
    let started = std::thread::Builder::new().name("vault-watch".to_string()).spawn(move || supervise(&kept, vault_cli::detect, crate::keeper::stopping, pause_unless_stopped));
    if let Err(error) = started {
        STARTED.store(false, Ordering::SeqCst);
        crate::keeper::log_line(state, &format!("vault watch did not start: {error}"));
    }
}

// Stops the running child; the supervisor starts it again unless the keeper is stopping.
pub fn stop(state: &Path) {
    if let Some(pid) = *CHILD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
        kill_group(pid);
    }
    forget_child(state);
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;
    use crate::vault_cli::fake::Fake;

    const SNAPSHOT: &str = r#"{"etag":"e1","log_key_template":"Log/{repo}-logg.md",
      "notes":[
        {"key":"Areas/app.md","stem":"app","role":"system","kind":"system","status":"active","title":"App","summary":"","priority":"","area":"","paths":["~/app"],"date":"2026-11-01","updated":"2026-11-01","mtime":1.0,"claim":null},
        {"key":"Projects/later.md","stem":"later","role":"task","kind":"task","status":"backlog","title":"Later","summary":"","priority":"high","area":"[[app]]","paths":[],"date":"2026-11-01","updated":"2026-11-01","mtime":2.0,"claim":null},
        {"key":"Projects/now.md","stem":"now","role":"task","kind":"task","status":"active","title":"Now","summary":"","priority":"","area":"[[app]]","paths":[],"date":"2026-11-01","updated":"2026-11-01","mtime":3.0,"claim":null}
      ],
      "openings":{"Projects/now.md":"Old opening."},
      "logs":{"Log/app-logg.md":[{"date":"2026-11-01","text":"first","links":[]}]}}"#;

    fn written(key: &str, frontmatter: Value, body: &str) -> Value {
        json!({ "key": key, "action": "write", "version": "v2", "note": { "frontmatter": frontmatter, "body": body } })
    }

    fn capable_cli(fake: &Fake) -> Cli {
        Cli { capabilities: Some(["tasks", "snapshot", "read", "watch"].map(str::to_string).to_vec()), ..fake.cli.clone() }
    }

    #[test]
    fn a_batch_rewrites_known_notes_and_logs_and_asks_for_a_snapshot_only_for_what_it_cannot_place() {
        let mut snapshot: Value = serde_json::from_str(SNAPSHOT).unwrap();
        let claim = json!({ "machine": "devbox", "task": "billing", "repo": "api", "since": "2026-11-12T08:00:00Z" });
        let applied = apply(&mut snapshot, &[
            written("Projects/later.md", json!({ "title": "Later, sooner", "kind": "task", "status": "backlog", "priority": "urgent", "area": "[[app]]", "date": "2026-11-01", "kebbie": claim }), "## Background\n\nWhy.\n"),
            written("Projects/now.md", json!({ "title": "Now", "kind": "task", "status": "active", "path": "~/app, ~/web" }), "# Now\n\nThe new opening\nwraps here.\n\nMore.\n"),
            written("Log/app-logg.md", json!({ "title": "app log" }), "- [2026-11-01] first\n- [2026-11-12] second | commits: abc | [[app]] [[Areas/web|Web]]\nnot a line\n"),
            json!({ "key": "Areas/app.md", "action": "delete", "version": null, "note": null }),
            written("Resources/trap.md", json!({ "kind": "trap" }), "x"),
        ], 50.0);
        assert!(applied.snapshot && !applied.full, "{applied:?}");
        assert_eq!(applied.bodies.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(), ["Projects/later.md", "Projects/now.md"]);
        assert_eq!(applied.gone, ["Areas/app.md"]);
        assert_eq!(snapshot["etag"], "", "a patched snapshot is never answered unchanged against its old etag");
        let parsed = crate::vault_snapshot::parse(&snapshot.to_string(), None).unwrap();
        assert_eq!(parsed.notes.iter().map(|entry| entry.note.key.as_str()).collect::<Vec<_>>(), ["Projects/later.md", "Projects/now.md"]);
        let later = &parsed.notes[0];
        assert_eq!((later.note.title.as_str(), later.note.priority.as_str(), later.role.as_str(), later.mtime), ("Later, sooner", "urgent", "task", 50.0));
        assert_eq!(later.claim.as_ref().map(|claim| claim.machine.as_str()), Some("devbox"));
        assert_eq!(parsed.notes[1].note.paths, ["~/app", "~/web"]);
        assert_eq!(parsed.openings["Projects/now.md"], "The new opening wraps here.");
        assert_eq!(parsed.logs[0].1.iter().map(|line| (line.text.as_str(), line.links.clone())).collect::<Vec<_>>(), [("first", vec![]), ("second", vec!["app".to_string(), "web".to_string()])]);
        for (change, why) in [
            (json!({ "key": ".vault/schema.yml", "action": "schema", "version": "s", "note": null }), "a schema change"),
            (written("Projects/new.md", json!({ "kind": "trap" }), "x"), "a new note in a folder the board reads"),
            (written("Archive/old.md", json!({ "kind": "task" }), "x"), "a new note of a board kind"),
            (written("Log/web-logg.md", json!({}), "- [2026-11-12] x"), "a new log note"),
            (written("Projects/now.md", json!({ "kind": "trap" }), "x"), "a known note whose kind changed"),
            (json!({ "key": "Projects/now.md", "action": "write", "note": { "frontmatter": {}, "body": "x", "error": "bad yaml" } }), "a note whose frontmatter does not parse"),
        ] {
            let mut snapshot: Value = serde_json::from_str(SNAPSHOT).unwrap();
            assert!(apply(&mut snapshot, &[change], 50.0).full, "{why} needs the vault's snapshot");
        }
        let mut snapshot: Value = serde_json::from_str(SNAPSHOT).unwrap();
        let filed = apply(&mut snapshot, &[written("Projects/filed.md", json!({ "kind": "task", "status": "backlog" }), "## Goal\n")], 50.0);
        assert_eq!(filed.bodies, [("Projects/filed.md".to_string(), "## Goal\n".to_string())], "a new note's body is kept once the snapshot has it");
        let mut snapshot: Value = serde_json::from_str(SNAPSHOT).unwrap();
        let quiet = apply(&mut snapshot, &[json!({ "key": "Resources/gone.md", "action": "delete", "note": null })], 50.0);
        assert!(!quiet.snapshot && !quiet.full && snapshot["etag"] == "e1", "a note the board never had changes nothing");
    }

    #[test]
    fn a_reset_takes_a_snapshot_and_a_batch_is_applied_with_its_backlog_bodies_and_the_cursor_kept() {
        let fake = Fake::new("watch-handle");
        let state = fake.state();
        let cli = capable_cli(&fake);
        fake.answer("snapshot---json", SNAPSHOT);
        assert_eq!(handle(&cli, &state, r#"{"cursor":"c1","reset":true,"more":false,"changes":[]}"#), Ok("c1".to_string()));
        assert_eq!(fake.calls(), ["snapshot --json", "read Projects/later.md --body"], "a reset reads the snapshot and fills the backlog bodies once");
        let line = json!({ "cursor": "c2", "reset": false, "more": false, "changes": [written("Projects/later.md", json!({ "kind": "task", "status": "backlog", "priority": "low", "title": "Later" }), "## Goal\n\nShip.\n")] });
        assert_eq!(handle(&cli, &state, &line.to_string()), Ok("c2".to_string()));
        assert_eq!(fake.calls().len(), 2, "a batch of known notes asks the vault nothing");
        let index = crate::vault::Index::of_state(&state).unwrap();
        assert_eq!(index.status_of("Projects/later.md").as_deref(), Some("backlog"));
        assert_eq!(crate::backlog::vault_body(Some(&index), &state, "Projects/later.md").as_deref(), Some("## Goal\n\nShip.\n"), "the backlog reads the body the batch carried");
        let unchanged = std::fs::metadata(crate::vault_snapshot::file(&state)).unwrap().modified().unwrap();
        assert_eq!(handle(&cli, &state, r#"{"cursor":"c3","reset":false,"more":false,"changes":[{"key":"Resources/x.md","action":"delete","note":null}]}"#), Ok("c3".to_string()));
        assert_eq!(std::fs::metadata(crate::vault_snapshot::file(&state)).unwrap().modified().unwrap(), unchanged, "a batch that changes nothing kept writes nothing");
        assert!(handle(&cli, &state, r#"{"reset":false}"#).is_err());
    }

    #[test]
    fn the_cursor_survives_a_restart_and_the_child_is_started_again_with_backoff() {
        let fake = Fake::new("watch-supervise");
        let state = fake.state();
        let cli = capable_cli(&fake);
        fake.answer("snapshot---json", SNAPSHOT);
        fake.answer("watch---json", r#"{"cursor":"c7","reset":true,"more":false,"changes":[]}"#);
        let rounds = Cell::new(0);
        let pauses = RefCell::new(Vec::new());
        supervise(&state, || Some(cli.clone()), || rounds.get() >= 3, |seconds| { pauses.borrow_mut().push(seconds); rounds.set(rounds.get() + 1) });
        let watches: Vec<String> = fake.calls().into_iter().filter(|call| call.starts_with("watch")).collect();
        assert_eq!(watches, ["watch --json --body", "watch --json --body --since c7", "watch --json --body --since c7"], "one child at a time, each from the kept cursor");
        assert_eq!(*pauses.borrow(), [1, 2, 4], "a child that keeps exiting is started again later each time");
        assert_eq!(kept_cursor(&state, &cli).as_deref(), Some("c7"));
        assert!(!watching(&state), "no child runs once the supervisor stops");
        let other = Cli { command: vec!["other".to_string()], ..cli.clone() };
        assert_eq!(kept_cursor(&state, &other), None, "a cursor is kept for the command that gave it");
    }

    #[test]
    fn a_vault_without_the_watch_is_never_followed_and_keeps_its_polling() {
        let fake = Fake::new("watch-without");
        let state = fake.state();
        let released = Cli { capabilities: Some(["tasks", "snapshot", "read"].map(str::to_string).to_vec()), ..fake.cli.clone() };
        assert!(!capable(&released) && !capable(&fake.cli), "a vault states the watch, it is never assumed");
        let rounds = Cell::new(0);
        supervise(&state, || Some(released.clone()), || rounds.get() >= 1, |seconds| { assert_eq!(seconds, UNABLE_PAUSE); rounds.set(1) });
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
        fake.answer("snapshot---json", SNAPSHOT);
        crate::vault_snapshot::tick_with(&released, &state, 1000).unwrap();
        assert_eq!(fake.calls(), ["snapshot --json"], "the routine polls as before");
    }

    #[test]
    fn while_the_keeper_watches_neither_the_routine_nor_the_board_asks_the_vault() {
        let fake = Fake::new("watch-board");
        let state = fake.state();
        let cli = capable_cli(&fake);
        fake.answer("snapshot---json", SNAPSHOT);
        crate::vault_snapshot::refresh(&cli, &state, 1000).unwrap();
        let mut sleeper = Command::new("sleep").arg("30").spawn().unwrap();
        record(&state, &cli, Some("c1"), Some(sleeper.id()));
        assert!(watching(&state) && live(&state, &cli));
        crate::vault_snapshot::tick_with(&cli, &state, 2000).unwrap();
        crate::backlog::fetch_vault_bodies_in(&cli, &state, || crate::vault::Index::of_state(&state));
        assert_eq!(fake.calls(), ["snapshot --json"], "the kept files are current: no snapshot, no body read");
        let _ = sleeper.kill();
        let _ = sleeper.wait();
        assert!(!watching(&state), "a child that is gone is no watch");
        crate::backlog::fetch_vault_bodies_in(&cli, &state, || crate::vault::Index::of_state(&state));
        assert_eq!(fake.calls().last().map(String::as_str), Some("read Projects/later.md --body"), "without the watch the board reads the body itself");
    }

    #[test]
    fn a_leftover_child_is_stopped_before_a_new_one_starts() {
        let fake = Fake::new("watch-leftover");
        let state = fake.state();
        let script = fake.dir.join("mindin");
        crate::executable::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        let mut leftover = Command::new(&script).args(["watch", "--json", "--body"]).process_group(0).spawn().unwrap();
        record(&state, &fake.cli, Some("c1"), Some(leftover.id()));
        stop_leftover(&state);
        let waited = Instant::now();
        let mut ended = None;
        while ended.is_none() && waited.elapsed() < Duration::from_secs(5) {
            ended = leftover.try_wait().unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = leftover.kill();
        let _ = leftover.wait();
        assert!(ended.is_some(), "the leftover watch is stopped");
        assert_eq!(read(&state)["child"], Value::Null);
        assert_eq!(kept_cursor(&state, &fake.cli).as_deref(), Some("c1"), "its cursor is kept for the next child");
    }

    #[test]
    fn openings_and_log_lines_are_cut_as_the_vault_cuts_them() {
        assert_eq!(opening("# Title\n\nFirst line\nsecond line\n\n## Next\n"), "First line second line");
        assert_eq!(opening("Para\n# Heading\n"), "Para");
        let long = "x".repeat(700);
        assert_eq!(opening(&long).chars().count(), OPENING_MAX);
        assert!(opening(&long).ends_with('…'));
        assert_eq!(log_lines("* [2026-11-12]   spaced  \n- [2026-1-12] short\n- [abcd-ef-gh] words").len(), 1);
        let many: String = (0..60).map(|day| format!("- [2026-11-{:02}] line {day}\n", day % 28 + 1)).collect();
        let kept = log_lines(&many);
        assert_eq!((kept.len(), kept[0]["text"].as_str()), (LOG_LINES, Some("line 10")), "the last 50 lines");
    }
}
