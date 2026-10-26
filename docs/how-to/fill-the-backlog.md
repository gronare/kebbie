# Fill the backlog

Give the board's Backlog column work to start from: a local file, the vault, Linear, or any mix.

## Pick the sources

1. List the sources in `~/.config/kebbie/config.toml`. The first one is where the lead files new ideas.

   ```toml
   [board.backlog]
   sources = ["local", "vault", "linear"]
   ```

2. Press `R` on the board. The lead reads the sources, and the Backlog column updates when it has saved what it found.

If you use `"vault"`, it needs the [vault plugin](connect-the-vault.md).

If you use `"linear"`, it needs a Linear MCP server in the lead's Claude.

If you want no backlog, set `sources = []`.

## Add items to the local backlog

On the board, move to the Backlog column and press `n`, then enter a title and a priority. Or from a shell:

```bash
kebbie backlog add --priority high "Fix login — the session drops on reload (app)"
```

Or edit `~/.config/kebbie/backlog.md` by hand, one item per line:

```markdown
- [high] Fix login — the session drops on reload (app)
- Write the release notes (web)
```

The `(app)` at the end is the folder name of the repo the work happens in. Starting the item as a task needs it. Format: [The local backlog file](../reference/files.md#the-local-backlog-file).

## Refresh from the vault or Linear

Press `R` on the board. The lead reads the sources, and the column updates when it has saved what it found.

## Start an item

1. Select it in the Backlog column.

2. Press `g`. It starts as a task with the item as its prompt, and leaves the column while the task runs.

A reset brings it back. When the task is done, the lead closes the item.
