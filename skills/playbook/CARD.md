# The sign-off card

The card is what the user reads above a task's commits when they push it. They read it cold, between other tasks, on a phone, perhaps a year later with no memory of the task, and push on it, so it says what they now have and what was decided on the way, and asks them nothing: what needed them was asked before it was built.

## Steps

1. **Make the three lists true.** Done when each holds:
   - Every decision you took is recorded where it lives (your note's decisions, or a message to the lead for a Codex task), with its reason and who took it.
   - Everything you noticed outside the task is on the card, each line checked against `kebbie found`.
   - Nothing waits: no obstacle without its decision, no check only the user can make left unasked, and the latest test run on this code is green, or its red is an obstacle you raised. A red that stays once its cause is found goes on the report as `--red-cause "<the line that fails and why>"`; kebbie refuses ready over a red run without it.
   - Before an Also noticed line, `kebbie backlog --search "<words>"` shows whether it is filed already: end the line with the `(found:<id>)` it lists.
2. **Write the card** in the shape below, to a fresh file: its headline and first Built line by the skill `kebbie:write-push-summary`, its Decisions lines by the skill `kebbie:write-decision-line`. Done when every part holds its rules.
3. **Report** with `kebbie report --state ready --card <file> …`. Done when kebbie takes the report: it refuses a card with any part beyond the three below, or one asking the user to confirm, and says what to change.

## The shape

The card is three parts under a headline. Each part opens a paragraph with its label and a colon, which the board draws as the part's header. The labels are English, as written here; the lines under them follow the vault's language, or the user's without a vault.

- **The headline**, in bold: the change named for the user in a few words, as they would say it. It carries no commit or subtask counts and no shas: the push window shows those.
- `Built:` what the user will notice, at most six lines. Its first line is the **push summary**: one or two sentences with a concrete example. The lines after it are the details, only those the user would miss, with an image of anything visual as `![what it shows](<absolute path>)`. What the task changed outside the repo goes here too: a tool it installed, a service it restarted, a site that was down while it worked. For work with no commits, this is the whole card.
- `Decisions:` only the choices the user would weigh (scope, behaviour they see, a trade-off), one self-contained line each, ending with who took it in parentheses: `you`, `lead`, `task` or `investigator`. A question the user answered is written as the decision it settled, in your words, never the question and the answer. kebbie checks who decided against its own records (the user's answers to your questions, the lead's calls, the obstacles' decisions) and keeps it as data: the user reads only your line. A line no record backs shows as `task` in that data. A recorded decision your card leaves out is never pasted in: kebbie names it back to you when you report, to write as a line when the user would weigh it. A decision that was reversed stays, with the line that replaced it: `replaced by: …`. Left out when nothing was decided.
- `Also noticed:` only what lies outside the task, one line per item: what it is and where. kebbie files each line under Found by tasks as it takes the card and links it to its item, `(found:<id>)`, out of the user's view; a thing `kebbie found` lists already gets that ending from you, so it counts as found again. Something on the task's own path was an obstacle and has its line under Decisions. Left out when there is nothing.

An example:

```
**Leaden kallar tasks vid titeln, inte grennamnet**

Built:
- När leaden skriver till dig om en task står titeln du ser på boardet, till exempel "Diskvarningen på boardet", aldrig `disk-warning-board`.

Decisions:
- Taskens titel och valvnot skrivs på valvets språk, svenska, och noten heter som titeln (you)
- En task utan titel än heter sitt namn tills Claude har gett den en (task)

Also noticed:
- Raden för en task i kö visar inte titeln förrän den har startat, på boardets Queued-rad.
```
