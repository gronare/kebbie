# The vault

This page explains what a vault note adds to a task, how kebbie reaches the vault, and how two machines share one vault.

The vault, such as [mindin](https://mindin.dev), is optional. Without it, kebbie keeps its backlog and state on its own. With a vault command in [`vault.command`](../reference/configuration.md#machine), kebbie keeps a note for every task, whatever agent runs it.

## kebbie runs its own vault command

kebbie writes task notes, claims and log lines, and reads the board's snapshot, by running one command: `vault.command` in its own `config.toml`, such as `uvx mindin`. It is the same for a task in Claude Code, Codex or Cursor, so kebbie's notes never depend on one agent's plugin.

Without `vault.command`, kebbie runs the Claude vault plugin's pinned command from its hooks, and `kebbie doctor` suggests the line to set.

## Setup chooses where the vault lives

The Vault step of `kebbie setup` asks one thing: where the vault lives, on your own server, in an S3 bucket, in a local folder, or nowhere. It preselects the place the vault answers for now, read from its `status`, so two commands that reach the same server show as one place.

When you choose another place, kebbie runs `mindin setup` with it as you apply, in the terminal, where the server signs you in through the browser and S3 asks for its secret. The vault saves the place; kebbie keeps no copy of it. Hosted mindin is listed first and opens once the service is live.

## Setup picks the command

kebbie runs the vault with one command, which it picks itself and shows under the list:

1. `mindin` on `PATH` or in `~/.local/bin`, when it answers;
2. the `vault.command` already set;
3. the Claude Code vault plugin's server, the exact command Claude Code starts;
4. an MCP server in Codex's `~/.codex/config.toml` or Cursor's `~/.cursor/mcp.json`, or `notes-vault-mcp` on `PATH`, that answers as a vault.

A command without `setup`, such as notes-vault-mcp up to 0.13, cannot move the vault, so a new place installs mindin first. So does a place chosen on a machine with no vault command.

## The vault says what it can do

kebbie asks the command what it is with `about --json`: its name, its version and its capabilities. It makes only the calls those capabilities cover, so a vault without claims still keeps notes, and a renamed package is known by its answer, not by a name kebbie expects. The exact shape is [The vault contract](../reference/vault-contract.md).

`kebbie doctor` asks again at each run. When the answer names the package's new name, it suggests the `vault.command` line with it, and `kebbie doctor --fix` writes that line after your `y`.

## The keeper follows the vault

The board, the Areas page and the backlog read kebbie's copy of the vault, never the vault itself. A vault that states `watch` tells the keeper what changed the moment it changes: the keeper runs one `watch` for the vault, with a server or a local folder or S3 alike, and writes each change into its copy. Between changes the keeper then starts no vault command; it waits on the one `watch`. With an older vault, the keeper asks for a fresh snapshot every 20 seconds instead.

## Each agent reaches the vault on its own

`vault.command` is kebbie's connection, not the agent's. A task's agent reads and writes the vault through its own MCP server: the vault plugin in Claude Code, `[mcp_servers.vault]` in Codex's `config.toml`, or Cursor's `mcp.json`. `kebbie setup` adds the vault to each agent you tick, the vault plugin in Claude Code and the vault command as Codex's server, and lists each of those writes on its review before it makes them.

## A note keeps the why after the session ends

A task's Claude forgets everything when its session ends. The reasons behind a change, the options it weighed and what it decided live in its conversation, and the conversation goes. A vault note keeps them, so the next task in the same area, the lead, and you a month later can read why the code is the way it is.

The vault also makes Claude state the problem before it starts changing code. A task that writes down the problem in its own words is less likely to solve the wrong one.

## What changes with the vault

| Step | With the vault |
| --- | --- |
| Start | Every task opens a note under `Projects/`, or claims the vault backlog note it was started from. A task across several repos shares one note. The task's role names the note. |
| Push | After each push, kebbie writes the repo's log line itself: the commit subjects with their shas. |
| Done | Closes the note. |
| Reset and drop | Put the note back in the backlog. |
| Plans | `kebbie ask --plan vault:<note>` asks you to approve a plan written in the task's note, with a short brief for you above the answers. |
| Language | Tasks and the lead write their questions, plans and cards in the language the vault's task template gives, and in English when it gives none. kebbie's own interface stays English. |
| The board | The Backlog page lists ONGOING, the project notes in progress, where [`board.backlog.ongoing`](../reference/configuration.md#keys) puts them. The Areas page lists the vault's system notes with their repos. |

## The note is the task's memory

The note is the task's own memory: research, paths and decisions with their reasons, in whatever shape serves it. A short purpose line at its top, what you can do once the task is done, helps whoever reads it next. Nothing waits for the note: edits go through from the start, and done closes it whatever it holds.

## Two machines, one vault

Two kebbies, say on a Mac and a work devbox, can share one vault. Each claims the notes it works on, so the other sees which machine and which task has a note.

A note gets a `kebbie` key in its frontmatter when a task is started from it, when kebbie opens it for a task, and when `kebbie link` ties it to a task. The key holds `machine` (this kebbie's [`machine.name`](../reference/configuration.md#keys)), `task`, `repo` and `since`. kebbie owns the key: the vault server keeps it as it is when a session rewrites the note, so never edit it by hand.

Claims need a vault that states the `claims` capability. With an older vault kebbie claims nothing, everything else works as before, and `kebbie doctor` says once that the vault is too old for cross-machine claims.

## The other machine sees the claim

- Its board's ONGOING row reads `working on <machine> · <task>` instead of `no task · ⏎ details`.
- Starting the note there asks first: `Start it here and take the note over from <machine>?`
- `kebbie start --backlog <note>` refuses it and names who holds it; `--anyway` starts it all the same and takes the claim over.
- The lead sees the same in the vault's context, as `worked on at <machine> · <task>`.

## A claim ends or goes stale

**Stale.** Each push and each ready report renews the claim. A claim whose note has not moved for more than 14 days shows as `stale` with its days. The vault's weekly `lint --park-stale` may then park the note, so a machine that went away does not hold a note for good.

**End.** Done, a landed task's close, drop and reset clear this machine's claim before the note is closed or goes back to the backlog.

- A note moved or renamed since is found by its claim's machine and task.
- A note deleted since counts as released, and the answer says it is gone.
- When another machine has taken the note over meanwhile, kebbie leaves the note as it is and says so.
- At done, a claim that names this same task and repo under another machine name, such as the host's name from before `machine.name` was set, is cleared all the same and the note closed: one task is not done on two machines.

## Each machine needs its own name

Each machine carries one name, [`machine.name`](../reference/configuration.md#keys), in its claims and on your phone. Give each machine its own, such as `mac` and `devbox`. `kebbie setup` asks for it first, `/kebbie:configure` when it sets kebbie up, and `kebbie hub join` when you connect a phone.

`kebbie doctor` warns while it is unset, and the claims then carry the host's short name.

## kebbie writes the log, not the Stop hook

The vault plugin's Stop hook asks a session to write the repo log before it ends, and to save what you told it once you have written a few messages without anything going to the vault. kebbie writes each task's log lines itself, so it turns off only the log check in the settings it writes for a task: `VAULT_STOP_HOOK_COMMITS=off` in their `env`, which Claude Code passes to the hooks it runs. A task is still asked to save what you or the lead told it. The lead and your own sessions keep the whole hook.

## A vault outage never blocks work

A vault call that fails never stops a start, a push or a done; the output says what was not written.

## The board reads without credentials

The Areas page and the vault backlog read the vault's local index, so a vault stored on S3 needs no credentials on the board's side. The index is as fresh as the last session that used the vault.

## Related

- [Run without the vault](../how-to/run-without-the-vault.md)
- [Connect the vault](../how-to/connect-the-vault.md)
- [The vault contract](../reference/vault-contract.md)
