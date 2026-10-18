mod actions;
mod area_page;
mod areas;
mod backlog;
mod board;
mod config;
mod doctor;
mod fetch;
mod herdr;
mod lead;
mod messages;
mod peers;
mod questions;
mod repo;
mod report;
mod review;
mod sh;
mod status;
mod tasks;
mod update;
mod vault;

use std::path::Path;
use std::process::ExitCode;

use sh::Result;

const USAGE: &str = "kelpie: herding coding agents in herdr

  kelpie start <name> [prompt]    start a task in a pooled worktree, in its own Space, with Claude
                                  (--repo <name> finds or clones that repo, repeated for a task across repos,
                                  --from <ref> branches off it
                                  instead of the default branch, --mode <mode> is Claude's permission mode,
                                  --no-focus keeps you where you are)
  kelpie move <name> [file ...]   move work under way in the main checkout into a task: branch <name> if it
                                  exists, else the unpushed commits, plus the given uncommitted files
                                  (--pane <id> moves that herdr pane along, --no-focus stays put)
  kelpie list [--json]            the pool's worktrees and their tasks
  kelpie status [--json]          every task and Claude session, grouped by what it needs from you
  kelpie board [--overlay]        the board: tasks and sessions, with keys to act on them
  kelpie areas                    the vault's areas with their repo: main against origin, last release, notes, actions
  kelpie lead                     go to the lead Space (creating it), or back to where you were
                                  (--first only moves the lead Space to the top of herdr's Spaces)
  kelpie peers                    who you are and the other task sessions, with what each is doing
  kelpie tell <to> <text>         message another session: a task (every half of it), task@repo, or lead; held
                                  while it waits for an answer, and logged
  kelpie report --state <state> --next <step> <summary>
                                  run by a task's Claude at the end of a turn: where it stands (needs-you, ready,
                                  working, blocked) and what you should do next, for the board and the lead;
                                  with ready, --card <file> is the sign-off card the push dialog shows above the commits
  kelpie ask --option \"<label>: <meaning>\" … --recommend <n> <question>
                                  run by a task's Claude to put a question to the user: two to four options, one
                                  recommended (--plan <file> asks to approve a plan instead); it waits in kelpie's
                                  queue and reaches the lead one question at a time, and the answer comes back as a message
  kelpie answered <task>          the lead's word that a task's question is settled without a kelpie tell, so the
                                  next question goes to the lead
  kelpie hook                     the hooks kelpie gives a task's Claude (reads the hook's JSON on stdin)
  kelpie messages [--print]       the sessions' messages, live (the lead Space shows them); --print the last 30,
                                  --unread those the lead has not seen (its prompt hook)
  kelpie backlog [save]           show the backlog the lead keeps; save reads a JSON array on stdin
  kelpie land [name]              rebase a task onto the default branch and check it is clean; on a conflict, its
                                  Claude resolves the rebase
  kelpie tidy <name> [note]       ask the task's Claude to reshape its commits by the rules skill (task.rules_skill)
  kelpie review [name]            send the task to review: a separate, read-only Claude reviews its commits in the
                                  background, by the skill kelpie:review and the rules skill; the task waits under To
                                  review, and push with it, until a verdict ok on its current code
  kelpie approve <name>           the lead's word that a task's commits hold to the rules skill; until a commit
                                  changes, the board shows it ready to push
  kelpie push [name]              after land: push the task to the default branch, then close it as done
                                  (--through <sha> pushes its commits up to that one and keeps the task open, the rest
                                  of its branch on top of what was pushed)
  kelpie done [name]              after pushing: close the task and free its worktree
  kelpie drop [name] [--force]    throw a task away; --force discards uncommitted work and unpushed commits
  kelpie reset [name]             stop a task for good: discard its work, free its worktree, close its Space, and put
                                  the backlog item it was started from back in the Backlog column
  kelpie config [--repo <path>]   the config layers found (.kelpie.toml, machine config, defaults) and every
                                  setting in effect with the layer it comes from
  kelpie doctor                   check that this machine has what kelpie needs, with the fix for each gap
  kelpie update                   update the herdr and Claude Code plugins, rebuild, and reload the plugins in
                                  every waiting Claude session

Without a name, land, review, push, done, drop and reset act on the task whose worktree you are in.
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
            if ["--pane", "-C", "--repo", "--from", "--mode", "--state", "--next", "--backlog", "--option", "--recommend", "--plan", "--card", "--through"].contains(&argument.as_str()) {
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

    fn values_of(&self, name: &str) -> Vec<&str> {
        self.values.iter().filter(|(flag, _)| flag == name).map(|(_, value)| value.as_str()).collect()
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
        let setup = row["setup"].as_str().unwrap_or("not set up");
        if row["free"].as_bool() == Some(true) {
            return format!("{slot:<22} free  · {setup}");
        }
        let task = row["task"].as_str().unwrap_or("(not free: uncommitted changes)");
        format!("{slot:<22} {task}  {} changed  ↑{} ↓{}  · {setup}", row["changed"], row["ahead"], row["behind"])
    }).collect::<Vec<_>>().join("\n"))
}

