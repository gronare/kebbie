mod actions;
mod agent;
mod area_page;
mod areas;
mod backlog;
mod backlog_file;
mod board;
mod ci;
mod config;
mod doctor;
mod evidence;
mod fetch;
mod herdr;
mod issues;
mod keeper;
mod keys;
mod lead;
mod login_env;
mod menu;
mod messages;
mod notify;
mod peers;
mod permit;
mod profiles;
mod push_block;
mod questions;
mod recover;
mod release;
mod repo;
mod report;
mod restart;
mod review;
mod routes;
mod routine_file;
mod routine_page;
mod routine_wizard;
mod routines;
mod sh;
mod state;
mod setup;
mod shell_write;
mod status;
mod task_note;
mod tasks;
#[cfg(test)]
mod test_seal;
mod typing;
mod update;
mod update_check;
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
                                  --profile <name> starts it with that profile instead of task.profile or task,
                                  --no-focus keeps you where you are, --after <task[@repo]> (repeatable) queues it
                                  to start from the default branch once each of those tasks ended as done and,
                                  where kelpie watches CI, its CI is green; the user's or the lead's)
  kelpie after [name]             the queued tasks: what each waits for, with that task's state; one blocked because
                                  a task it waited for was dropped, reset or is gone waits for you (--now starts it
                                  anyway and tells its Claude what was not done, --drop drops the wait)
  kelpie move <name> [file ...]   move work under way in the main checkout into a task: branch <name> if it
                                  exists, else the unpushed commits, plus the given uncommitted files
                                  (--pane <id> moves that herdr pane along, --no-focus stays put)
  kelpie list [--json]            the pool's worktrees and their tasks, with each one's port and dev_url
  kelpie routes [--plain-http]    print a Caddy config with a site per pooled worktree with a port, at the host its
                                  dev_url names, proxied to localhost:<port>, for the repos that turn on
                                  worktree.dev_route (opt-in per repo); --plain-http serves each as http://<host> for
                                  a proxy in front that ends TLS; writes nothing
  kelpie status [--json]          every task and Claude session, grouped by what it needs from you
  kelpie board [--overlay]        the board: tasks and sessions, with keys to act on them
  kelpie areas                    the vault's areas with their repo: main against origin, last release, notes, actions
  kelpie lead                     go to the lead Space (creating it), or back to where you were
                                  (--first only moves the lead Space to the top of herdr's Spaces; --restart
                                  restarts an idle lead's Claude on its current role and settings, resuming
                                  its conversation; --toggle-chat hides the lead's chat so the board takes the
                                  whole width, or shows it again; --revive-board, run by herdr's startup, waits
                                  in the background for herdr's restore and starts the board in the lead Space's
                                  board pane when that pane sits at a shell)
  kelpie focus                    in the lead Space, move focus between the board and the lead's chat; from
                                  anywhere else, go to the lead's chat
  kelpie peers                    who you are and the other task sessions, with what each is doing
  kelpie tell <to> <text>         message another session: a task (every half of it), task@repo, or lead; held
                                  while it waits for an answer, and logged
  kelpie tell --force <to> <text> yours or the lead's: deliver now, clearing a draft from its input box (logged word
                                  for word; Ctrl+Y in its pane brings it back), with what is held for it first
  kelpie release <to>             yours or the lead's: deliver what is held for a session now, as tell --force does
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
  kelpie choose <task> <n> --prompt <id>
                                  answer the prompt a task's Claude shows in its own pane (a permission or trust question)
                                  with option <n>, as kelpie told the lead; refused when that prompt is no longer on its
                                  screen; the user's, or the lead's on the user's explicit choice
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
  kelpie land [name]              rebase a task onto the default branch without pushing; on a conflict, its Claude
                                  resolves the rebase (push rebases by itself, so land is optional)
  kelpie follow [name] [on|off]   keep a task rebased on the default branch (also u on the board), off by default: when
                                  main moves, the keeper rebases it while its Claude is idle, its tracked files are
                                  committed and no rebase, review or waiting push is under way; a conflict goes to its
                                  Claude once per new main; never pushes; without on or off, says whether it follows
  kelpie tidy <name> [note]       ask the task's Claude to reshape its commits by the rules skill (task.rules_skill)
  kelpie review [name]            send the task to review: a separate, read-only Claude reviews its commits in the
                                  background, by the skill kelpie:review and the rules skill; the task waits under To
                                  review, and push with it, until a verdict ok on its current code (--withdraw
                                  takes it out of review, yours only: the reviewer stops, its verdict goes, push is
                                  offered again)
  kelpie evidence [name] [--json] the test runs kelpie recorded for the task, newest first: the command, its result, the
                                  commit it ran on and whether the code changed since (task.test_commands adds commands)
  kelpie approve <name>           the lead's word that a task's commits hold to the rules skill; until a commit
                                  changes, the board shows it ready to push
  kelpie push [name]              fetch, rebase the task onto the default branch and push it, then close it as done
                                  (once CI is green, when kelpie watches it; see kelpie ci); when the new upstream
                                  commits changed files the task changes, its Claude runs their tests first and kelpie
                                  pushes once it reports ready with the same commits; a conflict goes to its Claude
                                  (--through <sha> pushes its commits up to that one and keeps the task open, the rest
                                  of its branch on top of what was pushed); the user's step, or the lead's for a task the
                                  user gave push permission on the board (a), or in a repo they did on the Areas page (a)
                                  once its reviewer passed its current code; the lead is told what landed
  kelpie ci [--json]              the CI watches: after a full push to a GitHub origin, with gh logged in, the task stays
                                  open under CI running until every workflow run for the pushed commit is green, then
                                  closes as done; a failed run moves it to Needs you and its Claude gets the failing
                                  job's log ([ci] watch = false turns it off); the board checks every 30 seconds
  kelpie ci tick                  one round of checks for every running watch, as the board does
  kelpie routines [--json]        the scheduled routines from config.toml and the repo configs ([[routines]]), each with
                                  its schedule, repo, mode, next run and last outcome (ok, findings, failed, running);
                                  the keeper starts each due one as a task (--no-focus), never while its last run's
                                  task is still open
  kelpie routines tick            one round of the routine scheduler, as the keeper runs every minute
  kelpie routine add --title <name> --repo <repo> --at <time> | --every <interval> [--mode report|plan|fix]
                  [--push] [--paused] [--id <id>] <prompt>
                                  add a routine to config.toml, as the board's Routines page does (n); the id comes
                                  from the title unless --id names one; --push lets the lead push a fix routine's fix
  kelpie routine run <id>         start a routine's run now, by the same rules
  kelpie routine ok <summary>     run by a routine's task when all is well: records the run as ok and closes the task,
                                  which must have no changes and no commits
  kelpie done [name]              after pushing: close the task and free its worktree
  kelpie drop [name] [--force]    throw a task away; --force discards uncommitted work and unpushed commits
  kelpie reset [name]             stop a task for good: discard its work, free its worktree, close its Space, and put
                                  the backlog item it was started from back in the Backlog column
  kelpie profiles [--json]        the profiles a task can start with (kelpie start --profile, task.profile): the built-in
                                  task and reviewer, then each file in ~/.config/kelpie/profiles, with what each is for
  kelpie config [--repo <path>]   the config layers found (the repo config ~/.config/kelpie/repos/<repo>.toml,
                                  config.toml, defaults; a symlink with its target) and every key with its value in
                                  effect, the layer it comes from and what it does (--get <key> prints one
                                  machine-wide value, for scripts)
  kelpie doctor                   check that this machine has what kelpie needs, with the fix for each gap; checks
                                  config.toml and every repo config
  kelpie setup vault              install the optional vault plugin in Claude Code (marketplace, plugin, its options),
                                  showing each claude command and running it only on your y
  kelpie restart [--all]          restart, with claude --resume in its own pane and with its own flags, each Claude
                                  session that runs older code than is installed (Claude Code, a plugin, MCP servers,
                                  kelpie) and is idle, not the pane you are in and without background work (that one
                                  is asked about on the board); a task's Claude gets its role and settings written
                                  anew, and one running without them counts as stale; --all every idle one; works
                                  with [restart] auto = false
  kelpie keeper                   the keeper: one background kelpie per machine that runs the routines (stale-session
                                  restarts, the update check, held messages, delivery checks of typed prompts, the
                                  scheduled routines, rebasing the tasks that follow main, starting queued tasks) with no
                                  board open; herdr's
                                  startup, the board and kelpie update start it ([keeper] enabled = false hands them to
                                  the board) (--start starts it
                                  detached unless one runs, --status shows its pid, binary, uptime and each routine's
                                  last run, --stop stops it)
  kelpie issues [--json] [--all]  the problems and wishes about kelpie itself saved on this machine (open ones;
                                  --all every status): id, kind, status, the version they were filed on, GitHub link
  kelpie issue add --kind bug|wish --title <title> [<text>|-]
                                  save one, the text from the argument or stdin: what happened, what you expected,
                                  how to reproduce; the user's or the lead's (a task tells the lead)
  kelpie issue show|retry <id>    print one; retry marks it worth trying again
  kelpie issue close <id> fixed|wontfix|duplicate
  kelpie issue link <id> <number|url>
                                  tie it to its GitHub issue
  kelpie issue send <id>          print the text and a prefilled new-issue URL on GitHub (shortened past GitHub's
                                  limit), and open it in the browser for you to check and submit; nothing is sent
                                  without your Submit (kelpie update looks the issues up with gh and asks the lead
                                  which ones the new commits likely fix)
  kelpie update                   update everything to this machine's channel ([update] channel: release, the
                                  highest vX.Y.Z tag, or main, origin's default branch): the herdr plugin (rebuilt),
                                  the Claude Code plugin, the vault plugin when installed; list each version before →
                                  after, and restart each Claude session that is idle and not the pane you are in
                                  with claude --resume (the rest follow when idle; [restart] auto = false restarts none)
                                  (--check changes nothing: fetches and shows installed against the channel's latest;
                                  the keeper checks every 6 hours and the board shows ↑ when there is a newer one, U updates;
                                  --from <branch or path> installs that branch on origin or that checkout's commits
                                  until --stable returns to the channel)

