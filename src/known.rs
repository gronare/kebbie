use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::messages::{self, Lock};
use crate::peers::Session;
use crate::sh::Result;
use crate::status::LEAD_LABEL;

const FROM: &str = "kebbie";
const ROLE_CAP: usize = 6000;
const DROPPED_CHARS: usize = 160;

// What a session's conversation has been given of kebbie: its role (without the peer list), the skills by description and the commands by their usage.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub kebbie: String,
    pub role: String,
    pub skills: BTreeMap<String, String>,
    pub commands: BTreeMap<String, String>,
}

// started is the kebbie the session's process runs with, None when it began before kebbie kept this; known is what it has been told since.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub started: Option<String>,
    pub known: Snapshot,
}

impl Record {
    fn to_json(&self) -> Value {
        let known = &self.known;
        json!({ "started": self.started, "known": { "kebbie": known.kebbie, "role": known.role, "skills": known.skills, "commands": known.commands } })
    }

    fn from_json(value: &Value) -> Option<Record> {
        let known = &value["known"];
        let map = |value: &Value| value.as_object().map(|entries| entries.iter().filter_map(|(name, text)| Some((name.clone(), text.as_str()?.to_string()))).collect());
        Some(Record {
            started: value["started"].as_str().map(str::to_string),
            known: Snapshot { kebbie: known["kebbie"].as_str()?.to_string(), role: known["role"].as_str()?.to_string(), skills: map(&known["skills"])?, commands: map(&known["commands"])? },
        })
    }
}

pub fn this_kebbie() -> String {
    crate::version(env!("KEBBIE_BUILT_FROM"))
}

// Each command in kebbie's usage, by its words up to its first argument, with its whole entry.
pub fn commands(usage: &str) -> BTreeMap<String, String> {
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in usage.lines() {
        if let Some(entry) = line.strip_prefix("  kebbie ") {
            let name: Vec<&str> = std::iter::once("kebbie").chain(entry.split("  ").next().unwrap_or_default().split_whitespace().take_while(|word| !word.starts_with(['<', '[', '-', '"']))).collect();
            let name = name.join(" ");
            found.insert(name.clone(), line.trim().to_string());
            current = Some(name);
        } else if line.starts_with("    ") && let Some(entry) = current.as_ref().and_then(|name| found.get_mut(name)) {
            entry.push(' ');
            entry.push_str(line.trim());
        } else {
            current = None;
        }
    }
    found.into_iter().map(|(name, entry)| (name, entry.split_whitespace().collect::<Vec<_>>().join(" "))).collect()
}

pub fn skills(root: &Path) -> BTreeMap<String, String> {
    let Ok(entries) = std::fs::read_dir(root.join("skills")) else { return BTreeMap::new() };
    entries.flatten().filter_map(|entry| {
        let text = std::fs::read_to_string(entry.path().join("SKILL.md")).ok()?;
        let front = text.strip_prefix("---\n")?.split("\n---").next()?;
        let description = front.lines().find_map(|line| line.strip_prefix("description:")).unwrap_or_default().trim();
        Some((format!("kebbie:{}", entry.file_name().to_string_lossy()), description.to_string()))
    }).collect()
}

fn snapshot(role: &str) -> Snapshot {
    Snapshot {
        kebbie: this_kebbie(),
        role: role.to_string(),
        skills: crate::lead::install_root().map(|root| skills(&root)).unwrap_or_default(),
        commands: commands(crate::USAGE),
    }
}

fn paragraphs(text: &str) -> Vec<&str> {
    text.split("\n\n").map(str::trim).filter(|paragraph| !paragraph.is_empty()).collect()
}

fn short(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= DROPPED_CHARS { line } else { format!("{}…", line.chars().take(DROPPED_CHARS).collect::<String>()) }
}

