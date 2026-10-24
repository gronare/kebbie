use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;


pub const FILE: &str = "update-progress.json";
pub const DIR: &str = "update-progress";
pub const ENV: &str = "KELPIE_UPDATE_PROGRESS";
pub const BUILD_ENV: &str = "KELPIE_PROGRESS_DIR";

pub const FETCH: usize = 0;
pub const BUILD: usize = 1;
pub const PLUGINS: usize = 2;
pub const RESTART: usize = 3;

const BAR: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Pending,
    Current,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub label: String,
    pub state: State,
    #[serde(default)]
    pub since: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Meter {
    Build { done: u64, total: Option<u64>, compiling: Option<String> },
    Download { what: String, got: u64, total: Option<u64> },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub version: String,
    pub new: Vec<String>,
    pub fixed: Vec<String>,
    pub restart: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub started: u64,
    pub steps: Vec<Step>,
    pub meter: Option<Meter>,
    pub summary: Option<Summary>,
    #[serde(default)]
    pub finished: bool,
}

pub fn file(state: &Path) -> PathBuf {
    state.join(FILE)
}

pub fn load(path: &Path) -> Option<Progress> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub struct Tracker {
    pub path: PathBuf,
    pub progress: Progress,
}

impl Tracker {
    pub fn start(path: &Path, fetch: &str, build: &str, now: u64) -> Tracker {
        let step = |label: &str| Step { label: label.to_string(), state: State::Pending, since: 0 };
        let steps = vec![step(fetch), step(build), step("Claude Code and vault plugins"), step("Restart sessions")];
        let mut tracker = Tracker { path: path.to_path_buf(), progress: Progress { started: now, steps, ..Progress::default() } };
        tracker.begin(FETCH, fetch, now);
        tracker
    }

    pub fn resume(path: &Path) -> Option<Tracker> {
        load(path).map(|progress| Tracker { path: path.to_path_buf(), progress })
    }

    pub fn save(&self) {
        if let Ok(text) = serde_json::to_string(&self.progress) {
            let _ = crate::state::write(&self.path, text);
        }
    }

    fn set(&mut self, index: usize, label: &str, state: State, now: u64) {
        if let Some(step) = self.progress.steps.get_mut(index) {
            step.label = label.to_string();
            if state == State::Current {
                step.since = now;
            }
            step.state = state;
        }
        if state != State::Current {
            self.progress.meter = None;
        }
        self.save();
    }

    pub fn begin(&mut self, index: usize, label: &str, now: u64) {
        self.set(index, label, State::Current, now);
    }

    pub fn done(&mut self, index: usize, label: &str) {
        self.set(index, label, State::Done, 0);
    }

    pub fn fail(&mut self, index: usize) {
        let label = self.progress.steps.get(index).map(|step| step.label.clone()).unwrap_or_default();
        self.set(index, &label, State::Failed, 0);
    }

    pub fn fail_current(&mut self) {
        if let Some(index) = self.progress.steps.iter().position(|step| step.state == State::Current) {
            self.fail(index);
        }
    }

    pub fn state(&self, index: usize) -> Option<State> {
        self.progress.steps.get(index).map(|step| step.state)
    }

    pub fn meter(&mut self, meter: Meter) {
        if self.progress.meter.as_ref() != Some(&meter) {
            self.progress.meter = Some(meter);
            self.save();
        }
    }

    pub fn finish(&mut self, summary: Summary) {
        self.progress.summary = Some(summary);
        self.progress.finished = true;
        self.progress.meter = None;
        self.save();
    }
}

pub fn elapsed(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

pub fn bar(done: u64, total: u64, width: usize) -> String {
    let filled = if total == 0 { 0 } else { ((done.min(total) as u128 * width as u128) / total as u128) as usize };
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_000_000.0)
}

