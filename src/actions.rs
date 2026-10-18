use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::repo::{default_branch, git, is_clean, name_of};
use crate::sh::Result;

// Keys the Areas page keeps for its built-in actions and moving around.
pub const RESERVED: [char; 9] = ['g', 's', 'b', 'l', 'u', 'j', 'k', 'q', '?'];

#[derive(Clone, Debug, PartialEq)]
pub struct Bump {
    pub file: String,
    pub after: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    pub bump: Vec<Bump>,
    pub run: Vec<String>,
    pub commit: Option<String>,
    pub tag: Option<String>,
    pub push: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Work {
    Script(Script),
    Task(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub id: String,
    pub key: char,
    pub title: String,
    pub about: String,
    pub asks: Vec<String>,
    pub work: Work,
    pub then: Vec<String>,
    pub source: String,
}

pub type Answers = Vec<(String, String)>;
type Parsed = (Vec<Action>, Vec<String>);

fn string(table: &Table, field: &str) -> Result<Option<String>> {
    match table.get(field) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("{field} must be a string")),
    }
}

fn strings(table: &Table, field: &str) -> Result<Vec<String>> {
    match table.get(field) {
        None => Ok(Vec::new()),
        Some(Value::String(text)) => Ok(vec![text.clone()]),
        Some(Value::Array(items)) => items.iter().map(|item| item.as_str().map(str::to_string).ok_or(format!("{field} must hold strings"))).collect(),
        Some(_) => Err(format!("{field} must be a string or a list of strings")),
    }
}

// A bump entry is a file, or { file, after } to change only the first version line after a marker (uv.lock pins other packages too).
fn bumps(table: &Table) -> Result<Vec<Bump>> {
    let Some(value) = table.get("bump") else { return Ok(Vec::new()) };
    let items = value.as_array().ok_or("bump must be a list")?;
    items.iter().map(|item| match item {
        Value::String(file) => Ok(Bump { file: file.clone(), after: None }),
        Value::Table(entry) => Ok(Bump {
            file: string(entry, "file")?.ok_or("a bump entry needs a file")?,
            after: string(entry, "after")?,
        }),
        _ => Err("bump entries are files or { file, after }".to_string()),
    }).collect()
}

fn slug(text: &str) -> String {
    text.to_lowercase().split(|character: char| !character.is_ascii_alphanumeric()).filter(|word| !word.is_empty()).collect::<Vec<_>>().join("-")
}

fn action(table: &Table, source: &str) -> Result<Action> {
    let title = string(table, "title")?.ok_or("needs a title")?;
    let key = string(table, "key")?.ok_or("needs a key")?;
    let mut characters = key.chars();
    let (Some(key), None) = (characters.next(), characters.next()) else { return Err(format!("key \"{key}\" must be one character")) };
    if RESERVED.contains(&key) {
        return Err(format!("key {key} is the Areas page's own"));
    }
    let asks = strings(table, "ask")?;
    let work = match string(table, "task")? {
        Some(prompt) => {
            if ["bump", "run", "commit", "tag", "push"].iter().any(|field| table.contains_key(*field)) {
                return Err("a task action has no script steps".to_string());
            }
            Work::Task(prompt)
        }
        None => {
            let script = Script {
                bump: bumps(table)?,
                run: strings(table, "run")?,
                commit: string(table, "commit")?,
                tag: string(table, "tag")?,
                push: table.get("push").map(|value| value.as_bool().ok_or("push must be true or false")).transpose()?.unwrap_or(false),
            };
            if !script.bump.is_empty() && !asks.iter().any(|ask| ask == "version") {
                return Err("bump needs ask = \"version\"".to_string());
            }
            if script.bump.is_empty() && script.run.is_empty() && script.commit.is_none() && script.tag.is_none() && !script.push {
                return Err("needs task, or script steps (bump, run, commit, tag, push)".to_string());
            }
            Work::Script(script)
        }
    };
    let then = strings(table, "then")?;
    if let Some(bad) = then.iter().find(|next| !next.contains(':')) {
        return Err(format!("then \"{bad}\" must be repo:action"));
    }
    Ok(Action {
        id: string(table, "id")?.unwrap_or_else(|| slug(&title)),
        key,
        about: string(table, "about")?.unwrap_or_default(),
        title,
        asks,
        work,
        then,
        source: source.to_string(),
    })
}

