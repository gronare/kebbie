mod actions;
mod agent;
mod area_page;
mod areas;
mod backlog;
mod backlog_file;
mod changes;
mod board;
mod ci;
mod config;
mod doctor;
mod docs;
mod evidence;
#[cfg(test)]
mod executable;
mod fetch;
mod herdr;
mod hub;
mod inbox;
mod issue_recheck;
mod issues;
mod keeper;
mod keys;
mod known;
mod lead;
mod lead_restart;
mod login_env;
mod menu;
mod messages;
mod notice;
mod notify;
mod peers;
mod phase;
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
mod rules_check;
mod routes;
mod routine_file;
mod routine_page;
mod routine_wizard;
mod routines;
mod sandbox;
mod sh;
mod state;
mod setup;
mod shell_write;
mod signals;
mod status;
mod task_note;
mod tasks;
#[cfg(test)]
mod test_seal;
mod trusted_spawn;
mod typing;
mod update;
mod update_check;
mod update_progress;
mod usage;
mod vault;
mod vault_cli;
mod when;

use std::path::Path;
use std::process::ExitCode;

use sh::Result;

const USAGE: &str = "kelpie: herding coding agents in herdr

  kelpie start <name> [prompt]    start a task in a pooled worktree, in its own Space, with Claude
                                  (--repo <name> finds or clones that repo, repeated for a task across repos,
                                  --from <ref> branches off it
                                  instead of the default branch, --mode <mode> is Claude's permission mode,
                                  --profile <name> starts it with that profile instead of task.profile or task,
                                  --no-focus keeps you where you are, --anyway starts from a vault note kelpie on
                                  another machine works on, --after <task[@repo]> (repeatable) queues it
                                  to start from the default branch once each of those tasks ended as done and,
                                  where kelpie watches CI, its CI is green; the user's or the lead's;
                                  --at <time> queues it to start at that time (15:00, tomorrow 09:00, fri 09:00,
                                  24 oct 09:00, 2026-10-24 09:00, in 2h), with --after once both hold;
                                  --why <reason> says on its Queued row why it waits)
  kelpie after [name]             the queued tasks: what each waits for, with that task's state; one blocked because
                                  a task it waited for was dropped, reset or is gone waits for you (--now starts it
                                  anyway and tells its Claude what was not done, --drop drops the wait,
                                  --at <time> sets when it starts)
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
                                  its conversation, and --anyway also while it has background work running;
                                  --toggle-chat hides the lead's chat so the board takes the whole width, or
                                  shows it again; --revive-board, run by herdr's startup, waits
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
                                  with ready, --card <file> is the sign-off card the push dialog shows above the commits;
                                  ready is refused while a commit lacks a subject, a blank line and a body (with
                                  kelpie:work-rules), and the keeper then starts a read-only reviewer that checks the
                                  commits against the rules skill: a pass approves them, a break goes to the task and a
                                  line to the lead, no verdict in 15 minutes leaves the check to the lead
  kelpie ask --option \"<label>: <meaning>\" … --recommend <n> <question>
                                  run by a task's Claude to ask you what you want (scope, behaviour, a trade-off that is
                                  yours); a question of how goes to the lead with kelpie tell lead. The question opens
                                  with one plain sentence on what the task is doing, then asks; each of the two to four
                                  options says what you get with it; no flags, commit ids, file names or internal terms
                                  you did not use. Example: \"This task makes a waiting task easy to read on the board.
                                  Do you also want to start a task at a time you pick?\" --option \"Yes: you can say
                                  'start at 18:00' when you queue a task; this task takes longer to reach you\" --option
                                  \"Not now: a queued task starts once the tasks it waits for are done\"
                                  An option ending in \" +comment\" opens a comment field and sends only with your text.
                                  --plan vault:<note> or --plan <file> asks you to approve a plan instead. It waits on
                                  the board, where you answer it, the lead gets a copy, and the answer reaches the task
                                  as a message from you
  kelpie answered <task>          a task's question is settled elsewhere (in its Space, dropped, or sent back to be asked again): it
                                  leaves the board, so a kelpie tell to that task is no longer held
  kelpie notify <text>            run notify.command with this text (KELPIE_EVENT=summary): the lead's word to the user
                                  when they are away; tasks that need you or are ready, and questions, notify by themselves
  kelpie decide <task> <answer>   the lead's own answer to a task's question about how, not what, whether or not you
                                  are here; it closes the question on the board and is recorded for your return when away
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
                                  review, and push with it, until a verdict ok on its current code, which is its
                                  rules approval too, so no separate rules check runs (--withdraw
                                  takes it out of review, yours only: the reviewer stops, its verdict goes, push is
                                  offered again)
  kelpie evidence [name] [--json] the test runs kelpie recorded for the task, newest first: the command, its result, the
                                  commit it ran on and whether the code changed since (task.test_commands adds commands)
  kelpie approve <name>           yours or the lead's word that a task's commits hold to the rules skill, an override:
                                  the reviewer gives it by itself when a task reports ready; until a commit changes,
                                  the board shows it ready to push
  kelpie push [name]              put the task in its repo's push line; at the head, fetch, rebase it onto origin's
                                  default branch and push it, then close it as done (once CI is green, when kelpie
                                  watches it; see kelpie ci); one push per repo at a time, in the order they were
                                  asked for, and a task is rebased only when its turn comes; when the new upstream
                                  commits changed files the task changes, its Claude runs their tests first and kelpie
                                  pushes once it reports ready with the same commits; a conflict goes to its Claude;
                                  either way it keeps the head while its Claude works, and leaves the line for Needs
                                  you after 30 minutes without a ready report, or new commits nobody checked
                                  (--cancel takes it out of the line, unpushed)
                                  (--through <sha> pushes its commits up to that one and keeps the task open, the rest
                                  of its branch on top of what was pushed); the user's step, or the lead's for a task the
                                  user gave push permission on the board (a), or in a repo they did on the Areas page (a)
                                  once its reviewer passed its current code; the lead is told what landed
  kelpie stage [name]             put a ready task's commits on the main checkout's local default branch, unpushed, to
                                  try them there (repos with worktree.stage_on_main only); several go on in the order
                                  staged, a conflict goes to its Claude and leaves main as it was; the task stays open
                                  until main is pushed and its commits are on origin, and never goes through the push
                                  line meanwhile; in kelpie's own install root on the main channel, kelpie rebuilds;
                                  the user's step, or the lead's with the permission it would push the task with
  kelpie unstage [name]           take a staged task's commits off local main again, keeping the others staged and its
                                  branch as it is
  kelpie ci [--json]              the CI watches: after a full push to a GitHub origin, with gh logged in, the task stays
                                  open under CI running until every workflow run for the pushed commit is green, then
                                  closes as done; a failed run moves it to Needs you and its Claude gets the failing
                                  job's log ([ci] watch = false turns it off); the keeper checks every 30 seconds
  kelpie ci tick                  one round of checks for every running watch, as the board does
  kelpie ci outside \"<reason>\"    run by a task's Claude: its red CI is outside the code (deploy window, infra,
                                  secrets, a flake); the row says so, nothing is waived
  kelpie ci dismiss <task> [--why <reason>]
                                  yours alone (C on the board): keep the failed run on record with the reason, drop the
                                  watch and the push waiting for a fix; the task closes once its commits are on main
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
  kelpie done [name]              after pushing: close the task and free its worktree; the keeper does it by itself every
                                  minute once a task's commits are on the default branch, unless its Claude is mid-turn or
                                  asking, its worktree has changes or a stash, its CI is red, or you pushed it and kept it open
  kelpie done --landed            one round of that check, as the keeper runs it every minute
  kelpie drop [name] [--force]    throw a task away; --force discards uncommitted work and unpushed commits
  kelpie reset [name]             stop a task for good: discard its work, free its worktree, close its Space, and put
                                  the backlog item it was started from back in the Backlog column
  kelpie profiles [--json]        the profiles a task can start with (kelpie start --profile, task.profile): the built-in
                                  task, reviewer and ops (ssh, scp and rsync outside the sandbox), then each file in
                                  ~/.config/kelpie/profiles, with what each is for
  kelpie config [--repo <path>]   the config layers found (the repo config ~/.config/kelpie/repos/<repo>.toml,
                                  config.toml, defaults; a symlink with its target) and every key with its value in
                                  effect, the layer it comes from and what it does (--get <key> prints one
                                  machine-wide value, for scripts)
  kelpie docs [words ...]         where kelpie's docs are on this machine (docs/ in the install's checkout); with
                                  words, the pages whose title or headings match, each with its path and first
                                  sentence, best first; without, the docs index by section; never needs the network,
                                  and without the docs points at kelpie help and the docs online
  kelpie hub [status]             the hub this machine posts its board to, its devices by name with when each was
                                  paired and last seen and whether it has notifications on, and the last post
  kelpie hub connect [--name <name>]
                                  connect this machine to a hub through your browser: [hub] url, or asks for the kelpie
                                  hub or your own (tested, then saved as [hub] url); prints a link, a code to check
                                  and a QR code, opens the browser and waits for you to approve it there; the browser
                                  that approves it can show the board at once; goes by [machine] name, which --name
                                  sets, or asks for it in a terminal; H on the board does the same; yours only
  kelpie hub add-device           add a phone or browser: shows a link and its QR code, then six digits to compare
                                  with the phone's before it admits the phone on your y; yours only, in your own terminal
  kelpie hub remove-device <name|id>
                                  stop showing this machine's board on one device; it keeps the boards of your other
                                  machines; yours only
  kelpie hub join <url> <code> [--name <name>]
                                  join a kelpie hub with a code from kelpie-hub admin join-code, for scripts: makes
                                  this machine's keys, kept in kelpie's state and never printed; it goes by [machine]
                                  name, which --name sets, or asks for it in a terminal (the short host name
                                  otherwise); yours only
  kelpie hub pair                 pair a phone by a code typed in the hub's web app, then six digits to compare
                                  with the phone's before it admits the phone; the keeper then posts the board, sealed
                                  to every paired phone, when it changes and every minute, and a notice when a task
                                  needs you, asks, is ready to push or its CI fails; yours only, in your own terminal
  kelpie hub move <url>           the hub moved: post there once <url> answers as a hub; move only to a hub you run or trust;
                                  a redirect to another host is never followed by itself; yours only
  kelpie hub leave                disconnect this machine: take each device off its board and remove the keys and token
  kelpie doctor                   check that this machine has what kelpie needs, with the fix for each gap; checks
                                  config.toml and every repo config, and whether tasks run in the sandbox
                                  (task.sandbox; on Linux it needs bubblewrap, socat and the seccomp filter)
  kelpie setup [--yes]            put kelpie on PATH (~/.local/bin/kelpie), install or update its Claude Code plugin
                                  and add its keys to herdr's config.toml where missing, appending and asking first
                                  (--yes does not ask, and never writes through a link); says whether herdr needs a
                                  restart and ends with kelpie doctor's warnings; safe to run again
  kelpie setup vault              install the optional vault plugin in Claude Code (marketplace, plugin, its options),
                                  showing each claude command and running it only on your y
  kelpie setup codex [--uninstall]
                                  install kelpie's plugin into Codex's plugin cache, on your y, so tasks whose profile
                                  runs Codex report, get their role and are recorded; never edits Codex's config.toml
                                  or hooks.json (--uninstall removes the plugin)
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
  kelpie issues [--json] [--all]  the bugs in kelpie you ran into and your wishes for it, saved on this machine (open
                                  ones; --all every status): id, kind, status, the version they were filed on, GitHub link
  kelpie issue add --kind bug|wish --title <title> [<text>|-]
                                  save one, the text from the argument or stdin: what happened, what you expected,
                                  how to reproduce; the user's or the lead's (a task tells the lead)
  kelpie issue hold <line> | --take
                                  the lead's note of what kelpie did, held on this machine for a calm moment to propose
                                  it; one at a time, a new one replaces it, and it is never an issue, offered or sent;
                                  --take prints it and clears it; nothing is held with [issues] propose = false
  kelpie issue list [--json] [--all]
                                  the same as kelpie issues
  kelpie issue show|retry <id>    print one; retry marks it worth trying again
  kelpie issue close <id> fixed|wontfix|duplicate
  kelpie issue reopen <id>        open a closed one again, noted as come back on this build
  kelpie issue link <id> <number|url>
                                  tie it to its GitHub issue
  kelpie issue send <id>          optional: print the text and a prefilled new-issue URL on GitHub (shortened past GitHub's
                                  limit), and open it in the browser for you to check and submit; nothing is sent
                                  without your Submit (kelpie update looks the issues up with gh, and the keeper
                                  rechecks each open one filed on older code against the new commits and closes
                                  the ones they plainly fix)
  kelpie issue report <id>        show a bug in kelpie you ran into as it would go to kelpie's maintainers (paths under
                                  your home as ~, no repo names, remote URLs or tokens) and send it on your y, signed with
                                  this install's key; the board counts each new one in its header and B lists them ([report]
                                  text = \"never\" hides the offer); sent once, and the issue keeps the report's id
  kelpie issue report --always | --ask
                                  in your own terminal: let kelpie send each bug you run into without asking, redacted,
                                  each send a line under messages (with [report] text = \"always\" in config.toml);
                                  --ask asks again before every one
  kelpie update                   update everything to this machine's channel ([update] channel: release, the
                                  highest vX.Y.Z tag whose release binary is published, or main, origin's default
                                  branch): the herdr plugin (the release's binary; built from source only on main and
                                  with --from), the Claude Code plugin, the vault plugin when installed; list each
                                  version before → after, and restart each Claude session that is idle and not the
                                  pane you are in with claude --resume (the rest follow when idle; [restart] auto =
                                  false restarts none);
                                  every other running session gets a message, held while it is mid-turn, with what
                                  changed for it since it started: new or changed role text, skills and commands
                                  (--check changes nothing: fetches and shows the installed binary's version and
                                  commit against the channel's latest; the keeper checks every 6 hours and the
                                  board shows ↑ when there is a newer one, U updates;
                                  --from <branch or path> installs that branch on origin or that checkout's commits
                                  until --stable returns to the channel)

