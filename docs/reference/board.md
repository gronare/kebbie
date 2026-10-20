# Board

The board runs in the lead Space, as a popup over any Space with `prefix+t`, or in the current pane with `kelpie board`. `?` on the board lists every key.

## Layout

| Part | Shows |
| --- | --- |
| Status line, top | Counts of tasks that need you, are working, are to review and are ready to push; waiting questions; plan usage; `↻ N stale` while Claude sessions wait to be restarted; `↑ kelpie <version> · U updates` when a newer version is out. |
| Work, left | Tasks from every repo with a worktree pool, grouped. A task's halves in other repos sit next to each other. |
| Backlog, right | An ONGOING section of vault project notes in progress (status active or draft) when the vault is on, then the items from the configured sources, by priority. |
| Messages, bottom | What sessions tell each other, newest last. |

In a narrow pane the two columns stack.

A task row shows its commits, changed files and how far it is behind the default branch. Under it, a `you:` line gives your next step from the task's last report. While its dev server runs, a `web` line shows the server's address as a link.

The selected task opens in place with its review progress, its question and options, its report, and a warning about files changed in a main checkout outside any task.

## Groups

| Group | Contains |
| --- | --- |
| Needs you | A question on screen; a task that asked for you; a task Claude finished that you have not looked at; a ready task whose commits the lead has not approved, marked "rules not checked by the lead". Open questions first. |
| Working | Claude is mid-turn. |
| To review | Sent to the reviewer, waiting for a pass on its current code. |
| Ready to push | Approved by the lead, and passed by the reviewer when sent to review. |
| Waiting for work | Idle, or blocked on something outside you. |

Between turns, the task's last report decides its group.

## Keys

`⏎` opens the options for the selected row. Each option's letter also works directly.

### Everywhere on the Work page

| Key | Action |
| --- | --- |
| `⏎` | Options for the selected task or backlog item. |
| `↑` `↓`, `j` `k` | Move. |
| `←` `→` | Switch between Work and Backlog. |
| `1`, `2` | The Work page, the Areas page. |
| `tab` | Move focus to the lead's chat. |
| `q` | The question window. |
| `m` | Messages: once shows them on one line, again opens them, again hides them. |
| `s` | Start a new task: a name, then an optional prompt. Starts in the background. |
| `R` | Ask the lead to refresh the backlog. |
| `A` | Away mode on or off. |
| `U` | Run `kelpie update`. |
| `?` | Every key. |
| `esc` | Close a window, or the board when it is a popup. |
| `ctrl-c` | Quit the board. |

### On a task

| Key | Action | Offered when |
| --- | --- | --- |
| `q` | Answer its question. | It has an open question. |
| `g` | Go to its Space. On a task whose Space or Claude is gone, restart it and resume its conversation. | Always. |
| `p` | Message its Claude. | Always. |
| `r` | Open the review split next to its Claude. | Always. |
| `w` | Start or stop the dev server. | `worktree.dev_server` is set. |
| `b` | Start or stop the worker. | `worktree.dev_worker` is set. |
| `v` | Send to review; on a task in review, withdraw it. | It has commits and Claude is not mid-turn. |
| `t` | Ask its Claude to tidy its commits by the rules skill. | Claude is not mid-turn. |
| `l` | Push: rebase, then open the push window. | Claude is not mid-turn and it is not waiting for the reviewer. |
| `d` | Done: close a task you already pushed elsewhere and free its worktree. | Claude is not mid-turn. |
| `a` | Let the lead push it, or take that back. `done`, `drop` and `reset` take it back too. | It is a task. |
| `z` | Reset: its work goes, its backlog item returns. Asks first. | Always. |
| `x` | Drop. Asks first; on uncommitted work or unpushed commits, asks again to force. | Always. |

### In the Backlog column

| Key | Action | Offered when |
| --- | --- | --- |
| `g` | Start the item as a task, with the item as its prompt. Asks for a name, suggested from the title. | On an item. |
| `g` | Go to the task working on this note, or resume the note as a new task. | On an ONGOING note. |
| `R` | Ask the lead to refresh the backlog. | On an item. |
| `a` | Add an item to the local backlog: a title, then a priority. | `"local"` is a source. |

### The push window

Opened by `l`. The task's sign-off card sits above the commits, marked when the code changed after it was written.

| Key | Action |
| --- | --- |
| `↑` `↓`, `j` `k` | Select a commit and read its full message. |
| `e` | Edit the selected commit's message. |
| `s` | Squash all commits into one. Claude drafts the message in the background, ready to edit. Needs two or more commits. |
| `y` | Push every commit shown and close the task. |
| `p` | Push the commits up to the selected one and keep the task open. |
| `esc` | Close without pushing. |

In the message editor: `ctrl-s` saves, `ctrl-o` opens your `$EDITOR`, `esc` goes back.

Only messages and squashes change here; the code stays identical. A commit that arrives after the window opened stops the push.

### The question window

Opened by `q`. Questions list on the left; the selected one's plan or detail is in the middle.

| Key | Action |
| --- | --- |
| `tab`, `shift-tab` | Move between the list, the plan, the options and the comment field. |
| `↑` `↓`, `j` `k` | Move in the list or the options, or scroll the plan. |
| `PageUp`, `PageDown` | Scroll the plan by ten lines. |
| any character | Type into the comment field. |
| `⏎` | Answer with the selected option and the comment. With "In my own words" selected, the comment is the answer. |
| `g` | Go to the asking task's Space. |
| `esc` | Close. |

A plan question offers approve, approve with a comment, ask for changes, and reject with a comment.

### Messages panel

| Key | Action |
| --- | --- |
| `↑` `↓`, `j` `k` | Move through messages. |
| `⏎` | Read one message whole. |
| `esc` | Back to the tasks. |
| `m` | Hide the panel. |

## Areas page

Press `2` to open it and `1` to go back. It needs the vault: it lists the vault's system notes (`Areas/*.md`) with their repos. `kelpie areas` prints the same.

Areas group into **needs you** (main ahead of origin, commits since the last release, or a task waiting on you), **active** and **quiet**. Each shows its running tasks, open notes and backlog, main against origin, the last tag and commits since, and the last vault log line. Areas without a repo on this machine still list.

An area's repo is the system note's `path`, else where the vault's hooks saw it, else a checkout named after the area.

`⏎` lists an area's actions, `←` `→` move into the detail to pick one, and each letter works directly:

| Key | Action | Offered when |
| --- | --- | --- |
| `g` | Open the main checkout's Space. | The repo is on this machine. |
| `s` | Start a task in this repo, told the area's notes. | The repo is on this machine. |
| `a` | Let the lead push every task here whose reviewer passed its current code, or take that back. | The repo is on this machine. |
| `b` | Ask the lead to refresh this area's backlog. | Always. |
| `l` | Show the area's vault log, newest first. | Always. |
| `u` | Push main as you, after showing the commits, on `y`. | Main is ahead of origin. |
| your keys | The repo's own `[[actions]]`. | Set in its repo config. See [Configuration](configuration.md#actions). |

## Mouse

| Action | Effect |
| --- | --- |
| Click a row | Select it. Click again for its options. |
| Click an option, a commit or a key in the status line | Use it. |
| Click the messages line | Open it. |
| Scroll | Move through whatever is under the pointer. |
| Click outside a window | Close it. |
