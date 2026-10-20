# Updates and stale sessions

![U on the board: kelpie updates and restarts the idle Claude sessions](../assets/clips/update-and-restarts.gif)

kelpie updates itself, its Claude Code plugin and the vault plugin with one command, and then moves the running Claude sessions onto the new code without losing their conversations. This page explains how.

## One install root

herdr installs kelpie into a folder of its choosing; herdr's startup links `~/.local/share/kelpie/current` to it. The binary on your `PATH`, the Claude Code plugin's marketplace and the hooks all point through that link. When `kelpie update` checks out new code there and rebuilds, every part moves together.

## Channels

Each machine follows one channel:

- **`release`**, the default, installs the highest `vX.Y.Z` tag on origin. The install root is checked out detached at that tag. Only tags of exactly `v` and three numbers count, so `v1.4` and `v1.4.0-rc1` are never offered.
- **`main`** follows origin's default branch as it is pushed.

Because the release channel checks out tags in the install root, it refuses to touch an install root with uncommitted changes or one that sits on a branch. Develop in another checkout and try its commits with `kelpie update --from <path>`.

The board checks for updates when it starts and every 6 hours after, and shows `↑ kelpie <version> · U updates` when the channel has something newer. On a release machine the notice names the new tag, never what main carries.

## Overrides

`kelpie update --from <branch or path>` installs a branch on origin or another checkout's commits, and holds it: a plain `kelpie update` fetches the same branch or checkout again. `kelpie update --check`, `kelpie doctor` and the board say `running <branch or path> @ <sha>, not the channel` until `kelpie update --stable` returns you to the channel. Only commits are installed, so a checkout with uncommitted changes is refused.

## The Claude Code plugin

Claude Code runs a cached copy of each plugin. When the kelpie marketplace was added from `~/.local/share/kelpie/current`, `kelpie update` refreshes that copy from whatever it just checked out.

A marketplace added from GitHub follows GitHub's default branch instead. `kelpie update` updates it only on the `main` channel without an override; otherwise it leaves it and prints the commands that move it to the install root. `kelpie doctor` warns about it too.

## Stale sessions

A running Claude session keeps the code it started with. kelpie calls it stale when it runs older code than it would load if it started now:

- the `claude` binary on `PATH` changed after it started,
- a plugin was updated after it started,
- your MCP server config changed after it started, or
- `kelpie update` ran after it started.

A restart is `claude --resume <session id>` in the same pane, with the process's own flags such as `--model`, `--permission-mode` and `--add-dir`. The conversation stays, and the new process loads the new Claude Code, plugins, MCP servers and hooks. The lead is restarted through `kelpie lead --restart`, which also rewrites its role.

## When restarts happen

With `restart.auto` on, the default, `kelpie update` restarts idle sessions at once, and the board restarts each other stale session once it has been idle and out of focus for `restart.idle_minutes`. The status line shows `↻ N stale` while sessions wait.

A restart never interrupts work:

- A session mid-turn, or the pane you are in, is never restarted on its own.
- A session with background work running, such as a background shell, an agent or a monitor, is not cut off. The board asks you once, as a question.
- A session kelpie cannot restart safely is skipped with the reason: one whose session id herdr does not know, whose command line cannot be read or has an option kelpie does not know, whose Claude runs in another folder than its pane, or whose conversation file is missing.

`kelpie update` ends with one line per session: `restarted`, `restarts when idle`, `you are in it: restarts when you leave it`, `waiting for your answer: background work running`, or `skipped: <reason>`.

## Related

- [Switch update channel](../how-to/switch-update-channel.md)
- [Try unreleased code](../how-to/try-unreleased-code.md)
- [Turn off automatic restarts](../how-to/turn-off-automatic-restarts.md)
