use std::fs;
use std::path::{Path, PathBuf};

use crate::herdr;
use crate::hooks::{Answer, Payload};
use crate::lead::state_dir;
use crate::messages::{self, Report};
use crate::peers;
use crate::repo::{default_branch, git, main_checkout, name_of};
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use crate::tasks;

pub const STATES: [&str; 4] = ["needs-you", "ready", "working", "blocked"];

const USAGE: &str = "kebbie report --state needs-you|ready|working|blocked --next \"<what the user should do next, or nothing>\" [--card <file>, with ready] [--after <task[@repo]>, with ready] [--after-push \"<what you verify once the push lands>\", with ready] [--red-cause \"<the line that fails and why>\", with ready] \"<where the task stands>\"";
const CHECK_LIMIT: usize = 300;

pub fn parse(state: Option<&str>, next: Option<&str>, card: bool, summary: &str) -> Result<(Report, String)> {
    let state = state.ok_or(format!("kebbie report needs --state: {USAGE}"))?;
    if !STATES.contains(&state) {
        return Err(format!("--state is one of {}, not \"{state}\"", STATES.join(", ")));
    }
    if card && !["ready", "needs-you"].contains(&state) {
        return Err(format!("--card goes with --state ready: the user reads the card when they push. {USAGE}"));
    }
    let next = next.map(str::trim).filter(|next| !next.is_empty()).ok_or(format!("kebbie report needs --next (\"nothing\" when there is nothing for the user): {USAGE}"))?;
    let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        return Err(format!("kebbie report needs a one-line summary: {USAGE}"));
    }
    Ok((Report { state: state.to_string(), next: next.split_whitespace().collect::<Vec<_>>().join(" "), head: None, after: Vec::new() }, summary))
}

#[allow(clippy::too_many_arguments)]
pub fn report(cwd: &Path, state: Option<&str>, next: Option<&str>, card: Option<&str>, after: &[&str], after_push: Option<&str>, red_cause: Option<&str>, summary: &str) -> Result<String> {
    let (mut report, summary) = parse(state, next, card.is_some(), summary)?;
    let check = check_goes_with(&report.state, after_push)?;
    if red_cause.is_some() && report.state != "ready" {
        return Err(format!("--red-cause goes with --state ready: it names the cause of a red test you leave red. {USAGE}"));
    }
    report.head = crate::repo::git(cwd, &["rev-parse", "HEAD"]).ok();
    let me = peers::me(cwd).filter(|me| me != LEAD_LABEL).ok_or("kebbie report runs in a task's worktree")?;
    report.after = push_after(&report.state, &me, after)?;
    card_goes_with(&report.state, card.is_some(), crate::routines::routine_of(&me).is_some())?;
    if let Some(watch) = crate::ci::load(&state_dir()?, &me).filter(|watch| (watch.close || !watch.pushed.stages.is_empty()) && watch.checking().is_some()) {
        let ahead = main_checkout(cwd).and_then(|repo| default_branch(&repo)).and_then(|upstream| crate::repo::ahead_behind(cwd, &upstream)).map_or(0, |(ahead, _)| ahead);
        if let Some(said) = check_reported(&state_dir()?, &me, &watch, &report, &summary, ahead)? {
            return Ok(said);
        }
    }
    let red = if report.state == "ready" {
        crate::rules_check::refuse_unformed(cwd)?;
        red_gate(&crate::evidence::runs_in(&state_dir()?, &me), red_cause)?
    } else {
        None
    };
    let card = card.map(|file| cwd.join(file));
    let text = card.as_deref().map(read_card).transpose()?;
    let sign_off = report.state == "ready" && crate::routines::routine_of(&me).is_none();
    if let Some(text) = text.as_deref().filter(|_| sign_off) {
        crate::card::parse(text)?;
    }
    messages::log_report(&me, &report, &summary)?;
    let state = state_dir()?;
    tasks::subtasks_seen_in(&state, cwd);
    let pushing = tasks::pending_push(&state, &me).is_some();
    crate::phase::record(&state, &me, "report", &crate::phase::Event::Reported { state: &report.state, head: report.head.as_deref(), committed: crate::status::committed_at(cwd), pushing });
    let mut kept = match (&card, &text) {
        (Some(file), Some(text)) => take_card(cwd, file, &state, &me, text, sign_off, red.as_deref())?,
        _ => String::new(),
    };
    if let Some(cause) = &red {
        let line = format!("the latest test run stays red: {cause} (task)");
        let note = crate::decisions::write(&state, &me, &line, messages::now())?;
        kept.push_str(&crate::decisions::for_the_task(&note, &line).map(|write| format!(" {write}")).unwrap_or_default());
    }
    if let Some(check) = &check {
        save_check(&state, &me, check)?;
        kept.push_str(" kebbie asks you to run your check once the push lands, and closes the task when it holds.");
    }
    show_in_sidebar(&report);
    if report.state == "ready" && crate::task_note::linked(&state, &me).is_some() && let Some(cli) = crate::vault_cli::detect() {
        crate::task_note::heartbeat(&cli, &state, &me);
    }
    if crate::notify::Event::of_state(&report.state).is_some() && crate::routines::found(&me, &summary) {
        crate::notify::send(cwd, &crate::notify::Notice::from(crate::notify::Event::RoutineFindings, &me, format!("{me} found: {summary}")));
    } else if let Some(event) = crate::notify::Event::of_state(&report.state) {
        let text = if event == crate::notify::Event::Ready { format!("{me} is ready: {summary}") } else { format!("{me} needs you: {}", report.next) };
        crate::notify::send(cwd, &crate::notify::Notice::from(event, &me, text));
        if event == crate::notify::Event::Ready {
            crate::hub::notices::send(&ready_notice(cwd, &state, &me, &summary), None);
        }
    }
    if report.state == "ready" {
        crate::commit_tests::on_ready(cwd);
    }
    ask_to_close(cwd, &state, &me, &report, &summary);
    let check = if report.state == "ready" { review_again(cwd) + &crate::assessment::on_ready(cwd) + &rules_check(cwd) } else { String::new() };
    Ok(format!("Reported to the lead and the board as {me}.{kept}{check}"))
}

