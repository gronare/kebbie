use std::path::Path;
use std::process::{Command, Stdio};

pub type Result<T> = std::result::Result<T, String>;

pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_will_not_start_is_the_outer_error_and_a_failing_one_the_inner() {
        let missing = attempt("/no/such/program-kelpie", &[], None);
        assert!(missing.is_err() && missing.unwrap_err().starts_with("/no/such/program-kelpie: "));
        assert_eq!(attempt("sh", &["-c", "echo out; echo err >&2; exit 3"], None), Ok(Err("err".to_string())));
        assert_eq!(attempt("sh", &["-c", "printf '  out  \\n'"], None), Ok(Ok("out".to_string())), "stdout is trimmed");
        assert_eq!(run("sh", &["-c", "exit 1"], None).unwrap_err(), "sh -c exit 1 failed");
        assert_eq!(run("sh", &["-c", "echo why >&2; exit 1"], None).unwrap_err(), "sh -c echo why >&2; exit 1 failed: why");
        assert!(succeeds("true", &[], None) && !succeeds("false", &[], None) && !succeeds("/no/such/program-kelpie", &[], None));
    }

    #[test]
    fn arguments_reach_the_program_as_they_are_with_no_shell_in_between() {
        let output = run("printf", &["%s|", "a b", "$(id)", "`id`", "; ls", "'q'", "\"q\""], None).unwrap();
        assert_eq!(output, "a b|$(id)|`id`|; ls|'q'|\"q\"|");
        let dir = std::env::temp_dir();
        assert_eq!(run("pwd", &[], Some(&dir)).unwrap(), dir.canonicalize().unwrap().display().to_string());
        assert!(run("cat", &[], None).unwrap().is_empty(), "stdin is closed, so a program reading it never hangs");
    }
}
