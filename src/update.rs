use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::{self, Channel};
use crate::herdr;
use crate::lead;
use crate::release::{self, Plan, Request};
use crate::setup;
use crate::sh::{self, Result};
use crate::changes;
use crate::update_check::{self, Component, Places};
use crate::update_progress::{self as progress, Summary, Tracker};
use crate::vault_cli;

pub struct Outcome {
    pub summary: Summary,
    pub details: Vec<String>,
    pub started: u64,
}

fn now() -> u64 {
    crate::messages::now()
}

fn short_head(root: &Path) -> Option<String> {
    update_check::git(root, &["rev-parse", "HEAD"]).ok()
}

fn labels(channel: Channel, request: &Request, running: Option<&release::Override>) -> (String, &'static str) {
    match (request, running) {
        (Request::From(from), _) => (format!("Fetching {from}"), "Build kebbie"),
        (Request::Channel, Some(running)) => (format!("Fetching {}", running.name()), "Build kebbie"),
        _ if channel == Channel::Release => ("Finding the latest release".to_string(), "Download kebbie"),
        _ => ("Fetching main".to_string(), "Build kebbie"),
    }
}

pub fn run(claude_dir: &Path, visible: bool, request: &Request) -> Result<Outcome> {
    let plugin = kebbie_plugin()?;
    let root = plugin["plugin_root"].as_str().filter(|root| !root.is_empty()).map(PathBuf::from);
    let channel = config::channel()?;
    let state = lead::state_dir()?;
    let places = Places::new(root.clone(), claude_dir.to_path_buf(), channel);
    let before = update_check::installed(&places);
    let old_head = root.as_deref().and_then(short_head);
    let path = progress::file(&state);
    let (fetch, build) = labels(channel, request, release::read_override(&state).as_ref());
    let weighed = build_mode(channel, release::read_override(&state).is_some());
    let mut tracker = Tracker::start(&path, &fetch, build, now());
    tracker.progress.weights = progress::weights(&state, weighed);
    tracker.save();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let printer = visible.then(|| progress::print_while(path.clone(), stop.clone()));
    let (mut tracker, installed) = herdr_plugin(&plugin, &state, channel, request, tracker, old_head.as_deref());
    let result = installed.map(|line| {
        let mut lines = vec![line];
        lines.extend(finish_on_new_binary(claude_dir, root.as_deref(), &path));
        lines
    });
    tracker = Tracker::resume(&path).unwrap_or(tracker);
    let outcome = result.map(|lines| {
        let after = update_check::installed(&places);
        let running = release::read_override(&state);
        let mut details = versions(&before, &after, channel);
        details.insert(0, match &running {
            Some(running) => format!("kebbie is {}", running.describe(channel)),
            None => format!("kebbie follows the {} channel", channel.key()),
        });
        details.extend(crate::lead_restart::versions().map(|(from, to)| format!("lead: runs {from}; {to} is installed")));
        details.push(String::new());
        details.extend(lines);
        let version = after.iter().find(|component| component.name == update_check::KEBBIE).and_then(|component| component.installed.clone()).unwrap_or_default();
        let was = before.iter().find(|component| component.name == update_check::KEBBIE).and_then(|component| component.installed.clone());
        let main = channel == Channel::Main || running.is_some();
        let olds = old_commits(&before, old_head.as_deref());
        let notes = root.as_deref().map(|root| changes::for_update(root, &olds, was.as_deref(), &version, main)).unwrap_or_default();
        let head = root.as_deref().and_then(|root| update_check::git(root, &["rev-parse", "--short=7", "HEAD"]).ok()).unwrap_or_default();
        let from = if channel == Channel::Main { olds.first().cloned().unwrap_or_default() } else { String::new() };
        let restarted = tracker.progress.summary.take().unwrap_or_default();
        Outcome { summary: Summary { version: crate::shown_number(channel, &version), new: notes.new, fixed: notes.fixed, head, from, ..restarted }, details, started: tracker.progress.started }
    });
    match &outcome {
        Ok(outcome) => {
            progress::record_durations(&state, weighed, &tracker.progress);
            tracker.finish(outcome.summary.clone());
        }
        Err(_) => {
            tracker.fail_current();
            tracker.progress.finished = true;
            tracker.save();
        }
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(printer) = printer {
        let _ = printer.join();
    }
    outcome
}

// A checkout can reach the new commit before the update runs, when kebbie is developed in it, so the binary's own commit says best where the update came from.
fn old_commits(before: &[Component], old_head: Option<&str>) -> Vec<String> {
    let built = before.iter().find(|component| component.name == update_check::KEBBIE).and_then(|component| component.built.clone());
    built.into_iter().chain(old_head.map(str::to_string)).collect()
}

fn section(lines: &mut Vec<String>, heading: &str, entries: impl IntoIterator<Item = String>) {
    let entries: Vec<String> = entries.into_iter().collect();
    if !entries.is_empty() {
        lines.push(heading.to_string());
        lines.extend(entries.into_iter().map(|entry| format!("  • {entry}")));
    }
}

pub fn bug_line(title: &str) -> String {
    format!("A bug you ran into: {title}")
}

pub fn text(outcome: &Outcome, bugs: &[String]) -> String {
    let summary = &outcome.summary;
    let mut lines = vec![if summary.version.is_empty() { "✓ Updated kebbie".to_string() } else { format!("✓ Updated kebbie {}", summary.version) }, String::new()];
    if summary.new.is_empty() && summary.fixed.is_empty() && bugs.is_empty() {
        lines.push("No notes for this update".to_string());
    }
    section(&mut lines, "What's new", summary.new.clone());
    section(&mut lines, "Fixed", summary.fixed.iter().cloned().chain(bugs.iter().map(|title| bug_line(title))));
    if !summary.restart.is_empty() {
        lines.push(String::new());
        lines.push("Restarting".to_string());
        lines.extend(summary.restart.iter().map(|line| format!("  {line}")));
    }
    lines.push(String::new());
    lines.push("Details".to_string());
    lines.extend(outcome.details.iter().cloned());
    lines.join("\n")
}

// The steps after the build run on the binary just built, so a fix to them reaches the update that installs it.
fn finish_on_new_binary(claude_dir: &Path, root: Option<&Path>, progress: &Path) -> Vec<String> {
    let built = root.map(|root| root.join("target/release/kebbie")).filter(|binary| binary.is_file());
    let env = format!("{}={}", progress::ENV, progress.display());
    match built.map(|binary| sh::run("env", &[&env, &binary.to_string_lossy(), "update", "--finish"], None)) {
        Some(Ok(output)) => output.lines().map(str::to_string).collect(),
        Some(Err(error)) => vec![format!("the new kebbie could not finish the update ({error}); this one does")].into_iter().chain(finish(claude_dir, Some(progress)).unwrap_or_else(|error| vec![error])).collect(),
        None => finish(claude_dir, Some(progress)).unwrap_or_else(|error| vec![error]),
    }
}

// The old keeper stops before the update restarts sessions, so the two never restart one together, and a keeper on this binary starts after.
pub fn finish(claude_dir: &Path, progress: Option<&Path>) -> Result<Vec<String>> {
    let state = lead::state_dir()?;
    let mut tracker = progress.and_then(Tracker::resume);
    let keeper_ran = crate::keeper::stop_for_update(&state);
    let steps = finish_in(claude_dir, &state, &mut tracker);
    let keeper = crate::keeper::start_after_update(&state, keeper_ran);
    if let Some(tracker) = tracker.as_mut().filter(|_| steps.is_err()) {
        tracker.fail_current();
    }
    match steps {
        Ok(mut lines) => {
            lines.push(keeper);
            Ok(lines)
        }
        Err(error) => Err(format!("{error}\n{keeper}")),
    }
}

fn step(tracker: &mut Option<Tracker>, index: usize, label: &str, done: bool) {
    if let Some(tracker) = tracker.as_mut() {
        if done { tracker.done(index, label) } else { tracker.begin(index, label, now()) }
    }
}

fn count(tracker: &mut Option<Tracker>, what: &str, done: usize, total: usize) {
    if let Some(tracker) = tracker.as_mut() {
        tracker.meter(progress::Meter::Count { what: what.to_string(), done: done as u64, total: total as u64 });
    }
}

fn finish_in(claude_dir: &Path, state: &Path, tracker: &mut Option<Tracker>) -> Result<Vec<String>> {
    let plugin = kebbie_plugin()?;
    let root = plugin["plugin_root"].as_str().filter(|root| !root.is_empty()).map(PathBuf::from);
    let channel = config::channel()?;
    let running = release::read_override(state);
    let vault_installed = vault_cli::installed(claude_dir).is_some();
    let plugins = 1 + usize::from(vault_installed);
    step(tracker, progress::PLUGINS, "Claude Code and vault plugins", false);
    count(tracker, "plugins", 0, plugins);
    let (claude, updated) = updated_claude_plugin(claude_dir, &setup::claude_bin(), root.as_deref(), channel == Channel::Main && running.is_none());
    count(tracker, "plugins", 1, plugins);
    let vault = vault_plugin(claude_dir, &setup::claude_bin());
    count(tracker, "plugins", plugins, plugins);
    step(tracker, progress::PLUGINS, &plugins_said(&claude, updated, vault_installed, &vault), true);
    let mut lines = vec![claude];
    lines.extend(vault);
    lines.extend(crate::push_block::move_from_kelpie(&crate::status::known_repos(&crate::herdr::workspaces().unwrap_or_default(), crate::repo::pooled_checkouts()).into_iter().collect::<Vec<_>>(), state));
    step(tracker, progress::RESTART, "Restarting sessions", false);
    update_check::forget(state);
    crate::restart::mark_updated(state, crate::restart::now())?;
    let restarted = crate::restart::run_counted(crate::restart::Mode::Update, &mut crate::restart::Memory::default(), None, &mut |done, total| count(tracker, "sessions", done, total))?;
    lines.extend(crate::restart::report(&restarted));
    lines.extend(crate::known::announce().unwrap_or_else(|error| vec![format!("telling the sessions what changed failed: {error}")]));
    let restarted_at = now();
    let board = lead::renew_board().unwrap_or_else(|error| error);
    let mut said = restart_summary(&restarted);
    said.extend(board_summary(&board));
    lines.push(board);
    lines.extend(crate::issues::after_update(state, root.as_deref()));
    step(tracker, progress::RESTART, "Sessions move to the new kebbie", true);
    if let Some(tracker) = tracker.as_mut() {
        tracker.progress.summary = Some(Summary { restart: said, sessions: restarted, restarted_at, ..Summary::default() });
        tracker.save();
    }
    Ok(lines)
}

// The plugins step names a plugin as updated only when claude updated it, and says what became of one it did not.
fn plugins_said(claude: &str, updated: bool, vault_installed: bool, vault: &[String]) -> String {
    match (updated, vault_installed, vault.first()) {
        (true, true, None) => "Updated the Claude Code and vault plugins".to_string(),
        (true, false, _) => "Updated the Claude Code plugin".to_string(),
        (true, true, Some(vault)) => format!("Updated the Claude Code plugin; {vault}"),
        (false, true, None) => format!("Updated the vault plugin; {claude}"),
        (false, true, Some(vault)) => format!("{claude}; {vault}"),
        (false, false, _) => claude.to_string(),
    }
}

fn counted(lead: bool, sessions: usize) -> Option<(String, bool)> {
    let sessions = match sessions {
        0 => None,
        1 => Some("1 session".to_string()),
        many => Some(format!("{many} sessions")),
    };
    let plural = sessions.as_deref().is_some_and(|text| text != "1 session");
    match (lead, sessions) {
        (true, Some(sessions)) => Some((format!("The lead and {sessions}"), true)),
        (true, None) => Some(("The lead".to_string(), false)),
        (false, Some(sessions)) => Some((sessions, plural)),
        (false, None) => None,
    }
}

pub fn restart_summary(lines: &[crate::restart::Line]) -> Vec<String> {
    let lead = crate::status::LEAD_LABEL;
    let group = |test: &dyn Fn(&str) -> bool| (lines.iter().any(|line| line.label == lead && test(&line.said)), lines.iter().filter(|line| line.label != lead && test(&line.said)).count());
    let on = group(&|said| said.starts_with("restarted") || said == "up to date");
    let failed = group(&|said| said.starts_with("restart failed"));
    let asked = group(&|said| said.starts_with("waiting for your answer"));
    let manual = group(&|said| said.contains("run kebbie restart"));
    let held = group(&|said| said.starts_with("stale, held"));
    let waits = group(&|said| !said.contains("run kebbie restart") && (said.contains("restarts when") || said.contains("restarts once") || said.contains("then restarts") || said.starts_with("not restarted yet")));
    let mut said = Vec::new();
    if let Some((who, many)) = counted(on.0, on.1) {
        said.push(format!("✓ {who} {} on the new kebbie", if many { "are" } else { "is" }));
    }
    if let Some((who, many)) = counted(waits.0, waits.1) {
        said.push(if many { format!("{who} restart when they're idle") } else { format!("{who} restarts when it's idle") });
    }
    if let Some((who, many)) = counted(manual.0, manual.1) {
        let them = if many { "them" } else { "it" };
        said.push(format!("{who} keep{} the old kebbie until you restart {them}: restart.auto is off", if many { "" } else { "s" }));
    }
    if let Some((who, many)) = counted(held.0, held.1) {
        said.push(format!("{who} stay{} on the old kebbie, as a restart did not move {}; d shows why", if many { "" } else { "s" }, if many { "them" } else { "it" }));
    }
    if let Some((who, many)) = counted(asked.0, asked.1) {
        said.push(format!("{who} wait{} for your answer, as background work is still running", if many { "" } else { "s" }));
    }
    if let Some((who, _)) = counted(failed.0, failed.1) {
        said.push(format!("Restarting {} failed; d shows why", who.replacen("The lead", "the lead", 1)));
    }
    if said.is_empty() && lines.is_empty() {
        said.push("No Claude sessions are running".to_string());
    }
    said
}

pub fn board_summary(board: &str) -> Option<String> {
    match board {
        "board: restarted on the new kebbie" => Some("The board restarted on the new kebbie".to_string()),
        line if line.starts_with("board: not running") || line.starts_with("board: restarts on the new kebbie once") => Some("Open boards restart on the new kebbie when you next open them".to_string()),
        "board: up to date" | "board: no lead Space" | "board: the lead Space has no board pane" => None,
        _ => Some("The board did not restart; d shows why".to_string()),
    }
}

// The sessions the update went through, each as the latest restart round sees it; one no longer stale there runs the new kebbie.
pub fn live_sessions(update: &[crate::restart::Line], stale: Option<&[String]>) -> Vec<crate::restart::Line> {
    update.iter().map(|line| {
        let Some(stale) = stale.filter(|_| !line.said.starts_with("skipped")) else { return line.clone() };
        let prefix = format!("{}: ", line.label);
        let said = stale.iter().find_map(|entry| entry.strip_prefix(&prefix)).map_or("up to date", |said| said.split(crate::keeper::REASONS).next().unwrap_or(said));
        crate::restart::Line { said: said.to_string(), ..line.clone() }
    }).collect()
}

pub fn check(claude_dir: &Path) -> String {
    let places = Places::new(update_check::install_root(), claude_dir.to_path_buf(), config::channel().unwrap_or_default());
    let running = lead::state_location().ok().and_then(|state| release::read_override(&state)).map(|running| running.describe(places.channel));
    format!("channel {}\n\n{}", places.channel.key(), update_check::render(&update_check::check(&places), running.as_deref(), places.channel))
}

fn versions(before: &[Component], after: &[Component], channel: Channel) -> Vec<String> {
    after.iter().map(|now| {
        let earlier = before.iter().find(|was| was.name == now.name);
        let was = earlier.and_then(|was| was.installed.clone());
        let built = (earlier.and_then(|was| was.built.as_ref()), now.built.as_ref());
        if channel == Channel::Main && now.name == update_check::KEBBIE && now.installed.is_some() {
            return match built {
                (Some(old), Some(new)) if old != new => format!("kebbie main rebuilt, {old} → {new}"),
                _ if was == now.installed => "kebbie main, already latest".to_string(),
                _ => "kebbie main rebuilt".to_string(),
            };
        }
        match (was, &now.installed) {
            (Some(was), Some(now_version)) if &was == now_version => match built {
                (Some(old), Some(new)) if old != new => format!("{} {was} rebuilt, {old} → {new}", now.label()),
                _ => format!("{} {was}, already latest", now.label()),
            },
            (Some(was), Some(now_version)) => format!("{} {was} → {now_version}", now.label()),
            (None, Some(now_version)) => format!("{} → {now_version}", now.label()),
            (_, None) => format!("{}: version unknown", now.label()),
        }
    }).collect()
}

fn kebbie_plugin() -> Result<Value> {
    let list: Value = serde_json::from_str(&sh::run(&herdr::bin(), &["plugin", "list", "--json"], None)?).map_err(|error| format!("herdr plugin list: {error}"))?;
    list["result"]["plugins"].as_array().into_iter().flatten().find(|plugin| plugin["plugin_id"] == "kebbie").cloned()
        .ok_or_else(|| "kebbie is not registered with herdr; kebbie doctor shows how".to_string())
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map_or_else(String::new, |first| first.to_uppercase().chain(characters).collect())
}

fn fetched(root: &Path, old_head: Option<&str>, line: Option<&str>, pulled: bool, local: bool) -> String {
    let said = line.map(|line| capitalized(line.split(';').next().unwrap_or(line).trim()));
    if !pulled {
        return said.unwrap_or_else(|| "Fetched".to_string());
    }
    let branch = update_check::git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).ok().filter(|branch| branch != "HEAD").unwrap_or_else(|| "main".to_string());
    let count = old_head.and_then(|old| update_check::git(root, &["rev-list", "--count", &format!("{old}..HEAD")]).ok()).and_then(|count| count.parse::<u64>().ok());
    let not_pulled = said.filter(|_| line.is_some_and(|line| line.contains("not pulled")));
    let ahead = local.then(|| update_check::git(root, &["rev-list", "--count", "@{upstream}..HEAD"]).ok().and_then(|count| count.parse::<u64>().ok()).unwrap_or(0));
    fetch_label(&branch, count, not_pulled, ahead)
}

