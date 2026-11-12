use std::path::Path;
use std::process::{Command, Stdio};

pub type Result<T> = std::result::Result<T, String>;

pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

// Every git kebbie runs reads objects as they are stored, so what it checks is what a push sends.
pub fn git_command() -> Command {
    let mut command = Command::new("git");
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    command
}

fn command(program: &str, args: &[&str], cwd: Option<&Path>) -> Command {
    let mut command = if program == "git" { git_command() } else { Command::new(program) };
    #[cfg(test)]
    spawned::note(program, args);
    if program == "git" {
        command.args(crate::repo::unhooked(cwd, args));
    } else {
        command.args(args);
    }
    command.stdin(Stdio::null());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    command
}

// The work of each item on up to `workers` threads, the results in the items' order.
pub fn in_parallel<T: Sync, R: Send>(items: &[T], workers: usize, work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let mut done: Vec<(usize, R)> = std::thread::scope(|scope| {
        let spawned: Vec<_> = (0..items.len().min(workers.max(1))).map(|_| scope.spawn(|| {
            let mut own = Vec::new();
            loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(index) else { return own };
                own.push((index, work(item)));
            }
        })).collect();
        spawned.into_iter().flat_map(|worker| worker.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))).collect()
    });
    done.sort_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, result)| result).collect()
}

// The outer error is a program that would not run; the inner one is its stderr when it failed.
pub fn attempt(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<std::result::Result<String, String>> {
    let output = command(program, args, cwd).output().map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        return Ok(Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()));
    }
    Ok(Err(String::from_utf8_lossy(&output.stderr).trim().to_string()))
}

// Output as the program wrote it, for formats where a leading space is part of the first line.
pub fn run_untrimmed(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let output = command(program, args, cwd).output().map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    Err(format!("{program} {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()))
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

#[derive(Debug, PartialEq)]
pub struct Timed {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

const DRAIN: std::time::Duration = std::time::Duration::from_millis(500);

// None once it outlives the timeout, its process group stopped; a grandchild that keeps its pipes open is not waited for.
pub fn output_with_timeout(mut command: Command, timeout: std::time::Duration) -> std::io::Result<Option<Timed>> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn()?;
    let (sender, received) = std::sync::mpsc::channel();
    let pipes: [Option<Box<dyn Read + Send>>; 2] = [child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>), child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>)];
    for (index, pipe) in pipes.into_iter().enumerate() {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            let _ = sender.send((index, String::from_utf8_lossy(&bytes).to_string()));
        });
    }
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = Command::new("kill").args(["-s", "KILL", "--", &format!("-{}", child.id())]).stderr(Stdio::null()).status();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    let mut text = [String::new(), String::new()];
    let until = std::time::Instant::now() + DRAIN;
    for _ in 0..2 {
        let Ok((index, read)) = received.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) else { break };
        text[index] = read;
    }
    let [stdout, stderr] = text;
    Ok(Some(Timed { code: status.code(), stdout, stderr }))
}

// A child kebbie does not wait for itself is waited on by a thread, or it stays a zombie for as long as kebbie runs.
pub fn reap(mut child: std::process::Child) -> u32 {
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    pid
}

// A board that execs a rebuilt kebbie loses the threads reaping its children, so the new image waits on every child it inherited.
pub fn adopt_children() {
    adopt(children_of(std::process::id()));
}

// ps and not pgrep: pgrep leaves out a child that has already exited, which is the zombie this is for.
pub fn children_of(parent: u32) -> Vec<libc::pid_t> {
    listed_children(parent).map(|(children, _)| children).unwrap_or_default()
}

// The ps that lists them is a child too, reaped before the list is read, so its pid is left out.
fn listed_children(parent: u32) -> Option<(Vec<libc::pid_t>, u32)> {
    let lister = Command::new("ps").args(["-axo", "pid=,ppid="]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let own = lister.id();
    let output = lister.wait_with_output().ok().filter(|output| output.status.success())?;
    let children = String::from_utf8_lossy(&output.stdout).lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        let pid: u32 = fields.next()?.parse().ok()?;
        (fields.next()?.parse::<u32>().ok()? == parent && pid != own).then_some(pid as libc::pid_t)
    }).collect();
    Some((children, own))
}

pub fn adopt(pids: Vec<libc::pid_t>) {
    for pid in pids {
        std::thread::spawn(move || {
            // SAFETY: waits on a pid this process listed as its own child.
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
        });
    }
}