// While its check after the push runs, a task's report is the check's outcome: ready with nothing left to push is that it held, needs-you that it did not.
fn check_reported(state: &Path, me: &str, watch: &crate::ci::Watch, report: &Report, summary: &str, ahead: usize) -> Result<Option<String>> {
    let check = watch.checking().map(|check| check.what.clone()).unwrap_or_default();
    let staged = !watch.pushed.stages.is_empty();
    match report.state.as_str() {
        "ready" if ahead == 0 || staged => {
            let passed = crate::ci::Check { passed: true, ..watch.pushed.check.clone().unwrap_or_default() };
            crate::ci::save(state, &crate::ci::Watch { pushed: crate::ci::Pushed { check: Some(passed), ..watch.pushed.clone() }, ..watch.clone() })?;
            messages::log_report(me, report, summary)?;
            show_in_sidebar(report);
            let then = if staged { "kebbie marks its stage met on its next round" } else { "kebbie closes the task as done" };
            Ok(Some(format!("Reported that the check after the push held; {then}.")))
        }
        "ready" => {
            crate::ci::forget(state, me);
            let head = report.head.clone();
            crate::phase::update(state, me, "report", |_| Some(crate::phase::Phase::Ready { head }));
            Ok(None)
        }
        "needs-you" => {
            crate::ci::forget(state, me);
            if staged {
                crate::tasks::rollout::failed(state, me, &watch.pushed.stages, &format!("the check did not hold: {}", report.next), false);
            }
            if let Err(error) = messages::to_lead(messages::ToLead::CiHanded, &format!("The check {me} ran after its push did not hold ({check}): {}. It waits for the user under Needs you.", report.next)) {
                eprintln!("Could not tell the lead: {error}");
            }
            Ok(None)
        }
        _ => {
            messages::log_report(me, report, summary)?;
            show_in_sidebar(report);
            Ok(Some("Reported; kebbie waits for the check's outcome: report ready when it held, needs-you when it did not.".to_string()))
        }
    }
}

// A task done with nothing to push, a research task or a routine's findings, would wait for the user to close it by hand; kebbie asks instead, beside whatever else the task waits on.
fn ask_to_close(cwd: &Path, state: &Path, me: &str, report: &Report, summary: &str) {
    let routine = crate::routines::routine_of(me).is_some();
    if !(report.state == "ready" || (routine && report.state == "needs-you")) || crate::ci::load(state, me).is_some() || tasks::pending_push(state, me).is_some() {
        return;
    }
    let nothing = main_checkout(cwd).and_then(|repo| default_branch(&repo)).and_then(|upstream| crate::repo::ahead_behind(cwd, &upstream)).is_ok_and(|(ahead, _)| ahead == 0) && crate::repo::is_clean(cwd).unwrap_or(false);
    let staged = crate::tasks::rollout::all_met(state, me);
    if !nothing || !staged {
        return;
    }
    let task = me.split_once('@').map_or(me, |(task, _)| task);
    if let Err(error) = crate::kebbie_questions::ask_close(state, me, &format!("{task} says it is done: {summary}. Close it?")) {
        eprintln!("kebbie could not ask you to close {me}: {error}");
    }
}

