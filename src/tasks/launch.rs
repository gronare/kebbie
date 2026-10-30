use std::time::Duration;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::agent::{Agent, Started, TaskFiles};
use crate::config;
use crate::herdr;
use crate::lead::state_dir;
use crate::messages;
use crate::notice::{Notice, Tone};
use crate::peers;
use crate::profiles::{self, Profile};
use crate::repo::{branch_exists, default_branch, git, git_writing, main_checkout, name_of};
use crate::sh::Result;
use super::{fetch, slug};
use super::pool::{Slot, claim_slot, slots};
use super::started::{read_start, start_entry, started_file};

// A task's temporary files live in its worktree's own git dir, so they go with the task and never land in the repo.
pub(crate) fn task_tmp(worktree: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git(worktree, &["rev-parse", "--absolute-git-dir"])?).join("kebbie-tmp"))
}

pub(super) fn clear_tmp(worktree: &Path) {
    if let Ok(dir) = task_tmp(worktree) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

pub(crate) enum Session {
    New(String),
    Resume(String),
}

pub(crate) fn new_session_id() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut random| random.read_exact(&mut bytes)).map_err(|error| error.to_string())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

fn session_file(task: &str, repo: &Path) -> Result<PathBuf> {
    Ok(session_file_in(&crate::lead::state_location()?, &peers::address(task, &name_of(repo))))
}

fn session_file_in(state: &Path, address: &str) -> PathBuf {
    state.join("sessions").join(address)
}

pub(crate) const BYPASS: &str = "bypassPermissions";

pub fn bypasses(address: &str) -> bool {
    crate::lead::state_location().is_ok_and(|state| bypasses_in(&state, address))
}

fn bypasses_in(state: &Path, address: &str) -> bool {
    std::fs::read_to_string(session_file_in(state, address)).is_ok_and(|text| parse_record(&text).mode.as_deref() == Some(BYPASS))
}

pub fn profile_of(address: &str) -> Option<String> {
    let state = crate::lead::state_location().ok()?;
    std::fs::read_to_string(session_file_in(&state, address)).ok().and_then(|text| parse_record(&text).profile)
}

fn chosen_mode<'a>(given: Option<&'a str>, recorded: Option<&'a str>, configured: Option<&'a str>) -> Option<&'a str> {
    given.or(recorded).or(configured)
}

fn configured_mode(repo: &Path) -> Result<Option<String>> {
    Ok(config::resolve(Some(repo))?.settings.permission_mode.value)
}

// Claude Code's sandbox needs its helpers on the machine; Codex brings its own.
fn sandboxed(configured: bool, profile: &Profile, missing: impl FnOnce() -> Vec<String>) -> Result<bool> {
    let on = profile.sandbox.on(configured);
    if on && profile.agent == Agent::Claude {
        crate::sandbox::refusal(&missing())?;
    }
    Ok(on)
}

fn sandbox_said(on: bool, profile: &Profile) -> Option<String> {
    if !on {
        return Some(crate::sandbox::OFF.to_string());
    }
    let outside = &profile.sandbox.excluded;
    (!outside.is_empty()).then(|| format!("{} run outside the sandbox in this task: they reach other machines, and this one as you", outside.join(", ")))
}

fn task_profile(repo: &Path, given: Option<&str>, recorded: Option<&str>) -> Result<(Profile, Option<String>)> {
    let configured = config::resolve(Some(repo))?.settings.profile.value;
    profiles::for_running_task(&profiles::dir(), given, recorded, configured.as_deref()).map_err(|error| if given.is_some() { format!("--profile: {error}") } else { error })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Record {
    pub session: Option<String>,
    pub mode: Option<String>,
    pub profile: Option<String>,
    pub agent: Option<String>,
}

impl Record {
    fn agent(&self) -> Agent {
        self.agent.as_deref().and_then(|name| Agent::named(name).ok()).unwrap_or(Agent::Claude)
    }
}

// The task's session id, permission mode, profile and agent, so a restart resumes that conversation the same way; a file of two lines, id then mode, is a task on Claude.
pub(crate) fn parse_record(text: &str) -> Record {
    if let Ok(value) = serde_json::from_str::<Value>(text) && value.is_object() {
        let field = |key: &str| value[key].as_str().map(str::trim).filter(|text| !text.is_empty()).map(str::to_string);
        return Record { session: field("session"), mode: field("mode"), profile: field("profile"), agent: field("agent") };
    }
    let mut lines = text.lines().map(str::trim).map(|line| (!line.is_empty()).then(|| line.to_string()));
    Record { session: lines.next().flatten(), mode: lines.next().flatten(), profile: Some(profiles::TASK.to_string()), agent: Some(Agent::Claude.key().to_string()) }
}

pub(crate) fn record_text(record: &Record) -> String {
    json!({ "session": record.session, "mode": record.mode, "profile": record.profile, "agent": record.agent }).to_string()
}

fn recorded_session(task: &str, repo: &Path) -> Record {
    parse_record(&session_file(task, repo).ok().and_then(|file| std::fs::read_to_string(file).ok()).unwrap_or_default())
}

pub(super) fn forget_session(task: &str, repo: &Path) {
    if let Ok(file) = session_file(task, repo) {
        let _ = std::fs::remove_file(prompt_file(&file));
        let _ = std::fs::remove_file(file);
    }
}

fn prompt_file(session: &Path) -> PathBuf {
    let mut name = session.file_name().unwrap_or_default().to_os_string();
    name.push(".prompt");
    session.with_file_name(name)
}

fn store_prompt(file: &Path, prompt: Option<&str>) -> Result<()> {
    match prompt {
        Some(prompt) => crate::state::write(file, prompt),
        None => std::fs::remove_file(file).or_else(|error| if error.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(error.to_string()) }),
    }
}

fn stored_prompt(file: &Path) -> Option<String> {
    std::fs::read_to_string(file).ok().filter(|prompt| !prompt.trim().is_empty())
}

// A prompt given with the start always goes in; the stored first prompt goes to a new conversation, and to a resumed one that never heard a user turn, as when the first start's shell asked a question before Claude ran.
fn prompt_for(session: &Session, given: Option<&str>, stored: Option<String>, unheard: bool) -> Option<String> {
    if let Some(given) = given {
        return Some(given.to_string());
    }
    match session {
        Session::New(_) => stored,
        Session::Resume(_) if unheard => stored,
        Session::Resume(_) => None,
    }
}

// A slot is reused across tasks, so only a conversation written since this task's branch was created can be this task's.
pub(crate) fn latest_transcript(dir: &Path, since: std::time::SystemTime) -> Option<String> {
    std::fs::read_dir(dir).ok()?.flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "jsonl"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .filter(|(modified, _)| *modified >= since)
        .max_by_key(|(modified, _)| *modified)
        .and_then(|(_, path)| path.file_stem().map(|stem| stem.to_string_lossy().to_string()))
}

fn branch_created(repo: &Path, task: &str) -> Option<std::time::SystemTime> {
    let times = git(repo, &["reflog", "show", "--format=%ct", &format!("refs/heads/{task}")]).ok()?;
    let seconds: u64 = times.lines().last()?.trim().parse().ok()?;
    Some(std::time::UNIX_EPOCH + Duration::from_secs(seconds))
}

fn previous_session(agent: Agent, repo: &Path, slot: &Slot, task: &str, home: &Path, recorded: Option<String>) -> Option<String> {
    recorded.filter(|id| agent.session_transcript(home, Some(&slot.path), id).is_some())
        .or_else(|| agent.latest_session(home, &slot.path, branch_created(repo, task)?))
}

// The task's own conversation, where the user's prompt and later word to it are.
pub fn transcript(repo: &Path, slot: &Slot) -> Option<(Agent, PathBuf)> {
    let task = slot.task.as_deref()?;
    let record = recorded_session(task, repo);
    let agent = record.agent();
    let home = agent.home()?;
    let id = previous_session(agent, repo, slot, task, &home, record.session)?;
    Some((agent, agent.session_transcript(&home, Some(&slot.path), &id)?))
}

// Codex picks its own session id, so its SessionStart hook writes it into the task's record; a task on Claude keeps the id kebbie gave it.
pub(crate) fn record_session(address: &str, session: &str) -> Result<()> {
    record_session_in(&crate::lead::state_location()?, address, session)
}

fn record_session_in(state: &Path, address: &str, session: &str) -> Result<()> {
    let file = session_file_in(state, address);
    let Ok(text) = std::fs::read_to_string(&file) else { return Ok(()) };
    let record = parse_record(&text);
    if record.agent() != Agent::Codex || record.session.as_deref() == Some(session) {
        return Ok(());
    }
    crate::state::write(&file, record_text(&Record { session: Some(session.to_string()), ..record }))
}

