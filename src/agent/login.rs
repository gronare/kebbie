use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Login {
    In,
    Out,
    Unknown,
}

impl Login {
    pub fn shown(self, fix: &str) -> String {
        match self {
            Login::In => "signed in".to_string(),
            Login::Out => format!("not signed in: run {fix}"),
            Login::Unknown => "sign-in not checked".to_string(),
        }
    }
}

pub const CLAUDE_FIX: &str = "claude auth login";
pub const CODEX_FIX: &str = "codex login";

// Exit code and stdout with stderr, or None when it does not start or outlives the timeout.
fn answer(program: &Path, args: &[&str], timeout: Duration) -> Option<(Option<i32>, String)> {
    let mut command = Command::new(program);
    command.args(args);
    let done = crate::sh::output_with_timeout(command, timeout).ok()??;
    Some((done.code, [done.stdout, done.stderr].join("\n")))
}

// claude auth status prints JSON with loggedIn whatever its exit code; no JSON is no answer.
pub fn claude(claude: Option<&Path>, timeout: Duration) -> Login {
    let Some((_, text)) = claude.and_then(|claude| answer(claude, &["auth", "status"], timeout)) else { return Login::Unknown };
    let json = text.find('{').and_then(|start| serde_json::Deserializer::from_str(&text[start..]).into_iter::<serde_json::Value>().next()?.ok());
    match json.and_then(|json| json["loggedIn"].as_bool()) {
        Some(true) => Login::In,
        Some(false) => Login::Out,
        None => Login::Unknown,
    }
}

// codex login status exits 0 when logged in and 1 when not; anything else is no answer.
pub fn codex(codex: Option<&Path>, timeout: Duration) -> Login {
    match codex.and_then(|codex| answer(codex, &["login", "status"], timeout)) {
        Some((Some(0), _)) => Login::In,
        Some((Some(1), said)) if said.contains("Not logged in") || said.trim().is_empty() => Login::Out,
        _ => Login::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        crate::executable::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        path
    }

    #[test]
    fn claude_reads_logged_in_from_auth_status_and_anything_else_is_not_checked() {
        let dir = crate::sh::test_dir("login-claude");
        assert_eq!(claude(Some(&fake(&dir, "in", "echo '{\"loggedIn\": true, \"authMethod\": \"claude.ai\"}'")), TIMEOUT), Login::In);
        assert_eq!(claude(Some(&fake(&dir, "out", "echo '{\"loggedIn\": false}'; exit 1")), TIMEOUT), Login::Out);
        assert_eq!(claude(Some(&fake(&dir, "old", "echo \"error: unknown command 'auth'\" >&2; exit 1")), TIMEOUT), Login::Unknown, "an older claude without auth status is no answer");
        assert_eq!(claude(Some(&fake(&dir, "slow", "sleep 5")), Duration::from_millis(200)), Login::Unknown, "a claude that hangs is stopped and is no answer");
        assert_eq!(claude(None, TIMEOUT), Login::Unknown);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_reads_its_exit_code_and_anything_else_is_not_checked() {
        let dir = crate::sh::test_dir("login-codex");
        assert_eq!(codex(Some(&fake(&dir, "in", "echo 'Logged in using ChatGPT'")), TIMEOUT), Login::In);
        assert_eq!(codex(Some(&fake(&dir, "out", "echo 'Not logged in'; exit 1")), TIMEOUT), Login::Out);
        assert_eq!(codex(Some(&fake(&dir, "old", "echo 'unknown command' >&2; exit 2")), TIMEOUT), Login::Unknown);
        assert_eq!(codex(Some(&fake(&dir, "slow", "sleep 5")), Duration::from_millis(200)), Login::Unknown);
        assert_eq!(codex(None, TIMEOUT), Login::Unknown);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_status_command_that_leaves_a_child_holding_its_output_still_answers() {
        let dir = crate::sh::test_dir("login-grandchild");
        let started = std::time::Instant::now();
        assert_eq!(codex(Some(&fake(&dir, "forks", "sleep 30 &\necho 'Logged in using ChatGPT'")), Duration::from_secs(60)), Login::In);
        assert!(started.elapsed() < Duration::from_secs(20), "a grandchild on its stdout does not hold the check: {:?}", started.elapsed());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
