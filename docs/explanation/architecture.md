# How kebbie fits together

kebbie lets you run many Claude Code sessions at once without losing track of them. herdr already holds the terminals and the agents. kebbie adds what herdr does not know about: that a session is working on a task, in a branch, in a repo, and that the task ends with commits on the default branch.

## Six parts

**The `kebbie` binary.** One Rust program does all the work: it starts tasks, keeps the worktree pool, draws the board, carries messages and questions, decides who may push, runs the reviewer and updates itself. Everything else calls it by name.

**The keeper.** One `kebbie keeper` process per machine runs kebbie's routines in the background, so they go on while no board is open. See [The keeper](#the-keeper).

**The herdr plugin.** herdr installs kebbie from its repo and runs `scripts/build.sh`, which downloads the release's prebuilt binary for a release tag and builds with `cargo` otherwise. The plugin gives herdr its actions (`kebbie.lead`, `kebbie.focus`, `kebbie.chat`, `kebbie.board`, `kebbie.update`), the board and update popups, and a startup script that links `~/.local/share/kebbie/current` to wherever herdr put the code, starts the board again in a restored lead Space, and starts the keeper. That link gives configs, hooks and your `PATH` one stable address across updates. The plugin also keeps the lead Space at the top whenever Spaces are created or moved.

**The Claude Code plugin.** It carries kebbie's skills (worktree setup, commit rules, the reviewer's instructions, the review split) and the hook that opens the review split. You add its marketplace from the same `current` folder, so the skills Claude loads are always the same code as the binary that runs them.

**The vault plugin, optional.** [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp) keeps notes in a vault. When it is installed and enabled, kebbie finds it and uses it: each task gets a note, edits wait until the problem is written down, and pushes are logged. Without it, kebbie keeps its own state and loses nothing it needs to run. See [The vault](vault.md).

