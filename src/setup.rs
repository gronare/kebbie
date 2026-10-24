use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::sh::Result;
use crate::vault_cli::Plugin;

pub const CLAUDE_VAR: &str = "KEBBIE_CLAUDE_BIN";
const MARKETPLACE: &str = "https://github.com/gronare/claude-plugins.git";
const PLUGIN: &str = "vault@gronare";

pub fn claude_bin() -> String {
    std::env::var(CLAUDE_VAR).ok().filter(|bin| !bin.is_empty()).unwrap_or_else(|| "claude".to_string())
}

fn marketplace_known(claude_home: &Path, name: &str) -> bool {
    std::fs::read_to_string(claude_home.join("plugins/known_marketplaces.json")).ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .is_some_and(|known| known.get(name).is_some())
}

pub fn steps(claude: &str, plugin: &Plugin, marketplace: bool) -> Vec<Vec<String>> {
    let command = |args: &[&str]| std::iter::once(claude.to_string()).chain(args.iter().map(|arg| arg.to_string())).collect::<Vec<_>>();
    let mut steps = Vec::new();
    if !marketplace && matches!(plugin, Plugin::Missing) {
        steps.push(command(&["plugin", "marketplace", "add", MARKETPLACE]));
    }
    match plugin {
        Plugin::Missing => steps.push(command(&["plugin", "install", PLUGIN])),
        Plugin::Disabled { id, .. } => steps.push(command(&["plugin", "enable", id])),
        Plugin::Enabled { .. } => {}
    }
    steps.push(command(&["plugin", "configure", PLUGIN]));
    steps
}

