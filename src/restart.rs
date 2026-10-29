use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::agent::Agent;
use crate::config;
use crate::herdr;
use crate::lead;
use crate::messages::{self, Lock, USER_LABEL};
use crate::questions;
use crate::sh::{self, Result};
use crate::tasks;
use crate::typing::Draft;

pub const QUESTION_KIND: &str = "restart";
const FROM: &str = "kebbie";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Routine { min_idle: u64 },
    Update,
    Manual { all: bool },
}

impl Mode {
    fn follows_auto(self) -> bool {
        !matches!(self, Mode::Manual { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Answer {
    Yes,
    No,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Fresh,
    Restart,
    Busy,
    Focused,
    Quiet,
    Ask,
    Asked,
    Keep,
    Manual,
    Drafted,
    Skip(String),
}

impl Verdict {
    pub fn stale(&self) -> bool {
        !matches!(self, Verdict::Fresh | Verdict::Skip(_))
    }

    pub fn words(&self, auto: bool) -> String {
        match self {
            Verdict::Fresh => "up to date".to_string(),
            Verdict::Restart => "restarted".to_string(),
            Verdict::Busy | Verdict::Quiet if auto => "restarts when idle".to_string(),
            Verdict::Busy | Verdict::Quiet => "busy: run kebbie restart when it waits".to_string(),
            Verdict::Focused if auto => "you are in it: restarts when you leave it".to_string(),
            Verdict::Focused => "you are in it: run kebbie restart from another pane".to_string(),
            Verdict::Ask | Verdict::Asked => "waiting for your answer: background work running".to_string(),
            Verdict::Keep => "kept until its background work is done, then restarts when idle".to_string(),
            Verdict::Manual => "stale: run kebbie restart when you want it".to_string(),
            Verdict::Drafted if auto => "its input box holds text or cannot be read: restarts once it is empty".to_string(),
            Verdict::Drafted => "its input box holds text or cannot be read: run kebbie restart once it is empty".to_string(),
            Verdict::Skip(reason) => format!("skipped: {reason}"),
        }
    }
}

pub struct Facts {
    pub stale: bool,
    pub waiting: bool,
    pub focused: bool,
    pub idle_for: u64,
    pub asked: bool,
    pub answer: Option<Answer>,
}

// Nothing mid-turn or in the focused pane is ever restarted; background work is only cut off on the user's yes.
pub fn decide(mode: Mode, auto: bool, facts: &Facts, background: impl FnOnce() -> bool) -> Verdict {
    if !facts.stale && mode != (Mode::Manual { all: true }) {
        return Verdict::Fresh;
    }
    if mode.follows_auto() && !auto {
        return Verdict::Manual;
    }
    if !facts.waiting {
        return Verdict::Busy;
    }
    if facts.focused {
        return Verdict::Focused;
    }
    if let Mode::Routine { min_idle } = mode
        && facts.idle_for < min_idle
    {
        return Verdict::Quiet;
    }
    if !background() {
        return Verdict::Restart;
    }
    match (facts.answer, facts.asked) {
        (Some(Answer::Yes), _) => Verdict::Restart,
        (Some(Answer::No), _) => Verdict::Keep,
        (None, true) => Verdict::Asked,
        (None, false) => Verdict::Ask,
    }
}

// The settings file a session's command line names with --settings.
pub fn settings_file(argv: &[String]) -> Option<PathBuf> {
    argv.windows(2).find(|pair| pair[0] == "--settings").map(|pair| PathBuf::from(&pair[1])).or_else(|| argv.iter().find_map(|arg| arg.strip_prefix("--settings=")).map(PathBuf::from))
}

fn program(command: &str) -> String {
    let command = command.trim_start();
    match command.strip_prefix('\'') {
        Some(rest) => rest.split('\'').next().unwrap_or_default().to_string(),
        None => command.split_whitespace().next().unwrap_or_default().to_string(),
    }
}

// The kebbie a settings file's hooks run that is no kebbie on this machine, as the kelpie binary the rename removed, so evidence and gate hooks fail without a word.
pub fn gone_hooks(settings: &Value) -> Vec<String> {
    let commands = settings["hooks"].as_object().into_iter().flatten().flat_map(|(_, entries)| entries.as_array().into_iter().flatten())
        .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten()).filter_map(|hook| hook["command"].as_str());
    let mut gone: Vec<String> = commands.map(program).filter(|program| {
        let name = base(program);
        (name == "kelpie" || name == "kebbie") && (name == "kelpie" || !Path::new(program).is_file())
    }).collect();
    gone.sort();
    gone.dedup();
    gone
}

pub fn hooks_gone_in(argv: &[String]) -> bool {
    settings_file(argv).and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| serde_json::from_str::<Value>(&text).ok()).is_some_and(|settings| !gone_hooks(&settings).is_empty())
}

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub const HELD: &str = "stale, held: ";
pub const NO_ROLE: &str = "it runs without its task role";
pub const HOOKS_GONE: &str = "its hooks run a kebbie that is gone";
pub const OUTSIDE_LEAD_DIR: &str = "it runs outside the lead's folder";

// A lead started in another folder than the lead's own works on state kebbie no longer reads.
pub fn outside_lead_dir(cwd: Option<&Path>, own: &Path) -> bool {
    let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    cwd.is_some_and(|cwd| !real(cwd).starts_with(real(own)))
}

// ps prints the elapsed time as [[dd-]hh:]mm:ss.
pub fn parse_etime(text: &str) -> Option<u64> {
    let text = text.trim();
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, text),
    };
    let parts: Vec<u64> = clock.split(':').map(|part| part.parse().ok()).collect::<Option<_>>()?;
    let seconds = match parts.as_slice() {
        [minutes, seconds] => minutes * 60 + seconds,
        [hours, minutes, seconds] => hours * 3600 + minutes * 60 + seconds,
        _ => return None,
    };
    Some(days * 86_400 + seconds)
}

pub fn iso_seconds(text: &str) -> Option<u64> {
    let day = crate::backlog::epoch_day(text)?;
    let clock = text.get(11..19)?;
    let mut parts = clock.split(':').map(|part| part.parse::<i64>().ok());
    let (hours, minutes, seconds) = (parts.next()??, parts.next()??, parts.next()??);
    u64::try_from(day * 86_400 + hours * 3600 + minutes * 60 + seconds).ok()
}

fn mtime(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|elapsed| elapsed.as_secs())
}

#[cfg(target_os = "macos")]
fn exe_of(pid: u32) -> Option<PathBuf> {
    let mut buffer = vec![0u8; 4096];
    let length = unsafe { libc::proc_pidpath(pid as libc::c_int, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    (length > 0).then(|| PathBuf::from(String::from_utf8_lossy(&buffer[..length as usize]).to_string()))
}

#[cfg(not(target_os = "macos"))]
fn exe_of(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[derive(Clone, Debug, Default)]
pub struct Plugin {
    pub id: String,
    pub updated: u64,
    pub project: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub claude: Option<PathBuf>,
    pub claude_changed: Option<u64>,
    pub plugins: Vec<Plugin>,
    pub mcp: HashMap<String, u64>,
    pub updated: Option<u64>,
}

pub fn plugins(installed: &Value) -> Vec<Plugin> {
    installed["plugins"].as_object().into_iter().flatten().flat_map(|(id, entries)| {
        entries.as_array().into_iter().flatten().filter_map(move |entry| Some(Plugin {
            id: id.clone(),
            updated: entry["lastUpdated"].as_str().or(entry["installedAt"].as_str()).and_then(iso_seconds)?,
            project: entry["projectPath"].as_str().map(PathBuf::from),
        }))
    }).collect()
}

fn on_path(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|dir| dir.join(program)).find(|file| file.is_file()))
}

pub(crate) fn restart_dir(state: &Path) -> Result<PathBuf> {
    let dir = state.join("restart");
    crate::state::private_dir(&dir)?;
    Ok(dir)
}

pub fn mark_updated(state: &Path, now: u64) -> Result<()> {
    crate::state::write(&restart_dir(state)?.join("updated"), now.to_string())
}

fn claude_json() -> Option<PathBuf> {
    std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|_| std::env::var("HOME").map(PathBuf::from)).ok().map(|dir| dir.join(".claude.json"))
}

fn fingerprint(value: &Value) -> String {
    sha1_smol::Sha1::from(value.to_string()).digest().to_string()
}

// ~/.claude.json is rewritten all the time, so its MCP servers are fingerprinted and kebbie keeps when each fingerprint last changed; a key first seen in a file already watched counts as changed now.
pub fn observe_mcp(record: &Path, claude: &Value, now: u64) -> HashMap<String, u64> {
    let watched = record.exists();
    let mut seen: Map<String, Value> = std::fs::read_to_string(record).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|value| value.as_object().cloned()).unwrap_or_default();
    let mut current: Vec<(String, String)> = Vec::new();
    if claude.get("mcpServers").is_some() {
        current.push(("user".to_string(), fingerprint(&claude["mcpServers"])));
    }
    for (path, project) in claude["projects"].as_object().into_iter().flatten() {
        if project["mcpServers"].as_object().is_some_and(|servers| !servers.is_empty()) {
            current.push((format!("project:{path}"), fingerprint(&project["mcpServers"])));
        }
    }
    let mut changed = false;
    for (key, hash) in &current {
        let before = seen.get(key).and_then(|entry| entry["hash"].as_str()).map(str::to_string);
        if before.as_deref() != Some(hash) {
            let at = if watched || before.is_some() { now } else { 0 };
            seen.insert(key.clone(), json!({ "hash": hash, "changed": at }));
            changed = true;
        }
    }
    for (key, entry) in seen.iter_mut() {
        if !current.iter().any(|(name, _)| name == key) && entry["hash"] != "" {
            *entry = json!({ "hash": "", "changed": now });
            changed = true;
        }
    }
    if changed || !watched {
        let _ = crate::state::write(record, Value::Object(seen.clone()).to_string());
    }
    seen.into_iter().filter_map(|(key, entry)| Some((key, entry["changed"].as_u64()?))).collect()
}

