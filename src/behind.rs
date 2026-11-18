use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

// The outcome a run gives when its thread ends without one, as when it panics.
pub trait Lost {
    fn lost(label: &str) -> Self;
}

// Work a page runs behind it: a line for each while it runs, and outcomes that wait until the page can show them.
pub struct Runs<T> {
    running: Vec<(String, String, Receiver<T>)>,
    done: Vec<T>,
}

impl<T> Default for Runs<T> {
    fn default() -> Self {
        Runs { running: Vec::new(), done: Vec::new() }
    }
}

impl<T: Lost + Send + 'static> Runs<T> {
    pub fn start(&mut self, label: String, work: impl FnOnce() -> T + Send + 'static) {
        let key = label.clone();
        self.start_as(key, label, work);
    }

    // A run under a key, such as an action in one repo, that is_running finds while it runs.
    pub fn start_as(&mut self, key: String, label: String, work: impl FnOnce() -> T + Send + 'static) {
        let (sender, result) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(work());
        });
        self.running.push((key, label.trim_end_matches('…').to_string(), result));
    }

    pub fn is_running(&self, key: &str) -> bool {
        self.running.iter().any(|(running, _, _)| running == key)
    }

    pub fn labels(&self) -> Vec<String> {
        self.running.iter().map(|(_, label, _)| format!("{label}…")).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.running.is_empty() && self.done.is_empty()
    }

    // Gathers what finished; hands the oldest outcome over only once the page is ready to show it.
    pub fn next(&mut self, ready: bool) -> Option<T> {
        self.running.retain(|(_, label, result)| match result.try_recv() {
            Ok(outcome) => {
                self.done.push(outcome);
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.done.push(T::lost(label));
                false
            }
        });
        (ready && !self.done.is_empty()).then(|| self.done.remove(0))
    }
}

type Read<A> = (String, Box<dyn FnOnce() -> A + Send>);

// Reads a draw needs, done on one thread: each key is asked again once its last ask is older than the wait, and its last answer shows meanwhile; before spawn, as in tests, it reads in place.
pub struct Reader<A> {
    answers: HashMap<String, A>,
    asked: HashMap<String, Instant>,
    requests: Option<Sender<Read<A>>>,
    results: Option<Receiver<(String, A)>>,
}

impl<A> Default for Reader<A> {
    fn default() -> Self {
        Reader { answers: HashMap::new(), asked: HashMap::new(), requests: None, results: None }
    }
}

impl<A: Clone + Send + 'static> Reader<A> {
    pub fn spawn(&mut self) {
        let (request_tx, request_rx) = mpsc::channel::<Read<A>>();
        let (answer_tx, answer_rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok((key, read)) = request_rx.recv() {
                if answer_tx.send((key, read())).is_err() {
                    return;
                }
            }
        });
        (self.requests, self.results) = (Some(request_tx), Some(answer_rx));
    }

    #[cfg(test)]
    pub fn put(&mut self, key: &str, answer: A) {
        self.answers.insert(key.to_string(), answer);
        self.asked.insert(key.to_string(), Instant::now());
    }

    pub fn get(&mut self, key: &str, wait: Duration, read: impl FnOnce() -> A + Send + 'static) -> Option<A> {
        if self.asked.get(key).is_none_or(|at| at.elapsed() >= wait) {
            self.asked.insert(key.to_string(), Instant::now());
            match &self.requests {
                Some(requests) => {
                    let _ = requests.send((key.to_string(), Box::new(read)));
                }
                None => {
                    self.answers.insert(key.to_string(), read());
                }
            }
        }
        self.answers.get(key).cloned()
    }

    pub fn forget(&mut self, key: &str) {
        self.answers.remove(key);
        self.asked.remove(key);
    }

    pub fn take(&mut self) -> bool {
        let Some(results) = &self.results else { return false };
        let mut changed = false;
        while let Ok((key, answer)) = results.try_recv() {
            self.answers.insert(key, answer);
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::{Lost, Runs};

    impl Lost for i32 {
        fn lost(_: &str) -> Self {
            -1
        }
    }

    #[test]
    fn a_run_whose_thread_panics_leaves_a_lost_outcome() {
        let mut runs: Runs<i32> = Runs::default();
        runs.start("Pushing main".to_string(), || panic!("a run that panics"));
        let start = std::time::Instant::now();
        while runs.labels().len() == 1 {
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "the run never ended");
            assert_eq!(runs.next(false), None);
        }
        assert_eq!(runs.next(true), Some(-1), "it says it stopped instead of vanishing");
    }

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

    #[test]
    fn a_reader_asks_once_per_wait_and_shows_the_last_answer_while_it_reads_again() {
        let mut reader = super::Reader::default();
        reader.spawn();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        assert_eq!(reader.get("pane", std::time::Duration::ZERO, move || { let _ = wait.recv(); "first".to_string() }), None, "nothing until it is read");
        go.send(()).unwrap();
        let start = std::time::Instant::now();
        while !reader.take() {
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "it never answered");
        }
        assert_eq!(reader.get("pane", std::time::Duration::from_secs(60), || panic!("asked again before the wait")), Some("first".to_string()));
        let (go, wait) = std::sync::mpsc::channel::<()>();
        assert_eq!(reader.get("pane", std::time::Duration::ZERO, move || { let _ = wait.recv(); "second".to_string() }), Some("first".to_string()), "the last answer shows while it reads again");
        drop(go);
    }
}
