use std::path::Path;

use kebbie_hub_protocol::{MAX_STAGE_DETAIL_CHARS, MAX_STAGE_NOTE_CHARS, MAX_STAGE_TITLE_CHARS, MAX_STAGES, Requirement, RequirementKind, Stage, StageState, unlock_stages};
use serde_json::{Value, json};

use crate::lead::state_dir;
use crate::messages;
use crate::peers;
use crate::permit::{Caller, Permit};
use crate::repo::{default_branch, main_checkout, name_of};
use crate::sh::Result;
use super::plan::{self, Placed, Plan, Subtasks, Task, changed};
use super::pool::task_of;

// A stage as the plan file holds it: the subtasks it holds, and once kebbie pushed any of it, the commit that push left on the default branch and where its requirement stands.
#[derive(Clone, Debug, PartialEq)]
pub struct StagePlan {
    pub id: String,
    pub title: String,
    pub kind: RequirementKind,
    pub detail: Option<String>,
    pub subtasks: Vec<String>,
    pub through: Option<String>,
    pub pushed_at: Option<u64>,
    pub kept: Option<StageState>,
    pub met_at: Option<u64>,
    pub note: Option<String>,
}

impl StagePlan {
    fn new(id: String, title: String, kind: RequirementKind, detail: Option<String>) -> StagePlan {
        StagePlan { id, title, kind, detail, subtasks: Vec::new(), through: None, pushed_at: None, kept: None, met_at: None, note: None }
    }

    fn pushed(&self) -> bool {
        self.through.is_some()
    }
}

pub fn kind_word(kind: RequirementKind) -> &'static str {
    match kind {
        RequirementKind::Deploy => "deploy",
        RequirementKind::Check => "check",
        RequirementKind::Verified => "verified",
    }
}

fn kind_of(word: &str) -> Option<RequirementKind> {
    [RequirementKind::Deploy, RequirementKind::Check, RequirementKind::Verified].into_iter().find(|kind| kind_word(*kind) == word)
}

pub fn state_word(state: StageState) -> &'static str {
    match state {
        StageState::Locked => "locked",
        StageState::Open => "open",
        StageState::Pushed => "pushed",
        StageState::Waiting => "waiting",
        StageState::Met => "met",
        StageState::Failed => "failed",
    }
}

fn state_named(word: &str) -> Option<StageState> {
    [StageState::Pushed, StageState::Waiting, StageState::Met, StageState::Failed].into_iter().find(|state| state_word(*state) == word)
}

