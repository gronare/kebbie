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
    #[serde(default)]
    pub took: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Meter {
    Build { done: u64, total: Option<u64>, compiling: Option<String> },
    Download { what: String, got: u64, total: Option<u64> },
    Count { what: String, done: u64, total: u64 },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub version: String,
    pub new: Vec<String>,
    pub fixed: Vec<String>,
    pub restart: Vec<String>,
    #[serde(default)]
    pub sessions: Vec<crate::restart::Line>,
    #[serde(default)]
    pub restarted_at: u64,
    #[serde(default)]
    pub head: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub started: u64,
    pub steps: Vec<Step>,
    pub meter: Option<Meter>,
    pub summary: Option<Summary>,
    #[serde(default)]
    pub finished: bool,
    #[serde(default)]
    pub weights: Vec<u64>,
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
        let step = |label: &str| Step { label: label.to_string(), state: State::Pending, since: 0, took: 0 };
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
            if state == State::Done && step.state == State::Current && step.since > 0 {
                step.took = crate::messages::now().saturating_sub(step.since);
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

pub const DURATIONS: &str = "update-durations.json";
const CAP: f64 = 0.99;
const EASING: f64 = 1.5;

pub fn default_weights(channel: crate::config::Channel) -> Vec<u64> {
    match channel {
        crate::config::Channel::Main => vec![5, 150, 10, 15],
        crate::config::Channel::Release => vec![4, 6, 15, 20],
    }
}

fn read_durations(state: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(state.join(DURATIONS)).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|value| value.as_object().cloned()).unwrap_or_default()
}

pub fn weights(state: &Path, channel: crate::config::Channel) -> Vec<u64> {
    let defaults = default_weights(channel);
    let stored: Vec<u64> = read_durations(state).get(channel.key()).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).collect();
    if stored.len() == defaults.len() { stored } else { defaults }
}

pub fn record_durations(state: &Path, channel: crate::config::Channel, progress: &Progress) {
    let mut weights = weights(state, channel);
    for (weight, step) in weights.iter_mut().zip(&progress.steps) {
        if step.state == State::Done && step.since > 0 {
            *weight = (*weight + step.took).div_ceil(2).max(1);
        }
    }
    let mut stored = read_durations(state);
    stored.insert(channel.key().to_string(), Value::from(weights));
    let _ = crate::state::write(&state.join(DURATIONS), Value::Object(stored).to_string());
}

fn within(step: &Step, meter: Option<&Meter>, weight: u64, now: u64) -> f64 {
    let share = |done: u64, total: u64| if total == 0 { 0.0 } else { done as f64 / total as f64 };
    let measured = match meter {
        Some(Meter::Build { done, total: Some(total), .. }) => Some(share(*done, *total)),
        Some(Meter::Download { got, total: Some(total), .. }) => Some(share(*got, *total)),
        Some(Meter::Count { done, total, .. }) => Some(share(*done, *total)),
        _ => None,
    };
    measured.unwrap_or_else(|| (now.saturating_sub(step.since) as f64 / weight.max(1) as f64).min(0.9)).min(1.0)
}

pub fn fraction(progress: &Progress, now: u64) -> f64 {
    if progress.finished && progress.steps.iter().all(|step| step.state == State::Done) {
        return 1.0;
    }
    let weights = if progress.weights.len() == progress.steps.len() { progress.weights.clone() } else { default_weights(crate::config::Channel::Main) };
    let total: u64 = weights.iter().map(|weight| (*weight).max(1)).sum();
    let done: f64 = progress.steps.iter().zip(&weights).map(|(step, weight)| match step.state {
        State::Done => (*weight).max(1) as f64,
        State::Current => (*weight).max(1) as f64 * within(step, progress.meter.as_ref(), *weight, now),
        State::Pending | State::Failed => 0.0,
    }).sum();
    (done / total.max(1) as f64).min(CAP)
}

