mod actions;
mod area_page;
mod areas;
mod backlog;
mod backlog_file;
mod board;
mod config;
mod doctor;
mod fetch;
mod herdr;
mod lead;
mod messages;
mod notify;
mod peers;
mod permit;
mod push_block;
mod questions;
mod repo;
mod report;
mod review;
mod sh;
mod setup;
mod status;
mod task_note;
mod tasks;
mod update;
mod usage;
mod vault;
mod vault_cli;

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
                                  (--first only moves the lead Space to the top of herdr's Spaces; --restart
                                  restarts an idle lead's Claude on its current role and settings, resuming
                                  its conversation)
  kelpie focus                    in the lead Space, move focus between the board and the lead's chat; from
                                  anywhere else, go to the lead's chat
  kelpie peers                    who you are and the other task sessions, with what each is doing
  kelpie tell <to> <text>         message another session: a task (every half of it), task@repo, or lead; held
                                  while it waits for an answer, and logged
  kelpie report --state <state> --next <step> <summary>
                                  run by a task's Claude at the end of a turn: where it stands (needs-you, ready,
                                  working, blocked) and what you should do next, for the board and the lead;
                                  with ready, --card <file> is the sign-off card the push dialog shows above the commits
  kelpie ask --option \"<label>: <meaning>\" … --recommend <n> <question>
                                  run by a task's Claude to put a question to the user: two to four options, one
                                  recommended (--plan vault:<note> or --plan <file> asks to approve a plan instead); it
                                  waits on the board, where you answer it, the lead gets a copy, and the answer reaches
                                  the task as a message from you
  kelpie answered <task>          a task's question is settled elsewhere (in its Space, or dropped): it leaves the board
  kelpie notify <text>            run notify.command with this text (KELPIE_EVENT=summary): the lead's word to the user
                                  when they are away; tasks that need you or are ready, and questions, notify by themselves
  kelpie decide <task> <answer>   the lead's own answer to a task's question, only while the user is away and only on
                                  how, not what; it closes the question on the board and is recorded for the user's return
  kelpie defer <task>             the lead keeps a task's question for the user while they are away
  kelpie away [on|off]            away mode, the user's switch (also A on the board): the lead settles how-questions,
                                  keeps what-questions, and writes a summary when you are back
  kelpie hook                     the hooks kelpie gives a task's Claude (reads the hook's JSON on stdin)
  kelpie messages [--print]       the sessions' messages, live (the board shows them too); --print the last 30,
                                  --unread those the lead has not seen (its prompt hook)
  kelpie statusline               Claude Code's statusLine command: reads its JSON on stdin and keeps the plan's session
                                  and week usage for the board; prints nothing, so Claude Code shows no extra line
  kelpie backlog [save]           show the backlog the board shows; save reads the lead's JSON array on stdin
  kelpie backlog add [--priority <p>] \"<Title> — <summary> (<area>)\"
                                  add an item to the local backlog (backlog.md next to the machine config)
  kelpie link <name> <item>       tie a running task to a backlog item or project note, as start --backlog does:
                                  done tells the lead to close it, reset puts it back in the backlog
  kelpie land [name]              rebase a task onto the default branch and check it is clean; on a conflict, its
                                  Claude resolves the rebase
  kelpie tidy <name> [note]       ask the task's Claude to reshape its commits by the rules skill (task.rules_skill)
  kelpie review [name]            send the task to review: a separate, read-only Claude reviews its commits in the
                                  background, by the skill kelpie:review and the rules skill; the task waits under To
                                  review, and push with it, until a verdict ok on its current code (--withdraw
                                  takes it out of review, yours only: the reviewer stops, its verdict goes, push is
                                  offered again)
  kelpie approve <name>           the lead's word that a task's commits hold to the rules skill; until a commit
                                  changes, the board shows it ready to push
  kelpie push [name]              after land: push the task to the default branch, then close it as done
                                  (--through <sha> pushes its commits up to that one and keeps the task open, the rest
                                  of its branch on top of what was pushed); the user's step, or the lead's for a task the
                                  user gave push permission on the board (a), or in a repo they did on the Areas page (a)
                                  once its reviewer passed its current code; the lead is told what landed
  kelpie done [name]              after pushing: close the task and free its worktree
  kelpie drop [name] [--force]    throw a task away; --force discards uncommitted work and unpushed commits
  kelpie reset [name]             stop a task for good: discard its work, free its worktree, close its Space, and put
                                  the backlog item it was started from back in the Backlog column
  kelpie config [--repo <path>]   the config layers found (.kelpie.toml, machine config, defaults) and every
                                  setting in effect with the layer it comes from (--get <key> prints one machine-wide
                                  value, for scripts)
  kelpie doctor                   check that this machine has what kelpie needs, with the fix for each gap
  kelpie setup vault              install the optional vault plugin in Claude Code (marketplace, plugin, its options),
                                  showing each claude command and running it only on your y
  kelpie update                   update the herdr and Claude Code plugins, rebuild, reload the plugins in
                                  every waiting Claude session, and restart an idle lead on its new role