fn fetch_label(branch: &str, count: Option<u64>, not_pulled: Option<String>, local_ahead: Option<u64>) -> String {
    let fetched = match (count, not_pulled) {
        (Some(0), Some(said)) => said,
        (Some(0), None) => format!("Fetched {branch} (no new commits)"),
        (Some(1), _) => format!("Fetched {branch} (1 new commit)"),
        (Some(count), _) => format!("Fetched {branch} ({count} new commits)"),
        (None, _) => format!("Fetched {branch}"),
    };
    match local_ahead {
        None => fetched,
        Some(0) => format!("{fetched}; building your local checkout"),
        Some(1) => format!("{fetched}; building your local checkout, 1 commit ahead"),
        Some(ahead) => format!("{fetched}; building your local checkout, {ahead} commits ahead"),
    }
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

fn built_label(root: &Path, was: Option<std::time::SystemTime>) -> String {
    let binary = root.join("target/release/kebbie");
    match (binary_origin(root), modified(&binary) == was) {
        (Some(Origin::Downloaded { tag, .. }), false) => format!("Downloaded kebbie {}", tag.trim_start_matches('v')),
        (Some(Origin::Downloaded { .. }), true) => "kebbie was downloaded already".to_string(),
        (_, true) => "kebbie was built already".to_string(),
        _ => "Built kebbie".to_string(),
    }
}

fn herdr_plugin(plugin: &Value, state: &Path, channel: Channel, request: &Request, mut tracker: Tracker, old_head: Option<&str>) -> (Tracker, Result<String>) {
    let Some(root) = plugin["plugin_root"].as_str().filter(|root| !root.is_empty()).map(PathBuf::from) else {
        tracker.fail_current();
        return (tracker, Err("herdr names no folder kebbie is installed in".to_string()));
    };
    let dir = progress::prepare_dir(state);
    let binary = modified(&root.join("target/release/kebbie"));
    let release_build = build_mode(channel, release::read_override(state).is_some()) == Channel::Release;
    let building = if release_build { "Downloading kebbie" } else { "Building kebbie" };
    let local = plugin["source"]["kind"] == "local";
    let plan = release::install(&root, state, channel, request, !local, &release::Binaries::of(&root));
    match plan {
        Err(error) => {
            tracker.fail_current();
            (tracker, Err(error))
        }
        Ok(Plan::Herdr) => {
            let watch = progress::watch(tracker, dir.clone(), root.clone(), building.to_string(), now);
            let progress_env = format!("{}={}", progress::BUILD_ENV, dir.display());
            let ran = sh::run("env", &["KEBBIE_CHANNEL=main", &progress_env, &herdr::bin(), "plugin", "update", "--yes", "kebbie"], None);
            let mut tracker = watch.stop();
            if tracker.state(progress::BUILD) == Some(progress::State::Pending) && ran.is_ok() {
                tracker.done(progress::FETCH, &fetched(&root, old_head, None, true, false));
                tracker.begin(progress::BUILD, building, now());
            }
            match ran {
                Ok(_) => {
                    tracker.done(progress::BUILD, &built_label(&root, binary));
                    (tracker, Ok("herdr plugin: updated from GitHub and rebuilt".to_string()))
                }
                Err(error) => {
                    tracker.fail_current();
                    (tracker, Err(error))
                }
            }
        }
        Ok(Plan::Done(line)) => {
            tracker.done(progress::FETCH, &fetched(&root, old_head, Some(&line), channel == Channel::Main && *request == Request::Channel && release::read_override(state).is_none(), local));
            let label = match crate::issue_recheck::version_at(&root).filter(|_| release_build) {
                Some(version) => format!("Downloading kebbie {version}"),
                None => building.to_string(),
            };
            tracker.begin(progress::BUILD, &label, now());
            let watch = progress::watch(tracker, dir.clone(), root.clone(), label, now);
            let built = build(&root, build_mode(channel, release::read_override(state).is_some()), Some(&dir));
            let mut tracker = watch.stop();
            match built {
                Ok(said) => {
                    tracker.done(progress::BUILD, &built_label(&root, binary));
                    (tracker, Ok(format!("herdr plugin: {line}; {said}")))
                }
                Err(error) => {
                    tracker.fail_current();
                    (tracker, Err(error))
                }
            }
        }
    }
}

// kebbie run from the main checkout it was built in, on the main channel, rebuilds from what that checkout now holds; the board and keeper restart onto the new binary.
pub fn rebuild_staged(repo: &Path, announce: bool) -> Option<String> {
    let root = update_check::install_root()?;
    let state = lead::state_dir().ok()?;
    if !same_folder(&root, repo) {
        return None;
    }
    let channel = config::channel().ok()?;
    if let Some(running) = release::read_override(&state) {
        return Some(format!("not rebuilt: kebbie is {}; kebbie update --stable goes back to building this checkout", running.describe(channel)));
    }
    if channel != Channel::Main {
        return Some(format!("not rebuilt: kebbie follows the {} channel, which installs release binaries", channel.key()));
    }
    if announce {
        eprintln!("building kebbie from local main…");
    }
    Some(build(&root, Channel::Main, None).unwrap_or_else(|error| format!("the build failed: {error}")))
}

pub const BUILD_SCRIPT: &str = "scripts/build.sh";
const ORIGIN_FILE: &str = "target/release/kebbie.origin";

#[derive(Debug, PartialEq)]
pub enum Origin {
    Downloaded { tag: String, target: String },
    Built,
}

impl Origin {
    pub fn describe(&self) -> String {
        match self {
            Origin::Downloaded { tag, target } => format!("downloaded {tag} for {target}"),
            Origin::Built => "built from source".to_string(),
        }
    }
}

// The build script writes the origin file after the binary it downloaded, so a binary newer than it was built by cargo since.
pub fn binary_origin(root: &Path) -> Option<Origin> {
    let modified = |path: PathBuf| std::fs::metadata(path).and_then(|meta| meta.modified()).ok();
    let binary = modified(root.join("target/release/kebbie"))?;
    let downloaded = modified(root.join(ORIGIN_FILE)).filter(|written| *written >= binary).and_then(|_| std::fs::read_to_string(root.join(ORIGIN_FILE)).ok()).and_then(|text| {
        let mut words = text.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("downloaded"), Some(tag), Some(target)) => Some(Origin::Downloaded { tag: tag.to_string(), target: target.to_string() }),
            _ => None,
        }
    });
    Some(downloaded.unwrap_or(Origin::Built))
}

