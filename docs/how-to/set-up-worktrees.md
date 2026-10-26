# Set up a repo's worktrees

Make every new worktree of a repo runnable on its own, with its dependencies, gitignored inputs, databases and port.

1. In a Claude session in the repo's main checkout, run:

   ```text
   /kebbie:make-worktree-safe
   ```

   It audits the repo for what breaks across worktrees: fixed ports, shared databases and services, gitignored inputs, absolute paths and shared caches.

2. Say yes to its plan. It writes `bin/setup-worktree`, the port fix and a `## Working in a worktree` section, in a worktree for you to review.

3. Review the worktree and run the script yourself. Claude never starts it on its own.

4. Check each worktree's setup:

   ```bash
   kebbie list
   ```

   Each worktree shows the first line of its setup note. The full note is at `$(git rev-parse --git-dir)/kebbie-setup.md` inside the worktree.

The setup skill runs `bin/setup-worktree` whenever a repo has one.

- If you write `bin/setup-worktree` yourself, make it executable, take no arguments, make it safe to run again, and run it from inside a worktree. See the example below.
- If your script has another name, set `setup_command` under `[worktree]` in the repo config. See below.
- If several repos share the script name, set `setup_command` once in `config.toml`. A command that names a path applies only in repos that have that file; the others keep the default setup.
- If you want your own setup skill, set `setup_skill` under `[worktree]`, for example `setup_skill = "my-tools:set-up-worktree"`.

## A setup script for a Rails app

```sh
#!/bin/sh
set -eu
main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
for f in config/master.key .env; do [ -e "$f" ] || cp "$main/$f" "$f"; done
bundle check >/dev/null || bundle install
bin/rails db:prepare
```

## A script with another name

```toml
# ~/.config/kebbie/repos/app.toml
[worktree]
setup_command = "script/prepare_worktree"
```

See also: [The worktree pool](../explanation/worktree-pool.md).
