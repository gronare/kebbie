mod accept;
mod after;
mod approval;
mod finish;
mod follow;
mod history;
mod intent;
mod landed;
mod launch;
mod line;
mod plan;
mod pool;
mod push;
mod red_ci;
mod stage;
mod started;
#[cfg(test)]
pub(crate) mod test_support;

pub(crate) use accept::{accept, accepted_in, last_of_run, listing_at as accepted_listing_at, open_commits, rest_base, shown as accepted_shown};
pub(crate) use after::{EVERY as AFTER_EVERY, Asked as QueueAsked, and_list, after as after_command, at as start_at, drop as drop_wait, now as start_now, queue, refuse_queued_name, Shown as WaitShown, shown as waits_shown, tick as after_tick};
#[cfg(test)]
pub(crate) use approval::approval_holds;
pub(crate) use approval::{approve, approved, awaits_check, fingerprint_at, record_approval, rules_check, rules_check_subject, rules_check_task, same_code};
pub(crate) use crate::agent::{Started, wait_for_prompt};
pub(crate) use finish::{FORCE, done, drop, drop_routine_ok, reset};
pub(crate) use follow::{follow, follows, tick as follow_tick, toggle as toggle_follow};
pub(crate) use history::{Commit, commits, draft_squash_message, reword, squash};
pub(crate) use intent::{By as PushBy, Pending as PendingPush, Work as PushWork, after_red_ci, left as push_left, pending as pending_push};
pub(crate) use line::complete as complete_pushes;
pub(crate) use landed::{EVERY as LANDED_EVERY, Unsure as LandedUnsure, mark as landed_mark, tick as landed_tick, tick_cli as landed_tick_cli};
pub(crate) use red_ci::{dismiss as dismiss_ci, dismissed as ci_dismissed, outside as ci_outside, outside_reason as ci_outside_reason};
pub(crate) use launch::{Begun, Session, Start, begin, bypasses, latest_transcript, new_session_id, profile_of, record_session, start, task_at, task_role, task_role_now, task_tmp, transcript};
pub(crate) use plan::{noted_after as steps_noted_after, seen_in as steps_seen_in, steps};
pub(crate) use plan::{Step, Steps, State as StepState, Tips as StepTips, read_at as steps_read, shown_at as steps_shown_at, for_path as steps_for, mark as step_mark};
pub(crate) use pool::{Slot, find_task, list, move_work, slots, task_of};
pub(crate) use push::{after_ci, cancel as cancel_push, land, place_in_line, push, rebase, tidy};
pub(crate) use stage::{WAIT_CEILING as STAGE_WAIT, stage, staged, tick as stage_tick, unstage};
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

// Outside a push turn, main is rewritten only under the repo's turnstile; while another holds it, main is left for the next push to follow.
fn fast_forward_main(repo: &Path, upstream: &str) -> Result<String> {
    let Ok(state) = crate::lead::state_dir() else { return follow_upstream(repo, upstream, &[]) };
    let staged = stage::load(&state, repo);
    if staged.is_empty() || line::Turnstile::held_here(&state, repo) {
        return follow_upstream(repo, upstream, &staged);
    }
    match line::Turnstile::try_take(&state, repo) {
        Some(_turnstile) => follow_upstream(repo, upstream, &staged),
        None => Ok(format!("{} left as is: kebbie is pushing in it now", name_of(repo))),
    }
}

// The checkout's housekeeping after a push or a close never fails what went before it; what went wrong goes into the line it returns.
fn follow_upstream(repo: &Path, upstream: &str, staged: &[stage::Entry]) -> Result<String> {
    Ok(following(repo, upstream, staged).unwrap_or_else(|error| format!("{} left as is: {error}", name_of(repo))))
}

