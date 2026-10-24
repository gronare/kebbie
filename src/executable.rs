use std::fs;
use std::io::{Error, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

// A child process writes the file, never a descriptor of this one: a fork in another thread inherits every open descriptor until it execs, and Linux refuses to run a file open for writing anywhere (ETXTBSY).
pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let path = path.as_ref();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| Error::other(format!("{} names no file", path.display())))?.to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let mut child = Command::new("/bin/sh").args(["-c", "exec cat > \"$1\"", "sh"]).arg(&temp).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
    let fed = child.stdin.take().expect("a piped stdin").write_all(contents.as_ref());
    let output = child.wait_with_output();
    let written = match (fed, output) {
        (Ok(()), Ok(output)) if output.status.success() => fs::set_permissions(&temp, fs::Permissions::from_mode(0o755)).and_then(|()| fs::rename(&temp, path)),
        (Ok(()), Ok(output)) => Err(Error::other(format!("{}: {}", temp.display(), String::from_utf8_lossy(&output.stderr).trim()))),
        (Err(error), _) | (_, Err(error)) => Err(error),
    };
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-executable-{name}-{:?}", std::thread::current().id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_executable_runs_at_once_and_replacing_it_leaves_a_running_copy_alone() {
        let dir = scratch("runs");
        let (file, started) = (dir.join("fake"), dir.join("started"));
        write(&file, format!("#!/bin/sh\ntouch '{}'\nsleep 5\n", started.display())).unwrap();
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o755);
        let mut running = Command::new(&file).spawn().unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !started.exists() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(started.exists(), "the first copy runs");
        write(&file, "#!/bin/sh\necho second\n").unwrap();
        let said = Command::new(&file).output().unwrap();
        let still = running.try_wait().unwrap();
        let _ = running.kill();
        let _ = running.wait();
        assert_eq!(still, None, "the copy that was running when the file was replaced runs on");
        assert_eq!(String::from_utf8_lossy(&said.stdout), "second\n");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2, "no temp file is left beside it");
        let _ = fs::remove_dir_all(dir);
    }

    fn executable_mode(line: &str, after: &str) -> bool {
        line.match_indices(after).any(|(at, found)| {
            let digits: String = line[at + found.len()..].chars().take_while(|character| character.is_digit(8)).collect();
            u32::from_str_radix(&digits, 8).is_ok_and(|mode| mode & 0o111 != 0)
        })
    }

    fn first_argument(line: &str, call: &str) -> Option<String> {
        let rest = &line[line.find(call)? + call.len()..];
        let mut depth = 0;
        let end = rest.char_indices().find(|&(_, character)| match character {
            '(' => { depth += 1; false }
            ')' if depth == 0 => true,
            ')' => { depth -= 1; false }
            ',' => depth == 0,
            _ => false,
        })?.0;
        Some(rest[..end].trim().trim_start_matches('&').to_string())
    }

    // Test code that writes a file and gives it an execute bit, or copies one, does it through this process's own descriptor, which a fork in another test thread can hold open as it runs.
    fn offenders(source: &str, all_test: bool) -> Vec<usize> {
        let start = if all_test { 0 } else { source.lines().position(|line| line.trim() == "#[cfg(test)]").unwrap_or(usize::MAX) };
        let lines: Vec<&str> = source.lines().collect();
        let tested = lines.get(start..).unwrap_or_default();
        let written = |target: &str| tested.iter().any(|line| ["write(", "write(&", "create(", "create(&"].iter().any(|call| {
            let call = format!("{call}{target}");
            line.match_indices(call.as_str()).any(|(at, _)| line[at + call.len()..].starts_with([',', ')']))
        }));
        let data = [".json\"", ".jsonl\"", ".md\"", ".toml\"", ".txt\"", ".log\""];
        tested.iter().enumerate().filter(|(_, line)| {
            let chmod = executable_mode(line, "from_mode(0o") && first_argument(line, "set_permissions(").is_some_and(|target| written(&target));
            let opened = executable_mode(line, ".mode(0o") && !line.contains("DirBuilder");
            let copied = line.contains("fs::copy(") && !data.iter().any(|extension| line.contains(extension));
            chmod || opened || copied
        }).map(|(number, _)| start + number + 1).collect()
    }

    #[test]
    fn the_guard_catches_a_fake_written_or_copied_in_place_and_lets_folders_and_data_through() {
        let source = "fn production() {\n    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();\n}\n#[cfg(test)]\nmod tests {\n    fs::write(&fake, \"#!/bin/sh\\n\").unwrap();\n    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();\n    fs::create_dir_all(&locked).unwrap();\n    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();\n    fs::copy(KEBBIE, &binary).unwrap();\n    fs::copy(dir.join(\"s.jsonl\"), &to).unwrap();\n}\n";
        assert_eq!(offenders(source, false), [7, 10]);
        assert_eq!(offenders("fs::write(dirs.git_dir(), x).unwrap();\nfs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();\n", true), Vec::<usize>::new());
        assert_eq!(offenders("fs::write(bin.join(tool), x).unwrap();\nfs::set_permissions(bin.join(tool), fs::Permissions::from_mode(0o755)).unwrap();\n", true), [2]);
    }

    #[test]
    fn every_executable_the_tests_write_goes_through_this_file() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut found = Vec::new();
        let mut files = vec![manifest.join("src"), manifest.join("tests")];
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(fs::read_dir(&path).unwrap().map(|entry| entry.unwrap().path()));
            } else if path.extension().is_some_and(|extension| extension == "rs") && path != manifest.join("src/executable.rs") {
                let all_test = path.starts_with(manifest.join("tests"));
                found.extend(offenders(&fs::read_to_string(&path).unwrap(), all_test).into_iter().map(|line| format!("{}:{line}", path.strip_prefix(manifest).unwrap().display())));
            }
        }
        assert!(found.is_empty(), "write and copy executables in tests with executable::write: {found:?}");
    }
}