// A ready report over a red test run names the cause found, the line that fails and why; one that names a cause for a run that passed is refused too.
pub fn red_gate(runs: &[crate::evidence::Run], cause: Option<&str>) -> Result<Option<String>> {
    let cause = cause.map(|cause| cause.split_whitespace().collect::<Vec<_>>().join(" "));
    let latest = runs.last();
    let red = latest.filter(|run| run.result == crate::evidence::Outcome::Failed);
    match (red, cause) {
        (Some(run), None) => Err(format!(
            "Your latest test run is red: `{}` failed at {}. Find its cause, the line that fails and why, before you report ready: fix it when your change caused it; raise it with kebbie obstacle when it did not and it blocks your work; or, once the cause is found and it stays red, report ready again with --red-cause \"<the line that fails and why>\", which the card shows as your decision.",
            run.command, run.short()
        )),
        (_, Some(cause)) if cause.is_empty() => Err(format!("--red-cause needs the cause found: the line that fails and why. {USAGE}")),
        (None, Some(_)) => Err("--red-cause names why the latest test run is red, and it is not red: report ready without it.".to_string()),
        (Some(_), Some(cause)) => Ok(Some(cause)),
        (None, None) => Ok(None),
    }
}

// A check after the push is the task's to run, so it goes with a ready report, one line the board shows under CI running.
pub fn check_goes_with(state: &str, after_push: Option<&str>) -> Result<Option<String>> {
    let Some(check) = after_push else { return Ok(None) };
    if state != "ready" {
        return Err(format!("--after-push goes with --state ready: it names what you verify once the push lands. {USAGE}"));
    }
    let check = check.split_whitespace().collect::<Vec<_>>().join(" ");
    if check.is_empty() {
        return Err(format!("--after-push needs what you verify once the push lands: {USAGE}"));
    }
    if check.chars().count() > CHECK_LIMIT {
        return Err(format!("--after-push is over {CHECK_LIMIT} characters; the board shows it on one line, so name the check, not its steps"));
    }
    Ok(Some(check))
}

fn check_file(state: &Path, address: &str) -> PathBuf {
    state.join("checks").join(address)
}

pub fn save_check(state: &Path, address: &str, check: &str) -> Result<()> {
    crate::state::write(&check_file(state, address), check)
}

pub fn check_in(state: &Path, address: &str) -> Option<String> {
    fs::read_to_string(check_file(state, address)).ok().map(|text| text.trim().to_string()).filter(|text| !text.is_empty())
}

pub fn forget_check_in(state: &Path, address: &str) {
    let _ = fs::remove_file(check_file(state, address));
}

fn ready_notice(cwd: &Path, state: &Path, me: &str, summary: &str) -> crate::hub::notices::About {
    let commits = main_checkout(cwd).and_then(|repo| default_branch(&repo)).and_then(|upstream| crate::repo::ahead_behind(cwd, &upstream)).map_or(0, |(ahead, _)| ahead);
    let review = match crate::assessment::word_in(state, me) {
        Some(crate::assessment::Word::Assessing | crate::assessment::Word::NotAssessed) | None => None,
        Some(word) => Some(word.text()),
    };
    let subtasks = crate::tasks::subtasks_for(cwd, false);
    let to_review = subtasks.listed().then(|| subtasks.items.iter().filter(|subtask| subtask.state == crate::tasks::SubtaskState::Ready).count());
    crate::hub::notices::About::Ready { address: me.to_string(), does: summary.to_string(), commits, review, to_review }
}

// The tasks a ready task must be pushed after, each as task@repo; a bare name is a task in its own repo.
pub fn push_after(state: &str, me: &str, after: &[&str]) -> Result<Vec<String>> {
    if !after.is_empty() && state != "ready" {
        return Err(format!("--after goes with --state ready: it names a task to push before this one. {USAGE}"));
    }
    let repo = me.split_once('@').map_or("", |(_, repo)| repo);
    after.iter().map(|named| {
        let named = named.trim();
        let address = if named.contains('@') { named.to_string() } else { peers::address(named, repo) };
        let (task, place) = address.split_once('@').unwrap_or((&address, ""));
        if task.is_empty() || place.is_empty() || address.chars().any(char::is_whitespace) {
            return Err(format!("--after takes a task as <task> or <task@repo>, not \"{named}\""));
        }
        if address == me {
            return Err("--after names another task, not this one".to_string());
        }
        Ok(address)
    }).collect()
}

// A routine's task lists its findings on a card while it waits on the user; any other task writes one only when it is ready.
pub fn card_goes_with(state: &str, card: bool, routine: bool) -> Result<()> {
    if card && state != "ready" && !(routine && state == "needs-you") {
        return Err(format!("--card goes with --state ready (or needs-you in a routine's task): the user reads the card when they push. {USAGE}"));
    }
    Ok(())
}

