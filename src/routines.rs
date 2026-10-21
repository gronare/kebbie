use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use toml::Table;

use crate::permit::Caller;
use crate::repo::{ahead_behind, changed_count, default_branch, main_checkout, name_of};
use crate::sh::Result;

pub const KEYS: [&str; 9] = ["id", "title", "repo", "every", "at", "prompt", "mode", "push", "enabled"];
pub const EVERY: u64 = 60;
const DAY: u64 = 86_400;
const NAME_LIMIT: usize = 32;
pub const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Mode {
    #[default]
    Report,
    Plan,
    Fix,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Report, Mode::Plan, Mode::Fix];

    pub fn key(self) -> &'static str {
        match self {
            Mode::Report => "report",
            Mode::Plan => "plan",
            Mode::Fix => "fix",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Schedule {
    Every(u64),
    Daily(u32),
    Weekly(u32, u32),
}

impl Schedule {
    pub fn text(self) -> String {
        let clock = |minute: u32| format!("{:02}:{:02}", minute / 60, minute % 60);
        match self {
            Schedule::Every(seconds) if seconds % DAY == 0 => format!("every {}d", seconds / DAY),
            Schedule::Every(seconds) if seconds % 3600 == 0 => format!("every {}h", seconds / 3600),
            Schedule::Every(seconds) => format!("every {}m", seconds / 60),
            Schedule::Daily(minute) => format!("daily at {}", clock(minute)),
            Schedule::Weekly(day, minute) => format!("{} at {}", WEEKDAYS[day as usize], clock(minute)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Routine {
    pub id: String,
    pub title: String,
    pub repo: String,
    pub schedule: Schedule,
    pub prompt: String,
    pub mode: Mode,
    pub push: bool,
    pub enabled: bool,
    pub source: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Ok,
    Findings,
    Failed,
    Running,
}

impl Outcome {
    pub const ALL: [Outcome; 4] = [Outcome::Ok, Outcome::Findings, Outcome::Failed, Outcome::Running];

    pub fn key(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Findings => "findings",
            Outcome::Failed => "failed",
            Outcome::Running => "running",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Past {
    pub start: u64,
    pub outcome: Outcome,
    pub summary: Option<String>,
}

pub const EARLIER: usize = 4;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub seen: Option<u64>,
    pub last_start: Option<u64>,
    pub last_task: Option<String>,
    pub last_outcome: Option<Outcome>,
    pub last_summary: Option<String>,
    pub earlier: Vec<Past>,
}

fn text(table: &Table, field: &str, key: &str) -> Result<Option<String>> {
    match table.get(field) {
        None => Ok(None),
        Some(value) => match value.as_str().map(str::trim) {
            Some("") => Err(format!("{key}: must not be empty")),
            Some(text) => Ok(Some(text.to_string())),
            None => Err(format!("{key}: must be a string")),
        },
    }
}

fn flag(table: &Table, field: &str, key: &str) -> Result<Option<bool>> {
    table.get(field).map(|value| value.as_bool().ok_or(format!("{key}: must be true or false"))).transpose()
}

// The id names the routine's state file and starts its task names, so it is a slug that fits both.
pub fn valid_id(id: &str) -> bool {
    id.len() <= 40 && id.starts_with(|character: char| character.is_ascii_lowercase()) && !id.ends_with('-') && !id.contains("--")
        && id.chars().all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
}

pub fn parse_every(text: &str) -> Result<u64> {
    let refused = || format!("\"{text}\" is not an interval; use minutes, hours or days, as \"30m\", \"6h\" or \"1d\"");
    let unit = match text.chars().last() {
        Some('m') => 60,
        Some('h') => 3600,
        Some('d') => DAY,
        _ => return Err(refused()),
    };
    let count: u64 = text[..text.len() - 1].parse().ok().filter(|count| *count > 0).ok_or_else(refused)?;
    Ok(count * unit)
}

fn clock(text: &str) -> Option<u32> {
    let (hours, minutes) = text.split_once(':')?;
    let valid = (1..=2).contains(&hours.len()) && minutes.len() == 2 && hours.bytes().chain(minutes.bytes()).all(|byte| byte.is_ascii_digit());
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (valid && hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

pub fn parse_at(text: &str) -> Result<Schedule> {
    let refused = || format!("\"{text}\" is not a time; use \"07:00\" for every day, or a weekday and a time, as \"mon 07:00\"");
    let words: Vec<&str> = text.split_whitespace().collect();
    match words[..] {
        [time] => clock(time).map(Schedule::Daily).ok_or_else(refused),
        [day, time] => {
            let day = WEEKDAYS.iter().position(|name| name.eq_ignore_ascii_case(day)).ok_or_else(|| format!("\"{day}\" is not a weekday; use mon, tue, wed, thu, fri, sat or sun"))?;
            clock(time).map(|minute| Schedule::Weekly(day as u32, minute)).ok_or_else(refused)
        }
        _ => Err(refused()),
    }
}

fn routine(table: &Table, index: usize, default_repo: Option<&str>, source: &str) -> Result<Routine> {
    let key = |field: &str| format!("routines[{index}].{field}");
    crate::config::unknown_keys(table, &format!("routines[{index}]."), &KEYS)?;
    let id = text(table, "id", &key("id"))?.ok_or(format!("{}: every routine needs an id", key("id")))?;
    if !valid_id(&id) {
        return Err(format!("{}: \"{id}\" is not an id; use lower-case letters, digits and single dashes, starting with a letter, at most 40", key("id")));
    }
    let title = text(table, "title", &key("title"))?.ok_or(format!("{}: every routine needs a title", key("title")))?;
    let repo = match (text(table, "repo", &key("repo"))?, default_repo) {
        (Some(repo), _) => repo,
        (None, Some(repo)) => repo.to_string(),
        (None, None) => return Err(format!("{}: a routine in config.toml names the repo it runs in, by its folder name or its path", key("repo"))),
    };
    let schedule = match (text(table, "every", &key("every"))?, text(table, "at", &key("at"))?) {
        (Some(_), Some(_)) => return Err(format!("routines[{index}]: every and at are two schedules; give one")),
        (Some(every), None) => Schedule::Every(parse_every(&every).map_err(|error| format!("{}: {error}", key("every")))?),
        (None, Some(at)) => parse_at(&at).map_err(|error| format!("{}: {error}", key("at")))?,
        (None, None) => return Err(format!("routines[{index}]: needs a schedule, every = \"6h\" or at = \"07:00\"")),
    };
    let prompt = text(table, "prompt", &key("prompt"))?.ok_or(format!("{}: every routine needs a prompt, what to check", key("prompt")))?;
    let mode = match text(table, "mode", &key("mode"))? {
        None => Mode::default(),
        Some(name) => Mode::ALL.into_iter().find(|mode| mode.key() == name).ok_or(format!("{}: \"{name}\" is not a mode; use \"report\", \"plan\" or \"fix\"", key("mode")))?,
    };
    let push = flag(table, "push", &key("push"))?.unwrap_or(false);
    if push && mode != Mode::Fix {
        return Err(format!("{}: only a fix routine pushes; set mode = \"fix\" or leave push out", key("push")));
    }
    Ok(Routine { id, title, repo, schedule, prompt, mode, push, enabled: flag(table, "enabled", &key("enabled"))?.unwrap_or(true), source: source.to_string() })
}

// A config file's [[routines]]; a repo config's run in its own repo unless they name another.
pub fn parse_list(table: &Table, default_repo: Option<&str>, source: &str) -> Result<Vec<Routine>> {
    let Some(value) = table.get("routines") else { return Ok(Vec::new()) };
    let items = value.as_array().ok_or("routines: must be [[routines]] tables")?;
    let mut found: Vec<Routine> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let table = item.as_table().ok_or(format!("routines[{index}]: must be a table"))?;
        let routine = routine(table, index, default_repo, source)?;
        if found.iter().any(|other| other.id == routine.id) {
            return Err(format!("routines[{index}].id: \"{}\" is taken by an earlier routine", routine.id));
        }
        found.push(routine);
    }
    Ok(found)
}

fn parse_file(file: &Path, default_repo: Option<&str>) -> Result<Vec<Routine>> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", file.display())),
    };
    let table: Table = text.parse().map_err(|error: toml::de::Error| format!("{}: {}", file.display(), error.to_string().trim()))?;
    parse_list(&table, default_repo, &file.display().to_string()).map_err(|error| format!("{}: {error}", file.display()))
}

// Every routine on the machine: config.toml's, then each repo config's; a broken file is named and the rest still load.
pub fn load_from(machine: &Path, repos: &Path) -> (Vec<Routine>, Vec<String>) {
    let mut files = vec![(machine.to_path_buf(), None)];
    let mut repo_files: Vec<PathBuf> = fs::read_dir(repos).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|extension| extension == "toml")).collect();
    repo_files.sort();
    files.extend(repo_files.into_iter().map(|file| { let stem = file.file_stem().map(|stem| stem.to_string_lossy().to_string()); (file, stem) }));
    load_files(files)
}

// Each file with the repo its routines run in when they name none.
pub fn load_files(files: Vec<(PathBuf, Option<String>)>) -> (Vec<Routine>, Vec<String>) {
    let (mut routines, mut errors): (Vec<Routine>, Vec<String>) = (Vec::new(), Vec::new());
    for (file, stem) in files {
        match parse_file(&file, stem.as_deref()) {
            Ok(found) => for routine in found {
                match routines.iter().find(|other| other.id == routine.id) {
                    Some(other) => errors.push(format!("{}: routine \"{}\" is taken by {}", routine.source, routine.id, other.source)),
                    None => routines.push(routine),
                }
            },
            Err(error) => errors.push(error),
        }
    }
    (routines, errors)
}

pub fn load() -> (Vec<Routine>, Vec<String>) {
    load_from(&crate::config::path(), &crate::config::repos_dir())
}

pub fn local_offset(at: u64) -> i64 {
    let time = at as libc::time_t;
    // SAFETY: tm is plain integers and a pointer, for which all zeros is a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call, and localtime_r writes only into tm.
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() { 0 } else { tm.tm_gmtoff }
}

fn period(schedule: Schedule) -> u64 {
    match schedule {
        Schedule::Every(seconds) => seconds,
        Schedule::Daily(_) => DAY,
        Schedule::Weekly(..) => 7 * DAY,
    }
}

// The latest time at or before now that an at schedule names, in local time at the given offset from UTC.
pub fn latest_slot(schedule: Schedule, now: u64, offset: i64) -> Option<u64> {
    let local = now as i64 + offset;
    let day = local.div_euclid(DAY as i64);
    let (start, minute) = match schedule {
        Schedule::Every(_) => return None,
        Schedule::Daily(minute) => (day, minute),
        Schedule::Weekly(weekday, minute) => (day - (day + 3 - i64::from(weekday)).rem_euclid(7), minute),
    };
    let mut slot = start * DAY as i64 + i64::from(minute) * 60;
    if slot > local {
        slot -= period(schedule) as i64;
    }
    u64::try_from(slot - offset).ok()
}

// A run is due once its interval has passed since the last start, or its time has come since then: after a sleep that is once, however many slots it missed.
pub fn due(schedule: Schedule, state: &State, now: u64, offset: i64) -> bool {
    match schedule {
        Schedule::Every(every) => state.last_start.is_none_or(|last| now >= last + every),
        _ => state.last_start.or(state.seen).is_some_and(|since| latest_slot(schedule, now, offset).is_some_and(|slot| slot > since)),
    }
}

pub fn next_run(schedule: Schedule, state: &State, now: u64, offset: i64) -> u64 {
    if due(schedule, state, now, offset) {
        return now;
    }
    match schedule {
        Schedule::Every(every) => state.last_start.map_or(now, |last| last + every),
        _ => latest_slot(schedule, now, offset).map_or(now, |slot| slot + period(schedule)),
    }
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    Start,
    Wait,
    Open,
}

// Never a second run while the last one's task is still open, whatever the clock says.
pub fn decide(routine: &Routine, state: &State, open: bool, now: u64, offset: i64) -> Decision {
    if open {
        Decision::Open
    } else if routine.enabled && due(routine.schedule, state, now, offset) {
        Decision::Start
    } else {
        Decision::Wait
    }
}

pub fn grants_push(routine: &Routine) -> bool {
    routine.mode == Mode::Fix && routine.push
}

// herdr names an agent with at most 32 characters, so the id is cut to leave room for the date.
pub fn task_name(id: &str, now: u64, offset: i64) -> String {
    let stamp = crate::keeper::stamp(u64::try_from(now as i64 + offset).unwrap_or(now));
    let date = format!("{}{}{}-{}{}", &stamp[0..4], &stamp[5..7], &stamp[8..10], &stamp[11..13], &stamp[14..16]);
    let head: String = id.chars().take(NAME_LIMIT - 1 - date.len()).collect();
    format!("{}-{date}", head.trim_end_matches('-'))
}

pub fn prompt(routine: &Routine, kelpie: &str) -> String {
    let ok = format!("When all is well, run `{kelpie} routine ok \"<one line on what you checked and found in order>\"` as your last command: it records the run as ok and closes this task, so it works only while the task has no changes and no commits.");
    let work = match routine.mode {
        Mode::Report => format!(
            "Investigate and change nothing: no edits, no commits, nothing applied to the systems you look at. {ok} Otherwise write a sign-off card listing each finding (what you saw, where, how serious, what you recommend) and run `{kelpie} report --state needs-you --next \"<the first thing the user should do>\" --card <file> \"<one line on the findings>\"`."
        ),
        Mode::Plan => format!(
            "Investigate and change nothing yet. {ok} Otherwise write a plan for what should change and put it to the user with `{kelpie} ask --plan`, as your role's Questions section says; carry it out only once they approve it, in commits, and report ready as any task does."
        ),
        Mode::Fix => {
            let push = if routine.push {
                "kelpie gave the lead push permission for this task: once you report ready and your commits hold to the rules, the lead pushes them without waiting for the user."
            } else {
                "The user pushes your fix, as with any task."
            };
            format!("Investigate and fix what you find. {ok} Otherwise fix it in commits on this task's branch, run the tests related to the fix, and report `--state ready` with a sign-off card that lists what you found and what you changed. {push}")
        }
    };
    format!(
        "This is the routine \"{}\" ({}), which kelpie runs {} in {}, in {} mode. Nobody typed this prompt: kelpie started this task on the schedule, and the user reads the outcome later.\n\nWhat to check:\n{}\n\n{work}",
        routine.title, routine.id, routine.schedule.text(), routine.repo, routine.mode.key(), routine.prompt,
    )
}

pub fn keep_last(state: &mut State) {
    if let (Some(start), Some(outcome)) = (state.last_start, state.last_outcome) {
        state.earlier.insert(0, Past { start, outcome, summary: state.last_summary.clone() });
        state.earlier.truncate(EARLIER);
    }
}

pub fn state_file(dir: &Path, id: &str) -> PathBuf {
    dir.join("routines").join(format!("{id}.json"))
}

pub fn state_json(state: &State) -> Value {
    let earlier: Vec<Value> = state.earlier.iter().map(|past| json!({ "start": past.start, "outcome": past.outcome.key(), "summary": past.summary })).collect();
    json!({ "seen": state.seen, "last_start": state.last_start, "last_task": state.last_task, "last_outcome": state.last_outcome.map(Outcome::key), "last_summary": state.last_summary, "earlier": earlier })
}

fn outcome_of(value: &Value) -> Option<Outcome> {
    value.as_str().and_then(|key| Outcome::ALL.into_iter().find(|outcome| outcome.key() == key))
}

pub fn state_from(value: &Value) -> State {
    State {
        seen: value["seen"].as_u64(),
        last_start: value["last_start"].as_u64(),
        last_task: value["last_task"].as_str().map(str::to_string),
        last_outcome: outcome_of(&value["last_outcome"]),
        last_summary: value["last_summary"].as_str().map(str::to_string),
        earlier: value["earlier"].as_array().into_iter().flatten().filter_map(|past| Some(Past {
            start: past["start"].as_u64()?, outcome: outcome_of(&past["outcome"])?, summary: past["summary"].as_str().map(str::to_string),
        })).collect(),
    }
}

pub fn load_state(dir: &Path, id: &str) -> State {
    fs::read_to_string(state_file(dir, id)).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).map(|value| state_from(&value)).unwrap_or_default()
}

pub fn save_state(dir: &Path, id: &str, state: &State) -> Result<()> {
    crate::state::write(&state_file(dir, id), state_json(state).to_string())
}

// The routine whose latest run is this task, read from the state files alone, so the board can ask on every refresh.
pub fn run_of(dir: &Path, address: &str) -> Option<(String, State)> {
    fs::read_dir(dir.join("routines")).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        let id = path.file_name()?.to_str()?.strip_suffix(".json")?.to_string();
        let state = load_state(dir, &id);
        (state.last_task.as_deref() == Some(address)).then_some((id, state))
    })
}