// Harrison et al., "Rethinking the progress bar" (UIST 2007): a bar that starts slow and speeds up at the end is felt as the shortest wait, so the shown share is the true one to the power 1.5.
pub fn eased(fraction: f64) -> f64 {
    if fraction >= 1.0 { 1.0 } else { fraction.max(0.0).powf(EASING).min(CAP) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Overall {
    shown: f64,
}

impl Overall {
    pub fn advance(&mut self, progress: &Progress, now: u64) -> f64 {
        self.shown = self.shown.max(eased(fraction(progress, now)));
        self.shown
    }
}

pub fn overall_line(shown: f64, width: usize) -> String {
    let percent = format!("{:>3}%", (shown * 100.0).floor() as u64);
    let room = width.saturating_sub(percent.chars().count() + 2).max(4);
    format!("{}  {percent}", bar((shown * 1000.0).round() as u64, 1000, room))
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
        Some(Meter::Count { what, done, total }) => (*done, *total, format!("{done}/{total} {what}")),
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
    drawn: usize,
    overall: Overall,
}

impl Printer {
    pub fn new(tty: bool) -> Printer {
        Printer { tty, shown: Vec::new(), drawn: 0, overall: Overall::default() }
    }

    pub fn frame(&mut self, progress: &Progress, now: u64, width: usize) -> String {
        let lines = lines(progress, now, width);
        let mut out = String::new();
        if self.tty {
            if self.drawn > 0 {
                out.push_str(&format!("\x1b[{}A", self.drawn));
            }
            let block: Vec<String> = std::iter::once(overall_line(self.overall.advance(progress, now), width)).chain(lines.iter().map(|(_, text)| text.clone())).collect();
            for text in &block {
                out.push_str(&format!("\r\x1b[K{text}\n"));
            }
            self.drawn = block.len();
        } else {
            for (index, (state, text)) in lines.iter().enumerate() {
                let before = self.shown.get(index).copied().unwrap_or(State::Pending);
                let ended = matches!(state, State::Done | State::Failed);
                if (ended && before != *state) || (*state == State::Current && before == State::Pending) {
                    out.push_str(&format!("{text}\n"));
                }
            }
        }
        self.shown = lines.into_iter().map(|(state, _)| state).collect();
        out
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
    fn a_tty_redraws_the_bar_and_the_steps_in_place_and_a_pipe_prints_plain_lines() {
        let mut started = progress();
        started.steps[BUILD].state = State::Pending;
        started.steps[FETCH].state = State::Current;
        started.steps[FETCH].label = "Fetching main".to_string();
        started.meter = None;
        let mut tty = Printer::new(true);
        let first = tty.frame(&started, 100, 60);
        assert!(first.starts_with("\r\x1b[K░") && first.contains("  0%\n\r\x1b[K▸ Fetching main  0:00\n\r\x1b[K  Building kelpie\n"), "{first:?}");
        let building = progress();
        let drawn = tty.frame(&building, 202, 60);
        assert!(drawn.starts_with("\x1b[5A\r\x1b[K█") && drawn.contains("\r\x1b[K✓ Fetched main (3 new commits)\n\r\x1b[K▸ Building kelpie  ████████████░░░░░░░░  212/348 crates 1:42\n"), "the cursor goes back up over the block and draws it anew: {drawn:?}");
        let mut pipe = Printer::new(false);
        assert_eq!(pipe.frame(&started, 100, 60), "▸ Fetching main  0:00\n");
        assert_eq!(pipe.frame(&building, 150, 60), "✓ Fetched main (3 new commits)\n▸ Building kelpie  ████████████░░░░░░░░  212/348 crates 0:50\n");
        assert_eq!(pipe.frame(&building, 202, 60), "", "nothing is redrawn into a pipe");
    }

    fn timed(states: [State; 4], weights: Vec<u64>) -> Progress {
        let steps = states.iter().map(|state| Step { label: String::new(), state: *state, since: 100, took: 0 }).collect();
        Progress { started: 100, steps, weights, ..Progress::default() }
    }

    #[test]
    fn the_weights_come_from_what_each_step_took_last_time_on_that_channel() {
        use crate::config::Channel;
        let state = crate::sh::test_dir("progress-weights");
        assert_eq!(weights(&state, Channel::Main), default_weights(Channel::Main));
        assert!(default_weights(Channel::Main)[BUILD] > default_weights(Channel::Main).iter().sum::<u64>() / 2, "on main the build carries the bar");
        let release = default_weights(Channel::Release);
        assert!(release[PLUGINS] + release[RESTART] > release[FETCH] + release[BUILD], "on release the plugins and restarts carry it");
        let mut ran = timed([State::Done; 4], Vec::new());
        for (step, took) in ran.steps.iter_mut().zip([10, 50, 20, 40]) {
            step.took = took;
        }
        ran.steps[FETCH].since = 0;
        record_durations(&state, Channel::Main, &ran);
        assert_eq!(weights(&state, Channel::Main), [5, 100, 15, 28], "halfway to what each step took; a step that never ran keeps its weight");
        assert_eq!(weights(&state, Channel::Release), release, "each channel keeps its own");
    }

    #[test]
    fn the_bar_eases_in_grows_only_and_reaches_the_end_only_when_everything_is_done() {
        let mut last = 0.0;
        for step in 0..=100 {
            let shown = eased(f64::from(step) / 100.0 * CAP);
            assert!(shown >= last && shown < 1.0, "{step}: {shown}");
            last = shown;
        }
        assert!(eased(0.5) < 0.5, "slow at the start, fast at the end");
        let mut running = timed([State::Done, State::Done, State::Done, State::Current], vec![5, 150, 10, 15]);
        running.meter = Some(Meter::Count { what: "sessions".to_string(), done: 3, total: 3 });
        assert!(fraction(&running, 100) < 1.0, "every session gone through, yet the update has not said it is done");
        running.finished = true;
        assert!(fraction(&running, 100) < 1.0, "finished with a step still running is not done");
        running.steps[RESTART].state = State::Done;
        assert_eq!(fraction(&running, 100), 1.0);
        let mut overall = Overall::default();
        let ahead = timed([State::Current, State::Pending, State::Pending, State::Pending], vec![5, 150, 10, 15]);
        let high = overall.advance(&ahead, 105);
        assert!(overall.advance(&ahead, 100) == high && high > 0.0, "never backwards");
    }

    #[test]
    fn on_the_release_channel_a_quick_download_leaves_most_of_the_bar_to_the_plugins_and_restarts() {
        let weights = default_weights(crate::config::Channel::Release);
        let mut downloaded = timed([State::Done, State::Done, State::Current, State::Pending], weights);
        downloaded.meter = Some(Meter::Count { what: "plugins".to_string(), done: 0, total: 2 });
        assert!(fraction(&downloaded, 100) < 0.3, "{}", fraction(&downloaded, 100));
        downloaded.meter = Some(Meter::Count { what: "plugins".to_string(), done: 2, total: 2 });
        assert!(fraction(&downloaded, 100) > 0.5 && fraction(&downloaded, 100) < 0.9, "the restarts still carry the rest: {}", fraction(&downloaded, 100));
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