With the vault plugin installed, enabled and configured, the vault holds each task: start opens the task's note
(or claims the vault backlog note it starts from), edits wait until the note's Problem is written, push writes
the repo's log line from the commit subjects, done waits for the note's solution and decisions and then closes
it, and reset puts it back in the backlog. Without the plugin none of this runs.

Without a name, land, follow, review, evidence, push, stage, unstage, done, drop and reset act on the task whose worktree you are in.
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
            if ["--pane", "-C", "--repo", "--from", "--mode", "--state", "--next", "--backlog", "--option", "--recommend", "--plan", "--card", "--through", "--priority", "--get", "--title", "--id", "--at", "--every", "--kind", "--prompt", "--after", "--profile", "--why", "--name"].contains(&argument.as_str()) {
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
            let starts_at = arguments.value("--at").map(|text| crate::when::parse(text, messages::now(), &crate::recover::local_offset)).transpose().map_err(|error| format!("--at: {error}"))?;
            if !after.is_empty() || starts_at.is_some() {
                if arguments.value("--from").is_some() {
                    return Err("kelpie start --after and --at start from the default branch, so they take no --from".to_string());
                }
                if let Some(queued) = tasks::queue(&repos, &tasks::QueueAsked { name, prompt: prompt.unwrap_or_default(), after: &after, mode: arguments.value("--mode"), profile: arguments.value("--profile"), backlog: arguments.value("--backlog"), title: None, why: arguments.value("--why"), starts_at })? {
                    return Ok(queued);
                }
            }
            tasks::refuse_queued_name(name)?;
            let options = tasks::Start { prompt, focus: !arguments.flag("--no-focus"), from: arguments.value("--from"), mode: arguments.value("--mode"), profile: arguments.value("--profile"), backlog: arguments.value("--backlog"), note: true, anyway: arguments.flag("--anyway") };
            // One task across repos: a half per repo, the first one focused.
            let started: Vec<Result<String>> = repos.iter().enumerate()
                .map(|(index, repo)| tasks::start(repo, name, &tasks::Start { focus: options.focus && index == 0, ..options }))
                .collect();
            let report = started.iter().map(|result| match result { Ok(text) | Err(text) => text.as_str() }).collect::<Vec<_>>().join("\n");
            if started.iter().any(Result::is_err) { Err(report) } else { Ok(report) }
        }
        Some("after") => tasks::after_command(&arguments.positionals[1..], arguments.flag("--now"), arguments.flag("--drop"), arguments.value("--at")),
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
        Some("lead") if arguments.flag("--restart") => lead::restart(arguments.flag("--anyway")),
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
            if name == Some("inbox") {
                return Ok(arguments.positional(2).map(|address| inbox::hook(address, &input)).unwrap_or_default());
            }
            if name == Some("files") {
                return Ok(sandbox::file_hook(arguments.positional(2), arguments.positional(3), arguments.positional(4), &input));
            }
            if name == Some("gate") {
                return Ok(arguments.positional(2).map(|address| task_note::hook(address, &input)).unwrap_or_default());
            }
            if name == Some("codex") {
                return Ok(agent::codex::hook(&input));
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
        Some("push") if arguments.flag("--cancel") => tasks::cancel_push(&cwd, name),
        Some("push") => tasks::push(&cwd, name, arguments.value("--through"), arguments.value("--through").is_some()),
        Some("stage") => tasks::stage(&cwd, name, true),
        Some("unstage") => tasks::unstage(&cwd, name),
        Some("tidy") => tasks::tidy(&cwd, Some(name.ok_or("kelpie tidy needs a task name")?), &arguments.positionals[2..].join(" ")),
        Some("review") if arguments.flag("--run") && arguments.flag("--rules") => rules_check::run(&cwd),
        Some("review") if arguments.flag("--run") => review::run(&cwd),
        Some("review") if arguments.flag("--withdraw") => review::withdraw(&cwd, name),
        Some("review") => review::request(&cwd, name),
        Some("evidence") => evidence::print(&cwd, name, arguments.flag("--json")),
        Some("approve") => tasks::approve(&cwd, Some(name.ok_or("kelpie approve needs a task name")?)),
        Some("ci") if name == Some("tick") => ci::tick_cli(),
        Some("ci") if name == Some("dismiss") => tasks::dismiss_ci(&cwd, arguments.positional(2), arguments.value("--why")),
        Some("ci") if name == Some("outside") => tasks::ci_outside(&arguments.positionals[2..].join(" ")),
        Some("ci") if name.is_some() => Err("kelpie ci knows tick, outside and dismiss: kelpie ci [--json], kelpie ci tick, kelpie ci outside \"<reason>\" or kelpie ci dismiss <task> [--why <reason>]".to_string()),
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
        Some("done") if arguments.flag("--landed") => tasks::landed_tick_cli(),
        Some("done") => tasks::done(&cwd, name),
        Some("drop") => tasks::drop(&cwd, name, arguments.flag("--force")),
        Some("reset") => tasks::reset(&cwd, name),
        Some("link") => {
            let name = name.ok_or("kelpie link needs a task name and the item")?;
            tasks::link(&cwd, name, &arguments.positionals[2..].join(" "))
        }
        Some("docs") => docs::print(&arguments.positionals[1..]),
        Some("profiles") => profiles::print(arguments.flag("--json")),
        Some("hub") => hub::cli(name, &arguments.positionals[2..], arguments.value("--name")),
        Some("config") => match arguments.value("--get") {
            Some(key) => config::get(key),
            None => config::show(&cwd, arguments.value("--repo")),
        },
        Some("issues") => Ok(issues::list_in(&issues::dir(&lead::state_dir()?), arguments.flag("--all"), arguments.flag("--json"))),
        Some("issue") => issue(arguments, name),
        Some("setup") if name == Some("vault") => setup::vault(),
        Some("setup") if name == Some("codex") => setup::codex(arguments.flag("--uninstall")),
        Some("setup") if name.is_none() => setup::install(arguments.flag("--yes")),
        Some("setup") => Err("kelpie setup knows vault and codex: kelpie setup, kelpie setup vault, kelpie setup codex [--uninstall]".to_string()),
        Some("restart") => restart::now_cli(arguments.flag("--all")),
        Some("keeper") if arguments.flag("--status") => keeper::status_cli(),
        Some("keeper") if arguments.flag("--stop") => keeper::stop(),
        Some("keeper") if arguments.flag("--start") => keeper::start(),
        Some("keeper") => keeper::run(),
        Some("update") if arguments.flag("--check") => Ok(update::check(&doctor::Env::current().claude_dir)),
        Some("update") if arguments.flag("--finish") => update::finish(&doctor::Env::current().claude_dir, std::env::var_os(update_progress::ENV).map(std::path::PathBuf::from).as_deref()).map(|lines| lines.join("\n")),
        Some("update") => {
            let request = match (arguments.value("--from"), arguments.flag("--stable")) {
                (Some(_), true) => return Err("kelpie update takes --from <branch or path> or --stable, not both".to_string()),
                (Some(from), false) => release::Request::From(from.to_string()),
                (None, true) => release::Request::Stable,
                (None, false) => release::Request::Channel,
            };
            update::run(&doctor::Env::current().claude_dir, true, &request).map(|outcome| {
                let fixed = lead::state_dir().ok().and_then(|state| issue_recheck::shown(&state, messages::now()));
                update::text(&outcome, &fixed.filter(|fixed| fixed.version == outcome.summary.version).map(|fixed| fixed.bugs).unwrap_or_default())
            })
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
            issues::add_in(&dir, &caller(), &issues::Filing { kind: arguments.value("--kind"), title: arguments.value("--title"), body: &body }, env!("KELPIE_BUILT_FROM"), config::report_settings().map_or(config::ReportText::Ask, |report| report.text()), messages::now())
        }
        Some("hold") if arguments.flag("--take") => issues::take_held_in(&dir, &caller()),
        Some("hold") => issues::hold_in(&dir, &caller(), &arguments.positionals[2..].join(" "), config::propose_issues_enabled()?),
        Some("show") => issues::show_in(&dir, id()?),
        Some("close") => issues::close_in(&dir, &caller(), id()?, arguments.positional(3)),
        Some("retry") => issues::retry_in(&dir, &caller(), id()?),
        Some("reopen") => issues::reopen_in(&dir, &caller(), id()?, env!("KELPIE_BUILT_FROM")),
        Some("recheck") if arguments.flag("--run") => issue_recheck::run(id()?),
        Some("list") => Ok(issues::list_in(&dir, arguments.flag("--all"), arguments.flag("--json"))),
        Some("link") => issues::link_in(&dir, &caller(), id()?, arguments.positional(3)),
        Some("send") => {
            let opener = board::images::image_viewer(std::env::consts::OS, &|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
            issues::send_in(&dir, &caller(), id()?, opener, &issues::open_url, messages::now())
        }
        Some("report") if arguments.flag("--always") => hub::report::arm_cli(true),
        Some("report") if arguments.flag("--ask") => hub::report::arm_cli(false),
        Some("report") => hub::report::cli(id()?),
        _ => Err("kelpie issue knows add, hold <line> | --take, list, show <id>, close <id> fixed|wontfix|duplicate, reopen <id>, retry <id>, link <id> <number|url>, send <id>, report <id> and report --always | --ask".to_string()),
    }
}

// Many commits carry one version number, so the commit and its date say which code this binary is.
fn version(built: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    if built.is_empty() { format!("kelpie {version}") } else { format!("kelpie {version} ({built})") }
}

fn doctor() -> ExitCode {
    config::report_unknown_keys_quietly();
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
    signals::install_panic_hook();
    if matches!(arguments.positional(0), Some("board" | "start" | "after" | "land" | "push" | "done" | "move" | "update" | "ci" | "routine" | "routines")) {
        login_env::import();
    }
    match run(&arguments) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            let command = match (arguments.positional(0), arguments.positional(1)) {
                (Some(first @ ("routine" | "backlog")), Some(second)) => format!("{first} {second}"),
                (Some(first), _) => first.to_string(),
                (None, _) => String::new(),
            };
            let error = if sandbox::TASK_COMMANDS.contains(&command.as_str()) { sandbox::ran_sandboxed(&command, &error, sandbox::confined()) } else { error };
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
