# Worktrees

Every task runs in a git worktree from a pool next to the main checkout: `~/code/app` gets `wt1.app`, `wt2.app` and so on. Worktrees are kept and reused, so a new task usually starts in one that is already set up.

## The pool

Git is the only state. A worktree on a task branch is busy. A clean one that is detached, or on its own `wt<N>` branch, is free. `kelpie start` takes a free one, or creates a new one with `git worktree add --detach` when none is free. `kelpie done`, `drop` and `reset` give it back.

`kelpie list` shows the pool: each worktree's task or `free`, its uncommitted changes, commits ahead and behind, and the first line of its setup note.

## Setup

A new task's Claude makes its worktree runnable before anything else, with the skill `worktree.setup_skill` names. The default is `kelpie:set-up-worktree`. It:

1. Reads what the repo says about worktrees: AGENTS.md, CLAUDE.md, README.md, CONTRIBUTING.md and any project skill about worktrees.
2. Runs the repo's prepare command: `worktree.setup_command` when set, else `bin/setup-worktree` when the repo has one, else a worktree section in those docs, else works the setup out from `bin/setup`, the lockfiles and CI.
3. Installs dependencies, copies the gitignored inputs (keys, `.env` files) from the main checkout, creates the worktree's own databases, and picks a free port.
4. Runs one fast check that tests run.

A worktree inherited from an earlier task is taken over first: databases holding that task's migrations are rebuilt from this tree's schema, and runtime state inside the worktree (`tmp/`, logs, caches, compiled assets) is cleared. Dependencies, copied inputs and the port stay.

The skill never writes to the main checkout, and asks before anything that deleting the worktree would not undo. What it did goes into a note at `$(git rev-parse --git-dir)/kelpie-setup.md`, outside `git status`, so the next task in that worktree only checks what has drifted.

### `bin/setup-worktree`

The repo's own answer to setup: executable, no arguments, idempotent, run from inside a worktree, and with no knowledge of kelpie. Anyone who runs `git worktree add` by hand gets the same result.

```sh
#!/bin/sh
set -eu
main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
for f in config/master.key .env; do [ -e "$f" ] || cp "$main/$f" "$f"; done
bundle check >/dev/null || bundle install
bin/rails db:prepare
```

A script with another name, or one that takes arguments, goes in `worktree.setup_command`:

```toml
[worktree]
setup_command = "script/prepare_worktree"
```

A command that names a path applies only in repos that have that file. A team whose repos share a script name can set it once in the machine config, and every other repo keeps the default setup.

### Make a repo worktree-safe

`/kelpie:make-worktree-safe` audits a repo for what breaks across worktrees: fixed ports, shared databases and services, gitignored inputs, absolute paths, shared caches. When you say yes, it writes `bin/setup-worktree`, the port fix and a `## Working in a worktree` section, in a worktree for you to review. You run it; Claude never starts it on its own.

## Dev server and worker

Set the commands in the repo config:

```toml
# ~/.config/kelpie/repos/app.toml
[worktree]
dev_server = "bin/dev"
dev_worker = "bin/jobs"
```

- `w` on the board runs `dev_server` in a `server` tab of the task's Space, and `w` again stops it with ctrl-c.
- `b` does the same for `dev_worker` in a `worker` tab.

Both get `PORT` set to the worktree's own port: `PORT` in the worktree's `.env`, else the port in its setup note. With neither, no `PORT` is set. `worktree.server_name` and `worktree.worker_name` rename the tabs and the sidebar tags.

## URLs

`worktree.dev_url` is the address the board shows under a task while its dev server runs. It is a template:

| Placeholder | Value for `wt1.app` |
| --- | --- |
| `{worktree}` | `wt1.app` |
| `{repo}` | `app` |
| `{number}` | `1` |
| `{port}` | the worktree's port |

```toml
[worktree]
dev_url = "http://localhost:{port}"      # the default: servers on this machine
# dev_url = "https://{worktree}.test"    # behind a proxy that serves each checkout by folder name
```

A worktree without what the template names gets no link. `""` turns the link off. `kelpie status --json` carries the address as `url`.
