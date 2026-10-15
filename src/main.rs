mod backlog;
mod board;
mod herdr;
mod lead;
mod repo;
mod sh;
mod status;
mod tasks;

use std::path::Path;
use std::process::ExitCode;

use sh::Result;

const USAGE: &str = "kelpie: herding coding agents in herdr

  kelpie start <name> [prompt]    start a task in a pooled worktree, in its own Space, with Claude
                                  (--repo <name> finds or clones that repo, --from <ref> branches off it
                                  instead of the default branch, --mode <mode> is Claude's permission mode,
                                  --no-focus keeps you where you are)
  kelpie move <name> [file ...]   move work under way in the main checkout into a task: branch <name> if it
                                  exists, else the unpushed commits, plus the given uncommitted files
                                  (--pane <id> moves that herdr pane along, --no-focus stays put)
  kelpie list [--json]            the pool's worktrees and their tasks
  kelpie status [--json]          every task and Claude session, grouped by what it needs from you
  kelpie board [--overlay]        the board: tasks and sessions, with keys to act on them
  kelpie lead                     go to the lead Space (creating it), or back to where you were
  kelpie backlog [save]           show the backlog the lead keeps; save reads a JSON array on stdin
  kelpie land [name]              rebase a task onto the default branch and check it; prints the push command
  kelpie done [name]              after pushing: close the task and free its worktree
  kelpie drop [name] [--force]    throw a task away; --force discards uncommitted work and unpushed commits

Without a name, land, done and drop act on the task whose worktree you are in.
-C <dir> runs as if started in <dir>.";

struct Arguments {
    positionals: Vec<String>,
    flags: Vec<String>,
    values: Vec<(String, String)>,
}

impl Arguments {
    fn parse() -> Self {
        let mut arguments = Self { positionals: Vec::new(), flags: Vec::new(), values: Vec::new() };
        let mut input = std::env::args().skip(1);
        while let Some(argument) = input.next() {
            if ["--pane", "-C", "--repo", "--from", "--mode"].contains(&argument.as_str()) {
                arguments.values.push((argument, input.next().unwrap_or_default()));
            } else if argument.starts_with('-') {
                arguments.flags.push(argument);
            } else {
                arguments.positionals.push(argument);
            }
        }
        arguments
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values.iter().find(|(flag, _)| flag == name).map(|(_, value)| value.as_str())
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

fn status_text() -> Result<String> {
    let items = status::gather()?;
    if items.is_empty() {
        return Ok("Nothing running.".to_string());
    }
    let mut lines = Vec::new();
    let mut group = None;
    for item in &items {
        if group != Some(item.group()) {
            group = Some(item.group());
            lines.push(format!("\n{}", item.group().title()));
        }
        lines.push(format!("  {}  ({})\n    {}", item.name, item.place, item.summary()));
    }
    Ok(lines.join("\n").trim_start().to_string())
}

fn run(arguments: &Arguments) -> Result<String> {
    let cwd = match arguments.value("-C") {
        Some(dir) => std::path::PathBuf::from(dir),
        None => std::env::current_dir().map_err(|error| error.to_string())?,
    };
    if arguments.flag("-h") || arguments.flag("--help") {
        return Ok(USAGE.to_string());
    }
    let name = arguments.positional(1);
    match arguments.positional(0) {
        Some("start") => {
            let name = name.ok_or("kelpie start needs a task name")?;
            let prompt = arguments.positionals[2..].join(" ");
            let prompt = (!prompt.is_empty()).then_some(prompt.as_str());
            let repo = arguments.value("--repo").map(repo::locate).transpose()?.unwrap_or(cwd);
            let options = tasks::Start { prompt, focus: !arguments.flag("--no-focus"), from: arguments.value("--from"), mode: arguments.value("--mode") };
            tasks::start(&repo, name, &options)
        }
        Some("move") => {
            let name = name.ok_or("kelpie move needs a task name")?;
            tasks::move_work(&cwd, name, &arguments.positionals[2..], arguments.value("--pane"), !arguments.flag("--no-focus"))
        }
        Some("list") if arguments.flag("--json") => {
            serde_json::to_string_pretty(&tasks::list(&cwd)?).map_err(|error| error.to_string())
        }
        Some("list") => table(&cwd),
        Some("status") if arguments.flag("--json") => {
            let items: Vec<serde_json::Value> = status::gather()?.iter().map(status::Item::to_json).collect();
            let checkouts: Vec<serde_json::Value> = status::checkouts()?.iter().map(status::Checkout::to_json).collect();
            serde_json::to_string_pretty(&serde_json::json!({ "items": items, "checkouts": checkouts })).map_err(|error| error.to_string())
        }
        Some("status") => status_text(),
        Some("board") => board::run(arguments.flag("--overlay")),
        Some("lead") => lead::toggle(),
        Some("backlog") if arguments.positional(1) == Some("save") => backlog::save(),
        Some("backlog") => Ok(backlog::show()),
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
