# Fill the backlog

Give the board's Backlog column work to start from: a local folder or the vault, with Linear beside either.

![The Backlog column by priority; g starts an item as a task, which puts its plan to you](../assets/clips/backlog.webp)

## Pick the sources

1. List the sources in `~/.config/kebbie/config.toml`: `"local"` or `"vault"`, which is where new items are written, and `"linear"` if you want your Linear issues beside them.

   ```toml
   [board.backlog]
   sources = ["vault", "linear"]
   ```

2. Press `R` on the board. The lead reads the vault and Linear, and the Backlog column updates when it has saved what it found.

If you use `"vault"`, it needs the [vault plugin](connect-the-vault.md).

If you use `"linear"`, it needs a Linear MCP server in the lead's Claude. The lead only reads Linear; it never files items there.

If you want no backlog, set `sources = []`.

## Add an item

Write what you want in a line. The lead writes it out in full at once, from your conversation, the code and the notes, and shows you the written item.

On the board, move to the Backlog column and press `n`, then enter the line and a priority. Or from a shell:

```bash
kebbie backlog add --priority high "Fix login — the session drops on reload (app)"
```

The `(app)` at the end, or `--repo app`, is the folder name of the repo the work happens in. Starting the item as a task needs it; the lead fills it in when it can tell.

With the local folder you can also put a file of your own in `~/.config/kebbie/backlog/`, as short as you like. The lead writes it out the next time it reads the backlog.

When the lead is not running, the item waits as you typed it, and the lead writes it out when it starts. With the vault, kebbie keeps a row for it, marked `kept by kebbie`, until the lead's note replaces it.

## What a written-out item holds

Every item, whoever files it, has the sections of its place's template: the vault's task template for vault notes, the file [`board.backlog.template`](../reference/configuration.md#backlog-sources) names for local items, else kebbie's own. `kebbie backlog template` prints the one where new items go. kebbie's own, which the vault's default matches:

| Section | Holds |
| --- | --- |
| Background | What happens today and why it is a problem. |
| Goal | What you can do when it is done. |
| What we know | The files, commits, earlier decisions and links that bear on it. |
| Open questions | What you must decide before work starts. |
| Out of scope | What the item leaves alone. |
| Source | Who said it and when, with your own words. |

Background, Goal and Source are required. An item that lacks a heading or leaves a required section empty is to write. What the lead could not find out goes under Open questions. While one of them blocks the start, the item is drafting, and its row says how many questions wait on you.

A vault note written before the vault had its task template shows as to write too. The lead leaves it as it is until you start it, when its task writes the sections before it changes code, or until you open it and ask the lead to write it out. Format: [The local backlog](../reference/files.md#the-local-backlog).

## Answer an item's questions

1. Select the item in the Backlog column and press `⏎`. Its card opens.

2. Pick a question with `↑` `↓` and press `a`, type your answer and press `⏎`.

The lead folds your answer into the item and takes the question out. Until then the question shows your answer as with the lead. The item is ready once no question waits on you. `o` on the row opens the item's options.

## Refresh from the vault or Linear

Press `R` on the board. The lead reads the sources, and the column updates when it has saved what it found.

## Start an item

1. Select it in the Backlog column.

2. Press `g`. It starts as a task with the whole item as its prompt, and leaves the column while the task runs. While questions on it wait on you, `g` says so first; press `g` again to start it anyway.

A reset brings it back. When the task is done, the lead closes the item.