pub(super) fn read(value: &Value) -> Vec<StagePlan> {
    let text = |one: &Value, key: &str| one[key].as_str().filter(|text| !text.is_empty()).map(str::to_string);
    value.as_array().map(|stages| stages.iter().filter_map(|one| {
        let kind = kind_of(one["requires"]["kind"].as_str()?)?;
        let through = text(one, "through");
        Some(StagePlan {
            id: text(one, "id")?, title: text(one, "title")?, kind, detail: text(&one["requires"], "detail"),
            subtasks: one["subtasks"].as_array().map(|ids| ids.iter().filter_map(|id| id.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            pushed_at: through.as_ref().map(|_| one["pushed_at"].as_u64().unwrap_or_default()), kept: through.as_ref().and_then(|_| one["state"].as_str().and_then(state_named)),
            through, met_at: one["met_at"].as_u64(), note: text(one, "note"),
        })
    }).collect()).unwrap_or_default()
}

pub(super) fn written(stages: &[StagePlan]) -> Value {
    stages.iter().map(|stage| json!({
        "id": stage.id, "title": stage.title, "requires": { "kind": kind_word(stage.kind), "detail": stage.detail }, "subtasks": stage.subtasks,
        "through": stage.through, "pushed_at": stage.pushed_at, "state": stage.kept.map(state_word), "met_at": stage.met_at, "note": stage.note,
    })).collect()
}

// The stages as the board and the phone show them: a pushed one by where its requirement stands, the rest locked or open by whether every stage before is met.
pub fn shown(plans: &[StagePlan]) -> Vec<Stage> {
    let mut stages: Vec<Stage> = plans.iter().map(|plan| {
        let state = if plan.pushed() { plan.kept.unwrap_or(StageState::Pushed) } else { StageState::Locked };
        Stage {
            id: plan.id.clone(), title: plan.title.clone(), state, requirement: Requirement { kind: plan.kind, detail: plan.detail.clone().filter(|_| plan.kind != RequirementKind::Deploy) },
            through: plan.through.clone(), pushed_at: plan.through.as_ref().map(|_| plan.pushed_at.unwrap_or_default()),
            met_at: (state == StageState::Met).then(|| plan.met_at.or(plan.pushed_at).unwrap_or_default()), note: plan.note.clone(),
        }
    }).collect();
    unlock_stages(&mut stages);
    stages
}

// The stage a subtask's commits go out in: the one it was assigned to, else the last, so work no stage names never passes a boundary.
pub fn stage_for(plans: &[StagePlan], subtask: &str) -> Option<String> {
    plans.iter().find(|stage| stage.subtasks.iter().any(|id| id == subtask)).or(plans.last()).map(|stage| stage.id.clone())
}

// Names each open commit's stage, and returns the first commit that sits after a later stage's commits, where the pushable run must stop.
pub(super) fn placed(order: &mut [Placed], plans: &[StagePlan]) -> Option<usize> {
    if plans.is_empty() {
        return None;
    }
    let place = |id: &str| plans.iter().position(|stage| stage.id == id).unwrap_or(0);
    let mut last = 0;
    let mut out = None;
    for (at, row) in order.iter_mut().enumerate() {
        let id = stage_for(plans, &row.subtask);
        let here = id.as_deref().map_or(0, place);
        if here < last && out.is_none() {
            out = Some(at);
        }
        last = last.max(here);
        row.stage = id;
    }
    out
}

// The first stage before the one named that is not met: what a commit of it waits for.
pub fn held_by<'a>(stages: &'a [Stage], stage: &str) -> Option<&'a Stage> {
    stages.iter().take_while(|one| one.id != stage).find(|one| one.state != StageState::Met)
}

pub fn requirement_line(requirement: &Requirement) -> String {
    match (requirement.kind, requirement.detail.as_deref()) {
        (RequirementKind::Deploy, _) => "its deploy is green".to_string(),
        (RequirementKind::Check, Some(detail)) => format!("the task's check holds: {detail}"),
        (RequirementKind::Verified, Some(detail)) => format!("you verify: {detail}"),
        (_, None) => kind_word(requirement.kind).to_string(),
    }
}

fn state_line(stage: &Stage, stages: &[Stage]) -> String {
    match stage.state {
        StageState::Locked => held_by(stages, &stage.id).map_or("locked".to_string(), |before| format!("locked until {} is met", before.title)),
        StageState::Open => "open: its subtasks can be pushed".to_string(),
        StageState::Pushed => format!("pushed · waits until {}", requirement_line(&stage.requirement)),
        StageState::Waiting => match stage.requirement.kind {
            RequirementKind::Verified => "deployed · waits on your verification".to_string(),
            RequirementKind::Check => "deployed · the task runs its check".to_string(),
            RequirementKind::Deploy => "pushed · waits on your word that it deployed".to_string(),
        },
        StageState::Met => "met".to_string(),
        StageState::Failed => format!("failed{}", stage.note.as_deref().map(|note| format!(": {note}")).unwrap_or_default()),
    }
}

// A stage's state in a word or two for the board's row.
pub fn stage_word(stage: &Stage, stages: &[Stage]) -> String {
    match stage.state {
        StageState::Locked => held_by(stages, &stage.id).map_or("locked".to_string(), |before| format!("after {}", before.title)),
        StageState::Open => "open".to_string(),
        StageState::Pushed => format!("pushed · {}", kind_word(stage.requirement.kind)),
        StageState::Waiting if stage.requirement.kind == RequirementKind::Check => "task checks".to_string(),
        StageState::Waiting => "verify".to_string(),
        StageState::Met => "met".to_string(),
        StageState::Failed => "failed".to_string(),
    }
}

pub fn mark(state: StageState) -> &'static str {
    match state {
        StageState::Locked => "·",
        StageState::Open => "○",
        StageState::Pushed | StageState::Waiting => "↑",
        StageState::Met => "✓",
        StageState::Failed => "✗",
    }
}