pub fn mark(state: State) -> &'static str {
    match state {
        State::Done => "✓",
        State::Current => "▸",
        State::Failed => "✗",
        State::Pending => " ",
    }
}

pub fn meter_text(meter: Option<&Meter>, width: usize) -> String {
    let (done, total, unit) = match meter {
        Some(Meter::Build { done, total: Some(total), .. }) => (*done, *total, format!("{done}/{total} crates")),
        Some(Meter::Build { compiling: Some(name), .. }) => return format!("compiling {name}"),
        Some(Meter::Download { got, total: Some(total), .. }) => (*got, *total, format!("{}/{} MB", megabytes(*got), megabytes(*total))),
        Some(Meter::Download { got, .. }) => return format!("{} MB", megabytes(*got)),
        _ => return String::new(),
    };
    let room = width.saturating_sub(unit.chars().count() + 2);
    if room >= 6 { format!("{}  {unit}", bar(done, total, room.min(BAR))) } else { unit }
}

pub fn step_line(step: &Step, meter: Option<&Meter>, now: u64, width: usize) -> String {
    let head = format!("{} {}", mark(step.state), step.label);
    if step.state != State::Current {
        return head;
    }
    let clock = elapsed(now.saturating_sub(step.since));
    let room = width.saturating_sub(head.chars().count() + clock.chars().count() + 3);
    let middle = meter_text(meter, room);
    if middle.is_empty() { format!("{head}  {clock}") } else { format!("{head}  {middle} {clock}") }
}

pub fn lines(progress: &Progress, now: u64, width: usize) -> Vec<(State, String)> {
    progress.steps.iter().map(|step| (step.state, step_line(step, progress.meter.as_ref(), now, width))).collect()
}

pub fn artifacts(jsonl: &str) -> u64 {
    jsonl.lines().filter(|line| line.contains("\"reason\":\"compiler-artifact\"")).count() as u64
}

pub fn compiling(log: &str) -> Option<String> {
    log.lines().rev().find_map(|line| line.trim_start().strip_prefix("Compiling ")).and_then(|rest| rest.split_whitespace().next()).map(str::to_string)
}

// The units cargo reports as artifacts: one per package the root reaches through normal and build edges, one more for each build script, and one per target of the root.
pub fn units(metadata: &Value) -> Option<u64> {
    let root = metadata["resolve"]["root"].as_str()?;
    let nodes: HashMap<&str, &Value> = metadata["resolve"]["nodes"].as_array()?.iter().filter_map(|node| Some((node["id"].as_str()?, node))).collect();
    let packages: HashMap<&str, &Value> = metadata["packages"].as_array()?.iter().filter_map(|package| Some((package["id"].as_str()?, package))).collect();
    let mut seen = HashSet::from([root]);
    let mut queue = vec![root];
    while let Some(id) = queue.pop() {
        for dep in nodes.get(id)?["deps"].as_array().into_iter().flatten() {
            let wanted = dep["dep_kinds"].as_array().into_iter().flatten().any(|kind| kind["kind"].as_str() != Some("dev"));
            if let Some(next) = dep["pkg"].as_str().filter(|_| wanted)
                && seen.insert(next)
            {
                queue.push(next);
            }
        }
    }
    let kinds = |id: &str| -> Vec<String> { packages.get(id).and_then(|package| package["targets"].as_array()).into_iter().flatten().flat_map(|target| target["kind"].as_array().cloned().unwrap_or_default()).filter_map(|kind| kind.as_str().map(str::to_string)).collect() };
    Some(seen.iter().map(|id| {
        let kinds = kinds(id);
        let script = u64::from(kinds.iter().any(|kind| kind == "custom-build"));
        if *id == root { kinds.iter().filter(|kind| ["lib", "bin", "proc-macro"].contains(&kind.as_str())).count() as u64 + script } else { 1 + script }
    }).sum())
}

