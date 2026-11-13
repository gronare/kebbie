# Run a task with Codex

Run a task on OpenAI's Codex CLI instead of Claude Code.

A Codex task gets the same pooled worktree, card on the board, push block and `kebbie report` before each turn ends. The lead and the reviewer stay Claude.

Its task role is written for Codex. Codex can't run kebbie's setup and rules skills, so it prepares its worktree with the repo's own `worktree.setup_command` and is not told the rules skill. Its commits are still checked against the rules skill when it reports ready, as any task's are.

## Before you start

- Codex CLI 0.163.0 or later on `PATH`, logged in (`codex login`). `kebbie doctor` shows its version, and its `codex login` line says whether `codex login status` reads logged in.
- herdr starts it in the task's pane as `herdr agent start <task> --kind codex`.

## Set up Codex

1. Open the setup guide at its Agents step:

   ```bash
   kebbie setup codex
   ```

2. Keep **Codex** ticked. Its row shows the version, whether it is signed in, such as `not signed in: run codex login`, and whether kebbie's plugin is current.

3. Go on to the last step and read the **Codex** lines:

   | Line | What it writes |
   | --- | --- |
   | `plugin` | kebbie's plugin in Codex's plugin cache. |
   | `approvals` | `~/.codex/rules/kebbie.rules`, which lets the thirteen commands a task runs, `kebbie report`, `kebbie tell`, `kebbie ask` and `kebbie subtasks` among them, past Codex's approval question. |
   | `vault` | With a vault, `[mcp_servers.vault]` in Codex's `config.toml`, so a Codex task keeps its own note. |

4. Press `⏎`. Nothing is written before it, and `esc` quits without writing.

5. Check it:

   ```bash
   kebbie doctor
   ```

   Its `codex plugin`, `codex approvals` and `codex vault` lines read `ok`.

- If you run `kebbie setup codex --yes`, it writes the same lines without the guide, one printed line each, such as `kebbie's Codex plugin installed at ~/.codex/plugins/cache/kebbie-tasks/kebbie/local: .codex-plugin/plugin.json, hooks/hooks.json, bin/kebbie-hook`. With Codex not on `PATH` it stops and writes nothing.
- If you run it without a terminal and without `--yes`, it installs or updates the plugin alone and names the rest under `kebbie setup codex --yes also writes:`.
- If one is missing, `kebbie doctor` names it with `kebbie setup codex` as the fix: without the rules file Codex asks before each kebbie command; without the vault server the lead keeps the task's note. Neither stops a task.

### The vault server

The `vault` line runs the vault command kebbie runs, `vault.command`, such as `command = "/home/me/.local/bin/mindin"`. When that is the Claude vault plugin's own command, it takes the plugin's folder or S3 endpoint and bucket with it:

```toml
[mcp_servers.vault]
command = "uvx"
args = ["mindin==0.14.1"]
env_vars = ["S3_ACCESS_KEY", "S3_SECRET_KEY"]
[mcp_servers.vault.env]
S3_BUCKET = "notes"
S3_ENDPOINT = "https://s3.example.com"
```

- The keys never go in the file. `env_vars` has Codex read them from its own environment by name, and kebbie starts each Codex task in a tab whose shell has them set, from the vault plugin's settings in Claude Code. They are never typed into the pane or put on a command line, so `ps` doesn't show them.
- The task's own commands don't get the keys: kebbie starts Codex with them excluded from its `shell_environment_policy`, next to any exclude list of your own.
- The lines go at the end of your `config.toml`, and the rest of the file stays as it is. A vault server with other values is replaced where it stands.

### kebbie's plugin

The plugin goes in Codex's plugin cache, `$CODEX_HOME/plugins/cache/kebbie-tasks/kebbie/local/` (`CODEX_HOME` defaults to `~/.codex`), as the plugin `kebbie@kebbie-tasks`, and holds four hooks:

| Hook | What it does in a kebbie task |
| --- | --- |
| `SessionStart` | Records the session id Codex picked, so a restart resumes it, and gives the session its task role as context. |
| `UserPromptSubmit` | Marks the start of a turn. |
| `Stop` | Sends the turn back once when it has no `kebbie report` yet, the same as a Claude task's stop hook. |
| `PreToolUse` | Refuses a shell command that runs `git push` in any form, the same guard a Claude task's shell has. |

Outside a kebbie task's worktree each hook does nothing.

