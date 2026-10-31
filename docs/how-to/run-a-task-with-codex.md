# Run a task with Codex

Run a task on OpenAI's Codex CLI instead of Claude Code.

A Codex task gets the same pooled worktree, card on the board, push block and `kebbie report` before each turn ends. The lead and the reviewer stay Claude.

Its task role is written for Codex. Codex can't run kebbie's setup and rules skills, so it prepares its worktree with the repo's own `worktree.setup_command` and is not told the rules skill. Its commits are still checked against the rules skill when it reports ready, as any task's are.

## Before you start

- Codex CLI 0.163.0 or later on `PATH`, logged in (`codex login`). `kebbie doctor` shows its version, and its `codex login` line says whether `codex login status` reads logged in.
- herdr starts it in the task's pane as `herdr agent start <task> --kind codex`.

## Install kebbie's plugin in Codex

Codex runs kebbie's hooks from a Codex plugin.

1. Install it once:

   ```bash
   kebbie setup codex
   ```

   It writes the plugin and prints one line, such as `kebbie's Codex plugin installed at ~/.codex/plugins/cache/kebbie/kebbie/local: .codex-plugin/plugin.json, hooks/hooks.json, bin/kebbie-hook`. With Codex not on `PATH` it stops and writes nothing.

2. Check it:

   ```bash
   kebbie doctor
   ```

   Its `codex plugin` line reads `ok`.

The plugin goes in Codex's plugin cache, `$CODEX_HOME/plugins/cache/kebbie/kebbie/local/` (`CODEX_HOME` defaults to `~/.codex`), and holds three hooks:

| Hook | What it does in a kebbie task |
| --- | --- |
| `SessionStart` | Records the session id Codex picked, so a restart resumes it, and gives the session its task role as context. |
| `UserPromptSubmit` | Marks the start of a turn. |
| `Stop` | Sends the turn back once when it has no `kebbie report` yet, the same as a Claude task's stop hook. |

Outside a kebbie task's worktree each hook does nothing.

- `kebbie setup codex` never edits Codex's `config.toml` or `hooks.json`.
- Codex loads the plugin only in the sessions kebbie starts. kebbie turns it on with `-c plugins.kebbie@kebbie.enabled=true` and trusts its three hooks by their hash with `-c hooks.state=…`, for that session alone. Your own Codex sessions don't load it.
- Codex trusts the hooks by their command line, not by the launcher script they run. So kebbie starts a Codex task only when every file of the plugin is exactly what it writes, and a missing, out-of-date or changed plugin stops the start with `run kebbie setup codex`.
- A second run that finds nothing to change writes nothing.
- `kebbie setup codex` is yours to run: a task cannot run it.

The plugin follows kebbie's updates: `kebbie update` and `kebbie setup --yes` rewrite what a new kebbie writes differently, and say `Codex plugin: updated` with the files. If `kebbie doctor` still says the plugin is out of date, run `kebbie setup codex` again: it rewrites the files that changed and prints `kebbie's Codex plugin updated:` with their names. If you want the plugin gone, run `kebbie setup codex --uninstall`.

## Write a Codex profile

1. Write `~/.config/kebbie/profiles/coder.toml`:

   ```toml
   about = "Codex on a GPT model"
   for = "a second opinion on a bug, work you want Codex to do"
   agent = "codex"
   model = "gpt-5.5"
   effort = "high"
   permission_mode = "acceptEdits"
   ```

2. Check it:

   ```bash
   kebbie profiles
   ```

   Under the profile, a `codex:` line shows the Codex binary, its version and whether kebbie's plugin is installed.

A Codex profile takes `about`, `for`, `agent`, `model`, `effort`, `role`, `skills`, `permission_mode` and `sandbox.enabled`. `model` goes to `codex --model`, `effort` to `model_reasoning_effort`.

`deny`, `allow`, `[claude]` and the other `[sandbox]` keys set Claude Code's options, which Codex doesn't read. A Codex profile that has them doesn't load, and `kebbie profiles` names the key.

## Start the task

1. Start it with the profile:

   ```bash
   kebbie start flaky-test --profile coder "Find why the checkout test fails one run in ten"
   ```

The lead can choose the profile too, by its `for`.

- If Codex asks something before it runs, kebbie answers nothing and the start says what Codex asks: whether you trust the repo's folder, the first time Codex runs in it, or whether to trust hooks of your own that are new or changed. Answer it in the task's Space, and kebbie types the task's prompt in once the keeper or an open board runs.
- If you already trust the folder in Codex, that question is answered for you.

## How the sandbox and the permission mode map

With `task.sandbox` on, the default, a Codex task runs in Codex's own sandbox. Codex keeps `.git` read-only there and turns the network off, so a commit or a command that needs the network asks for approval first. So do kebbie's own commands, `kebbie report` among them, which write kebbie's state outside the worktree.

| Permission mode | Sandbox on | Sandbox off |
| --- | --- | --- |
| none, `default`, `acceptEdits` | `--sandbox workspace-write --ask-for-approval on-request` | Codex's own config decides |
| `plan` | `--sandbox read-only --ask-for-approval on-request` | the same |
| `dontAsk`, `bypassPermissions` | `--sandbox workspace-write --ask-for-approval never`: nothing leaves the sandbox, and what it refuses fails back to Codex | `dontAsk`: `--ask-for-approval never`; `bypassPermissions`: `--dangerously-bypass-approvals-and-sandbox` |
| `auto` | `--approve-for-me`: Codex's own reviewer model answers its requests to leave the sandbox instead of you, so a command can run outside it | `--ask-for-approval on-request` with Codex's reviewer answering |

Every command Codex runs gets kebbie's push block in its environment and `KEBBIE_ADDRESS` set to the task's address. A Codex process kebbie started counts as that task when it calls kebbie, and any other Codex counts as an agent, so neither can run `kebbie push`, `approve` or another of your commands.

## Messages to a Codex task

Codex has no inbox kebbie can write to, so kebbie types a message into the task's composer, only when it is empty. It counts the message as taken once the composer lets it go or the session's rollout file shows it as a user turn.

## Compressed sessions

Codex can compress a session file it hasn't written to for a week to `.jsonl.zst`. kebbie still finds such a session and resumes it, but can't read it. So it doesn't send the task's first prompt again and can't tell from it whether a typed message arrived.

## What a Codex task doesn't get

- The record of test runs that `kebbie evidence` and the reviewer read.
- The vault gate that holds edits until the task note's Problem is written.
- Restarts after a kebbie or agent update: `kebbie restart` skips Codex sessions.
- Recovery after an API error.

[Profiles](../reference/configuration.md#profiles) lists every key. [`kebbie setup codex`](../reference/commands.md#setup-and-maintenance) is in the command reference.
