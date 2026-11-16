---
name: write-decision-line
description: Write a decision the user reads on a sign-off card, in the vault's language: one self-contained line on a choice he would weigh, as he meets it. Use when a task writes its card's Decisions lines, when a question the user answered settled something on the task, and when the lead records a call with `kebbie decide` or `kebbie obstacles decide`.
---

# Write a decision line

A **decision line** is one line under `Decisions:` on the card the user pushes on, or the call the lead records for kebbie to put there. He reads it, a year later and with no context, to see what was chosen for him and to catch a choice he would have made otherwise. The full reasoning, the evidence and the alternatives live in the task's note. Every word follows [the words the user reads](../playbook/WORDS.md).

## Which choices get a line

Only a choice the user would weigh: the scope (what is in this task and what is not), behaviour he sees, or a trade-off that is his. How the code does it (which module, which key a message is matched on, how a check avoids running twice) is mechanics and stays in the note, however long it was discussed.

A question the user answered becomes **the decision it settled**, written by you in one line: never the question and his answer pasted in. His answer "in this task, and why is the card in English when the language is Swedish" becomes "Taskens titel och valvnot skrivs på valvets språk, svenska, och noten heter som titeln (you)".

## The shape

`<the choice>[, <because> <the reason>] (<who>)`, one sentence of about 20 words, at most 30:

- **The choice** first, as what he sees or gets, readable alone: "A long address continues on the rows below", not the code that does it, and never "this", "the title" or "the message" without saying which.
- **The reason** in one clause after the vault's word for because (eftersom in Swedish), only when the choice alone leaves him asking why.
- **Who** in parentheses, in kebbie's word for it: `you`, `lead`, `task` or `investigator`. kebbie keeps it as data to match the line to its records; the user sees only the line.

A choice that replaced another keeps its line and ends `replaced by: <the line that replaced it>`.

## Steps

1. **List the choices** you took, from your note's decisions and kebbie's records of answers and calls. Done when each choice the user would weigh is listed once, and each piece of mechanics is struck.
2. **Write** each as one line in the shape above. Done when each line opens with the choice and reads alone.
3. **Check** each against the words the user reads. Done when no line passes 30 words and no line holds a question, a sha, path, flag, test name, probe or measurement detail.
