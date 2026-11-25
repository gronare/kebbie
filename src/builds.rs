use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::sh::Result;

pub const UPDATE: &str = "kebbie update";
pub const EVERY: u64 = 3;
pub const HOLD: u64 = 180;
const LINGER: u64 = 10;
const UPDATE_HOLD: u64 = 30;
const MAX_HOLD: u64 = 30 * 60;
const UPDATE_WAIT: Duration = Duration::from_secs(10 * 60);
const KEEPER_QUIET: u64 = 60;
pub const QUEUED_EXIT: u8 = 75;
const HELD_VAR: &str = "KEBBIE_BUILD_SLOT";
const GUARD_DIR: &str = "build-guard";
const QUEUE: &str = "queue.json";
const CARGO_BUILDS: [&str; 16] = ["build", "b", "check", "c", "test", "t", "clippy", "bench", "doc", "d", "install", "rustc", "rustdoc", "fix", "nextest", "llvm-cov"];
const CARGO_VALUED: [&str; 5] = ["-C", "--config", "-Z", "--color", "--manifest-path"];
const QUEUE_LOCK_WAIT: Duration = Duration::from_secs(2);
const QUEUE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queue {
    pub slots: Option<u32>,
    pub commands: Option<Vec<String>>,
    pub holders: Vec<Holder>,
    pub line: Vec<String>,
    pub turns: Vec<Turn>,
    pub ticked: u64,
    pub builds_below: Option<crate::disk::Limit>,
    pub low_disk: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Holder {
    pub address: String,
    pub id: String,
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub address: String,
    pub until: u64,
    pub told: bool,
}

#[derive(Debug, PartialEq)]
pub enum Take {
    Taken,
    Queued(usize),
    NoKeeper,
}

// A holder id becomes a file name and an address a message's recipient, and sandboxed tasks write the queue, so only plain tokens pass.
fn plain(word: &str) -> bool {
    !word.is_empty() && word.len() <= 128 && !word.starts_with('.') && word.chars().all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
}

fn addressed(address: &str) -> bool {
    address == UPDATE || address.split_once('@').is_some_and(|(task, repo)| plain(task) && plain(repo))
}

impl Queue {
    fn drop_unsound(&mut self) {
        self.holders.retain(|holder| plain(&holder.id) && addressed(&holder.address));
        self.line.retain(|address| addressed(address));
        self.turns.retain(|turn| addressed(&turn.address));
    }

    // A lapsed turn ends here too, so a queue no keeper ticks still moves; one its task was told of is left to a ticking keeper to tell it the turn passed.
    fn lapse(&mut self, now: u64) {
        let keeper = now.saturating_sub(self.ticked) <= KEEPER_QUIET;
        self.turns.retain(|turn| turn.until > now || (turn.told && keeper));
    }

    fn slots(&self) -> usize {
        self.slots.unwrap_or(crate::config::DEFAULT_BUILD_SLOTS).max(1) as usize
    }

    fn used(&self) -> usize {
        self.holders.len() + self.turns.len()
    }

    pub fn place(&self, address: &str) -> Option<usize> {
        self.line.iter().position(|queued| queued == address).map(|at| at + 1)
    }

    fn commands(&self) -> Vec<String> {
        self.commands.clone().unwrap_or_else(|| crate::config::DEFAULT_BUILD_COMMANDS.map(str::to_string).to_vec())
    }

    // A turn handed to the address, a slot it already holds, or a free slot with nobody ahead of it is taken; else it keeps or takes its place in line, unless no keeper is there to tell it its turn.
    pub fn take(&mut self, address: &str, id: &str, front: bool, needs_keeper: bool, now: u64) -> Take {
        self.lapse(now);
        let turn = self.turns.iter().position(|turn| turn.address == address);
        let holding = self.holders.iter().any(|holder| holder.address == address);
        let first = self.line.first().is_none_or(|queued| queued == address);
        if let Some(at) = turn {
            self.turns.remove(at);
        }
        if turn.is_some() || holding || (self.used() < self.slots() && first) {
            self.line.retain(|queued| queued != address);
            self.holders.push(Holder { address: address.to_string(), id: id.to_string(), at: now });
            return Take::Taken;
        }
        if needs_keeper && now.saturating_sub(self.ticked) > KEEPER_QUIET {
            return Take::NoKeeper;
        }
        if self.place(address).is_none() {
            if front { self.line.insert(0, address.to_string()) } else { self.line.push(address.to_string()) }
        }
        Take::Queued(self.place(address).unwrap_or(1))
    }

    // A build that ends while others wait keeps its slot a few seconds, so the next command of the same script finds it.
    pub fn release(&mut self, id: &str, now: u64) {
        self.lapse(now);
        let Some(at) = self.holders.iter().position(|holder| holder.id == id) else { return };
        let holder = self.holders.remove(at);
        if !self.line.is_empty() && !self.holders.iter().any(|other| other.address == holder.address) {
            self.turns.push(Turn { address: holder.address, until: now + LINGER, told: false });
        }
    }

    pub fn forget(&mut self, address: &str) {
        self.line.retain(|queued| queued != address);
        self.turns.retain(|turn| turn.address != address);
    }

    // kebbie update stays first, then the heads of the push lines, whose pushes wait on their builds; the rest keep their order.
    fn put_first(&mut self, heads: &[String]) {
        self.line.sort_by_key(|address| if address == UPDATE { 0 } else if heads.contains(address) { 1 } else { 2 });
    }

    // The keeper's round: a task whose own disk is low gets no turn and keeps its place, dead builds free their slots, as does one that held its slot MAX_HOLD while others wait, a turn not taken in time passes on, and each free slot goes to the next in line, a push line's head first; what to tell whom comes back.
    pub fn tick(&mut self, slots: u32, commands: &[String], heads: &[String], alive: impl Fn(&str) -> bool, now: u64) -> Vec<(String, String)> {
        self.slots = Some(slots);
        self.commands = Some(commands.to_vec());
        self.ticked = now;
        self.holders.retain(|holder| alive(&holder.id));
        let mut told = Vec::new();
        if !self.line.is_empty() {
            let (long, kept): (Vec<Holder>, Vec<Holder>) = std::mem::take(&mut self.holders).into_iter().partition(|holder| now.saturating_sub(holder.at) >= MAX_HOLD);
            self.holders = kept;
            told.extend(long.into_iter().filter(|holder| holder.address != UPDATE).map(|holder| (holder.address, OUTHELD.to_string())));
        }
        let (expired, kept): (Vec<Turn>, Vec<Turn>) = std::mem::take(&mut self.turns).into_iter().partition(|turn| turn.until <= now);
        self.turns = kept;
        self.put_first(heads);
        told.extend(expired.into_iter().filter(|turn| turn.told).map(|turn| (turn.address, MISSED.to_string())));
        let mut at = 0;
        while self.used() < self.slots() && at < self.line.len() {
            if self.low_disk.contains(&self.line[at]) {
                at += 1;
                continue;
            }
            let address = self.line.remove(at);
            let tell = address != UPDATE;
            self.turns.push(Turn { address: address.clone(), until: now + if tell { HOLD } else { UPDATE_HOLD }, told: tell });
            if tell {
                told.push((address, TURN.to_string()));
            }
        }
        told
    }
}

pub const TURN: &str = "kebbie: a build slot is free for you; run your build within 3 minutes, or it passes to the next in line.";
pub const OUTHELD: &str = "kebbie: your build held its build slot 30 minutes while other tasks waited, so the slot passed to the next in line; the build runs on outside the queue.";
pub const MISSED: &str = "kebbie: your build turn passed unused to the next in line; run the build again to queue.";

#[derive(Clone, Debug, PartialEq)]
pub enum Blocked {
    Full(String),
    Ahead(String),
    Next,
}

impl Blocked {
    fn of(queue: &Queue, address: &str) -> Blocked {
        if queue.used() >= queue.slots() {
            return Blocked::Full(holding(&queue.holders, &queue.turns));
        }
        let ahead: Vec<&str> = queue.line.iter().take_while(|queued| *queued != address).map(String::as_str).collect();
        if ahead.is_empty() { Blocked::Next } else { Blocked::Ahead(ahead.join(", ")) }
    }

    pub fn who(&self) -> String {
        match self {
            Blocked::Full(holding) => holding.clone(),
            Blocked::Ahead(ahead) => format!("{ahead} ahead in line"),
            Blocked::Next => "one is free and kebbie hands it here next".to_string(),
        }
    }
}

pub fn queued_line(place: usize, blocked: &Blocked) -> String {
    let place = crate::tasks::ordinal(place);
    match blocked {
        Blocked::Full(holding) => format!("kebbie: no build slot is free for this task ({holding}); it is {place} in line, and kebbie tells it when its turn comes."),
        Blocked::Ahead(ahead) => format!("kebbie: a build slot is free, but it goes first to the tasks ahead of this one ({ahead}); it is {place} in line, and kebbie tells it when its turn comes."),
        Blocked::Next => format!("kebbie: a build slot is free and kebbie hands it to this task next; it is {place} in line, and kebbie tells it when its turn comes."),
    }
}

pub fn holding(holders: &[Holder], turns: &[Turn]) -> String {
    let building: Vec<&str> = holders.iter().map(|holder| holder.address.as_str()).collect();
    let mut parts: Vec<String> = turns.iter().map(|turn| format!("{} has its turn", turn.address)).collect();
    if !building.is_empty() {
        parts.insert(0, format!("{} building", building.join(", ")));
    }
    parts.join("; ")
}

// The keeper publishes the limit in the queue, as a sandboxed task reads no config, and the guard measures the disk under the build itself.
pub fn disk_refusal(space: Option<crate::disk::Space>, limit: Option<crate::disk::Limit>) -> Option<String> {
    let (space, limit) = (space?, limit?);
    space.below(limit).then(|| format!("kebbie: the disk under this worktree has {} free, below the {} a build needs (disk.builds_below), so this build takes no build slot and does not run. The keeper trims build output from worktrees nobody uses when the disk runs low; free space and build again.", crate::disk::gb(space.free), limit.shown()))
}

// Whether a command's output holds a line a build queued for a slot prints before it exits without building, whatever held it back.
pub fn queued_in(output: &str) -> bool {
    output.lines().any(|line| line.trim_start().starts_with("kebbie: ") && line.trim_end().ends_with(" in line, and kebbie tells it when its turn comes."))
}

pub fn board_note(place: usize) -> String {
    format!("waiting for a build slot, {}", crate::tasks::ordinal(place))
}

// Sandboxed tasks may write ~/.cache and nothing of kebbie's state, so the queue lives there.
pub fn dir() -> Result<PathBuf> {
    if cfg!(test) {
        return Ok(crate::lead::state_location()?.join("builds"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/kebbie/builds")).ok_or_else(|| "HOME is not set".to_string())
}

fn read_plain(file: &Path) -> Option<String> {
    use std::io::Read;
    let open = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(file).ok()?;
    if !open.metadata().is_ok_and(|meta| meta.is_file()) {
        return None;
    }
    let mut text = String::new();
    open.take(QUEUE_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

pub fn load(dir: &Path) -> Queue {
    let mut queue: Queue = read_plain(&dir.join(QUEUE)).and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    queue.drop_unsound();
    queue
}

fn with_queue<T>(dir: &Path, change: impl FnOnce(&mut Queue) -> T) -> Result<T> {
    crate::state::private_dir(dir)?;
    let _lock = crate::state::Locked::within(&dir.join("queue.lock"), QUEUE_LOCK_WAIT)?;
    let mut queue = load(dir);
    let out = change(&mut queue);
    crate::state::write(&dir.join(QUEUE), serde_json::to_string(&queue).map_err(|error| error.to_string())?)?;
    Ok(out)
}

pub fn place(address: &str) -> Option<usize> {
    dir().ok().and_then(|dir| load(&dir).place(address))
}

pub fn forget(address: &str) {
    if let Ok(dir) = dir() {
        let _ = with_queue(&dir, |queue| queue.forget(address));
    }
}

fn try_flock(file: &File) -> bool {
    // SAFETY: flock only reads the descriptor, which the File keeps open.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

fn held_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("held-{id}.lock"))
}

// A build holds a lock on its own file for as long as it runs, which the kernel drops however it ends, across a sandbox's process namespace too.
fn alive_in(dir: &Path, id: &str) -> bool {
    if !plain(id) {
        return false;
    }
    let file = held_file(dir, id);
    let Ok(open) = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(&file) else { return false };
    if !open.metadata().is_ok_and(|meta| meta.is_file()) {
        return false;
    }
    if try_flock(&open) {
        let _ = fs::remove_file(&file);
        return false;
    }
    true
}

pub struct Slot {
    _file: File,
    dir: PathBuf,
    id: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let _ = with_queue(&self.dir, |queue| {
            queue.release(&self.id, crate::messages::now());
            queue.holders.retain(|holder| alive_in(&self.dir, &holder.id));
        });
        let _ = fs::remove_file(held_file(&self.dir, &self.id));
    }
}

pub enum Took {
    Slot(Slot),
    Queued(usize, Blocked),
    NoKeeper,
}

pub fn take_slot(dir: &Path, address: &str, front: bool, needs_keeper: bool) -> Result<Took> {
    crate::state::private_dir(dir)?;
    let id = crate::messages::new_id("build");
    let path = held_file(dir, &id);
    let file = OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !try_flock(&file) {
        return Err(format!("{}: could not be locked", path.display()));
    }
    let (took, held) = with_queue(dir, |queue| {
        queue.holders.retain(|holder| alive_in(dir, &holder.id));
        let took = queue.take(address, &id, front, needs_keeper, crate::messages::now());
        (took, Blocked::of(queue, address))
    })?;
    match took {
        Take::Taken => Ok(Took::Slot(Slot { _file: file, dir: dir.to_path_buf(), id })),
        Take::Queued(place) => {
            let _ = fs::remove_file(&path);
            Ok(Took::Queued(place, held))
        }
        Take::NoKeeper => {
            let _ = fs::remove_file(&path);
            Ok(Took::NoKeeper)
        }
    }
}

// kebbie update goes first in line and waits there at most UPDATE_WAIT, since the user is watching it; a queue it cannot use lets it build at once.
pub fn wait_for_update() -> Option<Slot> {
    wait_in(&dir().ok()?, UPDATE_WAIT, Duration::from_secs(2))
}

fn wait_in(dir: &Path, limit: Duration, pause: Duration) -> Option<Slot> {
    let until = std::time::Instant::now() + limit;
    let mut said = false;
    loop {
        if std::time::Instant::now() >= until {
            eprintln!("still no build slot after {} minutes; building without one", limit.as_secs() / 60);
            return None;
        }
        match take_slot(dir, UPDATE, true, false) {
            Ok(Took::Slot(slot)) => return Some(slot),
            Ok(Took::Queued(_, held)) => {
                if !said {
                    eprintln!("waiting for a build slot: {}", held.who());
                    said = true;
                }
                std::thread::sleep(pause);
            }
            Ok(Took::NoKeeper) | Err(_) => return None,
        }
    }
}

pub fn tick_now() -> Result<Vec<String>> {
    let dir = dir()?;
    let (slots, commands) = crate::config::build_settings().unwrap_or_else(|_| (crate::config::DEFAULT_BUILD_SLOTS, crate::config::DEFAULT_BUILD_COMMANDS.map(str::to_string).to_vec()));
    let heads = crate::lead::state_dir().map(|state| crate::tasks::push_heads(&state, &crate::messages::load())).unwrap_or_default();
    let builds_below = crate::config::disk_limits().map_or(crate::disk::DEFAULT_BUILDS_BELOW, |(_, builds_below)| builds_below);
    let low = low_disk(builds_below);
    let told = with_queue(&dir, |queue| {
        queue.low_disk = queue.line.iter().filter(|address| low.contains(address)).cloned().collect();
        queue.builds_below = Some(builds_below);
        queue.tick(slots, &commands, &heads, |id| alive_in(&dir, id), crate::messages::now())
    })?;
    if told.is_empty() {
        return Ok(Vec::new());
    }
    let sessions = crate::peers::sessions().unwrap_or_default();
    let known = |address: &str| sessions.iter().any(|session| session.task.is_some() && session.address == address);
    Ok(told.into_iter().map(|(address, text)| {
        if !known(&address) {
            return format!("builds: told nobody: {address} is no task running here");
        }
        match crate::messages::hand_to(&address, &text) {
            Ok(()) => format!("builds: told {address}: {text}"),
            Err(error) => format!("builds: could not tell {address}: {error}"),
        }
    }).collect())
}

// The tasks whose own worktree's disk is below the limit, as the guard would refuse their build; read outside the queue's lock and matched to the line inside it, and the task list is read only when some pooled disk is low.
fn low_disk(builds_below: crate::disk::Limit) -> Vec<String> {
    let low = |path: &Path| crate::disk::space(path).is_some_and(|space| space.below(builds_below));
    if !crate::repo::pooled_checkouts().iter().any(|repo| low(repo)) {
        return Vec::new();
    }
    crate::status::gather().unwrap_or_default().into_iter().filter_map(|item| {
        let (path, repo) = (item.path.as_deref()?, item.repo.as_deref()?);
        let address = crate::peers::address(&item.name, &crate::repo::name_of(repo));
        low(path).then_some(address)
    }).collect()
}

// Whether this call of the program builds: cargo's compiling subcommands, or a configured command's first words.
pub fn builds(program: &str, args: &[String], commands: &[String]) -> bool {
    commands.iter().any(|command| {
        let mut words = command.split_whitespace();
        if words.next() != Some(program) {
            return false;
        }
        let rest: Vec<&str> = words.collect();
        if program == "cargo" && rest.is_empty() {
            return cargo_subcommand(args).is_some_and(|sub| CARGO_BUILDS.contains(&sub));
        }
        rest.len() <= args.len() && rest.iter().zip(args).all(|(want, given)| want == given)
    })
}

fn cargo_subcommand(args: &[String]) -> Option<&str> {
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if CARGO_VALUED.contains(&arg.as_str()) {
            skip = true;
            continue;
        }
        if !arg.starts_with('-') && !arg.starts_with('+') {
            return Some(arg);
        }
    }
    None
}

fn is_guard(file: &Path) -> bool {
    fs::read(file).is_ok_and(|bytes| bytes.windows(b" build-guard ".len()).any(|window| window == b" build-guard "))
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default()
}

// The program the guard stands in front of: the first on PATH that is no guard of kebbie's.
fn real_program(program: &str, path: &[PathBuf]) -> Option<PathBuf> {
    path.iter().map(|dir| dir.join(program)).find(|file| fs::metadata(file).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0) && !is_guard(file))
}

// cargo reads a wrapper from RUSTC_WRAPPER, CARGO_BUILD_RUSTC_WRAPPER and build.rustc-wrapper in its config files, nearest folder first, then CARGO_HOME's.
fn configured_wrapper(env: impl Fn(&str) -> Option<String>, cwd: &Path, cargo_home: &Path) -> Option<String> {
    if let Some(wrapper) = env("RUSTC_WRAPPER").or_else(|| env("CARGO_BUILD_RUSTC_WRAPPER")) {
        return Some(wrapper);
    }
    let configs = cwd.ancestors().map(|dir| dir.join(".cargo")).chain([cargo_home.to_path_buf()]);
    configs.flat_map(|dir| [dir.join("config.toml"), dir.join("config")]).find_map(|file| {
        let config = fs::read_to_string(file).ok()?.parse::<toml::Table>().ok()?;
        let wrapper = config.get("build")?.get("rustc-wrapper")?;
        Some(wrapper.as_str().unwrap_or_default().to_string())
    })
}

// The sccache whose server the build will use, and whether the guard sets it as the wrapper: when no wrapper is set, or when the one set is sccache itself.
fn sccache_for(wrapper: Option<&str>, path: &[PathBuf]) -> Option<(PathBuf, bool)> {
    let Some(wrapper) = wrapper else { return real_program("sccache", path).map(|sccache| (sccache, true)) };
    let named = Path::new(wrapper);
    if named.file_name().is_none_or(|name| name != "sccache") {
        return None;
    }
    let sccache = if named.is_absolute() { named.to_path_buf() } else if named.components().count() == 1 { real_program("sccache", path)? } else { return None };
    Some((sccache, false))
}

fn cache_wrapper(path: &[PathBuf]) -> Option<(PathBuf, bool)> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".cargo"));
    let cwd = std::env::current_dir().ok()?;
    sccache_for(configured_wrapper(|name| std::env::var(name).ok(), &cwd, &cargo_home).as_deref(), path)
}

