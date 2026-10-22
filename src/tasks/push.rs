use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::board::widgets::plural;
use crate::config;
use crate::herdr;
use crate::lead::state_dir;
use crate::messages;
use crate::peers;
use crate::permit::Caller;
use crate::repo::{ahead_behind, default_branch, git, git_writing, hidden_from_status, main_checkout, name_of, rebase_in_progress, uncommitted};
use crate::sh::Result;
use super::approval::{approved, fingerprint, record_approval, same_code};
use super::intent::{self, By, Intent, Through, Work};
use super::finish::{close_task, note_ready};
use super::{fast_forward_main, fetch};
use super::pool::{Slot, find_task, task_of};
use super::started::started_file;

pub fn land(cwd: &Path, name: Option<&str>) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    committed(&slot.path, task)?;
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    if ahead_behind(&slot.path, &upstream)?.0 == 0 {
        return Err(format!("{task} has no commits to land"));
    }
    if !rebase_or_hand_over(&slot, &upstream, "kelpie tried to land this task")? {
        return Err(format!("{task} conflicts with {upstream}; its Claude is resolving the rebase. Push again when it reports back."));
    }
    let commits = git(&slot.path, &["log", "--oneline", &format!("{upstream}..HEAD")])?;
    Ok(format!("{task} is rebased onto {upstream} and ready:\n{commits}\n\nPush with:  kelpie push {task}"))
}

pub fn rebase(cwd: &Path) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, None, cwd)?;
    let task = task_of(&slot);
    committed(&slot.path, task)?;
    fetch(&repo)?;
    let upstream = default_branch(&repo)?;
    if ahead_behind(&slot.path, &upstream)?.1 == 0 {
        return Ok(format!("{task} is already on top of {upstream}."));
    }
    if !rebase_or_hand_over(&slot, &upstream, "The user asked kelpie to rebase this task")? {
        return Err(format!("{task} conflicts with {upstream}; its Claude is resolving the rebase and reports back."));
    }
    Ok(format!("{task} is rebased onto {upstream}; nothing was pushed."))
}

// Uncommitted changes to tracked files may be unfinished work, so they keep a task from landing or pushing; untracked files do not.
pub(super) fn committed(path: &Path, task: &str) -> Result<()> {
    let dirty = uncommitted(path)?;
    if dirty.is_empty() {
        return Ok(());
    }
    Err(format!("{task} has {} in {} ({}); commit or stash them first", plural(dirty.len(), "uncommitted file"), path.display(), dirty.join(", ")))
}

