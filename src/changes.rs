use std::path::Path;

pub const FILE: &str = "CHANGES.md";
pub const UNRELEASED: &str = "Unreleased";
const NEW: &str = "What's new";
const FIXED: &str = "Fixed";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub version: String,
    pub new: Vec<String>,
    pub fixed: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notes {
    pub new: Vec<String>,
    pub fixed: Vec<String>,
}

// `## <version>` opens a section, `### What's new` or `### Fixed` a group, `- ` an entry, and an indented line continues the entry above.
pub fn parse(text: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut group: Option<bool> = None;
    let mut last: Option<bool> = None;
    for line in text.lines() {
        if let Some(version) = line.strip_prefix("## ") {
            sections.push(Section { version: version.trim().trim_start_matches('v').to_string(), ..Section::default() });
            (group, last) = (None, None);
        } else if let Some(heading) = line.strip_prefix("### ") {
            group = match heading.trim() {
                NEW => Some(true),
                FIXED => Some(false),
                _ => None,
            };
            last = None;
        } else if let (Some(section), Some(new)) = (sections.last_mut(), group) {
            let entries = if new { &mut section.new } else { &mut section.fixed };
            if let Some(entry) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
                entries.push(entry.trim().to_string());
                last = Some(new);
            } else if line.starts_with([' ', '\t']) && !line.trim().is_empty() && last == Some(new) {
                if let Some(entry) = entries.last_mut() {
                    entry.push(' ');
                    entry.push_str(line.trim());
                }
            } else {
                last = None;
            }
        }
    }
    sections
}

fn released_after(version: &str, old: Option<&str>) -> bool {
    version != UNRELEASED && old.is_none_or(|old| crate::update_check::newer(version, old))
}

fn push_new(into: &mut Vec<String>, entries: &[String], known: &[String]) {
    for entry in entries {
        if !known.contains(entry) && !into.contains(entry) {
            into.push(entry.clone());
        }
    }
}

// Entries the old checkout already carried were there before the update, wherever they sit now; without an old file the versions decide.
pub fn between(old: Option<&str>, new: &str, old_version: Option<&str>, new_version: &str, main: bool) -> Notes {
    let include = |section: &Section| if section.version == UNRELEASED { main } else { old.is_some() || (released_after(&section.version, old_version) && !crate::update_check::newer(&section.version, new_version)) };
    let (known_new, known_fixed): (Vec<String>, Vec<String>) = match old {
        Some(old) => parse(old).into_iter().fold((Vec::new(), Vec::new()), |(mut new, mut fixed), section| {
            new.extend(section.new);
            fixed.extend(section.fixed);
            (new, fixed)
        }),
        None => (Vec::new(), Vec::new()),
    };
    let mut notes = Notes::default();
    for section in parse(new).iter().filter(|section| include(section)) {
        push_new(&mut notes.new, &section.new, &known_new);
        push_new(&mut notes.fixed, &section.fixed, &known_fixed);
    }
    notes
}