// One line for the task's row: the stage that waits on something, by what it waits on.
pub fn note(stages: &[Stage]) -> Option<String> {
    let count = stages.len();
    let (at, stage) = stages.iter().enumerate().find(|(_, stage)| matches!(stage.state, StageState::Pushed | StageState::Waiting | StageState::Failed))?;
    Some(format!("stage {} of {count} {}", at + 1, state_line(stage, stages)))
}

const USAGE: &str = "kebbie stages \"<title>:deploy\" \"<title>:check:<what the task checks>\" \"<title>:verified:<what you verify>\" … declares the task's stages in order; kebbie stages assign <stage> <subtask> … puts subtasks in a stage; kebbie stages [--json] shows them. In the task's worktree, or with the task's name first elsewhere";

// "<title>:deploy", "<title>:check:<what>" or "<title>:verified:<what>", the what in quotes or not.
pub fn spec(text: &str) -> Result<(String, RequirementKind, Option<String>)> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let found = [(":check:", RequirementKind::Check), (":verified:", RequirementKind::Verified)].into_iter().filter_map(|(mark, kind)| text.find(mark).map(|at| (at, mark.len(), kind))).min_by_key(|(at, _, _)| *at);
    let (title, kind, detail) = match found {
        Some((at, len, kind)) => (text[..at].trim(), kind, Some(text[at + len..].trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string())),
        None => match text.strip_suffix(":deploy") {
            Some(title) => (title.trim(), RequirementKind::Deploy, None),
            None => return Err(format!("\"{text}\" names no requirement: end it with :deploy, :check:<what the task checks> or :verified:<what you verify>. {USAGE}")),
        },
    };
    if title.is_empty() || title.chars().count() > MAX_STAGE_TITLE_CHARS {
        return Err(format!("a stage needs a title of 1 to {MAX_STAGE_TITLE_CHARS} characters before its requirement: {USAGE}"));
    }
    if detail.as_deref().is_some_and(|detail| detail.is_empty() || detail.chars().count() > MAX_STAGE_DETAIL_CHARS) {
        return Err(format!("{}: say in 1 to {MAX_STAGE_DETAIL_CHARS} characters what holds once the stage is out", title));
    }
    Ok((title.to_string(), kind, detail))
}

pub fn command(cwd: &Path, words: &[String], as_json: bool) -> Result<String> {
    let repo = main_checkout(cwd)?;
    if !crate::config::resolve(Some(&repo))?.settings.steps.value {
        return Err(format!("kebbie stages goes with subtasks, and task.steps is off for {}: set task.steps = true in {} or {}.", name_of(&repo), crate::config::path().display(), crate::config::repo_file(&repo).display()));
    }
    let (slot, words) = plan::target(&repo, cwd, words)?;
    let address = peers::address(task_of(&slot), &name_of(&repo));
    let upstream = default_branch(&repo)?;
    let task = Task { path: &slot.path, upstream: &upstream, address: &address };
    let state = state_dir()?;
    let caller = crate::permit::caller();
    let said = match words.first().map(String::as_str) {
        None => None,
        Some("assign") => Some(assign_as(&caller, &state, &task, words.get(1).map(String::as_str).unwrap_or_default(), &words[2.min(words.len())..])?),
        Some(_) => Some(declare_as(&caller, &state, &task, &repo, words)?),
    };
    let subtasks = plan::read_now(&state, &slot.path, &upstream, &address);
    let plans = plan::load(&state, &address).map(|plan| plan.stages).unwrap_or_default();
    if as_json {
        return serde_json::to_string_pretty(&to_json(&address, &subtasks, &plans)).map_err(|error| error.to_string());
    }
    let shown = shown_text(&address, &subtasks, &plans);
    Ok(said.map_or(shown.clone(), |said| format!("{said}\n\n{shown}")))
}

