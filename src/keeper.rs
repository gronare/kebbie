use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::sh::Result;

pub const LOCK: &str = "keeper.lock";
pub const STATUS: &str = "keeper.json";
pub const LOG: &str = "keeper.log";
const LOG_BYTES: u64 = 256 * 1024;
const MAX_BACKOFF: u64 = 15 * 60;
const SLICE: Duration = Duration::from_millis(500);
pub const OFF: &str = "keeper: off ([keeper] enabled = false); an open board runs the routines";

#[derive(Default)]
pub struct Ctx {
    memory: crate::restart::Memory,
    stale: Vec<String>,
    stale_at: u64,
    notes: Vec<String>,
}

pub struct Routine {
    pub name: &'static str,
    pub every: u64,
    pub run: fn(&mut Ctx) -> Result<()>,
}

// Every routine the keeper runs, each with its interval in seconds; a new routine is one more entry here.
pub fn routines() -> Vec<Routine> {
    vec![
        Routine { name: "messages", every: 3, run: |_| crate::messages::flush().map(|_| ()) },
        Routine { name: "typed", every: 3, run: |_| { crate::typing::check(); Ok(()) } },
        Routine { name: "restart", every: 60, run: restart },
        Routine { name: "updates", every: 600, run: updates },
        Routine { name: "herdr", every: crate::herdr_link::EVERY, run: |ctx| { ctx.notes.extend(crate::herdr_link::heal()?); Ok(()) } },
        Routine { name: "push", every: 15, run: |ctx| { ctx.notes.extend(crate::tasks::complete_pushes(&crate::lead::state_dir()?)); Ok(()) } },
        Routine { name: "stage", every: 15, run: |ctx| { ctx.notes.extend(crate::tasks::stage_tick(&crate::lead::state_dir()?)); Ok(()) } },
        Routine { name: "ci", every: 30, run: |_| { crate::ci::tick(&crate::lead::state_dir()?); Ok(()) } },
        Routine { name: "after", every: crate::tasks::AFTER_EVERY, run: |ctx| { ctx.notes.extend(crate::tasks::after_tick(&crate::lead::state_dir()?)); Ok(()) } },
        Routine { name: "landed", every: crate::tasks::LANDED_EVERY, run: |ctx| { ctx.notes.extend(crate::tasks::landed_tick(&crate::lead::state_dir()?)); Ok(()) } },
        Routine { name: "follow", every: 60, run: |ctx| { ctx.notes.extend(crate::tasks::follow_tick(&crate::lead::state_dir()?)); Ok(()) } },
        Routine { name: "prompts", every: crate::menu::EVERY, run: |ctx| { ctx.notes.extend(crate::menu::tick(&crate::lead::state_dir()?)?); Ok(()) } },
        Routine { name: "recover", every: 30, run: |_| { crate::recover::tick(&crate::lead::state_dir()?); Ok(()) } },
        Routine { name: "issues", every: crate::issue_recheck::EVERY, run: |ctx| { ctx.notes.extend(crate::issue_recheck::tick(&crate::lead::state_dir()?)); Ok(()) } },
        Routine { name: "rules", every: crate::rules_check::EVERY, run: |ctx| { ctx.notes.extend(crate::rules_check::tick(&crate::lead::state_dir()?)); Ok(()) } },
        Routine { name: "routines", every: crate::routines::EVERY, run: |ctx| { ctx.notes.extend(crate::routines::tick(&crate::lead::state_dir()?)); Ok(()) } },
        Routine { name: "hub", every: crate::hub::EVERY, run: |_| crate::hub::tick() },
        Routine { name: "reports", every: crate::hub::inbox::EVERY, run: |ctx| { ctx.notes.extend(crate::hub::inbox::tick(&crate::lead::state_dir()?)?); Ok(()) } },
        Routine { name: "signals", every: crate::signals::EVERY, run: |ctx| { ctx.notes.extend(crate::signals::tick(&crate::lead::state_dir()?)?); Ok(()) } },
        Routine { name: "install", every: crate::signals::EVERY, run: |_| { crate::install_signal::tick(&crate::lead::state_dir()?); Ok(()) } },
        Routine { name: "outbox", every: crate::hub::report::UNASKED_EVERY, run: |ctx| { ctx.notes.extend(crate::hub::report::send_unasked_tick(&crate::lead::state_dir()?)?); Ok(()) } },
    ]
}

