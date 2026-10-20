# How kelpie fits together

kelpie lets you run many Claude Code sessions at once without losing track of them. herdr already holds the terminals and the agents. kelpie adds what herdr does not know about: that a session is working on a task, in a branch, in a repo, and that the task ends with commits on the default branch.

## Five parts

**The `kelpie` binary.** One Rust program does all the work: it starts tasks, keeps the worktree pool, draws the board, carries messages and questions, decides who may push, runs the reviewer and updates itself. Everything else calls it by name.

**The keeper.** One `kelpie keeper` process per machine runs kelpie's routines in the background, so they go on while no board is open. See [The keeper](#the-keeper).

**The herdr plugin.** herdr installs kelpie from its repo and builds the binary with `cargo`. The plugin gives herdr its actions (`kelpie.lead`, `kelpie.focus`, `kelpie.chat`, `kelpie.board`, `kelpie.update`), the board and update popups, and a startup script that links `~/.local/share/kelpie/current` to wherever herdr put the code and starts the keeper. That link gives configs, hooks and your `PATH` one stable address across updates. The plugin also keeps the lead Space at the top whenever Spaces are created or moved.

**The Claude Code plugin.** It carries kelpie's skills (worktree setup, commit rules, the reviewer's instructions, the review split) and the hook that opens the review split. You add its marketplace from the same `current` folder, so the skills Claude loads are always the same code as the binary that runs them.

**The vault plugin, optional.** [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp) keeps notes in a vault. When it is installed and enabled, kelpie finds it and uses it: each task gets a note, edits wait until the problem is written down, and pushes are logged. Without it, kelpie keeps its own state and loses nothing it needs to run. See [The vault](vault.md).

## What a task is

A task is three things that kelpie keeps together under one name:

- a branch in a git worktree from the repo's pool,
- a herdr Space labelled with the task's name,
- a Claude session running in that Space with the task role.

The role tells Claude which task and repo it is, which skill to run first, which rules to commit by, who its peers are, and how to report. kelpie also gives the session its own settings: hooks that collect a report at the end of each turn, and rules that keep it from pushing.

Because git is the source of truth for the pool, kelpie needs no database of tasks. A worktree on a task branch is a task. The rest of kelpie's state (reports, messages, questions, verdicts, permissions) lives in files in one private state directory.

## Who talks to whom

You talk to the lead. The lead is a Claude in the lead Space next to the board, with a role that makes it responsible for the whole picture. It starts tasks, reads their reports, checks their commits and relays what one task needs to know from another. See [The lead and away mode](lead-and-away-mode.md).

Tasks talk through kelpie, never directly into each other's terminals:

- `kelpie report` at the end of every turn says where the task stands and what you should do next. The board groups tasks by it.
- `kelpie ask` puts a question to you. It waits on the board until you answer, and every other message to that task is held meanwhile.
- `kelpie tell` messages another session. Each message is logged and shown on the board, and the lead gets a copy.

This keeps the traffic visible. You can see every message on the board, and the lead sees it without the messages crowding its chat.

## The keeper

Some of kelpie's work runs on a clock rather than on a command:

- held messages are delivered once their receiver is free, every 3 seconds,
- prompts kelpie typed are checked for delivery, every 3 seconds, and the board is told when one was not taken,
- stale Claude sessions are restarted once idle, every minute,
- tasks an API error stopped are resumed, every 30 seconds (see [When the API stops a task](lead-and-away-mode.md#when-the-api-stops-a-task)),
- the update check reads its cache every 10 minutes and fetches when the last fetch is 6 hours old.

The keeper runs all of them, each on its own interval, and sleeps in between. It has no terminal and logs to `keeper.log` in the state directory. A lock file in the state directory keeps it to one per machine; the kernel releases the lock when the keeper exits, so a crashed one never blocks the next. A routine that fails is retried later each time, up to every 15 minutes, and its error is logged once.

The keeper alone runs the routines while `[keeper] enabled` is on. The board then only reads what the keeper wrote: the stale count from `keeper.json`, the update notice from the update check's cache, and messages and questions from the message log. Each board starts a keeper when none runs, and checks again every minute. With `[keeper] enabled = false` no keeper runs, and an open board runs the routines itself.

The keeper calls the same functions the board did, with the same rules: it never types into a session whose input box holds a draft, never restarts the pane you are in or a session mid-turn, and asks about a session with background work once, as a question on the board. Your answer reaches it through the same file. It imports the login shell's SSH agent and `PATH`, as the board does, when herdr was started without them.

herdr's startup hook starts the keeper in its own session, so it outlives the hook and any pane. See [Updates and stale sessions](updates-and-restarts.md#the-keeper) for how it follows an update.

## Where configuration lives

kelpie reads its settings only from your home directory: one machine config and one optional file per repo. It never reads a file inside a repo. A repo's collaborators may use kelpie or not, and cloning a repo can never change how kelpie behaves on your machine. A team that wants a shared setup keeps it in a team repo and links the files in. See [Share a team setup](../how-to/share-a-team-setup.md).

## Related

- [Trunk-based by design](trunk-based.md)
- [The worktree pool](worktree-pool.md)
- [The trust boundary](trust-boundary.md)
