# Run a job on another machine

Keep the task, its Claude and its code on one machine, and have another machine of your hub account run what only it can: an Xcode build on a Mac for a task on a Linux devbox, or a test suite on a second devbox.

The machine with the tasks is the master. It holds the repos and names the jobs. A worker is any other machine of the same hub account with one setting on. It holds no repo and no file to begin with: each job brings the worktree along through the hub. No ssh, and no machine names to set up.

## What you need

- kebbie and git on both machines.
- Both machines connected to the same hub account, from the link in the browser: `kebbie hub status` on each shows the account. See [See your board on your phone](see-your-board-on-your-phone.md).
- The keeper running on both, as it is by default: it is what takes the job on the worker and the job's output on the master.
- On hub.kebbie.dev, Hub Pro.

## 1. Turn the worker on

On the worker, in `~/.config/kebbie/config.toml`:

```toml
[worker]
enabled = true
```

Its keeper then tells the hub this machine runs jobs, and it runs them one at a time, in the order they come. Turning the setting off, or stopping the keeper, tells the hub it no longer does.

Turning it on is your yes to every machine of your account: each can run commands in your user account on this machine. Only a machine on the list your account signed can send it a job, and nothing on the worker listens.

## 2. Name the repo's jobs on the master

In the repo's config on the master, `~/.config/kebbie/repos/app.toml`:

```toml
[jobs]
lint = "swiftlint"

[jobs.build]
run = "xcodebuild -scheme App -destination 'generic/platform=iOS Simulator' build"
on = "macos"
```

`on` picks the worker by its operating system, `macos` or `linux`, or by its machine name. A job without it runs on any worker. Of several that fit, the one the hub heard from last runs it.

## 3. Run a job from a worktree

On the master, in the task's worktree:

```sh
kebbie job run build
```

The job runs on the worktree as it stands, uncommitted and untracked files included, and its output prints as it comes, a couple of seconds behind. The command exits 0 when the job exited 0, and 1 otherwise, with the job's exit code or the reason in its last line, so a task's Claude reads it like a local build. `kebbie job run` alone lists the repo's jobs.

The first job a worker runs for a repo carries the commit's whole history; later ones carry only what came after the last commit it took. On the worker, each task has its own folder under `~/.local/share/kebbie/worker/<repo>/`, checked out fresh for each job with its ignored files, such as build caches, kept.

## What can go wrong

- `No machine of your hub account on macos runs jobs`: `worker.enabled` is off there, its keeper is not running, or it is connected to another account. On the worker, the keeper's log in its state directory says so on a line starting `jobs:`.
- `kebbie job run needs this machine's keeper running`: start it with `kebbie keeper --start`.
- `Running a job on another machine is part of Hub Pro`: your plan on the hub has no jobs.
- `sent nothing on build for 10 minutes`: the worker went away mid-job, as when it went to sleep. A worker reports at least once a minute while a job runs, however quiet the job is.
- `this repo is too big to run as a job for now`: a job carries at most 32 MiB of git history. After the first job only what is new goes, but the first one has to fit.
- A job that hangs on the worker: stop its keeper there, `kebbie keeper --stop`; the job is stopped and the master hears it failed.

See [`kebbie job run`](../reference/commands.md#jobs-on-another-machine) and [`[jobs]`](../reference/configuration.md#jobs).
