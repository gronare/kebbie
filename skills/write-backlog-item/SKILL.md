---
name: write-backlog-item
description: The one shape of a kebbie backlog item, and how to write one out from what is known while it is filed. Use when you file anything in the backlog (an idea the user brings, something a task defers), when kebbie tells you a short item waits to be written out or `kebbie backlog` marks one `to write`, and when the user answers an item's open question.
---

# Write a backlog item

An item is written out at the moment it is filed, by whoever files it, while the context exists. Whoever starts it later (a fresh session, the user a week on) has the item and nothing else, so it carries what that context knew. A one-line item is a **draft**, not an item.

The item is the user's **intent**, written out: their words are terse on purpose, and you add what the code, the notes and the history say about them. What you cannot find out is an open question, never a guess written as fact.

`kebbie` is the absolute path your kebbie role names for it, written out in full so your allow rules match the command; without a kebbie role, `${KEBBIE_BIN:-$HOME/.local/share/kebbie/current/target/release/kebbie}`.

## The item

Metadata: a **title** (what changes, a dozen words at most), a **priority** (`urgent`, `high`, `medium` or `low`) and the **repo** (the folder name of the repo the work happens in; starting the item as a task needs it).

Six sections, in this order, each a `## ` heading:

- **Background**: what happens today and why it is a problem.
- **Goal**: what the user can do when it is done, from the user's side.
- **What we know**: the files, commits, earlier decisions and links that bear on it, each one you read.
- **Open questions**: what the user must decide before work starts, one bullet each. `- [ ] ` before a question that blocks the start; a plain `- ` for one the work can settle. `None.` when there is none.
- **Out of scope**: what this item leaves alone.
- **Source**: who said it and when: `The user, 2026-11-12, in the lead's chat`, `The task billing@web, deferred from its own work`. Keep a short item's own words here, quoted.

## Steps

1. **Mark it drafting**, before you research, so the board shows you are on it:
   - a local item already filed (a short one, or a file the user wrote by hand): set `status = "drafting"` in its header;
   - a short item for the vault: write its note in `Projects/` now with its title, `priority` and `area` and status `draft`, and when kebbie's message names a kept row (`kept:<id>`), run `kebbie backlog filed kept:<id>`, so the board shows your note in place of the row kebbie kept.

   Done when `kebbie backlog` marks it `drafting`.
2. **Research.** Read what bears on it: the code it touches, the notes on its area, the history (`git log` on those files, the tasks and decisions before it), and your conversation with the user. Done when every claim you are about to write in Background and What we know points at something you read in this step, not at memory.
3. **Write.** Fill every section from that reading. Done when each section has content, the Goal reads from the user's side, and each thing you could not establish is a bullet under Open questions.
4. **Save** where your role says new items go:
   - **The vault**: a task note in `Projects/`, the metadata in its frontmatter (`title`, `priority`, `area` as a wikilink to the area note), the six sections as its body, status `backlog`.
   - **The local backlog**: a new item with `kebbie backlog add --priority <priority> --repo <repo> "<Title>" -`, the six sections on stdin in a quoted heredoc (`<<'EOF'`); it prints the item's ref and file. An item already filed you rewrite in its own file: keep its `+++` header, set its fields and `status = "backlog"`, and replace the body after the header.

   A `- [ ] ` question left in it keeps the item drafting on the board until the user answers it.

   Done when the item is saved and `kebbie backlog` (or the vault's backlog) lists it with the state the questions give it.
5. **Show it.** As the lead, put the written item in your reply, its open questions last; when it waited for you (it reached you only at your start, or `kebbie backlog` marked it `to write`), say so in a line. A task lists it under Also noticed.

## Open questions belong to the item

An item's questions stay in its Open questions, with the item: never in `kebbie ask`, never under Needs you. The user answers them on the item's card on the board, and kebbie brings you each answer with the question it answers. Fold the answer into the section it settles (Goal, What we know, Out of scope), take the question out, and save the item again: until you do, the card shows the answer as with you. It is **ready** once no `- [ ] ` question is left.