pub fn routine_of(address: &str) -> Option<String> {
    run_of(&crate::lead::state_location().ok()?, address).map(|(id, _)| id)
}

// A report or a question from a routine's task that has not said ok: the first one records the findings and is what the user hears.
pub fn found_in(dir: &Path, address: &str, summary: &str) -> bool {
    let Some((id, mut state)) = run_of(dir, address) else { return false };
    let first = state.last_outcome == Some(Outcome::Running);
    if first || state.last_outcome == Some(Outcome::Findings) {
        state.last_outcome = Some(Outcome::Findings);
        state.last_summary = Some(summary.to_string());
        let _ = save_state(dir, &id, &state);
    }
    first
}

pub fn found(address: &str, summary: &str) -> bool {
    crate::lead::state_dir().is_ok_and(|dir| found_in(&dir, address, summary))
}

pub fn repo_path(repo: &str) -> Result<PathBuf> {
    let path = match repo.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None if repo.contains('/') => PathBuf::from(repo),
        None => crate::repo::checkout_named(repo).ok_or(format!("repo {repo}: no checkout of that name under your repo folders"))?,
    };
    main_checkout(&path).map_err(|_| format!("repo {repo}: {} is not a git checkout", path.display()))
}

fn open(routine: &Routine, address: &str) -> bool {
    let task = address.split_once('@').map_or(address, |(task, _)| task);
    repo_path(&routine.repo).and_then(|repo| crate::tasks::slots(&repo)).is_ok_and(|slots| slots.iter().any(|slot| slot.task.as_deref() == Some(task)))
}

