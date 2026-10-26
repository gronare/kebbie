# The vault

This page explains what a vault note adds to a task, and how two machines share one vault.

The vault plugin, [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp) in Claude Code, is optional. Without it, kebbie keeps its backlog and state on its own. With it installed, enabled and configured, kebbie finds it by itself and keeps a note for every task.

## A note keeps the why after the session ends

A task's Claude forgets everything when its session ends. The reasons behind a change, the options it weighed and what it decided live in its conversation, and the conversation goes. A vault note keeps them, so the next task in the same area, the lead, and you a month later can read why the code is the way it is.

The vault also makes Claude state the problem before it starts changing code. A task that writes down the problem in its own words is less likely to solve the wrong one.

## What changes with the vault

| Step | With the vault |
| --- | --- |
| Start | Every task opens a note under `Projects/`, or claims the vault backlog note it was started from. A task across several repos shares one note. The task's role names the note. |
| Push | After each push, kebbie writes the repo's log line itself: the commit subjects with their shas. |
| Done | Waits until the note has its solution and decisions. kebbie tells the task what is missing, and once they are written it closes the note. |
| Reset and drop | Put the note back in the backlog. |
| Plans | `kebbie ask --plan vault:<note>` asks you to approve a plan written in the task's note. |
| The board | The Backlog column lists ONGOING, the project notes in progress, where [`board.backlog.ongoing`](../reference/configuration.md#keys) puts them. The Areas page lists the vault's system notes with their repos. |

## Edits wait for the Problem

Until the note's Problem section is written, the task's Claude cannot edit files, nor run a shell command that plainly writes one in its worktree: a `>` redirect, `tee`, `sed -i`, `cp` or `mv` into it, `git apply`, `patch`.

Claude writes the Problem in its own words from your prompt; your prompt never goes into the vault as you typed it.

## Two machines, one vault

Two kebbies, say on a Mac and a work devbox, can share one vault. Each claims the notes it works on, so the other sees which machine and which task has a note.

A note gets a `kebbie` key in its frontmatter when a task is started from it, when kebbie opens it for a task, and when `kebbie link` ties it to a task. The key holds `machine` (this kebbie's [`machine.name`](../reference/configuration.md#keys)), `task`, `repo` and `since`. kebbie owns the key: the vault server keeps it as it is when a session rewrites the note, so never edit it by hand.

Claims need a notes-vault-mcp that has `task claim`. With an older vault plugin kebbie claims nothing, everything else works as before, and `kebbie doctor` says once that the plugin is too old for cross-machine claims.

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

## Each machine needs its own name

Each machine carries one name, [`machine.name`](../reference/configuration.md#keys), in its claims and on your phone. Give each machine its own, such as `mac` and `devbox`. `/kebbie:configure` asks for it when it sets kebbie up, and `kebbie hub join` when you connect a phone.

`kebbie doctor` warns while it is unset, and the claims then carry the host's short name.

## kebbie writes the log, not the Stop hook

The vault plugin's Stop hook asks a session to write the repo log before it ends. kebbie writes each task's log lines itself, so it turns that hook off in the settings it writes for a task: `VAULT_STOP_HOOK=off` in their `env`, which Claude Code passes to the hooks it runs. The lead and your own sessions keep it.

## A vault outage never blocks work

A vault call that fails never stops a start, a push or a done; the output says what was not written. A vault that does not answer lets an edit through with a warning, so an outage never locks a task out.

## The Problem gate is a guard rail, not a lock

The Problem gate catches the common ways Claude writes files. A script that writes files gets past it. That is deliberate: the gate exists to make writing the problem down the easy path, not to contain a hostile process. See [The trust boundary](trust-boundary.md).

## The board reads without credentials

The Areas page and the vault backlog read notes-vault-mcp's local index, so a vault stored on S3 needs no credentials on the board's side. The index is as fresh as the last session that used the vault.

## Related

- [Run without the vault](../how-to/run-without-the-vault.md)
- [Connect the vault](../how-to/connect-the-vault.md)