fn metadata_units(root: &Path) -> Option<u64> {
    let mut args = vec!["metadata", "--format-version", "1", "--offline"];
    let target = crate::release::target_triple(std::env::consts::OS, std::env::consts::ARCH);
    if let Some(target) = target {
        args.extend(["--filter-platform", target]);
    }
    let text = crate::sh::run("cargo", &args, Some(root)).ok()?;
    units(&serde_json::from_str(&text).ok()?)
}

pub fn download(text: &str) -> Option<(String, Option<u64>, PathBuf)> {
    let mut words = text.split_whitespace();
    let tag = words.next()?.to_string();
    let total = words.next()?.parse().ok().filter(|total: &u64| *total > 0);
    Some((tag, total, PathBuf::from(words.collect::<Vec<_>>().join(" "))))
}

pub fn prepare_dir(state: &Path) -> PathBuf {
    let dir = state.join(DIR);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = crate::state::private_dir(&dir);
    dir
}

pub struct Watch {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<Tracker>,
}

impl Watch {
    pub fn stop(self) -> Tracker {
        self.stop.store(true, Ordering::SeqCst);
        self.handle.join().expect("the progress watcher does not panic")
    }
}

// herdr's update fetches and builds in one command, so the fetch ends when the build script first writes into `dir`.
pub fn watch(mut tracker: Tracker, dir: PathBuf, root: PathBuf, build_label: String, now: fn() -> u64) -> Watch {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let handle = std::thread::spawn(move || {
        let mut total = None;
        let mut tries = 0u32;
        while !stopped.load(Ordering::SeqCst) {
            let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
            let started = |tracker: &mut Tracker, label: &str| {
                if tracker.state(BUILD) == Some(State::Pending) {
                    tracker.done(FETCH, &tracker.progress.steps[FETCH].label.clone());
                    tracker.begin(BUILD, label, now());
                }
            };
            if let Some((tag, size, path)) = read("download").as_deref().and_then(download) {
                started(&mut tracker, &format!("Downloading kelpie {}", tag.trim_start_matches('v')));
                let got = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                tracker.meter(Meter::Download { what: tag, got, total: size });
            } else if let Some(jsonl) = read("cargo.jsonl") {
                started(&mut tracker, &build_label);
                if total.is_none() && tries.is_multiple_of(20) {
                    total = metadata_units(&root);
                }
                tries += 1;
                let done = artifacts(&jsonl);
                tracker.meter(Meter::Build { done, total: total.map(|total| total.max(done)), compiling: read("cargo.log").as_deref().and_then(compiling) });
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        tracker
    });
    Watch { stop, handle }
}

pub struct Printer {
    tty: bool,
    shown: Vec<State>,
    open: bool,
}

impl Printer {
    pub fn new(tty: bool) -> Printer {
        Printer { tty, shown: Vec::new(), open: false }
    }

    pub fn frame(&mut self, progress: &Progress, now: u64, width: usize) -> String {
        let mut out = String::new();
        let lines = lines(progress, now, width);
        for (index, (state, text)) in lines.iter().enumerate() {
            let before = self.shown.get(index).copied().unwrap_or(State::Pending);
            let ended = matches!(state, State::Done | State::Failed);
            if self.tty && *state == State::Current {
                out.push_str(&format!("\r\x1b[K{text}"));
                self.open = true;
            } else if ended && before != *state {
                if self.tty && self.open {
                    out.push_str("\r\x1b[K");
                }
                out.push_str(&format!("{text}\n"));
                self.open = false;
            } else if !self.tty && *state == State::Current && before == State::Pending {
                out.push_str(&format!("{text}\n"));
            }
        }
        self.shown = lines.into_iter().map(|(state, _)| state).collect();
        out
    }

    pub fn close(&mut self) -> String {
        if std::mem::take(&mut self.open) { "\r\x1b[K".to_string() } else { String::new() }
    }
}

pub fn print_while(path: PathBuf, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use std::io::Write;
        let tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
        let width = ratatui::crossterm::terminal::size().map_or(80, |(width, _)| width as usize).max(40);
        let mut printer = Printer::new(tty);
        loop {
            let last = stop.load(Ordering::SeqCst);
            if let Some(progress) = load(&path) {
                print!("{}", printer.frame(&progress, crate::messages::now(), width.saturating_sub(1)));
            }
            if last {
                print!("{}", printer.close());
                let _ = std::io::stdout().flush();
                return;
            }
            let _ = std::io::stdout().flush();
            std::thread::sleep(Duration::from_millis(200));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn progress() -> Progress {
        let dir = crate::sh::test_dir("progress");
        let mut tracker = Tracker::start(&dir.join(FILE), "Fetching main", "Build kelpie", 100);
        tracker.done(FETCH, "Fetched main (3 new commits)");
        tracker.begin(BUILD, "Building kelpie", 100);
        tracker.meter(Meter::Build { done: 212, total: Some(348), compiling: Some("serde".to_string()) });
        load(&tracker.path).unwrap()
    }

    #[test]
    fn the_steps_show_done_current_and_pending_with_a_bar_and_the_time_on_the_running_one() {
        let shown: Vec<String> = lines(&progress(), 202, 60).into_iter().map(|(_, text)| text).collect();
        assert_eq!(shown, ["✓ Fetched main (3 new commits)", "▸ Building kelpie  ████████████░░░░░░░░  212/348 crates 1:42", "  Claude Code and vault plugins", "  Restart sessions"]);
        let narrow = lines(&progress(), 202, 40)[BUILD].1.clone();
        assert!(narrow.ends_with("212/348 crates 1:42") && narrow.chars().count() <= 40, "a narrow width shortens the bar first: {narrow}");
    }

    #[test]
    fn without_a_total_the_crate_compiling_shows_and_a_download_counts_megabytes() {
        let build = Meter::Build { done: 3, total: None, compiling: Some("tokio".to_string()) };
        assert_eq!(meter_text(Some(&build), 40), "compiling tokio");
        let download = Meter::Download { what: "v0.28.0".to_string(), got: 4_200_000, total: Some(9_800_000) };
        assert_eq!(meter_text(Some(&download), 40), "████████░░░░░░░░░░░░  4.2/9.8 MB");
        assert_eq!(meter_text(Some(&Meter::Download { what: "v0.28.0".to_string(), got: 4_200_000, total: None }), 40), "4.2 MB");
        assert_eq!(bar(0, 0, 4), "░░░░");
        assert_eq!(bar(9, 4, 4), "████", "a count past the estimate fills the bar, never more");
    }

    #[test]
    fn cargo_progress_is_counted_from_its_json_and_its_compiling_lines() {
        let jsonl = [r#"{"reason":"compiler-artifact","package_id":"a","fresh":true}"#, r#"{"reason":"build-script-executed","package_id":"b"}"#, r#"{"reason":"compiler-artifact","package_id":"c","fresh":false}"#, r#"{"reason":"build-finished","success":true}"#].join("\n");
        assert_eq!(artifacts(&jsonl), 2, "only finished units count, not build-script runs");
        assert_eq!(compiling("   Compiling serde v1.0.219\n   Compiling serde_json v1.0.140\nwarning: x\n"), Some("serde_json".to_string()));
        assert_eq!(compiling("kelpie build: building from source"), None);
        assert_eq!(download("v0.28.0 9800000 /tmp/x/kelpie.tar.gz"), Some(("v0.28.0".to_string(), Some(9_800_000), PathBuf::from("/tmp/x/kelpie.tar.gz"))));
        assert_eq!(download("v0.28.0 0 /tmp/x").map(|(_, total, _)| total), Some(None), "no content length, no total");
    }

    #[test]
    fn the_total_counts_what_the_root_reaches_without_dev_dependencies() {
        let package = |id: &str, kinds: &[&[&str]]| json!({ "id": id, "targets": kinds.iter().map(|kind| json!({ "kind": kind })).collect::<Vec<_>>() });
        let dep = |id: &str, kind: Option<&str>| json!({ "pkg": id, "dep_kinds": [{ "kind": kind }] });
        let metadata = json!({
            "packages": [package("root", &[&["bin"], &["custom-build"]]), package("serde", &[&["lib"], &["custom-build"]]), package("derive", &[&["proc-macro"]]), package("cc", &[&["lib"]]), package("mock", &[&["lib"]])],
            "resolve": { "root": "root", "nodes": [
                { "id": "root", "deps": [dep("serde", None), dep("cc", Some("build")), dep("mock", Some("dev"))] },
                { "id": "serde", "deps": [dep("derive", None)] },
                { "id": "derive", "deps": [] }, { "id": "cc", "deps": [] }, { "id": "mock", "deps": [] },
            ] },
        });
        assert_eq!(units(&metadata), Some(2 + 2 + 1 + 1), "root bin and build script, serde and its build script, derive, cc; mock is a dev-dependency");
        assert_eq!(units(&json!({})), None);
    }

    #[test]
    fn a_tty_redraws_the_running_step_in_place_and_a_pipe_prints_plain_lines() {
        let mut started = progress();
        started.steps[BUILD].state = State::Pending;
        started.steps[FETCH].state = State::Current;
        started.steps[FETCH].label = "Fetching main".to_string();
        started.meter = None;
        let mut tty = Printer::new(true);
        assert_eq!(tty.frame(&started, 100, 60), "\r\x1b[K▸ Fetching main  0:00");
        let building = progress();
        let drawn = tty.frame(&building, 202, 60);
        assert_eq!(drawn, "\r\x1b[K✓ Fetched main (3 new commits)\n\r\x1b[K▸ Building kelpie  ████████████░░░░░░░░  212/348 crates 1:42", "the finished step stays as a line, the running one is drawn over");
        assert_eq!(tty.close(), "\r\x1b[K");
        let mut pipe = Printer::new(false);
        assert_eq!(pipe.frame(&started, 100, 60), "▸ Fetching main  0:00\n");
        assert_eq!(pipe.frame(&building, 150, 60), "✓ Fetched main (3 new commits)\n▸ Building kelpie  ████████████░░░░░░░░  212/348 crates 0:50\n");
        assert_eq!(pipe.frame(&building, 202, 60), "", "nothing is redrawn into a pipe");
    }

    #[test]
    fn the_watcher_moves_from_the_fetch_to_the_build_once_cargo_writes_and_meters_it() {
        let state = crate::sh::test_dir("progress-watch");
        let dir = prepare_dir(&state);
        let tracker = Tracker::start(&file(&state), "Fetching main", "Build kelpie", 100);
        let watch = watch(tracker, dir.clone(), state.clone(), "Building kelpie".to_string(), || 100);
        std::fs::write(dir.join("cargo.jsonl"), "{\"reason\":\"compiler-artifact\"}\n").unwrap();
        std::fs::write(dir.join("cargo.log"), "   Compiling kelpie v0.27.0\n").unwrap();
        let started = std::time::Instant::now();
        while load(&file(&state)).and_then(|progress| progress.meter).is_none() {
            assert!(started.elapsed() < Duration::from_secs(10), "the watcher never metered the build");
            std::thread::sleep(Duration::from_millis(20));
        }
        let tracker = watch.stop();
        assert_eq!(tracker.progress.steps[FETCH].state, State::Done);
        assert_eq!((tracker.progress.steps[BUILD].label.as_str(), tracker.progress.steps[BUILD].state), ("Building kelpie", State::Current));
        assert_eq!(tracker.progress.meter, Some(Meter::Build { done: 1, total: None, compiling: Some("kelpie".to_string()) }), "no Cargo.toml here, so no total and the crate compiling");
    }
}
