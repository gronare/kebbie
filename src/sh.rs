use std::path::Path;
use std::process::{Command, Stdio};

pub type Result<T> = std::result::Result<T, String>;

fn command(program: &str, args: &[&str], cwd: Option<&Path>) -> Command {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    command
}

// The outer error is a program that would not run; the inner one is its stderr when it failed.
pub fn attempt(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<std::result::Result<String, String>> {
    let output = command(program, args, cwd).output().map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        return Ok(Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()));
    }
    Ok(Err(String::from_utf8_lossy(&output.stderr).trim().to_string()))
}

pub fn run(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<String> {
    attempt(program, args, cwd)?.map_err(|stderr| {
        let detail = if stderr.is_empty() { String::new() } else { format!(": {stderr}") };
        format!("{program} {} failed{detail}", args.join(" "))
    })
}

pub fn succeeds(program: &str, args: &[&str], cwd: Option<&Path>) -> bool {
    command(program, args, cwd).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|status| status.success())
}

pub fn run_visibly(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<()> {
    let status = command(program, args, cwd).stdin(Stdio::inherit()).status().map_err(|error| format!("{program}: {error}"))?;
    if status.success() { Ok(()) } else { Err(format!("{program} {} failed", args.join(" "))) }
}
