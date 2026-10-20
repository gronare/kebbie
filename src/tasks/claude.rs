use std::time::Duration;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::herdr;
use crate::sh::Result;

enum Refusal {
    NotYet,
    Starting,
    Final,
}

// A new pane's shell takes a moment to reach its prompt; until then herdr calls the pane busy, or on macOS measures the line against a shorter limit and asks for a retry; a Claude that just exited holds its agent name a moment longer.
fn refusal(error: &str) -> Refusal {
    if error.contains("agent_pane_busy") || error.contains("wait for the shell prompt and retry") || error.contains("agent_name_taken") {
        Refusal::NotYet
    } else if error.contains("agent_not_ready") || error.contains("timeout") {
        Refusal::Starting
    } else {
        Refusal::Final
    }
}

#[derive(Debug, PartialEq)]
enum Launched {
    Ready,
    ShellBack(String),
}

// herdr types Claude's command into the pane's shell, so it is typed only once that shell is idle at its prompt.
pub fn start_claude(name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<()> {
    start_twice(pane, || {
        wait_for_prompt(pane)?;
        type_claude(name, pane, args, trusted)
    })
}

// A line read away by the shell's own startup, as oh-my-zsh's update question takes its first key, leaves the shell at its prompt: one more try then lands.
fn start_twice(pane: &str, mut attempt: impl FnMut() -> Result<Launched>) -> Result<()> {
    let mut shown = String::new();
    for _ in 0..2 {
        match attempt()? {
            Launched::Ready => return Ok(()),
            Launched::ShellBack(line) => shown = line,
        }
    }
    Err(format!("Claude did not start in pane {pane}: its shell is back at the prompt, showing \"{shown}\"; see what the shell printed there"))
}

fn type_claude(name: &str, pane: &str, args: &[&str], trusted: bool) -> Result<Launched> {
    let mut last = String::new();
    for _ in 0..30 {
        match herdr::start_agent(name, "claude", pane, args) {
            Ok(()) => return Ok(Launched::Ready),
            Err(error) => match refusal(&error) {
                Refusal::NotYet => last = error,
                Refusal::Starting => return wait_until_ready(name, pane, trusted),
                Refusal::Final => return Err(format!("herdr would not start Claude in pane {pane}: {error}")),
            },
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err(format!("The shell in pane {pane} never became ready for Claude; herdr said: {last}"))
}

fn last_line(screen: &str) -> &str {
    screen.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or_default()
}

fn asks(line: &str) -> bool {
    let line = line.to_lowercase();
    ["[y/n]", "(y/n)", "[yes/no]", "(yes/no)"].iter().any(|question| line.contains(question))
}

#[derive(Debug, PartialEq)]
enum Prompt {
    Waiting,
    Asking(String),
    Ready,
}

#[derive(Default)]
struct Settle {
    previous: Option<String>,
    quiet: usize,
}

const QUIET_LOOKS: usize = 4;

impl Settle {
    fn look(&mut self, at_shell: bool, screen: &str) -> Prompt {
        let line = last_line(screen);
        let unchanged = self.previous.as_deref() == Some(screen);
        self.previous = Some(screen.to_string());
        if !at_shell || line.is_empty() {
            self.quiet = 0;
            return Prompt::Waiting;
        }
        if asks(line) {
            self.quiet = 0;
            return Prompt::Asking(line.to_string());
        }
        self.quiet = if unchanged { self.quiet + 1 } else { 0 };
        if self.quiet >= QUIET_LOOKS { Prompt::Ready } else { Prompt::Waiting }
    }
}

pub(crate) fn wait_for_prompt(pane: &str) -> Result<()> {
    let look = || Ok((herdr::foreground(pane)?.is_none(), herdr::read_pane(pane)?));
    prompt_with(look, || std::thread::sleep(Duration::from_millis(250)), 120)?
        .map_err(|question| format!("The shell in pane {pane} asks \"{question}\" as it starts; answer it there, then start again"))
}

// A prompt that keeps redrawing never settles, so the wait ends without an error and the check after the start decides.
fn prompt_with(mut look: impl FnMut() -> Result<(bool, String)>, mut pause: impl FnMut(), tries: usize) -> Result<std::result::Result<(), String>> {
    let mut settle = Settle::default();
    let mut asking = None;
    for _ in 0..tries {
        let (at_shell, screen) = look()?;
        match settle.look(at_shell, &screen) {
            Prompt::Ready => return Ok(Ok(())),
            Prompt::Asking(question) => asking = Some(question),
            Prompt::Waiting => asking = None,
        }
        pause();
    }
    Ok(asking.map_or(Ok(()), Err))
}

// Escape clears what is typed or interrupts; Claude Code exits on its second ctrl-c, and a ctrl-c more would reach the shell.
pub fn end_claude(pane: &str) -> Result<()> {
    end_with(|key| herdr::send_keys(pane, &[key]), || Ok(herdr::foreground(pane)?.is_none()), || std::thread::sleep(Duration::from_millis(500)), 10)
        .map_err(|_| format!("Claude in {pane} did not exit after escape and ctrl-c; see what holds it there"))
}

fn end_with(mut send: impl FnMut(&str) -> Result<()>, mut at_shell: impl FnMut() -> Result<bool>, mut pause: impl FnMut(), tries: usize) -> Result<()> {
    send("esc")?;
    pause();
    for _ in 0..tries {
        if at_shell()? {
            return Ok(());
        }
        send("ctrl+c")?;
        pause();
    }
    if at_shell()? { Ok(()) } else { Err("still running".to_string()) }
}

// Claude asks about a folder it has not seen with "No, exit" selected; a prompt sent before it is ready is lost.
fn wait_until_ready(name: &str, pane: &str, trusted: bool) -> Result<Launched> {
    let mut at_shell = 0;
    for _ in 0..120 {
        let screen = herdr::read_pane(pane)?;
        at_shell = if herdr::foreground(pane)?.is_none() { at_shell + 1 } else { 0 };
        if at_shell >= QUIET_LOOKS {
            return Ok(Launched::ShellBack(last_line(&screen).to_string()));
        }
        if screen.contains("trust this folder") {
            if !trusted {
                return Err(format!("Claude in pane {pane} asks whether you trust its folder; you have not trusted the repo in Claude Code, so that answer is yours"));
            }
            herdr::send_keys(pane, &[if screen.contains("❯ Yes, I trust this folder") { "enter" } else { "down" }])?;
        } else if herdr::agent_ready(name).unwrap_or(false) {
            return Ok(Launched::Ready);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!("Claude in pane {pane} never became ready; see what it waits for there"))
}

// Claude Code's own record of the folders the user trusted, counting a folder under a trusted one.
pub fn trusted_by_user(dir: &Path) -> bool {
    let config = std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|_| std::env::var("HOME").map(PathBuf::from)).map(|dir| dir.join(".claude.json"));
    let claude = config.ok().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| serde_json::from_str::<Value>(&text).ok());
    claude.is_some_and(|claude| trusted_in(&claude, dir))
}

fn trusted_in(claude: &Value, dir: &Path) -> bool {
    dir.ancestors().any(|folder| folder.to_str().is_some_and(|folder| claude["projects"][folder]["hasTrustDialogAccepted"] == true))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;

    #[test]
    fn ending_a_claude_sends_escape_then_ctrl_c_only_until_the_shell_is_back() {
        let run = |exits_after: usize, tries: usize| {
            let keys = std::cell::RefCell::new(Vec::new());
            let result = end_with(|key| { keys.borrow_mut().push(key.to_string()); Ok(()) }, || Ok(keys.borrow().iter().filter(|key| *key == "ctrl+c").count() >= exits_after), || {}, tries);
            (result, keys.into_inner())
        };
        assert_eq!(run(2, 10), (Ok(()), vec!["esc".to_string(), "ctrl+c".to_string(), "ctrl+c".to_string()]));
        assert_eq!(run(0, 10), (Ok(()), vec!["esc".to_string()]));
        let (result, keys) = run(usize::MAX, 3);
        assert!(result.is_err());
        assert_eq!(keys.len(), 4, "{keys:?}");
    }

    #[test]
    fn start_retries_while_herdr_asks_for_the_prompt_and_stops_on_any_other_refusal() {
        assert!(matches!(refusal("herdr agent start refused: agent pane p_1 is busy (agent_pane_busy)"), Refusal::NotYet));
        assert!(matches!(refusal("herdr agent start refused: agent name lead is already used; candidates: … (agent_name_taken)"), Refusal::NotYet), "a Claude that just exited still holds its name");
        assert!(matches!(refusal("herdr agent start refused: input contains a 1100-byte line but the terminal is in canonical mode with a 1024-byte limit; wait for the shell prompt and retry (agent_start_input_failed)"), Refusal::NotYet));
        assert!(matches!(refusal("herdr agent start refused: not ready (agent_not_ready)"), Refusal::Starting));
        assert!(matches!(refusal("herdr agent start refused: agent target pane p_1 not found (agent_pane_not_found)"), Refusal::Final));
    }

    fn waited(looks: &[(bool, &str)], tries: usize) -> (std::result::Result<(), String>, usize) {
        let mut seen = looks.iter().map(|(at_shell, screen)| (*at_shell, screen.to_string()));
        let mut taken = 0;
        let result = prompt_with(|| { taken += 1; Ok(seen.next().unwrap_or((true, String::new()))) }, || {}, tries).unwrap();
        (result, taken)
    }

    #[test]
    fn claude_is_typed_only_once_the_shell_has_shown_the_same_prompt_for_a_while() {
        let prompt = "~/app/.worktrees/1 main\n❯";
        let settled = [(true, prompt); QUIET_LOOKS + 1];
        assert_eq!(waited(&settled, 20), (Ok(()), QUIET_LOOKS + 1));
        let busy_then_prompt = [(false, ""), (true, ""), (false, "Updating"), (true, prompt), (true, prompt), (true, prompt), (true, prompt), (true, prompt)];
        assert_eq!(waited(&busy_then_prompt, 20), (Ok(()), busy_then_prompt.len()), "startup work and an empty screen keep the wait going");
        let redrawn = [(true, "❯"), (true, "❯ 1"), (true, "❯"), (true, "❯ 1")];
        assert_eq!(waited(&redrawn, 4), (Ok(()), 4), "a prompt that never settles ends the wait without an error");
    }

    #[test]
    fn a_shell_asking_a_question_as_it_starts_gets_no_keys_and_the_question_is_named() {
        let omz = "[oh-my-zsh] Would you like to update? [Y/n]";
        let (result, taken) = waited(&[(true, omz); 12], 12);
        assert_eq!((result, taken), (Err(omz.to_string()), 12), "a question that holds still is not a settled prompt");
        let answered = [(true, omz), (true, omz), (false, "git pull"), (true, "❯"), (true, "❯"), (true, "❯"), (true, "❯"), (true, "❯")];
        assert_eq!(waited(&answered, 20), (Ok(()), answered.len()), "once answered in the pane the wait goes on to the prompt");
        assert!(asks("Proceed? (y/N)") && asks("Continue [yes/no]") && !asks("~/app ❯") && !asks("% "));
    }

    #[test]
    fn a_start_whose_line_the_shell_read_away_is_tried_once_more_and_then_named_by_its_pane() {
        let run = |outcomes: Vec<Launched>| {
            let mut outcomes = outcomes.into_iter();
            let mut tries = 0;
            let result = start_twice("w1-2", || { tries += 1; Ok(outcomes.next().unwrap()) });
            (result, tries)
        };
        assert_eq!(run(vec![Launched::Ready]), (Ok(()), 1));
        assert_eq!(run(vec![Launched::ShellBack("zsh: command not found: laude".to_string()), Launched::Ready]), (Ok(()), 2));
        let (result, tries) = run(vec![Launched::ShellBack("❯".to_string()), Launched::ShellBack("❯".to_string())]);
        assert_eq!(tries, 2);
        assert!(result.unwrap_err().starts_with("Claude did not start in pane w1-2: its shell is back at the prompt, showing \"❯\""));
    }

    #[test]
    fn a_folder_counts_as_trusted_when_it_or_a_folder_above_it_is() {
        let claude = json!({ "projects": { "/home/ada/code": { "hasTrustDialogAccepted": true }, "/home/ada/other": { "hasTrustDialogAccepted": false } } });
        assert!(trusted_in(&claude, Path::new("/home/ada/code")));
        assert!(trusted_in(&claude, Path::new("/home/ada/code/shop")));
        assert!(!trusted_in(&claude, Path::new("/home/ada/other/shop")));
        assert!(!trusted_in(&claude, Path::new("/home/ada")));
        assert!(!trusted_in(&json!({}), Path::new("/home/ada/code")));
    }

    #[test]
    fn the_settle_counts_quiet_looks_only_at_a_shell_prompt_that_asks_nothing() {
        let mut settle = Settle::default();
        assert_eq!(settle.look(false, "❯"), Prompt::Waiting);
        assert_eq!(settle.look(true, ""), Prompt::Waiting, "an empty screen is not a prompt");
        assert_eq!(settle.look(true, "Proceed? [y/N]"), Prompt::Asking("Proceed? [y/N]".to_string()));
        for _ in 0..QUIET_LOOKS {
            assert_eq!(settle.look(true, "❯"), Prompt::Waiting, "the first look at a new screen starts the count, the quiet ones add to it");
        }
        assert_eq!(settle.look(true, "❯"), Prompt::Ready);
        assert_eq!(settle.look(true, "❯ x"), Prompt::Waiting, "a change starts the count again");
        assert_eq!(last_line("a\n\n  b  \n\n"), "b");
        assert_eq!(last_line(""), "");
    }
}