fn status_text() -> Result<String> {
    let items = status::gather()?;
    if items.is_empty() {
        return Ok("Nothing running.".to_string());
    }
    let mut lines = Vec::new();
    let mut group = None;
    let mut tasks = true;
    for item in &items {
        if tasks && !item.is_task() {
            tasks = false;
            group = None;
            lines.push("\n— other Claude sessions —".to_string());
        }
        if group != Some(item.group()) {
            group = Some(item.group());
            lines.push(format!("\n{}", item.group().title()));
        }
        let halves: Vec<&str> = items.iter().filter(|other| other.is_task() && other.name == item.name && other.place != item.place).map(|other| other.place.as_str()).collect();
        let place = if halves.is_empty() || !item.is_task() { item.place.clone() } else { format!("{}, same task in {}", item.place, halves.join(", ")) };
        lines.push(format!("  {}  ({place})\n    {}", item.name, item.summary()));
        if let Some(report) = &item.report {
            lines.push(format!("    {} ({})", report.text, messages::age(report.at)));
        }
        if let Some(next) = item.next_for_you() {
            lines.push(format!("    you: {next}"));
        }
        if let Some(question) = &item.question {
            lines.push(format!("    {}", board::question_text(question).replace('\n', "\n    ")));
        }
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
            let mut repos = arguments.values_of("--repo").into_iter().map(repo::locate).collect::<Result<Vec<_>>>()?;
            if repos.is_empty() {
                repos.push(cwd);
            }
            let options = tasks::Start { prompt, focus: !arguments.flag("--no-focus"), from: arguments.value("--from"), mode: arguments.value("--mode"), backlog: arguments.value("--backlog") };
            // One task across repos: a half per repo, the first one focused.
            let started: Vec<Result<String>> = repos.iter().enumerate()
                .map(|(index, repo)| tasks::start(repo, name, &tasks::Start { focus: options.focus && index == 0, ..options }))
                .collect();
            let report = started.iter().map(|result| match result { Ok(text) | Err(text) => text.as_str() }).collect::<Vec<_>>().join("\n");
            if started.iter().any(Result::is_err) { Err(report) } else { Ok(report) }
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
        Some("areas") => areas::print(),
        Some("lead") if arguments.flag("--first") => lead::keep_first(),
        Some("lead") => lead::toggle(),
        Some("peers") => peers::peers(&cwd),
        Some("tell") => {
            let to = name.ok_or("kelpie tell needs a recipient: a task, task@repo or lead")?;
            // Taken as given, since a message can start with "-".
            let text: Vec<String> = std::env::args().skip_while(|argument| argument != "tell").skip(2).collect();
            messages::tell(&cwd, to, &text.join(" "))
        }
        Some("report") => report::report(&cwd, arguments.value("--state"), arguments.value("--next"), arguments.value("--card"), &arguments.positionals[1..].join(" ")),
        Some("ask") => questions::ask(&cwd, &arguments.values_of("--option"), arguments.value("--recommend"), arguments.value("--plan"), &arguments.positionals[1..].join(" ")),
        Some("answered") => questions::answered(name.ok_or("kelpie answered needs the task whose question is settled")?),
        Some("hook") => {
            let mut input = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
            Ok(if arguments.flag("--review") { review::guard(&input) } else { report::hook(&input) })
        }
        Some("messages") if arguments.flag("--print") => Ok(messages::print(30)),
        Some("messages") if arguments.flag("--unread") => messages::unread_for_lead(),
        Some("messages") => messages::run(),
        Some("backlog") if arguments.positional(1) == Some("save") => backlog::save(),
        Some("backlog") => Ok(backlog::show()),
        Some("land") => tasks::land(&cwd, name),
        Some("push") => tasks::push(&cwd, name, arguments.value("--through"), arguments.value("--through").is_some()),
        Some("tidy") => tasks::tidy(&cwd, Some(name.ok_or("kelpie tidy needs a task name")?), &arguments.positionals[2..].join(" ")),
        Some("review") if arguments.flag("--run") => review::run(&cwd),
        Some("review") => review::request(&cwd, name),
        Some("approve") => tasks::approve(&cwd, Some(name.ok_or("kelpie approve needs a task name")?)),
        Some("done") => tasks::done(&cwd, name),
        Some("drop") => tasks::drop(&cwd, name, arguments.flag("--force")),
        Some("reset") => tasks::reset(&cwd, name),
        Some("config") => config::show(&cwd, arguments.value("--repo")),
        Some("update") => update::run(&doctor::Env::current().claude_dir),
        None | Some("help") => Ok(USAGE.to_string()),
        Some(other) => Err(format!("Unknown command \"{other}\"\n\n{USAGE}")),
    }
}

fn doctor() -> ExitCode {
    let checks = doctor::checks(&doctor::Env::current());
    println!("{}", doctor::render(&checks));
    if doctor::failed(&checks) { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    if arguments.positional(0) == Some("doctor") {
        return doctor();
    }
    match run(&arguments) {
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
