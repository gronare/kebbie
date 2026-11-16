# Fill the backlog

Give the board's Backlog page work to start from: a local folder or the vault, with Linear beside either.

![The backlog by priority; g starts an item as a task, which puts its plan to you](../assets/clips/backlog.webp)

## Pick the sources

1. List the sources in `~/.config/kebbie/config.toml`: `"local"` or `"vault"`, which is where new items are written, and `"linear"` if you want your Linear issues beside them.

   ```toml
   [board.backlog]
   sources = ["vault", "linear"]
   ```

2. Press `R` on the board. The lead reads the vault and Linear, and the Backlog page updates when it has saved what it found.

If you use `"vault"`, it needs the [vault plugin](connect-the-vault.md).

If you use `"linear"`, it needs a Linear MCP server in the lead's Claude. The lead only reads Linear; it never files items there.

If you want no backlog, set `sources = []`.

## Add an item

Write what you want in a line. The lead writes it out in full at once, from your conversation, the code and the notes, and shows you the written item.

On the board, press `2` for the Backlog page and press `n`, then enter the line and a priority. Or from a shell:

```bash
kebbie backlog add --priority high "Fix login — the session drops on reload (app)"
```

The `(app)` at the end, or `--repo app`, is the folder name of the repo the work happens in. Starting the item as a task needs it; the lead fills it in when it can tell.

With the local folder you can also put a file of your own in `~/.config/kebbie/backlog/`, as short as you like. The lead writes it out the next time it reads the backlog.

When the lead is not running, the item waits as you typed it, and the lead writes it out when it starts. With the vault, kebbie keeps a row for it, marked `kept by kebbie`, until the lead's note replaces it.

## What a written-out item holds

A written-out item follows kebbie's [backlog style](#change-how-an-item-reads), which `kebbie backlog template` prints:

| Part | Holds |
| --- | --- |
| Title | What changes, in at most 12 words. |
| Purpose | One line on what you can do once it is done. |
| Plan | 3 to 5 bullets, each with a short bold lead. |
| Open questions | What you must decide before work starts (`- [ ] `), and what the work settles (`- `). |
| Left out | One line on what the item leaves alone. |

The research behind it stays in the lead's own vault notes, linked from the item. kebbie itself reads only the title, priority, repo and status, and the questions under `Open questions`. While a `- [ ] ` question waits on you, the item is drafting, and its row says how many questions wait on you.

An item is to write while it is a draft: a line you added, a note the vault filed as a draft, or no more than one line of text. An item written in an older shape, such as Background, Goal and Source, keeps that shape and reads as before. Format: [The local backlog](../reference/files.md#the-local-backlog).

## Change how an item reads

How a written-out item reads is kebbie's backlog style. With the vault, kebbie keeps it in `.vault/styles/backlog.md`, so every agent that files into the vault gets it, also on other machines and in sessions kebbie did not start. `kebbie backlog template` prints the style that applies.

To use your own, write `.vault/styles/backlog.override.md` in the vault, in Obsidian or on disk. It applies in place of kebbie's, and kebbie never writes it. Without the vault, or with one older than styles, kebbie's own applies. See [The backlog style](../reference/vault-contract.md#the-backlog-style).

## Have an item written out

Select an item marked to write on the Backlog page and press `⏎`: its options open with **Write it out** first and **Open the card** second. The lead writes it out in place by the skill and shows you the written item. From a shell:

```bash
kebbie backlog write Projects/fix-login.md
```

The reference is the one `kebbie backlog` lists.

From the moment you ask, the item's row and card show `writing…` with how long ago you asked (`writing… 2 h`), also after a restart, until its text is complete. On the phone it shows as drafting. **Write it out** is offered only for an item to write that no one is writing yet; `kebbie backlog write` refuses one written out or being written. A request the lead has not completed in 24 hours may be sent again: the options show **Write it out again (asked 26 h ago)**, and `kebbie backlog write` accepts it.

## An idea merged into an older note

When the lead files an item you added into a note that was already there, your row stays on the Backlog page for 7 days as `<your words> · merged into <note>`. `⏎` on it opens that note, and `g` starts it.

## Answer an item's questions

1. Select the item on the Backlog page and press `⏎`. Its card opens.

2. Pick a question with `↑` `↓` and press `a`, type your answer and press `⏎`.

The lead folds your answer into the item and takes the question out. Until then the question shows your answer as with the lead. The item is ready once no question waits on you. `o` on the row opens the item's options.

## Refresh from the vault or Linear

Press `R` on the board. The lead reads the sources, and the column updates when it has saved what it found.

## Start an item

1. Select it on the Backlog page.

2. Press `g`. It starts as a task with the whole item as its prompt, and leaves the column while the task runs. While questions on it wait on you, `g` says so first; press `g` again to start it anyway.

A reset brings it back. When the task is done, the lead closes the item.
