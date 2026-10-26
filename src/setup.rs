use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::sh::Result;
use crate::vault_cli::Plugin;

pub const CLAUDE_VAR: &str = "KEBBIE_CLAUDE_BIN";
pub const VAULT_MARKETPLACE: &str = "https://github.com/gronare/claude-plugins.git";
pub const VAULT_PLUGIN: &str = "vault@gronare";

pub fn claude_bin() -> String {
    std::env::var(CLAUDE_VAR).ok().filter(|bin| !bin.is_empty()).unwrap_or_else(|| "claude".to_string())
}

pub fn marketplace_known(claude_home: &Path, name: &str) -> bool {
    std::fs::read_to_string(claude_home.join("plugins/known_marketplaces.json")).ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .is_some_and(|known| known.get(name).is_some())
}

pub fn steps(claude: &str, plugin: &Plugin, marketplace: bool) -> Vec<Vec<String>> {
    let command = |args: &[&str]| std::iter::once(claude.to_string()).chain(args.iter().map(|arg| arg.to_string())).collect::<Vec<_>>();
    let mut steps = Vec::new();
    if !marketplace && matches!(plugin, Plugin::Missing) {
        steps.push(command(&["plugin", "marketplace", "add", VAULT_MARKETPLACE]));
    }
    match plugin {
        Plugin::Missing => steps.push(command(&["plugin", "install", VAULT_PLUGIN])),
        Plugin::Disabled { id, .. } => steps.push(command(&["plugin", "enable", id])),
        Plugin::Enabled { .. } => {}
    }
    steps.push(command(&["plugin", "configure", VAULT_PLUGIN]));
    steps
}

fn configure_text(claude: &str) -> String {
    format!(
        "Point the plugin at your vault, one of:\n  a local folder:  echo '{{\"vault_path\": \"/path/to/vault\"}}' | {claude} plugin configure {VAULT_PLUGIN} --values-stdin\n  an S3 bucket:    set s3_endpoint, s3_bucket, s3_access_key and s3_secret_key the same way, or in Claude Code's /plugin screen, which asks for them.\nThen run kebbie doctor: its vault line says whether kebbie keeps your tasks in the vault."
    )
}

// Each command is printed and runs only on the user's y.
pub fn run_steps(steps: &[Vec<String>], input: &mut impl BufRead, output: &mut impl Write, mut run: impl FnMut(&[String]) -> Result<()>) -> Result<usize> {
    let mut ran = 0;
    for step in steps {
        write!(output, "Run: {}\n  [y/N] ", step.join(" ")).map_err(|error| error.to_string())?;
        output.flush().map_err(|error| error.to_string())?;
        let mut answer = String::new();
        input.read_line(&mut answer).map_err(|error| error.to_string())?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            writeln!(output, "Skipped; nothing after it runs.").map_err(|error| error.to_string())?;
            return Ok(ran);
        }
        run(step)?;
        ran += 1;
    }
    Ok(ran)
}

pub fn vault() -> Result<String> {
    if in_terminal() {
        crate::permit::user_only("kebbie setup vault")?;
        let machine = machine()?;
        return crate::setup_guide::run(&crate::setup_guide::Env::current(&machine)?, crate::setup_guide::Step::Vault);
    }
    let claude = claude_bin();
    let home = crate::agent::claude::home().unwrap_or_default();
    let steps = steps(&claude, &crate::vault_cli::plugin(&home), marketplace_known(&home, "gronare"));
    println!("kebbie setup vault: the vault plugin keeps each task's note and the repos' logs in your Obsidian vault. Optional: kebbie works the same without it.");
    let stdin = std::io::stdin();
    let ran = run_steps(&steps, &mut stdin.lock(), &mut std::io::stdout(), |step| crate::sh::run_visibly(&step[0], &step[1..].iter().map(String::as_str).collect::<Vec<_>>(), None))?;
    if ran < steps.len() {
        return Ok(format!("Ran {ran} of {} steps.", steps.len()));
    }
    Ok(configure_text(&claude))
}

