# Try unreleased code

Run a branch or a local checkout of kelpie for a while, then go back to your channel.

## Install a branch from origin

```bash
kelpie update --from fix-board
```

## Install another checkout's commits

```bash
kelpie update --from ~/code/kelpie
```

Commit your changes first: only commits are installed, so a checkout with uncommitted changes is refused. The path cannot be the install root itself.

## While the override holds

- A plain `kelpie update` fetches the same branch or checkout again, so you can keep pulling in new commits.
- `kelpie update --check`, `kelpie doctor` and the board say `running <branch or path> @ <sha>, not the channel`.

## Go back to the channel

```bash
kelpie update --stable
```

See also: [Updates and stale sessions](../explanation/updates-and-restarts.md).