// The sccache server makes its temp files under the TMPDIR it started with, so it starts under one no task owns and that a sandboxed task may write.
fn server_tmpdir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache/kebbie/sccache-tmp")
}

fn environ_var(environ: &[u8], name: &str) -> Option<String> {
    environ.split(|byte| *byte == 0).find_map(|var| var.strip_prefix(name.as_bytes())?.strip_prefix(b"=")).map(|value| String::from_utf8_lossy(value).into_owned())
}

fn environ_tmpdir(environ: &[u8]) -> PathBuf {
    environ_var(environ, "TMPDIR").filter(|dir| !dir.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

// Where an sccache client or server reaches the server, read as sccache reads it: a socket when SCCACHE_SERVER_UDS is set, else SCCACHE_SERVER_PORT, else 4226.
fn server_address(var: impl Fn(&str) -> Option<String>) -> String {
    match var("SCCACHE_SERVER_UDS") {
        Some(socket) => format!("uds:{socket}"),
        None => format!("port:{}", var("SCCACHE_SERVER_PORT").map(|port| port.trim().to_string()).unwrap_or_else(|| "4226".to_string())),
    }
}

// The TMPDIR of each sccache server this user runs at the address, the one --stop-server reaches, or None where the processes' environments cannot be read.
fn running_servers(address: &str) -> Option<Vec<PathBuf>> {
    let procs = fs::read_dir("/proc").ok()?;
    let servers = procs.flatten().map(|entry| entry.path()).filter(|proc| fs::read_to_string(proc.join("comm")).is_ok_and(|comm| comm.trim() == "sccache"));
    let environs = servers.filter_map(|proc| fs::read(proc.join("environ")).ok()).filter(|environ| environ_var(environ, "SCCACHE_START_SERVER").as_deref() == Some("1"));
    Some(environs.filter(|environ| server_address(|name| environ_var(environ, name)) == address).map(|environ| environ_tmpdir(&environ)).collect())
}

#[derive(Debug, PartialEq)]
enum ServerStep {
    Stop,
    Start,
}

// A server whose TMPDIR is gone fails every compile, and one under a task's temp folder will once that task ends, so either is stopped; a server under a folder no task owns is left running, and a start is tried where no server is known to run, which fails harmlessly when one does.
fn server_steps(running: Option<&[PathBuf]>, task_tmp_root: &Path) -> Vec<ServerStep> {
    let Some(running) = running else { return vec![ServerStep::Start] };
    let breaks = |dir: &PathBuf| fs::canonicalize(dir).map_or(true, |dir| dir.starts_with(task_tmp_root));
    if running.iter().any(breaks) {
        vec![ServerStep::Stop, ServerStep::Start]
    } else if running.is_empty() {
        vec![ServerStep::Start]
    } else {
        vec![]
    }
}

fn server_command(sccache: &Path, step: &ServerStep, tmpdir: &Path) -> Command {
    let mut command = Command::new(sccache);
    command.arg(if *step == ServerStep::Stop { "--stop-server" } else { "--start-server" }).env("TMPDIR", tmpdir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    command
}

// Builds starting at once check and restart under one lock, so none stops the server another has just started.
fn prepare_server(sccache: &Path, running: impl Fn() -> Option<Vec<PathBuf>>, tmpdir: &Path, task_tmp_root: &Path) {
    if fs::create_dir_all(tmpdir).is_err() {
        return;
    }
    let lock = tmpdir.with_file_name("sccache-server.lock");
    let Ok(file) = OpenOptions::new().create(true).truncate(false).write(true).custom_flags(libc::O_NOFOLLOW).open(&lock) else { return };
    // SAFETY: flock only reads the descriptor, which the File keeps open; the kernel drops the lock when it closes.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return;
    }
    for step in server_steps(running().as_deref(), task_tmp_root) {
        let _ = server_command(sccache, &step, tmpdir).status();
    }
}

extern "C" fn keep_running(_: libc::c_int) {}

// The build's own Ctrl-C reaches it; the guard lives on to free its slot.
fn outlive_interrupts() {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: installs a handler that does nothing; exec resets it to the default in the child.
        unsafe { libc::signal(signal, keep_running as *const () as libc::sighandler_t) };
    }
}

fn exit_of(status: std::io::Result<std::process::ExitStatus>, program: &str) -> ExitCode {
    use std::os::unix::process::ExitStatusExt;
    match status {
        Ok(status) => ExitCode::from(status.code().or_else(|| status.signal().map(|signal| 128 + signal)).unwrap_or(1).clamp(0, 255) as u8),
        Err(error) => {
            eprintln!("kebbie: {program} could not start: {error}");
            ExitCode::from(127)
        }
    }
}

fn task_address() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let (repo, slot) = crate::tasks::task_at(&cwd)?;
    Some(crate::peers::address(slot.task.as_deref()?, &crate::repo::name_of(&repo)))
}

