# Troubleshooting

## Start with `kelpie doctor`

```bash
kelpie doctor
```

Every line is `ok`, `warn` or `fail`, and each warning or failure comes with its fix for macOS and Linux. It checks:

- the tools and their versions: `git`, `herdr`, `claude`, `jq`, `fzf`, `nvim`, `tig`, `curl`, `cargo`,
- the herdr plugin, that its binary is built, and the `current` link,
- that `kelpie` on your PATH is the `current` link's binary,
- the Claude Code plugin, and whether the copy Claude Code runs matches the checkout,
- the status line for plan usage,
- the last update check,
- the state directories,
- config.toml and every repo config,
- inside a repo, whether its task worktrees get the push block,
- that the setup skill's plugin is installed and enabled,
- the notify command (set or not; it is never run),
- the vault plugin and the backlog sources.

## Common fixes

| You see | Fix |
| --- | --- |
| `claude plugin ... runs in Claude Code, but kelpie's checkout is ...` | `kelpie update`. Claude Code runs a cached copy of the plugin, and the update refreshes it. |
| `claude plugin source: GitHub, following the default branch` on a `release` machine | `claude plugin marketplace remove kelpie && claude plugin marketplace add ~/.local/share/kelpie/current && claude plugin install kelpie@kelpie` |
| `kelpie: command not found` in a task or the lead | Link the binary: `ln -s ~/.local/share/kelpie/current/target/release/kelpie ~/.local/bin/kelpie`, with `~/.local/bin` on your PATH. |
| The `current` link is missing or points elsewhere | Restart herdr. Keep `XDG_DATA_HOME` unset for herdr, or create the link under `~/.local/share/kelpie` yourself. |
| A task shows `no Space` | Its Space or Claude is gone, for example after a herdr restart. Press `g` to restart it; it resumes its last conversation. |
| `kelpie update` refuses on the `release` channel | The install root has uncommitted changes or is not on a release tag. Commit or move your work elsewhere, or run `kelpie update --stable` on a clean checkout. |
| `kelpie lead --restart` refuses | The lead is working or waiting on a question. Run it once the lead is idle. |
| The lead cannot push a task | You have not given it push permission for that task (`a`), or for a repo-wide permission the reviewer has not passed the current code. The task waits for you under Ready to push. |
| `kelpie land` reports a conflict | kelpie aborted its rebase and handed it to the task's Claude, which resolves it and reports back. |
| A config error names a file and a key | The key is unknown, or belongs in config.toml rather than a repo config. `kelpie config` lists every valid key. |
| The review split does not open, or its list is empty | Check that `fzf` is 0.71 or later and `nvim` 0.10 or later; distro packages are often older. |
| The board shows no plan usage | Set the status line (see [Getting started](getting-started.md#6-optional-plan-usage-on-the-board)). Usage comes only on a Claude.ai plan, and is as fresh as the last response of any session. |
| No notifications arrive | Run the command by hand with `KELPIE_TEXT=test`. Failures are logged to `notify.log` in the state directory. |
| The vault is reported too old | `claude plugin update vault@gronare` |
| A session is not restarted after an update | `kelpie update` prints why for each session. A skipped one needs a manual restart; one with background work waits for your answer on the board. |

## Turning things off

- `herdr plugin disable kelpie` stops all of kelpie's herdr side at once.
- Remove the sync line from herdr's `[ui] tab_bar_right` to stop the sidebar sync.
- `[restart] auto = false` stops automatic restarts.

## Where kelpie keeps things

| What | Where |
| --- | --- |
| Machine config and local backlog | `~/.config/kelpie/config.toml`, `~/.config/kelpie/backlog.md` |
| Repo configs | `~/.config/kelpie/repos/<repo>.toml` |
| State: messages, reports, verdicts, roles, review marks, `notify.log` | `~/.local/state/kelpie` |
| The install link | `~/.local/share/kelpie/current` |
| A worktree's setup note | `$(git rev-parse --git-dir)/kelpie-setup.md` inside the worktree |

For a throwaway run that leaves your real state alone, set `KELPIE_STATE_DIR` to a temp directory and `HERDR_BIN_PATH=false`.
