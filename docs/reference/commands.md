# Commands

`kelpie --help` prints the same list.

## Global

| Option | Meaning |
| --- | --- |
| `-C <dir>` | Run as if started in `<dir>`. Works with every command. |
| `-h`, `--help`, `help` | Print the usage. |
| `--version`, `version` | Print the version and the commit and date the binary was built from, such as `kelpie 0.24.0 (db8963f 2026-10-21)`. Many commits share a version number, so the commit says whether a fix is in. A build outside git prints the version alone. |

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

Rebase the task onto the default branch without pushing. Uncommitted changes to tracked files stop it; untracked files do not. On a conflict it aborts its own rebase, leaves the worktree as it was, and hands the rebase to the task's Claude. `push` rebases by itself, so `land` is never required. When git refuses to start the rebase, kelpie shows git's reason, and names any files hidden from `git status` by skip-worktree or assume-unchanged that block it.

### `kelpie push [name]`

Fetch, rebase the task onto the default branch, push its commits there, then close it as `done`. Only you may run it, or the lead for a task you gave push permission. The lead is told what landed. Uncommitted changes to tracked files stop it; untracked files do not.

How far the task is behind the default branch does not matter. The rebase decides what happens next:

| Rebase | What happens |
| --- | --- |
| Clean, and the new upstream commits changed none of the task's files | kelpie pushes. |
| Clean, but the new upstream commits changed files the task changes too | Nothing is pushed yet. kelpie hands the task to its Claude with the files and the commits that touched them, to run their tests and report ready. When it reports ready with the same commits, kelpie pushes for whoever asked: you, or the lead under its permission, with the same checks. When its commits changed, the lead checks them again and the push is asked for again. |
| Conflict | kelpie aborts the rebase and hands it to the task's Claude, which resolves it, asks you when the conflict is a choice between two behaviours, and reports ready. The resolved commits need the lead's check before the next push. |

When origin is on GitHub and `gh` is logged in, the task stays open under CI running instead, and closes as `done` once every workflow run for the pushed commit is green. See `kelpie ci`. Otherwise the push result ends with `CI not watched:` and the reason.

| Option | Meaning |
| --- | --- |
| `--through <sha>` | Push the commits up to and including `<sha>` and keep the task open. The rest of the branch is rebased on top of what was pushed. |

### `kelpie ci [--json]`

The CI watches, one per pushed task: its sha, the GitHub repo, each workflow run with its status and address, and the last check's error. `--json` gives the watch records.

A watch runs while a board is open: every 30 seconds the board asks `gh run list --commit <sha>` for each running watch, and asks nothing when none runs.

| State | What happens |
| --- | --- |
| No runs yet | kelpie waits up to 3 minutes for runs to appear, then treats the commit as green. |
| Running | The task waits under CI running. |
| Every run passed, skipped or neutral | After at least a minute since the push, the task closes as `done`: its note is closed when the vault is on, and the lead hears what landed. |
| A run failed | The task moves to Needs you with `CI failed: <workflow> · <job>`. Its Claude gets the job's name and the last 60 lines of its log, and the lead gets a line. The watch stops. |

A task that fixes the failure pushes again as usual, and the new push is watched. `done` closes a task whatever CI says. A push with `--through` is watched too and leaves the task where it is.

### `kelpie ci tick`

One round of checks for every running watch, as the board runs it, and what came of it.

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

### `kelpie evidence [name] [--json]`

The test runs kelpie recorded for the task, newest first: each one's result, the command, when it ran, how long it took, its exit code, and the commit it ran on, marked `✓` when it passed on the code the branch holds now, `○` when the code changed since and `✗` when it failed. `--json` gives each run's fields with `shown` and `current`.

