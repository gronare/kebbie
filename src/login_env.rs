use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const WANTED: [&str; 3] = ["SSH_AUTH_SOCK", "GH_TOKEN", "GITHUB_TOKEN"];
const TIMEOUT: Duration = Duration::from_secs(5);

pub fn missing(current: impl Fn(&str) -> Option<String>, login: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in login.lines() {
        let Some((name, value)) = line.split_once('=') else { continue };
        if WANTED.contains(&name) && current(name).is_none_or(|own| own.is_empty()) && !value.is_empty() && !found.iter().any(|(seen, _): &(String, String)| seen == name) {
            found.push((name.to_string(), value.to_string()));
        }
        if name == "PATH" && !found.iter().any(|(seen, _)| seen == "PATH") {
            let own = current("PATH").unwrap_or_default();
            let mut entries: Vec<&str> = own.split(':').filter(|entry| !entry.is_empty()).collect();
            let before = entries.len();
            for entry in value.split(':').filter(|entry| !entry.is_empty()) {
                if !entries.contains(&entry) {
                    entries.push(entry);
                }
            }
            if entries.len() > before {
                found.push(("PATH".to_string(), entries.join(":")));
            }
        }
    }
    found
}

fn needed(current: impl Fn(&str) -> Option<String>) -> bool {
    current("SSH_AUTH_SOCK").is_none_or(|value| value.is_empty())
}

fn login_env(shell: &str) -> Option<String> {
    let mut command = Command::new(shell);
    command.args(["-lic", "env"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    // An interactive shell that shares the board's terminal is stopped by it until the timeout; its own session has no terminal to wait on.
    // SAFETY: setsid is async-signal-safe and touches nothing but the child.
    unsafe { command.pre_exec(|| if libc::setsid() == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) }) };
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = Command::new("kill").args(["-s", "KILL", "--", &format!("-{}", child.id())]).stderr(Stdio::null()).status();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

// herdr started by launchd or systemd hands its panes no SSH agent or tokens, so ssh fetch and push fail; the login shell has them, and they stay in this process only.
pub fn import() {
    let current = |name: &str| std::env::var(name).ok();
    if !needed(current) {
        return;
    }
    let Some(shell) = std::env::var("SHELL").ok().filter(|shell| !shell.is_empty()) else { return };
    let Some(login) = login_env(&shell) else { return };
    for (name, value) in missing(current, &login) {
        // SAFETY: called at the start of a command, before kebbie starts a thread.
        unsafe { std::env::set_var(name, value) };
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_names_this_process_lacks_are_taken_and_path_only_gains_entries() {
        let login = "HOME=/home/ada\nSSH_AUTH_SOCK=/tmp/agent.sock\nGH_TOKEN=gho_login\nGITHUB_TOKEN=\nPATH=/opt/homebrew/bin:/usr/bin:/home/ada/.cargo/bin\nEDITOR=nvim\n";
        let own = |name: &str| match name {
            "GH_TOKEN" => Some("gho_own".to_string()),
            "PATH" => Some("/usr/bin:/bin".to_string()),
            _ => None,
        };
        assert_eq!(missing(own, login), [
            ("SSH_AUTH_SOCK".to_string(), "/tmp/agent.sock".to_string()),
            ("PATH".to_string(), "/usr/bin:/bin:/opt/homebrew/bin:/home/ada/.cargo/bin".to_string()),
        ], "GH_TOKEN is kept as it is, an empty GITHUB_TOKEN is not taken, HOME and EDITOR are not wanted");
        let full = |name: &str| Some(match name { "PATH" => "/opt/homebrew/bin:/usr/bin:/home/ada/.cargo/bin", _ => "set" }.to_string());
        assert!(missing(full, login).is_empty());
        assert!(!needed(full) && needed(own));
        let agent_without_tokens = |name: &str| (name == "SSH_AUTH_SOCK").then(|| "/tmp/agent.sock".to_string());
        assert!(!needed(agent_without_tokens), "missing tokens alone never cost a login shell on every command");
    }

    #[test]
    fn the_login_shell_is_asked_with_a_timeout_and_nothing_is_written_to_disk() {
        let printed = login_env("sh").unwrap_or_default();
        assert!(printed.lines().any(|line| line.starts_with("PATH=")), "{printed}");
        let source = include_str!("login_env.rs");
        let body = &source[source.find("pub fn import").unwrap()..source.find("#[cfg(test)]").unwrap()];
        assert!(!body.contains("fs::") && !body.contains("state::"), "the imported values live in this process only");
    }
}
