# Connect the vault

Give every task a note in your [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp) vault, and log every push.

1. Open the setup guide at its Vault step:

   ```bash
   kebbie setup vault
   ```

2. Pick a local folder and type its path, or an S3 bucket with its endpoint, bucket, access key and secret, then `⏎`.

3. To list the vault's backlog notes on the board, tick Vault in the Backlog step (`←` goes back to it). The first source ticked is where the lead files new ideas.

4. On the last step, press `⏎`. The guide installs the vault plugin in Claude Code and gives it your answers.

5. Restart the lead so it picks up the vault in its role:

   ```bash
   kebbie lead --restart
   ```

6. Check it. The `vault` line reads `ok` with the plugin's version.

   ```bash
   kebbie doctor
   ```

- If the `vault` line reads `too old`, run `claude plugin update vault@gronare`.
- If you run it without a terminal, `kebbie setup vault` shows each `claude plugin` command and runs it only on your `y`; set the vault's options in Claude Code's `/plugin` screen after.

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
