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

Start a task in a pooled worktree, in its own herdr Space, with Claude. Running it on a task that already runs shows it; on a task whose Space or Claude is gone, it restarts them in the same permission mode and profile and resumes the last conversation without sending the prompt again.

In a folder you have trusted in Claude Code, kelpie answers Claude's "Do you trust this folder?" itself. In one you have not, that answer is yours: the prompt waits, and kelpie types it in once you have answered and Claude's input box is empty, while the keeper or an open board runs. If you type a first message yourself before then, kelpie drops the prompt instead of sending it again.

Started in `bypassPermissions`, Claude shows its bypass permissions warning until you accept it once on the machine. kelpie never answers it: the prompt waits the same way until you have. When managed settings disable bypass permissions mode, Claude starts with permission prompts, and `kelpie start` says so.

| Option | Meaning |
| --- | --- |
| `--repo <name>` | Start in that repo instead of the current one. Repeat it for one task across repos: one half per repo, the first focused. See [Finding repos](#finding-repos). |
| `--from <ref>` | Branch off this ref instead of the remote's default branch. |
| `--backlog <item>` | Tie the task to a backlog item, as `kelpie link` does. |
| `--mode <mode>` | Claude's permission mode for the task, one of Claude Code's: `default`, `acceptEdits`, `plan`, `auto`, `dontAsk` or `bypassPermissions`. Without it, the task starts in its profile's `permission_mode`, then [`task.permission_mode`](configuration.md#keys), and without those in Claude Code's own default. |
| `--profile <name>` | Start the task with this [profile](configuration.md#profiles) instead of `task.profile`, or the built-in `task`. A task that already runs keeps the profile it started with unless this names another; restarted with a new one, it gets that profile's model, role and rules, and keeps its recorded permission mode unless `--mode` is given too. A profile that does not load is refused before a worktree is claimed. With `--after`, the queued task starts with it. |
| `--no-focus` | Stay where you are. |
| `--after <task[@repo]>` | Queue the task instead of starting it: it starts once that task has ended as done. Repeat it to wait for several. `<task>` alone means every half of that task; `<task>@<repo>` means that half. Needs a prompt, and takes no `--from`. See [`kelpie after`](#kelpie-after-name). |
| `--at <time>` | Queue the task to start at that local time: `15:00` (its next one), `today 15:00`, `tomorrow 09:00`, `fri 09:00`, `24 oct 09:00`, `2026-10-24 09:00` or `in 2h`. With `--after` too, it starts once both hold. Needs a prompt, and takes no `--from`. |
| `--why <reason>` | With `--after`: why the task waits, in a few words for the user. The board's Queued row shows it: "Waits for Billing totals (ready to push): it uses the rounding field. Starts by itself once it is pushed and CI is green." |

### `kelpie after [name]`

The queued tasks, each with what it waits for and that task's state (`working`, `ci running`, `queued`, `done`, …). With a name, that task only.

`kelpie start <name> "<prompt>" --after <task>` queues a task. It gets no worktree, Space or Claude while it waits; it shows under Queued on the board. A task it waits for is finished once it has ended as done: its commits are on the default branch and, where kelpie [watches its CI](#kelpie-ci---json), CI is green for them. A task with no commits is finished once it ends as done, and so is a routine's task closed by `kelpie routine ok`. Then the keeper starts the queued task from the default branch, fetched fresh, without taking focus, and its prompt opens with a line naming each task it waited for and the commits that task landed. The lead is told.

A task it waits for that is still open, under CI running, with failed CI, in review or resolving a conflict keeps it waiting. A task it waits for that was dropped or reset, that was closed as done with red CI, or that is gone without ending as done, keeps it from starting: it moves to Needs you with the reason, the lead is told and `notify.command` runs with `needs-you`. It never starts by itself after that.

Queueing is refused, and nothing is written, for a name that is already a task or queued, for a task that would wait for itself or in a circle, and for a task to wait for that is neither open, queued nor done. When every task named is done already, the task starts at once. Only you and the lead queue a task or change a wait. `kelpie start` on a queued name, without `--after`, is refused and names `--now` and `--drop`.

| Option | Meaning |
| --- | --- |
| `--now` | Start it now anyway, from the default branch; its prompt says which tasks it waited for were not done. |
| `--drop` | Drop the wait. Nothing was started; the backlog item it was queued from shows in the Backlog column again. |
| `--at <time>` | Set or move the time it starts at, in the words `kelpie start --at` takes. |

### `kelpie move <name> [file ...]`

Move work under way in the main checkout into a task: branch `<name>` if it exists, else the unpushed commits, plus the uncommitted files named.

| Option | Meaning |
| --- | --- |
| `--pane <id>` | Move that herdr pane along. |
| `--no-focus` | Stay where you are. |

### `kelpie land [name]`

Rebase the task onto the default branch without pushing. Uncommitted changes to tracked files stop it; untracked files do not. On a conflict it aborts its own rebase, leaves the worktree as it was, and hands the rebase to the task's Claude. `push` rebases by itself, so `land` is never required. When git refuses to start the rebase, kelpie shows git's reason, and names any files hidden from `git status` by skip-worktree or assume-unchanged that block it.

### `kelpie follow [name] [on|off]`

Keep the task rebased on the default branch, or stop. Off by default; `u` on the board switches it too. Without `on` or `off`, it says whether the task follows.

While it is on, the keeper checks the task every minute, fetching the repo when its last fetch is over five minutes old. When the task is behind, kelpie rebases it, but only when nothing is under way in it:

| The task | kelpie |
| --- | --- |
| Its Claude is mid-turn or asking for something | Waits for it to finish. |
| Has uncommitted changes to tracked files, or a rebase in progress | Waits. Untracked files do not count. |
| Is in review, or a push waits for its tests | Waits. |
| Rebases cleanly | Rebases it and logs a line in the messages. When the new commits changed files the task changes too, its Claude is asked to run their tests. |
| Conflicts | Aborts the rebase, hands it to the task's Claude as `land` does, and tells the lead. It tries again only once main or the task moves. |

Nothing is ever pushed. You or the lead switch it; a task's own Claude cannot. `done`, `drop` and `reset` switch it off.

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
| `kelpie status [--json]` | Every task and Claude session, grouped by what it needs from you. `--json` gives `away_since`, `items` and `checkouts`; a task's `push_waits` is `user` or `lead` while a push it asked for waits for the task's tests, and its `profile` is the profile it started with (`task` for the built-in one, `null` for a session that is no task). |
| `kelpie profiles [--json]` | The [profiles](configuration.md#profiles) a task can start with: the built-in `task` and `reviewer`, then each file in `~/.config/kelpie/profiles`, each with what it is for, what it is about, and its agent, model, effort, permission mode and skills. A file that does not load is listed as ignored, with the reason. `--json` gives one object per profile, and `{"error": …}` for an ignored file. |
| `kelpie list [--json]` | The pool's worktrees: each one's task or `free`, uncommitted changes, commits ahead and behind, its port, and the first line of its setup note. `--json` adds `port` and `url`, the worktree's `dev_url` filled in, each `null` when there is none. |
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
| `kelpie lead --revive-board` | Run by herdr's startup hook. Returns at once; in the background it waits up to a minute for herdr to restore the lead Space, then starts the board in its board pane if that pane sits at a shell prompt. Never touches the lead's chat. Logs to `lead.log`. |
| `kelpie lead --toggle-chat` | Hide the lead's chat so the board takes the whole width, or show it again. The chat keeps running. Takes you to the lead Space. herdr's `kelpie.chat` action runs it. |
| `kelpie focus` | In the lead Space, move focus between the board and the lead's chat. From any other Space, go to the lead's chat. A hidden chat is shown first. |
| `kelpie away [on\|off]` | Away mode. Yours only. |
| `kelpie decide <task> <answer>` | The lead's own answer to a task's question about how, whether or not you are here. Closes the question on the board and records it, for your summary when you are away. |
| `kelpie choose <task> <n> --prompt <id>` | Answer the prompt a task's Claude shows in its own pane, such as a permission or trust question, with option `<n>` as kelpie numbered it for the lead. `<id>` is the prompt's id from kelpie's message; kelpie reads the pane again and refuses when that prompt is no longer on screen. On a menu with numbers whose focus is on its first option, for options 1 to 9, kelpie presses the option's digit. Otherwise it moves the focus with the arrow keys, reads the pane to check the focus landed on the chosen option, and only then presses Enter. It never types text, and refuses an option that takes your own words; that one you answer in the Space. It then reads the pane again and says whether the menu went away, or that it could not confirm it. Yours, or the lead's on your explicit choice: the lead's settings do not allow it, so Claude Code asks you before the lead runs it. When Claude Code asks you whether the lead may run `kelpie choose`, answer plain "Yes", never "don't ask again": that question is the gate only while your settings hold no allow rule for `kelpie choose`. Refused for a task. See [Claude's own prompts in a task](../explanation/lead-and-away-mode.md#claudes-own-prompts-in-a-task). |
| `kelpie defer <task>` | The lead keeps a task's question for you while you are away. |
| `kelpie notify <text>` | Run `notify.command` with this text as a `summary` event. You or the lead only. |

## Run by a task's Claude

| Command | What it does |
| --- | --- |
| `kelpie report --state <state> --next <step> <summary>` | Where the task stands and what you should do next. `<state>` is `needs-you`, `ready`, `working` or `blocked`. With `ready`, `--card <file>` is the sign-off card the push window shows, with an Also noticed list when the task found things outside its task. A routine's task also lists its findings on a card with `needs-you`. |
| `kelpie routine ok <summary>` | A routine's task, when all is well: records the run as ok with the summary and closes the task as `drop` does. Refused outside a routine's task, from anyone but that task's Claude, and while the task has changes or commits. |
| `kelpie ask --option "<label>: <meaning>" … --recommend <n> <question>` | Ask you what you want: scope, behaviour, a trade-off that is yours. A question about how goes to the lead instead. The question opens with one plain sentence on what the task is doing, and each of the two to four options says what you get with it, one recommended, without flags, commit ids, file names or internal terms you did not use. `--plan <file>` or `--plan vault:<note>` asks you to approve a plan instead. |
| `kelpie answered <task>` | The task's question was settled elsewhere; it leaves the board. |
| `kelpie tell <to> <text>` | Message another session: a task (every half of it), `task@repo`, or `lead`. Held while the receiver waits for an answer, and logged. |

## Setup and maintenance

| Command | What it does |
| --- | --- |
| `kelpie config [--repo <path>]` | The config layers found and every key with its value in effect, the layer it comes from and what it does. See [Configuration](configuration.md). |
| `kelpie config --get <key>` | Print one machine-wide value for scripts: `worktree.server_name` or `worktree.worker_name`. |
| `kelpie doctor` | Check this machine, with the fix for each gap. Exits non-zero on a `fail`. See [Requirements](requirements.md#what-kelpie-doctor-checks). |
| `kelpie routes [--plain-http]` | Print a Caddy config with one site per pooled worktree that has a port, in every repo kelpie knows whose repo config sets [`worktree.dev_route = true`](configuration.md#keys): the host its `dev_url` names, `reverse_proxy localhost:<port>`. Sites keep the scheme and port `dev_url` names, so Caddy serves their TLS; `--plain-http` serves each as `http://<host>` for a proxy in front that ends TLS. A worktree without a port, a `dev_url` on localhost or without a host, or a host another worktree already has gets no site; the last comment line counts them with the reason, and the repos left out. It writes nothing. See [Open dev servers from a remote machine](../how-to/open-dev-servers-from-a-remote-machine.md). |
| `kelpie setup vault` | Install the optional vault plugin in Claude Code, showing each `claude plugin` command and running it only on your `y`. |
| `kelpie update` | Update everything to this machine's channel and restart idle Claude sessions. With open [kelpie issues](#kelpie-issues), it looks them up on GitHub through `gh` and asks the lead which ones the new commits likely fix. |
| `kelpie update --check` | Fetch and show the installed versions against the channel's latest. Changes nothing else. |
| `kelpie update --from <branch or path>` | Install that branch on origin, or that checkout's commits, until `--stable`. |
| `kelpie update --stable` | Return to the channel. |
| `kelpie restart` | Restart each stale Claude session that is idle, not the pane you are in, and has no background work. A task's Claude gets its role and settings written anew. Works with `restart.auto = false`. |
| `kelpie restart --all` | Restart every idle session, stale or not. |
| `kelpie keeper --status` | Whether the keeper runs, with its pid, binary, uptime, log and each of its routines' interval, last run and last error. |
| `kelpie keeper --start` | Start the keeper in the background unless one runs. Says `off` while `keeper.enabled = false`. |
| `kelpie keeper --stop` | Stop the keeper. It finishes the routine under way first. The board or herdr's next startup starts it again; `keeper.enabled = false` keeps it off. |

`--from` and `--stable` cannot be combined.

## kelpie issues

Problems and wishes about kelpie itself, saved on this machine, one file each under `issues/` in the state directory. See [Report a problem with kelpie](../how-to/report-a-problem-with-kelpie.md) and [the issue file](files.md#the-issue-file).

| Command | What it does |
| --- | --- |
| `kelpie issues [--json] [--all]` | The open issues (`open` and `retry`): id, kind, status, the version and commit they were filed on, and the GitHub issue when linked. `--all` lists every status. |
| `kelpie issue add --kind bug\|wish --title <title> [<text>\|-]` | Save an issue. The text comes from the argument, or from stdin with `-`: what happened, what you expected, how to reproduce. kelpie records its version, commit, the date and who filed it. Yours or the lead's; a task is told to tell the lead instead. |
| `kelpie issue show <id>` | Print one issue with its text. |
| `kelpie issue close <id> fixed\|wontfix\|duplicate` | Close it. |
| `kelpie issue retry <id>` | Mark it worth retrying, typically after an update that may have fixed it. |
| `kelpie issue link <id> <number\|url>` | Tie it to its issue in kelpie's GitHub repo. |
| `kelpie issue send <id>` | Print the text and a prefilled new-issue URL on GitHub, and open it in the browser (`open` on macOS, `xdg-open` on Linux with a display; over SSH only the URL). The body adds the kind, version, commit and OS, and a `kelpie-issue: <id>` line; a text that would make the URL longer than GitHub accepts is shortened, and says so. Nothing is sent until you press Submit. |

After `kelpie update`, when `gh` is installed and logged in, kelpie searches kelpie's GitHub repo for each unlinked issue's `kelpie-issue:` line and links what it finds, and an issue closed on GitHub becomes `retry`. Without `gh` this step is skipped. Then the lead gets one message: the open issues filed on older code than the update installed, with the commit subjects since, to judge which are likely fixed.

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
