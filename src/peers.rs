use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::herdr;
use crate::lead::install_root;
use crate::repo::{main_checkout, name_of, setup_note};
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use crate::tasks::slots;

// A Claude that can be told things: a task in a repo (task@repo) or the lead.
pub struct Session {
    pub address: String,
    pub task: Option<String>,
    pub pane: Option<String>,
    pub status: Option<String>,
}

impl Session {
    pub fn state(&self) -> &str {
        match (&self.pane, self.status.as_deref()) {
            (None, _) => "no Claude",
            (Some(_), Some("blocked")) => "waiting for an answer",
            (Some(_), Some(status)) => status,
            (Some(_), None) => "unknown",
        }
    }
}

pub fn address(task: &str, repo: &str) -> String {
    format!("{task}@{repo}")
}

pub fn sessions() -> Result<Vec<Session>> {
    let workspaces = herdr::workspaces()?;
    let agents: Vec<Value> = herdr::agents()?.into_iter().filter(|agent| agent["agent"] == "claude").collect();
    let agent_in = |workspace: Option<&str>| workspace.and_then(|id| agents.iter().find(|agent| agent["workspace_id"].as_str() == Some(id)));
    let text = |value: &Value| value.as_str().map(str::to_string);
    let mut list = Vec::new();
    for workspace in workspaces.iter().filter(|workspace| workspace["label"] == LEAD_LABEL) {
        let agent = agent_in(workspace["workspace_id"].as_str());
        list.push(Session { address: LEAD_LABEL.to_string(), task: None, pane: agent.and_then(|agent| text(&agent["pane_id"])), status: agent.and_then(|agent| text(&agent["agent_status"])) });
    }
    let repos: BTreeSet<PathBuf> = workspaces.iter().filter_map(|workspace| workspace["worktree"]["repo_root"].as_str()).map(PathBuf::from).collect();
    for repo in &repos {
        for slot in slots(repo)? {
            let Some(task) = slot.task else { continue };
            let workspace = workspaces.iter().find(|workspace| workspace["worktree"]["checkout_path"].as_str() == slot.path.to_str())
                .and_then(|workspace| workspace["workspace_id"].as_str());
            let agent = agent_in(workspace);
            list.push(Session {
                address: address(&task, &name_of(repo)),
                task: Some(task),
                pane: agent.and_then(|agent| text(&agent["pane_id"])),
                status: agent.and_then(|agent| text(&agent["agent_status"])),
            });
        }
    }
    Ok(list)
}

// Who runs this: the task whose worktree holds cwd, or the lead by its herdr Space.
pub fn me(cwd: &Path) -> Option<String> {
    if let Ok(repo) = main_checkout(cwd)
        && let Some(task) = slots(&repo).ok()?.into_iter().find(|slot| slot.task.is_some() && cwd.starts_with(&slot.path)).and_then(|slot| slot.task)
    {
        return Some(address(&task, &name_of(&repo)));
    }
    let workspace = std::env::var("HERDR_WORKSPACE_ID").ok()?;
    herdr::workspaces().ok()?.iter()
        .any(|space| space["workspace_id"] == workspace.as_str() && space["label"] == LEAD_LABEL)
        .then(|| LEAD_LABEL.to_string())
}

// `task` reaches every half of that task but the sender's own; `task@repo` and `lead` reach one session.
pub fn resolve<'a>(to: &str, from: &str, sessions: &'a [Session]) -> Result<Vec<&'a Session>> {
    let found: Vec<&Session> = if to == LEAD_LABEL || to.contains('@') {
        sessions.iter().filter(|session| session.address == to).collect()
    } else {
        sessions.iter().filter(|session| session.task.as_deref() == Some(to)).collect()
    };
    let found: Vec<&Session> = found.into_iter().filter(|session| session.address != from).collect();
    if found.is_empty() {
        return Err(format!("No session \"{to}\" to tell (other than you); kelpie peers lists them"));
    }
    Ok(found)
}

pub fn list(me: Option<&str>, sessions: &[Session]) -> String {
    let task = me.and_then(|me| me.split_once('@')).map(|(task, _)| task);
    let line = |session: &Session| format!("  {:<36} {}", session.address, session.state());
    let others = |same: bool| -> Vec<String> {
        sessions.iter()
            .filter(|session| session.task.is_some() && Some(session.address.as_str()) != me && (session.task.as_deref() == task) == same)
            .map(line).collect()
    };
    let mut lines = vec![match me {
        Some(me) => format!("You are {me}."),
        None => "You are not in a task's worktree or the lead.".to_string(),
    }];
    let (same, other) = (others(true), others(false));
    if !same.is_empty() {
        lines.push("Same task, other repos:".to_string());
        lines.extend(same);
    }
    lines.push("Other tasks:".to_string());
    lines.extend(if other.is_empty() { vec!["  none".to_string()] } else { other });
    if me != Some(LEAD_LABEL) {
        lines.extend(sessions.iter().filter(|session| session.address == LEAD_LABEL).map(line));
    }
    lines.join("\n")
}

