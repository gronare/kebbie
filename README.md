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
| `kelpie start <name> [prompt]` | Takes a free worktree from the pool (`wt<N>.<repo>` next to the main checkout), or creates a bare one with `git worktree add --detach`. Puts it on a new branch `<name>` from the remote's default branch, opens it as a Space labelled `<name>`, and starts Claude with the prompt and [`roles/task.md`](roles/task.md): which task and repo it is, its worktree and the setup skill to run before anything else (see [Worktree setup](#worktree-setup)), its peers, and how to message them. `--repo <name>` starts in that repo instead of the current one (repeat it for one task across repos, e.g. `--repo api --repo web`: a half in each, the same branch name, each Claude told of the other): a checkout named so under `~/projects`, `~/personal`, `~/src` or `~/code` (or one level down; `KELPIE_REPO_DIRS` overrides the list), else a clone from the first owner among your other checkouts' remotes that has it. `--from <ref>` branches off that ref instead, `--mode <mode>` is Claude's permission mode (the task's Claude is never allowed `git push`; pushing stays yours), `--no-focus` keeps you where you are. A task that is already running is just shown. |
| `kelpie peers` | Who you are (by the worktree you are in) and the other task sessions with what each is doing, the same task's halves in other repos first. |
| `kelpie tell <to> <text>` | Run by a session to message another: a task (every repo it spans), `task@repo`, or `lead`. It arrives in that Claude's chat as `[from <task>@<repo>] <text>`. A session waiting for an answer, or with no Claude running, has the message held and delivered once it is free; nothing is ever typed into a question. Every message goes to `~/.local/state/kelpie/messages.jsonl`. The task role tells Claude that a peer's message is information, never your instruction, and pushing stays yours. |
| `kelpie report --state <state> --next "<step>" "<summary>"` | Run by a task's Claude at the end of every turn, as [`roles/task.md`](roles/task.md) tells it: where the task stands (`needs-you`, `ready`, `working`, `blocked`), what you should do next (`nothing` when it carries on alone) and a one-line summary. It goes to the lead through the message log (not typed into its chat), the board shows it on the task's row, and herdr's sidebar gets the next step as `$next` and the state in place of idle or done. A task that stops without one is sent back once by a Stop hook kelpie gives it (`kelpie hook`). |
| `kelpie messages [--print]` | The messages between sessions, live, newest at the bottom (`↑` `↓` scroll); it also delivers held messages as their sessions free up. `--print` prints the last 30. Reports are left out; the board shows them. The lead is copied: each of your prompts to it carries what sessions told each other and their latest reports since it last looked (`--unread`, run by a prompt hook kelpie gives the lead), so the traffic stays out of the chat. |
| `kelpie list [--json]` | The pool's worktrees: their task, or `free`, with uncommitted changes, commits ahead and behind, and the first line of the worktree's setup note (`not set up` without one). |
| `kelpie land [name]` | Rebases the task onto the default branch, folding `fixup!` and `squash!` commits into the commits they fix, and checks it (no uncommitted changes, no merge commits, no `Co-Authored-By` trailers), then prints the push command. It never pushes. On a conflict it aborts its rebase, leaving the worktree as it was, and hands the rebase to the task's Claude, which resolves it and reports back. |
| Push and close (board) | Lists the commits that will land, with the selected one's full message. `e` edits that message in the board; `s` squashes them into one, with a message Claude drafts in the background from the commits and the diff, ready to edit; ctrl-s saves, esc goes back, ctrl-o opens the text in your editor; `y` pushes. Only messages and the squash change; the code stays identical. |
| `kelpie tidy <name> [note]` | Asks the task's Claude to reshape its commits for review (one per logical change, functional change before refactors, fixups squashed in, messages that say why), keeping the final tree identical. The note says how, e.g. "squash into one". |
| `kelpie push [name]` | After land: pushes the task to the default branch and closes it as `done`. Only you run it (or the board's Push and close, which shows the commits and asks first); task agents cannot push. |
| `kelpie done [name]` | Once the commits are on the default branch: closes the Space, deletes the branch, frees the worktree, and fast-forwards the main checkout when it is clean. |
| `kelpie drop [name] [--force]` | Throws the task away; refuses while it has uncommitted changes or unpushed commits unless `--force`. |

Git is the only state: a worktree on a task branch is busy, a clean one that is detached or on its own `wt<N>` branch is free. Worktrees are kept and reused; the setup note each one keeps makes setting it up again for the next task a quick check.

### The lead and the board

`prefix+space` takes you to the **lead** Space, and back to where you were on the next press. The first time, it creates it: the **board** on the left, the **messages** between sessions under it (`kelpie messages`), and on the right a Claude started with [`roles/lead.md`](roles/lead.md), the session you talk to most. It is the master of all work: it holds the whole picture, finds work in Linear and the vault, writes informed task prompts, starts and checks tasks, reviews what they produce, passes on what one task needs to know from another, does small cross-cutting fixes itself and brings you recommendations; feature work happens in each task's own Space, and nobody pushes but you. The lead Space stays at the top of herdr's Spaces: kelpie moves it back whenever a Space is created or the order changes (`kelpie lead --first`).

The board (`kelpie board`, also over any Space with `prefix+t`) shows only what herdr's sidebar doesn't. A status line counts tasks that need you, are working or are ready to push. **Work** and **Backlog** sit side by side (stacked in a narrow pane; `←` `→` switch). Work lists your tasks grouped by what they need (a task's halves in other repos next to each other), with commits, changed files and how far behind master, and under each a `you:` line with the next step from the task's last report. Between turns the report decides the group, and Needs you puts open questions first, then what tasks ask of you, then what is ready. The selected one opens in place with review progress and its agent's open question, its report, or, until it first reports, Claude's recap, and warns about files changed in a main checkout outside any task. Backlog lists the vault backlog and Linear issues by priority. `⏎` brings up the options for the selected row (go to its Space, message its Claude, open the review, start or stop its dev server when the repo sets `worktree.dev_server`, tidy commits, push and close, done, drop; or start a backlog item as a task), each with a letter that also works directly. The lead keeps the backlog with `kelpie backlog save`; `R` asks it to refresh. `?` lists every key. The mouse works too: click a row to select it and again for its options, click an option or a commit, scroll to move, click outside a window to close it.

`kelpie status [--json]` gives the same picture on the command line, for the lead and for scripts.

### Areas

`tab` switches the board between **Work** and **Areas** (or click the tabs top right). Areas lists the vault's system notes (`Areas/*.md`) with their repo, grouped into needs you (main ahead of origin, commits since the last release, or a task waiting on you), active and quiet: running tasks, open notes and backlog, main against origin, the last tag and commits since, and the last log line. Areas without a repo on this machine still list. The repo is the system note's `path`, else where the vault's hooks saw it, else a checkout named after the area. kelpie reads [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp)'s local index (found through the vault plugin's options in Claude Code, or `VAULT_PATH` / `S3_*` and `VAULT_CACHE_DIR` as the server takes them), so a vault on S3 needs no credentials here; it is as fresh as the last session that used the vault. `kelpie areas` prints the same on the command line.

`⏎` lists an area's actions, and `← →` moves into the detail to pick one; each has a letter that also works directly, and the mouse works as on Work. Built in: open its Space (`g`), start a task in its repo told the area's notes (`s`), ask the lead to refresh its backlog (`b`), its vault log (`l`), and push main (`u`, only when main is ahead: shows the commits and pushes as you on `y`, like Push and close).

A repo adds its own actions in a `.kelpie.toml`, reviewed like any other code:

```toml
[[actions]]
key = "R"                     # one character, not one of g s b l u j k q ?
title = "Release"
about = "bump, commit, tag, push"
ask = "version"               # asked in turn; a version is prefilled with the next patch
bump = ["pyproject.toml", { file = "uv.lock", after = 'name = "notes-vault-mcp"' }]
run = ["uv build"]            # shell commands in the main checkout, after the bump
commit = "{version}"
tag = "v{version}"
push = true                   # asks again, then pushes main and the tag as you
then = ["claude-plugins:bump-pin"]   # repo:action offered once this is done, never run on its own

[[actions]]
id = "bump-pin"               # defaults to the title, slugged
key = "P"
title = "Bump a pin"
ask = ["plugin", "version"]
task = "Bump {plugin}'s pin to {version}; commit."   # starts a kelpie task with a Claude instead
```

A script action runs in the main checkout, which must be on the default branch, clean and not behind origin. The window shows each step first (the exact lines a bump changes, only lines that say version and hold the current one; `after` limits a file to the first such line after a marker), and nothing changes until `y`; pushing asks once more. [`examples/notes-vault-mcp.kelpie.toml`](examples/notes-vault-mcp.kelpie.toml) is the release for notes-vault-mcp. For a repo you don't own, put the same in `~/.config/kelpie/actions/<repo>.toml` (`$KELPIE_ACTIONS_DIR` overrides the folder; keep it in a private repo): an action there with the same `id` replaces the repo's own, others are added.

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

`scripts/sync.sh` fills the sidebar with what each Space and pane is doing: a `server` tag while the dev server the board's `w` started runs in the Space, Claude's task title on its pane border, model and context in use on its Agents row, and tab names. It also fetches each repo every 5 minutes, so behind-counts stay current. Run it from herdr's tab bar, with an example sidebar layout in [`examples/herdr-config.toml`](examples/herdr-config.toml):

```toml
[ui]
tab_bar_right = [
  { type = "command", command = "~/.local/share/kelpie/current/scripts/sync.sh", interval_seconds = 3, timeout_seconds = 3 },
]
```

## Configuration

Three layers, read key by key; the first that sets a key wins:

1. The repo's own `.kelpie.toml`, read from the main checkout and never from a worktree, so it may be gitignored.
2. The machine config, `~/.config/kelpie/config.toml` (`$XDG_CONFIG_HOME/kelpie/config.toml`; `KELPIE_CONFIG` names another file).
3. kelpie's built-in defaults.

An unknown key in either file is an error that names the file and the key.

| Key | Where | Default | What it does |
| --- | --- | --- | --- |
| `[board.backlog] sources` | machine config | `[]` | Where the lead finds work for the board's Backlog tab: `"vault"`, `"linear"` or both. |
| `[worktree] setup_skill` | both | `"kelpie:set-up-worktree"` | The skill a task's Claude runs first to make its worktree runnable: `plugin:skill` for a plugin's skill, a bare name for one in `~/.claude/skills` or the repo's `.claude/skills`. |
| `[worktree] dev_server` | `.kelpie.toml` | none | The shell command the board's `w` runs in a `server` tab of the task's Space, and `w` again stops with ctrl-c. Without it the board has no `w`. |

`dev_server` is a repo key; the machine config accepts it too, for every repo that sets none.

A machine config:

```toml
[board.backlog]
sources = ["vault", "linear"]

[worktree]
setup_skill = "my-tools:set-up-worktree"
```

A repo's `.kelpie.toml`, next to its [actions](#areas):

```toml
[worktree]
dev_server = "bin/dev"
```

## Worktree setup

kelpie creates a bare worktree; the task's Claude makes it runnable as its first step, with the skill `worktree.setup_skill` names. Its role text gives it the worktree's path, its slot, the main checkout and that skill.

`/kelpie:set-up-worktree` is the default. It runs the repo's `bin/setup-worktree` when there is one, else follows a worktree section in AGENTS.md, CLAUDE.md, README.md or CONTRIBUTING.md, else works the setup out from `bin/setup`, the lockfiles and CI: dependencies, the gitignored keys and `.env` files copied from the main checkout, the worktree's own databases, a free port, then one fast check. It asks before anything that deleting the worktree does not undo, and never writes to the main checkout, except to add `[worktree] dev_server` to its `.kelpie.toml` when you say yes. What it did goes in a note at `$(git rev-parse --git-dir)/kelpie-setup.md`, outside `git status`, so the next task in that worktree only checks what has drifted. `kelpie move` and `/kelpie:move-to-worktree` run the same skill.

`bin/setup-worktree` is the repo's own answer: executable, no arguments, idempotent, run from inside a worktree, with no knowledge of kelpie, so a colleague who runs `git worktree add` by hand gets the same result:

```sh
#!/bin/sh
set -eu
main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
for f in config/master.key .env; do [ -e "$f" ] || cp "$main/$f" "$f"; done
bundle check >/dev/null || bundle install
bin/rails db:prepare
```

`/kelpie:make-worktree-safe` audits a repo for what breaks across worktrees (fixed ports, shared databases and services, gitignored inputs, absolute paths, shared caches) and, when you say yes, writes `bin/setup-worktree`, the port fix and a `## Working in a worktree` section in a worktree for you to review. You run it; Claude never starts it on its own.

## Update

`kelpie update` brings everything to the latest version in one go: it updates the herdr plugin (a GitHub install through `herdr plugin update`; a linked checkout is fast-forwarded when it is clean, then built), runs `claude plugin update` for kelpie, and sends `/reload-plugins` to every Claude session that is waiting. The session you are in and the ones still working are listed instead, so nothing types into a prompt you are writing. In herdr, `prefix+u` in the example config opens it as a popup.

## Check the install

`kelpie doctor` checks that this machine has what kelpie needs: the tools and their versions, the herdr and Claude Code plugins, the `current` link, the state directories, both config files, and that the plugin of the setup skill in effect is installed and enabled in Claude Code. Each warning or failure comes with its fix for macOS and Linux.

`kelpie config [--repo <path>]` prints the three layers it found, then every key with its value in effect, the layer it comes from (`.kelpie.toml`, `machine config` or `default`) and what it does. Inside a repo it reads that repo's `.kelpie.toml`; `--repo` names another.

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
