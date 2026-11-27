use std::path::{Component, Path};

use toml::{Table, Value};

use super::{flag, path, permission_mode, skill, strings, subtable, table, template, text, unsettable, AlsoNoticed, BeforePush, Bump, ReviewAuto, Second, NONE};
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
    pub ci_deploy: Option<String>,
    pub ci_deploy_window: Option<String>,
    pub tests_per_commit: Option<bool>,
    pub tests_before_push: Option<BeforePush>,
    pub tests_full_command: Option<String>,
    pub tests_lint_command: Option<String>,
    pub comments_watch: Option<bool>,
    pub comments_days: Option<u64>,
    pub clean_on_release: Option<Vec<String>>,
    pub release_command: Option<String>,
    pub docs_required: Option<bool>,
    pub push_branch: Option<String>,
    pub push_choices: Option<Vec<String>>,
    pub push_prepare: Option<String>,
    pub push_version_file: Option<String>,
    pub push_version_key: Option<String>,
    pub push_version_command: Option<String>,
    pub push_version_bump: Option<Bump>,
}

fn worktree(table: &Table) -> Result<Worktree> {
    let Some(worktree) = subtable(table, "worktree", "worktree")? else { return Ok(Worktree::default()) };
    Ok(Worktree {
        setup_skill: skill(worktree, "setup_skill", "worktree.setup_skill")?,
        setup_command: unsettable(worktree, "setup_command", "worktree.setup_command")?,
        dev_server: unsettable(worktree, "dev_server", "worktree.dev_server")?,
        dev_worker: unsettable(worktree, "dev_worker", "worktree.dev_worker")?,
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
        ci_deploy: None,
        ci_deploy_window: None,
        tests_per_commit: None,
        tests_before_push: None,
        tests_full_command: None,
        tests_lint_command: None,
        comments_watch: None,
        comments_days: None,
        clean_on_release: worktree.get("clean_on_release").map(clean_on_release).transpose()?,
        release_command: unsettable(worktree, "release_command", "worktree.release_command")?,
        docs_required: None,
        push_branch: None,
        push_choices: None,
        push_prepare: None,
        push_version_file: None,
        push_version_key: None,
        push_version_command: None,
        push_version_bump: None,
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
        worktree.review_model = unsettable(review, "model", "review.model")?;
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
        worktree.ci_deploy = unsettable(ci, "deploy", "ci.deploy")?;
        worktree.ci_deploy_window = text(ci, "deploy_window", "ci.deploy_window")?;
        if let Some(window) = &worktree.ci_deploy_window {
            crate::deploy_window::parse(window).map_err(|error| format!("ci.deploy_window: {error}"))?;
        }
    }
    if let Some(tests) = subtable(table, "tests", "tests")? {
        worktree.tests_per_commit = flag(tests, "per_commit", "tests.per_commit")?;
        worktree.tests_before_push = before_push(tests)?;
        worktree.tests_full_command = text(tests, "full_command", "tests.full_command")?;
        worktree.tests_lint_command = text(tests, "lint_command", "tests.lint_command")?;
    }
    Ok(worktree)
}

fn before_push(tests: &Table) -> Result<Option<BeforePush>> {
    let Some(name) = text(tests, "before_push", "tests.before_push")? else { return Ok(None) };
    BeforePush::ALL.into_iter().find(|choice| choice.key() == name).map(Some)
        .ok_or(format!("tests.before_push: \"{name}\" is not a choice; use \"related\" or \"full\""))
}

fn with_comments(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(comments) = subtable(table, "comments", "comments")? {
        worktree.comments_watch = flag(comments, "watch", "comments.watch")?;
        worktree.comments_days = comments.get("watch_days").map(|value| value.as_integer().filter(|days| (1..=90).contains(days)).map(|days| days as u64).ok_or("comments.watch_days: must be a whole number of days, 1 to 90")).transpose()?;
    }
    Ok(worktree)
}

fn with_push(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    let Some(push) = subtable(table, "push", "push")? else { return Ok(worktree) };
    worktree.push_branch = text(push, "branch", "push.branch")?.map(|template| branch_template(&template).map(|_| template)).transpose().map_err(|error| format!("push.branch: {error}"))?;
    worktree.push_choices = push.get("choices").map(|value| strings(value, "push.choices")).transpose()?;
    for choice in worktree.push_choices.iter().flatten() {
        branch_template(choice).map_err(|error| format!("push.choices: {error}"))?;
    }
    worktree.push_prepare = text(push, "prepare", "push.prepare")?;
    if let Some(version) = subtable(push, "version", "push.version")? {
        worktree.push_version_file = text(version, "file", "push.version.file")?;
        worktree.push_version_key = text(version, "key", "push.version.key")?;
        worktree.push_version_command = text(version, "command", "push.version.command")?;
        worktree.push_version_bump = text(version, "bump", "push.version.bump")?
            .map(|name| Bump::ALL.into_iter().find(|bump| bump.key() == name).ok_or(format!("push.version.bump: \"{name}\" must be one of {}", Bump::ALL.map(|bump| format!("\"{}\"", bump.key())).join(", ")))).transpose()?;
        if worktree.push_version_file.is_some() && worktree.push_version_command.is_some() {
            return Err("push.version: set file or command, not both".to_string());
        }
        if worktree.push_version_key.is_some() && worktree.push_version_file.is_none() {
            return Err("push.version.key: names the key in push.version.file, so it needs the file".to_string());
        }
    }
    if (worktree.push_choices.is_some() || worktree.push_prepare.is_some() || worktree.push_version_file.is_some() || worktree.push_version_command.is_some() || worktree.push_version_bump.is_some()) && worktree.push_branch.is_none() {
        return Err("push: choices, prepare and version shape the branch push.branch names, so they need push.branch".to_string());
    }
    Ok(worktree)
}

pub const TASK_PLACE: &str = "{task}";
pub const NEXT_PLACE: &str = "{next}";
pub const CURRENT_PLACE: &str = "{current}";

// Each task gets a branch of its own, so a template without {task} would send every task to one shared branch.
fn branch_template(template: &str) -> Result<()> {
    if !template.contains(TASK_PLACE) {
        return Err(format!("\"{template}\" must name {TASK_PLACE}, such as \"feature/{TASK_PLACE}\", so each task pushes to a branch of its own"));
    }
    let mut rest = template.replace(TASK_PLACE, "");
    let versions = [NEXT_PLACE, CURRENT_PLACE].iter().map(|place| rest.matches(place).count()).sum::<usize>();
    if versions > 1 {
        return Err(format!("\"{template}\" names a version more than once; use one of {NEXT_PLACE} and {CURRENT_PLACE}"));
    }
    for place in [NEXT_PLACE, CURRENT_PLACE] {
        rest = rest.replace(place, "");
    }
    if let Some(start) = rest.find('{').filter(|at| rest[*at..].contains('}')) {
        let end = start + rest[start..].find('}').unwrap_or(0);
        return Err(format!("\"{template}\" names {}, which kebbie does not fill in; it takes {TASK_PLACE}, {NEXT_PLACE} and {CURRENT_PLACE}", &rest[start..=end]));
    }
    if !valid_branch(&fill(template, "task", Some("1.2"))) {
        return Err(format!("\"{template}\" is no branch name git takes"));
    }
    Ok(())
}

// The template with the task's name and the version in their places; a template that names no version ignores it.
pub fn fill(template: &str, task: &str, version: Option<&str>) -> String {
    let named = template.replace(TASK_PLACE, task);
    match version {
        Some(version) => named.replace(NEXT_PLACE, version).replace(CURRENT_PLACE, version),
        None => named,
    }
}

// git check-ref-format's rules for a branch name.
pub fn valid_branch(name: &str) -> bool {
    let bad_char = |character: char| character.is_ascii_control() || " ~^:?*[\\".contains(character);
    !name.is_empty() && !name.starts_with(['/', '-']) && !name.ends_with(['/', '.']) && !name.ends_with(".lock") && !name.contains("..") && !name.contains("//") && !name.contains("@{") && name != "@"
        && !name.chars().any(bad_char) && name.split('/').all(|part| !part.starts_with('.') && !part.ends_with(".lock"))
}

fn with_docs(table: &Table, mut worktree: Worktree) -> Result<Worktree> {
    if let Some(docs) = subtable(table, "docs", "docs")? {
        worktree.docs_required = flag(docs, "required", "docs.required")?;
    }
    Ok(worktree)
}

const UNSETTABLE: [&str; 6] = ["worktree.setup_command", "worktree.dev_server", "worktree.dev_worker", "worktree.release_command", "review.model", "ci.deploy"];

// The keys a repo config may set to "" to unset, by name, as layers read them.
pub(super) fn unset_keys(worktree: &Worktree) -> Vec<&'static str> {
    let values = [&worktree.setup_command, &worktree.dev_server, &worktree.dev_worker, &worktree.release_command, &worktree.review_model, &worktree.ci_deploy];
    UNSETTABLE.into_iter().zip(values).filter(|(_, value)| value.as_deref() == Some("")).map(|(key, _)| key).collect()
}

