# Obstacles and findings

This page explains what happens when a task meets something in its way, or a review finds something, and why the task's sign-off card asks you nothing.

## A task stops at an obstacle instead of going around it

An obstacle is something on the task's own path: a precondition its work needs is missing, a step of its plan cannot be built as planned, a trap makes it unsafe, or it goes only by a workaround. A red test the task did not cause, and a check it cannot run, count too.

The task stops that piece of work, writes the obstacle down with what it read and every way it sees, and raises it to the lead. It goes on with work the obstacle does not touch, or shows as blocked.

## An investigator decides, in line with the task's goal

The lead never answers an obstacle from memory. It has an investigator look into it: an agent running the skill [`kebbie:investigate`](../../skills/investigate/SKILL.md), with the same tools as the lead (the code, the vault, the web, kebbie's commands). It reads what the task is for, the code the obstacle names, what the vault already knows, and a tool's own documentation, then decides one of four:

| Decision | When | What happens |
| --- | --- | --- |
| `workaround` | What it goes around changes anyway, or the real fix is out of proportion and the workaround stays contained | The task goes on. The decision names what replaces the workaround, or files the real fix in the backlog. |
| `subtask` | The missing piece belongs to this task and is small next to it | The task builds it first, ahead of what it prepares. One per obstacle needs no answer from you. |
| `task` | The missing piece is work of its own, or the same obstacle was met before | The lead starts or points to that task, and the blocked work waits for it. |
| `ask` | The trade-off is yours, or the investigator is unsure | The task asks you on the board, with the options and a recommendation. |

The investigator prefers the way that leaves the code better: make the change easy, then make the easy change.

Today the investigator runs as a subagent of the lead, with high reasoning effort on the lead's model. While you are away it works as always; only an `ask` waits for you.

## Review findings are resolved, not passed along

Bugs, broken rules, missing tests and missing docs go to the task as work. A finding about scope or intent goes to the investigator first, since the answer is often your own words in the task's note, which the reviewer never sees. A finding whose every line asks for your confirmation still reaches you under Needs you, and the lead brings what the investigator found beside it.

A finding seen in a second task is raised once, as its own problem, with the lead's recommendation.

## The plan is living

A task plans what it knows, builds, learns and revises. It tells the lead each revision with its reason. A revision that changes what you get, such as a part of a plan you approved left out, is asked before it is built.

## The card asks you nothing

What needed you was asked before it was built, so the sign-off card has three parts and no confirm list:

- **Built**: what you can do now and where to see it, including anything changed outside the repo.
- **Decisions**: each choice taken on the way, its reason and who took it: you, the lead, the task or the investigator.
- **Also noticed**: only what lies outside the task, each with where it is filed.

The part labels are English; the text under them follows your vault's language. The procedure is in the skill [`kebbie:playbook`](../../skills/playbook/SKILL.md).
