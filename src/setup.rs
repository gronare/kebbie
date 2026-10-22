use std::io::{BufRead, Write};
use std::path::Path;

use crate::sh::Result;
use crate::vault_cli::Plugin;

pub const CLAUDE_VAR: &str = "KELPIE_CLAUDE_BIN";
const MARKETPLACE: &str = "https://github.com/gronare/claude-plugins.git";
const PLUGIN: &str = "vault@gronare";

pub fn claude_bin() -> String {
    std::env::var(CLAUDE_VAR).ok().filter(|bin| !bin.is_empty()).unwrap_or_else(|| "claude".to_string())
}

fn marketplace_known(claude_home: &Path) -> bool {
    std::fs::read_to_string(claude_home.join("plugins/known_marketplaces.json")).ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .is_some_and(|known| known.get("gronare").is_some())
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
        "Point the plugin at your vault, one of:\n  a local folder:  echo '{{\"vault_path\": \"/path/to/vault\"}}' | {claude} plugin configure {PLUGIN} --values-stdin\n  an S3 bucket:    set s3_endpoint, s3_bucket, s3_access_key and s3_secret_key the same way, or in Claude Code's /plugin screen, which asks for them.\nThen run kelpie doctor: its vault line says whether kelpie keeps your tasks in the vault."
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
    let steps = steps(&claude, &crate::vault_cli::plugin(&home), marketplace_known(&home));
    println!("kelpie setup vault: the vault plugin keeps each task's note and the repos' logs in your Obsidian vault. Optional: kelpie works the same without it.");
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

pub fn codex(uninstall: bool) -> Result<String> {
    crate::permit::user_only("kelpie setup codex")?;
    let home = crate::agent::codex::home().ok_or("Neither CODEX_HOME nor HOME is set, so kelpie cannot find Codex's home")?;
    let kelpie = crate::peers::kelpie_command()?;
    let stdin = std::io::stdin();
    codex_in(&home, &kelpie, uninstall, &mut stdin.lock(), &mut std::io::stdout())
}

// Codex's plugin cache is the one place kelpie writes in Codex's home, and only on the user's y; config.toml and hooks.json stay theirs.
pub fn codex_in(home: &Path, kelpie: &str, uninstall: bool, input: &mut impl BufRead, output: &mut impl Write) -> Result<String> {
    use crate::agent::codex::{self, Installed};
    let root = codex::plugin_root(home);
    if uninstall {
        if !root.exists() {
            return Ok(format!("kelpie's Codex plugin is not installed; {} is not there.", root.display()));
        }
        if !yes(&format!("kelpie setup codex --uninstall removes kelpie's plugin from Codex: {}", root.display()), input, output)? {
            return Ok("Skipped; nothing was removed.".to_string());
        }
        codex::uninstall(home)?;
        return Ok(format!("Removed {}. Tasks whose profile runs Codex now start without kelpie's hooks, so they neither report nor get their role.", root.display()));
    }
    let state = codex::installed(home, kelpie);
    if state == Installed::Current {
        return Ok(format!("kelpie's Codex plugin is installed and current at {}; nothing to do.", root.display()));
    }
    let verb = if state == Installed::Missing { "install" } else { "update" };
    let question = format!(
        "kelpie setup codex: {verb} kelpie's plugin in Codex's plugin cache, {}.\nIts three hooks (SessionStart, UserPromptSubmit, Stop) record the session, give it its role and ask for kelpie report before a turn ends; outside a kelpie task they do nothing.\nkelpie enables the plugin, and trusts those three hooks by their hash, only in the Codex sessions it starts (-c on its command line). It never edits {} or {}.",
        root.display(), home.join("config.toml").display(), home.join("hooks.json").display(),
    );
    if !yes(&question, input, output)? {
        return Ok("Skipped; nothing was written.".to_string());
    }
    let written = codex::install(home, kelpie)?;
    let files = written.iter().map(|file| format!("  {}", file.display())).collect::<Vec<_>>().join("\n");
    Ok(format!("Wrote:\n{files}\nkelpie doctor's codex lines say whether Codex and its hooks are ready."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_codex_writes_only_on_a_yes_and_says_what_it_leaves_alone() {
        let home = crate::sh::test_dir("setup-codex");
        std::fs::write(home.join("config.toml"), "model = \"gpt-5.5\"\n").unwrap();
        let run = |answers: &str, uninstall: bool| {
            let mut output = Vec::new();
            let said = codex_in(&home, "/opt/kelpie", uninstall, &mut answers.as_bytes(), &mut output).unwrap();
            (said, String::from_utf8(output).unwrap())
        };
        let (said, asked) = run("", false);
        assert_eq!(said, "Skipped; nothing was written.");
        assert!(asked.contains("install kelpie's plugin in Codex's plugin cache") && asked.contains(&format!("It never edits {}", home.join("config.toml").display())), "{asked}");
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
