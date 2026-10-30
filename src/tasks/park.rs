use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::messages;
use crate::notice::{Notice, Tone};
use crate::peers;
use crate::permit::Caller;
use crate::repo::{main_checkout, name_of};
use crate::sh::Result;
use super::pool::{find_task, task_of};

const DAY: i64 = 86_400;
const MORNING: &str = "09:00";
pub const UNPARKED: &str = "unparked";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parked {
    pub until: Option<u64>,
}

impl Parked {
    pub fn back(&self, now: u64, offset: &dyn Fn(i64) -> i64) -> String {
        match self.until {
            Some(at) => format!("back {}", crate::when::shown(at, now, offset)),
            None => "back when you unpark it".to_string(),
        }
    }

    pub fn back_now(&self) -> String {
        self.back(messages::now(), &crate::recover::local_offset)
    }
}

fn dir(state: &Path) -> PathBuf {
    state.join("parked")
}

fn file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(address)
}

// A park whose time has come is no park: the task is back in its group with nothing left to clear.
pub fn load(state: &Path, address: &str, now: u64) -> Option<Parked> {
    let record: Value = serde_json::from_str(&fs::read_to_string(file(state, address)).ok()?).ok()?;
    let until = record["until"].as_u64();
    if until.is_some_and(|at| at <= now) {
        return None;
    }
    Some(Parked { until })
}

pub fn all(state: &Path, now: u64) -> Vec<(String, Parked)> {
    let Ok(entries) = fs::read_dir(dir(state)) else { return Vec::new() };
    let mut found: Vec<(String, Parked)> = entries.filter_map(|entry| entry.ok()).map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|address| address.contains('@') && !address.starts_with('.'))
        .filter_map(|address| load(state, &address, now).map(|parked| (address, parked))).collect();
    found.sort_by(|one, other| one.0.cmp(&other.0));
    found
}

pub fn set(state: &Path, address: &str, until: Option<u64>, now: u64) -> Result<()> {
    crate::state::write(&file(state, address), json!({ "until": until, "at": now }).to_string())
}

pub fn clear(state: &Path, address: &str) -> Result<()> {
    match fs::remove_file(file(state, address)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {error}", file(state, address).display())),
        _ => Ok(()),
    }
}

pub fn forget(address: &str) {
    if let Ok(state) = crate::lead::state_dir() {
        let _ = clear(&state, address);
    }
}

pub fn morning_in(days: u32, now: u64, offset: &dyn Fn(i64) -> i64) -> Result<u64> {
    let today = (now as i64 + offset(now as i64)).div_euclid(DAY);
    let (year, month, date) = crate::when::civil(today + i64::from(days));
    crate::when::parse(&format!("{year:04}-{month:02}-{date:02} {MORNING}"), now, offset)
}

pub fn until(text: &str, now: u64, offset: &dyn Fn(i64) -> i64) -> Result<Option<u64>> {
    let text = text.trim();
    if text.eq_ignore_ascii_case(UNPARKED) {
        return Ok(None);
    }
    match crate::when::parse(text, now, offset) {
        Ok(at) => Ok(Some(at)),
        Err(error) => crate::when::parse(&format!("{text} {MORNING}"), now, offset).map(Some).map_err(|_| error),
    }
}

fn parker(caller: &Caller, what: &str) -> Result<()> {
    match caller {
        Caller::User | Caller::Lead => Ok(()),
        other => Err(format!("{what} a task is the user's call, or the lead's on their word; {} cannot", crate::permit::describe(other))),
    }
}

pub(crate) fn park_as(state: &Path, caller: &Caller, cwd: &Path, name: Option<&str>, when: Option<&str>, now: u64, offset: &dyn Fn(i64) -> i64) -> Result<String> {
    parker(caller, "parking")?;
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let address = peers::address(task_of(&slot), &name_of(&repo));
    let until = match when {
        Some(text) => until(text, now, offset).map_err(|error| format!("--until: {error}"))?,
        None => Some(morning_in(1, now, offset)?),
    };
    park_address(state, &address, until, now, offset)
}

pub fn park_address(state: &Path, address: &str, until: Option<u64>, now: u64, offset: &dyn Fn(i64) -> i64) -> Result<String> {
    set(state, address, until, now)?;
    let back = Parked { until }.back(now, offset);
    Notice::new(Tone::Done, format!("Parked, {back}"))
        .next("It keeps working; it leaves its group, the lead's summaries and your notifications until then, and the parked line on the board counts a question or red CI that comes up.")
        .detail("task", address).ok()
}

