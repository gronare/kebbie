use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use toml::Table;

use crate::agent::Agent;
use crate::config;
use crate::sandbox::Policy;
use crate::sh::Result;

pub const TASK: &str = "task";
pub const REVIEWER: &str = "reviewer";
pub const OPS: &str = "ops";
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub const CODEX_EFFORTS: [&str; 9] = ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra", "persistent"];
pub const KEYS: [&str; 13] = ["about", "for", "agent", "model", "effort", "role", "skills", "permission_mode", "deny", "allow", "claude", "sandbox", "kind"];
const SANDBOX_KEYS: [&str; 5] = ["enabled", "allowed_domains", "allow_write", "excluded_commands", "kubeconfig"];
const REVIEWER_KEYS: [&str; 7] = ["about", "for", "agent", "model", "effort", "role", "kind"];
pub const REVIEW_KIND: &str = "review";
const GUARDED: [&str; 9] = ["push", "try", "stage", "unstage", "approve", "choose", "away", "decide", "after"];

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub about: String,
    pub fits: String,
    pub agent: Agent,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub role: Option<PathBuf>,
    pub skills: Vec<String>,
    pub permission_mode: Option<String>,
    pub deny: Vec<String>,
    pub allow: Vec<String>,
    pub plugins: Vec<String>,
    pub sandbox: Policy,
    pub file: Option<PathBuf>,
    pub review: bool,
}

pub fn dir() -> PathBuf {
    std::env::var("KEBBIE_PROFILES_DIR").map(PathBuf::from).unwrap_or_else(|_| config::dir().join("profiles"))
}

pub fn built_in(name: &str) -> Option<Profile> {
    let (about, fits) = match name {
        TASK => ("kebbie's own task: its role, hooks and permissions, nothing added", "any task no other profile fits"),
        REVIEWER => ("The read-only reviewer that kebbie review runs", "kebbie review, never a task"),
        OPS => ("kebbie's own task whose ssh, scp and rsync run outside the sandbox, so they reach other machines, and this one as you", "work on other machines over ssh"),
        _ => return None,
    };
    let sandbox = if name == OPS { Policy { excluded: crate::sandbox::OPS_COMMANDS.map(str::to_string).to_vec(), ..Policy::default() } } else { Policy::default() };
    Some(Profile {
        name: name.to_string(), about: about.to_string(), fits: fits.to_string(), agent: Agent::Claude, model: None, effort: None, role: None,
        skills: Vec::new(), permission_mode: None, deny: Vec::new(), allow: Vec::new(), plugins: Vec::new(), sandbox, file: None, review: name == REVIEWER,
    })
}

pub fn valid_name(name: &str) -> bool {
    name.len() <= 40 && name.starts_with(|character: char| character.is_ascii_lowercase()) && name.chars().all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
}

pub fn name(name: &str) -> Result<&str> {
    if valid_name(name) { Ok(name) } else { Err(format!("\"{name}\" is not a profile name; use lower-case letters, digits and dashes, starting with a letter")) }
}

pub fn load(name: &str) -> Result<Profile> {
    load_in(&dir(), name)
}

// A file named after a built-in profile adds to it; any other name needs its file.
pub fn load_in(dir: &Path, wanted: &str) -> Result<Profile> {
    name(wanted)?;
    let file = dir.join(format!("{wanted}.toml"));
    let base = built_in(wanted);
    match std::fs::read_to_string(&file) {
        Ok(text) => parse(&text, wanted, &file, base),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => base.ok_or_else(|| format!("No profile \"{wanted}\": {} is not there; kebbie profiles lists them", file.display())),
        Err(error) => Err(format!("{}: {error}", file.display())),
    }
}

pub fn chosen<'a>(given: Option<&'a str>, recorded: Option<&'a str>, configured: Option<&'a str>) -> &'a str {
    given.or(recorded).or(configured).unwrap_or(TASK)
}

// A running task whose recorded profile no longer loads goes on as the built-in task, and the caller says why.
pub fn for_running_task(dir: &Path, given: Option<&str>, recorded: Option<&str>, configured: Option<&str>) -> Result<(Profile, Option<String>)> {
    match for_task(dir, chosen(given, recorded, configured)) {
        Ok(profile) => Ok((profile, None)),
        Err(error) if given.is_none() && recorded.is_some() => {
            let recorded = recorded.unwrap_or_default();
            let task = built_in(TASK).ok_or("the built-in task profile is missing")?;
            Ok((task, Some(format!("its profile \"{recorded}\" did not load ({error}), so it runs as the built-in task until the profile loads and the task starts again with --profile {recorded}"))))
        }
        Err(error) => Err(error),
    }
}

// A Codex task resumed on its profile alone: the built-in task the start falls back to is a Claude role, which a Codex session must never be given.
pub fn codex_restart_refusal(dir: &Path, recorded: Option<&str>, configured: Option<&str>) -> Option<String> {
    let wanted = chosen(None, recorded, configured);
    match for_task(dir, wanted) {
        Ok(profile) if profile.agent == Agent::Codex => None,
        Ok(_) => Some(format!("its profile \"{wanted}\" no longer runs Codex, so its Codex session is not resumed on a Claude role")),
        Err(error) => Some(format!("its profile \"{wanted}\" does not load ({error}), so its Codex session is not resumed on the built-in Claude role")),
    }
}

pub fn for_task(dir: &Path, wanted: &str) -> Result<Profile> {
    if wanted == REVIEWER {
        return Err(format!("\"{REVIEWER}\" is kebbie review's profile, not a task's; kebbie profiles lists the others"));
    }
    let profile = load_in(dir, wanted)?;
    if profile.review {
        return Err(format!("\"{wanted}\" is a review profile, which kebbie review runs, not a task's; kebbie profiles lists the others"));
    }
    Ok(profile)
}

// The profile review.profile or review.second names: one with kind = "review", or the built-in reviewer.
pub fn for_review(dir: &Path, wanted: &str) -> Result<Profile> {
    let profile = load_in(dir, wanted)?;
    if !profile.review {
        return Err(format!("\"{wanted}\" is a task's profile; a reviewer is a profile with kind = \"review\", as the built-in reviewer"));
    }
    Ok(profile)
}

fn strings(table: &Table, key: &str, source: &str) -> Result<Vec<String>> {
    let Some(value) = table.get(key) else { return Ok(Vec::new()) };
    let fail = || format!("{source}: {key}: must be a list of strings");
    let items = value.as_array().ok_or_else(fail)?;
    let found: Vec<String> = items.iter().map(|item| item.as_str().map(|text| text.trim().to_string()).ok_or_else(fail)).collect::<Result<_>>()?;
    match found.iter().find(|item| item.is_empty()) {
        Some(_) => Err(format!("{source}: {key}: an entry must not be empty")),
        None => Ok(found),
    }
}

fn text(table: &Table, key: &str, source: &str) -> Result<Option<String>> {
    let Some(value) = table.get(key) else { return Ok(None) };
    match value.as_str().map(str::trim) {
        Some("") => Err(format!("{source}: {key}: must not be empty")),
        Some(text) => Ok(Some(text.to_string())),
        None => Err(format!("{source}: {key}: must be a string")),
    }
}

// Claude Code lets a deny rule win over any allow, but kebbie away and decide are guarded by who calls them, so a rule naming one is refused outright.
pub fn releases_guard(rule: &str) -> bool {
    let Some(inner) = rule.trim().strip_prefix("Bash") else { return false };
    let inner = inner.trim();
    if inner.is_empty() {
        return true;
    }
    let Some(command) = inner.strip_prefix('(').and_then(|rest| rest.strip_suffix(')')) else { return false };
    let words: Vec<&str> = command.split(|character: char| character.is_whitespace() || character == ':').filter(|word| !word.is_empty()).collect();
    let Some((program, rest)) = words.split_first() else { return true };
    if program.rsplit('/').next() != Some("kebbie") && !program.contains('*') {
        return false;
    }
    let mut rest = rest.iter();
    let mut command = None;
    while let Some(word) = rest.next() {
        if *word == "-C" {
            rest.next();
        } else if word.starts_with('-') {
            return true;
        } else {
            command = Some(*word);
            break;
        }
    }
    command.is_none_or(|command| command.contains('*') || GUARDED.contains(&command))
}

