# The worktree pool

This page explains why every task runs in its own git worktree, and why kebbie keeps those worktrees and reuses them.

A task's worktree sits next to the main checkout: `~/code/app` gets `wt1.app`, `wt2.app` and so on.

## Each task gets its own files and branch

Several Claude sessions cannot share one checkout: they would edit the same files, switch branches under each other and run each other's tests. A worktree gives each task its own files and its own branch while sharing one repository. Commits made in a task are visible from the main checkout at once, and nothing is cloned twice.

Your main checkout stays yours. kebbie never starts a task in it, and the board warns when files there change outside any task.

## A kept worktree starts fast

A fresh worktree is not runnable. Real projects need dependencies installed, gitignored inputs such as `.env` files and keys copied in, their own databases, and a port that does not collide with the other worktrees. Doing that for every task would make each start slow.

So kebbie keeps worktrees after a task ends. The next task takes a free one and inherits its setup: dependencies, copied inputs and port stay. Only what belonged to the earlier task is cleared:

- databases holding its migrations are rebuilt from the new tree's schema,
- runtime state such as `tmp/`, logs, caches and compiled assets goes.

## Git is the only state

kebbie keeps no list of worktrees. It reads git:

- A worktree on a task branch is busy.
- A clean worktree that is detached, or on its own `wt<N>` branch, is free.

`kebbie start` takes a free one, or adds a new one with `git worktree add --detach` when none is free. `done`, `drop` and `reset` give it back.

Giving one back removes its build output, by default a Rust worktree's `target/`, so nine worktrees do not each keep tens of gigabytes. [`worktree.clean_on_release`](../reference/configuration.md) names the folders, and only what git ignores and tracks nothing in goes.

You can inspect or repair the pool with plain git, and nothing in kebbie's state can disagree with it.

## The repo knows how to set itself up

kebbie cannot know how to make your project runnable. A task's Claude works it out with the setup skill, guided by what the repo says about itself:

- `bin/setup-worktree` when the repo has one,
- a configured `worktree.setup_command`,
- a worktree section in its docs,
- failing those, `bin/setup`, the lockfiles and CI.

The best answer is a `bin/setup-worktree` in the repo. It is plain shell, takes no arguments, can run again safely, and knows nothing about kebbie, so a person running `git worktree add` by hand gets the same result. `/kebbie:make-worktree-safe` drafts one for you. See [Set up a repo's worktrees](../how-to/set-up-worktrees.md).

The skill records what it did in a setup note inside the worktree's git directory, outside `git status`. The next task in that worktree reads the note and checks only what drifted.

## Each worktree has its own port

Each worktree's port is `PORT` in its `.env`, else the one in its setup note. The board's `w` and `b` start the repo's dev server and worker with that `PORT`, in tabs of the task's Space, and the board shows the server's address under the task. Several tasks can each run their own server at once. See [Run a task's dev server](../how-to/run-dev-servers.md).