pub fn park(cwd: &Path, name: Option<&str>, when: Option<&str>) -> Result<String> {
    park_as(&crate::lead::state_dir()?, &crate::permit::caller(), cwd, name, when, messages::now(), &crate::recover::local_offset)
}

pub(crate) fn unpark_as(state: &Path, caller: &Caller, cwd: &Path, name: Option<&str>, now: u64) -> Result<String> {
    parker(caller, "unparking")?;
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    unpark_address(state, &peers::address(task_of(&slot), &name_of(&repo)), now)
}

pub fn unpark_address(state: &Path, address: &str, now: u64) -> Result<String> {
    if load(state, address, now).is_none() {
        clear(state, address)?;
        return Notice::new(Tone::NotNow, "Not parked").detail("task", address).err();
    }
    clear(state, address)?;
    Notice::new(Tone::Done, "Back from parking").next("It is in its group again.").detail("task", address).ok()
}

pub fn unpark(cwd: &Path, name: Option<&str>) -> Result<String> {
    unpark_as(&crate::lead::state_dir()?, &crate::permit::caller(), cwd, name, messages::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTC: fn(i64) -> i64 = |_| 0;
    // 2026-10-22, a Thursday, 12:00 UTC.
    const NOON: u64 = 1_792_670_400;

    #[test]
    fn a_park_reads_its_return_the_way_a_later_start_does() {
        let back = |text: &str| until(text, NOON, &UTC).map(|at| at.map(|at| crate::when::shown(at, NOON, &UTC)));
        assert_eq!(back("tomorrow 08:30").unwrap().as_deref(), Some("08:30 tomorrow"));
        assert_eq!(back("fri 09:00").unwrap().as_deref(), Some("09:00 tomorrow"));
        assert_eq!(back("in 2h").unwrap().as_deref(), Some("14:00 today"));
        assert_eq!(back("tomorrow").unwrap().as_deref(), Some("09:00 tomorrow"), "a day alone is its morning");
        assert_eq!(back("mon").unwrap().as_deref(), Some("Mon 26 Oct 09:00"));
        assert_eq!(back("24 oct").unwrap().as_deref(), Some("Sat 24 Oct 09:00"));
        assert_eq!(back("2026-11-02").unwrap().as_deref(), Some("Mon 2 Nov 09:00"));
        assert_eq!(back("unparked").unwrap(), None, "until the user brings it back");
        assert!(back("soonish").unwrap_err().contains("is not a time; use \"15:00\""), "the start time's own refusal");
        assert_eq!(crate::when::shown(morning_in(1, NOON, &UTC).unwrap(), NOON, &UTC), "09:00 tomorrow");
        assert_eq!(crate::when::shown(morning_in(3, NOON, &UTC).unwrap(), NOON, &UTC), "Sun 25 Oct 09:00");
    }

    #[test]
    fn a_park_is_kept_per_task_until_its_time_and_then_is_gone() {
        let state = crate::sh::test_dir("park-kept");
        let at = NOON + 3600;
        set(&state, "billing@web", Some(at), NOON).unwrap();
        set(&state, "search@web", None, NOON).unwrap();
        assert_eq!(load(&state, "billing@web", NOON), Some(Parked { until: Some(at) }), "read back as a new board would");
        assert_eq!(load(&state, "search@web", NOON + 400 * 86_400), Some(Parked { until: None }), "until unparked it never returns by itself");
        assert_eq!(load(&state, "billing@api", NOON), None, "another repo's half is its own task");
        assert_eq!(load(&state, "billing@web", at), None, "its time has come");
        assert_eq!(Parked { until: Some(at) }.back(NOON, &UTC), "back 13:00 today");
        assert_eq!(Parked { until: None }.back(NOON, &UTC), "back when you unpark it");
        assert!(unpark_address(&state, "search@web", NOON).unwrap().contains("Back from parking"));
        assert_eq!(load(&state, "search@web", NOON), None);
        assert!(unpark_address(&state, "search@web", NOON).unwrap_err().contains("Not parked"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn only_the_user_and_the_lead_park_a_task() {
        let state = crate::sh::test_dir("park-who");
        let refused = park_as(&state, &Caller::Task("billing@web".to_string()), &state, Some("billing"), None, NOON, &UTC).unwrap_err();
        assert!(refused.contains("the user's call, or the lead's") && refused.contains("the task billing@web"), "{refused}");
        assert!(unpark_as(&state, &Caller::Agent, &state, Some("billing"), NOON).unwrap_err().contains("unparking a task is the user's call"));
        fs::remove_dir_all(state).unwrap();
    }
}
