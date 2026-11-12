# Run without the vault

Run kebbie with no vault: the backlog comes from the local file and each task's decisions go to kebbie's own note for it.

With no `vault.command` and no vault plugin, there is nothing to do. Otherwise:

1. Take `vault.command` out of `~/.config/kebbie/config.toml`, or pick **No vault** at the Vault step of `kebbie setup`.

2. If the vault plugin is installed in Claude Code, disable it:

   ```bash
   claude plugin disable vault@gronare
   ```

   Without `vault.command`, kebbie runs the plugin's command whenever the plugin is installed and enabled, so this turns the vault off for kebbie and for every Claude session on the machine.

3. Take `"vault"` out of the backlog sources in `~/.config/kebbie/config.toml`:

   ```toml
   [board.backlog]
   sources = ["local"]
   ```

4. Restart the lead so its role drops the vault:

   ```bash
   kebbie lead --restart
   ```

5. Check the result:

   ```bash
   kebbie doctor
   ```

## What you keep and what goes

- **You keep** the board, tasks, the lead, review and pushes, with the [local backlog](../reference/files.md#the-local-backlog), and a note per task in kebbie's state, `notes/<task>@<repo>.md`, with its recipe and the assessor's word on its review as dated decisions.
- **What goes:** vault task notes, the Problem gate, push log lines and the Areas page, which lists the vault's system notes.