// Files the task changes that the upstream commits it was rebased over changed too, each with those commits as "sha subject".
#[derive(Debug, PartialEq)]
pub(super) struct Touched {
    pub file: String,
    pub commits: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub(super) enum Updated {
    Clean,
    Overlap(Vec<Touched>),
    Conflict,
}

// Rebases onto upstream, already fetched; a conflict is aborted, and a rebase git refuses to start is an error with git's own reason.
pub(super) fn update(path: &Path, upstream: &str) -> Result<Updated> {
    let touched = overlap(path, upstream)?;
    match git_writing(path, &["rebase", "--quiet", upstream]) {
        Ok(_) if touched.is_empty() => Ok(Updated::Clean),
        Ok(_) => Ok(Updated::Overlap(touched)),
        Err(_) if rebase_in_progress(path) => {
            git_writing(path, &["rebase", "--abort"])?;
            Ok(Updated::Conflict)
        }
        Err(error) => Err(refused(path, upstream, &error)),
    }
}

fn refused(path: &Path, upstream: &str, error: &str) -> String {
    let said = format!("git would not start rebasing onto {upstream} in {}: {error}", path.display());
    match hidden_from_status(path) {
        Ok(hidden) if !hidden.is_empty() => format!(
            "{said}\nThese files are hidden from git status (skip-worktree or assume-unchanged) and block the rebase: {}. Unhide them with git update-index --no-skip-worktree or --no-assume-unchanged, commit or stash them, then try again.",
            hidden.join(", ")
        ),
        _ => said,
    }
}

fn overlap(path: &Path, upstream: &str) -> Result<Vec<Touched>> {
    let base = git(path, &["merge-base", "HEAD", upstream])?;
    let mine: BTreeSet<String> = git(path, &["log", "--format=", "--name-only", "--no-renames", &format!("{base}..HEAD")])?.lines().filter(|line| !line.is_empty()).map(str::to_string).collect();
    let theirs = git(path, &["log", "--reverse", "--format=%x00%h %s", "--name-only", "--no-renames", &format!("{base}..{upstream}")])?;
    let mut touched: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in theirs.split('\0').filter(|entry| !entry.trim().is_empty()) {
        let mut lines = entry.trim().lines();
        let commit = lines.next().unwrap_or_default().to_string();
        for file in lines.filter(|file| mine.contains(*file)) {
            touched.entry(file.to_string()).or_default().push(commit.clone());
        }
    }
    Ok(touched.into_iter().map(|(file, commits)| Touched { file, commits }).collect())
}

// Ok(false) when the rebase conflicted and the task's Claude took it over.
fn rebase_or_hand_over(slot: &Slot, upstream: &str, why: &str) -> Result<bool> {
    if update(&slot.path, upstream)? != Updated::Conflict {
        return Ok(true);
    }
    let task = task_of(slot);
    hand_over(slot, &conflict_prompt(why, upstream)).map_err(|cannot| format!("{task} conflicts with {upstream} and kelpie could not hand it to its Claude ({cannot}); rebase it in {}", slot.path.display()))?;
    Ok(false)
}

pub(super) fn conflict_prompt(why: &str, upstream: &str) -> String {
    format!(
        "{why} and rebasing onto {upstream} conflicts (already fetched). Rebase this branch onto {upstream} with git rebase {upstream} (no -i), \
         resolve each conflict keeping the intent of both sides (read the upstream commits that touched the file), run the tests for the files \
         involved, and continue until the rebase is finished. When a conflict is a choice of intent rather than mechanics, both sides wanting \
         different behaviour from the same code, do not pick one: ask the user with kelpie ask, with the options and your recommendation, and \
         resolve it by their answer. Do not push. Then report ready, saying in a few lines what conflicted and how you resolved it. When a \
         push started this, kelpie pushes the resolved commits as soon as the lead has checked them; report needs-you only when a test stays red."
    )
}

fn overlap_prompt(by: By, upstream: &str, touched: &[Touched]) -> String {
    let files: Vec<String> = touched.iter().map(|touched| format!("- {}: {}", touched.file, touched.commits.join("; "))).collect();
    format!(
        "{} asked kelpie to push this task. kelpie fetched and rebased it onto {upstream} without conflicts, but the commits it was rebased over \
         changed files this task changes too:\n{}\nRun the tests that cover those files and fix what broke with a new commit. Do not push. When \
         the tests pass, report ready: kelpie pushes the task for {} then, once the lead has checked a fix against the rules. Report needs-you \
         only when a test stays red.",
        capitalized(by.word()), files.join("\n"), by.word()
    )
}

// Held like any message, so a draft in the box or an open question delays the work and never drops it.
pub(super) fn hand_over(slot: &Slot, text: &str) -> std::result::Result<(), String> {
    let repo = main_checkout(&slot.path)?;
    messages::hand_to(&peers::address(task_of(slot), &name_of(&repo)), text)
}

fn claude_of(path: &Path) -> Option<String> {
    let workspace = herdr::workspace_for(path).ok()??;
    herdr::agents().ok()?.into_iter()
        .find(|agent| agent["agent"] == "claude" && agent["workspace_id"].as_str() == Some(workspace.as_str()))
        .and_then(|agent| agent["pane_id"].as_str().map(str::to_string))
}

// Agents have no interactive git, so the prompt names the non-interactive ways to reshape history.
pub fn tidy(cwd: &Path, name: Option<&str>, note: &str) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let task = task_of(&slot);
    let pane = claude_of(&slot.path).ok_or_else(|| format!("{task} has no Claude running to tidy its commits"))?;
    let upstream = default_branch(&repo)?;
    let rules = config::resolve(Some(&repo))?.settings.rules_skill.value;
    let mut prompt = format!(
        "Reshape this branch's commits on top of {upstream} for review, following the skill `{rules}`. Interactive git is not \
         available: use git rebase with GIT_SEQUENCE_EDITOR editing the todo, git commit --amend, or git reset --soft and recommit. \
         The final tree must be identical: git diff against the old head must be empty. Do not push. Show the commit list before and after."
    );
    if !note.trim().is_empty() {
        prompt.push_str(&format!(" From the user: {}", note.trim()));
    }
    crate::typing::prompt(&pane, &prompt, task).map_err(|refused| format!("{task}'s Claude {}, so kelpie typed nothing; tidy again once its box is clear", refused.reason()))?;
    Ok(format!("Asked {task}'s Claude to tidy its commits."))
}

// Pushes the task's commits through `through` (all of them without it) to the default branch, then closes the task, or with keep_open leaves the rest of its branch on top of what was pushed.
pub fn push(cwd: &Path, name: Option<&str>, through: Option<&str>, keep_open: bool) -> Result<String> {
    let repo = main_checkout(cwd)?;
    let slot = find_task(&repo, name, cwd)?;
    let upstream = default_branch(&repo)?;
    let through = chosen(&slot, &upstream, through, keep_open)?;
    push_task(&crate::permit::caller(), &repo, &slot, through)
}

// The commit the user picked, as a count of the task's commits, so it survives the rebase push starts with.
fn chosen(slot: &Slot, upstream: &str, through: Option<&str>, keep_open: bool) -> Result<Through> {
    let ahead = ahead_behind(&slot.path, upstream)?.0;
    if ahead == 0 {
        return Err(format!("{} has no commits to push", task_of(slot)));
    }
    let Some(rev) = through else { return Ok(Through::All) };
    let head = git(&slot.path, &["rev-parse", "HEAD"])?;
    let sha = pushable_through(&slot.path, upstream, rev, keep_open)?;
    if !stays_open(keep_open, &sha, &head) {
        return Ok(Through::All);
    }
    let count = git(&slot.path, &["rev-list", "--count", &format!("{upstream}..{sha}")])?.parse::<usize>().map_err(|error| error.to_string())?;
    Ok(Through::First { count, of: ahead })
}

fn through_sha(path: &Path, upstream: &str, through: Through) -> Result<String> {
    match through {
        Through::All => git(path, &["rev-parse", "HEAD"]),
        Through::First { count, of } => {
            let commits: Vec<String> = git(path, &["rev-list", "--reverse", &format!("{upstream}..HEAD")])?.lines().map(str::to_string).collect();
            if commits.len() != of || count == 0 || count > of {
                return Err(format!("the task's commits changed since the push was chosen ({} now, {of} then); open push again", commits.len()));
            }
            Ok(commits[count - 1].clone())
        }
    }
}