pub(super) fn open_space(repo: &Path, slot: &Slot, task: &str, focus: bool) -> Result<herdr::Space> {
    let space = herdr::open_worktree(repo, &slot.path, task, focus)?;
    herdr::rename_workspace(&space.workspace_id, task)?;
    Ok(space)
}

pub struct Start<'a> {
    pub prompt: Option<&'a str>,
    pub focus: bool,
    pub from: Option<&'a str>,
    pub mode: Option<&'a str>,
    pub profile: Option<&'a str>,
    pub backlog: Option<&'a str>,
    pub note: bool,
    pub anyway: bool,
}

pub struct Begun {
    pub said: String,
    pub started: Started,
}

pub fn start(cwd: &Path, name: &str, options: &Start) -> Result<String> {
    begin(cwd, name, options).map(|begun| begun.said)
}

pub fn begin(cwd: &Path, name: &str, options: &Start) -> Result<Begun> {
    let Start { prompt, focus, from, mode, profile: given, note: with_note, backlog, anyway } = *options;
    mode.map(config::permission_mode).transpose().map_err(|error| format!("--mode: {error}"))?;
    let repo = main_checkout(cwd)?;
    let task = slug(name)?;
    super::after::queued_refusal(&state_dir()?, &peers::address(&task, &name_of(&repo)))?;
    let (setup_skill, setup_command, rules_skill) = skills(&repo)?;
    let vault = if with_note { crate::vault_cli::detect() } else { None };
    let running = slots(&repo)?.into_iter().find(|slot| slot.task.as_deref() == Some(task.as_str()));
    let recorded = running.as_ref().and_then(|_| recorded_session(&task, &repo).profile);
    let (profile, fell_back) = task_profile(&repo, given, recorded.as_deref())?;
    sandboxed(config::resolve(Some(&repo))?.settings.sandbox.value, &profile, crate::sandbox::missing_here)?;
    if profile.agent == Agent::Codex {
        crate::agent::codex::ready_to_start(crate::agent::codex::home().as_deref(), &peers::kebbie_command()?)?;
    }
    let place = |slot: &Slot| Launch { repo: &repo, task: &task, mode, profile: &profile, setup_skill: &setup_skill, setup_command: setup_command.as_deref(), rules_skill: &rules_skill, worktree: slot.path.clone(), vault: vault.is_some(), note: None };
    if let Some(existing) = running {
        return restart(&place(&existing), &existing, focus, prompt).map(|begun| match &fell_back {
            Some(why) => Begun { said: Notice::read(&begun.said, false).detail("careful", why).to_string(), ..begun },
            None => begun,
        });
    }
    let claimed = claim_backlog(vault.as_ref(), backlog, &task, &name_of(&repo), anyway)?;
    let begun = (|| {
        fetch(&repo)?;
        let upstream = default_branch(&repo)?;
        let base = from.unwrap_or(&upstream);
        let slot = claim_slot(&repo, &upstream)?;
        if branch_exists(&repo, &task) {
            git_writing(&slot.path, &["switch", "--quiet", &task])?;
        } else {
            git_writing(&slot.path, &["switch", "--quiet", "-c", &task, base])?;
        }
        let address = peers::address(&task, &name_of(&repo));
        let state = state_dir()?;
        super::after::forget_unwaited_watch(&state, &address);
        crate::phase::record(&state, &address, "start", &crate::phase::Event::Started { prompted: prompt.is_some() });
        let begun = launch_new(&repo, &task, &slot, &address, place(&slot), vault.as_ref(), options);
        if begun.is_err() {
            crate::phase::record(&state, &address, "start", &crate::phase::Event::Started { prompted: false });
        }
        begun
    })();
    if begun.is_err() {
        give_back(vault.as_ref(), claimed.as_deref(), &task);
    }
    begun
}

// Any failure after the claim, from the fetch to the Claude launch, gives the note back to whoever starts it next.
fn give_back(vault: Option<&crate::vault_cli::Cli>, claimed: Option<&str>, task: &str) {
    if let (Some(cli), Some(note), Ok(state)) = (vault, claimed, state_dir()) {
        crate::task_note::release(cli, &state, note, task);
    }
}

// A vault note another machine works on refuses the start unless --anyway; Ok(Some(note)) is the note this start claimed, and a vault that does not answer lets it start unclaimed.
fn claim_backlog(vault: Option<&crate::vault_cli::Cli>, backlog: Option<&str>, task: &str, repo: &str, anyway: bool) -> Result<Option<String>> {
    let (Some(cli), Some(note)) = (vault.filter(|cli| cli.claims), backlog.filter(|item| crate::vault_cli::is_note(item))) else { return Ok(None) };
    match crate::task_note::claim(cli, note, task, repo, anyway) {
        Ok(None) => Ok(Some(note.to_string())),
        Ok(Some(held)) => Err(crate::task_note::refusal(note, &held, crate::backlog::today())),
        Err(error) => {
            if let Ok(state) = state_dir() {
                crate::vault_cli::warn_log(&state, &format!("{note}: not claimed ({error})"));
            }
            Ok(None)
        }
    }
}

// The new task's Space, Claude and prompt; when any of them fails, its prompt never reached Claude.
fn launch_new(repo: &Path, task: &str, slot: &Slot, address: &str, place: Launch, vault: Option<&crate::vault_cli::Cli>, options: &Start) -> Result<Begun> {
    let shell = sandboxed(config::resolve(Some(repo))?.settings.sandbox.value, place.profile, crate::sandbox::missing_here)?;
    let Start { prompt, focus, mode, backlog, .. } = *options;
    let profile = place.profile;
    let shared = vault.map(|_| state_dir().map(|state| crate::task_note::shared(&state, task, address))).transpose()?.flatten();
    let (note, vault_said) = match (vault, shared) {
        (_, Some(note)) => (Some(note.clone()), format!("{note}, shared with its other halves")),
        (Some(cli), None) => match crate::task_note::open(cli, task, &slot.path, backlog) {
            Ok(note) => {
                claim_opened(cli, &note, backlog, task, repo);
                (Some(note.clone()), note)
            }
            Err(error) => (None, format!("not opened ({error}); the task runs without one")),
        },
        (None, None) => (None, String::new()),
    };
    if note.is_some() {
        let entry = start_entry(note.as_deref(), backlog).unwrap_or_default();
        started_file(task, repo).and_then(|file| crate::state::write(&file, entry))?;
    }
    store_prompt(&prompt_file(&session_file(task, repo)?), prompt)?;
    let space = open_space(repo, slot, task, false)?;
    let agent_pane = match space.root_pane_id {
        Some(pane) => pane,
        None => herdr::create_tab(&space.workspace_id, &slot.path, task)?,
    };
    name_agent_tab(&agent_pane, task);
    let configured = configured_mode(repo)?;
    let configured = profile.permission_mode.clone().or(configured);
    let (agent_name, started) = launch(&Launch { note: note.as_deref(), mode: chosen_mode(mode, None, configured.as_deref()), ..place }, &agent_pane, Session::New(new_session_id()?))?;
    let prompted = match &started {
        Ok(started) => send_prompt(profile.agent.name(), started, prompt, |text| crate::typing::prompt_confirmed(&agent_name, text, address), |text| messages::hold_prompt(address, text))
            .map_err(|error| unprompted(profile.agent.name(), "Started", task, &slot.path, &error)),
        Err(_) => Ok(Asks::default()),
    };
    if focus {
        herdr::focus_workspace(&space.workspace_id)?;
    }
    let started = started?;
    let trust_said = prompted?;
    if let Some(item) = backlog.filter(|_| note.is_none()) {
        started_file(task, repo).and_then(|file| crate::state::write(&file, item))?;
    }
    if let Err(error) = messages::announce(task, &name_of(repo)) {
        eprintln!("Could not tell {task}'s other halves it started: {error}");
    }
    let mut said = trust_said.on(Notice::new(Tone::Done, "Started")).detail("task", task).detail("folder", slot.path.display().to_string()).detail("note", vault_said);
    if let Some(sandbox) = sandbox_said(shell, profile) {
        said = said.detail("sandbox", sandbox);
    }
    Ok(Begun { said: said.to_string(), started })
}

// A note kebbie opened for the task is claimed as a backlog note is, so the other machine sees it worked on too.
fn claim_opened(cli: &crate::vault_cli::Cli, note: &str, backlog: Option<&str>, task: &str, repo: &Path) {
    if !cli.claims || backlog == Some(note) {
        return;
    }
    if let Err(error) = crate::task_note::claim(cli, note, task, &name_of(repo), false)
        && let Ok(state) = state_dir()
    {
        crate::vault_cli::warn_log(&state, &format!("{note}: not claimed ({error})"));
    }
}

