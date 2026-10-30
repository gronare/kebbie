# Board

What the board shows and every key on it. The board runs in the lead Space, as a popup over any Space with `prefix+t`, or in the current pane with `kebbie board`. `?` on the board lists every key.

A board you quit in the lead Space starts again the next time you press `prefix+space`, also when you are already in the lead Space; once the board runs, `prefix+space` takes you back to where you were.

## Layout

| Part | Shows |
| --- | --- |
| Status line, top | The page tabs, `1 Work  2 Areas  3 Routines  , Settings`, then counts and markers, below. |
| Lead line, under the status line | Whether the lead still runs an old kebbie, while it counts as stale. |
| Context line, under it | While the lead's context is 85% full or more: `The lead's context is N% full · L to restart it fresh`. |
| Hub line, under it | While the hub refuses the board for a lapsed subscription: `Your kebbie hub subscription isn't active: <link> · H hub`. |
| Work, left | Tasks from every repo with a worktree pool, grouped, in the order of [`board.groups`](configuration.md). |
| Backlog, right | Vault notes in progress, under ONGOING, and the items from the configured sources, by priority. |
| Messages, bottom | What sessions tell each other, newest last. |

The status line shows:

| Marker | Means |
| --- | --- |
| `⚠N` on Routines | N routine runs wait for you with findings. |
| Counts | Tasks that need you, are working, are in review, are ready to push and wait for CI, and waiting questions. |
| `◆ N to accept`, yellow | N commits of tasks working in steps wait for you to accept them. |
| `! N bugs to send` | Bugs in kebbie you ran into wait for you to send them to its maintainers, on their row under Work. |
| Plan usage | How much of the plan is used. |
| `↻ N stale` | Claude sessions wait to be restarted. |
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
- Under the tasks: the [bugs to send](#bugs-to-send), folded into one row until you open them, and the main checkouts with files changed outside any task.
- A folded row, the parked line, the comments or the bugs to send, is selected like a task: `⏎` or a click on it once selected opens it, and `esc` on a row inside folds it back with the selection on the row.
- After an update's recheck fixed some bugs, `✓ Fixed in kebbie <version>: N bugs you ran into` with their titles. `i` shows the list and clears the line.

Backlog: ONGOING lists vault project notes in progress (status active or draft) when the vault is on. It sits above the items, below them or nowhere, by [`board.backlog.ongoing`](configuration.md).

### A task row

A task row shows its commits, uncommitted and untracked files, and how far it is behind the default branch. Behind is information only: push rebases it when its turn in the push line comes, `e` rebases now, and a task you keep rebased on main catches up by itself.

| Line or mark | Means |
| --- | --- |
| `follows main` | On the open task, while it is kept rebased on main. |
| `bypass` | Its Claude runs in bypass permissions mode. The open task says `bypass permissions: Claude asks for no permission`. |
| `you:` | Your next step, from the task's last report. |
| `web` | While its dev server runs: the server's address as a link. |
| `push waits for tests · you` | A push waits for the task's Claude to test the files the default branch changed under it. `· lead` when the lead pushed. kebbie pushes once the task reports ready with the same commits. |

#### A task in steps

With [`task.steps`](configuration.md) on and steps listed, the task's steps not pushed yet sit under its row, one line each, with the step's number and title:

```
◐ Switch the payment provider to Stripe        billing · 1 to accept
    ↑ 1 pushed
    ● 2 Charge through a PaymentGateway        committed · rules ✓
    ◐ 3 Add Stripe as the gateway              in progress
    ○ 4 Remove the old payment client
   accept 2 · r review
```

| Line or mark | Means |
| --- | --- |
| `N to accept`, yellow | On the row, in place of its commit count: N committed steps wait for you to accept them. |
| `↑ N pushed`, muted | N steps are on the default branch already. |
| `○`, muted | Planned. |
| `◐ … in progress`, blue | Its Claude is at work on it. |
| `● … committed`, yellow | Committed, waiting for you to accept it, followed by the task's rules check in short: `· rules ✓` passed, `· rules ?` not checked yet, `· checking rules` while the reviewer or the lead checks, `· rules: N breaks` when it found some. |
| `✓ … accepted`, green | Committed, and you accepted it. |
| `accept N · r review` | Under the steps: the first step waiting for you. `r` opens the review, and [`kebbie accept`](commands.md#kebbie-accept-name-sha) accepts it. |

A step to accept leaves the task in the group its phase gives it, under Working while it builds the next step. The state word always shows: a long title is shortened at a word with `…` to make room, and only when the title would drop below 12 columns does the rules part shorten (`· 2 breaks`, `· checking`) and then go. Without steps, the row is as above. See [Work in steps](../how-to/work-in-steps.md).

While Claude waits on a prompt of its own, such as a permission or trust question, the `you:` line says `answer Claude's prompt in its Space (g) or through the lead`. `g` takes you there, and the lead has the question and its options in its chat. See [Claude's own prompts in a task](../explanation/lead-and-away-mode.md#claudes-own-prompts-in-a-task).

The selected task opens in place with its review progress, its question and options, its report, its latest test run, and a warning about files changed in a main checkout outside any task.

The `tests` line shows the last test command the task's Claude ran, as kebbie recorded it (see [`kebbie evidence`](commands.md#kebbie-evidence-name---json)). The wide panel shows the same line. No line means no test run is recorded.

When the task's Claude runs a command that looks like a test run but is not in the list kebbie records, such as `script/test` or `make test`, the row says `script/test is not recorded as a test run: add it to task.test_commands`. Once you add it to [`task.test_commands`](configuration.md#keys), the next run is recorded and the note goes.

| Line | Means |
| --- | --- |
| `✓ cargo test · 493 passed · at a1b2c3d`, green | It passed on the code the branch holds now, and the worktree has no uncommitted changes. |
| `○ cargo test · 493 passed · at a1b2c3d, code changed since`, yellow | It passed, but a commit changed the code after it, or the worktree has changes it did not run on. |
| `✗ cargo test failed · 2 failed · at a1b2c3d`, red | The latest run failed. |

## Groups

| Group | Contains |
| --- | --- |
| Needs you | Tasks waiting on your answer or action, open questions first. |
| Working | Tasks at work since their prompt, their last report or your answer. |
| In review | Reported ready and sent to the reviewer, waiting for a pass on its current code. The row says who acts next: `review running · waits on the reviewer`, or, with findings and its Claude idle, `withdraw with v to push without a pass`. |
| Ready to push | Reported ready with its commits, and pushes under way. |
| CI running | Pushed, with its worktree kept until every workflow run for the pushed commit is green; then it closes as done. A cancelled run waits for a later run on the default branch to pass. |
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
- Messages held behind a draft in its input box.
- A task whose commits are on the default branch that kebbie does not close by itself, marked `landed on main · d closes it` beside a question, or `landed on main, but its worktree has uncommitted changes or a stash` (see [`kebbie done`](commands.md#kebbie-done-name)).
- A task that reported needs-you, or ready with nothing committed.
- A task whose turn ended without a report even after kebbie asked for one.
- A task whose CI failed after a push of only some of its commits, marked `CI failed: <workflow> · <job> · <step>`, and `its Claude says outside the code: <reason>` once its Claude has said so. `C` dismisses it.
- A push under way whose Claude reported needs-you.
- A push that left its repo's push line unpushed, marked `out of the push line: <why>`: its Claude did not report ready within 30 minutes, it reported needs-you, or it has commits nobody checked since you chose push. Push again and it joins the end of the line.
- A push or close kebbie could not finish.
- A pushed task whose CI gh has not been able to read for 15 minutes, its row marked `gh cannot read it: <error>` meanwhile.
- A task an API error stopped that kebbie will not resume, marked `stopped by <reason>: <Claude Code's words>`.

### Working

- Started with a prompt, or at work since its last report or since you answered it.
- A pushed task whose CI is green and that kebbie closes once its note is written, marked `pushed · kebbie closes it`.
- A task kebbie resumes after an API error, marked `retrying after overload (2/4, next 21:14)`, `out of allowance until 17:00`, then `resumed after …` until Claude answers. See [When the API stops a task](../explanation/lead-and-away-mode.md#when-the-api-stops-a-task).

### Ready to push

A ready row is marked by its rules check:

- `reviewer checking rules` while the reviewer checks its commits against the rules skill;
- `rules: N breaks` when it found some;
- `lead checking rules` when the reviewer gave no verdict and the lead checks them;
- nothing of the rules once approved, and the count of uncommitted files when you edited after the report.

How far it is behind the default branch does not matter. A push under way stays here, marked `pushing for you · <what it waits for>`:

- its turn, as `2nd in line after <title>` while other pushes of its repo go first, and `kebbie pushes it now` at the head;
- the tests for files the default branch changed under it, a conflict, or a fix for CI that failed after the push.

Each repo pushes one task at a time, in the order you pushed them, and rebases a task only when its turn comes. A head whose Claude works keeps its place while the line waits for it, for up to 30 minutes. See [Pushing](../explanation/trunk-based.md#one-push-line-per-repo).

A task on local main stays here too, marked `on local main · waiting for your push`, until main is pushed and it closes as landed.

### CI running

A report its Claude writes meanwhile keeps it here unless it says needs-you or blocked. The row names the runs, and the open task links each run's page. See [`kebbie ci`](commands.md#kebbie-ci---json).

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
- `w In my own words` reopens with what you type in place of the proposal, under the same commit message.
- The answer to a question goes to a fresh reading.
- Once a commit carrying the `Addresses` line is on the default branch, the card turns to its reply, naming the fix's sha.
- `y` copies with the clipboard program (`pbcopy`, `wl-copy` or `xclip`), or over ssh through the terminal (OSC 52).

kebbie never posts a reply.

## Keys

`⏎` opens the options for the selected row. Each option's letter also works directly. Every key that is a plain character can be moved in config.toml's `[keys]`; help, hints and messages then show the key you chose. See [Keys](#keys-in-configtoml).

### Everywhere on the Work page

| Key | Action |
| --- | --- |
| `⏎` | Options for the selected task or backlog item. |
| `↑` `↓`, `j` `k` | Move. |
| `←` `→` | Switch between Work and Backlog. |
| `1`, `2`, `3`, `,` | The Work page, the Areas page, the Routines page, the [Settings page](#settings-page). |
| `tab` | Move focus to the lead's chat, showing it first when it is hidden. |
| `h` | Hide the lead's chat so the board takes the whole width while the chat runs on, or show it again. Only on the board in the lead Space. See [Hiding the chat](../explanation/lead-and-away-mode.md#hiding-the-chat). |
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

The [bugs to send](#bugs-to-send) are folded into one row at first. While any bug waits, the header counts them, such as `! 2 bugs to send`. The board remembers whether the list shows. See [Send it to kebbie's maintainers](../how-to/report-a-problem-with-kebbie.md#send-it-to-kebbies-maintainers).

`U` shows a bar while kebbie builds or downloads. `esc` hides the window while the update goes on, and `U` opens it again. Done, it shows what's new and fixed from kebbie's release notes and what restarts; `d` shows the details, any other key closes it. See [What an update shows](../explanation/updates-and-restarts.md#what-an-update-shows).

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
| `s` | Answer its question: the questions window, at this task's question. | It has an open question. |
| `g` | Go to its Space. On a task whose Space or Claude is gone, restart it and resume its conversation. | Always. |
| `c` | Chat: message its Claude. | Always. |
| `r` | Open the review split next to its Claude, or go to the one already open in its Space. | Always. |
| `s` | Show the sign-off card. | It has a card and no open question. |
| `w` | Start or stop the dev server. | `worktree.dev_server` is set. |
| `b` | Start or stop the worker. | `worktree.dev_worker` is set. |
| `1`–`9` | Open one of its links in the browser, listed under LINKS. | It has that link. |
| `v` | Send to review; on a task in review, withdraw it. | It has commits and Claude is not mid-turn. |
| `e` | Rebase onto the default branch now, in the background, without pushing. | It is behind the default branch and Claude is not mid-turn. |
| `t` | Ask its Claude to tidy its commits by the rules skill. | Claude is not mid-turn. |
| `p` | Push: open the push window. On a task already in the line, take it out again, unpushed. | Claude is not mid-turn and it is not waiting for the reviewer. |
| `M` | Put on local main to try it; on a staged task, take it off again. | See below. |
| `C` | Dismiss the red CI of a pushed task whose failure is not the code's. Asks first. Yours alone. | The task's CI failed. |
| `d` | Done: close a task you already pushed elsewhere and free its worktree. Asks first. | Claude is not mid-turn. |
| `a` | Let the lead push it, or take that back. `done`, `drop` and `reset` take it back too. | It is a task. |
| `u` | Keep it rebased on main, or switch that off; `●` marks it on. | It is a task. |
| `Z` | Park it until later; on a parked task, bring it back now. | It is a task. |
| `z` | Reset: its work goes, its backlog item returns. Asks first. | Always. |
| `x` | Drop. Asks first; on uncommitted work or unpushed commits, asks again to force. | Always. |

- `1`–`9`: the links are listed by what each is: the running dev server, each CI run, a mock, a pull request or another address its report or card names, and its vault note, which opens in Obsidian. The highlighted link's whole address shows on the description line. Where the board has no browser, as over ssh, it says the address instead.
- `e`: the result shows when it is done. A conflict goes to its Claude, as with [`kebbie land`](commands.md#kebbie-land-name).
- `p`: `y` or `p` in the push window puts the task in its repo's push line, which rebases it onto the default branch and pushes it when its turn comes, at once when the line is empty. Taking it out does not work for a fix for red CI, whose commits are on the default branch already (`C` dismisses that). `e` is not offered while a task is in the line.
- `M` puts its commits on the main checkout's local default branch, unpushed, after the tasks staged before it, and the row says `on local main · waiting for your push`. Taking them off leaves the other staged tasks. To put on, the repo sets `worktree.stage_on_main`, it has commits, Claude is not mid-turn, it is not waiting for the reviewer and has no CI watch. To take off: always. See [`kebbie stage`](commands.md#kebbie-stage-name).
- `C` keeps the failure on record with the reason its Claude gave, and the task closes once its commits are on main.
- `d` also closes one under CI running or with failed CI you choose to leave. The keeper closes a landed task by itself; `d` is for the ones it leaves to you.
- `Z` opens `Park <task> until` with four rows, each with the time it means on the right: `Tomorrow`, selected (`sat 09:00`), so `Z` then `⏎` parks it until tomorrow at 09:00; `Monday` (`mon 09:00`), the coming Monday at 09:00, which on a Monday before 09:00 is that morning; `A date…` turns the window into a field for a day as [`kebbie start --at`](commands.md#kebbie-start-name-prompt) takes it (`fri`, `24 nov`, `2026-12-01`, `tomorrow 13:00`), a day alone at 09:00, showing the time it reads as you type (`thu 5 nov 09:00`, or `?`), where `⏎` parks and `esc` goes back to the choices; `I bring it back` keeps it until you press `Z` on it. `1` to `4` pick directly, `esc` cancels. See [Parked](#parked).
- `u`: when main moves, the keeper rebases it, never pushes it. Off by default; `done`, `drop` and `reset` switch it off. See [`kebbie follow`](commands.md#kebbie-follow-name-onoff).

### How messages reach a Claude

kebbie's messages reach a Claude that kebbie started on Claude Code 2.1.224 or later through that session's inbox socket. It shows in the conversation as one dim line from kebbie that ctrl+o expands. Claude reads it between tool calls or starts a turn when idle, and your draft is never touched.

- kebbie's notices that ask nothing of the lead are not delivered live: a task started, pushed or closed, a review that passed, a rebase or a red CI its task resolves itself, an update. They show in the message log and reach the lead with your next prompt to it, grouped by task.
- What asks the lead to act is delivered: findings to pass on, a rules check the reviewer left, a task's question of method, a push that failed, a prompt on a task's screen, a red CI no task took. While you talk with the lead (it is mid-turn, you prompted it in the last 3 minutes, or your text is in its input box) it is held, and goes as one message grouped by task once you have been quiet for 3 minutes; a prompt on a task's screen comes first. In away mode it goes at once. See [It waits while you talk with it](../explanation/lead-and-away-mode.md#it-waits-while-you-talk-with-it).
- A message that starts a lead turn ends with the line `(kebbie: work silently; end in Done / Going on / Needs you, or one line if nothing needs the user)`. The lead works without narrating its steps and closes the turn with what landed, what is going on per task and what needs you, or with one line such as "Noted: 3 updates, nothing for you." when nothing does.
- kebbie then checks the conversation for it as it does for a typed message. After 10 minutes without it, the board says it was not delivered; a rules check or a hand-over is held and sent again.
- kebbie starts the lead and its tasks with `crossSessionInbound` set to `accept`, so any process of your user on the machine can message them. That is the same trust as typing into their panes, and the sandbox, when on, limits what a task can reach.

For an older Claude Code, or when the inbox refuses a message, kebbie types it instead:

- It types into a Claude's input box only when the box is empty. While you have a draft there, a message to that Claude waits and goes in once the box is clear.
- After typing, kebbie checks the conversation for it. When Claude has not taken it, kebbie presses Enter again after 5, 10 and 20 seconds, and after 40 seconds the board says it was not delivered.

### In the Backlog column

| Key | Action | Offered when |
| --- | --- | --- |
| `g` | Start the item as a task, with the item as its prompt. Asks for a name, suggested from the title. | On an item. |
| `g` | Go to the task working on this note, or resume the note as a new task. | On an ONGOING note. |
| `⏎` | The note's card. | On an ONGOING note. |
| `l` | Start the item as a task later: a name, then a time. It waits under Queued until then, as with [`kebbie start --at`](commands.md#kebbie-start-name-prompt). | On an item. |
| `R` | Ask the lead to refresh the backlog. | On an item. |
| `n` | Add an item to the local backlog: a title, then a priority. | `"local"` is a source. |

The note's card shows its whole title, area, status and summary, and who works on it (machine, task and since when). It shows its task and whether it runs here, and its opening: the text before its first heading and its first two sections, from the local vault index.

- `↓` `↑` `pgdn` `pgup` scroll it.
- `g` or `⏎` does what `g` does on the row.
- `esc` closes it.
- A click on the selected note opens it too.

### The card window

Opened by `s` on a task with a sign-off card and no open question, in a narrow board and a wide one alike. The title names the task. The first line says whether the card is current at its commit, or written at an earlier one with the code changed since.

The card's text follows in parts, each under a header on a rule. A paragraph that opens with a short label and a colon, such as `To check:` or `Please confirm:`, starts a part of its own headed by that label. The text before the first label is headed `CARD`, and a card with no label is one `CARD` part. `**bold**` shows bold.

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
| `COMMITS` | The commits that will land, with their count. A commit you accepted is marked `✓`, green. With steps, each commit carries its step's mark and number, and the steps not committed yet follow, muted: `in progress, stays` or `planned`. |
| `MESSAGE` | The selected commit's full message, set off by a bar so it does not read as part of the card. |

With accepted commits, the window opens on the last of the accepted ones from the first commit on, marked `◂ through here`, and `p` reads `p push N accepted, the task keeps working` while it is selected. Without any, it opens on the first commit. While the task's Claude is working, `p` opens the window only once it has accepted commits from the first one on, and the window then pushes only those: `↑` `↓` stop at the last of them, and `y`, `e` and `s` wait until the task reports.

The keys sit under a rule at the bottom and stay there. A window taller than the screen scrolls the parts above them, keeping the selected commit in view.

| Key | Action |
| --- | --- |
| `↑` `↓`, `j` `k` | Select a commit and read its full message. |
| `PageUp` `PageDown`, the wheel | Scroll the window, by ten lines or three, to read the card or a long message. |
| `e` | Edit the selected commit's message. |
| `s` | Squash all commits into one. Claude drafts the message in the background, ready to edit. Needs two or more commits. |
| `y` | Push every commit shown and close the task. |
| `p` | Push the commits up to the selected one and keep the task open. |
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

### Bugs to send

Folded, the bugs are one row under the tasks, `! 2 bugs to send · ⏎ shows them`; `↓` from the last task selects it.

| Key | Action |
| --- | --- |
| `⏎` on the row | Open the list with its first bug selected. A click on the selected row does the same. |
| `↑` `↓`, `j` `k` | Pick a bug. `↑` on the first, or on the folded row, goes back to the tasks. |
| `⏎` | Show the report as it would go out and ask `y / n`. |
| `n` | Keep the selected bug on this machine only, without opening it. |
| `esc`, `q` | Fold the list back into its row, with the selection on it; on the row, back to the tasks. |

In the report window `↓` and `pgdn` scroll, `y` sends once you have read to the end, `n` keeps it on this machine only, and `esc` decides later.

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
- It is red with why the board did not go out and how long ago on the line under it: a lapsed subscription with its link, a hub that can't be reached, or what the hub answered.
- A machine that holds no account key, because it was approved by its typed code or joined with a join code, posts no board. The dot is red, the line under it says no browser can read the board, and in place of where to sign in the window says to connect it from the link, `kebbie hub leave`, then `kebbie hub connect`.

| Key | Action |
| --- | --- |
| `n` | Rename this machine: the field holds `[machine] name`, and `⏎` writes it to `config.toml`. |
| `l` | Leave the hub, after `y`: the hub removes the machine and its board, and the keys, the token and the account key go. The command is `kebbie hub leave`. |
| `esc`, `q` | Close the window. |

A new name is checked as [`machine.name`](configuration.md#keys) is. A `config.toml` that is a link is not written through; the window says where to set it. The keeper sends the new name to the hub on its next post.

## Areas page

![The Areas page: areas with their repos, actions and vault log](../assets/clips/areas-page.gif)

Press `2` to open it and `1` to go back. It needs the vault: it lists the vault's system notes (`Areas/*.md`) with their repos. `kebbie areas` prints the same.

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
| `⏎` | Change the key: a pick list for a choice, the value in place for text, numbers and lists (`a, b` or `["a", "b"]`). |
| `d` | Remove the key from the tab's file, so the layer below applies. |
| `tab` | Switch between This machine and the repo. |
| `/` | Search the keys by name and text; `⏎` keeps the search, `esc` clears it. |
| `esc` | Back to the board, or quit `kebbie config edit`. |

- Each change is checked before it is written, and a value kebbie would refuse writes nothing and says why.
- A write changes only that key's line, keeping the file's comments and order, and goes to the tab's file.
- After each write, kebbie doctor's check of that file shows on one line, such as `doctor: ok, config.toml parses`.
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

The actions are `work_page`, `areas_page`, `routines_page`, `settings_page`, `hide_chat`, `messages`, `hub`, `help`, `inbox`, `new`, `go`, `chat`, `review`, `card`, `server`, `worker`, `send_review`, `permit`, `rebase`, `follow`, `tidy`, `push`, `stage`, `done`, `dismiss_ci`, `park`, `reset`, `drop`, `start_at`, `refresh`, `away`, `update`, `restart_lead`, `filter`, `area_refresh` (`b` on the Areas page), `log` (`l` on the Areas page), `edit` (`e`), `pause` (`p`) and `delete` (`D`) on the Routines page, and `close` (`q`).

- The key is one plain character.
- `esc`, `ctrl+c`, `⏎`, the arrows, `j` `k` and `tab` stay where they are, and so do the keys inside a window or a text field.
- A key another action already uses where both act is refused, naming that action. Moving two actions onto each other's keys in the same table works.
- An entry that breaks a rule is ignored; `kebbie config` lists every action with its key and says why an entry was ignored.
- An entry for `reports`, from an older config, is read without a word and changes nothing: the bugs to send open with `⏎` on their row.

## Mouse

| Action | Effect |
| --- | --- |
| Click a row | Select it. Click again for its options. |
| Click an option, a commit or a key in the status line | Use it. |
| Click the messages line | Open it. |
| Click a folded row, the parked line or the bugs to send | Select it. Click again to open it. |
| Click a bug to send | Select it. Click again to show its report. |
| Click a comment | Select it. Click again to open its window. |
| Scroll | Move through whatever is under the pointer. |
| Click outside a window | Close it, except the routine steps, which keep what you typed. |
