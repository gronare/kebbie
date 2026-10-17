---
name: make-worktree-safe
description: Audit a repo for what breaks when it runs in several git worktrees at once (fixed ports, shared databases and services, gitignored inputs, absolute paths, shared caches) and, on the user's yes, draft bin/setup-worktree and the fixes. Use only when the user runs /kelpie:make-worktree-safe or asks to make a repo safe for worktrees.
disable-model-invocation: true
---

# Make this repo worktree-safe

The main checkout is the parent of `git rev-parse --path-format=absolute --git-common-dir`. You read it; you write only in a linked worktree (`git rev-parse --git-dir` differs from `--git-common-dir`), where every change is a reviewable diff that deleting the worktree undoes.

1. Audit, reading only:
   - Fixed ports: numbers in `bin/dev`, `Procfile*`, the `dev` script in `package.json`, `vite.config.*`, `astro.config.*`, `wrangler.*`. A port the environment cannot override is a finding; `ENV["PORT"] ||` or `${PORT:-3000}` is fine.
   - Shared databases: the database config or the `DATABASE_URL` in `.env.example`. A relative file path is safe; an absolute path, or a server database with a fixed name, is shared by every worktree.
   - Shared services the dev script starts: containers or daemons with a fixed name or port. Ask whether sharing is intended.
   - Gitignored inputs: `git -C <main> status --ignored --short` minus caches, logs, builds and databases. Keys, `.env*` and local config are what a worktree must copy.
   - Absolute paths in tracked config: `/Users/`, `/home/`, `$HOME`, and the main checkout's own path.
   - Caches outside the tree keyed on nothing, e.g. `~/.cache/<tool>/<repo>`: two worktrees building at once collide.
   - Test parallelism: per-process test databases are fine on relative paths, a finding when their names are shared.
2. Propose one list, a finding and its fix per line:
   - `bin/setup-worktree`: executable, no arguments, idempotent, run from inside a worktree; it finds the main checkout with `git rev-parse --path-format=absolute --git-common-dir`, copies the inputs, installs, and prepares the worktree's own database. Port and dev server stay out of it.
   - The smallest change that makes a fixed port overridable, when there is one.
   - A `## Working in a worktree` section in AGENTS.md, else CLAUDE.md, else README.md: run `bin/setup-worktree`, the port rule (the main checkout's port plus the slot number), and what is shared on purpose.
   - When `.kelpie.toml` has no `[worktree] dev_server`: the command that starts the dev server, the same for every worktree.
3. Ask. On yes, write the list in this worktree. Run in the main checkout, write nothing: say the fixes belong in a worktree (`kelpie move` or `kelpie start`), except the `dev_server` key, which you may add to the main checkout's `.kelpie.toml` on its own yes, creating the file when it is missing.
4. Run `bin/setup-worktree` once in the worktree to prove it, then reply in two lines: what you wrote, and what is still shared and why.

Rules:
- The main checkout is read-only apart from that one `.kelpie.toml` key on the user's yes.
- Never `git add`, commit or push.
- Never put a secret in a tracked file; the setup script copies secrets, it never contains them.
- Never touch a database the worktree does not own, nor a production or staging entry in any config.
- Ask before anything that deleting the worktree does not undo.