- The plugin step never edits Codex's `config.toml` or `hooks.json`.
- Codex loads the plugin only in the sessions kebbie starts. kebbie turns it on with `-c plugins.kebbie@kebbie-tasks.enabled=true` and trusts its four hooks by their hash with `-c hooks.state=…`, for that session alone. Your own Codex sessions don't load it.
- kebbie also starts each Codex session with `-c check_for_update_on_startup=false`, so Codex's update screen never stands before the task's prompt. Your own Codex sessions still check. See [Codex updates](#codex-updates) for how kebbie offers a newer Codex.
- Codex 0.164 and later also bind that trust to the plugin's files. kebbie writes the same files whatever its version, so Codex keeps trusting the hooks across kebbie's updates. A machine whose plugin an older kebbie wrote gets `Hooks need review` once after its first update past 0.32.0: answer it in the task's Space, and Codex trusts the hooks from then on.
- Codex trusts the hooks by their command line, not by the launcher script they run. So kebbie starts a Codex task only when every file of the plugin is exactly what it writes, and a missing, out-of-date or changed plugin stops the start with `run kebbie setup codex`.
- A second run that finds nothing to change writes nothing.
- `kebbie setup codex` is yours to run: a task cannot run it.
- Codex installs an enabled plugin again from any marketplace of the same name it finds in the session's repo, and that copy replaces kebbie's. The plugin's id, `kebbie@kebbie-tasks`, is one no Claude or Codex marketplace uses, kebbie's own repo included, and kebbie refuses a Codex task in a repo whose marketplace offers a plugin under it.

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

A Codex profile takes `about`, `for`, `agent`, `model`, `effort`, `role`, `skills`, `permission_mode` and `sandbox.enabled`. `model` goes to `codex --model`, `effort` to `model_reasoning_effort`. `effort` takes Codex's levels, `ultra` among them, not only Claude Code's five.

- If the profile sets no `model`, Codex runs the `model` in its own `config.toml`. `kebbie profiles` shows it on the profile's `runs:` line, as `model gpt-5.5 from Codex's config.toml`, and `kebbie doctor` names it on its `codex model` line.
- If that model is one your Codex login cannot use, such as a model a ChatGPT plan does not include, the task's first turn stops on an API error. Set `model` in the profile to one it can.
- On the board's Settings page (`,`), the Profiles section marks such a profile with `⚠`, and `m` picks a model from the list Codex offers your account.

`deny`, `allow`, `[claude]` and the other `[sandbox]` keys set Claude Code's options, which Codex doesn't read. A Codex profile that has them doesn't load, and `kebbie profiles` names the key.

## Start the task

1. Start it with the profile:

   ```bash
   kebbie start flaky-test --profile coder "Find why the checkout test fails one run in ten"
   ```

The lead can choose the profile too, by its `for`.

- If Codex asks something before it runs, kebbie answers nothing and types nothing, and the start says what Codex asks. Answer it in the task's Space, and kebbie types the task's prompt in once the keeper or an open board runs. Codex 0.163 asks:

  | Codex shows | The start says Codex asks |
  | --- | --- |
  | `Welcome to Codex` and how you want to use it, or `Signed in with your ChatGPT account`, when it finds no login in its home | `you to sign in` |
  | `Codex just got an upgrade` with `Try new model` and `Use existing model` | `which model to use, after its notice of a new one` |
  | `Update available` with `Update now`, `Skip` and `Skip until next version`, in a Codex that checks for updates anyway, as under a managed config | `whether to update it first` |
  | `Trust this folder?`, the first time Codex runs in the repo | `whether you trust its folder` |
  | `Hooks need review`, for hooks of your own that are new or changed, or once for kebbie's after an update past 0.32.0 | `whether to trust the hooks it found new or changed` |

- If you already trust the folder in Codex, that question is answered for you.
- If Codex asks one of these after the start, as the trust question after you sign in, the task waits under Needs you with what Codex asks, and the lead hears it too. kebbie looks for these questions only until Codex has taken its first prompt since it started, so a task that later shows the same words, in code or a diff, stays where it is. A Codex task kebbie resumes or restarts is read for them again until its first turn.

## How the sandbox and the permission mode map

With `task.sandbox` on, the default, a Codex task runs in Codex's sandbox under a permission profile kebbie gives it, `kebbie-task`, which holds it to what kebbie's sandbox holds a Claude task to:

- It writes its worktree, Codex's temp folders, kebbie's sandbox folders (`~/.cargo/registry`, `~/.npm`, `~/.cache` and the rest, or a profile's `allow_write`) and what a commit writes in the repo's `.git`: the objects, the refs and their logs, and its worktree's own git dir. So it commits, stashes and rebases without asking.
- It never writes what git runs or follows as you: the repo's `.git/config`, its hooks and submodules, its worktree's `config.worktree`, `commondir` and `gitdir`, kebbie's push block, or another worktree's git dir. A change there asks for approval.
- It reads none of the credentials files kebbie's sandbox keeps from a Claude task (`~/.ssh`, `~/.config/gh`, `~/.aws`, Codex's `auth.json` and the rest), nor kebbie's state directory and `config.toml`.
- It reaches kebbie's sandbox hosts, the package registries and GitHub, or a profile's `allowed_domains`, through Codex's network proxy, and no other host. A profile with an empty `allowed_domains` turns the network off.

kebbie's own commands, `kebbie tell` and `kebbie report` among them, still ask for approval: they write kebbie's state outside the worktree and reach herdr's socket. kebbie keeps its state out of the sandbox's reach on purpose, so a task can't write it directly.

With `task.sandbox` off, Claude runs unsandboxed, and Codex runs in the sandbox your own Codex config picks. Under `workspace-write`, kebbie adds kebbie's sandbox folders (`~/.cargo/registry`, `~/.npm`, `~/.cache` and the rest, or a profile's `allow_write`) and what a commit writes in `.git` to your own `sandbox_workspace_write.writable_roots` and turns on its network, so a commit or `npm ci` runs without asking.

kebbie's rules file, which `kebbie setup` writes for a ticked Codex, lets kebbie's own task commands through without asking. Without it, each approval reaches you and the lead, as under [Approvals and choices](#approvals-and-choices), and the lead recommends `p`, `Yes, and don't ask again for commands that start with …`, for kebbie's commands: Codex itself adds the rule to `~/.codex/rules/default.rules`, and every later Codex session, your own included, runs that command outside its sandbox without asking. `kebbie push`, `approve` and `review` still ask, and kebbie refuses them to a task anyway.

| Permission mode | Sandbox on | Sandbox off |
| --- | --- | --- |
| none, `default`, `acceptEdits` | `kebbie-task` with `--ask-for-approval on-request` | Codex's own config decides |
| `plan` | `--sandbox read-only --ask-for-approval on-request` | the same |
| `dontAsk`, `bypassPermissions` | `kebbie-task` with `--ask-for-approval never`: nothing leaves the sandbox, and what it refuses fails back to Codex | `dontAsk`: `--ask-for-approval never`; `bypassPermissions`: `--dangerously-bypass-approvals-and-sandbox` |
| `auto` | `kebbie-task` with `--ask-for-approval on-request` and Codex's reviewer answering: its reviewer model answers requests to leave the sandbox instead of you, so a command can run outside it | `--ask-for-approval on-request` with Codex's reviewer answering |

Every command Codex runs gets kebbie's push block in its environment, with no GitHub token or git login and no ssh agent unless its profile runs `ssh` outside the sandbox, as `ops` does, and `KEBBIE_ADDRESS` set to the task's address, and `KUBECONFIG` when its profile names a [kubeconfig](manage-a-cluster-from-a-scheduled-task.md), and kebbie's `PreToolUse` hook refuses a command that runs `git push`, or `kubectl` and `helm` past that kubeconfig, so a task that can commit still can't push. A Codex process kebbie started counts as that task when it calls kebbie, and any other Codex counts as an agent, so neither can run `kebbie push`, `approve` or another of your commands.

## Approvals and choices

When Codex stops to ask you something mid-task, such as whether to run a command outside its sandbox, a question of its own or an MCP server's form, its terminal title starts with `[ ! ] Action Required`. kebbie reads that title, so the task waits under Needs you with `answer Codex's prompt in its Space or through the lead`, and `kebbie peers` shows it as waiting for an answer. The Codex sessions kebbie starts show Codex's default title, whatever `tui.terminal_title` in your Codex config lists.

The lead hears it once, as it hears a Claude task's prompt: the question with the command, then the options in order, with the key Codex shows for each. For example:

```text
1. Yes, proceed (y)  (focused)
2. Yes, and don't ask again for commands that start with `kebbie report` (p)
3. No, and tell Codex what to do differently (esc)
```

The lead answers only on a choice you type in its chat, with `kebbie choose <task> <number> --prompt <id>`: `<number>` is the option's number, or the key it shows in its place, as in `kebbie choose <task> y --prompt <id>`, and `<id>` is the prompt's id from kebbie's message to the lead. A view kebbie cannot read as a menu reaches the lead as its last lines on screen. You can always answer in the task's Space yourself instead.

## Messages to a Codex task

Codex has no inbox kebbie can write to, so kebbie types a message into the task's composer, only when it is empty. It counts the message as taken once the composer lets it go, after it was seen holding the text or a second after the paste, or once the session's rollout file shows it as a user turn.

- Empty means the composer shows nothing or Codex's own placeholder, `Ask Codex to do anything`. Any other text there, faint or not, keeps kebbie from typing, so nothing is glued onto it.
- The task's first prompt waits the same way: when the new Codex's composer already holds text, the start says so, and kebbie types the prompt in once the composer is empty, while the keeper or an open board runs.

## Restarts after an update

A Codex task keeps the role and hooks it started with until it restarts, and Codex never updates a running session. So kebbie restarts a Codex task as it does a Claude one, with `codex resume <session id>` in its pane, once it is idle and out of focus: after `kebbie update`, after a newer `codex` lands on `PATH`, or when it runs without kebbie's plugin.

- The same rules hold: `restart.auto` and `restart.idle_minutes` decide when, and a restart never ends a Codex mid-turn, in the pane you are in, with text in its composer or on one of its start-up questions.
- `kebbie restart` names each Codex task with the others, as `flaky-test: restarted, as kebbie update`.
- Your own Codex sessions, outside a task, are never restarted.
- A Codex task whose profile no longer loads, or no longer runs Codex, is skipped with the profile and the error, as `skipped: its profile "coder" does not load (…)`, rather than resumed on the built-in task's Claude role. Fix the profile and the next restart takes it.

## Codex updates

Codex does not update itself while it runs. Once a day kebbie checks for a newer Codex, read where Codex's own update check reads it for your kind of install, and reads Codex's model list, `~/.codex/models_cache.json`.

- A newer Codex, a model a profile names that Codex no longer lists or offers a newer one in place of, and a change of Codex's default model each get one notice on the board and a line in `kebbie doctor`.
- `update.agents = "ask"`, the default: after `U` on the board updates kebbie, an `agent updates` window offers the newer Codex, `Codex 0.163 → 0.164`, beside how it is installed. ⏎ runs Codex's own installer, the command Codex's update screen runs, such as `curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh` for a standalone install. `Skip this time` asks again on the next `U`.
- `update.agents = "auto"`: `U` installs it without asking, and the window says so.
- `update.agents = "notify"`: no window; the notice and doctor's line, with the installer's command as the fix, are all.

Once the new Codex is installed, kebbie restarts each Codex task on it when it is idle, as under [Restarts after an update](#restarts-after-an-update). Claude Code updates itself, so it is never listed.

### A model that is gone

A task whose profile names a model Codex no longer lists starts on the model Codex's list names in its place, else on Codex's default. The board gets a notice such as `coder: gpt-5.1-codex-max is gone; started on gpt-6-astra; pick one with m on the Settings page`, the lead is told, and the profile file stays as you wrote it. A profile without a model, and kebbie's own profiles, follow Codex's default.

## Compressed sessions

Codex can compress a session file it hasn't written to for a week to `.jsonl.zst`. kebbie still finds such a session and resumes it, but can't read it. So it doesn't send the task's first prompt again and can't tell from it whether a typed message arrived.

## What a Codex task doesn't get

- The record of test runs that `kebbie evidence` and the reviewer read.
- The vault's tools, until Codex has the vault server from [Set up Codex](#set-up-codex). Without it the lead keeps the task's note: the task's role has it send the lead the note's Problem before its first change, and its decisions and solution as it goes, with `kebbie tell lead`. Nothing holds its edits until the Problem is written.
- Resumes after an API error. An error that stops Codex's turn puts the task under Needs you with the API's message, and the lead hears it, but kebbie does not type it going again.

[Profiles](../reference/configuration.md#profiles) lists every key. [`kebbie setup codex`](../reference/commands.md#setup-and-maintenance) is in the command reference.
