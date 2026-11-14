use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::agent::Agent;
use crate::agent::codex_setup::{self, Fate, Installer};
use crate::profiles::Profile;
use crate::sh::Result;

pub const FILE: &str = "agent-check.json";
pub const EVERY: u64 = 24 * 60 * 60;
pub const ROUTINE_EVERY: u64 = 60 * 60;
pub const KIND: &str = "agents";

// One thing the daily check found about an agent that does not update itself; the key says whether the board was told already.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub key: String,
    pub text: String,
    pub fix: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub checked_at: u64,
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub installer: Option<Installer>,
    pub default_model: Option<String>,
    pub error: Option<String>,
    pub findings: Vec<Finding>,
}

const INSTALLERS: [(Installer, &str); 5] = [(Installer::Standalone, "standalone"), (Installer::Brew, "brew"), (Installer::Npm, "npm"), (Installer::Bun, "bun"), (Installer::Pnpm, "pnpm")];

fn installer_key(installer: Installer) -> &'static str {
    INSTALLERS.iter().find(|(known, _)| *known == installer).map_or("", |(_, key)| key)
}

impl Found {
    // A newer Codex than the one installed, with how to install it.
    pub fn newer(&self) -> Option<(&str, &str)> {
        let (installed, latest) = (self.installed.as_deref()?, self.latest.as_deref()?);
        crate::update_check::newer(latest, installed).then_some((installed, latest))
    }

    fn to_json(&self) -> Value {
        json!({
            "checked_at": self.checked_at, "installed": self.installed, "latest": self.latest, "installer": self.installer.map(installer_key),
            "default_model": self.default_model, "error": self.error,
            "findings": self.findings.iter().map(|finding| json!({ "key": finding.key, "text": finding.text, "fix": finding.fix })).collect::<Vec<_>>(),
        })
    }

    fn from_json(value: &Value) -> Option<Found> {
        let text = |key: &str| value[key].as_str().map(str::to_string);
        Some(Found {
            checked_at: value["checked_at"].as_u64()?,
            installed: text("installed"),
            latest: text("latest"),
            installer: value["installer"].as_str().and_then(|key| INSTALLERS.iter().find(|(_, known)| *known == key).map(|(installer, _)| *installer)),
            default_model: text("default_model"),
            error: text("error"),
            findings: value["findings"].as_array().into_iter().flatten().filter_map(|finding| Some(Finding { key: finding["key"].as_str()?.to_string(), text: finding["text"].as_str()?.to_string(), fix: finding["fix"].as_str().unwrap_or_default().to_string() })).collect(),
        })
    }
}

pub fn read(state: &Path) -> Option<Found> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(state.join(FILE)).ok()?).ok()?;
    Found::from_json(&value)
}

fn write(state: &Path, found: &Found) {
    let _ = crate::state::write(&state.join(FILE), found.to_json().to_string());
}

#[cfg(test)]
pub fn write_for_test(state: &Path, found: &Found) {
    write(state, found);
}

// After Codex's own installer ran, the record names the version now installed, so doctor and the next U see it.
pub fn installed_now(state: &Path) {
    if let Some(found) = read(state) {
        write(state, &Found { installed: Machine::current().version.or(found.installed.clone()), ..found });
    }
}

// What the check reads on this machine: Codex's binary and version, its home, the profiles that run it, and whether kebbie's vault is on.
pub struct Machine {
    pub binary: Option<PathBuf>,
    pub version: Option<String>,
    pub home: Option<PathBuf>,
    pub profiles: Vec<Profile>,
    pub vault_on: bool,
    pub plugin: bool,
    pub macos: bool,
}

impl Machine {
    pub fn current() -> Machine {
        let path: Vec<PathBuf> = std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default();
        let home = crate::agent::codex::home();
        let kebbie = crate::peers::kebbie_command().unwrap_or_default();
        let found = crate::agent::codex::availability(&path, home.as_deref(), &kebbie);
        let claude = crate::agent::claude::home().unwrap_or_default();
        Machine {
            version: found.version.as_deref().and_then(codex_setup::version_word),
            plugin: found.plugin != crate::agent::codex::Installed::Missing,
            binary: found.binary,
            home,
            profiles: crate::profiles::list_in(&crate::profiles::dir()).0,
            vault_on: matches!(crate::vault_cli::plugin(&claude), crate::vault_cli::Plugin::Enabled { .. }),
            macos: cfg!(target_os = "macos"),
        }
    }
}