fn review_again(cwd: &Path) -> String {
    match crate::review::again_if_stale(cwd) {
        Ok(Some(_)) => " Your review read older code, so the reviewer reads the branch again; the lead hears its verdict.".to_string(),
        Ok(None) => String::new(),
        Err(error) => format!(" The reviewer could not be started again: {error}."),
    }
}

pub struct Card {
    pub sha: String,
    pub text: String,
    pub file: PathBuf,
    pub data: Option<crate::card::Data>,
}

const CARD_LIMIT: usize = 4_000;

fn card_file(state: &Path, address: &str) -> PathBuf {
    state.join("cards").join(format!("{address}.md"))
}

pub fn read_card(file: &Path) -> Result<String> {
    let text = fs::read_to_string(file).map_err(|error| format!("--card {}: {error}", file.display()))?;
    let text = text.trim();
    if text.is_empty() {
        return Err(format!("--card {} is empty: write the change in the user's words, where to look, and what to confirm", file.display()));
    }
    if text.chars().count() > CARD_LIMIT {
        return Err(format!("--card {} is over {CARD_LIMIT} characters; the user reads it in the push dialog, so keep it to a dozen lines", file.display()));
    }
    Ok(text.to_string())
}

pub fn save_card(state: &Path, address: &str, sha: &str, text: &str) -> Result<()> {
    let file = card_file(state, address);
    crate::state::write(&file, format!("card: {sha}\n\n{text}\n"))
}

// An untracked card left in the worktree would count as an uncommitted change and stop the push, so kebbie keeps the card and removes the file; its Also noticed lines are filed as it is taken, and a sign-off card is kept as data with who took each decision from kebbie's own records.
fn take_card(cwd: &Path, file: &Path, state: &Path, address: &str, text: &str, sign_off: bool, red: Option<&str>) -> Result<String> {
    let now = messages::now();
    let filed = crate::found::file_card(state, address, text, now)?;
    let text = filed.text;
    save_card(state, address, &git(cwd, &["rev-parse", "HEAD"])?, &text)?;
    let mut said = String::new();
    if sign_off {
        let parsed = crate::card::parse(&text)?;
        let (decisions, unbacked) = crate::card::decisions(&parsed.decisions, &crate::card::records(state, address), red);
        let data = crate::card::next_version(&parsed, decisions, crate::card::load(state, address), now);
        crate::card::save(state, address, &data)?;
        if data.changed_at == Some(now) {
            said.push_str(&format!(" It replaces your card's version {}; the user sees what changed.", data.version - 1));
        }
        if !unbacked.is_empty() {
            said.push_str(&format!(" kebbie has no record of anyone but you taking these, so the card shows them as (task): {}. A choice that is the user's is asked with kebbie ask; a question of method goes to the lead.", unbacked.join("; ")));
        }
    } else {
        crate::card::forget(state, address);
    }
    for item in &filed.recurring {
        match crate::obstacle::raise_recurring(state, address, cwd, item) {
            Ok(told) => said.push_str(&told),
            Err(error) => said.push_str(&format!(" found:{} was found by a second task, and raising it failed: {error}; tell the lead.", item.id)),
        }
    }
    let untracked = !crate::sh::succeeds("git", &["ls-files", "--error-unmatch", &file.to_string_lossy()], Some(cwd));
    let top = git(cwd, &["rev-parse", "--show-toplevel"])?;
    let inside = file.canonicalize().is_ok_and(|file| Path::new(&top).canonicalize().is_ok_and(|top| file.starts_with(top)));
    if untracked && inside {
        fs::remove_file(file).map_err(|error| format!("{}: {error}", file.display()))?;
    }
    let filed = if text.contains("(found:") { format!(" Its Also noticed lines are filed under {} on the board's backlog, each line ending with its item.", crate::found::SECTION) } else { String::new() };
    Ok(format!(" The card is kept for the push dialog.{filed}{said}"))
}

pub fn card_in(state: &Path, address: &str) -> Option<Card> {
    let file = card_file(state, address);
    let text = fs::read_to_string(&file).ok()?;
    let (first, body) = text.split_once('\n')?;
    let sha = first.strip_prefix("card: ")?.trim().to_string();
    let data = crate::card::load(state, address);
    let text = data.as_ref().map_or_else(|| body.trim().to_string(), |data| crate::card::render(data, &crate::found::load(state)));
    Some(Card { sha, text, file, data })
}