fn declare_as(caller: &Caller, state: &Path, task: &Task, repo: &Path, words: &[String]) -> Result<String> {
    plan::writes(caller, task)?;
    let specs = words.iter().map(|word| spec(word)).collect::<Result<Vec<_>>>()?;
    if specs.len() > MAX_STAGES {
        return Err(format!("a task holds at most {MAX_STAGES} stages"));
    }
    if specs.iter().any(|(_, kind, _)| *kind == RequirementKind::Deploy) && !crate::config::resolve(Some(repo))?.settings.ci_watch.value {
        return Err(format!("a deploy requirement needs kebbie to watch CI, and ci.watch is off for {}: use :check:<what the task checks> or :verified:<what you verify> instead", name_of(repo)));
    }
    changed(state, task.address, |old| {
        let plan = old.filter(|plan| !plan.parts.is_empty()).ok_or_else(|| format!("{} has no subtasks yet: list them with kebbie subtasks first, then its stages", task.address))?;
        let mut stages: Vec<StagePlan> = Vec::new();
        for (title, kind, detail) in specs {
            let stage = match plan.stages.iter().find(|stage| stage.title == title) {
                Some(kept) if kept.pushed() && (kept.kind != kind || kept.detail != detail) => return Err(format!("{title} is pushed already, so its requirement stays as it is")),
                Some(kept) => StagePlan { kind, detail, ..kept.clone() },
                None => {
                    let taken: Vec<&str> = plan.stages.iter().chain(&stages).map(|stage| stage.id.as_str()).collect();
                    StagePlan::new(plan::new_id(&title, &taken), title, kind, detail)
                }
            };
            if stages.iter().any(|one| one.title == stage.title) {
                return Err(format!("{} is listed twice", stage.title));
            }
            stages.push(stage);
        }
        if let Some(gone) = plan.stages.iter().find(|stage| stage.pushed() && !stages.iter().any(|one| one.id == stage.id)) {
            return Err(format!("{} is pushed already, so it stays in the list", gone.title));
        }
        if stages.windows(2).any(|pair| !pair[0].pushed() && pair[1].pushed()) {
            return Err("the stages pushed already come first, in the order they went out".to_string());
        }
        let count = stages.len();
        Ok((Some(Plan { stages, ..plan }), format!("{} has {} now; a subtask no stage names goes out with the last.", task.address, if count == 1 { "1 stage".to_string() } else { format!("{count} stages") })))
    })
}

fn find_stage<'a>(stages: &'a [StagePlan], said: &str) -> Option<&'a StagePlan> {
    stages.iter().find(|stage| stage.id == said || stage.title == said).or_else(|| said.parse::<usize>().ok().and_then(|number| stages.get(number.checked_sub(1)?)))
}

fn assign_as(caller: &Caller, state: &Path, task: &Task, said: &str, subtasks: &[String]) -> Result<String> {
    plan::writes(caller, task)?;
    if said.is_empty() || subtasks.is_empty() {
        return Err(format!("kebbie stages assign needs a stage and one subtask or more: {USAGE}"));
    }
    changed(state, task.address, |_| {
        let (now, mut plan) = plan::noted_plan(state, task)?;
        let stage = find_stage(&plan.stages, said).cloned().ok_or_else(|| format!("{} has no stage {said}: kebbie stages lists them", task.address))?;
        let mut moved = Vec::new();
        for word in subtasks {
            let subtask = now.find(word).ok_or_else(|| format!("{} has no subtask {word}: kebbie subtasks lists them", task.address))?;
            if subtask.state == super::plan::State::Pushed || !subtask.pushed.is_empty() {
                return Err(format!("{} is pushed already, so it stays in its stage", subtask.title));
            }
            for one in plan.stages.iter_mut() {
                one.subtasks.retain(|id| *id != subtask.id);
            }
            if let Some(one) = plan.stages.iter_mut().find(|one| one.id == stage.id) {
                one.subtasks.push(subtask.id.clone());
            }
            moved.push(subtask.title.clone());
        }
        Ok((Some(plan), format!("{} go out in {} [{}].", moved.join(", "), stage.title, stage.id)))
    })
}

// A new subtask added with --stage goes out in that stage.
pub(super) fn put_in(plan: &mut Plan, subtask: &str, said: &str) -> Result<()> {
    let id = find_stage(&plan.stages, said).map(|stage| stage.id.clone()).ok_or_else(|| format!("no stage {said}: kebbie stages lists them"))?;
    for stage in plan.stages.iter_mut() {
        stage.subtasks.retain(|one| one != subtask);
        if stage.id == id {
            stage.subtasks.push(subtask.to_string());
        }
    }
    Ok(())
}

