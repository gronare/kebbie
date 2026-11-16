---
name: write-push-summary
description: Write what a ready-to-push task shows first, in the vault's language: a headline naming the change for the user, then one or two sentences on what he will notice, with a concrete example. Use when a task writes or rewrites its sign-off card, and when its card's Built part opens with how the work went, a test, or words only the task knows.
---

# Write the push summary

The **push summary** is the top of the sign-off card: the first thing the user reads before he pushes, on the phone and in the board's push window. Write it for someone with no context: the user a year from now, on a small screen, who never saw the task, its branch or its messages. It names what changes for him and shows it with one example, and stops there. Every word follows [the words the user reads](../playbook/WORDS.md), in the vault's language that your role names.

## Where it lives

The card's shape stays as the playbook's card page sets it ([CARD.md](../playbook/CARD.md)); the push summary is two of its places:

- **The headline**: the change named for the user in a few words, as he would say it ("Leaden kallar tasks vid titeln, inte grennamnet"). The board's push window shows it; the phone does not.
- **The first line under `Built:`**: one or two sentences on what the user will notice, from his side, in the present tense, with a concrete example he will meet: the name, the row, the text or the command he will see. Say which thing you mean ("the title you see on the board", not "the title"), and where he meets it ("when the lead writes to you about a task", not "in the message").
  - Proof only when it is something he can see or check himself, said in his terms ("Syns på raden för en task som är klar."). A test is not proof for him: it stays in the commits.

The lines after it under Built are the details: the rest of what changed, work outside the repo, the screenshot as `![what it shows](<absolute path>)` on a line of its own. Keep the card short: most cards need no detail line.

## The example

The card the user approved as the pattern, as the task writes it to its card file:

```
**Leaden kallar tasks vid titeln, inte grennamnet**

Built:
- När leaden skriver till dig om en task står titeln du ser på boardet, till exempel "Diskvarningen på boardet", aldrig `disk-warning-board`.

Decisions:
- Taskens titel och valvnot skrivs på valvets språk, svenska, och noten heter som titeln (you)
```

Not this: "Leaden nämner varje task med titeln du ser på raden, inte grennamnet. Bevis: ett test visar en tasks meddelande nå leaden med titeln först." It leaves him asking which title, where, and which message, and its proof is a test he cannot see.

## Steps

1. **Find the change.** From your commits and your note, name the one thing the user will notice after the push. Done when you can say it in one sentence with no word from the code.
2. **Find the example.** Pick the concrete thing he will meet that shows the change: a real name, row or text. Done when the sentence would make sense to someone who never saw the task.
3. **Write** the headline and the first Built line, then a detail line only when he would miss it. Done when the first line is one or two sentences and nothing in it tells how the work went.
4. **Check** both against the words the user reads. Done when each sentence is under 25 words, is in the vault's language, and holds no sha, path, flag, test name or kebbie term.
