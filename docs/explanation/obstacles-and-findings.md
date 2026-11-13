# Obstacles and findings

This page explains what happens when a task meets something in its way, or a review finds something, and why the task's sign-off card asks you nothing.

## A task stops at an obstacle instead of going around it

An obstacle is something on the task's own path: a precondition its work needs is missing, a step of its plan cannot be built as planned, a trap makes it unsafe, or it goes only by a workaround. A red test the task did not cause, and a check it cannot run, count too.

The task stops that piece of work, writes the obstacle down with what it read and every way it sees, and raises it with [`kebbie obstacle`](../reference/commands.md#kebbie-obstacle---on-work-text). kebbie keeps it, and the task's row reads `obstacle · investigating` under Working, where a blocked report alone would sink under Idle. The task goes on with work the obstacle does not touch, or reports blocked.

## kebbie's investigator decides, in line with the task's goal

The keeper starts kebbie's own investigator on each raised obstacle, the way it starts the reviewer: an agent running the skill [`kebbie:investigate`](../../skills/investigate/SKILL.md) with your own settings, so it has what every agent here has: the code and its git, the vault, the web and kebbie's reading commands. It reads what the task is for, the code the obstacle names, what the vault already knows, and a tool's own documentation, then decides one of four, and kebbie carries it out:

| Decision | When | What kebbie does |
| --- | --- | --- |
| `workaround` | What it goes around changes anyway, or the real fix is out of proportion and the workaround stays contained | Tells the task to go on, with what replaces the workaround; when nothing does, files the real fix in the backlog. |
| `subtask` | The missing piece belongs to this task and is small next to it | Puts it first among the task's unpushed steps and tells the task. One per task needs no answer from you; a second asks you. |
| `task` | The missing piece is work of its own, or the same obstacle was met before | Queues that task to start at once, the lead's way, or points to a running task that builds it. The blocked work waits for it. |
| `ask` | The trade-off is yours, or the investigator is unsure | Asks you on the board and the phone, with the options and a recommendation. Your answer goes to the task as yours. |

The investigator prefers the way that leaves the code better: make the change easy, then make the easy change.

Each decision is recorded with who took it: the investigator, the lead or you, never the task itself. `kebbie obstacles` lists them, and the task's note keeps them.

## One investigation per obstacle

The investigator is the most expensive reader kebbie runs, so it runs once per obstacle, up to 15 minutes. The same obstacle raised again gets its decision back. A second obstacle on the same piece of work, or a task's third, goes to you as one question: does the plan hold?

It does not run for a question of method, which is the task's own call, nor for red CI after a push, a conflict in a push or a fault in kebbie, which have their own paths. With [`investigate.profile`](../reference/configuration.md#investigation) set to `"none"` it never runs, and obstacles go to the lead, which decides them with `kebbie obstacles decide`. An investigator that gives no decision leaves its obstacle to the lead the same way.

## Review findings are resolved, not passed along

Bugs, broken rules, missing tests and missing docs go to the task as work. A finding about scope or intent goes to the investigator first, since the answer is often your own words in the task's note, which the reviewer never sees. When it settles a verdict that only asked for confirmation, the review passes on its word. What it cannot settle reaches you under Needs you as the review's own question.

A finding seen in a second task is raised once, as its own problem, with the lead's recommendation.

## The plan is living

A task plans what it knows, builds, learns and revises. It tells the lead each revision with its reason. A revision that changes what you get, such as a part of a plan you approved left out, is asked before it is built.

## The card asks you nothing

What needed you was asked before it was built, so the sign-off card has three parts and no confirm list:

- **Built**: what you can do now and where to see it, including anything changed outside the repo.
- **Decisions**: each choice taken on the way, its reason and who took it: you, the lead, the task or the investigator.
- **Also noticed**: only what lies outside the task. kebbie files each line under Found by tasks on the board's backlog, and the line ends with its item, `(found:f3)`.

The part labels are English; the text under them follows your vault's language. The procedure is in the skill [`kebbie:playbook`](../../skills/playbook/SKILL.md).

## What tasks notice is counted, not filed twice

Found by tasks, at the top of the board's backlog, holds what tasks listed under Also noticed: one item per thing, with the tasks that found it and how often. When a second task notices the same thing, the item counts up and adds that task, instead of a new item beside it, so something every task trips over stands out. From it you start a task, hand it to a running task, or dismiss it. See [In the Backlog column](../reference/board.md#in-the-backlog-column).