kelpie records a run from the hooks it gives the task's Claude, after every Bash command that starts with a test command (after any `cd … &&` and `VAR=value`): the built-in ones (`cargo test`, `cargo nextest`, `bin/rails test`, `bundle exec rspec`, `rspec`, `pytest`, `uv run pytest`, `npm test`, `pnpm test`, `yarn test`, `go test`, `mix test` and a few spellings of them) and the repo's own in [`task.test_commands`](configuration.md#keys). It keeps the last 20 per task. A command that exits non-zero is a failed run; a result line that counts failures makes it failed too, since a pipe such as `| tail` hides the exit code. A run with uncommitted changes counts for the commit that later holds exactly those changes. Background runs and interrupted ones are not recorded.

### `kelpie approve <name>`

The lead's word that a task's commits hold to the rules skill. Until a commit changes, the board shows the task ready to push. Tasks cannot run it.

## Seeing what is going on

| Command | Output |
| --- | --- |
| `kelpie board [--overlay]` | The board. `--overlay` is the popup form, which `esc` closes. See [Board](board.md). |
| `kelpie status [--json]` | Every task and Claude session, grouped by what it needs from you. `--json` gives `away_since`, `items` and `checkouts`; a task's `push_waits` is `user` or `lead` while a push it asked for waits for the task's tests. |
| `kelpie list [--json]` | The pool's worktrees: each one's task or `free`, uncommitted changes, commits ahead and behind, and the first line of its setup note. |
| `kelpie areas` | The vault's areas with their repo: main against origin, the last release, notes and actions. Needs the vault. |
| `kelpie peers` | Who you are and the other task sessions, with what each is doing. |
| `kelpie messages` | The sessions' messages, live. `--print` prints the last 30. |
| `kelpie backlog` | The backlog the board shows. |
| `kelpie backlog add [--priority <p>] "<Title> — <summary> (<area>)"` | Append an item to the local backlog file. See [Files](files.md#the-local-backlog-file). |

## Routines

See [Routines](../explanation/routines.md) for what a run does, and [`[[routines]]`](configuration.md#routines) for the keys.

| Command | What it does |
| --- | --- |
| `kelpie routines [--json]` | Every routine from config.toml and the repo configs: its schedule, repo, mode, next run, and its last run's outcome (`ok`, `findings`, `failed` or `running`) with its summary. A routine that is not valid is listed as ignored, with the reason. `--json` gives `routines` and `errors`. |
| `kelpie routines tick` | One round of the scheduler, as the keeper runs it every minute: a run whose task closed without a word is marked failed, and each due routine without an open run starts one. |
| `kelpie routine add --title <name> --repo <repo> --at <time> \| --every <interval> [--mode <mode>] [--push] [--paused] [--id <id>] <prompt>` | Add a routine to config.toml, the same table the board's Routines page adds with `n`. `--repo` is the repo's folder name or path, `--at` takes `07:00` or `mon 07:00`, `--every` takes `30m`, `6h` or `1d`, `--mode` is `report` (the default), `plan` or `fix`. The id comes from the title, with `-2` added when it is taken, unless `--id` names one. `--push` lets the lead push a fix routine's fix; `--paused` adds it with `enabled = false`. Refused when the id is taken or config.toml does not parse. |
| `kelpie routine run <id>` | Start a run now, by the same rules: never while the last run's task is open. Also starts a routine with `enabled = false`. |

## The lead and away mode

| Command | What it does |
| --- | --- |
| `kelpie lead` | Go to the lead Space, creating it, or back to where you were. |
| `kelpie lead --restart` | Restart an idle lead's Claude on its current role and settings, resuming its conversation. Refuses while the lead works or waits on a question. |
| `kelpie lead --first` | Only move the lead Space to the top of herdr's Spaces. |
| `kelpie lead --toggle-chat` | Hide the lead's chat so the board takes the whole width, or show it again. The chat keeps running. Takes you to the lead Space. herdr's `kelpie.chat` action runs it. |
| `kelpie focus` | In the lead Space, move focus between the board and the lead's chat. From any other Space, go to the lead's chat. A hidden chat is shown first. |
| `kelpie away [on\|off]` | Away mode. Yours only. |
| `kelpie decide <task> <answer>` | The lead's own answer to a question about how, only while you are away. Closes the question and records it. |
| `kelpie defer <task>` | The lead keeps a task's question for you while you are away. |
| `kelpie notify <text>` | Run `notify.command` with this text as a `summary` event. You or the lead only. |

## Run by a task's Claude

| Command | What it does |
| --- | --- |
| `kelpie report --state <state> --next <step> <summary>` | Where the task stands and what you should do next. `<state>` is `needs-you`, `ready`, `working` or `blocked`. With `ready`, `--card <file>` is the sign-off card the push window shows, with an Also noticed list when the task found things outside its task. A routine's task also lists its findings on a card with `needs-you`. |
| `kelpie routine ok <summary>` | A routine's task, when all is well: records the run as ok with the summary and closes the task as `drop` does. Refused outside a routine's task, from anyone but that task's Claude, and while the task has changes or commits. |
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
| `kelpie restart` | Restart each stale Claude session that is idle, not the pane you are in, and has no background work. A task's Claude gets its role and settings written anew. Works with `restart.auto = false`. |
| `kelpie restart --all` | Restart every idle session, stale or not. |
| `kelpie keeper --status` | Whether the keeper runs, with its pid, binary, uptime, log and each of its routines' interval, last run and last error. |
| `kelpie keeper --start` | Start the keeper in the background unless one runs. Says `off` while `keeper.enabled = false`. |
| `kelpie keeper --stop` | Stop the keeper. It finishes the routine under way first. The board or herdr's next startup starts it again; `keeper.enabled = false` keeps it off. |

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
| `kelpie update --finish` | The newly built binary, at the end of `kelpie update`. It stops the keeper first and starts it again on the new binary last. |
| `kelpie keeper` | The keeper itself, as `kelpie keeper --start` starts it: a loop with no terminal that runs the routines and logs to `keeper.log`. |
| `kelpie lead --renew-board` | `kelpie update`, to restart the board on the new binary. |

## Finding repos

`--repo <name>` takes a folder name, never a path. kelpie looks for a checkout of that name in each folder of `KELPIE_REPO_DIRS` (default `~/projects:~/personal:~/src:~/code`), in it or one level down. When there is none, it clones the repo from an owner found among your other checkouts' remotes.