impl Changes {
    pub fn gather(state: &Path, now: u64) -> Changes {
        let claude = on_path(&crate::setup::claude_bin()).and_then(|path| path.canonicalize().ok());
        let home = crate::agent::claude::home();
        let read = |path: Option<PathBuf>| path.and_then(|path| std::fs::read_to_string(path).ok()).and_then(|text| serde_json::from_str::<Value>(&text).ok()).unwrap_or_default();
        let installed = read(home.map(|home| home.join("plugins/installed_plugins.json")));
        let dir = restart_dir(state).ok();
        let mcp = dir.as_ref().map(|dir| observe_mcp(&dir.join("mcp.json"), &read(claude_json()), now)).unwrap_or_default();
        Changes {
            claude_changed: claude.as_deref().and_then(mtime),
            claude,
            plugins: plugins(&installed),
            mcp,
            updated: dir.and_then(|dir| std::fs::read_to_string(dir.join("updated")).ok()).and_then(|text| text.trim().parse().ok()),
        }
    }

    pub fn reasons(&self, exe: Option<&Path>, started: u64, cwd: Option<&Path>) -> Vec<String> {
        let mut reasons = Vec::new();
        let same_install = |exe: &Path, now: &Path| exe.parent() == now.parent() || exe.file_name() == now.file_name();
        // The keeper's PATH is the one it started with, so a Claude on it older than the session's own is the keeper's stale view, not the session's.
        let older = |now: &Path, exe: &Path| mtime(now).zip(mtime(exe)).is_some_and(|(now, exe)| now < exe);
        match (exe.map(|exe| exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf())), &self.claude) {
            (Some(exe), Some(now)) if same_install(&exe, now) && &exe != now && !older(now, &exe) => {
                let name = |path: &Path| path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                if exe.parent() == now.parent() { reasons.push(format!("Claude Code {} → {}", name(&exe), name(now))) } else { reasons.push("a newer Claude Code".to_string()) }
            }
            _ if self.claude_changed.is_some_and(|at| at > started) => reasons.push("a newer Claude Code".to_string()),
            _ => {}
        }
        let applies = |project: &Option<PathBuf>| project.as_ref().is_none_or(|project| cwd.is_some_and(|cwd| cwd.starts_with(project)));
        let mut names: Vec<&str> = self.plugins.iter().filter(|plugin| plugin.updated > started && applies(&plugin.project)).map(|plugin| plugin.id.split('@').next().unwrap_or(&plugin.id)).collect();
        names.dedup();
        if !names.is_empty() {
            reasons.push(format!("plugin {} updated", names.join(", ")));
        }
        let project_keys: Vec<String> = cwd.into_iter().flat_map(Path::ancestors).map(|folder| format!("project:{}", folder.display())).collect();
        if self.mcp.iter().any(|(key, at)| *at > started && (key == "user" || project_keys.contains(key))) {
            reasons.push("MCP servers changed".to_string());
        }
        if self.updated.is_some_and(|at| at > started) {
            reasons.push(KEBBIE_UPDATE.to_string());
        }
        reasons
    }
}

fn blocks<'a>(text: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    text.match_indices(&open).filter_map(|(at, _)| {
        let rest = &text[at + open.len()..];
        rest.find(&close).map(|end| &rest[..end])
    }).collect()
}

fn notices(record: &Value) -> Vec<String> {
    let mut texts: Vec<String> = [&record["content"], &record["attachment"]["prompt"]].into_iter().filter_map(|value| value.as_str().map(str::to_string)).collect();
    if record["type"] == "user" {
        match &record["message"]["content"] {
            Value::String(text) => texts.push(text.clone()),
            Value::Array(parts) => texts.extend(parts.iter().filter(|part| part["type"] == "text").filter_map(|part| part["text"].as_str().map(str::to_string))),
            _ => {}
        }
    }
    texts
}

// Background work a session launched since `since` and that has not finished: a backgrounded Bash (backgroundTaskId), an async agent (status async_launched), a Monitor (taskId, until its timeout); each ends with a <task-notification> carrying a <status>, or a TaskStop result naming it.
pub fn background_running(lines: impl Iterator<Item = String>, since: u64, now: u64) -> Vec<String> {
    let mut running: HashMap<String, Option<u64>> = HashMap::new();
    let mut agents: HashSet<String> = HashSet::new();
    for line in lines {
        if !(line.contains("toolUseResult") || line.contains("<task-notification>") || line.contains("\"SendMessage\"")) || line.contains("\"type\":\"api-request") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else { continue };
        let at = record["timestamp"].as_str().and_then(iso_seconds).unwrap_or(0);
        for text in notices(&record) {
            for block in blocks(&text, "task-notification") {
                if block.contains("<status>") {
                    for id in blocks(block, "task-id") {
                        running.remove(id.trim());
                    }
                }
            }
        }
        let result = &record["toolUseResult"];
        if let Some(id) = result["task_id"].as_str().filter(|_| result["message"].as_str().is_some_and(|message| message.starts_with("Successfully stopped"))) {
            running.remove(id);
        }
        if at < since {
            continue;
        }
        if let Some(id) = result["backgroundTaskId"].as_str() {
            running.insert(id.to_string(), None);
        }
        if result["status"] == "async_launched" && let Some(id) = result["agentId"].as_str() {
            running.insert(id.to_string(), None);
            agents.insert(id.to_string());
        }
        if let Some(id) = result["taskId"].as_str() {
            let until = (result["persistent"] != true).then(|| at + result["timeoutMs"].as_u64().unwrap_or(0) / 1000);
            running.insert(id.to_string(), until);
        }
        if record["type"] == "assistant" {
            for part in record["message"]["content"].as_array().into_iter().flatten() {
                if part["type"] == "tool_use" && part["name"] == "SendMessage" && let Some(to) = part["input"]["to"].as_str().filter(|to| agents.contains(*to)) {
                    running.insert(to.to_string(), None);
                }
            }
        }
    }
    let mut ids: Vec<String> = running.into_iter().filter(|(_, until)| until.is_none_or(|until| until > now)).map(|(id, _)| id).collect();
    ids.sort();
    ids
}

pub fn transcript(home: &Path, cwd: Option<&Path>, session: &str) -> Option<PathBuf> {
    let file = format!("{session}.jsonl");
    cwd.map(|cwd| crate::agent::claude::transcripts_dir(home, cwd).join(&file)).filter(|path| path.is_file())
        .or_else(|| std::fs::read_dir(home.join("projects")).ok()?.flatten().map(|dir| dir.path().join(&file)).find(|path| path.is_file()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub pane: String,
    pub name: Option<String>,
    pub label: String,
    pub session: Option<String>,
    pub waiting: bool,
    pub focused: bool,
    pub cwd: Option<PathBuf>,
    pub workspace: Option<String>,
    pub agent: Agent,
}

impl Pane {
    pub fn from_agent(agent: &Value) -> Option<Pane> {
        let kind = Agent::of(agent)?;
        let pane = agent["pane_id"].as_str()?.to_string();
        let name = agent["name"].as_str().filter(|name| !name.is_empty()).map(str::to_string);
        let label = name.clone().or_else(|| agent["display_agent"].as_str().filter(|title| !title.is_empty()).map(|title| format!("{title} ({pane})"))).unwrap_or_else(|| pane.clone());
        Some(Pane {
            label,
            name,
            session: agent["agent_session"]["value"].as_str().filter(|id| !id.is_empty()).map(str::to_string),
            waiting: ["idle", "done"].contains(&agent["agent_status"].as_str().unwrap_or_default()) && agent["interactive_ready"] == true,
            focused: agent["focused"] == true,
            cwd: agent["cwd"].as_str().map(PathBuf::from),
            workspace: agent["workspace_id"].as_str().map(str::to_string),
            agent: kind,
            pane,
        })
    }
}

pub struct Process {
    pub pid: u32,
    pub argv: Option<Vec<String>>,
    pub cwd: Option<PathBuf>,
}

// The pane's Claude leads its process group; its MCP servers and tools share the group.
pub fn claude_process(info: &Value) -> Option<Process> {
    let processes = info["foreground_processes"].as_array()?;
    let leader = info["foreground_process_group_id"].as_u64();
    let claude = processes.iter().find(|process| process["pid"].as_u64().is_some() && process["pid"].as_u64() == leader && base(process["argv0"].as_str().unwrap_or_default()) != "sh")
        .or_else(|| processes.iter().find(|process| base(process["argv0"].as_str().unwrap_or_default()) == "claude"))?;
    Some(Process {
        pid: u32::try_from(claude["pid"].as_u64()?).ok()?,
        argv: claude["argv"].as_array().map(|argv| argv.iter().filter_map(|arg| arg.as_str().map(str::to_string)).collect()).filter(|argv: &Vec<String>| !argv.is_empty()),
        cwd: claude["cwd"].as_str().map(PathBuf::from),
    })
}

pub(crate) fn started(pid: u32, now: u64) -> Option<u64> {
    let elapsed = sh::run("ps", &["-o", "etime=", "-p", &pid.to_string()], None).ok()?;
    parse_etime(&elapsed).map(|elapsed| now.saturating_sub(elapsed))
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

fn asked_file(state: &Path) -> Result<PathBuf> {
    Ok(restart_dir(state)?.join("asked.json"))
}

fn load_asked(state: &Path) -> Map<String, Value> {
    asked_file(state).ok().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|value| value.as_object().cloned()).unwrap_or_default()
}

const COOLDOWN: u64 = 30 * 60;

fn restarted_file(state: &Path) -> Result<PathBuf> {
    Ok(restart_dir(state)?.join("restarted.json"))
}

fn load_restarted(state: &Path) -> Map<String, Value> {
    restarted_file(state).ok().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|value| value.as_object().cloned()).unwrap_or_default()
}

pub const KEBBIE_UPDATE: &str = "kebbie update";

// The reasons a second restart cannot satisfy when the first left them standing: the pane's PATH starts the same Claude Code, or the restart did not give a task its role or working hooks.
fn sticky(entry: &Value) -> Vec<&str> {
    entry["reasons"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|reason| reason.starts_with("Claude Code ") || *reason == "a newer Claude Code" || *reason == NO_ROLE || *reason == HOOKS_GONE).collect()
}

// A session kebbie recorded starting on the installed kebbie already runs it, whatever the update's time says.
pub fn on_installed(reasons: &mut Vec<String>, started: Option<&str>, installed: &str) {
    if started == Some(installed) {
        reasons.retain(|reason| reason != KEBBIE_UPDATE);
    }
}

// Why a stale session must not be restarted again: kebbie restarted it lately, or a restart left it where it was, on the same Claude Code or on the kebbie it was restarted to leave; `now` is the entry this restart would record.
pub fn held_back(last: Option<&Value>, now: &Value, manual: bool) -> Option<String> {
    let last = last?;
    let when = last["at"].as_u64()?;
    let at = now["at"].as_u64().unwrap_or(when);
    let reasons: Vec<&str> = now["reasons"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let same_claude = !sticky(now).is_empty() && last["exe"] == now["exe"] && sticky(last) == sticky(now);
    let same_kebbie = !manual && reasons.contains(&KEBBIE_UPDATE) && !now["kebbie"].is_null() && last["kebbie"] == now["kebbie"] && last["started"] == now["started"];
    let held = |reason: &&str| (same_claude && sticky(now).contains(reason)) || (same_kebbie && *reason == KEBBIE_UPDATE);
    if !reasons.is_empty() && reasons.iter().all(held) {
        let left = sticky(now);
        return Some(if same_claude && left.iter().any(|reason| reason.starts_with("Claude Code ") || *reason == "a newer Claude Code") {
            format!("a restart brought back the same Claude Code ({}), so the pane starts that one; restarting again cannot update it", now["exe"].as_str().unwrap_or("?"))
        } else if same_claude {
            format!("{} after a restart already; restarting again would not mend it", left.join(" and "))
        } else {
            format!("a restart onto {} left it on {}; restarting again would not move it", now["kebbie"].as_str().unwrap_or("?"), now["started"].as_str().unwrap_or("an older kebbie"))
        });
    }
    (!manual && at.saturating_sub(when) < COOLDOWN).then(|| format!("restarted {} min ago; the next restart waits {} min", at.saturating_sub(when) / 60, COOLDOWN / 60))
}

fn save_asked(state: &Path, asked: &Map<String, Value>) -> Result<()> {
    crate::state::write(&asked_file(state)?, Value::Object(asked.clone()).to_string())
}

// One question per session and per process: a restart starts a new process, and a staleness after it is asked anew.
pub fn asked_for<'a>(asked: &'a Map<String, Value>, session: &str, pid: u32) -> Option<&'a Value> {
    asked.get(session).filter(|entry| entry["pid"].as_u64() == Some(u64::from(pid)))
}

