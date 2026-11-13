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

The body is the **template** of the place the item is saved: its sections in order, each a `## ` heading. `kebbie backlog template` prints the template where new items go (`--vault` or `--local` for the other place), marking the required sections and the one the open questions sit under. Fill the template your source gives, every section of it.

kebbie's own template, which the vault's default task template matches:

- **Background** (required): what happens today and why it is a problem.
- **Goal** (required): what the user can do when it is done, from the user's side.
- **What we know**: the files, commits, earlier decisions and links that bear on it, each one you read.
- **Open questions**: what the user must decide before work starts, one bullet each. `- [ ] ` before a question that blocks the start; a plain `- ` for one the work can settle. `None.` when there is none.
- **Out of scope**: what this item leaves alone.
- **Source** (required): who said it and when: `The user, 2026-11-12, in the lead's chat`, `The task billing@web, deferred from its own work`. Keep a short item's own words here, quoted.

A template with other sections is filled the same way, each section from its heading, the questions as bullets under its questions section.

## Steps

1. **Search first.** Look for the item already filed: with the vault, its `search` on the item's words; as the lead, `kebbie backlog` too. When it is there, add your source to its Source (`The task billing@web, seen again, 2026-11-14`) and what you saw that it lacks to What we know, save it, and stop: one item per problem, however many tasks meet it. Done when you found it and added to it, or searched where you can and it is not there.
2. **Mark it drafting**, before you research, so the board shows you are on it:
   - a local item already filed (a short one, or a file the user wrote by hand): set `status = "drafting"` in its header;
   - a short item for the vault: write its note in `Projects/` now with its title, `priority` and `area` and status `draft`, and when kebbie's message names a kept row (`kept:<id>`), run `kebbie backlog filed kept:<id>`, so the board shows your note in place of the row kebbie kept.

   Done when `kebbie backlog` marks it `drafting`.
3. **Research.** Read what bears on it: the code it touches, the notes on its area, the history (`git log` on those files, the tasks and decisions before it), and your conversation with the user. Done when every claim you are about to write in Background and What we know points at something you read in this step, not at memory.
4. **Write.** Fill every section of the template from that reading. Done when each required section has content, every heading of the template is there, the goal reads from the user's side, and each thing you could not establish is a bullet under the questions section.
5. **Save** where your role says new items go:
   - **The vault**: a task note in `Projects/`, the metadata in its frontmatter (`title`, `priority`, `area` as a wikilink to the area note), the template's sections as its body, status `backlog`.
   - **The local backlog**: a new item with `kebbie backlog add --priority <priority> --repo <repo> "<Title>" -`, the template's sections on stdin in a quoted heredoc (`<<'EOF'`); it prints the item's ref and file. An item already filed you rewrite in its own file: keep its `+++` header, set its fields and `status = "backlog"`, and replace the body after the header.

   A `- [ ] ` question left in it keeps the item drafting on the board until the user answers it.

   Done when the item is saved and `kebbie backlog` (or the vault's backlog) lists it with the state the questions give it.
6. **Show it.** As the lead, put the written item in your reply, its open questions last; when it waited for you (it reached you only at your start, or `kebbie backlog` marked it `to write`), say so in a line. A task lists it under Also noticed.

## Open questions belong to the item

An item's questions stay under its questions section (Open questions in kebbie's own template), with the item: never in `kebbie ask`, never under Needs you. The user answers them on the item's card on the board, and kebbie brings you each answer with the question it answers. Fold the answer into the section it settles (such as Goal, What we know or Out of scope), take the question out, and save the item again: until you do, the card shows the answer as with you. It is **ready** once no `- [ ] ` question is left.
