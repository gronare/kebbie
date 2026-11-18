use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use kebbie_hub_protocol::{Id, Outcome, Reason, StartTask};

use super::commands::{Done, plain};
use crate::permit::Caller;
use crate::sh::Result;

// A name a branch or a running task in the repo already has, which a new start would resume instead of starting.
pub fn taken(repo: &Path, name: &str) -> bool {
    let address = crate::peers::address(name, &crate::repo::name_of(repo));
    crate::repo::branch_exists(repo, name) || crate::tasks::started_links().contains_key(&address)
}

// How long a name an app start chose stays taken, which outlasts the background start making its branch.
const RESERVED_FOR: Duration = Duration::from_secs(120);

// Names app starts chose whose branches may not exist yet, so the next start in the same moment picks another.
#[derive(Default)]
pub struct Reserved(Mutex<BTreeMap<(PathBuf, String), Instant>>);

impl Reserved {
    fn choose(&self, repo: &Path, prompt: &str, taken: impl Fn(&Path, &str) -> bool) -> String {
        let mut held = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        held.retain(|_, at| at.elapsed() < RESERVED_FOR);
        let name = crate::board::task_name(prompt, |name| taken(repo, name) || held.contains_key(&(repo.to_path_buf(), name.to_string())));
        held.insert((repo.to_path_buf(), name.clone()), Instant::now());
        name
    }

    fn release(&self, repo: &Path, name: &str) {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&(repo.to_path_buf(), name.to_string()));
    }
}

static RESERVED: LazyLock<Reserved> = LazyLock::new(Reserved::default);

// A start from the app runs in the repo it names, under its name, or under one n would give its words when it leaves the name empty.
pub fn start_on(start: &StartTask, reserved: &Reserved, locate: impl FnOnce(&str) -> Result<PathBuf>, taken: impl Fn(&Path, &str) -> bool, run: impl FnOnce(&Path, &str, &str) -> Result<String>) -> Done {
    let repo = match locate(&start.repo) {
        Ok(repo) => repo,
        Err(error) => return (Outcome::Refused { reason: Reason::UnknownRepo }, format!("Not started: {}", plain(&error))),
    };
    let name = if start.name.is_empty() { reserved.choose(&repo, &start.prompt, taken) } else { start.name.clone() };
    match run(&repo, &name, &start.prompt) {
        Ok(_) => (Outcome::Done, format!("Started as {}. It shows under Work in a moment; if it cannot start, a notice says why", crate::peers::address(&name, &crate::repo::name_of(&repo)))),
        Err(error) => {
            reserved.release(&repo, &name);
            (Outcome::Failed, format!("Not started: {}", plain(&error)))
        }
    }
}

pub fn start_task(_: &Caller, start: &StartTask) -> Done {
    let device = super::commands::acting();
    start_on(start, &RESERVED, crate::repo::locate, taken, |repo, name, prompt| crate::board::start_described(repo, name, prompt, device.as_ref().map(Id::as_str)))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn start(name: &str) -> StartTask {
        StartTask { repo: "shop".to_string(), name: name.to_string(), prompt: "Fix the rounding in invoices, they are off by a cent".to_string() }
    }

    fn started(start: &StartTask, taken: &[&str]) -> (Done, Option<(PathBuf, String, String)>) {
        let ran = RefCell::new(None);
        let done = start_on(start, &Reserved::default(), |repo| Ok(PathBuf::from("/src").join(repo)), |_, name| taken.contains(&name), |repo, name, prompt| {
            *ran.borrow_mut() = Some((repo.to_path_buf(), name.to_string(), prompt.to_string()));
            Ok(String::new())
        });
        (done, ran.into_inner())
    }

    #[test]
    fn a_start_from_the_app_with_no_name_is_named_from_its_prompt_as_n_names_one_and_never_as_a_name_taken() {
        let fresh = started(&start(""), &[]);
        let named = crate::board::task_name(&start("").prompt, |_| false);
        assert_eq!(fresh.1, Some((PathBuf::from("/src/shop"), named.clone(), start("").prompt)));
        assert_eq!(fresh.0, (Outcome::Done, format!("Started as {named}@shop. It shows under Work in a moment; if it cannot start, a notice says why")));
        let again = started(&start(""), &[named.as_str()]);
        assert_eq!(again.1.unwrap().1, format!("{named}-2"), "a name a branch or running task has gets the next number");
    }

    #[test]
    fn two_app_starts_in_the_same_moment_get_two_names_and_a_failed_one_gives_its_name_back() {
        let reserved = Reserved::default();
        let name_of = |done: &Done| done.1.split_whitespace().nth(2).unwrap().to_string();
        let first = start_on(&start(""), &reserved, |repo| Ok(PathBuf::from(repo)), |_, _| false, |_, _, _| Ok(String::new()));
        let second = start_on(&start(""), &reserved, |repo| Ok(PathBuf::from(repo)), |_, _| false, |_, _, _| Ok(String::new()));
        assert_ne!(name_of(&first), name_of(&second), "the first name is held before the branch exists");
        let failed = start_on(&start(""), &reserved, |repo| Ok(PathBuf::from(repo)), |_, _| false, |_, _, _| Err("herdr is not running".to_string()));
        assert_eq!(failed.0, Outcome::Failed);
        let retried = start_on(&start(""), &reserved, |repo| Ok(PathBuf::from(repo)), |_, _| false, |_, _, _| Ok(String::new()));
        assert!(name_of(&retried).ends_with("-3@shop."), "{}", retried.1);
    }

    #[test]
    fn an_older_start_that_names_its_task_starts_under_that_name() {
        let (done, ran) = started(&start("invoice-rounding"), &["invoice-rounding"]);
        assert_eq!(ran.unwrap().1, "invoice-rounding");
        assert_eq!(done.0, Outcome::Done);
    }

    #[test]
    fn a_repo_kebbie_cannot_find_or_a_start_that_fails_says_so_and_starts_nothing() {
        let unknown = start_on(&start(""), &Reserved::default(), |_| Err("no checkout or clone of shop".to_string()), |_, _| false, |_, _, _| unreachable!());
        assert_eq!(unknown, (Outcome::Refused { reason: Reason::UnknownRepo }, "Not started: no checkout or clone of shop".to_string()));
        let failed = start_on(&start(""), &Reserved::default(), |repo| Ok(PathBuf::from(repo)), |_, _| false, |_, _, _| Err("herdr is not running".to_string()));
        assert_eq!(failed, (Outcome::Failed, "Not started: herdr is not running".to_string()));
    }
}
