use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::vault_cli::Probe;

pub const ON_PATH: [&str; 2] = ["mindin", "notes-vault-mcp"];

// Where setup found a vault command, which decides the command kebbie runs: a mindin first, then kebbie's own, then the Claude plugin's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Origin {
    Mindin,
    Kept,
    Plugin,
    Other,
}

// A vault command setup found: where, the words kebbie would run, and what it answered.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub source: String,
    pub command: Vec<String>,
    pub answer: Option<Probe>,
    pub origin: Origin,
}

impl Found {
    pub fn shown(&self) -> String {
        self.command.join(" ")
    }

    pub fn answered(&self) -> String {
        match &self.answer {
            Some(probe) if !probe.name.is_empty() => format!("{} {}", probe.name, probe.version),
            Some(probe) => format!("a vault {}", probe.version),
            None => "did not answer".to_string(),
        }
    }
}

pub struct Places<'a> {
    pub claude_dir: &'a Path,
    pub codex_home: Option<&'a Path>,
    pub cursor_dir: &'a Path,
    pub path: &'a [PathBuf],
    pub local_bin: &'a Path,
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok())
}

fn words(server: &Value) -> Option<Vec<String>> {
    let command = server["command"].as_str().filter(|command| !command.trim().is_empty())?;
    Some(std::iter::once(command.to_string()).chain(server["args"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string)).collect())
}

fn servers(config: &Value) -> Vec<(String, Vec<String>)> {
    config.as_object().into_iter().flatten().filter_map(|(name, server)| Some((name.clone(), words(server)?))).collect()
}

// Each MCP server in Codex's config.toml, Cursor's mcp.json, the vault commands on PATH and a mindin in ~/.local/bin; any of them may run the vault, whatever its name.
fn asked(places: &Places) -> Vec<(Origin, String, Vec<String>)> {
    let codex = places.codex_home.map(|home| home.join("config.toml")).and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|config| serde_json::to_value(config.get("mcp_servers")?).ok()).unwrap_or_default();
    let cursor = read_json(&places.cursor_dir.join("mcp.json")).map(|config| config["mcpServers"].clone()).unwrap_or_default();
    let origin = |name: &str| if name == "mindin" { Origin::Mindin } else { Origin::Other };
    let mut asked: Vec<(Origin, String, Vec<String>)> = ON_PATH.into_iter().filter(|name| crate::doctor::which(name, places.path).is_some()).map(|name| (origin(name), format!("{name} on PATH"), vec![name.to_string()])).collect();
    if crate::doctor::which("mindin", places.path).is_none() {
        asked.extend(crate::doctor::which("mindin", &[places.local_bin.to_path_buf()]).map(|mindin| (Origin::Mindin, "mindin in ~/.local/bin".to_string(), vec![mindin.to_string_lossy().to_string()])));
    }
    asked.extend(servers(&codex).into_iter().map(|(name, command)| (Origin::Other, format!("Codex's [mcp_servers.{name}]"), command)));
    asked.extend(servers(&cursor).into_iter().map(|(name, command)| (Origin::Other, format!("Cursor's mcp.json server {name}"), command)));
    asked
}

// The Claude plugin's server is the command Claude Code starts, so it is offered whatever it answers; every other one only when it answers as a vault.
pub fn candidates(places: &Places, probe: impl Fn(&[String]) -> Option<Probe> + Sync) -> Vec<Found> {
    let plugin = crate::vault_cli::installed(places.claude_dir).and_then(|(id, _, root)| Some((format!("the Claude Code mindin plugin {id}"), crate::vault_cli::server_command(&root)?)));
    let asked: Vec<(Origin, String, Vec<String>)> = plugin.into_iter().map(|(source, command)| (Origin::Plugin, source, command)).chain(asked(places)).collect();
    let answers = crate::sh::in_parallel(&asked, 4, |(_, _, command)| probe(command));
    let mut found: Vec<Found> = Vec::new();
    for ((origin, source, command), answer) in asked.into_iter().zip(answers) {
        let unique = !found.iter().any(|seen| seen.command == command);
        let runnable = !command.iter().any(|word| word.chars().any(char::is_whitespace));
        if unique && runnable && (origin == Origin::Plugin || answer.is_some()) {
            found.push(Found { source, command, answer, origin });
        }
    }
    found
}

