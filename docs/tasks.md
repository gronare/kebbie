# Tasks

A task is a branch in a pooled worktree, with its own herdr Space and its own Claude. This page follows one from start to done.

## Start

```bash
kelpie start billing "Invoices round the VAT per line; round per invoice instead."
```

kelpie takes a free worktree from the pool (`wt<N>.<repo>` next to the main checkout), or creates one. It puts it on a new branch `billing` from the remote's default branch, opens it as a Space labelled `billing`, and starts Claude with your prompt and the task role. The role tells Claude which task and repo it is, which setup skill to run first, who its peers are and how to message them.

| Option | What it does |
| --- | --- |
| `--repo <name>` | Start in that repo instead of the current one. kelpie finds a checkout of that name under `~/projects`, `~/personal`, `~/src` or `~/code` (or one level down; `KELPIE_REPO_DIRS` overrides the list), else clones it from an owner among your other checkouts' remotes. |
| `--from <ref>` | Branch off that ref instead of the default branch. |
| `--backlog <item>` | Tie the task to a backlog item, as `kelpie link` does. |
| `--mode <mode>` | Claude's permission mode for the task. |
| `--no-focus` | Stay where you are. |

You can also press `s` on the board, `g` on a backlog item, or ask the lead.

Running `start` on a task that already runs just shows it. If its Space or Claude is gone, kelpie brings them back in the same permission mode and resumes the last conversation without sending the prompt again.

### One task across repos

Repeat `--repo` for a change that spans repos, such as an event one service sends and another consumes:

```bash
kelpie start payload-v2 "Add currency to the payment event." --repo api --repo web
```

Each repo gets a half with the same branch name, and each Claude is told of the other. `kelpie tell payload-v2` reaches both halves.

## Move work into a task

```bash
kelpie move billing app/models/invoice.rb
```

Moves work under way in the main checkout into a task: branch `billing` if it exists, else the unpushed commits, plus the uncommitted files you name. `--pane <id>` moves that herdr pane along. `/kelpie:move-to-worktree` does the same from inside Claude.

## While it works

At the end of every turn the task's Claude runs `kelpie report` with its state (`needs-you`, `ready`, `working` or `blocked`), what you should do next, and a one-line summary. The board shows it on the task's row, herdr's sidebar shows the next step, and the lead gets a copy. A turn that ends without a report is sent back once to report.

When it reports `ready`, it adds a sign-off card: a few lines for you on the change, where to look, the decisions to confirm, and screenshots if any. The push window shows the card above the commits and marks it when the code changed since it was written.

When it needs a decision, it asks with `kelpie ask`. See [The lead](lead.md#questions).

`kelpie peers` lists the other task sessions and what each is doing. `kelpie tell <task> "<text>"` messages one.

## Rules and approval

How commits look is a skill's call. `task.rules_skill` names it, and the default is `kelpie:work-rules`: a subject and a body that says why, one logical change per commit, staging by explicit file list, the push left to you. Replace it in your machine config or per repo with your own skill.

When a task reports `ready`, the lead checks its commits against the rules skill. It tells the task what to fix, or runs `kelpie approve`. The board shows a task under Ready to push only while that approval matches its commits. Until then it waits under Needs you, marked "rules not checked by the lead", and you can push from there too.

`kelpie tidy billing "squash into one"` asks the task's Claude to reshape its commits by the rules skill, keeping the final tree identical.

## Land and push

kelpie is trunk-based: a task's commits land on the default branch, rebased, with no pull request and no merge.

- **`kelpie land`** rebases the task onto the default branch and checks it has no uncommitted changes. It never pushes. On a conflict it aborts its own rebase, leaves the worktree as it was, and hands the rebase to the task's Claude.
- **`kelpie push`** pushes the task to the default branch and closes it as done. `--through <sha>` pushes the commits up to that one and keeps the task open.
- **`l` on the board** does both: it rebases, shows the card and the commits, and pushes on `y`. See [the push window](board.md#the-push-window).

You push. The lead pushes only a task you gave it permission for, with `a` on the task's row, or with `a` on a repo in [Areas](areas.md) for every task there whose reviewer passed its current code. Either way the lead's rules approval must match the commits. After every push the lead is told which commits landed.

To open a pull request instead, push the task's branch yourself from the main checkout: `git -C ~/code/app push origin billing`.

## Review

`kelpie review billing` (or `v` on the board) sends the task to a separate, read-only reviewer. Its push waits until the reviewer passes the current code. See [Review](review.md).

## Finish

| Command | What it does |
| --- | --- |
| `kelpie done [name]` | Once every change on the branch is on the default branch (pushed as is, cherry-picked or squashed), closes the Space, deletes the branch, frees the worktree, and fast-forwards a clean main checkout. Otherwise it names the missing commits. |
| `kelpie drop [name] [--force]` | Throws the task away. Refuses while it has uncommitted changes or unpushed commits, unless `--force`. |
| `kelpie reset [name]` | Stops a task for good: discards its changes and commits, frees its worktree, closes its Space, tells the lead what was thrown away, and returns its backlog item to the Backlog column. `z` on the board, which asks first. |
| `kelpie link <name> <item>` | Ties a running task to a backlog item or project note (a vault path, a Linear id, or `local:<Title>`). When the task is done the lead closes the item; a reset puts it back. |

Without a name, `land`, `review`, `push`, `done`, `drop` and `reset` act on the task whose worktree you are in. `-C <dir>` runs any command as if started in `<dir>`.

## Listing

- `kelpie list [--json]`: the pool's worktrees, each with its task or `free`, uncommitted changes, commits ahead and behind, and the first line of its setup note.
- `kelpie status [--json]`: every task and Claude session, grouped like the board.
