# Run a job on another machine

Keep the task, its Claude and its code on one machine, and have another machine run what only it can: an Xcode build on a Mac for a task on a Linux devbox.

The machine with the tasks is the master. The other is a worker: it connects out to the master over ssh, so nothing on it listens, and it runs only the jobs its own file names.

## What you need

- kebbie on both machines.
- The worker can ssh to the master without a password prompt, as `ssh devbox` does.
- git on the worker. It needs no clone of the repo: it fetches from the master.

## 1. Name the jobs on the worker

In `~/.config/kebbie/worker.toml` on the worker, one table per repo, by the last part of the repo's origin:

```toml
[repos.app.jobs]
build = "xcodebuild -scheme App -destination 'generic/platform=iOS Simulator' build"
test = "xcodebuild test -scheme App -destination 'platform=iOS Simulator,name=iPhone 16'"
```

Each job is a shell command, run in the worktree's files.

## 2. Start the worker

On the worker, with the master's ssh name:

```sh
kebbie worker devbox
```

It stays connected and prints each job it runs. Stop it with ctrl+c.

## 3. Run a job from a worktree

On the master, in the task's worktree:

```sh
kebbie job run build
```

The job runs on the worktree as it stands, uncommitted and untracked files included, and its output prints as it comes. The command exits 0 when the job passed and 1 when it failed, so a task's Claude reads it like a local build.

## What can go wrong

- `No worker offers the job build for app`: no worker is connected, or its file names no such job for that repo. The repo's name is the last part of its origin, `app` for `git@github.com:acme/app.git`.
- `the worker could not get the commit`: the worker cannot fetch from the master's worktree over ssh. Try `git ls-remote devbox:<the worktree's path>` on the worker.
- A master can run any job the file names, and a build runs the repo's own scripts, so connect a worker only to a master you trust with that machine.

See [`kebbie job` and `kebbie worker`](../reference/commands.md#jobs-on-another-machine).