// Only a profile of the user's own pins a model; kebbie's own follow the agent's default.
fn pinned(profile: &Profile) -> Option<&str> {
    profile.file.as_ref().and(profile.model.as_deref()).filter(|_| profile.agent == Agent::Codex)
}

pub fn check(machine: &Machine, previous: Option<&Found>, now: u64, latest: impl FnOnce(Option<Installer>) -> Result<String>) -> Found {
    let mut found = Found { checked_at: now, ..Found::default() };
    let Some(binary) = &machine.binary else { return found };
    let home = machine.home.clone().unwrap_or_default();
    found.installed = machine.version.clone();
    found.installer = codex_setup::installer(binary, &home, machine.macos);
    match latest(found.installer) {
        Ok(version) => found.latest = Some(version),
        Err(error) => found.error = Some(error),
    }
    if let Some((installed, latest)) = found.newer() {
        let fix = found.installer.map_or("update Codex the way you installed it".to_string(), |installer| format!("U on the board, or {}", installer.command_line()));
        found.findings.push(Finding { key: format!("codex-update:{latest}"), text: format!("Codex {latest} is out; this machine runs {installed}"), fix });
    }
    let models = crate::agent::codex::cached_models(&home).unwrap_or_default();
    found.default_model = codex_setup::default_model(&models).map(|model| model.slug.clone());
    for profile in &machine.profiles {
        let Some(model) = pinned(profile).filter(|_| !models.is_empty()) else { continue };
        let fix = "pick one with m on the Settings page".to_string();
        match codex_setup::fate(&models, model) {
            Fate::Listed => {}
            Fate::Upgrade(to) => found.findings.push(Finding { key: format!("model-upgrade:{}:{model}:{to}", profile.name), text: format!("{}: Codex offers {to} in place of {model}", profile.name), fix }),
            Fate::Gone(to) => found.findings.push(Finding { key: format!("model-gone:{}:{model}", profile.name), text: format!("{}: {model} is gone from Codex's models; its tasks start on {}", profile.name, to.as_deref().unwrap_or("Codex's default")), fix }),
        }
    }
    if let (Some(before), Some(now)) = (previous.and_then(|previous| previous.default_model.as_deref()), found.default_model.as_deref())
        && before != now
    {
        found.findings.push(Finding { key: format!("default:{now}"), text: format!("Codex's default model is now {now}, was {before}; Codex profiles without a model follow it"), fix: "pick another with m on the Settings page".to_string() });
    }
    if machine.vault_on && machine.plugin && codex_setup::found_server(&codex_setup::read_config(machine.home.as_deref())).is_none() {
        found.findings.push(Finding { key: "vault-missing".to_string(), text: "Codex has no vault server, so the lead keeps Codex tasks' notes".to_string(), fix: "kebbie setup codex".to_string() });
    }
    found
}

// A finding goes to the board once; one the last check had already said stays quiet.
pub fn told(previous: Option<&Found>, found: &Found) -> Vec<String> {
    found.findings.iter().filter(|finding| !previous.is_some_and(|previous| previous.findings.iter().any(|seen| seen.key == finding.key)))
        .map(|finding| format!("{}: {}", finding.text, finding.fix)).collect()
}

// Once a day, like the update check: the claim is written before the fetch so a second keeper waits.
pub fn tick(state: &Path) -> Vec<String> {
    tick_with(state, crate::messages::now(), &Machine::current(), |installer| std::env::var(LATEST_VAR).map_or_else(|_| codex_setup::fetch_latest(installer), Ok))
}

// The scenario suite names Codex's latest version here, so its keeper fetches nothing.
pub const LATEST_VAR: &str = "KEBBIE_CODEX_LATEST";

fn tick_with(state: &Path, now: u64, machine: &Machine, latest: impl FnOnce(Option<Installer>) -> Result<String>) -> Vec<String> {
    let previous = read(state);
    if previous.as_ref().is_some_and(|previous| now.saturating_sub(previous.checked_at) < EVERY) {
        return Vec::new();
    }
    write(state, &Found { checked_at: now, ..previous.clone().unwrap_or_default() });
    let found = check(machine, previous.as_ref(), now, latest);
    let told = told(previous.as_ref(), &found);
    for said in &told {
        let _ = crate::messages::log_for_user_in(state, said, KIND);
    }
    write(state, &found);
    told.into_iter().map(|said| format!("agents: {said}")).collect()
}