// A branch or checkout installed with --from has no release binary, so it builds from source as the main channel does.
fn build_mode(channel: Channel, overridden: bool) -> Channel {
    if overridden { Channel::Main } else { channel }
}

// An install root checked out at a commit from before the build script has only cargo; with `progress` the script writes cargo's output there instead of keeping it quiet.
fn build(root: &Path, channel: Channel, progress: Option<&Path>) -> Result<String> {
    if root.join(BUILD_SCRIPT).is_file() {
        let channel = format!("KEBBIE_CHANNEL={}", channel.key());
        match progress {
            Some(dir) => sh::run("env", &[&channel, &format!("{}={}", progress::BUILD_ENV, dir.display()), "sh", BUILD_SCRIPT], Some(root))?,
            None => sh::run("env", &[&channel, "sh", BUILD_SCRIPT, "--quiet"], Some(root))?,
        };
    } else {
        let _ = std::fs::remove_file(root.join(ORIGIN_FILE));
        sh::run("cargo", &["build", "--release", "--quiet"], Some(root))?;
    }
    let origin = binary_origin(root).map_or_else(|| "built".to_string(), |origin| origin.describe());
    Ok(format!("{origin} in {}", root.display()))
}

fn vault_plugin(claude_dir: &Path, claude: &str) -> Vec<String> {
    let Some((id, _, _)) = vault_cli::installed(claude_dir) else { return Vec::new() };
    let marketplace = id.split_once('@').map_or("gronare", |(_, marketplace)| marketplace).to_string();
    for args in [vec!["plugin", "marketplace", "update", marketplace.as_str()], vec!["plugin", "update", id.as_str()]] {
        if let Err(error) = sh::run(claude, &args, None) {
            return vec![format!("vault plugin: {error}")];
        }
    }
    Vec::new()
}

