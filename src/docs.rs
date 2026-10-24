use std::path::{Path, PathBuf};

use crate::sh::Result;

const ONLINE: &str = "https://github.com/gronare/kebbie/tree/main/docs";
const SHOWN: usize = 12;
const NOISE: [&str; 12] = ["the", "and", "how", "what", "does", "with", "for", "kebbie", "can", "use", "set", "you"];

struct Page {
    path: PathBuf,
    title: String,
    first_line: String,
    headings: Vec<String>,
    rows: Vec<(String, String)>,
}

pub fn print(words: &[String]) -> Result<String> {
    Ok(render(crate::lead::install_root().ok().as_deref(), words))
}

pub fn render(root: Option<&Path>, words: &[String]) -> String {
    let Some(docs) = root.map(|root| root.join("docs")).filter(|docs| docs.join("README.md").is_file()) else {
        return missing(root);
    };
    let header = format!("kebbie's docs: {}\nkebbie help lists every command and option.", docs.display());
    if words.is_empty() {
        return format!("{header}\n\n{}", index(&docs));
    }
    format!("{header}\n\n{}", matches(&docs, words))
}

fn missing(root: Option<&Path>) -> String {
    let online = root
        .and_then(|root| crate::update_check::git(root, &["remote", "get-url", "origin"]).ok())
        .and_then(|remote| crate::release::github_repo(&remote))
        .map_or_else(|| ONLINE.to_string(), |repo| format!("https://github.com/{repo}/tree/main/docs"));
    let place = root.map(|root| format!(" (no docs/README.md in {})", root.display())).unwrap_or_default();
    format!("kebbie's docs are not on this machine{place}: this kebbie runs without its checkout.\nkebbie help lists every command and option; the docs are online at {online}")
}

fn index(docs: &Path) -> String {
    let readme = std::fs::read_to_string(docs.join("README.md")).unwrap_or_default();
    let mut lines = Vec::new();
    for line in readme.lines() {
        if let Some(section) = line.strip_prefix("## ") {
            lines.push(format!("\n{section}"));
        } else if let Some(entry) = line.strip_prefix("- [") {
            let (title, rest) = entry.split_once("](").unwrap_or((entry, ""));
            let (target, description) = rest.split_once(')').unwrap_or((rest, ""));
            lines.push(format!("  {target}  {title}{description}"));
        } else if !line.trim().is_empty() && !line.starts_with('#') && !lines.is_empty() {
            lines.push(format!(" {}", line.trim()));
        }
    }
    lines.join("\n").trim_start().to_string()
}

// A single character is a key such as M, so it keeps its case and matches whole words only.
fn terms(words: &[String]) -> Vec<String> {
    let all: Vec<String> = words.iter().flat_map(|word| word.split(|character: char| !character.is_alphanumeric())).filter(|word| !word.is_empty())
        .map(|word| if word.chars().count() == 1 { word.to_string() } else { word.to_lowercase() }).collect();
    let kept: Vec<String> = all.iter().filter(|word| match word.chars().count() {
        1 => !["a", "I"].contains(&word.as_str()),
        2 => false,
        _ => !NOISE.contains(&word.as_str()),
    }).cloned().collect();
    let mut seen = std::collections::HashSet::new();
    (if kept.is_empty() { all } else { kept }).into_iter().filter(|term| seen.insert(term.clone())).collect()
}

fn is_key(term: &str) -> bool {
    term.chars().count() == 1
}

fn has_word(text: &str, term: &str) -> bool {
    text.split(|character: char| character.is_whitespace() || [',', '`', '/', '(', ')'].contains(&character)).any(|word| word == term)
}

fn heading_matches(heading: &str, term: &str) -> bool {
    if is_key(term) { has_word(heading, term) } else { heading.to_lowercase().contains(term) }
}

fn section_rank(docs: &Path, path: &Path) -> usize {
    let section = path.strip_prefix(docs).ok().and_then(|relative| relative.components().next()).map(|first| first.as_os_str().to_string_lossy().to_string()).unwrap_or_default();
    ["reference", "how-to", "explanation", "tutorial.md"].iter().position(|name| *name == section).unwrap_or(4)
}