// A .kelpie.toml's [[actions]]; a broken action is reported and the rest still load.
pub fn parse(text: &str, source: &str) -> Parsed {
    let (actions, mut errors) = parse_actions(text, source);
    if text.parse::<Table>().is_ok()
        && let Err(error) = crate::config::parse_repo(text, source)
    {
        errors.push(error);
    }
    (actions, errors)
}

fn parse_override(text: &str, source: &str) -> Parsed {
    let (actions, mut errors) = parse_actions(text, source);
    if let Ok(table) = text.parse::<Table>()
        && let Err(error) = crate::config::unknown_keys(&table, "", &["actions"])
    {
        errors.push(format!("{source}: {error}"));
    }
    (actions, errors)
}

const ACTION_KEYS: [&str; 12] = ["id", "key", "title", "about", "ask", "task", "bump", "run", "commit", "tag", "push", "then"];

fn known_keys(table: &Table, index: usize) -> Result<()> {
    crate::config::unknown_keys(table, &format!("actions[{index}]."), &ACTION_KEYS)?;
    let entries = table.get("bump").and_then(Value::as_array).into_iter().flatten().enumerate();
    entries.filter_map(|(entry, item)| item.as_table().map(|item| (entry, item)))
        .try_for_each(|(entry, item)| crate::config::unknown_keys(item, &format!("actions[{index}].bump[{entry}]."), &["file", "after"]))
}

fn parse_actions(text: &str, source: &str) -> Parsed {
    let table: Table = match text.parse() {
        Ok(table) => table,
        Err(error) => return (Vec::new(), vec![format!("{source}: {}", error.to_string().lines().next().unwrap_or_default())]),
    };
    let Some(list) = table.get("actions").and_then(Value::as_array) else { return (Vec::new(), Vec::new()) };
    let (mut actions, mut errors) = (Vec::new(), Vec::new());
    for (index, item) in list.iter().enumerate() {
        if let Some(Err(error)) = item.as_table().map(|table| known_keys(table, index)) {
            errors.push(format!("{source}: {error}"));
            continue;
        }
        let parsed = item.as_table().ok_or("must be a table".to_string()).and_then(|table| action(table, source));
        match parsed {
            Ok(action) => actions.push(action),
            Err(error) => errors.push(format!("{source}, action {}: {error}", index + 1)),
        }
    }
    (actions, errors)
}

// Repos you don't own get their actions from your own kelpie config, $KELPIE_ACTIONS_DIR or ~/.config/kelpie/actions (a private repo is a good home).
pub fn overrides_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("KELPIE_ACTIONS_DIR") {
        return PathBuf::from(dir);
    }
    crate::config::dir().join("actions")
}

// An override with the same id replaces the repo's action; the rest are added. Two actions on one key keep the first.
pub fn merge(own: Vec<Action>, overrides: Vec<Action>) -> (Vec<Action>, Vec<String>) {
    let mut actions: Vec<Action> = own.into_iter().filter(|action| !overrides.iter().any(|other| other.id == action.id)).collect();
    actions.extend(overrides);
    let mut kept: Vec<Action> = Vec::new();
    let mut errors = Vec::new();
    for action in actions {
        match kept.iter().find(|other| other.key == action.key) {
            Some(other) => errors.push(format!("{}: key {} is taken by {}", action.source, action.key, other.title)),
            None => kept.push(action),
        }
    }
    (kept, errors)
}

pub fn load(repo: &Path) -> (Vec<Action>, Vec<String>) {
    let read = |path: PathBuf, source: String, parse: fn(&str, &str) -> Parsed| match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text, &source),
        Err(_) => (Vec::new(), Vec::new()),
    };
    let (own, mut errors) = read(repo.join(".kelpie.toml"), ".kelpie.toml".to_string(), parse);
    let file = overrides_dir().join(format!("{}.toml", name_of(repo)));
    let (overrides, override_errors) = read(file.clone(), file.display().to_string(), parse_override);
    errors.extend(override_errors);
    let (actions, merge_errors) = merge(own, overrides);
    errors.extend(merge_errors);
    (actions, errors)
}

pub fn answer<'a>(answers: &'a Answers, name: &str) -> Option<&'a str> {
    answers.iter().find(|(ask, _)| ask == name).map(|(_, value)| value.as_str())
}