fn added(old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> Vec<String> {
    new.iter().filter(|(name, _)| !old.contains_key(*name)).map(|(_, entry)| format!("- {entry}")).collect()
}

fn changed(old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> Vec<String> {
    new.iter().filter(|(name, entry)| old.get(*name).is_some_and(|before| before != *entry)).map(|(_, entry)| format!("- {entry}")).collect()
}

fn removed(old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> Vec<String> {
    let gone: Vec<&str> = old.keys().filter(|name| !new.contains_key(*name)).map(String::as_str).collect();
    if gone.is_empty() { Vec::new() } else { vec![gone.join(", ")] }
}

fn role_changes(old: &str, new: &str) -> (Vec<String>, Vec<String>) {
    let (before, after) = (paragraphs(old), paragraphs(new));
    let fresh: Vec<&str> = after.iter().copied().filter(|paragraph| !before.contains(paragraph)).collect();
    let mut shown = Vec::new();
    let mut length = 0;
    for paragraph in &fresh {
        if length + paragraph.len() > ROLE_CAP && !shown.is_empty() {
            break;
        }
        length += paragraph.len();
        shown.push(paragraph.to_string());
    }
    if shown.len() < fresh.len() {
        shown.push(format!("(and {} more; read your role file for them)", fresh.len() - shown.len()));
    }
    let dropped = before.iter().filter(|paragraph| !after.contains(paragraph)).map(|paragraph| format!("- {}", short(paragraph))).collect();
    (shown, dropped)
}

// What changed for a session between what it knows and what is installed, in full; None when nothing it works from changed.
pub fn changes(old: &Snapshot, new: &Snapshot) -> Option<String> {
    let named = |map: &BTreeMap<String, String>| map.iter().map(|(name, description)| (name.clone(), format!("{name}: {description}"))).collect::<BTreeMap<_, _>>();
    let (old_skills, new_skills) = (named(&old.skills), named(&new.skills));
    let (role, dropped) = role_changes(&old.role, &new.role);
    let sections = [
        ("New commands:\n", added(&old.commands, &new.commands), "\n"),
        ("Changed commands:\n", changed(&old.commands, &new.commands), "\n"),
        ("Removed commands:\n", removed(&old.commands, &new.commands), "\n"),
        ("New skills:\n", added(&old_skills, &new_skills), "\n"),
        ("Changed skills:\n", changed(&old_skills, &new_skills), "\n"),
        ("Removed skills:\n", removed(&old_skills, &new_skills), "\n"),
        ("New or changed in your role:\n\n", role, "\n\n"),
        ("Dropped from your role (each replaced by the text above or gone):\n", dropped, "\n"),
    ];
    let shown: Vec<String> = sections.into_iter().filter(|(_, lines, _)| !lines.is_empty()).map(|(title, lines, gap)| format!("{title}{}", lines.join(gap))).collect();
    (!shown.is_empty()).then(|| shown.join("\n\n"))
}

fn since(record: Option<&Record>) -> String {
    record.and_then(|record| record.started.clone()).unwrap_or_else(|| "an earlier kebbie than kebbie records".to_string())
}

// The message to a running session after an update: what changed since what it knows, and where its whole role is now.
pub fn update_message(record: Option<&Record>, new: &Snapshot, role_file: &Path) -> Option<String> {
    let read = format!("Your role as it is now is in {}: read it, and work from it rather than from the role you started with.", role_file.display());
    match record {
        None => Some(format!("kebbie was updated to {} while you ran, and kebbie has no record of the version you started on, so your role and skills may be older than what is installed. {read} `{} --help` lists the commands. The skills now: {}.", new.kebbie, crate::peers::kebbie_command().unwrap_or_else(|_| "kebbie".to_string()), new.skills.keys().cloned().collect::<Vec<_>>().join(", "))),
        Some(record) => changes(&record.known, new).map(|changes| format!("kebbie was updated from {} to {} while you ran. What changed for you:\n\n{changes}\n\n{read}", record.known.kebbie, new.kebbie)),
    }
}

// The message to a session kebbie resumed: its role was replaced under a conversation that began with another.
pub fn resumed_message(record: Option<&Record>, new: &Snapshot) -> Option<String> {
    let replaced = format!("You were resumed with the role of {}, which replaces the one this conversation began with ({}). Work from your role as it is now, not from habits earlier in this conversation.", new.kebbie, since(record));
    match record {
        None => Some(replaced),
        Some(record) if record.started.as_deref() == Some(new.kebbie.as_str()) && record.known == *new => None,
        Some(record) => Some(match changes(&record.known, new) {
            Some(changes) => format!("{replaced} What is new since what you were told:\n\n{changes}"),
            None => format!("{replaced} kebbie told you earlier what changed in it."),
        }),
    }
}

// A line for the session's prompt hook while it runs an older kebbie than is installed.
pub fn stale_line(record: Option<&Record>, installed: &str, role_file: &Path, restart: &str) -> Option<String> {
    let record = record?;
    if record.started.as_deref() == Some(installed) {
        return None;
    }
    let told = if record.known.kebbie == installed { format!("kebbie has sent you what changed for you, and your role as it is now is in {}", role_file.display()) } else { "What changed for you reaches you as a message from kebbie".to_string() };
    Some(format!("kebbie: you started on {}; {installed} is installed. {told}; {restart}.", since(Some(record))))
}

fn file_in(dir: &Path, address: &str) -> PathBuf {
    dir.join(format!("{address}.known.json"))
}

pub fn load_in(dir: &Path, address: &str) -> Option<Record> {
    std::fs::read_to_string(file_in(dir, address)).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).as_ref().and_then(Record::from_json)
}

fn save_in(dir: &Path, address: &str, record: &Record) -> Result<()> {
    crate::state::write(&file_in(dir, address), record.to_json().to_string())
}

// kebbie started or resumed a session with this role; a resumed one is told, once it is free, what its new role brings.
pub fn began(address: &str, role: &str, resumed: bool) {
    let done = crate::lead::roles_dir().and_then(|dir| began_in(&dir, address, &snapshot(role), resumed, &mut |text| {
        if address == LEAD_LABEL { messages::log_for_lead(text, Some(messages::NOTICE_KIND)) } else { messages::hold_for(FROM, address, text, None, Some(messages::NOTICE_KIND)) }
    }));
    if let Err(error) = done {
        eprintln!("Could not record which kebbie {address} runs: {error}");
    }
}

pub fn began_in(dir: &Path, address: &str, now: &Snapshot, resumed: bool, hold: &mut dyn FnMut(&str) -> Result<()>) -> Result<()> {
    let old = load_in(dir, address);
    if resumed && let Some(text) = resumed_message(old.as_ref(), now) {
        hold(&text)?;
    }
    save_in(dir, address, &Record { started: Some(now.kebbie.clone()), known: now.clone() })
}

// The role kebbie would give this session now, written to its role file: the file, and the role without its peer list.
fn role_now(session: &Session, cwd: Option<&Path>) -> Result<Option<(PathBuf, String)>> {
    if session.address == LEAD_LABEL {
        return crate::lead::role_file().map(Some);
    }
    cwd.map_or(Ok(None), crate::tasks::task_role_now)
}

// Each running session that knows an older kebbie than this one is told what changed for it, held while it is mid-turn.
pub fn announce() -> Result<Vec<String>> {
    let dir = crate::lead::roles_dir()?;
    let sessions = crate::peers::sessions()?;
    if !sessions.iter().any(|session| due(&dir, session, &this_kebbie())) || !installed_is_me() {
        return Ok(Vec::new());
    }
    let agents = crate::herdr::agents()?;
    let cwd_of = |session: &Session| agents.iter().find(|agent| agent["pane_id"].as_str() == session.pane.as_deref()).and_then(|agent| agent["cwd"].as_str()).map(PathBuf::from);
    announce_in(&dir, &sessions, &this_kebbie(), &mut |session| {
        let Some((file, role)) = role_now(session, cwd_of(session).as_deref())? else { return Ok(None) };
        Ok(Some((file, snapshot(&role))))
    }, &mut |session, text| {
        let _lock = Lock::take(true)?;
        messages::notice_locked(session, text, &sessions)
    })
}

// The kebbie on PATH, which a keeper left running on an older binary is not.
pub fn installed() -> String {
    crate::peers::kebbie_command().ok().and_then(|kebbie| crate::sh::run(&kebbie, &crate::BUILT_VERSION, None).ok()).map(|version| version.trim().to_string()).filter(|version| !version.is_empty()).unwrap_or_else(this_kebbie)
}

// A keeper left running on an older binary than is installed must not tell sessions its own, older kebbie.
fn installed_is_me() -> bool {
    crate::peers::kebbie_command().ok().and_then(|kebbie| crate::sh::run(&kebbie, &crate::BUILT_VERSION, None).ok()).is_some_and(|version| version.trim() == this_kebbie())
}

fn due(dir: &Path, session: &Session, installed: &str) -> bool {
    session.pane.is_some() && load_in(dir, &session.address).is_none_or(|record| record.known.kebbie != installed)
}

type Now<'a> = dyn FnMut(&Session) -> Result<Option<(PathBuf, Snapshot)>> + 'a;
type Send<'a> = dyn FnMut(&Session, &str) -> Result<Vec<String>> + 'a;

pub fn announce_in(dir: &Path, sessions: &[Session], installed: &str, now: &mut Now, send: &mut Send) -> Result<Vec<String>> {
    let mut said = Vec::new();
    for session in sessions.iter().filter(|session| due(dir, session, installed)) {
        let Some((file, current)) = now(session)? else { continue };
        let record = load_in(dir, &session.address);
        if let Some(text) = update_message(record.as_ref(), &current, &file) {
            said.extend(send(session, &text)?);
        }
        save_in(dir, &session.address, &Record { started: record.and_then(|record| record.started), known: current })?;
    }
    Ok(said)
}

pub fn lead_restart(auto: bool, key: &str) -> String {
    if auto {
        format!("kebbie restarts you on it, resuming this conversation, once the user is out of your pane and you are idle, or at once when they press {key} on the board and choose to resume")
    } else {
        format!("restart.auto is off, so nothing restarts you on its own: the user presses {key} on the board and chooses to resume this conversation on it or start fresh")
    }
}

pub fn stale_for(address: &str) -> Option<String> {
    let dir = crate::lead::roles_dir().ok()?;
    let auto = crate::config::restart_settings().map_or(true, |(auto, _)| auto);
    let restart = if address == LEAD_LABEL { lead_restart(auto, &crate::keys::map().label(crate::keys::Act::RestartLead)) } else { "a restart brings it in".to_string() };
    stale_line(load_in(&dir, address).as_ref(), &this_kebbie(), &dir.join(format!("{address}.md")), &restart)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(kebbie: &str, role: &str, skills: &[(&str, &str)], commands: &[(&str, &str)]) -> Snapshot {
        let map = |pairs: &[(&str, &str)]| pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();
        Snapshot { kebbie: kebbie.to_string(), role: role.to_string(), skills: map(skills), commands: map(commands) }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-known-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_usage_gives_each_command_with_its_whole_entry() {
        let usage = "kebbie: herding\n\n  kebbie tell <to> <text>         message another session\n                                  held while it waits\n  kebbie issue show|retry <id>    print one\n  kebbie lead                     go to the lead\n";
        let found = commands(usage);
        assert_eq!(found["kebbie tell"], "kebbie tell <to> <text> message another session held while it waits");
        assert_eq!(found["kebbie issue show|retry"], "kebbie issue show|retry <id> print one");
        assert!(found.contains_key("kebbie lead"));
        let real = commands(crate::USAGE);
        assert!(real.contains_key("kebbie issue add") && real.contains_key("kebbie restart") && real.contains_key("kebbie update"), "{:?}", real.keys());
    }

    #[test]
    fn the_skills_are_read_with_their_descriptions() {
        let dir = scratch("skills");
        std::fs::create_dir_all(dir.join("skills/propose-change")).unwrap();
        std::fs::write(dir.join("skills/propose-change/SKILL.md"), "---\nname: propose-change\ndescription: Propose saving a kebbie issue.\n---\n\nbody\ndescription: not this\n").unwrap();
        assert_eq!(skills(&dir), BTreeMap::from([("kebbie:propose-change".to_string(), "Propose saving a kebbie issue.".to_string())]));
        assert!(skills(Path::new(env!("CARGO_MANIFEST_DIR"))).contains_key("kebbie:propose-change"));
    }

    #[test]
    fn an_update_tells_the_new_commands_skills_and_role_text_in_full() {
        let old = snap("kebbie 0.22.4", "Report each turn.\n\nAsk the user.", &[("kebbie:review", "Review a task.")], &[("kebbie tell", "kebbie tell <to> <text> message"), ("kebbie old", "kebbie old gone")]);
        let new = snap("kebbie 0.24.0", "Report each turn.\n\nAsk through kebbie ask.\n\nFile what you notice.", &[("kebbie:review", "Review a task."), ("kebbie:propose-change", "Propose saving a kebbie issue.")], &[("kebbie tell", "kebbie tell <to> <text> message, held"), ("kebbie issue add", "kebbie issue add --kind bug save one")]);
        let text = changes(&old, &new).unwrap();
        assert!(text.contains("New commands:\n- kebbie issue add --kind bug save one"), "{text}");
        assert!(text.contains("Changed commands:\n- kebbie tell <to> <text> message, held"), "{text}");
        assert!(text.contains("Removed commands:\nkebbie old"), "{text}");
        assert!(text.contains("New skills:\n- kebbie:propose-change: Propose saving a kebbie issue."), "{text}");
        assert!(text.contains("New or changed in your role:\n\nAsk through kebbie ask.\n\nFile what you notice."), "{text}");
        assert!(text.contains("Dropped from your role (each replaced by the text above or gone):\n- Ask the user."), "{text}");
        assert!(!text.contains("Report each turn"), "unchanged text is not repeated: {text}");
        assert_eq!(changes(&new, &snap("kebbie 0.24.1", &new.role, &[("kebbie:review", "Review a task."), ("kebbie:propose-change", "Propose saving a kebbie issue.")], &[("kebbie tell", "kebbie tell <to> <text> message, held"), ("kebbie issue add", "kebbie issue add --kind bug save one")])), None, "a new version with nothing new for the session says nothing");
    }

    #[test]
    fn a_long_role_change_is_capped_and_points_to_the_role_file() {
        let paragraph = "x".repeat(2500);
        let role = (0..5).map(|index| format!("{index}{paragraph}")).collect::<Vec<_>>().join("\n\n");
        let text = changes(&snap("a", "", &[], &[]), &snap("b", &role, &[], &[])).unwrap();
        assert!(text.contains("(and 3 more; read your role file for them)"), "{}", &text[text.len() - 200..]);
        let message = update_message(Some(&Record { started: Some("a".to_string()), known: snap("a", "", &[], &[]) }), &snap("b", &role, &[], &[]), Path::new("/state/roles/lead.md")).unwrap();
        assert!(message.starts_with("kebbie was updated from a to b while you ran. What changed for you:") && message.ends_with("Your role as it is now is in /state/roles/lead.md: read it, and work from it rather than from the role you started with."), "{message}");
    }

    #[test]
    fn a_session_without_a_record_is_told_to_read_its_role_and_the_skills() {
        let message = update_message(None, &snap("kebbie 0.24.0", "role", &[("kebbie:propose-change", "p"), ("kebbie:review", "r")], &[]), Path::new("/r/lead.md")).unwrap();
        assert!(message.contains("no record of the version you started on") && message.contains("/r/lead.md") && message.contains("kebbie:propose-change, kebbie:review"), "{message}");
    }

    #[test]
    fn a_resumed_session_is_told_its_role_was_replaced_and_what_is_new() {
        let dir = scratch("resume");
        let old = snap("kebbie 0.22.4", "Ask the user.", &[], &[]);
        let new = snap("kebbie 0.24.0", "Ask through kebbie ask.", &[("kebbie:propose-change", "p")], &[]);
        let mut held = Vec::new();
        began_in(&dir, "lead", &old, false, &mut |text| { held.push(text.to_string()); Ok(()) }).unwrap();
        assert!(held.is_empty(), "a fresh start is told nothing");
        assert_eq!(load_in(&dir, "lead").unwrap().started.as_deref(), Some("kebbie 0.22.4"), "the start version is recorded");
        began_in(&dir, "lead", &new, true, &mut |text| { held.push(text.to_string()); Ok(()) }).unwrap();
        assert_eq!(held.len(), 1);
        assert!(held[0].starts_with("You were resumed with the role of kebbie 0.24.0, which replaces the one this conversation began with (kebbie 0.22.4).") && held[0].contains("What is new since what you were told:") && held[0].contains("Ask through kebbie ask.") && held[0].contains("kebbie:propose-change"), "{}", held[0]);
        assert_eq!(load_in(&dir, "lead").unwrap(), Record { started: Some("kebbie 0.24.0".to_string()), known: new.clone() });
        began_in(&dir, "lead", &new, true, &mut |text| { held.push(text.to_string()); Ok(()) }).unwrap();
        assert_eq!(held.len(), 1, "a resume on the same kebbie and role is told nothing");
    }

    #[test]
    fn a_resumed_session_already_told_hears_only_that_its_role_was_replaced() {
        let new = snap("kebbie 0.24.0", "Ask through kebbie ask.", &[], &[]);
        let told = Record { started: Some("kebbie 0.22.4".to_string()), known: new.clone() };
        let text = resumed_message(Some(&told), &new).unwrap();
        assert!(text.ends_with("kebbie told you earlier what changed in it."), "{text}");
        assert!(resumed_message(None, &new).unwrap().contains("(an earlier kebbie than kebbie records)"));
    }

    fn session(address: &str, pane: Option<&str>) -> Session {
        Session { address: address.to_string(), task: address.split_once('@').map(|(task, _)| task.to_string()), pane: pane.map(str::to_string), status: pane.map(|_| "working".to_string()) }
    }

    #[test]
    fn an_update_tells_each_running_session_once_what_changed_since_it_started() {
        let dir = scratch("announce");
        let old = snap("kebbie 0.22.4", "Ask the user.", &[], &[("kebbie tell", "kebbie tell <to>")]);
        let new = snap("kebbie 0.24.0", "Ask the user.", &[("kebbie:propose-change", "Propose saving a kebbie issue.")], &[("kebbie tell", "kebbie tell <to>"), ("kebbie issue add", "kebbie issue add --kind bug")]);
        began_in(&dir, "lead", &old, false, &mut |_| Ok(())).unwrap();
        began_in(&dir, "billing@app", &new, false, &mut |_| Ok(())).unwrap();
        let sessions = [session("lead", Some("p1")), session("billing@app", Some("p2")), session("search@app", Some("p3")), session("gone@app", None)];
        let mut sent: Vec<(String, String)> = Vec::new();
        let run = |sent: &mut Vec<(String, String)>| announce_in(&dir, &sessions, "kebbie 0.24.0", &mut |session| Ok(Some((PathBuf::from(format!("/r/{}.md", session.address)), new.clone()))), &mut |session, text| {
            sent.push((session.address.clone(), text.to_string()));
            Ok(vec![format!("Told {}.", session.address)])
        }).unwrap();
        assert_eq!(run(&mut sent), ["Told lead.", "Told search@app."], "a session on the installed kebbie and one without a pane are not told");
        assert!(sent[0].1.starts_with("kebbie was updated from kebbie 0.22.4 to kebbie 0.24.0 while you ran.") && sent[0].1.contains("- kebbie issue add --kind bug") && sent[0].1.contains("- kebbie:propose-change: Propose saving a kebbie issue.") && sent[0].1.contains("/r/lead.md"), "{}", sent[0].1);
        assert!(sent[1].1.contains("no record of the version you started on"), "a session from before kebbie kept records is told to read its role: {}", sent[1].1);
        let lead = load_in(&dir, "lead").unwrap();
        assert_eq!((lead.started.as_deref(), &lead.known), (Some("kebbie 0.22.4"), &new), "the start version stays, what it knows moves on");
        assert_eq!(load_in(&dir, "search@app").unwrap().started, None);
        assert!(run(&mut sent).is_empty(), "each is told once");
    }

    #[test]
    fn the_prompt_hook_line_shows_the_session_its_own_staleness() {
        let role = Path::new("/r/lead.md");
        let old = snap("kebbie 0.22.4", "", &[], &[]);
        let fresh = Record { started: Some("kebbie 0.24.0".to_string()), known: snap("kebbie 0.24.0", "", &[], &[]) };
        assert_eq!(stale_line(Some(&fresh), "kebbie 0.24.0", role, "a restart brings it in"), None);
        assert_eq!(stale_line(None, "kebbie 0.24.0", role, "a restart brings it in"), None);
        let untold = Record { started: Some("kebbie 0.22.4".to_string()), known: old };
        assert_eq!(stale_line(Some(&untold), "kebbie 0.24.0", role, "a restart brings it in").unwrap(), "kebbie: you started on kebbie 0.22.4; kebbie 0.24.0 is installed. What changed for you reaches you as a message from kebbie; a restart brings it in.");
        let told = Record { known: fresh.known.clone(), ..untold };
        assert_eq!(stale_line(Some(&told), "kebbie 0.24.0", role, "a restart brings it in").unwrap(), "kebbie: you started on kebbie 0.22.4; kebbie 0.24.0 is installed. kebbie has sent you what changed for you, and your role as it is now is in /r/lead.md; a restart brings it in.");
        let lead = |auto: bool| stale_line(Some(&told), "kebbie 0.24.0", role, &lead_restart(auto, "L")).unwrap();
        assert!(lead(true).ends_with("; kebbie restarts you on it, resuming this conversation, once the user is out of your pane and you are idle, or at once when they press L on the board and choose to resume."), "{}", lead(true));
        assert!(lead(false).ends_with("; restart.auto is off, so nothing restarts you on its own: the user presses L on the board and chooses to resume this conversation on it or start fresh."), "{}", lead(false));
        assert!(!lead(true).contains("kebbie lead --restart") && !lead(false).contains("another pane"));
    }
}