fn shown_text(address: &str, subtasks: &Subtasks, plans: &[StagePlan]) -> String {
    if subtasks.stages.is_empty() {
        return format!("{address} has no stages: kebbie stages \"<title>:<requirement>\" … declares them, when the change must go out in steps with a requirement between them (recipe rollout-in-stages).");
    }
    let mut lines = vec![format!("{address}'s stages:")];
    for (at, stage) in subtasks.stages.iter().enumerate() {
        lines.push(format!("  {} {}. {} [{}]  {}", mark(stage.state), at + 1, stage.title, stage.id, state_line(stage, &subtasks.stages)));
        lines.push(format!("       requires: {}", requirement_line(&stage.requirement)));
        let held: Vec<String> = subtasks.items.iter().chain(&subtasks.pushed).filter(|subtask| subtasks.order.iter().any(|row| row.subtask == subtask.id && row.stage.as_deref() == Some(stage.id.as_str())) || plans.get(at).is_some_and(|plan| plan.subtasks.contains(&subtask.id))).map(|subtask| subtask.id.clone()).collect();
        if !held.is_empty() {
            lines.push(format!("       subtasks: {}", held.join(", ")));
        }
    }
    if let Some(sha) = &subtasks.out_of_stage {
        lines.push(format!("Out of stage order: {} sits after a later stage's commits; move it up with kebbie order move {} --before <commit> --because \"<why>\". Nothing after it is pushable until then.", plan::short(sha), plan::short(sha)));
    }
    lines.join("\n")
}

pub fn stages_json(stages: &[Stage]) -> Value {
    stages.iter().map(|stage| json!({
        "id": stage.id, "title": stage.title, "state": state_word(stage.state), "requirement": { "kind": kind_word(stage.requirement.kind), "detail": stage.requirement.detail },
        "through": stage.through, "pushed_at": stage.pushed_at, "met_at": stage.met_at, "note": stage.note,
    })).collect()
}

fn to_json(address: &str, subtasks: &Subtasks, plans: &[StagePlan]) -> Value {
    let mut stages = stages_json(&subtasks.stages);
    for (stage, plan) in stages.as_array_mut().into_iter().flatten().zip(plans) {
        stage["subtasks"] = json!(plan.subtasks);
    }
    json!({ "task": address, "stages": stages, "out_of_stage": subtasks.out_of_stage })
}

// With stages, a push takes only commits whose stage is open; the lead pushes none for a repo permission, and none past a stage that waits on the user's verification.
#[allow(clippy::too_many_arguments)]
pub(super) fn refuse_past_stage(caller: &Caller, state: &Path, address: &str, task: &str, path: &Path, upstream: &str, take: Option<usize>, refused: &str) -> Result<()> {
    let subtasks = plan::read_now(state, path, upstream, address);
    if subtasks.stages.is_empty() {
        return Ok(());
    }
    let take = take.unwrap_or(subtasks.order.len()).min(subtasks.order.len());
    let rows = &subtasks.order[..take];
    let notice = |what: String| crate::notice::Notice::new(crate::notice::Tone::NotNow, format!("{refused}: {what}")).detail("task", task);
    if let Some(at) = subtasks.out_of_stage.as_ref().and_then(|out| subtasks.order.iter().position(|row| row.sha == *out)).filter(|at| *at < take) {
        return notice(format!("commit {} sits after a later stage's commits", plan::short(&subtasks.order[at].sha))).next("Its task moves it up with kebbie order move, or push only the commits above it.").err();
    }
    for row in rows {
        let Some(stage) = row.stage.as_deref() else { continue };
        if let Some(before) = held_by(&subtasks.stages, stage) {
            let title = subtasks.stages.iter().find(|one| one.id == stage).map_or(stage, |one| one.title.as_str());
            return notice(format!("{title} waits until {} is met", before.title)).next(format!("{} is met once {}; push only the commits above {title} meanwhile.", before.title, requirement_line(&before.requirement))).err();
        }
    }
    if matches!(caller, Caller::Lead | Caller::LeadSandboxed) {
        if crate::permit::of(address, address.split_once('@').map_or("", |(_, repo)| repo)) == Some(Permit::Repo) {
            return notice("a repo's push permission covers no stages".to_string()).next("The user pushes each stage, or gives this task its own permission.").err();
        }
        let after_verified = rows.iter().filter_map(|row| row.stage.as_deref()).any(|stage| {
            let at = subtasks.stages.iter().position(|one| one.id == stage).unwrap_or(0);
            at > 0 && subtasks.stages[at - 1].requirement.kind == RequirementKind::Verified
        });
        if after_verified {
            return notice("a stage after the user's verification is the user's to push".to_string()).next("The user pushes it from the board or the phone.").err();
        }
    }
    Ok(())
}

