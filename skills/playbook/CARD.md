# The sign-off card

The card is what the user reads above a task's commits when they push it. They read it cold, between other tasks, and push on it, so it says what they now have and what was decided on the way, and asks them nothing: what needed them was asked before it was built.

## Steps

1. **Make the three lists true.** Done when each holds:
   - Every decision you took is recorded where it lives (your note's decisions, or a message to the lead for a Codex task), with its reason and who took it.
   - Everything you noticed outside the task is on the card, each line checked against `kebbie found`.
   - Nothing waits: no obstacle without its decision, no check only the user can make left unasked, and the latest test run on this code is green, or its red is an obstacle you raised.
2. **Write the card** in the shape below, to a fresh file. Done when every part holds its rules.
3. **Report** with `kebbie report --state ready --card <file> …`. Done when kebbie takes the report.

## The shape

The card is three parts under a headline. Each part opens a paragraph with its label and a colon, which the board draws as the part's header. The labels are English, as written here; the lines under them follow the vault's language, or the user's without a vault.

- **The headline**, in bold: what the user can do now, or what works now, in their words. It carries no commit or step counts and no shas: the push window shows those.
- `Built:` what the user can do now and where to see it (a page, a command, a file), at most six lines, with an image of anything visual as `![what it shows](<absolute path>)`. What the task changed outside the repo goes here too: a tool it installed, a service it restarted, a site that was down while it worked. For work with no commits, this is the whole card.
- `Decisions:` each choice taken during the work, one line each: the choice, `because` its reason, and who took it in parentheses: `you`, `lead`, `task` or `investigator`. A decision that was reversed stays, with the line that replaced it: `replaced by: …`. Left out when nothing was decided.
- `Also noticed:` only what lies outside the task, one line per item: what it is and where. kebbie files each line under Found by tasks as it takes the card and ends it with its item, `(found:<id>)`; a thing `kebbie found` lists already gets that ending from you, so it counts as found again. Something on the task's own path was an obstacle and has its line under Decisions. Left out when there is nothing.

An example:

```
**A long link on the board opens whole, also where it wraps or is cut short.**

Built:
- Cmd-click any part of a wrapped link opens the full address: in a task's CI lines, its messages and the push window.

Decisions:
- A link wider than its row breaks across rows, because clipping hid the end of the address (task).
- The real-terminal check went to you before the push, because this task cannot reach a running terminal (investigator).

Also noticed:
- The note line under a task could link to the note too, in the board's detail lines.
```
