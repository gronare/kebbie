# Commands

`kelpie --help` prints the same list.

## Global

| Option | Meaning |
| --- | --- |
| `-C <dir>` | Run as if started in `<dir>`. Works with every command. |
| `-h`, `--help`, `help` | Print the usage. |
| `--version`, `version` | Print the version and the commit and date the binary was built from, such as `kelpie 0.24.0 (db8963f 2026-10-21)`. Many commits share a version number, so the commit says whether a fix is in. A build outside git prints the version alone. |

Without a name, `land`, `review`, `push`, `stage`, `unstage`, `done`, `drop` and `reset` act on the task whose worktree you are in.

## Tasks

### `kelpie start <name> [prompt]`

Start a task in a pooled worktree, in its own herdr Space, with Claude. Running it on a task that already runs shows it; on a task whose Space or Claude is gone, it restarts them in the same permission mode and profile and resumes the last conversation without sending the prompt again.

In a folder you have trusted in Claude Code, kelpie answers Claude's "Do you trust this folder?" itself. In one you have not, that answer is yours: the prompt waits, and kelpie types it in once you have answered and Claude's input box is empty, while the keeper or an open board runs. If you type a first message yourself before then, kelpie drops the prompt instead of sending it again.

A task's shell commands run in Claude Code's sandbox while `task.sandbox` is on; on a machine that cannot sandbox them fully, `kelpie start` refuses and names what to install. The start's answer says when the sandbox is off, and which commands a profile such as `ops` runs outside it. See [The trust boundary](../explanation/trust-boundary.md#the-sandbox).

Started in `bypassPermissions`, Claude shows its bypass permissions warning until you accept it once on the machine. kelpie never answers it: the prompt waits the same way until you have. When managed settings disable bypass permissions mode, Claude starts with permission prompts, and `kelpie start` says so.

