use crate::routines::{WEEKDAYS, clock};
use crate::sh::Result;

const DAY: i64 = 86_400;

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    days: [bool; 7],
    start: u32,
    end: u32,
    utc: bool,
}

pub fn parse(text: &str) -> Result<Window> {
    let refused = || format!("\"{text}\" is not a deploy window; use hours, as \"08:00-17:00\", weekdays and hours, as \"mon-fri 08:00-17:00\", and UTC after them for UTC hours");
    let mut words: Vec<&str> = text.split_whitespace().collect();
    let utc = words.last().is_some_and(|word| word.eq_ignore_ascii_case("utc"));
    if utc {
        words.pop();
    }
    let (days, hours) = match words[..] {
        [hours] => ([true; 7], hours),
        [days, hours] => (weekdays(days).ok_or_else(refused)?, hours),
        _ => return Err(refused()),
    };
    let (start, end) = hours.split_once('-').and_then(|(start, end)| Some((clock(start)?, clock(end)?))).ok_or_else(refused)?;
    if start >= end {
        return Err(format!("\"{text}\": the deploy window must end after it opens, on the same day"));
    }
    Ok(Window { days, start, end, utc })
}

fn weekdays(text: &str) -> Option<[bool; 7]> {
    let day = |name: &str| WEEKDAYS.iter().position(|day| day.eq_ignore_ascii_case(name));
    let (first, last) = match text.split_once('-') {
        Some((first, last)) => (day(first)?, day(last)?),
        None => (day(text)?, day(text)?),
    };
    let mut days = [false; 7];
    let mut at = first;
    loop {
        days[at] = true;
        if at == last {
            return Some(days);
        }
        at = (at + 1) % 7;
    }
}

impl Window {
    pub fn open(&self, now: u64, offset: &dyn Fn(i64) -> i64) -> bool {
        let local = now as i64 + if self.utc { 0 } else { offset(now as i64) };
        let weekday = (local.div_euclid(DAY) + 3).rem_euclid(7) as usize;
        let minute = (local.rem_euclid(DAY) / 60) as u32;
        self.days[weekday] && (self.start..self.end).contains(&minute)
    }

    pub fn opens_between(&self, since: u64, now: u64, offset: &dyn Fn(i64) -> i64) -> bool {
        let shift = |at: i64| if self.utc { 0 } else { offset(at) };
        let first = (since as i64 + shift(since as i64)).div_euclid(DAY);
        let last = (now as i64 + shift(now as i64)).div_euclid(DAY);
        (first..=last).any(|day| {
            let local = day * DAY + i64::from(self.start) * 60;
            let at = local - shift(local - shift(local));
            at > since as i64 && at <= now as i64 && self.open(at as u64, offset)
        })
    }

    pub fn open_throughout(&self, since: u64, now: u64, offset: &dyn Fn(i64) -> i64) -> bool {
        self.open(since, offset) && self.open(now, offset) && !self.opens_between(since, now, offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONDAY: u64 = 1_795_392_000;
    const HOUR: u64 = 3600;

    #[test]
    fn a_window_reads_hours_weekdays_and_utc_and_refuses_the_rest() {
        assert_eq!(parse("08:00-17:00").unwrap(), Window { days: [true; 7], start: 480, end: 1020, utc: false });
        assert_eq!(parse("mon-fri 8:00-17:30 UTC").unwrap(), Window { days: [true, true, true, true, true, false, false], start: 480, end: 1050, utc: true });
        assert_eq!(parse("sat-mon 10:00-12:00").unwrap().days, [true, false, false, false, false, true, true], "a range may run over the week's end");
        assert_eq!(parse("wed 06:00-07:00").unwrap().days, [false, false, true, false, false, false, false]);
        for broken in ["", "08:00", "mon-fri", "mon-fri 08:00-17:00 CET", "funday 08:00-17:00", "08:00-25:00", "17:00-08:00", "a b c d"] {
            assert!(parse(broken).is_err(), "{broken}");
        }
    }

    #[test]
    fn a_window_is_open_on_its_days_and_hours_in_its_own_zone() {
        let utc = |_: i64| 0;
        let stockholm = |_: i64| 3600;
        let window = parse("mon-fri 08:00-17:00").unwrap();
        assert!(window.open(MONDAY + 8 * HOUR, &utc));
        assert!(!window.open(MONDAY + 17 * HOUR, &utc), "it shuts at its end");
        assert!(!window.open(MONDAY + 7 * HOUR, &utc));
        assert!(window.open(MONDAY + 7 * HOUR, &stockholm), "08:00 in Stockholm is 07:00 UTC");
        assert!(!window.open(MONDAY + 5 * 24 * HOUR + 10 * HOUR, &utc), "Saturday");
        assert!(!parse("mon-fri 08:00-17:00 UTC").unwrap().open(MONDAY + 7 * HOUR, &stockholm), "a UTC window ignores the machine's zone");
    }

    #[test]
    fn a_window_opens_between_two_moments_only_on_its_days_and_stays_open_only_within_one_opening() {
        let utc = |_: i64| 0;
        let window = parse("mon-fri 08:00-17:00").unwrap();
        let friday_evening = MONDAY + 4 * 24 * HOUR + 18 * HOUR;
        assert!(!window.opens_between(friday_evening, friday_evening + 24 * HOUR, &utc), "the weekend stays shut");
        assert!(window.opens_between(friday_evening, MONDAY + 7 * 24 * HOUR + 8 * HOUR, &utc), "Monday 08:00");
        assert!(window.opens_between(friday_evening, MONDAY + 7 * 24 * HOUR + 18 * HOUR, &utc), "Monday's window came and went while kebbie slept");
        assert!(!window.opens_between(MONDAY + 9 * HOUR, MONDAY + 16 * HOUR, &utc), "it opened before since");
        assert!(window.open_throughout(MONDAY + 9 * HOUR, MONDAY + 16 * HOUR, &utc));
        assert!(!window.open_throughout(MONDAY + 16 * HOUR, MONDAY + 24 * HOUR + 9 * HOUR, &utc), "it shut overnight");
        assert!(!window.open_throughout(MONDAY + 7 * HOUR, MONDAY + 9 * HOUR, &utc), "it was shut at since");
    }
}
