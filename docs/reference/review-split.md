# Review split

A herdr split next to Claude with the files of the work under review: a file list in `fzf` and unified diffs in `nvim`. It opens with `r` on the board or `/kelpie:review-split [base or what to review]` in Claude.

## Scope and base

| Item | Value |
| --- | --- |
| Files listed | The work under review, in the order of Claude's numbered review guide. Claude takes the files it changed in this session, or, for work from elsewhere, what the unpushed commits, uncommitted changes and project notes say belongs to it, and asks when that is unclear. Asking for everything lists every change against the base. |
| Base inside a worktree | Where the branch left the remote's default branch, so the split shows that task's work, committed or not. |
| Base elsewhere | The argument to `/kelpie:review-split`, when given. |

## Keys

| Where | Key | Action |
| --- | --- | --- |
| list | typing | Filter. `!word` hides matches; `!spec/ ` is prefilled. |
| list | `Enter` | Open the file's unified diff in nvim. |
| list | `ctrl-r` | Mark the file read, or unread. |
| list | `ctrl-h` | Show or hide read files. |
| list | `ctrl-o` | Show or hide changes outside the work under review. |
| list | `?` | Show every key in the preview pane. |
| list | `ctrl-c` | Close the split. `Esc` does nothing in the list. |
| diff | `c` | Comment on the line, or on the visual selection. |
| diff | `o` | Open the real file over the diff, at that line. |
| diff | `-` | Open the file's folder over the diff: neo-tree when installed, else netrw. |
| diff | `u` | Reload the diff. |
| diff | `w` | Toggle line wrap. |
| diff | `r` | Mark the file read and go back to the list. |
| diff | `q`, `Esc` | Back to the list. |
| opened file | `leader m` | Comment on the line or visual selection. |
| opened file | `ctrl-q` | Back to the diff. |

## Behaviour

| Event | Result |
| --- | --- |
| A comment | Typed into Claude's input box as `path:line — text`, not submitted. Several pile up until you press Enter there. |
| Saving a file opened with `o` or `-` | Reported to Claude as `path:lines — edited by me during the review` when you go back to the diff. Unsaved edits ask to be saved or discarded first. |
| Files change on disk | The list reloads, keeping filter and cursor. An open diff marks itself `changed on disk`; `u` reloads it. |
| A read file changes | It comes back into the list. |

## Opening automatically

With the plugin option `auto_open` on, the first file edit by a task's Claude in a session opens the split next to it without taking focus. Sessions in a main checkout never open one on their own. Closing the pane keeps it closed for that session. See [Configuration](configuration.md#claude-code-plugin-option).

## History split

`/kelpie:history-split` opens `tig` in a split next to the pane, for the branch's history.
