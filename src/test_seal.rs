use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const KEPT: [&str; 4] = ["PATH", "USER", "LOGNAME", "LANG"];
const KEPT_PREFIXES: [&str; 3] = ["LC_", "RUST_", "CARGO_"];

static ROOT: OnceLock<PathBuf> = OnceLock::new();

// Runs before main, while the test binary has one thread, so no test ever sees the environment it was started in.
#[used]
#[cfg_attr(target_os = "linux", unsafe(link_section = ".init_array"))]
#[cfg_attr(target_os = "macos", unsafe(link_section = "__DATA,__mod_init_func"))]
static SEAL_AT_START: extern "C" fn() = seal_at_start;

extern "C" fn seal_at_start() {
    let root = std::env::temp_dir().canonicalize().unwrap_or_else(|_| PathBuf::from("/tmp")).join(format!("kelpie-sealed-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for dir in ["home", "state", "tmp", "bin"] {
        fs::create_dir_all(root.join(dir)).expect("the sealed test root");
    }
    fs::write(root.join("gitconfig"), "").expect("the sealed git config");
    fake_ps(&root.join("bin/ps"));
    let path = std::env::var("PATH").unwrap_or_default();
    // SAFETY: before main, so no other thread reads the environment.
    unsafe {
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if !KEPT.contains(&name.as_ref()) && !KEPT_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
                std::env::remove_var(name.as_ref());
            }
        }
        for (name, value) in sealed_vars(&root, &path) {
            std::env::set_var(name, value);
        }
        libc::atexit(remove_root);
    }
    let _ = ROOT.set(root);
}

fn sealed_vars(root: &Path, path: &str) -> Vec<(&'static str, String)> {
    let at = |part: &str| root.join(part).display().to_string();
    vec![
        ("PATH", format!("{}:{path}", at("bin"))),
        ("HOME", at("home")),
        ("TMPDIR", at("tmp")),
        ("KELPIE_STATE_DIR", at("state")),
        ("HERDR_BIN_PATH", "false".to_string()),
        ("HERDR_SOCKET_PATH", at("herdr.sock")),
        ("KELPIE_GH_BIN", "false".to_string()),
        ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
        ("GIT_CONFIG_GLOBAL", at("gitconfig")),
        ("GIT_CEILING_DIRECTORIES", root.display().to_string()),
        ("GIT_TERMINAL_PROMPT", "0".to_string()),
    ]
}

// The process table caller detection reads: only the caller's own chain up to this test binary, which stands as a root, so a Claude another test started beside it is never seen.
fn fake_ps(file: &Path) {
    let script = format!(
        r#"#!/bin/sh
if [ "$*" != "-A -o pid=,ppid=,args=" ]; then exec /bin/ps "$@"; fi
/bin/ps "$@" | awk -v root={} -v caller=$PPID '{{ parent[$1] = $2; line[$1] = $0 }} END {{ for (pid = caller; pid in line; pid = parent[pid]) {{ $0 = line[pid]; if (pid == root) {{ $2 = 1; printf "%s%s\n", chain, $0; exit }} chain = chain $0 "\n" }} }}'
"#,
        std::process::id()
    );
    fs::write(file, script).expect("the sealed ps");
    fs::set_permissions(file, fs::Permissions::from_mode(0o755)).expect("the sealed ps");
}

extern "C" fn remove_root() {
    if let Some(root) = ROOT.get() {
        stop_keepers_under(root);
        let _ = fs::remove_dir_all(root);
    }
}