fn yes(question: &str, input: &mut impl BufRead, output: &mut impl Write) -> Result<bool> {
    write!(output, "{question}\n  [y/N] ").map_err(|error| error.to_string())?;
    output.flush().map_err(|error| error.to_string())?;
    let mut answer = String::new();
    input.read_line(&mut answer).map_err(|error| error.to_string())?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Asking {
    Yes,
    Closed,
}

pub struct Machine {
    pub home: PathBuf,
    pub data_home: Option<PathBuf>,
    pub path: Vec<PathBuf>,
    pub claude_dir: PathBuf,
    pub claude: String,
    pub herdr: String,
    pub herdr_config: PathBuf,
    pub follows_github: bool,
}

const EXAMPLE: &str = include_str!("../examples/herdr-config.toml");

fn var(name: &str) -> Option<PathBuf> {
    std::env::var(name).ok().filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub fn herdr_config_path(home: &Path) -> PathBuf {
    var("HERDR_CONFIG_PATH").or_else(|| var("XDG_CONFIG_HOME").map(|dir| dir.join("herdr/config.toml"))).unwrap_or_else(|| home.join(".config/herdr/config.toml"))
}

pub fn machine() -> Result<Machine> {
    let home = var("HOME").ok_or("HOME is not set, so kebbie setup cannot find where to link kebbie")?;
    let channel = crate::config::channel()?;
    let state = crate::lead::state_dir()?;
    Ok(Machine {
        data_home: var("XDG_DATA_HOME"),
        path: std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default(),
        claude_dir: crate::agent::claude::home().unwrap_or_else(|| home.join(".claude")),
        claude: claude_bin(),
        herdr: crate::herdr::bin(),
        herdr_config: herdr_config_path(&home),
        follows_github: channel == crate::config::Channel::Main && crate::release::read_override(&state).is_none(),
        home,
    })
}

// The guide needs a terminal on both ends; a script or a pipe gets the steps that ask nothing.
pub fn in_terminal() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdin()) && std::io::IsTerminal::is_terminal(&std::io::stdout())
}

pub fn install(yes: bool) -> Result<String> {
    crate::permit::user_only("kebbie setup")?;
    let machine = machine()?;
    let env = crate::setup_guide::Env::current(&machine)?;
    if !yes && in_terminal() {
        return crate::setup_guide::run(&env, crate::setup_guide::Step::Name);
    }
    let asking = if yes { Asking::Yes } else { Asking::Closed };
    let rows = crate::setup_guide::without_asking(&env, asking);
    let ending = crate::setup_guide::ending(&rows, None, &crate::doctor::checks(&crate::doctor::Env::current()));
    Ok(rows.iter().map(|(step, outcome)| outcome.line(step)).chain(ending).collect::<Vec<_>>().join("\n"))
}

pub const LATER: &str = "later: H on the board";

#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub verb: &'static str,
    pub detail: String,
}

impl Outcome {
    pub fn new(verb: &'static str, detail: impl Into<String>) -> Self {
        Outcome { verb, detail: detail.into() }
    }

    pub fn line(&self, step: &str) -> String {
        line(self.verb, step, &self.detail)
    }
}

pub fn hub_connected(result: Result<String>, host: Option<String>) -> Outcome {
    match result {
        Ok(said) => {
            let connected = host.map_or("connected".to_string(), |host| format!("connected to {host}"));
            let said = said.lines().filter(|line| !line.trim().is_empty()).collect::<Vec<_>>().join("; ");
            Outcome::new("done", if said.is_empty() { connected } else { format!("{connected}; {said}") })
        }
        Err(error) if error == crate::hub::connect::STOPPED => Outcome::new("skipped", format!("stopped; {LATER}")),
        Err(error) => Outcome::new("failed", error),
    }
}

pub fn line(verb: &str, step: &str, detail: impl std::fmt::Display) -> String {
    format!("{verb:<7}  {step:<9}  {detail}")
}

pub fn link_binary(home: &Path, path: &[PathBuf]) -> Outcome {
    let binary = home.join(".local/share/kebbie/current/target/release/kebbie");
    let bin_dir = home.join(".local/bin");
    let link = bin_dir.join("kebbie");
    let on_path = if path.contains(&bin_dir) { String::new() } else { format!("; {} is not on PATH: add export PATH=\"$HOME/.local/bin:$PATH\" to your shell's profile", bin_dir.display()) };
    if !binary.is_file() {
        return Outcome::new("skipped", format!("{} is missing: restart herdr so kebbie's startup links {}, then run this again", binary.display(), home.join(".local/share/kebbie/current").display()));
    }
    match std::fs::read_link(&link) {
        Ok(target) if target == binary => Outcome::new("already", format!("{} → {}{on_path}", link.display(), binary.display())),
        Ok(target) => Outcome::new("skipped", format!("{} links to {}: remove it and run this again to link {}", link.display(), target.display(), binary.display())),
        Err(_) if std::fs::symlink_metadata(&link).is_ok() => Outcome::new("skipped", format!("{} is a file kebbie did not put there: remove it and run this again", link.display())),
        Err(_) => match std::fs::create_dir_all(&bin_dir).and_then(|_| std::os::unix::fs::symlink(&binary, &link)) {
            Ok(()) => Outcome::new("done", format!("linked {} → {}{on_path}", link.display(), binary.display())),
            Err(error) => Outcome::new("failed", format!("linking {}: {error}", link.display())),
        },
    }
}