pub fn peers(cwd: &Path) -> Result<String> {
    Ok(list(me(cwd).as_deref(), &sessions()?))
}

// The stable link when it leads to this binary, so a task's settings survive reinstalls.
pub fn kelpie_command() -> Result<String> {
    let exe = std::env::current_exe().and_then(|path| path.canonicalize()).map_err(|error| error.to_string())?;
    let home = std::env::var("HOME").unwrap_or_default();
    let stable = PathBuf::from(home).join(".local/share/kelpie/current/target/release/kelpie");
    Ok(if stable.canonicalize().ok() == Some(exe.clone()) { stable } else { exe }.to_string_lossy().to_string())
}

pub struct Place<'a> {
    pub repo: &'a Path,
    pub worktree: &'a Path,
    pub setup_skill: &'a str,
    pub setup_command: Option<&'a str>,
    pub rules_skill: &'a str,
}

pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    values.iter().fold(template.to_string(), |text, (name, value)| text.replace(&format!("{{{name}}}"), value))
}

pub fn worktree_values(place: &Place) -> Result<Vec<(&'static str, String)>> {
    Ok(vec![
        ("path", place.worktree.display().to_string()),
        ("slot", name_of(place.worktree)),
        ("main", place.repo.display().to_string()),
        ("note", setup_note(place.worktree)?.display().to_string()),
        ("setup_skill", place.setup_skill.to_string()),
        ("setup", setup_line(place.setup_command)),
        ("rules", rules_line(place.rules_skill)),
    ])
}

fn setup_line(command: Option<&str>) -> String {
    command.map_or(String::new(), |command| format!(" It prepares the worktree with the repo's own command `{command}` (worktree.setup_command)."))
}

fn rules_line(rules_skill: &str) -> String {
    format!("\n\nThen run the skill `{rules_skill}` and follow it for the rest of this task: it holds the user's rules for how work is planned, built and committed here. When you report ready, the lead checks your commits against it.")
}