// The card of the task whose worktree this is, and whether HEAD still makes the change it was written for.
pub fn card_for(path: &Path) -> Option<(Card, bool)> {
    let repo = main_checkout(path).ok()?;
    let slot = tasks::find_task(&repo, None, path).ok()?;
    let upstream = default_branch(&repo).ok()?;
    let card = card_in(&state_dir().ok()?, &peers::address(tasks::task_of(&slot), &name_of(&repo)))?;
    let head = git(&slot.path, &["rev-parse", "HEAD"]).ok()?;
    let current = tasks::same_code(&slot.path, &upstream, &card.sha, &head);
    Some((card, current))
}

pub fn forget_card(task: &str, repo: &Path) {
    if let Ok(state) = state_dir() {
        forget_card_in(&state, &peers::address(task, &name_of(repo)));
    }
}

pub fn forget_card_in(state: &Path, address: &str) {
    let _ = fs::remove_file(card_file(state, address));
    crate::card::forget(state, address);
}

// herdr's sidebar shows the same as the board: the next step as $next, and the state in place of idle or done.
pub fn show_in_sidebar(report: &Report) {
    if let Ok(pane) = std::env::var("HERDR_PANE_ID") {
        let agent = herdr::agents().ok().and_then(|agents| agents.iter().find(|agent| agent["pane_id"] == pane.as_str()).and_then(crate::agent::Agent::of)).unwrap_or(crate::agent::Agent::Claude);
        let args = metadata_args(report, agent);
        let _ = herdr::report_metadata(&pane, &args.iter().map(String::as_str).collect::<Vec<_>>());
    }
}

// A ready task waits under Ready to push, marked, until the reviewer has checked it against the rules skill, or the lead when the reviewer gives no verdict.
fn rules_check(cwd: &Path) -> String {
    use crate::rules_check::Started;
    match crate::rules_check::start(cwd) {
        Ok(Started::Queued) => " The reviewer checks your commits against the rules in the background; kebbie tells you if one needs fixing.".to_string(),
        Ok(Started::Retold(findings)) => format!(" The rules check already found these in these commits; fix each, then report ready again:\n{findings}"),
        Ok(Started::ByReview) => " Your reviewer's verdict covers the rules too.".to_string(),
        Ok(Started::Lead) => " The lead checks your commits against the rules next; it tells you if one needs fixing.".to_string(),
        Ok(Started::Nothing) => String::new(),
        Err(error) => format!(" The rules check could not be started: {error}."),
    }
}

fn label(state: &str) -> Option<&'static str> {
    match state {
        "needs-you" => Some("needs you"),
        "ready" => Some("ready"),
        "blocked" => Some("blocked"),
        _ => None,
    }
}

// herdr replaces a source's labels with each new set, and refuses to set and clear the same field at once.
pub fn metadata_args(report: &Report, agent: crate::agent::Agent) -> Vec<String> {
    let mut args: Vec<String> = ["--source", "kebbie", "--agent", agent.kind()].map(str::to_string).to_vec();
    let next = report.next.trim();
    if next.is_empty() || next.eq_ignore_ascii_case("nothing") {
        args.extend(["--clear-token".to_string(), "next".to_string()]);
    } else {
        args.extend(["--token".to_string(), format!("next={next}")]);
    }
    match label(&report.state) {
        Some(label) => args.extend(["idle", "done"].iter().flat_map(|status| ["--state-label".to_string(), format!("{status}={label}")])),
        None => args.push("--clear-state-labels".to_string()),
    }
    args
}

fn turn_file(me: &str) -> Result<PathBuf> {
    let dir = state_dir()?.join("turns");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join(me))
}

// When the task's latest turn began: a prompt typed into it, by the user or as a message.
pub fn turn_started(me: &str) -> Option<u64> {
    fs::read_to_string(turn_file(me).ok()?).ok()?.trim().parse().ok()
}

// Claude Code sets stop_hook_active on the stop that follows a block, so a session is asked once per turn.
pub fn should_block(stop_hook_active: bool, turn_started: Option<u64>, last_report: Option<u64>) -> bool {
    !stop_hook_active && unreported(turn_started, last_report)
}

// The stop after the one sent back, with still no report from this turn.
fn unreported(turn_started: Option<u64>, last_report: Option<u64>) -> bool {
    last_report.is_none_or(|at| at < turn_started.unwrap_or(0))
}

fn block_reason(kebbie: &str) -> String {
    format!("Before you stop, report where this task stands, so the board shows the user what to act on: {kebbie} report --state needs-you|ready|working|blocked --next \"<what the user should do next, or nothing>\" \"<one-line summary>\". Then stop; there is no need to repeat your answer.")
}

// Both hooks kebbie gives a task's Claude: a prompt marks the start of a turn, and a stop without a report since then is sent back once.
pub fn hook(input: &str) -> String {
    answer(&Payload::parse(input)).render()
}