**The hub, optional.** A [kebbie hub](https://github.com/gronare/kebbie-hub) relays each machine's board, sealed to the account that connected it, to any phone or browser signed in with that account's passkey, and takes kebbie's error signals and bug reports for its maintainers. The phone only reads: the hub cannot start anything on the machine. Notifications through the hub are not available in this version. See [See your board on your phone](../how-to/see-your-board-on-your-phone.md) and [What goes to the hub](trust-boundary.md#what-goes-to-the-hub).

## What a task is

A task is three things that kebbie keeps together under one name:

- a branch in a git worktree from the repo's pool,
- a herdr Space labelled with the task's name,
- a Claude session running in that Space with the task role.

The role tells Claude which task and repo it is, which skill to run first, which rules to commit by, who its peers are, and how to report. kebbie also gives the session its own settings: hooks that collect a report at the end of each turn, and rules that keep it from pushing.

Because git is the source of truth for the pool, kebbie needs no database of tasks. A worktree on a task branch is a task. The rest of kebbie's state (reports, messages, questions, verdicts, permissions) lives in files in one private state directory.

## Who talks to whom

You talk to the lead. The lead is a Claude in the lead Space next to the board, with a role that makes it responsible for the whole picture. It starts tasks, reads their reports, checks their commits when the reviewer's rules check gives no verdict, and relays what one task needs to know from another. See [The lead and away mode](lead-and-away-mode.md).

Tasks talk through kebbie, never directly into each other's terminals:

- `kebbie report` at the end of every turn says where the task stands and what you should do next. The board groups tasks by it.
- `kebbie ask` puts a question to you. It waits on the board until you answer, and every other message to that task is held meanwhile.
- `kebbie tell` messages another session. Each message is logged and shown on the board, and the lead gets a copy.

This keeps the traffic visible. You can see every message on the board, and the lead sees it without the messages crowding its chat.

## The keeper

Some of kebbie's work runs on a clock rather than on a command:

- held messages are delivered once their receiver is free, every 3 seconds, and so is a new task's prompt that waited for your answer to Claude's trust prompt,
- prompts kebbie typed are checked for delivery, every 3 seconds, and the board is told when one was not taken,
- a prompt a task's Claude waits on in its own pane is sent to the lead, every 5 seconds (see [Claude's own prompts in a task](lead-and-away-mode.md#claudes-own-prompts-in-a-task)),
- stale Claude sessions are restarted once idle, every minute,
- a push that waits for a task's Claude to test the files the default branch changed under it completes once the task reports ready with the same commits, every 15 seconds,
- a staged task whose stage conflicted goes back on local main once its Claude reports ready, and a staged task whose commits reached origin stops counting as staged, every 15 seconds (see [`kebbie stage`](../reference/commands.md#kebbie-stage-name)),
- pushed tasks' CI watches are checked, every 30 seconds (see [`kebbie ci`](../reference/commands.md#kebbie-ci---json)),
- tasks whose commits are on the default branch are closed as done, every minute (see [`kebbie done --landed`](../reference/commands.md#kebbie-done---landed)),
- a ready task's rules check gets its read-only reviewer, checked every 5 seconds (see [The review gate](review-gate.md)),
- tasks an API error stopped are resumed, every 30 seconds (see [When the API stops a task](lead-and-away-mode.md#when-the-api-stops-a-task)),
- the update check reads its cache every 10 minutes and fetches when the last fetch is 6 hours old,
- after an update, each open kebbie issue filed on older code is rechecked against the new commits, checked every 30 seconds (see [Bugs you ran into in kebbie](updates-and-restarts.md#bugs-you-ran-into-in-kebbie)),
- due [routines](routines.md) start their runs as tasks, every minute,
- tasks that follow main are rebased when it moves, every minute (see [`kebbie follow`](../reference/commands.md#kebbie-follow-name-onoff)),
- queued tasks start once every task they wait for has ended as done, every 15 seconds (see [`kebbie after`](../reference/commands.md#kebbie-after-name)),
- with a hub connected from its link, the board goes to the hub sealed to your account when it changes, checked every 15 seconds, and at least once a minute (see [See your board on your phone](../how-to/see-your-board-on-your-phone.md)),
- error signals counted since the last send go to kebbie's maintainers, checked every hour and sent at most once a day (see [Error signals](../how-to/report-a-problem-with-kebbie.md#error-signals)),
- with `[report] text = "always"` armed, the bugs in kebbie you ran into go to kebbie's maintainers, every minute,
- with `[report] inbox_token` set, on the machine of whoever runs the hub that takes the reports, the reports the hub let through are filed as kebbie issues, every 10 minutes (see [Receiving reports](../how-to/report-a-problem-with-kebbie.md#receiving-reports)).

The keeper runs all of them, each on its own interval, and sleeps in between. It has no terminal and logs to `keeper.log` in the state directory. A lock file in the state directory keeps it to one per machine; the kernel releases the lock when the keeper exits, so a crashed one never blocks the next. A routine that fails is retried later each time, up to every 15 minutes, and its error is logged once.

The keeper alone runs the routines while `[keeper] enabled` is on. The board then only reads what the keeper wrote: the stale count from `keeper.json`, the update notice from the update check's cache, and messages and questions from the message log. Each board starts a keeper when none runs, and checks again every minute. With `[keeper] enabled = false` no keeper runs, and an open board runs the routines itself.

The keeper calls the same functions the board did, with the same rules: it never types into a session whose input box holds a draft, never restarts the pane you are in or a session mid-turn, and asks about a session with background work once, as a question on the board. Your answer reaches it through the same file. It imports the login shell's SSH agent and `PATH`, as the board does, when herdr was started without them.

herdr's startup hook starts the keeper in its own session, so it outlives the hook and any pane. See [Updates and stale sessions](updates-and-restarts.md#the-keeper) for how it follows an update.

## The agent seam

Everything kebbie knows about Claude Code sits behind one seam, the `Agent` type in `src/agent`: the settings file and start flags a task's Claude gets, the trust and bypass dialogs it answers or leaves to you, how it is ended, where its conversation is kept and which messages in it are yours, how a running session is resumed with its own flags, and the read-only reviewer's command line. Claude is the one agent behind it today. The rest of kebbie, from the pool and the worktrees to the push block, the messages, the questions, the board and the routines, asks the seam and knows nothing of the CLI. A task's [profile](../reference/configuration.md#profiles) names its agent, and the task's session record keeps it, so a restart reaches the same one. Reading a Claude's screen for its own menus, its input box and its API errors, and the staleness checks on Claude Code's binary, plugins and MCP config, are Claude's alone and stay where they are.

## Where configuration lives

kebbie reads its settings only from your home directory: one machine config, one optional file per repo, and one file per profile. It never reads a file inside a repo. A repo's collaborators may use kebbie or not, and cloning a repo can never change how kebbie behaves on your machine. A team that wants a shared setup keeps it in a team repo and links the files in. See [Share a team setup](../how-to/share-a-team-setup.md).

## Related

- [Trunk-based by design](trunk-based.md)
- [The worktree pool](worktree-pool.md)
- [The trust boundary](trust-boundary.md)
