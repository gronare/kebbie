use std::path::{Path, PathBuf};

use serde_json::Value;

use super::codex::CachedModel;
use crate::sh::Result;

pub const RULES_FILE: &str = "rules/kebbie.rules";
pub const SECRETS: [&str; 2] = ["S3_ACCESS_KEY", "S3_SECRET_KEY"];
const SERVER: &str = "vault";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placed {
    Missing,
    Current,
    Differs,
}

pub fn rules_path(home: &Path) -> PathBuf {
    home.join(RULES_FILE)
}

// The three ways a task names kebbie: the path its role gives, the link setup makes, and the bare name on PATH.
pub fn rule_forms(kebbie: &str, link: &Path) -> Vec<String> {
    let mut forms: Vec<String> = Vec::new();
    for form in [kebbie.to_string(), link.to_string_lossy().to_string(), "kebbie".to_string()] {
        if !form.is_empty() && !forms.contains(&form) {
            forms.push(form);
        }
    }
    forms
}

fn quoted(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

pub fn rules_text(forms: &[String]) -> String {
    let mut text = String::new();
    for form in forms {
        for command in crate::sandbox::TASK_COMMANDS {
            let words: Vec<String> = std::iter::once(form.as_str()).chain(command.split_whitespace()).map(quoted).collect();
            text.push_str(&format!("prefix_rule(pattern=[{}], decision=\"allow\")\n", words.join(", ")));
        }
    }
    text
}

pub fn rules_placed(home: &Path, text: &str) -> Placed {
    match std::fs::read_to_string(rules_path(home)) {
        Err(_) => Placed::Missing,
        Ok(found) if found == text => Placed::Current,
        Ok(_) => Placed::Differs,
    }
}

pub fn write_rules(home: &Path, text: &str) -> Result<()> {
    let path = rules_path(home);
    let dir = path.parent().ok_or("the rules file has no folder")?;
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    std::fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_vars: Vec<String>,
}

fn read_json(path: &Path) -> Value {
    std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

// The vault plugin's options as Claude Code keeps them, under pluginConfigs."vault@<marketplace>".
pub fn vault_options(claude_dir: &Path) -> Value {
    read_json(&claude_dir.join("settings.json"))["pluginConfigs"].as_object().into_iter().flatten()
        .find(|(id, _)| id.starts_with("vault@")).map(|(_, config)| config["options"].clone()).unwrap_or_default()
}

// "${user_config.s3_bucket}" names the option it takes; any other text is the value itself.
fn filled(value: &str, options: &Value) -> String {
    match value.strip_prefix("${user_config.").and_then(|rest| rest.strip_suffix('}')) {
        Some(option) => options[option].as_str().unwrap_or_default().trim().to_string(),
        None => value.to_string(),
    }
}

fn plugin_server(plugin_root: &Path) -> Value {
    read_json(&plugin_root.join(".mcp.json"))["mcpServers"][SERVER].clone()
}

// The Claude vault plugin's own server for Codex: its command, its args and its settings, with the two keys named for Codex to read from the environment, never written.
pub fn vault_server(plugin_root: &Path, options: &Value) -> Option<Server> {
    let server = plugin_server(plugin_root);
    let command = server["command"].as_str()?.to_string();
    let args = server["args"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    let mut env = Vec::new();
    let mut env_vars = Vec::new();
    for (name, value) in server["env"].as_object().into_iter().flatten() {
        let value = filled(value.as_str().unwrap_or_default(), options);
        match (value.is_empty(), SECRETS.contains(&name.as_str())) {
            (true, _) => {}
            (false, true) => env_vars.push(name.clone()),
            (false, false) => env.push((name.clone(), value)),
        }
    }
    env.sort();
    env_vars.sort();
    Some(Server { command, args, env, env_vars })
}

// Codex runs the vault command kebbie runs; when that is the Claude vault plugin's own server, it takes the plugin's settings with it.
pub fn server_for(claude_dir: &Path, command: &[String]) -> Option<Server> {
    let plugin = crate::vault_cli::installed(claude_dir).map(|(_, _, root)| root).filter(|root| crate::vault_cli::server_command(root).as_deref() == Some(command));
    if let Some(root) = plugin {
        return vault_server(&root, &vault_options(claude_dir));
    }
    let (program, args) = command.split_first()?;
    Some(Server { command: program.clone(), args: args.to_vec(), env: Vec::new(), env_vars: Vec::new() })
}

pub fn wanted_server(claude_dir: &Path, command: Option<&[String]>) -> Option<Server> {
    match command {
        Some(command) => server_for(claude_dir, command),
        None => crate::vault_cli::installed(claude_dir).and_then(|(_, _, root)| vault_server(&root, &vault_options(claude_dir))),
    }
}

// The keys a Codex session needs in its environment, read where the Claude vault plugin keeps them; only those Codex's own config names.
pub fn vault_secrets(plugin_root: &Path, options: &Value, wanted: &[String]) -> Vec<(String, String)> {
    let server = plugin_server(plugin_root);
    server["env"].as_object().into_iter().flatten()
        .filter(|(name, _)| SECRETS.contains(&name.as_str()) && wanted.contains(name))
        .map(|(name, value)| (name.clone(), filled(value.as_str().unwrap_or_default(), options)))
        .filter(|(_, value)| !value.is_empty())
        .collect()
}

fn list(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|item| quoted(item)).collect::<Vec<_>>().join(", "))
}

impl Server {
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!("[mcp_servers.{SERVER}]"), format!("command = {}", quoted(&self.command)), format!("args = {}", list(&self.args))];
        if !self.env_vars.is_empty() {
            lines.push(format!("env_vars = {}", list(&self.env_vars)));
        }
        if !self.env.is_empty() {
            lines.push(format!("[mcp_servers.{SERVER}.env]"));
            lines.extend(self.env.iter().map(|(name, value)| format!("{name} = {}", quoted(value))));
        }
        lines
    }

    fn matches(&self, found: &toml::Value) -> bool {
        let text = |key: &str| found.get(key).and_then(toml::Value::as_str).map(str::to_string);
        let words = |key: &str| found.get(key).and_then(toml::Value::as_array).map(|items| items.iter().filter_map(toml::Value::as_str).map(str::to_string).collect::<Vec<_>>()).unwrap_or_default();
        let mut env: Vec<(String, String)> = found.get("env").and_then(toml::Value::as_table).into_iter().flatten().filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string()))).collect();
        env.sort();
        let mut env_vars = words("env_vars");
        env_vars.sort();
        text("command").as_deref() == Some(self.command.as_str()) && words("args") == self.args && env == self.env && env_vars == self.env_vars
    }
}

