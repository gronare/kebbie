use std::sync::mpsc::{self, Receiver};
use std::thread;

// Work a page runs behind it: a line for each while it runs, and outcomes that wait until the page can show them.
pub struct Runs<T> {
    running: Vec<(String, Receiver<T>)>,
    done: Vec<T>,
}

impl<T> Default for Runs<T> {
    fn default() -> Self {
        Runs { running: Vec::new(), done: Vec::new() }
    }
}

impl<T: Send + 'static> Runs<T> {
    pub fn start(&mut self, label: String, work: impl FnOnce() -> T + Send + 'static) {
        let (sender, result) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(work());
        });
        self.running.push((label.trim_end_matches('…').to_string(), result));
    }

    pub fn labels(&self) -> Vec<String> {
        self.running.iter().map(|(label, _)| format!("{label}…")).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.running.is_empty() && self.done.is_empty()
    }

    // Gathers what finished; hands the oldest outcome over only once the page is ready to show it.
    pub fn next(&mut self, ready: bool) -> Option<T> {
        self.running.retain(|(_, result)| match result.try_recv() {
            Ok(outcome) => {
                self.done.push(outcome);
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => false,
        });
        (ready && !self.done.is_empty()).then(|| self.done.remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::Runs;

    #[test]
    fn an_outcome_waits_until_the_page_is_ready_and_a_run_says_what_it_does_meanwhile() {
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let mut runs = Runs::default();
        runs.start("Pushing main…".to_string(), move || {
            let _ = wait.recv();
            7
        });
        assert_eq!(runs.labels(), ["Pushing main…"]);
        assert_eq!(runs.next(true), None, "nothing until it is done");
        go.send(()).unwrap();
        let start = std::time::Instant::now();
        while runs.labels().len() == 1 && start.elapsed() < std::time::Duration::from_secs(10) {
            assert_eq!(runs.next(false), None, "a page that is busy gets nothing");
        }
        assert!(!runs.is_empty(), "the outcome waits");
        assert_eq!(runs.next(true), Some(7));
        assert!(runs.is_empty());
    }
}
