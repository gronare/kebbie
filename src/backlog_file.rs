use std::path::{Path, PathBuf};

use crate::backlog::{Entry, PRIORITIES};
use crate::sh::Result;

pub const PREFIX: &str = "local:";
pub const FORMAT: &str = "- [priority] Title — one-line summary (area)";
const HEADER: &str = "<!-- kelpie's backlog: one item per line, - [urgent|high|medium|low] Title — one-line summary (area); the priority, summary and area are optional, and other lines are left as they are. -->\n\n";

pub fn path() -> PathBuf {
    crate::config::path().with_file_name("backlog.md")
}

pub fn reference(title: &str) -> String {
    format!("{PREFIX}{title}")
}

pub fn parse(line: &str) -> Option<Entry> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* "))?.trim();
    let (priority, rest) = match rest.strip_prefix('[').and_then(|inner| inner.split_once(']')) {
        Some((word, after)) if PRIORITIES.contains(&word.trim().to_lowercase().as_str()) => (word.trim().to_lowercase(), after.trim()),
        _ => ("none".to_string(), rest),
    };
    // The area is a repo's folder name, so a parenthesis with a space in it stays part of the text.
    let (rest, area) = match rest.strip_suffix(')').and_then(|inner| inner.rsplit_once(" (")) {
        Some((before, area)) if !area.is_empty() && !area.contains(char::is_whitespace) => (before.trim(), area),
        _ => (rest, ""),
    };
    let (title, summary) = rest.split_once(" — ").or_else(|| rest.split_once(" -- ")).unwrap_or((rest, ""));
    let title = title.trim();
    (!title.is_empty()).then(|| Entry {
        source: "local".to_string(),
        title: title.to_string(),
        priority,
        area: area.to_string(),
        reference: reference(title),
        summary: summary.trim().to_string(),
        age: String::new(),
    })
}

pub fn entries(text: &str) -> Vec<Entry> {
    text.lines().filter_map(parse).collect()
}

pub fn read(path: &Path) -> Vec<Entry> {
    std::fs::read_to_string(path).map(|text| entries(&text)).unwrap_or_default()
}

fn line(priority: &str, item: &str) -> String {
    if priority == "none" { format!("- {item}") } else { format!("- [{priority}] {item}") }
}

pub fn appended(existing: Option<&str>, priority: &str, item: &str) -> Result<(String, Entry)> {
    let priority = match priority.trim().to_lowercase() {
        empty if empty.is_empty() => "none".to_string(),
        word if PRIORITIES.contains(&word.as_str()) => word,
        other => return Err(format!("\"{other}\" is not a priority; use urgent, high, medium or low, or leave it empty")),
    };
    let new = line(&priority, item.trim());
    let entry = parse(&new).ok_or("The item needs a title")?;
    let existing = existing.unwrap_or(HEADER);
    if entries(existing).iter().any(|other| other.title == entry.title) {
        return Err(format!("\"{}\" is already in the backlog; its title is how kelpie knows it, so give this one another", entry.title));
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
    Ok((format!("{existing}{separator}{new}\n"), entry))
}

pub fn add_to(path: &Path, priority: &str, item: &str) -> Result<String> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let (text, entry) = appended(existing.as_deref(), priority, item)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(format!("Added \"{}\" [{}] to {}.", entry.title, entry.priority, path.display()))
}

pub fn add(priority: Option<&str>, item: &str) -> Result<String> {
    add_to(&path(), priority.unwrap_or_default(), item)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_line_reads_its_priority_title_summary_and_area() {
        let item = parse("- [high] Fix login — the session drops on reload (app)").unwrap();
        assert_eq!((item.priority.as_str(), item.title.as_str(), item.summary.as_str(), item.area.as_str()), ("high", "Fix login", "the session drops on reload", "app"));
        assert_eq!((item.source.as_str(), item.reference.as_str()), ("local", "local:Fix login"));
        let bare = parse("- Write the docs").unwrap();
        assert_eq!((bare.priority.as_str(), bare.title.as_str(), bare.summary.as_str(), bare.area.as_str()), ("none", "Write the docs", "", ""));
        assert_eq!(parse("  * [URGENT] Rotate keys (infra)").map(|item| (item.priority, item.area)), Some(("urgent".to_string(), "infra".to_string())));
        assert_eq!(parse("- [later] Maybe — sometime").map(|item| (item.priority, item.title)), Some(("none".to_string(), "[later] Maybe".to_string())));
        assert_eq!(parse("- Fix it (the old way)").map(|item| (item.title, item.area)), Some(("Fix it (the old way)".to_string(), String::new())), "a parenthesis with a space is text, not an area");
        assert_eq!(parse("- [low] Tidy -- ascii dash (web)").map(|item| item.summary), Some("ascii dash".to_string()));
        for not_an_item in ["# Backlog", "", "Some prose.", "-", "- [high]", "<!-- note -->"] {
            assert!(parse(not_an_item).is_none(), "{not_an_item:?}");
        }
    }

    #[test]
    fn adding_appends_one_line_and_keeps_every_other_line_as_written() {
        let existing = "# My backlog\n\nSome notes  with  spacing.\n- [low] Old item (app)\n<!-- keep -->";
        let (text, entry) = appended(Some(existing), "High", "New item — why it matters (web)").unwrap();
        assert_eq!(text, format!("{existing}\n- [high] New item — why it matters (web)\n"));
        assert_eq!((entry.title.as_str(), entry.area.as_str()), ("New item", "web"));
        let (text, _) = appended(Some("- a\n"), "", "b").unwrap();
        assert_eq!(text, "- a\n- b\n");
        let (fresh, _) = appended(None, "medium", "First").unwrap();
        assert!(fresh.starts_with("<!-- kelpie's backlog") && fresh.ends_with("\n\n- [medium] First\n"));
        assert_eq!(entries(&fresh).len(), 1, "the header is not an item");
    }

    #[test]
    fn adding_refuses_a_bad_priority_an_empty_title_and_a_title_already_there() {
        assert!(appended(None, "soon", "X").unwrap_err().contains("\"soon\" is not a priority"));
        assert!(appended(None, "high", "  ").unwrap_err().contains("needs a title"));
        assert!(appended(Some("- [low] Fix login (app)\n"), "high", "Fix login — again").unwrap_err().contains("already in the backlog"));
    }

    #[test]
    fn add_writes_the_file_next_to_the_config_creating_it() {
        let dir = std::env::temp_dir().join(format!("kelpie-backlog-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("kelpie/backlog.md");
        assert!(add_to(&file, "high", "Ship it (app)").unwrap().starts_with("Added \"Ship it\" [high]"));
        add_to(&file, "", "Then this").unwrap();
        let titles: Vec<String> = read(&file).into_iter().map(|entry| entry.title).collect();
        assert_eq!(titles, ["Ship it", "Then this"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