pub fn answer(input: &Payload) -> Answer {
    let cwd = input.cwd().map(PathBuf::from).or_else(|| std::env::current_dir().ok());
    let Some(me) = cwd.and_then(|cwd| peers::me(&cwd)).filter(|me| me != LEAD_LABEL) else { return Answer::Silent };
    let Ok(turn) = turn_file(&me) else { return Answer::Silent };
    match input.event() {
        Some("UserPromptSubmit") => {
            let _ = crate::state::write(&turn, messages::now().to_string());
            if let Ok(state) = state_dir() {
                let by_user = input.prompt().is_none_or(crate::phase::by_user);
                crate::phase::record(&state, &me, "turn", &crate::phase::Event::TurnBegan { by_user });
            }
            crate::known::stale_for(&me).map_or(Answer::Silent, Answer::Text)
        }
        Some("Stop") => {
            let turn_started = fs::read_to_string(turn).ok().and_then(|at| at.trim().parse().ok());
            let last_report = messages::latest_reports(&messages::load()).get(&me).map(|message| message.at);
            if !should_block(input.stop_hook_active(), turn_started, last_report) {
                if unreported(turn_started, last_report) && let Ok(state) = state_dir() {
                    crate::phase::record(&state, &me, "stop", &crate::phase::Event::TurnEndedUnreported);
                }
                return Answer::Silent;
            }
            let kebbie = peers::kebbie_command().unwrap_or_else(|_| "kebbie".to_string());
            Answer::Block(block_reason(&kebbie))
        }
        _ => Answer::Silent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_needs_a_known_state_a_next_step_and_a_summary() {
        let (report, summary) = parse(Some("needs-you"), Some(" answer:  which queue? "), false, "plan  written,\nwaiting").unwrap();
        assert_eq!(report, Report { state: "needs-you".to_string(), next: "answer: which queue?".to_string(), head: None, after: Vec::new() });
        assert_eq!(summary, "plan written, waiting");
        assert!(parse(None, Some("nothing"), false, "x").is_err());
        assert!(parse(Some("done"), Some("nothing"), false, "x").is_err());
        assert!(parse(Some("ready"), None, false, "x").is_err());
        assert!(parse(Some("ready"), Some("  "), false, "x").is_err());
        assert!(parse(Some("ready"), Some("push and close"), false, " ").is_err());
    }

    #[test]
    fn a_ready_report_names_the_tasks_to_push_before_it_and_keeps_them_in_the_log() {
        assert_eq!(push_after("ready", "payload-v2@web", &["payload-v2@api", "rounding"]), Ok(vec!["payload-v2@api".to_string(), "rounding@web".to_string()]), "a bare name is a task in its own repo");
        assert!(push_after("working", "payload-v2@web", &["payload-v2@api"]).unwrap_err().contains("--after goes with --state ready"));
        assert!(push_after("ready", "payload-v2@web", &["payload-v2"]).unwrap_err().contains("not this one"));
        assert!(push_after("ready", "payload-v2@web", &["a b@api"]).unwrap_err().contains("--after takes a task"));
        assert_eq!(push_after("ready", "payload-v2@web", &[]), Ok(Vec::new()));
        let logged = r#"{"id":"r1","at":10,"from":"payload-v2@web","to":"lead","text":"done","state":"logged","report":{"state":"ready","next":"push","head":null,"after":["payload-v2@api"]}}"#;
        assert_eq!(messages::fold(logged)[0].report.as_ref().map(|report| report.after.clone()), Some(vec!["payload-v2@api".to_string()]));
    }

    #[test]
    fn a_card_goes_with_a_ready_report_and_a_routines_findings_only() {
        assert!(parse(Some("ready"), Some("push and close"), true, "refunds done").is_ok());
        let error = parse(Some("working"), Some("nothing"), true, "x").err().unwrap();
        assert!(error.contains("--card goes with --state ready"), "{error}");
        assert_eq!(card_goes_with("ready", true, false), Ok(()));
        assert!(card_goes_with("needs-you", true, false).unwrap_err().contains("--card goes with --state ready (or needs-you in a routine's task)"));
        assert_eq!(card_goes_with("needs-you", true, true), Ok(()), "a routine's findings wait on the user with their card");
        assert!(card_goes_with("blocked", true, true).is_err());
        assert_eq!(card_goes_with("needs-you", false, false), Ok(()));
    }

    #[test]
    fn a_card_is_kept_with_the_sha_it_was_written_at_and_gone_once_forgotten() {
        let dir = std::env::temp_dir().join(format!("kebbie-cards-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let worktree = dir.join("wt1.app");
        fs::create_dir_all(&worktree).unwrap();
        git(&worktree, &["init", "--quiet"]).unwrap();
        git(&worktree, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--allow-empty", "-m", "first"]).unwrap();
        let head = git(&worktree, &["rev-parse", "HEAD"]).unwrap();
        let state = dir.join("state");
        let source = worktree.join("card.md");
        fs::write(&source, "  ").unwrap();
        assert!(read_card(&source).unwrap_err().contains("is empty"));
        fs::write(&source, "x".repeat(CARD_LIMIT + 1)).unwrap();
        assert!(read_card(&source).unwrap_err().contains("over 4000"));
        fs::write(&source, "\nRefunds show on the billing page.\n\nLook: /billing\n![the refund row](/tmp/refund.png)\n").unwrap();
        let text = read_card(&source).unwrap();
        assert!(card_in(&state, "billing@app").is_none());
        take_card(&worktree, &source, &state, "billing@app", &text, false, None).unwrap();
        assert!(!source.exists(), "a card left in the worktree is an uncommitted file that stops the push");
        fs::create_dir_all(worktree.join("src")).unwrap();
        fs::write(&source, &text).unwrap();
        take_card(&worktree.join("src"), &source, &state, "billing@app", &text, false, None).unwrap();
        assert!(!source.exists(), "taken from anywhere in the worktree, whatever folder the report ran in");
        let outside = dir.join("card.md");
        fs::write(&outside, &text).unwrap();
        take_card(&worktree, &outside, &state, "billing@app", &text, false, None).unwrap();
        assert!(outside.exists(), "a file outside the worktree is the task's own business");
        assert_eq!(git(&worktree, &["status", "--porcelain"]).unwrap(), "");
        let card = card_in(&state, "billing@app").unwrap();
        assert_eq!(card.sha, head);
        assert_eq!(card.text, "Refunds show on the billing page.\n\nLook: /billing\n![the refund row](/tmp/refund.png)");
        assert!(card_in(&state, "billing@web").is_none());
        forget_card_in(&state, "billing@app");
        assert!(card_in(&state, "billing@app").is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_check_after_the_push_goes_with_a_ready_report_and_is_kept_until_the_task_ends() {
        assert_eq!(check_goes_with("ready", Some("  the old key\n gets 401 ")), Ok(Some("the old key gets 401".to_string())));
        assert_eq!(check_goes_with("needs-you", None), Ok(None));
        assert!(check_goes_with("needs-you", Some("the old key gets 401")).unwrap_err().contains("--after-push goes with --state ready"));
        assert!(check_goes_with("ready", Some(" ")).unwrap_err().contains("needs what you verify"));
        assert!(check_goes_with("ready", Some(&"x".repeat(CHECK_LIMIT + 1))).unwrap_err().contains("name the check, not its steps"));
        let state = crate::sh::test_dir("report-check");
        assert_eq!(check_in(&state, "billing@app"), None);
        save_check(&state, "billing@app", "the old key gets 401").unwrap();
        assert_eq!(check_in(&state, "billing@app").as_deref(), Some("the old key gets 401"));
        assert_eq!(check_in(&state, "billing@web"), None);
        forget_check_in(&state, "billing@app");
        assert_eq!(check_in(&state, "billing@app"), None);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn while_the_check_runs_a_report_is_its_outcome() {
        use crate::ci::{Check, Pushed, State, Watch};
        use crate::phase::Phase;
        let state = crate::sh::test_dir("report-checked");
        let me = format!("checked-{}@app", std::process::id());
        let check = Check { what: "the old key gets 401".to_string(), asked: Some(10), passed: false };
        let watch = Watch { address: me.clone(), repo: PathBuf::from("/src/app"), slug: String::new(), sha: "abcdef0".to_string(), started_at: 0, checked_at: None, close: true, runs: Vec::new(), state: State::Running, error: None, erring_since: None, pushed: Pushed { check: Some(check.clone()), ..Pushed::default() } };
        let report = |state: &str, next: &str| Report { state: state.to_string(), next: next.to_string(), head: Some("abc".to_string()), after: Vec::new() };
        let begin = || {
            crate::ci::save(&state, &watch).unwrap();
            crate::phase::record(&state, &me, "ci", &crate::phase::Event::PushOut { sha: "abcdef0" });
        };
        let phase = || crate::phase::load(&state, &me).map(|record| record.phase);
        begin();
        assert!(check_reported(&state, &me, &watch, &report("working", "nothing"), "calling the API", 0).unwrap().is_some());
        assert_eq!(crate::ci::load(&state, &me).and_then(|watch| watch.checking().cloned()), Some(check.clone()), "a turn under way keeps it checking");
        assert_eq!(check_reported(&state, &me, &watch, &report("blocked", "nothing"), "waits for the deploy", 0).unwrap().map(|said| said.contains("waits for the check's outcome")), Some(true));
        assert_eq!(phase(), Some(Phase::CiRunning { sha: "abcdef0".to_string() }), "and under CI running");
        assert!(check_reported(&state, &me, &watch, &report("ready", "nothing"), "401 for the old key", 0).unwrap().unwrap().contains("held"));
        assert_eq!(crate::ci::load(&state, &me).and_then(|watch| watch.pushed.check).map(|check| check.passed), Some(true), "the keeper closes it on its next round");
        begin();
        assert_eq!(check_reported(&state, &me, &watch, &report("needs-you", "the old key still gets 200"), "200 for the old key", 0).unwrap(), None);
        assert!(crate::ci::load(&state, &me).is_none(), "a check that failed is no longer CI running");
        let told = messages::load().into_iter().filter(|message| message.to == crate::status::LEAD_LABEL && message.text.contains(&format!("The check {me} ran after its push did not hold"))).count();
        assert_eq!(told, 1);
        begin();
        assert_eq!(check_reported(&state, &me, &watch, &report("ready", "nothing"), "fixed the key rotation", 1).unwrap(), None, "a fix to push is a ready report as any other");
        assert!(crate::ci::load(&state, &me).is_none());
        assert_eq!(phase(), Some(Phase::Ready { head: Some("abc".to_string()) }));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn a_stop_is_sent_back_once_when_the_turn_has_no_report() {
        assert!(should_block(false, Some(100), None));
        assert!(should_block(false, Some(100), Some(90)));
        assert!(should_block(false, None, None));
        assert!(!should_block(false, Some(100), Some(100)));
        assert!(!should_block(false, Some(100), Some(120)));
        assert!(!should_block(false, None, Some(5)));
        assert!(!should_block(true, Some(100), None));
    }

    #[test]
    fn herdr_shows_the_next_step_and_the_state() {
        let report = |state: &str, next: &str| Report { state: state.to_string(), next: next.to_string(), head: None, after: Vec::new() };
        assert_eq!(metadata_args(&report("needs-you", "answer: which queue?"), crate::agent::Agent::Codex)[..4], ["--source", "kebbie", "--agent", "codex"], "herdr shows it on the pane of the agent that reported");
        assert_eq!(metadata_args(&report("needs-you", "answer: which queue?"), crate::agent::Agent::Claude)[4..], [
            "--token", "next=answer: which queue?", "--state-label", "idle=needs you", "--state-label", "done=needs you",
        ]);
        assert_eq!(metadata_args(&report("working", "nothing"), crate::agent::Agent::Claude)[4..], ["--clear-token", "next", "--clear-state-labels"]);
    }

    #[test]
    fn the_hook_is_quiet_outside_a_task() {
        assert_eq!(hook(r#"{"hook_event_name":"Stop","cwd":"/","stop_hook_active":false}"#), "");
        assert_eq!(hook("not json"), "");
    }

    #[test]
    fn a_ready_report_over_a_red_run_names_its_cause_and_one_over_a_green_run_names_none() {
        let run = |result: crate::evidence::Outcome| crate::evidence::Run { command: "cargo test".to_string(), runner: "cargo test".to_string(), result, why: String::new(), exit: None, duration_ms: None, summary: String::new(), sha: "abcdef0".to_string(), dirty: false, code: String::new(), at: 1, background: String::new(), transcript: String::new(), says: None, worktree: String::new(), read_to: 0, sandboxed: true };
        let red = [run(crate::evidence::Outcome::Passed), run(crate::evidence::Outcome::Failed)];
        let refused = red_gate(&red, None).unwrap_err();
        assert!(refused.contains("`cargo test` failed at abcdef0") && refused.contains("kebbie obstacle") && refused.contains("--red-cause"), "{refused}");
        assert_eq!(red_gate(&red, Some("  tests/vat.rs:12 fails on main\n too ")), Ok(Some("tests/vat.rs:12 fails on main too".to_string())));
        assert!(red_gate(&red, Some(" ")).unwrap_err().contains("needs the cause found"));
        let green = [run(crate::evidence::Outcome::Failed), run(crate::evidence::Outcome::Passed)];
        assert_eq!(red_gate(&green, None), Ok(None), "only the latest run counts");
        assert!(red_gate(&green, Some("x")).unwrap_err().contains("it is not red"));
        assert_eq!(red_gate(&[], None), Ok(None));
        assert_eq!(red_gate(&[run(crate::evidence::Outcome::Unknown)], None), Ok(None), "a result kebbie could not read is not red");
    }
}