fn matches(docs: &Path, words: &[String]) -> String {
    let terms = terms(words);
    let mut scored: Vec<(usize, Page, Vec<String>)> = pages(docs).into_iter().filter_map(|page| {
        let name = page.path.file_stem().map(|stem| stem.to_string_lossy().replace('-', " ")).unwrap_or_default();
        let head = format!("{} {name}", page.title).to_lowercase();
        let headings: Vec<&String> = page.headings.iter().filter(|heading| terms.iter().any(|term| heading_matches(heading, term))).collect();
        let rows: Vec<&(String, String)> = page.rows.iter().filter(|(cell, _)| terms.iter().any(|term| is_key(term) && has_word(cell, term))).collect();
        let score: usize = terms.iter().map(|term| {
            if !is_key(term) && head.contains(term.as_str()) { 2 } else { usize::from(headings.iter().any(|heading| heading_matches(heading, term)) || rows.iter().any(|(cell, _)| is_key(term) && has_word(cell, term))) }
        }).sum();
        let hits: Vec<String> = headings.into_iter().map(|heading| format!("§ {heading}")).chain(rows.into_iter().map(|(_, row)| row.clone())).collect();
        (score > 0).then_some((score, page, hits))
    }).collect();
    if scored.is_empty() {
        return format!("Nothing in kebbie's docs has \"{}\" in a title or heading; try the English words for it, as the docs are in English. The index is {}.", words.join(" "), docs.join("README.md").display());
    }
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| section_rank(docs, &left.1.path).cmp(&section_rank(docs, &right.1.path))).then_with(|| left.1.path.cmp(&right.1.path)));
    let more = scored.len().saturating_sub(SHOWN);
    let mut lines: Vec<String> = scored.into_iter().take(SHOWN).map(|(_, page, hits)| {
        let mut entry = format!("{}\n  {}: {}", page.path.display(), page.title, page.first_line);
        for heading in hits.iter().take(3) {
            entry.push_str(&format!("\n  {heading}"));
        }
        entry
    }).collect();
    if more > 0 {
        lines.push(format!("and {more} more; add words to narrow it."));
    }
    lines.join("\n")
}

fn pages(docs: &Path) -> Vec<Page> {
    let mut files = Vec::new();
    collect(docs, &mut files);
    files.sort();
    files.into_iter().filter_map(|path| {
        let text = std::fs::read_to_string(&path).ok()?;
        let title = text.lines().find_map(|line| line.strip_prefix("# ")).unwrap_or_default().trim().to_string();
        let first_line = text.lines().skip_while(|line| !line.starts_with("# ")).skip(1).map(str::trim).find(|line| !line.is_empty() && !line.starts_with(['#', '<']) && !line.starts_with("![")).map(first_sentence).unwrap_or_default();
        let headings = text.lines().filter(|line| line.starts_with("##")).map(|line| line.trim_start_matches('#').trim().to_string()).collect();
        let rows = text.lines().filter_map(|line| {
            let cell = line.strip_prefix('|')?.split('|').next()?.trim();
            (!cell.is_empty() && !cell.starts_with("---")).then(|| (cell.to_string(), line.trim().to_string()))
        }).collect();
        Some(Page { path, title, first_line, headings, rows })
    }).collect()
}

