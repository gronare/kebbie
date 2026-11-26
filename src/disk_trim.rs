use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::disk::{Limit, Space};
use crate::phase::{Phase, Why};

pub const EVERY: u64 = 120;
const RECORD: &str = "disk.json";
const SHOWN_FOR: u64 = 10 * 60;
const AGENTS: [&str; 2] = ["claude", "codex"];
const SHELLS: [&str; 5] = ["bash", "zsh", "sh", "fish", "dash"];
const SANDBOXES: [&str; 2] = ["bwrap", "sandbox-exec"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    Free,
    WaitsForPush,
    Finished,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub path: PathBuf,
    pub repo: PathBuf,
    pub label: String,
    pub address: Option<String>,
    pub rank: Rank,
}

// A task is trimmed only when its Claude is known not to be working and its phase is known to be past the work; anything unread keeps it, as does a task that may go back to its work: Idle (a blocked report among them) and every needs-you but a report, a check or nothing to push.
pub fn rank_of(phase: Option<&Phase>, agent: Option<&str>) -> Option<Rank> {
    if !matches!(agent, Some("idle" | "done" | "blocked")) {
        return None;
    }
    match phase? {
        Phase::Starting | Phase::Working | Phase::Closing | Phase::Idle => None,
        Phase::Ready { .. } | Phase::Staged { .. } | Phase::Pushing { .. } | Phase::CiRunning { .. } => Some(Rank::WaitsForPush),
        Phase::CiRed { .. } => Some(Rank::Finished),
        Phase::NeedsYou(why) => match why {
            Why::Report | Why::Verify | Why::NothingToPush => Some(Rank::Finished),
            Why::Question | Why::Unreported | Why::Stopped | Why::Failed | Why::Unreadable | Why::LeftLine | Why::NotStaged | Why::Unseen | Why::Cancelled => None,
        },
    }
}

// Free worktrees first, then tasks waiting for a push, then finished ones; never one whose task builds or waits for a build slot.
pub fn order(seen: Vec<Seen>, building: impl Fn(&str) -> bool) -> Vec<Seen> {
    let mut kept: Vec<Seen> = seen.into_iter().filter(|seen| seen.address.as_deref().is_none_or(|address| !building(address))).collect();
    kept.sort_by_key(|seen| seen.rank);
    kept
}

pub fn building_in(queue: &crate::builds::Queue, address: &str) -> bool {
    queue.holders.iter().any(|holder| holder.address == address) || queue.turns.iter().any(|turn| turn.address == address) || queue.line.iter().any(|queued| queued == address)
}

struct Process {
    pid: u32,
    parent: u32,
    program: String,
    cwd_inside: bool,
    exe_inside: bool,
}

fn processes(path: &Path, procs: &Path) -> Option<Vec<Process>> {
    let entries = std::fs::read_dir(procs).ok()?;
    Some(entries.flatten().filter_map(|entry| {
        let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
        let stat = std::fs::read_to_string(entry.path().join("stat")).unwrap_or_default();
        let parent = stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(1)?.parse().ok()).unwrap_or(0);
        let inside = |link: &str| std::fs::read_link(entry.path().join(link)).is_ok_and(|target| target.starts_with(path));
        let program = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default().trim().to_string();
        Some(Process { pid, parent, program, cwd_inside: inside("cwd"), exe_inside: inside("exe") })
    }).collect())
}

// The task's agent, its shells, its sandbox and kebbie sit in its worktree, as do the tools the agent starts itself (its MCP servers) and their children; a command run through a shell, a dev server among them, is not one of these, even with a sandbox between the agent and the shell.
fn the_agents_own(process: &Process, all: &[Process]) -> bool {
    let program = process.program.as_str();
    if AGENTS.contains(&program) || SHELLS.contains(&program) || SANDBOXES.contains(&program) || program == "kebbie" {
        return true;
    }
    let (mut child, mut below_agent) = (process, process);
    for _ in 0..64 {
        let Some(parent) = all.iter().find(|other| other.pid == child.parent) else { return false };
        if AGENTS.contains(&parent.program.as_str()) {
            return !SHELLS.contains(&below_agent.program.as_str());
        }
        if !SANDBOXES.contains(&parent.program.as_str()) {
            below_agent = parent;
        }
        child = parent;
    }
    false
}

