use std::path::{Component, Path};

use toml::{Table, Value};

use super::{flag, path, permission_mode, skill, strings, subtable, table, template, text, AlsoNoticed, ReviewAuto, Second, NONE};
use crate::sh::Result;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worktree {
    pub setup_skill: Option<String>,
    pub setup_command: Option<String>,
    pub dev_server: Option<String>,
    pub dev_worker: Option<String>,
    pub dev_url: Option<String>,
    pub dev_route: Option<bool>,
    pub stage_on_main: Option<bool>,
    pub rules_skill: Option<String>,
    pub test_commands: Option<Vec<String>>,
    pub also_noticed: Option<AlsoNoticed>,
    pub permission_mode: Option<String>,
    pub profile: Option<String>,
    pub sandbox: Option<bool>,
    pub steps: Option<bool>,
    pub require_accept: Option<bool>,
    pub review_model: Option<String>,
    pub review_auto: Option<ReviewAuto>,
    pub review_profile: Option<String>,
    pub review_second: Option<Second>,
    pub investigate_profile: Option<String>,
    pub notify_command: Option<String>,
    pub vault_command: Option<String>,
    pub server_name: Option<String>,
    pub worker_name: Option<String>,
    pub ci_watch: Option<bool>,
    pub tests_per_commit: Option<bool>,
    pub comments_watch: Option<bool>,
    pub comments_days: Option<u64>,
    pub clean_on_release: Option<Vec<String>>,
    pub docs_required: Option<bool>,
}

fn worktree(table: &Table) -> Result<Worktree> {
    let Some(worktree) = subtable(table, "worktree", "worktree")? else { return Ok(Worktree::default()) };
    Ok(Worktree {
        setup_skill: skill(worktree, "setup_skill", "worktree.setup_skill")?,
        setup_command: text(worktree, "setup_command", "worktree.setup_command")?,
        dev_server: text(worktree, "dev_server", "worktree.dev_server")?,
        dev_worker: text(worktree, "dev_worker", "worktree.dev_worker")?,
        dev_url: template(worktree, "dev_url", "worktree.dev_url")?,
        dev_route: flag(worktree, "dev_route", "worktree.dev_route")?,
        stage_on_main: flag(worktree, "stage_on_main", "worktree.stage_on_main")?,
        rules_skill: None,
        test_commands: None,
        also_noticed: None,
        permission_mode: None,
        profile: None,
        sandbox: None,
        steps: None,
        require_accept: None,
        review_model: None,
        review_auto: None,
        review_profile: None,
        review_second: None,
        investigate_profile: None,
        notify_command: None,
        vault_command: None,
        server_name: text(worktree, "server_name", "worktree.server_name")?,
        worker_name: text(worktree, "worker_name", "worktree.worker_name")?,
        ci_watch: None,
        tests_per_commit: None,
        comments_watch: None,
        comments_days: None,
        clean_on_release: worktree.get("clean_on_release").map(clean_on_release).transpose()?,
        docs_required: None,
    })
}

// Folders inside the worktree only: a path that climbs out or starts at / could reach anything.
fn clean_on_release(value: &Value) -> Result<Vec<String>> {
    let folders = strings(value, "worktree.clean_on_release")?;
    for folder in &folders {
        let inside = !folder.trim().is_empty() && Path::new(folder).components().all(|part| matches!(part, Component::Normal(name) if name != ".git"));
        if !inside {
            return Err(format!("worktree.clean_on_release: \"{folder}\" must be a folder inside the worktree, such as \"target\" or \"node_modules/.cache\""));
        }
    }
    Ok(folders)
}

fn with_task(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(task) = subtable(table, "task", "task")? {
        worktree.rules_skill = skill(task, "rules_skill", "task.rules_skill")?;
        worktree.test_commands = task.get("test_commands").map(test_commands).transpose()?;
        worktree.also_noticed = also_noticed(task)?;
        worktree.permission_mode = text(task, "permission_mode", "task.permission_mode")?
            .map(|mode| permission_mode(&mode).map(str::to_string).map_err(|error| format!("task.permission_mode: {error}"))).transpose()?;
        worktree.profile = profile(task, "task.profile")?;
        worktree.sandbox = flag(task, "sandbox", "task.sandbox")?;
        worktree.steps = flag(task, "steps", "task.steps")?;
        worktree.require_accept = flag(task, "require_accept", "task.require_accept")?;
    }
    Ok(worktree)
}

fn profile(table: &Table, key: &str) -> Result<Option<String>> {
    let Some(name) = text(table, "profile", key)? else { return Ok(None) };
    if name != NONE {
        crate::profiles::name(&name).map_err(|error| format!("{key}: {error}"))?;
    }
    Ok(Some(name))
}