fn notify(cwd: &Path, event: crate::notify::Event, address: &str, text: String) {
    crate::notify::send(cwd, &crate::notify::Notice::from(event, address, text));
}

// One routine's run: its state names the task before the task starts, so the task's first report already finds its routine.
fn start_run(routine: &Routine, dir: &Path, now: u64, offset: i64) -> Result<String> {
    let mut state = load_state(dir, &routine.id);
    let task = task_name(&routine.id, now, offset);
    keep_last(&mut state);
    state.last_start = Some(now);
    state.last_task = None;
    state.last_outcome = Some(Outcome::Running);
    state.last_summary = None;
    let repo = repo_path(&routine.repo);
    let address = repo.as_ref().ok().map(|repo| crate::peers::address(&task, &name_of(repo)));
    state.last_task.clone_from(&address);
    save_state(dir, &routine.id, &state)?;
    let started = repo.and_then(|repo| {
        let address = address.clone().unwrap_or_default();
        if grants_push(routine) {
            crate::permit::set_task_in(dir, &address, true)?;
        }
        let prompt = prompt(routine, &crate::peers::kelpie_command()?);
        let options = crate::tasks::Start { prompt: Some(&prompt), focus: false, from: None, mode: Some("auto"), backlog: None, note: false };
        crate::tasks::start(&repo, &task, &options).map(|said| (repo, said))
    });
    match started {
        Ok((_, said)) => Ok(format!("routine {}: {said}", routine.id)),
        Err(error) => {
            state.last_outcome = Some(Outcome::Failed);
            state.last_summary = Some(error.clone());
            save_state(dir, &routine.id, &state)?;
            let cwd = repo_path(&routine.repo).unwrap_or_else(|_| dir.to_path_buf());
            notify(&cwd, crate::notify::Event::RoutineFailed, address.as_deref().unwrap_or(&routine.id), format!("routine {} failed to start: {error}", routine.title));
            Err(format!("routine {}: {error}", routine.id))
        }
    }
}

