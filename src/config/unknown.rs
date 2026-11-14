use toml::Table;

use crate::sh::Result;

pub const UNKNOWN_FIX: &str = "a newer kebbie may know it; kebbie update";

static QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn report_unknown_keys_quietly() {
    QUIET.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(super) fn warn_once<T>(value: T, unknown: &[String]) -> T {
    static WARNED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    if QUIET.load(std::sync::atomic::Ordering::Relaxed) {
        return value;
    }
    let mut warned = WARNED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for line in unknown.iter().filter(|line| !warned.contains(line)).cloned().collect::<Vec<_>>() {
        eprintln!("kebbie: {line} (ignored: {UNKNOWN_FIX}, or fix the spelling)");
        warned.push(line);
    }
    value
}

pub(super) const MACHINE_KEYS: [&str; 21] = ["board", "worktree", "task", "review", "investigate", "notify", "vault", "update", "restart", "keeper", "recover", "machine", "hub", "keys", "ci", "tests", "comments", "routines", "report", "issues", "lead"];
pub(super) const REPO_KEYS: [&str; 10] = ["actions", "worktree", "task", "review", "investigate", "ci", "tests", "comments", "docs", "routines"];
const SECTION_KEYS: [(&str, &[&str]); 21] = [
    ("board", &["groups", "backlog"]),
    ("board.backlog", &["sources", "ongoing", "template"]),
    ("update", &["channel", "agents"]),
    ("restart", &["idle_minutes", "auto"]),
    ("keeper", &["enabled"]),
    ("recover", &["enabled"]),
    ("report", &["signals", "text", "enabled", "inbox_token"]),
    ("issues", &["propose"]),
    ("lead", &["profile"]),
    ("machine", &["name"]),
    ("hub", &["url", "commands", "trust", "auto_approve", "approve_from"]),
    ("worktree", &["setup_skill", "setup_command", "dev_server", "dev_worker", "dev_url", "dev_route", "stage_on_main", "server_name", "worker_name", "clean_on_release"]),
    ("task", &["rules_skill", "test_commands", "also_noticed", "permission_mode", "profile", "sandbox", "steps", "require_accept"]),
    ("review", &["model", "auto", "profile", "second"]),
    ("investigate", &["profile"]),
    ("notify", &["command"]),
    ("vault", &["command"]),
    ("ci", &["watch", "deploy"]),
    ("tests", &["per_commit", "before_push", "full_command"]),
    ("comments", &["watch", "watch_days"]),
    ("docs", &["required"]),
];

fn unknown_in(table: &Table, prefix: &str, known: &[&str]) -> Vec<String> {
    table.keys().filter(|key| !known.contains(&key.as_str())).map(|key| format!("{prefix}{key}: unknown key; known keys here: {}", known.join(", "))).collect()
}

pub fn unknown_keys(table: &Table, prefix: &str, known: &[&str]) -> Result<()> {
    unknown_in(table, prefix, known).into_iter().next().map_or(Ok(()), Err)
}

// A key a settings parser reads but SECTION_KEYS lacks is warned about while still read, so add every new key here too.
fn unknown_settings(table: &Table, top: &[&str]) -> Vec<String> {
    let sections = SECTION_KEYS.iter().filter_map(|(path, known)| path.split('.').try_fold(table, |table, key| table.get(key)?.as_table()).map(|section| unknown_in(section, &format!("{path}."), known)));
    unknown_in(table, "", top).into_iter().chain(sections.flatten()).collect()
}

pub(super) fn unknown_in_file(text: &str, source: &str, top: &[&str]) -> Vec<String> {
    text.parse::<Table>().map(|table| unknown_settings(&table, top).into_iter().map(|line| format!("{source}: {line}")).collect()).unwrap_or_default()
}