fn also_noticed(task: &Table) -> Result<Option<AlsoNoticed>> {
    let Some(name) = text(task, "also_noticed", "task.also_noticed")? else { return Ok(None) };
    AlsoNoticed::ALL.into_iter().find(|choice| choice.key() == name).map(Some)
        .ok_or(format!("task.also_noticed: \"{name}\" is not a choice; use \"backlog\", \"fix\" or \"ask\""))
}

fn test_commands(value: &Value) -> Result<Vec<String>> {
    let commands = strings(value, "task.test_commands")?;
    match commands.iter().find(|command| command.trim().is_empty()) {
        Some(_) => Err("task.test_commands: a command must not be empty".to_string()),
        None => Ok(commands.iter().map(|command| command.trim().to_string()).collect()),
    }
}

fn with_review(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(review) = subtable(table, "review", "review")? {
        worktree.review_model = text(review, "model", "review.model")?;
        worktree.review_auto = review_auto(review)?;
        worktree.review_profile = profile(review, "review.profile")?;
        worktree.review_second = text(review, "second", "review.second")?.map(|text| Second::parse(&text)).transpose()?;
    }
    if let Some(investigate) = subtable(table, "investigate", "investigate")? {
        worktree.investigate_profile = profile(investigate, "investigate.profile")?;
    }
    Ok(worktree)
}

fn review_auto(review: &Table) -> Result<Option<ReviewAuto>> {
    let Some(name) = text(review, "auto", "review.auto")? else { return Ok(None) };
    ReviewAuto::ALL.into_iter().find(|choice| choice.key() == name).map(Some)
        .ok_or(format!("review.auto: \"{name}\" is not a choice; use \"suggest\", \"run\" or \"require\""))
}

fn with_notify(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(notify) = subtable(table, "notify", "notify")? {
        worktree.notify_command = text(notify, "command", "notify.command")?;
    }
    Ok(worktree)
}

fn with_vault(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(vault) = subtable(table, "vault", "vault")? {
        worktree.vault_command = text(vault, "command", "vault.command")?;
    }
    Ok(worktree)
}

fn with_ci(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(ci) = subtable(table, "ci", "ci")? {
        worktree.ci_watch = flag(ci, "watch", "ci.watch")?;
    }
    if let Some(tests) = subtable(table, "tests", "tests")? {
        worktree.tests_per_commit = flag(tests, "per_commit", "tests.per_commit")?;
    }
    Ok(worktree)
}

fn with_comments(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(comments) = subtable(table, "comments", "comments")? {
        worktree.comments_watch = flag(comments, "watch", "comments.watch")?;
        worktree.comments_days = comments.get("watch_days").map(|value| value.as_integer().filter(|days| (1..=90).contains(days)).map(|days| days as u64).ok_or("comments.watch_days: must be a whole number of days, 1 to 90")).transpose()?;
    }
    Ok(worktree)
}

fn with_docs(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(docs) = subtable(table, "docs", "docs")? {
        worktree.docs_required = flag(docs, "required", "docs.required")?;
    }
    Ok(worktree)
}

pub(super) fn layers(table: &Table) -> Result<Worktree> {
    worktree(table).and_then(|worktree| with_task(table, worktree)).and_then(|worktree| with_review(table, worktree)).and_then(|worktree| with_notify(table, worktree)).and_then(|worktree| with_vault(table, worktree)).and_then(|worktree| with_ci(table, worktree)).and_then(|worktree| with_comments(table, worktree)).and_then(|worktree| with_docs(table, worktree))
}

const MACHINE_WIDE: &str = "applies to every repo, so it lives in config.toml";

// The [[actions]] are actions.rs's to check.
pub fn parse_repo(text: &str, source: &str) -> Result<Worktree> {
    let table = table(text, source)?;
    let fail = |error: String| format!("{source}: {error}");
    for (key, setting) in [("board", "[board] groups and [board.backlog] sources and ongoing"), ("notify", "[notify] command"), ("update", "[update] channel and agents"), ("vault", "[vault] command"), ("restart", "[restart] idle_minutes and auto"), ("keeper", "[keeper] enabled"), ("recover", "[recover] enabled"), ("machine", "[machine] name"), ("hub", "[hub] url"), ("keys", "[keys]"), ("report", "[report] signals, text and inbox_token"), ("issues", "[issues] propose")] {
        if table.contains_key(key) {
            return Err(fail(format!("{key}: {MACHINE_WIDE}; set {setting} in {}", path().display())));
        }
    }
    let worktree = layers(&table).map_err(fail)?;
    let repo = Path::new(source).file_stem().map(|stem| stem.to_string_lossy().to_string());
    crate::routines::parse_list(&table, repo.as_deref(), source).map_err(fail)?;
    if worktree.server_name.is_some() || worktree.worker_name.is_some() {
        return Err(fail(format!("worktree.server_name and worker_name: {MACHINE_WIDE}, since the sidebar names every Space's server and worker alike; set them in {}", path().display())));
    }
    Ok(worktree)
}
