# Fix common problems

Find what is wrong with kebbie, and the fix for each message it shows.

1. Run the checks:

   ```bash
   kebbie doctor
   ```

2. Apply the `→` fix under each check in **Needs you**. What it checks: [Requirements](../reference/requirements.md#what-kebbie-doctor-checks).

3. Run `kebbie doctor` again. **Needs you** is gone and every group shows `✓`.

- If doctor is clean and the problem remains, find what you see in the tables below.
- If kebbie itself got something wrong, save it as an issue and send it to kebbie's maintainers when you choose: [Report a problem with kebbie](report-a-problem-with-kebbie.md).

## Install and update

| You see | Fix |
| --- | --- |
| `claude plugin ... runs in Claude Code, but kebbie's checkout is ...` | `kebbie update`. Claude Code runs a cached copy of the plugin, and the update refreshes it. |
| `claude plugin source: GitHub, following the default branch` on a `release` machine | `claude plugin marketplace remove kebbie && claude plugin marketplace add ~/.local/share/kebbie/current && claude plugin install kebbie@kebbie` |
| `cargo is not installed to build one` from `herdr plugin install gronare/kebbie` | Install the release the message names, which needs no Rust: `herdr plugin install --ref <tag> gronare/kebbie`, or `curl -fsSL https://kebbie.dev/install.sh \| sh`. |
| `failed gh attestation verify` and `the release binary of kebbie <version> was not installed because its provenance check failed` | Run `gh auth status` and check the network, then try again. If gh still refuses it, the binary is not the one kebbie's release workflow built: keep the one you have and report it at https://github.com/gronare/kebbie/issues. |
| `the origin of kebbie-<version>-<target>.tar.gz was not verified` | Install `gh` and run `gh auth login`. The next release kebbie downloads is checked. |
| `kebbie: command not found` in a task or the lead | `ln -s ~/.local/share/kebbie/current/target/release/kebbie ~/.local/bin/kebbie`, with `~/.local/bin` on your `PATH`. |
| The board and the keeper do not start after a restart, `lead.log` stays empty, and `kebbie doctor` says `herdr links kebbie from <folder>, which is gone` | `kebbie doctor --fix`. It links kebbie in herdr again from the `current` link's checkout and starts the board and the keeper. See [The keeper links kebbie in herdr again](../explanation/updates-and-restarts.md#the-keeper-links-kebbie-in-herdr-again). |
| The `current` link is missing or points elsewhere | Restart herdr. Keep `XDG_DATA_HOME` unset for herdr, or create the link under `~/.local/share/kebbie` yourself. |
| `kebbie update` refuses on the `release` channel | The install root has uncommitted changes or sits on a branch. Move your work elsewhere, or run `kebbie update --stable` on a clean checkout. |
| A session is not restarted after an update | `kebbie update` prints why for each session. A skipped one needs a manual restart; one with background work waits for your answer on the board. |
| The vault is reported too old | `claude plugin update vault@gronare` |

## Tasks and Spaces

| You see | Fix |
| --- | --- |
| `Cannot tell <repo>'s default branch` | `git remote set-head origin -a` in that repo. |
| A new task asks "Do you trust this folder?" | You have not trusted the repo in Claude Code, so kebbie leaves the answer to you. Answer it in the task's Space. kebbie then types the task's prompt in while the keeper or an open board runs; if neither runs, paste the prompt yourself. |
| A task shows `no Space` | Its Space was closed; herdr reopens Spaces after a restart, so a restart alone does not cause it. Press `g` to restart the task. It resumes its last conversation. |
| A task shows `no Claude` | Its Claude exited, or herdr has not resumed it yet: herdr resumes each Claude once a client is attached after a restart. Press `g` if it stays with herdr open, or with herdr's `[session] resume_agents_on_restore = false`. |
| The lead is refused `kebbie approve` or `kebbie issue add` as an agent session | Its Claude runs without kebbie's lead settings, as when herdr resumed it on its own after a restart and kebbie could not restart it yet. Press `L` on the board and choose Resume this conversation. |
| A task with an approval stays out of Ready to push, marked `N uncommitted files` | Tracked files have uncommitted changes. Commit or discard them; untracked files do not count. |
| Your push of a task fails in its worktree's own `pre-push` hook | The worktree's `core.hooksPath` points somewhere other than kebbie's hooks for it. Run `kebbie doctor --fix`, which installs kebbie's push block there again; `kebbie update` does the same. |
| A task worktree has no push block | The repo's shared git config sets `core.worktree` or `core.bare = true`. Move those settings out of the shared config. `kebbie doctor` in the repo shows when the block applies. |

## Codex tasks

| You see | Fix |
| --- | --- |
| `kebbie doctor` says `Codex replaced kebbie's plugin with …`, or a Codex task asks you to review a hook that is new or changed | `kebbie setup codex`. It writes kebbie's plugin again, which wins over the copy Codex installed from a repo's marketplace. |
| A Codex task asks `Hooks need review` for kebbie's hooks right after an update past 0.32.0 | Answer it in the task's Space to trust them. Codex asks this once: kebbie's plugin keeps the same files across later updates. |
| `kebbie doctor` names `codex old plugin` | Run the `rm -rf` it names. The folder is an unused copy under `kebbie@kebbie`, which Codex made from a repo's marketplace named kebbie, and nothing turns it on. |
| A Codex task's start says Codex asks you to sign in | Codex found no login in its home. Sign in in the task's Space, or run `codex login` and start the task again; kebbie types the prompt once Codex takes it. `kebbie doctor`'s `codex login` line says what `codex login status` reads. |
| A sandboxed Codex task asks approval each time it runs `kebbie tell`, `kebbie report` or another kebbie command | Codex's sandbox keeps those commands from kebbie's state and herdr's socket. Answer `p` once per command, in the task's Space or through the lead, and Codex stops asking for it: Codex writes that rule to `~/.codex/rules/default.rules`. See [How the sandbox and the permission mode map](run-a-task-with-codex.md#how-the-sandbox-and-the-permission-mode-map). |
| A Codex task under Needs you says `stopped by a model that is not available` | The model Codex runs is one your login cannot use. Set `model` in the task's Codex profile to one it can, then restart the task with `g`. |

## Pushes and rebases

| You see | Fix |
| --- | --- |
| The lead cannot push a task | You have not given it push permission for that task (`a`), or under a repo-wide permission the reviewer has not passed the current code. The task waits for you under Ready to push. |
| `kebbie push` or `kebbie land` reports a conflict | Nothing to do. kebbie aborted its rebase and handed it to the task's Claude, which resolves it and reports back. The reviewer checks the resolved commits against the rules before the next push. |
| A push says the task's Claude runs tests first | The default branch changed files the task changes too. kebbie pushes once the task reports ready with the same commits. You can push it yourself before then. |
| A rebase is refused and names files hidden from `git status` | The files are marked skip-worktree or assume-unchanged and have local changes git will not overwrite. Unhide them with `git update-index --no-skip-worktree <file>` or `--no-assume-unchanged`, then commit or stash them. |

## Board, lead and keeper

| You see | Fix |
| --- | --- |
| The lead Space shows a shell where the board was | herdr's startup starts the board again within a minute of herdr starting. If you quit the board yourself, go to the lead (`kebbie lead`, or herdr's `kebbie.lead` action) and it starts again. `lead.log` in the state directory says what the startup did. |
| `kebbie lead --restart` or `L` says `Not restarted: …` | The lead is working, waiting on a question, holds text you have not sent, has background work running, or another kebbie is restarting it. Try again once that has passed; `--anyway` restarts it over background work. After an update the keeper restarts it by itself once it is free, unless `restart.auto` is off. |
| Sessions are not restarted, or held messages wait, with the board closed | `kebbie keeper --status`. Not running: `kebbie keeper --start`. A routine that keeps failing shows its error there and in `keeper.log` in the state directory. |
| The keeper should stop | `kebbie keeper --stop`. The board and herdr's next startup start it again; `[keeper] enabled = false` keeps it off and hands the routines to an open board. |
| The board shows no plan usage | [Set the status line](show-plan-usage.md). Usage comes only on a Claude.ai plan. |
| The review split does not open, or its diff stays empty | It opens only inside herdr. Check that `nvim` is 0.10 or later and `jq` is on your `PATH`. |
| No notifications arrive | Run the command by hand with `KEBBIE_TEXT=test`. Failures are in `notify.log` in the state directory. |

## Config errors

| You see | Fix |
| --- | --- |
| A config error names a file and a key | The key has a value of the wrong type, or belongs in config.toml rather than a repo config. `kebbie config` lists every valid key. |
| `kebbie: <file>: <key>: unknown key; known keys here: … (ignored: …)` | A newer kebbie may know the key: `kebbie update`. Otherwise fix its spelling; the line lists the keys known there. kebbie ignores the key until then. |

## Run kebbie without touching your state

Reproduce a problem with a clean state and no herdr:

```bash
KEBBIE_STATE_DIR=$(mktemp -d) HERDR_BIN_PATH=false kebbie status
```

## Turn parts off

| To stop | Do |
| --- | --- |
| All of kebbie's herdr side | `herdr plugin disable kebbie` |
| The sidebar sync | Remove its line from herdr's `[ui] tab_bar_right`. |
| Automatic restarts | `[restart] auto = false`. See [Turn off automatic restarts](turn-off-automatic-restarts.md). |
| The background keeper | `[keeper] enabled = false`. An open board runs the routines instead. |
| The review split opening by itself | Turn `auto_open` off under `/plugin` → Installed → kebbie → Configure. |
| Error counts and the daily install signal sent to kebbie's maintainers | `[report] signals = false`, or leave it unset: both are off until it is `true`. |
| The offer to send a bug report to kebbie's maintainers | `[report] text = "never"`. |
