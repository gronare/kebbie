# The board

The board shows every task by what it needs from you, next to your backlog. It lives in the lead Space, and `prefix+t` opens it as a popup over any Space. `kelpie board` runs it in the current pane.

## Layout

- **Status line** at the top: how many tasks need you, are working, are to review or are ready to push, how many questions wait, your plan usage, `↻ N stale` while Claude sessions wait to be restarted, and `↑ kelpie <version> · U updates` when a newer version is out.
- **Work** on the left: your tasks, grouped. It lists tasks from every repo with a worktree pool, not only those with a Space open. A task's halves in other repos sit next to each other.
- **Backlog** on the right: items from your configured sources, by priority. See [Backlog](backlog.md). In a narrow pane the two columns stack.
- **Messages** at the bottom: what sessions tell each other, newest last.

Each task row shows its commits, changed files and how far it is behind the default branch. Under it, a `you:` line says what you should do next, taken from the task's last report. While its dev server runs, a `web` line shows the server's address as a link that also opens over ssh.

The selected task opens in place with its review progress, the question it asked with its options, its report, and a warning about files changed in a main checkout outside any task.

## Groups

| Group | What is in it |
| --- | --- |
| Needs you | A question on screen, a task that asked for you, a task Claude finished that you have not looked at, a task that is ready but whose commits the lead has not approved yet. Open questions come first. |
| Working | Claude is mid-turn. |
| To review | Sent to the [reviewer](review.md) and waiting for a pass on its current code. |
| Ready to push | Approved by the lead (and passed by the reviewer when sent to review). |
| Waiting for work | Idle, or blocked on something outside you. |

Between turns, the task's last report decides its group.

## Keys

| Key | What it does |
| --- | --- |
| `⏎` | The options for the selected task or backlog item. Each option has a letter that also works directly. |
| `↑` `↓` / `j` `k` | Move. |
| `←` `→` | Switch between Work and Backlog. |
| `1` `2` | The Work and Areas pages. See [Areas](areas.md). |
| `tab` | Move focus to the lead's chat. |
| `q` | The questions waiting on you. |
| `m` | Messages: once shows them on one line, again opens them, again hides them. |
| `s` | Start a new task: a name, then an optional prompt. It starts in the background. |
| `g` | Go to the task's Space. On a task whose Space is gone, restart it. On a backlog item, start it as a task. |
| `p` | Message the task's Claude. |
| `r` | Open the task's review split next to its Claude. |
| `w` | Start or stop the task's dev server (`worktree.dev_server`). |
| `b` | Start or stop the task's worker (`worktree.dev_worker`). |
| `v` | Send the task to review. On a task in review, withdraw it. |
| `t` | Ask the task's Claude to tidy its commits by the rules skill. |
| `l` | Push: rebase, then show the card and commits. |
| `a` | Let the lead push this task itself, or take that back. In the Backlog column, add an item to the local backlog. |
| `d` | Done: close a task you already pushed elsewhere. |
| `z` | Reset the task: its work goes and its backlog item returns. Asks first. |
| `x` | Drop the task. Asks first. |
| `R` | Ask the lead to refresh the backlog. |
| `A` | Away mode on or off. See [The lead](lead.md#away-mode). |
| `U` | Run `kelpie update` without leaving the board. |
| `?` | Every key. |
| `esc` | Close a window, or the board when it is a popup. |
| `ctrl-c` | Quit the board. |

Options show only where they apply: `w` and `b` need their command configured, `v` needs commits, and `v`, `t`, `l` and `d` wait while Claude is mid-turn. A task in review offers no `l` until its reviewer passes it.

### The push window

| Key | What it does |
| --- | --- |
| `↑` `↓` | Select a commit and read its full message. |
| `e` | Edit the selected commit's message. `ctrl-s` saves, `esc` goes back, `ctrl-o` opens your editor. |
| `s` | Squash all commits into one. Claude drafts the message in the background, ready to edit. |
| `y` | Push exactly the commits shown and close the task. |
| `p` | Push the commits up to the selected one and keep the task open. The rest of the branch continues on top. |

Only messages and squashes change here; the code stays identical. A commit that arrives after the window opened stops the push.

### The question window

Questions from tasks list on the left, the selected one's plan or detail in the middle. `tab` moves between the list, the plan, the options and the comment field. Pick an option, add a comment or write your own answer, and press `⏎`. `g` goes to the asking task's Space. `PageUp` and `PageDown` scroll the plan.

### Messages

Once the messages panel is open, `↑` `↓` move through it, `⏎` reads one message whole, `esc` goes back to the tasks, and `m` hides it.

## Mouse

Click a row to select it, click again for its options. Click an option, a commit or a key in the status line to use it. Click the messages line to open it. Scroll to move through whatever is under the pointer. Click outside a window to close it.

## On the command line

`kelpie status [--json]` gives the same picture as the board, for scripts and for the lead.
