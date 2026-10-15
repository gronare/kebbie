# kelpie

Herding coding agents in [herdr](https://herdr.dev). A kelpie is the Australian working dog that keeps a herd moving while the drover directs; here herdr holds the agents and kelpie keeps their work moving towards you.

kelpie starts each task in its own git worktree with Claude, and reviews a task's changes in a split next to the Claude pane, sending your comments back to Claude without leaving the diff. A board over Linear and a backlog, and agent roles, are on the way.

## Requirements

Claude Code running inside herdr, plus `fzf`, `nvim` 0.10 or later, `jq`, `git` and `curl` on `PATH`; `tig` for the history split; `cargo` to build the `kelpie` binary. The scripts use POSIX `sh` and short flags, so they run with the BSD tools on macOS as well.

## Install

```bash
claude plugin marketplace add https://github.com/gronare/kelpie
claude plugin install kelpie@kelpie
```

## Tasks in worktrees

`kelpie` is a small command-line tool (Rust, one binary) that runs each task in a pooled git worktree with its own herdr Space and Claude:

| Command | What it does |
| --- | --- |
| `kelpie start <name> [prompt]` | Takes a free worktree from the pool (`wt<N>.<repo>` next to the main checkout), or creates one with the repo's `script/set_up_worktree` when it has one, else `git worktree add`. Puts it on a new branch `<name>` from the remote's default branch, opens it as a Space labelled `<name>`, runs `dev` in a second tab when the repo has a `dev.yml`, and starts Claude with the prompt. `--no-focus` keeps you where you are. A task that is already running is just shown. |
| `kelpie list [--json]` | The pool's worktrees: their task, or `free`, with uncommitted changes and commits ahead and behind. |
| `kelpie land [name]` | Rebases the task onto the default branch and checks it (no uncommitted changes, no merge commits, no `Co-Authored-By` trailers), then prints the push command. It never pushes. |
| `kelpie done [name]` | Once the commits are on the default branch: closes the Space, deletes the branch, frees the worktree, and fast-forwards the main checkout when it is clean. |
| `kelpie drop [name] [--force]` | Throws the task away; refuses while it has uncommitted changes or unpushed commits unless `--force`. |

Git is the only state: a worktree on a task branch is busy, a clean one that is detached or on its own `wt<N>` branch is free. Worktrees are kept and reused, so only the first task in a new worktree pays for setup.

### The lead and the board

`prefix+space` takes you to the **lead** Space, and back to where you were on the next press. The first time, it creates it: the **board** on the left, and on the right a Claude started with [`roles/lead.md`](roles/lead.md), the session you talk to most. It knows what every other session is doing, finds work in Linear and the vault, and starts, moves and checks on tasks; the work itself is discussed in each task's own Space.

The board (`kelpie board`, also over any Space with `prefix+t`) shows only what herdr's sidebar doesn't. A status line counts tasks that need you, are working or are ready to push. **Work** and **Backlog** sit side by side (stacked in a narrow pane; `←` `→` switch). Work lists your tasks grouped by what they need, with commits, changed files and how far behind master; the selected one opens in place with review progress and its agent's open question or last message, and warns about files changed in a main checkout outside any task. Backlog lists the vault backlog and Linear issues by priority. `⏎` brings up the options for the selected row (go to its Space, message its Claude, open the review, ready to push, done, drop; or start a backlog item as a task), each with a letter that also works directly. The lead keeps the backlog with `kelpie backlog save`; `R` asks it to refresh. `?` lists every key.

`kelpie status [--json]` gives the same picture on the command line, for the lead and for scripts.

### In herdr

The repo is also a herdr plugin. Install it (herdr builds the binary with `cargo`, so the Rust toolchain must be installed):

```bash
herdr plugin install gronare/kelpie
```

Then bind its keys in your herdr `config.toml`:

```toml
[[keys.command]]
key = "prefix+space"
type = "plugin_action"
command = "kelpie.lead"
description = "kelpie: go to the lead, or back"

[[keys.command]]
key = "prefix+t"
type = "plugin_action"
command = "kelpie.board"
description = "kelpie board"
```

 On startup the plugin links `~/.local/share/kelpie/current` to wherever herdr installed it, so configs can use a stable path. Outside herdr, run the binary from there: `~/.local/share/kelpie/current/target/release/kelpie`.

### Sidebar sync (optional)

`scripts/sync.sh` fills the sidebar with what each Space and pane is doing: `web` / `jobs` tags while `dev web` / `dev worker` run, Claude's task title on its pane border, model and context in use on its Agents row, and tab names. It also fetches each repo every 5 minutes, so behind-counts stay current. Run it from herdr's tab bar, with an example sidebar layout in [`examples/herdr-config.toml`](examples/herdr-config.toml):

```toml
[ui]
tab_bar_right = [
  { type = "command", command = "~/.local/share/kelpie/current/scripts/sync.sh", interval_seconds = 3, timeout_seconds = 3 },
]
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