pub fn render(template: &str, answers: &Answers) -> Result<String> {
    let mut text = template.to_string();
    for (name, value) in answers {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    match text.find('{').and_then(|start| text[start..].find('}').map(|end| &text[start..=start + end])) {
        Some(left) if !left.contains(' ') => Err(format!("{left} is not asked for")),
        _ => Ok(text),
    }
}

// The version a line assigns: a quoted dotted number on a line that says version.
fn quoted_version(line: &str) -> Option<&str> {
    if !line.to_lowercase().contains("version") {
        return None;
    }
    line.split('"').skip(1).step_by(2).find(|part| part.contains('.') && part.split('.').all(|piece| !piece.is_empty() && piece.chars().all(|character| character.is_ascii_digit())))
}

fn candidates<'a>(text: &'a str, bump: &Bump) -> Vec<(usize, &'a str)> {
    let lines = text.lines().enumerate();
    match &bump.after {
        Some(marker) => lines.skip_while(|(_, line)| !line.contains(marker.as_str())).filter(|(_, line)| quoted_version(line).is_some()).take(1).collect(),
        None => lines.filter(|(_, line)| quoted_version(line).is_some()).collect(),
    }
}

// The current version, from the first file to bump.
pub fn current_version(repo: &Path, script: &Script) -> Option<String> {
    let bump = script.bump.first()?;
    let text = std::fs::read_to_string(repo.join(&bump.file)).ok()?;
    candidates(&text, bump).first().and_then(|(_, line)| quoted_version(line)).map(str::to_string)
}

pub fn next_patch(version: &str) -> String {
    let mut parts: Vec<String> = version.split('.').map(str::to_string).collect();
    if let Some(last) = parts.last_mut()
        && let Ok(number) = last.parse::<u64>()
    {
        *last = (number + 1).to_string();
    }
    parts.join(".")
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Edit { file: String, line: usize, old: String, new: String },
    Run(String),
    Commit(String),
    Tag(String),
    Push { branch: String, tag: Option<String> },
}

impl Step {
    pub fn describe(&self) -> String {
        match self {
            Step::Edit { file, line, old, new } => format!("{file}:{}  {} → {}", line + 1, old.trim(), new.trim()),
            Step::Run(command) => format!("run  {command}"),
            Step::Commit(message) => format!("commit  \"{message}\""),
            Step::Tag(tag) => format!("tag  {tag}"),
            Step::Push { branch, tag } => match tag {
                Some(tag) => format!("push  {branch} and {tag} to origin (asks again first)"),
                None => format!("push  {branch} to origin (asks again first)"),
            },
        }
    }
}

// Every step a script action will take, worked out before anything changes; the plan is what the confirm window shows.
pub fn plan(script: &Script, repo: &Path, branch: &str, answers: &Answers) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    if !script.bump.is_empty() {
        let new = answer(answers, "version").filter(|version| !version.is_empty()).ok_or("No version given")?;
        let old = current_version(repo, script).ok_or_else(|| format!("No version line in {}", script.bump[0].file))?;
        if old == new {
            return Err(format!("{new} is already the version"));
        }
        for bump in &script.bump {
            let text = std::fs::read_to_string(repo.join(&bump.file)).map_err(|error| format!("{}: {error}", bump.file))?;
            let found: Vec<(usize, &str)> = candidates(&text, bump).into_iter().filter(|(_, line)| quoted_version(line) == Some(old.as_str())).collect();
            if found.is_empty() {
                return Err(format!("{} has no version line with {old}", bump.file));
            }
            for (line, text) in found {
                let changed = text.replacen(&format!("\"{old}\""), &format!("\"{new}\""), 1);
                steps.push(Step::Edit { file: bump.file.clone(), line, old: text.to_string(), new: changed });
            }
        }
    }
    for command in &script.run {
        steps.push(Step::Run(render(command, answers)?));
    }
    if let Some(message) = &script.commit {
        steps.push(Step::Commit(render(message, answers)?));
    }
    let tag = script.tag.as_ref().map(|tag| render(tag, answers)).transpose()?;
    if let Some(tag) = &tag {
        steps.push(Step::Tag(tag.clone()));
    }
    if script.push {
        steps.push(Step::Push { branch: branch.to_string(), tag });
    }
    Ok(steps)
}