fn following(repo: &Path, upstream: &str, staged: &[stage::Entry]) -> Result<String> {
    let local = upstream.trim_start_matches("origin/");
    let name = name_of(repo);
    if git(repo, &["branch", "--show-current"])? != local {
        return Ok(format!("{name} left as is (not on {local})"));
    }
    if git_writing(repo, &["merge", "--quiet", "--ff-only", upstream]).is_ok() {
        return Ok(format!("{name} fast-forwarded to {local}"));
    }
    if staged.is_empty() {
        return Ok(format!("{name} could not fast-forward to {local}"));
    }
    stage::follow_staged(repo, upstream, staged)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::repo::git;

    #[test]
    fn the_board_never_waits_on_the_vault_on_its_own_thread() {
        let board: String = std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/board")).unwrap().map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap()).collect();
        for call in ["tasks::done(", "tasks::reset(", "tasks::push(", "tasks::start(", "tasks::rebase("] {
            for (at, _) in board.match_indices(call) {
                let line = board[..at].rsplit('\n').next().unwrap_or_default();
                assert!(line.contains("move ||") || line.contains("sender.send("), "{call} runs on the board's thread: {line}");
            }
        }
        assert!(!board.contains("vault_cli::detect") && !board.contains("vault_cli::cli()"), "the board reads the kept verdict only");
    }

    #[test]
    fn kebbie_carries_no_commit_style_rules() {
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
    fn the_hooks_a_report_and_a_question_move_the_tasks_phase() {
        use crate::phase::{Event, Phase, Why};
        let (dir, origin, slot, _) = test_support::task_with_a_commit("billing");
        let state = crate::lead::state_dir().unwrap();
        let address = crate::peers::address("billing", &name_of(&dir));
        let phase = || crate::phase::load(&state, &address).map(|record| record.phase);
        let prompt = |text: &str| crate::report::hook(&serde_json::json!({ "hook_event_name": "UserPromptSubmit", "cwd": slot.path, "prompt": text }).to_string());
        let stop = |active: bool| crate::report::hook(&serde_json::json!({ "hook_event_name": "Stop", "cwd": slot.path, "stop_hook_active": active }).to_string());
        crate::phase::record(&state, &address, "start", &Event::Started { prompted: true });
        prompt("Add a refund column");
        assert_eq!(phase(), Some(Phase::Working), "the prompt kebbie typed for the user is taken up");
        assert!(stop(false).contains("\"block\""), "a stop without a report is sent back once");
        assert_eq!(phase(), Some(Phase::Working));
        stop(true);
        assert_eq!(phase(), Some(Phase::NeedsYou(Why::Unreported)), "and the stop after it, still without one, waits for the user");
        prompt(&crate::messages::delivered("lead", "fix the second commit"));
        prompt(&crate::messages::delivered("kebbie", "kebbie was updated"));
        assert_eq!(phase(), Some(Phase::NeedsYou(Why::Unreported)), "the inbox's raw content from the lead or kebbie moves nothing");
        prompt("go on");
        assert_eq!(phase(), Some(Phase::Working), "the user's own prompt does");
        let refused = crate::report::report(&slot.path, Some("ready"), Some("push and close"), None, "refunds done").unwrap_err();
        assert!(refused.contains("Not reported ready") && refused.contains("fourth: no body"), "a commit without a body is refused before kebbie hears the report: {refused}");
        assert_eq!(phase(), Some(Phase::Working), "the refused report moves nothing");
        git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--amend", "-m", "Add refunds\n\nThe billing page lists refunds now."]).unwrap();
        crate::report::report(&slot.path, Some("ready"), Some("push and close"), None, "refunds done").unwrap();
        assert_eq!(crate::rules_check::shown_in(&state, &address, |_| true), Some(crate::rules_check::Shown::Checking), "the ready report asks for the rules check");
        assert!(crate::trusted_spawn::SPAWNED.with(|spawned| spawned.borrow().is_empty()), "and starts nothing from the task's own process");
        assert_eq!(phase(), Some(Phase::Ready { head: Some(git(&slot.path, &["rev-parse", "HEAD"]).unwrap()) }));
        prompt(&crate::messages::delivered("kebbie", "kebbie was updated"));
        assert!(matches!(phase(), Some(Phase::Ready { .. })), "an update notice leaves it ready");
        crate::report::report(&slot.path, Some("working"), Some("nothing"), None, "one more test").unwrap();
        assert_eq!(phase(), Some(Phase::Working));
        crate::questions::ask(&slot.path, &["A: one", "B: two"], Some("1"), None, "Which queue?").unwrap();
        assert_eq!(phase(), Some(Phase::NeedsYou(Why::Question)));
        test_support::cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_start_that_never_reaches_its_claude_waits_for_work_and_not_under_working() {
        let (dir, origin, slot, _) = test_support::task_with_a_commit("billing");
        let state = crate::lead::state_dir().unwrap();
        let address = crate::peers::address("search", &name_of(&dir));
        let options = Start { prompt: Some("Search the invoices"), focus: false, from: None, mode: None, profile: None, backlog: None, note: false, anyway: false };
        assert!(begin(&dir, "search", &options).is_err(), "herdr is not there to open its Space");
        assert_eq!(crate::phase::load(&state, &address).map(|record| record.phase), Some(crate::phase::Phase::Idle));
        if let Some(search) = slots(&dir).unwrap().into_iter().find(|slot| slot.task.as_deref() == Some("search")) {
            git(&dir, &["worktree", "remove", "--force", &search.path.to_string_lossy()]).unwrap();
        }
        test_support::cleanup(dir, origin, &slot);
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