fn answer_of(entry: Option<&Value>) -> Option<Answer> {
    match entry?["answer"].as_str()? {
        "yes" => Some(Answer::Yes),
        "no" => Some(Answer::No),
        _ => None,
    }
}

pub fn answer(session: &str, yes: bool) -> Result<String> {
    let state = lead::state_dir()?;
    let mut asked = load_asked(&state);
    let entry = asked.get_mut(session).ok_or("That restart is no longer waiting")?;
    entry["answer"] = json!(if yes { "yes" } else { "no" });
    save_asked(&state, &asked)?;
    if !yes {
        return crate::notice::Notice::new(crate::notice::Tone::Done, "Kept running").next("It restarts once its background work is done and it is idle.").ok();
    }
    let lines = run(Mode::Manual { all: false }, &mut Memory::default(), Some(session))?;
    Ok(lines.first().map_or(crate::notice::Notice::new(crate::notice::Tone::Done, "Restarted").to_string(), |line| crate::notice::Notice::new(crate::notice::Tone::Done, "Restarted").detail(&line.label, &line.said).to_string()))
}

fn close_question(id: &str) {
    if let Ok(Some(_lock)) = Lock::take(true) {
        let messages = messages::load();
        if questions::open_now(&messages).iter().any(|question| question.id == id) {
            let _ = messages::append(&json!({ "id": id, "at": messages::now(), "state": "answered" }));
        }
    }
}

fn ask(label: &str, session: &str, cwd: &Path) -> Result<String> {
    let text = format!("{label}: restart anyway? background work is running and would be cut off");
    let asked = questions::Asked {
        text: text.clone(),
        options: vec![("Yes".to_string(), "restart it now, resuming its conversation".to_string()), ("No".to_string(), "keep it until its background work is done, then restart it when idle".to_string())],
        recommend: 2,
        comment: Vec::new(),
    };
    let id = messages::new_id("restart");
    let mut record = questions::record(&id, messages::now(), FROM, &asked, None, None);
    record["kind"] = json!(QUESTION_KIND);
    record["about"] = json!(session);
    {
        let _lock = Lock::take(true)?;
        messages::append(&record)?;
    }
    crate::notify::send(cwd, &crate::notify::Notice::from(crate::notify::Event::Question, FROM, text));
    Ok(id)
}

pub fn tell_board(text: &str) {
    let _ = messages::append(&json!({ "id": messages::new_id("restart"), "at": messages::now(), "from": FROM, "to": USER_LABEL, "text": text, "state": "logged" }));
}

pub(crate) struct SessionLock(PathBuf);

