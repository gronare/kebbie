# Run a task with Codex

A task can run OpenAI's Codex CLI instead of Claude Code. It gets the same pooled worktree, card on the board, push block and `kelpie report` before each turn ends, and the task role written for Codex: Codex can't run kelpie's setup and rules skills, so it prepares its worktree with the repo's own `worktree.setup_command` and is not told the rules skill. The lead and the reviewer stay Claude.

## Before you start

- Codex CLI 0.163.0 or later on `PATH`, logged in (`codex login`). `kelpie doctor` shows its version.
- herdr starts it in the task's pane as `herdr agent start <task> --kind codex`.

## Install kelpie's plugin in Codex

Codex runs kelpie's hooks from a Codex plugin. Install it once:

```bash
kelpie setup codex
```

It says what it writes and asks before writing anything. The plugin goes in Codex's plugin cache, `$CODEX_HOME/plugins/cache/kelpie/kelpie/local/` (`CODEX_HOME` defaults to `~/.codex`), and holds three hooks:

| Hook | What it does in a kelpie task |
| --- | --- |
| `SessionStart` | Records the session id Codex picked, so a restart resumes it, and gives the session its task role as context. |
| `UserPromptSubmit` | Marks the start of a turn. |
| `Stop` | Sends the turn back once when it has no `kelpie report` yet, the same as a Claude task's stop hook. |

Outside a kelpie task's worktree each hook does nothing.

`kelpie setup codex` never edits Codex's `config.toml` or `hooks.json`. Codex loads the plugin only in the sessions kelpie starts: kelpie turns it on with `-c plugins.kelpie@kelpie.enabled=true` and trusts its three hooks by their hash with `-c hooks.state=…`, for that session alone. Your own Codex sessions don't load it.

Codex trusts the hooks by their command line, not by the launcher script they run, so kelpie starts a Codex task only when every file of the plugin is exactly what it writes. A missing, out-of-date or changed plugin stops the start with `run kelpie setup codex`. Run it again after kelpie's binary moves; `kelpie doctor` then says the plugin is out of date. A second run that finds nothing to change writes nothing. `kelpie setup codex --uninstall` removes the plugin.

`kelpie setup codex` is yours to run: a task cannot run it.

## Write a Codex profile

`~/.config/kelpie/profiles/coder.toml`:

```toml
about = "Codex on a GPT model"
for = "a second opinion on a bug, work you want Codex to do"
agent = "codex"
model = "gpt-5.5"
effort = "high"
permission_mode = "acceptEdits"
```

Check it:

```bash
kelpie profiles
```

Under the profile, a `codex:` line shows the Codex binary, its version and whether kelpie's plugin is installed.

A Codex profile takes `about`, `for`, `agent`, `model`, `effort`, `role`, `skills`, `permission_mode` and `sandbox.enabled`. `model` goes to `codex --model`, `effort` to `model_reasoning_effort`. `deny`, `allow`, `[claude]` and the other `[sandbox]` keys set Claude Code's options, which Codex doesn't read, so a Codex profile that has them doesn't load and `kelpie profiles` names the key.

## Start the task

```bash
kelpie start flaky-test --profile coder "Find why the checkout test fails one run in ten"
```

The lead can choose the profile too, by its `for`.

When Codex asks something before it runs, kelpie answers nothing and the start says what Codex asks: whether you trust the repo's folder, the first time Codex runs in it, or whether to trust hooks of your own that are new or changed. Answer it in the task's Space, and kelpie types the task's prompt in once the keeper or an open board runs. A folder you already trust in Codex is answered for you.

## How the sandbox and the permission mode map

With `task.sandbox` on, the default, a Codex task runs in Codex's own sandbox. Codex keeps `.git` read-only there and turns the network off, so a commit or a command that needs the network asks for approval first. So do kelpie's own commands, `kelpie report` among them, which write kelpie's state outside the worktree.

| Permission mode | Sandbox on | Sandbox off |
| --- | --- | --- |
| none, `default`, `acceptEdits` | `--sandbox workspace-write --ask-for-approval on-request` | Codex's own config decides |
| `plan` | `--sandbox read-only --ask-for-approval on-request` | the same |
| `dontAsk`, `bypassPermissions` | `--sandbox workspace-write --ask-for-approval never`: nothing leaves the sandbox, and what it refuses fails back to Codex | `dontAsk`: `--ask-for-approval never`; `bypassPermissions`: `--dangerously-bypass-approvals-and-sandbox` |
| `auto` | `--approve-for-me`: Codex's own reviewer model answers its requests to leave the sandbox instead of you, so a command can run outside it | `--ask-for-approval on-request` with Codex's reviewer answering |

Every command Codex runs gets kelpie's push block in its environment and `KELPIE_ADDRESS` set to the task's address. A Codex process kelpie started counts as that task when it calls kelpie, and any other Codex counts as an agent, so neither can run `kelpie push`, `approve` or another of your commands.

## Messages to a Codex task

Codex has no inbox kelpie can write to, so kelpie types a message into the task's composer, only when it is empty. It counts the message as taken once the composer lets it go or the session's rollout file shows it as a user turn.

## Compressed sessions

Codex can compress a session file it hasn't written to for a week to `.jsonl.zst`. kelpie still finds such a session and resumes it, but can't read it, so it doesn't send the task's first prompt again and can't tell from it whether a typed message arrived.

## What a Codex task doesn't get

- The record of test runs that `kelpie evidence` and the reviewer read.
- The vault gate that holds edits until the task note's Problem is written.
- Restarts after a kelpie or agent update: `kelpie restart` skips Codex sessions.
- Recovery after an API error.

[Profiles](../reference/configuration.md#profiles) lists every key. [`kelpie setup codex`](../reference/commands.md#setup-and-maintenance) is in the command reference.