// Two boards, or a board and a keeper handing over, never start the same run twice.
struct Lock {
    _file: File,
}

impl Lock {
    fn take(dir: &Path) -> Option<Lock> {
        crate::state::private_dir(&dir.join("routines")).ok()?;
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(dir.join("routines").join("lock")).ok()?;
        // SAFETY: flock only reads the descriptor, which the File keeps open; the lock goes with the File.
        (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(Lock { _file: file })
    }
}

// One round of the scheduler: a run whose task closed without a word failed, and each due routine with no open run starts one.
pub fn tick(dir: &Path) -> Vec<String> {
    let Some(_lock) = Lock::take(dir) else { return Vec::new() };
    let (routines, _) = load();
    let now = crate::messages::now();
    let offset = local_offset(now);
    let mut notes = Vec::new();
    for routine in &routines {
        let mut state = load_state(dir, &routine.id);
        if state.seen.is_none() {
            state.seen = Some(now);
            let _ = save_state(dir, &routine.id, &state);
        }
        let task = state.last_task.clone();
        let open = task.as_deref().is_some_and(|address| open(routine, address));
        if !open && state.last_outcome == Some(Outcome::Running) {
            state.last_outcome = Some(Outcome::Failed);
            state.last_summary = Some("its task closed before it reported".to_string());
            let _ = save_state(dir, &routine.id, &state);
            let address = task.unwrap_or_default();
            notify(&repo_path(&routine.repo).unwrap_or_else(|_| dir.to_path_buf()), crate::notify::Event::RoutineFailed, &address, format!("routine {} failed: {address} closed before it reported", routine.title));
            notes.push(format!("routine {}: {address} closed before it reported", routine.id));
        }
        if decide(routine, &state, open, now, offset) == Decision::Start {
            notes.push(start_run(routine, dir, now, offset).unwrap_or_else(|error| error));
        }
    }
    notes
}

pub fn tick_cli() -> Result<String> {
    let notes = tick(&crate::lead::state_dir()?);
    Ok(if notes.is_empty() { "routines: nothing was due".to_string() } else { notes.join("\n") })
}

pub fn run_now(id: &str) -> Result<String> {
    let dir = crate::lead::state_dir()?;
    let (routines, _) = load();
    let routine = routines.iter().find(|routine| routine.id == id).ok_or(format!("No routine \"{id}\"; kelpie routines lists them"))?;
    let _lock = Lock::take(&dir).ok_or("another kelpie is starting routines right now; try again in a moment")?;
    let state = load_state(&dir, id);
    if let Some(task) = state.last_task.as_deref().filter(|address| open(routine, address)) {
        return Err(format!("{task}, the last run of {id}, is still open; a routine runs again once its task is closed"));
    }
    let now = crate::messages::now();
    start_run(routine, &dir, now, local_offset(now))
}

pub fn ok_allowed(caller: &Caller, me: &str) -> Result<()> {
    match caller {
        Caller::Task(address) if address == me => Ok(()),
        other => Err(format!("kelpie routine ok is the word of the routine's own task, {me}; {} cannot say it", crate::permit::describe(other))),
    }
}

// Ok closes the task as drop does, so it holds only for a run that changed nothing.
pub fn ok(cwd: &Path, summary: &str) -> Result<String> {
    let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        return Err("kelpie routine ok needs a one-line summary: kelpie routine ok \"<what you checked and found in order>\"".to_string());
    }
    let me = crate::peers::me(cwd).filter(|me| me != crate::status::LEAD_LABEL).ok_or("kelpie routine ok runs in a routine's task")?;
    let dir = crate::lead::state_dir()?;
    let (id, mut state) = run_of(&dir, &me).ok_or(format!("{me} is not a routine's task; report as any task does"))?;
    ok_allowed(&crate::permit::caller(), &me)?;
    let repo = main_checkout(cwd)?;
    let slot = crate::tasks::find_task(&repo, None, cwd)?;
    let (ahead, _) = ahead_behind(&slot.path, &default_branch(&repo)?)?;
    let changed = changed_count(&slot.path)?;
    if ahead > 0 || changed > 0 {
        return Err(format!("{me} has {changed} uncommitted change(s) and {ahead} commit(s): a run that changed something reports what it found instead of ok"));
    }
    state.last_outcome = Some(Outcome::Ok);
    state.last_summary = Some(summary);
    save_state(&dir, &id, &state)?;
    crate::tasks::drop(cwd, None, false).map(|said| format!("Recorded {id} as ok. {said}"))
}