With the vault plugin installed, enabled and configured, the vault holds each task: start opens the task's note
(or claims the vault backlog note it starts from), edits wait until the note's Problem is written, push writes
the repo's log line from the commit subjects, done waits for the note's solution and decisions and then closes
it, and reset puts it back in the backlog. Without the plugin none of this runs.

Without a name, land, follow, review, evidence, push, done, drop and reset act on the task whose worktree you are in.
-C <dir> runs as if started in <dir>. kelpie --version (or kelpie version) prints the version and the commit and
date it was built from.";

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
            if ["--pane", "-C", "--repo", "--from", "--mode", "--state", "--next", "--backlog", "--option", "--recommend", "--plan", "--card", "--through", "--priority", "--get", "--title", "--id", "--at", "--every", "--kind", "--prompt", "--after", "--profile"].contains(&argument.as_str()) {
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
        let setup = row["port"].as_u64().map_or(setup.to_string(), |port| format!("port {port} · {setup}"));
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
        None if arguments.flag("--version") => Ok(version(env!("KELPIE_BUILT_FROM"))),
        Some("version") => Ok(version(env!("KELPIE_BUILT_FROM"))),
        Some("start") => {
            let name = name.ok_or("kelpie start needs a task name")?;
            let prompt = arguments.positionals[2..].join(" ");
            let prompt = (!prompt.is_empty()).then_some(prompt.as_str());
            let mut repos = arguments.values_of("--repo").into_iter().map(repo::locate).collect::<Result<Vec<_>>>()?;
            if repos.is_empty() {
                repos.push(cwd);
            }
            let after = arguments.values_of("--after");
            if !after.is_empty() {
                if arguments.value("--from").is_some() {
                    return Err("kelpie start --after starts from the default branch, so it takes no --from".to_string());
                }
                if let Some(queued) = tasks::queue(&repos, name, prompt.unwrap_or_default(), &after, arguments.value("--mode"), arguments.value("--profile"), arguments.value("--backlog"))? {
                    return Ok(queued);
                }
            }
            tasks::refuse_queued_name(name)?;
            let options = tasks::Start { prompt, focus: !arguments.flag("--no-focus"), from: arguments.value("--from"), mode: arguments.value("--mode"), profile: arguments.value("--profile"), backlog: arguments.value("--backlog"), note: true };
            // One task across repos: a half per repo, the first one focused.
            let started: Vec<Result<String>> = repos.iter().enumerate()
                .map(|(index, repo)| tasks::start(repo, name, &tasks::Start { focus: options.focus && index == 0, ..options }))
                .collect();
            let report = started.iter().map(|result| match result { Ok(text) | Err(text) => text.as_str() }).collect::<Vec<_>>().join("\n");
            if started.iter().any(Result::is_err) { Err(report) } else { Ok(report) }
        }
        Some("after") => tasks::after_command(&arguments.positionals[1..], arguments.flag("--now"), arguments.flag("--drop")),
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
        Some("routes") => Ok(routes::print(arguments.flag("--plain-http"))),
        Some("lead") if arguments.flag("--first") => lead::keep_first(),
        Some("lead") if arguments.flag("--restart") => lead::restart(),
        Some("lead") if arguments.flag("--renew-board") => lead::renew_board(),
        Some("lead") if arguments.flag("--revive-board") && arguments.flag("--wait") => lead::revive_board(),
        Some("lead") if arguments.flag("--revive-board") => lead::revive_board_detached(),
        Some("lead") if arguments.flag("--toggle-chat") => lead::toggle_chat(),
        Some("lead") => lead::toggle(),
        Some("focus") => lead::focus_toggle().map(|_| String::new()),
        Some("peers") => peers::peers(&cwd),
        Some("tell") => {
            // Taken as given, since a message can start with "-"; only --force right after tell is a flag.
            let after: Vec<String> = std::env::args().skip_while(|argument| argument != "tell").skip(1).collect();
            let forced = after.first().is_some_and(|argument| argument == "--force");
            let after = &after[usize::from(forced)..];
            let to = after.first().ok_or("kelpie tell needs a recipient: a task, task@repo or lead")?;
            let text = after[1..].join(" ");
            if forced { messages::tell_forced(to, Some(&text)) } else { messages::tell(&cwd, to, &text) }
        }
        Some("release") => messages::tell_forced(name.ok_or("kelpie release needs the task whose held messages go through")?, None),
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
        Some("choose") => menu::choose(name, arguments.positional(2), arguments.value("--prompt")),
        Some("defer") => questions::defer(name.ok_or("kelpie defer needs the task whose question waits for the user")?),
        Some("away") => permit::away(name),
        Some("hook") => {
            let mut input = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
            if arguments.flag("--review") {
                return Ok(review::guard(&input));
            }
            if name == Some("evidence") {
                return Ok(arguments.positional(2).map(|address| evidence::hook(address, &input)).unwrap_or_default());
            }
            if name == Some("gate") {
                return Ok(arguments.positional(2).map(|address| task_note::hook(address, &input)).unwrap_or_default());
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
        Some("follow") => tasks::follow(&cwd, &arguments.positionals[1..]),
        Some("push") => tasks::push(&cwd, name, arguments.value("--through"), arguments.value("--through").is_some()),
        Some("tidy") => tasks::tidy(&cwd, Some(name.ok_or("kelpie tidy needs a task name")?), &arguments.positionals[2..].join(" ")),
        Some("review") if arguments.flag("--run") => review::run(&cwd),
        Some("review") if arguments.flag("--withdraw") => review::withdraw(&cwd, name),
        Some("review") => review::request(&cwd, name),
        Some("evidence") => evidence::print(&cwd, name, arguments.flag("--json")),
        Some("approve") => tasks::approve(&cwd, Some(name.ok_or("kelpie approve needs a task name")?)),
        Some("ci") if name == Some("tick") => ci::tick_cli(),
        Some("ci") if name.is_some() => Err("kelpie ci knows tick: kelpie ci [--json] or kelpie ci tick".to_string()),
        Some("ci") => Ok(ci::list(&lead::state_dir()?, arguments.flag("--json"))),
        Some("routines") if name == Some("tick") => routines::tick_cli(),
        Some("routines") if name.is_some() => Err("kelpie routines knows tick: kelpie routines [--json] or kelpie routines tick".to_string()),
        Some("routines") => routines::list(arguments.flag("--json")),
        Some("routine") if name == Some("run") => routines::run_now(arguments.positional(2).ok_or("kelpie routine run needs the routine's id; kelpie routines lists them")?),
        Some("routine") if name == Some("ok") => routines::ok(&cwd, &arguments.positionals[2..].join(" ")),
        Some("routine") if name == Some("add") => routine_file::add_cli(&routine_file::Asked {
            title: arguments.value("--title"), id: arguments.value("--id"), repo: arguments.value("--repo"), at: arguments.value("--at"), every: arguments.value("--every"),
            mode: arguments.value("--mode"), push: arguments.flag("--push"), paused: arguments.flag("--paused"), prompt: &arguments.positionals[2..].join(" "),
        }),
        Some("routine") => Err("kelpie routine knows add, run <id> and ok <summary>".to_string()),
        Some("done") => tasks::done(&cwd, name),
        Some("drop") => tasks::drop(&cwd, name, arguments.flag("--force")),
        Some("reset") => tasks::reset(&cwd, name),
        Some("link") => {
            let name = name.ok_or("kelpie link needs a task name and the item")?;
            tasks::link(&cwd, name, &arguments.positionals[2..].join(" "))
        }
        Some("profiles") => profiles::print(arguments.flag("--json")),
        Some("config") => match arguments.value("--get") {
            Some(key) => config::get(key),
            None => config::show(&cwd, arguments.value("--repo")),
        },
        Some("issues") => Ok(issues::list_in(&issues::dir(&lead::state_dir()?), arguments.flag("--all"), arguments.flag("--json"))),
        Some("issue") => issue(arguments, name),
        Some("setup") if name == Some("vault") => setup::vault(),
        Some("setup") => Err("kelpie setup knows vault: kelpie setup vault".to_string()),
        Some("restart") => restart::now_cli(arguments.flag("--all")),
        Some("keeper") if arguments.flag("--status") => keeper::status_cli(),
        Some("keeper") if arguments.flag("--stop") => keeper::stop(),
        Some("keeper") if arguments.flag("--start") => keeper::start(),
        Some("keeper") => keeper::run(),
        Some("update") if arguments.flag("--check") => Ok(update::check(&doctor::Env::current().claude_dir)),
        Some("update") if arguments.flag("--finish") => update::finish(&doctor::Env::current().claude_dir).map(|lines| lines.join("\n")),
        Some("update") => {
            let request = match (arguments.value("--from"), arguments.flag("--stable")) {
                (Some(_), true) => return Err("kelpie update takes --from <branch or path> or --stable, not both".to_string()),
                (Some(from), false) => release::Request::From(from.to_string()),
                (None, true) => release::Request::Stable,
                (None, false) => release::Request::Channel,
            };
            update::run(&doctor::Env::current().claude_dir, true, &request)
        }
        None | Some("help") => Ok(USAGE.to_string()),
        Some(other) => Err(format!("Unknown command \"{other}\"\n\n{USAGE}")),
    }
}

fn issue(arguments: &Arguments, action: Option<&str>) -> Result<String> {
    let dir = issues::dir(&lead::state_dir()?);
    let id = || arguments.positional(2).ok_or_else(|| format!("kelpie issue {} needs the issue's id; kelpie issues lists them", action.unwrap_or_default()));
    let caller = permit::caller;
    match action {
        Some("add") => {
            let body = issues::read_body(&arguments.positionals[2..], arguments.flag("-"));
            issues::add_in(&dir, &caller(), &issues::Filing { kind: arguments.value("--kind"), title: arguments.value("--title"), body: &body }, env!("KELPIE_BUILT_FROM"), messages::now())
        }
        Some("show") => issues::show_in(&dir, id()?),
        Some("close") => issues::close_in(&dir, &caller(), id()?, arguments.positional(3)),
        Some("retry") => issues::retry_in(&dir, &caller(), id()?),
        Some("link") => issues::link_in(&dir, &caller(), id()?, arguments.positional(3)),
        Some("send") => {
            let opener = board::images::image_viewer(std::env::consts::OS, &|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
            issues::send_in(&dir, &caller(), id()?, opener, &issues::open_url, messages::now())
        }
        _ => Err("kelpie issue knows add, show <id>, close <id> fixed|wontfix|duplicate, retry <id>, link <id> <number|url> and send <id>".to_string()),
    }
}

// Many commits carry one version number, so the commit and its date say which code this binary is.
fn version(built: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    if built.is_empty() { format!("kelpie {version}") } else { format!("kelpie {version} ({built})") }
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
    if matches!(arguments.positional(0), Some("board" | "start" | "after" | "land" | "push" | "done" | "move" | "update" | "ci" | "routine" | "routines")) {
        login_env::import();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_names_the_commit_and_its_date_the_binary_was_built_from() {
        let number = env!("CARGO_PKG_VERSION");
        assert_eq!(version("db8963f 2026-10-21"), format!("kelpie {number} (db8963f 2026-10-21)"));
        assert_eq!(version(""), format!("kelpie {number}"), "a build outside git still says its version");
        let built = env!("KELPIE_BUILT_FROM");
        if Path::new(env!("CARGO_MANIFEST_DIR")).join(".git").exists() {
            let (sha, date) = built.split_once(' ').unwrap_or_else(|| panic!("a build from git names its commit: {built:?}"));
            assert!(sha.len() >= 7 && sha.chars().all(|character| character.is_ascii_hexdigit()), "{built}");
            let parts: Vec<&str> = date.split('-').collect();
            assert!(parts.len() == 3 && [4, 2, 2] == [parts[0].len(), parts[1].len(), parts[2].len()] && date.chars().all(|character| character.is_ascii_digit() || character == '-'), "{built}");
        }
    }
}