// The stages the first `count` open commits belong to, in order: what a push of them puts out.
pub(super) fn touched(state: &Path, path: &Path, upstream: &str, address: &str, count: usize) -> Vec<StagePlan> {
    let subtasks = plan::read_now(state, path, upstream, address);
    let plans = plan::load(state, address).map(|plan| plan.stages).unwrap_or_default();
    let mut ids: Vec<String> = Vec::new();
    for row in subtasks.order.iter().take(count) {
        if let Some(stage) = &row.stage && !ids.contains(stage) {
            ids.push(stage.clone());
        }
    }
    plans.into_iter().filter(|stage| ids.contains(&stage.id)).collect()
}

fn each(state: &Path, address: &str, ids: &[String], mut change: impl FnMut(&mut StagePlan)) -> Result<Vec<Stage>> {
    changed(state, address, |plan| {
        let Some(mut plan) = plan else { return Ok((None, Vec::new())) };
        for stage in plan.stages.iter_mut().filter(|stage| ids.contains(&stage.id)) {
            change(stage);
        }
        let shown = shown(&plan.stages);
        Ok((Some(plan), shown))
    })
}

// A push of a stage's commits: its requirement starts over on the commit the push left, whatever it was before.
pub(super) fn mark_pushed(state: &Path, address: &str, ids: &[String], sha: &str, now: u64) {
    let pushed = each(state, address, ids, |stage| {
        stage.through = Some(sha.to_string());
        stage.pushed_at = Some(now);
        stage.kept = Some(StageState::Pushed);
        stage.met_at = None;
        stage.note = None;
    });
    if let Err(error) = pushed {
        eprintln!("kebbie: the stages of {address} may not show this push: {error}");
    }
    crate::kebbie_questions::settle_stages(state, address, ids);
}

// What a push of stages carries for the CI watch: the stages, the task's check when one of them is a check, and the deploy workflow every requirement waits for.
pub(super) fn for_watch(repo: &Path, stages: &[StagePlan]) -> (Vec<String>, Option<String>, Option<String>) {
    let checks: Vec<&str> = stages.iter().filter(|stage| stage.kind == RequirementKind::Check).filter_map(|stage| stage.detail.as_deref()).collect();
    let deploy = crate::config::resolve(Some(repo)).ok().and_then(|config| config.settings.ci_deploy.value).filter(|_| !stages.is_empty());
    (stages.iter().map(|stage| stage.id.clone()).collect(), (!checks.is_empty()).then(|| checks.join("; ")), deploy)
}

// What kebbie did once the pushed stages rolled out: met ones, ones asked of the user, and whether every stage is met.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rolled {
    pub met: Vec<String>,
    pub asked: Vec<String>,
    pub checking: bool,
    pub all_met: bool,
}

impl Rolled {
    pub fn text(&self) -> String {
        let mut parts = Vec::new();
        if !self.met.is_empty() {
            parts.push(format!("{} met", self.met.join(", ")));
        }
        if !self.asked.is_empty() {
            parts.push(format!("asked you to verify {}", self.asked.join(", ")));
        }
        if self.checking {
            parts.push("the task runs its check".to_string());
        }
        parts.join("; ")
    }
}