fn ago(seconds: u64) -> String {
    format!("{} ago", crate::keeper::span(seconds))
}

pub fn row_json(routine: &Routine, state: &State, open: bool, now: u64, offset: i64) -> Value {
    json!({
        "id": routine.id, "title": routine.title, "repo": routine.repo, "schedule": routine.schedule.text(), "mode": routine.mode.key(), "push": routine.push,
        "enabled": routine.enabled, "source": routine.source, "open": open,
        "next_run": routine.enabled.then(|| next_run(routine.schedule, state, now, offset)),
        "last_start": state.last_start, "last_task": state.last_task, "last_outcome": state.last_outcome.map(Outcome::key), "last_summary": state.last_summary,
    })
}

pub fn render(rows: &[(Routine, State, bool)], errors: &[String], now: u64, offset: i64) -> String {
    let mut lines = Vec::new();
    if rows.is_empty() {
        lines.push("No routines yet: add a [[routines]] table to config.toml or a repo config; kelpie config shows the keys.".to_string());
    }
    for (routine, state, open) in rows {
        let push = if routine.push { ", pushes its fix" } else { "" };
        let next = match (routine.enabled, *open) {
            (false, _) => "off (enabled = false)".to_string(),
            (true, true) => format!("after {} closes", state.last_task.as_deref().unwrap_or("its task")),
            (true, false) => match next_run(routine.schedule, state, now, offset) {
                at if at <= now => "now, at the next round".to_string(),
                at => format!("in {}", crate::keeper::span(at - now)),
            },
        };
        lines.push(format!("{}  {}", routine.id, routine.title));
        lines.push(format!("  {} in {}, {}{push}; next run {next}", routine.schedule.text(), routine.repo, routine.mode.key()));
        let last = match (state.last_outcome, state.last_start) {
            (Some(outcome), Some(at)) => format!("  last: {}, {}{}", outcome.key(), ago(now.saturating_sub(at)), state.last_summary.as_deref().map(|summary| format!(": {summary}")).unwrap_or_default()),
            _ => "  last: not run yet".to_string(),
        };
        lines.push(last);
    }
    lines.extend(errors.iter().map(|error| format!("ignored: {error}")));
    lines.join("\n")
}

