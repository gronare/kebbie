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
const KEEPER_QUIET: u64 = 60;
const QUEUED_EXIT: u8 = 75;
const HELD_VAR: &str = "KEBBIE_BUILD_SLOT";
const GUARD_DIR: &str = "build-guard";
const QUEUE: &str = "queue.json";
const CARGO_BUILDS: [&str; 18] = ["build", "b", "check", "c", "test", "t", "run", "r", "clippy", "bench", "doc", "d", "install", "rustc", "rustdoc", "fix", "nextest", "llvm-cov"];
const CARGO_VALUED: [&str; 5] = ["-C", "--config", "-Z", "--color", "--manifest-path"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queue {
    pub slots: Option<u32>,
    pub commands: Option<Vec<String>>,
    pub holders: Vec<Holder>,
    pub line: Vec<String>,
    pub turns: Vec<Turn>,
    pub ticked: u64,
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

impl Queue {
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

    // The keeper's round: dead builds free their slots, a turn not taken in time passes on, and each free slot goes to the next in line; what to tell whom comes back.
    pub fn tick(&mut self, slots: u32, commands: &[String], alive: impl Fn(&str) -> bool, now: u64) -> Vec<(String, String)> {
        self.slots = Some(slots);
        self.commands = Some(commands.to_vec());
        self.ticked = now;
        self.holders.retain(|holder| alive(&holder.id));
        let mut told = Vec::new();
        let (expired, kept): (Vec<Turn>, Vec<Turn>) = std::mem::take(&mut self.turns).into_iter().partition(|turn| turn.until <= now);
        self.turns = kept;
        told.extend(expired.into_iter().filter(|turn| turn.told).map(|turn| (turn.address, MISSED.to_string())));
        while self.used() < self.slots() && !self.line.is_empty() {
            let address = self.line.remove(0);
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
pub const MISSED: &str = "kebbie: your build turn passed unused to the next in line; run the build again to queue.";

pub fn queued_line(place: usize, holders: &[Holder]) -> String {
    let building: Vec<&str> = holders.iter().map(|holder| holder.address.as_str()).collect();
    format!("kebbie: every build slot is taken ({}); this task is {} in line, and kebbie tells it when its turn comes.", building.join(", "), crate::tasks::ordinal(place))
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

pub fn load(dir: &Path) -> Queue {
    fs::read_to_string(dir.join(QUEUE)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn with_queue<T>(dir: &Path, change: impl FnOnce(&mut Queue) -> T) -> Result<T> {
    crate::state::private_dir(dir)?;
    let _lock = crate::state::Locked::wait(&dir.join("queue.lock"))?;
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
    let file = held_file(dir, id);
    let Ok(open) = File::open(&file) else { return false };
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
        let _ = with_queue(&self.dir, |queue| queue.release(&self.id, crate::messages::now()));
        let _ = fs::remove_file(held_file(&self.dir, &self.id));
    }
}

pub enum Took {
    Slot(Slot),
    Queued(usize, Vec<Holder>),
    NoKeeper,
}

pub fn take_slot(dir: &Path, address: &str, front: bool, needs_keeper: bool) -> Result<Took> {
    crate::state::private_dir(dir)?;
    let id = crate::messages::new_id("build");
    let path = held_file(dir, &id);
    let file = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !try_flock(&file) {
        return Err(format!("{}: could not be locked", path.display()));
    }
    let (took, holders) = with_queue(dir, |queue| (queue.take(address, &id, front, needs_keeper, crate::messages::now()), queue.holders.clone()))?;
    match took {
        Take::Taken => Ok(Took::Slot(Slot { _file: file, dir: dir.to_path_buf(), id })),
        Take::Queued(place) => {
            let _ = fs::remove_file(&path);
            Ok(Took::Queued(place, holders))
        }
        Take::NoKeeper => {
            let _ = fs::remove_file(&path);
            Ok(Took::NoKeeper)
        }
    }
}

// kebbie update goes first in line and waits there, since the user is watching it; a queue it cannot use lets it build at once.
pub fn wait_for_update() -> Option<Slot> {
    let dir = dir().ok()?;
    let mut said = false;
    loop {
        match take_slot(&dir, UPDATE, true, false) {
            Ok(Took::Slot(slot)) => return Some(slot),
            Ok(Took::Queued(_, holders)) => {
                if !said {
                    let building: Vec<&str> = holders.iter().map(|holder| holder.address.as_str()).collect();
                    eprintln!("waiting for a build slot: {} builds now", building.join(", "));
                    said = true;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
            Ok(Took::NoKeeper) | Err(_) => return None,
        }
    }
}

pub fn tick_now() -> Result<Vec<String>> {
    let dir = dir()?;
    let (slots, commands) = crate::config::build_settings().unwrap_or_else(|_| (crate::config::DEFAULT_BUILD_SLOTS, crate::config::DEFAULT_BUILD_COMMANDS.map(str::to_string).to_vec()));
    let told = with_queue(&dir, |queue| queue.tick(slots, &commands, |id| alive_in(&dir, id), crate::messages::now()))?;
    Ok(told.into_iter().map(|(address, text)| match crate::messages::hand_to(&address, &text) {
        Ok(()) => format!("builds: told {address}: {text}"),
        Err(error) => format!("builds: could not tell {address}: {error}"),
    }).collect())
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
fn wrapper_set(env: impl Fn(&str) -> Option<String>, cwd: &Path, cargo_home: &Path) -> bool {
    if env("RUSTC_WRAPPER").is_some() || env("CARGO_BUILD_RUSTC_WRAPPER").is_some() {
        return true;
    }
    let configs = cwd.ancestors().map(|dir| dir.join(".cargo")).chain([cargo_home.to_path_buf()]);
    configs.flat_map(|dir| [dir.join("config.toml"), dir.join("config")]).any(|file| {
        fs::read_to_string(file).ok().and_then(|text| text.parse::<toml::Table>().ok()).is_some_and(|config| config.get("build").and_then(|build| build.get("rustc-wrapper")).is_some())
    })
}

fn cache_wrapper(path: &[PathBuf]) -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".cargo"));
    let cwd = std::env::current_dir().ok()?;
    if wrapper_set(|name| std::env::var(name).ok(), &cwd, &cargo_home) {
        return None;
    }
    real_program("sccache", path)
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
    if program == "cargo" && let Some(sccache) = cache_wrapper(&path) {
        command.env("RUSTC_WRAPPER", sccache);
    }
    let queue = dir();
    let gated = std::env::var_os(HELD_VAR).is_none() && queue.as_ref().is_ok_and(|dir| builds(program, args, &load(dir).commands()));
    let address = if gated { task_address() } else { None };
    let (Ok(dir), Some(address)) = (queue, address) else { return exit_of(command.status(), program) };
    let slot = match take_slot(&dir, &address, false, true) {
        Ok(Took::Slot(slot)) => slot,
        Ok(Took::Queued(place, holders)) => {
            eprintln!("{}", queued_line(place, &holders));
            return ExitCode::from(QUEUED_EXIT);
        }
        Ok(Took::NoKeeper) => {
            eprintln!("kebbie: every build slot is taken and no keeper runs to hand out turns; building without waiting.");
            return exit_of(command.status(), program);
        }
        Err(error) => {
            eprintln!("kebbie: the build queue could not be used ({error}); building without waiting.");
            return exit_of(command.status(), program);
        }
    };
    outlive_interrupts();
    let status = command.env(HELD_VAR, &address).status();
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
    fn the_keeper_hands_a_freed_slot_to_the_next_in_line_and_holds_it_for_it() {
        let mut queue = queue(1, 100);
        queue.take("a@app", "1", false, true, 100);
        queue.take("b@app", "2", false, true, 100);
        queue.release("1", 110);
        assert_eq!(queue.take("c@app", "3", false, true, 111), Take::Queued(2), "the slot a finished build keeps a moment is not free for another");
        assert!(queue.tick(1, &[], |_| true, 115).is_empty(), "the finished build's moment is not over");
        let told = queue.tick(1, &[], |_| true, 121);
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
        queue.tick(1, &[], |id| id != "1", 101);
        let told = queue.tick(1, &[], |_| true, 101 + HOLD);
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
        died.tick(1, &[], |_| false, 107);
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
        assert!(!wrapper_set(none, &work, &home));
        assert!(wrapper_set(|name| (name == "RUSTC_WRAPPER").then(String::new), &work, &home), "an empty RUSTC_WRAPPER is the user's choice too");
        fs::write(home.join("config.toml"), "[build]\nrustc-wrapper = \"/usr/bin/sccache\"\n").unwrap();
        assert!(wrapper_set(none, &work, &home));
        fs::remove_file(home.join("config.toml")).unwrap();
        fs::create_dir_all(root.join("app/.cargo")).unwrap();
        fs::write(root.join("app/.cargo/config.toml"), "[build]\nrustc-wrapper = \"\"\n").unwrap();
        assert!(wrapper_set(none, &work, &home));
        let _ = fs::remove_dir_all(root);
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
}
