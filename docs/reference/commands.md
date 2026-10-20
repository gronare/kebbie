# Commands

`kelpie --help` prints the same list.

## Global

| Option | Meaning |
| --- | --- |
| `-C <dir>` | Run as if started in `<dir>`. Works with every command. |
| `-h`, `--help`, `help` | Print the usage. |

Without a name, `land`, `review`, `push`, `done`, `drop` and `reset` act on the task whose worktree you are in.

## Tasks

### `kelpie start <name> [prompt]`

Start a task in a pooled worktree, in its own herdr Space, with Claude. Running it on a task that already runs shows it; on a task whose Space or Claude is gone, it restarts them in the same permission mode and resumes the last conversation without sending the prompt again.

| Option | Meaning |
| --- | --- |
| `--repo <name>` | Start in that repo instead of the current one. Repeat it for one task across repos: one half per repo, the first focused. See [Finding repos](#finding-repos). |
| `--from <ref>` | Branch off this ref instead of the remote's default branch. |
| `--backlog <item>` | Tie the task to a backlog item, as `kelpie link` does. |
| `--mode <mode>` | Claude's permission mode for the task. |
| `--no-focus` | Stay where you are. |

### `kelpie move <name> [file ...]`

Move work under way in the main checkout into a task: branch `<name>` if it exists, else the unpushed commits, plus the uncommitted files named.

| Option | Meaning |
| --- | --- |
| `--pane <id>` | Move that herdr pane along. |
| `--no-focus` | Stay where you are. |

### `kelpie land [name]`

Rebase the task onto the default branch and check it has no uncommitted changes. Never pushes. On a conflict it aborts its own rebase, leaves the worktree as it was, and hands the rebase to the task's Claude.

### `kelpie push [name]`

After `land`: push the task's commits to the default branch, then close it as `done`. Only you may run it, or the lead for a task you gave push permission. The lead is told what landed.

| Option | Meaning |
| --- | --- |
| `--through <sha>` | Push the commits up to and including `<sha>` and keep the task open. The rest of the branch is rebased on top of what was pushed. |

### `kelpie done [name]`

Close a task once every change on its branch is on the default branch, pushed as is, cherry-picked or squashed: closes its Space, deletes the branch, frees the worktree, and fast-forwards a clean main checkout. Otherwise it names the missing commits.

### `kelpie drop [name] [--force]`

Throw a task away. Refuses while it has uncommitted changes or unpushed commits, unless `--force`.

### `kelpie reset [name]`

Stop a task for good: discard its changes and commits, free its worktree, close its Space, tell the lead what was thrown away, and put the backlog item it was started from back in the Backlog column.

### `kelpie link <name> <item>`

Tie a running task to a backlog item or project note: a vault path, a Linear id, or `local:<Title>`. `done` tells the lead to close the item; `reset` puts it back.

### `kelpie tidy <name> [note]`

Ask the task's Claude to reshape its commits by the rules skill (`task.rules_skill`), keeping the final tree identical. The note is passed along, for example `"squash into one"`.

### `kelpie review [name]`

Send the task to the reviewer: a separate, read-only Claude reviews its commits in the background. The task waits under To review, and its push with it, until a `verdict: ok` on its current code.

| Option | Meaning |
| --- | --- |
| `--withdraw` | Take the task out of review: the reviewer stops, its verdict goes, push is offered again, the lead is told. Only you may run it. |

### `kelpie approve <name>`

The lead's word that a task's commits hold to the rules skill. Until a commit changes, the board shows the task ready to push. Tasks cannot run it.

## Seeing what is going on

| Command | Output |
| --- | --- |
| `kelpie board [--overlay]` | The board. `--overlay` is the popup form, which `esc` closes. See [Board](board.md). |
| `kelpie status [--json]` | Every task and Claude session, grouped by what it needs from you. `--json` gives `away_since`, `items` and `checkouts`. |
| `kelpie list [--json]` | The pool's worktrees: each one's task or `free`, uncommitted changes, commits ahead and behind, and the first line of its setup note. |
| `kelpie areas` | The vault's areas with their repo: main against origin, the last release, notes and actions. Needs the vault. |
| `kelpie peers` | Who you are and the other task sessions, with what each is doing. |
| `kelpie messages` | The sessions' messages, live. `--print` prints the last 30. |
| `kelpie backlog` | The backlog the board shows. |
| `kelpie backlog add [--priority <p>] "<Title> — <summary> (<area>)"` | Append an item to the local backlog file. See [Files](files.md#the-local-backlog-file). |

## The lead and away mode

| Command | What it does |
| --- | --- |
| `kelpie lead` | Go to the lead Space, creating it, or back to where you were. |
| `kelpie lead --restart` | Restart an idle lead's Claude on its current role and settings, resuming its conversation. Refuses while the lead works or waits on a question. |
| `kelpie lead --first` | Only move the lead Space to the top of herdr's Spaces. |
| `kelpie focus` | In the lead Space, move focus between the board and the lead's chat. From any other Space, go to the lead's chat. |
| `kelpie away [on\|off]` | Away mode. Yours only. |
| `kelpie decide <task> <answer>` | The lead's own answer to a question about how, only while you are away. Closes the question and records it. |
| `kelpie defer <task>` | The lead keeps a task's question for you while you are away. |
| `kelpie notify <text>` | Run `notify.command` with this text as a `summary` event. You or the lead only. |

## Run by a task's Claude

| Command | What it does |
| --- | --- |
| `kelpie report --state <state> --next <step> <summary>` | Where the task stands and what you should do next. `<state>` is `needs-you`, `ready`, `working` or `blocked`. With `ready`, `--card <file>` is the sign-off card the push window shows. |
| `kelpie ask --option "<label>: <meaning>" … --recommend <n> <question>` | Put a question to you: two to four options, one recommended. `--plan <file>` or `--plan vault:<note>` asks you to approve a plan instead. |
| `kelpie answered <task>` | The task's question was settled elsewhere; it leaves the board. |
| `kelpie tell <to> <text>` | Message another session: a task (every half of it), `task@repo`, or `lead`. Held while the receiver waits for an answer, and logged. |

## Setup and maintenance

| Command | What it does |
| --- | --- |
| `kelpie config [--repo <path>]` | The config layers found and every key with its value in effect, the layer it comes from and what it does. See [Configuration](configuration.md). |
| `kelpie config --get <key>` | Print one machine-wide value for scripts: `worktree.server_name` or `worktree.worker_name`. |
| `kelpie doctor` | Check this machine, with the fix for each gap. Exits non-zero on a `fail`. See [Requirements](requirements.md#what-kelpie-doctor-checks). |
| `kelpie setup vault` | Install the optional vault plugin in Claude Code, showing each `claude plugin` command and running it only on your `y`. |
| `kelpie update` | Update everything to this machine's channel and restart idle Claude sessions. |
| `kelpie update --check` | Fetch and show the installed versions against the channel's latest. Changes nothing else. |
| `kelpie update --from <branch or path>` | Install that branch on origin, or that checkout's commits, until `--stable`. |
| `kelpie update --stable` | Return to the channel. |
| `kelpie restart` | Restart each stale Claude session that is idle, not the pane you are in, and has no background work. Works with `restart.auto = false`. |
| `kelpie restart --all` | Restart every idle session, stale or not. |

`--from` and `--stable` cannot be combined.

## Used by kelpie itself

These run from hooks, the herdr plugin and kelpie's own processes. You do not run them.

| Command | Run by |
| --- | --- |
| `kelpie hook` | The hooks kelpie gives a task's Claude. Reads the hook's JSON on stdin. |
| `kelpie statusline` | Claude Code's `statusLine` command. Keeps the plan's usage for the board and prints nothing. |
| `kelpie backlog save` | The lead, after refreshing the backlog. Reads a JSON array on stdin. |
| `kelpie messages --unread` | The lead's prompt hook. |
| `kelpie review --run` | The reviewer process `kelpie review` starts. |
| `kelpie update --finish` | The newly built binary, at the end of `kelpie update`. |
| `kelpie lead --renew-board` | `kelpie update`, to restart the board on the new binary. |

## Finding repos

`--repo <name>` takes a folder name, never a path. kelpie looks for a checkout of that name in each folder of `KELPIE_REPO_DIRS` (default `~/projects:~/personal:~/src:~/code`), in it or one level down. When there is none, it clones the repo from an owner found among your other checkouts' remotes.