#[derive(Debug, PartialEq)]
pub enum Marketplace {
    InPlace(PathBuf),
    Git,
}

// A marketplace added from a folder loads its plugin from that folder at each start, so the install root's checkout is the plugin, whatever its version says.
pub fn marketplace(claude_dir: &Path, id: &str) -> Marketplace {
    let name = id.split_once('@').map_or(id, |(_, name)| name);
    let known: Value = std::fs::read_to_string(claude_dir.join("plugins/known_marketplaces.json")).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let entry = &known[name];
    if entry["source"]["source"] != "directory" {
        return Marketplace::Git;
    }
    Marketplace::InPlace(PathBuf::from(entry["source"]["path"].as_str().or(entry["installLocation"].as_str()).unwrap_or_default()))
}

pub const IN_PLACE_FIX: &str = "claude plugin marketplace remove kebbie && claude plugin marketplace add ~/.local/share/kebbie/current && claude plugin install kebbie@kebbie";

pub fn same_folder(one: &Path, other: &Path) -> bool {
    one.canonicalize().ok().is_some_and(|one| other.canonicalize().ok().as_ref() == Some(&one))
}

pub fn claude_plugin(claude_dir: &Path, claude: &str, root: Option<&Path>, follows_github: bool) -> String {
    updated_claude_plugin(claude_dir, claude, root, follows_github).0
}