fn configure_text(claude: &str) -> String {
    format!(
        "Point the plugin at your vault, one of:\n  a local folder:  echo '{{\"vault_path\": \"/path/to/vault\"}}' | {claude} plugin configure {PLUGIN} --values-stdin\n  an S3 bucket:    set s3_endpoint, s3_bucket, s3_access_key and s3_secret_key the same way, or in Claude Code's /plugin screen, which asks for them.\nThen run kebbie doctor: its vault line says whether kebbie keeps your tasks in the vault."
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
    Terminal,
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

pub fn install(yes: bool) -> Result<String> {
    crate::permit::user_only("kebbie setup")?;
    let home = var("HOME").ok_or("HOME is not set, so kebbie setup cannot find where to link kebbie")?;
    let channel = crate::config::channel()?;
    let state = crate::lead::state_dir()?;
    let machine = Machine {
        data_home: var("XDG_DATA_HOME"),
        path: std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default(),
        claude_dir: crate::agent::claude::home().unwrap_or_else(|| home.join(".claude")),
        claude: claude_bin(),
        herdr: crate::herdr::bin(),
        herdr_config: herdr_config_path(&home),
        follows_github: channel == crate::config::Channel::Main && crate::release::read_override(&state).is_none(),
        home,
    };
    let asking = if yes { Asking::Yes } else if std::io::IsTerminal::is_terminal(&std::io::stdin()) { Asking::Terminal } else { Asking::Closed };
    let stdin = std::io::stdin();
    install_on(&machine, asking, &mut stdin.lock(), &mut std::io::stdout())?;
    let connected = crate::hub::identity::load(&state).ok().flatten().is_some();
    let offered = hub_offer(asking, connected, &mut stdin.lock(), &mut std::io::stdout())?;
    println!("{}", offered.unwrap_or_else(|| hub_connected(crate::hub::cli(Some("connect"), &[], None))));
    Ok(crate::doctor::summary(&crate::doctor::checks(&crate::doctor::Env::current())))
}

const HUB_STEP: &str = "kebbie hub";

// None means the user said yes; the connect reads the terminal itself, so it runs after this lock on stdin is gone.
pub fn hub_offer(asking: Asking, connected: bool, input: &mut impl BufRead, output: &mut impl Write) -> Result<Option<String>> {
    if connected {
        return Ok(Some(line("already", HUB_STEP, "connected; kebbie hub status shows it")));
    }
    Ok(match asking {
        Asking::Closed => Some(line("skipped", HUB_STEP, "no terminal to ask in: kebbie hub connect shows the board on your phone")),
        Asking::Yes => Some(line("skipped", HUB_STEP, "kebbie hub connect shows the board on your phone when you want it")),
        Asking::Terminal if !yes("Connect a hub now, to see the board on your phone? (kebbie hub connect)", input, output)? => Some(line("skipped", HUB_STEP, "kebbie hub connect, or H on the board, connects one later")),
        Asking::Terminal => None,
    })
}

pub fn hub_connected(result: Result<String>) -> String {
    match result {
        Ok(said) => line("done", HUB_STEP, said.replace('\n', " ")),
        Err(error) => line("failed", HUB_STEP, error),
    }
}

pub fn install_on(machine: &Machine, asking: Asking, input: &mut impl BufRead, output: &mut impl Write) -> Result<()> {
    let say = |output: &mut dyn Write, line: String| writeln!(output, "{line}").map_err(|error| error.to_string());
    say(output, link_binary(&machine.home, &machine.path))?;
    say(output, claude_plugin(machine))?;
    let (keys, wrote) = herdr_keys(&machine.herdr_config, asking, input, output)?;
    say(output, keys)?;
    say(output, herdr_restart(machine, wrote))
}

fn line(verb: &str, step: &str, detail: impl std::fmt::Display) -> String {
    format!("{verb:<7}  {step:<18}  {detail}")
}

fn link_binary(home: &Path, path: &[PathBuf]) -> String {
    const STEP: &str = "kebbie on PATH";
    let binary = home.join(".local/share/kebbie/current/target/release/kebbie");
    let bin_dir = home.join(".local/bin");
    let link = bin_dir.join("kebbie");
    let on_path = if path.contains(&bin_dir) { String::new() } else { format!("; {} is not on PATH: add export PATH=\"$HOME/.local/bin:$PATH\" to your shell's profile", bin_dir.display()) };
    if !binary.is_file() {
        return line("skipped", STEP, format!("{} is missing: restart herdr so kebbie's startup links {}, then run this again", binary.display(), home.join(".local/share/kebbie/current").display()));
    }
    match std::fs::read_link(&link) {
        Ok(target) if target == binary => line("already", STEP, format!("{} → {}{on_path}", link.display(), binary.display())),
        Ok(target) => line("skipped", STEP, format!("{} links to {}: remove it and run this again to link {}", link.display(), target.display(), binary.display())),
        Err(_) if std::fs::symlink_metadata(&link).is_ok() => line("skipped", STEP, format!("{} is a file kebbie did not put there: remove it and run this again", link.display())),
        Err(_) => match std::fs::create_dir_all(&bin_dir).and_then(|_| std::os::unix::fs::symlink(&binary, &link)) {
            Ok(()) => line("done", STEP, format!("linked {} → {}{on_path}", link.display(), binary.display())),
            Err(error) => line("failed", STEP, format!("linking {}: {error}", link.display())),
        },
    }
}

fn claude_plugin(machine: &Machine) -> String {
    use crate::update::{self, Marketplace};
    const STEP: &str = "Claude Code plugin";
    let current = machine.home.join(".local/share/kebbie/current");
    let read = |file: &str| std::fs::read_to_string(machine.claude_dir.join(file)).ok().and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok()).unwrap_or_default();
    let installed = read("plugins/installed_plugins.json");
    if let Some(id) = installed["plugins"].as_object().and_then(|plugins| plugins.keys().find(|id| id.starts_with("kebbie@")).cloned()) {
        if read("settings.json")["enabledPlugins"][&id] == false {
            return match crate::sh::run(&machine.claude, &["plugin", "enable", &id], None) {
                Ok(_) => line("done", STEP, format!("enabled {id}")),
                Err(error) => line("failed", STEP, error),
            };
        }
        let said = update::claude_plugin(&machine.claude_dir, &machine.claude, Some(&current), machine.follows_github);
        return line("already", STEP, format!("{id} is installed; {}", said.trim_start_matches("Claude Code plugin: ")));
    }
    if marketplace_known(&machine.claude_dir, "kebbie") {
        if !matches!(update::marketplace(&machine.claude_dir, "kebbie@kebbie"), Marketplace::InPlace(path) if update::same_folder(&path, &current)) {
            return line("skipped", STEP, format!("Claude Code has a kebbie marketplace that is not {}: {}", current.display(), update::IN_PLACE_FIX));
        }
    } else if let Err(error) = crate::sh::run(&machine.claude, &["plugin", "marketplace", "add", &current.to_string_lossy()], None) {
        return line("failed", STEP, error);
    }
    match crate::sh::run(&machine.claude, &["plugin", "install", "kebbie@kebbie"], None) {
        Ok(_) => line("done", STEP, format!("installed kebbie@kebbie from {}", current.display())),
        Err(error) => line("failed", STEP, error),
    }
}

pub fn kebbie_keys() -> Vec<toml::Table> {
    let example: toml::Table = EXAMPLE.parse().expect("examples/herdr-config.toml is TOML");
    bindings(&example).into_iter().filter(|binding| text(binding, "command").starts_with("kebbie.")).cloned().collect()
}

fn bindings(config: &toml::Table) -> Vec<&toml::Table> {
    config.get("keys").and_then(|keys| keys.get("command")).and_then(toml::Value::as_array).into_iter().flatten().filter_map(toml::Value::as_table).collect()
}

fn text<'a>(table: &'a toml::Table, field: &str) -> &'a str {
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