fn authorize(caller: &Caller, repo: &Path, slot: &Slot, upstream: &str, through: Through) -> Result<&'static str> {
    let task = task_of(slot);
    let address = peers::address(task, &name_of(repo));
    let head = git(&slot.path, &["rev-parse", "HEAD"])?;
    let sha = through_sha(&slot.path, upstream, through).map_err(|why| format!("{task} cannot be pushed: {why}"))?;
    let shown = crate::review::shown(&address, &slot.path, upstream);
    let covered = |reviewed: &str| same_code(&slot.path, upstream, reviewed, &head) || same_code(&slot.path, upstream, reviewed, &sha);
    crate::review::gate(shown.as_ref(), covered).map_err(|why| format!("{task} cannot be pushed yet: {why}."))?;
    let check = Pusher { caller: caller.clone(), permit: crate::permit::of(&address, &name_of(repo)), reviewed: shown.as_ref().is_some_and(|shown| !shown.pending()), approved: approved(&slot.path, upstream, &address) };
    check.allowed().map_err(|why| format!("{task} cannot be pushed by {}: {why}.", crate::permit::describe(&check.caller)))
}

pub(super) fn push_task(caller: &Caller, repo: &Path, slot: &Slot, through: Through) -> Result<String> {
    match prepare(caller, repo, slot, through, &hand_over)? {
        Prepared::Push(pusher) => deliver(repo, slot, &default_branch(repo)?, through, pusher, &state_dir()?),
        Prepared::Handed(said) => Ok(said),
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Prepared {
    Push(&'static str),
    Handed(String),
}

pub(super) type HandOver<'a> = &'a dyn Fn(&Slot, &str) -> std::result::Result<(), String>;

fn prepare(caller: &Caller, repo: &Path, slot: &Slot, through: Through, hand_over: HandOver) -> Result<Prepared> {
    let task = task_of(slot).to_string();
    let upstream = default_branch(repo)?;
    let address = peers::address(&task, &name_of(repo));
    let state = state_dir()?;
    if ahead_behind(&slot.path, &upstream)?.0 == 0 {
        return Err(format!("{task} has no commits to push"));
    }
    committed(&slot.path, &task)?;
    let pusher = authorize(caller, repo, slot, &upstream, through)?;
    if intent::load(&state, &address).is_some() && *caller != Caller::User {
        return Err(format!("{task} is being pushed already: its Claude works on the rebase onto {upstream}, and kelpie pushes it once that reports ready"));
    }
    let by = By::of(caller).ok_or_else(|| format!("{task} cannot be pushed by {}", crate::permit::describe(caller)))?;
    fetch(repo)?;
    let recorded = fingerprint(&slot.path, &upstream)?;
    let seen = messages::latest_reports(&messages::load()).get(&address).map(|report| report.id.clone());
    let record = |work: Work, fingerprint: String| intent::save(&state, &Intent { by, work, address: address.clone(), repo: repo.to_path_buf(), task: task.clone(), fingerprint, through, at: messages::now(), seen: seen.clone() });
    match update(&slot.path, &upstream)? {
        Updated::Clean => {}
        Updated::Conflict => {
            record(Work::Conflict, recorded)?;
            let why = format!("{} asked kelpie to push this task", capitalized(pusher));
            if let Err(cannot) = hand_over(slot, &conflict_prompt(&why, &upstream)) {
                intent::forget(&state, &address);
                return Err(format!("{task} conflicts with {upstream} and kelpie could not hand it to its Claude ({cannot}); rebase it in {}", slot.path.display()));
            }
            tell_lead(&format!("{} pushed {address}; rebasing onto {upstream} conflicted, so its Claude resolves it as part of the push. When it reports ready, check the resolved commits against the rules (kelpie approve): kelpie pushes them for {} then, and the task stays under Ready to push meanwhile.", capitalized(pusher), by.word()), Some("push"));
            return Ok(Prepared::Handed(format!("{task} conflicts with {upstream}; its Claude resolves the rebase as part of the push, and kelpie pushes it for {pusher} once the lead has checked the resolved commits.")));
        }
        Updated::Overlap(touched) => {
            let files: Vec<&str> = touched.iter().map(|touched| touched.file.as_str()).collect();
            record(Work::Tests, fingerprint(&slot.path, &upstream)?)?;
            if let Err(cannot) = hand_over(slot, &overlap_prompt(by, &upstream, &touched)) {
                intent::forget(&state, &address);
                return Err(format!(
                    "{task} is rebased onto {upstream}, whose new commits changed files it changes too ({}), and kelpie could not hand it to its Claude ({cannot}). It is not pushed: run the tests for those files in {}, then push again.",
                    files.join(", "), slot.path.display()
                ));
            }
            return Ok(Prepared::Handed(format!(
                "{task} is rebased onto {upstream}, whose new commits changed files it changes too ({}). Its Claude runs their tests as part of the push; kelpie pushes it for {pusher} once they pass.",
                files.join(", ")
            )));
        }
    }
    intent::forget(&state, &address);
    authorize(caller, repo, slot, &upstream, through).map(Prepared::Push)
}

fn deliver(repo: &Path, slot: &Slot, upstream: &str, through: Through, pusher: &'static str, state: &Path) -> Result<String> {
    let task = task_of(slot).to_string();
    let address = peers::address(&task, &name_of(repo));
    let sha = through_sha(&slot.path, upstream, through)?;
    let keep_open = matches!(through, Through::First { .. });
    let was_approved = approved(&slot.path, upstream, &address);
    let pushed = git(&slot.path, &["log", "--reverse", "--format=%h %s", &format!("{upstream}..{sha}")])?;
    let note = started_file(&task, repo).ok().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| crate::task_note::items(&text).into_iter().next());
    let count = push_prefix(&slot.path, upstream, &sha, state).map_err(|error| format!("Pushing {task} failed: {error}"))?;
    let vault = crate::vault_cli::detect();
    let log = vault.as_ref().map(|cli| crate::task_note::log_push(cli, &slot.path, &task, &pushed));
    let logged = log.as_ref().map(crate::task_note::logged_text).unwrap_or_default();
    let target = upstream.trim_start_matches("origin/").to_string();
    let landed = |closed: bool, waits: bool| Landed { pusher, address: &address, target: &target, commits: &pushed, closed, note: note.as_deref(), logged: log.as_ref().map(|log| matches!(log, Ok(Some(_)))), waits };
    let short = &sha[..7.min(sha.len())];
    let told_ci = crate::ci::Pushed { pusher: pusher.to_string(), target: target.clone(), commits: pushed.clone(), note: note.clone(), logged: log.as_ref().map(|log| matches!(log, Ok(Some(_)))) };
    let watch = crate::ci::begin(state, repo, &address, &sha, !keep_open, told_ci);
    let ci = match &watch {
        crate::ci::Begin::Watching => format!(" kelpie watches CI for {short}."),
        crate::ci::Begin::NotWatched(reason) => format!(" CI not watched: {reason}."),
        crate::ci::Begin::Off => String::new(),
    };
    if !keep_open {
        if matches!(watch, crate::ci::Begin::Watching) {
            if let Err(error) = messages::notify_lead(&format!("{} pushed {address}: {} to {target}. kelpie watches CI for {short} and closes the task once every run is green; you hear what landed then.", capitalized(pusher), plural(count, "commit"))) {
                eprintln!("Could not tell the lead about the push: {error}");
            }
            return Ok(format!("Pushed {} to {upstream}.{logged} {task} stays open under CI running until CI for {short} is green; kelpie closes it then. {}", plural(count, "commit"), fast_forward_main(repo, upstream)?));
        }
        let ready = note_ready(vault.as_ref(), state, &address, &task);
        if let Err(waits) = &ready {
            tell_lead_pushed(&landed(false, true));
            return Ok(format!("Pushed {} to {upstream}.{logged}{ci} {waits}", plural(count, "commit")));
        }
        let closed = close_task(repo, Some(&task), vault.as_ref(), ready.unwrap_or_default(), Some(&pushed));
        tell_lead_pushed(&landed(closed.is_ok(), false));
        return closed.map(|closed| format!("Pushed {} to {upstream}.{logged}{ci} {closed}", plural(count, "commit")));
    }
    tell_lead_pushed(&landed(false, false));
    let (clean, rest) = match keep_going(state, repo, slot, upstream, &address, was_approved)? {
        Rest::Rebased { clean, rest } => (clean, rest),
        Rest::NotFetched(error) => return Ok(format!("Pushed {} through {short} to {upstream}.{logged} Fetching afterwards failed ({error}), so the rest of {task} is not rebased onto it yet: run kelpie land {task}.", plural(count, "commit"))),
    };
    let told = format!(
        "{} pushed {} of this task, through {short}, to {target}; they are on {target} for good. Your branch continues from there with {} on top: reshape only those.",
        capitalized(pusher), plural(count, "commit"), plural(rest, "commit")
    );
    if let Err(error) = messages::notify(&address, &told) {
        eprintln!("Could not tell {address} about the push: {error}");
    }
    let after = if clean { format!("{task} stays open with {} on top.", plural(rest, "commit")) } else { format!("{task} stays open; its Claude is resolving the rebase of the rest.") };
    Ok(format!("Pushed {} through {short} to {upstream}.{logged}{ci} {after} {}", plural(count, "commit"), fast_forward_main(repo, upstream)?))
}

fn tell_lead(text: &str, kind: Option<&str>) {
    if let Err(error) = messages::tell_lead(text, kind) {
        eprintln!("Could not tell the lead: {error}");
    }
}

pub fn after_ci(repo: &Path, task: &str, address: &str, told: &crate::ci::Pushed) -> Result<String> {
    let state = state_dir()?;
    let vault = crate::vault_cli::detect();
    let landed = |closed: bool, waits: bool| Landed { pusher: &told.pusher, address, target: &told.target, commits: &told.commits, closed, note: told.note.as_deref(), logged: told.logged, waits };
    let ready = note_ready(vault.as_ref(), &state, address, task);
    if let Err(waits) = &ready {
        tell_lead_pushed(&landed(false, true));
        return Ok(waits.clone());
    }
    let closed = close_task(repo, Some(task), vault.as_ref(), ready.unwrap_or_default(), Some(&told.commits));
    tell_lead_pushed(&landed(closed.is_ok(), false));
    closed
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map(|first| first.to_uppercase().chain(characters).collect()).unwrap_or_default()
}

// Who may push this task now: the user always, the lead only with the user's permission and its own rules check, a task or any other Claude never.
struct Pusher {
    caller: crate::permit::Caller,
    permit: Option<crate::permit::Permit>,
    reviewed: bool,
    approved: bool,
}

impl Pusher {
    fn allowed(&self) -> std::result::Result<&'static str, String> {
        use crate::permit::Caller;
        match &self.caller {
            Caller::User => Ok("the user"),
            Caller::Lead => {
                crate::permit::lead_may_push(self.permit, self.reviewed)?;
                if !self.approved {
                    return Err("its commits have no rules approval of yours that matches them: check them and run kelpie approve first".to_string());
                }
                Ok("the lead")
            }
            Caller::Task(_) | Caller::Agent => Err("no agent pushes; the user pushes from the board, or the lead for a task the user gave push permission".to_string()),
        }
    }
}

struct Landed<'a> {
    pusher: &'a str,
    address: &'a str,
    target: &'a str,
    commits: &'a str,
    closed: bool,
    note: Option<&'a str>,
    logged: Option<bool>,
    waits: bool,
}

