# Commands

`kelpie --help` prints this list too. Without a name, `land`, `review`, `push`, `done`, `drop` and `reset` act on the task whose worktree you are in. `-C <dir>` runs any command as if started in `<dir>`.

## Tasks

| Command | What it does |
| --- | --- |
| `kelpie start <name> [prompt]` | Start a task in a pooled worktree, in its own Space, with Claude. `--repo <name>` (repeatable), `--from <ref>`, `--backlog <item>`, `--mode <mode>`, `--no-focus`. See [Tasks](tasks.md#start). |
| `kelpie move <name> [file ...]` | Move work under way in the main checkout into a task. `--pane <id>`, `--no-focus`. |
| `kelpie land [name]` | Rebase a task onto the default branch and check it is clean. On a conflict, its Claude resolves the rebase. |
| `kelpie tidy <name> [note]` | Ask the task's Claude to reshape its commits by the rules skill. |
| `kelpie review [name]` | Send the task to the read-only reviewer. `--withdraw` takes it out of review (you only). See [Review](review.md#the-reviewer-gate). |
| `kelpie approve <name>` | The lead's word that a task's commits hold to the rules skill. |
| `kelpie push [name]` | After land: push the task to the default branch and close it as done. `--through <sha>` pushes up to that commit and keeps the task open. |
| `kelpie done [name]` | After pushing: close the task and free its worktree. |
| `kelpie drop [name] [--force]` | Throw a task away. `--force` discards uncommitted work and unpushed commits. |
| `kelpie reset [name]` | Stop a task for good: discard its work, free its worktree, close its Space, return its backlog item. |
| `kelpie link <name> <item>` | Tie a running task to a backlog item or project note. |

## Seeing what is going on

| Command | What it does |
| --- | --- |
| `kelpie board [--overlay]` | The board. `--overlay` is the popup form, which `esc` closes. See [The board](board.md). |
| `kelpie status [--json]` | Every task and Claude session, grouped by what it needs from you. |
| `kelpie list [--json]` | The pool's worktrees and their tasks. |
| `kelpie areas` | The vault's areas with their repo. See [Areas](areas.md). |
| `kelpie peers` | Who you are and the other task sessions, with what each is doing. |
| `kelpie messages [--print]` | The sessions' messages, live. `--print` prints the last 30. |
| `kelpie backlog` | The backlog the board shows. |
| `kelpie backlog add [--priority <p>] "<Title> — <summary> (<area>)"` | Add an item to the local backlog. |

## The lead

| Command | What it does |
| --- | --- |
| `kelpie lead` | Go to the lead Space (creating it), or back. `--restart` restarts an idle lead on its current role, resuming its conversation. `--first` only moves the lead Space to the top. |
| `kelpie focus` | Move focus between the board and the lead's chat. |
| `kelpie away [on\|off]` | Away mode, your switch. See [The lead](lead.md#away-mode). |
| `kelpie decide <task> <answer>` | The lead's own answer to a question about how, only while you are away. |
| `kelpie defer <task>` | The lead keeps a task's question for you while you are away. |
| `kelpie notify <text>` | Run `notify.command` with this text, as a `summary` event. |

## Run by a task's Claude

| Command | What it does |
| --- | --- |
| `kelpie report --state <state> --next <step> <summary>` | Where the task stands (`needs-you`, `ready`, `working`, `blocked`) and what you should do next. With `ready`, `--card <file>` is the sign-off card. |
| `kelpie ask --option "<label>: <meaning>" … --recommend <n> <question>` | Put a question to you, with two to four options. `--plan <file>` or `--plan vault:<note>` asks you to approve a plan. |
| `kelpie answered <task>` | A task's question was settled elsewhere; it leaves the board. |
| `kelpie tell <to> <text>` | Message another session: a task, `task@repo`, or `lead`. |
| `kelpie hook` | The hooks kelpie gives a task's Claude. Reads the hook's JSON on stdin. |
| `kelpie statusline` | Claude Code's status line command. Keeps your plan usage for the board and prints nothing. |

## Setup and maintenance

| Command | What it does |
| --- | --- |
| `kelpie config [--repo <path>]` | The config layers found and every key with its value, layer and meaning. `--get <key>` prints `worktree.server_name` or `worktree.worker_name`, for scripts. See [Configuration](configuration.md). |
| `kelpie doctor` | Check that this machine has what kelpie needs, with the fix for each gap. See [Troubleshooting](troubleshooting.md). |
| `kelpie setup vault` | Install the optional vault plugin, running each command only on your `y`. |
| `kelpie update` | Update everything to this machine's channel. `--check`, `--from <branch or path>`, `--stable`. See [Updating](updating.md). |
| `kelpie restart [--all]` | Restart stale idle Claude sessions with `claude --resume`. `--all` restarts every idle one. |