// The kernel gives cwd and exe as resolved paths, so the worktree is resolved too. A free worktree is in use by any process inside it; a task's by any process inside it but its agent's own, or a program run from it; an unreadable process table keeps it.
pub fn in_use(path: &Path, free: bool, procs: &Path) -> bool {
    let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let Some(all) = processes(&real, procs) else { return true };
    all.iter().any(|process| process.exe_inside || (process.cwd_inside && (free || !the_agents_own(process, &all))))
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub at: u64,
    pub line: Option<String>,
}

pub fn short_line(disk: &Path, space: Space, limit: Limit, trims: bool) -> String {
    let done = if trims { "after trimming the build output of every worktree nobody uses" } else { "and kebbie trims build output only on Linux" };
    format!("Disk under {}: {} free of {}, below {} (disk.trim_below), {done}; free space by hand", crate::disk::shown_path(disk), crate::disk::gb(space.free), crate::disk::gb(space.total), limit.shown())
}

// The line the board shows under Needs you, while a recent round left the disk short; an old record is a keeper that stopped, so it says nothing.
pub fn board_line_in(state: &Path, now: u64) -> Option<String> {
    let record: Record = serde_json::from_str(&std::fs::read_to_string(state.join(RECORD)).ok()?).ok()?;
    (now.saturating_sub(record.at) <= SHOWN_FOR).then_some(record.line).flatten()
}

pub fn board_line() -> Option<String> {
    board_line_in(&crate::lead::state_dir().ok()?, crate::messages::now())
}

fn seen_now(repos: &[PathBuf]) -> Vec<Seen> {
    let mut seen = Vec::new();
    for repo in repos {
        for slot in crate::tasks::slots(repo).unwrap_or_default().into_iter().filter(|slot| slot.free) {
            seen.push(Seen { label: crate::repo::name_of(&slot.path), path: slot.path, repo: repo.clone(), address: None, rank: Rank::Free });
        }
    }
    for item in crate::status::gather().unwrap_or_default() {
        let (Some(path), Some(repo), true) = (item.path.clone(), item.repo.clone(), item.is_task()) else { continue };
        let Some(rank) = rank_of(item.phase(), item.agent.as_ref().map(|agent| agent.status.as_str())) else { continue };
        let address = crate::peers::address(&item.name, &crate::repo::name_of(&repo));
        seen.push(Seen { label: format!("{} ({address})", crate::repo::name_of(&path)), path, repo, address: Some(address), rank });
    }
    seen
}

// What a round reads and does, so it runs against a made-up disk in tests: free space by path, the worktrees seen, whether a task builds now (read anew at each call), whether a worktree is in use, and the trim itself.
struct World<'a> {
    trims: bool,
    space: &'a dyn Fn(&Path) -> Option<Space>,
    disks: Vec<PathBuf>,
    seen: &'a dyn Fn() -> Vec<Seen>,
    building: &'a dyn Fn(&str) -> bool,
    in_use: &'a dyn Fn(&Seen) -> bool,
    trim: &'a dyn Fn(&Seen) -> Vec<(String, u64)>,
}

// Below disk.trim_below on any watched disk, the build cache's among them, it trims the worktrees on a disk below the limit in order, checking each again just before, until the disk is above the limit; the line is what the board shows when that was not enough.
fn round(limit: Limit, world: &World) -> (Vec<String>, Option<String>) {
    let short = || world.disks.iter().filter_map(|path| Some((path, (world.space)(path)?))).filter(|(_, space)| space.below(limit)).min_by_key(|(_, space)| space.free);
    let mut notes = Vec::new();
    if world.trims && short().is_some() {
        for seen in order((world.seen)(), world.building) {
            if !(world.space)(&seen.path).is_some_and(|space| space.below(limit)) {
                continue;
            }
            let still_quiet = || (world.seen)().iter().any(|now| now.path == seen.path);
            if seen.address.as_deref().is_some_and(world.building) || (world.in_use)(&seen) || !still_quiet() {
                continue;
            }
            let removed = (world.trim)(&seen);
            if !removed.is_empty() {
                let what: Vec<String> = removed.iter().map(|(folder, bytes)| format!("{folder}/ ({})", crate::disk::gb(*bytes))).collect();
                notes.push(format!("disk: trimmed {} in {}", what.join(", "), seen.label));
            }
        }
    }
    (notes, short().map(|(disk, space)| short_line(disk, space, limit, world.trims)))
}

fn record_in(state: &Path, line: Option<String>, now: u64) {
    if let Ok(text) = serde_json::to_string(&Record { at: now, line }) {
        let _ = crate::state::write(&state.join(RECORD), text);
    }
}

