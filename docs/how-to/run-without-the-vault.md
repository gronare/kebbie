# Run without the vault

kelpie needs no vault. Without one, the backlog comes from the local file and tasks keep no notes.

When the vault plugin is not installed, there is nothing to do.

When it is installed but you want kelpie to work without it:

1. Disable the plugin in Claude Code. kelpie uses the vault whenever the plugin is installed and enabled, so this turns it off for every Claude session on the machine:

   ```bash
   claude plugin disable vault@gronare
   ```

2. Take `"vault"` out of the backlog sources in `~/.config/kelpie/config.toml`:

   ```toml
   [board.backlog]
   sources = ["local"]
   ```

3. Restart the lead so its role drops the vault:

   ```bash
   kelpie lead --restart
   ```

4. Check:

   ```bash
   kelpie doctor
   ```

What you keep: the board, tasks, the lead, review and pushes, with the [local backlog file](../reference/files.md#the-local-backlog-file). What goes: task notes, the Problem gate, push log lines and the Areas page, which lists the vault's system notes.