pub fn list(json: bool) -> Result<String> {
    let dir = crate::lead::state_dir()?;
    let (routines, errors) = load();
    let now = crate::messages::now();
    let offset = local_offset(now);
    let rows: Vec<(Routine, State, bool)> = routines.into_iter().map(|routine| {
        let state = load_state(&dir, &routine.id);
        let open = state.last_task.as_deref().is_some_and(|address| open(&routine, address));
        (routine, state, open)
    }).collect();
    if json {
        let shown: Vec<Value> = rows.iter().map(|(routine, state, open)| row_json(routine, state, *open, now, offset)).collect();
        return serde_json::to_string_pretty(&json!({ "routines": shown, "errors": errors })).map_err(|error| error.to_string());
    }
    Ok(render(&rows, &errors, now, offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str, default_repo: Option<&str>) -> Result<Vec<Routine>> {
        parse_list(&text.parse::<Table>().unwrap(), default_repo, "config.toml")
    }

    fn one(text: &str) -> Routine {
        parsed(text, Some("infra")).unwrap().remove(0)
    }

    const CLUSTER: &str = "[[routines]]\nid = \"cluster-health\"\ntitle = \"Cluster health\"\nevery = \"6h\"\nprompt = \"Check that every node is Ready.\"\n";

    #[test]
    fn a_routine_takes_its_repo_configs_repo_and_the_defaults() {
        let routine = one(CLUSTER);
        assert_eq!(routine, Routine {
            id: "cluster-health".to_string(), title: "Cluster health".to_string(), repo: "infra".to_string(), schedule: Schedule::Every(6 * 3600),
            prompt: "Check that every node is Ready.".to_string(), mode: Mode::Report, push: false, enabled: true, source: "config.toml".to_string(),
        });
        let full = one("[[routines]]\nid = \"deps\"\ntitle = \"Updates\"\nrepo = \"~/code/app\"\nat = \"mon 07:00\"\nprompt = \"x\"\nmode = \"fix\"\npush = true\nenabled = false\n");
        assert_eq!((full.repo.as_str(), full.schedule, full.mode, full.push, full.enabled), ("~/code/app", Schedule::Weekly(0, 420), Mode::Fix, true, false));
        assert!(parsed("", None).unwrap().is_empty());
    }

    #[test]
    fn a_bad_routine_is_refused_with_the_key_and_the_value_named() {
        let refused = |text: &str, default_repo: Option<&str>| parsed(text, default_repo).unwrap_err();
        let base = |extra: &str| format!("[[routines]]\nid = \"x\"\ntitle = \"X\"\nprompt = \"p\"\n{extra}");
        assert!(refused(&base("every = \"1h\"\ncolour = \"red\"\n"), Some("app")).contains("routines[0].colour: unknown key; known keys here: id, title, repo, every, at, prompt, mode, push, enabled"));
        assert!(refused(&base("every = \"1h\"\n"), None).contains("routines[0].repo: a routine in config.toml names the repo"));
        assert!(refused(&base("every = \"soon\"\n"), Some("app")).contains("routines[0].every: \"soon\" is not an interval"));
        assert!(refused(&base("every = \"0m\"\n"), Some("app")).contains("\"0m\" is not an interval"));
        assert!(refused(&base("at = \"25:00\"\n"), Some("app")).contains("routines[0].at: \"25:00\" is not a time"));
        assert!(refused(&base("at = \"someday 07:00\"\n"), Some("app")).contains("\"someday\" is not a weekday"));
        assert!(refused(&base("every = \"1h\"\nat = \"07:00\"\n"), Some("app")).contains("every and at are two schedules"));
        assert!(refused(&base(""), Some("app")).contains("needs a schedule"));
        assert!(refused(&base("every = \"1h\"\nmode = \"panic\"\n"), Some("app")).contains("routines[0].mode: \"panic\" is not a mode; use \"report\", \"plan\" or \"fix\""));
        assert!(refused(&base("every = \"1h\"\npush = true\n"), Some("app")).contains("routines[0].push: only a fix routine pushes"));
        assert!(refused(&base("every = \"1h\"\npush = \"yes\"\n"), Some("app")).contains("routines[0].push: must be true or false"));
        assert!(refused("[[routines]]\nid = \"Bad Id\"\ntitle = \"X\"\nprompt = \"p\"\nevery = \"1h\"\n", Some("app")).contains("\"Bad Id\" is not an id"));
        assert!(refused("[[routines]]\ntitle = \"X\"\nprompt = \"p\"\nevery = \"1h\"\n", Some("app")).contains("every routine needs an id"));
        assert!(refused("[[routines]]\nid = \"x\"\nprompt = \"p\"\nevery = \"1h\"\n", Some("app")).contains("needs a title"));
        assert!(refused("[[routines]]\nid = \"x\"\ntitle = \"X\"\nevery = \"1h\"\n", Some("app")).contains("needs a prompt"));
        assert!(refused(&format!("{}{}", base("every = \"1h\"\n"), base("every = \"2h\"\n")), Some("app")).contains("routines[1].id: \"x\" is taken"));
        assert!(refused("routines = 3\n", Some("app")).contains("must be [[routines]] tables"));
        for good in ["a", "cluster-health", "deps2"] {
            assert!(valid_id(good), "{good}");
        }
        for bad in ["", "-a", "a-", "a--b", "A", "1a", "a/b", "a.b", &"a".repeat(41)] {
            assert!(!valid_id(bad), "{bad}");
        }
        assert_eq!(parse_every("30m"), Ok(1800));
        assert_eq!(parse_every("1d"), Ok(DAY));
        assert_eq!(parse_at("7:05"), Ok(Schedule::Daily(425)));
        assert_eq!(parse_at("Sun 23:59"), Ok(Schedule::Weekly(6, 1439)));
        assert!(parse_at("07:0").is_err() && parse_at("mon").is_err() && parse_at("mon tue 07:00").is_err());
    }

    #[test]
    fn the_config_files_refuse_a_bad_routine_and_the_loader_keeps_the_good_ones() {
        let dir = crate::sh::test_dir("routines-load");
        let repos = dir.join("repos");
        fs::create_dir_all(&repos).unwrap();
        fs::write(dir.join("config.toml"), format!("{CLUSTER}repo = \"infra\"\n")).unwrap();
        fs::write(repos.join("app.toml"), "[[routines]]\nid = \"deps\"\ntitle = \"Updates\"\nat = \"mon 07:00\"\nprompt = \"p\"\nmode = \"plan\"\n").unwrap();
        fs::write(repos.join("web.toml"), "[[routines]]\nid = \"deps\"\ntitle = \"Again\"\nevery = \"1d\"\nprompt = \"p\"\n").unwrap();
        fs::write(repos.join("broken.toml"), "[[routines]]\nid = \"x\"\n").unwrap();
        let (routines, errors) = load_from(&dir.join("config.toml"), &repos);
        assert_eq!(routines.iter().map(|routine| (routine.id.as_str(), routine.repo.as_str())).collect::<Vec<_>>(), [("cluster-health", "infra"), ("deps", "app")]);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("broken.toml") && errors[0].contains("needs a title"), "{errors:?}");
        assert!(errors[1].contains("web.toml: routine \"deps\" is taken by") && errors[1].contains("app.toml"), "{errors:?}");
        assert!(crate::config::parse_machine("[[routines]]\nid = \"x\"\ntitle = \"X\"\nprompt = \"p\"\nevery = \"1h\"\n", "config.toml").unwrap_err().contains("config.toml: routines[0].repo"));
        assert!(crate::config::parse_repo("[[routines]]\nid = \"x\"\ntitle = \"X\"\nprompt = \"p\"\nevery = \"1h\"\n", "/c/repos/app.toml").is_ok(), "a repo config's routine runs in its own repo");
        assert!(crate::config::parse_repo("[[routines]]\nid = \"x\"\ntitle = \"X\"\nprompt = \"p\"\nevery = \"1h\"\nmode = \"auto\"\n", "app.toml").unwrap_err().contains("app.toml: routines[0].mode"));
        fs::remove_dir_all(dir).unwrap();
    }

    const MONDAY_0630_UTC: u64 = 1_792_391_400;

    #[test]
    fn an_every_schedule_runs_once_the_interval_has_passed_since_the_last_start() {
        let every = Schedule::Every(6 * 3600);
        let never = State::default();
        assert!(due(every, &never, 1000, 0), "a new routine runs at the first round");
        let ran = State { last_start: Some(1000), ..State::default() };
        assert!(!due(every, &ran, 1000 + 6 * 3600 - 1, 0));
        assert!(due(every, &ran, 1000 + 6 * 3600, 0));
        assert!(due(every, &ran, 1000 + 30 * 86_400, 0), "a month asleep is one run on wake");
        assert_eq!(next_run(every, &ran, 2000, 0), 1000 + 6 * 3600);
        assert_eq!(Schedule::Every(1800).text(), "every 30m");
        assert_eq!(Schedule::Every(DAY).text(), "every 1d");
    }

    #[test]
    fn an_at_schedule_runs_once_its_time_has_come_since_the_last_start_in_local_time() {
        assert_eq!(crate::keeper::stamp(MONDAY_0630_UTC), "2026-10-19 06:30:00Z");
        let daily = Schedule::Daily(7 * 60);
        assert_eq!(latest_slot(daily, MONDAY_0630_UTC, 0), Some(MONDAY_0630_UTC - 23 * 3600 - 1800), "07:00 has not come today, so yesterday's");
        assert_eq!(latest_slot(daily, MONDAY_0630_UTC, 3600), Some(MONDAY_0630_UTC - 1800), "at UTC+1 it is 07:30 already");
        let fresh = State { seen: Some(MONDAY_0630_UTC), ..State::default() };
        assert!(!due(daily, &State::default(), MONDAY_0630_UTC, 0), "an at routine not seen before waits for its time");
        assert!(!due(daily, &fresh, MONDAY_0630_UTC + 1799, 0));
        assert!(due(daily, &fresh, MONDAY_0630_UTC + 1800, 0), "07:00 came");
        let ran = State { last_start: Some(MONDAY_0630_UTC + 1800), ..fresh.clone() };
        assert!(!due(daily, &ran, MONDAY_0630_UTC + 1860, 0), "and it ran");
        assert_eq!(next_run(daily, &ran, MONDAY_0630_UTC + 1860, 0), MONDAY_0630_UTC + 1800 + DAY);
        assert!(due(daily, &ran, MONDAY_0630_UTC + 1800 + 3 * DAY + 5, 0), "three missed days asleep");
        let caught_up = State { last_start: Some(MONDAY_0630_UTC + 1800 + 3 * DAY + 5), ..ran };
        assert!(!due(daily, &caught_up, MONDAY_0630_UTC + 1800 + 3 * DAY + 65, 0), "run once on wake, not once per missed day");
        let monday = Schedule::Weekly(0, 7 * 60);
        assert_eq!(latest_slot(monday, MONDAY_0630_UTC, 0), Some(MONDAY_0630_UTC + 1800 - 7 * DAY), "last Monday's");
        assert_eq!(latest_slot(monday, MONDAY_0630_UTC + 1800, 0), Some(MONDAY_0630_UTC + 1800));
        assert_eq!(latest_slot(Schedule::Weekly(6, 7 * 60), MONDAY_0630_UTC, 0), Some(MONDAY_0630_UTC + 1800 - DAY), "yesterday was Sunday");
        assert!(!due(monday, &fresh, MONDAY_0630_UTC + 1799, 0) && due(monday, &fresh, MONDAY_0630_UTC + 1800, 0));
        assert_eq!(next_run(monday, &fresh, MONDAY_0630_UTC, 0), MONDAY_0630_UTC + 1800);
        assert_eq!(monday.text(), "mon at 07:00");
        assert_eq!(daily.text(), "daily at 07:00");
    }

    #[test]
    fn a_routine_never_starts_twice_while_its_last_task_is_open() {
        let mut routine = one(CLUSTER);
        let old = State { last_start: Some(0), ..State::default() };
        assert_eq!(decide(&routine, &old, false, DAY, 0), Decision::Start);
        assert_eq!(decide(&routine, &old, true, DAY, 0), Decision::Open, "due, but its last run is still open");
        assert_eq!(decide(&routine, &State { last_start: Some(DAY - 60), ..State::default() }, false, DAY, 0), Decision::Wait);
        routine.enabled = false;
        assert_eq!(decide(&routine, &old, false, DAY, 0), Decision::Wait);
    }

    #[test]
    fn a_runs_task_is_named_by_its_id_and_local_start_within_herdrs_32_characters() {
        assert_eq!(task_name("cluster-health", MONDAY_0630_UTC, 0), "cluster-health-20261019-0630");
        assert_eq!(task_name("cluster-health", MONDAY_0630_UTC, 7200), "cluster-health-20261019-0830");
        let long = task_name("dependency-updates-for-the-app", MONDAY_0630_UTC, 0);
        assert_eq!(long, "dependency-updates-20261019-0630");
        assert!(long.len() <= 32);
        assert_eq!(task_name("aaaaaaaaaaaaaaaaa-b", MONDAY_0630_UTC, 0), "aaaaaaaaaaaaaaaaa-20261019-0630", "a cut never leaves two dashes");
        assert_eq!(crate::tasks::slug(&long).unwrap(), long);
    }

    #[test]
    fn the_state_survives_a_round_trip_and_finds_its_routine_by_task() {
        let dir = crate::sh::test_dir("routines-state");
        assert_eq!(load_state(&dir, "x"), State::default());
        let past = Past { start: 1, outcome: Outcome::Failed, summary: Some("kubectl: connection refused".to_string()) };
        let state = State { seen: Some(1), last_start: Some(2), last_task: Some("x-20261019-0630@infra".to_string()), last_outcome: Some(Outcome::Running), last_summary: None, earlier: vec![past] };
        save_state(&dir, "x", &state).unwrap();
        assert_eq!(load_state(&dir, "x"), state);
        assert!(dir.join("routines/x.json").is_file());
        assert_eq!(run_of(&dir, "x-20261019-0630@infra").map(|(id, _)| id).as_deref(), Some("x"));
        assert!(run_of(&dir, "billing@infra").is_none());
        assert!(found_in(&dir, "x-20261019-0630@infra", "node-3 NotReady"), "the first word of a run that is not ok is its findings");
        assert!(!found_in(&dir, "x-20261019-0630@infra", "plan approved, fixing"), "and is heard once");
        assert_eq!(load_state(&dir, "x").last_outcome, Some(Outcome::Findings));
        assert_eq!(load_state(&dir, "x").last_summary.as_deref(), Some("plan approved, fixing"));
        assert!(!found_in(&dir, "billing@infra", "x"));
        for outcome in Outcome::ALL {
            assert_eq!(state_from(&state_json(&State { last_outcome: Some(outcome), ..State::default() })).last_outcome, Some(outcome));
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_new_run_keeps_the_last_few_runs_before_it_newest_first() {
        let mut state = State::default();
        keep_last(&mut state);
        assert!(state.earlier.is_empty(), "a routine that never ran has nothing to keep");
        for start in 1..=6 {
            state.last_start = Some(start);
            state.last_outcome = Some(if start % 2 == 0 { Outcome::Ok } else { Outcome::Findings });
            state.last_summary = Some(format!("run {start}"));
            keep_last(&mut state);
        }
        assert_eq!(state.earlier.iter().map(|past| past.start).collect::<Vec<_>>(), [6, 5, 4, 3]);
        assert_eq!((state.earlier[0].outcome, state.earlier[0].summary.as_deref()), (Outcome::Ok, Some("run 6")));
        assert_eq!(state_from(&json!({ "earlier": [{ "start": 1, "outcome": "nonsense" }, { "start": 2, "outcome": "ok" }] })).earlier.len(), 1, "an entry kelpie cannot read is left out");
    }

    #[test]
    fn the_prompt_carries_the_routine_the_modes_rules_and_how_to_finish() {
        let mut routine = one(CLUSTER);
        let report = prompt(&routine, "/bin/kelpie");
        assert!(report.contains("routine \"Cluster health\" (cluster-health), which kelpie runs every 6h in infra, in report mode"), "{report}");
        assert!(report.contains("What to check:\nCheck that every node is Ready."));
        assert!(report.contains("change nothing") && report.contains("/bin/kelpie routine ok \"") && report.contains("/bin/kelpie report --state needs-you") && report.contains("--card <file>"), "{report}");
        routine.mode = Mode::Plan;
        let plan = prompt(&routine, "/bin/kelpie");
        assert!(plan.contains("/bin/kelpie ask --plan") && plan.contains("only once they approve it") && plan.contains("routine ok"), "{plan}");
        routine.mode = Mode::Fix;
        let fix = prompt(&routine, "/bin/kelpie");
        assert!(fix.contains("fix it in commits") && fix.contains("--state ready") && fix.contains("The user pushes your fix"), "{fix}");
        routine.push = true;
        assert!(prompt(&routine, "/bin/kelpie").contains("the lead pushes them without waiting for the user"));
    }

    #[test]
    fn only_a_fix_routine_with_push_grants_the_push_permission() {
        let mut routine = one(CLUSTER);
        for (mode, push, granted) in [(Mode::Report, false, false), (Mode::Plan, false, false), (Mode::Fix, false, false), (Mode::Fix, true, true), (Mode::Report, true, false)] {
            routine.mode = mode;
            routine.push = push;
            assert_eq!(grants_push(&routine), granted, "{mode:?} push = {push}");
        }
    }

    #[test]
    fn routine_ok_is_the_word_of_the_routines_own_task_alone() {
        assert_eq!(ok_allowed(&Caller::Task("x-1@infra".to_string()), "x-1@infra"), Ok(()));
        assert!(ok_allowed(&Caller::Task("billing@infra".to_string()), "x-1@infra").unwrap_err().contains("the task billing@infra cannot"));
        assert!(ok_allowed(&Caller::User, "x-1@infra").unwrap_err().contains("the user cannot"));
        assert!(ok_allowed(&Caller::Lead, "x-1@infra").unwrap_err().contains("the lead cannot"));
        assert!(ok_allowed(&Caller::Agent, "x-1@infra").is_err());
    }

    #[test]
    fn the_list_shows_each_routines_schedule_next_run_and_last_outcome() {
        let mut off = one("[[routines]]\nid = \"deps\"\ntitle = \"Updates\"\nat = \"mon 07:00\"\nprompt = \"p\"\nmode = \"plan\"\n");
        off.enabled = false;
        let ran = State { last_start: Some(1000), last_task: Some("cluster-health-x@infra".to_string()), last_outcome: Some(Outcome::Ok), last_summary: Some("all nodes Ready".to_string()), seen: Some(1), earlier: Vec::new() };
        let rows = [(one(CLUSTER), ran.clone(), false), (one(CLUSTER), State { last_outcome: Some(Outcome::Running), ..ran }, true), (off, State::default(), false)];
        assert_eq!(render(&rows, &["repos/web.toml: bad".to_string()], 1000 + 3600, 0), [
            "cluster-health  Cluster health",
            "  every 6h in infra, report; next run in 5h",
            "  last: ok, 1h ago: all nodes Ready",
            "cluster-health  Cluster health",
            "  every 6h in infra, report; next run after cluster-health-x@infra closes",
            "  last: running, 1h ago: all nodes Ready",
            "deps  Updates",
            "  mon at 07:00 in infra, plan; next run off (enabled = false)",
            "  last: not run yet",
            "ignored: repos/web.toml: bad",
        ].join("\n"));
        assert!(render(&[], &[], 0, 0).starts_with("No routines yet"));
        let shown = row_json(&rows[0].0, &rows[0].1, false, 1000 + 3600, 0);
        assert_eq!((shown["next_run"].as_u64(), shown["last_outcome"].as_str(), shown["schedule"].as_str()), (Some(1000 + 6 * 3600), Some("ok"), Some("every 6h")));
    }
}