// logged is None without the vault, else whether kelpie wrote the repo's log line itself; with the vault kelpie settles the note.
fn pushed_notice(landed: &Landed) -> String {
    let Landed { pusher, address, target, commits, closed, note, logged, waits } = *landed;
    let count = commits.lines().count();
    let mut text = format!("{} pushed {address}: {} to {target} ({}).", capitalized(pusher), plural(count, "commit"), commits.lines().collect::<Vec<_>>().join("; "));
    text.push_str(match (closed, waits) {
        (true, _) => " The task is done and closed.",
        (false, true) => " The task stays open until its note has its solution and decisions; kelpie asked it to write them.",
        (false, false) => " The task stays open with the rest of its branch.",
    });
    match logged {
        Some(true) => return text + " kelpie wrote the repo's log line in the vault from these commits; update the area note with what changed.",
        Some(false) => return text + " kelpie could not write the repo's log line: write it in the vault with what landed and these shas, and update the area note with what changed.",
        None => {}
    }
    text.push_str(" Write the repo's log line in the vault with what landed and these shas");
    match (note, closed) {
        (Some(note), true) => text.push_str(&format!(", and close or update its note {note}.")),
        (Some(note), false) => text.push_str(&format!(", and note in {note} what landed.")),
        (None, true) => text.push_str(", and close or update the task's note if it has one."),
        (None, false) => text.push('.'),
    }
    text
}