// The file is appended to, never rewritten, so every byte the user wrote stays as it was.
pub fn herdr_keys(config: &Path, asking: Asking, input: &mut impl BufRead, output: &mut impl Write) -> Result<(String, bool)> {
    const STEP: &str = "herdr keys";
    let skipped = |detail: String| Ok((line("skipped", STEP, detail), false));
    let existing = match std::fs::read_to_string(config) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return skipped(format!("could not read {}: {error}", config.display())),
    };
    let table: toml::Table = match existing.parse() {
        Ok(table) => table,
        Err(error) => return skipped(format!("{} is not valid TOML, so kebbie leaves it alone: {}", config.display(), error.message())),
    };
    let missing: Vec<toml::Table> = kebbie_keys().into_iter().filter(|wanted| !bindings(&table).iter().any(|binding| text(binding, "command") == text(wanted, "command"))).collect();
    if missing.is_empty() {
        return Ok((line("already", STEP, format!("{} binds kebbie's keys", config.display())), false));
    }
    let (free, taken): (Vec<_>, Vec<_>) = missing.iter().partition(|wanted| taken_by(&table, text(wanted, "key")).is_none());
    let taken = taken.iter().map(|wanted| format!("{} is already {}, so {} gets no key", text(wanted, "key"), taken_by(&table, text(wanted, "key")).unwrap_or_default(), text(wanted, "command"))).collect::<Vec<_>>().join("; ");
    let taken = if taken.is_empty() { taken } else { format!("; {taken}: bind it by hand in [[keys.command]]") };
    if free.is_empty() {
        return skipped(taken.trim_start_matches("; ").to_string());
    }
    let keys = free.iter().map(|wanted| text(wanted, "key")).collect::<Vec<_>>().join(", ");
    let separator = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
    let addition = format!("{separator}{}", free.iter().map(|wanted| block(wanted)).collect::<String>());
    if let Err(error) = format!("{existing}{addition}").parse::<toml::Table>() {
        return skipped(format!("{} cannot take [[keys.command]] at its end ({}); add {keys} by hand", config.display(), error.message()));
    }
    let resolved = std::fs::canonicalize(config).ok().or_else(|| Some(std::fs::canonicalize(config.parent()?).ok()?.join(config.file_name()?))).unwrap_or_else(|| config.to_path_buf());
    let via = if resolved == config { String::new() } else { format!(" (a link to {})", resolved.display()) };
    match asking {
        Asking::Closed => return skipped(format!("no terminal to ask in before writing {}{via}: run kebbie setup --yes to add {keys}", config.display())),
        Asking::Yes if !via.is_empty() => return skipped(format!("{}{via} is a link, which --yes never writes through: run kebbie setup in a terminal to be asked, or add {keys} by hand", config.display())),
        Asking::Terminal if !yes(&format!("kebbie setup adds kebbie's keys to the end of {}{via}, leaving the rest as it is:\n{addition}", config.display()), input, output)? => {
            return skipped(format!("you said no; {} is unchanged", config.display()));
        }
        _ => {}
    }
    if let Some(parent) = config.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| format!("creating {}: {error}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(config).map_err(|error| format!("opening {}: {error}", config.display()))?;
    file.write_all(addition.as_bytes()).map_err(|error| format!("writing {}: {error}", config.display()))?;
    Ok((line("done", STEP, format!("added {keys} to {}{via}{taken}", config.display())), true))
}