// Claude Code's permission rules, plugins and sandbox lists have no Codex counterpart kebbie can set, so a Codex profile is refused them rather than silently running without them.
fn claude_only(table: &Table) -> Option<String> {
    let keys = ["deny", "allow", "claude"].into_iter().find(|key| table.contains_key(*key)).map(str::to_string);
    let sandbox = table.get("sandbox").and_then(toml::Value::as_table).and_then(|sandbox| sandbox.keys().find(|key| !["enabled", "kubeconfig"].contains(&key.as_str())).map(|key| format!("sandbox.{key}")));
    keys.or(sandbox)
}

// Codex's own named levels, as its ReasoningEffort parses them; Claude Code takes its five.
pub fn efforts(agent: Agent) -> &'static [&'static str] {
    if agent == Agent::Codex { &CODEX_EFFORTS } else { &EFFORTS }
}

pub fn parse(contents: &str, name: &str, file: &Path, base: Option<Profile>) -> Result<Profile> {
    let source = file.display().to_string();
    let table: Table = contents.parse().map_err(|error: toml::de::Error| format!("{source}: {}", error.to_string().trim()))?;
    let reviewer = name == REVIEWER;
    let base_review = base.as_ref().is_some_and(|base| base.review);
    let review = match text(&table, "kind", &source)?.as_deref() {
        None => base_review,
        Some(REVIEW_KIND) if base.is_none() || base_review => true,
        Some("task") if !base_review => false,
        Some(kind @ ("task" | REVIEW_KIND)) => return Err(format!("{source}: kind: \"{name}\" is kebbie's own {} profile, so its kind is not \"{kind}\"", if base_review { "review" } else { "task" })),
        Some(kind) => return Err(format!("{source}: kind: \"{kind}\" is not a kind; use \"task\" or \"review\"")),
    };
    let known: &[&str] = if review { &REVIEWER_KEYS } else { &KEYS };
    if review && let Some(key) = table.keys().find(|key| !known.contains(&key.as_str()) && KEYS.contains(&key.as_str())) {
        return Err(format!("{source}: {key}: the reviewer only reads, so its profile takes {}", REVIEWER_KEYS.join(", ")));
    }
    config::unknown_keys(&table, "", known).map_err(|error| format!("{source}: {error}"))?;
    let mut profile = base.unwrap_or(Profile {
        name: name.to_string(), about: String::new(), fits: String::new(), agent: Agent::Claude, model: None, effort: None, role: None,
        skills: Vec::new(), permission_mode: None, deny: Vec::new(), allow: Vec::new(), plugins: Vec::new(), sandbox: Policy::default(), file: None, review,
    });
    profile.file = Some(file.to_path_buf());
    if let Some(about) = text(&table, "about", &source)? {
        profile.about = about;
    }
    if let Some(fits) = text(&table, "for", &source)? {
        profile.fits = fits;
    }
    if let Some(agent) = text(&table, "agent", &source)? {
        profile.agent = Agent::named(&agent).map_err(|error| format!("{source}: agent: {error}"))?;
    }
    if reviewer && profile.agent != Agent::Claude {
        return Err(format!("{source}: agent: the built-in reviewer runs Claude, as the assessor and the rules check read with it; a review profile of your own can name \"{}\"", profile.agent.key()));
    }
    if profile.agent == Agent::Codex && let Some(key) = claude_only(&table) {
        return Err(format!("{source}: {key}: Claude Code's, which a Codex task does not read; drop it from a profile with agent = \"codex\""));
    }
    profile.model = text(&table, "model", &source)?.or(profile.model);
    if let Some(effort) = text(&table, "effort", &source)? {
        let levels = efforts(profile.agent);
        if !levels.contains(&effort.as_str()) {
            let agent = if profile.agent == Agent::Codex { " Codex takes" } else { "" };
            return Err(format!("{source}: effort: \"{effort}\" is not an effort level{agent}; use {}", levels.iter().map(|level| format!("\"{level}\"")).collect::<Vec<_>>().join(", ")));
        }
        profile.effort = Some(effort);
    }
    if let Some(role) = text(&table, "role", &source)? {
        if !config::inside(&role) {
            return Err(format!("{source}: role: \"{role}\" leaves the profiles folder; name a file in it, without / at the start or .."));
        }
        let path = file.parent().unwrap_or(Path::new(".")).join(&role);
        if path.symlink_metadata().is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(format!("{source}: role: {} is a symlink; put the role file itself in the profiles folder", path.display()));
        }
        if !path.is_file() {
            return Err(format!("{source}: role: {} is not a file; a role is a markdown file, its path relative to the profile's folder", path.display()));
        }
        profile.role = Some(path);
    }
    profile.skills = strings(&table, "skills", &source)?;
    if let Some(skill) = profile.skills.iter().find(|skill| !config::skill_name(skill)) {
        return Err(format!("{source}: skills: \"{skill}\" is not a skill name; use <skill> or <plugin>:<skill>, without / or .."));
    }
    profile.permission_mode = text(&table, "permission_mode", &source)?
        .map(|mode| config::permission_mode(&mode).map(str::to_string).map_err(|error| format!("{source}: permission_mode: {error}"))).transpose()?;
    profile.deny = strings(&table, "deny", &source)?;
    profile.allow = strings(&table, "allow", &source)?;
    if let Some(rule) = profile.allow.iter().find(|rule| releases_guard(rule)) {
        return Err(format!("{source}: allow: \"{rule}\" would let the task run kebbie {}, which stay the user's and the lead's; name the commands it may run instead", GUARDED.join(", ")));
    }
    if let Some(claude) = table.get("claude") {
        let claude = claude.as_table().ok_or(format!("{source}: claude: must be a table"))?;
        config::unknown_keys(claude, "claude.", &["plugins"]).map_err(|error| format!("{source}: {error}"))?;
        profile.plugins = strings(claude, "plugins", &source)?;
        if let Some(plugin) = profile.plugins.iter().find(|plugin| plugin.split_once('@').is_none_or(|(name, market)| name.is_empty() || market.is_empty() || market.contains('@'))) {
            return Err(format!("{source}: claude.plugins: \"{plugin}\" is not a plugin; name it <plugin>@<marketplace>, as claude plugin list shows it"));
        }
    }
    if let Some(sandbox) = table.get("sandbox") {
        sandbox_policy(sandbox, &source, &mut profile.sandbox)?;
    }
    if profile.sandbox.kubeconfig.is_some() && file.symlink_metadata().is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(format!("{source}: sandbox.kubeconfig: the profile file is a symlink, so a file shared from elsewhere would pick the cluster login; write the profile itself in the profiles folder"));
    }
    Ok(profile)
}

