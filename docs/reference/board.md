# Board

What the board shows and every key on it. The board runs in the lead Space, as a popup over any Space with `prefix+t`, or in the current pane with `kebbie board`. `?` on the board lists every key.

A board you quit in the lead Space starts again the next time you press `prefix+space`, also when you are already in the lead Space; once the board runs, `prefix+space` takes you back to where you were.

## Layout

| Part | Shows |
| --- | --- |
| Status line, top | The page tabs, `1 Work  2 Areas  3 Routines  , Settings`, then counts and markers, below. |
| Lead line, under the status line | Whether the lead still runs an old kebbie, while it counts as stale. |
| Context line, under it | While the lead's context is 85% full or more: `The lead's context is N% full · L to restart it fresh`. |
| Hub line, under it | While the hub refuses the board because it has not enabled answering for your account: `This hub has not enabled answering for your account: <link> · H hub`. |
| Work, left | Tasks from every repo with a worktree pool, grouped, in the order of [`board.groups`](configuration.md). |
| Backlog, right | Vault notes in progress, under ONGOING, and the items from the configured sources, by priority. |
| Messages, bottom | What sessions tell each other, newest last. |

The status line shows:

| Marker | Means |
| --- | --- |
| `⚠N` on Routines | N routine runs wait for you with findings. |
| Counts | What needs you (tasks, and devices waiting for your approval), tasks that are working, are in review, are ready to push and wait for CI, and waiting questions. |
| `◆ N to accept`, yellow | N subtasks of tasks working in subtasks wait for you to accept them. |
| `! N bugs to send` | Bugs and wishes about kebbie, saved here, wait for you to send them to its maintainers, on their row under Work, such as `! 1 bug and 2 wishes to send`. |
| Plan usage | How much of the plan is used. |
| `↻ N stale` | Claude sessions or Codex tasks wait to be restarted. |
| `☾ away` | Away mode is on. |
| `chat hidden` | The lead's chat is hidden. |
| `↑ kebbie v0.28.0 · U updates` | A newer release is out. On the main channel, `↑ kebbie main, 3 new · U updates` with the commits the installed build lacks. |
| `⇣ updating kebbie · U` | An update whose window you hid runs. |
| `✓ kebbie updated · U` | The update's summary waits for you. |

In a narrow pane the two columns stack, and the status line keeps only the counts that are not zero, without their words, so the page tabs still fit.

The lead line shows while the lead is stale, and says what happens next:

| Line | When |
| --- | --- |
| `↻ The lead restarts on the new kebbie when it's idle · L restarts it now` | The keeper restarts it once it is idle; `when you leave it` while you are in its pane. |
| `↻ The lead runs the old kebbie until you restart it · L restarts it` | `restart.auto` is off. |
| `↻ The lead's restart on the new kebbie failed · L tries again` | A restart failed; the keeper tries again in 30 minutes. |
| `↻ A restart left the lead on the old kebbie · L tries again` | A restart onto the new kebbie left the lead on the old one. |
| `↻ The lead's shell starts the same older Claude Code, so a restart cannot update it · put the new one first on that shell's PATH` | A restart brought back the same older Claude Code. No `L`: fix the `PATH`. |
| `↻ The lead's hooks still run a kebbie that is gone after a restart · kebbie doctor shows the fix` | A restart left its hooks broken. No `L`: run `kebbie doctor`. |
| `↻ The lead stays stale after a restart and another would not mend it · kebbie restart says why` | Another reason a restart did not mend. No `L`. |

The line names why the lead is stale. `on the new kebbie` and `the old kebbie` mean a kebbie update; with several reasons, a kebbie update is named first:

| Reason | `The lead runs …` | `The lead restarts …`, `restart … failed` |
| --- | --- | --- |
| `kebbie update` ran | `the old kebbie` | `on the new kebbie` |
| A newer Claude Code | `an older Claude Code` | `on the new Claude Code` |
| A plugin was updated | `the old plugin vault` | `with the updated plugin vault` |
| The MCP servers changed | `MCP servers from before they changed` | `with the changed MCP servers` |
| It runs outside its folder | `outside its own folder` | `in its own folder` |
| Its hooks run a kebbie that is gone | `hooks of a kebbie that is gone` | `with working hooks` |
| Any other | `older code` | `on the new code` |