fn first_sentence(line: &str) -> String {
    line.find(". ").map_or(line, |end| &line[..=end]).to_string()
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("kebbie-docs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("docs/how-to")).unwrap();
        std::fs::create_dir_all(root.join("docs/reference")).unwrap();
        std::fs::write(root.join("docs/README.md"), "# kebbie documentation\n\nStart here.\n\n## How-to guides\n\nTasks\n\n- [Run a task with Codex](how-to/run-a-task-with-codex.md): a profile with `agent = \"codex\"`.\n\n## Reference\n\n- [Commands](reference/commands.md): every command.\n").unwrap();
        std::fs::write(root.join("docs/how-to/run-a-task-with-codex.md"), "# Run a task with Codex\n\nA task can run OpenAI's Codex CLI instead of Claude Code.\n\n## Before you start\n").unwrap();
        std::fs::write(root.join("docs/reference/commands.md"), "# Commands\n\n`kebbie --help` prints the same list.\n\n## Setup and maintenance\n\n### `kebbie setup codex`\n").unwrap();
        std::fs::create_dir_all(root.join("docs/explanation")).unwrap();
        std::fs::write(root.join("docs/reference/board.md"), "# Board\n\n![The board](assets/board.png)\n<img src=\"x.png\">\n\nThe board lists every task.\n\n## Keys\n\n| Key | What it does |\n| --- | --- |\n| `M` | Mark it read. |\n| `m` | Move it. |\n").unwrap();
        std::fs::write(root.join("docs/explanation/board.md"), "# Board\n\nWhy the board looks the way it does.\n").unwrap();
        root
    }

    #[test]
    fn a_board_key_matches_its_table_row_and_ties_go_to_reference_first() {
        let root = install("keys");
        let text = render(Some(&root), &words("what does M do on the board"));
        let reference = root.join("docs/reference/board.md").display().to_string();
        assert!(text.contains(&format!("{reference}\n  Board: The board lists every task.\n  | `M` | Mark it read. |")), "an image is no first sentence, and the key's row is named: {text}");
        assert!(!text.contains("Move it"), "a key keeps its case: {text}");
        assert!(text.find(&reference) < text.find(&root.join("docs/explanation/board.md").display().to_string()), "reference before explanation on a tie: {text}");
        assert!(!render(Some(&root), &words("M")).contains("commands.md"), "a single character matches whole words only");
        assert_eq!(terms(&words("Codex codex CODEX setup")), ["codex", "setup"]);
    }

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn without_words_it_names_the_docs_folder_and_lists_the_index_by_section() {
        let root = install("index");
        let text = render(Some(&root), &[]);
        assert!(text.starts_with(&format!("kebbie's docs: {}\n", root.join("docs").display())), "{text}");
        assert!(text.contains("How-to guides\n Tasks\n  how-to/run-a-task-with-codex.md  Run a task with Codex: a profile with `agent = \"codex\"`."), "{text}");
        assert!(text.contains("Reference\n  reference/commands.md  Commands: every command."), "{text}");
        assert!(text.contains("kebbie help lists every command"), "{text}");
    }

    #[test]
    fn words_find_pages_by_title_and_heading_with_the_title_match_first() {
        let root = install("match");
        let text = render(Some(&root), &words("How do I set up Codex?"));
        let codex = root.join("docs/how-to/run-a-task-with-codex.md").display().to_string();
        let commands = root.join("docs/reference/commands.md").display().to_string();
        assert!(text.contains(&format!("{codex}\n  Run a task with Codex: A task can run OpenAI's Codex CLI instead of Claude Code.")), "{text}");
        assert!(text.contains(&format!("{commands}\n  Commands: `kebbie --help` prints the same list.\n  § `kebbie setup codex`")), "a heading match names its heading: {text}");
        assert!(text.find(&codex) < text.find(&commands), "a title match ranks above a heading match: {text}");
        assert!(!text.contains("README.md\n"), "the index itself is no answer: {text}");
        let none = render(Some(&root), &words("routines"));
        assert!(none.contains("Nothing in kebbie's docs has \"routines\"") && none.contains("try the English words") && none.contains("README.md"), "{none}");
    }

    #[test]
    fn a_kebbie_without_its_docs_points_at_help_and_the_online_docs() {
        let root = std::env::temp_dir().join(format!("kebbie-docs-bare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let text = render(Some(&root), &words("codex"));
        assert!(text.contains("not on this machine") && text.contains("kebbie help") && text.ends_with(ONLINE), "{text}");
        crate::update_check::git(&root, &["init", "-q"]).unwrap();
        crate::update_check::git(&root, &["remote", "add", "origin", "git@github.com:someone/kebbie.git"]).unwrap();
        assert!(render(Some(&root), &[]).ends_with("https://github.com/someone/kebbie/tree/main/docs"));
        assert!(render(None, &[]).ends_with(ONLINE));
    }

    #[test]
    fn the_skill_and_both_roles_send_questions_about_kebbie_to_its_docs() {
        let skill = include_str!("../skills/using-kebbie/SKILL.md");
        let description = skill.lines().find_map(|line| line.strip_prefix("description: ")).unwrap();
        for trigger in ["set up", "command", "config key", "board key", "tasks", "the lead", "push line", "stage", "sandbox", "profiles", "Codex", "routines", "updates"] {
            assert!(description.contains(trigger), "the skill's description triggers on {trigger}");
        }
        assert!(skill.contains("kebbie docs <words>") && skill.contains("kebbie help") && skill.contains("Done when the answer names the doc page"), "{skill}");
        assert!(include_str!("../roles/lead.md").contains("run the skill `kebbie:using-kebbie`"));
        assert!(include_str!("../roles/task.md").contains("`{kebbie} docs <words>` names the pages to read"));
        assert!(crate::known::skills(Path::new(env!("CARGO_MANIFEST_DIR"))).contains_key("kebbie:using-kebbie"));
    }

    #[test]
    fn kebbies_own_docs_answer_how_to_set_up_codex() {
        let text = render(Some(Path::new(env!("CARGO_MANIFEST_DIR"))), &words("set up codex"));
        assert!(text.lines().nth(3).is_some_and(|line| line.ends_with("docs/how-to/run-a-task-with-codex.md")), "{text}");
    }
}
