# Backlog

The board's Backlog column lists work you have not started yet, by priority. `[board.backlog] sources` says where it comes from, in any combination:

| Source | Where the items are |
| --- | --- |
| `"local"` | `backlog.md` next to the machine config. The default. |
| `"vault"` | The vault's backlog notes. See [Vault](vault.md). |
| `"linear"` | Your open Linear issues, read by the lead through a Linear MCP server. |

```toml
[board.backlog]
sources = ["local", "vault"]
```

The first source listed is where the lead files new ideas. `[]` turns the backlog off.

## Working the backlog

- `g` on an item starts it as a task, with the item as its prompt. While the task runs, the item leaves the column. A reset brings it back; when the task is done, the lead closes the item.
- `R` asks the lead to refresh the backlog. The lead saves what it found with `kelpie backlog save`.
- `kelpie backlog` prints what the board shows.

## The local backlog file

`~/.config/kelpie/backlog.md` is read as it changes, and you can edit it by hand. One item per line:

```markdown
- [high] Fix login — the session drops on reload (app)
- Write the release notes (web)
```

- The priority is `urgent`, `high`, `medium` or `low`, or none.
- The summary after ` — ` is optional.
- The area in parentheses is the folder name of the repo the work happens in. It is optional, but starting a task needs it.
- The title is how kelpie knows the item, so keep titles unique.

Other lines are yours; kelpie only appends. Add an item with `a` in the Backlog column (a title, then a priority) or:

```bash
kelpie backlog add --priority high "Fix login — the session drops on reload (app)"
```