// A profile's lists of hosts and writable folders replace kebbie's own; its commands outside the sandbox add to the base's.
fn sandbox_policy(value: &toml::Value, source: &str, policy: &mut Policy) -> Result<()> {
    let table = value.as_table().ok_or(format!("{source}: sandbox: must be a table"))?;
    config::unknown_keys(table, "sandbox.", &SANDBOX_KEYS).map_err(|error| format!("{source}: {error}"))?;
    if let Some(enabled) = table.get("enabled") {
        policy.enabled = Some(enabled.as_bool().ok_or(format!("{source}: sandbox.enabled: must be true or false"))?);
    }
    if table.contains_key("allowed_domains") {
        policy.domains = Some(strings(table, "allowed_domains", source)?);
    }
    if table.contains_key("allow_write") {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        let writes = strings(table, "allow_write", source)?;
        if let Some(why) = writes.iter().find_map(|entry| crate::sandbox::write_refusal(entry, &home)) {
            return Err(format!("{source}: sandbox.allow_write: {why}"));
        }
        policy.writes = Some(writes);
    }
    let excluded = strings(table, "excluded_commands", source)?;
    if let Some(why) = excluded.iter().find_map(|entry| crate::sandbox::excluded_refusal(entry)) {
        return Err(format!("{source}: sandbox.excluded_commands: {why}"));
    }
    policy.excluded.extend(excluded.into_iter().filter(|entry| !policy.excluded.contains(entry)).collect::<Vec<_>>());
    if let Some(kubeconfig) = text(table, "kubeconfig", source).map_err(|error| error.replacen(": kubeconfig:", ": sandbox.kubeconfig:", 1))? {
        if crate::config::skill_path(&kubeconfig).is_none() {
            return Err(format!("{source}: sandbox.kubeconfig: \"{kubeconfig}\" is not a path from / or ~/; name the kubeconfig file its tasks' kubectl and helm use"));
        }
        policy.kubeconfig = Some(kubeconfig);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub enum Edited {
    Wrote(PathBuf),
    Removed(PathBuf),
    Unchanged,
}

// Sets or removes keys in a profile's file and nothing else; a built-in profile's file that is left empty goes, and a change the parser refuses is never written.
pub fn edit_in(dir: &Path, wanted: &str, changes: &[(&str, Option<&str>)]) -> Result<Edited> {
    name(wanted)?;
    let file = dir.join(format!("{wanted}.toml"));
    let base = built_in(wanted);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && base.is_some() => String::new(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(format!("No profile \"{wanted}\": {} is not there", file.display())),
        Err(error) => return Err(format!("{}: {error}", file.display())),
    };
    let mut changed = text.clone();
    for (key, value) in changes {
        changed = match value {
            Some(value) => crate::config_edit::set(&changed, key, &crate::config_edit::Set::Text(value.to_string())),
            None => crate::config_edit::remove(&changed, key),
        }.map_err(|error| format!("{}: {error}", file.display()))?;
    }
    if changed == text {
        return Ok(Edited::Unchanged);
    }
    if base.is_some() && changed.trim().is_empty() {
        std::fs::remove_file(&file).map_err(|error| format!("{}: {error}", file.display()))?;
        return Ok(Edited::Removed(file));
    }
    parse(&changed, wanted, &file, base)?;
    let target = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
    crate::state::write(&target, changed)?;
    Ok(Edited::Wrote(file))
}

pub fn list_in(dir: &Path) -> (Vec<Profile>, Vec<String>) {
    let (found, broken) = load_all_in(dir);
    (found, broken.into_iter().map(|(_, _, error)| error).collect())
}

pub fn load_all_in(dir: &Path) -> (Vec<Profile>, Vec<(String, PathBuf, String)>) {
    let mut names = vec![TASK.to_string(), REVIEWER.to_string(), OPS.to_string()];
    let mut files: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|entry| entry.path().file_name()?.to_str()?.strip_suffix(".toml").map(str::to_string))
        .filter(|stem| !names.contains(stem)).collect();
    files.sort();
    names.extend(files);
    let (mut found, mut errors) = (Vec::new(), Vec::new());
    for name in names {
        match load_in(dir, &name) {
            Ok(profile) => found.push(profile),
            Err(error) => errors.push((name.clone(), dir.join(format!("{name}.toml")), error)),
        }
    }
    (found, errors)
}

// kebbie's own role a profile's lines are read after, in kebbie's install, which an update replaces.
pub fn kebbie_role(name: &str) -> (&'static str, &'static str) {
    role_of_kind(name == REVIEWER)
}

pub fn kebbie_role_for(profile: &Profile) -> (&'static str, &'static str) {
    role_of_kind(profile.review)
}

fn role_of_kind(review: bool) -> (&'static str, &'static str) {
    if review { ("kebbie's review role", "skills/review/SKILL.md") } else { ("kebbie's task role", "roles/task.md") }
}

pub fn role_name(dir: &Path, profile: &Profile) -> Option<String> {
    let role = profile.role.as_ref()?;
    Some(role.strip_prefix(dir).unwrap_or(role).to_string_lossy().to_string())
}

pub fn role_files(dir: &Path, profiles: &[Profile]) -> Vec<(String, Vec<String>)> {
    let mut files: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().to_str().filter(|name| name.ends_with(".md")).map(str::to_string)).collect();
    files.extend(profiles.iter().filter_map(|profile| role_name(dir, profile)).filter(|name| !files.contains(name)).collect::<Vec<_>>());
    files.sort();
    files.dedup();
    files.into_iter().map(|file| {
        let users = profiles.iter().filter(|profile| role_name(dir, profile).as_deref() == Some(file.as_str())).map(|profile| profile.name.clone()).collect();
        (file, users)
    }).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Role {
    File(String),
    New(String),
    Copy { from: String, to: String },
    None,
}

impl Role {
    pub fn file(&self) -> Option<&str> {
        match self {
            Role::New(file) | Role::Copy { to: file, .. } | Role::File(file) => Some(file),
            Role::None => None,
        }
    }
}

pub fn role_choices(dir: &Path, profiles: &[Profile], name: &str) -> Vec<(String, String)> {
    role_files(dir, profiles).into_iter().map(|(file, users)| { let about = role_users(&users, name); (file, about) }).collect()
}

pub fn role_users(users: &[String], name: &str) -> String {
    match users {
        [] => "used by none".to_string(),
        [only] if only == name => format!("only {name}"),
        _ => format!("shared with {}", users.join(", ")),
    }
}

// Every file in the folder that names the role, a profile that does not load among them, so a role is never taken from one.
pub fn role_readers(dir: &Path, role: &str) -> Vec<String> {
    let mut readers: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|entry| Some((entry.file_name().to_str()?.strip_suffix(".toml")?.to_string(), std::fs::read_to_string(entry.path()).ok()?)))
        .filter(|(_, text)| raw_role(text).is_some_and(|named| Path::new(&named).components().eq(Path::new(role).components())))
        .map(|(name, _)| name).collect();
    readers.sort();
    readers
}

fn raw_role(text: &str) -> Option<String> {
    if let Ok(table) = text.parse::<Table>() {
        return table.get("role").and_then(toml::Value::as_str).map(|role| role.trim().to_string());
    }
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "role").then(|| value.split('#').next().unwrap_or_default().trim().trim_matches(['"', '\'']).trim().to_string())
    })
}

// A role file named after the profile that no file holds yet, so a new role never overwrites one.
pub fn free_role(dir: &Path, name: &str) -> String {
    (1..).map(|number| if number == 1 { format!("{name}.md") } else { format!("{name}-{number}.md") }).find(|file| !dir.join(file).exists()).unwrap_or_default()
}

pub struct Made {
    pub text: String,
    pub dropped: Vec<&'static str>,
}

// A new profile's file from the one it starts from: its about, for, skills, permissions and sandbox, never its role; a Codex profile leaves out what only Claude Code reads.
pub fn made_from(start: &Profile, agent: Agent, model: Option<&str>, effort: Option<&str>, role: Option<&str>) -> Made {
    use crate::config_edit::Set;
    let text = |value: &str| Set::Text(value.to_string()).shown();
    let list = |items: &[String]| Set::List(items.to_vec()).shown();
    let codex = agent == Agent::Codex;
    let mut lines = Vec::new();
    let mut dropped = Vec::new();
    if !start.about.is_empty() {
        lines.push(format!("about = {}", text(&start.about)));
    }
    if !start.fits.is_empty() {
        lines.push(format!("for = {}", text(&start.fits)));
    }
    lines.push(format!("agent = {}", text(agent.key())));
    lines.extend(model.map(|model| format!("model = {}", text(model))));
    lines.extend(effort.map(|effort| format!("effort = {}", text(effort))));
    lines.extend(role.map(|role| format!("role = {}", text(role))));
    if !start.skills.is_empty() {
        lines.push(format!("skills = {}", list(&start.skills)));
    }
    lines.extend(start.permission_mode.as_deref().map(|mode| format!("permission_mode = {}", text(mode))));
    for (key, items) in [("deny", &start.deny), ("allow", &start.allow)] {
        match (items.is_empty(), codex) {
            (true, _) => {}
            (false, true) => dropped.push(key),
            (false, false) => lines.push(format!("{key} = {}", list(items))),
        }
    }
    match (start.plugins.is_empty(), codex) {
        (true, _) => {}
        (false, true) => dropped.push("claude.plugins"),
        (false, false) => lines.push(format!("\n[claude]\nplugins = {}", list(&start.plugins))),
    }
    let sandbox = &start.sandbox;
    let mut table: Vec<String> = sandbox.enabled.map(|on| format!("enabled = {on}")).into_iter().collect();
    for (key, items) in [("allowed_domains", sandbox.domains.as_deref()), ("allow_write", sandbox.writes.as_deref()), ("excluded_commands", Some(sandbox.excluded.as_slice()).filter(|items| !items.is_empty()))] {
        match (items, codex) {
            (None, _) => {}
            (Some(_), true) => dropped.push(match key { "allowed_domains" => "sandbox.allowed_domains", "allow_write" => "sandbox.allow_write", _ => "sandbox.excluded_commands" }),
            (Some(items), false) => table.push(format!("{key} = {}", list(items))),
        }
    }
    table.extend(sandbox.kubeconfig.as_deref().map(|kubeconfig| format!("kubeconfig = {}", text(kubeconfig))));
    if !table.is_empty() {
        lines.push(format!("\n[sandbox]\n{}", table.join("\n")));
    }
    Made { text: lines.join("\n") + "\n", dropped }
}