fn line(level: crate::doctor::Level, name: &str, fact: impl Into<String>, fix: &[String]) -> crate::doctor::Check {
    crate::doctor::Check::new(level, name, fact, fix.to_vec())
}

pub struct Doctor<'a> {
    pub found: Option<&'a Found>,
    pub mode: crate::config::AgentUpdates,
    pub codex_home: &'a Path,
    pub claude_dir: &'a Path,
    pub rules: &'a str,
    pub vault_on: bool,
    pub vault_command: Option<&'a [String]>,
}

// What Codex lacks that kebbie setup codex offers, and what the daily check found; nothing here stops a task.
pub fn doctor(given: &Doctor) -> Vec<crate::doctor::Check> {
    use crate::doctor::Level;
    let setup = ["kebbie setup codex".to_string()];
    let mut checks = Vec::new();
    let age = |at: u64| match crate::messages::age(at).as_str() { "now" => "just now".to_string(), age => format!("{age} ago") };
    checks.push(match given.found {
        None => line(Level::Ok, "codex update", "not checked yet; kebbie checks once a day", &[]).brief("update not checked yet"),
        Some(found) => match (found.newer(), &found.error) {
            (Some((installed, latest)), _) => {
                let how = found.installer.map_or("installed some way kebbie cannot tell".to_string(), |installer| format!("a {}", installer.shown()));
                let fix = match found.installer {
                    Some(installer) if given.mode == crate::config::AgentUpdates::Notify => vec![installer.command_line()],
                    Some(_) => vec!["U on the board".to_string()],
                    None => vec!["update Codex the way you installed it".to_string()],
                };
                line(Level::Warn, "codex update", format!("Codex {installed} → {latest}, {how} (checked {})", age(found.checked_at)), &fix)
            }
            (None, Some(error)) => line(Level::Skip, "codex update", format!("could not read the latest Codex: {error} (checked {})", age(found.checked_at)), &[]),
            (None, None) => line(Level::Ok, "codex update", format!("{} is the latest (checked {})", found.installed.as_deref().unwrap_or("Codex"), age(found.checked_at)), &[]).brief("up to date"),
        },
    });
    for finding in given.found.into_iter().flat_map(|found| &found.findings).filter(|finding| finding.key.starts_with("model-")) {
        checks.push(line(Level::Warn, "codex profile", &finding.text, std::slice::from_ref(&finding.fix)));
    }
    let rules = crate::area_page::home_relative(&codex_setup::rules_path(given.codex_home));
    checks.push(match codex_setup::rules_placed(given.codex_home, given.rules) {
        codex_setup::Placed::Current => line(Level::Ok, "codex approvals", format!("{rules} lets kebbie's own task commands through"), &[]).brief("rules"),
        codex_setup::Placed::Missing => line(Level::Warn, "codex approvals", format!("{rules} is missing, so Codex asks before each kebbie command a task runs"), &setup),
        codex_setup::Placed::Differs => match std::fs::read_to_string(codex_setup::rules_path(given.codex_home)).ok().as_deref().and_then(codex_setup::missing_commands).filter(|missing| !missing.is_empty()) {
            Some(missing) => line(Level::Warn, "codex approvals", format!("{rules} has no line for {}, so Codex asks before a task runs {}", missing.join(", "), if missing.len() == 1 { "it" } else { "them" }), &setup),
            None => line(Level::Warn, "codex approvals", format!("{rules} is not the file this kebbie writes"), &setup),
        },
    });
    if given.vault_on {
        let config = given.codex_home.join("config.toml");
        let shown = crate::area_page::home_relative(&config);
        let text = std::fs::read_to_string(&config).ok();
        let server = codex_setup::wanted_server(given.claude_dir, given.vault_command);
        let present = codex_setup::found_server(&codex_setup::read_config(Some(given.codex_home))).is_some();
        checks.push(match (present, server.map(|server| codex_setup::vault_placed(text.as_deref(), &server))) {
            (false, _) => line(Level::Fail, "codex vault", format!("{shown} has no [mcp_servers.vault], so the lead keeps Codex tasks' notes; tasks still run"), &setup),
            (true, Some(codex_setup::Placed::Differs)) => line(Level::Warn, "codex vault", format!("[mcp_servers.vault] in {shown} has other values than {}", if given.vault_command.is_some() { "kebbie's vault command" } else { "the vault plugin's" }), &setup),
            (true, _) => line(Level::Ok, "codex vault", format!("[mcp_servers.vault] in {shown}; kebbie starts Codex tasks with its keys set"), &[]).brief("vault entry"),
        });
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex_profile(name: &str, model: Option<&str>) -> Profile {
        Profile { name: name.to_string(), agent: Agent::Codex, model: model.map(str::to_string), file: Some(PathBuf::from(format!("/p/{name}.toml"))), ..crate::profiles::built_in(crate::profiles::TASK).unwrap() }
    }

    fn machine(dir: &Path) -> Machine {
        let home = dir.join("codex");
        let release = home.join("packages/standalone/releases/0.163.0-aarch64-apple-darwin");
        std::fs::create_dir_all(&release).unwrap();
        crate::executable::write(release.join("codex"), "#!/bin/sh\n").unwrap();
        std::fs::write(home.join("models_cache.json"), crate::agent::codex::CACHE_FIXTURE).unwrap();
        let mut own_model = codex_profile("astra", Some("gpt-6-astra"));
        own_model.file = None;
        Machine {
            binary: Some(release.join("codex")),
            version: Some("0.163.0".to_string()),
            home: Some(home),
            profiles: vec![codex_profile("coder", Some("gpt-5.1-codex-max")), codex_profile("plain", None), codex_profile("astra", Some("gpt-6-astra")), Profile { name: "kebbies".to_string(), ..own_model }],
            vault_on: true,
            plugin: true,
            macos: true,
        }
    }

    #[test]
    fn the_daily_check_finds_a_newer_codex_a_model_gone_and_a_default_that_moved() {
        let dir = crate::sh::test_dir("agent-check");
        let machine = machine(&dir);
        let mut asked = None;
        let found = check(&machine, Some(&Found { default_model: Some("gpt-6-astra".to_string()), ..Found::default() }), 1000, |installer| { asked = Some(installer); Ok("0.164.0".to_string()) });
        assert_eq!(asked, Some(Some(Installer::Standalone)), "the latest comes from where Codex's own check reads it for its install");
        assert_eq!(found.newer(), Some(("0.163.0", "0.164.0")));
        let texts: Vec<String> = found.findings.iter().map(|finding| format!("{} · {}", finding.text, finding.fix)).collect();
        assert_eq!(texts, [
            "Codex 0.164.0 is out; this machine runs 0.163.0 · U on the board, or sh -c 'curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh'",
            "coder: gpt-5.1-codex-max is gone from Codex's models; its tasks start on gpt-6-astra · pick one with m on the Settings page",
            "Codex's default model is now gpt-6.1-sol, was gpt-6-astra; Codex profiles without a model follow it · pick another with m on the Settings page",
            "Codex has no vault server, so the lead keeps Codex tasks' notes · kebbie setup codex",
        ], "a listed model, a profile without one and kebbie's own profiles are fine");
        let quiet = check(&Machine { vault_on: false, ..machine }, None, 1000, |_| Err("offline".to_string()));
        assert_eq!((quiet.error.as_deref(), quiet.newer(), quiet.findings.len()), (Some("offline"), None, 1), "{quiet:?}");
        let none = check(&Machine { binary: None, version: None, home: None, profiles: Vec::new(), vault_on: true, plugin: false, macos: true }, None, 1, |_| panic!("no Codex, no fetch"));
        assert_eq!(none, Found { checked_at: 1, ..Found::default() });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn each_finding_reaches_the_board_once_a_day_at_most() {
        let dir = crate::sh::test_dir("agent-check-tick");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let machine = machine(&dir);
        let told = tick_with(&state, 1000, &machine, |_| Ok("0.164.0".to_string()));
        assert_eq!(told.len(), 3, "{told:?}");
        let messages = std::fs::read_to_string(state.join("messages.jsonl")).unwrap();
        assert!(messages.contains("Codex 0.164.0 is out") && messages.contains("\"kind\":\"agents\"") && messages.contains("\"to\":\"you\""), "{messages}");
        assert!(tick_with(&state, 2000, &machine, |_| panic!("checked once a day")).is_empty());
        assert_eq!(read(&state).unwrap().latest.as_deref(), Some("0.164.0"), "the board and doctor read what the check found");
        assert!(tick_with(&state, 1000 + EVERY, &machine, |_| Ok("0.164.0".to_string())).is_empty(), "a finding the board has is not told again");
        assert_eq!(tick_with(&state, 1000 + 2 * EVERY, &machine, |_| Ok("0.165.0".to_string())), ["agents: Codex 0.165.0 is out; this machine runs 0.163.0: U on the board, or sh -c 'curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh'"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn doctor_names_what_codex_lacks_with_its_fix_and_fails_a_missing_vault_server() {
        use crate::doctor::Level;
        let dir = crate::sh::test_dir("agent-check-doctor");
        let codex = dir.join("codex");
        std::fs::create_dir_all(&codex).unwrap();
        let rules = codex_setup::rules_text(&["kebbie".to_string()]);
        let found = Found { checked_at: crate::messages::now(), installed: Some("0.163.0".to_string()), latest: Some("0.164.0".to_string()), installer: Some(Installer::Standalone), findings: vec![Finding { key: "model-gone:coder:gpt-5".to_string(), text: "coder: gpt-5 is gone from Codex's models; its tasks start on gpt-6.1-sol".to_string(), fix: "pick one with m on the Settings page".to_string() }], ..Found::default() };
        let given = Doctor { found: Some(&found), mode: crate::config::AgentUpdates::Ask, codex_home: &codex, claude_dir: &dir.join("claude"), rules: &rules, vault_on: true, vault_command: None };
        let lines: Vec<(Level, String, String, Vec<String>)> = doctor(&given).into_iter().map(|check| (check.level, check.name, check.fact, check.fix)).collect();
        assert_eq!(lines[0], (Level::Warn, "codex update".to_string(), "Codex 0.163.0 → 0.164.0, a standalone install (checked just now)".to_string(), vec!["U on the board".to_string()]));
        assert_eq!((lines[1].0, lines[1].1.as_str(), lines[1].3[0].as_str()), (Level::Warn, "codex profile", "pick one with m on the Settings page"));
        assert!(lines[2].0 == Level::Warn && lines[2].2.ends_with("rules/kebbie.rules is missing, so Codex asks before each kebbie command a task runs") && lines[2].3 == ["kebbie setup codex"], "{:?}", lines[2]);
        assert!(lines[3].0 == Level::Fail && lines[3].2.contains("has no [mcp_servers.vault], so the lead keeps Codex tasks' notes; tasks still run") && lines[3].3 == ["kebbie setup codex"], "with kebbie's vault on, every agent needs it: {:?}", lines[3]);
        let notify = doctor(&Doctor { mode: crate::config::AgentUpdates::Notify, vault_on: false, ..given });
        assert_eq!(notify[0].fix, ["sh -c 'curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh'"], "only told: the fix is Codex's own installer");
        assert!(notify.iter().all(|check| check.name != "codex vault"), "no vault, no vault line");
        codex_setup::write_rules(&codex, &rules.lines().filter(|line| !line.contains("\"obstacle\"")).map(|line| format!("{line}\n")).collect::<String>()).unwrap();
        let stale = doctor(&Doctor { found: None, ..given }).into_iter().find(|check| check.name == "codex approvals").unwrap();
        assert!(stale.level == Level::Warn && stale.fact.ends_with("rules/kebbie.rules has no line for obstacle, so Codex asks before a task runs it") && stale.fix == ["kebbie setup codex"], "{:?}", stale.fact);
        codex_setup::write_rules(&codex, &rules).unwrap();
        assert_eq!(doctor(&Doctor { found: None, ..given })[..2].iter().map(|check| (check.level, check.name.as_str())).collect::<Vec<_>>(), [(Level::Ok, "codex update"), (Level::Ok, "codex approvals")]);
        let command = ["/home/me/.local/bin/mindin".to_string()];
        std::fs::write(codex.join("config.toml"), "[mcp_servers.vault]\ncommand = \"/home/me/.local/bin/mindin\"\n").unwrap();
        let vault = |given: &Doctor| doctor(given).into_iter().find(|check| check.name == "codex vault").map(|check| (check.level, check.fact)).unwrap();
        assert_eq!(vault(&Doctor { vault_command: Some(&command), ..given }).0, Level::Ok, "Codex's server runs kebbie's vault command");
        let other = ["uvx".to_string(), "mindin".to_string()];
        let differs = vault(&Doctor { vault_command: Some(&other), ..given });
        assert!(differs.0 == Level::Warn && differs.1.ends_with("has other values than kebbie's vault command"), "{differs:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
