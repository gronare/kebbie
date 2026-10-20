use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::sh::Result;

// kelpie's state holds messages, prompts and push tokens, so its folders are the user's alone.
pub fn private_dir(dir: &Path) -> Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|error| format!("{}: {error}", dir.display()))
}

// A state folder made before kelpie made them private is closed on its next use.
pub fn close_up(dir: &Path) -> Result<()> {
    private_dir(dir)?;
    let mode = fs::metadata(dir).map_err(|error| format!("{}: {error}", dir.display()))?.permissions().mode();
    if mode & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    Ok(())
}

// Written beside the file and renamed over it, so the board's refresher never reads half of one.
pub fn write(file: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let dir = file.parent().filter(|dir| !dir.as_os_str().is_empty()).ok_or_else(|| format!("{} has no folder", file.display()))?;
    private_dir(dir)?;
    let name = file.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    let temp = dir.join(format!(".{name}.{}-{}.tmp", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)));
    let written = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)
        .and_then(|mut out| out.write_all(contents.as_ref()))
        .and_then(|_| fs::rename(&temp, file));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written.map_err(|error| format!("{}: {error}", file.display()))
}

// A log a process writes its output to, started empty and private.
pub fn create(file: &Path) -> std::io::Result<fs::File> {
    if let Some(dir) = file.parent() {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(file)
}

// An append-only log, created private.
pub fn append_to(file: &Path) -> std::io::Result<fs::File> {
    if let Some(dir) = file.parent() {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    OpenOptions::new().create(true).append(true).mode(0o600).open(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_files_are_private_and_replaced_whole() {
        let root = std::env::temp_dir().join(format!("kelpie-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let file = root.join("approved/billing@app");
        write(&file, "one").unwrap();
        write(&file, "two").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "two");
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(root.join("approved")).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::read_dir(root.join("approved")).unwrap().count(), 1, "no temp file is left behind");
        let log = root.join("messages.jsonl");
        drop(append_to(&log).unwrap());
        assert_eq!(fs::metadata(&log).unwrap().permissions().mode() & 0o777, 0o600);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        close_up(&root).unwrap();
        assert_eq!(fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700, "an old world-readable state dir is closed");
        fs::remove_dir_all(root).unwrap();
    }
}
