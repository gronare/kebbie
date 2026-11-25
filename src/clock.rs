use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::doctor::{Check, Level};

pub const FILE: &str = "clock.json";
pub const EVERY: u64 = 300;
pub const OFF_BY: u64 = 60;

// offset is the hub's time minus this machine's: positive when the machine runs behind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub at: u64,
    pub offset: i64,
}

pub fn fix() -> &'static str {
    if cfg!(target_os = "macos") { "sudo sntp -sS time.apple.com" } else { "sudo chronyc makestep" }
}

fn off(offset: i64) -> String {
    let way = if offset > 0 { "behind" } else { "ahead of" };
    format!("{} {way}", crate::keeper::span(offset.unsigned_abs()))
}

// The protocol refuses a command signed more than CLOCK_SKEW ahead of this machine, and one past its expiry by this machine's clock.
fn risk(offset: i64) -> String {
    let skew = kebbie_hub_protocol::CLOCK_SKEW;
    match offset {
        behind if behind.unsigned_abs() > skew && behind > 0 => "so the phone's commands are refused as expired".to_string(),
        behind if behind > 0 => format!("and the phone's commands are refused as expired once it is more than {} behind", crate::keeper::span(skew)),
        ahead => format!("so the phone's commands expire {} early", crate::keeper::span(ahead.unsigned_abs())),
    }
}

pub fn line(offset: i64) -> Option<String> {
    (offset.unsigned_abs() > OFF_BY).then(|| format!("Clock: this machine runs {} the hub, {}; set the clock, for example with {}", off(offset), risk(offset), fix()))
}

// A command signed further ahead than the protocol's skew allows was refused only because this machine's clock is behind.
pub fn refused(sent_at: u64, now: u64) -> Option<String> {
    (sent_at > now + kebbie_hub_protocol::CLOCK_SKEW).then(|| format!("Not done: the machine's clock runs {} behind, so this looked expired. Set its clock, for example with {}, then send it again.", crate::keeper::span(sent_at - now), fix()))
}

// An HTTP Date in its one current form, as in "Wed, 25 Nov 2026 18:12:50 GMT".
pub fn http_date(text: &str) -> Option<u64> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [_, day, month, year, time, "GMT"] = parts.as_slice() else { return None };
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|name| name == month)? as u32 + 1;
    let (day, year): (u32, i64) = (day.parse().ok()?, year.parse().ok()?);
    let clock: Vec<u64> = time.split(':').map(str::parse).collect::<Result<_, _>>().ok()?;
    let [hours, minutes, seconds] = clock.as_slice() else { return None };
    if !(1..=31).contains(&day) || *hours > 23 || *minutes > 59 || *seconds > 60 {
        return None;
    }
    let days = u64::try_from(crate::recover::days_from_civil(year, month, day)).ok()?;
    Some(days * 86_400 + hours * 3600 + minutes * 60 + seconds)
}

pub fn measure(url: &str, now: impl Fn() -> u64) -> Result<i64, String> {
    let date = crate::hub::http::Client::new(url, None, std::time::Duration::from_secs(5)).date().map_err(|error| error.to_string())?;
    let date = date.ok_or("the hub's answer carries no Date")?;
    let hub = http_date(&date).ok_or_else(|| format!("the hub's Date {date:?} did not parse"))?;
    Ok(hub as i64 - now() as i64)
}

pub fn read(state: &Path) -> Option<Reading> {
    serde_json::from_str(&std::fs::read_to_string(state.join(FILE)).ok()?).ok()
}

// The keeper's round: measured against the hub this machine posts to, and forgotten when it posts to none.
pub fn tick_in(state: &Path, now: u64, measure: impl FnOnce(&str) -> Result<i64, String>) -> crate::sh::Result<Vec<String>> {
    let Some(identity) = crate::hub::identity::load(state)? else {
        let _ = std::fs::remove_file(state.join(FILE));
        return Ok(Vec::new());
    };
    let offset = measure(&identity.url)?;
    let was = read(state).and_then(|reading| line(reading.offset));
    crate::state::write(&state.join(FILE), serde_json::to_string(&Reading { at: now, offset }).map_err(|error| error.to_string())?)?;
    let said = line(offset);
    Ok(if said.is_some() && was.is_none() { said.into_iter().collect() } else { Vec::new() })
}

pub fn tick(state: &Path) -> crate::sh::Result<Vec<String>> {
    tick_in(state, crate::messages::now(), |url| measure(url, crate::messages::now))
}