impl SessionLock {
    pub(crate) fn take(state: &Path, session: &str) -> Result<SessionLock> {
        let path = restart_dir(state)?.join(format!("{session}.lock"));
        if mtime(&path).is_some_and(|at| now().saturating_sub(at) > 300) {
            let _ = std::fs::remove_file(&path);
        }
        std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|_| "another kebbie is restarting it".to_string())?;
        Ok(SessionLock(path))
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn same_folder(one: &Path, other: &Path) -> bool {
    one == other || one.canonicalize().ok().is_some_and(|one| other.canonicalize().ok() == Some(one))
}

fn wait_for_claude(pane: &str) -> Result<()> {
    for _ in 0..120 {
        if herdr::agents()?.iter().any(|agent| agent["pane_id"] == pane && Agent::of(agent).is_some() && agent["interactive_ready"] == true) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err(format!("Claude did not come back in pane {pane}; see what the pane shows"))
}

fn perform(pane: &Pane, own: Option<&Path>, args: &[String], lead: bool) -> Result<tasks::Started> {
    if lead {
        return match sh::attempt(&crate::peers::kebbie_command()?, &["lead", "--restart", "--anyway"], None)? {
            Ok(_) => Ok(tasks::Started::Ready),
            Err(stderr) if stderr.is_empty() => Err("kebbie lead --restart failed".to_string()),
            Err(stderr) => Err(stderr),
        };
    }
    let role = own.or(pane.cwd.as_deref()).map(tasks::task_role).transpose()?.flatten();
    let args = role.map_or_else(|| args.to_vec(), |(settings, role)| pane.agent.with_role(args, &settings, &role));
    pane.agent.end(&pane.pane)?;
    // A Claude started in a worktree from a pane in the main checkout resumes only from the worktree, where its conversation is kept.
    if let Some(own) = own.filter(|own| pane.cwd.as_deref().is_none_or(|shell| !same_folder(own, shell))) {
        tasks::wait_for_prompt(&pane.pane)?;
        herdr::run_in_pane(&pane.pane, &format!("cd {}", sh::quote(&own.to_string_lossy())))?;
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match &pane.name {
        Some(name) => pane.agent.start(name, &pane.pane, &refs, pane.cwd.as_deref().is_some_and(|cwd| pane.agent.trusted_by_user(cwd))),
        None => {
            tasks::wait_for_prompt(&pane.pane)?;
            let line = std::iter::once(pane.agent.kind().to_string()).chain(args.iter().map(|arg| sh::quote(arg))).collect::<Vec<_>>().join(" ");
            herdr::run_in_pane(&pane.pane, &line)?;
            wait_for_claude(&pane.pane).map(|()| tasks::Started::Ready)
        }
    }
}

fn restarted_said(label: &str, reasons: &[String], started: &tasks::Started) -> String {
    match started.asks() {
        None => format!("Restarted {label} with claude --resume, as it ran older code ({}).", reasons.join(", ")),
        Some(asks) => format!("Restarted {label}, as it ran older code ({}); Claude there asks {asks}. Answer it there, and it resumes its conversation.", reasons.join(", ")),
    }
}

#[derive(Default)]
pub struct Memory {
    active: HashMap<String, u64>,
    background: HashMap<PathBuf, (u64, u64, u64, Vec<String>)>,
}

impl Memory {
    fn idle_for(&mut self, session: &str, quiet: bool, transcript: Option<&Path>, now: u64) -> u64 {
        if !quiet {
            self.active.insert(session.to_string(), now);
            return 0;
        }
        let since = *self.active.entry(session.to_string()).or_insert_with(|| transcript.and_then(mtime).unwrap_or(now).min(now));
        now.saturating_sub(since)
    }

    fn background(&mut self, path: &Path, since: u64, now: u64) -> Vec<String> {
        let key = (std::fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0), mtime(path).unwrap_or(0), since);
        if let Some((length, modified, from, ids)) = self.background.get(path)
            && (*length, *modified, *from) == key
        {
            return ids.clone();
        }
        let ids = std::fs::File::open(path).map(|file| background_running(std::io::BufReader::new(file).lines().map_while(std::result::Result::ok), since, now)).unwrap_or_default();
        self.background.insert(path.to_path_buf(), (key.0, key.1, key.2, ids.clone()));
        ids
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Line {
    pub label: String,
    pub said: String,
    pub stale: bool,
    #[serde(default)]
    pub reasons: Vec<String>,
}

// The lead is the session the user talks to, so once they are out of its pane it waits a minute, not the idle minutes.
pub const LEAD_IDLE: u64 = 60;

pub fn mode_for(mode: Mode, lead: bool) -> Mode {
    match mode {
        Mode::Routine { min_idle } if lead => Mode::Routine { min_idle: min_idle.min(LEAD_IDLE) },
        other => other,
    }
}

// A restart on its own waits on a box kebbie cannot read, as it may hold a draft; one asked for goes ahead.
pub fn with_draft(mode: Mode, verdict: Verdict, draft: impl FnOnce() -> Draft) -> Verdict {
    if verdict != Verdict::Restart {
        return verdict;
    }
    match draft() {
        Draft::Typed(_) => Verdict::Drafted,
        Draft::Unreadable if mode.follows_auto() => Verdict::Drafted,
        Draft::Empty | Draft::Unreadable => Verdict::Restart,
    }
}

pub fn background_in(pane: &str, session: &str) -> Vec<String> {
    let Some(process) = herdr::process_info(pane).ok().as_ref().and_then(claude_process) else { return Vec::new() };
    let at = now();
    let Some(start) = started(process.pid, at) else { return Vec::new() };
    let found = crate::agent::claude::home().and_then(|home| transcript(&home, process.cwd.as_deref(), session));
    found.map(|path| Memory::default().background(&path, start, at)).unwrap_or_default()
}

fn refusal(error: &str) -> Option<&str> {
    error.strip_prefix(lead::REFUSED)
}

// A refusal leaves no record, so it holds nothing back and the next round tries again; whatever ended or started a Claude is recorded.
pub fn note_attempt(restarted: &mut Map<String, Value>, session: &str, entry: Value, done: &Result<tasks::Started>) -> bool {
    if done.as_ref().err().is_some_and(|error| refusal(error).is_some()) {
        return false;
    }
    restarted.insert(session.to_string(), entry);
    true
}

pub fn told_failure(label: &str, error: &str) -> Option<String> {
    refusal(error).is_none().then(|| format!("Restarting {label} failed: {error}"))
}

pub fn failed_said(error: &str) -> String {
    match refusal(error) {
        Some(why) => format!("not restarted yet: {}", why.trim_end_matches('.')),
        None => format!("restart failed: {error}"),
    }
}

pub fn run(mode: Mode, memory: &mut Memory, only: Option<&str>) -> Result<Vec<Line>> {
    run_counted(mode, memory, only, &mut |_, _| {})
}

fn ours(pane: &Pane, process: Option<&Process>, lead_space: Option<&str>) -> bool {
    (lead_space.is_some() && pane.workspace.as_deref() == lead_space) || process.and_then(|process| process.cwd.as_deref()).or(pane.cwd.as_deref()).and_then(tasks::task_at).is_some()
}

pub fn run_counted(mode: Mode, memory: &mut Memory, only: Option<&str>, counted: &mut dyn FnMut(usize, usize)) -> Result<Vec<Line>> {
    let (auto, idle_minutes) = config::restart_settings()?;
    let mode = match mode {
        Mode::Routine { .. } => Mode::Routine { min_idle: idle_minutes * 60 },
        other => other,
    };
    let state = lead::state_dir()?;
    let at = now();
    let changes = Changes::gather(&state, at);
    let lead_space = lead::lead_workspace(&herdr::workspaces()?);
    let home = crate::agent::claude::home();
    let mut asked = load_asked(&state);
    let mut restarted = load_restarted(&state);
    let mut live: HashMap<String, u32> = HashMap::new();
    let mut installed: Option<String> = None;
    let mut lines = Vec::new();
    let panes: Vec<(Pane, Option<Process>)> = herdr::agents()?.iter().filter_map(Pane::from_agent).filter(|pane| only.is_none_or(|only| pane.session.as_deref() == Some(only)))
        .map(|pane| {
            let process = (pane.agent == Agent::Claude).then(|| herdr::process_info(&pane.pane).ok().as_ref().and_then(claude_process)).flatten();
            (pane, process)
        })
        .filter(|(pane, process)| ours(pane, process.as_ref(), lead_space.as_deref()))
        .collect();
    let total = panes.len();
    for (index, (pane, process)) in panes.into_iter().enumerate() {
        counted(index, total);
        let line = |verdict: &Verdict| Line { label: pane.label.clone(), said: verdict.words(auto), stale: verdict.stale(), reasons: Vec::new() };
        if pane.agent != Agent::Claude {
            lines.push(line(&Verdict::Skip(format!("kebbie restarts Claude sessions; {} is left as it runs", pane.agent.name()))));
            continue;
        }
        let Some(session) = pane.session.clone() else {
            lines.push(line(&Verdict::Skip("its session id is unknown".to_string())));
            continue;
        };
        let Some(process) = process else {
            lines.push(line(&Verdict::Skip("its Claude process cannot be read".to_string())));
            continue;
        };
        live.insert(session.clone(), process.pid);
        let lead = lead_space.is_some() && pane.workspace == lead_space;
        let Some(start) = started(process.pid, at) else {
            lines.push(line(&Verdict::Skip("its start time cannot be read".to_string())));
            continue;
        };
        let cwd = process.cwd.clone().or_else(|| pane.cwd.clone());
        let exe = exe_of(process.pid);
        let mut reasons = changes.reasons(exe.as_deref(), start, cwd.as_deref());
        if process.argv.as_deref().is_some_and(|argv| pane.agent.lacks_role(argv)) && cwd.as_deref().and_then(tasks::task_at).is_some() {
            reasons.push(NO_ROLE.to_string());
        }
        if process.argv.as_deref().is_some_and(hooks_gone_in) {
            reasons.push(HOOKS_GONE.to_string());
        }
        if lead && lead::lead_dir().is_ok_and(|own| outside_lead_dir(cwd.as_deref(), &own)) {
            reasons.push(OUTSIDE_LEAD_DIR.to_string());
        }
        let (kebbie, started_on) = if reasons.iter().any(|reason| reason == KEBBIE_UPDATE) {
            let address = if lead { Some(crate::status::LEAD_LABEL.to_string()) } else { cwd.as_deref().and_then(tasks::task_at).and_then(|(repo, slot)| Some(crate::peers::address(slot.task.as_deref()?, &crate::repo::name_of(&repo)))) };
            let started_on = address.and_then(|address| crate::known::load_in(&crate::lead::roles_dir().ok()?, &address)).and_then(|record| record.started);
            let kebbie = installed.get_or_insert_with(crate::known::installed).clone();
            on_installed(&mut reasons, started_on.as_deref(), &kebbie);
            (Some(kebbie), started_on)
        } else {
            (None, None)
        };
        let record = json!({ "at": at, "exe": exe.as_deref().map(|exe| exe.to_string_lossy().to_string()), "reasons": reasons, "kebbie": kebbie, "started": started_on });
        if !reasons.is_empty() && let Some(why) = held_back(restarted.get(&session), &record, matches!(mode, Mode::Manual { .. })) {
            lines.push(Line { label: pane.label.clone(), said: format!("{HELD}{why}"), stale: lead, reasons: reasons.clone() });
            continue;
        }
        let args = match process.argv.as_deref().map(|argv| pane.agent.resume_args(argv, &session)) {
            None => Err("its command line cannot be read".to_string()),
            Some(args) => args,
        };
        let transcript = home.as_deref().and_then(|home| transcript(home, cwd.as_deref(), &session));
        let mode = mode_for(mode, lead);
        let entry = asked_for(&asked, &session, process.pid);
        let facts = Facts {
            stale: !reasons.is_empty(),
            waiting: pane.waiting,
            focused: pane.focused,
            idle_for: memory.idle_for(&session, pane.waiting && !pane.focused, transcript.as_deref(), at),
            asked: entry.is_some(),
            answer: answer_of(entry),
        };
        let verdict = match &args {
            Err(reason) if facts.stale || mode == (Mode::Manual { all: true }) => Verdict::Skip(reason.clone()),
            _ => match decide(mode, auto, &facts, || transcript.as_deref().is_some_and(|path| !memory.background(path, start, at).is_empty())) {
                Verdict::Restart if transcript.is_none() => Verdict::Skip("its conversation file is not found".to_string()),
                other => with_draft(mode, other, || crate::typing::draft_in(&pane.pane, pane.agent)),
            },
        };
        let verdict = match verdict {
            Verdict::Ask => match ask(&pane.label, &session, cwd.as_deref().unwrap_or(Path::new("/"))) {
                Ok(id) => {
                    asked.insert(session.clone(), json!({ "pid": process.pid, "question": id, "answer": null }));
                    Verdict::Asked
                }
                Err(error) => Verdict::Skip(format!("asking you failed: {error}")),
            },
            other => other,
        };
        if verdict != Verdict::Restart {
            lines.push(Line { reasons: if verdict.stale() { reasons.clone() } else { Vec::new() }, ..line(&verdict) });
            continue;
        }
        let done = SessionLock::take(&state, &session).and_then(|_lock| perform(&pane, process.cwd.as_deref(), args.as_deref().unwrap_or_default(), lead));
        if note_attempt(&mut restarted, &session, record, &done) {
            crate::state::write(&restarted_file(&state)?, Value::Object(restarted.clone()).to_string())?;
        }
        match done {
            Ok(started) => {
                memory.active.insert(session.clone(), at);
                live.remove(&session);
                if matches!(mode, Mode::Routine { .. }) || started.asks().is_some() {
                    tell_board(&restarted_said(&pane.label, &reasons, &started));
                }
                lines.push(match started.asks() {
                    None => Line { reasons: reasons.clone(), ..line(&Verdict::Restart) },
                    Some(asks) => Line { label: pane.label.clone(), said: format!("restarted; Claude there asks {asks}, answer it there"), stale: true, reasons: reasons.clone() },
                });
            }
            Err(error) => {
                if let Some(text) = told_failure(&pane.label, &error) {
                    tell_board(&text);
                }
                lines.push(Line { label: pane.label.clone(), said: failed_said(&error), stale: true, reasons: reasons.clone() });
            }
        }
    }
    counted(total, total);
    if only.is_none() {
        let gone: Vec<String> = asked.iter().filter(|(session, entry)| live.get(*session).map(|pid| u64::from(*pid)) != entry["pid"].as_u64()).map(|(session, _)| session.clone()).collect();
        for session in gone {
            if let Some(id) = asked.remove(&session).and_then(|entry| entry["question"].as_str().map(str::to_string)) {
                close_question(&id);
            }
        }
    } else if let Some(only) = only && !live.contains_key(only) && let Some(id) = asked.remove(only).and_then(|entry| entry["question"].as_str().map(str::to_string)) {
        close_question(&id);
    }
    keep_answers(&mut asked, &load_asked(&state));
    save_asked(&state, &asked)?;
    Ok(lines)
}

// The board answers in its own process while a run here goes through the sessions, so an answer saved meanwhile must survive this run's save.
pub fn keep_answers(asked: &mut Map<String, Value>, on_disk: &Map<String, Value>) {
    for (session, entry) in asked.iter_mut() {
        if let Some(saved) = on_disk.get(session).filter(|saved| entry["answer"].is_null() && !saved["answer"].is_null() && saved["pid"] == entry["pid"] && saved["question"] == entry["question"]) {
            entry["answer"] = saved["answer"].clone();
        }
    }
}

// kebbie restart says what each restart was for; an update's report leaves it out, as the update is the reason; a held line names its own.
pub fn with_reasons(lines: Vec<Line>) -> Vec<Line> {
    lines.into_iter().map(|line| match line.reasons.is_empty() || line.said.starts_with(HELD) {
        true => line,
        false => Line { said: format!("{}, as {}", line.said, line.reasons.join(", ")), ..line },
    }).collect()
}

pub fn report(lines: &[Line]) -> Vec<String> {
    let mut out = vec!["Claude sessions:".to_string()];
    if lines.is_empty() {
        out.push("  no Claude sessions running".to_string());
    }
    out.extend(lines.iter().map(|line| format!("  {}: {}", line.label, line.said)));
    out
}

pub fn now_cli(all: bool) -> Result<String> {
    Ok(report(&with_reasons(run(Mode::Manual { all }, &mut Memory::default(), None)?)).join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_lead_and_task_sessions_are_kebbies_to_restart() {
        let pane = |workspace: &str, cwd: &str| Pane::from_agent(&json!({ "agent": "claude", "pane_id": "p1", "workspace_id": workspace, "cwd": cwd, "agent_session": { "value": "s1" } }));
        let own = pane("w-lead", "/tmp/elsewhere").expect("a claude pane");
        assert!(ours(&own, None, Some("w-lead")), "the lead Space's Claude is kebbie's");
        let other = pane("w-other", "/tmp/elsewhere").expect("a claude pane");
        assert!(!ours(&other, None, Some("w-lead")), "a Claude of the user's own outside a task is left alone");
        assert!(!ours(&other, None, None), "with no lead Space nothing outside a task is kebbie's");
    }
    use crate::agent::claude::{lacks_role, resume_args, with_role};

    fn entry(at: u64, exe: &str, reasons: &[&str], kebbie: Option<&str>, started: Option<&str>) -> Value {
        json!({ "at": at, "exe": exe, "reasons": reasons, "kebbie": kebbie, "started": started })
    }

    #[test]
    fn a_session_whose_hooks_run_a_kebbie_that_is_gone_is_stale_until_a_restart_writes_its_settings_again() {
        let dir = crate::sh::test_dir("restart-hooks-gone");
        let kebbie = dir.join("kebbie");
        crate::executable::write(&kebbie, "#!/bin/sh\n").unwrap();
        let written = |program: &str| serde_json::from_str::<Value>(&crate::agent::claude::settings(program, "billing@app", Some("billing@app"), None, None)).unwrap();
        assert!(gone_hooks(&written(&kebbie.to_string_lossy())).is_empty());
        let kelpie = "/home/u/.local/share/kelpie/current/target/release/kelpie";
        assert_eq!(gone_hooks(&written(kelpie)), [kelpie], "kelpie's binary is gone after the rename, so its evidence and gate hooks fail silently");
        let missing = dir.join("gone/kebbie");
        assert_eq!(gone_hooks(&written(&missing.to_string_lossy())), [missing.to_string_lossy().to_string()]);
        let file = dir.join("billing@app.settings.json");
        std::fs::write(&file, written(kelpie).to_string()).unwrap();
        let argv = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        assert!(hooks_gone_in(&argv(&["claude", "--settings", &file.to_string_lossy(), "--resume", "x"])));
        assert!(hooks_gone_in(&argv(&["claude", &format!("--settings={}", file.display())])));
        assert!(!hooks_gone_in(&argv(&["claude", "--resume", "x"])));
        let source = include_str!("restart.rs");
        let run = &source[source.find("\npub fn run_counted(").unwrap()..];
        assert!(run.find("hooks_gone_in").unwrap() < run.find("held_back(").unwrap(), "the reason counts like any other, so a busy session restarts once idle");
        assert!(sticky(&json!({ "reasons": [HOOKS_GONE] })) == [HOOKS_GONE], "a restart that left the hooks broken is not repeated");
        let exe = "/v/2.1.310";
        let held = held_back(Some(&entry(1000, exe, &[HOOKS_GONE], None, None)), &entry(99_999, exe, &[HOOKS_GONE], None, None), true).unwrap();
        assert_eq!(held, "its hooks run a kebbie that is gone after a restart already; restarting again would not mend it", "the held line names its own reason, not Claude Code's version");
        let held = held_back(Some(&entry(1000, exe, &[NO_ROLE], None, None)), &entry(99_999, exe, &[NO_ROLE], None, None), true).unwrap();
        assert_eq!(held, "it runs without its task role after a restart already; restarting again would not mend it");
        let restarted = Line { label: "billing@app".to_string(), said: "restarted".to_string(), stale: false, reasons: vec![HOOKS_GONE.to_string()] };
        assert_eq!(report(&with_reasons(vec![restarted.clone()]))[1], "  billing@app: restarted, as its hooks run a kebbie that is gone", "kebbie restart says why it restarted a session");
        assert_eq!(report(&[restarted])[1], "  billing@app: restarted");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_session_restarted_lately_or_into_the_same_claude_is_held_back() {
        let reasons = ["a newer Claude Code"];
        let (old, newer) = ("/opt/devbox/claude-2.1.296/bin/claude", "/opt/devbox/claude-2.1.304/bin/claude");
        let last = entry(1000, old, &reasons, None, None);
        assert!(held_back(Some(&last), &entry(99_999, old, &reasons, None, None), true).unwrap().contains("same Claude Code"), "a restart that brought back the same Claude is never repeated, not even by hand");
        assert!(held_back(Some(&last), &entry(1000 + 60, newer, &reasons, None, None), false).unwrap().contains("restarted 1 min ago"), "the routine waits out the cooldown");
        assert_eq!(held_back(Some(&last), &entry(1000 + COOLDOWN, newer, &reasons, None, None), false), None);
        assert_eq!(held_back(Some(&last), &entry(1000 + 60, newer, &reasons, None, None), true), None, "kebbie restart by hand skips the cooldown");
        assert_eq!(held_back(None, &entry(1000, old, &reasons, None, None), false), None);
    }

    #[test]
    fn each_kebbie_update_restarts_a_session_again_on_the_same_claude_code() {
        let exe = "/Users/u/.local/share/claude/versions/2.1.310";
        let last = entry(1000, exe, &[KEBBIE_UPDATE, "plugin kebbie updated"], Some("kebbie 0.30.0"), Some("kebbie 0.29.0"));
        let next = entry(1000 + COOLDOWN, exe, &[KEBBIE_UPDATE, "plugin kebbie updated"], Some("kebbie 0.31.0"), Some("kebbie 0.30.0"));
        assert_eq!(held_back(Some(&last), &next, false), None, "the next kebbie update restarts it again");
        let before = json!({ "at": 1000, "exe": exe, "reasons": [KEBBIE_UPDATE, "plugin kebbie updated"] });
        assert_eq!(held_back(Some(&before), &entry(1000 + COOLDOWN, exe, &[KEBBIE_UPDATE, "plugin kebbie updated"], Some("kebbie 0.31.0"), Some("kebbie 0.30.0")), false), None, "a record from before kebbie kept versions holds nothing back");
        assert_eq!(held_back(Some(&entry(1000, exe, &["plugin vault updated"], None, None)), &entry(1000 + COOLDOWN, exe, &["plugin vault updated"], None, None), false), None, "a later plugin update restarts it again");
        let claude_too = entry(1000 + COOLDOWN, exe, &["a newer Claude Code", KEBBIE_UPDATE], Some("kebbie 0.31.0"), Some("kebbie 0.30.0"));
        assert_eq!(held_back(Some(&entry(1000, exe, &["a newer Claude Code"], None, None)), &claude_too, false), None, "a held Claude Code does not hold back the new kebbie");
    }

    #[test]
    fn a_restart_that_left_the_session_on_its_kebbie_is_not_repeated_by_the_routine() {
        let exe = "/v/2.1.310";
        let last = entry(1000, exe, &[KEBBIE_UPDATE], Some("kebbie 0.31.0"), Some("kebbie 0.30.0"));
        let again = entry(1000 + 10 * COOLDOWN, exe, &[KEBBIE_UPDATE], Some("kebbie 0.31.0"), Some("kebbie 0.30.0"));
        assert_eq!(held_back(Some(&last), &again, false).unwrap(), "a restart onto kebbie 0.31.0 left it on kebbie 0.30.0; restarting again would not move it");
        assert_eq!(held_back(Some(&last), &again, true), None, "kebbie restart by hand tries again");
        let mut reasons = vec![KEBBIE_UPDATE.to_string(), "plugin kebbie updated".to_string()];
        on_installed(&mut reasons, Some("kebbie 0.30.0"), "kebbie 0.31.0");
        assert_eq!(reasons, [KEBBIE_UPDATE, "plugin kebbie updated"], "a session on an older kebbie is stale for the update");
        on_installed(&mut reasons, Some("kebbie 0.31.0"), "kebbie 0.31.0");
        assert_eq!(reasons, ["plugin kebbie updated"], "a session that started on the installed kebbie is not");
        on_installed(&mut reasons, None, "kebbie 0.31.0");
        assert_eq!(reasons, ["plugin kebbie updated"]);
    }
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-restart-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn facts(stale: bool, waiting: bool, focused: bool, idle_for: u64) -> Facts {
        Facts { stale, waiting, focused, idle_for, asked: false, answer: None }
    }

    const ROUTINE: Mode = Mode::Routine { min_idle: 600 };

    #[test]
    fn a_stale_session_restarts_only_when_idle_unfocused_and_quiet_long_enough() {
        let none = || false;
        assert_eq!(decide(ROUTINE, true, &facts(true, true, false, 600), none), Verdict::Restart);
        assert_eq!(decide(ROUTINE, true, &facts(true, true, false, 599), none), Verdict::Quiet);
        assert_eq!(decide(ROUTINE, true, &facts(false, true, false, 9999), none), Verdict::Fresh);
        assert_eq!(decide(Mode::Update, true, &facts(true, true, false, 0), none), Verdict::Restart, "kebbie update does not wait the idle minutes");
        assert_eq!(decide(Mode::Manual { all: false }, true, &facts(false, true, false, 0), none), Verdict::Fresh);
        assert_eq!(decide(Mode::Manual { all: true }, true, &facts(false, true, false, 0), none), Verdict::Restart, "--all restarts a free session that is current");
    }

    #[test]
    fn a_focused_busy_or_background_working_session_is_never_restarted_without_a_yes() {
        let modes = [ROUTINE, Mode::Update, Mode::Manual { all: false }, Mode::Manual { all: true }];
        for mode in modes {
            assert_eq!(decide(mode, true, &facts(true, true, true, 99_999), || false), Verdict::Focused, "{mode:?}");
            assert_eq!(decide(mode, true, &facts(true, false, false, 99_999), || false), Verdict::Busy, "{mode:?}");
            assert_eq!(decide(mode, true, &facts(true, false, true, 99_999), || false), Verdict::Busy, "{mode:?}");
            assert_eq!(decide(mode, true, &facts(true, true, false, 99_999), || true), Verdict::Ask, "{mode:?}");
            let asked = Facts { asked: true, ..facts(true, true, false, 99_999) };
            assert_eq!(decide(mode, true, &asked, || true), Verdict::Asked, "{mode:?}: asked once, not every tick");
            let no = Facts { asked: true, answer: Some(Answer::No), ..facts(true, true, false, 99_999) };
            assert_eq!(decide(mode, true, &no, || true), Verdict::Keep, "{mode:?}");
            assert_eq!(decide(mode, true, &no, || false), Verdict::Restart, "{mode:?}: no keeps it only until its background work is done");
            let yes = Facts { asked: true, answer: Some(Answer::Yes), ..facts(true, true, false, 99_999) };
            assert_eq!(decide(mode, true, &yes, || true), Verdict::Restart, "{mode:?}");
            let yes_but_focused = Facts { focused: true, ..yes };
            assert_eq!(decide(mode, true, &yes_but_focused, || true), Verdict::Focused, "{mode:?}: a yes never reaches the pane you are in");
        }
    }

    #[test]
    fn with_auto_off_the_routine_and_update_leave_every_session_and_only_kebbie_restart_acts() {
        let mut looked = false;
        assert_eq!(decide(ROUTINE, false, &facts(true, true, false, 99_999), || { looked = true; true }), Verdict::Manual);
        assert!(!looked, "the routine never looks for background work, so it never asks");
        assert_eq!(decide(Mode::Update, false, &facts(true, true, false, 0), || false), Verdict::Manual);
        assert_eq!(Verdict::Manual.words(false), "stale: run kebbie restart when you want it");
        assert!(Verdict::Manual.stale(), "the board still marks it");
        assert_eq!(decide(Mode::Manual { all: false }, false, &facts(true, true, false, 0), || false), Verdict::Restart);
        assert_eq!(decide(Mode::Manual { all: false }, false, &facts(true, true, true, 0), || false), Verdict::Focused);
    }

    #[test]
    fn each_verdict_says_what_happens_to_the_session() {
        assert_eq!(Verdict::Restart.words(true), "restarted");
        assert_eq!(Verdict::Busy.words(true), "restarts when idle");
        assert_eq!(Verdict::Focused.words(true), "you are in it: restarts when you leave it");
        assert_eq!(Verdict::Asked.words(true), "waiting for your answer: background work running");
        assert_eq!(Verdict::Skip("its session id is unknown".to_string()).words(true), "skipped: its session id is unknown");
        assert!(!Verdict::Skip(String::new()).stale() && !Verdict::Fresh.stale() && Verdict::Busy.stale());
        assert_eq!(Verdict::Drafted.words(true), "its input box holds text or cannot be read: restarts once it is empty");
        assert!(Verdict::Drafted.stale());
    }

    #[test]
    fn a_refused_lead_restart_holds_nothing_back_and_a_real_failure_is_recorded_and_told() {
        let mut restarted = Map::new();
        let attempt = entry(1000, "/v/2.1.304", &[KEBBIE_UPDATE], Some("kebbie 0.31.0"), None);
        let next = entry(1060, "/v/2.1.304", &[KEBBIE_UPDATE], Some("kebbie 0.31.0"), None);
        let refused: Result<tasks::Started> = Err("Not restarted: the lead is working.".to_string());
        assert!(!note_attempt(&mut restarted, "s1", attempt.clone(), &refused));
        assert!(held_back(restarted.get("s1"), &next, false).is_none(), "the next round tries again");
        assert_eq!(told_failure("lead", "Not restarted: the lead is working."), None, "a refusal is not a failure to tell");
        assert_eq!(failed_said("Not restarted: the lead is working."), "not restarted yet: the lead is working");
        let failed: Result<tasks::Started> = Err("Claude did not come back in pane p_9".to_string());
        assert!(note_attempt(&mut restarted, "s1", attempt, &failed));
        assert!(held_back(restarted.get("s1"), &next, false).is_some(), "a failure that ended a Claude waits out the cooldown");
        assert_eq!(told_failure("lead", "Claude did not come back in pane p_9").as_deref(), Some("Restarting lead failed: Claude did not come back in pane p_9"));
        assert_eq!(failed_said("Claude did not come back in pane p_9"), "restart failed: Claude did not come back in pane p_9");
    }

    #[test]
    fn a_restart_never_ends_a_claude_over_unsent_text_and_on_its_own_waits_on_a_box_it_cannot_read() {
        let typed = || Draft::Typed("half a thought".to_string());
        for mode in [ROUTINE, Mode::Update, Mode::Manual { all: false }] {
            assert_eq!(with_draft(mode, Verdict::Restart, typed), Verdict::Drafted, "{mode:?}");
            assert_eq!(with_draft(mode, Verdict::Restart, || Draft::Empty), Verdict::Restart, "{mode:?}");
        }
        assert_eq!(with_draft(ROUTINE, Verdict::Restart, || Draft::Unreadable), Verdict::Drafted);
        assert_eq!(with_draft(Mode::Update, Verdict::Restart, || Draft::Unreadable), Verdict::Drafted);
        assert_eq!(with_draft(Mode::Manual { all: false }, Verdict::Restart, || Draft::Unreadable), Verdict::Restart, "kebbie restart by hand goes ahead");
        assert_eq!(with_draft(ROUTINE, Verdict::Busy, || panic!("a busy session's box is not read")), Verdict::Busy);
    }

    #[test]
    fn the_lead_restarts_on_its_own_a_minute_after_the_user_left_its_pane_but_never_while_they_are_in_it_or_it_has_background_work() {
        let lead = mode_for(ROUTINE, true);
        assert_eq!(lead, Mode::Routine { min_idle: LEAD_IDLE });
        assert_eq!(mode_for(ROUTINE, false), ROUTINE, "a task waits the idle minutes");
        assert_eq!(mode_for(Mode::Routine { min_idle: 30 }, true), Mode::Routine { min_idle: 30 });
        assert_eq!(decide(lead, true, &facts(true, true, false, LEAD_IDLE), || false), Verdict::Restart);
        assert_eq!(decide(lead, true, &facts(true, true, false, LEAD_IDLE - 1), || false), Verdict::Quiet);
        assert_eq!(decide(lead, true, &facts(true, true, true, 99_999), || false), Verdict::Focused, "the user's keys would land in a pane being ended");
        assert_eq!(decide(lead, true, &facts(true, true, false, 99_999), || true), Verdict::Ask, "background work is asked about");
        assert_eq!(decide(lead, false, &facts(true, true, false, 99_999), || false), Verdict::Manual, "restart.auto = false restarts nothing on its own");
    }

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn the_restart_keeps_the_flags_and_drops_the_old_conversation_and_prompt() {
        let lead = argv(&["claude", "--append-system-prompt-file", "/s/roles/lead.md", "--settings", "/s/roles/lead.settings.json", "--resume", "99137e9d"]);
        assert_eq!(resume_args(&lead, "new-id").unwrap(), argv(&["--append-system-prompt-file", "/s/roles/lead.md", "--settings", "/s/roles/lead.settings.json", "--resume", "new-id"]));
        let task = argv(&["/opt/homebrew/bin/claude", "--permission-mode", "acceptEdits", "--settings", "/s/x.json", "--session-id", "0b5e", "--dangerously-skip-permissions", "--add-dir", "/a", "/b", "--model=opus", "-c", "fix the bug"]);
        assert_eq!(resume_args(&task, "id").unwrap(), argv(&["--permission-mode", "acceptEdits", "--settings", "/s/x.json", "--dangerously-skip-permissions", "--add-dir", "/a", "/b", "--model=opus", "--resume", "id"]));
        assert_eq!(resume_args(&argv(&["claude", "--plugin-dir", "/p", "-r", "--", "--model", "x"]), "id").unwrap(), argv(&["--plugin-dir", "/p", "--resume", "id"]));
        assert_eq!(resume_args(&argv(&["claude", "explain this", "--resume=abc", "--fork-session"]), "id").unwrap(), argv(&["--resume", "id"]));
        assert_eq!(resume_args(&argv(&["node", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js", "--model", "sonnet", "hello"]), "id").unwrap(), argv(&["--model", "sonnet", "--resume", "id"]));
        assert_eq!(resume_args(&argv(&["claude", "--debug", "hooks"]), "id").unwrap(), argv(&["--debug", "hooks", "--resume", "id"]));
    }

    #[test]
    fn a_restart_that_waits_at_the_trust_prompt_tells_the_board_to_answer_it_rather_than_that_it_runs() {
        let reasons = ["kebbie was rebuilt".to_string()];
        assert_eq!(restarted_said("billing@app", &reasons, &tasks::Started::Ready), "Restarted billing@app with claude --resume, as it ran older code (kebbie was rebuilt).");
        let waits = restarted_said("billing@app", &reasons, &tasks::Started::AwaitsTrust);
        assert!(waits.contains("asks whether you trust its folder") && waits.contains("Answer it there, and it resumes its conversation") && !waits.contains("with claude --resume"), "{waits}");
        let source = include_str!("restart.rs").split("#[cfg(test)]").next().unwrap();
        assert!(source.contains("Ok(started) => {") && source.contains("tell_board(&restarted_said(&pane.label, &reasons, &started))"), "the board hears how the restart came out");
    }

    #[test]
    fn a_resumed_task_gets_its_settings_and_role_written_now_in_place_of_missing_or_stale_ones() {
        let (settings, role) = (Path::new("/s/roles/billing@app.settings.json"), Path::new("/s/roles/billing@app.md"));
        let fresh = ["--settings", "/s/roles/billing@app.settings.json", "--append-system-prompt-file", "/s/roles/billing@app.md", "--resume", "id"];
        let bare = resume_args(&argv(&["claude", "--permission-mode", "acceptEdits", "--resume", "old"]), "id").unwrap();
        assert_eq!(with_role(&bare, settings, role), [&["--permission-mode", "acceptEdits"][..], &fresh].concat(), "a session started before roles existed gets them on its resume");
        let stale = resume_args(&argv(&["claude", "--settings", "/old/x.json", "--model=opus", "--append-system-prompt-file=/old/task.md", "--resume", "old"]), "id").unwrap();
        assert_eq!(with_role(&stale, settings, role), [&["--model=opus"][..], &fresh].concat(), "an old path is replaced, never kept beside the new one");
        let source = include_str!("restart.rs");
        let perform = &source[source.find("\nfn perform(").unwrap()..];
        let perform = &perform[..perform.find("\n}\n").unwrap()];
        assert!(perform.find("tasks::task_role").unwrap() < perform.find(".end(&pane.pane)").unwrap() && perform.contains("with_role(args, &settings, &role)"), "every task Claude kebbie resumes runs with its settings and role, written before the old one ends");
    }

    #[test]
    fn a_task_claude_without_its_settings_or_role_file_is_told_apart() {
        let dir = scratch("role");
        let (settings, role) = (dir.join("t@r.settings.json"), dir.join("t@r.md"));
        std::fs::write(&settings, "{}").unwrap();
        std::fs::write(&role, "role").unwrap();
        let (settings, role) = (settings.to_string_lossy().to_string(), role.to_string_lossy().to_string());
        assert!(!lacks_role(&argv(&["claude", "--settings", &settings, "--append-system-prompt-file", &role, "--resume", "id"])));
        assert!(!lacks_role(&argv(&["claude", &format!("--settings={settings}"), &format!("--append-system-prompt-file={role}")])));
        assert!(lacks_role(&argv(&["claude", "--permission-mode", "acceptEdits", "--resume", "id"])), "started before kebbie gave tasks a role");
        assert!(lacks_role(&argv(&["claude", "--settings", &settings, "--resume", "id"])));
        assert!(lacks_role(&argv(&["claude", "--settings", &settings, "--append-system-prompt-file", &format!("{role}.gone")])), "its role file was lost");
        let source = include_str!("restart.rs").split("#[cfg(test)]").next().unwrap();
        assert!(source.contains("is_some_and(|argv| pane.agent.lacks_role(argv)) && cwd.as_deref().and_then(tasks::task_at).is_some()") && source.contains("reasons.push(NO_ROLE.to_string())"), "a task Claude without its role counts as stale, so it is restarted with it");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_command_line_kebbie_cannot_place_is_refused_rather_than_restarted_blind() {
        assert!(resume_args(&argv(&["claude", "--brand-new-flag", "maybe a prompt"]), "id").unwrap_err().contains("--brand-new-flag, an option kebbie does not know"));
        assert!(resume_args(&argv(&["claude", "-p", "hi"]), "id").unwrap_err().contains("-p"));
        assert!(resume_args(&argv(&["vim", "x"]), "id").unwrap_err().contains("not claude"));
        assert!(resume_args(&argv(&["claude", "--model"]), "id").unwrap_err().contains("--model has no value"));
        assert!(resume_args(&[], "id").is_err());
    }

    #[test]
    fn ps_elapsed_times_and_claude_timestamps_read_as_seconds() {
        assert_eq!(parse_etime("01-23:23:25"), Some(86_400 + 23 * 3600 + 23 * 60 + 25));
        assert_eq!(parse_etime("  23:25\n"), Some(23 * 60 + 25));
        assert_eq!(parse_etime("1:02:03"), Some(3723));
        assert_eq!(parse_etime("x"), None);
        assert_eq!(iso_seconds("1970-01-02T00:00:01.396Z"), Some(86_401));
        assert_eq!(iso_seconds("2026-10-20T13:06:06.770Z"), Some(1_792_501_566));
    }

    fn changes() -> Changes {
        Changes { claude: Some(PathBuf::from("/v/2.1.304")), ..Changes::default() }
    }

    #[test]
    fn a_claude_on_the_keepers_path_older_than_the_sessions_own_never_makes_it_stale() {
        let dir = crate::sh::test_dir("restart-older-path");
        let (old, new) = (dir.join("claude-2.1.296/bin/claude"), dir.join("claude-2.1.304/bin/claude"));
        for (file, at) in [(&old, 1_000), (&new, 2_000)] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "").unwrap();
            std::fs::File::options().write(true).open(file).unwrap().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(at)).unwrap();
        }
        let path_has = |claude: &Path| Changes { claude: Some(claude.canonicalize().unwrap()), ..Changes::default() };
        assert!(path_has(&old).reasons(Some(&new), 3_000, None).is_empty(), "a keeper started before the devbox moved Claude keeps the old PATH, and the lead it restarted runs the newer one");
        assert_eq!(path_has(&new).reasons(Some(&old), 3_000, None), ["a newer Claude Code"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn every_stale_line_carries_its_reasons_to_the_board_and_a_held_line_names_its_own_once() {
        let source = include_str!("restart.rs");
        let run = &source[source.find("\npub fn run_counted(").unwrap()..];
        let run = &run[run.find("held_back(").unwrap()..run.find("\n    counted(total, total)").unwrap()];
        assert!(!run.contains("reasons: Vec::new()"), "a line written once the reasons are known drops none of them");
        let held = Line { label: "lead".to_string(), said: format!("{HELD}a restart brought back the same Claude Code (/c), so the pane starts that one; restarting again cannot update it"), stale: true, reasons: vec!["a newer Claude Code".to_string()] };
        let manual = Line { label: "lead".to_string(), said: Verdict::Manual.words(false), stale: true, reasons: vec![OUTSIDE_LEAD_DIR.to_string()] };
        assert_eq!(crate::keeper::stale_lines(&[held.clone(), manual.clone()]), [format!("lead: {}{}a newer Claude Code", held.said, crate::keeper::REASONS), format!("lead: stale: run kebbie restart when you want it{}{OUTSIDE_LEAD_DIR}", crate::keeper::REASONS)]);
        assert_eq!(report(&with_reasons(vec![held.clone(), manual]))[1..], [format!("  lead: {}", held.said), format!("  lead: stale: run kebbie restart when you want it, as {OUTSIDE_LEAD_DIR}")]);
        let update = [Line { label: "lead".to_string(), said: "stale: run kebbie restart when you want it".to_string(), stale: true, reasons: Vec::new() }];
        assert_eq!(crate::update::live_sessions(&update, Some(&crate::keeper::stale_lines(&[held.clone()])))[0].said, held.said, "the update window reads the said without the reasons");
    }

    #[test]
    fn a_lead_restart_starts_its_claude_in_the_leads_own_folder() {
        let source = include_str!("lead.rs");
        let start = &source[source.find("\nfn start_chat(").unwrap()..];
        let start = &start[..start.find("\n}\n").unwrap()];
        let cd = start.find("cd_line(&lead_dir()?)").expect("the pane's shell goes to the lead's folder");
        assert!(cd < start.find("Agent::Claude.start(").unwrap(), "herdr starts Claude where the shell is, so a lead started from a renamed or other folder would stay outside its own");
        assert!(source[source.find("\nfn restart_locked(").unwrap()..].split("\n}\n").next().unwrap().contains("start_chat(&pane, session)"), "L goes through start_chat");
    }

    #[test]
    fn a_session_is_stale_when_it_runs_another_claude_binary_or_started_before_a_change_it_loads() {
        let current = changes();
        assert!(current.reasons(Some(Path::new("/v/2.1.304")), 100, None).is_empty());
        assert_eq!(current.reasons(Some(Path::new("/v/2.1.302")), 100, None), ["Claude Code 2.1.302 → 2.1.304"]);
        assert!(current.reasons(Some(Path::new("/usr/bin/node")), 100, None).is_empty(), "an interpreter is not the install, so only the change time counts");
        let replaced = Changes { claude_changed: Some(200), ..changes() };
        assert_eq!(replaced.reasons(Some(Path::new("/usr/bin/node")), 100, None), ["a newer Claude Code"]);
        assert!(replaced.reasons(Some(Path::new("/v/2.1.304")), 300, None).is_empty());
        let plugin = |id: &str, updated: u64, project: Option<&str>| Plugin { id: id.to_string(), updated, project: project.map(PathBuf::from) };
        let plugins = Changes { plugins: vec![plugin("vault@gronare", 500, None), plugin("kebbie@kebbie", 50, None), plugin("local@x", 900, Some("/repo"))], ..changes() };
        assert_eq!(plugins.reasons(Some(Path::new("/v/2.1.304")), 100, Some(Path::new("/elsewhere"))), ["plugin vault updated"]);
        assert_eq!(plugins.reasons(Some(Path::new("/v/2.1.304")), 100, Some(Path::new("/repo/sub"))), ["plugin vault, local updated"]);
        assert!(plugins.reasons(Some(Path::new("/v/2.1.304")), 1000, None).is_empty());
        let mcp = Changes { mcp: HashMap::from([("project:/repo".to_string(), 500), ("user".to_string(), 10)]), updated: Some(700), ..changes() };
        assert_eq!(mcp.reasons(Some(Path::new("/v/2.1.304")), 100, Some(Path::new("/repo/a"))), ["MCP servers changed", "kebbie update"]);
        assert_eq!(mcp.reasons(Some(Path::new("/v/2.1.304")), 100, Some(Path::new("/other"))), ["kebbie update"]);
    }

    #[test]
    fn installed_plugins_read_their_last_update() {
        let installed = json!({ "plugins": { "vault@gronare": [{ "lastUpdated": "1970-01-01T00:01:40.000Z" }], "x@y": [{ "installedAt": "1970-01-01T00:00:10Z", "projectPath": "/repo" }], "bad@z": [{}] } });
        let mut found = plugins(&installed);
        found.sort_by(|one, other| one.id.cmp(&other.id));
        assert_eq!(found.iter().map(|plugin| (plugin.id.as_str(), plugin.updated)).collect::<Vec<_>>(), [("vault@gronare", 100), ("x@y", 10)]);
        assert_eq!(found[1].project, Some(PathBuf::from("/repo")));
    }

    #[test]
    fn an_mcp_change_is_dated_by_kebbie_from_the_first_look_on() {
        let dir = scratch("mcp");
        let record = dir.join("mcp.json");
        let first = json!({ "mcpServers": { "a": { "command": "x" } }, "projects": { "/repo": { "mcpServers": {} } } });
        assert_eq!(observe_mcp(&record, &first, 100), HashMap::from([("user".to_string(), 0)]), "what was there before kebbie looked has no change time");
        assert_eq!(observe_mcp(&record, &first, 200), HashMap::from([("user".to_string(), 0)]));
        let added = json!({ "mcpServers": { "a": { "command": "x" } }, "projects": { "/repo": { "mcpServers": { "b": {} } } } });
        assert_eq!(observe_mcp(&record, &added, 300)["project:/repo"], 300);
        let changed = json!({ "mcpServers": { "a": { "command": "y" } }, "projects": {} });
        let seen = observe_mcp(&record, &changed, 400);
        assert_eq!((seen["user"], seen["project:/repo"]), (400, 400), "a server removed is a change too");
    }

    fn lines(records: &[Value]) -> impl Iterator<Item = String> {
        records.iter().map(Value::to_string).collect::<Vec<_>>().into_iter()
    }

    fn launched(at: &str, result: Value) -> Value {
        json!({ "type": "user", "timestamp": at, "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t" }] }, "toolUseResult": result })
    }

    fn notification(id: &str, status: Option<&str>) -> String {
        let status = status.map(|status| format!("<status>{status}</status>\n")).unwrap_or_default();
        format!("<task-notification>\n<task-id>{id}</task-id>\n{status}<summary>x</summary>\n</task-notification>")
    }

    #[test]
    fn background_work_runs_from_its_launch_until_its_notification_with_a_status() {
        let at = "1970-01-01T00:01:40.000Z";
        let records = [
            launched(at, json!({ "stdout": "", "backgroundTaskId": "bash1" })),
            launched(at, json!({ "isAsync": true, "status": "async_launched", "agentId": "agent1" })),
            launched(at, json!({ "taskId": "mon1", "timeoutMs": 1_800_000, "persistent": false })),
            launched(at, json!({ "stdout": "", "backgroundTaskId": "bash2" })),
            json!({ "type": "queue-operation", "operation": "enqueue", "timestamp": at, "content": notification("bash1", Some("completed")) }),
            json!({ "type": "attachment", "timestamp": at, "attachment": { "type": "queued_command", "prompt": notification("agent1", Some("failed")) } }),
            json!({ "type": "user", "timestamp": at, "message": { "role": "user", "content": notification("mon1", None) } }),
        ];
        assert_eq!(background_running(lines(&records), 50, 200), ["bash2", "mon1"], "a monitor's event without a status leaves it running");
        assert_eq!(background_running(lines(&records), 50, 100 + 1800), ["bash2"], "a monitor ends at its timeout");
        assert!(background_running(lines(&records), 150, 200).is_empty(), "work launched before this process started died with the old one");
        let stopped = [launched(at, json!({ "backgroundTaskId": "bash3" })), launched(at, json!({ "message": "Successfully stopped task: bash3 (sleep 9)", "task_id": "bash3", "task_type": "local_bash" }))];
        assert!(background_running(lines(&stopped), 50, 200).is_empty());
        let resumed = [
            launched(at, json!({ "status": "async_launched", "agentId": "agent2" })),
            json!({ "type": "queue-operation", "timestamp": at, "content": notification("agent2", Some("completed")) }),
            json!({ "type": "assistant", "timestamp": at, "message": { "content": [{ "type": "tool_use", "name": "SendMessage", "input": { "to": "agent2", "message": "more" } }] } }),
        ];
        assert_eq!(background_running(lines(&resumed), 50, 200), ["agent2"], "a message to a finished agent sets it working again");
        let quoted = [launched(at, json!({ "backgroundTaskId": "bash4" })), json!({ "type": "api-request", "timestamp": at, "content": notification("bash4", Some("completed")) })];
        assert_eq!(background_running(lines(&quoted), 50, 200), ["bash4"], "a request body quoting history is not a notification");
    }

    #[test]
    fn a_lead_running_in_another_folder_counts_as_behind_and_one_in_its_own_folder_does_not() {
        let own = Path::new("/h/.local/state/kebbie/lead");
        assert!(outside_lead_dir(Some(Path::new("/h/.local/state/other/lead")), own), "another folder");
        assert!(!outside_lead_dir(Some(own), own));
        assert!(!outside_lead_dir(None, own), "an unreadable cwd is no reason to restart");
    }

    #[test]
    fn the_claude_in_a_pane_is_its_process_group_leader_not_an_mcp_server() {
        let info = json!({ "foreground_process_group_id": 4099, "shell_pid": 1927, "foreground_processes": [
            { "argv": ["python", "notes-vault-mcp"], "argv0": "python", "cwd": "/lead", "pid": 4171 },
            { "argv0": "mcp-server", "pid": 4172 },
            { "argv": ["claude", "--resume", "99137e9d"], "argv0": "claude", "cwd": "/lead", "name": "2.1.304", "pid": 4099 },
        ] });
        let process = claude_process(&info).unwrap();
        assert_eq!((process.pid, process.cwd.as_deref()), (4099, Some(Path::new("/lead"))));
        assert_eq!(process.argv.unwrap(), argv(&["claude", "--resume", "99137e9d"]));
        let unreadable = json!({ "foreground_process_group_id": 7, "foreground_processes": [{ "argv0": "claude", "pid": 7 }] });
        assert!(claude_process(&unreadable).unwrap().argv.is_none(), "an argv herdr could not read stays unknown");
        assert!(claude_process(&json!({ "foreground_processes": [] })).is_none());
    }

    #[test]
    fn a_pane_is_named_by_its_agent_name_or_its_title_and_waits_only_when_idle_and_ready() {
        let agent = json!({ "agent": "claude", "agent_session": { "value": "726d" }, "agent_status": "idle", "interactive_ready": true, "display_agent": "Remote dev", "focused": true, "cwd": "/g", "pane_id": "w1:p1", "workspace_id": "w1" });
        let pane = Pane::from_agent(&agent).unwrap();
        assert_eq!((pane.label.as_str(), pane.session.as_deref(), pane.waiting, pane.focused, pane.name), ("Remote dev (w1:p1)", Some("726d"), true, true, None));
        let mut named = agent.clone();
        named["name"] = json!("lead");
        named["agent_status"] = json!("blocked");
        let lead = Pane::from_agent(&named).unwrap();
        assert_eq!((lead.label.as_str(), lead.waiting), ("lead", false));
        let mut unready = agent.clone();
        unready["interactive_ready"] = json!(false);
        assert!(!Pane::from_agent(&unready).unwrap().waiting);
        let mut anonymous = agent.clone();
        anonymous["agent_session"] = Value::Null;
        assert_eq!(Pane::from_agent(&anonymous).unwrap().session, None);
        assert_eq!(Pane::from_agent(&json!({ "agent": "codex", "pane_id": "p" })).map(|pane| pane.agent), Some(Agent::Codex), "a Codex pane is kebbie's");
        assert!(Pane::from_agent(&json!({ "agent": "gemini", "pane_id": "p" })).is_none());
    }

    #[test]
    fn an_answer_saved_by_the_board_during_a_run_is_kept() {
        let mut asked: Map<String, Value> = json!({ "726d": { "pid": 1, "question": "q1", "answer": null }, "9913": { "pid": 2, "question": "q2", "answer": null }, "new": { "pid": 3, "question": "q3", "answer": null } }).as_object().unwrap().clone();
        let on_disk: Map<String, Value> = json!({ "726d": { "pid": 1, "question": "q1", "answer": "yes" }, "9913": { "pid": 9, "question": "q2", "answer": "no" } }).as_object().unwrap().clone();
        keep_answers(&mut asked, &on_disk);
        assert_eq!(asked["726d"]["answer"], "yes");
        assert!(asked["9913"]["answer"].is_null(), "an answer for an earlier process is not this one's");
        assert!(asked["new"]["answer"].is_null());
    }

    #[test]
    fn a_session_is_asked_once_per_process() {
        let asked: Map<String, Value> = json!({ "726d": { "pid": 76033, "question": "q1", "answer": null }, "9913": { "pid": 4099, "question": "q2", "answer": "no" } }).as_object().unwrap().clone();
        assert!(asked_for(&asked, "726d", 76033).is_some());
        assert!(asked_for(&asked, "726d", 80000).is_none(), "a new process is a new staleness");
        assert_eq!(answer_of(asked_for(&asked, "726d", 76033)), None);
        assert_eq!(answer_of(asked_for(&asked, "9913", 4099)), Some(Answer::No));
        assert!(asked_for(&asked, "other", 1).is_none());
    }

    #[test]
    fn the_idle_clock_restarts_whenever_the_session_works_or_is_focused() {
        let mut memory = Memory::default();
        assert_eq!(memory.idle_for("s", true, None, 1000), 0, "first seen idle with no conversation file counts from now");
        assert_eq!(memory.idle_for("s", true, None, 1600), 600);
        assert_eq!(memory.idle_for("s", false, None, 1700), 0);
        assert_eq!(memory.idle_for("s", true, None, 1800), 100, "leaving the pane starts the wait again");
    }
}