pub fn claude_plugin(machine: &Machine) -> Outcome {
    use crate::update::{self, Marketplace};
    let current = machine.home.join(".local/share/kebbie/current");
    let read = |file: &str| std::fs::read_to_string(machine.claude_dir.join(file)).ok().and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok()).unwrap_or_default();
    let installed = read("plugins/installed_plugins.json");
    if let Some(id) = installed["plugins"].as_object().and_then(|plugins| plugins.keys().find(|id| id.starts_with("kebbie@")).cloned()) {
        if read("settings.json")["enabledPlugins"][&id] == false {
            return match crate::sh::run(&machine.claude, &["plugin", "enable", &id], None) {
                Ok(_) => Outcome::new("done", format!("enabled {id} in Claude Code")),
                Err(error) => Outcome::new("failed", error),
            };
        }
        let said = update::claude_plugin(&machine.claude_dir, &machine.claude, Some(&current), machine.follows_github);
        return Outcome::new("already", format!("{id} is installed in Claude Code; {}", said.trim_start_matches("Claude Code plugin: ")));
    }
    if marketplace_known(&machine.claude_dir, "kebbie") {
        if !matches!(update::marketplace(&machine.claude_dir, "kebbie@kebbie"), Marketplace::InPlace(path) if update::same_folder(&path, &current)) {
            return Outcome::new("skipped", format!("Claude Code has a kebbie marketplace that is not {}: {}", current.display(), update::IN_PLACE_FIX));
        }
    } else if let Err(error) = crate::sh::run(&machine.claude, &["plugin", "marketplace", "add", &current.to_string_lossy()], None) {
        return Outcome::new("failed", error);
    }
    match crate::sh::run(&machine.claude, &["plugin", "install", "kebbie@kebbie"], None) {
        Ok(_) => Outcome::new("done", format!("installed kebbie@kebbie in Claude Code from {}", current.display())),
        Err(error) => Outcome::new("failed", error),
    }
}

pub fn kebbie_keys() -> Vec<toml::Table> {
    let example: toml::Table = EXAMPLE.parse().expect("examples/herdr-config.toml is TOML");
    bindings(&example).into_iter().filter(|binding| text(binding, "command").starts_with("kebbie.")).cloned().collect()
}

fn bindings(config: &toml::Table) -> Vec<&toml::Table> {
    config.get("keys").and_then(|keys| keys.get("command")).and_then(toml::Value::as_array).into_iter().flatten().filter_map(toml::Value::as_table).collect()
}

pub fn text<'a>(table: &'a toml::Table, field: &str) -> &'a str {
    table.get(field).and_then(toml::Value::as_str).unwrap_or_default()
}

fn block(binding: &toml::Table) -> String {
    let fields = ["key", "type", "command", "description"].iter().map(|field| format!("{field} = {}\n", serde_json::Value::from(text(binding, field)))).collect::<String>();
    format!("\n[[keys.command]]\n{fields}")
}

fn taken_by(config: &toml::Table, key: &str) -> Option<String> {
    let command = bindings(config).into_iter().find(|binding| text(binding, "key") == key).map(|binding| text(binding, "command").to_string());
    let action = || config.get("keys").and_then(toml::Value::as_table)?.iter().find(|(_, value)| value.as_str() == Some(key)).map(|(name, _)| name.clone());
    command.or_else(action)
}

// Letters herdr 0.9 leaves unbound by default; its own defaults take b c e g h j k l n o p q r s v w x z.
const FREE_KEYS: &str = "yfmdaiu";

#[derive(Clone, Debug, PartialEq)]
pub struct Wanted {
    pub binding: toml::Table,
    pub taken: Option<String>,
    pub free: Option<String>,
}

impl Wanted {
    pub fn key(&self) -> &str {
        text(&self.binding, "key")
    }

    pub fn command(&self) -> &str {
        text(&self.binding, "command")
    }

