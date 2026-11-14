# Obstacles and findings

This page explains what happens when a task meets something in its way, or a review finds something, and why the task's sign-off card asks you nothing.

## A task stops at an obstacle instead of going around it

An obstacle is something on the task's own path: a precondition its work needs is missing, a part of its plan cannot be built as planned, a trap makes it unsafe, or it goes only by a workaround. A red test the task did not cause, and a check it cannot run, count too.

The task stops that piece of work, writes the obstacle down with what it read and every way it sees, and raises it with [`kebbie obstacle`](../reference/commands.md#kebbie-obstacle---on-work-text). kebbie keeps it, and the task's row reads `obstacle · investigating` under Working, where a blocked report alone would sink under Idle. The task goes on with work the obstacle does not touch, or reports blocked.

## kebbie's investigator decides, in line with the task's goal

The keeper starts kebbie's own investigator on each raised obstacle, the way it starts the reviewer: an agent running the skill [`kebbie:investigate`](../../skills/investigate/SKILL.md) with your own settings, so it has what every agent here has: the code and its git, the vault, the web and kebbie's reading commands. It reads what the task is for, the code the obstacle names, what the vault already knows, and a tool's own documentation, then decides one of four, and kebbie carries it out:

| Decision | When | What kebbie does |
| --- | --- | --- |
| `workaround` | What it goes around changes anyway, or the real fix is out of proportion and the workaround stays contained | Tells the task to go on, with what replaces the workaround; when nothing does, files the real fix in the backlog. |
| `subtask` | The missing piece belongs to this task and is small next to it | Adds it to the task's subtasks, with the investigator's reason, ahead of the subtask the task's commits go to now, sends the next commits to it, and tells the task its id. The task marks it done when it is built. One per task needs no answer from you; a second asks you, unless your earlier answer in the task already covers it. |
| `task` | The missing piece is work of its own, or the same obstacle was met before | Queues that task to start at once, the lead's way, or points to a running task that builds it. The blocked work waits for it. |
| `ask` | The trade-off is yours and you have not answered one like it in this task, or the investigator is unsure | Asks you on the board and the phone, with the options and a recommendation. Your answer goes to the task as yours. |

The investigator prefers the way that leaves the code better: make the change easy, then make the easy change.

What you already decided in the task stands: the plan you approved, your answers to its questions and your word on its earlier obstacles. When one of them covers an obstacle's choice, such as fixing the cause rather than working around it, the investigator applies it and the decision names your answer.

Each decision is recorded with who took it: the investigator, the lead or you, never the task itself. `kebbie obstacles` lists them, and the task's note keeps them.

## One investigation per obstacle

The investigator is the most expensive reader kebbie runs, so it runs once per obstacle, up to 15 minutes. The same obstacle raised again gets its decision back. A second obstacle on the same piece of work, or a task's third, goes to you as one question: does the plan hold?

It does not run for a question of method, which is the task's own call, nor for red CI after a push, a conflict in a push or a fault in kebbie, which have their own paths. With [`investigate.profile`](../reference/configuration.md#investigation) set to `"none"` it never runs, and obstacles go to the lead, which decides them with `kebbie obstacles decide`. An investigator that gives no decision leaves its obstacle to the lead the same way.

## Review findings are resolved, not passed along

Bugs, broken rules, missing tests and missing docs go to the task as work. A finding the task cannot settle by changing code goes to the investigator first: the reviewer marks it `confirm:` for your intent, `scope:` for work outside the task or an Also noticed line on its own path, and `gap:` for a test or docs gap only someone else can close. The answer is often your own words in the task's note, which the reviewer never sees. When it settles a verdict that only held such findings, the review passes on its word. Only its `ask` reaches you.

A point decided once stays decided. The reviewer's brief lists the task's decided obstacles with their decisions and the items it filed from its Also noticed lines, and kebbie leaves out a finding that names a decided point's `file:line` or its found item, so the same point never reaches the investigator twice. A verdict left with no finding passes.

A thing a second task notices is raised once, as its own obstacle, and the investigator decides its root cause, usually one task for it. Every task that found it hears to stop working around it.

## The plan is living

A task plans what it knows, builds, learns and revises. It tells the lead each revision with its reason. A revision that changes what you get, such as a part of a plan you approved left out, is asked before it is built.

## The card asks you nothing

What needed you was asked before it was built, so the sign-off card has three parts and no confirm list. kebbie refuses a card with any other part, and a ready report over a red test run unless the task names the cause it found.

- **Built**: what you can do now and where to see it, including anything changed outside the repo.
- **Decisions**: each choice taken on the way, its reason and who took it: you, the lead, the task or the investigator. Who decided comes from kebbie's own records, never the task's word, and a line no record backs shows as the task's.
- **Also noticed**: only what lies outside the task. kebbie files each line under Found by tasks on the board's backlog, and the line ends with its item, `(found:f3)`.

A card the task writes again keeps the version before it, so the board and your phone show what changed. The part labels are English; the text under them follows your vault's language. The procedure is in the skill [`kebbie:playbook`](../../skills/playbook/SKILL.md).

## What tasks notice is counted, not filed twice

Found by tasks, at the top of the board's backlog, holds what tasks listed under Also noticed: one item per thing, with the tasks that found it and how often. When a second task notices the same thing, the item counts up and adds that task, instead of a new item beside it, so something every task trips over stands out. From it you start a task, hand it to a running task, keep it in the backlog, or dismiss it, and the card it came from shows your choice. See [In the Backlog column](../reference/board.md#in-the-backlog-column).