// The line for the plugin, and whether claude updated it.
fn updated_claude_plugin(claude_dir: &Path, claude: &str, root: Option<&Path>, follows_github: bool) -> (String, bool) {
    let installed: Value = std::fs::read_to_string(claude_dir.join("plugins/installed_plugins.json")).ok()
        .and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let Some(id) = installed["plugins"].as_object().and_then(|plugins| plugins.keys().find(|id| id.starts_with("kebbie@")).cloned()) else {
        return ("Claude Code plugin: not installed; kebbie doctor shows how".to_string(), false);
    };
    match marketplace(claude_dir, &id) {
        Marketplace::InPlace(path) if root.is_some_and(|root| same_folder(&path, root)) => {
            let marketplace = id.split_once('@').map_or("kebbie", |(_, market)| market);
            match sh::run(claude, &["plugin", "marketplace", "update", marketplace], None).and_then(|_| sh::run(claude, &["plugin", "update", &id], None)) {
                Ok(output) => (format!("Claude Code plugin: {}", output.lines().last().unwrap_or("up to date").trim_start_matches(['✔', ' '])), true),
                Err(error) => (format!("Claude Code plugin: {error}"), false),
            }
        }
        Marketplace::InPlace(path) => (format!("Claude Code plugin: loaded in place from {}, which is not the install root; {IN_PLACE_FIX}", path.display()), false),
        Marketplace::Git if follows_github => match sh::run(claude, &["plugin", "update", &id], None) {
            Ok(output) => (format!("Claude Code plugin: {}", output.lines().last().unwrap_or("up to date").trim_start_matches(['✔', ' '])), true),
            Err(error) => (format!("Claude Code plugin: {error}"), false),
        },
        Marketplace::Git => (format!("Claude Code plugin: left as it is, since its GitHub marketplace follows the default branch and not this channel. Load it from the install root once (the remove forgets its options, such as auto_open): {IN_PLACE_FIX}"), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_update_moves_kelpies_push_blocks_over_before_it_restarts_the_sessions() {
        let source = include_str!("update.rs");
        let finish = &source[source.find("\nfn finish_in(").unwrap()..];
        assert!(finish.find("move_from_kelpie(").unwrap() < finish.find("restart::run_counted(").unwrap());
    }
    use serde_json::json;

    #[test]
    fn the_plugins_step_says_updated_only_for_a_plugin_claude_updated() {
        let home = scratch("plugins-said");
        let (claude, _) = fake_claude(&home);
        let (line, updated) = updated_claude_plugin(&home, &claude, None, true);
        assert_eq!((line.as_str(), updated), ("Claude Code plugin: not installed; kebbie doctor shows how", false));
        assert_eq!(plugins_said(&line, updated, false, &[]), "Claude Code plugin: not installed; kebbie doctor shows how");
        assert_eq!(plugins_said(&line, updated, true, &[]), "Updated the vault plugin; Claude Code plugin: not installed; kebbie doctor shows how");
        assert_eq!(plugins_said("Claude Code plugin: up to date", true, true, &[]), "Updated the Claude Code and vault plugins");
        assert_eq!(plugins_said("Claude Code plugin: up to date", true, false, &[]), "Updated the Claude Code plugin");
        assert_eq!(plugins_said("Claude Code plugin: up to date", true, true, &["vault plugin: offline".to_string()]), "Updated the Claude Code plugin; vault plugin: offline");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn the_steps_after_the_build_run_on_the_binary_just_built() {
        let dir = scratch("finish");
        let binary = dir.join("target/release/kebbie");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        crate::executable::write(&binary, "#!/bin/sh\n[ \"$*\" = \"update --finish\" ] && [ -n \"$KEBBIE_UPDATE_PROGRESS\" ] && printf 'Claude Code plugin: from the new binary\\nboard: up to date\\n'\n").unwrap();
        assert_eq!(finish_on_new_binary(&dir, Some(&dir), &dir.join(progress::FILE)), ["Claude Code plugin: from the new binary", "board: up to date"]);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_claude(dir: &Path) -> (String, PathBuf) {
        let log = dir.join("claude.log");
        let bin = dir.join("claude");
        crate::executable::write(&bin, format!("#!/bin/sh\necho \"$*\" >> '{}'\n", log.display())).unwrap();
        (bin.to_string_lossy().to_string(), log)
    }

    #[test]
    fn a_checkout_installed_with_from_builds_from_source_even_on_the_release_channel() {
        assert_eq!(build_mode(Channel::Release, false), Channel::Release);
        assert_eq!(build_mode(Channel::Release, true), Channel::Main);
        assert_eq!(build_mode(Channel::Main, false), Channel::Main);
    }

    #[test]
    fn update_skips_the_vault_plugin_when_it_is_not_installed() {
        let dir = scratch("no-vault");
        let (claude, log) = fake_claude(&dir);
        assert!(vault_plugin(&dir.join("claude-home"), &claude).is_empty());
        assert!(!log.exists(), "claude was not run");
    }

    #[test]
    fn update_refreshes_the_marketplace_then_updates_the_vault_plugin() {
        let dir = scratch("vault");
        let (claude, log) = fake_claude(&dir);
        let home = dir.join("claude-home");
        let install = home.join("plugins/cache/gronare/vault/0.2.16");
        std::fs::create_dir_all(install.join("hooks")).unwrap();
        std::fs::write(install.join("hooks/hooks.json"), r#"{"command":"uvx notes-vault-mcp==0.6.0 hook session-start"}"#).unwrap();
        std::fs::write(home.join("plugins/installed_plugins.json"), json!({ "plugins": { "vault@gronare": [{ "installPath": install, "version": "0.2.16" }] } }).to_string()).unwrap();
        assert!(vault_plugin(&home, &claude).is_empty());
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "plugin marketplace update gronare\nplugin update vault@gronare\n");
        assert_eq!(vault_plugin(&home, "false"), ["vault plugin: false plugin marketplace update gronare failed"]);
    }

    fn claude_home(dir: &Path, source: Value) -> PathBuf {
        let home = dir.join("claude-home");
        std::fs::create_dir_all(home.join("plugins")).unwrap();
        std::fs::write(home.join("plugins/installed_plugins.json"), json!({ "plugins": { "kebbie@kebbie": [{ "version": "0.20.0" }] } }).to_string()).unwrap();
        std::fs::write(home.join("plugins/known_marketplaces.json"), json!({ "kebbie": { "source": source, "installLocation": "/elsewhere" } }).to_string()).unwrap();
        home
    }

    #[test]
    fn the_claude_code_plugin_follows_github_only_on_the_main_channel() {
        let dir = scratch("claude-plugin");
        let (claude, log) = fake_claude(&dir);
        let home = claude_home(&dir, json!({ "source": "git", "url": "https://github.com/gronare/kebbie.git" }));
        assert_eq!(marketplace(&home, "kebbie@kebbie"), Marketplace::Git);
        claude_plugin(&home, &claude, None, true);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "plugin update kebbie@kebbie\n");
        let left = claude_plugin(&home, &claude, None, false);
        assert!(left.contains("left as it is") && left.contains(IN_PLACE_FIX), "{left}");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "plugin update kebbie@kebbie\n", "a release machine never pulls the plugin from main");
    }

    #[test]
    fn a_folder_marketplace_at_the_install_root_is_refreshed_and_its_cached_copy_updated() {
        let dir = scratch("in-place");
        let (claude, log) = fake_claude(&dir);
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let home = claude_home(&dir, json!({ "source": "directory", "path": root }));
        assert_eq!(marketplace(&home, "kebbie@kebbie"), Marketplace::InPlace(root.clone()));
        assert!(claude_plugin(&home, &claude, Some(&dir), false).contains("which is not the install root"));
        assert!(!log.exists(), "claude was not run for a folder that is not the install root");
        claude_plugin(&home, &claude, Some(&root), false);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "plugin marketplace update kebbie\nplugin update kebbie@kebbie\n", "Claude Code runs a cached copy, so the folder marketplace is refreshed and the plugin updated");
    }

    #[test]
    fn update_lists_each_component_before_and_after() {
        let component = |name: &str, installed: Option<&str>| Component { name: name.to_string(), installed: installed.map(str::to_string), ..Component::default() };
        let before = [component("kebbie", Some("0.18.1")), component("vault", Some("0.2.16")), component("notes-vault-mcp", Some("0.6.0"))];
        let after = [component("kebbie", Some("0.18.2")), component("vault", Some("0.2.16")), component("notes-vault-mcp", Some("0.6.1"))];
        assert_eq!(versions(&before, &after, Channel::Release), ["kebbie 0.18.1 → 0.18.2", "vault plugin 0.2.16, already latest", "notes-vault-mcp 0.6.0 → 0.6.1"]);
        assert_eq!(versions(&before[..1], &after[..2], Channel::Release), ["kebbie 0.18.1 → 0.18.2", "vault plugin → 0.2.16"]);
        assert_eq!(versions(&[], &[component("kebbie", None)], Channel::Release), ["kebbie: version unknown"]);
        let built = |sha: &str| Component { built: Some(sha.to_string()), ..component("kebbie", Some("0.25.0")) };
        assert_eq!(versions(&[built("e70d5d5")], &[built("1234567")], Channel::Release), ["kebbie 0.25.0 rebuilt, e70d5d5 → 1234567"], "a rebuild within one version names both commits");
        assert_eq!(versions(&[built("e70d5d5")], &[built("e70d5d5")], Channel::Release), ["kebbie 0.25.0, already latest"]);
        assert_eq!(versions(&[built("02c7fe0")], &[built("9cbd545")], Channel::Main), ["kebbie main rebuilt, 02c7fe0 → 9cbd545"], "the main channel names no number");
        assert_eq!(versions(&[built("9cbd545")], &[built("9cbd545")], Channel::Main), ["kebbie main, already latest"]);
        let bumped = Component { built: Some("9cbd545".to_string()), ..component("kebbie", Some("0.26.0")) };
        assert_eq!(versions(&[built("02c7fe0")], &[bumped.clone(), component("vault", Some("0.2.16"))], Channel::Main), ["kebbie main rebuilt, 02c7fe0 → 9cbd545", "vault plugin → 0.2.16"], "a new number on main is still a rebuild, and the vault keeps its version");
        assert_eq!(versions(&[built("02c7fe0")], &[bumped], Channel::Release), ["kebbie 0.25.0 → 0.26.0"]);
    }


    fn said(lines: &[(&str, &str)]) -> Vec<crate::restart::Line> {
        lines.iter().map(|(label, said)| crate::restart::Line { label: label.to_string(), said: said.to_string(), stale: true, reasons: Vec::new() }).collect()
    }

    #[test]
    fn the_restarts_are_summed_up_in_a_sentence_or_two_and_only_what_needs_you_is_named() {
        let lines = said(&[("lead", "restarts when idle"), ("billing@app", "restarts when idle"), ("api@app", "you are in it: restarts when you leave it"), ("ghost", "skipped: its session id is unknown")]);
        assert_eq!(restart_summary(&lines), ["The lead and 2 sessions restart when they're idle"]);
        assert_eq!(restart_summary(&said(&[("lead", "restarts when idle")])), ["The lead restarts when it's idle"]);
        let manual = said(&[("lead", "stale: run kebbie restart when you want it"), ("billing@app", "stale: run kebbie restart when you want it")]);
        assert_eq!(restart_summary(&manual), ["The lead and 1 session keep the old kebbie until you restart them: restart.auto is off"]);
        let trouble = said(&[("billing@app", "restart failed: Claude did not come back"), ("worker@app", "waiting for your answer: background work running"), ("api@app", "stale, held: a restart onto kebbie 0.27.0 left it on kebbie 0.26.0; restarting again would not move it")]);
        assert_eq!(restart_summary(&trouble), ["1 session stays on the old kebbie, as a restart did not move it; d shows why", "1 session waits for your answer, as background work is still running", "Restarting 1 session failed; d shows why"]);
        assert_eq!(restart_summary(&[]), ["No Claude sessions are running"]);
        let on = said(&[("lead", "restarted"), ("billing@app", "up to date"), ("api@app", "restarted"), ("worker@app", "up to date"), ("docs@app", "restarts when idle")]);
        assert_eq!(restart_summary(&on), ["✓ The lead and 3 sessions are on the new kebbie", "1 session restarts when it's idle"]);
        assert_eq!(restart_summary(&said(&[("billing@app", "up to date")])), ["✓ 1 session is on the new kebbie"]);
    }

    #[test]
    fn the_terminal_names_the_board_and_a_board_names_none() {
        assert_eq!(board_summary("board: not running; it starts with the new kebbie when you open the lead").as_deref(), Some("Open boards restart on the new kebbie when you next open them"));
        assert_eq!(board_summary("board: restarted on the new kebbie").as_deref(), Some("The board restarted on the new kebbie"));
        assert_eq!(board_summary("board: up to date"), None);
        assert_eq!(board_summary("board: did not quit on ctrl-c; quit it and open the lead again").as_deref(), Some("The board did not restart; d shows why"));
    }

    #[test]
    fn the_sessions_follow_the_latest_restart_round_and_one_gone_from_it_runs_the_new_kebbie() {
        let update = said(&[("lead", "restarts when idle"), ("billing@app", "you are in it: restarts when you leave it"), ("api@app", "restarts when idle"), ("ghost", "skipped: its session id is unknown")]);
        assert_eq!(live_sessions(&update, None), update, "without a round since the update, the update's own word stands");
        let live = live_sessions(&update, Some(&["api@app: restarts when idle".to_string()]));
        assert_eq!(live.iter().map(|line| line.said.as_str()).collect::<Vec<_>>(), ["up to date", "up to date", "restarts when idle", "skipped: its session id is unknown"]);
        assert_eq!(restart_summary(&live), ["✓ The lead and 1 session are on the new kebbie", "1 session restarts when it's idle"]);
    }

    #[test]
    fn the_notes_come_from_the_commit_the_old_binary_was_built_from_before_the_old_head() {
        let kebbie = |built: Option<&str>| Component { name: update_check::KEBBIE.to_string(), installed: Some("0.27.0".to_string()), built: built.map(str::to_string), ..Component::default() };
        assert_eq!(old_commits(&[kebbie(Some("c634f7b"))], Some("f39045b")), ["c634f7b", "f39045b"]);
        assert_eq!(old_commits(&[kebbie(None)], Some("f39045b")), ["f39045b"]);
        assert!(old_commits(&[], None).is_empty());
    }

    #[test]
    fn the_terminal_summary_puts_the_notes_and_the_fixed_bugs_above_the_details() {
        let outcome = Outcome { summary: Summary { version: "0.28.0".to_string(), new: vec!["Name a device".to_string()], restart: vec!["The lead restarts when it's idle".to_string(), "Open boards restart on the new kebbie when you next open them".to_string()], ..Summary::default() }, details: vec!["kebbie 0.27.0 → 0.28.0".to_string()], started: 0 };
        assert_eq!(text(&outcome, &["update stopped on a newer config key".to_string()]), "✓ Updated kebbie 0.28.0\n\nWhat's new\n  • Name a device\nFixed\n  • A bug you ran into: update stopped on a newer config key\n\nRestarting\n  The lead restarts when it's idle\n  Open boards restart on the new kebbie when you next open them\n\nDetails\nkebbie 0.27.0 → 0.28.0");
        let bare = Outcome { summary: Summary::default(), details: Vec::new(), started: 0 };
        assert!(text(&bare, &[]).contains("No notes for this update"), "never the commit subjects");
    }

    #[test]
    fn the_fetch_step_says_a_local_checkout_is_built_with_the_commits_it_holds_ahead() {
        assert_eq!(fetch_label("main", Some(0), None, None), "Fetched main (no new commits)");
        assert_eq!(fetch_label("main", Some(0), None, Some(38)), "Fetched main (no new commits); building your local checkout, 38 commits ahead");
        assert_eq!(fetch_label("main", Some(3), None, Some(1)), "Fetched main (3 new commits); building your local checkout, 1 commit ahead");
        assert_eq!(fetch_label("main", Some(0), None, Some(0)), "Fetched main (no new commits); building your local checkout", "nothing about ahead at 0");
        assert_eq!(fetch_label("main", Some(0), Some("Uncommitted changes in the linked checkout, not pulled".to_string()), Some(2)), "Uncommitted changes in the linked checkout, not pulled; building your local checkout, 2 commits ahead");
    }

    #[test]
    fn a_linked_checkout_counts_its_commits_ahead_of_its_upstream() {
        let dir = scratch("ahead");
        let git = |cwd: &Path, args: &[&str]| {
            let output = std::process::Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "-c", "init.defaultBranch=main"]).args(args).current_dir(cwd).output().unwrap();
            assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&dir, &["init", "--quiet", "--bare", "origin.git"]);
        git(&dir, &["clone", "--quiet", "origin.git", "checkout"]);
        let checkout = dir.join("checkout");
        git(&checkout, &["commit", "--quiet", "--allow-empty", "-m", "first"]);
        git(&checkout, &["push", "--quiet", "origin", "HEAD:main"]);
        git(&checkout, &["branch", "--quiet", "--set-upstream-to=origin/main"]);
        let head = git(&checkout, &["rev-parse", "HEAD"]);
        assert_eq!(fetched(&checkout, Some(&head), Some("pulled to abc1234"), true, true), "Fetched main (no new commits); building your local checkout");
        git(&checkout, &["commit", "--quiet", "--allow-empty", "-m", "second"]);
        git(&checkout, &["commit", "--quiet", "--allow-empty", "-m", "third"]);
        let head = git(&checkout, &["rev-parse", "HEAD"]);
        assert_eq!(fetched(&checkout, Some(&head), Some("pulled to abc1234"), true, true), "Fetched main (no new commits); building your local checkout, 2 commits ahead");
        assert_eq!(fetched(&checkout, Some(&head), None, true, false), "Fetched main (no new commits)", "herdr's own clone says nothing about a local checkout");
    }

}
