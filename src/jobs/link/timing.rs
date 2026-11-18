use std::sync::Mutex;
use std::time::Instant;

static LAST: Mutex<Option<(Instant, Instant)>> = Mutex::new(None);

fn on() -> bool {
    std::env::var_os("KEBBIE_LINK_TIMING").is_some_and(|value| !value.is_empty() && value != "0")
}

// With KEBBIE_LINK_TIMING=1, one stderr line per phase: the time since the last mark and since the first.
pub fn mark(phase: &str) {
    if !on() {
        return;
    }
    let now = Instant::now();
    let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (start, before) = last.unwrap_or((now, now));
    eprintln!("kebbie timing: {phase:<24} {:>7.1} ms  (at {:>7.1} ms)", (now - before).as_secs_f64() * 1000.0, (now - start).as_secs_f64() * 1000.0);
    *last = Some((start, now));
}