// roles/task.md filled in for this task, written to the state dir for --append-system-prompt-file.
pub fn role_file(task: &str, place: &Place) -> Result<PathBuf> {
    let template = fs::read_to_string(install_root()?.join("roles/task.md")).map_err(|error| format!("roles/task.md: {error}"))?;
    let repo = name_of(place.repo);
    let me = address(task, &repo);
    let sessions = sessions()?;
    let listed = list(Some(&me), &sessions);
    let peers = listed.split_once('\n').map(|(_, rest)| rest).unwrap_or_default().to_string();
    let mut values = vec![("address", me.clone()), ("task", task.to_string()), ("repo", repo), ("kelpie", kelpie_command()?), ("peers", peers)];
    values.extend(worktree_values(place)?);
    let pairs: Vec<(&str, &str)> = values.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let file = crate::lead::roles_dir()?.join(format!("{me}.md"));
    fs::write(&file, fill(&template, &pairs)).map_err(|error| error.to_string())?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(address: &str, status: Option<&str>) -> Session {
        let task = address.split_once('@').map(|(task, _)| task.to_string());
        Session { address: address.to_string(), task, pane: status.map(|_| "w1:p1".to_string()), status: status.map(str::to_string) }
    }

    fn sessions() -> Vec<Session> {
        vec![session("lead", Some("idle")), session("billing@api", Some("working")), session("billing@web", Some("blocked")), session("search@api", None)]
    }

    fn addresses(found: Result<Vec<&Session>>) -> Vec<String> {
        found.unwrap().iter().map(|session| session.address.clone()).collect()
    }

    #[test]
    fn a_task_name_reaches_its_other_halves() {
        let all = sessions();
        assert_eq!(addresses(resolve("billing", "billing@api", &all)), ["billing@web"]);
        assert_eq!(addresses(resolve("billing", "lead", &all)), ["billing@api", "billing@web"]);
    }

    #[test]
    fn an_address_or_the_lead_reaches_one_session() {
        let all = sessions();
        assert_eq!(addresses(resolve("search@api", "billing@web", &all)), ["search@api"]);
        assert_eq!(addresses(resolve("lead", "billing@web", &all)), ["lead"]);
    }

    #[test]
    fn nobody_tells_themselves_or_an_unknown_session() {
        let all = sessions();
        assert!(resolve("search", "search@api", &all).is_err());
        assert!(resolve("lead", "lead", &all).is_err());
        assert!(resolve("payments", "lead", &all).is_err());
    }

    #[test]
    fn the_setup_skill_reaches_the_role_from_each_layer() {
        use crate::config::{Machine, Worktree, parse_machine, parse_repo, settings};
        let template = include_str!("../roles/task.md");
        let machine = parse_machine("[worktree]\nsetup_skill = \"my-tools:set-up-worktree\"\n", "config.toml").unwrap();
        let own = parse_repo("[worktree]\nsetup_skill = \"app-tools:set-up-worktree\"\n", ".kelpie.toml").unwrap();
        for (own, machine, skill) in [
            (own.clone(), machine.clone(), "app-tools:set-up-worktree"),
            (Worktree::default(), machine, "my-tools:set-up-worktree"),
            (Worktree::default(), Machine::default(), "kelpie:set-up-worktree"),
        ] {
            let skill_value = settings(&own, &machine).setup_skill.value;
            let role = fill(template, &[("path", "/src/wt2.app"), ("slot", "wt2.app"), ("main", "/src/app"), ("note", "/src/app/.git/worktrees/wt2.app/kelpie-setup.md"), ("setup_skill", &skill_value)]);
            assert!(role.contains(&format!("run the skill `{skill}` and follow it")), "{role}");
            assert!(role.contains("You are in `/src/wt2.app`, worktree wt2.app of `/src/app`"));
            assert!(role.contains("a note in `/src/app/.git/worktrees/wt2.app/kelpie-setup.md`"));
        }
    }

    #[test]
    fn the_setup_command_reaches_the_role_only_when_set() {
        use crate::config::{Machine, Worktree, parse_repo, settings};
        let template = include_str!("../roles/task.md");
        let own = parse_repo("[worktree]\nsetup_command = \"script/prepare_worktree\"\n", ".kelpie.toml").unwrap();
        let role = |own: &Worktree| fill(template, &[("setup_skill", "kelpie:set-up-worktree"), ("setup", &setup_line(settings(own, &Machine::default()).setup_command.value.as_deref()))]);
        let set = role(&own);
        assert!(set.contains("on what it did. It prepares the worktree with the repo's own command `script/prepare_worktree` (worktree.setup_command). The main checkout"), "{set}");
        let unset = role(&Worktree::default());
        assert!(unset.contains("on what it did. The main checkout is read-only to you."), "{unset}");
        assert!(!unset.contains("{setup}") && !unset.contains("setup_command"));
    }

    #[test]
    fn the_rules_skill_reaches_the_role_from_each_layer() {
        use crate::config::{Machine, Worktree, parse_machine, parse_repo, settings};
        let template = include_str!("../roles/task.md");
        let machine = parse_machine("[task]\nrules_skill = \"my-rules\"\n", "config.toml").unwrap();
        let own = parse_repo("[task]\nrules_skill = \"app-rules\"\n", ".kelpie.toml").unwrap();
        for (own, machine, skill) in [
            (own.clone(), machine.clone(), "app-rules"),
            (Worktree::default(), machine, "my-rules"),
            (Worktree::default(), Machine::default(), "kelpie:work-rules"),
        ] {
            let role = fill(template, &[("rules", &rules_line(&settings(&own, &machine).rules_skill.value))]);
            assert!(role.contains(&format!("Then run the skill `{skill}` and follow it for the rest of this task")), "{role}");
            assert!(!role.contains("{rules}"));
        }
    }

    #[test]
    fn the_role_isolates_a_probe_from_the_users_state_and_packages() {
        let role = fill(include_str!("../roles/task.md"), &[]);
        for setting in ["KELPIE_STATE_DIR=<temp>/kelpie", "`GEM_HOME` and `BUNDLE_PATH`", "`NPM_CONFIG_PREFIX`", "a virtualenv", "`HERDR_BIN_PATH=false`", "HERDR_SOCKET_PATH=<temp>/herdr.sock"] {
            assert!(role.contains(setting), "{setting}");
        }
    }

    #[test]
    fn the_role_asks_the_user_through_kelpie_one_question_at_a_time() {
        let role = fill(include_str!("../roles/task.md"), &[("kelpie", "/bin/kelpie")]);
        for needed in ["/bin/kelpie ask --option", "--recommend <n>", "/bin/kelpie ask --plan <file>", "one question per", "the lead's reply to your question"] {
            assert!(role.contains(needed), "{needed}");
        }
    }

    #[test]
    fn the_list_puts_the_other_halves_first() {
        let listed = list(Some("billing@api"), &sessions());
        assert!(listed.starts_with("You are billing@api.\nSame task, other repos:\n  billing@web"), "{listed}");
        assert!(listed.contains("Other tasks:\n  search@api"));
        assert!(listed.lines().last().unwrap().starts_with("  lead"));
    }
}