fn tell_lead_pushed(landed: &Landed) {
    if let Err(error) = messages::tell_lead(&pushed_notice(landed), Some("push")) {
        eprintln!("Could not tell the lead about the push: {error}");
    }
}

#[derive(Debug, PartialEq)]
enum Rest {
    Rebased { clean: bool, rest: usize },
    NotFetched(String),
}

// After a push that keeps the task open: the rest of its branch goes on top of the new default branch, an approval of the whole branch holds for the rest, and the card, written for what was pushed, goes.
fn keep_going(state: &Path, repo: &Path, slot: &Slot, upstream: &str, address: &str, was_approved: bool) -> Result<Rest> {
    crate::report::forget_card_in(state, address);
    if let Err(error) = fetch(repo) {
        return Ok(Rest::NotFetched(error));
    }
    let clean = rebase_or_hand_over(slot, upstream, "kelpie pushed this task's first commits")?;
    let rest = ahead_behind(&slot.path, upstream)?.0;
    if clean && was_approved && rest > 0 {
        record_approval(state, address, &fingerprint(&slot.path, upstream)?)?;
    }
    Ok(Rest::Rebased { clean, rest })
}

// A push through the last commit leaves nothing to keep the task open for, so it closes like a full push.
fn stays_open(keep_open: bool, sha: &str, head: &str) -> bool {
    keep_open && sha != head
}

// One of the task's commits not yet on upstream; a full push names HEAD, so a commit that arrived after the dialog opened is never pushed unseen.
fn pushable_through(path: &Path, upstream: &str, rev: &str, keep_open: bool) -> Result<String> {
    let sha = git(path, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).map_err(|_| format!("{rev} is not a commit"))?;
    let on_branch = crate::sh::succeeds("git", &["merge-base", "--is-ancestor", &sha, "HEAD"], Some(path));
    let new = git(path, &["rev-list", "--count", &format!("{upstream}..{sha}")])?.parse::<usize>().unwrap_or(0) > 0;
    if !on_branch || !new {
        return Err(format!("{} is not one of the task's commits to push", &sha[..7.min(sha.len())]));
    }
    if !keep_open && sha != git(path, &["rev-parse", "HEAD"])? {
        return Err(format!("The branch has commits after {} that were not shown; open push again to see them", &sha[..7.min(sha.len())]));
    }
    Ok(sha)
}

