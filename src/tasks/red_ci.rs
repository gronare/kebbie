use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::messages;
use crate::notice::{Notice, Tone};
use crate::peers;
use crate::permit::Caller;
use crate::repo::{main_checkout, name_of};
use crate::sh::Result;
use super::intent::{self, Work};
use super::pool::{find_task, task_of};

fn dir(state: &Path) -> PathBuf {
    state.join("red-ci")
}

fn file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(format!("{address}.json"))
}

fn read(state: &Path, address: &str) -> Value {
    fs::read_to_string(file(state, address)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_else(|| json!({}))
}

fn red(state: &Path, address: &str) -> Option<crate::ci::Watch> {
    crate::ci::load(state, address).filter(|watch| !watch.running())
}

pub(crate) fn outside_reason(state: &Path, address: &str) -> Option<String> {
    let watch = red(state, address)?;
    let said = read(state, address)["outside"].clone();
    (said["sha"].as_str() == Some(watch.sha.as_str())).then(|| said["reason"].as_str().map(str::to_string)).flatten()
}

pub(crate) fn dismissed(state: &Path, address: &str) -> Vec<Value> {
    read(state, address)["dismissed"].as_array().cloned().unwrap_or_default()
}

pub(crate) fn forget(state: &Path, address: &str) {
    let _ = fs::remove_file(file(state, address));
}

pub(crate) fn outside_as(state: &Path, caller: &Caller, reason: &str) -> Result<String> {
    let Caller::Task(address) = caller else {
        return Err("kelpie ci outside is the task's word on its own red CI, run by its Claude; the user dismisses a red CI with kelpie ci dismiss".to_string());
    };
    if reason.trim().is_empty() {
        return Err("kelpie ci outside needs the reason: kelpie ci outside \"<what failed, outside the code>\"".to_string());
    }
    let watch = red(state, address).ok_or_else(|| format!("{address} has no red CI to speak of"))?;
    let mut record = read(state, address);
    record["outside"] = json!({ "reason": reason.trim(), "sha": watch.sha, "at": messages::now() });
    crate::state::write(&file(state, address), record.to_string())?;
    let text = format!("{address} says the red CI on {} is outside the code: {}. Only the user can dismiss it (kelpie ci dismiss).", watch.short(), reason.trim());
    if let Err(error) = messages::tell_lead(&text, Some("ci")) {
        eprintln!("Could not tell the lead: {error}");
    }
    Notice::new(Tone::Done, "Recorded: the red CI is outside the code").next("The board says so on its row; the user decides whether to dismiss it.").detail("task", address).ok()
}

pub fn outside(reason: &str) -> Result<String> {
    outside_as(&crate::lead::state_dir()?, &crate::permit::caller(), reason)
}

pub(crate) fn dismiss_as(state: &Path, caller: &Caller, cwd: &Path, name: Option<&str>, why: Option<&str>) -> Result<String> {
    crate::permit::user_only_as(caller, "dismissing a red CI")?;
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    let address = peers::address(task, &name_of(&repo));
    let Some(watch) = red(state, &address) else {
        return Notice::new(Tone::NotNow, "No red CI to dismiss").detail("task", task).err();
    };
    let why = why.map(str::trim).filter(|why| !why.is_empty()).map(str::to_string).or_else(|| outside_reason(state, &address)).unwrap_or_else(|| "dismissed by the user".to_string());
    let what = match &watch.state {
        crate::ci::State::Failed { what, url, .. } => json!({ "what": what, "url": url }),
        crate::ci::State::Running => json!({}),
    };
    let entry = json!({ "sha": watch.sha, "failed": what, "why": why, "at": messages::now() });
    let mut record = read(state, &address);
    record.as_object_mut().map(|record| record.remove("outside"));
    let mut archived = dismissed(state, &address);
    archived.push(entry);
    record["dismissed"] = json!(archived);
    crate::state::write(&file(state, &address), record.to_string())?;
    crate::ci::forget(state, &address);
    if intent::pending(state, &address).is_some_and(|pending| pending.work == Work::Ci) {
        intent::forget(state, &address);
    }
    super::after::red_ci_superseded(state, &address);
    crate::phase::record(state, &address, "dismiss", &crate::phase::Event::Dismissed { kept_open: super::landed::kept_open(state, &address) });
    let then = if super::landed::kept_open(state, &address) { "It stays open, as the user chose when pushing it." } else { "kelpie closes the task once its commits are on the default branch." };
    let text = format!("The user dismissed the red CI of {address} on {}: {why}. {then}", watch.short());
    if let Err(error) = messages::tell_lead(&text, Some("ci")) {
        eprintln!("Could not tell the lead: {error}");
    }
    Notice::new(Tone::Done, "Dismissed the red CI").next(then).detail("task", task).detail("why", why).ok()
}

pub fn dismiss(cwd: &Path, name: Option<&str>, why: Option<&str>) -> Result<String> {
    dismiss_as(&crate::lead::state_dir()?, &crate::permit::caller(), cwd, name, why)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::test_support::*;

    fn red_watch(dir: &Path, address: &str) -> crate::ci::Watch {
        crate::ci::Watch {
            address: address.to_string(), task: "billing".to_string(), repo: dir.to_path_buf(), slug: "acme/app".to_string(), sha: "abcdef0123".to_string(), started_at: 1, checked_at: None, close: true, runs: Vec::new(),
            state: crate::ci::State::Failed { at: 2, what: "deploy · release · Deploy".to_string(), url: "https://ci/1".to_string() }, error: None, erring_since: None, pushed: crate::ci::Pushed::default(),
        }
    }

    #[test]
    fn the_task_may_say_its_red_ci_is_outside_the_code_and_only_the_user_dismisses_it() {
        let (dir, origin, slot, _) = task_with_a_commit("billing");
        let state = state_of(&dir);
        let address = peers::address("billing", &name_of(&dir));
        crate::ci::save(&state, &red_watch(&dir, &address)).unwrap();
        intent::after_red_ci(&state, &dir, &address, intent::By::User).unwrap();
        assert!(outside_as(&state, &Caller::Lead, "deploy window").is_err(), "the lead does not speak for the task");
        outside_as(&state, &Caller::Task(address.clone()), "pushed outside the deploy window").unwrap();
        assert_eq!(outside_reason(&state, &address).as_deref(), Some("pushed outside the deploy window"));
        assert!(crate::ci::load(&state, &address).is_some(), "saying it waives nothing");
        for caller in [Caller::Task(address.clone()), Caller::Lead, Caller::Agent] {
            assert!(dismiss_as(&state, &caller, &dir, Some("billing"), None).unwrap_err().contains("the user's call"));
        }
        assert!(crate::ci::load(&state, &address).is_some() && intent::pending(&state, &address).is_some());
        let said = dismiss_as(&state, &Caller::User, &dir, Some("billing"), None).unwrap();
        assert!(said.contains("Dismissed the red CI"), "{said}");
        assert!(crate::ci::load(&state, &address).is_none() && intent::pending(&state, &address).is_none(), "the watch and the push waiting for a fix go");
        let archived = dismissed(&state, &address);
        assert_eq!((archived[0]["why"].as_str(), archived[0]["failed"]["what"].as_str()), (Some("pushed outside the deploy window"), Some("deploy · release · Deploy")), "the task's reason is the record's when the user gives none");
        assert!(dismiss_as(&state, &Caller::User, &dir, Some("billing"), Some("again")).unwrap_err().contains("No red CI"));
        crate::ci::save(&state, &red_watch(&dir, &address)).unwrap();
        crate::tasks::landed::keep_open(&state, &address, true);
        let kept = dismiss_as(&state, &Caller::User, &dir, Some("billing"), Some("a flake")).unwrap();
        assert!(kept.contains("It stays open, as the user chose") && !kept.contains("kelpie closes"), "{kept}");
        cleanup(dir, origin, &slot);
    }
}