pub fn found_server(config: &toml::Table) -> Option<&toml::Value> {
    config.get("mcp_servers")?.get(SERVER)
}

pub fn vault_placed(config: Option<&str>, server: &Server) -> Placed {
    let table = config.and_then(|text| text.parse::<toml::Table>().ok()).unwrap_or_default();
    match found_server(&table) {
        None => Placed::Missing,
        Some(found) if server.matches(found) => Placed::Current,
        Some(_) => Placed::Differs,
    }
}

// The server's lines go at the end of the user's file; a vault server of other values is replaced where it stands, and the rest of the file stays as it is.
pub fn with_vault(text: &str, server: &Server) -> Result<String> {
    let block = server.lines().join("\n") + "\n";
    let mut document: toml_edit::DocumentMut = text.parse().map_err(|error| format!("Codex's config.toml does not parse: {error}"))?;
    if document.get("mcp_servers").and_then(|servers| servers.get(SERVER)).is_none() {
        let gap = match text { "" => "", text if text.ends_with("\n\n") => "", text if text.ends_with('\n') => "\n", _ => "\n\n" };
        return Ok(format!("{text}{gap}{block}"));
    }
    let ours: toml_edit::DocumentMut = block.parse().map_err(|error| format!("kebbie's vault server lines do not parse: {error}"))?;
    document["mcp_servers"][SERVER] = ours["mcp_servers"][SERVER].clone();
    Ok(document.to_string())
}

