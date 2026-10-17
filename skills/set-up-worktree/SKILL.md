---
name: set-up-worktree
description: Make this git worktree runnable without touching the main checkout: dependencies, the gitignored inputs the main checkout has, its own databases, a free port, then one check that tests run. Use when kelpie's task role asks for it at the start of a task, when the user runs /kelpie:set-up-worktree, or when tests or the dev server fail in a worktree for want of setup.
---

# Set up this worktree

`kelpie` is `${KELPIE_BIN:-$HOME/.local/share/kelpie/current/target/release/kelpie}`. The main checkout is the parent of `git rev-parse --path-format=absolute --git-common-dir`. The note is `$(git rev-parse --git-dir)/kelpie-setup.md`.

1. Stop if this is not a linked worktree (`git rev-parse --git-dir` equals `--git-common-dir`): say this skill sets up a worktree next to a main checkout, and do nothing.
2. If the note exists, an earlier task set this worktree up. Check only what can have drifted: the lockfile since the commit the note names, the copied files, the database, the port. Go to step 8 when nothing has.
3. When kelpie runs you, say so on the board: `kelpie report --state working --next nothing "setting up <folder>"`.
4. Read what the repo says about itself, and stop at the first answer:
   - `bin/setup-worktree`: run it from the worktree, then go to step 7.
   - A worktree section in AGENTS.md, CLAUDE.md, README.md or CONTRIBUTING.md: follow it.
   - Otherwise `bin/setup`, the lockfiles, `.env.example`, the database config and the CI workflow, which installs the repo from scratch: set up the same way.
5. Do the minimum, in the worktree only:
   - Dependencies: install the way CI does (`bundle check || bundle install`, `pnpm install --frozen-lockfile`, `npm ci`, `uv sync`). A compiled language that builds into the worktree needs nothing.
   - Gitignored inputs the code reads: `git -C <main> status --ignored --short` lists what the main checkout has. Copy keys, `.env*` and local config; never caches, logs, builds or databases. Copy, never symlink.
   - Databases: a file database on a relative path is already this worktree's own; prepare it with the repo's idempotent command (`bin/rails db:prepare`). A server database (Postgres, MySQL, a URL in `.env`) is shared until the worktree has its own name, derived from the folder (`wt2_my_app`); creating it is step 6.
   - Port: the main checkout's port plus the slot number (`wt2` of a repo on 3000 gets 3002), stepping up while `lsof -i :<port>` shows it taken. Set it where the repo reads it per checkout (`PORT` in an untracked `.env`, a `--port` flag), never in a tracked file.
6. Ask before anything that deleting the worktree does not undo: creating a server database, migrating one, starting containers or services the main checkout shares, installing global tools. One line on what and why, then wait.
7. Verify with the smallest thing that proves it: one fast test file, or the server answering once on its port, then stop it. The whole suite is not setup.
8. Read `<main>/.kelpie.toml`. When it has no `[worktree] dev_server` and you know the command that starts the repo's dev server, ask the user whether to add it, so the kelpie board's `w` key starts it in this task's Space. The command is the same for every worktree (`bin/dev`, `npm run dev`); the port comes from where step 5 set it, never from the command. On yes, add only that key, creating the file when it is missing and keeping everything else in it.
9. Write the note: first line `# <folder> set up <date> by task <name>`, then one line per thing done, the dev server command with its port, and what was verified. Overwrite an old note.
10. Reply in two lines: what the worktree now has (the dev server command with its port included), and what you did not do and why. When kelpie runs you, report as the task role says; a failure is `--state needs-you` with the fix as `--next`.

Rules:
- The main checkout is read-only: never change, build in, install into or run commands against it. The one exception is the `dev_server` key in its `.kelpie.toml`, on the user's yes.
- Never `git add`, commit or push.
- Never put a secret in a tracked file.
- Never touch a database the worktree does not own: no drop, reset or migration. Never `git clean`, never delete outside the worktree.
- Ask before anything that deleting the worktree does not undo.
- Leave no server running; the board's `w` key or the user starts it.