// A new review profile's file: what a reviewer takes, kind first so the parser reads the rest as a reviewer's.
pub fn made_review(start: &Profile, agent: Agent, model: Option<&str>, effort: Option<&str>, role: Option<&str>) -> Made {
    let text = |value: &str| crate::config_edit::Set::Text(value.to_string()).shown();
    let mut lines = vec![format!("kind = {}", text(REVIEW_KIND))];
    lines.extend((!start.about.is_empty()).then(|| format!("about = {}", text(&start.about))));
    lines.extend((!start.fits.is_empty()).then(|| format!("for = {}", text(&start.fits))));
    lines.push(format!("agent = {}", text(agent.key())));
    lines.extend(model.map(|model| format!("model = {}", text(model))));
    lines.extend(effort.map(|effort| format!("effort = {}", text(effort))));
    lines.extend(role.map(|role| format!("role = {}", text(role))));
    Made { text: lines.join("\n") + "\n", dropped: Vec::new() }
}

// What the task's role says of its profile, after kebbie's own role; the built-in task adds nothing.
pub fn role_section(profile: &Profile) -> Result<String> {
    let mut parts = Vec::new();
    if !profile.skills.is_empty() {
        let skills = profile.skills.iter().map(|skill| format!("`{skill}`")).collect::<Vec<_>>().join(", ");
        parts.push(format!("Once your worktree is set up, run these skills and follow them for this task: {skills}."));
    }
    if let Some(role) = &profile.role {
        let text = std::fs::read_to_string(role).map_err(|error| format!("{}: {error}", role.display()))?;
        parts.push(text.trim().to_string());
    }
    if parts.is_empty() {
        return Ok(String::new());
    }
    let about = if profile.about.is_empty() { String::new() } else { format!("{}.\n\n", profile.about.trim_end_matches('.')) };
    Ok(format!("\n\n## Your profile: {}\n\n{about}{}\n", profile.name, parts.join("\n\n")))
}

pub fn to_json(profile: &Profile) -> Value {
    json!({
        "name": profile.name, "about": profile.about, "for": profile.fits, "agent": profile.agent.key(), "model": profile.model, "effort": profile.effort,
        "permission_mode": profile.permission_mode, "skills": profile.skills, "role": profile.role, "deny": profile.deny, "allow": profile.allow,
        "plugins": profile.plugins, "built_in": built_in(&profile.name).is_some(), "file": profile.file, "kind": if profile.review { REVIEW_KIND } else { "task" },
        "sandbox": { "enabled": profile.sandbox.enabled, "allowed_domains": profile.sandbox.domains, "allow_write": profile.sandbox.writes, "excluded_commands": profile.sandbox.excluded, "kubeconfig": profile.sandbox.kubeconfig },
    })
}

// codex_model is the model Codex's own config.toml names, which a Codex profile without model runs.
pub fn describe(profiles: &[Profile], errors: &[String], chosen: &[Agent], codex: Option<&str>, codex_model: Option<&str>) -> String {
    let mut lines = Vec::new();
    for profile in profiles {
        let source = match (&profile.file, built_in(&profile.name).is_some()) {
            (Some(file), true) => format!("built in, with {}", file.display()),
            (Some(file), false) => file.display().to_string(),
            (None, _) => "built in".to_string(),
        };
        lines.push(format!("{}  ({source})", profile.name));
        lines.push(format!("  for: {}", if profile.fits.is_empty() { "(not said)" } else { &profile.fits }));
        if profile.review {
            lines.push("  kind: review, run by kebbie review (review.profile, review.second)".to_string());
        }
        if !profile.about.is_empty() {
            lines.push(format!("  about: {}", profile.about));
        }
        let mut runs = vec![profile.agent.key().to_string()];
        runs.extend(profile.model.iter().map(|model| format!("model {model}")));
        runs.extend(codex_model.filter(|_| profile.agent == Agent::Codex && profile.model.is_none()).map(|model| format!("model {model} from Codex's config.toml")));
        runs.extend(profile.effort.iter().map(|effort| format!("effort {effort}")));
        runs.extend(profile.permission_mode.iter().map(|mode| format!("mode {mode}")));
        runs.extend((!profile.skills.is_empty()).then(|| format!("skills {}", profile.skills.join(", "))));
        runs.extend((profile.sandbox.enabled == Some(false)).then(|| "sandbox off".to_string()));
        runs.extend((!profile.sandbox.excluded.is_empty()).then(|| format!("outside the sandbox: {}", profile.sandbox.excluded.join(", "))));
        runs.extend(profile.sandbox.kubeconfig.as_deref().map(|kubeconfig| format!("kubectl and helm with {kubeconfig}")));
        lines.push(format!("  runs: {}", runs.join(" · ")));
        if !chosen.contains(&profile.agent) {
            lines.push(format!("  ⚠ {}", profile.agent.not_set_up()));
        } else if let Some(codex) = codex.filter(|_| profile.agent == Agent::Codex) {
            lines.push(format!("  codex: {codex}"));
        }
    }
    lines.extend(errors.iter().map(|error| format!("ignored: {error}")));
    lines.join("\n")
}

pub fn print(json: bool) -> Result<String> {
    let (profiles, errors) = list_in(&dir());
    let codex = profiles.iter().any(|profile| profile.agent == Agent::Codex).then(codex_availability).flatten();
    let chosen = crate::agent::chosen();
    if json {
        let mut shown: Vec<Value> = profiles.iter().map(to_json).collect();
        for (item, profile) in shown.iter_mut().zip(&profiles) {
            item["set_up"] = json!(chosen.contains(&profile.agent));
        }
        for item in shown.iter_mut().filter(|item| item["agent"] == "codex") {
            item["codex"] = json!(codex);
        }
        shown.extend(errors.iter().map(|error| json!({ "error": error })));
        return serde_json::to_string_pretty(&shown).map_err(|error| error.to_string());
    }
    let model = crate::agent::codex::home().and_then(|home| crate::agent::codex::configured_model(&home));
    Ok(describe(&profiles, &errors, &chosen, codex.as_deref(), model.as_deref()))
}

