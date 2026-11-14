use std::path::Path;

use crate::lead::state_dir;
use crate::notice::{Notice, Tone};
use crate::peers;
use crate::permit::Caller;
use crate::repo::{default_branch, git, git_writing, main_checkout, name_of};
use crate::sh::Result;
use super::accept::{Open, open_commits};
use super::plan::{Bound, MovedAt, Plan, Subtasks, Task, changed, noted_plan, short};
use super::pool::{Slot, find_task, task_of};

const USAGE: &str = "kebbie order shows the push order; kebbie order move <sha> --before <sha|top> --because \"<why>\" moves a commit; kebbie order group <sha> <sha>… --because \"<why>\" binds commits that go out together; kebbie order ungroup <sha|group> unbinds them. In the task's worktree, or with the task's name first elsewhere";

// What kebbie order was asked besides its words.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Asked<'a> {
    pub before: Option<&'a str>,
    pub because: Option<&'a str>,
    pub as_json: bool,
}

fn target<'a>(repo: &Path, cwd: &Path, words: &'a [String]) -> Result<(Slot, &'a [String])> {
    if let Ok(slot) = find_task(repo, None, cwd) {
        return Ok((slot, words));
    }
    let name = words.first().filter(|word| !["move", "group", "ungroup"].contains(&word.as_str())).ok_or_else(|| format!("Not inside a task's worktree; name the task: {USAGE}"))?;
    Ok((find_task(repo, Some(name), cwd)?, &words[1..]))
}

// A task reorders its own commits only; the lead and the user reorder any task's.
fn reorders(caller: &Caller, address: &str) -> Result<()> {
    super::plan::writes_to(caller, address, "orders its own commits")
}

pub fn command(cwd: &Path, words: &[String], asked: &Asked) -> Result<String> {
    let repo = main_checkout(cwd)?;
    if !crate::config::resolve(Some(&repo))?.settings.steps.value {
        return Err(format!("kebbie order is off for {}: it orders a task's subtasks' commits, so it needs task.steps = true", name_of(&repo)));
    }
    let (slot, words) = target(&repo, cwd, words)?;
    let address = peers::address(task_of(&slot), &name_of(&repo));
    let upstream = default_branch(&repo)?;
    let task = Task { path: &slot.path, upstream: &upstream, address: &address };
    let state = state_dir()?;
    let caller = crate::permit::caller();
    let reason = || asked.because.map(one_line).filter(|reason| !reason.is_empty()).ok_or_else(|| format!("kebbie order {} needs --because \"<why>\": {USAGE}", words[0]));
    let said = match words.first().map(String::as_str) {
        None => None,
        Some("move") if words.len() == 2 => {
            reorders(&caller, &address)?;
            let before = asked.before.ok_or_else(|| format!("kebbie order move needs --before <sha|top>: {USAGE}"))?;
            Some(move_as(&state, &task, &words[1], before, &reason()?)?)
        }
        Some("group") if words.len() >= 3 => {
            reorders(&caller, &address)?;
            Some(group_as(&state, &task, &words[1..], &reason()?)?)
        }
        Some("ungroup") if words.len() == 2 => {
            reorders(&caller, &address)?;
            Some(ungroup_as(&state, &task, &words[1])?)
        }
        Some(_) => return Err(format!("kebbie order does not know that: {USAGE}")),
    };
    let subtasks = super::plan::read_now(&state, &slot.path, &upstream, &address);
    if asked.as_json {
        let shown = serde_json::json!({ "task": address, "order": super::plan::order_json(&subtasks), "pushable": super::plan::pushable_json(&subtasks) });
        return serde_json::to_string_pretty(&shown).map_err(|error| error.to_string());
    }
    let shown = shown(&address, &subtasks);
    Ok(said.map_or(shown.clone(), |said| format!("{said}\n\n{shown}")))
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(kebbie_hub_protocol::MAX_MOVED_REASON_CHARS).collect()
}