With the vault plugin installed, enabled and configured, the vault holds each task: start opens the task's note
(or claims the vault backlog note it starts from), edits wait until the note's Problem is written, push writes
the repo's log line from the commit subjects, done waits for the note's solution and decisions and then closes
it, and reset puts it back in the backlog. Without the plugin none of this runs.

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
            if ["--pane", "-C", "--repo", "--from", "--mode", "--state", "--next", "--backlog", "--option", "--recommend", "--plan", "--card", "--through", "--priority", "--get"].contains(&argument.as_str()) {
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
            lines.push(format!("    {}", questions::text(question).replace('\n', "\n    ")));
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
            serde_json::to_string_pretty(&serde_json::json!({ "away_since": permit::away_since(), "items": items, "checkouts": checkouts })).map_err(|error| error.to_string())
        }
        Some("status") => status_text(),
        Some("board") => board::run(arguments.flag("--overlay")),
        Some("areas") => areas::print(),
        Some("lead") if arguments.flag("--first") => lead::keep_first(),
        Some("lead") if arguments.flag("--restart") => lead::restart(),
        Some("lead") => lead::toggle(),
        Some("focus") => lead::focus_toggle().map(|_| String::new()),
        Some("peers") => peers::peers(&cwd),
        Some("tell") => {
            let to = name.ok_or("kelpie tell needs a recipient: a task, task@repo or lead")?;
            // Taken as given, since a message can start with "-".
            let text: Vec<String> = std::env::args().skip_while(|argument| argument != "tell").skip(2).collect();
            messages::tell(&cwd, to, &text.join(" "))
        }
        Some("report") => report::report(&cwd, arguments.value("--state"), arguments.value("--next"), arguments.value("--card"), &arguments.positionals[1..].join(" ")),
        Some("ask") => questions::ask(&cwd, &arguments.values_of("--option"), arguments.value("--recommend"), arguments.value("--plan"), &arguments.positionals[1..].join(" ")),
        Some("notify") => {
            let text: Vec<String> = std::env::args().skip_while(|argument| argument != "notify").skip(1).collect();
            notify::notify(&cwd, &text.join(" "))
        }
        Some("answered") => questions::answered(name.ok_or("kelpie answered needs the task whose question is settled")?),
        Some("decide") => {
            let to = name.ok_or("kelpie decide needs the task whose question you answer")?;
            let answer: Vec<String> = std::env::args().skip_while(|argument| argument != "decide").skip(2).collect();
            questions::decide(to, &answer.join(" "))
        }
        Some("defer") => questions::defer(name.ok_or("kelpie defer needs the task whose question waits for the user")?),
        Some("away") => permit::away(name),
        Some("hook") => {
            let mut input = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
            if arguments.flag("--review") {
                return Ok(review::guard(&input));
            }
            if name == Some("gate") {
                return Ok(arguments.positional(2).map(task_note::hook).unwrap_or_default());
            }
            let _ = messages::flush();
            Ok(report::hook(&input))
        }
        Some("messages") if arguments.flag("--print") => Ok(messages::print(30)),
        Some("messages") if arguments.flag("--unread") => {
            let _ = messages::flush();
            messages::unread_for_lead()
        }
        Some("messages") => messages::run(),
        Some("backlog") if arguments.positional(1) == Some("save") => backlog::save(),
        Some("backlog") if arguments.positional(1) == Some("add") => backlog_file::add(arguments.value("--priority"), &arguments.positionals[2..].join(" ")),
        Some("backlog") => backlog::show(),
        Some("land") => tasks::land(&cwd, name),
        Some("push") => tasks::push(&cwd, name, arguments.value("--through"), arguments.value("--through").is_some()),
        Some("tidy") => tasks::tidy(&cwd, Some(name.ok_or("kelpie tidy needs a task name")?), &arguments.positionals[2..].join(" ")),
        Some("review") if arguments.flag("--run") => review::run(&cwd),
        Some("review") if arguments.flag("--withdraw") => review::withdraw(&cwd, name),
        Some("review") => review::request(&cwd, name),
        Some("approve") => tasks::approve(&cwd, Some(name.ok_or("kelpie approve needs a task name")?)),
        Some("done") => tasks::done(&cwd, name),
        Some("drop") => tasks::drop(&cwd, name, arguments.flag("--force")),
        Some("reset") => tasks::reset(&cwd, name),
        Some("link") => {
            let name = name.ok_or("kelpie link needs a task name and the item")?;
            tasks::link(&cwd, name, &arguments.positionals[2..].join(" "))
        }
        Some("config") => match arguments.value("--get") {
            Some(key) => config::get(key),
            None => config::show(&cwd, arguments.value("--repo")),
        },
        Some("setup") if name == Some("vault") => setup::vault(),
        Some("setup") => Err("kelpie setup knows vault: kelpie setup vault".to_string()),
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
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.len() > 2 && argv[0] == "notify" && argv[1] == notify::DELIVER {
        notify::deliver(&argv[2..]);
        return ExitCode::SUCCESS;
    }
    let arguments = Arguments::parse();
    if arguments.positional(0) == Some("doctor") {
        return doctor();
    }
    if arguments.positional(0) == Some("statusline") {
        let mut input = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
        usage::record(&input);
        return ExitCode::SUCCESS;
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