fn codex_availability() -> Option<String> {
    let path: Vec<PathBuf> = std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default();
    let kebbie = crate::peers::kebbie_command().ok()?;
    Some(crate::agent::codex::availability(&path, crate::agent::codex::home().as_deref(), &kebbie).line())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = crate::sh::test_dir(name);
        for (file, text) in files {
            std::fs::write(dir.join(file), text).unwrap();
        }
        dir
    }

    const DESIGNER: &str = "about = \"UI work: mockup first\"\nfor = \"UI, CSS, new surfaces\"\nagent = \"claude\"\nmodel = \"opus\"\neffort = \"high\"\nrole = \"designer.md\"\nskills = [\"impeccable\", \"frontend-design:frontend-design\"]\npermission_mode = \"acceptEdits\"\ndeny = [\"Bash(rm -rf:*)\"]\nallow = [\"Bash(npx playwright:*)\"]\n\n[claude]\nplugins = [\"design@claude-plugins\"]\n";

    #[test]
    fn a_profile_file_loads_every_field_and_its_role_beside_it() {
        let dir = folder("profile-fields", &[("designer.toml", DESIGNER), ("designer.md", "Measure in the browser.\n")]);
        let profile = load_in(&dir, "designer").unwrap();
        assert_eq!((profile.about.as_str(), profile.fits.as_str(), profile.agent), ("UI work: mockup first", "UI, CSS, new surfaces", Agent::Claude));
        assert_eq!((profile.model.as_deref(), profile.effort.as_deref(), profile.permission_mode.as_deref()), (Some("opus"), Some("high"), Some("acceptEdits")));
        assert_eq!(profile.role, Some(dir.join("designer.md")));
        assert_eq!(profile.skills, ["impeccable", "frontend-design:frontend-design"]);
        assert_eq!((profile.deny, profile.allow, profile.plugins), (vec!["Bash(rm -rf:*)".to_string()], vec!["Bash(npx playwright:*)".to_string()], vec!["design@claude-plugins".to_string()]));
        assert_eq!(profile.file, Some(dir.join("designer.toml")));
        let section = role_section(&load_in(&dir, "designer").unwrap()).unwrap();
        assert!(section.starts_with("\n\n## Your profile: designer\n\nUI work: mockup first.\n\nOnce your worktree is set up, run these skills and follow them for this task: `impeccable`, `frontend-design:frontend-design`.\n\nMeasure in the browser.\n"), "{section:?}");
        assert!(load_in(&dir, "missing").unwrap_err().contains("No profile \"missing\""));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_profile_names_claude_or_codex_or_is_refused_with_its_file_and_key() {
        let dir = folder("profile-refused", &[]);
        let refused = |text: &str| { std::fs::write(dir.join("x.toml"), text).unwrap(); load_in(&dir, "x").unwrap_err() };
        assert!(refused("agent = \"gemini\"\n").contains("agent: \"gemini\" is not an agent kebbie runs"));
        assert!(refused("permission_mode = \"yolo\"\n").contains("x.toml: permission_mode: \"yolo\" is not a Claude Code permission mode"), "the same check as task.permission_mode");
        assert!(refused("effort = \"huge\"\n").contains("effort: \"huge\" is not an effort level; use \"low\", \"medium\", \"high\", \"xhigh\", \"max\""));
        assert!(refused("colour = \"red\"\n").contains("x.toml: colour: unknown key; known keys here: about, for, agent, model, effort, role, skills, permission_mode, deny, allow, claude"));
        assert!(refused("role = \"gone.md\"\n").contains("role: ") && refused("role = \"gone.md\"\n").contains("is not a file"));
        std::fs::write(dir.join("outside.md"), "x").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for escaping in ["/etc/passwd", "../outside.md", "sub/../../outside.md"] {
            assert!(refused(&format!("role = \"{escaping}\"\n")).contains("leaves the profiles folder"), "{escaping}");
        }
        std::os::unix::fs::symlink(dir.join("outside.md"), dir.join("linked.md")).unwrap();
        assert!(refused("role = \"linked.md\"\n").contains("is a symlink"));
        assert!(refused("skills = [\"../outside\"]\n").contains("skills: \"../outside\" is not a skill name"));
        assert!(refused("[claude]\nplugins = [\"design\"]\n").contains("claude.plugins: \"design\" is not a plugin"));
        assert!(refused("[claude]\nmcp = true\n").contains("claude.mcp: unknown key"));
        assert!(refused("deny = \"Bash(rm:*)\"\n").contains("deny: must be a list of strings"));
        assert!(load_in(&dir, "Bad Name").unwrap_err().contains("is not a profile name"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_allow_that_would_release_a_guarded_kebbie_command_is_refused_when_the_profile_loads() {
        for released in ["Bash(kebbie push:*)", "Bash(/home/ada/.local/share/kebbie/current/target/release/kebbie approve:*)", "Bash(kebbie choose:*)", "Bash(kebbie away on)", "Bash(kebbie decide:*)", "Bash(kebbie after:*)", "Bash(kebbie:*)", "Bash(kebbie *)", "Bash(kebbie -C /x push:*)", "Bash", "Bash(*)", "Bash(:*)", "Bash(*push:*)"] {
            assert!(releases_guard(released), "{released}");
        }
        for released in ["Bash(kebbie --repo app push:*)", "Bash(kebbie --no-focus approve:*)", "Bash(kebbie -x tell:*)"] {
            assert!(releases_guard(released), "a flag kebbie's guard cannot place hides the command after it: {released}");
        }
        for kept in ["Bash(kebbie -C /src/app tell:*)", "Bash(kebbie tell:*)", "Bash(kebbie ask:*)", "Bash(npx playwright:*)", "Read", "WebFetch(domain:example.com)", "Bash(git log:*)"] {
            assert!(!releases_guard(kept), "{kept}");
        }
        let dir = folder("profile-allow", &[("x.toml", "allow = [\"Bash(npm test:*)\", \"Bash(kebbie push:*)\"]\n")]);
        let refused = load_in(&dir, "x").unwrap_err();
        assert!(refused.contains("x.toml: allow: \"Bash(kebbie push:*)\" would let the task run kebbie push, try, stage, unstage, approve, choose, away, decide, after"), "{refused}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_task_takes_its_profile_from_start_then_its_session_then_the_config_then_task() {
        assert_eq!(chosen(None, None, None), TASK);
        assert_eq!(chosen(None, None, Some("designer")), "designer", "task.profile");
        assert_eq!(chosen(None, Some("billing"), Some("designer")), "billing", "a restart keeps the profile it started with");
        assert_eq!(chosen(Some("quick"), Some("billing"), Some("designer")), "quick", "--profile wins");
        let dir = folder("profile-for-task", &[]);
        assert_eq!(for_task(&dir, TASK).unwrap(), built_in(TASK).unwrap(), "the built-in task needs no file");
        assert!(for_task(&dir, REVIEWER).unwrap_err().contains("kebbie review's profile, not a task's"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_built_in_task_adds_nothing_to_kebbies_role() {
        let task = built_in(TASK).unwrap();
        assert_eq!(role_section(&task).unwrap(), "");
        assert_eq!((task.model, task.effort, task.permission_mode, task.agent), (None, None, None, Agent::Claude));
        assert!(task.deny.is_empty() && task.allow.is_empty() && task.plugins.is_empty() && task.skills.is_empty());
    }

    #[test]
    fn the_reviewers_profile_takes_a_model_and_a_role_and_never_a_permission() {
        let dir = folder("profile-reviewer", &[]);
        assert_eq!(load_in(&dir, REVIEWER).unwrap(), built_in(REVIEWER).unwrap());
        std::fs::write(dir.join("reviewer.toml"), "model = \"opus\"\neffort = \"max\"\n").unwrap();
        let reviewer = load_in(&dir, REVIEWER).unwrap();
        assert_eq!((reviewer.model.as_deref(), reviewer.effort.as_deref(), reviewer.about.as_str()), (Some("opus"), Some("max"), "The read-only reviewer that kebbie review runs"));
        for key in ["permission_mode = \"bypassPermissions\"\n", "allow = [\"Edit\"]\n", "skills = [\"x\"]\n"] {
            std::fs::write(dir.join("reviewer.toml"), key).unwrap();
            assert!(load_in(&dir, REVIEWER).unwrap_err().contains("the reviewer only reads, so its profile takes about, for, agent, model, effort, role"), "{key}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_review_profile_takes_only_a_reviewers_keys_and_is_never_a_tasks() {
        let dir = folder("profile-review", &[("codex-review.toml", "kind = \"review\"\nagent = \"codex\"\nmodel = \"gpt-6.1-sol\"\neffort = \"high\"\n"), ("designer.toml", "model = \"opus\"\n")]);
        let codex = load_in(&dir, "codex-review").unwrap();
        assert!(codex.review && codex.agent == Agent::Codex && built_in(REVIEWER).unwrap().review && !built_in(TASK).unwrap().review);
        assert_eq!(to_json(&codex)["kind"], "review");
        assert!(describe(std::slice::from_ref(&codex), &[], &Agent::ALL, None, None).contains("  kind: review, run by kebbie review (review.profile, review.second)"));
        for key in ["skills = [\"x\"]\n", "permission_mode = \"plan\"\n", "allow = [\"Read\"]\n", "[sandbox]\nenabled = false\n"] {
            std::fs::write(dir.join("x.toml"), format!("kind = \"review\"\n{key}")).unwrap();
            assert!(load_in(&dir, "x").unwrap_err().contains("the reviewer only reads, so its profile takes about, for, agent, model, effort, role, kind"), "{key}");
        }
        std::fs::write(dir.join("x.toml"), "kind = \"reviewer\"\n").unwrap();
        assert!(load_in(&dir, "x").unwrap_err().contains("kind: \"reviewer\" is not a kind; use \"task\" or \"review\""));
        std::fs::write(dir.join("task.toml"), "kind = \"review\"\n").unwrap();
        assert!(load_in(&dir, TASK).unwrap_err().contains("kind: \"task\" is kebbie's own task profile"), "a built-in keeps its kind");
        std::fs::remove_file(dir.join("task.toml")).unwrap();
        assert!(for_task(&dir, "codex-review").unwrap_err().contains("\"codex-review\" is a review profile, which kebbie review runs, not a task's"));
        assert_eq!(for_review(&dir, "codex-review").unwrap(), codex);
        assert_eq!(for_review(&dir, REVIEWER).unwrap(), built_in(REVIEWER).unwrap());
        assert!(for_review(&dir, "designer").unwrap_err().contains("\"designer\" is a task's profile"));
        let made = made_review(&built_in(REVIEWER).unwrap(), Agent::Claude, Some("sonnet"), None, None);
        assert_eq!(made.text, "kind = \"review\"\nabout = \"The read-only reviewer that kebbie review runs\"\nfor = \"kebbie review, never a task\"\nagent = \"claude\"\nmodel = \"sonnet\"\n");
        assert!(parse(&made.text, "made", &dir.join("made.toml"), None).unwrap().review);
        assert_eq!((kebbie_role_for(&codex).1, kebbie_role_for(&load_in(&dir, "designer").unwrap()).1), ("skills/review/SKILL.md", "roles/task.md"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn kebbie_profiles_lists_the_built_in_ones_then_each_file_and_names_a_broken_one() {
        let dir = folder("profile-list", &[("designer.toml", "about = \"UI work\"\nfor = \"UI, CSS\"\nmodel = \"opus\"\n"), ("broken.toml", "agent = \"gemini\"\n"), ("notes.txt", "x")]);
        let (profiles, errors) = list_in(&dir);
        assert_eq!(profiles.iter().map(|profile| profile.name.as_str()).collect::<Vec<_>>(), ["task", "reviewer", "ops", "designer"]);
        assert_eq!(errors.len(), 1);
        let shown = describe(&profiles, &errors, &Agent::ALL, Some("codex-cli 0.163.0 at /bin/codex · kebbie's plugin installed"), None);
        assert!(shown.starts_with("task  (built in)\n  for: any task no other profile fits\n"), "{shown}");
        assert!(shown.contains(&format!("designer  ({})\n  for: UI, CSS\n  about: UI work\n  runs: claude · model opus", dir.join("designer.toml").display())), "{shown}");
        assert!(shown.contains("ignored: ") && shown.contains("broken.toml: agent: \"gemini\" is not an agent kebbie runs"), "{shown}");
        assert!(!shown.contains("  codex: "), "Codex's line shows only under a profile that runs it: {shown}");
        assert!(shown.contains("ops  (built in)\n  for: work on other machines over ssh\n") && shown.contains("outside the sandbox: ssh *, scp *, rsync *"), "{shown}");
        let json = to_json(&profiles[3]);
        assert_eq!((json["name"].as_str(), json["for"].as_str(), json["about"].as_str(), json["agent"].as_str(), json["built_in"].as_bool()), (Some("designer"), Some("UI, CSS"), Some("UI work"), Some("claude"), Some(false)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_lead_chooses_a_tasks_profile_by_what_each_is_for() {
        let lead = include_str!("../roles/lead.md");
        assert!(lead.contains("`kebbie profiles` lists the profiles") && lead.contains("`for`") && lead.contains("`--profile <name>`"), "the lead knows the profiles and passes one");
    }

    #[test]
    fn the_example_profiles_load_as_they_ship() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/profiles");
        let designer = load_in(&examples, "designer").unwrap();
        assert_eq!((designer.model.as_deref(), designer.permission_mode.as_deref(), designer.role.is_some()), (Some("opus"), Some("acceptEdits"), true));
        let careful = load_in(&examples, "careful").unwrap();
        assert_eq!((careful.effort.as_deref(), careful.permission_mode.as_deref()), (Some("max"), Some("plan")));
    }

    #[test]
    fn a_running_task_whose_recorded_profile_no_longer_loads_goes_on_as_task_and_says_why() {
        let dir = folder("profile-running", &[("designer.toml", "model = \"opus\"\n")]);
        assert_eq!(for_running_task(&dir, None, Some("designer"), None).unwrap().1, None);
        std::fs::write(dir.join("designer.toml"), "agent = \"gemini\"\n").unwrap();
        let (profile, said) = for_running_task(&dir, None, Some("designer"), None).unwrap();
        assert_eq!(profile, built_in(TASK).unwrap(), "work keeps going");
        let said = said.unwrap();
        assert!(said.contains("its profile \"designer\" did not load") && said.contains("designer.toml: agent: \"gemini\" is not an agent kebbie runs"), "{said}");
        std::fs::remove_file(dir.join("designer.toml")).unwrap();
        assert!(for_running_task(&dir, None, Some("designer"), None).unwrap().1.unwrap().contains("No profile \"designer\""));
        assert!(for_running_task(&dir, Some("designer"), Some("task"), None).is_err(), "a profile asked for by name is refused, not swapped");
        assert!(for_running_task(&dir, None, None, Some("designer")).is_err(), "a new start never falls back");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_codex_task_whose_profile_no_longer_loads_is_not_restarted_on_the_built_in_claude_role() {
        let dir = folder("profile-codex-restart", &[("coder.toml", "agent = \"codex\"\nmodel = \"gpt-5.5\"\n"), ("writer.toml", "about = \"Claude now\"\n")]);
        assert_eq!(codex_restart_refusal(&dir, Some("coder"), None), None);
        let gone = codex_restart_refusal(&dir, Some("designer"), None).unwrap();
        assert!(gone.starts_with("its profile \"designer\" does not load (") && gone.contains("No profile \"designer\"") && gone.ends_with("so its Codex session is not resumed on the built-in Claude role"), "{gone}");
        assert_eq!(codex_restart_refusal(&dir, Some("writer"), None).as_deref(), Some("its profile \"writer\" no longer runs Codex, so its Codex session is not resumed on a Claude role"));
        std::fs::write(dir.join("coder.toml"), "agent = \"codex\"\ndeny = [\"Bash(rm:*)\"]\n").unwrap();
        assert!(codex_restart_refusal(&dir, Some("coder"), None).unwrap().contains("does not load"), "a profile broken since the start is refused too");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_profile_that_says_codex_starts_codex_with_its_model_and_effort_and_no_claude_only_keys() {
        let dir = folder("profile-codex", &[("coder.toml", "about = \"Codex on GPT\"\nfor = \"a second opinion\"\nagent = \"codex\"\nmodel = \"gpt-5.5\"\neffort = \"xhigh\"\npermission_mode = \"acceptEdits\"\n[sandbox]\nenabled = true\n")]);
        let coder = load_in(&dir, "coder").unwrap();
        assert_eq!((coder.agent, coder.model.as_deref(), coder.effort.as_deref(), coder.sandbox.enabled), (Agent::Codex, Some("gpt-5.5"), Some("xhigh"), Some(true)));
        let shown = describe(&[built_in(TASK).unwrap(), coder.clone()], &[], &Agent::ALL, Some("codex-cli 0.163.0 at /bin/codex · kebbie's plugin installed"), Some("gpt-5.1-codex-max"));
        assert!(shown.contains("coder  (") && shown.contains("  runs: codex · model gpt-5.5 · effort xhigh · mode acceptEdits\n  codex: codex-cli 0.163.0 at /bin/codex · kebbie's plugin installed"), "{shown}");
        let unset = Profile { model: None, ..coder.clone() };
        assert!(describe(&[unset], &[], &Agent::ALL, None, Some("gpt-5.1-codex-max")).contains("  runs: codex · model gpt-5.1-codex-max from Codex's config.toml · effort xhigh"), "a profile without model names the one Codex runs");
        assert!(shown.starts_with("task  (built in)\n") && shown.contains("nothing added\n  runs: claude\ncoder"), "the built-in profiles stay Claude: {shown}");
        let unchosen = describe(std::slice::from_ref(&coder), &[], &[Agent::Claude], Some("codex-cli 0.163.0 at /bin/codex · kebbie's plugin not installed: kebbie setup codex"), None);
        assert!(unchosen.ends_with("  runs: codex · model gpt-5.5 · effort xhigh · mode acceptEdits\n  ⚠ Codex is not set up: kebbie setup codex"), "a Codex profile on a machine that did not choose Codex is marked: {unchosen}");
        assert_eq!(to_json(&coder)["agent"], "codex");
        let refused = |text: &str| { std::fs::write(dir.join("x.toml"), format!("agent = \"codex\"\n{text}")).unwrap(); load_in(&dir, "x").unwrap_err() };
        for (text, key) in [("deny = [\"Bash(rm:*)\"]\n", "deny"), ("allow = [\"Read\"]\n", "allow"), ("[claude]\nplugins = [\"a@b\"]\n", "claude"), ("[sandbox]\nallowed_domains = [\"x.org\"]\n", "sandbox.allowed_domains")] {
            assert!(refused(text).contains(&format!("x.toml: {key}: Claude Code's, which a Codex task does not read")), "{key}: {}", refused(text));
        }
        for level in CODEX_EFFORTS {
            std::fs::write(dir.join("x.toml"), format!("agent = \"codex\"\neffort = \"{level}\"\n")).unwrap();
            assert_eq!(load_in(&dir, "x").unwrap().effort.as_deref(), Some(level), "a level Codex's ReasoningEffort names");
        }
        assert!(refused("effort = \"huge\"\n").contains("x.toml: effort: \"huge\" is not an effort level Codex takes; use \"none\", \"minimal\", \"low\", \"medium\", \"high\", \"xhigh\", \"max\", \"ultra\", \"persistent\""));
        std::fs::write(dir.join("x.toml"), "effort = \"ultra\"\n").unwrap();
        assert!(load_in(&dir, "x").unwrap_err().contains("effort: \"ultra\" is not an effort level; use \"low\""), "a Claude profile keeps Claude Code's five");
        std::fs::write(dir.join("reviewer.toml"), "agent = \"codex\"\n").unwrap();
        assert!(load_in(&dir, REVIEWER).unwrap_err().contains("agent: the built-in reviewer runs Claude"), "the built-in reviewer stays Claude, for the assessor and the rules check read with it");
        let ops = { std::fs::write(dir.join("ops.toml"), "agent = \"codex\"\n").unwrap(); load_in(&dir, OPS).unwrap() };
        assert_eq!(ops.agent, Agent::Codex, "a file may move a built-in profile to Codex; only its own keys are checked");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_edit_changes_only_its_key_keeping_every_comment_and_a_built_ins_file_goes_when_empty() {
        let dir = folder("profile-edit", &[("coder.toml", "# Codex for building\nabout = \"Codex\" # short\nagent = \"codex\"\nmodel = \"gpt-5.5\" # was 5.1\n\n[sandbox]\nenabled = true\n")]);
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
        assert_eq!(edit_in(&dir, "coder", &[("model", Some("gpt-6.1-sol"))]).unwrap(), Edited::Wrote(dir.join("coder.toml")));
        assert_eq!(read("coder.toml"), "# Codex for building\nabout = \"Codex\" # short\nagent = \"codex\"\nmodel = \"gpt-6.1-sol\" # was 5.1\n\n[sandbox]\nenabled = true\n");
        edit_in(&dir, "coder", &[("effort", Some("ultra"))]).unwrap();
        assert_eq!(read("coder.toml"), "# Codex for building\nabout = \"Codex\" # short\nagent = \"codex\"\nmodel = \"gpt-6.1-sol\" # was 5.1\neffort = \"ultra\"\n\n[sandbox]\nenabled = true\n", "a new key joins the top of the file, before its tables");
        assert_eq!(load_in(&dir, "coder").unwrap().effort.as_deref(), Some("ultra"));
        let before = read("coder.toml");
        assert!(edit_in(&dir, "coder", &[("effort", Some("huge"))]).unwrap_err().contains("effort: \"huge\" is not an effort level Codex takes"));
        assert!(edit_in(&dir, "coder", &[("model", Some(""))]).unwrap_err().contains("model: must not be empty"));
        assert_eq!(read("coder.toml"), before, "a value the parser refuses is never written");
        assert_eq!(edit_in(&dir, "coder", &[("model", None), ("effort", None)]).unwrap(), Edited::Wrote(dir.join("coder.toml")));
        assert_eq!(read("coder.toml"), "# Codex for building\nabout = \"Codex\" # short\nagent = \"codex\"\n\n[sandbox]\nenabled = true\n", "a file profile is kept without them");
        assert_eq!(edit_in(&dir, "coder", &[("model", None)]).unwrap(), Edited::Unchanged);
        assert_eq!(edit_in(&dir, TASK, &[("model", Some("sonnet"))]).unwrap(), Edited::Wrote(dir.join("task.toml")));
        assert_eq!(read("task.toml"), "model = \"sonnet\"\n", "a built-in profile gets a file with that key alone");
        assert_eq!(load_in(&dir, TASK).unwrap(), Profile { model: Some("sonnet".to_string()), file: Some(dir.join("task.toml")), ..built_in(TASK).unwrap() }, "the file adds to the built-in, its role and rules as they ship");
        assert_eq!(edit_in(&dir, TASK, &[("model", None), ("effort", None)]).unwrap(), Edited::Removed(dir.join("task.toml")));
        assert!(!dir.join("task.toml").exists());
        std::fs::write(dir.join("ops.toml"), "# mine\nfor = \"the cluster\"\nmodel = \"opus\"\n").unwrap();
        edit_in(&dir, OPS, &[("model", None), ("effort", None)]).unwrap();
        assert_eq!(read("ops.toml"), "# mine\nfor = \"the cluster\"\n", "a file that holds something else stays");
        assert!(edit_in(&dir, "missing", &[("model", Some("opus"))]).unwrap_err().contains("No profile \"missing\""));
        assert!(!dir.join("missing.toml").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn role_readers_finds_every_file_that_names_a_role_one_that_does_not_load_among_them() {
        let dir = folder("profile-role-readers", &[("codex.md", "x"), ("a.toml", "role = \"codex.md\" # mine\n"), ("broken.toml", "agent = \"gemini\"\nrole = \"codex.md\"\n"), ("garbled.toml", "role = 'codex.md'\n[[[\n"), ("other.toml", "role = \"other.md\"\n")]);
        assert!(list_in(&dir).0.iter().all(|profile| profile.name != "broken"), "broken does not load");
        assert_eq!(role_readers(&dir, "codex.md"), ["a", "broken", "garbled"]);
        assert_eq!((role_users(&[], "a"), role_users(&["a".to_string()], "a"), role_users(&["a".to_string(), "b".to_string()], "a")), ("used by none".to_string(), "only a".to_string(), "shared with a, b".to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn role_files_lists_each_md_file_with_the_profiles_that_read_it_and_a_new_one_takes_a_free_name() {
        let dir = folder("profile-roles", &[("codex.md", "x"), ("spare.md", ""), ("a.toml", "role = \"codex.md\"\n"), ("b.toml", "role = \"codex.md\"\n"), ("notes.txt", "")]);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/deep.md"), "").unwrap();
        std::fs::write(dir.join("c.toml"), "role = \"sub/deep.md\"\n").unwrap();
        let (profiles, _) = list_in(&dir);
        assert_eq!(role_files(&dir, &profiles), [("codex.md".to_string(), vec!["a".to_string(), "b".to_string()]), ("spare.md".to_string(), vec![]), ("sub/deep.md".to_string(), vec!["c".to_string()])]);
        assert_eq!(role_name(&dir, &load_in(&dir, "c").unwrap()).as_deref(), Some("sub/deep.md"));
        assert_eq!((free_role(&dir, "fresh"), free_role(&dir, "codex")), ("fresh.md".to_string(), "codex-2.md".to_string()));
        for name in [TASK, OPS, REVIEWER] {
            assert!(Path::new(env!("CARGO_MANIFEST_DIR")).join(kebbie_role(name).1).is_file(), "kebbie's own role for {name} ships in its install");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_new_profile_takes_its_starts_rules_and_never_its_role_and_a_codex_one_leaves_out_claudes_keys() {
        let dir = folder("profile-made", &[("designer.toml", DESIGNER), ("designer.md", "Measure.\n"), ("fresh.md", "")]);
        let designer = load_in(&dir, "designer").unwrap();
        let made = made_from(&designer, Agent::Claude, Some("sonnet"), None, Some("fresh.md"));
        assert_eq!(made.text, "about = \"UI work: mockup first\"\nfor = \"UI, CSS, new surfaces\"\nagent = \"claude\"\nmodel = \"sonnet\"\nrole = \"fresh.md\"\nskills = [\"impeccable\", \"frontend-design:frontend-design\"]\npermission_mode = \"acceptEdits\"\ndeny = [\"Bash(rm -rf:*)\"]\nallow = [\"Bash(npx playwright:*)\"]\n\n[claude]\nplugins = [\"design@claude-plugins\"]\n");
        assert!(made.dropped.is_empty());
        let parsed = parse(&made.text, "made", &dir.join("made.toml"), None).unwrap();
        assert_eq!((parsed.skills, parsed.allow, parsed.role), (designer.skills.clone(), designer.allow.clone(), Some(dir.join("fresh.md"))));
        let codex = made_from(&designer, Agent::Codex, None, Some("ultra"), None);
        assert_eq!(codex.dropped, ["deny", "allow", "claude.plugins"]);
        assert!(!codex.text.contains("role") && codex.text.contains("agent = \"codex\"\neffort = \"ultra\"\n"), "{}", codex.text);
        assert!(parse(&codex.text, "made", &dir.join("made.toml"), None).is_ok(), "{}", codex.text);
        let ops = made_from(&built_in(OPS).unwrap(), Agent::Claude, None, None, None);
        assert!(ops.text.ends_with("\n[sandbox]\nexcluded_commands = [\"ssh *\", \"scp *\", \"rsync *\"]\n"), "ops's commands outside the sandbox come along: {}", ops.text);
        assert_eq!(parse(&ops.text, "made", &dir.join("made.toml"), None).unwrap().sandbox.excluded, built_in(OPS).unwrap().sandbox.excluded);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_profile_turns_the_sandbox_off_or_replaces_its_hosts_and_folders_and_adds_commands_outside_it() {
        let dir = folder("profile-sandbox", &[
            ("loose.toml", "[sandbox]\nenabled = false\n"),
            ("web.toml", "[sandbox]\nallowed_domains = [\"registry.yarnpkg.com\"]\nallow_write = [\"~/.gradle\"]\nexcluded_commands = [\"docker compose *\"]\n"),
            ("ops.toml", "model = \"opus\"\n[sandbox]\nexcluded_commands = [\"ansible-playbook *\", \"ssh *\"]\n"),
        ]);
        assert_eq!(load_in(&dir, "loose").unwrap().sandbox.enabled, Some(false));
        let web = load_in(&dir, "web").unwrap().sandbox;
        assert_eq!((web.enabled, web.domains(), web.writes(), web.excluded), (None, vec!["registry.yarnpkg.com".to_string()], vec!["~/.gradle".to_string()], vec!["docker compose *".to_string()]));
        assert_eq!(built_in(TASK).unwrap().sandbox.domains(), crate::sandbox::DOMAINS.map(str::to_string).to_vec(), "kebbie's list until a profile names its own");
        let ops = load_in(&dir, OPS).unwrap();
        assert_eq!((ops.model.as_deref(), ops.sandbox.excluded), (Some("opus"), vec!["ssh *".to_string(), "scp *".to_string(), "rsync *".to_string(), "ansible-playbook *".to_string()]), "a file adds to the built-in ops");
        let refused = |text: &str| { std::fs::write(dir.join("x.toml"), text).unwrap(); load_in(&dir, "x").unwrap_err() };
        assert!(refused("[sandbox]\nexcluded_commands = [\"tmux *\"]\n").contains("x.toml: sandbox.excluded_commands: \"tmux *\" would run tmux outside the sandbox"));
        assert!(refused("[sandbox]\nexcluded_commands = [\"kebbie push *\"]\n").contains("would run kebbie outside the sandbox"));
        assert!(refused("[sandbox]\nallow_write = [\"~/.config\"]\n").contains("sandbox.allow_write: \"~/.config\" would let a task's shell write"));
        assert!(refused("[sandbox]\nenabled = \"no\"\n").contains("sandbox.enabled: must be true or false"));
        assert!(refused("[sandbox]\nnetwork = true\n").contains("sandbox.network: unknown key"));
        std::fs::write(dir.join("reviewer.toml"), "[sandbox]\nenabled = false\n").unwrap();
        assert!(load_in(&dir, REVIEWER).unwrap_err().contains("the reviewer only reads"), "the reviewer's sandbox follows task.sandbox alone");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_a_profile_in_the_machines_own_profiles_folder_names_the_kubeconfig_its_tasks_reach_a_cluster_with() {
        let dir = folder("profile-kubeconfig", &[("ops.toml", "[sandbox]\nkubeconfig = \"~/.kube/kebbie-ops.yaml\"\n"), ("deploy.toml", "agent = \"codex\"\n[sandbox]\nkubeconfig = \"/etc/kebbie/deploy.yaml\"\n")]);
        for name in [TASK, REVIEWER, OPS] {
            assert_eq!(built_in(name).unwrap().sandbox.kubeconfig, None, "{name}: no built-in profile reaches a cluster");
        }
        let ops = load_in(&dir, OPS).unwrap();
        let home = std::env::var("HOME").unwrap();
        assert_eq!((ops.sandbox.kubeconfig.as_deref(), ops.sandbox.kubeconfig_file()), (Some("~/.kube/kebbie-ops.yaml"), Some(Path::new(&home).join(".kube/kebbie-ops.yaml"))));
        assert_eq!(ops.sandbox.excluded, built_in(OPS).unwrap().sandbox.excluded, "the kubeconfig adds no entry of the profile's own");
        assert!(describe(std::slice::from_ref(&ops), &[], &Agent::ALL, None, None).contains("kubectl and helm with ~/.kube/kebbie-ops.yaml") && to_json(&ops)["sandbox"]["kubeconfig"] == "~/.kube/kebbie-ops.yaml");
        assert_eq!(parse(&made_from(&ops, Agent::Claude, None, None, None).text, "made", &dir.join("made.toml"), None).unwrap().sandbox.kubeconfig, ops.sandbox.kubeconfig, "a copy keeps the login");
        assert_eq!(load_in(&dir, "deploy").unwrap().sandbox.kubeconfig_file(), Some(PathBuf::from("/etc/kebbie/deploy.yaml")), "a Codex profile takes it too");
        let refused = |text: &str| { std::fs::write(dir.join("x.toml"), text).unwrap(); load_in(&dir, "x").unwrap_err() };
        assert!(refused("[sandbox]\nkubeconfig = \"kube.yaml\"\n").contains("x.toml: sandbox.kubeconfig: \"kube.yaml\" is not a path from / or ~/"));
        assert!(refused("[sandbox]\nkubeconfig = 1\n").contains("x.toml: sandbox.kubeconfig: must be a string"));
        for kube in ["kubectl *", "/usr/local/bin/helm *"] {
            assert!(refused(&format!("[sandbox]\nexcluded_commands = [\"{kube}\"]\n")).contains("outside the sandbox with your own cluster logins; set sandbox.kubeconfig"), "{kube}");
        }
        let team = crate::sh::test_dir("profile-kubeconfig-team");
        std::fs::write(team.join("cluster.toml"), "[sandbox]\nkubeconfig = \"~/.kube/kebbie-ops.yaml\"\n").unwrap();
        std::os::unix::fs::symlink(team.join("cluster.toml"), dir.join("cluster.toml")).unwrap();
        assert!(load_in(&dir, "cluster").unwrap_err().contains("sandbox.kubeconfig: the profile file is a symlink"), "a repo's own file never picks the login");
        std::fs::write(team.join("cluster.toml"), "model = \"opus\"\n").unwrap();
        assert!(load_in(&dir, "cluster").is_ok(), "a linked profile without a kubeconfig loads as before");
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(team).unwrap();
    }
}