struct Launch<'a> {
    repo: &'a Path,
    task: &'a str,
    mode: Option<&'a str>,
    profile: &'a Profile,
    setup_skill: &'a str,
    setup_command: Option<&'a str>,
    rules_skill: &'a str,
    worktree: PathBuf,
    vault: bool,
    note: Option<&'a str>,
}

fn skills(repo: &Path) -> Result<(String, Option<String>, String)> {
    let config::Settings { setup_skill, setup_command, rules_skill, .. } = config::resolve(Some(repo))?.settings;
    Ok((setup_skill.value, config::setup_command_in(setup_command.value, repo), rules_skill.value))
}

pub(crate) struct Prepared {
    pub settings: PathBuf,
    pub role: PathBuf,
    pub sandboxed: bool,
    pub tmp: PathBuf,
}

// Every agent kebbie starts or resumes for a task runs with these two files, written afresh: a session without them never reports.
fn prepare(place: &Launch, resumed: bool) -> Result<Prepared> {
    let Launch { repo, task, profile, setup_skill, setup_command, rules_skill, ref worktree, vault, note, .. } = *place;
    crate::push_block::warn_if_off(crate::push_block::install(worktree, &state_dir()?)?);
    let address = peers::address(task, &name_of(repo));
    let note = note.filter(|_| vault);
    let tmp = task_tmp(worktree)?;
    std::fs::create_dir_all(&tmp).map_err(|error| format!("{}: {error}", tmp.display()))?;
    let shell = sandboxed(config::resolve(Some(repo))?.settings.sandbox.value, profile, crate::sandbox::missing_here)?;
    let mut dirs = crate::sandbox::TaskDirs::new(worktree, &tmp);
    crate::sandbox::mark(&state_dir()?, worktree, shell)?;
    if shell {
        let state = state_dir()?;
        crate::sandbox::record_half(&state, task, &dirs)?;
        dirs.halves = crate::sandbox::halves(&state, task, worktree);
    }
    let written = profile.agent.task_settings(&peers::kebbie_command()?, &address, note.map(|_| address.as_str()), Some(&dirs), profile, shell);
    let settings = crate::lead::write_settings(&crate::lead::roles_dir()?, &address, &written)?;
    let added = profiles::role_section(profile)?;
    let (role, text) = peers::role_file(task, &peers::Place { agent: profile.agent, repo, worktree, setup_skill, setup_command, rules_skill, note, profile: &added, sandboxed: shell })?;
    crate::known::began(&address, &text, resumed);
    Ok(Prepared { settings, role, sandboxed: shell, tmp })
}

pub(crate) fn task_at(cwd: &Path) -> Option<(PathBuf, Slot)> {
    let repo = main_checkout(cwd).ok()?;
    let slot = slots(&repo).ok()?.into_iter().find(|slot| slot.task.is_some() && cwd.starts_with(&slot.path))?;
    Some((repo, slot))
}

pub(crate) fn task_role(cwd: &Path) -> Result<Option<(PathBuf, PathBuf)>> {
    let Some((repo, slot)) = task_at(cwd) else { return Ok(None) };
    let task = slot.task.clone().unwrap_or_default();
    let (setup_skill, setup_command, rules_skill) = skills(&repo)?;
    let note = read_start(&task, &repo).and_then(|text| crate::task_note::note_in(&text));
    let vault = note.is_some() && crate::vault_cli::detect().is_some();
    let (profile, fell_back) = task_profile(&repo, None, recorded_session(&task, &repo).profile.as_deref())?;
    if let Some(why) = fell_back {
        let said = format!("{} restarts as the built-in task: {why}.", peers::address(&task, &name_of(&repo)));
        eprintln!("{said}");
        if let Err(error) = messages::to_lead(messages::ToLead::TaskRestarted, &said) {
            eprintln!("Could not tell the lead: {error}");
        }
    }
    let place = Launch { repo: &repo, task: &task, mode: None, profile: &profile, setup_skill: &setup_skill, setup_command: setup_command.as_deref(), rules_skill: &rules_skill, worktree: slot.path.clone(), vault, note: note.as_deref() };
    prepare(&place, true).map(|prepared| Some((prepared.settings, prepared.role)))
}

// The task's role as kebbie gives it now, written to its role file alone: the file, and the role without its peer list.
pub(crate) fn task_role_now(cwd: &Path) -> Result<Option<(PathBuf, String)>> {
    let Some((repo, slot)) = task_at(cwd) else { return Ok(None) };
    let task = slot.task.clone().unwrap_or_default();
    let (setup_skill, setup_command, rules_skill) = skills(&repo)?;
    let note = read_start(&task, &repo).and_then(|text| crate::task_note::note_in(&text)).filter(|_| crate::vault_cli::detect().is_some());
    let (profile, _) = task_profile(&repo, None, recorded_session(&task, &repo).profile.as_deref())?;
    let added = profiles::role_section(&profile)?;
    let sandboxed = profile.sandbox.on(config::resolve(Some(&repo))?.settings.sandbox.value);
    peers::role_file(&task, &peers::Place { agent: profile.agent, repo: &repo, worktree: &slot.path, setup_skill: &setup_skill, setup_command: setup_command.as_deref(), rules_skill: &rules_skill, note: note.as_deref(), profile: &added, sandboxed }).map(Some)
}

// Starts the task's agent in the pane with its role and settings; the outer error is setup, the inner one is the agent not starting.
fn launch(place: &Launch, pane: &str, session: Session) -> Result<(String, Result<Started>)> {
    let Launch { repo, task, mode, profile, .. } = *place;
    if profile.agent == Agent::Codex {
        crate::agent::codex::ready_to_start(crate::agent::codex::home().as_deref(), &peers::kebbie_command()?)?;
    }
    let prepared = prepare(place, matches!(session, Session::Resume(_)))?;
    if let Err(error) = messages::drop_held_prompts(&peers::address(task, &name_of(repo))) {
        eprintln!("Could not drop a prompt still held for {task} from an earlier start: {error}");
    }
    // herdr's agent names are global, so a task's half in another repo is named with its repo.
    let taken = herdr::agents()?.iter().any(|agent| agent["name"] == task);
    let agent_name: String = if taken { format!("{task}_{}", slug(&name_of(repo))?) } else { task.to_string() }.chars().take(32).collect();
    let agent = profile.agent;
    let address = peers::address(task, &name_of(repo));
    let files = TaskFiles { address: &address, settings: &prepared.settings, role: &prepared.role, sandboxed: prepared.sandboxed, tmp: Some(&prepared.tmp) };
    let agent_args = agent.start_args(mode, profile, &files, &session);
    let started = agent.start(&agent_name, pane, &agent_args.iter().map(String::as_str).collect::<Vec<_>>(), agent.trusted_by_user(repo));
    if let Ok(started) = &started {
        let id = match &session {
            Session::New(_) if agent == Agent::Codex => None,
            Session::New(id) => Some(id.clone()),
            Session::Resume(_) => recorded_session(task, repo).session,
        };
        let mode = if *started == Started::BypassDisabled { None } else { mode.map(str::to_string) };
        let record = Record { session: id, mode, profile: Some(profile.name.clone()), agent: Some(agent.key().to_string()) };
        crate::state::write(&session_file(task, repo)?, record_text(&record))?;
    }
    Ok((agent_name, started))
}

fn agent_in(workspace: &str) -> Result<Option<String>> {
    Ok(herdr::agents()?.iter().find(|agent| Agent::of(agent).is_some() && agent["workspace_id"].as_str() == Some(workspace)).map(|agent| agent["pane_id"].as_str().unwrap_or_default().to_string()))
}

// The task's tab carries the task's name; an earlier kebbie named it after the agent.
pub(super) fn name_agent_tab(pane: &str, task: &str) {
    herdr::name_tab_of(pane, task, &Agent::ALL.map(Agent::key));
}

