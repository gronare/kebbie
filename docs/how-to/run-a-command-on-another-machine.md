# Run a command on another machine

Run one command on another machine of your hub account as if it ran here: `kebbie run --on macos -- xcodebuild -scheme App build` from a task on a Linux machine. The output streams live, the command exits with the remote exit code, and Ctrl-C reaches the remote command.

`kebbie run` goes over a direct link between the two machines when they share a private network, Tailscale or WireGuard, and the hub only tells each machine where the other one listens. When no address answers within 2 s, the same link goes through a relay on the hub, which passes the encrypted session along and cannot read it. On hub.kebbie.dev the relay is part of Hub Pro; on a hub you run yourself it is free. The hub relays at most 4 GiB per link and 8 links at once per account.

## What you need

- Both machines connected to the same hub account, as for [jobs](run-a-job-on-another-machine.md#what-you-need), with the keeper running on both.
- For a direct link, both machines on one Tailscale tailnet or one WireGuard network. Without one, the link goes through the hub. kebbie finds the addresses itself: Tailscale's from `tailscale ip` and from addresses in 100.64.0.0/10 and fd7a:115c:a1e0::/48 on an interface named `tailscale…` or `utun…`, and WireGuard's from interfaces named `wg…` or that the system calls WireGuard, only on a private address (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 100.64.0.0/10 or fc00::/7).
- For a direct link, TCP port 47011 open between them on that network.

## 1. Turn the other machine on as a worker

On the machine that should run the commands, in `~/.config/kebbie/config.toml`:

```toml
[worker]
enabled = true
```

Its keeper then listens on port 47011, on its Tailscale and WireGuard addresses and on loopback. It never listens on a public interface or on every interface. It tells the account's other machines those addresses through the hub, sealed and signed with its own key, and keeps a WebSocket to the hub open, where it is rung for a link relayed through the hub. Its keeper log gets a line for each address it listens on and each time it tells the others.

Turning it on is your yes to every machine on your account's signed list: each can run commands in your user account on this machine. A machine proves which one it is with its own key during the TLS 1.3 handshake. The worker accepts only a key on the list the hub hands over at that moment, a list your account signed and that is no older than one the worker already saw. Turning the setting off stops the listening and tells the other machines.

## 2. Check the link

On the machine with the task:

```sh
kebbie link
```

```
workstation (this machine): direct on 127.0.0.1:47011, 0.4 ms
studio: direct on 100.101.102.103:47011, 3.1 ms
laptop: relayed through the hub, 38.4 ms
mini: down: mini takes no links: [worker] enabled is off there
```

Each machine of your account shows how a link reaches it, directly or through the hub, and the round trip, or why it does not.

## 3. Run a command

In the task's worktree:

```sh
kebbie run --on macos -- xcodebuild -scheme App build
```

Everything after `--` is the command. kebbie refuses a word that starts with `-` before `--`, such as `kebbie run --on studio ls -la`, and says to put it after `--`.

`--on` picks the machine as [`on`](run-a-job-on-another-machine.md#2-name-the-repos-jobs-on-the-master) picks a job's worker: by its system, `macos` or `linux`, or by its name. This machine's own name runs the command here, over loopback. Several words after `--` are quoted one by one. A single word is one shell line, so `kebbie run --on studio -- 'make lint && make test'` runs both.

The command runs on the worktree as it stands, uncommitted and untracked files included. The first time, the worker fetches the history from `origin` itself when `origin` is an https or ssh URL it can reach, and the rest comes over the link. Later runs send only the objects the worker lacks, and nothing when the worktree has not changed. On the worker, each task has its own folder under `~/.local/share/kebbie/worker/<repo>/.links/`, apart from the jobs' folders, checked out with `--force` and cleaned without `-x`, so ignored files such as build caches survive from one run to the next. Two runs in one task take turns: the second says it waits for the first and starts once the first has ended.

stdout and stderr come back apart and as they are written. `kebbie run` exits with the command's own exit code: 128 plus the signal's number when a signal ended it, and 255 when no link could be made or the link was lost. Ctrl-C sends SIGINT to the command's process group on the worker, a second sends SIGTERM and a third SIGKILL; a fourth quits `kebbie run` at once. Ctrl-C before the command has started stops the run there and then, and the command never starts. SIGTERM or SIGHUP to `kebbie run`, as when its terminal closes, counts as two Ctrl-C. The command runs with `sh -c`, with `KEBBIE_RUN_TASK` and `KEBBIE_RUN_FROM` set to the task and the asking machine's name, and stdin closed. Whatever the command left in the background is stopped when it ends.

Both sides send a beat every 10 s when they have nothing else to send. When the other machine sends nothing for 45 s, `kebbie run` stops waiting, says so and exits 255; the worker stops the command the same way when the asker goes silent. A send the other side does not take within 45 s fails the link too. A direct link also has TCP keepalive. A keeper that stops gives its running commands SIGTERM and waits up to 15 s for them.

## What can go wrong

- `kebbie: no link to studio: studio announced no address for a link … and studio is not waiting for links through the hub either`: `worker.enabled` is off there, its keeper is not running, or its kebbie is too old to take links.
- `studio is on no private network kebbie knows`: that machine has no Tailscale or WireGuard address. The link goes through the hub when it waits there.
- `none of its addresses answered within 2 s`: the two machines share no network kebbie can use, or a firewall blocks port 47011. The link then goes through the hub.
- `studio was rung through the hub but did not answer within 10 s`: the worker's keeper waits on the hub but did not take the link; its keeper log says why.
- `the TLS handshake failed`: something other than that machine answered on its address, or one of the two machines is no longer on your account's list.
- `kebbie: studio sent nothing for 45 s, so kebbie stopped waiting`: the machine went to sleep or off, or the network between went away. The command may still be running there.
- `relaying through hub.kebbie.dev needs Hub Pro`: the account has no Hub Pro, so only a direct link over Tailscale or WireGuard works.
- `the hub keeps no machine addresses yet` or `the hub relays no links yet`: the hub runs an older version than the machines.

Without any link, `kebbie run` points to `kebbie job run`, which goes through the hub's mailbox instead.

See [`kebbie run` and `kebbie link`](../reference/commands.md#commands-on-another-machine).
