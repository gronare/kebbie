# Run a command on another machine

Run one command on another machine of your hub account as if it ran here: `kebbie run --on macos -- xcodebuild -scheme App build` from a task on a Linux devbox. The output streams live, the command exits with the remote exit code, and Ctrl-C reaches the remote command.

`kebbie run` goes over a direct link between the two machines, on a private network they share: Tailscale or WireGuard. The hub only tells each machine where the other one listens. For a command you run often, or when the machines share no private network, a [job](run-a-job-on-another-machine.md) goes through the hub instead.

## What you need

- Both machines connected to the same hub account, as for [jobs](run-a-job-on-another-machine.md#what-you-need), with the keeper running on both.
- Both machines on one Tailscale tailnet or one WireGuard network. kebbie finds the addresses itself: Tailscale's from `tailscale ip` and from addresses in 100.64.0.0/10 and fd7a:115c:a1e0::/48, and WireGuard's from interfaces named `wg…` or that the system calls WireGuard.
- TCP port 47011 open between them on that network.

## 1. Turn the other machine on as a worker

On the machine that should run the commands, in `~/.config/kebbie/config.toml`:

```toml
[worker]
enabled = true
```

Its keeper then listens on port 47011, on its Tailscale and WireGuard addresses and on loopback. It never listens on a public interface or on every interface. It tells the account's other machines those addresses through the hub, sealed and signed with its own key. Its keeper log gets a line for each address it listens on and each time it tells the others.

Turning it on is your yes to every machine on your account's signed list: each can run commands in your user account on this machine. A machine proves which one it is with its own key during the TLS 1.3 handshake. The worker accepts only a key on the list the hub hands over at that moment, a list your account signed and that is no older than one the worker already saw. Turning the setting off stops the listening and tells the other machines.

## 2. Check the link

On the machine with the task:

```sh
kebbie link
```

```
devbox (this machine): direct on 127.0.0.1:47011, 0.4 ms
studio: direct on 100.101.102.103:47011, 3.1 ms
laptop: down: laptop takes no links: [worker] enabled is off there
```

Each machine of your account shows how a link reaches it and the round trip, or why it does not.

## 3. Run a command

In the task's worktree:

```sh
kebbie run --on macos -- xcodebuild -scheme App build
```

`--on` picks the machine as [`on`](run-a-job-on-another-machine.md#2-name-the-repos-jobs-on-the-master) picks a job's worker: by its system, `macos` or `linux`, or by its name. This machine's own name runs the command here, over loopback. Several words after `--` are quoted one by one. A single word is one shell line, so `kebbie run --on studio -- 'make lint && make test'` runs both.

The command runs on the worktree as it stands, uncommitted and untracked files included. The first time, the worker fetches the history from `origin` itself when `origin` is an https or ssh URL it can reach, and the rest comes over the link. Later runs send only the objects the worker lacks, and nothing when the worktree has not changed. On the worker, each task has its own folder under `~/.local/share/kebbie/worker/<repo>/tasks/`, checked out with `--force` and cleaned without `-x`, so ignored files such as build caches survive from one run to the next.

stdout and stderr come back apart and as they are written. `kebbie run` exits with the command's own exit code: 128 plus the signal's number when a signal ended it, and 255 when no link could be made or the link was lost. Ctrl-C sends SIGINT to the command's process group on the worker, a second sends SIGTERM and a third SIGKILL; a fourth quits `kebbie run` at once. The command runs with `sh -c`, with `KEBBIE_RUN_TASK` and `KEBBIE_RUN_FROM` set to the task and the asking machine's name, and stdin closed. Whatever the command left in the background is stopped when it ends.

## What can go wrong

- `kebbie: no link to studio: studio announced no address for a link`: `worker.enabled` is off there, or its kebbie is too old to take links, or its keeper has not told the hub yet. The keeper tells the hub within a minute of starting.
- `studio is on no private network kebbie knows`: that machine has no Tailscale or WireGuard address.
- `none of its addresses answered within 2 s`: the two machines share no network kebbie can use, or a firewall blocks port 47011. `kebbie run` names the addresses it tried and points to `kebbie job run`, which goes through the hub.
- `the TLS handshake failed`: something other than that machine answered on its address, or one of the two machines is no longer on your account's list.
- `the hub keeps no machine addresses yet; it needs an update`: the hub runs an older version than the machines.

See [`kebbie run` and `kebbie link`](../reference/commands.md#commands-on-another-machine).
