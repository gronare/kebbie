---
name: make-worktree-safe
description: Audit a repo for what breaks when it runs in several git worktrees at once (fixed ports, shared databases and services, gitignored inputs, absolute paths, shared caches) and, on the user's yes, draft bin/setup-worktree and the fixes.
disable-model-invocation: true
---

# Make this repo worktree-safe

The main checkout is the parent of `git rev-parse --path-format=absolute --git-common-dir`, and it is read-only: you audit it and write nothing there except the `dev_server` key in step 3. Fixes go into a linked worktree (`git rev-parse --git-dir` differs from `--git-common-dir`), where each one is a reviewable, uncommitted diff that deleting the worktree undoes. Anything that deletion does not undo waits for the user's yes.

A **finding** is something two worktrees running at once collide on, or something a fresh worktree lacks to start.

1. **Audit**, reading only. Done when every item below is checked and each finding noted with the file it lives in:
   - Fixed ports: numbers in `bin/dev`, `Procfile*`, the `dev` script in `package.json`, `vite.config.*`, `astro.config.*`, `wrangler.*`. A port the environment cannot override is a finding; `ENV["PORT"] ||` or `${PORT:-3000}` is fine.
   - Shared databases: the database config or the `DATABASE_URL` in `.env.example`. A relative file path is safe; an absolute path, or a server database with a fixed name, is shared by every worktree.
   - Shared services the dev script starts: containers or daemons with a fixed name or port. Ask whether sharing is intended.
   - Gitignored inputs: `git -C <main> status --ignored --short` minus caches, logs, builds and databases. Keys, `.env*` and local config are what a worktree must copy.
   - Absolute paths in tracked config: `/Users/`, `/home/`, `$HOME`, and the main checkout's own path.
   - Caches outside the tree keyed on nothing, e.g. `~/.cache/<tool>/<repo>`: two worktrees building at once collide.
   - Test parallelism: per-process test databases are fine on relative paths, a finding when their names are shared.
2. **Propose** one list, a finding and its fix per line. Done when every finding has a fix or a reason it stays shared:
   - `bin/setup-worktree`: executable, no arguments, idempotent, run from inside a worktree; it finds the main checkout with `git rev-parse --path-format=absolute --git-common-dir`, copies the inputs, installs, and prepares the worktree's own database. Port and dev server stay out of it.
   - The smallest change that makes a fixed port overridable, when there is one.
   - A `## Working in a worktree` section in AGENTS.md, else CLAUDE.md, else README.md: run `bin/setup-worktree`, the port rule (the main checkout's port plus the slot number), and what is shared on purpose.
   - When `<main>/.kelpie.toml` has no `[worktree] dev_server`: the command that starts the dev server, the same for every worktree.
3. **Ask**, then write what the user said yes to. In a linked worktree, write the list there. The `dev_server` key goes into `<main>/.kelpie.toml` on its own yes, creating the file when it is missing. Run from the main checkout, write only that key, and say the other fixes belong in a worktree (`kelpie move` or `kelpie start`). Done when every approved fix is written.
4. **Prove** it: run `bin/setup-worktree` once in the worktree. Done when it exits 0. Reply in two lines: what you wrote, and what is still shared and why.

## Guardrails

- The change stays an unstaged diff for the user to review: no `git add`, commit or push.
- Secrets stay in untracked files: the setup script copies them and never contains them.
- Write only to databases the worktree owns; production and staging entries in any config stay as they are.