// Runs the command as setup found it, its program looked up on the given PATH; one that does not answer status with a version is no vault.
pub fn probe_on(path: &[PathBuf], command: &[String]) -> Option<Probe> {
    let (program, rest) = command.split_first()?;
    let program = if program.contains('/') { PathBuf::from(program) } else { crate::doctor::which(program, path)? };
    let cli = crate::vault_cli::Cli::new(std::iter::once(program.to_string_lossy().to_string()).chain(rest.iter().cloned()).collect());
    let probe = cli.probe(false).ok().filter(|probe| !probe.version.is_empty())?;
    Some(crate::vault_cli::Probe { setup: cli.offers("setup"), ..probe })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(version: &str) -> Probe {
        Probe { name: "mindin".to_string(), version: version.to_string(), configured: true, ..Probe::default() }
    }

    #[test]
    fn setup_finds_the_vault_in_the_claude_plugin_codex_cursor_and_on_path_by_what_answers() {
        let dir = crate::sh::test_dir("vault-found");
        let claude = dir.join("claude");
        let root = claude.join("plugins/cache/gronare/vault/0.3.0");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".mcp.json"), r#"{"mcpServers":{"vault":{"command":"uvx","args":["mindin==0.14.0"],"env":{"VAULT_PATH":"${user_config.vault_path}"}}}}"#).unwrap();
        std::fs::write(claude.join("plugins/installed_plugins.json"), serde_json::json!({ "plugins": { "vault@gronare": [{ "installPath": root, "version": "0.3.0" }] } }).to_string()).unwrap();
        let codex = dir.join("codex");
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(codex.join("config.toml"), "[mcp_servers.notes]\ncommand = \"uv\"\nargs = [\"run\", \"--project\", \"/src/mindin\", \"mindin\"]\n\n[mcp_servers.github]\ncommand = \"gh-mcp\"\n\n[mcp_servers.same]\ncommand = \"uvx\"\nargs = [\"mindin==0.14.0\"]\n").unwrap();
        let cursor = dir.join("cursor");
        std::fs::create_dir_all(&cursor).unwrap();
        std::fs::write(cursor.join("mcp.json"), r#"{"mcpServers":{"brain":{"command":"/opt/vault/bin/mindin"},"remote":{"url":"https://example.com/mcp"}}}"#).unwrap();
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        crate::executable::write(bin.join("notes-vault-mcp"), "#!/bin/sh\n").unwrap();
        let path = [bin];
        let local_bin = dir.join("local-bin");
        std::fs::create_dir_all(&local_bin).unwrap();
        crate::executable::write(local_bin.join("mindin"), "#!/bin/sh\n").unwrap();
        let places = Places { claude_dir: &claude, codex_home: Some(&codex), cursor_dir: &cursor, path: &path, local_bin: &local_bin };
        let asked = std::sync::Mutex::new(Vec::new());
        let found = candidates(&places, |command| {
            asked.lock().unwrap().push(command.join(" "));
            (command[0] != "gh-mcp" && command[0] != "uvx").then(|| vault("0.14.0"))
        });
        let shown: Vec<(String, String, Origin)> = found.iter().map(|found| (found.source.clone(), found.shown(), found.origin)).collect();
        let mindin = local_bin.join("mindin").display().to_string();
        assert_eq!(shown, [
            ("the Claude Code mindin plugin vault@gronare".to_string(), "uvx mindin==0.14.0".to_string(), Origin::Plugin),
            ("notes-vault-mcp on PATH".to_string(), "notes-vault-mcp".to_string(), Origin::Other),
            ("mindin in ~/.local/bin".to_string(), mindin, Origin::Mindin),
            ("Codex's [mcp_servers.notes]".to_string(), "uv run --project /src/mindin mindin".to_string(), Origin::Other),
            ("Cursor's mcp.json server brain".to_string(), "/opt/vault/bin/mindin".to_string(), Origin::Other),
        ], "a server that does not answer as a vault is left out, a command found twice is kept once, the plugin's is kept even unanswered, and a mindin off PATH is found in ~/.local/bin");
        assert!(asked.lock().unwrap().contains(&"gh-mcp".to_string()), "a server is known by its answer, not its name");
        assert_eq!(found[0].answered(), "did not answer");
        assert_eq!(found[1].answered(), "mindin 0.14.0");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_command_whose_program_is_not_on_the_path_is_not_run() {
        assert_eq!(probe_on(&[], &["uvx".to_string(), "mindin".to_string()]), None);
    }
}
