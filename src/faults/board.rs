use std::path::Path;

use super::{in_use, occurrences, tracked};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tally {
    pub fixing: usize,
    pub waiting: usize,
    pub in_use: usize,
    pub came_back: usize,
}

pub fn tally(state: &Path) -> Tally {
    let tracked = tracked(state);
    Tally {
        fixing: tracked.iter().filter(|issue| issue.is_open() && issue.fix.is_some()).count(),
        waiting: tracked.iter().filter(|issue| issue.is_open() && issue.fix.is_none()).count(),
        in_use: tracked.iter().filter(|issue| in_use(issue)).count(),
        came_back: tracked.iter().filter(|issue| occurrences(state, &issue.id).iter().any(|occurrence| occurrence.came_back)).count(),
    }
}

pub fn board_line_in(state: &Path) -> Option<String> {
    let tally = tally(state);
    if tally == Tally::default() {
        return None;
    }
    let waiting = if tally.waiting > 0 { format!(", {} waiting for a fix task", tally.waiting) } else { String::new() };
    let back = if tally.came_back > 0 { format!(", {} came back after", tally.came_back) } else { String::new() };
    Some(format!("Faults the fault watcher follows: {} being fixed{waiting}, {} fixed and in use{back} · kebbie faults lists them", tally.fixing, tally.in_use))
}

pub fn board_line() -> Option<String> {
    board_line_in(&crate::lead::state_dir().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faults::Occurrence;
    use crate::faults::Source;
    use crate::issues::{self, Issue};

    #[test]
    fn the_board_counts_faults_being_fixed_in_use_and_come_back() {
        let state = crate::sh::test_dir("faults-board");
        assert_eq!(board_line_in(&state), None, "no line before the watcher follows a fault");
        let dir = issues::dir(&state);
        let filed = |id: &str, status: &str, fix: Option<&str>, history: &[&str]| issues::save(&dir, &Issue { id: id.to_string(), title: id.to_string(), kind: "bug".to_string(), status: status.to_string(), by: crate::faults::BY.to_string(), fix: fix.map(str::to_string), history: history.iter().map(|line| line.to_string()).collect(), ..Issue::default() }).unwrap();
        filed("aa0001", "open", Some("fault-aa0001"), &[]);
        filed("aa0002", "fixed", None, &["fixed in 0.42.0 (abc1234), checked automatically"]);
        filed("aa0003", "open", Some("fault-aa0003-2"), &["fixed in 0.41.0 (abc1234), checked automatically", "came back on 0.42.0 (def5678)"]);
        filed("aa0004", "open", None, &[]);
        crate::faults::record_occurrence(&state, "aa0003", &Occurrence { at: 1, case: String::new(), source: Source::Incident, from: "red CI".to_string(), quote: "q".to_string(), came_back: true }).unwrap();
        assert_eq!(tally(&state), Tally { fixing: 2, waiting: 1, in_use: 1, came_back: 1 }, "an open fault no task fixes is not being fixed");
        assert_eq!(board_line_in(&state).unwrap(), "Faults the fault watcher follows: 2 being fixed, 1 waiting for a fix task, 1 fixed and in use, 1 came back after · kebbie faults lists them");
        std::fs::remove_dir_all(state).unwrap();
    }
}
