use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

#[path = "../src/executable.rs"]
mod executable;
#[path = "../src/test_seal.rs"]
mod test_seal;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/release-smoke.sh");

// A kebbie that keeps each started task as a file, reports it ready at once (resumed once it was started again, or answers the restart with what unprompted holds; working and in its rules check for as many looks as busy says) or holds a Codex task under NEEDS YOU on what codex-asks says, logs every call, and a herdr that closes Spaces.
const KEBBIE: &str = r#"#!/bin/sh
d=$(dirname "$(dirname "$0")")
echo "$*" >> "$d/kebbie.log"
[ "$1" = -C ] && shift 2
case "$1" in
  start)
    if [ -f "$d/tasks/$2" ] && [ -f "$d/unprompted" ]; then cat "$d/unprompted"; elif [ -f "$d/tasks/$2" ]; then echo resumed > "$d/tasks/$2"; echo 'Restarted its Claude'; echo '  next: It resumes its last conversation and has the prompt again.'; else echo started > "$d/tasks/$2"; echo '✓ Started'; fi ;;
  status)
    busy=$(cat "$d/busy" 2>/dev/null || echo 0)
    [ "$busy" -gt 0 ] && echo $((busy - 1)) > "$d/busy" && echo "busy $busy" >> "$d/kebbie.log"
    printf '{"items":['
    sep=
    for file in "$d"/tasks/*; do
      [ -f "$file" ] || continue
      name=$(basename "$file")
      case "$(cat "$file")" in resumed) summary=resumed ;; *) summary='smoke done' ;; esac
      [ -f "$d/never-ready" ] && state=working || state=ready
      group='"READY TO PUSH"' next=null
      case "$name" in smoke-codex-*) [ -f "$d/codex-asks" ] && state=working group='"NEEDS YOU"' next="\"$(cat "$d/codex-asks")\"" ;; esac
      [ "$busy" -gt 0 ] && agent=working rules=checking || agent=idle rules=ok
      printf '%s{"name":"%s","place":"sandbox","workspace":"w_%s","group":%s,"next_for_you":%s,"agent":{"status":"%s"},"git":{"ahead":1,"rules":"%s"},"report":{"state":"%s","summary":"%s"}}' "$sep" "$name" "$name" "$group" "$next" "$agent" "$rules" "$state" "$summary"
      sep=,
    done
    printf ']}\n' ;;
  drop) rm -f "$d/tasks/$2" ;;
  doctor) [ -f "$d/codex" ] && echo 'ok    codex plugin      kebbie'"'"'s plugin at ~/.codex'; echo 'fail  herdr plugin      not registered'; exit 1 ;;
  profiles) echo '[{"name":"task","agent":"claude"},{"name":"second","agent":"codex","permission_mode":"plan"},{"name":"codex-review","agent":"codex","kind":"review"},{"name":"coder","agent":"codex"}]' ;;
  *) echo "the fake kebbie does not know $1" >&2; exit 1 ;;
esac
"#;

struct Sandbox {
    dir: PathBuf,
    repo: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("kebbie-smoke-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("tasks")).unwrap();
        executable::write(dir.join("bin/kebbie"), KEBBIE).unwrap();
        executable::write(dir.join("bin/herdr"), "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$(dirname \"$0\")\")/herdr.log\"\n").unwrap();
        let repo = dir.join("sandbox");
        fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&repo).output().unwrap().status.success());
        git(&["init", "--quiet", "--initial-branch=main"]);
        fs::write(repo.join("README.md"), "sandbox\n").unwrap();
        git(&["add", "README.md"]);
        git(&["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", "first"]);
        Sandbox { dir, repo }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("sh");
        command.arg(SCRIPT).args(args).env("HOME", self.dir.join("home")).env("PATH", format!("{}:/usr/bin:/bin", self.dir.join("bin").display()))
            .env("HERDR_BIN_PATH", self.dir.join("bin/herdr")).env("SMOKE_POLL", "0.1").env("SMOKE_TIMEOUT", "5").stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("kebbie.log")).unwrap_or_default().lines().map(str::to_string).collect()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn said(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn refused(output: &Output, words: &str) {
    assert_eq!(output.status.code(), Some(2), "{}", said(output));
    assert!(said(output).contains(words), "{}", said(output));
}

#[test]
fn the_smoke_refuses_a_folder_that_is_missing_no_git_repo_or_holds_uncommitted_work_and_starts_nothing() {
    let sandbox = Sandbox::new("refusals");
    let help = sandbox.run(&["--help"]);
    assert!(help.status.success() && said(&help).starts_with("usage: scripts/release-smoke.sh [sandbox repo]"), "{}", said(&help));
    refused(&sandbox.run(&["--push"]), "usage:");
    refused(&sandbox.run(&["a", "b"]), "usage:");
    refused(&sandbox.run(&[]), "kebbie-sandbox is not there");
    refused(&sandbox.run(&[sandbox.dir.join("nowhere").to_str().unwrap()]), "nowhere is not there");
    let plain = sandbox.dir.join("plain");
    fs::create_dir_all(&plain).unwrap();
    refused(&sandbox.run(&[plain.to_str().unwrap()]), "plain is not a git repo");
    let inner = sandbox.repo.join("docs");
    fs::create_dir_all(&inner).unwrap();
    refused(&sandbox.run(&[inner.to_str().unwrap()]), "docs is inside ");
    fs::write(sandbox.repo.join("scratch.txt"), "mine\n").unwrap();
    refused(&sandbox.run(&[sandbox.repo.to_str().unwrap()]), "sandbox has uncommitted changes");
    let without = sandbox.command(&[sandbox.repo.to_str().unwrap()]).env("PATH", "/usr/bin:/bin").output().unwrap();
    refused(&without, "kebbie is not on PATH");
    assert!(sandbox.calls().is_empty(), "nothing started: {:?}", sandbox.calls());
}

#[test]
fn the_smoke_runs_claude_then_codex_through_ready_restart_and_resume_drops_both_and_never_pushes() {
    let sandbox = Sandbox::new("green");
    fs::write(sandbox.dir.join("codex"), "").unwrap();
    let output = sandbox.run(&[sandbox.repo.to_str().unwrap()]);
    let shown = said(&output);
    assert!(output.status.success(), "{shown}");
    for row in ["claude   PASS  started smoke-claude-", "claude   PASS  reported ready with 1 commit", "claude   PASS  restarted and resumed its conversation", "codex    PASS  restarted and resumed its conversation", "cleanup  PASS  dropped the smoke tasks"] {
        assert!(shown.contains(row), "{row}: {shown}");
    }
    assert!(shown.ends_with("release-smoke: all green\n"), "{shown}");
    let calls = sandbox.calls();
    let repo = sandbox.repo.canonicalize().unwrap();
    assert!(calls.iter().filter(|call| call.contains(" start ")).all(|call| call.starts_with(&format!("-C {} start smoke-", repo.display()))), "every start names the sandbox: {calls:?}");
    assert!(calls.iter().filter(|call| call.contains(" start ")).all(|call| call.contains(" --no-note")), "a smoke task never makes a vault note: {calls:?}");
    assert!(calls.iter().any(|call| call.contains("start smoke-codex-") && call.ends_with("--profile coder")), "a read-only or review Codex profile never commits, so the smoke takes one that writes: {calls:?}");
    assert_eq!(calls.iter().filter(|call| call.contains(" drop smoke-")).count(), 2, "{calls:?}");
    assert!(!calls.iter().any(|call| call.split_whitespace().any(|word| word == "push")), "{calls:?}");
    assert_eq!(fs::read_to_string(sandbox.dir.join("herdr.log")).unwrap().lines().filter(|line| line.starts_with("workspace close w_smoke-")).count(), 2);
}

#[test]
fn without_kebbies_codex_plugin_the_smoke_skips_codex_and_a_task_that_never_reports_fails_it() {
    let sandbox = Sandbox::new("red");
    fs::write(sandbox.dir.join("never-ready"), "").unwrap();
    let output = sandbox.run(&[sandbox.repo.to_str().unwrap()]);
    let shown = said(&output);
    assert_eq!(output.status.code(), Some(1), "{shown}");
    assert!(shown.contains("claude   FAIL  no ready report within 5s") && shown.contains("codex    PASS  skipped: kebbie's Codex plugin is not installed") && shown.ends_with("release-smoke: FAILED\n"), "{shown}");
    assert!(sandbox.calls().iter().any(|call| call.contains(" drop smoke-claude-")), "the failed task is dropped too");
}

#[test]
fn the_smoke_closes_a_tasks_space_only_once_its_turn_and_rules_check_are_over() {
    let sandbox = Sandbox::new("busy");
    fs::write(sandbox.dir.join("busy"), "4").unwrap();
    let output = sandbox.run(&[sandbox.repo.to_str().unwrap()]);
    let shown = said(&output);
    assert!(output.status.success(), "{shown}");
    let calls = sandbox.calls();
    let restart = calls.iter().rposition(|call| call.contains(" start smoke-claude-")).unwrap();
    assert!(calls[..restart].iter().filter(|call| call.starts_with("busy ")).count() == 4, "the restart came while the task still worked: {calls:?}");
    let still = Sandbox::new("busy-long");
    fs::write(still.dir.join("busy"), "1000").unwrap();
    let output = still.run(&[still.repo.to_str().unwrap()]);
    let shown = said(&output);
    assert!(shown.contains("claude   FAIL  still working or in its rules check after 5s"), "{shown}");
    assert!(!fs::read_to_string(still.dir.join("herdr.log")).unwrap_or_default().contains("workspace close"), "a working task's Space is never closed");
}

#[test]
fn a_restart_that_does_not_resume_shows_kebbies_whole_answer_with_its_error() {
    let sandbox = Sandbox::new("unprompted");
    let answer = "! Restarted, but its prompt did not reach Claude\n  you: Paste the task's prompt into its Claude.\n  task: Kebbie release smoke test (smoke-claude-1)\n  folder: /home/ada/wt1.sandbox\n  error: its input box did not show the prompt kebbie pasted within 5 s";
    fs::write(sandbox.dir.join("unprompted"), answer).unwrap();
    let output = sandbox.run(&[sandbox.repo.to_str().unwrap()]);
    let shown = said(&output);
    assert_eq!(output.status.code(), Some(1), "{shown}");
    assert!(shown.contains("claude   FAIL  restart did not resume: ! Restarted, but its prompt did not reach Claude"), "{shown}");
    assert!(shown.contains("error: its input box did not show the prompt kebbie pasted within 5 s"), "the row is cut at 120 characters, so the error kebbie named must be shown in full: {shown}");
}

#[test]
fn a_task_under_needs_you_fails_at_once_with_what_it_waits_for_and_is_dropped() {
    let sandbox = Sandbox::new("asks");
    fs::write(sandbox.dir.join("codex"), "").unwrap();
    fs::write(sandbox.dir.join("codex-asks"), "answer Codex in its Space: it asks whether you trust its folder").unwrap();
    let output = sandbox.command(&[sandbox.repo.to_str().unwrap()]).env("SMOKE_TIMEOUT", "60").output().unwrap();
    let shown = said(&output);
    assert_eq!(output.status.code(), Some(1), "{shown}");
    assert!(!shown.contains("within 60s"), "the smoke waited out its timeout: {shown}");
    assert!(shown.contains("codex    FAIL  waits on you: answer Codex in its Space: it asks whether you trust its folder"), "{shown}");
    assert!(shown.contains("claude   PASS  restarted and resumed its conversation") && shown.ends_with("release-smoke: FAILED\n"), "{shown}");
    assert!(sandbox.calls().iter().any(|call| call.contains(" drop smoke-codex-")), "the task that waits is dropped too");
}

#[test]
fn ctrl_c_drops_the_task_the_smoke_started() {
    let sandbox = Sandbox::new("interrupt");
    fs::write(sandbox.dir.join("never-ready"), "").unwrap();
    let child = sandbox.command(&[sandbox.repo.to_str().unwrap()]).env("SMOKE_TIMEOUT", "60").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let begun = Instant::now();
    while !sandbox.calls().iter().any(|call| call.starts_with("status --json")) && begun.elapsed() < Duration::from_secs(120) {
        std::thread::sleep(Duration::from_millis(50));
    }
    // SAFETY: a signal to the child this test spawned.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) };
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", said(&output));
    assert!(said(&output).contains("release-smoke: stopped"), "{}", said(&output));
    assert!(sandbox.calls().iter().any(|call| call.contains(" drop smoke-claude-") && call.ends_with("--force")), "{:?}", sandbox.calls());
    assert!(fs::read_dir(sandbox.dir.join("tasks")).unwrap().next().is_none(), "no smoke task is left");
}
