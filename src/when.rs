use crate::recover::{Clock, days_from_civil, reset_clock, resolve, weekday, year_of};
use crate::sh::Result;

const DAY: i64 = 86_400;
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

// A start time in the user's words: "15:00", "today 15:00", "tomorrow 9:00", "fri 09:00", "24 oct 09:00", "2026-10-24 09:00" or "in 2h"; a time alone is its next one.
pub fn parse(text: &str, now: u64, offset: &dyn Fn(i64) -> i64) -> Result<u64> {
    let refused = || format!("\"{text}\" is not a time; use \"15:00\", \"tomorrow 09:00\", \"fri 09:00\", \"24 oct 09:00\", \"2026-10-24 09:00\" or \"in 2h\"");
    let words: Vec<String> = text.trim().to_lowercase().split_whitespace().map(str::to_string).collect();
    if let [first, interval] = &words[..] && first == "in" {
        return crate::routines::parse_every(interval).map(|seconds| now + seconds).map_err(|_| refused());
    }
    let today = (now as i64 + offset(now as i64)).div_euclid(DAY);
    let day = match words.first().map(String::as_str) {
        Some("today") => Some(today),
        Some("tomorrow") => Some(today + 1),
        Some(word) => iso_day(word),
        None => return Err(refused()),
    };
    let at = match day {
        Some(day) => {
            let rest = words[1..].join(" ");
            let clock = reset_clock(&format!("resets {rest}")).filter(|clock| clock.month.is_none() && clock.weekday.is_none() && clock.year.is_none()).ok_or_else(refused)?;
            at_local(day, &clock, offset)
        }
        None => {
            let clock = reset_clock(&format!("resets {}", words.join(" "))).ok_or_else(refused)?;
            resolve(&Clock { zone: None, ..clock }, now, offset).and_then(|at| i64::try_from(at).ok()).ok_or_else(refused)?
        }
    };
    let at = u64::try_from(at).map_err(|_| refused())?;
    if at <= now {
        return Err(format!("\"{text}\" is {}, which has passed", shown(at, now, offset)));
    }
    Ok(at)
}

fn iso_day(word: &str) -> Option<i64> {
    let parts: Vec<&str> = word.split('-').collect();
    let [year, month, day] = parts[..] else { return None };
    let (year, month, day): (i64, u32, u32) = (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    ((1..=12).contains(&month) && (1..=31).contains(&day) && year >= 1970).then(|| days_from_civil(year, month, day))
}

fn at_local(day: i64, clock: &Clock, offset: &dyn Fn(i64) -> i64) -> i64 {
    let local = day * DAY + i64::from(clock.hour) * 3600 + i64::from(clock.minute) * 60;
    local - offset(local - offset(local))
}

// "15:00 today", "09:00 tomorrow", "Fri 24 Oct 09:00", with the year when it is not this one's.
pub fn shown(at: u64, now: u64, offset: &dyn Fn(i64) -> i64) -> String {
    let local = at as i64 + offset(at as i64);
    let (day, seconds) = (local.div_euclid(DAY), local.rem_euclid(DAY));
    let time = format!("{:02}:{:02}", seconds / 3600, seconds % 3600 / 60);
    let today = (now as i64 + offset(now as i64)).div_euclid(DAY);
    if day == today {
        return format!("{time} today");
    }
    if day == today + 1 {
        return format!("{time} tomorrow");
    }
    let (year, month, date) = civil(day);
    let year = if year == year_of(today) { String::new() } else { format!(" {year}") };
    format!("{} {date} {}{year} {time}", WEEKDAYS[weekday(day) as usize], MONTHS[month as usize - 1])
}

pub(crate) fn civil(day: i64) -> (i64, u32, u32) {
    let year = year_of(day);
    let mut month = 1;
    while month < 12 && days_from_civil(year, month + 1, 1) <= day {
        month += 1;
    }
    (year, month, (day - days_from_civil(year, month, 1) + 1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTC: fn(i64) -> i64 = |_| 0;
    const STOCKHOLM: fn(i64) -> i64 = |_| 7200;

    // 2026-10-22, a Thursday, 12:00 UTC.
    const NOON: u64 = 1_792_670_400;

    #[test]
    fn a_start_time_reads_in_the_users_words_and_comes_out_the_same_way() {
        assert_eq!(shown(NOON, NOON, &UTC), "12:00 today");
        let at = |text: &str| parse(text, NOON, &UTC).map(|at| shown(at, NOON, &UTC));
        assert_eq!(at("15:00").unwrap(), "15:00 today");
        assert_eq!(at("09:00").unwrap(), "09:00 tomorrow", "a time alone that has passed today is tomorrow's");
        assert_eq!(at("tomorrow 9:00").unwrap(), "09:00 tomorrow");
        assert_eq!(at("fri 09:00").unwrap(), "09:00 tomorrow");
        assert_eq!(at("sat 9am").unwrap(), "Sat 24 Oct 09:00");
        assert_eq!(at("24 oct 09:00").unwrap(), "Sat 24 Oct 09:00");
        assert_eq!(at("2026-11-02 08:30").unwrap(), "Mon 2 Nov 08:30");
        assert_eq!(at("2027-01-04 08:30").unwrap(), "Mon 4 Jan 2027 08:30");
        assert_eq!(at("in 2h").unwrap(), "14:00 today");
        assert_eq!(at("today 09:00").unwrap_err(), "\"today 09:00\" is 09:00 today, which has passed");
        assert!(at("soonish").unwrap_err().contains("is not a time"));
        assert!(at("today fri 09:00").is_err());
    }

    #[test]
    fn a_start_time_is_local_time() {
        let at = parse("15:00", NOON, &STOCKHOLM).unwrap();
        assert_eq!(at, NOON + 3600, "15:00 in UTC+2 is 13:00 UTC");
        assert_eq!(shown(at, NOON, &STOCKHOLM), "15:00 today");
    }
}
