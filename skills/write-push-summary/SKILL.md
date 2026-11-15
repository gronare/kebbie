---
name: write-push-summary
description: Write what a ready-to-push task shows first: one sentence on what changes for the user when he pushes, and the one thing that proves it. Use when a task writes or rewrites its sign-off card, and when its card's Built part opens with how the work went instead of what changes.
---

# Write the push summary

The **push summary** is the top of the sign-off card: the first thing the user reads before he pushes, on the phone and in the board's push window. He pushes on it, so it says what changes for him once the push lands and what shows it works, and stops there. Everything else on the card is detail he reads only when he wants to. Every word follows [the words the user reads](../playbook/WORDS.md).

## Where it lives

The card's shape stays as the playbook's card page sets it ([CARD.md](../playbook/CARD.md)); the push summary is two of its places:

- **The headline**: the change's name in a few words, as the user would say it ("Long links on the board open whole"). The board's push window shows it; the phone does not.
- **The first line under `Built:`**: the summary itself, as one bullet of two sentences. The phone shows Built first, so this bullet is the top of the card there.
  1. What changes for the user when it is pushed, from his side, in the present tense: what he sees or can do. About 15 words.
  2. The proof, opening with "Proof:" in the vault's language (Bevis: in Swedish): the one test or measurement that shows it, said as what it shows ("Proof: a test draws a 120-character address whole over two rows."), or what the screenshot on the next line shows.

The lines after it under Built are the details: the rest of what changed, work outside the repo, the screenshot as `![what it shows](<absolute path>)` on a line of its own.

## Steps

1. **Find the change.** From your commits and your note, name the one thing the user will notice after the push. Done when you can say it in one sentence with no word from the code.
2. **Find the proof.** Pick the single test, measurement or screenshot that shows that sentence is true. Done when you can say in one sentence what it shows, without its name or a sha.
3. **Write** the headline and the first Built bullet, then the details under it. Done when the bullet is two sentences, the second opens with the proof word, and nothing in it tells how the work went.
4. **Check** both against the words the user reads. Done when each sentence is under 25 words and holds no sha, path, flag, test name or kebbie term.