pub fn tick(state: &Path) -> Vec<String> {
    let limit = crate::config::disk_limits().map_or(crate::disk::DEFAULT_TRIM_BELOW, |(trim_below, _)| trim_below);
    let repos = crate::repo::pooled_checkouts();
    let builds = crate::builds::dir().ok();
    let building = |address: &str| building_in(&builds.as_deref().map(crate::builds::load).unwrap_or_default(), address);
    let trim = |seen: &Seen| {
        let folders = crate::config::resolve(Some(&seen.repo)).map(|loaded| loaded.settings.clean_on_release.value).unwrap_or_else(|_| vec!["target".to_string()]);
        crate::tasks::clean_build_output(&seen.path, &folders)
    };
    let world = World {
        trims: Path::new("/proc").is_dir(),
        space: &crate::disk::space,
        disks: repos.iter().cloned().chain(crate::disk::build_cache()).collect(),
        seen: &|| seen_now(&repos),
        building: &building,
        in_use: &|seen: &Seen| in_use(&seen.path, seen.rank == Rank::Free, Path::new("/proc")),
        trim: &trim,
    };
    let (notes, line) = round(limit, &world);
    record_in(state, line, crate::messages::now());
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(label: &str, address: Option<&str>, rank: Rank) -> Seen {
        Seen { path: PathBuf::from(format!("/p/{label}")), repo: PathBuf::from("/p/app"), label: label.to_string(), address: address.map(str::to_string), rank }
    }

    #[test]
    fn a_task_whose_claude_works_or_is_unread_or_whose_phase_is_the_work_is_never_trimmed() {
        let ready = Phase::Ready { head: None };
        assert_eq!(rank_of(Some(&ready), Some("working")), None);
        assert_eq!(rank_of(Some(&ready), None), None, "a Claude herdr does not show is not known to be quiet");
        assert_eq!(rank_of(Some(&ready), Some("unknown")), None, "a pane herdr gives no status is not known to be quiet");
        assert_eq!(rank_of(None, Some("idle")), None, "a task without a kept phase is not known to be past its work");
        for phase in [Phase::Starting, Phase::Working, Phase::Closing] {
            assert_eq!(rank_of(Some(&phase), Some("idle")), None, "{phase:?}");
        }
    }

    #[test]
    fn tasks_waiting_for_a_push_go_before_finished_ones() {
        for phase in [Phase::Ready { head: None }, Phase::Staged { head: None }, Phase::Pushing { ahead: 1 }, Phase::CiRunning { sha: "a".into() }] {
            assert_eq!(rank_of(Some(&phase), Some("done")), Some(Rank::WaitsForPush), "{phase:?}");
        }
        for phase in [Phase::NeedsYou(Why::Report), Phase::NeedsYou(Why::Verify), Phase::NeedsYou(Why::NothingToPush), Phase::CiRed { sha: "a".into() }] {
            assert_eq!(rank_of(Some(&phase), Some("idle")), Some(Rank::Finished), "{phase:?}");
        }
    }

    #[test]
    fn a_task_waiting_mid_work_on_an_answer_a_decision_or_a_fix_keeps_its_build() {
        assert_eq!(rank_of(Some(&Phase::Idle), Some("blocked")), None, "a blocked report waits on an obstacle's decision or the lead, mid-work");
        for why in Why::ALL.into_iter().filter(|why| !matches!(why, Why::Report | Why::Verify | Why::NothingToPush)) {
            assert_eq!(rank_of(Some(&Phase::NeedsYou(why)), Some("idle")), None, "{why:?}");
        }
    }

    #[test]
    fn free_worktrees_go_first_and_a_task_that_builds_or_waits_to_is_left_out() {
        let all = vec![seen("wt3", Some("done@app"), Rank::Finished), seen("wt2", Some("ready@app"), Rank::WaitsForPush), seen("wt4", Some("building@app"), Rank::WaitsForPush), seen("wt1", None, Rank::Free)];
        let picked: Vec<String> = order(all, |address| address == "building@app").into_iter().map(|seen| seen.label).collect();
        assert_eq!(picked, ["wt1", "wt2", "wt3"]);
    }

    #[test]
    fn a_task_holding_a_slot_holding_a_turn_or_in_line_counts_as_building() {
        let queue = crate::builds::Queue {
            holders: vec![crate::builds::Holder { address: "a@app".into(), id: "1".into(), at: 1 }],
            turns: vec![crate::builds::Turn { address: "b@app".into(), until: 9, told: true }],
            line: vec!["c@app".into()],
            ..Default::default()
        };
        assert!(["a@app", "b@app", "c@app"].iter().all(|address| building_in(&queue, address)));
        assert!(!building_in(&queue, "d@app"));
    }

    #[test]
    fn a_process_inside_a_tasks_worktree_keeps_it_unless_it_is_the_agents_own() {
        let root = crate::sh::test_dir("disk-trim-in-use");
        let worktree = root.join("wt1.app");
        let procs = root.join("proc");
        std::fs::create_dir_all(worktree.join("web")).unwrap();
        let process = |pid: u32, parent: u32, comm: &str, cwd: &Path, exe: &Path| {
            let dir = procs.join(pid.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
            std::fs::write(dir.join("stat"), format!("{pid} ({comm}) S {parent} {pid} {pid} 0 -1")).unwrap();
            std::os::unix::fs::symlink(cwd, dir.join("cwd")).unwrap();
            std::os::unix::fs::symlink(exe, dir.join("exe")).unwrap();
        };
        let gone = |pid: u32| { let _ = std::fs::remove_dir_all(procs.join(pid.to_string())); };
        process(10, 1, "claude", &worktree, Path::new("/usr/bin/claude"));
        process(11, 1, "zsh", &worktree, Path::new("/usr/bin/zsh"));
        process(12, 10, "uv", &worktree, Path::new("/usr/bin/uv"));
        process(13, 12, "python", &worktree, Path::new("/usr/bin/python3"));
        process(14, 10, "bash", &worktree, Path::new("/usr/bin/bash"));
        process(15, 1, "kebbie", &worktree, Path::new("/usr/bin/kebbie"));
        assert!(!in_use(&worktree, false, &procs), "the agent, its shells, its MCP servers and kebbie sit in its worktree");
        assert!(in_use(&worktree, true, &procs), "a free worktree with anyone inside is in use");
        process(16, 14, "pnpm", &worktree.join("web"), Path::new("/usr/bin/node"));
        assert!(in_use(&worktree, false, &procs), "a dev server the agent ran through its shell");
        gone(16);
        process(20, 10, "bwrap", &worktree, Path::new("/usr/bin/bwrap"));
        process(21, 20, "bwrap", &worktree, Path::new("/usr/bin/bwrap"));
        process(22, 21, "bash", &worktree, Path::new("/usr/bin/bash"));
        assert!(!in_use(&worktree, false, &procs), "the sandbox and the shell in it are the agent's own");
        process(23, 22, "node", &worktree.join("web"), Path::new("/usr/bin/node"));
        assert!(in_use(&worktree, false, &procs), "a dev server run through the sandboxed shell");
        gone(23);
        process(17, 11, "node", &worktree.join("web"), Path::new("/usr/bin/node"));
        assert!(in_use(&worktree, false, &procs), "a dev server started in the task's pane");
        gone(17);
        process(18, 1, "cargo", &worktree, Path::new("/usr/bin/cargo"));
        assert!(in_use(&worktree, false, &procs), "a build started anywhere else");
        let link = root.join("wt1-link");
        std::os::unix::fs::symlink(&worktree, &link).unwrap();
        assert!(in_use(&link, false, &procs), "the kernel names the real folder, so a worktree reached through a link is resolved first");
        gone(18);
        process(19, 1, "server", Path::new("/"), &worktree.join("target/debug/server"));
        assert!(in_use(&worktree, false, &procs), "a program run from the worktree's build holds it");
        gone(19);
        assert!(!in_use(&worktree, false, &procs));
        assert!(in_use(&worktree, false, &root.join("no-proc")), "an unreadable process table keeps every worktree");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_line_names_the_free_space_the_disk_and_the_limit() {
        let line = short_line(Path::new("/p/app"), Space { free: 12_300_000_000, total: 242_000_000_000 }, Limit::Gb(30), true);
        assert_eq!(line, "Disk under /p/app: 12.3 GB free of 242.0 GB, below 30 GB (disk.trim_below), after trimming the build output of every worktree nobody uses; free space by hand");
    }

    #[test]
    fn the_board_shows_the_line_only_while_the_round_that_wrote_it_is_recent() {
        let state = crate::sh::test_dir("disk-trim-record");
        let line = short_line(Path::new("/p/app"), Space { free: 12_300_000_000, total: 242_000_000_000 }, Limit::Gb(30), true);
        record_in(&state, Some(line.clone()), 1000);
        assert_eq!(board_line_in(&state, 1000 + SHOWN_FOR), Some(line));
        assert_eq!(board_line_in(&state, 1001 + SHOWN_FOR), None, "a stopped keeper's warning goes");
        record_in(&state, None, 1000);
        assert_eq!(board_line_in(&state, 1000), None, "a round that freed enough clears the line");
        let _ = std::fs::remove_dir_all(state);
    }

    const GB: u64 = 1_000_000_000;

    struct Disk {
        free: std::cell::Cell<u64>,
        trimmed: std::cell::RefCell<Vec<String>>,
        building: std::cell::RefCell<Vec<String>>,
        building_reads: std::cell::Cell<usize>,
        seen_reads: std::cell::Cell<usize>,
    }

    fn disk(free: u64) -> Disk {
        Disk { free: std::cell::Cell::new(free), trimmed: Default::default(), building: Default::default(), building_reads: Default::default(), seen_reads: Default::default() }
    }

    // One disk under every worktree, 10 GB freed by each trim; a trim of wt1 starts a build in b@app, as a task may between two worktrees.
    fn run(disk: &Disk, in_use: &[&str]) -> (Vec<String>, Option<String>) {
        let space = |_: &Path| Some(Space { free: disk.free.get(), total: 200 * GB });
        let seen = || {
            disk.seen_reads.set(disk.seen_reads.get() + 1);
            vec![seen("wt3", Some("c@app"), Rank::Finished), seen("wt2", Some("b@app"), Rank::WaitsForPush), seen("wt1", None, Rank::Free), seen("wt4", Some("d@app"), Rank::Finished)]
        };
        let building = |address: &str| {
            disk.building_reads.set(disk.building_reads.get() + 1);
            disk.building.borrow().iter().any(|busy| busy == address)
        };
        let busy = |seen: &Seen| in_use.contains(&seen.label.as_str());
        let trim = |seen: &Seen| {
            disk.trimmed.borrow_mut().push(seen.label.clone());
            disk.free.set(disk.free.get() + 10 * GB);
            if seen.label == "wt1" {
                disk.building.borrow_mut().push("b@app".to_string());
            }
            vec![("target".to_string(), 10 * GB)]
        };
        let world = World { trims: true, space: &space, disks: vec![PathBuf::from("/p/app")], seen: &seen, building: &building, in_use: &busy, trim: &trim };
        round(Limit::Gb(30), &world)
    }

    #[test]
    fn a_disk_above_the_limit_is_left_alone_and_shows_no_line() {
        let disk = disk(31 * GB);
        assert_eq!(run(&disk, &[]), (Vec::new(), None));
        assert_eq!(disk.seen_reads.get(), 0, "the worktrees are not even listed");
        assert!(disk.trimmed.borrow().is_empty());
    }

    #[test]
    fn trimming_goes_in_order_and_stops_once_the_disk_is_above_the_limit() {
        let disk = disk(5 * GB);
        let (notes, line) = run(&disk, &[]);
        assert_eq!(*disk.trimmed.borrow(), ["wt1", "wt3", "wt4"], "b@app started building after wt1 was trimmed, so wt2 is skipped");
        assert_eq!(notes[0], "disk: trimmed target/ (10.0 GB) in wt1");
        assert_eq!(line, None, "35 GB free is enough");
    }

    #[test]
    fn the_queue_and_the_processes_are_read_again_before_each_trim() {
        let disk = disk(25 * GB);
        disk.building.borrow_mut().push("c@app".to_string());
        let (_, line) = run(&disk, &["wt1"]);
        assert!(disk.trimmed.borrow().as_slice() == ["wt2"], "wt1 is in use and c@app builds: {:?}", disk.trimmed.borrow());
        assert_eq!(disk.building_reads.get(), 4, "three reads for the order, and wt2's again just before its trim");
        assert_eq!(line, None);
    }

    #[test]
    fn a_disk_still_short_after_every_trim_shows_the_line() {
        let disk = disk(GB);
        let (notes, line) = run(&disk, &["wt3", "wt4"]);
        assert_eq!(*disk.trimmed.borrow(), ["wt1"]);
        assert_eq!(notes.len(), 1);
        assert_eq!(line, Some(short_line(Path::new("/p/app"), Space { free: 11 * GB, total: 200 * GB }, Limit::Gb(30), true)));
    }

    #[test]
    fn a_low_build_cache_disk_trims_no_worktree_on_another_disk_and_shows_its_line() {
        let trimmed = std::cell::RefCell::new(Vec::new());
        let space = |path: &Path| Some(Space { free: if path.starts_with("/cache") { 5 * GB } else { 100 * GB }, total: 200 * GB });
        let seen = || vec![seen("wt1", None, Rank::Free), seen("wt2", Some("b@app"), Rank::Finished)];
        let trim = |seen: &Seen| {
            trimmed.borrow_mut().push(seen.label.clone());
            Vec::new()
        };
        let world = World { trims: true, space: &space, disks: vec![PathBuf::from("/p/app"), PathBuf::from("/cache/sccache")], seen: &seen, building: &|_| false, in_use: &|_| false, trim: &trim };
        let (notes, line) = round(Limit::Gb(30), &world);
        assert!(trimmed.borrow().is_empty() && notes.is_empty(), "the worktrees' own disk has room");
        assert_eq!(line, Some(short_line(Path::new("/cache/sccache"), Space { free: 5 * GB, total: 200 * GB }, Limit::Gb(30), true)));
        assert!(line.unwrap().starts_with("Disk under /cache/sccache: 5.0 GB free"));
    }

    #[test]
    fn a_share_of_the_disk_is_checked_on_each_disk_by_its_own_size() {
        let trimmed = std::cell::RefCell::new(Vec::new());
        let space = |path: &Path| Some(if path.starts_with("/big") { Space { free: 100 * GB, total: 1000 * GB } } else { Space { free: 50 * GB, total: 100 * GB } });
        let seen = || vec![Seen { path: PathBuf::from("/big/wt1"), ..seen("wt1", None, Rank::Free) }];
        let trim = |seen: &Seen| {
            trimmed.borrow_mut().push(seen.label.clone());
            Vec::new()
        };
        let world = World { trims: true, space: &space, disks: vec![PathBuf::from("/big/app"), PathBuf::from("/small/app")], seen: &seen, building: &|_| false, in_use: &|_| false, trim: &trim };
        let (_, line) = round(Limit::Percent(15), &world);
        assert_eq!(*trimmed.borrow(), ["wt1"], "10% free on the big disk is below 15%, though the small disk has fewer free bytes");
        assert_eq!(line, Some(short_line(Path::new("/big/app"), Space { free: 100 * GB, total: 1000 * GB }, Limit::Percent(15), true)));
        let (_, line) = round(Limit::Percent(5), &world);
        assert_eq!(line, None, "neither disk is below 5%");
    }

    #[test]
    fn without_a_process_table_nothing_is_trimmed_and_the_line_says_so() {
        let trimmed = std::cell::Cell::new(false);
        let space = |_: &Path| Some(Space { free: 5 * GB, total: 200 * GB });
        let trim = |_: &Seen| {
            trimmed.set(true);
            Vec::new()
        };
        let world = World { trims: false, space: &space, disks: vec![PathBuf::from("/p/app")], seen: &|| vec![seen("wt1", None, Rank::Free)], building: &|_| false, in_use: &|_| false, trim: &trim };
        let (notes, line) = round(Limit::Gb(30), &world);
        assert!(!trimmed.get() && notes.is_empty());
        let line = line.unwrap();
        assert!(line.contains("trims build output only on Linux") && !line.contains("after trimming"), "{line}");
    }

    #[test]
    fn a_task_that_starts_working_during_the_round_is_left_alone() {
        let trimmed = std::cell::RefCell::new(Vec::new());
        let free = std::cell::Cell::new(5 * GB);
        let space = |_: &Path| Some(Space { free: free.get(), total: 200 * GB });
        let seen = || {
            let all = vec![seen("wt1", None, Rank::Free), seen("wt2", Some("b@app"), Rank::Finished)];
            all.into_iter().filter(|seen| seen.label != "wt2" || trimmed.borrow().is_empty()).collect()
        };
        let trim = |seen: &Seen| {
            trimmed.borrow_mut().push(seen.label.clone());
            free.set(free.get() + 10 * GB);
            Vec::new()
        };
        let world = World { trims: true, space: &space, disks: vec![PathBuf::from("/p/app")], seen: &seen, building: &|_| false, in_use: &|_| false, trim: &trim };
        round(Limit::Gb(30), &world);
        assert_eq!(*trimmed.borrow(), ["wt1"], "b@app's Claude started working once wt1 was trimmed, so its status read just before keeps wt2");
    }
}