// The pushed stages are out: a deploy is met when kebbie saw CI for it, a check waits on the task, a verification on the user; a deploy kebbie could not see is asked of the user instead.
pub fn rolled_out(state: &Path, address: &str, ids: &[String], unseen: Option<&str>, now: u64) -> Rolled {
    let mut rolled = Rolled::default();
    let mut ask: Vec<(String, String, Option<String>, bool)> = Vec::new();
    let stages = each(state, address, ids, |stage| {
        if stage.kept != Some(StageState::Pushed) {
            return;
        }
        match (stage.kind, unseen) {
            (RequirementKind::Deploy, None) => {
                stage.kept = Some(StageState::Met);
                stage.met_at = Some(now);
                rolled.met.push(stage.title.clone());
            }
            (RequirementKind::Deploy, Some(why)) => {
                stage.kept = Some(StageState::Waiting);
                ask.push((stage.id.clone(), stage.title.clone(), Some(why.to_string()), false));
                rolled.asked.push(stage.title.clone());
            }
            (RequirementKind::Check, _) => {
                stage.kept = Some(StageState::Waiting);
                rolled.checking = true;
            }
            (RequirementKind::Verified, _) => {
                stage.kept = Some(StageState::Waiting);
                ask.push((stage.id.clone(), stage.title.clone(), stage.detail.clone(), true));
                rolled.asked.push(stage.title.clone());
            }
        }
    }).unwrap_or_default();
    rolled.all_met = !stages.is_empty() && stages.iter().all(|stage| stage.state == StageState::Met);
    for (id, title, detail, verified) in ask {
        let text = if verified {
            format!("{title} is out. {}", detail.unwrap_or_default())
        } else {
            format!("{title} is pushed, but kebbie cannot see it deploy ({}). Has it deployed?", detail.unwrap_or_default())
        };
        if let Err(error) = crate::kebbie_questions::ask_verify(state, address, &text, Some(&id)) {
            eprintln!("kebbie: could not ask you to verify {title} of {address}: {error}");
        }
    }
    if !rolled.met.is_empty() {
        unlocked(address, &rolled.met, &stages);
    }
    rolled
}

// The task's check of the stages held: they are met.
pub fn check_held(state: &Path, address: &str, ids: &[String], now: u64) -> Rolled {
    let mut met = Vec::new();
    let stages = each(state, address, ids, |stage| {
        if stage.kind == RequirementKind::Check && matches!(stage.kept, Some(StageState::Waiting | StageState::Pushed)) {
            stage.kept = Some(StageState::Met);
            stage.met_at = Some(now);
            met.push(stage.title.clone());
        }
    }).unwrap_or_default();
    if !met.is_empty() {
        unlocked(address, &met, &stages);
    }
    Rolled { all_met: !stages.is_empty() && stages.iter().all(|stage| stage.state == StageState::Met), met, ..Rolled::default() }
}

// A stage that failed holds every stage after it; the task and the lead hear what failed, and the task raises it as an obstacle when the cause is not its own to fix.
pub fn failed(state: &Path, address: &str, ids: &[String], note: &str, tell_task: bool) {
    let note: String = note.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_STAGE_NOTE_CHARS).collect();
    let mut titles = Vec::new();
    let marked = each(state, address, ids, |stage| {
        if stage.pushed() && stage.kept != Some(StageState::Met) {
            stage.kept = Some(StageState::Failed);
            stage.note = Some(note.clone()).filter(|note| !note.is_empty());
            titles.push((stage.id.clone(), stage.title.clone()));
        }
    });
    if marked.is_err() || titles.is_empty() {
        return;
    }
    crate::kebbie_questions::settle_stages(state, address, ids);
    let names = titles.iter().map(|(_, title)| title.as_str()).collect::<Vec<_>>().join(", ");
    if tell_task {
        let id = &titles[0].0;
        let text = format!(
            "Stage {names} failed: {note}. The stages after it stay locked until it is met. Find the cause and fix it in a subtask of that stage: kebbie subtasks add \"<the fix>\" --because \"<what failed>\" --stage {id}, its commits moved up above the next stage's with kebbie order move when they land below them. Report ready; the next push of the stage asks for its requirement again. \
             When the cause is not yours to fix, raise it with kebbie obstacle --on \"stage {id}\" \"<what failed and what you found>\"."
        );
        if let Err(error) = messages::hand_to(address, &text) {
            eprintln!("Could not tell {address} that a stage failed: {error}");
        }
    }
    if let Err(error) = messages::to_lead(messages::ToLead::Notice, &format!("Stage {names} of {address} failed: {note}. The stages after it stay locked; the task fixes it, or raises it as an obstacle for the investigator.")) {
        eprintln!("Could not tell the lead: {error}");
    }
}

// Every stage of the task is met, or it has none.
pub fn all_met(state: &Path, address: &str) -> bool {
    plan::load(state, address).is_none_or(|plan| shown(&plan.stages).iter().all(|stage| stage.state == StageState::Met))
}

