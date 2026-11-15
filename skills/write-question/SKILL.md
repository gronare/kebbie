---
name: write-question
description: Write a question to the user, a choice or a check only he can make, so he can answer it on the first read. Use when you run `kebbie ask` or `kebbie ask --verify`, when the investigator writes an `ask`, and when the lead sends a question back to be asked again.
---

# Write a question

A **question** is read cold on the board or the phone, between other tasks, and answered from what it says alone. It asks him what is his (what he wants, a trade-off, a check only he can make) and gives him what he needs to answer, no more. Every word follows [the words the user reads](../playbook/WORDS.md).

## The shape

kebbie keeps a question's text as one line and shows it whole as the question's title when it fits in 300 characters, so the text reads as a title followed by a few sentences:

1. **The question first**, one short sentence ending in `?`: what he chooses, in his words.
2. **Context**, at most three plain sentences: what he needs to choose, said as what he sees or gets. What the task has built or tested so far stays out.
3. **A link**, when one helps him answer (a mock, a page): its address once, after the last sentence.

Keep the text under 300 characters where you can; the phone then shows it whole at the top.

**Options**, two to four, each `<label>: <what he gets>`:

- The label is one to three words in the vault's language, a choice he would say ("Separate task", "In this task").
- What he gets is one sentence: what he will see or be able to do with that choice, and what it costs him, such as a later delivery.
- `--recommend` names the option you would pick; its reason is the option's own sentence, so the context needs none.

**A check** (`kebbie ask --verify`) is one sentence: what to check and where, as a step he can follow ("Open the board on your phone and pull to refresh: the note line stays."), with the page's address after it.

An example, in an English vault:

`kebbie ask --option "Yes: you can say 'start at 18:00' when you queue a task; this task reaches you later" --option "Not now: a queued task starts once the tasks before it are done" --recommend 2 "Start a task at a time you pick? This task makes a waiting task easy to read on the board."`

## Steps

1. **Name the choice.** Say in one sentence what he chooses and why it is his: it changes what he gets, what it costs or how long it takes. Done when the choice is his and not a question of method, which goes to the lead.
2. **Write** the question, the context and the options. Done when each option says what he gets, and the text follows the order above.
3. **Check** it against the words the user reads. Done when the question is the first sentence, the context is at most three sentences, and no sha, path, flag, branch or kebbie term is left.