// A keeper runs detached and only sees its lock file gone between sleeps, so each one a test started is stopped here: a TERM, then a KILL for one that does not end.
pub fn stop_keepers_under(dir: &Path) {
    let pids: Vec<libc::pid_t> = locks_under(dir).iter().filter_map(|lock| holder(lock)).collect();
    let alive = |pid: &libc::pid_t| unsafe { libc::kill(*pid, 0) } == 0;
    for pid in &pids {
        // SAFETY: a signal to the pid that holds a keeper lock inside the sealed root.
        unsafe { libc::kill(*pid, libc::SIGTERM) };
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while pids.iter().any(alive) && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for pid in pids.iter().filter(|pid| alive(pid)) {
        // SAFETY: as above, for a keeper that outlived its TERM.
        unsafe { libc::kill(*pid, libc::SIGKILL) };
    }
}

fn locks_under(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries.flatten().flat_map(|entry| match entry.file_type() {
        Ok(kind) if kind.is_dir() => locks_under(&entry.path()),
        Ok(kind) if kind.is_file() && entry.file_name() == "keeper.lock" => vec![entry.path()],
        _ => Vec::new(),
    }).collect()
}

// The pid a keeper wrote into its lock, while the lock is still held.
fn holder(lock: &Path) -> Option<libc::pid_t> {
    use std::os::fd::AsRawFd;
    let text = fs::read_to_string(lock).ok()?;
    let file = fs::File::open(lock).ok()?;
    // SAFETY: probes the lock through this function's own descriptor, which closing releases.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
        return None;
    }
    let digits: String = text.split("\"pid\":").nth(1)?.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

pub fn root() -> &'static Path {
    ROOT.get().expect("the test binary was sealed at start")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn the_test_binary_reaches_no_real_herdr_state_home_temp_or_git_config() {
        let root = root();
        for name in ["HOME", "TMPDIR", "KELPIE_STATE_DIR", "HERDR_SOCKET_PATH", "GIT_CONFIG_GLOBAL"] {
            let value = std::env::var(name).unwrap_or_default();
            assert!(Path::new(&value).starts_with(root), "{name}={value} is outside the sealed root {}", root.display());
        }
        assert_eq!(std::env::var("HERDR_BIN_PATH").as_deref(), Ok("false"));
        assert_eq!(std::env::var("GIT_CONFIG_NOSYSTEM").as_deref(), Ok("1"));
        for (name, _) in std::env::vars() {
            assert!(!name.starts_with("HERDR_") || ["HERDR_BIN_PATH", "HERDR_SOCKET_PATH"].contains(&name.as_str()), "{name} reaches the test from the shell it ran in");
            assert!(!name.starts_with("GIT_CONFIG_") || ["GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_GLOBAL"].contains(&name.as_str()), "{name} reaches the test from the shell it ran in");
            assert!(!["CLAUDE_CONFIG_DIR", "KELPIE_CONFIG", "VAULT_PATH", "TERM_PROGRAM", "XDG_CONFIG_HOME"].contains(&name.as_str()), "{name} reaches the test from the shell it ran in");
        }
        assert_eq!(std::env::temp_dir(), root.join("tmp"));
    }

    #[test]
    fn a_new_folder_in_the_temp_dir_is_outside_any_repo() {
        let dir = std::env::temp_dir().join(format!("outside-{:?}", std::thread::current().id()));
        fs::create_dir_all(&dir).unwrap();
        let found = Command::new("git").args(["rev-parse", "--git-dir"]).current_dir(&dir).output().unwrap();
        assert!(!found.status.success(), "git found a repo above the sealed temp dir: {}", String::from_utf8_lossy(&found.stdout));
        let config = Command::new("git").args(["config", "--list", "--show-origin"]).current_dir(&dir).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&config.stdout), "", "git reads no config but the sealed one, which is empty");
    }

    #[test]
    fn the_process_table_is_the_callers_own_chain_ending_at_this_test_binary() {
        let mut beside = Command::new("sleep").arg("30").spawn().unwrap();
        let table = Command::new("ps").args(["-A", "-o", "pid=,ppid=,args="]).output().unwrap();
        let _ = beside.kill();
        let _ = beside.wait();
        let table = String::from_utf8_lossy(&table.stdout);
        let rows: Vec<Vec<&str>> = table.lines().map(|line| line.split_whitespace().collect()).collect();
        assert_eq!(rows.len(), 1, "only this test binary, not the process beside it nor a Claude above it: {table}");
        assert_eq!(rows[0][..2], [std::process::id().to_string().as_str(), "1"], "{table}");
    }

    #[test]
    fn every_test_binary_is_sealed() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let main = fs::read_to_string(manifest.join("src/main.rs")).unwrap();
        assert!(main.contains("#[cfg(test)]\nmod test_seal;"), "the unit tests are sealed");
        for entry in fs::read_dir(manifest.join("tests")).unwrap() {
            let file = entry.unwrap().path();
            if file.extension().is_some_and(|extension| extension == "rs") {
                let source = fs::read_to_string(&file).unwrap();
                assert!(source.contains("#[path = \"../src/test_seal.rs\"]\nmod test_seal;"), "{} is not sealed", file.display());
            }
        }
    }
}