// Fast-forwards the default branch on origin to sha; the remote-tracking branch follows.
fn push_prefix(path: &Path, upstream: &str, sha: &str, state: &Path) -> Result<usize> {
    let count = git(path, &["rev-list", "--count", &format!("{upstream}..{sha}")])?.parse::<usize>().map_err(|error| error.to_string())?;
    let target = format!("{sha}:refs/heads/{}", upstream.trim_start_matches("origin/"));
    crate::repo::push_from(path, &[&target], state)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::test_support::*;
    use crate::repo::{ahead_behind, branch_exists, git};
    use crate::tasks::approval::{approval_file, approval_holds, fingerprint, record_approval};
    use crate::tasks::pool::slots;

    #[test]
    fn only_the_user_pushes_freely_the_lead_with_permission_and_its_approval_and_no_agent_at_all() {
        use crate::permit::{Caller, Permit};
        let check = |caller: Caller, permit: Option<Permit>, reviewed: bool, approved: bool| Pusher { caller, permit, reviewed, approved }.allowed();
        assert_eq!(check(Caller::User, None, false, false), Ok("the user"));
        assert_eq!(check(Caller::Lead, Some(Permit::Task), false, true), Ok("the lead"));
        assert!(check(Caller::Lead, None, true, true).unwrap_err().contains("no push permission"));
        assert!(check(Caller::Lead, Some(Permit::Task), true, false).unwrap_err().contains("kelpie approve"));
        assert!(check(Caller::Lead, Some(Permit::Repo), false, true).unwrap_err().contains("reviewer passed"));
        assert_eq!(check(Caller::Lead, Some(Permit::Repo), true, true), Ok("the lead"));
        for agent in [Caller::Task("billing@app".to_string()), Caller::Agent] {
            assert!(check(agent, Some(Permit::Task), true, true).unwrap_err().contains("no agent pushes"), "a permission is the lead's, never a task's");
        }
    }

    #[test]
    fn after_a_push_the_lead_hears_what_landed_and_is_cued_to_log_it_and_settle_the_note() {
        let commits = "a1b2c3d Add refunds\ne4f5a6b Show refunds on the invoice";
        let done = pushed_notice(&Landed { pusher: "the lead", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing-refunds.md"), logged: None, waits: false });
        assert_eq!(done, "The lead pushed billing@app: 2 commits to main (a1b2c3d Add refunds; e4f5a6b Show refunds on the invoice). The task is done and closed. Write the repo's log line in the vault with what landed and these shas, and close or update its note Projects/billing-refunds.md.");
        let open = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits: "a1b2c3d Add refunds", closed: false, note: None, logged: None, waits: false });
        assert!(open.starts_with("The user pushed billing@app: 1 commit to main (a1b2c3d Add refunds). The task stays open"), "{open}");
        assert!(open.ends_with("these shas."), "{open}");
    }

    #[test]
    fn with_the_vault_on_the_lead_hears_kelpie_logged_the_push_and_is_not_asked_to_log_it() {
        let commits = "a1b2c3d Add refunds";
        let kept = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing.md"), logged: Some(true), waits: false });
        assert!(kept.contains("kelpie wrote the repo's log line") && kept.contains("update the area note") && !kept.contains("Write the repo's log line"), "{kept}");
        let waits = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: false, note: Some("Projects/billing.md"), logged: Some(true), waits: true });
        assert!(waits.contains("stays open until its note has its solution and decisions"), "{waits}");
        let failed = pushed_notice(&Landed { pusher: "the user", address: "billing@app", target: "main", commits, closed: true, note: Some("Projects/billing.md"), logged: Some(false), waits: false });
        assert!(failed.contains("kelpie could not write the repo's log line: write it in the vault with what landed and these shas") && !failed.contains("kelpie wrote"), "a log append that failed leaves the line to the lead: {failed}");
    }

    #[test]
    fn push_takes_one_of_the_tasks_own_commits_and_a_full_push_only_up_to_head() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let fifth = commit(&slot.path, "e", "fifth");
        assert_eq!(pushable_through(&slot.path, &upstream, &fourth, true).unwrap(), fourth);
        assert_eq!(pushable_through(&slot.path, &upstream, &fourth[..10], true).unwrap(), fourth);
        assert_eq!(pushable_through(&slot.path, &upstream, &fifth, false).unwrap(), fifth);
        let unseen = pushable_through(&slot.path, &upstream, &fourth, false).unwrap_err();
        assert!(unseen.contains("not shown"), "a commit that arrived after the dialog opened is never pushed unseen: {unseen}");
        assert!(pushable_through(&slot.path, &upstream, &upstream, true).unwrap_err().contains("not one of the task's commits"));
        let elsewhere = commit(&dir, "x", "on main only");
        assert!(pushable_through(&slot.path, &upstream, &elsewhere, true).unwrap_err().contains("not one of the task's commits"));
        assert!(pushable_through(&slot.path, &upstream, "no-such-ref", true).unwrap_err().contains("is not a commit"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_of_the_first_commits_leaves_the_task_open_with_the_rest_on_top_and_its_approval() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        let state = dir.with_extension("state");
        let _ = std::fs::remove_dir_all(&state);
        record_approval(&state, "billing@app", &fingerprint(&slot.path, &upstream).unwrap()).unwrap();
        crate::report::save_card(&state, "billing@app", &fourth, "Refunds on the billing page.").unwrap();
        assert_eq!(push_prefix(&slot.path, &upstream, &fourth, &state).unwrap(), 1);
        let branch = upstream.trim_start_matches("origin/");
        assert_eq!(git(&origin, &["rev-parse", branch]).unwrap(), fourth, "origin has the first commit");
        assert_eq!(keep_going(&state, &dir, &slot, &upstream, "billing@app", true).unwrap(), Rest::Rebased { clean: true, rest: 1 });
        assert!(branch_exists(&dir, "billing"), "the task stays open");
        assert_eq!(git(&slot.path, &["branch", "--show-current"]).unwrap(), "billing");
        assert!(slots(&dir).unwrap().iter().any(|busy| busy.path == slot.path && !busy.free), "its worktree stays the task's");
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0), "the rest sits on top of what was pushed");
        assert_eq!(git(&slot.path, &["log", "--format=%s", &format!("{upstream}..HEAD")]).unwrap(), "fifth", "no pushed commit twice");
        assert!(approval_holds(&state, "billing@app", &fingerprint(&slot.path, &upstream).unwrap()), "an approval of the whole branch holds for the rest");
        assert!(crate::report::card_in(&state, "billing@app").is_none(), "the card was for what was pushed");
        assert!(matches!(keep_going(&state.join("other"), &dir, &slot, &upstream, "billing@app", false).unwrap(), Rest::Rebased { rest: 1, .. }));
        assert!(!approval_file(&state.join("other"), "billing@app").exists(), "no approval is made up for a branch the lead never approved");
        std::fs::remove_dir_all(state).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_partial_push_that_landed_is_reported_even_when_the_fetch_after_it_fails() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        let state = dir.with_extension("state");
        let _ = std::fs::remove_dir_all(&state);
        crate::report::save_card(&state, "billing@app", &fourth, "Refunds on the billing page.").unwrap();
        push_prefix(&slot.path, &upstream, &fourth, &state).unwrap();
        git(&dir, &["remote", "set-url", "origin", &dir.with_extension("gone").to_string_lossy()]).unwrap();
        assert!(matches!(keep_going(&state, &dir, &slot, &upstream, "billing@app", true).unwrap(), Rest::NotFetched(_)), "the commits are on origin, so the push is no failure");
        assert!(crate::report::card_in(&state, "billing@app").is_none(), "the card was for what was pushed");
        git(&dir, &["remote", "set-url", "origin", &origin.to_string_lossy()]).unwrap();
        std::fs::remove_dir_all(state).unwrap();
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_push_through_the_last_commit_closes_the_task_like_a_full_push() {
        assert!(!stays_open(true, "abc", "abc"), "p on the last commit, or --through HEAD, leaves no commit to keep open for");
        assert!(stays_open(true, "abc", "def"));
        assert!(!stays_open(false, "abc", "def"));
    }

    // A file both main and the task change, apart, so the rebase is clean but the files overlap.
    fn shared_file(dir: &std::path::Path, slot: &crate::tasks::Slot, upstream: &str) {
        upstream_commit(dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n", "add m");
        git(&slot.path, &["rebase", "--quiet", upstream]).unwrap();
        std::fs::write(slot.path.join("m"), "one\n2\n3\n4\n5\n6\n7\n8\n9\n10\n").unwrap();
        git(&slot.path, &["commit", "--quiet", "-am", "first line of m"]).unwrap();
    }

    #[test]
    fn a_rebase_over_other_files_is_clean_over_the_same_files_is_an_overlap_and_a_conflict_is_aborted() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Clean);
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0));
        shared_file(&dir, &slot, &upstream);
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        let Updated::Overlap(touched) = update(&slot.path, &upstream).unwrap() else { panic!("main changed m too") };
        assert_eq!(touched.len(), 1);
        assert_eq!(touched[0].file, "m");
        assert!(touched[0].commits.len() == 1 && touched[0].commits[0].ends_with(" last line of m"), "{touched:?}");
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (2, 0), "an overlap is rebased all the same");
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        upstream_commit(&dir, "d", "theirs", "d on main");
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Conflict);
        assert!(!rebase_in_progress(&slot.path) && git(&slot.path, &["rev-parse", "HEAD"]).unwrap() == head, "a conflict is aborted and leaves the branch as it was");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_review_open_across_a_rebase_still_shows_only_the_tasks_own_changes() {
        let (dir, origin, slot, upstream) = task_with_a_commit("review");
        let state = state_of(&dir);
        let herdr = state.join("herdr");
        std::fs::write(&herdr, "#!/bin/sh\necho '{\"result\":{\"pane\":{\"pane_id\":\"p_review\"}}}'\n").unwrap();
        std::fs::set_permissions(&herdr, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/review.sh");
        let review = |args: &[&str]| {
            let output = std::process::Command::new("sh").arg(&script).args(args).current_dir(&slot.path)
                .env("KELPIE_STATE_DIR", &state).env("HERDR_BIN_PATH", &herdr).env("HERDR_PANE_ID", "p_claude").output().unwrap();
            assert!(output.status.success(), "review.sh {args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        review(&["split"]);
        let d = state.join("reviews/p_claude");
        let files = |rows: &str| rows.lines().skip(1).map(|line| line.split('\t').next().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(files(&review(&["rows", &d.to_string_lossy()])), ["d"]);
        land_on_main(&dir, &upstream, |_| {});
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Clean);
        assert_eq!(files(&review(&["rows", &d.to_string_lossy()])), ["d"], "main's commits stay out of a review opened before the rebase");
        let base = std::fs::read_to_string(d.join("base")).unwrap();
        assert_eq!(review(&["diff", base.trim(), "elsewhere"]), "", "the open diff compares against the fork point after the rebase too");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_rebase_git_refuses_to_start_reports_gits_reason_and_the_hidden_files_and_is_no_conflict() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        git(&slot.path, &["update-index", "--skip-worktree", "a"]).unwrap();
        std::fs::write(slot.path.join("a"), "local only").unwrap();
        assert!(uncommitted(&slot.path).unwrap().is_empty(), "git status does not show it");
        upstream_commit(&dir, "a", "theirs", "a on main");
        let refused = update(&slot.path, &upstream).unwrap_err();
        assert!(refused.contains("would be overwritten") || refused.contains("overwritten"), "git's own reason: {refused}");
        assert!(refused.contains("skip-worktree") && refused.contains(": a."), "{refused}");
        assert!(!refused.contains("no rebase in progress") && !rebase_in_progress(&slot.path), "{refused}");
        cleanup(dir, origin, &slot);
    }

    struct Pushing {
        state: std::path::PathBuf,
        address: String,
    }

    impl Pushing {
        fn new(dir: &std::path::Path) -> Pushing {
            Pushing { state: crate::lead::state_dir().unwrap(), address: crate::peers::address("billing", &name_of(dir)) }
        }
    }

    impl Drop for Pushing {
        fn drop(&mut self) {
            intent::forget(&self.state, &self.address);
            let _ = crate::permit::set_task_in(&self.state, &self.address, false);
            let _ = std::fs::remove_file(approval_file(&self.state, &self.address));
        }
    }

    type Typed = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    fn handed() -> (Typed, impl Fn(&Slot, &str) -> std::result::Result<(), String>) {
        let texts = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let kept = texts.clone();
        (texts, move |_: &Slot, text: &str| { kept.borrow_mut().push(text.to_string()); Ok(()) })
    }

    #[test]
    fn being_behind_main_never_blocks_a_push_it_rebases_first_and_untracked_files_do_not_block_it() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let _pushing = Pushing::new(&dir);
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        std::fs::write(slot.path.join("notes.txt"), "scratch").unwrap();
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 1));
        let (texts, hand) = handed();
        assert_eq!(prepare(&Caller::User, &dir, &slot, Through::All, &hand).unwrap(), Prepared::Push("the user"));
        assert_eq!(ahead_behind(&slot.path, &upstream).unwrap(), (1, 0), "rebased, ready for the push");
        assert!(texts.borrow().is_empty(), "nothing for its Claude to do");
        std::fs::write(slot.path.join("d"), "half done").unwrap();
        let dirty = prepare(&Caller::User, &dir, &slot, Through::All, &hand).unwrap_err();
        assert!(dirty.contains("1 uncommitted file") && dirty.contains("(d)"), "{dirty}");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn no_agent_and_no_lead_without_permission_can_start_a_push_and_nothing_is_touched_or_recorded() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        shared_file(&dir, &slot, &upstream);
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        record_approval(&pushing.state, &pushing.address, &fingerprint(&slot.path, &upstream).unwrap()).unwrap();
        let (texts, hand) = handed();
        for caller in [Caller::Agent, Caller::Task(pushing.address.clone()), Caller::Lead] {
            let refused = prepare(&caller, &dir, &slot, Through::All, &hand).unwrap_err();
            assert!(refused.contains("no agent pushes") || refused.contains("no push permission"), "{refused}");
            assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), head, "refused before the rebase");
            assert!(intent::load(&pushing.state, &pushing.address).is_none() && texts.borrow().is_empty());
        }
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_rebase_over_the_tasks_own_files_hands_it_to_its_claude_and_records_the_push_for_who_asked() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        shared_file(&dir, &slot, &upstream);
        upstream_commit(&dir, "m", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n", "last line of m");
        crate::permit::set_task_in(&pushing.state, &pushing.address, true).unwrap();
        record_approval(&pushing.state, &pushing.address, &fingerprint(&slot.path, &upstream).unwrap()).unwrap();
        let (texts, hand) = handed();
        let Prepared::Handed(said) = prepare(&Caller::Lead, &dir, &slot, Through::All, &hand).unwrap() else { panic!("main changed m too") };
        assert!(said.contains("(m)") && said.contains("for the lead"), "{said}");
        let typed = texts.borrow()[0].clone();
        assert!(typed.contains("- m: ") && typed.contains("last line of m") && typed.contains("Do not push") && typed.contains("report ready"), "{typed}");
        let recorded = intent::load(&pushing.state, &pushing.address).unwrap();
        assert_eq!((recorded.by, recorded.fingerprint.as_str(), recorded.through), (By::Lead, fingerprint(&slot.path, &upstream).unwrap().as_str(), Through::All));
        assert!(approved(&slot.path, &upstream, &pushing.address), "the clean rebase kept the approval");
        let waits = prepare(&Caller::Lead, &dir, &slot, Through::All, &hand).unwrap_err();
        assert!(waits.contains("being pushed already"), "{waits}");
        assert_eq!(prepare(&Caller::User, &dir, &slot, Through::All, &hand).unwrap(), Prepared::Push("the user"), "the user may push past it");
        assert!(intent::load(&pushing.state, &pushing.address).is_none());
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_conflict_on_push_goes_to_its_claude_with_the_intent_rule_and_records_the_push_for_who_asked() {
        use crate::permit::Caller;
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let pushing = Pushing::new(&dir);
        upstream_commit(&dir, "d", "theirs", "d on main");
        let head = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        let (texts, hand) = handed();
        let Prepared::Handed(said) = prepare(&Caller::User, &dir, &slot, Through::All, &hand).unwrap() else { panic!("the rebase conflicts") };
        assert!(said.contains("conflicts with") && said.contains("as part of the push"), "{said}");
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), head);
        let typed = texts.borrow()[0].clone();
        assert!(typed.contains("kelpie ask") && typed.contains("recommendation") && typed.contains("Do not push") && typed.contains("kelpie pushes the resolved commits"), "{typed}");
        let recorded = intent::load(&pushing.state, &pushing.address).unwrap();
        assert_eq!((recorded.by, recorded.work, recorded.fingerprint), (By::User, Work::Conflict, fingerprint(&slot.path, &upstream).unwrap()));
        let failing = |_: &Slot, _: &str| Err("the log is locked".to_string());
        let refused = prepare(&Caller::User, &dir, &slot, Through::All, &failing).unwrap_err();
        assert!(refused.contains("could not hand it to its Claude (the log is locked)"), "{refused}");
        assert!(intent::load(&pushing.state, &pushing.address).is_none(), "a push nobody works on is not shown as under way");
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn a_partial_push_keeps_its_place_across_the_rebase() {
        let (dir, origin, slot, upstream) = task_with_a_commit("billing");
        let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
        commit(&slot.path, "e", "fifth");
        let through = chosen(&slot, &upstream, Some(&fourth), true).unwrap();
        assert_eq!(through, Through::First { count: 1, of: 2 });
        assert_eq!(chosen(&slot, &upstream, None, false).unwrap(), Through::All);
        upstream_commit(&dir, "elsewhere", "x", "someone else's work");
        assert_eq!(update(&slot.path, &upstream).unwrap(), Updated::Clean);
        let rebased = through_sha(&slot.path, &upstream, through).unwrap();
        assert_ne!(rebased, fourth);
        assert_eq!(git(&slot.path, &["log", "-1", "--format=%s", &rebased]).unwrap(), "fourth");
        assert!(through_sha(&slot.path, &upstream, Through::First { count: 1, of: 3 }).unwrap_err().contains("open push again"));
        cleanup(dir, origin, &slot);
    }

    #[test]
    fn capitalized_takes_the_first_character_up_and_leaves_the_rest() {
        assert_eq!(capitalized("the lead"), "The lead");
        assert_eq!(capitalized(""), "");
        assert_eq!(capitalized("ärlig"), "Ärlig");
        assert_eq!(capitalized("ßx"), "SSx");
    }
}