fn herdr_restart(machine: &Machine, wrote: bool) -> String {
    const STEP: &str = "herdr";
    let current = crate::doctor::current_link(&machine.home, machine.data_home.as_deref(), None);
    if current.level != crate::doctor::Level::Ok {
        return line("to do", STEP, format!("restart herdr: {}, and kebbie's startup in herdr links it", current.fact));
    }
    if !wrote {
        return line("already", STEP, "nothing to restart");
    }
    match crate::sh::run(&machine.herdr, &["server", "reload-config"], None) {
        Ok(_) => line("done", STEP, "reloaded its config, so the keys work now"),
        Err(_) => line("to do", STEP, "restart herdr so it reads the new keys"),
    }
}

pub fn codex(uninstall: bool) -> Result<String> {
    crate::permit::user_only("kebbie setup codex")?;
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
mod tests {
    use super::*;

    #[test]
    fn setup_offers_a_hub_last_and_connects_only_on_a_yes_in_a_terminal() {
        let offer = |asking: Asking, connected: bool, answers: &str| {
            let mut output = Vec::new();
            let offered = hub_offer(asking, connected, &mut answers.as_bytes(), &mut output).unwrap();
            (offered, String::from_utf8(output).unwrap())
        };
        let (offered, asked) = offer(Asking::Terminal, false, "y\n");
        assert_eq!(offered, None, "a yes runs kebbie hub connect");
        assert!(asked.contains("Connect a hub now") && asked.contains("(kebbie hub connect)"), "{asked}");
        for (asking, answers) in [(Asking::Terminal, "n\n"), (Asking::Terminal, ""), (Asking::Closed, "y\n"), (Asking::Yes, "y\n")] {
            let (offered, asked) = offer(asking, false, answers);
            assert!(offered.as_deref().is_some_and(|line| line.starts_with("skipped") && line.contains("kebbie hub connect")), "{asking:?}: {offered:?}");
            assert!(asking == Asking::Terminal || asked.is_empty(), "only a terminal is asked");
        }
        let (offered, asked) = offer(Asking::Terminal, true, "y\n");
        assert!(offered.unwrap().starts_with("already") && asked.is_empty(), "a connected machine is not asked");
        assert!(hub_connected(Ok("Connected to x as mac\nset: y".to_string())).starts_with("done") && hub_connected(Err("Cancelled".to_string())).starts_with("failed"));
    }

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

    fn keys(config: &Path, asking: Asking, answers: &str) -> (String, bool, String) {
        let mut output = Vec::new();
        let (said, wrote) = herdr_keys(config, asking, &mut answers.as_bytes(), &mut output).unwrap();
        (said, wrote, String::from_utf8(output).unwrap())
    }

    fn commands(config: &Path) -> Vec<String> {
        let table: toml::Table = std::fs::read_to_string(config).unwrap().parse().unwrap();
        bindings(&table).iter().map(|binding| format!("{} {}", text(binding, "key"), text(binding, "command"))).collect()
    }

    #[test]
    fn the_keys_kebbie_adds_are_the_ones_the_example_config_binds() {
        let keys: Vec<String> = kebbie_keys().iter().map(|binding| format!("{} {}", text(binding, "key"), text(binding, "command"))).collect();
        assert_eq!(keys, ["prefix+space kebbie.lead", "prefix+tab kebbie.focus", "prefix+t kebbie.board", "prefix+u kebbie.update"]);
    }

    #[test]
    fn herdr_keys_are_appended_only_where_missing_and_the_rest_stays_byte_identical() {
        let dir = crate::sh::test_dir("setup-keys");
        let config = dir.join("herdr/config.toml");
        let (said, wrote, _) = keys(&config, Asking::Yes, "");
        assert!(wrote && said.starts_with("done") && said.contains("prefix+space, prefix+tab, prefix+t, prefix+u"), "{said}");
        assert_eq!(commands(&config).len(), 4, "a missing config is created with the four keys");
        let written = std::fs::read(&config).unwrap();
        let (said, wrote, _) = keys(&config, Asking::Yes, "");
        assert!(!wrote && said.starts_with("already"), "{said}");
        assert_eq!(std::fs::read(&config).unwrap(), written, "a second run writes nothing");

        let own = "onboarding = false # mine\n\n[[keys.command]]\nkey = \"prefix+t\"\ntype = \"plugin_action\"\ncommand = \"kebbie.board\"\n\n[ui]\npane_borders  =  \"auto\"";
        std::fs::write(&config, own).unwrap();
        let (said, wrote, _) = keys(&config, Asking::Yes, "");
        assert!(wrote && said.contains("added prefix+space, prefix+tab, prefix+u"), "{said}");
        let after = std::fs::read_to_string(&config).unwrap();
        assert!(after.starts_with(own) && after[own.len()..].starts_with("\n\n[[keys.command]]"), "the user's bytes stay a prefix, and a missing last newline is added before kebbie's: {after}");
        assert_eq!(commands(&config), ["prefix+t kebbie.board", "prefix+space kebbie.lead", "prefix+tab kebbie.focus", "prefix+u kebbie.update"]);
        assert!(keys(&config, Asking::Yes, "").0.starts_with("already"));
        assert_eq!(std::fs::read_to_string(&config).unwrap(), after);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_key_the_user_bound_to_something_else_is_left_to_them() {
        let dir = crate::sh::test_dir("setup-taken");
        let config = dir.join("config.toml");
        let own = "[keys]\ncycle_pane_next = \"prefix+tab\"\n\n[[keys.command]]\nkey = \"prefix+t\"\ntype = \"popup\"\ncommand = \"lazygit\"\n";
        std::fs::write(&config, own).unwrap();
        let (said, wrote, _) = keys(&config, Asking::Yes, "");
        assert!(wrote && said.contains("added prefix+space, prefix+u") && said.contains("prefix+tab is already cycle_pane_next, so kebbie.focus gets no key") && said.contains("prefix+t is already lazygit"), "{said}");
        assert_eq!(commands(&config), ["prefix+t lazygit", "prefix+space kebbie.lead", "prefix+u kebbie.update"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn herdr_keys_are_written_only_after_a_yes_and_never_without_a_terminal() {
        let dir = crate::sh::test_dir("setup-ask");
        let config = dir.join("config.toml");
        std::fs::write(&config, "onboarding = false\n").unwrap();
        let (said, wrote, asked) = keys(&config, Asking::Closed, "y\n");
        assert!(!wrote && said.contains("no terminal to ask in") && said.contains("kebbie setup --yes") && asked.is_empty(), "{said}");
        let (said, wrote, asked) = keys(&config, Asking::Terminal, "n\n");
        assert!(!wrote && said.contains("you said no") && asked.contains("[[keys.command]]\nkey = \"prefix+space\""), "{asked}");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "onboarding = false\n");
        assert!(keys(&config, Asking::Terminal, "y\n").1);
        assert_eq!(commands(&config).len(), 4);
        let broken = "onboarding = \n";
        std::fs::write(&config, broken).unwrap();
        let (said, wrote, _) = keys(&config, Asking::Yes, "");
        assert!(!wrote && said.contains("is not valid TOML"), "{said}");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), broken);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_linked_herdr_config_is_written_through_only_when_asked_and_named() {
        let dir = crate::sh::test_dir("setup-link");
        let target = dir.join("dotfiles/herdr.toml");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "onboarding = false\n").unwrap();
        let config = dir.join("config.toml");
        std::os::unix::fs::symlink(&target, &config).unwrap();
        let (said, wrote, _) = keys(&config, Asking::Yes, "");
        assert!(!wrote && said.contains(&format!("(a link to {})", target.display())) && said.contains("--yes never writes through"), "{said}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "onboarding = false\n");
        let (said, wrote, asked) = keys(&config, Asking::Terminal, "y\n");
        assert!(wrote && asked.contains(&format!("(a link to {})", target.display())) && said.contains(&target.display().to_string()), "{asked}\n{said}");
        assert!(std::fs::symlink_metadata(&config).unwrap().file_type().is_symlink(), "the link stays a link");
        assert_eq!(commands(&target).len(), 4);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn machine(dir: &Path) -> (Machine, PathBuf) {
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
            "#!/bin/sh\necho \"$*\" >> '{log}'\ncase \"$*\" in\n  'plugin marketplace add '*) printf '{{\"kebbie\":{{\"source\":{{\"source\":\"directory\",\"path\":\"%s\"}}}}}}' \"$4\" > '{plugins}/known_marketplaces.json' ;;\n  'plugin install kebbie@kebbie') printf '{{\"plugins\":{{\"kebbie@kebbie\":[{{\"version\":\"0.26.6\"}}]}}}}' > '{plugins}/installed_plugins.json' ;;\nesac\n",
            log = log.display(), plugins = plugins.display(),
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

    fn setup(machine: &Machine) -> Vec<String> {
        let mut output = Vec::new();
        install_on(machine, Asking::Yes, &mut "".as_bytes(), &mut output).unwrap();
        String::from_utf8(output).unwrap().lines().map(|line| line.split_whitespace().take(1).collect::<String>()).collect()
    }

    #[test]
    fn setup_does_each_step_once_and_a_second_run_finds_everything_in_place() {
        let dir = crate::sh::test_dir("setup-install");
        let (machine, log) = machine(&dir);
        assert_eq!(setup(&machine), ["done", "done", "done", "to"], "herdr is restarted by hand when it cannot reload");
        let binary = machine.home.join(".local/share/kebbie/current/target/release/kebbie");
        assert_eq!(std::fs::read_link(machine.home.join(".local/bin/kebbie")).unwrap(), binary);
        let current = machine.home.join(".local/share/kebbie/current");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), format!("plugin marketplace add {}\nplugin install kebbie@kebbie\n", current.display()));
        let config = std::fs::read(&machine.herdr_config).unwrap();
        assert_eq!(setup(&machine), ["already", "already", "already", "already"]);
        assert_eq!(std::fs::read(&machine.herdr_config).unwrap(), config);
        assert!(std::fs::read_to_string(&log).unwrap().ends_with("plugin marketplace update kebbie\nplugin update kebbie@kebbie\n"), "a second run updates the plugin as kebbie update does");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn setup_leaves_a_kebbie_on_path_it_did_not_link_and_says_when_bin_is_not_on_path() {
        let dir = crate::sh::test_dir("setup-path");
        let (mut machine, _) = machine(&dir);
        machine.path = Vec::new();
        let said = link_binary(&machine.home, &machine.path);
        assert!(said.starts_with("done") && said.contains("is not on PATH: add export PATH=\"$HOME/.local/bin:$PATH\""), "{said}");
        std::fs::remove_file(machine.home.join(".local/bin/kebbie")).unwrap();
        std::os::unix::fs::symlink("/opt/old/kebbie", machine.home.join(".local/bin/kebbie")).unwrap();
        assert!(link_binary(&machine.home, &machine.path).contains("links to /opt/old/kebbie: remove it"));
        assert_eq!(std::fs::read_link(machine.home.join(".local/bin/kebbie")).unwrap(), PathBuf::from("/opt/old/kebbie"));
        std::fs::remove_dir_all(dir).unwrap();
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