// The push order as a list, top first, with each commit's subtask, group, tests and where it was moved from.
pub fn shown(address: &str, subtasks: &Subtasks) -> String {
    if subtasks.order.is_empty() {
        return format!("{address} has no commits to push.");
    }
    let through = subtasks.pushable.as_ref().map(|run| run.count);
    let mut lines = vec![format!("{address}'s push order, top first:")];
    for (at, row) in subtasks.order.iter().enumerate() {
        let group = row.group.as_ref().map(|group| format!("  [{group}]")).unwrap_or_default();
        let tests = row.tests.as_ref().map(|tests| format!("  tests {}", super::plan::tests_word(tests.state))).unwrap_or_default();
        let moved = row.moved.as_ref().map(|moved| format!("  moved from {}: {}", moved.from, moved.reason)).unwrap_or_default();
        let mark = if row.accepted { "✓" } else { " " };
        lines.push(format!("  {}. {mark} {} {}  ({}){group}{tests}{moved}", at + 1, short(&row.sha), row.subject, row.subtask));
        if through == Some(at + 1) {
            lines.push("     ── pushable through here".to_string());
        }
    }
    lines.join("\n")
}

fn resolve<'a>(open: &'a [Open], said: &str) -> Result<&'a Open> {
    let found: Vec<&Open> = open.iter().filter(|commit| said.len() >= 4 && commit.sha.starts_with(said)).collect();
    match found.as_slice() {
        [one] => Ok(one),
        [] => Notice::new(Tone::Failed, format!("{said} is not one of the task's commits to push")).next("kebbie order lists them.").err(),
        _ => Notice::new(Tone::Failed, format!("{said} names more than one commit")).next("Give more of its sha.").err(),
    }
}

// The commits that move as one with sha: its group's, else itself.
fn unit(plan: &Plan, open: &[Open], sha: &str) -> Vec<String> {
    let code = open.iter().find(|commit| commit.sha == sha).map(|commit| commit.code.as_str()).unwrap_or_default();
    match plan.groups.iter().find(|group| group.codes.iter().any(|one| one == code)) {
        Some(group) => open.iter().filter(|commit| group.codes.contains(&commit.code)).map(|commit| commit.sha.clone()).collect(),
        None => vec![sha.to_string()],
    }
}

// The order with `moving` lifted out and put right before `before`, or at the top with None.
pub(super) fn reordered(order: &[String], moving: &[String], before: Option<&str>) -> Vec<String> {
    let mut rest: Vec<String> = order.iter().filter(|sha| !moving.contains(sha)).cloned().collect();
    let at = before.and_then(|before| rest.iter().position(|sha| sha == before)).unwrap_or(0);
    let block: Vec<String> = order.iter().filter(|sha| moving.contains(sha)).cloned().collect();
    rest.splice(at..at, block);
    rest
}

// Refused while a push of the task waits, since what it pushes is frozen, while files are uncommitted, or during a rebase.
fn may_rewrite(state: &Path, task: &Task) -> Result<()> {
    if let Some(intent) = super::intent::load(state, task.address) {
        let who = match intent.by { super::intent::By::Lead => "the lead's", _ => "your" };
        return Notice::new(Tone::NotNow, format!("Not moved: {who} push of {} waits in line, and the commits it pushes stay as they are until it goes out", task.address))
            .next(format!("Move it once the push is out, or cancel the push first with kebbie push {} --cancel.", task_word(task.address))).detail("task", task.address).err();
    }
    super::push::committed(task.path, task.address).map_err(|error| error.replacen("It has", "Not moved: it has", 1))
}

fn task_word(address: &str) -> &str {
    address.split_once('@').map_or(address, |(task, _)| task)
}

// What a replay that met a conflict tells the asker: the commit that did not apply and the files it conflicts on.
pub(super) struct Conflict {
    pub commit: String,
    pub files: Vec<String>,
    pub error: String,
}

