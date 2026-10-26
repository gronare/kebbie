# Run without the vault

Run kebbie with no vault: the backlog comes from the local file and tasks keep no notes.

When the vault plugin is not installed, there is nothing to do. When it is installed and you want kebbie to work without it:

1. Disable the plugin in Claude Code:

   ```bash
   claude plugin disable vault@gronare
   ```

   kebbie uses the vault whenever the plugin is installed and enabled, so this turns it off for every Claude session on the machine.

2. Take `"vault"` out of the backlog sources in `~/.config/kebbie/config.toml`:

   ```toml
   [board.backlog]
   sources = ["local"]
   ```

3. Restart the lead so its role drops the vault:

   ```bash
   kebbie lead --restart
   ```

4. Check the result:

   ```bash
   kebbie doctor
   ```

## What you keep and what goes

- **You keep** the board, tasks, the lead, review and pushes, with the [local backlog file](../reference/files.md#the-local-backlog-file).
- **What goes:** task notes, the Problem gate, push log lines and the Areas page, which lists the vault's system notes.