    pub fn moved(&self) -> Option<toml::Table> {
        let mut binding = self.binding.clone();
        binding.insert("key".to_string(), toml::Value::String(self.free.clone()?));
        Some(binding)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyPlan {
    pub config: PathBuf,
    pub via: String,
    pub blocked: Option<String>,
    pub missing: Vec<Wanted>,
}

pub fn key_plan(config: &Path) -> KeyPlan {
    let resolved = std::fs::canonicalize(config).ok().or_else(|| Some(std::fs::canonicalize(config.parent()?).ok()?.join(config.file_name()?))).unwrap_or_else(|| config.to_path_buf());
    let via = if resolved == config { String::new() } else { format!(" (a link to {})", resolved.display()) };
    let plan = |blocked: Option<String>, missing| KeyPlan { config: config.to_path_buf(), via: via.clone(), blocked, missing };
    let existing = match std::fs::read_to_string(config) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return plan(Some(format!("could not read {}: {error}", config.display())), Vec::new()),
    };
    let table: toml::Table = match existing.parse() {
        Ok(table) => table,
        Err(error) => return plan(Some(format!("{} is not valid TOML, so kebbie leaves it alone: {}", config.display(), error.message())), Vec::new()),
    };
    let wanted = kebbie_keys();
    let ours: Vec<String> = wanted.iter().map(|binding| text(binding, "key").to_string()).collect();
    let missing = wanted.into_iter().filter(|wanted| !bindings(&table).iter().any(|binding| text(binding, "command") == text(wanted, "command"))).map(|binding| {
        let taken = taken_by(&table, text(&binding, "key"));
        let free = taken.as_ref().and_then(|_| FREE_KEYS.chars().map(|key| format!("prefix+{key}")).find(|key| taken_by(&table, key).is_none() && !ours.contains(key)));
        Wanted { binding, taken, free }
    }).collect();
    plan(None, missing)
}

// The file is appended to, never rewritten, so every byte the user wrote stays as it was.
pub fn add_keys(plan: &KeyPlan, keys: &[toml::Table]) -> (Outcome, bool) {
    let config = &plan.config;
    let left: Vec<String> = plan.missing.iter().filter(|wanted| !keys.iter().any(|binding| text(binding, "command") == wanted.command()))
        .map(|wanted| format!("{} is already {}, so {} gets no key", wanted.key(), wanted.taken.clone().unwrap_or_default(), wanted.command())).collect();
    let left = if left.is_empty() { String::new() } else { format!("; {}: bind it by hand in [[keys.command]]", left.join("; ")) };
    if keys.is_empty() {
        return (Outcome::new("skipped", left.trim_start_matches("; ").to_string()), false);
    }
    let existing = std::fs::read_to_string(config).unwrap_or_default();
    let shown = keys.iter().map(|binding| text(binding, "key")).collect::<Vec<_>>().join(", ");
    let separator = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
    let addition = format!("{separator}{}", keys.iter().map(block).collect::<String>());
    if let Err(error) = format!("{existing}{addition}").parse::<toml::Table>() {
        return (Outcome::new("skipped", format!("{} cannot take [[keys.command]] at its end ({}); add {shown} by hand", config.display(), error.message())), false);
    }
    let written = config.parent().filter(|parent| !parent.as_os_str().is_empty()).map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(config))
        .and_then(|mut file| file.write_all(addition.as_bytes()));
    match written {
        Ok(()) => (Outcome::new("done", format!("added {shown} to {}{}{left}", config.display(), plan.via)), true),
        Err(error) => (Outcome::new("failed", format!("writing {}: {error}", config.display())), false),
    }
}

pub fn keys_without_asking(plan: &KeyPlan, asking: Asking) -> (Outcome, bool) {
    if let Some(blocked) = &plan.blocked {
        return (Outcome::new("skipped", blocked.clone()), false);
    }
    if plan.missing.is_empty() {
        return (Outcome::new("already", format!("{} binds kebbie's keys", plan.config.display())), false);
    }
    let free: Vec<toml::Table> = plan.missing.iter().filter(|wanted| wanted.taken.is_none()).map(|wanted| wanted.binding.clone()).collect();
    let shown = free.iter().map(|binding| text(binding, "key")).collect::<Vec<_>>().join(", ");
    match asking {
        _ if free.is_empty() => add_keys(plan, &free),
        Asking::Closed => (Outcome::new("skipped", format!("no terminal to ask in before writing {}{}: run kebbie setup --yes to add {shown}", plan.config.display(), plan.via)), false),
        Asking::Yes if !plan.via.is_empty() => (Outcome::new("skipped", format!("{}{} is a link, which --yes never writes through: run kebbie setup in a terminal to be asked, or add {shown} by hand", plan.config.display(), plan.via)), false),
        Asking::Yes => add_keys(plan, &free),
    }
}

