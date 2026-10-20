# Review

kelpie gives you two kinds of review: a split where you read a task's changes and comment on them, and a read-only reviewer Claude that can gate the push.

## The review split

`r` on the board, or `/kelpie:review-split [base or what to review]` in Claude, opens a split next to Claude with the files of the work under review.

Claude works out the scope first: the files it changed in this session, or, for work from elsewhere, what the unpushed commits, uncommitted changes and project notes say belongs to it. It asks when that is unclear. The list shows those files in the order of Claude's numbered review guide; other changes in the repo are one key away. Ask for everything and you get every change against the base.

Inside a worktree, the base is where its branch left the remote's default branch, so the split shows exactly that task's work, committed or not.

### Keys

| Where | Key | What it does |
| --- | --- | --- |
| list | type | Filter. `!spec/ ` is prefilled to hide specs. |
| list | `Enter` | Open the file's unified diff in nvim. |
| list | `ctrl-r` | Mark the file read, or unread. |
| list | `ctrl-h` | Show or hide read files. |
| list | `ctrl-o` | Show or hide changes outside the work under review. |
| list | `?` | Show every key in the preview pane. |
| list | `ctrl-c` | Close the split. `Esc` does nothing here, so it cannot close it by accident. |
| diff | `c` | Comment on the line or visual selection. |
| diff | `o` | Open the real file over the diff, at that line. |
| diff | `-` | Open the file's folder over the diff (neo-tree if installed, else netrw). |
| diff | `u` | Reload the diff after the file changed. |
| diff | `w` | Toggle line wrap. |
| diff | `r` | Mark the file read and go back to the list. |
| diff | `q`, `Esc` | Back to the list. |
| opened file | `leader m` | Comment on the line or visual selection. |
| opened file | `ctrl-q` | Back to the diff. |

### Comments go to Claude

A comment is typed into Claude's input box as `path:line — text`, not submitted, so several pile up until you press Enter there.

Files opened with `o` or `-` are your usual nvim: search, jump, edit. When you go back to the diff, files you saved are reported to Claude as `path:lines — edited by me during the review`. Unsaved edits ask to be saved or discarded first.

### It follows the repo

The list reloads when files change, keeping your filter and cursor. An open diff marks itself `changed on disk` instead of moving under you; `u` reloads it. A read file stays out of the list until its contents change.

### Open it automatically

Turn on **Open the review pane automatically** in Claude Code under `/plugin` → Installed → kelpie → Configure, or install with `claude plugin install kelpie@kelpie --config auto_open=true`. The first file edit by a task's Claude then opens the split next to it without taking focus. Sessions in a main checkout never open one on their own. Close the pane from herdr and it stays closed for that session.

### History

`/kelpie:history-split` opens tig in a split for the branch's history.

## The reviewer gate

`kelpie review billing`, or `v` on the board, sends a task to review. It waits under To review, and its push with it, until the reviewer passes the code its HEAD makes.

The reviewer is a separate Claude (`claude -p`) in the task's worktree. It gets:

- the [`kelpie:review`](../skills/review/SKILL.md) skill as its instructions,
- the commits against the default branch,
- the prompt and later messages from the task's own conversation,
- your rules skill (`task.rules_skill`).

It only reads. Its tools are Read, Grep, Glob, and the shell for `git log`, `git diff` and `git show` alone, in `dontAsk` mode, with a hook that refuses every other command. It runs without your own settings, hooks or plugins. `review.model` picks its model.

Its verdict goes to `verdicts/<task>@<repo>.md` in the state directory: `verdict: ok` or `verdict: changes`, the sha it reviewed, then findings with `file:line`, bugs first. The lead gets a one-line summary and passes the findings that matter back to the task. The task's row shows `reviewed`, `review: N findings`, or `review stale` once its code changes.

### What keeps a pass

A clean rebase, a reworded message or a squash keep the pass, since the code is the same. A changed line does not. When a task in review reports ready on changed code, kelpie starts the reviewer again.

### Withdraw

`kelpie review --withdraw billing`, or `v` again on the board, takes a task out of review: the reviewer stops, its verdict goes, push is offered again and the lead is told. Only you can withdraw; neither the lead nor a task can.

### When to use it

Most tasks go straight to push after the lead's rules check. Send the ones where a second reading pays: money, data, migrations, security, concurrency, wide refactors. The reviewer's pass is separate from the lead's approval; a task in review shows under Ready to push only with both.