A narrow board takes a shorter wording, such as `↻ Lead on old MCP servers · L restarts`, never a cut one. See [Restarting the lead after an update](../explanation/updates-and-restarts.md#restarting-the-lead-after-an-update).

The context line reads `Lead context N% · L` on a narrow board. A click on it asks as `L` does. No line shows while the share is unknown. See [Starting the lead fresh](../explanation/lead-and-away-mode.md#starting-the-lead-fresh).

Work:

- A task's halves in other repos sit next to each other.
- Under READY TO PUSH: [COMMENTS](#comments), the comments colleagues left on commits kebbie pushed, while any waits for you.
- Under the groups: the [parked line](#parked) while you have parked tasks.
- Under the tasks: the [bugs and wishes to send](#bugs-and-wishes-to-send), folded into one row until you open them, and the main checkouts with files changed outside any task.
- A folded row, the parked line, the comments or the bugs and wishes to send, is selected like a task: `⏎` or a click on it once selected opens it, and `esc` on a row inside folds it back with the selection on the row.
- After an update's recheck fixed some bugs, `✓ Fixed in kebbie <version>: N bugs you ran into` with their titles. `i` shows the list and clears the line.

Backlog: FOUND BY TASKS lists what tasks noticed outside their work. A `◆` and `2×` mark one found more than once; the selected item shows whole, with its repo and who found it (`found 2× · by billing, search`). See [`kebbie found`](commands.md#kebbie-found---json).

Backlog: ONGOING lists vault project notes in progress (status active, or draft without a priority) when the vault is on. It sits above the items, below them or nowhere, by [`board.backlog.ongoing`](configuration.md).

A backlog item's row shows its state in the right field:

| Right field | Means |
| --- | --- |
| `to write · 0 d` | A short item the lead has yet to write out. A row marked `kept by kebbie` is one you added while the vault takes new items: kebbie keeps it until the lead's note replaces it. |
| `drafting · 0 d` | The lead is writing it out. A muted line under the row says so. |
| `2 questions · 3 d` | Questions on the item wait on you before it can start, in yellow. Answer them on its card. |
| `6 d` | Ready: written out, and no question holds it. Its age alone. |

### A task row

A task row shows its commits, uncommitted and untracked files, and how far it is behind the default branch. Behind is information only: push rebases it when its turn in the push line comes, `e` rebases now, and a task you keep rebased on main catches up by itself.

| Line or mark | Means |
| --- | --- |
| `follows main` | On the open task, while it is kept rebased on main. |
| `bypass` | Its Claude runs in bypass permissions mode. The open task says `bypass permissions: Claude asks for no permission`. |
| `you:` | Your next step, from the task's last report. |
| `obstacle · investigating` | The task raised an [obstacle](../explanation/obstacles-and-findings.md) and kebbie's investigator is on it; the task shows under Working. `obstacle · asked you` while its question waits, `obstacle · with the lead` when the lead decides it. |
| `web` | While its dev server runs: the server's address as a link. |
| `push waits for tests · you` | A push waits for the task's Claude to test the files the default branch changed under it. `· lead` when the lead pushed. kebbie pushes once the task reports ready with the same commits. |
| `recipe: small fix`, muted | Under the row: how the task is done, by the [recipe](commands.md#kebbie-recipe-task-recipe-why) the lead named; the open task adds its why. The assessor's word follows it on the same line. |
| `review suggested: changes how VAT is rounded · v`, muted | Under the row, from the [assessor](../explanation/review-gate.md#an-assessor-judges-whether-the-work-needs-a-review) on its last ready report: the work needs a review, and `v` sends it. Under `review.auto = "run"` or `"require"` it reads `review: …`, and the reviewer is already on it. |
| `no review: wording in two templates`, muted | Under the row: the assessor judged the work needs no review. |
| `second opinion: 2 points · v i reads them`, muted | Under the row: the [second opinion](../how-to/send-a-task-to-review.md#ask-for-a-second-opinion) gave its points, which the lead passes to the task; the push never waits on them. `second opinion: codex-review reading…` while it reads, `(stale)` once the code changed. |
| `review being assessed`, `review not assessed · v`, muted | The assessor is at work, or gave no answer, which counts as a suggestion. |

#### A task in subtasks

With [`task.steps`](configuration.md) on and subtasks listed, the task's subtasks not pushed yet sit under its row, one line each, with the subtask's number, title and state:

```
◐ Switch the payment provider to Stripe        billing · 1 to accept
    ↑ 1 pushed
    ● 2 Charge through a PaymentGateway        ready · 2 commits
    ◐ 3 Add Stripe as the gateway              in progress · 1 commit
    ○ 4 Remove the old payment client
   accept 2 · r review
```

| Line or mark | Means |
| --- | --- |
| `N to accept`, yellow | On the row, in place of its commit count: N ready subtasks wait for you to accept them. |
| `↑ N pushed`, muted | N subtasks are on the default branch already. |
| `○`, muted | Planned. |
| `◐ … in progress · N commits`, blue | Its Claude is at work on it, with the commits it holds so far; without commits, just `in progress`. Only the last subtask with commits is in progress, and only until the task reports ready. |
| `● … ready · N commits`, yellow | Its agent ran `kebbie subtasks done` on it, or a later subtask holds commits, or it is the last and the task reported ready: finished, waiting for you to accept it, followed by the task's rules check in short while it is not passed: `· project rules not checked`, `· checking project rules` while the reviewer or the lead checks, `· breaks N project rules` when it found some, `· pushed past the rules check by you` (or `by the lead`) after `kebbie approve`. A passed check adds nothing. |
| `✓ … accepted`, green | You accepted every commit in it. `accepted · waits for <subtask>` when a commit of a subtask you have not accepted lies above one of its own in the push order. |
| `accept N · r review` | Under the subtasks: the first subtask waiting for you. `r` opens the review, and [`kebbie accept`](commands.md#kebbie-accept-name-sha) accepts it. |
| `accept subtask N · r review` | With [`task.require_accept`](configuration.md) on, a ready task with a subtask to accept waits under Needs you with this line: it cannot be pushed until subtask N is accepted. |

A subtask to accept leaves the task in the group its phase gives it, under Working while it builds on. The state word always shows: a long title is shortened at a word with `…` to make room, and only when the title would drop below 12 columns does the rules part shorten (`· 2 breaks`, `· checking`, `· not checked`, `· past the rules`) and then go. Without subtasks, the row is as above. See [Work in subtasks](../how-to/work-in-steps.md).

#### A task in stages

A task with [stages](commands.md#kebbie-stages-name-titlerequirement-) shows one line per stage under its subtasks, `stage N`, its title and where it stands: `after <stage>` while an earlier stage is not met, `open`, `pushed · deploy` (or `· check`, `· verified`) while its push goes through CI, `task checks`, `verify` while kebbie's question waits on you, `met` in green, `failed` in red. The row reads `stage N of M` with what the stage waits for, or what failed. The push window stops at the first stage that is not met.

#### kebbie's own questions

Beside a task's own question, kebbie asks you three things itself, in the inbox (`i`) and on the phone, each answered with its number key: a stage's verification and a check a task asked with `kebbie ask --verify` (`1` Verified, `2` Not yet, `3` Something is wrong with what you saw, `w` in your own words), and whether to close a task it would otherwise leave to you (`1` Close, `2` Not yet). Not yet on a verification leaves it waiting; on a close it lets the task be. Neither holds the task or its messages. kebbie asks to close a task that reports ready with nothing to push, a routine's findings with nothing committed, a task landed on main beside its own open question, and a task you pushed through its last commit and kept open. An answer from the phone waits on the hub up to a week for a machine that sleeps.

While Claude waits on a prompt of its own, such as a permission or trust question, the `you:` line says `answer Claude's prompt in its Space or through the lead`. `g` takes you to its Space, and the lead has the question and its options in its chat. A Codex task on a start-up question says what Codex asks, as `answer Codex in its Space: it asks whether you trust its folder`. See [Claude's own prompts in a task](../explanation/lead-and-away-mode.md#claudes-own-prompts-in-a-task).

The selected task opens in place with its review progress, its question and options, its report, its latest test run, and a warning about files changed in a main checkout outside any task.

The `tests` line shows the last test command the task's Claude ran, as kebbie recorded it (see [`kebbie evidence`](commands.md#kebbie-evidence-name---json)). The wide panel shows the same line. No line means no test run is recorded.

When the task's Claude runs a command that looks like a test run but is not in the list kebbie records, such as `script/test` or `make test`, the row says `script/test is not recorded as a test run: add it to task.test_commands`. Once you add it to [`task.test_commands`](configuration.md#keys), the next run is recorded and the note goes.

A listed test command inside a loop, a condition or `xargs` is not recorded either, since its result is not the command's: the row says `do script/test is not recorded as a test run: kebbie cannot tell its result inside a loop, a condition or xargs`. The task's Claude hears both notes as it runs the command.

| Line | Means |
| --- | --- |
| `✓ cargo test · 493 passed · at a1b2c3d`, green | It passed on the code the branch holds now, and the worktree has no uncommitted changes. |
| `○ cargo test · 493 passed · at a1b2c3d, code changed since`, yellow | It passed, but a commit changed the code after it, or the worktree has changes it did not run on. |
| `✗ cargo test failed · 2 failed · at a1b2c3d`, red | The latest run failed. |
| `? cargo test result unknown, piped · at a1b2c3d`, peach | kebbie saw the run but not its result, so it counts as neither a pass nor a fail. |

## Groups

| Group | Contains |
| --- | --- |
| Needs you | Tasks waiting on your answer or action, open questions first. |
| Working | Tasks at work since their prompt, their last report or your answer. |
| In review | Reported ready and sent to the reviewer, waiting for a pass on its current code. The row says who acts next: `review running · waits on the reviewer`, or, with findings and its Claude idle, `withdraw with v to push without a pass`. With no reviewer running, the line under it shows the verdict's first finding, as `review asks for changes: <finding> (+N more)`. |
| Ready to push | Reported ready with its commits, and pushes under way. |
| CI running | Pushed, with its worktree kept until every workflow run for the pushed commit is green and the task's check after the push holds; then it closes as done. A cancelled run waits for a later run on the default branch to pass. |
| Waiting for work | Started without work, blocked on something outside you, or kept open with nothing left on top. |
| Queued | Waiting to start after other tasks or at a time, with no worktree, Space or Claude. |
| Parked | Tasks you set aside until later, folded into one line under the other groups. See [Parked](#parked). |

kebbie keeps one phase per task, and only what changes it writes it: its start, the first prompt it takes up, its report, its question and your answer, a push and its hand-over, a CI verdict, an API error it will not resume. The group follows that phase.

- Whether its Claude is mid-turn shows as the row's `◐`, never as a move. A task reading kebbie's update notice, or one the lead tells to fix a commit, stays in its group until it reports.
- Only a prompt of your own, typed into its Space or sent as your answer, moves a task that waits for you or for work under Working. A message from the lead, another task or kebbie does not.
- Four things put a task under Needs you whatever its phase, for as long as they last: Claude's own prompt on its screen, an open question, messages held behind your draft, and landed work kebbie leaves to you.

### Needs you

- A question on screen, such as a permission or trust prompt, whatever else the task is doing.
- An open question from `kebbie ask`.
- A review whose findings only ask for your confirmation, marked `confirm what its review asks · <the finding>`: Confirm passes the review, Change it sends your comment to the task.
- Messages held behind a draft in its input box.
- A task whose commits are on the default branch that kebbie does not close by itself, marked `landed on main · d closes it` beside a question (kebbie asks you whether to close it), or `landed on main, but its worktree has uncommitted changes or a stash` (see [`kebbie done`](commands.md#kebbie-done-name)).
- A task that reported needs-you, or ready with nothing committed.
- A task that reported ready with commits while tracked files are uncommitted, marked `2 uncommitted files hold it: a.md and b.md · p then p pushes the commits and keeps them`. The push window opens on its last commit, so `p` pushes every commit and keeps the task open with the files as they are; its approval is of the commits and stays.
- With `task.require_accept` on, a ready task with a commit you have not accepted, marked `accept subtask N`.
- A task whose turn ended without a report even after kebbie asked for one.
- A task whose CI failed after a push of only some of its commits, marked `CI failed: <workflow> · <job> · <step>`, and `its Claude says outside the code: <reason>` once its Claude has said so. `C` dismisses it.
- A push under way whose Claude reported needs-you.
- A push that left its repo's push line unpushed, marked `out of the push line: <why>`: its Claude did not report ready within 30 minutes, it reported needs-you, or it has commits nobody checked since you chose push. Push again and it joins the end of the line.
- A push or close kebbie could not finish.
- A pushed task whose CI gh has not been able to read for 15 minutes, its row marked `gh cannot read it: <error>` meanwhile.
- A task an API error stopped that kebbie will not resume, marked `stopped by <reason>: <Claude Code's words>`.
- A task whose worktree git cannot read, such as one whose index names a missing object, marked `git cannot read its worktree: <git's error>`. The other tasks show as usual.

`N` puts what needs you in one numbered list, in the order to do it. See [The Next window](#the-next-window).

### Working

- Started with a prompt, or at work since its last report or since you answered it.
- A pushed task whose CI is green and that kebbie closes once its note is written, marked `pushed · kebbie closes it`.
- A task kebbie resumes after an API error, marked `retrying after overload (2/4, next 21:14)`, `out of allowance until 17:00`, then `resumed after …` until Claude answers. See [When the API stops a task](../explanation/lead-and-away-mode.md#when-the-api-stops-a-task).

### Ready to push

A ready task's rules check has its own line under the row:

- `checking project rules`, muted, while the reviewer checks its commits against the rules skill; `lead checking project rules` when the reviewer gave no verdict and the lead checks them;
- `breaks N project rules: <rule> · <rule>`, yellow, when the check found some, naming the rules the reviewer named;
- `pushed past the rules check by you` or `by the lead`, muted, after [`kebbie approve`](commands.md#kebbie-approve-name), under the breaks line when the check found some;
- `project rules not checked`, muted, when no check was asked for;
- nothing once the check passed.

The row itself counts uncommitted files when you edited after the report.

How far it is behind the default branch does not matter. A push under way stays here, marked `pushing for you · <what it waits for>`:

- its turn, as `2nd in line after <title>` while other pushes of its repo go first, and `kebbie pushes it now` at the head;
- the tests for files the default branch changed under it, a conflict, or a fix for CI that failed after the push.

Each repo pushes one task at a time, in the order you pushed them, and rebases a task only when its turn comes. A head whose Claude works keeps its place while the line waits for it, for up to 30 minutes. See [Pushing](../explanation/trunk-based.md#one-push-line-per-repo).

A task on local main stays here too, marked `on local main · waiting for your push`, until main is pushed and it closes as landed.

### CI running

A report its Claude writes meanwhile keeps it here unless it says needs-you or blocked. The row names the runs, and the open task links each run's page.

A task that named a check for after its push (`kebbie report --after-push`) adds `then checks: <check>`, muted, while CI runs. Once CI is green its Claude runs the check, marked `checking after push: <check>`; the task closes as done when it reports the check held, and moves to Needs you with what failed when it did not. See [`kebbie ci`](commands.md#kebbie-ci---json).

### Waiting for work

- Started without a prompt, or its start never reached its Claude.
- Reported blocked on something outside you.
- Pushed in part with `keep open` and nothing left on top.
- Kept open and its red CI dismissed.

### Queued

The row, marked `⧗`, carries a title from its backlog item or the first sentence of its prompt, which it keeps once started until Claude names the session. See [`kebbie after`](commands.md#kebbie-after-name).

- Its second line, whole, says why it waits and what starts it: `Waits for Billing totals (ready to push): it uses the rounding field. Starts by itself once it is pushed and CI is green.` The reason is the one given with `kebbie start --why`.
- It starts by itself once each task it waits for has ended as done.
- A task queued with `--at` waits for that time as well, or only for it, and the row says when: `Starts 15:00 today.`, `Starts Fri 24 Oct 09:00.`
- When a task it waits for is dropped, reset or gone, the row moves to Needs you with the reason instead.

### Parked

A task you park with `Z` leaves its group, whatever its phase, and keeps working. The other groups and the status line's counts leave it out, the lead leaves it out of its summaries, and its notifications are not sent. See [Park a task until later](../how-to/park-a-task.md).

| Line or mark | Means |
| --- | --- |
| `◌ 3 parked · ⏎ shows them` | The parked line, under the other groups: how many tasks are parked. Select it and press `⏎`, or click it twice, to show them. |
| `· 1 question`, `· 1 red CI` | On the parked line, peach: a parked task has an open question or a failed CI run. It stays parked; the line counts it so you see something waits. |
| `PARKED  3 · 1 question  esc hides them` | The header while the parked tasks show, each on its own row. `esc` folds them back into the line. |
| `back 09:00 tomorrow` | On a parked row: when it returns to its group by itself. |
| `back when you unpark it` | On a parked row: parked until you bring it back. |

Task keys on the parked line act on the line only, `Z` among them: `⏎` shows the parked tasks first. A parked task comes back when its time comes, with `Z` on its row, or with [`kebbie unpark`](commands.md#kebbie-unpark-name). `done`, `drop` and `reset` end its park with the task. The phone's board leaves parked tasks out.

## Comments

The comments colleagues leave on commits kebbie pushed, each read by an independent, read-only Claude, sit in their own section under READY TO PUSH. They are no task: Needs you, the status line's counts, the lead's summaries and your notifications leave them out, and one line in the messages says when a comment has been read. See [Act on comments on pushed commits](../how-to/act-on-comments-on-pushed-commits.md).

The section shows open while a card waits for your choice that you have not folded over, and folded otherwise, as `◇ 3 comments · ⏎ shows them`, peach while one waits. Open, its header reads `COMMENTS  3  esc hides them`, and each comment takes two lines: its row and the first words of the comment, dim.

| Row | Means |
| --- | --- |
| `◆ johan on 3f2a9c1 · refunds-in-part   holds · reopen with a fix` | The comment names a real defect; the reading proposes a fix. |
| `◇ anna on 3f2a9c1 · refunds-in-part   opinion · reply, no change` | A preference; the reading recommends replying, with a reason from the code. |
| `✓ maria on 3f2a9c1 · refunds-in-part   praise · nothing to do` | Thanks or approval. |
| `? lena on 3f2a9c1 · refunds-in-part   unclear · answer its question` | The reading cannot tell without you and asks one question. |
| `· johan on 3f2a9c1 · refunds-in-part   reading…` | The reading is under way. |
| `! johan on 3f2a9c1 · refunds-in-part   not read · ⏎ reads it again` | The reading gave no answer. |
| `↻ johan on 3f2a9c1 · refunds-in-part   fixing in refunds-in-part-followup` | A task fixes it. |
| `↩ johan on 3f2a9c1 · fixed in 7c9e2f1   reply ready · y copy` | The fix is on the default branch, or you chose to reply without one: the reply waits to be posted. |

A narrow column drops the task, then the commit, from the row before it cuts what the row says. A card stays until you mark its reply sent, finish it or dismiss it; it never leaves on its own.

### The comment window

`⏎` on a comment opens its card, titled such as `comment 1/3 · holds · refunds-in-part`: who wrote it on which commit, at which file and line, the comment, the hunk of the diff it is on, and `Independent reading`. Then, by its kind:

| Kind | Shows | Choices |
| --- | --- | --- |
| holds | `Proposal` with how to test it, and `The fix's commit message`, whose body ends with `Addresses johan's comment on 3f2a9c1.` | `1 Reopen <task> with the proposal (recommended)`, `2 Reply without a change`, `3 Dismiss`, `w In my own words` |
| opinion | `Recommendation` and `Reply to send` | `1 Reply without a change (recommended)`, `2 Reopen <task> and change it anyway`, `3 Dismiss`, `w Edit the reply` |
| praise | | `d Done (recommended)`, `r Reply anyway` |
| unclear | `Question` | `w Answer the question (recommended)`, `3 Dismiss` |
| reply ready | `Fixed in 7c9e2f1, on the default branch` and `Reply to send`, as plain lines you can select in the terminal | `y Copy the reply (recommended)`, `d Mark it sent`, `w Edit the reply` |

- Reopen tells the task that pushed the commit, while it runs, what to change and the commit message to use; once it has ended, a new task starts in the same repo, named `<task>-followup`, with that as its brief.
- `w In my own words` reopens with what you type in place of the proposal, under the same commit message. When the reading has no commit subject, as for a question, the fixing task writes the subject and body from its change and ends them with the `Addresses` line.
- The answer to a question goes to a fresh reading.
- Once a commit carrying the `Addresses` line is on the default branch, the card turns to its reply, naming the fix's sha.
- `y` copies with the clipboard program (`pbcopy`, `wl-copy` or `xclip`), or over ssh through the terminal (OSC 52).

kebbie never posts a reply.

## Keys

`⏎` opens the options for the selected row. Each option's letter also works directly. Every key that is a plain character can be moved in config.toml's `[keys]`; help, hints and messages then show the key you chose. See [Keys](#keys-in-configtoml).

### Everywhere on the Work page

| Key | Action |
| --- | --- |
| `⏎` | Options for the selected task. In the Backlog column, the item's card. |
| `↑` `↓`, `j` `k` | Move. |
| `←` `→` | Switch between Work and Backlog. |
| `1`, `2`, `3`, `,` | The Work page, the Areas page, the Routines page, the [Settings page](#settings-page). |
| `tab` | Move focus to the lead's chat, showing it first when it is hidden. |
| `h` | Hide the lead's chat so the board takes the whole width while the chat runs on, or show it again. Only on the board in the lead Space. See [Hiding the chat](../explanation/lead-and-away-mode.md#hiding-the-chat). |
| `N` | [Next](#the-next-window): what needs you, in the order to do it. |
| `i` | The inbox: the questions waiting on you, opened at the first. Works on the Areas page too. |
| `m` | Messages: once shows them on one line, again opens them, again hides them. |
| `H` | The [hub](#the-hub-window) window, to see this board on your phone. |
| `n` | New task: a name, then an optional prompt. Starts in the background. In the Backlog column, a new item. |
| `/` | Filter the selected column: type part of a name, `⏎` keeps the filter, `esc` clears it. |
| `R` | Ask the lead to refresh the backlog. |
| `A` | Away mode on or off. |
| `U` | Run `kebbie update` in a window that shows each step. |
| `L` | Restart the lead on the installed kebbie now, whether or not the lead line shows. |
| `?` | The keys window: every key on the page you are on. Any other key closes it. |
| `esc` | One step back: close a window, a menu or the filter. At the top, close the board when it is a popup; on the full board it does nothing. |
| `q` | Close the current view, like `esc`. At the top, close the popup; on the full board it says that `ctrl+c` twice quits. |
| `ctrl+c` | Twice within 2 seconds quits the board. The first press says so. |
| `ctrl+l` | Redraw every cell of the board, over any old text the terminal still shows, on every page and in every window. The board does this by itself after its pane changes size. |

The [bugs and wishes to send](#bugs-and-wishes-to-send) are folded into one row at first. While any waits, the header counts them, such as `! 2 bugs to send`. The board remembers whether the list shows. See [Send it to kebbie's maintainers](../how-to/report-a-problem-with-kebbie.md#send-it-to-kebbies-maintainers).

`U` shows a bar while kebbie builds or downloads. `esc` hides the window while the update goes on, and `U` opens it again. Done, it shows what's new and fixed from kebbie's release notes and what restarts; `d` shows the details, any other key or a click closes it. A summary or details taller than the screen scroll with `↑` `↓`, `PgUp` `PgDn`, `Home` `End` and the mouse wheel, and `↓ more` on the keys line says there is more. See [What an update shows](../explanation/updates-and-restarts.md#what-an-update-shows).

When the window closes and Codex has a newer version, the `agent updates` window asks `Which updates?`: `Codex 0.163 → 0.164` with how Codex is installed, or `Skip this time`, asked again on the next `U`. `⏎` runs Codex's own installer and shows when it is done; `esc` closes it. With `update.agents = "auto"` it installs at once and says so; with `"notify"` there is no window. Codex tasks restart on the new version once they are idle, and the board takes the new kebbie only once the window is closed. See [Codex updates](../how-to/run-a-task-with-codex.md#codex-updates).

`L` opens `Restart the lead` with two rows. See [Starting the lead fresh](../explanation/lead-and-away-mode.md#starting-the-lead-fresh).

- `New conversation`, selected, starts its Claude fresh from its role and the vault, the old conversation left on disk.
- `Resume this conversation` restarts it on the same one, as to pick up a new Claude Code.
- `↑` `↓` (or `j` `k`) move, `⏎` or a click picks, `1` and `2` pick directly, `esc` cancels.
- The result goes by like the board's other results.
- It is refused with the plain reason as a toast, before it asks, while the lead works, asks, holds text you have not sent or has background work running. Press it again later.

`?` groups the keys by what you want to do, one short line each, under the title `keys · work`, `keys · areas` or `keys · routines`.

- On a screen about 114 columns wide or more it is two columns. Narrower, it shows up to three keys a row, scrolling with `↑` `↓` and `pgup` `pgdn` when it does not fit.
- A moved key shows as you set it, and `↑` and the version beside `U` when there is a newer one.
- The last line names the kebbie that runs: `kebbie main` on the main channel, `kebbie 0.27.0` on the release channel.

On a queued row, `⏎` offers three things. `g` and `x` ask first.

- `g` starts it now from the default branch, without the changes of what it waits for, its Claude told what is missing.
- `l` sets when it starts (`15:00`, `tomorrow 09:00`, `fri 09:00`, `24 oct 09:00`, `in 2h`), and it starts then once what it waits for is done too.
- `x` drops the wait, so it never starts and its backlog item goes back to the backlog.

A question that asks first, as reset, drop and done do, takes `y` to go ahead and `n` or `esc` to cancel. Any other key keeps the question and says "press y or n".

### On a task

| Key | Action | Offered when |
| --- | --- | --- |
| `s` | Answer its question: the questions window, at this task's question. | It has an open question, its own or kebbie's on its review or an obstacle. |
| `g` | Go to its Space. On a task whose Space or Claude is gone, restart it and resume its conversation. | Always. |
| `c` | Chat: message its Claude. | Always. |
| `r` | Open the review split next to its Claude, or go to the one already open in its Space. | Always. |
| `s` | Show the sign-off card: Built, each decision with who took it, what became of each Also noticed line, and what changed since its last version. | It has a card and no open question. |
| `w` | Start or stop the dev server. | `worktree.dev_server` is set. |
| `b` | Start or stop the worker. | `worktree.dev_worker` is set. |
| `1`–`9` | Open one of its links in the browser, listed under LINKS. | It has that link. |
| `v` | The review menu: send it to review, or withdraw it while it is in review. See below. | It has commits and Claude is not mid-turn. |
| `e` | Rebase onto the default branch now, in the background, without pushing. | It is behind the default branch, Claude is not mid-turn, and it is not pushed. |
| `t` | Ask its Claude to tidy its commits by the rules skill. | Claude is not mid-turn, and it is not pushed. |
| `p` | Push: open the push window. On a task already in the line, take it out again, unpushed. | Claude is not mid-turn, it is not waiting for the reviewer, and it is not pushed. |
| `M` | Put on local main to try it; on a staged task, take it off again. | See below. |
| `C` | Dismiss the red CI of a pushed task whose failure is not the code's. Asks first. Yours alone. | The task's CI failed. |
| `G` | Re-run CI: gh runs the pushed commit's failed and cancelled runs again, and kebbie watches them as after a push, closing the task once they are green. | Every commit is on the default branch and its CI failed or was cancelled. |
| `d` | Done: close a task you already pushed elsewhere and free its worktree. Asks first. | Claude is not mid-turn. |
| `a` | Let the lead push it, or take that back. `done`, `drop` and `reset` take it back too. | It is a task, not pushed. |
| `u` | Keep it rebased on main, or switch that off; `●` marks it on. | It is a task, not pushed. |
| `Z` | Park it until later; on a parked task, bring it back now. | It is a task. |
| `z` | Reset: its work goes, its backlog item returns. Asks first. | Always. |
| `x` | Drop. Asks first; on uncommitted work or unpushed commits, asks again to force. | Always. |

A task counts as pushed once every commit is on the default branch and kebbie watches its CI or saw it land.

- `1`–`9`: the links are listed by what each is: the running dev server, each CI run, a mock, a pull request or another address its report or card names, and its vault note, which opens in Obsidian. The highlighted link's whole address shows on the description line. Where the board has no browser, as over ssh, it says the address instead.
- `e`: the result shows when it is done. A conflict goes to its Claude, as with [`kebbie land`](commands.md#kebbie-land-name).
- `p`: `y` or `p` in the push window puts the task in its repo's push line, which rebases it onto the default branch and pushes it when its turn comes, at once when the line is empty. Taking it out does not work for a fix for red CI, whose commits are on the default branch already (`C` dismisses that). `e` is not offered while a task is in the line.
- `M` puts its commits on the main checkout's local default branch, unpushed, after the tasks staged before it, and the row says `on local main · waiting for your push`. Taking them off leaves the other staged tasks. To put on, the repo sets `worktree.stage_on_main`, it has commits, Claude is not mid-turn, it is not waiting for the reviewer and has no CI watch. To take off: always. See [`kebbie try`](commands.md#kebbie-try-name).
- `C` keeps the failure on record with the reason its Claude gave, and the task closes once its commits are on main.
- `d` also closes one under CI running or with failed CI you choose to leave. The keeper closes a landed task by itself; `d` is for the ones it leaves to you.
- `Z` opens `Park <task> until` with four rows, each with the time it means on the right: `Tomorrow`, selected (`sat 09:00`), so `Z` then `⏎` parks it until tomorrow at 09:00; `Monday` (`mon 09:00`), the coming Monday at 09:00, which on a Monday before 09:00 is that morning; `A date…` turns the window into a field for a day as [`kebbie start --at`](commands.md#kebbie-start-name-prompt) takes it (`fri`, `24 nov`, `2026-12-01`, `tomorrow 13:00`), a day alone at 09:00, showing the time it reads as you type (`thu 5 nov 09:00`, or `?`), where `⏎` parks and `esc` goes back to the choices; `I bring it back` keeps it until you press `Z` on it. `1` to `4` pick directly, `esc` cancels. See [Parked](#parked).
- `u`: when main moves, the keeper rebases it, never pushes it. Off by default; `done`, `drop` and `reset` switch it off. See [`kebbie follow`](commands.md#kebbie-follow-name-onoff).

#### The review menu

`v` opens `review · <task>`: a line with its commits since the default branch and the assessor's word, then the answers, each with what it runs. Its letter runs one, `↑` `↓` and `⏎` too, `esc` closes it and sends nothing.

| Key | Action | Offered when |
| --- | --- | --- |
| `v` | Review: the reviewer [`review.profile`](configuration.md#review-and-docs) names reads its commits; the task waits under In review. With `review.second = "always …"` the second opinion runs beside it. | It is not in review. |
| `v` | Withdraw from review: the reviewer stops, its verdict and any second opinion go, push is offered again. Yours alone. | It is in review. |
| `s` | Review with a second opinion: the reviewer, and `review.second`'s profile beside it. | `review.second` names a profile, not `always`, and it is not in review. |
| `o` | Second opinion only. The push still waits on the reviewer while the task is in review. | `review.second` names a profile. |
| `i` | Read the second opinion's points in `$PAGER`. | A second opinion gave its points. |

### How messages reach a Claude

kebbie's messages reach a Claude that kebbie started on Claude Code 2.1.224 or later through that session's inbox socket. It shows in the conversation as one dim line from kebbie that ctrl+o expands. Claude reads it between tool calls or starts a turn when idle, and your draft is never touched.

- kebbie's notices that ask nothing of the lead are not delivered live: a task started, pushed or closed, a review that passed, a rebase or a red CI its task resolves itself, an update. They show in the message log and reach the lead with your next prompt to it, grouped by task.
- What asks the lead to act is delivered: findings to pass on, a rules check the reviewer left, a task's question of method, a push that failed, a prompt on a task's screen, a red CI no task took. While you talk with the lead (it is mid-turn, you prompted it in the last 3 minutes, or your text is in its input box) it is held, and goes as one message grouped by task once you have been quiet for 3 minutes; a prompt on a task's screen goes as soon as the lead's turn ends and comes first. In away mode it goes at once. See [It waits while you talk with it](../explanation/lead-and-away-mode.md#it-waits-while-you-talk-with-it).
- A message that starts a lead turn ends with the line `(kebbie: work silently; end in Done / Going on / Needs you, or one line if nothing needs the user)`. The lead works without narrating its steps and closes the turn with what landed, what is going on per task and what needs you, or with one line such as "Noted: 3 updates, nothing for you." when nothing does.
- kebbie then checks the conversation for it as it does for a typed message. After 10 minutes without it, the board says it was not delivered; a rules check or a hand-over is held and sent again.
- kebbie starts the lead and its tasks with `crossSessionInbound` set to `accept`, so any process of your user on the machine can message them. That is the same trust as typing into their panes, and the sandbox, when on, limits what a task can reach.

For an older Claude Code, or when the inbox refuses a message, kebbie types it instead:

- It types into a Claude's input box only when the box is empty. While you have a draft there, a message to that Claude waits and goes in once the box is clear.
- After typing, kebbie checks the conversation for it. When Claude has not taken it, kebbie presses Enter again after 5, 10 and 20 seconds, and after 40 seconds the board says it was not delivered.

### In the Backlog column

| Key | Action | Offered when |
| --- | --- | --- |
| `g` | Start the item as a task, with the whole item as its prompt. Asks for a name, suggested from the title. While questions on it wait on you, the status line says so for 3 seconds, and `g` again within them starts it anyway. | On an item. |
| `⏎` | The item's card. A row kept by kebbie or a Linear issue has no card: `⏎` is its options. | On an item. |
| `o` | The item's options, each one explained. | On an item. |
| `g` | Go to the task working on this note, or resume the note as a new task. | On an ONGOING note. |
| `o` | Start a task (a name, suggested from the line), Add to the running task that found it (a task name, filled in), Keep in backlog, or Dismiss. A kept item stays, marked `kept`, and its card shows the choice. | On a FOUND BY TASKS item. |
| `⏎` | The note's card. | On an ONGOING note. |
| `l` | Start the item as a task later: a name, then a time. It waits under Queued until then, as with [`kebbie start --at`](commands.md#kebbie-start-name-prompt). | On an item. |
| `R` | Ask the lead to refresh the backlog. | On an item. |
| `n` | Add an item: a line, then a priority. The lead writes it out. | `"local"` or `"vault"` is a source. |

The note's card shows its whole title, area, status and summary, and who works on it (machine, task and since when). It shows its task and whether it runs here, and its opening: the text before its first heading and its first two sections from the local vault index, or the first paragraph the vault snapshot carries.

- `↓` `↑` `pgdn` `pgup` scroll it.
- `g` or `⏎` does what `g` does on the row.
- `esc` closes it.
- A click on the selected note opens it too.

The item's card shows its title, priority, repo, state, the day it was filed and its file or vault note, then its six sections. Under Open questions, a peach `?` marks a question that blocks the start and a muted `·` one the work settles.

| Key | Action |
| --- | --- |
| `↑` `↓` | Pick a question. |
| `a`, `⏎` | Answer the picked question. `⏎` sends the answer to the lead, which folds it into the item. Until it does, the question shows `with the lead: "<your answer>"`. |
| `g` | Start the item, as `g` on its row. |
| `w` | Ask the lead to write the item out again. |
| `e` | Open the item's file in your `$EDITOR`. Local items only. |
| `pgdn` `pgup` | Scroll. |
| `esc` | Close the card, or the answer box back to the card. |

### The Next window

`N` opens it over the board: one numbered list of what waits on you, what stops the most work first. The number is the order, the heading says why.

```
╭ Next · 6 need you ───────────────────────────────────────────────────────────────────────────╮
│                                                                                              │
│  STOPPED ON YOU                                                                              │
│ ▸1  ? billing-vat         answer its question: Round per invoice or per line? waiting 42 min │
│  2  ✗ booking-calendar    CI red after push · its Claude says outside the co… waiting 12 min │
│                                                                                              │
│  PUSH, IN THIS ORDER                                                                         │
│  3  ↑ card-component      2 commits · 2 tasks wait on it                           ready 1 h │
│  4  ↑ fix-grid            2 commits · bump-pins waits on it                     ready 20 min │
│  5  ↑ copy-typo           2 commits                                                ready 3 h │
│                                                                                              │
│  TO READ                                                                                     │
│  6  ◆ herdr-app-research  result ready · recommends staying on the TUI                   2 h │
│                                                                                              │
│ 4 working · 3 queued · 1 parked                                                              │
│ ⏎ open · P push 3–5 in order · ↑↓ move · esc board                                           │
╰──────────────────────────────────────────────────────────────────────────────────────────────╯
```

| Group | What is in it | In what order |
| --- | --- | --- |
| Approve or deny | A device on your account that sent this machine a command under [`hub.trust = "device"`](configuration.md#machine), waiting for you to let it make changes here. Its row says when `hub.auto_approve` lets it in. | The one that has waited longest first. |
| Stopped on you | A task that stands still until you act: a question or a plan to approve, Claude's own prompt, a report that asks you something, a crash, a red CI only you can settle, a subtask to accept with `task.require_accept` on. | The one most tasks wait behind first, then the one that has waited longest. |
| Push, in this order | What is ready to push and not in the push line yet. A task whose ready report names another to push first (`kebbie report --after`) says `after <task>` while that task is open. | What other tasks wait on first, in any repo, queued or ready to push after it, then two tasks that change the same files in the order they were built, then the one that was ready first. |
| To read | A task that reported ready with nothing to push, such as a research result. | The one a decision waits on first, then the oldest. |

- The time on the right is how long it has waited on you.
- Tasks at work, in review, waiting for CI, queued or parked stay out of the list and are counted on the line at the bottom. A red CI its agent is fixing counts as working.
- When nothing needs you, it says `✓ Nothing needs you.` and what is on its way in: how many are working and which is next to come, what is queued and when a parked task comes back.
- A list taller than the window scrolls with the selection, and `↓ more` on the keys line says there is more.
- Next shows the same tasks as Needs you and Ready to push, in the same words; the board keeps those groups.

| Key | Action |
| --- | --- |
| `↑` `↓`, `j` `k` | Move. `PgUp` `PgDn`, `Home` `End` move further. |
| `⏎` | Open or do it: a question opens in the inbox, a push opens its push window, a result opens its card, anything else opens the task's options. Its row is selected on the board. |
| `P` | Push the push group in its order, after a window that shows the order. |
| `a`, `x` | On a device's row: approve it, so what it sent goes in now, or deny it, so it stays view only here. `⏎` opens a window with what it sent, its key and when it is let in by itself, with the same keys and `esc` to leave it waiting. |
| `esc`, `q`, `N` | Back to the board. |

`P` opens `Push 3 tasks in this order`, with each task, its commits and what waits on it.

- `Push all three` pushes them one after the other, in the order shown. Each is rebased onto main and joins its repo's push line.
- `Push card-component only` pushes the first.
- The first push that is refused stops the rest: the window that follows names it, why, and what is still ready. The rest stay under Ready to push.
- `←` `→` choose, `⏎` or a click pushes, `esc` goes back to the list.

`kebbie next` prints the same list. See [Commands](commands.md#seeing-what-is-going-on).

### The card window

Opened by `s` on a task with a sign-off card and no open question, in a narrow board and a wide one alike. The title names the task. The first line says whether the card is current at its commit, or written at an earlier one with the code changed since.

The card's text follows in parts, each under a header on a rule. A paragraph that opens with a short label and a colon, such as `Built:` or `Decisions:`, starts a part of its own headed by that label. The text before the first label is headed `CARD`, and a card with no label is one `CARD` part. `**bold**` shows bold.

The images show one at a time, each as large as its space allows:

- In a window 120 columns wide or more, the text sits on the left with an `IMAGES` part listing every image by file name and alt text, and the selected image fills the right side. Over it, `1/3 <file name>  <alt text>` says which image it is; the file name is a link to the file. A click on an image in the list selects it.
- In a narrower window the parts stack, and the lower part of the window shows the selected image across the window's width under `IMAGE 1/3 <file name>`.

How an image shows depends on the terminal. `KEBBIE_IMAGES` picks the protocol by hand; see [environment](environment.md).

- With a graphics protocol (Kitty, Sixel or iTerm2) the image is sharp. Inside herdr kebbie uses the Kitty protocol, which herdr passes on to a Kitty-capable outer terminal such as Ghostty or Kitty.
- With only halfblocks the image is a coarse preview in text cells, with `preview · f larger · o opens the image` under it.
- With images off (`KEBBIE_IMAGES=off`, or the board not on a terminal), or a file that does not decode as an image, only its name shows.

| Key | Action | Shown when |
| --- | --- | --- |
| `←` `→` | The previous or next image, wrapping around. | The card has two images or more. |
| `↑` `↓`, `j` `k` | Scroll the text. `PageUp`, `PageDown` scroll by ten lines. | Always. |
| `f` | Fullscreen: the selected image alone over the whole board, as large as it fits. | The card has an image and images are on. |
| `o` | Open the selected image in the system viewer: `open` on macOS, `xdg-open` on Linux. | See below. |
| `p` | Go on to the push window. | Always. |
| `esc`, `q` | Close. | Always. |

- In fullscreen, `←` `→` go between the card's images, `esc`, `q` or `f` go back to the card, and `o` opens the image shown.
- `o` is shown when the card has an image, the board does not run over SSH (`SSH_CONNECTION` or `SSH_TTY`), and on Linux `DISPLAY` or `WAYLAND_DISPLAY` is set.

In a narrow board the selected task also shows the card under its row: whether it is current, its first two lines of text, and how many images it has.

### The push window

Opened by `p`. The window is in four parts, each under a header on a rule:

| Part | Shows |
| --- | --- |
| `CARD` | The task's sign-off card, marked current at its commit or written before the code changed. Its images are listed by file name; `s` on the board shows them. |
| `TESTS` | The latest recorded test run, as on the task's row. A run before a clean rebase, a reworded message or a squash still counts as current. |
| `COMMITS` | The commits that will land, with their count. A commit you accepted is marked `✓`, green. With subtasks, the commits stand in push order, oldest first, and where a subtask's commits start they carry its mark, number and `· title`; the subtasks without commits follow, muted: `in progress, stays` or `planned`. |
| `MESSAGE` | The selected commit's full message, set off by a bar so it does not read as part of the card. |

With accepted commits, the window opens on the last of the accepted ones from the first commit on, marked `◂ through here`, and `p` reads `p push N accepted, the task keeps working` while it is selected. With subtasks, `◂ through here` ends the pushable run, the commits from the top that you accepted, never splitting a group, and `p` reads `p push the accepted run, N commits and keep it open`. Without any, it opens on the first commit. While the task's agent is working, `p` opens the window only once it has accepted commits from the first one on, and the window then pushes only those: `↑` `↓` stop at the last of them, and `y`, `e` and `s` wait until the task reports.

A group's commits carry a bracket down their left, `┌` `│` `└`, and go out together. After a commit's subject, `tests ✓`, `tests ✗`, `testing…` or `not tested` is its own test run once the task reported ready, and `↑ moved from N` marks a commit kebbie moved with `kebbie order move`.

The keys sit under a rule at the bottom and stay there. A window taller than the screen scrolls the parts above them, keeping the selected commit in view.

| Key | Action |
| --- | --- |
| `↑` `↓`, `j` `k` | Select a commit and read its full message. |
| `PageUp` `PageDown`, the wheel | Scroll the window, by ten lines or three, to read the card or a long message. |
| `e` | Edit the selected commit's message. |
| `s` | Squash all commits into one. Claude drafts the message in the background, ready to edit. Needs two or more commits. |
| `y` | Push every commit shown and close the task. |
| `p` | Push the commits up to the selected one and keep the task open. With subtasks and a pushable run, the run, whichever commit is selected. |
| `esc`, `q` | Close without pushing. |

In the message editor: `ctrl-s` saves, `ctrl-o` opens your `$EDITOR`, `esc` goes back.

Only messages and squashes change here; the code stays identical. A commit that arrives after the window opened stops the push.

### The inbox

Opened by `i` at the first question, or by `s` on a task at that task's question. It shows one question at a time.

- The title reads `question 2/3 · plan · <task>`, with `plan` for a plan to approve and `ask` for a plain question.
- In a window 120 columns wide or more, the questions also list on the left under `QUESTIONS`, set off by a bar, with the open one highlighted. A click on one opens it.
- The question's text comes first, in bold.
- A plan question then shows its plan under a `PLAN` header, which gives how far down the plan you have scrolled, as a percentage, and `changed since asked` when the vault note changed after the question was asked. The plan fills the height that is left.

The answers sit on one row at the bottom under an `ANSWER` header, each as its key and a short name, the selected one highlighted. The line under the row says what the selected answer means, with `recommended` on the one the task recommends. A click on an answer selects it.

- A plan question offers `a` approve, `c` approve with a comment, `r` ask for changes, `x` reject with a comment and `w` in my own words.
- A plain question numbers its options `1` to `9` and adds `w` in my own words.
- An answer that takes a comment opens the comment box in place of the row, under the answer's name and meaning; the plan gives up only the box's rows. The comment is optional, except for "In my own words", where it is the answer.

| Key | Action |
| --- | --- |
| `tab`, `shift-tab` | The next or previous question, wrapping around. |
| `↑` `↓`, `j` `k` | Scroll the plan. `PageUp`, `PageDown` scroll by ten lines, also while typing a comment. |
| `←` `→` | Select an answer, wrapping around. |
| `⏎` | Send the selected answer, or open the comment box when it takes a comment. In the comment box, send the answer with the comment. |
| an answer's letter or number | Send that answer at once, or open the comment box when it takes a comment. |
| `o` | Open the plan file in the system viewer, as `o` opens a card's image. Offered for a plan kept as a file, not for a vault note. |
| `g` | Go to the asking task's Space. |
| `esc` | In the comment box, back to the answers. Otherwise, close. |
| `q` | Close, outside the comment box. |

### Messages panel

| Key | Action |
| --- | --- |
| `↑` `↓`, `j` `k` | Move through messages. |
| `⏎` | Read one message whole. |
| `esc`, `q` | Back to the tasks. |
| `m` | Hide the panel. |

### Bugs and wishes to send

Folded, the bugs and wishes are one row under the tasks, `! 2 bugs to send · ⏎ shows them`; `↓` from the last task selects it.

| Key | Action |
| --- | --- |
| `⏎` on the row | Open the list with its first one selected. A click on the selected row does the same. |
| `↑` `↓`, `j` `k` | Pick one. `↑` on the first, or on the folded row, goes back to the tasks. |
| `⏎` | Show the report as it would go out and ask `y / n`. |
| `n` | Keep the selected one on this machine only, without opening it. |
| `a` | Show every one in the list as it would go out, and ask once whether to send them all. |
| `esc`, `q` | Fold the list back into its row, with the selection on it; on the row, back to the tasks. |

In the report window `↓` and `pgdn` scroll, `y` sends once you have read to the end, `n` keeps it on this machine only, and `esc` decides later.

After `a`, `y` sends them all once you have read to the end, and `esc` decides later. They go in the background, paced to the endpoint's 5 per 5 minutes, each a line under messages; the board says which went and which failed when the last one has.

### Comments

| Key | Action |
| --- | --- |
| `↓` from the last task above the section, `↑` from the first task below it | Select the section: its first card, or the folded row. |
| `↑` `↓`, `j` `k` | Pick a card; past the first or the last, on to the task above or below. |
| `⏎` | On the folded row, open the section; on a card, open its window. |
| `y` | On a card whose reply is ready, copy the reply. |
| `esc`, `q` | Fold the section into its row, with the selection on it; on the row, back to the tasks. |

No task key acts on a comment: `d`, `p`, `x` and the rest do nothing on its row. In the window:

| Key | Action |
| --- | --- |
| `1` `2` `3`, `w`, `d`, `r`, `y` | The card's choices, as it lists them. Any other key keeps the window and says so. |
| `o` | Open the comment on GitHub, or show its address when there is no browser to open. |
| `←` `→` | The previous or next comment. |
| `↑` `↓`, `pgup` `pgdn` | Scroll the card. |
| `esc` | Close it. |

### The hub window

`H` opens it. See [See your board on your phone](../how-to/see-your-board-on-your-phone.md).

Not connected, it offers two hubs, the picked row highlighted:

```
╭ kebbie hub ──────────────────────────────────────────────────────────────────────────╮
│ See this board on your phone, end-to-end encrypted.                                  │
│                                                                                      │
│  k  kebbie hub     hub.kebbie.dev · free to connect and read                         │
│  o  your own hub   a server you run kebbie-hub on                                    │
│                                                                                      │
│ k kebbie hub · o your own · ↑↓ ⏎ choose · esc close                                  │
╰──────────────────────────────────────────────────────────────────────────────────────╯
```

- `k` or `o` chooses, and so do `↑` `↓` and `⏎`.
- Your own hub asks for the address of the server that runs kebbie-hub; a bare host name is tried over `https`. `⏎` tests it and shows `✓ answers as a kebbie hub` with `TLS ok`, or `plain http, on this machine only` for a hub on `localhost`. A second `⏎` saves it as [`hub.url`](configuration.md#keys) and connects.
- With `hub.url` set, `⏎` connects there.
- With `machine.name` unset, it asks for the name once, with the host's short name filled in.

While it waits for you to approve the machine, the window shows the machine and the hub with a QR code of the link. The text sits to the right of the code from its top row, or above it when the window is too narrow for both side by side:

```
╭ kebbie hub · connect ────────────────────────────────────────────────────────────────╮
│                               Connect mac to kebbie hub                              │
│   𜵊𜷝𜵇▌𜶪𜴴𜴝𜶘𜴃𜶃𜶚𜶁𜶁𜵄𜶞𜵊▝𜶖𜴶𜵊𜷝𜵇▌     hub.kebbie.dev                                         │
│   𜴬𜴪𜴨𜴍𜷈𜷄𜶏𜶇𜶻𜷄𜵊𜴌𜷈𜷈𜵇𜴓𜵎𜶃𜴊𜴬𜴪𜴨𜴍                                                            │
│   ▀𜷥𜵸𜴌𜵧𜴊𜵕▟𜵹𜴰𜵬𜴢𜶾𜷅𜴲𜵳𜶻𜶦𜶤𜶶𜶚𜶃𜵈     ⏎ open in browser · or scan with your phone            │
│   𜵭𜶁▆𜴰𜶯𜶁𜷍𜷛𜶫𜵅𜴃𜴇𜶉𜴿𜵢𜶽𜴺𜶠𜴅𜶮𜵮𜴪𜺨     Check the page shows  Z78K-XF53                        │
│   𜴬𜵛𜵴𜶝𜷍𜴥𜶴🯦𜴀𜶉𜴴𜷜𜵆𜶡𜷉🯧𜵱𜷓𜵆𜶔𜵣𜴩𜴊                                                            │
│   𜵅𜴘𜵊𜴌𜵦𜶏𜵛▄𜴠𜷌𜵊𜴌𜵦𜵍𜴼𜶦🯦𜴡𜵊𜴌𜵰𜴅▖     ⧗ Waiting for you · 9 min left                         │
│   𜵲𜷃𜴟𜴌▛𜴙𜵂𜶘🮅𜶀𜶏𜷊𜶐𜶈𜶒𜶑𜶣▀█𜶒𜴗𜶤▘                                                            │
│   🯧𜷟𜵀𜴌▚▆𜴰𜷐𜵆𜶋𜴓𜵆𜶆𜴖𜵎𜵋𜵅𜵌𜴛𜶽𜶁▜🯦                                                            │
│   𜵲𜶭𜵋𜴊𜷡𜴯𜴲𜵅𜷂𜶥𜶹𜶶𜵺𜷍𜴊𜵲𜶷𜶦𜷞𜶜𜶘𜴐𜵈                                                            │
│   𜵢𜴩𜴨𜵅𜶢𜶿𜴌𜶐𜶏𜴶𜵊𜴌𜷈▝𜵢𜶂𜴺𜷇𜵊𜴌𜷀𜴝                                                             │
│   ▌🮅𜴍▌𜵝𜴾𜶲𜶫𜵆𜴒𜶃𜷆𜴕𜶓𜷃𜺨𜶃𜷌𜷠🮅𜴬𜴒▘                                                            │
│   🮂🮂🮂𜺨𜺨𜺫🮂 𜺨🮂𜺫𜺨𜺨🮂🮂𜺫  𜺨🮂𜺫🮂                                                             │
│                                                                                      │
│                                                                                      │
│ No browser here? Scan the code, or open                                              │
│ hub.kebbie.dev/connect?code=Z78K-XF53#…                                              │
│                                                                                      │
│ ⏎ open · c copy link · esc stop                                                      │
╰──────────────────────────────────────────────────────────────────────────────────────╯
```

The window takes the board's whole width and height for the code; a board too small for even the stacked layout shows the rest without it.

The QR code:

- It is drawn in the terminal's own colours without a background. The blocks are the light modules and the quiet zone of four modules around it, and the terminal's background shows through as the dark modules, so on a dark theme the code reads the right way round, dark on light. Every module is square on a cell twice as tall as wide.
- In Ghostty (herdr inside Ghostty too, and over SSH from Ghostty), kitty and WezTerm, which draw block characters themselves, it is drawn in octant blocks, two modules across and four down a character.
- In any other terminal it is drawn in half blocks, one module across and two down a character, which every font draws. The code is then twice as wide and twice as tall; a pane too small for it shows the card without the code.
- `KEBBIE_QR=octant` or `KEBBIE_QR=halfblock` picks one.

While it waits:

- Nothing opens by itself. `⏎` opens the link in your browser, where the board sits and not over SSH, and the line `⏎ open in browser · or scan with your phone` becomes `✓ Opened in your browser`. Without a browser there, neither the line nor `⏎ open` shows.
- The code is the one the page shows; approve the machine only when they match. The time left counts down.
- The link is shown up to its `#`: what follows is the secret that ties the approving browser to this machine.
- `c` copies the whole link, through `pbcopy`, `wl-copy` or `xclip` where the board sits and through the terminal (OSC 52) over SSH. Over SSH it also shows the whole link, in case the terminal did not take it.
- `esc` stops waiting.

Connected, it looks like this:

```
╭ kebbie hub ──────────────────────────────────────────────────────────────────────────╮
│ ● mac on hub.kebbie.dev   board sent 12 s ago                                        │
│                                                                                      │
│ Sign in with your passkey on any phone or browser                                    │
│ to see this board:  hub.kebbie.dev                                                   │
│                                                                                      │
│ n rename · l leave the hub · esc close                                               │
╰──────────────────────────────────────────────────────────────────────────────────────╯
```

- The first line names the machine by its name and the hub without its scheme. While `[machine] name` differs from the name the hub has, it shows the new name with `renaming on the hub…` until the keeper's next post renames it.
- The dot is green with how long ago the board went out, and amber before the first post.
- It is red with why the board did not go out and how long ago on the line under it: a hub that has not enabled answering for your account, with its link, a hub that can't be reached, or what the hub answered.
- A machine that holds no account key, because it was approved by its typed code or joined with a join code, posts no board. The dot is red, the line under it says no browser can read the board, and in place of where to sign in the window says to connect it from the link, `kebbie hub leave`, then `kebbie hub connect`.

| Key | Action |
| --- | --- |
| `n` | Rename this machine: the field holds `[machine] name`, and `⏎` writes it to `config.toml`. |
| `l` | Leave the hub, after `y`: the hub removes the machine and its board, and the keys, the token and the account key go. The command is `kebbie hub leave`. |
| `esc`, `q` | Close the window. |

A new name is checked as [`machine.name`](configuration.md#keys) is. A `config.toml` that is a link is not written through; the window says where to set it. The keeper sends the new name to the hub on its next post.

## Areas page

![The Areas page: areas with their repos, actions and vault log](../assets/clips/areas-page.webp)

Press `2` to open it and `1` to go back. It needs the vault: it lists the vault's system notes with their repos. `kebbie areas` prints the same.

It reads a snapshot of the vault where the vault server answers one, which works with a vault hosted on a server, and the local vault index where it does not. When the vault states `watch`, the keeper follows the vault with one long-lived `watch` and writes each change into the snapshot as it comes; otherwise it fetches the snapshot every 20 seconds, or the full board does while the keeper is off. The status line's `vault index:` says how long ago the snapshot last changed. A snapshot holds the last 50 lines of each log note.

- Areas group into **needs you** (main ahead of origin, commits since the last release, or a task waiting on you), **active** and **quiet**.
- Each shows its running tasks, open notes and backlog, main against origin, the last tag and commits since, and the last vault log line.
- Areas without a repo on this machine still list.
- An area's repo is the system note's `path`, else where the vault's hooks saw it, else a checkout named after the area.

`⏎` lists an area's actions, `←` `→` move into the detail to pick one, and each letter works directly:

| Key | Action | Offered when |
| --- | --- | --- |
| `g` | Open the main checkout's Space. | The repo is on this machine. |
| `n` | New task in this repo, told the area's notes. | The repo is on this machine. |
| `a` | Let the lead push every task here whose reviewer passed its current code, or take that back. | The repo is on this machine. |
| `b` | Ask the lead to refresh this area's backlog. | Always. |
| `l` | Show the area's vault log, newest first. | Always. |
| `p` | Push main as you, after showing the commits, on `y`. | Main is ahead of origin. |
| your keys | The repo's own `[[actions]]`, on any key the page does not use. | Set in its repo config. See [Configuration](configuration.md#actions). |

`esc` and `q` close the page's windows; at the top they act as on the Work page.

## Routines page

Press `3` to open it. It lists every routine from config.toml and the repo configs, one row each: its last run's outcome, its name, its schedule, and how its last run stands or when it runs next.

- Routines whose findings wait for you come first, paused ones last.
- The selected routine shows on the right: what it does in your words, its repo, what it may do and its last few runs.
- In a narrow pane the list stands alone; `⏎` opens the selected routine on the whole width and `esc` goes back.

| Mark | Last run |
| --- | --- |
| `⚠` | Found something. Peach while its task is open and waits for you, and counted on the tab. |
| `◐` | Running. |
| `✓` | Found nothing and closed itself. |
| `✗` | Failed to start, or its task closed before it reported. |
| `‖` | The routine is paused. |

A run is an ordinary task. It shows on the Work page like any other, marked `routine`, and waits under Needs you when it found something.

| Key | Action |
| --- | --- |
| `⏎` | Options for the selected routine: go to its last run's task, show the report, run now, edit, pause or resume, delete. |
| `n` | New routine: five steps, then the table it adds to config.toml. |
| `g` | Run it now, unless its last run's task is still open. |
| `e` | Edit it: the same five steps with its values. |
| `p` | Pause it, or resume it. |
| `s` | The last run's report: its task's sign-off card, or the run's outcome and summary. |
| `D` | Delete it, after asking. |

`n` asks, one step at a time:

1. What it should do, in your words: this is its prompt. `ctrl-e` opens it in your editor.
2. Which repo it runs in, from the checkouts under your repo folders; type to filter.
3. When: `07:00` every day, `mon 08:00` every week, or `every 6h`.
4. What it may do: Report, Plan, Fix, or Fix and push (the lead pushes the fix once reviewed).
5. Its name. The id comes from the name.

Each step takes the mouse too: a click selects a repo or what it may do, a second click on it goes on, the hints under a step (`⏎ next`, `esc cancel`, and `y`, `g` and `←` on the table) work as keys, and the wheel moves through the list.

Then it shows the `[[routines]]` table it will add. Nothing is written before `y`; `g` adds it and starts a run at once, `←` goes back.

- A new routine always goes to config.toml, as `kebbie routine add` adds it.
- Editing, pausing and deleting change the routine in the file it is in, a repo config's in that repo config, and keep the rest of the file as it was.
- The page reads the files again every 15 seconds, so a change made by hand shows within a minute.

With no routines yet, the page explains what a routine is and offers three to start from: a health check, a dependency bump and keeping CI green.

## Settings page

Press `,` to open it, or run `kebbie config edit` for the same page on its own. It lists every key [`kebbie config`](configuration.md) prints, by section on the left, with its value, the layer it comes from and, under the selected key, what it does.

| Tab | Shows and writes |
| --- | --- |
| This machine | `~/.config/kebbie/config.toml`, without the keys a repo config alone takes. |
| Repo: `<repo>` | `~/.config/kebbie/repos/<repo>.toml` for the selected task's repo, without the keys marked config.toml only. |

| Layer | Means |
| --- | --- |
| `default` | No file sets it; kebbie's built-in value applies. |
| `machine` | config.toml sets it. |
| `repo <repo>` | The repo config sets it, and it wins over config.toml. |
| `team link` | The repo config is a link into a team repo, and the team's file sets it. |

| Key | Action |
| --- | --- |
| `↑` `↓` | Select a key. |
| `←` `→` | The previous or next section. |
| `space` | Turn a `true` or `false` key on or off. |
| `⏎` | Change the key: a pick list for a choice, `task.profile` among them, from the profiles there are, the value in place for text, numbers and lists (`a, b` or `["a", "b"]`). |
| `d` | Remove the key from the tab's file, so the layer below applies. |
| `tab` | Switch between This machine and the repo. |
| `/` | Search the keys by name and text; `⏎` keeps the search, `esc` clears it. |
| `esc` | Back to the board, or quit `kebbie config edit`. |

- A pick asks its question in the form the new-routine steps use: what the answers mean under it, the one in effect marked `now`, as an answer of its own when the list does not offer it, such as a Codex effort the model's list leaves out, so `⏎` keeps it, `↑` `↓` and `⏎` or a click to choose, `esc` or a click beside it to close.
- Each change is checked before it is written, and a value kebbie would refuse writes nothing and says why.
- A write changes only that key's line, keeping the file's comments and order, and goes to the tab's file.
- After each write, kebbie doctor's check of that file shows on one line, such as `doctor: ok, config.toml parses`.

### Profiles

The Profiles section, on the This machine tab, lists every [profile](configuration.md#profiles) `kebbie profiles` lists, in two groups:

- **FROM KEBBIE**: `task`, `reviewer` and `ops`. They come with kebbie and follow its updates. A model, effort or role you set on one goes in your own `~/.config/kebbie/profiles/<name>.toml`, which an update never writes.
- **YOURS**: every other file in `~/.config/kebbie/profiles`. An update never writes there.
- **DOES NOT LOAD**: each file in the folder that does not load, in red, with why when it is selected, and `⏎` opens it. A task cannot start with it until it loads; a `task.toml`, `reviewer.toml` or `ops.toml` that does not load puts kebbie's own profile here.

Each row shows the profile's name, its agent, model and effort, and its role. In a window too narrow for the role column it gives way, and the selected row still names the role. A model shown dim is the agent's own default, Claude's from `~/.claude/settings.json` and Codex's from `~/.codex/config.toml`. A `⚠` marks a Codex profile whose model Codex's own list for your account does not offer, and the line under the page names it when that profile is selected; otherwise that line sums the profile up, its role and the file your settings on it are in.

The selected row shows its role in its two layers: kebbie's own role, from the installed kebbie, which an update replaces, and the profile's role file, which is yours and is read after kebbie's. A role file can be shared: several profiles may name the same one.

| Key | Action |
| --- | --- |
| `m` | Pick the profile's model. For Codex, from Codex's own list for your account, `~/.codex/models_cache.json`, the list Codex's `/model` shows; for Claude, `opus`, `sonnet`, `haiku` or `fable`. Either way also the agent's default, or a model id you type. |
| `e` | Pick its effort: the levels Codex lists for the profile's model, or Claude Code's five, or the agent's default. Codex's default is `model_reasoning_effort` in `~/.codex/config.toml` when it is set, otherwise the model's own. |
| `r` | On one of yours, pick its role: the one it has, any other role file in the folder with the profiles that share it, a new empty file named after the profile, a copy of its role to change for it alone, or none. `o` opens the selected file. On one of kebbie's, a menu: `s` shows kebbie's role read-only in `$PAGER` (`less` without one), `a` adds your lines in `~/.config/kebbie/profiles/<name>.md` and opens them, naming the other profiles that read that file when one is already there. A click on a choice runs it. |
| `n` | Add a profile, in seven steps and a check: its name, what it is for (Tasks or Review), the profile it starts from, its agent, model, effort and role. `y` writes it, `g` writes it and opens its role. The agent step offers the agents setup chose, Claude Code alone selected when Codex was not, and opens on the agent of the profile it starts from, and a click on an answer selects it, a second click goes on. On one of kebbie's, `n` in the `r` menu starts from it. |
| `⏎` | Open the profile's file in `$VISUAL` or `$EDITOR`. |
| `d` | Remove `model` and `effort` from the profile, so the agent's defaults apply. |
| `x` | Delete one of yours, after a `y` / `n` question: its file goes, and its role file too unless another profile reads it. |

- A pick writes only `model`, `effort` or `role` in the profile's file, keeping its other lines and comments, and a value the profile would not load with writes nothing.
- A new role file or a copy takes a name no file has, as `designer.md` or `designer-2.md`, so it never overwrites one, and opens in your editor. Leaving a role file, or picking none, keeps the file.
- On a profile from kebbie, a pick writes `~/.config/kebbie/profiles/<name>.toml` with that key alone, and kebbie's role and rules stay as they ship. `d` removes that file again when nothing else is in it.
- A new profile takes the profile it starts from's `about`, `for`, `skills`, `permission_mode`, rules, plugins and `[sandbox]`, never a role: its role is a new empty file, a copy of the start's role file, a role file already there, shared with the profiles that read it, or none, and kebbie's own role comes first either way. A Codex profile leaves out what only Claude Code reads, and the check says which. Nothing is written until `y` or `g`, and a file that would not load is not written.
- A profile whose agent setup did not choose, such as a Codex profile copied from another machine, shows ⚠ and `Codex is not set up: kebbie setup codex`. `m`, `e` and `r` still edit its file, but `n` does not offer it as a start, `task.profile` does not offer it, and `kebbie start --profile` refuses it with the same words.
- `x` keeps kebbie's profiles, a profile an open task runs on, named with that task, one `task.profile` names in config.toml or a repo config, one a routine starts its tasks with, and the one `lead.profile` names. A role file goes with the profile only when no other file in the folder names it, one that does not load included.
- A review profile, the `reviewer` and each of yours with `kind = "review"`, takes only an agent, a model, an effort and a role, and its kebbie role is the review skill, `skills/review/SKILL.md`. It starts from another review profile, and is never offered as a task's start or as `task.profile`.
- The row of the profile [`review.profile`](configuration.md#review-and-docs) names shows the model `kebbie review` runs: `review.model` when it is set, with its layer, ahead of the profile's own. `m` on it writes `review.model` in config.toml, and its default removes the key; `e` sets the profile's effort.
- `review.profile` and `review.second` under Review & docs are picks of the review profiles whose agent setup chose, each with its agent, model and effort; `review.second` adds None and Always, and leaves out the profile `review.profile` names.
- A task already running keeps its model and effort; the next start takes the new ones.
- Codex's list is read, never written. Without it, `m` offers Codex's default and a model you type.
- A change to a key from a `team link` asks first: `t` changes the team's file for everyone who links it, `o` makes the link a file of this machine's own with the change.
- `task.rules_skill` and `worktree.setup_skill` offer kebbie's skill, your skill by name, a Markdown file by path, or a copy of kebbie's skill in `~/.claude/skills/<name>` to edit, which the key then names.
- `report.inbox_token` is never shown; set it in config.toml by hand.

## Keys in config.toml

`[keys]` in config.toml moves a key, as `action = "key"`. It lives in config.toml only; a repo config refuses it.

```toml
[keys]
inbox = "I"
new = "N"
```

The actions are `work_page`, `areas_page`, `routines_page`, `settings_page`, `hide_chat`, `messages`, `hub`, `help`, `inbox`, `new`, `go`, `chat`, `review`, `card`, `server`, `worker`, `send_review`, `permit`, `rebase`, `follow`, `tidy`, `push`, `try` (the older name `stage` still reads), `done`, `dismiss_ci`, `rerun_ci`, `park`, `reset`, `drop`, `start_at`, `refresh`, `away`, `update`, `restart_lead`, `filter`, `area_refresh` (`b` on the Areas page), `log` (`l` on the Areas page), `edit` (`e`), `pause` (`p`) and `delete` (`D`) on the Routines page, `item_options` (`o` in the Backlog column), and `close` (`q`).

- The key is one plain character.
- `esc`, `ctrl+c`, `⏎`, the arrows, `j` `k` and `tab` stay where they are, and so do the keys inside a window or a text field.
- A key another action already uses where both act is refused, naming that action. Moving two actions onto each other's keys in the same table works.
- An entry that breaks a rule is ignored; `kebbie config` lists every action with its key and says why an entry was ignored.
- An entry for `reports`, from an older config, is read without a word and changes nothing: the bugs and wishes to send open with `⏎` on their row.

## Mouse

| Action | Effect |
| --- | --- |
| Click a row | Select it. Click again for its options. |
| Click an option, a commit or a key in the status line | Use it. |
| Click the messages line | Open it. |
| Click a folded row, the parked line or the bugs and wishes to send | Select it. Click again to open it. |
| Click a bug to send | Select it. Click again to show its report. |
| Click a comment | Select it. Click again to open its window. |
| Click a row in Next | Select it. Click again to open it. |
| Scroll | Move through whatever is under the pointer. |
| Click outside a window | Close it, except the routine steps, which keep what you typed. |
