# Connect the vault

Give every task a note in your [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp) vault, and log every push.

1. Install the vault plugin. It shows each `claude plugin` command and runs it only on your `y`.

   ```bash
   kebbie setup vault
   ```

2. To list the vault's backlog notes on the board, add it as a source in `~/.config/kebbie/config.toml`. The first source is where the lead files new ideas.

   ```toml
   [board.backlog]
   sources = ["local", "vault"]
   ```

3. Restart the lead so it picks up the vault in its role:

   ```bash
   kebbie lead --restart
   ```

4. Check it. The `vault` line reads `ok` with the plugin's version.

   ```bash
   kebbie doctor
   ```

If the `vault` line reads `too old`, run `claude plugin update vault@gronare`.

## Share the vault between machines

When you run kebbie on a second machine against the same vault, give each machine its own name in its own config.toml. The other machine then sees which one works on a note.

```toml
[machine]
name = "laptop"
```

The `machine` line of `kebbie doctor` shows the name in use. What a claim does: [Two machines, one vault](../explanation/vault.md#two-machines-one-vault).

## Run the vault from your own checkout

kebbie runs the plugin's pinned command. To run another one, set it in `config.toml`:

```toml
[vault]
command = "uv run --project ~/code/notes-vault-mcp notes-vault-mcp"
```

See also: [The vault](../explanation/vault.md).