// A task whose Space or Claude is gone gets them back: its last conversation resumed, or a new one given the task's first prompt again.
fn restart(place: &Launch, slot: &Slot, focus: bool, given: Option<&str>) -> Result<Begun> {
    let task = place.task;
    let open = herdr::workspace_for(&slot.path)?;
    if let Some(workspace) = &open
        && let Some(pane) = agent_in(workspace)?
    {
        name_agent_tab(&pane, task);
        if focus {
            herdr::focus_workspace(workspace)?;
        }
        return Ok(Begun { said: Notice::new(Tone::NotNow, "It is already running").detail("task", task).detail("folder", slot.path.display().to_string()).to_string(), started: Started::Ready });
    }
    let space = open_space(place.repo, slot, task, false)?;
    let pane = match (&open, space.root_pane_id) {
        (None, Some(pane)) => pane,
        _ => herdr::create_tab(&space.workspace_id, &slot.path, task)?,
    };
    name_agent_tab(&pane, task);
    let Record { session: recorded, mode, .. } = recorded_session(task, place.repo);
    let agent = place.profile.agent;
    let previous = agent.home().and_then(|home| previous_session(agent, place.repo, slot, task, &home, recorded));
    let note = read_start(task, place.repo).and_then(|text| crate::task_note::note_in(&text));
    let configured = configured_mode(place.repo)?;
    let configured = place.profile.permission_mode.clone().or(configured);
    let place = Launch { mode: chosen_mode(place.mode, mode.as_deref(), configured.as_deref()), worktree: place.worktree.clone(), note: note.as_deref(), ..*place };
    let session = match previous {
        Some(id) => Session::Resume(id),
        None => Session::New(new_session_id()?),
    };
    let resumed = matches!(session, Session::Resume(_));
    let prompt_path = session_file(task, place.repo).ok().map(|file| prompt_file(&file));
    let stored = prompt_path.as_deref().and_then(stored_prompt);
    if let (Some(given), None, Some(path)) = (given, &stored, &prompt_path) {
        store_prompt(path, Some(given))?;
    }
    let unheard = match &session {
        Session::Resume(id) => agent.home().and_then(|home| agent.session_transcript(&home, Some(&slot.path), id)).is_none_or(|file| unheard_in(agent, &file)),
        Session::New(_) => true,
    };
    let prompt = prompt_for(&session, given, stored, unheard);
    let (agent_name, started) = launch(&place, &pane, session)?;
    let address = peers::address(task, &name_of(place.repo));
    let prompted = match &started {
        Ok(started) => send_prompt(agent.name(), started, prompt.as_deref(), |text| crate::typing::prompt_confirmed(&agent_name, text, &address), |text| messages::hold_prompt(&address, text))
            .map_err(|error| unprompted(agent.name(), "Restarted", task, &slot.path, &error)),
        Err(_) => Ok(Asks::default()),
    };
    if focus {
        herdr::focus_workspace(&space.workspace_id)?;
    }
    let started = started?;
    let trust_said = prompted?;
    if prompt.is_some() {
        crate::phase::record(&state_dir()?, &address, "restart", &crate::phase::Event::Started { prompted: true });
    }
    let held = started.asks().is_some();
    let how = match (resumed, prompt.is_some()) {
        (true, true) if !held => "It resumes its last conversation and has the prompt again.",
        (true, _) => "It resumes its last conversation.",
        (false, true) if !held => "No conversation of its own was found, so it starts a new one with the task's prompt.",
        (false, true) => "No conversation of its own was found, so it starts a new one.",
        (false, false) => "No conversation of its own was found, so it starts a new one; there was no prompt to send again.",
    };
    let said = trust_said.on(Notice::new(Tone::Done, format!("Restarted its {}", agent.name())).next(how)).detail("task", task).detail("folder", slot.path.display().to_string());
    Ok(Begun { said: said.to_string(), started })
}

// A conversation kebbie cannot read, as a rollout Codex compressed, may have heard its prompt, so it is not sent again.
fn unheard_in(agent: Agent, file: &Path) -> bool {
    !crate::agent::codex::compressed(file) && std::fs::read_to_string(file).ok().is_none_or(|text| agent.user_messages(&text).is_empty())
}

fn unprompted(name: &str, how: &str, task: &str, folder: &Path, error: &str) -> String {
    Notice::new(Tone::You, format!("{how}, but its prompt did not reach {name}")).you(format!("Paste the task's prompt into its {name}.")).detail("task", task).detail("folder", folder.display().to_string()).detail("error", error).to_string()
}

// What a start still asks of the user once Claude runs: an answer only they can give, and what kebbie does after it.
#[derive(Debug, Default, PartialEq)]
struct Asks {
    you: Option<String>,
    next: Option<String>,
    detail: Option<String>,
}

impl Asks {
    fn on(self, notice: Notice) -> Notice {
        let mut notice = match self.you {
            Some(you) => Notice { tone: Tone::You, ..notice }.you(you),
            None => notice,
        };
        if let Some(next) = self.next {
            notice = notice.next(next);
        }
        match self.detail {
            Some(detail) => notice.detail("mode", detail),
            None => notice,
        }
    }
}