// kebbie build-guard <program> <args>: what the guard script on a task's PATH runs in its program's place.
pub fn guard(argv: &[String]) -> ExitCode {
    let Some((program, args)) = argv.split_first() else {
        eprintln!("kebbie build-guard needs the program it stands in for");
        return ExitCode::from(2);
    };
    let path = path_dirs();
    let Some(real) = real_program(program, &path) else {
        eprintln!("kebbie: {program} is not on PATH");
        return ExitCode::from(127);
    };
    let mut command = Command::new(&real);
    command.args(args);
    let sccache = if program == "cargo" { cache_wrapper(&path) } else { None };
    if let Some((sccache, true)) = &sccache {
        command.env("RUSTC_WRAPPER", sccache);
    }
    let run = |command: &mut Command| {
        if let Some((sccache, _)) = &sccache {
            prepare_server(sccache, || running_servers(&server_address(|name| std::env::var(name).ok())), &server_tmpdir(), &crate::tasks::task_tmp_root());
        }
        command.status()
    };
    let queue = dir();
    let loaded = queue.as_ref().map(|dir| load(dir)).unwrap_or_default();
    let gated = std::env::var_os(HELD_VAR).is_none() && queue.is_ok() && builds(program, args, &loaded.commands());
    let address = if gated { task_address() } else { None };
    let (Ok(dir), Some(address)) = (queue, address) else { return exit_of(run(&mut command), program) };
    if let Some(refusal) = disk_refusal(std::env::current_dir().ok().and_then(|cwd| crate::disk::space(&cwd)), loaded.builds_below) {
        eprintln!("{refusal}");
        return ExitCode::from(QUEUED_EXIT);
    }
    let slot = match take_slot(&dir, &address, false, true) {
        Ok(Took::Slot(slot)) => slot,
        Ok(Took::Queued(place, held)) => {
            eprintln!("{}", queued_line(place, &held));
            return ExitCode::from(QUEUED_EXIT);
        }
        Ok(Took::NoKeeper) => {
            eprintln!("kebbie: no build slot is free for this task and no keeper runs to hand out turns; building without waiting.");
            return exit_of(run(&mut command), program);
        }
        Err(error) => {
            eprintln!("kebbie: the build queue could not be used ({error}); building without waiting.");
            return exit_of(run(&mut command), program);
        }
    };
    outlive_interrupts();
    let status = run(command.env(HELD_VAR, &address));
    drop(slot);
    exit_of(status, program)
}

