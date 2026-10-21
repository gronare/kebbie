# Fix common problems

Start with:

```bash
kelpie doctor
```

Every `warn` and `fail` comes with its fix. What it checks: [Requirements](../reference/requirements.md#what-kelpie-doctor-checks).

## Common fixes

| You see | Fix |
| --- | --- |
| `claude plugin ... runs in Claude Code, but kelpie's checkout is ...` | `kelpie update`. Claude Code runs a cached copy of the plugin, and the update refreshes it. |
| `claude plugin source: GitHub, following the default branch` on a `release` machine | `claude plugin marketplace remove kelpie && claude plugin marketplace add ~/.local/share/kelpie/current && claude plugin install kelpie@kelpie` |
| `kelpie: command not found` in a task or the lead | `ln -s ~/.local/share/kelpie/current/target/release/kelpie ~/.local/bin/kelpie`, with `~/.local/bin` on your `PATH`. |
| The `current` link is missing or points elsewhere | Restart herdr. Keep `XDG_DATA_HOME` unset for herdr, or create the link under `~/.local/share/kelpie` yourself. |
| `Cannot tell <repo>'s default branch` | `git remote set-head origin -a` in that repo. |
| A task shows `no Space` | Its Space or Claude is gone, for example after a herdr restart. Press `g` to restart it; it resumes its last conversation. |
| `kelpie update` refuses on the `release` channel | The install root has uncommitted changes or is not on a release tag. Move your work elsewhere, or run `kelpie update --stable` on a clean checkout. |
| `kelpie lead --restart` refuses | The lead is working or waiting on a question. Run it once the lead is idle. |
| The lead cannot push a task | You have not given it push permission for that task (`a`), or under a repo-wide permission the reviewer has not passed the current code. The task waits for you under Ready to push. |
| `kelpie push` or `kelpie land` reports a conflict | kelpie aborted its rebase and handed it to the task's Claude, which resolves it and reports back. The lead checks the resolved commits before the next push. |
| A push says the task's Claude runs tests first | The default branch changed files the task changes too. kelpie pushes once the task reports ready with the same commits. You can push it yourself before then. |
| A rebase is refused and names files hidden from `git status` | Those files are marked skip-worktree or assume-unchanged and have local changes git will not overwrite. Unhide them with `git update-index --no-skip-worktree <file>` or `--no-assume-unchanged`, then commit or stash them. |
| A task with an approval stays out of Ready to push, marked `N uncommitted files` | Tracked files have changes that are not committed. Commit or discard them; untracked files do not count. |
| A config error names a file and a key | The key is unknown, or belongs in config.toml rather than a repo config. `kelpie config` lists every valid key. |
| The review split does not open, or its list is empty | Check that `fzf` is 0.71 or later and `nvim` 0.10 or later. |
| The board shows no plan usage | [Set the status line](show-plan-usage.md). Usage comes only on a Claude.ai plan. |
| No notifications arrive | Run the command by hand with `KELPIE_TEXT=test`. Failures are in `notify.log` in the state directory. |
| The vault is reported too old | `claude plugin update vault@gronare` |
| Sessions are not restarted, or held messages wait, with the board closed | `kelpie keeper --status`. Not running: `kelpie keeper --start`. A routine that keeps failing shows its error there and in `keeper.log` in the state directory. |
| The keeper should stop | `kelpie keeper --stop`. The board and herdr's next startup start it again; `[keeper] enabled = false` keeps it off and hands the routines to an open board. |
| A session is not restarted after an update | `kelpie update` prints why for each session. A skipped one needs a manual restart; one with background work waits for your answer on the board. |
| A task worktree has no push block | The repo's shared git config sets `core.worktree` or `core.bare = true`. Move those settings out of the shared config; `kelpie doctor` in the repo shows when the block applies. |

## Run kelpie without touching your state

To reproduce a problem with a clean state and no herdr:

```bash
KELPIE_STATE_DIR=$(mktemp -d) HERDR_BIN_PATH=false kelpie status
```

## Turn parts off

| To stop | Do |
| --- | --- |
| All of kelpie's herdr side | `herdr plugin disable kelpie` |
| The sidebar sync | Remove its line from herdr's `[ui] tab_bar_right`. |
| Automatic restarts | `[restart] auto = false`. See [Turn off automatic restarts](turn-off-automatic-restarts.md). |
| The background keeper | `[keeper] enabled = false`. An open board runs the routines instead. |
| The review split opening by itself | Turn `auto_open` off under `/plugin` → Installed → kelpie → Configure. |
