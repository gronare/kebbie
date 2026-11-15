---
name: write-plan-brief
description: The decision brief a plan's approval question carries, written for the user in the vault's language: one line on what they approve, then what they get, what it costs, what is left out and what is already decided. Use when you ask the user to approve a plan with `kebbie ask --plan`, and when kebbie refuses a plan question.
---

# Write a plan's brief

The user approves a plan from its **brief**: the text of the `kebbie ask --plan` question, which the board shows above the four answers, with the whole plan behind a Read the whole plan link. They read it cold, often on the phone, between other tasks, and decide from it alone. The plan is written for the work; the brief is written for the decision.

`kebbie` is the absolute path your role names for it.

## The shape

```
<One sentence: what the user approves.>

**<What you get>**
- <one fact>

**<What it costs>**
- <one fact>

**<What is left out>**
- <one fact>

**<Decisions already taken>**
- <one decision, with its reason>
```

- **The first line** is the question's title: one sentence of about 15 words saying what the user says yes to, as an outcome.
- **Four parts**, each a bold heading on its own line with one to four bullets under it:
  1. What you get: what the user can see or do once it is built.
  2. What it costs: risks, behaviour that changes or slows, and everything the user must do himself (a setting, a credential, a check on a device). "None" is an answer.
  3. What is left out: what the user might expect from this plan that it leaves for later or for good.
  4. Decisions already taken: each choice made for the user that they may want to overturn, with its reason in a few words.
- **The language** is the vault's, which your role names: the headings and every line. English for an English vault or none. The interface's own English words (Approve, Read the whole plan) appear only quoted, as what the user sees.

| Language | The four headings |
| --- | --- |
| Swedish | Vad du får · Vad det kostar · Vad som inte ingår · Beslut som redan är tagna |
| English | What you get · What it costs · What is left out · Decisions already taken |

Another language takes the same four, translated.

## Steps

1. **Read the plan as the user.** List each thing they will notice: what they see, do, wait for or must do themselves. Done when every part of the plan the user notices is on the list, and work they never notice (a refactor, a move, test setup, how the commits are split) is off it.
2. **Sort** the list into the four parts. The goal the user gave stays out: they know it, and the first line says what this plan does about it. Done when each part has one to four bullets, and an empty part says so in one bullet.
3. **Write** in the vault's language, in **plain** sentences of about 15 words, one fact per bullet, naming things in the user's words. Done when the whole brief is under 150 words and every line holds:
   - the user's words for things, never an internal name (a function, flag, table, module, recipe or subtask) they did not use;
   - no commit id, sha, branch name, file path, command, code or date: those live in the plan;
   - no status report of what is committed or tested, and no count of commits or subtasks.
4. **Ask.** With the plan written where your role says, put the brief on stdin:

   ```sh
   kebbie ask --plan vault:Projects/<note>.md - <<'EOF'
   <the brief>
   EOF
   ```

   `--plan <file>` in place of `vault:<note>` for a plan file. Done when kebbie answers that it asked the user. A refusal names each thing to fix: fix those in the brief and ask again.

## Example

Not this, one paragraph written for the agent:

> Plan: add plan_brief::check to questions::parse on f33621d, refuse --plan text without a title/body split, update roles/task.md and the plan_step in src/peers.rs, then a CHANGES.md line. Approve?

This, for a Swedish vault:

```
Godkänn att en plan bara kan läggas fram med en kort beslutsbrief överst.

**Vad du får**
- Varje plan börjar med en mening om vad du säger ja till.
- Under den står vad du får, vad det kostar, vad som inte ingår och vad som är bestämt.
- Texten är på valvets språk, och knapparna är som förut.

**Vad det kostar**
- En uppgift som skriver en lång eller teknisk fråga får skriva om den.

**Vad som inte ingår**
- Planerna i valvet skrivs inte om; de finns kvar bakom länken till hela planen.

**Beslut som redan är tagna**
- Kebbie granskar formen men inte språket, så att granskningen alltid ger samma svar.
```
