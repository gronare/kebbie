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

#[cfg(test)]
mod tests {
    use super::*;

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
