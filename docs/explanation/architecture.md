# How kebbie fits together

This page explains kebbie's parts and how they talk, so you can run many Claude Code sessions at once without losing track of them.

herdr already holds the terminals and the agents. kebbie adds what herdr does not know: that a session works on a task, in a branch, in a repo, and that the task ends with commits on the default branch.

## One binary does the work

**The `kebbie` binary.** One Rust program starts tasks, keeps the worktree pool, draws the board, carries messages and questions, decides who may push, runs the reviewer and updates itself. Everything else calls it by name.

**The keeper.** One `kebbie keeper` process per machine runs kebbie's routines in the background, so they go on while no board is open. See [The keeper](#the-keeper).

## The herdr plugin installs and starts kebbie

herdr installs kebbie from its repo and runs `scripts/build.sh`, which downloads the release's prebuilt binary for a release tag and builds with `cargo` otherwise. The plugin gives herdr:

- its actions: `kebbie.lead`, `kebbie.focus`, `kebbie.chat`, `kebbie.board`, `kebbie.update`,
- the board and update popups,
- a startup script that links `~/.local/share/kebbie/current` to wherever herdr put the code, starts the board again in a restored lead Space, and starts the keeper.

That link gives configs, hooks and your `PATH` one stable address across updates. The plugin also keeps the lead Space at the top whenever Spaces are created or moved.

## The Claude Code plugin carries the skills

The Claude Code plugin carries kebbie's skills (worktree setup, commit rules, the reviewer's instructions, the review split) and the hook that opens the review split. You add its marketplace from the same `current` folder, so the skills Claude loads are always the same code as the binary that runs them.

## The vault and the hub are optional

**The vault plugin.** [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp) keeps notes in a vault. When it is installed and enabled, kebbie finds it and uses it: each task gets a note, edits wait until the problem is written down, and pushes are logged. Without it, kebbie keeps its own state and loses nothing it needs to run. See [The vault](vault.md).