fn restart(ctx: &mut Ctx) -> Result<()> {
    let lines = crate::restart::run(crate::restart::Mode::Routine { min_idle: 0 }, &mut ctx.memory, None)?;
    ctx.notes.extend(lines.iter().filter(|line| line.said == "restarted" || line.said.starts_with("restart failed")).map(|line| format!("restart: {}: {}", line.label, line.said)));
    ctx.stale = stale_lines(&lines);
    ctx.stale_at = crate::messages::now();
    ctx.notes.extend(crate::known::announce()?.into_iter().map(|said| format!("update: {said}")));
    Ok(())
}

fn updates(_: &mut Ctx) -> Result<()> {
    let places = crate::update_check::Places::current();
    let state = crate::lead::state_dir()?;
    crate::update_check::current(&state, crate::messages::now(), places.channel, crate::update_check::installed(&places), || crate::update_check::check(&places));
    Ok(())
}

// The board words the lead's line by its reasons, which ride after this mark, joined by "; ".
pub const REASONS: &str = " · as ";

pub fn stale_lines(lines: &[crate::restart::Line]) -> Vec<String> {
    lines.iter().filter(|line| line.stale).map(|line| match line.reasons.is_empty() {
        true => format!("{}: {}", line.label, line.said),
        false => format!("{}: {}{REASONS}{}", line.label, line.said, line.reasons.join("; ")),
    }).collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub last: Option<u64>,
    pub failures: u32,
    pub error: Option<String>,
}

pub fn delay(every: u64, failures: u32) -> u64 {
    if failures == 0 {
        return every;
    }
    every.saturating_mul(1 << failures.min(10)).min(every.max(MAX_BACKOFF))
}

pub fn next_at(every: u64, track: &Track) -> u64 {
    track.last.map_or(0, |last| last + delay(every, track.failures))
}

pub fn due(routines: &[Routine], tracks: &[Track], now: u64) -> Vec<usize> {
    routines.iter().zip(tracks).enumerate().filter(|(_, (routine, track))| next_at(routine.every, track) <= now).map(|(index, _)| index).collect()
}

pub fn wait(routines: &[Routine], tracks: &[Track], now: u64) -> u64 {
    routines.iter().zip(tracks).map(|(routine, track)| next_at(routine.every, track).saturating_sub(now)).min().unwrap_or(60).clamp(1, 60)
}

pub fn record(track: &mut Track, now: u64, outcome: std::result::Result<(), String>) -> Option<String> {
    track.last = Some(now);
    match outcome {
        Ok(()) => {
            let recovered = track.failures > 0;
            track.failures = 0;
            track.error = None;
            recovered.then(|| "recovered".to_string())
        }
        Err(error) => {
            track.failures = track.failures.saturating_add(1);
            let changed = track.error.as_deref() != Some(error.as_str());
            track.error = Some(error.clone());
            changed.then_some(error)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Owner {
    Board,
    Keeper,
}

pub fn owner(enabled: bool) -> Owner {
    if enabled { Owner::Keeper } else { Owner::Board }
}

// A config.toml that does not parse leaves the keeper on, so a half-saved edit never hands the routines to the board and back.
pub fn enabled() -> bool {
    crate::config::keeper_enabled().unwrap_or(true)
}

pub fn current_owner() -> Owner {
    owner(enabled())
}

pub fn should_start(enabled: bool, running: bool) -> bool {
    enabled && !running
}

fn try_lock(file: &File, operation: libc::c_int) -> bool {
    // SAFETY: flock only reads the descriptor, which the File keeps open.
    unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) == 0 }
}

// The kernel drops a flock when its holder exits, however it exits, so a lock file left by a dead keeper is free to take.
pub struct Held {
    _file: File,
    path: PathBuf,
    id: (u64, u64),
}

impl Drop for Held {
    // A child forked by another thread holds a copy of the descriptor until it execs; unlocking releases the lock for every copy at once.
    fn drop(&mut self) {
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
    }
}

impl Held {
    pub fn still_ours(&self) -> bool {
        fs::metadata(&self.path).is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.id)
    }
}