// The secret names Codex reads from the environment for its vault server, as its config.toml gives them.
pub fn wanted_secrets(config: &toml::Table) -> Vec<String> {
    found_server(config).and_then(|server| server.get("env_vars")).and_then(toml::Value::as_array).into_iter().flatten()
        .filter_map(|item| item.as_str().map(str::to_string).or_else(|| item.get("name").and_then(toml::Value::as_str).map(str::to_string)))
        .filter(|name| SECRETS.contains(&name.as_str()))
        .collect()
}


// What a Codex task's session needs for the vault: the keys by the names its config gives, read where the Claude vault plugin keeps them, and those names kept from its commands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub vault: bool,
    pub env: Vec<(String, String)>,
    pub hidden: Vec<String>,
}

pub fn read_config(codex_home: Option<&Path>) -> toml::Table {
    codex_home.and_then(|home| std::fs::read_to_string(home.join("config.toml")).ok()).and_then(|text| text.parse().ok()).unwrap_or_default()
}

pub fn session(claude_dir: &Path, config: &toml::Table) -> Session {
    let names = wanted_secrets(config);
    let env = match crate::vault_cli::installed(claude_dir) {
        Some((_, _, root)) if !names.is_empty() => vault_secrets(&root, &vault_options(claude_dir), &names),
        _ => Vec::new(),
    };
    Session { vault: found_server(config).is_some(), hidden: hide_args(config, &names), env }
}

// Codex hands its own environment to the commands it runs (shell_environment_policy inherits all), so the keys are excluded by name; a config on the older exclude list keeps its entries, as filters and that list cannot mix.
pub fn hide_args(config: &toml::Table, names: &[String]) -> Vec<String> {
    if names.is_empty() {
        return Vec::new();
    }
    let policy = config.get("shell_environment_policy").and_then(toml::Value::as_table);
    if policy.is_some_and(|policy| policy.contains_key("exclude") || policy.contains_key("include_only")) {
        let theirs = policy.and_then(|policy| policy.get("exclude")).and_then(toml::Value::as_array).into_iter().flatten().filter_map(toml::Value::as_str).map(str::to_string);
        let all: Vec<String> = theirs.chain(names.iter().cloned()).collect();
        return vec!["-c".to_string(), format!("shell_environment_policy.exclude={}", list(&all))];
    }
    names.iter().flat_map(|name| ["-c".to_string(), format!("shell_environment_policy.filters.{name}=\"exclude\"")]).collect()
}

// "0.164.0" → "0.164", as a short label beside another version.
pub fn short_version(version: &str) -> String {
    version.strip_suffix(".0").filter(|rest| rest.contains('.')).unwrap_or(version).to_string()
}

// Codex's default: the listed model it ranks first (models-manager, the lowest priority number).
pub fn default_model(models: &[CachedModel]) -> Option<&CachedModel> {
    models.iter().filter(|model| model.listed).min_by_key(|model| model.priority.unwrap_or(i64::MAX))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Installer {
    Standalone,
    Brew,
    Npm,
    Bun,
    Pnpm,
}

impl Installer {
    // Codex 0.163's UpdateAction::command_args (tui/src/update_action.rs).
    pub fn command(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Installer::Standalone => ("sh", &["-c", "curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh"]),
            Installer::Brew => ("brew", &["upgrade", "--cask", "codex"]),
            Installer::Npm => ("npm", &["install", "-g", "@openai/codex"]),
            Installer::Bun => ("bun", &["install", "-g", "@openai/codex"]),
            Installer::Pnpm => ("pnpm", &["add", "-g", "@openai/codex"]),
        }
    }

    pub fn shown(self) -> &'static str {
        match self {
            Installer::Standalone => "standalone install",
            Installer::Brew => "Homebrew cask",
            Installer::Npm => "npm install",
            Installer::Bun => "bun install",
            Installer::Pnpm => "pnpm install",
        }
    }

    pub fn command_line(self) -> String {
        let (program, args) = self.command();
        std::iter::once(program.to_string()).chain(args.iter().map(|arg| if arg.contains(' ') { crate::sh::quote(arg) } else { arg.to_string() })).collect::<Vec<_>>().join(" ")
    }
}

