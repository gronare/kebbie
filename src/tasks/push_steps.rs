use std::cell::RefCell;

type Sink = Box<dyn Fn(&str, &str)>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = RefCell::new(None);
}

// The steps a push takes on this thread reach the sink as (task, step), so the board can say which one is under way.
pub(crate) fn watched<T>(sink: impl Fn(&str, &str) + 'static, run: impl FnOnce() -> T) -> T {
    SINK.with(|slot| *slot.borrow_mut() = Some(Box::new(sink)));
    let out = run();
    SINK.with(|slot| *slot.borrow_mut() = None);
    out
}

pub(crate) fn step(task: &str, what: &str) {
    SINK.with(|slot| {
        if let Some(sink) = slot.borrow().as_ref() {
            sink(task, what);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{step, watched};

    #[test]
    fn a_step_reaches_only_the_sink_of_its_own_thread() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let into = seen.clone();
        step("billing", "before");
        watched(move |task, what| into.lock().unwrap().push(format!("{task}: {what}")), || {
            step("billing", "rebasing onto main");
            std::thread::spawn(|| step("other", "elsewhere")).join().unwrap();
        });
        step("billing", "after");
        assert_eq!(*seen.lock().unwrap(), ["billing: rebasing onto main"]);
    }
}
