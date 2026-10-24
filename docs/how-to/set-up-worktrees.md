# Set up a repo's worktrees

Make every new worktree of a repo runnable on its own: dependencies, gitignored inputs, its own databases and its own port.

## Let Claude draft it

In a Claude session in the repo's main checkout:

```text
/kebbie:make-worktree-safe
```

It audits the repo for what breaks across worktrees: fixed ports, shared databases and services, gitignored inputs, absolute paths and shared caches. When you say yes, it writes `bin/setup-worktree`, the port fix and a `## Working in a worktree` section, in a worktree for you to review. You run the script; Claude never starts it on its own.

## Or write `bin/setup-worktree` yourself

Make it executable, take no arguments, make it safe to run again, and run it from inside a worktree. For example, for a Rails app:

```sh
#!/bin/sh
set -eu
main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
for f in config/master.key .env; do [ -e "$f" ] || cp "$main/$f" "$f"; done
bundle check >/dev/null || bundle install
bin/rails db:prepare
```

The setup skill runs `bin/setup-worktree` whenever a repo has one.

## Use a script with another name

Name it in the repo config:

```toml
# ~/.config/kebbie/repos/app.toml
[worktree]
setup_command = "script/prepare_worktree"
```

A command that names a path applies only in repos that have that file. Set it once in `config.toml` when several repos share the script name; the others keep the default setup.

## Use your own setup skill

```toml
[worktree]
setup_skill = "my-tools:set-up-worktree"
```

## Check a worktree's setup

```bash
kebbie list
```

Each worktree shows the first line of its setup note. The full note is at `$(git rev-parse --git-dir)/kebbie-setup.md` inside the worktree.

See also: [The worktree pool](../explanation/worktree-pool.md).