pub fn herdr_reload(machine: &Machine, wrote: bool) -> String {
    if !wrote {
        return String::new();
    }
    let current = crate::doctor::current_link(&machine.home, machine.data_home.as_deref(), None);
    if current.level != crate::doctor::Level::Ok {
        return format!("; restart herdr: {}, and kebbie's startup in herdr links it", current.fact);
    }
    match crate::sh::run(&machine.herdr, &["server", "reload-config"], None) {
        Ok(_) => "; herdr reloaded its config, so the keys work now".to_string(),
        Err(_) => "; restart herdr so it reads the new keys".to_string(),
    }
}

pub fn codex(uninstall: bool) -> Result<String> {
    crate::permit::user_only("kebbie setup codex")?;
    if !uninstall && in_terminal() {
        let machine = machine()?;
        return crate::setup_guide::run(&crate::setup_guide::Env::current(&machine)?, crate::setup_guide::Step::Agents);
    }
    let home = crate::agent::codex::home().ok_or("Neither CODEX_HOME nor HOME is set, so kebbie cannot find Codex's home")?;
    let kebbie = crate::peers::kebbie_command()?;
    let stdin = std::io::stdin();
    codex_in(&home, &kebbie, uninstall, &mut stdin.lock(), &mut std::io::stdout())
}