pub fn for_update(root: &Path, old_head: Option<&str>, old_version: Option<&str>, new_version: &str, main: bool) -> Notes {
    let Ok(new) = std::fs::read_to_string(root.join(FILE)) else { return Notes::default() };
    let old = old_head.and_then(|head| crate::update_check::git(root, &["show", &format!("{head}:{FILE}")]).ok());
    between(old.as_deref(), &new, old_version, new_version, main)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTES: &str = "# Changes\n\nOne line per change, as a user would say it.\n\n## Unreleased\n\n### What's new\n- The hub window shows your devices by name, and r removes\n  one\n- The update shows its steps\n\n### Fixed\n- Update no longer stops on a newer config key\n\n## 0.27.0\n\n### What's new\n- Connect a hub from the board\n\n## 0.26.10\n\n### Fixed\n- An answered question gives the board back\n";

    #[test]
    fn a_section_per_version_with_its_entries_grouped_and_wrapped_lines_joined() {
        let sections = parse(NOTES);
        assert_eq!(sections.iter().map(|section| section.version.as_str()).collect::<Vec<_>>(), ["Unreleased", "0.27.0", "0.26.10"]);
        assert_eq!(sections[0].new, ["The hub window shows your devices by name, and r removes one", "The update shows its steps"]);
        assert_eq!(sections[0].fixed, ["Update no longer stops on a newer config key"]);
        assert_eq!(sections[2].fixed, ["An answered question gives the board back"]);
    }

    #[test]
    fn the_main_channel_shows_the_entries_added_since_the_old_commit() {
        let old = "## Unreleased\n\n### What's new\n- The update shows its steps\n\n## 0.27.0\n\n### What's new\n- Connect a hub from the board\n\n## 0.26.10\n\n### Fixed\n- An answered question gives the board back\n";
        let notes = between(Some(old), NOTES, Some("0.27.0"), "0.27.0", true);
        assert_eq!(notes.new, ["The hub window shows your devices by name, and r removes one"], "an entry the old commit had is not new");
        assert_eq!(notes.fixed, ["Update no longer stops on a newer config key"]);
        let released = NOTES.replace("## Unreleased\n", "## Unreleased\n\n## 0.28.0\n");
        assert_eq!(between(Some(old), &released, Some("0.27.0"), "0.28.0", true).new, ["The hub window shows your devices by name, and r removes one"], "entries moved under a version heading by a release stay known");
        assert!(between(Some(NOTES), NOTES, Some("0.27.0"), "0.27.0", true)== Notes::default(), "nothing written since, nothing shown");
    }

    #[test]
    fn the_release_channel_shows_the_versions_between_the_old_and_the_new() {
        let notes = between(None, NOTES, Some("0.26.10"), "0.27.0", false);
        assert_eq!((notes.new, notes.fixed), (vec!["Connect a hub from the board".to_string()], Vec::<String>::new()), "never the unreleased entries, never the version already installed");
        let notes = between(None, NOTES, Some("0.26.9"), "0.27.0", false);
        assert_eq!(notes.fixed, ["An answered question gives the board back"]);
        assert_eq!(between(None, NOTES, Some("0.27.0"), "0.27.0", false), Notes::default());
        assert_eq!(between(None, "", Some("0.26.0"), "0.27.0", false), Notes::default(), "no file, no notes");
    }

    #[test]
    fn the_repos_own_notes_parse_and_say_what_is_unreleased() {
        let sections = parse(include_str!("../CHANGES.md"));
        assert_eq!(sections.first().map(|section| section.version.as_str()), Some(UNRELEASED), "Unreleased stays the first section, where release.sh takes its entries from");
        assert!(sections.iter().all(|section| section.new.iter().chain(&section.fixed).all(|entry| !entry.is_empty() && !entry.contains('`'))), "each entry is one plain line");
    }
    #[test]
    fn an_update_reads_the_old_notes_from_the_commit_it_came_from() {
        let root = crate::sh::test_dir("changes-git");
        let git = |args: &[&str]| crate::update_check::git(&root, &[&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"], args].concat()).unwrap();
        git(&["init", "--quiet"]);
        std::fs::write(root.join(FILE), "## Unreleased\n\n### What's new\n- Old line\n").unwrap();
        git(&["add", FILE]);
        git(&["commit", "--quiet", "-m", "one"]);
        let old = git(&["rev-parse", "HEAD"]);
        std::fs::write(root.join(FILE), "## Unreleased\n\n### What's new\n- Old line\n- New line\n").unwrap();
        assert_eq!(for_update(&root, Some(&old), Some("0.27.0"), "0.27.0", true).new, ["New line"]);
        assert_eq!(for_update(&root, Some("0000000"), Some("0.27.0"), "0.27.0", true).new, ["Old line", "New line"], "an old commit this clone lacks falls back to the versions");
        assert_eq!(for_update(&root.join("nowhere"), Some(&old), None, "0.27.0", true), Notes::default());
    }

}
