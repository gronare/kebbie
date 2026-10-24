# Connect the vault

Give every task a note in your [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp) vault, and log every push.

1. Install the vault plugin:

   ```bash
   kebbie setup vault
   ```

   It shows each `claude plugin` command and runs it only on your `y`.

2. Check it:

   ```bash
   kebbie doctor
   ```

   The `vault` line reads `ok` with the plugin's version. `too old` means run `claude plugin update vault@gronare`.

3. To list the vault's backlog notes on the board, add it as a source in `~/.config/kebbie/config.toml`:

   ```toml
   [board.backlog]
   sources = ["local", "vault"]
   ```

   The first source is where the lead files new ideas.

4. Restart the lead so it picks up the vault in its role:

   ```bash
   kebbie lead --restart
   ```

## Share the vault between machines

Running kebbie on a second machine against the same vault, give each machine its own name in its own config.toml, so the other one sees which machine works on a note:

```toml
[machine]
name = "laptop"
```

`kebbie doctor`'s `machine` line shows the name in use. What a claim does: [Two machines, one vault](../explanation/vault.md#two-machines-one-vault).

## Run the vault from your own checkout

kebbie runs the plugin's pinned command. To run another one, set it in `config.toml`:

```toml
[vault]
command = "uv run --project ~/code/notes-vault-mcp notes-vault-mcp"
```

See also: [The vault](../explanation/vault.md).
