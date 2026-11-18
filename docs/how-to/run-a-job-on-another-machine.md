# Run a job on another machine

Keep the task, its Claude and its code on one machine, and have another machine of your hub account run what only it can: a build for another operating system, or a test suite on a machine with more cores.

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

Turning it on is your yes to every machine of your account: each can run commands in your user account on this machine. Only a machine on the list your account signed can send it a job. Jobs come through the hub; the setting also lets the account's machines [run a command](run-a-command-on-another-machine.md) here over a direct link.

## 2. Name the repo's jobs on the master

In the repo's config on the master, `~/.config/kebbie/repos/app.toml`:

```toml
[jobs]
lint = "swiftlint"

[jobs.build]
run = "xcodebuild -scheme App -destination 'generic/platform=iOS Simulator' build"
on = "macos"
timeout_minutes = 90
```

`on` picks the worker by its operating system, `macos` or `linux`, or by its machine name. A job without it runs on any worker. Of several that fit, the one the hub heard from last runs it, and only one the hub heard from in the last ten minutes.

`timeout_minutes` is how long the job may run, 60 minutes unless set.

## 3. Run a job from a worktree

On the master, in the task's worktree:

```sh
kebbie job run build
```

The job runs on the worktree as it stands, uncommitted and untracked files included, and its output prints as it comes, a couple of seconds behind. The command exits 0 when the job exited 0, and 1 otherwise, with the job's exit code or the reason in its last line, so a task's Claude reads it like a local build. `kebbie job run` alone lists the repo's jobs.

Ctrl-C stops `kebbie job run` and removes the job's commit from the hub and its files from this machine; a second Ctrl-C quits at once. A worker cannot be told to stop a job, so one that took the job already runs it to its end. A worker that has not fetched the commit yet finds it gone and runs nothing, unless the job needed no commit sent.

When the repo's `origin` is an https or ssh URL that holds part of the branch, the worker fetches the history up to the newest commit `origin` has from there itself, with its own git credentials, and only the commits and changes after it go through the hub. The worker fetches each repo once into `~/.local/share/kebbie/worker/<repo>/`; later jobs carry only what came after the last commit it took, and nothing when it has the commit already. Without such an `origin`, or when the worker cannot fetch from it, the first job carries the commit's whole history through the hub. On the worker, each task has its own folder under `~/.local/share/kebbie/worker/<repo>/`, checked out fresh for each job with its ignored files, such as build caches, kept. A folder the repo does not track is missing in a task's first job, so a job that writes `build/test.log` starts with `mkdir -p build`. Once the job's command ends, whatever it left running in the background is stopped.

## What can go wrong

- `No machine of your hub account on macos runs jobs`: `worker.enabled` is off there, or it is connected to another account. On the worker, the keeper's log in its state directory says so on a line starting `jobs:`.
- `No machine of your hub account on macos runs jobs right now. studio is a worker, but the hub last heard from it 90 minutes ago`: that machine is asleep or off, or its keeper is not running.
- `kebbie job run needs this machine's keeper running`: start it with `kebbie keeper --start`.
- `Running a job on another machine is part of Hub Pro`: your plan on the hub has no jobs.
- `sent nothing on build for 10 minutes`: the worker went away, as when it went to sleep. From the moment it takes a job, a worker reports at least once a minute: while the job waits behind another, while its commit comes down and while it runs, however quiet it is. A job that waits behind another says so in its output.
- `[kebbie: 2 reports from studio were lost here, and the output they held with them]` in the output: the worker could not deliver those reports to the hub, and the job went on.
- `[kebbie: the fetch from https://github.com/you/app.git failed: ...]` and `studio did not fetch the history from ...`: git on the worker could not reach `origin` without a prompt. The job goes on with the whole history through the hub. Give the worker access with an ssh key or a credential helper, as `git clone` there would need.
- `app's history is 45.0 MiB as a git bundle, and the hub carries at most 31.9 MiB per job`: push the branch to an `origin` the worker can fetch from, so only what is not pushed goes through the hub. The line says which of the cases above held.
- `build ran past its timeout of 60 minutes, so the worker stopped it`: the job hung or needs a longer `timeout_minutes`. The worker sends its process group SIGTERM, then SIGKILL 10 s later if it is still there, and the line says so. To stop a hung job sooner, stop the keeper on the worker, `kebbie keeper --stop`: the job is stopped the same way and the master hears why.

See [`kebbie job run`](../reference/commands.md#jobs-on-another-machine) and [`[jobs]`](../reference/configuration.md#jobs).