// How Codex was installed, as Codex 0.163 tells it from its own executable (install-context/src/lib.rs); npm, bun and pnpm by the package folder their wrapper runs from.
pub fn installer(binary: &Path, home: &Path, macos: bool) -> Option<Installer> {
    let exe = binary.canonicalize().ok()?;
    let releases = home.canonicalize().ok().map(|home| home.join("packages/standalone/releases"));
    if releases.is_some_and(|releases| exe.starts_with(releases)) {
        return Some(Installer::Standalone);
    }
    let shown = exe.to_string_lossy();
    if shown.contains("node_modules/@openai/codex") {
        return Some(if shown.contains("/.bun/") { Installer::Bun } else if shown.contains("/pnpm/") { Installer::Pnpm } else { Installer::Npm });
    }
    (macos && (exe.starts_with("/opt/homebrew") || exe.starts_with("/usr/local"))).then_some(Installer::Brew)
}

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const HOMEBREW_CASK_API_URL: &str = "https://formulae.brew.sh/api/cask/codex.json";

// The latest Codex as Codex's own update check reads it (tui/src/updates.rs): the Homebrew cask for a cask install, GitHub's latest release otherwise.
pub fn latest_source(installer: Option<Installer>) -> &'static str {
    if installer == Some(Installer::Brew) { HOMEBREW_CASK_API_URL } else { LATEST_RELEASE_URL }
}

pub fn latest_from(url: &str, body: &Value) -> Option<String> {
    if url == HOMEBREW_CASK_API_URL {
        return body["version"].as_str().map(str::to_string);
    }
    body["tag_name"].as_str()?.strip_prefix("rust-v").map(str::to_string)
}

pub fn fetch_latest(installer: Option<Installer>) -> Result<String> {
    let url = latest_source(installer);
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .user_agent(format!("kebbie/{}", env!("CARGO_PKG_VERSION")))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent.get(url).call().map_err(|error| format!("{url}: {error}"))?;
    let text = response.body_mut().read_to_string().map_err(|error| format!("{url}: {error}"))?;
    let body: Value = serde_json::from_str(&text).map_err(|error| format!("{url}: {error}"))?;
    latest_from(url, &body).ok_or_else(|| format!("{url}: no version in its answer"))
}

