---
name: set-up-worktree
description: Make this git worktree runnable without touching the main checkout: dependencies, the gitignored inputs the main checkout has, its own databases, a free port, then one check that tests run. Use when kelpie's task role asks for it at the start of a task, when the user runs /kelpie:set-up-worktree, or when tests or the dev server fail in a worktree for want of setup.
---

# Set up this worktree

`kelpie` is `${KELPIE_BIN:-$HOME/.local/share/kelpie/current/target/release/kelpie}`. The main checkout is the parent of `git rev-parse --path-format=absolute --git-common-dir`. The note is `$(git rev-parse --git-dir)/kelpie-setup.md`.

Three words carry this skill:

- **Owns.** The worktree owns what lives inside its folder: the gitignored files under its root, a file database on a relative path, a server database named after its folder (`wt2_my_app`). It changes, rebuilds and deletes only what it owns. Everything else is read-only to it: the main checkout, other databases, services the main checkout shares, every path outside the folder. The one write outside is the `dev_server` key in step 9, on the user's yes.
- **Inherited.** Worktrees are pooled and handed from task to task, and freeing one resets only what git tracks. An inherited worktree still holds the previous task's databases and runtime state; step 6 takes them over.
- **Undo.** Deleting the worktree undoes everything it owns. Anything it does not undo (creating or migrating a server database, starting containers or services the main checkout shares, installing global tools) waits for the user's yes: one line on what and why, then wait. Rebuilding or clearing what an inherited worktree owns needs no question, since it held the previous task's state and this tree recreates it.

Setup leaves its changes as untracked, gitignored files: secrets stay out of tracked files, and nothing is staged or committed.

1. Confirm this is a linked worktree: `git rev-parse --git-dir` differs from `--git-common-dir`. When they are equal, say this skill sets up a worktree next to a main checkout, and stop.
2. Decide whose worktree this is. The current task is kelpie's task name when kelpie runs you, else `git branch --show-current`. A note naming this task and this branch means this task set the worktree up earlier: check only what can have drifted (the lockfile since the commit the note names, the copied files, the databases, the port), redo what drifted, and go to step 9. Any other note, or no note, means the worktree is inherited. Done when it is either this task's or inherited.
3. When kelpie runs you, say so on the board: `kelpie report --state working --next nothing "setting up <folder>"`.
4. Find how the repo sets itself up; the first match wins:
   - `bin/setup-worktree`: the repo's own script, which sets up on a clean worktree. In an inherited worktree, first install the dependencies the database check needs (step 5's first bullet) and take over (step 6); then run the script from the worktree and go to step 8.
   - A worktree section in AGENTS.md, CLAUDE.md, README.md or CONTRIBUTING.md: follow it through the steps below.
   - `bin/setup`, the lockfiles, `.env.example`, the database config and the CI workflow, which installs the repo from scratch: set up the same way.

   Done when you know the route, and for the two routes without a script also the install command, the inputs the code reads, the databases and the dev server's port.
5. Install and copy, in the worktree:
   - Dependencies: install the way CI does (`bundle check || bundle install`, `pnpm install --frozen-lockfile`, `npm ci`, `uv sync`). A compiled language that builds into the worktree needs nothing here.
   - Inputs: `git -C <main> status --ignored --short` lists the main checkout's gitignored files. Copy the ones the code reads to start: keys, `.env*`, local config. Copy, so the worktree owns its copy; the main checkout's caches, logs, builds, uploads and databases stay where they are.

   Done when the install exits 0 and every input the code reads exists in the worktree.
6. Take over an inherited worktree, on every route; a worktree that is this task's skips this step.
   - Databases, before anything prepares or migrates one: a database the previous owner left carries its migrations, and preparing on top of it writes that task's tables and columns into this task's schema dump. A database the worktree owns is the previous owner's when the note names another task or branch, or, with no note, when it records a migration this tree has no file for: read each environment's migration status (`bin/rails db:migrate:status` marks those `NO FILE`, `alembic current` names a revision missing from the tree). Rebuild such a database from this tree's schema. In Rails, `bin/rails db:drop db:prepare` in development rebuilds the test database too, and running it again with `RAILS_ENV=test` leaves the test database empty. Keep a database without these signs.
   - Runtime state: the gitignored files that running the previous task's app, tests and builds wrote under the worktree's root. `git status --ignored --short` in the worktree lists every gitignored path as a `!!` line; sort each one into cleared or kept. A listed directory that holds both (`vendor/`, `public/`, `node_modules/`) is opened and sorted one level down.
     - Cleared: `tmp/` (caches, pid files, sockets), `log/`, compiled assets (`public/assets`, `public/packs`, `app/assets/builds`, `.next/`), which the dev server would serve in place of this tree's sources, tool caches (`.cache/`, `node_modules/.cache`, `.pytest_cache`, `.turbo`), coverage and test reports.
     - Uploads (Active Storage's files in `storage/`, `public/uploads`, `public/system`) travel with the database whose rows point at them: cleared when you rebuilt that database, kept when you kept it.
     - Kept: dependencies (`node_modules`, `vendor/bundle`, `.venv`), copied inputs (keys, `.env*`, local config), the `.env` that holds the port, a compiler's build directory (`target/`), database files (`storage/*.sqlite3`, which the database bullet decided), a symlink or a path whose real path leaves the worktree, a pid file whose process still answers `kill -0`, and anything you cannot place, which the reply names.

     Delete each cleared path by its relative path from the worktree root (`rm -rf -- tmp/cache`). Git lists a directory as `!!` only when nothing in it is tracked, so tracked placeholders such as `tmp/.keep` stay.

   Done when every database the worktree owns is rebuilt or kept for a stated reason, every `!!` path is cleared or kept, and `git status --short` shows no tracked file deleted.
7. Give the worktree its own databases and port:
   - A file database on a relative path is already the worktree's own: prepare it with the repo's idempotent command (`bin/rails db:prepare`).
   - A server database (Postgres, MySQL, a URL in `.env`) becomes the worktree's own under a name derived from the folder (`wt2_my_app`); creating it needs the user's yes.
   - Port: the main checkout's port plus the slot number (`wt2` of a repo on 3000 gets 3002), stepping up while `lsof -i :<port>` shows it taken. Set it where the repo reads it per checkout (`PORT` in an untracked `.env`, a `--port` flag).
8. Verify with the smallest thing that proves it: one fast test file, or the server answering once on its port, then stopped. Done when that one check passes; the whole suite is the task's, not setup's.
9. Read `<main>/.kelpie.toml`. When it has no `[worktree] dev_server` and you know the command that starts the repo's dev server, ask the user whether to add it, so the kelpie board's `w` key starts it in this task's Space. The command is the same for every worktree (`bin/dev`, `npm run dev`); the port comes from where step 7 set it. On yes, add only that key, creating the file when it is missing and keeping everything else in it.
10. Write the note, replacing any old one: first line `# <folder> set up <date> by task <name>`, then `- branch <branch> at <short sha>`, which step 2 compares next time, then one line per thing done (an inherited database rebuilt, the runtime state cleared), the dev server command with its port as `port <number>`, which the board's `w` exports as `PORT` when the worktree's `.env` holds none, and what was verified.
11. Reply in two lines: what the worktree now has (the dev server command with its port included), and what you left undone and why. No server is left running; the board's `w` key or the user starts it. When kelpie runs you, report as the task role says; a failure is `--state needs-you` with the fix as `--next`.