// A script runs in the main checkout on its default branch, clean and level with or ahead of origin.
pub fn ready(repo: &Path) -> Result<String> {
    let upstream = default_branch(repo)?;
    let branch = upstream.trim_start_matches("origin/").to_string();
    let current = git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or_default();
    if current != branch {
        return Err(format!("{} is on {}, not {branch}", name_of(repo), if current.is_empty() { "a detached HEAD" } else { &current }));
    }
    if !is_clean(repo)? {
        return Err(format!("{} has uncommitted changes", name_of(repo)));
    }
    let behind = git(repo, &["rev-list", "--count", &format!("{branch}..{upstream}")])?;
    if behind != "0" {
        return Err(format!("{branch} is {behind} behind {upstream}; pull first"));
    }
    Ok(branch)
}

fn shell(repo: &Path, command: &str) -> Result<String> {
    let output = std::process::Command::new("sh").args(["-c", command]).current_dir(repo).stdin(std::process::Stdio::null())
        .output().map_err(|error| error.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)).trim().to_string();
    if output.status.success() { Ok(text) } else { Err(format!("{command} failed:\n{text}")) }
}

// The local steps, in order, stopping at the first failure; pushing is left to the caller, after asking.
pub fn run_local(repo: &Path, steps: &[Step]) -> Result<Vec<String>> {
    let mut done = Vec::new();
    for step in steps {
        match step {
            Step::Edit { file, line, old, new } => {
                let path = repo.join(file);
                let text = std::fs::read_to_string(&path).map_err(|error| format!("{file}: {error}"))?;
                let mut lines: Vec<&str> = text.split('\n').collect();
                if lines.get(*line).map(|current| current.trim_end_matches('\r')) != Some(old.as_str()) {
                    return Err(format!("{file}:{} changed since the plan", line + 1));
                }
                lines[*line] = new;
                std::fs::write(&path, lines.join("\n")).map_err(|error| format!("{file}: {error}"))?;
            }
            Step::Run(command) => {
                shell(repo, command)?;
            }
            Step::Commit(message) => {
                git(repo, &["add", "--update"])?;
                git(repo, &["commit", "--quiet", "-m", message])?;
            }
            Step::Tag(tag) => {
                git(repo, &["tag", tag])?;
            }
            Step::Push { .. } => continue,
        }
        done.push(step.describe());
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = r#"
[[actions]]
key = "R"
title = "Release"
about = "bump, commit, tag, push"
ask = "version"
bump = ["pyproject.toml", { file = "uv.lock", after = 'name = "notes-vault-mcp"' }]
commit = "{version}"
tag = "v{version}"
push = true
then = ["claude-plugins:bump-pin"]

[[actions]]
id = "bump-pin"
key = "P"
title = "Bump a pin"
ask = ["plugin", "version"]
task = "Bump {plugin}'s pin to {version}; commit."
"#;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-actions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn release() -> Script {
        match &parse(RELEASE, ".kelpie.toml").0[0].work {
            Work::Script(script) => script.clone(),
            Work::Task(_) => panic!("release is a script"),
        }
    }

    #[test]
    fn the_example_parses_and_plans_against_notes_vault_mcp_files() {
        let example = include_str!("../examples/notes-vault-mcp.kelpie.toml");
        let (actions, errors) = parse(example, "example");
        assert!(errors.is_empty(), "{errors:?}");
        let pin = example.lines().filter_map(|line| line.strip_prefix("# ")).collect::<Vec<_>>().join("\n");
        let (pins, errors) = parse(&pin[pin.find("[[actions]]").unwrap()..], "pin");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(pins[0].id, "bump-pin");
        let Work::Script(script) = &actions[0].work else { panic!("release is a script") };
        let repo = scratch("example");
        std::fs::create_dir_all(repo.join("notes_vault_mcp")).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = \"notes-vault-mcp\"\nversion = \"0.5.0\"\nrequires-python = \">=3.11\"\n").unwrap();
        std::fs::write(repo.join("server.json"), "{\n  \"version\": \"0.5.0\",\n  \"packages\": [{\n    \"version\": \"0.5.0\"\n  }]\n}\n").unwrap();
        std::fs::write(repo.join("notes_vault_mcp/server.py"), "VERSION = \"0.5.0\"\n").unwrap();
        std::fs::write(repo.join("uv.lock"), "[[package]]\nname = \"mcp\"\nversion = \"0.5.0\"\n\n[[package]]\nname = \"notes-vault-mcp\"\nversion = \"0.5.0\"\n").unwrap();
        let steps = plan(script, &repo, "main", &vec![("version".to_string(), "0.5.1".to_string())]).unwrap();
        let edits: Vec<(&str, usize)> = steps.iter().filter_map(|step| match step { Step::Edit { file, line, .. } => Some((file.as_str(), *line)), _ => None }).collect();
        assert_eq!(edits, [("pyproject.toml", 2), ("server.json", 1), ("server.json", 3), ("notes_vault_mcp/server.py", 0), ("uv.lock", 6)]);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn parses_script_and_task_actions() {
        let (actions, errors) = parse(RELEASE, ".kelpie.toml");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(actions[0].id, "release");
        assert_eq!(actions[0].key, 'R');
        assert_eq!(actions[0].asks, ["version"]);
        assert_eq!(actions[0].then, ["claude-plugins:bump-pin"]);
        assert_eq!(release().bump[1], Bump { file: "uv.lock".to_string(), after: Some("name = \"notes-vault-mcp\"".to_string()) });
        assert!(release().push);
        assert_eq!(actions[1].id, "bump-pin");
        assert_eq!(actions[1].asks, ["plugin", "version"]);
        assert_eq!(actions[1].work, Work::Task("Bump {plugin}'s pin to {version}; commit.".to_string()));
    }

    #[test]
    fn a_broken_action_is_reported_and_the_rest_load() {
        let text = "[[actions]]\nkey = \"g\"\ntitle = \"Mine\"\nrun = \"x\"\n\n[[actions]]\nkey = \"B\"\ntitle = \"Build\"\nrun = \"cargo build\"\n\n[[actions]]\nkey = \"Z\"\ntitle = \"Nothing\"\n";
        let (actions, errors) = parse(text, ".kelpie.toml");
        assert_eq!(actions.iter().map(|action| action.key).collect::<Vec<_>>(), ['B']);
        assert_eq!(errors, [".kelpie.toml, action 1: key g is the Areas page's own", ".kelpie.toml, action 3: needs task, or script steps (bump, run, commit, tag, push)"]);
        assert_eq!(parse("not toml [", "x").1.len(), 1);
        assert!(parse("[[actions]]\nkey = \"V\"\ntitle = \"v\"\nbump = [\"a\"]\n", "x").1[0].contains("ask = \"version\""));
    }

    #[test]
    fn an_unknown_key_in_an_action_or_a_bump_entry_is_named_and_that_action_skipped() {
        let text = "[[actions]]\nkey = \"B\"\ntitle = \"Build\"\nrun = \"cargo build\"\n\n[[actions]]\nkey = \"C\"\ntitle = \"Check\"\nrun = \"cargo check\"\ncolour = \"red\"\n\n[[actions]]\nkey = \"V\"\ntitle = \"Version\"\nask = \"version\"\nbump = [\"a\", { file = \"uv.lock\", afer = \"x\" }]\n";
        let (actions, errors) = parse(text, ".kelpie.toml");
        assert_eq!(actions.iter().map(|action| action.key).collect::<Vec<_>>(), ['B']);
        assert_eq!(errors, [
            ".kelpie.toml: actions[1].colour: unknown key; known keys here: id, key, title, about, ask, task, bump, run, commit, tag, push, then",
            ".kelpie.toml: actions[2].bump[1].afer: unknown key; known keys here: file, after",
        ]);
        assert_eq!(parse_override(text, "app.toml").1.len(), 2);
    }

    #[test]
    fn a_repo_file_takes_worktree_next_to_actions_and_names_unknown_keys() {
        let text = "[worktree]\ndev_server = \"npm run dev\"\n\n[[actions]]\nkey = \"B\"\ntitle = \"Build\"\nrun = \"cargo build\"\n";
        assert_eq!(parse(text, ".kelpie.toml"), (parse_actions(text, ".kelpie.toml").0, Vec::new()));
        let (actions, errors) = parse(&format!("colour = 1\n{text}"), "/src/app/.kelpie.toml");
        assert_eq!(actions.len(), 1);
        assert_eq!(errors, ["/src/app/.kelpie.toml: colour: unknown key; known keys here: actions, worktree, task, review, notify"]);
        assert_eq!(parse_override(text, "app.toml").1, ["app.toml: worktree: unknown key; known keys here: actions"]);
    }

    #[test]
    fn overrides_replace_by_id_and_keys_stay_unique() {
        let own = parse(RELEASE, ".kelpie.toml").0;
        let theirs = parse("[[actions]]\nid = \"release\"\nkey = \"R\"\ntitle = \"Release\"\nrun = \"make release\"\n\n[[actions]]\nkey = \"P\"\ntitle = \"Other\"\nrun = \"x\"\n", "override").0;
        let (merged, errors) = merge(own, theirs);
        assert_eq!(merged.iter().map(|action| (action.key, action.source.as_str())).collect::<Vec<_>>(), [('P', ".kelpie.toml"), ('R', "override")]);
        assert_eq!(errors, ["override: key P is taken by Bump a pin"]);
    }

    #[test]
    fn templates_fill_answers_and_refuse_unknown_ones() {
        let answers = vec![("version".to_string(), "0.5.1".to_string())];
        assert_eq!(render("v{version}", &answers).unwrap(), "v0.5.1");
        assert!(render("{plugin}", &answers).is_err());
        assert_eq!(render("echo '{ a }'", &answers).unwrap(), "echo '{ a }'");
    }

    #[test]
    fn versions_are_found_on_version_lines_and_bumped_by_a_patch() {
        assert_eq!(quoted_version("version = \"0.5.0\""), Some("0.5.0"));
        assert_eq!(quoted_version("  \"version\": \"0.5.0\","), Some("0.5.0"));
        assert_eq!(quoted_version("VERSION = \"0.5.0\""), Some("0.5.0"));
        assert_eq!(quoted_version("name = \"0.5.0\""), None);
        assert_eq!(quoted_version("version = \"latest\""), None);
        assert_eq!(next_patch("0.5.0"), "0.5.1");
        assert_eq!(next_patch("1.9"), "1.10");
    }

    #[test]
    fn a_release_plan_edits_only_the_versions_it_should() {
        let repo = scratch("plan");
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = \"notes-vault-mcp\"\nversion = \"0.5.0\"\n").unwrap();
        std::fs::write(repo.join("uv.lock"), "[[package]]\nname = \"other\"\nversion = \"0.5.0\"\n\n[[package]]\nname = \"notes-vault-mcp\"\nversion = \"0.5.0\"\n").unwrap();
        let script = release();
        assert_eq!(current_version(&repo, &script).as_deref(), Some("0.5.0"));
        let answers = vec![("version".to_string(), "0.5.1".to_string())];
        let steps = plan(&script, &repo, "main", &answers).unwrap();
        assert_eq!(steps, [
            Step::Edit { file: "pyproject.toml".to_string(), line: 2, old: "version = \"0.5.0\"".to_string(), new: "version = \"0.5.1\"".to_string() },
            Step::Edit { file: "uv.lock".to_string(), line: 6, old: "version = \"0.5.0\"".to_string(), new: "version = \"0.5.1\"".to_string() },
            Step::Commit("0.5.1".to_string()),
            Step::Tag("v0.5.1".to_string()),
            Step::Push { branch: "main".to_string(), tag: Some("v0.5.1".to_string()) },
        ]);
        assert!(plan(&script, &repo, "main", &vec![("version".to_string(), "0.5.0".to_string())]).is_err());
        std::fs::write(repo.join("uv.lock"), "name = \"notes-vault-mcp\"\nversion = \"0.4.0\"\n").unwrap();
        assert_eq!(plan(&script, &repo, "main", &answers).unwrap_err(), "uv.lock has no version line with 0.5.0");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn local_steps_edit_commit_and_tag_without_pushing() {
        let repo = scratch("run");
        let git = |args: &[&str]| crate::repo::git(&repo, args).unwrap();
        git(&["init", "--quiet", "-b", "main"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(repo.join("pyproject.toml"), "version = \"0.5.0\"\n").unwrap();
        std::fs::write(repo.join("uv.lock"), "name = \"notes-vault-mcp\"\nversion = \"0.5.0\"\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "start"]);
        let mut script = release();
        script.run = vec!["echo {version} > built".to_string()];
        let steps = plan(&script, &repo, "main", &vec![("version".to_string(), "0.5.1".to_string())]).unwrap();
        let done = run_local(&repo, &steps).unwrap();
        assert_eq!(done.len(), 5);
        assert_eq!(std::fs::read_to_string(repo.join("pyproject.toml")).unwrap(), "version = \"0.5.1\"\n");
        assert_eq!(git(&["log", "-1", "--format=%s"]), "0.5.1");
        assert_eq!(git(&["tag"]), "v0.5.1");
        assert_eq!(git(&["status", "--porcelain"]), "?? built");
        let _ = std::fs::remove_dir_all(&repo);
    }
}