// The user's answer on a stage's verification: Verified meets it, anything said against it fails it.
pub fn verified(state: &Path, address: &str, id: &str, now: u64) -> Result<String> {
    let mut title = String::new();
    let stages = each(state, address, &[id.to_string()], |stage| {
        if stage.pushed() {
            stage.kept = Some(StageState::Met);
            stage.met_at = Some(now);
            stage.note = None;
            title = stage.title.clone();
        }
    })?;
    if title.is_empty() {
        return Err(format!("{address} has no pushed stage {id}"));
    }
    unlocked(address, std::slice::from_ref(&title), &stages);
    let next = stages.iter().find(|stage| stage.state == StageState::Open).map(|stage| format!("; {} can be pushed once its subtasks are accepted", stage.title)).unwrap_or_default();
    if stages.iter().all(|stage| stage.state == StageState::Met) {
        return Ok(format!("{title} is met; every stage of {address} is. {}", close_when_done(state, address)));
    }
    Ok(format!("{title} is met{next}."))
}

// The last stage met with nothing left to push: the task is done, as a push whose CI went green is.
fn close_when_done(state: &Path, address: &str) -> String {
    let (task, place) = address.split_once('@').unwrap_or((address, ""));
    let Some(repo) = crate::repo::pooled_checkouts().into_iter().find(|repo| name_of(repo) == place) else { return String::new() };
    let Ok(slot) = super::pool::find_task(&repo, Some(task), &repo) else { return String::new() };
    let Ok(upstream) = default_branch(&repo) else { return String::new() };
    if !plan::read_now(state, &slot.path, &upstream, address).order.is_empty() || super::open_commits_of(&slot.path, &upstream).is_ok_and(|open| !open.is_empty()) {
        return "It stays open with commits left to push.".to_string();
    }
    match super::done(&repo, Some(task)) {
        Ok(_) => "kebbie closed it as done.".to_string(),
        Err(error) => {
            let _ = crate::kebbie_questions::ask_close(state, address, &format!("{task} has every stage met and nothing left to push, but kebbie could not close it ({}). Close it?", crate::notice::Notice::read(&error, false).headline));
            "kebbie could not close it, so it asks you to.".to_string()
        }
    }
}

fn unlocked(address: &str, met: &[String], stages: &[Stage]) {
    let next = stages.iter().find(|stage| stage.state == StageState::Open);
    let then = match next {
        Some(stage) => format!(" {} is open: its subtasks can be pushed once they are accepted.", stage.title),
        None if stages.iter().all(|stage| stage.state == StageState::Met) => " Every stage is met.".to_string(),
        None => String::new(),
    };
    let text = format!("Stage {} of {address} is met.{then}", met.join(", "));
    if let Err(error) = messages::notify(address, &text) {
        eprintln!("Could not tell {address}: {error}");
    }
    if let Err(error) = messages::to_lead(messages::ToLead::Notice, &text) {
        eprintln!("Could not tell the lead: {error}");
    }
}

// A task whose first stage, Backfill, waits on your verification, and whose second, Drop the old column, is locked behind it.
#[cfg(test)]
pub(crate) fn waiting_on_you(state: &Path, address: &str) -> Result<()> {
    let mut fill = StagePlan::new("fill".to_string(), "Backfill".to_string(), RequirementKind::Verified, Some("every row has its email".to_string()));
    fill.through = Some("f".repeat(40));
    fill.pushed_at = Some(5);
    fill.kept = Some(StageState::Waiting);
    let cut = StagePlan::new("cut".to_string(), "Drop the old column".to_string(), RequirementKind::Deploy, None);
    let part = super::plan::Part { id: "fill".to_string(), title: "Backfill".to_string(), ..super::plan::Part::default() };
    changed(state, address, |_| Ok((Some(Plan { base: "base".to_string(), parts: vec![part], stages: vec![fill, cut], ..Plan::default() }), ())))
}

#[cfg(test)]
pub(crate) fn stages_in(state: &Path, address: &str) -> Vec<Stage> {
    plan::load(state, address).map(|plan| shown(&plan.stages)).unwrap_or_default()
}

#[cfg(test)]
#[path = "rollout_tests.rs"]
mod tests;