pub fn script(kebbie: &str, program: &str) -> String {
    format!("#!/bin/sh\nexec {} build-guard {program} \"$@\"\n", crate::sh::quote(kebbie))
}

pub fn programs(commands: &[String]) -> Vec<String> {
    let mut programs: Vec<String> = commands.iter().filter_map(|command| command.split_whitespace().next()).map(str::to_string).collect();
    programs.sort();
    programs.dedup();
    programs
}

// One guard script per configured program, and none for a program the config no longer lists.
pub fn write_scripts(dir: &Path, kebbie: &str, commands: &[String]) -> Result<()> {
    crate::state::private_dir(dir)?;
    let programs = programs(commands);
    for entry in fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?.flatten() {
        if !programs.iter().any(|program| entry.file_name() == program.as_str()) {
            let _ = fs::remove_file(entry.path());
        }
    }
    for program in &programs {
        let file = dir.join(program);
        let text = script(kebbie, program);
        if fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
            crate::state::write(&file, text)?;
        }
        fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).map_err(|error| format!("{}: {error}", file.display()))?;
    }
    Ok(())
}

// Claude Code sources the env file before each command its shell runs, after the user's own shell setup, so the guard stays first on PATH.
pub fn env_line(dir: &Path) -> String {
    let dir = crate::sh::quote(&dir.to_string_lossy());
    format!("case \":$PATH:\" in *:{dir}:*) ;; *) export PATH={dir}:\"$PATH\" ;; esac\n")
}

