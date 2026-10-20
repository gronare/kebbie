# Updating

```bash
kelpie update
```

`prefix+u` runs it in a popup, and `U` runs it from the board. It brings everything to your channel's latest in one go: the herdr plugin (rebuilt), the Claude Code plugin, and the vault plugin when it is installed. It lists each version before → after, then restarts the Claude sessions that are idle so they load the new code.

## Channels

Each machine follows a channel, set in config.toml:

```toml
[update]
channel = "release"   # the default: the highest vX.Y.Z tag on origin
# channel = "main"    # origin's default branch, every push
```

- **`release`** fetches the tags in the install root (the folder herdr runs kelpie from), checks out the highest `vX.Y.Z` tag detached and builds it. Tags that are not exactly `v` and three numbers (`v1.0`, `v1.2.3-rc1`) are not releases. It refuses, and changes nothing, when the install root has uncommitted changes or is not on a release tag, so do not develop in the install root of a `release` machine.
- **`main`** updates a GitHub install through `herdr plugin update`, or fast-forwards a clean linked checkout, then builds.

`kelpie update --stable` moves a clean checkout that sits on a branch to the release tag; the branch keeps its commits.

## Trying unreleased code

```bash
kelpie update --from fix-board      # a branch on origin
kelpie update --from ~/code/kelpie  # the commits of another checkout
kelpie update --stable              # back to the channel
```

Only commits are installed, so a checkout with uncommitted changes is refused, and the path cannot be the install root itself. While the override holds, a plain `kelpie update` fetches the same branch or checkout again, and `kelpie update --check`, `kelpie doctor` and the board say `running <branch or path> @ <sha>, not the channel`.

## Checking for updates

`kelpie update --check` changes nothing but fetched refs. It shows the installed version against the channel's latest, and the vault plugin against its marketplace.

The board runs the same check when it starts and every 6 hours after. When something is newer, the board shows `↑ kelpie <version> · U updates`. On a `release` machine the notice names the new tag, never what main carries. `kelpie doctor` reports the last result without fetching.

## The Claude Code plugin

When the plugin's marketplace was added from `~/.local/share/kelpie/current`, it is the install root's own checkout: whatever `kelpie update` checked out is what the next and every restarted session loads.

A marketplace added from GitHub follows the default branch. `kelpie update` updates it only on the `main` channel without an override; otherwise it leaves it and prints the commands that move it to the install root. `kelpie doctor` warns about it too.

## Restarting stale sessions

A Claude session is stale when it runs older code than it would load if it started now:

- the `claude` binary on PATH changed after it started,
- a plugin was updated after it started,
- your MCP server config changed after it started, or
- `kelpie update` ran after it started.

A restart is `claude --resume <session id>` in the same pane, with the process's own flags (`--model`, `--permission-mode`, `--add-dir` and the rest). The conversation stays, and the new process loads the new Claude Code, plugins, MCP servers and hooks. The lead is restarted through `kelpie lead --restart`, which also rewrites its role.

Once a minute, the board restarts each stale session that has been idle and out of focus for `idle_minutes`. It never restarts one mid-turn or the pane you are in. A session with background work running (a background shell, an agent, a monitor) is not cut off: the board asks you once, and `[notify] command` runs as for any question. The status line shows `↻ N stale` while sessions wait.

kelpie skips, with the reason, any session it cannot restart safely: one whose session id herdr does not know, whose command line cannot be read or has an option kelpie does not know, whose Claude runs in another folder than its pane, or whose conversation file is missing. A restart that fails leaves a message on the board.

`kelpie update` ends with one line per session: `restarted`, `restarts when idle`, `you are in it: restarts when you leave it`, `waiting for your answer: background work running`, or `skipped: <reason>`.

### Settings

```toml
[restart]
auto = true          # false: restart nothing on its own, but still show ↻ N stale
idle_minutes = 10    # 1 to 1440
```

- `kelpie restart` restarts the stale sessions that are idle, not the pane you are in and without background work, whatever `auto` says.
- `kelpie restart --all` restarts every idle session, stale or not.
