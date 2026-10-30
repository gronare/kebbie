# Commands

Every kebbie command with its options and what it does. `kebbie --help` prints the same list.

## Global

| Option | What it does |
| --- | --- |
| `-C <dir>` | Run as if started in `<dir>`. Works with every command. |
| `-h`, `--help`, `help` | Print the usage. |
| `--version`, `version` | Print which kebbie runs: `kebbie main` on the main channel, the version, such as `kebbie 0.27.0`, on the release channel. |
| `version --built` | Print the version and the commit and date the binary was built from, such as `kebbie 0.27.0 (db8963f 2026-10-21)`. |

kebbie names itself the same way in `kebbie doctor`, the update window and the board's `?` window. kebbie reads `version --built` itself to tell builds apart; a build outside git prints the version alone.

Without a name, `land`, `follow`, `review`, `evidence`, `push`, `stage`, `unstage`, `done`, `drop` and `reset` act on the task whose worktree you are in.

## Tasks

| Command | What it does |
| --- | --- |
| [`kebbie start`](#kebbie-start-name-prompt) | Start a task in its own worktree, Space and Claude, or queue it. |
| [`kebbie after`](#kebbie-after-name) | List the queued tasks, or start, drop or move one. |
| [`kebbie move`](#kebbie-move-name-file-) | Move work from the main checkout into a task. |
| [`kebbie land`](#kebbie-land-name) | Rebase a task onto the default branch without pushing. |
| [`kebbie follow`](#kebbie-follow-name-onoff) | Keep a task rebased on the default branch. |
| [`kebbie park`](#kebbie-park-name---until-when), [`unpark`](#kebbie-unpark-name) | Set a task aside until later, or bring it back now. |
| [`kebbie push`](#kebbie-push-name) | Put a task in its repo's push line. |
| [`kebbie stage`](#kebbie-stage-name), [`unstage`](#kebbie-unstage-name) | Put a ready task's commits on local main to try them, or take them off. |
| [`kebbie ci`](#kebbie-ci---json) | The CI watches of pushed tasks. |
| [`kebbie done`](#kebbie-done-name) | Close a task whose work is on the default branch. |
| [`kebbie drop`](#kebbie-drop-name---force), [`reset`](#kebbie-reset-name) | Throw a task away, or stop it for good. |
| [`kebbie link`](#kebbie-link-name-item) | Tie a running task to a backlog item. |
| [`kebbie tidy`](#kebbie-tidy-name-note) | Ask a task's Claude to reshape its commits. |
| [`kebbie review`](#kebbie-review-name) | Send a task to the reviewer. |
| [`kebbie evidence`](#kebbie-evidence-name---json) | The test runs kebbie recorded for a task. |
| [`kebbie approve`](#kebbie-approve-name) | Approve a task's commits against the rules. |
| [`kebbie accept`](#kebbie-accept-name-sha) | Accept one of a task's commits. |
| [`kebbie accepted`](#kebbie-accepted-name) | Which of a task's commits are accepted. |
| [`kebbie steps`](#kebbie-steps-name-step-) | A task's steps and the state of each. |

### `kebbie start <name> [prompt]`

Start a task in a pooled worktree, in its own herdr Space, with Claude.

- On a task that already runs, it shows it.
- On a task whose Space or Claude is gone, it restarts them in the same permission mode and profile and resumes the last conversation, without sending the prompt again.

| Option | What it does |
| --- | --- |
| `--repo <name>` | Start in that repo instead of the current one. Repeat it for one task across repos: one half per repo, the first focused. See [Finding repos](#finding-repos). |
| `--from <ref>` | Branch off this ref instead of the remote's default branch. |
| `--backlog <item>` | Tie the task to a backlog item, as `kebbie link` does. A vault note is claimed for this machine: see [Two machines, one vault](../explanation/vault.md#two-machines-one-vault). |
| `--anyway` | Start from a vault note that kebbie on another machine works on. Without it, `--backlog` refuses such a note and names the machine and task that hold it. |
| `--mode <mode>` | Claude's permission mode, one of Claude Code's: `default`, `acceptEdits`, `plan`, `auto`, `dontAsk` or `bypassPermissions`. |
| `--profile <name>` | Start with this [profile](configuration.md#profiles) instead of `task.profile`, or the built-in `task`. |
| `--focus` | Go to the task's Space, also when the lead or a task starts it. |
| `--no-focus` | Stay where you are, also when you start it yourself. |
| `--after <task[@repo]>` | Queue the task until that task has ended as done. Repeat it to wait for several. See [`kebbie after`](#kebbie-after-name). |
| `--at <time>` | Queue the task to start at that local time. |
| `--why <reason>` | With `--after`: why the task waits, in a few words for the user, shown on the board's Queued row. |

A task you start takes you to its Space. One the lead or another task starts leaves you where you are, unless it passes `--focus`.

Without `--mode`, the task starts in its profile's `permission_mode`, then [`task.permission_mode`](configuration.md#keys), and without those in Claude Code's own default.

With `--profile`:

- A task that already runs keeps the profile it started with unless this names another.
- Restarted with a new one, it gets that profile's model, role and rules, and keeps its recorded permission mode unless `--mode` is given too.
- A profile that does not load is refused before a worktree is claimed.
- With `--after`, the queued task starts with it.

With `--after` or `--at`:

- `<task>` alone means every half of that task; `<task>@<repo>` means that half.
- Both need a prompt and take no `--from`. With both, the task starts once both hold.
- `--at` takes `15:00` (its next one), `today 15:00`, `tomorrow 09:00`, `fri 09:00`, `24 oct 09:00`, `2026-10-24 09:00` or `in 2h`.
- A `--why` reads on the Queued row as: "Waits for Billing totals (ready to push): it uses the rounding field. Starts by itself once it is pushed and CI is green."

Prompts at the start:

- In a folder you have trusted in Claude Code, kebbie answers Claude's "Do you trust this folder?" itself.
- In one you have not, that answer is yours. The prompt waits, and kebbie types it in once you have answered and Claude's input box is empty, while the keeper or an open board runs. If you type a first message yourself before then, kebbie drops the prompt instead of sending it again.
- Started in `bypassPermissions`, Claude shows its bypass permissions warning until you accept it once on the machine. kebbie never answers it: the prompt waits the same way until you have.
- When managed settings disable bypass permissions mode, Claude starts with permission prompts, and `kebbie start` says so.

A task's shell commands run in Claude Code's sandbox while `task.sandbox` is on. On a machine that cannot sandbox them fully, `kebbie start` refuses and names what to install. The start's answer says when the sandbox is off, and which commands a profile such as `ops` runs outside it. See [The trust boundary](../explanation/trust-boundary.md#the-sandbox).

### `kebbie after [name]`

The queued tasks, each with what it waits for and that task's state (`working`, `ci running`, `queued`, `done`, …). With a name, that task only.

| Option | What it does |
| --- | --- |
| `--now` | Start it now anyway, from the default branch; its prompt says which tasks it waited for were not done. |
| `--drop` | Drop the wait. Nothing was started; the backlog item it was queued from shows in the Backlog column again. |
| `--at <time>` | Set or move the time it starts at, in the words `kebbie start --at` takes. |

`kebbie start <name> "<prompt>" --after <task>` queues a task. It gets no worktree, Space or Claude while it waits, and shows under Queued on the board.

When it starts:

- A task it waits for is finished once it has ended as done: its commits are on the default branch and, where kebbie [watches its CI](#kebbie-ci---json), CI is green for them.
- A task with no commits is finished once it ends as done, and so is a routine's task closed by `kebbie routine ok`.
- Then the keeper starts the queued task from the default branch, fetched fresh, without taking focus. Its prompt opens with a line naming each task it waited for and the commits that task landed. The lead is told.
- When every task named is done already, the task starts at once.

When it does not:

- It keeps waiting while a task it waits for is still open, under CI running, with failed CI, in review or resolving a conflict.
- It is kept from starting when a task it waits for was dropped or reset, was closed as done with red CI, or is gone without ending as done. It moves to Needs you with the reason, the lead is told and `notify.command` runs with `needs-you`. It never starts by itself after that.

Queueing is refused, and nothing is written, for a name that is already a task or queued, for a task that would wait for itself or in a circle, and for a task to wait for that is neither open, queued nor done. Only you and the lead queue a task or change a wait. `kebbie start` on a queued name, without `--after`, is refused and names `--now` and `--drop`.

### `kebbie move <name> [file ...]`

Move work under way in the main checkout into a task: branch `<name>` if it exists, else the unpushed commits, plus the uncommitted files named.

| Option | What it does |
| --- | --- |
| `--pane <id>` | Move that herdr pane along. |
| `--no-focus` | Stay where you are. |

### `kebbie land [name]`

Rebase the task onto the default branch without pushing. `push` rebases by itself, so `land` is never required.

- Uncommitted changes to tracked files stop a push of all its commits; untracked files do not.
- On a conflict it aborts its own rebase, leaves the worktree as it was, and hands the rebase to the task's Claude.
- When git refuses to start the rebase, kebbie shows git's reason, and names any files hidden from `git status` by skip-worktree or assume-unchanged that block it.

### `kebbie follow [name] [on|off]`

Keep the task rebased on the default branch, or stop. Off by default; `u` on the board switches it too. Without `on` or `off`, it says whether the task follows.

While it is on, the keeper checks the task every minute, fetching the repo when its last fetch is over five minutes old. When the task is behind, kebbie rebases it, but only when nothing is under way in it:

| The task | kebbie |
| --- | --- |
| Its Claude is mid-turn or asking for something | Waits for it to finish. |
| Has uncommitted changes to tracked files, or a rebase in progress | Waits. Untracked files do not count. |
| Is in review, or a push waits for its tests | Waits. |
| Has an open question, or sits under Needs you | Waits, so the code does not move under your answer. It follows again once the task moves on. |
| Rebases cleanly | Rebases it and logs a line in the messages. When the new commits changed files the task changes too, its Claude is asked to run their tests. |
| Conflicts | Aborts the rebase, hands it to the task's Claude as `land` does, and tells the lead. It tries again only once main or the task moves. |

Nothing is ever pushed. You or the lead switch it; a task's own Claude cannot. `done`, `drop` and `reset` switch it off.

### `kebbie park [name] [--until <when>]`

Set the task aside until later, as `Z` on the board does. It keeps working, and while it is parked:

| What | While parked |
| --- | --- |
| The board | It leaves its group for the [parked line](board.md#parked), which counts its open question or red CI. |
| The lead | Leaves it out of its summaries: Done, Going on and Needs you. Its reports, notices and question copies stay on the board. |
| Notifications | None is sent for it. |
| [`kebbie status --json`](#kebbie-status---json) | `parked_until` says when it comes back, and its `group` is `PARKED`. |

| `--until` | Comes back |
| --- | --- |
| left out | Tomorrow at 09:00. |
| `tomorrow 13:00`, `fri 09:00`, `24 oct 09:00`, `2026-11-02 08:30`, `in 3d` | Then, read as [`kebbie start --at`](#kebbie-start-name-prompt) reads a time. |
| `fri`, `24 oct`, `2026-11-02`, `tomorrow` | That day at 09:00. |
| `unparked` | When you unpark it. |

It comes back to its group by itself at that time. Yours to run, or the lead's on your word; a task's own Claude cannot. `done`, `drop` and `reset` end the park.

### `kebbie unpark [name]`

Bring a parked task back to its group now, as `Z` on its row does.

### `kebbie push [name]`

Put the task in its repo's push line. When its turn comes, kebbie fetches, rebases it onto origin's default branch, pushes its commits there, then closes it as `done`.

- Only you may run it, or the lead for a task you gave push permission. Who may push is checked when you ask and again at the turn.
- The lead is told what landed.
- Uncommitted changes to tracked files stop a push of all its commits; untracked files do not.

| Option | What it does |
| --- | --- |
| `--through <sha>` | Push the commits up to and including `<sha>` and keep the task open, also when `<sha>` is its last commit. |
| `--cancel` | Take the task out of the push line before it goes out. Nothing of it is pushed, and it waits under Ready to push. |

With `--through`, kebbie leaves the task's worktree alone: uncommitted changes do not stop it, and its HEAD stays where its Claude left it. When the default branch moved, kebbie rebases the commits up to `<sha>` onto it in a worktree of its own and pushes them from there, then tells the task's Claude to rebase onto the default branch once its work in progress is committed. When those commits changed the same files as the default branch, or conflict with it, its Claude rebases, runs their tests (or resolves the conflict) and reports ready first. A commit the task makes after `<sha>` meanwhile does not stop the push; a change to the commits up to `<sha>` does. A task kept open stays open until you close it (`done`, or Done in its options).

With `--cancel`:

- The lead can take back only its own pushes.
- It does not apply while kebbie is pushing the task that moment, nor to a fix for red CI after a push, whose commits are on the default branch already: fix the CI or dismiss it (`kebbie ci dismiss`).
- `p` on a task in the line does the same on the board.

The push line:

- Each repo pushes one task at a time, in the order the pushes were asked for.
- With the line empty, the push goes out at once and the command says how it went. Otherwise it says `In the push line: 2nd in line after <task>`, and the keeper pushes it when the pushes before it are out.
- A task is fetched and rebased only when its turn comes, so a landing never sends a waiting task back to its Claude, and tasks nobody asked to push are never touched.
- A task mid-rebase does not join the line; a head found mid-rebase waits for the rebase to finish.
- A push leaves the line for Needs you, unpushed, when its commits changed after you chose push and nobody approved the new ones. Pushing again puts it at the end of the line.

How far the task is behind the default branch does not matter. The rebase at its turn decides what happens next:

| Rebase | What happens |
| --- | --- |
| Clean, and the new upstream commits changed none of the task's files | kebbie pushes. |
| Clean, but the new upstream commits changed files the task changes too | Nothing is pushed yet. kebbie hands the task to its Claude with the files and the commits that touched them, to run their tests and report ready. |
| Conflict | kebbie aborts the rebase and hands it to the task's Claude, which resolves it, asks you when the conflict is a choice between two behaviours, and reports ready. |

- After the tests, when the task reports ready with the same commits, kebbie pushes for whoever asked: you, or the lead under its permission, with the same checks. When its commits changed, their rules are checked again and the push is asked for again.
- After a conflict, the resolved commits need a rules check before the next push.
- While its Claude runs the tests or resolves the conflict, the task keeps the head of the line and the pushes behind it wait.
- It leaves the line for Needs you, marked `out of the push line: <why>`, when its Claude reports needs-you, or when 30 minutes pass without a ready report (or, after one, without a rules approval of the changed commits). The next push in line goes on then.

When origin is on GitHub and `gh` is logged in, the task stays open under CI running instead, and closes as `done` once every workflow run for the pushed commit is green. See `kebbie ci`. Otherwise the push result ends with `CI not watched:` and the reason.

### `kebbie stage [name]`

Put a ready task's commits on the main checkout's local default branch, to try them in the real tool before anything is pushed. `M` on the board does the same. Nothing is pushed.

- Only in a repo whose config sets `worktree.stage_on_main = true`; elsewhere it refuses and names the file to set it in.
- Only you may run it, or the lead with the permission it would push the task with.
- The main checkout must be on the default branch with no uncommitted changes to tracked files, and no rebase, merge or cherry-pick under way.

kebbie fetches, brings local main up to origin's default branch (fast-forward, or a rebase of what is already staged), then cherry-picks the task's commits onto it, in order.

| Outcome | What happens |
| --- | --- |
| Clean | Its commits are on local main. In kebbie's own install root on the main channel, kebbie rebuilds with `scripts/build.sh`, and the board and keeper restart onto the new binary. |
| Conflict | Main is left exactly as it was. The task's Claude gets the conflict as with a push: rebase onto local main, resolve, run the tests, report ready. When it reports ready, kebbie stages it again for whoever asked; a needs-you report drops that. |

A staged task:

- Several tasks can be staged; each goes on top of the ones before it, in the order you stage them. Staging a task again replaces its commits with the branch's current ones.
- It stays open, under Ready to push, its row marked `on local main · waiting for your push`, and `kebbie status --json` says `"staged": true`. A task in the push line leaves it.
- It never goes through the push line: `kebbie push` refuses it with `it is on local main; push main`.
- It stays staged only while one of its copies is on local main ahead of origin, so once main is pushed a task whose branch moved on goes through the push line again.

Push main yourself, with `git push` or `p` on the Areas page, and everything staged goes out together. The keeper then closes each task once its commits are on origin, as with any task that landed outside kebbie's push (see [`kebbie done`](#kebbie-done-name)). While local main is ahead, the push line, the rules approval and the landed check all compare against origin's default branch, and a push of another task rebases what is staged onto it.

kebbie never rewrites commits of your own:

- When local main holds commits that are neither on origin nor a staged task's, `stage` and `unstage` refuse. After another task's push, main is left as it is, with `local main has your own commits a1b2c3d; push or move them, then stage again`.
- A merge or an empty commit on local main counts as your own.
- `drop` and `reset` take a staged task off main first, and refuse with `kebbie unstage <task> first` when they cannot.
- `kebbie move` refuses while tasks are staged.

In kebbie's own install root, `kebbie stage` prints `building kebbie from local main…` before the build starts.

### `kebbie unstage [name]`

Take a staged task's commits off local main again, by patch, so they are found after main was rebased. On a staged task, `M` on the board does the same.

- The tasks staged after it stay, in their order.
- Its branch keeps the commits, and it waits under Ready to push.
- When a task staged after it builds on its commits, main is left as it was and kebbie says which tasks to take off first.

### `kebbie ci [--json]`

The CI watches, one per pushed task: its sha, the GitHub repo, each workflow run with its status and address, and the last check's error. `--json` gives the watch records.

Every 30 seconds the keeper (or an open board, with `keeper.enabled = false`) asks `gh run list --commit <sha>` for each running watch, and asks nothing when none runs.

| State | What happens |
| --- | --- |
| No runs yet | kebbie waits up to 3 minutes for runs to appear, then treats the commit as green. |
| Running | The task waits under CI running. |
| Every run passed, skipped or neutral | After at least a minute since the push, the task closes as `done`: its note is closed when the vault is on, and the lead hears what landed. |
| Every run done and one cancelled | A cancel is no failure: a deploy window refused the run, or a newer push superseded it. The task stays under CI running with `<workflow> cancelled, waits for the next run on <branch> to pass`, and kebbie asks every 5 minutes. A later run of that workflow on the branch that passes, on a commit holding the pushed one, closes the task as `done`. After 7 days without one, it closes as a commit with no runs does. |
| A run failed | The watch stops, and the failure is named by workflow, job and step: `CI failed: <workflow> · <job> · <step>`. |

When a run failed:

- Its Claude gets the failure and the last 60 lines of its log, and the lead gets a line.
- After a push of the whole task, the fix is part of that push. The task stays under Ready to push as `pushing for you · its Claude fixes the red CI`. Once its Claude reports ready and the reviewer passes the fix against the rules, kebbie pushes it for whoever gave the push, and watches it the same way. It moves to Needs you only when its Claude reports needs-you.
- After a push with `--through`, the task moves to Needs you with `CI failed: <workflow> · <job> · <step>`. When it fixes the failure it pushes again as usual, and the new push is watched.

A push with `--through` is watched too and leaves the task where it is. `done` closes a task whatever CI says.

A red run that is not the code's (a push outside the deploy window, infrastructure, secrets, a flake) stops counting in two ways:

- A later push to the default branch that carries the task's commits passes every workflow that failed on it: the red is superseded, and a task whose commits are on the default branch closes (see [`kebbie done`](#kebbie-done-name)).
- You dismiss it (below).

### `kebbie ci tick`

One round of checks for every running watch, as the keeper runs it, and what came of it.

### `kebbie ci outside "<reason>"`

Run by a task's Claude once it has read the failing log and found the failure outside the code. The row and `kebbie status --json` (`ci_outside`) carry the reason, and the lead hears it. It waives nothing: the red stands until a later push is green or you dismiss it.

### `kebbie ci dismiss <task> [--why <reason>]`

Yours alone, also `C` on the board: no Claude, the lead included, can run it.

- The failed run is kept on record with the reason (`--why`, else the one its Claude gave), shown in `kebbie status --json` as `ci_dismissed`.
- The watch and any push waiting for a fix are dropped, and a task queued after it only for that red waits again.
- The task then closes once its commits are on the default branch.

### `kebbie done [name]`

Close a task once every change on its branch is on the default branch, pushed as is, cherry-picked or squashed, and it has no uncommitted changes or stash. It closes its Space, deletes the branch, frees the worktree, and fast-forwards a clean main checkout. Otherwise it names the missing commits.

You rarely need it. Every minute the keeper closes a task whose commits reached the default branch some other way than kebbie's push of that task: pushed with git, cherry-picked or rebased with conflict resolutions, squashed, or carried by another task's push.

What counts as landed:

- The branch made a commit of its own, and every change it made is on the default branch, by patch or because merging it into the default branch changes nothing.
- A task with no commit of its own, one that only followed main or only cherry-picked commits from elsewhere, never counts.
- A task rebased onto main after its work landed still counts, by the commits it made.

The keeper closes it as `done` does, and the lead hears one line, when:

- herdr shows its Claude idle: not mid-turn, no prompt on screen; a Claude herdr cannot find leaves the task as it is;
- no question of its waits for you, and it has not reported needs-you since its last turn began;
- its worktree has no uncommitted changes, no untracked files and no stash made on its branch;
- no push of kebbie's is under way for it and no CI watch of its runs;
- CI did not fail on what it pushed, the red was superseded by a later push, or you dismissed it. A superseded red also sets a task queued after it going again when that red was all that blocked it.

Just before it closes a task, the keeper asks herdr again and leaves the task if its Claude has started a turn.

When one of these does not hold, the row says so in place:

- `landed on main · kebbie closes it after this turn` beside a turn, which closes once the turn ends;
- under Needs you, `landed on main · d closes it` beside a question;
- `landed on main, but its worktree has uncommitted changes or a stash` when there are, which are never discarded.

A task pushed with keep it open (`p` in the push dialog, or `--through`) stays open until you close it, whatever lands later, and shows no landed line.

### `kebbie done --landed`

One round of that check over every open task, as the keeper runs it every minute, and what came of it.

### `kebbie drop [name] [--force]`

Throw a task away. Refuses while it has uncommitted changes or unpushed commits, unless `--force`.

### `kebbie reset [name]`

Stop a task for good: discard its changes and commits, free its worktree, close its Space, tell the lead what was thrown away, and put the backlog item it was started from back in the Backlog column.

### `kebbie link <name> <item>`

Tie a running task to a backlog item or project note: a vault path, a Linear id, or `local:<Title>`. `done` tells the lead to close the item; `reset` puts it back.

### `kebbie tidy <name> [note]`

Ask the task's Claude to reshape its commits by the rules skill (`task.rules_skill`), keeping the final tree identical. The note is passed along, for example `"squash into one"`.

### `kebbie review [name]`

Send the task to the reviewer: a separate, read-only Claude reviews its commits in the background. The task waits under In review, and its push with it, until a `verdict: ok` on its current code. The reviewer reads the rules skill too, so that pass is also the task's rules approval and no separate rules check runs.

| Option | What it does |
| --- | --- |
| `--withdraw` | Take the task out of review: the reviewer stops, its verdict goes, push is offered again, the lead is told. Only you may run it. |

### `kebbie evidence [name] [--json]`

The test runs kebbie recorded for the task, newest first: each one's result, the command, when it ran, how long it took, its exit code, and the commit it ran on. `--json` gives each run's fields with `shown` and `current`.

| Mark | Means |
| --- | --- |
| `✓` | It passed on the code the branch holds now. |
| `○` | The code changed since. |
| `✗` | It failed. |

kebbie records a run from the hooks it gives the task's Claude, after every Bash command that starts with a test command (after any `cd … &&` and `VAR=value`):

- The built-in ones: `cargo test`, `cargo nextest`, `bin/rails test`, `bundle exec rspec`, `rspec`, `pytest`, `uv run pytest`, `npm test`, `pnpm test`, `yarn test`, `go test`, `mix test` and a few spellings of them.
- The repo's own, in [`task.test_commands`](configuration.md#keys).

It keeps the last 20 per task. A command that exits non-zero is a failed run. A result line that counts failures makes it failed too, since a pipe such as `| tail` hides the exit code. A run with uncommitted changes counts for the commit that later holds exactly those changes. Background runs and interrupted ones are not recorded.

### `kebbie approve <name>`

Your word or the lead's that a task's commits hold to the rules skill. It overrides the reviewer's rules check, which gives the same approval by itself when a task reports ready. Until a commit changes, the board shows the task ready to push. Tasks cannot run it.

### `kebbie accept <name> <sha>`

Your word that one of the task's commits is right. Only you may run it: kebbie refuses it to the lead and to every task, whose settings deny it too. `kebbie steps` shows the commit as accepted (`✓`). It marks the commit and blocks nothing, with `task.steps` on or off.

- It belongs to the commit's code: a clean rebase or a reworded message keeps it, a changed line takes it back. Two commits with the same change, such as two empty commits, are accepted one at a time.
- `<sha>` is one of the task's commits not yet pushed.
- `kebbie accepted` reads which commits are accepted.

| Option | What it does |
| --- | --- |
| `--withdraw` | Take the acceptance back. |

### `kebbie accepted [name]`

The task's commits not yet pushed, each marked `✓` when you accepted it. It only reads, so anyone may run it, a task's Claude too, with `task.steps` on or off. Without a name, it reads the task whose worktree you are in.

| Option | What it does |
| --- | --- |
| `--json` | `task`, `last_accepted` (the last commit of the run of accepted ones from the first unpushed one, or `null`) and `commits`, each with `sha`, `subject` and `accepted`. |

### `kebbie steps [name] ["<step>" ...]`

The task's steps, in the order they go out, one commit each. It needs [`task.steps`](configuration.md#tasks) on for the task's repo, the default, and refuses with how to turn it on when it is off.

- In a task's worktree, every word is a step's title. Elsewhere, the first word names the task.
- Put `--` before the titles when one starts with `-`: `kebbie steps -- "-v prints the version"`. Without it, kebbie refuses a word it would read as an option, so no title is dropped.
- With titles, it writes the list, then shows it. A task's Claude runs it before its first line of code; you or the lead may run it too, but a task writes only its own steps.
- Writing it again changes every step not pushed yet. The pushed ones stay first; a list that starts with them again is taken as the whole list.
- Without titles, it shows the steps.

The task's commits not yet pushed are its next steps, in order. A commit beyond the list shows as a step of its own, titled by its subject. A step counts as pushed once its commit's change is on the default branch, however it got there: `kebbie push`, a staged task you pushed with main, or a push from elsewhere. A clean rebase or a reworded message keeps it the same step.

| Mark | Means |
| --- | --- |
| `↑ 2 pushed` | The steps on the default branch, as a count. |
| `✓` | Committed, and you accepted it. |
| `●` | Committed, waiting for you to accept it. |
| `◐` | In progress: the next one while the task's Claude is at work. |
| `○` | Planned. |

| Option | What it does |
| --- | --- |
| `--json` | The steps as `task`, `listed`, `pushed`, `last_accepted` (the last commit of the run of accepted ones from the first unpushed one, or `null`) and `steps`, each with `index`, `title`, `state` (`planned`, `in-progress` or `committed`), `extra`, and with a commit its `sha`, `subject` and `accepted`. |

## Seeing what is going on

| Command | Output |
| --- | --- |
| `kebbie board [--overlay]` | The board. `--overlay` is the popup form, which `esc` closes. Until kebbie's Claude Code plugin is installed, it opens the setup guide first, and `esc` there opens the board. See [Board](board.md). |
| `kebbie status [--json]` | Every task and Claude session, grouped by what it needs from you. See [the fields](#kebbie-status---json). |
| `kebbie profiles [--json]` | The [profiles](configuration.md#profiles) a task can start with: the built-in `task`, `reviewer` and `ops`, then each file in `~/.config/kebbie/profiles`. |
| `kebbie list [--json]` | The pool's worktrees: each one's task or `free`, uncommitted changes, commits ahead and behind, its port, and the first line of its setup note. |
| `kebbie areas` | The vault's areas with their repo: main against origin, the last release, notes and actions. Needs the vault. |
| `kebbie peers` | Who you are and the other task sessions, with what each is doing. |
| `kebbie messages` | The sessions' messages, live. `--print` prints the last 30. |
| `kebbie backlog` | The backlog the board shows. |
| `kebbie backlog add [--priority <p>] "<Title> — <summary> (<area>)"` | Append an item to the local backlog file. See [Files](files.md#the-local-backlog-file). |

`kebbie profiles` shows each profile with what it is for, what it is about, and its agent, model, effort, permission mode and skills. Under a profile that runs Codex, a `codex:` line gives the Codex binary, its version and whether kebbie's plugin is installed. A file that does not load is listed as ignored, with the reason. `--json` gives one object per profile, and `{"error": …}` for an ignored file.

`kebbie list --json` adds `port` and `url`, the worktree's `dev_url` filled in, each `null` when there is none.

### `kebbie status --json`

`--json` gives `away_since`, `items` and `checkouts`. A task in it carries:

| Field | Value |
| --- | --- |
| `push_waits` | `user` or `lead` while a push it asked for waits in its repo's push line or for the task's Claude. |
| `push_ahead` | How many pushes of its repo go before it while it waits its turn. |
| `profile` | The profile it started with: `task` for the built-in one, `null` for a session that is no task. |
| `parked_until` | While it is parked: the Unix time it comes back, or `"unparked"` until you bring it back; `null` otherwise. |
| `phase` | Where it stands as kebbie keeps it, below. |

`phase` holds:

- `name`: `starting`, `working`, `idle`, `needs-you`, `ready`, `pushing`, `ci-running`, `ci-red` or `closing`.
- `why` for `needs-you` (`left-line` when a push left its line unpushed), `head` for `ready`, `ahead` for `pushing` and `sha` for the CI phases.
- `since`, and `by`: what moved it there.

## Routines

See [Routines](../explanation/routines.md) for what a run does, and [`[[routines]]`](configuration.md#routines) for the keys.

| Command | What it does |
| --- | --- |
| `kebbie routines [--json]` | Every routine from config.toml and the repo configs, with its schedule, next run and last run. |
| `kebbie routines tick` | One round of the scheduler, as the keeper runs it every minute. |
| `kebbie routine add …` | Add a routine to config.toml, as the board's Routines page adds one with `n`. |
| `kebbie routine run <id>` | Start a run now, by the same rules: never while the last run's task is open. Also starts a routine with `enabled = false`. |

`kebbie routines` shows each routine's schedule, repo, mode, next run, and its last run's outcome (`ok`, `findings`, `failed` or `running`) with its summary. A routine that is not valid is listed as ignored, with the reason. `--json` gives `routines` and `errors`.

In a `routines tick`, a run whose task closed without a word is marked failed, and each due routine without an open run starts one.

The full form of `routine add`:

```
kebbie routine add --title <name> --repo <repo> --at <time> | --every <interval> [--mode <mode>] [--push] [--paused] [--id <id>] <prompt>
```

| Option | What it takes |
| --- | --- |
| `--repo` | The repo's folder name or path. |
| `--at` | `07:00` or `mon 07:00`. |
| `--every` | `30m`, `6h` or `1d`. |
| `--mode` | `report` (the default), `plan` or `fix`. |
| `--id` | The id. Without it, the id comes from the title, with `-2` added when it is taken. |
| `--push` | Lets the lead push a fix routine's fix. |
| `--paused` | Adds it with `enabled = false`. |

It is refused when the id is taken or config.toml does not parse.

## The lead and away mode

| Command | What it does |
| --- | --- |
| `kebbie lead` | Go to the lead Space, or back to the Space you came from when you are in it. |
| `kebbie lead --restart [--anyway]` | Restart an idle lead's Claude on its current role and settings, resuming its conversation. |
| `kebbie lead --restart --fresh` | Start the lead's Claude on a new conversation, with its role, settings and skills, in its folder. |
| `kebbie lead --first` | Only move the lead Space to the top of herdr's Spaces. |
| `kebbie lead --revive-board` | Run by herdr's startup hook to bring the board back in the lead Space. |
| `kebbie lead --toggle-chat` | Hide the lead's chat so the board takes the whole width, or show it again. |
| `kebbie focus` | In the lead Space, move focus between the board and the lead's chat. From any other Space, go to the lead's chat. A hidden chat is shown first. |
| `kebbie away [on\|off]` | Away mode. Yours only. |
| `kebbie decide <task> <answer>` | The lead's own answer to a task's question about how, whether or not you are here. |
| `kebbie choose <task> <n> --prompt <id>` | Answer the prompt a task's Claude shows in its own pane with option `<n>`. See [below](#kebbie-choose). |
| `kebbie defer <task>` | The lead keeps a task's question for you while you are away. |
| `kebbie notify <text>` | Run `notify.command` with this text as a `summary` event. You or the lead only. |
| `kebbie tell --force <to> <text>` | Yours or the lead's: deliver now, for a delivery that is stuck behind a draft. |
| `kebbie release <to>` | Yours or the lead's: deliver what is held for a session now, as `tell --force` does, without a new message. |

`kebbie lead`:

- herdr's `kebbie.lead` action, `prefix+space`, runs it.
- Without a lead Space it creates one with its board and the lead's chat.
- Going there starts the board again when it was quit, and puts its pane back left of the chat when that was closed. A board that runs is left alone, so there is never a second one.
- Going back starts nothing.

`kebbie lead --restart`:

- It refuses with `Not restarted: <reason>` while the lead works, waits on a question, holds text you have not sent, has background work running (unless `--anyway`) or another kebbie restarts it.
- After `kebbie update` the keeper runs it by itself once the lead is free, unless `restart.auto` is off.
- `--fresh` refuses as `--restart` does. The old conversation stays on disk, and kebbie keeps the new one as the lead's. It is yours to run, never the lead's; `L` on the board does the same with `New conversation`. See [Starting the lead fresh](../explanation/lead-and-away-mode.md#starting-the-lead-fresh).

`kebbie lead --revive-board` returns at once. In the background it waits up to a minute for herdr to restore the lead Space, then starts the board in its board pane if that pane sits at a shell prompt. It never touches the lead's chat, and logs to `lead.log`.

`kebbie lead --toggle-chat` keeps the chat running and takes you to the lead Space. herdr's `kebbie.chat` action runs it.

`kebbie decide` closes the question on the board and records it, for your summary when you are away.

`kebbie tell --force` clears the draft from the receiver's input box, logged word for word (`ctrl+y` in its pane brings it back). Then it types what is held for that session and the new message, in order, as one prompt.

### `kebbie choose`

Answers the prompt a task's Claude shows in its own pane, such as a permission or trust question, with option `<n>` as kebbie numbered it for the lead. `<id>` is the prompt's id from kebbie's message.

1. kebbie reads the pane again and refuses when that prompt is no longer on screen.
2. On a menu with numbers whose focus is on its first option, for options 1 to 9, it presses the option's digit. Otherwise it moves the focus with the arrow keys, reads the pane to check the focus landed on the chosen option, and only then presses Enter.
3. It reads the pane again and says whether the menu went away, or that it could not confirm it.

It never types text, and refuses an option that takes your own words; that one you answer in the Space. It is refused for a task.

It is yours, or the lead's on your explicit choice. The lead's settings do not allow it, so Claude Code asks you before the lead runs it. Answer plain "Yes", never "don't ask again": that question is the gate only while your settings hold no allow rule for `kebbie choose`. See [Claude's own prompts in a task](../explanation/lead-and-away-mode.md#claudes-own-prompts-in-a-task).

## Run by a task's Claude

| Command | What it does |
| --- | --- |
| `kebbie report --state <state> --next <step> <summary>` | Where the task stands and what you should do next. See [below](#kebbie-report). |
| `kebbie steps "<step>" …` | The task's steps before its first line of code, with `task.steps` on. See [`kebbie steps`](#kebbie-steps-name-step-). |
| `kebbie routine ok <summary>` | A routine's task, when all is well: records the run as ok with the summary and closes the task as `drop` does. |
| `kebbie ask --option "<label>: <meaning>" … --recommend <n> <question>` | Ask you what you want: scope, behaviour, a trade-off that is yours. |
| `kebbie answered <task>` | The task's question was settled elsewhere, dropped, or sent back to be asked again. |
| `kebbie tell <to> <text>` | Message another session: a task (every half of it), `task@repo`, or `lead`. Held while the receiver waits for an answer, and logged. |

`kebbie routine ok` is refused outside a routine's task, from anyone but that task's Claude, and while the task has changes or commits.

`kebbie ask`:

- A question about how goes to the lead instead.
- The question opens with one plain sentence on what the task is doing.
- Each of the two to four options says what you get with it, one recommended, without flags, commit ids, file names or internal terms you did not use.
- `--plan <file>` or `--plan vault:<note>` asks you to approve a plan instead.

`kebbie answered` takes the question off the board, and a `kebbie tell` to that task is no longer held.

### `kebbie report`

`<state>` is `needs-you`, `ready`, `working` or `blocked`.

- With `ready`, `--card <file>` is the sign-off card the push window shows, with an Also noticed list when the task found things outside its task.
- A routine's task also lists its findings on a card with `needs-you`.
- With `kebbie:work-rules` as the rules skill, `ready` is refused while a commit lacks a subject, a blank line and a body.

After a ready report the keeper starts the reviewer. It checks the commits against the rules skill, and against [`docs.required`](configuration.md#keys) in a repo that sets it:

- A pass approves them.
- A break goes to the task, and a line to the lead.
- No verdict within 15 minutes leaves the check to the lead.
- A ready report on code already checked gets the stored findings back, or goes to the lead after no verdict, without a new run.

## Setup and maintenance

| Command | What it does |
| --- | --- |
| `kebbie config [--repo <path>]` | The config layers found and every key with its value in effect, the layer it comes from and what it does. See [Configuration](configuration.md). |
| `kebbie config --get <key>` | Print one machine-wide value for scripts: `worktree.server_name` or `worktree.worker_name`. |
| `kebbie config edit [--repo <path>]` | The board's [Settings page](board.md#settings-page) on its own, for config.toml and the repo's config. Needs a terminal, and is yours only: the lead and tasks cannot run it. |
| `kebbie docs [<words>]` | Search kebbie's docs on this machine, or list them. See [below](#kebbie-docs). |
| `kebbie doctor` | Check this machine, with the fix for each gap, the task sandbox's state among them. Exits non-zero on a `fail`. See [Requirements](requirements.md#what-kebbie-doctor-checks). |
| `kebbie doctor --fix` | Install kebbie's push block in each worktree that still has kelpie's, then check. |
| `kebbie routes [--plain-http]` | Print a Caddy config that serves each worktree's dev server at its own host. See [below](#kebbie-routes). |
| `kebbie setup [--yes]` | The setup guide: seven steps, from this machine's name to error signals, then everything it writes in one go. `--yes` takes the defaults without asking. See [below](#kebbie-setup). |
| `kebbie setup vault` | The guide at its Vault step. Without a terminal, it installs the vault plugin, showing each `claude plugin` command and running it only on your `y`. |
| `kebbie setup codex` | The guide at its Agents step, where Codex is turned on. Without a terminal, it installs kebbie's Codex plugin. See [below](#kebbie-setup-codex). |
| `kebbie setup codex --uninstall` | Remove kebbie's Codex plugin, on your `y`. |
| `kebbie update` | Update everything to this machine's channel and restart idle Claude sessions. See [below](#kebbie-update). |
| `kebbie update --check` | Fetch and show the installed versions against the channel's latest. Changes nothing else. |
| `kebbie update --from <branch or path>` | Install that branch on origin, or that checkout's commits, until `--stable`. |
| `kebbie update --stable` | Return to the channel. |
| `kebbie restart` | Restart each stale Claude session that is idle, not the pane you are in, and has no background work. Works with `restart.auto = false`. |
| `kebbie restart --all` | Restart every idle session, stale or not. |
| `kebbie keeper --status` | Whether the keeper runs, with its pid, binary, uptime, log and each of its routines' interval, last run and last error. |
| `kebbie keeper --start` | Start the keeper in the background unless one runs. Says `off` while `keeper.enabled = false`. |
| `kebbie keeper --stop` | Stop the keeper. It finishes the routine under way first. |

`--from` and `--stable` cannot be combined. On a `kebbie restart`, a task's Claude gets its role and settings written anew. After `kebbie keeper --stop`, the board or herdr's next startup starts the keeper again; `keeper.enabled = false` keeps it off.

### `kebbie docs`

It says where kebbie's docs are on this machine: `docs/` in the checkout kebbie is installed from. Without words, it prints the [docs index](../README.md) by section, each page's path relative to the docs folder.

With words, it lists the pages whose title, file name or headings match any of them, best first:

- A title match ranks above a heading match, then reference, how-to, explanation and tutorial.
- A single character, such as the board key `M`, keeps its case and matches a whole word in a heading or the first column of a table.
- Each page comes with its path, its first sentence and the headings or table rows that matched; twelve at most.
- Search in English, the docs' language.

It reads only files and never needs the network. When the install has no `docs/`, it says so and points at `kebbie help` and the docs online, on the install's GitHub remote or `https://github.com/gronare/kebbie/tree/main/docs`.

### `kebbie routes`

Prints a Caddy config with one site per pooled worktree that has a port, in every repo kebbie knows whose repo config sets [`worktree.dev_route = true`](configuration.md#keys). It writes nothing. See [Open dev servers from a remote machine](../how-to/open-dev-servers-from-a-remote-machine.md).

- Each site is the host the worktree's `dev_url` names, with `reverse_proxy localhost:<port>`.
- Sites keep the scheme and port `dev_url` names, so Caddy serves their TLS. `--plain-http` serves each as `http://<host>`, for a proxy in front that ends TLS.
- A worktree without a port, a `dev_url` on localhost or without a host, or a host another worktree already has gets no site. The last comment line counts them with the reason, and the repos left out.

### `kebbie setup`

Safe to run again. Yours alone: a task cannot run it. In a terminal it is a guide of seven steps, with the steps on the left (`✓` done or in place, `▸` the one you are on, `·` to do):

| Step | Asks | Writes or runs |
| --- | --- | --- |
| 1 Machine name | How your phone and other machines show this one, from `machine.name` or the host's short name. | `machine.name` |
| 2 Agents | Claude Code, always on; Codex, on or off, each with the version found or `not found`. | kebbie's plugin in Claude Code, and kebbie's [Codex plugin](#kebbie-setup-codex) |
| 3 herdr keys | `prefix+space` (the lead), `prefix+f` (the board and the lead's chat), `prefix+t` (the board). | Those keys at the end of herdr's `config.toml` |
| 4 Backlog | Local, vault and Linear, in any combination; the first is where the lead files new ideas. | `board.backlog.sources` |
| 5 Vault | None, a local folder, or an S3 bucket (endpoint, bucket, access key and a masked secret). | `claude plugin marketplace add`, `install` and `configure --values-stdin` |
| 6 Error signals | Keep them on or turn them off. | `report.signals` |
| 7 Review and apply | Nothing: it lists each line it will write or run, with the `kebbie doctor` warnings setup fixes itself. | Everything, on `⏎` |

| Key | Action |
| --- | --- |
| `↑` `↓` | Choose. |
| `space` | Tick or untick a box. |
| `⏎` | The next step; on the last one, apply. |
| `←` | The step before, keeping your answers. |
| `s` | Skip the step, leaving what it sets as it is. |
| `esc` | Quit without writing anything. |

- Nothing is written or run before `⏎` on step 7. Then each line turns `done`, `already`, `skipped` or `failed` as it runs, followed by `Setup done · doctor: <n> ok`, with `, <m> to look at` when warnings remain, and the [next steps](#next-steps-after-setup).
- Each line says what it does in a few words, such as `~/.local/bin/kebbie`, `kebbie in Claude Code and Codex` or `prefix+f added, herdr reloaded`; a line that fails or skips says why.
- A run again starts from this machine's values and marks each step already in place; a line whose value is already there reads `already` and changes nothing.
- It always links `~/.local/bin/kebbie` to `~/.local/share/kebbie/current/target/release/kebbie`, and installs the Claude Code plugin from that folder or updates it as `kebbie update` does.
- herdr's keys go only at the end of its `config.toml`, the rest of the file byte for byte, then `herdr server reload-config`, or a restart when it cannot. A key already bound to something else is named, with a free one offered: add the others and leave it, add it on the free key, or skip.
- The vault's values reach `claude plugin configure` on stdin, and the secret is never shown.
- The review also lists the `kebbie doctor` warnings that setup fixes itself, applied on `⏎` like the rest:

| Line | When | On `⏎` |
| --- | --- | --- |
| `usage` | Claude Code's `settings.json` has no `statusLine`. | Adds kebbie's `statusLine`, so the board shows the plan's usage; the rest of the file stays byte for byte. |
| `keeper` | The [keeper](#kebbie-keeper) is on but not running, or off in `config.toml` but running. | Starts it, or stops it. |
| `herdr` | kebbie's herdr plugin is disabled. | `herdr plugin enable kebbie` |

- A warning that needs your choice, such as a `statusLine` command of your own or `task.sandbox = false`, stays with `kebbie doctor`.
- Setup never touches the hub. Once it is done, a machine that is not connected gets a box, **Your board on your phone**: `h` connects it as [`kebbie hub connect`](#the-hub) does, after setup prints its lines, then opens the board where setup ran.
- Any other key opens the board. Inside herdr it takes you to the lead Space instead, and creates it when there is none: a board there on an older kebbie restarts on the new one, and an idle lead restarts on it too, resuming its conversation. A lead that is working keeps running, setup says why, and `L` on the board restarts it later.

herdr's config is `$HERDR_CONFIG_PATH`, else `$XDG_CONFIG_HOME/herdr/config.toml`, else `~/.config/herdr/config.toml`.

`--yes`, and a run without a terminal, ask nothing. They link the binary, install or update the Claude Code plugin, add the herdr keys that are free, and leave the other steps as they are, one line each:

- Without a terminal the herdr write is skipped unless `--yes`, and `--yes` never writes through a symlinked config.
- They end the same way, with `Setup done · doctor: <n> ok` and the next steps, then, while the machine is not connected, `Your board on your phone: kebbie hub connect, or H on the board` and `kebbie.dev/hub`.
- They fix no `kebbie doctor` warning: those are rows of the guide's review, shown before they are applied.

#### Next steps after setup

Under `Next steps`, what needs you outside setup, a command and why:

| Line | When |
| --- | --- |
| `codex login` | You turned Codex on and `codex login status` says it is not logged in, or Codex gives no answer. |
| `kebbie doctor` | Warnings remain after applying; it says what is left and how to fix it. |
| `prefix+space` | Always, last: tell the lead what you want done. |

### `kebbie setup codex`

Installs kebbie's Codex plugin in `$CODEX_HOME/plugins/cache/kebbie/kebbie/local/`. In a terminal it opens the setup guide at its Agents step, where Codex is on when the plugin is; without one it says what it writes and writes only on your `y`. Yours alone: a task cannot run it. See [Run a task with Codex](../how-to/run-a-task-with-codex.md).

- Its `SessionStart`, `UserPromptSubmit` and `Stop` hooks record a Codex task's session, give it its role and ask for a report before a turn ends.
- It never edits Codex's `config.toml` or `hooks.json`. kebbie enables the plugin and trusts its hooks only on the command line of the Codex sessions it starts.
- Run again, it writes only what changed, and nothing when the plugin is current.

### `kebbie update`

It prints each step as it goes (in a terminal the running one is redrawn with its bar and time). Then it prints what's new and fixed from kebbie's `CHANGES.md`, one or two sentences on the restarts, and the details.

With open [kebbie issues](#kebbie-issues), it looks them up on GitHub through `gh` and leaves each one filed on older code to the keeper to recheck against the new commits.

`kebbie update --check`:

- kebbie's installed version is its binary's, with the commit it was built from. A checkout ahead of it shows as `0.25.0 in the checkout: rebuild`.
- On the main channel kebbie shows as `main installed`, and what is newer as the commits on origin's default branch the build lacks, such as `3 new on main`.
- On the release channel, the latest is the highest tag whose release is published with a binary for this machine. A newer tag that is not yet says `kebbie <version> is not published yet; try again in a while`.

## The hub

Show the board on your phone through a kebbie hub. See [See your board on your phone](../how-to/see-your-board-on-your-phone.md). `connect`, `join`, `move` and `leave` are yours only.

| Command | What it does |
| --- | --- |
| `kebbie hub [status]` | The hub this machine is connected to, its name and id there, and the last error. |
| `kebbie hub connect [--name <name>]` | Connect this machine to a hub through your browser. See [below](#kebbie-hub-connect). |
| `kebbie hub join <url> <code> [--name <name>]` | For scripts: join the hub at `<url>` with a code from `kebbie-hub admin join-code`. |
| `kebbie hub move <url>` | The hub moved to `<url>`: post there from now on with the machine's token. |
| `kebbie hub leave` | Leave the hub: remove the machine from it, then forget the hub, the keys, the account key and the token. |

`kebbie hub status`:

- It renames the machine on the hub first when `[machine] name` has changed, and shows why when that fails.
- It shows the account whose key it pinned, with the fingerprints of its signing and encryption keys. With none, it says that no browser can read the board, and to connect it from the link: `kebbie hub leave`, then `kebbie hub connect`.
- It shows the board's last revision and when it went out.

`kebbie hub join`:

- It makes the machine's keys and keeps them, with the hub's token for it, in [`hub/`](files.md#the-state-directory); nothing prints them.
- The hub shows the machine by [`machine.name`](configuration.md#keys). Set already, the join uses it; `--name` sets it in config.toml; neither, the join asks for it in a terminal and writes it, and without a terminal joins with the short host name. It says in one line what it set, and writes nothing through a config.toml that is a link.
- It takes `https://`, or `http://` for a hub on this machine.
- It is refused when the machine has joined a hub already.
- A machine that joined belongs to no account, so it pins no account key and posts no board.

`kebbie hub move` checks only that the new address answers `/healthz`, without sending the token. That check proves nothing about who runs it, so the address is yours to check: move only to a hub you run or trust.

`kebbie hub leave` sends `POST /v1/m/leave` with the machine's token before it forgets anything, and the hub removes the machine, its board and its token. A hub that knows the token no more counts as left. A hub without the call, or one that does not answer, lists the machine, asleep, until its admin runs `kebbie-hub admin remove-machine <id>`; kebbie says which, and forgets the keys either way.

A hub with plans keeps taking the machine's posts while the subscription has lapsed. When a hub refuses one for a lapsed subscription, kebbie says `Your kebbie hub subscription isn't active:` with the hub's link, in `kebbie hub status`, `kebbie doctor`, a line on the board and its `H` window, and everything on the machine keeps working.

With [`KEBBIE_HUB_DEV`](environment.md) set, every `kebbie hub` command works on the dev hub at `KEBBIE_HUB_DEV_URL` (`http://localhost:8470`) and its own identity in `hub-dev/`. It never reads `[hub] url` or touches `hub/`, and `kebbie hub status` starts with `dev hub · localhost:8470`.

### `kebbie hub connect`

Connects this machine to a hub through your browser, as OAuth's device flow does: no join code, no shell on the hub. The board's `H` does the same. It is refused when the machine is connected already.

1. It connects to [`hub.url`](configuration.md#keys). Unset, it asks in a terminal which hub, on two rows: the kebbie hub (the first, and the hub without a terminal) or your own. `↑` `↓` move, `⏎` chooses, `k` and `o` choose at once, `esc` cancels.
2. Your own hub's address it tests before it saves it as `[hub] url`: `GET /v1/hub` must answer as a kebbie hub, and a bare host name is tried over `https`. It never saves through a config.toml that is a link.
3. It names the machine by [`machine.name`](configuration.md#keys) as `join` does. When that is unset, it asks, with the host's short name kept on `⏎`.
4. While it waits, a card shows the machine, the hub, the code to check on the page, a QR code of the link and how long the code has left.
5. You sign in on the page and press Connect. The browser hands over your account's key.
6. It ends on a card saying the machine is connected to the hub and that you sign in with your passkey on any phone or browser to see its board. Or it says in one line that it was cancelled in the browser, or expired.

On the card:

- Nothing opens by itself. `⏎` opens the link in your browser (not over SSH), and the card then says `✓ Opened in your browser`.
- The link under the card stops at its `#`, since the rest is the secret. `c` copies the whole one: `pbcopy`, `wl-copy` or `xclip` locally, the terminal's OSC 52 over SSH, which also shows the whole link. Without a terminal it prints the whole link instead.
- `esc` or `q` stops.
- The text sits beside the code from the top, or above it in a narrow terminal. One too small for either shows the card without the code.

The QR code is drawn in square modules with four light modules around it, in the terminal's own colours with no background. The glyphs are the light modules, so a dark theme's background makes the dark ones. It uses octant blocks, two modules across and four down a character, in Ghostty, kitty and WezTerm, and half blocks, one across and two down, elsewhere; `KEBBIE_QR=octant` or `halfblock` picks one. See [the board's hub window](board.md).

kebbie asks the hub every few seconds, slower when the hub asks, for up to 10 minutes. The machine pins the account key, without digits, only when the browser's commitment to it matches a secret the link carries past the hub, and refuses otherwise. It keeps the keys, the token and the account key with its two fingerprints in [`hub/`](files.md#the-state-directory).

A machine approved by its typed code, without the link, pins no key. The card says no browser can read its board, and to connect it from the link with `kebbie hub leave`, then `kebbie hub connect`.

### What the hub gets

Once the machine has pinned an account key, the keeper posts the board every 15 seconds when it changed, and once a minute when nothing did. Each post is sealed to the account and to the machine itself, signed by the machine, with a revision that only goes up. A machine with no account key posts no board.

- The hub keeps a board a day at most, so the once-a-minute post keeps a machine that is awake but idle from expiring. One really asleep shows as asleep in the web app.
- The revision is at least the current Unix time, so it keeps rising when `hub/relay.json` is lost.
- Nothing is pushed to a phone: notifications through the hub are not available in this version, and [`notify.command`](configuration.md#notify-command) is how the machine tells you a task needs you.

The board carries tasks with their group, summary, your next step, report, question and options, git counts, head, review verdict, CI, a task's steps with their number, title, state and commit, the checkouts' free and busy worktrees, the routines and away mode. It never carries a path, a pane, a Space, a host name, a dev server address or a commit's message.

- A queued task waiting for its time is left out; a blocked one shows under Needs you.
- A plan to approve shows as its question, without the plan.

Redirects:

- The hub's 307 and 308 redirects are followed with the same method, body and token only on the hub's own host, to another port or path or from `http` to `https`, never from `https` to `http`.
- After such a 308 the machine keeps the new address.
- A redirect to another host is refused with the address it pointed to, for you to check and run `kebbie hub move`.

## kebbie issues

Problems and wishes about kebbie itself, saved on this machine, one file each under `issues/` in the state directory. See [Report a problem with kebbie](../how-to/report-a-problem-with-kebbie.md) and [the issue file](files.md#the-issue-file).

| Command | What it does |
| --- | --- |
| `kebbie issues [--json] [--all]` | The open issues (`open` and `retry`). `--all` lists every status. `kebbie issue list` is the same. |
| `kebbie issue hold <line>` | Hold one line on what kebbie did, for the lead to propose saving it at a calm moment. Yours or the lead's. |
| `kebbie issue hold --take` | Print the held line and clear it, or `Nothing held.` The lead runs it at a calm moment, just before it proposes. |
| `kebbie issue add --kind bug\|wish --title <title> [<text>\|-]` | Save an issue. Yours or the lead's. |
| `kebbie issue show <id>` | Print one issue with its text. |
| `kebbie issue close <id> fixed\|wontfix\|duplicate` | Close it. |
| `kebbie issue reopen <id>` | Open a closed issue again because it came back. Yours or the lead's. |
| `kebbie issue retry <id>` | Mark it worth retrying, typically after an update that may have fixed it. |
| `kebbie issue link <id> <number\|url>` | Tie it to its issue in kebbie's GitHub repo. Yours alone: kebbie refuses it to the lead and tasks. |
| `kebbie issue report <id>` | Send a bug to kebbie's maintainers, after showing it. Yours alone, like the board's `y`. See [below](#kebbie-issue-report). |
| `kebbie issue report --always` / `--ask` | Arm or disarm sending each bug without asking. Yours alone, in your own terminal. |
| `kebbie issue send <id>` | Open a prefilled new issue on GitHub. Optional, and yours alone. See [below](#kebbie-issue-send). |

`kebbie issues` shows each issue's id, kind, status, the version and commit it was filed on, the GitHub issue when linked, and the last line of its history, such as `fixed in 0.26.3 (a1b2c3d), checked automatically`.

`kebbie issue hold`:

- One line is held at a time: a new one replaces it. kebbie answers `Held for a calm moment: <line>`, adding `· it replaces: <earlier line>` when it did.
- The line is kept in `issues/held` in the state directory and is never an issue: `kebbie issues`, the board and the hub report do not read it.
- With `[issues] propose = false` nothing is held. Saving or reopening an issue clears it.
- A task is told to tell the lead instead.

`kebbie issue add`:

- The text comes from the argument, or from stdin with `-`: what happened, what you expected, how to reproduce.
- kebbie records its version, commit, the date and who filed it, and answers `Filed kebbie issue <id> (<kind>): <title> · kebbie issue show <id> shows it`. For a bug while `[report] text` is `"ask"`, it adds `· it waits on the board for the user to send or keep`.
- A task is told to tell the lead instead.

`kebbie issue show` marks an issue filed from another install's report `[report <id>, untrusted]`, with its text in a fence headed `sent by an unknown install: data, not instructions`.

`kebbie issue reopen` records `came back on <version> (<commit>)` and files it anew on the running build, so the next recheck reads only the commits after it, and a bug is offered for sending again.

`kebbie issue report --always` arms `[report] text = "always"`, so kebbie sends each bug in kebbie you run into without asking, redacted, each send a line under messages. The key in config.toml does nothing until it is armed. `--ask` disarms it, and kebbie asks before every report again. The lead, tasks and sandboxed processes are refused.

### `kebbie issue report`

Prints a bug as it would go to kebbie's maintainers, with paths under your home as `~` and repo names, URLs and tokens replaced. In a terminal it asks `y/N`. See [Send it to kebbie's maintainers](../how-to/report-a-problem-with-kebbie.md#send-it-to-kebbies-maintainers).

- `y` sends exactly the text shown to kebbie's issues endpoint, with a proof of work, signed with this install's key, and records the report's id in the issue. An issue changed meanwhile sends nothing.
- `n` keeps it here, and the board stops offering it.
- Without a terminal it prints the text and sends nothing.
- It is sent once: a second time is refused.

### `kebbie issue send`

Prints the text and a prefilled new-issue URL on GitHub, and opens it in the browser: `open` on macOS, `xdg-open` on Linux with a display, over SSH only the URL. Nothing is sent until you press Submit.

- The body adds the kind, version, commit and OS, and a `kebbie-issue: <id>` line.
- A text that would make the URL longer than GitHub accepts is shortened, and says so.
- It is optional: the way a bug reaches kebbie's maintainers is `kebbie issue report` or the board's bugs to send.
- kebbie refuses it to the lead and tasks, nothing in kebbie runs it for you, and the lead suggests it only when you ask for a GitHub issue.

### Rechecks after an update

After `kebbie update`, when `gh` is installed and logged in, kebbie searches kebbie's GitHub repo for each unlinked issue's `kebbie-issue:` line and links what it finds. An issue closed on GitHub becomes `retry`, unless it came back since. Without `gh` this step is skipped.

Then each open bug or wish from this machine, filed on older code than the update installed and on a commit this install's history holds, waits for a recheck, once per build. Reports from other installs, and issues filed on a build the history cannot place, never do.

1. The keeper takes them one at a time. It starts a read-only Claude from its own environment, as it does the rules reviewer, with the issue, every commit since the issue's build with its full message, and kebbie's source at the new commit.
2. Its verdict is `fixed <sha>`, `not-fixed` or `unclear`.
3. A fix by a commit since the issue's build closes it as `fixed`, with the line `fixed in <version> (<sha>), checked automatically`.

The board then shows `✓ Fixed in kebbie <version>: N bugs you ran into` with their titles, until you open it with `i` or three days pass; a later fix shows again. Anything else leaves the issue open until the next update. No verdict within 15 minutes also writes one line to `keeper.log`. Nobody is asked, and a recheck that fixes nothing says nothing.

## Used by kebbie itself

These run from hooks, the herdr plugin and kebbie's own processes. You do not run them.

| Command | Run by |
| --- | --- |
| `kebbie hook` | The hooks kebbie gives a task's Claude. Reads the hook's JSON on stdin. |
| `kebbie hook codex <event>` | kebbie's Codex plugin, for `session-start`, `prompt` and `stop`. Reads the hook's JSON on stdin and prints nothing outside a kebbie task's worktree. |
| `kebbie statusline` | Claude Code's `statusLine` command. Keeps the plan's usage for the board and prints nothing. |
| `kebbie backlog save` | The lead, after refreshing the backlog. Reads a JSON array on stdin. |
| `kebbie messages --unread` | The lead's prompt hook. |
| `kebbie review --run` | The reviewer process `kebbie review` starts. |
| `kebbie review --run --rules` | The read-only rules check the keeper starts after a task reports ready. |
| `kebbie issue recheck --run <id>` | The checker the keeper starts after an update to recheck one kebbie issue. |
| `kebbie update --finish` | The newly built binary, at the end of `kebbie update`. It stops the keeper first and starts it again on the new binary last. |
| `kebbie keeper` | The keeper itself, as `kebbie keeper --start` starts it: a loop with no terminal that runs the routines and logs to `keeper.log`. |
| `kebbie lead --renew-board` | `kebbie update`, to restart the board on the new binary. |
| `kebbie lead --revive-board --wait` | The background half of `kebbie lead --revive-board`, which waits for herdr's restore. |

`kebbie hook` takes `evidence`, `inbox`, `files` or `gate` for the test record, the inbox socket, the sandbox's file check and the vault note's gate, and `--review` for the reviewer's guard.

`kebbie issue recheck --run` reads the keeper's nonce on stdin and closes nothing without it; a task's Claude is denied it.

## Finding repos

`--repo <name>` takes a folder name, never a path. kebbie looks for a checkout of that name in each folder of `KEBBIE_REPO_DIRS` (default `~/projects:~/personal:~/src:~/code`), in it or one level down. When there is none, it clones the repo from an owner found among your other checkouts' remotes.