pub fn acquire(dir: &Path, exe: &Path, tries: u32) -> Result<Option<Held>> {
    crate::state::private_dir(dir)?;
    let path = dir.join(LOCK);
    for attempt in 0..tries.max(1) {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        if !try_lock(&file, libc::LOCK_EX) {
            continue;
        }
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        let id = (metadata.dev(), metadata.ino());
        if !fs::metadata(&path).is_ok_and(|now| (now.dev(), now.ino()) == id) {
            continue;
        }
        file.set_permissions(fs::Permissions::from_mode(0o600)).map_err(|error| format!("{}: {error}", path.display()))?;
        let record = json!({ "pid": std::process::id(), "exe": exe.to_string_lossy(), "started": crate::messages::now() });
        file.set_len(0).and_then(|_| file.write_all(record.to_string().as_bytes())).map_err(|error| format!("{}: {error}", path.display()))?;
        return Ok(Some(Held { _file: file, path, id }));
    }
    Ok(None)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Holder {
    pub pid: Option<u32>,
    pub exe: Option<String>,
    pub started: Option<u64>,
}

pub fn holder(dir: &Path) -> Option<Holder> {
    let mut file = File::open(dir.join(LOCK)).ok()?;
    if try_lock(&file, libc::LOCK_SH) {
        // SAFETY: releases the probe's own shared lock on its own descriptor.
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        return None;
    }
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    let record: Value = serde_json::from_str(&text).unwrap_or_default();
    Some(Holder {
        pid: record["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()),
        exe: record["exe"].as_str().map(str::to_string),
        started: record["started"].as_u64(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Identity {
    dev: u64,
    ino: u64,
    len: u64,
    modified: (i64, i64),
}

pub fn identity(path: &Path) -> Option<Identity> {
    let metadata = fs::metadata(path).ok().filter(|metadata| metadata.len() > 0)?;
    Some(Identity { dev: metadata.dev(), ino: metadata.ino(), len: metadata.len(), modified: (metadata.mtime(), metadata.mtime_nsec()) })
}

// A rebuilt or relinked binary must look the same on two checks in a row, so a half-written one is never started.
pub struct Binary {
    started: Option<Identity>,
    last: Option<Identity>,
}

impl Binary {
    pub fn new(now: Option<Identity>) -> Self {
        Self { started: now, last: now }
    }

    pub fn replaced(&mut self, now: Option<Identity>) -> bool {
        let replaced = now.is_some() && now != self.started && now == self.last;
        self.last = now;
        replaced
    }
}

struct Log(PathBuf);

impl Log {
    fn line(&self, text: &str) {
        if fs::metadata(&self.0).is_ok_and(|metadata| metadata.len() > LOG_BYTES) {
            let _ = fs::rename(&self.0, self.0.with_extension("log.1"));
        }
        if let Ok(mut file) = crate::state::append_to(&self.0) {
            let _ = writeln!(file, "{} {text}", stamp(crate::messages::now()));
        }
    }
}

pub fn log_line(state: &Path, text: &str) {
    Log(state.join(LOG)).line(text);
}

pub fn stamp(at: u64) -> String {
    let days = (at / 86_400) as i64;
    let seconds = at % 86_400;
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

// A stop waits for the routine under way, as one cut off between ending a Claude and resuming it would leave the pane without one.
fn catch_stop() {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGTERM, on_stop as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_stop as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
}

fn status_json(holder: &Holder, routines: &[Routine], tracks: &[Track], stale: &[String], stale_at: u64) -> Value {
    let rows: Vec<Value> = routines.iter().zip(tracks).map(|(routine, track)| json!({ "name": routine.name, "every": routine.every, "last": track.last, "failures": track.failures, "error": track.error })).collect();
    json!({ "pid": holder.pid, "exe": holder.exe, "started": holder.started, "routines": rows, "stale": stale, "stale_at": stale_at })
}

fn read_status(dir: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(dir.join(STATUS)).ok()?).ok()
}

// The stale sessions the running keeper last saw and when, for the board's pill; a dead keeper's leftovers count as none, seen at 0.
pub fn stale_in(dir: &Path) -> (u64, Vec<String>) {
    let Some(holder) = holder(dir) else { return (0, Vec::new()) };
    read_status(dir).filter(|status| holder.pid.is_some() && status["pid"].as_u64() == holder.pid.map(u64::from))
        .map(|status| (status["stale_at"].as_u64().unwrap_or(0), status["stale"].as_array().into_iter().flatten().filter_map(|line| line.as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}

pub fn stale() -> (u64, Vec<String>) {
    crate::lead::state_dir().map(|dir| stale_in(&dir)).unwrap_or_default()
}

pub fn span(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 if seconds % 3600 / 60 == 0 => format!("{}h", seconds / 3600),
        3600..86_400 => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d {}h", seconds / 86_400, seconds % 86_400 / 3600),
    }
}

pub fn render_status(enabled: bool, holder: Option<&Holder>, status: Option<&Value>, log: &Path, now: u64) -> String {
    let Some(holder) = holder else {
        let head = if enabled { "keeper: not running; kebbie keeper --start starts it, and so do herdr's startup and the board".to_string() } else { OFF.to_string() };
        return format!("{head}\nlog: {}", log.display());
    };
    let pid = holder.pid.map_or("?".to_string(), |pid| pid.to_string());
    let up = holder.started.map_or(String::new(), |started| format!(", up {}", span(now.saturating_sub(started))));
    let mut lines = vec![format!("keeper: running, pid {pid}{up}")];
    if !enabled {
        lines.push("  [keeper] enabled = false: it stops at its next round, and the board takes over".to_string());
    }
    lines.push(format!("binary: {}", holder.exe.as_deref().unwrap_or("?")));
    lines.push(format!("log: {}", log.display()));
    let status = status.filter(|status| holder.pid.is_some() && status["pid"].as_u64() == holder.pid.map(u64::from));
    let Some(rows) = status.and_then(|status| status["routines"].as_array()) else {
        lines.push("routines: none has run yet".to_string());
        return lines.join("\n");
    };
    lines.push("routines:".to_string());
    let width = rows.iter().filter_map(|row| row["name"].as_str()).map(str::len).max().unwrap_or(0);
    for row in rows {
        let name = row["name"].as_str().unwrap_or("?");
        let every = format!("every {}", span(row["every"].as_u64().unwrap_or(0)));
        let last = row["last"].as_u64().map_or("not run yet".to_string(), |last| format!("last {} ago", span(now.saturating_sub(last))));
        let failed = match (row["failures"].as_u64().unwrap_or(0), row["error"].as_str()) {
            (0, _) | (_, None) => String::new(),
            (1, Some(error)) => format!(", failed: {error}"),
            (count, Some(error)) => format!(", failed {count} times in a row: {error}"),
        };
        lines.push(format!("  {name:<width$}  {every:<9}  {last}{failed}"));
    }
    lines.join("\n")
}

pub fn status_cli() -> Result<String> {
    let dir = crate::lead::state_dir()?;
    Ok(render_status(enabled(), holder(&dir).as_ref(), read_status(&dir).as_ref(), &dir.join(LOG), crate::messages::now()))
}

// Its own session, so a closing pane's hangup never reaches it, and its output in the log, so herdr's hook and kebbie update never wait on a pipe it holds.
fn spawn(exe: &Path, dir: &Path) -> Result<std::process::Child> {
    let log = crate::state::append_to(&dir.join(LOG)).map_err(|error| format!("{}: {error}", dir.join(LOG).display()))?;
    let mut command = Command::new(exe);
    command.arg("keeper").stdin(Stdio::null()).stdout(Stdio::null()).stderr(log)
        .env_remove("HERDR_PANE_ID").env_remove("HERDR_TAB_ID").env_remove("HERDR_WORKSPACE_ID");
    // SAFETY: setsid is async-signal-safe and touches nothing but the child.
    unsafe { command.pre_exec(|| if libc::setsid() == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) }) };
    command.spawn().map_err(|error| format!("keeper: {}: {error}", exe.display()))
}

fn wait_for(mut done: impl FnMut() -> bool, timeout: Duration) -> bool {
    let started = Instant::now();
    while !done() {
        if started.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    true
}

pub fn start_in(dir: &Path, exe: &Path, enabled: bool) -> Result<String> {
    if !enabled {
        return Ok(OFF.to_string());
    }
    if let Some(running) = holder(dir) {
        return Ok(format!("keeper: already running (pid {})", running.pid.map_or("?".to_string(), |pid| pid.to_string())));
    }
    let mut child = spawn(exe, dir)?;
    let pid = child.id();
    if wait_for(|| holder(dir).is_some_and(|running| running.pid == Some(pid)) || child.try_wait().ok().flatten().is_some(), Duration::from_secs(5))
        && let Some(running) = holder(dir)
    {
        return Ok(format!("keeper: started (pid {})", running.pid.map_or("?".to_string(), |pid| pid.to_string())));
    }
    Err(format!("keeper: started pid {pid}, which holds no lock; see {}", dir.join(LOG).display()))
}

pub fn start() -> Result<String> {
    let dir = crate::lead::state_dir()?;
    start_in(&dir, Path::new(&crate::peers::kebbie_command()?), enabled())
}

pub fn stop_in(dir: &Path, timeout: Duration) -> Result<String> {
    let Some(running) = holder(dir) else { return Ok("keeper: not running".to_string()) };
    let pid = running.pid.ok_or_else(|| crate::signals::internal("keeper.lock", kebbie_hub_protocol::SignalKind::CorruptState, "keeper: one runs, but its lock names no pid"))?;
    // SAFETY: a signal to the pid that holds the keeper's lock, read from that lock.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if wait_for(|| holder(dir).is_none(), timeout) {
        Ok(format!("keeper: stopped (pid {pid})"))
    } else {
        Err(format!("keeper: pid {pid} is finishing a routine and stops once it is done"))
    }
}

pub fn stop() -> Result<String> {
    stop_in(&crate::lead::state_dir()?, Duration::from_secs(90))
}

pub fn stop_for_update(dir: &Path) -> bool {
    holder(dir).is_some() && stop_in(dir, Duration::from_secs(90)).is_ok()
}

pub fn start_after_update(dir: &Path, was_running: bool) -> String {
    let exe = match crate::peers::kebbie_command() {
        Ok(exe) => PathBuf::from(exe),
        Err(error) => return format!("keeper: {error}"),
    };
    match start_in(dir, &exe, enabled()) {
        Ok(line) if was_running && line.starts_with("keeper: started") => line.replacen("keeper: started", "keeper: restarted on the new kebbie", 1),
        Ok(line) | Err(line) => line,
    }
}

// The board's part: a keeper runs while it is on. The start goes through a short-lived kebbie, so the keeper is never the board's child.
pub fn ensure() {
    let Ok(dir) = crate::lead::state_dir() else { return };
    if !should_start(enabled(), holder(&dir).is_some()) {
        return;
    }
    let Ok(exe) = crate::peers::kebbie_command() else { return };
    if let Ok(child) = Command::new(exe).args(["keeper", "--start"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        crate::sh::reap(child);
    }
}

fn pause(seconds: u64) {
    let until = Instant::now() + Duration::from_secs(seconds);
    while !STOP.load(Ordering::SeqCst) && Instant::now() < until {
        std::thread::sleep(SLICE.min(until.saturating_duration_since(Instant::now())));
    }
}

pub fn run() -> Result<String> {
    if !enabled() {
        return Err(OFF.to_string());
    }
    let dir = crate::lead::state_dir()?;
    let exe = PathBuf::from(crate::peers::kebbie_command()?);
    let Some(held) = acquire(&dir, &exe, 30)? else {
        let pid = holder(&dir).and_then(|running| running.pid).map_or("?".to_string(), |pid| pid.to_string());
        return Ok(format!("keeper: already running (pid {pid})"));
    };
    catch_stop();
    crate::login_env::import();
    let log = Log(dir.join(LOG));
    let me = Holder { pid: Some(std::process::id()), exe: Some(exe.to_string_lossy().to_string()), started: Some(crate::messages::now()) };
    log.line(&format!("started: pid {}, kebbie {}, {}", std::process::id(), env!("CARGO_PKG_VERSION"), exe.display()));
    let routines = routines();
    let mut tracks = vec![Track::default(); routines.len()];
    let mut ctx = Ctx::default();
    let mut binary = Binary::new(identity(&exe));
    let reason = loop {
        if STOP.load(Ordering::SeqCst) {
            break "stopped".to_string();
        }
        if !held.still_ours() {
            break "its lock file is gone".to_string();
        }
        if !enabled() {
            break "[keeper] enabled = false".to_string();
        }
        if binary.replaced(identity(&exe)) {
            break "a new kebbie binary is in place".to_string();
        }
        let now = crate::messages::now();
        let ran = due(&routines, &tracks, now);
        for &index in &ran {
            if STOP.load(Ordering::SeqCst) {
                break;
            }
            let routine = &routines[index];
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (routine.run)(&mut ctx))).unwrap_or_else(|_| Err("panicked".to_string()));
            if let Some(change) = record(&mut tracks[index], now, outcome) {
                let next = if tracks[index].failures > 0 { format!("; next try in {}", span(delay(routine.every, tracks[index].failures))) } else { String::new() };
                log.line(&format!("{}: {change}{next}", routine.name));
            }
            for note in ctx.notes.drain(..) {
                log.line(&note);
            }
        }
        if !ran.is_empty() {
            let _ = crate::state::write(&dir.join(STATUS), status_json(&me, &routines, &tracks, &ctx.stale, ctx.stale_at).to_string());
        }
        pause(wait(&routines, &tracks, crate::messages::now()));
    };
    log.line(&format!("exits: {reason}"));
    let replaced = reason.starts_with("a new kebbie");
    drop(held);
    if replaced {
        match spawn(&exe, &dir) {
            Ok(child) => log.line(&format!("handed over to pid {}", child.id())),
            Err(error) => log.line(&error),
        }
    }
    Ok(format!("keeper: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routine(name: &'static str, every: u64) -> Routine {
        Routine { name, every, run: |_| Ok(()) }
    }

    #[test]
    fn a_dead_keepers_lock_is_taken_over_and_a_live_one_is_not() {
        let dir = crate::sh::test_dir("keeper-lock");
        fs::write(dir.join(LOCK), r#"{"pid":999999,"exe":"/old/kebbie","started":1}"#).unwrap();
        assert_eq!(holder(&dir), None, "a lock file nobody holds is no keeper, whatever pid it names");
        let held = acquire(&dir, Path::new("/new/kebbie"), 1).unwrap().expect("a dead pid's lock is free");
        let running = holder(&dir).unwrap();
        assert_eq!((running.pid, running.exe.as_deref()), (Some(std::process::id()), Some("/new/kebbie")));
        assert!(acquire(&dir, Path::new("/other/kebbie"), 2).unwrap().is_none(), "a second keeper never runs beside the first");
        assert_eq!(fs::metadata(dir.join(LOCK)).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(held.still_ours());
        drop(held);
        assert_eq!(holder(&dir), None, "the lock goes with its holder");
        let again = acquire(&dir, Path::new("/new/kebbie"), 1).unwrap().unwrap();
        fs::remove_file(dir.join(LOCK)).unwrap();
        assert!(!again.still_ours(), "a removed state dir tells the keeper to go");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_scheduler_runs_what_is_due_and_backs_off_on_errors() {
        let routines = [routine("messages", 3), routine("restart", 60), routine("updates", 600)];
        let mut tracks = vec![Track::default(); 3];
        assert_eq!(due(&routines, &tracks, 1000), [0, 1, 2], "everything runs on the first round");
        for track in &mut tracks {
            record(track, 1000, Ok(()));
        }
        assert!(due(&routines, &tracks, 1002).is_empty());
        assert_eq!(due(&routines, &tracks, 1003), [0]);
        assert_eq!(due(&routines, &tracks, 1060), [0, 1]);
        assert_eq!(wait(&routines, &tracks, 1001), 2);
        assert_eq!(wait(&[routine("updates", 600)], &[Track { last: Some(1000), ..Track::default() }], 1000), 60, "never sleeps past a minute");
        assert_eq!(record(&mut tracks[1], 1060, Err("herdr agent list failed".to_string())).as_deref(), Some("herdr agent list failed"));
        assert_eq!(next_at(60, &tracks[1]), 1060 + 120);
        assert_eq!(record(&mut tracks[1], 1180, Err("herdr agent list failed".to_string())), None, "the same error is logged once");
        assert_eq!(next_at(60, &tracks[1]), 1180 + 240);
        assert_eq!(delay(60, 30), MAX_BACKOFF, "backing off stops at a quarter of an hour");
        assert_eq!(delay(3, 30), MAX_BACKOFF);
        assert_eq!(delay(3600, 5), 3600, "a routine slower than the cap keeps its own interval");
        assert_eq!(record(&mut tracks[1], 1420, Ok(())).as_deref(), Some("recovered"));
        assert_eq!((tracks[1].failures, next_at(60, &tracks[1])), (0, 1480));
    }

    #[test]
    fn the_routines_have_one_name_each_and_none_spins() {
        let routines = routines();
        let mut names: Vec<&str> = routines.iter().map(|routine| routine.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), routines.len());
        assert!(routines.iter().all(|routine| routine.every >= 3));
        assert_eq!(routines.iter().find(|routine| routine.name == "restart").map(|routine| routine.every), Some(60));
    }

    #[test]
    fn a_new_binary_is_seen_once_it_holds_still() {
        let dir = crate::sh::test_dir("keeper-binary");
        let exe = dir.join("kebbie");
        fs::write(&exe, "old").unwrap();
        let mut binary = Binary::new(identity(&exe));
        assert!(!binary.replaced(identity(&exe)));
        fs::remove_file(&exe).unwrap();
        assert!(!binary.replaced(identity(&exe)), "a binary mid-build is not started");
        fs::write(&exe, "").unwrap();
        assert!(!binary.replaced(identity(&exe)), "nor an empty one");
        fs::write(&exe, "the new one").unwrap();
        assert!(!binary.replaced(identity(&exe)), "changed, but not yet seen twice");
        assert!(binary.replaced(identity(&exe)));
        let link = dir.join("current");
        let other = dir.join("other");
        fs::write(&other, "the new one").unwrap();
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        let mut linked = Binary::new(identity(&link));
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(!linked.replaced(identity(&link)) && linked.replaced(identity(&link)), "a link moved to another install counts too");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_status_names_the_pid_the_binary_and_each_routines_last_run() {
        let log = Path::new("/state/keeper.log");
        let me = Holder { pid: Some(4242), exe: Some("/home/u/.local/share/kebbie/current/target/release/kebbie".to_string()), started: Some(10_000) };
        let routines = [routine("messages", 3), routine("restart", 60), routine("updates", 600)];
        let tracks = [
            Track { last: Some(17_198), ..Track::default() },
            Track { last: Some(17_160), failures: 2, error: Some("herdr agent list failed".to_string()) },
            Track::default(),
        ];
        let status = status_json(&me, &routines, &tracks, &["billing: restarts when idle".to_string()], 17_160);
        assert_eq!(render_status(true, Some(&me), Some(&status), log, 17_200), [
            "keeper: running, pid 4242, up 2h",
            "binary: /home/u/.local/share/kebbie/current/target/release/kebbie",
            "log: /state/keeper.log",
            "routines:",
            "  messages  every 3s   last 2s ago",
            "  restart   every 1m   last 40s ago, failed 2 times in a row: herdr agent list failed",
            "  updates   every 10m  not run yet",
        ].join("\n"));
        let other = Holder { pid: Some(7), ..me.clone() };
        assert!(render_status(true, Some(&other), Some(&status), log, 17_200).ends_with("routines: none has run yet"), "another keeper's leftovers are not this one's");
        assert_eq!(render_status(true, None, Some(&status), log, 17_200), "keeper: not running; kebbie keeper --start starts it, and so do herdr's startup and the board\nlog: /state/keeper.log");
        assert!(render_status(false, None, None, log, 0).starts_with(OFF));
        assert!(render_status(false, Some(&me), None, log, 17_200).contains("enabled = false: it stops at its next round"));
    }

    #[test]
    fn the_board_starts_a_keeper_only_when_it_is_on_and_none_runs_and_never_runs_the_routines_beside_it() {
        assert!(should_start(true, false));
        assert!(!should_start(true, true), "one keeper is enough");
        assert!(!should_start(false, false), "with the keeper off the board runs the routines itself");
        for enabled in [true, false] {
            let board_runs = owner(enabled) == Owner::Board;
            let keeper_runs = owner(enabled) == Owner::Keeper;
            assert!(board_runs != keeper_runs, "exactly one of them runs the routines, enabled = {enabled}");
        }
        assert_eq!(owner(false), Owner::Board);
    }

    #[test]
    fn the_board_shows_the_stale_sessions_only_a_running_keeper_saw() {
        let dir = crate::sh::test_dir("keeper-stale");
        let me = Holder { pid: Some(std::process::id()), exe: None, started: None };
        fs::write(dir.join(STATUS), status_json(&me, &[], &[], &["api: restarts when idle".to_string()], 500).to_string()).unwrap();
        assert_eq!(stale_in(&dir), (0, Vec::new()), "no keeper holds the lock");
        let held = acquire(&dir, Path::new("/kebbie"), 1).unwrap().unwrap();
        assert_eq!(stale_in(&dir), (500, vec!["api: restarts when idle".to_string()]), "with the round's time, so a board can tell a reading from before an update");
        drop(held);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stopping_without_a_keeper_says_so_and_off_starts_nothing() {
        let dir = crate::sh::test_dir("keeper-off");
        assert_eq!(stop_in(&dir, Duration::from_millis(10)).unwrap(), "keeper: not running");
        assert_eq!(start_in(&dir, Path::new("/no/such/kebbie"), false).unwrap(), OFF);
        assert!(!dir.join(LOG).exists(), "nothing was spawned");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn log_lines_carry_a_utc_time() {
        assert_eq!(stamp(0), "1970-01-01 00:00:00Z");
        assert_eq!(stamp(1_792_497_785), "2026-10-20 12:03:05Z");
        assert_eq!(span(59), "59s");
        assert_eq!(span(3 * 3600 + 120), "3h 2m");
        assert_eq!(span(2 * 86_400 + 3600), "2d 1h");
    }
}
