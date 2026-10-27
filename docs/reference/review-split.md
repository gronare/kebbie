# Review split

A herdr split next to Claude for reading the work under review: kebbie's own view on top, with the task's steps, the selected step's commit message and its files, and the selected file's diff in `nvim` below. It opens with `r` on the board or `/kebbie:review-split [base or what to review]` in Claude.

## Scope and base

| Item | Value |
| --- | --- |
| Files listed | The work under review, in the order of Claude's numbered review guide. Other changes follow, marked `other`. |
| Base inside a worktree | Where the branch left the remote's default branch, so the split shows that task's work, committed or not. |
| Base elsewhere | The argument to `/kebbie:review-split`, when given. |

Claude takes the files it changed in this session. For work from elsewhere, it takes what the unpushed commits, uncommitted changes and project notes say belongs to it, and asks when that is unclear. Asking for everything lists every change against the base.

## Layout

| Review width | Panes |
| --- | --- |
| Under 120 columns | The view on top, nvim below it. |
| 120 columns or more | The view on the left, nvim on the right. |

The width is the view's and nvim's together. Resizing the split, or the window, moves nvim to the other side when it crosses 120 columns; nvim keeps running, with its diff, cursor and comments. Zooming one of the two panes fills the tab with that pane alone, so a zoomed view moves nothing.

The view, top to bottom:

| Part | Shows |
| --- | --- |
| Header | `review <task> · since <default branch>`, then `N accepted` in green and `N to accept` in yellow for a task's commits. |
| Tabs | `all`, `↑ N pushed`, then each step with `task.steps` on, or each unpushed commit without, numbered, with its mark. |
| Message | The selected commit's message: subject and short sha, then the body. On `all`, which steps or commits the diff covers. `m` hides it. |
| Files | `+N -N path` for the selected step's commit, or for the whole branch on `all`. `✓ read` marks a read file. |
| Keys | The keys for the panel in focus, or the result of the last action. |

| Mark | Step or commit |
| --- | --- |
| `○` | Planned, muted. It has no commit to select. |
| `◐` | In progress, muted. It has no commit to select. |
| `●` | Committed, waiting for your accept. |
| `✓` | Committed and accepted. |

The panel in focus has a blue border. `tab` moves the focus from the tabs to the files to the diff and back to the tabs; `shift-tab` goes the other way. Going to the diff or back focuses the other herdr pane.

## Keys

| Where | Key | Action |
| --- | --- | --- |
| view | `←` `→`, `h` `l` | The previous or next step. Steps without a commit are skipped; before the first is `all`. |
| view | `↑` `↓`, `j` `k` | The previous or next file. nvim shows it at once. |
| view | click | A tab selects that step, a file shows that file. The wheel scrolls the files. |
| view | `a` | Accept the selected step's commit, then go to the next one waiting; on an accepted one, withdraw the acceptance. |
| view | `r` | Mark the file read, or unread, and go to the next file. |
| view | `/` | Filter the files: words to keep, `!word` to hide. `Enter` keeps the filter, `Esc` clears it. |
| view | `m` | Hide or show the commit message, leaving the tabs. The review remembers it until it closes. |
| view | `Enter` | Go to the diff. |
| view | `tab`, `shift-tab` | The next or previous panel. |
| view | `?` | Show every key. |
| view | `q` | Close the review: the view and nvim. |
| diff | drag, `V` | Select lines. |
| diff | `c` | Comment on the line, or on the selection. |
| diff | `y` | Copy the selection to the clipboard; `yy` copies the line. |
| diff | `o` | Open the real file over the diff, at that line. |
| diff | `-` | Open the file's folder over the diff: neo-tree when installed, else netrw. |
| diff | `u` | Reload the diff after the file changed. |
| diff | `w` | Toggle line wrap. Wrap is on when the diff opens. |
| diff | `tab`, `shift-tab` | Back to the tabs, or to the files. |
| diff | `q` | Close the review. |
| opened file | `leader m` | Comment on the line or visual selection. |
| opened file | `ctrl-q` | Back to the diff. |

## Commits

| Selection | Files and diff |
| --- | --- |
| `all` | Every change since the base, uncommitted ones included. |
| One step or commit | Only that commit's files, and its own diff against its parent (`sha^..sha`). |

`a` runs `kebbie accept <task> <sha>`, or `kebbie accept <task> <sha> --withdraw` for an accepted commit. Accepting is yours alone: kebbie accepts it from the review pane because the pane runs under your own shell, and refuses it to a Claude. The marks are kebbie's own, the same as `kebbie steps --json` with steps and `kebbie accepted <task> --json` without. The view checks every two seconds and reads them again, with the files, once HEAD, the default branch, `git status`, a changed file or kebbie's marks moved, so an acceptance given or withdrawn elsewhere shows here too. Outside a task, the tabs are the unpushed commits and `a` accepts nothing.

## Behaviour

| Event | Result |
| --- | --- |
| The review opens | On the first step waiting for your accept, else on `all`. |
| A comment | Typed into Claude's input box as `path:line — text`, not submitted. Several pile up until you press Enter there. In one commit's diff, the line is that commit's: `path:line in abc1234 — text`. |
| Saving a file opened with `o` or `-` | Reported to Claude as `path:lines — edited by me during the review` when you go back to the diff. Unsaved edits ask to be saved or discarded first. |
| Files change on disk | The view's list follows within two seconds. An open diff of the whole branch marks itself `changed on disk`; `u` reloads it. |
| A read file changes | It loses its `✓ read`. |
| A file read in one commit | Read in that commit, and in `all` while no later change touched it. |
| A selected commit is rebased or amended away | The view goes back to `all`. |
| A copy over SSH | With no clipboard tool where nvim runs, `y` copies through OSC 52: herdr passes it to the terminal it runs in, which puts it on that machine's clipboard. Your terminal must allow OSC 52 writes; Ghostty does by default. |
| The view's pane closes from herdr | nvim closes with it. |
| Claude's pane closes | The review's state goes when the next review opens. |

## Opening automatically

With the plugin option `auto_open` on, the first file edit by a task's Claude in a session opens the split next to it without taking focus. Sessions in a main checkout never open one on their own. Closing the pane keeps it closed for that session. See [Configuration](configuration.md#claude-code-plugin-option).

## History split

`/kebbie:history-split` opens `tig` in a split next to the pane, for the branch's history.
