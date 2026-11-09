use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::lead::state_dir;
use crate::sh::Result;

// One sender at a time, so a held message is never delivered twice; the kernel drops the flock when its holder dies, so a crash leaves nothing to steal.
pub struct Lock(File, PathBuf);

thread_local! {
    static HELD: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn lock_in(dir: &Path) -> PathBuf {
    dir.join("messages.lock")
}

// An append under a lock this thread already holds must not wait for itself.
pub(super) fn holds(dir: &Path) -> bool {
    HELD.with(|held| held.borrow().contains(&lock_in(dir)))
}

impl Lock {
    pub fn take(wait: bool) -> Result<Option<Lock>> {
        Lock::take_in(&state_dir()?, wait)
    }

    pub fn take_in(dir: &Path, wait: bool) -> Result<Option<Lock>> {
        let path = lock_in(dir);
        let file = OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        for _ in 0..100 {
            // SAFETY: flock on a descriptor this File owns.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                HELD.with(|held| held.borrow_mut().push(path.clone()));
                return Ok(Some(Lock(file, path)));
            }
            if !wait {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err("Another kebbie is holding the message log; try again".to_string())
    }
}

impl Drop for Lock {
    // A child forked by another thread holds a copy of the descriptor until it execs; unlocking releases the lock for every copy at once.
    fn drop(&mut self) {
        HELD.with(|held| held.borrow_mut().retain(|path| path != &self.1));
        // SAFETY: unlocks the descriptor this value owns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}