pub(super) fn layers(table: &Table) -> Result<Worktree> {
    worktree(table).and_then(|worktree| with_task(table, worktree)).and_then(|worktree| with_review(table, worktree)).and_then(|worktree| with_notify(table, worktree)).and_then(|worktree| with_vault(table, worktree)).and_then(|worktree| with_ci(table, worktree)).and_then(|worktree| with_comments(table, worktree)).and_then(|worktree| with_docs(table, worktree)).and_then(|worktree| with_push(table, worktree))
}

const MACHINE_WIDE: &str = "applies to every repo, so it lives in config.toml";

// The [[actions]] are actions.rs's to check.
pub fn parse_repo(text: &str, source: &str) -> Result<Worktree> {
    let table = table(text, source)?;
    let fail = |error: String| format!("{source}: {error}");
    for (key, setting) in [("board", "[board] groups and [board.backlog] sources and ongoing"), ("notify", "[notify] command"), ("update", "[update] channel and agents"), ("vault", "[vault] command"), ("restart", "[restart] idle_minutes, auto and lead_context"), ("keeper", "[keeper] enabled"), ("worker", "[worker] enabled"), ("link", "[link] networks"), ("builds", "[builds] slots and commands"), ("disk", "[disk] trim_below and builds_below"), ("recover", "[recover] enabled"), ("machine", "[machine] name"), ("hub", "[hub] url"), ("keys", "[keys]"), ("report", "[report] signals, text and inbox_token"), ("issues", "[issues] propose"), ("lead", "[lead] profile"), ("writer", "[writer] profile")] {
        if table.contains_key(key) {
            return Err(fail(format!("{key}: {MACHINE_WIDE}; set {setting} in {}", path().display())));
        }
    }
    let worktree = layers(&table).map_err(fail)?;
    let repo = Path::new(source).file_stem().map(|stem| stem.to_string_lossy().to_string());
    crate::routines::parse_list(&table, repo.as_deref(), source).map_err(fail)?;
    crate::jobs::parse(&table).map_err(fail)?;
    if worktree.server_name.is_some() || worktree.worker_name.is_some() {
        return Err(fail(format!("worktree.server_name and worker_name: {MACHINE_WIDE}, since the sidebar names every Space's server and worker alike; set them in {}", path().display())));
    }
    Ok(worktree)
}
