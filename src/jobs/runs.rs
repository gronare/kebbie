use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Def, TIMEOUT_MINUTES, MAX_TIMEOUT_MINUTES, repo_name, safe};
use crate::sh::Result;

const DIR: &str = "runs";
const KEPT: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub at: u64,
    pub command: String,
    #[serde(default)]
    pub on: Option<String>,
    pub machine: String,
    pub code: i32,
    pub seconds: u64,
}

fn file(state: &Path, repo: &str) -> PathBuf {
    state.join(DIR).join(format!("{}.jsonl", safe(repo)))
}

pub fn append(state: &Path, repo: &str, run: &Run) -> Result<()> {
    let _lock = crate::state::Locked::wait(&state.join(DIR).join(format!("{}.lock", safe(repo))))?;
    let path = file(state, repo);
    let mut lines: Vec<String> = std::fs::read_to_string(&path).unwrap_or_default().lines().filter(|line| !line.trim().is_empty()).map(str::to_string).collect();
    lines.push(serde_json::to_string(run).map_err(|error| error.to_string())?);
    let start = lines.len().saturating_sub(KEPT);
    crate::state::write(&path, format!("{}\n", lines[start..].join("\n")))
}

pub fn read(state: &Path, repo: &str) -> Vec<Run> {
    std::fs::read_to_string(file(state, repo)).unwrap_or_default().lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

// A run that ended with an exit code, kept for kebbie job suggest; a record that cannot be written never fails the run.
pub fn record(top: &Path, on: Option<&str>, machine: &str, command: &str, code: i32, took: Duration) {
    let Ok(state) = crate::lead::state_dir() else { return };
    let run = Run { at: crate::messages::now(), command: command.to_string(), on: on.map(str::to_string), machine: machine.to_string(), code, seconds: took.as_secs() };
    let _ = append(&state, &repo_name(top), &run);
}

fn normal(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn same_on(left: Option<&str>, right: Option<&str>) -> bool {
    left.map(str::to_ascii_lowercase) == right.map(str::to_ascii_lowercase)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Repeated {
    pub command: String,
    pub on: Option<String>,
    pub runs: usize,
    pub passed: usize,
    pub longest: u64,
    pub last: u64,
    pub machine: String,
    pub job: Option<String>,
}

impl Repeated {
    pub fn timeout_minutes(&self) -> Option<u64> {
        let minutes = (self.longest * 2).div_ceil(60).clamp(1, MAX_TIMEOUT_MINUTES);
        (minutes > TIMEOUT_MINUTES).then_some(minutes)
    }
}

// Runs of one command on one --on, most run first, each with the job already defined for it.
pub fn grouped(runs: &[Run], jobs: &BTreeMap<String, Def>) -> Vec<Repeated> {
    let mut groups: Vec<Repeated> = Vec::new();
    for run in runs {
        let command = normal(&run.command);
        let found = groups.iter_mut().find(|group| normal(&group.command) == command && same_on(group.on.as_deref(), run.on.as_deref()));
        match found {
            Some(group) => {
                group.runs += 1;
                group.passed += usize::from(run.code == 0);
                group.longest = group.longest.max(run.seconds);
                if run.at >= group.last {
                    (group.last, group.machine, group.command) = (run.at, run.machine.clone(), run.command.clone());
                }
            }
            None => groups.push(Repeated { command: run.command.clone(), on: run.on.clone(), runs: 1, passed: usize::from(run.code == 0), longest: run.seconds, last: run.at, machine: run.machine.clone(), job: None }),
        }
    }
    for group in &mut groups {
        group.job = jobs.iter().find(|(_, def)| normal(&def.run) == normal(&group.command) && same_on(def.on.as_deref(), group.on.as_deref())).map(|(name, _)| name.clone());
    }
    groups.sort_by(|left, right| right.runs.cmp(&left.runs).then(right.last.cmp(&left.last)));
    groups
}

fn took(seconds: u64) -> String {
    if seconds < 90 { format!("{seconds} s") } else { format!("{} min", seconds.div_ceil(60)) }
}

fn day(at: u64) -> String {
    let (year, month, date) = crate::when::civil((at / 86_400) as i64);
    format!("{year}-{month:02}-{date:02}")
}

pub fn shown(repo: &str, file: &Path, groups: &[Repeated], all: bool) -> String {
    let least = if all { 1 } else { 2 };
    let (listed, jobs): (Vec<&Repeated>, Vec<&Repeated>) = groups.iter().filter(|group| group.runs >= least).partition(|group| group.job.is_none());
    let mut lines = Vec::new();
    if listed.is_empty() {
        lines.push(format!("kebbie run --on has run no command {}in {repo} that is not a job yet", if all { "" } else { "twice " }));
    } else {
        lines.push(format!("Commands kebbie run --on ran {}in {repo}, most run first, from its last {KEPT} runs:", if all { "" } else { "more than once " }));
        for group in &listed {
            let on = group.on.as_deref().map_or_else(|| "on any worker".to_string(), |on| format!("on {on}"));
            let timeout = group.timeout_minutes().map(|minutes| format!("; as a job, timeout_minutes = {minutes}")).unwrap_or_default();
            lines.push(format!("\n{} run{} {on}, {} passed, longest {}, last {} on {}{timeout}\n  {}", group.runs, if group.runs == 1 { "" } else { "s" }, group.passed, took(group.longest), day(group.last), group.machine, group.command));
        }
        lines.push(format!("\nThey become jobs as [jobs.<name>] in {}; the skill kebbie:write-jobs writes and checks them.", file.display()));
    }
    if !jobs.is_empty() {
        lines.push(format!("Already jobs: {}", jobs.iter().filter_map(|group| group.job.as_deref()).collect::<Vec<_>>().join(", ")));
    }
    lines.join("\n")
}

pub fn suggest(cwd: &Path, all: bool) -> Result<String> {
    let state = crate::lead::state_dir()?;
    let top = PathBuf::from(crate::repo::git(cwd, &["rev-parse", "--show-toplevel"])?);
    let repo = repo_name(&top);
    let file = crate::config::repo_file(&top);
    let jobs = super::defined(&file)?;
    Ok(shown(&repo, &file, &grouped(&read(&state, &repo), &jobs), all))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-runs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn ran(at: u64, command: &str, on: Option<&str>, code: i32, seconds: u64) -> Run {
        Run { at, command: command.to_string(), on: on.map(str::to_string), machine: "studio".to_string(), code, seconds }
    }

    #[test]
    fn a_run_is_kept_per_repo_and_only_the_last_runs_stay() {
        let state = scratch("kept");
        append(&state, "app", &ran(1, "make", Some("macos"), 0, 3)).unwrap();
        append(&state, "other", &ran(2, "make", None, 1, 3)).unwrap();
        assert_eq!(read(&state, "app"), vec![ran(1, "make", Some("macos"), 0, 3)]);
        for at in 0..KEPT as u64 + 5 {
            append(&state, "app", &ran(at + 10, "make", None, 0, 1)).unwrap();
        }
        let kept = read(&state, "app");
        assert_eq!((kept.len(), kept[0].at, kept.last().map(|run| run.at)), (KEPT, 15, Some(KEPT as u64 + 14)), "the oldest runs go first");
        assert_eq!(read(&state, "other").len(), 1);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn runs_group_by_command_and_on_most_run_first_with_the_job_that_already_runs_them() {
        let runs = [
            ran(1, "xcodebuild -scheme App  build", Some("macos"), 1, 400),
            ran(2, "xcrun simctl boot X", Some("macos"), 0, 5),
            ran(3, "xcodebuild -scheme App build", Some("MacOS"), 0, 2000),
            ran(4, "xcodebuild -scheme App build", Some("linux"), 0, 10),
            ran(5, "xcrun simctl boot X", Some("macos"), 0, 6),
            ran(6, "xcodebuild -scheme App build", Some("macos"), 0, 300),
        ];
        let jobs = BTreeMap::from([("boot".to_string(), Def { run: "xcrun simctl  boot X".to_string(), on: Some("macos".to_string()), timeout_minutes: None })]);
        let groups = grouped(&runs, &jobs);
        let summary: Vec<_> = groups.iter().map(|group| (group.command.as_str(), group.on.as_deref(), group.runs, group.passed, group.longest, group.last, group.job.as_deref())).collect();
        assert_eq!(summary, [
            ("xcodebuild -scheme App build", Some("macos"), 3, 2, 2000, 6, None),
            ("xcrun simctl boot X", Some("macos"), 2, 2, 6, 5, Some("boot")),
            ("xcodebuild -scheme App build", Some("linux"), 1, 1, 10, 4, None),
        ]);
        assert_eq!((groups[0].timeout_minutes(), groups[1].timeout_minutes()), (Some(67), None), "twice the longest run, only past the default 60");
    }

    #[test]
    fn the_suggestion_lists_repeated_commands_that_are_no_job_yet() {
        let runs = [ran(86_400 * 20_775, "make test", Some("linux"), 0, 30), ran(86_400 * 20_775 + 9, "make test", Some("linux"), 0, 40), ran(1, "uname -a", None, 0, 1)];
        let groups = grouped(&runs, &BTreeMap::new());
        let text = shown("app", Path::new("/c/repos/app.toml"), &groups, false);
        assert!(text.contains("2 runs on linux, 2 passed, longest 40 s, last 2026-11-18 on studio\n  make test") && text.contains("[jobs.<name>] in /c/repos/app.toml") && !text.contains("uname"), "{text}");
        assert!(shown("app", Path::new("/x"), &groups, true).contains("1 run on any worker"), "--all lists a command run once");
        assert!(shown("app", Path::new("/x"), &[], false).starts_with("kebbie run --on has run no command twice in app"));
    }
}