pub fn doctor(url: Option<&str>, measure: impl FnOnce(&str) -> Result<i64, String>) -> Option<Check> {
    let url = url?;
    Some(match measure(url) {
        Ok(offset) if offset.unsigned_abs() > OFF_BY => Check::new(Level::Warn, "clock", format!("this machine runs {} the hub, {}", off(offset), risk(offset)), vec![format!("set the clock, for example with {}", fix())]).brief(format!("clock {}", off(offset))),
        Ok(offset) => Check::new(Level::Ok, "clock", format!("in step with the hub ({}s apart)", offset.unsigned_abs()), Vec::new()),
        Err(error) => Check::new(Level::Skip, "clock", format!("not compared with the hub: {error}"), Vec::new()).brief("clock not checked"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_http_date_reads_as_unix_seconds_and_anything_else_does_not() {
        assert_eq!(http_date("Wed, 25 Nov 2026 18:12:50 GMT"), Some(1_795_630_370));
        assert_eq!(http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        for bad in ["", "Wed, 25 Nov 2026 18:12:50 CET", "Wed, 25 Foo 2026 18:12:50 GMT", "Wed, 25 Nov 2026 25:00:00 GMT", "Wednesday, 25-Nov-26 18:12:50 GMT"] {
            assert_eq!(http_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_command_from_hours_ahead_says_the_machines_clock_is_behind_and_how_to_set_it() {
        let now = 1_795_607_054;
        let said = refused(now + 23_316, now).expect("6.5 hours ahead is the machine's clock");
        assert!(said.contains("clock runs 6h 28m behind") && said.contains(fix()) && said.ends_with("then send it again."), "{said}");
        assert_eq!(refused(now + kebbie_hub_protocol::CLOCK_SKEW, now), None, "within the protocol's skew the command really expired");
        assert_eq!(refused(now - 3600, now), None);
    }

    #[test]
    fn the_line_needs_more_than_a_minute_apart() {
        assert_eq!(line(OFF_BY as i64), None);
        assert_eq!(line(-(OFF_BY as i64)), None);
        let behind = line(23_316).unwrap();
        assert!(behind.starts_with("Clock: this machine runs 6h 28m behind the hub") && behind.ends_with(fix()), "{behind}");
        assert!(line(-120).unwrap().contains("runs 2m ahead of the hub, so the phone's commands expire 2m early;"), "{:?}", line(-120));
        assert!(line(120).unwrap().contains("runs 2m behind the hub, and the phone's commands are refused as expired once it is more than 5m behind;"), "{:?}", line(120));
        assert!(behind.contains("behind the hub, so the phone's commands are refused as expired;"), "{behind}");
    }

    #[test]
    fn the_keeper_measures_only_a_connected_machine_and_logs_the_moment_it_goes_wrong() {
        let state = crate::sh::test_dir("clock-tick");
        std::fs::write(state.join(FILE), r#"{"at":1,"offset":23316}"#).unwrap();
        assert_eq!(tick_in(&state, 10, |_| panic!("no hub, no measure")), Ok(Vec::new()));
        assert!(!state.join(FILE).exists(), "a machine that left the hub keeps no old reading");
        let machine = kebbie_hub_protocol::Id::new(kebbie_hub_protocol::Prefix::Machine).unwrap();
        crate::hub::identity::save(&state, &crate::hub::identity::Identity::new("https://hub.example.com", machine, "devbox", "khm_x".to_string(), &kebbie_hub_protocol::SecretKeys::generate(), 1)).unwrap();
        let notes = tick_in(&state, 10, |_| Ok(23_316)).unwrap();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(read(&state), Some(Reading { at: 10, offset: 23_316 }));
        assert_eq!(tick_in(&state, 20, |_| Ok(23_310)), Ok(Vec::new()), "said once, not every round");
        assert_eq!(tick_in(&state, 30, |_| Err("down".to_string())), Err("down".to_string()));
        assert_eq!(read(&state).map(|reading| reading.at), Some(20), "a failed round keeps the last reading");
    }

    #[test]
    fn doctor_shows_the_clock_only_with_a_hub_and_warns_past_a_minute() {
        assert_eq!(doctor(None, |_| panic!("no hub")), None);
        let wrong = doctor(Some("https://hub.example.com"), |_| Ok(23_316)).unwrap();
        assert!(wrong.level == Level::Warn && wrong.fact.contains("6h 28m behind the hub") && wrong.fix[0].contains(fix()), "{wrong:?}");
        let fine = doctor(Some("https://hub.example.com"), |_| Ok(-2)).unwrap();
        assert_eq!((fine.level, fine.fact.as_str()), (Level::Ok, "in step with the hub (2s apart)"));
        assert!(wrong.fact.ends_with("so the phone's commands are refused as expired"), "{wrong:?}");
        let drifting = doctor(Some("https://hub.example.com"), |_| Ok(120)).unwrap();
        assert!(drifting.level == Level::Warn && drifting.fact.ends_with("refused as expired once it is more than 5m behind"), "two minutes behind is still taken: {drifting:?}");
        let unanswered = doctor(Some("https://hub.example.com"), |_| Err("down".to_string())).unwrap();
        assert_eq!((unanswered.level, unanswered.fact.as_str()), (Level::Skip, "not compared with the hub: down"), "an unanswered hub is not checked, not a warning without a fix");
    }
}
