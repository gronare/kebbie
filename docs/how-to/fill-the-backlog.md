# Fill the backlog

Give the board's Backlog column work to start from: a local file, the vault, Linear, or any mix.

## Pick the sources

In `~/.config/kebbie/config.toml`:

```toml
[board.backlog]
sources = ["local", "vault", "linear"]
```

The first source is where the lead files new ideas. `"vault"` needs the [vault plugin](connect-the-vault.md). `"linear"` needs a Linear MCP server in the lead's Claude. `[]` turns the backlog off.

## Add items to the local backlog

On the board, move to the Backlog column and press `n`: a title, then a priority. Or:

```bash
kebbie backlog add --priority high "Fix login — the session drops on reload (app)"
```

Or edit `~/.config/kebbie/backlog.md` by hand, one item per line:

```markdown
- [high] Fix login — the session drops on reload (app)
- Write the release notes (web)
```

The `(app)` at the end is the folder name of the repo the work happens in; starting the item as a task needs it. Format: [The local backlog file](../reference/files.md#the-local-backlog-file).

## Refresh from the vault or Linear

Press `R` on the board. The lead reads the sources, and the column updates when it has saved what it found.

## Start an item

Select it and press `g`. It starts as a task with the item as its prompt and leaves the column while the task runs. A reset brings it back; when the task is done, the lead closes the item.