// kebbie's SessionStart hook for a task: its guard scripts, and their folder first on the PATH of every command the session runs.
pub fn hook() -> String {
    let (Ok(state), Ok(kebbie), Some(env_file)) = (crate::lead::state_dir(), crate::peers::kebbie_command(), std::env::var_os("CLAUDE_ENV_FILE")) else { return String::new() };
    let commands = crate::config::build_settings().map(|(_, commands)| commands).unwrap_or_else(|_| crate::config::DEFAULT_BUILD_COMMANDS.map(str::to_string).to_vec());
    let guards = state.join(GUARD_DIR);
    if let Err(error) = write_scripts(&guards, &kebbie, &commands) {
        eprintln!("kebbie: the build guard could not be written ({error}); builds run without the build queue.");
        return String::new();
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(env_file) {
        let _ = std::io::Write::write_all(&mut file, env_line(&guards).as_bytes());
    }
    String::new()
}

#[cfg(test)]
pub(crate) fn line_up(address: &str) {
    with_queue(&dir().unwrap(), |queue| queue.line.push(address.to_string())).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    fn queue(slots: u32, ticked: u64) -> Queue {
        Queue { slots: Some(slots), ticked, ..Queue::default() }
    }

    #[test]
    fn a_build_takes_a_free_slot_and_the_next_task_waits_in_line_by_its_place() {
        let mut queue = queue(1, 100);
        assert_eq!(queue.take("a@app", "1", false, true, 100), Take::Taken);
        assert_eq!(queue.take("b@app", "2", false, true, 100), Take::Queued(1));
        assert_eq!(queue.take("c@app", "3", false, true, 100), Take::Queued(2));
        assert_eq!(queue.take("b@app", "4", false, true, 101), Take::Queued(1), "a task asking again keeps its place");
        assert_eq!(queue.take("a@app", "5", false, true, 101), Take::Taken, "a task already building runs its second command, which cargo's own lock orders");
        assert_eq!(board_note(queue.place("c@app").unwrap()), "waiting for a build slot, 2nd");
    }

    #[test]
    fn kebbie_update_goes_first_in_line() {
        let mut queue = queue(1, 100);
        queue.take("a@app", "1", false, true, 100);
        queue.take("b@app", "2", false, true, 100);
        assert_eq!(queue.take(UPDATE, "3", true, false, 100), Take::Queued(1));
        assert_eq!(queue.line, strings(&[UPDATE, "b@app"]));
    }

    #[test]
    fn the_head_of_a_push_line_goes_before_other_tasks_but_after_kebbie_update() {
        let mut queue = queue(1, 100);
        queue.take("a@app", "1", false, true, 100);
        queue.take("b@app", "2", false, true, 100);
        queue.take("c@app", "3", false, true, 100);
        queue.take("head@app", "4", false, true, 100);
        queue.take(UPDATE, "5", true, false, 100);
        assert!(queue.tick(1, &[], &strings(&["head@app"]), |_| true, 101).is_empty(), "the slot is still a's");
        assert_eq!(queue.line, strings(&[UPDATE, "head@app", "b@app", "c@app"]));
        assert_eq!(queue.place("head@app"), Some(2));
        queue.line.retain(|address| address != UPDATE);
        let told = queue.tick(1, &[], &strings(&["head@app"]), |id| id != "1", 102);
        assert_eq!(told, vec![("head@app".to_string(), TURN.to_string())], "the freed slot goes to the push line's head");
        assert_eq!(queue.take("head@app", "6", false, true, 103), Take::Taken);
    }

    #[test]
    fn the_keeper_hands_a_freed_slot_to_the_next_in_line_and_holds_it_for_it() {
        let mut queue = queue(1, 100);
        queue.take("a@app", "1", false, true, 100);
        queue.take("b@app", "2", false, true, 100);
        queue.release("1", 110);
        assert_eq!(queue.take("c@app", "3", false, true, 111), Take::Queued(2), "the slot a finished build keeps a moment is not free for another");
        assert!(queue.tick(1, &[], &[], |_| true, 115).is_empty(), "the finished build's moment is not over");
        let told = queue.tick(1, &[], &[], |_| true, 121);
        assert_eq!(told, vec![("b@app".to_string(), TURN.to_string())]);
        assert_eq!(queue.take("c@app", "4", false, true, 122), Take::Queued(1), "the held slot is b's");
        assert_eq!(queue.take("b@app", "5", false, true, 130), Take::Taken);
    }

    #[test]
    fn a_turn_not_taken_in_time_passes_to_the_next_and_the_task_hears_it() {
        let mut queue = queue(1, 100);
        queue.take("a@app", "1", false, true, 100);
        queue.take("b@app", "2", false, true, 100);
        queue.take("c@app", "3", false, true, 100);
        queue.tick(1, &[], &[], |id| id != "1", 101);
        let told = queue.tick(1, &[], &[], |_| true, 101 + HOLD);
        assert_eq!(told, vec![("b@app".to_string(), MISSED.to_string()), ("c@app".to_string(), TURN.to_string())]);
    }

    #[test]
    fn a_build_that_died_frees_its_slot_and_a_script_keeps_its_slot_between_commands() {
        let mut queue = queue(1, 100);
        queue.take("a@app", "1", false, true, 100);
        queue.take("b@app", "2", false, true, 100);
        queue.release("1", 105);
        assert_eq!(queue.take("a@app", "3", false, true, 106), Take::Taken, "check.sh's next cargo finds the slot it just left");
        let mut died = queue.clone();
        died.tick(1, &[], &[], |_| false, 107);
        assert!(died.holders.is_empty() && died.turns[0].address == "b@app");
    }

    #[test]
    fn without_a_keeper_a_build_goes_ahead_and_joins_no_line() {
        let mut queue = queue(1, 0);
        queue.take("a@app", "1", false, true, 1000);
        assert_eq!(queue.take("b@app", "2", false, true, 1000), Take::NoKeeper);
        assert!(queue.line.is_empty());
    }

    #[test]
    fn without_a_keeper_a_lapsed_turn_frees_its_slot_for_kebbie_update() {
        let mut queue = queue(1, 0);
        queue.take("a@app", "1", false, false, 100);
        assert_eq!(queue.take(UPDATE, "2", true, false, 100), Take::Queued(1));
        queue.release("1", 101);
        assert_eq!(queue.take(UPDATE, "3", true, false, 105), Take::Queued(1), "a's script keeps its slot a moment");
        assert_eq!(queue.take(UPDATE, "4", true, false, 101 + LINGER), Take::Taken);
    }

    #[test]
    fn a_holder_whose_build_died_outright_frees_its_slot_without_a_keeper() {
        let dir = crate::sh::test_dir("builds-dead-holder");
        let dead = Queue { holders: vec![Holder { address: "a@app".into(), id: "1-1-0-build".into(), at: 1 }], ..queue(1, 0) };
        crate::state::write(&dir.join(QUEUE), serde_json::to_string(&dead).unwrap()).unwrap();
        fs::write(held_file(&dir, "1-1-0-build"), "").unwrap();
        assert!(matches!(take_slot(&dir, UPDATE, true, false), Ok(Took::Slot(_))), "a lock nobody holds is a build that ended");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_build_that_holds_its_slot_past_the_limit_while_others_wait_loses_it_and_hears_so() {
        let mut queue = queue(1, 100);
        queue.take("a@app", "1", false, true, 100);
        assert!(queue.tick(1, &[], &[], |_| true, 100 + MAX_HOLD).is_empty(), "nobody waits, so the slot stays");
        queue.take("b@app", "2", false, true, 100 + MAX_HOLD);
        let told = queue.tick(1, &[], &[], |_| true, 101 + MAX_HOLD);
        assert_eq!(told, vec![("a@app".to_string(), OUTHELD.to_string()), ("b@app".to_string(), TURN.to_string())]);
        assert!(queue.holders.is_empty());
    }

    #[test]
    fn a_task_on_a_low_disk_keeps_its_place_while_the_turn_goes_to_one_on_a_disk_with_room() {
        let mut queue = queue(1, 100);
        queue.take("a@app", "1", false, true, 100);
        queue.take("b@app", "2", false, true, 100);
        queue.take("c@app", "3", false, true, 100);
        queue.low_disk = strings(&["b@app"]);
        assert_eq!(queue.tick(1, &[], &[], |id| id != "1", 101), vec![("c@app".to_string(), TURN.to_string())], "nobody is told a turn the guard would refuse");
        assert_eq!(queue.line, strings(&["b@app"]));
        queue.turns.clear();
        assert!(queue.tick(1, &[], &[], |_| true, 102).is_empty(), "a line of only low disks hands out nothing");
        queue.low_disk.clear();
        assert_eq!(queue.tick(1, &[], &[], |_| true, 103), vec![("b@app".to_string(), TURN.to_string())]);
    }

    #[test]
    fn a_build_below_the_disk_limit_is_refused_with_the_free_space_and_the_key() {
        use crate::disk::{Limit, Space};
        let low = Space { free: 8_200_000_000, total: 242_000_000_000 };
        let refusal = disk_refusal(Some(low), Some(Limit::Gb(10))).unwrap();
        assert!(refusal.contains("8.2 GB free, below the 10 GB") && refusal.contains("disk.builds_below"), "{refusal}");
        assert_eq!(disk_refusal(Some(low), Some(Limit::Gb(8))), None);
        assert_eq!(disk_refusal(None, Some(Limit::Gb(10))), None, "a disk that cannot be read never stops a build");
        assert_eq!(disk_refusal(Some(low), None), None, "without a keeper's limit the build goes ahead");
    }

    #[test]
    fn cargo_run_takes_no_slot_as_what_it_runs_may_never_end() {
        let commands = strings(&["cargo"]);
        assert!(!builds("cargo", &strings(&["run"]), &commands) && !builds("cargo", &strings(&["r", "--release"]), &commands));
    }

    #[test]
    fn kebbie_update_waits_for_a_held_slot_only_so_long() {
        let dir = crate::sh::test_dir("builds-update-wait");
        crate::state::write(&dir.join(QUEUE), serde_json::to_string(&queue(1, crate::messages::now())).unwrap()).unwrap();
        let Ok(Took::Slot(_held)) = take_slot(&dir, "a@app", false, true) else { panic!("a free slot is taken") };
        let (sent, got) = std::sync::mpsc::channel();
        let inside = dir.clone();
        std::thread::spawn(move || {
            let _ = sent.send(wait_in(&inside, Duration::from_secs(1), Duration::from_millis(100)).is_none());
        });
        assert_eq!(got.recv_timeout(Duration::from_secs(10)), Ok(true));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_dropped_task_leaves_the_line_and_its_turn() {
        let mut queue = queue(1, 100);
        queue.take("a@app", "1", false, true, 100);
        queue.take("b@app", "2", false, true, 100);
        queue.forget("b@app");
        assert!(queue.line.is_empty());
    }

    #[test]
    fn only_cargos_compiling_commands_and_the_configured_words_take_a_slot() {
        let commands = strings(&["cargo", "npm run build", "xcodebuild"]);
        let builds = |program: &str, args: &[&str]| builds(program, &strings(args), &commands);
        assert!(builds("cargo", &["build", "--release"]) && builds("cargo", &["+1.90", "check"]) && builds("cargo", &["--locked", "-C", "sub", "test", "-q"]) && builds("cargo", &["clippy"]));
        assert!(!builds("cargo", &["fmt"]) && !builds("cargo", &["--version"]) && !builds("cargo", &["metadata"]));
        assert!(builds("npm", &["run", "build"]) && !builds("npm", &["test"]) && builds("xcodebuild", &["-scheme", "App"]));
        assert!(!builds("make", &[]));
    }

    #[test]
    fn sccache_waits_for_a_wrapper_set_in_the_env_or_any_cargo_config() {
        let root = std::env::temp_dir().join(format!("kebbie-builds-wrapper-{}", std::process::id()));
        let work = root.join("app/crate");
        let home = root.join("cargo-home");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&home).unwrap();
        let none = |_: &str| None;
        assert_eq!(configured_wrapper(none, &work, &home), None);
        assert_eq!(configured_wrapper(|name| (name == "RUSTC_WRAPPER").then(String::new), &work, &home).as_deref(), Some(""), "an empty RUSTC_WRAPPER is the user's choice too");
        fs::write(home.join("config.toml"), "[build]\nrustc-wrapper = \"/usr/bin/sccache\"\n").unwrap();
        assert_eq!(configured_wrapper(none, &work, &home).as_deref(), Some("/usr/bin/sccache"));
        fs::remove_file(home.join("config.toml")).unwrap();
        fs::create_dir_all(root.join("app/.cargo")).unwrap();
        fs::write(root.join("app/.cargo/config.toml"), "[build]\nrustc-wrapper = \"\"\n").unwrap();
        assert_eq!(configured_wrapper(none, &work, &home).as_deref(), Some(""), "the nearest config wins");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_wrapper_the_user_set_to_sccache_gets_its_server_kept_and_the_wrapper_left_as_it_is() {
        let root = std::env::temp_dir().join(format!("kebbie-builds-users-sccache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let on_path = root.join("sccache");
        crate::executable::write(&on_path, "#!/bin/sh\n").unwrap();
        let path = [root.clone()];
        assert_eq!(sccache_for(None, &path), Some((on_path.clone(), true)), "no wrapper set: the guard sets sccache");
        assert_eq!(sccache_for(Some("/home/u/.cargo/bin/sccache"), &path), Some((PathBuf::from("/home/u/.cargo/bin/sccache"), false)), "the user's own sccache: its server is kept, the wrapper is the user's");
        assert_eq!(sccache_for(Some("sccache"), &path), Some((on_path, false)), "a bare name is found on PATH as cargo finds it");
        assert_eq!(sccache_for(Some("/usr/bin/ccache"), &path), None);
        assert_eq!(sccache_for(Some(""), &path), None, "an empty wrapper turns wrapping off");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_sccache_server_starts_under_the_stable_tmpdir_and_one_under_a_tasks_temp_folder_is_restarted() {
        let root = std::env::temp_dir().join(format!("kebbie-builds-sccache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let log = root.join("calls");
        let sccache = root.join("sccache");
        crate::executable::write(&sccache, format!("#!/bin/sh\necho \"$1 $TMPDIR\" >> {}\n", crate::sh::quote(&log.to_string_lossy()))).unwrap();
        let stable = root.join("cache/kebbie/sccache-tmp");
        let task_root = fs::canonicalize(&root).unwrap().join("kebbie-1001");
        let calls = || fs::read_to_string(&log).unwrap_or_default();

        prepare_server(&sccache, || Some(vec![]), &stable, &task_root);
        assert_eq!(calls(), format!("--start-server {}\n", stable.display()), "no server runs: kebbie starts one under the stable folder");
        assert!(stable.is_dir());

        fs::remove_file(&log).unwrap();
        prepare_server(&sccache, || Some(vec![stable.clone()]), &stable, &task_root);
        assert_eq!(calls(), "", "a server under kebbie's own folder is left running");

        let unowned = root.join("tmp");
        fs::create_dir_all(&unowned).unwrap();
        prepare_server(&sccache, || Some(vec![unowned.clone()]), &stable, &task_root);
        assert_eq!(calls(), "", "a server under /tmp or another folder no task owns is left running");

        let live_task_tmp = task_root.join("wt1.kebbie-abc123");
        fs::create_dir_all(&live_task_tmp).unwrap();
        prepare_server(&sccache, || Some(vec![live_task_tmp.clone()]), &stable, &task_root);
        assert_eq!(calls(), format!("--stop-server {0}\n--start-server {0}\n", stable.display()), "a server under a task's temp folder that still exists is restarted before that task ends and takes it along");

        fs::remove_file(&log).unwrap();
        let gone = root.join("task-tmp-that-was-cleaned");
        prepare_server(&sccache, || Some(vec![gone.clone()]), &stable, &task_root);
        assert_eq!(calls(), format!("--stop-server {0}\n--start-server {0}\n", stable.display()), "a server whose TMPDIR is gone is stopped and started again");

        fs::remove_file(&log).unwrap();
        prepare_server(&sccache, || None, &stable, &task_root);
        assert_eq!(calls(), format!("--start-server {}\n", stable.display()), "where no process can be read, a start is tried");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_starting_at_once_restart_a_stale_server_once_and_never_stop_the_new_one() {
        let root = std::env::temp_dir().join(format!("kebbie-builds-race-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let log = root.join("calls");
        let started = root.join("started");
        let sccache = root.join("sccache");
        let (log_q, started_q) = (crate::sh::quote(&log.to_string_lossy()), crate::sh::quote(&started.to_string_lossy()));
        crate::executable::write(&sccache, format!("#!/bin/sh\necho \"$1\" >> {log_q}\nsleep 0.2\n[ \"$1\" = --start-server ] && touch {started_q}\nexit 0\n")).unwrap();
        let stable = root.join("cache/kebbie/sccache-tmp");
        let gone = root.join("cleaned-task-tmp");
        let running = || Some(vec![if started.exists() { stable.clone() } else { gone.clone() }]);
        std::thread::scope(|scope| {
            for _ in 0..3 {
                scope.spawn(|| prepare_server(&sccache, running, &stable, &root.join("kebbie-1001")));
            }
        });
        assert_eq!(fs::read_to_string(&log).unwrap(), "--stop-server\n--start-server\n", "the builds after the first find the server it started and leave it running");
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_running_sccache_server_is_found_at_its_own_address_with_the_tmpdir_it_started_with() {
        let root = std::env::temp_dir().join(format!("kebbie-builds-server-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let server = root.join("sccache");
        crate::executable::write(&server, "#!/bin/sh\nread line\n").unwrap();
        let (stable, gone) = (root.join("cache/kebbie/sccache-tmp"), root.join("cleaned-task-tmp"));
        let spawn = |port: &str, tmpdir: &Path| Command::new(&server).stdin(std::process::Stdio::piped()).env("SCCACHE_START_SERVER", "1").env("SCCACHE_SERVER_PORT", port).env("TMPDIR", tmpdir).spawn().unwrap();
        let mut servers = [spawn("4998", &gone), spawn("4999", &gone)];
        let ours = server_address(|name| (name == "SCCACHE_SERVER_PORT").then(|| "4998".to_string()));
        let seen = |want: &Path| (0..50).any(|_| running_servers(&ours).unwrap().iter().any(|dir| dir == want) || { std::thread::sleep(Duration::from_millis(20)); false });
        assert!(seen(&gone), "the server's TMPDIR is read from its environment");
        assert_eq!(running_servers(&ours).unwrap(), vec![gone.clone()], "the server on another port is not this address's");
        let _ = servers[0].kill();
        let _ = servers[0].wait();
        servers[0] = spawn("4998", &stable);
        assert!(seen(&stable));

        let log = root.join("calls");
        let sccache = root.join("recording-sccache");
        crate::executable::write(&sccache, format!("#!/bin/sh\necho \"$1\" >> {}\n", crate::sh::quote(&log.to_string_lossy()))).unwrap();
        prepare_server(&sccache, || running_servers(&ours), &stable, &root.join("kebbie-1001"));
        prepare_server(&sccache, || running_servers(&ours), &stable, &root.join("kebbie-1001"));
        for mut server in servers {
            let _ = server.kill();
            let _ = server.wait();
        }
        assert!(!log.exists(), "a server under a gone TMPDIR on another port, which --stop-server cannot reach, leaves this address's healthy server running build after build");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_server_address_is_its_socket_else_its_port_else_sccaches_default() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| move |name: &str| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string());
        assert_eq!(server_address(vars(&[])), "port:4226");
        assert_eq!(server_address(vars(&[("SCCACHE_SERVER_PORT", "4300")])), "port:4300");
        assert_eq!(server_address(vars(&[("SCCACHE_SERVER_PORT", "4300"), ("SCCACHE_SERVER_UDS", "/run/s.sock")])), "uds:/run/s.sock");
    }

    #[test]
    fn a_server_reads_its_tmpdir_from_its_environment_and_falls_back_to_tmp() {
        assert_eq!(environ_tmpdir(b"HOME=/h\0TMPDIR=/tmp/kebbie-1001/wt1\0SCCACHE_START_SERVER=1\0"), PathBuf::from("/tmp/kebbie-1001/wt1"));
        assert_eq!(environ_tmpdir(b"HOME=/h\0SCCACHE_START_SERVER=1\0"), PathBuf::from("/tmp"));
        assert_eq!(environ_tmpdir(b"TMPDIR=\0"), PathBuf::from("/tmp"));
    }

    #[test]
    fn the_guard_scripts_follow_the_config_and_the_env_line_puts_them_first_once() {
        let dir = std::env::temp_dir().join(format!("kebbie-builds-scripts-{}", std::process::id()));
        write_scripts(&dir, "/opt/kebbie", &strings(&["cargo", "npm run build", "npm run test"])).unwrap();
        assert_eq!(fs::read_to_string(dir.join("cargo")).unwrap(), "#!/bin/sh\nexec '/opt/kebbie' build-guard cargo \"$@\"\n");
        assert!(dir.join("npm").is_file() && is_guard(&dir.join("npm")));
        write_scripts(&dir, "/opt/kebbie", &strings(&["cargo"])).unwrap();
        assert!(!dir.join("npm").exists(), "a program the config dropped loses its guard");
        assert_eq!(real_program("cargo", std::slice::from_ref(&dir)), None, "the guard never runs itself as the real program");
        assert_eq!(env_line(Path::new("/s/build-guard")), "case \":$PATH:\" in *:'/s/build-guard':*) ;; *) export PATH='/s/build-guard':\"$PATH\" ;; esac\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_queue_a_task_wrote_names_no_file_outside_its_folder_and_no_address_but_a_tasks() {
        let root = crate::sh::test_dir("builds-untrusted");
        let dir = root.join("builds");
        let victim = root.join("victim/Cargo.lock");
        fs::create_dir_all(dir.join("held-")).unwrap();
        fs::create_dir_all(victim.parent().unwrap()).unwrap();
        fs::write(&victim, "kept").unwrap();
        let written = Queue { holders: vec![Holder { address: "a@app".into(), id: "/../../victim/Cargo".into(), at: 1 }], line: strings(&["lead", "../x@app", "b@app"]), ..queue(1, 1) };
        crate::state::write(&dir.join(QUEUE), serde_json::to_string(&written).unwrap()).unwrap();
        let told = with_queue(&dir, |queue| queue.tick(1, &[], &[], |id| alive_in(&dir, id), 10)).unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "kept", "a holder id is never a path");
        assert_eq!(told, vec![("b@app".to_string(), TURN.to_string())], "only a task's address is told");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_queue_lock_held_elsewhere_makes_the_queue_unusable_soon_instead_of_freezing_its_caller() {
        let dir = crate::sh::test_dir("builds-held-lock");
        let _held = crate::state::Locked::wait(&dir.join("queue.lock")).unwrap();
        let (sent, got) = std::sync::mpsc::channel();
        let inside = dir.clone();
        std::thread::spawn(move || {
            let _ = sent.send(with_queue(&inside, |queue| queue.forget("a@app")).is_err());
        });
        assert_eq!(got.recv_timeout(Duration::from_secs(10)), Ok(true));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_held_slot_is_freed_by_its_drop_and_its_lock_says_whether_its_build_still_runs() {
        let dir = crate::lead::state_location().unwrap().join(format!("builds-slot-{}", std::process::id()));
        crate::state::private_dir(&dir).unwrap();
        crate::state::write(&dir.join(QUEUE), serde_json::to_string(&queue(1, crate::messages::now())).unwrap()).unwrap();
        let Ok(Took::Slot(slot)) = take_slot(&dir, "a@app", false, true) else { panic!("a free slot is taken") };
        let id = load(&dir).holders[0].id.clone();
        assert!(alive_in(&dir, &id), "a running build's lock is held");
        assert!(matches!(take_slot(&dir, "b@app", false, true), Ok(Took::Queued(1, _))));
        drop(slot);
        assert!(load(&dir).holders.is_empty() && !alive_in(&dir, &id));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_waiting_line_names_a_task_that_holds_only_a_turn() {
        let dir = crate::lead::state_location().unwrap().join(format!("builds-turn-line-{}", std::process::id()));
        crate::state::private_dir(&dir).unwrap();
        let now = crate::messages::now();
        let turned = Queue { turns: vec![Turn { address: "alpha@app".into(), until: now + HOLD, told: true }], ..queue(1, now) };
        crate::state::write(&dir.join(QUEUE), serde_json::to_string(&turned).unwrap()).unwrap();
        let Ok(Took::Queued(place, held)) = take_slot(&dir, "b@app", false, false) else { panic!("a slot held by a turn queues the next build") };
        assert_eq!(queued_line(place, &held), "kebbie: no build slot is free for this task (alpha@app has its turn); it is 1st in line, and kebbie tells it when its turn comes.");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_build_queued_behind_a_task_ahead_while_a_slot_is_free_names_that_task() {
        let dir = crate::lead::state_location().unwrap().join(format!("builds-ahead-line-{}", std::process::id()));
        crate::state::private_dir(&dir).unwrap();
        let ahead = Queue { line: strings(&["alpha@app"]), ..queue(1, crate::messages::now()) };
        crate::state::write(&dir.join(QUEUE), serde_json::to_string(&ahead).unwrap()).unwrap();
        let Ok(Took::Queued(place, held)) = take_slot(&dir, "b@app", false, false) else { panic!("a task ahead in line keeps the free slot from the next build") };
        assert_eq!(queued_line(place, &held), "kebbie: a build slot is free, but it goes first to the tasks ahead of this one (alpha@app); it is 2nd in line, and kebbie tells it when its turn comes.");
        assert_eq!(held.who(), "alpha@app ahead in line");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn kebbie_update_put_first_in_line_while_a_slot_is_free_hears_it_comes_next() {
        let dir = crate::lead::state_location().unwrap().join(format!("builds-update-next-{}", std::process::id()));
        crate::state::private_dir(&dir).unwrap();
        let ahead = Queue { line: strings(&["alpha@app"]), ..queue(1, crate::messages::now()) };
        crate::state::write(&dir.join(QUEUE), serde_json::to_string(&ahead).unwrap()).unwrap();
        let Ok(Took::Queued(place, held)) = take_slot(&dir, UPDATE, true, false) else { panic!("kebbie update waits for the keeper to hand it the free slot") };
        assert_eq!((place, held.clone()), (1, Blocked::Next));
        assert_eq!(held.who(), "one is free and kebbie hands it here next");
        assert_eq!(queued_line(place, &held), "kebbie: a build slot is free and kebbie hands it to this task next; it is 1st in line, and kebbie tells it when its turn comes.");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_waiting_line_names_builds_before_turns() {
        let holders = [Holder { address: "a@app".into(), id: "1".into(), at: 1 }, Holder { address: "b@app".into(), id: "2".into(), at: 1 }];
        let turns = [Turn { address: "c@app".into(), until: 1, told: true }];
        assert_eq!(holding(&holders, &turns), "a@app, b@app building; c@app has its turn");
        assert_eq!(holding(&holders[..1], &[]), "a@app building");
    }
}
