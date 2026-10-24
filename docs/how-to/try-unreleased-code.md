# Try unreleased code

Run a branch or a local checkout of kebbie for a while, then go back to your channel.

## Install a branch from origin

```bash
kebbie update --from fix-board
```

## Install another checkout's commits

```bash
kebbie update --from ~/code/kebbie
```

Commit your changes first: only commits are installed, so a checkout with uncommitted changes is refused. The path cannot be the install root itself.

## While the override holds

- A plain `kebbie update` fetches the same branch or checkout again, so you can keep pulling in new commits.
- `kebbie update --check`, `kebbie doctor` and the board say `running <branch or path> @ <sha>, not the <channel> channel`.

## Go back to the channel

```bash
kebbie update --stable
```

See also: [Updates and stale sessions](../explanation/updates-and-restarts.md).
