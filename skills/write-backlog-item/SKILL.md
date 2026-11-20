---
name: write-backlog-item
description: How to write a kebbie backlog item a human scans in seconds, by the backlog style kebbie keeps in the vault. Use when you file anything in the backlog (an idea the user brings, something a task defers), when kebbie tells you a short item waits to be written out, hands you a writer's draft to review, runs you as its writer, or `kebbie backlog` marks one `to write`, and when the user answers an item's open question.
---

# Write a backlog item

An item is written out when it is filed, by whoever files it, while the context exists. Whoever starts it later has the item and what it links to, nothing else. A line alone is a draft, not an item.

`kebbie` is the absolute path your kebbie role names for it, written out in full so your allow rules match the command; without a kebbie role, `${KEBBIE_BIN:-$HOME/.local/share/kebbie/current/target/release/kebbie}`.

## The style

Run `kebbie backlog template` and follow the style it prints: the user's override in the vault, the vault's default, or kebbie's own when the vault gives none. The style decides how the item reads.

What kebbie's code reads is fixed, whatever the style says:

- **title**, **priority** (`urgent`, `high`, `medium` or `low`), **repo** (the folder name of the repo the work happens in) and **status**, in the vault note's frontmatter or the local item's header;
- the open questions, as bullets under the `Open questions` heading (or the questions heading `kebbie backlog template` names): `- [ ] ` before one that blocks the start, `- ` before one the work settles.

Working detail (research, paths, commits, history, reasoning) goes in your own vault notes, linked from the item. An item already filed keeps the shape it was written in: change what is wrong in it, never convert it to another shape.

## Steps

1. **Search first**, with the vault's `search` on the item's words and, as the lead, `kebbie backlog`. Found: add what it lacks, save it, and stop, one item per problem. When kebbie's message names a kept row (`kept:<id>`), run `kebbie backlog filed kept:<id> <note path>`.
2. **Mark it drafting** before you research: a local item gets `status = "drafting"` in its header; a vault item is a note in `Projects/` with its title, `priority`, `area` and status `draft` (then `kebbie backlog filed kept:<id> <path>` when a kept row is named). Done when `kebbie backlog` marks it `drafting`.
3. **Research** the code, the notes on its area, the history and your conversation with the user. What you cannot establish becomes an open question, never a guess written as fact.
   - **No repo given**: choose it from the user's words, the repos kebbie's message lists and `kebbie areas`, by where the code the work touches lives. When none fits, or two fit equally, ask which as a `- [ ] ` question.
   - **Started once written**: when kebbie's message says the user asked to start it once written, kebbie starts it as a task itself once it is saved with its repo and no `- [ ] ` question is left. Keep `- [ ] ` for what the start truly needs, and never start it yourself.
4. **Write** it by the style, and save:
   - **the vault**: the note in `Projects/`, status `backlog`;
   - **the local backlog**: `kebbie backlog add --priority <priority> --repo <repo> "<Title>" -` with the body on stdin in a quoted heredoc (`<<'EOF'`); an item already filed you rewrite in its own file, keeping its `+++` header and setting `status = "backlog"`.

   Done when `kebbie backlog` lists it with the state its questions give it.
5. **Show it.** As the lead, put the written item in your reply, its open questions last; say so in a line when it waited for you. A task lists it under Also noticed.

## As kebbie's writer, and the lead's review

When kebbie runs you as its writer, the lead was asked to write the item out and you do it in its place: search, research and write as above, but save nothing and mark nothing, and answer with the written item alone, `# <title>` first and its body after it. The lead never saw your research, so put what you found in the item or under its open questions.

When kebbie hands the lead a writer's draft, the lead reviews it instead of researching again: it adds what it knows from its conversation with the user, takes out an open question the code or the notes settle, keeps each one only the user can answer, then saves and shows it as steps 4 and 5 say.

## Answers to open questions

The questions stay in the item, never in `kebbie ask`. The user answers them on the item's card, and kebbie brings you the answers with the questions they answer. Fold each answer into the item in one save and take the answered question out; an answer marked `Asked as:` answers the question it became. Keep every question the user has not answered. The item is **ready** once no `- [ ] ` question is left.
