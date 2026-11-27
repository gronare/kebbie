use super::*;
use std::fs;
use std::path::{Path, PathBuf};

const MODULES: [&str; 1] = ["faults"];

fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap().map(Result::unwrap) {
        let path = entry.path();
        if path.is_dir() {
            files.extend(sources(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files
}

fn calls(line: &str, module: &str) -> bool {
    let path = format!("{module}::");
    line.match_indices(&path).any(|(at, _)| !line[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') && !line[at + path.len()..].starts_with('{'))
}

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn nothing_but_the_dev_gate_reads_the_developer_flag() {
    let flags = [["fault_watcher", "_enabled"].concat(), ["fault_watcher", "_in("].concat()];
    let readers: Vec<PathBuf> = sources(&src()).into_iter().filter(|file| !file.starts_with(src().join("config")) && !file.starts_with(src().join("dev")) && file != &src().join("dev.rs")).filter(|file| flags.iter().any(|flag| fs::read_to_string(file).unwrap().contains(flag))).collect();
    assert!(readers.is_empty(), "a developer feature asks crate::dev::on(), never the flag itself: {readers:?}");
}

#[test]
fn every_command_that_runs_a_developer_module_is_a_developer_command() {
    let main = fs::read_to_string(src().join("main.rs")).unwrap();
    let (gate, dispatch) = (main.find("filter(|command| dev::hidden(command))").expect("run refuses a hidden command"), main.find("match arguments.positional(0) {").unwrap());
    assert!(gate < dispatch, "the refusal comes before the dispatch, so no arm of it runs a developer command ungated");
    let mut arm: Option<String> = None;
    let mut checked = Vec::new();
    for line in main.lines() {
        let indent = line.len() - line.trim_start().len();
        if indent == 8 {
            arm = line.trim_start().strip_prefix("Some(\"").and_then(|rest| rest.split('"').next()).map(str::to_string);
        }
        let Some(command) = &arm else { continue };
        if MODULES.iter().any(|module| calls(line, module)) {
            assert!(COMMANDS.contains(&command.as_str()), "kebbie {command} runs a developer module but is not in crate::dev::COMMANDS, so it would show on every install: {line}");
            checked.push(command.clone());
        }
    }
    assert!(COMMANDS.iter().all(|command| checked.iter().any(|seen| seen == command)), "the scan finds each developer command's arm: {checked:?}");
}

#[test]
fn every_keeper_routine_that_runs_a_developer_module_is_registered_only_with_the_gate_on() {
    let keeper = fs::read_to_string(src().join("keeper.rs")).unwrap();
    let routines: Vec<&str> = keeper.lines().map(str::trim_start).filter(|line| line.starts_with("Routine::")).collect();
    assert!(routines.len() > 10, "the scan finds the keeper's routines");
    for line in routines.iter().filter(|line| MODULES.iter().any(|module| line.contains(&format!("crate::{module}::")))) {
        assert!(line.starts_with("Routine::dev("), "a routine that runs a developer module is built with Routine::dev: {line}");
    }
    let names = |dev| crate::keeper::routines_for(dev).iter().map(|routine| (routine.name, routine.dev)).collect::<Vec<_>>();
    assert!(names(false).iter().all(|(_, dev)| !dev), "kebbie keeper --status lists no developer routine with the gate off: {:?}", names(false));
    assert!(names(true).iter().filter(|(_, dev)| *dev).count() >= 2, "the fault watcher's routines are developer routines");
    assert!(crate::keeper::routines().iter().all(|routine| !routine.dev), "a machine without [kebbie_dev], as the tests run, registers none");
}

#[test]
fn the_lead_is_given_no_developer_command_with_the_gate_off() {
    assert!(COMMANDS.iter().all(|command| !shows_command(command, false) && shows_command(command, true) && hidden(command)));
    assert!(calls("faults::report(&words)", "faults") && calls("crate::faults::watch::tick()", "faults") && !calls("config::defaults::faults()", "faults") && !calls("use faults::{a, b};", "faults"));
    assert!(!on(), "the tests run without [kebbie_dev]");
    let (off, on) = (crate::lead::settings_for("/opt/kebbie/kebbie", false), crate::lead::settings_for("/opt/kebbie/kebbie", true));
    for command in COMMANDS {
        let rule = format!("Bash(/opt/kebbie/kebbie {command}:*)");
        assert!(!off.contains(&rule) && on.contains(&rule), "{command}: {off}");
    }
    let role = crate::lead::role_text_with(include_str!("../../roles/lead.md"), "/opt/kebbie/kebbie", &[], false, crate::lead::FaultRoles { propose: true, ..Default::default() }, "English");
    assert!(COMMANDS.iter().all(|command| !role.contains(&format!("kebbie {command}"))) && !role.contains("fault watcher"), "the lead's role names no developer command where the gate is off");
}
