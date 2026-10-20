# Vault

The vault plugin, [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp) in Claude Code, is optional. Without it kelpie keeps its backlog and state on its own. With it installed, enabled and configured, kelpie finds it by itself and keeps a note for every task.

## Install

```bash
kelpie setup vault
```

It installs the marketplace, the plugin and its options, showing each `claude plugin` command and running it only on your `y`. `kelpie doctor` then says whether the vault is on, missing (fine, it is optional) or too old for kelpie.

To use the vault's backlog notes on the board too, add it as a source:

```toml
[board.backlog]
sources = ["local", "vault"]
```

## What changes with the vault

- **Start.** Every `kelpie start`, from the lead, the board or the backlog, opens the task's note under `Projects/`, or claims the vault backlog note it was started from. A task across several repos shares one note. The task's role names the note.
- **Edits wait for the Problem.** Until the note's Problem section is written, the task's Claude cannot edit files, nor run a shell command that plainly writes one in its worktree (a `>` redirect, `tee`, `sed -i`, `cp` or `mv` into it, `git apply`, `patch`). Claude writes the Problem in its own words from your prompt; your prompt never goes into the vault as you typed it.
- **Push.** After each push, kelpie writes the repo's log line itself: the commit subjects with their shas.
- **Done** waits until the note has its solution and decisions. kelpie tells the task what is missing, and once they are written it closes the note.
- **Reset** and **drop** put the note back in the backlog.
- **Plans.** `kelpie ask --plan vault:<note>` asks you to approve a plan written in the task's note.
- **Areas.** The board's [Areas](areas.md) page lists the vault's system notes with their repos.

## When the vault does not answer

A vault call that fails never stops a start, a push or a done; the output says what was not written. A vault that does not answer lets an edit through with a warning, so an outage never locks a task out.

The Problem gate is a guard rail, not a sandbox: a script that writes files gets past it. See [Security](security.md).

## Configuration

kelpie runs the vault through the plugin's own pinned command, read from its hooks. `[vault] command` in the machine config overrides it:

```toml
[vault]
command = "uv run --project ~/code/notes-vault-mcp notes-vault-mcp"
```

The Areas page and the vault backlog read the notes-vault-mcp index the plugin's options name, or `VAULT_PATH`, `S3_ENDPOINT`, `S3_BUCKET`, `S3_PREFIX` and `VAULT_CACHE_DIR` as the server takes them. The index lives under `~/.cache/notes-vault-mcp` by default.
