mod herdr;
mod repo;
mod sh;
mod tasks;

use std::path::Path;
use std::process::ExitCode;

use sh::Result;

const USAGE: &str = "kelpie: herding coding agents in herdr

  kelpie start <name> [prompt]    start a task in a pooled worktree, in its own Space, with Claude
                                  (--no-focus keeps you where you are)
  kelpie list [--json]            the pool's worktrees and their tasks
  kelpie land [name]              rebase a task onto the default branch and check it; prints the push command
  kelpie done [name]              after pushing: close the task and free its worktree
  kelpie drop [name] [--force]    throw a task away; --force discards uncommitted work and unpushed commits

Without a name, land, done and drop act on the task whose worktree you are in.";

struct Arguments {
    positionals: Vec<String>,
    flags: Vec<String>,
}

impl Arguments {
    fn parse() -> Self {
        let (flags, positionals) = std::env::args().skip(1).partition(|argument| argument.starts_with('-'));
        Self { positionals, flags }
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }

    fn positional(&self, index: usize) -> Option<&str> {
        self.positionals.get(index).map(String::as_str)
    }
}

fn table(cwd: &Path) -> Result<String> {
    let rows = tasks::list(cwd)?;
    if rows.is_empty() {
        return Ok("No pooled worktrees yet; kelpie start <name> creates the first.".to_string());
    }
    Ok(rows.iter().map(|row| {
        let slot = row["slot"].as_str().unwrap_or_default();
        if row["free"].as_bool() == Some(true) {
            return format!("{slot:<22} free");
        }
        let task = row["task"].as_str().unwrap_or("(not free: uncommitted changes)");
        format!("{slot:<22} {task}  {} changed  ↑{} ↓{}", row["changed"], row["ahead"], row["behind"])
    }).collect::<Vec<_>>().join("\n"))
}

fn run(arguments: &Arguments) -> Result<String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    if arguments.flag("-h") || arguments.flag("--help") {
        return Ok(USAGE.to_string());
    }
    let name = arguments.positional(1);
    match arguments.positional(0) {
        Some("start") => {
            let name = name.ok_or("kelpie start needs a task name")?;
            let prompt = arguments.positionals[2..].join(" ");
            let prompt = (!prompt.is_empty()).then_some(prompt.as_str());
            tasks::start(&cwd, name, prompt, !arguments.flag("--no-focus"))
        }
        Some("list") if arguments.flag("--json") => {
            serde_json::to_string_pretty(&tasks::list(&cwd)?).map_err(|error| error.to_string())
        }
        Some("list") => table(&cwd),
        Some("land") => tasks::land(&cwd, name),
        Some("done") => tasks::done(&cwd, name),
        Some("drop") => tasks::drop(&cwd, name, arguments.flag("--force")),
        None | Some("help") => Ok(USAGE.to_string()),
        Some(other) => Err(format!("Unknown command \"{other}\"\n\n{USAGE}")),
    }
}

fn main() -> ExitCode {
    match run(&Arguments::parse()) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