// "codex-cli 0.163.0" → "0.163.0".
pub fn version_word(said: &str) -> Option<String> {
    said.split_whitespace().find(|word| word.starts_with(|character: char| character.is_ascii_digit())).map(str::to_string)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fate {
    Listed,
    Upgrade(String),
    Gone(Option<String>),
}

// A model no longer listed is gone, and its task starts on the model its entry's upgrade names, else on Codex's default.
pub fn fate(models: &[CachedModel], model: &str) -> Fate {
    let default = || default_model(models).map(|model| model.slug.clone());
    match models.iter().find(|cached| cached.slug == model) {
        Some(cached) if cached.listed => match &cached.upgrade {
            Some(upgrade) if upgrade != model => Fate::Upgrade(upgrade.clone()),
            _ => Fate::Listed,
        },
        Some(cached) => Fate::Gone(cached.upgrade.clone().filter(|upgrade| models.iter().any(|model| model.listed && &model.slug == upgrade)).or_else(default)),
        None => Fate::Gone(default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_file_lets_each_task_command_through_in_each_form_kebbie_is_called_by() {
        let forms = rule_forms("/home/u/.local/share/kebbie/current/target/release/kebbie", Path::new("/home/u/.local/bin/kebbie"));
        assert_eq!(forms, ["/home/u/.local/share/kebbie/current/target/release/kebbie", "/home/u/.local/bin/kebbie", "kebbie"]);
        let text = rules_text(&forms);
        assert_eq!(text.lines().count(), 39, "thirteen commands, three forms");
        assert!(text.contains("prefix_rule(pattern=[\"kebbie\", \"tell\"], decision=\"allow\")\n"));
        assert!(text.contains("prefix_rule(pattern=[\"/home/u/.local/bin/kebbie\", \"routine\", \"ok\"], decision=\"allow\")\n"), "a two-word command is two words of the pattern");
        assert!(text.contains("prefix_rule(pattern=[\"/home/u/.local/share/kebbie/current/target/release/kebbie\", \"backlog\", \"add\"], decision=\"allow\")\n"));
        for refused in ["push", "approve", "review"] {
            assert!(!text.contains(&format!("\"{refused}\"")), "{refused} still asks");
        }
        assert_eq!(rule_forms("kebbie", Path::new("/home/u/.local/bin/kebbie")).len(), 2, "a form named twice is one");
        let home = crate::sh::test_dir("codex-rules");
        assert_eq!(rules_placed(&home, &text), Placed::Missing);
        write_rules(&home, &text).unwrap();
        assert_eq!(rules_placed(&home, &text), Placed::Current);
        assert_eq!(rules_placed(&home, &rules_text(&forms[1..])), Placed::Differs);
        let _ = std::fs::remove_dir_all(home);
    }

    fn vault_plugin(dir: &Path) -> PathBuf {
        let root = dir.join("vault-plugin");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".mcp.json"), r#"{"mcpServers":{"vault":{"command":"uvx","args":["notes-vault-mcp==0.8.0"],"env":{"VAULT_PATH":"${user_config.vault_path}","S3_ENDPOINT":"${user_config.s3_endpoint}","S3_BUCKET":"${user_config.s3_bucket}","S3_ACCESS_KEY":"${user_config.s3_access_key}","S3_SECRET_KEY":"${user_config.s3_secret_key}"}}}}"#).unwrap();
        root
    }

    fn s3() -> Value {
        serde_json::json!({ "s3_endpoint": "https://s3.example.com", "s3_bucket": "notes", "s3_access_key": "AKIA1", "s3_secret_key": "hunter2" })
    }

    #[test]
    fn the_vault_server_for_codex_is_the_plugins_own_with_the_keys_named_and_never_written() {
        let dir = crate::sh::test_dir("codex-vault-server");
        let root = vault_plugin(&dir);
        let server = vault_server(&root, &s3()).unwrap();
        assert_eq!(server.lines(), [
            "[mcp_servers.vault]",
            "command = \"uvx\"",
            "args = [\"notes-vault-mcp==0.8.0\"]",
            "env_vars = [\"S3_ACCESS_KEY\", \"S3_SECRET_KEY\"]",
            "[mcp_servers.vault.env]",
            "S3_BUCKET = \"notes\"",
            "S3_ENDPOINT = \"https://s3.example.com\"",
        ]);
        assert!(!server.lines().join("\n").contains("hunter2") && !server.lines().join("\n").contains("AKIA1"));
        let folder = vault_server(&root, &serde_json::json!({ "vault_path": "/notes/vault" })).unwrap();
        assert_eq!(folder.lines()[3..], ["[mcp_servers.vault.env]", "VAULT_PATH = \"/notes/vault\""], "a folder vault has no keys to name");
        let wanted = vec!["S3_ACCESS_KEY".to_string(), "S3_SECRET_KEY".to_string()];
        assert_eq!(vault_secrets(&root, &s3(), &wanted), [("S3_ACCESS_KEY".to_string(), "AKIA1".to_string()), ("S3_SECRET_KEY".to_string(), "hunter2".to_string())]);
        assert!(vault_secrets(&root, &s3(), &[]).is_empty(), "only the names Codex's config asks for");
        assert_eq!(vault_server(&dir.join("missing"), &s3()), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adding_the_vault_server_keeps_the_users_config_toml_as_it_is() {
        let dir = crate::sh::test_dir("codex-vault-edit");
        let server = vault_server(&vault_plugin(&dir), &s3()).unwrap();
        let mine = "# my settings\nmodel = \"gpt-6.1-sol\"   # keep this comment\n\n[projects.\"/src/app\"]\ntrust_level = \"trusted\"\n";
        let added = with_vault(mine, &server).unwrap();
        assert!(added.starts_with(mine), "the user's text stays byte for byte: {added}");
        assert_eq!(added[mine.len()..], format!("\n{}\n", server.lines().join("\n")), "seven lines at the end, after one blank line");
        assert_eq!(vault_placed(Some(mine), &server), Placed::Missing);
        assert_eq!(vault_placed(Some(&added), &server), Placed::Current);
        assert_eq!(with_vault("", &server).unwrap(), server.lines().join("\n") + "\n");
        let other = format!("{mine}\n[mcp_servers.vault]\ncommand = \"uvx\"\nargs = [\"notes-vault-mcp==0.5.0\"]\nstartup_timeout_sec = 30\n\n[mcp_servers.github]\ncommand = \"gh-mcp\"\n");
        assert_eq!(vault_placed(Some(&other), &server), Placed::Differs);
        let replaced = with_vault(&other, &server).unwrap();
        assert!(replaced.starts_with(mine) && replaced.contains("[mcp_servers.github]\ncommand = \"gh-mcp\"") && replaced.contains("notes-vault-mcp==0.8.0") && !replaced.contains("0.5.0"), "{replaced}");
        assert_eq!(vault_placed(Some(&replaced), &server), Placed::Current);
        assert_eq!(wanted_secrets(&replaced.parse().unwrap()), ["S3_ACCESS_KEY", "S3_SECRET_KEY"]);
        assert!(with_vault("model = ", &server).unwrap_err().contains("does not parse"), "a file kebbie cannot read is left alone");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn codexs_vault_server_runs_kebbies_vault_command_and_the_plugins_settings_only_when_it_is_the_plugins_command() {
        let dir = crate::sh::test_dir("codex-vault-command");
        let claude = dir.join("claude");
        let root = vault_plugin(&dir);
        std::fs::create_dir_all(claude.join("plugins")).unwrap();
        std::fs::write(claude.join("plugins/installed_plugins.json"), serde_json::json!({ "plugins": { "vault@gronare": [{ "version": "0.3.0", "installPath": root }] } }).to_string()).unwrap();
        std::fs::write(claude.join("settings.json"), serde_json::json!({ "pluginConfigs": { "vault@gronare": { "options": s3() } } }).to_string()).unwrap();
        let own = server_for(&claude, &["/home/me/.local/bin/mindin".to_string()]).unwrap();
        assert_eq!(own.lines(), ["[mcp_servers.vault]", "command = \"/home/me/.local/bin/mindin\"", "args = []"], "a vault command of its own finds the vault itself");
        assert_eq!(vault_placed(Some("[mcp_servers.vault]\ncommand = \"/home/me/.local/bin/mindin\"\n"), &own), Placed::Current, "the lines mindin's installer writes are the same server");
        let plugins = server_for(&claude, &["uvx".to_string(), "notes-vault-mcp==0.8.0".to_string()]).unwrap();
        assert_eq!(plugins, vault_server(&root, &s3()).unwrap(), "the plugin's own command keeps the plugin's settings");
        assert_eq!(wanted_server(&claude, None), Some(plugins), "without [vault] command, the plugin's server as before");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_codex_task_gets_the_vault_keys_in_its_environment_and_its_commands_never_see_them() {
        let dir = crate::sh::test_dir("codex-vault-session");
        let claude = dir.join("claude");
        let root = vault_plugin(&dir);
        std::fs::create_dir_all(root.join("hooks")).unwrap();
        std::fs::write(root.join("hooks/hooks.json"), "uvx notes-vault-mcp==0.8.0 hook session-start").unwrap();
        std::fs::create_dir_all(claude.join("plugins")).unwrap();
        std::fs::write(claude.join("plugins/installed_plugins.json"), serde_json::json!({ "plugins": { "vault@gronare": [{ "version": "0.2.19", "installPath": root }] } }).to_string()).unwrap();
        std::fs::write(claude.join("settings.json"), serde_json::json!({ "pluginConfigs": { "vault@gronare": { "options": s3() } } }).to_string()).unwrap();
        let server = vault_server(&root, &s3()).unwrap();
        let config: toml::Table = with_vault("", &server).unwrap().parse().unwrap();
        let found = session(&claude, &config);
        assert!(found.vault);
        assert_eq!(found.env, [("S3_ACCESS_KEY".to_string(), "AKIA1".to_string()), ("S3_SECRET_KEY".to_string(), "hunter2".to_string())]);
        assert_eq!(found.hidden, ["-c", "shell_environment_policy.filters.S3_ACCESS_KEY=\"exclude\"", "-c", "shell_environment_policy.filters.S3_SECRET_KEY=\"exclude\""]);
        assert!(found.hidden.iter().all(|arg| !arg.contains("hunter2") && !arg.contains("AKIA1")), "only names go on the command line");
        let legacy: toml::Table = format!("{}\n[shell_environment_policy]\nexclude = [\"AWS_*\"]\n", with_vault("", &server).unwrap()).parse().unwrap();
        assert_eq!(session(&claude, &legacy).hidden, ["-c", "shell_environment_policy.exclude=[\"AWS_*\", \"S3_ACCESS_KEY\", \"S3_SECRET_KEY\"]"], "the user's own exclude list stays, as Codex refuses filters beside it");
        assert_eq!(session(&claude, &toml::Table::new()), Session::default(), "no vault server in Codex, nothing to set");
        let folder: toml::Table = with_vault("", &vault_server(&root, &serde_json::json!({ "vault_path": "/notes" })).unwrap()).unwrap().parse().unwrap();
        assert_eq!(session(&claude, &folder), Session { vault: true, env: Vec::new(), hidden: Vec::new() }, "a folder vault needs no keys");
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn codexs_installer_and_latest_version_come_from_where_codex_itself_reads_them() {
        let dir = crate::sh::test_dir("codex-installer");
        let release = dir.join("home/packages/standalone/releases/0.163.0-aarch64-apple-darwin");
        std::fs::create_dir_all(&release).unwrap();
        crate::executable::write(release.join("codex"), "#!/bin/sh\n").unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::os::unix::fs::symlink(release.join("codex"), dir.join("bin/codex")).unwrap();
        assert_eq!(installer(&dir.join("bin/codex"), &dir.join("home"), true), Some(Installer::Standalone), "the link on PATH points into Codex's own releases");
        let npm = dir.join("lib/node_modules/@openai/codex/bin");
        std::fs::create_dir_all(&npm).unwrap();
        crate::executable::write(npm.join("codex.js"), "#!/usr/bin/env node\n").unwrap();
        assert_eq!(installer(&npm.join("codex.js"), &dir.join("home"), false), Some(Installer::Npm));
        assert_eq!(installer(&dir.join("bin/missing"), &dir.join("home"), true), None);
        assert_eq!(Installer::Standalone.command_line(), "sh -c 'curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh'");
        assert_eq!(Installer::Brew.command_line(), "brew upgrade --cask codex");
        assert_eq!(latest_from(latest_source(Some(Installer::Standalone)), &serde_json::json!({ "tag_name": "rust-v0.164.0" })).as_deref(), Some("0.164.0"));
        assert_eq!(latest_from(latest_source(Some(Installer::Brew)), &serde_json::json!({ "version": "0.164.0" })).as_deref(), Some("0.164.0"));
        assert_eq!(latest_from(latest_source(None), &serde_json::json!({ "tag_name": "v1" })), None);
        assert_eq!((version_word("codex-cli 0.163.0").as_deref(), short_version("0.164.0").as_str(), short_version("0.164.2").as_str()), (Some("0.163.0"), "0.164", "0.164.2"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_model_gone_from_codexs_list_falls_to_its_upgrade_else_to_codexs_default() {
        let home = crate::sh::test_dir("codex-fate");
        std::fs::write(home.join("models_cache.json"), super::super::codex::CACHE_FIXTURE).unwrap();
        let models = super::super::codex::cached_models(&home).unwrap();
        assert_eq!(default_model(&models).map(|model| model.slug.as_str()), Some("gpt-6.1-sol"));
        assert_eq!(fate(&models, "gpt-6-astra"), Fate::Listed);
        assert_eq!(fate(&models, "gpt-5.1-codex-max"), Fate::Gone(Some("gpt-6-astra".to_string())), "a hidden model with an upgrade starts on it");
        assert_eq!(fate(&models, "gpt-4.1"), Fate::Gone(Some("gpt-6.1-sol".to_string())), "an unknown model starts on Codex's default");
        let _ = std::fs::remove_dir_all(home);
    }
}
