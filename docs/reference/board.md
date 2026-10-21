# Board

![The Areas page: areas with their repos, actions and vault log](../assets/clips/areas-page.gif)

The board runs in the lead Space, as a popup over any Space with `prefix+t`, or in the current pane with `kelpie board`. `?` on the board lists every key.

## Layout

| Part | Shows |
| --- | --- |
| Status line, top | The page tabs, `1 Work  2 Areas  3 Routines`, with `⚠N` on Routines while N routine runs wait for you with findings. Counts of tasks that need you, are working, are to review, are ready to push and wait for CI; waiting questions; plan usage; `↻ N stale` while Claude sessions wait to be restarted; `chat hidden` while the lead's chat is hidden; `↑ kelpie <version> · U updates` when a newer version is out. |
| Work, left | Tasks from every repo with a worktree pool, grouped. A task's halves in other repos sit next to each other. |
| Backlog, right | An ONGOING section of vault project notes in progress (status active or draft) when the vault is on, then the items from the configured sources, by priority. |
| Messages, bottom | What sessions tell each other, newest last. |

In a narrow pane the two columns stack, and the status line keeps only the counts that are not zero, without their words, so the page tabs still fit.

A task row shows its commits, uncommitted and untracked files, and how far it is behind the default branch. Behind is information only: push rebases first, `e` rebases now, and a task you keep rebased on main catches up by itself. The open task says `follows main` while that is on. Under it, a `you:` line gives your next step from the task's last report; while Claude waits on a prompt of its own, such as a permission or trust question, it says `answer Claude's prompt in its Space · g goes there`. While its dev server runs, a `web` line shows the server's address as a link. While a push waits for the task's Claude to test the files the default branch changed under it, the row and the open task say `push waits for tests · you`, or `· lead` when the lead pushed; kelpie pushes once the task reports ready with the same commits.

The selected task opens in place with its review progress, its question and options, its report, its latest test run, and a warning about files changed in a main checkout outside any task.

The `tests` line shows the last test command the task's Claude ran, as kelpie recorded it (see [`kelpie evidence`](commands.md#kelpie-evidence-name---json)):

| Line | Means |
| --- | --- |
| `✓ cargo test · 493 passed · at a1b2c3d`, green | It passed on the code the branch holds now, and the worktree has no uncommitted changes. |
| `○ cargo test · 493 passed · at a1b2c3d, code changed since`, yellow | It passed, but a commit changed the code after it, or the worktree has changes it did not run on. |
| `✗ cargo test failed · 2 failed · at a1b2c3d`, red | The latest run failed. |

No line means no test run is recorded. The wide panel shows the same line.

## Groups

| Group | Contains |
| --- | --- |
| Needs you | A question on screen; a task that asked for you; a task whose CI failed after a push, marked `CI failed: <workflow> · <job>`; a task an API error stopped that kelpie will not resume, marked `stopped by <reason>: <Claude Code's words>`; a task Claude finished that you have not looked at; a ready task whose commits the lead has not approved, marked "rules not checked by the lead". Open questions first. |
| Working | Claude is mid-turn, or kelpie resumes it after an API error: `retrying after overload (2/4, next 21:14)`, `out of allowance until 17:00`, then `resumed after …` until Claude answers. See [When the API stops a task](../explanation/lead-and-away-mode.md#when-the-api-stops-a-task). |
| To review | Sent to the reviewer, waiting for a pass on its current code. |
| Ready to push | Reported ready, approved by the lead, passed by the reviewer when sent to review, with no uncommitted changes to tracked files. How far it is behind the default branch does not matter. |
| CI running | Pushed, with its worktree kept until every workflow run for the pushed commit is green; then it closes as done. The row names the runs, and the open task links each run's page. See [`kelpie ci`](commands.md#kelpie-ci---json). |
| Waiting for work | Idle, or blocked on something outside you. |

Between turns, the task's last report decides its group. A CI watch, or kelpie resuming the task after an API error, decides it until the task reports again.

## Keys

`⏎` opens the options for the selected row. Each option's letter also works directly. Every key that is a plain character can be moved in config.toml's `[keys]`; help, hints and messages then show the key you chose. See [Keys](#keys-in-configtoml).

### Everywhere on the Work page

| Key | Action |
| --- | --- |
| `⏎` | Options for the selected task or backlog item. |
| `↑` `↓`, `j` `k` | Move. |
| `←` `→` | Switch between Work and Backlog. |
| `1`, `2`, `3` | The Work page, the Areas page, the Routines page. |
| `tab` | Move focus to the lead's chat, showing it first when it is hidden. |
| `h` | Hide the lead's chat, or show it again: the board takes the whole width while the chat runs on. Only on the board in the lead Space. See [Hiding the chat](../explanation/lead-and-away-mode.md#hiding-the-chat). |
| `i` | The inbox: the questions waiting on you, opened at the first. Works on the Areas page too. |
| `m` | Messages: once shows them on one line, again opens them, again hides them. |
| `n` | New task: a name, then an optional prompt. Starts in the background. In the Backlog column, a new item. |
| `/` | Filter the selected column: type part of a name, `⏎` keeps the filter, `esc` clears it. |
| `R` | Ask the lead to refresh the backlog. |
| `A` | Away mode on or off. |
| `U` | Run `kelpie update`. |
| `?` | Every key. |
| `esc` | One step back: close a window, a menu or the filter. At the top, close the board when it is a popup; on the full board it does nothing. |
| `q` | Close the current view, like `esc`. At the top, close the popup; on the full board it says that `ctrl+c` twice quits. |
| `ctrl+c` | Twice within 2 seconds quits the board. The first press says so. |

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
| `v` | Send to review; on a task in review, withdraw it. | It has commits and Claude is not mid-turn. |
| `e` | Rebase onto the default branch now, in the background, without pushing; the result shows when it is done. A conflict goes to its Claude, as with [`kelpie land`](commands.md#kelpie-land-name). | It is behind the default branch and Claude is not mid-turn. |
| `t` | Ask its Claude to tidy its commits by the rules skill. | Claude is not mid-turn. |
| `p` | Push: open the push window; `y` or `p` there rebases onto the default branch and pushes. | Claude is not mid-turn and it is not waiting for the reviewer. |
| `d` | Done: close a task you already pushed elsewhere and free its worktree, also one under CI running or with failed CI you choose to leave. Asks first. | Claude is not mid-turn. |
| `a` | Let the lead push it, or take that back. `done`, `drop` and `reset` take it back too. | It is a task. |
| `u` | Keep it rebased on main, or switch that off; `●` marks it on. When main moves, the keeper rebases it, never pushes it. See [`kelpie follow`](commands.md#kelpie-follow-name-onoff). Off by default; `done`, `drop` and `reset` switch it off. | It is a task. |
| `z` | Reset: its work goes, its backlog item returns. Asks first. | Always. |
| `x` | Drop. Asks first; on uncommitted work or unpushed commits, asks again to force. | Always. |

kelpie types into a Claude's input box only when the box is empty. While you have a draft there, a message to that Claude waits and goes in once the box is clear. After typing, kelpie checks the conversation for it; when Claude has not taken it, kelpie presses Enter again after 5, 10 and 20 seconds, and after 40 seconds the board says it was not delivered.

### In the Backlog column

| Key | Action | Offered when |
| --- | --- | --- |
| `g` | Start the item as a task, with the item as its prompt. Asks for a name, suggested from the title. | On an item. |
| `g` | Go to the task working on this note, or resume the note as a new task. | On an ONGOING note. |
| `R` | Ask the lead to refresh the backlog. | On an item. |
| `n` | Add an item to the local backlog: a title, then a priority. | `"local"` is a source. |

### The card window

Opened by `s` on a task with a sign-off card and no open question, in a narrow board and a wide one alike. The title names the task; the first line says whether the card is current at its commit, or written at an earlier one with the code changed since.

The card's text follows in parts, each under a header on a rule. A paragraph that opens with a short label and a colon, such as `To check:` or `Please confirm:`, starts a part of its own headed by that label; the text before the first label is headed `CARD`, and a card with no label is one `CARD` part. `**bold**` shows bold.

The images show one at a time, each as large as its space allows:

- In a window 120 columns wide or more, the text sits on the left with an `IMAGES` part listing every image by file name and alt text, and the selected image fills the right side. Over it, `1/3 <file name>  <alt text>` says which image it is; the file name is a link to the file. A click on an image in the list selects it.
- In a narrower window the parts stack, and the lower part of the window shows the selected image across the window's width under `IMAGE 1/3 <file name>`.

How an image shows depends on the terminal:

- With a graphics protocol (Kitty, Sixel or iTerm2) the image is sharp. Inside herdr kelpie uses the Kitty protocol, which herdr passes on to a Kitty-capable outer terminal such as Ghostty or Kitty.
- With only halfblocks the image is a coarse preview in text cells, with `preview · f larger · o opens the image` under it.
- With images off (`KELPIE_IMAGES=off`, or the board not on a terminal), or a file that does not decode as an image, only its name shows.

`KELPIE_IMAGES` picks the protocol by hand; see [environment](environment.md).

| Key | Action | Shown when |
| --- | --- | --- |
| `←` `→` | The previous or next image, wrapping around. | The card has two images or more. |
| `↑` `↓`, `j` `k` | Scroll the text. `PageUp`, `PageDown` scroll by ten lines. | Always. |
| `f` | Fullscreen: the selected image alone over the whole board, as large as it fits. `←` `→` go between the card's images; `esc`, `q` or `f` go back to the card. | The card has an image and images are on. |
| `o` | Open the selected image in the system viewer: `open` on macOS, `xdg-open` on Linux. In fullscreen it opens the image shown. | The card has an image, the board does not run over SSH (`SSH_CONNECTION` or `SSH_TTY`), and on Linux `DISPLAY` or `WAYLAND_DISPLAY` is set. |
| `p` | Go on to the push window. | Always. |
| `esc`, `q` | Close. | Always. |

In a narrow board the selected task also shows the card under its row: whether it is current, its first two lines of text, and how many images it has.

### The push window

Opened by `p`. The window is in four parts, each under a header on a rule:

- `CARD`: the task's sign-off card, marked current at its commit or written before the code changed. Its images are listed by file name; `s` on the board shows them.
- `TESTS`: the latest recorded test run, as on the task's row. A run before a clean rebase, a reworded message or a squash still counts as current.
- `COMMITS`: the commits that will land, with their count.
- `MESSAGE`: the selected commit's full message, set off by a bar so it does not read as part of the card.

The keys sit under a rule at the bottom.

| Key | Action |
| --- | --- |
| `↑` `↓`, `j` `k` | Select a commit and read its full message. |
| `e` | Edit the selected commit's message. |
| `s` | Squash all commits into one. Claude drafts the message in the background, ready to edit. Needs two or more commits. |
| `y` | Push every commit shown and close the task. |
| `p` | Push the commits up to the selected one and keep the task open. |
| `esc`, `q` | Close without pushing. |

In the message editor: `ctrl-s` saves, `ctrl-o` opens your `$EDITOR`, `esc` goes back.

Only messages and squashes change here; the code stays identical. A commit that arrives after the window opened stops the push.

### The inbox

Opened by `i` at the first question, or by `s` on a task at that task's question. It shows one question at a time; the title reads `question 2/3 · plan · <task>`, with `plan` for a plan to approve and `ask` for a plain question. In a window 120 columns wide or more, the questions also list on the left under `QUESTIONS`, set off by a bar, with the open one highlighted; a click on one opens it.

The question's text comes first, in bold. A plan question then shows its plan under a `PLAN` header, which gives how far down the plan you have scrolled, as a percentage, and `changed since asked` when the vault note changed after the question was asked. The plan fills the height that is left.

The answers sit on one row at the bottom under an `ANSWER` header, each as its key and a short name, the selected one highlighted. The line under the row says what the selected answer means, with `recommended` on the one the task recommends. A plan question offers `a` approve, `c` approve with a comment, `r` ask for changes, `x` reject with a comment and `w` in my own words; a plain question numbers its options `1` to `9` and adds `w` in my own words. A click on an answer selects it.

An answer that takes a comment opens the comment box in place of the row, under the answer's name and meaning; the plan gives up only the box's rows. The comment is optional, except for "In my own words", where it is the answer.

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

## Areas page

Press `2` to open it and `1` to go back. It needs the vault: it lists the vault's system notes (`Areas/*.md`) with their repos. `kelpie areas` prints the same.

Areas group into **needs you** (main ahead of origin, commits since the last release, or a task waiting on you), **active** and **quiet**. Each shows its running tasks, open notes and backlog, main against origin, the last tag and commits since, and the last vault log line. Areas without a repo on this machine still list.

An area's repo is the system note's `path`, else where the vault's hooks saw it, else a checkout named after the area.

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

Press `3` to open it. It lists every routine from config.toml and the repo configs, one row each: its last run's outcome, its name, its schedule, and how its last run stands or when it runs next. Routines whose findings wait for you come first, paused ones last. The selected routine shows on the right: what it does in your words, its repo, what it may do and its last few runs. In a narrow pane the list stands alone; `⏎` opens the selected routine on the whole width and `esc` goes back.

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

Then it shows the `[[routines]]` table it will add. Nothing is written before `y`; `g` adds it and starts a run at once, `←` goes back. A new routine always goes to config.toml, as `kelpie routine add` adds it. Editing, pausing and deleting change the routine in the file it is in, a repo config's in that repo config, and keep the rest of the file as it was. The page reads the files again every 15 seconds, so a change made by hand shows within a minute.

With no routines yet, the page explains what a routine is and offers three to start from: a health check, a dependency bump and keeping CI green.

## Keys in config.toml

`[keys]` in config.toml moves a key, as `action = "key"`. It lives in config.toml only; a repo config refuses it.

```toml
[keys]
inbox = "I"
new = "N"
```

The actions are `work_page`, `areas_page`, `routines_page`, `hide_chat`, `messages`, `help`, `inbox`, `new`, `go`, `chat`, `review`, `card`, `server`, `worker`, `send_review`, `permit`, `tidy`, `push`, `done`, `reset`, `drop`, `refresh`, `away`, `update`, `filter`, `area_refresh` (`b` on the Areas page), `log` (`l` on the Areas page), `edit` (`e`), `pause` (`p`) and `delete` (`D`) on the Routines page, and `close` (`q`).

- The key is one plain character.
- `esc`, `ctrl+c`, `⏎`, the arrows, `j` `k` and `tab` stay where they are, and so do the keys inside a window or a text field.
- A key another action already uses where both act is refused, naming that action. Moving two actions onto each other's keys in the same table works.
- An entry that breaks a rule is ignored; `kelpie config` lists every action with its key and says why an entry was ignored.

## Mouse

| Action | Effect |
| --- | --- |
| Click a row | Select it. Click again for its options. |
| Click an option, a commit or a key in the status line | Use it. |
| Click the messages line | Open it. |
| Scroll | Move through whatever is under the pointer. |
| Click outside a window | Close it, except the routine steps, which keep what you typed. |