// Codex's plugin cache is the one place kebbie writes in Codex's home, and only on the user's y; config.toml and hooks.json stay theirs.
pub fn codex_in(home: &Path, kebbie: &str, uninstall: bool, input: &mut impl BufRead, output: &mut impl Write) -> Result<String> {
    use crate::agent::codex::{self, Installed};
    let root = codex::plugin_root(home);
    if uninstall {
        if !root.exists() {
            return Ok(format!("kebbie's Codex plugin is not installed; {} is not there.", root.display()));
        }
        if !yes(&format!("kebbie setup codex --uninstall removes kebbie's plugin from Codex: {}", root.display()), input, output)? {
            return Ok("Skipped; nothing was removed.".to_string());
        }
        codex::uninstall(home)?;
        return Ok(format!("Removed {}. Tasks whose profile runs Codex now start without kebbie's hooks, so they neither report nor get their role.", root.display()));
    }
    let state = codex::installed(home, kebbie);
    if state == Installed::Current {
        return Ok(format!("kebbie's Codex plugin is installed and current at {}; nothing to do.", root.display()));
    }
    let verb = if state == Installed::Missing { "install" } else { "update" };
    let question = format!(
        "kebbie setup codex: {verb} kebbie's plugin in Codex's plugin cache, {}.\nIts three hooks (SessionStart, UserPromptSubmit, Stop) record the session, give it its role and ask for kebbie report before a turn ends; outside a kebbie task they do nothing.\nkebbie enables the plugin, and trusts those three hooks by their hash, only in the Codex sessions it starts (-c on its command line). It never edits {} or {}.",
        root.display(), home.join("config.toml").display(), home.join("hooks.json").display(),
    );
    if !yes(&question, input, output)? {
        return Ok("Skipped; nothing was written.".to_string());
    }
    let written = codex::install(home, kebbie)?;
    let files = written.iter().map(|file| format!("  {}", file.display())).collect::<Vec<_>>().join("\n");
    Ok(format!("Wrote:\n{files}\nkebbie doctor's codex lines say whether Codex and its hooks are ready."))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn setup_codex_writes_only_on_a_yes_and_says_what_it_leaves_alone() {
        let home = crate::sh::test_dir("setup-codex");
        std::fs::write(home.join("config.toml"), "model = \"gpt-5.5\"\n").unwrap();
        let run = |answers: &str, uninstall: bool| {
            let mut output = Vec::new();
            let said = codex_in(&home, "/opt/kebbie", uninstall, &mut answers.as_bytes(), &mut output).unwrap();
            (said, String::from_utf8(output).unwrap())
        };
        let (said, asked) = run("", false);
        assert_eq!(said, "Skipped; nothing was written.");
        assert!(asked.contains("install kebbie's plugin in Codex's plugin cache") && asked.contains(&format!("It never edits {}", home.join("config.toml").display())), "{asked}");
        assert!(!home.join("plugins").exists(), "no answer is no");
        let (said, _) = run("y\n", false);
        assert!(said.starts_with("Wrote:\n") && said.contains("hooks/hooks.json"), "{said}");
        let (said, asked) = run("", false);
        assert!(said.contains("installed and current") && asked.is_empty(), "a second setup asks nothing and writes nothing");
        assert_eq!(std::fs::read_to_string(home.join("config.toml")).unwrap(), "model = \"gpt-5.5\"\n");
        assert!(!home.join("hooks.json").exists(), "Codex's own hooks file is never written");
        assert_eq!(run("n\n", true).0, "Skipped; nothing was removed.");
        assert!(run("y\n", true).0.starts_with("Removed "));
        assert!(run("", true).0.contains("is not installed"));
        std::fs::remove_dir_all(home).unwrap();
    }

    fn keys(config: &Path, asking: Asking) -> (String, bool) {
        let (outcome, wrote) = keys_without_asking(&key_plan(config), asking);
        (outcome.line("keys"), wrote)
    }

    fn commands(config: &Path) -> Vec<String> {
        let table: toml::Table = std::fs::read_to_string(config).unwrap().parse().unwrap();
        bindings(&table).iter().map(|binding| format!("{} {}", text(binding, "key"), text(binding, "command"))).collect()
    }

    #[test]
    fn the_keys_kebbie_adds_are_the_ones_the_example_config_binds() {
        let keys: Vec<String> = kebbie_keys().iter().map(|binding| format!("{} {}", text(binding, "key"), text(binding, "command"))).collect();
        assert_eq!(keys, ["prefix+space kebbie.lead", "prefix+tab kebbie.focus", "prefix+t kebbie.board"], "updating is U on the board");
    }

    #[test]
    fn herdr_keys_are_appended_only_where_missing_and_the_rest_stays_byte_identical() {
        let dir = crate::sh::test_dir("setup-keys");
        let config = dir.join("herdr/config.toml");
        let (said, wrote) = keys(&config, Asking::Yes);
        assert!(wrote && said.starts_with("done") && said.contains("prefix+space, prefix+tab, prefix+t "), "{said}");
        assert_eq!(commands(&config).len(), 3, "a missing config is created with the three keys");
        let written = std::fs::read(&config).unwrap();
        let (said, wrote) = keys(&config, Asking::Yes);
        assert!(!wrote && said.starts_with("already"), "{said}");
        assert_eq!(std::fs::read(&config).unwrap(), written, "a second run writes nothing");

        let own = "onboarding = false # mine\n\n[[keys.command]]\nkey = \"prefix+t\"\ntype = \"plugin_action\"\ncommand = \"kebbie.board\"\n\n[ui]\npane_borders  =  \"auto\"";
        std::fs::write(&config, own).unwrap();
        let (said, wrote) = keys(&config, Asking::Yes);
        assert!(wrote && said.contains("added prefix+space, prefix+tab to"), "{said}");
        let after = std::fs::read_to_string(&config).unwrap();
        assert!(after.starts_with(own) && after[own.len()..].starts_with("\n\n[[keys.command]]"), "the user's bytes stay a prefix, and a missing last newline is added before kebbie's: {after}");
        assert_eq!(commands(&config), ["prefix+t kebbie.board", "prefix+space kebbie.lead", "prefix+tab kebbie.focus"]);
        assert!(keys(&config, Asking::Yes).0.starts_with("already"));
        assert_eq!(std::fs::read_to_string(&config).unwrap(), after);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_key_the_user_bound_to_something_else_is_named_with_a_free_one_to_use_instead() {
        let dir = crate::sh::test_dir("setup-taken");
        let config = dir.join("config.toml");
        let own = "[keys]\ncycle_pane_next = \"prefix+tab\"\n\n[[keys.command]]\nkey = \"prefix+t\"\ntype = \"popup\"\ncommand = \"lazygit\"\n\n[[keys.command]]\nkey = \"prefix+y\"\ntype = \"popup\"\ncommand = \"btop\"\n";
        std::fs::write(&config, own).unwrap();
        let plan = key_plan(&config);
        let taken: Vec<(String, Option<String>, Option<String>)> = plan.missing.iter().map(|wanted| (wanted.key().to_string(), wanted.taken.clone(), wanted.free.clone())).collect();
        assert_eq!(taken, [
            ("prefix+space".to_string(), None, None),
            ("prefix+tab".to_string(), Some("cycle_pane_next".to_string()), Some("prefix+f".to_string())),
            ("prefix+t".to_string(), Some("lazygit".to_string()), Some("prefix+f".to_string())),
        ], "prefix+y is btop's, so the next free key is offered");
        let (said, wrote) = keys(&config, Asking::Yes);
        assert!(wrote && said.contains("added prefix+space to") && said.contains("prefix+tab is already cycle_pane_next, so kebbie.focus gets no key") && said.contains("prefix+t is already lazygit"), "{said}");
        assert_eq!(commands(&config), ["prefix+t lazygit", "prefix+y btop", "prefix+space kebbie.lead"]);
        let moved = plan.missing[2].moved().unwrap();
        let (outcome, _) = add_keys(&key_plan(&config), &[moved]);
        assert!(outcome.verb == "done" && outcome.detail.contains("added prefix+f to"), "{outcome:?}");
        assert_eq!(commands(&config).last().map(String::as_str), Some("prefix+f kebbie.board"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn herdr_keys_are_never_written_without_a_terminal_and_a_broken_file_is_left_alone() {
        let dir = crate::sh::test_dir("setup-ask");
        let config = dir.join("config.toml");
        std::fs::write(&config, "onboarding = false\n").unwrap();
        let (said, wrote) = keys(&config, Asking::Closed);
        assert!(!wrote && said.contains("no terminal to ask in") && said.contains("kebbie setup --yes"), "{said}");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "onboarding = false\n");
        let broken = "onboarding = \n";
        std::fs::write(&config, broken).unwrap();
        let (said, wrote) = keys(&config, Asking::Yes);
        assert!(!wrote && said.contains("is not valid TOML"), "{said}");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), broken);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_linked_herdr_config_is_never_written_through_by_yes_and_is_named() {
        let dir = crate::sh::test_dir("setup-link");
        let target = dir.join("dotfiles/herdr.toml");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "onboarding = false\n").unwrap();
        let config = dir.join("config.toml");
        std::os::unix::fs::symlink(&target, &config).unwrap();
        let (said, wrote) = keys(&config, Asking::Yes);
        assert!(!wrote && said.contains(&format!("(a link to {})", target.display())) && said.contains("--yes never writes through"), "{said}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "onboarding = false\n");
        let plan = key_plan(&config);
        let (outcome, wrote) = add_keys(&plan, &plan.missing.iter().map(|wanted| wanted.binding.clone()).collect::<Vec<_>>());
        assert!(wrote && outcome.detail.contains(&target.display().to_string()), "the guide writes through a link it named: {outcome:?}");
        assert!(std::fs::symlink_metadata(&config).unwrap().file_type().is_symlink(), "the link stays a link");
        assert_eq!(commands(&target).len(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    pub(crate) fn machine(dir: &Path) -> (Machine, PathBuf) {
        let home = dir.join("home");
        let root = dir.join("herdr-install");
        std::fs::create_dir_all(root.join("target/release")).unwrap();
        std::fs::write(root.join("target/release/kebbie"), "").unwrap();
        std::fs::create_dir_all(home.join(".local/share/kebbie")).unwrap();
        std::os::unix::fs::symlink(&root, home.join(".local/share/kebbie/current")).unwrap();
        let claude_dir = home.join(".claude");
        std::fs::create_dir_all(claude_dir.join("plugins")).unwrap();
        let log = dir.join("claude.log");
        let plugins = claude_dir.join("plugins");
        let script = format!(
            "#!/bin/sh\n[ \"$1\" = --version ] && {{ echo '2.1.3 (Claude Code)'; exit 0; }}\necho \"$*\" >> '{log}'\ncase \"$*\" in\n  *--values-stdin) cat > '{stdin}' ;;\n  'plugin marketplace add '*) printf '{{\"kebbie\":{{\"source\":{{\"source\":\"directory\",\"path\":\"%s\"}}}}}}' \"$4\" > '{plugins}/known_marketplaces.json' ;;\n  'plugin install kebbie@kebbie') printf '{{\"plugins\":{{\"kebbie@kebbie\":[{{\"version\":\"0.26.6\"}}]}}}}' > '{plugins}/installed_plugins.json' ;;\nesac\n",
            log = log.display(), plugins = plugins.display(), stdin = dir.join("claude.stdin").display(),
        );
        crate::executable::write(dir.join("claude"), script).unwrap();
        let machine = Machine {
            data_home: None,
            path: vec![home.join(".local/bin")],
            claude_dir,
            claude: dir.join("claude").display().to_string(),
            herdr: "false".to_string(),
            herdr_config: home.join(".config/herdr/config.toml"),
            follows_github: false,
            home,
        };
        (machine, log)
    }

    #[test]
    fn setup_leaves_a_kebbie_on_path_it_did_not_link_and_says_when_bin_is_not_on_path() {
        let dir = crate::sh::test_dir("setup-path");
        let (mut machine, log) = machine(&dir);
        machine.path = Vec::new();
        let said = link_binary(&machine.home, &machine.path);
        assert!(said.verb == "done" && said.detail.contains("is not on PATH: add export PATH=\"$HOME/.local/bin:$PATH\""), "{said:?}");
        std::fs::remove_file(machine.home.join(".local/bin/kebbie")).unwrap();
        std::os::unix::fs::symlink("/opt/old/kebbie", machine.home.join(".local/bin/kebbie")).unwrap();
        assert!(link_binary(&machine.home, &machine.path).detail.contains("links to /opt/old/kebbie: remove it"));
        assert_eq!(std::fs::read_link(machine.home.join(".local/bin/kebbie")).unwrap(), PathBuf::from("/opt/old/kebbie"));
        let current = machine.home.join(".local/share/kebbie/current");
        assert_eq!(claude_plugin(&machine).verb, "done");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), format!("plugin marketplace add {}\nplugin install kebbie@kebbie\n", current.display()));
        let again = claude_plugin(&machine);
        assert_eq!(again.verb, "already");
        assert!(std::fs::read_to_string(&log).unwrap().ends_with("plugin marketplace update kebbie\nplugin update kebbie@kebbie\n"), "a second run updates the plugin as kebbie update does");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_hub_connect_reads_as_done_stopped_or_failed() {
        assert_eq!(hub_connected(Ok(String::new()), Some("hub.kebbie.dev".to_string())), Outcome::new("done", "connected to hub.kebbie.dev"));
        assert_eq!(hub_connected(Ok("[machine] name set\n".to_string()), None), Outcome::new("done", "connected; [machine] name set"));
        assert_eq!(hub_connected(Err(crate::hub::connect::STOPPED.to_string()), None), Outcome::new("skipped", "stopped; later: H on the board"));
        assert_eq!(hub_connected(Err("Cancelled".to_string()), None).verb, "failed");
    }

    fn steps_for(plugin: &Plugin, marketplace: bool) -> Vec<String> {
        steps("/fake/claude", plugin, marketplace).iter().map(|step| step.join(" ")).collect()
    }

    #[test]
    fn setup_adds_the_marketplace_installs_the_plugin_and_shows_its_options_with_the_given_claude() {
        assert_eq!(steps_for(&Plugin::Missing, false), [
            "/fake/claude plugin marketplace add https://github.com/gronare/claude-plugins.git",
            "/fake/claude plugin install vault@gronare",
            "/fake/claude plugin configure vault@gronare",
        ]);
        assert_eq!(steps_for(&Plugin::Missing, true)[0], "/fake/claude plugin install vault@gronare", "a known marketplace is not added again");
        assert_eq!(steps_for(&Plugin::Disabled { id: "vault@gronare".to_string(), version: "0.2.15".to_string() }, true)[0], "/fake/claude plugin enable vault@gronare");
        assert_eq!(steps_for(&Plugin::Enabled { id: "vault@gronare".to_string(), version: "0.2.15".to_string(), root: "/x".into() }, true), ["/fake/claude plugin configure vault@gronare"]);
    }

    #[test]
    fn setup_runs_nothing_without_a_yes_and_each_step_only_after_its_own() {
        let steps = steps("/fake/claude", &Plugin::Missing, false);
        let run = |answers: &str| {
            let mut ran = Vec::new();
            let mut output = Vec::new();
            let count = run_steps(&steps, &mut answers.as_bytes(), &mut output, |step| { ran.push(step.join(" ")); Ok(()) }).unwrap();
            (count, ran, String::from_utf8(output).unwrap())
        };
        let (count, ran, shown) = run("");
        assert_eq!((count, ran.len()), (0, 0), "no answer is no");
        assert!(shown.starts_with("Run: /fake/claude plugin marketplace add https://github.com/gronare/claude-plugins.git\n  [y/N] "), "{shown}");
        assert_eq!(run("n\n").1.len(), 0);
        let (count, ran, _) = run("y\nn\n");
        assert_eq!((count, ran), (1, vec!["/fake/claude plugin marketplace add https://github.com/gronare/claude-plugins.git".to_string()]));
        assert_eq!(run("y\ny\ny\n").0, 3);
    }
}