| Option | Meaning |
| --- | --- |
| `--repo <name>` | Start in that repo instead of the current one. Repeat it for one task across repos: one half per repo, the first focused. See [Finding repos](#finding-repos). |
| `--from <ref>` | Branch off this ref instead of the remote's default branch. |
| `--backlog <item>` | Tie the task to a backlog item, as `kelpie link` does. A vault note is claimed for this machine: see [Two machines, one vault](../explanation/vault.md#two-machines-one-vault). |
| `--anyway` | Start from a vault note that kelpie on another machine works on. Without it, `kelpie start --backlog` refuses such a note and names the machine and task that hold it. |
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

Put the task in its repo's push line; when its turn comes, fetch, rebase it onto origin's default branch, push its commits there, then close it as `done`. Only you may run it, or the lead for a task you gave push permission. The lead is told what landed. Uncommitted changes to tracked files stop it; untracked files do not.

Each repo pushes one task at a time, in the order the pushes were asked for. With the line empty, the push goes out at once and the command says how it went; otherwise it says `In the push line: 2nd in line after <task>`, and the keeper pushes it when the pushes before it are out. A task is fetched and rebased only when its turn comes, so a landing never sends a waiting task back to its Claude, and tasks nobody asked to push are never touched. Who may push is checked when you ask and again at the turn. A push leaves the line for Needs you, unpushed, when its commits changed after you chose push and nobody approved the new ones; pushing again puts it at the end of the line.

How far the task is behind the default branch does not matter. The rebase at its turn decides what happens next:

| Rebase | What happens |
| --- | --- |
| Clean, and the new upstream commits changed none of the task's files | kelpie pushes. |
| Clean, but the new upstream commits changed files the task changes too | Nothing is pushed yet. kelpie hands the task to its Claude with the files and the commits that touched them, to run their tests and report ready. When it reports ready with the same commits, kelpie pushes for whoever asked: you, or the lead under its permission, with the same checks. When its commits changed, their rules are checked again and the push is asked for again. |
| Conflict | kelpie aborts the rebase and hands it to the task's Claude, which resolves it, asks you when the conflict is a choice between two behaviours, and reports ready. The resolved commits need a rules check before the next push. |

While its Claude runs the tests or resolves the conflict, the task keeps the head of the line and the pushes behind it wait. It leaves the line for Needs you, marked `out of the push line: <why>`, when its Claude reports needs-you, or when 30 minutes pass without a ready report (or, after one, without a rules approval of the changed commits); the next push in line goes on then.

When origin is on GitHub and `gh` is logged in, the task stays open under CI running instead, and closes as `done` once every workflow run for the pushed commit is green. See `kelpie ci`. Otherwise the push result ends with `CI not watched:` and the reason.

| Option | Meaning |
| --- | --- |
| `--through <sha>` | Push the commits up to and including `<sha>` and keep the task open, also when `<sha>` is its last commit. The rest of the branch is rebased on top of what was pushed. A task kept open stays open until you close it (`done`, or Done in its options). |
| `--cancel` | Take the task out of the push line before it goes out; nothing of it is pushed, and it waits under Ready to push. The lead can take back only its own pushes. It does not apply while kelpie is pushing the task that moment, nor to a fix for red CI after a push, whose commits are on the default branch already: fix the CI or dismiss it (`kelpie ci dismiss`). `p` on a task in the line does the same on the board. A task mid-rebase does not join the line; a head found mid-rebase waits for the rebase to finish. |

### `kelpie stage [name]`

Put a ready task's commits on the main checkout's local default branch, to try them in the real tool before anything is pushed; `M` on the board does the same. Only in a repo whose config sets `worktree.stage_on_main = true`; elsewhere it refuses and names the file to set it in. Only you may run it, or the lead with the permission it would push the task with. Nothing is pushed.

kelpie fetches, brings local main up to origin's default branch (fast-forward, or a rebase of what is already staged), then cherry-picks the task's commits onto it, in order. Several tasks can be staged; each goes on top of the ones before it, in the order you stage them. The task stays open, under Ready to push, its row marked `on local main · waiting for your push`, and `kelpie status --json` says `"staged": true`. A task in the push line leaves it.

| Outcome | What happens |
| --- | --- |
| Clean | Its commits are on local main. In kelpie's own install root on the main channel, kelpie rebuilds with `scripts/build.sh`, and the board and keeper restart onto the new binary. |
| Conflict | Main is left exactly as it was. The task's Claude gets the conflict as with a push: rebase onto local main, resolve, run the tests, report ready. When it reports ready, kelpie stages it again for whoever asked; a needs-you report drops that. |

The main checkout must be on the default branch with no uncommitted changes to tracked files, and no rebase, merge or cherry-pick under way. Staging a task again replaces its commits with the branch's current ones. kelpie never rewrites commits of your own: when local main holds commits that are neither on origin nor a staged task's, `stage` and `unstage` refuse, and after another task's push main is left as it is, with `local main has your own commits a1b2c3d; push or move them, then stage again`. In kelpie's own install root, `kelpie stage` prints `building kelpie from local main…` before the build starts. A merge or an empty commit on local main counts as your own. `drop` and `reset` take a staged task off main first, and refuse with `kelpie unstage <task> first` when they cannot; `kelpie move` refuses while tasks are staged. A task stays staged only while one of its copies is on local main ahead of origin, so once main is pushed a task whose branch moved on goes through the push line again.

A staged task never goes through the push line: `kelpie push` refuses it with `it is on local main; push main`. Push main yourself, with `git push` or `p` on the Areas page, and everything staged goes out together; the keeper then closes each task once its commits are on origin, as with any task that landed outside kelpie's push (see [`kelpie done`](#kelpie-done-name)). While local main is ahead, the push line, the rules approval and the landed check all compare against origin's default branch, and a push of another task rebases what is staged onto it.

### `kelpie unstage [name]`

Take a staged task's commits off local main again, by patch, so they are found after main was rebased; the tasks staged after it stay, in their order. Its branch keeps the commits, and it waits under Ready to push. When a task staged after it builds on its commits, main is left as it was and kelpie says which tasks to take off first. On a staged task, `M` on the board does the same.

### `kelpie ci [--json]`

The CI watches, one per pushed task: its sha, the GitHub repo, each workflow run with its status and address, and the last check's error. `--json` gives the watch records.

A watch runs while a board is open: every 30 seconds the board asks `gh run list --commit <sha>` for each running watch, and asks nothing when none runs.

| State | What happens |
| --- | --- |
| No runs yet | kelpie waits up to 3 minutes for runs to appear, then treats the commit as green. |
| Running | The task waits under CI running. |
| Every run passed, skipped or neutral | After at least a minute since the push, the task closes as `done`: its note is closed when the vault is on, and the lead hears what landed. |
| A run failed | The failure is named by workflow, job and the step that failed: `CI failed: <workflow> · <job> · <step>`. Its Claude gets that and the last 60 lines of its log, and the lead gets a line. The watch stops. After a push of the whole task, the fix is part of that push: the task stays under Ready to push as `pushing for you · its Claude fixes the red CI`, and once its Claude reports ready and the reviewer passes the fix against the rules, kelpie pushes it for whoever gave the push, and watches it the same way. It moves to Needs you only when its Claude reports needs-you. After a push with `--through`, the task moves to Needs you with `CI failed: <workflow> · <job> · <step>`. |

A task with `--through` that fixes the failure pushes again as usual, and the new push is watched. `done` closes a task whatever CI says. A push with `--through` is watched too and leaves the task where it is.

A red run that is not the code's (a push outside the deploy window, infrastructure, secrets, a flake) stops counting in two ways:

- A later push to the default branch that carries the task's commits passes every workflow that failed on it: the red is superseded, and a task whose commits are on the default branch closes (see [`kelpie done`](#kelpie-done-name)).
- You dismiss it (below).

### `kelpie ci tick`

One round of checks for every running watch, as the board runs it, and what came of it.

### `kelpie ci outside "<reason>"`

Run by a task's Claude once it has read the failing log and found the failure outside the code. The row and `kelpie status --json` (`ci_outside`) carry the reason, and the lead hears it. It waives nothing: the red stands until a later push is green or you dismiss it.

### `kelpie ci dismiss <task> [--why <reason>]`

Yours alone, also `C` on the board: no Claude, the lead included, can run it. The failed run is kept on record with the reason (`--why`, else the one its Claude gave), shown in `kelpie status --json` as `ci_dismissed`; the watch and any push waiting for a fix are dropped, and a task queued after it only for that red waits again. The task then closes once its commits are on the default branch.

### `kelpie done [name]`

Close a task once every change on its branch is on the default branch, pushed as is, cherry-picked or squashed, and it has no uncommitted changes or stash: closes its Space, deletes the branch, frees the worktree, and fast-forwards a clean main checkout. Otherwise it names the missing commits.

You rarely need it. Every minute the keeper closes a task whose commits reached the default branch some other way than kelpie's push of that task: pushed with git, cherry-picked or rebased with conflict resolutions, squashed, or carried by another task's push. A task counts as landed when its branch made a commit of its own and every change it made is on the default branch, by patch or because merging it into the default branch changes nothing. A task with no commit of its own, one that only followed main or only cherry-picked commits from elsewhere, never counts. A task rebased onto main after its work landed still counts, by the commits it made. Just before it closes a task, the keeper asks herdr again and leaves the task if its Claude has started a turn. The keeper closes it as `done` does, and the lead hears one line, when:

- herdr shows its Claude idle: not mid-turn, no prompt on screen; a Claude herdr cannot find leaves the task as it is;
- no question of its waits for you, and it has not reported needs-you since its last turn began;
- its worktree has no uncommitted changes, no untracked files and no stash made on its branch;
- no push of kelpie's is under way for it and no CI watch of its runs;
- CI did not fail on what it pushed, the red was superseded by a later push, or you dismissed it. A superseded red also sets a task queued after it going again when that red was all that blocked it.

When one of these does not hold, the row says so in place: `landed on main · kelpie closes it after this turn` beside a turn, which closes once the turn ends; under Needs you, `landed on main · d closes it` beside a question, and `landed on main, but its worktree has uncommitted changes or a stash` when there are, which are never discarded. A task pushed with keep it open (`p` in the push dialog, or `--through`) stays open until you close it, whatever lands later, and shows no landed line.

### `kelpie done --landed`

One round of that check over every open task, as the keeper runs it every minute, and what came of it.

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

Your word or the lead's that a task's commits hold to the rules skill, the override for the reviewer's rules check, which gives the same approval by itself when a task reports ready. Until a commit changes, the board shows the task ready to push. Tasks cannot run it.

## Seeing what is going on

| Command | Output |
| --- | --- |
| `kelpie board [--overlay]` | The board. `--overlay` is the popup form, which `esc` closes. See [Board](board.md). |
| `kelpie status [--json]` | Every task and Claude session, grouped by what it needs from you. `--json` gives `away_since`, `items` and `checkouts`; a task's `push_waits` is `user` or `lead` while a push it asked for waits in its repo's push line or for the task's Claude, and `push_ahead` how many pushes of its repo go before it while it waits its turn, its `profile` is the profile it started with (`task` for the built-in one, `null` for a session that is no task), and its `phase` is where it stands as kelpie keeps it: `name` (`starting`, `working`, `idle`, `needs-you`, `ready`, `pushing`, `ci-running`, `ci-red` or `closing`), with `why` for `needs-you` (`left-line` when a push left its line unpushed), `head` for `ready`, `ahead` for `pushing` and `sha` for the CI phases, `since` and `by`, what moved it there. |
| `kelpie profiles [--json]` | The [profiles](configuration.md#profiles) a task can start with: the built-in `task`, `reviewer` and `ops`, then each file in `~/.config/kelpie/profiles`, each with what it is for, what it is about, and its agent, model, effort, permission mode and skills. Under a profile that runs Codex, a `codex:` line gives the Codex binary, its version and whether kelpie's plugin is installed. A file that does not load is listed as ignored, with the reason. `--json` gives one object per profile, and `{"error": …}` for an ignored file. |
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
| `kelpie report --state <state> --next <step> <summary>` | Where the task stands and what you should do next. `<state>` is `needs-you`, `ready`, `working` or `blocked`. With `ready`, `--card <file>` is the sign-off card the push window shows, with an Also noticed list when the task found things outside its task. A routine's task also lists its findings on a card with `needs-you`. With `kelpie:work-rules` as the rules skill, `ready` is refused while a commit lacks a subject, a blank line and a body. After a ready report the keeper starts the reviewer, which checks the commits against the rules skill: a pass approves them, a break goes to the task and a line to the lead, and no verdict within 15 minutes leaves the check to the lead. A ready report on code already checked gets the stored findings back, or goes to the lead after no verdict, without a new run. |
| `kelpie routine ok <summary>` | A routine's task, when all is well: records the run as ok with the summary and closes the task as `drop` does. Refused outside a routine's task, from anyone but that task's Claude, and while the task has changes or commits. |
| `kelpie ask --option "<label>: <meaning>" … --recommend <n> <question>` | Ask you what you want: scope, behaviour, a trade-off that is yours. A question about how goes to the lead instead. The question opens with one plain sentence on what the task is doing, and each of the two to four options says what you get with it, one recommended, without flags, commit ids, file names or internal terms you did not use. `--plan <file>` or `--plan vault:<note>` asks you to approve a plan instead. |
| `kelpie answered <task>` | The task's question was settled elsewhere, dropped, or sent back to be asked again; it leaves the board, and a `kelpie tell` to that task is no longer held. |
| `kelpie tell <to> <text>` | Message another session: a task (every half of it), `task@repo`, or `lead`. Held while the receiver waits for an answer, and logged. |

## Setup and maintenance

| Command | What it does |
| --- | --- |
| `kelpie config [--repo <path>]` | The config layers found and every key with its value in effect, the layer it comes from and what it does. See [Configuration](configuration.md). |
| `kelpie config --get <key>` | Print one machine-wide value for scripts: `worktree.server_name` or `worktree.worker_name`. |
| `kelpie docs [<words>]` | Where kelpie's docs are on this machine: `docs/` in the checkout kelpie is installed from. With words, the pages whose title, file name or headings match any of them, best first: a title match above a heading match, then reference, how-to, explanation and tutorial. A single character, such as the board key `M`, keeps its case and matches a whole word in a heading or the first column of a table. Each page comes with its path, its first sentence and the headings or table rows that matched; twelve at most. Search in English, the docs' language. Without words, the [docs index](../README.md) by section, each page's path relative to the docs folder. It reads only files and never needs the network. When the install has no `docs/`, it says so and points at `kelpie help` and the docs online, on the install's GitHub remote or `https://github.com/gronare/kelpie/tree/main/docs`. |
| `kelpie doctor` | Check this machine, with the fix for each gap, the task sandbox's state among them. Exits non-zero on a `fail`. See [Requirements](requirements.md#what-kelpie-doctor-checks). |
| `kelpie routes [--plain-http]` | Print a Caddy config with one site per pooled worktree that has a port, in every repo kelpie knows whose repo config sets [`worktree.dev_route = true`](configuration.md#keys): the host its `dev_url` names, `reverse_proxy localhost:<port>`. Sites keep the scheme and port `dev_url` names, so Caddy serves their TLS; `--plain-http` serves each as `http://<host>` for a proxy in front that ends TLS. A worktree without a port, a `dev_url` on localhost or without a host, or a host another worktree already has gets no site; the last comment line counts them with the reason, and the repos left out. It writes nothing. See [Open dev servers from a remote machine](../how-to/open-dev-servers-from-a-remote-machine.md). |
| `kelpie setup vault` | Install the optional vault plugin in Claude Code, showing each `claude plugin` command and running it only on your `y`. |
| `kelpie setup codex` | Install kelpie's Codex plugin in `$CODEX_HOME/plugins/cache/kelpie/kelpie/local/`, after saying what it writes and only on your `y`. Its `SessionStart`, `UserPromptSubmit` and `Stop` hooks record a Codex task's session, give it its role and ask for a report before a turn ends. It never edits Codex's `config.toml` or `hooks.json`: kelpie enables the plugin and trusts its hooks only on the command line of the Codex sessions it starts. Run again, it writes only what changed, and nothing when the plugin is current. Yours alone: a task cannot run it. See [Run a task with Codex](../how-to/run-a-task-with-codex.md). |
| `kelpie setup codex --uninstall` | Remove kelpie's Codex plugin, on your `y`. |
| `kelpie update` | Update everything to this machine's channel and restart idle Claude sessions. With open [kelpie issues](#kelpie-issues), it looks them up on GitHub through `gh` and asks the lead which ones the new commits likely fix. |
| `kelpie update --check` | Fetch and show the installed versions against the channel's latest. Changes nothing else. kelpie's installed version is its binary's, with the commit it was built from; a checkout ahead of it shows as `0.25.0 in the checkout: rebuild`. On the release channel, the latest is the highest tag whose release is published with a binary for this machine, and a newer tag that is not yet says `kelpie <version> is not published yet; try again in a while`. |
| `kelpie update --from <branch or path>` | Install that branch on origin, or that checkout's commits, until `--stable`. |
| `kelpie update --stable` | Return to the channel. |
| `kelpie restart` | Restart each stale Claude session that is idle, not the pane you are in, and has no background work. A task's Claude gets its role and settings written anew. Works with `restart.auto = false`. |
| `kelpie restart --all` | Restart every idle session, stale or not. |
| `kelpie keeper --status` | Whether the keeper runs, with its pid, binary, uptime, log and each of its routines' interval, last run and last error. |
| `kelpie keeper --start` | Start the keeper in the background unless one runs. Says `off` while `keeper.enabled = false`. |
| `kelpie keeper --stop` | Stop the keeper. It finishes the routine under way first. The board or herdr's next startup starts it again; `keeper.enabled = false` keeps it off. |

`--from` and `--stable` cannot be combined.

## The hub

Show the board on your phone through a kelpie hub. See [See your board on your phone](../how-to/see-your-board-on-your-phone.md).

| Command | What it does |
| --- | --- |
| `kelpie hub [status]` | The hub this machine joined, its name and id there, each paired phone with when it paired, the board's last revision and when it went out, the last error, and which commands from phones the machine accepts (none: the phone reads). |
| `kelpie hub join <url> <code> [--name <name>]` | Join the hub at `<url>` with a code from `kelpie-hub admin join-code`. Makes the machine's keys and keeps them, with the hub's token for it, in [`hub/`](files.md#the-state-directory); nothing prints them. `--name` is the name the hub and your phones show; the short host name otherwise. Takes `https://`, or `http://` for a hub on this machine. Refused when the machine has joined a hub already. Yours only. |
| `kelpie hub pair` | Pair one phone: prints a code to type in the hub's web app, waits up to 10 minutes, then shows six digits and admits the phone only on your `y` when the phone shows the same. The phone commits to its keys before it sees the machine's, and the machine checks them against that commitment before it shows any digits, so a hub that swaps keys cannot pair. A mismatch, a `n` or a timeout pairs nothing; start again. Yours only, in your own terminal: it refuses to run without one. |
| `kelpie hub move <url>` | The hub moved to `<url>`: kelpie checks only that it answers `/healthz`, without sending the token, then posts there from now on with the machine's token. That check proves nothing about who runs it, so the address is yours to check: move only to a hub you run or trust. Yours only. |
| `kelpie hub leave` | Take each paired phone off this machine's board and forget the hub, the keys and the token. The hub lists the machine until its admin runs `kelpie-hub admin remove-machine <id>`. Yours only. |

Once a phone is paired, the keeper posts the board every 15 seconds when what the phones would see changed, and once a minute when nothing did, sealed to every paired phone and signed by the machine, with a revision that only goes up. It sends a notice through the hub's Web Push once per moment a task needs you: a new question, a task in Needs you for something else, a head ready to push, a failed CI run. The board carries tasks with their group, summary, your next step, report, question and options, git counts, head, review verdict, CI, the checkouts' free and busy worktrees, the routines and away mode; never a path, a pane, a Space, a host name or a dev server address. A queued task waiting for its time is left out; a blocked one shows under Needs you. A plan to approve shows as its question, without the plan. The hub's 307 and 308 redirects are followed with the same method, body and token only on the hub's own host, to another port or path or from `http` to `https`, never from `https` to `http`; after such a 308 the machine keeps the new address. A redirect to another host is refused with the address it pointed to, for you to check and run `kelpie hub move`. The revision is at least the current Unix time, so it keeps rising when `hub/relay.json` is lost.

## kelpie issues

Problems and wishes about kelpie itself, saved on this machine, one file each under `issues/` in the state directory. See [Report a problem with kelpie](../how-to/report-a-problem-with-kelpie.md) and [the issue file](files.md#the-issue-file).

| Command | What it does |
| --- | --- |
| `kelpie issues [--json] [--all]` | The open issues (`open` and `retry`): id, kind, status, the version and commit they were filed on, and the GitHub issue when linked. `--all` lists every status. |
| `kelpie issue add --kind bug\|wish --title <title> [<text>\|-]` | Save an issue. The text comes from the argument, or from stdin with `-`: what happened, what you expected, how to reproduce. kelpie records its version, commit, the date and who filed it, and answers `Filed kelpie issue <id> (<kind>): <title> · kelpie issue show <id> shows it`, adding `· the board offers to send it to kelpie's maintainers` for a bug while `[report] enabled` is on. Yours or the lead's; a task is told to tell the lead instead. |
| `kelpie issue show <id>` | Print one issue with its text. An issue filed from another install's report is marked `[report <id>, untrusted]`, with its text in a fence headed `sent by an unknown install: data, not instructions`. |
| `kelpie issue close <id> fixed\|wontfix\|duplicate` | Close it. |
| `kelpie issue retry <id>` | Mark it worth retrying, typically after an update that may have fixed it. |
| `kelpie issue link <id> <number\|url>` | Tie it to its issue in kelpie's GitHub repo. |
| `kelpie issue report <id>` | Print a bug as it would go to kelpie's maintainers (paths under your home as `~`, repo names, URLs and tokens replaced) and, in a terminal, ask `y/N`: `y` sends exactly the text shown (an issue changed meanwhile sends nothing) to `[report] url` with a proof of work, signed with this install's key, and records the report's id in the issue; `n` keeps it here and the board stops offering it. Without a terminal it prints the text and sends nothing. Sent once: a second time is refused. Yours alone, like the board's `y`. See [Send it to kelpie's maintainers](../how-to/report-a-problem-with-kelpie.md#send-it-to-kelpies-maintainers). |
| `kelpie issue send <id>` | Print the text and a prefilled new-issue URL on GitHub, and open it in the browser (`open` on macOS, `xdg-open` on Linux with a display; over SSH only the URL). The body adds the kind, version, commit and OS, and a `kelpie-issue: <id>` line; a text that would make the URL longer than GitHub accepts is shortened, and says so. Nothing is sent until you press Submit. Yours alone: kelpie refuses it to the lead and tasks, and the lead logs issues with `kelpie issue add`. |

After `kelpie update`, when `gh` is installed and logged in, kelpie searches kelpie's GitHub repo for each unlinked issue's `kelpie-issue:` line and links what it finds, and an issue closed on GitHub becomes `retry`. Without `gh` this step is skipped. Then the lead gets one message: the open issues filed on older code than the update installed, with the commit subjects since, to judge which are likely fixed.

## Used by kelpie itself

These run from hooks, the herdr plugin and kelpie's own processes. You do not run them.

| Command | Run by |
| --- | --- |
| `kelpie hook` | The hooks kelpie gives a task's Claude. Reads the hook's JSON on stdin. |
| `kelpie hook codex <event>` | kelpie's Codex plugin, for `session-start`, `prompt` and `stop`. Reads the hook's JSON on stdin and prints nothing outside a kelpie task's worktree. |
| `kelpie statusline` | Claude Code's `statusLine` command. Keeps the plan's usage for the board and prints nothing. |
| `kelpie backlog save` | The lead, after refreshing the backlog. Reads a JSON array on stdin. |
| `kelpie messages --unread` | The lead's prompt hook. |
| `kelpie review --run` | The reviewer process `kelpie review` starts. |
| `kelpie update --finish` | The newly built binary, at the end of `kelpie update`. It stops the keeper first and starts it again on the new binary last. |
| `kelpie keeper` | The keeper itself, as `kelpie keeper --start` starts it: a loop with no terminal that runs the routines and logs to `keeper.log`. |
| `kelpie lead --renew-board` | `kelpie update`, to restart the board on the new binary. |

## Finding repos

`--repo <name>` takes a folder name, never a path. kelpie looks for a checkout of that name in each folder of `KELPIE_REPO_DIRS` (default `~/projects:~/personal:~/src:~/code`), in it or one level down. When there is none, it clones the repo from an owner found among your other checkouts' remotes.