#[cfg(test)]
pub fn test_dir(name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("kebbie-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// Every program a test thread starts, by its arguments, so a test can count the gits a read runs.
#[cfg(test)]
pub mod spawned {
    use std::cell::RefCell;

    thread_local!(static RUNS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) });

    pub fn note(program: &str, args: &[&str]) {
        RUNS.with(|runs| runs.borrow_mut().push(format!("{program} {}", args.join(" "))));
    }

    pub fn gits_during(work: impl FnOnce()) -> Vec<String> {
        let before = RUNS.with(|runs| runs.borrow().len());
        work();
        RUNS.with(|runs| runs.borrow()[before..].iter().filter_map(|run| run.strip_prefix("git ")).map(str::to_string).collect())
    }
}

pub fn run_visibly(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<()> {
    let status = command(program, args, cwd).stdin(Stdio::inherit()).status().map_err(|error| format!("{program}: {error}"))?;
    if status.success() { Ok(()) } else { Err(format!("{program} {} failed", args.join(" "))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_children_listed_for_adoption_leave_out_the_listing_process() {
        let (children, lister) = listed_children(std::process::id()).unwrap();
        assert!(!children.contains(&(lister as libc::pid_t)), "the ps that listed {children:?} is not among them");
    }

    #[test]
    fn every_git_reads_objects_without_replacements() {
        let dir = test_dir("no-replace");
        let git = |args: &[&str]| run("git", &[&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"], args].concat(), Some(&dir)).unwrap();
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "stored"]);
        let stored = git(&["rev-parse", "HEAD"]);
        git(&["commit", "-q", "--allow-empty", "-m", "other"]);
        let other = git(&["rev-parse", "HEAD"]);
        git(&["replace", &stored, &other]);
        assert_eq!(git(&["log", "-1", "--format=%s", &stored]), "stored");
        let direct = git_command().args(["log", "-1", "--format=%s", &stored]).current_dir(&dir).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&direct.stdout).trim(), "stored");
    }

    #[test]
    fn a_program_that_will_not_start_is_the_outer_error_and_a_failing_one_the_inner() {
        let missing = attempt("/no/such/program-kebbie", &[], None);
        assert!(missing.is_err() && missing.unwrap_err().starts_with("/no/such/program-kebbie: "));
        assert_eq!(attempt("sh", &["-c", "echo out; echo err >&2; exit 3"], None), Ok(Err("err".to_string())));
        assert_eq!(attempt("sh", &["-c", "printf '  out  \\n'"], None), Ok(Ok("out".to_string())), "stdout is trimmed");
        assert_eq!(run("sh", &["-c", "exit 1"], None).unwrap_err(), "sh -c exit 1 failed");
        assert_eq!(run("sh", &["-c", "echo why >&2; exit 1"], None).unwrap_err(), "sh -c echo why >&2; exit 1 failed: why");
        assert!(succeeds("true", &[], None) && !succeeds("false", &[], None) && !succeeds("/no/such/program-kebbie", &[], None));
    }

    // A zombie still answers kill(pid, 0); only a reaped child is gone.
    fn gone(pid: u32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) != 0 }
    }

    #[test]
    fn a_detached_child_is_reaped_once_it_exits() {
        let pid = reap(Command::new("true").spawn().unwrap());
        let waited = std::time::Instant::now();
        while !gone(pid) && waited.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(gone(pid), "the exited child is still a zombie");
        let dropped = Command::new("true").spawn().unwrap();
        let pid = dropped.id();
        drop(dropped);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!gone(pid), "a dropped Child stays a zombie, which is what reap is for");
        // SAFETY: waits on our own child so the test leaves no zombie behind.
        unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0) };
    }

    #[test]
    fn a_child_whose_handle_is_gone_is_reaped_once_adopted() {
        let orphan = Command::new("true").spawn().unwrap();
        let pid = orphan.id();
        drop(orphan);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!gone(pid), "without a handle the exited child stays a zombie");
        adopt(vec![pid as libc::pid_t]);
        let waited = std::time::Instant::now();
        while !gone(pid) && waited.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(gone(pid), "an adopted child is reaped, as a board renewed by exec does with the children it inherits");
    }

    #[test]
    fn a_child_that_already_exited_is_still_listed_as_a_child_to_adopt() {
        let orphan = Command::new("true").spawn().unwrap();
        let pid = orphan.id();
        drop(orphan);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let listed = children_of(std::process::id());
        // SAFETY: waits on our own child so the test leaves no zombie behind.
        unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0) };
        assert!(listed.contains(&(pid as libc::pid_t)), "a zombie child must be listed, or a board renewed by exec keeps it for life");
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