// The prompt goes in as a paste: as an argument, herdr would have to quote it for the shell.
fn send_prompt(name: &str, started: &Started, prompt: Option<&str>, type_now: impl FnOnce(&str) -> Result<()>, hold: impl FnOnce(&str) -> Result<()>) -> Result<Asks> {
    let asks = match started {
        Started::AwaitsTrust => format!("Answer {name}'s question in its Space: whether you trust the repo's folder."),
        Started::AwaitsBypass => format!("Answer {name}'s question in its Space: whether to accept bypass permissions mode."),
        Started::AwaitsHooks => format!("Answer {name}'s question in its Space: whether to trust the hooks it found new or changed."),
        Started::Ready | Started::BypassDisabled => {
            if let Some(prompt) = prompt {
                type_now(prompt)?;
            }
            let disabled = "Claude's managed settings disable bypass permissions mode (permissions.disableBypassPermissionsMode), so it runs with permission prompts";
            return Ok(Asks { detail: (*started == Started::BypassDisabled).then(|| disabled.to_string()), ..Asks::default() });
        }
    };
    Ok(match prompt {
        None => Asks { you: Some(asks.to_string()), ..Asks::default() },
        Some(prompt) => match hold(prompt) {
            Ok(()) => Asks { you: Some(asks.to_string()), next: Some("Once you answer, kebbie types the task's prompt in while the keeper or an open board runs.".to_string()), detail: None },
            Err(error) => Asks { you: Some(format!("{asks} Then paste the task's prompt there yourself: kebbie could not keep it ({error}).")), ..Asks::default() },
        },
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn a_start_from_a_vault_note_claims_it_for_this_machine_and_refuses_another_machines_without_anyway() {
        let fake = crate::vault_cli::fake::Fake::new("start-claim");
        let cli = fake.claiming();
        fake.answer("task-claim", r#"{"path":"Projects/web-search.md","claimed":true,"claim":{"machine":"mac","task":"search","repo":"web","since":"2026-10-23T09:00:00Z"},"replaced":null}"#);
        assert_eq!(claim_backlog(Some(&cli), Some("Projects/web-search.md"), "search", "web", false), Ok(Some("Projects/web-search.md".to_string())));
        fake.answer("task-claim", r#"{"path":"Projects/web-search.md","claimed":false,"claim":{"machine":"devbox","task":"filters","repo":"web","since":"2026-10-20T08:00:00Z"}}"#);
        let refused = claim_backlog(Some(&cli), Some("Projects/web-search.md"), "search", "web", false).unwrap_err();
        assert!(refused.contains("Not started: devbox works on it") && refused.contains("working on devbox · filters") && refused.contains("--anyway"), "{refused}");
        fake.answer("task-claim", r#"{"path":"Projects/web-search.md","claimed":true,"claim":{"machine":"mac","task":"search","repo":"web","since":"2026-10-23T09:00:00Z"},"replaced":{"machine":"devbox","task":"filters","since":"2026-10-20T08:00:00Z"}}"#);
        assert_eq!(claim_backlog(Some(&cli), Some("Projects/web-search.md"), "search", "web", true), Ok(Some("Projects/web-search.md".to_string())), "--anyway takes it over");
        assert_eq!(fake.calls(), [
            "task claim Projects/web-search.md --machine mac --task search --repo web",
            "task claim Projects/web-search.md --machine mac --task search --repo web",
            "task claim Projects/web-search.md --machine mac --task search --repo web --force",
        ]);
        fake.fail("task-claim", "S3 unreachable");
        assert_eq!(claim_backlog(Some(&cli), Some("Projects/web-search.md"), "search", "web", false), Ok(None), "a vault that does not answer never stops a start");
        let asked = fake.calls().len();
        assert_eq!(claim_backlog(Some(&cli), Some("local:Fix login"), "search", "web", false), Ok(None));
        assert_eq!(claim_backlog(Some(&fake.cli), Some("Projects/web-search.md"), "search", "web", false), Ok(None), "an older vault plugin claims nothing");
        assert_eq!(claim_backlog(None, Some("Projects/web-search.md"), "search", "web", false), Ok(None));
        assert_eq!(fake.calls().len(), asked, "a local item, a vault without claims and no vault ask nothing");
    }

    #[test]
    fn a_note_kebbie_opens_for_a_task_is_claimed_too_but_a_claimed_backlog_note_only_once() {
        let fake = crate::vault_cli::fake::Fake::new("open-claim");
        fake.answer("task-claim", r#"{"path":"Projects/web-search.md","claimed":true,"claim":{"machine":"m","task":"search","since":"2026-10-23T09:00:00Z"}}"#);
        claim_opened(&fake.claiming(), "Projects/web-search.md", None, "search", Path::new("/src/web"));
        claim_opened(&fake.claiming(), "Projects/web-search.md", Some("Projects/web-search.md"), "search", Path::new("/src/web"));
        claim_opened(&fake.cli, "Projects/web-search.md", None, "search", Path::new("/src/web"));
        assert_eq!(fake.calls(), ["task claim Projects/web-search.md --machine mac --task search --repo web"]);
        let source = include_str!("launch.rs");
        let begin = &source[source.find("\npub fn begin(").unwrap()..];
        let begin = &begin[..begin.find("\n}\n").unwrap()];
        assert!(begin.find("claim_backlog(").unwrap() < begin.find("claim_slot(").unwrap(), "a refused start claims no worktree");
        let after_claim = &begin[begin.find("claim_backlog(").unwrap()..];
        let inner = &after_claim[after_claim.find("(|| {").unwrap()..after_claim.find("})();").unwrap()];
        assert!(["fetch(", "default_branch(", "claim_slot(", "git_writing(", "launch_new("].iter().all(|step| inner.contains(step)), "every step that can fail after the claim runs inside the part whose failure gives it back");
        assert!(after_claim.contains("if begun.is_err() {\n        give_back("), "a start that fails gives its claim back");
        let fake = crate::vault_cli::fake::Fake::new("give-back");
        fake.answer("task-release", r#"{"path":"Projects/web-search.md","released":true,"claim":null}"#);
        give_back(Some(&fake.claiming()), Some("Projects/web-search.md"), "search");
        give_back(Some(&fake.claiming()), None, "search");
        assert_eq!(fake.calls(), ["task release Projects/web-search.md --machine mac"]);
    }
    use crate::agent::claude::{settings, start_args, transcripts_dir};
    use crate::tasks::test_support::*;
    use crate::peers;

    fn claude_args(mode: Option<&str>, settings: &Path, role: &Path, session: &Session) -> Vec<String> {
        start_args(mode, &profiles::built_in(profiles::TASK).unwrap(), settings, role, session)
    }

    #[test]
    fn edits_are_gated_only_in_a_task_started_with_the_vault_on() {
        let off: Value = serde_json::from_str(&settings("/bin/kebbie", "billing@app", None, None, None)).unwrap();
        assert!(off["hooks"].get("PreToolUse").is_none(), "without the vault no edit hook runs");
        for event in ["PostToolUse", "PostToolUseFailure"] {
            assert_eq!(off["hooks"][event][0]["matcher"], "Bash");
            assert_eq!(off["hooks"][event][0]["hooks"][0]["command"], "'/bin/kebbie' hook evidence 'billing@app'", "every task records its test runs, passed and failed");
        }
        let on: Value = serde_json::from_str(&settings("/bin/kebbie", "billing@app", Some("billing@app"), None, None)).unwrap();
        assert_eq!(on["hooks"]["PreToolUse"][0]["matcher"], "Edit|Write|MultiEdit|NotebookEdit|Bash", "a Bash command that writes a file waits for the Problem too");
        assert_eq!(on["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "'/bin/kebbie' hook gate 'billing@app'", "the hook is told its task, so it runs no git and no herdr to find it");
        assert_eq!(on["hooks"]["Stop"], off["hooks"]["Stop"]);
    }

    #[test]
    fn a_prompt_is_typed_at_once_into_a_ready_claude_and_held_while_the_user_owes_the_trust_answer() {
        let sent = |started: Started, prompt: Option<&str>, keeps: bool| {
            let (typed, held) = (std::cell::RefCell::new(Vec::new()), std::cell::RefCell::new(Vec::new()));
            let said = send_prompt("Claude", &started, prompt, |text| { typed.borrow_mut().push(text.to_string()); Ok(()) }, |text| { held.borrow_mut().push(text.to_string()); if keeps { Ok(()) } else { Err("Another kebbie is holding the message log; try again".to_string()) } });
            (said, typed.into_inner(), held.into_inner())
        };
        assert_eq!(sent(Started::Ready, Some("Fix the flaky test"), true), (Ok(Asks::default()), vec!["Fix the flaky test".to_string()], Vec::new()));
        let (said, typed, held) = sent(Started::AwaitsTrust, Some("Fix the flaky test"), true);
        assert_eq!((typed, held), (Vec::new(), vec!["Fix the flaky test".to_string()]));
        assert!(said.unwrap().next.unwrap().contains("kebbie types the task's prompt in while the keeper or an open board runs"));
        let (said, _, _) = sent(Started::AwaitsTrust, Some("Fix the flaky test"), false);
        assert!(said.unwrap().you.unwrap().contains("paste the task's prompt there yourself"), "a prompt kebbie could not keep is the user's to paste, and the start still stands");
        assert_eq!(sent(Started::AwaitsTrust, None, true).0.unwrap().you.as_deref(), Some("Answer Claude's question in its Space: whether you trust the repo's folder."));
        let source = include_str!("launch.rs");
        let launch = &source[source.find("\nfn launch(").unwrap()..];
        let launch = &launch[..launch.find("\n}\n").unwrap()];
        assert!(launch.find("drop_held_prompts(").unwrap() < launch.find("agent.start(").unwrap(), "a prompt held for an earlier start is dropped before a new one decides its own");
        assert!(launch.contains("if let Err(error) = messages::drop_held_prompts("), "a busy message log never stops a start");
    }

    #[test]
    fn every_task_worktree_gets_the_push_block_before_its_claude_or_its_work_arrives() {
        let source = [include_str!("launch.rs"), include_str!("pool.rs")].concat();
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).or_else(|| source.find(&format!("\npub(super) fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("prepare").contains("push_block::install(") && body("launch").find("prepare(").unwrap() < body("launch").find("agent.start(").unwrap(), "a start and a restart both block pushes before Claude runs");
        assert!(body("move_work").find("push_block::install(").unwrap() < body("move_work").find("stash\", \"pop").unwrap(), "moved work lands in a blocked worktree");
    }

    #[test]
    fn a_tasks_start_line_carries_its_settings_as_a_short_path_to_the_same_json() {
        let dir = std::env::temp_dir().join(format!("kebbie-task-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kebbie = format!("/Users/someone-with-a-long-name/projects/{}/target/release/kebbie", "nested/".repeat(20));
        let file = crate::lead::write_settings(&dir, &peers::address("billing", "app"), &settings(&kebbie, "billing@app", None, None, None)).unwrap();
        assert_eq!(file, dir.join("billing@app.settings.json"));
        let args = claude_args(Some("acceptEdits"), &file, &dir.join("billing@app.md"), &Session::New("0b6a".to_string()));
        let line = args.join(" ");
        assert!(!line.contains('{') && line.len() < 512, "{line}");
        assert_eq!(args[..2], ["--permission-mode", "acceptEdits"]);
        assert_eq!(args[args.iter().position(|arg| arg == "--settings").unwrap() + 1], file.to_string_lossy());
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written, serde_json::from_str::<Value>(&settings(&kebbie, "billing@app", None, None, None)).unwrap());
        let deny: Vec<&str> = written["permissions"]["deny"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        for rule in ["Bash(git push:*)".to_string(), format!("Bash({kebbie} approve:*)"), format!("Bash({kebbie} push:*)"), format!("Bash({kebbie} choose:*)"), format!("Bash({kebbie} after:*)"), format!("Bash({kebbie} ci dismiss:*)"), "Bash(herdr pane send-keys:*)".to_string(), "Bash(herdr pane send-text:*)".to_string(), "Bash(herdr pane run:*)".to_string(), "Bash(herdr agent send-keys:*)".to_string(), "Bash(herdr agent prompt:*)".to_string()] {
            assert!(deny.contains(&rule.as_str()), "a task never pushes, approves its own commits, answers a Claude's prompt nor types into a pane: {rule}");
        }
        assert_eq!(written["hooks"]["Stop"][0]["hooks"][0]["command"], format!("'{kebbie}' hook"));
        assert!(written["permissions"]["allow"].as_array().unwrap().contains(&json!(format!("Bash({kebbie} ask:*)"))), "a task asks the user without a permission prompt");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_restart_sends_the_stored_prompt_to_a_new_or_unheard_conversation_and_a_given_prompt_always() {
        let dir = crate::sh::test_dir("task-prompt");
        let file = prompt_file(&dir.join("sessions").join("billing@my.app"));
        assert_eq!(file, dir.join("sessions").join("billing@my.app.prompt"));
        assert_eq!(stored_prompt(&file), None);
        store_prompt(&file, Some("Fix the invoice rounding")).unwrap();
        assert_eq!(stored_prompt(&file).as_deref(), Some("Fix the invoice rounding"));
        let (new, resume) = (Session::New("1f0c".to_string()), Session::Resume("1f0c".to_string()));
        assert_eq!(prompt_for(&new, None, stored_prompt(&file), true).as_deref(), Some("Fix the invoice rounding"));
        assert_eq!(prompt_for(&resume, None, stored_prompt(&file), false), None, "a resumed conversation that heard its prompt already has it");
        assert_eq!(prompt_for(&resume, None, stored_prompt(&file), true).as_deref(), Some("Fix the invoice rounding"), "a first start whose shell asked a question left a conversation with no user turn");
        assert_eq!(prompt_for(&resume, Some("Use the new card"), stored_prompt(&file), false).as_deref(), Some("Use the new card"), "a prompt given on a resume is never dropped");
        store_prompt(&file, None).unwrap();
        assert_eq!(prompt_for(&new, None, stored_prompt(&file), true), None);
        std::fs::remove_dir_all(dir).unwrap();
        let source = include_str!("launch.rs");
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).or_else(|| source.find(&format!("\npub(super) fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("launch_new").find("store_prompt(").unwrap() < body("launch_new").find("launch(").unwrap(), "the prompt is kept even when the first start fails");
        assert!(body("begin").contains("restart(&place(&existing), &existing, focus, prompt)"), "start's prompt reaches the resume path");
        assert!(body("restart").contains("prompt_for(&session, given, stored, unheard)") && body("restart").contains("send_prompt(agent.name(), started, prompt.as_deref(),"));
        assert!(body("restart").contains("unheard_in(agent, &file)"));
        assert!(body("forget_session").contains("prompt_file("));
    }

    #[test]
    fn a_new_task_names_its_session_and_a_restart_resumes_it_with_the_same_role_and_settings() {
        let (settings, role) = (Path::new("/state/roles/billing@app.settings.json"), Path::new("/state/roles/billing@app.md"));
        let new = claude_args(Some("acceptEdits"), settings, role, &Session::New("1f0c".to_string()));
        let resumed = claude_args(Some("acceptEdits"), settings, role, &Session::Resume("1f0c".to_string()));
        assert_eq!(new[new.len() - 2..], ["--session-id", "1f0c"]);
        assert_eq!(resumed[resumed.len() - 2..], ["--resume", "1f0c"]);
        assert_eq!(new[..new.len() - 2], resumed[..resumed.len() - 2]);
        assert!(resumed.contains(&"--append-system-prompt-file".to_string()) && resumed.contains(&"--settings".to_string()));
        assert!(!resumed.iter().any(|arg| arg == "--continue" || arg == "-p"));
        let id = new_session_id().unwrap();
        assert_eq!((id.len(), id.matches('-').count(), &id[14..15]), (36, 4, "4"));
        assert_ne!(id, new_session_id().unwrap());
    }

    #[test]
    fn a_restart_resumes_the_conversation_recorded_for_the_task_or_else_the_newest_one_since_its_branch_began() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let home = dir.with_extension("claude");
        let transcripts = transcripts_dir(&home, &slot.path);
        assert_eq!(transcripts.parent().unwrap(), home.join("projects"));
        assert!(!transcripts.file_name().unwrap().to_string_lossy().contains(['/', '.']));
        std::fs::create_dir_all(&transcripts).unwrap();
        assert_eq!(previous_session(Agent::Claude, &dir, &slot, "billing", &home, None), None);
        let created = branch_created(&dir, "billing").unwrap();
        let write = |id: &str, at: std::time::SystemTime| {
            let file = std::fs::File::create(transcripts.join(format!("{id}.jsonl"))).unwrap();
            file.set_modified(at).unwrap();
        };
        write("previous-task", created - Duration::from_secs(60));
        assert_eq!(previous_session(Agent::Claude, &dir, &slot, "billing", &home, None), None);
        write("first", created + Duration::from_secs(10));
        write("latest", created + Duration::from_secs(20));
        assert_eq!(previous_session(Agent::Claude, &dir, &slot, "billing", &home, None).as_deref(), Some("latest"));
        assert_eq!(previous_session(Agent::Claude, &dir, &slot, "billing", &home, Some("first".to_string())).as_deref(), Some("first"));
        assert_eq!(previous_session(Agent::Claude, &dir, &slot, "billing", &home, Some("gone".to_string())).as_deref(), Some("latest"));
        std::fs::remove_dir_all(home).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_codex_task_resumes_the_session_its_hook_recorded_or_the_newest_rollout_begun_in_its_worktree() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let home = dir.with_extension("codex");
        let day = home.join("sessions/2026/10/22");
        std::fs::create_dir_all(&day).unwrap();
        let created = branch_created(&dir, "billing").unwrap();
        let write = |id: &str, cwd: &Path, at: std::time::SystemTime| {
            let file = day.join(format!("rollout-2026-10-22T09-00-00-{id}.jsonl"));
            std::fs::write(&file, format!("{}\n", json!({ "timestamp": "2026-10-22T09:00:00.000Z", "type": "session_meta", "payload": { "id": id, "cwd": cwd } }))).unwrap();
            std::fs::File::options().append(true).open(&file).unwrap().set_modified(at).unwrap();
        };
        write("0000-other-folder", Path::new("/elsewhere"), created + Duration::from_secs(30));
        assert_eq!(previous_session(Agent::Codex, &dir, &slot, "billing", &home, None), None, "a conversation in another folder is not the task's");
        write("0001-first", &slot.path, created + Duration::from_secs(10));
        write("0002-latest", &slot.path, created + Duration::from_secs(20));
        assert_eq!(previous_session(Agent::Codex, &dir, &slot, "billing", &home, None).as_deref(), Some("0002-latest"));
        assert_eq!(previous_session(Agent::Codex, &dir, &slot, "billing", &home, Some("0001-first".to_string())).as_deref(), Some("0001-first"));
        assert_eq!(previous_session(Agent::Codex, &dir, &slot, "billing", &home, Some("gone".to_string())).as_deref(), Some("0002-latest"));
        std::fs::remove_dir_all(home).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_resumed_conversation_hears_its_prompt_again_only_when_kebbie_can_read_it_had_none() {
        let dir = crate::sh::test_dir("unheard");
        let empty = dir.join("rollout-x-1.jsonl");
        std::fs::write(&empty, "").unwrap();
        let heard = dir.join("rollout-x-2.jsonl");
        std::fs::write(&heard, json!({ "timestamp": "2026-10-22T09:00:00.000Z", "type": "event_msg", "payload": { "type": "user_message", "message": "fix it" } }).to_string()).unwrap();
        let packed = dir.join("rollout-x-3.jsonl.zst");
        std::fs::write(&packed, [0x28, 0xb5, 0x2f, 0xfd]).unwrap();
        assert!(unheard_in(Agent::Codex, &empty));
        assert!(!unheard_in(Agent::Codex, &heard));
        assert!(!unheard_in(Agent::Codex, &packed), "a compressed rollout is resumed without sending the prompt twice");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_session_codex_picked_is_written_into_a_codex_tasks_record_and_never_a_claude_tasks() {
        let state = crate::sh::test_dir("codex-record");
        let codex = Record { session: None, mode: Some("acceptEdits".to_string()), profile: Some("coder".to_string()), agent: Some("codex".to_string()) };
        crate::state::write(&session_file_in(&state, "billing@app"), record_text(&codex)).unwrap();
        record_session_in(&state, "billing@app", "019b3136-fdf0").unwrap();
        assert_eq!(parse_record(&std::fs::read_to_string(session_file_in(&state, "billing@app")).unwrap()), Record { session: Some("019b3136-fdf0".to_string()), ..codex }, "the mode and profile stay");
        let claude = Record { session: Some("kebbies-own-id".to_string()), agent: Some("claude".to_string()), ..Record::default() };
        crate::state::write(&session_file_in(&state, "search@app"), record_text(&claude)).unwrap();
        record_session_in(&state, "search@app", "other").unwrap();
        assert_eq!(parse_record(&std::fs::read_to_string(session_file_in(&state, "search@app")).unwrap()).session.as_deref(), Some("kebbies-own-id"));
        record_session_in(&state, "nobody@app", "x").unwrap();
        assert!(!session_file_in(&state, "nobody@app").exists(), "a session outside kebbie's tasks writes no record");
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_transcripts_folder_is_the_worktree_path_with_every_other_character_as_a_dash() {
        assert_eq!(transcripts_dir(Path::new("/home/ada/.claude"), Path::new("/src/wt1.app")), PathBuf::from("/home/ada/.claude/projects/-src-wt1-app"));
        assert_eq!(transcripts_dir(Path::new("/c"), Path::new("/Users/Ada Ö/x_y")).file_name().unwrap(), "-Users-Ada---x-y");
        assert!(!transcripts_dir(Path::new("/c"), Path::new("/a/../b")).file_name().unwrap().to_string_lossy().contains('.'));
    }

    #[test]
    fn a_tasks_settings_deny_push_and_approve_in_every_spelling_claude_code_matches() {
        let on: Value = serde_json::from_str(&settings("/bin/kebbie", "billing@app", Some("billing@app"), None, None)).unwrap();
        let deny: Vec<&str> = on["permissions"]["deny"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        let user_only: Vec<String> = crate::permit::USER_ONLY.iter().map(|command| format!("Bash(/bin/kebbie {command}:*)")).collect();
        assert_eq!(deny[0], "Bash(git push:*)");
        assert_eq!(deny[1..=user_only.len()], user_only.iter().map(String::as_str).collect::<Vec<_>>()[..], "every user-only command, in kebbie's own spelling");
        assert_eq!(deny[user_only.len() + 1..user_only.len() + 6], ["Bash(herdr pane send-keys:*)", "Bash(herdr pane send-text:*)", "Bash(herdr pane run:*)", "Bash(herdr agent send-keys:*)", "Bash(herdr agent prompt:*)"]);
        for command in ["approve", "push", "choose", "after", "ci dismiss", "issue report"] {
            assert!(deny.contains(&format!("Bash(/bin/kebbie {command}:*)").as_str()), "{command}");
        }
        let allow: Vec<&str> = on["permissions"]["allow"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(!allow.iter().any(|rule| rule.contains("push") || rule.contains("approve") || rule.contains("decide") || rule.contains("away")), "{allow:?}");
        assert_eq!(on["env"]["GIT_CONFIG_KEY_0"], "url.kebbie-blocks-push://.pushInsteadOf");
        assert_eq!((on["env"]["GIT_CONFIG_KEY_1"].as_str(), on["env"]["GIT_CONFIG_VALUE_1"].as_str()), (Some("protocol.kebbie-blocks-push.allow"), Some("never")), "git refuses the rewritten URL before it starts ssh or a helper");
        assert_eq!(on["env"]["GIT_CONFIG_VALUE_0"], "");
        assert_eq!((on["env"]["GIT_CONFIG_COUNT"].as_str(), on["env"]["GIT_CONFIG_KEY_2"].as_str(), on["env"]["GIT_CONFIG_VALUE_2"].as_str()), (Some("6"), Some("url./.pushInsteadOf"), Some("/")), "a push to a local path, as a test's temp repo, goes through");
        assert_eq!(on["env"]["VAULT_STOP_HOOK"], "off", "the vault plugin's Stop hook never asks a task for the repo log kebbie writes");
        assert_eq!(on["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 40);
    }

    #[test]
    fn a_task_takes_its_mode_from_start_then_its_recorded_session_then_the_config() {
        assert_eq!(chosen_mode(None, None, None), None, "without any, Claude's own default applies");
        assert_eq!(chosen_mode(None, None, Some(BYPASS)), Some(BYPASS));
        assert_eq!(chosen_mode(Some("acceptEdits"), None, Some(BYPASS)), Some("acceptEdits"), "--mode wins over task.permission_mode");
        assert_eq!(chosen_mode(None, Some("plan"), Some(BYPASS)), Some("plan"), "a restart keeps the mode its conversation started in");
        assert_eq!(chosen_mode(Some(BYPASS), Some("plan"), None), Some(BYPASS));
        let source = include_str!("launch.rs");
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("launch_new").contains("mode: chosen_mode(mode, None, configured.as_deref())") && body("launch_new").contains("configured_mode(repo)?"));
        assert!(body("restart").contains("chosen_mode(place.mode, mode.as_deref(), configured.as_deref())"));
        assert!(body("begin").find("config::permission_mode").unwrap() < body("begin").find("fetch(").unwrap(), "a mistyped --mode is refused before a worktree is claimed");
        assert!(body("launch").contains("Started::BypassDisabled { None }"), "a Claude that policy keeps out of bypass is not recorded as bypassing");
    }

    #[test]
    fn a_task_started_in_bypass_permissions_still_never_pushes_nor_approves() {
        let dir = crate::sh::test_dir("task-bypass");
        let file = crate::lead::write_settings(&dir, "billing@app", &settings("/bin/kebbie", "billing@app", None, None, None)).unwrap();
        let args = claude_args(Some(BYPASS), &file, &dir.join("billing@app.md"), &Session::New("0b6a".to_string()));
        assert_eq!(args[..2], ["--permission-mode", "bypassPermissions"]);
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&args[args.iter().position(|arg| arg == "--settings").unwrap() + 1]).unwrap()).unwrap();
        let deny: Vec<&str> = written["permissions"]["deny"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        for rule in ["Bash(git push:*)", "Bash(/bin/kebbie approve:*)", "Bash(/bin/kebbie push:*)"] {
            assert!(deny.contains(&rule), "deny rules hold in every mode, bypassPermissions too: {deny:?}");
        }
        assert_eq!(written["env"]["GIT_CONFIG_KEY_0"], "url.kebbie-blocks-push://.pushInsteadOf");
        let source = include_str!("launch.rs");
        let prepare = &source[source.find("\nfn prepare(").unwrap()..];
        assert!(!prepare[..prepare.find("\n}\n").unwrap()].contains("mode"), "a task's settings are the same whatever its mode");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_board_reads_a_bypassing_task_from_the_mode_its_session_recorded() {
        let state = crate::sh::test_dir("task-bypass-mark");
        assert!(!bypasses_in(&state, "billing@app"), "no recorded session, no mark");
        std::fs::create_dir_all(state.join("sessions")).unwrap();
        for (record, marked) in [("1f0c\nbypassPermissions\n", true), ("1f0c\n\n", false), ("1f0c\nacceptEdits\n", false), ("1f0c\n", false)] {
            std::fs::write(session_file_in(&state, "billing@app"), record).unwrap();
            assert_eq!(bypasses_in(&state, "billing@app"), marked, "{record:?}");
        }
        assert!(!bypasses_in(&state, "search@app"));
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_policy_that_disables_bypass_is_said_and_the_prompt_still_goes_in() {
        let typed = std::cell::RefCell::new(Vec::new());
        let said = send_prompt("Claude", &Started::BypassDisabled, Some("Fix the flaky test"), |text| { typed.borrow_mut().push(text.to_string()); Ok(()) }, |_| panic!("nothing is held for a running Claude")).unwrap();
        assert_eq!(typed.into_inner(), ["Fix the flaky test"]);
        let detail = said.detail.unwrap();
        assert!(detail.contains("disable bypass permissions mode") && detail.contains("permissions.disableBypassPermissionsMode"), "{detail}");
        let held = std::cell::RefCell::new(Vec::new());
        let said = send_prompt("Claude", &Started::AwaitsBypass, Some("Fix the flaky test"), |_| panic!("no key reaches the bypass warning"), |text| { held.borrow_mut().push(text.to_string()); Ok(()) }).unwrap();
        assert_eq!(held.into_inner(), ["Fix the flaky test"]);
        assert_eq!(said.you.as_deref(), Some("Answer Claude's question in its Space: whether to accept bypass permissions mode."));
        assert!(said.next.unwrap().starts_with("Once you answer, kebbie types"));
    }

    #[test]
    fn the_hook_commands_run_from_a_path_with_an_apostrophe() {
        let dir = std::env::temp_dir().join(format!("kebbie-hook-quote-{}", std::process::id())).join("o'brien");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let kebbie = dir.join("kebbie");
        crate::executable::write(&kebbie, "#!/bin/sh\necho ran \"$@\"\n").unwrap();
        let written: Value = serde_json::from_str(&settings(&kebbie.to_string_lossy(), "billing@it's", Some("billing@it's"), None, None)).unwrap();
        for command in [&written["hooks"]["Stop"][0]["hooks"][0]["command"], &written["hooks"]["PreToolUse"][0]["hooks"][0]["command"], &written["hooks"]["PostToolUse"][0]["hooks"][0]["command"]] {
            let output = std::process::Command::new("sh").args(["-c", command.as_str().unwrap()]).output().unwrap();
            assert!(output.status.success(), "{command}: {}", String::from_utf8_lossy(&output.stderr));
            assert!(String::from_utf8_lossy(&output.stdout).starts_with("ran hook"), "{command}");
        }
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_tasks_session_record_keeps_its_profile_and_an_old_two_line_record_reads_as_task_on_claude() {
        let designer = Record { session: Some("1f0c".to_string()), mode: Some(BYPASS.to_string()), profile: Some("designer".to_string()), agent: Some("claude".to_string()) };
        assert_eq!(parse_record(&record_text(&designer)), designer);
        let old = |session: &str, mode: Option<&str>| Record { session: Some(session.to_string()), mode: mode.map(str::to_string), profile: Some("task".to_string()), agent: Some("claude".to_string()) };
        assert_eq!(parse_record("1f0c\nacceptEdits\n"), old("1f0c", Some("acceptEdits")));
        assert_eq!(parse_record("1f0c\n\n"), old("1f0c", None));
        assert_eq!(parse_record("1f0c\n"), old("1f0c", None));
        assert_eq!(parse_record(&record_text(&Record { mode: None, ..designer.clone() })).mode, None);
        assert_eq!(Record::default().agent(), Agent::Claude);
        let state = crate::sh::test_dir("task-record");
        std::fs::create_dir_all(state.join("sessions")).unwrap();
        std::fs::write(session_file_in(&state, "billing@app"), record_text(&designer)).unwrap();
        assert!(bypasses_in(&state, "billing@app"), "the board reads bypass from the new record too");
        std::fs::remove_dir_all(state).unwrap();
        let source = include_str!("launch.rs");
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).or_else(|| source.find(&format!("\npub(crate) fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("launch").contains("profile: Some(profile.name.clone())") && body("launch").contains("record_text(&record)"), "every start records the profile it ran with");
        assert!(body("begin").contains("recorded_session(&task, &repo).profile") && body("begin").find("task_profile(").unwrap() < body("begin").find("fetch(").unwrap(), "a running task keeps its profile, and a missing one is refused before a worktree is claimed");
        assert!(body("task_role").contains("recorded_session(&task, &repo).profile"), "kebbie restart writes the role and settings of the profile the task started with");
        assert!(body("begin").contains("detail(\"careful\", why)") && body("task_role").contains("messages::to_lead(messages::ToLead::TaskRestarted, &said)"), "a recorded profile that no longer loads is said where the restart reports, and the restart goes on");
    }

    #[test]
    fn a_task_started_with_a_profile_runs_its_model_effort_mode_and_rules_on_top_of_kebbies_own() {
        let task = profiles::built_in(profiles::TASK).unwrap();
        let designer = Profile {
            name: "designer".to_string(), model: Some("opus".to_string()), effort: Some("high".to_string()), permission_mode: Some("acceptEdits".to_string()),
            deny: vec!["Bash(rm -rf:*)".to_string()], allow: vec!["Bash(npx playwright:*)".to_string()], plugins: vec!["design@claude-plugins".to_string()], ..task.clone()
        };
        let plain: Value = serde_json::from_str(&crate::inbox::with_inbox(&settings("/bin/kebbie", "billing@app", Some("billing@app"), None, None), "/bin/kebbie", "billing@app")).unwrap();
        let ours: Value = serde_json::from_str(&Agent::Claude.task_settings("/bin/kebbie", "billing@app", Some("billing@app"), None, &task, false)).unwrap();
        assert_eq!(ours, plain, "the built-in task's settings are kebbie's own, nothing added");
        assert_eq!((&ours["crossSessionInbound"], &ours["hooks"]["SessionStart"][0]["hooks"][0]["command"]), (&json!("accept"), &json!("'/bin/kebbie' hook inbox 'billing@app'")), "a task takes kebbie's messages in its inbox, whatever its permission mode");
        let written: Value = serde_json::from_str(&Agent::Claude.task_settings("/bin/kebbie", "billing@app", Some("billing@app"), None, &designer, false)).unwrap();
        let deny = written["permissions"]["deny"].as_array().unwrap();
        let kebbies = plain["permissions"]["deny"].as_array().unwrap();
        assert_eq!(deny[..kebbies.len()], kebbies[..], "kebbie's deny rules stay, first and whole");
        assert_eq!(deny[kebbies.len()..], [json!("Bash(rm -rf:*)")]);
        assert!(written["permissions"]["allow"].as_array().unwrap().contains(&json!("Bash(npx playwright:*)")) && written["permissions"]["allow"].as_array().unwrap().contains(&json!("Bash(/bin/kebbie ask:*)")));
        assert_eq!((&written["hooks"], &written["env"]), (&plain["hooks"], &plain["env"]), "the hooks and the push block's env are kebbie's whatever the profile");
        assert_eq!(written["enabledPlugins"], json!({ "design@claude-plugins": true }));
        let configured = designer.permission_mode.clone().or(Some("plan".to_string()));
        assert_eq!(chosen_mode(None, None, configured.as_deref()), Some("acceptEdits"), "the profile's mode goes ahead of task.permission_mode");
        assert_eq!(chosen_mode(Some(BYPASS), None, configured.as_deref()), Some(BYPASS), "--mode goes ahead of the profile's");
        let (file, role) = (Path::new("/state/roles/billing@app.settings.json"), Path::new("/state/roles/billing@app.md"));
        let files = TaskFiles { address: "billing@app", settings: file, role, sandboxed: true, tmp: None };
        let args = Agent::Claude.start_args(chosen_mode(None, None, configured.as_deref()), &designer, &files, &Session::New("1f0c".to_string()));
        assert_eq!(args, ["--permission-mode", "acceptEdits", "--model", "opus", "--effort", "high", "--settings", "/state/roles/billing@app.settings.json", "--append-system-prompt-file", "/state/roles/billing@app.md", "--session-id", "1f0c"]);
        assert_eq!(Agent::Claude.start_args(None, &task, &files, &Session::Resume("1f0c".to_string())), claude_args(None, file, role, &Session::Resume("1f0c".to_string())));
        assert!(!claude_args(None, file, role, &Session::Resume("1f0c".to_string())).iter().any(|arg| arg == "--model" || arg == "--effort"), "the built-in task leaves the model to Claude");
        let source = include_str!("launch.rs");
        let prepare = &source[source.find("\nfn prepare(").unwrap()..];
        let prepare = &prepare[..prepare.find("\n}\n").unwrap()];
        assert!(prepare.contains("profile.agent.task_settings(") && prepare.contains("profiles::role_section(profile)?") && prepare.contains("profile: &added"), "the profile's rules reach the settings and its role and skills the role file");
        assert!(include_str!("../peers.rs").contains("write_role(&file, &template, &pairs, place.profile)"), "the profile's section goes after kebbie's own role");
    }

    #[test]
    fn a_task_runs_sandboxed_unless_its_profile_or_the_config_turns_it_off_and_never_on_a_machine_that_cannot() {
        let task = profiles::built_in(profiles::TASK).unwrap();
        let none = || Vec::new();
        assert_eq!(sandboxed(true, &task, none), Ok(true));
        assert_eq!(sandboxed(false, &task, || panic!("an unsandboxed task needs no check")), Ok(false));
        let off = Profile { sandbox: crate::sandbox::Policy { enabled: Some(false), ..Default::default() }, ..task.clone() };
        assert_eq!(sandboxed(true, &off, || panic!("a profile that turns it off needs no check")), Ok(false), "the profile wins over task.sandbox");
        let on = Profile { sandbox: crate::sandbox::Policy { enabled: Some(true), ..Default::default() }, ..task.clone() };
        assert_eq!(sandboxed(false, &on, none), Ok(true));
        let refused = sandboxed(true, &task, || vec!["bwrap".to_string(), "the seccomp filter".to_string()]).unwrap_err();
        assert!(refused.contains("cannot sandbox a task's shell commands fully") && refused.contains("bubblewrap") && refused.contains("@anthropic-ai/sandbox-runtime") && refused.contains("task.sandbox = false"), "{refused}");
        let source = include_str!("launch.rs");
        let body = |name: &str| { let start = source.find(&format!("\nfn {name}(")).or_else(|| source.find(&format!("\npub fn {name}("))).unwrap(); &source[start..start + source[start..].find("\n}\n").unwrap()] };
        assert!(body("begin").find("sandboxed(").unwrap() < body("begin").find("claim_slot(").unwrap(), "a machine that cannot sandbox claims no worktree");
        assert!(body("prepare").find("sandboxed(").unwrap() < body("prepare").find("task_settings(").unwrap(), "a restart is checked and written as a start is");
    }

    #[test]
    fn the_start_says_when_the_sandbox_is_off_and_what_a_profile_runs_outside_it() {
        let task = profiles::built_in(profiles::TASK).unwrap();
        assert_eq!(sandbox_said(true, &task), None);
        assert!(sandbox_said(false, &task).unwrap().starts_with("off (task.sandbox = false)"));
        let ops = sandbox_said(true, &profiles::built_in(profiles::OPS).unwrap()).unwrap();
        assert_eq!(ops, "ssh *, scp *, rsync * run outside the sandbox in this task: they reach other machines, and this one as you");
    }
}
