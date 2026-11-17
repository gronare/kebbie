use std::path::{Path, PathBuf};

use kebbie_hub_protocol::{Id, Outcome, Reason, StartTask};

use super::commands::{Done, plain};
use crate::permit::Caller;
use crate::sh::Result;

// A name a branch or a running task in the repo already has, which a new start would resume instead of starting.
pub fn taken(repo: &Path, name: &str) -> bool {
    let address = crate::peers::address(name, &crate::repo::name_of(repo));
    crate::repo::branch_exists(repo, name) || crate::tasks::started_links().contains_key(&address)
}

// A start from the app runs in the repo it names, under its name, or under one n would give its words when it leaves the name empty.
pub fn start_on(start: &StartTask, locate: impl FnOnce(&str) -> Result<PathBuf>, taken: impl Fn(&Path, &str) -> bool, run: impl FnOnce(&Path, &str, &str) -> Result<String>) -> Done {
    let repo = match locate(&start.repo) {
        Ok(repo) => repo,
        Err(error) => return (Outcome::Refused { reason: Reason::UnknownRepo }, format!("Not started: {}", plain(&error))),
    };
    let name = if start.name.is_empty() { crate::board::task_name(&start.prompt, |name| taken(&repo, name)) } else { start.name.clone() };
    match run(&repo, &name, &start.prompt) {
        Ok(_) => (Outcome::Done, format!("Started as {}. It shows under Work in a moment; if it cannot start, a notice says why", crate::peers::address(&name, &crate::repo::name_of(&repo)))),
        Err(error) => (Outcome::Failed, format!("Not started: {}", plain(&error))),
    }
}

pub fn start_task(_: &Caller, start: &StartTask) -> Done {
    let device = super::commands::acting();
    start_on(start, crate::repo::locate, taken, |repo, name, prompt| crate::board::start_described(repo, name, prompt, device.as_ref().map(Id::as_str)))
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
        let done = start_on(start, |repo| Ok(PathBuf::from("/src").join(repo)), |_, name| taken.contains(&name), |repo, name, prompt| {
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
    fn an_older_start_that_names_its_task_starts_under_that_name() {
        let (done, ran) = started(&start("invoice-rounding"), &["invoice-rounding"]);
        assert_eq!(ran.unwrap().1, "invoice-rounding");
        assert_eq!(done.0, Outcome::Done);
    }

    #[test]
    fn a_repo_kebbie_cannot_find_or_a_start_that_fails_says_so_and_starts_nothing() {
        let unknown = start_on(&start(""), |_| Err("no checkout or clone of shop".to_string()), |_, _| false, |_, _, _| unreachable!());
        assert_eq!(unknown, (Outcome::Refused { reason: Reason::UnknownRepo }, "Not started: no checkout or clone of shop".to_string()));
        let failed = start_on(&start(""), |repo| Ok(PathBuf::from(repo)), |_, _| false, |_, _, _| Err("herdr is not running".to_string()));
        assert_eq!(failed, (Outcome::Failed, "Not started: herdr is not running".to_string()));
    }
}