// The branch replayed in the new order of its open commits, by cherry-picks from the first place it changes; a conflict puts the branch back exactly as it was.
pub(super) fn replay(path: &Path, upstream: &str, open_now: &[String], wanted: &[String]) -> Result<std::result::Result<(), Conflict>> {
    let before = git(path, &["rev-parse", "HEAD"])?;
    let all: Vec<String> = git(path, &["rev-list", "--reverse", &format!("{}..HEAD", crate::repo::tracking(upstream))])?.lines().map(str::to_string).collect();
    if !git(path, &["rev-list", "--merges", &format!("{}..HEAD", crate::repo::tracking(upstream))])?.is_empty() {
        return Err("Not moved: the branch has a merge on it, which kebbie does not replay".to_string());
    }
    let mut next = wanted.iter();
    let new: Vec<String> = all.iter().map(|sha| if open_now.contains(sha) { next.next().cloned().unwrap_or_else(|| sha.clone()) } else { sha.clone() }).collect();
    let Some(first) = all.iter().zip(&new).position(|(old, new)| old != new) else { return Ok(Ok(())) };
    let base = git(path, &["rev-parse", &format!("{}^", all[first])])?;
    git_writing(path, &["reset", "--quiet", "--keep", &base])?;
    for sha in &new[first..] {
        if let Err(error) = git_writing(path, &["cherry-pick", "--allow-empty", "--keep-redundant-commits", sha]) {
            let files: Vec<String> = git(path, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default().lines().map(str::to_string).collect();
            let _ = git_writing(path, &["cherry-pick", "--abort"]);
            if let Err(error) = git_writing(path, &["reset", "--quiet", "--keep", &before]) {
                return Notice::new(Tone::Failed, format!("The branch is left part-way: kebbie could not put it back to {}", short(&before)))
                    .you(format!("Run git cherry-pick --abort, then git reset --keep {} in {}.", short(&before), path.display())).detail("error", error).err();
            }
            return Ok(Err(Conflict { commit: sha.clone(), files, error }));
        }
    }
    Ok(Ok(()))
}

fn conflict_notice(conflict: &Conflict, subject: &str) -> String {
    if conflict.files.is_empty() {
        return Notice::new(Tone::Failed, format!("Not moved: git did not replay {} {subject}", short(&conflict.commit))).next("The branch is as it was.").detail("error", &conflict.error).to_string();
    }
    let files = conflict.files.join(" · ");
    Notice::new(Tone::NotNow, format!("Not moved: {} {subject} does not apply in the new order", short(&conflict.commit)))
        .next("The branch is as it was. A commit that prepares another applies before it only when it was built from the place it moves to: build it again from there, or leave the order as it is.")
        .detail("conflicts", files).to_string()
}

// What a change of the order asks: the commits in their new order, those it moves, why, what to say, and the groups once it binds new ones.
struct Rewrite {
    wanted: Vec<String>,
    moved: Vec<String>,
    reason: String,
    said: String,
    groups: Option<Vec<Bound>>,
}

// One rewrite of the order under the plan's lock and the worktree's rebase lock; the plan comes back with what the change records.
fn rewritten(state: &Path, task: &Task, change: impl FnOnce(&Plan, &[Open]) -> Result<Rewrite>) -> Result<String> {
    changed(state, task.address, |_| {
        let (_, plan) = noted_plan(state, task)?;
        let open = open_commits(task.path, task.upstream)?;
        let Rewrite { wanted, moved, reason, said, groups } = change(&plan, &open)?;
        let now: Vec<String> = open.iter().map(|commit| commit.sha.clone()).collect();
        if wanted != now {
            may_rewrite(state, task)?;
            let _alone = super::push::rebase_alone(task.path)?;
            if let Err(conflict) = replay(task.path, task.upstream, &now, &wanted)? {
                let subject = git(task.path, &["log", "-1", "--format=%s", &conflict.commit]).unwrap_or_default();
                return Err(conflict_notice(&conflict, &subject));
            }
        }
        let (_, mut noted) = noted_plan(state, task)?;
        noted.groups = groups.unwrap_or(plan.groups);
        for sha in &moved {
            let Some(at) = now.iter().position(|one| one == sha) else { continue };
            if wanted.iter().position(|one| one == sha) == Some(at) {
                continue;
            }
            let code = open[at].code.clone();
            noted.moved.retain(|one| one.code != code);
            noted.moved.push(MovedAt { code, from: at as u32 + 1, reason: reason.clone() });
        }
        Ok((Some(noted), said))
    })
}

pub(super) fn move_as(state: &Path, task: &Task, sha: &str, before: &str, reason: &str) -> Result<String> {
    rewritten(state, task, |plan, open| {
        let commit = resolve(open, sha)?;
        let moving = unit(plan, open, &commit.sha);
        let order: Vec<String> = open.iter().map(|commit| commit.sha.clone()).collect();
        let target = match before {
            "top" => None,
            said => {
                let target = resolve(open, said)?;
                if moving.contains(&target.sha) {
                    return Err(format!("Not moved: {} goes with {} already", short(&target.sha), short(&commit.sha)));
                }
                unit(plan, open, &target.sha).into_iter().next()
            }
        };
        let wanted = reordered(&order, &moving, target.as_deref());
        let place = target.as_deref().map_or("to the top".to_string(), |target| format!("before {}", short(target)));
        let what = if moving.len() > 1 { format!("{} with its group", short(&commit.sha)) } else { short(&commit.sha).to_string() };
        let said = if wanted == order { format!("{what} is there already.") } else { format!("Moved {what} {place}; its subtask and acceptance go with it.") };
        Ok(Rewrite { wanted, moved: moving, reason: reason.to_string(), said, groups: None })
    })
}

fn group_as(state: &Path, task: &Task, said: &[String], reason: &str) -> Result<String> {
    rewritten(state, task, |plan, open| {
        let mut members: Vec<&Open> = Vec::new();
        for one in said {
            let commit = resolve(open, one)?;
            if !members.iter().any(|member| member.sha == commit.sha) {
                members.push(commit);
            }
        }
        if members.len() < 2 {
            return Err(format!("A group holds two commits or more: {USAGE}"));
        }
        if let Some(bound) = members.iter().find_map(|member| plan.groups.iter().find(|group| group.codes.contains(&member.code))) {
            return Err(format!("Not grouped: a commit is in the group {} already; kebbie order ungroup {} first", bound.id, bound.id));
        }
        let order: Vec<String> = open.iter().map(|commit| commit.sha.clone()).collect();
        let mut shas: Vec<String> = members.iter().map(|member| member.sha.clone()).collect();
        shas.sort_by_key(|sha| order.iter().position(|one| one == sha));
        let rest: Vec<String> = shas[1..].to_vec();
        let mut wanted: Vec<String> = order.iter().filter(|one| !rest.contains(one)).cloned().collect();
        let first_at = wanted.iter().position(|one| *one == shas[0]).unwrap_or(0);
        wanted.splice(first_at + 1..first_at + 1, rest.clone());
        let id = (1..).map(|number| format!("g{number}")).find(|id| !plan.groups.iter().any(|group| group.id == *id)).unwrap_or_default();
        let codes: Vec<String> = shas.iter().filter_map(|sha| open.iter().find(|commit| commit.sha == *sha)).map(|commit| commit.code.clone()).collect();
        let mut groups = plan.groups.clone();
        groups.push(Bound { id: id.clone(), codes, reason: reason.to_string() });
        let together = if wanted == order { "they sit together already" } else { "kebbie moved them together" };
        let said = format!("Grouped {} as {id}: {together}, and a push takes all of them or none.", shas.iter().map(|sha| short(sha)).collect::<Vec<_>>().join(" "));
        Ok(Rewrite { wanted, moved: rest, reason: reason.to_string(), said, groups: Some(groups) })
    })
}

fn ungroup_as(state: &Path, task: &Task, said: &str) -> Result<String> {
    changed(state, task.address, |_| {
        let (_, mut plan) = noted_plan(state, task)?;
        let open = open_commits(task.path, task.upstream)?;
        let code = if said.len() >= 4 { open.iter().find(|commit| commit.sha.starts_with(said)).map(|commit| commit.code.clone()) } else { None };
        let Some(at) = plan.groups.iter().position(|group| group.id == said || code.as_ref().is_some_and(|code| group.codes.contains(code))) else {
            return Err(format!("{said} is no group of {} and no commit in one", task.address));
        };
        let gone = plan.groups.remove(at);
        Ok((Some(plan), format!("Ungrouped {}: its commits go out apart again.", gone.id)))
    })
}

#[cfg(test)]
#[path = "order_tests.rs"]
mod tests;
