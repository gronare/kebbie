use std::time::{Duration, Instant};

const SNAPSHOT_LATE: Duration = Duration::from_secs(30);
const DRAW_LATE: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct Pulse {
    snapshot_at: Option<Instant>,
    snapshot_late: bool,
    waiting_since: Option<Instant>,
    draw_late: bool,
    rows: Option<Vec<String>>,
}

impl Pulse {
    pub(super) fn started(now: Instant) -> Self {
        Pulse { snapshot_at: Some(now), ..Default::default() }
    }

    pub(super) fn snapshot(&mut self, now: Instant) -> Option<String> {
        let gap = self.snapshot_at.replace(now).map(|at| now.duration_since(at));
        std::mem::take(&mut self.snapshot_late).then(|| format!("board: kebbie's state read again after {} s", gap.unwrap_or_default().as_secs()))
    }

    pub(super) fn waits(&mut self, now: Instant) {
        self.waiting_since.get_or_insert(now);
    }

    pub(super) fn drawn(&mut self, now: Instant, rows: Vec<String>) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(since) = self.waiting_since.take() && std::mem::take(&mut self.draw_late) {
            lines.push(format!("board: drew again after {} s", now.duration_since(since).as_secs()));
        }
        if let Some(before) = self.rows.replace(rows.clone()) && before != rows {
            let gone = before.iter().filter(|row| !rows.contains(row)).map(|row| format!("-{row}"));
            let came = rows.iter().filter(|row| !before.contains(row)).map(|row| format!("+{row}"));
            lines.push(format!("board: drew {}", gone.chain(came).collect::<Vec<_>>().join(" ")));
        }
        lines
    }

    pub(super) fn check(&mut self, now: Instant, gates: impl FnOnce() -> String) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.snapshot_late && self.snapshot_at.is_some_and(|at| now.duration_since(at) >= SNAPSHOT_LATE) {
            self.snapshot_late = true;
            lines.push(format!("board: kebbie's state not read for {} s", SNAPSHOT_LATE.as_secs()));
        }
        if !self.draw_late && self.waiting_since.is_some_and(|since| now.duration_since(since) >= DRAW_LATE) {
            self.draw_late = true;
            lines.push(format!("board: a change waits {} s for a draw ({})", DRAW_LATE.as_secs(), gates()));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pulse_logs_a_late_read_a_held_draw_and_each_change_to_the_drawn_rows_once() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut pulse = Pulse::started(start);
        assert_eq!(pulse.drawn(at(1), vec!["a@app ReadyToPush".to_string()]), Vec::<String>::new(), "the first frame is the baseline");
        assert_eq!(pulse.snapshot(at(3)), None);
        assert_eq!(pulse.check(at(20), String::new), Vec::<String>::new());
        assert_eq!(pulse.check(at(33), String::new), ["board: kebbie's state not read for 30 s"]);
        assert_eq!(pulse.check(at(40), String::new), Vec::<String>::new(), "said once");
        assert_eq!(pulse.snapshot(at(41)).as_deref(), Some("board: kebbie's state read again after 38 s"));
        pulse.waits(at(41));
        pulse.waits(at(43));
        assert_eq!(pulse.check(at(45), || "hold none".to_string()), Vec::<String>::new());
        assert_eq!(pulse.check(at(46), || "hold none".to_string()), ["board: a change waits 5 s for a draw (hold none)"]);
        assert_eq!(pulse.drawn(at(50), vec!["a@app ReadyToPush".to_string(), "b@app ReadyToPush".to_string()]), ["board: drew again after 9 s", "board: drew +b@app ReadyToPush"]);
        pulse.waits(at(51));
        assert_eq!(pulse.drawn(at(51), vec!["b@app ReadyToPush".to_string()]), ["board: drew -a@app ReadyToPush"], "a prompt draw is no news");
    }
}
