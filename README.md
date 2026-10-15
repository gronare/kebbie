# kelpie

Herding coding agents in [herdr](https://herdr.dev). A kelpie is the Australian working dog that keeps a herd moving while the drover directs; here herdr holds the agents and kelpie keeps their work moving towards you.

Today kelpie reviews a task's changes in a split next to the Claude pane and sends your comments back to Claude without leaving the diff. Starting tasks in their own worktrees, a board over Linear and a backlog, and agent roles are on the way.

## Requirements

Claude Code running inside herdr, plus `fzf`, `nvim` 0.10 or later, `jq`, `git` and `curl` on `PATH`; `tig` for the history split. The scripts use POSIX `sh` and short flags, so they run with the BSD tools on macOS as well.

## Install

```bash
claude plugin marketplace add https://github.com/gronare/kelpie
claude plugin install kelpie@kelpie
```

## Review


`/kelpie:review-split [base or what to review]` opens a split with the files of the work under review. Claude works out the scope first: the files it changed in this session, or, for work from elsewhere, what the unpushed commits, uncommitted changes and any project notes say belongs to it, asking when that is unclear. The list shows those files in the order of Claude's numbered review guide; other changes in the repo are one key away. Ask for everything and you get every change against the base. Inside a git worktree, the review is based on where its branch left the remote's default branch, so it shows exactly that worktree's work, committed or not, however far the default branch has moved on.

| Where | Key | What it does |
| --- | --- | --- |
| list | type | filter; `!spec/ ` is prefilled to hide specs |
| list | Enter | open the file's unified diff in nvim |
| list | ctrl-r | mark the file read, or unread |
| list | ctrl-h | show or hide read files |
| list | ctrl-o | show or hide changes outside the work under review |
| list | ? | show every key in the preview pane |
| list | ctrl-c | close the split (Esc does nothing, so it cannot close it by accident) |
| diff | c | comment on the line or visual selection |
| diff | o | open the real file over the diff, at that line |
| diff | - | open the file's folder over the diff (neo-tree's float if installed, else netrw) |
| diff | u | reload the diff after the file changed |
| diff | w | toggle line wrap |
| diff | r | mark the file read and go back to the list |
| diff | q, Esc | back to the list |
| opened file | leader m | comment on the line or visual selection |
| opened file | ctrl-q | back to the diff |

Files opened with `o` or `-` show in a float over the diff, and everything else in it is your usual nvim: jump around, search, edit. When you go back to the diff, files you saved are reported to Claude as `path:lines — edited by me during the review`, and unsaved edits ask to be saved or discarded first.

A comment is typed into Claude's input box as `path:line — text`, not submitted, so several pile up until you press Enter there. A read file stays out of the list until its contents change; marks live in `~/.local/state/kelpie/`, one file per repo.

The list follows the repo by itself: it reloads when files change, keeping your filter and cursor. An open diff marks itself `changed on disk` instead of moving under you; `u` reloads it.

## Live review (optional)

With the plugin setting **Open the review pane automatically** on, Claude's first file edit in a session opens the review split next to it without taking focus. Files that were already changed before that edit count as other changes (behind ctrl-o); everything changed after it is in scope, newest first. Inside a git worktree, everything since the branch left the default branch is in scope. Close the pane from herdr and it stays closed for that session; `/kelpie:review-split` brings it back, or rescopes the open one to Claude's guide.

The setting is off by default. Turn it on in Claude Code under `/plugin` → Installed → kelpie → Configure, or at install:

```bash
claude plugin install kelpie@kelpie --config auto_open=true
```

## History

`/kelpie:history-split` opens tig in a split for the branch's history.

## License

MIT
