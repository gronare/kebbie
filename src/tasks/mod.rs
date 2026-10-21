mod approval;
mod claude;
mod finish;
mod history;
mod intent;
mod launch;
mod pool;
mod push;
mod started;
#[cfg(test)]
mod test_support;

pub(crate) use approval::{approve, approved, rules_check, same_code};
pub(crate) use claude::{end_claude, start_claude, trusted_by_user, wait_for_prompt};
pub(crate) use finish::{done, drop, reset};
pub(crate) use history::{Commit, commits, draft_squash_message, reword, squash};
pub(crate) use intent::{By as PushBy, complete as complete_pushes, pending as pending_push};
pub(crate) use launch::{Session, Start, claude_home, new_session_id, start, transcript, transcripts_dir};
pub(crate) use pool::{Slot, find_task, list, move_work, slots, task_of};
pub(crate) use push::{after_ci, land, push, tidy};
pub(crate) use started::{link, started_items, started_links};

use std::path::Path;

use crate::repo::{git, git_writing, name_of};
use crate::sh::Result;

// Task names are ASCII for git and herdr, so a letter with a diacritic becomes its base letter instead of a gap.
pub fn ascii_fold(text: &str) -> String {
    text.chars().map(|character| match character {
        'å' | 'ä' | 'à' | 'á' | 'â' | 'ã' => "a".to_string(),
        'Å' | 'Ä' | 'À' | 'Á' | 'Â' | 'Ã' => "A".to_string(),
        'ö' | 'ø' | 'ò' | 'ó' | 'ô' | 'õ' => "o".to_string(),
        'Ö' | 'Ø' | 'Ò' | 'Ó' | 'Ô' | 'Õ' => "O".to_string(),
        'é' | 'è' | 'ê' | 'ë' => "e".to_string(),
        'É' | 'È' | 'Ê' | 'Ë' => "E".to_string(),
        'ü' | 'ù' | 'ú' | 'û' => "u".to_string(),
        'Ü' | 'Ù' | 'Ú' | 'Û' => "U".to_string(),
        'í' | 'ì' | 'î' | 'ï' => "i".to_string(),
        'ç' => "c".to_string(),
        'ñ' => "n".to_string(),
        'æ' => "ae".to_string(),
        'Æ' => "AE".to_string(),
        'ß' => "ss".to_string(),
        other => other.to_string(),
    }).collect()
}

pub fn slug(name: &str) -> Result<String> {
    let mut slug = String::new();
    for character in ascii_fold(&name.to_lowercase()).chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.chars().take(40).collect::<String>().trim_end_matches('-').to_string();
    if !slug.starts_with(|character: char| character.is_ascii_lowercase()) {
        return Err(format!("\"{name}\" needs to start with a letter to work as a task name"));
    }
    Ok(slug)
}

fn fetch(repo: &Path) -> Result<()> {
    crate::fetch::shared(repo, || git(repo, &["fetch", "--quiet", "origin"]).map(|_| ()))
}

fn fast_forward_main(repo: &Path, upstream: &str) -> Result<String> {
    let local = upstream.trim_start_matches("origin/");
    let name = name_of(repo);
    if git(repo, &["branch", "--show-current"])? != local {
        return Ok(format!("{name} left as is (not on {local})."));
    }
    Ok(match git_writing(repo, &["merge", "--quiet", "--ff-only", upstream]) {
        Ok(_) => format!("{name} fast-forwarded to {upstream}."),
        Err(_) => format!("{name} could not fast-forward to {upstream}."),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn the_board_never_waits_on_the_vault_on_its_own_thread() {
        let board: String = std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/board")).unwrap().map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap()).collect();
        for call in ["tasks::done(", "tasks::reset(", "tasks::push(", "tasks::start("] {
            for (at, _) in board.match_indices(call) {
                let line = board[..at].rsplit('\n').next().unwrap_or_default();
                assert!(line.contains("move ||") || line.contains("sender.send("), "{call} runs on the board's thread: {line}");
            }
        }
        assert!(!board.contains("vault_cli::detect") && !board.contains("vault_cli::cli()"), "the board reads the kept verdict only");
    }

    #[test]
    fn kelpie_carries_no_commit_style_rules() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let banned = [["auto", "squash"].concat(), ["co-authored", "-by"].concat(), ["fix", "up"].concat(), ["merge", " commit"].concat(), ["co", "authored"].concat(), ["attri", "bution"].concat()];
        let mut files = vec![root.join("README.md")];
        for dir in ["src", "roles", "skills", "hooks", "scripts"] {
            let mut stack = vec![root.join(dir)];
            while let Some(path) = stack.pop() {
                match std::fs::read_dir(&path) {
                    Ok(entries) => stack.extend(entries.filter_map(|entry| entry.ok()).map(|entry| entry.path())),
                    Err(_) => files.push(path),
                }
            }
        }
        assert!(files.len() > 20, "{files:?}");
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap_or_default().to_lowercase();
            for word in &banned {
                assert!(!text.contains(word.as_str()), "{} names a commit rule: {word}", file.display());
            }
        }
    }

    #[test]
    fn a_slug_is_lower_case_ascii_letters_digits_and_dashes_that_start_with_a_letter() {
        assert_eq!(slug("Fix Login").unwrap(), "fix-login");
        assert_eq!(slug("  --fix--login--  ").unwrap(), "fix-login");
        assert_eq!(slug("Föräldraledighet över jul").unwrap(), "foraldraledighet-over-jul");
        assert_eq!(slug("Straße/Ærø/çañon").unwrap(), "strasse-aero-canon");
        assert_eq!(slug("../../etc/passwd").unwrap(), "etc-passwd", "a path is a name: it never leaves the pool");
        assert_eq!(slug("a; rm -rf / #").unwrap(), "a-rm-rf");
        assert_eq!(slug("x$(id)`id`").unwrap(), "x-id-id");
        assert_eq!(slug(&format!("a{}", "b".repeat(100))).unwrap().len(), 40, "forty characters at most");
        assert_eq!(slug(&format!("{}-c", "a".repeat(39))).unwrap(), "a".repeat(39), "a cut never ends in a dash");
        assert_eq!(slug("-x").unwrap(), "x", "leading punctuation is dropped, not kept as a dash");
        assert_eq!(slug("@app").unwrap(), "app", "so an address-like name is its task part");
        for bad in ["", "123", "1-x", "!", "日本語", "  "] {
            assert!(slug(bad).unwrap_err().contains("needs to start with a letter"), "{bad:?}");
        }
        assert_eq!(ascii_fold("ÅÄÖ åäö ÉÈ ü ñ ß Æ"), "AAO aao EE u n ss AE");
        assert_eq!(ascii_fold("日本語"), "日本語", "other scripts are left for the slug to drop");
    }
}
