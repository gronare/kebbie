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
pub const KEYS: [&str; 12] = ["about", "for", "agent", "model", "effort", "role", "skills", "permission_mode", "deny", "allow", "claude", "sandbox"];
const SANDBOX_KEYS: [&str; 4] = ["enabled", "allowed_domains", "allow_write", "excluded_commands"];
const REVIEWER_KEYS: [&str; 6] = ["about", "for", "agent", "model", "effort", "role"];
const GUARDED: [&str; 6] = ["push", "approve", "choose", "away", "decide", "after"];

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
}

pub fn dir() -> PathBuf {
    std::env::var("KELPIE_PROFILES_DIR").map(PathBuf::from).unwrap_or_else(|_| config::dir().join("profiles"))
}

pub fn built_in(name: &str) -> Option<Profile> {
    let (about, fits) = match name {
        TASK => ("kelpie's own task: its role, hooks and permissions, nothing added", "any task no other profile fits"),
        REVIEWER => ("The read-only reviewer that kelpie review runs", "kelpie review, never a task"),
        OPS => ("kelpie's own task whose ssh, scp and rsync run outside the sandbox, so they reach other machines, and this one as you", "work on other machines over ssh"),
        _ => return None,
    };
    let sandbox = if name == OPS { Policy { excluded: crate::sandbox::OPS_COMMANDS.map(str::to_string).to_vec(), ..Policy::default() } } else { Policy::default() };
    Some(Profile {
        name: name.to_string(), about: about.to_string(), fits: fits.to_string(), agent: Agent::Claude, model: None, effort: None, role: None,
        skills: Vec::new(), permission_mode: None, deny: Vec::new(), allow: Vec::new(), plugins: Vec::new(), sandbox, file: None,
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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => base.ok_or_else(|| format!("No profile \"{wanted}\": {} is not there; kelpie profiles lists them", file.display())),
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

pub fn for_task(dir: &Path, wanted: &str) -> Result<Profile> {
    if wanted == REVIEWER {
        return Err(format!("\"{REVIEWER}\" is kelpie review's profile, not a task's; kelpie profiles lists the others"));
    }
    load_in(dir, wanted)
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

// Claude Code lets a deny rule win over any allow, but kelpie away and decide are guarded by who calls them, so a rule naming one is refused outright.
pub fn releases_guard(rule: &str) -> bool {
    let Some(inner) = rule.trim().strip_prefix("Bash") else { return false };
    let inner = inner.trim();
    if inner.is_empty() {
        return true;
    }
    let Some(command) = inner.strip_prefix('(').and_then(|rest| rest.strip_suffix(')')) else { return false };
    let words: Vec<&str> = command.split(|character: char| character.is_whitespace() || character == ':').filter(|word| !word.is_empty()).collect();
    let Some((program, rest)) = words.split_first() else { return true };
    if program.rsplit('/').next() != Some("kelpie") && !program.contains('*') {
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

pub fn parse(contents: &str, name: &str, file: &Path, base: Option<Profile>) -> Result<Profile> {
    let source = file.display().to_string();
    let table: Table = contents.parse().map_err(|error: toml::de::Error| format!("{source}: {}", error.to_string().trim()))?;
    let reviewer = name == REVIEWER;
    let known: &[&str] = if reviewer { &REVIEWER_KEYS } else { &KEYS };
    if reviewer && let Some(key) = table.keys().find(|key| !known.contains(&key.as_str()) && KEYS.contains(&key.as_str())) {
        return Err(format!("{source}: {key}: the reviewer only reads, so its profile takes {}", REVIEWER_KEYS.join(", ")));
    }
    config::unknown_keys(&table, "", known).map_err(|error| format!("{source}: {error}"))?;
    let mut profile = base.unwrap_or(Profile {
        name: name.to_string(), about: String::new(), fits: String::new(), agent: Agent::Claude, model: None, effort: None, role: None,
        skills: Vec::new(), permission_mode: None, deny: Vec::new(), allow: Vec::new(), plugins: Vec::new(), sandbox: Policy::default(), file: None,
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
    profile.model = text(&table, "model", &source)?.or(profile.model);
    if let Some(effort) = text(&table, "effort", &source)? {
        if !EFFORTS.contains(&effort.as_str()) {
            return Err(format!("{source}: effort: \"{effort}\" is not an effort level; use {}", EFFORTS.map(|level| format!("\"{level}\"")).join(", ")));
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
        return Err(format!("{source}: allow: \"{rule}\" would let the task run kelpie {}, which stay the user's and the lead's; name the commands it may run instead", GUARDED.join(", ")));
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
    Ok(profile)
}

// A profile's lists of hosts and writable folders replace kelpie's own; its commands outside the sandbox add to the base's.
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
    Ok(())
}

pub fn list_in(dir: &Path) -> (Vec<Profile>, Vec<String>) {
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
            Err(error) => errors.push(error),
        }
    }
    (found, errors)
}

// What the task's role says of its profile, after kelpie's own role; the built-in task adds nothing.
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
        "plugins": profile.plugins, "built_in": built_in(&profile.name).is_some(), "file": profile.file,
        "sandbox": { "enabled": profile.sandbox.enabled, "allowed_domains": profile.sandbox.domains, "allow_write": profile.sandbox.writes, "excluded_commands": profile.sandbox.excluded },
    })
}

pub fn describe(profiles: &[Profile], errors: &[String]) -> String {
    let mut lines = Vec::new();
    for profile in profiles {
        let source = match (&profile.file, built_in(&profile.name).is_some()) {
            (Some(file), true) => format!("built in, with {}", file.display()),
            (Some(file), false) => file.display().to_string(),
            (None, _) => "built in".to_string(),
        };
        lines.push(format!("{}  ({source})", profile.name));
        lines.push(format!("  for: {}", if profile.fits.is_empty() { "(not said)" } else { &profile.fits }));
        if !profile.about.is_empty() {
            lines.push(format!("  about: {}", profile.about));
        }
        let mut runs = vec![profile.agent.key().to_string()];
        runs.extend(profile.model.iter().map(|model| format!("model {model}")));
        runs.extend(profile.effort.iter().map(|effort| format!("effort {effort}")));
        runs.extend(profile.permission_mode.iter().map(|mode| format!("mode {mode}")));
        runs.extend((!profile.skills.is_empty()).then(|| format!("skills {}", profile.skills.join(", "))));
        runs.extend((profile.sandbox.enabled == Some(false)).then(|| "sandbox off".to_string()));
        runs.extend((!profile.sandbox.excluded.is_empty()).then(|| format!("outside the sandbox: {}", profile.sandbox.excluded.join(", "))));
        lines.push(format!("  runs: {}", runs.join(" · ")));
    }
    lines.extend(errors.iter().map(|error| format!("ignored: {error}")));
    lines.join("\n")
}

pub fn print(json: bool) -> Result<String> {
    let (profiles, errors) = list_in(&dir());
    if json {
        let mut shown: Vec<Value> = profiles.iter().map(to_json).collect();
        shown.extend(errors.iter().map(|error| json!({ "error": error })));
        return serde_json::to_string_pretty(&shown).map_err(|error| error.to_string());
    }
    Ok(describe(&profiles, &errors))
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
    fn a_profile_names_claude_or_is_refused_with_its_file_and_key() {
        let dir = folder("profile-refused", &[]);
        let refused = |text: &str| { std::fs::write(dir.join("x.toml"), text).unwrap(); load_in(&dir, "x").unwrap_err() };
        assert!(refused("agent = \"codex\"\n").ends_with("x.toml: agent: \"codex\" is not supported yet; kelpie runs \"claude\""), "{}", refused("agent = \"codex\"\n"));
        assert!(refused("agent = \"gemini\"\n").contains("agent: \"gemini\" is not an agent kelpie runs"));
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
    fn an_allow_that_would_release_a_guarded_kelpie_command_is_refused_when_the_profile_loads() {
        for released in ["Bash(kelpie push:*)", "Bash(/home/ada/.local/share/kelpie/current/target/release/kelpie approve:*)", "Bash(kelpie choose:*)", "Bash(kelpie away on)", "Bash(kelpie decide:*)", "Bash(kelpie after:*)", "Bash(kelpie:*)", "Bash(kelpie *)", "Bash(kelpie -C /x push:*)", "Bash", "Bash(*)", "Bash(:*)", "Bash(*push:*)"] {
            assert!(releases_guard(released), "{released}");
        }
        for released in ["Bash(kelpie --repo app push:*)", "Bash(kelpie --no-focus approve:*)", "Bash(kelpie -x tell:*)"] {
            assert!(releases_guard(released), "a flag kelpie's guard cannot place hides the command after it: {released}");
        }
        for kept in ["Bash(kelpie -C /src/app tell:*)", "Bash(kelpie tell:*)", "Bash(kelpie ask:*)", "Bash(npx playwright:*)", "Read", "WebFetch(domain:example.com)", "Bash(git log:*)"] {
            assert!(!releases_guard(kept), "{kept}");
        }
        let dir = folder("profile-allow", &[("x.toml", "allow = [\"Bash(npm test:*)\", \"Bash(kelpie push:*)\"]\n")]);
        let refused = load_in(&dir, "x").unwrap_err();
        assert!(refused.contains("x.toml: allow: \"Bash(kelpie push:*)\" would let the task run kelpie push, approve, choose, away, decide, after"), "{refused}");
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
        assert!(for_task(&dir, REVIEWER).unwrap_err().contains("kelpie review's profile, not a task's"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_built_in_task_adds_nothing_to_kelpies_role() {
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
        assert_eq!((reviewer.model.as_deref(), reviewer.effort.as_deref(), reviewer.about.as_str()), (Some("opus"), Some("max"), "The read-only reviewer that kelpie review runs"));
        for key in ["permission_mode = \"bypassPermissions\"\n", "allow = [\"Edit\"]\n", "skills = [\"x\"]\n"] {
            std::fs::write(dir.join("reviewer.toml"), key).unwrap();
            assert!(load_in(&dir, REVIEWER).unwrap_err().contains("the reviewer only reads, so its profile takes about, for, agent, model, effort, role"), "{key}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn kelpie_profiles_lists_the_built_in_ones_then_each_file_and_names_a_broken_one() {
        let dir = folder("profile-list", &[("designer.toml", "about = \"UI work\"\nfor = \"UI, CSS\"\nmodel = \"opus\"\n"), ("broken.toml", "agent = \"codex\"\n"), ("notes.txt", "x")]);
        let (profiles, errors) = list_in(&dir);
        assert_eq!(profiles.iter().map(|profile| profile.name.as_str()).collect::<Vec<_>>(), ["task", "reviewer", "ops", "designer"]);
        assert_eq!(errors.len(), 1);
        let shown = describe(&profiles, &errors);
        assert!(shown.starts_with("task  (built in)\n  for: any task no other profile fits\n"), "{shown}");
        assert!(shown.contains(&format!("designer  ({})\n  for: UI, CSS\n  about: UI work\n  runs: claude · model opus", dir.join("designer.toml").display())), "{shown}");
        assert!(shown.contains("ignored: ") && shown.contains("broken.toml: agent: \"codex\" is not supported yet"), "{shown}");
        assert!(shown.contains("ops  (built in)\n  for: work on other machines over ssh\n") && shown.contains("outside the sandbox: ssh *, scp *, rsync *"), "{shown}");
        let json = to_json(&profiles[3]);
        assert_eq!((json["name"].as_str(), json["for"].as_str(), json["about"].as_str(), json["agent"].as_str(), json["built_in"].as_bool()), (Some("designer"), Some("UI, CSS"), Some("UI work"), Some("claude"), Some(false)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_lead_chooses_a_tasks_profile_by_what_each_is_for() {
        let lead = include_str!("../roles/lead.md");
        assert!(lead.contains("`kelpie profiles` lists the profiles") && lead.contains("`for`") && lead.contains("`--profile <name>`"), "the lead knows the profiles and passes one");
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
        std::fs::write(dir.join("designer.toml"), "agent = \"codex\"\n").unwrap();
        let (profile, said) = for_running_task(&dir, None, Some("designer"), None).unwrap();
        assert_eq!(profile, built_in(TASK).unwrap(), "work keeps going");
        let said = said.unwrap();
        assert!(said.contains("its profile \"designer\" did not load") && said.contains("designer.toml: agent: \"codex\" is not supported yet"), "{said}");
        std::fs::remove_file(dir.join("designer.toml")).unwrap();
        assert!(for_running_task(&dir, None, Some("designer"), None).unwrap().1.unwrap().contains("No profile \"designer\""));
        assert!(for_running_task(&dir, Some("designer"), Some("task"), None).is_err(), "a profile asked for by name is refused, not swapped");
        assert!(for_running_task(&dir, None, None, Some("designer")).is_err(), "a new start never falls back");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_profile_turns_the_sandbox_off_or_replaces_its_hosts_and_folders_and_adds_commands_outside_it() {
        let dir = folder("profile-sandbox", &[
            ("loose.toml", "[sandbox]\nenabled = false\n"),
            ("web.toml", "[sandbox]\nallowed_domains = [\"registry.yarnpkg.com\"]\nallow_write = [\"~/.gradle\"]\nexcluded_commands = [\"docker compose *\"]\n"),
            ("ops.toml", "model = \"opus\"\n[sandbox]\nexcluded_commands = [\"kubectl *\", \"ssh *\"]\n"),
        ]);
        assert_eq!(load_in(&dir, "loose").unwrap().sandbox.enabled, Some(false));
        let web = load_in(&dir, "web").unwrap().sandbox;
        assert_eq!((web.enabled, web.domains(), web.writes(), web.excluded), (None, vec!["registry.yarnpkg.com".to_string()], vec!["~/.gradle".to_string()], vec!["docker compose *".to_string()]));
        assert_eq!(built_in(TASK).unwrap().sandbox.domains(), crate::sandbox::DOMAINS.map(str::to_string).to_vec(), "kelpie's list until a profile names its own");
        let ops = load_in(&dir, OPS).unwrap();
        assert_eq!((ops.model.as_deref(), ops.sandbox.excluded), (Some("opus"), vec!["ssh *".to_string(), "scp *".to_string(), "rsync *".to_string(), "kubectl *".to_string()]), "a file adds to the built-in ops");
        let refused = |text: &str| { std::fs::write(dir.join("x.toml"), text).unwrap(); load_in(&dir, "x").unwrap_err() };
        assert!(refused("[sandbox]\nexcluded_commands = [\"tmux *\"]\n").contains("x.toml: sandbox.excluded_commands: \"tmux *\" would run tmux outside the sandbox"));
        assert!(refused("[sandbox]\nexcluded_commands = [\"kelpie push *\"]\n").contains("would run kelpie outside the sandbox"));
        assert!(refused("[sandbox]\nallow_write = [\"~/.config\"]\n").contains("sandbox.allow_write: \"~/.config\" would let a task's shell write"));
        assert!(refused("[sandbox]\nenabled = \"no\"\n").contains("sandbox.enabled: must be true or false"));
        assert!(refused("[sandbox]\nnetwork = true\n").contains("sandbox.network: unknown key"));
        std::fs::write(dir.join("reviewer.toml"), "[sandbox]\nenabled = false\n").unwrap();
        assert!(load_in(&dir, REVIEWER).unwrap_err().contains("the reviewer only reads"), "the reviewer's sandbox follows task.sandbox alone");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
