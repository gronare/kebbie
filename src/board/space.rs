use std::path::PathBuf;

use crate::config;
use crate::herdr;
use crate::repo;
use crate::sh::Result;
use crate::status::Item;
use crate::lead;
use crate::keys::{self, Act};
use crate::notice::{Notice, Tone};
use super::App;

// A tab of that name from an earlier run, with its shell back at the prompt, gets reused.
fn idle_tab_pane(workspace: &str, label: &str) -> Option<String> {
    let tabs: Vec<String> = herdr::tabs_in(workspace).ok()?.into_iter()
        .filter(|tab| tab["label"] == label).filter_map(|tab| tab["tab_id"].as_str().map(str::to_string)).collect();
    herdr::panes_in(workspace).ok()?.into_iter()
        .filter(|pane| pane["tab_id"].as_str().is_some_and(|tab| tabs.iter().any(|candidate| candidate == tab)))
        .filter_map(|pane| pane["pane_id"].as_str().map(str::to_string))
        .find(|pane| matches!(herdr::foreground(pane), Ok(None)))
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Process {
    Server,
    Worker,
}

impl Process {
    // scripts/sync.sh reports a running process from a busy pane in a tab with this label.
    pub(super) fn tab(self, settings: Option<&config::Settings>) -> String {
        match (self, settings) {
            (Process::Server, Some(settings)) => settings.server_name.value.clone(),
            (Process::Worker, Some(settings)) => settings.worker_name.value.clone(),
            (Process::Server, None) => config::DEFAULT_SERVER_NAME.to_string(),
            (Process::Worker, None) => config::DEFAULT_WORKER_NAME.to_string(),
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Process::Server => "the dev server",
            Process::Worker => "the worker",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Process::Server => "worktree.dev_server",
            Process::Worker => "worktree.dev_worker",
        }
    }

    pub(super) fn command(self, settings: &config::Settings) -> Option<String> {
        self.setting(settings).value.clone()
    }

    fn setting(self, settings: &config::Settings) -> &config::Setting<Option<String>> {
        match self {
            Process::Server => &settings.dev_server,
            Process::Worker => &settings.dev_worker,
        }
    }
}

pub(super) fn settings_of(item: &Item) -> Option<config::Settings> {
    Some(config::resolve(Some(item.repo.as_deref()?)).ok()?.settings)
}

// export rather than a PORT= prefix, so the port reaches every part of a compound command; bash, zsh and fish all take it.
fn server_command(command: &str, port: Option<u16>) -> String {
    port.map_or_else(|| command.to_string(), |port| format!("export PORT={port}; {command}"))
}

fn busy_tab_pane(workspace: &str, label: &str) -> Option<String> {
    let tabs: Vec<String> = herdr::tabs_in(workspace).ok()?.into_iter()
        .filter(|tab| tab["label"] == label).filter_map(|tab| tab["tab_id"].as_str().map(str::to_string)).collect();
    herdr::panes_in(workspace).ok()?.into_iter()
        .filter(|pane| pane["tab_id"].as_str().is_some_and(|tab| tabs.iter().any(|candidate| candidate == tab)))
        .filter_map(|pane| pane["pane_id"].as_str().map(str::to_string))
        .find(|pane| matches!(herdr::foreground(pane), Ok(Some(_))))
}

// The review split runs scripts/review.sh list, and nvim or fzf under it while a file is open.
fn runs_review(info: &serde_json::Value) -> bool {
    info["foreground_processes"].as_array().into_iter().flatten().any(|process| {
        let argv: Vec<&str> = process["argv"].as_array().into_iter().flatten().filter_map(|arg| arg.as_str()).collect();
        argv.iter().any(|arg| arg.ends_with("review.sh")) && argv.contains(&"list")
    })
}

fn review_pane_in(panes: &[(String, serde_json::Value)]) -> Option<String> {
    panes.iter().find(|(_, info)| runs_review(info)).map(|(pane, _)| pane.clone())
}

fn open_review_pane(workspace: &str) -> Option<String> {
    let panes: Vec<(String, serde_json::Value)> = herdr::panes_in(workspace).ok()?.into_iter()
        .filter_map(|pane| pane["pane_id"].as_str().map(str::to_string))
        .filter_map(|pane| herdr::process_info(&pane).ok().map(|info| (pane, info)))
        .collect();
    review_pane_in(&panes)
}

// herdr focuses a pane only by its neighbour, so the pane is reached from whichever side has one.
pub(super) fn focus_pane(pane: &str) -> Result<()> {
    if herdr::focused_pane()?.as_deref() == Some(pane) {
        return Ok(());
    }
    if let Some(tab) = herdr::tab_of(pane)? {
        herdr::focus_tab(&tab)?;
    }
    for (side, back) in [("left", "right"), ("right", "left"), ("up", "down"), ("down", "up")] {
        if let Some(neighbour) = herdr::neighbour(pane, side)? {
            herdr::focus_beside(&neighbour, back)?;
            if herdr::focused_pane()?.as_deref() == Some(pane) {
                return Ok(());
            }
        }
    }
    if herdr::focused_pane()?.as_deref() == Some(pane) { Ok(()) } else { Err(format!("herdr did not move the focus to {pane}")) }
}

fn review_script() -> Option<PathBuf> {
    Some(lead::install_root().ok()?.join("scripts/review.sh"))
}

impl App {
    // Each process runs in a tab of the Space named after it; stopping it is ctrl-c to that tab.
    pub(super) fn toggle(&mut self, item: &Item, process: Process) {
        let (Some(workspace), Some(path)) = (&item.workspace, &item.path) else {
            self.about(process.key(), &item.name);
            self.show(Notice::new(Tone::NotNow, "Its Space is not open").next(format!("{} opens it.", keys::map().label(Act::Go))).ok());
            return;
        };
        let settings = settings_of(item);
        let Some(command) = settings.as_ref().and_then(|settings| process.command(settings)) else {
            self.about(process.key(), &item.name);
            self.show(Notice::new(Tone::You, format!("{} has no {} set", item.place, process.key())).you(format!("Set worktree.{} for {}; kebbie config shows where.", process.key(), item.place)).ok());
            return;
        };
        let tab = process.tab(settings.as_ref());
        let tab = tab.as_str();
        let busy = busy_tab_pane(workspace, tab);
        let result = match busy {
            Some(pane) => herdr::send_keys(&pane, &["ctrl+c"]).map(|_| Notice::new(Tone::Done, format!("Stopping {}", process.noun())).to_string()),
            None => {
                let port = repo::worktree_port(path);
                let on = port.map(|port| format!(" on port {port}")).unwrap_or_default();
                let url = (process == Process::Server).then(|| item.repo.as_deref().and_then(|repo| {
                    config::dev_url(settings.as_ref()?.dev_url.value.as_deref(), repo, path)
                })).flatten();
                let at = url.map(|url| format!(" at {url}")).unwrap_or_default();
                idle_tab_pane(workspace, tab).map(Ok).unwrap_or_else(|| herdr::create_tab(workspace, path, tab))
                    .and_then(|pane| herdr::run_in_pane(&pane, &server_command(&command, port)))
                    .map(|_| Notice::new(Tone::Done, format!("Started {}{on}{at}", process.noun())).detail("command", &command).detail("tab", tab).to_string())
            }
        };
        self.about(process.key(), &item.name);
        self.show(result);
    }

    // The review opens next to the task's own Claude, so comments reach that agent; one already open in the Space gets the focus instead of a second split.
    pub(super) fn open_review(&mut self, item: &Item) {
        let (Some(path), Some(agent), Some(script)) = (&item.path, &item.agent, review_script()) else {
            self.about("review", &item.name);
            self.show(Notice::new(Tone::NotNow, "No review: its Claude is not running").next(format!("{} starts it; then the review opens beside it.", keys::map().label(Act::Go))).ok());
            return;
        };
        if let Some(workspace) = &item.workspace
            && let Some(pane) = open_review_pane(workspace)
        {
            let _ = herdr::focus_workspace(workspace);
            if let Err(error) = focus_pane(&pane) {
                self.about("review", &item.name);
                self.show(Notice::new(Tone::NotNow, "The review is open in its Space, but kebbie could not focus it").detail("error", &error).ok());
                return;
            }
            if self.overlay {
                self.quit = true;
            }
            return;
        }
        let output = std::process::Command::new("sh").arg(&script).arg("split").current_dir(path)
            .env("HERDR_PANE_ID", &agent.pane).output();
        match output {
            Ok(output) if output.status.success() => {
                if let Some(workspace) = &item.workspace {
                    let _ = herdr::focus_workspace(workspace);
                }
                if self.overlay {
                    self.quit = true;
                }
            }
            Ok(output) => {
                self.about("review", &item.name);
                self.show(Notice::new(Tone::Failed, "The review did not open").detail("error", String::from_utf8_lossy(&output.stderr)).ok());
            }
            Err(error) => {
                self.about("review", &item.name);
                self.show(Notice::new(Tone::Failed, "The review did not open").detail("error", error.to_string()).ok());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_review_focuses_the_split_already_open_in_the_space() {
        let pane = |argv: &[&str]| serde_json::json!({ "foreground_processes": [{ "pid": 7, "argv": argv }] });
        let panes = vec![
            ("p_claude".to_string(), pane(&["claude", "--resume", "x"])),
            ("p_server".to_string(), pane(&["sh", "-c", "bin/dev"])),
            ("p_review".to_string(), pane(&["sh", "/k/scripts/review.sh", "list", "/state/reviews/p_claude"])),
        ];
        assert_eq!(review_pane_in(&panes).as_deref(), Some("p_review"));
        assert_eq!(review_pane_in(&panes[..2]), None, "without a review split one is made");
        assert!(!runs_review(&pane(&["sh", "/k/scripts/review.sh", "split"])), "the split command itself is not the review");
        let source = include_str!("space.rs");
        let open = &source[source.find("fn open_review(").unwrap()..];
        let open = &open[..open.find("\n    }\n").unwrap()];
        assert!(open.find("open_review_pane(workspace)").unwrap() < open.find("arg(\"split\")").unwrap(), "an open review is looked for before a split is made");
    }

    #[test]
    fn w_exports_the_worktrees_port_only_when_one_is_recorded() {
        assert_eq!(server_command("bin/dev", Some(3002)), "export PORT=3002; bin/dev");
        assert_eq!(server_command("bin/dev", None), "bin/dev");
    }
}