**The hub.** A [kebbie hub](https://github.com/gronare/kebbie-hub) relays each machine's board, sealed to the account that connected it, to any phone or browser signed in with that account's passkey. It also takes kebbie's error signals and bug reports for its maintainers. The phone only reads: the hub cannot start anything on the machine. Notifications through the hub are not available in this version. See [See your board on your phone](../how-to/see-your-board-on-your-phone.md) and [What goes to the hub](trust-boundary.md#what-goes-to-the-hub).

## A task is a worktree, a Space and a Claude

kebbie keeps three things together under one task name:

- a branch in a git worktree from the repo's pool,
- a herdr Space labelled with the task's name,
- a Claude session running in that Space with the task role.

The role tells Claude which task and repo it is, which skill to run first, which rules to commit by, who its peers are, and how to report. kebbie also gives the session its own settings: hooks that collect a report at the end of each turn, and rules that keep it from pushing.

Git is the source of truth for the pool, so kebbie needs no database of tasks: a worktree on a task branch is a task. The rest of kebbie's state (reports, messages, questions, verdicts, permissions) lives in files in one private state directory.

## You talk to the lead

The lead is a Claude in the lead Space next to the board, with a role that makes it responsible for the whole picture. It starts tasks, reads their reports, checks their commits when the reviewer's rules check gives no verdict, and relays what one task needs to know from another. See [The lead and away mode](lead-and-away-mode.md).

## Tasks talk through kebbie

Tasks never type into each other's terminals. They talk through kebbie:

- `kebbie report` at the end of every turn says where the task stands and what you should do next. The board groups tasks by it.
- `kebbie ask` puts a question to you. It waits on the board until you answer, and every other message to that task is held meanwhile.
- `kebbie tell` messages another session. Each message is logged and shown on the board, and the lead gets a copy.

So you see every message on the board, and the lead sees it without the messages crowding its chat.

## The keeper

Some of kebbie's work runs on a clock rather than on a command. The keeper runs each job on its own interval and sleeps in between. A job that fails is retried later each time, up to every 15 minutes, and its error is logged once.

## Messages and prompts move within seconds

| Every | The keeper |
| --- | --- |
| 3 seconds | Delivers held messages once their receiver is free, and a new task's prompt that waited for your answer to Claude's trust prompt. |
| 3 seconds | Checks prompts kebbie typed for delivery, and tells the board when one was not taken. |
| 5 seconds | Sends the lead a prompt a task's Claude waits on in its own pane. See [Claude's own prompts in a task](lead-and-away-mode.md#claudes-own-prompts-in-a-task). |
| 5 seconds | Gives a ready task's rules check its read-only reviewer. See [The review gate](review-gate.md). |

## Pushes and queues move every 15 seconds

| Every | The keeper |
| --- | --- |
| 15 seconds | Completes a push that waits for a task's Claude to test the files the default branch changed under it, once the task reports ready with the same commits. |
| 15 seconds | Puts a staged task whose stage conflicted back on local main once its Claude reports ready, and stops counting a staged task as staged once its commits reached origin. See [`kebbie stage`](../reference/commands.md#kebbie-stage-name). |
| 15 seconds | Starts queued tasks once every task they wait for has ended as done. See [`kebbie after`](../reference/commands.md#kebbie-after-name). |
| 15 seconds, and at least once a minute | With a hub connected from its link, sends the board to the hub sealed to your account when it changes. See [See your board on your phone](../how-to/see-your-board-on-your-phone.md). |

## CI, API errors and issue rechecks every 30 seconds

| Every | The keeper |
| --- | --- |
| 30 seconds | Checks pushed tasks' CI watches. See [`kebbie ci`](../reference/commands.md#kebbie-ci---json). |
| 30 seconds | Resumes tasks an API error stopped. See [When the API stops a task](lead-and-away-mode.md#when-the-api-stops-a-task). |
| 30 seconds | After an update, rechecks each open kebbie issue filed on older code against the new commits. See [Bugs you ran into in kebbie](updates-and-restarts.md#bugs-you-ran-into-in-kebbie). |

## Restarts, landings and routines every minute

| Every | The keeper |
| --- | --- |
| minute | Restarts stale Claude sessions once idle. |
| minute | Closes tasks whose commits are on the default branch as done. See [`kebbie done --landed`](../reference/commands.md#kebbie-done---landed). |
| minute | Starts due [routines](routines.md)' runs as tasks. |
| minute | Rebases tasks that follow main when it moves. See [`kebbie follow`](../reference/commands.md#kebbie-follow-name-onoff). |
| minute | With `[report] text = "always"` armed, sends the bugs in kebbie you ran into to kebbie's maintainers. |

## Updates and reports on a slower clock

| Every | The keeper |
| --- | --- |
| 10 minutes | Reads the update check's cache, and fetches when the last fetch is 6 hours old. |
| 10 minutes | With `[report] inbox_token` set, on a maintainer's machine, files the reports the issues endpoint let through, and the error signals three installs share, as kebbie issues. See [Receive bug reports as a maintainer](../how-to/receive-bug-reports-as-a-maintainer.md). |
| hour, sent at most once a day | Sends the error signals counted since the last send to kebbie's maintainers. See [Error signals](../how-to/report-a-problem-with-kebbie.md#error-signals). |
| hour, sent at most once a UTC day | Sends the install signal, so the maintainers can count active installs. See [The install signal](../how-to/report-a-problem-with-kebbie.md#the-install-signal). |

## One keeper per machine, with no terminal

The keeper logs to `keeper.log` in the state directory. A lock file there keeps it to one per machine. The kernel releases the lock when the keeper exits, so a crashed one never blocks the next.

herdr's startup hook starts the keeper in its own session, so it outlives the hook and any pane. See [Updates and stale sessions](updates-and-restarts.md#the-keeper) for how it follows an update.

## The board reads what the keeper wrote

While `[keeper] enabled` is on, the keeper alone runs these jobs. The board only reads what the keeper wrote: the stale count from `keeper.json`, the update notice from the update check's cache, and messages and questions from the message log. Each board starts a keeper when none runs, and checks again every minute.

With `[keeper] enabled = false` no keeper runs, and an open board runs the jobs itself.

## The keeper keeps the board's rules

The keeper calls the same functions the board did, with the same rules:

- It never types into a session whose input box holds a draft.
- It never restarts the pane you are in or a session mid-turn.
- It asks about a session with background work once, as a question on the board. Your answer reaches it through the same file.

When herdr was started without them, it imports the login shell's SSH agent and `PATH`, as the board does.

## Claude Code sits behind one seam

Everything kebbie knows about Claude Code sits behind the `Agent` type in `src/agent`:

- the settings file and start flags a task's Claude gets,
- the trust and bypass dialogs it answers or leaves to you,
- how it is ended, where its conversation is kept and which messages in it are yours,
- how a running session is resumed with its own flags,
- the read-only reviewer's command line.

Claude is the one agent behind it today. The rest of kebbie, from the pool and the worktrees to the push block, the messages, the questions, the board and the routines, asks the seam and knows nothing of the CLI.

## A task keeps its agent across restarts

A task's [profile](../reference/configuration.md#profiles) names its agent, and the task's session record keeps it, so a restart reaches the same one.

Two things are Claude's alone and stay outside the seam: reading a Claude's screen for its own menus, its input box and its API errors, and the staleness checks on Claude Code's binary, plugins and MCP config.

## Configuration lives in your home only

kebbie reads its settings only from your home directory: one machine config, one optional file per repo, and one file per profile. It never reads a file inside a repo.

A repo's collaborators may use kebbie or not, and cloning a repo can never change how kebbie behaves on your machine. A team that wants a shared setup keeps it in a team repo and links the files in. See [Share a team setup](../how-to/share-a-team-setup.md).

## Related

- [Trunk-based by design](trunk-based.md)
- [The worktree pool](worktree-pool.md)
- [The trust boundary](trust-boundary.md)
