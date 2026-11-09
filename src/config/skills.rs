use std::path::{Component, Path, PathBuf};

use toml::Table;

use super::text;
use crate::sh::Result;

// A path a repo config names stays inside the repo: relative, with no .. in it.
pub fn inside(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty() && path.components().all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

// A skill is a folder name, after its plugin's name and a colon or alone, so it never walks out of a skills folder.
pub fn skill_name(name: &str) -> bool {
    let (plugin, skill) = name.split_once(':').unwrap_or(("own", name));
    [plugin, skill].iter().all(|part| !part.is_empty() && *part != "." && *part != ".." && !part.contains(['/', '\\', ':']))
}

// A skill setting may name a Markdown file instead, by an absolute path or one under ~.
pub fn skill_path(value: &str) -> Option<PathBuf> {
    let path = match value.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").ok().filter(|home| !home.is_empty())?).join(rest),
        None if value.starts_with('/') => PathBuf::from(value),
        None => return None,
    };
    Some(path)
}

pub fn quoted_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map(|text| format!("--- {} ---\n{}\n--- end of {} ---", path.display(), text.trim_end(), path.display())).map_err(|error| format!("{}: {error}", path.display()))
}

fn markdown(value: &str) -> bool {
    (value.starts_with('/') || value.starts_with("~/")) && Path::new(value).extension().is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

pub fn follow(value: &str) -> String {
    match skill_path(value) {
        Some(path) => format!("read `{}` and follow it", path.display()),
        None => format!("run the skill `{value}` and follow it"),
    }
}

pub fn rules_named(value: &str) -> String {
    match skill_path(value) {
        Some(path) => format!("the rules in `{}`", path.display()),
        None => format!("the skill `{value}`"),
    }
}

pub(super) fn skill(table: &Table, field: &str, key: &str) -> Result<Option<String>> {
    let name = text(table, field, key)?;
    match name {
        Some(name) if !skill_name(&name) && !markdown(&name) => Err(format!("{key}: \"{name}\" is not a skill name or a Markdown file; use <skill>, <plugin>:<skill>, or a path to a .md file starting with / or ~/")),
        Some(name) if markdown(&name) && skill_path(&name).is_none() => Err(format!("{key}: \"{name}\" starts with ~/, and HOME is not set to say whose home that is; name the file from /")),
        name => Ok(name),
    }
}
