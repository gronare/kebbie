use std::path::{Path, PathBuf};

use crate::repo::{main_checkout_of, name_of};
use crate::sh::Result;

pub fn dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("kebbie")
}

pub fn path() -> PathBuf {
    std::env::var("KEBBIE_CONFIG").map(PathBuf::from).unwrap_or_else(|_| dir().join("config.toml"))
}

pub fn repos_dir() -> PathBuf {
    std::env::var("KEBBIE_REPOS_DIR").map(PathBuf::from).unwrap_or_else(|_| dir().join("repos"))
}

// A worktree shares its main checkout's file, since the file is named after the main checkout's folder.
pub fn repo_file_in(dir: &Path, repo: &Path) -> PathBuf {
    let main = main_checkout_of(repo).unwrap_or_else(|| repo.to_path_buf());
    dir.join(format!("{}.toml", name_of(&main)))
}

pub fn repo_file(repo: &Path) -> PathBuf {
    repo_file_in(&repos_dir(), repo)
}

pub fn shown(path: &Path) -> String {
    let found = if path.exists() { "found" } else { "not found" };
    match std::fs::read_link(path) {
        Ok(target) => format!("{} -> {} ({found})", path.display(), path.parent().map_or(target.clone(), |parent| parent.join(&target)).display()),
        Err(_) => format!("{} ({found})", path.display()),
    }
}

pub(super) fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}
