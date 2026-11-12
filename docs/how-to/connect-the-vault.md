# Connect the vault

Give every task a note in your vault, such as [mindin](https://mindin.dev), and log every push.

1. Open the setup guide at its Vault step:

   ```bash
   kebbie setup vault
   ```

2. Choose where the vault lives: **Your own server**, **S3 bucket** or **Local folder**, and fill in its fields, such as the address `https://vault.example.com`. The place in use is preselected, such as `Your own server  https://vault.example.com · signed in`. Press `⏎`.

3. To list the vault's backlog notes on the board and file new items there, tick Vault in the Backlog step. It takes the local file's place.

4. On the last step, read the `mindin` and `vault` lines and press `⏎`. When the place is new, `mindin setup` runs first, in the terminal: a server opens the browser to sign you in, and S3 asks for its access key and secret. The guide then writes `vault.command` to kebbie's `config.toml`, installs the vault plugin in Claude Code and adds `[mcp_servers.vault]` to Codex's `config.toml` when you ticked Codex.

5. Restart the lead so it picks up the vault in its role:

   ```bash
   kebbie lead --restart
   ```

6. Check it. The `vault` line reads `ok` with the vault's name, version and capabilities.

   ```bash
   kebbie doctor
   ```

- If no vault command on the machine can run `mindin setup`, the review lists `install` first: mindin's installer runs, then its setup, and the guide points kebbie and each agent at the `mindin` it installed.
- If your server reads `not signed in`, keep it chosen: `mindin setup` signs you in as you apply.
- If you choose **Local folder**, make the folder first: `mindin setup` takes only a folder that exists.
- If the `vault` line reads `too old`, run `claude plugin update vault@gronare`, or point `vault.command` at a newer vault.
- If the `vault` line names the package's new name, run `kebbie doctor --fix` and answer `y` to write the new `vault.command`.
- If you run it without a terminal, `kebbie setup vault` shows the vault command it would run, or the line that installs mindin, and writes nothing.
- If a task runs in Cursor, add the vault to Cursor's `mcp.json` yourself. See [Each agent reaches the vault on its own](../explanation/vault.md#each-agent-reaches-the-vault-on-its-own).

## Share the vault between machines

When you run kebbie on a second machine against the same vault, give each machine its own name in its own config.toml. The other machine then sees which one works on a note.

```toml
[machine]
name = "laptop"
```

The `machine` line of `kebbie doctor` shows the name in use. What a claim does: [Two machines, one vault](../explanation/vault.md#two-machines-one-vault).

## Run the vault from your own checkout

kebbie runs the command in `vault.command`. To run another one, such as a checkout of your own, set it in `config.toml`:

```toml
[vault]
command = "uv run --project ~/code/mindin mindin"
```

`kebbie doctor` then shows what it answered. What a vault command answers: [The vault contract](../reference/vault-contract.md).

See also: [The vault](../explanation/vault.md).
